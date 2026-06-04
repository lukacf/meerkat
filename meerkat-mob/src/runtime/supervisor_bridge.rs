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

pub(crate) struct MobSupervisorBridge {
    participant_name: String,
    endpoint_config: SupervisorBridgeEndpointConfig,
    authority: StdRwLock<SupervisorAuthorityRecord>,
    runtime: RwLock<Arc<meerkat_comms::CommsRuntime>>,
    dsl: RwLock<Arc<meerkat_runtime::HandleDslAuthority>>,
    buffered_candidates: Mutex<VecDeque<PeerInputCandidate>>,
    /// Serializes bridge command requests so that at most one
    /// request/response round-trip is in flight on this supervisor bridge
    /// at any time. Held for the full duration of `send_bridge_command`
    /// from `add_trusted_peer` through reply receipt, which keeps the
    /// comms `PeerResponse` correlation free of concurrent interleaving
    /// and makes rotation / bind fallbacks deterministic.
    request_lock: Mutex<()>,
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
            request_lock: Mutex::new(()),
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
        let mut runtime_guard = self.runtime.write().await;
        let (runtime, dsl) = match prepared.prebuilt {
            Some(prebuilt) => prebuilt,
            None => {
                #[cfg(not(target_arch = "wasm32"))]
                runtime_guard.stop_listeners_for_rebind().await;
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
        let _request_guard = self.request_lock.lock().await;
        self.replace_runtime_authority_locked(authority, true).await
    }

    pub(crate) async fn shutdown(&self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let runtime = self.runtime().await;
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

    pub(crate) async fn authority(&self) -> SupervisorAuthorityRecord {
        self.authority
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn runtime(&self) -> Arc<meerkat_comms::CommsRuntime> {
        self.runtime.read().await.clone()
    }

    pub(crate) async fn runtime_core(&self) -> Arc<dyn CoreCommsRuntime> {
        self.runtime().await as Arc<dyn CoreCommsRuntime>
    }

    async fn apply_bridge_trust(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        dsl: &Arc<meerkat_runtime::HandleDslAuthority>,
        recipient: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
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
        if dsl
            .snapshot_state()
            .direct_peer_endpoints
            .contains(&endpoint)
        {
            return Ok(());
        }
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
        let comms_runtime: Arc<dyn CoreCommsRuntime> = runtime.clone();
        let reconciler =
            meerkat_runtime::comms_trust_reconcile::CommsTrustReconciler::new(comms_runtime);
        reconciler
            .reconcile(&obligation)
            .await
            .map(|_report| ())
            .map_err(|error| {
                MobError::Internal(format!(
                    "supervisor bridge generated trust reconciliation failed: {error}"
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
        let authority = self.authority().await;
        self.supervisor_spec_with_address(&authority, format!("inproc://{}", self.participant_name))
    }

    pub(crate) async fn routable_supervisor_spec(&self) -> Result<TrustedPeerDescriptor, MobError> {
        let authority = self.authority().await;
        let runtime = self.runtime().await;
        let address = self.supervisor_routable_address(runtime.as_ref())?;
        self.supervisor_spec_with_address(&authority, address)
    }

    pub(crate) async fn supervisor_spec_for_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        self.supervisor_spec_for_authority_and_recipient(&self.authority().await, recipient)
            .await
    }

    pub(crate) async fn supervisor_spec_for_authority_and_recipient(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<TrustedPeerDescriptor, MobError> {
        if recipient.address.transport() == PeerTransport::Inproc {
            self.supervisor_spec_with_address(
                authority,
                format!("inproc://{}", self.participant_name),
            )
        } else {
            let runtime = self.runtime().await;
            let address = self.supervisor_routable_address(runtime.as_ref())?;
            self.supervisor_spec_with_address(authority, address)
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

    pub(crate) async fn send_bridge_command_as_authority(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let _request_guard = self.request_lock.lock().await;
        if Self::has_fixed_supervisor_bind_port(&self.endpoint_config)? {
            let previous_authority = self.authority().await;
            let restore_after_send = previous_authority != *authority;
            if restore_after_send {
                self.replace_runtime_authority_locked(authority.clone(), false)
                    .await?;
            }
            let send_result = async {
                let runtime = self.runtime().await;
                // Route the recipient trust through the generated machine
                // authority (DSL), not a raw comms add_trusted_peer, so the
                // trust write is revalidated by canonical authority under the
                // swapped rotation authority — matching the non-fixed-port
                // branch below and trust_recipient (Invariant #1: no raw or
                // legacy authority writes).
                let dsl = self.dsl.read().await.clone();
                Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
                self.request_json_with_runtime(
                    &runtime,
                    recipient,
                    super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
                    command,
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
        let probe_participant_name = format!(
            "{}/pending-supervisor-probe/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let (runtime, dsl) =
            Self::build_runtime(&probe_participant_name, authority, &self.endpoint_config).await?;
        Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await?;
        self.request_json_with_runtime(
            &runtime,
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
    }

    pub(crate) async fn trust_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let runtime = self.runtime().await;
        let dsl = self.dsl.read().await.clone();
        Self::apply_bridge_trust(&runtime, &dsl, recipient.clone()).await
    }

    pub(crate) async fn request_json<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let _request_guard = self.request_lock.lock().await;
        let runtime = self.runtime().await;
        self.request_json_with_runtime(&runtime, recipient, intent, payload, timeout)
            .await
    }

    async fn request_json_with_runtime<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let to = PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone());
        let params = serde_json::to_value(payload).map_err(|error| {
            MobError::Internal(format!("serialize supervisor payload: {error}"))
        })?;
        let receipt = runtime
            .send(CommsCommand::PeerRequest {
                to,
                intent: intent.to_string(),
                params,
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: InputStreamMode::None,
            })
            .await?;
        let SendReceipt::PeerRequestSent { envelope_id, .. } = receipt else {
            return Err(MobError::Internal(
                "unexpected receipt for supervisor request".to_string(),
            ));
        };
        self.wait_for_response(runtime, envelope_id, recipient.peer_id, timeout)
            .await
    }

    async fn wait_for_response(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
        expected_responder: PeerId,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        if let Some(result) = self
            .take_buffered_response(runtime, request_envelope_id, expected_responder)
            .await?
        {
            return Ok(result);
        }

        let deadline = BridgeRequestDeadline::start(timeout);
        loop {
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
                return Err(MobError::Internal(format!(
                    "supervisor request '{request_envelope_id}' timed out after {}ms",
                    timeout.as_millis()
                )));
            }

            if tokio::time::timeout(remaining, runtime.inbox_notify().notified())
                .await
                .is_err()
            {
                Self::record_request_timed_out(runtime, request_envelope_id)?;
                return Err(MobError::Internal(format!(
                    "supervisor request '{request_envelope_id}' timed out after {}ms",
                    timeout.as_millis()
                )));
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
                None => buffered.push_back(candidate),
            }
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
            .request_sent(corr_id, "worker-peer".to_string())
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
            .request_sent(corr_id, "worker-peer".to_string())
            .expect("request should be recorded by generated authority");

        MobSupervisorBridge::record_request_timed_out(&runtime, request_envelope_id)
            .expect("timeout should be recorded by generated authority");

        assert_eq!(
            handle.outbound_state(corr_id),
            None,
            "generated timeout transition should clean up pending supervisor request truth"
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
