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

    /// Never open a network connection, whatever else is configured.
    ///
    /// Checking is local and always has been; the only thing this switches off
    /// is reporting a result to PlexusPact Cloud. It exists so that "this ran
    /// entirely on our machines" can be enforced rather than assumed —
    /// `PLEXUSPACT_NO_NETWORK` in the environment does the same for a whole
    /// image, and neither can be overridden from below.
    #[arg(long, global = true)]
    pub offline: bool,

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
    /// Report a saved JSON result to PlexusPact Cloud.
    Push(PushArgs),
    /// Compare two contracts and classify the changes.
    Diff(DiffArgs),
    /// Put a contract into force in PlexusPact Cloud.
    Register(RegisterArgs),
    /// Parse and lint a contract without running it.
    ValidateContract(ValidateArgs),
    /// Export a contract to another tool's format (Databricks DLT, dbt, ODCS).
    Export(ExportArgs),
    /// Convert an Open Data Contract Standard (ODCS) document to a contract.
    ///
    /// Every other command also accepts an ODCS document wherever it takes a
    /// contract file; use `import` to keep the converted draft and review it.
    Import(ImportArgs),
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
    #[command(flatten)]
    pub input: InputArgs,
}

/// How to read the data file. Shared by `init` and `check`.
///
/// A contract can carry the same instructions under `settings.input`; these
/// flags win over it for one run. `init` records what it was told in the
/// draft, so the next `check` needs no flags at all.
#[derive(Debug, Default, clap::Args)]
pub struct InputArgs {
    /// Force the input format instead of detecting from the extension
    /// (csv, tsv, parquet, ndjson, json, excel, xml, fixed_width).
    #[arg(long, value_name = "FORMAT", help_heading = "Input")]
    pub input_format: Option<String>,
    /// For JSON input: dotted path to the record array inside a wrapping object
    /// (e.g. `results`, `data.items`). Use when the API returns
    /// `{ "results": [...] }` instead of a bare array.
    #[arg(long, value_name = "PATH", help_heading = "Input")]
    pub json_path: Option<String>,
    /// CSV: the field separator, one character or `\t` (default: from the
    /// extension — `,` for .csv, tab for .tsv).
    #[arg(long, value_name = "CHAR", help_heading = "Input")]
    pub delimiter: Option<String>,
    /// The first row holds data, not column names (CSV, workbooks,
    /// fixed-width). Columns are then named by position.
    #[arg(long, help_heading = "Input")]
    pub no_header: bool,
    /// Workbooks: the sheet to read, by name or 1-based position
    /// (default: the first sheet).
    #[arg(long, value_name = "NAME|N", help_heading = "Input")]
    pub sheet: Option<String>,
    /// Rows to skip before the header (report titles, notes above the table).
    #[arg(long, value_name = "N", help_heading = "Input")]
    pub skip_rows: Option<u32>,
    /// XML: the element that is one record — its name (`row`) or a path
    /// (`orders/order`). Default: the first element under the root.
    #[arg(long, value_name = "NAME|PATH", help_heading = "Input")]
    pub xml_record: Option<String>,
    /// Fixed-width text: the column layout, 1-based character positions,
    /// e.g. `id=1-8,name=9-40,amount=12` (`start-end` or a width).
    #[arg(long, value_name = "LAYOUT", help_heading = "Input")]
    pub fixed_width: Option<String>,
}

impl InputArgs {
    /// The flags as core overrides.
    pub fn overrides(&self) -> plexuspact_core::InputOverrides {
        plexuspact_core::InputOverrides {
            format: self.input_format.clone(),
            json_path: self.json_path.clone(),
            delimiter: self.delimiter.clone(),
            has_header: if self.no_header { Some(false) } else { None },
            sheet: self.sheet.clone(),
            skip_rows: self.skip_rows,
            xml_record: self.xml_record.clone(),
            fixed_width: self.fixed_width.clone(),
        }
    }
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
    #[command(flatten)]
    pub input: InputArgs,
    /// Disable ANSI colors (also respects `NO_COLOR`).
    #[arg(long)]
    pub no_color: bool,
    /// Reference time for freshness checks (RFC 3339). Testing/reproducibility.
    #[arg(long, value_name = "RFC3339")]
    pub now: Option<String>,
    /// A file holding the dataset a `references` check points at, as
    /// `<dataset>=<path>`. The keys the check looks up are read from it;
    /// repeat the flag for every referenced dataset.
    #[arg(long, value_name = "DATASET=PATH")]
    pub reference: Vec<String>,
    /// Require the result to be reported to PlexusPact Cloud.
    ///
    /// Reporting already happens on its own whenever `PLEXUSPACT_API_KEY` is
    /// set. This flag makes a missing key an error instead of a silence, which
    /// is what you want in CI: a secret that was never wired up should be
    /// noticed on the first run, not discovered a month later.
    #[arg(long, conflicts_with = "no_push")]
    pub push: bool,
    /// Never report this result, even if a key is set.
    #[arg(long)]
    pub no_push: bool,
    /// API root to report to (default `https://api.plexuspact.com/api/v1`).
    #[arg(long, value_name = "URL", env = "PLEXUSPACT_API")]
    pub api: Option<String>,
}

