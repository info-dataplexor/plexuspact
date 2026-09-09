//! Warehouse pushdown: the contract's checks compiled into SQL the warehouse
//! runs itself, and the warehouse's answers decoded into the same
//! [`EngineOutput`] the streaming engine produces.
//!
//! The streaming engine pulls rows to where it runs. A warehouse table is
//! already next to a query engine that can count, group and compare far faster
//! than the rows could be moved, so for a warehouse source the contract goes to
//! the data instead: [`compile`] turns it into **one aggregate scan** — row
//! count, per-column null and distinct counts, bounds, means, lengths, and one
//! `SUM(CASE WHEN … THEN 1 ELSE 0 END)` per row-level check — plus one
//! statement per user expression (`custom_expr`, `assert`), so a broken
//! expression only fails its own check. A second, optional round fetches
//! failure samples for the checks that failed, the distinct keys the
//! `primary_key` and `references` checks need, and the value set of
//! low-cardinality columns for the profile. [`Probe::decode`] then produces
//! check outcomes with the same ids, kinds, parameters, observed statistics and
//! messages as `plan.rs` / `checks.rs`, so a run evaluated in the warehouse and
//! a run evaluated by pull are compared like for like.
//!
//! What is deliberately different, and said so in the result:
//!
//! * A warehouse has no row order, so failure samples carry row `0`.
//! * `unique` and the profile's `distinct` are exact counts, never estimates.
//! * `format: iso_date` / `iso_datetime` prove the shape of the value with a
//!   regular expression, not the calendar — a `2026-02-30` passes here and
//!   fails by pull.
//! * A column declared as a number but stored as text is parsed with the
//!   dialect's safe cast (`TRY_CAST`, `SAFE_CAST`, or a shape guard on
//!   Postgres, which has none); a column whose stored type cannot be compared
//!   the way a check asks reports "could not run" rather than passing quietly.
//!
//! Every value the probe selects is cast to text, so a driver needs one way to
//! read a cell whatever the warehouse's wire types are.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use plexuspact_contract::{
    ColType, ColumnCheck, Contract, DatasetCheck, KnownFormat, LengthSpec, Number, Severity,
};
use serde_json::{json, Value};

use crate::formats::{format_name, ISO_3166_1_ALPHA2};
use crate::keys::{hash_key, KeySet, ReferenceSets};
use crate::plan::humanize_secs;
use crate::{CheckOutcome, ColumnStats, EngineOutput, ObservedColumn};

/// The SQL the probe is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// PostgreSQL and Redshift.
    Postgres,
    /// Snowflake.
    Snowflake,
    /// Databricks SQL (Spark).
    Databricks,
    /// BigQuery (GoogleSQL).
    BigQuery,
}

impl Dialect {
    /// The dialect's name as a contract or a configuration spells it.
    pub fn name(self) -> &'static str {
        match self {
            Dialect::Postgres => "postgres",
            Dialect::Snowflake => "snowflake",
            Dialect::Databricks => "databricks",
            Dialect::BigQuery => "bigquery",
        }
    }

    /// Parses a dialect name; `warehouse` and `redshift` mean Postgres SQL.
    pub fn parse(name: &str) -> Option<Dialect> {
        match name.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" | "redshift" | "warehouse" => Some(Dialect::Postgres),
            "snowflake" => Some(Dialect::Snowflake),
            "databricks" | "spark" => Some(Dialect::Databricks),
            "bigquery" => Some(Dialect::BigQuery),
            _ => None,
        }
    }

    /// A quoted identifier.
    pub fn ident(self, name: &str) -> String {
        match self {
            Dialect::Postgres | Dialect::Snowflake => {
                format!("\"{}\"", name.replace('"', "\"\""))
            }
            Dialect::Databricks => format!("`{}`", name.replace('`', "``")),
            Dialect::BigQuery => format!("`{}`", name.replace('\\', "\\\\").replace('`', "\\`")),
        }
    }

    /// A single-quoted string literal, escaped the way this dialect reads it.
    pub fn literal(self, text: &str) -> String {
        match self {
            Dialect::Postgres => format!("'{}'", text.replace('\'', "''")),
            Dialect::Snowflake | Dialect::Databricks | Dialect::BigQuery => {
                format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        }
    }

    fn text_type(self) -> &'static str {
        match self {
            Dialect::Postgres => "TEXT",
            Dialect::Snowflake => "VARCHAR",
            Dialect::Databricks | Dialect::BigQuery => "STRING",
        }
    }

    fn double_type(self) -> &'static str {
        match self {
            Dialect::Postgres => "DOUBLE PRECISION",
            Dialect::Snowflake | Dialect::Databricks => "DOUBLE",
            Dialect::BigQuery => "FLOAT64",
        }
    }

    fn to_text(self, expr: &str) -> String {
        format!("CAST({expr} AS {})", self.text_type())
    }

    /// `true` when `expr` matches `pattern` anywhere — search semantics, as the
    /// engine's `regex` check and Rust's `is_match` have, so a contract pattern
    /// anchors itself with `^`/`$` when it means to.
    fn regex_match(self, expr: &str, pattern: &str) -> String {
        match self {
            Dialect::Postgres => format!("({expr} ~ {})", self.literal(pattern)),
            Dialect::Snowflake => format!("(REGEXP_INSTR({expr}, {}) > 0)", self.literal(pattern)),
            Dialect::Databricks => format!("({expr} RLIKE {})", self.literal(pattern)),
            Dialect::BigQuery => {
                format!(
                    "REGEXP_CONTAINS({expr}, {})",
                    bigquery_regex_literal(pattern)
                )
            }
        }
    }

    /// The value as a double, or null when it does not read as a number.
    fn try_double(self, expr: &str) -> String {
        match self {
            Dialect::Postgres => format!(
                "(CASE WHEN {} THEN CAST({expr} AS DOUBLE PRECISION) END)",
                self.regex_match(expr, FLOAT_SHAPE)
            ),
            Dialect::Snowflake | Dialect::Databricks => format!("TRY_CAST({expr} AS DOUBLE)"),
            Dialect::BigQuery => format!("SAFE_CAST({expr} AS FLOAT64)"),
        }
    }

    /// The text value as a timestamp, or null when it does not read as one.
    /// Postgres has no safe cast, so a text column cannot be dated there.
    fn try_timestamp(self, expr: &str) -> Option<String> {
        match self {
            Dialect::Postgres => None,
            Dialect::Snowflake => Some(format!("TRY_TO_TIMESTAMP({expr})")),
            Dialect::Databricks => Some(format!("TRY_CAST({expr} AS TIMESTAMP)")),
            Dialect::BigQuery => Some(format!("SAFE_CAST({expr} AS TIMESTAMP)")),
        }
    }

    /// Seconds since the Unix epoch of a date, datetime or timestamp value.
    fn epoch_seconds(self, expr: &str) -> String {
        match self {
            Dialect::Postgres => format!("EXTRACT(EPOCH FROM CAST({expr} AS TIMESTAMPTZ))"),
            Dialect::Snowflake => format!("DATE_PART(EPOCH_SECOND, {expr})"),
            Dialect::Databricks => format!("UNIX_TIMESTAMP(CAST({expr} AS TIMESTAMP))"),
            Dialect::BigQuery => format!("UNIX_SECONDS(CAST({expr} AS TIMESTAMP))"),
        }
    }

    /// Rewrites the portable spelling a contract expression uses
    /// (`CAST(x AS DOUBLE)`, what the rule builder writes) into this dialect's.
    pub fn rewrite_expr(self, expr: &str) -> String {
        replace_ci(expr, "AS DOUBLE)", &format!("AS {})", self.double_type()))
    }
}

/// Shape of a value that reads as a number: `12`, `-3.5`, `.5`, `1e9`.
const FLOAT_SHAPE: &str = r"^[-+]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$";
const INT_SHAPE: &str = r"^[-+]?[0-9]+$";
const DATE_SHAPE: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$";
const DATETIME_SHAPE: &str = r"^[0-9]{4}-[0-9]{2}-[0-9]{2}[T ][0-9]{2}:[0-9]{2}(:[0-9]{2}(\.[0-9]+)?)?(Z|[+-][0-9]{2}:?[0-9]{2})?$";
const EMAIL_SHAPE: &str = r"^[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}$";
const UUID_SHAPE: &str =
    r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";
const URL_SHAPE: &str = r"^[A-Za-z][A-Za-z0-9+.\-]*://[^/?#[:space:]]+";
const BOOL_WORDS: &[&str] = &["true", "t", "1", "yes", "y", "false", "f", "0", "no", "n"];

/// Separator between the parts of a composite key inside the warehouse — the
/// ASCII unit separator, which no key part contains.
const KEY_SEPARATOR: char = '\u{1f}';

/// BigQuery regular expressions are raw strings when they can be, so
/// backslashes survive untouched.
fn bigquery_regex_literal(pattern: &str) -> String {
    if !pattern.contains('\'') {
        format!("r'{pattern}'")
    } else if !pattern.contains('"') {
        format!("r\"{pattern}\"")
    } else {
        Dialect::BigQuery.literal(pattern)
    }
}

/// Case-insensitive replacement of every `needle` in `hay`.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let lower = hay.to_ascii_lowercase();
    let needle_lower = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(hay.len());
    let mut from = 0;
    while let Some(at) = lower[from..].find(&needle_lower) {
        let start = from + at;
        out.push_str(&hay[from..start]);
        out.push_str(with);
        from = start + needle.len();
    }
    out.push_str(&hay[from..]);
    out
}

/// The broad type of a stored column, from the name the warehouse gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Whole numbers.
    Int,
    /// Decimals and floating point.
    Float,
    /// Booleans.
    Bool,
    /// Calendar dates.
    Date,
    /// Timestamps, with or without a zone.
    Datetime,
    /// Character strings.
    Text,
    /// Anything else — JSON, arrays, binary, intervals, geography.
    Other,
}

/// Classifies a warehouse type name (`BIGINT`, `NUMBER(38,0)`, `character
/// varying(120)`, `TIMESTAMP_NTZ`) into a [`Family`].
pub fn family_of(dialect: Dialect, dtype: &str) -> Family {
    let full = dtype.trim().to_ascii_lowercase();
    let base = full.split('(').next().unwrap_or("").trim().to_owned();
    let scale_zero = || {
        full.split_once('(')
            .and_then(|(_, rest)| rest.split_once(','))
            .map(|(_, scale)| scale.trim_end_matches(')').trim() == "0")
    };
    match base.as_str() {
        "number" | "numeric" | "decimal" | "dec" | "bignumeric" | "bigdecimal" => {
            match scale_zero() {
                Some(true) => Family::Int,
                Some(false) => Family::Float,
                // Snowflake's bare NUMBER is NUMBER(38,0); everyone else's bare
                // numeric is arbitrary precision.
                None if dialect == Dialect::Snowflake && base == "number" => Family::Int,
                None if full.contains('(') && !full.contains(',') => Family::Int,
                None => Family::Float,
            }
        }
        "int" | "integer" | "bigint" | "smallint" | "tinyint" | "byteint" | "int2" | "int4"
        | "int8" | "int64" | "int32" | "serial" | "bigserial" | "smallserial" | "long"
        | "short" => Family::Int,
        "float" | "float4" | "float8" | "float64" | "double" | "double precision" | "real"
        | "binary_float" | "binary_double" => Family::Float,
        "bool" | "boolean" => Family::Bool,
        "date" => Family::Date,
        "text" | "varchar" | "char" | "character" | "character varying" | "string" | "bpchar"
        | "name" | "citext" | "nvarchar" | "nchar" | "varchar2" | "nvarchar2" | "uuid" => {
            Family::Text
        }
        _ if base.starts_with("timestamp") || base.starts_with("datetime") => Family::Datetime,
        _ => Family::Other,
    }
}

/// One column as the warehouse describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceColumn {
    /// The column's name, exactly as the warehouse spells it.
    pub name: String,
    /// The warehouse's own type name for it.
    pub dtype: String,
}

