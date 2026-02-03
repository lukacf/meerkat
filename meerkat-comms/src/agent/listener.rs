//! Listener spawning for incoming connections.
//!
//! Provides functions to spawn UDS and TCP listeners that accept connections
//! and feed validated messages into the inbox.

use std::path::Path;
use std::sync::Arc;

use crate::{InboxSender, Keypair, TrustedPeers, handle_connection};
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

/// Handle to a spawned listener task.
///
/// Dropping this handle will NOT cancel the listener - use `abort()` to stop it.
pub struct ListenerHandle {
    handle: JoinHandle<()>,
}

impl ListenerHandle {
    /// Abort the listener task.
    pub fn abort(&self) {
        self.handle.abort();
    }

    /// Check if the listener task is finished.
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Wait for the listener task to finish.
    pub async fn join(self) -> Result<(), tokio::task::JoinError> {
        self.handle.await
    }
}

/// Spawn a Unix Domain Socket listener.
///
/// The listener accepts connections, validates incoming messages, sends acks,
/// and forwards valid messages to the inbox.
///
/// # Arguments
/// * `path` - Path for the UDS socket
/// * `keypair` - Our keypair for signing acks
/// * `trusted` - Trusted peers list for validation (shared for dynamic updates)
/// * `inbox_sender` - Channel to send validated messages to
///
/// # Returns
/// A handle to the spawned listener task.
#[cfg(unix)]
pub async fn spawn_uds_listener(
    path: impl AsRef<Path>,
    keypair: Arc<Keypair>,
    trusted: Arc<RwLock<TrustedPeers>>,
    inbox_sender: InboxSender,
) -> std::io::Result<ListenerHandle> {
    use std::io::ErrorKind;
    use tokio::net::UnixListener;

    let path = path.as_ref().to_path_buf();

    // Remove existing socket file if present.
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let listener = UnixListener::bind(&path)?;

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let keypair = keypair.clone();
                    let trusted = trusted.clone();
                    let inbox_sender = inbox_sender.clone();

                    tokio::spawn(async move {
                        // Get a snapshot of trusted peers for this connection
                        let trusted_snapshot = trusted.read().await.clone();
                        if let Err(e) = handle_connection(
                            stream,
                            keypair.as_ref(),
                            &trusted_snapshot,
                            &inbox_sender,
                        )
                        .await
                        {
                            tracing::warn!("UDS connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("UDS accept error: {}", e);
                    break;
                }
            }
        }
    });

    Ok(ListenerHandle { handle })
}

