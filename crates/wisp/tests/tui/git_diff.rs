use clankerdiff_client::{ClientState, DiffReviewEvent};
use clankerdiff_core::{RepoPath, RepositoryAction};
use std::sync::Arc;
use unicode_width::UnicodeWidthStr;
use wisp::screens::git_diff::GitDiffScreen;
use wisp::surfaces::input::UiOutput;

use super::support::*;

fn changed_git(path: &str, old: &str, new: &str) -> FakeGit {
    FakeGit::new("/workspace").with_modified(path, old, new)
}

fn open_diff(git: FakeGit, width: u16) -> TestUi {
    let mut ui = TestUiBuilder::new()
        .session(|session| session.working_dir("/workspace"))
        .dimensions(width, 15)
        .git(git)
        .build();
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

fn review_state(ui: &TestUi) -> Arc<ClientState> {
    ui.app().core().git_review().cloned().expect("the review is open")
}

fn builtin_theme_count() -> usize {
    clankerdiff_ratatui::theme::ReviewTheme::catalog().len()
}

fn is_repository_action(outputs: &[UiOutput]) -> bool {
    matches!(outputs, [UiOutput::ReviewEvent(DiffReviewEvent::RepositoryAction(_))])
}

fn run_command(ui: &mut TestUi, title: &str) {
    ui.key(key(KeyCode::Char('?')));
    ui.type_text(title);
    ui.key(key(KeyCode::Enter));
}

fn screen_command(screen: &mut GitDiffScreen, title: &str) -> Vec<UiOutput> {
    screen.on_ui_event(&UiEvent::Key(key(KeyCode::Char('?'))));
    for character in title.chars() {
        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Char(character))));
    }
    screen.on_ui_event(&UiEvent::Key(key(KeyCode::Enter)))
}

#[test]
fn external_edits_refresh_an_idle_git_review() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    git.write_file("lib.rs", "fn external_edit() {}\n");
    ui.settle_tasks();
    ui.assert_viewport_contains("external_edit");
    ui.assert_viewport_not_contains("fn new()");
}

#[test]
fn external_file_additions_and_removals_refresh_the_drawer() {
    let git = FakeGit::new("/workspace");
    let mut ui = open_diff(git.clone(), 120);
    git.add_file("external.rs", "fn added() {}\n");
    ui.settle_tasks();
    ui.assert_viewport_contains("external.rs");

    git.remove_file("external.rs");
    ui.settle_tasks();
    ui.assert_viewport_not_contains("external.rs");
}

#[test]
fn a_failed_initial_watch_can_be_retried_by_reopening_the_review() {
    let git = FakeGit::not_a_repository("/workspace");
    let mut ui = open_diff(git.clone(), 160);
    ui.assert_viewport_contains("path is not inside a Git worktree");
    git.set_repository_available(true);
    git.add_file("recovered.rs", "fn recovered() {}\n");
    ui.key(key(KeyCode::Esc));
    ui.key(ctrl('g'));
    ui.assert_viewport_contains("recovered.rs");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn background_refresh_retains_comments_and_selected_file() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep this feedback");
    ui.key(key(KeyCode::Enter));
    git.add_file("a.rs", "fn another_file() {}\n");
    ui.settle_tasks();
    ui.assert_viewport_contains("keep this feedback");
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_contains("a.rs");
}

#[test]
fn background_refresh_waits_until_a_comment_draft_closes() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("unfinished feedback");
    git.write_file("lib.rs", "fn intermediate() {}\n");
    ui.settle_tasks();
    git.write_file("lib.rs", "fn latest_edit() {}\n");
    ui.settle_tasks();
    ui.assert_viewport_contains("unfinished feedback");
    ui.assert_viewport_not_contains("latest_edit");
    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_contains("latest_edit");
    ui.assert_viewport_not_contains("intermediate");
}

