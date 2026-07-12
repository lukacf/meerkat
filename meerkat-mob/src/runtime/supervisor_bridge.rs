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
    fn error(&self) -> &MobError {
        match self {
            Self::BeforeSend(error) | Self::AfterSend(error) => error,
        }
    }

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
    /// Ordinary request sends hold the READ guard across the SEND only — the
    /// response wait consumes the shared intake buffer and the captured
    /// runtime's inbox, no gate-guarded state — so with tokio's
    /// write-preferring `RwLock` a pending rotation waits at most the
    /// in-flight sends, never a full `wait_for_response` window (ADJ-P6-2:
    /// rotation waits ≤ one poll window).
    authority_gate: RwLock<()>,
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
/// Native bridge requests must always name a signed TCP callback endpoint:
/// either the live runtime's routable endpoint or the distinct listener owned
/// by an ephemeral alternate-authority probe. Keeping this typed (rather than
/// `Option<PeerAddress>`) makes accidentally omitting callback authority from
/// an ordinary native request unrepresentable. WASM has no TCP callback
/// transport, so its only variant retains the in-process request path.
enum SupervisorRequestReplyEndpoint {
    Live,
    #[cfg(not(target_arch = "wasm32"))]
    EphemeralProbe(PeerAddress),
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
    prebuilt: Option<(
        Arc<meerkat_comms::CommsRuntime>,
        Arc<meerkat_runtime::HandleDslAuthority>,
    )>,
}

