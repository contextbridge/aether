use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitDiff {
    pub diff: String,
    pub stats: DiffStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffStats {
    pub files_changed: usize,
    pub lines_added: usize,
    pub lines_removed: usize,
}

impl DiffStats {
    pub fn from_diff(diff: &str) -> Self {
        let mut lines_added = 0;
        let mut lines_removed = 0;
        let mut files_changed = 0;

        for line in diff.lines() {
            if line.starts_with("diff --git") {
                files_changed += 1;
            } else if line.starts_with('+') && !line.starts_with("+++") {
                lines_added += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                lines_removed += 1;
            }
        }

        Self { files_changed, lines_added, lines_removed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_stats_from_diff_counts_files_and_changed_lines() {
        let diff = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@\n-old\n+new\ndiff --git a/b.txt b/b.txt\n+++ b/b.txt\n+added\n";

        let stats = DiffStats::from_diff(diff);

        assert_eq!(stats.files_changed, 2);
        assert_eq!(stats.lines_added, 2);
        assert_eq!(stats.lines_removed, 1);
    }
}
