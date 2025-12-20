use super::{DiagnosticResult, LspSession};
use lsp_types::{DiagnosticSeverity, PublishDiagnosticsParams};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::time::timeout;

/// Convert an LSP Uri to a file path string
fn uri_to_file_path(uri: &lsp_types::Uri) -> String {
    // Try to parse as url::Url and convert to file path
    if let Ok(url) = url::Url::parse(uri.as_str()) {
        if let Ok(path) = url.to_file_path() {
            return path.to_string_lossy().to_string();
        }
    }
    // Fallback: use the URI string directly
    uri.as_str().to_string()
}

pub struct DiagnosticCollector {
    session: LspSession,
    diagnostics: HashMap<String, Vec<DiagnosticResult>>,
    opened_files: usize,
}

impl DiagnosticCollector {
    pub fn new(session: LspSession) -> Self {
        Self {
            session,
            diagnostics: HashMap::new(),
            opened_files: 0,
        }
    }

    /// Open Rust files to trigger analysis
    pub async fn open_files(&mut self, workspace: &PathBuf, max_files: usize) -> Result<(), String> {
        let files = find_rust_files(workspace, max_files);
        for file in files {
            self.session.open_file(file).await?;
            self.opened_files += 1;
        }
        Ok(())
    }

    pub async fn collect_workspace_diagnostics(
        mut self,
        severity_filter: Option<&str>,
        timeout_duration: Duration,
    ) -> Result<Vec<DiagnosticResult>, String> {
        // Give rust-analyzer time to analyze the workspace and send diagnostics
        let collection_timeout = timeout(timeout_duration, async {
            let mut stable_iterations = 0;
            let required_stable_iterations = 3; // Wait for 3 iterations without new data

            loop {
                // Use timeout on notification receive to avoid blocking forever
                let notif_result = tokio::time::timeout(
                    Duration::from_millis(500),
                    self.session.get_next_notification(),
                )
                .await;

                match notif_result {
                    Ok(Some(notification)) => {
                        stable_iterations = 0; // Reset stability counter on any notification
                        if let Some(method) = notification.get("method").and_then(|m| m.as_str()) {
                            if method == "textDocument/publishDiagnostics" {
                                if let Some(params) = notification.get("params") {
                                    if let Ok(diagnostic_params) =
                                        serde_json::from_value::<PublishDiagnosticsParams>(
                                            params.clone(),
                                        )
                                    {
                                        self.process_diagnostics(diagnostic_params, severity_filter);
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Channel closed
                        break;
                    }
                    Err(_) => {
                        // Timeout - no notification received
                        stable_iterations += 1;

                        // Exit if we've had stable_iterations without new data
                        if self.opened_files > 0 && stable_iterations >= required_stable_iterations {
                            break;
                        }
                    }
                }
            }
        });

        if collection_timeout.await.is_err() {
            // Timeout occurred, return what we have
        }

        // Shutdown the LSP session
        let _ = self.session.shutdown().await;

        // Flatten all diagnostics into a single Vec
        let mut all_diagnostics = Vec::new();
        for file_diagnostics in self.diagnostics.values() {
            all_diagnostics.extend(file_diagnostics.iter().cloned());
        }

        Ok(all_diagnostics)
    }

    fn process_diagnostics(
        &mut self,
        params: PublishDiagnosticsParams,
        severity_filter: Option<&str>,
    ) {
        let file_path = uri_to_file_path(&params.uri);

        let mut file_diagnostics = Vec::new();

        for diagnostic in params.diagnostics {
            let severity_str = match diagnostic.severity {
                Some(DiagnosticSeverity::ERROR) => "error",
                Some(DiagnosticSeverity::WARNING) => "warning",
                Some(DiagnosticSeverity::INFORMATION) => "info",
                Some(DiagnosticSeverity::HINT) => "hint",
                None => "unknown",
                Some(_) => "unknown",
            };

            // Apply severity filter if specified
            if let Some(filter) = severity_filter {
                if severity_str != filter {
                    continue;
                }
            }

            let diagnostic_result = DiagnosticResult {
                file: file_path.clone(),
                line: diagnostic.range.start.line,
                column: diagnostic.range.start.character,
                severity: severity_str.to_string(),
                message: diagnostic.message,
                code: diagnostic.code.map(|c| match c {
                    lsp_types::NumberOrString::Number(n) => n.to_string(),
                    lsp_types::NumberOrString::String(s) => s,
                }),
            };

            file_diagnostics.push(diagnostic_result);
        }

        if file_diagnostics.is_empty() {
            // Remove the file entry if no diagnostics remain
            self.diagnostics.remove(&file_path);
        } else {
            // Update diagnostics for this file
            self.diagnostics.insert(file_path, file_diagnostics);
        }
    }
}

pub async fn collect_diagnostics(
    workspace_root: Option<String>,
    severity_filter: Option<String>,
) -> Result<Vec<DiagnosticResult>, String> {
    let workspace_path = workspace_root
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let session = LspSession::new(workspace_path.clone()).await?;
    let mut collector = DiagnosticCollector::new(session);

    // Open files to trigger analysis (limit to 50 files to avoid overwhelming the LSP)
    collector.open_files(&workspace_path, 50).await?;

    // Wait up to 30 seconds for diagnostics to be collected
    collector
        .collect_workspace_diagnostics(severity_filter.as_deref(), Duration::from_secs(30))
        .await
}

/// Find Rust files in a directory, skipping target and hidden directories
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