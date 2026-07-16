//! Multi-identity host acceptor for the mob member host daemon (D1).
//!
//! One TCP listener serves signed envelopes for EVERY identity registered on
//! this host (the host's own identity in phase 2; materialized member
//! identities from phase 3), demultiplexing per envelope by `envelope.to`.
//! Acks are signed by the ADDRESSED identity's keypair — the sender's router
//! accepts an ack only when `ack.from` equals the envelope's original `to`.
//!
//! Peer auth on this path is enforced by construction: neither
//! [`HostAcceptorConfig`] nor the demux entry point carries a flag that
//! could disable signature verification (D1 — mandatory, not configurable).
//!
//! The optional pairing branch is host-scoped ceremony transport (§7.2 step
//! 2): it proves possession of the host pairing secret and returns the
//! daemon-supplied host binding descriptor verbatim. Unlike member pairing
//! it installs NO trust and persists NOTHING (DEC-P2-3) — supervisor trust
//! arrives machine-gated at `HostBindAccepted`.

use std::any::Any;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use tokio::io::BufReader;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, oneshot, watch};
use tokio::task::JoinSet;
use uuid::Uuid;

use crate::classify::declared_tcp_port;
use crate::identity::{Keypair, PubKey};
use crate::inbox::InboxSender;
use crate::io_task::handle_tcp_connection_demux;
use crate::runtime::comms_runtime::{
    PAIRING_CHALLENGE_KIND, PAIRING_COMPLETE_KIND, PAIRING_VERSION, PairingClientMessage,
    PairingIdentityKind, comms_pairing_password_proof, constant_time_str_eq, parse_peer_address,
    read_pairing_message, validate_pairing_secret, write_pairing_json,
};

fn validate_advertised_tcp_endpoint(
    endpoint: &meerkat_core::comms::PeerAddress,
) -> Result<(), &'static str> {
    if declared_tcp_port(endpoint).is_none() {
        return Err("expected tcp://host:nonzero-port");
    }
    let authority = endpoint.endpoint();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let closing_bracket = bracketed
            .find(']')
            .ok_or("expected tcp://host:nonzero-port")?;
        let host = &bracketed[..closing_bracket];
        host.parse::<Ipv6Addr>()
            .map_err(|_| "bracketed advertised TCP hosts must be IPv6 addresses")?;
        host
    } else {
        authority
            .rsplit_once(':')
            .map(|(host, _)| host)
            .ok_or("expected tcp://host:nonzero-port")?
    };
    let wildcard_ip = host.parse::<IpAddr>().is_ok_and(|address| match address {
        IpAddr::V4(address) => address.is_unspecified(),
        IpAddr::V6(address) => {
            address.is_unspecified()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| mapped.is_unspecified())
        }
    });
    if host == "*" || wildcard_ip {
        return Err("advertised TCP host must be dialable, not a wildcard address");
    }
    Ok(())
}

/// Registration material one comms runtime hands to a host-acceptor
/// composer: the runtime's identity pubkey, the keypair the acceptor signs
/// transport acks with AS that identity, and the inbox sender inbound
/// envelopes are delivered through.
///
/// Produced by the concrete runtime behind the opaque
/// `CoreCommsRuntime::host_acceptor_registration_payload()` seam; consumed
/// by composers that hold this crate concretely (mob host daemon, the
/// controlling-side reverse-lane acceptor).
pub struct HostAcceptorRegistrationMaterial {
    pub pubkey: PubKey,
    pub keypair: Arc<Keypair>,
    pub inbox_sender: InboxSender,
}

/// Host-local projection of the daemon's registered inbound identities.
///
/// Installed/removed ONLY under the installed owner (machine-effect-driven
/// calls from the mob host actor); never self-populated, never seeded from
/// config or disk — the trust-store no-seed rule applies to this registry
/// too.
///
/// Comms cannot name the generated mob-side machine types (dependency
/// direction), so the comms-side gate is owner-token identity (Arc-ptr
/// equality, the `install_generated_mob_trust_owner` mechanics) and the
/// typed `from_transition` witness wraps the calls mob-side — the same
/// two-layer split the trust seam uses.
pub struct HostAcceptorIdentityRegistry {
    state: RwLock<HostAcceptorIdentityRegistryState>,
}

struct HostAcceptorIdentityRegistryState {
    identities: HashMap<PubKey, RegisteredAcceptorIdentity>,
    owner: Option<Arc<dyn Any + Send + Sync>>,
    reservation: Option<ReservedAcceptorIdentity>,
}

struct ReservedAcceptorIdentity {
    token: Arc<()>,
    owner: Arc<dyn Any + Send + Sync>,
    pubkey: PubKey,
    keypair: Arc<Keypair>,
}

/// Reversible reservation of the once-only registry owner and one identity.
///
/// The reservation prevents competing owners/identities without making the
/// identity resolvable by the acceptor. A composing daemon can therefore
/// publish its descriptor between [`HostAcceptorIdentityRegistry::reserve_identity`]
/// and [`Self::commit`]. Dropping the reservation rolls the exact private
/// owner/identity claim back.
#[must_use = "a host acceptor identity reservation must be committed or rolled back"]
pub struct HostAcceptorIdentityReservation {
    registry: Arc<HostAcceptorIdentityRegistry>,
    owner: Arc<dyn Any + Send + Sync>,
    token: Arc<()>,
    installed_owner: bool,
    committed: bool,
}

impl HostAcceptorIdentityReservation {
    /// Atomically make the reserved identity resolvable. Every fallible
    /// invariant was checked while reserving, and the reservation excludes a
    /// competing insert, so this commit has no error path after descriptor
    /// publication.
    pub fn commit(mut self, inbox_sender: InboxSender) {
        let mut state = self.registry.state.write();
        let reservation_is_exact = state.reservation.as_ref().is_some_and(|reservation| {
            Arc::ptr_eq(&reservation.token, &self.token)
                && Arc::ptr_eq(&reservation.owner, &self.owner)
        });
        if !reservation_is_exact {
            // This can only mean in-process authority corruption: the write
            // lock excludes races and the reservation API exposes no token
            // mutation. Do not continue with a different identity.
            std::process::abort();
        }
        let Some(reservation) = state.reservation.take() else {
            std::process::abort();
        };
        if state.identities.contains_key(&reservation.pubkey) {
            // Reservation excludes every other registry mutation. Reaching
            // this branch is likewise an impossible authority violation.
            std::process::abort();
        }
        state.identities.insert(
            reservation.pubkey,
            RegisteredAcceptorIdentity {
                keypair: reservation.keypair,
                inbox_sender,
            },
        );
        self.committed = true;
    }
}

impl Drop for HostAcceptorIdentityReservation {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let mut state = self.registry.state.write();
        let owns_reservation = state
            .reservation
            .as_ref()
            .is_some_and(|reservation| Arc::ptr_eq(&reservation.token, &self.token));
        if owns_reservation {
            state.reservation = None;
        }
        if self.installed_owner
            && owns_reservation
            && state.identities.is_empty()
            && state
                .owner
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &self.owner))
        {
            state.owner = None;
        }
    }
}

struct RegisteredAcceptorIdentity {
    keypair: Arc<Keypair>,
    inbox_sender: InboxSender,
}

