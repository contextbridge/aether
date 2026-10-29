use crate::support::{replying, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::DesktopHarnessBuilder;
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::schema::v2::SessionId;
use gpui::TestAppContext;

#[gpui::test]
fn concurrent_threads_each_receive_only_their_own_updates(cx: &mut TestAppContext) {
    let resumed = FakeAgent::default().replay_message("gamma-session", "earlier in gamma").prompt_reply("from gamma");
    let (harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .agent("/gamma", resumed)
        .start(cx, "/alpha");
    let beta = harness.start_thread(cx, "/beta");
    let gamma = harness
        .threads(cx)
        .update(cx, |threads, cx| threads.resume(SessionId::new("gamma-session"), "/gamma".into(), cx));
    let threads = [&alpha, &beta, &gamma];
    harness.run_until(cx, |cx| gamma.read(cx).status() == ThreadStatus::Idle);

    for (thread, prompt) in threads.iter().zip(["to alpha", "to beta", "to gamma"]) {
        thread.update(cx, |thread, cx| thread.send(prompt.into(), cx));
    }
    harness.run_until(cx, |cx| {
        threads
            .iter()
            .zip([2, 2, 3])
            .all(|(thread, len)| texts(thread.read(cx)).len() == len && thread.read(cx).status() == ThreadStatus::Idle)
    });

    cx.read(|cx| {
        assert_eq!(alpha.read(cx).session_id(), Some(&SessionId::new("alpha-session")));
        assert_eq!(texts(alpha.read(cx)), ["to alpha", "from alpha"]);
        assert_eq!(beta.read(cx).session_id(), Some(&SessionId::new("beta-session")));
        assert_eq!(texts(beta.read(cx)), ["to beta", "from beta"]);
        assert_eq!(gamma.read(cx).session_id(), Some(&SessionId::new("gamma-session")));
        assert_eq!(texts(gamma.read(cx)), ["earlier in gamma", "to gamma", "from gamma"]);
    });
}

#[gpui::test]
fn stopping_a_thread_ends_its_agent_and_returns_its_session_to_history(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarnessBuilder::default()
        .session("alpha-session", "/alpha", "2026-01-01T00:00:00Z", "hi")
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"))
        .start(cx, "/alpha");
    assert!(harness.bounds(cx, "history-row-alpha-session").is_none(), "the live thread stands in for its session");

    harness.click(cx, "thread-row-0-stop").unwrap();
    harness.run_until(cx, |_| harness.connector().live_agents("/alpha") == 0);

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.threads().is_empty()));
    assert!(harness.selected_thread(cx).is_none());
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::Dormant);
    assert!(harness.bounds(cx, "thread-row-0").is_none());
    assert!(harness.bounds(cx, "history-row-alpha-session").is_some(), "the session is back in history");

    harness.click(cx, "history-row-alpha-session").unwrap();
    let resumed = harness.connected_thread(cx);

    assert_eq!(resumed.read_with(cx, |thread, _| texts(thread)), ["replayed answer"]);
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
}

