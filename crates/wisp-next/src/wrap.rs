use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub fn wrap_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    lines.into_iter().flat_map(|line| wrap_line(line, width)).collect()
}

pub fn wrap_line(line: Line<'static>, width: u16) -> Vec<Line<'static>> {
    let max_width = usize::from(width.max(1));
    let line_style = line.style;
    let alignment = line.alignment;
    let mut output = Vec::new();
    let mut spans = Vec::new();
    let mut current_width = 0;

    for span in line.spans {
        let mut fragment = String::new();
        for character in span.content.chars() {
            if character == '\n' {
                push_fragment(&mut spans, &mut fragment, span.style);
                output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                current_width = 0;
                continue;
            }
            if character == '\t' {
                let spaces = TAB_WIDTH - current_width % TAB_WIDTH;
                for _ in 0..spaces {
                    if current_width + 1 > max_width {
                        push_fragment(&mut spans, &mut fragment, span.style);
                        output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                        current_width = 0;
                    }
                    fragment.push(' ');
                    current_width += 1;
                }
                continue;
            }

            let character_width = character.width().unwrap_or(0);
            let (character, character_width) =
                if character_width > max_width { ('…', 1) } else { (character, character_width) };
            if current_width + character_width > max_width {
                push_fragment(&mut spans, &mut fragment, span.style);
                output.push(make_line(std::mem::take(&mut spans), line_style, alignment));
                current_width = 0;
            }
            fragment.push(character);
            current_width += character_width;
        }
        push_fragment(&mut spans, &mut fragment, span.style);
    }

    if !spans.is_empty() || output.is_empty() {
        output.push(make_line(spans, line_style, alignment));
    }
    output
}

const TAB_WIDTH: usize = 4;

fn push_fragment(spans: &mut Vec<Span<'static>>, fragment: &mut String, style: ratatui::style::Style) {
    if !fragment.is_empty() {
        spans.push(Span::styled(std::mem::take(fragment), style));
    }
}

fn make_line(
    spans: Vec<Span<'static>>,
    style: ratatui::style::Style,
    alignment: Option<ratatui::layout::Alignment>,
) -> Line<'static> {
    Line { spans, style, alignment }
}
