use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests, select};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::schema::v2 as acp;
use gpui::{TestAppContext, px};
use utils::testing::TestWorkspace;

#[gpui::test]
fn up_and_down_walk_the_prompts_sent_before(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, replying("alpha-session", "ok"));
    harness.send(cx, &thread, "first");
    harness.send(cx, &thread, "second");

    harness.simulate_keystrokes(cx, "up");
    assert!(harness.shows(cx, "composer-text", "second"));
    harness.simulate_keystrokes(cx, "up");
    assert!(harness.shows(cx, "composer-text", "first"));
    harness.simulate_keystrokes(cx, "down");
    assert!(harness.shows(cx, "composer-text", "second"));
    harness.simulate_keystrokes(cx, "down");
    assert!(harness.shows(cx, "composer-text", ""));
}

#[gpui::test]
fn up_in_a_draft_moves_the_cursor_instead_of_recalling(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, replying("alpha-session", "ok"));
    harness.send(cx, &thread, "first");

    harness.simulate_input(cx, "draft");
    harness.simulate_keystrokes(cx, "up");

    assert!(harness.shows(cx, "composer-text", "draft"));
}

#[gpui::test]
fn a_slash_offers_the_agents_commands_and_sends_the_chosen_one(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, advertising_commands());
    harness.send(cx, &thread, "hi");

    harness.simulate_input(cx, "/rev");
    assert!(harness.bounds(cx, "completion-item-0").is_some(), "review matches");
    assert!(harness.bounds(cx, "completion-item-1").is_none(), "explain does not");
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| texts(thread.read(cx)).contains(&"/review".to_owned()));

    assert!(harness.shows(cx, "composer-text", ""));
}

#[gpui::test]
fn a_command_that_takes_input_waits_for_it(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, advertising_commands());
    harness.send(cx, &thread, "hi");

    harness.simulate_input(cx, "/expl");
    harness.simulate_keystrokes(cx, "tab");

    assert!(harness.shows(cx, "composer-text", "/explain "));
    assert!(harness.bounds(cx, "completion-menu").is_none());
}

#[gpui::test]
fn escape_closes_the_menu_until_the_token_ends(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarness::alpha(cx, advertising_commands());
    harness.send(cx, &thread, "hi");
    harness.simulate_input(cx, "/");
    assert!(harness.bounds(cx, "completion-menu").is_some());

    harness.simulate_keystrokes(cx, "escape");
    harness.simulate_input(cx, "r");

    assert!(harness.bounds(cx, "completion-menu").is_none());
    assert!(harness.shows(cx, "composer-text", "/r"));
}

#[gpui::test]
fn an_at_mention_sends_the_file_with_the_prompt(cx: &mut TestAppContext) {
    let workspace = TestWorkspace::new()
        .git_init()
        .file("src/lib.rs", "pub fn answer() -> u32 { 42 }\n")
        .file("README.md", "# alpha\n");
    let root = workspace.root();
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, _) = DesktopHarnessBuilder::default().agent(root, agent).start(cx, root);

    harness.simulate_input(cx, "explain @lib");
    harness.run_until_painted(cx, "completion-item-0");
    harness.simulate_keystrokes(cx, "enter");
    assert!(harness.shows(cx, "composer-text", "explain @src/lib.rs "));
    harness.simulate_keystrokes(cx, "enter");

    let prompt = harness.next(cx, &mut requests.prompt);
    let sent = format!("{:?}", prompt.prompt);
    assert!(sent.contains("explain @src/lib.rs"), "the prompt names the file: {sent}");
    assert!(sent.contains("pub fn answer() -> u32 { 42 }"), "the prompt carries the file: {sent}");
}

#[gpui::test]
fn the_option_shows_the_model_the_agent_switched_to(cx: &mut TestAppContext) {
    let (agent, mut requests) = agent_with_models(&["fast", "slow"], "fast");
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    assert!(harness.shows(cx, "config-model-value", "fast"));

    harness.click(cx, "config-model").unwrap();
    harness.click(cx, "config-model-picker-model-slow").unwrap();
    harness.next(cx, &mut requests.config);
    harness.run_until(cx, |cx| {
        let options = thread.read(cx).connected_core().map(|core| core.session().config_options());
        let model = options.and_then(|options| options.iter().find(|option| option.id == "model"));
        model.and_then(|model| model.select()).is_some_and(|model| model.current_value == "model-slow")
    });

    assert!(harness.shows(cx, "config-model-value", "slow"));
}

#[gpui::test]
fn the_sessions_options_sit_below_the_composer_in_the_chat_alone_led_by_their_categorys_icon(cx: &mut TestAppContext) {
    let option = |id: &str| select(id, id, "one", &[("one", "One")]);
    let options = vec![
        option("mode").category(acp::SessionConfigOptionCategory::Mode),
        option("model").category(acp::SessionConfigOptionCategory::Model),
        option("reasoning_effort").category(acp::SessionConfigOptionCategory::ThoughtLevel),
        option("verbosity"),
    ];
    let (harness, _) = DesktopHarness::alpha(cx, offering(options));

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
    let (model, composer) = (harness.bounds(cx, "config-model"), harness.bounds(cx, "composer"));
    assert!(model.zip(composer).is_some_and(|(model, composer)| model.top() >= composer.bottom()));

    harness.simulate_keystrokes(cx, "secondary-2");

    assert!(harness.bounds(cx, "config-model").is_none(), "the options leave with the composer");
}

