//! Application commissioning through the installed Driver's existing owners.
use super::*;
use meerkat_authorization_contracts::{
    constraints::{
        ActionRef, DelegationDepth, DomainRestriction, ExactRestriction, ExecutionRestrictions,
        LifetimeRestriction, ProcessorRef,
    },
    evidence::EvidenceId,
    work_association::GrantTarget,
};
use meerkat_core::MobMemberBinding;
use meerkat_mob::{MobRuntimeMode, ProfileName};
use meerkat_mobkit::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RestoreOutcome,
};
use meerkat_toolkit::{
    host_lifetime::policy::{
        CompiledMemberClassInstance, CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
        PreparedNativeMember,
    },
    setup::{SetupWorkBackend, SetupWorkflowTerms},
};
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, WorkEdgeKind, WorkItemRef,
    WorkOwner, WorkOwnerKey,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use toolkit_mobkit::{
    host_gateway::{MemberClassAddError, MemberClassAddStart},
    installed_host::{
        operator_setup::{SetupTargetRequest, WorkflowHelperAuthorityRequest},
        preparation::{PreparedControllerIntent, PreparedControllerTarget},
    },
    private_originals::{
        PrivateAuthenticatedEvent, PrivateOriginalSubmission, PrivateOriginalSubmit,
        PrivateSourceEvidence, WorkflowChildLaunchRequest,
    },
};
use toolkit_package::{ContentDigest, canonical_json};

