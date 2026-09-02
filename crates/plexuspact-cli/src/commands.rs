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
    PushArgs, RegisterArgs, ValidateArgs,
};
use crate::exit::{CODE_FAILURES, CODE_INTERNAL, CODE_OK, CODE_USAGE};
use crate::preflight;
use crate::push::{self, PushError, Reporting};
use crate::register;

type CmdResult = Result<u8, Box<dyn std::error::Error>>;

/// A loaded contract with its raw bytes (for the registry content hash), or
/// `None` when loading failed (a diagnostic was already printed).
type LoadedContract = Result<Option<(Contract, Vec<u8>)>, Box<dyn std::error::Error>>;

/// Routes a parsed CLI to its handler.
pub fn dispatch(cli: Cli) -> CmdResult {
    let offline = cli.offline;
    match cli.command {
        Command::Init(args) => cmd_init(args),
        Command::Check(args) => cmd_check(args, offline),
        Command::Push(args) => cmd_push(args, offline),
        Command::Diff(args) => cmd_diff(args, offline),
        Command::Register(args) => cmd_register(args, offline),
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

fn cmd_check(args: CheckArgs, offline: bool) -> CmdResult {
    // Resolve the reporting destination before reading a byte of data. A
    // mistyped `--api` should be an immediate usage error, not the last thing
    // you learn after a ten-minute scan.
    let reporting = if args.no_push {
        Reporting::Suppressed("--no-push was given")
    } else {
        match push::resolve(args.api.as_deref(), offline) {
            Ok(r) => r,
            Err(e) => {
                print_push_error(&e);
                return Ok(CODE_USAGE);
            }
        }
    };
    // `--push` says a missing run is as serious as a failing check, so anything
    // that would leave the run unreported has to be settled now rather than
    // discovered in the last line of the job log.
    let destination = match &reporting {
        Reporting::To(dest) => Some(dest),
        Reporting::Off if args.push => {
            eprintln!(
                "{} --push was given but {} is not set",
                err_glyph(),
                push::TOKEN_VAR
            );
            return Ok(CODE_USAGE);
        }
        Reporting::Suppressed(why) if args.push => {
            eprintln!("{} --push was given but {why}", err_glyph());
            return Ok(CODE_USAGE);
        }
        _ => None,
    };

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

    // Report the verdict onward, if there is somewhere to report it to.
    //
    // The exit code belongs to the data. A failed report can turn a clean run
    // into an internal error only when `--push` said reporting was required —
    // and never overwrites a `1`, because "the data broke the contract" is the
    // more important of the two things that went wrong.
    let verdict = exit_for(&result, args.strict);
    if let Some(dest) = destination {
        let body = render_json(&result, false)?;
        match push::send(dest, &body) {
            Ok(recorded) => {
                eprintln!("{} reported run to {}", ok_glyph(), recorded.url);
            }
            Err(e) => {
                print_push_error(&e);
                if args.push && verdict == CODE_OK {
                    return Ok(CODE_INTERNAL);
                }
            }
        }
    }

    Ok(verdict)
}

// ────────────────────────────────── push ────────────────────────────────

fn cmd_push(args: PushArgs, offline: bool) -> CmdResult {
    // Unlike `check`, reporting is the entire job here: nothing to report to is
    // a usage error rather than a quiet no-op.
    let dest = match push::resolve(args.api.as_deref(), offline) {
        Ok(Reporting::To(d)) => d,
        Ok(Reporting::Off) => {
            eprintln!(
                "{} {} is not set, so there is nowhere to report to",
                err_glyph(),
                push::TOKEN_VAR
            );
            eprintln!("    help: create a project API key in PlexusPact Cloud and export it");
            return Ok(CODE_USAGE);
        }
        Ok(Reporting::Suppressed(why)) => {
            eprintln!("{} nothing was reported: {why}", err_glyph());
            return Ok(CODE_USAGE);
        }
        Err(e) => {
            print_push_error(&e);
            return Ok(CODE_USAGE);
        }
    };

    let body = match read_input(&args.path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{} cannot read {}: {e}", err_glyph(), args.path);
            return Ok(CODE_USAGE);
        }
    };
    // Parsed here only to fail early and locally on something that was never a
    // result document — a truncated file, a shell that captured the wrong
    // stream. The bytes sent are the bytes read.
    if serde_json::from_str::<serde_json::Value>(&body).is_err() {
        eprintln!("{} {} is not JSON", err_glyph(), args.path);
        return Ok(CODE_USAGE);
    }

    match push::send(&dest, &body) {
        Ok(recorded) => {
            println!("{}", recorded.url);
            eprintln!("{} reported run {}", ok_glyph(), recorded.id);
            Ok(CODE_OK)
        }
        Err(e) => {
            print_push_error(&e);
            Ok(CODE_INTERNAL)
        }
    }
}

/// Reads a file, or stdin when the path is `-`.
fn read_input(path: &str) -> std::io::Result<String> {
    if path == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path)
    }
}

