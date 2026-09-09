//! Excel / OpenDocument workbook reader.
//!
//! A workbook is the format partners actually send: the finance team's
//! monthly extract, the supplier's price list, the hospital's census. It is
//! also the format with the least structure — data on the third sheet, two
//! title rows, a total row at the bottom, dates that are really numbers with
//! a display format. This reader takes the sheet the contract names, skips
//! the rows it says to, and renders every cell to the text a person would
//! read off the screen, so the engine's stringly type checks and failure
//! samples work exactly as they do for CSV.
//!
//! **Not streamable**: the sheet is decompressed and parsed whole by
//! `calamine` (a `.xlsx` is a zip of XML; the shared-strings table alone
//! forbids a single pass). Memory is O(file + sheet). Workbooks large enough for
//! that to matter are the wrong container anyway; the error path says so.
//!
//! Cell rendering (stringly): integers as-is; floats that are whole as
//! integers (`42`, not `42.0` — Excel stores every number as a float);
//! booleans as `true`/`false`; dates as `YYYY-MM-DD` when the time part is
//! midnight, else `YYYY-MM-DDTHH:MM:SS`; durations as `H:MM:SS`; empty cells,
//! empty strings and error cells (`#N/A`, `#REF!`) as null. A required column
//! with a `#REF!` in it fails on that row, which is the truth.

use std::fs::File;
use std::io::{Cursor, Read};
use std::sync::Arc;

use calamine::{Data, Range, Reader, Sheets};
use polars::prelude::*;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::options::ReadOptions;
use crate::readers::frame_from_text_columns;

/// Whole-sheet source with batched iteration.
#[derive(Debug)]
pub struct ExcelBatchSource {
    df: DataFrame,
    schema: Schema,
    offset: usize,
    batch_rows: usize,
    size_bytes: Option<u64>,
}

impl ExcelBatchSource {
    /// Opens a workbook and reads the configured sheet.
    pub fn from_file(
        file: File,
        opts: &ReadOptions,
        path_display: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let malformed = |detail: String| IoError::malformed_str(path_display, "excel", &detail);
        // calamine sniffs the flavour (xlsx / xls / xlsb / ods) by trying each
        // parser on a clone of the reader, so the bytes are held once behind
        // an `Arc` and the clones are free. The whole file is in memory either
        // way: a workbook is a zip that cannot be parsed in one pass.
        let mut bytes = Vec::with_capacity(size_bytes.unwrap_or(0) as usize);
        let mut file = file;
        file.read_to_end(&mut bytes)
            .map_err(|e| malformed(format!("cannot read the file: {e}")))?;
        let bytes: Arc<[u8]> = bytes.into();
        let mut workbook: Sheets<Cursor<Arc<[u8]>>> =
            calamine::open_workbook_auto_from_rs(Cursor::new(bytes))
                .map_err(|e| malformed(e.to_string()))?;
        let names = workbook.sheet_names();
        if names.is_empty() {
            return Err(IoError::EmptyInput {
                path: path_display.to_string(),
            });
        }
        let sheet = pick_sheet(&names, opts.excel.sheet.as_deref()).ok_or_else(|| {
            IoError::SheetNotFound {
                path: path_display.to_string(),
                sheet: opts.excel.sheet.clone().unwrap_or_default(),
                available: names.join(", "),
            }
        })?;
        let range: Range<Data> = workbook
            .worksheet_range(&sheet)
            .map_err(|e| malformed(format!("sheet `{sheet}`: {e}")))?;

        let (columns, rows) = table_from_range(&range, opts.excel.skip_rows, opts.excel.header());
        if columns.is_empty() {
            return Err(IoError::EmptyInput {
                path: format!("{path_display} (sheet `{sheet}`)"),
            });
        }
        let df = frame_from_text_columns(&columns, rows);
        let schema = df.schema().as_ref().clone();
        Ok(ExcelBatchSource {
            df,
            schema,
            offset: 0,
            batch_rows: opts.batch_rows.max(1),
            size_bytes,
        })
    }
}

/// Resolves the sheet selector: `None` → first sheet; a name, matched
/// case-insensitively; or a 1-based position.
fn pick_sheet(names: &[String], selector: Option<&str>) -> Option<String> {
    let Some(sel) = selector.map(str::trim).filter(|s| !s.is_empty()) else {
        return names.first().cloned();
    };
    if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(sel)) {
        return Some(n.clone());
    }
    sel.parse::<usize>()
        .ok()
        .filter(|i| *i >= 1)
        .and_then(|i| names.get(i - 1).cloned())
}

/// Turns the used range into column names and text rows.
///
/// After `skip_rows`, the first row with any content is the header (or the
/// first data row without one). Rows with no content at all are dropped
/// wherever they are — a blank spacer line is not a record.
fn table_from_range(
    range: &Range<Data>,
    skip_rows: usize,
    has_header: bool,
) -> (Vec<String>, Vec<Vec<Option<String>>>) {
    let mut rows = range
        .rows()
        .skip(skip_rows)
        .filter(|r| r.iter().any(|c| !is_blank(c)));
    let width = range.width();

    let columns: Vec<String> = if has_header {
        match rows.next() {
            Some(header) => header_names(header, width),
            None => return (Vec::new(), Vec::new()),
        }
    } else {
        (1..=width).map(|i| format!("column_{i}")).collect()
    };

    let data: Vec<Vec<Option<String>>> = rows
        .map(|r| {
            let mut out: Vec<Option<String>> = r.iter().map(render).collect();
            out.resize(columns.len(), None);
            out
        })
        .collect();
    (columns, data)
}

