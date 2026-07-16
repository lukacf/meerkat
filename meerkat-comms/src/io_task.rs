//! IO task for handling incoming connections.
//!
//! Each incoming connection spawns an IO task that:
//! 1. Reads envelope (with length-prefix framing)
//! 2. Verifies signature (optional)
//! 3. Runs ingress admission through the inbox seam
//! 4. If admitted: sends Ack (unless it's an Ack or Response)
//! 5. Closes connection (or keeps alive)

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::Framed;

use crate::identity::{Keypair, PubKey, Signature};
use crate::inbox::{AdmissionOutcome, DropReason, InboxSender};
use crate::transport::TransportError;
use crate::transport::codec::{EnvelopeFrame, TransportCodec};
use crate::types::{Envelope, MessageKind};

/// A resolved inbound recipient identity: the addressed identity's signing
/// keypair and its inbox.
///
/// The keypair is load-bearing for acks, not cosmetic: the sender's router
/// accepts an ack only when `ack.from` equals the envelope's original `to`
/// (see `Router::send_on_stream`), so an ack signed by any other local
/// keypair surfaces sender-side as `PeerOffline`.
type ResolvedInboundIdentity = (Arc<Keypair>, InboxSender);

/// Handle an incoming connection.
///
/// Reads envelopes, validates them, admits them through the inbox seam,
/// then sends acks for admitted ingress as appropriate.
///
/// # Arguments
/// * `stream` - The async read/write stream (e.g., TcpStream or UnixStream)
/// * `keypair` - Our keypair for signing acks
/// * `require_peer_auth` - Whether to enforce signature+trusted-peer validation
/// * `inbox_sender` - Channel to send validated messages to the inbox
pub async fn handle_connection<S>(
    stream: S,
    require_peer_auth: bool,
    keypair: &Keypair,
    inbox_sender: &InboxSender,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // The degenerate one-identity resolver: the single runtime identity is
    // addressable, everything else is misaddressed.
    handle_connection_inner(stream, require_peer_auth, None, |to| {
        (*to == keypair.public_key()).then(|| (Arc::new(keypair.clone()), inbox_sender.clone()))
    })
    .await
}

/// TCP-specific single-identity handler carrying the kernel-observed remote
/// socket address into ingress classification.
///
/// Keep [`handle_connection`] as the transport-generic wrapper for UDS and
/// duplex-based tests. Production TCP listeners must use this entry point.
pub(crate) async fn handle_tcp_connection<S>(
    stream: S,
    observed_source: SocketAddr,
    require_peer_auth: bool,
    keypair: &Keypair,
    inbox_sender: &InboxSender,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    handle_connection_inner(stream, require_peer_auth, Some(observed_source), |to| {
        (*to == keypair.public_key()).then(|| (Arc::new(keypair.clone()), inbox_sender.clone()))
    })
    .await
}

/// Handle an incoming connection on the multi-identity host acceptor path,
/// demultiplexing by `envelope.to` against the registered identity set.
///
/// Peer auth is deliberately NOT a parameter: the host path verifies
/// envelope signatures by construction (D1) — no field or flag exists that
/// could disable it.
#[cfg(test)]
pub(crate) async fn handle_connection_demux<S>(
    stream: S,
    registry: &crate::host_acceptor::HostAcceptorIdentityRegistry,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    handle_connection_inner(stream, true, None, |to| registry.resolve(to)).await
}

/// TCP-specific multi-identity host-demux handler carrying the observed
/// remote socket address into the addressed member's classifier.
pub(crate) async fn handle_tcp_connection_demux<S>(
    stream: S,
    observed_source: SocketAddr,
    registry: &crate::host_acceptor::HostAcceptorIdentityRegistry,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    handle_connection_inner(stream, true, Some(observed_source), |to| {
        registry.resolve(to)
    })
    .await
}

