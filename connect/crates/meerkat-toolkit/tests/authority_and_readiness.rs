//! Adversarial contract tests. These fixtures exercise local validation; they
//! do not certify live Meerkat bindings, provider accounts, or host migrations.

// Reuse the exact lower-crate data fixture. Its current owner refuses every
// operation; matching retained evidence is not native admission or permission.
#[path = "../src/authority/test_support.rs"]
mod native_fixture;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, NativeWorkTarget,
};
use meerkat_core::{
    auth::{PrincipalKind, PrincipalRef},
    authorization::WorkAuthorizationContext,
    connection::RealmId,
};
fn principal(value: &str) -> PrincipalRef {
    native_fixture::principal(PrincipalKind::Human, value)
}
fn dest(value: &str) -> NativeWorkTarget {
    native_fixture::target(value, &principal("person-a"))
}

use meerkat_toolkit::authority::*;
use meerkat_toolkit::binding::*;
use std::collections::{BTreeMap, BTreeSet};
use toolkit_package::{ContentDigest, Id, Revision};

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn lifetime() -> RoleLifetime {
    RoleLifetime {
        scope: LifetimeScope::Member {
            member: id("agent-a"),
        },
        expires_at_ms: Some(900),
    }
}

fn ids(values: &[&str]) -> BTreeSet<Id> {
    values.iter().map(|v| id(v)).collect()
}

struct Fixture {
    tenant: Id,
    deployment: Id,
    agent: Id,
    destination: NativeWorkTarget,
    policy: Revision,
    realm: RealmId,
    native: WorkAuthorizationContext,
}

impl Fixture {
    fn new() -> Self {
        Self {
            tenant: id("tenant-a"),
            deployment: id("deployment-a"),
            agent: id("agent-a"),
            destination: dest("agent-a"),
            policy: rev("policy-4"),
            realm: RealmId::parse("core-realm").unwrap(),
            native: native_fixture::current(&native_fixture::association(
                principal("person-a"),
                dest("agent-a"),
                "private-conversation-a",
                "original-event",
            )),
        }
    }

    fn context<'a>(
        &'a self,
        destination: &'a NativeWorkTarget,
        now_ms: u64,
    ) -> SetupUseContext<'a> {
        SetupUseContext {
            tenant: &self.tenant,
            deployment: &self.deployment,
            target: destination,
            realm: &self.realm,
            authorization: &self.native,
            policy_revision: &self.policy,
            now_ms,
        }
    }

    fn authority(&self) -> SetupRequestEvidence {
        SetupRequestEvidence::new(
            native_fixture::association(
                principal("person-a"),
                self.destination.clone(),
                "private-conversation-a",
                "original-event",
            ),
            SetupProposalConstraints {
                tenant: self.tenant.clone(),
                deployment: self.deployment.clone(),
                policy_revision: self.policy.clone(),
                issued_at_ms: 100,
                expires_at_ms: 1000,
                ceiling: Ceiling {
                    actions: [
                        (Action::FindAndInstall, "agent-a"),
                        (Action::GrantService, "agent-a"),
                        (Action::ExposeFrontend, "agent-a"),
                        (Action::Delegate, "peer-one"),
                        (Action::Delegate, "peer-two"),
                        (Action::Delegate, "peer-three"),
                        (Action::Delegate, "peer"),
                    ]
                    .into_iter()
                    .map(|(action, resource)| ActionPermission {
                        action,
                        resource: id(resource),
                    })
                    .collect(),
                    services: [ServicePermission {
                        target: id("agent-a"),
                        connection: id("mail-account-a"),
                        operation: id("read"),
                        resource: id("inbox-a"),
                        lifetime: lifetime(),
                    }]
                    .into(),
                    frontends: [FrontendPermission {
                        target: id("agent-a"),
                        endpoint: id("chat-endpoint-a"),
                        participant: id("person-a"),
                        interaction: id("send"),
                        lifetime: lifetime(),
                    }]
                    .into(),
                    output_audiences: ids(&["person-a"]),
                },
            },
        )
        .unwrap()
    }

    fn service(&self) -> BindingIntent {
        BindingIntent {
            id: id("read-mail"),
            tenant: self.tenant.clone(),
            deployment: self.deployment.clone(),
            requester: principal("person-a"),
            target: self.agent.clone(),
            original_work: self.authority().original_work().clone(),
            manifest_digest: ContentDigest::of_bytes(b"selected-version-and-config"),
            revision: rev("intent-2"),
            policy_revision: self.policy.clone(),
            request_valid_until_ms: 900,
            lifetime: lifetime(),
            role: RoleRequirement::Service {
                connection: id("mail-account-a"),
                connection_revision: rev("account-revision-3"),
                access: [ServiceAccess {
                    operation: id("read"),
                    resource: id("inbox-a"),
                }]
                .into(),
                runtime: id("runtime-a"),
            },
        }
    }

    fn frontend(&self) -> BindingIntent {
        BindingIntent {
            id: id("private-frontend"),
            role: RoleRequirement::Frontend {
                endpoint: id("chat-endpoint-a"),
                participants: ids(&["person-a"]),
                audience: ids(&["person-a"]),
                interactions: ids(&["send"]),
                agent: self.agent.clone(),
                conversation_policy: rev("isolated-per-thread-1"),
            },
            ..self.service()
        }
    }
}

