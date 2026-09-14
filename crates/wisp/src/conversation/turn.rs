use agent_client_protocol::schema::v2::{CompactionId, CompactionStatus, CompactionUpdate};
use std::collections::HashSet;

/// Context-window usage as the status line displays it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageDisplay {
    pub used_tokens: u32,
    pub limit_tokens: u32,
}

impl ContextUsageDisplay {
    pub fn used_ratio(self) -> f64 {
        if self.limit_tokens == 0 {
            return 0.0;
        }
        (f64::from(self.used_tokens) / f64::from(self.limit_tokens)).clamp(0.0, 1.0)
    }
}

#[derive(Debug, Default)]
pub struct TurnState {
    active_compactions: HashSet<CompactionId>,
    context_usage: Option<ContextUsageDisplay>,
    spinner_tick: usize,
}

impl TurnState {
    pub fn is_compaction_active(&self) -> bool {
        !self.active_compactions.is_empty()
    }

    pub fn apply_compaction(&mut self, update: &CompactionUpdate) {
        match update.status {
            CompactionStatus::InProgress => { self.active_compactions.insert(update.compaction_id.clone()); }
            CompactionStatus::Completed | CompactionStatus::Failed | CompactionStatus::Cancelled => {
                self.active_compactions.remove(&update.compaction_id);
            }
            _ => {}
        }
    }

    pub fn clear_compactions(&mut self) {
        self.active_compactions.clear();
    }

    pub fn set_context_usage(&mut self, context_usage: Option<ContextUsageDisplay>) {
        self.context_usage = context_usage;
    }

    pub fn context_usage(&self) -> Option<ContextUsageDisplay> {
        self.context_usage
    }

    pub fn spinner_tick(&self) -> usize {
        self.spinner_tick
    }

    pub fn advance_spinner(&mut self) {
        self.spinner_tick = self.spinner_tick.wrapping_add(1);
    }

    pub fn reset(&mut self) {
        let spinner_tick = self.spinner_tick;
        *self = Self { spinner_tick, ..Self::default() };
    }
}
