//! Gateway Ed25519 keypair — long-lived signing identity for cross-process
//! mob peering.
//!
//! The meta gateway needs a stable Ed25519 keypair so other gateways can
//! validate the signatures on envelopes it sends across TCP / UDS. Inproc
//! peering is unaffected: the in-process router authorises by identity map
//! and never inspects signatures, so an inproc-only gateway can stay on an
//! ephemeral key without persistence.
//!
//! Two construction modes:
//!
//! * [`GatewayPeerKeys::load_or_create`] — pass a state directory. The key
//!   is read from `<state_dir>/peer_key.ed25519` (raw 32-byte secret) if
//!   present; otherwise a fresh keypair is minted and persisted with
//!   `0o600` permissions on Unix.
//! * [`GatewayPeerKeys::ephemeral`] — mint a fresh keypair held in memory
//!   only. Tests and gateway processes that do not own a state directory
//!   use this.
//!
//! The 32-byte raw seed format keeps the on-disk key drop-in compatible
//! with `ed25519-dalek::SigningKey::from_bytes` and lets ops sites swap
//! a key in by writing 32 bytes — no PEM/CBOR parsing required.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{SecretKey, SigningKey, VerifyingKey};

/// File name used inside the gateway state directory.
pub const KEY_FILE_NAME: &str = "peer_key.ed25519";

/// A long-lived Ed25519 signing identity for the local gateway.
///
/// Cheap to clone (the underlying `SigningKey` is small and `Arc`-shared
/// at the wrapper level). Treat as opaque — callers should reach for
/// [`Self::verifying_key`] / [`Self::pubkey_bytes`] / [`Self::pubkey_b64`]
/// rather than touching the secret material.
#[derive(Clone)]
pub struct GatewayPeerKeys {
    inner: Arc<GatewayPeerKeysInner>,
    state_root: Option<Arc<PathBuf>>,
}

struct GatewayPeerKeysInner {
    signing: SigningKey,
    public: VerifyingKey,
}

impl GatewayPeerKeys {
    /// Load the keypair from `<state_dir>/peer_key.ed25519` if present, or
    /// mint a fresh one and persist it.
    ///
    /// The state directory is created if it does not exist. On Unix the
    /// key file is written with `0o600` so other users on the host cannot
    /// impersonate this gateway.
    ///
    /// An installation that already recorded host pairings under
    /// `state_dir` is enrolled: its identity is pinned by its peers, so a
    /// missing key refuses ([`GatewayPeerKeyError::EnrolledWithoutKey`])
    /// instead of minting a replacement.
    pub fn load_or_create(state_dir: &Path) -> Result<Self, GatewayPeerKeyError> {
        let key_path = state_dir.join(KEY_FILE_NAME);
        if key_path.exists() {
            return Self::load_existing(state_dir);
        }
        let pairings = state_dir.join(crate::runtime::remote_host::HOST_PAIRING_FILE_NAME);
        if pairings.exists() {
            return Err(GatewayPeerKeyError::EnrolledWithoutKey { path: key_path });
        }
        #[cfg(unix)]
        let state_root_existed = state_dir.exists();
        fs::create_dir_all(state_dir).map_err(|source| GatewayPeerKeyError::Io {
            path: state_dir.to_path_buf(),
            source,
        })?;
        #[cfg(unix)]
        if !state_root_existed {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(state_dir, fs::Permissions::from_mode(0o700)).map_err(
                |source| GatewayPeerKeyError::Io {
                    path: state_dir.to_path_buf(),
                    source,
                },
            )?;
        }
        let keys = Self::ephemeral();
        keys.persist_to(&key_path)?;
        Ok(Self {
            inner: keys.inner,
            state_root: Some(Arc::new(state_dir.to_path_buf())),
        })
    }

    /// Load the established keypair from `<state_dir>/peer_key.ed25519`,
    /// never minting one.
    pub fn load_existing(state_dir: &Path) -> Result<Self, GatewayPeerKeyError> {
        let key_path = state_dir.join(KEY_FILE_NAME);
        let mut keys = Self::load(&key_path)?;
        keys.state_root = Some(Arc::new(state_dir.to_path_buf()));
        Ok(keys)
    }

