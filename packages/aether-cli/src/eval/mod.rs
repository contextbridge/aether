mod render;

use crate::output::OutputFormat;
use aether_evals::{EvalFileError, EvalRunOptions, run_eval_files};
use render::render;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_MAX_CONCURRENCY: NonZeroUsize = NonZeroUsize::new(5).unwrap();

#[derive(clap::Args)]
pub struct EvalArgs {
    /// Eval JSON files or directories to discover eval files under. Defaults to ./evals.
    pub paths: Vec<PathBuf>,

    /// Run only the eval with this name.
    #[arg(long)]
    pub name: Option<String>,

    /// Maximum number of eval files to run concurrently.
    #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENCY)]
    pub max_concurrency: NonZeroUsize,

    /// Output format
    #[arg(long, default_value = "text")]
    pub output: OutputFormat,
}

pub async fn run(args: EvalArgs) -> Result<ExitCode, EvalFileError> {
    let report =
        run_eval_files(EvalRunOptions { paths: args.paths, filter: args.name, max_concurrency: args.max_concurrency })
            .await?;

    println!("{}", render(&report, args.output));
    Ok(if report.passed() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
