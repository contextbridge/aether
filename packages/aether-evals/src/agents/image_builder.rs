use futures::{StreamExt, TryStreamExt};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use thiserror::Error;
use tokio::process::Command;

/// A request to build a Docker image from a Dockerfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBuildRequest {
    pub dockerfile: PathBuf,
    pub context: PathBuf,
    pub tag: String,
}

#[derive(Debug, Error)]
pub enum ImageBuildError {
    #[error("failed to spawn `docker build`: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("`docker build -t {tag}` failed{}:\n{output_tail}", match code {
        Some(code) => format!(" with exit code {code}"),
        None => String::new(),
    })]
    Failed { tag: String, code: Option<i32>, output_tail: String },

    #[error("multiple builds target the image tag `{tag}` with different Dockerfiles or contexts")]
    ConflictingTag { tag: String },
}

/// Build the requested images concurrently (bounded by `max_concurrency`), deduplicating
/// identical requests. Two requests with the same tag but a different Dockerfile or context are
/// rejected before anything builds, since the second build would silently replace the first
/// image.
pub async fn build_images(
    requests: impl IntoIterator<Item = ImageBuildRequest>,
    max_concurrency: NonZeroUsize,
) -> Result<(), ImageBuildError> {
    let mut requests_by_tag: BTreeMap<String, ImageBuildRequest> = BTreeMap::new();
    for request in requests {
        match requests_by_tag.get(&request.tag) {
            Some(existing) if existing != &request => {
                return Err(ImageBuildError::ConflictingTag { tag: request.tag });
            }
            _ => {
                requests_by_tag.insert(request.tag.clone(), request);
            }
        }
    }

    futures::stream::iter(requests_by_tag.into_values().map(build_image))
        .buffer_unordered(max_concurrency.get())
        .try_collect::<Vec<()>>()
        .await
        .map(|_| ())
}

const BUILD_OUTPUT_TAIL_CHARS: usize = 8_000;

/// Build one image. Output is captured rather than streamed so concurrent builds do not
/// interleave; on failure the tail of the build output is attached to the error.
async fn build_image(request: ImageBuildRequest) -> Result<(), ImageBuildError> {
    let output = Command::new("docker")
        .args(["build", "-t", &request.tag, "-f"])
        .arg(&request.dockerfile)
        .arg(&request.context)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(ImageBuildError::Spawn)?;

    if output.status.success() {
        return Ok(());
    }

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    Err(ImageBuildError::Failed {
        tag: request.tag,
        code: output.status.code(),
        output_tail: tail_chars(&combined, BUILD_OUTPUT_TAIL_CHARS),
    })
}

fn tail_chars(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    if total <= max_chars {
        return value.to_string();
    }

    let tail: String = value.chars().skip(total - max_chars).collect();
    format!("[truncated]...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_conflicting_builds_for_the_same_tag() {
        let requests = [
            ImageBuildRequest { dockerfile: "a/Dockerfile".into(), context: "a".into(), tag: "same:tag".into() },
            ImageBuildRequest { dockerfile: "b/Dockerfile".into(), context: "b".into(), tag: "same:tag".into() },
        ];

        let error = build_images(requests, NonZeroUsize::new(2).unwrap()).await.unwrap_err();

        assert!(matches!(error, ImageBuildError::ConflictingTag { tag } if tag == "same:tag"));
    }

    #[test]
    fn tail_chars_keeps_the_end_of_long_output() {
        assert_eq!(tail_chars("short", 10), "short");
        assert_eq!(tail_chars("abcdefghij", 3), "[truncated]...hij");
    }
}
