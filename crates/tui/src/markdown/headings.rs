use pulldown_cmark::{Event, Parser, Tag, TagEnd};

use super::pulldown_options;
use super::source_map::SourceMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownHeading {
    pub title: String,
    pub level: u8,
    pub source_line_no: usize,
}

pub fn parse_markdown_headings(text: &str) -> Vec<MarkdownHeading> {
    let source = SourceMap::new(text);
    let parser = Parser::new_ext(text, pulldown_options()).into_offset_iter();
    let mut headings = Vec::new();
    let mut active: Option<ActiveHeading> = None;

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                active = Some(ActiveHeading {
                    level: level as u8,
                    source_line_no: source.line_no_for_start(&range),
                    title: String::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(active) = active.take() {
                    let title = active.title.trim().to_string();
                    if !title.is_empty() {
                        headings.push(MarkdownHeading {
                            title,
                            level: active.level,
                            source_line_no: active.source_line_no,
                        });
                    }
                }
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(active) = active.as_mut() {
                    active.title.push_str(&text);
                }
            }
            _ => {}
        }
    }

    headings
}

struct ActiveHeading {
    level: u8,
    source_line_no: usize,
    title: String,
}
