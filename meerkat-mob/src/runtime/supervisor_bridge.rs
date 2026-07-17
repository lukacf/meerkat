use crate::MobError;
use crate::definition::SupervisorBridgeEndpointConfig;
use crate::store::{SupervisorAuthorityBridgeAuthority, SupervisorAuthorityRecord};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerAddress, PeerId, PeerRoute, PeerTransport, SendReceipt,
    TrustedPeerDescriptor,
};
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate, TerminalityClass};
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::HandlingMode;
use meerkat_runtime::meerkat_machine::dsl as mm_dsl;
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::{Mutex, RwLock};

#[cfg(test)]
static FAIL_NEXT_BRIDGE_TRUST_ADD_RECONCILE_FOR_PEER: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
static FAIL_NEXT_BRIDGE_TRUST_REMOVE_RECONCILE_FOR_PEER: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);
#[cfg(test)]
static FAIL_NEXT_FIXED_PORT_ROTATION_REBUILD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(all(test, not(target_arch = "wasm32")))]
static FAIL_NEXT_SUPERVISOR_INSTALL_AFTER_LISTENER_BIND: std::sync::Mutex<Option<String>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) fn fail_next_bridge_trust_add_reconcile_for_test(peer_id: &str) {
    if let Ok(mut target) = FAIL_NEXT_BRIDGE_TRUST_ADD_RECONCILE_FOR_PEER.lock() {
        *target = Some(peer_id.to_string());
    }
}

#[cfg(test)]
pub(super) fn fail_next_bridge_trust_remove_reconcile_for_test(peer_id: &str) {
    if let Ok(mut target) = FAIL_NEXT_BRIDGE_TRUST_REMOVE_RECONCILE_FOR_PEER.lock() {
        *target = Some(peer_id.to_string());
    }
}

#[cfg(test)]
pub(super) fn fail_next_fixed_port_rotation_rebuild_for_test() {
    FAIL_NEXT_FIXED_PORT_ROTATION_REBUILD.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Outcome of installing recipient trust ahead of a routed bridge send
/// (see [`MobSupervisorBridge::trust_recipient`]).
///
/// Fail-closed rollback after a non-confirmed authorization outcome must be
/// scoped to trust installed by THAT call: rolling back `AlreadyTrusted`
/// would rip out trust that an earlier confirmed terminality legitimately
/// established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecipientTrustInstall {
    /// This call installed the trust; a failure path must remove it.
    NewlyInstalled,
    /// The recipient was already a trusted direct peer endpoint before this
    /// call; a failure path must leave the pre-existing trust in place.
    AlreadyTrusted,
}

/// The side-effect boundary for a supervisor bridge request failure.
///
/// A failure before [`SendReceipt::PeerRequestSent`] certifies that the
/// receiver could not have observed the command. Once that receipt exists,
/// any missing or undecodable terminal response leaves command application
/// ambiguous unless the response itself is an authenticated rejection.
#[derive(Debug)]
pub(crate) enum BridgeRequestFailure {
    BeforeSend(MobError),
    AfterSend(MobError),
}

impl BridgeRequestFailure {
    fn into_error(self) -> MobError {
        match self {
            Self::BeforeSend(error) | Self::AfterSend(error) => error,
        }
    }
}

