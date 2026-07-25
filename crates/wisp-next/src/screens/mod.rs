pub mod git_diff;
pub mod plan_review;

/// Work a screen wants performed off the UI thread. Only the git-diff screen
/// produces effects today, so this is its effect type directly.
pub type ScreenEffect = git_diff::GitDiffEffect;

/// The result of a [`ScreenEffect`], routed back to the screen that asked for it.
pub type ScreenEvent = git_diff::GitDiffEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    ScrollUp,
    ScrollDown,
    Click,
}
