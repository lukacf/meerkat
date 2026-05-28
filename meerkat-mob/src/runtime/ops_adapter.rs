#[cfg(feature = "runtime-adapter")]
use super::*;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::comms::TrustedPeerDescriptor;
#[cfg(feature = "runtime-adapter")]
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationLifecycleAction, OperationLifecycleSnapshot,
    OperationPeerHandle, OperationProgressUpdate, OperationSource, OperationSpec, OperationStatus,
    OpsLifecycleError, OpsLifecycleRegistry,
};
#[cfg(feature = "runtime-adapter")]
use meerkat_core::types::SessionId;
#[cfg(feature = "runtime-adapter")]
use std::collections::HashMap;
#[cfg(feature = "runtime-adapter")]
use std::sync::Mutex;

#[cfg(feature = "runtime-adapter")]
#[derive(Clone)]
struct MemberOpsBinding {
    owner_bridge_session_id: SessionId,
    registry: Arc<dyn OpsLifecycleRegistry>,
    display_name: Option<String>,
    operation_source: OperationSource,
}

#[cfg(feature = "runtime-adapter")]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum MemberOpsKey {
    Session(SessionId),
    BackendPeer { peer_id: String, address: String },
}

#[cfg(feature = "runtime-adapter")]
impl MemberOpsKey {
    fn from_member_ref(member_ref: &MemberRef) -> Option<Self> {
        match member_ref {
            MemberRef::Session { session_id } => Some(Self::Session(session_id.clone())),
            MemberRef::BackendPeer {
                session_id: Some(session_id),
                ..
            } => Some(Self::Session(session_id.clone())),
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id: None,
                ..
            } => Some(Self::BackendPeer {
                peer_id: peer_id.clone(),
                address: address.clone(),
            }),
        }
    }

    fn fallback_display_name(&self) -> String {
        match self {
            Self::Session(session_id) => format!("mob_member/{session_id}"),
            Self::BackendPeer { peer_id, address } => {
                format!("mob_member/backend_peer/{peer_id}@{address}")
            }
        }
    }

    fn child_session_id(&self) -> Option<SessionId> {
        match self {
            Self::Session(session_id) => Some(session_id.clone()),
            Self::BackendPeer { .. } => None,
        }
    }
}

#[cfg(feature = "runtime-adapter")]
pub(crate) struct MobOpsAdapter {
    member_bindings: Mutex<HashMap<MemberOpsKey, MemberOpsBinding>>,
}

