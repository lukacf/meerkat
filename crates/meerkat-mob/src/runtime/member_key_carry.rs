//! The member key carry of a member rewind.
//!
//! A member's comms key pair is stored per session
//! (`<root>/<session id>/identity.key`), and a durable fork copies none. A
//! member rewind moves the member onto a fork child of its own session and
//! keeps the member's endpoint, peer id and edges, so the child must hold
//! the key pair the member's peers trust: the rewind copies the old
//! session's key pair into the child's key directory.
//!
//! The carry is safe only for the child the rewind commits, so it takes a
//! [`VerifiedRewindChild`], which only a verification of the child against
//! the rewind's record makes: an abandoned child never holds the member's
//! key. A carry already done is observed and changes nothing, so a rewind
//! that is driven again after a crash repeats it freely. A stored key is
//! never replaced.
//!
//! The key file names and their layout are `meerkat-comms`'s
//! (`Keypair::load`); a row pins that the file written here loads there.

// The rewind (the first caller) lands in a later slice.
#![cfg_attr(not(test), allow(dead_code))]

use std::path::PathBuf;

use meerkat_core::service::{
    MemberRewindChildConflict, MemberRewindChildObservation, MemberRewindForkEvidence,
};
use meerkat_core::types::SessionId;

use super::dormant_seat::{MemberSeatEndpointSource, MemberSeatEndpointsUnavailable};

/// A planned member rewind fork child that was verified against the
/// rewind's record. Only [`verify_member_rewind_child`] makes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedRewindChild {
    session_id: SessionId,
}

impl VerifiedRewindChild {
    /// The verified child session.
    pub(crate) fn session_id(&self) -> &SessionId {
        &self.session_id
    }
}

/// Prove that the observed session is the child `expected` describes,
/// bound to `member`: every evidence field, the prefix digest and the
/// member binding included
/// ([`MemberRewindForkEvidence::verify_child`]). A conflict names the first
/// field that differs, and no verified child exists.
///
/// The verified child is the session the observation was read from
/// ([`MemberRewindChildObservation::session_id`]): it can name no session
/// the verification did not see.
pub(crate) fn verify_member_rewind_child(
    expected: &MemberRewindForkEvidence,
    member: &meerkat_core::MobMemberBinding,
    observed: &MemberRewindChildObservation,
) -> Result<VerifiedRewindChild, MemberRewindChildConflict> {
    expected.verify_child(member, observed)?;
    Ok(VerifiedRewindChild {
        session_id: observed.session_id.clone(),
    })
}

/// What a member rewind carries of the member's comms key, decided before
/// the rewind starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberKeyCarryPlan {
    /// The old session's key pair under `identity_root` goes to the child.
    Carry { identity_root: PathBuf },
    /// The old session holds no session-scoped key pair: a member on
    /// network comms (its key belongs to the realm), or a member that was
    /// never built.
    NothingToCarry,
}

/// What a carry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberKeyCarryOutcome {
    /// The child now holds the old session's key pair.
    Carried,
    /// The child already held it: nothing was written.
    AlreadyCarried,
    /// The plan carries nothing.
    NothingToCarry,
}

/// Why a member's key cannot be carried. No variant holds key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemberKeyCarryRefusal {
    /// The member's peers trust a session-scoped key, and the mob has no
    /// comms identity root to read it from.
    NoIdentityRoot(MemberSeatEndpointsUnavailable),
    /// The root does not hold the old session's key pair.
    SourceKeyMissing,
    /// The old session's stored key is not the key the member's endpoint
    /// records.
    SourceKeyMismatch,
    /// The child's key directory already holds another key.
    ChildHoldsAnotherKey,
    /// The key store could not be read or written: the reason.
    KeyStore(String),
}

impl std::fmt::Display for MemberKeyCarryRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoIdentityRoot(_) => {
                f.write_str("no comms identity root to carry the member's key")
            }
            Self::SourceKeyMissing => {
                f.write_str("the member's session holds no comms key under the identity root")
            }
            Self::SourceKeyMismatch => {
                f.write_str("the member's stored comms key is not the key its endpoint records")
            }
            Self::ChildHoldsAnotherKey => {
                f.write_str("the rewind child's key directory already holds another key")
            }
            Self::KeyStore(reason) => write!(f, "the member's comms key store failed: {reason}"),
        }
    }
}

