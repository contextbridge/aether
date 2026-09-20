use crate::language_catalog::LanguageId;
use lsp_types::Uri;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite, LengthDelimitedCodec};

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

/// Error code for a request that exceeded the daemon's per-request timeout.
/// The daemon kills and replaces the language server when this fires.
///
/// Daemon-synthesized codes live outside the ranges reserved by JSON-RPC
/// (-32000..=-32768) and LSP (-32800..=-32899), so they can never collide with
/// a code forwarded verbatim from a real language server.
pub const LSP_REQUEST_TIMED_OUT: i32 = -33001;

/// Error code for a request that failed because the language server process
/// exited or its transport shut down before responding.
pub const LSP_TRANSPORT_CLOSED: i32 = -33002;

fn invalid_data(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

pub(crate) struct JsonFrames<T>(LengthDelimitedCodec, PhantomData<fn() -> T>);

impl<T> JsonFrames<T> {
    pub(crate) fn new() -> Self {
        Self(
            LengthDelimitedCodec::builder()
                .big_endian()
                .length_field_type::<u32>()
                .max_frame_length(MAX_MESSAGE_SIZE as usize)
                .new_codec(),
            PhantomData,
        )
    }
}

impl<T: DeserializeOwned> Decoder for JsonFrames<T> {
    type Item = T;
    type Error = io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> io::Result<Option<T>> {
        self.0.decode(src)?.map(|b| serde_json::from_slice(&b).map_err(invalid_data)).transpose()
    }
}

impl<T: Serialize> Encoder<T> for JsonFrames<T> {
    type Error = io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> io::Result<()> {
        let json = serde_json::to_vec(&item).map_err(invalid_data)?;
        self.0.encode(json.into(), dst)
    }
}

pub(crate) type FrameReader<R, T> = FramedRead<R, JsonFrames<T>>;
pub(crate) type FrameWriter<W, T> = FramedWrite<W, JsonFrames<T>>;

pub(crate) fn frame_reader<R: tokio::io::AsyncRead, T: DeserializeOwned>(reader: R) -> FrameReader<R, T> {
    FramedRead::new(reader, JsonFrames::new())
}

pub(crate) fn frame_writer<W: tokio::io::AsyncWrite, T: Serialize>(writer: W) -> FrameWriter<W, T> {
    FramedWrite::new(writer, JsonFrames::new())
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

    #[tokio::test]
    async fn test_duplex_reads_multiple_back_to_back_frames() {
        use futures::{SinkExt, StreamExt};

        let (client_io, server_io) = tokio::io::duplex(1024);
        let mut writer = frame_writer::<_, DaemonRequest>(client_io);
        let mut reader = frame_reader::<_, DaemonRequest>(server_io);

        writer.send(DaemonRequest::Ping).await.expect("send ping");
        writer.send(DaemonRequest::Disconnect).await.expect("send disconnect");

        let first = reader.next().await.expect("first frame").expect("decode first frame");
        assert!(matches!(first, DaemonRequest::Ping));

        let second = reader.next().await.expect("second frame").expect("decode second frame");
        assert!(matches!(second, DaemonRequest::Disconnect));
    }
}
