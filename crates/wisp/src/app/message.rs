use crate::command::CommandResult;
use acp_utils::client::AcpEvent;
use crossterm::event::Event;
use std::time::Instant;

pub enum Message {
    Terminal(Event),
    Agent(Box<AcpEvent>),
    CommandFinished(CommandResult),
    Tick(Instant),
}
