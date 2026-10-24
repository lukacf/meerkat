//! Cross-process session hosting (#1813).
//!
//! "Session S is served by runtime owner O" is a fact every process on a
//! realm must be able to see. A hosting claim is an exclusive OS lock (flock
//! on unix, `LockFileEx` on Windows) on one lock file per session, held by
//! the process that hosts the session. The kernel releases it when the
//! process dies: there is no expiry, no heartbeat and no timer.
//!
//! A [`HostingClaim`] can only be minted by [`grant_session_hosting`]; the
//! operations it gates (cold attach, runtime registration, actor insertion)
//! take it by move, so "hosted without a claim" cannot be represented. A store
//! without cross-process claims grants an unclaimed [`HostingClaim`], still
//! minted only here, so the check always ran. A store that selected
//! cross-process claims never does: a claim it cannot take for a reason other
//! than another holder is refused, typed ([`HostingClaimUnavailable`]), never
//! weakened to an unclaimed grant.
//!
//! Claims are owner-scoped. Each runtime owner (one runtime control plane)
//! mints one [`HostingOwner`], and every claim carries its owner. Within one
//! process the holders of a session's claim share one OS lock through a
//! process-local registry, but only within the claim's lineage: a grant from
//! the same owner shares it, a grant from another local owner is refused like
//! a grant from another process. The lock is released when the last
//! [`HostingClaim`] of the lineage is dropped.
//!
//! Where a session is served is read from that registry only
//! ([`local_session_serving`]). Nothing ever takes a session's OS lock to
//! "probe" it: flock has no test operation, and a probe that took the lock
//! would exclude a real host in another process for its duration.
//!
//! Lock files carry a holder record (pid and process instance id) for
//! diagnostics only; correctness rests on the OS lock alone. Lock files are
//! never unlinked while their realm is online (removing a file another
//! process holds and re-creating it would admit two holders).

use std::path::PathBuf;
use std::sync::Arc;

use crate::types::SessionId;

/// Where a store's hosting claims and delivery coordination files live.
/// Injected by the composition from the realm's path authority; a store never
/// derives these paths on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingPaths {
    /// One `<session_id>.lock` per session.
    pub hosting_lock_dir: PathBuf,
    /// The lock whose holder is the store's cold-delivery owner.
    pub cold_delivery_lock: PathBuf,
    /// The store's database file. Delivery owners watch it and its sidecars
    /// for commits by any process (a wake hint; see the delivery owner).
    /// `None` until the store binds the paths.
    pub database: Option<PathBuf>,
}

/// How sessions on a store are hosted across processes.
#[derive(Debug, Clone)]
pub enum HostingCapability {
    /// OS-locked claims beside the store: several processes may open it and
    /// each session is hosted by exactly one runtime owner among them.
    OsLock(Arc<HostingPaths>),
    /// No other process can open the store (an in-memory store): this
    /// process is the only possible host.
    ProcessLocal,
    /// The store offers no claims, or the OS lock cannot be trusted on its
    /// filesystem: single-process behavior, no multi-process claim.
    None,
}

impl HostingPaths {
    /// The passive identity of this hosting domain, keyed like its claims:
    /// by the lock directory the composition injected, in canonical form, so
    /// a relative spelling, a trailing separator or a symlink names the same
    /// domain as the directory the claims actually lock in.
    pub fn domain_ref(&self) -> HostingDomainRef {
        domain_ref_of_lock_dir(&self.hosting_lock_dir)
    }
}

/// The hosting domain whose claims lock in `lock_dir` (see
/// [`HostingPaths::domain_ref`]).
fn domain_ref_of_lock_dir(lock_dir: &std::path::Path) -> HostingDomainRef {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let lock_dir = canonical_lock_dir(lock_dir);
    let digest = Sha256::digest(lock_dir.as_os_str().as_encoded_bytes());
    let mut rendered = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a String is infallible; the formatter error is
        // discarded deliberately rather than unwrapped.
        let _ = write!(rendered, "{byte:02x}");
    }
    HostingDomainRef(rendered)
}

/// The canonical form of a hosting lock directory, resolved as the lock
/// acquisition's own path is: absolute, then component by component through
/// the filesystem while the directory exists, so a symlink is followed
/// before a later `..` and that `..` leaves the link's target, never the
/// link's lexical parent. The part that does not exist yet cannot contain a
/// symlink and is normalized lexically.
fn canonical_lock_dir(dir: &std::path::Path) -> PathBuf {
    use std::path::Component;
    let absolute = if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| dir.to_path_buf(), |cwd| cwd.join(dir))
    };
    // `resolved` is the existing prefix as the filesystem resolved it;
    // `missing` is the lexical remainder below the first component that
    // does not resolve.
    let mut resolved = PathBuf::new();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => resolved.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if missing.pop().is_none() {
                    resolved.pop();
                }
            }
            Component::Normal(name) if missing.is_empty() => {
                match std::fs::canonicalize(resolved.join(name)) {
                    Ok(physical) => resolved = physical,
                    Err(_) => missing.push(name.to_owned()),
                }
            }
            Component::Normal(name) => missing.push(name.to_owned()),
        }
    }
    resolved.extend(missing);
    resolved
}

/// Passive identity of one hosting domain: the exclusion domain of a store's
/// session hosting claims. Claims are keyed by their lock path, so two
/// domains may host sessions with equal ids; this reference tells them
/// apart.
///
/// It is a hash of the canonical lock directory, and identity data only. It
/// grants nothing and proves nothing: no held claim, no owner death, no
/// authenticated host, and no equality of domains across hosts. Recovery,
/// wake and handoff decide on the actual hosting claim, never on this
/// reference. A domain whose lock directory moved is a different domain.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct HostingDomainRef(String);

impl HostingDomainRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl HostingCapability {
    /// The passive identity of the domain whose claims this capability
    /// takes. Only a cross-process capability has one: a process-local or
    /// claim-less store has no hosting domain another process could share.
    pub fn domain_ref(&self) -> Option<HostingDomainRef> {
        self.paths().map(HostingPaths::domain_ref)
    }

