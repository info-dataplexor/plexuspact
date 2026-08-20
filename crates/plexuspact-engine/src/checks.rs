//! Check executors. Each implements [`Check`]: fold per-batch state, then
//! [`Check::finalize`] into a [`CheckOutcome`].

use std::collections::HashMap;

use ahash::AHashSet;
use plexuspact_contract::{EnumValue, KnownFormat, LengthSpec, Number, Severity};
use polars::prelude::*;
use serde_json::json;

use crate::column::{ColumnData, Parsed};
use crate::error::EngineError;
use crate::formats::validator;
use crate::CheckOutcome;

/// Per-batch inputs shared by all checks.
pub(crate) struct BatchView<'a> {
    /// Extracted declared columns present in this batch, keyed by name.
    pub columns: &'a HashMap<String, ColumnData>,
    /// The raw batch frame (for `custom_expr`).
    pub df: &'a DataFrame,
    /// Absolute 1-based row number of the first row in this batch.
    pub base_row: u64,
    /// Failure-sample budget per check.
    pub sample_budget: usize,
}

/// A check that folds batch state and finalizes into an outcome.
pub(crate) trait Check {
    /// Processes one batch.
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError>;
    /// Produces the final outcome given the total row count of the run.
    fn finalize(self: Box<Self>, total_rows: u64) -> CheckOutcome;
}

/// Common check identity.
#[derive(Clone)]
pub(crate) struct Meta {
    pub id: String,
    pub column: Option<String>,
    pub kind: &'static str,
    pub params: serde_json::Value,
    pub severity: Severity,
}

/// Row-level accumulator shared by value checks.
#[derive(Default)]
struct RowAccum {
    rows_evaluated: u64,
    rows_failed: u64,
    samples: Vec<(u64, String)>,
}

impl RowAccum {
    fn record_failure(&mut self, abs_row: u64, value: String, budget: usize) {
        self.rows_failed += 1;
        if self.samples.len() < budget {
            self.samples.push((abs_row, value));
        }
    }
}

fn fail_ratio(failed: u64, evaluated: u64) -> Option<f64> {
    if evaluated == 0 {
        None
    } else {
        Some(failed as f64 / evaluated as f64)
    }
}

// ─────────────────────────────── required ───────────────────────────────

/// `required: true` — the column must have no nulls.
pub(crate) struct RequiredCheck {
    pub meta: Meta,
    acc: RowAccum,
}

impl RequiredCheck {
    pub fn new(meta: Meta) -> Self {
        RequiredCheck {
            meta,
            acc: RowAccum::default(),
        }
    }
}

impl Check for RequiredCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        else {
            return Ok(());
        };
        self.acc.rows_evaluated += col.len() as u64;
        for (i, cell) in col.raw.iter().enumerate() {
            if cell.is_none() {
                let abs = view.base_row + i as u64;
                self.acc
                    .record_failure(abs, "(null)".to_owned(), view.sample_budget);
            }
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message = failed.then(|| {
            format!(
                "{} null value(s) in a required column",
                self.acc.rows_failed
            )
        });
        outcome_row(self.meta, self.acc, message)
    }
}

// ────────────────────────────── type mismatch ───────────────────────────

/// Declared-type conformance for a column (`{col}.type`).
pub(crate) struct TypeCheck {
    pub meta: Meta,
    acc: RowAccum,
    structural: bool,
}

impl TypeCheck {
    pub fn new(meta: Meta) -> Self {
        TypeCheck {
            meta,
            acc: RowAccum::default(),
            structural: false,
        }
    }
}