impl HostAcceptorIdentityRegistry {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(HostAcceptorIdentityRegistryState {
                identities: HashMap::new(),
                owner: None,
                reservation: None,
            }),
        }
    }

    /// Reserve the once-only owner plus one inbound identity without exposing
    /// the identity to the acceptor yet.
    ///
    /// This is the startup transaction seam for descriptor-publishing
    /// daemons: all registry conflicts are reported before external
    /// publication, descriptor failure rolls back by dropping the returned
    /// reservation, and `commit` is infallible.
    pub fn reserve_identity(
        self: &Arc<Self>,
        owner: Arc<dyn Any + Send + Sync>,
        pubkey: PubKey,
        keypair: Arc<Keypair>,
    ) -> Result<HostAcceptorIdentityReservation, HostAcceptorError> {
        if keypair.public_key() != pubkey {
            return Err(HostAcceptorError::IdentityKeypairMismatch {
                pubkey: pubkey.to_pubkey_string(),
            });
        }
        let mut state = self.state.write();
        if state.reservation.is_some() {
            return Err(HostAcceptorError::RegistryReservationInProgress);
        }
        let installed_owner = match state.owner.as_ref() {
            Some(existing) if Arc::ptr_eq(existing, &owner) => false,
            Some(_) => return Err(HostAcceptorError::OwnerAlreadyInstalled),
            None => {
                state.owner = Some(Arc::clone(&owner));
                true
            }
        };
        if state.identities.contains_key(&pubkey) {
            return Err(HostAcceptorError::IdentityAlreadyRegistered {
                pubkey: pubkey.to_pubkey_string(),
            });
        }
        let token = Arc::new(());
        state.reservation = Some(ReservedAcceptorIdentity {
            token: Arc::clone(&token),
            owner: Arc::clone(&owner),
            pubkey,
            keypair,
        });
        drop(state);
        Ok(HostAcceptorIdentityReservation {
            registry: Arc::clone(self),
            owner,
            token,
            installed_owner,
            committed: false,
        })
    }

    /// Once-only owner install: re-installing the SAME owner (Arc-ptr
    /// equality) is idempotent; rebinding to a different owner is refused.
    pub fn install_owner(
        &self,
        owner: Arc<dyn Any + Send + Sync>,
    ) -> Result<(), HostAcceptorError> {
        let mut state = self.state.write();
        if state.reservation.is_some() {
            return Err(HostAcceptorError::RegistryReservationInProgress);
        }
        if let Some(existing) = state.owner.as_ref() {
            if Arc::ptr_eq(existing, &owner) {
                return Ok(());
            }
            return Err(HostAcceptorError::OwnerAlreadyInstalled);
        }
        state.owner = Some(owner);
        Ok(())
    }

    /// Register an inbound identity under the installed owner.
    ///
    /// `keypair` must be the signing keypair for `pubkey` — a mismatch would
    /// silently break ack verification sender-side, so it is a typed error
    /// here.
    pub fn register_identity(
        &self,
        owner: &Arc<dyn Any + Send + Sync>,
        pubkey: PubKey,
        keypair: Arc<Keypair>,
        inbox_sender: InboxSender,
    ) -> Result<(), HostAcceptorError> {
        if keypair.public_key() != pubkey {
            return Err(HostAcceptorError::IdentityKeypairMismatch {
                pubkey: pubkey.to_pubkey_string(),
            });
        }
        let mut state = self.state.write();
        if state.reservation.is_some() {
            return Err(HostAcceptorError::RegistryReservationInProgress);
        }
        Self::validate_owner_state(&state, owner)?;
        if state.identities.contains_key(&pubkey) {
            return Err(HostAcceptorError::IdentityAlreadyRegistered {
                pubkey: pubkey.to_pubkey_string(),
            });
        }
        state.identities.insert(
            pubkey,
            RegisteredAcceptorIdentity {
                keypair,
                inbox_sender,
            },
        );
        Ok(())
    }

    /// Remove an identity under the installed owner. Returns whether the
    /// identity was present.
    pub fn remove_identity(
        &self,
        owner: &Arc<dyn Any + Send + Sync>,
        pubkey: &PubKey,
    ) -> Result<bool, HostAcceptorError> {
        let mut state = self.state.write();
        if state.reservation.is_some() {
            return Err(HostAcceptorError::RegistryReservationInProgress);
        }
        Self::validate_owner_state(&state, owner)?;
        Ok(state.identities.remove(pubkey).is_some())
    }

    /// Resolve an inbound `envelope.to` to the addressed identity's ack
    /// keypair and inbox. `None` covers both never-registered and
    /// registered-then-removed identities — the demux path maps it to the
    /// typed misaddressed reject.
    pub(crate) fn resolve(&self, to: &PubKey) -> Option<(Arc<Keypair>, InboxSender)> {
        let state = self.state.read();
        let entry = state.identities.get(to)?;
        Some((entry.keypair.clone(), entry.inbox_sender.clone()))
    }

    fn validate_owner_state(
        state: &HostAcceptorIdentityRegistryState,
        owner: &Arc<dyn Any + Send + Sync>,
    ) -> Result<(), HostAcceptorError> {
        match state.owner.as_ref() {
            Some(installed) if Arc::ptr_eq(installed, owner) => Ok(()),
            Some(_) => Err(HostAcceptorError::OwnerMismatch),
            None => Err(HostAcceptorError::OwnerNotInstalled),
        }
    }
}

impl Default for HostAcceptorIdentityRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Configuration for the host acceptor.
///
/// Note the deliberate ABSENCE of any peer-auth field: signature
/// verification on the demux path is hardwired on (D1). The acceptor never
/// consults `CoreCommsConfig.require_peer_auth`.
pub struct HostAcceptorConfig {
    pub listen_tcp: SocketAddr,
    /// REQUIRED for non-loopback binds (tightens the member listener's
    /// wildcard-only rule: any non-loopback bind must state what it
    /// advertises). An explicit value must be
    /// `tcp://<host>:<nonzero-port>`; loopback binds default to
    /// `tcp://<local_addr>`.
    pub advertise_address: Option<String>,
    pub registry: Arc<HostAcceptorIdentityRegistry>,
    pub pairing: Option<HostPairingConfig>,
    pub bounds: HostAcceptorBounds,
}

/// Host-scoped pairing ceremony configuration (§7.2 step 2).
///
/// Deliberately holds NO router, trust-store, or persistence handles: the
/// host pairing branch cannot install trust or write snapshots by
/// construction (DEC-P2-3).
pub struct HostPairingConfig {
    /// Pairing secret; minimum 32 chars, validated at spawn.
    pub password: String,
    /// Challenge/proof target identity — the HOST keypair (§7.2).
    pub host_keypair: Arc<Keypair>,
    /// Host participant name carried in the pairing challenge/complete
    /// messages.
    pub participant_name: String,
    /// Pre-serialized `WireHostBindingDescriptor` JSON supplied by the
    /// daemon (comms stays contracts-free); returned verbatim as the
    /// PAIRING_COMPLETE `binding` object. Carries the CURRENT one-time
    /// token — the daemon refreshes this slot on token re-mint (DEC-P2-4).
    pub descriptor_json: watch::Receiver<String>,
}

/// Pre-auth resource bounds for the host acceptor (§21.2), config-fed from
/// `[mob_host]` with safe defaults.
#[derive(Debug, Clone, Copy)]
pub struct HostAcceptorBounds {
    /// Concurrent-connection cap; connections over the cap are dropped at
    /// the accept loop before any read.
    pub max_connections: usize,
    /// Bounds the complete pre-auth exchange per connection, from the
    /// classify peek through either the single signed envelope or the whole
    /// pairing challenge/proof exchange. This is one wall-clock budget, not
    /// a fresh timeout for each handshake phase.
    pub read_deadline: Duration,
    /// Pairing-branch rate limit, token bucket keyed by source IP.
    pub pairing_rate: PairingRateLimit,
}

pub const DEFAULT_HOST_ACCEPTOR_MAX_CONNECTIONS: usize = 64;
pub const DEFAULT_HOST_ACCEPTOR_READ_DEADLINE: Duration = Duration::from_secs(10);

impl Default for HostAcceptorBounds {
    fn default() -> Self {
        Self {
            max_connections: DEFAULT_HOST_ACCEPTOR_MAX_CONNECTIONS,
            read_deadline: DEFAULT_HOST_ACCEPTOR_READ_DEADLINE,
            pairing_rate: PairingRateLimit::default(),
        }
    }
}

/// Token-bucket pairing rate limit per source IP: bursts up to
/// `max_attempts`, refilling at `max_attempts` per `window`.
#[derive(Debug, Clone, Copy)]
pub struct PairingRateLimit {
    pub max_attempts: u32,
    pub window: Duration,
}

impl Default for PairingRateLimit {
    fn default() -> Self {
        Self {
            max_attempts: 10,
            window: Duration::from_secs(60),
        }
    }
}

/// Bound on tracked source-IP buckets. An entry idle for a full window has
/// necessarily refilled to capacity, so pruning it is lossless.
const PAIRING_RATE_MAX_TRACKED_SOURCES: usize = 1024;
const HOST_ACCEPTOR_ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

