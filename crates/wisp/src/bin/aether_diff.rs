#[path = "aether_diff/text_input.rs"]
mod text_input;

use gpui::{
    App, Application, Bounds, ClipboardItem, Context, Entity, Focusable, HighlightStyle, KeyBinding, SharedString,
    StyledText, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb, size, uniform_list,
};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use text_input::CommentInput;
use thiserror::Error;
use wisp::git_review::{
    CommentContext, DiffDocument, DiffScope, EMPTY_TREE, FileDiff, FileStatus, PatchAnchor, PatchLine, PatchLineKind,
    QueuedComment, ReviewQueue, StageState,
};

actions!(diff_review, [SubmitComment, CancelComment, CopyReview]);

const BACKGROUND: u32 = 0x0f1117;
const PANEL: u32 = 0x151821;
const PANEL_HOVER: u32 = 0x202532;
const BORDER: u32 = 0x2a3040;
const TEXT: u32 = 0xc8d0df;
const MUTED: u32 = 0x727b8d;
const ADDED_BG: u32 = 0x12291f;
const REMOVED_BG: u32 = 0x321a20;
const HEADER_BG: u32 = 0x18263b;
const ADDED: u32 = 0x63d297;
const REMOVED: u32 = 0xf27983;
const ACCENT: u32 = 0x7aa2f7;
const ROW_HEIGHT: f32 = 32.0;

static AYU_DARK_THEME: LazyLock<Theme> = LazyLock::new(|| {
    let cursor = std::io::Cursor::new(include_bytes!("../../assets/ayu-dark.tmTheme"));
    ThemeSet::load_from_reader(&mut std::io::BufReader::new(cursor)).expect("embedded ayu-dark.tmTheme is valid")
});

