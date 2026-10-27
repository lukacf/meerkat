//! Real SQLite publication mechanics. Application policy, ingress, runtime
//! custody and canonical-parent composition are explicit fixtures, not adoption.
use super::*;
use crate::authority::test_support as native_fixture;
use crate::authority::*;
use crate::request::*;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_core::authorization::WorkAuthorizationContext;
use meerkat_core::connection::RealmId;
use std::sync::Arc;
use toolkit_store::*;

fn id(s: &str) -> Id {
    Id::new(s).unwrap()
}
fn rev(s: &str) -> Revision {
    Revision::new(s).unwrap()
}
fn principal(s: &str, kind: PrincipalKind) -> PrincipalRef {
    native_fixture::principal(kind, s)
}
struct References;
impl ContextReferenceOwner for References {
    fn verify_access(
        &self,
        _: &ContextReference,
        authority: &SetupRequestEvidence,
        context: &SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        authority.check_context(context)?;
        Ok(())
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    store: SqliteStore,
    authorization: WorkAuthorizationContext,
    authority: SetupRequestEvidence,
    destination: NativeWorkTarget,
    now: u64,
    deadline: u64,
    snapshot: SetupSnapshot,
    policy: StoredRecord,
    lease: EffectLease,
    backend: SetupWorkBackend,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_ingress_authority("source").await
    }
    async fn with_ingress_authority(ingress_authority: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let now = store.now_ms().await.unwrap() as u64;
        let requester = principal("person", PrincipalKind::Human);
        let destination = native_fixture::target("original-agent", &requester);
        let mut association = native_fixture::association(
            requester, destination.clone(), "root-one", "origin-one",
        ).candidate().clone();
        association.ingress_namespace.ingress.authority =
            principal(ingress_authority, PrincipalKind::ServiceAccount);
        let authority = SetupRequestEvidence::new(
            InputAuthorityAssociation::new(association).unwrap(),
            SetupProposalConstraints {
                tenant: id("tenant"),
                deployment: id("deployment"),
                policy_revision: rev("policy-one"),
                issued_at_ms: now - 10_000,
                expires_at_ms: now + 120_000,
                ceiling: Ceiling {
                    actions: [
                        ActionPermission {
                            action: Action::FindAndInstall,
                            resource: id("original-agent"),
                        },
                        ActionPermission {
                            action: Action::Delegate,
                            resource: id("setup-receiver"),
                        },
                    ]
                    .into(),
                    output_audiences: [id("person")].into(),
                    services: Default::default(),
                    frontends: Default::default(),
                },
            },
        )
        .unwrap();
        let authorization = native_fixture::current(authority.association());
        let claims = authority.constraints();
        let context = SetupUseContext {
            tenant: &claims.tenant,
            deployment: &claims.deployment,
            realm: &authority.association().candidate().ingress_namespace.realm,
            target: &destination,
            authorization: &authorization,
            policy_revision: &claims.policy_revision,
            now_ms: now,
        };
        let setup = validate_request(
            FindAndInstallRequest {
                request: "Find the service and set it up".into(),
                user_context: vec![],
                caller_observations: vec![],
            },
            HostSetupRequest {
                setup_helper: None,
                target: id("original-agent"),
                correlation: id("invocation-one"),
                parent_operation: Some(id("request-one")),
                continuation: TaskContinuation { native_original: None,
                    owner: id("runtime"),
                    task: id("request-one"),
                    revision: rev("1"),
                    delivery_key: id("reply-one"),
                    audience: [id("person")].into(),
                },
                environment_revision: rev("environment-one"),
                limits: SetupLimits {
                    deadline_ms: now + 60_000,
                    max_workers: 2,
                    max_effect_attempts: 3,
                },
            },
            &authority,
            &context,
            &References,
        )
        .unwrap();
        let bridge =
            SetupStore::new_conditional(&store, SetupScope::new(id("tenant"), id("deployment")))
                .unwrap();
        let snapshot = bridge
            .admit(&setup, &authority, &context, &References)
            .await
            .unwrap()
            .snapshot;
        let effect = bridge.original_work_effect(&snapshot).await.unwrap();
        let EffectClaimOutcome::Claimed(effect) = store
            .claim_effect(&effect.key, snapshot.stored.revision, "publisher", 60_000)
            .await
            .unwrap()
        else {
            panic!("claim")
        };
        let policy = store
            .admit(AdmissionRequest {
                key: StoreKey {
                    realm: "tenant".into(),
                    owner: "actual-policy-fixture".into(),
                    id: "selection".into(),
                },
                admission_key: "policy".into(),
                data: RecordData {
                    schema: "test.policy".into(),
                    version: 1,
                    body: serde_json::json!({"revision":1}),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap()
            .record()
            .clone();
        let backend = selected_backend(directory.path(), "backend-one");
        Self {
            backend,
            directory,
            store,
            authorization,
            authority,
            destination,
            now,
            deadline: now + 60_000,
            snapshot,
            policy,
            lease: effect.lease.unwrap(),
        }
    }
    fn bridge<'a>(&self, store: &'a SqliteStore) -> SetupStore<'a> {
        SetupStore::new_conditional(store, SetupScope::new(id("tenant"), id("deployment"))).unwrap()
    }
    fn context(&self, now: u64) -> SetupUseContext<'_> {
        let c = self.authority.constraints();
        SetupUseContext {
            tenant: &c.tenant,
            deployment: &c.deployment,
            realm: &self
                .authority
                .association()
                .candidate()
                .ingress_namespace
                .realm,
            target: &self.destination,
            authorization: &self.authorization,
            policy_revision: &c.policy_revision,
            now_ms: now,
        }
    }
    fn access<'a>(&'a self, context: &'a SetupUseContext<'a>) -> SetupAccess<'a> {
        SetupAccess {
            evidence: &self.authority,
            context,
            references: &References,
        }
    }
    fn conditions(&self) -> PreparedWorkPublicationConditions {
        PreparedWorkPublicationConditions {
            owners: vec![RecordCondition {
                key: self.policy.key.clone(),
                revision: self.policy.revision,
                content_digest: self.policy.content_digest.clone(),
                claim: None,
                dispatch_claim: None,
            }],
            dispatch_claim: self.lease.clone(),
            before_ms: self.deadline as i64,
        }
    }
    fn child_evidence(
        &self,
        proposal: &NativePreparedWorkProposal,
        now: u64,
    ) -> SetupRequestEvidence {
        let mut association = self.authority.association().candidate().clone();
        association.original_work = proposal.delegation.original_work.clone();
        association.target = proposal.destination();
        association.logical_executor = association.target.logical_owner.clone();
        association.ingress_namespace.realm = proposal.delegation.realm_id.clone();
        association.contributing_work = vec![self.authority.original_work().clone()];
        let mut constraints = self.authority.constraints().clone();
        constraints.issued_at_ms = now;
        constraints.expires_at_ms = proposal.delegation.expires_at_ms;
        constraints.ceiling = proposal.delegation.ceiling.clone();
        SetupRequestEvidence::new(
            InputAuthorityAssociation::new(association).unwrap(),
            constraints,
        )
        .unwrap()
    }
    async fn propose(&self, title: &str) -> NativePreparedWorkProposal {
        self.bridge(&self.store)
            .propose_native_work(
                &self.snapshot,
                self.access(&self.context(self.now)),
                &Preparer {
                    title: title.into(),
                    expires: self.deadline,
                },
                &self.backend,
            )
            .await
            .unwrap()
    }
}
struct Preparer {
    title: String,
    expires: u64,
}
impl SetupWorkPreparer for Preparer {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        Ok(SetupWorkPreparation {
            scope: HostWorkGraphScope::new(id("work-realm"), WorkNamespace::new("setup").unwrap())?,
            input_reference: ContextReference {
                owner: id("input-owner"),
                id: id("input-one"),
                revision: rev("1"),
                content_digest: ContentDigest::of_bytes(&canonical_json(request.request())?),
            },
            work_request: CreateWorkItemRequest {
                title: self.title.clone(),
                description: Some("Exact bytes.\n  Keep these spaces.".into()),
                ..Default::default()
            },
            delegation: Some(SetupWorkDelegation {
                destination: native_fixture::target(
                    "setup-receiver",
                    &principal("person", PrincipalKind::Human),
                ),
                original_work: native_fixture::original("child-one"),
                realm_id: RealmId::parse("core-realm").unwrap(),
                delegation_resource: id("setup-receiver"),
                ceiling: request.constraints().ceiling.clone(),
                expires_at_ms: self.expires,
                composition_digest: ContentDigest::of_bytes(b"exact profiles, tools and skills"),
            }),
        })
    }
}

