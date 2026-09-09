//! Semantic lint of a parsed [`Contract`].
//!
//! Runs *after* parsing succeeds and returns **all** problems, not just the
//! first. Each finding carries a dotted path (`columns.age.checks[0]`), a
//! message, and a help string with a fix.

use std::collections::HashMap;
use std::fmt;

use crate::model::{ColType, ColumnCheck, Contract, DatasetCheck, InputFormat, LengthSpec};

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

    for (i, consumer) in contract.consumers.iter().enumerate() {
        lint_consumer(i, consumer, contract, &mut out);
    }

    if let Some(m) = &contract.migration {
        if !crate::model::is_iso_date(&m.window_ends) {
            out.push(LintError::error(
                "migration.window_ends",
                format!("`{}` is not a date", m.window_ends),
                Some("write it as `YYYY-MM-DD`, e.g. `window_ends: 2026-12-31`".into()),
            ));
        }
        // A window with no instructions is a deadline. The note is the part
        // that tells a consumer what to actually do before it, and without it
        // the announcement moves the work onto them without saying what it is.
        if m.note.as_deref().map(str::trim).unwrap_or("").is_empty() {
            out.push(LintError::warning(
                "migration.note",
                "the migration window says when, but not what to do",
                Some(
                    "add a `note` naming the replacement, e.g. `read `amount_minor`; \
                     `amount` becomes minor units`"
                        .into(),
                ),
            ));
        }
    }

    if contract.settings.columns_exact && contract.settings.allow_extra_columns {
        out.push(LintError::warning(
            "settings.columns_exact",
            "`columns_exact: true` conflicts with `allow_extra_columns: true`; exact matching wins",
            Some("set `allow_extra_columns: false` (or drop `columns_exact`) to make the intent explicit".into()),
        ));
    }

    lint_input(contract, &mut out);

    out
}

/// Lints `settings.input`: the reading instructions must be internally
/// consistent, and a fixed-width layout must actually be a layout.
fn lint_input(contract: &Contract, out: &mut Vec<LintError>) {
    let input = &contract.settings.input;
    let format = input.format;

    if let Some(d) = &input.delimiter {
        if d.chars().count() != 1 {
            out.push(LintError::error(
                "settings.input.delimiter",
                format!("`{d}` is not a single character"),
                Some("the delimiter is one character, e.g. `delimiter: \";\"` or `delimiter: \"\\t\"`".into()),
            ));
        }
    }

    // An option that belongs to one format, on a contract that pins another,
    // is a mistake somebody should hear about now rather than a silent no-op
    // at run time. When no format is pinned the file extension decides, and
    // the option may well apply — no complaint then.
    let only_for = |applies: &[InputFormat], key: &str, out: &mut Vec<LintError>| {
        if let Some(f) = format {
            if !applies.contains(&f) {
                let names: Vec<&str> = applies.iter().map(|f| f.name()).collect();
                out.push(LintError::error(
                    format!("settings.input.{key}"),
                    format!("`{key}` does not apply to {f} input"),
                    Some(format!(
                        "`{key}` is read only for {} input; drop it, or change `format`",
                        names.join("/")
                    )),
                ));
            }
        }
    };
    if input.delimiter.is_some() {
        only_for(&[InputFormat::Csv, InputFormat::Tsv], "delimiter", out);
    }
    if input.json_path.is_some() {
        only_for(&[InputFormat::Json], "json_path", out);
    }
    if input.sheet.is_some() {
        only_for(&[InputFormat::Excel], "sheet", out);
    }
    if input.xml_record.is_some() {
        only_for(&[InputFormat::Xml], "xml_record", out);
    }
    if input.has_header.is_some() {
        only_for(
            &[
                InputFormat::Csv,
                InputFormat::Tsv,
                InputFormat::Excel,
                InputFormat::FixedWidth,
            ],
            "has_header",
            out,
        );
    }
    if input.skip_rows.is_some() {
        only_for(
            &[
                InputFormat::Csv,
                InputFormat::Tsv,
                InputFormat::Excel,
                InputFormat::FixedWidth,
            ],
            "skip_rows",
            out,
        );
    }

    if format == Some(InputFormat::FixedWidth) && input.fixed_width.is_empty() {
        out.push(LintError::error(
            "settings.input.fixed_width",
            "`format: fixed_width` needs the field layout",
            Some(
                "list the fields with their character positions, e.g. `fixed_width:\n  - { name: id, start: 1, end: 8 }\n  - { name: amount, width: 12 }`"
                    .into(),
            ),
        ));
    }
    if !input.fixed_width.is_empty() {
        only_for(&[InputFormat::FixedWidth], "fixed_width", out);
        lint_fixed_width(contract, out);
    }
}

