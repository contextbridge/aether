use unicode_width::UnicodeWidthStr;
use wisp::command::GitCommand;

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
fn external_edits_refresh_an_idle_git_review() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.executor_mut().git_mut().write_file("lib.rs", "fn external_edit() {}\n");
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("external_edit");
    ui.assert_viewport_not_contains("fn new()");
}

#[test]
fn external_file_additions_and_removals_refresh_the_drawer() {
    let mut ui = open_diff(FakeGit::new("/workspace"), 120);
    ui.executor_mut().git_mut().add_file("external.rs", "fn added() {}\n");
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("external.rs");

    ui.executor_mut().git_mut().remove_file("external.rs");
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_not_contains("external.rs");
}

#[test]
fn a_failed_initial_watch_can_be_retried_with_manual_refresh() {
    let mut ui = open_diff(FakeGit::not_a_repository("/workspace"), 160);
    ui.assert_viewport_contains("path is not inside a Git worktree");
    ui.executor_mut().git_mut().set_repository_available(true);
    ui.executor_mut().git_mut().add_file("recovered.rs", "fn recovered() {}\n");
    ui.key(key(KeyCode::Char('r')));
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("recovered.rs");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn background_refresh_retains_comments_and_selected_file() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep this feedback");
    ui.key(key(KeyCode::Enter));
    ui.executor_mut().git_mut().add_file("a.rs", "fn another_file() {}\n");
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("keep this feedback");
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_contains("a.rs");
}

#[test]
fn background_refresh_waits_until_a_comment_draft_closes() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("unfinished feedback");
    ui.executor_mut().git_mut().write_file("lib.rs", "fn intermediate() {}\n");
    ui.settle_tasks();
    ui.executor_mut().git_mut().write_file("lib.rs", "fn latest_edit() {}\n");
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("unfinished feedback");
    ui.assert_viewport_not_contains("latest_edit");
    ui.key(key(KeyCode::Esc));
    ui.draw();
    ui.assert_viewport_contains("latest_edit");
    ui.assert_viewport_not_contains("intermediate");
}

#[test]
fn background_failure_retains_the_document_and_recovers_without_content_changes() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    ui.executor_mut().git_mut().set_repository_available(false);
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_contains("path is not inside a Git worktree");
    ui.executor_mut().git_mut().set_repository_available(true);
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn retained_watch_snapshot_survives_a_later_failure_while_editing() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("unfinished feedback");
    ui.executor_mut().git_mut().write_file("lib.rs", "fn retained_edit() {}\n");
    // Coalescing may skip a successful event before delivering the next failure.
    let _ = ui.executor_mut().next_git_watch_event().expect("changed snapshot");
    ui.executor_mut().git_mut().set_repository_available(false);
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("unfinished feedback");
    ui.assert_viewport_not_contains("retained_edit");
    ui.key(key(KeyCode::Esc));
    ui.draw();
    ui.assert_viewport_contains("retained_edit");
    ui.assert_viewport_contains("path is not inside a Git worktree");
    ui.executor_mut().git_mut().set_repository_available(true);
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("retained_edit");
    ui.assert_viewport_not_contains("path is not inside a Git worktree");
}

#[test]
fn manual_refresh_installs_the_latest_watcher_state() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    ui.executor_mut().git_mut().write_file("lib.rs", "fn stale_edit() {}\n");
    ui.executor_mut().git_mut().write_file("lib.rs", "fn current_edit() {}\n");
    ui.key(key(KeyCode::Char('r')));
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("current_edit");
    ui.assert_viewport_not_contains("stale_edit");
}

#[test]
fn background_snapshots_follow_scope_changes_and_reject_old_scope_updates() {
    let mut git = changed_git("lib.rs", "fn original() {}\n", "fn staged_version() {}\n");
    git.stage_all();
    git.write_file("lib.rs", "fn working_version() {}\n");
    let mut ui = open_diff(git, 160);
    ui.executor_mut().git_mut().write_file("lib.rs", "fn external_version() {}\n");
    let stale = ui.executor_mut().next_git_watch_event().expect("changed snapshot");
    for _ in 0..2 {
        ui.key(key(KeyCode::Char('S')));
        ui.settle_tasks();
    }
    ui.deliver_result(CommandResult::GitWatch(stale));
    ui.draw();
    ui.assert_viewport_contains("Git Diff · Staged");
    ui.assert_viewport_contains("staged_version");
    ui.assert_viewport_not_contains("external_version");
    ui.executor_mut().git_mut().stage_all();
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("external_version");
}

