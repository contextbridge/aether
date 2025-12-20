use std::future::Future;

use super::{
    BackgroundProcessHandle, BashInput, BashResult, EditFileArgs, EditFileResponse, ListFilesArgs,
    ListFilesResult, LspDiagnosticsArgs, LspDiagnosticsResponse, ReadBackgroundBashOutput,
    ReadFileArgs, ReadFileResult, WriteFileArgs, WriteFileResponse,
};

/// Trait defining the underlying implementation for coding tool operations.
///
/// This trait allows CodingMcp to be used in different contexts:
/// - DefaultCodingTools: Uses local filesystem (default behavior)
/// - AcpCodingTools: Delegates to ACP client for editor integration
pub trait CodingTools: Send + Sync + std::fmt::Debug {
    /// Read a file's contents
    fn read_file(
        &self,
        args: ReadFileArgs,
    ) -> impl Future<Output = Result<ReadFileResult, String>> + Send;

    /// Write content to a file
    fn write_file(
        &self,
        args: WriteFileArgs,
    ) -> impl Future<Output = Result<WriteFileResponse, String>> + Send;

    /// Edit a file using string replacement
    fn edit_file(
        &self,
        args: EditFileArgs,
    ) -> impl Future<Output = Result<EditFileResponse, String>> + Send;

    /// List files in a directory
    fn list_files(
        &self,
        args: ListFilesArgs,
    ) -> impl Future<Output = Result<ListFilesResult, String>> + Send;

    /// Execute a bash command
    fn bash(&self, args: BashInput) -> impl Future<Output = Result<BashResult, String>> + Send;

    /// Read output from a background bash process
    fn read_background_bash(
        &self,
        handle: BackgroundProcessHandle,
        filter: Option<String>,
    ) -> impl Future<
        Output = Result<(ReadBackgroundBashOutput, Option<BackgroundProcessHandle>), String>,
    > + Send;

    /// Get LSP diagnostics (compiler errors, warnings, etc.) from the language server
    fn lsp_diagnostics(
        &self,
        args: LspDiagnosticsArgs,
    ) -> impl Future<Output = Result<LspDiagnosticsResponse, String>> + Send;
}
