use crate::types::WebSocketClose;
use agent_client_protocol::{Client, ConnectTo, Lines};
use futures::channel::mpsc;
use futures::future::{self, Ready};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::futures::WebSocket;
use gloo_net::websocket::{Message, WebSocketError};
use std::cell::OnceCell;
use std::io;
use std::pin::pin;
use std::rc::Rc;
use thiserror::Error;
use wasm_bindgen_futures::spawn_local;

const OUTGOING_CAPACITY: usize = 32;

#[derive(Debug, Error)]
#[error("invalid WebSocket URL or protocols: {0}")]
pub struct InvalidRequest(String);

#[derive(Debug, Clone, Default)]
pub(crate) struct CloseStatus(Rc<OnceCell<WebSocketClose>>);

impl CloseStatus {
    pub(crate) fn get(&self) -> Option<WebSocketClose> {
        self.0.get().cloned()
    }
}

/// Open a browser WebSocket to an ACP agent, offering `protocols` as subprotocols.
pub(crate) fn connect_websocket(
    url: &str,
    protocols: &[String],
) -> Result<(impl ConnectTo<Client>, CloseStatus), InvalidRequest> {
    let socket = WebSocket::open_with_protocols(url, protocols).map_err(|error| InvalidRequest(error.to_string()))?;
    let status = CloseStatus::default();
    let (outgoing_tx, outgoing_rx) = mpsc::channel(OUTGOING_CAPACITY);
    let (incoming_tx, incoming_rx) = mpsc::unbounded();
    let (sink, stream) = socket.split();
    let send = outgoing_rx.map(|line| Ok(Message::Text(line))).forward(sink);
    let receive = stream.filter_map(incoming_line(status.clone())).map(Ok).forward(incoming_tx);
    spawn_local(async move {
        future::select(pin!(send), pin!(receive)).await;
    });
    Ok((Lines::new(outgoing_tx.sink_map_err(io::Error::other), incoming_rx), status))
}

fn incoming_line(
    status: CloseStatus,
) -> impl FnMut(Result<Message, WebSocketError>) -> Ready<Option<io::Result<String>>> {
    move |message| {
        future::ready(match message {
            Ok(Message::Text(line)) => Some(Ok(line)),
            Ok(Message::Bytes(_)) => {
                Some(Err(io::Error::new(io::ErrorKind::InvalidData, "binary WebSocket messages are not supported")))
            }
            Err(WebSocketError::ConnectionClose(event)) => {
                let close = WebSocketClose { code: event.code, reason: event.reason };
                let error = (!event.was_clean).then(|| {
                    io::Error::new(io::ErrorKind::ConnectionAborted, format!("WebSocket closed abnormally ({close})"))
                });
                let _ = status.0.set(close);
                error.map(Err)
            }
            // The browser always follows a connection error with the close that describes it.
            Err(WebSocketError::ConnectionError) => None,
            Err(error) => Some(Err(io::Error::other(error.to_string()))),
        })
    }
}
