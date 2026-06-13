use super::git_diff_mode::{GitDiffMode, GitDiffViewMessage};
use super::plan_review_mode::{PlanReviewAction, PlanReviewInput, PlanReviewMode};
use std::path::PathBuf;
use tui::{Component, Event, Frame, ViewContext};

pub enum ScreenRouterMessage {
    LoadGitDiff,
    RefreshGitDiff,
    SendPrompt { user_input: String },
    FinishPlanReview(PlanReviewAction),
}

pub struct ScreenRouter {
    mode: Option<FullScreenMode>,
    git_diff_working_dir: PathBuf,
}

// Both variants are boxed to keep ScreenRouter's stack frame small since
// GitDiffMode and PlanReviewMode own rendered patch buffers and split panels.
enum FullScreenMode {
    GitDiff(Box<GitDiffMode>),
    PlanReview(Box<PlanReviewMode>),
}

impl ScreenRouter {
    pub fn new(git_diff_working_dir: PathBuf) -> Self {
        Self { mode: None, git_diff_working_dir }
    }

    #[cfg(test)]
    pub fn is_git_diff(&self) -> bool {
        matches!(self.mode, Some(FullScreenMode::GitDiff(_)))
    }

    #[cfg(test)]
    pub fn is_plan_review(&self) -> bool {
        matches!(self.mode, Some(FullScreenMode::PlanReview(_)))
    }

    pub fn is_full_screen_mode(&self) -> bool {
        self.mode.is_some()
    }

    pub fn set_git_diff_working_dir(&mut self, dir: PathBuf) {
        self.git_diff_working_dir = dir;
    }

    pub fn toggle_git_diff(&mut self) -> Option<ScreenRouterMessage> {
        match self.mode.take() {
            None => {
                let mut git_diff_mode = GitDiffMode::new(self.git_diff_working_dir.clone());
                git_diff_mode.begin_open();
                self.mode = Some(FullScreenMode::GitDiff(Box::new(git_diff_mode)));
                Some(ScreenRouterMessage::LoadGitDiff)
            }
            Some(FullScreenMode::GitDiff(mut git_diff_mode)) => {
                git_diff_mode.close();
                None
            }
            Some(full_screen_mode) => {
                self.mode = Some(full_screen_mode);
                None
            }
        }
    }

    pub fn close_git_diff(&mut self) {
        match self.mode.take() {
            Some(FullScreenMode::GitDiff(mut git_diff_mode)) => {
                git_diff_mode.close();
            }
            Some(full_screen_mode) => {
                self.mode = Some(full_screen_mode);
            }
            None => {}
        }
    }

    pub fn open_plan_review(&mut self, input: PlanReviewInput) {
        self.mode = Some(FullScreenMode::PlanReview(Box::new(PlanReviewMode::new(input))));
    }

    #[cfg(test)]
    pub fn enter_git_diff_for_test(&mut self) {
        self.mode = Some(FullScreenMode::GitDiff(Box::new(GitDiffMode::new(self.git_diff_working_dir.clone()))));
    }

    pub fn git_diff_mode_mut(&mut self) -> &mut GitDiffMode {
        let Some(FullScreenMode::GitDiff(git_diff_mode)) = self.mode.as_mut() else {
            panic!("git diff mode should be active");
        };
        git_diff_mode
    }
}

impl Component for ScreenRouter {
    type Message = ScreenRouterMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<ScreenRouterMessage>> {
        match &mut self.mode {
            None => Some(vec![]),
            Some(FullScreenMode::GitDiff(git_diff_mode)) => {
                let messages = git_diff_mode.on_event(event).await.unwrap_or_default();
                let mut close_git_diff = false;
                let mut router_messages = Vec::new();
                for msg in messages {
                    match msg {
                        GitDiffViewMessage::Close => close_git_diff = true,
                        GitDiffViewMessage::Refresh => {
                            git_diff_mode.begin_refresh();
                            router_messages.push(ScreenRouterMessage::RefreshGitDiff);
                        }
                        GitDiffViewMessage::SubmitPrompt(user_input) => {
                            router_messages.push(ScreenRouterMessage::SendPrompt { user_input });
                        }
                    }
                }
                if close_git_diff {
                    self.mode = None;
                }
                Some(router_messages)
            }
            Some(FullScreenMode::PlanReview(mode)) => {
                let actions = mode.on_event(event).await.unwrap_or_default();
                let Some(action) = actions.into_iter().next() else {
                    return Some(vec![]);
                };
                self.mode = None;
                Some(vec![ScreenRouterMessage::FinishPlanReview(action)])
            }
        }
    }

    fn render(&mut self, ctx: &ViewContext) -> Frame {
        match &mut self.mode {
            None => Frame::empty(),
            Some(FullScreenMode::GitDiff(git_diff_mode)) => git_diff_mode.render(ctx),
            Some(FullScreenMode::PlanReview(mode)) => mode.render(ctx),
        }
    }
}