impl Check for TypeCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        else {
            return Ok(());
        };
        if col.structural_mismatch {
            self.structural = true;
            return Ok(());
        }
        self.acc.rows_evaluated += col.non_null();
        for (i, failed) in col.cast_failed.iter().enumerate() {
            if *failed {
                let abs = view.base_row + i as u64;
                let value = col.raw[i].clone().unwrap_or_default();
                self.acc.record_failure(abs, value, view.sample_budget);
            }
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        if self.structural {
            let mut out = outcome_row(
                self.meta,
                self.acc,
                Some("column has the wrong type (structural mismatch)".to_owned()),
            );
            out.failed = true;
            out.rows_failed = None;
            out.rows_evaluated = None;
            return out;
        }
        let failed = self.acc.rows_failed > 0;
        let message = failed.then(|| {
            format!(
                "{} value(s) do not match the declared type",
                self.acc.rows_failed
            )
        });
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── min / max ──────────────────────────────

/// Numeric/temporal lower or upper bound.
pub(crate) struct BoundCheck {
    pub meta: Meta,
    pub bound: f64,
    pub is_min: bool,
    acc: RowAccum,
    observed_extreme: Option<f64>,
}

impl BoundCheck {
    pub fn new(meta: Meta, bound: Number, is_min: bool) -> Self {
        BoundCheck {
            meta,
            bound: bound.as_f64(),
            is_min,
            acc: RowAccum::default(),
            observed_extreme: None,
        }
    }

    fn as_f64_values(col: &ColumnData) -> Option<Vec<Option<f64>>> {
        match &col.parsed {
            Parsed::Int(v) => Some(v.iter().map(|o| o.map(|x| x as f64)).collect()),
            Parsed::Float(v) => Some(v.clone()),
            Parsed::Date(v) => Some(v.iter().map(|o| o.map(|x| x as f64)).collect()),
            Parsed::Datetime(v) => Some(v.iter().map(|o| o.map(|x| x as f64)).collect()),
            Parsed::Bool(_) | Parsed::Str => None,
        }
    }
}

impl Check for BoundCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        else {
            return Ok(());
        };
        let Some(values) = Self::as_f64_values(col) else {
            return Ok(());
        };
        for (i, v) in values.iter().enumerate() {
            let Some(x) = v else { continue };
            self.acc.rows_evaluated += 1;
            self.observed_extreme = Some(match self.observed_extreme {
                None => *x,
                Some(cur) if self.is_min => cur.min(*x),
                Some(cur) => cur.max(*x),
            });
            let violates = if self.is_min {
                *x < self.bound
            } else {
                *x > self.bound
            };
            if violates {
                let abs = view.base_row + i as u64;
                let value = col.raw[i].clone().unwrap_or_default();
                self.acc.record_failure(abs, value, view.sample_budget);
            }
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let key = if self.is_min {
            "min_observed"
        } else {
            "max_observed"
        };
        let mut observed = std::collections::BTreeMap::new();
        if let Some(x) = self.observed_extreme {
            observed.insert(key.to_owned(), json_number(x));
        }
        let message = failed.then(|| match self.observed_extreme {
            Some(x) if self.is_min => format!("min observed: {}", trim_float(x)),
            Some(x) => format!("max observed: {}", trim_float(x)),
            None => "bound violated".to_owned(),
        });
        let mut out = outcome_row(self.meta, self.acc, message);
        out.observed.extend(observed);
        out
    }
}

// ─────────────────────────────── regex ──────────────────────────────────

/// Regular-expression match over string values.
pub(crate) struct RegexCheck {
    pub meta: Meta,
    re: regex::Regex,
    acc: RowAccum,
}

impl RegexCheck {
    pub fn new(meta: Meta, pattern: &str) -> Result<Self, regex::Error> {
        Ok(RegexCheck {
            meta,
            re: regex::Regex::new(pattern)?,
            acc: RowAccum::default(),
        })
    }
}

impl Check for RegexCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        run_string_predicate(&self.meta, &mut self.acc, view, |s| self.re.is_match(s));
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message = failed.then(|| {
            format!(
                "{} value(s) did not match the pattern",
                self.acc.rows_failed
            )
        });
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── format ─────────────────────────────────

/// Built-in format validation.
pub(crate) struct FormatCheck {
    pub meta: Meta,
    validate: fn(&str) -> bool,
    acc: RowAccum,
}