#[cfg(feature = "runtime-adapter")]
impl MobOpsAdapter {
    pub(crate) fn new() -> Self {
        Self {
            member_bindings: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn bind_session_registry(
        &self,
        child_session_id: SessionId,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
    ) {
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                MemberOpsKey::Session(child_session_id.clone()),
                MemberOpsBinding {
                    owner_bridge_session_id,
                    registry,
                    display_name: None,
                    operation_source: OperationSource::session_child(child_session_id),
                },
            );
    }

    pub(crate) fn bind_member_registry(
        &self,
        member_ref: &MemberRef,
        owner_bridge_session_id: SessionId,
        registry: Arc<dyn OpsLifecycleRegistry>,
        display_name: impl Into<String>,
        operation_source: OperationSource,
    ) -> Result<(), MobError> {
        let member_key = MemberOpsKey::from_member_ref(member_ref).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot bind registry for member without canonical key: {member_ref:?}",
            ))
        })?;
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                member_key,
                MemberOpsBinding {
                    owner_bridge_session_id,
                    registry,
                    display_name: Some(display_name.into()),
                    operation_source,
                },
            );
        Ok(())
    }

    fn binding_for_key(&self, member_key: &MemberOpsKey) -> Option<MemberOpsBinding> {
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(member_key)
            .cloned()
    }

    fn require_binding_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<MemberOpsBinding, MobError> {
        self.binding_for_key(member_key).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': missing generated owner operation binding"
            ))
        })
    }

    fn clear_member_binding(&self, member_key: &MemberOpsKey) {
        self.member_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(member_key);
    }

    /// Resolve the live canonical mob-child lifecycle operation for a session.
    ///
    /// This is intentionally stricter than "latest operation for session":
    /// callers that need to mutate or re-bind active member lifecycle truth
    /// must never silently reuse a terminal operation snapshot.
    pub(crate) async fn active_operation_id_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<OperationId> {
        self.active_operation_id_for_key(&MemberOpsKey::Session(session_id.clone()))
            .await
    }

    pub(crate) async fn active_operation_id_for_member(
        &self,
        member_ref: &MemberRef,
    ) -> Option<OperationId> {
        let member_key = MemberOpsKey::from_member_ref(member_ref)?;
        self.active_operation_id_for_key(&member_key).await
    }

    async fn active_operation_id_for_key(&self, member_key: &MemberOpsKey) -> Option<OperationId> {
        let binding = self.binding_for_key(member_key)?;
        let snapshots = match Self::matching_operations_for_binding(member_key, &binding) {
            Ok(snapshots) => snapshots,
            Err(error) => {
                tracing::error!(
                    member_key = ?member_key,
                    error = %error,
                    "mob ops adapter failed to project operations through generated authority",
                );
                return None;
            }
        };
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => {
                let operation_id = active_ids[0].clone();
                if let Err(error) = self.ensure_running_before_live_update(
                    member_key,
                    &operation_id,
                    "resolve active operation",
                ) {
                    tracing::error!(
                        member_key = ?member_key,
                        operation_id = %operation_id,
                        error = %error,
                        "mob ops adapter failed to canonicalize active operation before returning it",
                    );
                    return None;
                }
                Some(operation_id)
            }
            0 => None,
            _ => {
                tracing::error!(
                    member_key = ?member_key,
                    operation_ids = %Self::format_operation_ids(&active_ids),
                    "mob ops adapter found ambiguous active mob child operations; refusing to pick one",
                );
                None
            }
        }
    }

    fn require_member_key(
        member_ref: &MemberRef,
        operation: &str,
    ) -> Result<MemberOpsKey, MobError> {
        MemberOpsKey::from_member_ref(member_ref).ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot {operation} member without canonical lifecycle key: {member_ref:?}",
            ))
        })
    }

    fn matching_operations_for_key(
        &self,
        member_key: &MemberOpsKey,
    ) -> Result<Vec<OperationLifecycleSnapshot>, MobError> {
        let Some(binding) = self.binding_for_key(member_key) else {
            return Ok(Vec::new());
        };
        Self::matching_operations_for_binding(member_key, &binding)
    }

    fn snapshot_matches_binding(
        binding: &MemberOpsBinding,
        snapshot: &OperationLifecycleSnapshot,
    ) -> bool {
        snapshot.kind == OperationKind::MobMemberChild
            && snapshot.operation_source.as_ref() == Some(&binding.operation_source)
    }

    fn matching_operations_for_binding(
        _member_key: &MemberOpsKey,
        binding: &MemberOpsBinding,
    ) -> Result<Vec<OperationLifecycleSnapshot>, MobError> {
        Ok(binding
            .registry
            .list_operations()
            .map_err(|error| MobError::Internal(error.to_string()))?
            .into_iter()
            .filter(|snapshot| Self::snapshot_matches_binding(binding, snapshot))
            .collect())
    }

    fn require_operation_for_binding(
        member_key: &MemberOpsKey,
        binding: &MemberOpsBinding,
        operation_id: &OperationId,
        operation: &str,
    ) -> Result<OperationLifecycleSnapshot, MobError> {
        let Some(snapshot) = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
        else {
            return Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation}: operation '{operation_id}' not found",
            )));
        };
        if !Self::snapshot_matches_binding(binding, &snapshot) {
            return Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': operation '{operation_id}' is not owned by generated operation source authority"
            )));
        }
        Ok(snapshot)
    }

    fn newest_operation_snapshot(
        snapshots: &[OperationLifecycleSnapshot],
    ) -> Option<&OperationLifecycleSnapshot> {
        snapshots.iter().max_by(|lhs, rhs| {
            lhs.created_at_ms
                .cmp(&rhs.created_at_ms)
                .then_with(|| lhs.id.0.cmp(&rhs.id.0))
        })
    }

    fn snapshot_is_terminal(snapshot: &OperationLifecycleSnapshot) -> bool {
        snapshot.terminal
    }

    fn active_operation_ids(snapshots: &[OperationLifecycleSnapshot]) -> Vec<OperationId> {
        let mut active_ids = Vec::new();
        for snapshot in snapshots {
            if !Self::snapshot_is_terminal(snapshot) {
                active_ids.push(snapshot.id.clone());
            }
        }
        active_ids
    }

    fn invalid_transition_is_idempotent(
        registry: &Arc<dyn OpsLifecycleRegistry>,
        action: OperationLifecycleAction,
        error: &OpsLifecycleError,
    ) -> Result<bool, MobError> {
        match error {
            OpsLifecycleError::InvalidTransition { id, .. } => registry
                .classify_operation_transition_idempotence(id, action)
                .map_err(|error| MobError::Internal(error.to_string())),
            _ => Ok(false),
        }
    }

    fn format_operation_ids(ids: &[OperationId]) -> String {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn operation_status_with_terminality(
        &self,
        session_id: &SessionId,
        operation_id: &OperationId,
    ) -> Result<Option<(OperationStatus, bool)>, MobError> {
        let Some(binding) = self.binding_for_key(&MemberOpsKey::Session(session_id.clone())) else {
            return Ok(None);
        };
        let Some(snapshot) = binding
            .registry
            .snapshot(operation_id)
            .map_err(|error| MobError::Internal(error.to_string()))?
            .filter(|snapshot| Self::snapshot_matches_binding(&binding, snapshot))
        else {
            return Ok(None);
        };
        let terminal = binding
            .registry
            .classify_operation_terminality(operation_id, snapshot.status)
            .map_err(|error| MobError::Internal(error.to_string()))?;
        Ok(Some((snapshot.status, terminal)))
    }

    fn display_name_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<String, MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        Ok(binding
            .display_name
            .unwrap_or_else(|| member_key.fallback_display_name()))
    }

    fn registry_for_existing_binding(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
    ) -> Result<Arc<dyn OpsLifecycleRegistry>, MobError> {
        Ok(self
            .require_binding_for_key(member_key, operation)?
            .registry)
    }

    async fn resolve_or_register_active_operation_id_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation: &str,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        let snapshots = Self::matching_operations_for_binding(member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => Ok(active_ids[0].clone()),
            0 => {
                self.ensure_operation_for_key(member_key, display_name)
                    .await
            }
            _ => Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation} for member '{member_key:?}': multiple active mob child operations [{}]",
                Self::format_operation_ids(&active_ids),
            ))),
        }
    }

    async fn ensure_operation_for_key(
        &self,
        member_key: &MemberOpsKey,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let binding = self.require_binding_for_key(member_key, "register operation")?;
        let snapshots = Self::matching_operations_for_binding(member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => return Ok(active_ids[0].clone()),
            0 => {}
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot provision member '{member_key:?}' with ambiguous active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        }

        let operation_id = OperationId::new();
        let registry = binding.registry;
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: binding.owner_bridge_session_id,
                display_name: display_name.to_string(),
                source_label: "mob_member".to_string(),
                operation_source: Some(binding.operation_source.clone()),
                child_session_id: member_key.child_session_id(),
                expect_peer_channel: true,
            })
            .map_err(|error| MobError::Internal(error.to_string()))?;
        Ok(operation_id)
    }

    pub(crate) async fn mark_member_provisioned(
        &self,
        session_id: &SessionId,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        self.mark_member_provisioned_for_key(
            &MemberOpsKey::Session(session_id.clone()),
            display_name,
        )
        .await
    }

    pub(crate) async fn mark_member_provisioned_for_member(
        &self,
        member_ref: &MemberRef,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let member_key = Self::require_member_key(member_ref, "mark provisioned for")?;
        self.mark_member_provisioned_for_key(&member_key, display_name)
            .await
    }

    async fn mark_member_provisioned_for_key(
        &self,
        member_key: &MemberOpsKey,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let operation_id = self
            .ensure_operation_for_key(member_key, display_name)
            .await?;
        let registry = self.registry_for_existing_binding(member_key, "mark provisioned")?;
        match registry.provisioning_succeeded(&operation_id) {
            Ok(()) => {}
            Err(error) => {
                if !Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Start,
                    &error,
                )? {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
        }
        Ok(operation_id)
    }

    pub(crate) async fn report_member_progress(
        &self,
        member_ref: &MemberRef,
        message: impl Into<String>,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "report progress for")?;
        let display_name = self.display_name_for_key(&member_key, "report progress")?;
        let operation_id = self
            .resolve_or_register_active_operation_id_for_key(
                &member_key,
                "report progress",
                &display_name,
            )
            .await?;
        self.ensure_running_before_live_update(&member_key, &operation_id, "report progress")?;
        let registry = self.registry_for_existing_binding(&member_key, "report progress")?;
        match registry.report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: message.into(),
                percent: None,
            },
        ) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::ProgressReported,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }
    }

    pub(crate) async fn mark_member_peer_ready(
        &self,
        member_ref: &MemberRef,
        peer_name: &str,
        trusted_peer: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "mark peer ready for")?;
        let operation_id = self
            .resolve_or_register_active_operation_id_for_key(
                &member_key,
                "mark peer ready",
                peer_name,
            )
            .await?;
        self.ensure_running_before_live_update(&member_key, &operation_id, "mark peer ready")?;
        let registry = self.registry_for_existing_binding(&member_key, "mark peer ready")?;
        match registry.peer_ready(
            &operation_id,
            OperationPeerHandle {
                peer_name: meerkat_core::comms::PeerName::new(peer_name)
                    .map_err(|e| MobError::Internal(format!("invalid peer name: {e}")))?,
                trusted_peer,
            },
        ) {
            Ok(()) | Err(OpsLifecycleError::AlreadyPeerReady(_)) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::PeerReady,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        }
    }

    pub(crate) async fn mark_member_retired(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "mark retired for")?;
        let binding = self.require_binding_for_key(&member_key, "mark retired")?;
        let snapshots = Self::matching_operations_for_binding(&member_key, &binding)?;
        let active_ids = Self::active_operation_ids(&snapshots);
        let operation_id = match active_ids.len() {
            1 => active_ids[0].clone(),
            0 => {
                if let Some(latest) = Self::newest_operation_snapshot(&snapshots)
                    && Self::snapshot_is_terminal(latest)
                {
                    return Ok(());
                }
                return Ok(());
            }
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot retire member '{member_key:?}': multiple active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        };
        let registry = binding.registry;
        match registry.request_retire(&operation_id) {
            Ok(()) => {}
            Err(error) => {
                if !Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::RetireRequested,
                    &error,
                )? {
                    return Err(MobError::Internal(error.to_string()));
                }
            }
        }
        let result = match registry.mark_retired(&operation_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::RetireCompleted,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        };
        if result.is_ok() {
            self.clear_member_binding(&member_key);
        }
        result
    }

    pub(crate) async fn abort_member_provision(
        &self,
        session_id: &SessionId,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        self.abort_member_provision_for_key(
            &MemberOpsKey::Session(session_id.clone()),
            operation_id,
            reason,
        )
        .await
    }

    pub(crate) async fn abort_member_provision_for_member(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        let member_key = Self::require_member_key(member_ref, "abort provision for")?;
        self.abort_member_provision_for_key(&member_key, operation_id, reason)
            .await
    }

    async fn abort_member_provision_for_key(
        &self,
        member_key: &MemberOpsKey,
        operation_id: &OperationId,
        reason: Option<String>,
    ) -> Result<(), MobError> {
        let binding = self.require_binding_for_key(member_key, "abort provision")?;
        let registry = Arc::clone(&binding.registry);
        Self::require_operation_for_binding(member_key, &binding, operation_id, "abort provision")?;
        let result = match registry.abort_provisioning(operation_id, reason) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::NotFound(_)) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Abort,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(error.to_string()))
                }
            }
        };
        if result.is_ok() {
            self.clear_member_binding(member_key);
        }
        result
    }

    fn ensure_running_before_live_update(
        &self,
        member_key: &MemberOpsKey,
        operation_id: &OperationId,
        operation: &str,
    ) -> Result<(), MobError> {
        let binding = self.require_binding_for_key(member_key, operation)?;
        let registry = Arc::clone(&binding.registry);
        let snapshot =
            Self::require_operation_for_binding(member_key, &binding, operation_id, operation)?;
        if snapshot.status != OperationStatus::Provisioning {
            return Ok(());
        }

        match registry.provisioning_succeeded(operation_id) {
            Ok(()) => Ok(()),
            Err(error) => {
                if Self::invalid_transition_is_idempotent(
                    &registry,
                    OperationLifecycleAction::Start,
                    &error,
                )? {
                    Ok(())
                } else {
                    Err(MobError::Internal(format!(
                        "mob ops adapter cannot {operation}: failed to canonicalize provisioning operation '{operation_id}' into running state: {error}",
                    )))
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::{PeerAddress, PeerId, TrustedPeerDescriptor};
    use meerkat_core::ops_lifecycle::OperationStatus;
    use meerkat_runtime::RuntimeOpsLifecycleRegistry;

    #[tokio::test]
    async fn ops_registry_integration_red_ok_member_adapter_tracks_peer_ready_and_retire() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            session_id.clone(),
            owner_bridge_session_id,
            Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
        );

        let operation_id = adapter
            .mark_member_provisioned(&session_id, "mob-a/orchestrator/member-alpha")
            .await
            .expect("register member");
        adapter
            .report_member_progress(&member_ref, "turn dispatched")
            .await
            .expect("progress");
        adapter
            .mark_member_peer_ready(
                &member_ref,
                "mob-a/orchestrator/member-alpha",
                TrustedPeerDescriptor::test_only_unsigned(
                    "mob-a/orchestrator/member-alpha",
                    "00000000-0000-4000-8000-000000000002",
                    "inproc://member-alpha",
                )
                .expect("trusted peer"),
            )
            .await
            .expect("peer ready");

        let running_snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(running_snapshot.status, OperationStatus::Running);
        assert!(running_snapshot.peer_ready);
        assert_eq!(running_snapshot.progress_count, 1);

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("retire member");

        let retired_snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(retired_snapshot.status, OperationStatus::Retired);
    }

    #[tokio::test]
    async fn bound_session_registry_owns_child_operation_ids() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let bound_registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            child_session_id.clone(),
            owner_bridge_session_id,
            Arc::clone(&bound_registry) as Arc<dyn OpsLifecycleRegistry>,
        );

        let operation_id = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-bound")
            .await
            .expect("register bound child op");

        let bound_snapshot = bound_registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("bound registry should own exported child operation ids");
        assert_eq!(bound_snapshot.kind, OperationKind::MobMemberChild);
        assert_eq!(bound_snapshot.status, OperationStatus::Running);
        assert_eq!(bound_snapshot.child_session_id, Some(child_session_id));
    }

    #[tokio::test]
    async fn repeated_member_provision_uses_generated_idempotence_classifier() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            child_session_id.clone(),
            owner_bridge_session_id,
            Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
        );

        let first = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-idempotent")
            .await
            .expect("first provision");
        let second = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-idempotent")
            .await
            .expect("generated idempotent start classification");

        assert_eq!(first, second);
        let snapshot = registry
            .snapshot(&first)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Running);
    }

    #[tokio::test]
    async fn already_retiring_member_retire_uses_generated_idempotence_classifier() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(child_session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            child_session_id.clone(),
            owner_bridge_session_id,
            Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
        );

        let operation_id = adapter
            .mark_member_provisioned(&child_session_id, "mob/member-retiring")
            .await
            .expect("provision member");
        registry
            .request_retire(&operation_id)
            .expect("generated retire request");

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("generated idempotent retire-request classification");

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Retired);
    }

    #[tokio::test]
    async fn mark_member_retired_without_existing_operation_is_noop() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            session_id,
            owner_bridge_session_id,
            Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
        );

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("missing lifecycle entry should retire as no-op");

        assert!(
            registry.list_operations().unwrap().is_empty(),
            "retire must not fabricate a provisioning operation"
        );
    }

    #[tokio::test]
    async fn member_operation_registration_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let err = adapter
            .mark_member_provisioned(&session_id, "mob/member-unbound")
            .await
            .expect_err("missing generated owner binding must fail closed");
        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );

        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-a".into(),
            address: "inproc://peer-a".into(),
            pubkey: None,
            bootstrap_token: None,
            session_id: None,
        };
        let err = adapter
            .mark_member_provisioned_for_member(&member_ref, "mob/backend-peer")
            .await
            .expect_err("peer-only member without generated owner binding must fail closed");
        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
        assert!(
            adapter
                .active_operation_id_for_member(&member_ref)
                .await
                .is_none(),
            "unbound peer-only member must not create fallback operation state"
        );
    }

    #[tokio::test]
    async fn peer_only_operation_identity_uses_generated_operation_source() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        let peer_id = PeerId::new();
        let address = PeerAddress::parse("inproc://peer-a").expect("peer address");
        let operation_source = OperationSource::backend_peer(peer_id, address.clone());
        let member_ref = MemberRef::BackendPeer {
            peer_id: peer_id.to_string(),
            address: address.to_string(),
            pubkey: None,
            bootstrap_token: None,
            session_id: None,
        };
        adapter
            .bind_member_registry(
                &member_ref,
                owner_bridge_session_id.clone(),
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "display-before",
                operation_source.clone(),
            )
            .expect("bind peer-only member registry");

        let operation_id = adapter
            .mark_member_provisioned_for_member(&member_ref, "display-before")
            .await
            .expect("register peer-only child operation");
        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.display_name, "display-before");
        assert_eq!(snapshot.operation_source, Some(operation_source.clone()));

        adapter
            .bind_member_registry(
                &member_ref,
                owner_bridge_session_id,
                Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
                "display-after",
                operation_source,
            )
            .expect("rebinding display name must not change lifecycle identity");

        assert_eq!(
            adapter.active_operation_id_for_member(&member_ref).await,
            Some(operation_id)
        );
    }

    #[tokio::test]
    async fn mark_member_retired_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let member_ref = MemberRef::from_bridge_session_id(SessionId::new());

        let err = adapter
            .mark_member_retired(&member_ref)
            .await
            .expect_err("missing generated owner binding must fail closed");

        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn abort_member_provision_marks_provisioning_operation_aborted() {
        let adapter = MobOpsAdapter::new();
        let owner_bridge_session_id = SessionId::new();
        let session_id = SessionId::new();
        let operation_id = OperationId::new();
        let operation_source = OperationSource::session_child(session_id.clone());
        let registry = Arc::new(RuntimeOpsLifecycleRegistry::new());
        adapter.bind_session_registry(
            session_id.clone(),
            owner_bridge_session_id,
            Arc::clone(&registry) as Arc<dyn OpsLifecycleRegistry>,
        );
        registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: "mob/member-abort".into(),
                source_label: "mob_member".into(),
                operation_source: Some(operation_source),
                child_session_id: Some(session_id.clone()),
                expect_peer_channel: true,
            })
            .expect("register provisioning operation");

        adapter
            .abort_member_provision(&session_id, &operation_id, Some("mob is stopping".into()))
            .await
            .expect("abort provisioning should succeed");

        let snapshot = registry
            .snapshot(&operation_id)
            .expect("snapshot projection")
            .expect("snapshot");
        assert_eq!(snapshot.status, OperationStatus::Aborted);
    }

    #[tokio::test]
    async fn abort_member_provision_without_owner_binding_fails_closed() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let operation_id = OperationId::new();

        let err = adapter
            .abort_member_provision(&session_id, &operation_id, Some("mob is stopping".into()))
            .await
            .expect_err("missing generated owner binding must fail closed");

        assert!(
            err.to_string()
                .contains("missing generated owner operation binding"),
            "unexpected error: {err}"
        );
    }
}
