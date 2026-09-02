use aether_core::events::{AgentEvent, ContextEvent, ModelEvent, TurnEvent, TurnOutcome};
use aether_sessions::analytics::{IngestOptions, QueryOptions, SessionIndexError, ingest_sessions, run_query};
use aether_sessions::{SessionEvent, UserEvent};
use llm::testing::session_usage_event;
use llm::{
    ContextUsage, LlmCallPurpose, ModelIdentity, SessionUsageEvent, SessionUsageTotals, TokenUsage, UsageCost,
    UsageSource, Usd,
};
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn shared_discovery_filters_and_sorts_session_files_with_fingerprints() {
    let temp = TempDir::new().unwrap();
    fs::write(temp.path().join("b.jsonl"), "b").unwrap();
    fs::write(temp.path().join("a.jsonl"), "aa").unwrap();
    fs::write(temp.path().join("prompt-history.jsonl"), "history").unwrap();
    fs::write(temp.path().join("notes.txt"), "notes").unwrap();

    let files = aether_sessions::discover_session_files(temp.path()).unwrap();

    assert_eq!(
        files.iter().map(|file| file.path.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(),
        ["a.jsonl", "b.jsonl"],
    );
    assert_eq!(files[0].fingerprint.file_size, 2);
    assert_eq!(files[1].fingerprint.file_size, 1);
}

#[tokio::test]
async fn typed_event_contract_populates_every_documented_view() {
    let fixture = Fixture::new();
    fixture.write_typed_session(
        "s1.jsonl",
        &[
            SessionEvent::User(UserEvent::Message { content: vec![llm::ContentBlock::text("hello")] }),
            SessionEvent::Agent(AgentEvent::Turn(TurnEvent::RetryScheduled {
                purpose: LlmCallPurpose::Chat,
                attempt: 1,
                max_attempts: 3,
                delay_ms: 10,
            })),
            SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Cancelled })),
            SessionEvent::Agent(AgentEvent::Model(ModelEvent::Switched {
                previous: "old".to_string(),
                new: "new".to_string(),
            })),
            SessionEvent::Agent(AgentEvent::Context(ContextEvent::UsageUpdated {
                usage: ContextUsage { usage_ratio: Some(0.9), ..ContextUsage::default() },
            })),
            SessionEvent::Agent(AgentEvent::SessionUsage(root_usage(&UsageSource::new("root")))),
        ],
    );

    fixture.ingest().await;

    assert_eq!(fixture.query("select count(*) from session_usage").await.rows, vec![vec![json!(1)]]);
    assert_eq!(fixture.query("select count(*) from user_messages").await.rows, vec![vec![json!(1)]]);
    assert_eq!(fixture.query("select count(*) from retries").await.rows, vec![vec![json!(1)]]);
    assert_eq!(fixture.query("select count(*) from cancellations").await.rows, vec![vec![json!(1)]]);
    assert_eq!(
        fixture.query("select model_name from events where event_type = 'model_switched'").await.rows,
        vec![vec![json!("new")]]
    );
    assert_eq!(fixture.query("select count(*) from agent_messages").await.rows, vec![vec![json!(0)]]);
}

#[tokio::test]
async fn one_database_failure_does_not_abort_other_files() {
    let fixture = Fixture::new();
    fixture.write_session_with_id("a.jsonl", "duplicate", &[user_message("duplicate", "first")]);
    fixture.write_session_with_id("b.jsonl", "duplicate", &[user_message("duplicate", "second")]);
    fixture.write_session("c.jsonl", &[user_message("c", "third")]);

    let summary = fixture.ingest().await;

    assert_eq!(summary.files_indexed, 2);
    assert_eq!(summary.files_failed, 1);
    let sessions = fixture.query("select session_id from sessions order by session_id").await;
    assert_eq!(sessions.rows, vec![vec![json!("c")], vec![json!("duplicate")]]);
}

#[tokio::test]
async fn end_to_end_ingest_query() {
    let fixture = Fixture::new();
    fixture.write_session(
        "s1.jsonl",
        &[user_message("s1", "hello"), tool_call("read"), tool_error("read"), context_usage(0.9)],
    );

    let summary = fixture.ingest().await;

    assert_eq!(summary.files_seen, 1);
    assert_eq!(summary.files_indexed, 1);
    assert_eq!(summary.events_indexed, 4);
    let output = fixture.query("select tool_name, count(*) as failures from tool_errors group by tool_name").await;
    assert_eq!(output.rows, vec![vec![json!("read"), json!(1)]]);
}

