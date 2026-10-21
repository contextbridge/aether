use aether_desktop::ThemePair;
use aether_desktop::registry::{Disclosure, Registry, STATE_FILE, ThreadMeta};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AppContext, Entity, TestAppContext, px, size};
use std::fs;
use std::path::{Path, PathBuf};

#[gpui::test]
fn the_registry_reads_back_everything_it_saved(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let theme = ThemePair { light: "solarized-light".into(), dark: "solarized-dark".into() };
    let registry = load(cx, dir.path());

    registry.update(cx, |registry, cx| {
        registry.add_project("/alpha".into(), cx);
        registry.add_project("/beta".into(), cx);
        registry.set_disclosure("/alpha".into(), Disclosure::Collapsed, cx);
        registry.pin(&SessionId::new("pinned"), cx);
        registry.archive(&SessionId::new("archived"), cx);
        registry.rename(&SessionId::new("renamed"), "  Custom title ", cx);
        registry.set_window_size(size(px(900.0), px(700.0)), cx);
        registry.set_last_selected(Some(SessionId::new("renamed")), cx);
        registry.set_theme_override(Some(theme.clone()), cx);
    });
    cx.run_until_parked();
    let loaded = load(cx, dir.path());

    loaded.read_with(cx, |registry, _| {
        assert_eq!(registry.projects(), [PathBuf::from("/alpha"), PathBuf::from("/beta")]);
        assert_eq!(registry.disclosure(Path::new("/alpha")), Some(Disclosure::Collapsed));
        assert_eq!(registry.disclosure(Path::new("/beta")), None);
        assert_eq!(registry.thread(&SessionId::new("pinned")), &ThreadMeta { pinned: true, ..ThreadMeta::default() });
        assert_eq!(
            registry.thread(&SessionId::new("archived")),
            &ThreadMeta { archived: true, ..ThreadMeta::default() }
        );
        assert_eq!(registry.thread(&SessionId::new("renamed")).custom_title.as_deref(), Some("Custom title"));
        assert_eq!(registry.window_size(), Some(size(px(900.0), px(700.0))));
        assert_eq!(registry.last_selected(), Some(&SessionId::new("renamed")));
        assert_eq!(registry.theme_override(), Some(&theme));
    });
}

#[gpui::test]
fn undoing_every_change_to_a_thread_forgets_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let registry = load(cx, dir.path());
    let session = SessionId::new("session");

    registry.update(cx, |registry, cx| {
        registry.pin(&session, cx);
        registry.archive(&session, cx);
        registry.rename(&session, "Custom title", cx);
        registry.unpin(&session, cx);
        registry.unarchive(&session, cx);
        registry.rename(&session, "   ", cx);
    });
    cx.run_until_parked();

    assert_eq!(
        load(cx, dir.path()).read_with(cx, |registry, _| registry.thread(&session).clone()),
        ThreadMeta::default()
    );
    assert!(!fs::read_to_string(dir.path().join(STATE_FILE)).unwrap().contains("session"));
}

#[gpui::test]
fn a_corrupt_state_file_loads_as_nothing_remembered_and_is_replaced_by_the_next_save(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(STATE_FILE), "{ not json").unwrap();

    let registry = load(cx, dir.path());

    registry.read_with(cx, |registry, _| {
        assert_eq!(registry.projects(), Vec::<PathBuf>::new());
        assert_eq!(registry.window_size(), None);
        assert_eq!(registry.last_selected(), None);
        assert_eq!(registry.theme_override(), None);
    });
    registry.update(cx, |registry, cx| registry.add_project("/alpha".into(), cx));
    cx.run_until_parked();
    assert_eq!(
        load(cx, dir.path()).read_with(cx, |registry, _| registry.projects().to_vec()),
        [PathBuf::from("/alpha")]
    );
}

#[gpui::test]
fn a_missing_state_file_loads_as_nothing_remembered(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();

    let registry = load(cx, &dir.path().join("not-created-yet"));

    registry.read_with(cx, |registry, _| {
        assert_eq!(registry.projects(), Vec::<PathBuf>::new());
        assert_eq!(registry.window_size(), None);
    });
}

fn load(cx: &mut TestAppContext, dir: &Path) -> Entity<Registry> {
    let file = dir.join(STATE_FILE);
    cx.new(|cx| Registry::load(Some(file), cx))
}