/// `plexuspact push <result.json>`
///
/// For results that already exist: a file written earlier in the job, a nightly
/// batch, a retry of something the network ate the first time. `check` reports
/// on its own; this is the same hop for a document on disk.
#[derive(Debug, clap::Args)]
pub struct PushArgs {
    /// JSON result file to report (`-` for stdin).
    pub path: String,
    /// API root to report to (default `https://api.plexuspact.com/api/v1`).
    #[arg(long, value_name = "URL", env = "PLEXUSPACT_API")]
    pub api: Option<String>,
}

/// `plexuspact diff <old> <new>`, or `plexuspact diff <new> --against-registry`
///
/// Two files, or one file and the truth. Comparing two working-copy files
/// answers a question the author already knows the answer to; comparing against
/// the registry answers the one the pull request actually raises, which is what
/// is in force right now and who breaks if this lands.
#[derive(Debug, clap::Args)]
pub struct DiffArgs {
    /// The previous contract — or, with `--against-registry`, the only contract.
    pub old: PathBuf,
    /// The new contract. Omitted with `--against-registry`, which supplies it.
    pub new: Option<PathBuf>,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    pub json: bool,
    /// Compare against the version in force in PlexusPact Cloud, and report who
    /// breaks. Needs `PLEXUSPACT_API_KEY`; writes nothing.
    #[arg(long)]
    pub against_registry: bool,
    /// The registered dataset to compare against, when this version renames it.
    /// Without it a rename reads as a new dataset with nothing to break.
    #[arg(long, value_name = "NAME", requires = "against_registry")]
    pub dataset: Option<String>,
    /// Write a Markdown summary here, for a pull-request comment.
    #[arg(long, value_name = "FILE", requires = "against_registry")]
    pub markdown: Option<PathBuf>,
    /// API root to ask (default `https://api.plexuspact.com/api/v1`).
    #[arg(long, value_name = "URL", env = "PLEXUSPACT_API")]
    pub api: Option<String>,
}

/// `plexuspact register <contract.yaml>`
///
/// The other half of `diff --against-registry`. Preflight asks what a branch
/// would do and writes nothing; this is the call made after the merge, when the
/// change has actually been agreed.
///
/// `--source-url` is the point of it. An API key is one account, not one
/// person, so a contract registered from CI otherwise arrives from nowhere.
/// Passing the pull request that merged it turns "a machine did this" into
/// "these people agreed to this, here, and here is the discussion".
#[derive(Debug, clap::Args)]
pub struct RegisterArgs {
    /// Contract file to register.
    pub contract: PathBuf,
    /// Register under this dataset name instead of the contract's own.
    #[arg(long, value_name = "NAME")]
    pub dataset: Option<String>,
    /// A human label for this version (a tag, a release, a commit).
    #[arg(long, value_name = "LABEL")]
    pub version_label: Option<String>,
    /// Where this version was agreed — the pull request, merge request or work
    /// item that merged it. Recorded on the version and in the audit trail.
    #[arg(long, value_name = "URL")]
    pub source_url: Option<String>,
    /// Exit non-zero when the registry holds the version for approval instead
    /// of putting it in force. Off by default: a project that requires review
    /// is working as intended, and failing the merge job would punish it.
    #[arg(long)]
    pub fail_if_pending: bool,
    /// API root to register with (default `https://api.plexuspact.com/api/v1`).
    #[arg(long, value_name = "URL", env = "PLEXUSPACT_API")]
    pub api: Option<String>,
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
    /// Stable identity to stamp on an ODCS document (default: the dataset name).
    #[arg(long, value_name = "ID")]
    pub id: Option<String>,
    /// dbt only: emit the dataset as a table of this `sources:` entry, with
    /// native source freshness. Without it the dataset is a `models:` entry.
    #[arg(long, value_name = "NAME")]
    pub source: Option<String>,
}

/// Supported export targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ExportTarget {
    /// Databricks Delta Live Tables expectations.
    DatabricksDlt,
    /// dbt `schema.yml` with the contract's checks as tests.
    Dbt,
    /// Open Data Contract Standard v3 document (YAML).
    Odcs,
}

/// `plexuspact import <odcs.yaml>`
#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// ODCS v3 document (YAML or JSON) to convert.
    pub path: PathBuf,
    /// Write the converted contract to this file instead of stdout.
    #[arg(long, value_name = "FILE")]
    pub out: Option<PathBuf>,
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
