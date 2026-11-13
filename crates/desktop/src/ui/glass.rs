use super::Theme;
use super::tokens::RADIUS_FLOATING;
use rouge::core::{Div, Glass, Point, Styled, px};
use rouge::reactive::signal;

pub(crate) trait LiquidGlassExt {
    /// Frosted glass that bends and splits light at its rim and catches a glint under the pointer.
    #[must_use]
    fn liquid_glass(self) -> Self;
}

impl LiquidGlassExt for Div {
    fn liquid_glass(self) -> Self {
        let theme = Theme::current();
        let pointer = signal(None::<Point>);
        self.border(px(1.0))
            .border_color(theme.border_subtle)
            .rounded(RADIUS_FLOATING)
            .on_pointer_move(move |event| {
                pointer.set_if_changed(Some(event.local()));
            })
            .on_hover(move |inside| {
                if !inside {
                    pointer.set(None);
                }
            })
            .backdrop(move || {
                let glass = LIQUID_GLASS.tint(theme.elevated.get().with_alpha(TINT));
                match pointer.get() {
                    Some(at) => glass.light(at, GLINT_STRENGTH, GLINT_REACH),
                    None => glass,
                }
            })
    }
}

const LIQUID_GLASS: Glass = Glass::new()
    .blur(12.0)
    .edge_width(RADIUS_FLOATING)
    .refraction(0.6)
    .dispersion(0.15)
    .highlight(0.45)
    .shadow(0.3)
    .saturation(1.3);

const TINT: f32 = 0.68;

const GLINT_STRENGTH: f32 = 0.5;

const GLINT_REACH: f32 = 96.0;
