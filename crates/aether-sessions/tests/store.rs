use acp_utils::notifications::SessionPreviewRole;
use aether_sessions::testing::{
    DEFAULT_CREATED_AT, TestStore, agent_switched, assistant_text, partial_text, session_meta, tool_call, user_message,
    user_message_with,
};
use aether_sessions::{ScanLimits, SessionStore, SessionStoreError};
use llm::ContentBlock;
use std::io::Write;
use std::path::{Path, PathBuf};

#[test]
fn append_and_load_roundtrip_preserves_metadata_and_persisted_events() {
    let store = TestStore::new();
    let meta = session_meta("session-1", DEFAULT_CREATED_AT);
    store.append_meta("session-1", &meta);
    let user = user_message("Hello");
    store.append("session-1", &user);
    store.append("session-1", &partial_text("message-1", "partial"));
    store.append("session-1", &assistant_text("message-1", "Hi there"));

    let (loaded_meta, events) = store.store().load("session-1").expect("session exists");
    assert_eq!(loaded_meta, meta);
    assert_eq!(events, vec![user, assistant_text("message-1", "Hi there")]);
}

#[test]
fn load_ignores_malformed_trailing_event_lines() {
    let store = TestStore::new();
    let meta = serde_json::to_string(&session_meta("session-1", DEFAULT_CREATED_AT)).unwrap();
    let event = user_message("valid");
    let user = serde_json::to_string(&event).unwrap();
    store.write_raw("session-1.jsonl", &format!("{meta}\n{user}\n{{partial json\n"));

    let (_, events) = store.store().load("session-1").expect("metadata is valid");
    assert_eq!(events, vec![event]);
}

#[test]
fn list_sorts_sessions_and_extracts_first_user_title() {
    let store = TestStore::new().session("old", &[user_message("old title")]);
    store.append_meta("new", &session_meta("new", "2026-02-01T00:00:00Z"));
    store.append("new", &agent_switched(None, Some("coder")));
    store.append("new", &user_message("new title\nsecond line"));

    let sessions = store.store().list();
    assert_eq!(sessions.iter().map(|session| session.meta.session_id.as_str()).collect::<Vec<_>>(), ["new", "old"]);
    assert_eq!(sessions[0].title.as_deref(), Some("new title"));
}

#[test]
fn list_skips_non_session_jsonl_files_and_malformed_metadata() {
    let store = TestStore::new().session("valid", &[]);
    store.write_raw("prompt-history.jsonl", "not a session");
    store.write_raw("malformed.jsonl", "not metadata\n");
    store.write_raw("notes.txt", "ignored");

    let sessions = store.store().list();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].meta.session_id, "valid");
}

#[test]
fn prompt_search_is_smart_case_unicode_safe_and_retains_recent_entries() {
    let store = TestStore::new().session("session-1", &[]);
    for index in 0..105 {
        store.append("session-1", &user_message(&format!("prompt {index}")));
    }
    store.append("session-1", &user_message("HELLO world"));
    store.append("session-1", &user_message("café"));

    let old = store.store().search_prompts("prompt 0", None).expect("search succeeds");
    assert!(old.results.is_empty());

    let lower = store.store().search_prompts("hello", None).expect("search succeeds");
    assert_eq!(lower.results.len(), 1);
    let upper = store.store().search_prompts("Hello", None).expect("search succeeds");
    assert!(upper.results.is_empty());
    let unicode = store.store().search_prompts("fé", None).expect("search succeeds");
    let hit = &unicode.results[0];
    assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "fé");
}

#[test]
fn relocating_updates_metadata_and_derived_prompt_entries() {
    let store = TestStore::new().session("session-1", &[user_message("move me")]);

    store.store().relocate("session-1", Path::new("/tmp/new-project")).expect("relocation succeeds");

    assert_eq!(store.store().session_cwd("session-1"), Some(PathBuf::from("/tmp/new-project")));
    let response = store.store().search_prompts("move me", None).expect("search succeeds");
    assert_eq!(response.results[0].cwd, PathBuf::from("/tmp/new-project"));
}

