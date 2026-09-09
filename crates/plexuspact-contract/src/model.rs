//! Typed data-contract model (architecture doc 03 §4).
//!
//! These types are *declarative descriptions* of checks. No execution logic
//! lives here — only `plexuspact-engine` knows how to run a check (ADR-002).
//!
//! The serde implementations accept both check forms from the YAML surface:
//! bare strings (`unique`, `not_empty_string`) and single-key maps
//! (`{ min: 18 }`, `{ min: 18, severity: warn }`). Serialization round-trips:
//! `serialize → parse → equal` holds for every value.

use std::fmt;
use std::time::Duration;

use indexmap::IndexMap;
use schemars::gen::SchemaGenerator;
use schemars::schema::Schema;
use schemars::JsonSchema;
use serde::de::{Error as DeError, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::suggest::did_you_mean;

/// Severity of a failed check: `error` fails the run, `warn` is reported only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The check failing makes the run fail (exit code 1). Default.
    #[default]
    Error,
    /// The check failing is reported but does not fail the run (unless `--strict`).
    Warn,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => f.write_str("error"),
            Severity::Warn => f.write_str("warn"),
        }
    }
}

/// Contract schema version. Only `v1` exists today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ApiVersion {
    /// Contract schema version 1 (`apiVersion: v1`).
    #[serde(rename = "v1")]
    V1,
}

/// Column data type as declared in the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ColType {
    /// UTF-8 text.
    String,
    /// 64-bit signed integer.
    Int,
    /// 64-bit float.
    Float,
    /// Boolean.
    Bool,
    /// Calendar date (no time component).
    Date,
    /// Date and time.
    Datetime,
}

impl ColType {
    /// `true` for types on which `min`/`max` checks are meaningful
    /// (numeric and temporal types).
    pub fn is_ordered(self) -> bool {
        matches!(
            self,
            ColType::Int | ColType::Float | ColType::Date | ColType::Datetime
        )
    }

    /// `true` for `date` and `datetime`.
    pub fn is_temporal(self) -> bool {
        matches!(self, ColType::Date | ColType::Datetime)
    }
}

impl fmt::Display for ColType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ColType::String => "string",
            ColType::Int => "int",
            ColType::Float => "float",
            ColType::Bool => "bool",
            ColType::Date => "date",
            ColType::Datetime => "datetime",
        };
        f.write_str(s)
    }
}

/// How much a consumer is entitled to rely on a column.
///
/// The contract already says what a column *is*. This says how long it can be
/// counted on to stay that way, which is the question a consumer has to answer
/// before building anything on it — and which, until now, they could only
/// answer by asking somebody.
///
/// It also gives a supplier the one move they did not have: retiring a column
/// on purpose. Removing a column has always been a breaking change and always
/// will be. Announcing months ahead that it is going, with a date, turns that
/// breaking change from something that happens *to* consumers into something
/// they were given time to prepare for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Stability {
    /// The default, and what every column without the field means. It will not
    /// be removed or retyped without the notice a breaking change gets.
    #[default]
    Stable,
    /// Offered, not promised. Read it if you like; it may move without the
    /// ceremony a stable column gets.
    Beta,
    /// On its way out. `sunset` says when.
    Deprecated,
}

impl Stability {
    /// Ordering by how much is promised: stable promises most, deprecated least.
    ///
    /// Used by the diff, where the direction is the whole story — weakening a
    /// promise is a change consumers must read, and strengthening one is not.
    fn rank(self) -> u8 {
        match self {
            Stability::Stable => 0,
            Stability::Beta => 1,
            Stability::Deprecated => 2,
        }
    }

    /// Whether this is the value a contract that says nothing means.
    fn is_default(&self) -> bool {
        *self == Stability::Stable
    }

    /// Compares how much two levels promise. `Greater` means *more* promised.
    pub fn promises_more_than(self, other: Self) -> std::cmp::Ordering {
        other.rank().cmp(&self.rank())
    }
}

impl fmt::Display for Stability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stability::Stable => "stable",
            Stability::Beta => "beta",
            Stability::Deprecated => "deprecated",
        })
    }
}

/// What a consumer *is*.
///
/// "Three consumers" is a number. "A tier-1 dashboard, a model and an internal
/// report" is a decision, and it is a different decision in each case: a
/// dashboard that goes blank is embarrassing on a Monday morning, a model that
/// silently retrains on a changed column is wrong for a quarter, and an agent
/// reading the dataset over an API has nobody watching it at all.
///
/// The engine never reads this — a kind changes no verdict. It changes what
/// the sentence says when somebody has to decide whether to approve a
/// tightening today or wait until the window closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerKind {
    /// A chart somebody looks at. Breaks visibly, and to an audience.
    Dashboard,
    /// A trained or scheduled model. Breaks quietly, and keeps running.
    Model,
    /// A service serving this data onward. Its own consumers are not in here.
    Api,
    /// A scheduled job. Usually fails loudly, at 3am.
    Pipeline,
    /// An autonomous workload reading this without a person in the loop.
    AiAgent,
    /// A document produced on a schedule. Wrong quietly, and circulated.
    Report,
}

impl fmt::Display for ConsumerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConsumerKind::Dashboard => "dashboard",
            ConsumerKind::Model => "model",
            ConsumerKind::Api => "api",
            ConsumerKind::Pipeline => "pipeline",
            ConsumerKind::AiAgent => "AI agent",
            ConsumerKind::Report => "report",
        })
    }
}

/// The lowest tier number, and the most critical.
pub const TIER_MOST_CRITICAL: u8 = 1;
/// The highest tier number, and the least critical.
pub const TIER_LEAST_CRITICAL: u8 = 3;

/// Kind of personally identifiable information stored in a column.
///
/// Reserved field (PRD FR-11): parsed, validated, and echoed into results —
/// no engine behavior until Phase 2 (blast radius, compliance evidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PiiKind {
    /// Explicitly not PII.
    None,
    /// Email addresses.
    Email,
    /// Phone numbers.
    Phone,
    /// Personal names.
    Name,
    /// Physical addresses.
    Address,
    /// Government identifiers (SSN, Aadhaar, …).
    NationalId,
    /// Financial data (account/card numbers, balances).
    Financial,
    /// Health data.
    Health,
    /// PII not covered by the other kinds.
    Other,
}

/// Data classification level of a column.
///
/// Reserved field (PRD FR-11) — no engine behavior in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    /// May be shared publicly.
    Public,
    /// Internal use only.
    Internal,
    /// Restricted to specific teams.
    Confidential,
    /// Highest sensitivity.
    Restricted,
}

impl fmt::Display for DataClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DataClass::Public => "public",
            DataClass::Internal => "internal",
            DataClass::Confidential => "confidential",
            DataClass::Restricted => "restricted",
        })
    }
}

/// Built-in value formats accepted by the `format` check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KnownFormat {
    /// RFC-5322-ish email address.
    Email,
    /// RFC 4122 UUID.
    Uuid,
    /// ISO-8601 date (`2026-07-09`).
    IsoDate,
    /// ISO-8601 date-time (`2026-07-09T10:11:12Z`).
    IsoDatetime,
    /// Absolute URL.
    Url,
    /// ISO-3166 alpha-2 country code (`DE`, `IN`).
    CountryCodeIso2,
}

/// Names of [`KnownFormat`] values as they appear in YAML.
pub(crate) const FORMAT_NAMES: &[&str] = &[
    "email",
    "uuid",
    "iso_date",
    "iso_datetime",
    "url",
    "country_code_iso2",
];

/// A YAML number: integer or float. `min: 18` parses as [`Number::Int`],
/// `min: 18.5` as [`Number::Float`], and each serializes back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Number {
    /// An integer literal.
    Int(i64),
    /// A float literal.
    Float(f64),
}

