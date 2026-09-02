//! `run` — orchestrate one validation and assemble a [`RunResult`].

use chrono::{DateTime, Utc};
use plexuspact_contract::{Consumer, Contract, Severity};
use plexuspact_engine::{execute, profile, CheckOutcome, DatasetProfile, RunOptions};
use plexuspact_io::{detect, open, resolve, InputFormat, ReadOptions, Source};
use sha2::{Digest, Sha256};

use crate::error::CoreError;
use crate::result::{
    CheckMetrics, CheckResult, CheckSeverity, CheckStatus, ConsumerRef, ContractRef, FailureSample,
    ObservedColumnInfo, ObservedSchema, RunResult, RunStatus, RunSummary, SourceInfo,
    RESULT_SCHEMA_VERSION,
};

/// A request to validate one source against one contract.
#[derive(Debug, Clone)]
pub struct RunRequest {
    /// The parsed, already-validated contract.
    pub contract: Contract,
    /// Raw contract bytes — hashed for the registry identity (`content_sha256`).
    pub contract_bytes: Vec<u8>,
    /// Path the contract was loaded from (for the report), if any.
    pub contract_path: Option<String>,
    /// The resolved data source.
    pub source: Source,
    /// Display path for the source (`-` for stdin).
    pub source_display: String,
    /// Reader options.
    pub read_options: ReadOptions,
    /// Failure samples captured per check.
    pub sample_failures: usize,
    /// Whether to mask sample values in the result (customer-data safety).
    pub redact_samples: bool,
    /// Injected clock for `freshness`; `None` uses the wall clock at run start.
    pub now: Option<DateTime<Utc>>,
}

/// Value written in place of a redacted sample.
const REDACTED: &str = "<redacted>";

/// Convenience entry point for the CLI: validates a data file/stdin path against
/// an already-parsed contract, keeping `plexuspact-io` out of the caller's
/// dependency graph (layering rule).
#[allow(clippy::too_many_arguments)]
pub fn run_check(
    contract: Contract,
    contract_bytes: Vec<u8>,
    contract_path: Option<String>,
    data_path: &str,
    input_format: Option<&str>,
    json_path: Option<&str>,
    sample_failures: usize,
    redact_samples: bool,
    now: Option<DateTime<Utc>>,
    tool_version: &str,
) -> Result<RunResult, CoreError> {
    let source = resolve(data_path);
    let read_options = ReadOptions {
        format: input_format.and_then(InputFormat::from_name),
        json_path: json_path.map(str::to_owned),
        ..ReadOptions::default()
    };
    let req = RunRequest {
        contract,
        contract_bytes,
        contract_path,
        source,
        source_display: data_path.to_owned(),
        read_options,
        sample_failures,
        redact_samples,
        now,
    };
    run(req, tool_version)
}

/// Convenience entry point for `plexuspact init`: profiles a data file/stdin
/// path. Keeps `plexuspact-io` out of the caller's dependency graph.
pub fn profile_path(
    data_path: &str,
    input_format: Option<&str>,
    json_path: Option<&str>,
) -> Result<DatasetProfile, CoreError> {
    let source = resolve(data_path);
    let read_options = ReadOptions {
        format: input_format.and_then(InputFormat::from_name),
        json_path: json_path.map(str::to_owned),
        ..ReadOptions::default()
    };
    Ok(profile(&source, &read_options)?)
}

/// Runs the contract's checks against the source and assembles a [`RunResult`].
pub fn run(req: RunRequest, tool_version: &str) -> Result<RunResult, CoreError> {
    let started_at = Utc::now();
    let now = req.now.unwrap_or(started_at);

    let format = source_format(&req.source, &req.read_options);
    let mut source = open(&req.source, &req.read_options)?;
    let size_bytes = source.size_bytes();

    let opts = RunOptions {
        sample_failures: req.sample_failures,
        now,
    };
    let engine_out = execute(source.as_mut(), &req.contract, &opts)?;

    let duration_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;

    let observed_schema = Some(ObservedSchema {
        typed: engine_out.observed_typed,
        columns: engine_out
            .observed_columns
            .iter()
            .map(|c| ObservedColumnInfo {
                name: c.name.clone(),
                dtype: c.dtype.clone(),
            })
            .collect(),
    });

    let checks: Vec<CheckResult> = engine_out
        .checks
        .into_iter()
        .map(|c| map_check(c, req.redact_samples))
        .collect();

    let summary = summarize(&checks);
    let status = if summary.failed_error > 0 {
        RunStatus::Failed
    } else {
        RunStatus::Passed
    };

    let content_sha256 = sha256_hex(&req.contract_bytes);

    Ok(RunResult {
        result_schema_version: RESULT_SCHEMA_VERSION,
        tool_version: tool_version.to_owned(),
        contract: ContractRef {
            dataset: req.contract.dataset.clone(),
            path: req.contract_path,
            content_sha256,
            owner: req.contract.owner.clone(),
            consumers: req.contract.consumers.iter().map(map_consumer).collect(),
        },
        source: SourceInfo {
            path: req.source_display,
            format: format.to_owned(),
            rows: engine_out.rows_total,
            columns: engine_out.source_columns,
            bytes: size_bytes,
        },
        started_at,
        duration_ms,
        status,
        summary,
        checks,
        observed_schema,
    })
}

/// Maps one engine outcome into a wire-format [`CheckResult`].
fn map_check(c: CheckOutcome, redact: bool) -> CheckResult {
    // The engine stashes `fail_ratio` in `observed`; lift it into the typed field.
    let mut observed = c.observed;
    let fail_ratio = observed.remove("fail_ratio").and_then(|v| v.as_f64());

    let samples = c
        .samples
        .into_iter()
        .map(|(row, value)| FailureSample {
            row,
            value: if redact { REDACTED.to_owned() } else { value },
        })
        .collect();

    CheckResult {
        id: c.id,
        column: c.column,
        kind: c.kind,
        params: c.params,
        severity: map_severity(c.severity),
        status: if c.failed {
            CheckStatus::Failed
        } else {
            CheckStatus::Passed
        },
        metrics: CheckMetrics {
            rows_evaluated: c.rows_evaluated,
            rows_failed: c.rows_failed,
            fail_ratio,
            observed,
        },
        samples,
        message: c.message,
    }
}

fn map_severity(s: Severity) -> CheckSeverity {
    match s {
        Severity::Error => CheckSeverity::Error,
        Severity::Warn => CheckSeverity::Warn,
    }
}

fn map_consumer(c: &Consumer) -> ConsumerRef {
    ConsumerRef {
        name: c.name.clone(),
        contact: c.contact.clone(),
    }
}

fn summarize(checks: &[CheckResult]) -> RunSummary {
    let mut summary = RunSummary {
        checks_total: checks.len() as u64,
        passed: 0,
        failed_error: 0,
        failed_warn: 0,
    };
    for c in checks {
        match (c.status, c.severity) {
            (CheckStatus::Passed, _) => summary.passed += 1,
            (CheckStatus::Failed, CheckSeverity::Error) => summary.failed_error += 1,
            (CheckStatus::Failed, CheckSeverity::Warn) => summary.failed_warn += 1,
        }
    }
    summary
}

/// Determines the display format name for the source.
fn source_format(source: &Source, opts: &ReadOptions) -> &'static str {
    match source {
        Source::File(path) => detect(path, opts.format)
            .map(|(f, _)| f.name())
            .unwrap_or("unknown"),
        Source::Stdin => opts.format.map(InputFormat::name).unwrap_or("stdin"),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}
