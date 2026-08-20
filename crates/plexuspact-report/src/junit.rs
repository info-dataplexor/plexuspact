//! JUnit XML renderer for CI systems.
//!
//! One `<testsuite>` named after the dataset, one `<testcase>` per check
//! (`classname` = column, or `dataset` for dataset-level checks; `name` = the
//! stable check id). A pure function of [`RunResult`].
//!
//! # Severity mapping (deliberate)
//!
//! Failed **error**-severity checks map to JUnit `<failure>`. Failed
//! **warn**-severity checks map to `<skipped>` with an explanatory message —
//! *not* to a failure — because a warn check must not turn CI red (PRD FR-7:
//! warn exits 0 unless `--strict`; the CLI's exit code, not the report,
//! decides strict mode). The suite's `skipped` counter therefore counts warn
//! failures.
//!
//! Timing: per-testcase timing is not measured by the engine, so every case
//! reports `0`s and the suite carries the whole run's `duration_ms`.

use std::time::Duration;

use plexuspact_core::result::{CheckResult, CheckSeverity, CheckStatus, RunResult};
use quick_junit::{NonSuccessKind, Report, TestCase, TestCaseStatus, TestSuite};

use crate::util::{percent, thousands};
use crate::RenderError;

/// Renders `result` as a JUnit XML document (see module docs for the
/// severity-to-JUnit mapping).
pub fn render_junit(result: &RunResult) -> Result<String, RenderError> {
    let mut report = Report::new("plexuspact");
    report.set_timestamp(result.started_at.fixed_offset());
    report.set_time(Duration::from_millis(result.duration_ms));

    let mut suite = TestSuite::new(result.contract.dataset.clone());
    suite.set_timestamp(result.started_at.fixed_offset());
    suite.set_time(Duration::from_millis(result.duration_ms));

    for check in &result.checks {
        let status = case_status(check);
        let mut case = TestCase::new(check.id.clone(), status);
        case.set_classname(check.column.as_deref().unwrap_or("dataset"));
        case.set_time(Duration::ZERO);
        suite.add_test_case(case);
    }

    report.add_test_suite(suite);
    Ok(report.to_string()?)
}

/// Maps one check outcome to a JUnit testcase status.
fn case_status(check: &CheckResult) -> TestCaseStatus {
    match (check.status, check.severity) {
        (CheckStatus::Passed, _) => TestCaseStatus::success(),
        (CheckStatus::Failed, CheckSeverity::Error) => {
            let mut status = TestCaseStatus::non_success(NonSuccessKind::Failure);
            status.set_type(check.kind.clone());
            status.set_message(failure_message(check));
            status.set_description(failure_description(check));
            status
        }
        (CheckStatus::Failed, CheckSeverity::Warn) => {
            // Warn failures are informational in CI: skipped, never red.
            let mut status = TestCaseStatus::skipped();
            status.set_type(check.kind.clone());
            status.set_message(format!("[warn] {}", failure_message(check)));
            status.set_description(failure_description(check));
            status
        }
    }
}

/// One-line failure message: the check's own message, or its rows-failed
/// summary when the engine only counted rows.
fn failure_message(check: &CheckResult) -> String {
    if let Some(message) = &check.message {
        return message.clone();
    }
    rows_failed_summary(check).unwrap_or_else(|| "check failed".to_owned())
}

/// Multi-line failure body: message, rows-failed summary, and sample rows.
fn failure_description(check: &CheckResult) -> String {
    let mut lines = Vec::new();
    if let Some(message) = &check.message {
        lines.push(message.clone());
    }
    if let Some(summary) = rows_failed_summary(check) {
        lines.push(summary);
    }
    for sample in &check.samples {
        lines.push(format!("row {}: \"{}\"", sample.row, sample.value));
    }
    lines.join("\n")
}

/// `"512 rows failed (0.04%)"`, when the metrics carry a rows-failed count.
fn rows_failed_summary(check: &CheckResult) -> Option<String> {
    let rows = check.metrics.rows_failed?;
    let mut summary = format!("{} rows failed", thousands(rows));
    if let Some(ratio) = check.metrics.fail_ratio {
        summary.push_str(&format!(" ({})", percent(ratio)));
    }
    Some(summary)
}