fn grant(intent: &BindingIntent) -> GrantReceipt {
    GrantReceipt {
        id: id(&format!("grant/{}", intent.id)),
        owner: id("policy-owner"),
        intent: intent.id.clone(),
        intent_revision: intent.revision.clone(),
        tenant: intent.tenant.clone(),
        deployment: intent.deployment.clone(),
        subject: intent.target.clone(),
        policy_revision: intent.policy_revision.clone(),
        manifest_digest: intent.manifest_digest.clone(),
        original_work: intent.original_work.clone(),
        intent_digest: intent.digest().unwrap(),
        role: intent.role.clone(),
        lifetime: RoleLifetime {
            expires_at_ms: Some(850),
            ..intent.lifetime.clone()
        },
    }
}

fn receipt(intent: &BindingIntent) -> BindingReceipt {
    let evidence = match &intent.role {
        RoleRequirement::Service {
            runtime,
            connection_revision,
            ..
        } => RoleEvidence::Service {
            runtime: runtime.clone(),
            incarnation: id("incarnation-2"),
            catalog_revision: rev("catalog-5"),
            connection_revision: connection_revision.clone(),
        },
        RoleRequirement::Frontend {
            endpoint, audience, ..
        } => RoleEvidence::Frontend {
            endpoint: endpoint.clone(),
            admission_revision: rev("admission-2"),
            conversation_revision: rev("conversation-3"),
            delivery_revision: rev("delivery-2"),
            verified_audience: audience.clone(),
        },
    };
    BindingReceipt {
        id: id(&format!("receipt/{}", intent.id)),
        owner: id("runtime-owner"),
        intent: intent.id.clone(),
        intent_revision: intent.revision.clone(),
        grant: grant(intent).id,
        intent_digest: intent.digest().unwrap(),
        grant_digest: grant(intent).digest().unwrap(),
        manifest_digest: intent.manifest_digest.clone(),
        policy_revision: intent.policy_revision.clone(),
        observed_at_ms: 190,
        expires_at_ms: 800,
        evidence,
    }
}

#[derive(Default)]
struct Owners {
    grants: BTreeMap<Id, GrantReceipt>,
    bindings: BTreeMap<Id, BindingReceipt>,
    unavailable: bool,
}

impl Owners {
    fn ready(intents: &[BindingIntent]) -> Self {
        Self {
            grants: intents.iter().map(|i| (i.id.clone(), grant(i))).collect(),
            bindings: intents.iter().map(|i| (i.id.clone(), receipt(i))).collect(),
            unavailable: false,
        }
    }
}

