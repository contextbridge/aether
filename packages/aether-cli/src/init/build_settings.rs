use super::InitScope;
use super::recommendations::{ProviderRecommendations, recommended_for_provider};
use aether_core::agent_spec::ToolFilter;
use aether_project::{AetherSettings, AgentConfig, McpSourceSpec, PromptSource};
use llm::catalog::Provider;
use mcp_utils::client::{InMemoryServerConfig, InMemoryType, McpServerConfig};

const SYSTEM_PATH: &str = "SYSTEM.md";
const PROJECT_AGENTS_PATH: &str = "${WORKSPACE}/AGENTS.md";
const SYSTEM_MD: &str = include_str!("templates/SYSTEM.md");

const EXPLORER_AGENTS_MD: &str = include_str!("templates/agents/codebase-explorer/AGENTS.md");
const EXPLORER_AGENTS_PATH: &str = "agents/codebase-explorer/AGENTS.md";

const SKILLS_ARGS: &[&str] = &[
    "--dir",
    "${AETHER_HOME}/skills",
    "--dir",
    "${WORKSPACE}/.aether/skills",
    "--notes-dir",
    "${WORKSPACE}/.aether/notes",
];

const READ_ONLY_DENIED_CODING_TOOLS: &[&str] =
    &["coding__bash", "coding__edit_file", "coding__lsp_rename", "coding__write_file"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Preset {
    /// Single agent with bash and skills tools only.
    Minimal,
    /// Plan + Build + Explore agents wired to the full built-in MCP set.
    BatteriesIncluded,
}

pub(crate) struct ResolvedPreset {
    pub settings: AetherSettings,
    pub files: &'static [TemplateFile],
}

pub(crate) struct TemplateFile {
    pub path: &'static str,
    pub body: &'static str,
}

pub fn supported_providers() -> impl Iterator<Item = Provider> {
    Provider::ALL.iter().copied().filter(|p| recommended_for_provider(*p).is_some())
}

pub(crate) fn build_preset(
    preset: Preset,
    provider: Provider,
    recs: &ProviderRecommendations,
    scope: InitScope,
) -> ResolvedPreset {
    match preset {
        Preset::Minimal => minimal_preset(provider, recs, scope),
        Preset::BatteriesIncluded => build_batteries_included_preset(provider, recs, scope),
    }
}

fn minimal_preset(provider: Provider, recs: &ProviderRecommendations, scope: InitScope) -> ResolvedPreset {
    let display = provider.display_name();
    let agent = AgentConfig {
        name: "Default".to_string(),
        description: format!("{display} A minimal agent with only a bash tool and skills"),
        model: recs.plan.model.to_string(),
        reasoning_effort: recs.plan.reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(&[("coding", &[]), ("skills", SKILLS_ARGS)])],
        tools: ToolFilter { allow: vec!["coding__bash".to_string(), "skills__*".to_string()], deny: vec![] },
        ..AgentConfig::default()
    };

    ResolvedPreset {
        files: &[TemplateFile { path: SYSTEM_PATH, body: SYSTEM_MD }],
        settings: AetherSettings { prompts: default_prompts(scope), agents: vec![agent], ..AetherSettings::default() },
    }
}

fn build_batteries_included_preset(
    provider: Provider,
    recs: &ProviderRecommendations,
    scope: InitScope,
) -> ResolvedPreset {
    let display = provider.display_name();
    let plan = AgentConfig {
        name: "Plan".to_string(),
        description: format!("{display} planner (read-only except plan files)"),
        model: recs.plan.model.to_string(),
        reasoning_effort: recs.plan.reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(&[
            ("plan", &[]),
            ("coding", &[]),
            ("skills", SKILLS_ARGS),
            ("subagents", &[]),
            ("tasks", &[]),
            ("survey", &[]),
        ])],
        tools: read_only_coding_tools(),
        ..AgentConfig::default()
    };

    let build = AgentConfig {
        name: "Build".to_string(),
        description: format!("{display} implementor"),
        model: recs.build.model.to_string(),
        reasoning_effort: recs.build.reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(&[
            ("coding", &[]),
            ("skills", SKILLS_ARGS),
            ("subagents", &[]),
            ("tasks", &[]),
            ("survey", &[]),
        ])],
        ..AgentConfig::default()
    };

    let explore = AgentConfig {
        name: "Explore".to_string(),
        description: "Explores codebases to find relevant files, patterns, and integration points".to_string(),
        model: recs.explore.model.to_string(),
        reasoning_effort: recs.explore.reasoning_effort,
        agent_invocable: true,
        prompts: vec![PromptSource::file(settings_asset_path(scope, EXPLORER_AGENTS_PATH))],
        mcps: vec![mcps(&[("coding", &[])])],
        tools: read_only_coding_tools(),
        ..AgentConfig::default()
    };

    ResolvedPreset {
        files: &[
            TemplateFile { path: SYSTEM_PATH, body: SYSTEM_MD },
            TemplateFile { path: EXPLORER_AGENTS_PATH, body: EXPLORER_AGENTS_MD },
        ],
        settings: AetherSettings {
            prompts: default_prompts(scope),
            agents: vec![plan, build, explore],
            ..AetherSettings::default()
        },
    }
}

fn read_only_coding_tools() -> ToolFilter {
    ToolFilter { allow: vec![], deny: READ_ONLY_DENIED_CODING_TOOLS.iter().map(|tool| (*tool).to_string()).collect() }
}

fn default_prompts(scope: InitScope) -> Vec<PromptSource> {
    vec![
        PromptSource::file(settings_asset_path(scope, SYSTEM_PATH)),
        PromptSource::file(PROJECT_AGENTS_PATH).optional(),
    ]
}

fn settings_asset_path(scope: InitScope, asset_rel_path: &str) -> String {
    match scope {
        InitScope::User => asset_rel_path.to_string(),
        InitScope::Project => format!(".aether/{asset_rel_path}"),
    }
}

fn mcps(servers: &[(&str, &[&str])]) -> McpSourceSpec {
    let servers = servers
        .iter()
        .map(|(name, args)| {
            (
                (*name).to_string(),
                McpServerConfig::InMemory(InMemoryServerConfig {
                    type_: InMemoryType::InMemory,
                    args: args.iter().map(|s| (*s).to_string()).collect(),
                    input: None,
                    proxy: false,
                }),
            )
        })
        .collect();
    McpSourceSpec::Inline { servers }
}
