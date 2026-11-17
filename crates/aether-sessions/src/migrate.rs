//! One-time upgrade of session logs written by older Aether versions.

use aether_core::events::CompactionId;
use llm::MessageId;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use utils::fs::write_atomic;

use crate::SessionEvent;
use crate::store::discover_session_files;

/// What [`migrate_sessions`] changed. Each rewritten log keeps its original beside it as `<name>.jsonl.bak`.
#[derive(Debug, Default)]
pub struct MigrationReport {
    pub files_scanned: usize,
    pub files_rewritten: usize,
    pub lines_upgraded: usize,
    /// Streaming updates and partial chunks that the current format never persists.
    pub lines_dropped_transient: usize,
    /// Lines with no faithful equivalent in the current format, keyed by event type and parse error.
    pub lines_dropped_unconvertible: BTreeMap<String, usize>,
}

/// Rewrites every legacy session log in `sessions_dir` into the current [`SessionEvent`] format.
pub fn migrate_sessions(sessions_dir: &Path) -> io::Result<MigrationReport> {
    let mut report = MigrationReport::default();
    for file in discover_session_files(sessions_dir)? {
        report.files_scanned += 1;
        let content = fs::read_to_string(&file.path)?;
        if let Some(rewritten) = LogUpgrader::default().upgrade(&content, &mut report) {
            fs::copy(&file.path, file.path.with_extension("jsonl.bak"))?;
            write_atomic(&file.path, rewritten.as_bytes())?;
            report.files_rewritten += 1;
        }
    }
    Ok(report)
}

enum Upgrade {
    Unchanged,
    Upgraded(Box<SessionEvent>),
    Transient,
    Unconvertible(String),
}

/// Upgrades one log; compaction events written before `compaction_id` existed share the id of the
/// compaction that started most recently.
#[derive(Default)]
struct LogUpgrader {
    compaction_id: Option<CompactionId>,
}

impl LogUpgrader {
    fn upgrade(mut self, content: &str, report: &mut MigrationReport) -> Option<String> {
        let mut lines = content.lines().filter(|line| !line.trim().is_empty());
        let mut rewritten = format!("{}\n", lines.next()?);
        let mut changed = false;
        for line in lines {
            match self.upgrade_line(line) {
                Upgrade::Unchanged => {
                    rewritten.push_str(line);
                    rewritten.push('\n');
                }
                Upgrade::Upgraded(event) => {
                    rewritten.push_str(&serde_json::to_string(&event).ok()?);
                    rewritten.push('\n');
                    report.lines_upgraded += 1;
                    changed = true;
                }
                Upgrade::Transient => {
                    report.lines_dropped_transient += 1;
                    changed = true;
                }
                Upgrade::Unconvertible(error) => {
                    *report.lines_dropped_unconvertible.entry(error).or_default() += 1;
                    changed = true;
                }
            }
        }
        changed.then_some(rewritten)
    }

    fn upgrade_line(&mut self, line: &str) -> Upgrade {
        if serde_json::from_str::<SessionEvent>(line).is_ok() {
            return Upgrade::Unchanged;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return Upgrade::Unconvertible("invalid JSON".to_string());
        };
        let Some(upgraded) = self.upgrade_value(value) else {
            return Upgrade::Transient;
        };
        let label = event_label(&upgraded);
        match serde_json::from_value::<SessionEvent>(upgraded) {
            Ok(event) if event.is_persisted() => Upgrade::Upgraded(Box::new(event)),
            Ok(_) => Upgrade::Transient,
            Err(error) => Upgrade::Unconvertible(format!("{label}: {}", error_kind(&error.to_string()))),
        }
    }

    fn upgrade_value(&mut self, mut value: Value) -> Option<Value> {
        let data = value.get_mut("data")?.take();
        match value.get("kind").and_then(Value::as_str) {
            Some("user") => Some(upgrade_user(data)),
            Some("agent") => {
                let (category, event) = match data.get("category").and_then(Value::as_str) {
                    Some(category) => (category.to_string(), data.get("event").cloned().unwrap_or(Value::Null)),
                    None => Self::upgrade_uncategorized(data)?,
                };
                Some(agent(&category, &self.upgrade_categorized(&category, event)))
            }
            _ => Some(json!({ "kind": value["kind"], "data": data })),
        }
    }

    fn upgrade_categorized(&mut self, category: &str, mut event: Value) -> Value {
        let kind = event["type"].as_str().unwrap_or_default().to_string();
        match (category, kind.as_str()) {
            ("context", kind @ ("compaction_started" | "compaction_ended" | "compaction_result")) => {
                if event.get("compaction_id").is_none() {
                    if kind == "compaction_started" || self.compaction_id.is_none() {
                        self.compaction_id = Some(CompactionId::new());
                    }
                    event["compaction_id"] = json!(self.compaction_id);
                }
                if kind == "compaction_result" && event.get("message_id").is_none() {
                    event["message_id"] = json!(MessageId::new());
                }
            }
            ("turn", "ended") => {
                let outcome = &mut event["outcome"];
                if outcome["status"] == "failed" && outcome.get("message_id").is_none() {
                    outcome["message_id"] = json!(MessageId::new());
                }
            }
            ("session_usage", _) => {
                if let Some(totals) = event.get_mut("totals").and_then(Value::as_object_mut) {
                    for component in COST_COMPONENTS {
                        totals.entry(component).or_insert(json!(0.0));
                    }
                }
            }
            _ => {}
        }
        event
    }

