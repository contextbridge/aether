use crate::row::{EventRow, event_row};
use crate::{SessionIndexError, clamp_i64};
use aether_core::session::{SessionEvent, SessionLog, SessionLogEntry, SessionLogError, SessionMeta, UserEvent};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone)]
pub(crate) struct AetherSession {
    pub source_path: PathBuf,
    pub fingerprint: FileFingerprint,
    pub meta: SessionMeta,
    pub events: Vec<EventRow>,
    pub parse_errors: Vec<SessionParseError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct FileFingerprint {
    pub file_size: i64,
    pub file_mtime_ns: i64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DiscoveredSessionFile {
    pub path: PathBuf,
    pub fingerprint: FileFingerprint,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionParseError {
    pub source_path: PathBuf,
    pub session_id: Option<String>,
    pub line_number: Option<i64>,
    pub error: String,
    pub line_excerpt: Option<String>,
}

impl AetherSession {
    pub(crate) fn parse(path: impl AsRef<Path>) -> Result<Self, SessionIndexError> {
        let path = path.as_ref();
        let fingerprint = FileFingerprint::read(path)?;
        parse_session_file(path, fingerprint)
    }

    pub(crate) fn discover_sessions(
        sessions_dir: impl AsRef<Path>,
    ) -> Result<Vec<DiscoveredSessionFile>, SessionIndexError> {
        let mut files = Vec::new();
        for entry in fs::read_dir(sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.file_name().and_then(|name| name.to_str()) == Some("prompt-history.jsonl") {
                continue;
            }
            if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
                let path = path.canonicalize()?;
                let fingerprint = FileFingerprint::read(&path)?;
                files.push(DiscoveredSessionFile { path, fingerprint });
            }
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }
}

impl FileFingerprint {
    pub(crate) fn read(path: impl AsRef<Path>) -> Result<Self, SessionIndexError> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?.duration_since(UNIX_EPOCH).unwrap_or_default();
        Ok(Self { file_size: clamp_i64(metadata.len()), file_mtime_ns: clamp_i64(modified.as_nanos()) })
    }
}

fn parse_session_file(path: &Path, fingerprint: FileFingerprint) -> Result<AetherSession, SessionIndexError> {
    let mut log = SessionLog::open(path).map_err(|e| match e {
        SessionLogError::Io(io) => SessionIndexError::Io(io),
        SessionLogError::MissingMetadata => {
            SessionIndexError::InvalidMetadata { path: path.to_path_buf(), message: "missing metadata line".into() }
        }
        SessionLogError::InvalidMetadata { line_number, source } => {
            SessionIndexError::JsonLine { path: path.to_path_buf(), line_number, source }
        }
    })?;
    let meta = log.meta.clone();

    if meta.session_id.trim().is_empty() {
        return Err(SessionIndexError::InvalidMetadata {
            path: path.to_path_buf(),
            message: "sessionId is empty".into(),
        });
    }

    let mut events = Vec::new();
    let mut parse_errors = Vec::new();
    let mut current_turn_index: Option<i64> = None;

    while let Some(entry) = log.next_entry()? {
        match entry {
            SessionLogEntry::Persisted { line, event } => {
                if matches!(event.as_ref(), SessionEvent::User(UserEvent::Message { .. })) {
                    current_turn_index = Some(current_turn_index.map_or(0, |turn| turn + 1));
                }
                events.push(event_row(
                    &meta.session_id,
                    clamp_i64(events.len()),
                    clamp_i64(line.line_number),
                    current_turn_index,
                    event.as_ref(),
                    line.raw,
                ));
            }
            SessionLogEntry::Transient { .. } => {}
            SessionLogEntry::Malformed { line, error } => parse_errors.push(SessionParseError {
                source_path: path.to_path_buf(),
                session_id: Some(meta.session_id.clone()),
                line_number: Some(clamp_i64(line.line_number)),
                error: error.to_string(),
                line_excerpt: Some(line.raw.chars().take(240).collect()),
            }),
        }
    }

    Ok(AetherSession { source_path: path.to_path_buf(), fingerprint, meta, events, parse_errors })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn discovery_filters_and_sorts_jsonl_files() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("b.jsonl"), "").unwrap();
        fs::write(temp.path().join("a.jsonl"), "").unwrap();
        fs::write(temp.path().join("prompt-history.jsonl"), "").unwrap();
        fs::write(temp.path().join("notes.txt"), "").unwrap();
        let files = AetherSession::discover_sessions(temp.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].path.ends_with("a.jsonl"));
        assert!(files[1].path.ends_with("b.jsonl"));
    }

    #[test]
    fn malformed_event_line_records_parse_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("s.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, r#"{{"sessionId":"s","cwd":"/tmp","model":"m","createdAt":"now"}}"#).unwrap();
        writeln!(file, r#"{{"kind":"user","data":{{"type":"message","content":[{{"type":"text","text":"hi"}}]}}}}"#)
            .unwrap();
        writeln!(file, "not json").unwrap();
        writeln!(
            file,
            r#"{{"kind":"agent","data":{{"category":"message","event":{{"type":"text","message_id":"m","chunk":"ok","is_complete":true}}}}}}"#
        )
        .unwrap();
        let parsed = AetherSession::parse(&path).unwrap();
        assert_eq!(parsed.events.len(), 2);
        assert_eq!(parsed.parse_errors.len(), 1);
        assert_eq!(parsed.events[0].turn_index, Some(0));
        assert_eq!(parsed.events[1].turn_index, Some(0));
    }

    #[test]
    fn streaming_events_are_dropped_during_parse() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("s.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, r#"{{"sessionId":"s","cwd":"/tmp","model":"m","createdAt":"now"}}"#).unwrap();
        writeln!(
            file,
            r#"{{"kind":"agent","data":{{"category":"message","event":{{"type":"text","message_id":"m","chunk":"part","is_complete":false}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"kind":"agent","data":{{"category":"tool","event":{{"type":"call_update","tool_call_id":"1","chunk":"x"}}}}}}"#
        )
        .unwrap();
        writeln!(
            file,
            r#"{{"kind":"agent","data":{{"category":"message","event":{{"type":"text","message_id":"m","chunk":"final","is_complete":true}}}}}}"#
        )
        .unwrap();
        let parsed = AetherSession::parse(&path).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].content.as_deref(), Some("final"));
    }
}
