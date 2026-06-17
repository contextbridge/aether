mod acp_client;
mod agent;
mod docker;
mod docker_aether_agent;
mod docker_agent;
mod fake_agent;
mod image_builder;
mod transcript;

pub use acp_client::{AcpClientCommand, CONTAINER_AETHER_HOME, aether_eval_env_vars};
pub use agent::{Agent, AgentConfig, RunError};
pub use docker::{DockerError, DockerImage, DockerImageParseError};
pub use docker_aether_agent::DockerAetherAgent;
pub use docker_agent::{AETHER_EVAL_WRAPPED_TASK_PROMPT_ENV, AgentCommandBuilder, DockerAgent, DockerCommandConfig};
pub use fake_agent::FakeAgent;
pub use image_builder::{ImageBuildError, ImageBuildRequest, build_images};

pub(crate) use agent::build_task_prompt;
pub(crate) use transcript::{TRANSCRIPT_PAYLOAD_CHARS, get_transcript_line, is_terminal, truncate_chars};
