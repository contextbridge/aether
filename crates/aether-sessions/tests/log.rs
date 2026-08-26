use aether_sessions::{SessionLog, SessionLogEntry, SessionLogError};
use std::io::Cursor;

const META: &str =
    r#"{"sessionId":"session-1","cwd":"/tmp/project","model":"test-model","createdAt":"2026-01-01T00:00:00Z"}"#;
const USER: &str = r#"{"kind":"user","data":{"type":"message","content":[{"type":"text","text":"Hello"}]}}"#;
const TRANSIENT: &str = r#"{"kind":"agent","data":{"category":"message","event":{"type":"text","message_id":"message-1","chunk":"partial","is_complete":false}}}"#;

#[test]
fn reads_metadata_skips_blanks_and_classifies_entries() {
    let input = format!("\n{META}\n\n{USER}\n{TRANSIENT}\nnot-json\n");
    let mut log = SessionLog::from_reader(Cursor::new(input)).unwrap();

    assert_eq!(log.meta.session_id, "session-1");
    assert_eq!(log.meta.model, "test-model");

    let first = log.next_entry().unwrap().unwrap();
    assert!(matches!(first, SessionLogEntry::Persisted { ref line, .. } if line.line_number == 4));
    let transient = log.next_entry().unwrap().unwrap();
    assert!(matches!(transient, SessionLogEntry::Transient { ref line } if line.line_number == 5));
    let malformed = log.next_entry().unwrap().unwrap();
    assert!(matches!(malformed, SessionLogEntry::Malformed { ref line, .. } if line.line_number == 6));
    assert!(log.next_entry().unwrap().is_none());
}

#[test]
fn reports_missing_and_invalid_metadata() {
    assert!(matches!(SessionLog::from_reader(Cursor::new("\n\n")), Err(SessionLogError::MissingMetadata)));
    assert!(matches!(
        SessionLog::from_reader(Cursor::new("not-json\n")),
        Err(SessionLogError::InvalidMetadata { line_number: 1, .. })
    ));
}
