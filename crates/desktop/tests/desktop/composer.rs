use crate::support::Edges;
use crate::support::{replying, texts};
use acp_utils::testing::{FakeAgent, FakeAgentRequests, select};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::v2::schema as acp;
use utils::testing::TestWorkspace;

#[test]
fn up_and_down_walk_the_prompts_sent_before() {
    let (mut harness, thread) = DesktopHarness::alpha(replying("alpha-session", "ok"));
    harness.send(thread, "first");
    harness.send(thread, "second");

    harness.press("up");
    assert!(harness.shows("composer-text", "second"));
    harness.press("up");
    assert!(harness.shows("composer-text", "first"));
    harness.press("down");
    assert!(harness.shows("composer-text", "second"));
    harness.press("down");
    assert!(harness.shows("composer-text", ""));
}

#[test]
fn up_in_a_draft_moves_the_cursor_instead_of_recalling() {
    let (mut harness, thread) = DesktopHarness::alpha(replying("alpha-session", "ok"));
    harness.send(thread, "first");

    harness.type_text("draft");
    harness.press("up");

    assert!(harness.shows("composer-text", "draft"));
}

#[test]
fn a_slash_offers_the_agents_commands_and_sends_the_chosen_one() {
    let (mut harness, thread) = DesktopHarness::alpha(advertising_commands());
    harness.send(thread, "hi");

    harness.type_text("/rev");
    assert!(harness.bounds("completion-item-0").is_some(), "review matches");
    assert!(harness.bounds("completion-item-1").is_none(), "explain does not");
    harness.press("enter");
    harness.run_until(|_| texts(thread).contains(&"/review".to_owned()));

    assert!(harness.shows("composer-text", ""));
}

#[test]
fn a_command_that_takes_input_waits_for_it() {
    let (mut harness, thread) = DesktopHarness::alpha(advertising_commands());
    harness.send(thread, "hi");

    harness.type_text("/expl");
    harness.press("tab");

    assert!(harness.shows("composer-text", "/explain "));
    assert!(harness.bounds("completion-menu").is_none());
}

#[test]
fn escape_closes_the_menu_until_the_token_ends() {
    let (mut harness, thread) = DesktopHarness::alpha(advertising_commands());
    harness.send(thread, "hi");
    harness.type_text("/");
    assert!(harness.bounds("completion-menu").is_some());

    harness.press("escape");
    harness.type_text("r");

    assert!(harness.bounds("completion-menu").is_none());
    assert!(harness.shows("composer-text", "/r"));
}

#[test]
fn an_at_mention_sends_the_file_with_the_prompt() {
    let workspace = TestWorkspace::new()
        .git_init()
        .file("src/lib.rs", "pub fn answer() -> u32 { 42 }\n")
        .file("README.md", "# alpha\n");
    let root = workspace.root();
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, _) = DesktopHarnessBuilder::default().agent(root, agent).start(root);

    harness.type_text("explain @lib");
    harness.run_until_painted("completion-item-0");
    harness.press("enter");
    assert!(harness.shows("composer-text", "explain @src/lib.rs "));
    harness.press("enter");

    let prompt = harness.next(&mut requests.prompt);
    let sent = format!("{:?}", prompt.prompt);
    assert!(sent.contains("explain @src/lib.rs"), "the prompt names the file: {sent}");
    assert!(sent.contains("pub fn answer() -> u32 { 42 }"), "the prompt carries the file: {sent}");
}

#[test]
fn the_option_shows_the_model_the_agent_switched_to() {
    let (agent, mut requests) = agent_with_models(&["fast", "slow"], "fast");
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    assert!(harness.shows("config-model-value", "fast"));

    harness.click("config-model").unwrap();
    harness.click("config-model-picker-model-slow").unwrap();
    harness.next(&mut requests.config);
    harness.run_until(|_| {
        thread
            .with_connected_core(|core| {
                let options = core.session().config_options();
                let model = options.iter().find(|option| option.id == "model");
                model.and_then(|model| model.select()).is_some_and(|model| model.current_value == "model-slow")
            })
            .unwrap_or(false)
    });

    assert!(harness.shows("config-model-value", "slow"));
}

#[test]
fn the_sessions_options_sit_below_the_composer_in_the_chat_alone_led_by_their_categorys_icon() {
    let option = |id: &str| select(id, id, "one", &[("one", "One")]);
    let options = vec![
        option("mode").category(acp::SessionConfigOptionCategory::Mode),
        option("model").category(acp::SessionConfigOptionCategory::Model),
        option("reasoning_effort").category(acp::SessionConfigOptionCategory::ThoughtLevel),
        option("verbosity"),
    ];
    let (mut harness, _) = DesktopHarness::alpha(offering(options));

    assert!(harness.bounds("config-mode-icon-bot").is_some());
    assert!(harness.bounds("config-model-icon-cpu").is_some());
    assert!(harness.bounds("config-reasoning_effort-icon-brain").is_some());
    assert!(harness.bounds("config-verbosity").is_some());
    assert!(
        ["config-verbosity-icon-bot", "config-verbosity-icon-cpu", "config-verbosity-icon-brain"]
            .iter()
            .all(|icon| harness.bounds(icon).is_none()),
        "an option of no known category has no icon"
    );
    let (model, composer) = (harness.bounds("config-model"), harness.bounds("composer"));
    assert!(model.zip(composer).is_some_and(|(model, composer)| model.top() >= composer.bottom()));

    harness.press("secondary-2");

    assert!(harness.bounds("config-model").is_none(), "the options leave with the composer");
}

