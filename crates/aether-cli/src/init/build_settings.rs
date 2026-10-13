use super::InitScope;
use super::harness::HarnessIntegration;
use super::recommendations::{ModelWithReasoning, ProviderRecommendations, recommended_for_provider};
use aether_core::mcp::{InMemoryServerConfig, InMemoryType, McpConfig, McpServerConfig};
use aether_core::mcp::{ToolExposure, ToolFilter, ToolMatcher};
use aether_project::{AetherSettings, AgentConfig, McpSourceSpec, PromptSource};
use llm::{ReasoningEffort, catalog::Provider};

const SYSTEM_PATH: &str = "SYSTEM.md";
const SYSTEM_MD: &str = include_str!("templates/SYSTEM.md");

const EXPLORER_AGENTS_MD: &str = include_str!("templates/agents/codebase-explorer/AGENTS.md");
const EXPLORER_AGENTS_PATH: &str = "agents/codebase-explorer/AGENTS.md";

const AETHER_SKILL_MD: &str = include_str!("templates/skills/aether/SKILL.md");
const AETHER_SKILL_PATH: &str = "skills/aether/SKILL.md";
const PLAN_SKILL_MD: &str = include_str!("templates/skills/plan.md");
const PLAN_SKILL_PATH: &str = "skills/plan.md";

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
    let choice = (model, reasoning_effort);
    batteries_settings(display, [choice.clone(), choice.clone(), choice], scope, harnesses)
}

fn batteries_settings(
    display: &str,
    [plan, build, explore]: [ModelChoice; 3],
    scope: InitScope,
    harnesses: &[HarnessIntegration],
) -> AetherSettings {
    let plan_description = format!("{display} planner (may write plan artifacts and request review)");
    AetherSettings {
        prompts: batteries_prompts(scope, harnesses),
        agents: vec![
            primary_agent("Plan", plan_description, plan, harnesses, plan_tools()),
            primary_agent("Build", format!("{display} implementor"), build, harnesses, ToolFilter::default()),
            explore_agent(explore, scope, harnesses),
        ],
        ..AetherSettings::default()
    }
}

type ModelChoice = (String, Option<ReasoningEffort>);

fn primary_agent(
    name: &str,
    description: String,
    (model, reasoning_effort): ModelChoice,
    harnesses: &[HarnessIntegration],
    tools: ToolFilter,
) -> AgentConfig {
    AgentConfig {
        name: name.to_string(),
        description,
        model,
        reasoning_effort,
        user_invocable: true,
        mcps: vec![mcps(vec![
            ("coding", coding_args(harnesses)),
            ("skills", skills_args(harnesses)),
            ("subagents", vec![]),
            ("tasks", vec![]),
            ("review", vec![]),
        ])],
        tools,
        ..AgentConfig::default()
    }
}

fn explore_agent(
    (model, reasoning_effort): ModelChoice,
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
    let choice = |rec: &ModelWithReasoning| (rec.model.to_string(), rec.reasoning_effort);
    let choices = [choice(&recs.plan), choice(&recs.build), choice(&recs.explore)];
    ResolvedPreset {
        files: &[
            TemplateFile { path: SYSTEM_PATH, body: SYSTEM_MD },
            TemplateFile { path: EXPLORER_AGENTS_PATH, body: EXPLORER_AGENTS_MD },
            TemplateFile { path: AETHER_SKILL_PATH, body: AETHER_SKILL_MD },
            TemplateFile { path: PLAN_SKILL_PATH, body: PLAN_SKILL_MD },
        ],
        settings: batteries_settings(provider.display_name(), choices, scope, harnesses),
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
                    defer_tools: ToolExposure::ModelVisible,
                }),
            )
        })
        .collect();
    McpSourceSpec::Inline(McpConfig::new(servers))
}

fn plan_tools() -> ToolFilter {
    let mut tools = read_only_coding_tools();
    tools.allow.extend([
        ToolMatcher::name("coding__write_file"),
        ToolMatcher::name("coding__edit_file"),
        ToolMatcher::name("review__review_artifact"),
    ]);
    tools
}

fn read_only_coding_tools() -> ToolFilter {
    ToolFilter {
        allow: vec![
            ToolMatcher::read_only(),
            ToolMatcher::name("skills__*"),
            ToolMatcher::name("subagents__*"),
            ToolMatcher::name("tasks__*"),
        ],
        deny: vec![],
    }
}
