//! Tests for the JSON wire-format renderer and sample redaction.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use plexuspact_core::result::RunResult;
use plexuspact_report::{render_json, render_json_redacted};

#[test]
fn json_pretty_snapshot() {
    let out = render_json(&common::mixed_failures(), true).unwrap();
    insta::assert_snapshot!("json_pretty_mixed_failures", out);
}

#[test]
fn json_round_trips_to_the_same_document() {
    let result = common::mixed_failures();
    for pretty in [false, true] {
        let out = render_json(&result, pretty).unwrap();
        let back: RunResult = serde_json::from_str(&out).unwrap();
        assert_eq!(
            back, result,
            "render_json must be the exact serde form (pretty={pretty})"
        );
    }
}

#[test]
fn json_compact_is_single_line() {
    let out = render_json(&common::mixed_failures(), false).unwrap();
    assert_eq!(out.lines().count(), 1);
}

#[test]
fn redacted_json_hides_values_but_keeps_rows() {
    let result = common::mixed_failures();
    let out = render_json_redacted(&result, true).unwrap();

    // No sample value leaks.
    for check in &result.checks {
        for sample in &check.samples {
            if !sample.value.is_empty() {
                assert!(
                    !out.contains(&sample.value),
                    "redacted output must not contain sample value {:?}",
                    sample.value
                );
            }
        }
    }
    assert!(out.contains("<redacted>"));

    // Row numbers and every non-sample field are intact.
    let back: RunResult = serde_json::from_str(&out).unwrap();
    assert_eq!(back.summary, result.summary);
    assert_eq!(back.checks[3].samples[0].row, 10_442);
    assert_eq!(back.checks[3].samples.len(), result.checks[3].samples.len());
}
