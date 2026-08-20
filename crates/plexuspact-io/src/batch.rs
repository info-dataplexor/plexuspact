//! The [`BatchSource`] streaming interface.

use polars::prelude::{DataFrame, Schema};

use crate::error::IoError;

/// How the yielded columns are typed (see crate-level docs on type handling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputTyping {
    /// All columns are `String`; the engine casts to declared dtypes per batch
    /// and captures per-row cast failures (CSV/NDJSON/JSON check path).
    Stringly,
    /// Columns carry native dtypes (Parquet always; text formats when opened
    /// with [`crate::TypingMode::Inferred`]). Declared-vs-actual dtype
    /// mismatches are structural, file-level failures.
    Native,
}

/// A streaming source of row batches.
///
/// Constant-memory contract: implementations hold at most one batch (plus
/// bounded parser state) in memory at a time. The documented exception is JSON
/// arrays, which cannot be streamed and are parsed whole (see
/// [`crate::readers::json`]).
pub trait BatchSource: Send {
    /// Schema of the yielded frames (available before the first batch).
    fn schema(&self) -> &Schema;

    /// How the columns are typed.
    fn typing(&self) -> InputTyping;

    /// Yields the next batch, or `None` at end of input.
    ///
    /// Batches may be smaller than any batch, but subsequent batches never
    /// contain rows out of order: concatenating all batches reproduces the
    /// source row order (row numbers in failure samples depend on this).
    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError>;

    /// Source size in bytes when known (files); `None` for streams.
    fn size_bytes(&self) -> Option<u64> {
        None
    }
}