impl GrantOwner for Owners {
    fn current_grant(
        &self,
        intent: &BindingIntent,
        _: u64,
    ) -> Result<Option<GrantReceipt>, BindingError> {
        if self.unavailable {
            return Err(BindingError::OwnerUnavailable(
                "policy lookup failed".into(),
            ));
        }
        Ok(self.grants.get(&intent.id).cloned())
    }
}

impl BindingOwner for Owners {
    fn current_binding(
        &self,
        intent: &BindingIntent,
        _: &GrantReceipt,
        _: u64,
    ) -> Result<Option<BindingReceipt>, BindingError> {
        Ok(self.bindings.get(&intent.id).cloned())
    }
}

fn assert_no_operation_authority(native: &WorkAuthorizationContext) {
    use meerkat_core::{
        authorization::{
            AuthorizationOperation, OperationAuthorizationError, OperationAuthorizationFacts,
            PreparedAuthorizationBinding, PublicationAuthorizationFacts, PublicationMode,
            PublicationRecipient,
        },
        exact_operation::OperationExecutionScope,
        ops::OperationId,
    };
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Publication(PublicationAuthorizationFacts {
            recipient: PublicationRecipient::Principal(principal("person-a")),
            scope: None,
            live_channel: None,
            mode: PublicationMode::Buffered,
        }),
    });
    assert!(matches!(
        native.authorization().prepare(&binding),
        Err(OperationAuthorizationError::Unavailable)
    ));
}

