use mcp_lexicon::coding::lsp::{
    collect_diagnostics, DiagnosticResult, LspDiagnosticsArgs, LspDiagnosticsResponse, LspError,
};
use tempfile::TempDir;

#[test]
fn test_lsp_error_display_spawn_failed() {
    let error = LspError::SpawnFailed {
        command: "rust-analyzer".to_string(),
        error: "not found".to_string(),
    };
    assert_eq!(
        error.to_string(),
        "Failed to spawn LSP process 'rust-analyzer': not found"
    );
}

#[test]
fn test_lsp_error_display_initialization_failed() {
    let error = LspError::InitializationFailed("handshake failed".to_string());
    assert_eq!(
        error.to_string(),
        "LSP initialization failed: handshake failed"
    );
}

#[test]
fn test_lsp_error_display_request_failed() {
    let error = LspError::RequestFailed {
        method: "textDocument/hover".to_string(),
        error: "timeout".to_string(),
    };
    assert_eq!(
        error.to_string(),
        "LSP request 'textDocument/hover' failed: timeout"
    );
}

#[test]
fn test_lsp_error_display_invalid_path() {
    let error = LspError::InvalidPath("/invalid/path".to_string());
    assert_eq!(error.to_string(), "Invalid path: /invalid/path");
}

#[test]
fn test_lsp_error_display_channel_closed() {
    let error = LspError::ChannelClosed;
    assert_eq!(error.to_string(), "LSP communication channel closed");
}

#[test]
fn test_lsp_error_display_timeout() {
    let error = LspError::Timeout;
    assert_eq!(error.to_string(), "LSP operation timed out");
}

#[test]
fn test_diagnostic_result_serialization() {
    let diagnostic = DiagnosticResult {
        file: "/path/to/file.rs".to_string(),
        line: 10,
        column: 5,
        severity: "error".to_string(),
        message: "expected type `bool`, found `i32`".to_string(),
        code: Some("E0308".to_string()),
    };

    let json = serde_json::to_string(&diagnostic).unwrap();
    let deserialized: DiagnosticResult = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.file, "/path/to/file.rs");
    assert_eq!(deserialized.line, 10);
    assert_eq!(deserialized.column, 5);
    assert_eq!(deserialized.severity, "error");
    assert_eq!(deserialized.message, "expected type `bool`, found `i32`");
    assert_eq!(deserialized.code, Some("E0308".to_string()));
}

#[test]
fn test_lsp_diagnostics_args_serialization() {
    let args = LspDiagnosticsArgs {
        workspace_root: Some("/workspace/path".to_string()),
        severity_filter: Some("error".to_string()),
    };

    let json = serde_json::to_string(&args).unwrap();
    let deserialized: LspDiagnosticsArgs = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.workspace_root, Some("/workspace/path".to_string()));
    assert_eq!(deserialized.severity_filter, Some("error".to_string()));
}

#[test]
fn test_lsp_diagnostics_response_serialization() {
    let response = LspDiagnosticsResponse {
        status: "success".to_string(),
        diagnostics: vec![
            DiagnosticResult {
                file: "/path/to/file.rs".to_string(),
                line: 1,
                column: 0,
                severity: "warning".to_string(),
                message: "unused variable".to_string(),
                code: None,
            },
        ],
        total_count: 1,
    };

    let json = serde_json::to_string(&response).unwrap();
    let deserialized: LspDiagnosticsResponse = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.status, "success");
    assert_eq!(deserialized.diagnostics.len(), 1);
    assert_eq!(deserialized.total_count, 1);
}

// Integration tests - these require rust-analyzer to be installed
// and may take time to complete due to workspace analysis

