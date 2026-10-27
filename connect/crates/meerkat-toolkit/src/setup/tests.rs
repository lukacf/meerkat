//! Real SQLite and generated-owner composition tests. Policy/runtime owners
//! below are fixtures; these do not certify a provider or a live MobKit host.

use crate::authority::test_support as native_fixture;
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

use crate::{authority::*, binding::*, continuation::*, request::*, setup::*};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use toolkit_catalog::AcquisitionPhase;
use toolkit_package::{ContentDigest, Id, Revision};
use toolkit_store::*;

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn rev(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn reference(value: &str) -> ContextReference {
    ContextReference {
        owner: id("evidence-owner"),
        id: id(value),
        revision: rev("1"),
        content_digest: ContentDigest::of_bytes(value.as_bytes()),
    }
}

struct References(AtomicBool);
impl ContextReferenceOwner for References {
    fn verify_access(
        &self,
        reference: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        if self.0.load(Ordering::SeqCst) && reference.owner == id("evidence-owner") {
            Ok(())
        } else {
            Err(RequestError::ContextUnavailable)
        }
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: SqliteStore,
    tenant: Id,
    deployment: Id,
    receiver: Id,
    destination: NativeWorkTarget,
    policy: Revision,
    authorization: WorkAuthorizationContext,
    authority: SetupRequestEvidence,
    request: ValidatedSetupRequest,
    references: References,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path().join("toolkit.sqlite")).unwrap();
        let tenant = id("tenant-a");
        let deployment = id("deployment-a");
        let receiver = id("agent-a");
        let destination = dest("agent-a");
        let policy = rev("policy-1");
        let ceiling = Ceiling {
            actions: [
                Action::FindAndInstall,
                Action::GrantService,
                Action::ExposeFrontend,
            ]
            .into_iter()
            .map(|action| ActionPermission {
                action,
                resource: receiver.clone(),
            })
            .collect(),
            services: [ServicePermission {
                target: receiver.clone(),
                connection: id("account-a"),
                operation: id("read"),
                resource: id("inbox"),
                lifetime: lifetime(),
            }]
            .into(),
            frontends: [FrontendPermission {
                target: receiver.clone(),
                endpoint: id("chat-a"),
                participant: id("person-a"),
                interaction: id("send"),
                lifetime: lifetime(),
            }]
            .into(),
            output_audiences: [id("person-a")].into(),
        };
        let authority = SetupRequestEvidence::new(
            native_fixture::association(
                principal("person-a"),
                destination.clone(),
                "original-work",
                "private-a",
            ),
            SetupProposalConstraints {
                tenant: tenant.clone(),
                deployment: deployment.clone(),
                policy_revision: policy.clone(),
                issued_at_ms: 100,
                expires_at_ms: 1000,
                ceiling,
            },
        )
        .unwrap();
        let authorization = native_fixture::current(authority.association());
        let context = SetupUseContext {
            tenant: &tenant,
            deployment: &deployment,
            realm: &authority.association().candidate().ingress_namespace.realm,
            target: &destination,
            policy_revision: &policy,
            now_ms: 200,
            authorization: &authorization,
        };
        let references = References(AtomicBool::new(true));
        let request = validate_request(FindAndInstallRequest {
            request: "Find that vaguely remembered service and connect my private frontend.\nKeep this wording.".into(),
            user_context: vec![RequestContext::Reference(reference("original-context"))],
            caller_observations: vec!["There are no matching tools yet.".into()],
        }, HostSetupRequest {
            setup_helper: None,
            target: receiver.clone(), correlation: id("original-call-1"), parent_operation: Some(id("original-task")), continuation: TaskContinuation { native_original: None, owner: id("runtime"), task: id("original-task"), revision: rev("2"), delivery_key: id("delivery-1"), audience: [id("person-a")].into() }, environment_revision: rev("environment-1"), limits: SetupLimits { deadline_ms: 900, max_workers: 3, max_effect_attempts: 5 },
        }, &authority, &context, &references).unwrap();
        Self {
            directory,
            store,
            tenant,
            deployment,
            receiver,
            destination,
            policy,
            authorization,
            authority,
            request,
            references,
        }
    }

    fn context(&self, now_ms: u64) -> SetupUseContext<'_> {
        SetupUseContext {
            tenant: &self.tenant,
            deployment: &self.deployment,
            realm: &self
                .authority
                .association()
                .candidate()
                .ingress_namespace
                .realm,
            target: &self.destination,
            authorization: &self.authorization,
            policy_revision: &self.policy,
            now_ms,
        }
    }
    fn bridge(&self) -> SetupStore<'_> {
        self.for_store(&self.store)
    }
    fn for_store<'a>(&self, store: &'a SqliteStore) -> SetupStore<'a> {
        SetupStore::new(
            store,
            SetupScope::new(self.tenant.clone(), self.deployment.clone()),
        )
        .unwrap()
    }
    fn access<'a>(&'a self, context: &'a SetupUseContext<'a>) -> SetupAccess<'a> {
        SetupAccess {
            evidence: &self.authority,
            context,
            references: &self.references,
        }
    }
    async fn admit(&self) -> SetupSnapshot {
        self.bridge()
            .admit(
                &self.request,
                &self.authority,
                &self.context(200),
                &self.references,
            )
            .await
            .unwrap()
            .snapshot
    }
    async fn lease(&self, snapshot: &SetupSnapshot) -> RecordLease {
        self.store
            .claim_record(snapshot.key(), snapshot.state().revision, "worker-a", 10000)
            .await
            .unwrap()
    }
    fn service(&self) -> BindingIntent {
        BindingIntent {
            id: id("service-a"),
            tenant: self.tenant.clone(),
            deployment: self.deployment.clone(),
            requester: principal("person-a"),
            target: self.receiver.clone(),
            original_work: self.authority.original_work().clone(),
            manifest_digest: ContentDigest::of_bytes(b"manifest-a"),
            revision: rev("1"),
            policy_revision: self.policy.clone(),
            request_valid_until_ms: 900,
            lifetime: lifetime(),
            role: RoleRequirement::Service {
                connection: id("account-a"),
                connection_revision: rev("account-1"),
                access: [ServiceAccess {
                    operation: id("read"),
                    resource: id("inbox"),
                }]
                .into(),
                runtime: id("runtime-a"),
            },
        }
    }
    fn selection(&self) -> AgentSelection {
        let frontend = BindingIntent {
            id: id("frontend-a"),
            role: RoleRequirement::Frontend {
                endpoint: id("chat-a"),
                participants: [id("person-a")].into(),
                audience: [id("person-a")].into(),
                interactions: [id("send")].into(),
                agent: self.receiver.clone(),
                conversation_policy: rev("conversation-1"),
            },
            ..self.service()
        };
        AgentSelection {
            decision: reference("decision-1"),
            requirements: vec![self.service(), frontend],
        }
    }
}

