use foxfire_tui::settings::{UiSettings, list_theme_files, load_or_create_settings, save_settings};
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

#[test]
fn missing_settings_are_created_with_defaults() -> io::Result<()> {
    in_settings_home("missing_settings_are_created_with_defaults", |home| {
        assert_eq!(load_or_create_settings(), UiSettings::default());
        let saved: UiSettings = serde_json::from_slice(&fs::read(home.join("settings.json"))?)?;
        assert_eq!(saved, UiSettings::default());
        Ok(())
    })
}

#[test]
fn saved_settings_load_back() -> io::Result<()> {
    in_settings_home("saved_settings_load_back", |_| {
        let settings = UiSettings { content_padding: Some(4), ..UiSettings::default() };
        save_settings(&settings)?;
        assert_eq!(load_or_create_settings(), settings);
        Ok(())
    })
}

#[test]
fn malformed_settings_fall_back_to_defaults() -> io::Result<()> {
    in_settings_home("malformed_settings_fall_back_to_defaults", |home| {
        fs::create_dir_all(home)?;
        let path = home.join("settings.json");
        fs::write(&path, "{not-json")?;
        assert_eq!(load_or_create_settings(), UiSettings::default());
        assert_eq!(fs::read_to_string(path)?, "{not-json");
        Ok(())
    })
}

#[test]
fn the_default_settings_home_is_dot_foxfire() -> io::Result<()> {
    const TEST: &str = "the_default_settings_home_is_dot_foxfire";
    const CHILD: &str = "FOXFIRE_DEFAULT_HOME_TEST_CHILD";
    if let Some(home) = std::env::var_os(CHILD) {
        assert_eq!(load_or_create_settings(), UiSettings::default());
        assert!(Path::new(&home).join(".foxfire/settings.json").is_file());
        return Ok(());
    }

    let root = tempfile::tempdir()?;
    for override_home in [None, Some("")] {
        let mut command = Command::new(std::env::current_exe()?);
        command.args(["--exact", TEST, "--nocapture"]).env(CHILD, root.path()).env("HOME", root.path());
        if let Some(value) = override_home {
            command.env("FOXFIRE_HOME", value);
        } else {
            command.env_remove("FOXFIRE_HOME");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[test]
fn custom_themes_are_found_in_the_foxfire_home() -> io::Result<()> {
    in_settings_home("custom_themes_are_found_in_the_foxfire_home", |home| {
        let themes = home.join("themes");
        fs::create_dir_all(&themes)?;
        fs::write(themes.join("custom.json"), "{}")?;
        assert_eq!(list_theme_files(), ["custom.json"]);
        Ok(())
    })
}

fn in_settings_home(test_name: &str, check: impl FnOnce(&Path) -> io::Result<()>) -> io::Result<()> {
    const CHILD: &str = "FOXFIRE_SETTINGS_TEST_CHILD";
    if std::env::var(CHILD).as_deref() == Ok(test_name) {
        let home = std::env::var_os("FOXFIRE_HOME").ok_or_else(|| io::Error::other("missing Foxfire home"))?;
        return check(Path::new(&home));
    }

    let root = tempfile::TempDir::new()?;
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD, test_name)
        .env("FOXFIRE_HOME", root.path().join("foxfire"))
        .output()?;
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