/// The knobs of a pushdown run.
#[derive(Debug, Clone)]
pub struct ProbeOptions {
    /// The clock `freshness` is judged against.
    pub now: DateTime<Utc>,
    /// Failure samples fetched per failing check.
    pub sample_failures: usize,
    /// Key sets of other datasets, for `references`.
    pub references: ReferenceSets,
    /// The most distinct keys the probe will fetch for `primary_key` (to keep
    /// as this dataset's key set) or `references` (to look up). Above it, the
    /// key set is not kept and a `references` check reports that it could not
    /// run, rather than moving a fact table over the wire.
    pub max_keys: u64,
    /// Whether to profile every column alongside the checks.
    pub profile: bool,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        ProbeOptions {
            now: DateTime::<Utc>::UNIX_EPOCH,
            sample_failures: 5,
            references: ReferenceSets::none(),
            max_keys: 500_000,
            profile: true,
        }
    }
}

/// One SQL statement for the driver to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// What the statement is for — `aggregate`, `expr:<check id>`,
    /// `samples:<check id>`, `keys:<check id>`, `values:<column>`.
    pub label: String,
    /// The SQL.
    pub sql: String,
}

/// What the warehouse answered one statement with: rows of text cells, or the
/// error it raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The result rows, every cell rendered as text (`None` for SQL null).
    Rows(Vec<Vec<Option<String>>>),
    /// The warehouse refused the statement; the text is its error.
    Error(String),
}

