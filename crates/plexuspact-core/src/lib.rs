//! Orchestration layer: loads contracts, plans and runs checks, assembles `RunResult`.
//!
//! `RunResult` is the single source of truth (doc 03 §1): every renderer, the GitHub
//! Action, and the future SaaS `push` consume this struct. Renderers must never compute.
//!
//! Phase 2 constraint (doc 03 §12): this crate must remain usable as a library —
//! no printing, no `std::process::exit`.

mod draft;
mod error;
mod run;

pub mod result;

pub use draft::{draft_contract, draft_contract_with_input};
pub use error::CoreError;
pub use plexuspact_contract::InputSettings;
pub use plexuspact_engine::{profile, ColumnProfile, DatasetProfile, KeySet, ReferenceSets};
pub use result::{
    CheckMetrics, CheckResult, CheckSeverity, CheckStatus, ConsumerRef, ContractRef, FailureSample,
    ObservedColumnInfo, ObservedSchema, RunResult, RunStatus, RunSummary, SourceInfo,
    RESULT_SCHEMA_VERSION,
};
pub use run::{
    key_set_from_path, profile_path, profile_path_with, read_options_for, run, run_check,
    run_check_full, run_check_with, run_with_artifacts, CheckOptions, InputOverrides, RunArtifacts,
    RunRequest,
};
