//! OpenLineage renderer — emit a validation run as an [OpenLineage] `RunEvent`
//! carrying the standard **data-quality facets**, so any OpenLineage consumer
//! (Databricks, dbt via `dbt-ol`, Airflow, Marquez, DataHub, …) can ingest
//! PlexusPact's verdicts without a bespoke adapter.
//!
//! Mapping (all derived purely from [`RunResult`] — no new verdicts):
//! - each check → one entry in the `dataQualityAssertions` dataset facet
//!   (`assertion` = check id, `success` = passed, `column` when column-scoped);
//! - row/column statistics → the `dataQualityMetrics` input-dataset facet
//!   (`rowCount`, `bytes`, and per-column `nullCount`/`distinctCount`/`count`);
//! - contract identity + run summary → a PlexusPact-namespaced run facet.
//!
//! The event's `eventType` is `COMPLETE` on a passing run and `FAIL` when any
//! error-severity check failed, so an orchestrator can gate on it directly.
//!
//! [OpenLineage]: https://openlineage.io/

use plexuspact_core::result::{RunResult, RunStatus};
use serde_json::{json, Map, Value};

use crate::RenderError;

/// Source repository, used to build the OpenLineage `producer`/`_producer` URIs.
const REPO: &str = "https://github.com/info-dataplexor/plexuspact";
/// Pinned OpenLineage core spec version for the event `schemaURL`.
const OL_SPEC: &str = "https://openlineage.io/spec/2-0-2/OpenLineage.json#/$defs/RunEvent";
/// Published schema for the data-quality assertions facet.
const OL_ASSERTIONS: &str =
    "https://openlineage.io/spec/facets/1-0-0/DataQualityAssertionsDatasetFacet.json";
/// Published schema for the data-quality metrics facet.
const OL_METRICS: &str =
    "https://openlineage.io/spec/facets/1-0-0/DataQualityMetricsInputDatasetFacet.json";

/// Knobs for naming the emitted OpenLineage entities. Sensible defaults let the
/// CLI expose a single `--openlineage <FILE>` flag.
#[derive(Debug, Clone)]
pub struct OpenLineageOptions {
    /// Namespace of the *input dataset* — the storage system it lives in,
    /// e.g. `file`, `s3://bucket`, `postgres://host`. Defaults to `file`.
    pub dataset_namespace: String,
    /// Namespace of the *job*. Defaults to `plexuspact`.
    pub job_namespace: String,
    /// Job name. Defaults to `plexuspact.check.<dataset>`.
    pub job_name: Option<String>,
}

impl Default for OpenLineageOptions {
    fn default() -> Self {
        OpenLineageOptions {
            dataset_namespace: "file".to_owned(),
            job_namespace: "plexuspact".to_owned(),
            job_name: None,
        }
    }
}

/// Renders `result` as a single OpenLineage `RunEvent` JSON document.
pub fn render_openlineage(
    result: &RunResult,
    opts: &OpenLineageOptions,
) -> Result<String, RenderError> {
    let producer = format!("{REPO}/tree/v{}", result.tool_version);
    let dataset = &result.contract.dataset;
    let job_name = opts
        .job_name
        .clone()
        .unwrap_or_else(|| format!("plexuspact.check.{dataset}"));

    // Completion instant = start + wall-clock duration.
    let event_time = (result.started_at
        + chrono::Duration::milliseconds(result.duration_ms as i64))
    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

    let event_type = match result.status {
        RunStatus::Passed => "COMPLETE",
        RunStatus::Failed => "FAIL",
    };

    let run_id = run_uuid(
        &result.contract.content_sha256,
        result.started_at.timestamp_millis(),
    );

    let input = json!({
        "namespace": opts.dataset_namespace,
        "name": dataset,
        "facets": {
            "dataQualityAssertions": assertions_facet(result, &producer),
        },
        "inputFacets": {
            "dataQualityMetrics": metrics_facet(result, &producer),
        },
    });

    let event = json!({
        "eventType": event_type,
        "eventTime": event_time,
        "producer": producer,
        "schemaURL": OL_SPEC,
        "run": {
            "runId": run_id,
            "facets": {
                "plexuspact_contract": contract_run_facet(result, &producer),
            },
        },
        "job": {
            "namespace": opts.job_namespace,
            "name": job_name,
            "facets": job_facets(result, &producer),
        },
        "inputs": [input],
        "outputs": [],
    });

    Ok(serde_json::to_string_pretty(&event)?)
}

