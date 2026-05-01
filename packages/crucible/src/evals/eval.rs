use crate::agents::{Agent, AgentConfig, AgentEvalMessage};
use crate::evals::report::{DiffStats, EvalReport, GitDiff};
use crate::git_repo::GitRepo;
use crate::{EvalRunError, WorkspaceError};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

pub struct Workspace {
    path: PathBuf,
    source: WorkspaceSource,
    _drop_guard: tempfile::TempDir,
}

#[derive(Debug, Clone)]
pub enum WorkspaceSource {
    Local,
    GitRepo { url: String, start_commit: String, gold_commit: String },
}

pub struct GitRepoSpec {
    pub url: String,
    pub start_commit: String,
    pub gold_commit: String,
    pub subdir: Option<PathBuf>,
}

#[tracing::instrument(skip(agent, prompt, workspace))]
pub async fn run_eval<T: Agent>(
    agent: &T,
    prompt: impl Into<String>,
    workspace: Workspace,
) -> Result<EvalReport, EvalRunError> {
    let prompt = prompt.into();
    let messages = run_agent_and_collect_messages(agent, workspace.path(), &prompt).await?;
    let (agent_diff, reference_diff) = capture_git_diffs(&workspace);
    Ok(EvalReport::new(prompt, workspace, messages, agent_diff, reference_diff))
}

impl Workspace {
    pub fn empty() -> Result<Self, WorkspaceError> {
        let temp_dir = new_temp_dir()?;
        let path = temp_dir.path().to_path_buf();
        Ok(Self { path, source: WorkspaceSource::Local, _drop_guard: temp_dir })
    }

    pub fn from_dir(src_path: impl Into<PathBuf>) -> Result<Self, WorkspaceError> {
        let src_path = src_path.into();
        let temp_dir = new_temp_dir()?;
        let path = temp_dir.path().to_path_buf();

        copy_dir_contents(&src_path, &path).map_err(|source| WorkspaceError::CopyFixture {
            from: src_path.clone(),
            to: path.clone(),
            source,
        })?;

        Ok(Self { path, source: WorkspaceSource::Local, _drop_guard: temp_dir })
    }

    pub fn from_git_repo(spec: GitRepoSpec) -> Result<Self, WorkspaceError> {
        let GitRepoSpec { url, start_commit, gold_commit, subdir } = spec;
        let temp_dir = new_temp_dir()?;

        tracing::debug!("Cloning git repo {} at commit {}", url, start_commit);
        let repo = GitRepo::clone(&url, temp_dir.path())?;
        repo.checkout(&start_commit)?;

        let path = match subdir {
            None => temp_dir.path().to_path_buf(),
            Some(subdir) => {
                let working_path = temp_dir.path().join(subdir);
                if !working_path.exists() {
                    return Err(WorkspaceError::MissingSubdir { path: working_path });
                }
                working_path
            }
        };

        Ok(Self { path, source: WorkspaceSource::GitRepo { url, start_commit, gold_commit }, _drop_guard: temp_dir })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn source(&self) -> &WorkspaceSource {
        &self.source
    }
}

async fn run_agent_and_collect_messages<A: Agent>(
    agent: &A,
    workspace_path: &Path,
    task_prompt: &str,
) -> Result<Vec<AgentEvalMessage>, EvalRunError> {
    let (tx, mut rx) = mpsc::channel(100);
    let config = AgentConfig { workspace: workspace_path, task_prompt };
    let agent_task = agent.run(config, tx);
    let message_task = async {
        let mut messages = Vec::new();
        while let Some(message) = rx.recv().await {
            let is_done = matches!(message, AgentEvalMessage::Done);
            messages.push(message);
            if is_done {
                break;
            }
        }
        messages
    };

    let (run_result, messages) = tokio::join!(agent_task, message_task);
    run_result?;
    Ok(messages)
}

fn capture_git_diffs(workspace: &Workspace) -> (Option<GitDiff>, Option<GitDiff>) {
    let WorkspaceSource::GitRepo { start_commit, gold_commit, .. } = workspace.source() else {
        return (None, None);
    };

    let repo = GitRepo::from_path(workspace.path());
    let agent_diff = repo.diff_unstaged().ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });
    let reference_diff =
        repo.diff(start_commit, gold_commit).ok().map(|diff| GitDiff { stats: DiffStats::from_diff(&diff), diff });

    (agent_diff, reference_diff)
}

fn new_temp_dir() -> Result<tempfile::TempDir, WorkspaceError> {
    tempfile::tempdir().map_err(WorkspaceError::CreateTempDir)
}

fn copy_dir_contents(src: &Path, dst: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            std::fs::create_dir_all(&dest_path)?;
            copy_dir_contents(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FakeAgent, RunError};
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc::Sender;

    #[test]
    fn workspace_empty_creates_existing_directory() {
        let workspace = Workspace::empty().unwrap();
        assert!(workspace.path().exists());
        assert!(workspace.path().is_dir());
    }

    #[test]
    fn workspace_from_dir_copies_fixture_contents() {
        let fixture = tempfile::tempdir().unwrap();
        fs::create_dir(fixture.path().join("nested")).unwrap();
        fs::write(fixture.path().join("nested/file.txt"), "hello").unwrap();
        let workspace = Workspace::from_dir(fixture.path()).unwrap();
        assert_eq!(fs::read_to_string(workspace.path().join("nested/file.txt")).unwrap(), "hello");
    }

    #[tokio::test]
    async fn run_eval_returns_report_with_prompt_workspace_and_messages() {
        let report =
            run_eval(&FakeAgent::with_tool_call("bash", "success"), "do the thing", Workspace::empty().unwrap())
                .await
                .unwrap();

        assert_eq!(report.prompt(), "do the thing");
        assert!(report.workspace().path().exists());
        assert!(report.tool_called("bash"));
        assert!(matches!(report.messages().last(), Some(AgentEvalMessage::Done)));
    }

    #[tokio::test]
    async fn run_eval_passes_raw_prompt_without_scaffolding() {
        let agent = CapturingAgent::default();
        run_eval(&agent, "do the thing", Workspace::empty().unwrap()).await.unwrap();

        assert_eq!(agent.captured_task_prompt(), Some("do the thing".to_string()));
    }

    #[derive(Default)]
    struct CapturingAgent {
        task_prompt: Arc<Mutex<Option<String>>>,
    }

    impl CapturingAgent {
        fn captured_task_prompt(&self) -> Option<String> {
            self.task_prompt.lock().unwrap().clone()
        }
    }

    impl Agent for CapturingAgent {
        async fn run(&self, config: AgentConfig<'_>, tx: Sender<AgentEvalMessage>) -> Result<(), RunError> {
            *self.task_prompt.lock().unwrap() = Some(config.task_prompt.to_string());
            tx.send(AgentEvalMessage::Done).await.map_err(|e| RunError::ChannelSendFailed(e.to_string()))
        }
    }
}