#[test]
fn preview_returns_metadata_media_and_tool_counts() {
    let store = TestStore::new().session(
        "session-1",
        &[
            user_message("preview me"),
            assistant_text("message-1", "assistant reply"),
            tool_call("tool-1", "read", "{}"),
        ],
    );

    let preview = store.store().preview("session-1").expect("preview succeeds");
    assert_eq!(preview.session_id, "session-1");
    assert_eq!(preview.tool_call_count, 1);
    assert_eq!(preview.transcript[0].role, SessionPreviewRole::User);
    assert_eq!(preview.transcript[0].text, "preview me");
}

#[test]
fn preview_unknown_session_returns_not_found_store_error() {
    let store = TestStore::new();
    let error = store.store().preview("missing").unwrap_err();
    assert!(matches!(error, SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn prompt_search_applies_result_limit_and_reports_truncation() {
    let store = TestStore::new().session("session-1", &[]);
    for index in 0..5 {
        store.append("session-1", &user_message(&format!("matching prompt {index}")));
    }

    let response = store.store().search_prompts("matching", Some(2)).expect("search succeeds");
    assert_eq!(response.results.len(), 2);
    assert!(response.truncated);
}

#[test]
fn preview_marks_transcript_and_scan_limits_as_truncated() {
    let store = TestStore::new().session("session-1", &[]);
    for index in 0..205 {
        store.append("session-1", &user_message(&format!("prompt {index}")));
    }

    let preview = store.store().preview("session-1").expect("preview succeeds");
    assert_eq!(preview.transcript.len(), 8);
    assert!(preview.truncated);
}

#[test]
fn list_uses_media_prompt_and_truncates_long_titles() {
    let store = TestStore::new().session(
        "session-1",
        &[user_message_with(vec![ContentBlock::Image {
            data: "aW1n".to_string(),
            mime_type: "image/png".to_string(),
        }])],
    );
    let media_title = store.store().list()[0].title.clone();
    assert_eq!(media_title.as_deref(), Some("Media prompt"));

    let store = TestStore::new().session("session-1", &[user_message(&"a".repeat(120))]);
    let sessions = store.store().list();
    let title = sessions[0].title.as_deref().expect("title");
    assert!(title.ends_with('…'));
    assert!(title.len() <= 84);
}

#[test]
fn committed_event_survives_a_derived_index_failure_and_can_be_rebuilt() {
    let store = TestStore::new().session("session-1", &[]);
    std::fs::create_dir(store.path().join("prompt-history.jsonl")).unwrap();

    let event = user_message("repair me");
    store.append("session-1", &event);
    assert_eq!(store.store().load("session-1").expect("session exists").1, vec![event]);

    std::fs::remove_dir(store.path().join("prompt-history.jsonl")).unwrap();
    store.store().rebuild_prompt_history().expect("rebuild succeeds");
    assert_eq!(store.store().search_prompts("repair me", None).expect("search succeeds").results.len(), 1);
}

#[test]
fn blank_runs_and_oversized_lines_consume_preview_budget() {
    let store = TestStore::new().session("session-1", &[]);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(store.path().join("session-1.jsonl"))
        .expect("session file exists");
    write!(file, "{}", "\n".repeat(ScanLimits::PREVIEW.max_bytes + 1)).unwrap();
    writeln!(file, "{}", serde_json::to_string(&user_message("must not deserialize")).unwrap()).unwrap();

    let preview = store.store().preview("session-1").expect("preview succeeds");
    assert!(preview.truncated);
    assert!(preview.transcript.is_empty());
}

#[test]
fn empty_and_missing_stores_have_no_sessions_or_prompts() {
    let store = TestStore::new();
    assert!(store.store().list().is_empty());
    let missing_session = store.store().load("missing").unwrap_err();
    assert!(matches!(missing_session, SessionStoreError::Io(error) if error.kind() == std::io::ErrorKind::NotFound));
    let empty = store.store().search_prompts(" ", None).expect("search succeeds");
    assert!(empty.results.is_empty());

    let missing = SessionStore::from_path(store.path().join("missing"));
    assert!(missing.list().is_empty());
}
