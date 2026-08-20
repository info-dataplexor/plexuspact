//! Tests for the JUnit XML renderer, including the warn→skipped mapping.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use plexuspact_report::render_junit;

#[test]
fn junit_mixed_failures_snapshot() {
    let out = render_junit(&common::mixed_failures()).unwrap();
    insta::assert_snapshot!("junit_mixed_failures", out);
}

#[test]
fn junit_suite_is_named_after_the_dataset() {
    let out = render_junit(&common::mixed_failures()).unwrap();
    assert!(out.contains(r#"<testsuite name="user_signups""#));
}

#[test]
fn junit_warn_failures_are_skipped_not_failures() {
    let out = render_junit(&common::mixed_failures()).unwrap();
    // Two error failures, one warn failure (quick-junit 0.7 emits the warn
    // count in the `skipped` attribute).
    assert!(out.contains(r#"failures="2""#));
    assert!(out.contains(r#"skipped="1""#));
    // The warn check is a <skipped> element carrying the message, never <failure>.
    assert!(out.contains(r#"<skipped message="[warn] newest row is 51h old" type="freshness""#));
    assert!(!out.contains(r#"type="freshness"></failure"#));
    assert!(!out.contains(r#"<failure message="[warn]"#));
}

#[test]
fn junit_failures_carry_message_and_samples() {
    let out = render_junit(&common::mixed_failures()).unwrap();
    assert!(out.contains(r#"classname="email""#));
    assert!(out.contains(r#"name="email.format.email""#));
    assert!(out.contains("3 rows failed (0.0002%)"));
    assert!(out.contains("row 10442: &quot;bob@@example&quot;"));
    assert!(out.contains("min observed: 11"));
    // Dataset-level checks use the fixed classname.
    assert!(out.contains(r#"classname="dataset""#));
}

#[test]
fn junit_all_pass_has_no_failures() {
    let out = render_junit(&common::all_pass()).unwrap();
    assert!(out.contains(r#"tests="5""#));
    assert!(out.contains(r#"failures="0""#));
    assert!(!out.contains("<failure"));
    assert!(!out.contains("<skipped"));
}
