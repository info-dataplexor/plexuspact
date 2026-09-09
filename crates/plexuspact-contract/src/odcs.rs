//! Open Data Contract Standard (ODCS, Bitol) import and export.
//!
//! Maps between the PlexusPact contract model and an ODCS v3 document so a
//! contract can move in and out of the wider data-contract ecosystem — a
//! catalog that reads ODCS, a partner that publishes it, a CLI run against a
//! document somebody else wrote. Like [`crate::export`], this is a pure
//! projection of the model: no IO, no engine.
//!
//! The mapping philosophy:
//!
//! * Anything with a native ODCS slot uses it, so a foreign reader sees a rule
//!   and not a vendor blob: types, `required`, `unique`, `primaryKey`,
//!   `classification`, error-severity `min`/`max`/`regex`/`length`/`format`
//!   checks in `logicalTypeOptions`; a `references` check as a schema
//!   `relationships` entry (ODCS 3.1); a `freshness` check as the `latency`
//!   SLA property; a column's `sunset` as its `endOfLife`; `row_count_min` /
//!   `row_count_max` as the standard `rowCount` library metric.
//! * Everything else (enum, ratios, custom expressions, `assert`, every
//!   warn-severity check — ODCS slots carry no severity) rides in
//!   spec-sanctioned custom quality entries
//!   (`quality: {type: custom, engine: plexuspact, implementation: <yaml>}`),
//!   which makes export → import lossless for the whole check library.
//! * Import is tolerant: unmappable pieces are dropped with a human-readable
//!   note, never a hard failure, because the draft is meant to be reviewed
//!   (and linted) before it is put in force.
//!
//! Documents are stamped `v3.1.0`, the version that introduced relationships
//! and retired `slaDefaultElement`; 3.2.0 (September 2026) is a superset and
//! reads the same. Any v3 document is accepted on import.

use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};

use crate::model::{
    is_iso_date, ApiVersion, ColType, ColumnCheck, ColumnDef, Consumer, Contract, DataClass,
    DatasetCheck, KnownFormat, LengthRange, LengthSpec, Migration, Number, PiiKind, Settings,
    Severity, Stability,
};

/// ODCS spec version stamped on exported documents.
pub const ODCS_API_VERSION: &str = "v3.1.0";
/// Engine name used for PlexusPact-specific custom quality entries.
const ENGINE: &str = "plexuspact";

// ─────────────────────────── ODCS document shape ────────────────────────────
//
// A pragmatic subset of ODCS v3: every field we read or write. Unknown fields
// in foreign documents are ignored on import (serde default behavior).

/// Root of an ODCS v3 data-contract document.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsDocument {
    /// `apiVersion`, e.g. `v3.1.0`.
    pub api_version: String,
    /// Always `DataContract`.
    pub kind: String,
    /// The document's stable identity.
    pub id: String,
    /// Human name of the contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Contract version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Lifecycle status (`active`, `draft`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Business domain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Owning data product.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_product: Option<String>,
    /// Purpose / usage / limitations.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<OdcsDescription>,
    /// The datasets (schema objects) the contract describes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub schema: Vec<OdcsSchemaObject>,
    /// People and roles.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub team: Vec<OdcsTeamMember>,
    /// Service-level promises (`latency`, `endOfLife`, `frequency`, …).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sla_properties: Vec<OdcsSlaProperty>,
    /// Pre-3.1 default element for SLA properties without one. Read, never
    /// written: the field was deprecated in 3.1.0.
    #[serde(skip_serializing)]
    pub sla_default_element: Option<String>,
    /// Vendor extensions (`{property, value}`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_properties: Vec<OdcsCustomProperty>,
}

/// ODCS `description` block.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsDescription {
    /// Why the data exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    /// How it may be used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<String>,
    /// What it must not be used for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitations: Option<String>,
}

/// One schema object (a table/dataset) in an ODCS document.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsSchemaObject {
    /// Logical name.
    pub name: String,
    /// Name in the storage system.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_name: Option<String>,
    /// `object` for a table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_type: Option<String>,
    /// Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Columns.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<OdcsProperty>,
    /// Foreign keys into other objects (ODCS 3.1).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<OdcsRelationship>,
    /// Dataset-level quality rules.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quality: Vec<OdcsQuality>,
}

/// One column (property) of a schema object.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsProperty {
    /// Column name.
    pub name: String,
    /// ODCS logical type (`string`, `integer`, `number`, `boolean`, `date`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_type: Option<String>,
    /// Storage-system type name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_type: Option<String>,
    /// Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Nulls not allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// No repeated values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unique: Option<bool>,
    /// Part of the primary key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<bool>,
    /// 1-based position within a composite key; ODCS writes `-1` for "not
    /// part of the key", which import treats the same as absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_key_position: Option<i64>,
    /// Sensitivity (`public`, `internal`, `confidential`, `restricted`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    /// Portable value constraints.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logical_type_options: Option<OdcsTypeOptions>,
    /// Foreign keys from this column alone (ODCS 3.1).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<OdcsRelationship>,
    /// Column-level quality rules.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub quality: Vec<OdcsQuality>,
    /// Vendor extensions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub custom_properties: Vec<OdcsCustomProperty>,
}

/// ODCS `logicalTypeOptions`: portable value constraints.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsTypeOptions {
    /// Inclusive lower bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimum: Option<serde_yaml::Value>,
    /// Inclusive upper bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximum: Option<serde_yaml::Value>,
    /// Regular expression the value must match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Minimum length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// Maximum length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// Named format (`email`, `uuid`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

impl OdcsTypeOptions {
    fn is_empty(&self) -> bool {
        self.minimum.is_none()
            && self.maximum.is_none()
            && self.pattern.is_none()
            && self.min_length.is_none()
            && self.max_length.is_none()
            && self.format.is_none()
    }
}

/// A foreign key (ODCS 3.1 `relationships`). At object level `from` lists
/// this object's columns; at property level `from` is absent and the
/// property itself is the source. Targets use `object.column` notation.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsRelationship {
    /// `foreignKey`; the only kind the spec names today.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Source columns, as `object.column` or bare names.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "one_or_many"
    )]
    pub from: Vec<String>,
    /// Target columns, as `object.column`.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "one_or_many"
    )]
    pub to: Vec<String>,
}

/// One quality rule. `type: custom` + `engine: plexuspact` entries are ours
/// and round-trip whole; `type: library` entries with the standard
/// `rowCount` / `nullValues` / `duplicateValues` metrics are translated;
/// anything else is reported and dropped on import.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsQuality {
    /// `text`, `library`, `sql` or `custom`.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Library metric name (`rowCount`, `nullValues`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<String>,
    /// Pre-3.1 name of `metric`; also the free-text rule of a `text` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// Engine that runs a `custom` entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// The engine's own form of the rule.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation: Option<String>,
    /// Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Quality dimension (`completeness`, `accuracy`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension: Option<String>,
    /// `error` / `warning` in the documents we have seen; free text in the spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    /// `rows` or `percent` for library metrics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Comparison: equals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be: Option<serde_yaml::Value>,
    /// Comparison: not equals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_not_be: Option<serde_yaml::Value>,
    /// Comparison: `>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be_greater_than: Option<serde_yaml::Value>,
    /// Comparison: `>=`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be_greater_or_equal_to: Option<serde_yaml::Value>,
    /// Comparison: `<`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be_less_than: Option<serde_yaml::Value>,
    /// Comparison: `<=`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be_less_or_equal_to: Option<serde_yaml::Value>,
    /// Comparison: inclusive range `[low, high]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_be_between: Option<Vec<serde_yaml::Value>>,
    /// Comparison: outside `[low, high]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub must_not_be_between: Option<Vec<serde_yaml::Value>>,
}

