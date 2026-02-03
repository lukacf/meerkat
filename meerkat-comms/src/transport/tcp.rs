//! TCP transport for Meerkat comms.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};

use super::TransportError;
use super::codec::{EnvelopeFrame, TransportCodec};
use crate::types::Envelope;

/// A TCP listener for accepting incoming connections.
pub struct TcpTransportListener {
    listener: TcpListener,
}

impl TcpTransportListener {
    /// Bind to a TCP socket address.
    pub async fn bind(addr: SocketAddr) -> Result<Self, TransportError> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener })
    }

    /// Get the local address this listener is bound to.
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept an incoming connection.
    pub async fn accept(&self) -> Result<TcpConnection, TransportError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(TcpConnection {
            framed: Framed::new(
                stream,
                TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
            ),
        })
    }
}

/// A TCP connection for sending and receiving envelopes.
pub struct TcpConnection {
    framed: Framed<TcpStream, TransportCodec>,
}

impl TcpConnection {
    /// Connect to a TCP socket address.
    pub async fn connect(addr: SocketAddr) -> Result<Self, TransportError> {
        let stream = TcpStream::connect(addr).await?;
        Ok(Self {
            framed: Framed::new(
                stream,
                TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
            ),
        })
    }

    /// Send an envelope over the connection.
    pub async fn send(&mut self, envelope: &Envelope) -> Result<(), TransportError> {
        let frame = EnvelopeFrame {
            envelope: envelope.clone(),
            raw: Arc::new(Bytes::new()),
        };

        self.framed.send(frame).await.map_err(TransportError::Io)?;
        Ok(())
    }

    /// Receive an envelope from the connection.
    pub async fn recv(&mut self) -> Result<Envelope, TransportError> {
        match self.framed.next().await {
            Some(Ok(frame)) => Ok(frame.envelope),
            Some(Err(err)) => Err(TransportError::Io(err)),
            None => Err(TransportError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            ))),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::identity::{Keypair, PubKey, Signature};
    use crate::types::MessageKind;
    use uuid::Uuid;

    fn make_test_envelope() -> Envelope {
        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "hello from tcp".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        envelope
    }

    #[tokio::test]
    async fn test_tcp_bind() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = match TcpTransportListener::bind(addr).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        return;
                    }
                }
                panic!("TcpTransportListener::bind failed: {e:?}");
            }
        };
        let local = listener.local_addr().unwrap();
        assert!(local.port() > 0);
    }

    #[tokio::test]
    async fn test_tcp_connect() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = match TcpTransportListener::bind(addr).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        return;
                    }
                }
                panic!("TcpTransportListener::bind failed: {e:?}");
            }
        };
        let local = listener.local_addr().unwrap();

        // Spawn accept task
        let accept_handle = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });

        // Connect
        let _conn = match TcpConnection::connect(local).await {
            Ok(conn) => conn,
            Err(e) => {
                if let TransportError::Io(ref err) = e {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        return;
                    }
                }
                panic!("TcpConnection::connect failed: {e:?}");
            }
        };
        accept_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_envelope_roundtrip() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = match TcpTransportListener::bind(addr).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        return;
                    }
                }
                panic!("TcpTransportListener::bind failed: {e:?}");
            }
        };
        let local = listener.local_addr().unwrap();

        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        // Spawn server
        let server_handle = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.recv().await.unwrap()
        });

        // Client sends
        let mut client = match TcpConnection::connect(local).await {
            Ok(conn) => conn,
            Err(e) => {
                if let TransportError::Io(ref err) = e {
                    if err.kind() == std::io::ErrorKind::PermissionDenied {
                        return;
                    }
                }
                panic!("TcpConnection::connect failed: {e:?}");
            }
        };
        client.send(&envelope).await.unwrap();

        // Server receives
        let received = server_handle.await.unwrap();
        assert_eq!(received.id, envelope_id);
        assert!(received.verify());
    }
}
