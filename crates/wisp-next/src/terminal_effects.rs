use std::collections::VecDeque;

/// Non-render side effects that the event loop dispatches to the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalEffect {
    Bell,
    EnableMouseCapture,
    DisableMouseCapture,
}

/// Testable queue of terminal effects that decouples App state from real stdout.
#[derive(Debug, Default)]
pub struct TerminalEffects {
    queue: VecDeque<TerminalEffect>,
}

impl TerminalEffects {
    pub fn push(&mut self, effect: TerminalEffect) {
        self.queue.push_back(effect);
    }

    pub fn pop(&mut self) -> Option<TerminalEffect> {
        self.queue.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_preserves_fifo_order() {
        let mut effects = TerminalEffects::default();
        effects.push(TerminalEffect::Bell);
        effects.push(TerminalEffect::EnableMouseCapture);
        effects.push(TerminalEffect::DisableMouseCapture);

        assert_eq!(effects.pop(), Some(TerminalEffect::Bell));
        assert_eq!(effects.pop(), Some(TerminalEffect::EnableMouseCapture));
        assert_eq!(effects.pop(), Some(TerminalEffect::DisableMouseCapture));
        assert_eq!(effects.pop(), None);
    }

    #[test]
    fn empty_queue_returns_none() {
        let mut effects = TerminalEffects::default();
        assert!(effects.is_empty());
        assert_eq!(effects.pop(), None);
    }

    #[test]
    fn clear_removes_all_effects() {
        let mut effects = TerminalEffects::default();
        effects.push(TerminalEffect::Bell);
        effects.push(TerminalEffect::Bell);
        effects.clear();
        assert!(effects.is_empty());
        assert_eq!(effects.pop(), None);
    }
}
