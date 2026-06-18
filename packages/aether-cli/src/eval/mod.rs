mod render;

use crate::output::OutputFormat;
use aether_evals::{
    EvalFileError, EvalFilesReport, EvalSpec, EvalSpecLoadOptions, EvalStreamEvent, ResolvedEvalSpec,
    WorkspaceRetention, run_eval_spec_streaming, run_eval_specs,
};
use render::render;
use std::env::current_dir;
use std::fs::read_to_string;
use std::io::{Read, Write, stdin, stdout};
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

    /// Run a single eval from a JSON file, or from stdin with `-`. Emits newline-delimited
    /// `EvalStreamEvent` JSON on stdout: an `agent_message` event per agent message as it streams,
    /// terminated by one `outcome` event.
    #[arg(long, value_name = "PATH_OR_DASH")]
    pub spec_file: Option<String>,

    /// Retain the eval workspace on disk and include `retainedWorkspace` in JSON output. Used with
    /// `--spec-file`.
    #[arg(long)]
    pub retain_workspace: bool,

    /// Base directory for resolving relative paths in a `--spec-file` spec. Defaults to the current
    /// working directory.
    #[arg(long)]
    pub base_dir: Option<PathBuf>,
}

pub async fn run(args: EvalArgs) -> Result<ExitCode, EvalFileError> {
    if let Some(source) = args.spec_file {
        let base_dir = args.base_dir.or_else(|| current_dir().ok()).unwrap_or_else(|| PathBuf::from("."));
        let json = read_eval(&source)?;
        let retention = if args.retain_workspace { WorkspaceRetention::Retain } else { WorkspaceRetention::Discard };
        let spec: EvalSpec = serde_json::from_str(&json).map_err(|source| EvalFileError::ParseSpec { source })?;
        let case = ResolvedEvalSpec::resolve(spec, base_dir)?;
        let outcome = run_eval_spec_streaming(case, retention, |message| {
            print_json_event(&EvalStreamEvent::AgentMessage { message: message.clone() });
        })
        .await?;
        print_json_event(&EvalStreamEvent::Outcome { outcome });
        return Ok(ExitCode::SUCCESS);
    }

    let cases = ResolvedEvalSpec::load(EvalSpecLoadOptions { paths: args.paths, filter: args.name })?;
    let evals = run_eval_specs(cases, WorkspaceRetention::Discard, args.max_concurrency).await?;
    let report = EvalFilesReport { evals };

    println!("{}", render(&report, args.output));
    Ok(if report.passed() { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn print_json_event(event: &EvalStreamEvent) {
    println!("{}", serde_json::to_string(event).expect("EvalStreamEvent serializes to JSON"));
    let _ = stdout().flush();
}

fn read_eval(source: &str) -> Result<String, EvalFileError> {
    if source == "-" {
        let mut json = String::new();
        stdin()
            .read_to_string(&mut json)
            .map_err(|source| EvalFileError::ReadEvalFile { path: PathBuf::from("-"), source })?;
        return Ok(json);
    }

    let path = PathBuf::from(source);
    read_to_string(&path).map_err(|source| EvalFileError::ReadEvalFile { path, source })
}