    /// Maps the flat `AgentMessage` enum (internally or externally tagged) onto `(category, event)`.
    /// `None` marks events the current format treats as transient.
    fn upgrade_uncategorized(data: Value) -> Option<(String, Value)> {
        let legacy = internally_tagged(data);
        let field = |name: &str| legacy.get(name).cloned().unwrap_or(Value::Null);
        let (category, event) = match legacy.get("type").and_then(Value::as_str)? {
            kind @ ("text" | "thought") => {
                if legacy["is_complete"] != true {
                    return None;
                }
                let event = json!({
                    "type": kind,
                    "message_id": field("message_id"),
                    "chunk": field("chunk"),
                    "is_complete": true,
                });
                ("message", event)
            }
            "tool_call" => ("tool", json!({ "type": "call", "request": field("request") })),
            "tool_result" => {
                ("tool", json!({ "type": "result", "result": field("result"), "result_meta": field("result_meta") }))
            }
            "tool_error" => ("tool", json!({ "type": "error", "error": field("error") })),
            "context_usage" => {
                let usage = json!({
                    "usage_ratio": field("usage_ratio"),
                    "context_limit": field("context_limit"),
                    "input_tokens": legacy.get("input_tokens").or_else(|| legacy.get("tokens_used")),
                });
                ("context", json!({ "type": "usage_updated", "usage": usage }))
            }
            "context_compaction_started" => {
                ("context", json!({ "type": "compaction_started", "message_count": field("message_count") }))
            }
            "context_compaction_result" => {
                let event = json!({
                    "type": "compaction_result",
                    "summary": field("summary"),
                    "messages_removed": field("messages_removed"),
                });
                ("context", event)
            }
            "context_cleared" => ("context", json!({ "type": "cleared" })),
            "model_switched" => {
                ("model", json!({ "type": "switched", "previous": field("previous"), "new": field("new") }))
            }
            "retrying" => {
                let event = json!({
                    "type": "retry_scheduled",
                    "purpose": "chat",
                    "attempt": field("attempt"),
                    "max_attempts": field("max_attempts"),
                    "delay_ms": field("delay_ms"),
                });
                ("turn", event)
            }
            "done" => turn_ended(&json!({ "status": "completed" })),
            "cancelled" => turn_ended(&json!({ "status": "cancelled" })),
            "error" => {
                turn_ended(&json!({ "status": "failed", "message_id": MessageId::new(), "error": field("message") }))
            }
            _ => return None,
        };
        Some((category.to_string(), event))
    }
}

const COST_COMPONENTS: [&str; 4] =
    ["estimated_input_usd", "estimated_output_usd", "estimated_cache_read_usd", "estimated_cache_creation_usd"];

fn upgrade_user(mut data: Value) -> Value {
    if data["type"] == "clearContext" {
        return agent("context", &json!({ "type": "cleared" }));
    }
    if let Some(text) = data["content"].as_str() {
        data["content"] = json!([{ "type": "text", "text": text }]);
    }
    if data.get("message_id").is_none() {
        data["message_id"] = json!(MessageId::new());
    }
    json!({ "kind": "user", "data": data })
}

/// Externally tagged logs wrote `{"ToolCall": {..}}` or `"Done"`; normalize them to `{"type": "tool_call", ..}`.
fn internally_tagged(data: Value) -> Map<String, Value> {
    match data {
        Value::String(variant) => Map::from_iter([("type".to_string(), json!(snake_case(&variant)))]),
        Value::Object(map) if map.contains_key("type") => map,
        Value::Object(map) if map.len() == 1 => {
            let Some((variant, inner)) = map.into_iter().next() else { return Map::new() };
            let mut inner = match inner {
                Value::Object(inner) => inner,
                _ => Map::new(),
            };
            let tag = if variant == "ContextUsageUpdate" { "context_usage".to_string() } else { snake_case(&variant) };
            inner.insert("type".to_string(), json!(tag));
            inner
        }
        _ => Map::new(),
    }
}

fn turn_ended(outcome: &Value) -> (&'static str, Value) {
    ("turn", json!({ "type": "ended", "outcome": outcome }))
}

fn agent(category: &str, event: &Value) -> Value {
    json!({ "kind": "agent", "data": { "category": category, "event": event } })
}

fn event_label(value: &Value) -> String {
    let data = &value["data"];
    let kind = value["kind"].as_str().unwrap_or("?");
    match data.get("category").and_then(Value::as_str) {
        Some(category) => format!("{kind}/{category}/{}", data["event"]["type"].as_str().unwrap_or("-")),
        None => format!("{kind}/{}", data["type"].as_str().unwrap_or("-")),
    }
}

fn snake_case(variant: &str) -> String {
    let mut snake = String::with_capacity(variant.len() + 4);
    for (index, character) in variant.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.push(character.to_ascii_lowercase());
    }
    snake
}

fn error_kind(error: &str) -> String {
    error.split(" at line ").next().unwrap_or(error).chars().take(120).collect()
}
