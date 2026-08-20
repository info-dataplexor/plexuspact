//! Self-contained HTML report renderer.
//!
//! The template is embedded at compile time (`include_str!`), all CSS is
//! inlined in a `<style>` block, and no JavaScript is required — the output
//! makes **zero external requests**. Tera autoescaping is on (the template is
//! registered under a `.html` name), so all user-controlled data (dataset
//! names, sample values, messages) is HTML-escaped.
//!
//! Like every renderer in this crate, this is a pure function of
//! [`RunResult`]: the view model below only *formats* fields for display.

use plexuspact_core::result::{CheckResult, CheckSeverity, CheckStatus, RunResult, RunStatus};
use serde::Serialize;
use tera::Tera;

use crate::util::{duration_secs, param_summary, percent, render_value, thousands};
use crate::RenderError;

/// The embedded report template.
static TEMPLATE: &str = include_str!("templates/report.html.tera");

/// Renders `result` as a single self-contained HTML document (no external
/// requests, no JavaScript, inline CSS; renders on a plain white background).
pub fn render_html(result: &RunResult) -> Result<String, RenderError> {
    let mut tera = Tera::default();
    // Registered under a `.html` name so Tera's default autoescape applies.
    tera.add_raw_template("report.html", TEMPLATE)?;
    let context = tera::Context::from_serialize(HtmlView::new(result))?;
    Ok(tera.render("report.html", &context)?)
}

/// Top-level template view model: every field is a display-ready string
/// formatted from `RunResult` (never computed).
#[derive(Debug, Serialize)]
struct HtmlView {
    dataset: String,
    passed: bool,
    status_label: String,
    tool_version: String,
    started_at: String,
    duration: String,
    source_path: String,
    source_format: String,
    rows: String,
    columns: String,
    bytes: Option<String>,
    contract_path: Option<String>,
    contract_sha_short: String,
    owner: Option<String>,
    checks_total: String,
    checks_passed: String,
    failed_error: String,
    failed_warn: String,
    groups: Vec<GroupView>,
}

/// Checks grouped under one column heading (or `dataset`).
#[derive(Debug, Serialize)]
struct GroupView {
    label: String,
    checks: Vec<CheckView>,
}

/// One check card.
#[derive(Debug, Serialize)]
struct CheckView {
    id: String,
    summary: String,
    severity: String,
    passed: bool,
    warn: bool,
    icon: String,
    status_label: String,
    rows_evaluated: Option<String>,
    rows_failed: Option<String>,
    fail_ratio: Option<String>,
    message: Option<String>,
    samples: Vec<SampleView>,
    observed: Vec<KeyValueView>,
}

/// One sampled failing row.
#[derive(Debug, Serialize)]
struct SampleView {
    row: String,
    value: String,
}

/// One observed-metric entry.
#[derive(Debug, Serialize)]
struct KeyValueView {
    key: String,
    value: String,
}

impl HtmlView {
    fn new(result: &RunResult) -> Self {
        let passed = result.status == RunStatus::Passed;
        let sha = &result.contract.content_sha256;
        let contract_sha_short = sha.chars().take(12).collect();

        let mut groups: Vec<GroupView> = Vec::new();
        for check in &result.checks {
            let label = check.column.clone().unwrap_or_else(|| "dataset".to_owned());
            let view = CheckView::new(check);
            match groups.last_mut() {
                Some(group) if group.label == label => group.checks.push(view),
                _ => groups.push(GroupView {
                    label,
                    checks: vec![view],
                }),
            }
        }

        Self {
            dataset: result.contract.dataset.clone(),
            passed,
            status_label: if passed {
                "PASSED".to_owned()
            } else {
                "FAILED".to_owned()
            },
            tool_version: result.tool_version.clone(),
            started_at: result
                .started_at
                .format("%Y-%m-%d %H:%M:%S UTC")
                .to_string(),
            duration: duration_secs(result.duration_ms),
            source_path: result.source.path.clone(),
            source_format: result.source.format.clone(),
            rows: thousands(result.source.rows),
            columns: thousands(result.source.columns),
            bytes: result.source.bytes.map(format_bytes),
            contract_path: result.contract.path.clone(),
            contract_sha_short,
            owner: result.contract.owner.clone(),
            checks_total: thousands(result.summary.checks_total),
            checks_passed: thousands(result.summary.passed),
            failed_error: thousands(result.summary.failed_error),
            failed_warn: thousands(result.summary.failed_warn),
            groups,
        }
    }
}

impl CheckView {
    fn new(check: &CheckResult) -> Self {
        let passed = check.status == CheckStatus::Passed;
        let warn = !passed && check.severity == CheckSeverity::Warn;
        let (icon, status_label) = if passed {
            ("\u{2713}", "passed")
        } else if warn {
            ("\u{26a0}", "failed (warn)")
        } else {
            ("\u{2717}", "failed")
        };
        Self {
            id: check.id.clone(),
            summary: param_summary(&check.kind, &check.params),
            severity: match check.severity {
                CheckSeverity::Error => "error".to_owned(),
                CheckSeverity::Warn => "warn".to_owned(),
            },
            passed,
            warn,
            icon: icon.to_owned(),
            status_label: status_label.to_owned(),
            rows_evaluated: check.metrics.rows_evaluated.map(thousands),
            rows_failed: check.metrics.rows_failed.map(thousands),
            fail_ratio: check.metrics.fail_ratio.map(percent),
            message: check.message.clone(),
            samples: check
                .samples
                .iter()
                .map(|s| SampleView {
                    row: thousands(s.row),
                    value: s.value.clone(),
                })
                .collect(),
            observed: check
                .metrics
                .observed
                .iter()
                .map(|(k, v)| KeyValueView {
                    key: k.clone(),
                    value: render_value(v),
                })
                .collect(),
        }
    }
}

/// Human-readable byte size with decimal units: `812345678` → `"812.3 MB"`.
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0; // now in KB (UNITS[0])
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
