use std::sync::Arc;
use tokio::sync::watch;

#[derive(Clone, Default)]
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
        Self::default()
    }

    pub fn open(&self) {
        self.state.send_modify(|state| state.open = true);
    }

    pub async fn reached(&self) {
        let mut state = self.state.subscribe();
        let _ = state.wait_for(|state| state.reached).await;
    }

    pub async fn drained(&self) {
        self.state.closed().await;
    }

    pub(crate) async fn pass(&self) {
        let mut state = self.state.subscribe();
        self.state.send_modify(|state| state.reached = true);
        let _ = state.wait_for(|state| state.open).await;
    }
}