fn evidence(s: &str) -> EvidenceId {
    EvidenceId::new(s).expect("fixed acceptance evidence")
}
fn event(route: &str, text: &str, contributors: Vec<EvidenceId>) -> PrivateAuthenticatedEvent {
    PrivateAuthenticatedEvent {
        route: id(route),
        original: evidence(&format!("driver-{route}")),
        occurrence: evidence(&format!("driver-{route}-occurrence")),
        original_authentication: PrivateSourceEvidence {
            resource_id: evidence(&format!("driver-{route}-authentication")),
            revision: evidence("1"),
            sha256: digest(format!("trusted Driver launch {route}").as_bytes()),
        },
        root_event: PrivateSourceEvidence {
            resource_id: evidence(&format!("driver-{route}-root")),
            revision: evidence("1"),
            sha256: digest(text.as_bytes()),
        },
        contributors,
        content_sha256: digest(text.as_bytes()),
    }
}
fn require_admitted(value: &PrivateOriginalSubmission) -> Result<(), String> {
    if matches!(
        value,
        PrivateOriginalSubmission::Admitted {
            deduplicated: false,
            source_settlement:
                toolkit_mobkit::private_originals::PrivateSourceSettlement::FreshAcceptancePromoted,
            turn_ticket: Some(_),
            ..
        }
    ) {
        Ok(())
    } else {
        Err("actual fresh native admission and promotion required".into())
    }
}
async fn claimed(
    backend: &SetupWorkBackend,
    title: &str,
    owner: WorkOwnerKey,
) -> Result<WorkItemRef, String> {
    let created = backend
        .service()
        .create(CreateWorkItemRequest {
            realm_id: Some(backend.pin().realm().to_string()),
            namespace: Some(backend.pin().namespace().clone()),
            title: title.into(),
            labels: ["driver-shared-cycle".into()].into(),
            ..Default::default()
        })
        .await
        .map_err(error)?;
    let claimed = backend
        .service()
        .claim(ClaimWorkItemRequest {
            id: created.id,
            realm_id: Some(created.realm_id),
            namespace: Some(created.namespace),
            expected_revision: created.revision,
            owner: WorkOwner {
                key: owner,
                display_name: None,
            },
            lease_seconds: None,
            lease_expires_at: None,
        })
        .await
        .map_err(error)?;
    Ok(WorkItemRef {
        realm_id: claimed.realm_id,
        namespace: claimed.namespace,
        item_id: claimed.id,
    })
}
impl LaunchOwner {
    pub(super) async fn launch(&self) -> Result<Value, String> {
        let runtime = self.runtime().map_err(error)?;
        let source = runtime.host().workflow_source().map_err(error)?;
        let backend = runtime.work_backend().map_err(error)?;
        let parent_owner = WorkOwnerKey::agent("driver-parent-owner").map_err(error)?;
        let parent_work = claimed(
            &backend,
            "Two independently launched Driver tasks",
            parent_owner.clone(),
        )
        .await?;
        let p_event = event("planning-parent", P_TEXT, vec![]);
        let p = source.retain_event(p_event.clone()).await.map_err(error)?;
        let p_submit = PrivateOriginalSubmit {
            original: p.clone(),
            content: P_TEXT.into(),
            transport_authentication: p_event.original_authentication,
        };
        let controller = runtime
            .host()
            .prepare_local_controller_setup(p_submit.clone())
            .await
            .map_err(error)?;
        let operator = runtime
            .host()
            .application_operator_setup_for(&controller)
            .map_err(error)?;
        let p_result = runtime
            .host()
            .submit_authenticated_original(p_submit)
            .await
            .map_err(error)?;
        require_admitted(&p_result)?;

        let class = &self.plan.application.member_classes[0];
        if !matches!(
            class.purpose,
            meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow
        ) || class.profile != "shared-planner"
            || !class.callbacks.is_empty()
        {
            return Err("exact packaged shared class required".into());
        }
        let parent_route = self
            .plan
            .owner
            .native_host
            .routes
            .iter()
            .find(|row| row.id == p.route)
            .ok_or("selected P route missing")?;
        let CompiledNativeSourceOrigin::LocalConsole {
            requester,
            ingress_actor,
            ..
        } = &parent_route.origin
        else {
            return Err("exact test host local source selection required".into());
        };
        let executor = crate_principal("planning-lead")?;
        let member = PreparedNativeMember {
            member: MobMemberBinding {
                mob_id: "fixture".into(),
                role: class.profile.clone(),
                member: "planning-lead".into(),
            },
            logical_owner: executor.clone(),
            context_generation: 1,
            audience: meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                principal: requester.clone(),
            },
            setup_capability: None,
        };
        let instance = CompiledMemberClassInstance {
            class: class.id.clone(),
            member: member.clone(),
        };
        let route = CompiledNativeSourceRoute {
            id: id("planning-lead"),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member: member.member.clone(),
                requester: requester.clone(),
                ingress_actor: ingress_actor.clone(),
            },
            application_parent: None,
            controller_parent: None,
            logical_executor: executor.clone(),
            represented_subject: parent_route.represented_subject.clone(),
            source_authority: parent_route.source_authority.clone(),
            authentication_domain: parent_route.authentication_domain.clone(),
            event_domain: parent_route.event_domain.clone(),
        };
        let g_event = event("planning-lead", G_TEXT, vec![p.original.clone()]);
        let prepared_source = runtime
            .host()
            .retain_workflow_member_source(instance.clone(), &p, route.clone(), g_event.clone())
            .await
            .map_err(error)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(error)?
            .as_millis() as u64;
        // This finite independent selection is host-authored before G exists.
        // Every eventual operation also needs the exact caller's effect leaf.
        let operations = class
            .grants
            .iter()
            .filter(|grant| {
                ![
                    "tool_catalog_search",
                    "tool_catalog_load",
                    "toolkit_request_public_service",
                ]
                .contains(&grant.tool_name.as_str())
            })
            .map(|grant| grant.tool_name.as_str())
            .collect::<Vec<_>>();
        if [
            "setup_prepare",
            "setup_help",
            "setup_check",
            "setup_install",
            "setup_job",
            "setup_reconcile",
        ]
        .iter()
        .any(|required| !operations.contains(required))
            || class
                .grants
                .iter()
                .any(|grant| grant.action != meerkat_core::CompiledMemberToolAction::Invoke)
        {
            return Err("packaged class does not select this finite setup surface".into());
        }
        let mut actions = operations
            .iter()
            .map(|name| ActionRef {
                feature: "mcp".into(),
                action: (*name).into(),
            })
            .collect::<Vec<_>>();
        actions.extend([
            ActionRef {
                feature: "toolkit".into(),
                action: "catalog-read".into(),
            },
            ActionRef {
                feature: "toolkit".into(),
                action: "setup-public-service-request".into(),
            },
            meerkat_authorization::mcp_installation::mcp_connector_action(
                "toolkit-setup",
                &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
            ),
        ]);
        let envelope = ExecutionRestrictions {
            actions: ExactRestriction::exact(actions),
            resource_domains: DomainRestriction::owned_by([self
                .plan
                .owner
                .native_host
                .grant_administrator
                .clone()]),
            processors: ExactRestriction::exact([ProcessorRef::Principal {
                principal: executor,
            }]),
            audiences: ExactRestriction::exact([member.audience.clone()]),
            lifetime: LifetimeRestriction::window(now, now + 600_000),
            delegation_depth: DelegationDepth::remaining(0),
        };
        let terms = SetupWorkflowTerms {
            key: id("driver-shared-cohort"),
            backend: backend.pin().clone(),
            parent: parent_work.clone(),
            parent_owner: parent_owner.clone(),
            cycle_label: "driver-shared-cycle".into(),
            requester: requester.clone(),
            realm: self.plan.owner.native_host.realm.clone(),
            authority_revision: runtime.authority_revision().map_err(error)?,
            planning_source: prepared_source.source_key().clone(),
            lead: member.member.clone(),
            envelope_digest: ContentDigest::of_bytes(&canonical_json(&envelope).map_err(error)?),
            deadline_ms: now + 540_000,
            max_waiters: 2,
        };
        let admitted = runtime
            .host()
            .reserve_workflow_member_class(id("driver-shared-g"), &prepared_source, &terms)
            .await
            .map_err(error)?;
        let resources = meerkat_connect::resources::InstalledResources::locate().map_err(error)?;
        let storage = self.plan.application.realm.resolve().map_err(error)?;
        let physical = match resources
            .prepare_with_shared_setup(&storage, 1, std::future::pending())
            .await
        {
            Ok(value) => value,
            Err(failure) => {
                let cleanup = runtime
                    .host()
                    .retire_unaccepted_member_class(admitted.clone())
                    .await;
                return Err(format!(
                    "shared preparation failed: {failure}; canonical retirement: {cleanup:?}"
                ));
            }
        };
        if let Err((owner, failure)) = runtime.retain_helper_resources(admitted.clone(), physical) {
            let close = owner.close().await;
            let retire = runtime
                .host()
                .retire_unaccepted_member_class(admitted.clone())
                .await;
            return Err(format!(
                "resource transfer failed: {failure}; close: {close:?}; retirement: {retire:?}"
            ));
        }
        let spec = DurableAgentSpec {
            identity: AgentIdentity::parse("planning-lead").map_err(error)?,
            profile: ProfileName::from(class.profile.clone()),
            addressability: AgentAddressability::InternalOnly,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: vec![],
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        };
        let add = runtime
            .add_helper(
                admitted.clone(),
                spec,
                route.clone(),
                PreparedControllerIntent {
                    target: PreparedControllerTarget::OriginalRoute {
                        route: route.id.clone(),
                    },
                    issuance_id: evidence("driver-g-controller"),
                    controller_route: None,
                },
                tokio::time::Instant::now() + Duration::from_secs(60),
                || async { Ok(()) },
            )
            .await;
        let add = match add {
            Ok(value) => value,
            Err(failure) => {
                if matches!(failure, MemberClassAddError::ReservationNotAccepted { .. }) {
                    let cleanup = runtime.abandon_helper(admitted.clone()).await;
                    return Err(format!(
                        "Add not accepted: {failure:?}; reservation cleanup: {cleanup:?}"
                    ));
                }
                return Err(format!("accepted Add refused: {failure:?}"));
            }
        };
        let outcome = match add {
            MemberClassAddStart::Settled(value) => value,
            MemberClassAddStart::Unsettled(observation) => {
                observation.settled().await.map_err(error)?
            }
        };
        if outcome.authorization.is_err()
            || outcome.cleanup.is_err()
            || !matches!(
                outcome.native.as_ref().ok().and_then(|result| result
                    .outcomes
                    .get(&AgentIdentity::parse("planning-lead").unwrap())),
                Some(RestoreOutcome::Created { .. })
            )
        {
            return Err(format!(
                "actual late G Created and authorization required: {outcome:?}"
            ));
        }
        let receipt = runtime
            .host()
            .completed_workflow_member_class(&admitted)
            .map_err(error)?;
        let server = runtime.helper_server(&admitted).map_err(error)?;
        if server.name != "toolkit-setup"
            || server.access.as_ref().is_none_or(|access| {
                access
                    .allowed_operations
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    != operations.iter().copied().collect()
            })
            || !receipt.native_surface.iter().any(|tool| {
                tool.provenance.as_ref().is_some_and(|p| {
                    p.kind == meerkat_core::ToolSourceKind::Mcp
                        && p.source_id.as_str() == server.name
                })
            })
        {
            return Err("actual fresh profile did not construct the retained setup peer".into());
        }
        let helper = runtime
            .configure_helper(
                &operator,
                admitted.clone(),
                &WorkflowHelperAuthorityRequest {
                    parent_issuance_id: evidence("driver-g-planning-root"),
                    leaf_issuance_id: evidence("driver-g-planning-leaf"),
                    envelope,
                },
            )
            .await
            .map_err(error)?;
        let _planning = source
            .prepare_late_workflow_planning(
                &p,
                PrivateOriginalSubmit {
                    original: prepared_source.original().clone(),
                    content: G_TEXT.into(),
                    transport_authentication: g_event.original_authentication,
                },
                terms.clone(),
                helper.clone(),
            )
            .await
            .map_err(error)?;
        let mut observations = Vec::new();
        // Both exact graph/source/effect selections are retained before either
        // first native submit. No caller can be retrofitted after Find starts.
        let mut submissions = Vec::new();
        for tag in ["a", "b"] {
            let name = format!("original-{tag}");
            let text = request(tag);
            let event = event(&name, &text, vec![]);
            let original = source.retain_event(event.clone()).await.map_err(error)?;
            let submission = PrivateOriginalSubmit {
                original: original.clone(),
                content: text,
                transport_authentication: event.original_authentication,
            };
            let child_owner = WorkOwnerKey::agent(format!("driver-{tag}-owner")).map_err(error)?;
            let child = claimed(&backend, &name, child_owner.clone()).await?;
            backend
                .service()
                .link(LinkWorkItemsRequest {
                    realm_id: Some(parent_work.realm_id.clone()),
                    namespace: Some(parent_work.namespace.clone()),
                    kind: WorkEdgeKind::Parent,
                    from_id: child.item_id.clone(),
                    to_id: parent_work.item_id.clone(),
                })
                .await
                .map_err(error)?;
            let selected = WorkflowChildLaunchRequest {
                parent: p.clone(),
                planning: prepared_source.original().clone(),
                submission: submission.clone(),
                child,
                child_owner,
                issuance_id: evidence(&format!("driver-{tag}-effects")),
                targets: vec![
                    SetupTargetRequest {
                        target: Some(GrantTarget::McpServer {
                            server: server.name.clone(),
                        }),
                        issuance_id: evidence(&format!("driver-{tag}-setup-mcp")),
                    },
                    SetupTargetRequest {
                        target: Some(GrantTarget::RegisteredToolBundleOperation {
                            bundle: meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE.into(),
                            operation: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST.into(),
                        }),
                        issuance_id: evidence(&format!("driver-{tag}-setup-public")),
                    },
                ],
                output_audiences: BTreeSet::from([id("fixture-output")]),
            };
            // A different retained plan selector cannot borrow P/G custody.
            let mut foreign = selected.clone();
            foreign.planning = p.clone();
            if self
                .private_launcher
                .prepare_private_child(foreign)
                .await
                .is_ok()
            {
                return Err("different planning source cannot supply this child custody".into());
            }
            // Exercise the supplied production adapter, not an acceptance map.
            self.private_launcher
                .prepare_private_child(selected.clone())
                .await
                .map_err(error)?;
            self.private_launcher
                .prepare_private_child(selected.clone())
                .await
                .map_err(error)?;
            let mut changed = selected.clone();
            changed.child_owner = WorkOwnerKey::agent("not-the-launched-owner").map_err(error)?;
            if self
                .private_launcher
                .prepare_private_child(changed)
                .await
                .is_ok()
            {
                return Err("changed child claim must refuse".into());
            }
            let mut changed = selected.clone();
            changed.issuance_id = evidence(&format!("changed-{tag}-effects"));
            if self
                .private_launcher
                .prepare_private_child(changed)
                .await
                .is_ok()
            {
                return Err("configured child correlation must be immutable".into());
            }
            let mut changed = selected.clone();
            changed.output_audiences.insert(id("unselected-output"));
            if self
                .private_launcher
                .prepare_private_child(changed)
                .await
                .is_ok()
            {
                return Err("private request cannot widen output audience".into());
            }
            submissions.push((submission, selected));
        }
        for (submission, selected) in submissions {
            let original = submission.original.clone();
            let result = runtime
                .host()
                .submit_authenticated_original(submission)
                .await
                .map_err(error)?;
            require_admitted(&result)?;
            // Exact replay after actual binding observes the same selection;
            // it neither reissues effects nor resubmits the native input.
            self.private_launcher
                .prepare_private_child(selected.clone())
                .await
                .map_err(error)?;
            let mut changed = selected;
            changed.issuance_id = evidence("changed-after-native-binding");
            if self
                .private_launcher
                .prepare_private_child(changed)
                .await
                .is_ok()
            {
                return Err("bound child correlation cannot be replaced".into());
            }
            let source_key = source.source_record_key(&original).await.map_err(error)?;
            observations
                .push(json!({"original":original,"source_key":source_key,"admission":result}));
        }
        Ok(
            json!({"parent":{"original":p,"admission":p_result},"children":observations,
            "workflow":terms,"g_session":receipt.workflow_member.session().map_err(error)?,
            "g_surface":receipt.native_surface,"initial_members":4,"initial_g_members":0,
            "native_add":"created","same_bootstrap_workgraph":true,
            "canonical_child_launch":true,"exact_launch_replay":true,"bound_launch_replay":true,"foreign_plan_refused":true,
            "changed_child_claim_refused":true,"changed_launch_correlation_refused":true,"audience_widening_refused":true}),
        )
    }
}
fn crate_principal(name: &str) -> Result<meerkat_core::PrincipalRef, String> {
    serde_json::from_value(json!({"kind":"service_account","id":name,"qualification":{
        "kind":"qualified","trust_domain_id":"installed-gateway-fixture"}}))
    .map_err(error)
}