impl Number {
    /// Numeric value as `f64` for comparisons (lossy for very large ints).
    pub fn as_f64(self) -> f64 {
        match self {
            Number::Int(i) => i as f64,
            Number::Float(f) => f,
        }
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Number::Int(i) => write!(f, "{i}"),
            Number::Float(x) => write!(f, "{x}"),
        }
    }
}

/// A scalar value allowed inside an `enum` check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnumValue {
    /// Boolean literal.
    Bool(bool),
    /// Integer literal.
    Int(i64),
    /// Float literal.
    Float(f64),
    /// String literal.
    String(String),
}

impl fmt::Display for EnumValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnumValue::Bool(b) => write!(f, "{b}"),
            EnumValue::Int(i) => write!(f, "{i}"),
            EnumValue::Float(x) => write!(f, "{x}"),
            EnumValue::String(s) => write!(f, "{s}"),
        }
    }
}

/// String-length constraint: exact (`length: 2`) or a range
/// (`length: { min: 1, max: 10 }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum LengthSpec {
    /// Exact length in characters.
    Exact(u64),
    /// Inclusive min/max bounds; either side may be omitted.
    Range(LengthRange),
}

impl LengthSpec {
    /// Normalized `(min, max)` bounds; `Exact(n)` is `(n, Some(n))` and a
    /// missing `min` is `0`, a missing `max` is `None` (unbounded).
    pub fn bounds(self) -> (u64, Option<u64>) {
        match self {
            LengthSpec::Exact(n) => (n, Some(n)),
            LengthSpec::Range(r) => (r.min.unwrap_or(0), r.max),
        }
    }
}

/// Inclusive length range for [`LengthSpec::Range`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LengthRange {
    /// Minimum length (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<u64>,
    /// Maximum length (inclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<u64>,
}

/// Options accepted by the map form of `unique` (`{ unique: { approx: true } }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UniqueOpts {
    #[serde(default)]
    approx: bool,
}

/// A single column-level check.
///
/// Two YAML surface forms are accepted:
///
/// * bare string — `unique`, `not_empty_string`
/// * single-key map — `{ min: 18 }`, `{ min: 18, severity: warn }`,
///   `{ unique: { approx: true } }`, …
///
/// Every map form accepts an optional `severity` key (default `error`).
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnCheck {
    /// All non-null values must be distinct.
    Unique {
        /// Use approximate (HyperLogLog) distinct counting instead of exact hashing.
        approx: bool,
        /// Failure severity.
        severity: Severity,
    },
    /// String values must not be empty (`""`).
    NotEmptyString {
        /// Failure severity.
        severity: Severity,
    },
    /// Values must be `>= min`.
    Min {
        /// Inclusive lower bound.
        min: Number,
        /// Failure severity.
        severity: Severity,
    },
    /// Values must be `<= max`.
    Max {
        /// Inclusive upper bound.
        max: Number,
        /// Failure severity.
        severity: Severity,
    },
    /// String values must match the regular expression.
    Regex {
        /// Rust `regex` syntax pattern.
        regex: String,
        /// Failure severity.
        severity: Severity,
    },
    /// Values must be one of the listed values.
    Enum {
        /// Allowed values.
        values: Vec<EnumValue>,
        /// Failure severity.
        severity: Severity,
    },
    /// String length constraint.
    Length {
        /// Exact length or min/max range.
        length: LengthSpec,
        /// Failure severity.
        severity: Severity,
    },
    /// Values must match a built-in format.
    Format {
        /// The named format.
        format: KnownFormat,
        /// Failure severity.
        severity: Severity,
    },
    /// Null ratio of the column must be `<= ratio`.
    NullRatioMax {
        /// Maximum allowed null ratio in `[0, 1]`.
        ratio: f64,
        /// Failure severity.
        severity: Severity,
    },
    /// Distinct-value ratio of the column must be `>= ratio`.
    UniqueRatioMin {
        /// Minimum required unique ratio in `[0, 1]`.
        ratio: f64,
        /// Failure severity.
        severity: Severity,
    },
    /// Escape hatch: a Polars expression evaluated by the engine.
    CustomExpr {
        /// The expression source text.
        expr: String,
        /// Failure severity.
        severity: Severity,
    },
}

/// Names of all column checks as they appear in YAML.
pub(crate) const COLUMN_CHECK_NAMES: &[&str] = &[
    "unique",
    "not_empty_string",
    "min",
    "max",
    "regex",
    "enum",
    "length",
    "format",
    "null_ratio_max",
    "unique_ratio_min",
    "custom_expr",
];

impl ColumnCheck {
    /// The check's failure severity.
    pub fn severity(&self) -> Severity {
        match self {
            ColumnCheck::Unique { severity, .. }
            | ColumnCheck::NotEmptyString { severity }
            | ColumnCheck::Min { severity, .. }
            | ColumnCheck::Max { severity, .. }
            | ColumnCheck::Regex { severity, .. }
            | ColumnCheck::Enum { severity, .. }
            | ColumnCheck::Length { severity, .. }
            | ColumnCheck::Format { severity, .. }
            | ColumnCheck::NullRatioMax { severity, .. }
            | ColumnCheck::UniqueRatioMin { severity, .. }
            | ColumnCheck::CustomExpr { severity, .. } => *severity,
        }
    }

    /// The YAML name of this check kind (`"min"`, `"unique"`, …).
    pub fn kind_name(&self) -> &'static str {
        match self {
            ColumnCheck::Unique { .. } => "unique",
            ColumnCheck::NotEmptyString { .. } => "not_empty_string",
            ColumnCheck::Min { .. } => "min",
            ColumnCheck::Max { .. } => "max",
            ColumnCheck::Regex { .. } => "regex",
            ColumnCheck::Enum { .. } => "enum",
            ColumnCheck::Length { .. } => "length",
            ColumnCheck::Format { .. } => "format",
            ColumnCheck::NullRatioMax { .. } => "null_ratio_max",
            ColumnCheck::UniqueRatioMin { .. } => "unique_ratio_min",
            ColumnCheck::CustomExpr { .. } => "custom_expr",
        }
    }
}

/// A YAML example of the map form for the named check, used in error hints.
fn check_example(name: &str) -> &'static str {
    match name {
        "min" => "{ min: 18 }",
        "max" => "{ max: 120 }",
        "regex" => r#"{ regex: "^[A-Z]{2}$" }"#,
        "enum" => "{ enum: [free, pro, enterprise] }",
        "length" => "{ length: 2 } or { length: { min: 1, max: 10 } }",
        "format" => "{ format: email }",
        "null_ratio_max" => "{ null_ratio_max: 0.1 }",
        "unique_ratio_min" => "{ unique_ratio_min: 0.9 }",
        "custom_expr" => r#"{ custom_expr: "age >= 0" }"#,
        "unique" => "unique or { unique: { approx: true } }",
        "not_empty_string" => "not_empty_string",
        _ => "{ min: 18 }",
    }
}

/// Renders a YAML value for inclusion in an error message.
fn value_repr(value: &serde_yaml::Value) -> String {
    serde_yaml::to_string(value)
        .map(|s| s.trim_end().to_string())
        .unwrap_or_else(|_| "<value>".to_string())
}

/// Deserializes `value` as `T`, mapping failures to a message that names the
/// check, states what was expected, and shows an example fix.
fn typed_value<T: serde::de::DeserializeOwned>(
    check: &str,
    expected: &str,
    value: serde_yaml::Value,
) -> Result<T, String> {
    let repr = value_repr(&value);
    serde_yaml::from_value(value).map_err(|_| {
        format!(
            "invalid value `{repr}` for check `{check}`: expected {expected}, e.g. `{}`",
            check_example(check)
        )
    })
}