/// The `dataQualityAssertions` dataset facet: one assertion per check.
fn assertions_facet(result: &RunResult, producer: &str) -> Value {
    let assertions: Vec<Value> = result
        .checks
        .iter()
        .map(|c| {
            let mut a = Map::new();
            a.insert("assertion".to_owned(), json!(c.id));
            a.insert(
                "success".to_owned(),
                json!(c.status == plexuspact_core::result::CheckStatus::Passed),
            );
            if let Some(col) = &c.column {
                a.insert("column".to_owned(), json!(col));
            }
            Value::Object(a)
        })
        .collect();
    json!({
        "_producer": producer,
        "_schemaURL": OL_ASSERTIONS,
        "assertions": assertions,
    })
}

/// The `dataQualityMetrics` input-dataset facet: row/byte totals plus whatever
/// per-column statistics the checks observed.
fn metrics_facet(result: &RunResult, producer: &str) -> Value {
    let mut facet = Map::new();
    facet.insert("_producer".to_owned(), json!(producer));
    facet.insert("_schemaURL".to_owned(), json!(OL_METRICS));
    facet.insert("rowCount".to_owned(), json!(result.source.rows));
    if let Some(bytes) = result.source.bytes {
        facet.insert("bytes".to_owned(), json!(bytes));
    }

    // Merge per-column observations across all checks touching that column.
    let mut columns: std::collections::BTreeMap<String, Map<String, Value>> = Default::default();
    for check in &result.checks {
        let Some(col) = &check.column else { continue };
        let entry = columns.entry(col.clone()).or_default();
        entry
            .entry("count".to_owned())
            .or_insert_with(|| json!(result.source.rows));
        // null_ratio → nullCount (rounded to nearest row).
        if let Some(nr) = check
            .metrics
            .observed
            .get("null_ratio")
            .and_then(Value::as_f64)
        {
            let null_count = (nr * result.source.rows as f64).round() as u64;
            entry.insert("nullCount".to_owned(), json!(null_count));
        }
        for (obs_key, metric_key) in [
            ("distinct", "distinctCount"),
            ("distinct_count", "distinctCount"),
        ] {
            if let Some(v) = check.metrics.observed.get(obs_key).and_then(Value::as_u64) {
                entry.insert(metric_key.to_owned(), json!(v));
            }
        }
    }
    if !columns.is_empty() {
        let column_metrics: Map<String, Value> = columns
            .into_iter()
            .map(|(k, v)| (k, Value::Object(v)))
            .collect();
        facet.insert("columnMetrics".to_owned(), Value::Object(column_metrics));
    }
    Value::Object(facet)
}

/// A PlexusPact-namespaced run facet carrying contract identity + run summary,
/// so consumers can trace a quality event back to an exact contract version.
fn contract_run_facet(result: &RunResult, producer: &str) -> Value {
    json!({
        "_producer": producer,
        "_schemaURL": format!("{REPO}/blob/main/docs/src/result-schema.md"),
        "dataset": result.contract.dataset,
        "contentSha256": result.contract.content_sha256,
        "toolVersion": result.tool_version,
        "status": match result.status {
            RunStatus::Passed => "passed",
            RunStatus::Failed => "failed",
        },
        "summary": {
            "checksTotal": result.summary.checks_total,
            "passed": result.summary.passed,
            "failedError": result.summary.failed_error,
            "failedWarn": result.summary.failed_warn,
        },
    })
}

/// Job facets: documentation always; ownership when the contract names an owner.
fn job_facets(result: &RunResult, producer: &str) -> Value {
    let mut facets = Map::new();
    facets.insert(
        "documentation".to_owned(),
        json!({
            "_producer": producer,
            "_schemaURL": "https://openlineage.io/spec/facets/1-0-0/DocumentationJobFacet.json",
            "description": format!(
                "PlexusPact data-contract validation for dataset `{}`.",
                result.contract.dataset
            ),
        }),
    );
    if let Some(owner) = &result.contract.owner {
        facets.insert(
            "ownership".to_owned(),
            json!({
                "_producer": producer,
                "_schemaURL": "https://openlineage.io/spec/facets/1-0-0/OwnershipJobFacet.json",
                "owners": [{ "name": owner, "type": "CONTRACT_OWNER" }],
            }),
        );
    }
    Value::Object(facets)
}

