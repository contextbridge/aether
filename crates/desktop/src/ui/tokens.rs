//! Fixed measurements, all multiples of 4px at the base interface size of 16px.
//!
//! Padding and gaps use gpui's spacing scale, whose steps are a quarter rem each (`p_1` is 4px at
//! the base size, `p_2` is 8px, ...). Only whole steps are used, never the half steps such as
//! `p_0p5`. Every measurement here goes through [`rem`] too, so the whole interface scales with
//! the user's interface size.

pub use clankerdiff_gpui::ui::tokens::*;

/// The slot in front of a row's title that holds its status dot.
pub const DOT_SLOT: f32 = 8.0;
pub const DOT_SIZE: f32 = 8.0;

/// The width a chat transcript and composer grow to, centred in wider windows.
pub const CONTENT_MAX_WIDTH: f32 = 768.0;