/// Error message for an unknown check name, with a "did you mean" hint.
fn unknown_check_message(name: &str, known: &[&str]) -> String {
    let mut msg = format!("unknown check `{name}`");
    if let Some(suggestion) = did_you_mean(name, known.iter().copied()) {
        msg.push_str(&format!(" — did you mean `{suggestion}`?"));
    }
    msg.push_str(&format!(" (known checks: {})", known.join(", ")));
    msg
}

/// Parses a `severity` value, with a helpful message on failure.
fn parse_severity(value: serde_yaml::Value) -> Result<Severity, String> {
    let repr = value_repr(&value);
    serde_yaml::from_value(value).map_err(|_| {
        format!("invalid severity `{repr}`: expected `error` or `warn`, e.g. `{{ min: 18, severity: warn }}`")
    })
}

/// Builds a [`ColumnCheck`] from a map-form key/value pair.
fn build_column_check(
    name: &str,
    value: serde_yaml::Value,
    severity: Severity,
) -> Result<ColumnCheck, String> {
    match name {
        "min" => Ok(ColumnCheck::Min {
            min: typed_value(name, "a number", value)?,
            severity,
        }),
        "max" => Ok(ColumnCheck::Max {
            max: typed_value(name, "a number", value)?,
            severity,
        }),
        "regex" => Ok(ColumnCheck::Regex {
            regex: typed_value(name, "a pattern string", value)?,
            severity,
        }),
        "enum" => Ok(ColumnCheck::Enum {
            values: typed_value(name, "a list of allowed values", value)?,
            severity,
        }),
        "length" => Ok(ColumnCheck::Length {
            length: typed_value(
                name,
                "an exact length or a `{ min, max }` range of non-negative integers",
                value,
            )?,
            severity,
        }),
        "format" => {
            if let serde_yaml::Value::String(s) = &value {
                match serde_yaml::from_value::<KnownFormat>(value.clone()) {
                    Ok(format) => Ok(ColumnCheck::Format { format, severity }),
                    Err(_) => {
                        let mut msg = format!("unknown format `{s}`");
                        if let Some(suggestion) = did_you_mean(s, FORMAT_NAMES.iter().copied()) {
                            msg.push_str(&format!(" — did you mean `{suggestion}`?"));
                        }
                        msg.push_str(&format!(" (known formats: {})", FORMAT_NAMES.join(", ")));
                        Err(msg)
                    }
                }
            } else {
                Err(format!(
                    "invalid value `{}` for check `format`: expected a format name, e.g. `{{ format: email }}`",
                    value_repr(&value)
                ))
            }
        }
        "null_ratio_max" => Ok(ColumnCheck::NullRatioMax {
            ratio: typed_value(name, "a ratio between 0 and 1", value)?,
            severity,
        }),
        "unique_ratio_min" => Ok(ColumnCheck::UniqueRatioMin {
            ratio: typed_value(name, "a ratio between 0 and 1", value)?,
            severity,
        }),
        "custom_expr" => Ok(ColumnCheck::CustomExpr {
            expr: typed_value(name, "an expression string", value)?,
            severity,
        }),
        "unique" => match value {
            serde_yaml::Value::Null | serde_yaml::Value::Bool(true) => Ok(ColumnCheck::Unique {
                approx: false,
                severity,
            }),
            serde_yaml::Value::Mapping(_) => {
                let opts: UniqueOpts =
                    typed_value(name, "`true` or `{ approx: true }`", value)?;
                Ok(ColumnCheck::Unique {
                    approx: opts.approx,
                    severity,
                })
            }
            other => Err(format!(
                "invalid value `{}` for check `unique`: expected `true` or `{{ approx: true }}`; to disable the check remove it from the list",
                value_repr(&other)
            )),
        },
        "not_empty_string" => match value {
            serde_yaml::Value::Null | serde_yaml::Value::Bool(true) => {
                Ok(ColumnCheck::NotEmptyString { severity })
            }
            other => Err(format!(
                "invalid value `{}` for check `not_empty_string`: expected `true`; to disable the check remove it from the list",
                value_repr(&other)
            )),
        },
        other => Err(unknown_check_message(other, COLUMN_CHECK_NAMES)),
    }
}

impl<'de> Deserialize<'de> for ColumnCheck {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(ColumnCheckVisitor)
    }
}

struct ColumnCheckVisitor;

impl<'de> Visitor<'de> for ColumnCheckVisitor {
    type Value = ColumnCheck;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a check name like `unique` or a single-key map like `{ min: 18 }`")
    }

    fn visit_str<E: DeError>(self, s: &str) -> Result<ColumnCheck, E> {
        match s {
            "unique" => Ok(ColumnCheck::Unique {
                approx: false,
                severity: Severity::default(),
            }),
            "not_empty_string" => Ok(ColumnCheck::NotEmptyString {
                severity: Severity::default(),
            }),
            other if COLUMN_CHECK_NAMES.contains(&other) => Err(E::custom(format!(
                "check `{other}` requires a value; write it in map form, e.g. `{}`",
                check_example(other)
            ))),
            other => Err(E::custom(unknown_check_message(other, COLUMN_CHECK_NAMES))),
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ColumnCheck, A::Error> {
        let mut severity: Option<Severity> = None;
        let mut check: Option<(String, serde_yaml::Value)> = None;
        while let Some(key) = map.next_key::<String>()? {
            let value: serde_yaml::Value = map.next_value()?;
            if key == "severity" {
                severity = Some(parse_severity(value).map_err(A::Error::custom)?);
            } else if let Some((first, _)) = &check {
                return Err(A::Error::custom(format!(
                    "a check map must contain exactly one check key, found `{first}` and `{key}`; write them as separate list items: `[{{ {first}: … }}, {{ {key}: … }}]`"
                )));
            } else {
                check = Some((key, value));
            }
        }
        let (name, value) = check.ok_or_else(|| {
            A::Error::custom(
                "a check map needs a check key, e.g. `{ min: 18 }` (severity alone is not a check)",
            )
        })?;
        build_column_check(&name, value, severity.unwrap_or_default()).map_err(A::Error::custom)
    }
}

impl Serialize for ColumnCheck {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Bare-string forms when there is nothing else to say.
        match self {
            ColumnCheck::Unique {
                approx: false,
                severity: Severity::Error,
            } => return serializer.serialize_str("unique"),
            ColumnCheck::NotEmptyString {
                severity: Severity::Error,
            } => return serializer.serialize_str("not_empty_string"),
            _ => {}
        }
        let severity = self.severity();
        let entries = 1 + usize::from(severity == Severity::Warn);
        let mut map = serializer.serialize_map(Some(entries))?;
        match self {
            ColumnCheck::Unique { approx: true, .. } => {
                map.serialize_entry("unique", &UniqueOpts { approx: true })?;
            }
            ColumnCheck::Unique { approx: false, .. } => {
                map.serialize_entry("unique", &true)?;
            }
            ColumnCheck::NotEmptyString { .. } => {
                map.serialize_entry("not_empty_string", &true)?;
            }
            ColumnCheck::Min { min, .. } => map.serialize_entry("min", min)?,
            ColumnCheck::Max { max, .. } => map.serialize_entry("max", max)?,
            ColumnCheck::Regex { regex, .. } => map.serialize_entry("regex", regex)?,
            ColumnCheck::Enum { values, .. } => map.serialize_entry("enum", values)?,
            ColumnCheck::Length { length, .. } => map.serialize_entry("length", length)?,
            ColumnCheck::Format { format, .. } => map.serialize_entry("format", format)?,
            ColumnCheck::NullRatioMax { ratio, .. } => {
                map.serialize_entry("null_ratio_max", ratio)?;
            }
            ColumnCheck::UniqueRatioMin { ratio, .. } => {
                map.serialize_entry("unique_ratio_min", ratio)?;
            }
            ColumnCheck::CustomExpr { expr, .. } => map.serialize_entry("custom_expr", expr)?,
        }
        if severity == Severity::Warn {
            map.serialize_entry("severity", &Severity::Warn)?;
        }
        map.end()
    }
}