impl MobSupervisorBridge {
    pub(crate) async fn new(
        mob_id: &crate::MobId,
        authority: SupervisorAuthorityRecord,
        endpoint_config: Option<SupervisorBridgeEndpointConfig>,
    ) -> Result<Self, MobError> {
        let participant_name = format!("{mob_id}/__mob_supervisor__");
        let endpoint_config = endpoint_config.unwrap_or_default();
        let (runtime, dsl) =
            Self::build_runtime(&participant_name, &authority, &endpoint_config).await?;
        Ok(Self {
            participant_name,
            endpoint_config,
            authority: StdRwLock::new(authority),
            runtime: RwLock::new(runtime),
            dsl: RwLock::new(dsl),
            buffered_candidates: Mutex::new(VecDeque::new()),
            intake_notify: Arc::new(tokio::sync::Notify::new()),
            authority_gate: RwLock::new(()),
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
        let runtime = meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
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
            let mut runtime = runtime;
            let bind_address = Self::supervisor_bind_address(endpoint_config)?;
            runtime
                .set_listen_tcp_for_unstarted_runtime(bind_address)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "failed to configure mob supervisor comms listener '{participant_name}': {error}"
                    ))
                })?;
            if let Some(advertised_address) = endpoint_config.advertised_address.clone() {
                runtime
                    .set_advertise_address_for_unstarted_runtime(advertised_address)
                    .map_err(|error| {
                        MobError::Internal(format!(
                            "failed to configure mob supervisor advertised address '{participant_name}': {error}"
                        ))
                    })?;
            }
            runtime.start_listeners().await.map_err(|error| {
                MobError::Internal(format!(
                    "failed to start mob supervisor comms listener '{participant_name}': {error}"
                ))
            })?;
            let runtime = Arc::new(runtime);
            let dsl = Self::install_supervisor_peer_request_response_authority(
                &runtime,
                participant_name,
            )?;
            Ok((runtime, dsl))
        }
        #[cfg(target_arch = "wasm32")]
        {
            let runtime = Arc::new(runtime);
            let dsl = Self::install_supervisor_peer_request_response_authority(
                &runtime,
                participant_name,
            )?;
            Ok((runtime, dsl))
        }
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
    fn install_supervisor_peer_request_response_authority(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        participant_name: &str,
    ) -> Result<Arc<meerkat_runtime::HandleDslAuthority>, MobError> {
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
            runtime.as_ref(),
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to install supervisor bridge peer-comms authority '{participant_name}': {error}"
            ))
        })?;
        runtime.require_peer_comms_machine_authority();
        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                    Arc::clone(&dsl),
                )),
                Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(
                    Arc::clone(&dsl),
                )),
            ),
        );
        Ok(dsl)
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
            Some(
                Self::build_runtime(&self.participant_name, &authority, &self.endpoint_config)
                    .await?,
            )
        };
        Ok(PreparedSupervisorBridgeRotation {
            authority,
            prebuilt,
        })
    }

    pub(crate) async fn commit_prepared_rotation(
        &self,
        prepared: PreparedSupervisorBridgeRotation,
    ) -> Result<(), MobError> {
        // A prepared commit swaps the signing authority, runtime, and DSL as
        // one live bridge identity. Ordinary sends hold the read side of this
        // gate across their transport send, so take the write side before any
        // part of the swap becomes visible. Keep the mechanics in the locked
        // helper below: `rotate()` already owns this write guard and must not
        // recursively acquire it.
        let _authority_guard = self.authority_gate.write().await;
        self.commit_prepared_rotation_locked(prepared).await
    }

    /// Commit a prepared rotation while the caller holds
    /// `authority_gate.write()`.
    async fn commit_prepared_rotation_locked(
        &self,
        prepared: PreparedSupervisorBridgeRotation,
    ) -> Result<(), MobError> {
        let mut runtime_guard = self.runtime.write().await;
        let (runtime, dsl) = match prepared.prebuilt {
            Some(prebuilt) => prebuilt,
            None => {
                #[cfg(not(target_arch = "wasm32"))]
                runtime_guard.stop_listeners_for_rebind().await;
                #[cfg(test)]
                if FAIL_NEXT_FIXED_PORT_ROTATION_REBUILD
                    .swap(false, std::sync::atomic::Ordering::Relaxed)
                {
                    return Err(MobError::Internal(
                        "fault-injected fixed-port supervisor bridge rebuild failure".to_string(),
                    ));
                }
                Self::build_runtime(
                    &self.participant_name,
                    &prepared.authority,
                    &self.endpoint_config,
                )
                .await?
            }
        };
        *self
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prepared.authority;
        *runtime_guard = runtime;
        *self.dsl.write().await = dsl;
        self.buffered_candidates.lock().await.clear();
        Ok(())
    }

    pub(crate) async fn rotate(
        &self,
        authority: SupervisorAuthorityRecord,
    ) -> Result<(), MobError> {
        // WRITE guard (DEC-P6E-14): the durable authority swap excludes
        // every in-flight send; write-preference bounds the wait by the
        // longest outstanding read-guarded round-trip.
        let _authority_guard = self.authority_gate.write().await;
        self.replace_runtime_authority_locked(authority, true).await
    }

    pub(crate) async fn shutdown(&self) {
        let _authority_guard = self.authority_gate.write().await;
        #[cfg(not(target_arch = "wasm32"))]
        {
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
        let mut runtime_guard = self.runtime.write().await;
        #[cfg(not(target_arch = "wasm32"))]
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            runtime_guard.stop_listeners_for_rebind().await;
        }
        let (runtime, dsl) =
            Self::build_runtime(&self.participant_name, &authority, &self.endpoint_config).await?;
        *self
            .authority
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        *runtime_guard = runtime;
        *self.dsl.write().await = dsl;
        if clear_buffered_candidates {
            self.buffered_candidates.lock().await.clear();
        }
        Ok(())
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
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        dsl: &Arc<meerkat_runtime::HandleDslAuthority>,
        recipient: TrustedPeerDescriptor,
    ) -> Result<RecipientTrustInstall, MobError> {
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
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        dsl: &Arc<meerkat_runtime::HandleDslAuthority>,
        recipient: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
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
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            let _authority_guard = self.authority_gate.write().await;
            let previous_authority = self.authority_with_gate_held();
            let restore_after_send = previous_authority != *authority;
            if restore_after_send {
                self.replace_runtime_authority_locked(authority.clone(), false)
                    .await?;
            }
            let send_result = async {
                let runtime = self.runtime_with_gate_held().await;
                let dsl = self.dsl.read().await.clone();
                let _install = Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
                Self::send_supervisor_delivery_with_runtime(&runtime, recipient, delivery).await
            }
            .await;
            if restore_after_send
                && let Err(restore_error) = self
                    .replace_runtime_authority_locked(previous_authority, false)
                    .await
            {
                return match send_result {
                    Ok(()) => Err(restore_error),
                    Err(send_error) => Err(MobError::Internal(format!(
                        "alternate-authority supervisor delivery failed: {send_error}; additionally failed to restore supervisor bridge authority: {restore_error}"
                    ))),
                };
            }
            return send_result;
        }
        let _authority_guard = self.authority_gate.read().await;
        let probe_participant_name = format!(
            "{}/pending-supervisor-delivery/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let (runtime, dsl) =
            Self::build_runtime(&probe_participant_name, authority, &self.endpoint_config).await?;
        let _install = Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
        Self::send_supervisor_delivery_with_runtime(&runtime, recipient, delivery).await
    }

    pub(crate) async fn send_bridge_command_as_authority(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            // WRITE guard: the fixed-port authority swap must exclude every
            // concurrent send (read-guarded) for the swap/restore window.
            let _authority_guard = self.authority_gate.write().await;
            let previous_authority = self.authority_with_gate_held();
            let restore_after_send = previous_authority != *authority;
            if restore_after_send {
                self.replace_runtime_authority_locked(authority.clone(), false)
                    .await?;
            }
            let send_result = async {
                let runtime = self.runtime_with_gate_held().await;
                // Route the recipient trust through the generated machine
                // authority (DSL), not a raw comms add_trusted_peer, so the
                // trust write is revalidated by canonical authority under the
                // swapped rotation authority — matching the non-fixed-port
                // branch below and trust_recipient (Invariant #1: no raw or
                // legacy authority writes).
                let dsl = self.dsl.read().await.clone();
                let _install = Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
                self.request_json_with_runtime(
                    &runtime,
                    recipient,
                    super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    command,
                    SupervisorRequestReplyEndpoint::Live,
                    timeout,
                )
                .await
            }
            .await;
            if restore_after_send {
                if let Err(restore_error) = self
                    .replace_runtime_authority_locked(previous_authority, false)
                    .await
                {
                    return match send_result {
                        Ok(_) => Err(restore_error),
                        Err(send_error) => Err(MobError::Internal(format!(
                            "alternate supervisor authority bridge request failed: {send_error}; \
                             additionally failed to restore supervisor bridge authority: {restore_error}"
                        ))),
                    };
                }
            }
            return send_result;
        }
        // READ guard: the throwaway probe runtime never touches the shared
        // authority, so it only needs to stay clear of an in-flight swap.
        let _authority_guard = self.authority_gate.read().await;
        let probe_participant_name = format!(
            "{}/pending-supervisor-probe/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let (runtime, dsl) =
            Self::build_runtime(&probe_participant_name, authority, &self.endpoint_config).await?;
        let _install = Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
        #[cfg(not(target_arch = "wasm32"))]
        let reply_endpoint = Self::ephemeral_probe_reply_endpoint(&runtime, &self.endpoint_config)?;
        #[cfg(not(target_arch = "wasm32"))]
        let reply_endpoint = SupervisorRequestReplyEndpoint::EphemeralProbe(reply_endpoint);
        #[cfg(target_arch = "wasm32")]
        let reply_endpoint = SupervisorRequestReplyEndpoint::Live;
        self.request_json_with_runtime(
            &runtime,
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            reply_endpoint,
            timeout,
        )
        .await
    }

    /// Alternate-authority request preserving the transport send boundary.
    /// Supervisor rotation uses this for host rebind so a timeout after the
    /// request receipt cannot be mistaken for a certified pre-send failure.
    pub(crate) async fn send_bridge_command_as_authority_classified(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeRequestFailure> {
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)
            .map_err(BridgeRequestFailure::BeforeSend)?
        {
            let _authority_guard = self.authority_gate.write().await;
            let previous_authority = self.authority_with_gate_held();
            let restore_after_send = previous_authority != *authority;
            if restore_after_send {
                self.replace_runtime_authority_locked(authority.clone(), false)
                    .await
                    .map_err(BridgeRequestFailure::BeforeSend)?;
            }
            let send_result = async {
                let runtime = self.runtime_with_gate_held().await;
                let dsl = self.dsl.read().await.clone();
                Self::apply_bridge_trust(&runtime, &dsl, recipient.clone())
                    .await
                    .map_err(BridgeRequestFailure::BeforeSend)?;
                self.request_json_with_runtime_classified(
                    &runtime,
                    recipient,
                    super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    command,
                    SupervisorRequestReplyEndpoint::Live,
                    timeout,
                )
                .await
            }
            .await;
            if restore_after_send
                && let Err(restore_error) = self
                    .replace_runtime_authority_locked(previous_authority, false)
                    .await
            {
                let error = match &send_result {
                    Ok(_) => MobError::Internal(format!(
                        "alternate supervisor authority request completed but restoring bridge authority failed: {restore_error}"
                    )),
                    Err(failure) => MobError::Internal(format!(
                        "alternate supervisor authority request failed ({}); additionally failed to restore bridge authority: {restore_error}",
                        failure.error()
                    )),
                };
                return match send_result {
                    Err(BridgeRequestFailure::BeforeSend(_)) => {
                        Err(BridgeRequestFailure::BeforeSend(error))
                    }
                    Ok(_) | Err(BridgeRequestFailure::AfterSend(_)) => {
                        Err(BridgeRequestFailure::AfterSend(error))
                    }
                };
            }
            return send_result;
        }

        let _authority_guard = self.authority_gate.read().await;
        let probe_participant_name = format!(
            "{}/pending-supervisor-probe/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let (runtime, dsl) =
            Self::build_runtime(&probe_participant_name, authority, &self.endpoint_config)
                .await
                .map_err(BridgeRequestFailure::BeforeSend)?;
        Self::apply_bridge_trust(&runtime, &dsl, recipient.clone())
            .await
            .map_err(BridgeRequestFailure::BeforeSend)?;
        #[cfg(not(target_arch = "wasm32"))]
        let reply_endpoint = Self::ephemeral_probe_reply_endpoint(&runtime, &self.endpoint_config)
            .map_err(BridgeRequestFailure::BeforeSend)?;
        #[cfg(not(target_arch = "wasm32"))]
        let reply_endpoint = SupervisorRequestReplyEndpoint::EphemeralProbe(reply_endpoint);
        #[cfg(target_arch = "wasm32")]
        let reply_endpoint = SupervisorRequestReplyEndpoint::Live;
        self.request_json_with_runtime_classified(
            &runtime,
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            reply_endpoint,
            timeout,
        )
        .await
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
        Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await
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
        Self::remove_bridge_trust(&runtime, &dsl, recipient.clone()).await
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
        // READ guard (DEC-P6E-14) across the SEND only: the gate exists so
        // the fixed-port authority swap cannot race an in-flight send. The
        // response wait consumes the shared intake buffer plus the runtime
        // Arc captured under the guard — no gate-guarded state — so it runs
        // guard-free and a rotation waits at most one in-flight send, never
        // a full `wait_for_response` window (ADJ-P6-2).
        let (runtime, envelope_id) = {
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
            (runtime, envelope_id)
        };
        let value = self
            .wait_for_response(&runtime, envelope_id, recipient.peer_id, timeout)
            .await
            .map_err(BridgeRequestFailure::AfterSend)?;
        Ok((value, envelope_id))
    }

    async fn request_json_with_runtime<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        reply_endpoint: SupervisorRequestReplyEndpoint,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        self.request_json_with_runtime_and_receipt(
            runtime,
            recipient,
            intent,
            payload,
            reply_endpoint,
            timeout,
        )
        .await
        .map(|(value, _envelope_id)| value)
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

    async fn request_json_with_runtime_and_receipt<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        reply_endpoint: SupervisorRequestReplyEndpoint,
        timeout: Duration,
    ) -> Result<(serde_json::Value, uuid::Uuid), MobError> {
        let envelope_id = self
            .send_supervisor_request(runtime, recipient, intent, payload, reply_endpoint)
            .await?;
        let value = self
            .wait_for_response(runtime, envelope_id, recipient.peer_id, timeout)
            .await?;
        Ok((value, envelope_id))
    }

    /// Serialize + send ONE supervisor request envelope on `runtime`,
    /// returning the request envelope id replies correlate on
    /// (DEC-P6E-19). Split from the response wait so
    /// `request_json_with_receipt` can scope the authority-gate read guard
    /// to the send alone.
    async fn send_supervisor_request<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        reply_endpoint: SupervisorRequestReplyEndpoint,
    ) -> Result<uuid::Uuid, MobError> {
        let to = PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone());
        let params = serde_json::to_value(payload).map_err(|error| {
            MobError::Internal(format!("serialize supervisor payload: {error}"))
        })?;
        #[cfg(not(target_arch = "wasm32"))]
        let receipt = {
            let reply_endpoint = match reply_endpoint {
                SupervisorRequestReplyEndpoint::Live => {
                    self.live_runtime_reply_endpoint(runtime)?
                }
                SupervisorRequestReplyEndpoint::EphemeralProbe(endpoint) => endpoint,
            };
            runtime
                .send_peer_request_at_endpoint(to, intent, params, reply_endpoint)
                .await?
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
        // is not ours belongs to a CONCURRENT `wait_for_response` — under
        // the relaxed authority gate, waiter A can drain waiter B's reply
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

    fn response_candidate(
        request_envelope_id: uuid::Uuid,
        status: ResponseStatus,
        result: serde_json::Value,
    ) -> PeerInputCandidate {
        let id = InteractionId(uuid::Uuid::new_v4());
        let worker_peer_id = PeerId::from_uuid(
            uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f5")
                .expect("valid worker route id"),
        );
        meerkat_runtime::test_peer_input_candidate_from_interaction(
            InboxInteraction {
                objective_id: None,
                sender_taint: None,
                id,
                from_route: Some(worker_peer_id),
                from: "worker-rt".to_string(),
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
            worker_peer_id,
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
        MobSupervisorBridge::apply_bridge_trust(&recipient_runtime, &recipient_dsl, sender)
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
        MobSupervisorBridge::apply_bridge_trust(&recipient_runtime, &recipient_dsl, sender.clone())
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
        let prebuilt = MobSupervisorBridge::build_runtime(
            &format!("{mob_id}/__rotation_commit_candidate__"),
            &next,
            &SupervisorBridgeEndpointConfig::default(),
        )
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

        fail_next_bridge_trust_add_reconcile_for_test(&recipient.peer_id.to_string());
        MobSupervisorBridge::apply_bridge_trust(&runtime, &dsl, recipient.clone())
            .await
            .expect_err("fault after AddDirectPeerEndpoint must surface");
        assert!(
            dsl.snapshot_state()
                .direct_peer_endpoints
                .contains(&mm_dsl::PeerEndpoint::from(&recipient)),
            "the fault must exercise DSL-committed/live-unreconciled add state"
        );
        assert_eq!(
            MobSupervisorBridge::apply_bridge_trust(&runtime, &dsl, recipient.clone())
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
        MobSupervisorBridge::remove_bridge_trust(&runtime, &dsl, recipient.clone())
            .await
            .expect_err("fault after RemoveDirectPeerEndpoint must surface");
        assert!(
            !dsl.snapshot_state()
                .direct_peer_endpoints
                .contains(&mm_dsl::PeerEndpoint::from(&recipient)),
            "the fault must exercise DSL-committed/live-unreconciled remove state"
        );
        MobSupervisorBridge::remove_bridge_trust(&runtime, &dsl, recipient.clone())
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
