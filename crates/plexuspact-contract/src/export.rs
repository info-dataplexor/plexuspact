//! Export a [`Contract`] to artifacts consumed by other data tools.
//!
//! Like [`crate::schema`], every function here is a **pure projection of the
//! contract model** — no IO, no new dependencies. The first target is
//! Databricks [Delta Live Tables] (DLT) *expectations*: contract checks become
//! per-row SQL predicates so the same rules PlexusPact enforces at the source
//! also run inside the DLT pipeline.
//!
//! Error-severity checks map to `ON VIOLATION FAIL UPDATE` (a violation stops
//! the pipeline, matching PlexusPact's exit-1 semantics); warn-severity checks
//! are recorded without failing the update. Checks that are inherently
//! aggregate (uniqueness, null-ratio, row counts) or engine-specific
//! (`custom_expr`, a Polars expression) cannot be expressed as row-level DLT
//! expectations and are listed in a trailing comment instead of silently
//! dropped.
//!
//! [Delta Live Tables]: https://docs.databricks.com/delta-live-tables/expectations.html

use std::fmt::Write as _;

use crate::model::{ColumnCheck, Contract, EnumValue, KnownFormat, LengthSpec, Number, Severity};

/// Output language for the Databricks DLT export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DltLang {
    /// SQL `CONSTRAINT … EXPECT (…)` clauses.
    Sql,
    /// Python `@dlt.expect_all*` decorators.
    Python,
}

/// One generated expectation: a stable name, a Spark-SQL boolean predicate, and
/// whether a violation should fail the pipeline (error) or only be recorded (warn).
struct Expectation {
    name: String,
    predicate: String,
    fail: bool,
}

/// Renders `contract` as Databricks DLT expectations in the requested language.
pub fn databricks_dlt(contract: &Contract, lang: DltLang) -> String {
    let mut expectations: Vec<Expectation> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();
    let mut used_names: std::collections::HashSet<String> = Default::default();

    for (col, def) in &contract.columns {
        // `required` → NOT NULL, at error severity.
        if def.required {
            push(
                &mut expectations,
                &mut used_names,
                format!("{col}_not_null"),
                format!("{} IS NOT NULL", ident(col)),
                true,
            );
        }
        for check in &def.checks {
            match column_predicate(col, check) {
                Some(pred) => push(
                    &mut expectations,
                    &mut used_names,
                    format!("{col}_{}", check.kind_name()),
                    pred,
                    check.severity() == Severity::Error,
                ),
                None => unsupported.push(format!(
                    "column `{col}` check `{}` — not a row-level predicate; \
                     keep enforcing it with `plexuspact check`",
                    check.kind_name()
                )),
            }
        }
    }

    for ds in &contract.dataset_checks {
        unsupported.push(format!(
            "dataset check `{}` — aggregate, not a row-level predicate; \
             keep enforcing it with `plexuspact check`",
            ds.kind_name()
        ));
    }

    match lang {
        DltLang::Sql => render_sql(contract, &expectations, &unsupported),
        DltLang::Python => render_python(contract, &expectations, &unsupported),
    }
}

/// Line-comment prefix for each output language.
const fn comment_prefix(lang: DltLang) -> &'static str {
    match lang {
        DltLang::Sql => "--",
        DltLang::Python => "#",
    }
}

/// Adds an expectation, ensuring the constraint name is unique and identifier-safe.
fn push(
    out: &mut Vec<Expectation>,
    used: &mut std::collections::HashSet<String>,
    raw_name: String,
    predicate: String,
    fail: bool,
) {
    let base = sanitize_name(&raw_name);
    let mut name = base.clone();
    let mut n = 2;
    while !used.insert(name.clone()) {
        name = format!("{base}_{n}");
        n += 1;
    }
    out.push(Expectation {
        name,
        predicate,
        fail,
    });
}

