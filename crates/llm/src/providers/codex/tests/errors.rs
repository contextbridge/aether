use super::test_server::{FakeResponsesWebsocketServer, Reply};
use super::{context, provider, provider_at};
use crate::StreamingModelProvider;
use futures::StreamExt;

#[tokio::test]
async fn classified_frame_errors_keep_status_code_and_request_id() {
    for (status, code, kind) in [
        (401, "invalid_api_key", crate::ProviderErrorKind::Authentication),
        (403, "invalid_authentication", crate::ProviderErrorKind::Authentication),
        (429, "rate_limit_exceeded", crate::ProviderErrorKind::RateLimit),
        (500, "server_error", crate::ProviderErrorKind::Server),
    ] {
        for wrapped in [true, false] {
            let error = serde_json::json!({"code":code,"message":"rejected"});
            let frame = if wrapped {
                serde_json::json!({"type":"error","status":status,"error":error,"headers":{"X-Request-ID":["req-1"]}})
            } else {
                serde_json::json!({"type":"error","status":status,"code":code,"message":"rejected","headers":{"x-request-id":"req-1"}})
            };
            let server = FakeResponsesWebsocketServer::start(vec![Reply::events(vec![frame])]).await;
            let responses = provider(&server).stream_response(&context("same")).collect::<Vec<_>>().await;
            assert_eq!(responses.len(), 1);
            let error = responses[0].as_ref().unwrap_err().provider().unwrap();
            assert_eq!(error.kind, kind);
            assert_eq!(error.http_status, Some(status));
            assert_eq!(error.code.as_deref(), Some(code));
            assert_eq!(error.request_id.as_deref(), Some("req-1"));
        }
    }
}

#[tokio::test]
async fn statusless_errors_use_explicit_codes_and_status_takes_precedence() {
    for (code, kind) in [
        ("invalid_api_key", crate::ProviderErrorKind::Authentication),
        ("invalid_authentication", crate::ProviderErrorKind::Authentication),
        ("authentication_error", crate::ProviderErrorKind::Authentication),
        ("token_expired", crate::ProviderErrorKind::Authentication),
        ("rate_limit_exceeded", crate::ProviderErrorKind::RateLimit),
        ("server_error", crate::ProviderErrorKind::Server),
        ("unclassified", crate::ProviderErrorKind::Unknown),
    ] {
        for status in [None, Some(400)] {
            let frame = serde_json::json!({"type":"error","status":status,"error":{"code":code,"message":"rejected"}});
            let server = FakeResponsesWebsocketServer::start(vec![Reply::events(vec![frame])]).await;
            let responses = provider(&server).stream_response(&context("same")).collect::<Vec<_>>().await;
            assert_eq!(
                responses[0].as_ref().unwrap_err().provider().unwrap().kind,
                if status.is_some() { crate::ProviderErrorKind::Api } else { kind }
            );
        }
    }
}

#[tokio::test]
async fn failed_response_preserves_frame_and_connection_diagnostics() {
    for headers in [serde_json::json!({"X-Request-Id":["frame-request"]}), serde_json::json!({})] {
        let frame = serde_json::json!({"type":"response.failed","status":429,"headers":headers,"response":{"error":{"code":"quota","message":"Try later"}}});
        let server = FakeResponsesWebsocketServer::start(vec![Reply::events(vec![frame])]).await;
        let responses = provider(&server).stream_response(&context("same")).collect::<Vec<_>>().await;
        let error = responses[0].as_ref().unwrap_err().provider().unwrap();
        assert_eq!(error.kind, crate::ProviderErrorKind::RateLimit);
        assert_eq!(error.http_status, Some(429));
        assert_eq!(error.code.as_deref(), Some("quota"));
        assert_eq!(error.message, "Try later");
        assert_eq!(
            error.request_id.as_deref(),
            Some(if headers.as_object().unwrap().is_empty() { "upgrade-request" } else { "frame-request" })
        );
    }
}

#[tokio::test]
async fn upgrade_rejections_have_diagnostics_and_never_fall_back_to_post() {
    for (status, kind) in [
        (401, crate::ProviderErrorKind::Authentication),
        (403, crate::ProviderErrorKind::Authentication),
        (429, crate::ProviderErrorKind::RateLimit),
        (500, crate::ProviderErrorKind::Server),
        (426, crate::ProviderErrorKind::Api),
    ] {
        let server = FakeResponsesWebsocketServer::start(vec![]).await;
        let provider = provider_at(&format!("{}/reject/{status}", server.base_url));
        let responses = provider.stream_response(&context("same")).collect::<Vec<_>>().await;
        assert_eq!(responses.len(), 1);
        let error = responses[0].as_ref().unwrap_err().provider().unwrap();
        assert_eq!(error.kind, kind);
        assert_eq!(error.http_status, Some(status));
        assert_eq!(error.code.as_deref(), Some("rejected"));
        assert_eq!(error.request_id.as_deref(), Some("upgrade-rejected"));
    }
}

#[tokio::test]
async fn upgrade_rejection_diagnostics_are_bounded_and_non_json_is_safe() {
    for mode in ["large", "text"] {
        let server = FakeResponsesWebsocketServer::start(vec![]).await;
        let provider = provider_at(&format!("{}/reject-body/{mode}", server.base_url));
        let responses = provider.stream_response(&context("same")).collect::<Vec<_>>().await;
        let error = responses[0].as_ref().unwrap_err().provider().unwrap();
        assert_eq!(error.kind, crate::ProviderErrorKind::RateLimit);
        assert_eq!(error.http_status, Some(429));
        assert_eq!(error.request_id.as_deref(), Some("upgrade-rejected"));
        assert_eq!(error.code, None);
        assert!(!error.message.contains("sensitive"));
    }
}
