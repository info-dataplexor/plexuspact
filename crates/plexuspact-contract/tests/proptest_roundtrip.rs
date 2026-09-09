//! Property tests: any generated contract must survive
//! `serialize → parse → equal`, and `diff` must satisfy its invariants.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use indexmap::IndexMap;
use plexuspact_contract::{
    diff, parse_str, ApiVersion, ColType, ColumnCheck, ColumnDef, Consumer, ConsumerKind, Contract,
    DataClass, DatasetCheck, EnumValue, Impact, KnownFormat, LengthRange, LengthSpec, Migration,
    Number, PiiKind, Settings, Severity, Stability,
};
use proptest::prelude::*;

fn ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,11}"
}

fn text() -> impl Strategy<Value = String> {
    "[ -~]{0,24}"
}

fn severity() -> impl Strategy<Value = Severity> {
    prop_oneof![Just(Severity::Error), Just(Severity::Warn)]
}

fn number() -> impl Strategy<Value = Number> {
    prop_oneof![
        (-100_000i64..100_000).prop_map(Number::Int),
        (-100_000.0f64..100_000.0).prop_map(Number::Float),
    ]
}

fn ratio() -> impl Strategy<Value = f64> {
    0.0f64..=1.0
}

fn enum_value() -> impl Strategy<Value = EnumValue> {
    prop_oneof![
        any::<bool>().prop_map(EnumValue::Bool),
        (-1000i64..1000).prop_map(EnumValue::Int),
        (-1000.0f64..1000.0).prop_map(EnumValue::Float),
        ident().prop_map(EnumValue::String),
    ]
}

fn length_spec() -> impl Strategy<Value = LengthSpec> {
    prop_oneof![
        (0u64..1000).prop_map(LengthSpec::Exact),
        (
            proptest::option::of(0u64..1000),
            proptest::option::of(0u64..1000)
        )
            .prop_map(|(min, max)| LengthSpec::Range(LengthRange { min, max })),
    ]
}

fn known_format() -> impl Strategy<Value = KnownFormat> {
    prop_oneof![
        Just(KnownFormat::Email),
        Just(KnownFormat::Uuid),
        Just(KnownFormat::IsoDate),
        Just(KnownFormat::IsoDatetime),
        Just(KnownFormat::Url),
        Just(KnownFormat::CountryCodeIso2),
    ]
}

/// Every column-check form, including both severities and both unique modes.
fn column_check() -> impl Strategy<Value = ColumnCheck> {
    prop_oneof![
        (any::<bool>(), severity())
            .prop_map(|(approx, severity)| ColumnCheck::Unique { approx, severity }),
        severity().prop_map(|severity| ColumnCheck::NotEmptyString { severity }),
        (number(), severity()).prop_map(|(min, severity)| ColumnCheck::Min { min, severity }),
        (number(), severity()).prop_map(|(max, severity)| ColumnCheck::Max { max, severity }),
        (text(), severity()).prop_map(|(regex, severity)| ColumnCheck::Regex { regex, severity }),
        (proptest::collection::vec(enum_value(), 0..4), severity())
            .prop_map(|(values, severity)| ColumnCheck::Enum { values, severity }),
        (length_spec(), severity())
            .prop_map(|(length, severity)| ColumnCheck::Length { length, severity }),
        (known_format(), severity())
            .prop_map(|(format, severity)| ColumnCheck::Format { format, severity }),
        (ratio(), severity())
            .prop_map(|(ratio, severity)| ColumnCheck::NullRatioMax { ratio, severity }),
        (ratio(), severity())
            .prop_map(|(ratio, severity)| ColumnCheck::UniqueRatioMin { ratio, severity }),
        (text(), severity())
            .prop_map(|(expr, severity)| ColumnCheck::CustomExpr { expr, severity }),
    ]
}

fn col_type() -> impl Strategy<Value = ColType> {
    prop_oneof![
        Just(ColType::String),
        Just(ColType::Int),
        Just(ColType::Float),
        Just(ColType::Bool),
        Just(ColType::Date),
        Just(ColType::Datetime),
    ]
}

fn pii_kind() -> impl Strategy<Value = PiiKind> {
    prop_oneof![
        Just(PiiKind::None),
        Just(PiiKind::Email),
        Just(PiiKind::Phone),
        Just(PiiKind::Name),
        Just(PiiKind::Address),
        Just(PiiKind::NationalId),
        Just(PiiKind::Financial),
        Just(PiiKind::Health),
        Just(PiiKind::Other),
    ]
}

fn data_class() -> impl Strategy<Value = DataClass> {
    prop_oneof![
        Just(DataClass::Public),
        Just(DataClass::Internal),
        Just(DataClass::Confidential),
        Just(DataClass::Restricted),
    ]
}

fn stability() -> impl Strategy<Value = Stability> {
    prop_oneof![
        Just(Stability::Stable),
        Just(Stability::Beta),
        Just(Stability::Deprecated),
    ]
}

/// Days stop at 28 so every generated date is real in every month and year —
/// the round trip is testing serialization, not the calendar.
fn iso_date() -> impl Strategy<Value = String> {
    (2000u32..2100, 1u32..13, 1u32..29).prop_map(|(y, m, d)| format!("{y:04}-{m:02}-{d:02}"))
}