impl JsonSchema for ColumnCheck {
    fn schema_name() -> String {
        "ColumnCheck".to_string()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let severity = schema_to_value(generator.subschema_for::<Severity>());
        let number = schema_to_value(generator.subschema_for::<Number>());
        let enum_value = schema_to_value(generator.subschema_for::<EnumValue>());
        let length = schema_to_value(generator.subschema_for::<LengthSpec>());
        let format = schema_to_value(generator.subschema_for::<KnownFormat>());
        let ratio = serde_json::json!({ "type": "number", "minimum": 0.0, "maximum": 1.0 });
        let string = serde_json::json!({ "type": "string" });
        let unique_value = serde_json::json!({
            "anyOf": [
                { "type": "boolean" },
                {
                    "type": "object",
                    "properties": { "approx": { "type": "boolean" } },
                    "additionalProperties": false
                }
            ]
        });
        let forms = vec![
            serde_json::json!({
                "type": "string",
                "enum": ["unique", "not_empty_string"]
            }),
            map_form_schema("min", &number, &severity),
            map_form_schema("max", &number, &severity),
            map_form_schema("regex", &string, &severity),
            map_form_schema(
                "enum",
                &serde_json::json!({ "type": "array", "items": enum_value }),
                &severity,
            ),
            map_form_schema("length", &length, &severity),
            map_form_schema("format", &format, &severity),
            map_form_schema("null_ratio_max", &ratio, &severity),
            map_form_schema("unique_ratio_min", &ratio, &severity),
            map_form_schema("custom_expr", &string, &severity),
            map_form_schema("unique", &unique_value, &severity),
            map_form_schema(
                "not_empty_string",
                &serde_json::json!({ "type": "boolean" }),
                &severity,
            ),
        ];
        value_to_schema(serde_json::json!({ "anyOf": forms }))
    }
}

/// Serializes a schemars [`Schema`] to a JSON value (infallible in practice).
fn schema_to_value(schema: Schema) -> serde_json::Value {
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Bool(true))
}

/// Parses a JSON value into a schemars [`Schema`]; the literals used here are
/// static and covered by tests, so the permissive fallback never fires.
fn value_to_schema(value: serde_json::Value) -> Schema {
    serde_json::from_value(value).unwrap_or(Schema::Bool(true))
}

/// JSON Schema for a single-key check map `{ <key>: <value>, severity? }`.
fn map_form_schema(
    key: &str,
    value: &serde_json::Value,
    severity: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { key: value, "severity": severity },
        "required": [key],
        "additionalProperties": false
    })
}

/// A dataset-level check (row counts, freshness, column ratios, expressions).
///
/// Each list item is a single-key map; the key names the check:
///
/// ```yaml
/// dataset_checks:
///   - row_count_min: 1
///   - freshness: { column: signed_up, max_age: 48h, severity: warn }
///   - null_ratio_max: { column: age, ratio: 0.10 }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum DatasetCheck {
    /// The dataset must contain at least this many rows.
    RowCountMin {
        /// Minimum row count (inclusive).
        count: u64,
        /// Failure severity.
        severity: Severity,
    },
    /// The dataset must contain at most this many rows.
    RowCountMax {
        /// Maximum row count (inclusive).
        count: u64,
        /// Failure severity.
        severity: Severity,
    },
    /// The newest value of a date/datetime column must be recent enough.
    Freshness {
        /// The date/datetime column to inspect.
        column: String,
        /// Maximum age of the newest row (parsed from humantime strings like
        /// `48h`, `7d`; serialized back as a humanized string).
        max_age: Duration,
        /// Failure severity.
        severity: Severity,
    },
    /// Null ratio of a column must be `<= ratio`.
    NullRatioMax {
        /// Target column.
        column: String,
        /// Maximum allowed null ratio in `[0, 1]`.
        ratio: f64,
        /// Failure severity.
        severity: Severity,
    },
    /// Distinct-value ratio of a column must be `>= ratio`.
    UniqueRatioMin {
        /// Target column.
        column: String,
        /// Minimum required unique ratio in `[0, 1]`.
        ratio: f64,
        /// Failure severity.
        severity: Severity,
    },
    /// Escape hatch: a Polars expression evaluated over the whole dataset.
    CustomExpr {
        /// The expression source text.
        expr: String,
        /// Failure severity.
        severity: Severity,
    },
}

/// Names of all dataset checks as they appear in YAML.
pub(crate) const DATASET_CHECK_NAMES: &[&str] = &[
    "row_count_min",
    "row_count_max",
    "freshness",
    "null_ratio_max",
    "unique_ratio_min",
    "custom_expr",
];

impl DatasetCheck {
    /// The check's failure severity.
    pub fn severity(&self) -> Severity {
        match self {
            DatasetCheck::RowCountMin { severity, .. }
            | DatasetCheck::RowCountMax { severity, .. }
            | DatasetCheck::Freshness { severity, .. }
            | DatasetCheck::NullRatioMax { severity, .. }
            | DatasetCheck::UniqueRatioMin { severity, .. }
            | DatasetCheck::CustomExpr { severity, .. } => *severity,
        }
    }

