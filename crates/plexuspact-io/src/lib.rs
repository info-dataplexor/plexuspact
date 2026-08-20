//! # plexuspact-io
//!
//! Input abstraction for the check engine (doc 03 §1): `Source` resolution,
//! format detection, transparent decompression, and streaming readers that
//! yield Polars [`DataFrame`](polars::prelude::DataFrame) batches.
//!
//! ## Streaming model
//!
//! Every reader implements [`BatchSource`]: an iterator-style interface that
//! yields row batches plus a schema accessor. The check path never materializes
//! the whole file (NFR-2, constant memory), with one documented exception:
//! JSON *arrays* are not streamable — the file is parsed whole and then split
//! into batches (see [`readers::json`]).
//!
//! ## Type handling (doc 03 §5 "Type handling")
//!
//! Text formats (CSV, NDJSON, JSON) are read in **stringly** mode for the check
//! path: every column is delivered as a `String` column. The engine casts each
//! contract column to its declared dtype per batch, which lets it *count* cast
//! failures per row and capture the offending raw values as samples. Reading
//! with forced dtypes at the parser level would silently lose those values.
//!
//! Parquet is already typed ([`InputTyping::Native`]): a declared-vs-actual
//! dtype mismatch there is a structural, file-level failure without row
//! capture — a wrongly-typed value cannot exist inside a typed Parquet column.
//! The engine documents and tests this asymmetry (ADR-008).
//!
//! For profiling (`init`), readers run in [`TypingMode::Inferred`] and deliver
//! natively inferred dtypes instead.
//!
//! ## Datetime convention
//!
//! Declared `datetime` columns map to `Datetime(µs, UTC)`; naive datetime
//! strings are assumed UTC. Declared `date` maps to `Date`. See [`dtypes`].

pub mod batch;
pub mod dtypes;
pub mod error;
pub mod format;
pub mod open;
pub mod options;
pub mod readers;
pub mod source;

pub use batch::{BatchSource, InputTyping};
pub use dtypes::{dtype_for, schema_from_contract};
pub use error::IoError;
pub use format::{detect, sniff_compression, Compression, InputFormat};
pub use open::{open, open_reader};
pub use options::{CsvEncoding, CsvOptions, ReadOptions, TypingMode};
pub use source::{resolve, Source};
