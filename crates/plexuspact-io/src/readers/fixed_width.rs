//! Streaming fixed-width text reader.
//!
//! Mainframe extracts, bank statements, government files: every line is a
//! record and every column is a character span, with no delimiter at all.
//! The layout is the contract's business (`settings.input.fixed_width`) and
//! the reader only slices: for each field it takes the characters in the
//! span, trims them, and yields the text — an empty span, or a line too short
//! to reach it, is a null. Blank lines are skipped. Memory is bounded by the
//! batch size.
//!
//! Positions are *characters*, not bytes, so a `é` in a name field does not
//! shift every column after it.

use std::io::BufRead;

use polars::prelude::*;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::options::FixedWidthOptions;
use crate::readers::frame_from_text_columns;

/// Streaming fixed-width batch source.
pub struct FixedWidthBatchSource {
    reader: Box<dyn BufRead + Send>,
    layout: FixedWidthOptions,
    names: Vec<String>,
    schema: Schema,
    batch_rows: usize,
    pending: Option<DataFrame>,
    /// Data rows yielded so far (for error positions).
    rows_read: u64,
    /// Physical lines consumed so far.
    line_no: u64,
    finished: bool,
    path_display: String,
    codec: Option<&'static str>,
    size_bytes: Option<u64>,
}

impl std::fmt::Debug for FixedWidthBatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixedWidthBatchSource")
            .field("rows_read", &self.rows_read)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

impl FixedWidthBatchSource {
    /// Opens a fixed-width source from any buffered reader (file, stdin, or a
    /// decompression stream — pass `codec` for better error messages then).
    pub fn from_reader(
        reader: Box<dyn BufRead + Send>,
        layout: &FixedWidthOptions,
        batch_rows: usize,
        path_display: &str,
        codec: Option<&'static str>,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let names: Vec<String> = layout.fields.iter().map(|f| f.name.clone()).collect();
        let mut source = FixedWidthBatchSource {
            reader,
            layout: layout.clone(),
            names,
            schema: Schema::default(),
            batch_rows: batch_rows.max(1),
            pending: None,
            rows_read: 0,
            line_no: 0,
            finished: false,
            path_display: path_display.to_string(),
            codec,
            size_bytes,
        };
        for _ in 0..source.layout.skip_rows {
            if source.read_line()?.is_none() {
                break;
            }
        }
        if source.layout.has_header {
            // The header is the first line with anything on it.
            while let Some(line) = source.read_line()? {
                if !line.trim().is_empty() {
                    break;
                }
            }
        }
        match source.read_batch()? {
            Some(df) => {
                source.schema = df.schema().as_ref().clone();
                source.pending = Some(df);
                Ok(source)
            }
            None => Err(IoError::EmptyInput {
                path: path_display.to_string(),
            }),
        }
    }

    /// The next physical line without its terminator; `None` at end of input.
    fn read_line(&mut self) -> Result<Option<String>, IoError> {
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| self.io_error(e))?;
        if n == 0 {
            return Ok(None);
        }
        self.line_no += 1;
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(Some(line))
    }

    fn io_error(&self, e: std::io::Error) -> IoError {
        match self.codec {
            Some(codec) => IoError::Decompress {
                path: self.path_display.clone(),
                codec,
                source: e,
            },
            None => IoError::Malformed {
                path: self.path_display.clone(),
                format: "fixed-width",
                position: format!(" (line {})", self.line_no + 1),
                detail: if e.kind() == std::io::ErrorKind::InvalidData {
                    "the line is not valid UTF-8".to_owned()
                } else {
                    e.to_string()
                },
            },
        }
    }

    /// Reads data lines until a batch is full or the input ends.
    fn read_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if self.finished {
            return Ok(None);
        }
        let mut rows: Vec<Vec<Option<String>>> = Vec::new();
        while rows.len() < self.batch_rows {
            let Some(line) = self.read_line()? else {
                self.finished = true;
                break;
            };
            if line.trim().is_empty() {
                continue;
            }
            rows.push(slice_line(&line, &self.layout));
            self.rows_read += 1;
        }
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(frame_from_text_columns(&self.names, rows)))
    }
}

