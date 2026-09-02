//! Check execution engine: compiles a `Contract` into a plan and executes it
//! over streamed batches (doc 03 §5).
//!
//! Layering (ADR-002): depends only on `plexuspact-contract` and
//! `plexuspact-io`. It knows nothing about the CLI, reports, or the wire format
//! — it returns plain [`EngineOutput`] data that `plexuspact-core` maps into a
//! `RunResult`.
//!
//! ## Strategies
//!
//! * **Row-level value checks** (min, max, regex, enum, length, format,
//!   not_empty_string, type, required) evaluate per batch and fold counters +
//!   bounded failure samples.
//! * **Stateful checks** (unique, unique_ratio_min) keep an `ahash` set of value
//!   hashes across batches.
//! * **Dataset checks** (row_count, freshness, null_ratio_max, custom_expr)
//!   accumulate aggregates and finalize once at end of stream.
//!
//! The clock for `freshness` is injected (never read inside the engine) so runs
//! are deterministic and testable.

mod column;
mod error;
pub mod formats;
mod profile;

mod checks;
mod exec;
mod hll;
mod plan;

pub use profile::{profile, ColumnProfile, DatasetProfile};

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use plexuspact_contract::Severity;

pub use error::EngineError;
pub use exec::execute;

/// Options controlling a run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Maximum failure samples captured per check (PRD default 5).
    pub sample_failures: usize,
    /// Clock used for `freshness` — injected for determinism (never `Utc::now()`
    /// inside the engine).
    pub now: DateTime<Utc>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions {
            sample_failures: 5,
            now: DateTime::<Utc>::UNIX_EPOCH,
        }
    }
}

/// Everything the engine observed for one run — mapped into a `RunResult` by
/// `plexuspact-core`.
#[derive(Debug, Clone)]
pub struct EngineOutput {
    /// Total rows read from the source.
    pub rows_total: u64,
    /// Number of columns present in the source.
    pub source_columns: u64,
    /// The source's own schema, in source order — what was *there*, as opposed
    /// to what the contract expected to be there. Recorded on every run so a
    /// later run can be compared against it: a column that quietly appeared,
    /// vanished, moved, or changed type is a change nobody declared, and it is
    /// only visible if somebody wrote down what the shape was last time.
    pub observed_columns: Vec<ObservedColumn>,
    /// Whether the dtypes above are the source's own. False for CSV/NDJSON/JSON
    /// read stringly, where every column arrives as text and a recorded dtype
    /// would be the reader's convention rather than the source's declaration.
    pub observed_typed: bool,
    /// Per-check outcomes, in a stable order (schema checks, then column checks
    /// in contract order, then dataset checks).
    pub checks: Vec<CheckOutcome>,
}

/// One column as the source presented it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedColumn {
    /// Column name, exactly as the source spells it.
    pub name: String,
    /// The source's dtype, when the source carries types; `None` when it does
    /// not. `None` means "the source does not say", never "unknown type" —
    /// the difference matters when deciding whether a type changed.
    pub dtype: Option<String>,
}

/// Outcome of a single check.
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    /// Stable identifier, e.g. `email.format.email`, `age.min`, `dataset.row_count_min`.
    pub id: String,
    /// Column the check applies to; `None` for dataset-level checks.
    pub column: Option<String>,
    /// Check kind, e.g. `min`, `format`, `unique`, `type`, `row_count_min`.
    pub kind: String,
    /// Declared parameters, as JSON (e.g. `{"format":"email"}`).
    pub params: serde_json::Value,
    /// Declared severity.
    pub severity: Severity,
    /// Whether the check failed.
    pub failed: bool,
    /// Rows the check evaluated (`None` for pure dataset aggregates).
    pub rows_evaluated: Option<u64>,
    /// Rows that violated the check (`None` for pure dataset aggregates).
    pub rows_failed: Option<u64>,
    /// Observed statistics (captured even when passing — drift source).
    pub observed: BTreeMap<String, serde_json::Value>,
    /// Up to `sample_failures` `(absolute_row, rendered_value)` pairs.
    pub samples: Vec<(u64, String)>,
    /// Human-readable one-line detail for failures (e.g. `min observed: 11`).
    pub message: Option<String>,
}
