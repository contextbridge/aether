use tui::testing::{TestTerminal, render_lines};
use tui::{Line, MarkdownBlock, Theme, ViewContext, render_markdown_result, render_markdown_source_lines};
use unicode_width::UnicodeWidthStr;

fn ctx() -> ViewContext {
    ViewContext::new((80, 24))
}

fn themed_ctx(theme: &Theme) -> ViewContext {
    ViewContext::new_with_theme((80, 24), theme.clone())
}

fn render(md: &str) -> TestTerminal {
    let ctx = ctx();
    render_lines(&render_markdown_result(md, &ctx).to_lines(), 80, 24)
}

fn render_themed(md: &str, theme: &Theme) -> TestTerminal {
    let ctx = themed_ctx(theme);
    render_lines(&render_markdown_result(md, &ctx).to_lines(), 80, 24)
}

fn render_tall(md: &str) -> TestTerminal {
    let ctx = ViewContext::new((80, 100));
    render_lines(&render_markdown_result(md, &ctx).to_lines(), 80, 100)
}

fn render_tall_themed(md: &str, theme: &Theme) -> TestTerminal {
    let ctx = ViewContext::new_with_theme((80, 100), theme.clone());
    render_lines(&render_markdown_result(md, &ctx).to_lines(), 80, 100)
}

fn find_row(term: &TestTerminal, text: &str) -> Option<usize> {
    term.get_lines().iter().position(|line| line.contains(text))
}

fn all_text(term: &TestTerminal) -> String {
    term.get_lines().join("\n")
}

/// Assert the joined terminal output contains every needle.
fn assert_contains_all(term: &TestTerminal, needles: &[&str]) {
    let text = all_text(term);
    for needle in needles {
        assert!(text.contains(needle), "Expected to find {needle:?} in:\n{text}");
    }
}

fn row_inner_display_widths(row: &str) -> Vec<usize> {
    let segments: Vec<&str> = row.split('│').collect();
    if segments.len() < 3 {
        return Vec::new();
    }
    segments[1..segments.len() - 1].iter().map(|s| UnicodeWidthStr::width(*s)).collect()
}

