//! JSON-array reader.
//!
//! **JSON arrays are not streamable**: the closing bracket is at the end of
//! the file and elements have no record separator, so the whole document must
//! be parsed before the first row can be validated. This reader parses the
//! file once and then yields configurable batch slices so the engine's
//! streaming fold works uniformly. Memory is O(file size) — documented
//! limitation; prefer NDJSON for large data.
//!
//! When a `--json-path` is supplied, the document is first walked with
//! `serde_json` to the record array nested inside a wrapping object (the common
//! `{ "results": [...] }` / `{ "data": [...] }` REST-envelope shape), then that
//! array alone is handed to Polars. This costs one extra parse and is only paid
//! when a path is given.

use std::fs::File;
use std::io::{Cursor, Read};

use polars::io::mmap::MmapBytesReader;
use polars::prelude::*;
use serde_json::Value;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;
use crate::readers::stringify_columns;

/// Whole-file JSON array source with batched iteration.
#[derive(Debug)]
pub struct JsonBatchSource {
    df: DataFrame,
    schema: Schema,
    stringly: bool,
    offset: usize,
    batch_rows: usize,
    size_bytes: Option<u64>,
}

impl JsonBatchSource {
    /// Parses a JSON array file, optionally descending to `json_path` first.
    pub fn from_file(
        mut file: File,
        batch_rows: usize,
        stringly: bool,
        path_display: &str,
        size_bytes: Option<u64>,
        json_path: Option<&str>,
    ) -> Result<Self, IoError> {
        let df = match json_path {
            None => read_df(file, path_display)?,
            Some(pointer) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|source| IoError::MissingFile {
                        path: path_display.to_string(),
                        source,
                    })?;
                let root: Value = serde_json::from_slice(&bytes)
                    .map_err(|e| IoError::malformed_str(path_display, "json", &e.to_string()))?;
                let array_bytes =
                    extract_array(&root, pointer).map_err(|detail| IoError::JsonPath {
                        path: path_display.to_string(),
                        pointer: pointer.to_string(),
                        detail,
                    })?;
                read_df(Cursor::new(array_bytes), path_display)?
            }
        };
        let df = if stringly { stringify_columns(df) } else { df };
        if df.width() == 0 {
            return Err(IoError::EmptyInput {
                path: path_display.to_string(),
            });
        }
        let schema = df.schema().as_ref().clone();
        Ok(JsonBatchSource {
            df,
            schema,
            stringly,
            offset: 0,
            batch_rows: batch_rows.max(1),
            size_bytes,
        })
    }
}

/// Parses a JSON array from any byte source into a DataFrame.
fn read_df<R: MmapBytesReader>(reader: R, path_display: &str) -> Result<DataFrame, IoError> {
    JsonReader::new(reader)
        .with_json_format(JsonFormat::Json)
        .infer_schema_len(None)
        .finish()
        .map_err(|e| IoError::malformed(path_display, "json", None, e))
}

/// Walks `root` to the dotted `pointer` and serializes the record array found
/// there. A single object is wrapped into a one-element array so single-record
/// responses validate too. Returns a human-readable reason on failure.
fn extract_array(root: &Value, pointer: &str) -> Result<Vec<u8>, String> {
    let cleaned = pointer
        .trim()
        .trim_start_matches('$')
        .trim_start_matches('.');
    let mut cur = root;
    for seg in cleaned.split('.').filter(|s| !s.is_empty()) {
        match cur {
            Value::Object(map) => {
                cur = map.get(seg).ok_or_else(|| {
                    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
                    keys.sort_unstable();
                    let available = if keys.is_empty() {
                        "<none>".to_owned()
                    } else {
                        keys.join(", ")
                    };
                    format!("no key `{seg}` at that level (available keys: {available})")
                })?;
            }
            other => {
                return Err(format!(
                    "cannot descend into `{seg}`: the value above it is {}, not an object",
                    kind_of(other)
                ));
            }
        }
    }
    match cur {
        Value::Array(_) => serde_json::to_vec(cur).map_err(|e| e.to_string()),
        Value::Object(_) => serde_json::to_vec(&[cur]).map_err(|e| e.to_string()),
        other => Err(format!(
            "the value there is {}, not an array or object; --json-path must point at \
             the record array (or a single record object)",
            kind_of(other)
        )),
    }
}

/// Article-qualified type name of a JSON value, for error messages.
fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

impl BatchSource for JsonBatchSource {
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
