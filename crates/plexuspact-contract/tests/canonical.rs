//! The canonical contract from PRD §7 (fixtures/contract.yaml) must parse,
//! populate every field, lint clean, and round-trip through serialization.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::time::Duration;

use plexuspact_contract::{
    diff, parse_file, parse_str, validate, ApiVersion, ColType, ColumnCheck, Consumer,
    DatasetCheck, EnumValue, KnownFormat, LengthSpec, Number, PiiKind, Severity,
};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/contract.yaml")
}

#[test]
fn canonical_contract_parses_with_every_field() {
    let c = parse_file(fixture_path()).expect("canonical contract must parse");

    assert_eq!(c.api_version, ApiVersion::V1);
    assert_eq!(c.dataset, "user_signups");
    assert_eq!(c.owner.as_deref(), Some("growth-team@acme.com"));
    assert_eq!(
        c.description.as_deref(),
        Some("Daily signup export from the app database.")
    );
    assert_eq!(c.version, None);

    // consumers (reserved field, FR-11)
    assert_eq!(
        c.consumers,
        vec![Consumer {
            name: "analytics-core".into(),
            contact: Some("data-team@acme.com".into()),
        }]
    );

    // columns, in YAML declaration order
    let names: Vec<&str> = c.columns.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["user_id", "email", "age", "country", "plan", "signed_up"]
    );

    let user_id = &c.columns["user_id"];
    assert_eq!(user_id.r#type, ColType::String);
    assert!(user_id.required);
    assert_eq!(
        user_id.checks,
        vec![ColumnCheck::Unique {
            approx: false,
            severity: Severity::Error
        }]
    );

    let email = &c.columns["email"];
    assert_eq!(email.r#type, ColType::String);
    assert!(email.required);
    assert_eq!(email.pii, Some(PiiKind::Email));
    assert_eq!(
        email.checks,
        vec![ColumnCheck::Format {
            format: KnownFormat::Email,
            severity: Severity::Error
        }]
    );

    let age = &c.columns["age"];
    assert_eq!(age.r#type, ColType::Int);
    assert!(!age.required);
    assert_eq!(
        age.checks,
        vec![
            ColumnCheck::Min {
                min: Number::Int(18),
                severity: Severity::Error
            },
            ColumnCheck::Max {
                max: Number::Int(120),
                severity: Severity::Error
            },
        ]
    );

    let country = &c.columns["country"];
    assert_eq!(
        country.checks,
        vec![
            ColumnCheck::Length {
                length: LengthSpec::Exact(2),
                severity: Severity::Error
            },
            ColumnCheck::Regex {
                regex: "^[A-Z]{2}$".into(),
                severity: Severity::Error
            },
        ]
    );

    let plan = &c.columns["plan"];
    assert_eq!(
        plan.checks,
        vec![ColumnCheck::Enum {
            values: vec![
                EnumValue::String("free".into()),
                EnumValue::String("pro".into()),
                EnumValue::String("enterprise".into()),
            ],
            severity: Severity::Error
        }]
    );

    let signed_up = &c.columns["signed_up"];
    assert_eq!(signed_up.r#type, ColType::Datetime);
    assert!(signed_up.required);
    assert!(signed_up.checks.is_empty());

    // dataset checks
    assert_eq!(
        c.dataset_checks,
        vec![
            DatasetCheck::RowCountMin {
                count: 1,
                severity: Severity::Error
            },
            DatasetCheck::Freshness {
                column: "signed_up".into(),
                max_age: Duration::from_secs(48 * 3600),
                severity: Severity::Warn
            },
            DatasetCheck::NullRatioMax {
                column: "age".into(),
                ratio: 0.10,
                severity: Severity::Error
            },
        ]
    );

    // settings
    assert!(c.settings.allow_extra_columns);
    assert!(!c.settings.columns_exact);
    assert_eq!(c.settings.on_type_mismatch, Severity::Error);
}

#[test]
fn canonical_contract_lints_clean() {
    let c = parse_file(fixture_path()).unwrap();
    assert_eq!(validate(&c), vec![]);
}

#[test]
fn canonical_contract_round_trips() {
    let c = parse_file(fixture_path()).unwrap();
    let yaml = serde_yaml::to_string(&c).unwrap();
    let reparsed = parse_str(&yaml, "roundtrip.yaml").unwrap();
    assert_eq!(reparsed, c, "serialized yaml was:\n{yaml}");
}

#[test]
fn canonical_contract_self_diff_is_empty() {
    let c = parse_file(fixture_path()).unwrap();
    assert_eq!(diff(&c, &c), vec![]);
}
