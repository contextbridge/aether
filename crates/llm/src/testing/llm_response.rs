use crate::LlmResponse;

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

    pub fn tool_call_with_invalid_json(mut self, id: &str, name: &str) -> Self {
        self.chunks.push(LlmResponse::tool_request_start(id, name));
        self.chunks.push(LlmResponse::tool_request_complete(id, name, "invalid json"));

        self
    }

    pub fn build(mut self) -> Vec<LlmResponse> {
        self.chunks.push(LlmResponse::done());
        self.chunks
    }
}
