use crate::command::HostCommandResult;
use crossterm::event::Event;
use foxfire_core::session_core::Input;
use std::time::Instant;

pub enum Message {
    Terminal(Event),
    Session(Box<Input>),
    Host(Box<HostCommandResult>),
    Tick(Instant),
}
