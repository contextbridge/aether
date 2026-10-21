//! Fixed measurements, all multiples of 4px.
//!
//! Padding and gaps use gpui's spacing scale, whose steps are 4px each (`p_1` is 4px, `p_2` is
//! 8px, ...). Only whole steps are used, never the half steps such as `p_0p5`.

/// A list row: a thread, a menu entry, a switcher result.
pub const ROW_HEIGHT: f32 = 28.0;
/// A group header above rows, such as a project in the sidebar.
pub const GROUP_HEIGHT: f32 = 32.0;
/// A bar across a pane: the title bar, the sidebar header and footer, a thread's header.
pub const BAR_HEIGHT: f32 = 40.0;
/// A square control holding one icon.
pub const ICON_BUTTON_SIZE: f32 = 24.0;
/// An icon in a control or beside text.
pub const ICON_SIZE: f32 = 16.0;
/// An icon inline with caption text.
pub const ICON_SIZE_SMALL: f32 = 12.0;
/// The slot in front of a row's title that holds its status dot.
pub const DOT_SLOT: f32 = 8.0;
pub const DOT_SIZE: f32 = 8.0;

/// Controls: buttons, rows, inputs.
pub const RADIUS_CONTROL: f32 = 4.0;
/// Containers: cards, popovers, modals.
pub const RADIUS_CONTAINER: f32 = 8.0;

/// The width a chat transcript and composer grow to, centred in wider windows.
pub const CONTENT_MAX_WIDTH: f32 = 768.0;
pub const SIDEBAR_WIDTH: f32 = 272.0;
pub const MENU_WIDTH: f32 = 224.0;