impl FormatCheck {
    pub fn new(meta: Meta, format: KnownFormat) -> Self {
        FormatCheck {
            meta,
            validate: validator(format),
            acc: RowAccum::default(),
        }
    }
}

impl Check for FormatCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let f = self.validate;
        run_string_predicate(&self.meta, &mut self.acc, view, f);
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message = failed.then(|| format!("{} value(s) are not valid", self.acc.rows_failed));
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── enum ───────────────────────────────────

/// Membership in an allowed set.
pub(crate) struct EnumCheck {
    pub meta: Meta,
    allowed: AHashSet<String>,
    acc: RowAccum,
}

impl EnumCheck {
    pub fn new(meta: Meta, values: &[EnumValue]) -> Self {
        let allowed = values.iter().map(|v| v.to_string()).collect();
        EnumCheck {
            meta,
            allowed,
            acc: RowAccum::default(),
        }
    }
}

impl Check for EnumCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let allowed = &self.allowed;
        run_string_predicate(&self.meta, &mut self.acc, view, |s| allowed.contains(s));
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message =
            failed.then(|| format!("{} value(s) outside the allowed set", self.acc.rows_failed));
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── length ─────────────────────────────────

/// String length constraint (chars).
pub(crate) struct LengthCheck {
    pub meta: Meta,
    min: u64,
    max: Option<u64>,
    acc: RowAccum,
}

impl LengthCheck {
    pub fn new(meta: Meta, spec: LengthSpec) -> Self {
        let (min, max) = spec.bounds();
        LengthCheck {
            meta,
            min,
            max,
            acc: RowAccum::default(),
        }
    }
}

impl Check for LengthCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let (min, max) = (self.min, self.max);
        run_string_predicate(&self.meta, &mut self.acc, view, |s| {
            let n = s.chars().count() as u64;
            n >= min && max.map(|m| n <= m).unwrap_or(true)
        });
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message =
            failed.then(|| format!("{} value(s) have the wrong length", self.acc.rows_failed));
        outcome_row(self.meta, self.acc, message)
    }
}

// ────────────────────────── not_empty_string ────────────────────────────

/// Non-empty string constraint.
pub(crate) struct NotEmptyCheck {
    pub meta: Meta,
    acc: RowAccum,
}

impl NotEmptyCheck {
    pub fn new(meta: Meta) -> Self {
        NotEmptyCheck {
            meta,
            acc: RowAccum::default(),
        }
    }
}

impl Check for NotEmptyCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        run_string_predicate(&self.meta, &mut self.acc, view, |s| !s.is_empty());
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let failed = self.acc.rows_failed > 0;
        let message = failed.then(|| format!("{} empty string value(s)", self.acc.rows_failed));
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── unique ─────────────────────────────────

/// Distinct-values constraint (stateful across batches).
///
/// Exact mode (default): 8 bytes per distinct value (a 64-bit `ahash` of the
/// string; collision probability is negligible for realistic cardinalities),
/// with row-level duplicate samples. Approx mode (`{ approx: true }`, ADR-005):
/// a 16 KiB HyperLogLog sketch — constant memory, no row samples, and a
/// documented ~2% tolerance so sketch error (±0.81% typical) can never fail a
/// genuinely unique column.
pub(crate) struct UniqueCheck {
    pub meta: Meta,
    counter: DistinctCounter,
    acc: RowAccum,
    first_dupe: Option<String>,
}

enum DistinctCounter {
    Exact(AHashSet<u64>),
    Approx(crate::hll::Hll),
}

/// Fraction of evaluated rows the HLL estimate must fall short by before the
/// approx check fails — ≈2.5σ of the sketch's standard error, so clean data
/// does not produce false duplicates.
const APPROX_TOLERANCE: f64 = 0.02;

impl UniqueCheck {
    pub fn new(meta: Meta, approx: bool) -> Self {
        UniqueCheck {
            meta,
            counter: if approx {
                DistinctCounter::Approx(crate::hll::Hll::new())
            } else {
                DistinctCounter::Exact(AHashSet::new())
            },
            acc: RowAccum::default(),
            first_dupe: None,
        }
    }
}