/// Formats a deterministic, RFC-4122-shaped UUID from the contract content hash
/// and the run's start instant. Deterministic so the same run reproduces the
/// same `runId` (idempotent emission) while distinct runs differ by their salt.
fn run_uuid(seed_hex: &str, salt: i64) -> String {
    let hex = if seed_hex.is_empty() { "0" } else { seed_hex };
    let hb = hex.as_bytes();
    let mut bytes = [0u8; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        let hi = hex_val(hb[(i * 2) % hb.len()]);
        let lo = hex_val(hb[(i * 2 + 1) % hb.len()]);
        *b = (hi << 4) | lo;
    }
    let salt_bytes = salt.to_le_bytes();
    for (i, sb) in salt_bytes.iter().enumerate() {
        bytes[i] ^= sb;
    }
    // Version 5 (name-based) nibble + RFC-4122 variant bits.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let h = |r: std::ops::Range<usize>| {
        bytes[r]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    format!(
        "{}-{}-{}-{}-{}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

/// Hex digit → value; non-hex bytes fold to 0.
fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn sample() -> RunResult {
        // Reuse the core sample by round-tripping a minimal document.
        let json = serde_json::json!({
            "result_schema_version": 1,
            "tool_version": "0.1.0",
            "contract": { "dataset": "user_signups", "content_sha256": "ab".repeat(32),
                          "owner": "growth@acme.com" },
            "source": { "path": "data.csv", "format": "csv", "rows": 1000, "columns": 2, "bytes": 4096 },
            "started_at": "2026-07-09T10:11:12Z",
            "duration_ms": 250,
            "status": "failed",
            "summary": { "checks_total": 2, "passed": 1, "failed_error": 1, "failed_warn": 0 },
            "checks": [
                { "id": "email.format.email", "column": "email", "kind": "format",
                  "params": {"format": "email"}, "severity": "error", "status": "failed",
                  "metrics": { "rows_evaluated": 1000, "rows_failed": 3, "null_ratio": 0.02 } },
                { "id": "dataset.row_count_min", "kind": "row_count_min",
                  "severity": "error", "status": "passed", "metrics": {} }
            ]
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn emits_valid_run_event_with_quality_facets() {
        let out = render_openlineage(&sample(), &OpenLineageOptions::default()).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        // A failed run maps to eventType FAIL.
        assert_eq!(v["eventType"], "FAIL");
        assert_eq!(v["job"]["name"], "plexuspact.check.user_signups");
        assert_eq!(v["inputs"][0]["name"], "user_signups");

        // Assertions facet: one per check, success reflects status, column carried.
        let assertions = v["inputs"][0]["facets"]["dataQualityAssertions"]["assertions"]
            .as_array()
            .unwrap();
        assert_eq!(assertions.len(), 2);
        assert_eq!(assertions[0]["assertion"], "email.format.email");
        assert_eq!(assertions[0]["success"], false);
        assert_eq!(assertions[0]["column"], "email");
        assert_eq!(assertions[1]["success"], true);

        // Metrics facet: row count + derived nullCount for the email column.
        let metrics = &v["inputs"][0]["inputFacets"]["dataQualityMetrics"];
        assert_eq!(metrics["rowCount"], 1000);
        assert_eq!(metrics["bytes"], 4096);
        assert_eq!(metrics["columnMetrics"]["email"]["nullCount"], 20);

        // Contract identity travels on a run facet.
        assert_eq!(
            v["run"]["facets"]["plexuspact_contract"]["dataset"],
            "user_signups"
        );
        assert_eq!(
            v["job"]["facets"]["ownership"]["owners"][0]["name"],
            "growth@acme.com"
        );

        // runId is a well-formed UUID (8-4-4-4-12 hex).
        let rid = v["run"]["runId"].as_str().unwrap();
        let parts: Vec<&str> = rid.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
    }

    #[test]
    fn passing_run_is_complete_and_run_id_is_deterministic() {
        let mut r = sample();
        r.status = RunStatus::Passed;
        r.summary.failed_error = 0;
        r.summary.passed = 2;
        let a = render_openlineage(&r, &OpenLineageOptions::default()).unwrap();
        let b = render_openlineage(&r, &OpenLineageOptions::default()).unwrap();
        assert_eq!(a, b, "same result must render byte-identically");
        let v: Value = serde_json::from_str(&a).unwrap();
        assert_eq!(v["eventType"], "COMPLETE");
    }
}
