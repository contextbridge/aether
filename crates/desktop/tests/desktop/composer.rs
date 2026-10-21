use crate::support::{replying, start_thread, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2 as acp;
use gpui::{Entity, TestAppContext};
use std::fs;

#[gpui::test]
fn up_and_down_walk_the_prompts_sent_before(cx: &mut TestAppContext) {
    let harness = harness_with(cx, replying("alpha-session", "ok"));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "first");
    send(&harness, cx, &thread, "second");

    harness.simulate_keystrokes(cx, "up");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("second"));
    harness.simulate_keystrokes(cx, "up");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("first"));
    harness.simulate_keystrokes(cx, "down");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("second"));
    harness.simulate_keystrokes(cx, "down");
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
}

#[gpui::test]
fn up_in_a_draft_moves_the_cursor_instead_of_recalling(cx: &mut TestAppContext) {
    let harness = harness_with(cx, replying("alpha-session", "ok"));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "first");

    harness.simulate_input(cx, "draft");
    harness.simulate_keystrokes(cx, "up");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("draft"));
}

#[gpui::test]
fn a_slash_offers_the_agents_commands_and_sends_the_chosen_one(cx: &mut TestAppContext) {
    let harness = harness_with(cx, advertising_commands());
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "hi");

    harness.simulate_input(cx, "/rev");
    assert!(harness.bounds(cx, "completion-item-0").is_some(), "review matches");
    assert!(harness.bounds(cx, "completion-item-1").is_none(), "explain does not");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).contains(&"/review".to_owned()));

    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
}

#[gpui::test]
fn a_command_that_takes_input_waits_for_it(cx: &mut TestAppContext) {
    let harness = harness_with(cx, advertising_commands());
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "hi");

    harness.simulate_input(cx, "/expl");
    harness.simulate_keystrokes(cx, "tab");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("/explain "));
    assert!(harness.bounds(cx, "completion-menu").is_none());
}

#[gpui::test]
fn escape_closes_the_menu_until_the_token_ends(cx: &mut TestAppContext) {
    let harness = harness_with(cx, advertising_commands());
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "hi");
    harness.simulate_input(cx, "/");
    assert!(harness.bounds(cx, "completion-menu").is_some());

    harness.simulate_keystrokes(cx, "escape");
    harness.simulate_input(cx, "r");

    assert!(harness.bounds(cx, "completion-menu").is_none());
    assert_eq!(harness.composer_text(cx).as_deref(), Some("/r"));
}

#[gpui::test]
fn an_at_mention_sends_the_file_with_the_prompt(cx: &mut TestAppContext) {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().join("alpha");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
    fs::write(root.join("README.md"), "# alpha\n").unwrap();
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session")).capture();
    let connector = FakeConnector::default().agent(&root, agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).project(&root).build(cx);
    start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_input(cx, "explain @lib");
    harness.run_until_painted(cx, "completion-item-0");
    harness.simulate_keystrokes(cx, "enter");
    assert_eq!(harness.composer_text(cx).as_deref(), Some("explain @src/lib.rs "));
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());

    let (prompt, _) = requests.prompt.try_recv().unwrap();
    let sent = format!("{:?}", prompt.prompt);
    assert!(sent.contains("explain @src/lib.rs"), "the prompt names the file: {sent}");
    assert!(sent.contains("pub fn answer() -> u32 { 42 }"), "the prompt carries the file: {sent}");
}

#[gpui::test]
fn choosing_a_model_asks_the_agent_to_switch(cx: &mut TestAppContext) {
    let model = acp::SessionConfigOption::select(
        "model",
        "Model",
        "fast".to_owned(),
        vec![acp::SessionConfigSelectOption::new("fast", "Fast"), acp::SessionConfigSelectOption::new("slow", "Slow")],
    );
    let (agent, mut requests) = FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session").config_options(vec![model]))
        .capture();
    let harness = harness_with(cx, agent);
    start_thread(&harness, cx, "project-alpha-new-thread");

    harness.click(cx, "config-model").unwrap();
    harness.click(cx, "config-model-slow").unwrap();
    harness.run_until(cx, |_| !requests.config.is_empty());

    let request = requests.config.try_recv().unwrap();
    assert_eq!(request.config_id.0.as_ref(), "model");
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("slow")));
}

#[gpui::test]
fn the_context_meter_shows_once_the_agent_reports_usage(cx: &mut TestAppContext) {
    let usage = acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(50_000, 200_000));
    let agent = FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session"))
        .prompt_updates(vec![usage]);
    let harness = harness_with(cx, agent);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    assert!(harness.bounds(cx, "context-meter").is_none(), "nothing is reported yet");

    send(&harness, cx, &thread, "hi");

    assert!(harness.bounds(cx, "context-meter").is_some());
}

#[gpui::test]
fn a_prompt_the_agent_has_not_taken_up_shows_as_queued(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session")).capture();
    let harness = harness_with(cx, agent);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_input(cx, "later");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.prompt.is_empty());
    assert!(harness.bounds(cx, "queued-prompts").is_some());

    let (_, responder) = requests.prompt.try_recv().unwrap();
    responder.respond(acp::PromptResponse::new("user-message")).unwrap();
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| core.queued_prompts().is_empty()));
    assert!(harness.bounds(cx, "queued-prompts").is_none());
}

#[gpui::test]
fn prompt_search_puts_a_past_prompt_back_in_the_composer(cx: &mut TestAppContext) {
    let agent = replying("alpha-session", "ok").prompt_history(&["fix the build", "write the docs"]);
    let harness = harness_with(cx, agent);
    start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_keystrokes(cx, "secondary-r");
    harness.simulate_input(cx, "build");
    harness.run_until_painted(cx, "prompt-search-result-0");
    assert!(harness.bounds(cx, "prompt-search-result-1").is_none(), "only one prompt matches");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "prompt-search").is_none());
    assert_eq!(harness.composer_text(cx).as_deref(), Some("fix the build"));
}

#[gpui::test]
fn prompt_search_is_offered_only_by_agents_that_support_it(cx: &mut TestAppContext) {
    let harness = harness_with(cx, replying("alpha-session", "ok"));
    start_thread(&harness, cx, "project-alpha-new-thread");

    harness.simulate_keystrokes(cx, "secondary-r");

    assert!(harness.bounds(cx, "prompt-search").is_none());
}

/// An agent that advertises `/review`, which takes no input, and `/explain`, which does, during
/// its first turn.
fn advertising_commands() -> FakeAgent {
    let explain = acp::AvailableCommand::new("explain", "Explain something")
        .input(acp::AvailableCommandInput::Text(acp::TextCommandInput::new("topic")));
    let commands =
        acp::AvailableCommandsUpdate::new(vec![acp::AvailableCommand::new("review", "Review the diff"), explain]);
    FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session"))
        .prompt_updates(vec![acp::SessionUpdate::AvailableCommandsUpdate(commands)])
}

fn harness_with(cx: &mut TestAppContext, agent: FakeAgent) -> DesktopHarness {
    let connector = FakeConnector::default().agent("/alpha", agent);
    DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx)
}

/// Sends `prompt` through the composer and waits for the agent's turn on it to end.
fn send(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
    let sent = thread.read_with(cx, |thread, _| texts(thread).iter().filter(|text| *text == prompt).count()) + 1;
    harness.simulate_input(cx, prompt);
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| {
        let thread = thread.read(cx);
        thread.status() == ThreadStatus::Idle && texts(thread).iter().filter(|text| *text == prompt).count() == sent
    });
}
