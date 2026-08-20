//! End-to-end CLI tests: every command × its exit codes (PRD FR-7 / ADR-004).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;

use assert_cmd::Command;
use predicates::prelude::*;

const CONTRACT: &str = "../../fixtures/contract.yaml";
const DATA: &str = "../../fixtures/signups_small.csv";

fn bin() -> Command {
    let mut cmd = Command::cargo_bin("plexuspact").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd
}

fn write_temp(name: &str, contents: &str) -> tempfile::TempPath {
    let mut f = tempfile::Builder::new().suffix(name).tempfile().unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f.into_temp_path()
}

#[test]
fn check_failing_data_exits_1() {
    bin()
        .args([
            "check",
            DATA,
            "--contract",
            CONTRACT,
            "--now",
            "2026-07-10T00:00:00Z",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("checks failed"))
        .stdout(predicate::str::contains("bob@@example"));
}

#[test]
fn check_json_is_valid_and_versioned() {
    let out = bin()
        .args([
            "check",
            DATA,
            "--contract",
            CONTRACT,
            "--format",
            "json",
            "--now",
            "2026-07-10T00:00:00Z",
        ])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("valid JSON");
    assert_eq!(v["result_schema_version"], 1);
    assert_eq!(v["status"], "failed");
    assert_eq!(v["contract"]["dataset"], "user_signups");
}

#[test]
fn check_missing_data_file_exits_2() {
    bin()
        .args([
            "check",
            "../../fixtures/does_not_exist.csv",
            "--contract",
            CONTRACT,
        ])
        .assert()
        .code(2);
}

#[test]
fn check_missing_contract_exits_2() {
    bin()
        .args(["check", DATA, "--contract", "../../fixtures/nope.yaml"])
        .assert()
        .code(2);
}

#[test]
fn check_clean_data_exits_0() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n2,bob\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("passed"));
}

#[test]
fn validate_good_contract_exits_0() {
    bin().args(["validate-contract", CONTRACT]).assert().code(0);
}

#[test]
fn validate_broken_contract_exits_2() {
    let bad = write_temp(
        "bad.yaml",
        "apiVersion: v1\ndataset: x\ncolumns:\n  a: { type: notatype }\n",
    );
    bin().arg("validate-contract").arg(&bad).assert().code(2);
}

#[test]
fn diff_breaking_change_exits_1() {
    let old = write_temp("old.yaml", CLEAN_CONTRACT);
    // Narrow the type / add a required column ⇒ breaking.
    let new = write_temp(
        "new.yaml",
        "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: int, required: true }\n  name: { type: string, required: true }\n  extra: { type: string, required: true }\n",
    );
    bin()
        .arg("diff")
        .arg(&old)
        .arg(&new)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("BREAKING"));
}

#[test]
fn diff_identical_contracts_exits_0() {
    let a = write_temp("a.yaml", CLEAN_CONTRACT);
    let b = write_temp("b.yaml", CLEAN_CONTRACT);
    bin()
        .arg("diff")
        .arg(&a)
        .arg(&b)
        .assert()
        .code(0)
        .stdout(predicate::str::contains("no changes"));
}

