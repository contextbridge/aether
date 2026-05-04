use tui::testing::{assert_buffer_eq, cols, key, render_component};
use tui::{Component, Event, KeyCode, ViewContext};
use wisp::components::file_list_panel::FileListPanel;
use wisp::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, PatchLineKind};

const W: u16 = 40;

fn ev(code: KeyCode) -> Event {
    Event::Key(key(code))
}

fn file(path: &str, status: FileStatus, additions: usize, deletions: usize) -> FileDiff {
    let mut lines = Vec::new();
    for i in 0..additions {
        lines.push(PatchLine {
            kind: PatchLineKind::Added,
            text: format!("added {i}"),
            old_line_no: None,
            new_line_no: Some(i + 1),
        });
    }
    for i in 0..deletions {
        lines.push(PatchLine {
            kind: PatchLineKind::Removed,
            text: format!("removed {i}"),
            old_line_no: Some(i + 1),
            new_line_no: None,
        });
    }
    FileDiff {
        old_path: None,
        path: path.to_string(),
        status,
        hunks: if lines.is_empty() {
            vec![]
        } else {
            vec![Hunk {
                header: "@@ -1 +1 @@".to_string(),
                old_start: 1,
                old_count: deletions,
                new_start: 1,
                new_count: additions,
                lines,
            }]
        },
        binary: false,
    }
}

fn panel_with_flat_files() -> FileListPanel {
    let files = vec![file("app.rs", FileStatus::Modified, 3, 1), file("lib.rs", FileStatus::Added, 5, 0)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel
}

fn panel_with_directory() -> FileListPanel {
    let files = vec![file("src/main.rs", FileStatus::Modified, 2, 1), file("src/util.rs", FileStatus::Added, 4, 0)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel
}

fn flat_selected_first() -> [String; 4] {
    [
        cols(&[(">   M app.rs", 35), ("+3/-1", 5)]),
        cols(&[("    A lib.rs", 35), ("+5/-0", 5)]),
        String::new(),
        String::new(),
    ]
}

fn flat_selected_second() -> [String; 4] {
    [
        cols(&[("    M app.rs", 35), ("+3/-1", 5)]),
        cols(&[(">   A lib.rs", 35), ("+5/-0", 5)]),
        String::new(),
        String::new(),
    ]
}

fn dir_expanded() -> [String; 5] {
    [
        cols(&[("> \u{25be} src/", 40)]),
        cols(&[("      M main.rs", 35), ("+2/-1", 5)]),
        cols(&[("      A util.rs", 35), ("+4/-0", 5)]),
        String::new(),
        String::new(),
    ]
}

#[test]
fn renders_flat_files_with_first_selected() {
    let mut panel = panel_with_flat_files();
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_selected_first());
}

#[test]
fn renders_directory_tree() {
    let mut panel = panel_with_directory();
    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(&term, &dir_expanded());
}

#[tokio::test]
async fn navigate_down_moves_selection() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_selected_second());
}

#[tokio::test]
async fn navigate_up_moves_selection() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    panel.on_event(&ev(KeyCode::Char('k'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_selected_first());
}

#[tokio::test]
async fn navigation_wraps_around() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('k'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_selected_second());
}

#[tokio::test]
async fn collapse_directory_hides_children() {
    let mut panel = panel_with_directory();
    panel.on_event(&ev(KeyCode::Char('h'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(
        &term,
        &[cols(&[("> \u{25b8} src/", 40)]), String::new(), String::new(), String::new(), String::new()],
    );
}

#[test]
fn renders_queued_comment_indicator() {
    let files = vec![file("a.rs", FileStatus::Modified, 1, 1)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel.set_queued_comment_count(3);
    let term = render_component(|ctx| panel.render(ctx), W, 3);

    assert_buffer_eq(&term, &[cols(&[(">   M a.rs", 35), ("+1/-1", 5)]), String::new(), " [3 comments]".to_string()]);
}

#[test]
fn queued_comment_singular() {
    let files = vec![file("a.rs", FileStatus::Modified, 1, 1)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel.set_queued_comment_count(1);
    let term = render_component(|ctx| panel.render(ctx), W, 3);

    assert_buffer_eq(&term, &[cols(&[(">   M a.rs", 35), ("+1/-1", 5)]), String::new(), " [1 comment]".to_string()]);
}

#[test]
fn empty_panel_renders_blank_rows() {
    let mut panel = FileListPanel::new();
    let term = render_component(|ctx| panel.render(ctx), 30, 3);

    assert_buffer_eq(&term, &["", "", ""]);
}

#[test]
fn file_status_markers_render_correctly() {
    let files = vec![
        file("added.rs", FileStatus::Added, 1, 0),
        file("deleted.rs", FileStatus::Deleted, 0, 1),
        file("modified.rs", FileStatus::Modified, 1, 1),
    ];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(
        &term,
        &[
            cols(&[(">   A added.rs", 35), ("+1/-0", 5)]),
            cols(&[("    D deleted.rs", 35), ("+0/-1", 5)]),
            cols(&[("    M modified.rs", 35), ("+1/-1", 5)]),
            String::new(),
            String::new(),
        ],
    );
}

#[test]
fn selected_row_has_selection_style() {
    let mut panel = panel_with_flat_files();
    let ctx = ViewContext::new((W, 4));
    let term = render_component(|c| panel.render(c), W, 4);

    let selected_bg = ctx.theme.selected_row_style().bg;
    assert_eq!(term.get_style_at(0, 0).bg, selected_bg, "selected row bg");

    let non_selected_bg = Some(ctx.theme.sidebar_bg());
    assert_eq!(term.get_style_at(1, 0).bg, non_selected_bg, "non-selected row bg");
}

#[tokio::test]
async fn arrow_keys_navigate() {
    let mut panel = panel_with_flat_files();

    panel.on_event(&ev(KeyCode::Down)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);
    assert_buffer_eq(&term, &flat_selected_second());

    panel.on_event(&ev(KeyCode::Up)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);
    assert_buffer_eq(&term, &flat_selected_first());
}

#[tokio::test]
async fn renders_only_viewport_rows_after_scrolling_many_files() {
    let files = (0..20).map(|i| file(&format!("file-{i:02}.rs"), FileStatus::Modified, 1, 1)).collect::<Vec<_>>();
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);

    for _ in 0..7 {
        panel.on_event(&ev(KeyCode::Char('j'))).await;
    }

    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(
        &term,
        &[
            cols(&[("    M file-03.rs", 35), ("+1/-1", 5)]),
            cols(&[("    M file-04.rs", 35), ("+1/-1", 5)]),
            cols(&[("    M file-05.rs", 35), ("+1/-1", 5)]),
            cols(&[("    M file-06.rs", 35), ("+1/-1", 5)]),
            cols(&[(">   M file-07.rs", 35), ("+1/-1", 5)]),
        ],
    );
}
