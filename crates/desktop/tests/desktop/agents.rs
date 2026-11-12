use crate::support::{replying, write_agents_file};
use aether_desktop::agents::{
    AgentCatalog, AgentTransport, Agents, CommandLine, Endpoint, EndpointSource, project_agents_file,
};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use utils::fs::JsonFileError;

#[test]
fn a_project_offers_the_builtin_agent_beside_those_its_file_defines() {
    let project = tempfile::tempdir().unwrap();
    write_agents_file(
        project.path(),
        &json!({ "agents": {
            "zed": { "type": "stdio", "command": "zed-agent" },
            "claude": { "type": "stdio", "command": "npx", "args": ["-y", "@zed-industries/claude-code-acp"] },
        }}),
    );

    let catalog = agents(None).catalog(project.path());

    assert_eq!(names(&catalog), ["aether", "claude", "zed"]);
    assert_eq!(
        catalog.default_agent().config.transport,
        AgentTransport::Stdio(CommandLine::new("aether").args(["acp"]))
    );
    assert_eq!(catalog.get("claude").unwrap().workspace, project.path());
}

#[test]
fn a_project_agent_replaces_the_users_of_the_same_name_which_replaces_the_builtin_one() {
    let (home, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let user_file = home.path().join("acp.json");
    fs::write(
        &user_file,
        json!({ "agents": {
            "aether": { "type": "stdio", "command": "aether-dev", "args": ["acp"] },
            "claude": { "type": "stdio", "command": "user-claude" },
        }})
        .to_string(),
    )
    .unwrap();
    write_agents_file(
        project.path(),
        &json!({ "agents": { "claude": { "type": "stdio", "command": "project-claude" } } }),
    );

    let catalog = agents(Some(user_file)).catalog(project.path());

    assert_eq!(names(&catalog), ["aether", "claude"]);
    assert_eq!(
        catalog.default_agent().config.transport,
        AgentTransport::Stdio(CommandLine::new("aether-dev").args(["acp"]))
    );
    assert_eq!(
        catalog.get("claude").unwrap().config.transport,
        AgentTransport::Stdio(CommandLine::new("project-claude"))
    );
}

#[test]
fn an_agent_of_either_type_may_say_where_its_working_tree_is_diffed() {
    let project = tempfile::tempdir().unwrap();
    write_agents_file(
        project.path(),
        &json!({ "agents": {
            "claude": { "type": "stdio", "command": "claude-code-acp" },
            "claude (ssh)": {
                "type": "stdio",
                "command": "ssh",
                "diff": { "url": "ws://localhost:7331/ws", "headers": { "Authorization": "Bearer ${TOKEN}" } },
            },
            "codex (VM)": {
                "type": "websocket",
                "url": "wss://vm:8765",
                "diff": { "endpoint": { "command": "./diff-endpoint.sh", "args": ["vm"] } },
            },
        }}),
    );

    let catalog = agents(None).catalog(project.path());

    assert!(catalog.problems().is_empty());
    assert_eq!(catalog.get("claude").unwrap().config.diff, None);
    assert_eq!(
        catalog.get("claude (ssh)").unwrap().config.diff,
        Some(EndpointSource::Fixed(Endpoint {
            url: "ws://localhost:7331/ws".to_owned(),
            headers: [("Authorization".to_owned(), "Bearer ${TOKEN}".to_owned())].into(),
        }))
    );
    assert_eq!(
        catalog.get("codex (VM)").unwrap().config.diff,
        Some(EndpointSource::Command(CommandLine::new("./diff-endpoint.sh").args(["vm"])))
    );
}

#[test]
fn a_file_that_cannot_be_read_is_reported_and_its_agents_left_out() {
    let project = tempfile::tempdir().unwrap();
    let file = project_agents_file(project.path());
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "{ not json").unwrap();

    let catalog = agents(None).catalog(project.path());

    assert_eq!(names(&catalog), ["aether"]);
    assert_eq!(catalog.problems().iter().map(JsonFileError::path).collect::<Vec<_>>(), [file.as_path()]);
}

