use rouge::core::{
    Color, Div, Ease, Effect, Inputs, IntoProp, PaintRegion, Prop, Styled, Tween, div, switch, window_focused,
};
use rouge::reactive::{Signal, expect_context, memo, provide_context, signal, stored, use_context};

/// Activity, not progress: each state has a distinct silhouette and direction of motion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WispState {
    Connecting,
    Thinking,
    Compacting,
    Working,
    Writing,
    Waiting,
}

/// A shared, reactive preference for activity indicators. Reduced motion removes their clocks.
#[derive(Clone, Copy)]
pub struct WispMotion(Signal<bool>);

/// A quiet activity mark. Icons below 32 design pixels use the compact artwork.
/// Waiting is always still; reduced motion, loss of focus, and clipping also suspend the clock.
/// Pair this decoration with a visible activity label rather than relying on its color or motion.
pub fn wisp<T>(size: f32, tint: impl IntoProp<Color, T>, state: impl Fn() -> WispState + 'static) -> Div {
    let tint = stored(tint.into_prop());
    let state = memo(state);
    let region = signal(PaintRegion::default());
    let focused = window_focused();
    let motion = use_context::<WispMotion>();
    let animated = memo(move || {
        state.get() != WispState::Waiting
            && focused.get()
            && !motion.is_some_and(WispMotion::reduced)
            && region.with(|region| !region.bounds.is_empty())
    });
    let expanded = size >= EXPANDED_SIZE;
    div().flex_none().size(size).track_paint_region(region).child(switch(
        move || animated.get(),
        move |animated| {
            let art = div().size_full().bg(move || tint.with(Prop::get)).pass_through();
            if animated {
                art.transition(Tween::ms(SETTLE_MS).ease(Ease::Out))
                    .effect(move || Flow { state: state.get(), expanded })
            } else {
                art.effect(move || Still { state: state.get(), expanded })
            }
        },
    ))
}

impl WispState {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Connecting",
            Self::Thinking => "Thinking",
            Self::Compacting => "Compacting the conversation",
            Self::Working => "Working",
            Self::Writing => "Writing",
            Self::Waiting => "Waiting for you",
        }
    }
}

impl WispMotion {
    #[must_use]
    pub fn install(reduced: bool) -> Self {
        let motion = Self(signal(reduced));
        provide_context(motion);
        motion
    }

    #[must_use]
    pub fn current() -> Self {
        expect_context()
    }

    pub fn set_reduced(self, reduced: bool) {
        self.0.set_if_changed(reduced);
    }

    #[must_use]
    pub fn reduced(self) -> bool {
        self.0.get()
    }
}

#[derive(Clone, Copy)]
struct Flow {
    state: WispState,
    expanded: bool,
}

#[derive(Clone, Copy)]
struct Still {
    state: WispState,
    expanded: bool,
}

impl Effect for Flow {
    const WGSL: &'static str = include_str!("wisp.wgsl");
    const INPUTS: Inputs = Inputs::CONTENT;
    const CLOCK: Option<usize> = Some(0);

    fn uniforms(&self) -> Vec<f32> {
        uniforms(self.state, self.expanded)
    }

    fn margin(&self) -> f32 {
        HALO_REACH
    }
}

impl Effect for Still {
    const WGSL: &'static str = include_str!("wisp.wgsl");
    const INPUTS: Inputs = Inputs::CONTENT;

    fn uniforms(&self) -> Vec<f32> {
        uniforms(self.state, self.expanded)
    }

    fn margin(&self) -> f32 {
        HALO_REACH
    }
}

fn uniforms(state: WispState, expanded: bool) -> Vec<f32> {
    let on = |target| if state == target { 1.0 } else { 0.0 };
    vec![
        0.0,
        on(WispState::Thinking),
        on(WispState::Working),
        on(WispState::Writing),
        on(WispState::Connecting),
        on(WispState::Compacting),
        on(WispState::Waiting),
        if expanded { 1.0 } else { 0.0 },
    ]
}

const EXPANDED_SIZE: f32 = 32.0;
const HALO_REACH: f32 = 2.0;
const SETTLE_MS: f32 = 320.0;
