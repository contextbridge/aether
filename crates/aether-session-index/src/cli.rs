use crate::paths::{default_db_path, default_sessions_dir};
use aether_sessions::analytics::{
    IngestOptions, QueryOptions, SessionIndexError, default_parse_concurrency, ingest_sessions, render_schema_text,
    render_tsv, run_query, schema_doc,
};
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aether-session-index")]
#[command(about = "Index and query Aether session logs")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    Ingest(IngestArgs),
    Query(QueryArgs),
    Schema(SchemaArgs),
}

#[derive(Args)]
pub struct IngestArgs {
    #[arg(long)]
    pub sessions_dir: Option<PathBuf>,
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long)]
    pub no_prune: bool,
    #[arg(long)]
    pub parse_concurrency: Option<usize>,
}

#[derive(Args)]
pub struct QueryArgs {
    #[arg(long)]
    pub db: Option<PathBuf>,
    #[arg(long, value_enum, default_value = "json")]
    pub format: QueryFormat,
    #[arg(long, default_value_t = 100)]
    pub max_rows: usize,
    #[arg(long, default_value_t = 1000)]
    pub max_cell_chars: usize,
    #[arg(long, default_value_t = 2000)]
    pub timeout_ms: u64,
    #[arg(required = true, trailing_var_arg = true)]
    pub sql: Vec<String>,
}

#[derive(Args)]
pub struct SchemaArgs {
    #[arg(long, value_enum, default_value = "text")]
    pub format: SchemaFormat,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum QueryFormat {
    Json,
    Tsv,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SchemaFormat {
    Text,
    Json,
}

pub async fn run_cli(cli: Cli) -> Result<(), SessionIndexError> {
    match cli.command {
        Command::Ingest(args) => ingest(args).await?,
        Command::Query(args) => query(args).await?,
        Command::Schema(args) => print_schema(args.format)?,
    }
    Ok(())
}

async fn ingest(args: IngestArgs) -> Result<(), SessionIndexError> {
    let summary = ingest_sessions(IngestOptions {
        sessions_dir: args.sessions_dir.map_or_else(default_sessions_dir, Ok)?,
        db_path: args.db.map_or_else(default_db_path, Ok)?,
        prune: !args.no_prune,
        parse_concurrency: args.parse_concurrency.unwrap_or_else(default_parse_concurrency),
    })
    .await?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

async fn query(args: QueryArgs) -> Result<(), SessionIndexError> {
    let output = run_query(&QueryOptions {
        db_path: args.db.map_or_else(default_db_path, Ok)?,
        sql: args.sql.join(" "),
        max_rows: args.max_rows,
        max_cell_chars: args.max_cell_chars,
        timeout_ms: args.timeout_ms,
    })
    .await?;

    match args.format {
        QueryFormat::Json => println!("{}", serde_json::to_string(&output)?),
        QueryFormat::Tsv => println!("{}", render_tsv(&output)),
    }
    Ok(())
}

fn print_schema(format: SchemaFormat) -> Result<(), SessionIndexError> {
    let schema = schema_doc();
    match format {
        SchemaFormat::Text => println!("{}", render_schema_text(&schema)),
        SchemaFormat::Json => println!("{}", serde_json::to_string(&schema)?),
    }
    Ok(())
}
