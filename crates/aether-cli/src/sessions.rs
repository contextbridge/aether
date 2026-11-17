use aether_sessions::{MigrationReport, migrate_sessions, sessions_dir};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Clone, Subcommand)]
pub enum SessionsCommand {
    /// Upgrade session logs written by older Aether versions to the current format
    Migrate(SessionsMigrateArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SessionsMigrateArgs {
    /// Sessions directory to migrate (default: `$AETHER_HOME/sessions`)
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionsCommandError {
    #[error("Aether home directory is not configured")]
    MissingAetherHome,
    #[error("failed to migrate sessions: {0}")]
    Io(#[from] std::io::Error),
}

pub fn run(command: SessionsCommand) -> Result<(), SessionsCommandError> {
    match command {
        SessionsCommand::Migrate(args) => {
            let dir = args.dir.or_else(sessions_dir).ok_or(SessionsCommandError::MissingAetherHome)?;
            print_report(&migrate_sessions(&dir)?);
            Ok(())
        }
    }
}

fn print_report(report: &MigrationReport) {
    println!("Scanned {} session files, rewrote {}", report.files_scanned, report.files_rewritten);
    println!("  upgraded:  {} lines", report.lines_upgraded);
    println!("  dropped:   {} transient lines (never persisted by the current format)", report.lines_dropped_transient);
    let unconvertible: usize = report.lines_dropped_unconvertible.values().sum();
    if unconvertible > 0 {
        println!("  dropped:   {unconvertible} lines with no current equivalent");
        for (error, count) in &report.lines_dropped_unconvertible {
            println!("    {count:>8}  {error}");
        }
    }
    if report.files_rewritten > 0 {
        println!("Originals are kept beside each rewritten log as <session>.jsonl.bak");
    }
}
