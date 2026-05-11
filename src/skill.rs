use anyhow::{Context, Result};
use std::io::Write;
use tracing::info;

use crate::cli::SkillArgs;
use crate::db::SCHEMA_SQL;

/// Raw skill markdown with a `{{SCHEMA_DDL}}` placeholder. Lives in a
/// separate file so it can be edited without touching Rust source.
const SKILL_TEMPLATE: &str = include_str!("skill_template.md");

pub fn run(args: &SkillArgs) -> Result<()> {
    let body = render();

    if args.stdout {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(body.as_bytes())?;
        return Ok(());
    }

    if let Some(parent) = args.output.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output dir {}", parent.display()))?;
    }
    std::fs::write(&args.output, body)
        .with_context(|| format!("writing skill to {}", args.output.display()))?;
    info!("wrote {}", args.output.display());
    Ok(())
}

fn render() -> String {
    SKILL_TEMPLATE.replace("{{SCHEMA_DDL}}", SCHEMA_SQL.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_contains_schema_placeholder() {
        assert!(SKILL_TEMPLATE.contains("{{SCHEMA_DDL}}"));
    }

    #[test]
    fn render_substitutes_schema() {
        let out = render();
        assert!(!out.contains("{{SCHEMA_DDL}}"));
        assert!(out.contains("create table if not exists scans"));
        assert!(out.contains("create table if not exists lineage_edges"));
    }
}
