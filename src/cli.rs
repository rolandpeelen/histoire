use anyhow::{Context, Result, anyhow};
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
    /// Trace the history of a single file:line target.
    Trace(TraceArgs),
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
pub struct TraceArgs {
    /// Target as `path:line` (e.g. `src/main.rs:300`) or `path:start-end`.
    pub target: String,

    /// SQLite database path. Defaults to <git-dir>/histoire.sqlite.
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// Maximum recursion depth for blame expansion.
    #[arg(long, default_value_t = 20)]
    pub max_depth: u32,

    /// Stop at commits older than this date (yyyy-mm-dd). Defaults to twelve months ago.
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

/// A `path:line` or `path:start-end` target parsed off the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceTarget {
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
}

impl TraceTarget {
    /// Parse a `path:line` or `path:start-end` argument. Splits on the
    /// rightmost `:` so paths containing colons still work as long as the
    /// suffix is a numeric line or range.
    pub fn parse(target: &str) -> Result<Self> {
        let (path, range_part) = target.rsplit_once(':').ok_or_else(|| {
            anyhow!("target '{target}' must be of the form path:line or path:start-end")
        })?;
        if path.is_empty() {
            return Err(anyhow!("target '{target}' has an empty path"));
        }
        let (start_str, end_str) = match range_part.split_once('-') {
            Some((start, end)) => (start, end),
            None => (range_part, range_part),
        };
        let start_line: u32 = start_str
            .parse()
            .with_context(|| format!("parsing start line '{start_str}'"))?;
        let end_line: u32 = end_str
            .parse()
            .with_context(|| format!("parsing end line '{end_str}'"))?;
        if start_line == 0 || end_line == 0 {
            return Err(anyhow!("line numbers must be 1-indexed"));
        }
        if end_line < start_line {
            return Err(anyhow!(
                "end line {end_line} is before start line {start_line}"
            ));
        }
        Ok(Self {
            path: path.to_string(),
            start_line,
            end_line,
        })
    }
}

fn months_ago(months: u32) -> NaiveDate {
    let today = Utc::now().date_naive();
    today
        .checked_sub_months(Months::new(months))
        .unwrap_or(today)
}

pub fn default_scan_since() -> NaiveDate {
    months_ago(6)
}

pub fn default_trace_since() -> NaiveDate {
    months_ago(12)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_line() -> Result<()> {
        let target = TraceTarget::parse("src/main.rs:42")?;
        assert_eq!(target.path, "src/main.rs");
        assert_eq!(target.start_line, 42);
        assert_eq!(target.end_line, 42);
        Ok(())
    }

    #[test]
    fn parse_line_range() -> Result<()> {
        let target = TraceTarget::parse("foo.rs:10-25")?;
        assert_eq!(target.path, "foo.rs");
        assert_eq!(target.start_line, 10);
        assert_eq!(target.end_line, 25);
        Ok(())
    }

    #[test]
    fn parse_rejects_missing_line() {
        assert!(TraceTarget::parse("src/main.rs").is_err());
    }

    #[test]
    fn parse_rejects_empty_path() {
        assert!(TraceTarget::parse(":42").is_err());
    }

    #[test]
    fn parse_rejects_zero_line() {
        assert!(TraceTarget::parse("foo.rs:0").is_err());
    }

    #[test]
    fn parse_rejects_inverted_range() {
        assert!(TraceTarget::parse("foo.rs:30-10").is_err());
    }
}
