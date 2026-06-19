mod headings;
mod renderer;
mod source_lines;
mod source_map;
mod table;

use pulldown_cmark::Options;

pub use headings::{MarkdownHeading, parse_markdown_headings};
pub use renderer::{MarkdownBlock, MarkdownRenderResult, SourceMappedLine, render_markdown_result};
pub use source_lines::{SourceMarkdownLine, SourceMarkdownRenderResult, render_markdown_source_lines};

pub(super) fn pulldown_options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES
}
