use std::path::Path;
use std::process::Command;
use wisp::settings::{UiSettings, load_or_create_settings, save_settings};

#[test]
fn missing_settings_are_created_with_defaults() {
    in_settings_home("missing_settings_are_created_with_defaults", |home| {
        assert_eq!(load_or_create_settings(), UiSettings::default());
        let saved: UiSettings = serde_json::from_slice(&std::fs::read(home.join("settings.json")).unwrap()).unwrap();
        assert_eq!(saved, UiSettings::default());
    });
}

#[test]
fn saved_settings_load_back() {
    in_settings_home("saved_settings_load_back", |_| {
        let settings = UiSettings { content_padding: Some(4), ..UiSettings::default() };
        save_settings(&settings).unwrap();
        assert_eq!(load_or_create_settings(), settings);
    });
}

#[test]
fn malformed_settings_fall_back_to_defaults() {
    in_settings_home("malformed_settings_fall_back_to_defaults", |home| {
        std::fs::create_dir_all(home).unwrap();
        let path = home.join("settings.json");
        std::fs::write(&path, "{not-json").unwrap();
        assert_eq!(load_or_create_settings(), UiSettings::default());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{not-json");
    });
}

fn in_settings_home(test_name: &str, check: impl FnOnce(&Path)) {
    const CHILD: &str = "WISP_SETTINGS_TEST_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(test_name) {
        check(Path::new(&std::env::var_os("WISP_HOME").unwrap()));
        return;
    }

    let root = tempfile::TempDir::new().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD, test_name)
        .env("WISP_HOME", root.path().join("wisp"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