#[test]
fn serialized_evidence_cannot_replace_current_native_identity_or_grant_permission() {
    let f = Fixture::new();
    let original = f.authority();
    let base = original.association().candidate();
    let mut candidates = Vec::new();
    let mut changed = base.clone();
    changed.requester = principal("administrator");
    candidates.push(changed);
    let mut changed = base.clone();
    changed.root_event = native_fixture::event("public-channel");
    candidates.push(changed);
    let mut changed = base.clone();
    changed.original_work = native_fixture::original("other-work");
    changed.contributing_work = vec![changed.original_work.clone()];
    candidates.push(changed);
    let mut changed = base.clone();
    changed.target = dest("privileged-agent");
    changed.logical_executor = changed.target.logical_owner.clone();
    candidates.push(changed);
    let mut changed = base.clone();
    changed.original_authentication = native_fixture::event("other-login");
    candidates.push(changed);
    for changed in candidates {
        let changed = SetupRequestEvidence::new(
            InputAuthorityAssociation::new(changed).unwrap(),
            original.constraints().clone(),
        )
        .unwrap();
        let decoded: SetupRequestEvidence =
            serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_eq!(
            decoded.check_context(&f.context(&f.destination, 200)),
            Err(SetupContextError::WrongContext)
        );
    }
    let mut constraints = original.constraints().clone();
    constraints.tenant = id("tenant-b");
    let changed = SetupRequestEvidence::new(original.association().clone(), constraints).unwrap();
    assert_eq!(
        changed.check_context(&f.context(&f.destination, 200)),
        Err(SetupContextError::WrongContext)
    );
    // Toolkit constraints are unsigned proposal data, not recovered grants.
    // Even a wider decoded proposal with matching native identity grants nothing.
    let mut constraints = original.constraints().clone();
    constraints.expires_at_ms = 99999;
    constraints.ceiling.actions.insert(ActionPermission {
        action: Action::InstallArtifacts,
        resource: f.agent.clone(),
    });
    let changed = SetupRequestEvidence::new(original.association().clone(), constraints).unwrap();
    let decoded: SetupRequestEvidence =
        serde_json::from_slice(&serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(
        decoded
            .check_proposal(
                Action::InstallArtifacts,
                &f.agent,
                &f.context(&f.destination, 200)
            )
            .is_ok()
    );
    assert_no_operation_authority(&f.native);
    for legacy in [
        serde_json::json!({"version": 0, "claims": {}, "signature": "old"}),
        serde_json::json!({"version": 1, "claims": {}, "signature": "old"}),
    ] {
        assert!(serde_json::from_value::<SetupRequestEvidence>(legacy).is_err());
    }
}

#[test]
fn host_context_not_envelope_selects_tenant_receiver_and_current_policy() {
    let f = Fixture::new();
    let evidence = f.authority();
    let other = id("tenant-b");
    let newer_policy = rev("policy-5");
    assert_eq!(
        evidence.check_context(&SetupUseContext {
            tenant: &other,
            ..f.context(&f.destination, 200)
        }),
        Err(SetupContextError::WrongContext)
    );
    assert_eq!(
        evidence.check_context(&f.context(&dest("other-agent"), 200)),
        Err(SetupContextError::WrongContext)
    );
    assert_eq!(
        evidence.check_context(&SetupUseContext {
            policy_revision: &newer_policy,
            ..f.context(&f.destination, 200)
        }),
        Err(SetupContextError::StalePolicy)
    );
    for now in [99, 1000, 1001] {
        assert_eq!(
            evidence.check_context(&f.context(&f.destination, now)),
            Err(SetupContextError::Expired)
        );
    }
    let foreign = native_fixture::current(&native_fixture::association(
        principal("person-b"),
        f.destination.clone(),
        "private-conversation-a",
        "original-event",
    ));
    assert_eq!(
        evidence.check_context(&SetupUseContext {
            authorization: &foreign,
            ..f.context(&f.destination, 200)
        }),
        Err(SetupContextError::WrongContext)
    );
    let missing = WorkAuthorizationContext::new(
        f.native.authorization().clone(),
        f.native.execution_scope().clone(),
    );
    assert_eq!(
        evidence.check_context(&SetupUseContext {
            authorization: &missing,
            ..f.context(&f.destination, 200)
        }),
        Err(SetupContextError::WrongContext)
    );
}

#[test]
fn peer_evidence_preserves_original_identity_but_requires_each_current_native_context() {
    let f = Fixture::new();
    let parent = f.authority();
    let mut previous = parent.clone();
    for (peer, expiry) in [("peer-one", 700), ("peer-two", 650)] {
        let mut candidate = previous.association().candidate().clone();
        candidate.target = dest(peer);
        candidate.logical_executor = candidate.target.logical_owner.clone();
        let mut constraints = previous.constraints().clone();
        constraints
            .ceiling
            .actions
            .retain(|right| right.action != Action::ExposeFrontend);
        constraints.expires_at_ms = expiry;
        assert!(
            previous
                .constraints()
                .ceiling
                .contains(&constraints.ceiling)
        );
        assert!(constraints.expires_at_ms <= previous.constraints().expires_at_ms);
        let evidence = SetupRequestEvidence::new(
            InputAuthorityAssociation::new(candidate).unwrap(),
            constraints,
        )
        .unwrap();
        let target = dest(peer);
        assert_eq!(
            evidence.check_context(&f.context(&target, 300)),
            Err(SetupContextError::WrongContext)
        );
        let native = native_fixture::current(evidence.association());
        assert!(
            evidence
                .check_context(&SetupUseContext {
                    authorization: &native,
                    ..f.context(&target, 300)
                })
                .is_ok()
        );
        assert_eq!(
            evidence.association().candidate().requester,
            parent.association().candidate().requester
        );
        assert!(
            evidence.association().candidate().root_event
                == parent.association().candidate().root_event
        );
        assert_eq!(evidence.original_work(), parent.original_work());
        assert!(
            !evidence
                .constraints()
                .ceiling
                .permits(Action::ExposeFrontend, &f.agent)
        );
        assert_no_operation_authority(&native);
        previous = evidence;
    }
    // Actual delegation depth and grant-chain use belong to the native grant owner,
    // not this historical-evidence/proposal boundary (see the coverage map).
}

#[test]
fn proposal_narrowing_cannot_add_actions_resources_audience_or_lifetime() {
    let f = Fixture::new();
    let allowed = f.authority().constraints().ceiling.clone();
    let mut added_action = allowed.clone();
    added_action.actions.insert(ActionPermission {
        action: Action::InstallArtifacts,
        resource: id("agent-a"),
    });
    let mut added_resource = allowed.clone();
    added_resource.services.insert(ServicePermission {
        target: id("agent-a"),
        connection: id("mail-account-a"),
        operation: id("read"),
        resource: id("other-inbox"),
        lifetime: lifetime(),
    });
    let mut added_audience = allowed.clone();
    added_audience
        .output_audiences
        .insert(id("monitoring-channel"));
    let mut added_lifetime = allowed.clone();
    let mut right = added_lifetime.services.pop_first().unwrap();
    right.lifetime.expires_at_ms = Some(1001);
    added_lifetime.services.insert(right);
    let mut persistent = allowed.clone();
    let mut right = persistent.frontends.pop_first().unwrap();
    right.lifetime.expires_at_ms = None;
    persistent.frontends.insert(right);
    for wider in [
        added_action,
        added_resource,
        added_audience,
        added_lifetime,
        persistent,
    ] {
        assert!(!allowed.contains(&wider));
    }
    assert_no_operation_authority(&f.native);
}

#[test]
fn readiness_requires_every_requested_role_and_actual_owner_evidence() {
    let f = Fixture::new();
    let intents = [f.service(), f.frontend()];
    let authority = f.authority();
    let mut owners = Owners::ready(&intents);
    assert!(
        verify_readiness(
            &authority,
            &intents,
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        )
        .unwrap()
        .all_ready()
    );
    owners.bindings.remove(&intents[1].id);
    let result = verify_readiness(
        &authority,
        &intents,
        &owners,
        &owners,
        &f.context(&f.destination, 200),
    )
    .unwrap();
    assert!(!result.all_ready());
    assert!(matches!(
        result.outcomes()[&intents[1].id],
        RequirementOutcome::PendingBinding { .. }
    ));
    owners.grants.remove(&intents[0].id);
    let result = verify_readiness(
        &authority,
        &intents,
        &owners,
        &owners,
        &f.context(&f.destination, 200),
    )
    .unwrap();
    assert!(matches!(
        result.outcomes()[&intents[0].id],
        RequirementOutcome::PendingGrant
    ));
    assert!(
        !verify_readiness(
            &authority,
            &[],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        )
        .unwrap()
        .all_ready()
    );
}

#[test]
fn revoked_grant_and_failed_policy_lookup_never_reuse_old_ready_status() {
    let f = Fixture::new();
    let intent = f.service();
    let mut owners = Owners::ready(std::slice::from_ref(&intent));
    let authority = f.authority();
    assert!(
        verify_readiness(
            &authority,
            std::slice::from_ref(&intent),
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        )
        .unwrap()
        .all_ready()
    );
    owners.grants.clear();
    assert!(
        !verify_readiness(
            &authority,
            std::slice::from_ref(&intent),
            &owners,
            &owners,
            &f.context(&f.destination, 201)
        )
        .unwrap()
        .all_ready()
    );
    owners.unavailable = true;
    assert!(matches!(
        verify_readiness(
            &authority,
            &[intent],
            &owners,
            &owners,
            &f.context(&f.destination, 202)
        ),
        Err(BindingError::OwnerUnavailable(_))
    ));
}

#[test]
fn changed_account_or_selected_manifest_invalidates_previous_evidence() {
    let f = Fixture::new();
    let intent = f.service();
    let mut owners = Owners::ready(std::slice::from_ref(&intent));
    let authority = f.authority();
    let grant = owners.grants.get_mut(&intent.id).unwrap();
    let RoleRequirement::Service { connection, .. } = &mut grant.role else {
        unreachable!()
    };
    *connection = id("other-mail-account");
    assert!(matches!(
        verify_readiness(
            &authority,
            std::slice::from_ref(&intent),
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidGrant)
    ));
    owners = Owners::ready(std::slice::from_ref(&intent));
    owners.bindings.get_mut(&intent.id).unwrap().manifest_digest =
        ContentDigest::of_bytes(b"old-selection");
    assert!(matches!(
        verify_readiness(
            &authority,
            &[intent],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidBinding)
    ));
}

#[test]
fn service_receipt_cannot_certify_frontend_and_monitoring_copy_expands_audience() {
    let f = Fixture::new();
    let intent = f.frontend();
    let mut owners = Owners::ready(std::slice::from_ref(&intent));
    let authority = f.authority();
    owners.bindings.get_mut(&intent.id).unwrap().evidence = receipt(&f.service()).evidence;
    assert!(matches!(
        verify_readiness(
            &authority,
            std::slice::from_ref(&intent),
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidBinding)
    ));
    owners = Owners::ready(std::slice::from_ref(&intent));
    let RoleEvidence::Frontend {
        verified_audience, ..
    } = &mut owners.bindings.get_mut(&intent.id).unwrap().evidence
    else {
        unreachable!()
    };
    verified_audience.insert(id("monitoring-channel"));
    assert!(matches!(
        verify_readiness(
            &authority,
            &[intent],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidBinding)
    ));
}

#[test]
fn evidence_has_time_bounds_and_does_not_survive_its_observation_window() {
    let f = Fixture::new();
    let intent = f.service();
    let authority = f.authority();
    for now in [800, 850, 900, 1000] {
        let owners = Owners::ready(std::slice::from_ref(&intent));
        assert!(
            verify_readiness(
                &authority,
                std::slice::from_ref(&intent),
                &owners,
                &owners,
                &f.context(&f.destination, now)
            )
            .is_err()
        );
    }
    let mut owners = Owners::ready(std::slice::from_ref(&intent));
    owners.bindings.get_mut(&intent.id).unwrap().observed_at_ms = 201;
    assert!(matches!(
        verify_readiness(
            &authority,
            &[intent],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidBinding)
    ));
}

#[test]
fn duplicate_requirements_and_foreign_context_cannot_disappear_in_projection() {
    let f = Fixture::new();
    let intent = f.service();
    let owners = Owners::ready(std::slice::from_ref(&intent));
    let authority = f.authority();
    assert!(matches!(
        verify_readiness(
            &authority,
            &[intent.clone(), intent.clone()],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::InvalidRequirements)
    ));
    let foreign = BindingIntent {
        requester: principal("person-b"),
        ..intent
    };
    assert!(matches!(
        verify_readiness(
            &authority,
            &[foreign],
            &owners,
            &owners,
            &f.context(&f.destination, 200)
        ),
        Err(BindingError::WrongContext)
    ));
}

#[test]
fn mixed_rights_and_read_only_ceiling_cannot_be_crossed_or_widened_at_peer() {
    let f = Fixture::new();
    let base = f.authority();
    let mut constraints = base.constraints().clone();
    constraints.ceiling.services.insert(ServicePermission {
        target: id("agent-a"),
        connection: id("calendar-account-a"),
        operation: id("write"),
        resource: id("calendar-a"),
        lifetime: lifetime(),
    });
    let mut candidate = base.association().candidate().clone();
    candidate.target = dest("peer-one");
    candidate.logical_executor = candidate.target.logical_owner.clone();
    let evidence = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(candidate).unwrap(),
        constraints,
    )
    .unwrap();
    let native = native_fixture::current(evidence.association());
    let target = dest("peer-one");
    let context = SetupUseContext {
        authorization: &native,
        ..f.context(&target, 220)
    };
    let mut intent = f.service();
    validate_proposal(&evidence, &intent, &context).unwrap();
    let RoleRequirement::Service { access, .. } = &mut intent.role else {
        unreachable!()
    };
    *access = [ServiceAccess {
        operation: id("write"),
        resource: id("inbox-a"),
    }]
    .into();
    assert!(matches!(
        validate_proposal(&evidence, &intent, &context),
        Err(BindingError::InvalidRequirements)
    ));
    let mut wider = evidence.constraints().ceiling.clone();
    wider.services.insert(ServicePermission {
        target: id("agent-a"),
        connection: id("mail-account-a"),
        operation: id("write"),
        resource: id("inbox-a"),
        lifetime: lifetime(),
    });
    assert!(!evidence.constraints().ceiling.contains(&wider));
    assert_no_operation_authority(&native);
}

