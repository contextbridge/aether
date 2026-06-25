use tui::testing::{TestTerminal, assert_buffer_eq, cols, key, render_component};
use tui::{Component, Event, KeyCode, KeyModifiers, MouseEvent, MouseEventKind, ViewContext};
use wisp::components::file_list_panel::FileListPanel;
use wisp::git_diff::{FileDiff, FileStatus, Hunk, PatchLine, PatchLineKind, StageState};

const W: u16 = 40;

fn ev(code: KeyCode) -> Event {
    Event::Key(key(code))
}

fn mouse_scroll(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent { kind, column: 0, row: 0, modifiers: KeyModifiers::NONE })
}

fn rule() -> String {
    "─".repeat(W as usize)
}

fn assert_selected_tree_row(term: &TestTerminal, selected: usize, unselected: usize) {
    let theme = &ViewContext::new((W, 24)).theme;
    assert_eq!(term.get_style_at(selected, 1).bg, Some(theme.highlight_bg()), "row {selected} should be selected");
    assert_eq!(term.get_style_at(unselected, 1).bg, None, "row {unselected} should not be selected");
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
        staged: StageState::Unstaged,
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

fn flat_rows(selected: usize) -> [String; 4] {
    let app_indicator = if selected == 0 { "▎" } else { " " };
    let lib_indicator = if selected == 1 { "▎" } else { " " };
    [
        " Git Diff  2 files  +8 -1".to_string(),
        rule(),
        cols(&[(format!("{app_indicator}── app.rs").as_str(), 31), ("+3 -1 M ☐", 0)]),
        cols(&[(format!("{lib_indicator}── lib.rs").as_str(), 31), ("+5 -0 A ☐", 0)]),
    ]
}

#[test]
fn nested_directory_uses_compact_connector_to_align_child_branch() {
    let files = vec![
        file("src/components/button.rs", FileStatus::Modified, 1, 0),
        file("src/lib.rs", FileStatus::Modified, 1, 0),
    ];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    let lines = render_component(|ctx| panel.render(ctx), W, 8).get_lines();

    let dir_row = lines.iter().find(|line| line.contains("components/")).expect("components directory row");
    let file_row = lines.iter().find(|line| line.contains("button.rs")).expect("components child row");

    assert!(dir_row.contains("├─▾ components/"), "directory rows should use a compact connector: {dir_row:?}");
    assert!(file_row.contains("│ └── button.rs"), "child rows should advance by two columns: {file_row:?}");
}

#[test]
fn sibling_directory_and_file_align_their_connectors() {
    let files =
        vec![file("pkg/dir/inner.rs", FileStatus::Modified, 1, 0), file("pkg/file.rs", FileStatus::Modified, 1, 0)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    let lines = render_component(|ctx| panel.render(ctx), W, 8).get_lines();

    let connector_col = |needle: &str, branch: char| {
        let row = lines.iter().find(|line| line.contains(needle)).unwrap_or_else(|| panic!("{needle} row"));
        row[..row.find(branch).expect("branch char")].chars().count()
    };

    assert_eq!(
        connector_col("dir/", '├'),
        connector_col("file.rs", '└'),
        "a directory and its sibling file must draw their connector in the same column so the vertical guide stays continuous",
    );
}

#[test]
fn file_rows_keep_single_space_after_horizontal_connector() {
    let mut panel = panel_with_directory();
    let lines = render_component(|ctx| panel.render(ctx), W, 5).get_lines();
    let main_row = lines.iter().find(|line| line.contains("main.rs")).expect("main.rs row");

    assert!(
        main_row.contains("├── main.rs"),
        "file row should have one space after the horizontal guide: {main_row:?}"
    );
    assert!(!main_row.contains("├──    main.rs"), "file row should not leave an icon-sized gutter: {main_row:?}");
}

#[test]
fn child_branch_aligns_with_parent_directory_indicator() {
    let files = vec![
        file("crates/tui/src/components/text_field.rs", FileStatus::Modified, 1, 0),
        file("crates/wisp/src/lib.rs", FileStatus::Modified, 1, 0),
    ];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    let lines = render_component(|ctx| panel.render(ctx), W, 8).get_lines();

    let parent_row =
        lines.iter().find(|line| line.contains("tui/src/components/")).expect("compressed parent directory row");
    let child_row = lines.iter().find(|line| line.contains("text_field.rs")).expect("child file row");
    let parent_branch_col = parent_row[..parent_row.find('├').expect("parent branch")].chars().count();
    let parent_indicator_col = parent_row[..parent_row.find('▾').expect("parent expand indicator")].chars().count();
    let child_branch_col = child_row[..child_row.find('└').expect("child branch")].chars().count();

    assert_eq!(
        child_branch_col, parent_indicator_col,
        "child branch should line up with its parent directory indicator\n parent: {parent_row:?}\n  child: {child_row:?}",
    );
    assert_eq!(
        child_branch_col - parent_branch_col,
        2,
        "each child level should indent by two columns\n parent: {parent_row:?}\n  child: {child_row:?}",
    );
}

#[test]
fn renders_flat_files_with_first_selected() {
    let mut panel = panel_with_flat_files();
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_rows(0));
    assert_selected_tree_row(&term, 2, 3);
}

#[test]
fn renders_directory_tree() {
    let mut panel = panel_with_directory();
    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(
        &term,
        &[
            " Git Diff  2 files  +6 -1".to_string(),
            rule(),
            "▎▾  src/".to_string(),
            cols(&[(" ├── main.rs", 31), ("+2 -1 M ☐", 0)]),
            cols(&[(" └── util.rs", 31), ("+4 -0 A ☐", 0)]),
        ],
    );
}

#[test]
fn tall_panel_renders_git_diff_sidebar_chrome() {
    let mut panel = panel_with_directory();
    let term = render_component(|ctx| panel.render(ctx), W, 13);
    let lines = term.get_lines();

    assert!(lines[0].contains("Git Diff"));
    assert!(lines[0].contains("2 files"));
    assert!(lines[0].contains("+6 -1"));
    assert!(lines[1].starts_with('─'));
    assert!(lines[2].contains("▾  src/"));
    assert!(lines[3].contains("├── main.rs"));
    assert!(lines[3].contains("+2 -1 M"));
    assert!(lines[4].contains("└── util.rs"));
    assert!(lines[4].contains("+4 -0 A"));
}

#[test]
fn tree_guides_are_muted() {
    let mut panel = panel_with_directory();
    let ctx = ViewContext::new((W, 13));
    let term = render_component(|c| panel.render(c), W, 13);
    let lines = term.get_lines();

    let row = lines.iter().position(|line| line.contains("main.rs")).expect("main.rs row should render");
    let guide_col = lines[row][..lines[row].find('├').unwrap()].chars().count();
    let name_col = lines[row][..lines[row].find("main.rs").unwrap()].chars().count();

    assert_eq!(term.get_style_at(row, guide_col).fg, Some(ctx.theme.muted()), "tree guides should use muted color");
    assert_ne!(term.get_style_at(row, name_col).fg, Some(ctx.theme.muted()), "file names should keep their own style");
}

#[test]
fn directories_use_distinct_color_from_files() {
    let mut panel = panel_with_directory();
    let ctx = ViewContext::new((W, 13));
    let term = render_component(|c| panel.render(c), W, 13);
    let lines = term.get_lines();

    let dir_row = lines.iter().position(|line| line.contains("src/")).expect("src/ row should render");
    let dir_col = lines[dir_row][..lines[dir_row].find("src/").unwrap()].chars().count();
    assert_eq!(
        term.get_style_at(dir_row, dir_col).fg,
        Some(ctx.theme.info()),
        "directory names should use the info color"
    );

    let file_row = lines.iter().position(|line| line.contains("main.rs")).expect("main.rs row should render");
    let file_col = lines[file_row][..lines[file_row].find("main.rs").unwrap()].chars().count();
    assert_ne!(
        term.get_style_at(file_row, file_col).fg,
        Some(ctx.theme.info()),
        "file names should not share the directory color"
    );
}

#[test]
fn tree_guides_do_not_extend_past_last_siblings() {
    let files = vec![
        file("pkg/tui/one.rs", FileStatus::Modified, 1, 0),
        file("pkg/wisp/src/two.rs", FileStatus::Modified, 1, 0),
        file("pkg/wisp/tests/three.rs", FileStatus::Modified, 1, 0),
    ];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    let term = render_component(|ctx| panel.render(ctx), W, 14);
    let lines = term.get_lines();
    let row = |needle: &str| {
        lines.iter().find(|line| line.contains(needle)).unwrap_or_else(|| panic!("{needle} row should render"))
    };
    let guides_before = |needle: &str| {
        let line = row(needle);
        line[..line.find(needle).unwrap()].chars().skip(1).filter(|&c| c == '│').count()
    };

    assert!(row("one.rs").contains("└── one.rs"), "one.rs is tui/'s last child, expected └: {:?}", row("one.rs"));
    assert_eq!(guides_before("src/"), 0, "wisp/ is pkg/'s last child, no guide should pass src/: {:?}", row("src/"));
    assert_eq!(guides_before("two.rs"), 1, "only src/'s sibling line should pass two.rs: {:?}", row("two.rs"));
    assert_eq!(guides_before("three.rs"), 0, "nothing continues below three.rs: {:?}", row("three.rs"));
}

#[test]
fn tall_panel_marks_open_file_and_comment_badges() {
    let mut panel = panel_with_directory();
    panel.select_file_index(1);
    panel.sync_view_state(0, vec![0, 2]);
    let term = render_component(|ctx| panel.render(ctx), W, 13);
    let lines = term.get_lines();

    let main_row = lines.iter().find(|line| line.contains("main.rs")).expect("main.rs row");
    let util_row = lines.iter().find(|line| line.contains("util.rs")).expect("util.rs row");
    assert!(!main_row.contains('▎'), "main.rs is not open: {main_row:?}");
    assert!(util_row.contains('▎'), "util.rs should carry the open-file marker: {util_row:?}");
    assert!(util_row.contains("◆2"), "util.rs should show its queued comment badge: {util_row:?}");
    assert!(!main_row.contains('◆'), "main.rs has no comments: {main_row:?}");
}

#[tokio::test]
async fn navigate_down_moves_selection() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_buffer_eq(&term, &flat_rows(1));
    assert_selected_tree_row(&term, 3, 2);
}

