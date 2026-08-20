//! JSON Schema generation for the contract file (published for editor
//! autocomplete via `yaml.schemas`, task 1.7).

use schemars::schema_for;

use crate::model::Contract;

/// Generates the JSON Schema (draft-07) for `contract.yaml` as a JSON value.
///
/// Output is stable for a given crate version: schemars keeps definitions in
/// sorted maps, so the same input always serializes identically.
pub fn json_schema() -> serde_json::Value {
    let schema = schema_for!(Contract);
    serde_json::to_value(&schema).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn schema_generates_and_is_not_the_fallback() {
        let schema = json_schema();
        assert!(schema.is_object(), "schema generation fell back to Null");
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .unwrap();
        for key in [
            "apiVersion",
            "dataset",
            "owner",
            "description",
            "version",
            "consumers",
            "columns",
            "dataset_checks",
            "settings",
        ] {
            assert!(
                props.contains_key(key),
                "missing top-level property `{key}`"
            );
        }
        let required = schema.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v == "apiVersion"));
        assert!(required.iter().any(|v| v == "dataset"));
    }

    #[test]
    fn column_check_schema_has_both_forms() {
        let schema = json_schema();
        let defs = schema
            .get("definitions")
            .and_then(|d| d.as_object())
            .unwrap();
        let check = defs.get("ColumnCheck").unwrap();
        let any_of = check.get("anyOf").and_then(|a| a.as_array()).unwrap();
        // The bare-string form must be present…
        let string_form = any_of
            .iter()
            .find(|s| s.get("type").is_some_and(|t| t == "string"))
            .expect("no bare-string form in ColumnCheck schema");
        let names = string_form.get("enum").unwrap().as_array().unwrap();
        assert!(names.iter().any(|v| v == "unique"));
        assert!(names.iter().any(|v| v == "not_empty_string"));
        // …plus one map form per parameterized check.
        let map_keys: Vec<String> = any_of
            .iter()
            .filter_map(|s| s.get("required"))
            .filter_map(|r| r.as_array())
            .filter_map(|r| r.first())
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        for key in [
            "min",
            "max",
            "regex",
            "enum",
            "length",
            "format",
            "null_ratio_max",
            "unique_ratio_min",
            "custom_expr",
            "unique",
        ] {
            assert!(
                map_keys.contains(&key.to_string()),
                "missing map form for `{key}`"
            );
        }
    }

    #[test]
    fn schema_is_deterministic() {
        assert_eq!(json_schema(), json_schema());
    }
}