#[test]
fn source_line_renderer_preserves_every_source_line() {
    let md = "# Title\n\nfirst\nsecond\n```rust\nlet x = 1;\n```";
    let result = render_markdown_source_lines(md, &ctx());
    let line_numbers: Vec<_> = result.lines.iter().map(|line| line.source_line_no).collect();

    assert_eq!(line_numbers, vec![1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn source_line_renderer_preserves_blank_lines() {
    let result = render_markdown_source_lines("first\n\nsecond", &ctx());

    assert_eq!(result.lines.len(), 3);
    assert!(result.lines[1].line.is_empty());
    assert_eq!(result.lines[1].source_line_no, 2);
}

#[test]
fn source_line_renderer_preserves_inline_styles_per_line() {
    let theme = Theme::default();
    let context = themed_ctx(&theme);
    let result = render_markdown_source_lines("alpha **bold** and `code`", &context);
    let term = render_lines(&result.lines.into_iter().map(|line| line.line).collect::<Vec<_>>(), 80, 24);

    assert_eq!(term.get_lines()[0].trim(), "alpha bold and code");
    assert!(term.style_of_text(0, "bold").unwrap().bold);
    assert_eq!(term.style_of_text(0, "code").unwrap().fg, Some(theme.code_fg()));
}

#[test]
fn source_line_renderer_keeps_fenced_code_delimiters() {
    let result = render_markdown_source_lines("```rust\nlet x = 1;\n```", &ctx());
    let lines: Vec<_> = result.lines.iter().map(|line| line.line.plain_text()).collect();

    assert_eq!(lines, vec!["```rust", "let x = 1;", "```"]);
    assert_eq!(result.lines.iter().map(|line| line.source_line_no).collect::<Vec<_>>(), vec![1, 2, 3]);
}

#[test]
fn source_line_renderer_keeps_table_source_rows() {
    let md = "| A | B |\n|---|---|\n| 1 | 2 |";
    let result = render_markdown_source_lines(md, &ctx());
    let lines: Vec<_> = result.lines.iter().map(|line| line.line.plain_text()).collect();

    assert_eq!(lines, vec!["| A | B |", "|---|---|", "| 1 | 2 |"]);
}

#[test]
fn source_line_renderer_maps_code_content_lines_to_source_lines() {
    let result = render_markdown_source_lines("intro\n```rust\nlet x = 1;\nlet y = 2;\n```", &ctx());
    let code_lines: Vec<_> = result
        .lines
        .iter()
        .filter(|line| line.line.plain_text().contains("let "))
        .map(|line| line.source_line_no)
        .collect();

    assert_eq!(code_lines, vec![3, 4]);
}

#[test]
fn plain_text_passes_through() {
    assert_eq!(render("hello world").get_lines()[0], "hello world");
}

#[test]
fn heading_renders_with_prefix_and_style() {
    let term = render("# Title");
    let row = find_row(&term, "# Title").expect("heading row not found");
    assert!(term.get_lines()[row].contains("# Title"));
    assert!(term.style_of_text(row, "Title").unwrap().bold);
}

fn find_block(blocks: &[MarkdownBlock], anchor: usize) -> &MarkdownBlock {
    blocks
        .iter()
        .find(|block| block.anchor_line_no == anchor)
        .unwrap_or_else(|| panic!("no block anchored at source line {anchor}; blocks={blocks:?}"))
}

#[test]
fn block_metadata_anchors_heading_and_paragraph_lines() {
    let md = "# Title\n\nParagraph with **bold**\nsecond line";
    let result = render_markdown_result(md, &ctx());

    let anchors: Vec<_> = result.blocks.iter().map(|block| block.anchor_line_no).collect();
    assert_eq!(anchors, vec![1, 3]);
    let heading_text = &result.lines[find_block(&result.blocks, 1).rendered_line_range.clone()];
    assert_eq!(heading_text.len(), 1);
    assert!(heading_text[0].line.plain_text().contains("Title"));
}

#[test]
fn block_metadata_excludes_blank_separator_rows() {
    let md = "first\n\nsecond";
    let result = render_markdown_result(md, &ctx());

    let blocks: Vec<_> = result.blocks.iter().map(|block| block.anchor_line_no).collect();
    assert_eq!(blocks, vec![1, 3]);

    for block in &result.blocks {
        for line in &result.lines[block.rendered_line_range.clone()] {
            assert!(!line.line.plain_text().is_empty(), "block should not include blank separator rows");
        }
    }
}

#[test]
fn block_metadata_tracks_list_items_individually() {
    let md = "- alpha\n- beta\n- gamma";
    let result = render_markdown_result(md, &ctx());

    let anchors: Vec<_> = result.blocks.iter().map(|block| block.anchor_line_no).collect();
    assert_eq!(anchors, vec![1, 2, 3]);
}

#[test]
fn block_metadata_treats_fenced_code_block_as_single_block() {
    let md = "intro\n\n```rust\nlet x = 1;\nlet y = 2;\n```";
    let result = render_markdown_result(md, &ctx());

    let code_block = find_block(&result.blocks, 3);
    let rendered_code: Vec<_> =
        result.lines[code_block.rendered_line_range.clone()].iter().map(|line| line.line.plain_text()).collect();
    assert!(rendered_code.iter().any(|text| text.contains("let x = 1")));
    assert!(rendered_code.iter().any(|text| text.contains("let y = 2")));
}

#[test]
fn block_metadata_treats_table_as_single_block() {
    let md = "paragraph\n\n| A | B |\n|---|---|\n| 1 | 2 |";
    let result = render_markdown_result(md, &ctx());

    let table_block = find_block(&result.blocks, 3);
    let rendered_table: Vec<_> =
        result.lines[table_block.rendered_line_range.clone()].iter().map(|line| line.line.plain_text()).collect();
    assert!(rendered_table.iter().any(|text| text.contains('│')));
}

#[test]
fn parse_only_heading_extraction_matches_renderer_headings() {
    let md = "# One\n\nbody\n\n## Two\nmore\n\n### Three";
    let rendered = render_markdown_result(md, &ctx());
    let extracted = tui::parse_markdown_headings(md);

    assert_eq!(extracted, rendered.headings);
    assert_eq!(extracted.len(), 3);
    assert_eq!(extracted[0].source_line_no, 1);
    assert_eq!(extracted[1].source_line_no, 5);
    assert_eq!(extracted[2].source_line_no, 8);
}

#[test]
fn display_render_wrapper_keeps_existing_output() {
    let md = "# Title\n\nfirst line\nsecond line\n\n- item";
    let rendered = render_markdown_result(md, &ctx()).to_lines();
    let text: Vec<_> = rendered.iter().map(Line::plain_text).collect();

    assert_eq!(
        text,
        vec![
            "# Title".to_string(),
            String::new(),
            "first line second line".to_string(),
            String::new(),
            "- item".to_string(),
        ]
    );
}

#[test]
fn inline_formatting_styles() {
    type StyleCase = (&'static str, &'static str, &'static str, fn(&tui::Style) -> bool);
    let cases: &[StyleCase] = &[
        ("some **bold** text", "some bold text", "bold", |s| s.bold),
        ("some *italic* text", "some italic text", "italic", |s| s.italic),
        ("some ~~struck~~ text", "some struck text", "struck", |s| s.strikethrough),
        ("***bold and italic***", "bold and italic", "bold and italic", |s| s.bold && s.italic),
    ];
    for (md, expected_text, styled_span, check) in cases {
        let term = render(md);
        assert_eq!(term.get_lines()[0].trim(), *expected_text, "md={md}");
        let style = term.style_of_text(0, styled_span).unwrap();
        assert!(check(&style), "style check failed for md={md}");
    }
}

#[test]
fn inline_code_has_code_style() {
    let theme = Theme::default();
    let term = render_themed("use `foo()` here", &theme);
    assert_eq!(term.get_lines()[0].trim(), "use foo() here");
    assert_eq!(term.style_of_text(0, "foo()").unwrap().fg, Some(theme.code_fg()));
}

#[test]
fn fenced_code_block_produces_lines() {
    assert!(all_text(&render("```rust\nfn main() {}\n```")).contains("fn main()"));
}

#[test]
fn list_items_render() {
    let cases: &[(&str, &[&str])] = &[
        ("- alpha\n- beta\n- gamma", &["- alpha", "- beta", "- gamma"]),
        ("1. first\n2. second\n3. third", &["1. first", "2. second", "3. third"]),
    ];
    for (md, expected) in cases {
        let term = render(md);
        let output = term.get_lines();
        for item in *expected {
            assert!(output.iter().any(|t| t.contains(item)), "Missing {item:?} in {md}");
        }
    }
}

#[test]
fn tight_list_items_are_not_double_spaced() {
    let term = render("1. first\n2. second\n3. third");

    let first_row = find_row(&term, "1. first").expect("first list item row not found");
    let second_row = find_row(&term, "2. second").expect("second list item row not found");
    let third_row = find_row(&term, "3. third").expect("third list item row not found");

    assert_eq!(second_row - first_row, 1);
    assert_eq!(third_row - second_row, 1);
}

#[test]
fn loose_list_item_with_nested_list_has_matching_spacing_before_next_sibling() {
    let term = render("1. first\n\n   - nested a\n   - nested b\n2. second");
    let output = term.get_lines();

    let first_row = find_row(&term, "1. first").expect("first list item row not found");
    let nested_first_item_row = find_row(&term, "nested a").expect("nested list item row not found");
    let nested_second_item_row = find_row(&term, "nested b").expect("nested list item row not found");
    let second_row = find_row(&term, "2. second").expect("second list item row not found");

    assert_eq!(nested_first_item_row - first_row, 2);
    assert!(output[first_row + 1].is_empty());
    assert_eq!(second_row - nested_second_item_row, 2);
    assert!(output[nested_second_item_row + 1].is_empty());
}

#[test]
fn tight_nested_list_keeps_next_sibling_compact() {
    let term = render("- outer\n  - inner\n- sibling");

    let inner_row = find_row(&term, "inner").expect("nested list item row not found");
    let sibling_row = find_row(&term, "sibling").expect("sibling list item row not found");

    assert_eq!(sibling_row - inner_row, 1);
}

#[test]
fn list_followed_by_paragraph_has_single_blank_row() {
    let term = render("- one\n- two\n\nafter");
    let output = term.get_lines();

    let one_row = find_row(&term, "- one").expect("first list item row not found");
    let two_row = find_row(&term, "- two").expect("second list item row not found");
    let after_row = find_row(&term, "after").expect("paragraph row not found");

    assert_eq!(two_row - one_row, 1);
    assert_eq!(after_row - two_row, 2);
    assert!(output[two_row + 1].is_empty());
}

#[test]
fn blockquote_is_indented() {
    let theme = Theme::default();
    let term = render_themed("> quoted text", &theme);
    let output = term.get_lines();
    let row = find_row(&term, "quoted text").expect("quoted text row not found");
    assert!(output[row].starts_with("  quoted text"));
    let style = term.style_of_text(row, "quoted text").unwrap();
    assert_eq!(style.fg, Some(theme.blockquote()));
    assert!(!style.dim);
}

#[test]
fn horizontal_rule() {
    assert!(all_text(&render("above\n\n---\n\nbelow")).contains("───"));
}

#[test]
fn link_is_underlined() {
    let theme = Theme::default();
    let term = render_themed("click [here](https://example.com)", &theme);
    let style = term.style_of_text(0, "here").unwrap();
    assert!(style.underline);
    assert_eq!(style.fg, Some(theme.link()));
}

#[test]
fn empty_input_returns_empty() {
    assert!(render_markdown_result("", &ctx()).to_lines().is_empty());
}

#[test]
fn multiple_paragraphs_have_single_blank_row() {
    let term = render("para one\n\npara two");
    let output = term.get_lines();

    let para_one_row = find_row(&term, "para one").expect("first paragraph row not found");
    let para_two_row = find_row(&term, "para two").expect("second paragraph row not found");

    assert_eq!(para_two_row - para_one_row, 2);
    assert!(output[para_one_row + 1].is_empty());
}

#[test]
fn unknown_language_falls_back_to_plain() {
    let theme = Theme::default();
    let term = render_themed("```nosuchlang\nsome code\n```", &theme);
    assert!(all_text(&term).contains("some code"));
    let row = find_row(&term, "some code").expect("code row not found");
    assert_eq!(term.style_of_text(row, "some code").unwrap().fg, Some(theme.code_fg()));
}

#[test]
fn nested_list_indents() {
    let output = render("- outer\n  - inner").get_lines();
    let inner = output.iter().find(|t| t.contains("inner")).unwrap();
    let outer = output.iter().find(|t| t.contains("outer")).unwrap();
    assert!(inner.len() - inner.trim_start().len() > outer.len() - outer.trim_start().len());
}

#[test]
fn highlight_cache_returns_consistent_results() {
    let ctx = ctx();
    let md = "```rust\nfn main() {}\n```";
    let first = render_markdown_result(md, &ctx).to_lines();
    let second = render_markdown_result(md, &ctx).to_lines();
    assert_eq!(first, second);

    let md2 = "```rust\nfn b() {}\n```";
    let lines2 = render_markdown_result(md2, &ctx).to_lines();
    assert_ne!(
        first.iter().map(Line::plain_text).collect::<String>(),
        lines2.iter().map(Line::plain_text).collect::<String>(),
    );
}

#[test]
fn simple_table_renders_correctly() {
    let md = "| Name | Age | City |\n|------|-----|------|\n| Alice | 30 | NYC |\n| Bob | 25 | LA |";
    let term = render_tall(md);
    let output = term.get_lines();
    let text = all_text(&term);
    let non_empty: Vec<&String> = output.iter().filter(|t| !t.is_empty()).collect();

    for ch in ['┌', '┐', '┬', '┼', '┴', '├', '┤', '└', '┘', '│'] {
        assert!(text.contains(ch), "Missing table char {ch:?}");
    }
    assert_eq!(output.iter().filter(|t| t.contains('┼')).count(), 1);
    assert_eq!(non_empty.len(), 6);
    assert_contains_all(&term, &["Alice", "30", "NYC", "Bob", "25", "LA"]);
    assert!(!output.iter().any(|t| t.trim() == "Alice"), "Table content leaked to standalone line");
}

#[test]
fn table_with_alignment() {
    let md = "| Left | Center | Right |\n|:-----|:------:|------:|\n| L | C | R |";
    assert_contains_all(&render_tall(md), &["Left", "Center", "Right"]);
}

#[test]
fn table_with_empty_cells() {
    let term = render_tall("| A | B | C |\n|---|---|---|\n| 1 |   | 3 |");
    let text = all_text(&term);
    assert!(text.contains('┌'));
    assert!(text.contains('1'));
    assert!(text.contains('3'));
}

#[test]
fn table_cell_inline_code_does_not_leak_line() {
    let term = render_tall("| A | B |\n|---|---|\n| `x` and **y** | z |");
    let output = term.get_lines();
    let non_empty: Vec<&String> = output.iter().filter(|t| !t.is_empty()).collect();

    assert_eq!(non_empty.len(), 5);
    assert!(!output.iter().any(|t| t.trim() == "x"), "Inline code leaked outside table");
    let body_row =
        output.iter().find(|t| t.starts_with('│') && t.contains("and y")).expect("Expected a rendered table body row");
    assert!(body_row.contains('x'));
}

#[test]
fn table_cell_preserves_inline_styles() {
    let theme = Theme::default();
    let md = "| A | B | C |\n|---|---|---|\n| **bold** | [link](https://example.com) | `code` |";
    let term = render_tall_themed(md, &theme);

    let bold_row = find_row(&term, "bold").expect("bold row not found");
    assert!(term.style_of_text(bold_row, "bold").unwrap().bold);

    let link_row = find_row(&term, "link").expect("link row not found");
    let link_style = term.style_of_text(link_row, "link").unwrap();
    assert!(link_style.underline);
    assert_eq!(link_style.fg, Some(theme.link()));

    let code_row = find_row(&term, "code").expect("code row not found");
    assert_eq!(term.style_of_text(code_row, "code").unwrap().fg, Some(theme.code_fg()));
}

#[test]
fn table_unicode_alignment_uses_display_width() {
    let md = "| Left | Right |\n|------|-------|\n| a | 你 |\n| bb | 😀 |";
    let output = render_tall(md).get_lines();
    let row_texts: Vec<&String> = output.iter().filter(|t| t.starts_with('│')).collect();

    assert!(row_texts.len() >= 3);
    let expected = row_inner_display_widths(row_texts[0]);
    for row in row_texts.iter().skip(1) {
        assert_eq!(row_inner_display_widths(row), expected);
    }
}

#[test]
fn table_row_cell_count_normalization() {
    let md = "| A | B | C |\n|---|---|---|\n| 1 | 2 |\n| 3 | 4 | 5 | 6 |";
    let output = render_tall(md).get_lines();
    let row_texts: Vec<&String> = output.iter().filter(|t| t.starts_with('│')).collect();

    assert_eq!(row_texts.len(), 3);
    for row in &row_texts {
        assert_eq!(row.matches('│').count(), 4);
        assert_eq!(row_inner_display_widths(row).len(), 3);
    }
}

#[test]
fn table_in_paragraph_context() {
    let md = "Here is a table:\n\n| Item | Price |\n|------|-------|\n| Apple | $1.00 |\n| Orange | $1.50 |\n\nThat's the table.";
    assert_contains_all(&render_tall(md), &["Here is a table:", "That's the table.", "Apple", "$1.00"]);
}

#[test]
fn table_single_column() {
    let term = render_tall("| Value |\n|--------|\n| Hello |");
    let text = all_text(&term);
    assert!(text.contains('┌'));
    assert!(text.contains('┐'));
    assert!(text.contains("Hello"));
}