#[derive(Debug, Error)]
enum AppError {
    #[error("Not a git repository: {0}")]
    NotARepository(PathBuf),
    #[error("Could not run git: {0}")]
    GitIo(#[from] std::io::Error),
    #[error("Git command failed: {0}")]
    Git(String),
    #[error("Could not parse git diff: {0}")]
    Parse(#[from] wisp::git_review::GitDiffError),
}

#[derive(Clone)]
struct SplitRow {
    kind: RowKind,
    left: Option<DiffCell>,
    right: Option<DiffCell>,
}

#[derive(Clone)]
enum ReviewRow {
    Diff(SplitRow),
    Comment(QueuedComment),
    Draft(CommentTarget, Entity<CommentInput>),
}

#[derive(Clone)]
struct CommentTarget {
    anchor: PatchAnchor,
    context: CommentContext,
    side: DiffSide,
}

struct CommentDraft {
    target: CommentTarget,
    input: Entity<CommentInput>,
}

#[derive(Clone)]
struct DiffCell {
    line_number: Option<usize>,
    text: SharedString,
    highlights: Vec<(Range<usize>, u32)>,
    tone: CellTone,
    comment: Option<CommentTarget>,
}

#[derive(Clone, Copy)]
enum RowKind {
    Header,
    Code,
}

#[derive(Clone, Copy)]
enum CellTone {
    Context,
    Added,
    Removed,
    Empty,
}

#[derive(Clone, Copy)]
enum DiffSide {
    Left,
    Right,
}

struct DiffApp {
    document: DiffDocument,
    selected_file: usize,
    diff_rows: Vec<SplitRow>,
    rows: Vec<ReviewRow>,
    review: ReviewQueue,
    draft: Option<CommentDraft>,
}

impl DiffApp {
    fn new(document: DiffDocument) -> Self {
        let diff_rows = document.files.first().map(|file| build_split_rows(0, file)).unwrap_or_default();
        let rows = diff_rows.iter().cloned().map(ReviewRow::Diff).collect();
        Self { document, selected_file: 0, diff_rows, rows, review: ReviewQueue::default(), draft: None }
    }

    fn selected_file(&self) -> Option<&FileDiff> {
        self.document.files.get(self.selected_file)
    }

    fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected_file = index;
        self.diff_rows = self.document.files.get(index).map(|file| build_split_rows(index, file)).unwrap_or_default();
        self.draft = None;
        self.rebuild_rows();
        cx.notify();
    }

    fn begin_comment(&mut self, target: CommentTarget, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(CommentInput::new);
        window.focus(&input.focus_handle(cx));
        self.draft = Some(CommentDraft { target, input });
        self.rebuild_rows();
        cx.notify();
    }

    fn submit_comment(&mut self, _: &SubmitComment, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        let body = draft.input.read(cx).content().trim().to_string();
        if !body.is_empty() {
            self.review.push(QueuedComment { anchor: draft.target.anchor, body, context: draft.target.context });
        }
        self.rebuild_rows();
        cx.notify();
    }

    fn cancel_comment(&mut self, _: &CancelComment, _window: &mut Window, cx: &mut Context<Self>) {
        self.draft = None;
        self.rebuild_rows();
        cx.notify();
    }

    fn copy_review(&mut self, _: &CopyReview, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.review.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.review.format_prompt()));
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for row in &self.diff_rows {
            rows.push(ReviewRow::Diff(row.clone()));
            for target in comment_targets(row) {
                rows.extend(
                    self.review
                        .comments()
                        .iter()
                        .filter(|comment| comment.anchor == target.anchor)
                        .cloned()
                        .map(ReviewRow::Comment),
                );
                if let Some(draft) = self.draft.as_ref().filter(|draft| draft.target.anchor == target.anchor) {
                    rows.push(ReviewRow::Draft(draft.target.clone(), draft.input.clone()));
                }
            }
        }
        self.rows = rows;
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut sidebar = div()
            .w(px(280.0))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(BORDER));

        sidebar = sidebar.child(
            div()
                .h(px(52.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px_4()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(format!("CHANGED FILES  {}", self.document.files.len())),
        );

        let mut files = div().id("changed-files").flex_1().overflow_y_scroll().py_2();
        for (index, file) in self.document.files.iter().enumerate() {
            let selected = index == self.selected_file;
            let status_color = match file.status {
                FileStatus::Added | FileStatus::Untracked => ADDED,
                FileStatus::Deleted => REMOVED,
                FileStatus::Modified | FileStatus::Renamed => ACCENT,
            };
            let stage = match file.staged {
                StageState::Staged => "●",
                StageState::PartiallyStaged => "◐",
                StageState::Unstaged => "○",
            };
            files = files.child(
                div()
                    .id(("file", index))
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .cursor_pointer()
                    .when(selected, |row| row.bg(rgb(PANEL_HOVER)))
                    .hover(|row| row.bg(rgb(PANEL_HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_file(index, cx)))
                    .child(div().w(px(14.0)).text_color(rgb(status_color)).child(file.status.marker().to_string()))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(file.path.clone()))
                    .child(div().text_xs().text_color(rgb(MUTED)).child(format!(
                        "+{} −{} {stage}",
                        file.additions(),
                        file.deletions()
                    ))),
            );
        }
        sidebar.child(files)
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(file) = self.selected_file() else {
            return div()
                .flex_1()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(MUTED))
                .child("No working tree changes");
        };

        let header = div()
            .h(px(52.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .px_4()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(file.path.clone()))
            .child(div().text_color(rgb(ADDED)).child(format!("+{}", file.additions())))
            .child(div().ml_2().text_color(rgb(REMOVED)).child(format!("−{}", file.deletions())))
            .child(
                div()
                    .ml_4()
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(format!("{} comments · click a line to comment", self.review.len())),
            )
            .when(!self.review.is_empty(), |header| {
                header.child(
                    div()
                        .id("copy-review")
                        .ml_3()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgb(PANEL_HOVER))
                        .text_xs()
                        .text_color(rgb(ACCENT))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, window, cx| this.copy_review(&CopyReview, window, cx)))
                        .child("Copy review"),
                )
            });

        let column_headers = div()
            .h(px(30.0))
            .flex_shrink_0()
            .flex()
            .bg(rgb(PANEL))
            .border_b_1()
            .border_color(rgb(BORDER))
            .text_xs()
            .text_color(rgb(MUTED))
            .child(
                div()
                    .w_1_2()
                    .h_full()
                    .flex()
                    .items_center()
                    .px_3()
                    .border_r_1()
                    .border_color(rgb(BORDER))
                    .child(file.old_path.clone().unwrap_or_else(|| file.path.clone())),
            )
            .child(div().w_1_2().h_full().flex().items_center().px_3().child(file.path.clone()));

        let row_count = self.rows.len();
        let list = uniform_list(
            "split-diff-rows",
            row_count,
            cx.processor(|this, range: Range<usize>, _window, cx| {
                range
                    .filter_map(|index| this.rows.get(index).cloned().map(|row| render_review_row(index, &row, cx)))
                    .collect()
            }),
        )
        .h_full();

        div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(header)
            .child(column_headers)
            .child(div().flex_1().overflow_hidden().child(list))
    }
}

