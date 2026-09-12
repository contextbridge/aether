#![cfg(feature = "testing")]

use clankerdiff_ratatui::theme::{ReviewTheme, Rgba, SelectionState as ThemeSelectionState, ThemeError};
use clankerdiff_ratatui::{RatatuiUiTheme, composite_color, layered_style};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::StatefulWidget,
};
use wisp::{
    theme::Theme,
    view::{list_view::ListView, selection::SelectionState},
};

#[test]
fn sage_retains_semantic_status_and_text_roles() -> Result<(), ThemeError> {
    let theme = Theme::from_review(ReviewTheme::sage()?);
    assert_eq!(theme.info, Color::Rgb(130, 177, 204));
    assert_eq!(theme.warning, Color::Rgb(216, 181, 106));
    assert_eq!(theme.muted, Color::Rgb(92, 112, 104));
    assert_ne!(theme.text_secondary, theme.muted);
    Ok(())
}

#[test]
fn every_builtin_maps_semantic_roles_directly() -> Result<(), ThemeError> {
    for descriptor in ReviewTheme::catalog() {
        let review = ReviewTheme::builtin(&descriptor.id)?;
        let ui = review.ui;
        let theme = Theme::from_review(review);
        for (actual, expected) in [
            (theme.text_primary, ui.text),
            (theme.text_secondary, ui.text_secondary),
            (theme.muted, ui.text_muted),
            (theme.info, ui.info),
            (theme.warning, ui.warning),
            (theme.success, ui.positive),
            (theme.error, ui.destructive),
            (theme.surface, ui.surface),
        ] {
            assert_eq!(actual, composite_color(expected, ui.canvas), "{}", descriptor.id);
        }
    }
    Ok(())
}

#[test]
fn lists_use_shared_selection_instead_of_ordinary_surfaces() -> Result<(), ThemeError> {
    for id in ["sage", "dracula", "github-light"] {
        let theme = Theme::from_review(ReviewTheme::builtin(id)?);
        let native = RatatuiUiTheme::from(&theme.review().ui);
        for focused in [false, true] {
            let area = Rect::new(0, 0, 30, 2);
            let mut buffer = Buffer::empty(area);
            buffer.set_style(area, theme.surface_style());
            let rows = vec![
                Line::styled("selected", Style::new().fg(theme.info)),
                Line::styled("ordinary", Style::new().fg(theme.warning)),
            ];
            let list = ListView::new(rows, &theme);
            let list = if focused { list.pane("empty") } else { list };
            list.render(area, &mut buffer, &mut SelectionState::new(2));
            let selected = native.selection_style(if focused {
                ThemeSelectionState::Focused
            } else {
                ThemeSelectionState::Selected
            });
            assert_eq!(Some(buffer[(0, 0)].fg), selected.fg);
            assert_eq!(Some(buffer[(0, 0)].bg), selected.bg);
            assert_ne!(buffer[(0, 0)].bg, theme.surface);
            assert_eq!(buffer[(0, 1)].bg, theme.surface);
            assert_eq!(buffer[(0, 1)].fg, theme.warning);
        }
    }
    Ok(())
}

#[test]
fn ordinary_surfaces_composite_text_against_the_surface() {
    let mut review = ReviewTheme::default();
    review.ui.text = Rgba::new(200, 220, 240, 128);
    review.ui.surface = Rgba::new(50, 100, 150, 128);
    let expected = layered_style(review.ui.text, review.ui.surface, review.ui.canvas);
    assert_eq!(Theme::from_review(review).surface_style(), expected);
}
