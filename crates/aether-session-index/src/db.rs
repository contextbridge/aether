use crate::query::QueryOutput;
use crate::session::{AetherSession, DiscoveredSessionFile, FileFingerprint};
use crate::{SessionIndexError, clamp_i64};
use chrono::Utc;
use futures::TryStreamExt;
use serde_json::Value;
use sqlx::migrate::{MigrateError, Migrator};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteRow};
use sqlx::{
    AssertSqlSafe, Column, Connection, Executor, QueryBuilder, Row, SqlSafeStr, Sqlite, SqliteConnection, Transaction,
    TypeInfo, ValueRef,
};
use std::collections::{HashMap, HashSet};
use std::fs::create_dir_all;
use std::path::Path;

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

pub(crate) struct Db {
    conn: SqliteConnection,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct QueryLimits {
    pub max_rows: usize,
    pub max_cell_chars: usize,
}

impl Db {
    /// Opens the index for writing and brings it to the current schema. Known
    /// schema drift is rebuilt in a temporary database before replacing the
    /// disposable cache; operational migration errors preserve the existing DB.
    pub(crate) async fn open_writable(path: &Path) -> Result<Self, SessionIndexError> {
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);

        let mut conn = SqliteConnection::connect_with(&options).await?;
        if let Err(error) = MIGRATOR.run(&mut conn).await {
            conn.close().await?;
            if !is_schema_drift(&error) {
                return Err(error.into());
            }
            rebuild_database(path, &options).await?;
            conn = SqliteConnection::connect_with(&options).await?;
        }

        Ok(Self { conn })
    }

    pub(crate) async fn open_readonly(path: &Path) -> Result<Self, SessionIndexError> {
        let options = SqliteConnectOptions::new().filename(path).read_only(true).create_if_missing(false);
        let mut conn = SqliteConnection::connect_with(&options).await?;
        conn.execute("pragma query_only = on").await?;
        let _ = conn.execute("pragma trusted_schema = off").await;

        Ok(Self { conn })
    }

