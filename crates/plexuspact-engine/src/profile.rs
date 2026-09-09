//! Dataset profiling for `plexuspact init` (doc 03 §6), and the per-column
//! statistics a `check` run records alongside its checks.
//!
//! One streaming pass over the source (read stringly) computes, per column:
//! null count/ratio, a distinct estimate (exact up to a cap, then reported as
//! "at least"), min/max, numeric mean/std via Welford, string length bounds,
//! and an inferred type. The profile is serializable so the Phase 1.5 AI-assist
//! layer can consume it.
//!
//! The same accumulator runs inside [`crate::execute`] on every column the
//! source presented, in a leaner mode ([`ColumnAcc::for_run`]): distinct
//! values are sketched rather than held, and the type and format detectors —
//! the expensive part, several parses per value — are off, because the
//! contract already says what each column is. What comes out is a
//! [`ColumnStats`] per column: the record a later run is compared against.

use ahash::AHashSet;
use plexuspact_io::{open, ReadOptions, Source, TypingMode};
use polars::prelude::*;
use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::hll::Hll;

/// Cap on exact distinct-value tracking; beyond this the count is a lower bound.
const DISTINCT_CAP: usize = 1_000_000;

/// How many distinct values are *kept* (not merely counted) for a column.
///
/// Counting distinctness tells you a column looks categorical; it does not tell
/// you what the categories are, which is the only form in which the fact is
/// useful to anyone drafting a rule. Small and bounded on purpose — this is for
/// `status`/`region`/`plan`, not for holding a copy of the data.
const KEEP_VALUES_CAP: usize = 25;

/// Longest value kept in [`ColumnProfile::values`]. A category name is short;
/// anything longer is prose or an identifier and is not worth retaining.
const KEEP_VALUE_MAX_CHARS: usize = 64;

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
    /// A named contract format (`email`, `uuid`, `url`) that *every* non-null
    /// value matched. Only formats with a negligible false-positive rate are
    /// reported: a guess here becomes a rule somebody's supplier is held to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// The distinct values themselves, when the column held few enough of them
    /// to be worth keeping (see `KEEP_VALUES_CAP`). Empty otherwise — an
    /// empty list means "not collected", never "no values".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<String>,
    /// Whether [`Self::values`] is the complete distinct set. False when the
    /// column had too many distinct values, or values too long, to retain.
    #[serde(default)]
    pub values_complete: bool,
}

/// What one column looked like during a `check` run.
///
/// The same accumulator as [`ColumnProfile`], run alongside the checks on
/// every column the source presented — declared or not — and reduced to what
/// a later run can be compared against. No inferred type and no format: the
/// contract already says what a column is, and the question this answers is
/// not "what is it" but "is it still what it was".
///
/// `distinct` is an estimate (HyperLogLog, about 1% error). A check run must
/// stay bounded in memory however wide the dataset is, and a drift detector
/// comparing 1,204,441 with 1,198,006 does not need the last digit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColumnStats {
    /// Column name, exactly as the source spells it.
    pub name: String,
    /// Number of null/empty values.
    pub null_count: u64,
    /// `null_count / rows`.
    pub null_ratio: f64,
    /// Estimated distinct non-null values.
    pub distinct: u64,
    /// Minimum observed value (numeric min, else lexical), rendered.
    pub min: Option<String>,
    /// Maximum observed value, rendered.
    pub max: Option<String>,
    /// Mean of the values, when every non-null value was a number.
    pub mean: Option<f64>,
    /// Sample standard deviation, when numeric and more than one value.
    pub std: Option<f64>,
    /// Minimum string length (chars) over non-null values.
    pub min_length: Option<u64>,
    /// Maximum string length (chars) over non-null values.
    pub max_length: Option<u64>,
    /// The distinct values themselves, when few and short enough to keep (see
    /// `KEEP_VALUES_CAP`). Empty means "not collected", never "no values".
    pub values: Vec<String>,
    /// Whether [`Self::values`] is the complete distinct set.
    pub values_complete: bool,
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

/// How a column's distinct values are counted.
enum Distinct {
    /// Every hash, exactly, up to the cap; past it the count is a lower bound.
    Exact { seen: AHashSet<u64>, capped: bool },
    /// A 16 KiB sketch, whatever the cardinality.
    Sketch(Hll),
}

