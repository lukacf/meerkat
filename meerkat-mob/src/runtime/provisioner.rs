use super::*;
use crate::MobBackendKind;
use crate::definition::ExternalBackendConfig;
use crate::event::MemberRef;
use async_trait::async_trait;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::event_injector::SubscribableInjector;
use meerkat_core::service::{CreateSessionRequest, StartTurnRequest};
use meerkat_core::types::SessionId;

pub struct ProvisionMemberRequest {
    pub create_session: CreateSessionRequest,
    pub backend: MobBackendKind,
    pub peer_name: String,
}

#[async_trait]
pub trait MobProvisioner: Send + Sync {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError>;
    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    async fn event_injector(&self, session_id: &SessionId)
    -> Option<Arc<dyn SubscribableInjector>>;
    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError>;
    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>>;
    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError>;

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

pub struct SubagentBackend {
    session_service: Arc<dyn MobSessionService>,
}

impl SubagentBackend {
    pub fn new(session_service: Arc<dyn MobSessionService>) -> Self {
        Self { session_service }
    }

    fn require_session(
        member_ref: &MemberRef,
        operation: &'static str,
    ) -> Result<SessionId, MobError> {
        member_ref.session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "session-backed provisioner cannot {operation} member without session bridge: {member_ref:?}"
            ))
        })
    }

    fn trusted_peer_spec(
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        TrustedPeerSpec::new(
            fallback_name,
            fallback_peer_id,
            format!("inproc://{fallback_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
    }
}

#[async_trait]
impl MobProvisioner for SubagentBackend {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError> {
        tracing::debug!(
            backend = ?req.backend,
            peer_name = %req.peer_name,
            "SubagentBackend::provision_member start"
        );
        let created = self
            .session_service
            .create_session(req.create_session)
            .await?;
        tracing::debug!(
            session_id = %created.session_id,
            "SubagentBackend::provision_member created session"
        );
        Ok(MemberRef::from_session_id(created.session_id))
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "retire")?;
        self.session_service.archive(&session_id).await?;
        Ok(())
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        self.session_service.interrupt(&session_id).await?;
        Ok(())
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start turn")?;
        self.session_service.start_turn(&session_id, req).await?;
        Ok(())
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session_service.event_injector(session_id).await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        let session_id = match member_ref.session_id() {
            Some(id) => id.clone(),
            None => return Ok(None),
        };
        match self.session_service.read(&session_id).await {
            Ok(view) => Ok(Some(view.state.is_active)),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => Ok(Some(false)),
            Err(error) => Err(error.into()),
        }
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        let session_id = member_ref.session_id()?;
        self.session_service.comms_runtime(session_id).await
    }

    async fn trusted_peer_spec(
        &self,
        _member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        Self::trusted_peer_spec(fallback_name, fallback_peer_id)
    }

    async fn cancel_all_checkpointers(&self) {
        self.session_service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session_service.rearm_all_checkpointers().await;
    }
}

pub struct ExternalBackend {
    session_service: Arc<dyn MobSessionService>,
    address_base: String,
}

impl ExternalBackend {
    pub fn new(session_service: Arc<dyn MobSessionService>, config: ExternalBackendConfig) -> Self {
        Self {
            session_service,
            address_base: config.address_base.trim_end_matches('/').to_string(),
        }
    }
}

fn is_valid_peer_name_component(component: &str) -> bool {
    if component.is_empty() {
        return false;
    }
    let mut chars = component.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_valid_external_peer_name(peer_name: &str) -> bool {
    let mut parts = peer_name.split('/');
    let Some(mob_id) = parts.next() else {
        return false;
    };
    let Some(profile) = parts.next() else {
        return false;
    };
    let Some(meerkat_id) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [mob_id, profile, meerkat_id]
        .iter()
        .all(|part| is_valid_peer_name_component(part))
}

pub struct MultiBackendProvisioner {
    subagent: SubagentBackend,
    external: Option<ExternalBackend>,
}

impl MultiBackendProvisioner {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        external: Option<ExternalBackendConfig>,
    ) -> Self {
        let subagent = SubagentBackend::new(session_service.clone());
        let external = external.map(|cfg| ExternalBackend::new(session_service, cfg));
        Self { subagent, external }
    }

    async fn external_member_ref(
        &self,
        create_session: CreateSessionRequest,
        peer_name: String,
    ) -> Result<MemberRef, MobError> {
        if !is_valid_external_peer_name(&peer_name) {
            return Err(MobError::WiringError(format!(
                "invalid external peer name '{}': expected '<mob>/<profile>/<meerkat>' using identifier-safe segments",
                peer_name
            )));
        }
        tracing::debug!(
            peer_name = %peer_name,
            "ExternalBackend::external_member_ref start"
        );
        let external = self
            .external
            .as_ref()
            .ok_or_else(|| MobError::WiringError("external backend is not configured".into()))?;
        let created = external
            .session_service
            .create_session(create_session)
            .await?;
        tracing::debug!(
            session_id = %created.session_id,
            "ExternalBackend::external_member_ref created session"
        );
        let comms = external
            .session_service
            .comms_runtime(&created.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "external backend missing comms runtime for '{peer_name}'"
                ))
            })?;
        let peer_id = comms.public_key().ok_or_else(|| {
            MobError::WiringError(format!(
                "external backend missing public key for '{peer_name}'"
            ))
        })?;
        let address = format!("{}/{}", external.address_base, peer_name);
        tracing::debug!(
            peer_id = %peer_id,
            address = %address,
            "ExternalBackend::external_member_ref success"
        );
        Ok(MemberRef::BackendPeer {
            peer_id,
            address,
            session_id: Some(created.session_id),
        })
    }
}

#[async_trait]
impl MobProvisioner for MultiBackendProvisioner {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError> {
        match req.backend {
            MobBackendKind::Subagent => {
                self.subagent
                    .provision_member(ProvisionMemberRequest {
                        create_session: req.create_session,
                        backend: MobBackendKind::Subagent,
                        peer_name: req.peer_name,
                    })
                    .await
            }
            MobBackendKind::External => {
                self.external_member_ref(req.create_session, req.peer_name)
                    .await
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.subagent.retire_member(member_ref).await
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.subagent.interrupt_member(member_ref).await
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        self.subagent.start_turn(member_ref, req).await
    }

    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.subagent.event_injector(session_id).await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        self.subagent.is_member_active(member_ref).await
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.subagent.comms_runtime(member_ref).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.subagent
                    .trusted_peer_spec(member_ref, fallback_name, fallback_peer_id)
                    .await
            }
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id,
            } => {
                if let Some(session_id) = session_id {
                    // External members still keep a local session bridge; use a sendable
                    // transport address for trust wiring while preserving backend identity
                    // in MemberRef::BackendPeer.address.
                    return self
                        .subagent
                        .trusted_peer_spec(
                            &MemberRef::Session {
                                session_id: session_id.clone(),
                            },
                            fallback_name,
                            peer_id,
                        )
                        .await;
                }
                TrustedPeerSpec::new(fallback_name, peer_id.clone(), address.clone())
                    .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
            }
        }
    }

    async fn cancel_all_checkpointers(&self) {
        self.subagent.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.subagent.rearm_all_checkpointers().await;
    }
}
