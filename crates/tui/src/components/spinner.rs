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