#[gpui::test]
fn the_model_picker_opens_on_the_current_model(cx: &mut TestAppContext) {
    let (agent, mut requests) = agent_with_models(&["one", "two", "three"], "two");
    let (harness, _) = DesktopHarness::alpha(cx, agent);

    harness.click(cx, "config-model").unwrap();
    assert!(harness.bounds(cx, "config-model-picker-search").is_none(), "a short list needs no search");
    harness.simulate_keystrokes(cx, "down enter");

    let request = harness.next(cx, &mut requests.config);
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("model-three")));
}

#[gpui::test]
fn a_long_list_of_models_is_searchable_and_scrolls_inside_the_window(cx: &mut TestAppContext) {
    let names: Vec<String> = (0..80).map(|n| format!("m{n}")).collect();
    let (agent, _requests) = agent_with_models(&names.iter().map(String::as_str).collect::<Vec<_>>(), "m0");
    let (harness, _) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(1000.0, 600.0).start(cx, "/alpha");

    harness.click(cx, "config-model").unwrap();

    assert!(harness.bounds(cx, "config-model-picker-search").is_some());
    let picker = harness.bounds(cx, "config-model-picker").expect("the picker opens");
    assert!(picker.top() >= px(0.0), "the picker starts inside the window");
    assert!(picker.bottom() <= px(600.0), "the list scrolls instead of growing past the window");
}

#[gpui::test]
fn the_context_meter_shows_once_the_agent_reports_usage_and_its_counts_once_nearly_full(cx: &mut TestAppContext) {
    let usage = |used| acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(used, 200_000));
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    assert!(harness.bounds(cx, "context-meter").is_none(), "nothing is reported yet");

    let turn = harness.start_turn(cx, &thread, &mut requests, "hi");
    turn.update(usage(50_000));
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert!(harness.bounds(cx, "context-ring").is_some(), "the meter stays once the turn ends");
    let (meter, composer) = (harness.bounds(cx, "context-meter").unwrap(), harness.bounds(cx, "composer").unwrap());
    assert!(meter.top() >= composer.bottom(), "the meter is on the bar below the composer");
    assert!(harness.bounds(cx, "context-tokens").is_none(), "a quarter-full window leaves its counts to the tooltip");

    harness.start_turn(cx, &thread, &mut requests, "more").update(usage(160_000));
    harness.run_until_painted(cx, "context-tokens");
}

#[gpui::test]
fn a_prompt_the_agent_has_not_taken_up_shows_as_queued(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").hold::<acp::PromptRequest>().capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);

    harness.simulate_input(cx, "later");
    harness.simulate_keystrokes(cx, "enter");
    let (_, responder) = harness.next(cx, requests.held::<acp::PromptRequest>());
    assert!(harness.bounds(cx, "queued-prompts").is_some());

    responder.respond(acp::PromptResponse::new("user-message")).unwrap();
    harness.run_until(cx, |cx| thread.read(cx).core().is_some_and(|core| core.queued_prompts().is_empty()));
    assert!(harness.bounds(cx, "queued-prompts").is_none());
}

#[gpui::test]
fn prompt_search_puts_a_past_prompt_back_in_the_composer(cx: &mut TestAppContext) {
    let agent = replying("alpha-session", "ok").prompt_history(&["fix the build", "write the docs"]);
    let (harness, _) = DesktopHarness::alpha(cx, agent);

    harness.simulate_keystrokes(cx, "secondary-r");
    harness.simulate_input(cx, "build");
    harness.run_until_painted(cx, "prompt-search-result-0");
    assert!(harness.bounds(cx, "prompt-search-result-1").is_none(), "only one prompt matches");
    harness.simulate_keystrokes(cx, "enter");

    assert!(harness.bounds(cx, "prompt-search").is_none());
    assert!(harness.shows(cx, "composer-text", "fix the build"));
}

#[gpui::test]
fn prompt_search_is_offered_only_by_agents_that_support_it(cx: &mut TestAppContext) {
    let (harness, _) = DesktopHarness::alpha(cx, replying("alpha-session", "ok"));

    harness.simulate_keystrokes(cx, "secondary-r");

    assert!(harness.bounds(cx, "prompt-search").is_none());
}

#[gpui::test]
fn prompt_search_is_not_offered_once_the_agent_disconnects(cx: &mut TestAppContext) {
    let agent = replying("alpha-session", "ok").prompt_history(&["fix the build"]);
    let (harness, thread) = DesktopHarness::alpha(cx, agent);

    harness.connector().kill_agents("/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Disconnected);
    harness.simulate_keystrokes(cx, "secondary-r");

    assert!(harness.bounds(cx, "prompt-search").is_none(), "a disconnected agent cannot answer a search");
}

fn advertising_commands() -> FakeAgent {
    let explain = acp::AvailableCommand::new("explain", "Explain something")
        .input(acp::AvailableCommandInput::Text(acp::TextCommandInput::new("topic")));
    let commands =
        acp::AvailableCommandsUpdate::new(vec![acp::AvailableCommand::new("review", "Review the diff"), explain]);
    FakeAgent::new("alpha-session").prompt_updates(vec![acp::SessionUpdate::AvailableCommandsUpdate(commands)])
}

fn agent_with_models(names: &[&str], current: &str) -> (FakeAgent, FakeAgentRequests) {
    let values: Vec<_> = names.iter().map(|name| format!("model-{name}")).collect();
    let values: Vec<_> = values.iter().map(String::as_str).zip(names.iter().copied()).collect();
    offering(vec![select("model", "Model", &format!("model-{current}"), &values)]).capture()
}

fn offering(options: Vec<acp::SessionConfigOption>) -> FakeAgent {
    FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session").config_options(options))
}
