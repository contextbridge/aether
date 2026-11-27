use base64::{Engine, prelude::BASE64_STANDARD};
use foxfire_acp::display_meta::ToolDisplayMeta;
use futures::future::join_all;
use mcp_utils::Displayed;
use mcp_utils::client::{ClientError, McpClient};
use mcp_utils::gateway::McpGateway;
use mcp_utils::tool_kind::{ToolKind, tool_kind_meta};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::tool::schema_for_output;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Implementation, ReadResourceDirectoryRequestParams, ResourceContents, SKILL_MD, ServerCapabilities, ServerConfig,
    SkillIntegrityError, SkillResources, SkillValidationError,
};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Clone)]
pub(super) struct SkillsHost {
    name: String,
    gateway: McpGateway,
    tool_router: ToolRouter<Self>,
}

impl SkillsHost {
    pub(super) fn new(name: impl Into<String>, gateway: McpGateway) -> Self {
        Self { name: name.into(), gateway, tool_router: Self::tool_router() }
    }

    pub(super) fn name(&self) -> &str {
        &self.name
    }
}

#[derive(Deserialize, JsonSchema)]
#[schemars(extend("properties" = {}))]
struct ListSkillsInput {}

#[derive(Serialize, JsonSchema)]
struct ListSkillsOutput {
    skills: Vec<SkillListItem>,
}

#[derive(Serialize, JsonSchema)]
struct SkillListItem {
    server: String,
    uri: String,
    description: String,
}

#[derive(Deserialize, JsonSchema)]
struct ReadSkillInput {
    server: String,
    uri: String,
    /// A supporting file to load instead.
    #[serde(default = "skill_md")]
    path: String,
}

#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for SkillsHost {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(&self.name, env!("CARGO_PKG_VERSION")))
    }
}

#[tool_router]
impl SkillsHost {
    #[doc = include_str!("skills_host/list_skills.md")]
    #[tool(
        meta = tool_kind_meta(ToolKind::Read),
        annotations(read_only_hint = true, open_world_hint = false),
        output_schema = schema_for_output::<ListSkillsOutput>()
    )]
    async fn list_skills(&self, Parameters(_): Parameters<ListSkillsInput>) -> Displayed<ListSkillsOutput> {
        let servers =
            self.gateway.servers().into_iter().filter(|server| server.client().capabilities().supports_skills());
        let listed = join_all(servers.map(|server| async move {
            let skills = server.client().list_skills().await.unwrap_or_else(|error| {
                tracing::warn!(server = server.name(), %error, "Failed to list MCP skills");
                Vec::new()
            });
            let valid = skills.into_iter().filter(|skill| skill.validate().is_ok());
            let listed = valid.map(|skill| SkillListItem {
                server: server.name().to_string(),
                uri: skill.uri,
                description: skill.frontmatter.description,
            });
            listed.collect::<Vec<_>>()
        }));
        let mut skills: Vec<_> = listed.await.into_iter().flatten().collect();
        skills.sort_by(|a, b| (&a.server, &a.uri).cmp(&(&b.server, &b.uri)));

        let display = ToolDisplayMeta::new("Skills", format!("{} skills", skills.len()));
        Displayed::new(ListSkillsOutput { skills }, display)
    }

    #[doc = include_str!("skills_host/read_skill.md")]
    #[tool(meta = tool_kind_meta(ToolKind::Read), annotations(read_only_hint = true, open_world_hint = false))]
    async fn read_skill(&self, Parameters(input): Parameters<ReadSkillInput>) -> Result<String, String> {
        self.read(&input).await.map_err(|error| error.to_string())
    }
}