/// Maps a column check to a Spark-SQL predicate that is TRUE for conforming rows.
///
/// Value checks permit NULL (`col IS NULL OR …`) so they mirror PlexusPact's
/// semantics, where nullability is governed by `required`, not by value checks.
/// Returns `None` for checks that cannot be expressed per-row.
fn column_predicate(col: &str, check: &ColumnCheck) -> Option<String> {
    let c = ident(col);
    let pred = match check {
        ColumnCheck::NotEmptyString { .. } => format!("{c} IS NULL OR length({c}) > 0"),
        ColumnCheck::Min { min, .. } => format!("{c} IS NULL OR {c} >= {}", num(*min)),
        ColumnCheck::Max { max, .. } => format!("{c} IS NULL OR {c} <= {}", num(*max)),
        ColumnCheck::Regex { regex, .. } => {
            format!("{c} IS NULL OR {c} RLIKE '{}'", sql_str(regex))
        }
        ColumnCheck::Format { format, .. } => {
            format!("{c} IS NULL OR {c} RLIKE '{}'", format_regex(*format))
        }
        ColumnCheck::Enum { values, .. } => {
            let list = values
                .iter()
                .map(enum_literal)
                .collect::<Vec<_>>()
                .join(", ");
            format!("{c} IS NULL OR {c} IN ({list})")
        }
        ColumnCheck::Length { length, .. } => match length {
            LengthSpec::Exact(n) => format!("{c} IS NULL OR length({c}) = {n}"),
            LengthSpec::Range(r) => {
                let mut parts = Vec::new();
                if let Some(min) = r.min {
                    parts.push(format!("length({c}) >= {min}"));
                }
                if let Some(max) = r.max {
                    parts.push(format!("length({c}) <= {max}"));
                }
                if parts.is_empty() {
                    return None;
                }
                format!("{c} IS NULL OR ({})", parts.join(" AND "))
            }
        },
        // Aggregate or engine-specific — not expressible as a row predicate.
        ColumnCheck::Unique { .. }
        | ColumnCheck::NullRatioMax { .. }
        | ColumnCheck::UniqueRatioMin { .. }
        | ColumnCheck::CustomExpr { .. } => return None,
    };
    Some(pred)
}

/// A backslash-free Spark-SQL regex for each built-in format (POSIX classes so
/// the pattern survives Spark string-literal escaping unchanged).
fn format_regex(format: KnownFormat) -> &'static str {
    match format {
        KnownFormat::Email => "^[^@ ]+@[^@ ]+[.][^@ ]+$",
        KnownFormat::Uuid => {
            "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$"
        }
        KnownFormat::IsoDate => "^[0-9]{4}-[0-9]{2}-[0-9]{2}$",
        KnownFormat::IsoDatetime => "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}",
        KnownFormat::Url => "^https?://",
        KnownFormat::CountryCodeIso2 => "^[A-Z]{2}$",
    }
}

/// Backtick-quotes a column identifier, escaping embedded backticks.
fn ident(col: &str) -> String {
    format!("`{}`", col.replace('`', "``"))
}

/// Escapes a string for a single-quoted Spark-SQL literal.
fn sql_str(s: &str) -> String {
    s.replace('\'', "''")
}

/// Renders a numeric literal.
fn num(n: Number) -> String {
    n.to_string()
}

/// Renders an enum value as a SQL literal (strings quoted, scalars bare).
fn enum_literal(v: &EnumValue) -> String {
    match v {
        EnumValue::String(s) => format!("'{}'", sql_str(s)),
        EnumValue::Bool(b) => b.to_string(),
        EnumValue::Int(i) => i.to_string(),
        EnumValue::Float(x) => x.to_string(),
    }
}

/// Lowercases and replaces non-identifier characters with `_`.
fn sanitize_name(raw: &str) -> String {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if s.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        s.insert(0, 'c');
    }
    s
}

fn render_sql(contract: &Contract, exps: &[Expectation], unsupported: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "-- Databricks Delta Live Tables expectations generated by PlexusPact\n\
         -- from contract `{}` (apiVersion v1). Paste the CONSTRAINT lines into\n\
         -- your DLT table definition (CREATE OR REFRESH … or a SQL @dlt.table).\n\
         -- Error-severity checks FAIL the update; warn-severity are recorded only.",
        contract.dataset
    );
    if exps.is_empty() {
        let _ = writeln!(out, "-- (no row-level expectations could be derived)");
    }
    for e in exps {
        let violation = if e.fail {
            " ON VIOLATION FAIL UPDATE"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "CONSTRAINT {} EXPECT ({}){},",
            e.name, e.predicate, violation
        );
    }
    append_unsupported(&mut out, unsupported, DltLang::Sql);
    out
}

