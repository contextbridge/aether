mod protocol;

pub use clankerdiff_ratatui::diff::{DiffDocument, DiffScope, FileDiff, FileStatus, StageState};
pub use protocol::{GitDiffError, GitDiffEvent, GitWatchError, GitWatchEvent, GitWatchResult};