/// Lints a fixed-width layout: every field has a width, none overlap, names
/// are unique, and every declared column is produced by some field.
fn lint_fixed_width(contract: &Contract, out: &mut Vec<LintError>) {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let mut cursor: u32 = 1; // next free position
    for (i, f) in contract.settings.input.fixed_width.iter().enumerate() {
        let path = format!("settings.input.fixed_width[{i}]");
        if f.name.trim().is_empty() {
            out.push(LintError::error(
                format!("{path}.name"),
                "the field has no name",
                Some("give every field the column name it is read into".into()),
            ));
        }
        if let Some(prev) = seen.insert(f.name.as_str(), i) {
            out.push(LintError::error(
                format!("{path}.name"),
                format!("`{}` is already the name of field [{prev}]", f.name),
                Some("field names become column names, so each must be unique".into()),
            ));
        }
        let start = f.start.unwrap_or(cursor);
        if start == 0 {
            out.push(LintError::error(
                format!("{path}.start"),
                "positions are 1-based; `start: 0` names no character",
                Some("the first character of the line is position 1".into()),
            ));
        }
        if start < cursor {
            out.push(LintError::error(
                format!("{path}.start"),
                format!(
                    "`{}` starts at {start}, inside the previous field (which ends at {})",
                    f.name,
                    cursor.saturating_sub(1)
                ),
                Some("fields lie on the line in order and never overlap; check the layout".into()),
            ));
        }
        let end = match (f.end, f.width) {
            (Some(_), Some(_)) => {
                out.push(LintError::error(
                    format!("{path}.width"),
                    format!("`{}` gives both `end` and `width`", f.name),
                    Some("keep one: `end` is the last position, `width` the length".into()),
                ));
                f.end
            }
            (Some(e), None) => {
                if e < start {
                    out.push(LintError::error(
                        format!("{path}.end"),
                        format!("`{}` ends at {e}, before it starts at {start}", f.name),
                        Some("`end` is inclusive and cannot be before `start`".into()),
                    ));
                }
                Some(e)
            }
            (None, Some(w)) => {
                if w == 0 {
                    out.push(LintError::error(
                        format!("{path}.width"),
                        format!("`{}` has width 0", f.name),
                        Some("a field is at least one character wide".into()),
                    ));
                }
                Some(start + w.saturating_sub(1))
            }
            (None, None) => {
                out.push(LintError::error(
                    path.to_string(),
                    format!("`{}` has no `end` and no `width`", f.name),
                    Some("say where the field stops: `end: 20` (inclusive) or `width: 12`".into()),
                ));
                None
            }
        };
        cursor = end.map(|e| e.max(start) + 1).unwrap_or(cursor);
    }

    for name in contract.columns.keys() {
        if !seen.contains_key(name.as_str()) {
            out.push(LintError::warning(
                format!("columns.{name}"),
                format!("`{name}` is declared, but no fixed-width field is named `{name}`"),
                Some(
                    "add a field for it under `settings.input.fixed_width`, or the column will be reported missing"
                        .to_string(),
                ),
            ));
        }
    }
}

