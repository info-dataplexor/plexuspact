//! Compiles a `Contract` (plus the set of source columns actually present) into
//! an ordered list of executable checks.

use std::collections::BTreeSet;

use plexuspact_contract::{ColumnCheck, Contract, DatasetCheck, Severity};
use serde_json::json;

use crate::checks::{
    BoundCheck, Check, ColumnsCheck, CustomExprCheck, EnumCheck, ErroredCheck, FormatCheck,
    FreshnessCheck, LengthCheck, Meta, MissingColumnCheck, NotEmptyCheck, NullRatioCheck,
    RegexCheck, RequiredCheck, RowCountCheck, TypeCheck, UniqueCheck, UniqueRatioCheck,
};
use crate::formats::format_name;

/// Builds the ordered check list for a contract against the given present
/// source columns. `now_micros` is the injected clock for freshness.
pub(crate) fn build(
    contract: &Contract,
    present: &BTreeSet<String>,
    now_micros: i64,
) -> Vec<Box<dyn Check>> {
    let mut checks: Vec<Box<dyn Check>> = Vec::new();

    // 1. Column-set (schema) check.
    let declared: Vec<String> = contract.columns.keys().cloned().collect();
    let extra: Vec<String> = present
        .iter()
        .filter(|c| !contract.columns.contains_key(*c))
        .cloned()
        .collect();
    let missing: Vec<String> = declared
        .iter()
        .filter(|c| !present.contains(*c))
        .cloned()
        .collect();
    checks.push(Box::new(ColumnsCheck {
        meta: Meta {
            id: "dataset.columns".to_owned(),
            column: None,
            kind: "columns",
            params: json!({
                "allow_extra_columns": contract.settings.allow_extra_columns,
                "columns_exact": contract.settings.columns_exact,
            }),
            severity: Severity::Error,
        },
        allow_extra: contract.settings.allow_extra_columns,
        exact: contract.settings.columns_exact,
        declared: declared.clone(),
        extra,
        missing,
    }));

    // 2. Per-column checks.
    for (name, col) in &contract.columns {
        if !present.contains(name) {
            checks.push(Box::new(MissingColumnCheck {
                meta: Meta {
                    id: format!("{name}.present"),
                    column: Some(name.clone()),
                    kind: "column_present",
                    params: json!({}),
                    severity: Severity::Error,
                },
            }));
            continue;
        }

        // Type conformance for every present declared column.
        checks.push(Box::new(TypeCheck::new(Meta {
            id: format!("{name}.type"),
            column: Some(name.clone()),
            kind: "type",
            params: json!({ "type": type_name(col.r#type) }),
            severity: contract.settings.on_type_mismatch,
        })));

        // Required (non-null) when declared.
        if col.required {
            checks.push(Box::new(RequiredCheck::new(Meta {
                id: format!("{name}.required"),
                column: Some(name.clone()),
                kind: "required",
                params: json!({ "required": true }),
                severity: Severity::Error,
            })));
        }

        // Value checks in declaration order.
        for check in &col.checks {
            checks.push(compile_column_check(name, check));
        }
    }

    // 3. Dataset checks.
    for dc in &contract.dataset_checks {
        checks.push(compile_dataset_check(dc, now_micros));
    }

    checks
}

fn compile_column_check(name: &str, check: &ColumnCheck) -> Box<dyn Check> {
    let severity = check.severity();
    match check {
        ColumnCheck::Unique { approx, .. } => Box::new(UniqueCheck::new(
            Meta {
                id: format!("{name}.unique"),
                column: Some(name.to_owned()),
                kind: "unique",
                params: json!({ "approx": approx }),
                severity,
            },
            *approx,
        )),
        ColumnCheck::NotEmptyString { .. } => Box::new(NotEmptyCheck::new(Meta {
            id: format!("{name}.not_empty_string"),
            column: Some(name.to_owned()),
            kind: "not_empty_string",
            params: json!({}),
            severity,
        })),
        ColumnCheck::Min { min, .. } => Box::new(BoundCheck::new(
            Meta {
                id: format!("{name}.min"),
                column: Some(name.to_owned()),
                kind: "min",
                params: json!({ "min": number_json(*min) }),
                severity,
            },
            *min,
            true,
        )),
        ColumnCheck::Max { max, .. } => Box::new(BoundCheck::new(
            Meta {
                id: format!("{name}.max"),
                column: Some(name.to_owned()),
                kind: "max",
                params: json!({ "max": number_json(*max) }),
                severity,
            },
            *max,
            false,
        )),
        ColumnCheck::Regex { regex, .. } => {
            let meta = Meta {
                id: format!("{name}.regex"),
                column: Some(name.to_owned()),
                kind: "regex",
                params: json!({ "regex": regex }),
                severity,
            };
            match RegexCheck::new(meta.clone(), regex) {
                Ok(c) => Box::new(c),
                // validate() already rejects bad regexes; this is a belt-and-suspenders fallback.
                Err(e) => Box::new(ErroredCheck {
                    meta,
                    error: e.to_string(),
                }),
            }
        }
        ColumnCheck::Enum { values, .. } => Box::new(EnumCheck::new(
            Meta {
                id: format!("{name}.enum"),
                column: Some(name.to_owned()),
                kind: "enum",
                params: json!({ "enum": values.iter().map(|v| v.to_string()).collect::<Vec<_>>() }),
                severity,
            },
            values,
        )),
        ColumnCheck::Length { length, .. } => Box::new(LengthCheck::new(
            Meta {
                id: format!("{name}.length"),
                column: Some(name.to_owned()),
                kind: "length",
                params: length_json(*length),
                severity,
            },
            *length,
        )),
        ColumnCheck::Format { format, .. } => Box::new(FormatCheck::new(
            Meta {
                id: format!("{name}.format.{}", format_name(*format)),
                column: Some(name.to_owned()),
                kind: "format",
                params: json!({ "format": format_name(*format) }),
                severity,
            },
            *format,
        )),
        ColumnCheck::NullRatioMax { ratio, .. } => Box::new(NullRatioCheck::new(
            Meta {
                id: format!("{name}.null_ratio_max"),
                column: Some(name.to_owned()),
                kind: "null_ratio_max",
                params: json!({ "ratio": ratio }),
                severity,
            },
            *ratio,
        )),
        ColumnCheck::UniqueRatioMin { ratio, .. } => Box::new(UniqueRatioCheck::new(
            Meta {
                id: format!("{name}.unique_ratio_min"),
                column: Some(name.to_owned()),
                kind: "unique_ratio_min",
                params: json!({ "ratio": ratio }),
                severity,
            },
            *ratio,
        )),
        ColumnCheck::CustomExpr { expr, .. } => Box::new(CustomExprCheck::new(
            Meta {
                id: format!("{name}.custom_expr"),
                column: Some(name.to_owned()),
                kind: "custom_expr",
                params: json!({ "expr": expr }),
                severity,
            },
            expr.clone(),
        )),
    }
}

fn compile_dataset_check(dc: &DatasetCheck, now_micros: i64) -> Box<dyn Check> {
    let severity = dc.severity();
    match dc {
        DatasetCheck::RowCountMin { count, .. } => Box::new(RowCountCheck {
            meta: Meta {
                id: "dataset.row_count_min".to_owned(),
                column: None,
                kind: "row_count_min",
                params: json!({ "count": count }),
                severity,
            },
            bound: *count,
            is_min: true,
        }),
        DatasetCheck::RowCountMax { count, .. } => Box::new(RowCountCheck {
            meta: Meta {
                id: "dataset.row_count_max".to_owned(),
                column: None,
                kind: "row_count_max",
                params: json!({ "count": count }),
                severity,
            },
            bound: *count,
            is_min: false,
        }),
        DatasetCheck::Freshness {
            column, max_age, ..
        } => {
            let secs = max_age.as_secs() as i64;
            Box::new(FreshnessCheck::new(
                Meta {
                    id: format!("dataset.freshness.{column}"),
                    column: Some(column.clone()),
                    kind: "freshness",
                    params: json!({
                        "column": column,
                        "max_age": humanize_secs(secs),
                        "max_age_secs": secs,
                    }),
                    severity,
                },
                secs,
                now_micros,
            ))
        }
        DatasetCheck::NullRatioMax { column, ratio, .. } => Box::new(NullRatioCheck::new(
            Meta {
                id: format!("dataset.null_ratio_max.{column}"),
                column: Some(column.clone()),
                kind: "null_ratio_max",
                params: json!({ "column": column, "ratio": ratio }),
                severity,
            },
            *ratio,
        )),
        DatasetCheck::UniqueRatioMin { column, ratio, .. } => Box::new(UniqueRatioCheck::new(
            Meta {
                id: format!("dataset.unique_ratio_min.{column}"),
                column: Some(column.clone()),
                kind: "unique_ratio_min",
                params: json!({ "column": column, "ratio": ratio }),
                severity,
            },
            *ratio,
        )),
        DatasetCheck::CustomExpr { expr, .. } => Box::new(CustomExprCheck::new(
            Meta {
                id: "dataset.custom_expr".to_owned(),
                column: None,
                kind: "custom_expr",
                params: json!({ "expr": expr }),
                severity,
            },
            expr.clone(),
        )),
    }
}

fn type_name(t: plexuspact_contract::ColType) -> &'static str {
    use plexuspact_contract::ColType::*;
    match t {
        String => "string",
        Int => "int",
        Float => "float",
        Bool => "bool",
        Date => "date",
        Datetime => "datetime",
    }
}

fn number_json(n: plexuspact_contract::Number) -> serde_json::Value {
    match n {
        plexuspact_contract::Number::Int(i) => json!(i),
        plexuspact_contract::Number::Float(f) => json!(f),
    }
}

fn length_json(spec: plexuspact_contract::LengthSpec) -> serde_json::Value {
    match spec {
        plexuspact_contract::LengthSpec::Exact(n) => json!({ "length": n }),
        plexuspact_contract::LengthSpec::Range(r) => json!({ "min": r.min, "max": r.max }),
    }
}

/// Compact human duration for report display: whole hours (`48h`), whole days
/// (`7d`), else falls back to `humantime`.
fn humanize_secs(secs: i64) -> String {
    if secs <= 0 {
        return "0s".to_owned();
    }
    let s = secs as u64;
    // Prefer hours for durations up to 3 days (so `48h` stays `48h`, matching
    // how contracts are typically written), days beyond that.
    if s % 3_600 == 0 && s / 3_600 <= 72 {
        format!("{}h", s / 3_600)
    } else if s % 86_400 == 0 {
        format!("{}d", s / 86_400)
    } else if s % 3_600 == 0 {
        format!("{}h", s / 3_600)
    } else {
        humantime::format_duration(std::time::Duration::from_secs(s)).to_string()
    }
}
