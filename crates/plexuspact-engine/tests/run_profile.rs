//! Integration test: a `check` run records what every column looked like.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use plexuspact_engine::{execute, EngineOutput, RunOptions};
use plexuspact_io::{open, resolve, ReadOptions};

fn run(profile: bool) -> EngineOutput {
    let yaml = std::fs::read_to_string("../../fixtures/contract.yaml").unwrap();
    let contract = plexuspact_contract::parse_str(&yaml, "contract.yaml").unwrap();
    let source = resolve("../../fixtures/signups_small.csv");
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    execute(
        bs.as_mut(),
        &contract,
        &RunOptions {
            profile,
            ..RunOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn every_source_column_is_profiled_in_source_order() {
    let out = run(true);
    let profile = out.profile.expect("profiling is on");
    let names: Vec<&str> = profile.iter().map(|c| c.name.as_str()).collect();
    let observed: Vec<&str> = out
        .observed_columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, observed);
    assert_eq!(profile.len() as u64, out.source_columns);
}

#[test]
fn a_categorical_column_keeps_its_values_and_a_numeric_one_its_mean() {
    let out = run(true);
    let profile = out.profile.unwrap();
    let by_name = |n: &str| profile.iter().find(|c| c.name == n).unwrap();

    let plan = by_name("plan");
    assert!(plan.values_complete, "{plan:?}");
    assert!(plan.values.iter().any(|v| v == "free"), "{plan:?}");
    assert_eq!(plan.distinct as usize, plan.values.len());
    assert_eq!(plan.mean, None);

    let age = by_name("age");
    assert!(age.mean.is_some_and(|m| m > 0.0), "{age:?}");
    assert!(age.std.is_some(), "{age:?}");

    // Every column's null ratio is the share of the same row count.
    for c in &profile {
        assert!((0.0..=1.0).contains(&c.null_ratio), "{c:?}");
        assert!(c.null_count <= out.rows_total, "{c:?}");
    }
}

#[test]
fn profiling_can_be_switched_off_without_changing_a_verdict() {
    let with = run(true);
    let without = run(false);
    assert!(without.profile.is_none());
    assert_eq!(with.rows_total, without.rows_total);
    let verdicts = |o: &EngineOutput| -> Vec<(String, bool)> {
        o.checks.iter().map(|c| (c.id.clone(), c.failed)).collect()
    };
    assert_eq!(verdicts(&with), verdicts(&without));
}