#[tokio::test]
async fn navigate_up_moves_selection() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    panel.on_event(&ev(KeyCode::Char('k'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_selected_tree_row(&term, 2, 3);
}

#[tokio::test]
async fn navigation_stops_at_first_entry() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('k'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_selected_tree_row(&term, 2, 3);
}

#[tokio::test]
async fn navigation_stops_at_last_entry() {
    let mut panel = panel_with_flat_files();
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    panel.on_event(&ev(KeyCode::Char('j'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);

    assert_selected_tree_row(&term, 3, 2);
}

#[tokio::test]
async fn collapse_directory_hides_children() {
    let mut panel = panel_with_directory();
    panel.on_event(&ev(KeyCode::Char('h'))).await;
    let term = render_component(|ctx| panel.render(ctx), W, 5);

    assert_buffer_eq(
        &term,
        &[" Git Diff  2 files  +6 -1".to_string(), rule(), "▎▸  src/".to_string(), String::new(), String::new()],
    );
}

#[test]
fn renders_queued_comment_indicator() {
    let files = vec![file("a.rs", FileStatus::Modified, 1, 1)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel.sync_view_state(3, vec![0]);
    let term = render_component(|ctx| panel.render(ctx), W, 3);

    assert_buffer_eq(
        &term,
        &[" Git Diff  1 file  +1 -1  ◆3".to_string(), rule(), cols(&[("▎── a.rs", 31), ("+1 -1 M ☐", 0)])],
    );
}

#[test]
fn queued_comment_singular() {
    let files = vec![file("a.rs", FileStatus::Modified, 1, 1)];
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);
    panel.sync_view_state(1, vec![0]);
    let term = render_component(|ctx| panel.render(ctx), W, 4);
    let lines = term.get_lines();

    assert!(lines[0].contains("◆1"), "header row should show the queued comment chip: {:?}", lines[0]);
}

#[test]
fn empty_panel_renders_chrome_only() {
    let mut panel = FileListPanel::new();
    let term = render_component(|ctx| panel.render(ctx), 30, 2);

    assert_buffer_eq(&term, &[" Git Diff  0 files  +0 -0", &"─".repeat(30)]);
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
            " Git Diff  3 files  +2 -2".to_string(),
            rule(),
            cols(&[("▎── added.rs", 31), ("+1 -0 A ☐", 0)]),
            cols(&[(" ── deleted.rs", 31), ("+0 -1 D ☐", 0)]),
            cols(&[(" ── modified.rs", 31), ("+1 -1 M ☐", 0)]),
        ],
    );
}

#[test]
fn selected_row_has_selection_style() {
    let mut panel = panel_with_flat_files();
    let term = render_component(|c| panel.render(c), W, 5);

    assert_selected_tree_row(&term, 2, 3);
    assert_eq!(term.get_style_at(0, 1).bg, None, "chrome rows should not set an explicit bg");
}

#[tokio::test]
async fn arrow_keys_navigate() {
    let mut panel = panel_with_flat_files();

    panel.on_event(&ev(KeyCode::Down)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);
    assert_selected_tree_row(&term, 3, 2);

    panel.on_event(&ev(KeyCode::Up)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 4);
    assert_selected_tree_row(&term, 2, 3);
}

#[tokio::test]
async fn mouse_scroll_moves_once_until_next_render() {
    let files = (0..5).map(|i| file(&format!("file-{i}.rs"), FileStatus::Modified, 1, 1)).collect::<Vec<_>>();
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);

    panel.on_event(&mouse_scroll(MouseEventKind::ScrollDown)).await;
    panel.on_event(&mouse_scroll(MouseEventKind::ScrollDown)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 7);

    assert_selected_tree_row(&term, 3, 2);

    panel.on_event(&mouse_scroll(MouseEventKind::ScrollDown)).await;
    let term = render_component(|ctx| panel.render(ctx), W, 7);

    assert_selected_tree_row(&term, 4, 3);
}

#[tokio::test]
async fn renders_only_viewport_rows_after_scrolling_many_files() {
    let files = (0..20).map(|i| file(&format!("file-{i:02}.rs"), FileStatus::Modified, 1, 1)).collect::<Vec<_>>();
    let mut panel = FileListPanel::new();
    panel.rebuild_from_files(&files);

    for _ in 0..7 {
        panel.on_event(&ev(KeyCode::Char('j'))).await;
    }

    let term = render_component(|ctx| panel.render(ctx), W, 8);

    assert_buffer_eq(
        &term,
        &[
            " Git Diff  20 files  +20 -20".to_string(),
            rule(),
            cols(&[(" ── file-02.rs", 31), ("+1 -1 M ☐", 0)]),
            cols(&[(" ── file-03.rs", 31), ("+1 -1 M ☐", 0)]),
            cols(&[(" ── file-04.rs", 31), ("+1 -1 M ☐", 0)]),
            cols(&[(" ── file-05.rs", 31), ("+1 -1 M ☐", 0)]),
            cols(&[(" ── file-06.rs", 31), ("+1 -1 M ☐", 0)]),
            cols(&[("▎── file-07.rs", 31), ("+1 -1 M ☐", 0)]),
        ],
    );
    assert_selected_tree_row(&term, 7, 6);
}
