use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::Thread;
use agent_client_protocol::schema::v2 as acp;
use gpui::{Entity, TestAppContext, px};
use std::fs;

#[gpui::test]
fn up_and_down_walk_the_prompts_sent_before(cx: &mut TestAppContext) {
    let (harness, thread) = harness_with(cx, replying("alpha-session", "ok"));
    harness.send(cx, &thread, "first");
    harness.send(cx, &thread, "second");

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
    let (harness, thread) = harness_with(cx, replying("alpha-session", "ok"));
    harness.send(cx, &thread, "first");

    harness.simulate_input(cx, "draft");
    harness.simulate_keystrokes(cx, "up");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("draft"));
}

#[gpui::test]
fn a_slash_offers_the_agents_commands_and_sends_the_chosen_one(cx: &mut TestAppContext) {
    let (harness, thread) = harness_with(cx, advertising_commands());
    harness.send(cx, &thread, "hi");

    harness.simulate_input(cx, "/rev");
    assert!(harness.bounds(cx, "completion-item-0").is_some(), "review matches");
    assert!(harness.bounds(cx, "completion-item-1").is_none(), "explain does not");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).contains(&"/review".to_owned()));

    assert_eq!(harness.composer_text(cx).as_deref(), Some(""));
}

#[gpui::test]
fn a_command_that_takes_input_waits_for_it(cx: &mut TestAppContext) {
    let (harness, thread) = harness_with(cx, advertising_commands());
    harness.send(cx, &thread, "hi");

    harness.simulate_input(cx, "/expl");
    harness.simulate_keystrokes(cx, "tab");

    assert_eq!(harness.composer_text(cx).as_deref(), Some("/explain "));
    assert!(harness.bounds(cx, "completion-menu").is_none());
}

#[gpui::test]
fn escape_closes_the_menu_until_the_token_ends(cx: &mut TestAppContext) {
    let (harness, thread) = harness_with(cx, advertising_commands());
    harness.send(cx, &thread, "hi");
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
    let harness = DesktopHarnessBuilder::default().agent(&root, agent).build(cx);
    harness.start_thread(cx, &root);

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
    let (harness, _) = harness_with(cx, agent);

    harness.click(cx, "config-model").unwrap();
    harness.click(cx, "config-model-picker-slow").unwrap();
    harness.run_until(cx, |_| !requests.config.is_empty());

    let request = requests.config.try_recv().unwrap();
    assert_eq!(request.config_id.0.as_ref(), "model");
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("slow")));
}

#[gpui::test]
fn each_picker_leads_with_the_icon_of_its_category(cx: &mut TestAppContext) {
    let option = |id: &str, category: Option<acp::SessionConfigOptionCategory>| {
        let option = acp::SessionConfigOption::select(
            id.to_owned(),
            id.to_owned(),
            "one".to_owned(),
            vec![acp::SessionConfigSelectOption::new("one", "One")],
        );
        match category {
            Some(category) => option.category(category),
            None => option,
        }
    };
    let options = vec![
        option("mode", Some(acp::SessionConfigOptionCategory::Mode)),
        option("model", Some(acp::SessionConfigOptionCategory::Model)),
        option("reasoning_effort", Some(acp::SessionConfigOptionCategory::ThoughtLevel)),
        option("verbosity", None),
    ];
    let agent = FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session").config_options(options));
    let (harness, _) = harness_with(cx, agent);

    assert!(harness.bounds(cx, "config-mode-icon-bot").is_some());
    assert!(harness.bounds(cx, "config-model-icon-cpu").is_some());
    assert!(harness.bounds(cx, "config-reasoning_effort-icon-brain").is_some());
    assert!(harness.bounds(cx, "config-verbosity").is_some());
    assert!(
        ["config-verbosity-icon-bot", "config-verbosity-icon-cpu", "config-verbosity-icon-brain"]
            .iter()
            .all(|icon| harness.bounds(cx, icon).is_none()),
        "an option of no known category has no icon"
    );
}

#[gpui::test]
fn typing_in_the_model_picker_filters_the_models_and_enter_chooses_one(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        agent_with_models(&["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota"]);
    let (harness, _) = harness_with(cx, agent);

    harness.click(cx, "config-model").unwrap();
    assert!(harness.bounds(cx, "config-model-picker-search").is_some(), "a long list can be searched");
    harness.simulate_input(cx, "thet");

    assert!(harness.bounds(cx, "config-model-picker-model-theta").is_some());
    assert!(harness.bounds(cx, "config-model-picker-model-alpha").is_none(), "models not matching are hidden");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |_| !requests.config.is_empty());

    let request = requests.config.try_recv().unwrap();
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("model-theta")));
    assert!(harness.bounds(cx, "config-model-picker").is_none(), "choosing a model closes the picker");
}

