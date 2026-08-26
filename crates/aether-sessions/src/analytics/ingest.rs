use super::SessionIndexError;
use super::db::Db;
use super::session::AetherSession;
use crate::{DiscoveredSessionFile, FileFingerprint, discover_session_files};
use futures::StreamExt;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct IngestOptions {
    pub sessions_dir: PathBuf,
    pub db_path: PathBuf,
    pub prune: bool,
    pub parse_concurrency: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct IngestSummary {
    pub sessions_dir: PathBuf,
    #[serde(rename = "db")]
    pub db_path: PathBuf,
    pub files_seen: usize,
    pub files_indexed: usize,
    pub files_skipped_unchanged: usize,
    pub files_failed: usize,
    pub events_indexed: usize,
    pub parse_errors: usize,
    pub stale_files_pruned: usize,
}

pub async fn ingest_sessions(options: IngestOptions) -> Result<IngestSummary, SessionIndexError> {
    let sessions_dir = options.sessions_dir.canonicalize()?;
    let db_path = options.db_path;
    let mut db = Db::open_writable(&db_path).await?;
    let files = discover_session_files(&sessions_dir)?;
    let mut summary = IngestSummary {
        sessions_dir: sessions_dir.clone(),
        db_path: db_path.clone(),
        files_seen: files.len(),
        ..IngestSummary::default()
    };

    if options.prune {
        summary.stale_files_pruned = db.prune_missing_files(&files).await?;
    }

    let indexed = db.indexed_file_fingerprints().await?;
    let changed_files: Vec<DiscoveredSessionFile> = files
        .into_iter()
        .filter(|file| {
            let source_path = file.path.to_string_lossy();
            if indexed.get(source_path.as_ref()) == Some(&file.fingerprint) {
                summary.files_skipped_unchanged += 1;
                false
            } else {
                true
            }
        })
        .collect();

    let tasks = changed_files.into_iter().map(|file| tokio::task::spawn_blocking(move || parse_changed_file(file)));
    let mut outcomes = futures::stream::iter(tasks).buffered(options.parse_concurrency.max(1));

    while let Some(result) = outcomes.next().await {
        match result? {
            FileIngestOutcome::Parsed(session) => match db.replace_session(&session).await {
                Ok(()) => {
                    summary.events_indexed += session.events.len();
                    summary.parse_errors += session.parse_errors.len();
                    summary.files_indexed += 1;
                }
                Err(error) => {
                    db.record_file_error(&session.source_path, session.fingerprint, error.to_string()).await?;
                    summary.files_failed += 1;
                }
            },
            FileIngestOutcome::Failed { file, fingerprint, error } => {
                db.record_file_error(&file, fingerprint, error).await?;
                summary.files_failed += 1;
            }
        }
    }

    Ok(summary)
}

pub fn default_parse_concurrency() -> usize {
    std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get)
}

#[derive(Debug)]
enum FileIngestOutcome {
    Parsed(AetherSession),
    Failed { file: PathBuf, fingerprint: FileFingerprint, error: String },
}

fn parse_changed_file(file: DiscoveredSessionFile) -> FileIngestOutcome {
    match AetherSession::parse(&file.path) {
        Ok(session) => FileIngestOutcome::Parsed(session),
        Err(error) => {
            FileIngestOutcome::Failed { file: file.path, fingerprint: file.fingerprint, error: error.to_string() }
        }
    }
}
