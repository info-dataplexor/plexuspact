//! Semantic lint of a parsed [`Contract`].
//!
//! Runs *after* parsing succeeds and returns **all** problems, not just the
//! first. Each finding carries a dotted path (`columns.age.checks[0]`), a
//! message, and a help string with a fix.

use std::collections::HashMap;
use std::fmt;

use crate::model::{ColType, ColumnCheck, Contract, DatasetCheck, LengthSpec};

/// How serious a lint finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    /// The contract is semantically broken and should not be used.
    Error,
    /// Suspicious but usable; the engine will still run.
    Warning,
}

impl fmt::Display for LintLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LintLevel::Error => f.write_str("error"),
            LintLevel::Warning => f.write_str("warning"),
        }
    }
}

/// A single semantic problem found in a contract.
#[derive(Debug, Clone, PartialEq)]
pub struct LintError {
    /// Severity of the finding.
    pub level: LintLevel,
    /// Dotted path to the offending element, e.g. `columns.age.checks[0]`.
    pub path: String,
    /// What is wrong.
    pub message: String,
    /// How to fix it, when we can suggest something concrete.
    pub help: Option<String>,
}

impl fmt::Display for LintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}", self.level, self.path, self.message)
    }
}

impl LintError {
    fn error(path: impl Into<String>, message: impl Into<String>, help: Option<String>) -> Self {
        LintError {
            level: LintLevel::Error,
            path: path.into(),
            message: message.into(),
            help,
        }
    }

    fn warning(path: impl Into<String>, message: impl Into<String>, help: Option<String>) -> Self {
        LintError {
            level: LintLevel::Warning,
            path: path.into(),
            message: message.into(),
            help,
        }
    }
}

/// Check kinds where more than one instance per column is meaningless.
const SINGLE_INSTANCE_KINDS: &[&str] = &[
    "min",
    "max",
    "length",
    "format",
    "enum",
    "null_ratio_max",
    "unique_ratio_min",
    "unique",
    "not_empty_string",
];

/// Semantically lints a contract, returning **all** findings.
pub fn validate(contract: &Contract) -> Vec<LintError> {
    let mut out = Vec::new();

    if contract.columns.is_empty() {
        out.push(LintError::error(
            "columns",
            "the contract defines no columns",
            Some(
                "add at least one column, e.g. `columns:\n  id: { type: string, required: true }`"
                    .into(),
            ),
        ));
    }

    for (name, col) in &contract.columns {
        lint_column(name, col, &mut out);
    }

    for (i, check) in contract.dataset_checks.iter().enumerate() {
        lint_dataset_check(i, check, contract, &mut out);
    }

    if contract.settings.columns_exact && contract.settings.allow_extra_columns {
        out.push(LintError::warning(
            "settings.columns_exact",
            "`columns_exact: true` conflicts with `allow_extra_columns: true`; exact matching wins",
            Some("set `allow_extra_columns: false` (or drop `columns_exact`) to make the intent explicit".into()),
        ));
    }

    out
}

