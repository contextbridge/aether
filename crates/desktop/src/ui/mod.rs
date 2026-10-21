//! The desktop app's visual language: a 4px spacing grid, one type scale set in Inter, Lucide
//! icons, and the controls built from them.
//!
//! Every view paints with these instead of picking its own sizes, so that the app lines up on
//! one grid and reads in one voice.

mod button;
mod empty_state;
mod icon;
mod list_item;
mod menu;
mod typography;

pub mod tokens;

pub use button::{Button, ButtonVariant, IconButton};
pub use empty_state::EmptyState;
pub use icon::{Icon, IconName};
pub use list_item::ListItem;
pub use menu::{MenuItem, with_menu};
pub use typography::{CODE_FONT, Typography, TypographyExt, UI_FONT, load_fonts};
