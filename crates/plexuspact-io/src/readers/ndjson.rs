//! Streaming NDJSON reader: reads the input in line chunks and parses each
//! chunk into a `DataFrame`, so memory stays bounded by the batch size (US-6:
//! stdin NDJSON in constant memory).
//!
//! Row-number convention: the absolute row of a record is its 1-based index
//! among *parsed records*; blank/whitespace-only lines are skipped and do not
//! count.
//!
//! Schema note: each chunk is parsed independently with full-chunk inference;
//! `schema()` reports the first chunk's schema. In stringly mode every column
//! is cast to `String` after parsing, so chunks are type-consistent for the
//! engine even when JSON types drift between lines.

use std::io::BufRead;

use polars::prelude::*;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::readers::stringify_columns;

/// Streaming NDJSON batch source.
pub struct NdjsonBatchSource {
    reader: Box<dyn BufRead + Send>,
    schema: Schema,
    stringly: bool,
    batch_rows: usize,
    pending: Option<DataFrame>,
    records_read: u64,
    finished: bool,
    path_display: String,
    /// Compression codec name when the stream is being decompressed on the
    /// fly, so read errors can say "gzip" instead of "ndjson".
    codec: Option<&'static str>,
    size_bytes: Option<u64>,
}

impl std::fmt::Debug for NdjsonBatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NdjsonBatchSource")
            .field("records_read", &self.records_read)
            .finish_non_exhaustive()
    }
}

impl NdjsonBatchSource {
    /// Opens an NDJSON source from any buffered reader (file, stdin, or a
    /// decompression stream — pass `codec` for better error messages then).
    pub fn from_reader(
        reader: Box<dyn BufRead + Send>,
        batch_rows: usize,
        stringly: bool,
        path_display: &str,
        codec: Option<&'static str>,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let mut source = NdjsonBatchSource {
            reader,
            schema: Schema::default(),
            stringly,
            batch_rows: batch_rows.max(1),
            pending: None,
            records_read: 0,
            finished: false,
            path_display: path_display.to_string(),
            codec,
            size_bytes,
        };
        // Parse the first chunk eagerly so `schema()` is available before the
        // first `next_batch` call (structural checks need it).
        match source.read_and_parse_chunk()? {
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

    /// Reads up to `batch_rows` non-blank lines into a buffer.
    /// Returns `None` at end of input.
    fn read_chunk(&mut self) -> Result<Option<(String, u64, u64)>, IoError> {
        if self.finished {
            return Ok(None);
        }
        let first_record = self.records_read + 1;
        let mut buf = String::new();
        let mut lines = 0u64;
        let mut line = String::new();
        while lines < self.batch_rows as u64 {
            line.clear();
            let n = self
                .reader
                .read_line(&mut line)
                .map_err(|source| match self.codec {
                    Some(codec) => IoError::Decompress {
                        path: self.path_display.clone(),
                        codec,
                        source,
                    },
                    None => IoError::Malformed {
                        path: self.path_display.clone(),
                        format: "ndjson",
                        position: format!(" (around record {})", self.records_read + lines),
                        detail: source.to_string(),
                    },
                })?;
            if n == 0 {
                self.finished = true;
                break;
            }
            if line.trim().is_empty() {
                continue;
            }
            buf.push_str(&line);
            if !line.ends_with('\n') {
                buf.push('\n');
            }
            lines += 1;
        }
        if lines == 0 {
            return Ok(None);
        }
        self.records_read += lines;
        Ok(Some((buf, first_record, self.records_read)))
    }

    /// Reads and parses one chunk; `None` at end of input.
    fn read_and_parse_chunk(&mut self) -> Result<Option<DataFrame>, IoError> {
        let Some((buf, first, last)) = self.read_chunk()? else {
            return Ok(None);
        };
        let cursor = std::io::Cursor::new(buf);
        let df = JsonLineReader::new(cursor)
            .infer_schema_len(None) // scan the whole chunk
            .finish()
            .map_err(|e| {
                IoError::malformed(
                    &self.path_display,
                    "ndjson",
                    Some(format!("records {first}..{last}")),
                    e,
                )
            })?;
        Ok(Some(if self.stringly {
            stringify_columns(df)
        } else {
            df
        }))
    }
}

impl BatchSource for NdjsonBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        if self.stringly {
            InputTyping::Stringly
        } else {
            InputTyping::Native
        }
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if let Some(df) = self.pending.take() {
            return Ok(Some(df));
        }
        self.read_and_parse_chunk()
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}
