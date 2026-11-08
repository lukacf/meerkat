//! Real SQLite orchestration controls, reusing the existing WorkGraph fixture.
//! Native authority, lead spawn/delivery and physical cleanup are not exercised.
use super::shared_preparation::backend;
use super::*;

async fn acquisition(
    f: &mut Fixture,
    backend: &SetupWorkBackend,
    parent: &meerkat_workgraph::WorkItem,
    name: &str,
) -> SetupSnapshot {
    // Distinct fixture Originals share only the explicitly selected principal
    // and parent. These are storage controls, not native authority proofs.
    f.authority = SetupRequestEvidence::new(
        native_fixture::association(
            principal("person-a"),
            f.destination.clone(),
            &format!("original-{name}"),
            &format!("private-{name}"),
        ),
        f.request.constraints().clone(),
    )
    .unwrap();
    f.authorization = native_fixture::current(f.authority.association());
    super::shared_preparation::acquisition(f, backend, parent, name).await
}

fn terms(snapshot: &SetupSnapshot) -> SetupWorkflowTerms {
    let work = snapshot.request().host().work_context.as_ref().unwrap();
    let claims = snapshot.request().evidence().association().candidate();
    SetupWorkflowTerms {
        key: id("host-workflow-1"),
        backend: work.backend().clone(),
        parent: work.parent().clone(),
        parent_owner: work.parent_owner().clone(),
        cycle_label: work.cycle_label().into(),
        requester: claims.requester.clone(),
        realm: claims.ingress_namespace.realm.clone(),
        authority_revision: snapshot.request().constraints().policy_revision.clone(),
        planning_source: StoreKey {
            realm: snapshot.key().realm.clone(),
            owner: "fixture-planning".into(),
            id: "lead-1".into(),
        },
        lead: meerkat_core::MobMemberBinding {
            mob_id: "fixture-mob".into(),
            role: "setup".into(),
            member: "one-lead".into(),
        },
        envelope_digest: ContentDigest::of_bytes(b"host-frozen-setup-envelope"),
        deadline_ms: 1000,
        max_waiters: 8,
    }
}
fn member(
    f: &Fixture,
    terms: &SetupWorkflowTerms,
    snapshot: &SetupSnapshot,
) -> SetupWorkflowWaiter {
    f.bridge()
        .workflow_waiter(
            terms,
            snapshot,
            StoreKey {
                realm: snapshot.key().realm.clone(),
                owner: "fixture-source".into(),
                id: snapshot.key().id.clone(),
            },
        )
        .unwrap()
}
async fn join(
    f: &Fixture,
    terms: &SetupWorkflowTerms,
    waiter: &SetupWorkflowWaiter,
    snapshot: &SetupSnapshot,
    backend: &SetupWorkBackend,
) -> Result<SetupWorkflowJoin, SetupError> {
    let evidence = snapshot.request().evidence();
    let authorization = native_fixture::current(evidence.association());
    let context = SetupUseContext {
        tenant: &f.tenant,
        deployment: &f.deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &evidence.association().candidate().target,
        policy_revision: &f.policy,
        now_ms: 200,
        authorization: &authorization,
    };
    f.bridge()
        .join_setup_workflow(
            terms,
            waiter,
            SetupAccess {
                evidence,
                context: &context,
                references: &f.references,
            },
            backend,
        )
        .await
}

#[tokio::test]
async fn concurrent_waiters_commit_one_launch_and_independent_delivery_intents() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "waiter-a").await;
    let b = acquisition(&mut f, &backend, &parent, "waiter-b").await;
    let terms = terms(&a);
    let ma = member(&f, &terms, &a);
    let mb = member(&f, &terms, &b);
    assert!(ma.input() != mb.input());
    assert!(a.request().original_work() != b.request().original_work());
    assert_ne!(
        a.request().authority_digest().unwrap(),
        b.request().authority_digest().unwrap()
    );
    let (ja, jb) = tokio::join!(
        join(&f, &terms, &ma, &a, &backend),
        join(&f, &terms, &mb, &b, &backend)
    );
    assert!(ja.unwrap().joined);
    assert!(jb.unwrap().joined);
    let current = f.bridge().load_setup_workflow(ma.workflow()).await.unwrap();
    assert_eq!(current.waiter_count(), 2);
    assert_eq!(current.active_waiters(), 2);
    let before = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert_eq!(before.len(), 3);
    assert_eq!(
        before
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_launch")
            .count(),
        1
    );
    assert!(
        before
            .iter()
            .all(|effect| effect.status == EffectStatus::Pending && effect.attempt == 0)
    );
    for waiter in [&ma, &mb] {
        let effect = f
            .store
            .get_effect(&waiter.delivery_effect().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            effect.intent.request.body["payload"],
            serde_json::to_value(waiter).unwrap()
        );
        assert_eq!(
            effect.intent.replay_safety,
            ReplaySafety::ReconcileBeforeRetry
        );
    }
    // Exact normal replay finds the same committed obligation without rewriting it.
    assert!(!join(&f, &terms, &ma, &a, &backend).await.unwrap().joined);
    assert!(!join(&f, &terms, &mb, &b, &backend).await.unwrap().joined);
    assert_eq!(
        f.store
            .list_effects(ma.workflow().key(), None, 16)
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        f.bridge()
            .load_setup_workflow(ma.workflow())
            .await
            .unwrap()
            .revision(),
        current.revision()
    );
    assert_eq!(
        f.store.get(a.key()).await.unwrap().unwrap().revision,
        a.condition().revision
    );
    assert_eq!(
        f.store.get(b.key()).await.unwrap().unwrap().revision,
        b.condition().revision
    );
}

