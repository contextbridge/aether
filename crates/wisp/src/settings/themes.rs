//! The theme files kept beside the settings: listing them for the picker and
//! loading the one the user chose.

use crate::settings::UiSettings;
use crate::theme::Theme;
use std::path::{Path, PathBuf};

pub fn resolve_theme_file_path(settings: &UiSettings) -> Option<PathBuf> {
    let file_name = settings.theme.file.as_deref()?.trim();
    resolve_theme_file_path_from_name(file_name)
}

fn themes_dir_path() -> Option<PathBuf> {
    let home = super::store()?.home().to_path_buf();
    Some(home.join("themes"))
}

pub fn list_theme_files() -> Vec<String> {
    let Some(themes_dir) = themes_dir_path() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(themes_dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let Ok(file_type) = entry.file_type() else {
                return None;
            };
            if !file_type.is_file() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            if !name.ends_with(".tmTheme") {
                return None;
            }
            Some(name)
        })
        .collect();
    files.sort_unstable();
    files
}

pub fn load_theme_file(file_name: &str) -> Theme {
    let Some(path) = resolve_theme_file_path_from_name(file_name) else {
        return Theme::default();
    };
    Theme::load_from_path(&path)
}

fn resolve_theme_file_path_from_name(file_name: &str) -> Option<PathBuf> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    let base_name = candidate.file_name()?.to_str()?;
    if base_name != trimmed {
        return None;
    }
    if matches!(base_name, "." | "..") {
        return None;
    }
    Some(themes_dir_path()?.join(base_name))
}
