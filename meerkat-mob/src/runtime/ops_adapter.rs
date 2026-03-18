use super::*;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationPeerHandle, OperationProgressUpdate, OperationSpec,
    OperationStatus, OpsLifecycleError, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeOpsLifecycleRegistry;
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Debug, Default)]
pub(crate) struct MobOpsAdapter {
    registry: Arc<RuntimeOpsLifecycleRegistry>,
    operation_ids: RwLock<HashMap<SessionId, OperationId>>,
}

impl MobOpsAdapter {
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(RuntimeOpsLifecycleRegistry::new()),
            operation_ids: RwLock::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn registry(&self) -> Arc<RuntimeOpsLifecycleRegistry> {
        Arc::clone(&self.registry)
    }

    pub(crate) async fn operation_id_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<OperationId> {
        self.operation_ids.read().await.get(session_id).cloned()
    }

    async fn ensure_operation(
        &self,
        session_id: &SessionId,
        display_name: &str,
    ) -> Result<OperationId, MobError> {
        if let Some(existing) = self.operation_id_for_session(session_id).await {
            return Ok(existing);
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
        self.operation_ids
            .write()
            .await
            .insert(session_id.clone(), operation_id.clone());
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
        let Some(session_id) = member_ref.session_id() else {
            return Ok(());
        };
        let Some(operation_id) = self.operation_id_for_session(session_id).await else {
            return Ok(());
        };
        match self.registry.report_progress(
            &operation_id,
            OperationProgressUpdate {
                message: message.into(),
                percent: None,
            },
        ) {
            Ok(()) => Ok(()),
            Err(OpsLifecycleError::InvalidTransition { .. }) => Ok(()),
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }

    pub(crate) async fn mark_member_peer_ready(
        &self,
        member_ref: &MemberRef,
        peer_name: &str,
        trusted_peer: TrustedPeerSpec,
    ) -> Result<(), MobError> {
        let Some(session_id) = member_ref.session_id() else {
            return Ok(());
        };
        let Some(operation_id) = self.operation_id_for_session(session_id).await else {
            return Ok(());
        };
        match self.registry.peer_ready(
            &operation_id,
            OperationPeerHandle {
                peer_name: peer_name.to_string(),
                trusted_peer,
            },
        ) {
            Ok(()) | Err(OpsLifecycleError::AlreadyPeerReady(_)) => Ok(()),
            Err(error) => Err(MobError::Internal(error.to_string())),
        }
    }

    pub(crate) async fn mark_member_retired(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let Some(session_id) = member_ref.session_id() else {
            return Ok(());
        };
        let Some(operation_id) = self.operation_id_for_session(session_id).await else {
            return Ok(());
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
