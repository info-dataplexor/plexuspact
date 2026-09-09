//! `run` — orchestrate one validation and assemble a [`RunResult`].

use chrono::{DateTime, Utc};
use plexuspact_contract::{Consumer, Contract, InputSettings, Severity};
use plexuspact_engine::{execute, profile, CheckOutcome, DatasetProfile, RunOptions};
use plexuspact_io::{
    delimiter_byte, detect, open, resolve, FixedWidthOptions, InputFormat, IoError, ReadOptions,
    Source,
};
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

/// Reading instructions given by the caller (command-line flags), as text.
///
/// They layer on top of the contract's `settings.input`: the contract is the
/// durable place for how a feed is read, the flags are for the one-off — a
/// different sheet this month, a header-less re-export. Every field is
/// optional; an unset field leaves the contract's value in force.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputOverrides {
    /// Format name (`csv`, `excel`, `xml`, …); replaces extension detection.
    pub format: Option<String>,
    /// JSON only: dotted path to the record array.
    pub json_path: Option<String>,
    /// CSV only: one character, or `\t`.
    pub delimiter: Option<String>,
    /// Whether the first row names the columns.
    pub has_header: Option<bool>,
    /// Workbooks only: sheet name or 1-based position.
    pub sheet: Option<String>,
    /// Rows to skip before the header.
    pub skip_rows: Option<u32>,
    /// XML only: the element that is one record.
    pub xml_record: Option<String>,
    /// Fixed-width only: the layout, `id=1-8,name=9-40,amount=12`.
    pub fixed_width: Option<String>,
}

impl InputOverrides {
    /// Whether nothing is set.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == InputOverrides::default()
    }

    /// The overrides as a contract `settings.input` block, so `init` can
    /// record in the draft what it was told. Fails on a format name or a
    /// fixed-width spec that does not parse.
    pub fn to_input_settings(&self) -> Result<InputSettings, CoreError> {
        let format = match (&self.format, &self.fixed_width) {
            (Some(name), _) => Some(parse_format(name)?),
            // A layout only means one thing; mainframe extracts are `.txt`
            // and `.dat`, so the format cannot come from the extension.
            (None, Some(_)) => Some(InputFormat::FixedWidth),
            (None, None) => None,
        };
        Ok(InputSettings {
            format,
            delimiter: self.delimiter.clone(),
            has_header: self.has_header,
            json_path: self.json_path.clone(),
            sheet: self.sheet.clone(),
            skip_rows: self.skip_rows,
            xml_record: self.xml_record.clone(),
            fixed_width: match &self.fixed_width {
                Some(spec) => FixedWidthOptions::parse_fields(spec)?,
                None => Vec::new(),
            },
        })
    }
}

fn parse_format(name: &str) -> Result<InputFormat, CoreError> {
    InputFormat::from_name(name).ok_or_else(|| {
        IoError::InvalidOption {
            key: "--input-format".to_owned(),
            detail: format!(
                "`{name}` is not a known format; use one of \
                 csv, tsv, parquet, ndjson, json, excel, xml, fixed_width"
            ),
        }
        .into()
    })
}

/// Builds reader options from a contract's `settings.input` and the caller's
/// overrides, in that order.
pub fn read_options_for(
    input: &InputSettings,
    overrides: &InputOverrides,
) -> Result<ReadOptions, CoreError> {
    let mut opts = ReadOptions::default();
    opts.apply_input_settings(input)?;

    if let Some(name) = &overrides.format {
        opts.format = Some(parse_format(name)?);
    } else if overrides.fixed_width.is_some() {
        opts.format = Some(InputFormat::FixedWidth);
    }
    if let Some(p) = &overrides.json_path {
        opts.json_path = Some(p.clone());
    }
    if let Some(d) = &overrides.delimiter {
        opts.csv.delimiter = Some(delimiter_byte(d, "--delimiter")?);
    }
    if let Some(s) = &overrides.sheet {
        opts.excel.sheet = Some(s.clone());
    }
    if let Some(r) = &overrides.xml_record {
        opts.xml.record = Some(r.clone());
    }
    if let Some(spec) = &overrides.fixed_width {
        let mut fw = FixedWidthOptions::parse(spec)?;
        // Keep what the contract said about the header and leading lines
        // unless the flags say otherwise (below).
        fw.has_header = input.has_header.unwrap_or(false);
        fw.skip_rows = input.skip_rows.unwrap_or(0) as usize;
        opts.fixed_width = Some(fw);
    }
    if let Some(h) = overrides.has_header {
        opts.csv.has_header = h;
        opts.excel.has_header = Some(h);
        if let Some(fw) = opts.fixed_width.as_mut() {
            fw.has_header = h;
        }
    }
    if let Some(n) = overrides.skip_rows {
        opts.csv.skip_rows = n as usize;
        opts.excel.skip_rows = n as usize;
        if let Some(fw) = opts.fixed_width.as_mut() {
            fw.skip_rows = n as usize;
        }
    }
    Ok(opts)
}