impl Check for UniqueCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        else {
            return Ok(());
        };
        for (i, cell) in col.raw.iter().enumerate() {
            let Some(value) = cell else { continue };
            self.acc.rows_evaluated += 1;
            let h = hash_str(value);
            match &mut self.counter {
                DistinctCounter::Exact(seen) => {
                    if !seen.insert(h) {
                        // Second or later occurrence is the duplicate (ADR-009).
                        if self.first_dupe.is_none() {
                            self.first_dupe = Some(value.clone());
                        }
                        let abs = view.base_row + i as u64;
                        self.acc
                            .record_failure(abs, value.clone(), view.sample_budget);
                    }
                }
                DistinctCounter::Approx(hll) => hll.add(h),
            }
        }
        Ok(())
    }

    fn finalize(mut self: Box<Self>, _total: u64) -> CheckOutcome {
        let mut observed = std::collections::BTreeMap::new();
        let message;
        match &self.counter {
            DistinctCounter::Exact(seen) => {
                observed.insert("distinct_count".to_owned(), json!(seen.len()));
                // ADR-005: surface a memory hint once exact tracking crosses
                // ~80 MB (10M × 8-byte hashes) instead of silently degrading.
                if seen.len() >= 10_000_000 {
                    observed.insert(
                        "hint".to_owned(),
                        json!("high cardinality — consider `unique: { approx: true }` (ADR-005)"),
                    );
                }
                let failed = self.acc.rows_failed > 0;
                message = failed.then(|| match &self.first_dupe {
                    Some(v) => {
                        format!("{} duplicate value(s), e.g. \"{v}\"", self.acc.rows_failed)
                    }
                    None => format!("{} duplicate value(s)", self.acc.rows_failed),
                });
            }
            DistinctCounter::Approx(hll) => {
                let est = hll.estimate().min(self.acc.rows_evaluated);
                let threshold =
                    (self.acc.rows_evaluated as f64 * (1.0 - APPROX_TOLERANCE)).floor() as u64;
                observed.insert("distinct_estimate".to_owned(), json!(est));
                observed.insert("approx".to_owned(), json!(true));
                if est < threshold {
                    // Report the shortfall as the duplicate count; no row
                    // samples exist in sketch mode.
                    self.acc.rows_failed = self.acc.rows_evaluated - est;
                    message = Some(format!(
                        "≈{} duplicate value(s) (distinct ≈ {est} of {} evaluated; HLL ±0.8%)",
                        self.acc.rows_failed, self.acc.rows_evaluated
                    ));
                } else {
                    message = None;
                }
            }
        }
        let mut out = outcome_row(self.meta, self.acc, message);
        out.observed.extend(observed);
        out
    }
}

// ──────────────────────── ratio checks (column) ─────────────────────────

/// Maximum null-ratio for a column.
pub(crate) struct NullRatioCheck {
    pub meta: Meta,
    pub max_ratio: f64,
    nulls: u64,
    total: u64,
}

impl NullRatioCheck {
    pub fn new(meta: Meta, max_ratio: f64) -> Self {
        NullRatioCheck {
            meta,
            max_ratio,
            nulls: 0,
            total: 0,
        }
    }
}

impl Check for NullRatioCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        if let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        {
            self.nulls += col.null_count();
            self.total += col.len() as u64;
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let ratio = if self.total == 0 {
            0.0
        } else {
            self.nulls as f64 / self.total as f64
        };
        let failed = ratio > self.max_ratio;
        let mut observed = std::collections::BTreeMap::new();
        observed.insert("null_ratio".to_owned(), json_number(ratio));
        let message =
            failed.then(|| format!("null ratio {:.4} exceeds {:.4}", ratio, self.max_ratio));
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed,
            rows_evaluated: Some(self.total),
            rows_failed: Some(self.nulls),
            observed,
            samples: Vec::new(),
            message,
        }
    }
}

