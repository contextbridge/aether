pub mod client;
pub mod diagnostics;
pub mod error;

pub use client::{LspClient, LspSession};
pub use diagnostics::{collect_diagnostics, DiagnosticCollector};
pub use error::LspError;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsArgs {
    /// Workspace root directory (defaults to current directory)
    pub workspace_root: Option<String>,
    /// Filter by severity: "error", "warning", "info", "hint" (optional)
    pub severity_filter: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticResult {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsResponse {
    pub status: String,
    pub diagnostics: Vec<DiagnosticResult>,
    pub total_count: usize,
}