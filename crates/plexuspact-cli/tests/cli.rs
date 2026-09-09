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
    // Reporting switches itself on from the environment, so a developer with a
    // real key exported would otherwise have every test in this file try to
    // reach the network. Each test that wants reporting asks for it.
    cmd.env_remove("PLEXUSPACT_API_KEY");
    cmd.env_remove("PLEXUSPACT_API");
    cmd.env_remove("PLEXUSPACT_NO_NETWORK");
    cmd
}

/// A cloud that is definitely not there. Port 1 refuses immediately, so these
/// tests exercise the failure path without waiting on a timeout.
const NOWHERE: &str = "http://127.0.0.1:1/api/v1";

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

const ODCS_DOC: &str = "apiVersion: v3.1.0\n\
kind: DataContract\n\
id: orders-contract\n\
name: orders\n\
team:\n\
\x20 - { username: data-owner@example.test, role: owner }\n\
schema:\n\
\x20 - name: t\n\
\x20   properties:\n\
\x20     - { name: id, logicalType: integer, required: true, primaryKey: true }\n\
\x20     - { name: name, logicalType: string }\n\
\x20     - { name: blob, logicalType: object }\n\
\x20   relationships:\n\
\x20     - { type: foreignKey, from: [t.id], to: [accounts.id] }\n\
slaProperties:\n\
\x20 - { property: retention, value: 3, unit: y }\n";

#[test]
fn export_dbt_schema_as_source_and_model() {
    let contract = write_temp(
        "contract.yaml",
        "apiVersion: v1\n\
dataset: t\n\
columns:\n\
\x20 id: { type: int, required: true, checks: [unique, { min: 1 }] }\n\
\x20 name: { type: string, checks: [{ enum: [a, b], severity: warn }] }\n\
\x20 at: { type: datetime }\n\
dataset_checks:\n\
\x20 - { freshness: { column: at, max_age: 6h } }\n",
    );
    let out = bin()
        .args(["export"])
        .arg(&contract)
        .args(["--target", "dbt", "--source", "raw"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("sources:"), "{text}");
    assert!(text.contains("name: raw"), "{text}");
    assert!(text.contains("loaded_at_field: at"), "{text}");
    assert!(text.contains("period: hour"), "{text}");
    assert!(text.contains("- not_null"), "{text}");
    assert!(text.contains("plexuspact.min:"), "{text}");
    assert!(text.contains("severity: warn"), "{text}");

    let out = bin()
        .args(["export"])
        .arg(&contract)
        .args(["--target", "dbt"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("models:"), "{text}");
    assert!(text.contains("plexuspact.freshness:"), "{text}");
    assert!(!text.contains("loaded_at_field"), "{text}");
}

#[test]
fn export_odcs_document_from_contract() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let out = bin()
        .args(["export"])
        .arg(&contract)
        .args(["--target", "odcs", "--id", "urn:example:t"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(out).unwrap();
    assert!(text.contains("apiVersion: v3.1.0"), "{text}");
    assert!(text.contains("kind: DataContract"), "{text}");
    assert!(text.contains("id: urn:example:t"), "{text}");
    assert!(text.contains("logicalType: integer"), "{text}");
}

#[test]
fn import_odcs_document_writes_contract_and_notes() {
    let doc = write_temp("orders.odcs.yaml", ODCS_DOC);
    let out = tempfile::Builder::new()
        .suffix("contract.yaml")
        .tempfile()
        .unwrap()
        .into_temp_path();
    let assert = bin()
        .args(["import"])
        .arg(&doc)
        .arg("--out")
        .arg(&out)
        .assert()
        .success();
    let stderr = String::from_utf8(assert.get_output().stderr.clone()).unwrap();
    // what could not be mapped is said, what could is written
    assert!(stderr.contains("blob"), "{stderr}");
    assert!(stderr.contains("retention"), "{stderr}");
    let written = std::fs::read_to_string(&out).unwrap();
    assert!(written.contains("dataset: t"), "{written}");
    assert!(
        written.contains("owner: data-owner@example.test"),
        "{written}"
    );
    assert!(written.contains("references:"), "{written}");
    assert!(!written.contains("blob"), "{written}");

    // the written contract is a real one: it lints clean
    bin()
        .args(["validate-contract"])
        .arg(&out)
        .assert()
        .success();
}

#[test]
fn check_accepts_an_odcs_document_as_the_contract() {
    let doc = write_temp("orders.odcs.yaml", ODCS_DOC);
    let data = write_temp("data.csv", "id,name\n1,alice\n2,bob\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&doc)
        .args(["--reference"])
        .arg(format!("accounts={}", data.display()))
        .assert()
        .success()
        .stderr(predicate::str::contains("is an ODCS document"));
}

#[test]
fn import_rejects_a_document_that_is_not_odcs() {
    let bad = write_temp("bad.yaml", "just: text\n");
    bin().args(["import"]).arg(&bad).assert().code(2);
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

// ─────────────────────────── push / report home ──────────────────────────

#[test]
fn push_without_a_token_is_a_usage_error() {
    let result = write_temp("result.json", r#"{"result_schema_version":1}"#);
    bin()
        .args(["push"])
        .arg(&result)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("PLEXUSPACT_API_KEY"));
}

#[test]
fn requiring_a_push_without_a_token_is_a_usage_error() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .arg("--push")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("PLEXUSPACT_API_KEY"));
}

#[test]
fn an_unreachable_cloud_never_changes_a_clean_verdict() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("could not reach"));
}

#[test]
fn an_unreachable_cloud_never_changes_a_failing_verdict() {
    bin()
        .args([
            "check",
            DATA,
            "--contract",
            CONTRACT,
            "--now",
            "2026-07-10T00:00:00Z",
        ])
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("could not reach"));
}