fn lifetime() -> RoleLifetime {
    RoleLifetime {
        scope: LifetimeScope::Member {
            member: id("agent-a"),
        },
        expires_at_ms: Some(850),
    }
}

struct Owners {
    frontend_ready: bool,
    granted: bool,
}
impl GrantOwner for Owners {
    fn current_grant(
        &self,
        intent: &BindingIntent,
        _now: u64,
    ) -> Result<Option<GrantReceipt>, BindingError> {
        Ok(self.granted.then(|| GrantReceipt {
            id: id(&format!("grant/{}", intent.id)),
            owner: id("policy"),
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
            lifetime: intent.lifetime.clone(),
        }))
    }
}
impl BindingOwner for Owners {
    fn current_binding(
        &self,
        intent: &BindingIntent,
        grant: &GrantReceipt,
        now: u64,
    ) -> Result<Option<BindingReceipt>, BindingError> {
        let evidence = match &intent.role {
            RoleRequirement::Service {
                runtime,
                connection_revision,
                ..
            } => RoleEvidence::Service {
                runtime: runtime.clone(),
                incarnation: id("incarnation-new"),
                catalog_revision: rev("catalog-1"),
                connection_revision: connection_revision.clone(),
            },
            RoleRequirement::Frontend {
                endpoint, audience, ..
            } => {
                if !self.frontend_ready {
                    return Ok(None);
                }
                RoleEvidence::Frontend {
                    endpoint: endpoint.clone(),
                    admission_revision: rev("admission-1"),
                    conversation_revision: rev("conversation-1"),
                    delivery_revision: rev("delivery-1"),
                    verified_audience: audience.clone(),
                }
            }
        };
        Ok(Some(BindingReceipt {
            id: id(&format!("binding/{}", intent.id)),
            owner: id("runtime"),
            intent: intent.id.clone(),
            intent_revision: intent.revision.clone(),
            grant: grant.id.clone(),
            intent_digest: intent.digest().unwrap(),
            grant_digest: grant.digest().unwrap(),
            manifest_digest: intent.manifest_digest.clone(),
            policy_revision: intent.policy_revision.clone(),
            observed_at_ms: now,
            expires_at_ms: 800,
            evidence,
        }))
    }
}