#[tokio::test]
async fn native_v4_reopens_exact_child_and_payload_without_dispatch_availability() {
    let f = Fixture::new().await;
    let proposal = f.propose("original pinned work").await;
    let unavailable =
        SetupStore::new(&f.store, SetupScope::new(id("tenant"), id("deployment"))).unwrap();
    assert!(matches!(
        unavailable
            .propose_native_work(
                &f.snapshot,
                f.access(&f.context(f.now)),
                &Preparer {
                    title: "unused".into(),
                    expires: f.deadline
                },
                &f.backend,
            )
            .await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    let context = f.context(f.now);
    let published = f
        .bridge(&f.store)
        .publish_prepared_work(
            &f.snapshot,
            &proposal,
            f.child_evidence(&proposal, f.now),
            f.access(&context),
            f.conditions(),
        )
        .await
        .unwrap();
    let reference = published.child_reference().clone();
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    let recovered = f
        .bridge(&reopened)
        .load_prepared_work_exact(
            &operation_id(f.snapshot.request()).unwrap(),
            &reference,
            f.access(&context),
            &f.backend,
        )
        .await
        .unwrap();
    assert_eq!(published.child(), recovered.child());
    assert_eq!(published.digest(), recovered.digest());
    assert_eq!(
        canonical_json(published.work_request()).unwrap(),
        canonical_json(recovered.work_request()).unwrap()
    );
    assert_eq!(published.key(), proposal.key());
    assert_eq!(recovered.child_reference().clone(), reference);
    revise(&reopened, &f.policy).await;
    reopened
        .acknowledge_effect(
            &f.lease,
            EffectOutcome::Uncertain {
                evidence: f.policy.data.clone(),
            },
        )
        .await
        .unwrap();
    let mut old_conditions = f.conditions();
    old_conditions.before_ms = 1;
    let replay = f
        .bridge(&reopened)
        .publish_prepared_work(
            &f.snapshot,
            &proposal,
            f.child_evidence(&proposal, f.now),
            f.access(&context),
            old_conditions,
        )
        .await
        .unwrap();
    assert_eq!(replay.child(), published.child());
    assert!(matches!(
        f.bridge(&reopened)
            .prepare_work(
                &operation_id(f.snapshot.request()).unwrap(),
                f.access(&context),
                &Preparer {
                    title: "unused".into(),
                    expires: f.deadline
                }
            )
            .await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    assert!(
        reopened
            .list_effects(published.key(), None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reopened
            .read_events(published.key(), 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_same_native_evidence_converges_on_one_immutable_prepared_record() {
    let f = Arc::new(Fixture::new().await);
    let proposal = Arc::new(f.propose("same proposal").await);
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut tasks = vec![];
    for _ in 0..2 {
        let f = f.clone();
        let proposal = proposal.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let store = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
            let context = f.context(f.now);
            let child = f.child_evidence(&proposal, f.now);
            barrier.wait().await;
            f.bridge(&store)
                .publish_prepared_work(
                    &f.snapshot,
                    &proposal,
                    child,
                    f.access(&context),
                    f.conditions(),
                )
                .await
                .unwrap()
        }));
    }
    let first = tasks.remove(0).await.unwrap();
    let second = tasks.remove(0).await.unwrap();
    assert_eq!(first.child(), second.child());
    assert_eq!(first.digest(), second.digest());
    assert_eq!(f.store.get(first.key()).await.unwrap().unwrap().revision, 1);
    // A second observation time is different historical evidence, even when
    // the source-owned child identity and proposal are otherwise unchanged.
    assert!(matches!(
        f.bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &proposal,
                f.child_evidence(&proposal, f.now + 1),
                f.access(&f.context(f.now)),
                f.conditions(),
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));

    let changed = f.propose("different work under same slot").await;
    assert!(matches!(
        f.bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &changed,
                f.child_evidence(&changed, f.now),
                f.access(&f.context(f.now)),
                f.conditions()
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    let mut changed = f.conditions();
    changed.owners[0].revision += 1;
    assert!(matches!(
        f.bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &proposal,
                f.child_evidence(&proposal, f.now),
                f.access(&f.context(f.now)),
                changed
            )
            .await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
}

async fn revise(store: &SqliteStore, prior: &StoredRecord) {
    store
        .compare_and_swap(Transition {
            key: prior.key.clone(),
            expected_revision: prior.revision,
            expected_digest: Some(prior.content_digest.clone()),
            claim: None,
            data: RecordData {
                schema: prior.data.schema.clone(),
                version: prior.data.version,
                body: serde_json::json!({"revoked":true}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
}
#[tokio::test]
async fn changed_policy_cancellation_and_settled_dispatch_refuse_without_prepared_record() {
    for failure in ["policy", "cancel", "claim", "deadline"] {
        let f = Fixture::new().await;
        let proposal = f.propose("work").await;
        let mut conditions = f.conditions();
        match failure {
            "policy" => revise(&f.store, &f.policy).await,
            "cancel" => {
                f.bridge(&f.store)
                    .resolve(
                        &f.snapshot,
                        Resolution::Cancelled(ContextReference {
                            owner: id("input-owner"),
                            id: id("cancel"),
                            revision: rev("1"),
                            content_digest: ContentDigest::of_bytes(b"cancel"),
                        }),
                        None,
                        f.access(&f.context(f.now)),
                    )
                    .await
                    .unwrap();
            }
            "claim" => {
                f.store
                    .acknowledge_effect(
                        &f.lease,
                        EffectOutcome::Uncertain {
                            evidence: f.policy.data.clone(),
                        },
                    )
                    .await
                    .unwrap();
            }
            "deadline" => conditions.before_ms = 1,
            _ => unreachable!(),
        }
        let result = f
            .bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &proposal,
                f.child_evidence(&proposal, f.now),
                f.access(&f.context(f.now)),
                conditions,
            )
            .await;
        assert!(result.is_err(), "{failure}");
        assert!(f.store.get(proposal.key()).await.unwrap().is_none());
        assert!(
            f.store
                .list_effects(proposal.key(), None, 100)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn legacy_changed_lineage_unknown_fields_and_mutated_record_never_recover_as_native() {
    for mutation in ["legacy", "version-two", "unknown", "lineage", "revision"] {
        let f = Fixture::new().await;
        let proposal = f.propose("work").await;
        let child = f.child_evidence(&proposal, f.now);
        let reference = child.original_work().clone();
        let mut owners = f.conditions().owners;
        let basis = publication_basis(
            &[
                owners.remove(0),
                RecordCondition {
                    key: f.snapshot.key().clone(),
                    revision: f.snapshot.stored.revision,
                    content_digest: f.snapshot.stored.content_digest.clone(),
                    claim: None,
                    dispatch_claim: Some(f.lease.clone()),
                },
            ],
            proposal.key(),
        )
        .unwrap();
        let mut record = NativePreparedRecord {
            helper_source: None,
            work: proposal.work.clone(),
            metadata: proposal.metadata.clone(),
            delegation: proposal.delegation.clone(),
            child,
            basis,
        };
        let mut version = VERSION;
        if mutation == "lineage" {
            record.work.effect_digest = "wrong".into();
        }
        let mut body = serde_json::to_value(&record).unwrap();
        if mutation == "legacy" {
            version = 1;
            body = serde_json::to_value(&record.work).unwrap();
        }
        if mutation == "version-two" {
            version = 2;
            body.as_object_mut().unwrap().remove("metadata");
        }
        if mutation == "unknown" {
            body["work"]["request"]["unknown_authority"] = serde_json::json!("discard me");
        }
        let saved = f
            .store
            .admit(AdmissionRequest {
                key: proposal.key.clone(),
                admission_key: proposal.admission_key.clone(),
                data: RecordData {
                    schema: SCHEMA.into(),
                    version,
                    body,
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        if mutation == "revision" {
            let r = saved.record();
            f.store
                .compare_and_swap(Transition {
                    key: r.key.clone(),
                    expected_revision: 1,
                    expected_digest: Some(r.content_digest.clone()),
                    claim: None,
                    data: r.data.clone(),
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap();
        }
        let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
        let recovered = f
            .bridge(&reopened)
            .load_prepared_work_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &reference,
                f.access(&f.context(f.now)),
                &f.backend,
            )
            .await;
        if matches!(mutation, "legacy" | "version-two") {
            assert!(
                matches!(recovered, Err(SetupError::UnsupportedRecordVersion)),
                "{mutation}"
            );
        } else {
            assert!(recovered.is_err(), "{mutation}");
        }
        assert!(
            f.bridge(&reopened)
                .load_prepared_child_for_operation(
                    &operation_id(f.snapshot.request()).unwrap(),
                    &proposal.destination(),
                    &f.authority,
                )
                .await
                .is_err(),
            "operation lookup must preserve {mutation} refusal"
        );
    }
}

#[tokio::test]
async fn wrong_parent_exact_reference_and_locator_scope_are_rejected() {
    let f = Fixture::new().await;
    let proposal = f.propose("work").await;
    let published = f
        .bridge(&f.store)
        .publish_prepared_work(
            &f.snapshot,
            &proposal,
            f.child_evidence(&proposal, f.now),
            f.access(&f.context(f.now)),
            f.conditions(),
        )
        .await
        .unwrap();
    let wrong = native_fixture::original("wrong-child");
    assert!(
        f.bridge(&f.store)
            .load_prepared_work_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &wrong,
                f.access(&f.context(f.now)),
                &f.backend
            )
            .await
            .is_err()
    );
    let other = SetupStore::new_conditional(
        &f.store,
        SetupScope::new(id("tenant"), id("other-deployment")),
    )
    .unwrap();
    assert!(matches!(
        other
            .load_prepared_work_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &published.child_reference().clone(),
                f.access(&f.context(f.now)),
                &f.backend
            )
            .await,
        Err(SetupError::Store(StoreError::NotFound))
    ));
    let mut wrong = f
        .child_evidence(&proposal, f.now)
        .association()
        .candidate()
        .clone();
    wrong.original_authentication = native_fixture::event("another-authentication");
    let wrong_child = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(wrong).unwrap(),
        f.child_evidence(&proposal, f.now).constraints().clone(),
    )
    .unwrap();
    assert!(
        f.bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &proposal,
                wrong_child,
                f.access(&f.context(f.now)),
                f.conditions()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn historical_exact_evidence_lookup_does_not_require_current_parent_use() {
    let f = Fixture::new().await;
    let proposal = f.propose("history").await;
    let context = f.context(f.now);
    let saved = f
        .bridge(&f.store)
        .publish_prepared_work(
            &f.snapshot,
            &proposal,
            f.child_evidence(&proposal, f.now),
            f.access(&context),
            f.conditions(),
        )
        .await
        .unwrap();
    let reference = saved.child_reference().clone();
    let expired = f.context(f.now + 200_000);
    assert!(f.authority.check_context(&expired).is_err());
    assert!(
        f.bridge(&f.store)
            .load_prepared_work_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &reference,
                f.access(&expired),
                &f.backend
            )
            .await
            .is_err()
    );
    let reopened = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
    assert_eq!(
        f.bridge(&reopened)
            .load_prepared_child_exact(
                &operation_id(f.snapshot.request()).unwrap(),
                &reference,
                &f.authority
            )
            .await
            .unwrap(),
        *saved.child()
    );
}

#[tokio::test]
async fn fixed_ceiling_original_composition_and_destination_changes_conflict() {
    for field in ["ceiling", "original", "composition", "destination"] {
        let f = Fixture::new().await;
        let proposal = f.propose("one fixed proposal").await;
        let context = f.context(f.now);
        f.bridge(&f.store)
            .publish_prepared_work(
                &f.snapshot,
                &proposal,
                f.child_evidence(&proposal, f.now),
                f.access(&context),
                f.conditions(),
            )
            .await
            .unwrap();
        let mut changed = proposal.clone();
        match field {
            "ceiling" => changed.delegation.ceiling.output_audiences.clear(),
            "original" => {
                changed.delegation.original_work = native_fixture::original("other-original");
            }
            "composition" => {
                changed.delegation.composition_digest = ContentDigest::of_bytes(b"other profile")
            }
            "destination" => {
                let mut destination = changed.destination();
                destination.logical_owner =
                    principal("another-receiver", PrincipalKind::SharedAgent);
                changed.delegation.destination = destination;
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                f.bridge(&f.store)
                    .publish_prepared_work(
                        &f.snapshot,
                        &changed,
                        f.child_evidence(&changed, f.now),
                        f.access(&context),
                        f.conditions()
                    )
                    .await,
                Err(SetupError::Store(StoreError::AdmissionConflict))
            ),
            "{field}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn policy_race_on_separate_connections_has_one_serialized_order() {
    for _ in 0..4 {
        let f = Arc::new(Fixture::new().await);
        let proposal = Arc::new(f.propose("race").await);
        // Test-only transactional trace: trigger rows roll back with the owner
        // transaction, and their rowids observe actual SQLite serialization.
        let trace = rusqlite::Connection::open(f.directory.path().join("store.sqlite")).unwrap();
        trace
            .execute_batch(
                "CREATE TABLE publication_trace (kind TEXT NOT NULL);
            CREATE TRIGGER trace_prepared AFTER INSERT ON toolkit_records
            WHEN NEW.owner='toolkit/prepared-work/deployment'
            BEGIN INSERT INTO publication_trace(kind) VALUES('published'); END;
            CREATE TRIGGER trace_revoked AFTER UPDATE ON toolkit_records
            WHEN NEW.owner='actual-policy-fixture' AND NEW.revision=2
            BEGIN INSERT INTO publication_trace(kind) VALUES('revoked'); END;",
            )
            .unwrap();
        drop(trace);
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let publisher = {
            let f = f.clone();
            let proposal = proposal.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                let store = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
                let context = f.context(f.now);
                let child = f.child_evidence(&proposal, f.now);
                barrier.wait().await;
                f.bridge(&store)
                    .publish_prepared_work(
                        &f.snapshot,
                        &proposal,
                        child,
                        f.access(&context),
                        f.conditions(),
                    )
                    .await
            })
        };
        let revoker = {
            let f = f.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                let store = SqliteStore::open(f.directory.path().join("store.sqlite")).unwrap();
                barrier.wait().await;
                store
                    .compare_and_swap(Transition {
                        key: f.policy.key.clone(),
                        expected_revision: 1,
                        expected_digest: Some(f.policy.content_digest.clone()),
                        claim: None,
                        data: RecordData {
                            schema: "test.policy".into(),
                            version: 1,
                            body: serde_json::json!({"revoked":true}),
                        },
                        events: vec![EventWrite {
                            kind: "revoked".into(),
                            data: f.policy.data.clone(),
                        }],
                        effects: vec![],
                    })
                    .await
                    .unwrap();
            })
        };
        let result = publisher.await.unwrap();
        revoker.await.unwrap();
        match result {
            Ok(saved) => {
                let db =
                    rusqlite::Connection::open(f.directory.path().join("store.sqlite")).unwrap();
                let id = |kind: &str| {
                    db.query_row(
                        "SELECT rowid FROM publication_trace WHERE kind=?1",
                        [kind],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap()
                };
                assert!(id("published") < id("revoked"));
                assert_eq!(saved.key(), proposal.key());
            }
            Err(SetupError::Store(StoreError::Conflict)) => {
                assert!(f.store.get(proposal.key()).await.unwrap().is_none())
            }
            _ => panic!("unexpected policy race result"),
        }
        assert_eq!(
            f.store.get(&f.policy.key).await.unwrap().unwrap().revision,
            2
        );
    }
}

#[tokio::test]
async fn lost_publication_reply_recovers_by_operation_after_cancellation_and_claim_settlement() {
    struct DeniedReferences;
    impl ContextReferenceOwner for DeniedReferences {
        fn verify_access(
            &self,
            _: &ContextReference,
            _: &SetupRequestEvidence,
            _: &SetupUseContext<'_>,
        ) -> Result<(), RequestError> {
            Err(RequestError::ContextUnavailable)
        }
    }
    struct NeverPrepare;
    impl SetupWorkPreparer for NeverPrepare {
        fn prepare(
            &self,
            _: &ValidatedSetupRequest,
        ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
            panic!("read-only recovery cannot call a preparer or enable dispatch")
        }
    }

    let (directory, operation, expected_destination) = {
        let f = Fixture::new().await;
        // Independent canonical parent owner fixture. Cold recovery below reads
        // this file, never the acquisition's copied parent or original bridge.
        std::fs::write(
            f.directory.path().join("canonical-parent.json"),
            serde_json::to_vec(&f.authority).unwrap(),
        )
        .unwrap();
        let proposal = f.propose("original work survives a lost reply").await;
        let operation = operation_id(f.snapshot.request()).unwrap();
        let destination = proposal.destination();
        // Drop the publication result immediately. In particular, retain no
        // child reference/digest, original observation time or publication basis.
        drop(
            f.bridge(&f.store)
                .publish_prepared_work(
                    &f.snapshot,
                    &proposal,
                    f.child_evidence(&proposal, f.now),
                    f.access(&f.context(f.now)),
                    f.conditions(),
                )
                .await
                .unwrap(),
        );
        // All candidate, parent evidence, old snapshot, policy, claim,
        // connection and reply state leave scope. Only routing facts survive.
        (f.directory, operation, destination)
    };
    {
        // Independent control/effect owners act after the publication process
        // is gone. They read their own current records, not its retained basis.
        let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let bridge =
            SetupStore::new(&store, SetupScope::new(id("tenant"), id("deployment"))).unwrap();
        let root: SetupRequestEvidence = serde_json::from_slice(
            &std::fs::read(directory.path().join("canonical-parent.json")).unwrap(),
        )
        .unwrap();
        let parent = root;
        let authorization = native_fixture::current(parent.association());
        let claims = parent.constraints();
        let destination = parent.association().candidate().target.clone();
        let context = SetupUseContext {
            tenant: &claims.tenant,
            deployment: &claims.deployment,
            realm: &parent.association().candidate().ingress_namespace.realm,
            target: &destination,
            authorization: &authorization,
            policy_revision: &claims.policy_revision,
            now_ms: store.now_ms().await.unwrap() as u64,
        };
        let snapshot = bridge
            .load(&operation, &parent, &context, &References)
            .await
            .unwrap();
        let effect = bridge.original_work_effect(&snapshot).await.unwrap();
        bridge
            .cancel(
                &operation,
                ContextReference {
                    owner: id("input-owner"),
                    id: id("cancel-after-publication"),
                    revision: rev("1"),
                    content_digest: ContentDigest::of_bytes(b"cancel after publication"),
                },
                SetupAccess {
                    evidence: &parent,
                    context: &context,
                    references: &References,
                },
            )
            .await
            .unwrap();
        let policy = store
            .get(&StoreKey {
                realm: "tenant".into(),
                owner: "actual-policy-fixture".into(),
                id: "selection".into(),
            })
            .await
            .unwrap()
            .unwrap();
        store
            .acknowledge_effect(
                effect.lease.as_ref().unwrap(),
                EffectOutcome::Uncertain {
                    evidence: policy.data.clone(),
                },
            )
            .await
            .unwrap();
        revise(&store, &policy).await;
    }
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let backend = selected_backend(directory.path(), "backend-one");
    // Recovery is a read and does not need ConditionalAdmissionStore assembly.
    let bridge = SetupStore::new(&store, SetupScope::new(id("tenant"), id("deployment"))).unwrap();
    let root: SetupRequestEvidence = serde_json::from_slice(
        &std::fs::read(directory.path().join("canonical-parent.json")).unwrap(),
    )
    .unwrap();
    let parent = root;
    let authorization = native_fixture::current(parent.association());
    let claims = parent.constraints();
    let parent_destination = parent.association().candidate().target.clone();
    let context = SetupUseContext {
        tenant: &claims.tenant,
        deployment: &claims.deployment,
        realm: &parent.association().candidate().ingress_namespace.realm,
        target: &parent_destination,
        authorization: &authorization,
        policy_revision: &claims.policy_revision,
        now_ms: store.now_ms().await.unwrap() as u64,
    };
    let access = || SetupAccess {
        evidence: &parent,
        context: &context,
        references: &References,
    };
    let key = bridge.preparation_key(&operation);
    let before = store.get(&key).await.unwrap().unwrap();
    let acquisition_before = store
        .get(&bridge.scope.key(&operation))
        .await
        .unwrap()
        .unwrap();
    let effects_before = store
        .list_effects(&bridge.scope.key(&operation), None, 100)
        .await
        .unwrap();
    let events_before = store
        .read_events(&bridge.scope.key(&operation), 0, 100)
        .await
        .unwrap();
    let recovered = bridge
        .load_prepared_work_for_operation(&operation, &expected_destination, access(), &backend)
        .await
        .unwrap();
    // The raw saved child is only a post-restart test oracle, never an input
    // supplied to the recovery operation or a second retained owner.
    assert_eq!(
        serde_json::to_value(recovered.child()).unwrap(),
        before.data.body["child"]
    );
    assert_eq!(
        recovered.work_request().title,
        "original work survives a lost reply"
    );
    let snapshot = bridge
        .load(&operation, &parent, &context, &References)
        .await
        .unwrap();
    assert_ne!(snapshot.state().terminal_revision, 0);
    let original_effect = bridge.original_work_effect(&snapshot).await.unwrap();
    assert_eq!(original_effect.status, EffectStatus::Uncertain);
    assert!(original_effect.lease.is_none());
    assert_eq!(
        bridge
            .load_prepared_child_for_operation(&operation, &expected_destination, &parent)
            .await
            .unwrap(),
        *recovered.child()
    );
    let recovered_ref = recovered.child_reference().clone();
    assert_eq!(
        bridge
            .load_prepared_child_exact(&operation, &recovered_ref, &parent)
            .await
            .unwrap(),
        *recovered.child()
    );
    assert!(
        bridge
            .load_prepared_work_for_operation(
                &operation,
                &expected_destination,
                SetupAccess {
                    evidence: &parent,
                    context: &context,
                    references: &DeniedReferences
                },
                &backend,
            )
            .await
            .is_err()
    );
    for field in ["owner", "runtime", "context"] {
        let mut wrong = expected_destination.clone();
        match field {
            "owner" => {
                wrong.logical_owner = principal("wrong-receiver", PrincipalKind::SharedAgent)
            }
            "runtime" => {
                wrong.logical_runtime =
                    meerkat_authorization_contracts::evidence::EvidenceId::new("wrong-runtime")
                        .unwrap()
            }
            "context" => {
                wrong.context =
                    meerkat_authorization_contracts::evidence::EvidenceId::new("wrong-context")
                        .unwrap()
            }
            _ => unreachable!(),
        }
        assert!(matches!(
            bridge
                .load_prepared_child_for_operation(&operation, &wrong, &parent)
                .await,
            Err(SetupError::WrongScope)
        ));
        assert!(matches!(
            bridge
                .load_prepared_work_for_operation(&operation, &wrong, access(), &backend)
                .await,
            Err(SetupError::WrongScope)
        ));
    }
    assert!(
        bridge
            .load_prepared_child_for_operation(
                &id("missing-operation"),
                &expected_destination,
                &parent,
            )
            .await
            .is_err()
    );
    let expired = SetupUseContext {
        tenant: context.tenant,
        deployment: context.deployment,
        realm: context.realm,
        target: context.target,
        authorization: context.authorization,
        policy_revision: context.policy_revision,
        now_ms: claims.expires_at_ms + 1,
    };
    assert!(parent.check_context(&expired).is_err());
    assert!(
        bridge
            .load_prepared_work_for_operation(
                &operation,
                &expected_destination,
                SetupAccess {
                    evidence: &parent,
                    context: &expired,
                    references: &References
                },
                &backend,
            )
            .await
            .is_err()
    );
    assert_eq!(
        bridge
            .load_prepared_child_for_operation(&operation, &expected_destination, &parent)
            .await
            .unwrap(),
        *recovered.child()
    );
    assert!(matches!(
        bridge
            .prepare_work(&operation, access(), &NeverPrepare)
            .await,
        Err(SetupError::NativeExecutionUnavailable)
    ));
    assert_eq!(store.get(&key).await.unwrap().unwrap(), before);
    assert_eq!(
        store
            .get(&bridge.scope.key(&operation))
            .await
            .unwrap()
            .unwrap(),
        acquisition_before
    );
    assert_eq!(
        store
            .list_effects(&bridge.scope.key(&operation), None, 100)
            .await
            .unwrap(),
        effects_before
    );
    assert_eq!(
        store
            .read_events(&bridge.scope.key(&operation), 0, 100)
            .await
            .unwrap(),
        events_before
    );
    assert!(
        store
            .list_effects(&key, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.read_events(&key, 0, 100).await.unwrap().is_empty());
}

#[tokio::test]
async fn operation_recovery_requires_the_independent_exact_parent_and_configured_scope() {
    let f = Fixture::new().await;
    let proposal = f.propose("work").await;
    f.bridge(&f.store)
        .publish_prepared_work(
            &f.snapshot,
            &proposal,
            f.child_evidence(&proposal, f.now),
            f.access(&f.context(f.now)),
            f.conditions(),
        )
        .await
        .unwrap();
    let operation = operation_id(f.snapshot.request()).unwrap();
    let destination = proposal.destination();
    let mut wrong_association = f.authority.association().candidate().clone();
    wrong_association.original_work = native_fixture::original("different-canonical-parent");
    let wrong_parent = SetupRequestEvidence::new(
        InputAuthorityAssociation::new(wrong_association).unwrap(),
        f.authority.constraints().clone(),
    )
    .unwrap();
    assert!(matches!(
        f.bridge(&f.store)
            .load_prepared_child_for_operation(&operation, &destination, &wrong_parent)
            .await,
        Err(SetupError::InvalidRecord)
    ));
    let other_scope = SetupStore::new(
        &f.store,
        SetupScope::new(id("tenant"), id("other-deployment")),
    )
    .unwrap();
    assert!(
        other_scope
            .load_prepared_child_for_operation(&operation, &destination, &f.authority)
            .await
            .is_err()
    );
}

fn selected_backend(directory: &std::path::Path, generation: &str) -> SetupWorkBackend {
    use meerkat_core::service::WorkGraphNamespaceGrant;
    use meerkat_core::storage_durability::{DurabilityDeclaration, DurabilityResolution};
    use meerkat_workgraph::{SqliteWorkGraphStore, WorkGraphService};
    // Real selected SQLite store plus an explicit fixture declaration. This is
    // not a claim that a production provider wrapper is already wired.
    let selected =
        Arc::new(SqliteWorkGraphStore::open(directory.join("workgraph.sqlite")).unwrap());
    let service = WorkGraphService::with_namespace_grant(
        selected,
        WorkGraphNamespaceGrant::new("work-realm", "setup").unwrap(),
    )
    .unwrap();
    SetupWorkBackend::new(
        service,
        "fixture-provider".into(),
        id(generation),
        &[DurabilityDeclaration::durable(
            "workgraph",
            DurabilityResolution::Persistent,
        )],
    )
    .unwrap()
}

mod v3_metadata;

mod pre_worker;
