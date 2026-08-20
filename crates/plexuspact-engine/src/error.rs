//! Engine error type.

use plexuspact_io::IoError;

/// Errors raised while planning or executing checks.
///
/// These are *engine* failures (unreadable input, an internal Polars error, a
/// malformed `custom_expr`), never ordinary check failures — a violated check
/// is a normal [`crate::CheckOutcome`], not an error.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// The input source could not be read.
    #[error(transparent)]
    Io(#[from] IoError),

    /// A declared column holds nested data (arrays/objects) that the engine
    /// can't validate as a flat column. Actionable, not an internal error.
    #[error("column `{column}` contains nested {kind} values; plexuspact validates flat, tabular columns — select or flatten the field before validating (e.g. extract the record array, or map the nested field to a scalar)")]
    NestedColumn {
        /// The offending column name.
        column: String,
        /// `array` or `object`.
        kind: &'static str,
    },

    /// An internal Polars operation failed unexpectedly.
    #[error("internal data-processing error: {0}")]
    Polars(#[from] polars::error::PolarsError),
}