/// Lints one column definition.
fn lint_column(name: &str, col: &crate::model::ColumnDef, out: &mut Vec<LintError>) {
    let mut min_seen: Option<(usize, f64)> = None;
    let mut max_seen: Option<(usize, f64)> = None;
    let mut kind_first: HashMap<&'static str, usize> = HashMap::new();

    for (i, check) in col.checks.iter().enumerate() {
        let path = format!("columns.{name}.checks[{i}]");

        // Duplicate check kinds where a second instance is meaningless.
        let kind = check.kind_name();
        if SINGLE_INSTANCE_KINDS.contains(&kind) {
            if let Some(first) = kind_first.get(kind) {
                out.push(LintError::error(
                    path.clone(),
                    format!("duplicate `{kind}` check (first defined at checks[{first}])"),
                    Some(format!("keep a single `{kind}` check per column")),
                ));
            } else {
                kind_first.insert(kind, i);
            }
        }

        match check {
            ColumnCheck::Regex { regex, .. } => {
                if let Err(e) = regex::Regex::new(regex) {
                    out.push(LintError::error(
                        path.clone(),
                        format!("invalid regex `{regex}`: {e}"),
                        Some(
                            "fix the pattern; Rust regex syntax is documented at docs.rs/regex"
                                .into(),
                        ),
                    ));
                }
            }
            ColumnCheck::Enum { values, .. } => {
                if values.is_empty() {
                    out.push(LintError::error(
                        path.clone(),
                        "`enum` check has no values, so every row would fail",
                        Some("list the allowed values, e.g. `{ enum: [free, pro] }`".into()),
                    ));
                }
            }
            ColumnCheck::Length {
                length: LengthSpec::Range(r),
                ..
            } => {
                if r.min.is_none() && r.max.is_none() {
                    out.push(LintError::error(
                        path.clone(),
                        "`length` constrains nothing: neither `min` nor `max` is set".to_string(),
                        Some(
                            "give a bound, e.g. `{ length: 2 }` or `{ length: { min: 1, max: 10 } }`"
                                .into(),
                        ),
                    ));
                } else if let (Some(lo), Some(hi)) = (r.min, r.max) {
                    if lo > hi {
                        out.push(LintError::error(
                            path.clone(),
                            format!("`length` range is impossible: min {lo} > max {hi}"),
                            Some("swap the bounds, e.g. `{ length: { min: 1, max: 10 } }`".into()),
                        ));
                    }
                }
            }
            ColumnCheck::NullRatioMax { ratio, .. } | ColumnCheck::UniqueRatioMin { ratio, .. } => {
                if !(0.0..=1.0).contains(ratio) {
                    out.push(LintError::error(
                        path.clone(),
                        format!("ratio {ratio} is outside [0, 1]"),
                        Some("ratios are fractions, e.g. `0.1` for 10%".into()),
                    ));
                }
            }
            ColumnCheck::Min { min, .. } => {
                if !min.as_f64().is_finite() {
                    out.push(LintError::error(
                        path.clone(),
                        "`min` must be a finite number (not NaN or infinity)".to_string(),
                        Some("use a concrete bound, e.g. `{ min: 0 }`".into()),
                    ));
                }
                min_seen = min_seen.or(Some((i, min.as_f64())));
            }
            ColumnCheck::Max { max, .. } => {
                if !max.as_f64().is_finite() {
                    out.push(LintError::error(
                        path.clone(),
                        "`max` must be a finite number (not NaN or infinity)".to_string(),
                        Some("use a concrete bound, e.g. `{ max: 100 }`".into()),
                    ));
                }
                max_seen = max_seen.or(Some((i, max.as_f64())));
            }
            _ => {}
        }

        // Type compatibility.
        match check {
            ColumnCheck::Min { .. } | ColumnCheck::Max { .. } if !col.r#type.is_ordered() => {
                out.push(LintError::error(
                    path.clone(),
                    format!(
                        "`{}` check is not meaningful on a `{}` column",
                        check.kind_name(),
                        col.r#type
                    ),
                    Some("min/max apply to int, float, date, and datetime columns".into()),
                ));
            }
            ColumnCheck::Format { .. } if col.r#type != ColType::String => {
                out.push(LintError::error(
                    path.clone(),
                    format!(
                        "`format` check requires a string column, but this column is `{}`",
                        col.r#type
                    ),
                    Some("declare the column as `type: string`, or drop the format check".into()),
                ));
            }
            _ => {}
        }
    }

    if let (Some((_, min)), Some((max_idx, max))) = (min_seen, max_seen) {
        if min > max {
            out.push(LintError::error(
                format!("columns.{name}.checks[{max_idx}]"),
                format!("impossible range: min {min} > max {max}; every row would fail"),
                Some("lower `min` or raise `max` so the range is satisfiable".into()),
            ));
        }
    }
}

