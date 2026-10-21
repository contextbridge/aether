use crate::support::{replying, save_session};
use aether_desktop::history::{HistoryEntry, ProjectHistory, UNTITLED, git_root, group_by_project};
use aether_desktop::testing::{DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::ThreadStatus;
use aether_sessions::SessionSummary;
use aether_sessions::testing::session_meta;
use chrono::DateTime;
use gpui::TestAppContext;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn sessions_group_by_project_root_newest_first_and_untitled_ones_read_new_thread() {
    let sessions = vec![
        summary("first", "/repo", "2026-01-01T00:00:00Z", Some("first prompt")),
        summary("elsewhere", "/other", "2026-02-01T00:00:00Z", Some("other prompt")),
        summary("latest", "/repo/crates/core", "2026-03-01T00:00:00Z", None),
    ];

    let projects =
        group_by_project(
            sessions,
            |cwd| {
                if cwd.starts_with("/repo") { PathBuf::from("/repo") } else { cwd.to_path_buf() }
            },
        );

    assert_eq!(
        projects,
        [
            ProjectHistory {
                root: "/repo".into(),
                sessions: vec![
                    entry("latest", "/repo/crates/core", "2026-03-01T00:00:00Z", UNTITLED),
                    entry("first", "/repo", "2026-01-01T00:00:00Z", "first prompt"),
                ],
            },
            ProjectHistory {
                root: "/other".into(),
                sessions: vec![entry("elsewhere", "/other", "2026-02-01T00:00:00Z", "other prompt")],
            },
        ]
    );
}

#[test]
fn a_directory_inside_a_checkout_belongs_to_the_checkouts_root() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("crates/core")).unwrap();

    assert_eq!(git_root(&repo.join("crates/core")), repo);
    assert_eq!(git_root(&repo), repo);
}

#[test]
fn a_directory_outside_any_checkout_is_its_own_project() {
    let dir = tempfile::tempdir().unwrap();
    let plain = dir.path().join("plain");
    fs::create_dir_all(&plain).unwrap();

    assert_eq!(git_root(&plain), plain);
    assert_eq!(git_root(Path::new("/no/such/dir")), Path::new("/no/such/dir"));
}

#[gpui::test]
fn a_finished_turn_refreshes_history(cx: &mut TestAppContext) {
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", "done"));
    let harness = DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx);
    let history = harness.history(cx);
    harness.click(cx, "project-alpha-new-thread").unwrap();
    let thread = harness.selected_thread(cx).expect("a thread is selected");
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    save_session(harness.sessions(), "saved-by-the-agent", "/alpha", "2026-01-01T00:00:00Z", Some("hi"));
    assert!(cx.read(|cx| history.read(cx).projects().is_empty()), "history has not been re-read yet");

    thread.update(cx, |thread, cx| thread.submit("hi", cx));
    harness.run_until(cx, |cx| !history.read(cx).projects().is_empty());

    let sessions = cx.read(|cx| history.read(cx).projects()[0].sessions.clone());
    assert_eq!(sessions, [entry("saved-by-the-agent", "/alpha", "2026-01-01T00:00:00Z", "hi")]);
}

fn summary(session_id: &str, cwd: &str, created_at: &str, title: Option<&str>) -> SessionSummary {
    let mut meta = session_meta(session_id, created_at);
    meta.cwd = cwd.into();
    SessionSummary { meta, title: title.map(str::to_owned) }
}

fn entry(session_id: &str, cwd: &str, created_at: &str, title: &str) -> HistoryEntry {
    HistoryEntry {
        session_id: session_id.into(),
        cwd: cwd.into(),
        title: title.into(),
        created_at: Some(DateTime::parse_from_rfc3339(created_at).unwrap().to_utc()),
    }
}
