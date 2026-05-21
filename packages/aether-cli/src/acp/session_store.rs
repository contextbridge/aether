use super::prompt_history_index::PromptHistoryIndex;
use acp_utils::notifications::{PromptSearchParams, PromptSearchResponse};
use aether_core::context::ext::{SessionEvent, UserEvent};
use aether_core::events::AgentMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tracing::warn;

const PROMPT_HISTORY_FILE: &str = "prompt-history.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMeta {
    pub session_id: String,
    pub cwd: PathBuf,
    pub model: String,
    #[serde(default)]
    pub selected_mode: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionSummary {
    pub meta: SessionMeta,
    pub title: Option<String>,
}

pub struct SessionStore {
    dir: PathBuf,
    prompt_history: PromptHistoryIndex,
    meta_cache: Mutex<HashMap<String, SessionMeta>>,
}

impl SessionStore {
    pub fn new() -> io::Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Home directory not found"))?;
        Ok(Self::from_path(home.join(".aether/sessions")))
    }

    pub(crate) fn from_path(dir: PathBuf) -> Self {
        let prompt_history = PromptHistoryIndex::new(dir.join(PROMPT_HISTORY_FILE));
        Self { dir, prompt_history, meta_cache: Mutex::new(HashMap::new()) }
    }

    pub fn append_meta(&self, session_id: &str, meta: &SessionMeta) -> io::Result<()> {
        self.append_line(session_id, meta)?;
        self.cache_meta(session_id, meta.clone());
        Ok(())
    }

    pub fn append_event(&self, session_id: &str, event: &SessionEvent) -> io::Result<()> {
        if is_streaming_event(event) {
            return Ok(());
        }
        self.append_line(session_id, event)?;
        if let Some(prompt) = user_prompt_text_from_event(event)
            && let Some(meta) = self.session_meta(session_id)
        {
            self.prompt_history.append_prompt(&meta, prompt)?;
        }
        Ok(())
    }

    pub fn load(&self, session_id: &str) -> Option<(SessionMeta, Vec<SessionEvent>)> {
        let path = self.session_path(session_id);
        let file = File::open(&path).ok()?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let meta_line = lines.next()?.ok()?;
        let meta: SessionMeta = match serde_json::from_str(&meta_line) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to parse session meta: {e}");
                return None;
            }
        };

        let mut events = Vec::new();
        for line in lines {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<SessionEvent>(&line) {
                Ok(event) => events.push(event),
                Err(e) => {
                    warn!("Skipping malformed session log line: {e}");
                }
            }
        }

        Some((meta, events))
    }

    pub fn list(&self) -> Vec<SessionSummary> {
        self.read_metas_sorted()
            .into_iter()
            .map(|meta| {
                let title = self.title_for_session(&meta.session_id);
                SessionSummary { meta, title }
            })
            .collect()
    }

    pub fn search_prompts(&self, params: &PromptSearchParams) -> io::Result<PromptSearchResponse> {
        self.prompt_history.search(params)
    }

    fn session_meta(&self, session_id: &str) -> Option<SessionMeta> {
        if let Some(meta) = self.lock_meta_cache().get(session_id) {
            return Some(meta.clone());
        }
        let file = File::open(self.session_path(session_id)).ok()?;
        let mut reader = BufReader::new(file);
        let mut first_line = String::new();
        reader.read_line(&mut first_line).ok()?;
        let meta: SessionMeta = serde_json::from_str(first_line.trim()).ok()?;
        self.cache_meta(session_id, meta.clone());
        Some(meta)
    }

    fn cache_meta(&self, session_id: &str, meta: SessionMeta) {
        self.lock_meta_cache().insert(session_id.to_string(), meta);
    }

    fn lock_meta_cache(&self) -> MutexGuard<'_, HashMap<String, SessionMeta>> {
        self.meta_cache.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read_metas_sorted(&self) -> Vec<SessionMeta> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut metas: Vec<SessionMeta> = entries
            .filter_map(|entry| {
                let path = entry.ok()?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("jsonl")
                    || self.prompt_history.is_index_path(&path)
                {
                    return None;
                }
                let mut first_line = String::new();
                BufReader::new(File::open(&path).ok()?).read_line(&mut first_line).ok()?;
                serde_json::from_str::<SessionMeta>(first_line.trim()).ok()
            })
            .collect();

        metas.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        metas
    }

    fn title_for_session(&self, session_id: &str) -> Option<String> {
        let file = File::open(self.session_path(session_id)).ok()?;
        let mut reader = BufReader::new(file);
        let mut first = String::new();
        reader.read_line(&mut first).ok()?;
        let mut second = String::new();
        let read = reader.read_line(&mut second).ok()?;
        if read == 0 {
            return None;
        }
        match serde_json::from_str::<SessionEvent>(second.trim()).ok()? {
            SessionEvent::User(UserEvent::Message { content }) => Some(extract_title(&content)),
            _ => None,
        }
    }

    fn append_line<T: Serialize>(&self, session_id: &str, value: &T) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.session_path(session_id);
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        let line = serde_json::to_string(value)
            .map_err(|e| io::Error::other(format!("Failed to serialize log entry: {e}")))?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    fn session_path(&self, session_id: &str) -> PathBuf {
        self.dir.join(format!("{session_id}.jsonl"))
    }
}