    /// The YAML name of this check kind (`"row_count_min"`, …).
    pub fn kind_name(&self) -> &'static str {
        match self {
            DatasetCheck::RowCountMin { .. } => "row_count_min",
            DatasetCheck::RowCountMax { .. } => "row_count_max",
            DatasetCheck::Freshness { .. } => "freshness",
            DatasetCheck::NullRatioMax { .. } => "null_ratio_max",
            DatasetCheck::UniqueRatioMin { .. } => "unique_ratio_min",
            DatasetCheck::CustomExpr { .. } => "custom_expr",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreshnessDe {
    column: String,
    max_age: String,
    #[serde(default)]
    severity: Option<Severity>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnRatioDe {
    column: String,
    ratio: f64,
    #[serde(default)]
    severity: Option<Severity>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomExprDe {
    expr: String,
    #[serde(default)]
    severity: Option<Severity>,
}

#[derive(Serialize)]
struct FreshnessSer<'a> {
    column: &'a str,
    max_age: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<Severity>,
}

#[derive(Serialize)]
struct ColumnRatioSer<'a> {
    column: &'a str,
    ratio: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<Severity>,
}

#[derive(Serialize)]
struct CustomExprSer<'a> {
    expr: &'a str,
    severity: Severity,
}

/// Resolves severity given both possible spellings (inside the check's value
/// map vs. alongside the check key). Specifying both is an error.
fn resolve_severity(
    check: &str,
    inner: Option<Severity>,
    sibling: Option<Severity>,
) -> Result<Severity, String> {
    match (inner, sibling) {
        (Some(_), Some(_)) => Err(format!(
            "severity for `{check}` is specified twice (inside the map and alongside it); keep exactly one"
        )),
        (inner, sibling) => Ok(inner.or(sibling).unwrap_or_default()),
    }
}

/// Builds a [`DatasetCheck`] from a map-form key/value pair.
fn build_dataset_check(
    name: &str,
    value: serde_yaml::Value,
    sibling_severity: Option<Severity>,
) -> Result<DatasetCheck, String> {
    match name {
        "row_count_min" => Ok(DatasetCheck::RowCountMin {
            count: typed_ds_value(name, "a non-negative integer", "- row_count_min: 1", value)?,
            severity: sibling_severity.unwrap_or_default(),
        }),
        "row_count_max" => Ok(DatasetCheck::RowCountMax {
            count: typed_ds_value(
                name,
                "a non-negative integer",
                "- row_count_max: 1000000",
                value,
            )?,
            severity: sibling_severity.unwrap_or_default(),
        }),
        "freshness" => {
            let spec: FreshnessDe = typed_ds_value(
                name,
                "a map with `column` and `max_age`",
                "- freshness: { column: signed_up, max_age: 48h }",
                value,
            )?;
            let max_age = humantime::parse_duration(&spec.max_age).map_err(|e| {
                format!(
                    "invalid `max_age` duration `{}` in `freshness`: {e}; examples: 30m, 48h, 7d",
                    spec.max_age
                )
            })?;
            let severity = resolve_severity(name, spec.severity, sibling_severity)?;
            Ok(DatasetCheck::Freshness {
                column: spec.column,
                max_age,
                severity,
            })
        }
        "null_ratio_max" => {
            let spec: ColumnRatioDe = typed_ds_value(
                name,
                "a map with `column` and `ratio`",
                "- null_ratio_max: { column: age, ratio: 0.1 }",
                value,
            )?;
            let severity = resolve_severity(name, spec.severity, sibling_severity)?;
            Ok(DatasetCheck::NullRatioMax {
                column: spec.column,
                ratio: spec.ratio,
                severity,
            })
        }
        "unique_ratio_min" => {
            let spec: ColumnRatioDe = typed_ds_value(
                name,
                "a map with `column` and `ratio`",
                "- unique_ratio_min: { column: user_id, ratio: 0.9 }",
                value,
            )?;
            let severity = resolve_severity(name, spec.severity, sibling_severity)?;
            Ok(DatasetCheck::UniqueRatioMin {
                column: spec.column,
                ratio: spec.ratio,
                severity,
            })
        }
        "custom_expr" => match value {
            serde_yaml::Value::String(expr) => Ok(DatasetCheck::CustomExpr {
                expr,
                severity: sibling_severity.unwrap_or_default(),
            }),
            other => {
                let spec: CustomExprDe = typed_ds_value(
                    name,
                    "an expression string or a map with `expr`",
                    r#"- custom_expr: "row_count > 0""#,
                    other,
                )?;
                let severity = resolve_severity(name, spec.severity, sibling_severity)?;
                Ok(DatasetCheck::CustomExpr {
                    expr: spec.expr,
                    severity,
                })
            }
        },
        other => {
            if COLUMN_CHECK_NAMES.contains(&other) {
                return Err(format!(
                    "`{other}` is a column check, not a dataset check; move it under `columns.<name>.checks` (dataset checks: {})",
                    DATASET_CHECK_NAMES.join(", ")
                ));
            }
            Err(unknown_check_message(other, DATASET_CHECK_NAMES))
        }
    }
}

/// Like [`typed_value`] but with a dataset-check example string.
fn typed_ds_value<T: serde::de::DeserializeOwned>(
    check: &str,
    expected: &str,
    example: &str,
    value: serde_yaml::Value,
) -> Result<T, String> {
    let repr = value_repr(&value);
    serde_yaml::from_value(value).map_err(|e| {
        format!("invalid value `{repr}` for dataset check `{check}`: {e}; expected {expected}, e.g. `{example}`")
    })
}

impl<'de> Deserialize<'de> for DatasetCheck {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DatasetCheckVisitor)
    }
}

struct DatasetCheckVisitor;

impl<'de> Visitor<'de> for DatasetCheckVisitor {
    type Value = DatasetCheck;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a single-key map like `row_count_min: 1`")
    }

    fn visit_str<E: DeError>(self, s: &str) -> Result<DatasetCheck, E> {
        if DATASET_CHECK_NAMES.contains(&s) {
            return Err(E::custom(format!(
                "dataset check `{s}` requires a value; write it in map form, e.g. `- {s}: …`"
            )));
        }
        Err(E::custom(unknown_check_message(s, DATASET_CHECK_NAMES)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DatasetCheck, A::Error> {
        let mut severity: Option<Severity> = None;
        let mut check: Option<(String, serde_yaml::Value)> = None;
        while let Some(key) = map.next_key::<String>()? {
            let value: serde_yaml::Value = map.next_value()?;
            if key == "severity" {
                severity = Some(parse_severity(value).map_err(A::Error::custom)?);
            } else if let Some((first, _)) = &check {
                return Err(A::Error::custom(format!(
                    "a dataset check must contain exactly one check key, found `{first}` and `{key}`; write them as separate list items"
                )));
            } else {
                check = Some((key, value));
            }
        }
        let (name, value) = check.ok_or_else(|| {
            A::Error::custom(
                "a dataset check needs a check key, e.g. `- row_count_min: 1` (severity alone is not a check)",
            )
        })?;
        build_dataset_check(&name, value, severity).map_err(A::Error::custom)
    }
}

impl Serialize for DatasetCheck {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let severity = self.severity();
        let sibling_warn = matches!(
            self,
            DatasetCheck::RowCountMin { .. } | DatasetCheck::RowCountMax { .. }
        ) && severity == Severity::Warn;
        let entries = 1 + usize::from(sibling_warn);
        let mut map = serializer.serialize_map(Some(entries))?;
        match self {
            DatasetCheck::RowCountMin { count, .. } => {
                map.serialize_entry("row_count_min", count)?;
            }
            DatasetCheck::RowCountMax { count, .. } => {
                map.serialize_entry("row_count_max", count)?;
            }
            DatasetCheck::Freshness {
                column, max_age, ..
            } => {
                map.serialize_entry(
                    "freshness",
                    &FreshnessSer {
                        column,
                        max_age: humantime::format_duration(*max_age).to_string(),
                        severity: (severity == Severity::Warn).then_some(Severity::Warn),
                    },
                )?;
            }
            DatasetCheck::NullRatioMax { column, ratio, .. } => {
                map.serialize_entry(
                    "null_ratio_max",
                    &ColumnRatioSer {
                        column,
                        ratio: *ratio,
                        severity: (severity == Severity::Warn).then_some(Severity::Warn),
                    },
                )?;
            }
            DatasetCheck::UniqueRatioMin { column, ratio, .. } => {
                map.serialize_entry(
                    "unique_ratio_min",
                    &ColumnRatioSer {
                        column,
                        ratio: *ratio,
                        severity: (severity == Severity::Warn).then_some(Severity::Warn),
                    },
                )?;
            }
            DatasetCheck::CustomExpr { expr, .. } => {
                if severity == Severity::Warn {
                    map.serialize_entry(
                        "custom_expr",
                        &CustomExprSer {
                            expr,
                            severity: Severity::Warn,
                        },
                    )?;
                } else {
                    map.serialize_entry("custom_expr", expr)?;
                }
            }
        }
        if sibling_warn {
            map.serialize_entry("severity", &Severity::Warn)?;
        }
        map.end()
    }
}

impl JsonSchema for DatasetCheck {
    fn schema_name() -> String {
        "DatasetCheck".to_string()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let severity = schema_to_value(generator.subschema_for::<Severity>());
        let count = serde_json::json!({ "type": "integer", "minimum": 0 });
        let freshness = serde_json::json!({
            "type": "object",
            "properties": {
                "column": { "type": "string" },
                "max_age": { "type": "string" },
                "severity": severity
            },
            "required": ["column", "max_age"],
            "additionalProperties": false
        });
        let column_ratio = serde_json::json!({
            "type": "object",
            "properties": {
                "column": { "type": "string" },
                "ratio": { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                "severity": severity
            },
            "required": ["column", "ratio"],
            "additionalProperties": false
        });
        let custom_expr = serde_json::json!({
            "anyOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "properties": { "expr": { "type": "string" }, "severity": severity },
                    "required": ["expr"],
                    "additionalProperties": false
                }
            ]
        });
        let forms = vec![
            map_form_schema("row_count_min", &count, &severity),
            map_form_schema("row_count_max", &count, &severity),
            map_form_schema("freshness", &freshness, &severity),
            map_form_schema("null_ratio_max", &column_ratio, &severity),
            map_form_schema("unique_ratio_min", &column_ratio, &severity),
            map_form_schema("custom_expr", &custom_expr, &severity),
        ];
        value_to_schema(serde_json::json!({ "anyOf": forms }))
    }
}

