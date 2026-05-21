#![doc = include_str!("../README.md")]

// Core modules - always available
pub(crate) mod components;
pub(crate) mod diffs;
pub(crate) mod focus;
pub(crate) mod rendering;
pub(crate) use rendering::line;
pub(crate) use rendering::span;
pub(crate) use rendering::style;
pub(crate) mod theme;

// Feature-gated modules
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

// Core re-exports - always available
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
pub use diffs::diff_types::{DiffLine, DiffPreview, DiffTag, SplitDiffCell, SplitDiffRow};
pub use focus::{FocusOutcome, FocusRing};
pub use rendering::frame::{FitOptions, Frame, FramePart, Overflow};
pub use rendering::gutter::{digit_count, wrap_with_gutter};
pub use rendering::line::Line;
pub use rendering::render_context::{Insets, Size};
pub use rendering::style::Style;
pub use theme::{Theme, ThemeBuildError};

// Rendering (always available - no runtime dependency)
pub use rendering::renderer::{Renderer, RendererCommand};

// Runtime
pub use crossterm::event::Event as CrosstermEvent;
pub use runtime::terminal::terminal_size;
pub use runtime::{MouseCapture, TerminalConfig, TerminalRuntime, TerminalSession};

// &str text utilities
pub use rendering::soft_wrap::{
    display_width_text, pad_text_to_width, soft_wrap_line, soft_wrap_text_position, truncate_line, truncate_text,
};

// Span type
pub use rendering::span::Span;

// Markdown
#[cfg(feature = "syntax")]
pub use markdown::{
    MarkdownBlock, MarkdownHeading, MarkdownRenderResult, SourceMappedLine, parse_markdown_headings,
    render_markdown_result,
};

// Feature-gated re-exports
#[cfg(feature = "syntax")]
pub use diffs::diff::highlight_diff;

#[cfg(feature = "syntax")]
pub use diffs::split_diff::render_diff;
#[cfg(feature = "syntax")]
pub use diffs::split_diff::{
    MIN_GUTTER_WIDTH, SEPARATOR, SEPARATOR_WIDTH, Side as SplitDiffSide, gutter_width_for_preview,
    render_cell as split_render_cell,
};

#[cfg(feature = "syntax")]
pub use syntax_highlighting::SyntaxHighlighter;

#[cfg(feature = "picker")]
pub use combobox::{Combobox, PickerKey, classify_key};
#[cfg(feature = "picker")]
pub use fuzzy_matcher::Searchable;

#[cfg(feature = "picker")]
pub use fuzzy_matcher::FuzzyMatcher;

// Terminal event types (re-exported from crossterm)
pub use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind};
pub use crossterm::style::Color;