#[gpui::test]
fn the_arrow_keys_move_the_pickers_highlight_from_the_current_value(cx: &mut TestAppContext) {
    let (agent, mut requests) = agent_with_models(&["one", "two", "three"]);
    let (harness, _) = harness_with(cx, agent);

    harness.click(cx, "config-model").unwrap();
    harness.simulate_keystrokes(cx, "down down enter");
    harness.run_until(cx, |_| !requests.config.is_empty());

    let request = requests.config.try_recv().unwrap();
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("model-three")));
}

#[gpui::test]
fn space_types_into_the_pickers_search_instead_of_closing_it(cx: &mut TestAppContext) {
    let (agent, _requests) =
        agent_with_models(&["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota"]);
    let (harness, _) = harness_with(cx, agent);

    harness.click(cx, "config-model").unwrap();
    harness.simulate_keystrokes(cx, "t space");

    assert!(harness.bounds(cx, "config-model-picker").is_some(), "the picker stays open");
    assert_eq!(harness.composer_text(cx).as_deref(), Some(""), "nothing reaches the composer");
}

#[gpui::test]
fn space_leaves_a_short_picker_open_and_escape_closes_it_without_switching(cx: &mut TestAppContext) {
    let (agent, requests) = agent_with_models(&["one", "two"]);
    let (harness, _) = harness_with(cx, agent);

    harness.click(cx, "config-model").unwrap();
    assert!(harness.bounds(cx, "config-model-picker-search").is_none(), "a short list needs no search");
    harness.simulate_keystrokes(cx, "space");
    assert!(harness.bounds(cx, "config-model-picker").is_some());
    harness.simulate_keystrokes(cx, "escape");

    assert!(harness.bounds(cx, "config-model-picker").is_none());
    assert!(requests.config.is_empty());
}

#[gpui::test]
fn a_long_list_of_models_scrolls_inside_the_window(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..80).map(|n| format!("m{n}")).collect();
    let (agent, _requests) = agent_with_models(&names.iter().map(String::as_str).collect::<Vec<_>>());
    let harness = DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(1000.0, 600.0).build(cx);
    harness.start_thread(cx, "/alpha");

    harness.click(cx, "config-model").unwrap();

    let picker = harness.bounds(cx, "config-model-picker").expect("the picker opens");
    assert!(picker.top() >= px(0.0), "the picker starts inside the window");
    assert!(picker.bottom() <= px(600.0), "the list scrolls instead of growing past the window");
}

#[gpui::test]
fn the_context_meter_shows_once_the_agent_reports_usage(cx: &mut TestAppContext) {
    let usage = acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(50_000, 200_000));
    let agent = FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session"))
        .prompt_updates(vec![usage]);
    let (harness, thread) = harness_with(cx, agent);
    assert!(harness.bounds(cx, "context-meter").is_none(), "nothing is reported yet");

    harness.send(cx, &thread, "hi");

    assert!(harness.bounds(cx, "context-ring").is_some());
}

#[gpui::test]
fn a_prompt_the_agent_has_not_taken_up_shows_as_queued(cx: &mut TestAppContext) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session")).capture();
    let (harness, thread) = harness_with(cx, agent);

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
    let (harness, _) = harness_with(cx, agent);

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
    let (harness, _) = harness_with(cx, replying("alpha-session", "ok"));

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

/// An agent whose session offers a `model` option with a `model-<name>` value per name, the
/// first current.
fn agent_with_models(names: &[&str]) -> (FakeAgent, FakeAgentRequests) {
    let values: Vec<_> =
        names.iter().map(|name| acp::SessionConfigSelectOption::new(format!("model-{name}"), *name)).collect();
    let model = acp::SessionConfigOption::select("model", "Model", format!("model-{}", names[0]), values);
    FakeAgent::default()
        .new_session_response(acp::NewSessionResponse::new("alpha-session").config_options(vec![model]))
        .capture()
}

/// A connected thread in `/alpha` talking to `agent`.
fn harness_with(cx: &mut TestAppContext, agent: FakeAgent) -> (DesktopHarness, Entity<Thread>) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", agent).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    (harness, thread)
}