struct PairingRateLimiter {
    limit: PairingRateLimit,
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl PairingRateLimiter {
    fn new(limit: PairingRateLimit) -> Self {
        Self {
            limit,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    fn try_admit(&self, source: IpAddr) -> bool {
        if self.limit.max_attempts == 0 {
            return false;
        }
        let capacity = f64::from(self.limit.max_attempts);
        let now = Instant::now();
        let mut buckets = self.buckets.lock();
        if buckets.len() >= PAIRING_RATE_MAX_TRACKED_SOURCES && !buckets.contains_key(&source) {
            buckets.retain(|_, bucket| now.duration_since(bucket.last_refill) < self.limit.window);
            if buckets.len() >= PAIRING_RATE_MAX_TRACKED_SOURCES {
                // Every tracked source is currently active: refuse rather
                // than grow without bound (pre-auth fail-closed posture).
                return false;
            }
        }
        let bucket = buckets.entry(source).or_insert(TokenBucket {
            tokens: capacity,
            last_refill: now,
        });
        let window_secs = self.limit.window.as_secs_f64();
        if window_secs > 0.0 {
            let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
            bucket.tokens = (bucket.tokens + elapsed * capacity / window_secs).min(capacity);
        }
        bucket.last_refill = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Typed host acceptor faults.
#[derive(Debug, thiserror::Error)]
pub enum HostAcceptorError {
    #[error("host acceptor registry is already bound to a different owner")]
    OwnerAlreadyInstalled,
    #[error("host acceptor registry has no installed owner")]
    OwnerNotInstalled,
    #[error("host acceptor registry mutation requires the installed owner")]
    OwnerMismatch,
    #[error("host acceptor registry has an identity reservation in progress")]
    RegistryReservationInProgress,
    #[error("identity {pubkey} is already registered on the host acceptor")]
    IdentityAlreadyRegistered { pubkey: String },
    #[error("registered identity pubkey {pubkey} does not match the supplied keypair")]
    IdentityKeypairMismatch { pubkey: String },
    #[error("invalid host pairing secret: {reason}")]
    InvalidPairingSecret { reason: String },
    #[error(
        "host pairing may listen only on a loopback address; use the 0600 descriptor out-of-band or tunnel a loopback listener (got {addr})"
    )]
    PairingRequiresLoopback { addr: SocketAddr },
    #[error("invalid host acceptor advertise address '{address}': {reason}")]
    InvalidAdvertiseAddress { address: String, reason: String },
    #[error(
        "host acceptor bound to non-loopback address {addr} requires an explicit advertise address"
    )]
    AdvertiseAddressRequired { addr: SocketAddr },
    #[error("host acceptor bind failed: {0}")]
    Bind(#[from] std::io::Error),
}

/// Running host acceptor: the bound listener plus its accept-loop task.
pub struct HostAcceptorHandle {
    local_addr: SocketAddr,
    advertised_address: String,
    task: Option<tokio::task::JoinHandle<()>>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    overflow_dropped: Arc<AtomicU64>,
    pairing_refused: Arc<AtomicU64>,
}

impl HostAcceptorHandle {
    /// Real bound address (ephemeral port resolved).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The address peers should dial: the configured advertise address, else
    /// `tcp://<local_addr>` for loopback binds.
    pub fn advertised_address(&self) -> &str {
        &self.advertised_address
    }

    /// Connections dropped at the accept loop because the concurrent
    /// connection cap was reached.
    pub fn overflow_dropped(&self) -> u64 {
        self.overflow_dropped.load(Ordering::Relaxed)
    }

    /// Pairing attempts refused by the per-source rate limit.
    pub fn pairing_refused(&self) -> u64 {
        self.pairing_refused.load(Ordering::Relaxed)
    }

    /// Stop accepting new connections and wait for every accepted
    /// connection task to finish under its single pre-auth deadline. No
    /// connection task outlives successful shutdown.
    pub async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for HostAcceptorHandle {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(task) = self.task.as_ref() {
            // Drop cannot await the bounded connection drain. Aborting the
            // owner task drops its JoinSet, which aborts every owned child;
            // explicit `shutdown` remains the orderly join path.
            task.abort();
        }
    }
}

/// Bind the host acceptor and start its accept loop.
///
/// The loop mirrors the member TCP listener: bind, resolve the real
/// ephemeral `local_addr`, then one task per accepted connection — with the
/// §21.2 pre-auth bounds applied before any envelope is read.
pub async fn spawn_host_acceptor(
    config: HostAcceptorConfig,
) -> Result<HostAcceptorHandle, HostAcceptorError> {
    let HostAcceptorConfig {
        listen_tcp,
        advertise_address,
        registry,
        pairing,
        bounds,
    } = config;
    // Pairing returns the one-time host binding descriptor. The signed comms
    // transport is deliberately plaintext, so exposing this branch on a
    // remotely reachable listener would disclose the bootstrap token to a
    // passive on-path observer. Remote mob traffic remains supported on this
    // acceptor; only descriptor transfer is loopback/tunnel scoped.
    if pairing.is_some() && !listen_tcp.ip().is_loopback() {
        return Err(HostAcceptorError::PairingRequiresLoopback { addr: listen_tcp });
    }
    if let Some(pairing) = pairing.as_ref() {
        validate_pairing_secret(&pairing.password).map_err(|err| {
            HostAcceptorError::InvalidPairingSecret {
                reason: err.to_string(),
            }
        })?;
    }
    // Validate the explicit route before binding. This acceptor serves TCP
    // only, so publishing any other transport (or an unusable port) would
    // create a healthy-looking daemon whose signed binding descriptor cannot
    // route back to it.
    let advertise_address = advertise_address
        .map(|address| {
            let endpoint = parse_peer_address(&address).map_err(|reason| {
                HostAcceptorError::InvalidAdvertiseAddress {
                    address: address.clone(),
                    reason,
                }
            })?;
            if let Err(reason) = validate_advertised_tcp_endpoint(&endpoint) {
                return Err(HostAcceptorError::InvalidAdvertiseAddress {
                    address,
                    reason: reason.to_string(),
                });
            }
            Ok(address)
        })
        .transpose()?;
    let listener = TcpListener::bind(listen_tcp).await?;
    let local_addr = listener.local_addr()?;
    let advertised_address = match advertise_address {
        Some(address) => address,
        None => {
            if !local_addr.ip().is_loopback() {
                return Err(HostAcceptorError::AdvertiseAddressRequired { addr: local_addr });
            }
            format!("tcp://{local_addr}")
        }
    };

    let semaphore = Arc::new(Semaphore::new(bounds.max_connections));
    let overflow_dropped = Arc::new(AtomicU64::new(0));
    let pairing_refused = Arc::new(AtomicU64::new(0));
    let rate_limiter = Arc::new(PairingRateLimiter::new(bounds.pairing_rate));
    let pairing = Arc::new(pairing);
    let read_deadline = bounds.read_deadline;

    let loop_advertised = advertised_address.clone();
    let loop_overflow = overflow_dropped.clone();
    let loop_pairing_refused = pairing_refused.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            let accepted = tokio::select! {
                biased;
                _ = &mut shutdown_rx => break,
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = joined {
                        tracing::warn!(%error, "mob host acceptor connection task failed");
                    }
                    continue;
                }
                accepted = listener.accept() => accepted,
            };
            let (stream, source) = match accepted {
                Ok(accepted) => accepted,
                Err(error) => {
                    // Tokio listeners may surface transient accept errors.
                    // Exiting here would leave the daemon and its published
                    // binding descriptor alive while ingress had silently
                    // stopped. Retry under a small backoff so a persistent
                    // descriptor never claims a dead listener after one
                    // recoverable kernel error.
                    tracing::error!(
                        %error,
                        retry_delay_ms = HOST_ACCEPTOR_ACCEPT_RETRY_DELAY.as_millis(),
                        "mob host acceptor failed to accept a connection; retrying"
                    );
                    tokio::select! {
                        biased;
                        _ = &mut shutdown_rx => break,
                        () = tokio::time::sleep(HOST_ACCEPTOR_ACCEPT_RETRY_DELAY) => {}
                    }
                    continue;
                }
            };
            // Connection cap is enforced here, before any read, so an
            // over-cap connection costs nothing beyond the accept.
            let Ok(permit) = semaphore.clone().try_acquire_owned() else {
                loop_overflow.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    %source,
                    "host acceptor connection cap reached; dropping connection"
                );
                continue;
            };
            let registry = registry.clone();
            let pairing = pairing.clone();
            let rate_limiter = rate_limiter.clone();
            let advertised = loop_advertised.clone();
            let pairing_refused = loop_pairing_refused.clone();
            connections.spawn(async move {
                let _permit = permit;
                if let Err(err) = handle_host_connection(
                    stream,
                    source,
                    &registry,
                    pairing.as_ref().as_ref(),
                    &rate_limiter,
                    &pairing_refused,
                    read_deadline,
                    &advertised,
                )
                .await
                {
                    tracing::debug!(
                        %source,
                        error = %err,
                        "host acceptor connection ended with error"
                    );
                }
            });
        }

        // Stop kernel admission immediately. Keeping the listener alive
        // while draining children would leave its backlog connectable even
        // though no task polls `accept` anymore.
        drop(listener);
        // The listener is no longer polled. Retain ownership of accepted
        // tasks and wait for their already-running, deadline-bounded
        // exchanges so shutdown cannot return with a live pairing or demux
        // task still holding transport/state references.
        while let Some(joined) = connections.join_next().await {
            if let Err(error) = joined {
                tracing::warn!(%error, "mob host acceptor connection task failed during shutdown");
            }
        }
    });

    Ok(HostAcceptorHandle {
        local_addr,
        advertised_address,
        task: Some(task),
        shutdown_tx: Some(shutdown_tx),
        overflow_dropped,
        pairing_refused,
    })
}

