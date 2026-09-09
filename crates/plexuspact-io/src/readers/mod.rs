//! Format-specific [`crate::BatchSource`] implementations.

pub mod csv;
pub mod excel;
pub mod fixed_width;
pub mod json;
pub mod ndjson;
pub mod parquet;
pub mod xml;

use polars::prelude::{Column, DataFrame, DataType, NamedFrom, Series};

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

/// Builds a stringly frame from row-major text: one `String` column per
/// name, in order. A short row is padded with nulls; a long one is cut.
///
/// This is the common tail of the readers that assemble values themselves
/// (Excel, XML, fixed-width) rather than delegating to a Polars parser.
pub(crate) fn frame_from_text_columns(
    names: &[String],
    rows: Vec<Vec<Option<String>>>,
) -> DataFrame {
    let mut columns: Vec<Vec<Option<String>>> = (0..names.len())
        .map(|_| Vec::with_capacity(rows.len()))
        .collect();
    for row in rows {
        let mut cells = row.into_iter();
        for column in columns.iter_mut() {
            column.push(cells.next().flatten());
        }
    }
    let cols: Vec<Column> = names
        .iter()
        .zip(columns)
        .map(|(name, values)| Column::from(Series::new(name.as_str().into(), values)))
        .collect();
    // Every column has exactly `rows.len()` values and the names are unique
    // by construction in each caller, so this cannot fail.
    DataFrame::new(cols).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn text_columns_pad_and_cut() {
        let names = vec!["a".to_owned(), "b".to_owned()];
        let rows = vec![
            vec![Some("1".to_owned())],
            vec![Some("2".to_owned()), None, Some("extra".to_owned())],
        ];
        let df = frame_from_text_columns(&names, rows);
        assert_eq!(df.shape(), (2, 2));
        assert_eq!(df.column("b").unwrap().null_count(), 2);
        assert_eq!(df.column("a").unwrap().dtype(), &DataType::String);
    }

    #[test]
    fn text_columns_with_no_rows() {
        let names = vec!["a".to_owned()];
        let df = frame_from_text_columns(&names, Vec::new());
        assert_eq!(df.shape(), (0, 1));
    }
}