/// Minimum unique-ratio for a column.
pub(crate) struct UniqueRatioCheck {
    pub meta: Meta,
    pub min_ratio: f64,
    seen: AHashSet<u64>,
    total: u64,
}

impl UniqueRatioCheck {
    pub fn new(meta: Meta, min_ratio: f64) -> Self {
        UniqueRatioCheck {
            meta,
            min_ratio,
            seen: AHashSet::new(),
            total: 0,
        }
    }
}

impl Check for UniqueRatioCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        if let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        {
            for value in col.raw.iter().flatten() {
                self.total += 1;
                self.seen.insert(hash_str(value));
            }
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let ratio = if self.total == 0 {
            1.0
        } else {
            self.seen.len() as f64 / self.total as f64
        };
        let failed = ratio < self.min_ratio;
        let mut observed = std::collections::BTreeMap::new();
        observed.insert("unique_ratio".to_owned(), json_number(ratio));
        observed.insert("distinct_count".to_owned(), json!(self.seen.len()));
        let message =
            failed.then(|| format!("unique ratio {:.4} below {:.4}", ratio, self.min_ratio));
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed,
            rows_evaluated: Some(self.total),
            rows_failed: None,
            observed,
            samples: Vec::new(),
            message,
        }
    }
}

// ──────────────────────────── row count ─────────────────────────────────

/// Dataset row-count bound.
pub(crate) struct RowCountCheck {
    pub meta: Meta,
    pub bound: u64,
    pub is_min: bool,
}

impl Check for RowCountCheck {
    fn eval_batch(&mut self, _view: &BatchView) -> Result<(), EngineError> {
        Ok(())
    }

    fn finalize(self: Box<Self>, total: u64) -> CheckOutcome {
        let failed = if self.is_min {
            total < self.bound
        } else {
            total > self.bound
        };
        let mut observed = std::collections::BTreeMap::new();
        observed.insert("row_count".to_owned(), json!(total));
        let message = failed.then(|| {
            if self.is_min {
                format!("{total} rows, fewer than the required {}", self.bound)
            } else {
                format!("{total} rows, more than the allowed {}", self.bound)
            }
        });
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed,
            rows_evaluated: None,
            rows_failed: None,
            observed,
            samples: Vec::new(),
            message,
        }
    }
}

// ──────────────────────────── freshness ─────────────────────────────────

/// Freshness: the newest value of a date/datetime column must be recent.
pub(crate) struct FreshnessCheck {
    pub meta: Meta,
    pub max_age_secs: i64,
    pub now_micros: i64,
    newest_micros: Option<i64>,
}

impl FreshnessCheck {
    pub fn new(meta: Meta, max_age_secs: i64, now_micros: i64) -> Self {
        FreshnessCheck {
            meta,
            max_age_secs,
            now_micros,
            newest_micros: None,
        }
    }
}

impl Check for FreshnessCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(col) = view
            .columns
            .get(self.meta.column.as_deref().unwrap_or_default())
        else {
            return Ok(());
        };
        let micros: Option<i64> = match &col.parsed {
            Parsed::Datetime(v) => v.iter().flatten().copied().max(),
            // Date: days → micros.
            Parsed::Date(v) => v
                .iter()
                .flatten()
                .map(|d| *d as i64 * 86_400 * 1_000_000)
                .max(),
            _ => None,
        };
        if let Some(m) = micros {
            self.newest_micros = Some(self.newest_micros.map_or(m, |cur| cur.max(m)));
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let mut observed = std::collections::BTreeMap::new();
        let (failed, message) = match self.newest_micros {
            None => (false, None),
            Some(newest) => {
                let age_secs = (self.now_micros - newest) / 1_000_000;
                let age_hours = age_secs as f64 / 3600.0;
                observed.insert("age_hours".to_owned(), json_number(age_hours));
                let failed = age_secs > self.max_age_secs;
                let msg =
                    failed.then(|| format!("newest row is {}h old", age_hours.round() as i64));
                (failed, msg)
            }
        };
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed,
            rows_evaluated: None,
            rows_failed: None,
            observed,
            samples: Vec::new(),
            message,
        }
    }
}

