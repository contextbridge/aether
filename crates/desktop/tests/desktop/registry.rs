use aether_desktop::registry::{Registry, STATE_FILE, State};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AppContext, Entity, TestAppContext, px, size};
use std::fs;
use std::path::{Path, PathBuf};

#[gpui::test]
fn the_registry_reads_back_everything_it_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let registry = load(cx, dir.path());

    Registry::update(&registry, cx, |state| {
        state.add_project("/alpha".into());
        state.add_project("/beta".into());
        state.add_project("/alpha".into());
        state.collapsed.insert("/alpha".into());
        state.pinned.insert(SessionId::new("pinned"));
        state.archived.insert(SessionId::new("archived"));
        state.rename(SessionId::new("renamed"), "  Custom title ");
        state.last_agents.insert("/alpha".into(), "claude".into());
        state.window_size = Some(size(px(900.0), px(700.0)));
        state.last_selected = Some(SessionId::new("renamed"));
        state.interface_size = Some(19.0);
        state.sidebar_width = Some(20.0);
        state.sidebar_collapsed = true;
    });
    cx.run_until_parked();
    let saved = registry.read_with(cx, |registry, _| registry.state().clone());

    assert_eq!(saved.projects, [PathBuf::from("/alpha"), PathBuf::from("/beta")]);
    assert_eq!(saved.title(Some(&SessionId::new("renamed")), "derived".into()), "Custom title");
    assert_eq!(load(cx, dir.path()).read_with(cx, |registry, _| registry.state().clone()), saved);
}

#[gpui::test]
fn undoing_every_change_to_a_thread_forgets_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let registry = load(cx, dir.path());
    let session = SessionId::new("session");

    Registry::update(&registry, cx, |state| {
        state.pinned.insert(session.clone());
        state.rename(session.clone(), "Custom title");
        state.add_project("/alpha".into());
    });
    Registry::update(&registry, cx, |state| {
        state.pinned.remove(&session);
        state.rename(session.clone(), "   ");
    });
    cx.run_until_parked();

    assert_eq!(
        registry.read_with(cx, |registry, _| registry.state().title(Some(&session), "derived".into())),
        "derived"
    );
    assert!(!fs::read_to_string(dir.path().join(STATE_FILE)).unwrap().contains("session"));
}

#[gpui::test]
fn a_corrupt_state_file_loads_as_nothing_remembered_and_is_replaced_by_the_next_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(STATE_FILE), "{ not json").unwrap();

    let registry = load(cx, dir.path());

    assert_eq!(registry.read_with(cx, |registry, _| registry.state().clone()), State::default());
    Registry::update(&registry, cx, |state| state.add_project("/alpha".into()));
    cx.run_until_parked();
    assert_eq!(
        load(cx, dir.path()).read_with(cx, |registry, _| registry.state().projects.clone()),
        [PathBuf::from("/alpha")]
    );
}

#[gpui::test]
fn a_missing_state_file_loads_as_nothing_remembered(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();

    let registry = load(cx, &dir.path().join("not-created-yet"));

    assert_eq!(registry.read_with(cx, |registry, _| registry.state().clone()), State::default());
}

fn load(cx: &mut TestAppContext, dir: &Path) -> Entity<Registry> {
    let file = dir.join(STATE_FILE);
    cx.new(|cx| Registry::load(Some(file), cx))
}
