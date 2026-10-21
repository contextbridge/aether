use acp_utils::testing::FakeAgent;
use aether_desktop::testing::DesktopHarness;
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_sessions::testing::{TestStore, session_meta, user_message};
use agent_client_protocol::schema::v2::NewSessionResponse;
use gpui::{Entity, TestAppContext};
use std::borrow::Cow;

/// An agent that opens `session_id` and answers every prompt with `reply`.
pub(crate) fn replying(session_id: &str, reply: &str) -> FakeAgent {
    FakeAgent::default().new_session_response(NewSessionResponse::new(session_id)).prompt_reply(reply)
}

/// The text of every item in the thread's conversation that has text.
pub(crate) fn texts(thread: &Thread) -> Vec<String> {
    thread
        .core()
        .map(|core| core.conversation().items().iter().filter_map(|item| item.text().map(Cow::into_owned)).collect())
        .unwrap_or_default()
}

/// Saves a session that ran in `cwd`, titled by its first `prompt` if it has one.
pub(crate) fn save_session(store: &TestStore, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
    let mut meta = session_meta(session_id, created_at);
    meta.cwd = cwd.into();
    store.append_meta(session_id, &meta);
    if let Some(prompt) = prompt {
        store.append(session_id, &user_message(prompt));
    }
}

/// Waits until the app has read at least one saved session.
pub(crate) fn wait_for_history(harness: &DesktopHarness, cx: &mut TestAppContext) {
    let history = harness.history(cx);
    harness.run_until(cx, |cx| !history.read(cx).projects().is_empty());
}

/// Opens the context menu of the row tagged `row` and chooses the entry tagged `entry`.
pub(crate) fn choose(harness: &DesktopHarness, cx: &mut TestAppContext, row: &'static str, entry: &'static str) {
    harness.right_click(cx, row).unwrap();
    harness.click(cx, entry).unwrap();
}

/// Starts a thread by clicking a project's `+` button, tagged `new_thread`, and waits for its
/// agent to connect.
pub(crate) fn start_thread(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    new_thread: &'static str,
) -> Entity<Thread> {
    let thread = start_thread_in(harness, cx, new_thread);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    thread
}

pub(crate) fn start_thread_in(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    new_thread: &'static str,
) -> Entity<Thread> {
    harness.click(cx, new_thread).unwrap();
    harness.selected_thread(cx).expect("the new thread is selected")
}
