mod effects;
mod input;
mod rendering;
mod state;

pub use effects::{GitDiffEffect, GitDiffEvent};
pub use state::GitDiffScreen;

use crate::surface::SurfaceMessage;

/// Wraps an effect as the single message its handler returns.
fn effect(effect: GitDiffEffect) -> Vec<SurfaceMessage> {
    vec![SurfaceMessage::Effect(effect.into())]
}
