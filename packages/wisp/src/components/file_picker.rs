use ignore::WalkBuilder;
use std::env::current_dir;
use std::path::{Path, PathBuf};
use tui::{Combobox, Component, Event, Frame, Line, PickerMessage, Searchable, ViewContext};

const MAX_INDEXED_FILES: usize = 50_000;

#[doc = include_str!("../docs/file_picker.md")]
pub struct FilePicker {
    combobox: Combobox<FileMatch>,
}

pub type FilePickerMessage = PickerMessage<FileMatch>;

#[derive(Debug, Clone)]
pub struct FileMatch {
    pub path: PathBuf,
    pub display_name: String,
}

impl Searchable for FileMatch {
    fn search_text(&self) -> String {
        self.display_name.clone()
    }
}

impl FilePicker {
    pub fn new() -> Self {
        let root = current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut entries = Vec::new();

        let walker = WalkBuilder::new(&root)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .hidden(false)
            .parents(true)
            .build();

        for entry in walker.flatten().take(MAX_INDEXED_FILES) {
            let path = entry.path();
            if !entry.file_type().is_some_and(|ft| ft.is_file()) || should_exclude_path(path) {
                continue;
            }

            let display_name = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().replace('\\', "/");

            entries.push(FileMatch { path: path.to_path_buf(), display_name });
        }

        entries.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        Self { combobox: Combobox::new(entries) }
    }

    pub fn query(&self) -> &str {
        self.combobox.query()
    }

    pub fn new_with_entries(entries: Vec<FileMatch>) -> Self {
        Self { combobox: Combobox::new(entries) }
    }
}

impl Default for FilePicker {
    fn default() -> Self {
        Self::new()
    }
}

fn should_exclude_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}

impl Component for FilePicker {
    type Message = FilePickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        self.combobox.handle_picker_event(event)
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let mut lines = Vec::new();

        if self.combobox.is_empty() {
            lines.push(Line::new("  (no matches found)".to_string()));
            return Frame::new(lines);
        }

        let item_lines = self.combobox.render_items(context, |file, is_selected, ctx| {
            let line_text = file.display_name.clone();
            if is_selected { ctx.theme.selected_row_line(line_text) } else { Line::new(line_text) }
        });
        lines.extend(item_lines);

        Frame::new(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::ViewContext;
    use tui::test_picker::{rendered_lines_from, rendered_raw_lines_with_context, type_query};
    use tui::{KeyCode, KeyEvent, KeyModifiers};

    const DEFAULT_SIZE: (u16, u16) = (120, 40);

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn file_match(path: &str) -> FileMatch {
        FileMatch { path: PathBuf::from(path), display_name: path.to_string() }
    }

    fn selected_text(picker: &mut FilePicker) -> Option<String> {
        let context = ViewContext::new(DEFAULT_SIZE);
        let frame = picker.render(&context);
        let highlight_bg = context.theme.highlight_bg();
        frame
            .lines()
            .iter()
            .find(|line| {
                line.fill() == Some(highlight_bg) || line.spans().iter().any(|s| s.style().bg == Some(highlight_bg))
            })
            .map(tui::Line::plain_text)
    }

    #[test]
    fn excludes_vcs_and_build_paths() {
        assert!(should_exclude_path(Path::new(".git/config")));
        assert!(should_exclude_path(Path::new(".hg/store")));
        assert!(should_exclude_path(Path::new(".svn/entries")));
        assert!(should_exclude_path(Path::new("node_modules/react/index.js")));
        assert!(should_exclude_path(Path::new("target/debug/wisp")));
        assert!(!should_exclude_path(Path::new("src/main.rs")));
        assert!(!should_exclude_path(Path::new(".github/pull_request_template.md")));
        assert!(!should_exclude_path(Path::new(".vscode/settings.json")));
    }

    #[tokio::test]
    async fn query_filters_matches() {
        let mut picker = FilePicker::new_with_entries(vec![
            file_match("src/main.rs"),
            file_match("src/renderer.rs"),
            file_match("README.md"),
        ]);

        type_query(&mut picker, "rend").await;

        let lines = rendered_lines_from(&picker.render(&ViewContext::new(DEFAULT_SIZE)));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("src/renderer.rs"));
    }

