//! # plexuspact-contract
//!
//! Data contract model: parsing, semantic validation, diffing, and JSON
//! Schema generation for `contract.yaml`.
//!
//! ## Layering rule (ADR-002)
//!
//! This crate depends on **nothing internal** and must never depend on
//! Polars. Checks here are *declarative descriptions*; only
//! `plexuspact-engine` knows how to execute them. Keep `Cargo.toml` free of
//! any DataFrame dependency — that boundary is what makes Python bindings and
//! the Phase 2 agent possible without a rewrite.
//!
//! ## Modules
//!
//! * [`model`] — the typed contract ([`Contract`], [`ColumnCheck`], …) with
//!   dual-form serde (`unique` bare strings and `{ min: 18 }` maps).
//! * [`parse`] — [`parse_str`]/[`parse_file`] with miette span diagnostics.
//! * [`validate`](mod@validate) — semantic lint after parse; returns **all** findings.
//! * [`diff`](mod@diff) — semantic diff classifying changes as
//!   breaking / non-breaking / cosmetic.
//! * [`schema`] — JSON Schema generation for editor autocomplete.

pub mod dbt;
pub mod diff;
pub mod export;
pub mod model;
pub mod odcs;
pub mod parse;
pub mod schema;
mod suggest;
pub mod validate;

pub use dbt::{dbt_schema, DbtTarget};
pub use diff::{diff, Change, Impact};
pub use export::{databricks_dlt, DltLang};
pub use model::{
    is_iso_date, ApiVersion, ColType, ColumnCheck, ColumnDef, Consumer, ConsumerKind, Contract,
    DataClass, DatasetCheck, EnumValue, FixedWidthField, InputFormat, InputSettings, KnownFormat,
    LengthRange, LengthSpec, Migration, Number, PiiKind, Settings, Severity, Stability,
    TIER_LEAST_CRITICAL, TIER_MOST_CRITICAL,
};
pub use parse::{parse_file, parse_str, InvalidContract, ParseError};
pub use schema::json_schema;
pub use validate::{validate, LintError, LintLevel};