#[allow(clippy::too_many_arguments)]
async fn handle_host_connection(
    stream: TcpStream,
    source: SocketAddr,
    registry: &HostAcceptorIdentityRegistry,
    pairing: Option<&HostPairingConfig>,
    rate_limiter: &PairingRateLimiter,
    pairing_refused: &AtomicU64,
    read_deadline: Duration,
    advertised_address: &str,
) -> Result<(), std::io::Error> {
    match tokio::time::timeout(
        read_deadline,
        handle_host_connection_within_deadline(
            stream,
            source,
            registry,
            pairing,
            rate_limiter,
            pairing_refused,
            advertised_address,
        ),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "host acceptor read deadline elapsed",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_host_connection_within_deadline(
    stream: TcpStream,
    source: SocketAddr,
    registry: &HostAcceptorIdentityRegistry,
    pairing: Option<&HostPairingConfig>,
    rate_limiter: &PairingRateLimiter,
    pairing_refused: &AtomicU64,
    advertised_address: &str,
) -> Result<(), std::io::Error> {
    // First-byte `{` sniff, exactly as the member listener classifies:
    // signed CBOR envelope frames never start with `{`.
    let mut first = [0_u8; 1];
    let is_pairing = pairing.is_some() && stream.peek(&mut first).await? == 1 && first[0] == b'{';
    if is_pairing && let Some(pairing) = pairing {
        // Pre-auth pairing rate limit: refused before the challenge is
        // issued, so an abusive source never advances the handshake.
        if !rate_limiter.try_admit(source.ip()) {
            pairing_refused.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%source, "host pairing rate limit exceeded; refusing attempt");
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "host pairing rate limit exceeded",
            ));
        }
        return handle_host_pairing_connection(stream, pairing, advertised_address).await;
    }

    // Peer auth is hardwired on (no parameter exists). The caller's single
    // outer timeout also covers classification and the full pairing branch.
    handle_tcp_connection_demux(stream, source, registry)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))
}