/// Convenience entry point for the CLI: validates a data file/stdin path against
/// an already-parsed contract, keeping `plexuspact-io` out of the caller's
/// dependency graph (layering rule).
///
/// The contract's `settings.input` is applied; `input_format` and `json_path`
/// override it. See [`run_check_with`] for the full set of overrides.
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
    let overrides = InputOverrides {
        format: input_format.map(str::to_owned),
        json_path: json_path.map(str::to_owned),
        ..InputOverrides::default()
    };
    run_check_with(
        contract,
        contract_bytes,
        contract_path,
        data_path,
        &overrides,
        sample_failures,
        redact_samples,
        now,
        tool_version,
    )
}

/// [`run_check`] with every reading instruction the caller can give. The
/// contract's `settings.input` is applied first, then `overrides`.
#[allow(clippy::too_many_arguments)]
pub fn run_check_with(
    contract: Contract,
    contract_bytes: Vec<u8>,
    contract_path: Option<String>,
    data_path: &str,
    overrides: &InputOverrides,
    sample_failures: usize,
    redact_samples: bool,
    now: Option<DateTime<Utc>>,
    tool_version: &str,
) -> Result<RunResult, CoreError> {
    let source = resolve(data_path);
    let read_options = read_options_for(&contract.settings.input, overrides)?;
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
    let overrides = InputOverrides {
        format: input_format.map(str::to_owned),
        json_path: json_path.map(str::to_owned),
        ..InputOverrides::default()
    };
    profile_path_with(data_path, &overrides)
}

/// [`profile_path`] with every reading instruction the caller can give.
pub fn profile_path_with(
    data_path: &str,
    overrides: &InputOverrides,
) -> Result<DatasetProfile, CoreError> {
    let source = resolve(data_path);
    let read_options = read_options_for(&InputSettings::default(), overrides)?;
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

#[cfg(test)]
mod input_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn contract_input_then_overrides() {
        let input = InputSettings {
            format: Some(InputFormat::Excel),
            sheet: Some("Orders".into()),
            skip_rows: Some(2),
            ..Default::default()
        };
        let opts = read_options_for(&input, &InputOverrides::default()).unwrap();
        assert_eq!(opts.format, Some(InputFormat::Excel));
        assert_eq!(opts.excel.sheet.as_deref(), Some("Orders"));
        assert_eq!(opts.excel.skip_rows, 2);

        let overrides = InputOverrides {
            sheet: Some("2".into()),
            has_header: Some(false),
            ..Default::default()
        };
        let opts = read_options_for(&input, &overrides).unwrap();
        assert_eq!(opts.excel.sheet.as_deref(), Some("2"));
        assert_eq!(opts.excel.has_header, Some(false));
        assert_eq!(opts.excel.skip_rows, 2, "untouched by the flags");
    }

    #[test]
    fn fixed_width_flags_layer() {
        let input = InputSettings {
            has_header: Some(true),
            ..Default::default()
        };
        let overrides = InputOverrides {
            fixed_width: Some("id=1-4,name=5-20".into()),
            skip_rows: Some(1),
            ..Default::default()
        };
        let opts = read_options_for(&input, &overrides).unwrap();
        let fw = opts.fixed_width.unwrap();
        assert_eq!(fw.fields.len(), 2);
        assert!(fw.has_header, "from the contract");
        assert_eq!(fw.skip_rows, 1, "from the flag");
    }

    #[test]
    fn bad_flags_are_user_errors() {
        let overrides = InputOverrides {
            format: Some("dbf".into()),
            ..Default::default()
        };
        let e = read_options_for(&InputSettings::default(), &overrides).unwrap_err();
        assert!(e.is_user_error());
        assert!(e.to_string().contains("--input-format"), "{e}");

        let overrides = InputOverrides {
            delimiter: Some(";;".into()),
            ..Default::default()
        };
        let e = read_options_for(&InputSettings::default(), &overrides).unwrap_err();
        assert!(e.to_string().starts_with("--delimiter:"), "{e}");
    }

    #[test]
    fn overrides_become_a_settings_block() {
        let overrides = InputOverrides {
            format: Some("xlsx".into()),
            sheet: Some("Data".into()),
            fixed_width: Some("id=1-4,rest=10".into()),
            ..Default::default()
        };
        let input = overrides.to_input_settings().unwrap();
        assert_eq!(
            input.format,
            Some(InputFormat::Excel),
            "an explicit format wins"
        );
        let implied = InputOverrides {
            fixed_width: Some("id=4".into()),
            ..Default::default()
        }
        .to_input_settings()
        .unwrap();
        assert_eq!(implied.format, Some(InputFormat::FixedWidth));
        assert_eq!(input.sheet.as_deref(), Some("Data"));
        assert_eq!(input.fixed_width.len(), 2);
        assert_eq!(input.fixed_width[1].width, Some(10));
        assert!(InputOverrides::default()
            .to_input_settings()
            .unwrap()
            .is_empty());
    }
}
