//! What one poisoned scope refuses, and what it leaves serving.
//!
//! A poisoned scope refuses every call that needs it until the realm is
//! reopened: its memory may differ from the host. A read across many
//! scopes (a store-wide listing) depends only on the scopes of the families
//! it reads. Where a listing enumerates a family that holds a poisoned
//! scope, it answers that scope fail closed (listed as holding work, or
//! named unavailable) or refuses typed; it never leaves the scope out, since
//! a missing entry reads as an absent one.

use super::*;
use crate::delivery_inbox::RuntimeDeliveryInbox;
use crate::store::{RefusedScope, UnavailableRuntime};
use meerkat_core::host_store::HostStoreError;

/// A runtime holding a nonterminal input: recovery work.
async fn busy_runtime(store: &SpiRuntimeStore, name: &str) -> LogicalRuntimeId {
    let runtime = LogicalRuntimeId::new(name);
    store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("a nonterminal input lands");
    runtime
}

/// A registered idle runtime holding a delivery authority and no work.
async fn quiet_runtime(store: &SpiRuntimeStore, name: &str) -> LogicalRuntimeId {
    let runtime = LogicalRuntimeId::new(name);
    store
        .commit_machine_lifecycle(&runtime, idle_lifecycle(&runtime), &[])
        .await
        .expect("the runtime is registered idle");
    let outcome = store
        .compare_and_swap_runtime_delivery_authority(
            &runtime,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            Some(delivery_record(1)),
        )
        .await
        .expect("delivery cas");
    assert!(matches!(
        outcome,
        RuntimeDeliveryAuthorityCasOutcome::Applied(_)
    ));
    runtime
}

