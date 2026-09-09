//! Streaming CSV/TSV reader built on the Polars batched CSV reader.
//!
//! In [`TypingMode::Stringly`] every column is read as `String`
//! (`infer_schema_length = 0`), so the engine can cast to declared dtypes and
//! capture per-row mismatches. In [`TypingMode::Inferred`] (profiling) a schema
//! is inferred from a prefix of the file and then pinned for all batches;
//! values that do not fit the pinned dtype become nulls (`ignore_errors`) —
//! acceptable for profiling, never used in the check path.

use std::fs::File;

use polars::prelude::*;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::format::InputFormat;
use crate::options::{CsvEncoding as OurEncoding, ReadOptions, TypingMode};

/// Rows scanned for schema inference in `Inferred` mode.
const INFER_ROWS: usize = 10_000;

/// Streaming CSV batch source.
pub struct CsvBatchSource {
    batched: OwnedBatchedCsvReader,
    schema: Schema,
    typing: InputTyping,
    size_bytes: Option<u64>,
}

impl std::fmt::Debug for CsvBatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CsvBatchSource").finish_non_exhaustive()
    }
}

/// Builds the Polars parse options from ours.
fn parse_options(opts: &ReadOptions, format: InputFormat) -> CsvParseOptions {
    CsvParseOptions::default()
        .with_separator(opts.csv.effective_delimiter(format))
        .with_quote_char(opts.csv.quote_char)
        .with_encoding(match opts.csv.encoding {
            OurEncoding::Utf8 => CsvEncoding::Utf8,
            OurEncoding::Utf8Lossy => CsvEncoding::LossyUtf8,
        })
        .with_try_parse_dates(matches!(opts.typing, TypingMode::Inferred))
}

/// Builds the Polars read options (without schema pinning).
fn read_options(opts: &ReadOptions, format: InputFormat) -> CsvReadOptions {
    let infer = match opts.typing {
        // 0 = do not infer: every column is String.
        TypingMode::Stringly => Some(0),
        TypingMode::Inferred => Some(INFER_ROWS),
    };
    CsvReadOptions::default()
        .with_has_header(opts.csv.has_header)
        .with_skip_rows(opts.csv.skip_rows)
        .with_infer_schema_length(infer)
        .with_chunk_size(opts.batch_rows.max(1))
        .with_ignore_errors(matches!(opts.typing, TypingMode::Inferred))
        .with_parse_options(parse_options(opts, format))
}

impl CsvBatchSource {
    /// Opens a CSV/TSV source from an already-decompressed file handle.
    ///
    /// `path_display` is only used in error messages; `size_bytes` is the
    /// original on-disk size when known.
    pub fn from_file(
        file: File,
        format: InputFormat,
        opts: &ReadOptions,
        path_display: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let map_err = |e: PolarsError| map_csv_error(path_display, e);

        // Probe read: header + inference prefix only. Establishes the schema
        // (needed before the first batch for structural checks) and turns an
        // empty file into a friendly error.
        let probe_handle = file.try_clone().map_err(|source| IoError::MissingFile {
            path: path_display.to_string(),
            source,
        })?;
        let probe_rows = match opts.typing {
            TypingMode::Stringly => 0,
            TypingMode::Inferred => INFER_ROWS,
        };
        let probe = read_options(opts, format)
            .with_n_rows(Some(probe_rows))
            .into_reader_with_file_handle(probe_handle)
            .finish()
            .map_err(map_err)?;
        let schema: Schema = probe.schema().as_ref().clone();

        // Batched reader over the full file, pinned to the probed schema so
        // every batch matches `schema()` exactly.
        let handle: Box<dyn polars::io::mmap::MmapBytesReader> = Box::new(file);
        let batched = read_options(opts, format)
            .into_reader_with_file_handle(handle)
            .batched(Some(Arc::new(schema.clone())))
            .map_err(map_err)?;

        Ok(CsvBatchSource {
            batched,
            schema,
            typing: match opts.typing {
                TypingMode::Stringly => InputTyping::Stringly,
                TypingMode::Inferred => InputTyping::Native,
            },
            size_bytes,
        })
    }
}

/// Maps a Polars CSV error to an [`IoError`] with context.
fn map_csv_error(path: &str, e: PolarsError) -> IoError {
    let text = e.to_string();
    if text.contains("empty CSV") || text.contains("no data") {
        return IoError::EmptyInput {
            path: path.to_string(),
        };
    }
    IoError::malformed(path, "csv", None, e)
}

impl BatchSource for CsvBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        self.typing
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        let batches = self
            .batched
            .next_batches(1)
            .map_err(|e| IoError::malformed("<csv>", "csv", None, e))?;
        Ok(batches.and_then(|mut v| {
            if v.is_empty() {
                None
            } else {
                Some(v.remove(0))
            }
        }))
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}