#[test]
fn background_failure_retains_the_document_and_recovers_without_content_changes() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    git.set_repository_available(false);
    ui.settle_tasks();
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_contains("path is not inside a Git worktree");
    git.set_repository_available(true);
    ui.settle_tasks();
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn retained_watch_snapshot_survives_a_later_failure_while_editing() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("unfinished feedback");
    git.write_file("lib.rs", "fn retained_edit() {}\n");
    ui.settle_tasks();
    git.set_repository_available(false);
    ui.settle_tasks();
    ui.assert_viewport_contains("unfinished feedback");
    ui.assert_viewport_not_contains("retained_edit");
    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_contains("retained_edit");
    ui.assert_viewport_contains("path is not inside a Git worktree");
    git.set_repository_available(true);
    ui.settle_tasks();
    ui.assert_viewport_contains("retained_edit");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn manual_refresh_installs_the_latest_watcher_state() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    git.write_file("lib.rs", "fn stale_edit() {}\n");
    git.write_file("lib.rs", "fn current_edit() {}\n");
    run_command(&mut ui, "Refresh repository");
    ui.settle_tasks();
    ui.assert_viewport_contains("current_edit");
    ui.assert_viewport_not_contains("stale_edit");
}

#[test]
fn background_snapshots_follow_scope_changes() {
    let git = changed_git("lib.rs", "fn original() {}\n", "fn staged_version() {}\n");
    git.stage_all();
    git.write_file("lib.rs", "fn working_version() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    git.write_file("lib.rs", "fn external_version() {}\n");
    ui.click_text("Staged");
    ui.settle_tasks();
    ui.assert_viewport_contains("Git Diff · Staged");
    ui.assert_viewport_contains("staged_version");
    ui.assert_viewport_not_contains("external_version");
    git.stage_all();
    ui.settle_tasks();
    ui.assert_viewport_contains("external_version");
}

#[test]
fn background_updates_after_a_repository_action_show_the_latest_snapshot() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    run_command(&mut ui, "Stage all");
    git.write_file("lib.rs", "fn external_version() {}\n");
    ui.settle_tasks();
    ui.assert_viewport_contains("external_version");
    assert_eq!(git.status("lib.rs"), Some((FileStatus::Modified, StageState::PartiallyStaged)));
    run_command(&mut ui, "Unstage all");
    ui.settle_tasks();
    assert_eq!(git.status("lib.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn action_completion_does_not_refresh_or_install_a_snapshot() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    let opened = review_state(&ui);
    git.write_file("lib.rs", "fn watcher_only() {}\n");
    ui.settle_tasks();
    let watched = review_state(&ui);
    let mut screen = screen_showing(&opened);
    assert!(is_repository_action(&screen_command(&mut screen, "Stage all")));

    screen.on_action_result(Ok(()));

    let text = buffer_text(&render_git_diff(&mut screen, 160, 15));
    assert!(text.contains("fn new()") && !text.contains("watcher_only"), "{text}");
    screen.install(&watched);
    assert!(buffer_text(&render_git_diff(&mut screen, 160, 15)).contains("watcher_only"));
    assert!(is_repository_action(&screen_command(&mut screen, "Unstage all")));
}

#[test]
fn a_snapshot_does_not_complete_an_in_flight_action() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    let opened = review_state(&ui);
    git.stage_all();
    ui.settle_tasks();
    let staged = review_state(&ui);
    let mut screen = screen_showing(&opened);
    assert!(is_repository_action(&screen_command(&mut screen, "Stage all")));

    screen.install(&staged);
    assert!(!is_repository_action(&screen_command(&mut screen, "Unstage all")));
    screen.on_ui_event(&UiEvent::Key(key(KeyCode::Esc)));

    screen.on_action_result(Ok(()));
    assert!(is_repository_action(&screen_command(&mut screen, "Unstage all")));
}

#[test]
fn initial_watch_document_while_help_is_open_does_not_leave_review_loading() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui =
        TestUiBuilder::new().session(|session| session.working_dir("/workspace")).dimensions(160, 15).git(git).build();
    ui.key(ctrl('g'));
    ui.key(key(KeyCode::Char('?')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_contains("fn new()");
}

#[test]
fn closing_review_stops_its_watch_and_reopening_starts_fresh() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    assert!(ui.app().core().git_review().is_some());
    ui.key(key(KeyCode::Esc));
    assert!(ui.app().core().git_review().is_none());
    git.write_file("lib.rs", "fn current_edit() {}\n");
    ui.settle_tasks();
    assert!(ui.app().core().git_review().is_none(), "a closed review watches nothing");
    ui.key(ctrl('g'));
    assert!(ui.app().core().git_review().is_some());
    ui.assert_viewport_contains("current_edit");
}

#[test]
fn git_diff_footer_prioritizes_contextual_actions() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 120);
    ui.assert_viewport_contains("h/l fold/open");
    ui.assert_viewport_contains("Tab pane");
    ui.assert_viewport_contains("? commands");

    open_patch(&mut ui);
    ui.assert_viewport_contains("c comment");
    ui.assert_viewport_contains("? commands");
}

