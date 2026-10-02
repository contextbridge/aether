use super::common::uri_to_path;
use crate::workspace_paths::relative_path;
use lsp_types::Location;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLine {
    /// Absolute file path
    pub file_path: String,
    pub line: u32,
}

impl SourceLine {
    /// The start line of an LSP `Range` (0-indexed → 1-indexed).
    pub fn from_range(file_path: String, range: &lsp_types::Range) -> Self {
        Self { file_path, line: range.start.line + 1 }
    }

    pub fn from_location(location: &Location) -> Self {
        Self::from_range(uri_to_path(&location.uri), &location.range)
    }
}

pub async fn render_locations(locations: &[SourceLine], project_root: &Path, context_lines: Option<u32>) -> String {
    let mut groups: Vec<(&str, Vec<u32>)> = Vec::new();
    for location in locations {
        match groups.iter_mut().find(|(path, _)| *path == location.file_path) {
            Some((_, lines)) => lines.push(location.line),
            None => groups.push((&location.file_path, vec![location.line])),
        }
    }

    let mut context = SourceContext::new(context_lines);
    let mut rendered = Vec::new();
    for (path, mut lines) in groups {
        lines.sort_unstable();
        lines.dedup();
        let numbers: Vec<String> = lines.iter().map(u32::to_string).collect();
        rendered.push(format!("{}: {}", relative_path(project_root, path), numbers.join(", ")));
        if let Some(context) = context.as_mut()
            && let Some(source) = context.around(path, &lines).await
        {
            rendered.push(source);
        }
    }
    rendered.join("\n")
}

pub fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn extract_context(content: &str, anchor_lines: &[u32], context_lines: u32) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let mut anchors: Vec<u32> = anchor_lines.iter().copied().filter(|line| (1..=total).contains(line)).collect();
    anchors.sort_unstable();

    let mut windows: Vec<(u32, u32)> = Vec::new();
    for anchor in anchors {
        let from = anchor.saturating_sub(context_lines).max(1);
        let to = anchor.saturating_add(context_lines).min(total);
        match windows.last_mut() {
            Some((_, end)) if from <= *end + 1 => *end = (*end).max(to),
            _ => windows.push((from, to)),
        }
    }

    let width = windows.last().map_or(0, |(_, end)| end.to_string().len());
    windows
        .into_iter()
        .map(|(from, to)| {
            (from..=to).map(|number| format!("{number:>width$}\t{}", lines[number as usize - 1])).collect::<Vec<_>>()
        })
        .map(|window| window.join("\n"))
        .collect::<Vec<_>>()
        .join("\n--\n")
}

struct SourceContext {
    context_lines: u32,
    files: HashMap<String, Option<String>>,
}

impl SourceContext {
    fn new(context_lines: Option<u32>) -> Option<Self> {
        context_lines.filter(|lines| *lines > 0).map(|context_lines| Self { context_lines, files: HashMap::new() })
    }

    async fn around(&mut self, file_path: &str, anchor_lines: &[u32]) -> Option<String> {
        if !self.files.contains_key(file_path) {
            let content = tokio::fs::read_to_string(file_path).await.ok();
            self.files.insert(file_path.to_string(), content);
        }
        let content = self.files.get(file_path)?.as_deref()?;
        Some(extract_context(content, anchor_lines, self.context_lines)).filter(|context| !context.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_includes_lines_around_the_anchor() {
        let content = "line1\nline2\nline3\nline4\nline5\nline6\nline7";
        assert_eq!(extract_context(content, &[4], 1), "3\tline3\n4\tline4\n5\tline5");
    }

    #[test]
    fn context_clamps_to_the_file() {
        let content = "line1\nline2\nline3";
        assert_eq!(extract_context(content, &[1], 3), "1\tline1\n2\tline2\n3\tline3");
        assert_eq!(extract_context(content, &[3], 5), "1\tline1\n2\tline2\n3\tline3");
    }

    #[test]
    fn context_with_zero_lines_is_only_the_anchor() {
        assert_eq!(extract_context("a\nb\nc", &[2], 0), "2\tb");
    }

    #[test]
    fn context_skips_anchors_outside_the_file() {
        assert_eq!(extract_context("", &[1], 2), "");
        assert_eq!(extract_context("a\nb", &[0, 5], 2), "");
    }

    #[test]
    fn context_merges_overlapping_and_adjacent_windows_and_marks_gaps() {
        let content = (1..=12).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");

        assert_eq!(extract_context(&content, &[3, 2], 1), "1\tline1\n2\tline2\n3\tline3\n4\tline4");
        assert_eq!(extract_context(&content, &[2, 5], 1), "1\tline1\n2\tline2\n3\tline3\n4\tline4\n5\tline5\n6\tline6");
        assert_eq!(
            extract_context(&content, &[2, 10], 1),
            " 1\tline1\n 2\tline2\n 3\tline3\n--\n 9\tline9\n10\tline10\n11\tline11"
        );
    }

    #[tokio::test]
    async fn locations_without_context_are_grouped_by_relative_file_path() {
        let locations = [
            location("/project/src/main.rs", 7),
            location("/project/src/lib.rs", 2),
            location("/project/src/main.rs", 1),
            location("/elsewhere/dep.rs", 40),
        ];

        let rendered = render_locations(&locations, Path::new("/project"), None).await;

        assert_eq!(rendered, "src/main.rs: 1, 7\nsrc/lib.rs: 2\n/elsewhere/dep.rs: 40");
    }

    #[tokio::test]
    async fn locations_on_the_same_line_are_listed_once() {
        let locations = [location("/project/src/main.rs", 5), location("/project/src/main.rs", 5)];

        let rendered = render_locations(&locations, Path::new("/project"), None).await;

        assert_eq!(rendered, "src/main.rs: 5");
    }

    #[tokio::test]
    async fn location_context_follows_each_file_group() {
        let dir = tempfile::tempdir().unwrap();
        let main = numbered_file(dir.path(), "main.rs", 20);
        let lib = numbered_file(dir.path(), "lib.rs", 20);
        let locations = [location(&main, 6), location(&lib, 2), location(&main, 7)];

        let rendered = render_locations(&locations, dir.path(), Some(1)).await;

        assert_eq!(
            rendered,
            [
                "main.rs: 6, 7",
                "5\tline5",
                "6\tline6",
                "7\tline7",
                "8\tline8",
                "lib.rs: 2",
                "1\tline1",
                "2\tline2",
                "3\tline3"
            ]
            .join("\n")
        );
    }

    #[tokio::test]
    async fn locations_in_unreadable_files_get_no_context() {
        let rendered = render_locations(&[location("/does/not/exist.rs", 1)], Path::new("/project"), Some(2)).await;

        assert_eq!(rendered, "/does/not/exist.rs: 1");
    }

    #[test]
    fn source_lines_start_at_the_range_start() {
        let range = lsp_types::Range {
            start: lsp_types::Position { line: 2, character: 4 },
            end: lsp_types::Position { line: 15, character: 1 },
        };

        assert_eq!(SourceLine::from_range("/a.rs".to_string(), &range), location("/a.rs", 3));
    }

    #[test]
    fn single_line_collapses_whitespace() {
        assert_eq!(single_line("mismatched types\n  expected `u32`"), "mismatched types expected `u32`");
    }

    fn location(file_path: &str, line: u32) -> SourceLine {
        SourceLine { file_path: file_path.to_string(), line }
    }

    fn numbered_file(dir: &Path, name: &str, line_count: u32) -> String {
        let path = dir.join(name);
        let content = (1..=line_count).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
        std::fs::write(&path, content).unwrap();
        path.to_string_lossy().to_string()
    }
}