#[test]
fn a_required_push_that_fails_turns_a_clean_run_into_an_error() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .arg("--push")
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(3);
}

#[test]
fn no_push_ignores_a_key_in_the_environment() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .arg("--no-push")
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("could not reach").not());
}

#[test]
fn a_bad_api_url_is_caught_before_the_data_is_read() {
    // The data path is deliberately nonexistent: if this exits 2 complaining
    // about the URL rather than the file, nothing was read.
    bin()
        .args(["check", "no-such-file.csv", "--contract", CONTRACT])
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", "ftp://example.com")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("not an http(s) URL"));
}

#[test]
fn a_key_is_never_sent_in_the_clear_to_a_remote_host() {
    bin()
        .args(["check", DATA, "--contract", CONTRACT])
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", "http://example.com/api/v1")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("plain http"));
}

#[test]
fn offline_outranks_a_key_in_the_environment() {
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .arg("--offline")
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("could not reach").not());
}

#[test]
fn the_no_network_variable_outranks_a_key_in_the_environment() {
    // The point of the variable rather than the flag: a base image sets it and
    // nothing running underneath can talk, whatever it exports.
    let contract = write_temp("contract.yaml", CLEAN_CONTRACT);
    let data = write_temp("data.csv", "id,name\n1,alice\n");
    bin()
        .args(["check"])
        .arg(&data)
        .arg("--contract")
        .arg(&contract)
        .env("PLEXUSPACT_NO_NETWORK", "1")
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(0)
        .stderr(predicate::str::contains("could not reach").not());
}

#[test]
fn requiring_a_push_while_offline_is_a_usage_error() {
    // Contradictory instructions are worth an error rather than a quiet
    // preference for one of them.
    bin()
        .args(["check", DATA, "--contract", CONTRACT, "--push", "--offline"])
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--offline"));
}

#[test]
fn push_rejects_a_file_that_is_not_a_result() {
    let junk = write_temp("junk.json", "not json at all");
    bin()
        .args(["push"])
        .arg(&junk)
        .env("PLEXUSPACT_API_KEY", "ck_live_notarealkeyjustshapedlikeone")
        .env("PLEXUSPACT_API", NOWHERE)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("is not JSON"));
}

// ───────────────── Excel / XML / fixed-width inputs, contract-carried ─────────────────

const XLSX: &str = "../../fixtures/orders.xlsx";
const XML: &str = "../../fixtures/orders.xml";
const FWF: &str = "../../fixtures/orders.fwf";
const XLSX_CONTRACT: &str = "../../fixtures/orders_excel.yaml";
const XML_CONTRACT: &str = "../../fixtures/orders_xml.yaml";
const FWF_CONTRACT: &str = "../../fixtures/orders_fwf.yaml";

