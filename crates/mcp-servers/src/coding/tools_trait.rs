use std::future::Future;
use std::path::PathBuf;

use super::error::CodingError;
use super::tools::bash::{BashInput, BashOutput};
use super::tools::edit_file::{EditFileArgs, EditFileResponse};
use super::tools::read_file::{ReadFileArgs, ReadFileResult};
use super::tools::write_file::{WriteFileArgs, WriteFileResponse};

#[doc = include_str!("../docs/coding_tools.md")]
pub trait CodingTools: Send + Sync {
    /// Read a file's contents
    fn read_file(&self, args: ReadFileArgs) -> impl Future<Output = Result<ReadFileResult, CodingError>> + Send;

    /// Write content to a file
    fn write_file(&self, args: WriteFileArgs) -> impl Future<Output = Result<WriteFileResponse, CodingError>> + Send;

    /// Edit a file using string replacement
    fn edit_file(&self, args: EditFileArgs) -> impl Future<Output = Result<EditFileResponse, CodingError>> + Send;

    // Execute a bash command with an optional working directory
    fn bash(
        &self,
        args: BashInput,
        cwd: Option<PathBuf>,
    ) -> impl Future<Output = Result<BashOutput, CodingError>> + Send;
}
