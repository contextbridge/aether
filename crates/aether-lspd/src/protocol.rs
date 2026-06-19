use crate::language_catalog::LanguageId;
use lsp_types::Uri;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::path::PathBuf;

#[doc = include_str!("docs/protocol.md")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonRequest {
    Initialize(InitializeRequest),
    LspCall {
        client_id: i64,
        method: String,
        params: Value,
    },
    GetDiagnostics {
        client_id: i64,
        /// If None, return all cached diagnostics for the workspace
        uri: Option<Uri>,
    },
    QueueDiagnosticRefresh {
        client_id: i64,
        uri: Uri,
    },
    Disconnect,
    Ping,
}

/// Initialize request to set up LSP for a workspace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitializeRequest {
    pub workspace_root: PathBuf,
    pub language: LanguageId,
}

/// LSP notification from client to server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspNotification {
    pub method: String,
    pub params: Value,
}

/// Top-level daemon response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonResponse {
    Initialized,
    Pong,
    LspResult { client_id: i64, result: Result<Value, LspErrorResponse> },
    Error(ProtocolError),
}

/// LSP error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspErrorResponse {
    pub code: i32,
    pub message: String,
}

/// Protocol-level error (not LSP error)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolError {
    pub message: String,
    /// Optional `client_id` for correlating errors back to LSP requests
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<i64>,
}

impl ProtocolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), client_id: None }
    }

    pub fn with_client_id(message: impl Into<String>, client_id: i64) -> Self {
        Self { message: message.into(), client_id: Some(client_id) }
    }
}

/// Extract the document URI from an LSP request's params by method name.
///
/// Used by the daemon for auto-open: if the request targets a specific file,
/// the daemon ensures the file is opened before forwarding the request.
pub fn extract_document_uri(method: &str, params: &Value) -> Option<Uri> {
    if !method.starts_with("textDocument/") {
        return None;
    }
    params.pointer("/textDocument/uri").and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
}

/// Maximum message size (16 MB)
pub const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

/// Read a length-prefixed frame from an async reader
pub(crate) async fn read_frame<R, T>(reader: &mut R) -> io::Result<Option<T>>
where
    R: tokio::io::AsyncReadExt + Unpin,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(len_buf);

    if len > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Message too large: {len} bytes")));
    }

    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;

    serde_json::from_slice(&buf).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Write a length-prefixed frame to an async writer
pub(crate) async fn write_frame<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
    T: Serialize,
{
    let json = serde_json::to_vec(message)?;

    if json.len() > MAX_MESSAGE_SIZE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("Message too large: {} bytes", json.len())));
    }

    let len = u32::try_from(json.len()).unwrap_or(u32::MAX);
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&json).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_error_new() {
        let err = ProtocolError::new("test error");
        assert_eq!(err.message, "test error");
    }

    #[test]
    fn test_daemon_request_lsp_call_roundtrip() {
        let req = DaemonRequest::LspCall {
            client_id: 42,
            method: "textDocument/definition".to_string(),
            params: serde_json::json!({
                "textDocument": { "uri": "file:///test.rs" },
                "position": { "line": 0, "character": 0 }
            }),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: DaemonRequest = serde_json::from_str(&json).unwrap();
        match decoded {
            DaemonRequest::LspCall { client_id, method, .. } => {
                assert_eq!(client_id, 42);
                assert_eq!(method, "textDocument/definition");
            }
            _ => panic!("Wrong variant"),
        }
    }

    #[test]
    fn test_extract_document_uri_definition() {
        let params = serde_json::json!({
            "textDocument": { "uri": "file:///src/main.rs" },
            "position": { "line": 10, "character": 5 }
        });
        let uri = extract_document_uri("textDocument/definition", &params);
        assert!(uri.is_some());
        assert_eq!(uri.unwrap().as_str(), "file:///src/main.rs");
    }

    #[test]
    fn test_extract_document_uri_references() {
        let params = serde_json::json!({
            "textDocument": { "uri": "file:///src/lib.rs" },
            "position": { "line": 5, "character": 3 },
            "context": { "includeDeclaration": true }
        });
        let uri = extract_document_uri("textDocument/references", &params);
        assert!(uri.is_some());
        assert_eq!(uri.unwrap().as_str(), "file:///src/lib.rs");
    }

    #[test]
    fn test_extract_document_uri_document_symbol() {
        let params = serde_json::json!({
            "textDocument": { "uri": "file:///src/foo.rs" }
        });
        let uri = extract_document_uri("textDocument/documentSymbol", &params);
        assert!(uri.is_some());
        assert_eq!(uri.unwrap().as_str(), "file:///src/foo.rs");
    }

    #[test]
    fn test_extract_document_uri_workspace_symbol_returns_none() {
        let params = serde_json::json!({ "query": "Foo" });
        let uri = extract_document_uri("workspace/symbol", &params);
        assert!(uri.is_none());
    }

    #[test]
    fn test_extract_document_uri_unknown_method_returns_none() {
        let params = serde_json::json!({});
        let uri = extract_document_uri("textDocument/unknown", &params);
        assert!(uri.is_none());
    }
}
