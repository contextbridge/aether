use super::support::*;

async fn settle_screen_effects(app: &mut App) {
    while let Some(effect) = app.take_screen_effect() {
        app.on_screen_event(effect.execute().await);
    }
}

fn run_git(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn init_git_repo(dir: &std::path::Path) {
    run_git(dir, &["init", "--quiet"]);
    run_git(dir, &["config", "user.email", "test@example.com"]);
    run_git(dir, &["config", "user.name", "Test"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn ctrl_g_opens_and_esc_closes_git_diff() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal();

    app.on_key(ctrl('g'));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Git Diff"));

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git diff"));
}

#[tokio::test]
async fn git_diff_renders_file_drawer_and_highlighted_patch() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old_lib() {}\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn old_main() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new_lib() {}\n").unwrap();
    std::fs::write(root.join("src/main.rs"), "fn new_main() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    let removed_background = renderer.theme().diff_removed_bg;
    let added_background = renderer.theme().diff_added_bg;

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff · Both"), "{viewport}");
    assert!(viewport.contains("lib.rs"), "{viewport}");
    assert!(viewport.contains("main.rs"), "{viewport}");
    assert!(viewport.contains("old_lib") && viewport.contains("new_lib"), "{viewport}");
    assert!(
        viewport.lines().any(|line| line.contains("old_lib") && line.contains("new_lib")),
        "expected wide Git patch to use split layout:\n{viewport}"
    );
    assert!(has_cell(terminal.backend().buffer(), "f", |cell| {
        cell.bg == removed_background && cell.fg != renderer.theme().diff_removed_fg
    }));
    assert!(has_cell(terminal.backend().buffer(), "f", |cell| {
        cell.bg == added_background && cell.fg != renderer.theme().diff_added_fg
    }));

    app.on_key(key(KeyCode::Char('j')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("new_main"));
}

#[tokio::test]
async fn git_diff_cycles_scope_and_stages_selected_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("tracked.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("tracked.rs"), "fn new() {}\n").unwrap();
    std::fs::write(root.join("untracked.rs"), "fn scratch() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    let status = run_git(root, &["status", "--porcelain"]);
    assert!(status.contains("M  tracked.rs"), "{status}");

    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff · Unstaged"), "{viewport}");
    assert!(viewport.contains("untracked.rs"), "{viewport}");
}

#[tokio::test]
async fn git_diff_stages_selected_directory() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn b() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/a.rs"), "fn changed_a() {}\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "fn changed_b() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Up));
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;

    let status = run_git(root, &["status", "--porcelain"]);
    assert!(status.contains("M  src/a.rs"), "{status}");
    assert!(status.contains("M  src/b.rs"), "{status}");
}

#[tokio::test]
async fn git_diff_reports_non_repository_without_blocking_close() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal_with_width(80);

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_disabled_with_nothing_staged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "content\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Nothing staged to commit"), "{viewport}");

    app.on_key(key(KeyCode::Esc));
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_success() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "my commit message");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    let log = run_git(root, &["log", "--oneline", "-1"]);
    assert!(log.contains("my commit message"), "log was: {log}");
}

#[tokio::test]
async fn git_diff_commit_empty_message_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Commit message cannot be empty"), "{viewport}");
}

#[tokio::test]
async fn git_diff_commit_esc_cancels() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "should not commit");
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Git Diff"), "should still be on diff screen:\n{viewport}");
}

#[tokio::test]
async fn git_diff_discard_confirmation_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Discard changes to"), "{viewport}");
    assert!(viewport.contains("file.txt"), "{viewport}");

    app.on_key(key(KeyCode::Char('n')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Discard"), "discard prompt should be gone:\n{viewport}");

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "changed\n");
}

#[tokio::test]
async fn git_diff_discard_reverts_modified_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "original\n");
}

#[tokio::test]
async fn git_diff_discard_removes_untracked_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("tracked.txt"), "v1\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("untracked.txt"), "scratch\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    assert!(!root.join("untracked.txt").exists());
}

#[tokio::test]
async fn git_diff_discard_restores_deleted_file() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::remove_file(root.join("file.txt")).unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('y')));
    settle_screen_effects(&mut app).await;

    let content = std::fs::read_to_string(root.join("file.txt")).unwrap();
    assert_eq!(content, "original\n");
}

#[tokio::test]
async fn git_diff_full_file_toggle_shows_content() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/main.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/main.rs"), "fn new() {}\nfn extra() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("[full file]"), "{viewport}");
    assert!(viewport.contains("fn new()"), "{viewport}");
    assert!(viewport.contains("fn extra()"), "{viewport}");

    app.on_key(key(KeyCode::Char('o')));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("[full file]"), "{viewport}");
    assert!(viewport.contains("fn old()"), "{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_shows_deleted_message() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "content\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::remove_file(root.join("file.txt")).unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("deleted"), "{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_toggle_at_narrow_width() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.rs"), "fn new() {}\nfn two() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(50);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("fn new()"), "{viewport}");
    assert!(viewport.contains("fn two()"), "{viewport}");
}

