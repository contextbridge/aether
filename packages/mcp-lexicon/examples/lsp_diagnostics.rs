use lsp_types::PublishDiagnosticsParams;
use mcp_lexicon::coding::lsp::LspSession;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let workspace = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap());

    println!("Collecting diagnostics from: {:?}", workspace);
    println!("Creating LSP session...");

    let session = match LspSession::new(workspace.clone()).await {
        Ok(s) => {
            println!("LSP session created successfully!");
            s
        }
        Err(e) => {
            eprintln!("Failed to create LSP session: {}", e);
            return;
        }
    };

    // Find some Rust files to open
    let rust_files = find_rust_files(&workspace, 5);
    if rust_files.is_empty() {
        println!("No Rust files found in workspace");
    } else {
        println!("Opening {} Rust files for analysis...", rust_files.len());
        for file in &rust_files {
            println!("  Opening: {:?}", file);
            if let Err(e) = session.open_file(file.clone()).await {
                eprintln!("    Failed to open: {}", e);
            }
        }
    }

    println!("\nWaiting for diagnostics (30 second timeout)...\n");

    let start = std::time::Instant::now();
    let timeout_duration = Duration::from_secs(30);
    let mut diagnostics_received = 0;

    loop {
        if start.elapsed() > timeout_duration {
            println!("\nTimeout reached.");
            break;
        }

        // Use tokio::time::timeout to avoid blocking forever
        let notification = tokio::time::timeout(
            Duration::from_millis(500),
            session.get_next_notification(),
        )
        .await;

        match notification {
            Ok(Some(notif)) => {
                if let Some(method) = notif.get("method").and_then(|m| m.as_str()) {
                    print!("\nGot: {}", method);
                    if method == "textDocument/publishDiagnostics" {
                        if let Some(params) = notif.get("params") {
                            if let Ok(diag_params) =
                                serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
                            {
                                diagnostics_received += 1;
                                println!(" ({})", diag_params.uri.as_str());
                                println!("  Diagnostics: {}", diag_params.diagnostics.len());
                                for d in &diag_params.diagnostics {
                                    println!("    - {:?}: {}", d.severity, d.message);
                                }
                            }
                        }
                    } else {
                        println!();
                    }
                } else {
                    println!("\nGot notification without method");
                }
            }
            Ok(None) => {
                println!("Channel closed");
                break;
            }
            Err(_) => {
                // Timeout - no notification received, continue waiting
                print!(".");
                std::io::Write::flush(&mut std::io::stdout()).ok();
            }
        }

        // Exit early if we've received diagnostics for all opened files
        if diagnostics_received >= rust_files.len() && diagnostics_received > 0 {
            println!("\nReceived diagnostics for all opened files.");
            break;
        }
    }

    println!("\nTotal diagnostic notifications received: {}", diagnostics_received);
    println!("Shutting down...");
    let _ = session.shutdown().await;
    println!("Done.");
}

fn find_rust_files(dir: &PathBuf, max: usize) -> Vec<PathBuf> {
    let mut files = Vec::new();
    find_rust_files_recursive(dir, &mut files, max);
    files
}

fn find_rust_files_recursive(dir: &PathBuf, files: &mut Vec<PathBuf>, max: usize) {
    if files.len() >= max {
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        if files.len() >= max {
            return;
        }

        let path = entry.path();

        // Skip target directory and hidden directories
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name == "target" || name.starts_with('.') {
                continue;
            }
        }

        if path.is_dir() {
            find_rust_files_recursive(&path, files, max);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
}
