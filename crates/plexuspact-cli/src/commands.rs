//! Command handlers. Each returns the process exit code (ADR-004); only truly
//! unexpected failures return `Err`, which `main` maps to exit code 3.

use std::io::{IsTerminal, Write};
use std::path::Path;

use owo_colors::OwoColorize;
use plexuspact_contract::{
    databricks_dlt, diff, validate, Change, Contract, DltLang, Impact, LintLevel,
};
use plexuspact_core::{draft_contract, profile_path, run_check, CoreError, RunResult};
use plexuspact_report::{
    render_html, render_human, render_json, render_junit, render_openlineage, HumanOptions,
    OpenLineageOptions,
};

use crate::cli::{
    CheckArgs, Cli, Command, DiffArgs, ExportArgs, ExportLang, ExportTarget, Format, InitArgs,
    ValidateArgs,
};
use crate::exit::{CODE_FAILURES, CODE_INTERNAL, CODE_OK, CODE_USAGE};

type CmdResult = Result<u8, Box<dyn std::error::Error>>;

/// A loaded contract with its raw bytes (for the registry content hash), or
/// `None` when loading failed (a diagnostic was already printed).
type LoadedContract = Result<Option<(Contract, Vec<u8>)>, Box<dyn std::error::Error>>;

/// Routes a parsed CLI to its handler.
pub fn dispatch(cli: Cli) -> CmdResult {
    match cli.command {
        Command::Init(args) => cmd_init(args),
        Command::Check(args) => cmd_check(args),
        Command::Diff(args) => cmd_diff(args),
        Command::ValidateContract(args) => cmd_validate(args),
        Command::Export(args) => cmd_export(args),
    }
}

/// The compiled tool version, embedded into every `RunResult`.
fn tool_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

// ─────────────────────────────── validate ───────────────────────────────

fn cmd_validate(args: ValidateArgs) -> CmdResult {
    let Some((contract, _)) = load_contract(&args.path)? else {
        return Ok(CODE_USAGE);
    };
    let findings = validate(&contract);
    if findings.is_empty() {
        println!("{} contract is valid ({})", ok_glyph(), args.path.display());
        return Ok(CODE_OK);
    }
    let has_error = findings.iter().any(|f| f.level == LintLevel::Error);
    for f in &findings {
        print_lint(f);
    }
    if has_error {
        Ok(CODE_USAGE)
    } else {
        // Warnings only: usable contract.
        Ok(CODE_OK)
    }
}

fn print_lint(f: &plexuspact_contract::LintError) {
    let color = stderr_color();
    let tag = match (f.level, color) {
        (LintLevel::Error, true) => "✗ error".red().to_string(),
        (LintLevel::Error, false) => "✗ error".to_owned(),
        (LintLevel::Warning, true) => "⚠ warning".yellow().to_string(),
        (LintLevel::Warning, false) => "⚠ warning".to_owned(),
    };
    eprintln!("{tag} {}: {}", f.path, f.message);
    if let Some(help) = &f.help {
        eprintln!("    help: {help}");
    }
}

// ───────────────────────────────── check ────────────────────────────────

