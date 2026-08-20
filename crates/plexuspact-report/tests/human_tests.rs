//! Snapshot + behavior tests for the human terminal renderer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use plexuspact_report::{render_human, HumanOptions};

fn plain() -> HumanOptions {
    HumanOptions {
        color: false,
        max_samples: 5,
    }
}

#[test]
fn human_mixed_failures_matches_prd_layout() {
    let out = render_human(&common::mixed_failures(), &plain());
    insta::assert_snapshot!("human_mixed_failures", out);
}

#[test]
fn human_all_pass_is_a_single_line() {
    let out = render_human(&common::all_pass(), &plain());
    insta::assert_snapshot!("human_all_pass", out);
    assert_eq!(out.lines().count(), 1);
}

#[test]
fn human_empty_dataset() {
    let out = render_human(&common::empty_dataset(), &plain());
    insta::assert_snapshot!("human_empty_dataset", out);
}

#[test]
fn human_huge_numbers_use_thousands_separators() {
    let out = render_human(&common::huge_numbers(), &plain());
    insta::assert_snapshot!("human_huge_numbers", out);
    assert!(out.contains("123,456,789 rows"));
    assert!(out.contains("1,234,567 rows failed (1%)"));
}

#[test]
fn color_off_output_has_no_ansi_escapes() {
    let out = render_human(&common::mixed_failures(), &plain());
    assert!(
        !out.contains('\u{1b}'),
        "color=false must not emit ANSI escapes"
    );
}

#[test]
fn color_on_output_has_ansi_escapes_but_same_content() {
    let colored = render_human(
        &common::mixed_failures(),
        &HumanOptions {
            color: true,
            max_samples: 5,
        },
    );
    assert!(
        colored.contains('\u{1b}'),
        "color=true must emit ANSI escapes"
    );
    // Stripping the escape sequences yields exactly the plain rendering.
    let stripped = strip_ansi(&colored);
    let plain_out = render_human(&common::mixed_failures(), &plain());
    assert_eq!(stripped, plain_out);
}

#[test]
fn max_samples_caps_sample_lines() {
    let out = render_human(
        &common::mixed_failures(),
        &HumanOptions {
            color: false,
            max_samples: 1,
        },
    );
    assert!(out.contains("row 10442: \"bob@@example\""));
    assert!(
        !out.contains("row 88310"),
        "second sample must be dropped at max_samples=1"
    );
    assert!(
        out.contains("min observed: 11"),
        "the message must survive sample capping"
    );
}

/// Minimal ANSI CSI stripper for test assertions.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip "[" and everything up to and including the final byte.
            if chars.peek() == Some(&'[') {
                for f in chars.by_ref() {
                    if f.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
