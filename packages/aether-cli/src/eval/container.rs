use crate::error::CliError;
use crate::sandbox;
use crucible::{EnvironmentSpec, SpecReport};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

/// Run a single eval spec inside its declared container environment.
///
/// Builds (or resolves) the image, then invokes `aether eval --single … --output
/// json` inside it with the project mounted at `/workspace`, and parses the
/// resulting [`SpecReport`] from stdout. The image must contain the `aether`
/// binary as its entrypoint (the same contract as `--sandbox-image`).
pub fn run_spec_in_container(
    spec_file: &Path,
    environment: &EnvironmentSpec,
    cwd: &Path,
    base_dir: &Path,
) -> Result<SpecReport, CliError> {
    let image = resolve_image(environment, base_dir)?;
    let aether_home = sandbox::aether_home().map_err(|error| CliError::Eval(error.to_string()))?;
    let env_vars = sandbox::forwarded_vars();

    let relative = spec_file.strip_prefix(cwd).unwrap_or(spec_file);
    let container_spec = Path::new("/workspace").join(relative);
    let args = [
        "eval".to_string(),
        "--single".to_string(),
        container_spec.display().to_string(),
        "--output".to_string(),
        "json".to_string(),
        "-C".to_string(),
        "/workspace".to_string(),
    ];

    let output = sandbox::run_in_container(&image, cwd, &aether_home, &env_vars, &args)
        .map_err(|error| CliError::Eval(error.to_string()))?;

    if !output.status.success() {
        return Err(CliError::Eval(format!(
            "container eval failed (exit {:?}): {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    serde_json::from_slice(&output.stdout)
        .map_err(|error| CliError::Eval(format!("could not parse container eval output: {error}")))
}

fn resolve_image(environment: &EnvironmentSpec, base_dir: &Path) -> Result<String, CliError> {
    match environment {
        EnvironmentSpec::Image { image } => Ok(image.clone()),
        EnvironmentSpec::Dockerfile { dockerfile } => {
            let path = base_dir.join(dockerfile);
            let context = path.parent().unwrap_or(base_dir).to_path_buf();
            let tag = format!("aether-eval-{:016x}", stable_hash(&path));
            sandbox::build_image(&tag, &path, &context).map_err(|error| CliError::Eval(error.to_string()))?;
            Ok(tag)
        }
    }
}

fn stable_hash(path: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}