#[tokio::test]
async fn typed_projection_exposes_event_fields() {
    let fixture = Fixture::new();
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello"), tool_call("read"), context_usage(0.9)]);
    fixture.ingest().await;

    let usage = fixture.query("select usage_ratio, input_tokens from context_usage").await;
    assert_eq!(usage.rows, vec![vec![json!(0.9), json!(1)]]);

    let model = fixture.query("select model_name from tool_calls").await;
    assert_eq!(model.rows, vec![vec![serde_json::Value::Null]]);
}

#[tokio::test]
async fn session_usage_columns_are_projected_for_root_and_sub_agent_samples() {
    let fixture = Fixture::new();
    let root = UsageSource::new("root");
    let mut child = UsageSource::new("explorer");
    child.parent_agent_id = Some(root.agent_id.clone());
    child.task_id = Some("task_0".to_string());
    let child_usage = SessionUsageEvent {
        source: child,
        purpose: LlmCallPurpose::Compaction,
        totals: SessionUsageTotals {
            tokens: TokenUsage { cache_read_tokens: Some(3.into()), ..TokenUsage::new(14, 7) },
            estimated_usd: Usd::new(0.25),
            unpriced_calls: 1,
        },
        ..session_usage_event(2, TokenUsage::new(4, 2))
    };
    fixture.write_typed_session(
        "s1.jsonl",
        &[
            SessionEvent::Agent(AgentEvent::SessionUsage(root_usage(&root))),
            SessionEvent::Agent(AgentEvent::SessionUsage(child_usage)),
        ],
    );
    fixture.ingest().await;

    let rows = fixture
        .query(
            "select usage_sequence, agent_name, parent_agent_id, task_id, call_purpose, provider, model_name, input_tokens, cache_read_tokens, total_input_tokens, estimated_cost_usd, total_estimated_cost_usd, unpriced_calls from session_usage order by usage_sequence",
        )
        .await;
    assert_eq!(
        rows.rows,
        vec![
            vec![
                json!(1),
                json!("root"),
                serde_json::Value::Null,
                serde_json::Value::Null,
                json!("chat"),
                json!("anthropic"),
                json!("claude"),
                json!(10),
                json!(3),
                json!(10),
                json!(0.25),
                json!(0.25),
                json!(0),
            ],
            vec![
                json!(2),
                json!("explorer"),
                json!(root.agent_id),
                json!("task_0"),
                json!("compaction"),
                serde_json::Value::Null,
                serde_json::Value::Null,
                json!(4),
                serde_json::Value::Null,
                json!(14),
                serde_json::Value::Null,
                json!(0.25),
                json!(1),
            ],
        ]
    );
}

#[tokio::test]
async fn tool_columns_are_projected_from_typed_events() {
    let fixture = Fixture::new();
    fixture.write_session(
        "s1.jsonl",
        &[user_message("s1", "hello"), tool_call("read"), tool_result("read"), tool_error("write")],
    );
    fixture.ingest().await;

    let schema = fixture.query("select sql from sqlite_master where type = 'table' and name = 'events'").await;
    let schema_sql = schema.rows[0][0].as_str().unwrap();
    assert!(schema_sql.contains("tool_call_id text"));
    assert!(schema_sql.contains("tool_name text"));
    assert!(schema_sql.contains("tool_arguments text"));
    assert!(!schema_sql.contains("generated always"));

    let tools = fixture
        .query("select event_type, tool_call_id, tool_name, tool_arguments from events where tool_call_id is not null order by event_index")
        .await;
    assert_eq!(
        tools.rows,
        vec![
            vec![json!("tool_call"), json!("call-read"), json!("read"), json!("{}")],
            vec![json!("tool_result"), json!("call-read"), json!("read"), json!("{}")],
            vec![json!("tool_error"), json!("call-write"), json!("write"), json!("{}")],
        ]
    );
}

#[tokio::test]
async fn concurrent_ingest_indexes_multiple_changed_files_deterministically() {
    let fixture = Fixture::new();
    fixture.write_session("b.jsonl", &[user_message("b", "second")]);
    fixture.write_session("a.jsonl", &[user_message("a", "first")]);

    let summary = fixture.ingest().await;

    assert_eq!(summary.files_seen, 2);
    assert_eq!(summary.files_indexed, 2);
    let output = fixture.query("select session_id from sessions order by source_path").await;
    assert_eq!(output.rows, vec![vec![json!("a")], vec![json!("b")]]);
}