#[test]
fn receipts_from_colliding_account_or_tenant_ids_are_rejected() {
    let f = Fixture::new();
    let intent = f.service();
    for foreign_tenant in [false, true] {
        let mut other = intent.clone();
        if foreign_tenant {
            other.tenant = id("other-tenant");
        } else {
            let RoleRequirement::Service { connection, .. } = &mut other.role else {
                unreachable!()
            };
            *connection = id("mail-account-b");
        }
        // All local IDs, revision strings, manifest, runtime and catalog still
        // collide. Only full-context digests distinguish these receipts.
        let mut owners = Owners::ready(std::slice::from_ref(&intent));
        owners.bindings.insert(intent.id.clone(), receipt(&other));
        assert!(matches!(
            verify_readiness(
                &f.authority(),
                std::slice::from_ref(&intent),
                &owners,
                &owners,
                &f.context(&f.destination, 200)
            ),
            Err(BindingError::InvalidBinding)
        ));
    }
}

#[test]
fn persistent_exposure_outlives_setup_but_requires_explicit_lifetime_authority() {
    let f = Fixture::new();
    let lifetime = RoleLifetime {
        scope: LifetimeScope::Deployment {
            deployment: f.deployment.clone(),
        },
        expires_at_ms: None,
    };
    let mut intent = f.frontend();
    intent.lifetime = lifetime.clone();
    assert!(matches!(
        validate_proposal(&f.authority(), &intent, &f.context(&f.destination, 200)),
        Err(BindingError::InvalidRequirements)
    ));
    let mut claims = f.authority().constraints().clone();
    let mut right = claims.ceiling.frontends.first().unwrap().clone();
    right.lifetime = lifetime.clone();
    claims.ceiling.frontends = [right].into();
    let verified = SetupRequestEvidence::new(f.authority().association().clone(), claims).unwrap();
    intent.original_work = verified.original_work().clone();
    validate_proposal(&verified, &intent, &f.context(&f.destination, 200)).unwrap();
    let mut approved = grant(&intent);
    approved.lifetime = lifetime;
    let mut bound = receipt(&intent);
    bound.grant_digest = approved.digest().unwrap();
    bound.observed_at_ms = 1900;
    bound.expires_at_ms = 5000;
    let owners = Owners {
        grants: [(intent.id.clone(), approved)].into(),
        bindings: [(intent.id.clone(), bound)].into(),
        unavailable: false,
    };
    assert!(matches!(
        verify_readiness(
            &verified,
            std::slice::from_ref(&intent),
            &owners,
            &owners,
            &f.context(&f.destination, 2000)
        ),
        Err(BindingError::Authority(SetupContextError::Expired))
    ));
    let context = BindingContext {
        tenant: &f.tenant,
        deployment: &f.deployment,
        policy_revision: &f.policy,
        now_ms: 2000,
    };
    assert!(
        observe_readiness(&[intent], &owners, &owners, &context)
            .unwrap()
            .all_ready()
    );
}

