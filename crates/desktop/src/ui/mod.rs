//! The desktop app's visual language: clankerdiff's shared design system (a 4px spacing grid,
//! one type scale set in Inter, Lucide icons, and the components built from them), plus the few
//! measurements only this app needs.
//!
//! Every view paints with these instead of picking its own sizes, so that the app lines up on
//! one grid and reads in one voice with the diff viewer it embeds.

pub mod tokens;

pub use clankerdiff_gpui::ui::prelude::*;
pub use clankerdiff_gpui::ui::{InterfaceSize, fonts::load_fonts, interface_size, window_chrome};

/// Plain text with the same paint observation as code text in the desktop test harness.
pub(crate) fn plain_text(text: impl Into<gpui::SharedString>) -> gpui::AnyElement {
    use gpui::IntoElement;

    let text = text.into();
    #[cfg(feature = "testing")]
    {
        crate::testing::observe_text(text, Vec::new()).into_any_element()
    }
    #[cfg(not(feature = "testing"))]
    {
        text.into_any_element()
    }
}
