mod cli;
mod db;
mod git_ops;
mod scan;
mod skill;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    let parsed = cli::Cli::parse();

    let default_filter = if parsed.verbose {
        "histoire=debug"
    } else {
        "histoire=info"
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    match parsed.command {
        cli::Command::Scan(args) => scan::run(&args),
        cli::Command::Skill(args) => skill::run(&args),
    }
}