#[tokio::test]
async fn changed_workflow_terms_source_and_parent_cannot_join_or_duplicate_effects() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "waiter-a").await;
    let terms = terms(&a);
    let ma = member(&f, &terms, &a);
    join(&f, &terms, &ma, &a, &backend).await.unwrap();
    let before = f.store.get(ma.workflow().key()).await.unwrap().unwrap();
    let effects = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    let mut changed = terms.clone();
    changed.envelope_digest = ContentDigest::of_bytes(b"different-envelope");
    let changed_member = member(&f, &changed, &a);
    assert!(matches!(
        join(&f, &changed, &changed_member, &a, &backend).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    let changed_member = f
        .bridge()
        .workflow_waiter(
            &terms,
            &a,
            StoreKey {
                realm: a.key().realm.clone(),
                owner: "another-source".into(),
                id: a.key().id.clone(),
            },
        )
        .unwrap();
    assert!(matches!(
        join(&f, &terms, &changed_member, &a, &backend).await,
        Err(SetupError::Store(StoreError::AdmissionConflict))
    ));
    changed = terms.clone();
    changed.parent_owner = meerkat_workgraph::WorkOwnerKey::agent("foreign-parent").unwrap();
    assert!(matches!(
        f.bridge()
            .workflow_waiter(&changed, &a, ma.source().clone()),
        Err(SetupError::WrongScope)
    ));
    changed = terms.clone();
    changed.requester = principal("another-person");
    assert!(matches!(
        f.bridge()
            .workflow_waiter(&changed, &a, ma.source().clone()),
        Err(SetupError::WrongScope)
    ));
    changed = terms.clone();
    changed.authority_revision = rev("policy-other");
    assert!(matches!(
        f.bridge()
            .workflow_waiter(&changed, &a, ma.source().clone()),
        Err(SetupError::WrongScope)
    ));
    let foreign = SetupStore::new(
        &f.store,
        SetupScope::new(f.tenant.clone(), id("other-deployment")),
    )
    .unwrap();
    assert!(foreign.load_setup_workflow(ma.workflow()).await.is_err());
    assert_eq!(
        f.store.get(ma.workflow().key()).await.unwrap().unwrap(),
        before
    );
    assert_eq!(
        f.store
            .list_effects(ma.workflow().key(), None, 16)
            .await
            .unwrap(),
        effects
    );
}

#[tokio::test]
async fn reopen_preserves_lost_ack_launch_and_old_outcome_but_creates_late_waiter_delivery() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "waiter-a").await;
    let b = acquisition(&mut f, &backend, &parent, "waiter-b").await;
    let c = acquisition(&mut f, &backend, &parent, "waiter-c").await;
    let terms = terms(&a);
    let ma = member(&f, &terms, &a);
    let mb = member(&f, &terms, &b);
    let mc = member(&f, &terms, &c);
    join(&f, &terms, &ma, &a, &backend).await.unwrap();
    join(&f, &terms, &mb, &b, &backend).await.unwrap();
    let current = f.bridge().load_setup_workflow(ma.workflow()).await.unwrap();
    // This is an actual outbox outcome with an explicitly synthetic receipt,
    // not a claimed native input admission or an executed lead.
    let key = ma.delivery_effect().unwrap();
    let claimed = f
        .store
        .claim_effect(&key, current.revision(), "fixture-worker", 10000)
        .await
        .unwrap();
    let EffectClaimOutcome::Claimed(effect) = claimed else {
        panic!("fresh delivery must be claimable")
    };
    let completed = f
        .store
        .acknowledge_effect(
            effect.lease.as_ref().unwrap(),
            EffectOutcome::Succeeded {
                receipt: RecordData {
                    schema: "fixture-only-not-native".into(),
                    version: 1,
                    body: serde_json::json!({"waiter": "a"}),
                },
            },
        )
        .await
        .unwrap();
    let launch_key = ma.workflow().launch_effect();
    // Simulate loss of the dispatch owner's reply, not successful native launch.
    // Only this actual existing outbox claim is persisted before owner loss.
    let EffectClaimOutcome::Claimed(launch) = f
        .store
        .claim_effect(&launch_key, current.revision(), "lost-reply-owner", 1)
        .await
        .unwrap()
    else {
        panic!("fresh launch must be claimable")
    };
    let launch_expiry = launch.lease.as_ref().unwrap().expires_at_ms;
    let left = f.bridge().depart_setup_workflow(&ma).await.unwrap();
    assert_eq!(left.active_waiters(), 1);
    assert!(!left.sealed());
    // Drop the only original writable SQLite owner before reopening it.
    let path = f.directory.path().join("toolkit.sqlite");
    let replacement = SqliteStore::open(f.directory.path().join("unused.sqlite")).unwrap();
    let original = std::mem::replace(&mut f.store, replacement);
    drop(original);
    f.store = SqliteStore::open_existing(&path).unwrap();
    // An exact join replay after reopening cannot duplicate either obligation.
    let before_replay = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert!(!join(&f, &terms, &mb, &b, &backend).await.unwrap().joined);
    assert_eq!(
        f.store
            .list_effects(ma.workflow().key(), None, 16)
            .await
            .unwrap(),
        before_replay
    );
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while f.store.now_ms().await.unwrap() < launch_expiry {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual SQLite lease clock must reach the bounded expiry");
    let current = f.bridge().load_setup_workflow(ma.workflow()).await.unwrap();
    let EffectClaimOutcome::NeedsReconciliation(unconfirmed) = f
        .store
        .claim_effect(&launch_key, current.revision(), "reopened-owner", 10000)
        .await
        .unwrap()
    else {
        panic!("lost launch reply must never authorize another dispatch")
    };
    assert_eq!(unconfirmed.intent, launch.intent);
    assert_eq!(unconfirmed.attempt, 1);
    assert_eq!(unconfirmed.status, EffectStatus::Uncertain);
    assert!(unconfirmed.outcome.is_none());
    assert!(unconfirmed.lease.is_none());
    let joined = join(&f, &terms, &mc, &c, &backend).await.unwrap();
    assert!(joined.joined);
    assert_eq!(joined.snapshot.active_waiters(), 2);
    assert!(joined.snapshot.departed(&ma).unwrap());
    assert!(!joined.snapshot.departed(&mc).unwrap());
    assert_eq!(f.store.get_effect(&key).await.unwrap().unwrap(), completed);
    assert_eq!(
        f.store.get_effect(&launch_key).await.unwrap().unwrap(),
        unconfirmed
    );
    let delivery_c = f
        .store
        .get_effect(&mc.delivery_effect().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(delivery_c.status, EffectStatus::Pending);
    assert!(delivery_c.outcome.is_none());
    assert_eq!(delivery_c.attempt, 0);
    assert!(mc.input() != ma.input());
    assert_eq!(
        f.store.get(c.key()).await.unwrap().unwrap().revision,
        c.condition().revision
    );
    f.bridge().depart_setup_workflow(&mb).await.unwrap();
    assert!(
        f.store
            .get_effect(&ma.workflow().close_effect())
            .await
            .unwrap()
            .is_none()
    );
    let closed = f.bridge().depart_setup_workflow(&mc).await.unwrap();
    assert!(closed.sealed());
    assert_eq!(closed.active_waiters(), 0);
    let close = f
        .store
        .get_effect(&ma.workflow().close_effect())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(close.status, EffectStatus::Pending);
    assert!(close.outcome.is_none());
    f.bridge().depart_setup_workflow(&mc).await.unwrap();
    assert_eq!(
        f.store
            .get_effect(&ma.workflow().close_effect())
            .await
            .unwrap()
            .unwrap(),
        close
    );
    assert_eq!(f.store.get_effect(&key).await.unwrap().unwrap(), completed);
}

#[tokio::test]
async fn join_racing_final_departure_cannot_reopen_a_sealed_workflow() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "waiter-a").await;
    let b = acquisition(&mut f, &backend, &parent, "waiter-b").await;
    let terms = terms(&a);
    let ma = member(&f, &terms, &a);
    let mb = member(&f, &terms, &b);
    join(&f, &terms, &ma, &a, &backend).await.unwrap();
    let bridge = f.bridge();
    let (left, added) = tokio::join!(
        bridge.depart_setup_workflow(&ma),
        join(&f, &terms, &mb, &b, &backend)
    );
    left.unwrap();
    let current = bridge.load_setup_workflow(ma.workflow()).await.unwrap();
    assert!(current.departed(&ma).unwrap());
    match added {
        Ok(joined) => {
            assert!(joined.joined);
            assert!(!current.sealed());
            assert_eq!(current.active_waiters(), 1);
            assert!(
                f.store
                    .get_effect(&ma.workflow().close_effect())
                    .await
                    .unwrap()
                    .is_none()
            );
            bridge.depart_setup_workflow(&mb).await.unwrap();
        }
        Err(SetupError::Rejected) => {
            assert!(current.sealed());
            assert_eq!(current.active_waiters(), 0);
            assert!(
                f.store
                    .get_effect(&mb.delivery_effect().unwrap())
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        Err(error) => panic!("unexpected join failure: {error}"),
    }
    let effects = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert_eq!(
        effects
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_close")
            .count(),
        1
    );
    assert_eq!(
        effects
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_launch")
            .count(),
        1
    );
}

#[tokio::test]
async fn explicit_seal_and_capacity_preserve_existing_waiters_until_last_departure() {
    let mut f = Fixture::new();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "waiter-a").await;
    let b = acquisition(&mut f, &backend, &parent, "waiter-b").await;
    let mut terms = terms(&a);
    terms.max_waiters = 1;
    let ma = member(&f, &terms, &a);
    let mb = member(&f, &terms, &b);
    join(&f, &terms, &ma, &a, &backend).await.unwrap();
    assert!(matches!(
        join(&f, &terms, &mb, &b, &backend).await,
        Err(SetupError::Rejected)
    ));
    let sealed = f.bridge().seal_setup_workflow(ma.workflow()).await.unwrap();
    assert!(sealed.sealed());
    assert_eq!(sealed.active_waiters(), 1);
    assert!(
        f.store
            .get_effect(&ma.workflow().close_effect())
            .await
            .unwrap()
            .is_none()
    );
    let replay = join(&f, &terms, &ma, &a, &backend).await.unwrap();
    assert!(!replay.joined);
    assert!(replay.snapshot.sealed());
    assert_eq!(replay.snapshot.revision(), sealed.revision());
    let left = f.bridge().depart_setup_workflow(&ma).await.unwrap();
    assert_eq!(left.active_waiters(), 0);
    assert!(left.sealed());
    assert!(matches!(
        join(&f, &terms, &mb, &b, &backend).await,
        Err(SetupError::Rejected)
    ));
    let replay = join(&f, &terms, &ma, &a, &backend).await.unwrap();
    assert!(!replay.joined);
    assert!(replay.snapshot.departed(&ma).unwrap());
    let effects = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert_eq!(effects.len(), 3);
}