    /// Whether this capability coordinates hosting across processes.
    pub fn is_cross_process(&self) -> bool {
        matches!(self, Self::OsLock(_))
    }

    /// The coordination paths, for a cross-process capability.
    pub fn paths(&self) -> Option<&HostingPaths> {
        match self {
            Self::OsLock(paths) => Some(paths),
            Self::ProcessLocal | Self::None => None,
        }
    }
}

/// The identity of one runtime owner's hosting claims. Minted once per
/// runtime owner; two owners never compare equal, even over one store.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostingOwner(u64);

impl HostingOwner {
    /// A fresh owner identity, distinct from every other in this process.
    pub fn mint() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

/// Another runtime owner holds the session's hosting claim: another process
/// on the realm, or another runtime owner in this process.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("session {session_id} is served by another runtime owner on this realm")]
pub struct ServedElsewhere {
    pub session_id: SessionId,
}

impl From<ServedElsewhere> for crate::service::SessionError {
    fn from(refused: ServedElsewhere) -> Self {
        Self::ServedElsewhere {
            id: refused.session_id,
        }
    }
}

/// The store selected cross-process hosting claims ([`HostingCapability::OsLock`]),
/// but this session's claim cannot be taken for a reason other than another
/// holder (the lock file cannot be created or locked: permissions, descriptor
/// exhaustion). Nothing is hosted or written for the session without its
/// claim; the refusal clears once the claim can be taken. `reason` is a local
/// diagnostic (it names paths) and never crosses a wire boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("hosting claim for session {session_id} is unavailable: {reason}")]
pub struct HostingClaimUnavailable {
    pub session_id: SessionId,
    pub reason: String,
}

impl From<HostingClaimUnavailable> for crate::service::SessionError {
    fn from(unavailable: HostingClaimUnavailable) -> Self {
        Self::HostingUnavailable {
            id: unavailable.session_id,
        }
    }
}

/// Why [`grant_session_hosting`] refused a claim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostingRefused {
    /// Another runtime owner holds the claim.
    #[error(transparent)]
    ServedElsewhere(#[from] ServedElsewhere),
    /// The store's cross-process claim cannot be taken at all.
    #[error(transparent)]
    Unavailable(#[from] HostingClaimUnavailable),
}

impl HostingRefused {
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::ServedElsewhere(refused) => &refused.session_id,
            Self::Unavailable(unavailable) => &unavailable.session_id,
        }
    }
}

impl From<HostingRefused> for crate::service::SessionError {
    fn from(refused: HostingRefused) -> Self {
        match refused {
            HostingRefused::ServedElsewhere(refused) => refused.into(),
            HostingRefused::Unavailable(unavailable) => unavailable.into(),
        }
    }
}

/// Authority for one runtime owner to host one session, or write its durable
/// state. Only [`grant_session_hosting`] mints it. A clone is a lineage clone
/// (same session, same owner); the session's OS lock (if any) is released
/// when the last claim of the lineage is dropped.
#[derive(Clone)]
pub struct HostingClaim {
    session_id: SessionId,
    owner: HostingOwner,
    acquisition: HostingAcquisition,
    #[cfg(not(target_arch = "wasm32"))]
    held: Option<native::SharedLock>,
}

/// How one grant obtained its session's hosting claim: the branch the grant
/// took. A clone reports the branch of the grant it was cloned from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostingAcquisition {
    /// A process-local or claim-less store's grant: it took no exclusion
    /// across processes.
    Unclaimed,
    /// The grant shares the live lineage of the same runtime owner.
    Joined,
    /// The grant took over a lineage of this process whose every claim was
    /// already dropped, with the session's OS lock still held by this
    /// process.
    TookOverDeadLineage,
    /// The grant took the session's OS lock while no process held it.
    AcquiredFresh,
}

/// Proof that one grant excluded every earlier holder of a session's
/// cross-process hosting claim in `domain`: it took the session's OS lock
/// while no process held it, or took over a lineage of this process whose
/// every claim was already dropped. Whatever runtime owner held the claim
/// before (in another process, or a dead lineage of this process) holds
/// nothing of the session now.
///
/// Minted only by that grant, and settled once per lineage, by the
/// registration that attaches the claim and publishes
/// ([`HostingClaim::reserve_owner_loss`]). A joined grant, and a
/// process-local or claim-less store's grant, never mint one. The proof is
/// historical exclusion data: it grants nothing and is not present custody.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostingOwnerLossProof {
    domain: HostingDomainRef,
    session_id: SessionId,
    acquisition: HostingAcquisition,
}

impl HostingOwnerLossProof {
    /// The hosting domain whose claim the grant took.
    pub fn domain(&self) -> &HostingDomainRef {
        &self.domain
    }

    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The exclusion the grant took: [`HostingAcquisition::AcquiredFresh`]
    /// or [`HostingAcquisition::TookOverDeadLineage`].
    pub fn acquisition(&self) -> HostingAcquisition {
        self.acquisition
    }
}

/// What an attachment of a claim finds of its lineage's owner-loss
/// settlement ([`HostingClaim::reserve_owner_loss`]).
#[derive(Debug)]
pub enum OwnerLossAttachment {
    /// Nothing is owed: the lineage's settlement completed, or the claim is
    /// a process-local or claim-less store's grant.
    Settled,
    /// This attachment holds the lineage's proof until it completes the
    /// settlement at its winning publication.
    Reserved(OwnerLossReservation),
    /// Another attachment of the lineage holds its proof: the settlement is
    /// in progress, not complete.
    InProgress,
}

/// One attachment's hold on its claim lineage's owner-loss proof. It belongs
/// to that lineage: completing it at the winning publication settles the
/// lineage once, and dropping it returns the proof to the same lineage, so a
/// failed settlement, an abandoned or lost publication, or a cancelled future
/// leaves the settlement owed. There is no other way to put a proof back.
#[must_use = "complete it at the winning publication; dropping it returns the proof"]
pub struct OwnerLossReservation {
    #[cfg(not(target_arch = "wasm32"))]
    lineage: native::SharedLock,
    proof: HostingOwnerLossProof,
    held: bool,
}

