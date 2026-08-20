//! The canonical contract must validate against the generated JSON Schema
//! (real validation via the `jsonschema` crate).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;

use plexuspact_contract::{json_schema, parse_file};

fn fixture_yaml() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/contract.yaml");
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn canonical_contract_validates_against_generated_schema() {
    let schema = json_schema();
    let validator = jsonschema::validator_for(&schema).expect("generated schema must compile");

    // Validate the raw fixture (as-authored YAML → JSON).
    let instance: serde_json::Value = serde_yaml::from_str(&fixture_yaml()).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect();
    assert!(
        errors.is_empty(),
        "fixture failed schema validation:\n{errors:#?}"
    );
}

#[test]
fn serialized_contract_validates_against_generated_schema() {
    let schema = json_schema();
    let validator = jsonschema::validator_for(&schema).unwrap();

    // Validate our own serialization of the parsed contract too, so the
    // Serialize impls and the schema can never drift apart silently.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/contract.yaml");
    let contract = parse_file(path).unwrap();
    let instance = serde_json::to_value(&contract).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&instance)
        .map(|e| format!("{} at {}", e, e.instance_path))
        .collect();
    assert!(
        errors.is_empty(),
        "serialized contract failed schema validation:\n{errors:#?}"
    );
}

#[test]
fn schema_rejects_bad_contracts() {
    let schema = json_schema();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let missing_dataset = serde_json::json!({ "apiVersion": "v1", "columns": {} });
    assert!(!validator.is_valid(&missing_dataset));

    let bad_type = serde_json::json!({
        "apiVersion": "v1",
        "dataset": "t",
        "columns": { "id": { "type": "text" } }
    });
    assert!(!validator.is_valid(&bad_type));

    let bad_check = serde_json::json!({
        "apiVersion": "v1",
        "dataset": "t",
        "columns": { "id": { "type": "string", "checks": ["uniqueish"] } }
    });
    assert!(!validator.is_valid(&bad_check));
}
