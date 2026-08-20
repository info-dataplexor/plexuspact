//! Input format and compression detection.
//!
//! Detection is extension-based for files (`.csv`, `.tsv`, `.parquet`,
//! `.ndjson`, `.jsonl`, `.json`, each optionally followed by `.gz` or `.zst`).
//! Stdin has no extension: compression is sniffed from magic bytes and the
//! format must be given explicitly (see [`crate::IoError::StdinNeedsFormat`]).

use std::path::Path;

use crate::error::IoError;

/// A supported input data format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputFormat {
    /// Comma-separated values (delimiter configurable).
    Csv,
    /// Tab-separated values (CSV with `\t` as the default delimiter).
    Tsv,
    /// Apache Parquet.
    Parquet,
    /// Newline-delimited JSON (one JSON object per line).
    Ndjson,
    /// A single JSON array of objects. Not streamable — parsed whole.
    Json,
}

impl InputFormat {
    /// Stable lowercase name, used in `RunResult.source.format`.
    pub fn name(self) -> &'static str {
        match self {
            InputFormat::Csv => "csv",
            InputFormat::Tsv => "tsv",
            InputFormat::Parquet => "parquet",
            InputFormat::Ndjson => "ndjson",
            InputFormat::Json => "json",
        }
    }

    /// Parses a format name as given to `--input-format`.
    pub fn from_name(s: &str) -> Option<InputFormat> {
        match s.to_ascii_lowercase().as_str() {
            "csv" => Some(InputFormat::Csv),
            "tsv" => Some(InputFormat::Tsv),
            "parquet" => Some(InputFormat::Parquet),
            "ndjson" | "jsonl" => Some(InputFormat::Ndjson),
            "json" => Some(InputFormat::Json),
            _ => None,
        }
    }
}

/// Transparent compression codec of the input container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Not compressed.
    None,
    /// gzip (`.gz`, magic `1f 8b`).
    Gzip,
    /// Zstandard (`.zst`, magic `28 b5 2f fd`).
    Zstd,
}

/// Maps one extension component to a format, if known.
fn format_for_ext(ext: &str) -> Option<InputFormat> {
    match ext.to_ascii_lowercase().as_str() {
        "csv" => Some(InputFormat::Csv),
        "tsv" => Some(InputFormat::Tsv),
        "parquet" => Some(InputFormat::Parquet),
        "ndjson" | "jsonl" => Some(InputFormat::Ndjson),
        "json" => Some(InputFormat::Json),
        _ => None,
    }
}

/// Maps one extension component to a compression codec, if known.
fn compression_for_ext(ext: &str) -> Option<Compression> {
    match ext.to_ascii_lowercase().as_str() {
        "gz" | "gzip" => Some(Compression::Gzip),
        "zst" | "zstd" => Some(Compression::Zstd),
        _ => None,
    }
}

/// Detects `(format, compression)` from a file path.
///
/// Compound extensions like `data.csv.gz` are handled: the outermost extension
/// names the compression, the next one the format. `format_override` (from
/// `--input-format`) replaces the detected *inner* format but never the
/// compression.
pub fn detect(
    path: &Path,
    format_override: Option<InputFormat>,
) -> Result<(InputFormat, Compression), IoError> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut parts = file_name.rsplit('.');
    let last = parts.next().unwrap_or_default();

    let (compression, format_ext) = match compression_for_ext(last) {
        Some(c) => (c, parts.next().map(str::to_string)),
        None => (Compression::None, Some(last.to_string())),
    };

    if let Some(fmt) = format_override {
        return Ok((fmt, compression));
    }

    let format = format_ext
        .as_deref()
        .and_then(format_for_ext)
        .ok_or_else(|| IoError::UndetectableFormat {
            path: path.display().to_string(),
        })?;
    Ok((format, compression))
}

/// Sniffs a compression codec from the first bytes of a stream (used for stdin,
/// which has no extension).
pub fn sniff_compression(magic: &[u8]) -> Compression {
    if magic.starts_with(&[0x1f, 0x8b]) {
        Compression::Gzip
    } else if magic.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Compression::Zstd
    } else {
        Compression::None
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::path::PathBuf;

    fn det(p: &str) -> Result<(InputFormat, Compression), IoError> {
        detect(&PathBuf::from(p), None)
    }

    #[test]
    fn plain_extensions() {
        assert_eq!(det("a.csv").unwrap(), (InputFormat::Csv, Compression::None));
        assert_eq!(det("a.tsv").unwrap(), (InputFormat::Tsv, Compression::None));
        assert_eq!(
            det("a.parquet").unwrap(),
            (InputFormat::Parquet, Compression::None)
        );
        assert_eq!(
            det("a.ndjson").unwrap(),
            (InputFormat::Ndjson, Compression::None)
        );
        assert_eq!(
            det("a.jsonl").unwrap(),
            (InputFormat::Ndjson, Compression::None)
        );
        assert_eq!(
            det("a.json").unwrap(),
            (InputFormat::Json, Compression::None)
        );
        assert_eq!(det("A.CSV").unwrap(), (InputFormat::Csv, Compression::None));
    }

    #[test]
    fn compound_extensions() {
        assert_eq!(
            det("a.csv.gz").unwrap(),
            (InputFormat::Csv, Compression::Gzip)
        );
        assert_eq!(
            det("a.ndjson.zst").unwrap(),
            (InputFormat::Ndjson, Compression::Zstd)
        );
        assert_eq!(
            det("dir.v2/a.jsonl.gz").unwrap(),
            (InputFormat::Ndjson, Compression::Gzip)
        );
    }

    #[test]
    fn override_wins_but_keeps_compression() {
        let (f, c) = detect(&PathBuf::from("a.dat.gz"), Some(InputFormat::Csv)).unwrap();
        assert_eq!((f, c), (InputFormat::Csv, Compression::Gzip));
    }

    #[test]
    fn unknown_extension_is_actionable() {
        let e = det("data.xlsx").unwrap_err().to_string();
        assert!(e.contains(".csv"), "{e}");
        assert!(e.contains(".parquet"), "{e}");
        assert!(e.contains("--input-format"), "{e}");
    }

    #[test]
    fn bare_gz_is_undetectable() {
        assert!(det("data.gz").is_err());
    }

    #[test]
    fn magic_bytes() {
        assert_eq!(sniff_compression(&[0x1f, 0x8b, 0x08]), Compression::Gzip);
        assert_eq!(
            sniff_compression(&[0x28, 0xb5, 0x2f, 0xfd, 0x00]),
            Compression::Zstd
        );
        assert_eq!(sniff_compression(b"user_id,email"), Compression::None);
        assert_eq!(sniff_compression(&[]), Compression::None);
    }

    #[test]
    fn from_name_parses() {
        assert_eq!(InputFormat::from_name("CSV"), Some(InputFormat::Csv));
        assert_eq!(InputFormat::from_name("jsonl"), Some(InputFormat::Ndjson));
        assert_eq!(InputFormat::from_name("xls"), None);
    }
}
