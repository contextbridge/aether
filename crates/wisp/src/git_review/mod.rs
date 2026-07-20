mod model;
mod protocol;

pub(crate) use model::build_untracked_file_diff;
pub use model::{
    CommentContext, DiffDocument, DiffScope, EMPTY_TREE, FileDiff, FileStatus, GitDiffError, Hunk, PatchAnchor, PatchLine,
    PatchLineKind, QueuedComment, ReviewQueue, StageState, parse_porcelain_status, parse_unified_diff,
};
pub use protocol::GitDiffEvent;
