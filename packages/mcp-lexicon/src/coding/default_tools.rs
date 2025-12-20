use super::{
    BackgroundProcessHandle, BashInput, BashResult, EditFileArgs, EditFileResponse, ListFilesArgs,
    ListFilesResult, LspDiagnosticsArgs, LspDiagnosticsResponse, ReadBackgroundBashOutput,
    ReadFileArgs, ReadFileResult, WriteFileArgs, WriteFileResponse, edit_file_contents,
    execute_command, list_files, lsp, read_background_bash, read_file_contents,
    tools_trait::CodingTools, write_file_contents,
};

/// Default implementation that uses local filesystem operations.
///
/// This is the standard behavior for CodingMcp when running outside
/// of an ACP context.
#[derive(Debug, Default, Clone)]
pub struct DefaultCodingTools;

impl CodingTools for DefaultCodingTools {
    async fn read_file(&self, args: ReadFileArgs) -> Result<ReadFileResult, String> {
        read_file_contents(args)
            .await
            .map_err(|e| format!("Read file error: {e}"))
    }

    async fn write_file(&self, args: WriteFileArgs) -> Result<WriteFileResponse, String> {
        write_file_contents(args)
            .await
            .map_err(|e| format!("Write file error: {e}"))
    }

    async fn edit_file(&self, args: EditFileArgs) -> Result<EditFileResponse, String> {
        edit_file_contents(args)
            .await
            .map_err(|e| format!("Edit file error: {e}"))
    }

    async fn list_files(&self, args: ListFilesArgs) -> Result<ListFilesResult, String> {
        list_files(args)
            .await
            .map_err(|e| format!("List files error: {e}"))
    }

    async fn bash(&self, args: BashInput) -> Result<BashResult, String> {
        execute_command(args)
            .await
            .map_err(|e| format!("Bash command error: {e}"))
    }

    async fn read_background_bash(
        &self,
        handle: BackgroundProcessHandle,
        filter: Option<String>,
    ) -> Result<(ReadBackgroundBashOutput, Option<BackgroundProcessHandle>), String> {
        read_background_bash(handle, filter)
            .await
            .map_err(|e| format!("Failed to get output: {e}"))
    }

    async fn lsp_diagnostics(
        &self,
        args: LspDiagnosticsArgs,
    ) -> Result<LspDiagnosticsResponse, String> {
        let diagnostics =
            lsp::collect_diagnostics(args.workspace_root, args.severity_filter).await?;

        Ok(LspDiagnosticsResponse {
            status: "success".to_string(),
            diagnostics: diagnostics.clone(),
            total_count: diagnostics.len(),
        })
    }
}