#[gpui::test]
fn stopping_one_thread_leaves_the_others_running(cx: &mut TestAppContext) {
    let (harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .start(cx, "/alpha");
    harness.start_thread(cx, "/beta");

    harness.click(cx, "thread-row-1-stop").unwrap();
    harness.run_until(cx, |_| harness.connector().live_agents("/beta") == 0);
    harness.submit(cx, &alpha, "still there?");

    assert_eq!(alpha.read_with(cx, |thread, _| texts(thread)), ["still there?", "from alpha"]);
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
}

#[gpui::test]
fn a_thread_whose_agent_cannot_start_is_errored_and_gives_a_prompt_back(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().project("/nowhere").build(cx);

    let thread = harness.new_thread(cx, "/nowhere");
    harness.run_until(cx, |cx| thread.read(cx).status() != ThreadStatus::Connecting);

    let (status, error) = thread.read_with(cx, |thread, _| (thread.status(), thread.error().map(str::to_owned)));
    assert_eq!(status, ThreadStatus::Errored);
    assert!(error.as_deref().is_some_and(|error| error.contains("no fake agent")), "{error:?}");
    assert!(harness.bounds(cx, "thread-row-0-status-errored-warning-triangle").is_some());
    assert!(harness.bounds(cx, "thread-row-0-conversation-icon").is_none());
    harness.simulate_input(cx, "hello?");
    harness.simulate_keystrokes(cx, "enter");
    assert!(harness.shows(cx, "composer-text", "hello?"), "a prompt the agent never received is kept");
}

#[gpui::test]
fn clicking_a_saved_session_resumes_it_in_a_selected_thread_that_replaces_its_row(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .session("saved-session", "/alpha", "2026-01-01T00:00:00Z", "earlier prompt")
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .build(cx);

    harness.click(cx, "history-row-saved-session").unwrap();
    let thread = harness.connected_thread(cx);

    assert_eq!(thread.read_with(cx, |thread, _| thread.session_id().cloned()), Some(SessionId::new("saved-session")));
    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["an earlier answer"]);
    assert!(harness.bounds(cx, "transcript-row-0").is_some(), "the replayed history is painted");
    assert!(harness.bounds(cx, "history-row-saved-session").is_none(), "the live thread replaces the saved row");
    assert!(harness.bounds(cx, "thread-row-0").is_some());
}

#[gpui::test]
fn resuming_a_session_that_is_already_open_selects_its_thread(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .agent("/beta", replying("beta-session", "hello"))
        .build(cx);
    let threads = harness.threads(cx);
    let resumed =
        threads.update(cx, |threads, cx| threads.resume(SessionId::new("saved-session"), "/alpha".into(), cx));
    harness.new_thread(cx, "/beta");

    let again = threads.update(cx, |threads, cx| threads.resume(SessionId::new("saved-session"), "/alpha".into(), cx));

    assert_eq!(again, resumed);
    assert_eq!(threads.read_with(cx, |threads, _| threads.threads().len()), 2);
    assert_eq!(harness.selected_thread(cx), Some(resumed));
}

#[gpui::test]
fn a_thread_whose_agent_died_reconnects_and_replays_its_session_without_duplicates(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"))
        .start(cx, "/alpha");
    harness.submit(cx, &thread, "hi");

    harness.connector().kill_agents("/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Disconnected);
    assert!(harness.bounds(cx, "thread-row-0-status-disconnected").is_some());

    harness.click(cx, "thread-row-0-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["replayed answer"]);
    assert_eq!(harness.selected_thread(cx), Some(thread));
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
}

#[gpui::test]
fn a_thread_whose_agent_could_not_start_reconnects_once_it_can(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    let thread = harness.new_thread(cx, "/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Errored);

    harness.connector().script("/alpha", replying("alpha-session", "hello"));
    harness.click(cx, "thread-row-0-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| thread.session_id().cloned()), Some(SessionId::new("alpha-session")));
}

#[gpui::test]
fn a_disconnected_thread_reconnects_from_its_transcript(cx: &mut TestAppContext) {
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"))
        .start(cx, "/alpha");
    harness.submit(cx, &thread, "hi");
    assert!(harness.bounds(cx, "transcript-reconnect").is_none(), "a connected thread offers no reconnect");

    harness.connector().kill_agents("/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Disconnected);
    harness.click(cx, "transcript-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert!(harness.bounds(cx, "transcript-reconnect").is_none());
}

#[gpui::test]
fn a_thread_that_could_not_start_offers_to_reconnect_from_its_transcript(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    let thread = harness.new_thread(cx, "/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Errored);

    harness.connector().script("/alpha", replying("alpha-session", "hello"));
    harness.click(cx, "transcript-reconnect").unwrap();

    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
}
