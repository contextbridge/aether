use std::fs::{create_dir_all, read_to_string, write};
use std::path::Path;
use std::process::Command;

use tui::{KeyCode, KeyModifiers};

use super::common::*;

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(output.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn init_temp_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path();
    run_git(path, &["init", "--quiet"]);
    run_git(path, &["config", "user.email", "test@example.com"]);
    run_git(path, &["config", "user.name", "Test"]);
    run_git(path, &["config", "commit.gpgsign", "false"]);
    dir
}

async fn open_git_diff(renderer: &mut Renderer) -> TestResult {
    press_with_modifiers(renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await
}

#[tokio::test]
async fn git_diff_scope_cycles_between_both_unstaged_and_staged() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "two\n").unwrap();
    run_git(&dir, &["add", "a.txt"]);
    write(dir.join("a.txt"), "three\n").unwrap();
    write(dir.join("untracked.txt"), "scratch\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir).build()?;
    open_git_diff(&mut renderer).await?;
    assert_buffer_contains(renderer.writer(), "Git Diff · Both");
    assert_buffer_contains(renderer.writer(), "untracked.txt");

    press(&mut renderer, KeyCode::Char('t')).await?;
    assert_buffer_contains(renderer.writer(), "Git Diff · Unstaged");
    assert_buffer_contains(renderer.writer(), "untracked.txt");

    press(&mut renderer, KeyCode::Char('t')).await?;
    assert_buffer_contains(renderer.writer(), "Git Diff · Staged");
    assert_buffer_not_contains(renderer.writer(), "untracked.txt");
    Ok(())
}

#[tokio::test]
async fn typing_long_commit_message_keeps_diff_rows_visible() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    let mut renderer = RendererTest::new().size((50, 12)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;
    press(&mut renderer, KeyCode::Char(' ')).await?;
    press(&mut renderer, KeyCode::Char('C')).await?;
    type_string(&mut renderer, "this is a fairly long commit message that should not break layout").await?;

    assert_buffer_contains(renderer.writer(), "Git Diff");
    assert_buffer_contains(renderer.writer(), "break layout");
    Ok(())
}

#[tokio::test]
async fn typing_short_chars_in_commit_box_does_not_scroll_tall_diff() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    let initial = (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n") + "\n";
    write(dir.join("a.txt"), &initial).unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    let modified = (0..40).map(|i| format!("line {i} edited")).collect::<Vec<_>>().join("\n") + "\n";
    write(dir.join("a.txt"), &modified).unwrap();

    let mut renderer = RendererTest::new().size((80, 10)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;
    press(&mut renderer, KeyCode::Char(' ')).await?;
    press(&mut renderer, KeyCode::Char('C')).await?;

    let rows_before = renderer.writer().get_lines().iter().filter(|l| !l.trim().is_empty()).count();
    type_string(&mut renderer, "hello").await?;
    let rows_after = renderer.writer().get_lines().iter().filter(|l| !l.trim().is_empty()).count();

    assert_buffer_contains(renderer.writer(), "Git Diff");
    assert_eq!(rows_before, rows_after, "typing into the commit box must not change the number of visible rows");
    Ok(())
}

#[tokio::test]
async fn space_stages_selected_file() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;
    assert_buffer_contains(renderer.writer(), "☐");

    press(&mut renderer, KeyCode::Char(' ')).await?;

    assert_buffer_contains(renderer.writer(), "☑");
    assert!(run_git(&dir, &["status", "--porcelain"]).contains("M  a.txt"), "a.txt should be staged");
    Ok(())
}

