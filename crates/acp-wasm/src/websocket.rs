//! Browser WebSocket transport for ACP.

use agent_client_protocol::{Client, ConnectTo, Lines};
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt, select};
use serde::Serialize;
use std::cell::OnceCell;
use std::fmt;
use std::io;
use std::rc::Rc;
use thiserror::Error;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

#[derive(Debug, Error)]
pub enum WebSocketError {
    #[error("invalid WebSocket URL or protocols: {0}")]
    InvalidRequest(String),
    /// The socket closed while connecting.
    #[error("WebSocket closed ({0})")]
    Closed(WebSocketClose),
}

/// The close code and reason the browser reported when the socket closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WebSocketClose {
    pub code: u16,
    pub reason: String,
}

impl fmt::Display for WebSocketClose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "code {}", self.code)?;
        if !self.reason.is_empty() {
            write!(f, ": {}", self.reason)?;
        }
        Ok(())
    }
}

/// The close the browser reported for an open socket, recorded before the ACP connection sees its stream end.
///
/// Empty while the socket is open, and after this side closed it.
#[derive(Debug, Clone, Default)]
pub struct CloseStatus(Rc<OnceCell<WebSocketClose>>);

impl CloseStatus {
    pub fn get(&self) -> Option<WebSocketClose> {
        self.0.get().cloned()
    }
}

/// Open a browser WebSocket to an ACP agent, offering `protocols` as subprotocols, and resolve once it is open.
///
/// Each text frame carries one JSON-RPC message. A binary frame or an abnormal close fails the connection, a
/// clean close ends it, and dropping the ACP connection closes the socket.
pub async fn connect_websocket(
    url: &str,
    protocols: &[String],
) -> Result<(impl ConnectTo<Client>, CloseStatus), WebSocketError> {
    let (events_tx, mut events) = mpsc::unbounded();
    let socket = BrowserSocket::open(url, protocols, events_tx)?;
    match events.next().await {
        Some(SocketEvent::Open) => {}
        Some(SocketEvent::Closed { close, .. }) => return Err(WebSocketError::Closed(close)),
        Some(SocketEvent::Text(_) | SocketEvent::Binary) | None => {
            unreachable!("a WebSocket delivers no messages before it opens, and its handlers hold every sender")
        }
    }
    let status = CloseStatus::default();
    let (outgoing_tx, outgoing_rx) = mpsc::channel(OUTGOING_CAPACITY);
    let (incoming_tx, incoming_rx) = mpsc::unbounded();
    spawn_local(pump(socket, events, outgoing_rx, incoming_tx, status.clone()));
    Ok((Lines::new(outgoing_tx.sink_map_err(io::Error::other), incoming_rx), status))
}

const OUTGOING_CAPACITY: usize = 32;
const NORMAL_CLOSURE: u16 = 1000;

enum SocketEvent {
    Open,
    Text(String),
    Binary,
    Closed { close: WebSocketClose, clean: bool },
}

/// Owns the JS socket and its event handlers, which forward into a `Send` channel for [`pump`].
struct BrowserSocket {
    socket: WebSocket,
    _on_open: Closure<dyn FnMut(Event)>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_close: Closure<dyn FnMut(CloseEvent)>,
}

impl BrowserSocket {
    fn open(
        url: &str,
        protocols: &[String],
        events: mpsc::UnboundedSender<SocketEvent>,
    ) -> Result<Self, WebSocketError> {
        let protocols: js_sys::Array = protocols.iter().map(|protocol| JsValue::from_str(protocol)).collect();
        let socket = WebSocket::new_with_str_sequence(url, &protocols)
            .map_err(|error| WebSocketError::InvalidRequest(describe(&error)))?;
        socket.set_binary_type(BinaryType::Arraybuffer);
        let on_open = Closure::<dyn FnMut(Event)>::new({
            let events = events.clone();
            move |_: Event| {
                let _ = events.unbounded_send(SocketEvent::Open);
            }
        });
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
            let events = events.clone();
            move |event: MessageEvent| {
                let message = event.data().as_string().map_or(SocketEvent::Binary, SocketEvent::Text);
                let _ = events.unbounded_send(message);
            }
        });
        let on_close = Closure::<dyn FnMut(CloseEvent)>::new(move |event: CloseEvent| {
            let _ = events.unbounded_send(SocketEvent::Closed {
                close: WebSocketClose { code: event.code(), reason: event.reason() },
                clean: event.was_clean(),
            });
        });
        socket.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        Ok(Self { socket, _on_open: on_open, _on_message: on_message, _on_close: on_close })
    }

    fn send(&self, line: &str) -> io::Result<()> {
        self.socket.send_with_str(line).map_err(|error| io::Error::other(describe(&error)))
    }
}

impl Drop for BrowserSocket {
    fn drop(&mut self) {
        // Detach first: the browser must not call handlers whose closures are about to be freed.
        self.socket.set_onopen(None);
        self.socket.set_onmessage(None);
        self.socket.set_onclose(None);
        let _ = self.socket.close_with_code(NORMAL_CLOSURE);
    }
}

async fn pump(
    socket: BrowserSocket,
    mut events: mpsc::UnboundedReceiver<SocketEvent>,
    mut outgoing: mpsc::Receiver<String>,
    incoming: mpsc::UnboundedSender<io::Result<String>>,
    status: CloseStatus,
) {
    loop {
        select! {
            line = outgoing.next() => {
                let Some(line) = line else { return };
                if let Err(error) = socket.send(&line) {
                    let _ = incoming.unbounded_send(Err(error));
                    return;
                }
            }
            event = events.next() => match event {
                Some(SocketEvent::Text(text)) => {
                    let _ = incoming.unbounded_send(Ok(text));
                }
                Some(SocketEvent::Binary) => {
                    let error = io::Error::new(io::ErrorKind::InvalidData, "binary WebSocket messages are not supported");
                    let _ = incoming.unbounded_send(Err(error));
                }
                Some(SocketEvent::Closed { close, clean }) => {
                    if !clean {
                        let error =
                            io::Error::new(io::ErrorKind::ConnectionAborted, format!("WebSocket closed abnormally ({close})"));
                        let _ = incoming.unbounded_send(Err(error));
                    }
                    let _ = status.0.set(close);
                    return;
                }
                None => return,
                Some(SocketEvent::Open) => {}
            },
        }
    }
}

fn describe(error: &JsValue) -> String {
    error.dyn_ref::<js_sys::Error>().map_or_else(|| format!("{error:?}"), |error| error.message().into())
}
