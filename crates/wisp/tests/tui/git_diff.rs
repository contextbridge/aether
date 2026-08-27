use unicode_width::UnicodeWidthStr;

use super::support::*;

fn changed_git(path: &str, old: &str, new: &str) -> FakeGit {
    let mut git = FakeGit::new("/workspace");
    git.add_file(path, old);
    assert!(git.stage(path));
    git.commit("init").unwrap();
    git.write_file(path, new);
    git
}

fn open_diff(git: FakeGit, width: u16) -> TestUi {
    let mut ui = TestUiBuilder::new().working_dir("/workspace").dimensions(width, 15).git(git).build();
    ui.key(ctrl('g'));
    ui.settle_tasks();
    ui.draw();
    ui
}

fn open_patch(ui: &mut TestUi) {
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();
    ui.draw();
}

#[test]
fn ctrl_g_opens_and_esc_closes_git_diff() {
    let mut ui = open_diff(FakeGit::new("/workspace"), 80);
    assert!(ui.viewport_text().contains("Git Diff"));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
}

#[test]
fn non_repository_error_does_not_block_close() {
    let mut ui = open_diff(FakeGit::not_a_repository("/workspace"), 80);
    assert!(ui.viewport_text().contains("Not a git repository"));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
}

#[test]
fn untracked_files_render_from_the_fake_repository() {
    let mut git = FakeGit::new("/workspace");
    git.add_file("README.md", "hello world\n");
    let mut ui = open_diff(git, 120);

    let viewport = ui.viewport_text();
    assert!(viewport.contains("README.md"), "{viewport}");
    assert!(viewport.contains("hello world"), "{viewport}");
}

#[test]
fn diff_renders_changes_and_stages_the_selected_file() {
    let mut ui = open_diff(changed_git("src/lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    let viewport = ui.viewport_text();
    assert!(viewport.contains("lib.rs"), "{viewport}");
    assert!(viewport.contains("old") && viewport.contains("new"), "{viewport}");

    ui.key(key(KeyCode::Char(' ')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("src/lib.rs"), Some((FileStatus::Modified, StageState::Staged)));

    ui.key(key(KeyCode::Char('t')));
    ui.settle_tasks();
    ui.draw();
    assert!(ui.viewport_text().contains("Git Diff · Unstaged"));
}

#[test]
fn stage_all_and_unstage_all_model_index_state() {
    let mut git = FakeGit::new("/workspace");
    for (path, old, new) in [("src/a.rs", "a old\n", "a new\n"), ("src/b.rs", "b old\n", "b new\n")] {
        git.add_file(path, old);
        assert!(git.stage(path));
        git.commit("init").unwrap();
        git.write_file(path, new);
    }
    let mut ui = open_diff(git, 160);

    ui.key(key(KeyCode::Char('a')));
    ui.settle_tasks();
    for path in ["src/a.rs", "src/b.rs"] {
        assert_eq!(ui.executor().git().status(path), Some((FileStatus::Modified, StageState::Staged)));
    }

    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    for path in ["src/a.rs", "src/b.rs"] {
        assert_eq!(ui.executor().git().status(path), Some((FileStatus::Modified, StageState::Unstaged)));
    }
}

#[test]
fn commit_updates_fake_repository_without_git_side_effects() {
    let git = changed_git("file.txt", "original\n", "changed\n");
    let mut ui = open_diff(git, 120);
    ui.key(key(KeyCode::Char(' ')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Char('C')));
    ui.type_text("my commit message");
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();

    assert_eq!(ui.executor().git().commits(), vec!["init", "my commit message"]);
    assert_eq!(ui.executor().git().status("file.txt"), None, "a committed working tree should be clean");
}

#[test]
fn empty_commit_message_is_reported() {
    let git = changed_git("file.txt", "original\n", "changed\n");
    let mut ui = open_diff(git, 120);
    ui.key(key(KeyCode::Char(' ')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Char('C')));
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();
    ui.draw();

    assert!(ui.viewport_text().contains("Commit message cannot be empty"));
}

#[test]
fn discard_restores_tracked_content_and_removes_untracked_files() {
    let mut git = changed_git("tracked.txt", "original\n", "changed\n");
    git.add_file("untracked.txt", "scratch\n");
    let mut ui = open_diff(git, 120);

    ui.key(key(KeyCode::Char('d')));
    ui.key(key(KeyCode::Char('y')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().file("tracked.txt").and_then(|file| file.contents), Some(b"original\n".to_vec()));

    ui.key(key(KeyCode::Char('d')));
    ui.key(key(KeyCode::Char('y')));
    ui.settle_tasks();
    assert!(ui.executor().git().file("untracked.txt").is_none());
}

#[test]
fn discarded_deleted_file_is_restored_from_commit() {
    let mut git = changed_git("file.txt", "original\n", "changed\n");
    git.remove_file("file.txt");
    let mut ui = open_diff(git, 120);

    ui.key(key(KeyCode::Char('d')));
    ui.key(key(KeyCode::Char('y')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().file("file.txt").and_then(|file| file.contents), Some(b"original\n".to_vec()));
}

#[test]
fn full_file_mode_reads_fake_content_and_binary_files_have_a_label() {
    let mut ui = open_diff(changed_git("src/main.rs", "fn old() {}\n", "fn new() {}\nfn extra() {}\n"), 120);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('o')));
    ui.settle_tasks();
    ui.draw();
    assert!(ui.viewport_text().contains("[full file]"));
    assert!(ui.viewport_text().contains("fn extra()"));

    let mut binary = FakeGit::new("/workspace");
    binary.add_file("data.bin", b"\x00\x01");
    binary.stage("data.bin");
    binary.commit("init").unwrap();
    binary.write_file("data.bin", b"\x00\x01\x02");
    let mut binary_ui = open_diff(binary, 120);
    assert!(binary_ui.viewport_text().contains("Binary file"));
}

#[test]
fn git_diff_owns_the_cursor_and_styles_comment_drafts() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    assert!(!ui.backend().cursor_visible(), "the hidden composer must not own the cursor");

    ui.key(key(KeyCode::Char('c')));
    ui.draw();
    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "│ > ").expect("empty draft body");
    let text = row_text(buffer, row);
    let prefix_column = u16::try_from(text[..text.find("│ > ").unwrap()].width()).unwrap();
    assert!(!text.contains('█'));
    assert_eq!(ui.backend().cursor_position(), Position::new(prefix_column + 4, row));

    ui.type_text("a界");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "│ > a界").expect("draft body");
    let text = row_text(buffer, row);
    let text_column = u16::try_from(text[..text.find("a界").unwrap()].width()).unwrap();
    assert!(ui.backend().cursor_visible());
    assert_eq!(ui.backend().cursor_position(), Position::new(text_column + 3, row));

    let theme = Theme::default();
    for y in [row - 1, row, row + 1] {
        assert_eq!(buffer[(text_column, y)].bg, theme.sidebar_bg);
        assert_eq!(buffer[(text_column + 8, y)].bg, theme.sidebar_bg);
    }
}