#[tokio::test]
async fn concurrent_exact_calls_admit_one_request_and_one_durable_work_handoff() {
    let fixture = std::sync::Arc::new(Fixture::new());
    let mut tasks = Vec::new();
    for offset in 0..24 {
        let f = fixture.clone();
        tasks.push(tokio::spawn(async move {
            f.bridge()
                .admit(
                    &f.request,
                    &f.authority,
                    &f.context(200 + offset),
                    &f.references,
                )
                .await
                .unwrap()
        }));
    }
    let mut created = 0;
    let mut snapshots = Vec::new();
    for task in tasks {
        let admitted = task.await.unwrap();
        created += usize::from(admitted.created);
        snapshots.push(admitted.snapshot);
    }
    assert_eq!(created, 1);
    assert!(
        snapshots
            .iter()
            .all(|snapshot| snapshot.key() == snapshots[0].key())
    );
    let effects = fixture
        .store
        .list_effects(snapshots[0].key(), None, 100)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].intent.kind, "work_admission_requested");
    assert_eq!(effects[0].status, EffectStatus::Pending);
    assert_eq!(
        fixture
            .store
            .read_events(snapshots[0].key(), 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn changed_input_conflicts_and_revoked_context_cannot_replay() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let mut original = f.request.request().clone();
    original.request.push_str("changed");
    let changed = validate_request(
        original,
        f.request.host().clone(),
        &f.authority,
        &f.context(210),
        &f.references,
    )
    .unwrap();
    assert!(matches!(
        f.bridge()
            .admit(&changed, &f.authority, &f.context(210), &f.references)
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    f.references.0.store(false, Ordering::SeqCst);
    assert!(
        f.bridge()
            .admit(&f.request, &f.authority, &f.context(210), &f.references)
            .await
            .is_err()
    );
    assert!(
        f.bridge()
            .load(
                &id(&snapshot.key().id),
                &f.authority,
                &f.context(210),
                &f.references
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pending_frontend_prevents_completion_then_ready_and_handoff_survive_reopen() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let lease = f.lease(&initial).await;
    let selected = f
        .bridge()
        .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
        .await
        .unwrap();
    let pending = Owners {
        frontend_ready: false,
        granted: true,
    };
    assert!(matches!(
        f.bridge()
            .observe_ready(
                &selected,
                &lease,
                f.access(&f.context(230)),
                &pending,
                &pending
            )
            .await
            .unwrap(),
        ReadinessResult::ObservedRoles(_)
    ));
    assert_eq!(
        f.store.get(selected.key()).await.unwrap().unwrap().revision,
        2
    );
    assert_eq!(
        f.store
            .list_effects(selected.key(), None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let owners = Owners {
        frontend_ready: true,
        granted: true,
    };
    let ReadinessResult::Ready(ready) = f
        .bridge()
        .observe_ready(
            &selected,
            &lease,
            f.access(&f.context(240)),
            &owners,
            &owners,
        )
        .await
        .unwrap()
    else {
        panic!("all fixture owners are ready")
    };
    assert_eq!(ready.state().lifecycle_phase, AcquisitionPhase::Ready);
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let recovered = f
        .for_store(&reopened)
        .load(
            &id(&ready.key().id),
            &f.authority,
            &f.context(250),
            &f.references,
        )
        .await
        .unwrap();
    assert_eq!(recovered.request().request(), f.request.request());
    assert_eq!(recovered.state(), ready.state());
    let effects = reopened.list_effects(ready.key(), None, 100).await.unwrap();
    assert_eq!(effects.len(), 2);
    assert!(
        effects
            .iter()
            .any(|effect| effect.intent.kind == "continuation_requested"
                && effect.intent.replay_safety == ReplaySafety::ReconcileBeforeRetry)
    );
    assert!(!format!("{recovered:?}").contains("vaguely"));
}

#[tokio::test]
async fn cancel_wins_over_late_ready_and_stale_selection() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let lease = f.lease(&initial).await;
    let selected = f
        .bridge()
        .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
        .await
        .unwrap();
    let cancelled = f
        .bridge()
        .resolve(
            &selected,
            Resolution::Cancelled(reference("cancelled")),
            None,
            f.access(&f.context(230)),
        )
        .await
        .unwrap();
    let owners = Owners {
        frontend_ready: true,
        granted: true,
    };
    assert!(matches!(
        f.bridge()
            .observe_ready(
                &selected,
                &lease,
                f.access(&f.context(240)),
                &owners,
                &owners
            )
            .await,
        Err(SetupError::Store(StoreError::Conflict))
    ));
    assert!(matches!(
        f.bridge()
            .observe_ready(
                &cancelled,
                &lease,
                f.access(&f.context(240)),
                &owners,
                &owners
            )
            .await,
        Err(SetupError::Rejected)
    ));
    assert!(
        f.bridge()
            .select(&initial, f.selection(), &lease, f.access(&f.context(240)))
            .await
            .is_err()
    );
    assert_eq!(
        f.store
            .list_effects(initial.key(), None, 100)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.bridge()
            .load(
                &id(&initial.key().id),
                &f.authority,
                &f.context(250),
                &f.references
            )
            .await
            .unwrap()
            .state()
            .lifecycle_phase,
        AcquisitionPhase::Cancelled
    );
}

#[tokio::test]
async fn old_worker_fence_cannot_commit_selection() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let old = f.lease(&snapshot).await;
    f.store.release_record_claim(&old).await.unwrap();
    let fresh = f
        .store
        .claim_record(snapshot.key(), 1, "worker-b", 10000)
        .await
        .unwrap();
    assert!(matches!(
        f.bridge()
            .select(&snapshot, f.selection(), &old, f.access(&f.context(220)))
            .await,
        Err(SetupError::Store(StoreError::Fenced))
    ));
    f.bridge()
        .select(&snapshot, f.selection(), &fresh, f.access(&f.context(220)))
        .await
        .unwrap();
}

#[tokio::test]
async fn duplicate_roles_wrong_target_and_changed_policy_cannot_select() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let lease = f.lease(&snapshot).await;
    let mut duplicate = f.selection();
    duplicate
        .requirements
        .push(duplicate.requirements[0].clone());
    assert!(matches!(
        f.bridge()
            .select(&snapshot, duplicate, &lease, f.access(&f.context(220)))
            .await,
        Err(SetupError::InvalidSelection)
    ));
    let mut wrong = f.selection();
    wrong.requirements[0].target = id("agent-b");
    assert!(
        f.bridge()
            .select(&snapshot, wrong, &lease, f.access(&f.context(220)))
            .await
            .is_err()
    );
    let policy = rev("policy-new");
    let context = SetupUseContext {
        policy_revision: &policy,
        ..f.context(220)
    };
    assert!(
        f.bridge()
            .select(&snapshot, f.selection(), &lease, f.access(&context))
            .await
            .is_err()
    );
    assert_eq!(
        f.store.get(snapshot.key()).await.unwrap().unwrap().revision,
        1
    );
}

#[tokio::test]
async fn selection_history_and_request_content_are_durable_and_scope_bound() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let lease = f.lease(&initial).await;
    let first = f
        .bridge()
        .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
        .await
        .unwrap();
    let mut replacement = f.selection();
    replacement.decision = reference("decision-2");
    let second = f
        .bridge()
        .select(&first, replacement, &lease, f.access(&f.context(230)))
        .await
        .unwrap();
    let third = f
        .bridge()
        .select(&second, f.selection(), &lease, f.access(&f.context(240)))
        .await
        .unwrap();
    assert_eq!(third.state().selection_generation, 3);
    let stored = f.store.get(third.key()).await.unwrap().unwrap();
    assert_eq!(stored.data.body["selections"].as_array().unwrap().len(), 3);
    let wrong = SetupStore::new(
        &f.store,
        SetupScope::new(f.tenant.clone(), id("deployment-b")),
    )
    .unwrap();
    assert!(matches!(
        wrong
            .select(&third, f.selection(), &lease, f.access(&f.context(250)))
            .await,
        Err(SetupError::WrongScope)
    ));
}

struct DeadlineOwner(bool);
#[async_trait]
impl SetupDeadlineOwner for DeadlineOwner {
    async fn authorize_expiry(&self, _snapshot: &SetupSnapshot) -> Result<(), SetupError> {
        if self.0 {
            Ok(())
        } else {
            Err(SetupError::Denied)
        }
    }
}

#[tokio::test]
async fn expiry_uses_host_authorization_and_store_clock_after_token_lifetime() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    assert!(matches!(
        f.bridge()
            .expire(&id(&snapshot.key().id), &DeadlineOwner(false))
            .await,
        Err(SetupError::Denied)
    ));
    let expired = f
        .bridge()
        .expire(&id(&snapshot.key().id), &DeadlineOwner(true))
        .await
        .unwrap();
    assert_eq!(expired.state().lifecycle_phase, AcquisitionPhase::Expired);
    assert!(expired.state().terminal_at >= 900);
    assert_eq!(
        f.store
            .list_effects(snapshot.key(), None, 100)
            .await
            .unwrap()
            .len(),
        2
    );
    assert!(matches!(
        f.bridge()
            .expire(&id(&snapshot.key().id), &DeadlineOwner(true))
            .await,
        Err(SetupError::Rejected)
    ));
}

#[tokio::test]
async fn corrupted_original_content_fails_recovery_even_with_valid_storage_digest() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let mut stored = f.store.get(initial.key()).await.unwrap().unwrap();
    stored.data.body["request"]["request"]["request"] = "corrupted".into();
    stored.data.body["state"]["revision"] = 2.into();
    f.store
        .compare_and_swap(Transition {
            key: initial.key().clone(),
            expected_revision: 1,
            expected_digest: Some(stored.content_digest),
            claim: None,
            data: stored.data,
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    assert!(
        f.bridge()
            .load(
                &id(&initial.key().id),
                &f.authority,
                &f.context(250),
                &f.references
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn sb1_recovery_rejects_empty_pending_extra_or_missing_ready_outcomes() {
    use toolkit_catalog::{
        AcquisitionInput, ToolkitAcquisitionMachineAuthority, ToolkitAcquisitionMachineMutator,
    };
    use toolkit_package::canonical_json;

    for outcomes in [
        serde_json::json!({}),
        serde_json::json!({
            "service-a": {"status": "pending_grant"},
            "frontend-a": {"status": "pending_grant"}
        }),
        serde_json::json!({
            "service-a": {"status": "ready", "grant": "grant-a", "receipt": "receipt-a"}
        }),
        serde_json::json!({
            "wrong-role": {"status": "ready", "grant": "grant-a", "receipt": "receipt-a"}
        }),
    ] {
        let f = Fixture::new();
        let initial = f.admit().await;
        let lease = f.lease(&initial).await;
        let selected = f
            .bridge()
            .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
            .await
            .unwrap();
        let mut stored = f.store.get(selected.key()).await.unwrap().unwrap();
        let ready = serde_json::json!({
            "selection_digest": selected.state().selection_digest,
            "selection_generation": selected.state().selection_generation,
            "observed_at_ms": 230,
            "outcomes": outcomes,
        });
        let ready_digest = ContentDigest::of_bytes(&canonical_json(&ready).unwrap()).to_string();
        let state = selected.state();
        let mut generated =
            ToolkitAcquisitionMachineAuthority::recover_from_state(state.clone()).unwrap();
        generated
            .apply(AcquisitionInput::ResolveReady {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                selection_digest: state.selection_digest.clone(),
                selection_generation: state.selection_generation,
                readiness_ref: ready_digest.clone(),
                readiness_digest: ready_digest,
                observed_at: 230,
                now: 230,
            })
            .unwrap();
        stored.data.body["state"] = serde_json::to_value(generated.state()).unwrap();
        stored.data.body["ready"] = ready;
        f.store
            .compare_and_swap(Transition {
                key: selected.key().clone(),
                expected_revision: selected.state().revision,
                expected_digest: Some(stored.content_digest),
                claim: Some(lease),
                data: stored.data,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let restored = f
            .bridge()
            .load(
                &id(&selected.key().id),
                &f.authority,
                &f.context(240),
                &f.references,
            )
            .await;
        assert!(matches!(restored, Err(SetupError::InvalidRecord)));
    }
}

#[tokio::test]
async fn sb2_historical_role_observations_bind_exact_revision_and_selection() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let lease = f.lease(&initial).await;
    let selected = f
        .bridge()
        .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
        .await
        .unwrap();
    let cancelled = f
        .bridge()
        .resolve(
            &selected,
            Resolution::Cancelled(reference("cancelled")),
            None,
            f.access(&f.context(230)),
        )
        .await
        .unwrap();
    let owners = Owners {
        frontend_ready: false,
        granted: true,
    };
    for snapshot in [&selected, &cancelled] {
        let ReadinessResult::ObservedRoles(observation) = f
            .bridge()
            .observe_ready(
                snapshot,
                &lease,
                f.access(&f.context(240)),
                &owners,
                &owners,
            )
            .await
            .unwrap()
        else {
            panic!("one role is missing its binding")
        };
        assert_eq!(observation.operation.as_str(), snapshot.key().id);
        assert_eq!(observation.acquisition_revision, snapshot.state().revision);
        assert_eq!(
            observation.selection_generation,
            snapshot.state().selection_generation
        );
        assert_eq!(
            observation.selection_digest.as_str(),
            snapshot.state().selection_digest
        );
        assert!(!observation.roles.all_ready());
    }
    let stored = f.store.get(selected.key()).await.unwrap().unwrap();
    assert_eq!(stored.revision, cancelled.state().revision);
    assert_eq!(stored.data.body["state"]["lifecycle_phase"], "cancelled");
}

struct OriginalContextRevoked;
impl ContextReferenceOwner for OriginalContextRevoked {
    fn verify_access(
        &self,
        reference: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        if reference.id == id("original-context") {
            Err(RequestError::ContextUnavailable)
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn sb3_requester_can_cancel_by_identity_after_restart_and_context_revocation() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let operation = id(&initial.key().id);
    let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
    let context = f.context(220);
    assert!(
        f.for_store(&reopened)
            .load(&operation, &f.authority, &context, &OriginalContextRevoked)
            .await
            .is_err()
    );
    let result = f
        .for_store(&reopened)
        .cancel(
            &operation,
            reference("cancelled"),
            SetupAccess {
                evidence: &f.authority,
                context: &context,
                references: &OriginalContextRevoked,
            },
        )
        .await
        .unwrap();
    assert_eq!(result.phase, AcquisitionPhase::Cancelled);
    assert_eq!(result.revision, 2);
    assert_eq!(result.operation, operation);
}

fn continuation_receipt() -> ContextReference {
    ContextReference {
        owner: id("runtime"),
        ..reference("continuation-receipt-1")
    }
}

struct ContinuationOwner {
    mode: u8,
    calls: std::sync::atomic::AtomicUsize,
    seen: std::sync::Mutex<Vec<serde_json::Value>>,
}

impl ContinuationOwner {
    fn new(mode: u8) -> Self {
        Self {
            mode,
            calls: 0.into(),
            seen: Default::default(),
        }
    }
}

#[async_trait]
impl SetupContinuationOwner for ContinuationOwner {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.seen
            .lock()
            .unwrap()
            .push(serde_json::to_value(obligation).unwrap());
        if self.mode == 1 {
            return Err(ContinuationError::Denied);
        }
        if self.mode == 2 {
            return Ok(None);
        }
        assert_eq!(obligation.continuation().audience, [id("person-a")].into());
        assert_eq!(obligation.requester(), &principal("person-a"));
        assert!(obligation.origin() == &native_fixture::event("private-a"));
        let mut evidence = ContinuationAdmissionEvidence {
            receipt: receipt.clone(),
            obligation_digest: obligation.digest()?,
            delivery_identity: obligation.delivery_identity()?,
            runtime: id("runtime-a"),
            session: id("session-a"),
            durable_input: reference("durable-input-a"),
            observed_at_ms: obligation.terminal_at_ms() + 1,
        };
        match self.mode {
            3 => evidence.obligation_digest = ContentDigest::of_bytes(b"wrong payload or audience"),
            4 => evidence.delivery_identity = ContentDigest::of_bytes(b"wrong delivery"),
            5 => evidence.receipt = reference("wrong-owner"),
            6 => evidence.observed_at_ms = obligation.terminal_at_ms() - 1,
            7 => evidence.session = id("changed-session"),
            _ => (),
        }
        Ok(Some(evidence))
    }
}

async fn terminal(f: &Fixture, phase: AcquisitionPhase) -> SetupSnapshot {
    let initial = f.admit().await;
    if phase == AcquisitionPhase::Expired {
        return f
            .bridge()
            .expire(&id(&initial.key().id), &DeadlineOwner(true))
            .await
            .unwrap();
    }
    let lease = f.lease(&initial).await;
    if phase == AcquisitionPhase::Ready {
        let selected = f
            .bridge()
            .select(&initial, f.selection(), &lease, f.access(&f.context(220)))
            .await
            .unwrap();
        let owners = Owners {
            frontend_ready: true,
            granted: true,
        };
        let ReadinessResult::Ready(ready) = f
            .bridge()
            .observe_ready(
                &selected,
                &lease,
                f.access(&f.context(230)),
                &owners,
                &owners,
            )
            .await
            .unwrap()
        else {
            panic!("ready");
        };
        return *ready;
    }
    let resolution = match phase {
        AcquisitionPhase::Unavailable => Resolution::Unavailable(reference("unavailable")),
        AcquisitionPhase::Failed => Resolution::Failed(reference("failed")),
        AcquisitionPhase::Cancelled => Resolution::Cancelled(reference("cancelled")),
        _ => panic!("terminal fixture phase"),
    };
    f.bridge()
        .resolve(
            &initial,
            resolution,
            Some(&lease),
            f.access(&f.context(220)),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn all_terminal_continuations_acknowledge_once_and_replay_after_restart_without_old_context()
{
    for phase in [
        AcquisitionPhase::Ready,
        AcquisitionPhase::Unavailable,
        AcquisitionPhase::Failed,
        AcquisitionPhase::Cancelled,
        AcquisitionPhase::Expired,
    ] {
        let f = Fixture::new();
        let ended = terminal(&f, phase).await;
        let operation = id(&ended.key().id);
        f.references.0.store(false, Ordering::SeqCst);
        let owner = ContinuationOwner::new(0);
        let receipt = continuation_receipt();
        let first = f
            .bridge()
            .acknowledge_continuation(&operation, &receipt, &owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.phase, phase);
        assert_eq!(first.revision, ended.state().revision + 1);
        let before = f.store.get(ended.key()).await.unwrap().unwrap();
        let events = f.store.read_events(ended.key(), 0, 100).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|e| e.kind == "continuation_acknowledged")
                .count(),
            1
        );
        let reopened = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
        let replay = f
            .for_store(&reopened)
            .acknowledge_continuation(&operation, &receipt, &owner)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay.revision, first.revision);
        assert_eq!(replay.phase, phase);
        assert_eq!(before, reopened.get(ended.key()).await.unwrap().unwrap());
        assert_eq!(
            events,
            reopened.read_events(ended.key(), 0, 100).await.unwrap()
        );
        assert_eq!(owner.calls.load(Ordering::SeqCst), 2);
        let seen = owner.seen.lock().unwrap();
        assert_eq!(seen[0], seen[1]);
        let encoded = serde_json::to_string(&seen[0]).unwrap();
        assert!(!encoded.contains("vaguely remembered"));
        assert!(!encoded.contains("original-context"));
        assert_eq!(
            seen[0]["terminal_revision"],
            ended.state().terminal_revision
        );
        assert_eq!(
            seen[0]["continuation"]["audience"],
            serde_json::json!(["person-a"])
        );
    }
}

#[tokio::test]
async fn continuation_pending_denied_and_mismatched_receipts_cannot_mutate() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let operation = id(&ended.key().id);
    let before = f.store.get(ended.key()).await.unwrap().unwrap();
    let receipt = continuation_receipt();
    for mode in 1..=6 {
        let result = f
            .bridge()
            .acknowledge_continuation(&operation, &receipt, &ContinuationOwner::new(mode))
            .await;
        if mode == 2 {
            assert!(result.unwrap().is_none());
        } else {
            assert!(result.is_err());
        }
        assert_eq!(before, f.store.get(ended.key()).await.unwrap().unwrap());
    }
    let wrong_owner = reference("wrong-owner-receipt");
    assert!(
        f.bridge()
            .acknowledge_continuation(&operation, &wrong_owner, &ContinuationOwner::new(0))
            .await
            .is_err()
    );
    assert_eq!(before, f.store.get(ended.key()).await.unwrap().unwrap());
}

#[tokio::test]
async fn continuation_replay_rechecks_owner_and_rejects_changed_evidence() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Expired).await;
    let operation = id(&ended.key().id);
    let receipt = continuation_receipt();
    f.bridge()
        .acknowledge_continuation(&operation, &receipt, &ContinuationOwner::new(0))
        .await
        .unwrap();
    let before = f.store.get(ended.key()).await.unwrap().unwrap();
    for mode in [1, 7] {
        assert!(
            f.bridge()
                .acknowledge_continuation(&operation, &receipt, &ContinuationOwner::new(mode))
                .await
                .is_err()
        );
    }
    let other = ContextReference {
        owner: id("runtime"),
        ..reference("different-receipt")
    };
    assert!(
        f.bridge()
            .acknowledge_continuation(&operation, &other, &ContinuationOwner::new(0))
            .await
            .is_err()
    );
    assert_eq!(before, f.store.get(ended.key()).await.unwrap().unwrap());
}

#[tokio::test]
async fn active_request_never_exposes_a_continuation_obligation() {
    let f = Fixture::new();
    let initial = f.admit().await;
    let owner = ContinuationOwner::new(0);
    assert!(matches!(
        f.bridge()
            .acknowledge_continuation(&id(&initial.key().id), &continuation_receipt(), &owner)
            .await,
        Err(SetupError::Rejected)
    ));
    assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn concurrent_acknowledgments_commit_one_event_and_no_delivery_effect() {
    let f = Fixture::new();
    let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
    let effects_before = f.store.list_effects(ended.key(), None, 100).await.unwrap();
    let operation = id(&ended.key().id);
    let receipt = continuation_receipt();
    let owner = ContinuationOwner::new(0);
    let mut tasks = tokio::task::JoinSet::new();
    let owner = std::sync::Arc::new(owner);
    for _ in 0..16 {
        let store = SqliteStore::open(f.directory.path().join("toolkit.sqlite")).unwrap();
        let scope = SetupScope::new(f.tenant.clone(), f.deployment.clone());
        let operation = operation.clone();
        let receipt = receipt.clone();
        let owner = owner.clone();
        tasks.spawn(async move {
            SetupStore::new(&store, scope)
                .unwrap()
                .acknowledge_continuation(&operation, &receipt, owner.as_ref())
                .await
        });
    }
    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result.unwrap());
    }
    assert!(results.iter().any(|r| r.is_ok()));
    assert!(
        results
            .iter()
            .all(|r| r.is_ok() || matches!(r, Err(SetupError::Store(StoreError::Conflict))))
    );
    let stored = f.store.get(ended.key()).await.unwrap().unwrap();
    assert_eq!(stored.revision, ended.state().revision + 1);
    assert_eq!(
        f.store
            .read_events(ended.key(), 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.kind == "continuation_acknowledged")
            .count(),
        1
    );
    assert_eq!(
        effects_before,
        f.store.list_effects(ended.key(), None, 100).await.unwrap()
    );
}

#[tokio::test]
async fn corrupt_continuation_receipt_is_rejected_during_recovery() {
    for mutation in 0..5 {
        let f = Fixture::new();
        let ended = terminal(&f, AcquisitionPhase::Cancelled).await;
        let operation = id(&ended.key().id);
        let receipt = continuation_receipt();
        f.bridge()
            .acknowledge_continuation(&operation, &receipt, &ContinuationOwner::new(0))
            .await
            .unwrap();
        let mut stored = f.store.get(ended.key()).await.unwrap().unwrap();
        match mutation {
            0 => stored.data.body["continuation_receipt"] = serde_json::Value::Null,
            1 => {
                stored.data.body["continuation_receipt"]["obligation_digest"] =
                    serde_json::to_value(ContentDigest::of_bytes(b"another-result")).unwrap()
            }
            2 => {
                stored.data.body["continuation_receipt"]["receipt"]["owner"] = "other-owner".into()
            }
            3 => stored.data.body["continuation_receipt"]["observed_at_ms"] = 0.into(),
            4 => {
                stored.data.body["request"]["host"]["continuation"]["audience"] =
                    serde_json::json!(["other-person"])
            }
            _ => unreachable!(),
        }
        // This storage fixture preserves outer CAS/data integrity. Domain
        // recovery still must reject its contradictory content.
        let revision = stored.revision + 1;
        stored.data.body["state"]["revision"] = revision.into();
        stored.data.body["state"]["terminal_revision"] = (revision - 1).into();
        if mutation != 0 {
            let changed = &stored.data.body["continuation_receipt"];
            stored.data.body["state"]["continuation_receipt_ref"] =
                ContentDigest::of_bytes(&toolkit_package::canonical_json(changed).unwrap())
                    .to_string()
                    .into();
        }
        f.store
            .compare_and_swap(Transition {
                key: ended.key().clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest),
                claim: None,
                data: stored.data,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let owner = ContinuationOwner::new(0);
        assert!(
            f.bridge()
                .acknowledge_continuation(&operation, &receipt, &owner)
                .await
                .is_err()
        );
        assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
    }
}

mod continuation_executor;

mod work_preparation;

mod work_executor;

#[tokio::test]
async fn native_evidence_is_historical_and_changed_terms_conflict() {
    let f = Fixture::new();
    let first = f.admit().await;
    let row = f.store.get(first.key()).await.unwrap().unwrap();
    assert_eq!(row.data.version, 4);
    let request = &row.data.body["request"];
    assert_eq!(request["evidence"]["version"], 1);
    assert!(request.get("authority").is_none());
    assert!(request["evidence"].get("signature").is_none());
    assert_eq!(first.request().original_work(), f.authority.original_work());
    let mut constraints = f.authority.constraints().clone();
    constraints.expires_at_ms -= 1;
    let changed =
        SetupRequestEvidence::new(f.authority.association().clone(), constraints).unwrap();
    let candidate = validate_request(
        f.request.request().clone(),
        f.request.host().clone(),
        &changed,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    assert_eq!(
        candidate.correlation_digest(),
        first.request().correlation_digest()
    );
    assert_ne!(candidate.input_digest(), first.request().input_digest());
    assert!(matches!(
        f.bridge()
            .admit(&candidate, &changed, &f.context(200), &f.references)
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    let after = f.store.get(first.key()).await.unwrap().unwrap();
    assert_eq!(row.content_digest, after.content_digest);
    assert_eq!(row.revision, after.revision);
}

#[tokio::test]
async fn obsolete_and_unknown_acquisitions_refuse_without_mutation() {
    for version in [1, 2, 3, u32::MAX] {
        let f = Fixture::new();
        let snapshot = f.admit().await;
        let mut stored = f.store.get(snapshot.key()).await.unwrap().unwrap();
        stored.data.version = version;
        f.store
            .compare_and_swap(Transition {
                key: stored.key.clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest),
                claim: None,
                data: stored.data,
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let before = f.store.get(snapshot.key()).await.unwrap().unwrap();
        let events = f.store.read_events(snapshot.key(), 0, 100).await.unwrap();
        let effects = f
            .store
            .list_effects(snapshot.key(), None, 100)
            .await
            .unwrap();
        let operation = id(&snapshot.key().id);
        assert!(matches!(
            f.bridge()
                .load(&operation, &f.authority, &f.context(200), &f.references)
                .await,
            Err(SetupError::UnsupportedRecordVersion)
        ));
        let owner = ContinuationOwner::new(0);
        assert!(matches!(
            f.bridge()
                .acknowledge_continuation(&operation, &continuation_receipt(), &owner)
                .await,
            Err(SetupError::UnsupportedRecordVersion)
        ));
        assert_eq!(owner.calls.load(Ordering::SeqCst), 0);
        let after = f.store.get(snapshot.key()).await.unwrap().unwrap();
        assert_eq!(before.content_digest, after.content_digest);
        assert_eq!(before.revision, after.revision);
        assert_eq!(
            serde_json::to_value(events).unwrap(),
            serde_json::to_value(f.store.read_events(snapshot.key(), 0, 100).await.unwrap())
                .unwrap()
        );
        assert_eq!(
            serde_json::to_value(effects).unwrap(),
            serde_json::to_value(
                f.store
                    .list_effects(snapshot.key(), None, 100)
                    .await
                    .unwrap()
            )
            .unwrap()
        );
    }
}

#[tokio::test]
async fn legacy_signed_root_in_current_record_requires_migration() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let mut stored = f.store.get(snapshot.key()).await.unwrap().unwrap();
    stored.data.body["request"]["evidence"] = serde_json::json!({
        "version": 3, "claims": {}, "signature": "obsolete-root"
    });
    f.store
        .compare_and_swap(Transition {
            key: stored.key.clone(),
            expected_revision: stored.revision,
            expected_digest: Some(stored.content_digest),
            claim: None,
            data: stored.data,
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    let before = f.store.get(snapshot.key()).await.unwrap().unwrap();
    let result = f
        .bridge()
        .load(
            &id(&snapshot.key().id),
            &f.authority,
            &f.context(200),
            &f.references,
        )
        .await;
    let Err(SetupError::Encoding(error)) = result else {
        panic!("legacy root must fail during historical evidence decoding")
    };
    assert!(
        error
            .to_string()
            .contains("legacy signed request roots require native ingress migration")
    );
    let after = f.store.get(snapshot.key()).await.unwrap().unwrap();
    assert_eq!(before.content_digest, after.content_digest);
    assert_eq!(before.revision, after.revision);
}

mod native_cutover;


#[tokio::test]
async fn installed_terms_recover_exact_original_deadline_and_refuse_foreign_absence() {
    let f = Fixture::new();
    let bridge = f.bridge();
    let context = f.context(200);
    let correlation = &f.request.host().correlation;
    assert!(bridge.invocation_terms(f.authority.association(), correlation, &context).await.unwrap().is_none());
    let foreign = native_fixture::association(principal("person-b"), f.destination.clone(), "foreign-work", "foreign-source");
    assert!(matches!(bridge.invocation_terms(&foreign, correlation, &context).await, Err(SetupError::Denied)));
    bridge.admit(&f.request, &f.authority, &context, &f.references).await.unwrap();
    let later = f.context(700);
    let saved = bridge.invocation_terms(f.authority.association(), correlation, &later).await.unwrap().unwrap();
    assert_eq!(saved.host, *f.request.host());
    assert_eq!(saved.evidence, f.authority);
    assert_eq!(saved.evidence.constraints().issued_at_ms, 100);
    assert_eq!(saved.evidence.constraints().expires_at_ms, 1000);
    assert_eq!(saved.host.limits.deadline_ms, 900);
    let mut changed = f.request.request().clone();
    changed.request.push_str(" Changed content cannot reuse these terms.");
    assert!(matches!(bridge.admit_or_recover(changed, saved.host, &saved.evidence, &later, &f.references).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))));
}

#[tokio::test]
async fn canonical_work_input_uses_complete_acquisition_content_and_current_reference_owner() {
    let f = Fixture::new();
    let context = f.context(200);
    let saved = f.bridge().admit(&f.request, &f.authority, &context, &f.references).await.unwrap();
    let request = saved.snapshot.request();
    let reference = canonical_setup_input_reference(request).unwrap();
    assert_eq!(reference.content_digest, ContentDigest::of_bytes(&toolkit_package::canonical_json(request.request()).unwrap()));
    assert_eq!(reference.id.as_str(), saved.snapshot.key().id);
    // The canonical full input does not need an invented external owner. Its
    // embedded external references still use the actual reference owner.
    super::installed_input::verify_work_input_reference(request, &reference, &f.authority, &context, &f.references).unwrap();
    let mut wrong = reference.clone();
    wrong.content_digest = ContentDigest::of_bytes(b"only a summary");
    assert!(super::installed_input::verify_work_input_reference(request, &wrong, &f.authority, &context, &f.references).is_err());
    f.references.0.store(false, Ordering::SeqCst);
    assert!(super::installed_input::verify_work_input_reference(request, &reference, &f.authority, &context, &f.references).is_err());
}

#[test]
fn original_native_work_is_preserved_by_the_existing_request_codec() {
    use meerkat_core::{RunId, SessionId, lifecycle::InputId,
        retained_work::{RetainedWorkIdentity, RetainedContributorRef, SelectedInputBinding}};
    use crate::request::{NativeOriginalTask, StoredSetupRequest};
    let f = Fixture::new();
    let session = SessionId::new();
    let input = InputId::new();
    let native = NativeOriginalTask {
        origin_session_id: session.clone(),
        retained_work: RetainedWorkIdentity::new(format!("rt:session:{session}"), RunId::new(),
            vec![RetainedContributorRef { input_id: input.clone(), association_digest: "association-a".into(), submission_digest: "submission-a".into() }],
            [(input.to_string(), SelectedInputBinding { binding: "exact-native-binding".into(), batch_key: "native-batch".into() })].into(), None),
        member: Some(meerkat_core::MobMemberBinding { mob_id: "paired_host".into(), role: "research".into(), member: "native-member-a".into() }),
    };
    let mut host = f.request.host().clone();
    host.continuation.native_original = Some(native.clone());
    let request = validate_request(f.request.request().clone(), host, &f.authority,
        &f.context(200), &f.references).unwrap();
    let encoded = StoredSetupRequest::capture(&request).unwrap();
    let recovered = encoded.recover().unwrap();
    assert_eq!(recovered.host().continuation.native_original, Some(native));
    assert_eq!(canonical_json(&recovered).unwrap(), canonical_json(&request).unwrap());
    let mut changed = serde_json::to_value(&encoded).unwrap();
    changed["host"]["continuation"]["native_original"]["retained_work"]["contributors"][0]["submission_digest"] = "different".into();
    let changed: StoredSetupRequest = serde_json::from_value(changed).unwrap();
    assert!(changed.recover().is_err(), "full native identity is part of the existing immutable input");
}

#[test]
fn generic_request_codec_keeps_old_none_bytes_without_native_authority() {
    use crate::request::StoredSetupRequest;
    let f = Fixture::new();
    let bytes = canonical_json(&f.request).unwrap();
    assert!(!String::from_utf8(bytes.clone()).unwrap().contains("native_original"));
    let stored: StoredSetupRequest = serde_json::from_slice(&bytes).unwrap();
    let recovered = stored.recover().unwrap();
    assert!(recovered.host().continuation.native_original.is_none());
    assert_eq!(canonical_json(&recovered).unwrap(), bytes);
}

#[test]
fn legacy_admission_timestamp_is_not_reinterpreted_as_observation_time() {
    let evidence = ContinuationAdmissionEvidence {
        receipt: continuation_receipt(), obligation_digest: ContentDigest::of_bytes(b"obligation"),
        delivery_identity: ContentDigest::of_bytes(b"delivery"), runtime: id("runtime"), session: id("session"),
        durable_input: reference("input"), observed_at_ms: 42,
    };
    let mut legacy = serde_json::to_value(&evidence).unwrap();
    let value = legacy.as_object_mut().unwrap().remove("observed_at_ms").unwrap();
    legacy["admitted_at_ms"] = value;
    assert!(serde_json::from_value::<ContinuationAdmissionEvidence>(legacy).is_err());
}


#[tokio::test]
async fn absent_attachment_failure_preserves_legacy_record_and_obligation_bytes() {
    let f = Fixture::new();
    let terminal = terminal(&f, AcquisitionPhase::Unavailable).await;
    let record_bytes = canonical_json(&terminal.record).unwrap();
    let legacy: serde_json::Value = serde_json::from_slice(&record_bytes).unwrap();
    assert!(legacy.get("service_attachment_failure").is_none());
    assert!(legacy.get("service_attachment_observation").is_none());
    let decoded: super::AcquisitionRecord = serde_json::from_value(legacy).unwrap();
    assert!(decoded.service_attachment_failure.is_none());
    assert!(decoded.service_attachment_observation.is_none());
    assert_eq!(canonical_json(&decoded).unwrap(), record_bytes);
    let obligation = super::continuation_obligation(&decoded, terminal.request()).unwrap();
    let bytes = canonical_json(&obligation).unwrap();
    let mut legacy = serde_json::to_value(&obligation).unwrap();
    assert!(legacy.as_object_mut().unwrap().remove("service_attachment_failure").is_none());
    assert!(
        legacy
            .as_object_mut()
            .unwrap()
            .remove("service_attachment_observation")
            .is_none()
    );
    assert_eq!(canonical_json(&legacy).unwrap(), bytes);
    assert_eq!(obligation.digest().unwrap(), ContentDigest::of_bytes(&bytes));
}

#[tokio::test]
async fn attachment_failure_record_requires_reserved_unavailable_without_readiness() {
    use super::service_continuation::ApprovedServiceReady;
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let selected = ApprovedServiceSelection {
        source: StoreKey { realm: f.tenant.to_string(), owner: f.deployment.to_string(), id: "b".into() },
        original_work: meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: f.request.original_work().authority.clone(), work: meerkat_authorization_contracts::evidence::EvidenceId::new("different-b").unwrap() },
        source_identity: ContentDigest::of_bytes(b"exact-b-source"),
    };
    // Codec controls construct generated states, not admitted source authority.
    let (mut reserved, _) = super::apply(&snapshot, AcquisitionInput::RecordSelection {
        operation_id: snapshot.state().operation_id.clone(), authority_digest: snapshot.state().authority_digest.clone(),
        expected_revision: snapshot.state().revision, selection_digest: super::digest(&selected).unwrap().to_string(),
        decision_ref: super::digest(&selected).unwrap().to_string(), now: 201,
    }).unwrap();
    reserved.approved_service = Some(selected.clone());
    let mut reserved_snapshot = snapshot.clone();
    reserved_snapshot.record = reserved.clone();
    let (mut failed, _) = super::apply_selected_source(&reserved_snapshot, AcquisitionInput::ReportUnavailable {
        operation_id: reserved.state.operation_id.clone(), authority_digest: reserved.state.authority_digest.clone(),
        expected_revision: reserved.state.revision, detail_ref: super::digest(&reference("failed")).unwrap().to_string(), now: 202,
    }).unwrap();
    failed.detail = Some(reference("failed"));
    for failure in [
        ApprovedServiceAttachmentFailure::Failed,
        ApprovedServiceAttachmentFailure::Rejected,
    ] {
        failed.service_attachment_failure = Some(failure);
        super::validate_record(&failed, &f.request).unwrap();
        let bytes = canonical_json(&failed).unwrap();
        let legacy: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(legacy.get("service_attachment_observation").is_none());
        let decoded: super::AcquisitionRecord = serde_json::from_value(legacy).unwrap();
        assert_eq!(canonical_json(&decoded).unwrap(), bytes);
        let obligation = super::continuation_obligation(&decoded, &f.request).unwrap();
        let mut legacy = serde_json::to_value(&obligation).unwrap();
        assert!(
            legacy
                .as_object_mut()
                .unwrap()
                .remove("service_attachment_observation")
                .is_none()
        );
        assert_eq!(
            canonical_json(&legacy).unwrap(),
            canonical_json(&obligation).unwrap()
        );
    }
    failed.service_attachment_failure = Some(ApprovedServiceAttachmentFailure::Failed);
    failed.service_attachment_observation = Some(ApprovedServiceAttachmentObservation {
        phase: ApprovedServiceAttachmentPhase::Initialize,
        class: ApprovedServiceAttachmentClass::Closed { exit: None },
    });
    super::validate_record(&failed, &f.request).unwrap();
    let mut without_failure = failed.clone();
    without_failure.service_attachment_failure = None;
    assert!(super::validate_record(&without_failure, &f.request).is_err());
    let mut without_selection = failed.clone();
    without_selection.approved_service = None;
    assert!(super::validate_record(&without_selection, &f.request).is_err());
    reserved.service_attachment_failure = Some(ApprovedServiceAttachmentFailure::Failed);
    assert!(super::validate_record(&reserved, &f.request).is_err());
    let mut with_ready = failed;
    with_ready.service_ready = Some(ApprovedServiceReady { selection_digest: super::digest(&selected).unwrap(),
        selection_generation: 1, observed_at_ms: 202,
        admission: ApprovedServiceInput { source_admission: reference("input"), runtime: id("runtime"), session: id("session"), input: id("input") } });
    assert!(super::validate_record(&with_ready, &f.request).is_err());
}

#[test]
fn attachment_observation_codec_rejects_unbounded_or_invalid_fields() {
    let cases = [
        serde_json::json!({"phase": "launch", "class": {"kind": "spawn_failed"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "closed"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 1}}}),
        serde_json::json!({"phase": "tool_discovery", "class": {"kind": "closed", "exit": {"kind": "signal", "value": 15}}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "rejected"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "protocol_error"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "timed_out"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "unknown"}}),
    ];
    for value in &cases {
        let decoded: ApprovedServiceAttachmentObservation =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *value);
    }
    for value in [
        serde_json::json!({"phase": "future", "class": {"kind": "unknown"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "future"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "unknown"}, "server": "private-marker"}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "unknown", "detail": "private-marker"}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "rejected", "code": -32601}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "timed_out", "exit": {"kind": "code", "value": 1}}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "future", "value": 1}}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 2147483648_i64}}}),
        serde_json::json!({"phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 1, "detail": "private-marker"}}}),
    ] {
        assert!(serde_json::from_value::<ApprovedServiceAttachmentObservation>(value).is_err());
    }
}

