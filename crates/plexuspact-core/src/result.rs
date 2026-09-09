//! `RunResult` — the versioned output contract of every validation run (doc 03 §8).
//!
//! This is the **single source of truth**: the human renderer, JSON output, JUnit,
//! HTML report, the GitHub Action annotations, and the Phase 2 SaaS `push` payload
//! are all pure functions of this document. Renderers must never compute results.
//!
//! Stability rules (ADR-007): within `result_schema_version: 1` changes are
//! additive-only — never rename, remove, or repurpose a field. These types are
//! deliberately self-contained (no dependency on the contract model) so the wire
//! format cannot drift when the contract schema evolves.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The current result schema version emitted by this crate.
pub const RESULT_SCHEMA_VERSION: u32 = 1;

/// Complete, serializable result of one `plexuspact check` run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunResult {
    /// Version of this document's schema. Additive-only within a version.
    pub result_schema_version: u32,
    /// Version of the plexuspact binary that produced this result.
    pub tool_version: String,
    /// Identity of the contract the data was validated against.
    pub contract: ContractRef,
    /// Description of the validated source.
    pub source: SourceInfo,
    /// UTC timestamp at which the run started (RFC 3339).
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// Wall-clock duration of the run in milliseconds.
    pub duration_ms: u64,
    /// Overall status: `passed` iff no error-severity check failed.
    pub status: RunStatus,
    /// Aggregate counts over all checks.
    pub summary: RunSummary,
    /// Per-check outcomes, in contract order (columns first, then dataset checks).
    pub checks: Vec<CheckResult>,
    /// The shape the source actually had, in source order.
    ///
    /// Additive within schema version 1 (ADR-007): older readers ignore it,
    /// older producers omit it, and a consumer that receives `None` knows only
    /// that nothing was recorded — never that the source had no columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_schema: Option<ObservedSchema>,
}

/// The source's own schema at the moment of a run.
///
/// The checks answer "did the data honour the contract". This answers a
/// different and quieter question: *what was actually there*. A column that
/// appears, disappears, moves, or changes type without anybody declaring it is
/// invisible to a contract that never mentioned it — and it is exactly the
/// change that breaks a downstream reader at 3am. Comparing this snapshot with
/// the previous run's is how that change gets a name and a timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedSchema {
    /// Whether `columns[].dtype` is the source's own typing.
    ///
    /// False for CSV, NDJSON, and JSON read as text: every column arrives as a
    /// string, so a recorded dtype would describe the reader, not the source.
    /// Names and order stay honest either way, which is most of what drift is.
    pub typed: bool,
    /// Every column the source presented, in the order it presented them.
    pub columns: Vec<ObservedColumnInfo>,
}

/// One column as the source presented it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedColumnInfo {
    /// Column name, exactly as spelled in the source.
    pub name: String,
    /// The source's dtype, when it carries one.
    ///
    /// `None` means the source does not say — not "unknown type". A comparison
    /// that treated the two the same would report a type change every time a
    /// CSV feed switched readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dtype: Option<String>,
}

/// Identity and provenance of the contract used for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractRef {
    /// Dataset name declared in the contract (`dataset:` field).
    pub dataset: String,
    /// Path the contract was loaded from, if it came from a file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// SHA-256 of the raw contract bytes — contract identity for the registry
    /// (doc 03 §12: identity = dataset name + content hash, computed offline).
    pub content_sha256: String,
    /// Contract owner, echoed for reports and alert routing (PRD FR-11).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// Declared downstream consumers, echoed for reports (PRD FR-11; no behavior in v1).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub consumers: Vec<ConsumerRef>,
}

/// A declared downstream consumer of the dataset (reserved field, PRD FR-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsumerRef {
    /// Consumer name (team, system, or dashboard).
    pub name: String,
    /// Contact for the consumer (email, Slack handle, …).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<String>,
}

/// Description of the data source that was validated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// File path, or `-` for stdin.
    pub path: String,
    /// Detected or forced input format (`csv`, `tsv`, `parquet`, `ndjson`,
    /// `json`, `excel`, `xml`, `fixed_width`).
    pub format: String,
    /// Total rows read.
    pub rows: u64,
    /// Number of columns present in the source.
    pub columns: u64,
    /// Source size in bytes, when known (absent for streams).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// Overall run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// No error-severity check failed (warn-severity failures allowed).
    Passed,
    /// At least one error-severity check failed.
    Failed,
}

/// Aggregate counts over all checks in the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    /// Total number of checks executed.
    pub checks_total: u64,
    /// Checks that passed.
    pub passed: u64,
    /// Error-severity checks that failed.
    pub failed_error: u64,
    /// Warn-severity checks that failed.
    pub failed_warn: u64,
}

/// Severity of a check as declared in the contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckSeverity {
    /// Failure fails the run (exit code 1).
    #[default]
    Error,
    /// Failure is reported but exits 0 unless `--strict`.
    Warn,
}

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The check held for all evaluated rows / the dataset.
    Passed,
    /// The check was violated.
    Failed,
}

