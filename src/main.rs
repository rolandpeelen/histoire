mod cli;
mod db;
mod git_ops;
mod scan;
mod skill;

use anyhow::Result;
use clap::Parser;
use std::path::{Path, PathBuf};
use tracing::info;
use tracing_subscriber::EnvFilter;

fn persist_and_report(scan: &db::Scan, db_override: Option<&Path>) -> Result<()> {
    let db_path: PathBuf = match db_override {
        Some(path) => path.to_path_buf(),
        None => Path::new(&scan.repository.git_dir_path).join("histoire.sqlite"),
    };
    info!("database: {}", db_path.display());

    let mut conn = db::open_fresh(&db_path)?;
    scan.save(&mut conn)?;

    let summary = scan::summary(scan);
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

fn run_scan(args: &cli::ScanArgs) -> Result<()> {
    let scan = scan::run_scan(args)?;
    persist_and_report(&scan, args.db.as_deref())
}

fn run_trace(args: &cli::TraceArgs) -> Result<()> {
    let scan = scan::run_trace(args)?;
    persist_and_report(&scan, args.db.as_deref())
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
        cli::Command::Trace(args) => run_trace(&args),
        cli::Command::Skill(args) => skill::run(&args),
    }
}