#[derive(Debug, Error)]
enum ReadSkillError {
    #[error("No connected MCP server is named '{0}'")]
    ServerNotFound(String),
    #[error(transparent)]
    Request(#[from] ClientError),
    #[error("The server answered with skill {0} instead of the requested one")]
    WrongSkill(String),
    #[error("The server's entry for the skill is invalid: {0}")]
    InvalidSkill(#[from] SkillValidationError),
    #[error("'{path}' is not a file of this skill. Its files are: {}", files.join(", "))]
    NotInSkill { path: String, files: Vec<String> },
    #[error("'{0}' is not a path inside the skill's directory")]
    OutsideSkill(String),
    #[error("The server returned something other than the one requested file")]
    UnexpectedContents,
    #[error("The file is not valid base64: {0}")]
    Encoding(#[from] base64::DecodeError),
    #[error("The file does not match what the skill lists: {0}")]
    Integrity(#[from] SkillIntegrityError),
    #[error("The file is not UTF-8 text")]
    NotText,
    #[error("The frontmatter of SKILL.md does not match the server's entry for the skill")]
    FrontmatterMismatch,
    #[error("The skill's directory takes more than {DIRECTORY_READ_LIMIT} requests to list")]
    DirectoryTooLarge,
}

const DIRECTORY_READ_LIMIT: usize = 512;

impl SkillsHost {
    async fn read(&self, input: &ReadSkillInput) -> Result<String, ReadSkillError> {
        let server =
            self.gateway.server(&input.server).ok_or_else(|| ReadSkillError::ServerNotFound(input.server.clone()))?;
        let client = server.client();
        let skill = client.get_skill(&input.uri).await?;
        if skill.uri != input.uri {
            return Err(ReadSkillError::WrongSkill(skill.uri));
        }
        skill.validate()?;

        let root = skill.uri.strip_suffix(SKILL_MD).unwrap_or(&skill.uri);
        let path = input.path.as_str();
        let uri = format!("{root}{path}");
        let manifest = match &skill.resources {
            SkillResources::Files(files) => Some(files),
            _ => None,
        };
        let listed = || {
            let files = manifest.into_iter().flatten().filter(|file| file.uri != skill.uri);
            files.filter_map(|file| file.uri.strip_prefix(root)).map(str::to_string).collect::<Vec<_>>()
        };
        let entry = match manifest {
            Some(files) => Some(
                files
                    .iter()
                    .find(|file| file.uri == uri)
                    .ok_or_else(|| ReadSkillError::NotInSkill { path: path.to_string(), files: listed() })?,
            ),
            None if is_inside_skill(path) => None,
            None => return Err(ReadSkillError::OutsideSkill(path.to_string())),
        };

        let bytes = match <[ResourceContents; 1]>::try_from(client.read_resource(&uri).await?) {
            Ok([ResourceContents::TextResourceContents { uri: read, text, .. }]) if read == uri => text.into_bytes(),
            Ok([ResourceContents::BlobResourceContents { uri: read, blob, .. }]) if read == uri => {
                BASE64_STANDARD.decode(blob)?
            }
            _ => return Err(ReadSkillError::UnexpectedContents),
        };
        if let Some(entry) = entry {
            entry.verify(&bytes)?;
        }
        let content = String::from_utf8(bytes).map_err(|_| ReadSkillError::NotText)?;

        if uri != skill.uri {
            return Ok(content);
        }
        let (frontmatter, body) = split_frontmatter(&content).ok_or(ReadSkillError::FrontmatterMismatch)?;
        if Some(frontmatter) != serde_json::to_value(&skill.frontmatter).ok() {
            return Err(ReadSkillError::FrontmatterMismatch);
        }
        let files = match manifest {
            Some(_) => listed(),
            None => discover_files(client, root).await?,
        };
        if files.is_empty() {
            return Ok(body.to_string());
        }
        Ok(format!("{body}\n\nSupporting files: {}", files.join(", ")))
    }
}

async fn discover_files(client: &McpClient, root: &str) -> Result<Vec<String>, ReadSkillError> {
    let mut files = Vec::new();
    let mut pending = Vec::new();
    if client.capabilities().supports_directory_read() {
        pending.push(ReadResourceDirectoryRequestParams::new(root.trim_end_matches('/')));
    }
    let mut requests = 0;
    while let Some(request) = pending.pop() {
        requests += 1;
        if requests > DIRECTORY_READ_LIMIT {
            return Err(ReadSkillError::DirectoryTooLarge);
        }
        let page = client.read_directory(request.clone()).await?;
        for child in page.resources {
            match child.uri.strip_prefix(root) {
                Some(_) if child.is_directory() => pending.push(ReadResourceDirectoryRequestParams::new(&child.uri)),
                Some(path) if path != SKILL_MD => files.push(path.to_string()),
                _ => {}
            }
        }
        pending.extend(page.next_cursor.map(|cursor| request.with_cursor(cursor)));
    }
    files.sort();
    Ok(files)
}

fn skill_md() -> String {
    SKILL_MD.to_string()
}

fn is_inside_skill(path: &str) -> bool {
    !path.contains(['?', '#']) && path.split('/').all(|segment| !matches!(segment, "" | "." | ".."))
}

fn split_frontmatter(skill_md: &str) -> Option<(Value, &str)> {
    let rest = skill_md.strip_prefix("---")?;
    let (yaml, after) = rest.split_once("\n---")?;
    let body = after.split_once('\n').map_or("", |(_, body)| body);
    Some((noyalib::from_str(yaml).ok()?, body.trim()))
}
