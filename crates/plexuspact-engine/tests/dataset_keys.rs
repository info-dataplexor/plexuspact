//! Integration tests for the dataset-level identity checks: `primary_key`,
//! `references` and `assert`. Small CSV files written to a temp dir, read the
//! way the CLI reads them (stringly), so the type conversions `assert` makes
//! from declared column types are exercised too.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use plexuspact_engine::{
    collect_key_set, execute, hash_key, CheckOutcome, EngineOutput, KeySet, ReferenceSets,
    RunOptions,
};
use plexuspact_io::{open, resolve, ReadOptions};

/// Tests run in parallel and several share a dataset name, so every file gets
/// its own directory: a reader may still have the previous one mapped.
static NEXT_FILE: AtomicUsize = AtomicUsize::new(0);

fn temp_csv(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "plexuspact-dataset-keys-{}-{}-{name}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.csv"));
    std::fs::write(&path, body).unwrap();
    path
}

fn run(yaml: &str, csv: &str, refs: ReferenceSets) -> EngineOutput {
    let contract = plexuspact_contract::parse_str(yaml, "contract.yaml").unwrap();
    let path = temp_csv(&contract.dataset, csv);
    let source = resolve(path.to_str().unwrap());
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    execute(
        bs.as_mut(),
        &contract,
        &RunOptions {
            references: refs,
            ..RunOptions::default()
        },
    )
    .unwrap()
}

fn find<'a>(out: &'a EngineOutput, id: &str) -> &'a CheckOutcome {
    out.checks.iter().find(|c| c.id == id).unwrap_or_else(|| {
        panic!(
            "no check `{id}` (have: {:?})",
            out.checks.iter().map(|c| c.id.clone()).collect::<Vec<_>>()
        )
    })
}

fn asserts(out: &EngineOutput) -> Vec<&CheckOutcome> {
    out.checks.iter().filter(|c| c.kind == "assert").collect()
}

const ORDERS: &str = r#"
apiVersion: v1
dataset: orders
owner: ops@acme.com
primary_key: [region, order_id]
columns:
  region: { type: string, required: true }
  order_id: { type: int, required: true }
  customer_id: { type: string }
  amount: { type: float }
  paid: { type: bool }
dataset_checks:
  - row_count_min: 1
  - references: { columns: [customer_id], dataset: customers }
  - assert: "SUM(amount) = 60.5"
  - assert: { expr: "COUNT(*) = 4", severity: warn }
"#;

const ORDER_ROWS: &str = "region,order_id,customer_id,amount,paid\n\
EU,1,c-1,10.5,true\n\
EU,2,c-2,20,false\n\
US,1,c-1,30,yes\n\
US,3,,0,no\n";

fn customers() -> KeySet {
    let path = temp_csv("customers", "id,name\nc-1,Ann\nc-2,Bo\nc-3,Cy\n");
    let source = resolve(path.to_str().unwrap());
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    collect_key_set(bs.as_mut(), &["id".to_owned()]).unwrap()
}

fn refs_with_customers(columns: Vec<&str>) -> ReferenceSets {
    let base = customers();
    let set = KeySet::from_hashes(
        columns.into_iter().map(str::to_owned).collect(),
        base.sorted_hashes(),
    );
    let mut refs = ReferenceSets::none();
    refs.insert("customers", set);
    refs
}

#[test]
fn primary_key_passes_and_yields_key_set_in_order() {
    let out = run(ORDERS, ORDER_ROWS, refs_with_customers(vec!["customer_id"]));
    let pk = find(&out, "dataset.primary_key");
    assert!(!pk.failed, "{pk:?}");
    assert_eq!(pk.kind, "primary_key");
    assert_eq!(pk.rows_evaluated, Some(4));
    assert_eq!(pk.observed["distinct_keys"], 4);
    assert_eq!(pk.observed["duplicate_rows"], 0);
    assert_eq!(pk.observed["null_key_rows"], 0);

    // Slot: after the column checks, before the dataset checks.
    let ids: Vec<&str> = out.checks.iter().map(|c| c.id.as_str()).collect();
    let pk_at = ids
        .iter()
        .position(|id| *id == "dataset.primary_key")
        .unwrap();
    let rc_at = ids
        .iter()
        .position(|id| *id == "dataset.row_count_min")
        .unwrap();
    assert_eq!(pk_at + 1, rc_at, "{ids:?}");

    let keys = out.primary_key.expect("key set produced");
    assert_eq!(keys.columns, vec!["region", "order_id"]);
    assert_eq!(keys.len(), 4);
    assert!(keys.contains(hash_key(["EU", "1"])));
    assert!(!keys.contains(hash_key(["1", "EU"])));
}

