use crate::support::{replying, save_session, texts, wait_for_history};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::TestStore;
use agent_client_protocol::schema::v2::SessionId;
use gpui::{Entity, TestAppContext};

#[gpui::test]
fn a_new_thread_shows_the_agents_reply_to_a_prompt(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello from alpha"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let thread = new_thread(&harness, cx, "/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    thread.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["hi", "hello from alpha"]);
}

#[gpui::test]
fn concurrent_threads_each_receive_only_their_own_updates(cx: &mut TestAppContext) {
    let resumed = FakeAgent::default().replay_message("gamma-session", "earlier in gamma").prompt_reply("from gamma");
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .agent("/gamma", resumed);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let alpha = new_thread(&harness, cx, "/alpha");
    let beta = new_thread(&harness, cx, "/beta");
    let gamma = harness
        .threads(cx)
        .update(cx, |threads, cx| threads.resume(SessionId::new("gamma-session"), "/gamma".into(), cx));
    let threads = [&alpha, &beta, &gamma];
    harness.run_until(cx, |cx| threads.iter().all(|thread| thread.read(cx).status() == ThreadStatus::Idle));

    alpha.update(cx, |thread, cx| thread.submit("to alpha", cx));
    beta.update(cx, |thread, cx| thread.submit("to beta", cx));
    gamma.update(cx, |thread, cx| thread.submit("to gamma", cx));
    harness.run_until(cx, |cx| {
        texts(alpha.read(cx)).len() == 2
            && texts(beta.read(cx)).len() == 2
            && texts(gamma.read(cx)).len() == 3
            && threads.iter().all(|thread| thread.read(cx).status() == ThreadStatus::Idle)
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
    let sessions = TestStore::new();
    save_session(&sessions, "alpha-session", "/alpha", "2026-01-01T00:00:00Z", Some("hi"));
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"));
    let harness =
        DesktopHarnessBuilder::default().connector(connector.clone()).history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);
    let thread = new_thread(&harness, cx, "/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert!(harness.bounds(cx, "history-row-alpha-session").is_none(), "the live thread stands in for its session");

    harness.click(cx, "thread-row-0-stop").unwrap();
    harness.run_until(cx, |_| connector.live_agents("/alpha") == 0);

    assert!(harness.threads(cx).read_with(cx, |threads, _| threads.threads().is_empty()));
    assert!(harness.selected_thread(cx).is_none());
    assert_eq!(thread.read_with(cx, |thread, _| thread.status()), ThreadStatus::Dormant);
    assert!(harness.bounds(cx, "thread-row-0").is_none());
    assert!(harness.bounds(cx, "history-row-alpha-session-status-dormant").is_some(), "the session is back in history");

    harness.click(cx, "history-row-alpha-session").unwrap();
    let resumed = harness.selected_thread(cx).expect("the resumed thread is selected");
    harness.run_until(cx, |cx| resumed.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(resumed.read_with(cx, |thread, _| texts(thread)), ["replayed answer"]);
    assert_eq!(connector.live_agents("/alpha"), 1);
}

#[gpui::test]
fn a_session_stopped_in_a_project_only_history_knows_about_stays_in_view(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "alpha-session", "/alpha", "2026-01-01T00:00:00Z", Some("hi"));
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).history(sessions).build(cx);
    wait_for_history(&harness, cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    let thread = harness.selected_thread(cx).expect("the new thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    harness.click(cx, "thread-row-0-stop").unwrap();
    harness.run_until(cx, |_| connector.live_agents("/alpha") == 0);

    assert!(harness.bounds(cx, "history-row-alpha-session").is_some(), "the stopped session is still listed");
}

#[gpui::test]
fn stopping_one_thread_leaves_the_others_running(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).build(cx);
    let alpha = new_thread(&harness, cx, "/alpha");
    let beta = new_thread(&harness, cx, "/beta");
    harness.run_until(cx, |cx| [&alpha, &beta].iter().all(|thread| thread.read(cx).status() == ThreadStatus::Idle));

    harness.click(cx, "thread-row-1-stop").unwrap();
    harness.run_until(cx, |_| connector.live_agents("/beta") == 0);
    alpha.update(cx, |thread, cx| thread.submit("still there?", cx));
    harness.run_until(cx, |cx| texts(alpha.read(cx)).len() == 2 && alpha.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(alpha.read_with(cx, |thread, _| texts(thread)), ["still there?", "from alpha"]);
    assert_eq!(connector.live_agents("/alpha"), 1);
}

#[gpui::test]
fn resuming_a_session_replays_its_history_into_the_transcript(cx: &mut TestAppContext) {
    let agent = FakeAgent::default().replay_message("saved-session", "an earlier answer");
    let connector = FakeConnector::default().agent("/alpha", agent);
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);

    let threads = harness.threads(cx);
    let thread = threads.update(cx, |threads, cx| threads.resume(SessionId::new("saved-session"), "/alpha".into(), cx));
    harness.run_until(cx, |cx| !texts(thread.read(cx)).is_empty());

    cx.read(|cx| {
        assert_eq!(thread.read(cx).status(), ThreadStatus::Idle);
        assert_eq!(thread.read(cx).session_id(), Some(&SessionId::new("saved-session")));
        assert_eq!(texts(thread.read(cx)), ["an earlier answer"]);
    });
}

#[gpui::test]
fn a_thread_whose_agent_cannot_start_is_errored(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    let thread = new_thread(&harness, cx, "/nowhere");
    harness.run_until(cx, |cx| thread.read(cx).status() != ThreadStatus::Connecting);

    let status = thread.read_with(cx, |thread, _| thread.status());
    assert!(matches!(&status, ThreadStatus::Errored(error) if error.contains("no fake agent")), "{status:?}");
}

#[gpui::test]
fn clicking_a_saved_session_resumes_it_in_a_selected_thread_that_replaces_its_row(cx: &mut TestAppContext) {
    let sessions = TestStore::new();
    save_session(&sessions, "saved-session", "/alpha", "2026-01-01T00:00:00Z", Some("earlier prompt"));
    let connector = FakeConnector::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"));
    let harness = DesktopHarnessBuilder::default().connector(connector).history(sessions).project("/alpha").build(cx);
    wait_for_history(&harness, cx);

    harness.click(cx, "history-row-saved-session").unwrap();
    let thread = harness.selected_thread(cx).expect("the resumed thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["an earlier answer"]);
    assert!(harness.bounds(cx, "transcript-row-0").is_some(), "the replayed history is painted");
    assert!(harness.bounds(cx, "history-row-saved-session").is_none(), "the live thread replaces the saved row");
    assert!(harness.bounds(cx, "thread-row-0").is_some());
}

#[gpui::test]
fn resuming_a_session_that_is_already_open_selects_its_thread(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .agent("/beta", replying("beta-session", "hello"));
    let harness = DesktopHarnessBuilder::default().connector(connector).build(cx);
    let threads = harness.threads(cx);
    let resumed =
        threads.update(cx, |threads, cx| threads.resume(SessionId::new("saved-session"), "/alpha".into(), cx));
    new_thread(&harness, cx, "/beta");

    let again = threads.update(cx, |threads, cx| threads.resume(SessionId::new("saved-session"), "/alpha".into(), cx));

    assert_eq!(again, resumed);
    assert_eq!(threads.read_with(cx, |threads, _| threads.threads().len()), 2);
    let selected = harness.selected_thread(cx).expect("a thread is selected");
    assert_eq!(selected, resumed);
}

#[gpui::test]
fn a_thread_whose_agent_died_reconnects_and_replays_its_session_without_duplicates(cx: &mut TestAppContext) {
    let connector = FakeConnector::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"));
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).build(cx);
    let thread = new_thread(&harness, cx, "/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| texts(thread.read(cx)).len() == 2 && thread.read(cx).status() == ThreadStatus::Idle);

    connector.kill_agents("/alpha");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Disconnected);
    assert!(harness.bounds(cx, "thread-row-0-status-errored").is_some());

    harness.click(cx, "thread-row-0-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| texts(thread)), ["replayed answer"]);
    assert_eq!(harness.selected_thread(cx), Some(thread));
    assert_eq!(connector.live_agents("/alpha"), 1);
}

#[gpui::test]
fn a_thread_whose_agent_could_not_start_reconnects_once_it_can(cx: &mut TestAppContext) {
    let connector = FakeConnector::default();
    let harness = DesktopHarnessBuilder::default().connector(connector.clone()).build(cx);
    let thread = new_thread(&harness, cx, "/alpha");
    harness.run_until(cx, |cx| matches!(thread.read(cx).status(), ThreadStatus::Errored(_)));

    connector.clone().agent("/alpha", replying("alpha-session", "hello"));
    harness.click(cx, "thread-row-0-reconnect").unwrap();
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert_eq!(thread.read_with(cx, |thread, _| thread.session_id().cloned()), Some(SessionId::new("alpha-session")));
}

fn new_thread(harness: &DesktopHarness, cx: &mut TestAppContext, cwd: &str) -> Entity<Thread> {
    harness.threads(cx).update(cx, |threads, cx| threads.new_thread(cwd.into(), cx))
}
