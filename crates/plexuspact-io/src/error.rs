//! Errors produced while opening and reading input data.
//!
//! Error-message rule (implementation guide §0): every message says what was
//! expected and suggests one concrete fix.

use thiserror::Error;

/// Errors from the input layer.
#[derive(Debug, Error)]
pub enum IoError {
    /// The input file does not exist or cannot be opened.
    #[error("cannot open `{path}`: {source}; check that the file exists and the path is spelled correctly")]
    MissingFile {
        /// Path that failed to open.
        path: String,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// The input is empty (zero bytes, or no records at all).
    #[error("`{path}` is empty: expected at least a header/record; point plexuspact at a non-empty file")]
    EmptyInput {
        /// Path (or `<stdin>`) that was empty.
        path: String,
    },

    /// The input format could not be detected from the file extension.
    #[error(
        "cannot detect the input format of `{path}`: expected one of the extensions \
         .csv, .tsv, .parquet, .ndjson, .jsonl, .json, .xlsx, .xls, .ods, .xml, .fwf \
         (optionally with .gz or .zst); pass --input-format \
         csv|tsv|parquet|ndjson|json|excel|xml|fixed_width to set it explicitly, \
         or pin it in the contract under `settings.input.format`"
    )]
    UndetectableFormat {
        /// Path whose extension was not recognized.
        path: String,
    },

    /// Stdin was requested but no explicit format was given.
    #[error(
        "reading from stdin requires an explicit format: pass \
         --input-format csv|tsv|ndjson|json|excel|xml|fixed_width \
         (parquet is not supported on stdin)"
    )]
    StdinNeedsFormat,

    /// A format that requires random access was requested on stdin.
    #[error(
        "parquet cannot be read from stdin (the format needs random access to its footer); \
         write the data to a file first and pass the file path"
    )]
    ParquetOnStdin,

    /// Decompression failed.
    #[error(
        "failed to decompress `{path}` as {codec}: {source}; \
         check that the file is a valid {codec} archive (or rename it if the extension is wrong)"
    )]
    Decompress {
        /// Path being decompressed.
        path: String,
        /// Codec name (`gzip` or `zstd`).
        codec: &'static str,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// The file content could not be parsed as the (detected or forced) format.
    #[error(
        "malformed {format} in `{path}`{position}: {detail}; \
         check the file content, or pass --input-format if the format was mis-detected"
    )]
    Malformed {
        /// Path being read.
        path: String,
        /// Format that was being parsed.
        format: &'static str,
        /// Position hint like ` (around record 12000)`, or empty.
        position: String,
        /// Parser error detail.
        detail: String,
    },

    /// `--json-path` was given for a non-JSON input format.
    #[error(
        "--json-path only applies to JSON input, but `{path}` is being read as {format}; \
         drop --json-path, or pass --input-format json if the data really is JSON"
    )]
    JsonPathWrongFormat {
        /// Path being read.
        path: String,
        /// The format that was detected or forced.
        format: &'static str,
    },

    /// The JSON document could not be walked to the requested `--json-path`.
    #[error(
        "--json-path `{pointer}` did not resolve in `{path}`: {detail}; \
         check the path against the response shape (dotted keys, e.g. `data.items`)"
    )]
    JsonPath {
        /// Path being read.
        path: String,
        /// The user-supplied json-path.
        pointer: String,
        /// What specifically went wrong (missing key, wrong type, …).
        detail: String,
    },

    /// Fixed-width input was requested without a field layout.
    #[error(
        "`{path}` is fixed-width text but no field layout was given; \
         put the layout in the contract under `settings.input.fixed_width` \
         (name + start/end or width per field), or pass --fixed-width \
         \"id=1-8,name=9-40,amount=12\""
    )]
    FixedWidthNeedsLayout {
        /// Path being read.
        path: String,
    },

    /// The requested worksheet is not in the workbook.
    #[error(
        "`{path}` has no sheet `{sheet}` (it has: {available}); \
         set `settings.input.sheet` (or --sheet) to one of those names, or to its 1-based position"
    )]
    SheetNotFound {
        /// Path being read.
        path: String,
        /// The sheet that was asked for.
        sheet: String,
        /// The workbook's sheet names, comma-separated.
        available: String,
    },

    /// No record element matched in the XML document.
    #[error(
        "no `{record}` records found in `{path}` (elements seen: {seen}); \
         set `settings.input.xml_record` (or --xml-record) to the element that is one record, \
         as a name (`order`) or a path from the root (`orders/order`)"
    )]
    XmlRecordNotFound {
        /// Path being read.
        path: String,
        /// The record element that was looked for (`<auto>` when none was set).
        record: String,
        /// Element paths seen near the top of the document.
        seen: String,
    },

    /// A read option is not usable as given (`--delimiter ";;"`, a fixed-width
    /// spec that does not parse, …).
    #[error("{key}: {detail}")]
    InvalidOption {
        /// The option, as the user names it (`--delimiter`, `settings.input.fixed_width`).
        key: String,
        /// What is wrong and what would be right.
        detail: String,
    },

    /// A temp file could not be created (stdin buffering, decompression).
    #[error("failed to create a temporary file for buffering: {source}; check free disk space and TMP permissions")]
    TempFile {
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

impl IoError {
    /// Builds a [`IoError::Malformed`] from a Polars error, extracting nothing
    /// fancy — the polars message plus our path/format context.
    pub(crate) fn malformed(
        path: &str,
        format: &'static str,
        position: Option<String>,
        err: polars::error::PolarsError,
    ) -> Self {
        IoError::Malformed {
            path: path.to_string(),
            format,
            position: position.map(|p| format!(" ({p})")).unwrap_or_default(),
            detail: err.to_string(),
        }
    }

    /// Builds a [`IoError::Malformed`] from an already-stringified detail (e.g.
    /// a `serde_json` parse error) with the given path/format context.
    pub(crate) fn malformed_str(path: &str, format: &'static str, detail: &str) -> Self {
        IoError::Malformed {
            path: path.to_string(),
            format,
            position: String::new(),
            detail: detail.to_string(),
        }
    }
}
