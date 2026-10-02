//! Utilities for working with LSP diagnostics
//!
//! This module provides helpers for formatting and counting diagnostics.

use super::common::uri_to_path;
use super::render::single_line;
use lsp_types::{Diagnostic, DiagnosticSeverity, Uri};
use schemars::JsonSchema;
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone)]
pub struct FormattedDiagnostic {
    /// The file path (extracted from URI)
    pub file: String,
    /// The line number (1-indexed for display)
    pub line: u32,
    /// The column number (1-indexed for display)
    pub column: u32,
    /// The severity level
    pub severity: Severity,
    /// The diagnostic message
    pub message: String,
    /// The diagnostic code
    pub code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

impl From<Option<DiagnosticSeverity>> for Severity {
    fn from(severity: Option<DiagnosticSeverity>) -> Self {
        match severity {
            Some(DiagnosticSeverity::ERROR) | None => Severity::Error,
            Some(DiagnosticSeverity::WARNING) => Severity::Warning,
            Some(DiagnosticSeverity::HINT) => Severity::Hint,
            _ => Severity::Info, // INFORMATION or unknown severity
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Error => write!(f, "error"),
            Severity::Warning => write!(f, "warning"),
            Severity::Info => write!(f, "info"),
            Severity::Hint => write!(f, "hint"),
        }
    }
}

impl FormattedDiagnostic {
    /// Create a formatted diagnostic from a raw diagnostic and URI
    pub fn from_diagnostic(uri: &Uri, diagnostic: &Diagnostic) -> Self {
        let code = diagnostic.code.as_ref().map(|c| match c {
            lsp_types::NumberOrString::Number(n) => n.to_string(),
            lsp_types::NumberOrString::String(s) => s.clone(),
        });

        Self {
            file: uri_to_path(uri),
            line: diagnostic.range.start.line + 1,
            column: diagnostic.range.start.character + 1,
            severity: diagnostic.severity.into(),
            message: diagnostic.message.clone(),
            code,
        }
    }
}

impl fmt::Display for FormattedDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{} {}", self.line, self.column, self.severity)?;
        if let Some(code) = &self.code {
            write!(f, "[{code}]")?;
        }
        write!(f, ": {}", single_line(&self.message))
    }
}

/// Count diagnostics by severity
pub fn count_by_severity(diagnostics: &[FormattedDiagnostic]) -> DiagnosticCounts {
    let mut counts = DiagnosticCounts::default();
    for d in diagnostics {
        match d.severity {
            Severity::Error => counts.errors += 1,
            Severity::Warning => counts.warnings += 1,
            Severity::Info => counts.infos += 1,
            Severity::Hint => counts.hints += 1,
        }
    }
    counts.total = counts.errors + counts.warnings + counts.infos + counts.hints;
    counts
}

/// Counts of diagnostics by severity
#[derive(Debug, Default, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCounts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub hints: usize,
    pub total: usize,
}

impl DiagnosticCounts {
    /// Returns true if there are any errors
    pub fn has_errors(&self) -> bool {
        self.errors > 0
    }
}

impl fmt::Display for DiagnosticCounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} errors, {} warnings, {} info, {} hints", self.errors, self.warnings, self.infos, self.hints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn make_diagnostic(severity: DiagnosticSeverity, message: &str, line: u32) -> Diagnostic {
        Diagnostic {
            range: Range { start: Position { line, character: 0 }, end: Position { line, character: 10 } },
            severity: Some(severity),
            code: None,
            code_description: None,
            source: Some("test".to_string()),
            message: message.to_string(),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    fn make_uri(path: &str) -> Uri {
        format!("file://{path}").parse().unwrap()
    }

    #[test]
    fn test_severity_conversion() {
        assert_eq!(Severity::from(Some(DiagnosticSeverity::ERROR)), Severity::Error);
        assert_eq!(Severity::from(Some(DiagnosticSeverity::WARNING)), Severity::Warning);
        assert_eq!(Severity::from(Some(DiagnosticSeverity::INFORMATION)), Severity::Info);
        assert_eq!(Severity::from(Some(DiagnosticSeverity::HINT)), Severity::Hint);
        assert_eq!(Severity::from(None), Severity::Error);
    }

    #[test]
    fn test_formatted_diagnostic() {
        let uri = make_uri("/path/to/file.rs");
        let diagnostic = make_diagnostic(DiagnosticSeverity::ERROR, "test error", 5);

        let formatted = FormattedDiagnostic::from_diagnostic(&uri, &diagnostic);

        assert_eq!(formatted.file, "/path/to/file.rs");
        assert_eq!(formatted.line, 6); // 1-indexed
        assert_eq!(formatted.column, 1);
        assert_eq!(formatted.severity, Severity::Error);
        assert_eq!(formatted.message, "test error");
        assert_eq!(formatted.to_string(), "6:1 error: test error");
    }

    #[test]
    fn severities_are_ordered_from_most_severe() {
        assert!(Severity::Error < Severity::Warning);
        assert!(Severity::Warning < Severity::Info);
        assert!(Severity::Info < Severity::Hint);
    }

    #[test]
    fn test_count_by_severity() {
        let uri = make_uri("/path/to/file.rs");
        let diagnostics = [
            make_diagnostic(DiagnosticSeverity::ERROR, "error 1", 0),
            make_diagnostic(DiagnosticSeverity::ERROR, "error 2", 1),
            make_diagnostic(DiagnosticSeverity::WARNING, "warning 1", 2),
        ];

        let formatted: Vec<_> = diagnostics.iter().map(|d| FormattedDiagnostic::from_diagnostic(&uri, d)).collect();

        let counts = count_by_severity(&formatted);

        assert_eq!(counts.errors, 2);
        assert_eq!(counts.warnings, 1);
        assert_eq!(counts.infos, 0);
        assert_eq!(counts.hints, 0);
        assert_eq!(counts.total, 3);
        assert!(counts.has_errors());
    }

    #[test]
    fn test_uri_to_path() {
        // Unix-style path
        assert_eq!(uri_to_path(&make_uri("/path/to/file.rs")), "/path/to/file.rs");

        // Non-file URI
        let non_file_uri: Uri = "https://example.com/file.rs".parse().unwrap();
        assert_eq!(uri_to_path(&non_file_uri), "https://example.com/file.rs");
    }
}
