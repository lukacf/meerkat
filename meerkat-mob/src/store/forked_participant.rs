//! Durable store contract for source-owned forked-participant capabilities.
//!
//! One row per capability record. The row carries three separable things:
//!
//! * the **immutable sidecar** — the request shape, the planned child identity,
//!   and (once the fork exists) the full [`ForkedParticipantRef`];
//! * the **generated machine state** — the canonical
//!   `ForkedParticipantLifecycleMachine` state, which is the only lifecycle
//!   truth;
//! * **optimistic concurrency + typed cleanup debt** — a revision for CAS and
//!   the typed detail of a failed archive.
//!
//! The store never interprets lifecycle legality. It loads, compares, and
//! compare-and-swaps; every transition is decided by the machine in the service
//! layer above.

use super::MobStoreError;
use crate::forked_participant::{
    ForkedParticipantCapabilityId, ForkedParticipantCleanupClaim, ForkedParticipantCleanupDebt,
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute, ForkedParticipantRef,
    ForkedParticipantRequestId, ForkedParticipantReusePolicy,
};
use crate::ids::AgentIdentity;
use crate::machines::forked_participant_lifecycle::ForkedParticipantLifecycleMachineState;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_core::SessionId;
use serde::{Deserialize, Serialize};

/// Immutable request/ref/provenance sidecar of one capability record.
///
/// Nothing here is lifecycle state. It is the durable copy of what the source
/// owner promised, so a restarted owner can rebuild the exact same capability
/// reference without re-deriving it from mutable runtime state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantSidecar {
    /// Source member whose conversation was forked.
    pub source_identity: AgentIdentity,
    /// Source session the fork was requested against.
    pub source_session_id: SessionId,
    /// Typed route to the owning runtime.
    pub owner_route: ForkedParticipantOwnerRoute,
    /// Operations the holder may perform.
    pub scope: ForkedParticipantOperationScope,
    /// One-shot or bounded reuse.
    pub reuse: ForkedParticipantReusePolicy,
    /// Absolute expiry instant computed once, with checked arithmetic, at
    /// reservation time.
    pub expires_at: DateTime<Utc>,
    /// Complete-boundary prefix length requested; `None` selected the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_prefix_message_count: Option<usize>,
    /// Full immutable capability reference, present once the fork activated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_ref: Option<ForkedParticipantRef>,
}

/// One durable forked-participant capability record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkedParticipantRecord {
    /// Bearer identity of the capability (primary key).
    pub capability_id: ForkedParticipantCapabilityId,
    /// Caller-stable request identity (unique).
    pub request_id: ForkedParticipantRequestId,
    /// Fingerprint the machine compares on replay/conflict.
    pub request_fingerprint: String,
    /// Child session identity reserved BEFORE the fork was taken.
    pub planned_child_session_id: SessionId,
    /// Immutable request/ref/provenance sidecar.
    pub sidecar: ForkedParticipantSidecar,
    /// Canonical generated lifecycle state.
    pub machine_state: ForkedParticipantLifecycleMachineState,
    /// Typed cleanup failure detail, retained until the archive succeeds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_debt: Option<ForkedParticipantCleanupDebt>,
    /// Mechanical, crash-recoverable exclusive cleanup claim.
    ///
    /// Not lifecycle authority: it only stops two sweepers from archiving the
    /// same fork. Terminal cleanup completion stays machine-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_claim: Option<ForkedParticipantCleanupClaim>,
    /// Optimistic concurrency token.
    pub revision: u64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last mutation instant.
    pub updated_at: DateTime<Utc>,
}

impl ForkedParticipantRecord {
    /// Fork session identity, once the capability activated.
    pub fn fork_session_id(&self) -> Option<&SessionId> {
        self.sidecar
            .capability_ref
            .as_ref()
            .map(ForkedParticipantRef::fork_session_id)
    }
}