    /// Mint a fresh keypair held in memory only.
    ///
    /// Intended for tests and gateway profiles that do not own a state
    /// directory. The pubkey is still stable for the lifetime of the
    /// process — peers that fetch it via `meta/peer_pubkey` will see
    /// a consistent identity until restart.
    pub fn ephemeral() -> Self {
        let mut rng = rand_core::OsRng;
        let signing = SigningKey::generate(&mut rng);
        let public = signing.verifying_key();
        Self {
            inner: Arc::new(GatewayPeerKeysInner { signing, public }),
            state_root: None,
        }
    }

    fn load(path: &Path) -> Result<Self, GatewayPeerKeyError> {
        let bytes = fs::read(path).map_err(|source| GatewayPeerKeyError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        if bytes.len() != 32 {
            return Err(GatewayPeerKeyError::InvalidLength {
                path: path.to_path_buf(),
                actual: bytes.len(),
            });
        }
        let mut secret: SecretKey = [0u8; 32];
        secret.copy_from_slice(&bytes);
        let signing = SigningKey::from_bytes(&secret);
        let public = signing.verifying_key();
        Ok(Self {
            inner: Arc::new(GatewayPeerKeysInner { signing, public }),
            state_root: None,
        })
    }

    fn persist_to(&self, path: &Path) -> Result<(), GatewayPeerKeyError> {
        let bytes = self.inner.signing.to_bytes();
        // On Unix, create the file with mode 0o600 ATOMICALLY (the mode is set
        // at creation, before any bytes are written), so the 32-byte Ed25519
        // secret seed is never momentarily world/group-readable. `fs::write`
        // would create with `0o666 & !umask` (typically 0o644) and only chmod
        // afterwards, leaving a window in which a co-tenant could read the
        // signing key and impersonate this gateway across TCP/UDS.
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .map_err(|source| GatewayPeerKeyError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
            file.write_all(&bytes)
                .map_err(|source| GatewayPeerKeyError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
        }
        #[cfg(not(unix))]
        {
            fs::write(path, bytes).map_err(|source| GatewayPeerKeyError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        }
        Ok(())
    }

    /// 32-byte Ed25519 verifying key, suitable for stamping onto a
    /// `TrustedPeerDescriptor`.
    pub fn pubkey_bytes(&self) -> [u8; 32] {
        self.inner.public.to_bytes()
    }

    /// Borrow the verifying key.
    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.inner.public
    }

    /// Borrow the signing key. Tests use this to build inbound envelopes
    /// that need to be signed by the peer; production callers should not
    /// reach for this directly.
    pub fn signing_key(&self) -> &SigningKey {
        &self.inner.signing
    }

    /// Standard-base64 encoding of the 32-byte verifying key. Used by the
    /// `meta/peer_pubkey` RPC and bootstrap-by-fetch flows.
    pub fn pubkey_b64(&self) -> String {
        BASE64.encode(self.inner.public.to_bytes())
    }

    /// Durable state root that owns this key, when it was loaded or created
    /// through [`Self::load_or_create`]. Runtime-host pairings use the same
    /// root so endpoint pins and the controller identity survive together.
    pub fn state_root(&self) -> Option<&Path> {
        self.state_root.as_deref().map(PathBuf::as_path)
    }
}

impl std::fmt::Debug for GatewayPeerKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayPeerKeys")
            .field("pubkey_b64", &self.pubkey_b64())
            .finish()
    }
}

/// Errors loading or persisting the gateway keypair.
#[derive(Debug)]
pub enum GatewayPeerKeyError {
    /// The on-disk key file was the wrong size — typically corruption or
    /// a non-32-byte format that needs manual cleanup.
    InvalidLength { path: PathBuf, actual: usize },
    /// The state directory recorded host pairings, so peers pin its
    /// identity, but the key file is missing. Restore the key; a fresh one
    /// would silently break every pin.
    EnrolledWithoutKey { path: PathBuf },
    /// Filesystem failure (read / write / mkdir).
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl std::fmt::Display for GatewayPeerKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLength { path, actual } => write!(
                f,
                "gateway peer key file {} is {actual} bytes (expected 32)",
                path.display()
            ),
            Self::EnrolledWithoutKey { path } => write!(
                f,
                "gateway peer key {} is missing but this installation has recorded host \
                 pairings; restore the established key (a new one would break every pin)",
                path.display()
            ),
            Self::Io { path, source } => {
                write!(
                    f,
                    "gateway peer key io error for {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for GatewayPeerKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidLength { .. } | Self::EnrolledWithoutKey { .. } => None,
        }
    }
}

