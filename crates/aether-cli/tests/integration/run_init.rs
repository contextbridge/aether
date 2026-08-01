use aether_cli::init::{HarnessIntegration, InitError, InitOutcome, InitTarget, OverwriteMode, Preset, apply_init};
use aether_core::agent_spec::ToolMatcher;
use aether_core::core::Prompt;
use aether_project::{AetherSettings, AgentCatalog, McpSourceSpec, PromptSource};
use llm::catalog::Provider;
use llm::{LlmModel, ReasoningEffort};
use mcp_servers::{CodingMcpArgs, PlanMcpArgs, SkillsMcpArgs, SubAgentsMcpArgs, TasksMcpArgs};
use mcp_utils::client::McpServerConfig;
use std::collections::BTreeMap;
use std::path::Path;

fn load(settings_path: &Path) -> AetherSettings {
    let content = std::fs::read_to_string(settings_path).expect("settings.json");
    AetherSettings::try_from(content.as_str()).expect("settings parses")
}

fn agent_provider(agent: &aether_project::AgentConfig) -> Provider {
    agent.model.parse::<LlmModel>().expect("model parses").provider_enum()
}

fn inline_args(servers: &BTreeMap<String, McpServerConfig>, name: &str) -> Vec<String> {
    let McpServerConfig::InMemory(in_memory) = servers.get(name).unwrap_or_else(|| panic!("{name} server")) else {
        panic!("expected in-memory {name}");
    };
    in_memory.args.clone()
}

fn inline_servers(spec: &McpSourceSpec) -> &BTreeMap<String, McpServerConfig> {
    let McpSourceSpec::Inline { servers } = spec else {
        panic!("expected inline MCPs, got a file spec");
    };
    servers
}

#[test]
fn writes_user_minimal_preset_for_codex() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = apply_init(
        InitTarget::user(dir.path()),
        Provider::Codex,
        Preset::Minimal,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

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
    let outcome = apply_init(
        InitTarget::project(dir.path()),
        Provider::Codex,
        Preset::Minimal,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

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
    let outcome = apply_init(
        InitTarget::project(dir.path()),
        Provider::Anthropic,
        Preset::BatteriesIncluded,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    assert!(dir.path().join(".aether/agents/codebase-explorer/AGENTS.md").is_file());

    let skill = dir.path().join(".aether/skills/aether/SKILL.md");
    assert!(skill.is_file(), "batteries-included should write the aether docs skill into a scanned skills dir");
    assert!(std::fs::read_to_string(&skill).unwrap().contains("name: aether"));

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

    let plan_servers = inline_servers(&plan.mcps[0]);
    let plan_server_names: Vec<&str> = plan_servers.keys().map(String::as_str).collect();
    assert_eq!(plan_server_names, vec!["coding", "plan", "skills", "subagents", "survey", "tasks"]);
    assert!(!plan.tools.deny.iter().any(|tool| matches!(tool, ToolMatcher::Name(name) if name.starts_with("plan__"))));

    let explore = &settings.agents[2];
    assert!(explore.agent_invocable);
    assert!(!explore.user_invocable);
    assert_eq!(explore.prompts[0].path(), Some(".aether/agents/codebase-explorer/AGENTS.md"));
    assert_read_only_coding_tools(explore);
    let explore_servers = inline_servers(&explore.mcps[0]);
    let server_names: Vec<&str> = explore_servers.keys().map(String::as_str).collect();
    assert_eq!(server_names, vec!["coding"]);
}

#[test]
fn batteries_included_harness_configurations() {
    for case in &harness_cases() {
        let dir = tempfile::tempdir().unwrap();
        apply_init(
            InitTarget::project(dir.path()),
            Provider::Anthropic,
            Preset::BatteriesIncluded,
            &case.harnesses,
            OverwriteMode::PreserveExisting,
        )
        .unwrap_or_else(|e| panic!("{}: apply_init failed: {e}", case.name));

        let settings = load(&dir.path().join(".aether/settings.json"));

        assert_eq!(settings.prompts.len(), case.expected_prompt_paths.len(), "{}: prompt count mismatch", case.name);
        for (i, expected) in case.expected_prompt_paths.iter().enumerate() {
            assert_eq!(settings.prompts[i].path(), Some(*expected), "{}: prompt[{i}] path mismatch", case.name);
        }

        let plan = &settings.agents[0];
        let servers = inline_servers(&plan.mcps[0]);

        if !case.skills_fragments.is_empty() {
            let args = inline_args(servers, "skills");
            assert_contains_all(&args, &case.skills_fragments, &format!("{}: skills args", case.name));
        }

        if !case.rules_fragments.is_empty() {
            let args = inline_args(servers, "coding");
            assert_contains_all(&args, &case.rules_fragments, &format!("{}: rules args", case.name));
        }

        for fragment in &case.excluded_fragments {
            let skills = inline_args(servers, "skills");
            let coding = inline_args(servers, "coding");
            assert_does_not_contain(&skills, fragment, &format!("{}: skills should exclude", case.name));
            assert_does_not_contain(&coding, fragment, &format!("{}: rules should exclude", case.name));
        }
    }
}

#[test]
fn batteries_included_harness_skills_dirs_are_ordered_before_project_skills() {
    let dir = tempfile::tempdir().unwrap();
    apply_init(
        InitTarget::project(dir.path()),
        Provider::Anthropic,
        Preset::BatteriesIncluded,
        &[HarnessIntegration::Agents],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

    let settings = load(&dir.path().join(".aether/settings.json"));
    let plan = &settings.agents[0];
    let servers = inline_servers(&plan.mcps[0]);
    let skills_args = inline_args(servers, "skills");

    let aether_home_pos = skills_args.iter().position(|a| a == "${AETHER_HOME}/skills").unwrap();
    let home_agents_pos = skills_args.iter().position(|a| a == "${HOME}/.agents/skills").unwrap();
    let workspace_agents_pos = skills_args.iter().position(|a| a == "${WORKSPACE}/.agents/skills").unwrap();
    let project_skills_pos = skills_args.iter().position(|a| a == "${WORKSPACE}/.aether/skills").unwrap();

    assert!(aether_home_pos < home_agents_pos, "AETHER_HOME skills should come before harness user skills");
    assert!(home_agents_pos < workspace_agents_pos, "harness user skills should come before harness project skills");
    assert!(
        workspace_agents_pos < project_skills_pos,
        "harness project skills should come before native project skills"
    );
}

#[test]
fn refuses_to_overwrite_existing_user_settings_without_force() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), "{}").unwrap();

    let outcome = apply_init(
        InitTarget::user(dir.path()),
        Provider::Codex,
        Preset::Minimal,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

    assert!(matches!(outcome, InitOutcome::AlreadyInitialized { .. }), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join("settings.json")).unwrap(), "{}");
}