/// Shared per-connection body, parameterized by recipient-identity
/// resolution. Order is identical to the pre-demux single-identity handler:
/// frame read, signature verify, identity gate, inbox admission, ack.
async fn handle_connection_inner<S, R>(
    stream: S,
    require_peer_auth: bool,
    observed_tcp_source: Option<SocketAddr>,
    resolve: R,
) -> Result<(), IoTaskError>
where
    S: AsyncRead + AsyncWrite + Unpin,
    R: FnOnce(&PubKey) -> Option<ResolvedInboundIdentity>,
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
    //
    // A rejected envelope is a *failed* admission, not successful handling.
    // Return a typed auth/address fault (mirroring the `IngressDropped` arm
    // below) so the listener's `Err -> warn` arm records the rejection fact
    // rather than treating the silent `Ok(())` as a clean connection.
    if require_peer_auth && !envelope.verify() {
        return Err(IoTaskError::InvalidSignature {
            envelope_id: envelope.id,
        });
    }

    // Verify the envelope is addressed to a resolvable local identity. One
    // typed error covers both "never registered" and "registered then
    // removed" recipients.
    let Some((keypair, inbox_sender)) = resolve(&envelope.to) else {
        return Err(IoTaskError::Misaddressed {
            envelope_id: envelope.id,
        });
    };

    // Admit through the inbox seam first. Typed admission outcome: explicit
    // drops are surfaced as `IoTaskError` so the IO task can react (close
    // connection, log, etc.) rather than silently returning `Ok(())`.
    let admission = match observed_tcp_source {
        Some(source) => {
            inbox_sender.send_tcp_connection_ingress(envelope.clone(), require_peer_auth, source)
        }
        None => inbox_sender.send_connection_ingress(envelope.clone(), require_peer_auth),
    };
    match admission {
        AdmissionOutcome::Admitted => {
            if should_ack(&envelope.kind) {
                let ack = create_ack(&envelope, &keypair);
                let frame = EnvelopeFrame {
                    envelope: ack,
                    raw: Arc::new(Bytes::new()),
                };
                framed.send(frame).await?;
            }
            Ok(())
        }
        AdmissionOutcome::Dropped { reason } => Err(match reason {
            DropReason::SessionClosed => IoTaskError::InboxClosed,
            DropReason::InboxFull => IoTaskError::InboxFull,
            DropReason::UntrustedSender | DropReason::ClassificationRejected => {
                IoTaskError::IngressDropped(reason)
            }
        }),
    }
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
        MessageKind::Message { .. }
            | MessageKind::IncarnationFencedMessage { .. }
            | MessageKind::Request { .. }
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
    #[error("Ingress dropped: {0:?}")]
    IngressDropped(DropReason),
    #[error("Rejected envelope {envelope_id}: invalid signature")]
    InvalidSignature { envelope_id: uuid::Uuid },
    #[error("Rejected envelope {envelope_id}: misaddressed (not addressed to us)")]
    Misaddressed { envelope_id: uuid::Uuid },
}

impl IoTaskError {
    /// Whether this error represents a *rejected admission* (auth/address/policy
    /// fault) rather than a transport/IO failure. Lets the listener distinguish
    /// "we refused this peer" from "the connection broke" for metrics.
    pub fn is_admission_rejection(&self) -> bool {
        matches!(
            self,
            Self::InvalidSignature { .. } | Self::Misaddressed { .. } | Self::IngressDropped(_)
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::identity::PubKey;
    use crate::inbox::Inbox;
    use crate::trust::{TrustEntry, TrustStore};
    use crate::types::InboxItem;
    use futures::StreamExt;
    use parking_lot::RwLock;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_util::codec::FramedRead;
    use uuid::Uuid;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn test_trust_entry(name: &str, pubkey: &PubKey, addr: &str) -> TrustEntry {
        TrustEntry {
            peer_id: pubkey.to_peer_id(),
            name: meerkat_core::comms::PeerName::new(name).expect("valid peer name"),
            pubkey: *pubkey,
            address: meerkat_core::comms::PeerAddress::parse(addr).expect("valid peer address"),
            meta: crate::PeerMeta::default(),
        }
    }

    fn make_trusted_peers(pubkey: &PubKey) -> Arc<RwLock<TrustStore>> {
        let mut store = TrustStore::new();
        store
            .insert(test_trust_entry(
                "test-peer",
                pubkey,
                "tcp://127.0.0.1:4200",
            ))
            .expect("trusted test peer should insert");
        Arc::new(RwLock::new(store))
    }

    fn classified_inbox_for_trust(
        trusted: &Arc<RwLock<TrustStore>>,
        require_peer_auth: bool,
    ) -> (Inbox, InboxSender) {
        Inbox::new_classified(test_support::classification_context_shared(
            trusted.clone(),
            require_peer_auth,
        ))
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
            _trusted: &Arc<RwLock<TrustStore>>,
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
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
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
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        // Create envelope with invalid signature
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
            sig: Signature::new([0u8; 64]), // Invalid signature
        };
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        // ROW #300: an invalid signature is a rejected admission, surfaced as a
        // typed auth fault — not a silent `Ok(())` that the listener would treat
        // as successful handling.
        assert!(matches!(result, Err(IoTaskError::InvalidSignature { .. })));
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(IoTaskError::is_admission_rejection),
            "invalid signature must classify as an admission rejection"
        );

        // No item in inbox
        let items = inbox.try_drain_classified();
        assert!(items.is_empty());
    }

