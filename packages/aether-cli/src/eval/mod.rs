mod container;
mod settings_agent;

pub use settings_agent::SettingsAgent;

use crate::credentials::build_oauth_credential_store;
use crate::error::CliError;
use crate::headless::resolve_spec;
use crate::mcp_config_args::McpConfigArgs;
use crate::provider_connection_args::ProviderConnectionArgs;
use crate::settings_args::SettingsSourceArgs;
use aether_auth::OAuthCredentialStorage;
use aether_core::agent_spec::{AgentSpec, McpConfigSource};
use crucible::{EvalSpec, SpecReport, run_spec};
use llm::ProviderConnectionOverrides;
use llm::StreamingModelProvider;
use llm::parser::ModelProviderParser;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

#[derive(clap::Args)]
pub struct EvalArgs {
    /// Eval spec files or directories to run (default: `.aether/evals`).
    pub paths: Vec<PathBuf>,

    /// Working directory used to resolve settings and relative spec paths.
    #[arg(short = 'C', long = "cwd", default_value = ".")]
    pub cwd: PathBuf,

    #[command(flatten)]
    pub settings_source: SettingsSourceArgs,

    #[command(flatten)]
    pub provider_connection: ProviderConnectionArgs,

    #[command(flatten)]
    pub mcp_config: McpConfigArgs,

    /// Run exactly one spec file (used internally by container mode).
    #[arg(long = "single")]
    pub single: Option<PathBuf>,

    /// Output format.
    #[arg(long, default_value = "text")]
    pub output: EvalOutputFormat,
}

#[derive(Clone, clap::ValueEnum)]
pub enum EvalOutputFormat {
    Text,
    Json,
}

/// Discover and run declarative eval specs against the user's configured agents.
pub async fn run_eval_command(args: EvalArgs) -> Result<ExitCode, CliError> {
    let cwd = args.cwd.canonicalize().map_err(CliError::IoError)?;
    let spec_files = match &args.single {
        Some(path) => vec![cwd.join(path)],
        None => discover_specs(&args.paths, &cwd),
    };
    if spec_files.is_empty() {
        return Err(CliError::Eval("no eval specs found".to_string()));
    }

    let oauth_store = build_oauth_credential_store(&args.settings_source, &cwd)?;
    let mcp_sources = args.mcp_config.sources(&cwd);

    let mut all_passed = true;
    let mut entries = Vec::new();

    for spec_file in &spec_files {
        let json = std::fs::read_to_string(spec_file).map_err(CliError::IoError)?;
        let spec =
            EvalSpec::parse(&json).map_err(|error| CliError::Eval(format!("{}: {error}", spec_file.display())))?;
        let base_dir = spec_file.parent().unwrap_or(&cwd).to_path_buf();

        // A spec with an `environment` runs inside its own container — unless we
        // are already the in-container `--single` invocation, which runs on host.
        let report = match &spec.environment {
            Some(environment) if args.single.is_none() => {
                container::run_spec_in_container(spec_file, environment, &cwd, &base_dir)?
            }
            _ => {
                run_one_spec(
                    &spec,
                    &base_dir,
                    &cwd,
                    &args.settings_source,
                    args.provider_connection.clone().into_overrides(),
                    &oauth_store,
                    &mcp_sources,
                )
                .await?
            }
        };

        if !report.passed() {
            all_passed = false;
        }

        if matches!(args.output, EvalOutputFormat::Text) {
            print_text_report(spec_file, &report);
        }
        entries.push(SpecReportEntry { spec: spec_file.display().to_string(), report });
    }

    if matches!(args.output, EvalOutputFormat::Json) {
        emit_json(args.single.is_some(), entries);
    }

    Ok(if all_passed { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

#[derive(Serialize)]
struct SpecReportEntry {
    spec: String,
    report: SpecReport,
}

async fn run_one_spec(
    spec: &EvalSpec,
    base_dir: &Path,
    cwd: &Path,
    settings_source: &SettingsSourceArgs,
    provider_connections: ProviderConnectionOverrides,
    oauth_store: &Arc<dyn OAuthCredentialStorage>,
    mcp_sources: &[McpConfigSource],
) -> Result<SpecReport, CliError> {
    let agent_spec =
        resolve_spec(spec.agent.as_deref(), spec.model.as_deref(), cwd, settings_source, provider_connections)?;
    let judge_provider =
        build_judge_provider(&agent_spec, spec.judge_model.as_deref(), Arc::clone(oauth_store)).await?;
    let agent = SettingsAgent::new(agent_spec, mcp_sources.to_vec(), Arc::clone(oauth_store));

    run_spec(spec, base_dir, &agent, judge_provider.as_ref()).await.map_err(|error| CliError::Eval(error.to_string()))
}

async fn build_judge_provider(
    spec: &AgentSpec,
    judge_model: Option<&str>,
    oauth_store: Arc<dyn OAuthCredentialStorage>,
) -> Result<Box<dyn StreamingModelProvider>, CliError> {
    let parser = ModelProviderParser::default()
        .with_provider_connections(spec.provider_connections.clone())
        .with_codex_provider(oauth_store);
    let model = judge_model.unwrap_or(spec.model.as_str());
    let (provider, _) = parser.parse(model).await.map_err(|error| CliError::ModelError(error.to_string()))?;
    Ok(provider)
}

fn discover_specs(paths: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = if paths.is_empty() {
        vec![cwd.join(".aether/evals")]
    } else {
        paths.iter().map(|path| cwd.join(path)).collect()
    };

    let mut specs = Vec::new();
    for root in roots {
        collect_json(&root, &mut specs);
    }
    specs.sort();
    specs
}

fn collect_json(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                collect_json(&entry.path(), out);
            }
        }
    } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
        out.push(path.to_path_buf());
    }
}

fn print_text_report(spec_file: &Path, report: &SpecReport) {
    let status = if report.passed() { "PASS" } else { "FAIL" };
    println!("[{status}] {}", spec_file.display());
    for check in &report.results {
        let mark = if check.passed { '✓' } else { '✗' };
        println!("  {mark} {} — {}", check.label, check.detail);
    }
}

fn emit_json(single: bool, entries: Vec<SpecReportEntry>) {
    let json = if single {
        entries.into_iter().next().map(|entry| serde_json::to_string(&entry.report)).transpose()
    } else {
        serde_json::to_string(&entries).map(Some)
    };
    if let Ok(Some(json)) = json {
        println!("{json}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discover_specs_defaults_to_aether_evals_dir_recursively() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join(".aether/evals/nested");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.path().join(".aether/evals/a.json"), "{}").unwrap();
        fs::write(nested.join("b.json"), "{}").unwrap();
        fs::write(dir.path().join(".aether/evals/ignore.txt"), "x").unwrap();

        let specs = discover_specs(&[], dir.path());

        let names: Vec<_> = specs.iter().map(|path| path.file_name().unwrap().to_str().unwrap().to_string()).collect();
        assert_eq!(names, vec!["a.json", "b.json"]);
    }

    #[test]
    fn discover_specs_accepts_explicit_file_path() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("one.json"), "{}").unwrap();

        let specs = discover_specs(&[PathBuf::from("one.json")], dir.path());

        assert_eq!(specs.len(), 1);
        assert!(specs[0].ends_with("one.json"));
    }

    #[test]
    fn discover_specs_empty_when_default_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(discover_specs(&[], dir.path()).is_empty());
    }
}
