use crate::components::common::render_key_hints;
use crate::components::file_list_panel::{FileListMessage, FileListPanel};
use crate::components::git_diff::git_diff_panel::{GitDiffPanel, GitDiffPanelMessage};
use crate::components::git_diff::{DiffAnchor, PatchAnchor};
use crate::components::review_comments::{CommentAnchor, ReviewComment};
use crate::git_diff::{GitDiffDocument, PatchLineKind, load_git_diff};
use std::collections::HashMap;
use std::path::PathBuf;
use tui::{Component, Either, Event, Frame, KeyCode, Line, SplitLayout, SplitPanel, Style, ViewContext};

pub enum GitDiffViewMessage {
    Close,
    Refresh,
    SubmitPrompt(String),
}

pub enum GitDiffLoadState {
    Loading,
    Ready(GitDiffDocument),
    Empty,
    Error { message: String },
}

#[derive(Debug, Clone)]
pub(crate) struct GitDiffCommentContext {
    pub file_path: String,
    pub line_text: String,
    pub line_number: Option<usize>,
    pub line_kind: PatchLineKind,
}

#[derive(Debug, Clone)]
pub(crate) struct QueuedComment {
    pub review: ReviewComment<PatchAnchor>,
    pub context: GitDiffCommentContext,
}

pub struct GitDiffMode {
    working_dir: PathBuf,
    cached_repo_root: Option<PathBuf>,
    document_revision: usize,
    pub load_state: GitDiffLoadState,
    split: SplitPanel<FileListPanel, GitDiffPanel>,
    comments: ReviewQueue,
    pending_restore: Option<RefreshState>,
}

impl GitDiffMode {
    pub fn new(working_dir: PathBuf) -> Self {
        Self {
            working_dir,
            cached_repo_root: None,
            document_revision: 0,
            load_state: GitDiffLoadState::Empty,
            split: SplitPanel::new(FileListPanel::new(), GitDiffPanel::new(), SplitLayout::fraction(1, 3, 20, 28))
                .with_separator("│", Style::default())
                .with_resize_keys(),
            comments: ReviewQueue::default(),
            pending_restore: None,
        }
    }

    pub(crate) fn begin_open(&mut self) {
        self.reset(GitDiffLoadState::Loading);
    }

    pub(crate) fn begin_refresh(&mut self) {
        self.pending_restore = Some(RefreshState {
            selected_path: self.selected_file_path().map(ToOwned::to_owned),
            was_right_focused: !self.split.is_left_focused(),
        });
        self.load_state = GitDiffLoadState::Loading;
        self.split.right_mut().clear_rendered_patches();
    }

    pub(crate) async fn complete_load(&mut self) {
        match load_git_diff(&self.working_dir, self.cached_repo_root.as_deref()).await {
            Ok(doc) => {
                if self.cached_repo_root.is_none() {
                    self.cached_repo_root = Some(doc.repo_root.clone());
                }
                let restore = self.pending_restore.take();
                self.apply_loaded_document(doc, restore);
            }
            Err(error) => {
                self.pending_restore = None;
                self.load_state = GitDiffLoadState::Error { message: error.to_string() };
                self.split.right_mut().clear_rendered_patches();
            }
        }
    }

    pub(crate) fn close(&mut self) {
        self.reset(GitDiffLoadState::Empty);
    }

    fn reset(&mut self, load_state: GitDiffLoadState) {
        self.pending_restore = None;
        self.load_state = load_state;
        *self.split.left_mut() = FileListPanel::new();
        *self.split.right_mut() = GitDiffPanel::new();
        self.comments.clear();
        self.split.focus_left();
    }

    pub fn load_document(&mut self, doc: GitDiffDocument) {
        self.apply_loaded_document(doc, None);
    }

    pub fn set_load_state(&mut self, state: GitDiffLoadState) {
        self.load_state = state;
    }
}

