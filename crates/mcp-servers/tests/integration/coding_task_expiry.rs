use super::common::{TestClient, production_client_info};
use mcp_servers::coding::{
    CodingMcp, CodingTools, DefaultCodingTools,
    error::CodingError,
    tools::{
        ast_grep::{AstGrepInput, AstGrepOutput},
        bash::{BashEnvironment, BashInput, BashOutput},
        edit_file::{EditFileArgs, EditFileResponse},
        find::{FindInput, FindOutput},
        grep::{GrepInput, GrepOutput},
        list_files::{ListFilesArgs, ListFilesResult},
        read_file::{ReadFileArgs, ReadFileResult},
        write_file::{WriteFileArgs, WriteFileResponse},
    },
};
use mcp_utils::server::tasks::BACKGROUND_TASK_TTL_MS;
use rmcp::model::{CallToolRequestParams, CallToolResponse};
use serde_json::json;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

#[tokio::test(start_paused = true)]
async fn abandoned_bash_tasks_expire_without_client_polling() {
    let commands = Arc::new(Mutex::new(HashSet::new()));
    let started = Arc::new(Notify::new());
    let tools = WaitingShell { commands: commands.clone(), started: started.clone(), files: DefaultCodingTools::new() };
    let client = TestClient::start_with(|| CodingMcp::with_tools(tools), production_client_info()).await.unwrap();
    let response =
        client
            .raw()
            .call_tool_once(CallToolRequestParams::new("bash").with_arguments(
                json!({"command":"wait-for-input","runInBackground":true}).as_object().unwrap().clone(),
            ))
            .await
            .unwrap();
    assert!(matches!(response, CallToolResponse::Task(_)));
    started.notified().await;
    assert!(commands.lock().unwrap().contains("wait-for-input"));
    tokio::time::advance(Duration::from_millis(BACKGROUND_TASK_TTL_MS + 1)).await;
    tokio::task::yield_now().await;
    assert!(
        commands.lock().unwrap().is_empty(),
        "an abandoned task must release its running subprocess without tasks/get"
    );
}

struct WaitingShell {
    commands: Arc<Mutex<HashSet<String>>>,
    started: Arc<Notify>,
    files: DefaultCodingTools,
}

struct RunningCommand {
    commands: Arc<Mutex<HashSet<String>>>,
    command: String,
}

impl Drop for RunningCommand {
    fn drop(&mut self) {
        self.commands.lock().unwrap().remove(&self.command);
    }
}

impl CodingTools for WaitingShell {
    async fn bash(&self, args: BashInput, _: Option<PathBuf>, _: BashEnvironment) -> Result<BashOutput, CodingError> {
        self.commands.lock().unwrap().insert(args.command.clone());
        let _process = RunningCommand { commands: self.commands.clone(), command: args.command };
        self.started.notify_one();
        std::future::pending().await
    }

    async fn read_file(&self, args: ReadFileArgs) -> Result<ReadFileResult, CodingError> {
        self.files.read_file(args).await
    }
    async fn write_file(&self, args: WriteFileArgs) -> Result<WriteFileResponse, CodingError> {
        self.files.write_file(args).await
    }
    async fn edit_file(&self, args: EditFileArgs) -> Result<EditFileResponse, CodingError> {
        self.files.edit_file(args).await
    }
    async fn list_files(&self, args: ListFilesArgs) -> Result<ListFilesResult, CodingError> {
        self.files.list_files(args).await
    }
    async fn grep(&self, args: GrepInput) -> Result<GrepOutput, CodingError> {
        self.files.grep(args).await
    }
    async fn ast_grep(&self, args: AstGrepInput) -> Result<AstGrepOutput, CodingError> {
        self.files.ast_grep(args).await
    }
    async fn find(&self, args: FindInput) -> Result<FindOutput, CodingError> {
        self.files.find(args).await
    }
}
