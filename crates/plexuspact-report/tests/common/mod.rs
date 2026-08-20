//! Shared `RunResult` fixtures for the renderer integration tests.
//!
//! Fixtures are fully deterministic (fixed timestamps, versions, and hashes)
//! so snapshot tests are stable across machines.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use plexuspact_core::result::{
    CheckMetrics, CheckResult, CheckSeverity, CheckStatus, ConsumerRef, ContractRef, FailureSample,
    RunResult, RunStatus, RunSummary, SourceInfo, RESULT_SCHEMA_VERSION,
};
use serde_json::json;

fn started_at() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-07-09T10:11:12Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn contract_ref(dataset: &str) -> ContractRef {
    ContractRef {
        dataset: dataset.to_owned(),
        path: Some("contract.yaml".to_owned()),
        content_sha256: "ab".repeat(32),
        owner: Some("growth-team@acme.com".to_owned()),
        consumers: vec![ConsumerRef {
            name: "analytics-core".to_owned(),
            contact: Some("data-team@acme.com".to_owned()),
        }],
    }
}

fn passed_column_check(column: &str, kind: &str, params: serde_json::Value) -> CheckResult {
    CheckResult {
        id: format!("{column}.{kind}"),
        column: Some(column.to_owned()),
        kind: kind.to_owned(),
        params,
        severity: CheckSeverity::Error,
        status: CheckStatus::Passed,
        metrics: CheckMetrics {
            rows_evaluated: Some(1_204_441),
            rows_failed: Some(0),
            fail_ratio: Some(0.0),
            observed: BTreeMap::new(),
        },
        samples: vec![],
        message: None,
    }
}

/// A run where every check passed. Exercises the single ✓ header line.
pub fn all_pass() -> RunResult {
    RunResult {
        result_schema_version: RESULT_SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        contract: contract_ref("user_signups"),
        source: SourceInfo {
            path: "data.csv".to_owned(),
            format: "csv".to_owned(),
            rows: 1_204_441,
            columns: 6,
            bytes: Some(812_345_678),
        },
        started_at: started_at(),
        duration_ms: 6231,
        status: RunStatus::Passed,
        summary: RunSummary {
            checks_total: 5,
            passed: 5,
            failed_error: 0,
            failed_warn: 0,
        },
        checks: vec![
            passed_column_check("user_id", "unique", json!(null)),
            passed_column_check("email", "format", json!({"format": "email"})),
            passed_column_check("age", "min", json!({"min": 18})),
            passed_column_check(
                "plan",
                "enum",
                json!({"enum": ["free", "pro", "enterprise"]}),
            ),
            CheckResult {
                id: "dataset.row_count_min".to_owned(),
                column: None,
                kind: "row_count_min".to_owned(),
                params: json!({"min": 1}),
                severity: CheckSeverity::Error,
                status: CheckStatus::Passed,
                metrics: CheckMetrics {
                    rows_evaluated: None,
                    rows_failed: None,
                    fail_ratio: None,
                    observed: BTreeMap::from([("row_count".to_owned(), json!(1_204_441))]),
                },
                samples: vec![],
                message: None,
            },
        ],
    }
}