const MAX_TITLE_LEN: usize = 80;

fn extract_title(content: &[llm::ContentBlock]) -> String {
    let first_line =
        llm::ContentBlock::first_text(content).and_then(|text| text.lines().next()).unwrap_or("Media prompt").trim();
    if first_line.len() > MAX_TITLE_LEN {
        let end = first_line.floor_char_boundary(MAX_TITLE_LEN);
        format!("{}…", &first_line[..end])
    } else {
        first_line.to_string()
    }
}

fn user_prompt_text_from_event(event: &SessionEvent) -> Option<String> {
    match event {
        SessionEvent::User(UserEvent::Message { content }) => {
            let joined = llm::ContentBlock::join_text(content);
            if joined.is_empty() { None } else { Some(joined) }
        }
        _ => None,
    }
}

fn is_streaming_event(event: &SessionEvent) -> bool {
    matches!(
        event,
        SessionEvent::Agent(
            AgentMessage::Text { is_complete: false, .. } | AgentMessage::Thought { is_complete: false, .. }
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_core::context::ext::UserEvent;
    use llm::ToolCallResult;

    fn meta(id: &str, created_at: &str, mode: Option<&str>) -> SessionMeta {
        SessionMeta {
            session_id: id.to_string(),
            cwd: PathBuf::from("/tmp"),
            model: "test-model".to_string(),
            selected_mode: mode.map(str::to_string),
            created_at: created_at.to_string(),
        }
    }

    fn default_meta() -> SessionMeta {
        meta("s1", "2026-01-01T00:00:00Z", Some("planner"))
    }

    fn user_msg(content: &str) -> SessionEvent {
        SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text(content)] })
    }

    fn agent_text(msg_id: &str, chunk: &str, complete: bool) -> SessionEvent {
        SessionEvent::Agent(AgentMessage::Text {
            message_id: msg_id.to_string(),
            chunk: chunk.to_string(),
            is_complete: complete,
            model_name: "test".to_string(),
        })
    }

    fn temp_store() -> (tempfile::TempDir, SessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::from_path(dir.path().to_path_buf());
        (dir, store)
    }

    fn listed_title(content: Option<&str>) -> Option<String> {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        if let Some(c) = content {
            store.append_event("s1", &user_msg(c)).unwrap();
        }
        store.list().into_iter().next().unwrap().title
    }

    #[test]
    fn append_meta_persists_selected_mode_field() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        let raw = std::fs::read_to_string(dir.path().join("s1.jsonl")).unwrap();
        assert!(raw.contains("\"selectedMode\""), "missing selectedMode: {raw}");
    }

    #[test]
    fn append_and_load_roundtrip() {
        let (_dir, store) = temp_store();
        let m = default_meta();
        let user = user_msg("Hello");
        let agent = agent_text("msg_1", "Hi there!", true);

        store.append_meta("s1", &m).unwrap();
        store.append_event("s1", &user).unwrap();
        store.append_event("s1", &agent).unwrap();

        let (loaded, events) = store.load("s1").unwrap();
        assert_eq!(loaded, m);
        assert_eq!(events, vec![user, agent]);
    }

    #[test]
    fn load_skips_malformed_trailing_line() {
        let (dir, store) = temp_store();
        let m = default_meta();
        let mut file = File::create(dir.path().join("s2.jsonl")).unwrap();
        writeln!(file, "{}", serde_json::to_string(&m).unwrap()).unwrap();
        writeln!(file, "{{truncated garbage").unwrap();

        let (loaded, events) = store.load("s2").unwrap();
        assert_eq!(loaded, m);
        assert!(events.is_empty());
    }

    #[test]
    fn load_nonexistent_returns_none() {
        let (_dir, store) = temp_store();
        assert!(store.load("nonexistent").is_none());
    }

    #[test]
    fn append_drops_streaming_chunks_and_persists_everything_else() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();

        let dropped = [
            agent_text("m", "partial", false),
            SessionEvent::Agent(AgentMessage::Thought {
                message_id: "m".to_string(),
                chunk: "thinking".to_string(),
                is_complete: false,
                model_name: "test".to_string(),
            }),
        ];
        let kept = vec![
            agent_text("m", "full", true),
            SessionEvent::Agent(AgentMessage::Error { message: "oops".to_string() }),
            SessionEvent::Agent(AgentMessage::Done),
            SessionEvent::Agent(AgentMessage::ToolResult {
                result: ToolCallResult {
                    id: "1".to_string(),
                    name: "t".to_string(),
                    arguments: "{}".to_string(),
                    result: "ok".to_string(),
                },
                result_meta: None,
                model_name: "test".to_string(),
            }),
            SessionEvent::Agent(AgentMessage::ToolCallUpdate {
                tool_call_id: "1".to_string(),
                chunk: r#"{"filePath":"Cargo.toml"}"#.to_string(),
                model_name: "test".to_string(),
            }),
        ];

        for e in &dropped {
            store.append_event("s1", e).unwrap();
        }
        for e in &kept {
            store.append_event("s1", e).unwrap();
        }

        let (_, events) = store.load("s1").unwrap();
        assert_eq!(events, kept);
    }

    #[test]
    fn list_empty_and_nonexistent_dirs_return_empty() {
        let (_dir, store) = temp_store();
        assert!(store.list().is_empty());

        let missing = SessionStore::from_path(PathBuf::from("/nonexistent/path"));
        assert!(missing.list().is_empty());
    }

    #[test]
    fn list_returns_sessions_sorted_by_created_at_descending() {
        let (_dir, store) = temp_store();
        let old = meta("s-old", "2026-01-01T00:00:00Z", None);
        let new = meta("s-new", "2026-02-01T00:00:00Z", None);
        store.append_meta("s-old", &old).unwrap();
        store.append_meta("s-new", &new).unwrap();

        let ids: Vec<_> = store.list().iter().map(|s| s.meta.session_id.clone()).collect();
        assert_eq!(ids, vec!["s-new", "s-old"]);
    }

    #[test]
    fn list_skips_malformed_files() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        std::fs::write(dir.path().join("bad.jsonl"), "not valid json\n").unwrap();
        std::fs::write(dir.path().join("readme.txt"), "ignore me").unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].meta.session_id, "s1");
    }

    #[test]
    fn list_ignores_prompt_history_file() {
        let (dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].meta.session_id, "s1");
        assert!(dir.path().join(PROMPT_HISTORY_FILE).exists());
    }

    #[test]
    fn prompt_history_searches_recent_user_prompts() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();
        store.append_event("s1", &agent_text("msg", "hello from agent", true)).unwrap();

        let response = store.search_prompts(&PromptSearchParams { query: "hello".to_string(), limit: None }).unwrap();
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].prompt, "hello world");
        assert_eq!(
            &response.results[0].prompt[response.results[0].match_start..response.results[0].match_end],
            "hello"
        );
    }

    #[test]
    fn prompt_history_keeps_only_last_entries() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        for i in 0..105 {
            store.append_event("s1", &user_msg(&format!("prompt {i}"))).unwrap();
        }

        let old = store.search_prompts(&PromptSearchParams { query: "prompt 0".to_string(), limit: None }).unwrap();
        assert!(old.results.is_empty());

        let newest =
            store.search_prompts(&PromptSearchParams { query: "prompt 104".to_string(), limit: None }).unwrap();
        assert_eq!(newest.results.len(), 1);
    }

    #[test]
    fn prompt_history_matching_is_literal_smart_case_and_unicode_safe() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store.append_event("s1", &user_msg("hello world")).unwrap();
        store.append_event("s1", &user_msg("HELLO world")).unwrap();
        store.append_event("s1", &user_msg("hello.world")).unwrap();
        store.append_event("s1", &user_msg("café hello")).unwrap();

        let literal =
            store.search_prompts(&PromptSearchParams { query: "hello.world".to_string(), limit: None }).unwrap();
        assert_eq!(literal.results.len(), 1);
        assert_eq!(literal.results[0].prompt, "hello.world");

        let lower = store.search_prompts(&PromptSearchParams { query: "hello".to_string(), limit: None }).unwrap();
        assert!(lower.results.iter().any(|hit| hit.prompt == "hello world"));
        assert!(lower.results.iter().any(|hit| hit.prompt == "HELLO world"));

        let upper = store.search_prompts(&PromptSearchParams { query: "Hello".to_string(), limit: None }).unwrap();
        assert!(upper.results.is_empty());

        let unicode = store.search_prompts(&PromptSearchParams { query: "fé".to_string(), limit: None }).unwrap();
        assert_eq!(unicode.results.len(), 1);
        let hit = &unicode.results[0];
        assert_eq!(&hit.prompt[hit.match_start..hit.match_end], "fé");
    }

    #[test]
    fn list_title_extraction() {
        let cases: &[(&str, Option<&str>)] =
            &[("Fix the login bug", Some("Fix the login bug")), ("First line\nSecond\nThird", Some("First line"))];
        for (input, expected) in cases {
            assert_eq!(listed_title(Some(input)).as_deref(), *expected, "input: {input}");
        }
    }

    #[test]
    fn list_returns_none_title_when_no_user_message() {
        assert_eq!(listed_title(None), None);
    }

    #[test]
    fn list_truncates_long_title() {
        let title = listed_title(Some(&"a".repeat(120))).unwrap();
        assert!(title.len() <= 84);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn list_uses_media_prompt_title_when_no_text_blocks_exist() {
        let (_dir, store) = temp_store();
        store.append_meta("s1", &default_meta()).unwrap();
        store
            .append_event(
                "s1",
                &SessionEvent::User(UserEvent::Message {
                    content: vec![llm::ContentBlock::Image {
                        data: "aW1n".to_string(),
                        mime_type: "image/png".to_string(),
                    }],
                }),
            )
            .unwrap();

        let title = store.list().into_iter().next().unwrap().title;
        assert_eq!(title.as_deref(), Some("Media prompt"));
    }
}
