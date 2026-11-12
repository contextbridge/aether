use crate::support::{replying, texts};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::DesktopHarnessBuilder;
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::v2::schema::SessionId;

#[test]
fn concurrent_threads_each_receive_only_their_own_updates() {
    let resumed = FakeAgent::default().replay_message("gamma-session", "earlier in gamma").prompt_reply("from gamma");
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .agent("/gamma", resumed)
        .start("/alpha");
    let beta = harness.start_thread("/beta");
    let gamma = harness.threads().resume(SessionId::new("gamma-session"), "/gamma".into());
    let threads = [alpha, beta, gamma];
    harness.run_until(|_| gamma.status() == ThreadStatus::Idle);

    for (thread, prompt) in threads.iter().zip(["to alpha", "to beta", "to gamma"]) {
        thread.send(prompt.into());
    }
    harness.run_until(|_| {
        threads
            .iter()
            .zip([2, 2, 3])
            .all(|(thread, len)| texts(*thread).len() == len && thread.status() == ThreadStatus::Idle)
    });

    assert_eq!(alpha.session_id(), Some(SessionId::new("alpha-session")));
    assert_eq!(texts(alpha), ["to alpha", "from alpha"]);
    assert_eq!(beta.session_id(), Some(SessionId::new("beta-session")));
    assert_eq!(texts(beta), ["to beta", "from beta"]);
    assert_eq!(gamma.session_id(), Some(SessionId::new("gamma-session")));
    assert_eq!(texts(gamma), ["earlier in gamma", "to gamma", "from gamma"]);
}

#[test]
fn stopping_a_thread_ends_its_agent_and_returns_its_session_to_history() {
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .session("alpha-session", "/alpha", "2026-01-01T00:00:00Z", "hi")
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"))
        .start("/alpha");
    assert!(harness.bounds("history-row-alpha-session").is_none(), "the live thread stands in for its session");

    harness.hover("thread-row-0").unwrap();
    harness.click("thread-row-0-stop").unwrap();
    harness.run_until(|harness| harness.connector().live_agents("/alpha") == 0);

    assert_eq!(harness.threads().threads(), Vec::<Thread>::new());
    assert!(harness.selected_thread().is_none());
    assert_eq!(thread.status(), ThreadStatus::Dormant);
    assert!(harness.bounds("thread-row-0").is_none());
    assert!(harness.bounds("history-row-alpha-session").is_some(), "the session is back in history");

    harness.click("history-row-alpha-session").unwrap();
    let resumed = harness.connected_thread();

    assert_eq!(texts(resumed), ["replayed answer"]);
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
}

#[test]
fn stopping_one_thread_leaves_the_others_running() {
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "from alpha"))
        .agent("/beta", replying("beta-session", "from beta"))
        .start("/alpha");
    harness.start_thread("/beta");

    harness.hover("thread-row-1").unwrap();
    harness.click("thread-row-1-stop").unwrap();
    harness.run_until(|harness| harness.connector().live_agents("/beta") == 0);
    harness.submit(alpha, "still there?");

    assert_eq!(texts(alpha), ["still there?", "from alpha"]);
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
}

#[test]
fn a_thread_whose_agent_cannot_start_is_errored_and_gives_a_prompt_back() {
    let mut harness = DesktopHarnessBuilder::default().project("/nowhere").build();

    let thread = harness.new_thread("/nowhere");
    harness.run_until(|_| thread.status() != ThreadStatus::Connecting);

    let (status, error) = (thread.status(), thread.error());
    assert_eq!(status, ThreadStatus::Errored);
    assert!(error.as_deref().is_some_and(|error| error.contains("no fake agent")), "{error:?}");
    assert!(harness.bounds("thread-row-0-status-errored-warning-triangle").is_some());
    assert!(harness.bounds("thread-row-0-conversation-icon").is_none());
    harness.type_text("hello?");
    harness.press("enter");
    assert!(harness.shows("composer-text", "hello?"), "a prompt the agent never received is kept");
}

#[test]
fn clicking_a_saved_session_resumes_it_in_a_selected_thread_that_replaces_its_row() {
    let mut harness = DesktopHarnessBuilder::default()
        .session("saved-session", "/alpha", "2026-01-01T00:00:00Z", "earlier prompt")
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .build();

    harness.click("history-row-saved-session").unwrap();
    let thread = harness.connected_thread();

    assert_eq!(thread.session_id(), Some(SessionId::new("saved-session")));
    assert_eq!(texts(thread), ["an earlier answer"]);
    assert!(harness.bounds("transcript-row-0").is_some(), "the replayed history is painted");
    assert!(harness.bounds("history-row-saved-session").is_none(), "the live thread replaces the saved row");
    assert!(harness.bounds("thread-row-0").is_some());
}

#[test]
fn resuming_a_session_that_is_already_open_selects_its_thread() {
    let mut harness = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::default().replay_message("saved-session", "an earlier answer"))
        .agent("/beta", replying("beta-session", "hello"))
        .build();
    let threads = harness.threads();
    let resumed = threads.resume(SessionId::new("saved-session"), "/alpha".into());
    harness.new_thread("/beta");

    let again = threads.resume(SessionId::new("saved-session"), "/alpha".into());

    assert_eq!(again, resumed);
    assert_eq!(threads.threads().len(), 2);
    assert_eq!(harness.selected_thread(), Some(resumed));
}

#[test]
fn a_thread_whose_agent_died_reconnects_from_its_transcript_and_replays_its_session_without_duplicates() {
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", "hello"))
        .agent("/alpha", FakeAgent::default().replay_message("alpha-session", "replayed answer"))
        .start("/alpha");
    harness.submit(thread, "hi");
    assert!(harness.bounds("transcript-reconnect").is_none(), "a connected thread offers no reconnect");

    harness.connector().kill_agents("/alpha");
    harness.run_until(|_| thread.status() == ThreadStatus::Disconnected);
    assert!(harness.bounds("thread-row-0-status-disconnected").is_some());
    harness.click("transcript-reconnect").unwrap();
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);

    assert_eq!(texts(thread), ["replayed answer"]);
    assert_eq!(harness.selected_thread(), Some(thread));
    assert_eq!(harness.connector().live_agents("/alpha"), 1);
    assert!(harness.bounds("transcript-reconnect").is_none());
}

#[test]
fn a_thread_whose_agent_could_not_start_reconnects_from_the_sidebar_once_it_can() {
    let mut harness = DesktopHarnessBuilder::default().build();
    let thread = harness.new_thread("/alpha");
    harness.run_until(|_| thread.status() == ThreadStatus::Errored);

    harness.connector().script("/alpha", replying("alpha-session", "hello"));
    harness.click("thread-row-0-reconnect").unwrap();
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);

    assert_eq!(thread.session_id(), Some(SessionId::new("alpha-session")));
}
