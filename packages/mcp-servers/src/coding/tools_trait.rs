use std::fmt::Debug;
use std::future::Future;

use super::error::CodingError;
use super::tools::bash::{BackgroundProcessHandle, BashInput, BashResult, ReadBackgroundBashOutput};
use super::tools::edit_file::{EditFileArgs, EditFileResponse};
use super::tools::find::{FindInput, FindOutput, find_files_by_name};
use super::tools::grep::{GrepInput, GrepOutput, perform_grep};
use super::tools::list_files::{ListFilesArgs, ListFilesResult};
use super::tools::read_file::{ReadFileArgs, ReadFileResult};
use super::tools::write_file::{WriteFileArgs, WriteFileResponse};

#[doc = include_str!("../docs/coding_tools.md")]
pub trait CodingTools: Send + Sync + Debug {
    /// Read a file's contents
    fn read_file(&self, args: ReadFileArgs) -> impl Future<Output = Result<ReadFileResult, CodingError>> + Send;

    /// Write content to a file
    fn write_file(&self, args: WriteFileArgs) -> impl Future<Output = Result<WriteFileResponse, CodingError>> + Send;

    /// Edit a file using line-numbered edits
    fn edit_file(&self, args: EditFileArgs) -> impl Future<Output = Result<EditFileResponse, CodingError>> + Send;

    /// List files in a directory
    fn list_files(&self, args: ListFilesArgs) -> impl Future<Output = Result<ListFilesResult, CodingError>> + Send;

    /// Execute a bash command
    fn bash(&self, args: BashInput) -> impl Future<Output = Result<BashResult, CodingError>> + Send;

    /// Read output from a background bash process
    fn read_background_bash(
        &self,
        handle: BackgroundProcessHandle,
        filter: Option<String>,
    ) -> impl Future<Output = Result<(ReadBackgroundBashOutput, Option<BackgroundProcessHandle>), CodingError>> + Send;

    /// Search file contents using regex patterns.
    fn grep(&self, args: GrepInput) -> impl Future<Output = Result<GrepOutput, CodingError>> + Send {
        async move { perform_grep(args).await.map_err(CodingError::from) }
    }

    /// Find files by name using glob patterns.
    fn find(&self, args: FindInput) -> impl Future<Output = Result<FindOutput, CodingError>> + Send {
        async move { find_files_by_name(args).await.map_err(CodingError::from) }
    }
}