    #[tokio::test]
    async fn selection_wraps() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("a.rs"), file_match("b.rs"), file_match("c.rs")]);

        let first = selected_text(&mut picker).unwrap();

        picker.on_event(&Event::Key(key(KeyCode::Up))).await;
        let last = selected_text(&mut picker).unwrap();
        assert_ne!(first, last);

        picker.on_event(&Event::Key(key(KeyCode::Down))).await;
        let back_to_first = selected_text(&mut picker).unwrap();
        assert_eq!(first, back_to_first);
    }

    #[test]
    fn selected_entry_has_highlight_background() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("a.rs"), file_match("b.rs"), file_match("c.rs")]);
        let context = ViewContext::new((80, 24));
        let frame = picker.render(&context);
        let selected_line = frame
            .lines()
            .iter()
            .find(|line| line.plain_text().starts_with("  "))
            .expect("should render a selected line");

        let has_bg = selected_line.spans().iter().any(|span| span.style().bg == Some(context.theme.highlight_bg()));
        assert!(has_bg, "selected entry should have highlight background");
    }

    #[test]
    fn selected_entry_has_text_primary_foreground() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("a.rs")]);
        let context = ViewContext::new((80, 24));
        let lines = rendered_raw_lines_with_context(|ctx| picker.render(ctx), (80, 24));
        let selected_line =
            lines.iter().find(|line| line.plain_text().starts_with("  ")).expect("should render a selected line");

        let has_fg = selected_line.spans().iter().any(|span| span.style().fg == Some(context.theme.text_primary()));
        assert!(has_fg, "selected entry should have text_primary foreground");
    }

    #[test]
    fn selected_entry_highlight_fills_full_line_width() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("a.rs")]);
        let context = ViewContext::new((20, 24));
        let highlight_bg = context.theme.highlight_bg();
        let term = tui::testing::render_component(|ctx| picker.render(ctx), 20, 24);
        let row = term.get_lines().iter().position(|l| l.starts_with("  a.rs")).expect("should render a selected line");
        let last_col_style = term.get_style_at(row, 19);
        assert_eq!(
            last_col_style.bg,
            Some(highlight_bg),
            "selected row should fill the full visible width with highlight background",
        );
    }

    #[tokio::test]
    async fn handle_key_char_updates_query_and_returns_char_typed() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/renderer.rs")]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Char('r')))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CharTyped('r')]));
        assert_eq!(picker.query(), "r");
    }

    #[tokio::test]
    async fn handle_key_whitespace_closes_picker() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/main.rs")]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Char(' ')))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CloseWithChar(' ')]));
    }

    #[tokio::test]
    async fn handle_key_enter_requests_confirmation() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/main.rs")]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Enter))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::Confirm(_)]));
    }

    #[tokio::test]
    async fn handle_key_tab_requests_confirmation() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/main.rs")]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Tab))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::Confirm(_)]));
    }

    #[tokio::test]
    async fn handle_key_tab_with_no_matches_closes() {
        let mut picker = FilePicker::new_with_entries(vec![]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Tab))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::Close]));
    }

    #[tokio::test]
    async fn backspace_with_empty_query_closes_and_pops() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/main.rs")]);

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::CloseAndPopChar]));
    }

    #[tokio::test]
    async fn backspace_with_query_pops_char() {
        let mut picker = FilePicker::new_with_entries(vec![file_match("src/main.rs")]);
        type_query(&mut picker, "ma").await;

        let outcome = picker.on_event(&Event::Key(key(KeyCode::Backspace))).await;

        assert!(outcome.is_some());

        assert!(matches!(outcome.unwrap().as_slice(), [PickerMessage::PopChar]));
        assert_eq!(picker.query(), "m");
    }
}