#[test]
fn background_updates_during_a_repository_action_do_not_lose_the_latest_snapshot() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    ui.key(key(KeyCode::Char('a')));
    ui.executor_mut().git_mut().write_file("lib.rs", "fn external_version() {}\n");
    let event = ui.executor_mut().next_git_watch_event().expect("changed snapshot");
    ui.deliver_result(CommandResult::GitWatch(event));
    ui.settle_tasks();
    ui.draw();
    ui.assert_viewport_contains("external_version");
    assert_eq!(ui.executor().git().status("lib.rs"), Some((FileStatus::Modified, StageState::Staged)));
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("lib.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn action_completion_does_not_refresh_or_install_a_snapshot() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    ui.key(key(KeyCode::Char('a')));
    let commands = ui.take_commands();
    let (review_id, action) = commands
        .into_iter()
        .find_map(|command| match command {
            Command::Git(GitCommand::Apply { review_id, action }) => Some((review_id, action)),
            _ => None,
        })
        .expect("stage action");
    ui.executor_mut().git_mut().apply(action).expect("stage file");
    ui.executor_mut().git_mut().write_file("lib.rs", "fn watcher_only() {}\n");
    ui.deliver_result(CommandResult::GitDiff(GitDiffEvent { review_id, result: Ok(()) }));
    assert!(ui.take_commands().is_empty(), "action completion must not request a refresh");
    ui.draw();
    ui.assert_viewport_contains("fn new()");
    ui.assert_viewport_not_contains("watcher_only");
    let event = ui.executor_mut().next_git_watch_event().expect("watcher update");
    ui.deliver_result(CommandResult::GitWatch(event));
    ui.draw();
    ui.assert_viewport_contains("watcher_only");
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("lib.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn a_snapshot_does_not_complete_an_in_flight_action() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    ui.key(key(KeyCode::Char('a')));
    let commands = ui.take_commands();
    let (review_id, action) = commands
        .into_iter()
        .find_map(|command| match command {
            Command::Git(GitCommand::Apply { review_id, action }) => Some((review_id, action)),
            _ => None,
        })
        .expect("stage action");
    ui.executor_mut().git_mut().apply(action).expect("stage file");
    let event = ui.executor_mut().next_git_watch_event().expect("index changed");
    ui.deliver_result(CommandResult::GitWatch(event));
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("lib.rs"), Some((FileStatus::Modified, StageState::Staged)));
    ui.deliver_result(CommandResult::GitDiff(GitDiffEvent { review_id, result: Ok(()) }));
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("lib.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn initial_watch_completion_while_help_is_open_does_not_leave_review_loading() {
    let git = changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n");
    let mut ui = TestUiBuilder::new().working_dir("/workspace").dimensions(160, 15).git(git).build();
    ui.key(ctrl('g'));
    ui.key(key(KeyCode::Char('?')));
    ui.settle_tasks();
    ui.key(key(KeyCode::Esc));
    ui.draw();
    ui.assert_viewport_contains("fn new()");
}

#[test]
fn closing_review_stops_its_watch_and_reopening_rejects_old_events() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    let old_id = ui.executor().git_watch_id().expect("active watch");
    ui.executor_mut().git_mut().write_file("lib.rs", "fn stale_edit() {}\n");
    let stale = ui.executor_mut().next_git_watch_event().expect("changed snapshot");
    ui.key(key(KeyCode::Esc));
    ui.settle_tasks();
    assert!(ui.executor().git_watch_id().is_none());
    ui.executor_mut().git_mut().write_file("lib.rs", "fn current_edit() {}\n");
    assert!(ui.executor_mut().next_git_watch_event().is_none());
    ui.key(ctrl('g'));
    ui.settle_tasks();
    assert_ne!(ui.executor().git_watch_id(), Some(old_id));
    ui.deliver_result(CommandResult::GitWatch(stale));
    ui.draw();
    ui.assert_viewport_contains("current_edit");
    ui.assert_viewport_not_contains("stale_edit");
}

