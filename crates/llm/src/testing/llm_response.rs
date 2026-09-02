use crate::{LlmResponse, StopReason};

pub fn llm_response(message_id: &str) -> LlmResponseBuilder {
    LlmResponseBuilder::new(message_id)
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