#[test]
fn git_diff_shortcut_help_opens_and_closes_without_leaving_review() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 120);

    ui.key(key(KeyCode::Char('?')));
    ui.assert_viewport_contains("┌ Commands ");
    ui.assert_viewport_contains("Navigation");
    ui.assert_viewport_contains("Enter run");

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_contains("Git Diff");
    ui.assert_viewport_not_contains("┌ Commands ");
}

#[test]
fn ctrl_g_opens_and_esc_closes_git_diff() {
    let git = FakeGit::new("/workspace");
    let mut ui = open_diff(git.clone(), 80);
    ui.assert_viewport_contains("Git Diff");

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_not_contains("Git Diff");
}

#[test]
fn non_repository_error_does_not_block_close() {
    let git = FakeGit::not_a_repository("/workspace");
    let mut ui = open_diff(git.clone(), 80);
    ui.assert_viewport_contains("path is not inside a Git worktree");

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_not_contains("Git Diff");
}

#[test]
fn untracked_files_render_from_the_fake_repository() {
    let git = FakeGit::new("/workspace").with_untracked("README.md", "hello world\n");
    let mut ui = open_diff(git.clone(), 120);

    ui.assert_viewport_contains("README.md");
    ui.assert_viewport_contains("hello world");
}

#[test]
fn diff_renders_changes_and_stages_the_selected_file() {
    let git = changed_git("src/lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    ui.assert_viewport_contains("lib.rs");
    ui.assert_viewport_contains("old");
    ui.assert_viewport_contains("new");

    ui.key(key(KeyCode::Char(' ')));
    ui.settle_tasks();
    assert_eq!(git.status("src/lib.rs"), Some((FileStatus::Modified, StageState::Staged)));

    ui.click_text("Unstaged");
    ui.settle_tasks();
    ui.assert_viewport_contains("Git Diff · Unstaged");
}

#[test]
fn review_keys_ask_for_the_repository_actions_they_name() {
    let ui = open_diff(changed_git("file.txt", "original\n", "changed\n"), 120);
    let state = review_state(&ui);
    let commit: Vec<_> =
        std::iter::once('C').chain("ship it".chars()).map(KeyCode::Char).chain([KeyCode::Enter]).collect();
    let file = RepoPath::new("file.txt").unwrap();
    let typed =
        |title: &str| std::iter::once('?').chain(title.chars()).map(KeyCode::Char).chain([KeyCode::Enter]).collect();
    let cases = [
        (vec![KeyCode::Char(' ')], RepositoryAction::StagePaths(vec![file.clone()])),
        (typed("Stage all"), RepositoryAction::StageAll),
        (typed("Unstage all"), RepositoryAction::UnstageAll),
        (
            vec![KeyCode::Char('d'), KeyCode::Char('y')],
            RepositoryAction::Discard { path: file, status: FileStatus::Modified },
        ),
        (commit, RepositoryAction::Commit { message: "ship it".into() }),
    ];
    for (keys, expected) in cases {
        let mut screen = screen_showing(&state);
        let outputs: Vec<_> = keys.into_iter().flat_map(|code| screen.on_ui_event(&UiEvent::Key(key(code)))).collect();
        assert!(
            matches!(&outputs[..], [UiOutput::ReviewEvent(DiffReviewEvent::RepositoryAction(action))] if *action == expected),
            "expected {expected:?}, got {outputs:?}"
        );
    }
}

#[test]
fn empty_commit_message_is_reported() {
    let git = changed_git("file.txt", "original\n", "changed\n");
    let mut ui = open_diff(git.clone(), 120);
    ui.key(key(KeyCode::Char(' ')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Char('C')));
    ui.key(key(KeyCode::Enter));
    ui.settle_tasks();

    ui.assert_viewport_contains("Commit message cannot be empty");
}

#[test]
fn full_file_mode_reads_fake_content_and_binary_files_have_a_label() {
    let old = (0..40).fold(String::new(), |mut text, index| {
        writeln!(text, "// unchanged context {index:02}").unwrap();
        text
    });
    let new = format!("{old}fn extra() {{}}\n");
    let git = changed_git("src/main.rs", &old, &new);
    let mut ui = open_diff(git.clone(), 120);
    open_patch(&mut ui);
    ui.assert_viewport_not_contains("unchanged context 00");
    ui.key(key(KeyCode::Char('f')));
    for _ in 0..60 {
        ui.key(key(KeyCode::Up));
    }
    ui.assert_viewport_contains("unchanged context 00");

    let binary = FakeGit::new("/workspace").with_modified("data.bin", b"\x00\x01", b"\x00\x01\x02");
    let mut binary_ui = open_diff(binary, 120);
    binary_ui.assert_viewport_contains("Binary file");
}

#[test]
fn git_diff_owns_the_cursor_and_styles_comment_drafts() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    assert!(!ui.backend().cursor_visible(), "the hidden composer must not own the cursor");

    ui.key(key(KeyCode::Char('c')));
    ui.draw();
    assert!(ui.backend().cursor_visible(), "the library draft owns the cursor");

    ui.type_text("a界");
    ui.draw();

    let buffer = ui.backend().buffer();
    let row = row_containing(buffer, "a界").expect("draft body");
    let text = row_text(buffer, row);
    let text_column = u16::try_from(text[..text.find("a界").unwrap()].width()).unwrap();
    assert!(ui.backend().cursor_visible());
    assert_eq!(ui.backend().cursor_position(), Position::new(text_column + 3, row));

    assert!(!text.contains('█'), "the host uses the terminal cursor, not a painted cursor");
}

