use crate::command::HostCommandResult;
use client_core::runtime::Input;
use crossterm::event::Event;
use std::time::Instant;

pub enum Message {
    Terminal(Event),
    Session(Box<Input>),
    Host(Box<HostCommandResult>),
    Tick(Instant),
}
