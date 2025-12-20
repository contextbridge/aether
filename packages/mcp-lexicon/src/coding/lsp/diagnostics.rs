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
}

impl DiagnosticCollector {
    pub fn new(session: LspSession) -> Self {
        Self {
            session,
            diagnostics: HashMap::new(),
        }
    }

    pub async fn collect_workspace_diagnostics(
        mut self,
        severity_filter: Option<&str>,
        timeout_duration: Duration,
    ) -> Result<Vec<DiagnosticResult>, String> {
        // Give rust-analyzer time to analyze the workspace and send diagnostics
        let collection_timeout = timeout(timeout_duration, async {
            let mut first_diagnostic_received = false;
            let mut stable_count = 0;
            let target_stable_iterations = 5; // Wait for 5 iterations without new diagnostics

            loop {
                match self.session.get_next_notification().await {
                    Some(notification) => {
                        if let Some(method) = notification.get("method").and_then(|m| m.as_str()) {
                            if method == "textDocument/publishDiagnostics" {
                                if let Some(params) = notification.get("params") {
                                    if let Ok(diagnostic_params) = serde_json::from_value::<PublishDiagnosticsParams>(params.clone()) {
                                        self.process_diagnostics(diagnostic_params, severity_filter);
                                        first_diagnostic_received = true;
                                        stable_count = 0; // Reset stability counter
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        if first_diagnostic_received {
                            stable_count += 1;
                            if stable_count >= target_stable_iterations {
                                break; // No more notifications, we're done
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        });

        if let Err(_) = collection_timeout.await {
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

    let session = LspSession::new(workspace_path).await?;
    let collector = DiagnosticCollector::new(session);

    // Wait up to 10 seconds for diagnostics to be collected
    collector
        .collect_workspace_diagnostics(
            severity_filter.as_deref(),
            Duration::from_secs(10),
        )
        .await
}