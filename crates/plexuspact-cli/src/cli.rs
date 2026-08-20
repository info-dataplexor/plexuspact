//! Command-line surface (clap derive). Mirrors PRD §7.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

/// PlexusPact — shift-left data contract validation.
///
/// Validate any dataset against a versioned `contract.yaml`, at the source, in
/// seconds. A single binary with zero runtime dependencies.
#[derive(Debug, Parser)]
#[command(name = "plexuspact", version, about, long_about = None)]
pub struct Cli {
    /// Increase logging verbosity (`-v` info, `-vv` debug). Logs go to stderr.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    #[command(subcommand)]
    pub command: Command,
}

/// Top-level subcommands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Profile a dataset and draft a starter `contract.yaml`.
    Init(InitArgs),
    /// Validate a dataset against a contract.
    Check(CheckArgs),
    /// Compare two contracts and classify the changes.
    Diff(DiffArgs),
    /// Parse and lint a contract without running it.
    ValidateContract(ValidateArgs),
    /// Export a contract to another tool's format (e.g. Databricks DLT).
    Export(ExportArgs),
}

/// `plexuspact init <path>`
#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Data file to profile (`-` for stdin).
    pub path: String,
    /// Where to write the drafted contract (default: stdout).
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
    /// Dataset name to record in the contract (default: derived from the path).
    #[arg(long)]
    pub dataset: Option<String>,
    /// Force the input format instead of detecting from the extension.
    #[arg(long, value_name = "FORMAT")]
    pub input_format: Option<String>,
    /// For JSON input: dotted path to the record array inside a wrapping object
    /// (e.g. `results`, `data.items`). Use when the API returns
    /// `{ "results": [...] }` instead of a bare array.
    #[arg(long, value_name = "PATH")]
    pub json_path: Option<String>,
}

/// `plexuspact check <path> --contract <file>`
#[derive(Debug, clap::Args)]
pub struct CheckArgs {
    /// Data file to validate (`-` for stdin).
    pub path: String,
    /// Contract file to validate against.
    #[arg(short, long, value_name = "FILE")]
    pub contract: PathBuf,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    /// Also write a self-contained HTML report to this path.
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,
    /// Also emit an OpenLineage RunEvent (data-quality facets) to this path,
    /// for Databricks / dbt / Airflow / Marquez ingestion.
    #[arg(long, value_name = "FILE")]
    pub openlineage: Option<PathBuf>,
    /// Namespace for the input dataset in the OpenLineage event (the storage
    /// system it lives in, e.g. `s3://bucket`, `snowflake://acct`). Default `file`.
    #[arg(long, value_name = "NS", requires = "openlineage")]
    pub openlineage_namespace: Option<String>,
    /// Treat warn-severity failures as failures (non-zero exit).
    #[arg(long)]
    pub strict: bool,
    /// Maximum failing-row samples captured per check.
    #[arg(long, default_value_t = 5, value_name = "N")]
    pub sample_failures: usize,
    /// Mask sample values in output (keep row numbers).
    #[arg(long)]
    pub redact_samples: bool,
    /// Force the input format instead of detecting from the extension.
    #[arg(long, value_name = "FORMAT")]
    pub input_format: Option<String>,
    /// For JSON input: dotted path to the record array inside a wrapping object
    /// (e.g. `results`, `data.items`). Use when the API returns
    /// `{ "results": [...] }` instead of a bare array.
    #[arg(long, value_name = "PATH")]
    pub json_path: Option<String>,
    /// Disable ANSI colors (also respects `NO_COLOR`).
    #[arg(long)]
    pub no_color: bool,
    /// Reference time for freshness checks (RFC 3339). Testing/reproducibility.
    #[arg(long, value_name = "RFC3339")]
    pub now: Option<String>,
}

/// `plexuspact diff <old> <new>`
#[derive(Debug, clap::Args)]
pub struct DiffArgs {
    /// The previous contract.
    pub old: PathBuf,
    /// The new contract.
    pub new: PathBuf,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    pub json: bool,
}

/// `plexuspact validate-contract <file>`
#[derive(Debug, clap::Args)]
pub struct ValidateArgs {
    /// Contract file to parse and lint.
    pub path: PathBuf,
}

/// `plexuspact export <contract> --target <TARGET>`
#[derive(Debug, clap::Args)]
pub struct ExportArgs {
    /// Contract file to export.
    pub contract: PathBuf,
    /// Target tool/format to generate.
    #[arg(long, value_enum)]
    pub target: ExportTarget,
    /// Output language, where the target supports more than one.
    #[arg(long, value_enum, default_value_t = ExportLang::Sql)]
    pub lang: ExportLang,
    /// Write to this file instead of stdout.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
}

/// Supported export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportTarget {
    /// Databricks Delta Live Tables expectations.
    DatabricksDlt,
}

/// Output language for exports that support several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportLang {
    /// SQL (`CONSTRAINT … EXPECT (…)`).
    Sql,
    /// Python (`@dlt.expect_all*`).
    Python,
}

/// `check` output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable terminal output (default).
    Human,
    /// The versioned JSON result document.
    Json,
    /// JUnit XML for CI systems.
    Junit,
}
