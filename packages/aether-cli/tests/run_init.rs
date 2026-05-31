use aether_cli::init::{InitError, InitOutcome, InitTarget, Preset, apply_init};
use aether_core::core::Prompt;
use aether_project::{AetherSettings, AgentCatalog, McpSourceSpec, PromptSource};
use llm::catalog::Provider;
use llm::{LlmModel, ReasoningEffort};
use mcp_servers::{CodingMcpArgs, PlanMcpArgs, SkillsMcpArgs, SubAgentsMcpArgs, TasksMcpArgs};
use mcp_utils::client::McpServerConfig;
use std::path::Path;

fn load(settings_path: &Path) -> AetherSettings {
    let content = std::fs::read_to_string(settings_path).expect("settings.json");
    AetherSettings::try_from(content.as_str()).expect("settings parses")
}

fn agent_provider(agent: &aether_project::AgentConfig) -> Provider {
    agent.model.parse::<LlmModel>().expect("model parses").provider_enum()
}

#[test]
fn writes_user_minimal_preset_for_codex() {
    let dir = tempfile::tempdir().unwrap();
    let outcome =
        apply_init(InitTarget::user(dir.path()), Provider::Codex, Preset::Minimal, false).expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    assert!(dir.path().join("settings.json").is_file());
    assert!(dir.path().join("SYSTEM.md").is_file());
    assert!(
        !dir.path().join("agents/codebase-explorer/AGENTS.md").exists(),
        "minimal should not write explorer assets"
    );

    let settings = load(&dir.path().join("settings.json"));
    assert_eq!(settings.prompts[0].path(), Some("SYSTEM.md"));
    assert_eq!(settings.prompts[1], PromptSource::file("${WORKSPACE}/AGENTS.md").optional());
    assert_eq!(settings.agents.len(), 1);
    let plan = &settings.agents[0];
    assert_eq!(plan.name, "Default");
    assert_eq!(agent_provider(plan), Provider::Codex);
    assert_eq!(plan.reasoning_effort, Some(ReasoningEffort::Xhigh));
    assert_minimal_mcp_and_tools(plan);
}

#[test]
fn writes_project_minimal_preset_for_codex() {
    let dir = tempfile::tempdir().unwrap();
    let outcome =
        apply_init(InitTarget::project(dir.path()), Provider::Codex, Preset::Minimal, false).expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    assert!(dir.path().join(".aether/settings.json").is_file());
    assert!(dir.path().join(".aether/SYSTEM.md").is_file());

    let settings = load(&dir.path().join(".aether/settings.json"));
    assert_eq!(settings.prompts[0].path(), Some(".aether/SYSTEM.md"));
    assert_eq!(settings.prompts[1], PromptSource::file("${WORKSPACE}/AGENTS.md").optional());
    assert_minimal_mcp_and_tools(&settings.agents[0]);

    std::fs::write(dir.path().join("AGENTS.md"), "Project instructions").unwrap();
    let catalog = AgentCatalog::from_settings(dir.path(), settings).expect("catalog resolves");
    let prompts = &catalog.default_agent().unwrap().prompts;
    let prompt_path = match &prompts[0] {
        Prompt::File { path, .. } => path,
        other => panic!("expected file prompt, got {other:?}"),
    };
    assert_eq!(prompt_path, &dir.path().join(".aether/SYSTEM.md"));
    let agents_prompt_path = match &prompts[1] {
        Prompt::File { path, .. } => path,
        other => panic!("expected AGENTS.md file prompt, got {other:?}"),
    };
    assert_eq!(agents_prompt_path, &dir.path().join("AGENTS.md"));
}

#[test]
fn writes_project_batteries_preset_for_anthropic() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = apply_init(InitTarget::project(dir.path()), Provider::Anthropic, Preset::BatteriesIncluded, false)
        .expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    assert!(dir.path().join(".aether/agents/codebase-explorer/AGENTS.md").is_file());

    let settings = load(&dir.path().join(".aether/settings.json"));
    let names: Vec<&str> = settings.agents.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Plan", "Build", "Explore"]);

    for agent in &settings.agents {
        assert_eq!(agent_provider(agent), Provider::Anthropic);
    }
    assert_eq!(settings.agents[0].reasoning_effort, Some(ReasoningEffort::High));
    assert_eq!(settings.agents[1].reasoning_effort, Some(ReasoningEffort::High));

    let plan = &settings.agents[0];
    assert_read_only_coding_tools(plan);

    let explore = &settings.agents[2];
    assert!(explore.agent_invocable);
    assert!(!explore.user_invocable);
    assert_eq!(explore.prompts[0].path(), Some(".aether/agents/codebase-explorer/AGENTS.md"));
    assert_read_only_coding_tools(explore);
    let McpSourceSpec::Inline { servers } = &explore.mcps[0] else { panic!("explore MCPs should be inline") };
    let server_names: Vec<&str> = servers.keys().map(String::as_str).collect();
    assert_eq!(server_names, vec!["coding"]);
}