/// Prints a reporting failure to stderr, where it cannot corrupt `--format
/// json` on stdout.
fn print_push_error(e: &PushError) {
    eprintln!("{} {e}", warn_glyph());
    if let Some(hint) = &e.hint {
        eprintln!("    help: {hint}");
    }
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

fn cmd_diff(args: DiffArgs, offline: bool) -> CmdResult {
    if args.against_registry {
        return cmd_diff_registry(args, offline);
    }
    let Some(new_path) = args.new.clone() else {
        eprintln!(
            "{} diff needs two contracts, or one and --against-registry",
            err_glyph()
        );
        return Ok(CODE_USAGE);
    };
    let Some((old, _)) = load_contract(&args.old)? else {
        return Ok(CODE_USAGE);
    };
    let Some((new, _)) = load_contract(&new_path)? else {
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

/// `diff <contract> --against-registry`
///
/// The exit code is the whole point: `1` when something tightens, so a pipeline
/// stops on the change that would have broken a consumer rather than on the
/// delivery that eventually did. A contract that cannot be read is a usage
/// error (`2`) — nothing was compared, and calling that "no breaking changes"
/// would be the worst possible lie to tell a green build.
fn cmd_diff_registry(args: DiffArgs, offline: bool) -> CmdResult {
    if args.new.is_some() {
        eprintln!(
            "{} --against-registry compares one contract against what is in force; \
             pass a single file",
            err_glyph()
        );
        return Ok(CODE_USAGE);
    }
    // Read the bytes, not the parsed model: the server has to see exactly what
    // is on the branch, including whatever makes it invalid.
    let content = match std::fs::read_to_string(&args.old) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{} could not read {}: {e}", err_glyph(), args.old.display());
            return Ok(CODE_USAGE);
        }
    };

    let dest = match push::resolve_endpoint(args.api.as_deref(), offline, "contracts/preflight") {
        Ok(Reporting::To(dest)) => dest,
        Ok(Reporting::Off) => {
            eprintln!(
                "{} --against-registry needs a project API key in {}",
                err_glyph(),
                push::TOKEN_VAR
            );
            return Ok(CODE_USAGE);
        }
        Ok(Reporting::Suppressed(why)) => {
            eprintln!("{} cannot ask the registry: {why}", err_glyph());
            return Ok(CODE_USAGE);
        }
        Err(e) => {
            print_push_error(&e);
            return Ok(CODE_USAGE);
        }
    };

    let result = match preflight::fetch(&dest, &content, args.dataset.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            print_push_error(&e);
            // Unreachable registry is not a passing build: the question was
            // asked and never answered, and a job that treats that as "no
            // breaking changes" is a job that stops catching them the first
            // time the network is slow.
            return Ok(CODE_INTERNAL);
        }
    };

    if let Some(path) = &args.markdown {
        std::fs::write(path, preflight::to_markdown(&result))?;
    }
    if args.json {
        println!("{}", preflight_json(&result)?);
    } else {
        print_preflight_human(&result);
    }

    Ok(if result.is_invalid() {
        CODE_USAGE
    } else if result.is_breaking() {
        CODE_FAILURES
    } else {
        CODE_OK
    })
}

// ─────────────────────────────── register ───────────────────────────────