impl OwnerLossReservation {
    /// The proof this reservation holds: what the attachment settles. It
    /// names the scope (hosting domain and session) and is never authority
    /// by itself: only a borrow of the live reservation settles.
    pub fn proof(&self) -> &HostingOwnerLossProof {
        &self.proof
    }

    /// Whether `claim` belongs to the very claim lineage this reservation
    /// holds (the same live claim), not merely to the same session.
    pub(crate) fn holds_lineage_of(&self, claim: &HostingClaim) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            claim
                .held
                .as_ref()
                .is_some_and(|held| std::sync::Arc::ptr_eq(held, &self.lineage))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = claim;
            false
        }
    }

    /// The winning publication completed the lineage's settlement: no later
    /// attachment of the lineage owes it again. Returns the proof as the
    /// historical record of what was settled.
    pub fn complete(mut self) -> HostingOwnerLossProof {
        self.held = false;
        #[cfg(not(target_arch = "wasm32"))]
        self.lineage.complete_owner_loss();
        self.proof.clone()
    }
}

impl Drop for OwnerLossReservation {
    fn drop(&mut self) {
        if self.held {
            #[cfg(not(target_arch = "wasm32"))]
            self.lineage.return_owner_loss(self.proof.clone());
        }
    }
}

impl std::fmt::Debug for OwnerLossReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerLossReservation")
            .field("proof", &self.proof)
            .field("held", &self.held)
            .finish()
    }
}

impl HostingClaim {
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// The runtime owner whose lineage this claim belongs to.
    pub fn owner(&self) -> &HostingOwner {
        &self.owner
    }

    /// The branch the grant of this claim took (see [`HostingAcquisition`]).
    pub fn acquisition(&self) -> HostingAcquisition {
        self.acquisition
    }

    /// The hosting domain of this claim's lineage: the domain of the lock
    /// directory its OS lock is held in. Only a cross-process claim has one.
    pub fn domain_ref(&self) -> Option<HostingDomainRef> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.held.as_ref().map(|held| held.domain_ref().clone())
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    /// Reserve the owner-loss settlement of this claim's lineage for the
    /// registration that attaches this claim. A lineage that began with a
    /// cross-process fresh acquisition or a takeover owes one settlement of
    /// its proof. Any claim of the lineage reaches it, because the grant that
    /// began the lineage need not be the one the registration attaches.
    ///
    /// The registration settles while it holds the reservation, and completes
    /// the reservation only at its winning publication; dropping the
    /// reservation (a failed settlement, an abandoned or lost publication, a
    /// cancelled future) returns the proof to this same lineage. While one
    /// attachment holds it, every other attachment of the lineage finds the
    /// settlement in progress, never complete.
    pub fn reserve_owner_loss(&self) -> OwnerLossAttachment {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match self.held.as_ref() {
                Some(held) => native::ClaimInner::reserve_owner_loss(held),
                None => OwnerLossAttachment::Settled,
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            OwnerLossAttachment::Settled
        }
    }

    /// Whether this claim holds a cross-process OS lock (as opposed to a
    /// process-local or claim-less store's grant).
    pub fn is_cross_process(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.held.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }
}

impl std::fmt::Debug for HostingClaim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostingClaim")
            .field("session_id", &self.session_id)
            .field("owner", &self.owner)
            .field("acquisition", &self.acquisition)
            .field("cross_process", &self.is_cross_process())
            .finish()
    }
}

/// Take `owner`'s hosting claim for `session_id`, without waiting.
///
/// A claim of the same owner's lineage is shared. A claim held by another
/// runtime owner, in this process or another, refuses at once with
/// [`ServedElsewhere`]: waiting would be a timer.
///
/// On a store that selected cross-process claims, a claim that cannot be
/// taken for another reason (the lock file cannot be created or locked) is
/// refused with [`HostingClaimUnavailable`], in every hosting mode: an
/// unclaimed grant there would let a second owner host the session. Only a
/// process-local store, or one without cross-process claims, grants unclaimed.
///
/// The claim reports the branch the grant took ([`HostingClaim::acquisition`]);
/// a lineage that a cross-process grant began fresh or by takeover owes one
/// owner-loss settlement ([`HostingClaim::reserve_owner_loss`]).
pub fn grant_session_hosting(
    capability: &HostingCapability,
    owner: &HostingOwner,
    session_id: &SessionId,
) -> Result<HostingClaim, HostingRefused> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return Ok(HostingClaim {
                session_id: session_id.clone(),
                owner: owner.clone(),
                acquisition: HostingAcquisition::Unclaimed,
                held: None,
            });
        };
        match native::try_claim_session(&paths.hosting_lock_dir, owner, session_id) {
            native::ClaimAttempt::Acquired { lock, acquisition } => Ok(HostingClaim {
                session_id: session_id.clone(),
                owner: owner.clone(),
                acquisition,
                held: Some(lock),
            }),
            native::ClaimAttempt::HeldByAnotherOwner => Err(ServedElsewhere {
                session_id: session_id.clone(),
            }
            .into()),
            native::ClaimAttempt::Unavailable { reason } => {
                tracing::warn!(
                    session_id = %session_id,
                    %reason,
                    "session hosting claim unavailable; the session is not hosted here"
                );
                Err(HostingClaimUnavailable {
                    session_id: session_id.clone(),
                    reason,
                }
                .into())
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = capability;
        Ok(HostingClaim {
            session_id: session_id.clone(),
            owner: owner.clone(),
            acquisition: HostingAcquisition::Unclaimed,
        })
    }
}

/// One runtime owner's authority to grant its own claims: its owner identity
/// and its store's hosting capability.
#[derive(Debug, Clone)]
pub struct SessionHostingAuthority {
    capability: HostingCapability,
    owner: HostingOwner,
}

