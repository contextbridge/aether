use acp_utils::notifications::SessionPreviewRole;
use aether_sessions::testing::{
    agent_switched, assistant_chunk, assistant_text, session_meta, tool_call, user_message,
};
use aether_sessions::{ScanLimits, SessionEvent, SessionStore, SessionStoreError, UserEvent};
use llm::ContentBlock;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn temp_store() -> (tempfile::TempDir, SessionStore) {
    let directory = tempfile::tempdir().expect("temporary session directory");
    let store = SessionStore::from_path(directory.path().to_path_buf());
    (directory, store)
}

#[test]
fn append_and_load_roundtrip_preserves_metadata_and_persisted_events() -> TestResult {
    let (_directory, store) = temp_store();
    let meta = session_meta("session-1").build();
    let user = user_message("Hello");
    let assistant = assistant_text("message-1", "Hi there");
    let transient = assistant_chunk("message-1", "partial");

    store.append_meta("session-1", &meta)?;
    store.append_event("session-1", &user)?;
    store.append_event("session-1", &transient)?;
    store.append_event("session-1", &assistant)?;

    let (loaded_meta, events) = store.load("session-1").expect("session exists");
    assert_eq!(loaded_meta, meta);
    assert_eq!(events, vec![user, assistant]);
    Ok(())
}

#[test]
fn load_ignores_malformed_trailing_event_lines() -> TestResult {
    let (directory, store) = temp_store();
    let mut file = File::create(directory.path().join("session-1.jsonl"))?;
    writeln!(file, "{}", serde_json::to_string(&session_meta("session-1").build())?)?;
    writeln!(file, "{}", serde_json::to_string(&user_message("valid"))?)?;
    writeln!(file, "{{partial json")?;

    let (_, events) = store.load("session-1").expect("metadata is valid");
    assert_eq!(events, vec![user_message("valid")]);
    Ok(())
}

#[test]
fn list_sorts_sessions_and_extracts_first_user_title() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("old", &session_meta("old").created_at("2026-01-01T00:00:00Z").build())?;
    store.append_event("old", &user_message("old title"))?;
    store.append_meta("new", &session_meta("new").created_at("2026-02-01T00:00:00Z").build())?;
    store.append_event("new", &agent_switched(None, Some("coder")))?;
    store.append_event("new", &user_message("new title\nsecond line"))?;

    let sessions = store.list();
    assert_eq!(sessions.iter().map(|session| session.meta.session_id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
    assert_eq!(sessions[0].title.as_deref(), Some("new title"));
    Ok(())
}

#[test]
fn list_skips_non_session_jsonl_files_and_malformed_metadata() -> TestResult {
    let (directory, store) = temp_store();
    store.append_meta("valid", &session_meta("valid").build())?;
    std::fs::write(directory.path().join("prompt-history.jsonl"), "not a session")?;
    std::fs::write(directory.path().join("malformed.jsonl"), "not metadata\n")?;
    std::fs::write(directory.path().join("notes.txt"), "ignored")?;

    let sessions = store.list();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta.session_id, "valid");
    Ok(())
}

#[test]
fn prompt_search_is_smart_case_unicode_safe_and_retains_recent_entries() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    for index in 0..105 {
        store.append_event("session-1", &user_message(&format!("prompt {index}")))?;
    }
    store.append_event("session-1", &user_message("HELLO world"))?;
    store.append_event("session-1", &user_message("café"))?;

    let old = store.search_prompts("prompt 0", None)?;
    assert!(old.results.is_empty());

    let lower = store.search_prompts("hello", None)?;
    assert_eq!(lower.results.len(), 1);
    let upper = store.search_prompts("Hello", None)?;
    assert!(upper.results.is_empty());
    let unicode = store.search_prompts("fé", None)?;
    let hit = &unicode.results[0];
    assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "fé");
    Ok(())
}

#[test]
fn relocating_updates_metadata_and_derived_prompt_entries() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    store.append_event("session-1", &user_message("move me"))?;

    store.relocate("session-1", Path::new("/tmp/new-project"))?;

    assert_eq!(store.session_cwd("session-1"), Some(PathBuf::from("/tmp/new-project")));
    let response = store.search_prompts("move me", None)?;
    assert_eq!(response.results[0].cwd, PathBuf::from("/tmp/new-project"));
    Ok(())
}