/// Cuts one line into the layout's spans (character positions), trimmed;
/// an empty or unreachable span is a null.
fn slice_line(line: &str, layout: &FixedWidthOptions) -> Vec<Option<String>> {
    let chars: Vec<char> = line.chars().collect();
    layout
        .fields
        .iter()
        .map(|span| {
            if span.start >= chars.len() {
                return None;
            }
            let end = span.end.min(chars.len());
            let text: String = chars[span.start..end].iter().collect();
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_owned())
            }
        })
        .collect()
}

impl BatchSource for FixedWidthBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        InputTyping::Stringly
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if let Some(df) = self.pending.take() {
            return Ok(Some(df));
        }
        self.read_batch()
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use std::io::Cursor;

    fn layout(spec: &str) -> FixedWidthOptions {
        FixedWidthOptions::parse(spec).unwrap()
    }

    fn open(
        text: &str,
        layout: &FixedWidthOptions,
        batch: usize,
    ) -> Result<FixedWidthBatchSource, IoError> {
        FixedWidthBatchSource::from_reader(
            Box::new(Cursor::new(text.as_bytes().to_vec())),
            layout,
            batch,
            "test.fwf",
            None,
            None,
        )
    }

    fn col(df: &DataFrame, name: &str) -> Vec<Option<String>> {
        df.column(name)
            .unwrap()
            .as_materialized_series()
            .str()
            .unwrap()
            .into_iter()
            .map(|v| v.map(str::to_owned))
            .collect()
    }

    const LINES: &str = "0001Ada Lovelace     12.50\r\n\n0002Zoë             \n0003";

    #[test]
    fn slices_trims_and_nulls_short_lines() {
        let l = layout("id=1-4,name=5-20,amount=6");
        let mut src = open(LINES, &l, 100).unwrap();
        let df = src.next_batch().unwrap().unwrap();
        assert_eq!(df.height(), 3, "the blank line is not a record");
        assert_eq!(
            col(&df, "id"),
            vec![
                Some("0001".into()),
                Some("0002".into()),
                Some("0003".into())
            ]
        );
        assert_eq!(
            col(&df, "name"),
            vec![Some("Ada Lovelace".into()), Some("Zoë".into()), None]
        );
        assert_eq!(col(&df, "amount"), vec![Some("12.50".into()), None, None]);
        assert!(src.next_batch().unwrap().is_none());
        assert_eq!(src.typing(), InputTyping::Stringly);
        assert_eq!(src.schema().len(), 3);
    }

    #[test]
    fn positions_are_characters_not_bytes() {
        let l = layout("name=1-4,code=5-6");
        let mut src = open("Zoë X1\n", &l, 10).unwrap();
        let df = src.next_batch().unwrap().unwrap();
        assert_eq!(col(&df, "name"), vec![Some("Zoë".into())]);
        assert_eq!(col(&df, "code"), vec![Some("X1".into())]);
    }

    #[test]
    fn header_and_skip_rows() {
        let mut l = layout("id=1-4,name=5-20");
        l.skip_rows = 1;
        l.has_header = true;
        let text = "REPORT 2024\n\nID  NAME\n0001Ada\n0002Bob\n";
        let mut src = open(text, &l, 1).unwrap();
        let first = src.next_batch().unwrap().unwrap();
        assert_eq!(col(&first, "id"), vec![Some("0001".into())]);
        let second = src.next_batch().unwrap().unwrap();
        assert_eq!(col(&second, "name"), vec![Some("Bob".into())]);
        assert!(src.next_batch().unwrap().is_none());
        assert_eq!(src.rows_read, 2);
    }

    #[test]
    fn empty_and_invalid_utf8() {
        let l = layout("id=1-4");
        assert!(matches!(
            open("\n\n", &l, 10).unwrap_err(),
            IoError::EmptyInput { .. }
        ));
        let bad = FixedWidthBatchSource::from_reader(
            Box::new(Cursor::new(vec![0xff, 0xfe, b'\n'])),
            &l,
            10,
            "bad.fwf",
            None,
            None,
        )
        .unwrap_err();
        let msg = bad.to_string();
        assert!(
            msg.contains("fixed-width") && msg.contains("UTF-8"),
            "{msg}"
        );
    }
}