/// Decide what a rewind of the member on `old_session_id` carries.
///
/// `endpoint_source` is where the mob's seats take member endpoints from
/// (`member_seat_endpoint_source`), and `recorded_key` the public key of
/// the member's recorded in-process endpoint, if it has one. A recorded key
/// with no resolvable root refuses, and so does a root that does not hold
/// that key; without a recorded key and without a stored one there is
/// nothing to carry.
pub(crate) async fn plan_member_key_carry(
    endpoint_source: Result<MemberSeatEndpointSource, MemberSeatEndpointsUnavailable>,
    old_session_id: &SessionId,
    recorded_key: Option<[u8; 32]>,
) -> Result<MemberKeyCarryPlan, MemberKeyCarryRefusal> {
    let identity_root = match endpoint_source {
        Ok(MemberSeatEndpointSource::SessionScopedInproc { identity_root }) => identity_root,
        Ok(MemberSeatEndpointSource::Network) => return Ok(MemberKeyCarryPlan::NothingToCarry),
        Err(unavailable) => {
            return match recorded_key {
                Some(_) => Err(MemberKeyCarryRefusal::NoIdentityRoot(unavailable)),
                None => Ok(MemberKeyCarryPlan::NothingToCarry),
            };
        }
    };
    match key_store::stored_public_key(&identity_root.join(old_session_id.to_string())).await? {
        Some(stored) => {
            if recorded_key.is_some_and(|recorded| recorded != stored) {
                return Err(MemberKeyCarryRefusal::SourceKeyMismatch);
            }
            Ok(MemberKeyCarryPlan::Carry { identity_root })
        }
        None if recorded_key.is_some() => Err(MemberKeyCarryRefusal::SourceKeyMissing),
        None => Ok(MemberKeyCarryPlan::NothingToCarry),
    }
}

/// Carry the key pair of `old_session_id` to the verified `child`.
pub(crate) async fn carry_member_key(
    plan: &MemberKeyCarryPlan,
    old_session_id: &SessionId,
    child: &VerifiedRewindChild,
) -> Result<MemberKeyCarryOutcome, MemberKeyCarryRefusal> {
    carry_member_key_linking(plan, old_session_id, child, hard_link_key).await
}