impl Component for GitDiffMode {
    type Message = GitDiffViewMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<GitDiffViewMessage>> {
        if self.split.right().is_in_comment_mode() {
            return Some(self.on_comment_mode_event(event).await);
        }

        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Esc => return Some(vec![GitDiffViewMessage::Close]),
                KeyCode::Char('r') => return Some(vec![GitDiffViewMessage::Refresh]),
                KeyCode::Char('u') => {
                    self.comments.pop();
                    self.sync_comment_state();
                    self.split.right_mut().invalidate_comment_splices();
                    return Some(vec![]);
                }
                KeyCode::Char('s') if !self.split.is_left_focused() => {
                    return Some(self.submit_review());
                }
                KeyCode::Char('h') | KeyCode::Left if !self.split.is_left_focused() => {
                    self.split.focus_left();
                    return Some(vec![]);
                }
                _ => {}
            }
        }

        self.split.on_event(event).await.map(|msgs| self.handle_split_messages(msgs))
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let theme = &context.theme;
        if context.size.width < 10 {
            return Frame::new(vec![Line::new("Too narrow")]);
        }

        let body_height = context.size.height.saturating_sub(1);
        let body_context = context.with_size((context.size.width, body_height));

        let status_msg = match &self.load_state {
            GitDiffLoadState::Loading => Some("Loading...".to_string()),
            GitDiffLoadState::Empty => Some("No changes in working tree relative to HEAD".to_string()),
            GitDiffLoadState::Ready(doc) if doc.files.is_empty() => {
                Some("No changes in working tree relative to HEAD".to_string())
            }
            GitDiffLoadState::Error { message } => Some(format!("Git diff unavailable: {message}")),
            GitDiffLoadState::Ready(_) => None,
        };

        let body = if let Some(msg) = status_msg {
            let height = body_height as usize;
            let widths = self.split.widths(context.size.width);
            let left_width = widths.left as usize;
            let mut rows = Vec::with_capacity(height);
            for i in 0..height {
                let mut line = Line::default();
                line.push_text(" ".repeat(left_width));
                line.push_with_style("│", Style::fg(theme.muted()));
                if i == 0 {
                    line.push_with_style(&msg, Style::fg(theme.text_secondary()));
                }
                rows.push(line);
            }
            Frame::new(rows)
        } else {
            let left_focused = self.split.is_left_focused();
            self.split.left_mut().set_focused(left_focused);
            self.split.right_mut().set_focused(!left_focused);
            self.prepare_right_panel_layers(&body_context);
            self.split.set_separator_style(Style::fg(theme.muted()));
            self.split.render(&body_context)
        };

        let help_keys: &[(&str, &str)] = if self.split.is_left_focused() { &LEFT_HELP_KEYS } else { &RIGHT_HELP_KEYS };
        Frame::vstack([body, Frame::new(vec![render_key_hints(theme, help_keys)])])
    }
}

impl GitDiffMode {
    fn prepare_right_panel_layers(&mut self, context: &ViewContext) {
        let GitDiffLoadState::Ready(doc) = &self.load_state else {
            return;
        };

        let selected = self.split.left().selected_file_index().unwrap_or(0).min(doc.files.len().saturating_sub(1));
        let file = &doc.files[selected];

        let file_comments = self.comments.for_file(&file.path);

        let right_width = self.split.widths(context.size.width).right;
        self.split.right_mut().ensure_layers(file, &file_comments, right_width, self.document_revision);
    }

    fn on_file_selected(&mut self, idx: usize) {
        self.split.left_mut().select_file_index(idx);
        self.split.right_mut().reset_for_new_file();
    }

    async fn on_comment_mode_event(&mut self, event: &Event) -> Vec<GitDiffViewMessage> {
        if let Some(msgs) = self.split.right_mut().on_event(event).await {
            return self.handle_right_panel_messages(msgs);
        }
        vec![]
    }

    fn handle_split_messages(
        &mut self,
        msgs: Vec<Either<FileListMessage, GitDiffPanelMessage>>,
    ) -> Vec<GitDiffViewMessage> {
        let mut right_msgs = Vec::new();
        for msg in msgs {
            match msg {
                Either::Left(FileListMessage::Selected(idx)) => {
                    self.on_file_selected(idx);
                }
                Either::Left(FileListMessage::FileOpened(idx)) => {
                    self.on_file_selected(idx);
                    self.split.focus_right();
                }
                Either::Right(panel_msg) => right_msgs.push(panel_msg),
            }
        }
        self.handle_right_panel_messages(right_msgs)
    }

    fn handle_right_panel_messages(&mut self, msgs: Vec<GitDiffPanelMessage>) -> Vec<GitDiffViewMessage> {
        for msg in msgs {
            let GitDiffPanelMessage::CommentSubmitted { anchor, text } = msg;
            self.queue_comment(anchor, &text);
        }
        vec![]
    }

    fn queue_comment(&mut self, anchor: DiffAnchor, text: &str) {
        let GitDiffLoadState::Ready(doc) = &self.load_state else {
            return;
        };
        let CommentAnchor(PatchAnchor { hunk: hunk_index, line: line_index }) = anchor;
        let selected = self.split.left().selected_file_index().unwrap_or(0);
        let Some(file) = doc.files.get(selected) else {
            return;
        };
        let Some(hunk) = file.hunks.get(hunk_index) else {
            return;
        };
        let Some(patch_line) = hunk.lines.get(line_index) else {
            return;
        };

        self.comments.push(QueuedComment {
            review: ReviewComment::new(anchor, text),
            context: GitDiffCommentContext {
                file_path: file.path.clone(),
                line_text: patch_line.text.clone(),
                line_number: patch_line.new_line_no.or(patch_line.old_line_no),
                line_kind: patch_line.kind,
            },
        });
        self.sync_comment_state();
        self.split.right_mut().invalidate_comment_splices();
    }

