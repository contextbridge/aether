mod agent;
mod docker;
mod docker_agent;
mod env;
mod fake_agent;
mod transcript;

pub use agent::{Agent, AgentConfig, RunError};
pub use docker::{DockerError, DockerImage, DockerImageParseError};
pub use docker_agent::{
    AETHER_EVAL_CWD_ENV, AETHER_EVAL_WORKSPACE_ROOT_ENV, AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, DockerAgent,
};
pub use env::{CONTAINER_AETHER_HOME, default_eval_env_vars};
pub use fake_agent::FakeAgent;

pub(crate) use agent::build_task_prompt;
pub(crate) use transcript::{TRANSCRIPT_PAYLOAD_CHARS, get_transcript_line, is_terminal};
