mod db;
mod error;
mod ingest;
mod query;
mod row;
mod schema_doc;
mod session;

pub use error::SessionIndexError;
pub use ingest::{IngestOptions, IngestSummary, default_parse_concurrency, ingest_sessions};
pub use query::{OutputFormat, QueryOptions, QueryOutput, render_tsv, run_query};
pub use schema_doc::{Example, SchemaDoc, render_schema_text, schema_doc};

fn clamp_i64<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}
