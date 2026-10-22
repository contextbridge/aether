use super::tokens::rem;
use gpui::{App, FontWeight, Styled};
use std::borrow::Cow;

/// The family the interface is set in.
pub const UI_FONT: &str = "Inter";
/// The family code, tool output and diffs are set in.
pub const CODE_FONT: &str = clankerdiff_gpui::DEFAULT_FONT_FAMILY;

/// The steps of the type scale. Each has a size and a line height on the 4px grid, and a
/// weight of 400, 500 or 600.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typography {
    /// Timestamps, counts, shortcut hints.
    Caption,
    /// Metadata and secondary lines.
    Small,
    /// Group labels, such as a project's name above its threads.
    Label,
    /// Rows, prose and buttons.
    Body,
    /// Emphasised body text, such as a row's label in a strong position.
    BodyStrong,
    /// A thread's title, a modal's title.
    Title,
    Heading,
    Display,
}

impl Typography {
    pub fn size(self) -> f32 {
        match self {
            Self::Caption => 11.0,
            Self::Small | Self::Label => 12.0,
            Self::Body | Self::BodyStrong => 13.0,
            Self::Title => 14.0,
            Self::Heading => 16.0,
            Self::Display => 20.0,
        }
    }

    pub fn line_height(self) -> f32 {
        match self {
            Self::Caption | Self::Small | Self::Label => 16.0,
            Self::Body | Self::BodyStrong | Self::Title => 20.0,
            Self::Heading => 24.0,
            Self::Display => 28.0,
        }
    }

    pub fn weight(self) -> FontWeight {
        match self {
            Self::Caption | Self::Small | Self::Body => FontWeight::NORMAL,
            Self::Label | Self::BodyStrong | Self::Title => FontWeight::MEDIUM,
            Self::Heading | Self::Display => FontWeight::SEMIBOLD,
        }
    }
}

/// Sets an element's text in a step of the type scale.
pub trait TypographyExt: Styled + Sized {
    fn typography(self, typography: Typography) -> Self {
        self.text_size(rem(typography.size()))
            .line_height(rem(typography.line_height()))
            .font_weight(typography.weight())
    }
}

impl<T: Styled> TypographyExt for T {}

/// Registers the bundled Inter faces with the text system.
///
/// # Errors
///
/// Fails if the text system rejects a face.
pub fn load_fonts(cx: &App) -> gpui::Result<()> {
    cx.text_system().add_fonts(INTER.iter().map(|face| Cow::Borrowed(*face)).collect())
}

const INTER: [&[u8]; 6] = [
    include_bytes!("../../assets/fonts/inter/Inter-Regular.ttf"),
    include_bytes!("../../assets/fonts/inter/Inter-Italic.ttf"),
    include_bytes!("../../assets/fonts/inter/Inter-Medium.ttf"),
    include_bytes!("../../assets/fonts/inter/Inter-SemiBold.ttf"),
    include_bytes!("../../assets/fonts/inter/Inter-Bold.ttf"),
    include_bytes!("../../assets/fonts/inter/Inter-BoldItalic.ttf"),
];