/// Spawn a TCP listener.
///
/// The listener accepts connections, validates incoming messages, sends acks,
/// and forwards valid messages to the inbox.
///
/// # Arguments
/// * `addr` - Address to bind to (e.g., "127.0.0.1:4200")
/// * `keypair` - Our keypair for signing acks
/// * `trusted` - Trusted peers list for validation (shared for dynamic updates)
/// * `inbox_sender` - Channel to send validated messages to
///
/// # Returns
/// A handle to the spawned listener task.
pub async fn spawn_tcp_listener(
    addr: &str,
    keypair: Arc<Keypair>,
    trusted: Arc<RwLock<TrustedPeers>>,
    inbox_sender: InboxSender,
) -> std::io::Result<ListenerHandle> {
    let listener = TcpListener::bind(addr).await?;

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _addr)) => {
                    let keypair = keypair.clone();
                    let trusted = trusted.clone();
                    let inbox_sender = inbox_sender.clone();

                    tokio::spawn(async move {
                        // Get a snapshot of trusted peers for this connection
                        let trusted_snapshot = trusted.read().await.clone();
                        if let Err(e) = handle_connection(
                            stream,
                            keypair.as_ref(),
                            &trusted_snapshot,
                            &inbox_sender,
                        )
                        .await
                        {
                            tracing::warn!("TCP connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("TCP accept error: {}", e);
                    break;
                }
            }
        }
    });

    Ok(ListenerHandle { handle })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::{Envelope, Inbox, MessageKind, PubKey, Signature, TrustedPeer};
    use tempfile::TempDir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use uuid::Uuid;

    fn make_keypair() -> Keypair {
        Keypair::generate()
    }

    fn make_trusted_peers(name: &str, pubkey: &PubKey) -> TrustedPeers {
        TrustedPeers {
            peers: vec![TrustedPeer {
                name: name.to_string(),
                pubkey: *pubkey,
                addr: "tcp://127.0.0.1:4200".to_string(),
            }],
        }
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

    #[cfg(unix)]
    #[tokio::test]
    async fn test_spawn_uds_listener() {
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("test.sock");

        let sender_keypair = make_keypair();
        let sender_pubkey = sender_keypair.public_key();
        let receiver_keypair = make_keypair();
        let receiver_pubkey = receiver_keypair.public_key();
        let receiver_keypair = Arc::new(receiver_keypair);
        let trusted = make_trusted_peers("sender", &sender_pubkey);

        let (mut inbox, inbox_sender) = Inbox::new();

        let handle = match spawn_uds_listener(
            &sock_path,
            receiver_keypair,
            Arc::new(RwLock::new(trusted)),
            inbox_sender,
        )
        .await
        {
            Ok(handle) => handle,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    return;
                }
                panic!("spawn_uds_listener failed: {e}");
            }
        };

        // Give listener time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Connect and send a message
        let mut stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_pubkey,
            to: receiver_pubkey,
            kind: MessageKind::Message {
                body: "hello from test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender_keypair);
        let envelope_id = envelope.id;

        let bytes = envelope_to_bytes(&envelope).await;
        stream.write_all(&bytes).await.unwrap();

        // Read the ack
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await.unwrap();
        let len = u32::from_be_bytes(len_bytes);
        let mut ack_payload = vec![0u8; len as usize];
        stream.read_exact(&mut ack_payload).await.unwrap();

        // Check inbox received the message
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            crate::InboxItem::External { envelope } => {
                assert_eq!(envelope.id, envelope_id);
            }
            _ => unreachable!("expected External"),
        }

        handle.abort();
    }

    #[tokio::test]
    async fn test_spawn_tcp_listener() {
        let sender_keypair = make_keypair();
        let sender_pubkey = sender_keypair.public_key();
        let receiver_keypair = make_keypair();
        let receiver_pubkey = receiver_keypair.public_key();
        let receiver_keypair = Arc::new(receiver_keypair);
        let trusted = make_trusted_peers("sender", &sender_pubkey);

        let (mut inbox, inbox_sender) = Inbox::new();

        // Use port 0 to get a random available port
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    return;
                }
                panic!("TcpListener::bind failed: {e}");
            }
        };
        let addr = listener.local_addr().unwrap();
        drop(listener); // Release the port

        let handle = match spawn_tcp_listener(
            &addr.to_string(),
            receiver_keypair,
            Arc::new(RwLock::new(trusted)),
            inbox_sender,
        )
        .await
        {
            Ok(handle) => handle,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    return;
                }
                panic!("spawn_tcp_listener failed: {e}");
            }
        };

        // Give listener time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Connect and send a message
        let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_pubkey,
            to: receiver_pubkey,
            kind: MessageKind::Message {
                body: "hello from tcp test".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender_keypair);
        let envelope_id = envelope.id;

        let bytes = envelope_to_bytes(&envelope).await;
        stream.write_all(&bytes).await.unwrap();

        // Read the ack
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await.unwrap();
        let len = u32::from_be_bytes(len_bytes);
        let mut ack_payload = vec![0u8; len as usize];
        stream.read_exact(&mut ack_payload).await.unwrap();

        // Check inbox received the message
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            crate::InboxItem::External { envelope } => {
                assert_eq!(envelope.id, envelope_id);
            }
            _ => unreachable!("expected External"),
        }

        handle.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_comms_manager_listeners() {
        use crate::agent::manager::{CommsManager, CommsManagerConfig};

        let sender_keypair = make_keypair();
        let sender_pubkey = sender_keypair.public_key();
        let receiver_keypair = make_keypair();
        let receiver_pubkey = receiver_keypair.public_key();
        let trusted = make_trusted_peers("sender", &sender_pubkey);

        let config =
            CommsManagerConfig::with_keypair(receiver_keypair).trusted_peers(trusted.clone());

        let mut manager = CommsManager::new(config).unwrap();

        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("manager.sock");

        let keypair = manager.router().keypair_arc();

        // Start listener using manager's components
        let handle = match spawn_uds_listener(
            &sock_path,
            keypair,
            Arc::new(RwLock::new(trusted)),
            manager.inbox_sender().clone(),
        )
        .await
        {
            Ok(handle) => handle,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    return;
                }
                panic!("spawn_uds_listener failed: {e}");
            }
        };

        // Give listener time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Connect and send a message
        let mut stream = tokio::net::UnixStream::connect(&sock_path).await.unwrap();

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_pubkey,
            to: receiver_pubkey,
            kind: MessageKind::Message {
                body: "hello via manager".to_string(),
            },
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender_keypair);

        let bytes = envelope_to_bytes(&envelope).await;
        stream.write_all(&bytes).await.unwrap();

        // Read the ack
        let mut len_bytes = [0u8; 4];
        stream.read_exact(&mut len_bytes).await.unwrap();
        let len = u32::from_be_bytes(len_bytes);
        let mut ack_payload = vec![0u8; len as usize];
        stream.read_exact(&mut ack_payload).await.unwrap();

        // Check manager received the message
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        let messages = manager.drain_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from_peer, "sender");

        handle.abort();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_listener_handle_abort() {
        // Test that ListenerHandle has the expected methods
        let tmp = TempDir::new().unwrap();
        let sock_path = tmp.path().join("abort.sock");
        let keypair = make_keypair();
        let keypair = Arc::new(keypair);
        let trusted = Arc::new(RwLock::new(TrustedPeers::new()));
        let (_, inbox_sender) = Inbox::new();

        let handle = match spawn_uds_listener(&sock_path, keypair, trusted, inbox_sender).await {
            Ok(handle) => handle,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    return;
                }
                panic!("spawn_uds_listener failed: {e}");
            }
        };

        // Give it a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify it's running (listener loop should be active)
        assert!(!handle.is_finished(), "Listener should be running");

        // Abort it
        handle.abort();

        // Give it a moment to abort
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Now it should be finished
        assert!(
            handle.is_finished(),
            "Listener should be finished after abort"
        );
    }
}
