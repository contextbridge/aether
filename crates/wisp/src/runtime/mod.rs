mod agent;
mod dispatcher;
mod files;
mod git_review;
mod runner;
mod tasks;

#[cfg(feature = "testing")]
pub use dispatcher::CommandDispatcher;
#[cfg(not(feature = "testing"))]
pub(crate) use dispatcher::CommandDispatcher;
pub(crate) use runner::run;
