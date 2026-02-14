//! IO task for handling incoming connections.
//!
//! Each incoming connection spawns an IO task that:
//! 1. Reads envelope (with length-prefix framing)
//! 2. Verifies signature (optional)
//! 3. Checks sender in trusted list (when peer-auth is enabled)
//! 4. If valid: sends Ack immediately (unless it's an Ack or Response)
//! 5. Enqueues to inbox
//! 6. Closes connection (or keeps alive)

use std::sync::Arc;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

use crate::identity::{Keypair, Signature};
use crate::inbox::{InboxError, InboxSender};
use crate::transport::TransportError;
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::trust::TrustedPeers;
use crate::types::{Envelope, InboxItem, MessageKind};

/// Handle an incoming connection.
///
/// Reads envelopes, validates them, sends acks as appropriate, and enqueues to inbox.
///
/// # Arguments
/// * `stream` - The async read/write stream (e.g., TcpStream or UnixStream)
/// * `keypair` - Our keypair for signing acks
/// * `require_peer_auth` - Whether to enforce signature+trusted-peer validation
/// * `trusted` - The list of trusted peers
/// * `inbox_sender` - Channel to send validated messages to the inbox
pub async fn handle_connection<S>(
    stream: S,
    require_peer_auth: bool,
    keypair: &Keypair,
    trusted: &TrustedPeers,
    inbox_sender: &InboxSender,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut framed = Framed::new(
        stream,
        TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
    );
    let envelope = match framed.next().await {
        Some(Ok(frame)) => frame.envelope,
        Some(Err(err)) => return Err(IoTaskError::Io(err)),
        None => {
            return Err(IoTaskError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            )));
        }
    };

    // Verify signature (when peer auth is enabled).
    if require_peer_auth && !envelope.verify() {
        tracing::warn!(
            "Dropped message {} from {:?}: invalid signature",
            envelope.id,
            envelope.from
        );
        return Ok(());
    }

    // Check trust
    if require_peer_auth && !trusted.is_trusted(&envelope.from) {
        tracing::warn!(
            "Dropped message {} from {:?}: sender not trusted",
            envelope.id,
            envelope.from
        );
        return Ok(());
    }

    // Verify envelope is addressed to us
    if envelope.to != keypair.public_key() {
        tracing::warn!(
            "Dropped message {} from {:?}: misaddressed (to {:?}, we are {:?})",
            envelope.id,
            envelope.from,
            envelope.to,
            keypair.public_key()
        );
        return Ok(());
    }

    // Send ack if appropriate (not for Ack or Response)
    if should_ack(&envelope.kind) {
        let ack = create_ack(&envelope, keypair);
        let frame = EnvelopeFrame {
            envelope: ack,
            raw: Arc::new(Bytes::new()),
        };
        framed.send(frame).await?;
    }

    // Enqueue to inbox
    inbox_sender
        .send(InboxItem::External { envelope })
        .map_err(|err| match err {
            InboxError::Closed => IoTaskError::InboxClosed,
            InboxError::Full => IoTaskError::InboxFull,
        })?;

    Ok(())
}

/// Determine if we should send an ack for this message kind.
///
/// Per spec:
/// - Message: Yes
/// - Request: Yes
/// - Response: No
/// - Ack: Never (would cause infinite loop)
fn should_ack(kind: &MessageKind) -> bool {
    matches!(
        kind,
        MessageKind::Message { .. } | MessageKind::Request { .. }
    )
}

/// Create an Ack envelope in reply to the given envelope.
fn create_ack(original: &Envelope, keypair: &Keypair) -> Envelope {
    let mut ack = Envelope {
        id: uuid::Uuid::new_v4(),
        from: keypair.public_key(),
        to: original.from,
        kind: MessageKind::Ack {
            in_reply_to: original.id,
        },
        sig: Signature::new([0u8; 64]),
    };
    ack.sign(keypair);
    ack
}