/// Lints one dataset-level check.
fn lint_dataset_check(
    index: usize,
    check: &DatasetCheck,
    contract: &Contract,
    out: &mut Vec<LintError>,
) {
    let path = format!("dataset_checks[{index}]");
    let unknown_column_help = |column: &str| {
        Some(format!(
            "declare `{column}` under `columns:` or point the check at one of: {}",
            contract
                .columns
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    match check {
        DatasetCheck::Freshness { column, .. } => match contract.columns.get(column) {
            None => out.push(LintError::error(
                path,
                format!("`freshness` references unknown column `{column}`"),
                unknown_column_help(column),
            )),
            Some(col) if !col.r#type.is_temporal() => out.push(LintError::error(
                path,
                format!(
                    "`freshness` requires a date or datetime column, but `{column}` is `{}`",
                    col.r#type
                ),
                Some(format!(
                    "declare `{column}` as `type: datetime` (or `date`)"
                )),
            )),
            Some(_) => {}
        },
        DatasetCheck::NullRatioMax { column, ratio, .. }
        | DatasetCheck::UniqueRatioMin { column, ratio, .. } => {
            if !contract.columns.contains_key(column) {
                out.push(LintError::error(
                    path.clone(),
                    format!(
                        "`{}` references unknown column `{column}`",
                        check.kind_name()
                    ),
                    unknown_column_help(column),
                ));
            }
            if !(0.0..=1.0).contains(ratio) {
                out.push(LintError::error(
                    path,
                    format!("ratio {ratio} is outside [0, 1]"),
                    Some("ratios are fractions, e.g. `0.1` for 10%".into()),
                ));
            }
        }
        DatasetCheck::RowCountMin { .. }
        | DatasetCheck::RowCountMax { .. }
        | DatasetCheck::CustomExpr { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::parse::parse_str;

    fn contract(yaml: &str) -> Contract {
        parse_str(yaml, "test.yaml").unwrap()
    }

    fn lints(yaml: &str) -> Vec<LintError> {
        validate(&contract(yaml))
    }

    const HEAD: &str = "apiVersion: v1\ndataset: t\n";

    #[test]
    fn clean_contract_has_no_lints() {
        let yaml = format!(
            "{HEAD}columns:\n  id: {{ type: string, required: true, checks: [unique] }}\n  age: {{ type: int, checks: [{{ min: 0 }}, {{ max: 120 }}] }}\n"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn vacuous_length_range_is_rejected() {
        let yaml =
            format!("{HEAD}columns:\n  c: {{ type: string, checks: [{{ length: {{}} }}] }}\n");
        let out = lints(&yaml);
        assert!(
            out.iter().any(|l| l.message.contains("constrains nothing")),
            "got: {out:?}"
        );
    }

    #[test]
    fn non_finite_bounds_are_rejected() {
        let yaml = format!("{HEAD}columns:\n  n: {{ type: float, checks: [{{ min: .nan }}] }}\n");
        let out = lints(&yaml);
        assert!(
            out.iter().any(|l| l.message.contains("finite")),
            "NaN min must lint; got: {out:?}"
        );
    }

    #[test]
    fn empty_columns_map() {
        let out = lints(&format!("{HEAD}columns: {{}}\n"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, "columns");
        assert_eq!(out[0].level, LintLevel::Error);
    }

    #[test]
    fn invalid_regex() {
        let yaml =
            format!("{HEAD}columns:\n  c: {{ type: string, checks: [{{ regex: \"([\" }}] }}\n");
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("invalid regex"),
            "{}",
            out[0].message
        );
        assert_eq!(out[0].path, "columns.c.checks[0]");
    }

    #[test]
    fn min_greater_than_max() {
        let yaml = format!(
            "{HEAD}columns:\n  age: {{ type: int, checks: [{{ min: 100 }}, {{ max: 18 }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("min 100 > max 18"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn min_max_valid_range_no_lint() {
        let yaml = format!(
            "{HEAD}columns:\n  age: {{ type: int, checks: [{{ min: 18 }}, {{ max: 18 }}] }}\n"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn empty_enum() {
        let yaml = format!("{HEAD}columns:\n  p: {{ type: string, checks: [{{ enum: [] }}] }}\n");
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(out[0].message.contains("no values"), "{}", out[0].message);
    }

    #[test]
    fn length_min_greater_than_max() {
        let yaml = format!(
            "{HEAD}columns:\n  c: {{ type: string, checks: [{{ length: {{ min: 10, max: 2 }} }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("min 10 > max 2"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn column_ratio_out_of_range() {
        let yaml = format!(
            "{HEAD}columns:\n  c: {{ type: string, checks: [{{ null_ratio_max: 1.5 }}, {{ unique_ratio_min: -0.1 }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|l| l.message.contains("outside [0, 1]")));
    }

    #[test]
    fn freshness_unknown_column() {
        let yaml = format!(
            "{HEAD}columns:\n  id: {{ type: string }}\ndataset_checks:\n  - freshness: {{ column: created, max_age: 1h }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("unknown column `created`"),
            "{}",
            out[0].message
        );
        assert_eq!(out[0].path, "dataset_checks[0]");
    }

    #[test]
    fn freshness_non_temporal_column() {
        let yaml = format!(
            "{HEAD}columns:\n  id: {{ type: string }}\ndataset_checks:\n  - freshness: {{ column: id, max_age: 1h }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("date or datetime"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn freshness_on_date_column_is_fine() {
        let yaml = format!(
            "{HEAD}columns:\n  d: {{ type: date }}\ndataset_checks:\n  - freshness: {{ column: d, max_age: 1h }}\n"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn dataset_ratio_checks_unknown_column_and_range() {
        let yaml = format!(
            "{HEAD}columns:\n  id: {{ type: string }}\ndataset_checks:\n  - null_ratio_max: {{ column: nope, ratio: 2.0 }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 2);
        assert!(out[0].message.contains("unknown column `nope`"));
        assert!(out[1].message.contains("outside [0, 1]"));
    }

    #[test]
    fn duplicate_min_checks() {
        let yaml = format!(
            "{HEAD}columns:\n  age: {{ type: int, checks: [{{ min: 1 }}, {{ min: 2 }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("duplicate `min`"),
            "{}",
            out[0].message
        );
        assert_eq!(out[0].path, "columns.age.checks[1]");
    }

    #[test]
    fn duplicate_regex_is_allowed() {
        let yaml = format!(
            "{HEAD}columns:\n  c: {{ type: string, checks: [{{ regex: \"^a\" }}, {{ regex: \"b$\" }}] }}\n"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn columns_exact_conflict_is_warning() {
        let yaml = format!(
            "{HEAD}columns:\n  id: {{ type: string }}\nsettings:\n  columns_exact: true\n  allow_extra_columns: true\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].level, LintLevel::Warning);
        assert_eq!(out[0].path, "settings.columns_exact");
    }

    #[test]
    fn columns_exact_with_defaults_also_warns() {
        // allow_extra_columns defaults to true, so columns_exact alone conflicts.
        let yaml =
            format!("{HEAD}columns:\n  id: {{ type: string }}\nsettings:\n  columns_exact: true\n");
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].level, LintLevel::Warning);
    }

    #[test]
    fn format_on_non_string_column() {
        let yaml = format!("{HEAD}columns:\n  n: {{ type: int, checks: [{{ format: email }}] }}\n");
        let out = lints(&yaml);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].message.contains("requires a string column"),
            "{}",
            out[0].message
        );
    }

    #[test]
    fn min_max_on_non_numeric_columns() {
        let yaml = format!(
            "{HEAD}columns:\n  s: {{ type: string, checks: [{{ min: 1 }}] }}\n  b: {{ type: bool, checks: [{{ max: 1 }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 2);
        assert!(
            out[0].message.contains("not meaningful on a `string`"),
            "{}",
            out[0].message
        );
        assert!(
            out[1].message.contains("not meaningful on a `bool`"),
            "{}",
            out[1].message
        );
    }

    #[test]
    fn min_max_on_date_column_is_fine() {
        let yaml = format!("{HEAD}columns:\n  d: {{ type: date, checks: [{{ min: 0 }}] }}\n");
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn returns_all_errors_not_just_first() {
        let yaml = format!(
            "{HEAD}columns:\n  a: {{ type: string, checks: [{{ regex: \"([\" }}, {{ enum: [] }}, {{ min: 1 }}] }}\n"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 3, "{out:?}");
    }
}