#[tokio::test]
async fn space_toggles_staging_for_selected_directory() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    create_dir_all(dir.join("src/nested")).unwrap();
    write(dir.join("src/a.txt"), "one\n").unwrap();
    write(dir.join("src/b.txt"), "one\n").unwrap();
    write(dir.join("src/nested/c.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("src/a.txt"), "two\n").unwrap();
    write(dir.join("src/b.txt"), "two\n").unwrap();
    write(dir.join("src/nested/c.txt"), "two\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;

    assert!(
        renderer.writer().get_lines().iter().any(|line| {
            let line = line.trim_end();
            line.contains("src/") && line.contains("☐")
        }),
        "directory should show its unstaged checkbox"
    );

    press(&mut renderer, KeyCode::Char(' ')).await?;

    assert!(
        renderer.writer().get_lines().iter().any(|line| {
            let line = line.trim_end();
            line.contains("src/") && line.contains("☑")
        }),
        "directory should show its staged checkbox"
    );
    assert_eq!(run_git(&dir, &["status", "--porcelain"]), "M  src/a.txt\nM  src/b.txt\nM  src/nested/c.txt\n");

    press(&mut renderer, KeyCode::Char(' ')).await?;

    assert_eq!(run_git(&dir, &["status", "--porcelain"]), " M src/a.txt\n M src/b.txt\n M src/nested/c.txt\n");
    Ok(())
}

#[tokio::test]
async fn commit_via_composer_creates_commit_and_clears_changes() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;
    press(&mut renderer, KeyCode::Char(' ')).await?;
    press(&mut renderer, KeyCode::Char('C')).await?;
    type_string(&mut renderer, "second commit").await?;
    assert_buffer_contains(renderer.writer(), "second commit");

    press(&mut renderer, Enter).await?;

    assert!(run_git(&dir, &["log", "--oneline"]).contains("second commit"), "commit should be created");
    assert_buffer_contains(renderer.writer(), "No changes");
    Ok(())
}

#[tokio::test]
async fn commit_with_nothing_staged_shows_hint() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "one\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "one\ntwo\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;

    press(&mut renderer, KeyCode::Char('C')).await?;

    assert_buffer_contains(renderer.writer(), "Nothing staged");
    Ok(())
}

#[tokio::test]
async fn discard_confirm_reverts_file() -> TestResult {
    let repo = init_temp_repo();
    let dir = repo.path().to_path_buf();
    write(dir.join("a.txt"), "v1\n").unwrap();
    run_git(&dir, &["add", "-A"]);
    run_git(&dir, &["commit", "--quiet", "-m", "init"]);
    write(dir.join("a.txt"), "v2\n").unwrap();

    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).working_dir(dir.clone()).build()?;
    open_git_diff(&mut renderer).await?;

    press(&mut renderer, KeyCode::Char('d')).await?;
    assert_buffer_contains(renderer.writer(), "Discard");

    press(&mut renderer, KeyCode::Char('y')).await?;

    assert_eq!(read_to_string(dir.join("a.txt")).unwrap(), "v1\n", "file should be reverted");
    assert_buffer_contains(renderer.writer(), "No changes");
    Ok(())
}

#[tokio::test]
async fn ctrl_g_toggles_git_diff_and_mouse_capture() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    assert!(!renderer.needs_mouse_capture());

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(renderer.needs_mouse_capture(), "git diff mode should capture mouse input");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(!renderer.needs_mouse_capture(), "closing git diff should release mouse capture");
    Ok(())
}

#[tokio::test]
async fn ctrl_g_is_ignored_while_modal_is_open() -> TestResult {
    let mut renderer = open_settings(&make_settings_options(), (TEST_WIDTH, 40)).await?;
    assert!(has_settings_menu(renderer.writer()), "settings menu should be visible");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;

    assert!(has_settings_menu(renderer.writer()), "settings menu should remain visible");
    assert!(renderer.needs_mouse_capture(), "modal should continue capturing mouse input");
    Ok(())
}

#[tokio::test]
async fn esc_in_git_diff_does_not_cancel_waiting_prompt() -> TestResult {
    let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;

    type_string(&mut renderer, "hello").await?;
    press(&mut renderer, Enter).await?;
    assert_buffer_contains(renderer.writer(), "esc to interrupt");

    press_with_modifiers(&mut renderer, KeyCode::Char('g'), KeyModifiers::CONTROL).await?;
    assert!(renderer.needs_mouse_capture(), "git diff should be active");

    press(&mut renderer, Esc).await?;

    assert!(!renderer.needs_mouse_capture(), "Esc in git diff should close diff mode");
    assert_buffer_contains(renderer.writer(), "esc to interrupt");
    Ok(())
}
