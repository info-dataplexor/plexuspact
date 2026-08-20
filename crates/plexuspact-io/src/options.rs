//! Read options shared by all readers.

use crate::format::InputFormat;

/// Default number of rows per streamed batch.
pub const DEFAULT_BATCH_ROWS: usize = 50_000;

/// Character encoding handling for CSV input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CsvEncoding {
    /// Strict UTF-8; invalid bytes are an error.
    #[default]
    Utf8,
    /// Invalid UTF-8 bytes are replaced with `�`.
    Utf8Lossy,
}

/// CSV/TSV parsing options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CsvOptions {
    /// Field delimiter. `None` = `,` for CSV, `\t` for TSV.
    pub delimiter: Option<u8>,
    /// Quote character (default `"`). `None` disables quoting.
    pub quote_char: Option<u8>,
    /// Whether the first record is a header row (default `true`).
    pub has_header: bool,
    /// Encoding handling.
    pub encoding: CsvEncoding,
}

impl Default for CsvOptions {
    fn default() -> Self {
        CsvOptions {
            delimiter: None,
            quote_char: Some(b'"'),
            has_header: true,
            encoding: CsvEncoding::Utf8,
        }
    }
}

impl CsvOptions {
    /// Effective delimiter for the given format.
    pub fn effective_delimiter(&self, format: InputFormat) -> u8 {
        self.delimiter.unwrap_or(match format {
            InputFormat::Tsv => b'\t',
            _ => b',',
        })
    }
}

/// How columns are typed in the yielded batches (see crate docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TypingMode {
    /// Text formats deliver every column as `String`; the engine casts and
    /// counts per-row type mismatches (check path). Parquet ignores this and
    /// always delivers native dtypes.
    #[default]
    Stringly,
    /// Deliver inferred/native dtypes (profiling path for `init`).
    Inferred,
}

/// Options for opening a [`crate::BatchSource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadOptions {
    /// Explicit format (`--input-format`); `None` = detect from extension.
    pub format: Option<InputFormat>,
    /// CSV/TSV options.
    pub csv: CsvOptions,
    /// Target rows per batch (readers may deliver slightly different sizes).
    pub batch_rows: usize,
    /// Stringly (check path) or inferred (profiling) typing.
    pub typing: TypingMode,
    /// For JSON input only: a dotted path to the record array nested inside a
    /// wrapping object (e.g. `results`, `data.items`). `None` = the document is
    /// itself the array. Leading `.`/`$.` are tolerated. See `--json-path`.
    pub json_path: Option<String>,
}

impl Default for ReadOptions {
    fn default() -> Self {
        ReadOptions {
            format: None,
            csv: CsvOptions::default(),
            batch_rows: DEFAULT_BATCH_ROWS,
            typing: TypingMode::Stringly,
            json_path: None,
        }
    }
}
