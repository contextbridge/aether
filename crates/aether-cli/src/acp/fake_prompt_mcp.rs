use rmcp::model::{
    GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult, PaginatedRequestParams,
    Prompt as McpPrompt, PromptMessage, Role, ServerCapabilities, ServerInfo,
};
use rmcp::service::{DynService, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

#[derive(Clone)]
pub(crate) struct FakePromptMcp {
    prompt_name: String,
}

impl FakePromptMcp {
    pub(crate) fn new(prompt_name: &str) -> Self {
        Self { prompt_name: prompt_name.to_string() }
    }

    pub(crate) fn into_dyn(self) -> Box<dyn DynService<RoleServer>> {
        Box::new(self)
    }
}

impl ServerHandler for FakePromptMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_prompts().build())
            .with_server_info(Implementation::new("fake-prompt-mcp", "0.1.0"))
            .with_instructions("Fake MCP server exposing a single prompt for tests")
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListPromptsResult, McpError>> + Send + '_ {
        let prompt = McpPrompt::new(&self.prompt_name, Some(format!("{} command", self.prompt_name)), None);
        std::future::ready(Ok(ListPromptsResult::with_all_items(vec![prompt])))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<GetPromptResponse, McpError>> + Send + '_ {
        let result = if request.name.as_str() == self.prompt_name {
            let messages = vec![PromptMessage::new_text(Role::User, format!("expanded {}", self.prompt_name))];
            Ok(rmcp::model::GetPromptResult::new(messages).into())
        } else {
            Err(McpError::invalid_params(format!("Prompt '{}' not found", request.name), None))
        };
        std::future::ready(result)
    }
}