fn column_def() -> impl Strategy<Value = ColumnDef> {
    (
        col_type(),
        proptest::option::of("[a-zA-Z0-9 ,.'-]{1,60}"),
        any::<bool>(),
        proptest::option::of(pii_kind()),
        proptest::option::of(data_class()),
        stability(),
        proptest::option::of(iso_date()),
        proptest::collection::vec(column_check(), 0..4),
    )
        .prop_map(
            |(r#type, description, required, pii, classification, stability, sunset, checks)| {
                ColumnDef {
                    r#type,
                    description,
                    required,
                    pii,
                    classification,
                    stability,
                    sunset,
                    checks,
                }
            },
        )
}

fn migration() -> impl Strategy<Value = Migration> {
    (iso_date(), proptest::option::of("[a-zA-Z0-9 ,.'-]{1,60}"))
        .prop_map(|(window_ends, note)| Migration { window_ends, note })
}

fn duration() -> impl Strategy<Value = Duration> {
    (0u64..10_000_000).prop_map(Duration::from_secs)
}

fn dataset_check() -> impl Strategy<Value = DatasetCheck> {
    prop_oneof![
        (any::<u64>(), severity())
            .prop_map(|(count, severity)| DatasetCheck::RowCountMin { count, severity }),
        (any::<u64>(), severity())
            .prop_map(|(count, severity)| DatasetCheck::RowCountMax { count, severity }),
        (ident(), duration(), severity()).prop_map(|(column, max_age, severity)| {
            DatasetCheck::Freshness {
                column,
                max_age,
                severity,
            }
        }),
        (ident(), ratio(), severity()).prop_map(|(column, ratio, severity)| {
            DatasetCheck::NullRatioMax {
                column,
                ratio,
                severity,
            }
        }),
        (ident(), ratio(), severity()).prop_map(|(column, ratio, severity)| {
            DatasetCheck::UniqueRatioMin {
                column,
                ratio,
                severity,
            }
        }),
        (text(), severity())
            .prop_map(|(expr, severity)| DatasetCheck::CustomExpr { expr, severity }),
    ]
}

fn settings() -> impl Strategy<Value = Settings> {
    (any::<bool>(), any::<bool>(), severity()).prop_map(
        |(allow_extra_columns, columns_exact, on_type_mismatch)| Settings {
            allow_extra_columns,
            columns_exact,
            on_type_mismatch,
            input: Default::default(),
        },
    )
}

fn consumer() -> impl Strategy<Value = Consumer> {
    (
        ident(),
        proptest::option::of(text()),
        proptest::collection::vec(ident(), 0..3),
        proptest::option::of(prop_oneof![
            Just(ConsumerKind::Dashboard),
            Just(ConsumerKind::Model),
            Just(ConsumerKind::Api),
            Just(ConsumerKind::Pipeline),
            Just(ConsumerKind::AiAgent),
            Just(ConsumerKind::Report),
        ]),
        proptest::option::of(1u8..=3),
    )
        .prop_map(|(name, contact, reads, kind, tier)| Consumer {
            name,
            contact,
            reads,
            kind,
            tier,
        })
}

/// A full contract covering every check form and reserved field.
fn contract() -> impl Strategy<Value = Contract> {
    (
        ident(),
        proptest::option::of(text()),
        proptest::option::of(text()),
        proptest::option::of(ident()),
        proptest::collection::vec(consumer(), 0..3),
        proptest::collection::hash_map(ident(), column_def(), 0..4),
        proptest::collection::vec(dataset_check(), 0..4),
        proptest::option::of(migration()),
        settings(),
    )
        .prop_map(
            |(
                dataset,
                owner,
                description,
                version,
                consumers,
                columns,
                dataset_checks,
                migration,
                settings,
            )| {
                Contract {
                    api_version: ApiVersion::V1,
                    dataset,
                    owner,
                    description,
                    version,
                    consumers,
                    columns: columns.into_iter().collect::<IndexMap<_, _>>(),
                    primary_key: Vec::new(),
                    dataset_checks,
                    migration,
                    settings,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// serialize → parse → equal, for arbitrary contracts.
    #[test]
    fn roundtrip_serialize_then_parse(original in contract()) {
        let yaml = serde_yaml::to_string(&original).unwrap();
        let parsed = parse_str(&yaml, "prop.yaml")
            .unwrap_or_else(|e| panic!("generated yaml failed to parse: {e}\n---\n{yaml}"));
        prop_assert_eq!(parsed, original, "yaml was:\n{}", yaml);
    }

    /// diff(x, x) is empty.
    #[test]
    fn diff_of_identical_contracts_is_empty(c in contract()) {
        prop_assert_eq!(diff(&c, &c), vec![]);
    }

    /// A breaking old→new change is never invisible (or merely cosmetic)
    /// when looking new→old: breaking-ness is asymmetric, not symmetric noise.
    #[test]
    fn breaking_changes_are_not_cosmetic_in_reverse(a in contract(), b in contract()) {
        let forward = diff(&a, &b);
        if forward.iter().any(|c| c.impact == Impact::Breaking) {
            let reverse = diff(&b, &a);
            prop_assert!(!reverse.is_empty(), "reverse diff empty though forward was breaking");
            prop_assert!(
                reverse.iter().any(|c| c.impact != Impact::Cosmetic),
                "reverse diff all-cosmetic though forward was breaking: {:#?}",
                reverse
            );
        }
    }
}