#[test]
fn refuses_to_overwrite_existing_user_settings_without_force() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), "{}").unwrap();

    let outcome =
        apply_init(InitTarget::user(dir.path()), Provider::Codex, Preset::Minimal, false).expect("apply_init");

    assert!(matches!(outcome, InitOutcome::AlreadyInitialized { .. }), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.json")).unwrap(), "{}");
}

#[test]
fn refuses_to_overwrite_existing_project_settings_without_force() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aether")).unwrap();
    std::fs::write(dir.path().join(".aether/settings.json"), "{}").unwrap();

    let outcome =
        apply_init(InitTarget::project(dir.path()), Provider::Codex, Preset::Minimal, false).expect("apply_init");

    assert!(matches!(outcome, InitOutcome::AlreadyInitialized { .. }), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap(), "{}");
}

#[test]
fn force_overwrites_selected_target_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), "{}").unwrap();
    std::fs::create_dir_all(dir.path().join(".aether")).unwrap();
    std::fs::write(dir.path().join(".aether/settings.json"), "{}").unwrap();

    let outcome =
        apply_init(InitTarget::user(dir.path()), Provider::Anthropic, Preset::Minimal, true).expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    let settings = load(&dir.path().join("settings.json"));
    assert_eq!(agent_provider(&settings.agents[0]), Provider::Anthropic);
    assert_eq!(std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap(), "{}");
}

#[test]
fn every_inline_mcp_in_init_presets_parses_its_args() {
    for preset in [Preset::Minimal, Preset::BatteriesIncluded] {
        let dir = tempfile::tempdir().unwrap();
        apply_init(InitTarget::user(dir.path()), Provider::Anthropic, preset, false).expect("apply_init");
        let settings = load(&dir.path().join("settings.json"));

        for agent in &settings.agents {
            for mcp in &agent.mcps {
                let McpSourceSpec::Inline { servers } = mcp else { continue };
                for (name, config) in servers {
                    let McpServerConfig::InMemory(in_memory) = config else { continue };
                    let args = in_memory.args.clone();
                    if name == "skills" {
                        assert_eq!(
                            args,
                            vec![
                                "--dir",
                                "${AETHER_HOME}/skills",
                                "--dir",
                                "${WORKSPACE}/.aether/skills",
                                "--notes-dir",
                                "${WORKSPACE}/.aether/notes",
                            ],
                            "skills MCP should read user and project skill directories"
                        );
                    }
                    let parse_result = match name.as_str() {
                        "coding" => CodingMcpArgs::from_args(args).map(|_| ()),
                        "skills" => SkillsMcpArgs::from_args(args).map(|_| ()),
                        "subagents" => SubAgentsMcpArgs::from_args(args).map(|_| ()),
                        "tasks" => TasksMcpArgs::from_args(args).map(|_| ()),
                        "plan" => PlanMcpArgs::from_args(args).map(|_| ()),
                        "survey" => Ok(()),
                        other => panic!("preset references unknown in-memory MCP `{other}`; add a parse check"),
                    };
                    assert!(
                        parse_result.is_ok(),
                        "{preset:?} agent `{}` server `{name}` args failed to parse: {parse_result:?}",
                        agent.name
                    );
                }
            }
        }
    }
}

#[test]
fn unsupported_provider_returns_error_without_writing_files() {
    let dir = tempfile::tempdir().unwrap();
    let err = apply_init(InitTarget::user(dir.path()), Provider::Gemini, Preset::Minimal, false)
        .expect_err("Gemini has no preset");

    assert!(matches!(err, InitError::UnsupportedProvider { provider: Provider::Gemini, .. }), "{err:?}");
    assert!(!dir.path().join("settings.json").exists(), "no settings.json should be written");
    assert!(!dir.path().join("SYSTEM.md").exists(), "no SYSTEM.md should be written");
}

fn assert_read_only_coding_tools(agent: &aether_project::AgentConfig) {
    assert_eq!(agent.tools.allow, Vec::<String>::new());
    assert_eq!(agent.tools.deny, vec!["coding__bash", "coding__edit_file", "coding__lsp_rename", "coding__write_file"]);
}

fn assert_minimal_mcp_and_tools(agent: &aether_project::AgentConfig) {
    let McpSourceSpec::Inline { servers } = &agent.mcps[0] else { panic!("minimal MCPs should be inline") };
    let names: Vec<&str> = servers.keys().map(String::as_str).collect();
    assert_eq!(names, vec!["coding", "skills"]);
    assert_eq!(agent.tools.allow, vec!["coding__bash", "skills__*"]);
    assert!(agent.tools.deny.is_empty());
}
