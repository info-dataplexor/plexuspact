//! Engine throughput benchmark: rows/second through `execute` on the stringly
//! (CSV-equivalent) check path, over a representative contract.
//!
//! Run with `cargo bench -p plexuspact-engine`. Numbers quoted in the README
//! come from this bench; regenerate them on the hardware you cite.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use plexuspact_io::{BatchSource, InputTyping, IoError};
use polars::prelude::{Column, DataFrame, IntoLazy, Schema};

const BATCH_ROWS: usize = 65_536;

/// In-memory stringly source: pre-built String-column batches, cloned per
/// iteration so the bench measures the engine, not the parser.
struct MemSource {
    schema: Schema,
    frames: Vec<DataFrame>,
    idx: usize,
}

impl MemSource {
    fn new(frames: Vec<DataFrame>) -> Self {
        let schema = frames[0].schema().as_ref().clone();
        MemSource {
            schema,
            frames,
            idx: 0,
        }
    }
}

impl BatchSource for MemSource {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn typing(&self) -> InputTyping {
        InputTyping::Stringly
    }

    fn next_batch(&mut self) -> Result<Option<DataFrame>, IoError> {
        let out = self.frames.get(self.idx).cloned();
        self.idx += 1;
        Ok(out)
    }
}

fn contract() -> plexuspact_contract::Contract {
    plexuspact_contract::parse_str(
        r#"
apiVersion: v1
dataset: bench
columns:
  user_id: { type: int,    required: true, checks: [unique] }
  email:   { type: string, required: true, checks: [{ format: email }] }
  amount:  { type: float,  checks: [{ min: 0 }, { max: 1000000 }] }
  plan:    { type: string, checks: [{ enum: [free, pro, enterprise] }] }
  country: { type: string, checks: [{ regex: "^[A-Z]{2}$" }] }
dataset_checks:
  - row_count_min: 1
  - null_ratio_max: { column: amount, ratio: 0.5 }
"#,
        "bench.yaml",
    )
    .expect("bench contract parses")
}

/// Deterministic synthetic batches: all-String columns, ~1.5% failure mix so
/// sample capture and failure counting are exercised, not skipped.
fn build_frames(total_rows: usize) -> Vec<DataFrame> {
    let plans = ["free", "pro", "enterprise"];
    let countries = ["US", "DE", "IN", "BR", "JP"];
    let mut frames = Vec::new();
    let mut row = 0usize;
    while row < total_rows {
        let n = BATCH_ROWS.min(total_rows - row);
        let mut user_id = Vec::with_capacity(n);
        let mut email = Vec::with_capacity(n);
        let mut amount = Vec::with_capacity(n);
        let mut plan = Vec::with_capacity(n);
        let mut country = Vec::with_capacity(n);
        for i in row..row + n {
            // ~1% duplicate ids, ~0.5% malformed emails.
            let id = if i % 100 == 7 { i.saturating_sub(1) } else { i };
            user_id.push(id.to_string());
            email.push(if i % 200 == 11 {
                "not-an-email".to_owned()
            } else {
                format!("user{i}@example.com")
            });
            amount.push(format!("{}.{:02}", i % 9000, i % 100));
            plan.push(plans[i % plans.len()].to_owned());
            country.push(countries[i % countries.len()].to_owned());
        }
        let df = DataFrame::new(vec![
            Column::new("user_id".into(), user_id),
            Column::new("email".into(), email),
            Column::new("amount".into(), amount),
            Column::new("plan".into(), plan),
            Column::new("country".into(), country),
        ])
        .expect("bench frame builds");
        // Force materialization once so per-iteration clones are cheap.
        let df = df.lazy().collect().expect("bench frame collects");
        frames.push(df);
        row += n;
    }
    frames
}

fn bench_execute(c: &mut Criterion) {
    let contract = contract();
    let opts = plexuspact_engine::RunOptions::default();

    let mut group = c.benchmark_group("engine_execute");
    for &total_rows in &[100_000usize, 1_000_000usize] {
        let frames = build_frames(total_rows);
        group.throughput(Throughput::Elements(total_rows as u64));
        group.sample_size(10);
        group.bench_with_input(
            BenchmarkId::new("stringly_5col_9checks", total_rows),
            &frames,
            |b, frames| {
                b.iter(|| {
                    let mut source = MemSource::new(frames.clone());
                    plexuspact_engine::execute(&mut source, &contract, &opts)
                        .expect("bench run succeeds")
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_execute);
criterion_main!(benches);
