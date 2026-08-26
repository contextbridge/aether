use acp_utils::notifications::SessionPreviewRole;
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent};
use aether_sessions::{SessionEvent, SessionMeta, SessionStore, SessionStoreError, UserEvent};
use llm::ContentBlock;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn meta(id: &str, created_at: &str) -> SessionMeta {
    SessionMeta {
        session_id: id.to_string(),
        cwd: PathBuf::from("/tmp/project"),
        model: "test-model".to_string(),
        selected_mode: Some("planner".to_string()),
        created_at: created_at.to_string(),
    }
}

fn user_message(text: &str) -> SessionEvent {
    SessionEvent::User(UserEvent::Message { content: vec![ContentBlock::text(text)] })
}

fn assistant_message(text: &str) -> SessionEvent {
    SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
        message_id: "message-1".to_string(),
        chunk: text.to_string(),
        is_complete: true,
    }))
}

fn temp_store() -> (tempfile::TempDir, SessionStore) {
    let directory = tempfile::tempdir().expect("temporary session directory");
    let store = SessionStore::from_path(directory.path().to_path_buf());
    (directory, store)
}

#[test]
fn append_and_load_roundtrip_preserves_metadata_and_persisted_events() -> TestResult {
    let (_directory, store) = temp_store();
    let session_meta = meta("session-1", "2026-01-01T00:00:00Z");
    let user = user_message("Hello");
    let assistant = assistant_message("Hi there");
    let transient = SessionEvent::Agent(AgentEvent::Message(MessageEvent::Text {
        message_id: "message-1".to_string(),
        chunk: "partial".to_string(),
        is_complete: false,
    }));

    store.append_meta("session-1", &session_meta)?;
    store.append_event("session-1", &user)?;
    store.append_event("session-1", &transient)?;
    store.append_event("session-1", &assistant)?;

    let (loaded_meta, events) = store.load("session-1").expect("session exists");
    assert_eq!(loaded_meta, session_meta);
    assert_eq!(events, vec![user, assistant]);
    Ok(())
}

#[test]
fn load_ignores_malformed_trailing_event_lines() -> TestResult {
    let (directory, store) = temp_store();
    let session_meta = meta("session-1", "2026-01-01T00:00:00Z");
    let mut file = File::create(directory.path().join("session-1.jsonl"))?;
    writeln!(file, "{}", serde_json::to_string(&session_meta)?)?;
    writeln!(file, "{}", serde_json::to_string(&user_message("valid"))?)?;
    writeln!(file, "{{partial json")?;

    let (_, events) = store.load("session-1").expect("metadata is valid");
    assert_eq!(events, vec![user_message("valid")]);
    Ok(())
}

#[test]
fn list_sorts_sessions_and_extracts_first_user_title() -> TestResult {
    let (_directory, store) = temp_store();
    store.append_meta("old", &meta("old", "2026-01-01T00:00:00Z"))?;
    store.append_event("old", &user_message("old title"))?;
    store.append_meta("new", &meta("new", "2026-02-01T00:00:00Z"))?;
    store.append_event(
        "new",
        &SessionEvent::Control(aether_sessions::SessionControlEvent::AgentSwitched {
            from: None,
            to: Some("coder".to_string()),
        }),
    )?;
    store.append_event("new", &user_message("new title\nsecond line"))?;

    let sessions = store.list();
    assert_eq!(sessions.iter().map(|session| session.meta.session_id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
    assert_eq!(sessions[0].title.as_deref(), Some("new title"));
    Ok(())
}

#[test]
fn list_skips_non_session_jsonl_files_and_malformed_metadata() -> TestResult {
    let (directory, store) = temp_store();
    store.append_meta("valid", &meta("valid", "2026-01-01T00:00:00Z"))?;
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
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
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
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
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
    let session_meta = meta("session-1", "2026-01-01T00:00:00Z");
    store.append_meta("session-1", &session_meta)?;
    store.append_event("session-1", &user_message("preview me"))?;
    store.append_event("session-1", &assistant_message("assistant reply"))?;
    store.append_event(
        "session-1",
        &SessionEvent::Agent(AgentEvent::Tool(ToolEvent::Call {
            request: llm::ToolCallRequest {
                id: "tool-1".to_string(),
                name: "read".to_string(),
                arguments: "{}".to_string(),
            },
        })),
    )?;

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
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
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
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
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
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
    store.append_event(
        "session-1",
        &SessionEvent::User(UserEvent::Message {
            content: vec![ContentBlock::Image { data: "aW1n".to_string(), mime_type: "image/png".to_string() }],
        }),
    )?;
    let media_title = store.list()[0].title.clone();
    assert_eq!(media_title.as_deref(), Some("Media prompt"));

    let (_directory, store) = temp_store();
    store.append_meta("session-1", &meta("session-1", "2026-01-01T00:00:00Z"))?;
    store.append_event("session-1", &user_message(&"a".repeat(120)))?;
    let sessions = store.list();
    let title = sessions[0].title.as_deref().expect("title");
    assert!(title.ends_with('…'));
    assert!(title.len() <= 84);
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
