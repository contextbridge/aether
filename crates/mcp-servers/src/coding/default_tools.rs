use super::error::CodingError;
use super::tools::bash::{BashEnvironment, execute_command};
use super::{
    BashInput, BashOutput, EditFileArgs, EditFileResponse, ReadFileArgs, ReadFileResult, WriteFileArgs,
    WriteFileResponse, edit_file_contents, read_file_contents, tools_trait::CodingTools, write_file_contents,
};
use std::path::PathBuf;

/// Default implementation that uses local filesystem operations.
///
/// This is the standard behavior for `CodingMcp` when running outside
/// of an ACP context. Bash commands inherit the Aether executable's
/// directory on `PATH` so the `aether` CLI resolves from Bash.
#[derive(Debug)]
pub struct DefaultCodingTools {
    bash_environment: BashEnvironment,
}

impl Default for DefaultCodingTools {
    fn default() -> Self {
        Self { bash_environment: BashEnvironment::default().with_current_exe_dir_on_path() }
    }
}

impl DefaultCodingTools {
    /// Create a new `DefaultCodingTools` instance
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_bash_environment(mut self, environment: BashEnvironment) -> Self {
        self.bash_environment = environment;
        self
    }
}

impl CodingTools for DefaultCodingTools {
    async fn read_file(&self, args: ReadFileArgs) -> Result<ReadFileResult, CodingError> {
        read_file_contents(args).await.map_err(CodingError::from)
    }

    async fn write_file(&self, args: WriteFileArgs) -> Result<WriteFileResponse, CodingError> {
        write_file_contents(args).await.map_err(CodingError::from)
    }

    async fn edit_file(&self, args: EditFileArgs) -> Result<EditFileResponse, CodingError> {
        edit_file_contents(args).await.map_err(CodingError::from)
    }

    async fn bash(&self, args: BashInput, cwd: Option<PathBuf>) -> Result<BashOutput, CodingError> {
        execute_command(args, cwd.as_deref(), &self.bash_environment).await.map_err(CodingError::from)
    }
}