/// Decode a `"<base64>"` (32-byte) Ed25519 pubkey string.
///
/// Used by both the contact-directory loader (where pubkey lives in TOML)
/// and clients consuming the `meta/peer_pubkey` RPC response. Accepts
/// the bare standard-base64 form; the `ed25519:` prefix used inside
/// meerkat-comms trust files is stripped if present so callers can paste
/// either shape.
pub fn decode_pubkey_b64(text: &str) -> Result<[u8; 32], PubkeyDecodeError> {
    let trimmed = text.trim();
    let body = trimmed.strip_prefix("ed25519:").unwrap_or(trimmed);
    let bytes = BASE64.decode(body).map_err(PubkeyDecodeError::Base64)?;
    if bytes.len() != 32 {
        return Err(PubkeyDecodeError::WrongLength(bytes.len()));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Errors decoding a base64-encoded Ed25519 pubkey.
#[derive(Debug)]
pub enum PubkeyDecodeError {
    Base64(base64::DecodeError),
    WrongLength(usize),
}

impl std::fmt::Display for PubkeyDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Base64(e) => write!(f, "invalid pubkey base64: {e}"),
            Self::WrongLength(n) => write!(f, "pubkey must be 32 bytes, got {n}"),
        }
    }
}

impl std::error::Error for PubkeyDecodeError {}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn ephemeral_minted_keys_are_unique() {
        let a = GatewayPeerKeys::ephemeral();
        let b = GatewayPeerKeys::ephemeral();
        assert_ne!(
            a.pubkey_bytes(),
            b.pubkey_bytes(),
            "fresh ephemeral keys must be distinct"
        );
    }

    #[test]
    fn load_or_create_persists_and_round_trips() {
        let dir = tempdir().expect("tempdir");
        let first = GatewayPeerKeys::load_or_create(dir.path()).expect("create");
        assert_eq!(
            first.state_root(),
            Some(dir.path()),
            "the key must retain the exact durable root that owns its pairing state"
        );
        let pubkey = first.pubkey_bytes();
        // Second call returns the persisted key, not a fresh one.
        let second = GatewayPeerKeys::load_or_create(dir.path()).expect("load");
        assert_eq!(second.pubkey_bytes(), pubkey, "key must persist");
        assert_eq!(second.state_root(), Some(dir.path()));
        assert!(dir.path().join(KEY_FILE_NAME).exists());
    }

    #[test]
    fn ephemeral_keys_have_no_durable_state_provenance() {
        assert_eq!(GatewayPeerKeys::ephemeral().state_root(), None);
    }

    #[test]
    fn load_or_create_rejects_short_file() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join(KEY_FILE_NAME), b"too short").expect("write");
        let err = GatewayPeerKeys::load_or_create(dir.path()).expect_err("short file must fail");
        assert!(matches!(err, GatewayPeerKeyError::InvalidLength { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn persisted_secret_key_is_created_0o600_not_world_readable() {
        // Regression: the secret seed must be created with mode 0o600 so it is
        // never momentarily world/group-readable (no create-then-chmod window).
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().expect("tempdir");
        GatewayPeerKeys::load_or_create(dir.path()).expect("create");
        let mode = std::fs::metadata(dir.path().join(KEY_FILE_NAME))
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "peer secret key file must be 0o600, got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_state_root_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempdir().expect("tempdir");
        let state_root = parent.path().join("gateway-state");
        GatewayPeerKeys::load_or_create(&state_root).expect("create state root");
        let mode = std::fs::metadata(&state_root)
            .expect("state-root metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700, "new gateway state root must be owner-only");
    }

    #[test]
    fn pubkey_b64_round_trips() {
        let keys = GatewayPeerKeys::ephemeral();
        let encoded = keys.pubkey_b64();
        let decoded = decode_pubkey_b64(&encoded).expect("decode");
        assert_eq!(decoded, keys.pubkey_bytes());
    }

    #[test]
    fn decode_strips_ed25519_prefix() {
        let keys = GatewayPeerKeys::ephemeral();
        let prefixed = format!("ed25519:{}", keys.pubkey_b64());
        let decoded = decode_pubkey_b64(&prefixed).expect("decode prefixed");
        assert_eq!(decoded, keys.pubkey_bytes());
    }

    #[test]
    fn decode_rejects_wrong_length() {
        let err = decode_pubkey_b64("aGVsbG8=").expect_err("must reject 5-byte payload");
        assert!(matches!(err, PubkeyDecodeError::WrongLength(_)));
    }
}