impl Render for DiffApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("DiffReview")
            .on_action(cx.listener(Self::submit_comment))
            .on_action(cx.listener(Self::cancel_comment))
            .on_action(cx.listener(Self::copy_review))
            .size_full()
            .flex()
            .bg(rgb(BACKGROUND))
            .text_color(rgb(TEXT))
            .font_family("Menlo")
            .text_size(px(13.0))
            .child(self.render_sidebar(cx))
            .child(self.render_diff(cx))
    }
}

fn render_review_row(index: usize, row: &ReviewRow, cx: &mut Context<DiffApp>) -> gpui::AnyElement {
    match row {
        ReviewRow::Diff(row) => render_diff_row(index, row, cx),
        ReviewRow::Comment(comment) => render_comment_row(index, comment),
        ReviewRow::Draft(target, input) => render_draft_row(index, target, input, cx),
    }
}

fn render_diff_row(index: usize, row: &SplitRow, cx: &mut Context<DiffApp>) -> gpui::AnyElement {
    match row.kind {
        RowKind::Header => {
            let text = row.left.as_ref().map(|cell| cell.text.clone()).unwrap_or_default();
            div()
                .id(("row", index))
                .h(px(ROW_HEIGHT))
                .w_full()
                .flex()
                .items_center()
                .px_3()
                .bg(rgb(HEADER_BG))
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(ACCENT))
                .child(text)
                .into_any_element()
        }
        RowKind::Code => div()
            .id(("row", index))
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex()
            .child(render_cell(index, row.left.as_ref(), true, cx))
            .child(render_cell(index, row.right.as_ref(), false, cx))
            .into_any_element(),
    }
}

fn render_cell(index: usize, cell: Option<&DiffCell>, left: bool, cx: &mut Context<DiffApp>) -> gpui::AnyElement {
    let (line_number, text, highlights, tone) = match cell {
        Some(cell) => (cell.line_number, cell.text.clone(), cell.highlights.clone(), cell.tone),
        None => (None, SharedString::default(), Vec::new(), CellTone::Empty),
    };
    let background = match tone {
        CellTone::Context => BACKGROUND,
        CellTone::Added => ADDED_BG,
        CellTone::Removed => REMOVED_BG,
        CellTone::Empty => PANEL,
    };
    let marker = match tone {
        CellTone::Added => "+",
        CellTone::Removed => "−",
        CellTone::Context | CellTone::Empty => " ",
    };
    let styled = StyledText::new(text).with_highlights(
        highlights.into_iter().map(|(range, color)| (range, HighlightStyle::color(rgb(color).into()))),
    );

    let target = cell.and_then(|cell| cell.comment.clone());
    div()
        .id((if left { "left-cell" } else { "right-cell" }, index))
        .w_1_2()
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .bg(rgb(background))
        .when(left, |pane| pane.border_r_1().border_color(rgb(BORDER)))
        .when_some(target, |pane, target| {
            pane.cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL_HOVER)))
                .on_click(cx.listener(move |this, _, window, cx| this.begin_comment(target.clone(), window, cx)))
        })
        .child(
            div()
                .w(px(52.0))
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_end()
                .pr_2()
                .bg(rgb(PANEL))
                .text_color(rgb(MUTED))
                .child(line_number.map(|number| number.to_string()).unwrap_or_default()),
        )
        .child(div().w(px(22.0)).flex_shrink_0().text_color(rgb(MUTED)).text_center().child(marker))
        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(styled))
        .into_any_element()
}

fn render_comment_row(index: usize, comment: &QueuedComment) -> gpui::AnyElement {
    let line = comment.context.line_number.map(|line| format!("line {line}"));
    div()
        .id(("comment", index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .px_4()
        .bg(rgb(0x172033))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().text_color(rgb(ACCENT)).font_weight(gpui::FontWeight::SEMIBOLD).child("COMMENT"))
        .when_some(line, |row, line| row.child(div().text_xs().text_color(rgb(MUTED)).child(line)))
        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(comment.body.clone()))
        .into_any_element()
}