#[tokio::test]
async fn retained_attachment_observation_decodes_generated_snapshot() {
    let f = Fixture::new();
    let snapshot = f.admit().await;
    let selected = ApprovedServiceSelection {
        source: StoreKey {
            realm: f.tenant.to_string(),
            owner: f.deployment.to_string(),
            id: "b".into(),
        },
        original_work: meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: f.request.original_work().authority.clone(),
            work: meerkat_authorization_contracts::evidence::EvidenceId::new("different-b")
                .unwrap(),
        },
        source_identity: ContentDigest::of_bytes(b"exact-b-source"),
    };
    // This is a retained snapshot/reader control, not admitted B authority.
    // Use generated transitions and commit the approved-service reservation
    // before adding synthetic historical metadata to the terminal write.
    let (mut reserved, effects) = super::apply(
        &snapshot,
        AcquisitionInput::RecordSelection {
            operation_id: snapshot.state().operation_id.clone(),
            authority_digest: snapshot.state().authority_digest.clone(),
            expected_revision: snapshot.state().revision,
            selection_digest: super::digest(&selected).unwrap().to_string(),
            decision_ref: super::digest(&selected).unwrap().to_string(),
            now: 201,
        },
    )
    .unwrap();
    reserved.approved_service = Some(selected);
    let reserved = f
        .bridge()
        .commit(&snapshot, reserved, effects, None)
        .await
        .unwrap();
    let (mut failed, effects) = super::apply_selected_source(
        &reserved,
        AcquisitionInput::ReportUnavailable {
            operation_id: reserved.state().operation_id.clone(),
            authority_digest: reserved.state().authority_digest.clone(),
            expected_revision: reserved.state().revision,
            detail_ref: super::digest(&reference("failed")).unwrap().to_string(),
            now: 202,
        },
    )
    .unwrap();
    failed.detail = Some(reference("failed"));
    failed.service_attachment_failure = Some(ApprovedServiceAttachmentFailure::Failed);
    super::validate_record(&failed, &f.request).unwrap();
    let observation = serde_json::json!({
        "phase": "initialize", "class": {"kind": "closed", "exit": {"kind": "code", "value": 1}},
    });
    let mut data = super::encode_record(&failed).unwrap();
    data.body["service_attachment_observation"] = observation.clone();
    let (events, effects) = super::handoffs(&failed, effects).unwrap();
    let stored = f
        .store
        .compare_and_swap(Transition {
            key: reserved.stored.key.clone(),
            expected_revision: reserved.stored.revision,
            expected_digest: Some(reserved.stored.content_digest.clone()),
            claim: None,
            data,
            events,
            effects,
        })
        .await
        .unwrap();
    assert_eq!(stored.revision, failed.state.revision);
    assert_eq!(failed.state.terminal_revision, failed.state.revision);
    // The old reader refuses the new field here. The new reader must retain it
    // without changing the stored row; actual native delivery has its own test.
    let obligation = f
        .bridge()
        .committed_continuation(&id(&snapshot.key().id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&obligation).unwrap()["service_attachment_observation"],
        observation
    );
    assert_eq!(f.store.get(snapshot.key()).await.unwrap().unwrap(), stored);
}
