mod agent;
mod dispatcher;
mod files;
mod git;
mod runner;
mod tasks;

pub use git::{execute as execute_git, resolve_workspace_status};

#[cfg(feature = "testing")]
pub use dispatcher::CommandDispatcher;
#[cfg(not(feature = "testing"))]
pub(crate) use dispatcher::CommandDispatcher;
pub(crate) use runner::run;
