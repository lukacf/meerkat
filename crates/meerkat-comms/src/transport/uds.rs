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
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello from uds".to_string(),
                handling_mode: None,
                in_reply_to: None,
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

    /// A listener bound at `path` outside this transport, as a listener of
    /// another process is. `None` where the environment refuses a socket
    /// bind.
    fn outside_listener(path: &Path) -> Option<UnixListener> {
        match UnixListener::bind(path) {
            Ok(listener) => Some(listener),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => None,
            Err(error) => panic!("bind {}: {error}", path.display()),
        }
    }

    /// `bind` does not take a socket path from a live listener: it is
    /// refused with an error that names the path, and the listener that held
    /// the path still answers a dial.
    #[tokio::test]
    async fn test_uds_bind_is_refused_on_a_path_a_live_listener_holds() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("held.sock");
        let Some(held) = outside_listener(&path) else {
            return;
        };

        match UdsListener::bind(&path).await {
            Err(TransportError::Io(error)) => {
                assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse, "{error}");
                assert!(
                    error.to_string().contains(&path.display().to_string()),
                    "the error names the path: {error}"
                );
            }
            Err(other) => panic!("bind must be refused as an address in use: {other:?}"),
            Ok(_) => panic!("bind must be refused while a live listener holds the path"),
        }

        let mut dial = UnixStream::connect(&path).await.expect("dial the path");
        dial.write_all(b"x").await.expect("write to the dial");
        let reached = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let (mut stream, _) = held.accept().await.expect("accept");
                let mut byte = [0_u8; 1];
                // A connection closed without a byte is not this dial.
                if matches!(stream.read(&mut byte).await, Ok(1)) && byte == *b"x" {
                    break;
                }
            }
        })
        .await;
        assert!(
            reached.is_ok(),
            "the listener that held the path still answers"
        );
    }

    /// `bind` takes over a socket file no listener holds.
    #[tokio::test]
    async fn test_uds_bind_takes_over_a_socket_file_no_listener_holds() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("ended.sock");
        let Some(ended) = outside_listener(&path) else {
            return;
        };
        drop(ended);
        assert!(path.exists(), "the ended listener left its socket file");

        let _listener = UdsListener::bind(&path)
            .await
            .expect("bind over a socket file no listener holds");
        UnixStream::connect(&path)
            .await
            .expect("a dial reaches the new listener");
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