#[test]
fn init_drafts_parseable_contract() {
    let out = bin()
        .args(["init", DATA, "--dataset", "signups"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    assert!(yaml.contains("apiVersion: v1"));
    assert!(yaml.contains("dataset: signups"));
    // The draft must itself be a valid contract.
    plexuspact_contract::parse_str(&yaml, "draft").expect("init output parses");
}

#[test]
fn help_and_version_work() {
    bin()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("plexuspact"));
    bin().arg("--version").assert().code(0);
}

const CLEAN_CONTRACT: &str = "apiVersion: v1\n\
dataset: t\n\
columns:\n\
\x20 id: { type: int, required: true }\n\
\x20 name: { type: string, required: true }\n";

// ─────────────────────────── --json-path selector ───────────────────────────

/// A typical REST envelope: the records live under a wrapping key.
const WRAPPED: &str =
    r#"{"status":"ok","count":2,"results":[{"id":1,"name":"Ada"},{"id":2,"name":"Grace"}]}"#;

#[test]
fn json_path_extracts_wrapped_array() {
    let data = write_temp("wrapped.json", WRAPPED);
    let out = bin()
        .args(["init"])
        .arg(&data)
        .args([
            "--input-format",
            "json",
            "--json-path",
            "results",
            "--dataset",
            "w",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    // The two record fields — not the envelope's `status`/`count` — become columns.
    assert!(yaml.contains("id:"), "expected id column, got:\n{yaml}");
    assert!(yaml.contains("name:"), "expected name column, got:\n{yaml}");
    assert!(!yaml.contains("status:"), "envelope key leaked in:\n{yaml}");
    plexuspact_contract::parse_str(&yaml, "draft").expect("draft parses");
}

#[test]
fn json_path_check_validates_wrapped_records() {
    let data = write_temp("wrapped.json", WRAPPED);
    let contract = write_temp(
        "w.yaml",
        "apiVersion: v1\ndataset: w\ncolumns:\n  id: { type: int, required: true }\n  name: { type: string, required: true }\n",
    );
    bin()
        .args(["check"])
        .arg(&data)
        .args(["--contract"])
        .arg(&contract)
        .args(["--input-format", "json", "--json-path", "results"])
        .assert()
        .code(0);
}

#[test]
fn json_path_unknown_key_errors_with_available_keys() {
    let data = write_temp("wrapped.json", WRAPPED);
    bin()
        .args(["init"])
        .arg(&data)
        .args([
            "--input-format",
            "json",
            "--json-path",
            "resultz",
            "--dataset",
            "w",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no key `resultz`"))
        .stderr(predicate::str::contains("results"));
}

#[test]
fn json_path_rejected_on_non_json_format() {
    bin()
        .args(["init", DATA, "--json-path", "results"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("only applies to JSON input"));
}

#[test]
fn export_databricks_dlt_sql_from_contract() {
    let contract = write_temp(
        "c.yaml",
        "apiVersion: v1\ndataset: orders\ncolumns:\n  id: { type: int, required: true, checks: [{ min: 1 }] }\n  cid: { type: int, checks: [unique] }\ndataset_checks:\n  - row_count_min: 1\n",
    );
    let out = bin()
        .args(["export"])
        .arg(&contract)
        .args(["--target", "databricks-dlt"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let sql = String::from_utf8(out).unwrap();
    assert!(
        sql.contains("CONSTRAINT id_not_null EXPECT (`id` IS NOT NULL) ON VIOLATION FAIL UPDATE")
    );
    assert!(sql.contains("`id` IS NULL OR `id` >= 1"));
    // Aggregate checks are reported, never silently dropped.
    assert!(sql.contains("column `cid` check `unique`"));
    assert!(sql.contains("dataset check `row_count_min`"));
}

#[test]
fn export_rejects_broken_contract_with_exit_2() {
    let bad = write_temp(
        "bad.yaml",
        "apiVersion: v1\ndataset: x\ncolumns:\n  a: { type: notatype }\n",
    );
    bin()
        .args(["export"])
        .arg(&bad)
        .args(["--target", "databricks-dlt"])
        .assert()
        .code(2);
}

#[test]
fn openlineage_event_written_with_quality_facets() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\nx,bob\n"); // `x` fails id.type
    let event = tempfile::Builder::new()
        .suffix("ol.json")
        .tempfile()
        .unwrap()
        .into_temp_path();
    bin()
        .args(["check"])
        .arg(&data)
        .args(["--contract"])
        .arg(&contract)
        .args(["--openlineage"])
        .arg(&event)
        .args(["--openlineage-namespace", "s3://lake"])
        .assert()
        .code(1); // id.type fails → run fails
    let text = std::fs::read_to_string(&event).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid OpenLineage JSON");
    assert_eq!(v["eventType"], "FAIL");
    assert_eq!(v["inputs"][0]["namespace"], "s3://lake");
    assert_eq!(v["inputs"][0]["name"], "t");
    // Every check appears as an assertion; the failing one is marked unsuccessful.
    let assertions = v["inputs"][0]["facets"]["dataQualityAssertions"]["assertions"]
        .as_array()
        .unwrap();
    assert!(assertions
        .iter()
        .any(|a| a["assertion"] == "id.type" && a["success"] == false));
    assert_eq!(
        v["inputs"][0]["inputFacets"]["dataQualityMetrics"]["rowCount"],
        2
    );
}

#[test]
fn json_path_wraps_single_object_as_one_record() {
    // Single-record responses (`{ "record": {…} }`) validate as a 1-row table.
    let data = write_temp(
        "single.json",
        r#"{"meta":{"ok":true},"record":{"id":42,"name":"solo"}}"#,
    );
    let out = bin()
        .args(["init"])
        .arg(&data)
        .args([
            "--input-format",
            "json",
            "--json-path",
            "record",
            "--dataset",
            "s",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    assert!(
        yaml.contains("id:") && yaml.contains("name:"),
        "got:\n{yaml}"
    );
    plexuspact_contract::parse_str(&yaml, "draft").expect("draft parses");
}
