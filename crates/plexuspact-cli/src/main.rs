//! PlexusPact CLI entry point.
//!
//! Exit codes (ADR-004): `0` pass, `1` error-severity check failures,
//! `2` usage/contract error, `3` internal error.

mod cli;
mod commands;
mod exit;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;
use crate::exit::CODE_INTERNAL;

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let code = commands::dispatch(cli).unwrap_or_else(|err| {
        // Any error that bubbles this far is an internal bug: report and exit 3.
        eprintln!("plexuspact: internal error: {err:?}");
        CODE_INTERNAL
    });

    ExitCode::from(code)
}

/// Configures `tracing` verbosity from `-v`/`-vv` (stderr only).
fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = std::env::var("PLEXUSPACT_LOG").unwrap_or_else(|_| level.to_owned());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .without_time()
        .try_init();
}