#[test]
fn cached_verification_cannot_bypass_changed_policy_or_receiver() {
    let f = Fixture::new();
    let cached = f.authority();
    let newer = rev("policy-5");
    let context = SetupUseContext {
        policy_revision: &newer,
        ..f.context(&f.destination, 201)
    };
    assert!(matches!(
        cached.check_proposal(Action::GrantService, &f.agent, &context),
        Err(SetupContextError::StalePolicy)
    ));
    let owners = Owners::ready(&[f.service()]);
    assert!(matches!(
        verify_readiness(&cached, &[f.service()], &owners, &owners, &context),
        Err(BindingError::Authority(SetupContextError::StalePolicy))
    ));
    let peer = id("peer-one");
    assert!(matches!(
        cached.check_proposal(
            Action::GrantService,
            &f.agent,
            &f.context(&dest(peer.as_str()), 201)
        ),
        Err(SetupContextError::WrongContext)
    ));
}

#[test]
fn exposure_cannot_add_participants_or_control_to_send_permission() {
    let f = Fixture::new();
    let authority = f.authority();
    for participant_change in [false, true] {
        let mut intent = f.frontend();
        let RoleRequirement::Frontend {
            participants,
            interactions,
            ..
        } = &mut intent.role
        else {
            unreachable!()
        };
        if participant_change {
            participants.insert(id("person-b"));
        } else {
            interactions.insert(id("manage"));
        }
        assert!(matches!(
            validate_proposal(&authority, &intent, &f.context(&f.destination, 200)),
            Err(BindingError::InvalidRequirements)
        ));
    }
}

