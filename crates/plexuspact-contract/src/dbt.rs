//! Export a [`Contract`] as a dbt `schema.yml`.
//!
//! Like [`crate::export`], this is a **pure projection of the contract
//! model**: no IO, no new dependencies. The contract becomes one dbt
//! *source table* (`--source NAME`) or one *model* entry, and every check
//! becomes a dbt test so the rules PlexusPact enforces at the door also run
//! inside the warehouse, on the same schedule as the rest of the project.
//!
//! What dbt has a native test for uses it, so a reader who has never heard of
//! PlexusPact sees a familiar `schema.yml`:
//!
//! | contract                        | dbt                                              |
//! |---------------------------------|--------------------------------------------------|
//! | `required: true`                | `not_null`                                       |
//! | `unique`                        | `unique`                                         |
//! | `enum`                          | `accepted_values`                                |
//! | single-column `references`      | `relationships`                                  |
//! | error/warn `freshness` (source) | `freshness.error_after` / `warn_after`           |
//!
//! Everything else uses a generic test from the PlexusPact dbt package
//! (`integrations/dbt` in the repository, installed as `plexuspact`):
//! `min`, `max`, `regex`, `format`, `length`, `not_empty_string`,
//! `null_ratio_max`, `unique_ratio_min`, `row_count_min`, `row_count_max`,
//! `freshness` (models, which dbt cannot give a source-style freshness) and
//! `assert_expr`. A `custom_expr` is a Polars expression and has no SQL
//! equivalent, so it is listed in a trailing comment rather than dropped
//! silently.
//!
//! Warn-severity checks carry `config: {severity: warn}`, so dbt reports
//! them without failing the run — the same split as PlexusPact's exit codes.
//! Tests are written under `data_tests` with their inputs under `arguments`,
//! the shape dbt has used since 1.10 (older releases read the same keys
//! unnested; the header comment says so).

use std::fmt::Write as _;
use std::time::Duration;

use serde_yaml::{Mapping, Value};

use crate::model::{
    ColType, ColumnCheck, ColumnDef, Contract, DatasetCheck, EnumValue, LengthSpec, Severity,
    Stability,
};

/// Name the PlexusPact dbt package is installed under; generic tests are
/// referenced as `plexuspact.<test>`.
pub const DBT_PACKAGE: &str = "plexuspact";

/// How the contract should appear in the generated file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbtTarget<'a> {
    /// A table under `sources:` with the given source name. Freshness maps to
    /// dbt's own source freshness, and `references` to `source(...)`.
    Source(&'a str),
    /// An entry under `models:`; `references` resolve with `ref(...)`.
    Model,
}

