use crate::coding::error::GlobError;
use globset::{Glob, GlobSet, GlobSetBuilder};

pub fn build_glob_set(glob: Option<&str>) -> Result<Option<GlobSet>, GlobError> {
    let Some(glob) = glob else {
        return Ok(None);
    };

    let mut builder = GlobSetBuilder::new();
    builder.add(
        Glob::new(glob).map_err(|e| GlobError::InvalidPattern { pattern: glob.to_owned(), reason: e.to_string() })?,
    );
    builder.build().map(Some).map_err(|e| GlobError::BuildFailed(e.to_string()))
}