fn render_draft_row(
    index: usize,
    target: &CommentTarget,
    input: &Entity<CommentInput>,
    cx: &mut Context<DiffApp>,
) -> gpui::AnyElement {
    let side = match target.side {
        DiffSide::Left => "OLD",
        DiffSide::Right => "NEW",
    };
    div()
        .id(("draft", index))
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .bg(rgb(0x172033))
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().w(px(38.0)).text_xs().text_color(rgb(ACCENT)).child(side))
        .child(div().flex_1().child(input.clone()))
        .child(
            div()
                .id(("add-comment", index))
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(ACCENT))
                .text_color(rgb(BACKGROUND))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.submit_comment(&SubmitComment, window, cx)))
                .child("Add"),
        )
        .child(
            div()
                .id(("cancel-comment", index))
                .px_2()
                .py_1()
                .text_color(rgb(MUTED))
                .cursor_pointer()
                .on_click(cx.listener(|this, _, window, cx| this.cancel_comment(&CancelComment, window, cx)))
                .child("Cancel"),
        )
        .into_any_element()
}

fn comment_targets(row: &SplitRow) -> Vec<CommentTarget> {
    let mut targets = Vec::new();
    for target in row.left.iter().chain(row.right.iter()).filter_map(|cell| cell.comment.clone()) {
        if targets.iter().all(|existing: &CommentTarget| existing.anchor != target.anchor) {
            targets.push(target);
        }
    }
    targets
}

fn build_split_rows(file_index: usize, file: &FileDiff) -> Vec<SplitRow> {
    if file.binary {
        return vec![header_row("Binary file")];
    }

    let mut rows = Vec::new();
    let highlighter = SyntaxHighlighter::new(file.language());
    for (hunk_index, hunk) in file.hunks.iter().enumerate() {
        rows.push(header_row(&hunk.header));
        let lines: Vec<(usize, &PatchLine)> =
            hunk.lines.iter().enumerate().filter(|(_, line)| line.kind != PatchLineKind::HunkHeader).collect();
        let mut index = 0;
        while index < lines.len() {
            match lines[index].1.kind {
                PatchLineKind::Context => {
                    let (line_index, line) = lines[index];
                    rows.push(SplitRow {
                        kind: RowKind::Code,
                        left: Some(highlighter.cell(
                            file_index,
                            hunk_index,
                            line_index,
                            file,
                            line,
                            CellTone::Context,
                            DiffSide::Left,
                        )),
                        right: Some(highlighter.cell(
                            file_index,
                            hunk_index,
                            line_index,
                            file,
                            line,
                            CellTone::Context,
                            DiffSide::Right,
                        )),
                    });
                    index += 1;
                }
                PatchLineKind::Removed | PatchLineKind::Added => {
                    let start = index;
                    while index < lines.len()
                        && matches!(lines[index].1.kind, PatchLineKind::Removed | PatchLineKind::Added)
                    {
                        index += 1;
                    }
                    let block = &lines[start..index];
                    let removed: Vec<_> =
                        block.iter().copied().filter(|(_, line)| line.kind == PatchLineKind::Removed).collect();
                    let added: Vec<_> =
                        block.iter().copied().filter(|(_, line)| line.kind == PatchLineKind::Added).collect();
                    for offset in 0..removed.len().max(added.len()) {
                        rows.push(SplitRow {
                            kind: RowKind::Code,
                            left: removed.get(offset).map(|(line_index, line)| {
                                highlighter.cell(
                                    file_index,
                                    hunk_index,
                                    *line_index,
                                    file,
                                    line,
                                    CellTone::Removed,
                                    DiffSide::Left,
                                )
                            }),
                            right: added.get(offset).map(|(line_index, line)| {
                                highlighter.cell(
                                    file_index,
                                    hunk_index,
                                    *line_index,
                                    file,
                                    line,
                                    CellTone::Added,
                                    DiffSide::Right,
                                )
                            }),
                        });
                    }
                }
                PatchLineKind::Meta => {
                    rows.push(header_row(&lines[index].1.text));
                    index += 1;
                }
                PatchLineKind::HunkHeader => index += 1,
            }
        }
    }
    rows
}