#[test]
fn preview_returns_metadata_media_and_tool_counts() -> TestResult {
    let (_directory, store) = temp_store();
    let meta = session_meta("session-1").build();
    store.append_meta("session-1", &meta)?;
    store.append_event("session-1", &user_message("preview me"))?;
    store.append_event("session-1", &assistant_text("message-1", "assistant reply"))?;
    store.append_event("session-1", &tool_call("tool-1", "read"))?;

    let preview = store.preview("session-1")?;
    assert_eq!(preview.session_id, "session-1");
    assert_eq!(preview.tool_call_count, 1);
    assert_eq!(preview.transcript[0].role, SessionPreviewRole::User);
    assert_eq!(preview.transcript[0].text, "preview me");
    Ok(())
}

#[test]
fn preview_unknown_session_returns_not_found_store_error() {
    let (_directory, store) = temp_store();
    let error = store.preview("missing").unwrap_err();
    assert!(matches!(error, SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn prompt_search_applies_result_limit_and_reports_truncation() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    for index in 0..5 {
        store.append_event("session-1", &user_message(&format!("matching prompt {index}")))?;
    }

    let response = store.search_prompts("matching", Some(2))?;
    assert_eq!(response.results.len(), 2);
    assert!(response.truncated);
    Ok(())
}

#[test]
fn preview_marks_transcript_and_scan_limits_as_truncated() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    for index in 0..205 {
        store.append_event("session-1", &user_message(&format!("prompt {index}")))?;
    }

    let preview = store.preview("session-1")?;
    assert_eq!(preview.transcript.len(), 8);
    assert!(preview.truncated);
    Ok(())
}

#[test]
fn list_uses_media_prompt_and_truncates_long_titles() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    store.append_event(
        "session-1",
        &SessionEvent::User(UserEvent::Message {
            content: vec![ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() }],
        }),
    )?;
    let media_title = store.list()[0].title.clone();
    assert_eq!(media_title.as_deref(), Some("Media prompt"));

    let (_directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    store.append_event("session-1", &user_message(&"a".repeat(120)))?;
    let sessions = store.list();
    let title = sessions[0].title.as_deref().expect("title");
    assert!(title.ends_with('…'));
    assert!(title.len() <= 84);
    Ok(())
}

#[test]
fn committed_event_survives_a_derived_index_failure_and_can_be_rebuilt() -> TestResult {
    let (directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    std::fs::create_dir(directory.path().join("prompt-history.jsonl"))?;

    store.append_event("session-1", &user_message("repair me"))?;
    assert_eq!(store.load("session-1")?.1, vec![user_message("repair me")]);

    std::fs::remove_dir(directory.path().join("prompt-history.jsonl"))?;
    store.rebuild_prompt_history()?;
    assert_eq!(store.search_prompts("repair me", None)?.results.len(), 1);
    Ok(())
}

#[test]
fn blank_runs_and_oversized_lines_consume_preview_budget() -> TestResult {
    let (directory, store) = temp_store();
    store.append_meta("session-1", &session_meta("session-1").build())?;
    let mut file = std::fs::OpenOptions::new().append(true).open(directory.path().join("session-1.jsonl"))?;
    write!(file, "{}", "\n".repeat(ScanLimits::PREVIEW.max_bytes + 1))?;
    writeln!(file, "{}", serde_json::to_string(&user_message("must not deserialize"))?)?;

    let preview = store.preview("session-1")?;
    assert!(preview.truncated);
    assert!(preview.transcript.is_empty());
    Ok(())
}

#[test]
fn empty_and_missing_stores_have_no_sessions_or_prompts() -> TestResult {
    let (directory, store) = temp_store();
    assert!(store.list().is_empty());
    let missing_session = store.load("missing").unwrap_err();
    assert!(matches!(missing_session, SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound));
    let empty = store.search_prompts(" ", None)?;
    assert!(empty.results.is_empty());

    let missing = SessionStore::from_path(directory.path().join("missing"));
    assert!(missing.list().is_empty());
    Ok(())
}