#[tokio::test]
#[ignore = "requires rust-analyzer to be installed"]
async fn test_collect_diagnostics_on_valid_project() {
    // Create a temp directory with a Rust project that has compile errors
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create Cargo.toml
    std::fs::write(
        temp_path.join("Cargo.toml"),
        r#"[package]
name = "test_project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create src directory and main.rs with a compile error
    std::fs::create_dir_all(temp_path.join("src")).unwrap();
    std::fs::write(
        temp_path.join("src/main.rs"),
        r#"fn main() {
    let x: i32 = "not a number";  // Type mismatch error
}
"#,
    )
    .unwrap();

    // Collect diagnostics
    let result = collect_diagnostics(
        Some(temp_path.to_string_lossy().to_string()),
        Some("error".to_string()),
    )
    .await;

    // rust-analyzer might not be installed, so we accept errors
    match result {
        Ok(diagnostics) => {
            // Should have at least one error diagnostic
            assert!(!diagnostics.is_empty(), "Expected at least one diagnostic");

            // Check that diagnostics contain an error about type mismatch
            let has_type_error = diagnostics.iter().any(|d| {
                d.severity == "error"
                    && (d.message.contains("expected") || d.message.contains("mismatched types"))
            });
            assert!(has_type_error, "Expected a type mismatch error");
        }
        Err(e) => {
            // If rust-analyzer is not installed, that's okay for this test
            if e.contains("rust-analyzer") {
                eprintln!("Skipping: rust-analyzer not available: {}", e);
            } else {
                panic!("Unexpected error: {}", e);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires rust-analyzer to be installed"]
async fn test_collect_diagnostics_empty_on_clean_project() {
    // Create a temp directory with a valid Rust project (no errors)
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create Cargo.toml
    std::fs::write(
        temp_path.join("Cargo.toml"),
        r#"[package]
name = "clean_project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create src directory and main.rs without errors
    std::fs::create_dir_all(temp_path.join("src")).unwrap();
    std::fs::write(
        temp_path.join("src/main.rs"),
        r#"fn main() {
    println!("Hello, world!");
}
"#,
    )
    .unwrap();

    // Collect diagnostics - filter for errors only
    let result = collect_diagnostics(
        Some(temp_path.to_string_lossy().to_string()),
        Some("error".to_string()),
    )
    .await;

    match result {
        Ok(diagnostics) => {
            // Clean project should have no error diagnostics
            let errors: Vec<_> = diagnostics
                .iter()
                .filter(|d| d.severity == "error")
                .collect();
            assert!(
                errors.is_empty(),
                "Expected no error diagnostics for clean project, got {:?}",
                errors
            );
        }
        Err(e) => {
            if e.contains("rust-analyzer") {
                eprintln!("Skipping: rust-analyzer not available: {}", e);
            } else {
                panic!("Unexpected error: {}", e);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires rust-analyzer to be installed"]
async fn test_collect_diagnostics_severity_filter() {
    // Create a temp directory with a Rust project that has warnings
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create Cargo.toml
    std::fs::write(
        temp_path.join("Cargo.toml"),
        r#"[package]
name = "warning_project"
version = "0.1.0"
edition = "2021"
"#,
    )
    .unwrap();

    // Create src directory and main.rs with unused variable (warning)
    std::fs::create_dir_all(temp_path.join("src")).unwrap();
    std::fs::write(
        temp_path.join("src/main.rs"),
        r#"fn main() {
    let unused_var = 42;
}
"#,
    )
    .unwrap();

    // Collect only warning diagnostics
    let result = collect_diagnostics(
        Some(temp_path.to_string_lossy().to_string()),
        Some("warning".to_string()),
    )
    .await;

    match result {
        Ok(diagnostics) => {
            // All returned diagnostics should be warnings
            for d in &diagnostics {
                assert_eq!(d.severity, "warning", "Expected only warnings, got {:?}", d);
            }
        }
        Err(e) => {
            if e.contains("rust-analyzer") {
                eprintln!("Skipping: rust-analyzer not available: {}", e);
            } else {
                panic!("Unexpected error: {}", e);
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires rust-analyzer to be installed"]
async fn test_collect_diagnostics_invalid_workspace() {
    // Try to collect diagnostics from a non-existent directory
    // This test validates error handling when workspace is invalid
    let result = collect_diagnostics(
        Some("/nonexistent/path/that/does/not/exist".to_string()),
        None,
    )
    .await;

    // Should fail gracefully - either with an error or empty diagnostics
    // (depending on how rust-analyzer handles invalid paths)
    match result {
        Ok(diagnostics) => {
            // If it succeeds, it should return empty diagnostics
            assert!(diagnostics.is_empty(), "Expected no diagnostics for invalid workspace");
        }
        Err(e) => {
            // Expected - invalid workspace should error
            eprintln!("Got expected error: {}", e);
        }
    }
}