#[test]
fn an_agent_whose_fields_do_not_go_together_is_reported() {
    let command = json!({ "command": "true" });
    let invalid = [
        json!({ "type": "websocket" }),
        json!({ "type": "websocket", "url": "ws://host", "endpoint": command }),
        json!({ "type": "websocket", "endpoint": command, "headers": { "Authorization": "token" } }),
        json!({ "type": "websocket", "url": "ws://host", "terminate": command }),
        json!({ "type": "websocket", "endpoint": command, "launch": command }),
        json!({ "type": "stdio", "commnd": "aether" }),
        json!({ "type": "carrier-pigeon", "command": "aether" }),
        json!({ "type": "stdio", "command": "claude", "diff": {} }),
        json!({ "type": "stdio", "command": "claude", "diff": { "url": "ws://host", "endpoint": command } }),
        json!({ "type": "stdio", "command": "claude", "diff": { "endpoint": command, "headers": { "Authorization": "token" } } }),
        json!({ "type": "stdio", "command": "claude", "diff": { "uri": "ws://host" } }),
    ];

    for config in invalid {
        let project = tempfile::tempdir().unwrap();
        write_agents_file(project.path(), &json!({ "agents": { "broken": config } }));

        let catalog = agents(None).catalog(project.path());

        assert!(catalog.get("broken").is_none(), "{config} is left out");
        assert_eq!(catalog.problems().len(), 1, "{config} is reported");
    }
}

#[test]
fn edits_to_an_agents_file_show_once_the_agents_are_reloaded() {
    let project = tempfile::tempdir().unwrap();
    let mut agents = agents(None);
    let before = names(&agents.catalog(project.path()));
    write_agents_file(project.path(), &json!({ "agents": { "claude": { "type": "stdio", "command": "claude" } } }));
    let unreloaded = names(&agents.catalog(project.path()));

    agents.reload();

    assert_eq!(before, ["aether"]);
    assert_eq!(unreloaded, ["aether"], "a catalog is read once");
    assert_eq!(names(&agents.catalog(project.path())), ["aether", "claude"]);
}

#[test]
fn a_project_with_only_the_builtin_agent_starts_a_thread_with_it_at_once() {
    let project = Project::new(&json!({}));
    let mut harness = project.harness(&["s1"]);

    harness.click("project-alpha-new-thread").unwrap();

    assert_eq!(agent_of(harness.selected_thread().unwrap()), "aether");
}

#[test]
fn a_project_with_several_agents_starts_a_thread_with_the_one_chosen_from_its_menu() {
    let project = Project::new(&json!({ "claude": { "type": "stdio", "command": "claude-code-acp" } }));
    let mut harness = project.harness(&["s1"]);

    harness.click("project-alpha-new-thread").unwrap();
    harness.click("new-thread-agent-claude").unwrap();

    let thread = harness.connected_thread();
    assert_eq!(agent_of(thread), "claude");
}

#[test]
fn the_new_thread_shortcut_starts_the_agent_last_chosen_in_the_project() {
    let project = Project::new(&json!({ "claude": { "type": "stdio", "command": "claude-code-acp" } }));
    let mut harness = project.harness(&["s1", "s2"]);
    harness.click("project-alpha-new-thread").unwrap();
    harness.click("new-thread-agent-claude").unwrap();
    let first = harness.connected_thread();

    harness.press("secondary-n");

    let second = harness.selected_thread().unwrap();
    assert_ne!(first, second);
    assert_eq!(agent_of(second), "claude");
}

#[test]
fn an_agents_file_that_cannot_be_read_is_offered_for_fixing_in_the_new_thread_menu() {
    let project = Project::new(&json!({}));
    fs::write(project_agents_file(&project.root), "{ not json").unwrap();
    let mut harness = project.harness(&[]);

    harness.click("project-alpha-new-thread").unwrap();

    assert!(harness.bounds("new-thread-agent-aether").is_some());
    assert!(harness.bounds("new-thread-problem-0").is_some());
}

struct Project {
    _dir: TempDir,
    root: PathBuf,
}

impl Project {
    fn new(agents: &serde_json::Value) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("alpha");
        fs::create_dir(&root).unwrap();
        write_agents_file(&root, &json!({ "agents": agents }));
        Self { _dir: dir, root }
    }

    fn harness(&self, sessions: &[&str]) -> DesktopHarness {
        let builder = DesktopHarnessBuilder::default().project(&self.root);
        sessions.iter().fold(builder, |builder, session| builder.script(&self.root, replying(session, "hi"))).build()
    }
}

fn agents(user_file: Option<PathBuf>) -> Agents {
    Agents::new(CommandLine::new("aether").args(["acp"]), user_file)
}

fn names(catalog: &AgentCatalog) -> Vec<String> {
    catalog.agents().iter().map(|agent| agent.name.clone()).collect()
}

fn agent_of(thread: Thread) -> String {
    thread.target().label().to_owned()
}
