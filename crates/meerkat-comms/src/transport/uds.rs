//! Unix domain socket transport for Meerkat comms.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::Framed;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};

use super::TransportError;
use super::codec::{EnvelopeFrame, TransportCodec};
use crate::types::Envelope;

/// How long the dial of an existing socket file may take before the path
/// counts as held. A dial of a Unix socket answers at once (connected,
/// refused, or the listener's backlog is full), so the bound is a guard
/// against a hang and is not reached in practice.
const LIVE_LISTENER_DIAL_TIMEOUT: Duration = Duration::from_secs(2);

/// The socket paths this process's listeners are bound to, with the number
/// of listeners on each. An entry lasts until its listener is closed.
static OWN_SOCKET_PATHS: LazyLock<parking_lot::Mutex<HashMap<PathBuf, usize>>> =
    LazyLock::new(Default::default);

/// One listener's entry in [`OWN_SOCKET_PATHS`].
struct OwnSocketPath(PathBuf);

impl OwnSocketPath {
    fn enter(path: &Path) -> Self {
        let mut paths = OWN_SOCKET_PATHS.lock();
        let count = paths.entry(path.to_path_buf()).or_default();
        *count = count.saturating_add(1);
        Self(path.to_path_buf())
    }

    fn is_entered(path: &Path) -> bool {
        OWN_SOCKET_PATHS.lock().contains_key(path)
    }
}

impl Drop for OwnSocketPath {
    fn drop(&mut self) {
        let mut paths = OWN_SOCKET_PATHS.lock();
        if let Some(count) = paths.get_mut(&self.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                paths.remove(&self.0);
            }
        }
    }
}

/// A listener bound to a socket path.
///
/// [`Self::bind`] removes a socket file that a listener which has ended left
/// at the path, as a restarted host needs. A live listener at the path is
/// kept: the bind is refused with [`std::io::ErrorKind::AddrInUse`], the kind
/// a taken TCP address gives, and an error that names the path. The one
/// exception is a listener of this same process, which a new listener
/// replaces as before: a runtime's end aborts its listener without waiting
/// for it, so a runtime rebuilt in one process can meet its predecessor's
/// listener still open.
pub(crate) struct BoundSocketListener {
    // Field order: the listener is closed before its entry goes.
    listener: UnixListener,
    _own: OwnSocketPath,
}

impl BoundSocketListener {
    pub(crate) async fn bind(path: &Path) -> std::io::Result<Self> {
        if !OwnSocketPath::is_entered(path) && has_live_listener(path).await {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("socket path {} is held by a live listener", path.display()),
            ));
        }
        // Remove existing socket file if present (without blocking the executor).
        if let Err(err) = tokio::fs::remove_file(path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            return Err(err);
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            _own: OwnSocketPath::enter(path),
        })
    }

    pub(crate) async fn accept(&self) -> std::io::Result<UnixStream> {
        let (stream, _) = self.listener.accept().await?;
        Ok(stream)
    }
}

/// Whether a listener answers a dial of `path`. A connection is an answer;
/// so is a full backlog, and so is a dial that neither connects nor fails
/// within [`LIVE_LISTENER_DIAL_TIMEOUT`]. A missing file, a socket file
/// nobody listens on and any other dial error are no answer.
async fn has_live_listener(path: &Path) -> bool {
    match tokio::time::timeout(LIVE_LISTENER_DIAL_TIMEOUT, UnixStream::connect(path)).await {
        Ok(Ok(_connected)) => true,
        Ok(Err(error)) => error.kind() == std::io::ErrorKind::WouldBlock,
        Err(_elapsed) => true,
    }
}

/// A UDS listener for accepting incoming connections.
pub struct UdsListener {
    listener: BoundSocketListener,
}

impl UdsListener {
    /// Bind to a Unix domain socket path.
    ///
    /// A socket file no listener holds is removed first. A path a live
    /// listener of another process holds is refused with
    /// [`std::io::ErrorKind::AddrInUse`].
    pub async fn bind(path: &Path) -> Result<Self, TransportError> {
        let listener = BoundSocketListener::bind(path).await?;
        Ok(Self { listener })
    }

    /// Accept an incoming connection.
    pub async fn accept(&self) -> Result<UdsConnection, TransportError> {
        let stream = self.listener.accept().await?;
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