#[test]
fn git_diff_footer_prioritizes_contextual_actions() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 120);
    let drawer_footer = ui.viewport_text();
    assert!(drawer_footer.contains("Right open"), "{drawer_footer}");
    assert!(drawer_footer.contains("Tab switch pane"), "{drawer_footer}");
    assert!(drawer_footer.contains("? help"), "{drawer_footer}");
    assert!(!drawer_footer.contains("full file"), "secondary actions belong in shortcut help: {drawer_footer}");

    open_patch(&mut ui);
    let patch_footer = ui.viewport_text();
    assert!(patch_footer.contains("files"), "{patch_footer}");
    assert!(patch_footer.contains("? help"), "{patch_footer}");
}

#[test]
fn git_diff_shortcut_help_opens_and_closes_without_leaving_review() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 120);

    ui.key(key(KeyCode::Char('?')));
    ui.draw();
    let help = ui.viewport_text();
    assert!(help.contains("Review shortcuts"), "{help}");
    assert!(help.contains("Navigation"), "{help}");
    assert!(help.contains("Git"), "{help}");
    assert!(help.contains("j/k scroll"), "{help}");

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(ui.viewport_text().contains("Git Diff"));
    assert!(!ui.viewport_text().contains("Review shortcuts"));
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
    assert!(ui.viewport_text().contains("path is not inside a Git worktree"));

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

    ui.key(key(KeyCode::Char('S')));
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
    let old = (0..40).fold(String::new(), |mut text, index| {
        writeln!(text, "// unchanged context {index:02}").unwrap();
        text
    });
    let new = format!("{old}fn extra() {{}}\n");
    let mut ui = open_diff(changed_git("src/main.rs", &old, &new), 120);
    open_patch(&mut ui);
    assert!(!ui.viewport_text().contains("unchanged context 00"));
    ui.key(key(KeyCode::Char('f')));
    for _ in 0..60 {
        ui.key(key(KeyCode::Up));
    }
    ui.draw();
    assert!(ui.viewport_text().contains("unchanged context 00"), "{}", ui.viewport_text());

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
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("discard me");
    ui.draw();
    assert!(ui.viewport_text().contains("discard me"));
    assert!(ui.viewport_text().contains("[Esc] cancel"));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(ui.viewport_text().contains("Git Diff"));
    assert!(!ui.viewport_text().contains("discard me"));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
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
fn refreshing_the_document_retains_review_comments() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(key(KeyCode::Enter));
    ui.key(key(KeyCode::Char('r')));
    ui.settle_tasks();
    ui.draw();
    assert!(ui.viewport_text().contains("keep me"), "{}", ui.viewport_text());
}

#[test]
fn escape_closes_the_library_review_with_queued_comments() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(key(KeyCode::Enter));

    ui.key(key(KeyCode::Esc));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
    assert!(ui.next_agent_command().is_none(), "cancel does not submit feedback");
}

#[test]
fn ctrl_g_only_closes_in_browse_not_draft_or_help() {
    let mut ui = open_diff(changed_git("lib.rs", "fn old() {}\n", "fn new() {}\n"), 160);
    open_patch(&mut ui);
    ui.key(key(KeyCode::Char('c')));
    ui.type_text("keep me");
    ui.key(ctrl('g'));
    ui.draw();
    assert!(ui.viewport_text().contains("keep me"));
    ui.key(key(KeyCode::Esc));
    ui.key(key(KeyCode::Char('?')));
    ui.key(ctrl('g'));
    ui.draw();
    assert!(ui.viewport_text().contains("Review shortcuts"));
    ui.key(key(KeyCode::Esc));
    ui.key(ctrl('g'));
    ui.draw();
    assert!(!ui.viewport_text().contains("Git Diff"));
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
fn old_review_completions_cannot_finish_a_new_reviews_action() {
    let mut ui = open_diff(changed_git("file.rs", "fn old() {}\n", "fn new() {}\n"), 120);
    let old_id = ui.executor().git_watch_id().expect("old review");
    ui.key(key(KeyCode::Esc));
    ui.settle_tasks();
    ui.key(ctrl('g'));
    ui.settle_tasks();
    let review_id = ui.executor().git_watch_id().expect("new review");
    assert_ne!(old_id, review_id);
    ui.key(key(KeyCode::Char('a')));
    let commands = ui.take_commands();
    let action = commands
        .into_iter()
        .find_map(|command| match command {
            Command::Git(GitCommand::Apply { action, .. }) => Some(action),
            _ => None,
        })
        .expect("stage action");
    ui.executor_mut().git_mut().apply(action).unwrap();
    let event = ui.executor_mut().next_git_watch_event().expect("staged snapshot");
    ui.deliver_result(CommandResult::GitWatch(event));
    ui.deliver_result(CommandResult::GitDiff(GitDiffEvent { review_id: old_id, result: Ok(()) }));
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("file.rs"), Some((FileStatus::Modified, StageState::Staged)));
    ui.deliver_result(CommandResult::GitDiff(GitDiffEvent { review_id, result: Ok(()) }));
    ui.key(key(KeyCode::Char('A')));
    ui.settle_tasks();
    assert_eq!(ui.executor().git().status("file.rs"), Some((FileStatus::Modified, StageState::Unstaged)));
}

