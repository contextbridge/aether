mod build_settings;
mod harness;
mod recommendations;
mod tui_runner;
use aether_project::user_settings_path;
pub use build_settings::{
    Preset, build_batteries_included_build_agent, build_batteries_included_explore_agent,
    build_batteries_included_plan_agent, build_batteries_included_settings,
};
pub use harness::HarnessIntegration;
use llm::catalog::Provider;
use recommendations::recommended_for_provider;
use std::fs;
use std::io;
use std::path::PathBuf;
use thiserror::Error;

use crate::init::build_settings::supported_providers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitScope {
    User,
    Project,
}

impl InitScope {
    pub fn asset_path(self, asset_rel_path: &str) -> String {
        match self {
            Self::User => asset_rel_path.to_string(),
            Self::Project => format!(".aether/{asset_rel_path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitTarget {
    pub scope: InitScope,
    pub settings_path: PathBuf,
    pub asset_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct InitRequest {
    pub target: InitTargetRequest,
    pub provider: Option<Provider>,
    pub preset: Option<Preset>,
    pub harnesses: Vec<HarnessIntegration>,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitTargetRequest {
    User,
    Project { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitOutcome {
    Applied { settings_path: PathBuf, missing_env_var: Option<&'static str> },
    Cancelled,
    AlreadyInitialized { settings_path: PathBuf },
}

#[derive(Debug, Error)]
pub enum InitError {
    #[error("could not determine user home directory; set $AETHER_HOME or $HOME")]
    NoHomeDir,
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("provider `{provider}` does not have a curated preset; supported: {supported}")]
    UnsupportedProvider { provider: Provider, supported: String },
    #[error("terminal error: {0}")]
    Terminal(#[source] io::Error),
}

impl InitTarget {
    pub fn user(aether_home: impl Into<PathBuf>) -> Self {
        let asset_root = aether_home.into();
        Self { scope: InitScope::User, settings_path: asset_root.join("settings.json"), asset_root }
    }

    pub fn project(project_root: impl Into<PathBuf>) -> Self {
        let project_root = project_root.into();
        let asset_root = project_root.join(".aether");
        Self { scope: InitScope::Project, settings_path: asset_root.join("settings.json"), asset_root }
    }
}

impl InitRequest {
    pub fn user_onboarding() -> Self {
        Self { target: InitTargetRequest::User, provider: None, preset: None, harnesses: vec![], force: false }
    }
}

pub fn apply_init(
    target: InitTarget,
    provider: Provider,
    preset: Preset,
    harnesses: &[HarnessIntegration],
    force: bool,
) -> Result<InitOutcome, InitError> {
    if target.settings_path.is_file() && !force {
        return Ok(InitOutcome::AlreadyInitialized { settings_path: target.settings_path });
    }

    let recs = recommended_for_provider(provider).ok_or_else(|| InitError::UnsupportedProvider {
        provider,
        supported: supported_providers().map(Provider::parser_name).collect::<Vec<_>>().join(", "),
    })?;

    let built = build_settings::build_preset(preset, provider, &recs, target.scope, harnesses);

    fs::create_dir_all(&target.asset_root).map_err(|e| InitError::Io { path: target.asset_root.clone(), source: e })?;

    for file in built.files {
        let dest = target.asset_root.join(file.path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| InitError::Io { path: parent.to_path_buf(), source: e })?;
        }
        fs::write(&dest, file.body).map_err(|e| InitError::Io { path: dest, source: e })?;
    }

    let serialized = serde_json::to_string_pretty(&built.settings).expect("AetherSettings always serializes");
    fs::write(&target.settings_path, format!("{serialized}\n"))
        .map_err(|e| InitError::Io { path: target.settings_path.clone(), source: e })?;

    let missing_env_var = provider.required_env_var().filter(|var| std::env::var(var).is_err());
    Ok(InitOutcome::Applied { settings_path: target.settings_path, missing_env_var })
}

pub async fn run_init(request: InitRequest) -> Result<InitOutcome, InitError> {
    let target = resolve_target(&request)?;

    if target.settings_path.is_file() && !request.force {
        return Ok(InitOutcome::AlreadyInitialized { settings_path: target.settings_path });
    }

    let Some((provider, preset, harnesses)) =
        tui_runner::run_wizard(request.provider, request.preset, request.harnesses).await?
    else {
        return Ok(InitOutcome::Cancelled);
    };

    apply_init(target, provider, preset, &harnesses, request.force)
}

pub fn next_steps_message(outcome: &InitOutcome) -> Option<String> {
    match outcome {
        InitOutcome::Applied { settings_path, missing_env_var: Some(var) } => {
            Some(format!("Wrote {}. Set ${var} in your shell.", settings_path.display()))
        }
        InitOutcome::Applied { settings_path, missing_env_var: None } => {
            Some(format!("Wrote {}", settings_path.display()))
        }
        InitOutcome::AlreadyInitialized { settings_path } => {
            Some(format!("Already initialized at {}; pass --force to overwrite.", settings_path.display()))
        }
        InitOutcome::Cancelled => None,
    }
}

fn resolve_target(request: &InitRequest) -> Result<InitTarget, InitError> {
    match &request.target {
        InitTargetRequest::User => {
            let settings_path = user_settings_path().ok_or(InitError::NoHomeDir)?;
            let home = settings_path.parent().ok_or(InitError::NoHomeDir)?.to_path_buf();
            Ok(InitTarget::user(home))
        }
        InitTargetRequest::Project { path } => {
            let root = path.canonicalize().unwrap_or_else(|_| path.clone());
            Ok(InitTarget::project(root))
        }
    }
}
