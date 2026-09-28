use crate::output::OutputFormat;
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct SessionHooks {
    pub echo: Option<OutputFormat>,
    pub idle: Option<IdleHook>,
}

#[derive(Clone, Debug)]
pub struct IdleHook {
    pub after: Duration,
    pub command: String,
}