#[test]
fn comments_are_stateful_and_submit_as_a_review_prompt() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("feedback");
    ui.key(key(KeyCode::Enter));
    ui.draw();
    assert!(ui.viewport_text().contains("feedback"));

    ui.key(key(KeyCode::Char('s')));
    ui.settle_tasks();
    match ui.next_agent_command().expect("review prompt") {
        AgentCommand::Prompt { text, content, .. } => {
            assert!(text.contains("I'm reviewing the working tree diff"));
            assert!(text.contains("feedback"));
            assert!(content.is_none());
        }
        other => panic!("expected review prompt, got {other:?}"),
    }
}

#[test]
fn comment_confirmation_preserves_comments_until_an_action_is_confirmed() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Char('r')));
    ui.draw();
    assert!(ui.viewport_text().contains("will clear"));
    assert!(ui.viewport_text().contains("keep me"));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(ui.viewport_text().contains("keep me"));
}

#[test]
fn modified_key_events_do_not_trigger_git_actions() {
    let git = changed_git("file.txt", "original\n", "changed\n");
    let mut ui = open_diff(git, 120);
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER] {
        ui.key(KeyEvent::new(KeyCode::Char('a'), modifiers));
    }
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("file.txt"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn stale_git_events_do_not_replace_the_current_screen() {
    let mut ui = open_diff(changed_git("file.rs", "fn old() {}\n", "fn new() {}\n"), 120);
    ui.deliver_result(CommandResult::GitDiff(GitDiffEvent::ActionFinished {
        request_id: wisp::request::RequestId::from(0),
        result: Ok(()),
    }));
    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
}

#[test]
fn inline_preview_renders_a_bounded_prefix_of_canonical_rows() {
    use wisp::git_review::FileDiff;
    use wisp::view::diff::{DiffRowKind, diff_rows, render_diff};

    let old: String = (1..=40).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "line {n}");
        text
    });
    let new: String = (1..=40).fold(String::new(), |mut text, n| {
        let _ = if n % 3 == 0 { writeln!(text, "changed {n}") } else { writeln!(text, "line {n}") };
        text
    });
    let file = FileDiff::from_texts("src/lib.rs", &old, &new);
    let theme = Theme::default();

    for width in [60u16, 120] {
        let mut highlighter = SyntaxHighlighter::new();
        let canonical: Vec<String> = diff_rows(&file, width, &theme, &mut highlighter)
            .into_iter()
            .filter(|row| row.kind == DiffRowKind::Content)
            .map(|row| row.line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();
        let preview: Vec<String> = render_diff(&file, width, &theme, &mut highlighter)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect())
            .collect();

        let shown = preview.iter().take_while(|line| !line.contains("more rows")).count();
        assert!(shown > 0, "preview at width {width} is empty");
        assert!(shown < preview.len(), "preview at width {width} should end in a truncation notice");
        assert_eq!(
            preview[..shown],
            canonical[..shown],
            "inline preview at width {width} must be a prefix of the canonical content rows"
        );
    }
}

#[test]
fn page_down_scrolls_the_patch_view() {
    let new: String = (1..=60).fold(String::new(), |mut out, n| {
        let _ = writeln!(out, "line number {n:02}");
        out
    });
    let mut ui = open_diff(changed_git("src/lib.rs", "", &new), 120);
    open_patch(&mut ui);

    let before = ui.viewport_text();
    assert!(before.contains("line number 01"), "the patch must start at the top:\n{before}");
    assert!(!before.contains("line number 20"), "later lines start off screen:\n{before}");

    ui.key(key(KeyCode::PageDown));
    ui.draw();

    let after = ui.viewport_text();
    assert_ne!(before, after, "PageDown must scroll the patch view");
    assert!(!after.contains("line number 01"), "the first line must scroll away:\n{after}");
}