/// `init` on a workbook records the sheet and the skipped rows in the draft,
/// so the next `check` needs no flags.
#[test]
fn excel_init_records_sheet_and_skip_rows() {
    let out = bin()
        .args(["init", XLSX, "--sheet", "2", "--skip-rows", "2"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    assert!(yaml.contains("dataset: orders\n"), "{yaml}");
    assert!(yaml.contains("  customer:"), "{yaml}");
    assert!(yaml.contains("  placed_at:"), "{yaml}");
    assert!(yaml.contains("  input:\n"), "{yaml}");
    assert!(yaml.contains("    skip_rows: 2\n"), "{yaml}");
    assert!(yaml.contains("    sheet: \"2\"\n"), "{yaml}");
    let parsed = plexuspact_contract::parse_str(&yaml, "draft").expect("draft parses");
    assert_eq!(parsed.settings.input.sheet.as_deref(), Some("2"));
    assert_eq!(parsed.settings.input.skip_rows, Some(2));
}

/// The contract says which sheet and where the table starts; `check` reads
/// the workbook that way with no flags at all.
#[test]
fn excel_check_reads_the_sheet_the_contract_names() {
    bin()
        .args([
            "check",
            XLSX,
            "--contract",
            XLSX_CONTRACT,
            "--format",
            "json",
        ])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"rows\":4").or(predicate::str::contains("\"rows\": 4")));
}