#[test]
fn refuses_to_overwrite_existing_project_settings_without_force() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".aether")).unwrap();
    std::fs::write(dir.path().join(".aether/settings.json"), "{}").unwrap();

    let outcome = apply_init(
        InitTarget::project(dir.path()),
        Provider::Codex,
        Preset::Minimal,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect("apply_init");

    assert!(matches!(outcome, InitOutcome::AlreadyInitialized { .. }), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap(), "{}");
}

#[test]
fn force_overwrites_selected_target_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("settings.json"), "{}").unwrap();
    std::fs::create_dir_all(dir.path().join(".aether")).unwrap();
    std::fs::write(dir.path().join(".aether/settings.json"), "{}").unwrap();

    let outcome = apply_init(
        InitTarget::user(dir.path()),
        Provider::Anthropic,
        Preset::Minimal,
        &[],
        OverwriteMode::OverwriteExisting,
    )
    .expect("apply_init");

    assert!(matches!(outcome, InitOutcome::Applied { .. }), "{outcome:?}");
    let settings = load(&dir.path().join("settings.json"));
    assert_eq!(agent_provider(&settings.agents[0]), Provider::Anthropic);
    assert_eq!(std::fs::read_to_string(dir.path().join(".aether/settings.json")).unwrap(), "{}");
}

