use agent_client_protocol::{ConnectTo, Error, Lines, Role};
use futures::{SinkExt, StreamExt, stream};
use std::{io, time::Duration};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_util::sync::PollSender;

#[derive(Debug, Error)]
pub enum WebSocketError {
    #[error(transparent)]
    WebSocket(#[from] tungstenite::Error),
    #[error("unexpected raw WebSocket frame")]
    UnexpectedFrame,
    #[error("binary WebSocket messages are not supported")]
    BinaryMessage,
    #[error("WebSocket consumer cannot keep up")]
    SlowConsumer,
    #[error("WebSocket write deadline exceeded")]
    WriteDeadline,
}

/// Websocket transport for ACP
pub struct WebSocketTransport<T> {
    socket: WebSocketStream<T>,
}

impl<T: AsyncRead + AsyncWrite + Unpin> WebSocketTransport<T> {
    pub fn new(socket: WebSocketStream<T>) -> Self {
        Self { socket }
    }
}

impl<T, U> ConnectTo<U> for WebSocketTransport<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    U: Role,
{
    async fn connect_to(self, peer: impl ConnectTo<U::Counterpart>) -> Result<(), Error> {
        let (to_tx, to_rx) = mpsc::channel::<String>(32);
        let (from_tx, from_rx) = mpsc::channel::<io::Result<String>>(32);
        let output = PollSender::new(to_tx).sink_map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe));
        let input = stream::unfold(from_rx, |mut rx| async { rx.recv().await.map(|item| (item, rx)) });
        let connection_future = ConnectTo::<U>::connect_to(Lines::new(output, input), peer);
        let socket_loop = self.run_socket_loop(to_rx, from_tx);
        tokio::pin!(connection_future, socket_loop);
        tokio::select! {
            result = &mut connection_future => {
                result?;
                socket_loop.await.map_err(Error::into_internal_error)
            }

            result = &mut socket_loop => {
                result.map_err(Error::into_internal_error)?;
                connection_future.await
            },
        }
    }
}

const WRITE_DEADLINE: Duration = Duration::from_secs(10);

impl<T: AsyncRead + AsyncWrite + Unpin> WebSocketTransport<T> {
    async fn run_socket_loop(
        mut self,
        mut to_rx: mpsc::Receiver<String>,
        from_tx: mpsc::Sender<io::Result<String>>,
    ) -> Result<(), WebSocketError> {
        loop {
            tokio::select! {
                message = to_rx.recv() => {
                    let Some(text) = message else {
                        return self.finish_close().await;
                    };
                    self.write(Message::Text(text.into())).await?;
                }
                message = self.socket.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            from_tx.try_send(Ok(text.to_string())).map_err(|_| WebSocketError::SlowConsumer)?;
                        }
                        Some(Ok(Message::Ping(_))) => {
                            // Tungstenite queues the matching Pong; flush it even while ACP is idle.
                            timeout(WRITE_DEADLINE, self.socket.flush()).await
                                .map_err(|_| WebSocketError::WriteDeadline)??;
                        }
                        Some(Ok(Message::Pong(_))) => {},
                        Some(Ok(Message::Close(_))) => {
                            let _ = timeout(WRITE_DEADLINE, self.socket.flush()).await;
                            return Ok(());
                        }
                        Some(Ok(Message::Binary(_))) => {
                            self.close(CloseCode::Unsupported).await;
                            return Err(WebSocketError::BinaryMessage);
                        }
                        Some(Ok(Message::Frame(_))) => return Err(WebSocketError::UnexpectedFrame),
                        Some(Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed)) | None => return Ok(()),
                        Some(Err(error)) => {
                            if matches!(error, tungstenite::Error::Capacity(_)) {
                                self.close(CloseCode::Size).await;
                            }
                            return Err(error.into());
                        }
                    }
                }
            }
        }
    }

    async fn finish_close(&mut self) -> Result<(), WebSocketError> {
        timeout(WRITE_DEADLINE, async {
            match self.socket.send(Message::Close(None)).await {
                Ok(()) => {}
                Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
            while let Some(message) = self.socket.next().await {
                match message {
                    Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => break,
                    Err(error) => return Err(error.into()),
                    _ => {}
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| WebSocketError::WriteDeadline)?
    }

    async fn write(&mut self, message: Message) -> Result<(), WebSocketError> {
        timeout(WRITE_DEADLINE, self.socket.send(message))
            .await
            .map_err(|_| WebSocketError::WriteDeadline)?
            .map_err(WebSocketError::from)
    }

    async fn close(&mut self, code: CloseCode) {
        let _ = self.write(Message::Close(Some(CloseFrame { code, reason: "".into() }))).await;
    }
}