/// A downstream consumer of the dataset.
///
/// The validation engine still never reads this — a consumer changes no
/// verdict. What [`Consumer::reads`] adds is the other half of the sentence a
/// contract is for: not only what the supplier promised, but who inside your
/// own company is standing behind that promise. It is what lets a tool answer
/// *who breaks* when a column is dropped or a delivery fails on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    /// Consumer name (team, service, or dashboard).
    pub name: String,
    /// Contact for the consumer (email, Slack channel, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
    /// What this consumer is — see [`ConsumerKind`].
    ///
    /// Absent stays absent. There is no sensible default here: guessing
    /// `pipeline` for a consumer whose author said nothing would print a
    /// category somebody could act on and nobody wrote down.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ConsumerKind>,
    /// How much depends on this one, from 1 (most) to 3 (least).
    ///
    /// Deliberately three numbers and not a free-text severity. The point of a
    /// tier is to be *comparable* across two consumers nobody has ever had to
    /// compare before — the moment somebody has to choose which of two teams
    /// finds out first. A scale with five levels, or with words, is a scale
    /// where everything ends up in the top two.
    ///
    /// Absent means it has not been ranked, which is not the same as tier 3.
    /// [`crate::validate`](fn@crate::validate) proves the range; nothing here decides what a tier
    /// is worth, because that is a decision each organisation makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = 3))]
    pub tier: Option<u8>,
    /// The columns this consumer depends on.
    ///
    /// **Empty means the whole dataset**, and that is the honest default: a
    /// consumer who has not said which columns they read is a consumer who
    /// might break on any of them, and quietly narrowing their blast radius to
    /// nothing would make an unanswered question look like a safe answer. Say
    /// the columns and the answer gets sharper — a change to `region` stops
    /// paging the team that only reads `amount`.
    ///
    /// Every name must be a column the contract declares; [`crate::validate`](fn@crate::validate)
    /// rejects the rest, because a typo here is a dependency that silently
    /// matches nothing, which is worse than no declaration at all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reads: Vec<String>,
}

impl Consumer {
    /// Whether this consumer depends on `column`.
    ///
    /// A consumer that named no columns depends on all of them — see
    /// [`Consumer::reads`].
    pub fn reads_column(&self, column: &str) -> bool {
        self.reads.is_empty() || self.reads.iter().any(|c| c == column)
    }

    /// Whether this consumer's dependency is undeclared, and therefore total.
    ///
    /// Worth distinguishing when reporting: "affected (reads the whole
    /// dataset)" and "affected (reads `email`)" are different degrees of
    /// certainty, and flattening them would overstate the second or understate
    /// the first.
    pub fn reads_whole_dataset(&self) -> bool {
        self.reads.is_empty()
    }
}

/// Definition of a single column in the contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColumnDef {
    /// Declared data type.
    pub r#type: ColType,
    /// What the column means, in a sentence the supplier would recognise.
    ///
    /// The engine never reads this. It exists because the expensive half of a
    /// broken delivery is the argument about what the column was supposed to
    /// contain, and that argument is settled by a sentence written before
    /// anything broke. ODCS carries the same field on a property, so it
    /// survives a round trip in both directions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// If `true`, the column must not contain nulls.
    #[serde(default)]
    pub required: bool,
    /// PII tag (reserved, FR-11 — no engine behavior in v1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pii: Option<PiiKind>,
    /// Data classification (reserved, FR-11 — no engine behavior in v1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classification: Option<DataClass>,
    /// How much a consumer may rely on this column staying as it is.
    ///
    /// Absent means [`Stability::Stable`] — the promise every column has always
    /// carried, stated out loud rather than assumed.
    #[serde(default, skip_serializing_if = "Stability::is_default")]
    pub stability: Stability,
    /// The date after which this column will no longer be here, as `YYYY-MM-DD`.
    ///
    /// Held as a string rather than a date type on purpose: this crate parses
    /// contracts on machines that may have no clock worth trusting, and it has
    /// no business deciding what "today" is. [`crate::validate`](fn@crate::validate) proves the
    /// shape and the calendar; ISO-8601 sorts lexicographically, which is all
    /// the diff needs to tell a date that moved closer from one that moved
    /// away. Anything that acts on the date — refusing a change, chasing the
    /// consumers who still read the column — does it where a clock exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sunset: Option<String>,
    /// Column-level checks, evaluated in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<ColumnCheck>,
}

/// The window a supplier gives consumers to get ready for a breaking change.
///
/// A breaking change with a date attached is a plan. The same change without
/// one is an outage that has not happened yet, and the difference between them
/// is not the diff — it is whether anybody downstream was told when.
///
/// This block is about *this version*: what it broke, and by when everyone
/// reading it has to have moved. It is expected to be dropped again once the
/// window closes, so a later version losing it is not itself an event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    /// The date consumers must be ready by, as `YYYY-MM-DD`.
    ///
    /// Required, and deliberately so. A migration block without a deadline is a
    /// promise to deal with it later, which is the thing it exists to replace.
    pub window_ends: String,
    /// What consumers have to actually do, in their words rather than the
    /// diff's. "`amount` is minor units from 15 Oct; read `amount_minor` today"
    /// is the sentence that saves the calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Dataset-wide validation settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Whether columns not declared in the contract are allowed (default `true`).
    pub allow_extra_columns: bool,
    /// Whether the dataset must contain *exactly* the declared columns
    /// (default `false`).
    pub columns_exact: bool,
    /// Severity applied when a value cannot be read as the declared type
    /// (default `error`).
    pub on_type_mismatch: Severity,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            allow_extra_columns: true,
            columns_exact: false,
            on_type_mismatch: Severity::Error,
        }
    }
}

/// A parsed data contract (the root of `contract.yaml`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Contract schema version (`apiVersion: v1`).
    #[serde(rename = "apiVersion")]
    pub api_version: ApiVersion,
    /// Dataset name; part of the contract's identity.
    pub dataset: String,
    /// Owning team or person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Human description of the dataset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Free-form contract version string (metadata only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Downstream consumers (reserved, FR-11 — no engine behavior in v1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<Consumer>,
    /// Column definitions, in declaration order.
    #[serde(default)]
    pub columns: IndexMap<String, ColumnDef>,
    /// Dataset-level checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dataset_checks: Vec<DatasetCheck>,
    /// The migration window for the breaking changes this version introduces.
    ///
    /// Optional, because most versions break nothing. A project can require it
    /// for the ones that do — see the cloud's review settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migration: Option<Migration>,
    /// Validation settings.
    #[serde(default)]
    pub settings: Settings,
}

