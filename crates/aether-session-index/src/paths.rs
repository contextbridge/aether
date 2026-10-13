use aether_sessions::analytics::SessionIndexError;
use aether_sessions::sessions_dir;
use std::path::PathBuf;
use utils::settings::aether_home;

pub(crate) fn default_sessions_dir() -> Result<PathBuf, SessionIndexError> {
    sessions_dir().ok_or(SessionIndexError::MissingAetherHome)
}

pub(crate) fn default_db_path() -> Result<PathBuf, SessionIndexError> {
    aether_home().map(|home| home.join("session-index.sqlite")).ok_or(SessionIndexError::MissingAetherHome)
}
