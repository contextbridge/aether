//! Example: Get diagnostics using `LspRegistry`
//!
//! This example demonstrates how to:
//! 1. Create an `LspRegistry` for multi-language LSP integration
//! 2. Query diagnostics through the `LspRegistry`
//!
//! The LSP daemon (`aether-lspd`) automatically handles `didOpen` when it
//! receives a request for a file that hasn't been opened yet.
//!
//! Usage:
//!   ```text
//!   cargo run -p mcp-servers --example lsp_diagnostics -- /path/to/rust/project
//!   ```
//!
//! Requirements:
//! - `rust-analyzer` must be installed and in PATH (for Rust projects)
//! - The target project must be a valid Rust project with `Cargo.toml`

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use mcp_servers::lsp::diagnostics::{FormattedDiagnostic, count_by_severity};
use mcp_servers::lsp::registry::LspRegistry;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project_path =
        env::args().nth(1).map_or_else(|| env::current_dir().expect("Failed to get current directory"), PathBuf::from);

    if !project_path.join("Cargo.toml").exists() {
        eprintln!("Error: {} is not a Rust project (no Cargo.toml found)", project_path.display());
        std::process::exit(1);
    }

    println!("Creating LspRegistry for: {}", project_path.display());

    let registry = LspRegistry::new(project_path.clone());
    registry.spawn_project_lsps().await;

    println!("LspRegistry created; LSP servers spawning for detected languages.");

    println!("\nWaiting for LSP to index the project...");
    sleep(Duration::from_secs(5)).await;

    println!("Fetching diagnostics...");
    let diagnostics_by_file = registry.collect_diagnostics(None).await;

    if diagnostics_by_file.values().all(Vec::is_empty) {
        println!("No diagnostics reported (project is clean!)");
    } else {
        for (file_path, diagnostics) in &diagnostics_by_file {
            if diagnostics.is_empty() {
                continue;
            }
            let uri: lsp_types::Uri = format!("file://{file_path}").parse().unwrap();
            let formatted: Vec<FormattedDiagnostic> =
                diagnostics.iter().map(|d| FormattedDiagnostic::from_diagnostic(&uri, d)).collect();

            let counts = count_by_severity(&formatted);
            println!("\n{file_path}: {counts}");

            for diag in &formatted {
                println!("  {}", diag.format());
            }
        }
    }

    println!("\nDone!");

    Ok(())
}