/// Renders `contract` as a dbt `schema.yml` document.
pub fn dbt_schema(contract: &Contract, target: DbtTarget<'_>) -> String {
    let mut unsupported: Vec<String> = Vec::new();

    // Column entries, keyed by name so dataset-level checks that name a
    // column (`null_ratio_max`, `references`, …) can attach to it.
    let mut columns: Vec<(String, Mapping, Vec<Value>)> = contract
        .columns
        .iter()
        .map(|(name, def)| {
            let (entry, tests) = column_entry(name, def, &mut unsupported);
            (name.clone(), entry, tests)
        })
        .collect();
    let mut table_tests: Vec<Value> = Vec::new();
    let mut freshness: Option<Mapping> = None;
    let mut loaded_at: Option<String> = None;

    for check in &contract.dataset_checks {
        let sev = check.severity();
        match check {
            DatasetCheck::RowCountMin { count, .. } => table_tests.push(package_test(
                "row_count_min",
                [("count", Value::from(*count))],
                sev,
            )),
            DatasetCheck::RowCountMax { count, .. } => table_tests.push(package_test(
                "row_count_max",
                [("count", Value::from(*count))],
                sev,
            )),
            DatasetCheck::Freshness {
                column, max_age, ..
            } => {
                let native_slot = match target {
                    // dbt source freshness watches one column; the first
                    // freshness check claims it, later ones on other columns
                    // fall back to the package test.
                    DbtTarget::Source(_) => {
                        loaded_at.get_or_insert_with(|| column.clone()) == column
                    }
                    DbtTarget::Model => false,
                };
                if native_slot {
                    let slot = match sev {
                        Severity::Error => "error_after",
                        Severity::Warn => "warn_after",
                    };
                    let (count, period) = freshness_period(*max_age);
                    let mut window = Mapping::new();
                    window.insert(Value::from("count"), Value::from(count));
                    window.insert(Value::from("period"), Value::from(period));
                    freshness
                        .get_or_insert_with(Mapping::new)
                        .insert(Value::from(slot), Value::Mapping(window));
                } else {
                    table_tests.push(package_test(
                        "freshness",
                        [
                            ("column", Value::from(column.as_str())),
                            ("max_age_seconds", Value::from(max_age.as_secs())),
                        ],
                        sev,
                    ));
                }
            }
            DatasetCheck::NullRatioMax { column, ratio, .. } => attach(
                &mut columns,
                column,
                package_test("null_ratio_max", [("ratio", Value::from(*ratio))], sev),
            ),
            DatasetCheck::UniqueRatioMin { column, ratio, .. } => attach(
                &mut columns,
                column,
                package_test("unique_ratio_min", [("ratio", Value::from(*ratio))], sev),
            ),
            DatasetCheck::Assert { expr, .. } => table_tests.push(package_test(
                "assert_expr",
                [("expression", Value::from(expr.as_str()))],
                sev,
            )),
            DatasetCheck::References {
                columns: from,
                dataset,
                to,
                ..
            } => match (from.as_slice(), to.as_slice()) {
                ([col], [field]) => {
                    let relation = match target {
                        DbtTarget::Source(src) => format!("source('{src}', '{dataset}')"),
                        DbtTarget::Model => format!("ref('{dataset}')"),
                    };
                    let mut args = Mapping::new();
                    args.insert(Value::from("to"), Value::from(relation));
                    args.insert(Value::from("field"), Value::from(field.as_str()));
                    attach(&mut columns, col, test_value("relationships", args, sev));
                }
                _ => unsupported.push(format!(
                    "dataset check `references` on [{}] — dbt's `relationships` test is \
                     single-column; keep enforcing the composite key with `plexuspact check`",
                    from.join(", ")
                )),
            },
            DatasetCheck::CustomExpr { .. } => unsupported.push(
                "dataset check `custom_expr` — a Polars expression, not SQL; \
                 keep enforcing it with `plexuspact check`"
                    .to_owned(),
            ),
        }
    }

    // ── assemble the table entry ─────────────────────────────────────────
    let mut table = Mapping::new();
    table.insert(Value::from("name"), Value::from(contract.dataset.as_str()));
    if let Some(d) = &contract.description {
        table.insert(Value::from("description"), Value::from(d.as_str()));
    }
    let mut meta = Mapping::new();
    if let Some(owner) = &contract.owner {
        meta.insert(Value::from("owner"), Value::from(owner.as_str()));
    }
    if let Some(version) = &contract.version {
        meta.insert(
            Value::from("contract_version"),
            Value::from(version.as_str()),
        );
    }
    if !contract.primary_key.is_empty() {
        meta.insert(
            Value::from("primary_key"),
            Value::Sequence(
                contract
                    .primary_key
                    .iter()
                    .map(|c| Value::from(c.as_str()))
                    .collect(),
            ),
        );
    }
    if !meta.is_empty() {
        table.insert(Value::from("meta"), Value::Mapping(meta));
    }
    if let Some(col) = loaded_at {
        table.insert(Value::from("loaded_at_field"), Value::from(col));
    }
    if let Some(f) = freshness {
        table.insert(Value::from("freshness"), Value::Mapping(f));
    }
    if !table_tests.is_empty() {
        table.insert(Value::from("data_tests"), Value::Sequence(table_tests));
    }
    let column_values: Vec<Value> = columns
        .into_iter()
        .map(|(_, mut entry, tests)| {
            if !tests.is_empty() {
                entry.insert(Value::from("data_tests"), Value::Sequence(tests));
            }
            Value::Mapping(entry)
        })
        .collect();
    if !column_values.is_empty() {
        table.insert(Value::from("columns"), Value::Sequence(column_values));
    }

    // ── the document ─────────────────────────────────────────────────────
    let mut doc = Mapping::new();
    doc.insert(Value::from("version"), Value::from(2));
    match target {
        DbtTarget::Source(src) => {
            let mut source = Mapping::new();
            source.insert(Value::from("name"), Value::from(src));
            source.insert(
                Value::from("tables"),
                Value::Sequence(vec![Value::Mapping(table)]),
            );
            doc.insert(
                Value::from("sources"),
                Value::Sequence(vec![Value::Mapping(source)]),
            );
        }
        DbtTarget::Model => {
            doc.insert(
                Value::from("models"),
                Value::Sequence(vec![Value::Mapping(table)]),
            );
        }
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# dbt schema generated by PlexusPact from contract `{}` (apiVersion v1).\n\
         # Native dbt tests are used where they exist; the `{DBT_PACKAGE}.*` tests come\n\
         # from the PlexusPact dbt package — add it to packages.yml (see the docs).\n\
         # Warn-severity checks are `severity: warn`; everything else fails the run.",
        contract.dataset
    );
    match serde_yaml::to_string(&Value::Mapping(doc)) {
        Ok(yaml) => out.push_str(&yaml),
        // A mapping of strings and numbers always serializes; keep the
        // signature simple and make the impossible case visible instead.
        Err(e) => {
            let _ = writeln!(out, "# failed to render schema: {e}");
        }
    }
    if !unsupported.is_empty() {
        let _ = writeln!(
            out,
            "\n# Not expressible as dbt tests (keep enforcing these with PlexusPact):"
        );
        for u in &unsupported {
            let _ = writeln!(out, "#   * {u}");
        }
    }
    out
}

