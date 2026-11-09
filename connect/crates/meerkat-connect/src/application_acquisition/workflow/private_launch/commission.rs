//! Supplied fresh G commissioning through the actual Driver and private ACK.
//! The existing pipe owns this future through EOF. Native Add owns its entered
//! callback and cleanup; no transport result is interpreted as native rollback.
use super::*;
use meerkat_mobkit::identity_first::{AgentAddressability, DurableAgentSpec, RestoreOutcome};
use meerkat_toolkit::setup::SetupWorkflowTerms;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toolkit_mobkit::{
    host_gateway::{MemberClassAddError, MemberClassAddStart},
    installed_host::{
        operator_setup::WorkflowHelperAuthorityRequest,
        preparation::{PreparedControllerIntent, PreparedControllerTarget},
    },
    private_host::{
        MemberClassResponse, WorkflowCommissionRequest, WorkflowCommissionResponse,
        WorkflowMemberPublication,
    },
    private_originals::PrivateOriginalSubmit,
};
use toolkit_package::{ContentDigest, canonical_json};
mod selection;

impl PrivateApplicationWorkflowLauncher {
    pub(super) async fn prepare_parent(
        &self,
        request: toolkit_mobkit::private_host::WorkflowParentRequest,
    ) -> Result<(), HostLifetimeError> {
        let (selected, host) = self.selected()?;
        let source = host.workflow_source()?;
        let native = host.workflow_native_selection()?;
        let now = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|_| HostLifetimeError::Unavailable)?
                .as_millis(),
        )
        .map_err(|_| HostLifetimeError::Unavailable)?;
        let commission = &request.commission;
        let ids = [
            &request.parent_root_issuance_id,
            &request.parent_leaf_issuance_id,
            &commission.controller_issuance_id,
            &commission.planning_root_issuance_id,
            &commission.planning_leaf_issuance_id,
        ];
        if ids
            .iter()
            .enumerate()
            .any(|(index, id)| ids[..index].contains(id))
        {
            return Err(HostLifetimeError::Denied);
        }
        let chosen = selection::select(&native, commission, now)?;
        selection::check_parent_work(&selected.backend, commission).await?;
        let event = source
            .event(&commission.parent.original)
            .await
            .map_err(source_error)?;
        if event.original_authentication != commission.parent.transport_authentication
            || event.content_sha256
                != ContentDigest::of_bytes(commission.parent.content.as_bytes()).to_string()
        {
            return Err(HostLifetimeError::Denied);
        }
        let controller = host
            .prepare_local_controller_setup(commission.parent.clone())
            .await
            .map_err(source_error)?;
        let operator = host.application_operator_setup_for(&controller)?;
        let intent =
            toolkit_mobkit::installed_host::operator_setup::WorkflowParentAuthoritySelection {
                commission_sha256: commission_digest(commission)?,
                instance: commission.instance.clone(),
                authority: WorkflowHelperAuthorityRequest {
                    parent_issuance_id: request.parent_root_issuance_id,
                    leaf_issuance_id: request.parent_leaf_issuance_id,
                    envelope: chosen.envelope,
                },
            };
        operator
            .configure_workflow_parent(&source, &commission.parent.original, &intent)
            .await
    }

    pub(super) async fn commission(
        &self,
        request: WorkflowCommissionRequest,
        publication: WorkflowMemberPublication,
    ) -> Result<WorkflowCommissionResponse, HostLifetimeError> {
        let (selected, host) = self.selected()?;
        let runtime = selected.runtime(host.clone());
        let source = host.workflow_source()?;
        let native = host.workflow_native_selection()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| HostLifetimeError::Unavailable)?
            .as_millis();
        let now = u64::try_from(now).map_err(|_| HostLifetimeError::Unavailable)?;
        let chosen = selection::select(&native, &request, now)?;
        let workflow_deadline =
            tokio::time::Instant::now() + Duration::from_millis(request.deadline_ms - now);
        selection::check_parent_work(&selected.backend, &request).await?;
        let event = source
            .event(&request.parent.original)
            .await
            .map_err(source_error)?;
        if event.original_authentication != request.parent.transport_authentication
            || event.content_sha256
                != ContentDigest::of_bytes(request.parent.content.as_bytes()).to_string()
        {
            return Err(HostLifetimeError::Denied);
        }
        let controller = host
            .prepare_local_controller_setup(request.parent.clone())
            .await
            .map_err(source_error)?;
        let operator = host.application_operator_setup_for(&controller)?;
        source
            .check_workflow_parent_commission(
                &request.parent.original,
                &commission_digest(&request)?,
            )
            .await
            .map_err(source_error)?;
        // This checks actual retained Admitted P, immutable source bytes and
        // the exact approved SharedWorkflow class. No G route is projected.
        let prepared = host
            .retain_workflow_member_source(
                request.instance.clone(),
                &request.parent.original,
                chosen.route.clone(),
                request.event.clone(),
            )
            .await?;
        let terms = SetupWorkflowTerms {
            key: request.workflow.clone(),
            backend: selected.backend.pin().clone(),
            parent: request.parent_work.clone(),
            parent_owner: request.parent_owner.clone(),
            cycle_label: request.cycle_label.clone(),
            requester: chosen.requester,
            realm: native.realm.clone(),
            authority_revision: runtime.authority_revision()?,
            planning_source: prepared.source_key().clone(),
            lead: request.instance.member.member.clone(),
            envelope_digest: ContentDigest::of_bytes(
                &canonical_json(&chosen.envelope)
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
            ),
            deadline_ms: request.deadline_ms,
            max_waiters: request.max_waiters,
        };
        // Same-ID replay has no original Add token. It refuses before another
        // physical peer is prepared and cannot clean up the accepted attempt.
        let admitted = host
            .reserve_fresh_workflow_member_class(request.admission_id.clone(), &prepared, &terms)
            .await?;
        let resources = match crate::resources::InstalledResources::locate() {
            Ok(value) => value,
            Err(_) => {
                return Ok(WorkflowCommissionResponse::PreparationRefused {
                    error: HostLifetimeError::Unavailable.into(),
                    retirement: host.retire_unaccepted_member_class(admitted).await.into(),
                    physical_cleanup: None,
                });
            }
        };
        let storage = match selected.realm.resolve() {
            Ok(value) => value,
            Err(_) => {
                return Ok(WorkflowCommissionResponse::PreparationRefused {
                    error: HostLifetimeError::InvalidSelection.into(),
                    retirement: host.retire_unaccepted_member_class(admitted).await.into(),
                    physical_cleanup: None,
                });
            }
        };
        // Signal the preparer's existing joined cancellation path. Never drop
        // its entered work because the response receiver or private pipe left.
        let physical = match resources
            .prepare_with_shared_setup(&storage, 1, async {
                tokio::select! {
                    _ = publication.closed() => (),
                    _ = tokio::time::sleep_until(workflow_deadline) => (),
                }
                Ok(())
            })
            .await
        {
            Ok(value) => value,
            Err(_) => {
                return Ok(WorkflowCommissionResponse::PreparationRefused {
                    error: HostLifetimeError::Unavailable.into(),
                    retirement: host.retire_unaccepted_member_class(admitted).await.into(),
                    // No physical owner was returned. This does not claim that an
                    // unavailable preparer proved physical settlement.
                    physical_cleanup: None,
                });
            }
        };
        if let Err((physical, error)) = runtime.retain_helper_resources(admitted.clone(), physical)
        {
            let physical_cleanup = physical
                .close()
                .await
                .map_err(|_| HostLifetimeError::NotSettled);
            let retirement = host.retire_unaccepted_member_class(admitted).await;
            return Ok(WorkflowCommissionResponse::PreparationRefused {
                error: error.into(),
                retirement: retirement.into(),
                physical_cleanup: Some(physical_cleanup.into()),
            });
        }
        let server = runtime.helper_server(&admitted);
        if server.as_ref().map_or(true, |server| {
            !selection::matching_peer(server, &chosen.operations)
        }) {
            let retirement = runtime.abandon_helper(admitted).await;
            return Ok(WorkflowCommissionResponse::PreparationRefused {
                error: HostLifetimeError::InvalidSelection.into(),
                retirement: retirement.into(),
                // The same accepted retirement task owns physical cleanup;
                // its combined result above is not duplicated as a success.
                physical_cleanup: None,
            });
        }
        let spec = DurableAgentSpec {
            identity: request.identity.clone(),
            profile: meerkat_mob::ProfileName::from(chosen.profile),
            addressability: AgentAddressability::InternalOnly,
            display_name: None,
            labels: Default::default(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        };
        let deadline = (tokio::time::Instant::now()
            + Duration::from_millis(request.hold_timeout_ms))
        .min(workflow_deadline);
        let publication_admitted = admitted.clone();
        let publication_identity = request.identity.clone();
        let add = runtime
            .add_helper(
                admitted.clone(),
                spec,
                chosen.route.clone(),
                PreparedControllerIntent {
                    target: PreparedControllerTarget::OriginalRoute {
                        route: chosen.route.id.clone(),
                    },
                    issuance_id: request.controller_issuance_id.clone(),
                    controller_route: None,
                },
                deadline,
                move || async move {
                    publication
                        .publish(&publication_admitted, &publication_identity, deadline)
                        .await
                },
            )
            .await;
        let start = match add {
            Ok(start) => start,
            Err(error @ MemberClassAddError::ReservationNotAccepted { .. }) => {
                // This handle was created by this invocation, but Add never
                // accepted it. Transfer through the existing one-shot retire.
                let retirement = runtime.abandon_helper(admitted).await;
                return Ok(WorkflowCommissionResponse::AddNotAccepted {
                    native: add_refusal(error)?,
                    retirement: retirement.into(),
                });
            }
            Err(error) => {
                return Ok(WorkflowCommissionResponse::Native {
                    outcome: add_refusal(error)?,
                });
            }
        };
        let outcome = match start {
            MemberClassAddStart::Settled(outcome) => outcome,
            // The already accepted private JoinSet retains observation through
            // eventual native settlement, including a lost response receiver.
            MemberClassAddStart::Unsettled(observation) => observation.settled().await?,
        };
        let created = outcome.authorization.is_ok()
            && outcome.cleanup.is_ok()
            && matches!(
                outcome
                    .native
                    .as_ref()
                    .ok()
                    .and_then(|value| value.outcomes.get(&request.identity)),
                Some(RestoreOutcome::Created { .. })
            );
        let native: MemberClassResponse = outcome.into();
        if !created {
            return Ok(WorkflowCommissionResponse::Native { outcome: native });
        }
        let planning = prepared.original().clone();
        let parent = request.parent.original.clone();
        let admission_id = request.admission_id.clone();
        let finish = async {
            selection::check_parent_work(&selected.backend, &request).await?;
            let helper = runtime
                .configure_helper(
                    &operator,
                    admitted,
                    &WorkflowHelperAuthorityRequest {
                        parent_issuance_id: request.planning_root_issuance_id,
                        leaf_issuance_id: request.planning_leaf_issuance_id,
                        envelope: chosen.envelope,
                    },
                )
                .await?;
            source
                .prepare_late_workflow_planning(
                    &request.parent.original,
                    PrivateOriginalSubmit {
                        original: planning.clone(),
                        content: request.content,
                        transport_authentication: request.event.original_authentication,
                    },
                    terms,
                    helper,
                )
                .await
                .map_err(source_error)?;
            Ok::<(), HostLifetimeError>(())
        }
        .await;
        match finish {
            Ok(()) => Ok(WorkflowCommissionResponse::Prepared {
                parent,
                planning,
                admission_id,
                native,
            }),
            Err(error) => Ok(WorkflowCommissionResponse::CreatedWithoutPlanning {
                parent,
                planning,
                admission_id,
                native,
                error: error.into(),
            }),
        }
    }
}

fn add_refusal(error: MemberClassAddError) -> Result<MemberClassResponse, HostLifetimeError> {
    match error {
        MemberClassAddError::Host(error) => Err(error),
        MemberClassAddError::ReservationNotAccepted { error } => {
            Ok(MemberClassResponse::ReservationNotAccepted {
                error: error.into(),
            })
        }
        MemberClassAddError::Preflight { error, cleanup } => {
            Ok(MemberClassResponse::PreflightRefused {
                error: error.into(),
                cleanup: cleanup.into(),
            })
        }
        MemberClassAddError::Native(error) => Ok(MemberClassResponse::NativeRefused {
            message: error.to_string(),
            data: error.structured_data(),
        }),
    }
}

fn commission_digest(request: &WorkflowCommissionRequest) -> Result<String, HostLifetimeError> {
    Ok(ContentDigest::of_bytes(
        &canonical_json(request).map_err(|_| HostLifetimeError::InvalidSelection)?,
    )
    .to_string())
}
