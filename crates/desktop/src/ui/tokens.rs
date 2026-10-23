//! Fixed measurements, all multiples of 4px at the base interface size of 16px.
//!
//! Padding and gaps use gpui's spacing scale, whose steps are a quarter rem each (`p_1` is 4px at
//! the base size, `p_2` is 8px, ...). Only whole steps are used, never the half steps such as
//! `p_0p5`. Every measurement here goes through [`rem`] too, so the whole interface scales with
//! the user's interface size.

pub use clankerdiff_gpui::ui::tokens::*;

pub const UNREAD_DOT_SIZE: f32 = 8.0;

/// The width a chat transcript and composer grow to, centred in wider windows.
pub const CONTENT_MAX_WIDTH: f32 = 768.0;