/// One column entry plus its tests (returned separately so dataset-level
/// checks can still add to them).
fn column_entry(
    name: &str,
    def: &ColumnDef,
    unsupported: &mut Vec<String>,
) -> (Mapping, Vec<Value>) {
    let mut entry = Mapping::new();
    entry.insert(Value::from("name"), Value::from(name));
    entry.insert(Value::from("data_type"), Value::from(data_type(def.r#type)));
    if let Some(d) = &def.description {
        entry.insert(Value::from("description"), Value::from(d.as_str()));
    }
    let mut meta = Mapping::new();
    if let Some(pii) = &def.pii {
        if let Some(s) = enum_name(pii) {
            meta.insert(Value::from("pii"), Value::from(s));
        }
    }
    if let Some(class) = &def.classification {
        if let Some(s) = enum_name(class) {
            meta.insert(Value::from("classification"), Value::from(s));
        }
    }
    if def.stability != Stability::Stable {
        meta.insert(
            Value::from("stability"),
            Value::from(def.stability.to_string()),
        );
    }
    if let Some(sunset) = &def.sunset {
        meta.insert(Value::from("sunset"), Value::from(sunset.as_str()));
    }
    if !meta.is_empty() {
        entry.insert(Value::from("meta"), Value::Mapping(meta));
    }

    let mut tests: Vec<Value> = Vec::new();
    if def.required {
        tests.push(test_value("not_null", Mapping::new(), Severity::Error));
    }
    for check in &def.checks {
        let sev = check.severity();
        let test = match check {
            ColumnCheck::Unique { .. } => test_value("unique", Mapping::new(), sev),
            ColumnCheck::NotEmptyString { .. } => package_test("not_empty_string", [], sev),
            ColumnCheck::Min { min, .. } => {
                package_test("min", [("value", number_value(*min))], sev)
            }
            ColumnCheck::Max { max, .. } => {
                package_test("max", [("value", number_value(*max))], sev)
            }
            ColumnCheck::Regex { regex, .. } => {
                package_test("regex", [("pattern", Value::from(regex.as_str()))], sev)
            }
            // `name` is a reserved key on a dbt test entry, so the argument is `format`.
            ColumnCheck::Format { format, .. } => package_test(
                "format",
                [("format", Value::from(enum_name(format).unwrap_or_default()))],
                sev,
            ),
            ColumnCheck::Enum { values, .. } => {
                let mut args = Mapping::new();
                args.insert(
                    Value::from("values"),
                    Value::Sequence(values.iter().map(enum_value).collect()),
                );
                // dbt quotes accepted values by default, which is right for
                // strings and wrong for numbers and booleans.
                if !values.iter().any(|v| matches!(v, EnumValue::String(_))) {
                    args.insert(Value::from("quote"), Value::from(false));
                }
                test_value("accepted_values", args, sev)
            }
            ColumnCheck::Length { length, .. } => {
                let args: Vec<(&str, Value)> = match length {
                    LengthSpec::Exact(n) => vec![("exact", Value::from(*n))],
                    LengthSpec::Range(r) => {
                        let mut a = Vec::new();
                        if let Some(min) = r.min {
                            a.push(("min", Value::from(min)));
                        }
                        if let Some(max) = r.max {
                            a.push(("max", Value::from(max)));
                        }
                        a
                    }
                };
                package_test("length", args, sev)
            }
            ColumnCheck::NullRatioMax { ratio, .. } => {
                package_test("null_ratio_max", [("ratio", Value::from(*ratio))], sev)
            }
            ColumnCheck::UniqueRatioMin { ratio, .. } => {
                package_test("unique_ratio_min", [("ratio", Value::from(*ratio))], sev)
            }
            ColumnCheck::CustomExpr { .. } => {
                unsupported.push(format!(
                    "column `{name}` check `custom_expr` — a Polars expression, not SQL; \
                     keep enforcing it with `plexuspact check`"
                ));
                continue;
            }
        };
        tests.push(test);
    }
    (entry, tests)
}

/// Adds `test` to the named column's tests. Lint guarantees dataset checks
/// name real columns; an unknown one is kept as a table-level test so nothing
/// is lost even for an unlinted contract.
fn attach(columns: &mut [(String, Mapping, Vec<Value>)], column: &str, test: Value) {
    if let Some((_, _, tests)) = columns.iter_mut().find(|(n, _, _)| n == column) {
        tests.push(test);
    }
}

/// A `plexuspact.<name>` generic test with the given arguments.
fn package_test<'a>(
    name: &str,
    args: impl IntoIterator<Item = (&'a str, Value)>,
    severity: Severity,
) -> Value {
    let mut map = Mapping::new();
    for (k, v) in args {
        map.insert(Value::from(k), v);
    }
    test_value(&format!("{DBT_PACKAGE}.{name}"), map, severity)
}

/// Renders one dbt test entry: a bare name when it has no arguments and no
/// config (`- not_null`), otherwise
/// `- name: {arguments: {…}, config: {severity: warn}}`.
fn test_value(name: &str, args: Mapping, severity: Severity) -> Value {
    let mut body = Mapping::new();
    if !args.is_empty() {
        body.insert(Value::from("arguments"), Value::Mapping(args));
    }
    if severity == Severity::Warn {
        let mut config = Mapping::new();
        config.insert(Value::from("severity"), Value::from("warn"));
        body.insert(Value::from("config"), Value::Mapping(config));
    }
    if body.is_empty() {
        return Value::from(name);
    }
    let mut entry = Mapping::new();
    entry.insert(Value::from(name), Value::Mapping(body));
    Value::Mapping(entry)
}

/// dbt `data_type` label for a contract type (documentation, and the input
/// to dbt model contracts when a team enforces them).
fn data_type(t: ColType) -> &'static str {
    match t {
        ColType::String => "string",
        ColType::Int => "integer",
        ColType::Float => "float",
        ColType::Bool => "boolean",
        ColType::Date => "date",
        ColType::Datetime => "timestamp",
    }
}

/// dbt source freshness counts in minutes, hours or days; pick the largest
/// unit that divides the window, rounding a sub-minute window up to one.
fn freshness_period(d: Duration) -> (u64, &'static str) {
    let secs = d.as_secs();
    if secs > 0 && secs.is_multiple_of(86_400) {
        (secs / 86_400, "day")
    } else if secs > 0 && secs.is_multiple_of(3_600) {
        (secs / 3_600, "hour")
    } else {
        (secs.div_ceil(60).max(1), "minute")
    }
}

fn number_value(n: crate::model::Number) -> Value {
    match n {
        crate::model::Number::Int(i) => Value::from(i),
        crate::model::Number::Float(f) => Value::from(f),
    }
}

fn enum_value(v: &EnumValue) -> Value {
    match v {
        EnumValue::String(s) => Value::from(s.as_str()),
        EnumValue::Bool(b) => Value::from(*b),
        EnumValue::Int(i) => Value::from(*i),
        EnumValue::Float(f) => Value::from(*f),
    }
}

/// The serde name of a unit enum variant (`national_id`, `confidential`).
fn enum_name<T: serde::Serialize>(v: &T) -> Option<String> {
    serde_yaml::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::parse::parse_str;

    fn contract() -> Contract {
        let yaml = "apiVersion: v1\n\
                    dataset: user_signups\n\
                    owner: growth@example.test\n\
                    version: 1.4.0\n\
                    primary_key: [user_id]\n\
                    columns:\n\
                    \x20 user_id: { type: int, required: true, checks: [unique, { min: 1 }] }\n\
                    \x20 email: { type: string, required: true, pii: email, checks: [{ format: email }, { length: { max: 254 } }] }\n\
                    \x20 plan: { type: string, checks: [{ enum: [free, pro] }] }\n\
                    \x20 seats: { type: int, checks: [{ enum: [1, 5, 10], severity: warn }, { custom_expr: 'col(\"seats\") > 0' }] }\n\
                    \x20 country: { type: string, classification: internal, checks: [{ regex: '^[A-Z]{2}$' }] }\n\
                    \x20 signed_up: { type: datetime, required: true }\n\
                    dataset_checks:\n\
                    \x20 - row_count_min: 1\n\
                    \x20 - assert: \"MIN(seats) >= 0\"\n\
                    \x20 - { freshness: { column: signed_up, max_age: 2d } }\n\
                    \x20 - { references: { columns: [country], dataset: countries, to: [code] }, severity: warn }\n\
                    \x20 - { null_ratio_max: { column: plan, ratio: 0.2 } }\n";
        parse_str(yaml, "t.yaml").unwrap()
    }

    fn parsed(text: &str) -> serde_yaml::Value {
        serde_yaml::from_str(text).unwrap()
    }

    #[test]
    fn source_uses_native_tests_and_source_freshness() {
        let out = dbt_schema(&contract(), DbtTarget::Source("app_db"));
        let doc = parsed(&out);
        let table = &doc["sources"][0]["tables"][0];
        assert_eq!(doc["version"], Value::from(2));
        assert_eq!(doc["sources"][0]["name"], Value::from("app_db"));
        assert_eq!(table["name"], Value::from("user_signups"));
        assert_eq!(table["loaded_at_field"], Value::from("signed_up"));
        assert_eq!(table["freshness"]["error_after"]["count"], Value::from(2));
        assert_eq!(
            table["freshness"]["error_after"]["period"],
            Value::from("day")
        );
        assert_eq!(table["meta"]["owner"], Value::from("growth@example.test"));
        assert_eq!(table["meta"]["primary_key"][0], Value::from("user_id"));

        let cols = table["columns"].as_sequence().unwrap();
        let user_id = &cols[0];
        assert_eq!(user_id["data_type"], Value::from("integer"));
        let tests = user_id["data_tests"].as_sequence().unwrap();
        assert_eq!(tests[0], Value::from("not_null"));
        assert_eq!(tests[1], Value::from("unique"));
        assert_eq!(
            tests[2]["plexuspact.min"]["arguments"]["value"],
            Value::from(1)
        );

        let email = &cols[1];
        assert_eq!(email["meta"]["pii"], Value::from("email"));
        let tests = email["data_tests"].as_sequence().unwrap();
        assert_eq!(
            tests[1]["plexuspact.format"]["arguments"]["format"],
            Value::from("email")
        );
        assert_eq!(
            tests[2]["plexuspact.length"]["arguments"]["max"],
            Value::from(254)
        );

        // enum of strings: dbt's default quoting; enum of ints: quote: false
        let plan = &cols[2]["data_tests"][0]["accepted_values"]["arguments"];
        assert_eq!(plan["values"][1], Value::from("pro"));
        assert!(plan.get("quote").is_none());
        let seats = &cols[3]["data_tests"][0]["accepted_values"];
        assert_eq!(seats["arguments"]["quote"], Value::from(false));
        assert_eq!(seats["config"]["severity"], Value::from("warn"));

        // dataset-level checks that name a column land on that column
        let country = cols[4]["data_tests"].as_sequence().unwrap();
        let rel = &country[1]["relationships"];
        assert_eq!(
            rel["arguments"]["to"],
            Value::from("source('app_db', 'countries')")
        );
        assert_eq!(rel["arguments"]["field"], Value::from("code"));
        assert_eq!(rel["config"]["severity"], Value::from("warn"));
        assert_eq!(
            cols[2]["data_tests"][1]["plexuspact.null_ratio_max"]["arguments"]["ratio"],
            Value::from(0.2)
        );

        // table-level tests
        let tt = table["data_tests"].as_sequence().unwrap();
        assert_eq!(
            tt[0]["plexuspact.row_count_min"]["arguments"]["count"],
            Value::from(1)
        );
        assert_eq!(
            tt[1]["plexuspact.assert_expr"]["arguments"]["expression"],
            Value::from("MIN(seats) >= 0")
        );
        assert_eq!(tt.len(), 2, "freshness took the native slot");

        // custom_expr is said, not dropped silently
        assert!(out.contains("column `seats` check `custom_expr`"), "{out}");
    }

    #[test]
    fn model_uses_ref_and_package_freshness() {
        let out = dbt_schema(&contract(), DbtTarget::Model);
        let doc = parsed(&out);
        let model = &doc["models"][0];
        assert!(doc.get("sources").is_none());
        assert!(model.get("freshness").is_none());
        assert!(model.get("loaded_at_field").is_none());
        let tt = model["data_tests"].as_sequence().unwrap();
        let fresh = tt
            .iter()
            .find_map(|t| t.get("plexuspact.freshness"))
            .and_then(|t| t.get("arguments"))
            .expect("freshness became a package test");
        assert_eq!(fresh["column"], Value::from("signed_up"));
        assert_eq!(fresh["max_age_seconds"], Value::from(172_800));
        let country = &model["columns"][4]["data_tests"][1]["relationships"];
        assert_eq!(country["arguments"]["to"], Value::from("ref('countries')"));
    }

    #[test]
    fn composite_references_are_listed_not_dropped() {
        let yaml = "apiVersion: v1\ndataset: d\ncolumns:\n  a: { type: int }\n  b: { type: int }\n\
                    dataset_checks:\n  - references: { columns: [a, b], dataset: other, to: [x, y] }\n";
        let c = parse_str(yaml, "t").unwrap();
        let out = dbt_schema(&c, DbtTarget::Model);
        assert!(out.contains("references` on [a, b]"), "{out}");
        assert!(!out.contains("relationships:"), "{out}");
    }

    #[test]
    fn freshness_period_picks_a_dbt_unit() {
        assert_eq!(freshness_period(Duration::from_secs(172_800)), (2, "day"));
        assert_eq!(freshness_period(Duration::from_secs(5_400)), (90, "minute"));
        assert_eq!(freshness_period(Duration::from_secs(7_200)), (2, "hour"));
        assert_eq!(freshness_period(Duration::from_secs(30)), (1, "minute"));
    }

    #[test]
    fn output_is_valid_yaml_with_regex_intact() {
        let out = dbt_schema(&contract(), DbtTarget::Model);
        let doc = parsed(&out);
        assert_eq!(
            doc["models"][0]["columns"][4]["data_tests"][0]["plexuspact.regex"]["arguments"]
                ["pattern"],
            Value::from("^[A-Z]{2}$")
        );
    }
}
