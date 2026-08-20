//! The streaming execution loop: read batches, fold check state, finalize.

use std::collections::{BTreeSet, HashMap};

use plexuspact_contract::Contract;
use plexuspact_io::BatchSource;

use crate::checks::BatchView;
use crate::column::{self, ColumnData};
use crate::{CheckOutcome, EngineError, EngineOutput, RunOptions};

/// Executes a contract's checks against a streaming source.
///
/// The clock for freshness comes from `opts.now`; the engine never reads the
/// system clock. Memory stays bounded to one batch plus stateful-check state.
pub fn execute(
    source: &mut dyn BatchSource,
    contract: &Contract,
    opts: &RunOptions,
) -> Result<EngineOutput, EngineError> {
    // Present source columns (available before the first batch).
    let present: BTreeSet<String> = source
        .schema()
        .iter_names()
        .map(|n| n.to_string())
        .collect();
    let source_columns = present.len() as u64;

    let now_micros = opts.now.timestamp_micros();
    let mut checks = crate::plan::build(contract, &present, now_micros);

    // Which declared columns to extract each batch (present ∩ declared).
    let to_extract: Vec<(String, plexuspact_contract::ColType)> = contract
        .columns
        .iter()
        .filter(|(name, _)| present.contains(name.as_str()))
        .map(|(name, def)| (name.clone(), def.r#type))
        .collect();

    let typing = source.typing();
    let mut base_row: u64 = 1; // 1-based data row numbers (header excluded)
    let mut rows_total: u64 = 0;

    while let Some(df) = source.next_batch()? {
        let batch_rows = df.height() as u64;
        if batch_rows == 0 {
            continue;
        }

        let mut columns: HashMap<String, ColumnData> = HashMap::with_capacity(to_extract.len());
        for (name, coltype) in &to_extract {
            let data = column::extract(&df, name, *coltype, typing)?;
            columns.insert(name.clone(), data);
        }

        let view = BatchView {
            columns: &columns,
            df: &df,
            base_row,
            sample_budget: opts.sample_failures,
        };
        for check in &mut checks {
            check.eval_batch(&view)?;
        }

        base_row += batch_rows;
        rows_total += batch_rows;
    }

    let outcomes: Vec<CheckOutcome> = checks.into_iter().map(|c| c.finalize(rows_total)).collect();

    Ok(EngineOutput {
        rows_total,
        source_columns,
        checks: outcomes,
    })
}
