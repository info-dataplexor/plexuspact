//! Dataset profiling for `plexuspact init` (doc 03 §6).
//!
//! One streaming pass over the source (read stringly) computes, per column:
//! null count/ratio, a distinct estimate (exact up to a cap, then reported as
//! "at least"), min/max, numeric mean/std via Welford, string length bounds,
//! and an inferred type. The profile is serializable so the Phase 1.5 AI-assist
//! layer can consume it.

use ahash::AHashSet;
use plexuspact_io::{open, ReadOptions, Source, TypingMode};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::EngineError;

/// Cap on exact distinct-value tracking; beyond this the count is a lower bound.
const DISTINCT_CAP: usize = 1_000_000;

/// Profile of one column.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnProfile {
    /// Column name (source order).
    pub name: String,
    /// Inferred contract type (`string`, `int`, `float`, `bool`, `date`, `datetime`).
    pub inferred_type: String,
    /// Number of null/empty values.
    pub null_count: u64,
    /// `null_count / rows`.
    pub null_ratio: f64,
    /// Distinct non-null values; `at_least` is `true` when the cap was hit.
    pub distinct: u64,
    /// Whether `distinct` is a lower bound (cap reached).
    pub distinct_at_least: bool,
    /// Minimum observed value (numeric min, else lexical), rendered.
    pub min: Option<String>,
    /// Maximum observed value, rendered.
    pub max: Option<String>,
    /// Mean of numeric values, when the column is numeric.
    pub mean: Option<f64>,
    /// Sample standard deviation of numeric values, when numeric.
    pub std: Option<f64>,
    /// Minimum string length (chars) over non-null values.
    pub min_length: Option<u64>,
    /// Maximum string length (chars) over non-null values.
    pub max_length: Option<u64>,
}

/// Profile of a whole dataset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DatasetProfile {
    /// Total rows scanned.
    pub rows: u64,
    /// Per-column profiles, in source order.
    pub columns: Vec<ColumnProfile>,
}

/// Profiles a source in a single streaming pass.
pub fn profile(source: &Source, read_options: &ReadOptions) -> Result<DatasetProfile, EngineError> {
    let mut opts = read_options.clone();
    opts.typing = TypingMode::Stringly; // uniform: infer types ourselves
    let mut bs = open(source, &opts)?;

    let names: Vec<String> = bs.schema().iter_names().map(|n| n.to_string()).collect();
    let mut accs: Vec<ColumnAcc> = names.iter().map(|n| ColumnAcc::new(n.clone())).collect();
    let index: std::collections::HashMap<&str, usize> = names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    let mut rows: u64 = 0;
    while let Some(df) = bs.next_batch()? {
        rows += df.height() as u64;
        for col in df.get_columns() {
            let Some(&i) = index.get(col.name().as_str()) else {
                continue;
            };
            let native = col.as_materialized_series();
            // Nested columns (JSON arrays/objects) can't be profiled as scalars —
            // fail with a clear, actionable message instead of an opaque cast error.
            if let Some(kind) = crate::column::nested_kind(native.dtype()) {
                return Err(EngineError::NestedColumn {
                    column: col.name().to_string(),
                    kind,
                });
            }
            let series = native.cast(&DataType::String)?;
            let ca = series.str()?;
            accs[i].ingest(ca);
        }
    }

    let columns = accs.into_iter().map(|a| a.finish(rows)).collect();
    Ok(DatasetProfile { rows, columns })
}

/// Per-column streaming accumulator.
struct ColumnAcc {
    name: String,
    non_null: u64,
    null: u64,
    distinct: AHashSet<u64>,
    distinct_capped: bool,
    int_ok: u64,
    float_ok: u64,
    bool_ok: u64,
    date_ok: u64,
    datetime_ok: u64,
    // Numeric (Welford over float-parseable values).
    num_count: u64,
    mean: f64,
    m2: f64,
    num_min: Option<f64>,
    num_max: Option<f64>,
    // String.
    str_min: Option<String>,
    str_max: Option<String>,
    min_len: Option<u64>,
    max_len: Option<u64>,
}