#[tokio::test]
async fn idempotent_rerun_skips_unchanged_files() {
    let fixture = Fixture::new();
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello"), tool_error("read")]);

    fixture.ingest().await;
    let second = fixture.ingest().await;

    assert_eq!(second.files_skipped_unchanged, 1);
    let output = fixture.query("select count(*) from events").await;
    assert_eq!(output.rows, vec![vec![json!(2)]]);
}

#[tokio::test]
async fn changed_file_replaces_old_rows() {
    let fixture = Fixture::new();
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello")]);
    fixture.ingest().await;
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello"), tool_error("read")]);

    fixture.ingest().await;

    let output = fixture.query("select count(*) from events").await;
    assert_eq!(output.rows, vec![vec![json!(2)]]);
}

#[tokio::test]
async fn deleted_file_pruning_removes_rows() {
    let fixture = Fixture::new();
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello"), tool_error("read")]);
    fixture.ingest().await;
    fs::remove_file(fixture.sessions_dir.join("s1.jsonl")).unwrap();

    let summary = fixture.ingest().await;

    assert_eq!(summary.stale_files_pruned, 1);
    let output = fixture.query("select count(*) from events").await;
    assert_eq!(output.rows, vec![vec![json!(0)]]);
}

#[tokio::test]
async fn malformed_event_line_is_recorded() {
    let fixture = Fixture::new();
    let path = fixture.sessions_dir.join("s1.jsonl");
    fs::write(path, format!("{}\n{}\nnot-json\n{}\n", metadata("s1"), user_message("s1", "hello"), tool_error("read")))
        .unwrap();

    let summary = fixture.ingest().await;

    assert_eq!(summary.parse_errors, 1);
    let output = fixture.query("select count(*) from events").await;
    assert_eq!(output.rows, vec![vec![json!(2)]]);
    let errors = fixture.query("select count(*) from parse_errors").await;
    assert_eq!(errors.rows, vec![vec![json!(1)]]);
}

#[tokio::test]
async fn malformed_metadata_records_file_failure() {
    let fixture = Fixture::new();
    fs::write(fixture.sessions_dir.join("bad.jsonl"), "not-json\n{}").unwrap();

    let summary = fixture.ingest().await;

    assert_eq!(summary.files_failed, 1);
    let output = fixture.query("select status, count(*) from session_files group by status").await;
    assert_eq!(output.rows, vec![vec![json!("error"), json!(1)]]);
}

#[tokio::test]
async fn valid_file_replaces_previous_metadata_failure() {
    let fixture = Fixture::new();
    fs::write(fixture.sessions_dir.join("s1.jsonl"), "not-json\n{}").unwrap();
    let first = fixture.ingest().await;
    assert_eq!(first.files_failed, 1);

    fixture.write_session("s1.jsonl", &[user_message("s1", "hello")]);
    let second = fixture.ingest().await;

    assert_eq!(second.files_indexed, 1);
    let output = fixture.query("select status, session_id, event_count from session_files").await;
    assert_eq!(output.rows, vec![vec![json!("indexed"), json!("s1"), json!(1)]]);
}

#[tokio::test]
async fn metadata_failure_can_be_recorded_repeatedly() {
    let fixture = Fixture::new();
    fs::write(fixture.sessions_dir.join("bad.jsonl"), "not-json\n{}").unwrap();
    fixture.ingest().await;
    fs::write(fixture.sessions_dir.join("bad.jsonl"), "still-not-json\n{}").unwrap();

    let second = fixture.ingest().await;

    assert_eq!(second.files_failed, 1);
    let output = fixture.query("select status, session_id from session_files").await;
    assert_eq!(output.rows, vec![vec![json!("error"), serde_json::Value::Null]]);
}

