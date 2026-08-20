//! Cross-renderer consistency: every output format is a pure function of the
//! same `RunResult`, so their counts can never disagree (crate-level
//! guarantee, doc 03 §1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use plexuspact_report::{render_human, render_json, render_junit, HumanOptions};

#[test]
fn human_json_and_junit_agree_on_counts() {
    let result = common::mixed_failures();

    // JSON: the summary object verbatim.
    let json: serde_json::Value =
        serde_json::from_str(&render_json(&result, false).unwrap()).unwrap();
    let total = json["summary"]["checks_total"].as_u64().unwrap();
    let failed_error = json["summary"]["failed_error"].as_u64().unwrap();
    let failed_warn = json["summary"]["failed_warn"].as_u64().unwrap();
    let failed = failed_error + failed_warn;

    // Human: the header repeats the same counts.
    let human = render_human(
        &result,
        &HumanOptions {
            color: false,
            max_samples: 5,
        },
    );
    let header = human.lines().next().unwrap();
    assert!(
        header.contains(&format!("{failed} of {total} checks failed")),
        "human header {header:?} must repeat the JSON summary counts"
    );
    // One aligned line per failed check.
    let failure_lines = human
        .lines()
        .filter(|l| l.starts_with("  \u{2717}") || l.starts_with("  \u{26a0}"))
        .count() as u64;
    assert_eq!(failure_lines, failed);

    // JUnit: tests = checks listed, failures = error failures, skipped = warn
    // failures (quick-junit 0.7 wire format: `skipped` is the emitted counter).
    let junit = render_junit(&result).unwrap();
    assert!(junit.contains(&format!(
        r#"<testsuite name="user_signups" tests="{total}""#
    )));
    assert!(junit.contains(&format!(r#"failures="{failed_error}""#)));
    assert!(junit.contains(&format!(r#"skipped="{failed_warn}""#)));
    assert_eq!(junit.matches("<testcase").count() as u64, total);
    assert_eq!(junit.matches("<failure").count() as u64, failed_error);
    assert_eq!(junit.matches("<skipped").count() as u64, failed_warn);
}

#[test]
fn all_pass_is_consistent_everywhere() {
    let result = common::all_pass();

    let json: serde_json::Value =
        serde_json::from_str(&render_json(&result, false).unwrap()).unwrap();
    assert_eq!(json["status"], "passed");
    let total = json["summary"]["checks_total"].as_u64().unwrap();

    let human = render_human(&result, &HumanOptions::default());
    assert!(human.contains(&format!("all {total} checks passed")));

    let junit = render_junit(&result).unwrap();
    assert!(junit.contains(&format!(
        r#"tests="{total}" skipped="0" errors="0" failures="0""#
    )));
}