impl SessionHostingAuthority {
    pub fn new(capability: HostingCapability, owner: HostingOwner) -> Self {
        Self { capability, owner }
    }

    pub fn capability(&self) -> &HostingCapability {
        &self.capability
    }

    pub fn owner(&self) -> &HostingOwner {
        &self.owner
    }

    /// Take this owner's claim for `session_id` (see [`grant_session_hosting`]).
    pub fn grant(&self, session_id: &SessionId) -> Result<HostingClaim, HostingRefused> {
        grant_session_hosting(&self.capability, &self.owner, session_id)
    }

    /// Where `session_id` is served, as far as this process knows (see
    /// [`local_session_serving`]).
    pub fn serving(&self, session_id: &SessionId) -> SessionServing {
        local_session_serving(&self.capability, &self.owner, session_id)
    }
}

/// How a session about to be created gets its hosting claim. The claim then
/// moves into the session's task and lives exactly as long as the task, so it
/// is released only after the actor's last durable write.
#[derive(Debug, Clone)]
pub enum SessionHostingIntent {
    /// The caller already holds the claim (a session whose id it knows: a
    /// resume, or a pre-assigned id). Its session id must match the created
    /// session's.
    Granted(HostingClaim),
    /// Grant at actor insertion, once the generated session id exists. A
    /// brand-new id has no other holder, so the grant is refused only when
    /// the store's claim is unavailable.
    GrantAtInsert(SessionHostingAuthority),
}

impl Default for SessionHostingIntent {
    /// A standalone service (no runtime store) is the only possible host.
    fn default() -> Self {
        Self::GrantAtInsert(SessionHostingAuthority::new(
            HostingCapability::ProcessLocal,
            HostingOwner::mint(),
        ))
    }
}

impl SessionHostingIntent {
    /// Resolve the claim for the created session `session_id`.
    pub fn resolve(self, session_id: &SessionId) -> Result<HostingClaim, ResolveHostingError> {
        match self {
            Self::Granted(claim) if claim.session_id() == session_id => Ok(claim),
            Self::Granted(claim) => Err(ResolveHostingError::SessionMismatch {
                claimed: claim.session_id().clone(),
                created: session_id.clone(),
            }),
            Self::GrantAtInsert(authority) => {
                authority
                    .grant(session_id)
                    .map_err(|refused| match refused {
                        HostingRefused::ServedElsewhere(refused) => {
                            ResolveHostingError::ServedElsewhere(refused)
                        }
                        HostingRefused::Unavailable(unavailable) => {
                            ResolveHostingError::Unavailable(unavailable)
                        }
                    })
            }
        }
    }
}

/// Why a [`SessionHostingIntent`] could not resolve.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveHostingError {
    #[error(transparent)]
    ServedElsewhere(ServedElsewhere),
    #[error(transparent)]
    Unavailable(HostingClaimUnavailable),
    #[error("hosting claim for session {claimed} offered to created session {created}")]
    SessionMismatch {
        claimed: SessionId,
        created: SessionId,
    },
}

impl From<ResolveHostingError> for crate::service::SessionError {
    fn from(error: ResolveHostingError) -> Self {
        match error {
            ResolveHostingError::ServedElsewhere(refused) => refused.into(),
            ResolveHostingError::Unavailable(unavailable) => unavailable.into(),
            mismatch @ ResolveHostingError::SessionMismatch { .. } => Self::Agent(
                crate::error::AgentError::InternalError(mismatch.to_string()),
            ),
        }
    }
}

/// Where a session is served, as far as this process can tell without
/// touching the session's lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionServing {
    /// The asking owner's lineage holds the session's claim. On a store
    /// without cross-process claims every session is served here.
    HeldHere,
    /// Another runtime owner of this process holds the claim.
    HeldByAnotherLocalOwner,
    /// No runtime owner of this process holds the claim. Another process
    /// may hold it; only an actual grant can tell.
    NotHeldInThisProcess,
}

/// Where `session_id` is served for `owner`, read from this process's claim
/// registry only. It never opens or locks the session's lock file, so it can
/// never make another process's grant fail.
pub fn local_session_serving(
    capability: &HostingCapability,
    owner: &HostingOwner,
    session_id: &SessionId,
) -> SessionServing {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return SessionServing::HeldHere;
        };
        native::local_serving(&paths.hosting_lock_dir, owner, session_id)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (capability, owner, session_id);
        SessionServing::HeldHere
    }
}

/// This process's standing as a store's cold-delivery owner: the single
/// process that applies deliveries for sessions no process hosts.
#[derive(Debug)]
pub enum ColdDeliveryOwnership {
    /// This process holds the store's cold-delivery lock.
    #[cfg(not(target_arch = "wasm32"))]
    Owner(ExclusiveProcessLock),
    /// No other process can apply this store's deliveries (a process-local
    /// store, or a store without cross-process claims): this process applies
    /// cold deliveries.
    Sole,
    /// Another process holds the cold-delivery lock.
    NotOwner,
    /// The store selected cross-process claims, but its cold-delivery lock
    /// cannot be opened or locked. This process does not apply cold
    /// deliveries; it tries again at its next pass, like a non-owner.
    Unavailable,
}

impl ColdDeliveryOwnership {
    /// Whether this process applies cold deliveries.
    pub fn applies_cold_deliveries(&self) -> bool {
        !matches!(self, Self::NotOwner | Self::Unavailable)
    }
}

