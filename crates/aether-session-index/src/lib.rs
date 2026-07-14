pub mod cli;
mod db;
mod error;
mod ingest;
mod paths;
mod query;
mod row;
mod schema_doc;
mod session;

pub use error::SessionIndexError;
pub use ingest::{IngestOptions, IngestSummary, ingest_sessions};
pub use query::{OutputFormat, QueryOptions, QueryOutput, run_query};

/// Saturating conversion of a count/size into the `i64` that `SQLite` stores natively.
pub(crate) fn clamp_i64<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}
