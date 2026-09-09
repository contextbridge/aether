use crate::{LlmError, LlmResponse, StopReason};

pub fn llm_response(message_id: &str) -> LlmResponseBuilder {
    LlmResponseBuilder::new(message_id)
}

/// A turn whose call fails before the provider emits any frames.
pub fn failed_call(error: impl Into<LlmError>) -> Vec<Result<LlmResponse, LlmError>> {
    vec![Err(error.into())]
}

pub struct LlmResponseBuilder {
    chunks: Vec<LlmResponse>,
}

impl LlmResponseBuilder {
    pub fn new(message_id: &str) -> Self {
        Self { chunks: vec![LlmResponse::start(message_id)] }
    }

    pub fn text(mut self, chunks: &[&str]) -> Self {
        for chunk in chunks {
            self.chunks.push(LlmResponse::text(chunk));
        }

        self
    }

    pub fn reasoning(mut self, chunks: &[&str]) -> Self {
        for chunk in chunks {
            self.chunks.push(LlmResponse::reasoning(chunk));
        }

        self
    }

    pub fn tool_call(mut self, id: &str, name: &str, argument_chunks: &[&str]) -> Self {
        self.chunks.push(LlmResponse::tool_request_start(id, name));

        for chunk in argument_chunks {
            self.chunks.push(LlmResponse::tool_request_arg(id, chunk));
        }

        self.chunks.push(LlmResponse::tool_request_complete(id, name, &argument_chunks.join("")));

        self
    }

    pub fn usage(mut self, input_tokens: u64, output_tokens: u64) -> Self {
        self.chunks.push(LlmResponse::usage(input_tokens, output_tokens));
        self
    }

    pub fn tool_call_with_invalid_json(mut self, id: &str, name: &str) -> Self {
        self.chunks.push(LlmResponse::tool_request_start(id, name));
        self.chunks.push(LlmResponse::tool_request_complete(id, name, "invalid json"));

        self
    }

    pub fn build(mut self) -> Vec<LlmResponse> {
        self.chunks.push(LlmResponse::done());
        self.chunks
    }

    pub fn build_with_stop_reason(mut self, stop_reason: StopReason) -> Vec<LlmResponse> {
        self.chunks.push(LlmResponse::done_with_stop_reason(stop_reason));
        self.chunks
    }

    pub fn build_results(self) -> Vec<Result<LlmResponse, LlmError>> {
        self.build().into_iter().map(Ok).collect()
    }

    /// The stream surfaces `error` after the frames built so far, then closes
    /// with `Done` — a provider that reports a failure before ending cleanly.
    pub fn build_with_error(self, error: impl Into<LlmError>) -> Vec<Result<LlmResponse, LlmError>> {
        let mut results = self.build_results();
        results.insert(results.len() - 1, Err(error.into()));
        results
    }

    /// The stream dies on `error` instead of delivering `Done` — a connection
    /// lost mid-flight.
    pub fn build_interrupted(self, error: impl Into<LlmError>) -> Vec<Result<LlmResponse, LlmError>> {
        let mut results: Vec<_> = self.chunks.into_iter().map(Ok).collect();
        results.push(Err(error.into()));
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderError;

    #[test]
    fn build_with_stop_reason_preserves_response_chunks() {
        let response = llm_response("message").text(&["hello"]).usage(10, 2).build_with_stop_reason(StopReason::Length);

        assert!(matches!(
            response.as_slice(),
            [
                LlmResponse::Start { .. },
                LlmResponse::Text { .. },
                LlmResponse::Usage { .. },
                LlmResponse::Done { stop_reason: Some(StopReason::Length) },
            ]
        ));
    }

    #[test]
    fn reasoning_appends_reasoning_frames() {
        let frames = llm_response("message").reasoning(&["thinking", "harder"]).text(&["answer"]).build();

        assert!(matches!(
            frames.as_slice(),
            [
                LlmResponse::Start { .. },
                LlmResponse::Reasoning { .. },
                LlmResponse::Reasoning { .. },
                LlmResponse::Text { .. },
                LlmResponse::Done { .. },
            ]
        ));
    }

    #[test]
    fn build_results_wraps_success_frames_in_ok() {
        let results = llm_response("message").text(&["hi"]).build_results();

        assert!(matches!(
            results.as_slice(),
            [Ok(LlmResponse::Start { .. }), Ok(LlmResponse::Text { .. }), Ok(LlmResponse::Done { .. })]
        ));
    }

    #[test]
    fn build_with_error_surfaces_error_before_done() {
        let results = llm_response("message").usage(9, 1).build_with_error(ProviderError::api("HTTP 500"));

        assert!(matches!(
            results.as_slice(),
            [Ok(LlmResponse::Start { .. }), Ok(LlmResponse::Usage { .. }), Err(_), Ok(LlmResponse::Done { .. }),]
        ));
    }

    #[test]
    fn build_interrupted_ends_with_error_and_no_done() {
        let results =
            llm_response("message").text(&["partial"]).build_interrupted(ProviderError::stream_interrupted("boom"));

        assert!(matches!(results.as_slice(), [Ok(LlmResponse::Start { .. }), Ok(LlmResponse::Text { .. }), Err(_)]));
    }

    #[test]
    fn failed_call_contains_only_the_error() {
        let results = failed_call(ProviderError::server("boom"));

        assert!(matches!(results.as_slice(), [Err(_)]));
    }
}
