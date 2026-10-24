use crate::support::{replying, write_agents_file};
use aether_desktop::agents::{
    AgentCatalog, AgentCommand, AgentTransport, Agents, Endpoint, EndpointSource, project_agents_file,
};
use aether_desktop::target::InstanceId;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use gpui::{AppContext, Entity, TestAppContext};
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
        AgentTransport::Stdio(AgentCommand::new("aether").args(["acp"]))
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
        AgentTransport::Stdio(AgentCommand::new("aether-dev").args(["acp"]))
    );
    assert_eq!(
        catalog.get("claude").unwrap().config.transport,
        AgentTransport::Stdio(AgentCommand::new("project-claude"))
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
                "diff": { "endpoint": { "command": "./diff-endpoint.sh", "args": ["${INSTANCE_ID}"] } },
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
        Some(EndpointSource::Command(AgentCommand::new("./diff-endpoint.sh").args(["${INSTANCE_ID}"])))
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
        json!({ "type": "websocket", "url": "ws://host", "launches": command }),
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

#[gpui::test]
fn edits_to_an_agents_file_show_once_the_agents_are_reloaded(cx: &mut TestAppContext) {
    let project = tempfile::tempdir().unwrap();
    let agents = cx.new(|_| agents(None));
    let before = agents.read_with(cx, |agents, _| names(&agents.catalog(project.path())));
    write_agents_file(project.path(), &json!({ "agents": { "claude": { "type": "stdio", "command": "claude" } } }));
    let unreloaded = agents.read_with(cx, |agents, _| names(&agents.catalog(project.path())));

    agents.update(cx, Agents::reload);

    assert_eq!(before, ["aether"]);
    assert_eq!(unreloaded, ["aether"], "a catalog is read once");
    assert_eq!(agents.read_with(cx, |agents, _| names(&agents.catalog(project.path()))), ["aether", "claude"]);
}

#[gpui::test]
fn a_project_with_only_the_builtin_agent_starts_a_thread_with_it_at_once(cx: &mut TestAppContext) {
    let project = Project::new(&json!({}));
    let harness = project.harness(FakeConnector::default().agent(&project.root, replying("s1", "hi")), cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();

    assert_eq!(agent_of(&harness.selected_thread(cx).unwrap(), cx), "aether");
}

#[gpui::test]
fn a_project_with_several_agents_starts_a_thread_with_the_one_chosen_from_its_menu(cx: &mut TestAppContext) {
    let project = Project::new(&json!({ "claude": { "type": "stdio", "command": "claude-code-acp" } }));
    let harness = project.harness(FakeConnector::default().agent(&project.root, replying("s1", "hi")), cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();
    harness.click(cx, "new-thread-agent-claude").unwrap();

    let thread = harness.selected_thread(cx).unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert_eq!(agent_of(&thread, cx), "claude");
}

#[gpui::test]
fn the_new_thread_shortcut_starts_the_agent_last_chosen_in_the_project(cx: &mut TestAppContext) {
    let project = Project::new(&json!({ "claude": { "type": "stdio", "command": "claude-code-acp" } }));
    let connector =
        FakeConnector::default().agent(&project.root, replying("s1", "hi")).agent(&project.root, replying("s2", "hi"));
    let harness = project.harness(connector, cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    harness.click(cx, "new-thread-agent-claude").unwrap();
    let first = harness.selected_thread(cx).unwrap();
    harness.run_until(cx, |cx| first.read(cx).status() == ThreadStatus::Idle);

    harness.simulate_keystrokes(cx, "secondary-n");

    let second = harness.selected_thread(cx).unwrap();
    assert_ne!(first.entity_id(), second.entity_id());
    assert_eq!(agent_of(&second, cx), "claude");
}

#[gpui::test]
fn an_agents_file_that_cannot_be_read_is_offered_for_fixing_in_the_new_thread_menu(cx: &mut TestAppContext) {
    let project = Project::new(&json!({}));
    fs::write(project_agents_file(&project.root), "{ not json").unwrap();
    let harness = project.harness(FakeConnector::default(), cx);

    harness.click(cx, "project-alpha-new-thread").unwrap();

    assert!(harness.bounds(cx, "new-thread-agent-aether").is_some());
    assert!(harness.bounds(cx, "new-thread-problem-0").is_some());
}

#[gpui::test]
fn a_launched_instance_is_reused_on_reconnect_and_terminated_when_its_thread_stops(cx: &mut TestAppContext) {
    let command = json!({ "command": "clankervm" });
    let project = Project::new(&json!({
        "vm": { "type": "websocket", "launch": command, "endpoint": command, "terminate": command },
    }));
    let connector =
        FakeConnector::default().agent(&project.root, replying("s1", "hi")).agent(&project.root, replying("s1", "hi"));
    let harness = project.harness(connector.clone(), cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    harness.click(cx, "new-thread-agent-vm").unwrap();
    let thread = harness.selected_thread(cx).unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    let launched = connector.instances();

    connector.kill_agents(&project.root);
    harness.run_until(cx, |cx| thread.read(cx).status().can_reconnect());
    harness.threads(cx).update(cx, |threads, cx| threads.reconnect(&thread, cx));
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    let reconnected = connector.instances();
    harness.threads(cx).update(cx, |threads, cx| threads.stop(&thread, cx));
    harness.run_until(cx, |_| connector.instances().is_empty());

    assert_eq!(launched, [InstanceId::new("instance-0")]);
    assert_eq!(reconnected, launched, "the reconnected thread is on the same instance");
    assert_eq!(connector.launches(), 1);
}

/// A project named `alpha` on disk, whose `acp.json` defines `agents`.
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

    fn harness(&self, connector: FakeConnector, cx: &mut TestAppContext) -> DesktopHarness {
        DesktopHarnessBuilder::default().project(&self.root).connector(connector).build(cx)
    }
}

fn agents(user_file: Option<PathBuf>) -> Agents {
    Agents::new(AgentCommand::new("aether").args(["acp"]), user_file)
}

fn names(catalog: &AgentCatalog) -> Vec<String> {
    catalog.agents().iter().map(|agent| agent.name.clone()).collect()
}

fn agent_of(thread: &Entity<Thread>, cx: &TestAppContext) -> String {
    thread.read_with(cx, |thread, _| thread.target().agent.name.clone())
}
