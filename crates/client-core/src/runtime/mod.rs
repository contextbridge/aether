mod agent;
mod git_review;
mod tasks;

pub use tasks::TaskSupervisor;

use crate::command::{Command, CommandResult};
use crate::composer::attachment::build_attachments;
use acp_utils::client::{AcpClientHandle, AcpEvent};
use git_review::GitReviewRuntime;
use std::collections::VecDeque;
use tokio::sync::mpsc::UnboundedReceiver;

pub enum Input {
    Event(AcpEvent),
    Result(CommandResult),
}

pub struct SessionRuntime {
    handle: AcpClientHandle,
    events: UnboundedReceiver<AcpEvent>,
    events_closed: bool,
    tasks: TaskSupervisor<(), CommandResult>,
    git_review: GitReviewRuntime,
    ready: VecDeque<CommandResult>,
}

impl SessionRuntime {
    pub fn new(handle: AcpClientHandle, events: UnboundedReceiver<AcpEvent>) -> Self {
        Self {
            git_review: GitReviewRuntime::new(handle.clone()),
            handle,
            events,
            events_closed: false,
            tasks: TaskSupervisor::default(),
            ready: VecDeque::new(),
        }
    }

    pub fn dispatch(&mut self, command: Command) {
        let result = match command {
            Command::Agent(command) => agent::execute(&self.handle, command, &mut self.tasks),
            Command::GitReview(command) => self.git_review.dispatch(command, &mut self.tasks),
            Command::PrepareSubmission { attachments, access } => {
                self.tasks.spawn_blocking_read((), move || {
                    CommandResult::SubmissionPrepared(build_attachments(&attachments, access))
                });
                None
            }
        };
        self.ready.extend(result);
    }

    pub async fn next(&mut self) -> Input {
        if let Some(result) = self.ready.pop_front() {
            return Input::Result(result);
        }
        loop {
            tokio::select! {
                event = self.events.recv(), if !self.events_closed => {
                    let Some(event) = event else {
                        self.events_closed = true;
                        return Input::Event(AcpEvent::ConnectionClosed);
                    };
                    if let Some(event) = self.route(event) {
                        return Input::Event(event);
                    }
                }
                state = self.git_review.changed() => match state {
                    Some(state) => return Input::Result(CommandResult::GitReview(state)),
                    None => self.git_review.close(),
                },
                Some(result) = self.tasks.next(), if !self.tasks.is_empty() => return Input::Result(result),
            }
        }
    }

    pub fn try_next_event(&mut self) -> Option<AcpEvent> {
        while let Ok(event) = self.events.try_recv() {
            if let Some(event) = self.route(event) {
                return Some(event);
            }
        }
        None
    }

    pub async fn shutdown(&mut self) {
        self.git_review.close();
        self.handle.disconnect().await;
        self.tasks.shutdown().await;
    }

    fn route(&mut self, event: AcpEvent) -> Option<AcpEvent> {
        match event {
            AcpEvent::GitDiffEvent(params) => {
                self.git_review.forward(&params.session_id, params.event);
                None
            }
            event => Some(event),
        }
    }
}