// ──────────────────────────── columns (schema) ──────────────────────────

/// Extra / exact column-set check.
pub(crate) struct ColumnsCheck {
    pub meta: Meta,
    pub allow_extra: bool,
    pub exact: bool,
    pub declared: Vec<String>,
    pub extra: Vec<String>,
    pub missing: Vec<String>,
}

impl Check for ColumnsCheck {
    fn eval_batch(&mut self, _view: &BatchView) -> Result<(), EngineError> {
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        let extra_violation = (!self.allow_extra || self.exact) && !self.extra.is_empty();
        let missing_violation = self.exact && !self.missing.is_empty();
        let failed = extra_violation || missing_violation;
        let mut observed = std::collections::BTreeMap::new();
        observed.insert("declared_columns".to_owned(), json!(self.declared.len()));
        if !self.extra.is_empty() {
            observed.insert("extra_columns".to_owned(), json!(self.extra));
        }
        if !self.missing.is_empty() {
            observed.insert("missing_columns".to_owned(), json!(self.missing));
        }
        let mut parts = Vec::new();
        if extra_violation {
            parts.push(format!("unexpected column(s): {}", self.extra.join(", ")));
        }
        if missing_violation {
            parts.push(format!("missing column(s): {}", self.missing.join(", ")));
        }
        let message = failed.then(|| parts.join("; "));
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed,
            rows_evaluated: None,
            rows_failed: None,
            observed,
            samples: Vec::new(),
            message,
        }
    }
}

/// A declared column absent from the source (`{col}.present`).
pub(crate) struct MissingColumnCheck {
    pub meta: Meta,
}

impl Check for MissingColumnCheck {
    fn eval_batch(&mut self, _view: &BatchView) -> Result<(), EngineError> {
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed: true,
            rows_evaluated: None,
            rows_failed: None,
            observed: std::collections::BTreeMap::new(),
            samples: Vec::new(),
            message: Some("declared column is missing from the source".to_owned()),
        }
    }
}

/// A check that could not be constructed (e.g. a regex that slipped past
/// `validate()` but failed to compile). Fails with the constructor's real
/// error instead of masquerading as a different check kind.
pub(crate) struct ErroredCheck {
    pub meta: Meta,
    pub error: String,
}

impl Check for ErroredCheck {
    fn eval_batch(&mut self, _view: &BatchView) -> Result<(), EngineError> {
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        CheckOutcome {
            id: self.meta.id,
            column: self.meta.column,
            kind: self.meta.kind.to_owned(),
            params: self.meta.params,
            severity: self.meta.severity,
            failed: true,
            rows_evaluated: None,
            rows_failed: None,
            observed: std::collections::BTreeMap::new(),
            samples: Vec::new(),
            message: Some(format!("check could not run: {}", self.error)),
        }
    }
}

// ──────────────────────────── custom_expr ───────────────────────────────

/// User Polars/SQL expression evaluated per row; a row fails when the boolean
/// result is `false`. A parse/type error marks the whole check errored (no
/// panic, no crash). Expressions have no filesystem or network access.
pub(crate) struct CustomExprCheck {
    pub meta: Meta,
    expr_src: String,
    parsed: Option<Expr>,
    error: Option<String>,
    acc: RowAccum,
}

impl CustomExprCheck {
    pub fn new(meta: Meta, expr_src: String) -> Self {
        let (parsed, error) = match polars::sql::sql_expr(&expr_src) {
            Ok(e) => (Some(e), None),
            Err(e) => (None, Some(format!("invalid custom_expr: {e}"))),
        };
        CustomExprCheck {
            meta,
            expr_src,
            parsed,
            error,
            acc: RowAccum::default(),
        }
    }
}