#[test]
fn escape_cancels_comment_draft_before_closing_git_diff() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("discard me");
    ui.assert_viewport_contains("discard me");
    ui.assert_viewport_contains("Esc cancel");

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_contains("Git Diff");
    ui.assert_viewport_not_contains("discard me");

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_not_contains("Git Diff");
}

#[test]
fn comments_are_stateful_and_submit_as_a_review_prompt() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("feedback");
    ui.key(key(KeyCode::Enter));
    ui.assert_viewport_contains("feedback");

    ui.key(key(KeyCode::Char('s')));
    let review = sent_prompt(&mut ui);
    let text = prompt_text(&review).unwrap_or_default();
    assert!(text.contains("I'm reviewing the working tree diff") && text.contains("feedback"), "{text}");
}

#[test]
fn refreshing_the_document_retains_review_comments() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(key(KeyCode::Enter));
    run_command(&mut ui, "Refresh repository");
    ui.settle_tasks();
    ui.assert_viewport_contains("keep me");
}

#[test]
fn escape_closes_the_library_review_with_queued_comments() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(key(KeyCode::Enter));

    ui.key(key(KeyCode::Esc));
    ui.assert_viewport_not_contains("Git Diff");
    assert!(!sent_any_prompt(&mut ui), "cancel does not submit feedback");
}