#[tokio::test]
async fn git_diff_stale_event_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.rs"), "fn one() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.rs"), "fn changed() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    let stale_event = GitDiffEvent::ActionFinished { request_id: 0, result: Ok(()) };
    app.on_screen_event(ScreenEvent::GitDiff(stale_event));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_screen_closable_on_error() {
    let directory = tempfile::tempdir().unwrap();
    let (mut app, _command_rx) = make_app_in(directory.path().to_path_buf());
    let mut terminal = make_terminal_with_width(80);

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("Not a git repository"));

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_failure_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join(".git/hooks")).ok();
    std::fs::write(root.join(".git/hooks/pre-commit"), "#!/bin/sh\necho nope >&2\nexit 1\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.join(".git/hooks/pre-commit"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init", "--no-verify"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    type_text(&mut app, "should fail");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("nope") || viewport.contains("CommandFailed") || viewport.contains("failed"),
        "expected commit error in viewport:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_binary_file_shows_label() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("image.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("image.png"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Binary file"), "expected binary file label in:\n{viewport}");
}

#[tokio::test]
async fn git_diff_full_file_binary_shows_message() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("data.bin"), b"\x00\x01\x02\x03").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("data.bin"), b"\x00\x01\x02\x03\x04").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(
        viewport.contains("Binary file") || viewport.contains("binary"),
        "expected binary message in full-file mode:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_full_file_load_error_exits_full_file_mode() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("source.rs"), "fn answer() -> u32 { 42 }\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("source.rs"), "fn answer() -> u32 { 43 }\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("[full file]"), "{viewport}");

    std::fs::remove_file(root.join("source.rs")).unwrap();
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('o')));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("[full file]"), "should exit full-file mode on load error:\n{viewport}");
    assert!(
        viewport.contains("Cannot read") || viewport.contains("source.rs"),
        "should show error in footer:\n{viewport}"
    );

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();
    assert!(!buffer_text(terminal.backend().buffer()).contains("Git Diff"));
}

#[tokio::test]
async fn git_diff_commit_editor_unicode_cursor_and_render() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("file.txt"), "original\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("file.txt"), "changed\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;

    type_text(&mut app, "héllo wörld — café");
    settle_screen_effects(&mut app).await;
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo wörld — café"), "expected unicode commit message:\n{viewport}");

    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
}

#[tokio::test]
async fn git_diff_comment_draft_submit_cancel_undo() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new() {}\nfn another() {}\nfn third() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Navigate into patch pane: move to file, then enter
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Press 'c' to start draft on first line
    app.on_key(key(KeyCode::Char('c')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Draft"), "draft should appear:\n{viewport}");

    // Type a comment
    type_text(&mut app, "this looks wrong");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("this looks wrong"), "typed text should appear in draft:\n{viewport}");

    // Submit the comment
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Comment"), "submitted comment should appear:\n{viewport}");
    assert!(viewport.contains("this looks wrong"), "submitted text should be visible:\n{viewport}");

    // Undo the comment
    app.on_key(key(KeyCode::Char('u')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("this looks wrong"), "comment should be removed after undo:\n{viewport}");

    // Esc cancels draft
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "will cancel");
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("will cancel"), "cancelled draft should not appear:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_counts_in_footer() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\nfn two() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\nfn two() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.lines().any(|l| l.contains("1 comment")), "file header should show comment count:\n{viewport}");

    // Footer should show total count - check the raw footer area
    assert!(viewport.contains("(1 comment)"), "footer should show (1 comment):\n{viewport}");
}

#[tokio::test]
async fn git_diff_comments_survive_file_switches() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("a.rs"), "fn a_old() {}\n").unwrap();
    std::fs::write(root.join("b.rs"), "fn b_old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("a.rs"), "fn a_new() {}\n").unwrap();
    std::fs::write(root.join("b.rs"), "fn b_new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add comment on first file (a.rs)
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "comment on A");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("comment on A"), "comment on A should be visible:\n{viewport}");

    // Switch to drawer and select second file
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("comment on A"), "comment on A should not appear on B:\n{viewport}");

    // Switch back to first file
    app.on_key(key(KeyCode::Char('h')));
    app.on_key(key(KeyCode::Char('k')));
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("comment on A"), "comment on A should persist after switching back:\n{viewport}");
}

#[tokio::test]
async fn git_diff_submit_review_emits_prompt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, mut command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback text");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Submit
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    // Verify the prompt was sent
    let cmd = command_rx.try_recv().unwrap();
    match cmd {
        PromptCommand::Prompt { text, content, .. } => {
            assert!(text.contains("I'm reviewing the working tree diff"), "text should contain review prefix:\n{text}");
            assert!(text.contains("## `lib.rs`"), "text should contain file header:\n{text}");
            assert!(text.contains("feedback text"), "text should contain comment body:\n{text}");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }

    // Screen should be closed after successful submit
    assert!(!app.full_screen_active(), "screen should close after submit");
}

