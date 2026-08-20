//! Human terminal renderer (PRD §7) — the default `plexuspact check` output.
//!
//! A pure function of [`RunResult`]: the header repeats `summary` counts, each
//! failed check repeats its own `metrics`, `samples`, and `message` verbatim.
//! Nothing is recomputed here.

use owo_colors::OwoColorize;
use plexuspact_core::result::{CheckResult, CheckSeverity, RunResult, RunStatus};

use crate::util::{duration_secs, param_summary, percent, thousands};

/// Options for [`render_human`].
///
/// The CLI decides TTY detection and `NO_COLOR` handling and passes the
/// verdict in via `color`; this crate never inspects the environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanOptions {
    /// Emit ANSI color codes. When `false` the output contains no escape
    /// sequences at all.
    pub color: bool,
    /// Maximum number of sample failing rows printed per check.
    pub max_samples: usize,
}

impl Default for HumanOptions {
    fn default() -> Self {
        Self {
            color: false,
            max_samples: 5,
        }
    }
}

/// Terminal tints used by this renderer.
#[derive(Debug, Clone, Copy)]
enum Tint {
    Green,
    Red,
    Yellow,
}

/// Applies `tint` to `text` when color is enabled, otherwise returns it as-is.
fn paint(text: &str, tint: Tint, enabled: bool) -> String {
    if !enabled {
        return text.to_owned();
    }
    match tint {
        Tint::Green => text.green().to_string(),
        Tint::Red => text.red().to_string(),
        Tint::Yellow => text.yellow().to_string(),
    }
}

/// One prepared line for a failed check, kept apart from its color so that
/// column alignment is computed on the plain text.
struct FailLine {
    symbol: &'static str,
    tint: Tint,
    /// `column · kind: params` (without the leading symbol).
    left: String,
    /// Right-hand summary: rows failed + percentage, or the check message.
    right: String,
    /// Whether to append the trailing `[warn]` tag.
    warn: bool,
    /// Indented detail lines below the check (samples, message).
    details: Vec<String>,
}

/// Renders `result` in the human terminal format of PRD §7.
///
/// When every check passed, the output is the single ✓ header line. Otherwise
/// the header states how many checks failed, followed by one aligned line per
/// failed check with its sample rows and message indented beneath. Passed
/// checks are never listed individually.
#[must_use]
pub fn render_human(result: &RunResult, opts: &HumanOptions) -> String {
    let summary = &result.summary;
    let failed = summary.failed_error + summary.failed_warn;
    let stats = format!(
        "({} rows in {})",
        thousands(result.source.rows),
        duration_secs(result.duration_ms)
    );

    let (symbol, tint) = match result.status {
        RunStatus::Passed => ("\u{2713}", Tint::Green),
        RunStatus::Failed => ("\u{2717}", Tint::Red),
    };
    let sym = paint(symbol, tint, opts.color);

    if failed == 0 {
        return format!(
            "{sym} {} \u{2014} all {} checks passed {stats}\n",
            result.contract.dataset, summary.checks_total
        );
    }

    let mut out = format!(
        "{sym} {} \u{2014} {failed} of {} checks failed {stats}\n\n",
        result.contract.dataset, summary.checks_total
    );

    let lines: Vec<FailLine> = result
        .checks
        .iter()
        .filter(|c| c.status == plexuspact_core::result::CheckStatus::Failed)
        .map(|c| fail_line(c, opts.max_samples))
        .collect();
    // Right column starts 4 spaces after the widest left column.
    let max_left = lines
        .iter()
        .map(|l| l.symbol.chars().count() + 1 + l.left.chars().count())
        .max()
        .unwrap_or(0);

    for line in &lines {
        let width = line.symbol.chars().count() + 1 + line.left.chars().count();
        let mut rendered = format!(
            "  {} {}",
            paint(line.symbol, line.tint, opts.color),
            line.left
        );
        if !line.right.is_empty() {
            rendered.push_str(&" ".repeat(max_left - width + 4));
            rendered.push_str(&line.right);
        }
        if line.warn {
            rendered.push_str("   ");
            rendered.push_str(&paint("[warn]", Tint::Yellow, opts.color));
        }
        out.push_str(&rendered);
        out.push('\n');
        for detail in &line.details {
            out.push_str("      ");
            out.push_str(detail);
            out.push('\n');
        }
    }
    out
}

/// Prepares the aligned line + details for one failed check. Sample rows are
/// capped at `max_samples`; the verbatim message (when not already used as the
/// right-hand summary) always follows them.
fn fail_line(check: &CheckResult, max_samples: usize) -> FailLine {
    let (symbol, tint) = match check.severity {
        CheckSeverity::Error => ("\u{2717}", Tint::Red),
        CheckSeverity::Warn => ("\u{26a0}", Tint::Yellow),
    };
    let target = check.column.as_deref().unwrap_or("dataset");
    let left = format!(
        "{target} \u{00b7} {}",
        param_summary(&check.kind, &check.params)
    );

    // Right column: the rows-failed summary when the engine counted rows,
    // otherwise the check's own message (e.g. freshness: "newest row is 51h old").
    let mut message_consumed = false;
    let right = match check.metrics.rows_failed {
        Some(rows) => {
            let mut r = format!("{} rows failed", thousands(rows));
            if let Some(ratio) = check.metrics.fail_ratio {
                r.push_str(&format!(" ({})", percent(ratio)));
            }
            r
        }
        None => {
            message_consumed = true;
            check.message.clone().unwrap_or_default()
        }
    };

    let mut details: Vec<String> = check
        .samples
        .iter()
        .take(max_samples)
        .map(|s| format!("row {}: \"{}\"", s.row, s.value))
        .collect();
    if !message_consumed {
        if let Some(message) = &check.message {
            details.push(message.clone());
        }
    }

    FailLine {
        symbol,
        tint,
        left,
        right,
        warn: check.severity == CheckSeverity::Warn,
        details,
    }
}