pub(crate) struct MobSupervisorBridge {
    participant_name: String,
    endpoint_config: SupervisorBridgeEndpointConfig,
    authority: StdRwLock<SupervisorAuthorityRecord>,
    runtime: RwLock<Arc<meerkat_comms::CommsRuntime>>,
    dsl: RwLock<Arc<meerkat_runtime::HandleDslAuthority>>,
    buffered_candidates: Mutex<VecDeque<PeerInputCandidate>>,
    /// Shared-intake coordination with the mob upcall responder (DEC-U3):
    /// fired whenever foreign candidates are pushed into the shared buffer
    /// or `buffer_and_extract` retains request-class candidates, so neither
    /// drain loop can starve the other.
    intake_notify: Arc<tokio::sync::Notify>,
    /// Authority gate for concurrent bridge round-trips (DEC-P6E-14,
    /// FLAG-P6E-2). Reply correlation is per-request (`in_reply_to`
    /// envelope id + canonical responder identity in `wait_for_response`),
    /// so ordinary sends — including member long-polls — interleave freely
    /// under READ guards. Durable rotations and fixed-port temporary
    /// authority swaps take the WRITE guard: a concurrent ordinary send
    /// during a swap would ride the wrong authority, while a concurrent
    /// authority/runtime/spec read could expose the temporary identity.
    /// Every request retains the READ guard through response terminality. Even
    /// a private TCP listener shares `buffered_candidates` with the bridge: a
    /// concurrent drain can move its response there, so rotation must not clear
    /// the shared handoff while any old-authority waiter remains active.
    authority_gate: RwLock<()>,
    /// A throwaway shared-ingress runtime is keyed by the requested durable
    /// authority. Two concurrent recovery probes for the same historical
    /// authority therefore have the same signing key and cannot own distinct
    /// acceptor inboxes at once. Serialize only the true alternate-authority
    /// lane; current-authority traffic continues to use the durable runtime
    /// concurrently under `authority_gate.read()`.
    alternate_authority_probe_gate: Arc<Mutex<()>>,
    /// Serializes the complete generated peer-projection transition through
    /// canonical trust-store reconciliation. The DSL mutex alone cannot cover
    /// the awaited reconciliation tail: without this gate, a concurrent
    /// add/remove can advance the projection epoch and make the first call's
    /// exact generated obligation stale before it mints mutation authority.
    trust_reconcile_gate: Mutex<()>,
    /// Process-scoped TCP ingress shared with local mob-member identities.
    /// When present, the bridge runtime itself owns no TCP listener: this
    /// exact lease is the single routable projection of its signing identity.
    #[cfg(not(target_arch = "wasm32"))]
    controlling_acceptor: Mutex<Option<SupervisorControllingAcceptorBinding>>,
    /// Synchronous address projection used while constructing signed peer
    /// descriptors and request reply endpoints. It is published only after the
    /// matching identity lease has been installed on the shared acceptor.
    #[cfg(not(target_arch = "wasm32"))]
    controlling_reply_endpoint: StdRwLock<Option<PeerAddress>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SupervisorControllingAcceptorBinding {
    config: super::builder::ControllingAcceptorConfig,
    lease: Option<super::builder::ControllingAcceptorRegistrationLease>,
}

#[cfg(not(target_arch = "wasm32"))]
struct TemporarySupervisorControllingAcceptorBinding {
    registration: Option<SupervisorControllingAcceptorBinding>,
    reply_endpoint: PeerAddress,
    serialization_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl SupervisorControllingAcceptorBinding {
    fn new(
        config: super::builder::ControllingAcceptorConfig,
        lease: super::builder::ControllingAcceptorRegistrationLease,
    ) -> Self {
        Self {
            config,
            lease: Some(lease),
        }
    }

    fn lease(&self) -> Result<&super::builder::ControllingAcceptorRegistrationLease, MobError> {
        self.lease.as_ref().ok_or_else(|| {
            MobError::Internal("live supervisor acceptor binding lost its exact lease".to_string())
        })
    }

    async fn cleanup(&mut self) -> Result<(), MobError> {
        let Some(lease) = self.lease.as_ref() else {
            return Ok(());
        };
        self.config.remove(lease).await?;
        self.lease = None;
        Ok(())
    }

    /// The shared acceptor's atomic replacement already removed this exact
    /// lease. Disarm Drop so successful commit has no best-effort cleanup tail.
    fn disarm_after_atomic_replacement(&mut self) {
        self.lease = None;
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for SupervisorControllingAcceptorBinding {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let config = self.config.clone();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(
                "dropping a live supervisor process-ingress lease outside a Tokio runtime; exact cleanup could not be scheduled"
            );
            return;
        };
        let _cleanup_task = runtime.spawn(async move {
            if let Err(error) = config.remove(&lease).await {
                tracing::warn!(%error, "failed to remove dropped supervisor process-ingress lease");
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl TemporarySupervisorControllingAcceptorBinding {
    async fn cleanup(&mut self) -> Result<(), MobError> {
        if let Some(registration) = self.registration.as_mut() {
            registration.cleanup().await?;
        }
        self.registration = None;
        self.serialization_guard = None;
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for TemporarySupervisorControllingAcceptorBinding {
    fn drop(&mut self) {
        let Some(mut registration) = self.registration.take() else {
            return;
        };
        // Move the owned serialization guard into the cleanup task. A queued
        // probe cannot claim the same historical key until cancellation has
        // compare-removed this inbox, eliminating the drop-vs-next-call race.
        let serialization_guard = self.serialization_guard.take();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(
                "dropping a temporary supervisor process-ingress lease outside a Tokio runtime; exact cleanup could not be scheduled"
            );
            return;
        };
        let _cleanup_task = runtime.spawn(async move {
            if let Err(error) = registration.cleanup().await {
                tracing::warn!(%error, "failed to remove cancelled temporary supervisor process-ingress lease");
            }
            drop(serialization_guard);
        });
    }
}

/// Typed reachability domain for the supervisor bridge's advertised
/// self-address. The bridge advertises one address per domain — in-process
/// loopback (the inproc registry, keyed by participant name) or routable
/// network (the bind/advertised address). The domain is a function of the
/// recipient's *transport* (a reachability fact), never of the recipient's
/// identity, so two recipients on the same transport always receive the same
/// self-address.
enum SupervisorReachability<'a> {
    /// The peer shares this runtime and routes through the inproc registry.
    InProcess,
    /// The peer is reachable only over the network bind/advertised address.
    Routable(&'a meerkat_comms::CommsRuntime),
}

/// Reply transport carried by one supervisor Request.
///
/// Native network bridge requests name a signed TCP callback endpoint: either
/// the live runtime's routable endpoint or the distinct listener owned by an
/// ephemeral alternate-authority probe. In-process alternate-authority probes
/// reply through their isolated inproc route. UDS request/response fails closed
/// because authenticated UDS ingress cannot authorize a TCP callback.
enum SupervisorRequestReplyEndpoint {
    Live,
    #[cfg(not(target_arch = "wasm32"))]
    EphemeralProbe(PeerAddress),
    /// In-process alternate-authority probes receive their response through
    /// the probe runtime's pubkey route. Advertising the durable live TCP
    /// endpoint would route the response to the wrong signing identity.
    #[cfg(not(target_arch = "wasm32"))]
    InProcessProbe,
}

impl<'a> SupervisorReachability<'a> {
    /// Classify the reachability domain a recipient on `transport` lives in.
    /// Inproc recipients can only route via in-process loopback; UDS/TCP
    /// recipients require the routable network address.
    fn for_recipient_transport(
        transport: PeerTransport,
        runtime: &'a meerkat_comms::CommsRuntime,
    ) -> Self {
        match transport {
            // Only confirmed in-process recipients use loopback. Every network
            // transport — UDS, TCP, and any future `#[non_exhaustive]` variant —
            // resolves via the routable address (which itself fails closed on an
            // unspecified bind), never fabricating a loopback for a non-inproc
            // peer.
            PeerTransport::Inproc => Self::InProcess,
            _ => Self::Routable(runtime),
        }
    }
}

pub(crate) struct PreparedSupervisorBridgeRotation {
    authority: SupervisorAuthorityRecord,
    prebuilt: Option<PreparedSupervisorRuntime>,
}

/// Fully configured supervisor runtime whose process-global inproc route is
/// still dormant. Listener startup and generated DSL authority installation
/// are allowed during preparation; only commit may publish the stable live
/// participant name.
struct PreparedSupervisorRuntime {
    runtime: meerkat_comms::PreparedCommsRuntime,
    dsl: Arc<meerkat_runtime::HandleDslAuthority>,
    request_response_authority: meerkat_comms::PeerRequestResponseAuthority,
}

impl PreparedSupervisorRuntime {
    fn runtime(&self) -> &meerkat_comms::CommsRuntime {
        self.runtime.runtime()
    }

    fn publish(
        self,
    ) -> Result<
        (
            Arc<meerkat_comms::CommsRuntime>,
            Arc<meerkat_runtime::HandleDslAuthority>,
        ),
        MobError,
    > {
        let Self {
            runtime,
            dsl,
            request_response_authority,
        } = self;
        let runtime = runtime.publish().map_err(|error| {
            MobError::Internal(format!(
                "failed to publish prepared mob supervisor comms runtime: {error}"
            ))
        })?;
        let runtime = Arc::new(runtime);
        runtime.install_peer_request_response_authority(request_response_authority);
        Ok((runtime, dsl))
    }

    fn publish_replacing_recoverable(
        self,
        current: &meerkat_comms::CommsRuntime,
    ) -> Result<
        (
            Arc<meerkat_comms::CommsRuntime>,
            Arc<meerkat_runtime::HandleDslAuthority>,
        ),
        (Box<Self>, MobError),
    > {
        let Self {
            runtime,
            dsl,
            request_response_authority,
        } = self;
        let runtime = match runtime.publish_replacing_recoverable(current) {
            Ok(runtime) => runtime,
            Err((runtime, error)) => {
                return Err((
                    Box::new(Self {
                        runtime: *runtime,
                        dsl,
                        request_response_authority,
                    }),
                    MobError::Internal(format!(
                        "failed to publish prepared mob supervisor runtime replacement: {error}"
                    )),
                ));
            }
        };
        let runtime = Arc::new(runtime);
        runtime.install_peer_request_response_authority(request_response_authority);
        Ok((runtime, dsl))
    }

    async fn stop_listeners(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        self.runtime.runtime().stop_listeners_for_rebind().await;
    }
}

impl MobSupervisorBridge {
    pub(crate) fn preflight_ingress_ownership(
        endpoint_config: Option<&SupervisorBridgeEndpointConfig>,
        controlling_acceptor: Option<&super::builder::ControllingAcceptorConfig>,
    ) -> Result<(), MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        if controlling_acceptor.is_some() && endpoint_config.is_some() {
            return Err(MobError::WiringError(
                "mob supervisor bridge cannot configure both backend.external.supervisor_bridge and process [mob_host] ingress; the process acceptor is the single TCP listener"
                    .to_string(),
            ));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (endpoint_config, controlling_acceptor);
        Ok(())
    }

    pub(crate) async fn new(
        mob_id: &crate::MobId,
        authority: SupervisorAuthorityRecord,
        endpoint_config: Option<SupervisorBridgeEndpointConfig>,
    ) -> Result<Self, MobError> {
        Self::new_with_controlling_acceptor(mob_id, authority, endpoint_config, None).await
    }

    pub(crate) async fn new_with_controlling_acceptor(
        mob_id: &crate::MobId,
        authority: SupervisorAuthorityRecord,
        endpoint_config: Option<SupervisorBridgeEndpointConfig>,
        controlling_acceptor: Option<super::builder::ControllingAcceptorConfig>,
    ) -> Result<Self, MobError> {
        Self::preflight_ingress_ownership(endpoint_config.as_ref(), controlling_acceptor.as_ref())?;
        let participant_name = format!("{mob_id}/__mob_supervisor__");
        let endpoint_config = endpoint_config.unwrap_or_default();
        #[cfg(not(target_arch = "wasm32"))]
        let uses_controlling_acceptor = controlling_acceptor.is_some();
        #[cfg(not(target_arch = "wasm32"))]
        let (runtime, dsl) = Self::build_runtime_with_tcp_listener(
            &participant_name,
            &authority,
            &endpoint_config,
            !uses_controlling_acceptor,
        )
        .await?;
        #[cfg(target_arch = "wasm32")]
        let (runtime, dsl) =
            Self::build_runtime(&participant_name, &authority, &endpoint_config).await?;

        #[cfg(not(target_arch = "wasm32"))]
        let (controlling_acceptor, controlling_reply_endpoint) = if let Some(config) =
            controlling_acceptor
        {
            let registration = Self::acceptor_registration_for_runtime(&runtime)?;
            let lease = config
                .register(participant_name.clone(), registration)
                .await?;
            let mut binding = SupervisorControllingAcceptorBinding::new(config, lease);
            let reply_endpoint = match Self::validated_controlling_reply_endpoint(
                &binding.lease()?.advertised_address,
            ) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    return match binding.cleanup().await {
                        Ok(()) => Err(error),
                        Err(cleanup_error) => Err(MobError::Internal(format!(
                            "supervisor process-ingress startup failed: {error}; additionally failed to unwind its durable lease: {cleanup_error}"
                        ))),
                    };
                }
            };
            (Some(binding), Some(reply_endpoint))
        } else {
            (None, None)
        };

        Ok(Self {
            participant_name,
            endpoint_config,
            authority: StdRwLock::new(authority),
            runtime: RwLock::new(runtime),
            dsl: RwLock::new(dsl),
            buffered_candidates: Mutex::new(VecDeque::new()),
            intake_notify: Arc::new(tokio::sync::Notify::new()),
            authority_gate: RwLock::new(()),
            alternate_authority_probe_gate: Arc::new(Mutex::new(())),
            trust_reconcile_gate: Mutex::new(()),
            #[cfg(not(target_arch = "wasm32"))]
            controlling_acceptor: Mutex::new(controlling_acceptor),
            #[cfg(not(target_arch = "wasm32"))]
            controlling_reply_endpoint: StdRwLock::new(controlling_reply_endpoint),
        })
    }

    async fn build_runtime(
        participant_name: &str,
        authority: &SupervisorAuthorityRecord,
        endpoint_config: &SupervisorBridgeEndpointConfig,
    ) -> Result<
        (
            Arc<meerkat_comms::CommsRuntime>,
            Arc<meerkat_runtime::HandleDslAuthority>,
        ),
        MobError,
    > {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::build_runtime_with_tcp_listener(
                participant_name,
                authority,
                endpoint_config,
                true,
            )
            .await
        }
        #[cfg(target_arch = "wasm32")]
        {
            Self::build_runtime_with_tcp_listener(
                participant_name,
                authority,
                endpoint_config,
                false,
            )
            .await
        }
    }

    async fn build_runtime_with_tcp_listener(
        participant_name: &str,
        authority: &SupervisorAuthorityRecord,
        endpoint_config: &SupervisorBridgeEndpointConfig,
        start_tcp_listener: bool,
    ) -> Result<
        (
            Arc<meerkat_comms::CommsRuntime>,
            Arc<meerkat_runtime::HandleDslAuthority>,
        ),
        MobError,
    > {
        Self::prepare_runtime_with_tcp_listener(
            participant_name,
            authority,
            endpoint_config,
            start_tcp_listener,
        )
        .await?
        .publish()
    }

    async fn prepare_runtime_with_tcp_listener(
        participant_name: &str,
        authority: &SupervisorAuthorityRecord,
        endpoint_config: &SupervisorBridgeEndpointConfig,
        start_tcp_listener: bool,
    ) -> Result<PreparedSupervisorRuntime, MobError> {
        let prepared =
            meerkat_comms::CommsRuntime::prepare_inproc_only_with_keypair_and_silent_intents(
                participant_name,
                None,
                authority.keypair(),
                Arc::new(HashSet::new()),
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "failed to construct mob supervisor comms runtime '{participant_name}': {error}"
                ))
            })?;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut prepared = prepared;
            if start_tcp_listener {
                let bind_address = Self::supervisor_bind_address(endpoint_config)?;
                prepared
                    .runtime_mut()
                    .set_listen_tcp_for_unstarted_runtime(bind_address)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to configure mob supervisor comms listener '{participant_name}': {error}"
                        ))
                    })?;
                if let Some(advertised_address) = endpoint_config.advertised_address.clone() {
                    prepared
                        .runtime_mut()
                        .set_advertise_address_for_unstarted_runtime(advertised_address)
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "failed to configure mob supervisor advertised address '{participant_name}': {error}"
                            ))
                        })?;
                }
                prepared
                    .runtime_mut()
                    .start_listeners()
                    .await
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to start mob supervisor comms listener '{participant_name}': {error}"
                        ))
                    })?;
            }
            let (dsl, request_response_authority) =
                match Self::prepare_supervisor_peer_request_response_authority(
                    prepared.runtime(),
                    participant_name,
                ) {
                    Ok(authority) => authority,
                    Err(error) => {
                        prepared.runtime().stop_listeners_for_rebind().await;
                        return Err(error);
                    }
                };
            Ok(PreparedSupervisorRuntime {
                runtime: prepared,
                dsl,
                request_response_authority,
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = start_tcp_listener;
            let (dsl, request_response_authority) =
                Self::prepare_supervisor_peer_request_response_authority(
                    prepared.runtime(),
                    participant_name,
                )?;
            Ok(PreparedSupervisorRuntime {
                runtime: prepared,
                dsl,
                request_response_authority,
            })
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn acceptor_registration_for_runtime(
        runtime: &meerkat_comms::CommsRuntime,
    ) -> Result<super::builder::MemberAcceptorRegistration, MobError> {
        let payload = CoreCommsRuntime::host_acceptor_registration_payload(runtime).ok_or_else(
            || {
                MobError::WiringError(
                    "mob supervisor comms runtime did not expose host-acceptor registration material"
                        .to_string(),
                )
            },
        )?;
        let material = payload
            .downcast::<meerkat_comms::HostAcceptorRegistrationMaterial>()
            .map_err(|_| {
                MobError::WiringError(
                    "mob supervisor comms runtime exposed incompatible host-acceptor registration material"
                        .to_string(),
                )
            })?;
        Ok(super::builder::MemberAcceptorRegistration {
            pubkey: material.pubkey,
            keypair: Arc::clone(&material.keypair),
            inbox_sender: material.inbox_sender.clone(),
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn validated_controlling_reply_endpoint(address: &str) -> Result<PeerAddress, MobError> {
        let endpoint = PeerAddress::parse(address).map_err(|error| {
            MobError::WiringError(format!(
                "invalid process host-ingress advertised address '{address}': {error}"
            ))
        })?;
        if endpoint.transport() != PeerTransport::Tcp {
            return Err(MobError::WiringError(format!(
                "process host-ingress advertised address must use tcp transport, got '{}'",
                endpoint.transport()
            )));
        }
        let port = endpoint
            .endpoint()
            .rsplit_once(':')
            .and_then(|(host, port)| (!host.is_empty()).then_some(port))
            .and_then(|port| port.parse::<u16>().ok());
        if port.is_none_or(|port| port == 0) {
            return Err(MobError::WiringError(format!(
                "process host-ingress advertised address '{endpoint}' has no valid nonzero TCP port"
            )));
        }
        Ok(endpoint)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn uses_controlling_acceptor(&self) -> bool {
        self.controlling_reply_endpoint
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    #[cfg(target_arch = "wasm32")]
    fn uses_controlling_acceptor(&self) -> bool {
        false
    }

    pub(crate) fn require_supported_rotation_endpoint(
        &self,
        transport: PeerTransport,
    ) -> Result<(), MobError> {
        match transport {
            PeerTransport::Inproc => Ok(()),
            PeerTransport::Uds => Err(MobError::WiringError(
                "supervisor request/response over UDS is unsupported: authenticated UDS ingress cannot authorize the required TCP reply endpoint"
                    .to_string(),
            )),
            PeerTransport::Tcp => {
                if self.uses_controlling_acceptor()
                    || Self::has_fixed_supervisor_bind_port(&self.endpoint_config)?
                {
                    Ok(())
                } else {
                    Err(MobError::WiringError(
                        "supervisor rotation with TCP peers requires a stable callback endpoint; configure a nonzero fixed supervisor bridge bind port or process [mob_host] shared ingress"
                            .to_string(),
                    ))
                }
            }
            _ => Err(MobError::WiringError(format!(
                "supervisor rotation does not support peer transport '{transport}'"
            ))),
        }
    }

    async fn prepare_live_runtime(
        &self,
        authority: &SupervisorAuthorityRecord,
    ) -> Result<PreparedSupervisorRuntime, MobError> {
        Self::prepare_runtime_with_tcp_listener(
            &self.participant_name,
            authority,
            &self.endpoint_config,
            !self.uses_controlling_acceptor(),
        )
        .await
    }

    async fn build_probe_runtime(
        &self,
        participant_name: &str,
        authority: &SupervisorAuthorityRecord,
        start_tcp_listener: bool,
    ) -> Result<
        (
            Arc<meerkat_comms::CommsRuntime>,
            Arc<meerkat_runtime::HandleDslAuthority>,
        ),
        MobError,
    > {
        Self::build_runtime_with_tcp_listener(
            participant_name,
            authority,
            &self.endpoint_config,
            start_tcp_listener,
        )
        .await
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn register_temporary_controlling_acceptor_runtime(
        &self,
        logical_owner: String,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        serialization_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    ) -> Result<Option<TemporarySupervisorControllingAcceptorBinding>, MobError> {
        let config = self
            .controlling_acceptor
            .lock()
            .await
            .as_ref()
            .map(|binding| binding.config.clone());
        let Some(config) = config else {
            return Ok(None);
        };
        let registration = Self::acceptor_registration_for_runtime(runtime)?;
        let lease = config.register(logical_owner, registration).await?;
        let mut registration = SupervisorControllingAcceptorBinding::new(config, lease);
        let reply_endpoint = match Self::validated_controlling_reply_endpoint(
            &registration.lease()?.advertised_address,
        ) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                return match registration.cleanup().await {
                    Ok(()) => Err(error),
                    Err(cleanup_error) => Err(MobError::Internal(format!(
                        "temporary supervisor process-ingress endpoint was invalid: {error}; additionally failed to unwind its lease: {cleanup_error}"
                    ))),
                };
            }
        };
        Ok(Some(TemporarySupervisorControllingAcceptorBinding {
            registration: Some(registration),
            reply_endpoint,
            serialization_guard,
        }))
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn remove_temporary_controlling_acceptor_runtime(
        mut binding: TemporarySupervisorControllingAcceptorBinding,
    ) -> Result<(), MobError> {
        binding.cleanup().await
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn supervisor_bind_address(
        endpoint_config: &SupervisorBridgeEndpointConfig,
    ) -> Result<std::net::SocketAddr, MobError> {
        match endpoint_config.bind_address.as_deref() {
            Some(raw) => raw.parse::<std::net::SocketAddr>().map_err(|error| {
                MobError::WiringError(format!(
                    "invalid mob supervisor bridge bind_address '{raw}': {error}"
                ))
            }),
            None => Ok(std::net::SocketAddr::from(([127, 0, 0, 1], 0))),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn has_fixed_supervisor_bind_port(
        endpoint_config: &SupervisorBridgeEndpointConfig,
    ) -> Result<bool, MobError> {
        Ok(endpoint_config
            .bind_address
            .as_deref()
            .map(|raw| {
                raw.parse::<std::net::SocketAddr>()
                    .map(|socket| socket.port() != 0)
                    .map_err(|error| {
                        MobError::WiringError(format!(
                            "invalid mob supervisor bridge bind_address '{raw}': {error}"
                        ))
                    })
            })
            .transpose()?
            .unwrap_or(false))
    }

    /// Return the socket endpoint that can actually receive a reply on an
    /// ephemeral alternate-authority runtime.
    ///
    /// The normal supervisor descriptor deliberately keeps the durable bridge
    /// address. A non-fixed authority probe, however, owns a distinct listener
    /// with a kernel-assigned port. Its signed request must advertise that
    /// concrete listener as reply-only transport metadata; reusing the durable
    /// address routes the response to the live runtime under the wrong key.
    #[cfg(not(target_arch = "wasm32"))]
    fn ephemeral_probe_reply_endpoint(
        runtime: &meerkat_comms::CommsRuntime,
        endpoint_config: &SupervisorBridgeEndpointConfig,
    ) -> Result<PeerAddress, MobError> {
        let bound = runtime.bound_tcp_listener_address().ok_or_else(|| {
            MobError::Internal(
                "alternate supervisor probe has no bound TCP reply listener".to_string(),
            )
        })?;
        if bound.port() == 0 {
            return Err(MobError::Internal(
                "alternate supervisor probe reply listener retained port zero after start"
                    .to_string(),
            ));
        }

        let endpoint = if let Some(advertised) = endpoint_config.advertised_address.as_deref() {
            let advertised = PeerAddress::parse(advertised).map_err(|error| {
                MobError::WiringError(format!(
                    "invalid mob supervisor bridge advertised_address '{advertised}': {error}"
                ))
            })?;
            if advertised.transport() != PeerTransport::Tcp {
                return Err(MobError::WiringError(format!(
                    "mob supervisor bridge advertised_address must use tcp transport, got '{}'",
                    advertised.transport()
                )));
            }
            let (host, configured_port) =
                advertised.endpoint().rsplit_once(':').ok_or_else(|| {
                    MobError::WiringError(format!(
                        "mob supervisor bridge advertised_address '{advertised}' has no TCP port"
                    ))
                })?;
            if host.is_empty()
                || configured_port.parse::<u16>().is_err()
                || (host.contains(':') && !(host.starts_with('[') && host.ends_with(']')))
            {
                return Err(MobError::WiringError(format!(
                    "mob supervisor bridge advertised_address '{advertised}' has an invalid TCP endpoint"
                )));
            }
            format!("{host}:{}", bound.port())
        } else {
            bound.to_string()
        };
        Ok(PeerAddress::new(PeerTransport::Tcp, endpoint))
    }

    /// Resolve the callback endpoint for the currently installed live
    /// supervisor runtime.
    ///
    /// Unlike an ephemeral authority probe, the live runtime must preserve
    /// its configured advertised port: a fixed proxy/NAT mapping may differ
    /// from the local bind port. The receiver source-confines the advertised
    /// host to the kernel-observed request source IP, but deliberately retains
    /// this signed port.
    #[cfg(not(target_arch = "wasm32"))]
    fn live_runtime_reply_endpoint(
        &self,
        runtime: &meerkat_comms::CommsRuntime,
    ) -> Result<PeerAddress, MobError> {
        let address = self.supervisor_routable_address(runtime)?;
        let endpoint = PeerAddress::parse(&address).map_err(|error| {
            MobError::WiringError(format!(
                "invalid live mob supervisor reply endpoint '{address}': {error}"
            ))
        })?;
        if endpoint.transport() != PeerTransport::Tcp {
            return Err(MobError::WiringError(format!(
                "live mob supervisor reply endpoint must use tcp transport, got '{}'",
                endpoint.transport()
            )));
        }
        // Mirror the receiver's callback grammar so a locally accepted
        // endpoint cannot be silently discarded at authenticated ingress.
        let authority = endpoint.endpoint();
        let port = if let Some(bracketed) = authority.strip_prefix('[') {
            bracketed.find(']').and_then(|closing| {
                (closing != 0)
                    .then(|| bracketed[closing + 1..].strip_prefix(':'))
                    .flatten()
            })
        } else {
            authority
                .rsplit_once(':')
                .and_then(|(host, port)| (!host.is_empty() && !host.contains(':')).then_some(port))
        };
        let valid_port = port
            .filter(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|port| port.parse::<u16>().ok())
            .is_some_and(|port| port != 0);
        if !valid_port {
            return Err(MobError::WiringError(format!(
                "live mob supervisor reply endpoint '{endpoint}' has no valid nonzero TCP port"
            )));
        }
        Ok(endpoint)
    }

    #[cfg(target_arch = "wasm32")]
    fn has_fixed_supervisor_bind_port(
        _endpoint_config: &SupervisorBridgeEndpointConfig,
    ) -> Result<bool, MobError> {
        Ok(false)
    }

    /// Supervisor bridge runtimes intentionally issue semantic peer
    /// request/response commands outside a session surface. Give them an
    /// explicit ephemeral machine authority so comms never owns that lifecycle.
    fn prepare_supervisor_peer_request_response_authority(
        runtime: &meerkat_comms::CommsRuntime,
        participant_name: &str,
    ) -> Result<
        (
            Arc<meerkat_runtime::HandleDslAuthority>,
            meerkat_comms::PeerRequestResponseAuthority,
        ),
        MobError,
    > {
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "mob_supervisor_bridge::initialize",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to initialize supervisor bridge authority '{participant_name}': {error}"
            ))
        })?;
        let session_id =
            meerkat_runtime::meerkat_machine::dsl::SessionId::from(participant_name.to_string());
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: session_id.clone(),
            },
            "mob_supervisor_bridge::register",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to register supervisor bridge authority '{participant_name}': {error}"
            ))
        })?;
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::EnsureSessionWithExecutor {
                session_id,
            },
            "mob_supervisor_bridge::ensure_executor",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to attach supervisor bridge authority '{participant_name}': {error}"
            ))
        })?;

        meerkat_runtime::RuntimePeerCommsHandle::install_generated_on(
            Arc::clone(&dsl),
            runtime,
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to install supervisor bridge peer-comms authority '{participant_name}': {error}"
            ))
        })?;
        runtime.require_peer_comms_machine_authority();
        let request_response_authority = meerkat_comms::PeerRequestResponseAuthority::new(
            Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                Arc::clone(&dsl),
            )),
            Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(
                Arc::clone(&dsl),
            )),
        );
        Ok((dsl, request_response_authority))
    }

    pub(crate) async fn prepare_rotation(
        &self,
        authority: SupervisorAuthorityRecord,
        generated_authority: &SupervisorAuthorityBridgeAuthority,
    ) -> Result<PreparedSupervisorBridgeRotation, MobError> {
        generated_authority.verify_record(&authority)?;
        // For an ephemeral bind port (the default) the replacement runtime is
        // built ahead of commit, preserving the two-phase "build-can-fail /
        // commit-is-atomic" rotation contract. A fixed TCP bind port cannot be
        // double-bound while the live runtime still holds it, so its rebuild is
        // deferred to commit, which stops the old listener first.
        let prebuilt = if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            None
        } else {
            Some(self.prepare_live_runtime(&authority).await?)
        };
        Ok(PreparedSupervisorBridgeRotation {
            authority,
            prebuilt,
        })
    }

    pub(crate) async fn commit_prepared_rotation(
        self: &Arc<Self>,
        prepared: PreparedSupervisorBridgeRotation,
    ) -> Result<(), MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        if prepared.prebuilt.is_none() {
            let bridge = Arc::clone(self);
            return tokio::spawn(async move {
                let _authority_guard = bridge.authority_gate.write().await;
                bridge.commit_prepared_rotation_locked(prepared, true).await
            })
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "fixed-port supervisor rotation worker terminated unexpectedly: {error}"
                ))
            })?;
        }

        let _authority_guard = self.authority_gate.write().await;
        self.commit_prepared_rotation_locked(prepared, true).await
    }

    /// Commit one prepared rotation while `authority_gate.write()` is held.
    async fn commit_prepared_rotation_locked(
        &self,
        prepared: PreparedSupervisorBridgeRotation,
        clear_buffered_candidates: bool,
    ) -> Result<(), MobError> {
        let PreparedSupervisorBridgeRotation {
            authority,
            prebuilt,
        } = prepared;
        if self.live_transport_uses_authority(&authority)
            && self.live_transport_is_healthy_with_gate_held().await?
        {
            return self
                .update_live_authority_metadata_with_gate_held(authority, clear_buffered_candidates)
                .await;
        }
        match prebuilt {
            Some(prepared_runtime) => {
                self.install_prepared_runtime_with_gate_held(
                    authority,
                    prepared_runtime,
                    clear_buffered_candidates,
                )
                .await
            }
            None => {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.rebuild_fixed_runtime_with_gate_held(authority, clear_buffered_candidates)
                        .await
                }
                #[cfg(target_arch = "wasm32")]
                {
                    Err(MobError::Internal(
                        "WASM supervisor rotation had no prepared runtime".to_string(),
                    ))
                }
            }
        }
    }

    /// Update non-transport authority metadata without replacing a live route.
    async fn update_live_authority_metadata_with_gate_held(
        &self,
        authority: SupervisorAuthorityRecord,
        clear_buffered_candidates: bool,
    ) -> Result<(), MobError> {
        let mut buffered = self.buffered_candidates.lock().await;
        *self
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        if clear_buffered_candidates {
            buffered.clear();
        }
        self.intake_notify.notify_waiters();
        Ok(())
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    fn inject_install_failure_after_listener_bind(participant_name: &str) -> bool {
        let mut target = FAIL_NEXT_SUPERVISOR_INSTALL_AFTER_LISTENER_BIND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if target.as_deref() != Some(participant_name) {
            return false;
        }
        target.take();
        true
    }

    /// Publish one dormant runtime and atomically replace the live identity.
    async fn install_prepared_runtime_with_gate_held(
        &self,
        authority: SupervisorAuthorityRecord,
        prepared_runtime: PreparedSupervisorRuntime,
        clear_buffered_candidates: bool,
    ) -> Result<(), MobError> {
        let _trust_reconcile_guard = self.trust_reconcile_gate.lock().await;
        let live_dsl = self.dsl.read().await.clone();
        let direct_peer_endpoints =
            live_dsl.with_state_lock(|state| state.direct_peer_endpoints.clone());
        if let Err(error) =
            Self::replay_direct_peer_projection(&prepared_runtime, direct_peer_endpoints).await
        {
            prepared_runtime.stop_listeners().await;
            return Err(error);
        }

        #[cfg(all(test, not(target_arch = "wasm32")))]
        if Self::inject_install_failure_after_listener_bind(&self.participant_name) {
            prepared_runtime.stop_listeners().await;
            return Err(MobError::Internal(
                "fault-injected supervisor install failure after listener bind".to_string(),
            ));
        }

        let mut runtime_guard = self.runtime.write().await;

        // Every guard protecting the published identity tuple is acquired
        // before the inproc registry changes. All async/fallible acceptor work
        // also completes first; after publish_replacing succeeds, commit is a
        // sequence of infallible assignments and notifications only.
        let mut dsl_guard = self.dsl.write().await;
        let mut buffered_guard = self.buffered_candidates.lock().await;

        #[cfg(not(target_arch = "wasm32"))]
        let mut binding_guard = self.controlling_acceptor.lock().await;
        #[cfg(not(target_arch = "wasm32"))]
        let prepared_binding = if let Some(current_binding) = binding_guard.as_ref() {
            let registration =
                match Self::acceptor_registration_for_runtime(prepared_runtime.runtime()) {
                    Ok(registration) => registration,
                    Err(error) => {
                        drop(binding_guard);
                        drop(buffered_guard);
                        drop(dsl_guard);
                        drop(runtime_guard);
                        prepared_runtime.stop_listeners().await;
                        return Err(error);
                    }
                };
            let config = current_binding.config.clone();
            let current_lease = match current_binding.lease() {
                Ok(lease) => lease,
                Err(error) => {
                    drop(binding_guard);
                    drop(buffered_guard);
                    drop(dsl_guard);
                    drop(runtime_guard);
                    prepared_runtime.stop_listeners().await;
                    return Err(error);
                }
            };
            let replacement = match config
                .prepare_replacement(current_lease, self.participant_name.clone(), registration)
                .await
            {
                Ok(replacement) => replacement,
                Err(error) => {
                    drop(binding_guard);
                    drop(buffered_guard);
                    drop(dsl_guard);
                    drop(runtime_guard);
                    prepared_runtime.stop_listeners().await;
                    return Err(error);
                }
            };
            let reply_endpoint = match Self::validated_controlling_reply_endpoint(
                replacement.advertised_address(),
            ) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    drop(replacement);
                    drop(binding_guard);
                    drop(buffered_guard);
                    drop(dsl_guard);
                    drop(runtime_guard);
                    prepared_runtime.stop_listeners().await;
                    return Err(error);
                }
            };
            Some((replacement, reply_endpoint, config))
        } else {
            None
        };

        let (runtime, dsl) =
            match prepared_runtime.publish_replacing_recoverable(runtime_guard.as_ref()) {
                Ok(published) => published,
                Err((prepared_runtime, publish_error)) => {
                    #[cfg(not(target_arch = "wasm32"))]
                    drop(prepared_binding);
                    #[cfg(not(target_arch = "wasm32"))]
                    drop(binding_guard);
                    drop(buffered_guard);
                    drop(dsl_guard);
                    drop(runtime_guard);
                    prepared_runtime.stop_listeners().await;
                    return Err(publish_error);
                }
            };

        #[cfg(not(target_arch = "wasm32"))]
        let retired_binding = prepared_binding.and_then(|(replacement, reply_endpoint, config)| {
            let replacement_lease = replacement.commit();
            let replacement = SupervisorControllingAcceptorBinding::new(config, replacement_lease);
            let retired = binding_guard.replace(replacement);
            *self
                .controlling_reply_endpoint
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reply_endpoint);
            retired
        });
        #[cfg(not(target_arch = "wasm32"))]
        let retired_binding = retired_binding.map(|mut retired| {
            retired.disarm_after_atomic_replacement();
            retired
        });
        *self
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        *runtime_guard = runtime;
        *dsl_guard = dsl;
        if clear_buffered_candidates {
            buffered_guard.clear();
        }
        #[cfg(not(target_arch = "wasm32"))]
        drop(retired_binding);
        // The upcall responder may still be parked on the previous runtime's
        // inbox notify. Wake the shared intake only after the replacement
        // runtime/authority/DSL tuple is fully published so its next loop
        // resolves and drains the new inbox.
        self.intake_notify.notify_waiters();
        Ok(())
    }

    /// Rebuild one fixed TCP endpoint, restoring the captured current runtime
    /// if any post-stop step fails.
    #[cfg(not(target_arch = "wasm32"))]
    async fn rebuild_fixed_runtime_with_gate_held(
        &self,
        authority: SupervisorAuthorityRecord,
        clear_buffered_candidates: bool,
    ) -> Result<(), MobError> {
        let retained_authority = self.authority_with_gate_held();
        self.runtime_with_gate_held()
            .await
            .stop_listeners_for_rebind()
            .await;

        // This hook models durable activation. Temporary alternate-authority
        // probes must not consume it.
        #[cfg(test)]
        let replacement_result = if clear_buffered_candidates
            && FAIL_NEXT_FIXED_PORT_ROTATION_REBUILD
                .swap(false, std::sync::atomic::Ordering::Relaxed)
        {
            Err(MobError::Internal(
                "fault-injected fixed-port supervisor bridge rebuild failure".to_string(),
            ))
        } else {
            match self.prepare_live_runtime(&authority).await {
                Ok(prepared) => {
                    self.install_prepared_runtime_with_gate_held(
                        authority,
                        prepared,
                        clear_buffered_candidates,
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        };
        #[cfg(not(test))]
        let replacement_result = match self.prepare_live_runtime(&authority).await {
            Ok(prepared) => {
                self.install_prepared_runtime_with_gate_held(
                    authority,
                    prepared,
                    clear_buffered_candidates,
                )
                .await
            }
            Err(error) => Err(error),
        };

        let Err(replacement_error) = replacement_result else {
            return Ok(());
        };
        let recovery_result = match self.prepare_live_runtime(&retained_authority).await {
            Ok(prepared) => {
                self.install_prepared_runtime_with_gate_held(
                    retained_authority.clone(),
                    prepared,
                    false,
                )
                .await
            }
            Err(error) => Err(error),
        };
        match recovery_result {
            Ok(()) => Err(replacement_error),
            Err(recovery_error) => Err(MobError::Internal(format!(
                "fixed-port supervisor runtime replacement failed: {replacement_error}; additionally failed to restore peer={} epoch={}: {recovery_error}",
                retained_authority.public_peer_id, retained_authority.epoch
            ))),
        }
    }

    pub(crate) async fn rotate(
        self: &Arc<Self>,
        authority: SupervisorAuthorityRecord,
    ) -> Result<(), MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            let bridge = Arc::clone(self);
            return tokio::spawn(async move {
                let _authority_guard = bridge.authority_gate.write().await;
                bridge
                    .replace_runtime_authority_locked(authority, true)
                    .await
            })
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "fixed-port supervisor rotation worker terminated unexpectedly: {error}"
                ))
            })?;
        }

        let prepared = self.prepare_live_runtime(&authority).await?;
        let _authority_guard = self.authority_gate.write().await;
        self.commit_prepared_rotation_locked(
            PreparedSupervisorBridgeRotation {
                authority,
                prebuilt: Some(prepared),
            },
            true,
        )
        .await
    }

    pub(crate) async fn shutdown(&self) {
        let _authority_guard = self.authority_gate.write().await;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut binding = self.controlling_acceptor.lock().await;
            if let Some(installed) = binding.as_mut() {
                if let Err(error) = installed.cleanup().await {
                    tracing::warn!(%error, "failed to remove mob supervisor from process host ingress at shutdown");
                } else {
                    *binding = None;
                    *self
                        .controlling_reply_endpoint
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                }
            }
            let runtime = self.runtime_with_gate_held().await;
            runtime.stop_listeners_for_rebind().await;
        }
    }

    /// Rebuild the supervisor runtime under a (possibly new) authority record,
    /// swapping authority, runtime, and DSL together under the runtime write
    /// lock. For a fixed TCP bind port the live listener is stopped first so the
    /// rebuilt runtime can re-bind it. Used by `send_bridge_command_as_authority`
    /// to temporarily adopt a rotation-target authority and restore afterward.
    async fn replace_runtime_authority_locked(
        &self,
        authority: SupervisorAuthorityRecord,
        clear_buffered_candidates: bool,
    ) -> Result<(), MobError> {
        if self.live_transport_uses_authority(&authority)
            && self.live_transport_is_healthy_with_gate_held().await?
        {
            return self
                .update_live_authority_metadata_with_gate_held(authority, clear_buffered_candidates)
                .await;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            return self
                .rebuild_fixed_runtime_with_gate_held(authority, clear_buffered_candidates)
                .await;
        }
        let prepared = self.prepare_live_runtime(&authority).await?;
        self.install_prepared_runtime_with_gate_held(authority, prepared, clear_buffered_candidates)
            .await
    }

    /// Read the durable live authority without crossing an in-flight
    /// fixed-port alternate-authority swap.
    pub(crate) async fn authority(&self) -> SupervisorAuthorityRecord {
        let _authority_guard = self.authority_gate.read().await;
        self.authority_with_gate_held()
    }

    /// Read `authority` while the caller holds either side of
    /// `authority_gate`.
    fn authority_with_gate_held(&self) -> SupervisorAuthorityRecord {
        self.authority
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether `authority` names the exact signing transport installed in the
    /// live runtime. `pending_rotation` is durable retry bookkeeping for the
    /// mob actor; it does not alter the key, epoch, or protocol installed in
    /// this bridge. Treating that metadata as transport identity would make a
    /// current-authority recovery call try to register a colliding temporary
    /// shared-ingress owner for its own key.
    fn live_transport_uses_authority(&self, authority: &SupervisorAuthorityRecord) -> bool {
        let live = self.authority_with_gate_held();
        live.secret_key == authority.secret_key && live.public_peer_id == authority.public_peer_id
    }

    /// Metadata-only authority refresh is valid only while the installed
    /// transport is still live. In particular, a failed fixed-port rebuild may
    /// retain the old authority record after stopping its listener; a later
    /// same-authority recovery must rebind rather than report false success.
    async fn live_transport_is_healthy_with_gate_held(&self) -> Result<bool, MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
                return Ok(self
                    .runtime_with_gate_held()
                    .await
                    .bound_tcp_listener_address()
                    .is_some());
            }
        }
        Ok(true)
    }

    /// Read the live runtime without crossing an in-flight fixed-port
    /// alternate-authority swap.
    pub(crate) async fn runtime(&self) -> Arc<meerkat_comms::CommsRuntime> {
        let _authority_guard = self.authority_gate.read().await;
        self.runtime_with_gate_held().await
    }

    /// Read `runtime` while the caller holds either side of `authority_gate`.
    async fn runtime_with_gate_held(&self) -> Arc<meerkat_comms::CommsRuntime> {
        self.runtime.read().await.clone()
    }

    pub(crate) async fn runtime_core(&self) -> Arc<dyn CoreCommsRuntime> {
        let _authority_guard = self.authority_gate.read().await;
        self.runtime_with_gate_held().await as Arc<dyn CoreCommsRuntime>
    }

    async fn apply_bridge_trust(
        trust_reconcile_gate: &Mutex<()>,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        dsl: &Arc<meerkat_runtime::HandleDslAuthority>,
        recipient: TrustedPeerDescriptor,
    ) -> Result<RecipientTrustInstall, MobError> {
        let _trust_reconcile_guard = trust_reconcile_gate.lock().await;
        let local_endpoint = Self::local_endpoint_for_runtime(runtime.as_ref())?;
        dsl.apply_input(
            mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
                endpoint: local_endpoint,
            },
            "mob_supervisor_bridge::publish_local_endpoint",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "supervisor bridge DSL rejected local endpoint publication: {error}"
            ))
        })?;
        let endpoint = mm_dsl::PeerEndpoint::from(&recipient);
        let reconciled_endpoint = endpoint.clone();
        let transition = dsl
            .apply_input_with_transition(
                mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint { endpoint },
                "mob_supervisor_bridge::trust_recipient",
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor bridge DSL rejected recipient trust projection: {error}"
                ))
            })?;
        let obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = match obligations.as_slice() {
            [obligation] => obligation.clone(),
            [] => {
                return Err(MobError::Internal(
                    "supervisor bridge trust projection emitted no reconcile request".to_string(),
                ));
            }
            _ => {
                return Err(MobError::Internal(
                    "supervisor bridge trust projection emitted multiple reconcile requests"
                        .to_string(),
                ));
            }
        };
        #[cfg(test)]
        if let Ok(mut target) = FAIL_NEXT_BRIDGE_TRUST_ADD_RECONCILE_FOR_PEER.lock()
            && target.as_deref() == Some(recipient.peer_id.to_string().as_str())
        {
            target.take();
            return Err(MobError::Internal(
                "fault-injected supervisor bridge trust-add reconciliation failure".to_string(),
            ));
        }
        let comms_runtime: Arc<dyn CoreCommsRuntime> = runtime.clone();
        let reconciler =
            meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(comms_runtime);
        reconciler
            .reconcile(&obligation)
            .await
            .map(|report| {
                if report.added.contains(&reconciled_endpoint) {
                    RecipientTrustInstall::NewlyInstalled
                } else {
                    RecipientTrustInstall::AlreadyTrusted
                }
            })
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor bridge generated trust reconciliation failed: {error}"
                ))
            })
    }

    /// Fail-closed rollback mirror of [`Self::apply_bridge_trust`]. Drives the
    /// generated `RemoveDirectPeerEndpoint` transition and reconciles the
    /// resulting removal obligation against the live comms runtime so the DSL
    /// authority and the comms trust set never diverge. The generated machine
    /// has an explicit repair arm when the declarative endpoint is already
    /// absent; always applying the input is what lets a retry heal the window
    /// where the prior DSL mutation committed but live reconciliation failed.
    async fn remove_bridge_trust(
        trust_reconcile_gate: &Mutex<()>,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        dsl: &Arc<meerkat_runtime::HandleDslAuthority>,
        recipient: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let _trust_reconcile_guard = trust_reconcile_gate.lock().await;
        let endpoint = mm_dsl::PeerEndpoint::from(&recipient);
        let transition = dsl
            .apply_input_with_transition(
                mm_dsl::MeerkatMachineInput::RemoveDirectPeerEndpoint { endpoint },
                "mob_supervisor_bridge::untrust_recipient",
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor bridge DSL rejected recipient trust removal projection: {error}"
                ))
            })?;
        let obligations =
            meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                &transition,
                dsl.peer_projection_freshness_authority(),
            );
        let obligation = match obligations.as_slice() {
            [obligation] => obligation.clone(),
            [] => {
                return Err(MobError::Internal(
                    "supervisor bridge trust removal projection emitted no reconcile request"
                        .to_string(),
                ));
            }
            _ => {
                return Err(MobError::Internal(
                    "supervisor bridge trust removal projection emitted multiple reconcile requests"
                        .to_string(),
                ));
            }
        };
        #[cfg(test)]
        if let Ok(mut target) = FAIL_NEXT_BRIDGE_TRUST_REMOVE_RECONCILE_FOR_PEER.lock()
            && target.as_deref() == Some(recipient.peer_id.to_string().as_str())
        {
            target.take();
            return Err(MobError::Internal(
                "fault-injected supervisor bridge trust-remove reconciliation failure".to_string(),
            ));
        }
        let comms_runtime: Arc<dyn CoreCommsRuntime> = runtime.clone();
        let reconciler =
            meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(comms_runtime);
        reconciler
            .reconcile(&obligation)
            .await
            .map(|_report| ())
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor bridge generated trust removal reconciliation failed: {error}"
                ))
            })
    }

    /// Recreate the live machine-owned direct-peer projection on a dormant
    /// rotation candidate before it is published.
    async fn replay_direct_peer_projection(
        prepared: &PreparedSupervisorRuntime,
        direct_peer_endpoints: std::collections::BTreeSet<mm_dsl::PeerEndpoint>,
    ) -> Result<(), MobError> {
        if direct_peer_endpoints.is_empty() {
            return Ok(());
        }
        let local_endpoint = Self::local_endpoint_for_runtime(prepared.runtime())?;
        prepared
            .dsl
            .apply_input(
                mm_dsl::MeerkatMachineInput::PublishLocalEndpoint {
                    endpoint: local_endpoint,
                },
                "mob_supervisor_bridge::rotation_publish_local_endpoint",
            )
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor rotation candidate rejected local endpoint publication: {error}"
                ))
            })?;

        for endpoint in direct_peer_endpoints {
            let transition = prepared
                .dsl
                .apply_input_with_transition(
                    mm_dsl::MeerkatMachineInput::AddDirectPeerEndpoint { endpoint },
                    "mob_supervisor_bridge::rotation_replay_direct_peer",
                )
                .map_err(|error| {
                    MobError::Internal(format!(
                        "supervisor rotation candidate rejected direct-peer replay: {error}"
                    ))
                })?;
            let obligations =
                meerkat_runtime::protocol_comms_trust_reconcile::extract_obligations_with_freshness(
                    &transition,
                    prepared.dsl.peer_projection_freshness_authority(),
                );
            let obligation = match obligations.as_slice() {
                [obligation] => obligation,
                [] => {
                    return Err(MobError::Internal(
                        "supervisor rotation direct-peer replay emitted no reconcile request"
                            .to_string(),
                    ));
                }
                _ => {
                    return Err(MobError::Internal(
                        "supervisor rotation direct-peer replay emitted multiple reconcile requests"
                            .to_string(),
                    ));
                }
            };
            meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::reconcile_runtime(
                prepared.runtime(),
                obligation,
            )
            .await
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor rotation direct-peer reconciliation failed: {error}"
                ))
            })?;
        }
        Ok(())
    }

    fn local_endpoint_for_runtime(
        runtime: &dyn CoreCommsRuntime,
    ) -> Result<mm_dsl::PeerEndpoint, MobError> {
        let peer_id = runtime.peer_id().ok_or_else(|| {
            MobError::Internal("supervisor bridge runtime peer_id unavailable".to_string())
        })?;
        let name = runtime.comms_name().ok_or_else(|| {
            MobError::Internal("supervisor bridge runtime comms_name unavailable".to_string())
        })?;
        let address = runtime.advertised_address().ok_or_else(|| {
            MobError::Internal(
                "supervisor bridge runtime advertised_address unavailable".to_string(),
            )
        })?;
        let pubkey = runtime.public_key_bytes().ok_or_else(|| {
            MobError::Internal("supervisor bridge runtime public_key_bytes unavailable".to_string())
        })?;
        let descriptor =
            TrustedPeerDescriptor::unsigned_with_pubkey(name, peer_id.to_string(), pubkey, address)
                .map_err(|error| {
                    MobError::WiringError(format!(
                        "supervisor bridge runtime local endpoint descriptor invalid: {error}"
                    ))
                })?;
        Ok(mm_dsl::PeerEndpoint::from(&descriptor))
    }

    pub(crate) async fn supervisor_spec(&self) -> Result<TrustedPeerDescriptor, MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let authority = self.authority_with_gate_held();
        let address = self.resolve_self_address(SupervisorReachability::InProcess)?;
        self.supervisor_spec_with_address(&authority, address)
    }

    pub(crate) async fn routable_supervisor_spec(&self) -> Result<TrustedPeerDescriptor, MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let authority = self.authority_with_gate_held();
        let runtime = self.runtime_with_gate_held().await;
        let address =
            self.resolve_self_address(SupervisorReachability::Routable(runtime.as_ref()))?;
        self.supervisor_spec_with_address(&authority, address)
    }

    pub(crate) async fn supervisor_spec_for_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let authority = self.authority_with_gate_held();
        self.supervisor_spec_for_authority_and_recipient_with_gate_held(&authority, recipient)
            .await
    }

    pub(crate) async fn supervisor_spec_for_authority_and_recipient(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let _authority_guard = self.authority_gate.read().await;
        self.supervisor_spec_for_authority_and_recipient_with_gate_held(authority, recipient)
            .await
    }

    /// Resolve an authority-specific recipient view while the caller holds
    /// either side of `authority_gate`.
    async fn supervisor_spec_for_authority_and_recipient_with_gate_held(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let runtime = self.runtime_with_gate_held().await;
        let reachability = SupervisorReachability::for_recipient_transport(
            recipient.address.transport(),
            runtime.as_ref(),
        );
        let address = self.resolve_self_address(reachability)?;
        self.supervisor_spec_with_address(authority, address)
    }

    /// Resolve the single typed self-address the bridge advertises for the
    /// given reachability domain. The bridge identity (peer id, public key,
    /// participant name) is invariant across recipients; only the reachable
    /// address differs, and it differs *only* by reachability domain (in-process
    /// loopback vs. routable network), never by who is asking. In-process peers
    /// reach the bridge through the inproc registry by participant name; network
    /// peers reach it through the routable bind/advertised address, which fails
    /// closed on an unspecified interface.
    fn resolve_self_address(
        &self,
        reachability: SupervisorReachability<'_>,
    ) -> Result<String, MobError> {
        match reachability {
            SupervisorReachability::InProcess => Ok(format!("inproc://{}", self.participant_name)),
            SupervisorReachability::Routable(runtime) => self.supervisor_routable_address(runtime),
        }
    }

    fn supervisor_routable_address(
        &self,
        runtime: &meerkat_comms::CommsRuntime,
    ) -> Result<String, MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(endpoint) = self
            .controlling_reply_endpoint
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            return Ok(endpoint.to_string());
        }
        if let Some(address) = self.endpoint_config.advertised_address.as_deref() {
            let parsed = PeerAddress::parse(address).map_err(|error| {
                MobError::WiringError(format!(
                    "invalid mob supervisor bridge advertised_address '{address}': {error}"
                ))
            })?;
            if parsed.transport() != PeerTransport::Tcp {
                return Err(MobError::WiringError(format!(
                    "mob supervisor bridge advertised_address must use tcp transport, got '{}'",
                    parsed.transport()
                )));
            }
            return Ok(address.to_string());
        }
        let address = CoreCommsRuntime::advertised_address(runtime)
            .unwrap_or_else(|| format!("inproc://{}", self.participant_name));
        if let Ok(parsed) = PeerAddress::parse(&address)
            && parsed.transport() == PeerTransport::Tcp
            && let Ok(socket) = parsed.endpoint().parse::<std::net::SocketAddr>()
            && socket.ip().is_unspecified()
        {
            return Err(MobError::WiringError(
                "mob supervisor bridge bind_address uses an unspecified interface; \
                 backend.external.supervisor_bridge.advertised_address is required"
                    .to_string(),
            ));
        }
        Ok(address)
    }

    fn supervisor_spec_with_address(
        &self,
        authority: &SupervisorAuthorityRecord,
        address: String,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        let public_key = authority.keypair().public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            self.participant_name.clone(),
            authority.public_peer_id.clone(),
            *public_key.as_bytes(),
            address,
        )
        .map_err(|error| MobError::WiringError(format!("invalid supervisor spec: {error}")))
    }

    /// Send a typed bridge command and return the raw JSON response.
    pub(crate) async fn send_bridge_command(
        &self,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        self.request_json(
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
    }

    /// Send a bridge command while preserving whether failure happened before
    /// or after the request obtained a transport send receipt. BindMember
    /// callers use this boundary to avoid certifying rollback when a remote
    /// bind may have committed but its terminal response was lost.
    pub(crate) async fn send_bridge_command_classified(
        &self,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        self.request_json_with_receipt_classified(
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
        .map(|(value, _envelope_id)| value)
    }

    /// Send a typed bridge command and return the raw JSON response together
    /// with the request envelope id (DEC-P6E-19, cross-lane seam name).
    ///
    /// For `DeliverMemberInput`, the member runtime stamps this envelope id
    /// as the accepted input's `InteractionId`, so the member's terminal
    /// `InteractionComplete` / `InteractionCallbackPending` /
    /// `InteractionFailed` events carry it — the correlation fact remote
    /// completion waiters key on.
    pub(crate) async fn send_bridge_command_with_receipt(
        &self,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<(serde_json::Value, uuid::Uuid), MobError> {
        self.request_json_with_receipt(
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
    }

    /// Submit a closed supervisor delivery as a genuine one-way peer message.
    ///
    /// The send receipt proves only that comms accepted/delivered the envelope;
    /// it is never interpreted as rotation completion. Durable completion is
    /// observed separately through `ObserveSupervisorRotation` as the target
    /// authority.
    pub(crate) async fn send_supervisor_delivery(
        &self,
        recipient: &TrustedPeerDescriptor,
        delivery: &super::bridge_protocol::BridgeSupervisorDelivery,
    ) -> Result<(), MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let runtime = self.runtime_with_gate_held().await;
        Self::send_supervisor_delivery_with_runtime(&runtime, recipient, delivery).await
    }

    async fn send_supervisor_delivery_with_runtime(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        delivery: &super::bridge_protocol::BridgeSupervisorDelivery,
    ) -> Result<(), MobError> {
        let body = serde_json::to_string(delivery).map_err(|error| {
            MobError::Internal(format!("serialize supervisor delivery: {error}"))
        })?;
        let receipt = runtime
            .send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone()),
                body,
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            })
            .await?;
        match receipt {
            SendReceipt::PeerMessageSent { .. } => Ok(()),
            _ => Err(MobError::Internal(
                "unexpected receipt for one-way supervisor delivery".to_string(),
            )),
        }
    }

    async fn send_supervisor_delivery_with_live_runtime(
        &self,
        recipient: &TrustedPeerDescriptor,
        delivery: &super::bridge_protocol::BridgeSupervisorDelivery,
    ) -> Result<(), MobError> {
        let runtime = self.runtime_with_gate_held().await;
        let dsl = self.dsl.read().await.clone();
        let _install = Self::apply_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await?;
        Self::send_supervisor_delivery_with_runtime(&runtime, recipient, delivery).await
    }

    async fn send_bridge_command_with_live_runtime_classified(
        &self,
        _authority_guard: Option<tokio::sync::RwLockReadGuard<'_, ()>>,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        let runtime = self.runtime_with_gate_held().await;
        let dsl = self.dsl.read().await.clone();
        Self::apply_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
        .map_err(BridgeRequestFailure::BeforeSend)?;
        let envelope_id = self
            .send_supervisor_request(
                &runtime,
                recipient,
                super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                command,
                SupervisorRequestReplyEndpoint::Live,
            )
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
        self.wait_for_response(&runtime, envelope_id, recipient.peer_id, timeout)
            .await
            .map_err(BridgeRequestFailure::AfterSend)
    }

    /// Send a one-way delivery under a specific durable supervisor authority.
    /// Used only to adopt an operation id onto a legacy member that already
    /// committed the exact target authority before operation-correlated
    /// rotation existed.
    pub(crate) async fn send_supervisor_delivery_as_authority(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        delivery: &super::bridge_protocol::BridgeSupervisorDelivery,
    ) -> Result<(), MobError> {
        let authority_guard = self.authority_gate.read().await;
        if self.live_transport_uses_authority(authority) {
            return self
                .send_supervisor_delivery_with_live_runtime(recipient, delivery)
                .await;
        }
        drop(authority_guard);
        // Historical authorities reuse their durable key. Serialize their
        // short-lived runtimes so two concurrent probes cannot replace the
        // same pubkey route. One-way delivery needs no callback listener or
        // process-acceptor lease, even when the durable bridge uses a fixed
        // TCP port.
        let _alternate_probe_guard = Arc::clone(&self.alternate_authority_probe_gate)
            .lock_owned()
            .await;
        let _authority_guard = self.authority_gate.read().await;
        if self.live_transport_uses_authority(authority) {
            return self
                .send_supervisor_delivery_with_live_runtime(recipient, delivery)
                .await;
        }
        let probe_participant_name = format!(
            "{}/pending-supervisor-delivery/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let (runtime, dsl) = self
            .build_probe_runtime(&probe_participant_name, authority, false)
            .await?;
        let _install = Self::apply_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await?;
        Self::send_supervisor_delivery_with_runtime(&runtime, recipient, delivery).await
    }

    pub(crate) async fn send_bridge_command_as_authority(
        self: &Arc<Self>,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        self.send_bridge_command_as_authority_classified(authority, recipient, command, timeout)
            .await
            .map_err(BridgeRequestFailure::into_error)
    }

    /// Run the private fixed-port authority swap as an owned task.
    ///
    /// A fixed proxy/NAT mapping requires the request callback to reuse the
    /// durable listener's exact port. The temporary signing identity therefore
    /// replaces the live runtime for the bounded round-trip. Keeping the whole
    /// switch/request/restore sequence in one owned task makes dropping the
    /// caller detach the waiter instead of cancelling restoration halfway
    /// through and publishing the temporary identity as durable bridge truth.
    #[cfg(not(target_arch = "wasm32"))]
    async fn send_fixed_port_bridge_command_as_authority_owned(
        self: Arc<Self>,
        authority: SupervisorAuthorityRecord,
        recipient: TrustedPeerDescriptor,
        command: super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        let _authority_guard = self.authority_gate.write().await;
        // A concurrent durable rotation may have installed the requested
        // authority while this worker was queued.
        if self.live_transport_uses_authority(&authority) {
            return self
                .send_bridge_command_with_live_runtime_classified(
                    None, &recipient, &command, timeout,
                )
                .await;
        }

        let previous_authority = self.authority_with_gate_held();
        if let Err(switch_error) = self
            .replace_runtime_authority_locked(authority.clone(), false)
            .await
        {
            // The fixed rebuild helper has already restored the captured
            // current runtime, or included that recovery failure in this error.
            tracing::warn!(
                error = %switch_error,
                requested_peer_id = %authority.public_peer_id,
                restored_peer_id = %previous_authority.public_peer_id,
                "fixed-port alternate-authority switch failed"
            );
            return Err(BridgeRequestFailure::BeforeSend(switch_error));
        }

        let send_result = async {
            let runtime = self.runtime_with_gate_held().await;
            let dsl = self.dsl.read().await.clone();
            Self::apply_bridge_trust(
                &self.trust_reconcile_gate,
                &runtime,
                &dsl,
                recipient.clone(),
            )
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
            self.request_json_with_runtime_classified(
                &runtime,
                &recipient,
                super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                &command,
                SupervisorRequestReplyEndpoint::Live,
                timeout,
            )
            .await
        }
        .await;

        if let Err(primary_restore_error) = self
            .replace_runtime_authority_locked(previous_authority.clone(), false)
            .await
        {
            // The failed attempt restored the temporary identity. Retry the
            // same explicit fixed-port rebuild once for the durable identity.
            let fallback_result = self
                .rebuild_fixed_runtime_with_gate_held(previous_authority.clone(), false)
                .await;
            let fallback_recovered = fallback_result.is_ok();
            let restore_error = match fallback_result {
                Ok(()) => MobError::Internal(format!(
                    "restoring the previous fixed-port supervisor runtime failed: {primary_restore_error}; fallback rebuild restored the previous authority"
                )),
                Err(fallback_error) => MobError::Internal(format!(
                    "restoring the previous fixed-port supervisor runtime failed: {primary_restore_error}; fallback rebuild also failed: {fallback_error}"
                )),
            };
            // This task deliberately outlives a cancelled caller, so report the
            // restoration failure here as well as through its typed result.
            tracing::error!(
                error = %restore_error,
                requested_peer_id = %authority.public_peer_id,
                previous_peer_id = %previous_authority.public_peer_id,
                fallback_recovered,
                "fixed-port alternate-authority request could not restore its original runtime normally"
            );
            return match send_result {
                Err(BridgeRequestFailure::BeforeSend(_)) => {
                    Err(BridgeRequestFailure::BeforeSend(restore_error))
                }
                Ok(_) | Err(BridgeRequestFailure::AfterSend(_)) => {
                    Err(BridgeRequestFailure::AfterSend(restore_error))
                }
            };
        }

        send_result
    }

    /// Alternate-authority request preserving the transport send boundary.
    /// Supervisor rotation uses this for host rebind so a timeout after the
    /// request receipt cannot be mistaken for a certified pre-send failure.
    pub(crate) async fn send_bridge_command_as_authority_classified(
        self: &Arc<Self>,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        #[cfg(not(target_arch = "wasm32"))]
        if recipient.address.transport() == PeerTransport::Uds {
            return Err(BridgeRequestFailure::BeforeSend(MobError::WiringError(
                "supervisor request/response over UDS is unsupported: authenticated UDS ingress cannot authorize the required TCP reply endpoint"
                    .to_string(),
            )));
        }
        let authority_guard = self.authority_gate.read().await;
        if self.live_transport_uses_authority(authority) {
            return self
                .send_bridge_command_with_live_runtime_classified(
                    Some(authority_guard),
                    recipient,
                    command,
                    timeout,
                )
                .await;
        }
        drop(authority_guard);

        #[cfg(not(target_arch = "wasm32"))]
        if recipient.address.transport() != PeerTransport::Inproc
            && Self::has_fixed_supervisor_bind_port(&self.endpoint_config)
                .map_err(BridgeRequestFailure::BeforeSend)?
        {
            let worker_bridge = Arc::clone(self);
            let worker_authority = authority.clone();
            let worker_recipient = recipient.clone();
            let worker_command = command.clone();
            let worker = tokio::spawn(async move {
                worker_bridge
                    .send_fixed_port_bridge_command_as_authority_owned(
                        worker_authority,
                        worker_recipient,
                        worker_command,
                        timeout,
                    )
                    .await
            });
            return worker.await.unwrap_or_else(|join_error| {
                // A panic can happen on either side of the send boundary. Never
                // certify it as pre-send when the worker cannot report its own
                // transport receipt.
                Err(BridgeRequestFailure::AfterSend(MobError::Internal(
                    format!(
                        "fixed-port alternate-authority request worker terminated unexpectedly: {join_error}"
                    ),
                )))
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let mut alternate_probe_guard = Some(
            Arc::clone(&self.alternate_authority_probe_gate)
                .lock_owned()
                .await,
        );
        #[cfg(target_arch = "wasm32")]
        let _alternate_probe_guard = Arc::clone(&self.alternate_authority_probe_gate)
            .lock_owned()
            .await;
        let _authority_guard = self.authority_gate.read().await;
        if self.live_transport_uses_authority(authority) {
            return self
                .send_bridge_command_with_live_runtime_classified(
                    Some(_authority_guard),
                    recipient,
                    command,
                    timeout,
                )
                .await;
        }
        let probe_participant_name = format!(
            "{}/pending-supervisor-probe/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let needs_routable_callback = recipient.address.transport() != PeerTransport::Inproc;
        let (runtime, dsl) = self
            .build_probe_runtime(
                &probe_participant_name,
                authority,
                needs_routable_callback && !self.uses_controlling_acceptor(),
            )
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
        Self::apply_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
        .map_err(BridgeRequestFailure::BeforeSend)?;
        #[cfg(not(target_arch = "wasm32"))]
        let temporary = if needs_routable_callback && self.uses_controlling_acceptor() {
            self.register_temporary_controlling_acceptor_runtime(
                probe_participant_name,
                &runtime,
                alternate_probe_guard.take(),
            )
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?
        } else {
            None
        };
        #[cfg(not(target_arch = "wasm32"))]
        let reply_endpoint = if needs_routable_callback {
            let endpoint = temporary
                .as_ref()
                .map(|binding| binding.reply_endpoint.clone())
                .map(Ok)
                .unwrap_or_else(|| {
                    Self::ephemeral_probe_reply_endpoint(&runtime, &self.endpoint_config)
                })
                .map_err(BridgeRequestFailure::BeforeSend)?;
            SupervisorRequestReplyEndpoint::EphemeralProbe(endpoint)
        } else {
            SupervisorRequestReplyEndpoint::InProcessProbe
        };
        #[cfg(target_arch = "wasm32")]
        let reply_endpoint = SupervisorRequestReplyEndpoint::Live;
        let request_result = self
            .request_json_with_runtime_classified(
                &runtime,
                recipient,
                super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                command,
                reply_endpoint,
                timeout,
            )
            .await;
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cleanup_result = match temporary {
                Some(binding) => Self::remove_temporary_controlling_acceptor_runtime(binding).await,
                None => Ok(()),
            };
            match (request_result, cleanup_result) {
                (result, Ok(())) => result,
                (Ok(_), Err(cleanup_error)) => Err(BridgeRequestFailure::AfterSend(cleanup_error)),
                (Err(BridgeRequestFailure::BeforeSend(request_error)), Err(cleanup_error)) => Err(
                    BridgeRequestFailure::BeforeSend(MobError::Internal(format!(
                        "alternate-authority supervisor request failed before send: {request_error}; additionally failed to remove its process-ingress lease: {cleanup_error}"
                    ))),
                ),
                (Err(BridgeRequestFailure::AfterSend(request_error)), Err(cleanup_error)) => Err(
                    BridgeRequestFailure::AfterSend(MobError::Internal(format!(
                        "alternate-authority supervisor request failed after send: {request_error}; additionally failed to remove its process-ingress lease: {cleanup_error}"
                    ))),
                ),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            request_result
        }
    }

    /// Install recipient trust ahead of a routed bridge send.
    ///
    /// Returns whether THIS call installed the trust or the recipient was
    /// already a trusted direct peer endpoint before the call. Fail-closed
    /// rollback on authorization failure must remove only newly installed
    /// trust: a peer whose trust was confirmed by an earlier terminality must
    /// not lose it because a later re-authorization attempt failed.
    pub(crate) async fn trust_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<RecipientTrustInstall, MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let runtime = self.runtime_with_gate_held().await;
        let dsl = self.dsl.read().await.clone();
        Self::apply_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
    }

    /// Fail-closed rollback for [`Self::trust_recipient`]: removes the recipient
    /// from the bridge trust set (DSL + reconciled comms runtime) when an
    /// install-before-send must be undone (e.g. supervisor authorization was
    /// rejected after trust was installed for transport).
    pub(crate) async fn untrust_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let _authority_guard = self.authority_gate.read().await;
        let runtime = self.runtime_with_gate_held().await;
        let dsl = self.dsl.read().await.clone();
        Self::remove_bridge_trust(
            &self.trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
    }

    pub(crate) async fn request_json<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        self.request_json_with_receipt(recipient, intent, payload, timeout)
            .await
            .map(|(value, _envelope_id)| value)
    }

    /// [`Self::request_json`] variant surfacing the request envelope id
    /// (DEC-P6E-19): the member-side input's `InteractionId` IS this
    /// envelope id, so interaction-terminal completion waiters key on it.
    pub(crate) async fn request_json_with_receipt<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<(serde_json::Value, uuid::Uuid), MobError> {
        self.request_json_with_receipt_classified(recipient, intent, payload, timeout)
            .await
            .map_err(BridgeRequestFailure::into_error)
    }

    async fn request_json_with_receipt_classified<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<(serde_json::Value, uuid::Uuid), BridgeRequestFailure> {
        // The shared response buffer is part of the callback path even for a
        // private TCP listener, so retain the gate through terminality.
        let _authority_guard = self.authority_gate.read().await;
        let runtime = self.runtime_with_gate_held().await;
        let envelope_id = self
            .send_supervisor_request(
                &runtime,
                recipient,
                intent,
                payload,
                SupervisorRequestReplyEndpoint::Live,
            )
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
        let value = self
            .wait_for_response(&runtime, envelope_id, recipient.peer_id, timeout)
            .await
            .map_err(BridgeRequestFailure::AfterSend)?;
        Ok((value, envelope_id))
    }

    async fn request_json_with_runtime_classified<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        reply_endpoint: SupervisorRequestReplyEndpoint,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        let envelope_id = self
            .send_supervisor_request(runtime, recipient, intent, payload, reply_endpoint)
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
        self.wait_for_response(runtime, envelope_id, recipient.peer_id, timeout)
            .await
            .map_err(BridgeRequestFailure::AfterSend)
    }

    /// Serialize + send ONE supervisor request envelope on `runtime`,
    /// returning the request envelope id replies correlate on
    /// (DEC-P6E-19). Split from the response wait so
    /// `request_json_with_receipt` can keep the authority-gate read guard over
    /// both this send and the subsequent shared response handoff.
    async fn send_supervisor_request<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        reply_endpoint: SupervisorRequestReplyEndpoint,
    ) -> Result<uuid::Uuid, MobError> {
        #[cfg(not(target_arch = "wasm32"))]
        if recipient.address.transport() == PeerTransport::Uds {
            return Err(MobError::WiringError(
                "supervisor request/response over UDS is unsupported: authenticated UDS ingress cannot authorize the required TCP reply endpoint"
                    .to_string(),
            ));
        }
        let to = PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone());
        let params = serde_json::to_value(payload).map_err(|error| {
            MobError::Internal(format!("serialize supervisor payload: {error}"))
        })?;
        #[cfg(not(target_arch = "wasm32"))]
        let receipt = match reply_endpoint {
            SupervisorRequestReplyEndpoint::InProcessProbe => {
                runtime
                    .send(CommsCommand::PeerRequest {
                        objective_id: None,
                        to,
                        intent: intent.to_string(),
                        params,
                        blocks: None,
                        content_taint: None,
                        handling_mode: HandlingMode::Queue,
                        stream: InputStreamMode::None,
                    })
                    .await?
            }
            SupervisorRequestReplyEndpoint::Live => {
                let reply_endpoint = self.live_runtime_reply_endpoint(runtime)?;
                runtime
                    .send_peer_request_at_endpoint(to, intent, params, reply_endpoint)
                    .await?
            }
            SupervisorRequestReplyEndpoint::EphemeralProbe(reply_endpoint) => {
                runtime
                    .send_peer_request_at_endpoint(to, intent, params, reply_endpoint)
                    .await?
            }
        };
        #[cfg(target_arch = "wasm32")]
        let receipt = {
            let SupervisorRequestReplyEndpoint::Live = reply_endpoint;
            runtime
                .send(CommsCommand::PeerRequest {
                    objective_id: None,
                    to,
                    intent: intent.to_string(),
                    params,
                    blocks: None,
                    content_taint: None,
                    handling_mode: HandlingMode::Queue,
                    stream: InputStreamMode::None,
                })
                .await?
        };
        let SendReceipt::PeerRequestSent { envelope_id, .. } = receipt else {
            return Err(MobError::Internal(
                "unexpected receipt for supervisor request".to_string(),
            ));
        };
        Ok(envelope_id)
    }

    /// DEC-U3 shared-intake seam: the upcall responder pushes every
    /// non-upcall candidate it drained back into the shared buffer and
    /// wakes any in-flight `wait_for_response`.
    pub(crate) async fn buffer_foreign_candidates(&self, candidates: Vec<PeerInputCandidate>) {
        if candidates.is_empty() {
            return;
        }
        self.buffered_candidates.lock().await.extend(candidates);
        self.intake_notify.notify_waiters();
    }

    /// DEC-U3 shared-intake seam: drain buffered REQUEST-class supervisor-
    /// bridge candidates (member upcalls) out of the shared buffer,
    /// retaining everything else for `wait_for_response`.
    pub(crate) async fn take_buffered_upcall_requests(&self) -> Vec<PeerInputCandidate> {
        let mut buffered = self.buffered_candidates.lock().await;
        let mut taken = Vec::new();
        let mut retained = VecDeque::with_capacity(buffered.len());
        while let Some(candidate) = buffered.pop_front() {
            let is_upcall_request = matches!(
                &candidate.interaction.content,
                meerkat_core::interaction::InteractionContent::Request { intent, .. }
                    if intent == super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT
            );
            if is_upcall_request {
                taken.push(candidate);
            } else {
                retained.push_back(candidate);
            }
        }
        *buffered = retained;
        taken
    }

    /// Waker handle for the upcall responder's select loop (DEC-U3).
    pub(crate) fn intake_notify(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.intake_notify)
    }

    async fn wait_for_response(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
        expected_responder: PeerId,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let deadline = BridgeRequestDeadline::start(timeout);
        let inbox_notify = runtime.inbox_notify();
        loop {
            // Register both waiters BEFORE checking the buffer and draining:
            // `inbox_notify` and `intake_notify` fire with `notify_waiters()`,
            // which only wakes already-registered (`enable()`d) listeners — a
            // response the upcall responder pushes via
            // `buffer_foreign_candidates` between the drain below and the
            // trailing select would otherwise be a lost wake (the comms
            // `recv_message` discipline).
            let mut inbox_notified = std::pin::pin!(inbox_notify.notified());
            inbox_notified.as_mut().enable();
            let mut intake_notified = std::pin::pin!(self.intake_notify.notified());
            intake_notified.as_mut().enable();

            // Re-check the shared buffer at EVERY loop head: a responder-
            // drained Response lands here via `buffer_foreign_candidates`
            // (DEC-U3), not on the inbox.
            if let Some(result) = self
                .take_buffered_response(runtime, request_envelope_id, expected_responder)
                .await?
            {
                return Ok(result);
            }
            let drained = runtime.drain_peer_input_candidates().await;
            if let Some(result) = self
                .buffer_and_extract(runtime, drained, request_envelope_id, expected_responder)
                .await?
            {
                return Ok(result);
            }

            let remaining = deadline.remaining();
            if remaining.is_zero() {
                Self::record_request_timed_out(runtime, request_envelope_id)?;
                // Typed timeout (not an `Internal` string): the multi-host
                // materialize dispatcher's ADJ-4 resend classification keys
                // on this class, so it must be a variant, never a substring.
                return Err(MobError::BridgeRequestTimedOut {
                    request_envelope_id: request_envelope_id.to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                });
            }

            let woke = tokio::time::timeout(remaining, async {
                tokio::select! {
                    () = inbox_notified.as_mut() => {}
                    () = intake_notified.as_mut() => {}
                }
            })
            .await;
            if woke.is_err() {
                Self::record_request_timed_out(runtime, request_envelope_id)?;
                // Typed timeout (not an `Internal` string): the multi-host
                // materialize dispatcher's ADJ-4 resend classification keys
                // on this class, so it must be a variant, never a substring.
                return Err(MobError::BridgeRequestTimedOut {
                    request_envelope_id: request_envelope_id.to_string(),
                    timeout_ms: timeout.as_millis() as u64,
                });
            }
        }
    }

    async fn take_buffered_response(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
        expected_responder: PeerId,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let mut buffered = self.buffered_candidates.lock().await;
        let mut retained = VecDeque::new();
        let mut matched = None;

        while let Some(candidate) = buffered.pop_front() {
            let outcome = Self::response_outcome(&candidate, request_envelope_id)?;
            if outcome.is_some()
                && !Self::responder_identity_matches(&candidate, expected_responder)
            {
                tracing::warn!(
                    corr_id = %request_envelope_id,
                    "supervisor bridge dropping buffered response: in_reply_to matched but responder identity did not"
                );
                continue;
            }
            match outcome {
                Some(SupervisorResponseOutcome::Terminal { value, disposition })
                    if matched.is_none() =>
                {
                    Self::record_response_terminal(runtime, request_envelope_id, disposition)?;
                    matched = Some(value);
                }
                Some(SupervisorResponseOutcome::Progress) => {
                    Self::record_response_progress(runtime, request_envelope_id)?;
                }
                Some(SupervisorResponseOutcome::Terminal { .. }) => {}
                None => retained.push_back(candidate),
            }
        }

        *buffered = retained;
        Ok(matched)
    }

    async fn buffer_and_extract(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        drained: Vec<PeerInputCandidate>,
        request_envelope_id: uuid::Uuid,
        expected_responder: PeerId,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let mut buffered = self.buffered_candidates.lock().await;
        let mut matched = None;
        let mut retained_any = false;

        for candidate in drained {
            let outcome = Self::response_outcome(&candidate, request_envelope_id)?;
            if outcome.is_some()
                && !Self::responder_identity_matches(&candidate, expected_responder)
            {
                tracing::warn!(
                    corr_id = %request_envelope_id,
                    "supervisor bridge dropping response: in_reply_to matched but responder identity did not"
                );
                continue;
            }
            match outcome {
                Some(SupervisorResponseOutcome::Terminal { value, disposition })
                    if matched.is_none() =>
                {
                    Self::record_response_terminal(runtime, request_envelope_id, disposition)?;
                    matched = Some(value);
                }
                Some(SupervisorResponseOutcome::Progress) => {
                    Self::record_response_progress(runtime, request_envelope_id)?;
                }
                Some(SupervisorResponseOutcome::Terminal { .. }) => {}
                None => {
                    retained_any = true;
                    buffered.push_back(candidate);
                }
            }
        }
        drop(buffered);

        // Wake the shared intake on EVERY retained candidate, response-class
        // included (DEC-P6E-15, FLAG-P6E-3 — the DEC-U3 lost-wake class).
        // Request-class candidates belong to the upcall responder
        // (`take_buffered_upcall_requests`); a response-class candidate that
        // is not ours belongs to a CONCURRENT `wait_for_response` — waiter A
        // can drain waiter B's reply
        // into the shared buffer, and B's only wake for it is this notify
        // (the candidate already left the inbox, so no inbox notification
        // will ever fire for it again).
        if retained_any {
            self.intake_notify.notify_waiters();
        }

        Ok(matched)
    }

    /// Bind a bridge response to the recipient the request was sent to.
    ///
    /// Request correlation matches on `in_reply_to`, but `in_reply_to` alone is
    /// not an identity: a response carrying our envelope id from any *other*
    /// trusted peer (a misattribution or a spoof) must not satisfy the awaited
    /// request. The canonical, comms-admitted sender identity
    /// (`PeerIngressFact::canonical_peer_id`) must equal the peer the request was
    /// dispatched to. Fails closed when the sender identity is absent.
    fn responder_identity_matches(
        candidate: &PeerInputCandidate,
        expected_responder: PeerId,
    ) -> bool {
        candidate.ingress.canonical_peer_id == Some(expected_responder)
    }

    /// Extract a terminal response value for the awaited request, if any.
    ///
    /// Bridge code does not interpret `ResponseStatus` directly. The
    /// ingress classifier stamps response terminality onto the candidate, and
    /// the bridge consumes that typed fact.
    fn response_value(
        candidate: &PeerInputCandidate,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MobError> {
        Ok(
            match Self::response_outcome(candidate, request_envelope_id)? {
                Some(SupervisorResponseOutcome::Terminal { value, .. }) => Some(value),
                Some(SupervisorResponseOutcome::Progress) | None => None,
            },
        )
    }

    fn response_outcome(
        candidate: &PeerInputCandidate,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<SupervisorResponseOutcome>, MobError> {
        let InteractionContent::Response {
            in_reply_to,
            status: _,
            result,
            blocks: _,
        } = &candidate.interaction.content
        else {
            return Ok(None);
        };

        if in_reply_to.0 != request_envelope_id {
            return Ok(None);
        }

        match candidate.response_terminality {
            Some(TerminalityClass::Terminal { disposition }) => match disposition {
                meerkat_core::interaction::TerminalDisposition::Completed => {
                    Ok(Some(SupervisorResponseOutcome::Terminal {
                        value: result.clone(),
                        disposition: meerkat_core::handles::PeerTerminalDisposition::Completed,
                    }))
                }
                meerkat_core::interaction::TerminalDisposition::Failed => {
                    Ok(Some(SupervisorResponseOutcome::Terminal {
                        value: result.clone(),
                        disposition: meerkat_core::handles::PeerTerminalDisposition::Failed,
                    }))
                }
                _ => Err(MobError::Internal(format!(
                    "supervisor bridge received response for {request_envelope_id} with unsupported terminal disposition"
                ))),
            },
            Some(TerminalityClass::Progress) => Ok(Some(SupervisorResponseOutcome::Progress)),
            None => Err(MobError::Internal(format!(
                "supervisor bridge received response for {request_envelope_id} without machine terminality"
            ))),
            _ => Err(MobError::Internal(format!(
                "supervisor bridge received response for {request_envelope_id} with unsupported terminality"
            ))),
        }
    }

    fn peer_interaction_handle(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Arc<dyn meerkat_core::handles::PeerInteractionHandle>, MobError> {
        runtime.peer_interaction_handle().ok_or_else(|| {
            MobError::Internal(format!(
                "supervisor request '{request_envelope_id}' has no peer-interaction authority"
            ))
        })
    }

    fn record_response_progress(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
    ) -> Result<(), MobError> {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(request_envelope_id);
        let handle = Self::peer_interaction_handle(runtime, request_envelope_id)?;
        handle.response_progress(corr_id).map_err(|error| {
            MobError::Internal(format!(
                "supervisor request '{request_envelope_id}' progress rejected by machine authority: {error}"
            ))
        })
    }

    fn record_response_terminal(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
        disposition: meerkat_core::handles::PeerTerminalDisposition,
    ) -> Result<(), MobError> {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(request_envelope_id);
        let handle = Self::peer_interaction_handle(runtime, request_envelope_id)?;
        handle.response_terminal(corr_id, disposition).map_err(|error| {
            MobError::Internal(format!(
                "supervisor request '{request_envelope_id}' terminal response rejected by machine authority: {error}"
            ))
        })
    }

    fn record_request_timed_out(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
    ) -> Result<(), MobError> {
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(request_envelope_id);
        let handle = Self::peer_interaction_handle(runtime, request_envelope_id)?;
        handle.request_timed_out(corr_id).map_err(|error| {
            MobError::Internal(format!(
                "supervisor request '{request_envelope_id}' timeout rejected by machine authority: {error}"
            ))
        })
    }
}

enum SupervisorResponseOutcome {
    Progress,
    Terminal {
        value: serde_json::Value,
        disposition: meerkat_core::handles::PeerTerminalDisposition,
    },
}

#[derive(Debug)]
struct BridgeRequestDeadline {
    started: Instant,
    timeout: Duration,
}

impl BridgeRequestDeadline {
    fn start(timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            timeout,
        }
    }

    fn remaining(&self) -> Duration {
        Self::remaining_after(self.timeout, self.started.elapsed())
    }

    fn remaining_after(timeout: Duration, elapsed: Duration) -> Duration {
        timeout.saturating_sub(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use meerkat_core::handles::PeerInteractionHandle as _;
    use meerkat_core::interaction::{InboxInteraction, InteractionId, ResponseStatus};

    #[cfg(not(target_arch = "wasm32"))]
    struct NoLocalAcceptorMaterial;

    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl super::super::builder::LocalMemberAcceptorMaterialSource for NoLocalAcceptorMaterial {
        async fn registration_for(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Option<super::super::builder::MemberAcceptorRegistration> {
            None
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn shared_bridge_with_remote(
        label: &str,
    ) -> (
        Arc<MobSupervisorBridge>,
        super::super::builder::ControllingAcceptorConfig,
        SupervisorAuthorityRecord,
        Arc<meerkat_comms::CommsRuntime>,
        Arc<meerkat_runtime::HandleDslAuthority>,
        TrustedPeerDescriptor,
        TrustedPeerDescriptor,
    ) {
        let suffix = uuid::Uuid::new_v4();
        let config = super::super::builder::ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoLocalAcceptorMaterial),
        );
        let authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new_with_controlling_acceptor(
                &crate::MobId::from(format!("mob/{label}-{suffix}")),
                authority.clone(),
                None,
                Some(config.clone()),
            )
            .await
            .expect("shared-ingress supervisor bridge should build"),
        );
        let remote_name = format!("remote-{label}-{suffix}");
        let remote_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (remote_runtime, remote_dsl) = MobSupervisorBridge::build_runtime(
            &remote_name,
            &remote_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("remote runtime should build");
        let remote = TrustedPeerDescriptor::unsigned_with_pubkey(
            remote_name,
            remote_runtime.public_key().to_peer_id().to_string(),
            *remote_runtime.public_key().as_bytes(),
            remote_runtime.advertised_address(),
        )
        .expect("remote descriptor should be valid");
        bridge
            .trust_recipient(&remote)
            .await
            .expect("supervisor should trust remote");
        let supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("remote should receive shared supervisor endpoint");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            supervisor.clone(),
        )
        .await
        .expect("remote should trust shared supervisor identity");
        (
            bridge,
            config,
            authority,
            remote_runtime,
            remote_dsl,
            remote,
            supervisor,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn fixed_private_bridge_with_remote(
        label: &str,
    ) -> (
        Arc<MobSupervisorBridge>,
        SupervisorAuthorityRecord,
        Arc<meerkat_comms::CommsRuntime>,
        Arc<meerkat_runtime::HandleDslAuthority>,
        TrustedPeerDescriptor,
        TrustedPeerDescriptor,
        PeerAddress,
    ) {
        let suffix = uuid::Uuid::new_v4();
        let port_reservation =
            std::net::TcpListener::bind("127.0.0.1:0").expect("reserve fixed loopback port");
        let fixed_port = port_reservation
            .local_addr()
            .expect("reserved port address")
            .port();
        drop(port_reservation);
        let advertised = PeerAddress::parse(format!("tcp://127.0.0.1:{fixed_port}"))
            .expect("fixed supervisor address");
        let authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &crate::MobId::from(format!("mob/{label}-{suffix}")),
                authority.clone(),
                Some(SupervisorBridgeEndpointConfig {
                    bind_address: Some(format!("127.0.0.1:{fixed_port}")),
                    advertised_address: Some(advertised.to_string()),
                }),
            )
            .await
            .expect("fixed-port supervisor bridge should build"),
        );
        let remote_name = format!("remote-{label}-{suffix}");
        let remote_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (remote_runtime, remote_dsl) = MobSupervisorBridge::build_runtime(
            &remote_name,
            &remote_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("remote runtime should build");
        let remote = TrustedPeerDescriptor::unsigned_with_pubkey(
            remote_name,
            remote_runtime.public_key().to_peer_id().to_string(),
            *remote_runtime.public_key().as_bytes(),
            remote_runtime.advertised_address(),
        )
        .expect("remote descriptor should be valid");
        bridge
            .trust_recipient(&remote)
            .await
            .expect("supervisor should trust remote");
        let supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("remote should receive fixed supervisor endpoint");
        assert_eq!(supervisor.address, advertised);
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            supervisor.clone(),
        )
        .await
        .expect("remote should trust fixed supervisor identity");
        (
            bridge,
            authority,
            remote_runtime,
            remote_dsl,
            remote,
            supervisor,
            advertised,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn ephemeral_private_bridge_with_remote(
        label: &str,
    ) -> (
        Arc<MobSupervisorBridge>,
        SupervisorAuthorityRecord,
        Arc<meerkat_comms::CommsRuntime>,
        Arc<meerkat_runtime::HandleDslAuthority>,
        TrustedPeerDescriptor,
        TrustedPeerDescriptor,
    ) {
        let suffix = uuid::Uuid::new_v4();
        let authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &crate::MobId::from(format!("mob/{label}-{suffix}")),
                authority.clone(),
                None,
            )
            .await
            .expect("private ephemeral supervisor bridge should build"),
        );
        let remote_name = format!("remote-{label}-{suffix}");
        let remote_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (remote_runtime, remote_dsl) = MobSupervisorBridge::build_runtime(
            &remote_name,
            &remote_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("remote runtime should build");
        let remote = TrustedPeerDescriptor::unsigned_with_pubkey(
            remote_name,
            remote_runtime.public_key().to_peer_id().to_string(),
            *remote_runtime.public_key().as_bytes(),
            remote_runtime.advertised_address(),
        )
        .expect("remote descriptor should be valid");
        bridge
            .trust_recipient(&remote)
            .await
            .expect("supervisor should trust remote");
        let supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("remote should receive ephemeral supervisor endpoint");
        assert_eq!(supervisor.address.transport(), PeerTransport::Tcp);
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            supervisor.clone(),
        )
        .await
        .expect("remote should trust ephemeral supervisor identity");
        (
            bridge,
            authority,
            remote_runtime,
            remote_dsl,
            remote,
            supervisor,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn next_remote_candidate(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
    ) -> PeerInputCandidate {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(candidate) = runtime.drain_peer_input_candidates().await.pop() {
                    break candidate;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("remote candidate should arrive")
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn attach_inproc_remote(
        bridge: &Arc<MobSupervisorBridge>,
        label: &str,
    ) -> (
        Arc<meerkat_comms::CommsRuntime>,
        Arc<meerkat_runtime::HandleDslAuthority>,
        TrustedPeerDescriptor,
        TrustedPeerDescriptor,
    ) {
        let suffix = uuid::Uuid::new_v4();
        let remote_name = format!("inproc-remote-{label}-{suffix}");
        let remote_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (remote_runtime, remote_dsl) = MobSupervisorBridge::build_runtime_with_tcp_listener(
            &remote_name,
            &remote_authority,
            &SupervisorBridgeEndpointConfig::default(),
            false,
        )
        .await
        .expect("inproc remote runtime should build");
        let remote = TrustedPeerDescriptor::unsigned_with_pubkey(
            remote_name,
            remote_runtime.public_key().to_peer_id().to_string(),
            *remote_runtime.public_key().as_bytes(),
            remote_runtime.advertised_address(),
        )
        .expect("inproc remote descriptor should be valid");
        assert_eq!(remote.address.transport(), PeerTransport::Inproc);
        bridge
            .trust_recipient(&remote)
            .await
            .expect("supervisor should trust inproc remote");
        let supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("inproc remote should receive current supervisor identity");
        assert_eq!(supervisor.address.transport(), PeerTransport::Inproc);
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            supervisor.clone(),
        )
        .await
        .expect("inproc remote should trust current supervisor identity");
        (remote_runtime, remote_dsl, remote, supervisor)
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn reply_to_remote_candidate(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        candidate: PeerInputCandidate,
        result: serde_json::Value,
    ) {
        let reply_route = candidate.ingress.route.clone().or_else(|| {
            candidate
                .interaction
                .from_route
                .map(meerkat_core::PeerRoute::new)
        });
        runtime
            .peer_interaction_handle()
            .expect("remote peer interaction authority")
            .request_received(
                meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0),
                candidate.interaction.handling_mode,
            )
            .expect("record remote request before sending its response");
        runtime
            .send(CommsCommand::PeerResponse {
                to: reply_route.expect("request should carry an authenticated reply route"),
                in_reply_to: candidate.interaction.id,
                status: ResponseStatus::Completed,
                result,
                blocks: None,
                content_taint: None,
                handling_mode: None,
                objective_id: candidate.interaction.objective_id,
            })
            .await
            .expect("remote response should reach the supervisor runtime");
    }

    fn response_candidate(
        request_envelope_id: uuid::Uuid,
        status: ResponseStatus,
        result: serde_json::Value,
    ) -> PeerInputCandidate {
        let worker_peer_id = PeerId::from_uuid(
            uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f5")
                .expect("valid worker route id"),
        );
        response_candidate_from(
            request_envelope_id,
            status,
            result,
            worker_peer_id,
            "worker-rt",
        )
    }

    fn response_candidate_from(
        request_envelope_id: uuid::Uuid,
        status: ResponseStatus,
        result: serde_json::Value,
        responder_peer_id: PeerId,
        responder_name: &str,
    ) -> PeerInputCandidate {
        let id = InteractionId(uuid::Uuid::new_v4());
        meerkat_runtime::test_peer_input_candidate_from_interaction(
            InboxInteraction {
                objective_id: None,
                sender_taint: None,
                id,
                from_route: Some(responder_peer_id),
                from: responder_name.to_string(),
                content: InteractionContent::Response {
                    in_reply_to: InteractionId(request_envelope_id),
                    status,
                    result,
                    blocks: None,
                },
                rendered_text: "[worker-rt]: response".to_string(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            responder_peer_id,
        )
    }

    #[tokio::test]
    async fn supervisor_runtime_installs_generated_peer_comms_authority() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (runtime, _dsl) = MobSupervisorBridge::build_runtime(
            "mob/__mob_supervisor__/authority-test",
            &authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("supervisor runtime should build");

        assert!(
            runtime.peer_comms_machine_authority_required(),
            "supervisor bridge ingress must fail closed without generated peer-comms authority"
        );
        assert!(
            runtime.peer_comms_handle().is_some(),
            "supervisor bridge ingress terminality must come from generated peer-comms authority"
        );
        assert!(
            runtime.peer_interaction_handle().is_some(),
            "supervisor bridge request lifecycle must use generated peer-interaction authority"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ephemeral_probe_reply_endpoint_uses_bound_port_and_advertised_host() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let endpoint_config = SupervisorBridgeEndpointConfig {
            bind_address: Some("127.0.0.1:0".to_string()),
            advertised_address: Some("tcp://supervisor.example.test:42000".to_string()),
        };
        let (runtime, _dsl) = MobSupervisorBridge::build_runtime(
            "mob/__mob_supervisor__/ephemeral-reply-test",
            &authority,
            &endpoint_config,
        )
        .await
        .expect("ephemeral probe runtime should build");

        let bound = runtime
            .bound_tcp_listener_address()
            .expect("started probe exposes its concrete listener");
        let reply =
            MobSupervisorBridge::ephemeral_probe_reply_endpoint(runtime.as_ref(), &endpoint_config)
                .expect("probe reply endpoint should derive from live listener truth");
        assert_eq!(reply.transport(), PeerTransport::Tcp);
        assert_eq!(
            reply.endpoint(),
            format!("supervisor.example.test:{}", bound.port())
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn live_runtime_reply_endpoint_preserves_advertised_port() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let advertised = "tcp://supervisor.example.test:42000";
        let bridge = MobSupervisorBridge::new(
            &crate::MobId::from(format!("mob/live-reply-endpoint-{}", uuid::Uuid::new_v4())),
            authority,
            Some(SupervisorBridgeEndpointConfig {
                bind_address: Some("127.0.0.1:0".to_string()),
                advertised_address: Some(advertised.to_string()),
            }),
        )
        .await
        .expect("supervisor bridge should build");
        let runtime = bridge.runtime.read().await.clone();

        let reply = bridge
            .live_runtime_reply_endpoint(runtime.as_ref())
            .expect("live reply endpoint should use advertised transport truth");

        assert_eq!(
            reply,
            PeerAddress::parse(advertised).expect("advertised endpoint should parse"),
            "the live path must preserve a fixed proxy/NAT port instead of substituting the local bind port"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ordinary_supervisor_request_carries_live_signed_reply_endpoint() {
        let suffix = uuid::Uuid::new_v4();
        let bridge = MobSupervisorBridge::new(
            &crate::MobId::from(format!("mob/live-reply-request-{suffix}")),
            SupervisorAuthorityRecord::generate(
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            ),
            None,
        )
        .await
        .expect("supervisor bridge should build");
        let recipient_name = format!("live-reply-recipient-{suffix}");
        let recipient_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (recipient_runtime, recipient_dsl) = MobSupervisorBridge::build_runtime(
            &recipient_name,
            &recipient_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("recipient runtime should build");
        let recipient = TrustedPeerDescriptor::unsigned_with_pubkey(
            recipient_name,
            recipient_runtime.public_key().to_peer_id().to_string(),
            *recipient_runtime.public_key().as_bytes(),
            recipient_runtime.advertised_address(),
        )
        .expect("recipient descriptor should be valid");
        bridge
            .trust_recipient(&recipient)
            .await
            .expect("sender should trust recipient");
        let sender = bridge
            .supervisor_spec_for_recipient(&recipient)
            .await
            .expect("sender descriptor should resolve");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &recipient_runtime,
            &recipient_dsl,
            sender,
        )
        .await
        .expect("recipient should trust sender for classified ingress");
        let runtime = bridge.runtime.read().await.clone();
        let expected_reply_endpoint = bridge
            .live_runtime_reply_endpoint(runtime.as_ref())
            .expect("live reply endpoint should resolve");

        let request_envelope_id = bridge
            .send_supervisor_request(
                &runtime,
                &recipient,
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                &serde_json::json!({"probe": "live-reply-endpoint"}),
                SupervisorRequestReplyEndpoint::Live,
            )
            .await
            .expect("ordinary supervisor request should send");
        let mut candidates = recipient_runtime.drain_peer_input_candidates().await;
        assert_eq!(candidates.len(), 1, "recipient should admit one request");
        let candidate = candidates.pop().expect("one admitted request");

        assert_eq!(candidate.interaction.id.0, request_envelope_id);
        assert_eq!(
            candidate.ingress.declared_reply_endpoint,
            Some(expected_reply_endpoint),
            "ordinary production requests must carry the signed live callback endpoint"
        );
        MobSupervisorBridge::record_request_timed_out(&runtime, request_envelope_id)
            .expect("test request lifecycle should clean up");
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn shared_process_ingress_delivers_remote_envelope_to_supervisor_inbox() {
        let suffix = uuid::Uuid::new_v4();
        let config = super::super::builder::ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoLocalAcceptorMaterial),
        );
        let bridge = MobSupervisorBridge::new_with_controlling_acceptor(
            &crate::MobId::from(format!("mob/shared-supervisor-ingress-{suffix}")),
            SupervisorAuthorityRecord::generate(
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            ),
            None,
            Some(config),
        )
        .await
        .expect("shared-ingress supervisor bridge should build");
        let remote_name = format!("remote-host-{suffix}");
        let remote_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (remote_runtime, remote_dsl) = MobSupervisorBridge::build_runtime(
            &remote_name,
            &remote_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("remote runtime should build");
        let remote = TrustedPeerDescriptor::unsigned_with_pubkey(
            remote_name,
            remote_runtime.public_key().to_peer_id().to_string(),
            *remote_runtime.public_key().as_bytes(),
            remote_runtime.advertised_address(),
        )
        .expect("remote descriptor should be valid");
        bridge
            .trust_recipient(&remote)
            .await
            .expect("supervisor should trust remote sender");
        let supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("remote should receive shared supervisor endpoint");
        assert_eq!(supervisor.address.transport(), PeerTransport::Tcp);
        let live_runtime = bridge.runtime().await;
        assert_eq!(
            bridge
                .live_runtime_reply_endpoint(live_runtime.as_ref())
                .expect("signed callback endpoint"),
            supervisor.address,
            "the peer descriptor and signed request callback must name the same shared listener"
        );
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            supervisor.clone(),
        )
        .await
        .expect("remote should trust shared supervisor identity");

        let receipt = remote_runtime
            .send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(supervisor.peer_id, supervisor.name.clone()),
                body: "shared-ingress-probe".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            })
            .await
            .expect("remote envelope should traverse the shared TCP listener");
        assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));
        let candidates = live_runtime.drain_peer_input_candidates().await;
        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            &candidates[0].interaction.content,
            InteractionContent::Message { body, .. } if body == "shared-ingress-probe"
        ));

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn shared_ingress_rotation_atomically_fences_old_key_and_accepts_new_key() {
        let (bridge, config, current, remote_runtime, remote_dsl, remote, old_supervisor) =
            shared_bridge_with_remote("shared-atomic-rotation").await;
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let next_supervisor = bridge
            .supervisor_spec_for_authority_and_recipient(&next, &remote)
            .await
            .expect("next authority should retain the shared endpoint");
        assert_eq!(next_supervisor.address, old_supervisor.address);

        bridge
            .rotate(next.clone())
            .await
            .expect("shared supervisor rotation should commit");
        assert_eq!(bridge.authority().await, next);
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "atomic replacement must leave exactly one shared-ingress registration"
        );

        let old_send = tokio::time::timeout(
            Duration::from_secs(2),
            remote_runtime.send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(
                    old_supervisor.peer_id,
                    old_supervisor.name.clone(),
                ),
                body: "retired-shared-key".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            }),
        )
        .await
        .expect("retired-key rejection should be prompt");
        assert!(
            old_send.is_err(),
            "the old shared-ingress key must be fenced when rotation returns"
        );

        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            next_supervisor.clone(),
        )
        .await
        .expect("remote should trust the replacement supervisor identity");
        let receipt = remote_runtime
            .send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(
                    next_supervisor.peer_id,
                    next_supervisor.name.clone(),
                ),
                body: "replacement-shared-key".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            })
            .await
            .expect("replacement key should be accepted immediately");
        assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));
        let replacement_runtime = bridge.runtime().await;
        let delivered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(candidate) = replacement_runtime
                    .drain_peer_input_candidates()
                    .await
                    .pop()
                {
                    break candidate;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("replacement runtime should receive the new-key envelope");
        assert!(matches!(
            delivered.interaction.content,
            InteractionContent::Message { body, .. } if body == "replacement-shared-key"
        ));

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn shared_ingress_inproc_conflict_rolls_back_prepared_acceptor_replacement() {
        let (bridge, config, current, remote_runtime, _remote_dsl, remote, old_supervisor) =
            shared_bridge_with_remote("shared-inproc-conflict").await;
        let current_runtime = bridge.runtime().await;
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let prepared = bridge
            .prepare_live_runtime(&next)
            .await
            .expect("shared replacement runtime should prepare without publication");

        // Occupy only the candidate key in the inproc registry. The live
        // predecessor remains exact, so publication must fail without mutation
        // after the shared-acceptor replacement has already been reserved.
        let conflict_name = format!("inproc-conflict-{}", uuid::Uuid::new_v4());
        let conflict = meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            &conflict_name,
            None,
            next.keypair(),
            Arc::new(HashSet::new()),
        )
        .expect("candidate-key conflict should publish under another name");
        let error = bridge
            .commit_prepared_rotation(PreparedSupervisorBridgeRotation {
                authority: next,
                prebuilt: Some(prepared),
            })
            .await
            .expect_err("occupied inproc replacement key must reject rotation");
        assert!(
            error
                .to_string()
                .contains("replacement public key is already occupied"),
            "unexpected inproc publication error: {error}"
        );
        assert_eq!(bridge.authority().await, current);
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "failed inproc publication must drop the prepared acceptor reservation inertly"
        );

        let receipt = remote_runtime
            .send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(
                    old_supervisor.peer_id,
                    old_supervisor.name.clone(),
                ),
                body: "old-route-after-conflict".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            })
            .await
            .expect("old shared route must remain live after publication conflict");
        assert!(matches!(receipt, SendReceipt::PeerMessageSent { .. }));
        let delivered = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(candidate) = current_runtime.drain_peer_input_candidates().await.pop() {
                    break candidate;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("current runtime should still receive through the old shared key");
        assert!(matches!(
            delivered.interaction.content,
            InteractionContent::Message { body, .. } if body == "old-route-after-conflict"
        ));

        drop(conflict);
        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn alternate_authority_probe_shared_ingress_lease_cleans_exactly() {
        let suffix = uuid::Uuid::new_v4();
        let config = super::super::builder::ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoLocalAcceptorMaterial),
        );
        let bridge = MobSupervisorBridge::new_with_controlling_acceptor(
            &crate::MobId::from(format!("mob/shared-probe-cleanup-{suffix}")),
            SupervisorAuthorityRecord::generate(
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            ),
            None,
            Some(config.clone()),
        )
        .await
        .expect("shared-ingress supervisor bridge should build");
        assert_eq!(config.registration_count_for_test().await, 1);

        let alternate = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let probe_name = format!(
            "{}/pending-supervisor-probe/{suffix}",
            bridge.participant_name
        );
        let (probe_runtime, _probe_dsl) = bridge
            .build_probe_runtime(&probe_name, &alternate, false)
            .await
            .expect("listenerless alternate-authority probe");
        assert!(probe_runtime.bound_tcp_listener_address().is_none());
        let temporary = bridge
            .register_temporary_controlling_acceptor_runtime(probe_name, &probe_runtime, None)
            .await
            .expect("temporary probe registration")
            .expect("shared mode must mint a temporary lease");
        assert_eq!(config.registration_count_for_test().await, 2);

        MobSupervisorBridge::remove_temporary_controlling_acceptor_runtime(temporary)
            .await
            .expect("temporary probe lease cleanup");
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "temporary cleanup must preserve the durable live supervisor lease"
        );
        let live = bridge
            .routable_supervisor_spec()
            .await
            .expect("live supervisor remains routable");
        tokio::net::TcpStream::connect(live.address.endpoint())
            .await
            .expect("live shared endpoint remains reachable after probe cleanup");

        bridge.shutdown().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn current_authority_sends_reuse_shared_supervisor_ingress_lease() {
        let (bridge, config, authority, remote_runtime, _remote_dsl, remote, supervisor) =
            shared_bridge_with_remote("shared-current-authority").await;
        assert_eq!(config.registration_count_for_test().await, 1);

        // The durable actor record can carry a pending-rotation retry anchor
        // while the live bridge still installs this exact stable transport.
        // That metadata difference must not turn a current-authority call into
        // a temporary same-key registration on shared process ingress.
        let mut requested_authority = authority.clone();
        let mut pending_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        pending_authority.epoch = authority.epoch + 1;
        requested_authority.pending_rotation = Some(
            crate::store::SupervisorPendingRotationRecord::from_authority(
                &pending_authority,
                super::super::bridge_protocol::SupervisorRotationOperationId::new(),
                Vec::new(),
                std::collections::BTreeMap::new(),
            ),
        );
        assert_ne!(requested_authority, authority);

        let delivery =
            super::super::bridge_protocol::BridgeSupervisorDelivery::SubmitSupervisorRotation(
                super::super::bridge_protocol::BridgeSupervisorRotationSubmit {
                    operation_id: super::super::bridge_protocol::SupervisorRotationOperationId::new(
                    ),
                    target: supervisor.clone().into(),
                    target_epoch: authority.epoch,
                    protocol_version:
                        super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                },
            );
        bridge
            .send_supervisor_delivery_as_authority(&requested_authority, &remote, &delivery)
            .await
            .expect("current-authority delivery should reuse the live shared lease");
        let delivered = next_remote_candidate(&remote_runtime).await;
        assert!(matches!(
            delivered.interaction.content,
            InteractionContent::Message { .. }
        ));

        let command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: supervisor.into(),
                epoch: authority.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/shared-current-authority".to_string(),
            },
        );
        bridge
            .send_bridge_command_as_authority(
                &requested_authority,
                &remote,
                &command,
                Duration::from_millis(40),
            )
            .await
            .expect_err("unanswered current-authority request should time out after send");
        let unclassified = next_remote_candidate(&remote_runtime).await;
        assert!(matches!(
            unclassified.interaction.content,
            InteractionContent::Request { .. }
        ));

        let classified = bridge
            .send_bridge_command_as_authority_classified(
                &requested_authority,
                &remote,
                &command,
                Duration::from_millis(40),
            )
            .await;
        assert!(
            matches!(classified, Err(BridgeRequestFailure::AfterSend(_))),
            "a current-authority shared request must reach the transport before timing out: {classified:?}"
        );
        let classified_candidate = next_remote_candidate(&remote_runtime).await;
        assert!(matches!(
            classified_candidate.interaction.content,
            InteractionContent::Request { .. }
        ));
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "current-authority sends must never mint a colliding temporary lease"
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn private_inproc_authority_request_retains_live_route_until_response_before_rotation() {
        let suffix = uuid::Uuid::new_v4();
        let mob_id = crate::MobId::from(format!("mob/private-inflight-rotation-{suffix}"));
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(&mob_id, current.clone(), None)
                .await
                .expect("private supervisor bridge should build"),
        );
        let (remote_runtime, _remote_dsl, remote, supervisor) =
            attach_inproc_remote(&bridge, "private-inflight-rotation").await;
        let command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: supervisor.into(),
                epoch: current.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: mob_id.to_string(),
            },
        );

        let requesting_bridge = Arc::clone(&bridge);
        let request_authority = current.clone();
        let request_recipient = remote.clone();
        let request_task = tokio::spawn(async move {
            requesting_bridge
                .send_bridge_command_as_authority(
                    &request_authority,
                    &request_recipient,
                    &command,
                    Duration::from_secs(2),
                )
                .await
        });
        let candidate = next_remote_candidate(&remote_runtime).await;

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let rotating_bridge = Arc::clone(&bridge);
        let (rotation_started_tx, rotation_started_rx) = tokio::sync::oneshot::channel();
        let mut rotation = tokio::spawn(async move {
            let _ = rotation_started_tx.send(());
            rotating_bridge.rotate(next.clone()).await.map(|()| next)
        });
        rotation_started_rx
            .await
            .expect("concurrent rotation task should start");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut rotation)
                .await
                .is_err(),
            "private TCP listener capture cannot preserve an inproc callback; rotation must wait for response terminality"
        );

        reply_to_remote_candidate(
            &remote_runtime,
            candidate,
            serde_json::json!({"old_authority_reply": true}),
        )
        .await;
        let response = tokio::time::timeout(Duration::from_secs(2), request_task)
            .await
            .expect("inproc response should reach the retained current runtime")
            .expect("request task should not panic")
            .expect("current-authority request should complete before rotation");
        assert_eq!(response, serde_json::json!({"old_authority_reply": true}));
        let rotated = tokio::time::timeout(Duration::from_secs(2), rotation)
            .await
            .expect("rotation should proceed after response terminality")
            .expect("rotation task should not panic")
            .expect("rotation should succeed");
        assert_eq!(bridge.authority().await, rotated);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn concurrent_alternate_authority_probes_serialize_and_cancel_cleanup_exactly() {
        let (bridge, config, current, remote_runtime, remote_dsl, remote, supervisor) =
            shared_bridge_with_remote("shared-concurrent-alternate").await;
        let mut alternate = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        alternate.epoch = current.epoch + 1;
        let alternate_key = alternate.keypair().public_key();
        let alternate_supervisor = TrustedPeerDescriptor::unsigned_with_pubkey(
            bridge.participant_name.clone(),
            alternate.public_peer_id.clone(),
            *alternate_key.as_bytes(),
            supervisor.address.to_string(),
        )
        .expect("alternate supervisor descriptor should use shared ingress");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            alternate_supervisor.clone(),
        )
        .await
        .expect("remote should trust historical supervisor authority");

        let command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: alternate_supervisor.into(),
                epoch: alternate.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/shared-concurrent-alternate".to_string(),
            },
        );
        let responder_runtime = Arc::clone(&remote_runtime);
        let responder = tokio::spawn(async move {
            for ordinal in 0..2_u64 {
                let candidate = next_remote_candidate(&responder_runtime).await;
                let reply_route = candidate.ingress.route.clone().or_else(|| {
                    candidate
                        .interaction
                        .from_route
                        .map(meerkat_core::PeerRoute::new)
                });
                responder_runtime
                    .peer_interaction_handle()
                    .expect("remote peer interaction authority")
                    .request_received(
                        meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0),
                        candidate.interaction.handling_mode,
                    )
                    .expect("record alternate request before sending its response");
                responder_runtime
                    .send(CommsCommand::PeerResponse {
                        to: reply_route
                            .expect("alternate request should carry an authenticated reply route"),
                        in_reply_to: candidate.interaction.id,
                        status: ResponseStatus::Completed,
                        result: serde_json::json!({"ordinal": ordinal}),
                        blocks: None,
                        content_taint: None,
                        handling_mode: None,
                        objective_id: candidate.interaction.objective_id,
                    })
                    .await
                    .expect("alternate authority response should traverse shared ingress");
            }
        });

        let first_bridge = Arc::clone(&bridge);
        let first_authority = alternate.clone();
        let first_recipient = remote.clone();
        let first_command = command.clone();
        let first = tokio::spawn(async move {
            first_bridge
                .send_bridge_command_as_authority(
                    &first_authority,
                    &first_recipient,
                    &first_command,
                    Duration::from_secs(2),
                )
                .await
        });
        let second_bridge = Arc::clone(&bridge);
        let second_authority = alternate.clone();
        let second_recipient = remote.clone();
        let second_command = command.clone();
        let second = tokio::spawn(async move {
            second_bridge
                .send_bridge_command_as_authority(
                    &second_authority,
                    &second_recipient,
                    &second_command,
                    Duration::from_secs(2),
                )
                .await
        });

        for result in [first, second] {
            tokio::time::timeout(Duration::from_secs(3), result)
                .await
                .expect("concurrent alternate probe should terminate")
                .expect("alternate probe task should not panic")
                .expect("alternate probe should receive its response");
        }
        responder.await.expect("responder should not panic");
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "serialized probes must preserve only the durable live lease"
        );

        let cancelled_bridge = Arc::clone(&bridge);
        let cancelled_authority = alternate;
        let cancelled_recipient = remote.clone();
        let cancelled_command = command;
        let cancelled = tokio::spawn(async move {
            cancelled_bridge
                .send_bridge_command_as_authority(
                    &cancelled_authority,
                    &cancelled_recipient,
                    &cancelled_command,
                    Duration::from_secs(30),
                )
                .await
        });
        let _unanswered = next_remote_candidate(&remote_runtime).await;
        assert_eq!(
            config.registration_count_for_test().await,
            2,
            "in-flight alternate probe should own one temporary lease"
        );
        cancelled.abort();
        assert!(
            cancelled
                .await
                .expect_err("probe should be cancelled")
                .is_cancelled(),
            "test must exercise future-drop cleanup"
        );
        let post_cancel_guard = tokio::time::timeout(
            Duration::from_secs(2),
            Arc::clone(&bridge.alternate_authority_probe_gate).lock_owned(),
        )
        .await
        .expect("a later alternate probe should resume after cancellation cleanup");
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "the alternate gate must remain held until cancelled-lease compare-remove finishes"
        );
        drop(post_cancel_guard);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn cancelled_fixed_port_alternate_request_restores_current_runtime_and_same_port() {
        let (bridge, current, remote_runtime, remote_dsl, remote, current_supervisor, advertised) =
            fixed_private_bridge_with_remote("fixed-cancel-restore").await;
        let original_runtime = bridge.runtime().await;
        let original_bound = original_runtime.bound_tcp_listener_address();
        assert_eq!(
            original_bound,
            Some(
                advertised
                    .endpoint()
                    .parse()
                    .expect("fixed advertised socket address")
            )
        );

        let mut alternate = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        alternate.epoch = current.epoch + 1;
        let alternate_supervisor = bridge
            .supervisor_spec_for_authority_and_recipient(&alternate, &remote)
            .await
            .expect("alternate supervisor should preserve the fixed endpoint");
        assert_eq!(alternate_supervisor.address, advertised);
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            alternate_supervisor.clone(),
        )
        .await
        .expect("remote should trust alternate supervisor identity");
        let alternate_command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: alternate_supervisor.into(),
                epoch: alternate.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/fixed-cancel-restore".to_string(),
            },
        );

        // The outer task is the caller. The authority worker it awaits owns the
        // full fixed-port lifecycle and must survive this caller being aborted.
        let caller_bridge = Arc::clone(&bridge);
        let caller_authority = alternate.clone();
        let caller_recipient = remote.clone();
        let caller = tokio::spawn(async move {
            caller_bridge
                .send_bridge_command_as_authority(
                    &caller_authority,
                    &caller_recipient,
                    &alternate_command,
                    Duration::from_millis(250),
                )
                .await
        });
        let unanswered = next_remote_candidate(&remote_runtime).await;
        assert_eq!(
            unanswered.ingress.declared_reply_endpoint,
            Some(advertised.clone()),
            "the alternate request must preserve the same fixed callback port"
        );
        caller.abort();
        assert!(
            caller
                .await
                .expect_err("outer fixed-port request caller should be cancelled")
                .is_cancelled(),
            "test must drop the future that awaits the owned authority worker"
        );

        let restored = tokio::time::timeout(Duration::from_secs(3), bridge.authority())
            .await
            .expect("owned worker should restore authority after its bounded wait");
        assert_eq!(restored, current);
        let restored_runtime = bridge.runtime().await;
        assert_eq!(
            restored_runtime.public_key(),
            current.keypair().public_key(),
            "restored runtime must carry the original signing identity"
        );
        assert_eq!(
            restored_runtime.bound_tcp_listener_address(),
            original_bound,
            "restoration must rebind the exact fixed proxy/NAT port"
        );

        let current_command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: current_supervisor.into(),
                epoch: current.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/fixed-cancel-restore".to_string(),
            },
        );
        let current_bridge = Arc::clone(&bridge);
        let current_authority = current.clone();
        let current_recipient = remote.clone();
        let current_request = tokio::spawn(async move {
            current_bridge
                .send_bridge_command_as_authority(
                    &current_authority,
                    &current_recipient,
                    &current_command,
                    Duration::from_secs(2),
                )
                .await
        });
        let current_candidate = next_remote_candidate(&remote_runtime).await;
        reply_to_remote_candidate(
            &remote_runtime,
            current_candidate,
            serde_json::json!({"current": true}),
        )
        .await;
        assert_eq!(
            current_request
                .await
                .expect("current-authority request task should not panic")
                .expect("current authority should round-trip after cancellation recovery"),
            serde_json::json!({"current": true})
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn fixed_port_post_bind_failure_restores_current_roundtrip_before_retry() {
        let (bridge, current, remote_runtime, _remote_dsl, remote, _supervisor, advertised) =
            fixed_private_bridge_with_remote("fixed-post-bind-restore").await;
        *FAIL_NEXT_SUPERVISOR_INSTALL_AFTER_LISTENER_BIND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(bridge.participant_name.clone());
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;

        let error = bridge
            .rotate(next)
            .await
            .expect_err("post-bind install fault should reject the rotation");
        assert!(
            error
                .to_string()
                .contains("fault-injected supervisor install failure after listener bind"),
            "unexpected post-bind rotation error: {error}"
        );
        assert_eq!(bridge.authority().await, current);
        assert_eq!(
            bridge.runtime().await.bound_tcp_listener_address(),
            Some(
                advertised
                    .endpoint()
                    .parse()
                    .expect("fixed advertised socket address")
            ),
            "rollback must restore the exact fixed listener before returning"
        );

        let requesting_bridge = Arc::clone(&bridge);
        let request_recipient = remote.clone();
        let request = tokio::spawn(async move {
            requesting_bridge
                .request_json(
                    &request_recipient,
                    super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    &serde_json::json!({"probe": "before-any-retry"}),
                    Duration::from_secs(2),
                )
                .await
        });
        let candidate = next_remote_candidate(&remote_runtime).await;
        reply_to_remote_candidate(
            &remote_runtime,
            candidate,
            serde_json::json!({"restored": true}),
        )
        .await;
        assert_eq!(
            request
                .await
                .expect("restored request task should not panic")
                .expect("restored current authority should round-trip before any retry"),
            serde_json::json!({"restored": true})
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn fixed_port_same_key_rotation_recovers_an_unhealthy_listener() {
        let (bridge, current, remote_runtime, _remote_dsl, remote, _supervisor, advertised) =
            fixed_private_bridge_with_remote("fixed-same-key-health").await;
        bridge.runtime().await.stop_listeners_for_rebind().await;
        assert!(
            bridge
                .runtime()
                .await
                .bound_tcp_listener_address()
                .is_none()
        );

        bridge
            .rotate(current.clone())
            .await
            .expect("same-key rotation should rebuild an unhealthy fixed listener");
        assert_eq!(bridge.authority().await, current);
        assert_eq!(
            bridge.runtime().await.bound_tcp_listener_address(),
            Some(
                advertised
                    .endpoint()
                    .parse()
                    .expect("fixed advertised socket address")
            ),
            "same-key recovery must restore the exact fixed port"
        );

        let requesting_bridge = Arc::clone(&bridge);
        let request_recipient = remote.clone();
        let request = tokio::spawn(async move {
            requesting_bridge
                .request_json(
                    &request_recipient,
                    super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    &serde_json::json!({"probe": "same-key-recovery"}),
                    Duration::from_secs(2),
                )
                .await
        });
        let candidate = next_remote_candidate(&remote_runtime).await;
        reply_to_remote_candidate(
            &remote_runtime,
            candidate,
            serde_json::json!({"healthy": true}),
        )
        .await;
        assert_eq!(
            request
                .await
                .expect("same-key request task should not panic")
                .expect("same-key recovered authority should round-trip"),
            serde_json::json!({"healthy": true})
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn cancelled_fixed_port_rotation_caller_does_not_cancel_rebuild() {
        let (bridge, current, remote_runtime, _remote_dsl, _remote, _supervisor, advertised) =
            fixed_private_bridge_with_remote("fixed-rotation-cancel").await;
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let expected = next.clone();

        // Queue the owned worker behind the write gate, then cancel only its
        // caller. Once admitted, the worker must finish the fixed-port rebuild.
        let held = bridge.authority_gate.write().await;
        let rotating_bridge = Arc::clone(&bridge);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            let _ = started_tx.send(());
            rotating_bridge.rotate(next).await
        });
        started_rx
            .await
            .expect("rotation caller should enter before cancellation");
        tokio::task::yield_now().await;
        caller.abort();
        assert!(
            caller
                .await
                .expect_err("outer rotation caller should be cancelled")
                .is_cancelled()
        );
        drop(held);

        let active = tokio::time::timeout(Duration::from_secs(3), bridge.authority())
            .await
            .expect("owned fixed-port worker should finish after caller cancellation");
        assert_eq!(active, expected);
        assert_eq!(
            bridge.runtime().await.bound_tcp_listener_address(),
            Some(
                advertised
                    .endpoint()
                    .parse()
                    .expect("fixed advertised socket address")
            ),
            "owned rotation must rebind the exact fixed port"
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn cancelled_inproc_alternate_authority_probe_never_replaces_live_identity() {
        let suffix = uuid::Uuid::new_v4();
        let config = super::super::builder::ControllingAcceptorConfig::new(
            "127.0.0.1:0".parse().expect("loopback address"),
            None,
            Arc::new(NoLocalAcceptorMaterial),
        );
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new_with_controlling_acceptor(
                &crate::MobId::from(format!("mob/inproc-cancel-restore-{suffix}")),
                current.clone(),
                None,
                Some(config.clone()),
            )
            .await
            .expect("shared-ingress supervisor bridge should build"),
        );
        let original_runtime = bridge.runtime().await;
        let (remote_runtime, remote_dsl, remote, current_supervisor) =
            attach_inproc_remote(&bridge, "cancel-restore").await;
        assert_eq!(config.registration_count_for_test().await, 1);

        let mut alternate = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        alternate.epoch = current.epoch + 1;
        let alternate_supervisor = bridge
            .supervisor_spec_for_authority_and_recipient(&alternate, &remote)
            .await
            .expect("inproc remote should receive alternate supervisor identity");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            alternate_supervisor.clone(),
        )
        .await
        .expect("inproc remote should trust alternate supervisor identity");
        let alternate_command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: alternate_supervisor.into(),
                epoch: alternate.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/inproc-cancel-restore".to_string(),
            },
        );

        let requesting_bridge = Arc::clone(&bridge);
        let requested_authority = alternate.clone();
        let request_recipient = remote.clone();
        let cancelled = tokio::spawn(async move {
            requesting_bridge
                .send_bridge_command_as_authority(
                    &requested_authority,
                    &request_recipient,
                    &alternate_command,
                    Duration::from_secs(30),
                )
                .await
        });
        let _withheld_response = next_remote_candidate(&remote_runtime).await;

        assert_eq!(
            bridge.authority_with_gate_held(),
            current,
            "an alternate inproc probe must not mutate the durable authority"
        );
        let live_runtime = bridge.runtime.read().await.clone();
        assert_eq!(live_runtime.public_key(), current.keypair().public_key());
        assert!(Arc::ptr_eq(&live_runtime, &original_runtime));
        assert_eq!(
            config.registration_count_for_test().await,
            1,
            "an inproc probe needs no temporary process-ingress lease"
        );

        cancelled.abort();
        assert!(
            cancelled
                .await
                .expect_err("alternate request should be cancelled")
                .is_cancelled(),
            "test must exercise future-drop probe cleanup"
        );
        tokio::time::timeout(
            Duration::from_secs(2),
            Arc::clone(&bridge.alternate_authority_probe_gate).lock_owned(),
        )
        .await
        .expect("probe cancellation must release its serialization guard");

        let retained = tokio::time::timeout(Duration::from_secs(2), bridge.authority())
            .await
            .expect("live authority should remain readable after probe cancellation");
        assert_eq!(
            retained, current,
            "dropping the probe must leave the current authority unchanged"
        );
        let retained_runtime = bridge.runtime().await;
        assert_eq!(
            retained_runtime.public_key(),
            current.keypair().public_key()
        );
        assert_eq!(config.registration_count_for_test().await, 1);

        let current_command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: current_supervisor.into(),
                epoch: current.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: "mob/inproc-cancel-restore".to_string(),
            },
        );
        let current_bridge = Arc::clone(&bridge);
        let current_recipient = remote.clone();
        let current_request = tokio::spawn(async move {
            current_bridge
                .send_bridge_command(&current_recipient, &current_command, Duration::from_secs(2))
                .await
        });
        let current_candidate = next_remote_candidate(&remote_runtime).await;
        reply_to_remote_candidate(
            &remote_runtime,
            current_candidate,
            serde_json::json!({"current": true}),
        )
        .await;
        assert_eq!(
            current_request
                .await
                .expect("current-authority request task should not panic")
                .expect("current-authority route should survive cancellation cleanup"),
            serde_json::json!({"current": true})
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ordinary_inproc_request_retains_live_route_until_response_before_rotation() {
        let suffix = uuid::Uuid::new_v4();
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &crate::MobId::from(format!("mob/inproc-request-rotation-{suffix}")),
                current.clone(),
                None,
            )
            .await
            .expect("private-listener supervisor bridge should build"),
        );
        let (remote_runtime, _remote_dsl, remote, _current_supervisor) =
            attach_inproc_remote(&bridge, "request-rotation").await;

        let requesting_bridge = Arc::clone(&bridge);
        let request_recipient = remote.clone();
        let request = tokio::spawn(async move {
            requesting_bridge
                .request_json(
                    &request_recipient,
                    super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    &serde_json::json!({"probe": "retain-inproc-callback"}),
                    Duration::from_secs(2),
                )
                .await
        });
        let candidate = next_remote_candidate(&remote_runtime).await;

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let rotating_bridge = Arc::clone(&bridge);
        let (rotation_started_tx, rotation_started_rx) = tokio::sync::oneshot::channel();
        let mut rotation = tokio::spawn(async move {
            let _ = rotation_started_tx.send(());
            rotating_bridge.rotate(next.clone()).await.map(|()| next)
        });
        rotation_started_rx
            .await
            .expect("concurrent rotation task should start");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut rotation)
                .await
                .is_err(),
            "an inproc request must retain the live registry owner until its response arrives"
        );

        reply_to_remote_candidate(
            &remote_runtime,
            candidate,
            serde_json::json!({"retained": true}),
        )
        .await;
        assert_eq!(
            request
                .await
                .expect("ordinary request task should not panic")
                .expect("ordinary inproc response should reach the pre-rotation runtime"),
            serde_json::json!({"retained": true})
        );
        let rotated = tokio::time::timeout(Duration::from_secs(2), rotation)
            .await
            .expect("rotation should proceed after response terminality")
            .expect("rotation task should not panic")
            .expect("rotation should succeed");
        assert_eq!(bridge.authority().await, rotated);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn prebuilt_rotation_candidate_does_not_evict_live_inproc_route_before_commit() {
        let suffix = uuid::Uuid::new_v4();
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &crate::MobId::from(format!("mob/inproc-prebuild-route-{suffix}")),
                current.clone(),
                None,
            )
            .await
            .expect("private-listener supervisor bridge should build"),
        );
        let current_runtime = bridge.runtime().await;
        let (remote_runtime, _remote_dsl, _remote, current_supervisor) =
            attach_inproc_remote(&bridge, "prebuild-route").await;

        let send_probe = |body: &'static str| {
            let remote_runtime = Arc::clone(&remote_runtime);
            let current_supervisor = current_supervisor.clone();
            async move {
                remote_runtime
                    .send(CommsCommand::PeerMessage {
                        objective_id: None,
                        to: PeerRoute::with_display_name(
                            current_supervisor.peer_id,
                            current_supervisor.name,
                        ),
                        body: body.to_string(),
                        blocks: None,
                        content_taint: None,
                        handling_mode: HandlingMode::Queue,
                    })
                    .await
            }
        };

        send_probe("before-prebuild")
            .await
            .expect("live inproc route should work before prebuild");
        assert_eq!(current_runtime.drain_peer_input_candidates().await.len(), 1);

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        // This is the prebuild seam used by `prepare_rotation`. Constructing
        // an uncommitted candidate must not mutate the process-global route
        // owned by the still-live authority.
        let uncommitted = bridge
            .prepare_live_runtime(&next)
            .await
            .expect("rotation candidate should prebuild");
        send_probe("while-prebuilt")
            .await
            .expect("uncommitted prebuild must not evict the live inproc route");
        assert_eq!(current_runtime.drain_peer_input_candidates().await.len(), 1);

        drop(uncommitted);
        send_probe("after-prebuild-drop")
            .await
            .expect("dropping an uncommitted prebuild must preserve the live inproc route");
        assert_eq!(current_runtime.drain_peer_input_candidates().await.len(), 1);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn rotation_preserves_machine_owned_direct_peer_projection() {
        let suffix = uuid::Uuid::new_v4();
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &crate::MobId::from(format!("mob/rotation-trust-replay-{suffix}")),
                current.clone(),
                None,
            )
            .await
            .expect("supervisor bridge should build"),
        );
        let (remote_runtime, _remote_dsl, _remote, _supervisor) =
            attach_inproc_remote(&bridge, "rotation-trust-replay").await;
        let expected = bridge
            .dsl
            .read()
            .await
            .snapshot_state()
            .direct_peer_endpoints;
        assert_eq!(expected.len(), 1);

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        bridge
            .rotate(next)
            .await
            .expect("rotation should preserve direct-peer trust");

        let replacement_dsl = bridge.dsl.read().await.clone();
        assert_eq!(
            replacement_dsl.snapshot_state().direct_peer_endpoints,
            expected,
            "rotation must preserve machine-owned direct-peer DSL truth"
        );
        let replacement_runtime = bridge.runtime().await;
        let trusted = CoreCommsRuntime::trusted_peer_projection_snapshot_for_source(
            replacement_runtime.as_ref(),
            meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
        )
        .await
        .expect("replacement runtime should expose generated trust projection");
        let trusted = trusted
            .iter()
            .map(mm_dsl::PeerEndpoint::from)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            trusted, expected,
            "rotation must reconcile the same direct-peer set onto the dormant runtime"
        );

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn shared_ingress_rotation_waits_for_old_authority_response() {
        let (bridge, config, current, remote_runtime, _remote_dsl, remote, _supervisor) =
            shared_bridge_with_remote("shared-inflight-rotation").await;
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let prepared = PreparedSupervisorBridgeRotation {
            authority: next.clone(),
            prebuilt: Some(
                bridge
                    .prepare_live_runtime(&next)
                    .await
                    .expect("replacement shared-ingress runtime should build"),
            ),
        };

        let requesting_bridge = Arc::clone(&bridge);
        let request_recipient = remote.clone();
        let request_task = tokio::spawn(async move {
            requesting_bridge
                .request_json(
                    &request_recipient,
                    super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    &serde_json::json!({"probe": "old-authority-callback"}),
                    Duration::from_secs(2),
                )
                .await
        });
        let candidate = next_remote_candidate(&remote_runtime).await;

        let committing_bridge = Arc::clone(&bridge);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut commit_task = tokio::spawn(async move {
            let _ = started_tx.send(());
            committing_bridge.commit_prepared_rotation(prepared).await
        });
        started_rx.await.expect("rotation commit task should start");
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut commit_task)
                .await
                .is_err(),
            "shared-ingress rotation must retain the old identity lease through response terminality"
        );
        assert_eq!(config.registration_count_for_test().await, 1);

        let reply_route = candidate.ingress.route.clone().or_else(|| {
            candidate
                .interaction
                .from_route
                .map(meerkat_core::PeerRoute::new)
        });
        remote_runtime
            .peer_interaction_handle()
            .expect("remote peer interaction authority")
            .request_received(
                meerkat_core::PeerCorrelationId::from_uuid(candidate.interaction.id.0),
                candidate.interaction.handling_mode,
            )
            .expect("record inbound request before sending its response");
        remote_runtime
            .send(CommsCommand::PeerResponse {
                to: reply_route.expect("request should carry an authenticated callback route"),
                in_reply_to: candidate.interaction.id,
                status: ResponseStatus::Completed,
                result: serde_json::json!({"ok": true}),
                blocks: None,
                content_taint: None,
                handling_mode: None,
                objective_id: candidate.interaction.objective_id,
            })
            .await
            .expect("old-authority response should traverse the retained shared lease");

        let response = tokio::time::timeout(Duration::from_secs(2), request_task)
            .await
            .expect("ordinary request should complete")
            .expect("ordinary request task should not panic")
            .expect("ordinary request should receive its response");
        assert_eq!(response, serde_json::json!({"ok": true}));
        tokio::time::timeout(Duration::from_secs(2), commit_task)
            .await
            .expect("rotation should complete after the response releases its guard")
            .expect("rotation commit task should not panic")
            .expect("rotation commit should succeed");
        assert_eq!(bridge.authority().await, next);
        assert_eq!(config.registration_count_for_test().await, 1);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn private_ephemeral_rotation_waits_for_buffered_response_terminality() {
        let (bridge, current, remote_runtime, _remote_dsl, remote, _supervisor) =
            ephemeral_private_bridge_with_remote("ephemeral-buffered-rotation").await;

        // Hold the response buffer so the live request can send, receive a
        // response into the shared intake, and then park before terminality.
        // Its authority read guard must remain held across this handoff.
        let mut buffered = bridge.buffered_candidates.lock().await;
        let requesting_bridge = Arc::clone(&bridge);
        let request_recipient = remote.clone();
        let request = tokio::spawn(async move {
            requesting_bridge
                .request_json(
                    &request_recipient,
                    super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    &serde_json::json!({"probe": "buffered-terminal-response"}),
                    Duration::from_secs(2),
                )
                .await
        });
        let delivered = next_remote_candidate(&remote_runtime).await;
        buffered.push_back(response_candidate_from(
            delivered.interaction.id.0,
            ResponseStatus::Completed,
            serde_json::json!({"buffered": true}),
            remote.peer_id,
            remote.name.as_str(),
        ));
        assert!(
            bridge.authority_gate.try_write().is_err(),
            "a request waiting on a buffered terminal response must retain its authority read guard"
        );

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let expected = next.clone();
        let rotating_bridge = Arc::clone(&bridge);
        let mut rotation = tokio::spawn(async move { rotating_bridge.rotate(next).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut rotation)
                .await
                .is_err(),
            "rotation must wait until the buffered response becomes terminal"
        );

        drop(buffered);
        bridge.intake_notify.notify_waiters();
        assert_eq!(
            request
                .await
                .expect("buffered request task should not panic")
                .expect("buffered response should complete the live request"),
            serde_json::json!({"buffered": true})
        );
        rotation
            .await
            .expect("rotation task should not panic")
            .expect("rotation should resume after response terminality");
        assert_eq!(bridge.authority().await, expected);

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn authority_rotation_wakes_intake_parked_on_retired_runtime_before_new_delivery() {
        let (bridge, config, current, remote_runtime, remote_dsl, remote, _old_supervisor) =
            shared_bridge_with_remote("shared-rotation-wakeup").await;
        let retired_runtime = bridge.runtime().await;
        let retired_inbox_notify = retired_runtime.inbox_notify();
        let intake_notify = bridge.intake_notify();
        let mut retired_inbox_wake = std::pin::pin!(retired_inbox_notify.notified());
        retired_inbox_wake.as_mut().enable();
        let mut intake_wake = std::pin::pin!(intake_notify.notified());
        intake_wake.as_mut().enable();

        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        bridge
            .rotate(next)
            .await
            .expect("shared supervisor rotation should publish");
        tokio::time::timeout(Duration::from_secs(1), intake_wake.as_mut())
            .await
            .expect("rotation publication must wake shared intake");
        assert!(
            tokio::time::timeout(Duration::from_millis(20), retired_inbox_wake.as_mut())
                .await
                .is_err(),
            "rotation wake must not depend on traffic reaching the retired inbox"
        );

        bridge
            .trust_recipient(&remote)
            .await
            .expect("replacement runtime should trust remote");
        let replacement_supervisor = bridge
            .supervisor_spec_for_recipient(&remote)
            .await
            .expect("replacement supervisor descriptor");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &remote_runtime,
            &remote_dsl,
            replacement_supervisor.clone(),
        )
        .await
        .expect("remote should trust replacement supervisor");
        let replacement_runtime = bridge.runtime().await;
        let replacement_notify = replacement_runtime.inbox_notify();
        let mut replacement_wake = std::pin::pin!(replacement_notify.notified());
        replacement_wake.as_mut().enable();
        remote_runtime
            .send(CommsCommand::PeerMessage {
                objective_id: None,
                to: PeerRoute::with_display_name(
                    replacement_supervisor.peer_id,
                    replacement_supervisor.name.clone(),
                ),
                body: "post-rotation-delivery".to_string(),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
            })
            .await
            .expect("post-rotation delivery should traverse shared ingress");
        tokio::time::timeout(Duration::from_secs(1), replacement_wake.as_mut())
            .await
            .expect("new runtime inbox should wake for post-rotation delivery");
        let candidates = replacement_runtime.drain_peer_input_candidates().await;
        assert!(candidates.iter().any(|candidate| matches!(
            &candidate.interaction.content,
            InteractionContent::Message { body, .. } if body == "post-rotation-delivery"
        )));

        bridge.shutdown().await;
        remote_runtime.stop_listeners_for_rebind().await;
        assert_eq!(config.registration_count_for_test().await, 0);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn supervisor_request_over_uds_fails_closed_before_send() {
        let authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let mob_id = crate::MobId::from(format!("mob/uds-request-{}", uuid::Uuid::new_v4()));
        let fixed_listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("reserve fixed supervisor bridge test port");
        let fixed_address = fixed_listener
            .local_addr()
            .expect("read fixed supervisor bridge test port");
        drop(fixed_listener);
        let bridge = Arc::new(
            MobSupervisorBridge::new(
                &mob_id,
                authority.clone(),
                Some(SupervisorBridgeEndpointConfig {
                    bind_address: Some(fixed_address.to_string()),
                    advertised_address: Some(format!("tcp://{fixed_address}")),
                }),
            )
            .await
            .expect("fixed-port supervisor bridge should build"),
        );
        let recipient_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let recipient = TrustedPeerDescriptor::unsigned_with_pubkey(
            "uds-recipient",
            recipient_authority.public_peer_id.clone(),
            *recipient_authority.keypair().public_key().as_bytes(),
            format!("uds:///tmp/meerkat-uds-{}.sock", uuid::Uuid::new_v4()),
        )
        .expect("UDS recipient descriptor should be valid");
        let runtime = bridge.runtime().await;

        let supervisor = bridge
            .supervisor_spec_for_authority_and_recipient(&authority, &recipient)
            .await
            .expect("supervisor spec should resolve before the request");
        let command = super::super::bridge_protocol::BridgeCommand::HostStatus(
            super::super::bridge_protocol::BridgeHostStatusPayload {
                supervisor: supervisor.into(),
                epoch: authority.epoch,
                binding_generation: 1,
                protocol_version: super::super::bridge_protocol::BridgeProtocolVersion::V4,
                mob_id: mob_id.to_string(),
            },
        );
        let original_authority = bridge.authority().await;
        let original_direct_peers = bridge
            .dsl
            .read()
            .await
            .snapshot_state()
            .direct_peer_endpoints;
        let original_live_peers = runtime.peers().await;

        let admission_error = bridge
            .require_supported_rotation_endpoint(PeerTransport::Uds)
            .expect_err("fixed callback configuration must not admit UDS rotation targets");
        assert!(
            matches!(&admission_error, MobError::WiringError(message) if message.contains("request/response over UDS is unsupported")),
            "unexpected UDS rotation admission failure: {admission_error:?}"
        );

        for request_authority in [
            authority.clone(),
            SupervisorAuthorityRecord::generate(
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
            ),
        ] {
            let error = bridge
                .send_bridge_command_as_authority_classified(
                    &request_authority,
                    &recipient,
                    &command,
                    Duration::from_millis(40),
                )
                .await
                .expect_err("UDS command must fail before current or alternate authority work");
            assert!(
                matches!(&error, BridgeRequestFailure::BeforeSend(MobError::WiringError(message)) if message.contains("request/response over UDS is unsupported")),
                "unexpected classified UDS failure: {error:?}"
            );
        }
        assert_eq!(bridge.authority().await, original_authority);
        assert!(Arc::ptr_eq(&runtime, &bridge.runtime().await));
        assert_eq!(
            bridge
                .dsl
                .read()
                .await
                .snapshot_state()
                .direct_peer_endpoints,
            original_direct_peers,
        );
        assert_eq!(runtime.peers().await, original_live_peers);

        let error = bridge
            .send_supervisor_request(
                &runtime,
                &recipient,
                super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                &serde_json::json!({"probe": "uds"}),
                SupervisorRequestReplyEndpoint::Live,
            )
            .await
            .expect_err("UDS request/response must fail closed");
        assert!(
            matches!(&error, MobError::WiringError(message) if message.contains("request/response over UDS is unsupported")),
            "unexpected UDS failure: {error}"
        );

        bridge.shutdown().await;
    }

    #[tokio::test]
    async fn supervisor_terminal_response_cleans_pending_request_via_authority() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (runtime, _dsl) = MobSupervisorBridge::build_runtime(
            "mob/__mob_supervisor__/terminal-test",
            &authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("supervisor runtime should build");
        let request_envelope_id = uuid::Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(request_envelope_id);
        let handle = runtime
            .peer_interaction_handle()
            .expect("supervisor runtime installs peer interaction authority");
        handle
            .request_sent(corr_id)
            .expect("request should be recorded by generated authority");

        MobSupervisorBridge::record_response_terminal(
            &runtime,
            request_envelope_id,
            meerkat_core::handles::PeerTerminalDisposition::Completed,
        )
        .expect("terminal response should be recorded by generated authority");

        assert_eq!(
            handle.outbound_state(corr_id),
            None,
            "generated terminal transition should clean up pending supervisor request truth"
        );
    }

    #[tokio::test]
    async fn supervisor_request_timeout_cleans_pending_request_via_authority() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (runtime, _dsl) = MobSupervisorBridge::build_runtime(
            "mob/__mob_supervisor__/timeout-test",
            &authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("supervisor runtime should build");
        let request_envelope_id = uuid::Uuid::new_v4();
        let corr_id = meerkat_core::PeerCorrelationId::from_uuid(request_envelope_id);
        let handle = runtime
            .peer_interaction_handle()
            .expect("supervisor runtime installs peer interaction authority");
        handle
            .request_sent(corr_id)
            .expect("request should be recorded by generated authority");

        MobSupervisorBridge::record_request_timed_out(&runtime, request_envelope_id)
            .expect("timeout should be recorded by generated authority");

        assert_eq!(
            handle.outbound_state(corr_id),
            None,
            "generated timeout transition should clean up pending supervisor request truth"
        );
    }

    #[tokio::test]
    async fn supervisor_rotation_submission_is_a_one_way_typed_peer_message() {
        let suffix = uuid::Uuid::new_v4();
        let authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = MobSupervisorBridge::new(
            &crate::MobId::from(format!("mob/rotation-delivery-{suffix}")),
            authority,
            None,
        )
        .await
        .expect("supervisor bridge should build");
        let recipient_name = format!("rotation-delivery-recipient-{suffix}");
        let recipient_authority = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let (recipient_runtime, recipient_dsl) = MobSupervisorBridge::build_runtime(
            &recipient_name,
            &recipient_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("recipient runtime should build");
        let recipient = TrustedPeerDescriptor::unsigned_with_pubkey(
            recipient_name,
            recipient_runtime.public_key().to_peer_id().to_string(),
            *recipient_runtime.public_key().as_bytes(),
            recipient_runtime.advertised_address(),
        )
        .expect("recipient descriptor");
        bridge
            .trust_recipient(&recipient)
            .await
            .expect("sender trusts recipient");
        let sender = bridge
            .supervisor_spec_for_recipient(&recipient)
            .await
            .expect("sender descriptor");
        MobSupervisorBridge::apply_bridge_trust(
            &bridge.trust_reconcile_gate,
            &recipient_runtime,
            &recipient_dsl,
            sender.clone(),
        )
        .await
        .expect("recipient trusts sender");

        let operation_id = super::super::bridge_protocol::SupervisorRotationOperationId::new();
        let delivery =
            super::super::bridge_protocol::BridgeSupervisorDelivery::SubmitSupervisorRotation(
                super::super::bridge_protocol::BridgeSupervisorRotationSubmit {
                    operation_id,
                    target: sender.into(),
                    target_epoch: 1,
                    protocol_version:
                        super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
                },
            );
        bridge
            .send_supervisor_delivery(&recipient, &delivery)
            .await
            .expect("one-way delivery should send");

        let candidates = recipient_runtime.drain_peer_input_candidates().await;
        let candidate = candidates
            .into_iter()
            .next()
            .expect("recipient should receive one delivery");
        let InteractionContent::Message { body, .. } = candidate.interaction.content else {
            panic!("rotation submission must not create request/response authority");
        };
        let decoded: super::super::bridge_protocol::BridgeSupervisorDelivery =
            serde_json::from_str(&body).expect("typed delivery body");
        assert_eq!(decoded, delivery);
    }

    #[tokio::test]
    async fn prepared_rotation_commit_waits_for_in_flight_send_guard() {
        let suffix = uuid::Uuid::new_v4();
        let mob_id = crate::MobId::from(format!("mob/rotation-commit-gate-{suffix}"));
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(&mob_id, current.clone(), None)
                .await
                .expect("supervisor bridge should build"),
        );
        let mut next = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        next.epoch = current.epoch + 1;
        let prebuilt = bridge
            .prepare_live_runtime(&next)
            .await
            .expect("replacement supervisor runtime should build");
        let prepared = PreparedSupervisorBridgeRotation {
            authority: next.clone(),
            prebuilt: Some(prebuilt),
        };

        // Ordinary sends hold this read guard across the transport send. A
        // prepared commit must not expose any part of the replacement bridge
        // identity until that in-flight send leaves the old authority.
        let in_flight_send_guard = bridge.authority_gate.read().await;
        let committing_bridge = Arc::clone(&bridge);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut commit_task = tokio::spawn(async move {
            let _ = started_tx.send(());
            committing_bridge.commit_prepared_rotation(prepared).await
        });
        started_rx
            .await
            .expect("prepared rotation commit task should start");

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut commit_task)
                .await
                .is_err(),
            "prepared rotation commit must wait for the in-flight send guard"
        );
        assert_eq!(
            bridge.authority_with_gate_held(),
            current,
            "the live authority must remain unchanged while an old-authority send is in flight"
        );

        drop(in_flight_send_guard);
        tokio::time::timeout(Duration::from_secs(2), commit_task)
            .await
            .expect("prepared rotation commit should finish after the send guard is released")
            .expect("prepared rotation commit task should not panic")
            .expect("prepared rotation commit should succeed");
        assert_eq!(bridge.authority().await, next);
    }

    #[tokio::test]
    async fn public_authority_read_hides_transient_alternate_authority_swap() {
        let suffix = uuid::Uuid::new_v4();
        let mob_id = crate::MobId::from(format!("mob/authority-read-gate-{suffix}"));
        let current = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        let bridge = Arc::new(
            MobSupervisorBridge::new(&mob_id, current.clone(), None)
                .await
                .expect("supervisor bridge should build"),
        );
        let mut transient = SupervisorAuthorityRecord::generate(
            super::super::bridge_protocol::SUPERVISOR_BRIDGE_PROTOCOL_VERSION,
        );
        transient.epoch = current.epoch + 1;

        // A fixed-port alternate-authority send temporarily replaces the
        // shared authority/runtime/DSL while holding the write side of this
        // gate. Public readers must wait for restoration rather than observe
        // that probe identity as the durable live bridge identity.
        let alternate_send_guard = bridge.authority_gate.write().await;
        *bridge
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = transient;

        let reading_bridge = Arc::clone(&bridge);
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let mut read_task = tokio::spawn(async move {
            let _ = started_tx.send(());
            reading_bridge.authority().await
        });
        started_rx
            .await
            .expect("public authority read task should start");

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut read_task)
                .await
                .is_err(),
            "public authority read must wait for the alternate-authority swap to restore"
        );

        *bridge
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = current.clone();
        drop(alternate_send_guard);

        let observed = tokio::time::timeout(Duration::from_secs(2), read_task)
            .await
            .expect("public authority read should finish after restoration")
            .expect("public authority read task should not panic");
        assert_eq!(observed, current);
    }

    #[tokio::test]
    async fn bridge_trust_reconcile_serializes_concurrent_projection_epochs() {
        let suffix = uuid::Uuid::new_v4();
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (runtime, dsl) = MobSupervisorBridge::build_runtime(
            &format!("mob/__mob_supervisor__/trust-serialization-{suffix}"),
            &authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("supervisor runtime should build");
        let recipient = |label: &str| {
            let keypair = meerkat_comms::Keypair::generate();
            TrustedPeerDescriptor::unsigned_with_pubkey(
                format!("trust-serialization-{label}-{suffix}"),
                keypair.public_key().to_peer_id().to_string(),
                *keypair.public_key().as_bytes(),
                format!("inproc://trust-serialization-{label}-{suffix}"),
            )
            .expect("valid recipient descriptor")
        };
        let first_recipient = recipient("first");
        let second_recipient = recipient("second");
        let expected_peer_ids = [first_recipient.peer_id, second_recipient.peer_id];
        let trust_reconcile_gate = Arc::new(Mutex::new(()));

        // Queue both callers behind the same held gate. Releasing it makes
        // them runnable together while the bridge-owned gate keeps each
        // projection transition coupled to its awaited reconciliation tail.
        let held = trust_reconcile_gate.lock().await;
        let first = {
            let gate = Arc::clone(&trust_reconcile_gate);
            let runtime = Arc::clone(&runtime);
            let dsl = Arc::clone(&dsl);
            tokio::spawn(async move {
                MobSupervisorBridge::apply_bridge_trust(
                    gate.as_ref(),
                    &runtime,
                    &dsl,
                    first_recipient,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        let second = {
            let gate = Arc::clone(&trust_reconcile_gate);
            let runtime = Arc::clone(&runtime);
            let dsl = Arc::clone(&dsl);
            tokio::spawn(async move {
                MobSupervisorBridge::apply_bridge_trust(
                    gate.as_ref(),
                    &runtime,
                    &dsl,
                    second_recipient,
                )
                .await
            })
        };
        tokio::task::yield_now().await;
        drop(held);

        first
            .await
            .expect("first trust task should join")
            .expect("first trust projection should reconcile");
        second
            .await
            .expect("second trust task should join")
            .expect("second trust projection should reconcile");

        let trusted_peer_ids = runtime
            .trusted_peers_shared()
            .entries()
            .into_iter()
            .map(|peer| peer.peer_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(
            expected_peer_ids
                .into_iter()
                .all(|peer_id| trusted_peer_ids.contains(&peer_id)),
            "both serialized projection epochs must reach canonical trust: {trusted_peer_ids:?}",
        );
    }

    #[tokio::test]
    async fn bridge_trust_reconcile_repair_heals_committed_dsl_windows() {
        let suffix = uuid::Uuid::new_v4();
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (runtime, dsl) = MobSupervisorBridge::build_runtime(
            &format!("mob/__mob_supervisor__/trust-repair-{suffix}"),
            &authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("supervisor runtime should build");
        let recipient_name = format!("trust-repair-recipient-{suffix}");
        let recipient_authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let (recipient_runtime, _recipient_dsl) = MobSupervisorBridge::build_runtime(
            &recipient_name,
            &recipient_authority,
            &SupervisorBridgeEndpointConfig::default(),
        )
        .await
        .expect("recipient runtime should build");
        let recipient = TrustedPeerDescriptor::unsigned_with_pubkey(
            recipient_name,
            recipient_runtime.public_key().to_peer_id().to_string(),
            *recipient_runtime.public_key().as_bytes(),
            recipient_runtime.advertised_address(),
        )
        .expect("valid recipient descriptor");
        let trust_reconcile_gate = Mutex::new(());

        fail_next_bridge_trust_add_reconcile_for_test(&recipient.peer_id.to_string());
        MobSupervisorBridge::apply_bridge_trust(
            &trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
        .expect_err("fault after AddDirectPeerEndpoint must surface");
        assert!(
            dsl.snapshot_state()
                .direct_peer_endpoints
                .contains(&mm_dsl::PeerEndpoint::from(&recipient)),
            "the fault must exercise DSL-committed/live-unreconciled add state"
        );
        assert_eq!(
            MobSupervisorBridge::apply_bridge_trust(
                &trust_reconcile_gate,
                &runtime,
                &dsl,
                recipient.clone(),
            )
            .await
            .expect("RepairAddDirectPeerEndpoint must reconcile live trust"),
            RecipientTrustInstall::NewlyInstalled
        );

        let add_probe = runtime
            .send(CommsCommand::PeerRequest {
                objective_id: None,
                to: PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone()),
                intent: super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({"probe": "trust-added"}),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
                stream: InputStreamMode::None,
            })
            .await;
        assert!(
            matches!(add_probe, Ok(SendReceipt::PeerRequestSent { .. })),
            "repair-add must install the live router row, got {add_probe:?}"
        );

        fail_next_bridge_trust_remove_reconcile_for_test(&recipient.peer_id.to_string());
        MobSupervisorBridge::remove_bridge_trust(
            &trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
        .expect_err("fault after RemoveDirectPeerEndpoint must surface");
        assert!(
            !dsl.snapshot_state()
                .direct_peer_endpoints
                .contains(&mm_dsl::PeerEndpoint::from(&recipient)),
            "the fault must exercise DSL-committed/live-unreconciled remove state"
        );
        MobSupervisorBridge::remove_bridge_trust(
            &trust_reconcile_gate,
            &runtime,
            &dsl,
            recipient.clone(),
        )
        .await
        .expect("RepairRemoveDirectPeerEndpoint must remove stranded live trust");

        let send_result = runtime
            .send(CommsCommand::PeerRequest {
                objective_id: None,
                to: PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone()),
                intent: super::super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT.to_string(),
                params: serde_json::json!({"probe": "trust-removed"}),
                blocks: None,
                content_taint: None,
                handling_mode: HandlingMode::Queue,
                stream: InputStreamMode::None,
            })
            .await;
        assert!(
            matches!(
                send_result,
                Err(meerkat_core::comms::SendError::PeerNotFound(_))
            ),
            "repair-remove must remove the live router row, got {send_result:?}"
        );
    }

    #[test]
    fn response_value_completed_returns_payload() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let expected = serde_json::json!({"ok": true});
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Completed,
            expected.clone(),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("completed response should parse");

        assert_eq!(value, Some(expected));
    }

    #[test]
    fn response_value_accepted_is_not_terminal() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Accepted,
            serde_json::json!({"phase": "queued"}),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("accepted response should parse");

        assert!(
            value.is_none(),
            "Accepted is progress and must not satisfy wait_for_response"
        );
    }

    #[test]
    fn response_value_failed_returns_payload() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let expected = serde_json::json!({"error": "boom"});
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Failed,
            expected.clone(),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("failed response should parse");

        assert_eq!(
            value,
            Some(expected),
            "Failed is terminal and must surface the result payload"
        );
    }

    #[test]
    fn response_value_routes_all_statuses_through_machine_terminality() {
        for status in [
            ResponseStatus::Accepted,
            ResponseStatus::Completed,
            ResponseStatus::Failed,
        ] {
            let request_envelope_id = uuid::Uuid::new_v4();
            let payload = serde_json::json!({"status": format!("{status:?}")});
            let candidate = response_candidate(request_envelope_id, status, payload.clone());

            let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
                .expect("status-response should parse");

            match candidate
                .response_terminality
                .expect("generated peer-comms authority should classify response terminality")
            {
                meerkat_core::TerminalityClass::Terminal { .. } => assert_eq!(
                    value,
                    Some(payload),
                    "terminal classification must surface payload (status={status:?})"
                ),
                meerkat_core::TerminalityClass::Progress => assert!(
                    value.is_none(),
                    "progress classification must not surface payload (status={status:?})"
                ),
                _ => panic!(
                    "TerminalityClass variant grew; update supervisor_bridge::response_value \
                     and this test before landing the new class"
                ),
            }
        }
    }

    #[test]
    fn response_value_missing_machine_terminality_fails_closed() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let mut candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Completed,
            serde_json::json!({"ok": true}),
        );
        candidate.response_terminality = None;

        let result = MobSupervisorBridge::response_value(&candidate, request_envelope_id);

        assert!(
            matches!(result, Err(MobError::Internal(ref message)) if message.contains("without machine terminality")),
            "missing terminality must not be treated as no response: {result:?}"
        );
    }

    #[test]
    fn response_value_for_unrelated_envelope_returns_none() {
        let awaited = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        assert_ne!(awaited, other);
        let candidate = response_candidate(
            other,
            ResponseStatus::Completed,
            serde_json::json!({"ok": true}),
        );

        let value = MobSupervisorBridge::response_value(&candidate, awaited)
            .expect("mismatched envelope should still parse");

        assert!(
            value.is_none(),
            "response for a different envelope must not satisfy the current wait"
        );
    }

    fn recipient_with_transport(
        name: &str,
        transport: PeerTransport,
        endpoint: &str,
    ) -> TrustedPeerDescriptor {
        let mut pubkey = [0u8; 32];
        for (index, byte) in name.bytes().enumerate() {
            let slot = index % pubkey.len();
            pubkey[slot] = pubkey[slot].wrapping_add(byte).wrapping_add(index as u8);
        }
        if pubkey == [0u8; 32] {
            pubkey[0] = 1;
        }
        let address = PeerAddress::new(transport, endpoint);
        TrustedPeerDescriptor::unsigned_with_pubkey(
            name,
            PeerId::from_ed25519_pubkey(&pubkey).to_string(),
            pubkey,
            address.to_string(),
        )
        .expect("valid non-zero test recipient descriptor")
    }

    #[tokio::test]
    async fn supervisor_self_address_is_invariant_across_recipients_within_a_transport() {
        let authority = SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let bridge = MobSupervisorBridge::new(
            &crate::MobId::from("mob/self-address-test"),
            authority,
            None,
        )
        .await
        .expect("supervisor bridge should build");

        // Two distinct inproc recipients must receive the SAME self-address:
        // the advertised address is the bridge's identity-scoped reachability
        // fact, not a function of who is asking.
        let inproc_a = recipient_with_transport("member-a", PeerTransport::Inproc, "member-a");
        let inproc_b = recipient_with_transport("member-b", PeerTransport::Inproc, "member-b");
        let spec_a = bridge
            .supervisor_spec_for_recipient(&inproc_a)
            .await
            .expect("inproc recipient a self-address");
        let spec_b = bridge
            .supervisor_spec_for_recipient(&inproc_b)
            .await
            .expect("inproc recipient b self-address");
        assert_eq!(
            spec_a.address, spec_b.address,
            "inproc self-address must not depend on which recipient is asking"
        );

        // Inproc loopback must still route through the inproc registry by the
        // bridge participant name (the historical loopback contract).
        assert_eq!(
            spec_a.address.transport(),
            PeerTransport::Inproc,
            "inproc recipients must receive an inproc loopback self-address"
        );
        assert_eq!(
            spec_a.address.to_string(),
            format!("inproc://{}/__mob_supervisor__", "mob/self-address-test"),
            "inproc loopback must route to the supervisor bridge participant name"
        );

        // Two distinct TCP recipients must also receive the SAME self-address,
        // and it must be the routable network address — a different reachability
        // domain than inproc loopback.
        let tcp_a = recipient_with_transport("remote-a", PeerTransport::Tcp, "10.0.0.1:9000");
        let tcp_b = recipient_with_transport("remote-b", PeerTransport::Tcp, "10.0.0.2:9000");
        let routable_a = bridge
            .supervisor_spec_for_recipient(&tcp_a)
            .await
            .expect("tcp recipient a self-address");
        let routable_b = bridge
            .supervisor_spec_for_recipient(&tcp_b)
            .await
            .expect("tcp recipient b self-address");
        assert_eq!(
            routable_a.address, routable_b.address,
            "routable self-address must not depend on which recipient is asking"
        );
        assert_ne!(
            routable_a.address, spec_a.address,
            "inproc loopback and routable network are distinct reachability domains"
        );

        // The bridge identity (peer id, public key, name) is constant across
        // every recipient and reachability domain.
        assert_eq!(spec_a.peer_id, routable_a.peer_id);
        assert_eq!(spec_a.pubkey, routable_a.pubkey);
        assert_eq!(spec_a.name, routable_a.name);
    }

    #[test]
    fn bridge_request_deadline_saturates_elapsed_timeout() {
        assert_eq!(
            BridgeRequestDeadline::remaining_after(
                Duration::from_millis(25),
                Duration::from_millis(30)
            ),
            Duration::ZERO
        );
        assert_eq!(
            BridgeRequestDeadline::remaining_after(
                Duration::from_millis(25),
                Duration::from_millis(10)
            ),
            Duration::from_millis(15)
        );
    }
}