    pub(crate) async fn indexed_file_fingerprints(
        &mut self,
    ) -> Result<HashMap<String, FileFingerprint>, SessionIndexError> {
        let rows = sqlx::query_as::<_, (String, i64, i64)>(
            "select source_path, file_size, file_mtime_ns from session_files where status = 'indexed'",
        )
        .fetch_all(&mut self.conn)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(source_path, file_size, file_mtime_ns)| (source_path, FileFingerprint { file_size, file_mtime_ns }))
            .collect())
    }

    pub(crate) async fn replace_session(&mut self, session: &AetherSession) -> Result<(), SessionIndexError> {
        let mut tx = self.conn.begin().await?;
        let source_path = path_to_string(&session.source_path);
        clear_source_path(&mut tx, &source_path).await?;
        insert_indexed_file(&mut tx, &source_path, session).await?;
        insert_events(&mut tx, session).await?;
        insert_parse_errors(&mut tx, session).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn record_file_error(
        &mut self,
        source_path: &Path,
        fingerprint: FileFingerprint,
        error: impl Into<String>,
    ) -> Result<(), SessionIndexError> {
        let mut tx = self.conn.begin().await?;
        let source_path = path_to_string(source_path);
        clear_source_path(&mut tx, &source_path).await?;
        let indexed_at = Utc::now().to_rfc3339();
        let error = error.into();
        sqlx::query!(
            r"
            insert into session_files (source_path, status, error, file_size, file_mtime_ns, indexed_at)
            values (?1, 'error', ?2, ?3, ?4, ?5)
            ",
            source_path,
            error,
            fingerprint.file_size,
            fingerprint.file_mtime_ns,
            indexed_at,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn prune_missing_files(
        &mut self,
        current_files: &[DiscoveredSessionFile],
    ) -> Result<usize, SessionIndexError> {
        let current: HashSet<String> = current_files.iter().map(|file| path_to_string(&file.path)).collect();
        let existing = sqlx::query_scalar!(r#"select source_path as "source_path!" from session_files"#)
            .fetch_all(&mut self.conn)
            .await?;
        let mut tx = self.conn.begin().await?;
        let mut pruned = 0;
        for source_path in existing {
            if current.contains(&source_path) {
                continue;
            }
            clear_source_path(&mut tx, &source_path).await?;
            pruned += 1;
        }
        tx.commit().await?;
        Ok(pruned)
    }

    pub(crate) async fn query(&mut self, sql: &str, limits: QueryLimits) -> Result<QueryOutput, SessionIndexError> {
        let sql = AssertSqlSafe(sql).into_sql_str();
        let describe = self.conn.describe(sql.clone()).await?;
        let columns = describe.columns().iter().map(|column| column.name().to_string()).collect::<Vec<_>>();
        let mut rows = sqlx::query(sql).fetch(&mut self.conn);
        let mut output_rows = Vec::new();
        let mut truncated_rows = false;
        let mut truncated_cells = false;

        while let Some(row) = rows.try_next().await? {
            if output_rows.len() >= limits.max_rows {
                truncated_rows = true;
                break;
            }
            let values = (0..columns.len())
                .map(|index| sqlite_value_to_json(&row, index, limits.max_cell_chars, &mut truncated_cells))
                .collect::<Result<Vec<_>, _>>()?;
            output_rows.push(values);
        }

        Ok(QueryOutput { columns, rows: output_rows, truncated_rows, truncated_cells })
    }
}

async fn insert_indexed_file(
    tx: &mut Transaction<'_, Sqlite>,
    source_path: &str,
    session: &AetherSession,
) -> Result<(), SessionIndexError> {
    let indexed_at = Utc::now().to_rfc3339();
    let event_count = clamp_i64(session.events.len());
    let parse_error_count = clamp_i64(session.parse_errors.len());
    let cwd = session.meta.cwd.to_string_lossy();
    sqlx::query!(
        r#"
        insert into session_files (
          source_path, session_id, status, file_size, file_mtime_ns, indexed_at, event_count, parse_error_count,
          cwd, model, selected_mode, created_at
        ) values (?1, ?2, 'indexed', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        "#,
        source_path,
        session.meta.session_id,
        session.fingerprint.file_size,
        session.fingerprint.file_mtime_ns,
        indexed_at,
        event_count,
        parse_error_count,
        cwd,
        session.meta.model,
        session.meta.selected_mode,
        session.meta.created_at,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_events(tx: &mut Transaction<'_, Sqlite>, session: &AetherSession) -> Result<(), SessionIndexError> {
    for chunk in session.events.chunks(30) {
        let mut builder = QueryBuilder::<Sqlite>::new(
            "insert into events (session_id, event_index, line_number, turn_index, content, content_len, raw_json, kind, event_type, outcome, tool_call_id, tool_name, tool_arguments, model_name, message_id, usage_ratio, context_limit, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, reasoning_tokens, total_input_tokens, total_output_tokens, total_cache_read_tokens, total_cache_creation_tokens, total_reasoning_tokens) ",
        );
        builder.push_values(chunk, |mut b, event| {
            b.push_bind(&event.session_id)
                .push_bind(event.event_index)
                .push_bind(event.line_number)
                .push_bind(event.turn_index)
                .push_bind(&event.content)
                .push_bind(event.content_len)
                .push_bind(&event.raw_json)
                .push_bind(event.kind)
                .push_bind(event.event_type)
                .push_bind(event.outcome)
                .push_bind(&event.tool_call_id)
                .push_bind(&event.tool_name)
                .push_bind(&event.tool_arguments)
                .push_bind(&event.model_name)
                .push_bind(&event.message_id)
                .push_bind(event.usage_ratio)
                .push_bind(event.context_limit)
                .push_bind(event.input_tokens)
                .push_bind(event.output_tokens)
                .push_bind(event.cache_read_tokens)
                .push_bind(event.cache_creation_tokens)
                .push_bind(event.reasoning_tokens)
                .push_bind(event.total_input_tokens)
                .push_bind(event.total_output_tokens)
                .push_bind(event.total_cache_read_tokens)
                .push_bind(event.total_cache_creation_tokens)
                .push_bind(event.total_reasoning_tokens);
        });
        builder.build().execute(&mut **tx).await?;
    }
    Ok(())
}

async fn insert_parse_errors(
    tx: &mut Transaction<'_, Sqlite>,
    session: &AetherSession,
) -> Result<(), SessionIndexError> {
    for error in &session.parse_errors {
        let source_path = path_to_string(&error.source_path);
        sqlx::query!(
            r#"
            insert into parse_errors (source_path, session_id, line_number, error, line_excerpt)
            values (?1, ?2, ?3, ?4, ?5)
            "#,
            source_path,
            error.session_id,
            error.line_number,
            error.error,
            error.line_excerpt,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn clear_source_path(tx: &mut Transaction<'_, Sqlite>, source_path: &str) -> Result<(), SessionIndexError> {
    sqlx::query!("delete from session_files where source_path = ?1", source_path).execute(&mut **tx).await?;
    sqlx::query!("delete from parse_errors where source_path = ?1", source_path).execute(&mut **tx).await?;
    Ok(())
}

fn sqlite_value_to_json(
    row: &SqliteRow,
    index: usize,
    max_cell_chars: usize,
    truncated_cells: &mut bool,
) -> Result<Value, SessionIndexError> {
    let value = row.try_get_raw(index)?;
    if value.is_null() {
        return Ok(Value::Null);
    }
    match value.type_info().name() {
        "INTEGER" | "INT" | "BIGINT" => Ok(Value::from(row.try_get::<i64, _>(index)?)),
        "REAL" | "FLOAT" | "DOUBLE" => Ok(Value::from(row.try_get::<f64, _>(index)?)),
        "BLOB" => {
            let value = row.try_get::<Vec<u8>, _>(index)?;
            Ok(Value::String(format!("<blob {} bytes>", value.len())))
        }
        _ => {
            let text = row.try_get::<String, _>(index)?;
            if text.chars().count() <= max_cell_chars {
                return Ok(Value::String(text));
            }
            *truncated_cells = true;
            Ok(Value::String(text.chars().take(max_cell_chars).collect()))
        }
    }
}

fn is_schema_drift(error: &MigrateError) -> bool {
    matches!(
        error,
        MigrateError::VersionMissing(_)
            | MigrateError::VersionMismatch(_)
            | MigrateError::VersionNotPresent(_)
            | MigrateError::VersionTooOld(_, _)
    )
}

async fn rebuild_database(path: &Path, options: &SqliteConnectOptions) -> Result<(), SessionIndexError> {
    let rebuild_path = path.with_extension(format!("rebuild-{}", std::process::id()));
    remove_database_files(&rebuild_path)?;
    let rebuild_options = options.clone().filename(&rebuild_path);
    let mut rebuild = SqliteConnection::connect_with(&rebuild_options).await?;
    MIGRATOR.run(&mut rebuild).await?;
    rebuild.close().await?;

    remove_database_files(path)?;
    std::fs::rename(rebuild_path, path)?;
    Ok(())
}

fn remove_database_files(path: &Path) -> std::io::Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn schema_checksum_drift_rebuilds_after_fresh_database_succeeds() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("index.sqlite");
        let options = SqliteConnectOptions::new().filename(&path).create_if_missing(true);
        let mut conn = SqliteConnection::connect_with(&options).await.unwrap();
        conn.execute(
            r"
            create table _sqlx_migrations (
              version bigint primary key,
              description text not null,
              installed_on timestamp not null default current_timestamp,
              success boolean not null,
              checksum blob not null,
              execution_time bigint not null
            );
            insert into _sqlx_migrations
              (version, description, success, checksum, execution_time)
              values (1, 'session index', true, x'00', 0);
            create table preserved_until_rebuild_succeeds (value text);
            ",
        )
        .await
        .unwrap();
        conn.close().await.unwrap();

        let mut db = Db::open_writable(&path).await.unwrap();
        let sql = format!("select count(*) from {}", "sessions");
        let rows = db.query(&sql, QueryLimits { max_rows: 1, max_cell_chars: 100 }).await.unwrap();

        assert_eq!(rows.rows, vec![vec![Value::from(0)]]);
    }
}