#[test]
fn every_inline_mcp_in_init_presets_parses_its_args() {
    for preset in [Preset::Minimal, Preset::BatteriesIncluded] {
        for harnesses in
            [vec![], vec![HarnessIntegration::Claude], vec![HarnessIntegration::Claude, HarnessIntegration::Agents]]
        {
            let dir = tempfile::tempdir().unwrap();
            apply_init(
                InitTarget::user(dir.path()),
                Provider::Anthropic,
                preset,
                &harnesses,
                OverwriteMode::PreserveExisting,
            )
            .expect("apply_init");
            let settings = load(&dir.path().join("settings.json"));

            for agent in &settings.agents {
                for mcp in &agent.mcps {
                    let McpSourceSpec::Inline { servers } = mcp else { continue };
                    for (name, config) in servers {
                        let McpServerConfig::InMemory(in_memory) = config else { continue };
                        let args = in_memory.args.clone();
                        let parse_result = match name.as_str() {
                            "coding" => CodingMcpArgs::from_args(args).map(|_| ()),
                            "skills" => SkillsMcpArgs::from_args(args).map(|_| ()),
                            "subagents" => SubAgentsMcpArgs::from_args(args).map(|_| ()),
                            "tasks" => TasksMcpArgs::from_args(args).map(|_| ()),
                            "plan" => PlanMcpArgs::from_args(args).map(|_| ()),
                            "survey" => Ok(()),
                            other => {
                                panic!("preset references unknown in-memory MCP `{other}`; add a parse check")
                            }
                        };
                        assert!(
                            parse_result.is_ok(),
                            "{preset:?} agent `{}` server `{name}` harnesses={harnesses:?} args failed to parse: {parse_result:?}",
                            agent.name
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unsupported_provider_returns_error_without_writing_files() {
    let dir = tempfile::tempdir().unwrap();
    let err = apply_init(
        InitTarget::user(dir.path()),
        Provider::Gemini,
        Preset::Minimal,
        &[],
        OverwriteMode::PreserveExisting,
    )
    .expect_err("Gemini has no preset");

    assert!(matches!(err, InitError::UnsupportedProvider { provider: Provider::Gemini, .. }), "{err:?}");
    assert!(!dir.path().join("settings.json").exists(), "no settings.json should be written");
    assert!(!dir.path().join("SYSTEM.md").exists(), "no SYSTEM.md should be written");
}

fn assert_read_only_coding_tools(agent: &aether_project::AgentConfig) {
    assert_eq!(
        agent.tools.allow,
        vec![
            ToolMatcher::read_only(),
            ToolMatcher::name("plan__*"),
            ToolMatcher::name("skills__*"),
            ToolMatcher::name("subagents__*"),
            ToolMatcher::name("tasks__*"),
            ToolMatcher::name("survey__*"),
        ]
    );
    assert!(agent.tools.deny.is_empty());
}

fn assert_minimal_mcp_and_tools(agent: &aether_project::AgentConfig) {
    let McpSourceSpec::Inline { servers } = &agent.mcps[0] else { panic!("minimal MCPs should be inline") };
    let names: Vec<&str> = servers.keys().map(String::as_str).collect();
    assert_eq!(names, vec!["coding", "skills"]);
    assert_eq!(agent.tools.allow, vec![ToolMatcher::name("coding__bash"), ToolMatcher::name("skills__*")]);
    assert!(agent.tools.deny.is_empty());
}

fn assert_contains_all(args: &[String], expected: &[&str], context: &str) {
    for expected_item in expected {
        assert!(
            args.iter().any(|a| a == expected_item),
            "{context}: expected args to contain `{expected_item}`, got: {args:?}"
        );
    }
}

fn assert_does_not_contain(args: &[String], fragment: &str, context: &str) {
    assert!(
        !args.iter().any(|a| a.contains(fragment)),
        "{context}: expected args not to contain `{fragment}`, got: {args:?}"
    );
}

struct HarnessCase {
    name: &'static str,
    harnesses: Vec<HarnessIntegration>,
    expected_prompt_paths: Vec<&'static str>,
    skills_fragments: Vec<&'static str>,
    rules_fragments: Vec<&'static str>,
    excluded_fragments: Vec<&'static str>,
}

fn harness_cases() -> Vec<HarnessCase> {
    vec![
        HarnessCase {
            name: "no harnesses",
            harnesses: vec![],
            expected_prompt_paths: vec![".aether/SYSTEM.md"],
            skills_fragments: vec![],
            rules_fragments: vec![],
            excluded_fragments: vec![],
        },
        HarnessCase {
            name: "claude only",
            harnesses: vec![HarnessIntegration::Claude],
            expected_prompt_paths: vec![".aether/SYSTEM.md", "${WORKSPACE}/CLAUDE.md"],
            skills_fragments: vec!["--dir", "${HOME}/.claude/skills", "--dir", "${WORKSPACE}/.claude/skills"],
            rules_fragments: vec!["--rules-dir", "${HOME}/.claude/rules", "--rules-dir", "${WORKSPACE}/.claude/rules"],
            excluded_fragments: vec![".agents/"],
        },
        HarnessCase {
            name: "agents only",
            harnesses: vec![HarnessIntegration::Agents],
            expected_prompt_paths: vec![".aether/SYSTEM.md", "${WORKSPACE}/AGENTS.md"],
            skills_fragments: vec!["--dir", "${HOME}/.agents/skills", "--dir", "${WORKSPACE}/.agents/skills"],
            rules_fragments: vec!["--rules-dir", "${HOME}/.agents/rules", "--rules-dir", "${WORKSPACE}/.agents/rules"],
            excluded_fragments: vec![".claude/"],
        },
        HarnessCase {
            name: "both harnesses",
            harnesses: vec![HarnessIntegration::Claude, HarnessIntegration::Agents],
            expected_prompt_paths: vec![".aether/SYSTEM.md", "${WORKSPACE}/CLAUDE.md", "${WORKSPACE}/AGENTS.md"],
            skills_fragments: vec![
                "--dir",
                "${HOME}/.claude/skills",
                "--dir",
                "${WORKSPACE}/.claude/skills",
                "--dir",
                "${HOME}/.agents/skills",
                "--dir",
                "${WORKSPACE}/.agents/skills",
            ],
            rules_fragments: vec![
                "--rules-dir",
                "${HOME}/.claude/rules",
                "--rules-dir",
                "${WORKSPACE}/.claude/rules",
                "--rules-dir",
                "${HOME}/.agents/rules",
                "--rules-dir",
                "${WORKSPACE}/.agents/rules",
            ],
            excluded_fragments: vec![],
        },
    ]
}