#[tokio::test]
async fn git_diff_submit_no_comments_shows_error() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Press 's' with no comments
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();

    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("No comments to submit"), "should show error for no comments:\n{viewport}");
}

#[tokio::test]
async fn git_diff_submit_send_failure_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, fail_signal, _command_rx) = make_failable_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter)); // into patch
    settle_screen_effects(&mut app).await;

    // Add a comment
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "feedback text");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Enable send failure
    fail_signal.store(true, Ordering::SeqCst);

    // Try to submit
    app.on_key(key(KeyCode::Char('s')));
    settle_screen_effects(&mut app).await;

    // The screen should still be active (failure retains state)
    assert!(app.full_screen_active(), "screen should remain open after send failure");

    // Comments should still be visible
    app.on_key(key(KeyCode::Esc)); // close screen to check transcript
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Failed to send review"), "should show send failure message in transcript:\n{viewport}");
}

fn make_failable_app_in(working_dir: std::path::PathBuf) -> (App, Arc<AtomicBool>, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, fail_signal, command_rx) = AcpPromptHandle::failable();
    let app =
        build_app_with_handle(working_dir, acp::SessionCapabilities::new(), Vec::new(), Vec::new(), prompt_handle);
    (app, fail_signal, command_rx)
}

#[tokio::test]
async fn git_diff_comment_refresh_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "keep me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("keep me"), "comment should appear before refresh:\n{viewport}");

    // First 'r' — show confirmation
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation message should appear:\n{viewport}");
    assert!(viewport.contains("keep me"), "comment should still appear during confirmation:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("keep me"), "comment should survive cancel:\n{viewport}");
    assert!(!viewport.contains("will clear"), "confirmation message should be gone:\n{viewport}");

    // Confirm path: press r twice
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('r')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("keep me"), "comment should be cleared after confirmed refresh:\n{viewport}");
    assert!(viewport.contains("Git Diff"), "screen should still show diff:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_scope_switch_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "scope comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 't' — show confirmation
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("scope comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("scope comment"), "comment should be cleared after scope switch:\n{viewport}");
    assert!(viewport.contains("Git Diff"), "screen should still show diff:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_stage_all_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "stage me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'a' — show confirmation
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("stage me"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('a')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("stage me"), "comment should be cleared after stage-all:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_toggle_stage_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("src/lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "space comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First space — show confirmation
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("space comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char(' ')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("space comment"), "comment should be cleared after stage:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_commit_cancel_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "commit comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'C' — show confirmation (even though nothing staged, confirm appears first)
    app.on_key(key(KeyCode::Char('C')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("commit comment"), "comment should survive cancel:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_discard_cancel_preserves_comments() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "discard comment");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'd' — show confirmation
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels — comments preserved
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("discard comment"), "comment should survive cancel:\n{viewport}");

    // Confirm path — should enter discard confirmation
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('d')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Discard changes"), "discard confirmation should appear:\n{viewport}");
}

#[tokio::test]
async fn git_diff_comment_unstage_all_confirm_clears_cancel_preserves() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn changed() {}\n").unwrap();
    run_git(root, &["add", "-A"]);

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    // Switch to staged scope to see the staged file
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('t')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "unstage me");
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // First 'A' — show confirmation
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("will clear"), "confirmation should appear:\n{viewport}");

    // Esc cancels
    app.on_key(key(KeyCode::Esc));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("unstage me"), "comment should survive cancel:\n{viewport}");

    // Confirm path
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Char('A')));
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("unstage me"), "comment should be cleared after unstage-all:\n{viewport}");
}

#[tokio::test]
async fn git_diff_draft_cursor_with_unicode() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    init_git_repo(root);
    std::fs::write(root.join("lib.rs"), "fn old() {}\n").unwrap();
    run_git(root, &["add", "-A"]);
    run_git(root, &["commit", "--quiet", "-m", "init"]);
    std::fs::write(root.join("lib.rs"), "fn new() {}\n").unwrap();

    let (mut app, _command_rx) = make_app_in(root.to_path_buf());
    let mut terminal = make_terminal_with_width(160);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_key(ctrl('g'));
    settle_screen_effects(&mut app).await;
    app.on_key(key(KeyCode::Enter));
    settle_screen_effects(&mut app).await;

    // Start draft and type unicode text
    app.on_key(key(KeyCode::Char('c')));
    type_text(&mut app, "héllo wörld");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo wörld"), "unicode text should appear in draft:\n{viewport}");

    // Move cursor left and type more
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Left));
    type_text(&mut app, "★");
    settle_screen_effects(&mut app).await;
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("héllo★ wörld"), "unicode insertion should work:\n{viewport}");
}