#[tokio::test]
async fn read_only_query_safety_keeps_rows() {
    let fixture = Fixture::new();
    fixture.write_session("s1.jsonl", &[user_message("s1", "hello")]);
    fixture.ingest().await;

    let result = run_query(&QueryOptions {
        db_path: fixture.db_path.clone(),
        sql: "delete from events".to_string(),
        max_rows: 100,
        max_cell_chars: 1000,
        timeout_ms: 2000,
    })
    .await;

    assert!(matches!(result, Err(SessionIndexError::Sqlx(_))));
    let output = fixture.query("select count(*) from events").await;
    assert_eq!(output.rows, vec![vec![json!(1)]]);
}

struct Fixture {
    _temp: TempDir,
    sessions_dir: std::path::PathBuf,
    db_path: std::path::PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let sessions_dir = temp.path().join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        let db_path = temp.path().join("index.sqlite");
        Self { _temp: temp, sessions_dir, db_path }
    }

    fn write_session(&self, name: &str, events: &[String]) {
        let session_id = Path::new(name).file_stem().unwrap().to_string_lossy();
        self.write_session_with_id(name, &session_id, events);
    }

    fn write_session_with_id(&self, name: &str, session_id: &str, events: &[String]) {
        let mut content = metadata(session_id);
        content.push('\n');
        for event in events {
            content.push_str(event);
            content.push('\n');
        }
        fs::write(self.sessions_dir.join(name), content).unwrap();
    }

    fn write_typed_session(&self, name: &str, events: &[SessionEvent]) {
        let serialized = events.iter().map(|event| serde_json::to_string(event).unwrap()).collect::<Vec<_>>();
        self.write_session(name, &serialized);
    }

    async fn ingest(&self) -> aether_sessions::analytics::IngestSummary {
        ingest_sessions(self.ingest_options()).await.unwrap()
    }

    async fn query(&self, sql: &str) -> aether_sessions::analytics::QueryOutput {
        run_query(&self.query_options(sql)).await.unwrap()
    }

    fn ingest_options(&self) -> IngestOptions {
        IngestOptions {
            sessions_dir: self.sessions_dir.clone(),
            db_path: self.db_path.clone(),
            prune: true,
            parse_concurrency: 2,
        }
    }

    fn query_options(&self, sql: &str) -> QueryOptions {
        QueryOptions {
            db_path: self.db_path.clone(),
            sql: sql.to_string(),
            max_rows: 100,
            max_cell_chars: 1000,
            timeout_ms: 2000,
        }
    }
}

fn root_usage(source: &UsageSource) -> SessionUsageEvent {
    let tokens = TokenUsage { cache_read_tokens: Some(3.into()), ..TokenUsage::new(10, 5) };
    SessionUsageEvent {
        source: source.clone(),
        model: ModelIdentity { provider: Some("anthropic".into()), model_id: Some("claude".into()), pricing: None },
        estimated_cost: Some(UsageCost { total_usd: Usd::new(0.25), ..UsageCost::default() }),
        totals: SessionUsageTotals { tokens, estimated_usd: Usd::new(0.25), unpriced_calls: 0 },
        ..session_usage_event(1, tokens)
    }
}

fn metadata(session_id: &str) -> String {
    json!({"sessionId":session_id,"cwd":"/repo","model":"m","selectedMode":"Coder","createdAt":"2026-01-01T00:00:00Z"})
        .to_string()
}

fn user_message(_session_id: &str, text: &str) -> String {
    json!({"kind":"user","data":{"type":"message","content":[{"type":"text","text":text}]}}).to_string()
}

fn tool_call(tool_name: &str) -> String {
    json!({"kind":"agent","data":{"category":"tool","event":{"type":"call","request":{"id":format!("call-{tool_name}"),"name":tool_name,"arguments":"{}"}}}}).to_string()
}

fn tool_result(tool_name: &str) -> String {
    json!({"kind":"agent","data":{"category":"tool","event":{"type":"result","result":{"id":format!("call-{tool_name}"),"name":tool_name,"arguments":"{}","result":"ok"},"result_meta":null}}}).to_string()
}

fn tool_error(tool_name: &str) -> String {
    json!({"kind":"agent","data":{"category":"tool","event":{"type":"error","error":{"id":format!("call-{tool_name}"),"name":tool_name,"arguments":"{}","error":"failed"}}}}).to_string()
}

fn context_usage(ratio: f64) -> String {
    json!({"kind":"agent","data":{"category":"context","event":{
        "type":"usage_updated",
        "usage":{
            "usage_ratio":ratio,
            "context_limit":100,
            "input_tokens":1
        }
    }}})
    .to_string()
}