fn header_row(text: &str) -> SplitRow {
    SplitRow {
        kind: RowKind::Header,
        left: Some(DiffCell {
            line_number: None,
            text: text.to_string().into(),
            highlights: Vec::new(),
            tone: CellTone::Context,
            comment: None,
        }),
        right: None,
    }
}

struct SyntaxHighlighter {
    syntax_set: SyntaxSet,
    theme: syntect::highlighting::Theme,
    extension: String,
}

impl SyntaxHighlighter {
    fn new(extension: &str) -> Self {
        Self {
            syntax_set: two_face::syntax::extra_newlines(),
            theme: AYU_DARK_THEME.clone(),
            extension: extension.to_string(),
        }
    }

    fn cell(
        &self,
        file_index: usize,
        hunk: usize,
        line_index: usize,
        file: &FileDiff,
        line: &PatchLine,
        tone: CellTone,
        side: DiffSide,
    ) -> DiffCell {
        let syntax = self
            .syntax_set
            .find_syntax_by_extension(&self.extension)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());
        let mut highlighter = HighlightLines::new(syntax, &self.theme);
        let source = format!("{}\n", line.text);
        let highlights = highlighter
            .highlight_line(&source, &self.syntax_set)
            .unwrap_or_default()
            .into_iter()
            .scan(0, |offset, (style, segment)| {
                let start = *offset;
                let end = start + segment.len().min(line.text.len().saturating_sub(start));
                *offset += segment.len();
                (start < end).then_some((start..end, syntect_color(style.foreground)))
            })
            .collect();
        let line_number = match side {
            DiffSide::Left => line.old_line_no,
            DiffSide::Right => line.new_line_no,
        };
        DiffCell {
            line_number,
            text: line.text.clone().into(),
            highlights,
            tone,
            comment: Some(CommentTarget {
                anchor: PatchAnchor { file_index, hunk, line: line_index },
                context: CommentContext {
                    file_path: file.path.clone(),
                    line_text: line.text.clone(),
                    line_number,
                    line_kind: line.kind,
                },
                side,
            }),
        }
    }
}

fn syntect_color(color: Color) -> u32 {
    (u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)
}

fn load_working_tree(path: &Path) -> Result<DiffDocument, AppError> {
    let root_output =
        run_git(path, &["rev-parse", "--show-toplevel"]).map_err(|_| AppError::NotARepository(path.to_path_buf()))?;
    let repo_root = PathBuf::from(String::from_utf8_lossy(&root_output.stdout).trim());
    let head_exists = Command::new("git")
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .current_dir(&repo_root)
        .output()
        .is_ok_and(|output| output.status.success());
    let base = if head_exists { "HEAD" } else { EMPTY_TREE };
    let diff = run_git(&repo_root, &["diff", "--no-ext-diff", "--find-renames", base])?;
    let status = run_git(&repo_root, &["status", "--porcelain=v1", "-z"])?;
    let untracked_paths = run_git(&repo_root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let untracked = untracked_paths
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let relative = String::from_utf8_lossy(path).into_owned();
            let bytes = std::fs::read(repo_root.join(&relative)).unwrap_or_default();
            (relative, bytes)
        })
        .collect::<Vec<_>>();

    DiffDocument::from_git_output(
        repo_root,
        &String::from_utf8_lossy(&diff.stdout),
        &String::from_utf8_lossy(&status.stdout),
        untracked,
        DiffScope::Both,
    )
    .map_err(AppError::from)
}

fn run_git(repo_root: &Path, args: &[&str]) -> Result<Output, AppError> {
    let output = Command::new("git").args(args).current_dir(repo_root).output()?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(AppError::Git(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    }
}

fn main() {
    let path = std::env::args_os().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let document = match load_working_tree(&path) {
        Ok(document) => document,
        Err(error) => {
            eprintln!("aether-diff: {error}");
            std::process::exit(1);
        }
    };

    Application::new().run(move |cx: &mut App| {
        CommentInput::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-enter", SubmitComment, None),
            KeyBinding::new("ctrl-enter", SubmitComment, None),
            KeyBinding::new("escape", CancelComment, None),
            KeyBinding::new("cmd-shift-c", CopyReview, None),
        ]);
        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        cx.open_window(
            WindowOptions {
                titlebar: Some(gpui::TitlebarOptions { title: Some("Aether Diff".into()), ..Default::default() }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| DiffApp::new(document)),
        )
        .expect("failed to open Aether Diff window");
        cx.activate(true);
    });
}
