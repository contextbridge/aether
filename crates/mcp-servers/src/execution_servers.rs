use crate::{CodingMcp, CodingMcpArgs, DefaultCodingTools, ReviewMcp, SkillsMcp};
use crate::{coding::tools::bash::BashEnvironment, error::ServerInitError, workspace_paths::resolve_path};
use std::path::Path;

/// Construct workspace tools without agent runtime dependencies or argument fallbacks.
pub fn coding(args: Vec<String>, root: &Path, environment: BashEnvironment) -> Result<CodingMcp, ServerInitError> {
    let CodingMcpArgs { permission_mode, rules_dirs, disable_lsp, root_dir } = CodingMcpArgs::from_args(args)?;
    let root = root_dir.map_or_else(|| root.to_path_buf(), |path| resolve_path(root, path));
    let rules_dirs = rules_dirs.into_iter().map(|path| resolve_path(&root, path)).collect();
    let tools = DefaultCodingTools::new().with_bash_environment(environment);
    let server = CodingMcp::with_tools(tools)
        .with_rules_dirs(rules_dirs)
        .with_root_dir(root.clone())
        .with_permission_mode(permission_mode);
    Ok(if disable_lsp { server } else { server.with_lsp(root) })
}

pub fn skills(args: Vec<String>, root: &Path) -> Result<SkillsMcp, ServerInitError> {
    SkillsMcp::from_args_with_base_dir(args, root)
}

pub fn review(args: Vec<String>, root: &Path) -> Result<ReviewMcp, ServerInitError> {
    ReviewMcp::from_args_with_base_dir(args, root)
}
