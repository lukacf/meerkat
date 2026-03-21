use super::*;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationLifecycleSnapshot, OperationPeerHandle,
    OperationProgressUpdate, OperationSpec, OperationStatus, OpsLifecycleError,
    OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeOpsLifecycleRegistry;

#[derive(Debug, Default)]
pub(crate) struct MobOpsAdapter {
    registry: Arc<RuntimeOpsLifecycleRegistry>,
}

impl MobOpsAdapter {
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(RuntimeOpsLifecycleRegistry::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn registry(&self) -> Arc<RuntimeOpsLifecycleRegistry> {
        Arc::clone(&self.registry)
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
        let snapshots = self.matching_operations_for_session(session_id);
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => {
                let operation_id = active_ids[0].clone();
                if let Err(error) = self
                    .ensure_running_before_live_update(&operation_id, "resolve active operation")
                {
                    tracing::error!(
                        session_id = %session_id,
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
                    session_id = %session_id,
                    operation_ids = %Self::format_operation_ids(&active_ids),
                    "mob ops adapter found ambiguous active mob child operations; refusing to pick one",
                );
                None
            }
        }
    }

    fn require_member_session(
        member_ref: &MemberRef,
        operation: &str,
    ) -> Result<SessionId, MobError> {
        member_ref.session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "mob ops adapter cannot {operation} member without session-backed identity: {member_ref:?}",
            ))
        })
    }

    fn matching_operations_for_session(
        &self,
        session_id: &SessionId,
    ) -> Vec<OperationLifecycleSnapshot> {
        self.registry
            .list_operations()
            .into_iter()
            .filter(|snapshot| {
                snapshot.kind == OperationKind::MobMemberChild
                    && snapshot.child_session_id.as_ref() == Some(session_id)
            })
            .collect()
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

    fn active_operation_ids(snapshots: &[OperationLifecycleSnapshot]) -> Vec<OperationId> {
        snapshots
            .iter()
            .filter(|snapshot| !snapshot.status.is_terminal())
            .map(|snapshot| snapshot.id.clone())
            .collect()
    }

    fn format_operation_ids(ids: &[OperationId]) -> String {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn ensure_running_before_live_update(
        &self,
        operation_id: &OperationId,
        operation: &str,
    ) -> Result<(), MobError> {
        let Some(snapshot) = self.registry.snapshot(operation_id) else {
            return Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation}: operation '{operation_id}' not found",
            )));
        };
        if snapshot.status != OperationStatus::Provisioning {
            return Ok(());
        }

        match self.registry.provisioning_succeeded(operation_id) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::InvalidTransition { status, .. })
                if status != OperationStatus::Provisioning =>
            {
                Ok(())
            }
            Err(error) => Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation}: failed to canonicalize provisioning operation '{operation_id}' into running state: {error}",
            ))),
        }
    }

    async fn resolve_or_register_active_operation_id(
        &self,
        session_id: &SessionId,
        operation: &str,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let snapshots = self.matching_operations_for_session(session_id);
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => Ok(active_ids[0].clone()),
            0 => self.ensure_operation(session_id, display_name).await,
            _ => Err(MobError::Internal(format!(
                "mob ops adapter cannot {operation} for session '{session_id}': multiple active mob child operations [{}]",
                Self::format_operation_ids(&active_ids),
            ))),
        }
    }

    async fn ensure_operation(
        &self,
        session_id: &SessionId,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        let snapshots = self.matching_operations_for_session(session_id);
        let active_ids = Self::active_operation_ids(&snapshots);
        match active_ids.len() {
            1 => return Ok(active_ids[0].clone()),
            0 => {}
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot provision session '{session_id}' with ambiguous active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        }

        let operation_id = OperationId::new();
        self.registry
            .register_operation(OperationSpec {
                id: operation_id.clone(),
                kind: OperationKind::MobMemberChild,
                owner_session_id: session_id.clone(),
                display_name: display_name.to_string(),
                source_label: "mob_member".to_string(),
                child_session_id: Some(session_id.clone()),
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
        let operation_id = self.ensure_operation(session_id, display_name).await?;
        match self.registry.provisioning_succeeded(&operation_id) {
            Ok(()) => {}
            Err(OpsLifecycleError::InvalidTransition {
                status: OperationStatus::Running,
                ..
            }) => {}
            Err(error) => return Err(MobError::Internal(error.to_string())),
        }
        Ok(operation_id)
    }

    pub(crate) async fn report_member_progress(
        &self,
        member_ref: &MemberRef,
        message: impl Into<String>,
    ) -> Result<(), MobError> {
        let session_id = Self::require_member_session(member_ref, "report progress for")?;
        let display_name = format!("mob_member/{session_id}");
        let operation_id = self
            .resolve_or_register_active_operation_id(&session_id, "report progress", &display_name)
            .await?;
        self.ensure_running_before_live_update(&operation_id, "report progress")?;
        match self.registry.report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: message.into(),
                percent: None,
            },
        ) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::InvalidTransition { status, .. }) if status.is_terminal() => {
                Ok(())
            }
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }

    pub(crate) async fn mark_member_peer_ready(
        &self,
        member_ref: &MemberRef,
        peer_name: &str,
        trusted_peer: TrustedPeerSpec,
    ) -> Result<(), MobError> {
        let session_id = Self::require_member_session(member_ref, "mark peer ready for")?;
        let operation_id = self
            .resolve_or_register_active_operation_id(&session_id, "mark peer ready", peer_name)
            .await?;
        self.ensure_running_before_live_update(&operation_id, "mark peer ready")?;
        match self.registry.peer_ready(
            &operation_id,
            OperationPeerHandle {
                peer_name: peer_name.to_string(),
                trusted_peer,
            },
        ) {
            Err(OpsLifecycleError::InvalidTransition { status, .. }) if status.is_terminal() => {
                Ok(())
            }
            Ok(()) | Err(OpsLifecycleError::AlreadyPeerReady(_)) => Ok(()),
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }

    pub(crate) async fn mark_member_retired(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_member_session(member_ref, "mark retired for")?;
        let snapshots = self.matching_operations_for_session(&session_id);
        let active_ids = Self::active_operation_ids(&snapshots);
        let operation_id = match active_ids.len() {
            1 => active_ids[0].clone(),
            0 => {
                if let Some(latest) = Self::newest_operation_snapshot(&snapshots)
                    && latest.status == OperationStatus::Retired
                {
                    return Ok(());
                }
                let display_name = format!("mob_member/{session_id}");
                self.ensure_operation(&session_id, &display_name).await?
            }
            _ => {
                return Err(MobError::Internal(format!(
                    "cannot retire session '{session_id}': multiple active mob child operations [{}]",
                    Self::format_operation_ids(&active_ids)
                )));
            }
        };
        match self.registry.request_retire(&operation_id) {
            Ok(()) => {}
            Err(OpsLifecycleError::InvalidTransition {
                status: OperationStatus::Retiring | OperationStatus::Retired,
                ..
            }) => {}
            Err(error) => return Err(MobError::Internal(error.to_string())),
        }
        match self.registry.mark_retired(&operation_id) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::InvalidTransition {
                status: OperationStatus::Retired,
                ..
            }) => Ok(()),
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::comms::TrustedPeerSpec;
    use meerkat_core::ops_lifecycle::OperationStatus;

    #[tokio::test]
    #[ignore = "Phase 4 shared lifecycle suite"]
    async fn ops_registry_integration_red_ok_member_adapter_tracks_peer_ready_and_retire() {
        let adapter = MobOpsAdapter::new();
        let session_id = SessionId::new();
        let member_ref = MemberRef::from_session_id(session_id.clone());

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
                TrustedPeerSpec::new(
                    "mob-a/orchestrator/member-alpha",
                    "peer-member-alpha",
                    "inproc://member-alpha",
                )
                .expect("trusted peer"),
            )
            .await
            .expect("peer ready");

        let running_snapshot = adapter
            .registry()
            .snapshot(&operation_id)
            .expect("snapshot");
        assert_eq!(running_snapshot.status, OperationStatus::Running);
        assert!(running_snapshot.peer_ready);
        assert_eq!(running_snapshot.progress_count, 1);

        adapter
            .mark_member_retired(&member_ref)
            .await
            .expect("retire member");

        let retired_snapshot = adapter
            .registry()
            .snapshot(&operation_id)
            .expect("snapshot");
        assert_eq!(retired_snapshot.status, OperationStatus::Retired);
    }
}