/// `register <contract> [--source-url <pr>]`
///
/// Run on the merge, not on the branch. The contract has been agreed by the
/// time this runs, so unlike preflight it writes — and it refuses to send a
/// contract it could not parse, because the registry is where suppliers are
/// judged from and a broken version in force fails everybody at once.
fn cmd_register(args: RegisterArgs, offline: bool) -> CmdResult {
    let Some((contract, raw)) = load_contract(&args.contract)? else {
        return Ok(CODE_USAGE);
    };
    // Send the bytes on disk, not a re-serialisation of the parsed model: the
    // content hash is the version's identity, and it must be the hash of the
    // file that was reviewed.
    let content = match String::from_utf8(raw) {
        Ok(c) => c,
        Err(_) => {
            eprintln!(
                "{} {} is not valid UTF-8",
                err_glyph(),
                args.contract.display()
            );
            return Ok(CODE_USAGE);
        }
    };
    let dataset = args
        .dataset
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .unwrap_or(contract.dataset.as_str())
        .to_owned();

    let dest = match push::resolve_endpoint(args.api.as_deref(), offline, "contracts") {
        Ok(Reporting::To(dest)) => dest,
        Ok(Reporting::Off) => {
            eprintln!(
                "{} register needs a project API key in {}",
                err_glyph(),
                push::TOKEN_VAR
            );
            return Ok(CODE_USAGE);
        }
        Ok(Reporting::Suppressed(why)) => {
            eprintln!("{} cannot register: {why}", err_glyph());
            return Ok(CODE_USAGE);
        }
        Err(e) => {
            print_push_error(&e);
            return Ok(CODE_USAGE);
        }
    };

    let recorded = match register::put(
        &dest,
        &dataset,
        &content,
        args.version_label.as_deref(),
        args.source_url.as_deref(),
    ) {
        Ok(r) => r,
        Err(e) => {
            print_push_error(&e);
            // Nothing was registered. A merge job that shrugs this off leaves
            // the registry describing a dataset that no longer exists.
            return Ok(CODE_INTERNAL);
        }
    };

    if recorded.is_active() {
        let label = recorded
            .version_label
            .as_deref()
            .map(|l| format!(" ({l})"))
            .unwrap_or_default();
        let novelty = if recorded.created {
            "is now in force"
        } else {
            "was already in force"
        };
        println!(
            "{} {}{label} {novelty} — {}",
            ok_glyph(),
            recorded.dataset,
            short_sha(&recorded.content_sha256)
        );
    } else {
        let why = recorded.message.clone().unwrap_or_else(|| {
            "the version in force keeps applying until somebody approves it".to_owned()
        });
        println!(
            "{} {} recorded as {} — {why}",
            warn_glyph(),
            recorded.dataset,
            recorded.status
        );
    }

    Ok(if args.fail_if_pending && !recorded.is_active() {
        CODE_FAILURES
    } else {
        CODE_OK
    })
}

/// Enough of a content hash to recognise, not so much that it fills a line.
fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

fn print_preflight_human(p: &preflight::Preflight) {
    let glyph = if p.is_breaking() || p.is_invalid() {
        err_glyph()
    } else {
        ok_glyph()
    };
    println!("{glyph} {}", p.summary);
    for (level, path, message) in &p.findings {
        let where_ = if path.is_empty() {
            String::new()
        } else {
            format!("{path}: ")
        };
        println!("  {level}  {where_}{message}");
    }
    if !p.changes.is_empty() {
        println!();
        print_diff_lines(&p.changes);
    }
    if !p.consumers.is_empty() {
        println!("\n  who breaks:");
        for (name, whole, muted) in &p.consumers {
            let mut note = String::new();
            if *whole {
                note.push_str(" (reads the whole dataset)");
            }
            if *muted {
                note.push_str(" (muted — will not hear)");
            }
            println!("    {name}{note}");
        }
    }
}

/// The impact-tagged lines, shared by both diff paths so a change reads the
/// same whether it was compared against a file or against the registry.
fn print_diff_lines(changes: &[(String, String, String)]) {
    let color = use_color(false);
    for (impact, path, description) in changes {
        let tag = match (impact.as_str(), color) {
            ("breaking", true) => "BREAKING".red().to_string(),
            ("breaking", false) => "BREAKING".to_owned(),
            ("non_breaking", true) => "non-breaking".yellow().to_string(),
            ("non_breaking", false) => "non-breaking".to_owned(),
            (_, true) => "cosmetic".dimmed().to_string(),
            (_, false) => "cosmetic".to_owned(),
        };
        println!("  {tag}  {path}: {description}");
    }
}

fn preflight_json(p: &preflight::Preflight) -> Result<String, serde_json::Error> {
    let changes: Vec<_> = p
        .changes
        .iter()
        .map(|(impact, path, description)| {
            serde_json::json!({ "impact": impact, "path": path, "description": description })
        })
        .collect();
    let consumers: Vec<_> = p
        .consumers
        .iter()
        .map(|(name, whole, muted)| {
            serde_json::json!({ "name": name, "whole_dataset": whole, "muted": muted })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "verdict": p.verdict,
        "dataset": p.dataset,
        "summary": p.summary,
        "breaking": p.breaking,
        "requires_review": p.requires_review,
        "baseline_version_label": p.baseline_version_label,
        "changes": changes,
        "impact": { "declared": p.declared, "consumers": consumers },
    }))
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
            (Impact::Semantic, true) => "SEMANTIC".magenta().to_string(),
            (Impact::Semantic, false) => "SEMANTIC".to_owned(),
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
                    Impact::Semantic => "semantic",
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

/// A yellow `⚠`, for something that went wrong without changing the verdict.
fn warn_glyph() -> String {
    if stderr_color() {
        "⚠".yellow().to_string()
    } else {
        "⚠".to_owned()
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