impl Check for CustomExprCheck {
    fn eval_batch(&mut self, view: &BatchView) -> Result<(), EngineError> {
        let Some(expr) = &self.parsed else {
            return Ok(());
        };
        if self.error.is_some() {
            return Ok(());
        }
        let result = view
            .df
            .clone()
            .lazy()
            .select([expr.clone().alias("__plexuspact_expr")])
            .collect();
        let frame = match result {
            Ok(f) => f,
            Err(e) => {
                self.error = Some(format!("custom_expr failed to evaluate: {e}"));
                return Ok(());
            }
        };
        let series = frame
            .column("__plexuspact_expr")?
            .as_materialized_series()
            .clone();
        let bools = match series.bool() {
            Ok(b) => b.clone(),
            Err(_) => {
                self.error = Some("custom_expr must evaluate to a boolean per row".to_owned());
                return Ok(());
            }
        };
        for (i, v) in bools.into_iter().enumerate() {
            self.acc.rows_evaluated += 1;
            // Null result → cannot determine → treated as pass (documented).
            if v == Some(false) {
                let abs = view.base_row + i as u64;
                self.acc
                    .record_failure(abs, String::new(), view.sample_budget);
            }
        }
        Ok(())
    }

    fn finalize(self: Box<Self>, _total: u64) -> CheckOutcome {
        if let Some(err) = self.error {
            return CheckOutcome {
                id: self.meta.id,
                column: self.meta.column,
                kind: self.meta.kind.to_owned(),
                params: self.meta.params,
                severity: self.meta.severity,
                failed: true,
                rows_evaluated: None,
                rows_failed: None,
                observed: std::collections::BTreeMap::new(),
                samples: Vec::new(),
                message: Some(err),
            };
        }
        let failed = self.acc.rows_failed > 0;
        let message =
            failed.then(|| format!("{} row(s) failed `{}`", self.acc.rows_failed, self.expr_src));
        outcome_row(self.meta, self.acc, message)
    }
}

// ─────────────────────────────── helpers ────────────────────────────────

/// Applies a string predicate to a check's column, recording non-matching,
/// non-null rows as failures. `pred` returns `true` for a *valid* value.
fn run_string_predicate(
    meta: &Meta,
    acc: &mut RowAccum,
    view: &BatchView,
    pred: impl Fn(&str) -> bool,
) {
    let Some(col) = view.columns.get(meta.column.as_deref().unwrap_or_default()) else {
        return;
    };
    for (i, cell) in col.raw.iter().enumerate() {
        let Some(value) = cell else { continue };
        acc.rows_evaluated += 1;
        if !pred(value) {
            let abs = view.base_row + i as u64;
            acc.record_failure(abs, value.clone(), view.sample_budget);
        }
    }
}

/// Builds a row-level [`CheckOutcome`] from an accumulator.
fn outcome_row(meta: Meta, acc: RowAccum, message: Option<String>) -> CheckOutcome {
    let mut observed = std::collections::BTreeMap::new();
    if let Some(r) = fail_ratio(acc.rows_failed, acc.rows_evaluated) {
        observed.insert("fail_ratio".to_owned(), json_number(r));
    }
    CheckOutcome {
        id: meta.id,
        column: meta.column,
        kind: meta.kind.to_owned(),
        params: meta.params,
        severity: meta.severity,
        failed: acc.rows_failed > 0,
        rows_evaluated: Some(acc.rows_evaluated),
        rows_failed: Some(acc.rows_failed),
        observed,
        samples: acc.samples,
        message,
    }
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = ahash::AHasher::default();
    s.hash(&mut hasher);
    hasher.finish()
}

/// JSON number that stays an integer when whole (stable output).
fn json_number(x: f64) -> serde_json::Value {
    if x.fract() == 0.0 && x.abs() < 9e15 {
        json!(x as i64)
    } else {
        serde_json::Number::from_f64(x)
            .map(serde_json::Value::Number)
            .unwrap_or(json!(null))
    }
}

/// Trims a whole float to an integer string (`11.0` → `11`).
fn trim_float(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}