fn render_python(contract: &Contract, exps: &[Expectation], unsupported: &[String]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Databricks Delta Live Tables expectations generated by PlexusPact\n\
         # from contract `{}` (apiVersion v1).\n\
         import dlt\n",
        contract.dataset
    );

    let fail: Vec<&Expectation> = exps.iter().filter(|e| e.fail).collect();
    let warn: Vec<&Expectation> = exps.iter().filter(|e| !e.fail).collect();

    let _ = writeln!(
        out,
        "# Error-severity: a violation fails the pipeline update."
    );
    write_py_dict(&mut out, "plexuspact_expect_or_fail", &fail);
    let _ = writeln!(
        out,
        "\n# Warn-severity: violations are recorded, update continues."
    );
    write_py_dict(&mut out, "plexuspact_expect", &warn);

    let dataset = sanitize_name(&contract.dataset);
    let _ = writeln!(
        out,
        "\n\n@dlt.table(name=\"{}\")\n\
         @dlt.expect_all_or_fail(plexuspact_expect_or_fail)\n\
         @dlt.expect_all(plexuspact_expect)\n\
         def {dataset}():\n\
         \x20   # TODO: return the source DataFrame for this dataset.\n\
         \x20   raise NotImplementedError(\"wire up your source read\")",
        contract.dataset
    );
    if !unsupported.is_empty() {
        let _ = writeln!(out);
        append_unsupported(&mut out, unsupported, DltLang::Python);
    }
    out
}

/// Emits a Python dict literal of `{name: predicate}` entries.
fn write_py_dict(out: &mut String, var: &str, exps: &[&Expectation]) {
    if exps.is_empty() {
        let _ = writeln!(out, "{var} = {{}}");
        return;
    }
    let _ = writeln!(out, "{var} = {{");
    for e in exps {
        let _ = writeln!(
            out,
            "    \"{}\": \"{}\",",
            e.name,
            e.predicate.replace('"', "\\\"")
        );
    }
    let _ = writeln!(out, "}}");
}

/// Appends the trailing comment block listing checks that couldn't be exported,
/// using the target language's line-comment prefix.
fn append_unsupported(out: &mut String, unsupported: &[String], lang: DltLang) {
    if unsupported.is_empty() {
        return;
    }
    let cp = comment_prefix(lang);
    let _ = writeln!(
        out,
        "\n{cp} Not expressible as row-level DLT expectations \
         (keep enforcing these with PlexusPact):"
    );
    for u in unsupported {
        let _ = writeln!(out, "{cp}   * {u}");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::parse::parse_str;

    fn contract() -> Contract {
        let yaml = "apiVersion: v1\n\
                    dataset: users\n\
                    columns:\n\
                    \x20 id: { type: int, required: true, checks: [{ min: 1 }] }\n\
                    \x20 email: { type: string, required: true, checks: [{ format: email }] }\n\
                    \x20 plan: { type: string, checks: [{ enum: [free, pro] }, unique] }\n\
                    dataset_checks:\n\
                    \x20 - row_count_min: 1\n";
        parse_str(yaml, "t.yaml").unwrap()
    }

    #[test]
    fn sql_maps_required_and_value_checks() {
        let sql = databricks_dlt(&contract(), DltLang::Sql);
        assert!(sql.contains(
            "CONSTRAINT id_not_null EXPECT (`id` IS NOT NULL) ON VIOLATION FAIL UPDATE,"
        ));
        assert!(sql.contains("`id` IS NULL OR `id` >= 1"));
        assert!(sql.contains("`email` RLIKE '^[^@ ]+@"));
        assert!(sql.contains("`plan` IN ('free', 'pro')"));
    }

    #[test]
    fn aggregate_checks_are_listed_not_dropped() {
        let sql = databricks_dlt(&contract(), DltLang::Sql);
        // `unique` (column) and `row_count_min` (dataset) can't be row predicates.
        assert!(sql.contains("column `plan` check `unique`"));
        assert!(sql.contains("dataset check `row_count_min`"));
        // …and no bogus expectation was emitted for them.
        assert!(!sql.contains("plan_unique EXPECT"));
    }

    #[test]
    fn python_groups_by_severity_and_is_importable_shape() {
        let py = databricks_dlt(&contract(), DltLang::Python);
        assert!(py.contains("import dlt"));
        assert!(py.contains("@dlt.expect_all_or_fail(plexuspact_expect_or_fail)"));
        assert!(py.contains("\"id_not_null\": \"`id` IS NOT NULL\""));
        assert!(py.contains("def users():"));
        // The trailing "unsupported" notes must use Python comments, not SQL `--`.
        assert!(py.contains("# Not expressible as row-level DLT"));
        assert!(!py.contains("-- Not expressible"));
    }

    #[test]
    fn warn_severity_check_does_not_fail_update() {
        let yaml = "apiVersion: v1\ndataset: d\ncolumns:\n  a: { type: int, checks: [{ max: 10, severity: warn }] }\n";
        let c = parse_str(yaml, "t").unwrap();
        let sql = databricks_dlt(&c, DltLang::Sql);
        assert!(sql.contains("EXPECT (`a` IS NULL OR `a` <= 10),"));
        assert!(!sql.contains("`a` <= 10) ON VIOLATION"));
    }
}