fn cmd_check(args: CheckArgs) -> CmdResult {
    let Some((contract, bytes)) = load_contract(&args.contract)? else {
        return Ok(CODE_USAGE);
    };
    // Error-severity lints block the run.
    let findings = validate(&contract);
    if findings.iter().any(|f| f.level == LintLevel::Error) {
        for f in &findings {
            print_lint(f);
        }
        return Ok(CODE_USAGE);
    }

    let now = match &args.now {
        None => None,
        Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Some(dt.to_utc()),
            Err(_) => {
                eprintln!(
                    "{} invalid --now `{s}`: expected an RFC 3339 timestamp, e.g. 2026-07-09T00:00:00Z",
                    err_glyph()
                );
                return Ok(CODE_USAGE);
            }
        },
    };

    let result = match run_check(
        contract,
        bytes,
        Some(args.contract.display().to_string()),
        &args.path,
        args.input_format.as_deref(),
        args.json_path.as_deref(),
        args.sample_failures,
        args.redact_samples,
        now,
        tool_version(),
    ) {
        Ok(r) => r,
        Err(e) => return Ok(handle_core_error(e)),
    };

    // Render primary output.
    match args.format {
        Format::Human => {
            let opts = HumanOptions {
                color: use_color(args.no_color),
                max_samples: args.sample_failures,
            };
            print!("{}", render_human(&result, &opts));
        }
        Format::Json => println!("{}", render_json(&result, true)?),
        Format::Junit => println!("{}", render_junit(&result)?),
    }

    // Optional HTML report.
    if let Some(path) = &args.report {
        let html = render_html(&result)?;
        std::fs::write(path, html)?;
        eprintln!("{} wrote HTML report to {}", ok_glyph(), path.display());
    }

    // Optional OpenLineage event (data-quality facets for downstream tools).
    if let Some(path) = &args.openlineage {
        let mut opts = OpenLineageOptions::default();
        if let Some(ns) = &args.openlineage_namespace {
            opts.dataset_namespace = ns.clone();
        }
        let event = render_openlineage(&result, &opts)?;
        std::fs::write(path, event)?;
        eprintln!(
            "{} wrote OpenLineage event to {}",
            ok_glyph(),
            path.display()
        );
    }

    Ok(exit_for(&result, args.strict))
}

/// Exit code from a run result (ADR-004 / FR-7).
fn exit_for(result: &RunResult, strict: bool) -> u8 {
    if result.is_failure(strict) {
        CODE_FAILURES
    } else {
        CODE_OK
    }
}

/// Maps a core error to a friendly message + exit code.
///
/// I/O problems (missing file, unreadable input) are user errors (exit 2);
/// an internal data error is exit 3.
fn handle_core_error(err: CoreError) -> u8 {
    if err.is_user_error() {
        eprintln!("{} {err}", err_glyph());
        CODE_USAGE
    } else {
        eprintln!("{} internal error: {err}", err_glyph());
        CODE_INTERNAL
    }
}

// ────────────────────────────────── diff ────────────────────────────────

fn cmd_diff(args: DiffArgs) -> CmdResult {
    let Some((old, _)) = load_contract(&args.old)? else {
        return Ok(CODE_USAGE);
    };
    let Some((new, _)) = load_contract(&args.new)? else {
        return Ok(CODE_USAGE);
    };
    let changes = diff(&old, &new);
    let breaking = changes.iter().any(|c| c.impact == Impact::Breaking);

    if args.json {
        println!("{}", diff_json(&changes)?);
    } else {
        print_diff_human(&changes);
    }

    Ok(if breaking { CODE_FAILURES } else { CODE_OK })
}

fn print_diff_human(changes: &[Change]) {
    if changes.is_empty() {
        println!("{} no changes", ok_glyph());
        return;
    }
    let color = use_color(false);
    for c in changes {
        let tag = match (c.impact, color) {
            (Impact::Breaking, true) => "BREAKING".red().to_string(),
            (Impact::Breaking, false) => "BREAKING".to_owned(),
            (Impact::NonBreaking, true) => "non-breaking".yellow().to_string(),
            (Impact::NonBreaking, false) => "non-breaking".to_owned(),
            (Impact::Cosmetic, true) => "cosmetic".dimmed().to_string(),
            (Impact::Cosmetic, false) => "cosmetic".to_owned(),
        };
        println!("  {tag}  {}: {}", c.path, c.description);
    }
    let breaking = changes
        .iter()
        .filter(|c| c.impact == Impact::Breaking)
        .count();
    if breaking > 0 {
        println!("\n{} {breaking} breaking change(s)", err_glyph());
    }
}