async fn conditional_join(
    f: &Fixture,
    terms: &SetupWorkflowTerms,
    waiter: &SetupWorkflowWaiter,
    snapshot: &SetupSnapshot,
    backend: &SetupWorkBackend,
    source: &StoredRecord,
) -> Result<SetupWorkflowJoin, SetupError> {
    let evidence = snapshot.request().evidence();
    let authorization = native_fixture::current(evidence.association());
    let context = SetupUseContext {
        tenant: &f.tenant,
        deployment: &f.deployment,
        realm: &evidence.association().candidate().ingress_namespace.realm,
        target: &evidence.association().candidate().target,
        policy_revision: &f.policy,
        now_ms: 200,
        authorization: &authorization,
    };
    SetupStore::new_conditional(
        &f.store,
        SetupScope::new(f.tenant.clone(), f.deployment.clone()),
    )
    .unwrap()
    .join_setup_workflow_conditionally(
        terms,
        waiter,
        SetupAccess {
            evidence,
            context: &context,
            references: &f.references,
        },
        backend,
        toolkit_store::RecordCondition {
            key: source.key.clone(),
            revision: source.revision,
            content_digest: source.content_digest.clone(),
            claim: None,
            dispatch_claim: None,
        },
    )
    .await
}

#[tokio::test]
async fn conditional_a_b_and_late_c_keep_independent_membership_and_delivery() {
    let mut f = Fixture::new();
    // This remains a storage/WorkGraph fixture. Its synthetic request authority
    // uses the existing context clock; the conditional store uses real time.
    let deadline = u64::try_from(f.store.now_ms().await.unwrap()).unwrap() + 60_000;
    let mut constraints = f.request.constraints().clone();
    constraints.expires_at_ms = deadline + 1_000;
    f.authority =
        SetupRequestEvidence::new(f.authority.association().clone(), constraints).unwrap();
    f.authorization = native_fixture::current(f.authority.association());
    let mut host = f.request.host().clone();
    host.limits.deadline_ms = deadline;
    f.request = validate_request(
        f.request.request().clone(),
        host,
        &f.authority,
        &f.context(200),
        &f.references,
    )
    .unwrap();
    let (backend, parent) = backend(&f).await;
    let a = acquisition(&mut f, &backend, &parent, "conditional-a").await;
    let b = acquisition(&mut f, &backend, &parent, "conditional-b").await;
    let c = acquisition(&mut f, &backend, &parent, "conditional-c").await;
    let mut terms = terms(&a);
    terms.deadline_ms = deadline;
    let ma = member(&f, &terms, &a);
    let mb = member(&f, &terms, &b);
    let mc = member(&f, &terms, &c);
    let mut sources = Vec::new();
    for membership in [&ma, &mb, &mc] {
        sources.push(
            f.store
                .admit(AdmissionRequest {
                    key: membership.source().clone(),
                    admission_key: membership.source().id.clone(),
                    data: RecordData {
                        schema: "test.workflow_source".into(),
                        version: 1,
                        body: serde_json::json!({"cancelled":false}),
                    },
                    events: vec![],
                    effects: vec![],
                })
                .await
                .unwrap()
                .record()
                .clone(),
        );
    }
    let (ja, jb) = tokio::join!(
        conditional_join(&f, &terms, &ma, &a, &backend, &sources[0]),
        conditional_join(&f, &terms, &mb, &b, &backend, &sources[1]),
    );
    assert!(ja.unwrap().joined && jb.unwrap().joined);
    let before = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        3,
        "one launch, independent A/B delivery obligations"
    );
    let cancelled = f
        .store
        .compare_and_swap(Transition {
            key: sources[0].key.clone(),
            expected_revision: sources[0].revision,
            expected_digest: Some(sources[0].content_digest.clone()),
            claim: None,
            data: RecordData {
                schema: "test.workflow_source".into(),
                version: 1,
                body: serde_json::json!({"cancelled":true}),
            },
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    assert_ne!(cancelled.revision, sources[0].revision);
    // Recover the exact canonical A membership, independent of any retained
    // join response or launch-interest projection, then record its departure.
    let snapshot = f.bridge().load_setup_workflow(ma.workflow()).await.unwrap();
    let census = snapshot.waiters_for_source(&sources[0].key);
    assert!(census.len() == 1 && census[0] == ma);
    f.bridge().depart_setup_workflow(&census[0]).await.unwrap();
    let joined_c = conditional_join(&f, &terms, &mc, &c, &backend, &sources[2])
        .await
        .unwrap();
    assert!(joined_c.joined);
    assert_eq!(joined_c.snapshot.active_waiters(), 2);
    assert!(joined_c.snapshot.departed(&ma).unwrap());
    assert!(!joined_c.snapshot.departed(&mb).unwrap());
    assert!(!joined_c.snapshot.departed(&mc).unwrap());
    let effects = f
        .store
        .list_effects(ma.workflow().key(), None, 16)
        .await
        .unwrap();
    assert_eq!(
        effects.len(),
        4,
        "C gets its own pending context obligation, no second launch"
    );
    for prior in before {
        assert!(effects.iter().any(|current| current == &prior));
    }
    assert!(
        effects
            .iter()
            .all(|effect| effect.status == EffectStatus::Pending && effect.attempt == 0)
    );
    let replay = conditional_join(&f, &terms, &ma, &a, &backend, &sources[0])
        .await
        .unwrap();
    assert!(!replay.joined && replay.snapshot.departed(&ma).unwrap());
    assert_eq!(
        f.store
            .list_effects(ma.workflow().key(), None, 16)
            .await
            .unwrap(),
        effects
    );
    assert!(ma.input() != mb.input());
    assert!(ma.input() != mc.input());
}