/// One `slaProperties` entry.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsSlaProperty {
    /// `latency`, `frequency`, `retention`, `endOfLife`, …
    pub property: String,
    /// The promised value.
    pub value: serde_yaml::Value,
    /// Extended value (a second number for `frequency`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_ext: Option<serde_yaml::Value>,
    /// Unit of `value` (`d`, `h`, `m`, `y`, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// What the promise is about, as `object.column` or `object`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// Why (`regulatory`, `analytics`, `operational`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Free text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One `team` entry.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsTeamMember {
    /// Login or email.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Display name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `owner`, `steward`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

/// One `customProperties` entry (`{property, value}`).
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct OdcsCustomProperty {
    /// Key.
    pub property: String,
    /// Any YAML value.
    pub value: serde_yaml::Value,
}

/// Accepts `to: a.b` and `to: [a.b, c.d]` alike.
fn one_or_many<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match Option::<OneOrMany>::deserialize(d)? {
        None => Vec::new(),
        Some(OneOrMany::One(s)) => vec![s],
        Some(OneOrMany::Many(v)) => v,
    })
}

// ───────────────────────────────── export ───────────────────────────────────

/// Converts a PlexusPact contract to an ODCS document. `id` is the stable
/// identity to stamp on the document (a registry uses the contract's UUID;
/// the CLI uses the dataset name unless told otherwise).
pub fn export(contract: &Contract, id: &str) -> OdcsDocument {
    let dataset = contract.dataset.as_str();
    let mut sla_properties = Vec::new();

    let mut properties: Vec<OdcsProperty> = contract
        .columns
        .iter()
        .map(|(name, def)| {
            // The retirement date has a native slot: `endOfLife` on the column.
            if let Some(sunset) = &def.sunset {
                sla_properties.push(OdcsSlaProperty {
                    property: "endOfLife".to_owned(),
                    value: serde_yaml::Value::String(sunset.clone()),
                    element: Some(format!("{dataset}.{name}")),
                    ..OdcsSlaProperty::default()
                });
            }
            export_column(name, def)
        })
        .collect();
    // The key goes to ODCS's own slots, so a foreign reader sees a key and not
    // a vendor blob; position keeps a composite key's order across the trip.
    for (index, key) in contract.primary_key.iter().enumerate() {
        if let Some(prop) = properties.iter_mut().find(|p| &p.name == key) {
            prop.primary_key = Some(true);
            prop.primary_key_position = Some(index as i64 + 1);
        }
    }

    let mut quality = Vec::new();
    let mut relationships = Vec::new();
    for check in &contract.dataset_checks {
        // Error-severity checks with a native slot go there; warn severity
        // has no ODCS expression and rides custom, as for columns.
        match check {
            DatasetCheck::References {
                columns,
                dataset: other,
                to,
                severity: Severity::Error,
            } => relationships.push(OdcsRelationship {
                kind: Some("foreignKey".to_owned()),
                from: columns.iter().map(|c| format!("{dataset}.{c}")).collect(),
                to: to.iter().map(|c| format!("{other}.{c}")).collect(),
            }),
            DatasetCheck::Freshness {
                column,
                max_age,
                severity: Severity::Error,
            } => {
                let (value, unit) = sla_duration(*max_age);
                sla_properties.push(OdcsSlaProperty {
                    property: "latency".to_owned(),
                    value: serde_yaml::Value::from(value),
                    unit: Some(unit.to_owned()),
                    element: Some(format!("{dataset}.{column}")),
                    ..OdcsSlaProperty::default()
                });
            }
            DatasetCheck::RowCountMin {
                count,
                severity: Severity::Error,
            } => quality.push(OdcsQuality {
                must_be_greater_or_equal_to: Some(serde_yaml::Value::from(*count)),
                ..row_count_metric("row_count_min")
            }),
            DatasetCheck::RowCountMax {
                count,
                severity: Severity::Error,
            } => quality.push(OdcsQuality {
                must_be_less_or_equal_to: Some(serde_yaml::Value::from(*count)),
                ..row_count_metric("row_count_max")
            }),
            other => {
                if let Some(q) =
                    custom_quality(other, &format!("dataset check `{}`", other.kind_name()))
                {
                    quality.push(q);
                }
            }
        }
    }

    let mut custom_properties = Vec::new();
    if contract.settings != Settings::default() {
        if let Ok(value) = serde_yaml::to_value(&contract.settings) {
            custom_properties.push(OdcsCustomProperty {
                property: "plexuspactSettings".to_owned(),
                value,
            });
        }
    }
    if !contract.consumers.is_empty() {
        if let Ok(value) = serde_yaml::to_value(&contract.consumers) {
            custom_properties.push(OdcsCustomProperty {
                property: "plexuspactConsumers".to_owned(),
                value,
            });
        }
    }
    // A migration window is a promise made to consumers, so it has to survive
    // the trip out and back. ODCS has no slot for it, so it rides custom.
    if let Some(migration) = &contract.migration {
        if let Ok(value) = serde_yaml::to_value(migration) {
            custom_properties.push(OdcsCustomProperty {
                property: "plexuspactMigration".to_owned(),
                value,
            });
        }
    }

    OdcsDocument {
        api_version: ODCS_API_VERSION.to_owned(),
        kind: "DataContract".to_owned(),
        id: id.to_owned(),
        name: Some(contract.dataset.clone()),
        version: contract.version.clone(),
        status: Some("active".to_owned()),
        domain: None,
        data_product: None,
        description: contract.description.clone().map(|purpose| OdcsDescription {
            purpose: Some(purpose),
            usage: None,
            limitations: None,
        }),
        schema: vec![OdcsSchemaObject {
            name: contract.dataset.clone(),
            physical_name: Some(contract.dataset.clone()),
            logical_type: Some("object".to_owned()),
            description: None,
            properties,
            relationships,
            quality,
        }],
        team: contract
            .owner
            .clone()
            .map(|owner| OdcsTeamMember {
                username: Some(owner),
                name: None,
                role: Some("owner".to_owned()),
            })
            .into_iter()
            .collect(),
        sla_properties,
        sla_default_element: None,
        custom_properties,
    }
}

/// Converts a contract to ODCS YAML.
pub fn export_yaml(contract: &Contract, id: &str) -> Result<String, String> {
    serde_yaml::to_string(&export(contract, id))
        .map_err(|e| format!("failed to serialize ODCS document: {e}"))
}

