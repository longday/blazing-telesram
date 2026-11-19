//! Production [`Connector`] over `tokio-tungstenite` with native TLS.

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::xiva::{ConnectError, Connector, Socket, SocketEvent, Target};

pub struct TungsteniteConnector {
    /// Bounds TCP + TLS + upgrade together.
    pub connect_timeout: Duration,
}

impl Default for TungsteniteConnector {
    fn default() -> Self {
        Self { connect_timeout: Duration::from_secs(15) }
    }
}

pub struct WsSocket(WebSocketStream<MaybeTlsStream<TcpStream>>);

impl Connector for TungsteniteConnector {
    type Socket = WsSocket;

    async fn connect(&self, target: &Target) -> Result<WsSocket, ConnectError> {
        let failed = |e: String| ConnectError::Failed(e);
        let mut request = target.url.as_str().into_client_request().map_err(|e| failed(e.to_string()))?;
        let mut cookie = HeaderValue::from_str(&target.cookie).map_err(|_| failed("cookie is not a valid header".into()))?;
        cookie.set_sensitive(true);
        let origin = HeaderValue::from_str(&target.origin).map_err(|e| failed(e.to_string()))?;
        request.headers_mut().insert("Cookie", cookie);
        request.headers_mut().insert("Origin", origin);

        let handshake = tokio_tungstenite::connect_async_tls_with_config(request, None, false, None);
        match tokio::time::timeout(self.connect_timeout, handshake).await {
            Err(_) => Err(failed("connect timed out".into())),
            Ok(Ok((stream, _))) => Ok(WsSocket(stream)),
            Ok(Err(Error::Http(response))) if matches!(response.status().as_u16(), 401 | 403) => {
                Err(ConnectError::Unauthorized)
            }
            Ok(Err(Error::Http(response))) => Err(failed(format!("handshake status {}", response.status()))),
            Ok(Err(e)) => Err(failed(e.to_string())),
        }
    }
}

impl Socket for WsSocket {
    async fn send(&mut self, frame: Vec<u8>) -> Result<(), String> {
        self.0.send(Message::binary(frame)).await.map_err(|e| e.to_string())
    }

    async fn recv(&mut self) -> SocketEvent {
        loop {
            match self.0.next().await {
                None => return SocketEvent::Closed { code: None, reason: "stream ended".into() },
                Some(Err(e)) => return SocketEvent::Error(e.to_string()),
                Some(Ok(Message::Binary(bytes))) => return SocketEvent::Binary(bytes.to_vec()),
                Some(Ok(Message::Text(text))) => return SocketEvent::Text(text.as_str().to_owned()),
                Some(Ok(Message::Close(frame))) => {
                    return match frame {
                        Some(f) => SocketEvent::Closed { code: Some(u16::from(f.code)), reason: f.reason.as_str().to_owned() },
                        None => SocketEvent::Closed { code: None, reason: String::new() },
                    };
                }
                // Pings are answered by tungstenite itself on the next write or flush.
                Some(Ok(_)) => {}
            }
        }
    }

    async fn close(&mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.0.close(None)).await;
    }
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    use super::*;

    fn target(addr: std::net::SocketAddr) -> Target {
        Target { url: format!("ws://{addr}/v2"), cookie: "Session_id=secret".into(), origin: "https://yandex.ru".into() }
    }

    #[tokio::test]
    async fn handshake_carries_cookie_and_origin_and_frames_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut ws = tokio_tungstenite::accept_hdr_async(stream, |req: &Request, resp: Response| {
                for name in ["cookie", "origin"] {
                    seen.push(req.headers().get(name).map(|v| v.to_str().unwrap().to_owned()));
                }
                Ok(resp)
            })
            .await
            .unwrap();
            ws.send(Message::text("hello")).await.unwrap();
            let got = ws.next().await.unwrap().unwrap();
            ws.send(got).await.unwrap();
            ws.send(Message::Close(None)).await.unwrap();
            seen
        });
        let mut socket = TungsteniteConnector::default().connect(&target(addr)).await.unwrap();
        assert!(matches!(socket.recv().await, SocketEvent::Text(t) if t == "hello"));
        socket.send(vec![1, 2, 3]).await.unwrap();
        assert!(matches!(socket.recv().await, SocketEvent::Binary(b) if b == [1, 2, 3]));
        assert!(matches!(socket.recv().await, SocketEvent::Closed { .. }));
        assert_eq!(server.await.unwrap(), [Some("Session_id=secret".to_owned()), Some("https://yandex.ru".to_owned())]);
    }

    #[tokio::test]
    async fn rejected_handshake_is_unauthorized() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut stream, b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n").await.unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let err = TungsteniteConnector::default().connect(&target(addr)).await.err().unwrap();
        assert!(matches!(err, ConnectError::Unauthorized));
    }

    #[tokio::test]
    async fn silent_server_hits_the_connect_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _hold = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let connector = TungsteniteConnector { connect_timeout: Duration::from_millis(150) };
        let err = connector.connect(&target(addr)).await.err().unwrap();
        assert!(matches!(err, ConnectError::Failed(m) if m.contains("timed out")));
    }
}
