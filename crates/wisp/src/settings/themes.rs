use clankerdiff_theme::{ReviewTheme, ThemeChoice};

use crate::settings::{ThemeSettings, UiSettings};
use crate::theme::{Theme, ThemeLoadError};
use std::path::{Path, PathBuf};

pub(crate) fn builtin_review_theme_choices() -> Vec<ThemeChoice> {
    clankerdiff_theme::ReviewTheme::catalog()
        .into_iter()
        .map(|descriptor| {
            ThemeChoice::new(descriptor.name, ReviewTheme::builtin(&descriptor.id).expect("catalog theme"))
        })
        .collect()
}

pub(crate) fn review_theme_choices() -> Vec<ThemeChoice> {
    let mut choices = builtin_review_theme_choices();
    for file in list_theme_files() {
        match load_theme_file(&file) {
            Ok(theme) => choices.push(ThemeChoice::new(file.trim_end_matches(".json"), theme.review().clone())),
            Err(error) => tracing::warn!(%file, %error, "Could not load review theme choice"),
        }
    }
    choices
}

pub fn resolve_theme_file_path(settings: &UiSettings) -> Option<PathBuf> {
    let ThemeSettings::File { file } = &settings.theme else {
        return None;
    };
    resolve_theme_file_path_from_name(file).ok()
}

pub fn list_theme_files() -> Vec<String> {
    let Some(directory) = themes_dir_path() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut files = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_file() {
                return None;
            }
            let name = entry.file_name().into_string().ok()?;
            validate_file_name(&name).ok()?;
            Some(name)
        })
        .collect::<Vec<_>>();
    files.sort_unstable();
    files
}

pub fn load_theme_file(file: &str) -> Result<Theme, ThemeLoadError> {
    let path = resolve_theme_file_path_from_name(file)?;
    if !std::fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(ThemeLoadError::InvalidFile(file.into()));
    }
    Theme::load_from_path(&path)
}

pub(super) fn validate_file_name(file: &str) -> Result<(), ThemeLoadError> {
    let path = Path::new(file);
    if file.is_empty()
        || file.trim() != file
        || path.file_name().and_then(|name| name.to_str()) != Some(file)
        || file.contains(['/', '\\'])
        || path.extension().is_none_or(|extension| extension != "json")
    {
        return Err(ThemeLoadError::InvalidFile(file.into()));
    }
    Ok(())
}

fn resolve_theme_file_path_from_name(file: &str) -> Result<PathBuf, ThemeLoadError> {
    validate_file_name(file)?;
    themes_dir_path().map(|directory| directory.join(file)).ok_or_else(|| ThemeLoadError::InvalidFile(file.into()))
}

fn themes_dir_path() -> Option<PathBuf> {
    Some(super::store()?.home().join("themes"))
}