/// The skeleton of a standard `rowCount` library rule; the caller adds the
/// comparison.
fn row_count_metric(label: &str) -> OdcsQuality {
    OdcsQuality {
        kind: Some("library".to_owned()),
        metric: Some("rowCount".to_owned()),
        unit: Some("rows".to_owned()),
        dimension: Some("completeness".to_owned()),
        severity: Some("error".to_owned()),
        description: Some(format!("PlexusPact dataset check `{label}`")),
        ..OdcsQuality::default()
    }
}

/// A duration as the largest whole unit ODCS readers expect: days, hours,
/// minutes, else seconds.
fn sla_duration(d: Duration) -> (u64, &'static str) {
    let secs = d.as_secs();
    if secs > 0 && secs.is_multiple_of(86_400) {
        (secs / 86_400, "d")
    } else if secs > 0 && secs.is_multiple_of(3_600) {
        (secs / 3_600, "h")
    } else if secs > 0 && secs.is_multiple_of(60) {
        (secs / 60, "m")
    } else {
        (secs, "s")
    }
}

/// ODCS logical type name for a PlexusPact column type.
fn logical_type(t: ColType) -> &'static str {
    match t {
        ColType::String => "string",
        ColType::Int => "integer",
        ColType::Float => "number",
        ColType::Bool => "boolean",
        ColType::Date | ColType::Datetime => "date",
    }
}

