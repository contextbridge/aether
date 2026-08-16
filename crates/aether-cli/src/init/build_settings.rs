use super::InitScope;
use super::harness::HarnessIntegration;
use super::recommendations::{ProviderRecommendations, recommended_for_provider};
use aether_core::agent_spec::{ToolFilter, ToolMatcher};
use aether_project::{AetherSettings, AgentConfig, McpSourceSpec, PromptSource};
use llm::{ReasoningEffort, catalog::Provider};
use mcp_utils::client::{InMemoryServerConfig, InMemoryType, McpServerConfig, ToolExposure};

const SYSTEM_PATH: &str = "SYSTEM.md";
const SYSTEM_MD: &str = include_str!("templates/SYSTEM.md");

const EXPLORER_AGENTS_MD: &str = include_str!("templates/agents/codebase-explorer/AGENTS.md");
const EXPLORER_AGENTS_PATH: &str = "agents/codebase-explorer/AGENTS.md";

const AETHER_SKILL_MD: &str = include_str!("templates/skills/aether/SKILL.md");
const AETHER_SKILL_PATH: &str = "skills/aether/SKILL.md";

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

pub fn build_batteries_included_settings(
    display: &str,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    scope: InitScope,
    harnesses: &[HarnessIntegration],
) -> AetherSettings {
    let plan = build_batteries_included_plan_agent(display, model.clone(), reasoning_effort, harnesses);
    let build = build_batteries_included_build_agent(display, model.clone(), reasoning_effort, harnesses);
    let explore = build_batteries_included_explore_agent(model, reasoning_effort, scope, harnesses);

    AetherSettings {
        prompts: batteries_prompts(scope, harnesses),
        agents: vec![plan, build, explore],
        ..AetherSettings::default()
    }
}

fn build_batteries_included_plan_agent(
    display: &str,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    harnesses: &[HarnessIntegration],
) -> AgentConfig {
    let coding = coding_args(harnesses);
    let skills = skills_args(harnesses);
    AgentConfig {
        name: "Plan".to_string(),
        description: format!("{display} planner (read-only except plan files)"),
        model,
        reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(vec![
            ("plan", vec![]),
            ("coding", coding),
            ("skills", skills),
            ("subagents", vec![]),
            ("tasks", vec![]),
            ("survey", vec![]),
        ])],
        tools: read_only_coding_tools(),
        ..AgentConfig::default()
    }
}

fn build_batteries_included_build_agent(
    display: &str,
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    harnesses: &[HarnessIntegration],
) -> AgentConfig {
    let coding = coding_args(harnesses);
    let skills = skills_args(harnesses);
    AgentConfig {
        name: "Build".to_string(),
        description: format!("{display} implementor"),
        model,
        reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(vec![
            ("coding", coding),
            ("skills", skills),
            ("subagents", vec![]),
            ("tasks", vec![]),
            ("survey", vec![]),
        ])],
        ..AgentConfig::default()
    }
}

fn build_batteries_included_explore_agent(
    model: String,
    reasoning_effort: Option<ReasoningEffort>,
    scope: InitScope,
    harnesses: &[HarnessIntegration],
) -> AgentConfig {
    let coding = coding_args(harnesses);
    AgentConfig {
        name: "Explore".to_string(),
        description: "Explores codebases to find relevant files, patterns, and integration points".to_string(),
        model,
        reasoning_effort,
        agent_invocable: true,
        prompts: vec![PromptSource::file(scope.asset_path(EXPLORER_AGENTS_PATH))],
        mcps: vec![mcps(vec![("coding", coding)])],
        tools: read_only_coding_tools(),
        ..AgentConfig::default()
    }
}

