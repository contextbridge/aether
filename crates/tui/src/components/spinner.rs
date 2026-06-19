use crate::components::{Component, Event, ViewContext};
use crate::line::Line;
use crate::rendering::frame::Frame;

pub const BRAILLE_FRAMES: &[char] = &['⠒', '⠮', '⠷', '⢷', '⡾', '⣯', '⣽', '⣿', '⣭', '⢯'];

pub struct Spinner {
    tick: u16,
    pub visible: bool,
    frames: &'static [char],
}

impl Spinner {
    pub fn new(frames: &'static [char]) -> Self {
        Self { tick: 0, visible: false, frames }
    }

    pub fn braille() -> Self {
        Self::new(BRAILLE_FRAMES)
    }

    pub fn current_frame(&self) -> char {
        self.frames[self.frame_index()]
    }

    pub fn frame_index(&self) -> usize {
        self.tick as usize % self.frames.len()
    }

    pub fn reset(&mut self) {
        self.tick = 0;
        self.visible = true;
    }

    #[allow(dead_code)]
    pub fn set_tick(&mut self, tick: u16) {
        self.tick = tick;
    }

    /// Advance the animation state. Call this on tick events.
    pub fn on_tick(&mut self) {
        if self.visible {
            self.tick = self.tick.wrapping_add(1);
        }
    }
}

impl Component for Spinner {
    type Message = ();

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match event {
            Event::Tick => {
                self.on_tick();
                Some(vec![])
            }
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        if !self.visible {
            return Frame::new(vec![]);
        }

        let ch = self.current_frame();
        let mut line = Line::default();
        line.push_styled(ch.to_string(), context.theme.info());
        Frame::new(vec![line])
    }
}

impl Default for Spinner {
    fn default() -> Self {
        Self::braille()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invisible_renders_empty() {
        let mut spinner = Spinner::default();
        let ctx = ViewContext::new((80, 24));
        let frame = spinner.render(&ctx);
        assert!(frame.lines().is_empty());
    }

    #[test]
    fn visible_renders_one_line() {
        let mut spinner = Spinner { visible: true, ..Spinner::default() };
        let ctx = ViewContext::new((80, 24));
        let frame = spinner.render(&ctx);
        assert_eq!(frame.lines().len(), 1);
    }

    #[test]
    fn different_ticks_produce_different_output() {
        let ctx = ViewContext::new((80, 24));

        let mut spinner_a = Spinner { visible: true, ..Spinner::default() };

        let mut spinner_b = Spinner { visible: true, ..Spinner::default() };
        spinner_b.set_tick(1);

        let a = spinner_a.render(&ctx).lines()[0].plain_text();
        let b = spinner_b.render(&ctx).lines()[0].plain_text();

        assert_ne!(a, b);
    }

    #[test]
    fn cycles_after_full_rotation() {
        let ctx = ViewContext::new((80, 24));

        let mut spinner_a = Spinner { visible: true, ..Spinner::default() };

        let mut spinner_b = Spinner { visible: true, ..Spinner::default() };
        #[allow(clippy::cast_possible_truncation)]
        spinner_b.set_tick(BRAILLE_FRAMES.len() as u16);

        let a = spinner_a.render(&ctx).lines()[0].plain_text();
        let b = spinner_b.render(&ctx).lines()[0].plain_text();

        assert_eq!(a, b);
    }

    #[test]
    fn custom_frames() {
        static CUSTOM: &[char] = &['|', '/', '-', '\\'];
        let mut spinner = Spinner::new(CUSTOM);
        spinner.set_tick(1);
        spinner.visible = true;
        assert_eq!(spinner.current_frame(), '/');
    }

    #[test]
    fn on_tick_advances_when_visible() {
        let mut spinner = Spinner { visible: true, ..Spinner::default() };
        spinner.on_tick();
        assert_eq!(spinner.current_frame(), BRAILLE_FRAMES[1]);
    }

    #[test]
    fn on_tick_noop_when_invisible() {
        let mut spinner = Spinner::default();
        spinner.on_tick();
        assert_eq!(spinner.current_frame(), BRAILLE_FRAMES[0]);
    }

    #[test]
    fn reset_sets_tick_zero_and_visible() {
        let mut spinner = Spinner::default();
        spinner.set_tick(5);
        spinner.visible = false;
        spinner.reset();
        assert!(spinner.visible);
        assert_eq!(spinner.current_frame(), BRAILLE_FRAMES[0]);
    }
}
