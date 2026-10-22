use super::tokens::{ICON_SIZE, rem};
use gpui::{
    Animation, AnimationExt, App, Hsla, IntoElement, RenderOnce, SharedString, Styled, Transformation, Window,
    prelude::*, radians, svg,
};
use std::f32::consts::TAU;
use std::time::Duration;

/// The Lucide icons the app uses, bundled into the binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconName {
    Archive,
    ArchiveRestore,
    ArrowUp,
    ArrowDown,
    Check,
    ChevronDown,
    ChevronRight,
    Circle,
    CircleCheck,
    CircleDot,
    CircleSlash,
    CircleX,
    Copy,
    Ellipsis,
    FolderPlus,
    Info,
    LoaderCircle,
    Minus,
    Pencil,
    Pin,
    PinOff,
    Plus,
    RotateCw,
    Search,
    Square,
    SquarePen,
    X,
}

impl IconName {
    fn svg(self) -> &'static [u8] {
        match self {
            Self::Archive => include_bytes!("../../assets/icons/archive.svg"),
            Self::ArchiveRestore => include_bytes!("../../assets/icons/archive-restore.svg"),
            Self::ArrowDown => include_bytes!("../../assets/icons/arrow-down.svg"),
            Self::ArrowUp => include_bytes!("../../assets/icons/arrow-up.svg"),
            Self::Check => include_bytes!("../../assets/icons/check.svg"),
            Self::ChevronDown => include_bytes!("../../assets/icons/chevron-down.svg"),
            Self::ChevronRight => include_bytes!("../../assets/icons/chevron-right.svg"),
            Self::Circle => include_bytes!("../../assets/icons/circle.svg"),
            Self::CircleSlash => include_bytes!("../../assets/icons/circle-slash.svg"),
            Self::CircleDot => include_bytes!("../../assets/icons/circle-dot.svg"),
            Self::CircleCheck => include_bytes!("../../assets/icons/circle-check.svg"),
            Self::CircleX => include_bytes!("../../assets/icons/circle-x.svg"),
            Self::Copy => include_bytes!("../../assets/icons/copy.svg"),
            Self::Ellipsis => include_bytes!("../../assets/icons/ellipsis.svg"),
            Self::Info => include_bytes!("../../assets/icons/info.svg"),
            Self::FolderPlus => include_bytes!("../../assets/icons/folder-plus.svg"),
            Self::LoaderCircle => include_bytes!("../../assets/icons/loader-circle.svg"),
            Self::Minus => include_bytes!("../../assets/icons/minus.svg"),
            Self::Pencil => include_bytes!("../../assets/icons/pencil.svg"),
            Self::Pin => include_bytes!("../../assets/icons/pin.svg"),
            Self::PinOff => include_bytes!("../../assets/icons/pin-off.svg"),
            Self::Plus => include_bytes!("../../assets/icons/plus.svg"),
            Self::RotateCw => include_bytes!("../../assets/icons/rotate-cw.svg"),
            Self::Search => include_bytes!("../../assets/icons/search.svg"),
            Self::Square => include_bytes!("../../assets/icons/square.svg"),
            Self::SquarePen => include_bytes!("../../assets/icons/square-pen.svg"),
            Self::X => include_bytes!("../../assets/icons/x.svg"),
        }
    }
}

/// An icon, 16px unless sized otherwise, painted in `color`.
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: f32,
    color: Option<Hsla>,
    hover: Option<(SharedString, Hsla)>,
    spin: Option<SharedString>,
}

impl Icon {
    pub fn new(name: IconName, color: Hsla) -> Self {
        Self { name, size: ICON_SIZE, color: Some(color), hover: None, spin: None }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Uses the parent's text color, including its hover and keyboard-focus styles.
    pub fn inherit_color(mut self) -> Self {
        self.color = None;
        self
    }

    /// Continuously rotates the icon, with `id` identifying its animation in the parent view.
    pub fn spin(mut self, id: impl Into<SharedString>) -> Self {
        self.spin = Some(id.into());
        self
    }

    /// Paints the icon in `color` while the pointer is over the element in hover group `group`.
    pub fn group_hover(mut self, group: impl Into<SharedString>, color: Hsla) -> Self {
        self.hover = Some((group.into(), color));
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let icon = svg()
            .data(self.name.svg())
            .flex_none()
            .size(rem(self.size))
            .text_color(self.color.unwrap_or_else(|| window.text_style().color))
            .when_some(self.hover, |icon, (group, color)| icon.group_hover(group, move |icon| icon.text_color(color)));
        if let Some(id) = self.spin {
            icon.with_animation(id, Animation::new(Duration::from_millis(1200)).repeat_synced(), |icon, delta| {
                icon.with_transformation(Transformation::rotate(radians(delta * TAU)))
            })
            .into_any_element()
        } else {
            icon.into_any_element()
        }
    }
}