impl Distinct {
    fn add(&mut self, hash: u64) {
        match self {
            Distinct::Exact { seen, capped } => {
                if *capped {
                    return;
                }
                if seen.len() >= DISTINCT_CAP {
                    *capped = true;
                } else {
                    seen.insert(hash);
                }
            }
            Distinct::Sketch(hll) => hll.add(hash),
        }
    }

    fn count(&self) -> u64 {
        match self {
            Distinct::Exact { seen, .. } => seen.len() as u64,
            Distinct::Sketch(hll) => hll.estimate(),
        }
    }

    fn capped(&self) -> bool {
        matches!(self, Distinct::Exact { capped: true, .. })
    }
}

/// Per-column streaming accumulator.
pub(crate) struct ColumnAcc {
    name: String,
    non_null: u64,
    null: u64,
    distinct: Distinct,
    /// Whether to run the type and format detectors. On for `init`, which
    /// has to guess what a column is; off for a `check` run, which knows.
    infer: bool,
    int_ok: u64,
    float_ok: u64,
    bool_ok: u64,
    date_ok: u64,
    datetime_ok: u64,
    email_ok: u64,
    uuid_ok: u64,
    url_ok: u64,
    // Retained distinct values, abandoned the moment the column proves it is
    // not categorical. `kept_values` is the ordered set; `keeping` goes false
    // once the cap or the length limit is breached and never comes back.
    kept_values: Vec<String>,
    keeping: bool,
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
        Self::with(
            name,
            Distinct::Exact {
                seen: AHashSet::new(),
                capped: false,
            },
            true,
        )
    }

    /// The accumulator a `check` run carries per source column: a distinct
    /// sketch instead of a set, and no guessing at types or formats.
    pub(crate) fn for_run(name: String) -> Self {
        Self::with(name, Distinct::Sketch(Hll::new()), false)
    }

    /// The column this accumulates.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn with(name: String, distinct: Distinct, infer: bool) -> Self {
        ColumnAcc {
            name,
            non_null: 0,
            null: 0,
            distinct,
            infer,
            int_ok: 0,
            float_ok: 0,
            bool_ok: 0,
            date_ok: 0,
            datetime_ok: 0,
            email_ok: 0,
            uuid_ok: 0,
            url_ok: 0,
            kept_values: Vec::new(),
            keeping: true,
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

    pub(crate) fn ingest(&mut self, ca: &StringChunked) {
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
        self.distinct.add(hash_str(s));

        // The numeric parses stay on in both modes: they are cheap, and the
        // mean of a numeric column is the single most useful thing a later
        // run can be compared against.
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
        if self.infer {
            if is_bool(t) {
                self.bool_ok += 1;
            }
            if is_date(t) {
                self.date_ok += 1;
            }
            if is_datetime(t) {
                self.datetime_ok += 1;
            }
            if is_email(t) {
                self.email_ok += 1;
            }
            if is_uuid(t) {
                self.uuid_ok += 1;
            }
            if is_url(t) {
                self.url_ok += 1;
            }
        }

        if self.keeping {
            if s.chars().count() > KEEP_VALUE_MAX_CHARS {
                self.stop_keeping();
            } else if !self.kept_values.iter().any(|v| v == s) {
                if self.kept_values.len() >= KEEP_VALUES_CAP {
                    self.stop_keeping();
                } else {
                    self.kept_values.push(s.to_owned());
                }
            }
        }

        // String stats.
        let len = s.chars().count() as u64;
        self.min_len = Some(self.min_len.map_or(len, |m| m.min(len)));
        self.max_len = Some(self.max_len.map_or(len, |m| m.max(len)));
        if self.str_min.as_deref().is_none_or(|cur| s < cur) {
            self.str_min = Some(s.to_owned());
        }
        if self.str_max.as_deref().is_none_or(|cur| s > cur) {
            self.str_max = Some(s.to_owned());
        }
    }

    /// Give up on retaining values, and release what was already held. A column
    /// that overflows the cap is not categorical, so the partial list is not a
    /// smaller truth — it is a misleading one, and keeping it would tempt a
    /// caller into proposing an `enum` of the first 25 values it happened to see.
    fn stop_keeping(&mut self) {
        self.keeping = false;
        self.kept_values = Vec::new();
        self.kept_values.shrink_to_fit();
    }

    /// The named format every non-null value matched, if any.
    ///
    /// Only formats that are effectively unambiguous are reported. A column of
    /// two-letter codes is not necessarily countries and a column of digits is
    /// not necessarily a phone number; proposing either would put a rule in
    /// front of a reviewer that is wrong more often than it is right, which is
    /// how a helpful default becomes a habit of clicking past the defaults.
    fn infer_format(&self) -> Option<&'static str> {
        if self.non_null == 0 {
            return None;
        }
        let all = self.non_null;
        if self.uuid_ok == all {
            Some("uuid")
        } else if self.email_ok == all {
            Some("email")
        } else if self.url_ok == all {
            Some("url")
        } else if self.datetime_ok == all {
            Some("iso_datetime")
        } else if self.date_ok == all {
            Some("iso_date")
        } else {
            None
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

    /// Whether every non-null value parsed as a number.
    fn numeric(&self) -> bool {
        self.non_null > 0 && (self.int_ok == self.non_null || self.float_ok == self.non_null)
    }

    fn null_ratio(&self, rows: u64) -> f64 {
        if rows == 0 {
            0.0
        } else {
            self.null as f64 / rows as f64
        }
    }

    fn std(&self) -> Option<f64> {
        (self.num_count > 1).then(|| (self.m2 / (self.num_count as f64 - 1.0)).sqrt())
    }

    /// Bounds, rendered: numeric when the column is, else lexical.
    fn bounds(&self) -> (Option<String>, Option<String>) {
        if self.numeric() {
            (self.num_min.map(fmt_num), self.num_max.map(fmt_num))
        } else {
            (self.str_min.clone(), self.str_max.clone())
        }
    }

    fn take_values(&mut self) -> (Vec<String>, bool) {
        let complete = self.keeping;
        let mut values = std::mem::take(&mut self.kept_values);
        values.sort();
        (values, complete)
    }

    fn finish(mut self, rows: u64) -> ColumnProfile {
        let inferred = self.infer_type();
        let format = self.infer_format().map(str::to_owned);
        let (values, values_complete) = self.take_values();
        let null_ratio = self.null_ratio(rows);
        let distinct = self.distinct.count();
        let distinct_at_least = self.distinct.capped();
        let numeric = matches!(inferred, "int" | "float");
        let (min, max) = if numeric {
            (self.num_min.map(fmt_num), self.num_max.map(fmt_num))
        } else {
            (self.str_min.clone(), self.str_max.clone())
        };
        let std = self.std();

        ColumnProfile {
            name: self.name,
            inferred_type: inferred.to_owned(),
            null_count: self.null,
            null_ratio,
            distinct,
            distinct_at_least,
            min,
            max,
            mean: numeric.then_some(self.mean),
            std: if numeric { std } else { None },
            min_length: self.min_len,
            max_length: self.max_len,
            format,
            values,
            values_complete,
        }
    }

    /// The run-time reduction: what a later run compares itself against.
    pub(crate) fn finish_stats(mut self, rows: u64) -> ColumnStats {
        let (values, values_complete) = self.take_values();
        let numeric = self.numeric();
        let (min, max) = self.bounds();
        let std = self.std();
        let null_ratio = self.null_ratio(rows);
        let distinct = self.distinct.count();

        ColumnStats {
            name: self.name,
            null_count: self.null,
            null_ratio,
            distinct,
            min,
            max,
            mean: numeric.then_some(self.mean),
            std: if numeric { std } else { None },
            min_length: self.min_len,
            max_length: self.max_len,
            values,
            values_complete,
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
        // What the engine's bool cast accepts, minus `1`/`0` (those are ints).
        "true" | "false" | "t" | "f" | "yes" | "no" | "y" | "n"
    )
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Same shape the `format: email` check enforces: one `@`, something either
/// side, and a dot in the domain. Deliberately not RFC 5322 — this decides
/// whether to *offer* a rule, and the rule itself does the real validation.
fn is_email(s: &str) -> bool {
    let mut parts = s.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !local.is_empty()
        && domain.len() >= 3
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !s.chars().any(char::is_whitespace)
}

/// Canonical 8-4-4-4-12 hyphenated UUID, any version, either case.
fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, c) in b.iter().enumerate() {
        let hyphen = matches!(i, 8 | 13 | 18 | 23);
        if hyphen {
            if *c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

/// An absolute http(s) URL with a host. Relative paths are not URLs for this
/// purpose — a column of `/images/1.png` is a path, and saying otherwise would
/// register a rule that fails on the very sample it was drafted from.
fn is_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    match rest {
        Some(rest) => {
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            !host.is_empty() && !s.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

/// The forms the engine's `datetime` cast accepts (a bare date is left to
/// `is_date`, so a column of dates drafts as `date`).
fn is_datetime(s: &str) -> bool {
    const NAIVE: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ];
    chrono::DateTime::parse_from_rfc3339(s).is_ok()
        || NAIVE
            .iter()
            .any(|f| chrono::NaiveDateTime::parse_from_str(s, f).is_ok())
}

fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 9e15 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc_of(values: &[&str]) -> ColumnProfile {
        let mut acc = ColumnAcc::new("c".to_owned());
        for v in values {
            acc.ingest_value(v);
        }
        acc.finish(values.len() as u64)
    }

    #[test]
    fn formats_are_reported_only_when_every_value_matches() {
        assert_eq!(
            acc_of(&["a@b.com", "c.d@e.co.uk"]).format.as_deref(),
            Some("email")
        );
        // One value that isn't an address withdraws the whole proposal.
        assert_eq!(acc_of(&["a@b.com", "not an email"]).format, None);
        assert_eq!(
            acc_of(&["0191E2A9-8C2B-7000-8000-0123456789AB"])
                .format
                .as_deref(),
            Some("uuid")
        );
        assert_eq!(
            acc_of(&["https://example.com/x?y=1"]).format.as_deref(),
            Some("url")
        );
        // A path is not a URL; proposing `format: url` here would fail the very
        // file the rule was drafted from.
        assert_eq!(acc_of(&["/images/1.png"]).format, None);
        // Two-letter codes, digits and free text get no format at all.
        assert_eq!(acc_of(&["DE", "IN"]).format, None);
    }

    #[test]
    fn small_value_sets_are_kept_and_large_ones_are_abandoned() {
        let low = acc_of(&["free", "pro", "free", "enterprise"]);
        assert!(low.values_complete);
        assert_eq!(low.values, vec!["enterprise", "free", "pro"]);

        // Past the cap the partial list is dropped, not truncated — a caller
        // must not be able to mistake "the first 25" for "all of them".
        let many: Vec<String> = (0..KEEP_VALUES_CAP + 5).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let high = acc_of(&refs);
        assert!(!high.values_complete);
        assert!(high.values.is_empty());

        // So is a column of long values, however few of them there are.
        let long = acc_of(&["x".repeat(KEEP_VALUE_MAX_CHARS + 1).as_str()]);
        assert!(!long.values_complete);
        assert!(long.values.is_empty());
    }

    #[test]
    fn the_run_accumulator_keeps_the_stats_and_skips_the_guessing() {
        let mut acc = ColumnAcc::for_run("plan".to_owned());
        for v in ["free", "pro", "free", "enterprise", "pro", "free"] {
            acc.ingest_value(v);
        }
        // Detectors are off: nothing was counted as an email, a date, a bool.
        assert_eq!(acc.email_ok + acc.date_ok + acc.bool_ok, 0);
        let stats = acc.finish_stats(8);
        assert_eq!(stats.null_count, 0);
        assert_eq!(stats.null_ratio, 0.0);
        assert_eq!(stats.distinct, 3);
        assert_eq!(stats.values, vec!["enterprise", "free", "pro"]);
        assert!(stats.values_complete);
        assert_eq!(stats.min.as_deref(), Some("enterprise"));
        assert_eq!(stats.max.as_deref(), Some("pro"));
        assert_eq!(stats.mean, None);
        assert_eq!((stats.min_length, stats.max_length), (Some(3), Some(10)));
    }

    #[test]
    fn the_run_accumulator_averages_numbers_and_sketches_distinctness() {
        let mut acc = ColumnAcc::for_run("amount".to_owned());
        let values: Vec<String> = (1..=1_000).map(|i| i.to_string()).collect();
        for v in &values {
            acc.ingest_value(v);
        }
        let stats = acc.finish_stats(1_000);
        assert_eq!(stats.mean, Some(500.5));
        assert!(stats.std.is_some_and(|s| (s - 288.819).abs() < 0.01));
        assert_eq!(stats.min.as_deref(), Some("1"));
        assert_eq!(stats.max.as_deref(), Some("1000"));
        // A sketch, not a set: near the truth, and the value list was dropped
        // once the column proved it was not categorical.
        assert!(
            (950..=1_050).contains(&stats.distinct),
            "{}",
            stats.distinct
        );
        assert!(stats.values.is_empty());
        assert!(!stats.values_complete);
    }
}
