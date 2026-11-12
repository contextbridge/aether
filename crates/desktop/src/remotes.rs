use crate::tasks::{blocking, spawn_in};
use clankerdiff_remote::{Remotes, RemotesFile};
use clankerdiff_rouge_shell::rouge::reactive::{Scope, Signal, Stored, signal, sleep, stored};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub use clankerdiff_remote::{
    Acp, Action, Channel, Instance, InstanceState, Inventory, Lifecycle, RemoteSpec, default_file, short_id,
};

pub const EXPIRY_WARNING: Duration = Duration::from_mins(10);

#[derive(Clone, Copy)]
pub struct Fleet {
    remotes: Stored<Arc<dyn Remotes>>,
    file: Stored<Option<PathBuf>>,
    state: Signal<FleetState>,
    scope: Scope,
}

struct FleetState {
    inventory: Inventory,
    file_problem: Option<String>,
}

pub fn time_left(instance: &Instance, now: SystemTime) -> Option<Duration> {
    let expires_at = instance.expires_at.filter(|_| instance.state == InstanceState::Running)?;
    Some(expires_at.duration_since(now).unwrap_or_default())
}

pub fn time_left_label(left: Duration) -> String {
    let minutes = left.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => "<1m".to_owned(),
        (0, minutes) => format!("{minutes}m"),
        (hours, minutes) => format!("{hours}h {minutes:02}m"),
    }
}

impl Fleet {
    pub fn new(remotes: Arc<dyn Remotes>, file: Option<PathBuf>) -> Self {
        let fleet = Self {
            remotes: stored(remotes),
            file: stored(file),
            state: signal(FleetState { inventory: Inventory::new(Vec::new()), file_problem: None }),
            scope: Scope::new(),
        };
        spawn_in(fleet.scope, async move {
            loop {
                if !fleet.state.is_alive() {
                    return;
                }
                sleep(fleet.state.with_untracked(|state| state.inventory.poll_interval())).await;
                if !fleet.state.is_alive() {
                    return;
                }
                fleet.refresh();
            }
        });
        fleet.reload();
        fleet
    }

    pub fn file(&self) -> Option<PathBuf> {
        self.file.get()
    }

    pub fn file_problem(&self) -> Option<String> {
        self.state.with(|state| state.file_problem.clone())
    }

    pub fn with_inventory<T>(&self, read: impl FnOnce(&Inventory) -> T) -> T {
        self.state.with(|state| read(&state.inventory))
    }

    pub fn instances(&self, remote: &str) -> Vec<Instance> {
        self.with_inventory(|inventory| {
            let instances = inventory.instances(remote).iter();
            instances.filter(|instance| instance.state != InstanceState::Stopping).cloned().collect()
        })
    }

    pub fn reload(&self) {
        let read = self.file.with(|file| file.as_deref().map(RemotesFile::read).transpose());
        self.state.update(|state| match read {
            Ok(file) => {
                state.file_problem = None;
                let remotes: Vec<RemoteSpec> =
                    file.unwrap_or_default().remotes.into_iter().filter(|remote| remote.acp.is_some()).collect();
                if remotes != state.inventory.remotes() {
                    state.inventory.set_remotes(remotes);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "keeping the remotes as they were");
                state.file_problem = Some(error.to_string());
            }
        });
        self.refresh();
    }

    pub fn refresh(&self) {
        let Some(listing) = self.state.try_update(|state| state.inventory.begin_listing()).flatten() else {
            return;
        };
        let (fleet, remotes) = (*self, self.remotes.get());
        spawn_in(self.scope, async move {
            let Some(listed) = blocking(move || listing.run(&*remotes)).await else {
                return;
            };
            let again = fleet.state.try_update(|state| state.inventory.finish_listing(listed));
            if again == Some(true) {
                fleet.refresh();
            }
        });
    }

    pub fn change(&self, remote: &str, id: &str, change: Lifecycle) {
        let spec =
            self.with_inventory(|inventory| inventory.remote(remote).filter(|spec| spec.can_change(change)).cloned());
        let Some(spec) = spec else { return };
        self.state.update(|state| state.inventory.begin_change(remote, id, change));
        let (fleet, remotes, id) = (*self, self.remotes.get(), id.to_owned());
        spawn_in(self.scope, async move {
            let changed = blocking(move || remotes.change(&spec, &id, change)).await;
            if let Some(Err(error)) = changed {
                tracing::warn!(%error, ?change, "could not change an instance");
            }
            if fleet.state.try_update(|state| state.inventory.finish_change()).is_some() {
                fleet.refresh();
            }
        });
    }
}
