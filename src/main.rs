mod cli;
mod db;
mod git_ops;
mod scan;
mod skill;

use anyhow::Result;
use clap::Parser;
use std::path::Path;
use tracing::info;
use tracing_subscriber::EnvFilter;

fn run_scan(args: &cli::ScanArgs) -> Result<()> {
    let result = scan::run(args)?;

    let db_path = args
        .db
        .clone()
        .unwrap_or_else(|| Path::new(&result.repository.git_dir_path).join("histoire.sqlite"));
    info!("database: {}", db_path.display());

    let mut conn = db::open_fresh(&db_path)?;
    result.save(&mut conn)?;

    let summary = result.summary();
    info!(
        "scan complete: seed_files={}, seed_ranges={}, requests_processed={}, commits_discovered={}, terminal_spans={}",
        summary.seed_files,
        summary.seed_ranges,
        summary.requests_processed,
        summary.commits_discovered,
        summary.terminal_spans
    );
    Ok(())
}

fn main() -> Result<()> {
    let parsed = cli::Cli::parse();

    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("histoire=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();

    match parsed.command {
        cli::Command::Scan(args) => run_scan(&args),
        cli::Command::Skill(args) => skill::run(&args),
    }
}