impl ColumnAcc {
    fn new(name: String) -> Self {
        ColumnAcc {
            name,
            non_null: 0,
            null: 0,
            distinct: AHashSet::new(),
            distinct_capped: false,
            int_ok: 0,
            float_ok: 0,
            bool_ok: 0,
            date_ok: 0,
            datetime_ok: 0,
            num_count: 0,
            mean: 0.0,
            m2: 0.0,
            num_min: None,
            num_max: None,
            str_min: None,
            str_max: None,
            min_len: None,
            max_len: None,
        }
    }

    fn ingest(&mut self, ca: &StringChunked) {
        for opt in ca.into_iter() {
            match opt {
                None => self.null += 1,
                Some("") => self.null += 1, // empty CSV field is a missing value
                Some(s) => self.ingest_value(s),
            }
        }
    }

    fn ingest_value(&mut self, s: &str) {
        self.non_null += 1;

        if !self.distinct_capped {
            if self.distinct.len() >= DISTINCT_CAP {
                self.distinct_capped = true;
            } else {
                self.distinct.insert(hash_str(s));
            }
        }

        let t = s.trim();
        if t.parse::<i64>().is_ok() {
            self.int_ok += 1;
        }
        if let Ok(f) = t.parse::<f64>() {
            if f.is_finite() {
                self.float_ok += 1;
                self.push_numeric(f);
            }
        }
        if is_bool(t) {
            self.bool_ok += 1;
        }
        if is_date(t) {
            self.date_ok += 1;
        }
        if is_datetime(t) {
            self.datetime_ok += 1;
        }

        // String stats.
        let len = s.chars().count() as u64;
        self.min_len = Some(self.min_len.map_or(len, |m| m.min(len)));
        self.max_len = Some(self.max_len.map_or(len, |m| m.max(len)));
        if self.str_min.as_deref().map_or(true, |cur| s < cur) {
            self.str_min = Some(s.to_owned());
        }
        if self.str_max.as_deref().map_or(true, |cur| s > cur) {
            self.str_max = Some(s.to_owned());
        }
    }

    fn push_numeric(&mut self, x: f64) {
        self.num_count += 1;
        let delta = x - self.mean;
        self.mean += delta / self.num_count as f64;
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
        self.num_min = Some(self.num_min.map_or(x, |m| m.min(x)));
        self.num_max = Some(self.num_max.map_or(x, |m| m.max(x)));
    }

    fn infer_type(&self) -> &'static str {
        if self.non_null == 0 {
            return "string";
        }
        let all = self.non_null;
        if self.bool_ok == all {
            "bool"
        } else if self.int_ok == all {
            "int"
        } else if self.float_ok == all {
            "float"
        } else if self.datetime_ok == all {
            "datetime"
        } else if self.date_ok == all {
            "date"
        } else {
            "string"
        }
    }

    fn finish(self, rows: u64) -> ColumnProfile {
        let inferred = self.infer_type();
        let numeric = matches!(inferred, "int" | "float");
        let null_ratio = if rows == 0 {
            0.0
        } else {
            self.null as f64 / rows as f64
        };
        let std = if self.num_count > 1 {
            Some((self.m2 / (self.num_count as f64 - 1.0)).sqrt())
        } else {
            None
        };

        let (min, max) = if numeric {
            (self.num_min.map(fmt_num), self.num_max.map(fmt_num))
        } else {
            (self.str_min.clone(), self.str_max.clone())
        };

        ColumnProfile {
            name: self.name,
            inferred_type: inferred.to_owned(),
            null_count: self.null,
            null_ratio,
            distinct: self.distinct.len() as u64,
            distinct_at_least: self.distinct_capped,
            min,
            max,
            mean: numeric.then_some(self.mean),
            std: if numeric { std } else { None },
            min_length: self.min_len,
            max_length: self.max_len,
        }
    }
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = ahash::AHasher::default();
    s.hash(&mut h);
    h.finish()
}

fn is_bool(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "false" | "t" | "f" | "yes" | "no"
    )
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

fn is_datetime(s: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
        || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").is_ok()
        || chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").is_ok()
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 9e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}
