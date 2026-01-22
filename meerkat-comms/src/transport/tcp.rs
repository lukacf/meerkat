//! TCP transport for Meerkat comms.

use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{TransportError, MAX_PAYLOAD_SIZE};
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
        Ok(TcpConnection { stream })
    }
}

/// A TCP connection for sending and receiving envelopes.
pub struct TcpConnection {
    stream: TcpStream,
}

impl TcpConnection {
    /// Connect to a TCP socket address.
    pub async fn connect(addr: SocketAddr) -> Result<Self, TransportError> {
        let stream = TcpStream::connect(addr).await?;
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
        let listener = TcpTransportListener::bind(addr).await.unwrap();
        let local = listener.local_addr().unwrap();
        assert!(local.port() > 0);
    }

    #[tokio::test]
    async fn test_tcp_connect() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpTransportListener::bind(addr).await.unwrap();
        let local = listener.local_addr().unwrap();

        // Spawn accept task
        let accept_handle = tokio::spawn(async move {
            listener.accept().await.unwrap();
        });

        // Connect
        let _conn = TcpConnection::connect(local).await.unwrap();
        accept_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_tcp_envelope_roundtrip() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let listener = TcpTransportListener::bind(addr).await.unwrap();
        let local = listener.local_addr().unwrap();

        let envelope = make_test_envelope();
        let envelope_id = envelope.id;

        // Spawn server
        let server_handle = tokio::spawn(async move {
            let mut conn = listener.accept().await.unwrap();
            let received = conn.recv().await.unwrap();
            received
        });

        // Client sends
        let mut client = TcpConnection::connect(local).await.unwrap();
        client.send(&envelope).await.unwrap();

        // Server receives
        let received = server_handle.await.unwrap();
        assert_eq!(received.id, envelope_id);
        assert!(received.verify());
    }
}
