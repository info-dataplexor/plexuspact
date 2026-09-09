//! Integration test: the canonical contract against the canonical CSV fixture.
//!
//! Asserts exact per-check outcomes and absolute failing-row numbers, so any
//! regression in check semantics or row numbering is caught.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use plexuspact_engine::{execute, CheckOutcome, EngineError, EngineOutput, RunOptions};
use plexuspact_io::{open, resolve, ReadOptions};

fn run_at(now_rfc3339: &str) -> EngineOutput {
    let yaml = std::fs::read_to_string("../../fixtures/contract.yaml").unwrap();
    let contract = plexuspact_contract::parse_str(&yaml, "contract.yaml").unwrap();
    let source = resolve("../../fixtures/signups_small.csv");
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339(now_rfc3339)
        .unwrap()
        .to_utc();
    execute(
        bs.as_mut(),
        &contract,
        &RunOptions {
            sample_failures: 5,
            now,
            ..RunOptions::default()
        },
    )
    .unwrap()
}

fn find<'a>(out: &'a EngineOutput, id: &str) -> &'a CheckOutcome {
    out.checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no check with id `{id}` (have: {:?})", ids(out)))
}

fn ids(out: &EngineOutput) -> Vec<String> {
    out.checks.iter().map(|c| c.id.clone()).collect()
}

#[test]
fn total_rows_and_columns() {
    let out = run_at("2026-07-10T00:00:00Z");
    assert_eq!(out.rows_total, 30);
    assert_eq!(out.source_columns, 6);
    assert_eq!(
        out.checks.len(),
        20,
        "expected 20 checks, got {:?}",
        ids(&out)
    );
}

#[test]
fn email_format_fails_one_row() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "email.format.email");
    assert!(c.failed);
    assert_eq!(c.rows_failed, Some(1));
    assert_eq!(c.samples, vec![(11, "bob@@example".to_owned())]);
}

#[test]
fn age_min_fails_two_rows() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "age.min");
    assert!(c.failed);
    assert_eq!(c.rows_failed, Some(2));
    let rows: Vec<u64> = c.samples.iter().map(|(r, _)| *r).collect();
    assert_eq!(rows, vec![12, 13]);
}

#[test]
fn age_max_fails_one_row() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "age.max");
    assert!(c.failed);
    assert_eq!(c.rows_failed, Some(1));
    assert_eq!(c.samples, vec![(20, "133".to_owned())]);
}

#[test]
fn user_id_unique_fails_on_second_occurrence() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "user_id.unique");
    assert!(c.failed);
    assert_eq!(c.rows_failed, Some(1));
    assert_eq!(c.samples, vec![(14, "u-0010".to_owned())]);
}

#[test]
fn country_length_and_regex_fail_one_row_each() {
    let out = run_at("2026-07-10T00:00:00Z");
    let len = find(&out, "country.length");
    assert!(len.failed);
    assert_eq!(len.samples, vec![(15, "usa".to_owned())]);
    let re = find(&out, "country.regex");
    assert!(re.failed);
    assert_eq!(re.rows_failed, Some(1));
}

#[test]
fn plan_enum_fails_one_row() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "plan.enum");
    assert!(c.failed);
    assert_eq!(c.samples, vec![(16, "premium".to_owned())]);
}

#[test]
fn required_checks_pass() {
    let out = run_at("2026-07-10T00:00:00Z");
    for id in ["user_id.required", "email.required", "signed_up.required"] {
        assert!(!find(&out, id).failed, "{id} should pass");
    }
}

#[test]
fn type_checks_pass() {
    let out = run_at("2026-07-10T00:00:00Z");
    for id in ["age.type", "signed_up.type", "user_id.type"] {
        assert!(!find(&out, id).failed, "{id} should pass");
    }
}

#[test]
fn dataset_checks_pass_at_reference_time() {
    let out = run_at("2026-07-10T00:00:00Z");
    assert!(!find(&out, "dataset.row_count_min").failed);
    assert!(!find(&out, "dataset.null_ratio_max.age").failed);
    assert!(
        !find(&out, "dataset.freshness.signed_up").failed,
        "freshness passes at ~32h"
    );
}

#[test]
fn freshness_fails_when_stale() {
    let out = run_at("2026-07-11T00:00:00Z");
    let c = find(&out, "dataset.freshness.signed_up");
    assert!(c.failed, "freshness should fail at ~56h > 48h");
    assert_eq!(c.severity, plexuspact_contract::Severity::Warn);
}

#[test]
fn error_failure_count_is_seven() {
    let out = run_at("2026-07-10T00:00:00Z");
    let error_failures = out
        .checks
        .iter()
        .filter(|c| c.failed && c.severity == plexuspact_contract::Severity::Error)
        .count();
    assert_eq!(error_failures, 7, "failed: {:?}", failed_ids(&out));
}

fn failed_ids(out: &EngineOutput) -> Vec<String> {
    out.checks
        .iter()
        .filter(|c| c.failed)
        .map(|c| c.id.clone())
        .collect()
}

#[test]
fn null_ratio_observed_metric_present() {
    let out = run_at("2026-07-10T00:00:00Z");
    let c = find(&out, "dataset.null_ratio_max.age");
    assert!(c.observed.contains_key("null_ratio"));
}

/// A declared column holding a JSON array (common when pointing the tool at a
/// raw REST-API response) must surface a clear, actionable `NestedColumn` error
/// naming the column and its kind — never an opaque internal cast failure.
#[test]
fn nested_array_column_yields_actionable_error() {
    let yaml = std::fs::read_to_string("../../fixtures/nested_arrays.contract.yaml").unwrap();
    let contract = plexuspact_contract::parse_str(&yaml, "nested_arrays.contract.yaml").unwrap();
    let source = resolve("../../fixtures/nested_arrays.ndjson");
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    let now = chrono::DateTime::parse_from_rfc3339("2026-07-10T00:00:00Z")
        .unwrap()
        .to_utc();
    let err = execute(
        bs.as_mut(),
        &contract,
        &RunOptions {
            sample_failures: 5,
            now,
            ..RunOptions::default()
        },
    )
    .expect_err("nested array column must error");
    match err {
        EngineError::NestedColumn { column, kind } => {
            assert_eq!(column, "web_pages");
            assert_eq!(kind, "array");
        }
        other => panic!("expected NestedColumn, got {other:?}"),
    }
}
