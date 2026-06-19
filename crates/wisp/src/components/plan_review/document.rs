use tui::{MarkdownHeading, parse_markdown_headings};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanDocument {
    pub path: String,
    pub lines: Vec<PlanSourceLine>,
    pub outline: Vec<PlanSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSourceLine {
    pub line_no: usize,
    pub text: String,
    pub section_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSection {
    pub title: String,
    pub level: u8,
    pub first_line_no: usize,
}

impl PlanDocument {
    pub fn parse(path: impl Into<String>, markdown: &str) -> Self {
        let outline = parse_markdown_headings(markdown).into_iter().map(PlanSection::from).collect::<Vec<_>>();
        let mut lines = markdown
            .split('\n')
            .enumerate()
            .map(|(index, raw_line)| PlanSourceLine {
                line_no: index + 1,
                text: raw_line.trim_end_matches('\r').to_string(),
                section_index: None,
            })
            .collect::<Vec<_>>();

        assign_section_indices(&mut lines, &outline);

        Self { path: path.into(), lines, outline }
    }

    pub fn section_title_for(&self, line: &PlanSourceLine) -> Option<&str> {
        line.section_index.and_then(|index| self.outline.get(index)).map(|section| section.title.as_str())
    }

    pub fn markdown_text(&self) -> String {
        self.lines.iter().map(|line| line.text.as_str()).collect::<Vec<_>>().join("\n")
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn line_by_no(&self, line_no: usize) -> Option<&PlanSourceLine> {
        line_no.checked_sub(1).and_then(|index| self.lines.get(index))
    }
}

fn assign_section_indices(lines: &mut [PlanSourceLine], outline: &[PlanSection]) {
    let mut outline_index = 0;
    let mut current_section: Option<usize> = None;

    for line in lines {
        while let Some(section) = outline.get(outline_index)
            && section.first_line_no <= line.line_no
        {
            current_section = Some(outline_index);
            outline_index += 1;
        }

        line.section_index = current_section;
    }
}

impl From<MarkdownHeading> for PlanSection {
    fn from(value: MarkdownHeading) -> Self {
        Self { title: value.title, level: value.level, first_line_no: value.source_line_no }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_preserves_source_line_numbers() {
        let document = PlanDocument::parse("plan.md", "# Title\n\n- item\nparagraph");

        let line_numbers: Vec<_> = document.lines.iter().map(|line| line.line_no).collect();
        assert_eq!(line_numbers, vec![1, 2, 3, 4]);
    }

    #[test]
    fn parse_builds_outline_from_headings() {
        let document = PlanDocument::parse("plan.md", "# Top\n## Child\ntext");

        assert_eq!(document.outline.len(), 2);
        assert_eq!(document.outline[0].title, "Top");
        assert_eq!(document.outline[0].first_line_no, 1);
        assert_eq!(document.outline[1].title, "Child");
        assert_eq!(document.outline[1].first_line_no, 2);
    }

    #[test]
    fn parse_preserves_raw_source_lines_for_feedback() {
        let document = PlanDocument::parse("plan.md", "# Intro\n`inline` and **bold**\n```rust");

        assert_eq!(document.lines[1].text, "`inline` and **bold**");
        assert_eq!(document.lines[2].text, "```rust");
        assert_eq!(document.markdown_text(), "# Intro\n`inline` and **bold**\n```rust");
    }

    #[test]
    fn parse_tracks_active_section_title_for_lines() {
        let document = PlanDocument::parse("plan.md", "# Intro\nline\n## Details\nmore");

        assert_eq!(document.section_title_for(&document.lines[0]), Some("Intro"));
        assert_eq!(document.section_title_for(&document.lines[1]), Some("Intro"));
        assert_eq!(document.section_title_for(&document.lines[2]), Some("Details"));
        assert_eq!(document.section_title_for(&document.lines[3]), Some("Details"));
    }

    #[test]
    fn line_by_no_returns_line_when_present() {
        let document = PlanDocument::parse("plan.md", "first\nsecond");
        let line = document.line_by_no(2).expect("line exists");
        assert_eq!(line.text, "second");
    }
}