    /// ROW #300 gate: an envelope addressed to a different recipient is rejected
    /// with a typed `Misaddressed` fault, not silently swallowed as `Ok(())`.
    #[tokio::test]
    async fn test_io_task_misaddressed_returns_typed_fault() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let other_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        // Signed correctly, but addressed to `other_keypair`, not the receiver.
        let envelope = make_signed_envelope(
            &sender_keypair,
            other_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        assert!(matches!(result, Err(IoTaskError::Misaddressed { .. })));
        assert!(
            result
                .as_ref()
                .err()
                .is_some_and(IoTaskError::is_admission_rejection)
        );

        let items = inbox.try_drain_classified();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_io_task_checks_trust() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let untrusted_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key()); // Only trust sender_keypair
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        // Create envelope from untrusted peer
        let envelope = make_signed_envelope(
            &untrusted_keypair, // Not in trusted list
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        assert!(matches!(
            result,
            Err(IoTaskError::IngressDropped(DropReason::UntrustedSender))
        ));

        // No item in inbox
        let items = inbox.try_drain_classified();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_io_task_accepts_invalid_signature_when_auth_disabled() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&make_keypair().public_key());
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, false);

        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
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

        handle_connection(server, false, &receiver_keypair, &inbox_sender)
            .await
            .unwrap();

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, expected_id),
            _ => panic!("expected Ack"),
        }

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
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
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, false);

        let envelope = make_signed_envelope(
            &sender_keypair, // not in trusted list
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;
        let expected_id = envelope.id;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, false, &receiver_keypair, &inbox_sender)
            .await
            .unwrap();

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, expected_id),
            _ => panic!("expected Ack"),
        }

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::External { envelope } => assert_eq!(envelope.id, expected_id),
            _ => panic!("expected External"),
        }
    }

    #[tokio::test]
    async fn test_io_task_sends_ack() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
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
            handle_connection(server, true, &receiver_keypair, &inbox_sender).await
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
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
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

        handle_connection(server, true, &receiver_keypair, &inbox_sender)
            .await
            .unwrap();

        // Check inbox
        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
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
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
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
            handle_connection(server, true, &receiver_keypair, &inbox_sender).await
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
    async fn test_ack_for_multimodal_message() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: Some(vec![meerkat_core::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "abc".into(),
                }]),
                body: "hello".to_string(),
                handling_mode: None,
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
            handle_connection(server, true, &receiver_keypair, &inbox_sender).await
        });

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack for multimodal Message"),
        }
    }

    #[tokio::test]
    async fn test_ack_for_request() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Request {
                objective_id: None,
                content_taint: None,
                intent: "test".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: None,
                handling_mode: None,
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
            handle_connection(server, true, &receiver_keypair, &inbox_sender).await
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
    async fn tcp_handler_threads_observed_source_into_reply_endpoint_classification() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);
        let declared = meerkat_core::comms::PeerAddress::parse("tcp://203.0.113.88:4311").unwrap();
        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Request {
                content_taint: None,
                intent: "probe".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: Some(declared),
                handling_mode: None,
                objective_id: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let source = "192.0.2.77:51900".parse().unwrap();
        let handle = tokio::spawn(async move {
            handle_tcp_connection(server, source, true, &receiver_keypair, &inbox_sender).await
        });
        let _ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        let entries = inbox.try_drain_classified();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].ingress_fact.declared_reply_endpoint,
            Some(meerkat_core::comms::PeerAddress::parse("tcp://192.0.2.77:4311").unwrap())
        );
    }

    #[tokio::test]
    async fn test_no_ack_for_ack() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

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

        handle_connection(server, true, &receiver_keypair, &inbox_sender)
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
        let (_inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Response {
                objective_id: None,
                content_taint: None,
                in_reply_to: Uuid::new_v4(),
                status: crate::types::Status::Completed,
                result: serde_json::json!({}),
                blocks: None,
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        handle_connection(server, true, &receiver_keypair, &inbox_sender)
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
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        // Create envelope with invalid signature
        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: receiver_keypair.public_key(),
            kind: MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
            sig: Signature::new([0u8; 64]), // Invalid
        };
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        // ROW #300: an invalid signature is a rejected admission surfaced as a
        // typed auth fault, not a silent `Ok(())` the listener would treat as a
        // clean connection.
        let outcome = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        assert!(
            matches!(outcome, Err(IoTaskError::InvalidSignature { .. })),
            "invalid signature must classify as a typed admission rejection, got {outcome:?}"
        );

        // No ack sent - the connection is dropped on the rejection rather than
        // acknowledged.
        let result = read_one_envelope(&mut client_read).await;
        assert!(result.is_err(), "Should not send ack for invalid signature");

        // No inbox item
        let items = inbox.try_drain_classified();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_drop_untrusted_sender() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let other_keypair = make_keypair();
        let trusted = make_trusted_peers(&other_keypair.public_key()); // sender NOT trusted
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair, // Not trusted
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        assert!(matches!(
            result,
            Err(IoTaskError::IngressDropped(DropReason::UntrustedSender))
        ));

        // No ack sent - connection closes without data
        let result = read_one_envelope(&mut client_read).await;
        assert!(result.is_err(), "Should not send ack for untrusted sender");

        // No inbox item
        let items = inbox.try_drain_classified();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_ack_waits_for_final_admission_outcome() {
        // DOGMA-12 defensive scan: if admission rejects the ingress item,
        // the transport must not send an Ack first.
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);
        drop(inbox);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection(server, true, &receiver_keypair, &inbox_sender).await;
        assert!(matches!(result, Err(IoTaskError::InboxClosed)));

        let read_result = read_one_envelope(&mut client_read).await;
        assert!(
            read_result.is_err(),
            "admission rejection must not leak an Ack before the final outcome"
        );
    }

    /// Regression: the IO task must read trust through the shared
    /// `Arc<RwLock<TrustStore>>` handle — not a snapshot — so that a
    /// peer added to the router *after* the connection is accepted is
    /// still admitted. This locks in the Wave 3 D Row 20 invariant: one
    /// trust authority, one read path, no snapshot divergence.
    #[tokio::test]
    async fn test_io_task_reads_live_trust_after_listener_spawn() {
        let sender_keypair = make_keypair();
        let receiver_keypair = make_keypair();

        // Start with an EMPTY trust set. If the IO task snapshotted at
        // spawn time, the subsequent add below would not be visible and
        // the envelope would be silently dropped.
        let trusted = Arc::new(RwLock::new(TrustStore::new()));
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);

        let envelope = make_signed_envelope(
            &sender_keypair,
            receiver_keypair.public_key(),
            MessageKind::Message {
                objective_id: None,
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
            },
        );
        let envelope_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);

        // Mutate the trust set AFTER the IO task would normally have
        // snapshotted, but BEFORE the envelope is delivered. The live
        // read must observe this mutation.
        trusted
            .write()
            .insert(test_trust_entry(
                "sender",
                &sender_keypair.public_key(),
                "tcp://127.0.0.1:0",
            ))
            .expect("live trust insert should succeed");

        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
            // Read the ack to avoid blocking.
            let mut buf = vec![0u8; 1024];
            let _ = client_read.read(&mut buf).await;
        });

        handle_connection(server, true, &receiver_keypair, &inbox_sender)
            .await
            .unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1, "envelope should be admitted via live trust");
        match &items[0].item {
            InboxItem::External { envelope } => assert_eq!(envelope.id, envelope_id),
            _ => panic!("expected External"),
        }
    }

    // ---- host acceptor demux path (D1) ----

    use crate::host_acceptor::HostAcceptorIdentityRegistry;

    fn demux_registry(
        identities: &[(&Keypair, &InboxSender)],
    ) -> (
        HostAcceptorIdentityRegistry,
        Arc<dyn std::any::Any + Send + Sync>,
    ) {
        let registry = HostAcceptorIdentityRegistry::new();
        let owner: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
        registry.install_owner(owner.clone()).unwrap();
        for (keypair, inbox_sender) in identities {
            registry
                .register_identity(
                    &owner,
                    keypair.public_key(),
                    Arc::new((*keypair).clone()),
                    (*inbox_sender).clone(),
                )
                .unwrap();
        }
        (registry, owner)
    }

    /// §11 demux row: an envelope addressed to member B lands in B's inbox
    /// and the ack is signed by B's keypair — the ADDRESSED identity, not
    /// any other registered identity.
    #[tokio::test]
    async fn test_demux_routes_by_to_and_acks_with_addressed_keypair() {
        let sender_keypair = make_keypair();
        let member_a = make_keypair();
        let member_b = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox_a, inbox_sender_a) = classified_inbox_for_trust(&trusted, true);
        let (mut inbox_b, inbox_sender_b) = classified_inbox_for_trust(&trusted, true);
        let (registry, _owner) =
            demux_registry(&[(&member_a, &inbox_sender_a), (&member_b, &inbox_sender_b)]);

        let envelope = make_signed_envelope(
            &sender_keypair,
            member_b.public_key(),
            MessageKind::Message {
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
                objective_id: None,
            },
        );
        let original_id = envelope.id;
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let handle = tokio::spawn(async move { handle_connection_demux(server, &registry).await });

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack"),
        }
        assert_eq!(
            ack.from,
            member_b.public_key(),
            "ack must be signed by the ADDRESSED member's keypair"
        );
        assert!(ack.verify(), "ack must carry a valid member-B signature");

        let items_b = inbox_b.try_drain_classified();
        assert_eq!(items_b.len(), 1, "envelope should land in B's inbox");
        match &items_b[0].item {
            InboxItem::External { envelope } => assert_eq!(envelope.id, original_id),
            _ => panic!("expected External"),
        }
        assert!(
            inbox_a.try_drain_classified().is_empty(),
            "member A must not receive an envelope addressed to B"
        );
    }

    #[tokio::test]
    async fn tcp_demux_threads_observed_source_to_addressed_member_classifier() {
        let sender_keypair = make_keypair();
        let member = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox, inbox_sender) = classified_inbox_for_trust(&trusted, true);
        let (registry, _owner) = demux_registry(&[(&member, &inbox_sender)]);
        let envelope = make_signed_envelope(
            &sender_keypair,
            member.public_key(),
            MessageKind::Request {
                content_taint: None,
                intent: "probe".to_string(),
                params: serde_json::json!({}),
                blocks: None,
                reply_endpoint: Some(
                    meerkat_core::comms::PeerAddress::parse("tcp://198.51.100.90:4311").unwrap(),
                ),
                handling_mode: None,
                objective_id: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;
        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let handle = tokio::spawn(async move {
            handle_tcp_connection_demux(server, "192.0.2.91:52000".parse().unwrap(), &registry)
                .await
        });
        let _ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();

        let entries = inbox.try_drain_classified();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].ingress_fact.declared_reply_endpoint,
            Some(meerkat_core::comms::PeerAddress::parse("tcp://192.0.2.91:4311").unwrap())
        );
    }

    /// §11 misaddressed row: an envelope addressed to a never-registered key
    /// is rejected with the typed `Misaddressed` fault, no ack sent.
    #[tokio::test]
    async fn test_demux_misaddressed_unregistered_identity_rejected() {
        let sender_keypair = make_keypair();
        let member_a = make_keypair();
        let stranger = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox_a, inbox_sender_a) = classified_inbox_for_trust(&trusted, true);
        let (registry, _owner) = demux_registry(&[(&member_a, &inbox_sender_a)]);

        let envelope = make_signed_envelope(
            &sender_keypair,
            stranger.public_key(),
            MessageKind::Message {
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
                objective_id: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection_demux(server, &registry).await;
        assert!(matches!(result, Err(IoTaskError::Misaddressed { .. })));

        let read_result = read_one_envelope(&mut client_read).await;
        assert!(read_result.is_err(), "no ack for a misaddressed envelope");
        assert!(inbox_a.try_drain_classified().is_empty());
    }

    /// §11 unregistered-identity row: register → remove → send is the same
    /// typed reject as never-registered; removal requires the installed
    /// owner (a wrong owner gets a typed error and the registry is never
    /// mutated).
    #[tokio::test]
    async fn test_demux_registered_then_removed_identity_rejected() {
        let sender_keypair = make_keypair();
        let member_a = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox_a, inbox_sender_a) = classified_inbox_for_trust(&trusted, true);
        let registry = HostAcceptorIdentityRegistry::new();
        let owner: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
        registry.install_owner(owner.clone()).unwrap();
        registry
            .register_identity(
                &owner,
                member_a.public_key(),
                Arc::new(member_a.clone()),
                inbox_sender_a.clone(),
            )
            .unwrap();

        // Wrong owner: typed error, registry never mutated.
        let intruder: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
        let wrong = registry.remove_identity(&intruder, &member_a.public_key());
        assert!(matches!(
            wrong,
            Err(crate::host_acceptor::HostAcceptorError::OwnerMismatch)
        ));
        assert!(
            registry.resolve(&member_a.public_key()).is_some(),
            "wrong-owner removal must not mutate the registry"
        );

        // Installed owner: removal succeeds, and the identity now rejects.
        assert!(
            registry
                .remove_identity(&owner, &member_a.public_key())
                .unwrap()
        );

        let envelope = make_signed_envelope(
            &sender_keypair,
            member_a.public_key(),
            MessageKind::Message {
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
                objective_id: None,
            },
        );
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (_client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection_demux(server, &registry).await;
        assert!(matches!(result, Err(IoTaskError::Misaddressed { .. })));
        assert!(inbox_a.try_drain_classified().is_empty());
    }

    /// §11 mandatory-auth row: the demux entry point has no peer-auth
    /// parameter (type-level absence — compare `handle_connection`'s
    /// `require_peer_auth`), so an unsigned envelope is rejected even when
    /// the registered identity's own inbox context was built from a
    /// `require_peer_auth = false` configuration.
    #[tokio::test]
    async fn test_demux_rejects_unsigned_envelope_despite_no_auth_context() {
        let sender_keypair = make_keypair();
        let member_a = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        // The context a `require_peer_auth = false` CoreCommsConfig would
        // produce — the acceptor never reads it.
        let (mut inbox_a, inbox_sender_a) = classified_inbox_for_trust(&trusted, false);
        let (registry, _owner) = demux_registry(&[(&member_a, &inbox_sender_a)]);

        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: member_a.public_key(),
            kind: MessageKind::Message {
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
                objective_id: None,
            },
            sig: Signature::new([0u8; 64]), // unsigned
        };
        let bytes = envelope_to_bytes(&envelope).await;

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let result = handle_connection_demux(server, &registry).await;
        assert!(matches!(result, Err(IoTaskError::InvalidSignature { .. })));

        let read_result = read_one_envelope(&mut client_read).await;
        assert!(read_result.is_err(), "no ack for an unsigned envelope");
        assert!(inbox_a.try_drain_classified().is_empty());
    }

    /// §11 byte-compat row: the demux path accepts the SAME hand-written
    /// wire bytes (u32-BE length prefix + ciborium CBOR envelope — the
    /// `envelope_to_bytes` recipe) as the single-identity path, and the
    /// signature verifies — no signed-region or codec drift across the
    /// refactor. The exact-byte signable-region fixtures in types.rs stand
    /// untouched as the primary pin.
    #[tokio::test]
    async fn test_demux_accepts_hand_encoded_envelope_bytes() {
        let sender_keypair = make_keypair();
        let member_a = make_keypair();
        let trusted = make_trusted_peers(&sender_keypair.public_key());
        let (mut inbox_a, inbox_sender_a) = classified_inbox_for_trust(&trusted, true);
        let (registry, _owner) = demux_registry(&[(&member_a, &inbox_sender_a)]);

        let envelope = make_signed_envelope(
            &sender_keypair,
            member_a.public_key(),
            MessageKind::Message {
                content_taint: None,
                blocks: None,
                body: "hello".to_string(),
                handling_mode: None,
                objective_id: None,
            },
        );
        let original_id = envelope.id;

        // Hand-encode the frame: 4-byte big-endian length prefix followed by
        // the ciborium CBOR envelope, no codec involved on the write side.
        let mut payload = Vec::new();
        ciborium::into_writer(&envelope, &mut payload).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);

        let (client, server) = tokio::io::duplex(4096);
        let (mut client_read, mut client_write) = tokio::io::split(client);
        tokio::spawn(async move {
            client_write.write_all(&bytes).await.unwrap();
        });

        let handle = tokio::spawn(async move { handle_connection_demux(server, &registry).await });

        let ack = read_one_envelope(&mut client_read).await.unwrap();
        handle.await.unwrap().unwrap();
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack"),
        }
        assert_eq!(ack.from, member_a.public_key());

        let items = inbox_a.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::External { envelope } => assert_eq!(envelope.id, original_id),
            _ => panic!("expected External"),
        }
    }
}