fn diff_json(changes: &[Change]) -> Result<String, serde_json::Error> {
    let arr: Vec<_> = changes
        .iter()
        .map(|c| {
            serde_json::json!({
                "impact": match c.impact {
                    Impact::Breaking => "breaking",
                    Impact::NonBreaking => "non_breaking",
                    Impact::Cosmetic => "cosmetic",
                },
                "path": c.path,
                "description": c.description,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "changes": arr }))
}

// ───────────────────────────────── export ───────────────────────────────

fn cmd_export(args: ExportArgs) -> CmdResult {
    let Some((contract, _)) = load_contract(&args.contract)? else {
        return Ok(CODE_USAGE);
    };
    // Surface lint errors before exporting a contract that wouldn't run.
    let findings = validate(&contract);
    if findings.iter().any(|f| f.level == LintLevel::Error) {
        for f in &findings {
            print_lint(f);
        }
        return Ok(CODE_USAGE);
    }

    let rendered = match args.target {
        ExportTarget::DatabricksDlt => {
            let lang = match args.lang {
                ExportLang::Sql => DltLang::Sql,
                ExportLang::Python => DltLang::Python,
            };
            databricks_dlt(&contract, lang)
        }
    };

    match &args.out {
        Some(path) => {
            std::fs::write(path, &rendered)?;
            eprintln!("{} wrote export to {}", ok_glyph(), path.display());
        }
        None => print!("{rendered}"),
    }
    Ok(CODE_OK)
}

// ────────────────────────────────── init ────────────────────────────────

fn cmd_init(args: InitArgs) -> CmdResult {
    let profile = match profile_path(
        &args.path,
        args.input_format.as_deref(),
        args.json_path.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => return Ok(handle_core_error(e)),
    };
    let dataset = args
        .dataset
        .unwrap_or_else(|| derive_dataset_name(&args.path));
    let yaml = draft_contract(&profile, &dataset);

    match &args.out {
        Some(path) => {
            std::fs::write(path, &yaml)?;
            eprintln!(
                "{} profiled {} rows, wrote draft contract to {}",
                ok_glyph(),
                profile.rows,
                path.display()
            );
        }
        None => {
            std::io::stdout().write_all(yaml.as_bytes())?;
        }
    }
    Ok(CODE_OK)
}

/// Derives a dataset name from a data file path (stem, sans compression/format).
fn derive_dataset_name(path: &str) -> String {
    if path == "-" {
        return "dataset".to_owned();
    }
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy())
        .map(|n| n.split('.').next().unwrap_or("dataset").to_owned())
        .unwrap_or_else(|| "dataset".to_owned())
}

// ───────────────────────────────── shared ───────────────────────────────

/// Loads and parses a contract file. Returns `None` (after printing a
/// diagnostic) when parsing fails, so the caller can exit with code 2.
fn load_contract(path: &Path) -> LoadedContract {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!(
                "{} cannot read contract {}: {e}",
                err_glyph(),
                path.display()
            );
            return Ok(None);
        }
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    match plexuspact_contract::parse_str(&text, &path.display().to_string()) {
        Ok(contract) => Ok(Some((contract, bytes))),
        Err(err) => {
            // Render the span-labeled miette diagnostic to stderr.
            eprintln!("{:?}", miette::Report::new(err));
            Ok(None)
        }
    }
}

/// Whether to emit ANSI colors: not when `--no-color`, `NO_COLOR` is set, or
/// stdout is not a terminal.
fn use_color(no_color_flag: bool) -> bool {
    if no_color_flag || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Whether status glyphs (printed to stderr) may use color.
fn stderr_color() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stderr().is_terminal()
}

/// A green `✓`, colored only when the terminal supports it.
fn ok_glyph() -> String {
    if stderr_color() {
        "✓".green().to_string()
    } else {
        "✓".to_owned()
    }
}

/// A red `✗`, colored only when the terminal supports it.
fn err_glyph() -> String {
    if stderr_color() {
        "✗".red().to_string()
    } else {
        "✗".to_owned()
    }
}