/// The PRD §7 scenario: error + warn failures with samples, messages, observed
/// metrics, and dataset-level checks. 14 checks: 11 passed, 2 failed at error
/// severity, 1 failed at warn severity.
pub fn mixed_failures() -> RunResult {
    let email_format_failed = CheckResult {
        id: "email.format.email".to_owned(),
        column: Some("email".to_owned()),
        kind: "format".to_owned(),
        params: json!({"format": "email"}),
        severity: CheckSeverity::Error,
        status: CheckStatus::Failed,
        metrics: CheckMetrics {
            rows_evaluated: Some(1_204_441),
            rows_failed: Some(3),
            fail_ratio: Some(2.49e-6),
            observed: BTreeMap::new(),
        },
        samples: vec![
            FailureSample {
                row: 10_442,
                value: "bob@@example".to_owned(),
            },
            FailureSample {
                row: 88_310,
                value: "alice@".to_owned(),
            },
            FailureSample {
                row: 990_004,
                value: "not-an-email".to_owned(),
            },
        ],
        message: None,
    };
    let age_min_failed = CheckResult {
        id: "age.min".to_owned(),
        column: Some("age".to_owned()),
        kind: "min".to_owned(),
        params: json!({"min": 18}),
        severity: CheckSeverity::Error,
        status: CheckStatus::Failed,
        metrics: CheckMetrics {
            rows_evaluated: Some(1_204_441),
            rows_failed: Some(512),
            fail_ratio: Some(4.25e-4),
            observed: BTreeMap::from([("min".to_owned(), json!(11))]),
        },
        samples: vec![],
        message: Some("min observed: 11".to_owned()),
    };
    let freshness_warn_failed = CheckResult {
        id: "dataset.freshness.signed_up".to_owned(),
        column: Some("signed_up".to_owned()),
        kind: "freshness".to_owned(),
        params: json!({"column": "signed_up", "max_age": "48h"}),
        severity: CheckSeverity::Warn,
        status: CheckStatus::Failed,
        metrics: CheckMetrics {
            rows_evaluated: None,
            rows_failed: None,
            fail_ratio: None,
            observed: BTreeMap::from([("newest_age_hours".to_owned(), json!(51))]),
        },
        samples: vec![],
        message: Some("newest row is 51h old".to_owned()),
    };

    RunResult {
        result_schema_version: RESULT_SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        contract: contract_ref("user_signups"),
        source: SourceInfo {
            path: "data.csv".to_owned(),
            format: "csv".to_owned(),
            rows: 1_204_441,
            columns: 6,
            bytes: Some(812_345_678),
        },
        started_at: started_at(),
        duration_ms: 6231,
        status: RunStatus::Failed,
        summary: RunSummary {
            checks_total: 14,
            passed: 11,
            failed_error: 2,
            failed_warn: 1,
        },
        checks: vec![
            passed_column_check("user_id", "required", json!(null)),
            passed_column_check("user_id", "unique", json!(null)),
            passed_column_check("email", "required", json!(null)),
            email_format_failed,
            passed_column_check("email", "length", json!({"min": 3, "max": 254})),
            age_min_failed,
            passed_column_check("age", "max", json!({"max": 120})),
            passed_column_check("country", "length", json!({"length": 2})),
            passed_column_check("country", "regex", json!({"regex": "^[A-Z]{2}$"})),
            passed_column_check(
                "plan",
                "enum",
                json!({"enum": ["free", "pro", "enterprise"]}),
            ),
            passed_column_check("signed_up", "required", json!(null)),
            CheckResult {
                id: "dataset.row_count_min".to_owned(),
                column: None,
                kind: "row_count_min".to_owned(),
                params: json!({"min": 1}),
                severity: CheckSeverity::Error,
                status: CheckStatus::Passed,
                metrics: CheckMetrics {
                    rows_evaluated: None,
                    rows_failed: None,
                    fail_ratio: None,
                    observed: BTreeMap::from([("row_count".to_owned(), json!(1_204_441))]),
                },
                samples: vec![],
                message: None,
            },
            CheckResult {
                id: "dataset.null_ratio_max.age".to_owned(),
                column: None,
                kind: "null_ratio_max".to_owned(),
                params: json!({"column": "age", "ratio": 0.10}),
                severity: CheckSeverity::Error,
                status: CheckStatus::Passed,
                metrics: CheckMetrics {
                    rows_evaluated: Some(1_204_441),
                    rows_failed: None,
                    fail_ratio: None,
                    observed: BTreeMap::from([("null_ratio".to_owned(), json!(0.031))]),
                },
                samples: vec![],
                message: None,
            },
            freshness_warn_failed,
        ],
    }
}

