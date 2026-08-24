use agent_client_protocol::schema::v1::{
    ContentBlock, EmbeddedResource, EmbeddedResourceResource, ResourceLink, TextResourceContents,
};
use serde::Serialize;
use specta::Type;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileEntry {
    pub(crate) path: String,
    pub(crate) display_name: String,
}

pub(crate) fn build_file_content_blocks(root: &Path, paths: &[String]) -> Result<Vec<ContentBlock>, String> {
    let canonical_root =
        root.canonicalize().map_err(|error| format!("failed to resolve working directory: {error}"))?;
    paths
        .iter()
        .map(|path| {
            let requested = PathBuf::from(path);
            let full_path = if requested.is_absolute() { requested } else { root.join(requested) };
            let canonical_path =
                full_path.canonicalize().map_err(|error| format!("failed to read mentioned file {path}: {error}"))?;
            if !canonical_path.starts_with(&canonical_root) {
                return Err(format!("mentioned file is outside the workspace: {path}"));
            }
            if !canonical_path.is_file() {
                return Err(format!("mentioned path is not a file: {path}"));
            }

            let mut bytes = Vec::new();
            File::open(&canonical_path)
                .map_err(|error| format!("failed to read mentioned file {path}: {error}"))?
                .take(1_048_577)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("failed to read mentioned file {path}: {error}"))?;
            let mime_type = mime_guess::from_path(&canonical_path).first_or_octet_stream().to_string();
            let uri = url::Url::from_file_path(&canonical_path)
                .map_err(|()| format!("failed to build file URI for {path}"))?
                .to_string();

            if bytes.len() > 1_048_576 {
                bytes.truncate(1_048_576);
            }
            match String::from_utf8(bytes) {
                Ok(text) => {
                    Ok(ContentBlock::Resource(EmbeddedResource::new(EmbeddedResourceResource::TextResourceContents(
                        TextResourceContents::new(text, uri).mime_type(mime_type),
                    ))))
                }
                Err(_) => Ok(ContentBlock::ResourceLink(ResourceLink::new(canonical_path.to_string_lossy(), uri))),
            }
        })
        .collect()
}

pub(crate) fn collect_workspace_files(root: &Path) -> Result<Vec<FileEntry>, String> {
    if !root.is_dir() {
        return Err(format!("working directory does not exist: {}", root.display()));
    }

    let mut entries = ignore::WalkBuilder::new(root)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .hidden(false)
        .parents(true)
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter(|entry| !excluded_path(entry.path()))
        .map(|entry| {
            let path = entry.into_path();
            let display_name = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            FileEntry { path: path.to_string_lossy().into_owned(), display_name }
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    entries.truncate(50_000);
    Ok(entries)
}

fn excluded_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component.as_os_str().to_string_lossy().as_ref(), ".git" | ".hg" | ".svn" | "node_modules" | "target")
    })
}
