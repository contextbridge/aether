use crate::coding::tools::bash::BashEnvironment;
use crate::{CodingMcp, DefaultCodingTools, ReviewMcp, SkillsMcp, SubAgentsMcp, TasksMcp};
use aether_core::core::AgentDeps;
use aether_core::mcp::{AETHER_MCP_IPC_SOCKET, McpBuilder, RuntimeServices, ToolFilter, mcp};
use clap::{Parser, ValueEnum};
use mcp_utils::server::McpServer;
use std::path::Path;

#[doc = include_str!("docs/mcp_builder_ext.md")]
pub trait McpBuilderExt {
    fn with_builtin_servers(self) -> Self;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Builtin {
    Coding,
    Skills,
    Tasks,
    Subagents,
    Review,
}

pub fn agent_mcp(root: impl AsRef<Path>, deps: AgentDeps, tools: ToolFilter) -> McpBuilder {
    mcp(root).with_agent_deps(deps).with_tool_filter(tools).with_builtin_servers()
}

impl McpBuilderExt for McpBuilder {
    fn with_builtin_servers(self) -> Self {
        Builtin::value_variants().iter().fold(self, |builder, &builtin| {
            builder.register_in_memory_server(builtin.name(), move |args, services| Ok(builtin.build(args, services)?))
        })
    }
}

impl Builtin {
    pub fn name(self) -> &'static str {
        match self {
            Self::Coding => "coding",
            Self::Skills => "skills",
            Self::Tasks => "tasks",
            Self::Subagents => "subagents",
            Self::Review => "review",
        }
    }

    pub fn build(self, args: Vec<String>, services: &RuntimeServices) -> Result<McpServer, clap::Error> {
        let root = services.root_dir.as_path();
        Ok(match self {
            Self::Coding => CodingMcp::from_args(parse(args)?, root, coding_tools(services)).into(),
            Self::Skills => SkillsMcp::from_args(parse(args)?, root).into(),
            Self::Tasks => TasksMcp::from_args(parse(args)?, root).into(),
            Self::Review => ReviewMcp::from_args(parse(args)?, root).into(),
            Self::Subagents => SubAgentsMcp::from_args(parse(args)?, root, services.agent_deps.clone()).into(),
        })
    }
}

fn parse<T: Parser>(args: Vec<String>) -> Result<T, clap::Error> {
    T::try_parse_from(args)
}

fn coding_tools(services: &RuntimeServices) -> DefaultCodingTools {
    let environment = BashEnvironment::new().with_current_exe_dir_on_path();
    let environment = match &services.deferred_tools_socket {
        Some(socket) => environment.with_var(AETHER_MCP_IPC_SOCKET, socket.to_string_lossy()),
        None => environment,
    };
    DefaultCodingTools::new().with_bash_environment(environment)
}