/// Edge case: an empty source (0 rows) failing a dataset-level check with a
/// message and no rows-failed count.
pub fn empty_dataset() -> RunResult {
    RunResult {
        result_schema_version: RESULT_SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        contract: ContractRef {
            dataset: "orders".to_owned(),
            path: Some("orders.contract.yaml".to_owned()),
            content_sha256: "cd".repeat(32),
            owner: None,
            consumers: vec![],
        },
        source: SourceInfo {
            path: "-".to_owned(),
            format: "ndjson".to_owned(),
            rows: 0,
            columns: 0,
            bytes: None,
        },
        started_at: started_at(),
        duration_ms: 12,
        status: RunStatus::Failed,
        summary: RunSummary {
            checks_total: 3,
            passed: 2,
            failed_error: 1,
            failed_warn: 0,
        },
        checks: vec![
            CheckResult {
                id: "order_id.required".to_owned(),
                column: Some("order_id".to_owned()),
                kind: "required".to_owned(),
                params: json!(null),
                severity: CheckSeverity::Error,
                status: CheckStatus::Passed,
                metrics: CheckMetrics {
                    rows_evaluated: Some(0),
                    rows_failed: Some(0),
                    fail_ratio: None,
                    observed: BTreeMap::new(),
                },
                samples: vec![],
                message: None,
            },
            CheckResult {
                id: "order_id.unique".to_owned(),
                column: Some("order_id".to_owned()),
                kind: "unique".to_owned(),
                params: json!(null),
                severity: CheckSeverity::Error,
                status: CheckStatus::Passed,
                metrics: CheckMetrics {
                    rows_evaluated: Some(0),
                    rows_failed: Some(0),
                    fail_ratio: None,
                    observed: BTreeMap::new(),
                },
                samples: vec![],
                message: None,
            },
            CheckResult {
                id: "dataset.row_count_min".to_owned(),
                column: None,
                kind: "row_count_min".to_owned(),
                params: json!({"min": 1}),
                severity: CheckSeverity::Error,
                status: CheckStatus::Failed,
                metrics: CheckMetrics {
                    rows_evaluated: None,
                    rows_failed: None,
                    fail_ratio: None,
                    observed: BTreeMap::from([("row_count".to_owned(), json!(0))]),
                },
                samples: vec![],
                message: Some("expected at least 1 row, found 0".to_owned()),
            },
        ],
    }
}

/// Edge case: very large counts (>1M failed rows, >100M total rows) to lock
/// thousands-separator formatting, plus a long duration.
pub fn huge_numbers() -> RunResult {
    RunResult {
        result_schema_version: RESULT_SCHEMA_VERSION,
        tool_version: "0.1.0".to_owned(),
        contract: ContractRef {
            dataset: "clickstream".to_owned(),
            path: None,
            content_sha256: "ef".repeat(32),
            owner: None,
            consumers: vec![],
        },
        source: SourceInfo {
            path: "events.parquet".to_owned(),
            format: "parquet".to_owned(),
            rows: 123_456_789,
            columns: 42,
            bytes: Some(9_876_543_210),
        },
        started_at: started_at(),
        duration_ms: 754_321,
        status: RunStatus::Failed,
        summary: RunSummary {
            checks_total: 2,
            passed: 1,
            failed_error: 1,
            failed_warn: 0,
        },
        checks: vec![
            passed_column_check("event_id", "unique", json!(null)),
            CheckResult {
                id: "session_id.not_empty_string".to_owned(),
                column: Some("session_id".to_owned()),
                kind: "not_empty_string".to_owned(),
                params: json!(null),
                severity: CheckSeverity::Error,
                status: CheckStatus::Failed,
                metrics: CheckMetrics {
                    rows_evaluated: Some(123_456_789),
                    rows_failed: Some(1_234_567),
                    fail_ratio: Some(0.01),
                    observed: BTreeMap::new(),
                },
                samples: vec![FailureSample {
                    row: 1_000_001,
                    value: String::new(),
                }],
                message: None,
            },
        ],
    }
}