/// Flags win over the contract for one run: the first sheet has the table at
/// the top (and an extra `due` column the contract tolerates).
#[test]
fn excel_flags_override_contract_input() {
    bin()
        .args([
            "check",
            XLSX,
            "--contract",
            XLSX_CONTRACT,
            "--sheet",
            "Orders",
            "--skip-rows",
            "0",
        ])
        .assert()
        .code(0);
    bin()
        .args([
            "check",
            XLSX,
            "--contract",
            XLSX_CONTRACT,
            "--sheet",
            "Totals",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no sheet `Totals`"))
        .stderr(predicate::str::contains("Orders, Report"));
}

/// XML: attributes and children become columns, a nested attribute becomes
/// a dotted column, entities and CDATA decode, `xsi:nil` is a null.
#[test]
fn xml_init_and_check() {
    let out = bin()
        .args(["init", XML])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    for col in [
        "  id:",
        "  shipped:",
        "  customer:",
        "  amount.currency:",
        "  amount:",
        "  note:",
    ] {
        assert!(yaml.contains(col), "missing {col} in:\n{yaml}");
    }
    assert!(
        !yaml.contains("input:"),
        "no flags were given, so nothing to record:\n{yaml}"
    );

    bin()
        .args(["check", XML, "--contract", XML_CONTRACT])
        .assert()
        .code(0);
}

#[test]
fn xml_record_not_found_lists_what_it_saw() {
    bin()
        .args([
            "check",
            XML,
            "--contract",
            XML_CONTRACT,
            "--xml-record",
            "item",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("item"))
        .stderr(predicate::str::contains("orders/order"));
}

/// The layout lives in the contract; the data file is just lines.
#[test]
fn fixed_width_check_via_contract_layout() {
    bin()
        .args(["check", FWF, "--contract", FWF_CONTRACT, "--format", "json"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("\"rows\":4").or(predicate::str::contains("\"rows\": 4")));
}

/// `--fixed-width` on `init` drafts the layout into the contract.
#[test]
fn fixed_width_init_records_layout() {
    let out = bin()
        .args([
            "init",
            FWF,
            "--fixed-width",
            "id=1-4,customer=5-20,amount=8,currency=3,shipped=1,placed_at=16,note=49-80",
            "--skip-rows",
            "3",
        ])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    assert!(yaml.contains("    format: fixed_width\n"), "{yaml}");
    assert!(yaml.contains("    skip_rows: 3\n"), "{yaml}");
    assert!(
        yaml.contains("      - { name: \"id\", start: 1, end: 4 }\n"),
        "{yaml}"
    );
    assert!(
        yaml.contains("      - { name: \"amount\", width: 8 }\n"),
        "{yaml}"
    );
    // The values were sliced at the right places: amounts profile as numbers.
    assert!(
        yaml.contains("  amount:") && yaml.contains("type: float"),
        "{yaml}"
    );
    let parsed = plexuspact_contract::parse_str(&yaml, "draft").expect("draft parses");
    assert_eq!(parsed.settings.input.fixed_width.len(), 7);
}

#[test]
fn fixed_width_without_layout_is_a_usage_error() {
    bin()
        .args([
            "check",
            FWF,
            "--contract",
            CONTRACT,
            "--input-format",
            "fixed_width",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no field layout"))
        .stderr(predicate::str::contains("settings.input.fixed_width"));
    bin()
        .args(["init", FWF, "--fixed-width", "id=0-4"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--fixed-width"));
}

#[test]
fn unknown_input_format_is_a_usage_error() {
    bin()
        .args(["init", DATA, "--input-format", "dbf"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--input-format"))
        .stderr(predicate::str::contains("fixed_width"));
}

/// A semicolon-separated, header-less export: the flags describe it and the
/// draft carries the description forward.
#[test]
fn delimiter_and_no_header_are_recorded() {
    let data = write_temp("export.csv", "1;Ada;12.5\n2;Grace;250\n");
    let out = bin()
        .args(["init"])
        .arg(&data)
        .args(["--delimiter", ";", "--no-header", "--dataset", "export"])
        .assert()
        .code(0)
        .get_output()
        .stdout
        .clone();
    let yaml = String::from_utf8(out).unwrap();
    assert!(yaml.contains("    delimiter: \";\"\n"), "{yaml}");
    assert!(yaml.contains("    has_header: false\n"), "{yaml}");
    assert!(
        !yaml.contains("  Ada:"),
        "the first row is data, not names:\n{yaml}"
    );
    let parsed = plexuspact_contract::parse_str(&yaml, "draft").expect("draft parses");
    assert_eq!(parsed.settings.input.delimiter.as_deref(), Some(";"));
    assert_eq!(parsed.settings.input.has_header, Some(false));
}

/// A `references` check needs the other dataset's keys. `--reference` reads
/// them from a file; without it the check cannot run, and a check that could
/// not run does not pass.
#[test]
fn reference_flag_supplies_the_other_datasets_keys() {
    let contract = write_temp(
        "orders.yaml",
        "apiVersion: v1\ndataset: orders\nprimary_key: [order_id]\ncolumns:\n  order_id: { type: string, required: true }\n  customer_id: { type: string, required: true }\ndataset_checks:\n  - references: { columns: [customer_id], dataset: customers }\n",
    );
    let orders = write_temp("orders.csv", "order_id,customer_id\n1,c1\n2,c2\n");
    let customers = write_temp("customers.csv", "customer_id,name\nc1,Ada\nc2,Grace\n");
    let missing = write_temp("customers.csv", "customer_id,name\nc1,Ada\n");

    bin()
        .arg("check")
        .arg(&orders)
        .arg("--contract")
        .arg(&contract)
        .arg("--reference")
        .arg(format!("customers={}", customers.display()))
        .assert()
        .code(0)
        .stderr(predicate::str::contains(
            "2 distinct key(s) for `customers`",
        ));

    bin()
        .arg("check")
        .arg(&orders)
        .arg("--contract")
        .arg(&contract)
        .arg("--reference")
        .arg(format!("customers={}", missing.display()))
        .assert()
        .code(1)
        .stdout(predicate::str::contains("references"));

    bin()
        .arg("check")
        .arg(&orders)
        .arg("--contract")
        .arg(&contract)
        .assert()
        .code(1)
        .stdout(predicate::str::contains("no keys known for `customers`"));
}

#[test]
fn malformed_reference_flag_is_a_usage_error() {
    bin()
        .args([
            "check",
            DATA,
            "--contract",
            CONTRACT,
            "--reference",
            "customers",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("expected `<dataset>=<path>`"));

    bin()
        .arg("check")
        .arg(DATA)
        .arg("--contract")
        .arg(CONTRACT)
        .arg("--reference")
        .arg(format!("customers={DATA}"))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("no `references` check"));
}
