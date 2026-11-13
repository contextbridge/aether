use aether_desktop::ui::{WispMotion, WispState, wisp};
use clankerdiff_rouge::ui::testing::{TestUi, component_test};
use clankerdiff_rouge_shell::rouge::core::{Color, Styled, div, show};
use clankerdiff_rouge_shell::rouge::reactive::{Entered, Runtime, Signal, signal};

#[test]
fn waiting_and_reduced_motion_have_no_clock_and_never_change_pixels() {
    for (state, reduced) in [(WispState::Waiting, false), (WispState::Writing, true)] {
        let mut scene = SceneBuilder::default().state(state).reduced(reduced).build();
        let before = scene.ui.render().unwrap().pixels;
        scene.ui.run(3.0);
        assert!(!scene.ui.is_animating());
        assert_eq!(scene.ui.render().unwrap().pixels, before);
    }
}

#[test]
fn every_state_renders_a_distinct_legible_mark_at_both_detail_levels() {
    for size in [12.0, 48.0] {
        for scale in [1.0, 2.0] {
            let mut scene = SceneBuilder::default().size(size).scale(scale).reduced(true).build();
            let mut frames = Vec::new();
            for state in STATES {
                scene.state.set(state);
                scene.ui.advance(0.0);
                let image = scene.ui.render().unwrap();
                assert!(
                    image.pixels.as_chunks::<4>().0.iter().filter(|pixel| pixel[3] > 64).count() >= 8,
                    "{state:?} at {size}px/{scale}x must not disappear"
                );
                assert!(!frames.contains(&image.pixels), "{state:?} needs a distinct static silhouette");
                frames.push(image.pixels);
            }
        }
    }
}

#[test]
fn each_active_state_actually_moves_instead_of_only_running_a_clock() {
    for state in &STATES[..5] {
        let mut scene = SceneBuilder::default().state(*state).size(48.0).build();
        let before = scene.ui.render().unwrap().pixels;
        scene.ui.run(0.8);
        assert_ne!(before, scene.ui.render().unwrap().pixels, "{state:?} should have directional motion");
    }
}

#[test]
fn working_states_morph_without_recreating_the_art_or_relaying_out() {
    let mut scene = SceneBuilder::default().build();
    scene.ui.run(1.0);
    let before = scene.ui.stats();
    for state in [WispState::Working, WispState::Writing, WispState::Compacting, WispState::Thinking] {
        scene.state.set(state);
        scene.ui.run(0.1);
    }
    scene.ui.run(1.0);
    let after = scene.ui.stats();
    assert_eq!(after.elements_created, before.elements_created, "state changes preserve the running art");
    assert_eq!(after.layouts, before.layouts, "animation only updates compositor uniforms");
    assert!(scene.ui.is_animating());
}

#[test]
fn clipped_hidden_and_unfocused_wisps_stop_and_resume_their_clocks() {
    let mut scene = SceneBuilder::default().build();
    assert!(scene.ui.is_animating());
    scene.offset.set(1000.0);
    scene.ui.run(1.0);
    assert!(!scene.ui.is_animating(), "clipped art is still");
    scene.offset.set(0.0);
    scene.ui.advance(0.0);
    assert!(scene.ui.is_animating());
    scene.hidden.set(true);
    scene.ui.run(1.0);
    assert!(!scene.ui.is_animating(), "hidden art is still");
    scene.hidden.set(false);
    scene.ui.advance(0.0);
    assert!(scene.ui.is_animating());
    scene.ui.set_focused(false);
    scene.ui.run(1.0);
    assert!(!scene.ui.is_animating(), "unfocused art is still");
    scene.ui.set_focused(true);
    scene.ui.advance(0.0);
    assert!(scene.ui.is_animating());
    scene.motion.set_reduced(true);
    scene.ui.run(1.0);
    assert!(!scene.ui.is_animating(), "the preference applies to mounted art");
}

#[test]
fn many_compact_wisps_have_a_bounded_rendering_cost_and_do_not_repaint_the_tree() {
    let mut scene = SceneBuilder::default().count(32).build();
    let before = scene.ui.stats();
    scene.ui.run(1.0);
    let after = scene.ui.stats();
    assert_eq!(before.layouts, after.layouts);
    assert_eq!(before.paints, after.paints);
    let stats = scene.ui.draw().unwrap();
    assert_eq!(stats.groups, 32, "one effect group per compact wisp");
    assert_eq!(stats.group_passes, 32, "one content-effect pass per compact wisp");
    assert_eq!(stats.backdrop_passes, 0);
    assert_eq!(stats.content_blurs, 0);
}

const STATES: [WispState; 6] = [
    WispState::Connecting,
    WispState::Thinking,
    WispState::Compacting,
    WispState::Working,
    WispState::Writing,
    WispState::Waiting,
];

struct Scene {
    ui: TestUi,
    state: Signal<WispState>,
    motion: WispMotion,
    offset: Signal<f32>,
    hidden: Signal<bool>,
    _entered: Entered,
}

struct SceneBuilder {
    state: WispState,
    size: f32,
    scale: f32,
    reduced: bool,
    count: usize,
}

impl Default for SceneBuilder {
    fn default() -> Self {
        Self { state: WispState::Thinking, size: 12.0, scale: 1.0, reduced: false, count: 1 }
    }
}

impl SceneBuilder {
    fn state(mut self, state: WispState) -> Self {
        self.state = state;
        self
    }
    fn reduced(mut self, reduced: bool) -> Self {
        self.reduced = reduced;
        self
    }
    fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
    fn scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }
    fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    fn build(self) -> Scene {
        let entered = Runtime::new().enter();
        let (ui, (state, motion, offset, hidden)) =
            component_test().size(800.0, 120.0).scale(self.scale).build_with(move || {
                let state = signal(self.state);
                let motion = WispMotion::install(self.reduced);
                let offset = signal(0.0);
                let hidden = signal(false);
                let art = show(
                    move || !hidden.get(),
                    move || {
                        div().absolute().top(16.0).left(move || offset.get()).flex().gap(8.0).children(
                            (0..self.count).map(move |_| wisp(self.size, Color::rgb(0x0038_7ccf), move || state.get())),
                        )
                    },
                );
                (div().size_full().relative().overflow_hidden().p(16.0).child(art), (state, motion, offset, hidden))
            });
        Scene { ui, state, motion, offset, hidden, _entered: entered }
    }
}
