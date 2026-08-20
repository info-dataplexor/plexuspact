//! Snapshot tests of rendered parse-error diagnostics (US-8: "errors teach me").
//!
//! Rendered with miette's graphical handler, unicode theme without color and
//! a fixed 80-column width, so output is deterministic across machines.
//! Review criteria: every snapshot must show the file name, a line/column
//! pointer into the source, what was expected, and (where known) a fix hint.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use miette::{GraphicalReportHandler, GraphicalTheme};
use plexuspact_contract::{parse_str, ParseError};

fn render(src: &str) -> String {
    let err: ParseError = parse_str(src, "contract.yaml").unwrap_err();
    let mut out = String::new();
    GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
        .with_width(80)
        .render_report(&mut out, &err)
        .unwrap();
    out
}

#[test]
fn snapshot_invalid_yaml_syntax() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n\tuser_id: { type: string }\n"
    ));
}

#[test]
fn snapshot_unknown_top_level_key() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndatasett: user_signups\ncolumns:\n  id: { type: string }\n"
    ));
}

#[test]
fn snapshot_wrong_api_version() {
    insta::assert_snapshot!(render(
        "apiVersion: v2\ndataset: t\ncolumns:\n  id: { type: string }\n"
    ));
}

#[test]
fn snapshot_missing_api_version() {
    insta::assert_snapshot!(render("dataset: t\ncolumns:\n  id: { type: string }\n"));
}

#[test]
fn snapshot_missing_dataset() {
    insta::assert_snapshot!(render("apiVersion: v1\ncolumns:\n  id: { type: string }\n"));
}

#[test]
fn snapshot_unknown_column_field() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  id: { typ: string }\n"
    ));
}

#[test]
fn snapshot_unknown_column_type() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: text }\n"
    ));
}

#[test]
fn snapshot_unknown_check_name() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string, checks: [uniq] }\n"
    ));
}

#[test]
fn snapshot_bare_check_needing_value() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  age: { type: int, checks: [min] }\n"
    ));
}

#[test]
fn snapshot_wrong_check_value_type() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  age: { type: int, checks: [{ min: eighteen }] }\n"
    ));
}

#[test]
fn snapshot_unknown_format() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  email: { type: string, checks: [{ format: emial }] }\n"
    ));
}

#[test]
fn snapshot_bad_severity() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  age: { type: int, checks: [{ min: 18, severity: fatal }] }\n"
    ));
}

#[test]
fn snapshot_two_checks_in_one_map() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  age: { type: int, checks: [{ min: 18, max: 99 }] }\n"
    ));
}

#[test]
fn snapshot_bad_freshness_duration() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  ts: { type: datetime }\ndataset_checks:\n  - freshness: { column: ts, max_age: two-days }\n"
    ));
}

#[test]
fn snapshot_unknown_dataset_check() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  id: { type: string }\ndataset_checks:\n  - row_cout_min: 1\n"
    ));
}

#[test]
fn snapshot_empty_contract() {
    insta::assert_snapshot!(render("\n"));
}

#[test]
fn snapshot_missing_pii_value_is_unknown_variant() {
    insta::assert_snapshot!(render(
        "apiVersion: v1\ndataset: t\ncolumns:\n  email: { type: string, pii: emails }\n"
    ));
}