    fn sync_comment_state(&mut self) {
        let counts = match &self.load_state {
            GitDiffLoadState::Ready(doc) => self.comments.counts_for(&doc.files),
            _ => vec![],
        };
        self.split.left_mut().sync_view_state(self.comments.len(), counts);
    }

    fn submit_review(&self) -> Vec<GitDiffViewMessage> {
        if self.comments.is_empty() {
            return vec![];
        }
        vec![GitDiffViewMessage::SubmitPrompt(self.comments.format_prompt())]
    }

    fn selected_file_path(&self) -> Option<&str> {
        let GitDiffLoadState::Ready(doc) = &self.load_state else {
            return None;
        };
        let idx = self.split.left().selected_file_index()?;
        doc.files.get(idx).map(|f| f.path.as_str())
    }

    fn apply_loaded_document(&mut self, doc: GitDiffDocument, restore: Option<RefreshState>) {
        self.document_revision = self.document_revision.saturating_add(1);

        if doc.files.is_empty() {
            self.load_state = GitDiffLoadState::Empty;
            self.split.right_mut().clear_rendered_patches();
            return;
        }

        self.split.left_mut().rebuild_from_files(&doc.files);
        self.split.right_mut().clear_rendered_patches();
        self.split.right_mut().set_repo_root(doc.repo_root.clone());

        if let Some(restore) = restore {
            if restore.was_right_focused {
                self.split.focus_right();
            } else {
                self.split.focus_left();
            }
            self.split.right_mut().reset_scroll();
            if let Some(path) = &restore.selected_path
                && let Some(idx) = doc.files.iter().position(|file| file.path == *path)
            {
                self.split.left_mut().select_file_index(idx);
            }
        }

        self.load_state = GitDiffLoadState::Ready(doc);
        self.sync_comment_state();
    }
}

const LEFT_HELP_KEYS: [(&str, &str); 6] =
    [("j/k", "move"), ("h/l", "fold/open"), ("enter", "view"), ("u", "undo"), ("r", "refresh"), ("Esc", "close")];

const RIGHT_HELP_KEYS: [(&str, &str); 8] = [
    ("j/k", "move"),
    ("h", "back"),
    ("c", "comment"),
    ("s", "submit"),
    ("o", "full file"),
    ("u", "undo"),
    ("r", "refresh"),
    ("Esc", "close"),
];

#[derive(Default)]
struct ReviewQueue {
    comments: Vec<QueuedComment>,
}

impl ReviewQueue {
    fn is_empty(&self) -> bool {
        self.comments.is_empty()
    }

    fn len(&self) -> usize {
        self.comments.len()
    }

    fn clear(&mut self) {
        self.comments.clear();
    }

    fn push(&mut self, comment: QueuedComment) {
        self.comments.push(comment);
    }

    fn pop(&mut self) -> Option<QueuedComment> {
        self.comments.pop()
    }

    fn for_file(&self, path: &str) -> Vec<&QueuedComment> {
        self.comments.iter().filter(|comment| comment.context.file_path == path).collect()
    }

    fn counts_for(&self, files: &[crate::git_diff::FileDiff]) -> Vec<usize> {
        files
            .iter()
            .map(|file| self.comments.iter().filter(|comment| comment.context.file_path == file.path).count())
            .collect()
    }

    fn format_prompt(&self) -> String {
        use std::fmt::Write;

        let mut prompt = String::from("I'm reviewing the working tree diff. Here are my comments:\n");
        let mut file_order: Vec<&str> = Vec::new();
        let mut grouped: HashMap<&str, Vec<&QueuedComment>> = HashMap::new();

        for comment in &self.comments {
            let path = comment.context.file_path.as_str();
            if !grouped.contains_key(path) {
                file_order.push(path);
            }
            grouped.entry(path).or_default().push(comment);
        }

        for file_path in file_order {
            let file_comments = grouped.get(file_path).expect("group exists for ordered path");
            write!(prompt, "\n## `{file_path}`\n").unwrap();

            for comment in file_comments {
                let kind_label = match comment.context.line_kind {
                    PatchLineKind::Added => "added",
                    PatchLineKind::Removed => "removed",
                    PatchLineKind::Context => "context",
                    PatchLineKind::HunkHeader => "header",
                    PatchLineKind::Meta => "meta",
                };
                let line_ref = match comment.context.line_number {
                    Some(n) => format!("Line {n} ({kind_label})"),
                    None => kind_label.to_string(),
                };
                write!(prompt, "\n**{line_ref}:** `{}`\n> {}\n", comment.context.line_text, comment.review.body)
                    .unwrap();
            }
        }

        prompt
    }
}

struct RefreshState {
    selected_path: Option<String>,
    was_right_focused: bool,
}
