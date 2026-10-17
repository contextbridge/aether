use crate::language_catalog::LanguageId;
use lsp_types::Uri;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;
use tokio_util::bytes::{Bytes, BytesMut};
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite, LengthDelimitedCodec};

#[doc = include_str!("docs/protocol.md")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DaemonRequest {
    Initialize(InitializeRequest),
    Call { id: i64, call: DaemonCall },
    Disconnect,
}

/// Work a client asks of its bound workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DaemonCall {
    /// Forward an LSP request to the language server.
    Lsp { method: String, params: Value },
    /// Read diagnostics for one file, or for the whole workspace when `uri` is `None`.
    Diagnostics { uri: Option<Uri> },
    /// Re-check a file in the background and update its cached diagnostics.
    QueueRefresh { uri: Uri },
}

/// Binds a connection to one workspace and language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InitializeRequest {
    pub(crate) workspace_root: PathBuf,
    pub(crate) language: LanguageId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum DaemonResponse {
    Initialized,
    InitializeFailed(String),
    Reply { id: i64, result: Result<Value, LspErrorResponse> },
}

/// An error answer to a call, either forwarded from the language server or synthesized by the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LspErrorResponse {
    pub(crate) code: i32,
    pub(crate) message: String,
}

pub(crate) const PROTOCOL_VERSION: u32 = 2;

/// Error code for a request that exceeded the daemon's per-request timeout.
/// The daemon kills and replaces the language server when this fires.
///
/// Daemon-synthesized codes live outside the ranges reserved by JSON-RPC
/// (-32000..=-32768) and LSP (-32800..=-32899), so they can never collide with
/// a code forwarded verbatim from a real language server.
pub const LSP_REQUEST_TIMED_OUT: i32 = -33001;

/// Error code for a request that failed because the language server process
/// exited or its transport shut down before responding.
pub(crate) const LSP_TRANSPORT_CLOSED: i32 = -33002;

pub(crate) const DAEMON_ERROR: i32 = -1;
const MAX_MESSAGE_SIZE: u32 = 16 * 1024 * 1024;

impl LspErrorResponse {
    pub(crate) fn daemon(message: impl Into<String>) -> Self {
        Self { code: DAEMON_ERROR, message: message.into() }
    }
}

pub(crate) fn extract_document_uri(method: &str, params: &Value) -> Option<Uri> {
    if !method.starts_with("textDocument/") {
        return None;
    }
    params.pointer("/textDocument/uri").and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
}

pub(crate) type FrameReader<R, T> = FramedRead<R, JsonFrames<T>>;
pub(crate) type FrameWriter<W, T> = FramedWrite<W, JsonFrames<T>>;

pub(crate) fn frame_reader<R: tokio::io::AsyncRead, T: DeserializeOwned>(reader: R) -> FrameReader<R, T> {
    FramedRead::new(reader, JsonFrames::new())
}

pub(crate) fn frame_writer<W: tokio::io::AsyncWrite, T: Serialize>(writer: W) -> FrameWriter<W, T> {
    FramedWrite::new(writer, JsonFrames::new())
}

pub(crate) struct JsonFrames<T>(LengthDelimitedCodec, PhantomData<fn() -> T>);

impl<T> JsonFrames<T> {
    fn new() -> Self {
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
        self.0.encode(Bytes::from(json), dst)
    }
}

fn invalid_data(err: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lsp_call_round_trips_with_its_id() {
        let request = DaemonRequest::Call {
            id: 42,
            call: DaemonCall::Lsp {
                method: "textDocument/definition".to_string(),
                params: json!({
                    "textDocument": { "uri": "file:///test.rs" },
                    "position": { "line": 0, "character": 0 }
                }),
            },
        };
        let decoded: DaemonRequest = serde_json::from_str(&serde_json::to_string(&request).unwrap()).unwrap();

        let DaemonRequest::Call { id, call: DaemonCall::Lsp { method, .. } } = decoded else { panic!("Wrong variant") };
        assert_eq!(id, 42);
        assert_eq!(method, "textDocument/definition");
    }

    #[test]
    fn extract_document_uri_reads_text_document_requests_only() {
        let main_rs = json!({
            "textDocument": { "uri": "file:///src/main.rs" },
            "position": { "line": 10, "character": 5 }
        });
        let cases = [
            ("textDocument/definition", main_rs.clone(), Some("file:///src/main.rs")),
            ("textDocument/references", main_rs.clone(), Some("file:///src/main.rs")),
            (
                "textDocument/documentSymbol",
                json!({ "textDocument": { "uri": "file:///src/foo.rs" } }),
                Some("file:///src/foo.rs"),
            ),
            ("workspace/symbol", json!({ "query": "Foo" }), None),
            ("workspace/executeCommand", main_rs, None),
            ("textDocument/unknown", json!({}), None),
        ];

        for (method, params, expected) in cases {
            let uri = extract_document_uri(method, &params);
            assert_eq!(uri.as_ref().map(|uri| uri.as_str()), expected, "{method}");
        }
    }

    #[tokio::test]
    async fn reads_back_to_back_frames() {
        use futures::{SinkExt, StreamExt};

        let (client_io, server_io) = tokio::io::duplex(1024);
        let mut writer = frame_writer::<_, DaemonRequest>(client_io);
        let mut reader = frame_reader::<_, DaemonRequest>(server_io);

        let call = DaemonCall::Diagnostics { uri: None };
        writer.send(DaemonRequest::Call { id: 1, call }).await.expect("send call");
        writer.send(DaemonRequest::Disconnect).await.expect("send disconnect");

        let first = reader.next().await.expect("first frame").expect("decode first frame");
        assert!(matches!(first, DaemonRequest::Call { id: 1, call: DaemonCall::Diagnostics { uri: None } }));

        let second = reader.next().await.expect("second frame").expect("decode second frame");
        assert!(matches!(second, DaemonRequest::Disconnect));
    }
}
