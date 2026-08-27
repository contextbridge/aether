use aether_sessions::analytics::SessionIndexError;
use std::path::PathBuf;

pub(crate) fn default_sessions_dir() -> Result<PathBuf, SessionIndexError> {
    Ok(default_aether_home()?.join("sessions"))
}

pub(crate) fn default_db_path() -> Result<PathBuf, SessionIndexError> {
    Ok(default_aether_home()?.join("session-index.sqlite"))
}

fn default_aether_home() -> Result<PathBuf, SessionIndexError> {
    utils::settings::aether_home().ok_or(SessionIndexError::MissingAetherHome)
}