fn export_column(name: &str, def: &ColumnDef) -> OdcsProperty {
    let mut options = OdcsTypeOptions::default();
    let mut unique = false;
    let mut quality = Vec::new();

    for check in &def.checks {
        // Error-severity checks with a native ODCS slot go there (portable);
        // everything else — warn severity included, since ODCS options carry
        // no severity — becomes a custom quality entry and round-trips intact.
        let portable = check.severity() == Severity::Error;
        match check {
            ColumnCheck::Unique { approx: false, .. } if portable && !unique => unique = true,
            ColumnCheck::Min { min, .. } if portable && options.minimum.is_none() => {
                options.minimum = serde_yaml::to_value(min).ok();
            }
            ColumnCheck::Max { max, .. } if portable && options.maximum.is_none() => {
                options.maximum = serde_yaml::to_value(max).ok();
            }
            ColumnCheck::Regex { regex, .. } if portable && options.pattern.is_none() => {
                options.pattern = Some(regex.clone());
            }
            ColumnCheck::Length { length, .. }
                if portable && options.min_length.is_none() && options.max_length.is_none() =>
            {
                let (min, max) = length.bounds();
                options.min_length = Some(min);
                options.max_length = max;
            }
            ColumnCheck::Format { format, .. } if portable && options.format.is_none() => {
                options.format = format_name(*format).map(str::to_owned);
            }
            other => {
                if let Some(q) =
                    custom_quality(other, &format!("check `{}` on `{name}`", other.kind_name()))
                {
                    quality.push(q);
                }
            }
        }
    }

    let mut custom_properties = Vec::new();
    if let Some(pii) = def.pii {
        if let Ok(value) = serde_yaml::to_value(pii) {
            custom_properties.push(OdcsCustomProperty {
                property: "plexuspactPii".to_owned(),
                value,
            });
        }
    }
    // What the supplier still promises about a column on its way out. The
    // date itself is the column's `endOfLife` SLA property, written by the
    // caller; the promise level has no ODCS word for it.
    if def.stability != Stability::Stable {
        if let Ok(value) = serde_yaml::to_value(def.stability) {
            custom_properties.push(OdcsCustomProperty {
                property: "plexuspactStability".to_owned(),
                value,
            });
        }
    }

    OdcsProperty {
        name: name.to_owned(),
        logical_type: Some(logical_type(def.r#type).to_owned()),
        physical_type: Some(def.r#type.to_string()),
        description: def.description.clone(),
        required: def.required.then_some(true),
        unique: unique.then_some(true),
        primary_key: None,
        primary_key_position: None,
        classification: def.classification.as_ref().and_then(|c| {
            serde_yaml::to_value(c)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
        }),
        logical_type_options: (!options.is_empty()).then_some(options),
        relationships: Vec::new(),
        quality,
        custom_properties,
    }
}

/// Wraps any serializable check in a `type: custom, engine: plexuspact`
/// quality entry whose implementation is the check's own YAML form.
fn custom_quality<T: Serialize>(check: &T, label: &str) -> Option<OdcsQuality> {
    let implementation = serde_yaml::to_string(check).ok()?;
    Some(OdcsQuality {
        kind: Some("custom".to_owned()),
        engine: Some(ENGINE.to_owned()),
        implementation: Some(implementation.trim_end().to_owned()),
        description: Some(format!("PlexusPact {label}")),
        ..OdcsQuality::default()
    })
}

/// The wire name of a [`KnownFormat`] (its snake_case serde form).
fn format_name(format: KnownFormat) -> Option<&'static str> {
    Some(match format {
        KnownFormat::Email => "email",
        KnownFormat::Uuid => "uuid",
        KnownFormat::IsoDate => "iso_date",
        KnownFormat::IsoDatetime => "iso_datetime",
        KnownFormat::Url => "url",
        KnownFormat::CountryCodeIso2 => "country_code_iso2",
    })
}

// ───────────────────────────────── import ───────────────────────────────────

/// Whether `content` looks like an ODCS document rather than a PlexusPact
/// contract: `kind: DataContract`, or a v3 `apiVersion` next to a `schema`
/// list. Cheap enough to run on every file the CLI opens.
pub fn looks_like_odcs(content: &str) -> bool {
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str::<serde_yaml::Value>(content)
    else {
        return false;
    };
    let get = |k: &str| map.get(serde_yaml::Value::from(k));
    if get("kind").and_then(|v| v.as_str()) == Some("DataContract") {
        return true;
    }
    get("apiVersion")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v.starts_with("v3"))
        && get("schema").is_some_and(|v| v.is_sequence())
}

/// Parses an ODCS v3 document (YAML or JSON) into a PlexusPact contract plus
/// notes describing anything that could not be mapped. Hard-fails only when
/// the document itself is unreadable or contains no usable schema.
pub fn import(content: &str) -> Result<(Contract, Vec<String>), String> {
    let doc: OdcsDocument =
        serde_yaml::from_str(content).map_err(|e| format!("not a readable ODCS document: {e}"))?;
    let mut notes = Vec::new();

    if !doc.kind.is_empty() && doc.kind != "DataContract" {
        notes.push(format!(
            "document kind is `{}`, expected `DataContract`",
            doc.kind
        ));
    }
    if !doc.api_version.is_empty() && !doc.api_version.starts_with("v3") {
        notes.push(format!(
            "document is written for ODCS `{}`; read as v3, so some fields may have moved",
            doc.api_version
        ));
    }

    let object = doc
        .schema
        .first()
        .ok_or_else(|| "ODCS document has no `schema` objects to import".to_owned())?;
    if doc.schema.len() > 1 {
        notes.push(format!(
            "document defines {} schema objects; imported the first (`{}`) — PlexusPact contracts describe one dataset each",
            doc.schema.len(),
            object.name
        ));
    }

    let dataset = if object.name.trim().is_empty() {
        doc.name.clone().unwrap_or_default()
    } else {
        object.name.clone()
    };
    if dataset.trim().is_empty() {
        return Err("neither the schema object nor the document has a name".to_owned());
    }

    let owner = doc
        .team
        .iter()
        .find(|m| {
            m.role
                .as_deref()
                .is_some_and(|r| r.to_ascii_lowercase().contains("owner"))
        })
        .or_else(|| doc.team.first())
        .and_then(|m| m.username.clone().or_else(|| m.name.clone()));

    let description = doc.description.as_ref().and_then(|d| d.purpose.clone());
    if let Some(d) = &doc.description {
        if d.usage.is_some() || d.limitations.is_some() {
            notes.push(
                "description `usage`/`limitations` have no PlexusPact field and were dropped"
                    .to_owned(),
            );
        }
    }

    let mut columns: IndexMap<String, ColumnDef> = IndexMap::new();
    for prop in &object.properties {
        match import_column(prop, &mut notes) {
            Some(def) => {
                columns.insert(prop.name.clone(), def);
            }
            None => notes.push(format!(
                "column `{}` skipped: logical type `{}` has no PlexusPact equivalent",
                prop.name,
                prop.logical_type.as_deref().unwrap_or("<none>")
            )),
        }
    }

    // Key columns in position order; a document that marks keys without
    // positions keeps them in the order they were written. A key naming a
    // column that was skipped (object/array) cannot be a key here.
    let mut keyed: Vec<(i64, String)> = object
        .properties
        .iter()
        .enumerate()
        .filter(|(_, p)| p.primary_key == Some(true) && columns.contains_key(&p.name))
        .map(|(index, p)| {
            let position = p
                .primary_key_position
                .filter(|pos| *pos > 0)
                .unwrap_or(index as i64 + 1);
            (position, p.name.clone())
        })
        .collect();
    keyed.sort_by_key(|(position, _)| *position);
    let primary_key: Vec<String> = keyed.into_iter().map(|(_, name)| name).collect();

    let mut dataset_checks = Vec::new();
    for q in &object.quality {
        match parse_engine_quality::<DatasetCheck>(q) {
            EngineQuality::Parsed(check) => dataset_checks.push(check),
            EngineQuality::Broken(err) => {
                notes.push(format!("dataset quality entry could not be parsed: {err}"));
            }
            EngineQuality::Foreign => match row_count_checks(q) {
                Ok(checks) if !checks.is_empty() => dataset_checks.extend(checks),
                Ok(_) => notes.push(foreign_quality_note(q, "dataset")),
                Err(why) => notes.push(format!("row-count rule dropped: {why}")),
            },
        }
    }

    // Foreign keys: the object's own list, then the ones written on a column.
    let object_name = object.name.as_str();
    for rel in &object.relationships {
        match references_check(rel, None, object_name, &columns) {
            Ok(check) => dataset_checks.push(check),
            Err(why) => notes.push(format!("relationship dropped: {why}")),
        }
    }
    for prop in &object.properties {
        for rel in &prop.relationships {
            match references_check(rel, Some(prop.name.as_str()), object_name, &columns) {
                Ok(check) => dataset_checks.push(check),
                Err(why) => notes.push(format!("relationship on `{}` dropped: {why}", prop.name)),
            }
        }
    }

    // SLA properties: `latency` is a freshness check, `endOfLife` on a column
    // is its sunset. The rest are promises about operations, not about the
    // data in a delivery, and PlexusPact has nothing to check them with.
    let mut unmapped_sla: Vec<String> = Vec::new();
    for sla in &doc.sla_properties {
        let element = sla
            .element
            .as_deref()
            .or(doc.sla_default_element.as_deref())
            .map(str::trim)
            .filter(|e| !e.is_empty());
        let column = element.and_then(|e| local_column(e, object_name, &columns));
        match sla.property.to_ascii_lowercase().as_str() {
            "latency" | "ly" => match (column, parse_sla_duration(&sla.value, sla.unit.as_deref()))
            {
                (Some(column), Some(max_age)) => dataset_checks.push(DatasetCheck::Freshness {
                    column,
                    max_age,
                    severity: Severity::Error,
                }),
                (None, _) => notes.push(format!(
                    "SLA `latency` dropped: element `{}` is not a column of `{object_name}`",
                    element.unwrap_or("<none>")
                )),
                (_, None) => notes.push(format!(
                    "SLA `latency` dropped: value `{}` with unit `{}` is not a duration",
                    yaml_scalar(&sla.value),
                    sla.unit.as_deref().unwrap_or("<none>")
                )),
            },
            "endoflife" => {
                let Some(column) = column else {
                    unmapped_sla.push(sla.property.clone());
                    continue;
                };
                let date = yaml_scalar(&sla.value);
                let day = date.get(..10).unwrap_or(&date);
                if is_iso_date(day) {
                    if let Some(def) = columns.get_mut(&column) {
                        def.sunset = Some(day.to_owned());
                    }
                } else {
                    notes.push(format!(
                        "SLA `endOfLife` on `{column}` dropped: `{date}` is not a date"
                    ));
                }
            }
            _ => unmapped_sla.push(sla.property.clone()),
        }
    }
    if !unmapped_sla.is_empty() {
        unmapped_sla.sort();
        unmapped_sla.dedup();
        notes.push(format!(
            "SLA properties {} describe operations, not a delivery, and have no PlexusPact check",
            unmapped_sla
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let settings = doc
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactSettings")
        .and_then(|p| serde_yaml::from_value::<Settings>(p.value.clone()).ok())
        .unwrap_or_default();
    let consumers: Vec<Consumer> = doc
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactConsumers")
        .and_then(|p| serde_yaml::from_value(p.value.clone()).ok())
        .unwrap_or_default();

    let migration = doc
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactMigration")
        .and_then(|p| serde_yaml::from_value::<Migration>(p.value.clone()).ok());

    let contract = Contract {
        api_version: ApiVersion::V1,
        dataset,
        owner,
        description,
        version: doc.version.clone(),
        consumers,
        columns,
        primary_key,
        dataset_checks,
        migration,
        settings,
    };
    Ok((contract, notes))
}

/// [`import`], with the contract rendered as native PlexusPact YAML — what a
/// CLI writes to disk or sends to a registry.
pub fn import_yaml(content: &str) -> Result<(String, Vec<String>), String> {
    let (contract, notes) = import(content)?;
    let yaml = serde_yaml::to_string(&contract)
        .map_err(|e| format!("failed to render the imported contract: {e}"))?;
    Ok((yaml, notes))
}

/// Maps one ODCS property to a column definition; `None` when the logical
/// type has no tabular equivalent (object/array).
fn import_column(prop: &OdcsProperty, notes: &mut Vec<String>) -> Option<ColumnDef> {
    let r#type = column_type(prop)?;
    let mut checks: Vec<ColumnCheck> = Vec::new();
    let mut required = prop.required.unwrap_or(false);

    if prop.unique == Some(true) {
        checks.push(ColumnCheck::Unique {
            approx: false,
            severity: Severity::Error,
        });
    }

    if let Some(options) = &prop.logical_type_options {
        if let Some(min) = &options.minimum {
            push_bound(min, true, &prop.name, &mut checks, notes);
        }
        if let Some(max) = &options.maximum {
            push_bound(max, false, &prop.name, &mut checks, notes);
        }
        if let Some(pattern) = &options.pattern {
            checks.push(ColumnCheck::Regex {
                regex: pattern.clone(),
                severity: Severity::Error,
            });
        }
        if options.min_length.is_some() || options.max_length.is_some() {
            let length = match (options.min_length, options.max_length) {
                (Some(a), Some(b)) if a == b => LengthSpec::Exact(a),
                (min, max) => LengthSpec::Range(LengthRange { min, max }),
            };
            checks.push(ColumnCheck::Length {
                length,
                severity: Severity::Error,
            });
        }
        if let Some(format) = &options.format {
            match serde_yaml::from_str::<KnownFormat>(format) {
                Ok(known) => checks.push(ColumnCheck::Format {
                    format: known,
                    severity: Severity::Error,
                }),
                Err(_) => notes.push(format!(
                    "format `{format}` on `{}` is not a PlexusPact format; add a `regex` check if it matters",
                    prop.name
                )),
            }
        }
    }

    for q in &prop.quality {
        match parse_engine_quality::<ColumnCheck>(q) {
            EngineQuality::Parsed(check) => checks.push(check),
            EngineQuality::Broken(err) => notes.push(format!(
                "quality entry on `{}` could not be parsed: {err}",
                prop.name
            )),
            EngineQuality::Foreign => match column_metric(q) {
                Some(ColumnMetric::NoNulls) => required = true,
                Some(ColumnMetric::NoDuplicates) => {
                    if !checks
                        .iter()
                        .any(|c| matches!(c, ColumnCheck::Unique { approx: false, .. }))
                    {
                        checks.push(ColumnCheck::Unique {
                            approx: false,
                            severity: Severity::Error,
                        });
                    }
                }
                None => notes.push(foreign_quality_note(q, &prop.name)),
            },
        }
    }

    let classification = prop.classification.as_ref().and_then(|c| {
        let parsed = serde_yaml::from_str::<DataClass>(&c.to_ascii_lowercase()).ok();
        if parsed.is_none() {
            notes.push(format!(
                "classification `{c}` on `{}` is not one of public/internal/confidential/restricted and was dropped",
                prop.name
            ));
        }
        parsed
    });
    let pii = prop
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactPii")
        .and_then(|p| serde_yaml::from_value::<PiiKind>(p.value.clone()).ok());
    let stability = prop
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactStability")
        .and_then(|p| serde_yaml::from_value::<Stability>(p.value.clone()).ok())
        .unwrap_or_default();
    // Documents exported before the date moved to `endOfLife` carry it here.
    let sunset = prop
        .custom_properties
        .iter()
        .find(|p| p.property == "plexuspactSunset")
        .and_then(|p| p.value.as_str().map(str::to_owned));

    Some(ColumnDef {
        r#type,
        description: prop.description.clone(),
        required,
        pii,
        classification,
        stability,
        sunset,
        checks,
    })
}

/// Resolves the PlexusPact column type: exact `physicalType` names win, then
/// the ODCS logical type, with timestamps sniffed out of the physical name.
fn column_type(prop: &OdcsProperty) -> Option<ColType> {
    let physical = prop
        .physical_type
        .as_deref()
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match physical.as_str() {
        "string" => return Some(ColType::String),
        "int" => return Some(ColType::Int),
        "float" => return Some(ColType::Float),
        "bool" => return Some(ColType::Bool),
        "date" => return Some(ColType::Date),
        "datetime" => return Some(ColType::Datetime),
        _ => {}
    }
    match prop.logical_type.as_deref().unwrap_or("string") {
        "string" => Some(ColType::String),
        "integer" => Some(ColType::Int),
        "number" => Some(ColType::Float),
        "boolean" => Some(ColType::Bool),
        "date" => {
            let temporal_hint = physical.contains("timestamp") || physical.contains("datetime");
            Some(if temporal_hint {
                ColType::Datetime
            } else {
                ColType::Date
            })
        }
        _ => None,
    }
}

/// Adds a `min`/`max` check from an ODCS `minimum`/`maximum` value.
fn push_bound(
    value: &serde_yaml::Value,
    is_min: bool,
    column: &str,
    checks: &mut Vec<ColumnCheck>,
    notes: &mut Vec<String>,
) {
    match serde_yaml::from_value::<Number>(value.clone()) {
        Ok(n) => checks.push(if is_min {
            ColumnCheck::Min {
                min: n,
                severity: Severity::Error,
            }
        } else {
            ColumnCheck::Max {
                max: n,
                severity: Severity::Error,
            }
        }),
        Err(_) => notes.push(format!(
            "{} on `{column}` is not numeric and was dropped",
            if is_min { "minimum" } else { "maximum" }
        )),
    }
}

/// A `references` check from one ODCS relationship. `source` is the property
/// a column-level relationship sits on; object-level ones list `from`.
fn references_check(
    rel: &OdcsRelationship,
    source: Option<&str>,
    object: &str,
    columns: &IndexMap<String, ColumnDef>,
) -> Result<DatasetCheck, String> {
    if let Some(kind) = rel.kind.as_deref() {
        if !kind.eq_ignore_ascii_case("foreignKey") {
            return Err(format!("type `{kind}` is not a foreign key"));
        }
    }
    let from: Vec<String> = match source {
        Some(name) if rel.from.is_empty() => vec![name.to_owned()],
        _ => rel.from.clone(),
    };
    if from.is_empty() {
        return Err("no `from` columns".to_owned());
    }
    if rel.to.is_empty() {
        return Err("no `to` columns".to_owned());
    }
    if from.len() != rel.to.len() {
        return Err(format!(
            "{} `from` columns against {} `to` columns",
            from.len(),
            rel.to.len()
        ));
    }
    let mut local = Vec::with_capacity(from.len());
    for f in &from {
        let column = local_column(f, object, columns)
            .ok_or_else(|| format!("`{f}` is not a column of `{object}`"))?;
        local.push(column);
    }
    let mut dataset: Option<String> = None;
    let mut to = Vec::with_capacity(rel.to.len());
    for t in &rel.to {
        let (other, column) = t
            .split_once('.')
            .map(|(o, c)| (o.trim(), c.trim()))
            .filter(|(o, c)| !o.is_empty() && !c.is_empty())
            .ok_or_else(|| format!("`{t}` is not written as `dataset.column`"))?;
        match &dataset {
            None => dataset = Some(other.to_owned()),
            Some(d) if d != other => {
                return Err(format!("`to` names two datasets, `{d}` and `{other}`"))
            }
            Some(_) => {}
        }
        to.push(column.to_owned());
    }
    let dataset = dataset.ok_or_else(|| "no `to` columns".to_owned())?;
    Ok(DatasetCheck::References {
        to,
        columns: local,
        dataset,
        severity: Severity::Error,
    })
}

/// The column an element path names, when it is one of this object's. Bare
/// names are accepted; `object.column` is stripped of the object; a path
/// into another object is not local.
fn local_column(
    element: &str,
    object: &str,
    columns: &IndexMap<String, ColumnDef>,
) -> Option<String> {
    let element = element.trim();
    if columns.contains_key(element) {
        return Some(element.to_owned());
    }
    let rest = element.strip_prefix(object)?.strip_prefix('.')?;
    columns.contains_key(rest).then(|| rest.to_owned())
}

/// A scalar YAML value as text (numbers, strings, dates alike).
fn yaml_scalar(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_owned(),
    }
}

/// `value` + `unit` as a duration. Units follow the spec's examples and
/// humantime's spellings; `M` alone is a month, `m` a minute.
fn parse_sla_duration(value: &serde_yaml::Value, unit: Option<&str>) -> Option<Duration> {
    let n: f64 = match value {
        serde_yaml::Value::Number(n) => n.as_f64()?,
        serde_yaml::Value::String(s) => s.trim().parse().ok()?,
        _ => return None,
    };
    if !n.is_finite() || n < 0.0 {
        return None;
    }
    let unit = unit?.trim();
    let secs_per_unit: f64 = if unit == "M" {
        30.0 * 86_400.0
    } else {
        match unit.to_ascii_lowercase().as_str() {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
            "d" | "day" | "days" => 86_400.0,
            "w" | "wk" | "week" | "weeks" => 7.0 * 86_400.0,
            "mo" | "month" | "months" => 30.0 * 86_400.0,
            "y" | "yr" | "yrs" | "year" | "years" => 365.0 * 86_400.0,
            _ => return None,
        }
    };
    Some(Duration::from_secs((n * secs_per_unit).round() as u64))
}

/// A whole number out of a comparison value (`120`, `120.0`, `"120"`).
fn whole_number(value: &serde_yaml::Value) -> Option<u64> {
    match value {
        serde_yaml::Value::Number(n) => n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64)
        }),
        serde_yaml::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The library metric a quality entry names, in its 3.1 (`metric`) or 3.0
/// (`rule`) spelling, lower-cased.
fn library_metric(q: &OdcsQuality) -> Option<String> {
    let library = q
        .kind
        .as_deref()
        .is_none_or(|k| k.eq_ignore_ascii_case("library"));
    if !library {
        return None;
    }
    q.metric
        .as_deref()
        .or(q.rule.as_deref())
        .map(|m| m.trim().to_ascii_lowercase())
}

/// Severity of a foreign quality entry: anything that reads as a warning is
/// one; the default is to fail, as in the spec's examples.
fn quality_severity(q: &OdcsQuality) -> Severity {
    match q
        .severity
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("warn") | Some("warning") | Some("info") | Some("low") => Severity::Warn,
        _ => Severity::Error,
    }
}

/// Row-count checks from a standard `rowCount` library rule. `Ok(empty)`
/// means the entry is not a row-count rule at all.
fn row_count_checks(q: &OdcsQuality) -> Result<Vec<DatasetCheck>, String> {
    if library_metric(q).as_deref() != Some("rowcount") {
        return Ok(Vec::new());
    }
    if q.unit
        .as_deref()
        .is_some_and(|u| u.eq_ignore_ascii_case("percent"))
    {
        return Err("a row count in percent has nothing to be a percentage of".to_owned());
    }
    let severity = quality_severity(q);
    let number = |v: &serde_yaml::Value| {
        whole_number(v).ok_or_else(|| format!("`{}` is not a whole number", yaml_scalar(v)))
    };
    let mut min = None;
    let mut max = None;
    if let Some(v) = &q.must_be {
        let n = number(v)?;
        min = Some(n);
        max = Some(n);
    }
    if let Some(v) = &q.must_be_greater_or_equal_to {
        min = Some(number(v)?);
    }
    if let Some(v) = &q.must_be_greater_than {
        min = Some(number(v)? + 1);
    }
    if let Some(v) = &q.must_be_less_or_equal_to {
        max = Some(number(v)?);
    }
    if let Some(v) = &q.must_be_less_than {
        let n = number(v)?;
        max = Some(n.checked_sub(1).ok_or("fewer than 0 rows is impossible")?);
    }
    if let Some(range) = &q.must_be_between {
        match range.as_slice() {
            [low, high] => {
                min = Some(number(low)?);
                max = Some(number(high)?);
            }
            _ => return Err("`mustBeBetween` needs exactly two numbers".to_owned()),
        }
    }
    if q.must_not_be.is_some() || q.must_not_be_between.is_some() {
        return Err(
            "PlexusPact bounds a row count from below and above, not by exclusion".to_owned(),
        );
    }
    let mut checks = Vec::new();
    if let Some(count) = min {
        checks.push(DatasetCheck::RowCountMin { count, severity });
    }
    if let Some(count) = max {
        checks.push(DatasetCheck::RowCountMax { count, severity });
    }
    if checks.is_empty() {
        return Err("no comparison (`mustBe…`) given".to_owned());
    }
    Ok(checks)
}

/// What a column-level library rule amounts to in contract terms.
enum ColumnMetric {
    /// `nullValues` must be 0: the column is required.
    NoNulls,
    /// `duplicateValues` must be 0: the column is unique.
    NoDuplicates,
}

fn column_metric(q: &OdcsQuality) -> Option<ColumnMetric> {
    let zero = |v: &Option<serde_yaml::Value>| v.as_ref().and_then(whole_number) == Some(0);
    let none_allowed = zero(&q.must_be)
        || zero(&q.must_be_less_or_equal_to)
        || q.must_be_less_than.as_ref().and_then(whole_number) == Some(1);
    if !none_allowed {
        return None;
    }
    match library_metric(q)?.as_str() {
        "nullvalues" | "missingvalues" => Some(ColumnMetric::NoNulls),
        "duplicatevalues" | "duplicatecount" => Some(ColumnMetric::NoDuplicates),
        _ => None,
    }
}

/// Result of inspecting one quality entry for a PlexusPact implementation.
enum EngineQuality<T> {
    /// A `type: custom, engine: plexuspact` entry that parsed cleanly.
    Parsed(T),
    /// Ours by engine name, but the implementation did not parse.
    Broken(String),
    /// Another tool's rule (text/sql/library or a different engine).
    Foreign,
}

fn parse_engine_quality<T: serde::de::DeserializeOwned>(q: &OdcsQuality) -> EngineQuality<T> {
    let ours = q
        .engine
        .as_deref()
        .is_some_and(|e| e.eq_ignore_ascii_case(ENGINE));
    if !ours {
        return EngineQuality::Foreign;
    }
    let Some(implementation) = q.implementation.as_deref() else {
        return EngineQuality::Broken("missing `implementation`".to_owned());
    };
    match serde_yaml::from_str(implementation) {
        Ok(check) => EngineQuality::Parsed(check),
        Err(e) => EngineQuality::Broken(e.to_string()),
    }
}

fn foreign_quality_note(q: &OdcsQuality, target: &str) -> String {
    let kind = q.kind.as_deref().unwrap_or("unknown");
    let what = q
        .metric
        .as_deref()
        .or(q.rule.as_deref())
        .or(q.description.as_deref())
        .unwrap_or("unnamed rule");
    format!("quality rule `{what}` (type {kind}) on {target} is not executable by PlexusPact and was dropped")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::parse_str;

    /// A contract exercising every mapping path: portable options, custom
    /// quality entries, PII, classification, settings, consumers, dataset
    /// checks in both severities, a foreign key, a freshness promise, a
    /// retirement.
    fn rich_contract() -> Contract {
        let yaml = r#"
apiVersion: v1
dataset: user_signups
owner: growth-team@acme.com
description: Daily signup export.
version: "1.2.0"
consumers:
  - { name: analytics-core, contact: data-team@acme.com }
columns:
  user_id:
    type: string
    description: Stable account identifier, never reused.
    required: true
    checks: [unique, { length: 12 }]
  email:
    type: string
    required: true
    pii: email
    classification: confidential
    checks: [{ format: email }, not_empty_string]
  age:
    type: int
    checks: [{ min: 18 }, { max: 120 }, { null_ratio_max: 0.1, severity: warn }]
  country:
    type: string
    checks: [{ regex: "^[A-Z]{2}$" }, { enum: [DE, IN, US] }]
  legacy_ref:
    type: string
    stability: deprecated
    sunset: "2027-03-31"
  signed_up:
    type: datetime
    required: true
primary_key: [user_id]
dataset_checks:
  - row_count_min: 1
  - row_count_max: 5000000
    severity: warn
  - assert: "COUNT(*) >= 1"
  - references: { columns: [country], dataset: countries, to: [code], severity: warn }
  - references: { columns: [user_id], dataset: accounts }
  - freshness: { column: signed_up, max_age: 48h }
  - freshness: { column: signed_up, max_age: 90m, severity: warn }
settings:
  allow_extra_columns: false
"#;
        parse_str(yaml, "test.yaml").unwrap()
    }

    #[test]
    fn export_uses_portable_slots() {
        let doc = export(&rich_contract(), "test-id");
        assert_eq!(doc.api_version, ODCS_API_VERSION);
        assert_eq!(doc.kind, "DataContract");
        assert_eq!(doc.name.as_deref(), Some("user_signups"));
        assert_eq!(
            doc.team[0].username.as_deref(),
            Some("growth-team@acme.com")
        );

        let object = &doc.schema[0];
        let props = &object.properties;
        let age = props.iter().find(|p| p.name == "age").unwrap();
        let opts = age.logical_type_options.as_ref().unwrap();
        assert_eq!(opts.minimum, serde_yaml::to_value(18).ok());
        assert_eq!(opts.maximum, serde_yaml::to_value(120).ok());
        // warn-severity null_ratio_max stays a custom quality entry
        assert_eq!(age.quality.len(), 1);
        assert_eq!(age.quality[0].engine.as_deref(), Some(ENGINE));

        let user_id = props.iter().find(|p| p.name == "user_id").unwrap();
        assert_eq!(user_id.unique, Some(true));
        assert_eq!(user_id.primary_key, Some(true));
        assert_eq!(user_id.primary_key_position, Some(1));
        assert_eq!(
            props.iter().filter(|p| p.primary_key == Some(true)).count(),
            1
        );
        assert_eq!(
            user_id.description.as_deref(),
            Some("Stable account identifier, never reused.")
        );
        let u_opts = user_id.logical_type_options.as_ref().unwrap();
        assert_eq!((u_opts.min_length, u_opts.max_length), (Some(12), Some(12)));

        let email = props.iter().find(|p| p.name == "email").unwrap();
        assert_eq!(email.classification.as_deref(), Some("confidential"));
        assert_eq!(
            email
                .logical_type_options
                .as_ref()
                .unwrap()
                .format
                .as_deref(),
            Some("email")
        );

        let signed_up = props.iter().find(|p| p.name == "signed_up").unwrap();
        assert_eq!(signed_up.logical_type.as_deref(), Some("date"));
        assert_eq!(signed_up.physical_type.as_deref(), Some("datetime"));

        // The error-severity foreign key is a native relationship, the warn
        // one rides custom.
        assert_eq!(object.relationships.len(), 1);
        let rel = &object.relationships[0];
        assert_eq!(rel.kind.as_deref(), Some("foreignKey"));
        assert_eq!(rel.from, vec!["user_signups.user_id"]);
        assert_eq!(rel.to, vec!["accounts.user_id"]);

        // row_count_min (error) is the standard rowCount metric; the warn
        // row_count_max, assert, warn references and warn freshness ride custom.
        let row_count = object
            .quality
            .iter()
            .find(|q| q.metric.as_deref() == Some("rowCount"))
            .unwrap();
        assert_eq!(row_count.kind.as_deref(), Some("library"));
        assert_eq!(
            row_count.must_be_greater_or_equal_to,
            Some(serde_yaml::Value::from(1u64))
        );
        assert_eq!(
            object
                .quality
                .iter()
                .filter(|q| q.engine.as_deref() == Some(ENGINE))
                .count(),
            4
        );

        // SLA: latency for the error freshness, endOfLife for the sunset.
        let latency = doc
            .sla_properties
            .iter()
            .find(|s| s.property == "latency")
            .unwrap();
        // 48h is two whole days, so the exporter writes the larger unit.
        assert_eq!(latency.value, serde_yaml::Value::from(2u64));
        assert_eq!(latency.unit.as_deref(), Some("d"));
        assert_eq!(latency.element.as_deref(), Some("user_signups.signed_up"));
        let eol = doc
            .sla_properties
            .iter()
            .find(|s| s.property == "endOfLife")
            .unwrap();
        assert_eq!(eol.element.as_deref(), Some("user_signups.legacy_ref"));
        assert_eq!(eol.value.as_str(), Some("2027-03-31"));
        let legacy = props.iter().find(|p| p.name == "legacy_ref").unwrap();
        assert!(legacy
            .custom_properties
            .iter()
            .all(|p| p.property != "plexuspactSunset"));
    }

    #[test]
    fn roundtrip_preserves_contract() {
        let original = rich_contract();
        let yaml = export_yaml(&original, "test-id").unwrap();
        let (imported, notes) = import(&yaml).unwrap();
        assert!(notes.is_empty(), "unexpected notes: {notes:?}");
        // Native slots come back in a different order than they were written
        // (relationships and SLA entries are read after the quality list);
        // the set of checks is what has to survive.
        let mut want = original.clone();
        let mut got = imported.clone();
        want.dataset_checks.sort_by_key(|c| format!("{c:?}"));
        got.dataset_checks.sort_by_key(|c| format!("{c:?}"));
        assert_eq!(got, want);
    }

    #[test]
    fn looks_like_odcs_tells_documents_apart() {
        assert!(looks_like_odcs(
            "apiVersion: v3.1.0\nkind: DataContract\nid: x\nschema: []\n"
        ));
        assert!(looks_like_odcs(
            "apiVersion: v3.0.2\nschema:\n  - name: t\n"
        ));
        assert!(!looks_like_odcs(
            "apiVersion: v1\ndataset: t\ncolumns: {}\n"
        ));
        assert!(!looks_like_odcs("{{not yaml"));
    }

    #[test]
    fn imports_foreign_odcs_document() {
        let odcs = r#"
apiVersion: v3.1.0
kind: DataContract
id: orders-contract
name: orders
version: "2.0.0"
description:
  purpose: Order events from the commerce platform.
  usage: Analytics only.
team:
  - username: alice@shop.example
    role: Data Owner
  - username: bob@shop.example
    role: steward
schema:
  - name: orders
    logicalType: object
    relationships:
      - type: foreignKey
        from: [orders.customer_id, orders.region]
        to: [customers.id, customers.region_code]
      - to: [warehouses.id, warehouses.site]
        from: [orders.status]
    properties:
      - name: order_id
        logicalType: string
        required: true
        unique: true
        primaryKey: true
        primaryKeyPosition: 2
      - name: amount
        logicalType: number
        logicalTypeOptions: { minimum: 0 }
      - name: created_at
        logicalType: date
        physicalType: timestamp_ntz
        primaryKey: true
        primaryKeyPosition: 1
      - name: customer_id
        logicalType: string
        quality:
          - metric: nullValues
            mustBe: 0
      - name: region
        logicalType: string
      - name: sku
        logicalType: string
        relationships:
          - to: products.sku
        quality:
          - type: library
            metric: duplicateValues
            mustBe: 0
      - name: metadata
        logicalType: object
      - name: status
        logicalType: string
        classification: Internal
        quality:
          - type: library
            rule: validValues
    quality:
      - type: sql
        description: rowcount over 100
      - metric: rowCount
        mustBeBetween: [100, 5000]
        severity: warning
      - type: library
        rule: rowCount
        mustBeGreaterThan: 9
slaProperties:
  - property: latency
    value: 2
    unit: d
    element: orders.created_at
  - property: endOfLife
    value: "2030-06-30T00:00:00-08:00"
    element: orders.region
  - property: frequency
    value: 1
    unit: d
    element: orders.created_at
  - property: retention
    value: 3
    unit: y
"#;
        let (contract, notes) = import(odcs).unwrap();
        assert_eq!(contract.dataset, "orders");
        assert_eq!(contract.owner.as_deref(), Some("alice@shop.example"));
        assert_eq!(contract.version.as_deref(), Some("2.0.0"));
        assert_eq!(
            contract.description.as_deref(),
            Some("Order events from the commerce platform.")
        );
        // object-typed column skipped, others mapped
        assert_eq!(contract.columns.len(), 7);
        assert!(contract.columns["order_id"].required);
        assert_eq!(contract.columns["created_at"].r#type, ColType::Datetime);
        // a composite key comes back in ODCS position order, not file order
        assert_eq!(contract.primary_key, vec!["created_at", "order_id"]);
        assert_eq!(
            contract.columns["status"].classification,
            Some(DataClass::Internal)
        );
        assert_eq!(
            contract.columns["amount"].checks,
            vec![ColumnCheck::Min {
                min: Number::Int(0),
                severity: Severity::Error
            }]
        );
        // library metrics on columns: no nulls → required, no duplicates → unique
        assert!(contract.columns["customer_id"].required);
        assert_eq!(
            contract.columns["sku"].checks,
            vec![ColumnCheck::Unique {
                approx: false,
                severity: Severity::Error
            }]
        );
        // endOfLife on a column is its sunset, date only
        assert_eq!(
            contract.columns["region"].sunset.as_deref(),
            Some("2030-06-30")
        );

        let checks = &contract.dataset_checks;
        assert!(checks.contains(&DatasetCheck::RowCountMin {
            count: 100,
            severity: Severity::Warn
        }));
        assert!(checks.contains(&DatasetCheck::RowCountMax {
            count: 5000,
            severity: Severity::Warn
        }));
        assert!(checks.contains(&DatasetCheck::RowCountMin {
            count: 10,
            severity: Severity::Error
        }));
        assert!(checks.contains(&DatasetCheck::References {
            columns: vec!["customer_id".to_owned(), "region".to_owned()],
            dataset: "customers".to_owned(),
            to: vec!["id".to_owned(), "region_code".to_owned()],
            severity: Severity::Error
        }));
        assert!(checks.contains(&DatasetCheck::References {
            columns: vec!["sku".to_owned()],
            dataset: "products".to_owned(),
            to: vec!["sku".to_owned()],
            severity: Severity::Error
        }));
        assert!(checks.contains(&DatasetCheck::Freshness {
            column: "created_at".to_owned(),
            max_age: Duration::from_secs(2 * 86_400),
            severity: Severity::Error
        }));
        // one `from` column against two `to` columns cannot be a key
        assert!(!checks.iter().any(
            |c| matches!(c, DatasetCheck::References { dataset, .. } if dataset == "warehouses")
        ));

        // notes: usage dropped, metadata skipped, foreign quality ×2 (sql,
        // validValues), the lopsided relationship, the unmapped SLA properties
        assert_eq!(notes.len(), 6, "notes: {notes:?}");
        assert!(notes.iter().any(|n| n.contains("`frequency`, `retention`")));
        assert!(notes.iter().any(|n| n.contains("relationship dropped")));

        // the imported draft parses + round-trips through the real model
        let yaml = serde_yaml::to_string(&contract).unwrap();
        parse_str(&yaml, "imported.yaml").unwrap();
    }

    #[test]
    fn import_reads_older_exports() {
        // A document exported before `sunset` moved to `endOfLife` and with
        // the 3.0 default element for SLA properties.
        let odcs = r#"
apiVersion: v3.0.2
kind: DataContract
id: legacy
slaDefaultElement: events.happened_at
slaProperties:
  - property: latency
    value: 36
    unit: hours
schema:
  - name: events
    properties:
      - name: happened_at
        logicalType: date
        physicalType: datetime
      - name: old_code
        logicalType: string
        customProperties:
          - property: plexuspactSunset
            value: "2026-12-31"
"#;
        let (contract, notes) = import(odcs).unwrap();
        assert!(notes.is_empty(), "notes: {notes:?}");
        assert_eq!(
            contract.columns["old_code"].sunset.as_deref(),
            Some("2026-12-31")
        );
        assert_eq!(
            contract.dataset_checks,
            vec![DatasetCheck::Freshness {
                column: "happened_at".to_owned(),
                max_age: Duration::from_secs(36 * 3_600),
                severity: Severity::Error
            }]
        );
    }

    #[test]
    fn import_rejects_garbage_and_empty() {
        assert!(import("{{not yaml").is_err());
        assert!(import("apiVersion: v3.0.2\nkind: DataContract\nid: x").is_err());
    }

    #[test]
    fn sla_duration_picks_the_largest_whole_unit() {
        assert_eq!(sla_duration(Duration::from_secs(172_800)), (2, "d"));
        assert_eq!(sla_duration(Duration::from_secs(5_400)), (90, "m"));
        assert_eq!(sla_duration(Duration::from_secs(90)), (90, "s"));
        assert_eq!(
            parse_sla_duration(&serde_yaml::Value::from(1.5), Some("d")),
            Some(Duration::from_secs(129_600))
        );
        assert_eq!(parse_sla_duration(&serde_yaml::Value::from(4), None), None);
    }
}