#[test]
fn native_intent_strict_requester_and_exact_original_work_cannot_be_substituted() {
    let f = Fixture::new();
    let mut wire = serde_json::to_value(f.service()).unwrap();
    wire["requester"]["approved"] = serde_json::json!(true);
    assert!(serde_json::from_value::<BindingIntent>(wire).is_err());
    let mut wire = serde_json::to_value(f.service()).unwrap();
    wire["approved"] = serde_json::json!(true);
    assert!(serde_json::from_value::<BindingIntent>(wire).is_err());
    let mut intent = f.service();
    intent.requester.kind = PrincipalKind::ServiceAccount;
    assert!(matches!(
        validate_proposal(&f.authority(), &intent, &f.context(&f.destination, 200)),
        Err(BindingError::WrongContext)
    ));
    let mut intent = f.service();
    intent.original_work = native_fixture::original("other-locator");
    assert!(matches!(
        validate_proposal(&f.authority(), &intent, &f.context(&f.destination, 200)),
        Err(BindingError::WrongContext)
    ));
}

#[test]
fn native_intent_requester_decode_preserves_native_validation_and_rejects_duplicate_fields() {
    let f = Fixture::new();
    let intent = f.service();
    let raw = serde_json::to_string(&intent).unwrap();
    let requester = serde_json::to_string(&intent.requester).unwrap();
    let roundtrip = serde_json::from_str::<BindingIntent>(&raw).unwrap();
    assert_eq!(roundtrip, intent);
    roundtrip.requester.validate_qualified().unwrap();
    assert_eq!(serde_json::to_string(&roundtrip.requester).unwrap(), requester);
    assert_no_operation_authority(&f.native);

    let needle = format!("\"requester\":{requester}");
    assert_eq!(raw.matches(&needle).count(), 1);
    for (field, value) in [
        ("kind", serde_json::to_string(&intent.requester.kind).unwrap()),
        ("id", serde_json::to_string(&intent.requester.id).unwrap()),
        (
            "qualification",
            serde_json::to_string(&intent.requester.qualification).unwrap(),
        ),
    ] {
        let repeated = format!(
            "{{\"{field}\":{value},{}",
            requester.strip_prefix('{').unwrap()
        );
        let duplicate = raw.replacen(&needle, &format!("\"requester\":{repeated}"), 1);
        assert!(
            serde_json::from_str::<BindingIntent>(&duplicate).is_err(),
            "duplicate requester field {field} must be rejected"
        );
    }

    let mut unqualified = serde_json::to_value(&intent).unwrap();
    unqualified["requester"]["qualification"] = serde_json::json!({"kind": "unqualified"});
    assert!(serde_json::from_value::<BindingIntent>(unqualified).is_err());
    let mut invalid_id = serde_json::to_value(&intent).unwrap();
    invalid_id["requester"]["id"] = serde_json::json!("");
    assert!(serde_json::from_value::<BindingIntent>(invalid_id).is_err());
}