#[test]
fn the_model_picker_opens_on_the_current_model() {
    let (agent, mut requests) = agent_with_models(&["one", "two", "three"], "two");
    let (mut harness, _) = DesktopHarness::alpha(agent);

    harness.click("config-model").unwrap();
    assert!(harness.bounds("picker-query").is_none(), "a short list needs no search");
    harness.press("down enter");

    let request = harness.next(&mut requests.config);
    assert_eq!(format!("{:?}", request.value), format!("{:?}", acp::SessionConfigOptionValue::from("model-three")));
}

#[test]
fn a_long_list_of_models_is_searchable_and_scrolls_inside_the_window() {
    let names: Vec<String> = (0..80).map(|n| format!("m{n}")).collect();
    let (agent, _requests) = agent_with_models(&names.iter().map(String::as_str).collect::<Vec<_>>(), "m0");
    let (mut harness, _) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(1000.0, 600.0).start("/alpha");

    harness.click("config-model").unwrap();

    assert!(harness.bounds("picker-query").is_some());
    let picker = harness.bounds("config-model-picker").expect("the picker opens");
    assert!(picker.top() >= 0.0, "the picker starts inside the window");
    assert!(picker.bottom() <= 600.0, "the list scrolls instead of growing past the window");
}

#[test]
fn the_context_meter_shows_once_the_agent_reports_usage_and_its_counts_once_nearly_full() {
    let usage = |used| acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(used, 200_000));
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    assert!(harness.bounds("context-meter").is_none(), "nothing is reported yet");

    let turn = harness.start_turn(thread, &mut requests, "hi");
    turn.update(usage(50_000));
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);

    assert!(harness.bounds("context-ring").is_some(), "the meter stays once the turn ends");
    let (meter, composer) = (harness.bounds("context-meter").unwrap(), harness.bounds("composer").unwrap());
    assert!(meter.top() >= composer.bottom(), "the meter is on the bar below the composer");
    assert!(harness.bounds("context-tokens").is_none(), "a quarter-full window leaves its counts to the tooltip");

    harness.start_turn(thread, &mut requests, "more").update(usage(160_000));
    harness.run_until_painted("context-tokens");
}

#[test]
fn a_prompt_the_agent_has_not_taken_up_shows_as_queued() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").hold::<acp::PromptRequest>().capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);

    harness.type_text("later");
    harness.press("enter");
    let (_, responder) = harness.next(requests.held::<acp::PromptRequest>());
    assert!(harness.bounds("queued-prompts").is_some());

    responder.respond(acp::PromptResponse::new("user-message")).unwrap();
    harness.run_until(|_| thread.with_core(|core| core.queued_prompts().is_empty()).unwrap_or(false));
    assert!(harness.bounds("queued-prompts").is_none());
}

#[test]
fn prompt_search_puts_a_past_prompt_back_in_the_composer() {
    let agent = replying("alpha-session", "ok").prompt_history(&["fix the build", "write the docs"]);
    let (mut harness, _) = DesktopHarness::alpha(agent);

    harness.press("secondary-r");
    harness.type_text("build");
    harness.run_until_painted("prompt-search-result-0");
    assert!(harness.bounds("prompt-search-result-1").is_none(), "only one prompt matches");
    harness.press("enter");

    assert!(harness.bounds("prompt-search").is_none());
    assert!(harness.shows("composer-text", "fix the build"));
}

#[test]
fn prompt_search_is_offered_only_by_agents_that_support_it() {
    let (mut harness, _) = DesktopHarness::alpha(replying("alpha-session", "ok"));

    harness.press("secondary-r");

    assert!(harness.bounds("prompt-search").is_none());
}

#[test]
fn prompt_search_is_not_offered_once_the_agent_disconnects() {
    let agent = replying("alpha-session", "ok").prompt_history(&["fix the build"]);
    let (mut harness, thread) = DesktopHarness::alpha(agent);

    harness.connector().kill_agents("/alpha");
    harness.run_until(|_| thread.status() == ThreadStatus::Disconnected);
    harness.press("secondary-r");

    assert!(harness.bounds("prompt-search").is_none(), "a disconnected agent cannot answer a search");
}

fn advertising_commands() -> FakeAgent {
    let explain = acp::Command::new("explain", "Explain something")
        .input(acp::AvailableCommandInput::Text(acp::TextCommandInput::new("topic")));
    let review = acp::Command::new("review", "Review the diff");
    let commands = acp::AvailableCommandsUpdate::new(vec![
        acp::AvailableCommand::Command(review),
        acp::AvailableCommand::Command(explain),
    ]);
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