#[test]
fn primary_key_reports_duplicates_and_null_parts() {
    let rows = "region,order_id,customer_id,amount,paid\n\
EU,1,c-1,1,true\n\
EU,1,c-2,1,true\n\
,2,c-1,1,true\n\
US,1,c-1,1,true\n";
    let out = run(ORDERS, rows, refs_with_customers(vec!["customer_id"]));
    let pk = find(&out, "dataset.primary_key");
    assert!(pk.failed);
    assert_eq!(pk.rows_failed, Some(2));
    assert_eq!(pk.observed["distinct_keys"], 2);
    assert_eq!(pk.observed["duplicate_rows"], 1);
    assert_eq!(pk.observed["null_key_rows"], 1);
    assert_eq!(
        pk.samples,
        vec![
            (2, "region=EU, order_id=1".to_owned()),
            (3, "region=<null>, order_id=2 (null key)".to_owned())
        ]
    );
    let msg = pk.message.as_deref().unwrap();
    assert!(
        msg.contains("1 repeated key(s), e.g. \"region=EU, order_id=1\""),
        "{msg}"
    );
    assert!(msg.contains("1 row(s) with a null in the key"), "{msg}");
    // A failing key still yields the set: whoever stores runs decides.
    assert!(out.primary_key.is_some());
}

#[test]
fn primary_key_column_missing_from_source_cannot_run() {
    let rows = "region,customer_id,amount,paid\nEU,c-1,1,true\n";
    let out = run(ORDERS, rows, ReferenceSets::none());
    let pk = find(&out, "dataset.primary_key");
    assert!(pk.failed);
    assert_eq!(pk.rows_evaluated, None);
    assert_eq!(
        pk.message.as_deref(),
        Some("key column(s) missing from the source: order_id")
    );
    assert!(out.primary_key.is_none());
}

#[test]
fn references_passes_when_every_key_is_known_and_skips_nulls() {
    let out = run(ORDERS, ORDER_ROWS, refs_with_customers(vec!["customer_id"]));
    let r = find(&out, "dataset.references.customers");
    assert!(!r.failed, "{r:?}");
    assert_eq!(r.rows_evaluated, Some(3));
    assert_eq!(r.observed["rows_skipped_null"], 1);
    assert_eq!(r.observed["referenced_keys"], 3);
    assert_eq!(r.observed["referenced_dataset"], "customers");
}

#[test]
fn references_fails_on_unknown_keys_with_samples() {
    let rows = "region,order_id,customer_id,amount,paid\n\
EU,1,c-1,1,true\n\
EU,2,c-9,1,true\n\
US,1,c-8,1,true\n";
    let out = run(ORDERS, rows, refs_with_customers(vec!["customer_id"]));
    let r = find(&out, "dataset.references.customers");
    assert!(r.failed);
    assert_eq!(r.rows_failed, Some(2));
    assert_eq!(
        r.samples,
        vec![(2, "c-9".to_owned()), (3, "c-8".to_owned())]
    );
    assert_eq!(
        r.message.as_deref(),
        Some("2 row(s) point at keys `customers` does not have, e.g. \"c-9\"")
    );
}

#[test]
fn references_without_a_key_set_cannot_run_and_fails() {
    let out = run(ORDERS, ORDER_ROWS, ReferenceSets::none());
    let r = find(&out, "dataset.references.customers");
    assert!(r.failed);
    assert_eq!(r.rows_evaluated, None);
    let msg = r.message.as_deref().unwrap();
    assert!(
        msg.starts_with("check could not run: no keys known for `customers`"),
        "{msg}"
    );
    assert!(msg.contains("--reference customers=<file>"), "{msg}");
}

