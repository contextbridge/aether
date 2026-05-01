use crate::EvalHarnessError;
use crucible::{AetherAgent, Prompt};
use futures::FutureExt;
use llm::StreamingModelProvider;
use llm::parser::ModelProviderParser;
use mcp_servers::{CodingMcp, PermissionMode};
use mcp_utils::ServiceExt;
use mcp_utils::client::ServerFactory;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub type Agent = AetherAgent<Arc<dyn StreamingModelProvider>>;

pub async fn create_aether_agent(workspace_path: &Path) -> Result<Agent, EvalHarnessError> {
    let model = std::env::var("AETHER_EVAL_MODEL").map_err(|_| EvalHarnessError::MissingEvalModel)?;
    let (provider, _) = ModelProviderParser::default().parse(&model).await?;
    let provider = Arc::from(provider);

    let prompt_path = format!("{}/prompts/coding_agent.md", env!("CARGO_MANIFEST_DIR"));
    let system_prompt = Prompt::file(&prompt_path).with_cwd(workspace_path.to_path_buf());

    Ok(AetherAgent::new(provider)
        .with_mcp_server_factory("coding", coding_server_factory(workspace_path.to_path_buf()))
        .with_system_prompt(system_prompt))
}

fn coding_server_factory(root: PathBuf) -> ServerFactory {
    Box::new(move |_args, _input| {
        let root = root.clone();
        async move { CodingMcp::new().with_root_dir(root).with_permission_mode(PermissionMode::AlwaysAllow).into_dyn() }
            .boxed()
    })
}