/// Try to become the store's cold-delivery owner, without waiting. Only a
/// store without cross-process claims makes this process the [`Sole`] cold
/// owner; a cross-process lock this process cannot take, for any reason,
/// leaves it ineligible.
///
/// [`Sole`]: ColdDeliveryOwnership::Sole
pub fn try_cold_delivery_ownership(capability: &HostingCapability) -> ColdDeliveryOwnership {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let HostingCapability::OsLock(paths) = capability else {
            return ColdDeliveryOwnership::Sole;
        };
        match native::try_exclusive_lock(&paths.cold_delivery_lock) {
            native::ExclusiveAttempt::Acquired(lock) => ColdDeliveryOwnership::Owner(lock),
            native::ExclusiveAttempt::Held => ColdDeliveryOwnership::NotOwner,
            native::ExclusiveAttempt::Unavailable { reason } => {
                tracing::warn!(
                    %reason,
                    "cold-delivery owner lock unavailable; this process does not apply cold \
                     deliveries"
                );
                ColdDeliveryOwnership::Unavailable
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = capability;
        ColdDeliveryOwnership::Sole
    }
}

#[cfg(not(target_arch = "wasm32"))]
tokio::task_local! {
    /// The hosting claims of the store-only write running on this task.
    static WRITE_HOSTING: Vec<HostingClaim>;
}

/// Run a store-only durable write under `claim` (#1813). Every blocking
/// store operation the write starts through [`spawn_blocking_holding_claim`]
/// holds the claim until that operation returns, even when this future is
/// cancelled first. Nested scopes accumulate their claims.
pub async fn with_write_hosting<F>(claim: HostingClaim, write: F) -> F::Output
where
    F: std::future::Future,
{
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut claims = WRITE_HOSTING.try_with(Clone::clone).unwrap_or_default();
        claims.push(claim);
        WRITE_HOSTING.scope(claims, write).await
    }
    #[cfg(target_arch = "wasm32")]
    {
        // A browser build's claims hold no OS lock: nothing to keep alive.
        let _ = claim;
        write.await
    }
}

