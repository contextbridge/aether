use crate::support::choose;
use crate::thread_menu::row_order;
use acp_utils::testing::FakeAgent;
use aether_desktop::registry::{Registry, STATE_FILE};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use aether_desktop::ui::interface_size::DEFAULT_INTERFACE_SIZE;
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AppContext, TestAppContext, point, px};
use std::path::{Path, PathBuf};

#[gpui::test]
fn the_next_launch_restores_what_the_user_changed(first: &mut TestAppContext, second: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let harness = launch(first, state.path());
    harness.click(first, "add-project").unwrap();
    harness.click(first, "add-project-browse").unwrap();
    first.simulate_path_prompt_response(|_| Some(vec![PathBuf::from("/picked")]));
    harness.click(first, "project-beta").unwrap();
    choose(&harness, first, "history-row-pinned", "menu-pin");
    choose(&harness, first, "history-row-gone", "menu-archive");
    choose(&harness, first, "history-row-renamed", "menu-rename");
    harness.simulate_input(first, "Renamed");
    harness.simulate_keystrokes(first, "enter");
    harness.click(first, "history-row-saved").unwrap();
    let resumed = harness.selected_thread(first).expect("the resumed session is selected");
    harness.run_until(first, |cx| resumed.read(cx).status() == ThreadStatus::Idle);
    harness.simulate_keystrokes(first, "secondary-= secondary-=");
    harness.drag(first, "sidebar-resize-handle", point(px(400.0), px(300.0))).unwrap();
    harness.resize(first, 900.0, 700.0);
    first.run_until_parked();

    let relaunched = launch(second, state.path());
    let threads = relaunched.threads(second);
    relaunched.run_until(second, |cx| {
        threads.read(cx).selected().is_some_and(|thread| thread.read(cx).status() == ThreadStatus::Idle)
    });

    let thread = relaunched.selected_thread(second).expect("the last selected session is selected again");
    assert_eq!(thread.read_with(second, |thread, _| thread.session_id().cloned()), Some(SessionId::new("saved")));
    assert!(relaunched.bounds(second, "project-picked").is_some(), "an added project is listed");
    assert!(relaunched.bounds(second, "history-row-hidden").is_none(), "a collapsed project stays collapsed");
    assert_eq!(row_order(&relaunched, second, &["history-row-pinned", "thread-row-0"]), [0, 1], "pins are kept");
    assert!(relaunched.bounds(second, "history-row-gone").is_none(), "archived rows stay archived");
    assert!(relaunched.shows(second, "history-row-renamed-title", "Renamed"));
    assert_eq!(relaunched.rem_size(second), px(DEFAULT_INTERFACE_SIZE + 2.0));
    let sidebar = relaunched.bounds(second, "sidebar").expect("the sidebar is painted");
    let main = relaunched.bounds(second, "main").expect("the main region is painted");
    assert!((sidebar.size.width - px(400.0)).abs() < px(0.5), "the sidebar keeps its width: {sidebar:?}");
    assert_eq!((sidebar.size.width + main.size.width, main.size.height), (px(900.0), px(700.0)));
}

#[gpui::test]
fn a_last_selected_session_missing_from_history_is_not_resumed(cx: &mut TestAppContext) {
    let state = tempfile::tempdir().unwrap();
    let registry = cx.new(|cx| Registry::load(Some(state.path().join(STATE_FILE)), cx));
    Registry::update(&registry, cx, |state| state.last_selected = Some(SessionId::new("deleted")));
    cx.run_until_parked();

    let relaunched = launch(cx, state.path());

    assert!(relaunched.selected_thread(cx).is_none());
}

fn launch(cx: &mut TestAppContext, state: &Path) -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .state_dir(state)
        .agent("/alpha", FakeAgent::default().replay_message("saved", "an earlier answer"))
        .project("/beta")
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "saved prompt")
        .session("pinned", "/alpha", "2026-01-02T00:00:00Z", "pinned prompt")
        .session("gone", "/alpha", "2026-01-03T00:00:00Z", "gone prompt")
        .session("renamed", "/alpha", "2026-01-04T00:00:00Z", "renamed prompt")
        .session("hidden", "/beta", "2026-01-01T00:00:00Z", "beta prompt")
        .build(cx)
}
