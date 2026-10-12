use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone)]
pub struct Gate {
    state: Arc<watch::Sender<GateState>>,
}

#[derive(Clone, Copy, Default)]
struct GateState {
    reached: bool,
    open: bool,
}

impl Gate {
    pub fn new() -> Self {
        Self { state: Arc::new(watch::Sender::new(GateState::default())) }
    }

    pub fn open(&self) {
        self.state.send_modify(|state| state.open = true);
    }

    pub async fn reached(&self) {
        let mut state = self.state.subscribe();
        let _ = state.wait_for(|state| state.reached).await;
    }

    pub(crate) async fn pass(&self) {
        self.state.send_modify(|state| state.reached = true);
        let mut state = self.state.subscribe();
        let _ = state.wait_for(|state| state.open).await;
    }
}

impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
}