#[test]
fn ctrl_g_only_closes_in_browse_not_draft_or_help() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(ctrl('g'));
    ui.assert_viewport_contains("keep me");
    ui.key(key(KeyCode::Esc));
    ui.key(key(KeyCode::Char('?')));
    ui.key(ctrl('g'));
    ui.assert_viewport_contains("┌ Commands ");
    ui.key(key(KeyCode::Esc));
    ui.key(ctrl('g'));
    ui.assert_viewport_not_contains("Git Diff");
}

#[test]
fn modified_key_events_do_not_trigger_git_actions() {
    let git = changed_git("file.txt", "original\n", "changed\n");
    let mut ui = open_diff(git.clone(), 120);
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT, KeyModifiers::SUPER] {
        ui.key(KeyEvent::new(KeyCode::Char(' '), modifiers));
    }
    ui.settle_tasks();
    assert_eq!(git.status("file.txt"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn repository_actions_still_work_after_reopening_the_review() {
    let git = changed_git("file.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = open_diff(git.clone(), 120);
    ui.key(key(KeyCode::Esc));
    ui.settle_tasks();
    ui.key(ctrl('g'));
    ui.settle_tasks();
    run_command(&mut ui, "Stage all");
    ui.settle_tasks();
    assert_eq!(git.status("file.rs"), Some((FileStatus::Modified, StageState::Staged)));
    run_command(&mut ui, "Unstage all");
    ui.settle_tasks();
    assert_eq!(git.status("file.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn git_theme_picker_routes_selection_to_global_settings() {
    let git = FakeGit::new("/workspace");
    let mut ui = open_diff(git.clone(), 120);
    run_command(&mut ui, "Choose theme");
    ui.draw();
    ui.key(key(KeyCode::Enter));
    assert_eq!(take_applied_theme(&mut ui).theme.selection_id(), "builtin:sage");
}

#[test]
fn discovered_custom_review_theme_uses_its_filename_globally() {
    use clankerdiff_ratatui::ThemeChoice;
    use clankerdiff_ratatui::theme::{ReviewTheme, ThemeId};
    let theme =
        ReviewTheme::from_bytes(ThemeId::Custom("custom.json".into()), &ReviewTheme::default().to_bytes().unwrap())
            .unwrap();
    let mut ui = TestUiBuilder::new()
        .git(FakeGit::new("/workspace"))
        .review_themes(vec![ThemeChoice::new("My custom theme", theme.clone())])
        .dimensions(120, 15)
        .build();
    ui.key(ctrl('g'));
    ui.draw();
    run_command(&mut ui, "Choose theme");
    for _ in 0..builtin_theme_count() {
        if ui.viewport_text().contains("› My custom theme") {
            break;
        }
        ui.key(key(KeyCode::Down));
    }
    ui.assert_viewport_contains("› My custom theme");
    ui.key(key(KeyCode::Enter));
    let settings = take_applied_theme(&mut ui);
    assert_eq!(settings.theme.selection_id(), "file:custom.json");
    ui.deliver_host_result(HostCommandResult::ThemeApplied(Ok((
        Box::new(settings),
        Theme::from_review(theme.clone()),
    ))));
    ui.draw();
    assert_eq!(ui.app().theme().review().revision(), theme.revision());
}

#[test]
fn failed_review_theme_selection_restores_installed_colors() {
    let git = FakeGit::new("/workspace");
    let mut ui = open_diff(git.clone(), 120);
    let original = ui.backend().buffer()[(0, 0)].bg;
    run_command(&mut ui, "Choose theme");
    ui.draw();
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.draw();
    assert_ne!(ui.backend().buffer()[(0, 0)].bg, original);
    let settings = take_applied_theme(&mut ui);
    assert_ne!(settings.theme.selection_id(), "builtin:sage");
    ui.deliver_host_result(HostCommandResult::ThemeApplied(Err(wisp::theme::ThemeApplicationError::Save(
        std::io::Error::other("save failed"),
    ))));
    ui.draw();
    assert_eq!(ui.backend().buffer()[(0, 0)].bg, original);
}

#[test]
fn inline_preview_renders_a_bounded_prefix_of_canonical_rows() {
    use clankerdiff_ratatui::diff::FileDiff;
    use clankerdiff_ratatui::{DiffPreviewOptions, DiffPreviewState};

    let old: String = (1..=40).fold(String::new(), |mut text, n| {
        let _ = writeln!(text, "line {n}");
        text
    });
    let new: String = (1..=40).fold(String::new(), |mut text, n| {
        let _ = if n % 3 == 0 { writeln!(text, "changed {n}") } else { writeln!(text, "line {n}") };
        text
    });
    let file = FileDiff::from_texts("src/main.rs", &old, &new).unwrap();
    let theme = Theme::default();

    for width in [60u16, 120] {
        let mut ui = TestUi::with_dimensions(width, 40);
        ui.submit("edit file");
        ui.acp_event(tool_call("edit", "Edit file"));
        ui.acp_event(tool_completed_with_diff_contents("edit", &old, &new));
        ui.complete_prompt(acp::StopReason::EndTurn);
        let conversation = ui.conversation();
        let start = row_containing(&conversation, "Edit file").expect("tool row") + 1;
        let end = row_containing(&conversation, "more rows").expect("bounded preview notice");
        let preview: Vec<String> =
            (start..end).map(|row| row_text(&conversation, row).trim_end().to_string()).collect();
        let canonical: Vec<String> = DiffPreviewState::new(file.clone())
            .render(
                width - 4,
                theme.review(),
                &mut clankerdiff_ratatui::syntax::SyntaxHighlighter::default(),
                DiffPreviewOptions { max_content_rows: usize::MAX, ..DiffPreviewOptions::default() },
            )
            .iter()
            .map(|line| {
                let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
                format!("  {}", text.trim_end())
            })
            .collect();

        assert_eq!(preview.len(), 20, "conversation previews must remain bounded");
        assert_eq!(
            row_text(&conversation, end).trim_end(),
            format!("  … {} more rows", canonical.len() - preview.len()),
        );
        assert!(preview.len() < canonical.len(), "preview must truncate canonical rows");
        assert_eq!(
            preview,
            canonical[..preview.len()],
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
    let git = changed_git("src/lib.rs", "", &new);
    let mut ui = open_diff(git.clone(), 120);
    open_patch(&mut ui);

    let before = ui.viewport_text();
    assert!(before.contains("line number 01"), "the patch must start at the top:\n{before}");
    assert!(!before.contains("line number 20"), "later lines start off screen:\n{before}");

    ui.key(key(KeyCode::PageDown));

    let after = ui.viewport_text();
    assert_ne!(before, after, "PageDown must scroll the patch view");
    assert!(!after.contains("line number 01"), "the first line must scroll away:\n{after}");
}

mod screen_mouse {
    use super::*;
    use clankerdiff_ratatui::diff::FileDiff;
    use wisp::view::input::MouseAction;

    fn open_screen() -> GitDiffScreen {
        git_diff_screen(vec![
            FileDiff::from_texts("src/main.rs", "fn old() {}\n", "fn main() {}\n").unwrap(),
            FileDiff::from_texts("src/lib.rs", "", "pub fn library() {}\n").unwrap(),
            FileDiff::from_texts("Cargo.toml", "old\n", "new\n").unwrap(),
        ])
    }

    fn position_of(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
        let row = row_containing(buffer, needle)?;
        let text = row_text(buffer, row);
        let column = u16::try_from(text[..text.find(needle)?].chars().count()).ok()?;
        Some((column, row))
    }

    fn assert_drawer_focus(buffer: &Buffer, focused: bool) {
        let accent = Theme::default().accent;
        let selected = (1..buffer.area.bottom() - 2)
            .map(|y| &buffer[(1, y)])
            .find(|cell| cell.bg == accent)
            .expect("drawer has a selected row");
        assert_eq!(
            selected.modifier.contains(ratatui::style::Modifier::BOLD),
            focused,
            "selected drawer row is bold only while focused:\n{}",
            buffer_text(buffer)
        );
    }

    #[test]
    fn git_diff_click_left_side_selects_drawer() {
        let mut screen = open_screen();

        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Tab)));
        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, false);

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3), KeyModifiers::NONE)).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&buffer, true);
    }

    #[test]
    fn git_diff_click_right_side_selects_patch() {
        let mut screen = open_screen();

        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, true);

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (60, 3), KeyModifiers::NONE)).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&buffer, false);
    }

    #[test]
    fn git_diff_click_narrow_layout_always_patch() {
        let mut screen = open_screen();

        let before = render_git_diff(&mut screen, 60, 40);
        let (column, code_row) = position_of(&before, "pub fn library").expect("diff line");

        assert!(
            screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (column, code_row), KeyModifiers::NONE)).is_empty()
        );
        let selected = render_git_diff(&mut screen, 60, 40);
        assert!(!buffer_text(&selected).contains("save"), "a click on a line selects it without drafting");

        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (2, code_row), KeyModifiers::NONE)).is_empty());
        let buffer = render_git_diff(&mut screen, 60, 40);
        assert!(buffer_text(&buffer).contains("save"), "a click in a line's gutter drafts a comment");
    }

    #[test]
    fn git_diff_click_on_border_is_ignored() {
        let mut screen = open_screen();

        let before = render_git_diff(&mut screen, 120, 40);

        // Click at y=0 (top border) — should be ignored
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (40, 0), KeyModifiers::NONE)).is_empty());

        let buffer = render_git_diff(&mut screen, 120, 40);
        assert_eq!(buffer, before, "border clicks must not alter selection or focus");
    }

    #[test]
    fn git_diff_ctrl_hover_over_a_name_opens_its_definition_card() {
        let mut screen = git_diff_screen(vec![
            FileDiff::from_texts("src/main.rs", "", "fn main() {\n    double(1);\n}\n").unwrap(),
            FileDiff::from_texts("src/lib.rs", "", "pub fn double(value: u32) -> u32 {\n    value * 2\n}\n").unwrap(),
        ]);
        let before = render_git_diff(&mut screen, 120, 40);
        let (column, row) = position_of(&before, "double(1)").expect("call site is visible");

        screen.on_ui_event(&UiEvent::Mouse(MouseAction::Moved, (column, row), KeyModifiers::NONE));
        let plain = render_git_diff(&mut screen, 120, 40);
        assert!(
            !buffer_text(&plain).contains("1 use in this diff"),
            "a plain hover opens nothing:\n{}",
            buffer_text(&plain)
        );

        screen.on_ui_event(&UiEvent::Mouse(MouseAction::Moved, (column + 1, row), KeyModifiers::CONTROL));
        let held = render_git_diff(&mut screen, 120, 40);
        assert!(
            buffer_text(&held).contains("1 use in this diff"),
            "a ctrl hover opens the card:\n{}",
            buffer_text(&held)
        );
    }

    #[test]
    fn git_diff_click_after_resize_uses_new_pane_rects() {
        let mut screen = open_screen();

        render_git_diff(&mut screen, 60, 40);
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3), KeyModifiers::NONE)).is_empty());
        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Char('c'))));
        let draft = render_git_diff(&mut screen, 60, 40);
        assert!(buffer_text(&draft).contains("save"));
        screen.on_ui_event(&UiEvent::Key(key(KeyCode::Esc)));

        let before = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&before, false);
        assert!(screen.on_ui_event(&UiEvent::Mouse(MouseAction::Click, (20, 3), KeyModifiers::NONE)).is_empty());
        let after = render_git_diff(&mut screen, 120, 40);
        assert_drawer_focus(&after, true);
    }
}
