use clankerdiff_remote::{Remotes, RemotesFile};
use gpui::{AppContext, Context, Task};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub use clankerdiff_remote::{
    Acp, Action, Channel, Instance, InstanceState, Inventory, Lifecycle, RemoteSpec, default_file, short_id,
};

pub const EXPIRY_WARNING: Duration = Duration::from_mins(10);

pub struct Fleet {
    remotes: Arc<dyn Remotes>,
    file: Option<PathBuf>,
    inventory: Inventory,
    file_problem: Option<String>,
    _poll: Task<()>,
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
    pub fn new(remotes: Arc<dyn Remotes>, file: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let Ok(interval) = this.read_with(cx, |fleet, _| fleet.inventory.poll_interval()) else {
                    return;
                };
                cx.background_executor().timer(interval).await;
                if this.update(cx, Self::refresh).is_err() {
                    return;
                }
            }
        });
        let mut fleet = Self { remotes, file, inventory: Inventory::new(Vec::new()), file_problem: None, _poll: poll };
        fleet.reload(cx);
        fleet
    }

    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }

    pub fn file_problem(&self) -> Option<&str> {
        self.file_problem.as_deref()
    }

    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    pub fn instances(&self, remote: &str) -> Vec<Instance> {
        let instances = self.inventory.instances(remote).iter();
        instances.filter(|instance| instance.state != InstanceState::Stopping).cloned().collect()
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        match self.file.as_deref().map(RemotesFile::read).transpose() {
            Ok(file) => {
                self.file_problem = None;
                let remotes: Vec<RemoteSpec> =
                    file.unwrap_or_default().remotes.into_iter().filter(|remote| remote.acp.is_some()).collect();
                if remotes != self.inventory.remotes() {
                    self.inventory.set_remotes(remotes);
                }
            }
            Err(error) => {
                tracing::warn!(%error, "keeping the remotes as they were");
                self.file_problem = Some(error.to_string());
            }
        }
        cx.notify();
        self.refresh(cx);
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(listing) = self.inventory.begin_listing() else { return };
        let remotes = self.remotes.clone();
        let listed = cx.background_spawn(async move { listing.run(&*remotes) });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            let _ = this.update(cx, |fleet, cx| {
                let again = fleet.inventory.finish_listing(listed);
                cx.notify();
                if again {
                    fleet.refresh(cx);
                }
            });
        })
        .detach();
    }

    pub fn change(&mut self, remote: &str, id: &str, change: Lifecycle, cx: &mut Context<Self>) {
        let Some(spec) = self.inventory.remote(remote).filter(|spec| spec.can_change(change)).cloned() else { return };
        self.inventory.begin_change(remote, id, change);
        cx.notify();
        let (remotes, id) = (self.remotes.clone(), id.to_owned());
        let changed = cx.background_spawn(async move { remotes.change(&spec, &id, change) });
        cx.spawn(async move |this, cx| {
            if let Err(error) = changed.await {
                tracing::warn!(%error, ?change, "could not change an instance");
            }
            let _ = this.update(cx, |fleet, cx| {
                fleet.inventory.finish_change();
                fleet.refresh(cx);
            });
        })
        .detach();
    }
}
