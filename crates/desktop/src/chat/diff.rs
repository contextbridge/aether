use super::code::{span_styles, styled_text};
use clankerdiff_core::{
    DiffDocument, DiffError, DiffPresentation, DiffTone, PresentationOptions, RepoPath, RowKind, ViewMode,
    parse_git_diff_with_path_mapper,
};
use clankerdiff_gpui::style::color;
use clankerdiff_syntax::{Fingerprint, LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::ReviewTheme;
use gpui::{AnyElement, div, prelude::*};
use std::collections::VecDeque;
use std::sync::Arc;

/// A transcript's bounded, content-keyed cache of parsed patches, independent of the active theme.
#[derive(Default)]
pub(crate) struct DiffCache {
    entries: VecDeque<(Fingerprint, Result<DiffPresentation, DiffError>)>,
}

impl DiffCache {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// A read-only unified preview. Source comes exclusively from the historical patch.
    pub(crate) fn render(
        &mut self,
        patch: &str,
        theme: &ReviewTheme,
        highlighter: &mut SyntaxHighlighter,
    ) -> AnyElement {
        let id = Fingerprint::of([patch.as_bytes()]);
        let index = self.entries.iter().position(|(cached, _)| *cached == id).unwrap_or_else(|| {
            if self.entries.len() == MAX_CACHED_PATCHES {
                self.entries.pop_front();
            }
            self.entries.push_back((id, parse_patch(patch)));
            self.entries.len() - 1
        });
        match &self.entries[index].1 {
            Ok(presentation) => render_preview(presentation, theme, highlighter),
            Err(error) => div()
                .flex()
                .flex_col()
                .text_color(color(theme.diff.foreground))
                .child(div().text_color(color(theme.diff.muted)).child(format!("Could not display patch: {error}")))
                .child(styled_text(patch.to_owned(), Vec::new()))
                .into_any_element(),
        }
    }
}

const MAX_CACHED_PATCHES: usize = 32;

/// ACP patches may name absolute paths; normalize them only for this filesystem-free preview.
fn parse_patch(patch: &str) -> Result<DiffPresentation, DiffError> {
    let files = parse_git_diff_with_path_mapper(patch.as_bytes(), |path| {
        RepoPath::new(path.strip_prefix('/').unwrap_or(path))
    })?;
    let document = Arc::new(DiffDocument { repo_root: String::new(), files });
    Ok(DiffPresentation::new(
        document,
        PresentationOptions { view_mode: ViewMode::Unified, ..PresentationOptions::default() },
    ))
}

fn render_preview(
    presentation: &DiffPresentation,
    theme: &ReviewTheme,
    highlighter: &mut SyntaxHighlighter,
) -> AnyElement {
    let palette = &theme.diff;
    let mut view = div().flex().flex_col().min_w_0();
    for row in presentation.rows(0..presentation.row_count()) {
        if row.kind == RowKind::ExpandGap {
            continue;
        }
        let Some(cell) = row.primary_cell() else { continue };
        if !matches!(row.kind, RowKind::Code | RowKind::ExpandedContext) {
            view = view.child(
                div()
                    .px_1()
                    .text_color(color(if row.kind == RowKind::FileHeader { palette.foreground } else { palette.muted }))
                    .child(styled_text(cell.text.to_string(), Vec::new())),
            );
            continue;
        }
        let source = presentation.cell_context(row, cell);
        let highlights = highlighter
            .with_theme(&theme.syntax)
            .highlight_document(source.id, LanguageHint::Path(source.path), || source.text())
            .ok()
            .map(|document| span_styles(document.line(source.target_line).unwrap_or_default(), 0))
            .unwrap_or_default();
        let indicator = match cell.tone {
            DiffTone::Added => "+",
            DiffTone::Removed => "−",
            _ => " ",
        };
        let tone = palette.tone(cell.tone);
        view = view.child(
            div()
                .flex()
                .items_start()
                .min_w_0()
                .px_1()
                .bg(color(tone.background))
                .child(div().flex_none().text_color(color(tone.foreground)).child(indicator))
                .child(div().min_w_0().flex_1().text_color(color(palette.foreground)).child(styled_text(
                    if cell.text.is_empty() { " ".to_owned() } else { cell.text.to_string() },
                    highlights,
                ))),
        );
    }
    view.into_any_element()
}