/// Host-scoped pairing handshake (§7.2 step 2), modeled on the member
/// pairing recipe with two deliberate differences (DEC-P2-3): the
/// challenge/proof target identity is the HOST keypair, and completing the
/// handshake writes NOTHING — no `add_trusted_peer_for_source`, no trust
/// snapshot persistence. It hands out the daemon-supplied binding
/// descriptor and stops.
async fn handle_host_pairing_connection(
    stream: TcpStream,
    pairing: &HostPairingConfig,
    advertised_address: &str,
) -> Result<(), std::io::Error> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let PairingClientMessage::Hello { version } =
        read_pairing_message(&mut reader, &mut line).await?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected meerkat pairing hello",
        ));
    };
    if version != PAIRING_VERSION {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "unsupported meerkat pairing version",
        ));
    }

    let challenge = Uuid::new_v4().to_string();
    write_pairing_json(
        reader.get_mut(),
        serde_json::json!({
            "kind": PAIRING_CHALLENGE_KIND,
            "version": PAIRING_VERSION,
            "challenge": challenge,
            "target": {
                "name": pairing.participant_name,
                "address": advertised_address,
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": pairing.host_keypair.public_key().to_pubkey_string(),
                }
            }
        }),
    )
    .await?;

    let PairingClientMessage::Proof {
        caller,
        password_proof,
    } = read_pairing_message(&mut reader, &mut line).await?
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "expected meerkat pairing proof",
        ));
    };
    let PairingIdentityKind::Ed25519PublicKey = caller.identity.kind;
    let expected = comms_pairing_password_proof(
        &pairing.password,
        &challenge,
        &caller.identity.public_key.raw,
        &caller.address.0,
    );
    if !constant_time_str_eq(&password_proof, &expected) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "invalid pairing password proof",
        ));
    }

    // The descriptor is an opaque pass-through: the daemon owns the typed
    // `WireHostBindingDescriptor`; comms embeds the current serialized form
    // verbatim (the watch slot carries the CURRENT one-time token).
    let descriptor_json = pairing.descriptor_json.borrow().clone();
    let descriptor: serde_json::Value = serde_json::from_str(&descriptor_json).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("host binding descriptor is not valid JSON: {err}"),
        )
    })?;
    write_pairing_json(
        reader.get_mut(),
        serde_json::json!({
            "kind": PAIRING_COMPLETE_KIND,
            "version": PAIRING_VERSION,
            "binding": descriptor,
            "comms_name": pairing.participant_name,
        }),
    )
    .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::identity::Signature;
    use crate::inbox::Inbox;
    use crate::router::{CommsConfig, Router, SendError};
    use crate::transport::codec::TransportCodec;
    use crate::trust::{TrustEntry, TrustStore};
    use crate::types::{Envelope, InboxItem, MessageKind};
    use futures::StreamExt;
    use meerkat_core::comms::{
        GeneratedCommsTrustAuthoritySourceKind, PeerAddress, PeerDeliveryOutcome, PeerId, PeerName,
    };
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    use tokio_util::codec::FramedRead;

    struct MemberFixture {
        keypair: Keypair,
        trusted: Arc<parking_lot::RwLock<TrustStore>>,
        inbox: Inbox,
        inbox_sender: InboxSender,
    }

    fn member_trusting(sender_pubkey: &PubKey, require_peer_auth: bool) -> MemberFixture {
        let keypair = Keypair::generate();
        let mut store = TrustStore::new();
        store
            .insert(TrustEntry {
                peer_id: sender_pubkey.to_peer_id(),
                name: PeerName::new("sender").expect("valid peer name"),
                pubkey: *sender_pubkey,
                address: PeerAddress::parse("tcp://127.0.0.1:1").expect("valid peer address"),
                meta: crate::PeerMeta::default(),
            })
            .expect("sender trust insert");
        let trusted = Arc::new(parking_lot::RwLock::new(store));
        let (inbox, inbox_sender) = Inbox::new_classified(
            test_support::classification_context_shared(trusted.clone(), require_peer_auth),
        );
        MemberFixture {
            keypair,
            trusted,
            inbox,
            inbox_sender,
        }
    }

    fn registry_with_owner() -> (
        Arc<HostAcceptorIdentityRegistry>,
        Arc<dyn Any + Send + Sync>,
    ) {
        let registry = Arc::new(HostAcceptorIdentityRegistry::new());
        let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry
            .install_owner(owner.clone())
            .expect("owner install");
        (registry, owner)
    }

    fn register_member(
        registry: &HostAcceptorIdentityRegistry,
        owner: &Arc<dyn Any + Send + Sync>,
        member: &MemberFixture,
    ) {
        registry
            .register_identity(
                owner,
                member.keypair.public_key(),
                Arc::new(member.keypair.clone()),
                member.inbox_sender.clone(),
            )
            .expect("identity registration");
    }

    async fn spawn_loopback_acceptor(
        registry: Arc<HostAcceptorIdentityRegistry>,
        pairing: Option<HostPairingConfig>,
        bounds: HostAcceptorBounds,
    ) -> HostAcceptorHandle {
        spawn_host_acceptor(HostAcceptorConfig {
            listen_tcp: "127.0.0.1:0".parse().expect("loopback socket addr"),
            advertise_address: None,
            registry,
            pairing,
            bounds,
        })
        .await
        .expect("host acceptor spawn")
    }

    /// Sender-side router whose ack validation (`ack.from == sent_to`) is
    /// the per-member ack-signing assertion.
    fn sender_router(sender_keypair: &Keypair) -> (Router, Inbox) {
        let (inbox, inbox_sender) = Inbox::new();
        let router = Router::new(
            sender_keypair.clone(),
            CommsConfig::default(),
            inbox_sender,
            true,
        );
        (router, inbox)
    }

    fn trust_recipient(router: &Router, name: &str, pubkey: &PubKey, address: &str) -> PeerId {
        let peer_id = pubkey.to_peer_id();
        router
            .add_trusted_peer_for_source(
                TrustEntry {
                    peer_id,
                    name: PeerName::new(name).expect("valid peer name"),
                    pubkey: *pubkey,
                    address: PeerAddress::parse(address).expect("valid peer address"),
                    meta: crate::PeerMeta::default(),
                },
                GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                false,
            )
            .expect("recipient trust install");
        peer_id
    }

    fn test_message() -> MessageKind {
        MessageKind::Message {
            content_taint: None,
            blocks: None,
            body: "hello".to_string(),
            handling_mode: None,
            objective_id: None,
        }
    }

    fn envelope_frame_bytes(envelope: &Envelope) -> Vec<u8> {
        let mut payload = Vec::new();
        ciborium::into_writer(envelope, &mut payload).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes
    }

    // ---- registry owner gate ----

    #[test]
    fn registry_owner_install_is_once_only_and_idempotent() {
        let registry = HostAcceptorIdentityRegistry::new();
        let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry.install_owner(owner.clone()).unwrap();
        // Same Arc: idempotent.
        registry.install_owner(owner.clone()).unwrap();
        // Different owner: refused.
        let other: Arc<dyn Any + Send + Sync> = Arc::new(());
        assert!(matches!(
            registry.install_owner(other),
            Err(HostAcceptorError::OwnerAlreadyInstalled)
        ));
    }

    #[test]
    fn registry_identity_reservation_is_hidden_reversible_and_exactly_committed() {
        let registry = Arc::new(HostAcceptorIdentityRegistry::new());
        let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        let keypair = Arc::new(Keypair::generate());
        let reservation = registry
            .reserve_identity(
                Arc::clone(&owner),
                keypair.public_key(),
                Arc::clone(&keypair),
            )
            .expect("reserve host identity");
        assert!(
            registry.resolve(&keypair.public_key()).is_none(),
            "reserved identity must remain invisible to the acceptor"
        );
        assert!(matches!(
            registry.install_owner(Arc::clone(&owner)),
            Err(HostAcceptorError::RegistryReservationInProgress)
        ));
        drop(reservation);

        let replacement_owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry
            .install_owner(replacement_owner)
            .expect("dropped reservation restores an unowned registry");

        let committed_registry = Arc::new(HostAcceptorIdentityRegistry::new());
        let committed_owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        let committed_keypair = Arc::new(Keypair::generate());
        let (_inbox, committed_inbox_sender) = Inbox::new();
        committed_registry
            .reserve_identity(
                committed_owner,
                committed_keypair.public_key(),
                Arc::clone(&committed_keypair),
            )
            .expect("reserve committed host identity")
            .commit(committed_inbox_sender);
        assert!(
            committed_registry
                .resolve(&committed_keypair.public_key())
                .is_some(),
            "reservation commit makes the exact identity resolvable"
        );
    }

    #[test]
    fn registry_mutations_require_installed_owner() {
        let registry = HostAcceptorIdentityRegistry::new();
        let keypair = Keypair::generate();
        let (_inbox, inbox_sender) = Inbox::new();
        let stranger: Arc<dyn Any + Send + Sync> = Arc::new(());

        // No owner installed yet: register/remove fail closed.
        assert!(matches!(
            registry.register_identity(
                &stranger,
                keypair.public_key(),
                Arc::new(keypair.clone()),
                inbox_sender.clone(),
            ),
            Err(HostAcceptorError::OwnerNotInstalled)
        ));
        assert!(matches!(
            registry.remove_identity(&stranger, &keypair.public_key()),
            Err(HostAcceptorError::OwnerNotInstalled)
        ));

        let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry.install_owner(owner.clone()).unwrap();

        // Wrong owner: refused, registry never mutated.
        assert!(matches!(
            registry.register_identity(
                &stranger,
                keypair.public_key(),
                Arc::new(keypair.clone()),
                inbox_sender.clone(),
            ),
            Err(HostAcceptorError::OwnerMismatch)
        ));
        assert!(registry.resolve(&keypair.public_key()).is_none());

        registry
            .register_identity(
                &owner,
                keypair.public_key(),
                Arc::new(keypair.clone()),
                inbox_sender,
            )
            .unwrap();
        assert!(registry.resolve(&keypair.public_key()).is_some());
    }

    #[test]
    fn registry_rejects_duplicate_and_mismatched_identities() {
        let registry = HostAcceptorIdentityRegistry::new();
        let owner: Arc<dyn Any + Send + Sync> = Arc::new(());
        registry.install_owner(owner.clone()).unwrap();
        let keypair = Keypair::generate();
        let other_keypair = Keypair::generate();
        let (_inbox, inbox_sender) = Inbox::new();

        // A keypair that cannot sign for the claimed pubkey is a typed error.
        assert!(matches!(
            registry.register_identity(
                &owner,
                keypair.public_key(),
                Arc::new(other_keypair),
                inbox_sender.clone(),
            ),
            Err(HostAcceptorError::IdentityKeypairMismatch { .. })
        ));

        registry
            .register_identity(
                &owner,
                keypair.public_key(),
                Arc::new(keypair.clone()),
                inbox_sender.clone(),
            )
            .unwrap();
        assert!(matches!(
            registry.register_identity(
                &owner,
                keypair.public_key(),
                Arc::new(keypair),
                inbox_sender,
            ),
            Err(HostAcceptorError::IdentityAlreadyRegistered { .. })
        ));
    }

    // ---- spawn validation ----

    #[tokio::test]
    async fn spawn_rejects_short_pairing_secret() {
        let (registry, _owner) = registry_with_owner();
        let (_tx, rx) = watch::channel(String::from("{}"));
        let result = spawn_host_acceptor(HostAcceptorConfig {
            listen_tcp: "127.0.0.1:0".parse().unwrap(),
            advertise_address: None,
            registry,
            pairing: Some(HostPairingConfig {
                password: "short".to_string(),
                host_keypair: Arc::new(Keypair::generate()),
                participant_name: "host".to_string(),
                descriptor_json: rx,
            }),
            bounds: HostAcceptorBounds::default(),
        })
        .await;
        assert!(matches!(
            result,
            Err(HostAcceptorError::InvalidPairingSecret { .. })
        ));
    }

    #[tokio::test]
    async fn spawn_rejects_pairing_on_a_non_loopback_listener_before_bind() {
        let (registry, _owner) = registry_with_owner();
        let (_tx, rx) = watch::channel(String::from("{}"));
        let addr: SocketAddr = "0.0.0.0:0".parse().expect("wildcard socket address");
        let result = spawn_host_acceptor(HostAcceptorConfig {
            listen_tcp: addr,
            advertise_address: Some("tcp://203.0.113.10:7801".to_string()),
            registry,
            pairing: Some(HostPairingConfig {
                password: "a-very-long-host-pairing-secret-0123456789".to_string(),
                host_keypair: Arc::new(Keypair::generate()),
                participant_name: "host".to_string(),
                descriptor_json: rx,
            }),
            bounds: HostAcceptorBounds::default(),
        })
        .await;
        assert!(matches!(
            result,
            Err(HostAcceptorError::PairingRequiresLoopback { addr: rejected })
                if rejected == addr
        ));
    }

    #[tokio::test]
    async fn spawn_requires_advertise_address_for_non_loopback_bind() {
        let (registry, _owner) = registry_with_owner();
        let result = spawn_host_acceptor(HostAcceptorConfig {
            listen_tcp: "0.0.0.0:0".parse().unwrap(),
            advertise_address: None,
            registry,
            pairing: None,
            bounds: HostAcceptorBounds::default(),
        })
        .await;
        assert!(matches!(
            result,
            Err(HostAcceptorError::AdvertiseAddressRequired { .. })
        ));
    }

    #[tokio::test]
    async fn spawn_rejects_unparseable_advertise_address() {
        let (registry, _owner) = registry_with_owner();
        let result = spawn_host_acceptor(HostAcceptorConfig {
            listen_tcp: "127.0.0.1:0".parse().unwrap(),
            advertise_address: Some("not a peer address".to_string()),
            registry,
            pairing: None,
            bounds: HostAcceptorBounds::default(),
        })
        .await;
        assert!(matches!(
            result,
            Err(HostAcceptorError::InvalidAdvertiseAddress { .. })
        ));
    }

    #[tokio::test]
    async fn spawn_rejects_non_tcp_or_unusable_tcp_advertise_address() {
        for address in [
            "inproc://host",
            "uds:///tmp/host.sock",
            "tcp://host:0",
            "tcp://0.0.0.0:7801",
            "tcp://[::]:7801",
            "tcp://[::ffff:0.0.0.0]:7801",
            "tcp://*:7801",
            "tcp://[not-an-ip]:7801",
            "tcp://[127.0.0.1]:7801",
            "tcp://host",
            "tcp://host:not-a-port",
            "tcp://host:65536",
            "tcp://:7801",
            "tcp://2001:db8::1:7801",
        ] {
            let (registry, _owner) = registry_with_owner();
            let result = spawn_host_acceptor(HostAcceptorConfig {
                listen_tcp: "127.0.0.1:0".parse().unwrap(),
                advertise_address: Some(address.to_string()),
                registry,
                pairing: None,
                bounds: HostAcceptorBounds::default(),
            })
            .await;
            assert!(
                matches!(
                    result,
                    Err(HostAcceptorError::InvalidAdvertiseAddress { .. })
                ),
                "invalid TCP advertise route {address} must fail before the acceptor starts"
            );
        }
    }

    #[tokio::test]
    async fn spawn_accepts_dns_and_bracketed_ipv6_tcp_advertise_addresses() {
        for address in ["tcp://worker.example:7801", "tcp://[2001:db8::1]:7801"] {
            let (registry, _owner) = registry_with_owner();
            let handle = spawn_host_acceptor(HostAcceptorConfig {
                listen_tcp: "127.0.0.1:0".parse().unwrap(),
                advertise_address: Some(address.to_string()),
                registry,
                pairing: None,
                bounds: HostAcceptorBounds::default(),
            })
            .await
            .expect("valid advertised TCP route should not require DNS resolution at startup");
            assert_eq!(handle.advertised_address(), address);
            handle.shutdown().await;
        }
    }

    // ---- §11 demux rows over real loopback TCP ----

    /// Two identities on one acceptor: an envelope to A lands in A's inbox
    /// with `delivery == Acked` — the router's `ack.from == sent_to` validation
    /// IS the per-member ack-signing assertion — and the same holds for B.
    #[tokio::test]
    async fn acceptor_demuxes_by_to_and_acks_per_member() {
        let sender_keypair = Keypair::generate();
        let mut member_a = member_trusting(&sender_keypair.public_key(), true);
        let mut member_b = member_trusting(&sender_keypair.public_key(), true);
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        register_member(&registry, &owner, &member_b);
        let handle = spawn_loopback_acceptor(registry, None, HostAcceptorBounds::default()).await;
        let address = handle.advertised_address().to_string();

        let (router, _sender_inbox) = sender_router(&sender_keypair);
        let peer_a = trust_recipient(
            &router,
            "member-a",
            &member_a.keypair.public_key(),
            &address,
        );
        let peer_b = trust_recipient(
            &router,
            "member-b",
            &member_b.keypair.public_key(),
            &address,
        );

        let outcome_a = router
            .send(peer_a, test_message())
            .await
            .expect("send to A");
        assert!(
            matches!(outcome_a.delivery, PeerDeliveryOutcome::Acked),
            "ack for A must be signed by A's keypair (ack.from == sent_to)"
        );
        let items_a = member_a.inbox.try_drain_classified();
        assert_eq!(items_a.len(), 1, "envelope to A lands in A's inbox");
        match &items_a[0].item {
            InboxItem::External { envelope } => {
                assert_eq!(envelope.to, member_a.keypair.public_key());
            }
            _ => panic!("expected External"),
        }
        assert!(
            member_b.inbox.try_drain_classified().is_empty(),
            "B must not see A's envelope"
        );

        let outcome_b = router
            .send(peer_b, test_message())
            .await
            .expect("send to B");
        assert!(
            matches!(outcome_b.delivery, PeerDeliveryOutcome::Acked),
            "ack for B must be signed by B's keypair (ack.from == sent_to)"
        );
        let items_b = member_b.inbox.try_drain_classified();
        assert_eq!(items_b.len(), 1, "envelope to B lands in B's inbox");
        assert!(member_a.inbox.try_drain_classified().is_empty());

        handle.shutdown().await;
    }

    /// An envelope addressed to an unregistered identity is refused with no
    /// ack — the sender observes `PeerOffline`.
    #[tokio::test]
    async fn acceptor_send_to_unregistered_identity_reports_peer_offline() {
        let sender_keypair = Keypair::generate();
        let mut member_a = member_trusting(&sender_keypair.public_key(), true);
        let stranger = Keypair::generate();
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        let handle = spawn_loopback_acceptor(registry, None, HostAcceptorBounds::default()).await;
        let address = handle.advertised_address().to_string();

        let (router, _sender_inbox) = sender_router(&sender_keypair);
        let peer_stranger = trust_recipient(&router, "stranger", &stranger.public_key(), &address);

        let outcome = router.send(peer_stranger, test_message()).await;
        assert!(
            matches!(outcome, Err(SendError::PeerOffline)),
            "misaddressed envelope must surface sender-side as PeerOffline, got {outcome:?}"
        );
        assert!(member_a.inbox.try_drain_classified().is_empty());

        handle.shutdown().await;
    }

    /// Register → remove → send is rejected exactly like never-registered.
    #[tokio::test]
    async fn acceptor_rejects_registered_then_removed_identity() {
        let sender_keypair = Keypair::generate();
        let mut member_a = member_trusting(&sender_keypair.public_key(), true);
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        let handle =
            spawn_loopback_acceptor(registry.clone(), None, HostAcceptorBounds::default()).await;
        let address = handle.advertised_address().to_string();

        let (router, _sender_inbox) = sender_router(&sender_keypair);
        let peer_a = trust_recipient(
            &router,
            "member-a",
            &member_a.keypair.public_key(),
            &address,
        );

        let before = router
            .send(peer_a, test_message())
            .await
            .expect("send while registered");
        assert!(matches!(before.delivery, PeerDeliveryOutcome::Acked));
        assert_eq!(member_a.inbox.try_drain_classified().len(), 1);

        assert!(
            registry
                .remove_identity(&owner, &member_a.keypair.public_key())
                .expect("owner removal")
        );

        let after = router.send(peer_a, test_message()).await;
        assert!(
            matches!(after, Err(SendError::PeerOffline)),
            "removed identity must reject like never-registered, got {after:?}"
        );
        assert!(member_a.inbox.try_drain_classified().is_empty());

        handle.shutdown().await;
    }

    /// D1 type-level pin: `HostAcceptorConfig` has no peer-auth field to
    /// consult, so even an identity whose own inbox context was built from a
    /// `require_peer_auth = false` configuration still rejects unsigned
    /// envelopes at the acceptor's hardwired signature gate.
    #[tokio::test]
    async fn acceptor_rejects_unsigned_envelope_and_has_no_auth_knob() {
        let sender_keypair = Keypair::generate();
        let mut member_a = member_trusting(&sender_keypair.public_key(), false);
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        let handle = spawn_loopback_acceptor(registry, None, HostAcceptorBounds::default()).await;

        let envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: member_a.keypair.public_key(),
            kind: test_message(),
            sig: Signature::new([0u8; 64]), // unsigned
        };
        let bytes = envelope_frame_bytes(&envelope);

        let mut stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        stream.write_all(&bytes).await.expect("write frame");
        let mut buf = [0u8; 64];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("read completes before deadline")
            .expect("read");
        assert_eq!(
            read, 0,
            "connection must close with no ack for an unsigned envelope"
        );
        assert!(member_a.inbox.try_drain_classified().is_empty());

        handle.shutdown().await;
    }

    /// §11 byte-compat row at the acceptor: hand-encoded frame bytes (u32-BE
    /// length prefix + ciborium CBOR) drive the demux path end to end; the
    /// signature verifies, delivery happens, and the ack decodes with the
    /// addressed member as signer.
    #[tokio::test]
    async fn acceptor_accepts_hand_encoded_envelope_bytes() {
        let sender_keypair = Keypair::generate();
        let mut member_a = member_trusting(&sender_keypair.public_key(), true);
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        let handle = spawn_loopback_acceptor(registry, None, HostAcceptorBounds::default()).await;

        let mut envelope = Envelope {
            id: Uuid::new_v4(),
            from: sender_keypair.public_key(),
            to: member_a.keypair.public_key(),
            kind: test_message(),
            sig: Signature::new([0u8; 64]),
        };
        envelope.sign(&sender_keypair);
        let original_id = envelope.id;
        let bytes = envelope_frame_bytes(&envelope);

        let mut stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        stream.write_all(&bytes).await.expect("write frame");
        let mut framed = FramedRead::new(
            &mut stream,
            TransportCodec::new(crate::transport::MAX_PAYLOAD_SIZE),
        );
        let ack = match tokio::time::timeout(Duration::from_secs(5), framed.next()).await {
            Ok(Some(Ok(frame))) => frame.envelope,
            other => panic!("expected ack frame, got {other:?}"),
        };
        match ack.kind {
            MessageKind::Ack { in_reply_to } => assert_eq!(in_reply_to, original_id),
            _ => panic!("expected Ack"),
        }
        assert_eq!(ack.from, member_a.keypair.public_key());
        assert!(ack.verify());

        let items = member_a.inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::External { envelope } => assert_eq!(envelope.id, original_id),
            _ => panic!("expected External"),
        }

        handle.shutdown().await;
    }

    // ---- pre-auth bounds (§21.2) ----

    #[tokio::test]
    async fn acceptor_connection_cap_drops_overflow() {
        let (registry, _owner) = registry_with_owner();
        let bounds = HostAcceptorBounds {
            max_connections: 1,
            read_deadline: Duration::from_secs(30),
            pairing_rate: PairingRateLimit::default(),
        };
        let handle = spawn_loopback_acceptor(registry, None, bounds).await;

        // First connection stalls, holding the single permit.
        let _held = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect held");
        // Second connection must be dropped at the accept loop.
        let mut overflow = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect overflow");
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(5), overflow.read(&mut buf))
            .await
            .expect("overflow connection must be closed promptly")
            .expect("read");
        assert_eq!(read, 0, "over-cap connection must be dropped without data");
        assert_eq!(handle.overflow_dropped(), 1);

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn acceptor_read_deadline_closes_stalled_connection() {
        let (registry, _owner) = registry_with_owner();
        let bounds = HostAcceptorBounds {
            max_connections: 4,
            read_deadline: Duration::from_millis(200),
            pairing_rate: PairingRateLimit::default(),
        };
        let handle = spawn_loopback_acceptor(registry, None, bounds).await;

        // Open and stall: send nothing.
        let mut stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        let mut buf = [0u8; 8];
        let read = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("stalled connection must be closed by the read deadline")
            .expect("read");
        assert_eq!(read, 0, "stalled connection must be closed with no data");

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn host_pairing_entire_handshake_obeys_acceptor_read_deadline() {
        let (pairing, _tx, _host_keypair, _password) =
            pairing_fixture(r#"{"kind":"host","bootstrap_token":"tok-1"}"#);
        let (registry, _owner) = registry_with_owner();
        let bounds = HostAcceptorBounds {
            max_connections: 4,
            read_deadline: Duration::from_millis(200),
            pairing_rate: PairingRateLimit::default(),
        };
        let handle = spawn_loopback_acceptor(registry, Some(pairing), bounds).await;

        let stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        let mut reader = tokio::io::BufReader::new(stream);
        reader
            .get_mut()
            .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
            .await
            .expect("write hello");
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read challenge");
        let challenge: serde_json::Value = serde_json::from_str(&line).expect("challenge JSON");
        assert_eq!(challenge["kind"], PAIRING_CHALLENGE_KIND);

        // Stall before proof. This used to bypass the configured 200ms
        // acceptor budget and inherit read_pairing_message's independent
        // ten-second timeout. The one outer connection deadline must close
        // the socket promptly instead.
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(1), reader.read(&mut byte))
            .await
            .expect("the configured host acceptor deadline must close pairing");
        match read {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("stalled pairing must close without data, got {other:?}"),
        }

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn host_acceptor_shutdown_joins_accepted_pairing_tasks() {
        let (pairing, _tx, _host_keypair, _password) =
            pairing_fixture(r#"{"kind":"host","bootstrap_token":"tok-1"}"#);
        let (registry, _owner) = registry_with_owner();
        let bounds = HostAcceptorBounds {
            max_connections: 4,
            read_deadline: Duration::from_millis(200),
            pairing_rate: PairingRateLimit::default(),
        };
        let handle = spawn_loopback_acceptor(registry, Some(pairing), bounds).await;

        let stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        let mut reader = tokio::io::BufReader::new(stream);
        reader
            .get_mut()
            .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
            .await
            .expect("write hello");
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read challenge");

        tokio::time::timeout(Duration::from_secs(1), handle.shutdown())
            .await
            .expect("shutdown must join the deadline-bounded child task");

        // Successful shutdown is an ownership boundary: no detached pairing
        // task may retain the accepted socket after it returns.
        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_millis(50), reader.read(&mut byte))
            .await
            .expect("joined child must already have released its socket");
        match read {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("shutdown must leave no live child transport, got {other:?}"),
        }
    }

    // ---- host pairing branch (DEC-P2-3 / DEC-P2-4) ----

    fn pairing_fixture(
        descriptor: &str,
    ) -> (
        HostPairingConfig,
        watch::Sender<String>,
        Arc<Keypair>,
        String,
    ) {
        let password = "a-very-long-host-pairing-secret-0123456789".to_string();
        let host_keypair = Arc::new(Keypair::generate());
        let (tx, rx) = watch::channel(descriptor.to_string());
        (
            HostPairingConfig {
                password: password.clone(),
                host_keypair: host_keypair.clone(),
                participant_name: "mob-host".to_string(),
                descriptor_json: rx,
            },
            tx,
            host_keypair,
            password,
        )
    }

    async fn pairing_handshake(
        addr: SocketAddr,
        password: &str,
    ) -> (serde_json::Value, serde_json::Value) {
        let caller_keypair = Keypair::generate();
        let caller_pubkey = caller_keypair.public_key().to_pubkey_string();
        let caller_address = "tcp://127.0.0.1:9";
        let stream = TcpStream::connect(addr).await.expect("connect");
        let mut reader = tokio::io::BufReader::new(stream);
        reader
            .get_mut()
            .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
            .await
            .expect("write hello");
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read challenge");
        let challenge_msg: serde_json::Value = serde_json::from_str(&line).expect("challenge JSON");
        let challenge = challenge_msg["challenge"].as_str().expect("challenge str");
        let proof =
            comms_pairing_password_proof(password, challenge, &caller_pubkey, caller_address);
        let proof_msg = serde_json::json!({
            "kind": "meerkat_pairing_proof",
            "caller": {
                "name": "caller",
                "address": caller_address,
                "identity": {
                    "kind": "ed25519_public_key",
                    "public_key": caller_pubkey,
                }
            },
            "password_proof": proof,
        });
        let mut proof_line = serde_json::to_vec(&proof_msg).expect("proof encode");
        proof_line.push(b'\n');
        reader
            .get_mut()
            .write_all(&proof_line)
            .await
            .expect("write proof");
        line.clear();
        reader.read_line(&mut line).await.expect("read complete");
        let complete: serde_json::Value = serde_json::from_str(&line).expect("complete JSON");
        (challenge_msg, complete)
    }

    /// DEC-P2-3 regression pin: host pairing proves possession of the HOST
    /// secret (the challenge targets the HOST identity), returns the
    /// daemon-supplied descriptor verbatim, and writes NOTHING — the
    /// registered identity's trust view is untouched and the registry is
    /// never mutated. Type-level backstop: `HostPairingConfig` holds no
    /// router/trust-store/persistence handles at all.
    #[tokio::test]
    async fn host_pairing_returns_descriptor_and_installs_no_trust() {
        let descriptor =
            r#"{"kind":"host","address":"tcp://127.0.0.1:4400","bootstrap_token":"tok-1"}"#;
        let (pairing, _tx, host_keypair, password) = pairing_fixture(descriptor);
        let sender_keypair = Keypair::generate();
        let member_a = member_trusting(&sender_keypair.public_key(), true);
        let (registry, owner) = registry_with_owner();
        register_member(&registry, &owner, &member_a);
        let trusted_before = member_a.trusted.read().clone();
        let handle = spawn_loopback_acceptor(
            registry.clone(),
            Some(pairing),
            HostAcceptorBounds::default(),
        )
        .await;

        let (challenge_msg, complete) = pairing_handshake(handle.local_addr(), &password).await;

        // Challenge targets the HOST identity at the advertised address.
        assert_eq!(
            challenge_msg["target"]["identity"]["public_key"],
            serde_json::Value::String(host_keypair.public_key().to_pubkey_string()),
        );
        assert_eq!(
            challenge_msg["target"]["address"],
            serde_json::Value::String(handle.advertised_address().to_string()),
        );

        assert_eq!(complete["kind"], "meerkat_pairing_complete");
        assert_eq!(
            complete["binding"],
            serde_json::from_str::<serde_json::Value>(descriptor).unwrap(),
            "the binding object must be the daemon-supplied descriptor verbatim"
        );
        assert_eq!(complete["comms_name"], "mob-host");

        // Zero trust writes: the member's trust view is unchanged.
        let trusted_after = member_a.trusted.read().clone();
        assert_eq!(
            trusted_before, trusted_after,
            "host pairing must not install trust"
        );
        assert_eq!(
            trusted_after.len(),
            1,
            "only the pre-existing sender entry remains"
        );
        // Registry unchanged: still exactly the one registered identity.
        assert!(registry.resolve(&member_a.keypair.public_key()).is_some());

        handle.shutdown().await;
    }

    /// DEC-P2-4: the descriptor watch slot serves the CURRENT value — after
    /// a re-mint pushes a new serialized descriptor, the next pairing
    /// returns the new one.
    #[tokio::test]
    async fn host_pairing_serves_reminted_descriptor_from_watch() {
        let first = r#"{"kind":"host","bootstrap_token":"tok-1"}"#;
        let second = r#"{"kind":"host","bootstrap_token":"tok-2"}"#;
        let (pairing, tx, _host_keypair, password) = pairing_fixture(first);
        let (registry, _owner) = registry_with_owner();
        let handle =
            spawn_loopback_acceptor(registry, Some(pairing), HostAcceptorBounds::default()).await;

        let (_, complete_one) = pairing_handshake(handle.local_addr(), &password).await;
        assert_eq!(complete_one["binding"]["bootstrap_token"], "tok-1");

        tx.send(second.to_string())
            .expect("descriptor re-mint push");

        let (_, complete_two) = pairing_handshake(handle.local_addr(), &password).await;
        assert_eq!(
            complete_two["binding"]["bootstrap_token"], "tok-2",
            "pairing must serve the re-minted descriptor"
        );

        handle.shutdown().await;
    }

    #[tokio::test]
    async fn host_pairing_wrong_password_is_refused() {
        let (pairing, _tx, _host_keypair, _password) =
            pairing_fixture(r#"{"kind":"host","bootstrap_token":"tok-1"}"#);
        let (registry, _owner) = registry_with_owner();
        let handle =
            spawn_loopback_acceptor(registry, Some(pairing), HostAcceptorBounds::default()).await;

        let caller_keypair = Keypair::generate();
        let caller_pubkey = caller_keypair.public_key().to_pubkey_string();
        let stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        let mut reader = tokio::io::BufReader::new(stream);
        reader
            .get_mut()
            .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
            .await
            .expect("write hello");
        let mut line = String::new();
        reader.read_line(&mut line).await.expect("read challenge");
        let challenge_msg: serde_json::Value = serde_json::from_str(&line).expect("challenge JSON");
        let challenge = challenge_msg["challenge"].as_str().expect("challenge str");
        let proof = comms_pairing_password_proof(
            "the-wrong-password-entirely-0123456789012345",
            challenge,
            &caller_pubkey,
            "tcp://127.0.0.1:9",
        );
        let proof_msg = serde_json::json!({
            "kind": "meerkat_pairing_proof",
            "caller": {
                "name": "caller",
                "address": "tcp://127.0.0.1:9",
                "identity": { "kind": "ed25519_public_key", "public_key": caller_pubkey },
            },
            "password_proof": proof,
        });
        let mut proof_line = serde_json::to_vec(&proof_msg).expect("proof encode");
        proof_line.push(b'\n');
        reader
            .get_mut()
            .write_all(&proof_line)
            .await
            .expect("write proof");
        line.clear();
        let read = reader.read_line(&mut line).await.expect("read refusal");
        assert_eq!(
            read, 0,
            "wrong password proof must close with no pairing-complete"
        );

        handle.shutdown().await;
    }

    /// Pairing attempts beyond the per-source budget are refused BEFORE the
    /// challenge is issued.
    #[tokio::test]
    async fn host_pairing_rate_limit_refuses_before_challenge() {
        let (pairing, _tx, _host_keypair, _password) =
            pairing_fixture(r#"{"kind":"host","bootstrap_token":"tok-1"}"#);
        let (registry, _owner) = registry_with_owner();
        let bounds = HostAcceptorBounds {
            max_connections: 8,
            read_deadline: Duration::from_secs(10),
            pairing_rate: PairingRateLimit {
                max_attempts: 2,
                window: Duration::from_secs(60),
            },
        };
        let handle = spawn_loopback_acceptor(registry, Some(pairing), bounds).await;

        for attempt in 0..2 {
            let stream = TcpStream::connect(handle.local_addr())
                .await
                .expect("connect");
            let mut reader = tokio::io::BufReader::new(stream);
            reader
                .get_mut()
                .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
                .await
                .expect("write hello");
            let mut line = String::new();
            let read = reader.read_line(&mut line).await.expect("read challenge");
            assert!(
                read > 0,
                "attempt {attempt} within budget must receive a challenge"
            );
            let challenge_msg: serde_json::Value =
                serde_json::from_str(&line).expect("challenge JSON");
            assert_eq!(challenge_msg["kind"], "meerkat_pairing_challenge");
        }

        // Third attempt from the same source: refused before the challenge.
        let stream = TcpStream::connect(handle.local_addr())
            .await
            .expect("connect");
        let mut reader = tokio::io::BufReader::new(stream);
        reader
            .get_mut()
            .write_all(b"{\"kind\":\"meerkat_pairing_hello\",\"version\":1}\n")
            .await
            .expect("write hello");
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("refused attempt must close promptly");
        // A refusal closes the socket without a challenge. The close may
        // surface as EOF or, on platforms that RST a socket closed with
        // unread inbound data (macOS), as ConnectionReset — both are the
        // refused shape; a challenge line is the failure.
        match read {
            Ok(0) => {}
            Ok(n) => {
                panic!("over-budget pairing attempt must be refused, got {n}-byte reply: {line}")
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(error) => panic!("unexpected read error shape: {error}"),
        }
        assert_eq!(handle.pairing_refused(), 1);

        handle.shutdown().await;
    }

    #[test]
    fn pairing_rate_limiter_zero_budget_refuses_everything() {
        let limiter = PairingRateLimiter::new(PairingRateLimit {
            max_attempts: 0,
            window: Duration::from_secs(60),
        });
        assert!(!limiter.try_admit("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn pairing_rate_limiter_is_per_source() {
        let limiter = PairingRateLimiter::new(PairingRateLimit {
            max_attempts: 1,
            window: Duration::from_secs(60),
        });
        let a: IpAddr = "127.0.0.1".parse().unwrap();
        let b: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(limiter.try_admit(a));
        assert!(!limiter.try_admit(a), "source A exhausted its budget");
        assert!(limiter.try_admit(b), "source B has an independent budget");
    }
}