/// One mob host binding (`mob-bound`) and one revoked binding
/// (`mob-revoked`), as the mob host actor writes them.
async fn mob_host_records(store: &SpiRuntimeStore) {
    assert!(
        store
            .put_mob_host_binding_if_absent("mob-bound", br#"{"binding":1}"#)
            .await
            .expect("a binding lands")
    );
    assert!(
        store
            .put_mob_host_binding_if_absent("mob-revoked", br#"{"binding":2}"#)
            .await
            .expect("a binding lands")
    );
    assert!(
        store
            .revoke_mob_host_binding("mob-revoked", br#"{"binding":2}"#, br#"{"receipt":1}"#)
            .await
            .expect("a revocation lands")
    );
}

/// Poison `runtime`'s scope: the store refuses the batch of an input write,
/// after memory changed.
async fn poison_runtime(host: &TestHost, store: &SpiRuntimeStore, runtime: &LogicalRuntimeId) {
    host.faults.lock().unwrap().conflict = 1;
    let refused = store
        .persist_input_state(
            runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(refused.is_err(), "the store refuses the batch: {refused:?}");
}

fn mob_host_bindings() -> Vec<(String, Vec<u8>)> {
    vec![("mob-bound".to_string(), br#"{"binding":1}"#.to_vec())]
}

fn mob_host_revocations() -> Vec<(String, Vec<u8>)> {
    vec![("mob-revoked".to_string(), br#"{"receipt":1}"#.to_vec())]
}

/// A poisoned scope of a family no runtime read touches (a mob event log,
/// a Meta document) leaves every store-wide read of the runtime adapter
/// answering, completely: the session catalog listing, the delivery
/// authority listing, the recovery scan, both mob host listings and the
/// controller custody. Before, one poisoned scope of any family refused
/// all of them until a restart.
#[tokio::test]
async fn a_poisoned_scope_of_another_family_leaves_the_runtime_listings_answering() {
    let host = Arc::new(TestHost::new());
    let (realm, store) = open(&host).await;
    let busy = busy_runtime(&store, "busy").await;
    let quiet = quiet_runtime(&store, "quiet").await;
    mob_host_records(&store).await;

    let other = ScopeId::new(TEST_FAMILY, "poisoned");
    host.faults.lock().unwrap().conflict = 1;
    let refused = commit_one(
        &realm,
        &other,
        ScopeDelta::Changes(vec![("row/a".into(), Some(Arc::new(b"a".to_vec())))]),
    )
    .await;
    assert!(
        matches!(
            refused,
            Err(SpiError::Host(HostStoreError::Conflict { .. }))
        ),
        "{refused:?}"
    );
    assert!(realm.has_poisoned_scopes(), "the scope is poisoned");

    let catalog = RuntimeStore::list_runtime_session_catalog_entries(
        &store,
        meerkat_core::SessionFilter::default(),
    )
    .await;
    assert!(
        catalog.is_ok(),
        "the session catalog listing answers: {catalog:?}"
    );
    let authorities = store
        .list_runtime_delivery_authorities()
        .await
        .expect("the delivery authority listing answers");
    assert_eq!(
        authorities
            .into_iter()
            .map(|(runtime, _)| runtime)
            .collect::<Vec<_>>(),
        vec![quiet.clone()]
    );
    assert_eq!(
        store
            .list_runtimes_with_recovery_work()
            .await
            .expect("the recovery scan answers"),
        vec![busy.clone()]
    );
    assert_eq!(
        store
            .list_mob_host_bindings()
            .await
            .expect("the binding listing answers"),
        mob_host_bindings()
    );
    assert_eq!(
        store
            .list_mob_host_revocations()
            .await
            .expect("the revocation listing answers"),
        mob_host_revocations()
    );
    let claim = RuntimeStore::execution_custody(&store)
        .expect("custody")
        .try_acquire_governed()
        .expect("claim");
    let custody = RuntimeStore::try_controller_mutation_custody(&store, &claim);
    assert!(
        custody.is_ok(),
        "the controller custody is available: {:?}",
        custody.err()
    );
}

/// A poisoned runtime scope leaves the mob host listings answering: they
/// read only their own record families. The reads that depend on every
/// runtime still refuse while a runtime scope is poisoned (fail closed):
/// the session catalog listing and the controller custody, a veto over
/// every runtime.
#[tokio::test]
async fn a_poisoned_runtime_scope_leaves_the_mob_host_listings_answering() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    mob_host_records(&store).await;
    let doomed = LogicalRuntimeId::new("doomed");
    poison_runtime(&host, &store, &doomed).await;

    assert_eq!(
        store
            .list_mob_host_bindings()
            .await
            .expect("the binding listing answers"),
        mob_host_bindings()
    );
    assert_eq!(
        store
            .list_mob_host_revocations()
            .await
            .expect("the revocation listing answers"),
        mob_host_revocations()
    );
    let catalog = RuntimeStore::list_runtime_session_catalog_entries(
        &store,
        meerkat_core::SessionFilter::default(),
    )
    .await;
    assert!(
        matches!(
            &catalog,
            Err(RuntimeStoreError::ScopeRefused { scopes })
                if scopes.len() == 1 && scopes[0].scope == "rt:doomed"
        ),
        "the session catalog listing refuses typed while a runtime scope is poisoned: {catalog:?}"
    );
    let claim = RuntimeStore::execution_custody(&store)
        .expect("custody")
        .try_acquire_governed()
        .expect("claim");
    let custody = RuntimeStore::try_controller_mutation_custody(&store, &claim).err();
    assert!(
        matches!(
            &custody,
            Some(RuntimeStoreError::ScopeRefused { scopes })
                if scopes.len() == 1 && scopes[0].scope == "rt:doomed"
        ),
        "the controller custody refuses typed while a runtime scope is poisoned: {custody:?}"
    );
}

/// A poisoned mob host binding scope refuses the binding listing, typed and
/// naming the scope, while the revocation listing and the runtime listings
/// answer: the host may hold a binding record memory does not, so the
/// listing never answers without it.
#[tokio::test]
async fn a_poisoned_mob_host_binding_scope_refuses_the_binding_listing_typed() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    mob_host_records(&store).await;
    host.faults.lock().unwrap().conflict = 1;
    let refused = store
        .compare_and_put_mob_host_binding("mob-bound", br#"{"binding":1}"#, br#"{"binding":3}"#)
        .await;
    assert!(refused.is_err(), "the store refuses the batch: {refused:?}");

    let bindings = store.list_mob_host_bindings().await;
    assert!(
        matches!(
            &bindings,
            Err(RuntimeStoreError::ScopeRefused { scopes })
                if scopes.len() == 1
                    && scopes[0].scope == "mhb:mob-bound"
                    && !scopes[0].cause.is_empty()
        ),
        "the binding listing refuses typed, naming the poisoned scope: {bindings:?}"
    );
    assert_eq!(
        store
            .list_mob_host_revocations()
            .await
            .expect("the revocation listing answers"),
        mob_host_revocations()
    );
    RuntimeStore::list_runtime_session_catalog_entries(
        &store,
        meerkat_core::SessionFilter::default(),
    )
    .await
    .expect("the session catalog listing answers");
}

/// A revoke whose batch the host refused poisons the mob's binding and
/// revocation scopes, and both listings refuse, typed and naming them. The
/// revocation listing never answers without the revoked mob: memory does
/// not hold the revocation the host may hold, and a revocation read as
/// absent would let the revoked binding act (fail open).
#[tokio::test]
async fn a_poisoned_mob_host_revocation_scope_refuses_the_revocation_listing_typed() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    mob_host_records(&store).await;
    host.faults.lock().unwrap().conflict = 1;
    let refused = store
        .revoke_mob_host_binding("mob-bound", br#"{"binding":1}"#, br#"{"receipt":2}"#)
        .await;
    assert!(refused.is_err(), "the store refuses the batch: {refused:?}");

    let revocations = store.list_mob_host_revocations().await;
    assert!(
        matches!(
            &revocations,
            Err(RuntimeStoreError::ScopeRefused { scopes })
                if scopes.len() == 1
                    && scopes[0].scope == "mhr:mob-bound"
                    && !scopes[0].cause.is_empty()
        ),
        "the revocation listing refuses typed, never answers without the revoked mob: {revocations:?}"
    );
    let bindings = store.list_mob_host_bindings().await;
    assert!(
        matches!(
            &bindings,
            Err(RuntimeStoreError::ScopeRefused { scopes })
                if scopes.len() == 1 && scopes[0].scope == "mhb:mob-bound"
        ),
        "the binding listing refuses typed too: {bindings:?}"
    );
    RuntimeStore::list_runtime_session_catalog_entries(
        &store,
        meerkat_core::SessionFilter::default(),
    )
    .await
    .expect("the session catalog listing answers");
}

/// A call refused by a poisoned scope says so typed, not as a plain read
/// or write failure: the refusal lasts until the realm is reopened, and a
/// caller that reads it as an ordinary failure retries a call that can only
/// refuse again. Its own writes, its own reads and the store-wide reads that
/// need it all answer the typed refusal.
#[tokio::test]
async fn a_poisoned_scope_refuses_typed_until_the_realm_reopens() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let doomed = LogicalRuntimeId::new("doomed");
    poison_runtime(&host, &store, &doomed).await;

    let write = store
        .persist_input_state(
            &doomed,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect_err("the poisoned scope refuses a write");
    let read = store
        .load_input_states(&doomed)
        .await
        .expect_err("the poisoned scope refuses a read");
    let listing = RuntimeStore::list_runtime_session_catalog_entries(
        &store,
        meerkat_core::SessionFilter::default(),
    )
    .await
    .expect_err("the session catalog listing refuses");
    for refusal in [write, read, listing] {
        assert!(
            !matches!(
                refusal,
                RuntimeStoreError::WriteFailed(_) | RuntimeStoreError::ReadFailed(_)
            ),
            "a refusal until the realm reopens stays typed: {refusal:?}"
        );
        let RuntimeStoreError::ScopeRefused { scopes } = &refusal else {
            panic!("the poisoned scope's typed refusal: {refusal:?}");
        };
        assert_eq!(scopes.len(), 1, "{refusal:?}");
        assert_eq!(scopes[0].scope, "rt:doomed");
        assert!(!scopes[0].cause.is_empty(), "{refusal:?}");
        assert!(refusal.is_refused_until_reopen());
        assert!(
            refusal
                .to_string()
                .contains("host store scope rt:doomed is poisoned: "),
            "{refusal}"
        );
    }

    // A poisoned realm refuses every call, typed too.
    let _ = store
        .realm()
        .poison_all("the row poisons the realm".to_string());
    let refusal = store
        .load_input_states(&LogicalRuntimeId::new("healthy"))
        .await
        .expect_err("a poisoned realm refuses every call");
    assert!(
        matches!(
            &refusal,
            RuntimeStoreError::RealmRefused { cause } if cause == "the row poisons the realm"
        ),
        "{refusal:?}"
    );
    assert!(refusal.is_refused_until_reopen());
}

/// The adapter's poison errors map to the typed refusals for reads and
/// writes alike, naming every scope with its cause; any other adapter
/// failure maps as before.
#[test]
fn a_poisoned_scope_or_realm_maps_to_its_typed_refusal() {
    let scope = SpiError::ScopePoisoned {
        scope: "rt:a".to_string(),
        cause: "batch refused".to_string(),
    };
    let scopes = SpiError::ScopesPoisoned {
        scopes: vec![
            super::super::PoisonedScope {
                id: ScopeId::new(super::super::RUNTIME_FAMILY, "a"),
                cause: "batch refused".to_string(),
            },
            super::super::PoisonedScope {
                id: ScopeId::new(super::super::RUNTIME_FAMILY, "b"),
                cause: "outcome unknown".to_string(),
            },
        ],
    };
    let realm = SpiError::RealmPoisoned("fenced out".to_string());
    for into in [SpiError::into_read, SpiError::into_write] {
        let RuntimeStoreError::ScopeRefused { scopes: refused } = into(scope.clone()) else {
            panic!("a poisoned scope refuses typed");
        };
        assert_eq!(refused, vec![RefusedScope::new("rt:a", "batch refused")]);
        let RuntimeStoreError::ScopeRefused { scopes: refused } = into(scopes.clone()) else {
            panic!("poisoned scopes refuse typed");
        };
        assert_eq!(
            refused,
            vec![
                RefusedScope::new("rt:a", "batch refused"),
                RefusedScope::new("rt:b", "outcome unknown"),
            ]
        );
        assert!(matches!(
            into(realm.clone()),
            RuntimeStoreError::RealmRefused { cause } if cause == "fenced out"
        ));
    }
    assert_eq!(
        scopes.to_string(),
        "host store scopes rt:a, rt:b are poisoned (rt:a: batch refused; rt:b: outcome unknown)"
    );
    assert!(matches!(
        SpiError::LeaseLapsed("renew failed".to_string()).into_write(),
        RuntimeStoreError::WriteFailed(_)
    ));
}

/// The realm names every poisoned scope with its cause, and a read across
/// scopes refuses only for the families it names.
#[tokio::test]
async fn the_realm_names_each_poisoned_scope_and_refuses_only_the_families_read() {
    let host = Arc::new(TestHost::new());
    let (realm, store) = open(&host).await;
    assert!(realm.poisoned_scopes().is_empty());
    let doomed = LogicalRuntimeId::new("doomed");
    poison_runtime(&host, &store, &doomed).await;

    let poisoned = realm.poisoned_scopes();
    assert_eq!(poisoned.len(), 1, "{poisoned:?}");
    assert_eq!(
        poisoned[0].id,
        ScopeId::new(super::super::RUNTIME_FAMILY, "doomed")
    );
    assert!(!poisoned[0].cause.is_empty());
    assert_eq!(
        realm.poisoned_scopes_of(super::super::RUNTIME_FAMILY),
        poisoned
    );
    assert!(realm.poisoned_scopes_of(TEST_FAMILY).is_empty());
    assert!(realm.check_cross_scope_read_of(&[TEST_FAMILY]).is_ok());
    assert!(matches!(
        realm.check_cross_scope_read_of(&[TEST_FAMILY, super::super::RUNTIME_FAMILY]),
        Err(SpiError::ScopesPoisoned { scopes }) if scopes == poisoned
    ));
    assert!(matches!(
        realm.check_cross_scope_read(),
        Err(SpiError::ScopesPoisoned { scopes }) if scopes == poisoned
    ));
}

/// The delivery authority listing per runtime answers every healthy
/// runtime's record and names the poisoned runtime unavailable; the inbox's
/// backlog drains the healthy ones from it and keeps the unavailable one
/// apart. The answers that cannot be given while a runtime is unavailable,
/// the backlog as one population and its total, refuse typed, naming it.
#[tokio::test]
async fn the_delivery_backlog_names_a_poisoned_runtime_unavailable_and_answers_the_others() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let store = Arc::new(store);
    let inbox = RuntimeDeliveryInbox::new(Arc::clone(&store) as Arc<dyn RuntimeStore>);
    let delivered = LogicalRuntimeId::new("delivered");
    inbox
        .submit(
            &delivered,
            crate::delivery_inbox::RuntimeDeliverySubmission::new(
                crate::delivery_inbox::RuntimeDeliveryId::new("delivery").unwrap(),
                crate::delivery_inbox::RuntimeDeliveryKind::JobTerminal,
                "job",
                1,
                "interaction",
                b"payload".to_vec(),
            )
            .unwrap(),
        )
        .await
        .expect("a delivery lands");
    let doomed = LogicalRuntimeId::new("doomed");
    poison_runtime(&host, &store, &doomed).await;

    let listing = store
        .list_runtime_delivery_authorities_or_unavailable()
        .await
        .expect("the listing per runtime answers");
    assert_eq!(
        listing
            .records
            .iter()
            .map(|(runtime, _)| runtime.clone())
            .collect::<Vec<_>>(),
        vec![delivered.clone()]
    );
    assert_eq!(listing.unavailable.len(), 1, "{listing:?}");
    assert_eq!(listing.unavailable[0].runtime_id, doomed);
    assert_eq!(listing.unavailable[0].refusal.scope, "rt:doomed");

    let backlog = inbox
        .runtime_delivery_backlog()
        .await
        .expect("the backlog answers");
    assert_eq!(backlog.pending, vec![delivered]);
    assert_eq!(backlog.unavailable, listing.unavailable);
    for refusal in [
        inbox.runtimes_with_pending_deliveries().await.map(|_| ()),
        inbox.pending_delivery_total().await.map(|_| ()),
    ] {
        assert!(
            matches!(
                &refusal,
                Err(crate::delivery_inbox::RuntimeDeliveryError::Store(
                    RuntimeStoreError::ScopeRefused { scopes }
                )) if scopes.len() == 1 && scopes[0].scope == "rt:doomed"
            ),
            "{refusal:?}"
        );
    }
}

/// A store whose listing answers all or nothing keeps its answer per
/// runtime through the trait default: every record, nothing unavailable.
#[tokio::test]
async fn the_default_listing_per_runtime_answers_every_record() {
    let store = crate::store::InMemoryRuntimeStore::new();
    let runtime = LogicalRuntimeId::new("in-memory");
    store
        .compare_and_swap_runtime_delivery_authority(
            &runtime,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            None,
        )
        .await
        .expect("delivery cas");
    let listing = store
        .list_runtime_delivery_authorities_or_unavailable()
        .await
        .expect("the default answers");
    assert_eq!(
        listing.records,
        store.list_runtime_delivery_authorities().await.unwrap()
    );
    assert_eq!(listing.unavailable, Vec::<UnavailableRuntime>::new());
}

/// The recovery scan answers while a runtime scope is poisoned: every
/// healthy runtime as its memory holds it, and the poisoned runtime as
/// holding work, the scan's fail-closed answer for a runtime whose facts it
/// cannot trust. A runtime with no work stays out. Before, the scan refused,
/// and every mob boot over the realm woke every member.
#[tokio::test]
async fn a_poisoned_runtime_is_listed_as_holding_recovery_work_and_the_others_answer_exactly() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let busy = busy_runtime(&store, "busy").await;
    quiet_runtime(&store, "quiet").await;
    let doomed = LogicalRuntimeId::new("doomed");
    poison_runtime(&host, &store, &doomed).await;

    let mut listed = store
        .list_runtimes_with_recovery_work()
        .await
        .expect("the recovery scan answers");
    listed.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(listed, vec![busy, doomed]);
}

/// A managed entry in a poisoned scope is never reported absent. The
/// runtime's input landed, and the batch that would have consumed it was
/// refused: memory says the runtime holds no work, the host says it does.
/// The recovery scan lists it all the same, and the listings that cannot
/// answer for it refuse instead of leaving it out.
#[tokio::test]
async fn a_managed_entry_in_a_poisoned_scope_is_never_reported_absent() {
    let host = Arc::new(TestHost::new());
    let (_realm, store) = open(&host).await;
    let doomed = LogicalRuntimeId::new("doomed");
    store
        .commit_machine_lifecycle(&doomed, idle_lifecycle(&doomed), &[])
        .await
        .expect("the runtime is registered idle");
    let accepted = StoredInputState::new_accepted(InputId::new());
    store
        .persist_input_state(&doomed, &persistable(accepted.clone()))
        .await
        .expect("the input lands");
    let outcome = store
        .compare_and_swap_runtime_delivery_authority(
            &doomed,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, br#"{"state":1}"#.to_vec()),
            Some(delivery_record(1)),
        )
        .await
        .expect("delivery cas");
    assert!(matches!(
        outcome,
        RuntimeDeliveryAuthorityCasOutcome::Applied(_)
    ));
    assert_eq!(
        store
            .list_runtimes_with_recovery_work()
            .await
            .expect("the scan answers"),
        vec![doomed.clone()],
        "the landed input is work"
    );

    let mut consumed = accepted;
    consumed.seed.phase = InputLifecycleState::Consumed;
    consumed.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
    consumed.seed.recovery_lane = None;
    host.faults.lock().unwrap().conflict = 1;
    assert!(
        store
            .persist_input_state(&doomed, &persistable(consumed))
            .await
            .is_err(),
        "the store refuses the batch that consumes the input"
    );

    assert_eq!(
        store
            .list_runtimes_with_recovery_work()
            .await
            .expect("the recovery scan answers"),
        vec![doomed.clone()],
        "a poisoned runtime is listed as holding work, whatever its memory says"
    );
    assert!(
        store.list_runtime_delivery_authorities().await.is_err(),
        "the delivery authority listing refuses rather than leave the runtime out"
    );
    assert!(
        RuntimeStore::list_runtime_session_catalog_entries(
            &store,
            meerkat_core::SessionFilter::default()
        )
        .await
        .is_err(),
        "the session catalog listing refuses rather than leave the runtime out"
    );
    let listing = store
        .list_runtime_delivery_authorities_or_unavailable()
        .await
        .expect("the listing per runtime answers");
    assert!(
        listing.records.is_empty(),
        "the poisoned runtime's record is not answered from memory: {listing:?}"
    );
    assert_eq!(
        listing
            .unavailable
            .iter()
            .map(|unavailable| unavailable.runtime_id.clone())
            .collect::<Vec<_>>(),
        vec![doomed],
        "the poisoned runtime is named unavailable, not left out"
    );
}
