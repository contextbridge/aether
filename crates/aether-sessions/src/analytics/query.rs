use super::SessionIndexError;
use super::db::{Db, QueryLimits};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct QueryOptions {
    pub db_path: PathBuf,
    pub sql: String,
    pub max_rows: usize,
    pub max_cell_chars: usize,
    pub timeout_ms: u64,
}

impl QueryOptions {
    pub fn new(db_path: impl Into<PathBuf>, sql: impl Into<String>) -> Self {
        Self { db_path: db_path.into(), sql: sql.into(), max_rows: 100, max_cell_chars: 1000, timeout_ms: 2000 }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QueryOutput {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub truncated_rows: bool,
    pub truncated_cells: bool,
}

pub async fn run_query(options: &QueryOptions) -> Result<QueryOutput, SessionIndexError> {
    let sql = options.sql.trim();
    if sql.is_empty() {
        return Err(SessionIndexError::EmptyQuery);
    }
    tokio::time::timeout(Duration::from_millis(options.timeout_ms), run_query_inner(options, sql))
        .await
        .map_err(|_| SessionIndexError::QueryTimeout { timeout_ms: options.timeout_ms })?
}

pub fn render_tsv(output: &QueryOutput) -> String {
    let header = output.columns.join("\t");
    let rows = output.rows.iter().map(|row| row.iter().map(value_to_cell).collect::<Vec<_>>().join("\t"));
    std::iter::once(header).chain(rows).collect::<Vec<_>>().join("\n")
}

async fn run_query_inner(options: &QueryOptions, sql: &str) -> Result<QueryOutput, SessionIndexError> {
    let mut db = Db::open_readonly(&options.db_path).await?;
    db.query(sql, QueryLimits { max_rows: options.max_rows, max_cell_chars: options.max_cell_chars }).await
}

fn value_to_cell(value: &Value) -> String {
    let text = match value {
        Value::Null => return String::new(),
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    text.replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{Connection, Executor, SqliteConnection};
    use tempfile::TempDir;

    struct TestDb {
        _temp: TempDir,
        path: PathBuf,
    }

    async fn test_db() -> TestDb {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("db.sqlite");
        let mut conn =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(&path).create_if_missing(true))
                .await
                .unwrap();
        conn.execute("create table events (id integer, content text, payload blob); insert into events values (1, 'abcdef', x'0102'); insert into events values (2, 'ghijkl', x'03');")
            .await
            .unwrap();
        TestDb { _temp: temp, path }
    }

    #[tokio::test]
    async fn empty_query_errors() {
        let result = run_query(&QueryOptions::new("missing", " ")).await;
        assert!(matches!(result, Err(SessionIndexError::EmptyQuery)));
    }

    #[tokio::test]
    async fn select_succeeds_and_truncates() {
        let db = test_db().await;
        let options = QueryOptions {
            max_rows: 1,
            max_cell_chars: 3,
            ..QueryOptions::new(db.path, "select content, payload from events order by id")
        };
        let output = run_query(&options).await.unwrap();
        assert_eq!(output.rows, vec![vec![Value::from("abc"), Value::from("<blob 2 bytes>")]]);
        assert!(output.truncated_rows);
        assert!(output.truncated_cells);
    }

    #[tokio::test]
    async fn mutation_is_rejected_by_read_only_database() {
        let db = test_db().await;
        let result = run_query(&QueryOptions::new(db.path, "delete from events")).await;
        assert!(matches!(result, Err(SessionIndexError::Sqlx(_))));
    }

    #[tokio::test]
    async fn commented_selects_are_accepted() {
        let db = test_db().await;
        for sql in
            ["-- note\nselect content from events order by id", "/* safety */ select content from events order by id"]
        {
            let output = run_query(&QueryOptions::new(&db.path, sql)).await.unwrap();
            assert_eq!(output.rows, vec![vec![Value::from("abcdef")], vec![Value::from("ghijkl")]], "{sql}");
        }
    }

    #[test]
    fn tsv_escapes_tabs_and_newlines() {
        let output = QueryOutput {
            columns: vec!["c".to_string()],
            rows: vec![vec![Value::String("hello\tworld\nnew".to_string())]],
            truncated_rows: false,
            truncated_cells: false,
        };
        let tsv = render_tsv(&output);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "hello\\tworld\\nnew");
    }

    #[tokio::test]
    async fn trailing_mutation_cannot_write_through_read_only_connection() {
        let db = test_db().await;
        let _ = run_query(&QueryOptions::new(&db.path, "select 1; delete from events")).await;

        let output = run_query(&QueryOptions::new(&db.path, "select count(*) from events")).await.unwrap();
        assert_eq!(output.rows, vec![vec![Value::from(2)]]);
    }
}