/// Lints one declared consumer.
///
/// A consumer that names a column the contract does not declare is a
/// dependency that matches nothing. Nothing about the run changes, which is
/// exactly the danger: the consumer looks declared, is reported as unaffected
/// by every change, and the first anyone hears of the typo is the morning
/// their dashboard is empty. So it is an error, not a warning — an unstated
/// dependency at least reads as unstated.
fn lint_consumer(
    i: usize,
    consumer: &crate::model::Consumer,
    contract: &Contract,
    out: &mut Vec<LintError>,
) {
    let path = format!("consumers[{i}]");

    if consumer.name.trim().is_empty() {
        out.push(LintError::error(
            format!("{path}.name"),
            "consumer has no name",
            Some("name the team, service or dashboard, e.g. `{ name: finance-weekly }`".into()),
        ));
    }

    // A tier outside 1–3 is not a stricter consumer, it is a typo that will be
    // sorted and compared against real ones. Reject it here rather than let a
    // "tier 0" quietly outrank everything on every screen downstream.
    if let Some(tier) = consumer.tier {
        use crate::model::{TIER_LEAST_CRITICAL, TIER_MOST_CRITICAL};
        if !(TIER_MOST_CRITICAL..=TIER_LEAST_CRITICAL).contains(&tier) {
            out.push(LintError::error(
                format!("{path}.tier"),
                format!(
                    "consumer `{}` has tier {tier}, which is outside 1–3",
                    consumer.name
                ),
                Some(
                    "use 1 for the ones that stop the business, 3 for the ones that can wait"
                        .into(),
                ),
            ));
        }
    }

    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (j, column) in consumer.reads.iter().enumerate() {
        let at = format!("{path}.reads[{j}]");

        if let Some(first) = seen.insert(column.as_str(), j) {
            out.push(LintError::warning(
                at.clone(),
                format!("`{column}` is listed twice (first at reads[{first}])"),
                Some("remove the duplicate; it does not widen the dependency".into()),
            ));
            continue;
        }

        if !contract.columns.contains_key(column) {
            let help = crate::suggest::did_you_mean(
                column,
                contract.columns.keys().map(String::as_str),
            )
            .map_or_else(
                || {
                    "name a column this contract declares, or drop `reads` to depend on the whole \
                     dataset"
                        .to_owned()
                },
                |s| format!("did you mean `{s}`?"),
            );
            out.push(LintError::error(
                at,
                format!(
                    "consumer `{}` reads `{column}`, which this contract does not declare",
                    consumer.name
                ),
                Some(help),
            ));
        }
    }
}

/// Lints one column definition.
/// Lints a column's retirement plan: the `stability` promise and the date.
///
/// The two fields only work as a pair. `stability: deprecated` with no date is
/// a column somebody has stopped supporting without saying until when, which
/// leaves a consumer knowing they have to move and not knowing by when — the
/// half of the announcement that is hard to act on. A `sunset` date on a column
/// still marked stable is the same failure from the other side: the schedule is
/// there and the warning is not, so a consumer reading `stability` sees a
/// promise the date contradicts.
///
/// Both are warnings, not errors. Each half on its own is more than most
/// contracts say today, and refusing to register a partial announcement would
/// mean the supplier says nothing instead.
fn lint_retirement(name: &str, col: &crate::model::ColumnDef, out: &mut Vec<LintError>) {
    use crate::model::Stability;

    if let Some(date) = &col.sunset {
        if !crate::model::is_iso_date(date) {
            out.push(LintError::error(
                format!("columns.{name}.sunset"),
                format!("`{date}` is not a date"),
                Some("write it as `YYYY-MM-DD`, e.g. `sunset: 2026-12-31`".into()),
            ));
        }
        if col.stability != Stability::Deprecated {
            out.push(LintError::warning(
                format!("columns.{name}.stability"),
                format!(
                    "`{name}` has a removal date but is still marked `{}`",
                    col.stability
                ),
                Some(
                    "set `stability: deprecated` so a consumer reading the column sees the \
                     warning, not just the date"
                        .into(),
                ),
            ));
        }
    } else if col.stability == Stability::Deprecated {
        out.push(LintError::warning(
            format!("columns.{name}.sunset"),
            format!("`{name}` is deprecated with no removal date"),
            Some(
                "add `sunset: YYYY-MM-DD` — a consumer told to move and not told by when \
                 cannot schedule the work"
                    .into(),
            ),
        ));
    }

    if col.stability == Stability::Deprecated && col.required {
        out.push(LintError::warning(
            format!("columns.{name}.required"),
            format!("`{name}` is deprecated but still required, so nulls still fail"),
            Some(
                "drop `required: true` if consumers are meant to stop populating it before \
                 the sunset date"
                    .into(),
            ),
        ));
    }
}

fn lint_column(name: &str, col: &crate::model::ColumnDef, out: &mut Vec<LintError>) {
    lint_retirement(name, col, out);

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

    #[test]
    fn a_consumer_reading_a_declared_column_is_clean() {
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance, reads: [id] }}
columns:
  id: {{ type: string }}
"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn a_consumer_that_names_no_columns_is_clean() {
        // Silence means the whole dataset, which is a legitimate answer and the
        // shape every contract written before `reads` existed already has.
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance }}
columns:
  id: {{ type: string }}
"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn a_ranked_and_typed_consumer_is_clean() {
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: exec, kind: dashboard, tier: 1, reads: [id] }}
  - {{ name: bot, kind: ai_agent, tier: 3 }}