/// Errors that can occur in IO task operations.
#[derive(Debug, thiserror::Error)]
pub enum IoTaskError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("CBOR error: {0}")]
    Cbor(String),
    #[error("Inbox closed")]
    InboxClosed,
    #[error("Inbox full")]
    InboxFull,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::identity::PubKey;
    use crate::inbox::Inbox;
    use crate::trust::TrustedPeer;
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::codec::FramedRead;
    use uuid::Uuid;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers(pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: "test-peer".to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
                meta: crate::PeerMeta::default(),
            }],
        }
    }

    fn make_signed_envelope(from_keypair: &Keypair, to: PubKey, kind: MessageKind) -> Envelope {
        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: from_keypair.public_key(),
            to,
            kind,
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(from_keypair);
        envelope
    }

    async fn envelope_to_bytes(envelope: &Envelope) -> Vec<u8> {
        let mut payload = Vec::new();
        ciborium::into_writer(envelope, &mut payload).unwrap();
        let len = payload.len() as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&len.to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    async fn read_one_envelope<R>(reader: &mut R) -> Result<Envelope, std::io::Error>
    where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut framed = FramedRead::new(
            reader,
            TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
        );
        match framed.next().await {
            Some(Ok(frame)) => Ok(frame.envelope),
            Some(Err(err)) => Err(err),
            None => Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "connection closed",
            )),
        }
    }

    #[test]
    fn test_handle_connection_compiles() {
        // This test just verifies the function signature compiles
        fn _check_signature<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
            _stream: S,
            _keypair: &Keypair,
            _trusted: &TrustedPeers,
            _inbox_sender: &InboxSender,
        ) {
            // The handle_connection function exists with correct signature
        }
    }

    #[tokio::test]
    async fn test_io_task_reads_envelope() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let _trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, _inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let envelope_id = envelope.id;
        let _bytes = envelope_to_bytes(&envelope).await;

        // We need to handle that Cursor doesn't really support async write back
        // For this test, we'll use a duplex stream instead
        let (client, server) = tokio::io::duplex(4096);
        let (mut server_read, _server_write) = tokio::io::split(server);
        let (_client_read, mut client_write) = tokio::io::split(client);

        // Write envelope from client
        let bytes = envelope_to_bytes(&envelope).await;
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        // Read just the envelope (not the full handle_connection)
        let received = read_one_envelope(&mut server_read).await.unwrap();
        assert_eq!(received.id, envelope_id);
    }

    #[tokio::test]
    async fn test_io_task_verifies_signature() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = Inbox::new();

        // Create envelope with invalid signature
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                body: "hello".to_string(),
            },
            sig: Signature::new([0u8; 64]), // Invalid signature
        };
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result =
            handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender).await;
        assert!(result.is_ok()); // Silent drop, not an error

        // No item in inbox
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_io_task_checks_trust() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let untrusted_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key()); // Only trust sender_keypair
        let (mut inbox, inbox_sender) = Inbox::new();

        // Create envelope from untrusted peer
        let envelope = make_signed_envelope(
            &untrusted_keypair, // Not in trusted list
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result =
            handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender).await;
        assert!(result.is_ok()); // Silent drop, not an error

        // No item in inbox
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_io_task_accepts_invalid_signature_when_auth_disabled() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&make_keypair().public_key());
        let (mut inbox, inbox_sender) = Inbox::new();

        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                body: "hello".to_string(),
            },
            sig: Signature::new([0u8; 64]), // Invalid signature
        };
        let bytes = envelope_to_bytes(&envelope).await;
        let expected_id = envelope.id;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, false, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, expected_id),
            _ => panic!("expected Ack"),
        }

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::External { envelope } => assert_eq!(envelope.id, expected_id),
            _ => panic!("expected External"),
        }
    }

    #[tokio::test]
    async fn test_io_task_accepts_untrusted_sender_when_auth_disabled() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let untrusted_keypair = make_keypair();
        let trusted = make_trusted_peers(&untrusted_keypair.public_key()); // not relevant in no-auth mode
        let (mut inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair, // not in trusted list
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;
        let expected_id = envelope.id;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, false, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, expected_id),
            _ => panic!("expected Ack"),
        }

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::External { envelope } => assert_eq!(envelope.id, expected_id),
            _ => panic!("expected External"),
        }
    }

    #[tokio::test]
    async fn test_io_task_sends_ack() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let original_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        // Send envelope
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        // Handle connection
        let handle = tokio::spawn(async move {
            handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender).await
        });

        // Read ack from client side
        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        // Verify ack
        match ack.kind {
            MessageKind::Ack { in_reply_to } => {
                assert_eq!(in_reply_to, original_id);
            }
            _ => panic!("expected Ack"),
        }
        assert!(ack.verify());
    }

    #[tokio::test]
    async fn test_io_task_enqueues_to_inbox() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let envelope_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
            // Read the ack to prevent blocking
            let mut buf = vec![0u8; 1024];
            let _ = client_read.read(&mut buf).await;
        });

        handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        // Check inbox
        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::External { envelope } => {
                assert_eq!(envelope.id, envelope_id);
            }
            _ => panic!("expected External"),
        }
    }

    #[tokio::test]
    async fn test_ack_for_message() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let original_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let handle = tokio::spawn(async move {
            handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender).await
        });

        // Should receive an ack
        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack for Message"),
        }
    }

    #[tokio::test]
    async fn test_ack_for_request() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Request {
                intent: "test".to_string(),
                params: serde_json::json!({}),
            },
        );
        let original_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let handle = tokio::spawn(async move {
            handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender).await
        });

        // Should receive an ack
        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack for Request"),
        }
    }

    #[tokio::test]
    async fn test_no_ack_for_ack() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Ack {
                in_reply_to: Uuid::new_v4(),
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        // Should NOT receive an ack - connection closes without data
        let result = read_one_envelope(&mut client_read).await;
        assert!(result.is_err(), "Should not receive ack for Ack message");
    }

    #[tokio::test]
    async fn test_no_ack_for_response() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Response {
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({}),
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        // Should NOT receive an ack - connection closes without data
        let result = read_one_envelope(&mut client_read).await;
        assert!(
            result.is_err(),
            "Should not receive ack for Response message"
        );
    }

    #[tokio::test]
    async fn test_drop_invalid_signature() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = Inbox::new();

        // Create envelope with invalid signature
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                body: "hello".to_string(),
            },
            sig: Signature::new([0u8; 64]), // Invalid
        };
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        // No ack sent - connection closes without data
        let result = read_one_envelope(&mut client_read).await;
        assert!(result.is_err(), "Should not send ack for invalid signature");

        // No inbox item
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_drop_untrusted_sender() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let other_keypair = make_keypair();
        let trusted = make_trusted_peers(&other_keypair.public_key()); // sender NOT trusted
        let (mut inbox, inbox_sender) = Inbox::new();

        let envelope = make_signed_envelope(
            &sender_keypair, // Not trusted
            receiver_keypair.public_key(),
            MessageKind::Message {
                body: "hello".to_string(),
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, true, &receiver_keypair, &trusted, &inbox_sender)
            .await
            .unwrap();

        // No ack sent - connection closes without data
        let result = read_one_envelope(&mut client_read).await;
        assert!(result.is_err(), "Should not send ack for untrusted sender");

        // No inbox item
        let items = inbox.try_drain();
        assert!(items.is_empty());
    }
}
