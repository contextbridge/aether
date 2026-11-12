use crate::support::choose;
use acp_utils::testing::FakeAgent;
use aether_desktop::registry::{STATE_FILE, load};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use aether_desktop::ui::interface_size::DEFAULT_INTERFACE_SIZE;
use agent_client_protocol::v2::schema::SessionId;
use clankerdiff_rouge_shell::rouge::core::Point;
use std::path::{Path, PathBuf};
use tokio::runtime::Builder;

#[test]
fn the_next_launch_restores_what_the_user_changed() {
    let state = tempfile::tempdir().unwrap();
    let mut harness = launch(state.path());
    harness.click("add-project").unwrap();
    harness.click("add-project-browse").unwrap();
    harness.platform().answer_path_prompt(Some(vec![PathBuf::from("/picked")]));
    harness.click("project-beta").unwrap();
    choose(&mut harness, "history-row-gone", "menu-item-Archive");
    choose(&mut harness, "history-row-renamed", "menu-item-Rename");
    harness.type_text("Renamed");
    harness.press("enter");
    harness.click("history-row-saved").unwrap();
    let resumed = harness.selected_thread().expect("the resumed session is selected");
    harness.run_until(|_| resumed.status() == ThreadStatus::Idle);
    harness.press("secondary-= secondary-=");
    let (handle, sidebar) = (harness.bounds("panel-resize-handle").unwrap(), harness.bounds("sidebar").unwrap());
    harness.drag("panel-resize-handle", Point::new(handle.center().x + 400.0 - sidebar.size.width, 300.0)).unwrap();
    harness.resize(900.0, 700.0);
    harness.quit();

    let mut relaunched = launch(state.path());
    let threads = relaunched.threads();
    relaunched.run_until(|_| threads.selected().is_some_and(|thread| thread.status() == ThreadStatus::Idle));

    let thread = relaunched.selected_thread().expect("the last selected session is selected again");
    assert_eq!(thread.session_id(), Some(SessionId::new("saved")));
    assert!(relaunched.bounds("project-picked").is_some(), "an added project is listed");
    assert!(relaunched.bounds("history-row-hidden").is_none(), "a collapsed project stays collapsed");
    assert!(relaunched.bounds("history-row-gone").is_none(), "archived rows stay archived");
    assert!(relaunched.shows("history-row-renamed-title", "Renamed"));
    assert_eq!(relaunched.rem_size(), DEFAULT_INTERFACE_SIZE + 2.0);
    let sidebar = relaunched.bounds("sidebar").expect("the sidebar is painted");
    let main = relaunched.bounds("main").expect("the main region is painted");
    assert!((sidebar.size.width - 400.0).abs() < 0.5, "the sidebar keeps its width: {sidebar:?}");
    assert_eq!((sidebar.size.width + main.size.width, main.size.height), (900.0, 700.0));
}

#[test]
fn a_last_selected_session_missing_from_history_is_not_resumed() {
    let state = tempfile::tempdir().unwrap();
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    let (saved, flush) = load(Some(state.path().join(STATE_FILE)), runtime.handle());
    saved.update(|state| {
        state.last_selected = Some(SessionId::new("deleted"));
        true
    });
    drop(saved);
    runtime.block_on(flush).unwrap().unwrap();

    let relaunched = launch(state.path());

    assert!(relaunched.selected_thread().is_none());
}

fn launch(state: &Path) -> DesktopHarness {
    DesktopHarnessBuilder::default()
        .state_dir(state)
        .agent("/alpha", FakeAgent::default().replay_message("saved", "an earlier answer"))
        .project("/beta")
        .session("saved", "/alpha", "2026-01-01T00:00:00Z", "saved prompt")
        .session("gone", "/alpha", "2026-01-03T00:00:00Z", "gone prompt")
        .session("renamed", "/alpha", "2026-01-04T00:00:00Z", "renamed prompt")
        .session("hidden", "/beta", "2026-01-01T00:00:00Z", "beta prompt")
        .build()
}