columns:
  id: {{ type: string }}
"
        );
        assert_eq!(lints(&yaml), vec![]);
    }

    #[test]
    fn a_tier_outside_one_to_three_is_an_error() {
        // Not a stricter consumer: a typo that would sort above every real one
        // on every screen downstream, and outrank a tier 1 nobody meant to
        // outrank.
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: exec, tier: 0 }}
columns:
  id: {{ type: string }}
"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].level, LintLevel::Error);
        assert_eq!(out[0].path, "consumers[0].tier");
        assert!(out[0].message.contains("outside"), "{out:?}");
    }

    #[test]
    fn a_consumer_reading_an_undeclared_column_is_an_error() {
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance, reads: [emial] }}
columns:
  email: {{ type: string }}
"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].level, LintLevel::Error);
        assert_eq!(out[0].path, "consumers[0].reads[0]");
        assert!(out[0].message.contains("does not declare"), "{out:?}");
        // A typo is the likely cause, so the fix is offered rather than described.
        assert_eq!(out[0].help.as_deref(), Some("did you mean `email`?"));
    }

    #[test]
    fn an_unguessable_column_gets_the_general_advice() {
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance, reads: [zzzzzzzz] }}
columns:
  email: {{ type: string }}
"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(
            out[0].help.as_deref().unwrap().contains("whole dataset"),
            "{out:?}"
        );
    }

    #[test]
    fn a_repeated_column_is_a_warning_not_an_error() {
        // It does not widen the dependency and it does not break anything, so
        // stopping a register over it would be the bureaucracy that gets the
        // field left blank.
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance, reads: [id, id] }}
columns:
  id: {{ type: string }}
"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].level, LintLevel::Warning);
    }

    #[test]
    fn an_unnamed_consumer_is_an_error() {
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: \"  \" }}
columns:
  id: {{ type: string }}
"
        );
        let out = lints(&yaml);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].path, "consumers[0].name");
    }

    #[test]
    fn every_bad_column_in_one_consumer_is_reported() {
        // The lint pass exists to return all findings; fixing one typo and
        // re-running to find the next is the workflow it was built to avoid.
        let yaml = format!(
            "{HEAD}consumers:
  - {{ name: finance, reads: [nope, alsonope] }}
columns:
  id: {{ type: string }}
"
        );
        assert_eq!(lints(&yaml).len(), 2);
    }
    #[test]
    fn half_an_announcement_is_a_warning_not_an_error() {
        // A date with no warning: the schedule is there and `stability` still
        // says the column is safe to build on.
        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, sunset: 2026-12-31 }\n",
        );
        let found = validate(&c);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].level, LintLevel::Warning);
        assert_eq!(found[0].path, "columns.id.stability");

        // And a warning with no date: move, by an unspecified time.
        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated }\n",
        );
        let found = validate(&c);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].path, "columns.id.sunset");

        // Both halves, and it is clean.
        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated, sunset: 2026-12-31 }\n",
        );
        assert_eq!(validate(&c), vec![]);
    }

    #[test]
    fn a_sunset_that_is_not_a_date_is_an_error() {
        for bad in ["31-12-2026", "2026-13-01", "2026-02-30", "2026-2-1", "soon"] {
            let c = contract(&format!(
                "apiVersion: v1\ndataset: t\ncolumns:\n  id: {{ type: string, stability: deprecated, sunset: \"{bad}\" }}\n"
            ));
            let found = validate(&c);
            assert!(
                found
                    .iter()
                    .any(|e| e.level == LintLevel::Error && e.path == "columns.id.sunset"),
                "{bad} was accepted: {found:#?}"
            );
        }
        // A leap day in a leap year is a real date.
        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, stability: deprecated, sunset: 2028-02-29 }\n",
        );
        assert_eq!(validate(&c), vec![]);
    }

    #[test]
    fn a_migration_window_with_no_instructions_is_a_warning() {
        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\nmigration: { window_ends: 2026-12-31 }\n",
        );
        let found = validate(&c);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].level, LintLevel::Warning);
        assert_eq!(found[0].path, "migration.note");

        let c = contract(
            "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\nmigration: { window_ends: nope, note: move }\n",
        );
        let found = validate(&c);
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].level, LintLevel::Error);
        assert_eq!(found[0].path, "migration.window_ends");
    }
}
