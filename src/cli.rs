use chrono::{Months, NaiveDate, Utc};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "histoire",
    version,
    about = "Recursively trace history behind changed lines on the current branch."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Scan the current branch and write the history graph to SQLite.
    Scan(ScanArgs),
    /// Emit a SKILL.md describing how to use histoire and query its SQLite output.
    Skill(SkillArgs),
}

#[derive(Parser, Debug)]
pub struct ScanArgs {
    /// Base ref to compare HEAD against.
    #[arg(default_value = "origin/main")]
    pub base_ref: String,

    /// SQLite database path. Defaults to <git-dir>/histoire.sqlite.
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// Maximum recursion depth for blame expansion.
    #[arg(long, default_value_t = 5)]
    pub max_depth: u32,

    /// Stop at commits older than this date (yyyy-mm-dd). Defaults to six months ago.
    #[arg(long)]
    pub since: Option<NaiveDate>,

    /// Include binary files in blame (still recorded as binary_skipped events otherwise).
    #[arg(long)]
    pub include_binary: bool,

    /// Rename detection similarity threshold (0-100). Lower is more aggressive.
    #[arg(long, default_value_t = 50)]
    pub rename_threshold: u16,
}

#[derive(Parser, Debug)]
pub struct SkillArgs {
    /// Output path for the skill markdown file.
    #[arg(long, short = 'o', default_value = "SKILL.md")]
    pub output: PathBuf,

    /// Print the skill to stdout instead of writing to a file.
    #[arg(long)]
    pub stdout: bool,
}

pub fn default_since() -> NaiveDate {
    let today = Utc::now().date_naive();
    today.checked_sub_months(Months::new(6)).unwrap_or(today)
}