/// `spawn_blocking` for a store's blocking operations: the closure holds the
/// hosting claims of the calling task's write scope (see
/// [`with_write_hosting`]) until it returns, so a claim is never released
/// while a write made under it is still running. Outside a scope it holds
/// nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_blocking_holding_claim<F, T>(blocking: F) -> tokio::task::JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let held = WRITE_HOSTING.try_with(Clone::clone).ok();
    tokio::task::spawn_blocking(move || {
        let _held = held;
        blocking()
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{ExclusiveProcessLock, session_hosting_lock_path};

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::HashMap;
    use std::fs::{File, OpenOptions};
    use std::io::Write as _;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock, Weak};

    use super::{
        HostingAcquisition, HostingDomainRef, HostingOwner, HostingOwnerLossProof,
        OwnerLossAttachment, OwnerLossReservation, SessionServing,
    };
    use crate::types::SessionId;

    /// The lock file of `session_id`'s hosting claim under `lock_dir`.
    pub fn session_hosting_lock_path(lock_dir: &Path, session_id: &SessionId) -> PathBuf {
        lock_dir.join(format!("{session_id}.lock"))
    }

    /// Identity of this process for holder records.
    fn process_instance_id() -> &'static str {
        static INSTANCE: OnceLock<String> = OnceLock::new();
        INSTANCE.get_or_init(|| uuid::Uuid::now_v7().to_string())
    }

    /// One claim this process holds: its owner, its lineage's shared token and
    /// the open lock file. The token is weak: a claim lives exactly as long as
    /// its strong holders, and the OS lock exactly as long as the entry.
    struct RegistryEntry {
        owner: HostingOwner,
        claim: Weak<ClaimInner>,
        /// The OS lock is held through this file until the entry drops.
        _file: File,
    }

    /// An entry drops only when the registry removes it, after the last claim
    /// of its lineage was released: a successor that took the entry over
    /// keeps it, file and lock included. Its lock is released here, before
    /// the file closes. The lock belongs to the open file description, and a
    /// process spawned meanwhile holds a copy of the descriptor until it
    /// executes its program, which would keep a claim released by the close
    /// alone. The entry opened that description itself, so the unlock
    /// releases no other holder's lock.
    impl Drop for RegistryEntry {
        fn drop(&mut self) {
            let _ = self._file.unlock();
        }
    }

    /// Session claims this process holds, keyed by lock path.
    fn registry() -> std::sync::MutexGuard<'static, HashMap<PathBuf, RegistryEntry>> {
        static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, RegistryEntry>>> = OnceLock::new();
        REGISTRY
            .get_or_init(|| Mutex::new(HashMap::new()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) struct ClaimInner {
        path: PathBuf,
        /// The hosting domain of the lock directory this lineage's OS lock is
        /// held in.
        domain: HostingDomainRef,
        /// The owner-loss settlement this lineage owes for the proof the
        /// grant that began it minted (a fresh acquisition or a takeover).
        owner_loss: Mutex<OwnerLossState>,
    }

    /// Where one lineage's owner-loss settlement stands.
    enum OwnerLossState {
        /// The settlement waits for an attachment to reserve it.
        Owed(HostingOwnerLossProof),
        /// An attachment holds the proof (see [`OwnerLossReservation`]).
        Reserved,
        /// A winning publication completed it.
        Completed,
    }

    impl ClaimInner {
        fn begin(path: PathBuf, proof: HostingOwnerLossProof) -> Arc<Self> {
            Arc::new(Self {
                path,
                domain: proof.domain.clone(),
                owner_loss: Mutex::new(OwnerLossState::Owed(proof)),
            })
        }

        pub(super) fn domain_ref(&self) -> &HostingDomainRef {
            &self.domain
        }

        fn owner_loss(&self) -> std::sync::MutexGuard<'_, OwnerLossState> {
            self.owner_loss
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        pub(super) fn reserve_owner_loss(lineage: &SharedLock) -> OwnerLossAttachment {
            let mut state = lineage.owner_loss();
            match std::mem::replace(&mut *state, OwnerLossState::Reserved) {
                OwnerLossState::Owed(proof) => {
                    OwnerLossAttachment::Reserved(OwnerLossReservation {
                        lineage: Arc::clone(lineage),
                        proof,
                        held: true,
                    })
                }
                OwnerLossState::Reserved => OwnerLossAttachment::InProgress,
                OwnerLossState::Completed => {
                    *state = OwnerLossState::Completed;
                    OwnerLossAttachment::Settled
                }
            }
        }

        pub(super) fn complete_owner_loss(&self) {
            *self.owner_loss() = OwnerLossState::Completed;
        }

        pub(super) fn return_owner_loss(&self, proof: HostingOwnerLossProof) {
            let mut state = self.owner_loss();
            if matches!(*state, OwnerLossState::Reserved) {
                *state = OwnerLossState::Owed(proof);
            }
        }
    }

    impl Drop for ClaimInner {
        fn drop(&mut self) {
            #[cfg(test)]
            tests::hold_release(&self.path);
            let mut registry = registry();
            // Remove (closing the file, so releasing the OS lock) only a dead
            // entry: a grant that found this lineage dead may already have
            // taken the entry over.
            if registry
                .get(&self.path)
                .is_some_and(|entry| entry.claim.strong_count() == 0)
            {
                registry.remove(&self.path);
            }
        }
    }

    pub(super) type SharedLock = Arc<ClaimInner>;

    pub(super) enum ClaimAttempt {
        Acquired {
            lock: SharedLock,
            acquisition: HostingAcquisition,
        },
        /// Another runtime owner holds the claim: in this process (registry)
        /// or in another (the OS lock).
        HeldByAnotherOwner,
        Unavailable {
            reason: String,
        },
    }

    fn open_lock_file(path: &Path) -> std::io::Result<File> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
    }

    fn write_holder_record(file: &mut File) {
        // Diagnostic only: a failed write changes nothing about who holds
        // the lock, so it is not an error.
        let _ = file.set_len(0);
        let _ = writeln!(
            file,
            "pid={} instance={}",
            std::process::id(),
            process_instance_id()
        );
    }

    /// The owner-loss proof a grant that begins a lineage mints. Computed
    /// under the registry lock, so the proof exists from the instant the
    /// lineage does and no joined grant can miss it.
    fn owner_loss_proof(
        lock_dir: &Path,
        session_id: &SessionId,
        acquisition: HostingAcquisition,
    ) -> HostingOwnerLossProof {
        HostingOwnerLossProof {
            domain: super::domain_ref_of_lock_dir(lock_dir),
            session_id: session_id.clone(),
            acquisition,
        }
    }

    pub(super) fn try_claim_session(
        lock_dir: &Path,
        owner: &HostingOwner,
        session_id: &SessionId,
    ) -> ClaimAttempt {
        let path = session_hosting_lock_path(lock_dir, session_id);
        let mut registry = registry();
        if let Some(entry) = registry.get_mut(&path) {
            if entry.claim.strong_count() > 0 && entry.owner != *owner {
                return ClaimAttempt::HeldByAnotherOwner;
            }
            // Upgraded under the registry lock and returned to the caller:
            // never dropped while the registry is held.
            if let Some(inner) = entry.claim.upgrade() {
                return ClaimAttempt::Acquired {
                    lock: inner,
                    acquisition: HostingAcquisition::Joined,
                };
            }
            // A dead lineage whose last holder has not yet removed its entry:
            // take the entry over with its open file, so the OS lock is never
            // released in between and the releasing drop leaves it alone.
            let acquisition = HostingAcquisition::TookOverDeadLineage;
            let inner =
                ClaimInner::begin(path, owner_loss_proof(lock_dir, session_id, acquisition));
            entry.owner = owner.clone();
            entry.claim = Arc::downgrade(&inner);
            return ClaimAttempt::Acquired {
                lock: inner,
                acquisition,
            };
        }
        let mut file = match open_lock_file(&path) {
            Ok(file) => file,
            Err(error) => {
                return ClaimAttempt::Unavailable {
                    reason: format!(
                        "cannot open session hosting lock {}: {error}",
                        path.display()
                    ),
                };
            }
        };
        match file.try_lock() {
            Ok(()) => {
                write_holder_record(&mut file);
                let acquisition = HostingAcquisition::AcquiredFresh;
                let inner = ClaimInner::begin(
                    path.clone(),
                    owner_loss_proof(lock_dir, session_id, acquisition),
                );
                registry.insert(
                    path,
                    RegistryEntry {
                        owner: owner.clone(),
                        claim: Arc::downgrade(&inner),
                        _file: file,
                    },
                );
                ClaimAttempt::Acquired {
                    lock: inner,
                    acquisition,
                }
            }
            Err(std::fs::TryLockError::WouldBlock) => ClaimAttempt::HeldByAnotherOwner,
            Err(std::fs::TryLockError::Error(error)) => ClaimAttempt::Unavailable {
                reason: format!(
                    "cannot lock session hosting lock {}: {error}",
                    path.display()
                ),
            },
        }
    }

    pub(super) fn local_serving(
        lock_dir: &Path,
        owner: &HostingOwner,
        session_id: &SessionId,
    ) -> SessionServing {
        let path = session_hosting_lock_path(lock_dir, session_id);
        match registry().get(&path) {
            Some(entry) if entry.claim.strong_count() > 0 => {
                if entry.owner == *owner {
                    SessionServing::HeldHere
                } else {
                    SessionServing::HeldByAnotherLocalOwner
                }
            }
            _ => SessionServing::NotHeldInThisProcess,
        }
    }

    /// An exclusive OS lock on one file with a single holder (no in-process
    /// sharing): the cold-delivery owner lock.
    pub struct ExclusiveProcessLock {
        path: PathBuf,
        _file: File,
    }

    impl ExclusiveProcessLock {
        pub fn lock_path(&self) -> &Path {
            &self.path
        }
    }

    /// Released before the file closes, as a registry entry's claim is.
    impl Drop for ExclusiveProcessLock {
        fn drop(&mut self) {
            let _ = self._file.unlock();
        }
    }

    impl std::fmt::Debug for ExclusiveProcessLock {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("ExclusiveProcessLock")
                .field("path", &self.path)
                .finish_non_exhaustive()
        }
    }

    pub(super) enum ExclusiveAttempt {
        Acquired(ExclusiveProcessLock),
        Held,
        Unavailable { reason: String },
    }

    pub(super) fn try_exclusive_lock(path: &Path) -> ExclusiveAttempt {
        let mut file = match open_lock_file(path) {
            Ok(file) => file,
            Err(error) => {
                return ExclusiveAttempt::Unavailable {
                    reason: format!("cannot open lock {}: {error}", path.display()),
                };
            }
        };
        match file.try_lock() {
            Ok(()) => {
                write_holder_record(&mut file);
                ExclusiveAttempt::Acquired(ExclusiveProcessLock {
                    path: path.to_path_buf(),
                    _file: file,
                })
            }
            Err(std::fs::TryLockError::WouldBlock) => ExclusiveAttempt::Held,
            Err(std::fs::TryLockError::Error(error)) => ExclusiveAttempt::Unavailable {
                reason: format!("cannot lock {}: {error}", path.display()),
            },
        }
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::panic)]
    mod tests {
        use super::*;
        #[cfg(unix)]
        use crate::process_isolation_tests::DescriptorCopy;
        use std::sync::mpsc;
        use std::time::Duration;

        /// One armed hold: the next release of a lineage of this lock path
        /// acknowledges and waits for the test, before it takes the registry.
        type ReleaseHold = (PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>);

        static RELEASE_HOLDS: Mutex<Vec<ReleaseHold>> = Mutex::new(Vec::new());

        /// Called by a lineage's release (its last claim dropped) before it
        /// takes the registry.
        pub(super) fn hold_release(path: &Path) {
            let hold = {
                let mut holds = RELEASE_HOLDS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                holds
                    .iter()
                    .position(|(held, _, _)| held == path)
                    .map(|index| holds.remove(index))
            };
            if let Some((_, entered, release)) = hold {
                let _ = entered.send(());
                let _ = release.recv();
            }
        }

        /// Hold the next release of a lineage of `path`.
        fn hold_next_release(path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
            let (entered_tx, entered_rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            RELEASE_HOLDS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((path.to_path_buf(), entered_tx, release_rx));
            (entered_rx, release_tx)
        }

        /// Whether another open of the lock file at `path` (another process's
        /// open file description) can take its lock now. It gives back what
        /// it took.
        fn lock_is_free(path: &Path) -> bool {
            let probe = open_lock_file(path).expect("lock file");
            let free = probe.try_lock().is_ok();
            if free {
                probe.unlock().expect("give the probe's lock back");
            }
            free
        }

        /// A claim this process released is free at once for any other
        /// claimant, while a process spawned meanwhile still holds a copy of
        /// the claim's descriptor (as every process does until it executes
        /// its program).
        #[cfg(unix)]
        #[test]
        fn a_released_claim_is_free_while_a_spawned_process_holds_its_descriptor() {
            let dir = tempfile::tempdir().expect("tempdir");
            let lock_dir = dir.path().join("hosting");
            let session = SessionId::new();
            let path = session_hosting_lock_path(&lock_dir, &session);
            let ClaimAttempt::Acquired { lock: claim, .. } =
                try_claim_session(&lock_dir, &HostingOwner::mint(), &session)
            else {
                panic!("the session's claim");
            };
            let copy = DescriptorCopy::spawn(&registry().get(&path).expect("its entry")._file);
            drop(claim);
            assert!(
                matches!(
                    try_claim_session(&lock_dir, &HostingOwner::mint(), &session),
                    ClaimAttempt::Acquired {
                        acquisition: HostingAcquisition::AcquiredFresh,
                        ..
                    }
                ),
                "a released claim is free while a spawned process holds a copy of its descriptor"
            );
            drop(copy);
        }

        /// A claim a successor took over from a dead lineage keeps the OS
        /// lock when its predecessor's release runs afterwards, while a
        /// spawned process holds a copy of the claim's descriptor: the
        /// predecessor no longer owns the file. Once the successor releases
        /// it, the claim is free though the copy is still open.
        #[cfg(unix)]
        #[test]
        fn a_claim_taken_over_by_a_successor_stays_held_after_its_predecessor_releases() {
            let dir = tempfile::tempdir().expect("tempdir");
            let lock_dir = dir.path().join("hosting");
            let session = SessionId::new();
            let path = session_hosting_lock_path(&lock_dir, &session);
            let ClaimAttempt::Acquired {
                lock: predecessor, ..
            } = try_claim_session(&lock_dir, &HostingOwner::mint(), &session)
            else {
                panic!("the predecessor's claim");
            };
            let copy = DescriptorCopy::spawn(&registry().get(&path).expect("its entry")._file);

            // The predecessor's last claim drops; its release holds before it
            // takes the registry, its lineage already dead.
            let (entered, release) = hold_next_release(&path);
            let releasing = std::thread::spawn(move || drop(predecessor));
            entered
                .recv_timeout(Duration::from_secs(60))
                .expect("the predecessor's release began");
            let ClaimAttempt::Acquired {
                lock: successor,
                acquisition,
            } = try_claim_session(&lock_dir, &HostingOwner::mint(), &session)
            else {
                panic!("the successor takes the dead lineage's entry over");
            };
            assert_eq!(acquisition, HostingAcquisition::TookOverDeadLineage);
            release.send(()).expect("let the predecessor's release go");
            releasing.join().expect("the predecessor's release");

            assert!(
                !lock_is_free(&path),
                "the successor holds the claim after its predecessor's release"
            );
            assert!(
                registry().contains_key(&path),
                "the successor's entry stays"
            );
            drop(successor);
            assert!(
                lock_is_free(&path),
                "a released claim is free while a spawned process holds a copy of its descriptor"
            );
            drop(copy);
        }

        /// The cold-delivery owner lock, released, is free at once while a
        /// spawned process holds a copy of its descriptor.
        #[cfg(unix)]
        #[test]
        fn a_released_exclusive_lock_is_free_while_a_spawned_process_holds_its_descriptor() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("cold-delivery.lock");
            let ExclusiveAttempt::Acquired(lock) = try_exclusive_lock(&path) else {
                panic!("the lock");
            };
            let copy = DescriptorCopy::spawn(&lock._file);
            drop(lock);
            assert!(
                matches!(try_exclusive_lock(&path), ExclusiveAttempt::Acquired(_)),
                "a released lock is free while a spawned process holds a copy of its descriptor"
            );
            drop(copy);
        }

        /// A lineage of this process whose every claim was dropped before
        /// its last holder removed the registry entry is taken over with its
        /// open file. The takeover begins a lineage carrying one owner-loss
        /// proof for the session in the lock directory's domain; a joined
        /// grant of the new lineage reaches the same single proof.
        #[test]
        fn a_takeover_of_a_dead_lineage_carries_one_owner_loss_proof() {
            let dir = tempfile::tempdir().expect("tempdir");
            let lock_dir = dir.path().join("hosting");
            let session = SessionId::new();
            let path = session_hosting_lock_path(&lock_dir, &session);
            let mut file = open_lock_file(&path).expect("lock file");
            file.try_lock()
                .expect("this process holds the session's lock");
            write_holder_record(&mut file);
            registry().insert(
                path,
                RegistryEntry {
                    owner: HostingOwner::mint(),
                    claim: Weak::new(),
                    _file: file,
                },
            );

            let owner = HostingOwner::mint();
            let ClaimAttempt::Acquired { lock, acquisition } =
                try_claim_session(&lock_dir, &owner, &session)
            else {
                panic!("the dead lineage is taken over");
            };
            assert_eq!(acquisition, HostingAcquisition::TookOverDeadLineage);
            let ClaimAttempt::Acquired {
                lock: joined,
                acquisition: joined_acquisition,
            } = try_claim_session(&lock_dir, &owner, &session)
            else {
                panic!("the new lineage is shared with its owner");
            };
            assert_eq!(joined_acquisition, HostingAcquisition::Joined);
            let OwnerLossAttachment::Reserved(reservation) =
                ClaimInner::reserve_owner_loss(&joined)
            else {
                panic!("a joined grant reaches the lineage's owed settlement");
            };
            let proof = reservation.proof().clone();
            assert_eq!(proof.acquisition(), HostingAcquisition::TookOverDeadLineage);
            assert_eq!(proof.session_id(), &session);
            assert_eq!(
                proof.domain(),
                &super::super::domain_ref_of_lock_dir(&lock_dir)
            );
            assert!(
                matches!(
                    ClaimInner::reserve_owner_loss(&lock),
                    OwnerLossAttachment::InProgress
                ),
                "while one attachment holds it, the settlement is in progress for the others"
            );
            drop(reservation);
            let OwnerLossAttachment::Reserved(again) = ClaimInner::reserve_owner_loss(&lock) else {
                panic!("a dropped reservation returns the proof to its lineage");
            };
            assert_eq!(again.complete(), proof);
            assert!(
                matches!(
                    ClaimInner::reserve_owner_loss(&joined),
                    OwnerLossAttachment::Settled
                ),
                "a completed settlement is never owed again"
            );
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used)]
mod domain_ref_tests {
    use super::*;