/// Column names from the header row: the cell text, `column_N` for a blank
/// header cell, and `_2`, `_3` suffixes for repeats so every name is unique.
fn header_names(header: &[Data], width: usize) -> Vec<String> {
    let mut names: Vec<String> = Vec::with_capacity(width);
    for i in 0..width {
        let raw = header
            .get(i)
            .and_then(render)
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("column_{}", i + 1));
        let mut name = raw.clone();
        let mut n = 2;
        while names.contains(&name) {
            name = format!("{raw}_{n}");
            n += 1;
        }
        names.push(name);
    }
    names
}

/// Whether a cell carries nothing a record could be made of.
fn is_blank(cell: &Data) -> bool {
    match cell {
        Data::Empty | Data::Error(_) => true,
        Data::String(s) => s.trim().is_empty(),
        _ => false,
    }
}

/// Renders a cell to the text a person reads off the sheet; `None` for the
/// cell kinds that carry no value.
fn render(cell: &Data) -> Option<String> {
    match cell {
        Data::Empty | Data::Error(_) => None,
        Data::String(s) => {
            if s.trim().is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Data::Int(i) => Some(i.to_string()),
        Data::Float(f) => Some(render_float(*f)),
        Data::Bool(b) => Some(if *b { "true" } else { "false" }.to_owned()),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                dt.as_duration().map(|d| {
                    let secs = d.num_seconds();
                    let sign = if secs < 0 { "-" } else { "" };
                    let secs = secs.abs();
                    format!(
                        "{sign}{}:{:02}:{:02}",
                        secs / 3600,
                        (secs % 3600) / 60,
                        secs % 60
                    )
                })
            } else {
                dt.as_datetime().map(render_datetime)
            }
        }
        Data::DateTimeIso(s) | Data::DurationIso(s) => Some(s.clone()),
    }
}

/// A date when the time part is midnight, else a datetime. Excel's serial
/// numbers leave sub-millisecond noise on times, so the value is rounded to
/// the millisecond first and the fraction is shown only when it is non-zero.
fn render_datetime(ndt: chrono::NaiveDateTime) -> String {
    use chrono::Timelike;
    let millis = (f64::from(ndt.nanosecond()) / 1e6).round() as u32;
    let whole = ndt.with_nanosecond(0).unwrap_or(ndt);
    let ndt = if millis >= 1000 {
        whole + chrono::Duration::seconds(1)
    } else {
        whole.with_nanosecond(millis * 1_000_000).unwrap_or(whole)
    };
    if ndt.time() == chrono::NaiveTime::MIN {
        ndt.format("%Y-%m-%d").to_string()
    } else if ndt.nanosecond() == 0 {
        ndt.format("%Y-%m-%dT%H:%M:%S").to_string()
    } else {
        ndt.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
    }
}

/// Whole floats print as integers; everything else as Rust's shortest
/// round-trip text, which is what a CSV export of the same sheet would carry.
fn render_float(f: f64) -> String {
    if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        f.to_string()
    }
}

impl BatchSource for ExcelBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        InputTyping::Stringly
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if self.offset >= self.df.height() {
            return Ok(None);
        }
        let len = self.batch_rows.min(self.df.height() - self.offset);
        let batch = self.df.slice(self.offset as i64, len);
        self.offset += len;
        Ok(Some(batch))
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn floats_render_like_a_csv_export() {
        assert_eq!(render_float(42.0), "42");
        assert_eq!(render_float(-3.0), "-3");
        assert_eq!(render_float(0.1), "0.1");
        assert_eq!(render_float(1234.5), "1234.5");
        assert_eq!(render_float(f64::NAN), "NaN");
    }

    #[test]
    fn sheet_selection() {
        let names = vec!["Cover".to_owned(), "Orders".to_owned()];
        assert_eq!(pick_sheet(&names, None).as_deref(), Some("Cover"));
        assert_eq!(
            pick_sheet(&names, Some("orders")).as_deref(),
            Some("Orders")
        );
        assert_eq!(pick_sheet(&names, Some("2")).as_deref(), Some("Orders"));
        assert_eq!(pick_sheet(&names, Some("0")), None);
        assert_eq!(pick_sheet(&names, Some("Totals")), None);
    }

    #[test]
    fn header_names_are_unique_and_filled() {
        let header = vec![
            Data::String("id".into()),
            Data::Empty,
            Data::String("id".into()),
            Data::Float(2024.0),
        ];
        assert_eq!(
            header_names(&header, 5),
            vec!["id", "column_2", "id_2", "2024", "column_5"]
        );
    }

    #[test]
    fn cells_render_to_text() {
        assert_eq!(render(&Data::Int(7)).as_deref(), Some("7"));
        assert_eq!(render(&Data::Bool(true)).as_deref(), Some("true"));
        assert_eq!(render(&Data::String("  ".into())), None);
        assert_eq!(render(&Data::Empty), None);
        assert_eq!(render(&Data::Error(calamine::CellErrorType::Ref)), None);
        let date = Data::DateTime(calamine::ExcelDateTime::new(
            45292.0, // 2024-01-01
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        assert_eq!(render(&date).as_deref(), Some("2024-01-01"));
        let stamp = Data::DateTime(calamine::ExcelDateTime::new(
            45292.5, // 2024-01-01 12:00
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        assert_eq!(render(&stamp).as_deref(), Some("2024-01-01T12:00:00"));
    }
}
