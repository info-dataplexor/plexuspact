//! Streaming Parquet reader: reads the footer once, then yields row slices
//! (aligned to row groups by Polars internally) in constant memory.
//!
//! Parquet is natively typed ([`InputTyping::Native`]): declared-vs-actual
//! dtype mismatches are structural, file-level check failures — a bad value
//! cannot exist inside a typed column, so there is no per-row capture for
//! Parquet (the documented asymmetry with text formats; ADR-008).

use std::fs::File;
use std::sync::Arc;

use polars::prelude::*;

use crate::batch::{BatchSource, InputTyping};
use crate::error::IoError;

/// Streaming Parquet batch source.
pub struct ParquetBatchSource {
    file: File,
    metadata: Arc<polars::io::parquet::read::FileMetadata>,
    schema: Schema,
    total_rows: usize,
    offset: usize,
    batch_rows: usize,
    path_display: String,
    size_bytes: Option<u64>,
}

impl std::fmt::Debug for ParquetBatchSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParquetBatchSource")
            .field("total_rows", &self.total_rows)
            .field("offset", &self.offset)
            .finish_non_exhaustive()
    }
}

impl ParquetBatchSource {
    /// Opens a Parquet source from a file handle.
    pub fn from_file(
        file: File,
        batch_rows: usize,
        path_display: &str,
        size_bytes: Option<u64>,
    ) -> Result<Self, IoError> {
        let handle = file.try_clone().map_err(|source| IoError::MissingFile {
            path: path_display.to_string(),
            source,
        })?;
        let mut reader = ParquetReader::new(handle);
        let metadata = reader
            .get_metadata()
            .map_err(|e| IoError::malformed(path_display, "parquet", None, e))?
            .clone();
        let arrow_schema = reader
            .schema()
            .map_err(|e| IoError::malformed(path_display, "parquet", None, e))?;
        let schema = Schema::from_arrow_schema(arrow_schema.as_ref());
        let total_rows = metadata.num_rows;
        Ok(ParquetBatchSource {
            file,
            metadata,
            schema,
            total_rows,
            offset: 0,
            batch_rows: batch_rows.max(1),
            path_display: path_display.to_string(),
            size_bytes,
        })
    }
}

impl BatchSource for ParquetBatchSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        InputTyping::Native
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        if self.offset >= self.total_rows {
            return Ok(None);
        }
        let len = self.batch_rows.min(self.total_rows - self.offset);
        let handle = self
            .file
            .try_clone()
            .map_err(|source| IoError::MissingFile {
                path: self.path_display.clone(),
                source,
            })?;
        let mut reader = ParquetReader::new(handle);
        reader.set_metadata(self.metadata.clone());
        let df = reader
            .with_slice(Some((self.offset, len)))
            .finish()
            .map_err(|e| {
                IoError::malformed(
                    &self.path_display,
                    "parquet",
                    Some(format!("rows {}..{}", self.offset, self.offset + len)),
                    e,
                )
            })?;
        self.offset += len;
        Ok(Some(df))
    }

    fn size_bytes(&self) -> Option<u64> {
        self.size_bytes
    }
}