    fn paths(dir: PathBuf) -> HostingPaths {
        HostingPaths {
            cold_delivery_lock: dir.join("cold-delivery.lock"),
            hosting_lock_dir: dir,
            database: None,
        }
    }

    /// Every spelling of one lock directory names one hosting domain: a
    /// trailing separator, `.` and `..` components, a relative path, and a
    /// symlink to it or to an ancestor of a directory not created yet. A
    /// symlink followed by `..` names the directory the lock path opens.
    /// Another directory is another domain.
    #[test]
    fn every_spelling_of_one_lock_directory_names_one_domain() {
        let root = tempfile::tempdir().expect("tempdir");
        let real = root.path().join("hosting");
        std::fs::create_dir(&real).expect("lock dir");
        let reference = paths(real.clone()).domain_ref();

        let mut trailing = real.clone().into_os_string();
        trailing.push(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(paths(PathBuf::from(trailing)).domain_ref(), reference);
        assert_eq!(
            paths(real.join(".").join("later").join("..")).domain_ref(),
            reference
        );
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(
            paths(PathBuf::from("relative-hosting")).domain_ref(),
            paths(cwd.join("relative-hosting")).domain_ref()
        );
        #[cfg(unix)]
        {
            let link = root.path().join("link");
            std::os::unix::fs::symlink(&real, &link).expect("symlink");
            assert_eq!(paths(link).domain_ref(), reference);
            let root_link = root.path().join("root-link");
            std::os::unix::fs::symlink(root.path(), &root_link).expect("symlink");
            assert_eq!(
                paths(root_link.join("not-yet")).domain_ref(),
                paths(root.path().join("not-yet")).domain_ref()
            );

            // A symlink followed by `..` names the directory the lock path
            // actually opens: the parent of the link's target, never the
            // lexical parent of the link, whether or not the lock directory
            // exists yet.
            let target = root.path().join("other").join("child");
            std::fs::create_dir_all(&target).expect("link target");
            let base = root.path().join("base");
            std::fs::create_dir(&base).expect("link parent");
            let alias = base.join("child-link");
            std::os::unix::fs::symlink(&target, &alias).expect("symlink");
            let aliased = alias.join("..").join("hosting");
            let actual = root.path().join("other").join("hosting");
            let lexical = base.join("hosting");
            for created in [false, true] {
                if created {
                    std::fs::create_dir(&aliased).expect("created through the alias");
                    assert!(actual.is_dir(), "the alias opens the actual target");
                    assert!(!lexical.exists(), "and never the lexical one");
                }
                assert_eq!(
                    paths(aliased.clone()).domain_ref(),
                    paths(actual.clone()).domain_ref(),
                    "the actual target, created: {created}"
                );
                assert_ne!(
                    paths(aliased.clone()).domain_ref(),
                    paths(lexical.clone()).domain_ref(),
                    "not the lexical target, created: {created}"
                );
            }
        }
        assert_ne!(paths(root.path().join("other")).domain_ref(), reference);
    }
}
