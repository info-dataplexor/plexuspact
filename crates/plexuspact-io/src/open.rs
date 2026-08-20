//! Opening a [`Source`] as a [`BatchSource`]: format detection, transparent
//! decompression, and stdin handling.
//!
//! Decompression strategy: NDJSON is decompressed *streaming* (constant
//! memory). Formats that need random access (CSV batched reader, Parquet
//! footer, JSON whole-parse) are decompressed to a temporary file first; the
//! temp file lives as long as the returned source.
//!
//! Stdin rules: an explicit format is required (no extension to sniff);
//! compression is sniffed from magic bytes; Parquet on stdin is rejected
//! (needs random access to the footer); CSV/JSON from stdin are buffered to a
//! temp file; NDJSON streams directly.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use flate2::read::MultiGzDecoder;
use polars::prelude::{DataFrame, Schema};
use tempfile::NamedTempFile;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::format::{detect, sniff_compression, Compression, InputFormat};
use crate::options::ReadOptions;
use crate::readers::csv::CsvBatchSource;
use crate::readers::json::JsonBatchSource;
use crate::readers::ndjson::NdjsonBatchSource;
use crate::readers::parquet::ParquetBatchSource;
use crate::source::Source;

/// Keeps a temp file alive for as long as its reader.
struct WithTempGuard {
    inner: Box<dyn BatchSource>,
    _temp: NamedTempFile,
}

impl BatchSource for WithTempGuard {
    fn schema(&self) -> &Schema {
        self.inner.schema()
    }
    fn typing(&self) -> InputTyping {
        self.inner.typing()
    }
    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        self.inner.next_batch()
    }
    fn size_bytes(&self) -> Option<u64> {
        self.inner.size_bytes()
    }
}

/// Opens a resolved [`Source`] with the given options.
///
/// For [`Source::Stdin`] this reads the real process stdin; tests and library
/// callers can use [`open_reader`] to inject any stream instead.
pub fn open(source: &Source, opts: &ReadOptions) -> Result<Box<dyn BatchSource>, IoError> {
    match source {
        Source::File(path) => open_file(path, opts),
        Source::Stdin => {
            let format = opts.format.ok_or(IoError::StdinNeedsFormat)?;
            open_reader(Box::new(std::io::stdin()), format, opts, "-")
        }
    }
}

/// Opens a file path (detects format + compression from the extension).
fn open_file(path: &Path, opts: &ReadOptions) -> Result<Box<dyn BatchSource>, IoError> {
    let display = path.display().to_string();
    let (format, compression) = detect(path, opts.format)?;
    ensure_json_path_applicable(format, opts, &display)?;
    let file = File::open(path).map_err(|source| IoError::MissingFile {
        path: display.clone(),
        source,
    })?;
    let size = file.metadata().ok().map(|m| m.len());
    if size == Some(0) {
        return Err(IoError::EmptyInput { path: display });
    }

    match compression {
        Compression::None => open_plain_file(file, format, opts, &display, size),
        Compression::Gzip | Compression::Zstd => {
            if format == InputFormat::Ndjson {
                // Stream-decompress NDJSON: constant memory, no temp file.
                let (reader, codec) =
                    decompress_stream(Box::new(BufReader::new(file)), compression, &display)?;
                Ok(Box::new(NdjsonBatchSource::from_reader(
                    reader,
                    opts.batch_rows,
                    stringly(opts),
                    &display,
                    Some(codec),
                    size,
                )?))
            } else {
                let temp = decompress_to_temp(file, compression, &display)?;
                let inner = open_plain_file(reopen_temp(&temp)?, format, opts, &display, size)?;
                Ok(Box::new(WithTempGuard { inner, _temp: temp }))
            }
        }
    }
}

/// Opens an uncompressed file handle as the given format.
fn open_plain_file(
    file: File,
    format: InputFormat,
    opts: &ReadOptions,
    display: &str,
    size: Option<u64>,
) -> Result<Box<dyn BatchSource>, IoError> {
    match format {
        InputFormat::Csv | InputFormat::Tsv => Ok(Box::new(CsvBatchSource::from_file(
            file, format, opts, display, size,
        )?)),
        InputFormat::Parquet => Ok(Box::new(ParquetBatchSource::from_file(
            file,
            opts.batch_rows,
            display,
            size,
        )?)),
        InputFormat::Ndjson => Ok(Box::new(NdjsonBatchSource::from_reader(
            Box::new(BufReader::new(file)),
            opts.batch_rows,
            stringly(opts),
            display,
            None,
            size,
        )?)),
        InputFormat::Json => Ok(Box::new(JsonBatchSource::from_file(
            file,
            opts.batch_rows,
            stringly(opts),
            display,
            size,
            opts.json_path.as_deref(),
        )?)),
    }
}