#[test]
fn references_refuses_a_key_set_over_other_columns() {
    let out = run(ORDERS, ORDER_ROWS, refs_with_customers(vec!["id"]));
    let r = find(&out, "dataset.references.customers");
    assert!(r.failed);
    let msg = r.message.as_deref().unwrap();
    assert!(msg.contains("keys known for `customers` are [id]"), "{msg}");
    assert!(msg.contains("points at [customer_id]"), "{msg}");
}

#[test]
fn assert_sums_declared_floats_from_a_text_source() {
    let out = run(ORDERS, ORDER_ROWS, refs_with_customers(vec!["customer_id"]));
    let checks = asserts(&out);
    assert_eq!(checks.len(), 2);
    assert!(!checks[0].failed, "{:?}", checks[0]);
    assert_eq!(checks[0].observed["rows"], 4);
    assert_eq!(checks[0].observed["columns"], serde_json::json!(["amount"]));
    assert!(!checks[1].failed, "{:?}", checks[1]);
    assert_eq!(checks[1].severity, plexuspact_contract::Severity::Warn);
}

#[test]
fn assert_false_null_and_per_row_forms_are_reported_distinctly() {
    let yaml = r#"
apiVersion: v1
dataset: ledger
owner: fin@acme.com
columns:
  amount: { type: float }
  paid: { type: bool }
  note: { type: string }
dataset_checks:
  - assert: "SUM(amount) = 1"
  - assert: "SUM(CASE WHEN paid THEN amount ELSE 0 END) = 30"
  - assert: "MAX(note) IS NULL"
  - assert: "amount > 0"
  - assert: "SUM(amount)"
  - assert: "SUM(missing) = 1"
"#;
    let rows = "amount,paid,note\n10,true,\n20,yes,\n30,no,\n";
    let out = run(yaml, rows, ReferenceSets::none());
    let checks = asserts(&out);
    assert_eq!(checks.len(), 6);

    assert!(checks[0].failed);
    assert_eq!(
        checks[0].message.as_deref(),
        Some("`SUM(amount) = 1` is false over 3 row(s)")
    );

    // Booleans spelled true/yes/no are read as booleans.
    assert!(!checks[1].failed, "{:?}", checks[1]);

    // An all-null column is still a column; the assertion about it holds.
    assert!(!checks[2].failed, "{:?}", checks[2]);

    // A per-row expression is refused with a pointer at custom_expr.
    assert!(checks[3].failed);
    let msg = checks[3].message.as_deref().unwrap();
    assert!(msg.contains("produced 3 values, not one"), "{msg}");
    assert!(msg.contains("custom_expr"), "{msg}");

    // Not a boolean.
    assert!(checks[4].failed);
    let msg = checks[4].message.as_deref().unwrap();
    assert!(msg.contains("not true/false"), "{msg}");

    // A column the source lacks.
    assert!(checks[5].failed);
    assert_eq!(
        checks[5].message.as_deref(),
        Some("check could not run: assert names column `missing`, which the source does not have")
    );
}

#[test]
fn assert_over_an_empty_dataset_sees_zero_rows() {
    let yaml = r#"
apiVersion: v1
dataset: empty
owner: fin@acme.com
columns:
  amount: { type: float }
dataset_checks:
  - assert: "COUNT(*) = 0"
  - assert: "SUM(amount) > 0"
  - assert: "AVG(amount) > 0"
"#;
    let out = run(yaml, "amount\n", ReferenceSets::none());
    let checks = asserts(&out);
    assert!(!checks[0].failed, "{:?}", checks[0]);
    // A sum of nothing is 0, and 0 > 0 is plainly false.
    assert!(checks[1].failed, "{:?}", checks[1]);
    let msg = checks[1].message.as_deref().unwrap();
    assert!(msg.contains("false over 0 row(s)"), "{msg}");
    // An average of nothing is null, and null never passes.
    assert!(checks[2].failed, "{:?}", checks[2]);
    let msg = checks[2].message.as_deref().unwrap();
    assert!(msg.contains("null"), "{msg}");
}

#[test]
fn collect_key_set_reports_a_missing_column() {
    let path = temp_csv("nokey", "a,b\n1,2\n");
    let source = resolve(path.to_str().unwrap());
    let mut bs = open(&source, &ReadOptions::default()).unwrap();
    let err = collect_key_set(bs.as_mut(), &["id".to_owned()]).unwrap_err();
    assert_eq!(err.to_string(), "key column `id` is not in the source");
}