/// Whether `s` is a real calendar date written `YYYY-MM-DD`.
///
/// Hand-rolled rather than pulled from a date library because this crate holds
/// its dependencies down to the ones the parser cannot do without, and this is
/// twenty lines. It is strict about the shape — no `2026-9-1`, no times, no
/// zone — so that two dates can always be compared as strings.
pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    if !b
        .iter()
        .enumerate()
        .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let num = |from: usize, to: usize| s[from..to].parse::<u32>().unwrap_or(0);
    let (y, m, d) = (num(0, 4), num(5, 7), num(8, 10));
    if !(1..=12).contains(&m) || d == 0 {
        return false;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    d <= days
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn check(yaml: &str) -> ColumnCheck {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn check_err(yaml: &str) -> String {
        serde_yaml::from_str::<ColumnCheck>(yaml)
            .unwrap_err()
            .to_string()
    }

    fn ds_check(yaml: &str) -> DatasetCheck {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn ds_check_err(yaml: &str) -> String {
        serde_yaml::from_str::<DatasetCheck>(yaml)
            .unwrap_err()
            .to_string()
    }

    // ── bare string forms ─────────────────────────────────────────────

    #[test]
    fn bare_unique() {
        assert_eq!(
            check("unique"),
            ColumnCheck::Unique {
                approx: false,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn bare_not_empty_string() {
        assert_eq!(
            check("not_empty_string"),
            ColumnCheck::NotEmptyString {
                severity: Severity::Error
            }
        );
    }

    // ── map forms, one per check ──────────────────────────────────────

    #[test]
    fn map_min_int() {
        assert_eq!(
            check("{ min: 18 }"),
            ColumnCheck::Min {
                min: Number::Int(18),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_min_float_with_severity() {
        assert_eq!(
            check("{ min: 18.5, severity: warn }"),
            ColumnCheck::Min {
                min: Number::Float(18.5),
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn map_max() {
        assert_eq!(
            check("{ max: 120 }"),
            ColumnCheck::Max {
                max: Number::Int(120),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_regex() {
        assert_eq!(
            check(r#"{ regex: "^[A-Z]{2}$" }"#),
            ColumnCheck::Regex {
                regex: "^[A-Z]{2}$".to_string(),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_enum_mixed_values() {
        assert_eq!(
            check("{ enum: [free, 2, 2.5, true] }"),
            ColumnCheck::Enum {
                values: vec![
                    EnumValue::String("free".into()),
                    EnumValue::Int(2),
                    EnumValue::Float(2.5),
                    EnumValue::Bool(true),
                ],
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_length_exact() {
        assert_eq!(
            check("{ length: 2 }"),
            ColumnCheck::Length {
                length: LengthSpec::Exact(2),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_length_range() {
        assert_eq!(
            check("{ length: { min: 1, max: 10 } }"),
            ColumnCheck::Length {
                length: LengthSpec::Range(LengthRange {
                    min: Some(1),
                    max: Some(10)
                }),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_length_open_range() {
        assert_eq!(
            check("{ length: { min: 3 } }"),
            ColumnCheck::Length {
                length: LengthSpec::Range(LengthRange {
                    min: Some(3),
                    max: None
                }),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_format() {
        assert_eq!(
            check("{ format: email }"),
            ColumnCheck::Format {
                format: KnownFormat::Email,
                severity: Severity::Error
            }
        );
        assert_eq!(
            check("{ format: country_code_iso2 }"),
            ColumnCheck::Format {
                format: KnownFormat::CountryCodeIso2,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_null_ratio_max() {
        assert_eq!(
            check("{ null_ratio_max: 0.1 }"),
            ColumnCheck::NullRatioMax {
                ratio: 0.1,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_unique_ratio_min() {
        assert_eq!(
            check("{ unique_ratio_min: 0.9, severity: warn }"),
            ColumnCheck::UniqueRatioMin {
                ratio: 0.9,
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn map_custom_expr() {
        assert_eq!(
            check(r#"{ custom_expr: "age >= 0" }"#),
            ColumnCheck::CustomExpr {
                expr: "age >= 0".to_string(),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_unique_approx() {
        assert_eq!(
            check("{ unique: { approx: true } }"),
            ColumnCheck::Unique {
                approx: true,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn map_unique_true_with_severity() {
        assert_eq!(
            check("{ unique: true, severity: warn }"),
            ColumnCheck::Unique {
                approx: false,
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn map_not_empty_string_with_severity() {
        assert_eq!(
            check("{ not_empty_string: true, severity: warn }"),
            ColumnCheck::NotEmptyString {
                severity: Severity::Warn
            }
        );
    }

    // ── rejection tests ───────────────────────────────────────────────

    #[test]
    fn rejects_unknown_bare_check_with_suggestion() {
        let err = check_err("uniq");
        assert!(err.contains("unknown check `uniq`"), "{err}");
        assert!(err.contains("did you mean `unique`?"), "{err}");
    }

    #[test]
    fn rejects_unknown_map_check_with_suggestion() {
        let err = check_err("{ mn: 18 }");
        assert!(err.contains("unknown check `mn`"), "{err}");
        assert!(err.contains("did you mean `min`?"), "{err}");
    }

    #[test]
    fn rejects_bare_check_that_needs_value() {
        let err = check_err("min");
        assert!(err.contains("requires a value"), "{err}");
        assert!(err.contains("{ min: 18 }"), "{err}");
    }

    #[test]
    fn rejects_wrong_value_type_for_min() {
        let err = check_err("{ min: eighteen }");
        assert!(err.contains("invalid value"), "{err}");
        assert!(err.contains("expected a number"), "{err}");
        assert!(err.contains("{ min: 18 }"), "{err}");
    }

    #[test]
    fn rejects_wrong_value_type_for_regex() {
        let err = check_err("{ regex: 42 }");
        assert!(err.contains("expected a pattern string"), "{err}");
    }

    #[test]
    fn rejects_unknown_format_with_suggestion() {
        let err = check_err("{ format: emial }");
        assert!(err.contains("unknown format `emial`"), "{err}");
        assert!(err.contains("did you mean `email`?"), "{err}");
    }

    #[test]
    fn rejects_non_list_enum() {
        let err = check_err("{ enum: free }");
        assert!(err.contains("expected a list of allowed values"), "{err}");
    }

    #[test]
    fn rejects_bad_length() {
        let err = check_err("{ length: tiny }");
        assert!(err.contains("expected an exact length"), "{err}");
    }

    #[test]
    fn rejects_bad_severity() {
        let err = check_err("{ min: 18, severity: fatal }");
        assert!(err.contains("invalid severity `fatal`"), "{err}");
        assert!(err.contains("expected `error` or `warn`"), "{err}");
    }

    #[test]
    fn rejects_two_check_keys_in_one_map() {
        let err = check_err("{ min: 18, max: 120 }");
        assert!(err.contains("exactly one check key"), "{err}");
        assert!(err.contains("separate list items"), "{err}");
    }

    #[test]
    fn rejects_severity_only_map() {
        let err = check_err("{ severity: warn }");
        assert!(err.contains("severity alone is not a check"), "{err}");
    }

    #[test]
    fn rejects_unique_false() {
        let err = check_err("{ unique: false }");
        assert!(err.contains("to disable the check"), "{err}");
    }

    #[test]
    fn rejects_bad_unique_opts() {
        let err = check_err("{ unique: { approximate: true } }");
        assert!(err.contains("invalid value"), "{err}");
    }

    #[test]
    fn rejects_non_string_non_map_check() {
        let err = check_err("42");
        assert!(err.contains("check name"), "{err}");
    }

    // ── dataset checks ────────────────────────────────────────────────

    #[test]
    fn ds_row_count_min() {
        assert_eq!(
            ds_check("row_count_min: 1"),
            DatasetCheck::RowCountMin {
                count: 1,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn ds_row_count_max_with_sibling_severity() {
        assert_eq!(
            ds_check("{ row_count_max: 100, severity: warn }"),
            DatasetCheck::RowCountMax {
                count: 100,
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn ds_freshness_inner_severity() {
        assert_eq!(
            ds_check("freshness: { column: signed_up, max_age: 48h, severity: warn }"),
            DatasetCheck::Freshness {
                column: "signed_up".into(),
                max_age: Duration::from_secs(48 * 3600),
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn ds_freshness_sibling_severity() {
        assert_eq!(
            ds_check("{ freshness: { column: ts, max_age: 7d }, severity: warn }"),
            DatasetCheck::Freshness {
                column: "ts".into(),
                max_age: Duration::from_secs(7 * 24 * 3600),
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn ds_null_ratio_max() {
        assert_eq!(
            ds_check("null_ratio_max: { column: age, ratio: 0.10 }"),
            DatasetCheck::NullRatioMax {
                column: "age".into(),
                ratio: 0.10,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn ds_unique_ratio_min() {
        assert_eq!(
            ds_check("unique_ratio_min: { column: user_id, ratio: 0.99 }"),
            DatasetCheck::UniqueRatioMin {
                column: "user_id".into(),
                ratio: 0.99,
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn ds_custom_expr_bare_string() {
        assert_eq!(
            ds_check(r#"custom_expr: "row_count > 0""#),
            DatasetCheck::CustomExpr {
                expr: "row_count > 0".into(),
                severity: Severity::Error
            }
        );
    }

    #[test]
    fn ds_custom_expr_map_form() {
        assert_eq!(
            ds_check(r#"custom_expr: { expr: "row_count > 0", severity: warn }"#),
            DatasetCheck::CustomExpr {
                expr: "row_count > 0".into(),
                severity: Severity::Warn
            }
        );
    }

    #[test]
    fn ds_rejects_double_severity() {
        let err = ds_check_err(
            "{ freshness: { column: ts, max_age: 1h, severity: warn }, severity: error }",
        );
        assert!(err.contains("specified twice"), "{err}");
    }

    #[test]
    fn ds_rejects_bad_duration() {
        let err = ds_check_err("freshness: { column: ts, max_age: soon }");
        assert!(err.contains("invalid `max_age` duration `soon`"), "{err}");
        assert!(err.contains("48h"), "{err}");
    }

    #[test]
    fn ds_rejects_unknown_check_with_suggestion() {
        let err = ds_check_err("row_count_mn: 1");
        assert!(err.contains("unknown check `row_count_mn`"), "{err}");
        assert!(err.contains("did you mean `row_count_min`?"), "{err}");
    }

    #[test]
    fn ds_rejects_column_check_in_dataset_position() {
        let err = ds_check_err("min: 3");
        assert!(err.contains("column check, not a dataset check"), "{err}");
    }

    #[test]
    fn ds_rejects_bare_string() {
        let err = ds_check_err("freshness");
        assert!(err.contains("requires a value"), "{err}");
    }

    // ── serialization round-trips ─────────────────────────────────────

    #[test]
    fn roundtrip_every_column_check_form() {
        let checks = vec![
            ColumnCheck::Unique {
                approx: false,
                severity: Severity::Error,
            },
            ColumnCheck::Unique {
                approx: true,
                severity: Severity::Warn,
            },
            ColumnCheck::Unique {
                approx: false,
                severity: Severity::Warn,
            },
            ColumnCheck::NotEmptyString {
                severity: Severity::Error,
            },
            ColumnCheck::NotEmptyString {
                severity: Severity::Warn,
            },
            ColumnCheck::Min {
                min: Number::Int(18),
                severity: Severity::Error,
            },
            ColumnCheck::Max {
                max: Number::Float(1.5),
                severity: Severity::Warn,
            },
            ColumnCheck::Regex {
                regex: "^[A-Z]{2}$".into(),
                severity: Severity::Error,
            },
            ColumnCheck::Enum {
                values: vec![EnumValue::String("a".into()), EnumValue::Int(1)],
                severity: Severity::Error,
            },
            ColumnCheck::Length {
                length: LengthSpec::Exact(2),
                severity: Severity::Error,
            },
            ColumnCheck::Length {
                length: LengthSpec::Range(LengthRange {
                    min: Some(1),
                    max: Some(10),
                }),
                severity: Severity::Warn,
            },
            ColumnCheck::Format {
                format: KnownFormat::Uuid,
                severity: Severity::Error,
            },
            ColumnCheck::NullRatioMax {
                ratio: 0.1,
                severity: Severity::Error,
            },
            ColumnCheck::UniqueRatioMin {
                ratio: 0.9,
                severity: Severity::Warn,
            },
            ColumnCheck::CustomExpr {
                expr: "age >= 0".into(),
                severity: Severity::Error,
            },
        ];
        for original in checks {
            let yaml = serde_yaml::to_string(&original).unwrap();
            let parsed: ColumnCheck = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(parsed, original, "round-trip failed for yaml: {yaml}");
        }
    }

    #[test]
    fn roundtrip_every_dataset_check_form() {
        let checks = vec![
            DatasetCheck::RowCountMin {
                count: 1,
                severity: Severity::Error,
            },
            DatasetCheck::RowCountMax {
                count: 10,
                severity: Severity::Warn,
            },
            DatasetCheck::Freshness {
                column: "ts".into(),
                max_age: Duration::from_secs(48 * 3600),
                severity: Severity::Warn,
            },
            DatasetCheck::Freshness {
                column: "ts".into(),
                max_age: Duration::from_secs(90),
                severity: Severity::Error,
            },
            DatasetCheck::NullRatioMax {
                column: "age".into(),
                ratio: 0.1,
                severity: Severity::Error,
            },
            DatasetCheck::UniqueRatioMin {
                column: "id".into(),
                ratio: 0.99,
                severity: Severity::Warn,
            },
            DatasetCheck::CustomExpr {
                expr: "x > 0".into(),
                severity: Severity::Error,
            },
            DatasetCheck::CustomExpr {
                expr: "x > 0".into(),
                severity: Severity::Warn,
            },
        ];
        for original in checks {
            let yaml = serde_yaml::to_string(&original).unwrap();
            let parsed: DatasetCheck = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(parsed, original, "round-trip failed for yaml: {yaml}");
        }
    }

    #[test]
    fn bare_forms_serialize_as_strings() {
        let yaml = serde_yaml::to_string(&ColumnCheck::Unique {
            approx: false,
            severity: Severity::Error,
        })
        .unwrap();
        assert_eq!(yaml.trim(), "unique");
        let yaml = serde_yaml::to_string(&ColumnCheck::NotEmptyString {
            severity: Severity::Error,
        })
        .unwrap();
        assert_eq!(yaml.trim(), "not_empty_string");
    }

    #[test]
    fn settings_defaults() {
        let s: Settings = serde_yaml::from_str("{}").unwrap();
        assert_eq!(s, Settings::default());
        assert!(s.allow_extra_columns);
        assert!(!s.columns_exact);
        assert_eq!(s.on_type_mismatch, Severity::Error);
    }

    #[test]
    fn settings_rejects_unknown_key() {
        let err = serde_yaml::from_str::<Settings>("allow_extra_cols: true").unwrap_err();
        assert!(err.to_string().contains("unknown field"), "{err}");
    }

    #[test]
    fn column_def_defaults() {
        let c: ColumnDef = serde_yaml::from_str("type: string").unwrap();
        assert_eq!(c.r#type, ColType::String);
        assert!(!c.required);
        assert!(c.pii.is_none());
        assert!(c.classification.is_none());
        assert!(c.checks.is_empty());
    }

    #[test]
    fn pii_and_classification_parse() {
        let c: ColumnDef =
            serde_yaml::from_str("{ type: string, pii: national_id, classification: restricted }")
                .unwrap();
        assert_eq!(c.pii, Some(PiiKind::NationalId));
        assert_eq!(c.classification, Some(DataClass::Restricted));
    }
}