/// Result of one check, with metrics captured even on pass (doc 03 §12: the SaaS
/// drift charts are built from these metrics, so passing checks carry them too).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Stable identifier, e.g. `email.format.email` or `dataset.row_count_min`.
    pub id: String,
    /// Column the check applies to; absent for dataset-level checks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Check kind, e.g. `min`, `format`, `unique`, `row_count_min`.
    pub kind: String,
    /// Check parameters as declared in the contract, e.g. `{"format": "email"}`.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub params: serde_json::Value,
    /// Declared severity.
    pub severity: CheckSeverity,
    /// Outcome.
    pub status: CheckStatus,
    /// Quantitative metrics for this check.
    pub metrics: CheckMetrics,
    /// Up to `--sample-failures` sample failing rows (absolute row numbers).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<FailureSample>,
    /// Human-readable one-line detail, e.g. `min observed: 11` or
    /// `newest row is 51h old`. Renderers display it verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Metrics for one check. Standard fields are typed; engine-specific observations
/// (observed min/max/mean, null_ratio, distinct estimates, …) flatten into `observed`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct CheckMetrics {
    /// Rows the check evaluated (absent for dataset-level checks where meaningless).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_evaluated: Option<u64>,
    /// Rows that violated the check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rows_failed: Option<u64>,
    /// `rows_failed / rows_evaluated`, when both are known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_ratio: Option<f64>,
    /// Additional observed statistics, captured even for passing checks
    /// (drift time-series source). Keys are stable per check kind.
    #[serde(flatten)]
    pub observed: BTreeMap<String, serde_json::Value>,
}

/// One sampled failing row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FailureSample {
    /// Absolute 1-based row number in the source (header excluded for CSV).
    pub row: u64,
    /// The offending value, rendered as a string (may be redacted).
    pub value: String,
}

impl RunResult {
    /// True iff the run should produce a non-zero exit code under the given
    /// strictness (ADR-004): error failures always fail; warn failures fail
    /// only with `--strict`.
    #[must_use]
    pub fn is_failure(&self, strict: bool) -> bool {
        self.summary.failed_error > 0 || (strict && self.summary.failed_warn > 0)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn sample() -> RunResult {
        RunResult {
            result_schema_version: RESULT_SCHEMA_VERSION,
            tool_version: "0.1.0".to_owned(),
            contract: ContractRef {
                dataset: "user_signups".to_owned(),
                path: Some("contract.yaml".to_owned()),
                content_sha256: "ab".repeat(32),
                owner: Some("growth-team@acme.com".to_owned()),
                consumers: vec![ConsumerRef {
                    name: "analytics-core".to_owned(),
                    contact: Some("data-team@acme.com".to_owned()),
                }],
            },
            source: SourceInfo {
                path: "data.csv".to_owned(),
                format: "csv".to_owned(),
                rows: 1_204_441,
                columns: 6,
                bytes: Some(812_345_678),
            },
            started_at: chrono::DateTime::parse_from_rfc3339("2026-07-09T10:11:12Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            duration_ms: 6231,
            status: RunStatus::Failed,
            summary: RunSummary {
                checks_total: 14,
                passed: 12,
                failed_error: 1,
                failed_warn: 1,
            },
            checks: vec![CheckResult {
                id: "email.format.email".to_owned(),
                column: Some("email".to_owned()),
                kind: "format".to_owned(),
                params: serde_json::json!({"format": "email"}),
                severity: CheckSeverity::Error,
                status: CheckStatus::Failed,
                metrics: CheckMetrics {
                    rows_evaluated: Some(1_204_441),
                    rows_failed: Some(3),
                    fail_ratio: Some(2.49e-6),
                    observed: BTreeMap::new(),
                },
                samples: vec![FailureSample {
                    row: 10442,
                    value: "bob@@example".to_owned(),
                }],
                message: None,
            }],
            observed_schema: None,
        }
    }

    #[test]
    fn json_round_trip_preserves_document() {
        let result = sample();
        let json = serde_json::to_string_pretty(&result).unwrap();
        let back: RunResult = serde_json::from_str(&json).unwrap();
        assert_eq!(result, back);
    }

    #[test]
    fn wire_format_field_names_match_doc_03_section_8() {
        let value = serde_json::to_value(sample()).unwrap();
        assert_eq!(value["result_schema_version"], 1);
        assert_eq!(value["contract"]["dataset"], "user_signups");
        assert_eq!(value["source"]["format"], "csv");
        assert_eq!(value["status"], "failed");
        assert_eq!(value["summary"]["failed_error"], 1);
        assert_eq!(value["checks"][0]["id"], "email.format.email");
        assert_eq!(value["checks"][0]["metrics"]["rows_failed"], 3);
        assert_eq!(value["checks"][0]["samples"][0]["row"], 10442);
    }

    #[test]
    fn failure_semantics_respect_strict_flag() {
        let mut result = sample();
        assert!(result.is_failure(false));
        result.summary.failed_error = 0;
        assert!(
            !result.is_failure(false),
            "warn-only must pass without --strict"
        );
        assert!(result.is_failure(true), "warn-only must fail with --strict");
    }
}
