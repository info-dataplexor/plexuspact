//! Developer automation (`cargo xtask <task>`).
//!
//! Tasks:
//! * `gen-schema` — write `schema/contract.v1.json` from the contract model.
//! * `gen-fixture` — write a deterministic CSV fixture with a controllable
//!   bad-row ratio, for benchmarks and streaming/memory tests.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "xtask", about = "PlexusPact developer tasks")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Generate the JSON Schema for `contract.yaml`.
    GenSchema {
        /// Output path.
        #[arg(long, default_value = "schema/contract.v1.json")]
        out: PathBuf,
    },
    /// Generate a deterministic CSV fixture matching the canonical schema.
    GenFixture {
        /// Number of data rows.
        #[arg(long, default_value_t = 1000)]
        rows: usize,
        /// RNG seed (deterministic output).
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Fraction of rows that carry a deliberate violation (0.0–1.0).
        #[arg(long, default_value_t = 0.02)]
        bad_ratio: f64,
        /// Output path.
        #[arg(long, default_value = "fixtures/generated/signups.csv")]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().task {
        Task::GenSchema { out } => gen_schema(&out),
        Task::GenFixture {
            rows,
            seed,
            bad_ratio,
            out,
        } => gen_fixture(rows, seed, bad_ratio, &out),
    }
}

fn gen_schema(out: &PathBuf) -> Result<()> {
    let schema = plexuspact_contract::json_schema();
    let pretty = serde_json::to_string_pretty(&schema)?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(out, format!("{pretty}\n"))
        .with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}

fn gen_fixture(rows: usize, seed: u64, bad_ratio: f64, out: &PathBuf) -> Result<()> {
    use rand::Rng;
    use rand::SeedableRng;
    use std::fmt::Write as _;

    if !(0.0..=1.0).contains(&bad_ratio) {
        bail!("--bad-ratio must be between 0.0 and 1.0");
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);
    let countries = ["US", "DE", "FR", "GB", "IN", "JP", "BR", "CA", "AU", "ES"];
    let plans = ["free", "pro", "enterprise"];

    let mut csv = String::with_capacity(rows * 48);
    csv.push_str("user_id,email,age,country,plan,signed_up\n");

    for i in 0..rows {
        let bad = rng.gen::<f64>() < bad_ratio;
        let user_id = format!("u-{i:07}");
        // Base (valid) values.
        let mut email = format!("user{i}@example.com");
        let mut age: i64 = rng.gen_range(18..=90);
        let mut country = countries[rng.gen_range(0..countries.len())].to_owned();
        let mut plan = plans[rng.gen_range(0..plans.len())].to_owned();
        // Deterministic timestamp within a day window.
        let minute = i % 1440;
        let signed_up = format!("2026-07-08T{:02}:{:02}:00Z", minute / 60, minute % 60);

        if bad {
            // Introduce exactly one class of violation per bad row.
            match rng.gen_range(0..5) {
                0 => email = format!("broken{i}@@example"),
                1 => age = rng.gen_range(0..18),
                2 => country = "usa".to_owned(),
                3 => plan = "premium".to_owned(),
                _ => age = rng.gen_range(121..200),
            }
        }

        let _ = writeln!(csv, "{user_id},{email},{age},{country},{plan},{signed_up}");
    }

    std::fs::write(out, csv).with_context(|| format!("writing {}", out.display()))?;
    println!("wrote {} rows to {}", rows, out.display());
    Ok(())
}
