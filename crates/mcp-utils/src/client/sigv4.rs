use aether_auth::SigV4Signer;
use futures::stream::BoxStream;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::auth::AuthError;
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use sse_stream::Sse;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct SigV4HttpClient {
    http: reqwest::Client,
    signer: SigV4Signer,
}

impl SigV4HttpClient {
    pub(super) fn new(signer: SigV4Signer) -> Self {
        Self { http: reqwest::Client::default(), signer }
    }

    async fn sign_message(
        &self,
        uri: &str,
        message: &ClientJsonRpcMessage,
        headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<HashMap<HeaderName, HeaderValue>, StreamableHttpError<reqwest::Error>> {
        self.sign("POST", uri, &serde_json::to_vec(message)?, headers).await
    }

    async fn sign(
        &self,
        method: &str,
        uri: &str,
        body: &[u8],
        mut headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<HashMap<HeaderName, HeaderValue>, StreamableHttpError<reqwest::Error>> {
        let signed = self
            .signer
            .headers(method, uri, body)
            .await
            .map_err(|error| StreamableHttpError::Auth(AuthError::AuthorizationFailed(error.to_string())))?;

        headers.extend(signed.into_iter().filter_map(|(name, value)| Some((name?, value))));
        Ok(headers)
    }
}

impl StreamableHttpClient for SigV4HttpClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let headers = self.sign_message(&uri, &message, custom_headers).await?;
        self.http.post_message(uri, message, session_id, auth_header, headers).await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let headers = self.sign_message(&uri, &message, custom_headers).await?;
        self.http
            .post_message_with_max_sse_event_size(uri, message, session_id, auth_header, headers, max_sse_event_size)
            .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        let headers = self.sign("DELETE", &uri, &[], custom_headers).await?;
        self.http.delete_session(uri, session_id, auth_header, headers).await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let headers = self.sign("GET", &uri, &[], custom_headers).await?;
        self.http.get_stream(uri, session_id, last_event_id, auth_header, headers).await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let headers = self.sign("GET", &uri, &[], custom_headers).await?;
        self.http
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                auth_header,
                headers,
                max_sse_event_size,
            )
            .await
    }
}
