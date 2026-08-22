use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

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
        let outline = parse_headings(markdown);
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownHeading {
    title: String,
    level: u8,
    source_line_no: usize,
}

fn parse_headings(text: &str) -> Vec<PlanSection> {
    let mut headings: Vec<MarkdownHeading> = Vec::new();
    let mut line_starts = vec![0usize];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            line_starts.push(index + 1);
        }
    }

    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let parser = Parser::new_ext(text, options).into_offset_iter();

    let mut active: Option<(u8, usize, String)> = None;
    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                let line_no = line_starts.partition_point(|ls| *ls <= range.start).max(1);
                active = Some((level as u8, line_no, String::new()));
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, line_no, title)) = active.take() {
                    let title = title.trim().to_string();
                    if !title.is_empty() {
                        headings.push(MarkdownHeading { title, level, source_line_no: line_no });
                    }
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, title)) = active.as_mut() {
                    title.push_str(&text);
                }
            }
            _ => {}
        }
    }

    headings
        .into_iter()
        .map(|h| PlanSection { title: h.title, level: h.level, first_line_no: h.source_line_no })
        .collect()
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