/// [`carry_member_key`] with the step that links the written key into
/// place: a seam for rows of a file system that cannot hard-link.
pub(crate) async fn carry_member_key_linking(
    plan: &MemberKeyCarryPlan,
    old_session_id: &SessionId,
    child: &VerifiedRewindChild,
    link: fn(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
) -> Result<MemberKeyCarryOutcome, MemberKeyCarryRefusal> {
    let MemberKeyCarryPlan::Carry { identity_root } = plan else {
        return Ok(MemberKeyCarryOutcome::NothingToCarry);
    };
    key_store::carry(
        &identity_root.join(old_session_id.to_string()),
        &identity_root.join(child.session_id().to_string()),
        link,
    )
    .await
}

/// The hard link a carry stores the key with.
pub(crate) fn hard_link_key(
    temporary: &std::path::Path,
    key: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::hard_link(temporary, key)
}

#[cfg(not(target_arch = "wasm32"))]
mod key_store {
    use std::path::Path;

    use super::{MemberKeyCarryOutcome, MemberKeyCarryRefusal};

    /// The secret key file of a session's key directory, as
    /// `meerkat_comms::Keypair::load` reads it.
    const KEY_FILE: &str = "identity.key";
    /// Its public half, as `meerkat_comms::Keypair::save` writes it.
    const PUBLIC_KEY_FILE: &str = "identity.pub";

    fn failed(error: impl std::fmt::Display) -> MemberKeyCarryRefusal {
        MemberKeyCarryRefusal::KeyStore(error.to_string())
    }

    /// The key pair stored in `dir`, if it holds one.
    async fn stored_key(
        dir: &Path,
    ) -> Result<Option<meerkat_comms::Keypair>, MemberKeyCarryRefusal> {
        if !tokio::fs::try_exists(dir.join(KEY_FILE))
            .await
            .map_err(failed)?
        {
            return Ok(None);
        }
        meerkat_comms::Keypair::load(dir)
            .await
            .map(Some)
            .map_err(failed)
    }

    /// The public key of the key pair stored in `dir`, if it holds one.
    pub(super) async fn stored_public_key(
        dir: &Path,
    ) -> Result<Option<[u8; 32]>, MemberKeyCarryRefusal> {
        Ok(stored_key(dir)
            .await?
            .map(|keypair| *keypair.public_key().as_bytes()))
    }

    /// A copy of a secret key, cleared when dropped.
    struct SecretBytes([u8; 32]);

    impl Drop for SecretBytes {
        fn drop(&mut self) {
            self.0.fill(0);
            std::hint::black_box(&self.0);
        }
    }

    pub(super) async fn carry(
        old_dir: &Path,
        child_dir: &Path,
        link: fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<MemberKeyCarryOutcome, MemberKeyCarryRefusal> {
        let old = stored_key(old_dir)
            .await?
            .ok_or(MemberKeyCarryRefusal::SourceKeyMissing)?;
        let public_key = *old.public_key().as_bytes();
        if let Some(held) = stored_key(child_dir).await? {
            return observe(child_dir, &public_key, &held).await;
        }
        let secret = SecretBytes(old.secret_bytes());
        let dir = child_dir.to_path_buf();
        let stored = tokio::task::spawn_blocking(move || store_new_key(&dir, &secret.0, link))
            .await
            .map_err(failed)?
            .map_err(failed)?;
        match stored {
            KeyFileCreation::Created => {
                tokio::fs::write(child_dir.join(PUBLIC_KEY_FILE), public_key)
                    .await
                    .map_err(failed)?;
                Ok(MemberKeyCarryOutcome::Carried)
            }
            // A key appeared between the read above and the store: it
            // stays, and it is the carried key or it is another one.
            KeyFileCreation::AlreadyStored => {
                let held = stored_key(child_dir)
                    .await?
                    .ok_or(MemberKeyCarryRefusal::ChildHoldsAnotherKey)?;
                observe(child_dir, &public_key, &held).await
            }
        }
    }

    /// The child already holds a key: the carried one (a carry already
    /// done, which a carry interrupted before its public half completes
    /// here), or another one, which stays.
    async fn observe(
        child_dir: &Path,
        public_key: &[u8; 32],
        held: &meerkat_comms::Keypair,
    ) -> Result<MemberKeyCarryOutcome, MemberKeyCarryRefusal> {
        if held.public_key().as_bytes() != public_key {
            return Err(MemberKeyCarryRefusal::ChildHoldsAnotherKey);
        }
        let public_path = child_dir.join(PUBLIC_KEY_FILE);
        if !tokio::fs::try_exists(&public_path).await.map_err(failed)? {
            tokio::fs::write(&public_path, public_key)
                .await
                .map_err(failed)?;
        }
        Ok(MemberKeyCarryOutcome::AlreadyCarried)
    }

    /// How [`store_new_key`] left a directory's key file.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum KeyFileCreation {
        /// The key given is the stored key.
        Created,
        /// A key was stored already: it stays the stored key.
        AlreadyStored,
    }

    /// Store `secret` as `dir`'s key file unless a key is stored there
    /// already, as `meerkat-comms` stores a new key: written whole to a
    /// temporary file in `dir`, which is hard-linked to the key's name (a
    /// link fails when that name exists) and then removed, on every path.
    /// Where the file system cannot hard-link, the key file is created
    /// exclusively and written instead. Both files are created readable by
    /// their owner only on Unix, in the open that creates them.
    fn store_new_key(
        dir: &Path,
        secret: &[u8],
        link: fn(&Path, &Path) -> std::io::Result<()>,
    ) -> std::io::Result<KeyFileCreation> {
        std::fs::create_dir_all(dir)?;
        let key_path = dir.join(KEY_FILE);
        let temporary = TemporaryKeyFile(
            dir.join(format!(".{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4().simple())),
        );
        write_new_private_file(&temporary.0, secret)?;
        let linked = link(&temporary.0, &key_path);
        drop(temporary);
        match linked {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(KeyFileCreation::AlreadyStored);
            }
            // No hard link here: create the key file exclusively.
            Err(_) => match write_new_private_file(&key_path, secret) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Ok(KeyFileCreation::AlreadyStored);
                }
                Err(error) => return Err(error),
            },
        }
        Ok(KeyFileCreation::Created)
    }

    /// Create `path`, never an existing file, holding `bytes`, readable by
    /// its owner only on Unix from its creation (mode `0600` in the open,
    /// which the umask can only narrow). A file this created but could not
    /// write whole is removed.
    fn write_new_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write as _;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(path)?;
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            remove_key_file(path);
            return Err(error);
        }
        Ok(())
    }

    /// A temporary key file of [`store_new_key`], removed when dropped.
    struct TemporaryKeyFile(std::path::PathBuf);

    impl Drop for TemporaryKeyFile {
        fn drop(&mut self) {
            remove_key_file(&self.0);
        }
    }

    /// Remove a key file this module wrote; one that is gone already is
    /// fine.
    fn remove_key_file(path: &Path) {
        if let Err(error) = std::fs::remove_file(path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a temporary member key file could not be removed");
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod key_store {
    use std::path::Path;

    use super::{MemberKeyCarryOutcome, MemberKeyCarryRefusal};

    fn not_kept() -> MemberKeyCarryRefusal {
        MemberKeyCarryRefusal::KeyStore(
            "a browser runtime keeps no durable comms identity".to_string(),
        )
    }

    pub(super) async fn stored_public_key(
        _dir: &Path,
    ) -> Result<Option<[u8; 32]>, MemberKeyCarryRefusal> {
        Err(not_kept())
    }

    pub(super) async fn carry(
        _old_dir: &Path,
        _child_dir: &Path,
        _link: fn(&Path, &Path) -> std::io::Result<()>,
    ) -> Result<MemberKeyCarryOutcome, MemberKeyCarryRefusal> {
        Err(not_kept())
    }
}
