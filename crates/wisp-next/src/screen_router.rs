use std::path::Path;

use agent_client_protocol::Responder;
use crossterm::event::KeyEvent;
use ratatui::Frame;
use utils::plan_review::PlanReviewElicitationMeta;

use crate::screens::git_diff::{GitDiffEffect, GitDiffEvent, GitDiffOutcome, GitDiffScreen};
use crate::screens::plan_review::PlanReviewScreen;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;

pub struct ScreenRouter {
    mode: Option<FullScreenMode>,
}

pub enum ScreenEffect {
    GitDiff(GitDiffEffect),
}

pub enum ScreenEvent {
    GitDiff(GitDiffEvent),
}

enum FullScreenMode {
    GitDiff(GitDiffScreen),
    PlanReview(PlanReviewScreen),
}

impl ScreenRouter {
    pub fn new() -> Self {
        Self { mode: None }
    }

    pub fn is_active(&self) -> bool {
        self.mode.is_some()
    }

    pub fn open_git_diff(&mut self, working_dir: &Path) -> ScreenEffect {
        self.close();
        let (screen, effect) = GitDiffScreen::new(working_dir.to_path_buf());
        self.mode = Some(FullScreenMode::GitDiff(screen));
        ScreenEffect::GitDiff(effect)
    }

    pub fn open_plan_review(
        &mut self,
        meta: PlanReviewElicitationMeta,
        responder: Responder<acp_utils::notifications::ElicitationResponse>,
    ) {
        self.close();
        self.mode = Some(FullScreenMode::PlanReview(PlanReviewScreen::new(meta, responder)));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Option<ScreenEffect> {
        let mode = self.mode.as_mut()?;
        match mode {
            FullScreenMode::GitDiff(screen) => match screen.on_key(key) {
                GitDiffOutcome::None => None,
                GitDiffOutcome::Close => {
                    self.mode = None;
                    None
                }
                GitDiffOutcome::Effect(effect) => Some(ScreenEffect::GitDiff(effect)),
            },
            FullScreenMode::PlanReview(screen) => {
                if screen.on_key(key) {
                    self.mode = None;
                }
                None
            }
        }
    }

    pub fn on_event(&mut self, event: ScreenEvent) -> Option<ScreenEffect> {
        match (&mut self.mode, event) {
            (Some(FullScreenMode::GitDiff(screen)), ScreenEvent::GitDiff(event)) => {
                screen.on_event(event).map(ScreenEffect::GitDiff)
            }
            _ => None,
        }
    }

    pub fn render(&mut self, frame: &mut Frame, theme: &Theme, highlighter: &mut SyntaxHighlighter) {
        match self.mode.as_mut() {
            Some(FullScreenMode::GitDiff(screen)) => screen.render(frame, theme, highlighter),
            Some(FullScreenMode::PlanReview(screen)) => screen.render(frame, theme, highlighter),
            None => {}
        }
    }

    pub fn close(&mut self) {
        if let Some(FullScreenMode::GitDiff(screen)) = self.mode.as_mut() {
            screen.cancel();
        }
        if let Some(FullScreenMode::PlanReview(screen)) = self.mode.as_mut() {
            screen.cancel();
        }
        self.mode = None;
    }
}

impl ScreenEffect {
    pub async fn execute(self) -> ScreenEvent {
        match self {
            Self::GitDiff(effect) => ScreenEvent::GitDiff(effect.execute().await),
        }
    }
}

impl Default for ScreenRouter {
    fn default() -> Self {
        Self::new()
    }
}
