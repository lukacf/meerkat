//! Unix domain socket transport for Meerkat comms.

use std::path::Path;
use std::sync::Arc;

use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::Framed;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};

use super::TransportError;
use super::codec::{EnvelopeFrame, TransportCodec};
use crate::types::Envelope;

/// A UDS listener for accepting incoming connections.
pub struct UdsListener {
    listener: UnixListener,
}

impl UdsListener {
    /// Bind to a Unix domain socket path.
    pub async fn bind(path: &Path) -> Result<Self, TransportError> {
        // Remove existing socket file if present (without blocking the executor).
        if let Err(err) = tokio::fs::remove_file(path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err.into());
        }

        let listener = UnixListener::bind(path)?;
        Ok(Self { listener })
    }

    /// Accept an incoming connection.
    pub async fn accept(&self) -> Result<UdsConnection, TransportError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(UdsConnection {
            framed: Framed::new(
                stream,
                TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
            ),
        })
    }
}

/// A UDS connection for sending and receiving envelopes.
pub struct UdsConnection {
    framed: Framed<UnixStream, TransportCodec>,
}

impl UdsConnection {
    /// Connect to a Unix domain socket.
    pub async fn connect(path: &Path) -> Result<Self, TransportError> {
        let stream = UnixStream::connect(path).await?;
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
    use tempfile::TempDir;
    use uuid::Uuid;

    fn make_test_envelope() -> Envelope {
        let keypair = Keypair::generate();
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: keypair.public_key(),
            to: PubKey::new([2u8; 32]),
            kind: MessageKind::Message {
                body: "hello from uds".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&keypair);
        envelope
    }

    #[tokio::test]
    async fn test_uds_bind() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");

        let listener = match UdsListener::bind(&path).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return;
                }
                panic!("UdsListener::bind failed: {e:?}");
            }
        };
        assert!(path.exists());
        drop(listener);
    }

    #[tokio::test]
    async fn test_uds_connect() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");

        let listener = match UdsListener::bind(&path).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return;
                }
                panic!("UdsListener::bind failed: {e:?}");
            }
        };

        // Spawn accept task
        let path_clone = path.clone();
        let accept_handle = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });

        // Connect
        let _conn = match UdsConnection::connect(&path_clone).await {
            Ok(conn) => conn,
            Err(e) => {
                if let TransportError::Io(ref err) = e
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return;
                }
                panic!("UdsConnection::connect failed: {e:?}");
            }
        };
        accept_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_uds_envelope_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");

        let listener = match UdsListener::bind(&path).await {
            Ok(listener) => listener,
            Err(e) => {
                if let TransportError::Io(ref err) = e
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return;
                }
                panic!("UdsListener::bind failed: {e:?}");
            }
        };
        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        // Spawn server
        let server_handle = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            conn.recv().await.unwrap()
        });

        // Client sends
        let mut client = match UdsConnection::connect(&path).await {
            Ok(conn) => conn,
            Err(e) => {
                if let TransportError::Io(ref err) = e
                    && err.kind() == std::io::ErrorKind::PermissionDenied
                {
                    return;
                }
                panic!("UdsConnection::connect failed: {e:?}");
            }
        };
        client.send(&envelope).await.unwrap();

        // Server receives
        let received = server_handle.await.unwrap();
        assert_eq!(received.id, envelope_id);
        assert!(received.verify());
    }
}