pub(crate) fn build_preset(
    preset: Preset,
    provider: Provider,
    recs: &ProviderRecommendations,
    scope: InitScope,
    harnesses: &[HarnessIntegration],
) -> ResolvedPreset {
    match preset {
        Preset::Minimal => minimal_preset(provider, recs, scope),
        Preset::BatteriesIncluded => batteries_included_preset(provider, recs, scope, harnesses),
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
        mcps: vec![mcps(vec![("coding", vec![]), ("skills", skills_args(&[]))])],
        tools: ToolFilter {
            allow: vec![ToolMatcher::name("coding__bash"), ToolMatcher::name("skills__*")],
            deny: vec![],
        },
        ..AgentConfig::default()
    };

    ResolvedPreset {
        files: &[TemplateFile { path: SYSTEM_PATH, body: SYSTEM_MD }],
        settings: AetherSettings { prompts: default_prompts(scope), agents: vec![agent], ..AetherSettings::default() },
    }
}

fn batteries_included_preset(
    provider: Provider,
    recs: &ProviderRecommendations,
    scope: InitScope,
    harnesses: &[HarnessIntegration],
) -> ResolvedPreset {
    let display = provider.display_name();
    let plan = build_batteries_included_plan_agent(
        display,
        recs.plan.model.to_string(),
        recs.plan.reasoning_effort,
        harnesses,
    );

    let build = build_batteries_included_build_agent(
        display,
        recs.build.model.to_string(),
        recs.build.reasoning_effort,
        harnesses,
    );

    let explore = build_batteries_included_explore_agent(
        recs.explore.model.to_string(),
        recs.explore.reasoning_effort,
        scope,
        harnesses,
    );

    ResolvedPreset {
        files: &[
            TemplateFile { path: SYSTEM_PATH, body: SYSTEM_MD },
            TemplateFile { path: EXPLORER_AGENTS_PATH, body: EXPLORER_AGENTS_MD },
            TemplateFile { path: AETHER_SKILL_PATH, body: AETHER_SKILL_MD },
        ],
        settings: AetherSettings {
            prompts: batteries_prompts(scope, harnesses),
            agents: vec![plan, build, explore],
            ..AetherSettings::default()
        },
    }
}

fn skills_args(harnesses: &[HarnessIntegration]) -> Vec<String> {
    let mut args = vec!["--dir".to_string(), "${AETHER_HOME}/skills".to_string()];
    for harness in harnesses {
        for dir in harness.skills_dirs() {
            args.extend(["--dir".to_string(), dir]);
        }
    }

    args.extend(["--dir".to_string(), "${WORKSPACE}/.aether/skills".to_string()]);

    args
}

fn coding_args(harnesses: &[HarnessIntegration]) -> Vec<String> {
    let mut args = Vec::new();
    for harness in harnesses {
        for dir in harness.rules_dirs() {
            args.extend(["--rules-dir".to_string(), dir]);
        }
    }
    args
}

fn batteries_prompts(scope: InitScope, harnesses: &[HarnessIntegration]) -> Vec<PromptSource> {
    let mut prompts = vec![PromptSource::file(scope.asset_path(SYSTEM_PATH))];
    for harness in harnesses {
        if let Some(source) = harness.prompt_source() {
            prompts.push(source);
        }
    }

    prompts
}

fn default_prompts(scope: InitScope) -> Vec<PromptSource> {
    let mut prompts = vec![PromptSource::file(scope.asset_path(SYSTEM_PATH))];
    prompts.extend(HarnessIntegration::Agents.prompt_source());
    prompts
}

fn mcps(servers: Vec<(&str, Vec<String>)>) -> McpSourceSpec {
    let servers = servers
        .into_iter()
        .map(|(name, args)| {
            (
                name.to_string(),
                McpServerConfig::InMemory(InMemoryServerConfig {
                    type_: InMemoryType::InMemory,
                    args,
                    input: None,
                    defer_tools: ToolExposure::ModelVisible,
                }),
            )
        })
        .collect();
    McpSourceSpec::Inline { servers }
}

fn read_only_coding_tools() -> ToolFilter {
    ToolFilter {
        allow: vec![
            ToolMatcher::read_only(),
            ToolMatcher::name("plan__*"),
            ToolMatcher::name("skills__*"),
            ToolMatcher::name("subagents__*"),
            ToolMatcher::name("tasks__*"),
            ToolMatcher::name("survey__*"),
        ],
        deny: vec![],
    }
}
