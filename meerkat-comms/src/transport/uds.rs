//! Unix domain socket transport for Meerkat comms.

use std::path::Path;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};

use super::{TransportError, MAX_PAYLOAD_SIZE};
use crate::types::Envelope;

/// A UDS listener for accepting incoming connections.
pub struct UdsListener {
    listener: UnixListener,
}

impl UdsListener {
    /// Bind to a Unix domain socket path.
    pub async fn bind(path: &Path) -> Result<Self, TransportError> {
        // Remove existing socket file if present
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path)?;
        Ok(Self { listener })
    }

    /// Accept an incoming connection.
    pub async fn accept(&self) -> Result<UdsConnection, TransportError> {
        let (stream, _) = self.listener.accept().await?;
        Ok(UdsConnection { stream })
    }
}

/// A UDS connection for sending and receiving envelopes.
pub struct UdsConnection {
    stream: UnixStream,
}

impl UdsConnection {
    /// Connect to a Unix domain socket.
    pub async fn connect(path: &Path) -> Result<Self, TransportError> {
        let stream = UnixStream::connect(path).await?;
        Ok(Self { stream })
    }

    /// Send an envelope over the connection.
    pub async fn send(&mut self, envelope: &Envelope) -> Result<(), TransportError> {
        let mut payload = Vec::new();
        ciborium::into_writer(envelope, &mut payload)
            .map_err(|e| TransportError::Cbor(e.to_string()))?;

        let len = payload.len() as u32;
        if len > MAX_PAYLOAD_SIZE {
            return Err(TransportError::MessageTooLarge { size: len });
        }

        self.stream.write_all(&len.to_be_bytes()).await?;
        self.stream.write_all(&payload).await?;
        self.stream.flush().await?;
        Ok(())
    }

    /// Receive an envelope from the connection.
    pub async fn recv(&mut self) -> Result<Envelope, TransportError> {
        let mut len_bytes = [0u8; 4];
        self.stream.read_exact(&mut len_bytes).await?;
        let len = u32::from_be_bytes(len_bytes);

        if len > MAX_PAYLOAD_SIZE {
            return Err(TransportError::MessageTooLarge { size: len });
        }

        let mut payload = vec![0u8; len as usize];
        self.stream.read_exact(&mut payload).await?;

        let envelope: Envelope = ciborium::from_reader(&payload[..])
            .map_err(|e| TransportError::InvalidFrame(format!("CBOR decode error: {e}")))?;

        Ok(envelope)
    }
}

#[cfg(test)]
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

        let listener = UdsListener::bind(&path).await.unwrap();
        assert!(path.exists());
        drop(listener);
    }

    #[tokio::test]
    async fn test_uds_connect() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");

        let listener = UdsListener::bind(&path).await.unwrap();

        // Spawn accept task
        let path_clone = path.clone();
        let accept_handle = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });

        // Connect
        let _conn = UdsConnection::connect(&path_clone).await.unwrap();
        accept_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_uds_envelope_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.sock");

        let listener = UdsListener::bind(&path).await.unwrap();
        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        // Spawn server
        let server_handle = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            let received = conn.recv().await.unwrap();
            received
        });

        // Client sends
        let mut client = UdsConnection::connect(&path).await.unwrap();
        client.send(&envelope).await.unwrap();

        // Server receives
        let received = server_handle.await.unwrap();
        assert_eq!(received.id, envelope_id);
        assert!(received.verify());
    }
}