/// Durable store for source-owned forked-participant capability records.
///
/// Every mutation is an atomic load / machine transition / compare-and-swap
/// performed by the service above; the store exposes exactly the primitives
/// that sequence needs.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait ForkedParticipantStore: Send + Sync {
    /// Insert a freshly reserved record.
    ///
    /// Fails with [`MobStoreError::CasConflict`] when the capability id or the
    /// request id is already taken, or when another record already owns the
    /// planned child session. Fork-child identity is unique capability custody:
    /// a session may never be reserved by two capability records, including
    /// during the pre-activation crash window.
    async fn insert_reserved(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError>;

    /// Load a record by bearer identity alone.
    ///
    /// Only the owner's own maintenance paths use this. Holder-driven paths use
    /// [`Self::load_exact`], which compares the full immutable reference.
    async fn load_by_capability_id(
        &self,
        capability_id: &ForkedParticipantCapabilityId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError>;

    /// Load a record by caller-stable request identity.
    async fn load_by_request_id(
        &self,
        request_id: &ForkedParticipantRequestId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError>;

    /// Load the capability that owns an exact fork session.
    ///
    /// Fork-session identity is unique capability custody: ordinary resume
    /// admission uses this lookup to prevent a caller from bypassing the
    /// capability lease with the session id visible in a reference.
    ///
    /// It matches on the PLANNED child session id, which is the same id an
    /// activated [`ForkedParticipantRef::fork_session_id`] carries. Matching
    /// the planned id resolves an activated fork AND a durable-but-not-yet-
    /// activated planned child, so the crash window between "child is saved"
    /// and "activation is recorded" is not a containment blind spot: that child
    /// already holds the source's selected transcript prefix and is exactly as
    /// sensitive as an activated fork.
    ///
    /// Backends must answer this from an index, not a scan: it is called per
    /// session resume.
    async fn load_by_fork_session_id(
        &self,
        fork_session_id: &SessionId,
    ) -> Result<Option<ForkedParticipantRecord>, MobStoreError>;

    /// Load the record for a presented capability reference.
    ///
    /// The store compares the FULL immutable reference, not just the id: a
    /// presented reference whose route, provenance, scope, expiry, or reuse
    /// policy differs from the stored one is a tampered bearer and fails with
    /// [`MobStoreError::CasConflict`] rather than resolving to the record.
    async fn load_exact(
        &self,
        capability: &ForkedParticipantRef,
    ) -> Result<ForkedParticipantRecord, MobStoreError>;

    /// Compare-and-swap one record forward.
    ///
    /// `record.revision` is the revision the caller read; the stored row must
    /// still be at that revision or the write fails with
    /// [`MobStoreError::CasConflict`]. The returned record carries the next
    /// revision.
    async fn commit(
        &self,
        record: &ForkedParticipantRecord,
    ) -> Result<ForkedParticipantRecord, MobStoreError>;

    /// List every record, oldest first.
    ///
    /// Sweeps read this and then transition each record individually, so one
    /// failing record never blocks the rest.
    async fn list_all(&self) -> Result<Vec<ForkedParticipantRecord>, MobStoreError>;
}

/// Resolve a loaded record against a presented capability reference.
///
/// Shared by every backend so "the store compares the FULL immutable
/// reference" is one rule with one implementation: a record whose stored
/// reference is absent or differs in ANY field is a tampered or not-yet-active
/// bearer and never resolves.
pub(crate) fn forked_participant_exact_reference(
    record: ForkedParticipantRecord,
    presented: &ForkedParticipantRef,
) -> Result<ForkedParticipantRecord, MobStoreError> {
    let Some(stored) = record.sidecar.capability_ref.as_ref() else {
        return Err(MobStoreError::CasConflict(format!(
            "forked participant capability {} has no activated reference to compare",
            presented.capability_id().correlation_hint()
        )));
    };
    if stored != presented {
        return Err(MobStoreError::CasConflict(format!(
            "forked participant capability {} reference mismatch",
            presented.capability_id().correlation_hint()
        )));
    }
    Ok(record)
}

#[cfg(test)]
pub(crate) mod contract_tests {
    use super::*;
    use crate::forked_participant::{
        ForkedParticipantCleanupId, ForkedParticipantProvenance, ForkedParticipantRevocationId,
    };
    use crate::machines::forked_participant_lifecycle::{
        ForkedParticipantLifecycleInput, ForkedParticipantLifecycleMachineAuthority,
        ForkedParticipantLifecycleMachineMutator,
    };
    use meerkat_core::connection::RealmId;

    pub(crate) fn realm() -> RealmId {
        RealmId::parse("global").expect("realm")
    }

    pub(crate) fn reserved_machine_state(
        fingerprint: &str,
        max_uses: u64,
    ) -> ForkedParticipantLifecycleMachineState {
        let mut authority = ForkedParticipantLifecycleMachineAuthority::new();
        ForkedParticipantLifecycleMachineMutator::apply(
            &mut authority,
            ForkedParticipantLifecycleInput::Reserve {
                request_fingerprint: fingerprint.to_owned(),
                max_uses,
            },
        )
        .expect("reserve");
        authority.state().clone()
    }

    pub(crate) fn record(request: &str) -> ForkedParticipantRecord {
        let now = Utc::now();
        let request_id = ForkedParticipantRequestId::new(request).expect("request id");
        let fingerprint = format!("sha256:{request}");
        ForkedParticipantRecord {
            capability_id: ForkedParticipantCapabilityId::mint().expect("mint"),
            request_id,
            request_fingerprint: fingerprint.clone(),
            planned_child_session_id: SessionId::new(),
            sidecar: ForkedParticipantSidecar {
                source_identity: AgentIdentity::from("researcher"),
                source_session_id: SessionId::new(),
                owner_route: ForkedParticipantOwnerRoute::Local { realm_id: realm() },
                scope: ForkedParticipantOperationScope::InvokeAndObserve,
                reuse: ForkedParticipantReusePolicy::OneShot,
                expires_at: now + chrono::Duration::seconds(600),
                requested_prefix_message_count: Some(2),
                capability_ref: None,
            },
            machine_state: reserved_machine_state(&fingerprint, 1),
            cleanup_debt: None,
            cleanup_claim: None,
            revision: 0,
            created_at: now,
            updated_at: now,
        }
    }

    pub(crate) fn activated(record: &ForkedParticipantRecord) -> ForkedParticipantRecord {
        let mut activated = record.clone();
        activated.sidecar.capability_ref = Some(ForkedParticipantRef::new_source_owned(
            record.capability_id.clone(),
            record.sidecar.source_identity.clone(),
            record.planned_child_session_id.clone(),
            record.sidecar.owner_route.clone(),
            ForkedParticipantProvenance {
                source_session_id: record.sidecar.source_session_id.clone(),
                prefix_message_count: 2,
                prefix_digest: "sha256:prefix".to_string(),
            },
            record.sidecar.scope,
            record.sidecar.expires_at,
            record.sidecar.reuse,
            ForkedParticipantRevocationId::for_request(&record.request_id),
            ForkedParticipantCleanupId::for_request(&record.request_id),
        ));
        activated
    }

    pub(crate) async fn insert_and_load_by_every_key<S: ForkedParticipantStore>(store: &S) {
        let record = record("req-insert");
        let stored = store.insert_reserved(&record).await.expect("insert");
        assert_eq!(stored.revision, 1, "a fresh insert starts at revision 1");

        let by_capability = store
            .load_by_capability_id(&record.capability_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(by_capability, stored);

        let by_request = store
            .load_by_request_id(&record.request_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(by_request, stored);

        assert!(
            store
                .load_by_request_id(&ForkedParticipantRequestId::new("absent").expect("request id"))
                .await
                .expect("load")
                .is_none()
        );
    }

    pub(crate) async fn duplicate_request_id_loses<S: ForkedParticipantStore>(store: &S) {
        let first = record("req-duplicate");
        store.insert_reserved(&first).await.expect("insert");

        let mut second = record("req-duplicate");
        second.request_fingerprint = first.request_fingerprint.clone();
        let error = store
            .insert_reserved(&second)
            .await
            .expect_err("a second reservation for the same request must lose");
        assert!(
            matches!(error, MobStoreError::CasConflict(_)),
            "expected CAS conflict, got {error:?}"
        );

        // The first reservation is untouched, including its planned child.
        let stored = store
            .load_by_request_id(&first.request_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(stored.capability_id, first.capability_id);
        assert_eq!(
            stored.planned_child_session_id,
            first.planned_child_session_id
        );
    }

    pub(crate) async fn commit_is_compare_and_swap<S: ForkedParticipantStore>(store: &S) {
        let record = record("req-cas");
        let stored = store.insert_reserved(&record).await.expect("insert");

        let mut next = activated(&stored);
        next.revision = stored.revision;
        let committed = store.commit(&next).await.expect("commit");
        assert_eq!(committed.revision, stored.revision + 1);
        assert!(committed.sidecar.capability_ref.is_some());

        // A stale writer loses.
        let mut stale = activated(&stored);
        stale.revision = stored.revision;
        let error = store.commit(&stale).await.expect_err("stale writer loses");
        assert!(
            matches!(error, MobStoreError::CasConflict(_)),
            "expected CAS conflict, got {error:?}"
        );

        // Committing an absent record is a typed not-found, never an insert.
        let mut absent = record.clone();
        absent.capability_id = ForkedParticipantCapabilityId::mint().expect("mint");
        absent.request_id = ForkedParticipantRequestId::new("req-cas-absent").expect("request id");
        absent.revision = 1;
        let error = store
            .commit(&absent)
            .await
            .expect_err("absent commit fails");
        assert!(
            matches!(error, MobStoreError::NotFound(_)),
            "expected NotFound, got {error:?}"
        );
    }

    pub(crate) async fn load_exact_rejects_a_tampered_reference<S: ForkedParticipantStore>(
        store: &S,
    ) {
        let record = record("req-tamper");
        let stored = store.insert_reserved(&record).await.expect("insert");
        let mut activated_record = activated(&stored);
        activated_record.revision = stored.revision;
        let committed = store.commit(&activated_record).await.expect("commit");
        let capability = committed
            .sidecar
            .capability_ref
            .clone()
            .expect("activated ref");

        let loaded = store.load_exact(&capability).await.expect("exact load");
        assert_eq!(loaded, committed);

        for tampered in tampered_variants(&capability) {
            let error = store
                .load_exact(&tampered)
                .await
                .expect_err("a tampered reference must never resolve");
            assert!(
                matches!(error, MobStoreError::CasConflict(_)),
                "expected CAS conflict for a tampered reference, got {error:?}"
            );
        }

        let mut unknown_value = serde_json::to_value(&capability).expect("serialize");
        unknown_value
            .as_object_mut()
            .expect("capability object")
            .insert(
                "capability_id".to_string(),
                serde_json::json!(
                    ForkedParticipantCapabilityId::mint()
                        .expect("mint")
                        .expose_bearer_token()
                ),
            );
        let unknown = serde_json::from_value::<ForkedParticipantRef>(unknown_value)
            .expect("an unknown bearer is still well typed");
        let error = store
            .load_exact(&unknown)
            .await
            .expect_err("an unknown bearer must not resolve");
        assert!(
            matches!(error, MobStoreError::NotFound(_)),
            "expected NotFound, got {error:?}"
        );
    }

    /// Tampered references are produced the way an attacker would: by editing
    /// the serialized capability and presenting it back. Private fields make
    /// in-memory mutation impossible, so the wire is the only surface.
    fn tampered_variants(capability: &ForkedParticipantRef) -> Vec<ForkedParticipantRef> {
        let base = serde_json::to_value(capability).expect("serialize capability");
        let edits: Vec<(&str, serde_json::Value)> = vec![
            ("scope", serde_json::json!("invoke")),
            (
                "expires_at",
                serde_json::json!(capability.expires_at() + chrono::Duration::seconds(60)),
            ),
            (
                "reuse",
                serde_json::json!({"kind": "bounded_reuse", "max_uses": 9}),
            ),
            (
                "owner_route",
                serde_json::json!({"kind": "host", "realm_id": "global", "host_id": "host-x"}),
            ),
            ("fork_session_id", serde_json::json!(SessionId::new())),
            ("source_identity", serde_json::json!("impostor")),
        ];

        let mut tampered = Vec::new();
        for (field, value) in edits {
            let mut edited = base.clone();
            edited
                .as_object_mut()
                .expect("capability object")
                .insert(field.to_string(), value);
            tampered.push(
                serde_json::from_value::<ForkedParticipantRef>(edited)
                    .expect("a tampered-but-well-typed capability still parses"),
            );
        }

        let mut provenance_edit = base;
        provenance_edit
            .as_object_mut()
            .expect("capability object")
            .get_mut("provenance")
            .expect("provenance")
            .as_object_mut()
            .expect("provenance object")
            .insert(
                "prefix_digest".to_string(),
                serde_json::json!("sha256:other"),
            );
        tampered.push(
            serde_json::from_value::<ForkedParticipantRef>(provenance_edit)
                .expect("a tampered provenance still parses"),
        );
        tampered
    }

    pub(crate) async fn load_by_fork_session_id_resolves_planned_and_activated<
        S: ForkedParticipantStore,
    >(
        store: &S,
    ) {
        let record = record("req-fork-session");
        let stored = store.insert_reserved(&record).await.expect("insert");

        // A durable planned child resolves before activation: the crash window
        // between "child is saved" and "activation is recorded" must not be a
        // containment blind spot.
        let planned = store
            .load_by_fork_session_id(&stored.planned_child_session_id)
            .await
            .expect("load")
            .expect("planned child resolves");
        assert_eq!(planned, stored);

        // ...and still resolves once the capability is activated.
        let mut activated_record = activated(&stored);
        activated_record.revision = stored.revision;
        let committed = store.commit(&activated_record).await.expect("commit");
        let activated_lookup = store
            .load_by_fork_session_id(&committed.planned_child_session_id)
            .await
            .expect("load")
            .expect("activated fork resolves");
        assert_eq!(activated_lookup, committed);
        assert_eq!(
            activated_lookup
                .fork_session_id()
                .expect("activated fork session"),
            &committed.planned_child_session_id
        );

        assert!(
            store
                .load_by_fork_session_id(&SessionId::new())
                .await
                .expect("load")
                .is_none(),
            "an unrelated session must not resolve to a capability record"
        );
    }

    pub(crate) async fn fork_session_id_is_unique_across_records<S: ForkedParticipantStore>(
        store: &S,
    ) {
        let first = record("req-unique-fork-session-first");
        let stored = store.insert_reserved(&first).await.expect("insert first");
        let mut second = record("req-unique-fork-session-second");
        second.planned_child_session_id = stored.planned_child_session_id.clone();
        assert!(
            matches!(
                store.insert_reserved(&second).await,
                Err(MobStoreError::CasConflict(_))
            ),
            "one fork child session may have only one capability record"
        );
    }

    pub(crate) async fn list_all_returns_every_record<S: ForkedParticipantStore>(store: &S) {
        assert!(store.list_all().await.expect("list").is_empty());
        let first = record("req-list-1");
        let second = record("req-list-2");
        store.insert_reserved(&first).await.expect("insert");
        store.insert_reserved(&second).await.expect("insert");

        let listed = store.list_all().await.expect("list");
        assert_eq!(listed.len(), 2);
        let ids: Vec<_> = listed.iter().map(|row| row.request_id.clone()).collect();
        assert!(ids.contains(&first.request_id));
        assert!(ids.contains(&second.request_id));
    }

    pub(crate) async fn machine_state_and_cleanup_debt_round_trip<S: ForkedParticipantStore>(
        store: &S,
    ) {
        let record = record("req-roundtrip");
        let stored = store.insert_reserved(&record).await.expect("insert");
        assert_eq!(stored.machine_state, record.machine_state);

        let mut next = activated(&stored);
        next.revision = stored.revision;
        next.cleanup_debt = Some(ForkedParticipantCleanupDebt {
            fork_session_id: stored.planned_child_session_id.clone(),
            attempts: 2,
            last_error: "archive refused".to_string(),
            observed_at: Utc::now(),
        });
        let mut authority = ForkedParticipantLifecycleMachineAuthority::recover_from_state(
            next.machine_state.clone(),
        )
        .expect("recover");
        ForkedParticipantLifecycleMachineMutator::apply(
            &mut authority,
            ForkedParticipantLifecycleInput::RecordForkActivation {
                request_fingerprint: next.request_fingerprint.clone(),
                fork_activation_id: next.planned_child_session_id.to_string(),
            },
        )
        .expect("activate");
        next.machine_state = authority.state().clone();

        let committed = store.commit(&next).await.expect("commit");
        let reloaded = store
            .load_by_capability_id(&record.capability_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(reloaded, committed);
        assert_eq!(reloaded.machine_state, next.machine_state);
        assert_eq!(
            reloaded
                .cleanup_debt
                .as_ref()
                .map(|debt| debt.attempts)
                .unwrap_or_default(),
            2
        );
        // The restored machine state is still admissible authority.
        ForkedParticipantLifecycleMachineAuthority::recover_from_state(reloaded.machine_state)
            .expect("restored machine state must recover");
    }
}
