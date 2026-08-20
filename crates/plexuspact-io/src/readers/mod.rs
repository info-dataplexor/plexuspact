//! Format-specific [`crate::BatchSource`] implementations.

pub mod csv;
pub mod json;
pub mod ndjson;
pub mod parquet;

use polars::prelude::{Column, DataFrame, DataType};

/// Casts every non-`String` column of a frame to `String` (stringly mode for
/// NDJSON/JSON, where the parser infers native JSON types).
///
/// Columns that cannot be cast (nested lists/structs) are left unchanged;
/// nested values are not supported as contract columns and are documented as
/// such in the crate docs.
pub(crate) fn stringify_columns(df: DataFrame) -> DataFrame {
    let cols: Vec<Column> = df
        .take_columns()
        .into_iter()
        .map(|c| {
            if c.dtype() == &DataType::String {
                c
            } else {
                match c.cast(&DataType::String) {
                    Ok(s) => s,
                    Err(_) => c,
                }
            }
        })
        .collect();
    // Columns keep their original lengths, so this cannot fail; fall back to
    // an empty frame defensively rather than panicking.
    DataFrame::new(cols).unwrap_or_default()
}
