#![doc = include_str!("../README.md")]

pub(crate) mod components;
pub(crate) mod diffs;
pub(crate) mod focus;
pub(crate) mod rendering;
pub(crate) use rendering::line;
pub(crate) use rendering::span;
pub(crate) use rendering::style;
pub(crate) mod terminal_codes;
pub(crate) mod theme;

#[cfg(feature = "syntax")]
mod syntax_highlighting;

#[cfg(feature = "syntax")]
pub(crate) mod markdown;

#[cfg(all(feature = "picker", feature = "testing"))]
pub mod test_picker;

#[cfg(feature = "picker")]
pub(crate) mod combobox;

#[cfg(feature = "picker")]
pub(crate) mod fuzzy_matcher;

pub(crate) mod runtime;

#[cfg(feature = "testing")]
pub mod testing;

pub use components::bordered_text_field::BorderedTextField;
pub use components::checkbox::Checkbox;
pub use components::form::{Form, FormField, FormFieldKind, FormMessage};
pub use components::gallery::{Gallery, GalleryMessage};
pub use components::multi_select::MultiSelect;
pub use components::number_field::NumberField;
pub use components::panel::{BORDER_H_PAD, Panel};
pub use components::radio_select::RadioSelect;
pub use components::select_list::{SelectItem, SelectList, SelectListMessage};
pub use components::select_option::SelectOption;
pub use components::spinner::{BRAILLE_FRAMES, Spinner};
pub use components::split_panel::{Either, SplitLayout, SplitPanel, SplitWidths};
pub use components::stepper::{StepVisualState, Stepper, StepperItem};
pub use components::text_field::TextField;

pub use components::{Component, Cursor, Event, PickerMessage, ViewContext, merge, wrap_selection};
pub use diffs::diff_types::{DiffLine, DiffPreview, DiffTag, SplitDiffEntry, SplitDiffRow};
pub use focus::{FocusOutcome, FocusRing};
pub use rendering::frame::{FitOptions, Frame, FramePart, Overflow};
pub use rendering::gutter::{digit_count, wrap_with_gutter};
pub use rendering::line::Line;
pub use rendering::render_context::{Insets, Size};
pub use rendering::style::Style;
pub use theme::{Theme, ThemeBuildError};

pub use rendering::renderer::{Renderer, RendererCommand};

pub use crossterm::event::Event as CrosstermEvent;
pub use runtime::terminal::terminal_size;
pub use runtime::{MouseCapture, TerminalConfig, TerminalRuntime, TerminalSession};

pub use rendering::soft_wrap::{
    display_width_text, pad_text_to_width, soft_wrap_line, soft_wrap_text_position, truncate_line, truncate_text,
};

pub use rendering::span::Span;

#[cfg(feature = "syntax")]
pub use markdown::{
    MarkdownBlock, MarkdownHeading, MarkdownRenderResult, SourceMappedLine, SourceMarkdownLine,
    SourceMarkdownRenderResult, parse_markdown_headings, render_markdown_result, render_markdown_source_lines,
};

#[cfg(feature = "syntax")]
pub use diffs::diff::highlight_diff;

#[cfg(feature = "syntax")]
pub use diffs::split_diff::render_diff;
#[cfg(feature = "syntax")]
pub use diffs::split_diff::{
    GutterTint, MIN_GUTTER_WIDTH, MIN_SPLIT_WIDTH, SEPARATOR, SEPARATOR_WIDTH, Side as SplitDiffSide,
    SplitLayoutDimensions, WRAP_MARKER, header_left_padding, render_entry as split_render_entry,
    usize_to_u16_saturating,
};

#[cfg(feature = "syntax")]
pub use syntax_highlighting::SyntaxHighlighter;

#[cfg(feature = "picker")]
pub use combobox::{Combobox, PickerKey, classify_key};
#[cfg(feature = "picker")]
pub use fuzzy_matcher::Searchable;

#[cfg(feature = "picker")]
pub use fuzzy_matcher::FuzzyMatcher;

pub use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
pub use crossterm::style::Color;