impl Reply {
    fn rows(&self) -> Option<&[Vec<Option<String>>]> {
        match self {
            Reply::Rows(rows) => Some(rows),
            Reply::Error(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Meta {
    id: String,
    column: Option<String>,
    kind: &'static str,
    params: Value,
    severity: Severity,
}

/// Which text the failure message of a counted check uses.
#[derive(Debug, Clone, Copy)]
enum Wording {
    Required,
    Type,
    Regex,
    Format,
    Enum,
    Length,
    NotEmpty,
}

#[derive(Debug, Clone)]
enum How {
    /// Known before any SQL runs: schema checks, structural mismatches, and
    /// checks the warehouse cannot evaluate.
    Fixed {
        failed: bool,
        message: Option<String>,
        observed: BTreeMap<String, Value>,
    },
    /// Rows evaluated and rows failed are two aggregate slots.
    Counted {
        evaluated: usize,
        failed: Option<usize>,
        extreme: Option<(usize, &'static str)>,
        wording: Wording,
        bound_message: Option<String>,
        sample: Option<String>,
    },
    Unique {
        nonnull: usize,
        distinct: usize,
        sample: String,
    },
    NullRatio {
        nonnull: usize,
        max_ratio: f64,
    },
    UniqueRatio {
        nonnull: usize,
        distinct: usize,
        min_ratio: f64,
    },
    RowCount {
        is_min: bool,
        bound: u64,
    },
    Freshness {
        epoch: usize,
        max_secs: i64,
    },
    PrimaryKey {
        nonnull_rows: usize,
        distinct_keys: usize,
        columns: Vec<String>,
        sample: String,
        keys: String,
    },
    /// One statement of its own; `row_level` for `custom_expr`, whole-dataset
    /// for `assert`.
    Expression {
        statement: usize,
        row_level: bool,
        expr: String,
    },
    References {
        nonnull_rows: usize,
        dataset: String,
        groups: String,
    },
}

#[derive(Debug, Clone)]
struct Planned {
    meta: Meta,
    how: How,
}

#[derive(Debug, Clone)]
struct ProfileSlots {
    name: String,
    family: Family,
    nonnull: usize,
    distinct: usize,
    min: Option<usize>,
    max: Option<usize>,
    mean: Option<usize>,
    std: Option<usize>,
    min_length: Option<usize>,
    max_length: Option<usize>,
    values: String,
}

#[derive(Debug, Clone)]
enum Purpose {
    Samples(usize),
    PrimaryKeys(usize),
    ReferenceGroups(usize),
    Values(usize),
}

/// A compiled contract: the statements to run and how to read their answers.
#[derive(Debug, Clone)]
pub struct Probe {
    dialect: Dialect,
    table: String,
    now: DateTime<Utc>,
    sample_failures: usize,
    max_keys: u64,
    references: ReferenceSets,
    columns: Vec<SourceColumn>,
    items: Vec<String>,
    checks: Vec<Planned>,
    profile: Vec<ProfileSlots>,
    expressions: Vec<Statement>,
    follow_ups: Vec<(Purpose, Statement)>,
}

/// Compiles `contract` into a [`Probe`] over `table` — the already quoted,
/// fully qualified table (`"public"."orders"`, `` `proj.dataset.orders` ``) —
/// given the columns the warehouse says the table has.
pub fn compile(
    contract: &Contract,
    table: &str,
    dialect: Dialect,
    columns: &[SourceColumn],
    options: &ProbeOptions,
) -> Probe {
    let mut probe = Probe {
        dialect,
        table: table.to_owned(),
        now: options.now,
        sample_failures: options.sample_failures,
        max_keys: options.max_keys,
        references: options.references.clone(),
        columns: columns.to_vec(),
        items: Vec::new(),
        checks: Vec::new(),
        profile: Vec::new(),
        expressions: Vec::new(),
        follow_ups: Vec::new(),
    };
    // Slot 0 is always the row count.
    probe.slot("COUNT(*)");

    let families: BTreeMap<&str, Family> = columns
        .iter()
        .map(|c| (c.name.as_str(), family_of(dialect, &c.dtype)))
        .collect();
    let present: BTreeSet<&str> = families.keys().copied().collect();

    probe.plan_columns_check(contract, &present);
    for (name, col) in &contract.columns {
        let Some(&family) = families.get(name.as_str()) else {
            probe.checks.push(Planned {
                meta: Meta {
                    id: format!("{name}.present"),
                    column: Some(name.clone()),
                    kind: "column_present",
                    params: json!({}),
                    severity: Severity::Error,
                },
                how: How::Fixed {
                    failed: true,
                    message: Some("declared column is missing from the source".to_owned()),
                    observed: BTreeMap::new(),
                },
            });
            continue;
        };
        let dtype = columns
            .iter()
            .find(|c| &c.name == name)
            .map(|c| c.dtype.clone())
            .unwrap_or_default();
        probe.plan_type_check(
            name,
            col.r#type,
            family,
            &dtype,
            contract.settings.on_type_mismatch,
        );
        if col.required {
            probe.plan_required(name);
        }
        for check in &col.checks {
            probe.plan_column_check(name, family, check);
        }
    }
    probe.plan_primary_key(contract, &present);
    for dc in &contract.dataset_checks {
        probe.plan_dataset_check(dc, &families);
    }
    if options.profile {
        for c in columns {
            let family = families
                .get(c.name.as_str())
                .copied()
                .unwrap_or(Family::Other);
            probe.plan_profile(&c.name, family);
        }
    }
    probe
}

impl Probe {
    fn slot(&mut self, expr: &str) -> usize {
        self.items.push(expr.to_owned());
        self.items.len() - 1
    }

    fn col(&self, name: &str) -> String {
        self.dialect.ident(name)
    }

    /// The column as text: itself when it already is, cast otherwise.
    fn text_of(&self, name: &str, family: Family) -> String {
        let col = self.col(name);
        if family == Family::Text {
            col
        } else {
            self.dialect.to_text(&col)
        }
    }

    /// The expression a column is grouped or counted distinct by — the column
    /// itself for types every warehouse can compare, its text for the rest
    /// (Postgres has no equality for `json`).
    fn key_of(&self, name: &str, family: Family) -> String {
        if family == Family::Other {
            self.dialect.to_text(&self.col(name))
        } else {
            self.col(name)
        }
    }

    fn failed_where(&self, predicate: &str) -> String {
        format!("SUM(CASE WHEN {predicate} THEN 1 ELSE 0 END)")
    }

    fn sample_sql(&self, shown: &str, predicate: &str) -> String {
        format!(
            "SELECT {} AS v FROM {} WHERE {predicate} LIMIT {}",
            self.dialect.to_text(shown),
            self.table,
            self.sample_failures.max(1)
        )
    }

    fn plan_columns_check(&mut self, contract: &Contract, present: &BTreeSet<&str>) {
        let declared: Vec<String> = contract.columns.keys().cloned().collect();
        let extra: Vec<String> = present
            .iter()
            .filter(|c| !contract.columns.contains_key(**c))
            .map(|c| (*c).to_owned())
            .collect();
        let missing: Vec<String> = declared
            .iter()
            .filter(|c| !present.contains(c.as_str()))
            .cloned()
            .collect();
        let allow_extra = contract.settings.allow_extra_columns;
        let exact = contract.settings.columns_exact;
        let extra_violation = (!allow_extra || exact) && !extra.is_empty();
        let missing_violation = exact && !missing.is_empty();
        let failed = extra_violation || missing_violation;
        let mut observed = BTreeMap::new();
        observed.insert("declared_columns".to_owned(), json!(declared.len()));
        if !extra.is_empty() {
            observed.insert("extra_columns".to_owned(), json!(extra));
        }
        if !missing.is_empty() {
            observed.insert("missing_columns".to_owned(), json!(missing));
        }
        let mut parts = Vec::new();
        if extra_violation {
            parts.push(format!("unexpected column(s): {}", extra.join(", ")));
        }
        if missing_violation {
            parts.push(format!("missing column(s): {}", missing.join(", ")));
        }
        self.checks.push(Planned {
            meta: Meta {
                id: "dataset.columns".to_owned(),
                column: None,
                kind: "columns",
                params: json!({
                    "allow_extra_columns": allow_extra,
                    "columns_exact": exact,
                }),
                severity: Severity::Error,
            },
            how: How::Fixed {
                failed,
                message: failed.then(|| parts.join("; ")),
                observed,
            },
        });
    }

    fn plan_type_check(
        &mut self,
        name: &str,
        declared: ColType,
        family: Family,
        dtype: &str,
        severity: Severity,
    ) {
        let meta = Meta {
            id: format!("{name}.type"),
            column: Some(name.to_owned()),
            kind: "type",
            params: json!({ "type": type_name(declared) }),
            severity,
        };
        let col = self.col(name);
        let compatible = match declared {
            ColType::String => family == Family::Text,
            ColType::Int => family == Family::Int,
            ColType::Float => matches!(family, Family::Int | Family::Float),
            ColType::Bool => family == Family::Bool,
            ColType::Date => family == Family::Date,
            ColType::Datetime => family == Family::Datetime,
        };
        let how = if compatible {
            let evaluated = self.slot(&format!("COUNT({col})"));
            How::Counted {
                evaluated,
                failed: None,
                extreme: None,
                wording: Wording::Type,
                bound_message: None,
                sample: None,
            }
        } else if family == Family::Text {
            // Stored as text, declared as something else: parse every value,
            // as the engine does for CSV.
            let bad = match declared {
                ColType::Int => format!("NOT {}", self.dialect.regex_match(&col, INT_SHAPE)),
                ColType::Float => format!("NOT {}", self.dialect.regex_match(&col, FLOAT_SHAPE)),
                ColType::Bool => format!(
                    "LOWER({col}) NOT IN ({})",
                    BOOL_WORDS
                        .iter()
                        .map(|w| self.dialect.literal(w))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                ColType::Date => format!("NOT {}", self.dialect.regex_match(&col, DATE_SHAPE)),
                ColType::Datetime => {
                    format!("NOT {}", self.dialect.regex_match(&col, DATETIME_SHAPE))
                }
                ColType::String => "FALSE".to_owned(),
            };
            let predicate = format!("{col} IS NOT NULL AND {bad}");
            let evaluated = self.slot(&format!("COUNT({col})"));
            let failed = self.slot(&self.failed_where(&predicate));
            How::Counted {
                evaluated,
                failed: Some(failed),
                extreme: None,
                wording: Wording::Type,
                bound_message: None,
                sample: Some(self.sample_sql(&col, &predicate)),
            }
        } else {
            let mut observed = BTreeMap::new();
            observed.insert("observed_type".to_owned(), json!(dtype));
            How::Fixed {
                failed: true,
                message: Some("column has the wrong type (structural mismatch)".to_owned()),
                observed,
            }
        };
        self.checks.push(Planned { meta, how });
    }

    fn plan_required(&mut self, name: &str) {
        let col = self.col(name);
        let predicate = format!("{col} IS NULL");
        let failed = self.slot(&self.failed_where(&predicate));
        let sample = self.sample_sql(&self.dialect.literal("(null)"), &predicate);
        self.checks.push(Planned {
            meta: Meta {
                id: format!("{name}.required"),
                column: Some(name.to_owned()),
                kind: "required",
                params: json!({ "required": true }),
                severity: Severity::Error,
            },
            how: How::Counted {
                evaluated: 0,
                failed: Some(failed),
                extreme: None,
                wording: Wording::Required,
                bound_message: None,
                sample: Some(sample),
            },
        });
    }

    /// A row-level check over the non-null values of a column: `bad` is the
    /// SQL predicate a value fails on.
    fn counted(&mut self, meta: Meta, name: &str, bad: &str, wording: Wording) {
        let col = self.col(name);
        let predicate = format!("{col} IS NOT NULL AND {bad}");
        let evaluated = self.slot(&format!("COUNT({col})"));
        let failed = self.slot(&self.failed_where(&predicate));
        let sample = self.sample_sql(&col, &predicate);
        self.checks.push(Planned {
            meta,
            how: How::Counted {
                evaluated,
                failed: Some(failed),
                extreme: None,
                wording,
                bound_message: None,
                sample: Some(sample),
            },
        });
    }

    fn cannot_run(&mut self, meta: Meta, why: String) {
        self.checks.push(Planned {
            meta,
            how: How::Fixed {
                failed: true,
                message: Some(format!("check could not run: {why}")),
                observed: BTreeMap::new(),
            },
        });
    }

    /// The column as a number the warehouse can compare, if it can be one.
    fn numeric_of(&self, name: &str, family: Family) -> Option<String> {
        let col = self.col(name);
        match family {
            Family::Int | Family::Float => Some(col),
            Family::Text => Some(self.dialect.try_double(&col)),
            _ => None,
        }
    }

    fn plan_column_check(&mut self, name: &str, family: Family, check: &ColumnCheck) {
        let severity = check.severity();
        let column = Some(name.to_owned());
        let text = self.text_of(name, family);
        match check {
            ColumnCheck::Unique { approx, .. } => {
                let col = self.col(name);
                let key = self.key_of(name, family);
                let nonnull = self.slot(&format!("COUNT({col})"));
                let distinct = self.slot(&format!("COUNT(DISTINCT {key})"));
                let sample = format!(
                    "SELECT {} AS v FROM {} WHERE {col} IS NOT NULL GROUP BY {key} HAVING COUNT(*) > 1 LIMIT {}",
                    self.dialect.to_text(&key),
                    self.table,
                    self.sample_failures.max(1)
                );
                self.checks.push(Planned {
                    meta: Meta {
                        id: format!("{name}.unique"),
                        column,
                        kind: "unique",
                        params: json!({ "approx": approx }),
                        severity,
                    },
                    how: How::Unique {
                        nonnull,
                        distinct,
                        sample,
                    },
                });
            }
            ColumnCheck::NotEmptyString { .. } => {
                let meta = Meta {
                    id: format!("{name}.not_empty_string"),
                    column,
                    kind: "not_empty_string",
                    params: json!({}),
                    severity,
                };
                self.counted(meta, name, &format!("{text} = ''"), Wording::NotEmpty);
            }
            ColumnCheck::Min { min, .. } => {
                self.plan_bound(name, family, *min, true, severity);
            }
            ColumnCheck::Max { max, .. } => {
                self.plan_bound(name, family, *max, false, severity);
            }
            ColumnCheck::Regex { regex, .. } => {
                let meta = Meta {
                    id: format!("{name}.regex"),
                    column,
                    kind: "regex",
                    params: json!({ "regex": regex }),
                    severity,
                };
                let bad = format!("NOT {}", self.dialect.regex_match(&text, regex));
                self.counted(meta, name, &bad, Wording::Regex);
            }
            ColumnCheck::Enum { values, .. } => {
                let rendered: Vec<String> = values.iter().map(|v| v.to_string()).collect();
                let meta = Meta {
                    id: format!("{name}.enum"),
                    column,
                    kind: "enum",
                    params: json!({ "enum": rendered }),
                    severity,
                };
                let list = rendered
                    .iter()
                    .map(|v| self.dialect.literal(v))
                    .collect::<Vec<_>>()
                    .join(", ");
                let bad = if list.is_empty() {
                    "TRUE".to_owned()
                } else {
                    format!("{text} NOT IN ({list})")
                };
                self.counted(meta, name, &bad, Wording::Enum);
            }
            ColumnCheck::Length { length, .. } => {
                let meta = Meta {
                    id: format!("{name}.length"),
                    column,
                    kind: "length",
                    params: length_json(*length),
                    severity,
                };
                let (min, max) = length.bounds();
                let mut parts = vec![format!("LENGTH({text}) < {min}")];
                if let Some(max) = max {
                    parts.push(format!("LENGTH({text}) > {max}"));
                }
                let bad = format!("({})", parts.join(" OR "));
                self.counted(meta, name, &bad, Wording::Length);
            }
            ColumnCheck::Format { format, .. } => {
                let meta = Meta {
                    id: format!("{name}.format.{}", format_name(*format)),
                    column,
                    kind: "format",
                    params: json!({ "format": format_name(*format) }),
                    severity,
                };
                let bad = match format {
                    KnownFormat::Email => {
                        format!("NOT {}", self.dialect.regex_match(&text, EMAIL_SHAPE))
                    }
                    KnownFormat::Uuid => {
                        format!("NOT {}", self.dialect.regex_match(&text, UUID_SHAPE))
                    }
                    KnownFormat::IsoDate => {
                        format!("NOT {}", self.dialect.regex_match(&text, DATE_SHAPE))
                    }
                    KnownFormat::IsoDatetime => {
                        format!("NOT {}", self.dialect.regex_match(&text, DATETIME_SHAPE))
                    }
                    KnownFormat::Url => {
                        format!("NOT {}", self.dialect.regex_match(&text, URL_SHAPE))
                    }
                    KnownFormat::CountryCodeIso2 => format!(
                        "{text} NOT IN ({})",
                        ISO_3166_1_ALPHA2
                            .iter()
                            .map(|c| self.dialect.literal(c))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                self.counted(meta, name, &bad, Wording::Format);
            }
            ColumnCheck::NullRatioMax { ratio, .. } => {
                let col = self.col(name);
                let nonnull = self.slot(&format!("COUNT({col})"));
                self.checks.push(Planned {
                    meta: Meta {
                        id: format!("{name}.null_ratio_max"),
                        column,
                        kind: "null_ratio_max",
                        params: json!({ "ratio": ratio }),
                        severity,
                    },
                    how: How::NullRatio {
                        nonnull,
                        max_ratio: *ratio,
                    },
                });
            }
            ColumnCheck::UniqueRatioMin { ratio, .. } => {
                let col = self.col(name);
                let key = self.key_of(name, family);
                let nonnull = self.slot(&format!("COUNT({col})"));
                let distinct = self.slot(&format!("COUNT(DISTINCT {key})"));
                self.checks.push(Planned {
                    meta: Meta {
                        id: format!("{name}.unique_ratio_min"),
                        column,
                        kind: "unique_ratio_min",
                        params: json!({ "ratio": ratio }),
                        severity,
                    },
                    how: How::UniqueRatio {
                        nonnull,
                        distinct,
                        min_ratio: *ratio,
                    },
                });
            }
            ColumnCheck::CustomExpr { expr, .. } => {
                let meta = Meta {
                    id: format!("{name}.custom_expr"),
                    column,
                    kind: "custom_expr",
                    params: json!({ "expr": expr }),
                    severity,
                };
                self.plan_expression(meta, expr, true);
            }
        }
    }

    fn plan_bound(
        &mut self,
        name: &str,
        family: Family,
        bound: Number,
        is_min: bool,
        severity: Severity,
    ) {
        let (kind, key) = if is_min {
            ("min", "min")
        } else {
            ("max", "max")
        };
        let meta = Meta {
            id: format!("{name}.{kind}"),
            column: Some(name.to_owned()),
            kind,
            params: json!({ key: number_json(bound) }),
            severity,
        };
        let Some(num) = self.numeric_of(name, family) else {
            self.cannot_run(
                meta,
                format!(
                    "a {} column cannot be compared with a number in the warehouse",
                    family_word(family)
                ),
            );
            return;
        };
        let col = self.col(name);
        let literal = number_literal(bound);
        let predicate = if is_min {
            format!("{num} < {literal}")
        } else {
            format!("{num} > {literal}")
        };
        let evaluated = self.slot(&format!("COUNT({num})"));
        let failed = self.slot(&self.failed_where(&predicate));
        let extreme = self.slot(&format!("{}({num})", if is_min { "MIN" } else { "MAX" }));
        let sample = self.sample_sql(&col, &predicate);
        self.checks.push(Planned {
            meta,
            how: How::Counted {
                evaluated,
                failed: Some(failed),
                extreme: Some((
                    extreme,
                    if is_min {
                        "min_observed"
                    } else {
                        "max_observed"
                    },
                )),
                wording: Wording::Regex,
                bound_message: Some(
                    if is_min {
                        "min observed"
                    } else {
                        "max observed"
                    }
                    .to_owned(),
                ),
                sample: Some(sample),
            },
        });
    }

    fn plan_expression(&mut self, meta: Meta, expr: &str, row_level: bool) {
        let rewritten = self.dialect.rewrite_expr(expr);
        let sql = if row_level {
            format!(
                "SELECT {} AS n, {} AS f FROM {}",
                self.dialect.to_text("COUNT(*)"),
                self.dialect.to_text(&format!(
                    "SUM(CASE WHEN COALESCE(({rewritten}), FALSE) THEN 0 ELSE 1 END)"
                )),
                self.table
            )
        } else {
            format!(
                "SELECT {} AS v, {} AS n FROM {}",
                self.dialect.to_text(&format!("({rewritten})")),
                self.dialect.to_text("COUNT(*)"),
                self.table
            )
        };
        self.expressions.push(Statement {
            label: format!("expr:{}", meta.id),
            sql,
        });
        let statement = self.expressions.len() - 1;
        self.checks.push(Planned {
            meta,
            how: How::Expression {
                statement,
                row_level,
                expr: expr.to_owned(),
            },
        });
    }

    fn plan_primary_key(&mut self, contract: &Contract, present: &BTreeSet<&str>) {
        if contract.primary_key.is_empty() {
            return;
        }
        let meta = Meta {
            id: "dataset.primary_key".to_owned(),
            column: None,
            kind: "primary_key",
            params: json!({ "columns": contract.primary_key }),
            severity: Severity::Error,
        };
        let missing: Vec<&String> = contract
            .primary_key
            .iter()
            .filter(|c| !present.contains(c.as_str()))
            .collect();
        if !missing.is_empty() {
            self.checks.push(Planned {
                meta,
                how: How::Fixed {
                    failed: true,
                    message: Some(format!(
                        "key column(s) missing from the source: {}",
                        missing
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                    observed: BTreeMap::new(),
                },
            });
            return;
        }
        let cols: Vec<String> = contract.primary_key.iter().map(|c| self.col(c)).collect();
        let all_present = cols
            .iter()
            .map(|c| format!("{c} IS NOT NULL"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let texts: Vec<String> = cols.iter().map(|c| self.dialect.to_text(c)).collect();
        let key_expr = if texts.len() == 1 {
            texts[0].clone()
        } else {
            format!(
                "CONCAT({})",
                texts.join(&format!(
                    ", {}, ",
                    self.dialect.literal(&KEY_SEPARATOR.to_string())
                ))
            )
        };
        let shown = if texts.len() == 1 {
            texts[0].clone()
        } else {
            format!(
                "CONCAT({})",
                texts.join(&format!(", {}, ", self.dialect.literal(", ")))
            )
        };
        let nonnull_rows = self.slot(&self.failed_where(&all_present));
        let distinct_keys = self.slot(&format!(
            "COUNT(DISTINCT CASE WHEN {all_present} THEN {key_expr} END)"
        ));
        let group_by = cols.join(", ");
        let sample = format!(
            "SELECT {shown} AS v FROM {} WHERE {all_present} GROUP BY {group_by} HAVING COUNT(*) > 1 LIMIT {}",
            self.table,
            self.sample_failures.max(1)
        );
        let keys = format!(
            "SELECT {} FROM {} WHERE {all_present} GROUP BY {group_by} LIMIT {}",
            texts.join(", "),
            self.table,
            self.max_keys.saturating_add(1)
        );
        self.checks.push(Planned {
            meta,
            how: How::PrimaryKey {
                nonnull_rows,
                distinct_keys,
                columns: contract.primary_key.clone(),
                sample,
                keys,
            },
        });
    }

    fn plan_dataset_check(&mut self, dc: &DatasetCheck, families: &BTreeMap<&str, Family>) {
        let severity = dc.severity();
        match dc {
            DatasetCheck::Assert { expr, .. } => {
                let meta = Meta {
                    id: "dataset.assert".to_owned(),
                    column: None,
                    kind: "assert",
                    params: json!({ "expr": expr }),
                    severity,
                };
                self.plan_expression(meta, expr, false);
            }
            DatasetCheck::CustomExpr { expr, .. } => {
                let meta = Meta {
                    id: "dataset.custom_expr".to_owned(),
                    column: None,
                    kind: "custom_expr",
                    params: json!({ "expr": expr }),
                    severity,
                };
                self.plan_expression(meta, expr, true);
            }
            DatasetCheck::RowCountMin { count, .. } => self.checks.push(Planned {
                meta: Meta {
                    id: "dataset.row_count_min".to_owned(),
                    column: None,
                    kind: "row_count_min",
                    params: json!({ "count": count }),
                    severity,
                },
                how: How::RowCount {
                    is_min: true,
                    bound: *count,
                },
            }),
            DatasetCheck::RowCountMax { count, .. } => self.checks.push(Planned {
                meta: Meta {
                    id: "dataset.row_count_max".to_owned(),
                    column: None,
                    kind: "row_count_max",
                    params: json!({ "count": count }),
                    severity,
                },
                how: How::RowCount {
                    is_min: false,
                    bound: *count,
                },
            }),
            DatasetCheck::Freshness {
                column, max_age, ..
            } => {
                let secs = max_age.as_secs() as i64;
                let meta = Meta {
                    id: format!("dataset.freshness.{column}"),
                    column: Some(column.clone()),
                    kind: "freshness",
                    params: json!({
                        "column": column,
                        "max_age": humanize_secs(secs),
                        "max_age_secs": secs,
                    }),
                    severity,
                };
                let Some(&family) = families.get(column.as_str()) else {
                    self.cannot_run(meta, format!("column `{column}` is not in the source"));
                    return;
                };
                let col = self.col(column);
                let epoch_expr = match family {
                    Family::Date | Family::Datetime => {
                        Some(self.dialect.epoch_seconds(&format!("MAX({col})")))
                    }
                    Family::Text => self
                        .dialect
                        .try_timestamp(&col)
                        .map(|ts| format!("MAX({})", self.dialect.epoch_seconds(&ts))),
                    _ => None,
                };
                let Some(epoch_expr) = epoch_expr else {
                    self.cannot_run(
                        meta,
                        format!(
                            "a {} column cannot be read as a time in the warehouse",
                            family_word(family)
                        ),
                    );
                    return;
                };
                let epoch = self.slot(&epoch_expr);
                self.checks.push(Planned {
                    meta,
                    how: How::Freshness {
                        epoch,
                        max_secs: secs,
                    },
                });
            }
            DatasetCheck::NullRatioMax { column, ratio, .. } => {
                let meta = Meta {
                    id: format!("dataset.null_ratio_max.{column}"),
                    column: Some(column.clone()),
                    kind: "null_ratio_max",
                    params: json!({ "column": column, "ratio": ratio }),
                    severity,
                };
                if !families.contains_key(column.as_str()) {
                    self.cannot_run(meta, format!("column `{column}` is not in the source"));
                    return;
                }
                let col = self.col(column);
                let nonnull = self.slot(&format!("COUNT({col})"));
                self.checks.push(Planned {
                    meta,
                    how: How::NullRatio {
                        nonnull,
                        max_ratio: *ratio,
                    },
                });
            }
            DatasetCheck::UniqueRatioMin { column, ratio, .. } => {
                let meta = Meta {
                    id: format!("dataset.unique_ratio_min.{column}"),
                    column: Some(column.clone()),
                    kind: "unique_ratio_min",
                    params: json!({ "column": column, "ratio": ratio }),
                    severity,
                };
                let Some(&family) = families.get(column.as_str()) else {
                    self.cannot_run(meta, format!("column `{column}` is not in the source"));
                    return;
                };
                let col = self.col(column);
                let key = self.key_of(column, family);
                let nonnull = self.slot(&format!("COUNT({col})"));
                let distinct = self.slot(&format!("COUNT(DISTINCT {key})"));
                self.checks.push(Planned {
                    meta,
                    how: How::UniqueRatio {
                        nonnull,
                        distinct,
                        min_ratio: *ratio,
                    },
                });
            }
            DatasetCheck::References {
                columns,
                dataset,
                to,
                ..
            } => {
                let meta = Meta {
                    id: format!("dataset.references.{dataset}"),
                    column: None,
                    kind: "references",
                    params: json!({ "columns": columns, "dataset": dataset, "to": to }),
                    severity,
                };
                let missing: Vec<&str> = columns
                    .iter()
                    .filter(|c| !families.contains_key(c.as_str()))
                    .map(|c| c.as_str())
                    .collect();
                if !missing.is_empty() {
                    self.cannot_run(
                        meta,
                        format!("column(s) missing from the source: {}", missing.join(", ")),
                    );
                    return;
                }
                match self.references.get(dataset) {
                    None => {
                        self.cannot_run(
                            meta,
                            format!(
                                "no keys are recorded for `{dataset}` yet — validate that dataset first"
                            ),
                        );
                        return;
                    }
                    Some(set) if &set.columns != to => {
                        self.cannot_run(
                            meta,
                            format!(
                                "the keys recorded for `{dataset}` are ({}), not ({})",
                                set.columns.join(", "),
                                to.join(", ")
                            ),
                        );
                        return;
                    }
                    Some(_) => {}
                }
                let cols: Vec<String> = columns.iter().map(|c| self.col(c)).collect();
                let all_present = cols
                    .iter()
                    .map(|c| format!("{c} IS NOT NULL"))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                let texts: Vec<String> = cols.iter().map(|c| self.dialect.to_text(c)).collect();
                let nonnull_rows = self.slot(&self.failed_where(&all_present));
                let groups = format!(
                    "SELECT {}, {} AS n FROM {} WHERE {all_present} GROUP BY {} LIMIT {}",
                    texts.join(", "),
                    self.dialect.to_text("COUNT(*)"),
                    self.table,
                    cols.join(", "),
                    self.max_keys.saturating_add(1)
                );
                self.checks.push(Planned {
                    meta,
                    how: How::References {
                        nonnull_rows,
                        dataset: dataset.clone(),
                        groups,
                    },
                });
            }
        }
    }

    fn plan_profile(&mut self, name: &str, family: Family) {
        let col = self.col(name);
        let key = self.key_of(name, family);
        let nonnull = self.slot(&format!("COUNT({col})"));
        let distinct = self.slot(&format!("COUNT(DISTINCT {key})"));
        let ordered = matches!(
            family,
            Family::Int | Family::Float | Family::Date | Family::Datetime | Family::Text
        );
        let (min, max) = if ordered {
            (
                Some(self.slot(&format!("MIN({col})"))),
                Some(self.slot(&format!("MAX({col})"))),
            )
        } else {
            (None, None)
        };
        let numeric = matches!(family, Family::Int | Family::Float);
        let (mean, std) = if numeric {
            (
                Some(self.slot(&format!("AVG({col})"))),
                Some(self.slot(&format!("STDDEV_SAMP({col})"))),
            )
        } else {
            (None, None)
        };
        let (min_length, max_length) = if family == Family::Text {
            (
                Some(self.slot(&format!("MIN(LENGTH({col}))"))),
                Some(self.slot(&format!("MAX(LENGTH({col}))"))),
            )
        } else {
            (None, None)
        };
        let values = format!(
            "SELECT {} AS v FROM {} WHERE {col} IS NOT NULL GROUP BY {key} LIMIT {}",
            self.dialect.to_text(&key),
            self.table,
            KEEP_VALUES_CAP + 1
        );
        self.profile.push(ProfileSlots {
            name: name.to_owned(),
            family,
            nonnull,
            distinct,
            min,
            max,
            mean,
            std,
            min_length,
            max_length,
            values,
        });
    }

    /// The first round of statements: the aggregate scan, then one statement
    /// per expression check. Run them all; a failure of one is a failure of
    /// that statement's checks only.
    pub fn statements(&self) -> Vec<Statement> {
        let mut out = Vec::with_capacity(1 + self.expressions.len());
        let items = self
            .items
            .iter()
            .enumerate()
            .map(|(i, expr)| format!("  {} AS c{i}", self.dialect.to_text(expr)))
            .collect::<Vec<_>>()
            .join(",\n");
        out.push(Statement {
            label: "aggregate".to_owned(),
            sql: format!("SELECT\n{items}\nFROM {}", self.table),
        });
        out.extend(self.expressions.iter().cloned());
        out
    }

    /// The second round, chosen from the first round's answers: failure
    /// samples for the checks that failed, the keys `primary_key` and
    /// `references` need, and the value set of every low-cardinality column.
    /// Optional — [`Probe::decode`] does without any of it — but a run with it
    /// says *which* values failed, keeps its keys for other datasets, and
    /// notices a category that was never there before.
    pub fn follow_ups(&mut self, replies: &[Reply]) -> Vec<Statement> {
        let mut out = Vec::new();
        let Some(main) = replies
            .first()
            .and_then(Reply::rows)
            .and_then(|r| r.first())
        else {
            self.follow_ups = Vec::new();
            return Vec::new();
        };
        let rows = int_cell(main, 0).unwrap_or(0);
        for (i, planned) in self.checks.iter().enumerate() {
            match &planned.how {
                How::Counted {
                    failed: Some(failed),
                    sample: Some(sample),
                    ..
                } if int_cell(main, *failed).unwrap_or(0) > 0 => {
                    out.push((
                        Purpose::Samples(i),
                        Statement {
                            label: format!("samples:{}", planned.meta.id),
                            sql: sample.clone(),
                        },
                    ));
                }
                How::Unique {
                    nonnull,
                    distinct,
                    sample,
                } if int_cell(main, *nonnull).unwrap_or(0)
                    > int_cell(main, *distinct).unwrap_or(0) =>
                {
                    out.push((
                        Purpose::Samples(i),
                        Statement {
                            label: format!("samples:{}", planned.meta.id),
                            sql: sample.clone(),
                        },
                    ));
                }
                How::PrimaryKey {
                    nonnull_rows,
                    distinct_keys,
                    sample,
                    keys,
                    ..
                } => {
                    let nonnull = int_cell(main, *nonnull_rows).unwrap_or(0);
                    let distinct = int_cell(main, *distinct_keys).unwrap_or(0);
                    if nonnull > distinct {
                        out.push((
                            Purpose::Samples(i),
                            Statement {
                                label: format!("samples:{}", planned.meta.id),
                                sql: sample.clone(),
                            },
                        ));
                    }
                    if distinct <= self.max_keys && rows > 0 {
                        out.push((
                            Purpose::PrimaryKeys(i),
                            Statement {
                                label: format!("keys:{}", planned.meta.id),
                                sql: keys.clone(),
                            },
                        ));
                    }
                }
                How::References { groups, .. } => {
                    out.push((
                        Purpose::ReferenceGroups(i),
                        Statement {
                            label: format!("keys:{}", planned.meta.id),
                            sql: groups.clone(),
                        },
                    ));
                }
                _ => {}
            }
        }
        for (i, p) in self.profile.iter().enumerate() {
            let distinct = int_cell(main, p.distinct).unwrap_or(0);
            let short = p
                .max_length
                .and_then(|s| int_cell(main, s))
                .map(|len| len <= KEEP_VALUE_MAX_CHARS)
                .unwrap_or(true);
            let kept_kind = matches!(p.family, Family::Text | Family::Int | Family::Bool);
            if kept_kind && distinct > 0 && distinct <= KEEP_VALUES_CAP && short {
                out.push((
                    Purpose::Values(i),
                    Statement {
                        label: format!("values:{}", p.name),
                        sql: p.values.clone(),
                    },
                ));
            }
        }
        self.follow_ups = out.clone();
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// Reads the warehouse's answers into an [`EngineOutput`]. `replies` are
    /// the answers to [`Probe::statements`] in order; `follow_up_replies` the
    /// answers to [`Probe::follow_ups`] in order, or empty when that round was
    /// skipped.
    pub fn decode(self, replies: &[Reply], follow_up_replies: &[Reply]) -> EngineOutput {
        let main_reply = replies.first();
        let main_error = match main_reply {
            Some(Reply::Error(e)) => Some(e.clone()),
            Some(Reply::Rows(rows)) if rows.is_empty() => {
                Some("the aggregate query returned no row".to_owned())
            }
            None => Some("the aggregate query was not run".to_owned()),
            _ => None,
        };
        let main: Vec<Option<String>> = main_reply
            .and_then(Reply::rows)
            .and_then(|r| r.first())
            .cloned()
            .unwrap_or_default();
        let rows_total = int_cell(&main, 0).unwrap_or(0);

        // Follow-up answers, by what they were for.
        let mut samples: BTreeMap<usize, Vec<(u64, String)>> = BTreeMap::new();
        let mut key_rows: BTreeMap<usize, &Reply> = BTreeMap::new();
        let mut group_rows: BTreeMap<usize, &Reply> = BTreeMap::new();
        let mut values: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for ((purpose, _), reply) in self.follow_ups.iter().zip(follow_up_replies) {
            match purpose {
                Purpose::Samples(i) => {
                    if let Some(rows) = reply.rows() {
                        samples.insert(
                            *i,
                            rows.iter()
                                .map(|r| {
                                    (
                                        0,
                                        r.first()
                                            .cloned()
                                            .flatten()
                                            .unwrap_or_else(|| "(null)".to_owned()),
                                    )
                                })
                                .collect(),
                        );
                    }
                }
                Purpose::PrimaryKeys(i) => {
                    key_rows.insert(*i, reply);
                }
                Purpose::ReferenceGroups(i) => {
                    group_rows.insert(*i, reply);
                }
                Purpose::Values(i) => {
                    if let Some(rows) = reply.rows() {
                        let mut v: Vec<String> = rows
                            .iter()
                            .filter_map(|r| r.first().cloned().flatten())
                            .collect();
                        v.sort();
                        values.insert(*i, v);
                    }
                }
            }
        }

        let mut primary_key: Option<KeySet> = None;
        let mut checks = Vec::with_capacity(self.checks.len());
        for (i, planned) in self.checks.iter().enumerate() {
            let outcome = match &planned.how {
                How::Fixed {
                    failed,
                    message,
                    observed,
                } => outcome(
                    planned,
                    *failed,
                    None,
                    None,
                    observed.clone(),
                    Vec::new(),
                    message.clone(),
                ),
                _ if main_error.is_some() && !matches!(planned.how, How::Expression { .. }) => {
                    let why = main_error.clone().unwrap_or_default();
                    outcome(
                        planned,
                        true,
                        None,
                        None,
                        BTreeMap::new(),
                        Vec::new(),
                        Some(format!("check could not run: {why}")),
                    )
                }
                How::Counted {
                    evaluated,
                    failed,
                    extreme,
                    wording,
                    bound_message,
                    ..
                } => {
                    let evaluated = int_cell(&main, *evaluated).unwrap_or(0);
                    let failed_n = failed.and_then(|s| int_cell(&main, s)).unwrap_or(0);
                    let mut observed = BTreeMap::new();
                    if let Some((slot, key)) = extreme {
                        if let Some(x) = num_cell(&main, *slot) {
                            observed.insert((*key).to_owned(), json_number(x));
                        }
                    }
                    let message = (failed_n > 0).then(|| match (bound_message, extreme) {
                        (Some(prefix), Some((slot, _))) => match num_cell(&main, *slot) {
                            Some(x) => format!("{prefix}: {}", trim_float(x)),
                            None => format!("{failed_n} value(s) out of bounds"),
                        },
                        _ => wording_message(*wording, failed_n),
                    });
                    outcome(
                        planned,
                        failed_n > 0,
                        Some(evaluated),
                        Some(failed_n),
                        observed,
                        samples.remove(&i).unwrap_or_default(),
                        message,
                    )
                }
                How::Unique {
                    nonnull, distinct, ..
                } => {
                    let nonnull = int_cell(&main, *nonnull).unwrap_or(0);
                    let distinct = int_cell(&main, *distinct).unwrap_or(0);
                    let dupes = nonnull.saturating_sub(distinct);
                    let mut observed = BTreeMap::new();
                    observed.insert("distinct_count".to_owned(), json!(distinct));
                    let samples = samples.remove(&i).unwrap_or_default();
                    let message = (dupes > 0).then(|| match samples.first() {
                        Some((_, v)) => format!("{dupes} duplicate value(s), e.g. \"{v}\""),
                        None => format!("{dupes} duplicate value(s)"),
                    });
                    outcome(
                        planned,
                        dupes > 0,
                        Some(nonnull),
                        Some(dupes),
                        observed,
                        samples,
                        message,
                    )
                }
                How::NullRatio { nonnull, max_ratio } => {
                    let nonnull = int_cell(&main, *nonnull).unwrap_or(0);
                    let nulls = rows_total.saturating_sub(nonnull);
                    let ratio = if rows_total == 0 {
                        0.0
                    } else {
                        nulls as f64 / rows_total as f64
                    };
                    let failed = ratio > *max_ratio;
                    let mut observed = BTreeMap::new();
                    observed.insert("null_ratio".to_owned(), json_number(ratio));
                    let message =
                        failed.then(|| format!("null ratio {:.4} exceeds {:.4}", ratio, max_ratio));
                    outcome(
                        planned,
                        failed,
                        Some(rows_total),
                        Some(nulls),
                        observed,
                        Vec::new(),
                        message,
                    )
                }
                How::UniqueRatio {
                    nonnull,
                    distinct,
                    min_ratio,
                } => {
                    let nonnull = int_cell(&main, *nonnull).unwrap_or(0);
                    let distinct = int_cell(&main, *distinct).unwrap_or(0);
                    let ratio = if nonnull == 0 {
                        1.0
                    } else {
                        distinct as f64 / nonnull as f64
                    };
                    let failed = ratio < *min_ratio;
                    let mut observed = BTreeMap::new();
                    observed.insert("unique_ratio".to_owned(), json_number(ratio));
                    observed.insert("distinct_count".to_owned(), json!(distinct));
                    let message =
                        failed.then(|| format!("unique ratio {:.4} below {:.4}", ratio, min_ratio));
                    outcome(
                        planned,
                        failed,
                        Some(nonnull),
                        None,
                        observed,
                        Vec::new(),
                        message,
                    )
                }
                How::RowCount { is_min, bound } => {
                    let failed = if *is_min {
                        rows_total < *bound
                    } else {
                        rows_total > *bound
                    };
                    let mut observed = BTreeMap::new();
                    observed.insert("row_count".to_owned(), json!(rows_total));
                    let message = failed.then(|| {
                        if *is_min {
                            format!("{rows_total} rows, fewer than the required {bound}")
                        } else {
                            format!("{rows_total} rows, more than the allowed {bound}")
                        }
                    });
                    outcome(planned, failed, None, None, observed, Vec::new(), message)
                }
                How::Freshness { epoch, max_secs } => match num_cell(&main, *epoch) {
                    None => outcome(
                        planned,
                        true,
                        None,
                        None,
                        BTreeMap::new(),
                        Vec::new(),
                        Some(if rows_total == 0 {
                            "no rows, so nothing is fresh".to_owned()
                        } else {
                            "check could not run: no value in the column reads as a time".to_owned()
                        }),
                    ),
                    Some(newest) => {
                        let age_secs = self.now.timestamp() as f64 - newest;
                        let age_hours = age_secs / 3_600.0;
                        let failed = age_secs > *max_secs as f64;
                        let mut observed = BTreeMap::new();
                        observed.insert("age_hours".to_owned(), json_number(age_hours));
                        let message = failed
                            .then(|| format!("newest row is {}h old", age_hours.round() as i64));
                        outcome(planned, failed, None, None, observed, Vec::new(), message)
                    }
                },
                How::PrimaryKey {
                    nonnull_rows,
                    distinct_keys,
                    columns,
                    ..
                } => {
                    let nonnull = int_cell(&main, *nonnull_rows).unwrap_or(0);
                    let distinct = int_cell(&main, *distinct_keys).unwrap_or(0);
                    let duplicate_rows = nonnull.saturating_sub(distinct);
                    let null_rows = rows_total.saturating_sub(nonnull);
                    let rows_failed = duplicate_rows + null_rows;
                    let samples = samples.remove(&i).unwrap_or_default();
                    let message = (rows_failed > 0).then(|| {
                        let mut parts = Vec::new();
                        if duplicate_rows > 0 {
                            parts.push(match samples.first() {
                                Some((_, v)) => {
                                    format!("{duplicate_rows} repeated key(s), e.g. \"{v}\"")
                                }
                                None => format!("{duplicate_rows} repeated key(s)"),
                            });
                        }
                        if null_rows > 0 {
                            parts.push(format!("{null_rows} row(s) with a null in the key"));
                        }
                        parts.join("; ")
                    });
                    let mut observed = BTreeMap::new();
                    observed.insert("distinct_keys".to_owned(), json!(distinct));
                    observed.insert("duplicate_rows".to_owned(), json!(duplicate_rows));
                    observed.insert("null_key_rows".to_owned(), json!(null_rows));
                    if let Some(Reply::Rows(rows)) = key_rows.get(&i) {
                        if rows.len() as u64 <= self.max_keys {
                            let hashes = rows.iter().map(|r| {
                                hash_key(r.iter().map(|c| c.as_deref().unwrap_or_default()))
                            });
                            primary_key = Some(KeySet::from_hashes(columns.clone(), hashes));
                        }
                    }
                    outcome(
                        planned,
                        rows_failed > 0,
                        Some(rows_total),
                        Some(rows_failed),
                        observed,
                        samples,
                        message,
                    )
                }
                How::Expression {
                    statement,
                    row_level,
                    expr,
                } => {
                    let reply = replies.get(statement + 1);
                    decode_expression(planned, reply, *row_level, expr)
                }
                How::References {
                    nonnull_rows,
                    dataset,
                    ..
                } => {
                    let nonnull = int_cell(&main, *nonnull_rows).unwrap_or(0);
                    let null_rows = rows_total.saturating_sub(nonnull);
                    let set = self.references.get(dataset).cloned();
                    let (failed_rows, first_missing, why) = match (group_rows.get(&i), set.as_ref())
                    {
                        (Some(Reply::Rows(rows)), Some(set)) => {
                            if rows.len() as u64 > self.max_keys {
                                (0, None, Some(format!(
                                    "more than {} distinct keys in the warehouse; evaluate this feed by pull",
                                    self.max_keys
                                )))
                            } else {
                                let mut failed_rows = 0u64;
                                let mut first: Option<String> = None;
                                for r in rows {
                                    let n = r.len().saturating_sub(1);
                                    let parts: Vec<&str> = r[..n]
                                        .iter()
                                        .map(|c| c.as_deref().unwrap_or_default())
                                        .collect();
                                    if !set.contains(hash_key(parts.iter().copied())) {
                                        let count = r
                                            .get(n)
                                            .cloned()
                                            .flatten()
                                            .and_then(|c| c.parse::<f64>().ok())
                                            .unwrap_or(1.0)
                                            as u64;
                                        failed_rows += count.max(1);
                                        if first.is_none() {
                                            first = Some(parts.join(", "));
                                        }
                                    }
                                }
                                (failed_rows, first, None)
                            }
                        }
                        (Some(Reply::Error(e)), _) => (0, None, Some(e.clone())),
                        (None, _) => (0, None, Some("the key lookup was not run".to_owned())),
                        (_, None) => (
                            0,
                            None,
                            Some(format!("no keys are recorded for `{dataset}`")),
                        ),
                    };
                    if let Some(why) = why {
                        outcome(
                            planned,
                            true,
                            None,
                            None,
                            BTreeMap::new(),
                            Vec::new(),
                            Some(format!("check could not run: {why}")),
                        )
                    } else {
                        let mut observed = BTreeMap::new();
                        observed.insert("referenced_dataset".to_owned(), json!(dataset));
                        observed.insert(
                            "referenced_keys".to_owned(),
                            json!(set.as_ref().map(|s| s.len()).unwrap_or(0)),
                        );
                        observed.insert("rows_skipped_null".to_owned(), json!(null_rows));
                        let message = (failed_rows > 0).then(|| match &first_missing {
                            Some(v) => format!(
                                "{failed_rows} row(s) point at keys `{dataset}` does not have, e.g. \"{v}\""
                            ),
                            None => format!("{failed_rows} row(s) point at keys `{dataset}` does not have"),
                        });
                        let samples = first_missing.into_iter().map(|v| (0, v)).collect();
                        outcome(
                            planned,
                            failed_rows > 0,
                            Some(nonnull),
                            Some(failed_rows),
                            observed,
                            samples,
                            message,
                        )
                    }
                }
            };
            checks.push(outcome);
        }

        let profile = if self.profile.is_empty() || main_error.is_some() {
            None
        } else {
            Some(
                self.profile
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let nonnull = int_cell(&main, p.nonnull).unwrap_or(0);
                        let null_count = rows_total.saturating_sub(nonnull);
                        let distinct = int_cell(&main, p.distinct).unwrap_or(0);
                        let vals = values.remove(&i).unwrap_or_default();
                        let complete = !vals.is_empty() && vals.len() as u64 == distinct;
                        ColumnStats {
                            name: p.name.clone(),
                            null_count,
                            null_ratio: if rows_total == 0 {
                                0.0
                            } else {
                                null_count as f64 / rows_total as f64
                            },
                            distinct,
                            min: p.min.and_then(|s| text_cell(&main, s)),
                            max: p.max.and_then(|s| text_cell(&main, s)),
                            mean: p.mean.and_then(|s| num_cell(&main, s)),
                            std: p.std.and_then(|s| num_cell(&main, s)),
                            min_length: p.min_length.and_then(|s| int_cell(&main, s)),
                            max_length: p.max_length.and_then(|s| int_cell(&main, s)),
                            values: if complete { vals } else { Vec::new() },
                            values_complete: complete,
                        }
                    })
                    .collect(),
            )
        };

        EngineOutput {
            rows_total,
            source_columns: self.columns.len() as u64,
            observed_columns: self
                .columns
                .iter()
                .map(|c| ObservedColumn {
                    name: c.name.clone(),
                    dtype: Some(c.dtype.clone()),
                })
                .collect(),
            observed_typed: true,
            checks,
            primary_key,
            profile,
        }
    }
}

/// Profile value set: kept when a column has at most this many distinct
/// values, each at most [`KEEP_VALUE_MAX_CHARS`] long — the same rule as the
/// streaming profile.
const KEEP_VALUES_CAP: u64 = 25;
const KEEP_VALUE_MAX_CHARS: u64 = 64;

fn decode_expression(
    planned: &Planned,
    reply: Option<&Reply>,
    row_level: bool,
    expr: &str,
) -> CheckOutcome {
    let row = match reply {
        Some(Reply::Rows(rows)) => rows.first().cloned(),
        Some(Reply::Error(e)) => {
            return outcome(
                planned,
                true,
                None,
                None,
                BTreeMap::new(),
                Vec::new(),
                Some(format!("check could not run: {e}")),
            )
        }
        None => None,
    };
    let Some(row) = row else {
        return outcome(
            planned,
            true,
            None,
            None,
            BTreeMap::new(),
            Vec::new(),
            Some("check could not run: the expression returned no row".to_owned()),
        );
    };
    if row_level {
        let evaluated = int_cell(&row, 0).unwrap_or(0);
        let failed = int_cell(&row, 1).unwrap_or(0);
        let mut observed = BTreeMap::new();
        if let Some(r) = fail_ratio(failed, evaluated) {
            observed.insert("fail_ratio".to_owned(), json_number(r));
        }
        let message = (failed > 0).then(|| format!("{failed} row(s) failed `{expr}`"));
        return outcome(
            planned,
            failed > 0,
            Some(evaluated),
            Some(failed),
            observed,
            Vec::new(),
            message,
        );
    }
    let rows = int_cell(&row, 1).unwrap_or(0);
    let verdict = text_cell(&row, 0).and_then(|v| match v.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "1" => Some(true),
        "false" | "f" | "0" => Some(false),
        _ => None,
    });
    let (failed, message) = match verdict {
        Some(true) => (false, None),
        Some(false) => (true, Some(format!("`{expr}` is false over {rows} row(s)"))),
        None => (
            true,
            Some(format!(
                "`{expr}` came out null over {rows} row(s) — an average of no rows, or a column that could not be read as its declared type"
            )),
        ),
    };
    let mut observed = BTreeMap::new();
    observed.insert("rows".to_owned(), json!(rows));
    outcome(planned, failed, None, None, observed, Vec::new(), message)
}

#[allow(clippy::too_many_arguments)]
fn outcome(
    planned: &Planned,
    failed: bool,
    rows_evaluated: Option<u64>,
    rows_failed: Option<u64>,
    mut observed: BTreeMap<String, Value>,
    samples: Vec<(u64, String)>,
    message: Option<String>,
) -> CheckOutcome {
    if let (Some(f), Some(e)) = (rows_failed, rows_evaluated) {
        if let Some(r) = fail_ratio(f, e) {
            observed
                .entry("fail_ratio".to_owned())
                .or_insert_with(|| json_number(r));
        }
    }
    CheckOutcome {
        id: planned.meta.id.clone(),
        column: planned.meta.column.clone(),
        kind: planned.meta.kind.to_owned(),
        params: planned.meta.params.clone(),
        severity: planned.meta.severity,
        failed,
        rows_evaluated,
        rows_failed,
        observed,
        samples,
        message,
    }
}

fn wording_message(wording: Wording, failed: u64) -> String {
    match wording {
        Wording::Required => format!("{failed} null value(s) in a required column"),
        Wording::Type => format!("{failed} value(s) do not match the declared type"),
        Wording::Regex => format!("{failed} value(s) did not match the pattern"),
        Wording::Format => format!("{failed} value(s) are not valid"),
        Wording::Enum => format!("{failed} value(s) outside the allowed set"),
        Wording::Length => format!("{failed} value(s) have the wrong length"),
        Wording::NotEmpty => format!("{failed} empty string value(s)"),
    }
}

fn text_cell(row: &[Option<String>], slot: usize) -> Option<String> {
    row.get(slot).cloned().flatten()
}

fn num_cell(row: &[Option<String>], slot: usize) -> Option<f64> {
    text_cell(row, slot).and_then(|s| s.trim().parse::<f64>().ok())
}

fn int_cell(row: &[Option<String>], slot: usize) -> Option<u64> {
    let s = text_cell(row, slot)?;
    let s = s.trim();
    s.parse::<u64>()
        .ok()
        .or_else(|| s.parse::<f64>().ok().map(|x| x.round().max(0.0) as u64))
}

fn fail_ratio(failed: u64, evaluated: u64) -> Option<f64> {
    (evaluated > 0).then(|| failed as f64 / evaluated as f64)
}

fn json_number(x: f64) -> Value {
    if x.fract() == 0.0 && x.abs() < 9e15 {
        json!(x as i64)
    } else {
        serde_json::Number::from_f64(x)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

fn trim_float(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

fn number_json(n: Number) -> Value {
    match n {
        Number::Int(i) => json!(i),
        Number::Float(f) => json!(f),
    }
}

fn number_literal(n: Number) -> String {
    match n {
        Number::Int(i) => i.to_string(),
        Number::Float(f) => {
            if f.is_finite() {
                format!("{f:?}")
            } else {
                "NULL".to_owned()
            }
        }
    }
}

fn length_json(spec: LengthSpec) -> Value {
    match spec {
        LengthSpec::Exact(n) => json!({ "length": n }),
        LengthSpec::Range(r) => json!({ "min": r.min, "max": r.max }),
    }
}

fn type_name(t: ColType) -> &'static str {
    match t {
        ColType::String => "string",
        ColType::Int => "int",
        ColType::Float => "float",
        ColType::Bool => "bool",
        ColType::Date => "date",
        ColType::Datetime => "datetime",
    }
}

fn family_word(f: Family) -> &'static str {
    match f {
        Family::Int => "whole-number",
        Family::Float => "decimal",
        Family::Bool => "boolean",
        Family::Date => "date",
        Family::Datetime => "timestamp",
        Family::Text => "text",
        Family::Other => "non-scalar",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use chrono::TimeZone;

    fn contract(yaml: &str) -> Contract {
        plexuspact_contract::parse_str(yaml, "test.yaml").unwrap()
    }

    fn col(name: &str, dtype: &str) -> SourceColumn {
        SourceColumn {
            name: name.to_owned(),
            dtype: dtype.to_owned(),
        }
    }

    fn row(cells: &[&str]) -> Vec<Option<String>> {
        cells
            .iter()
            .map(|c| (*c != "NULL").then(|| (*c).to_owned()))
            .collect()
    }

    const ORDERS: &str = r#"
apiVersion: v1
dataset: orders
owner: data@acme.test
columns:
  order_id: { type: int, required: true }
  email:
    type: string
    checks:
      - format: email
      - unique: true
  amount:
    type: float
    checks:
      - min: 0
      - max: 10000
  status:
    type: string
    checks:
      - enum: [new, paid, shipped]
primary_key: [order_id]
dataset_checks:
  - row_count_min: 1
  - freshness: { column: created_at, max_age: 48h }
"#;

    fn orders_columns() -> Vec<SourceColumn> {
        vec![
            col("order_id", "bigint"),
            col("email", "character varying"),
            col("amount", "numeric(12,2)"),
            col("status", "text"),
            col("created_at", "timestamp with time zone"),
        ]
    }

    #[test]
    fn families_from_vendor_type_names() {
        assert_eq!(family_of(Dialect::Postgres, "bigint"), Family::Int);
        assert_eq!(
            family_of(Dialect::Postgres, "character varying(120)"),
            Family::Text
        );
        assert_eq!(family_of(Dialect::Postgres, "numeric"), Family::Float);
        assert_eq!(family_of(Dialect::Postgres, "numeric(10,0)"), Family::Int);
        assert_eq!(
            family_of(Dialect::Postgres, "timestamp with time zone"),
            Family::Datetime
        );
        assert_eq!(family_of(Dialect::Postgres, "json"), Family::Other);
        assert_eq!(family_of(Dialect::Postgres, "uuid"), Family::Text);
        assert_eq!(family_of(Dialect::Snowflake, "NUMBER"), Family::Int);
        assert_eq!(family_of(Dialect::Snowflake, "NUMBER(38,0)"), Family::Int);
        assert_eq!(family_of(Dialect::Snowflake, "NUMBER(10,2)"), Family::Float);
        assert_eq!(
            family_of(Dialect::Snowflake, "TIMESTAMP_NTZ(9)"),
            Family::Datetime
        );
        assert_eq!(family_of(Dialect::Databricks, "STRING"), Family::Text);
        assert_eq!(
            family_of(Dialect::Databricks, "DECIMAL(18,2)"),
            Family::Float
        );
        assert_eq!(family_of(Dialect::BigQuery, "INT64"), Family::Int);
        assert_eq!(family_of(Dialect::BigQuery, "FLOAT64"), Family::Float);
        assert_eq!(family_of(Dialect::BigQuery, "DATETIME"), Family::Datetime);
        assert_eq!(family_of(Dialect::BigQuery, "BOOL"), Family::Bool);
    }

    #[test]
    fn dialect_literals_and_regex_operators() {
        assert_eq!(Dialect::Postgres.literal("it's"), "'it''s'");
        assert_eq!(Dialect::Snowflake.literal(r"a\d'"), r"'a\\d\''");
        assert_eq!(Dialect::Postgres.ident("Order Id"), "\"Order Id\"");
        assert_eq!(Dialect::Databricks.ident("x"), "`x`");
        assert_eq!(
            Dialect::Postgres.regex_match("\"e\"", r"^\d+$"),
            r#"("e" ~ '^\d+$')"#
        );
        assert_eq!(
            Dialect::Snowflake.regex_match("\"e\"", r"^\d+$"),
            r#"(REGEXP_INSTR("e", '^\\d+$') > 0)"#
        );
        assert_eq!(
            Dialect::Databricks.regex_match("`e`", r"^\d+$"),
            r"(`e` RLIKE '^\\d+$')"
        );
        assert_eq!(
            Dialect::BigQuery.regex_match("`e`", r"^\d+$"),
            r"REGEXP_CONTAINS(`e`, r'^\d+$')"
        );
        assert_eq!(bigquery_regex_literal("it's"), "r\"it's\"");
        assert_eq!(
            Dialect::BigQuery.rewrite_expr("CAST(amount AS DOUBLE) > 0"),
            "CAST(amount AS FLOAT64) > 0"
        );
        assert_eq!(
            Dialect::Postgres.rewrite_expr("cast(a as double) > cast(b as Double)"),
            "cast(a AS DOUBLE PRECISION) > cast(b AS DOUBLE PRECISION)"
        );
        assert_eq!(Dialect::parse("Redshift"), Some(Dialect::Postgres));
        assert_eq!(Dialect::parse("warehouse"), Some(Dialect::Postgres));
        assert_eq!(Dialect::parse("duckdb"), None);
    }

    #[test]
    fn postgres_probe_is_one_aggregate_scan() {
        let c = contract(ORDERS);
        let probe = compile(
            &c,
            "\"public\".\"orders\"",
            Dialect::Postgres,
            &orders_columns(),
            &ProbeOptions {
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let statements = probe.statements();
        assert_eq!(
            statements.len(),
            1,
            "no expression checks, so one statement"
        );
        let sql = &statements[0].sql;
        let expected = "SELECT\n  \
            CAST(COUNT(*) AS TEXT) AS c0,\n  \
            CAST(COUNT(\"order_id\") AS TEXT) AS c1,\n  \
            CAST(SUM(CASE WHEN \"order_id\" IS NULL THEN 1 ELSE 0 END) AS TEXT) AS c2,\n  \
            CAST(COUNT(\"email\") AS TEXT) AS c3,\n  \
            CAST(COUNT(\"email\") AS TEXT) AS c4,\n  \
            CAST(SUM(CASE WHEN \"email\" IS NOT NULL AND NOT (\"email\" ~ '^[A-Za-z0-9._%+\\-]+@[A-Za-z0-9.\\-]+\\.[A-Za-z]{2,}$') THEN 1 ELSE 0 END) AS TEXT) AS c5,\n  \
            CAST(COUNT(\"email\") AS TEXT) AS c6,\n  \
            CAST(COUNT(DISTINCT \"email\") AS TEXT) AS c7,\n  \
            CAST(COUNT(\"amount\") AS TEXT) AS c8,\n  \
            CAST(COUNT(\"amount\") AS TEXT) AS c9,\n  \
            CAST(SUM(CASE WHEN \"amount\" < 0 THEN 1 ELSE 0 END) AS TEXT) AS c10,\n  \
            CAST(MIN(\"amount\") AS TEXT) AS c11,\n  \
            CAST(COUNT(\"amount\") AS TEXT) AS c12,\n  \
            CAST(SUM(CASE WHEN \"amount\" > 10000 THEN 1 ELSE 0 END) AS TEXT) AS c13,\n  \
            CAST(MAX(\"amount\") AS TEXT) AS c14,\n  \
            CAST(COUNT(\"status\") AS TEXT) AS c15,\n  \
            CAST(COUNT(\"status\") AS TEXT) AS c16,\n  \
            CAST(SUM(CASE WHEN \"status\" IS NOT NULL AND \"status\" NOT IN ('new', 'paid', 'shipped') THEN 1 ELSE 0 END) AS TEXT) AS c17,\n  \
            CAST(SUM(CASE WHEN \"order_id\" IS NOT NULL THEN 1 ELSE 0 END) AS TEXT) AS c18,\n  \
            CAST(COUNT(DISTINCT CASE WHEN \"order_id\" IS NOT NULL THEN CAST(\"order_id\" AS TEXT) END) AS TEXT) AS c19,\n  \
            CAST(EXTRACT(EPOCH FROM CAST(MAX(\"created_at\") AS TIMESTAMPTZ)) AS TEXT) AS c20\n\
            FROM \"public\".\"orders\"";
        assert_eq!(sql, expected);
    }

    #[test]
    fn decode_matches_the_streaming_engine_shape() {
        let c = contract(ORDERS);
        let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
        let mut probe = compile(
            &c,
            "\"public\".\"orders\"",
            Dialect::Postgres,
            &orders_columns(),
            &ProbeOptions {
                now,
                profile: false,
                ..ProbeOptions::default()
            },
        );
        // 1000 rows; 3 null ids; 2 bad emails; 1 duplicate email; amount min
        // -5 (2 rows below 0), max 9000; 4 bad statuses; 997 non-null keys, 995
        // distinct; newest row 30h ago.
        let newest = (now.timestamp() - 30 * 3_600).to_string();
        let main = row(&[
            "1000", "997", "3", "990", "990", "2", "990", "989", "1000", "1000", "2", "-5", "1000",
            "0", "9000.50", "1000", "1000", "4", "997", "995", &newest,
        ]);
        let replies = vec![Reply::Rows(vec![main])];
        let follow = probe.follow_ups(&replies);
        let labels: Vec<&str> = follow.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "samples:order_id.required",
                "samples:email.format.email",
                "samples:email.unique",
                "samples:amount.min",
                "samples:status.enum",
                "samples:dataset.primary_key",
                "keys:dataset.primary_key",
            ]
        );
        assert_eq!(
            follow[2].sql,
            "SELECT CAST(\"email\" AS TEXT) AS v FROM \"public\".\"orders\" WHERE \"email\" IS NOT NULL GROUP BY \"email\" HAVING COUNT(*) > 1 LIMIT 5"
        );
        assert_eq!(
            follow[6].sql,
            "SELECT CAST(\"order_id\" AS TEXT) FROM \"public\".\"orders\" WHERE \"order_id\" IS NOT NULL GROUP BY \"order_id\" LIMIT 500001"
        );
        let follow_replies = vec![
            Reply::Rows(vec![row(&["(null)"]), row(&["(null)"]), row(&["(null)"])]),
            Reply::Rows(vec![row(&["nobody"]), row(&["x@y"])]),
            Reply::Rows(vec![row(&["dup@acme.test"])]),
            Reply::Rows(vec![row(&["-5"]), row(&["-1"])]),
            Reply::Rows(vec![row(&["cancelled"])]),
            Reply::Rows(vec![row(&["17"]), row(&["23"])]),
            Reply::Rows((1..=995).map(|i| row(&[&i.to_string()])).collect()),
        ];
        let out = probe.decode(&replies, &follow_replies);
        assert_eq!(out.rows_total, 1000);
        assert_eq!(out.source_columns, 5);
        assert!(out.observed_typed);
        assert_eq!(
            out.observed_columns[2].dtype.as_deref(),
            Some("numeric(12,2)")
        );
        let ids: Vec<&str> = out.checks.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "dataset.columns",
                "order_id.type",
                "order_id.required",
                "email.type",
                "email.format.email",
                "email.unique",
                "amount.type",
                "amount.min",
                "amount.max",
                "status.type",
                "status.enum",
                "dataset.primary_key",
                "dataset.row_count_min",
                "dataset.freshness.created_at",
            ]
        );
        let by_id = |id: &str| out.checks.iter().find(|c| c.id == id).unwrap();

        let columns = by_id("dataset.columns");
        assert!(!columns.failed);
        assert_eq!(columns.observed["extra_columns"], json!(["created_at"]));

        let required = by_id("order_id.required");
        assert!(required.failed);
        assert_eq!(required.rows_evaluated, Some(1000));
        assert_eq!(required.rows_failed, Some(3));
        assert_eq!(
            required.message.as_deref(),
            Some("3 null value(s) in a required column")
        );
        assert_eq!(required.observed["fail_ratio"], json!(0.003));
        assert_eq!(required.samples.len(), 3);
        assert_eq!(required.samples[0], (0, "(null)".to_owned()));

        let typed = by_id("amount.type");
        assert!(!typed.failed);
        assert_eq!(typed.rows_evaluated, Some(1000));
        assert_eq!(typed.rows_failed, Some(0));
        assert_eq!(typed.params, json!({"type": "float"}));

        let email = by_id("email.format.email");
        assert!(email.failed);
        assert_eq!(email.message.as_deref(), Some("2 value(s) are not valid"));
        assert_eq!(email.samples[1].1, "x@y");

        let unique = by_id("email.unique");
        assert!(unique.failed);
        assert_eq!(unique.rows_failed, Some(1));
        assert_eq!(unique.observed["distinct_count"], json!(989));
        assert_eq!(
            unique.message.as_deref(),
            Some("1 duplicate value(s), e.g. \"dup@acme.test\"")
        );

        let min = by_id("amount.min");
        assert!(min.failed);
        assert_eq!(min.observed["min_observed"], json!(-5));
        assert_eq!(min.message.as_deref(), Some("min observed: -5"));
        assert_eq!(min.params, json!({"min": 0}));
        let max = by_id("amount.max");
        assert!(!max.failed);
        assert_eq!(max.observed["max_observed"], json!(9000.5));
        assert!(max.message.is_none());

        let status = by_id("status.enum");
        assert_eq!(
            status.message.as_deref(),
            Some("4 value(s) outside the allowed set")
        );
        assert_eq!(status.params, json!({"enum": ["new", "paid", "shipped"]}));

        let pk = by_id("dataset.primary_key");
        assert!(pk.failed);
        assert_eq!(pk.rows_evaluated, Some(1000));
        assert_eq!(pk.rows_failed, Some(5));
        assert_eq!(pk.observed["distinct_keys"], json!(995));
        assert_eq!(pk.observed["duplicate_rows"], json!(2));
        assert_eq!(pk.observed["null_key_rows"], json!(3));
        assert_eq!(
            pk.message.as_deref(),
            Some("2 repeated key(s), e.g. \"17\"; 3 row(s) with a null in the key")
        );
        let keys = out.primary_key.expect("key set kept");
        assert_eq!(keys.columns, vec!["order_id".to_owned()]);
        assert_eq!(keys.len(), 995);
        assert!(keys.contains(hash_key(["17"])));
        assert!(!keys.contains(hash_key(["1001"])));

        let rc = by_id("dataset.row_count_min");
        assert!(!rc.failed);
        assert_eq!(rc.observed["row_count"], json!(1000));

        let fresh = by_id("dataset.freshness.created_at");
        assert!(!fresh.failed);
        assert_eq!(fresh.observed["age_hours"], json!(30));
        assert_eq!(
            fresh.params,
            json!({"column": "created_at", "max_age": "48h", "max_age_secs": 172800})
        );
    }

    #[test]
    fn text_columns_declared_as_numbers_are_parsed_not_rejected() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  qty:
    type: int
    checks:
      - min: 1
  flag: { type: bool }
  when: { type: datetime }
"#,
        );
        for (dialect, table) in [
            (Dialect::Snowflake, "\"DB\".\"S\".\"T\""),
            (Dialect::Databricks, "`db`.`s`.`t`"),
            (Dialect::BigQuery, "`p.d.t`"),
        ] {
            let probe = compile(
                &c,
                table,
                dialect,
                &[
                    col("qty", "STRING"),
                    col("flag", "STRING"),
                    col("when", "STRING"),
                ],
                &ProbeOptions {
                    profile: false,
                    ..ProbeOptions::default()
                },
            );
            let sql = probe.statements()[0].sql.clone();
            match dialect {
                Dialect::Snowflake => {
                    assert!(
                        sql.contains(r#"NOT (REGEXP_INSTR("qty", '^[-+]?[0-9]+$') > 0)"#),
                        "{sql}"
                    );
                    assert!(sql.contains(r#"TRY_CAST("qty" AS DOUBLE) < 1"#), "{sql}");
                }
                Dialect::Databricks => {
                    assert!(sql.contains("(`qty` RLIKE '^[-+]?[0-9]+$')"), "{sql}");
                    assert!(sql.contains("TRY_CAST(`qty` AS DOUBLE) < 1"), "{sql}");
                }
                Dialect::BigQuery => {
                    assert!(
                        sql.contains("REGEXP_CONTAINS(`qty`, r'^[-+]?[0-9]+$')"),
                        "{sql}"
                    );
                    assert!(sql.contains("SAFE_CAST(`qty` AS FLOAT64) < 1"), "{sql}");
                }
                Dialect::Postgres => unreachable!(),
            }
            assert!(
                sql.contains("NOT IN ('true', 't', '1', 'yes', 'y', 'false', 'f', '0', 'no', 'n')")
            );
        }
        // Postgres, which has no safe cast, guards the cast with the shape.
        let probe = compile(
            &c,
            "\"public\".\"t\"",
            Dialect::Postgres,
            &[col("qty", "text"), col("flag", "text"), col("when", "text")],
            &ProbeOptions {
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let sql = probe.statements()[0].sql.clone();
        assert!(sql.contains(
            r#"(CASE WHEN ("qty" ~ '^[-+]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][-+]?[0-9]+)?$') THEN CAST("qty" AS DOUBLE PRECISION) END) < 1"#
        ), "{sql}");
    }

    #[test]
    fn structural_mismatch_and_uncomparable_checks_do_not_pass_quietly() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  id: { type: string }
  payload:
    type: string
    checks:
      - min: 3
  gone: { type: int }
"#,
        );
        let probe = compile(
            &c,
            "\"public\".\"t\"",
            Dialect::Postgres,
            &[col("id", "bigint"), col("payload", "jsonb")],
            &ProbeOptions::default(),
        );
        // The profile never asks Postgres to order or compare json.
        let sql = probe.statements()[0].sql.clone();
        assert!(sql.contains("COUNT(DISTINCT CAST(\"payload\" AS TEXT))"));
        assert!(!sql.contains("MIN(\"payload\")"));
        let main = row(&[
            "10", "10", "10", "10", "10", "0", "0", "10", "10", "9", "0", "5",
        ]);
        let out = probe.decode(&[Reply::Rows(vec![main])], &[]);
        let by_id = |id: &str| out.checks.iter().find(|c| c.id == id).unwrap();
        let id = by_id("id.type");
        assert!(id.failed);
        assert_eq!(id.rows_evaluated, None);
        assert_eq!(
            id.message.as_deref(),
            Some("column has the wrong type (structural mismatch)")
        );
        assert_eq!(id.observed["observed_type"], json!("bigint"));
        let min = by_id("payload.min");
        assert!(min.failed);
        assert_eq!(
            min.message.as_deref(),
            Some("check could not run: a non-scalar column cannot be compared with a number in the warehouse")
        );
        let gone = by_id("gone.present");
        assert!(gone.failed);
        assert_eq!(gone.kind, "column_present");
    }

    #[test]
    fn expressions_run_on_their_own_and_fail_alone() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  amount: { type: float }
  discount:
    type: float
    checks:
      - custom_expr: "CAST(discount AS DOUBLE) <= CAST(amount AS DOUBLE)"
dataset_checks:
  - assert: "SUM(amount) > 0"
  - row_count_max: 5000
"#,
        );
        let probe = compile(
            &c,
            "`p.d.t`",
            Dialect::BigQuery,
            &[col("amount", "FLOAT64"), col("discount", "FLOAT64")],
            &ProbeOptions {
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let statements = probe.statements();
        assert_eq!(statements.len(), 3);
        assert_eq!(statements[1].label, "expr:discount.custom_expr");
        assert_eq!(
            statements[1].sql,
            "SELECT CAST(COUNT(*) AS STRING) AS n, CAST(SUM(CASE WHEN COALESCE((CAST(discount AS FLOAT64) <= CAST(amount AS FLOAT64)), FALSE) THEN 0 ELSE 1 END) AS STRING) AS f FROM `p.d.t`"
        );
        assert_eq!(
            statements[2].sql,
            "SELECT CAST((SUM(amount) > 0) AS STRING) AS v, CAST(COUNT(*) AS STRING) AS n FROM `p.d.t`"
        );
        let replies = vec![
            Reply::Rows(vec![row(&["120", "120", "120"])]),
            Reply::Rows(vec![row(&["120", "7"])]),
            Reply::Error("Unrecognized name: amount at [1:13]".to_owned()),
        ];
        let out = probe.decode(&replies, &[]);
        let by_id = |id: &str| out.checks.iter().find(|c| c.id == id).unwrap();
        let ce = by_id("discount.custom_expr");
        assert!(ce.failed);
        assert_eq!(ce.rows_failed, Some(7));
        assert_eq!(
            ce.message.as_deref(),
            Some("7 row(s) failed `CAST(discount AS DOUBLE) <= CAST(amount AS DOUBLE)`")
        );
        let assert = by_id("dataset.assert");
        assert!(assert.failed);
        assert_eq!(
            assert.message.as_deref(),
            Some("check could not run: Unrecognized name: amount at [1:13]")
        );
        // The aggregate still answered, so the rest of the run stands.
        assert!(!by_id("dataset.row_count_max").failed);
        assert_eq!(
            by_id("dataset.row_count_max").observed["row_count"],
            json!(120)
        );
    }

    #[test]
    fn assert_verdicts_true_false_null() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  n: { type: int }
dataset_checks:
  - assert: "AVG(n) > 1"
"#,
        );
        for (cell, failed, needle) in [
            ("true", false, ""),
            ("false", true, "is false over 3 row(s)"),
            ("NULL", true, "came out null over 3 row(s)"),
        ] {
            let probe = compile(
                &c,
                "\"s\".\"t\"",
                Dialect::Postgres,
                &[col("n", "integer")],
                &ProbeOptions {
                    profile: false,
                    ..ProbeOptions::default()
                },
            );
            let out = probe.decode(
                &[
                    Reply::Rows(vec![row(&["3", "3"])]),
                    Reply::Rows(vec![row(&[cell, "3"])]),
                ],
                &[],
            );
            let a = &out.checks[2];
            assert_eq!(a.id, "dataset.assert");
            assert_eq!(a.failed, failed, "{cell}");
            assert_eq!(a.observed["rows"], json!(3));
            if !needle.is_empty() {
                assert!(a.message.as_deref().unwrap().contains(needle), "{cell}");
            }
        }
    }

    #[test]
    fn references_are_looked_up_against_the_recorded_keys() {
        let c = contract(
            r#"
apiVersion: v1
dataset: orders
owner: o@x.test
columns:
  customer_id: { type: int }
dataset_checks:
  - references: { columns: [customer_id], dataset: customers, to: [id] }
"#,
        );
        let mut refs = ReferenceSets::none();
        refs.insert(
            "customers",
            KeySet::from_hashes(
                vec!["id".to_owned()],
                ["1", "2", "3"].into_iter().map(|k| hash_key([k])),
            ),
        );
        let mut probe = compile(
            &c,
            "\"public\".\"orders\"",
            Dialect::Postgres,
            &[col("customer_id", "integer")],
            &ProbeOptions {
                references: refs.clone(),
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let replies = vec![Reply::Rows(vec![row(&["10", "10", "9"])])];
        let follow = probe.follow_ups(&replies);
        assert_eq!(follow.len(), 1);
        assert_eq!(
            follow[0].sql,
            "SELECT CAST(\"customer_id\" AS TEXT), CAST(COUNT(*) AS TEXT) AS n FROM \"public\".\"orders\" WHERE \"customer_id\" IS NOT NULL GROUP BY \"customer_id\" LIMIT 500001"
        );
        let groups = Reply::Rows(vec![
            row(&["1", "4"]),
            row(&["2", "2"]),
            row(&["9", "2"]),
            row(&["3", "1"]),
        ]);
        let out = probe.decode(&replies, &[groups]);
        let r = &out.checks[2];
        assert_eq!(r.id, "dataset.references.customers");
        assert!(r.failed);
        assert_eq!(r.rows_evaluated, Some(9));
        assert_eq!(r.rows_failed, Some(2));
        assert_eq!(r.observed["referenced_keys"], json!(3));
        assert_eq!(r.observed["rows_skipped_null"], json!(1));
        assert_eq!(
            r.message.as_deref(),
            Some("2 row(s) point at keys `customers` does not have, e.g. \"9\"")
        );

        // Nothing recorded for the other dataset: the check says so.
        let probe = compile(
            &c,
            "\"public\".\"orders\"",
            Dialect::Postgres,
            &[col("customer_id", "integer")],
            &ProbeOptions {
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let out = probe.decode(&[Reply::Rows(vec![row(&["10"])])], &[]);
        assert!(out.checks[2].failed);
        assert!(out.checks[2]
            .message
            .as_deref()
            .unwrap()
            .starts_with("check could not run: no keys are recorded for `customers`"));
    }

    #[test]
    fn profile_is_read_from_the_same_scan() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  plan: { type: string }
"#,
        );
        let mut probe = compile(
            &c,
            "\"public\".\"t\"",
            Dialect::Postgres,
            &[
                col("plan", "text"),
                col("age", "integer"),
                col("blob", "bytea"),
            ],
            &ProbeOptions::default(),
        );
        let sql = probe.statements()[0].sql.clone();
        assert!(sql.contains("CAST(AVG(\"age\") AS TEXT)"));
        assert!(sql.contains("CAST(STDDEV_SAMP(\"age\") AS TEXT)"));
        assert!(sql.contains("CAST(MIN(LENGTH(\"plan\")) AS TEXT)"));
        assert!(!sql.contains("MIN(\"blob\")"));
        // c0 rows, c1 COUNT(plan) for the type check, then the profile:
        // plan: nonnull, distinct, min, max, minlen, maxlen
        // age:  nonnull, distinct, min, max, avg, stddev
        // blob: nonnull, distinct
        let main = row(&[
            "400",
            "400",
            "400",
            "3",
            "enterprise",
            "pro",
            "3",
            "10",
            "394",
            "47",
            "18",
            "64",
            "40.5",
            "12.25",
            "400",
            "400",
        ]);
        let replies = vec![Reply::Rows(vec![main])];
        let follow = probe.follow_ups(&replies);
        let labels: Vec<&str> = follow.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["values:plan"]);
        assert_eq!(
            follow[0].sql,
            "SELECT CAST(\"plan\" AS TEXT) AS v FROM \"public\".\"t\" WHERE \"plan\" IS NOT NULL GROUP BY \"plan\" LIMIT 26"
        );
        let out = probe.decode(
            &replies,
            &[Reply::Rows(vec![
                row(&["pro"]),
                row(&["enterprise"]),
                row(&["free"]),
            ])],
        );
        let profile = out.profile.expect("profiled");
        assert_eq!(profile.len(), 3);
        let plan = &profile[0];
        assert_eq!(plan.distinct, 3);
        assert_eq!(plan.values, vec!["enterprise", "free", "pro"]);
        assert!(plan.values_complete);
        assert_eq!(plan.min_length, Some(3));
        assert_eq!(plan.max_length, Some(10));
        let age = &profile[1];
        assert_eq!(age.null_count, 6);
        assert!((age.null_ratio - 0.015).abs() < 1e-9);
        assert_eq!(age.mean, Some(40.5));
        assert_eq!(age.std, Some(12.25));
        assert_eq!(age.min.as_deref(), Some("18"));
        assert!(age.values.is_empty());
        assert!(!age.values_complete);
    }

    #[test]
    fn a_failed_scan_fails_every_check_loudly() {
        let c = contract(ORDERS);
        let probe = compile(
            &c,
            "\"public\".\"orders\"",
            Dialect::Postgres,
            &orders_columns(),
            &ProbeOptions::default(),
        );
        let out = probe.decode(
            &[Reply::Error(
                "permission denied for table orders".to_owned(),
            )],
            &[],
        );
        assert_eq!(out.rows_total, 0);
        assert!(out.profile.is_none());
        let required = out
            .checks
            .iter()
            .find(|c| c.id == "order_id.required")
            .unwrap();
        assert!(required.failed);
        assert_eq!(
            required.message.as_deref(),
            Some("check could not run: permission denied for table orders")
        );
        // Schema facts do not need the scan.
        let columns = out
            .checks
            .iter()
            .find(|c| c.id == "dataset.columns")
            .unwrap();
        assert!(!columns.failed);
    }

    #[test]
    fn composite_keys_and_snowflake_identifiers() {
        let c = contract(
            r#"
apiVersion: v1
dataset: t
owner: o@x.test
columns:
  a: { type: int }
  b: { type: string }
primary_key: [a, b]
"#,
        );
        let probe = compile(
            &c,
            "\"DB\".\"S\".\"T\"",
            Dialect::Snowflake,
            &[col("a", "NUMBER(38,0)"), col("b", "VARCHAR(16777216)")],
            &ProbeOptions {
                profile: false,
                ..ProbeOptions::default()
            },
        );
        let sql = probe.statements()[0].sql.clone();
        assert!(sql.contains(
            "COUNT(DISTINCT CASE WHEN \"a\" IS NOT NULL AND \"b\" IS NOT NULL THEN CONCAT(CAST(\"a\" AS VARCHAR), '\u{1f}', CAST(\"b\" AS VARCHAR)) END)"
        ), "{sql}");
    }
}
