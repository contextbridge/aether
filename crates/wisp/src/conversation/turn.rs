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
    prompt_in_flight: bool,
    compaction_active: bool,
    context_usage: Option<ContextUsageDisplay>,
    spinner_tick: usize,
}

impl TurnState {
    pub fn is_prompt_in_flight(&self) -> bool {
        self.prompt_in_flight
    }

    pub fn set_prompt_in_flight(&mut self, value: bool) {
        self.prompt_in_flight = value;
    }

    pub fn is_compaction_active(&self) -> bool {
        self.compaction_active
    }

    pub fn set_compaction_active(&mut self, value: bool) {
        self.compaction_active = value;
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