#[test]
fn git_theme_picker_routes_selection_to_global_settings() {
    let mut ui = open_diff(FakeGit::new("/workspace"), 120);
    ui.key(key(KeyCode::Char('t')));
    ui.draw();
    ui.key(key(KeyCode::Enter));
    assert!(ui.take_commands().iter().any(|command| matches!(command,
        Command::Filesystem(FilesystemCommand::ApplyTheme { settings }) if settings.theme.selection_id() == "builtin:sage"
    )));
}

#[test]
fn opening_review_schedules_theme_discovery() {
    let mut ui = TestUiBuilder::new().working_dir("/workspace").build();
    ui.key(ctrl('g'));
    assert!(
        ui.take_commands()
            .iter()
            .any(|command| matches!(command, Command::Filesystem(FilesystemCommand::ListReviewThemes)))
    );
}

#[test]
fn discovered_custom_review_theme_uses_its_filename_globally() {
    use clankerdiff_ratatui::ThemeChoice;
    use clankerdiff_ratatui::theme::{ReviewTheme, ThemeId};
    let mut ui = open_diff(FakeGit::new("/workspace"), 120);
    let theme =
        ReviewTheme::from_bytes(ThemeId::Custom("custom.json".into()), &ReviewTheme::default().to_bytes().unwrap())
            .unwrap();
    ui.deliver_result(CommandResult::ReviewThemesListed(vec![ThemeChoice::new("My custom theme", theme.clone())]));
    ui.key(key(KeyCode::Char('t')));
    ui.draw();
    assert!(ui.viewport_text().contains("My custom theme"));
    ui.key(key(KeyCode::Enter));
    let settings = ui
        .take_commands()
        .into_iter()
        .find_map(|command| match command {
            Command::Filesystem(FilesystemCommand::ApplyTheme { settings }) => {
                assert_eq!(settings.theme.selection_id(), "file:custom.json");
                Some(settings)
            }
            _ => None,
        })
        .expect("custom selection schedules application");
    ui.deliver_result(CommandResult::ThemeApplied(Ok((settings, Theme::from_review(theme.clone())))));
    ui.draw();
    assert_eq!(ui.app().ui_settings().theme.selection_id(), "file:custom.json");
    assert_eq!(ui.app().theme().review().revision(), theme.revision());
}

#[test]
fn failed_review_theme_selection_restores_installed_colors() {
    let mut ui = open_diff(FakeGit::new("/workspace"), 120);
    let original = ui.backend().buffer()[(0, 0)].bg;
    ui.key(key(KeyCode::Char('t')));
    ui.draw();
    ui.key(key(KeyCode::Down));
    ui.key(key(KeyCode::Enter));
    ui.draw();
    assert_ne!(ui.backend().buffer()[(0, 0)].bg, original);
    let settings = ui
        .take_commands()
        .into_iter()
        .find_map(|command| match command {
            Command::Filesystem(FilesystemCommand::ApplyTheme { settings, .. }) => Some(settings),
            _ => None,
        })
        .expect("review selection schedules theme application");
    assert_ne!(settings.theme.selection_id(), "builtin:sage");
    ui.deliver_result(CommandResult::ThemeApplied(Err(wisp::theme::ThemeApplicationError::Save(
        std::io::Error::other("save failed"),
    ))));
    ui.draw();
    assert_eq!(ui.app().ui_settings().theme.selection_id(), "builtin:sage");
    assert_eq!(ui.backend().buffer()[(0, 0)].bg, original);
}

#[test]
fn inline_preview_renders_a_bounded_prefix_of_canonical_rows() {
    use clankerdiff_ratatui::{DiffPreviewOptions, DiffPreviewState};
    use wisp::git_review::FileDiff;

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
        ui.draw();
        let conversation = ui.conversation();
        let start = row_containing(&conversation, "@@").expect("diff hunk header");
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