/// `--json-path` is meaningful only for JSON input; reject it on any other
/// format up front so the user gets a targeted error, not a silent no-op.
fn ensure_json_path_applicable(
    format: InputFormat,
    opts: &ReadOptions,
    display: &str,
) -> Result<(), IoError> {
    if opts.json_path.is_some() && format != InputFormat::Json {
        return Err(IoError::JsonPathWrongFormat {
            path: display.to_string(),
            format: format.name_static(),
        });
    }
    Ok(())
}

/// Opens an arbitrary byte stream (stdin, tests) as the given format.
///
/// Compression is sniffed from the stream's magic bytes. Parquet is rejected:
/// it needs random access to its footer.
pub fn open_reader(
    reader: Box<dyn Read + Send>,
    format: InputFormat,
    opts: &ReadOptions,
    display: &str,
) -> Result<Box<dyn BatchSource>, IoError> {
    if format == InputFormat::Parquet {
        return Err(IoError::ParquetOnStdin);
    }
    ensure_json_path_applicable(format, opts, display)?;
    let mut buffered = BufReader::new(reader);
    let magic = buffered.fill_buf().map_err(|source| IoError::Malformed {
        path: display.to_string(),
        format: format.name_static(),
        position: String::new(),
        detail: source.to_string(),
    })?;
    if magic.is_empty() {
        return Err(IoError::EmptyInput {
            path: display.to_string(),
        });
    }
    let compression = sniff_compression(magic);
    let (decoded, codec): (Box<dyn BufRead + Send>, Option<&'static str>) = match compression {
        Compression::None => (Box::new(buffered), None),
        other => {
            let (r, codec) = decompress_stream(Box::new(buffered), other, display)?;
            (r, Some(codec))
        }
    };

    match format {
        InputFormat::Ndjson => Ok(Box::new(NdjsonBatchSource::from_reader(
            decoded,
            opts.batch_rows,
            stringly(opts),
            display,
            codec,
            None,
        )?)),
        InputFormat::Csv | InputFormat::Tsv | InputFormat::Json => {
            // These readers need random access: buffer the stream to a temp file.
            let temp = stream_to_temp(decoded, codec, display)?;
            let inner = open_plain_file(reopen_temp(&temp)?, format, opts, display, None)?;
            Ok(Box::new(WithTempGuard { inner, _temp: temp }))
        }
        InputFormat::Parquet => Err(IoError::ParquetOnStdin),
    }
}

/// Whether the options ask for stringly typing.
fn stringly(opts: &ReadOptions) -> bool {
    matches!(opts.typing, crate::options::TypingMode::Stringly)
}

impl InputFormat {
    /// `name()` with a `'static` lifetime for error structs.
    fn name_static(self) -> &'static str {
        self.name()
    }
}

/// Wraps a buffered stream in the right decompressor.
fn decompress_stream(
    reader: Box<dyn BufRead + Send>,
    compression: Compression,
    display: &str,
) -> Result<(Box<dyn BufRead + Send>, &'static str), IoError> {
    match compression {
        Compression::Gzip => Ok((
            Box::new(BufReader::new(MultiGzDecoder::new(reader))),
            "gzip",
        )),
        Compression::Zstd => {
            let dec =
                zstd::stream::read::Decoder::new(reader).map_err(|source| IoError::Decompress {
                    path: display.to_string(),
                    codec: "zstd",
                    source,
                })?;
            Ok((Box::new(BufReader::new(dec)), "zstd"))
        }
        Compression::None => Ok((reader, "none")),
    }
}

/// Decompresses a whole file into a temp file (for formats needing seek).
fn decompress_to_temp(
    file: File,
    compression: Compression,
    display: &str,
) -> Result<NamedTempFile, IoError> {
    let (mut decoded, codec) =
        decompress_stream(Box::new(BufReader::new(file)), compression, display)?;
    let mut temp = NamedTempFile::new().map_err(|source| IoError::TempFile { source })?;
    std::io::copy(&mut decoded, temp.as_file_mut()).map_err(|source| IoError::Decompress {
        path: display.to_string(),
        codec,
        source,
    })?;
    temp.as_file_mut()
        .flush()
        .map_err(|source| IoError::TempFile { source })?;
    Ok(temp)
}

/// Buffers an arbitrary stream into a temp file.
fn stream_to_temp(
    mut reader: Box<dyn BufRead + Send>,
    codec: Option<&'static str>,
    display: &str,
) -> Result<NamedTempFile, IoError> {
    let mut temp = NamedTempFile::new().map_err(|source| IoError::TempFile { source })?;
    std::io::copy(&mut reader, temp.as_file_mut()).map_err(|source| match codec {
        Some(codec) => IoError::Decompress {
            path: display.to_string(),
            codec,
            source,
        },
        None => IoError::TempFile { source },
    })?;
    temp.as_file_mut()
        .flush()
        .map_err(|source| IoError::TempFile { source })?;
    Ok(temp)
}

/// Reopens a temp file for reading from the start.
fn reopen_temp(temp: &NamedTempFile) -> Result<File, IoError> {
    temp.reopen().map_err(|source| IoError::TempFile { source })
}
