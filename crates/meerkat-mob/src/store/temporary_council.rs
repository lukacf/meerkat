//! Durable store contract for temporary-council orchestration custody.
//!
//! One row per council. The row carries four separable things:
//!
//! * the **immutable request binding** — the council id, the canonical request
//!   fingerprint, the deterministic temporary [`MobId`], and the convener of a
//!   council the agent-facing `council` tool convened;
//! * the **canonical lifecycle state** — the generated
//!   `TemporaryCouncilLifecycleMachine` state, which is the only phase truth —
//!   plus the per-participant custody and the ordered exchange receipts
//!   persisted before each send;
//! * the **immutable result** — present exactly once, never rewritten; and
//! * the **cleanup receipt** — retained and overwritten by later attempts
//!   until it settles.
//!
//! The store interprets no lifecycle. It inserts, loads, and compare-and-swaps.
//! Mob, member, and capability truth stay with `MobMachine`, the member
//! machines, and `ForkedParticipantLifecycleMachine`; this contract exists only
//! so a crashed coordinator can converge to a typed terminal result and a
//! settled cleanup instead of silently re-executing model work.

use super::MobStoreError;
use crate::ids::MobId;
use crate::machines::temporary_council_lifecycle::{
    TemporaryCouncilLifecycleEffect, TemporaryCouncilLifecycleInput,
    TemporaryCouncilLifecycleInputVariant, TemporaryCouncilLifecycleMachineAuthority,
    TemporaryCouncilLifecycleMachineMutator, TemporaryCouncilLifecycleMachineState,
    TemporaryCouncilLifecycleMachineTransitionError,
    TemporaryCouncilLifecycleMachineTransitionTrigger,
};
use crate::temporary_council::{
    TemporaryCouncilCleanupReceipt, TemporaryCouncilConvener, TemporaryCouncilDurability,
    TemporaryCouncilExchangeReceipt, TemporaryCouncilId, TemporaryCouncilJobBinding,
    TemporaryCouncilMobCustody, TemporaryCouncilParticipantCustody, TemporaryCouncilResult,
    TemporaryCouncilStoreDurability,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One durable temporary-council record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilRecord {
    /// Council identity (primary key).
    pub council_id: TemporaryCouncilId,
    /// Canonical fingerprint of the accepted request.
    ///
    /// A later call under the same council id must present this exact
    /// fingerprint; anything else is a conflicting request, not a retry.
    pub request_fingerprint: String,
    /// Deterministic temporary mob this council owns.
    pub temporary_mob_id: MobId,
    /// What the council recorded about the mob it creates under
    /// `temporary_mob_id`: whether its create was issued, and the
    /// incarnation that create made. Seating and cleanup act on that
    /// incarnation only. `None` in a record written before this was
    /// recorded: such a record cannot name its mob, and its cleanup falls
    /// back to the id under the guards its coordinator documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary_mob_custody: Option<TemporaryCouncilMobCustody>,
    /// Absolute deadline computed once, with checked arithmetic, before work.
    pub deadline: DateTime<Utc>,
    /// Durability the caller declared for this council.
    pub durability: TemporaryCouncilDurability,
    /// Absolute expiry of the CURRENT coordinator claim's lease.
    ///
    /// Sidecar, not authority: the machine reads no clock. A recovering
    /// coordinator compares this to the wall clock once and passes the
    /// resulting `lease_expired` observation into the claim command; the
    /// machine decides what that observation means.
    pub claim_lease_expires_at: DateTime<Utc>,
    /// Canonical generated lifecycle state. The store never interprets it.
    pub machine_state: TemporaryCouncilLifecycleMachineState,
    /// Per-participant custody in deterministic slot order.
    pub participants: Vec<TemporaryCouncilParticipantCustody>,
    /// Ordered exchange receipts. Each is persisted before its send.
    pub exchanges: Vec<TemporaryCouncilExchangeReceipt>,
    /// The immutable result, once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TemporaryCouncilResult>,
    /// The most recent cleanup attempt's receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<TemporaryCouncilCleanupReceipt>,
    /// The convener's detached job awaiting this council's outcome, when the
    /// council was run detached. Lets a restarted host deliver the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detached_job: Option<TemporaryCouncilJobBinding>,
    /// Who convened the council through the agent-facing `council` tool:
    /// the session that tool's surface is bound to. `None` for a council a
    /// host runs directly, and in records written before this field
    /// existed. Written with the first insert and never changed: it belongs
    /// to the immutable request binding, outside the request fingerprint
    /// and the lifecycle machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convener: Option<TemporaryCouncilConvener>,
    /// Optimistic concurrency token.
    pub revision: u64,
    /// Creation instant.
    pub created_at: DateTime<Utc>,
    /// Last mutation instant.
    pub updated_at: DateTime<Utc>,
}

/// The machine's own verdict about what a recovery sweep still owes a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TemporaryCouncilRecoveryVerdict {
    /// Whether a recovery sweep still owes this record work.
    pub unfinished: bool,
    /// Whether an immutable result is already sealed.
    pub result_sealed: bool,
    /// Whether cleanup still needs to run.
    pub needs_cleanup: bool,
}

impl TemporaryCouncilRecord {
    /// Ask the canonical machine what this record still owes.
    ///
    /// This is the ONLY phase predicate in the system: no caller re-derives
    /// "unfinished" from the persisted state, and no backend matches on a
    /// phase enum of its own.
    pub fn recovery_verdict(
        &self,
    ) -> Result<TemporaryCouncilRecoveryVerdict, TemporaryCouncilLifecycleMachineTransitionError>
    {
        let mut authority = TemporaryCouncilLifecycleMachineAuthority::recover_from_state(
            self.machine_state.clone(),
        )?;
        let transition = TemporaryCouncilLifecycleMachineMutator::apply(
            &mut authority,
            TemporaryCouncilLifecycleInput::ClassifyRecovery {},
        )?;
        for effect in transition.effects() {
            if let TemporaryCouncilLifecycleEffect::RecoveryClassified {
                unfinished,
                result_sealed,
                needs_cleanup,
            } = effect
            {
                return Ok(TemporaryCouncilRecoveryVerdict {
                    unfinished: *unfinished,
                    result_sealed: *result_sealed,
                    needs_cleanup: *needs_cleanup,
                });
            }
        }
        Err(
            TemporaryCouncilLifecycleMachineTransitionError::NoMatchingTransition {
                phase: self.machine_state.lifecycle_phase,
                trigger: TemporaryCouncilLifecycleMachineTransitionTrigger::Input(
                    TemporaryCouncilLifecycleInputVariant::ClassifyRecovery,
                ),
            },
        )
    }

    /// Whether a recovery sweep still owes this record work.
    ///
    /// A record whose persisted machine state cannot be recovered is reported
    /// as unfinished: refusing to see it would strand it forever.
    #[must_use]
    pub fn is_unfinished(&self) -> bool {
        self.recovery_verdict()
            .map_or(true, |verdict| verdict.unfinished)
    }

    /// Custody for one participant slot.
    #[must_use]
    pub fn participant(&self, order: u32) -> Option<&TemporaryCouncilParticipantCustody> {
        self.participants
            .iter()
            .find(|participant| participant.order == order)
    }
}

/// Durable store for temporary-council orchestration custody.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait TemporaryCouncilStore: Send + Sync {
    /// What this backend actually guarantees across a process restart.
    ///
    /// Deliberately has no default: every backend states its own durability,
    /// so a council can never be told its custody is crash-recoverable by a
    /// store that quietly forgot to say otherwise.
    fn durability(&self) -> TemporaryCouncilStoreDurability;

    /// Insert a freshly prepared council record.
    ///
    /// Fails with [`MobStoreError::CasConflict`] when the council id is
    /// already taken. Callers resolve replay-versus-conflict by loading the
    /// existing record and comparing its fingerprint; the store never decides.
    async fn insert_new(
        &self,
        record: &TemporaryCouncilRecord,
    ) -> Result<TemporaryCouncilRecord, MobStoreError>;

    /// Load one record by council identity.
    async fn load(
        &self,
        council_id: &TemporaryCouncilId,
    ) -> Result<Option<TemporaryCouncilRecord>, MobStoreError>;

    /// Compare-and-swap one record forward.
    ///
    /// `record.revision` is the revision the caller read; the stored row must
    /// still be at that revision or the write fails with
    /// [`MobStoreError::CasConflict`]. The returned record carries the next
    /// revision.
    async fn commit(
        &self,
        record: &TemporaryCouncilRecord,
    ) -> Result<TemporaryCouncilRecord, MobStoreError>;

    /// List every record, oldest first.
    async fn list_all(&self) -> Result<Vec<TemporaryCouncilRecord>, MobStoreError>;

    /// List records a recovery sweep still owes work, oldest first.
    ///
    /// Backends may override this with an indexed query; the default derives
    /// it from [`Self::list_all`] so every backend agrees on the predicate.
    async fn list_unfinished(&self) -> Result<Vec<TemporaryCouncilRecord>, MobStoreError> {
        Ok(self
            .list_all()
            .await?
            .into_iter()
            .filter(TemporaryCouncilRecord::is_unfinished)
            .collect())
    }
}

#[cfg(test)]
pub(crate) mod contract_tests {
    use super::*;
    use crate::forked_participant::ForkedParticipantOperationScope;
    use crate::ids::AgentIdentity;
    use crate::ids::ProfileName;
    use crate::machines::temporary_council_lifecycle::TemporaryCouncilLifecycleState;
    use crate::temporary_council::{
        TemporaryCouncilCleanupDebt, TemporaryCouncilExchangeOutcome, TemporaryCouncilExitReason,
        TemporaryCouncilMergeOutcome, TemporaryCouncilParticipantProvenance,
    };
    use meerkat_core::SessionId;

    /// Advance a fresh authority through the given inputs and return its state.
    pub(crate) fn machine_state(
        inputs: &[TemporaryCouncilLifecycleInput],
    ) -> TemporaryCouncilLifecycleMachineState {
        let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
        for input in inputs {
            TemporaryCouncilLifecycleMachineMutator::apply(&mut authority, input.clone())
                .expect("canonical council transition");
        }
        authority.state().clone()
    }

    pub(crate) fn record(council: &str) -> TemporaryCouncilRecord {
        let now = Utc::now();
        let council_id = TemporaryCouncilId::new(council).expect("council id");
        TemporaryCouncilRecord {
            request_fingerprint: format!("tcf1:sha256:{council}"),
            temporary_mob_id: council_id.temporary_mob_id(),
            temporary_mob_custody: Some(TemporaryCouncilMobCustody::NotCreated),
            deadline: now + chrono::Duration::seconds(600),
            durability: TemporaryCouncilDurability::ProcessBound,
            claim_lease_expires_at: now + chrono::Duration::seconds(60),
            machine_state: machine_state(&[TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: format!("tcf1:sha256:{council}"),
            }]),
            participants: vec![TemporaryCouncilParticipantCustody {
                order: 0,
                role: "analyst".to_string(),
                source_mob_id: MobId::from("source-mob"),
                source_identity: AgentIdentity::from("researcher"),
                target_identity: AgentIdentity::from("analyst"),
                target_profile: ProfileName::from("participant"),
                scope: ForkedParticipantOperationScope::InvokeAndObserve,
                capability_request_id: council_id
                    .capability_request_id(0)
                    .expect("capability request id"),
                capability_correlation_hint: None,
                capability_ref: None,
                attachment_id: council_id.attachment_id(0).expect("attachment id"),
                acquisition: crate::temporary_council::TemporaryCouncilAcquisition::NotAttempted,
                seated: false,
                seated_session_id: None,
            }],
            exchanges: Vec::new(),
            result: None,
            cleanup: None,
            detached_job: None,
            convener: None,
            revision: 0,
            created_at: now,
            updated_at: now,
            council_id,
        }
    }

    fn sealed_result(record: &TemporaryCouncilRecord) -> TemporaryCouncilResult {
        TemporaryCouncilResult {
            council_id: record.council_id.clone(),
            request_fingerprint: record.request_fingerprint.clone(),
            temporary_mob_id: record.temporary_mob_id.clone(),
            exit_reason: TemporaryCouncilExitReason::Completed,
            rounds_completed: 1,
            exchanges: record.exchanges.clone(),
            merge: TemporaryCouncilMergeOutcome::NoMerge {
                confirmed_participants: vec![AgentIdentity::from("analyst")],
            },
            participants: vec![TemporaryCouncilParticipantProvenance {
                order: 0,
                role: "analyst".to_string(),
                source_mob_id: MobId::from("source-mob"),
                source_identity: AgentIdentity::from("researcher"),
                target_identity: AgentIdentity::from("analyst"),
                scope: ForkedParticipantOperationScope::InvokeAndObserve,
                capability_request_id: record
                    .council_id
                    .capability_request_id(0)
                    .expect("capability request id"),
                capability: None,
                attachment_id: record.council_id.attachment_id(0).expect("attachment id"),
                seated: true,
            }],
            truncated_exchange_count: 0,
            merge_truncated: false,
            durability: TemporaryCouncilDurability::ProcessBound,
            concluded_at: Utc::now(),
        }
    }

    pub(crate) async fn insert_and_load<S: TemporaryCouncilStore>(store: &S) {
        let record = record("insert");
        let stored = store.insert_new(&record).await.expect("insert");
        assert_eq!(stored.revision, 1, "a fresh insert starts at revision 1");

        let loaded = store
            .load(&record.council_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(loaded, stored);
        assert!(
            store
                .load(&TemporaryCouncilId::new("absent").expect("id"))
                .await
                .expect("load")
                .is_none()
        );
    }

    pub(crate) async fn duplicate_council_id_loses<S: TemporaryCouncilStore>(store: &S) {
        let record = record("duplicate");
        store.insert_new(&record).await.expect("first insert");
        let mut conflicting = record.clone();
        conflicting.request_fingerprint = "tcf1:sha256:other".to_string();
        assert!(
            matches!(
                store.insert_new(&conflicting).await,
                Err(MobStoreError::CasConflict(_))
            ),
            "a second insert under the same council id must lose"
        );
        let loaded = store
            .load(&record.council_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(
            loaded.request_fingerprint, record.request_fingerprint,
            "the losing insert must not overwrite the bound fingerprint"
        );
    }

    pub(crate) async fn commit_is_compare_and_swap<S: TemporaryCouncilStore>(store: &S) {
        let record = record("cas");
        let stored = store.insert_new(&record).await.expect("insert");

        let mut advanced = stored.clone();
        advanced.machine_state = machine_state(&[
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: "tcf1:sha256:cas".to_string(),
            },
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: "contract-claim".to_string(),
                lease_expired: false,
            },
            TemporaryCouncilLifecycleInput::StartDiscussion {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
        ]);
        advanced.exchanges.push(TemporaryCouncilExchangeReceipt {
            round: 0,
            sequence: 0,
            participant_order: 0,
            target_identity: AgentIdentity::from("analyst"),
            delivery_idempotency_key: "council:cas:round:r0:p0".to_string(),
            delivery_correlation_id: uuid::Uuid::new_v4().to_string(),
            started_at: Utc::now(),
            outcome: TemporaryCouncilExchangeOutcome::Completed {
                text: "hello".to_string(),
                truncated: false,
                session_id: SessionId::new(),
                completed_at: Utc::now(),
            },
        });
        let committed = store.commit(&advanced).await.expect("commit");
        assert_eq!(committed.revision, stored.revision + 1);
        assert_eq!(committed.exchanges.len(), 1);

        assert!(
            matches!(
                store.commit(&advanced).await,
                Err(MobStoreError::CasConflict(_))
            ),
            "a stale revision must lose"
        );
    }

    pub(crate) async fn unfinished_listing_tracks_settlement<S: TemporaryCouncilStore>(store: &S) {
        let running = store.insert_new(&record("running")).await.expect("insert");
        let settling = store.insert_new(&record("settling")).await.expect("insert");

        let unfinished = store.list_unfinished().await.expect("list unfinished");
        assert_eq!(unfinished.len(), 2);

        let settled_state = machine_state(&[
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: "tcf1:sha256:settling".to_string(),
            },
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: "contract-claim".to_string(),
                lease_expired: false,
            },
            TemporaryCouncilLifecycleInput::StartDiscussion {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::StartMerge {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::SealResult {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::RecordCleanupSettled {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
        ]);
        let mut settled = settling.clone();
        settled.result = Some(sealed_result(&settling));
        settled.cleanup = Some(TemporaryCouncilCleanupReceipt {
            attempted_at: Utc::now(),
            attempts: 1,
            temporary_mob_destroyed: true,
            released_participants: vec![0],
            revoked_participants: Vec::new(),
            debts: Vec::new(),
            budget_exhausted: false,
        });
        settled.machine_state = settled_state;
        assert_eq!(
            settled.machine_state.lifecycle_phase,
            TemporaryCouncilLifecycleState::Settled
        );
        store.commit(&settled).await.expect("commit settled");

        let unfinished = store.list_unfinished().await.expect("list unfinished");
        assert_eq!(unfinished.len(), 1);
        assert_eq!(unfinished[0].council_id, running.council_id);

        let mut indebted = store
            .load(&settling.council_id)
            .await
            .expect("load")
            .expect("present");
        indebted.machine_state = machine_state(&[
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: "tcf1:sha256:settling".to_string(),
            },
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: "contract-claim".to_string(),
                lease_expired: false,
            },
            TemporaryCouncilLifecycleInput::StartDiscussion {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::StartMerge {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::SealResult {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::RecordCleanupDebt {
                claim_id: "contract-claim".to_string(),
                claim_epoch: 1,
            },
        ]);
        indebted.cleanup = Some(TemporaryCouncilCleanupReceipt {
            attempted_at: Utc::now(),
            attempts: 2,
            temporary_mob_destroyed: false,
            released_participants: Vec::new(),
            revoked_participants: Vec::new(),
            debts: vec![TemporaryCouncilCleanupDebt {
                subject: "mob:council--settling".to_string(),
                detail: "destroy failed".to_string(),
            }],
            budget_exhausted: false,
        });
        store.commit(&indebted).await.expect("commit debt");
        assert_eq!(
            store
                .list_unfinished()
                .await
                .expect("list unfinished")
                .len(),
            2,
            "retained cleanup debt keeps the record in the recovery sweep"
        );
        assert_eq!(store.list_all().await.expect("list all").len(), 2);
    }

    /// A store keeps the convener a record was inserted with through its
    /// insert, a load and a later commit.
    pub(crate) async fn convener_survives_insert_load_and_commit<S: TemporaryCouncilStore>(
        store: &S,
    ) {
        let convener = TemporaryCouncilConvener::new(SessionId::new());
        let mut convened = record("convened");
        convened.convener = Some(convener.clone());
        let stored = store.insert_new(&convened).await.expect("insert");
        assert_eq!(stored.convener.as_ref(), Some(&convener));
        let loaded = store
            .load(&convened.council_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(loaded.convener.as_ref(), Some(&convener));

        let mut claimed = loaded;
        claimed.machine_state = machine_state(&[
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: "tcf1:sha256:convened".to_string(),
            },
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: "contract-claim".to_string(),
                lease_expired: false,
            },
        ]);
        let committed = store.commit(&claimed).await.expect("commit");
        assert_eq!(committed.convener.as_ref(), Some(&convener));
        let reloaded = store
            .load(&convened.council_id)
            .await
            .expect("load")
            .expect("present");
        assert_eq!(reloaded.convener, Some(convener));

        let hosted = store.insert_new(&record("hosted")).await.expect("insert");
        assert_eq!(
            hosted.convener, None,
            "a record inserted without a convener has none"
        );
    }
}

/// The stored form of the convener, and what a reader from before the field
/// existed does with it.
#[cfg(test)]
mod convener_stored_form_tests {
    use super::contract_tests::record;
    use super::*;
    use meerkat_core::SessionId;
    use serde_json::{Value, json};

    /// The record as 0.9.14 reads and writes it: the same fields in the same
    /// order, without `temporary_mob_custody` and without `convener`, and
    /// like the record itself tolerant of keys it does not know.
    #[derive(Debug, Serialize, Deserialize)]
    struct RecordOfAnEarlierRelease {
        council_id: TemporaryCouncilId,
        request_fingerprint: String,
        temporary_mob_id: MobId,
        deadline: DateTime<Utc>,
        durability: TemporaryCouncilDurability,
        claim_lease_expires_at: DateTime<Utc>,
        machine_state: TemporaryCouncilLifecycleMachineState,
        participants: Vec<TemporaryCouncilParticipantCustody>,
        exchanges: Vec<TemporaryCouncilExchangeReceipt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<TemporaryCouncilResult>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cleanup: Option<TemporaryCouncilCleanupReceipt>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detached_job: Option<TemporaryCouncilJobBinding>,
        revision: u64,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    }

    fn convened_record(label: &str) -> (TemporaryCouncilRecord, SessionId) {
        let session = SessionId::new();
        let mut record = record(label);
        record.convener = Some(TemporaryCouncilConvener::new(session.clone()));
        record.detached_job = Some(TemporaryCouncilJobBinding::new(
            "job-of-the-convener",
            session.clone(),
        ));
        (record, session)
    }

    /// The convener is one optional key, `convener`, holding the session id.
    /// A record without a convener writes no such key.
    #[test]
    fn the_convener_is_one_optional_key_of_the_stored_record() {
        let (convened, session) = convened_record("stored-form");
        let stored = serde_json::to_value(&convened).expect("encode");
        assert_eq!(stored["convener"], json!({ "session_id": session }));
        let decoded: TemporaryCouncilRecord = serde_json::from_value(stored).expect("decode");
        assert_eq!(decoded, convened);

        let hosted = record("stored-form-hosted");
        let stored = serde_json::to_value(&hosted).expect("encode");
        assert!(
            stored.get("convener").is_none(),
            "a record with no convener writes no key: {stored}"
        );
    }

    /// A row an earlier release wrote has neither new key and decodes as a
    /// council with no convener that names no mob of its own.
    #[test]
    fn a_record_stored_without_the_new_keys_decodes_with_no_convener_and_no_custody() {
        let (convened, _) = convened_record("older-row");
        let older = RecordOfAnEarlierRelease {
            council_id: convened.council_id.clone(),
            request_fingerprint: convened.request_fingerprint.clone(),
            temporary_mob_id: convened.temporary_mob_id.clone(),
            deadline: convened.deadline,
            durability: convened.durability,
            claim_lease_expires_at: convened.claim_lease_expires_at,
            machine_state: convened.machine_state.clone(),
            participants: convened.participants.clone(),
            exchanges: convened.exchanges.clone(),
            result: convened.result.clone(),
            cleanup: convened.cleanup.clone(),
            detached_job: convened.detached_job.clone(),
            revision: convened.revision,
            created_at: convened.created_at,
            updated_at: convened.updated_at,
        };
        let row = serde_json::to_vec(&older).expect("encode the older row");
        let stored = serde_json::from_slice::<Value>(&row).expect("json");
        assert!(stored.get("convener").is_none());
        assert!(stored.get("temporary_mob_custody").is_none());
        let decoded: TemporaryCouncilRecord = serde_json::from_slice(&row).expect("decode");
        let mut expected = convened;
        expected.convener = None;
        expected.temporary_mob_custody = None;
        assert_eq!(decoded, expected);
    }

    /// The record refuses no key it does not know: a row that carries one
    /// still decodes, with everything it does know intact.
    #[test]
    fn a_record_stored_with_an_unknown_key_decodes() {
        let (convened, _) = convened_record("unknown-key");
        let mut stored = serde_json::to_value(&convened).expect("encode");
        stored["a_key_of_a_later_release"] = json!({ "anything": [1, 2, 3] });
        let decoded: TemporaryCouncilRecord = serde_json::from_value(stored).expect("decode");
        assert_eq!(decoded, convened);
    }

    /// A reader of an earlier release reads a row that carries both new
    /// keys, and when it writes the record back it drops `convener` and
    /// `temporary_mob_custody` and nothing else. A current reader then finds
    /// a council with no convener that names no mob of its own.
    #[test]
    fn an_older_reader_drops_the_convener_and_custody_keys_and_nothing_else() {
        let (convened, _) = convened_record("older-reader");
        let stored = serde_json::to_value(&convened).expect("encode");
        assert!(stored.get("convener").is_some());
        assert_eq!(
            stored["temporary_mob_custody"],
            json!({ "state": "not_created" })
        );

        let older: RecordOfAnEarlierRelease =
            serde_json::from_value(stored.clone()).expect("an older reader decodes the row");
        let rewritten = serde_json::to_value(&older).expect("an older reader writes it back");

        let mut expected = stored;
        for key in ["convener", "temporary_mob_custody"] {
            expected
                .as_object_mut()
                .expect("the record is an object")
                .remove(key)
                .expect("the key was there");
        }
        assert_eq!(rewritten, expected);

        let reread: TemporaryCouncilRecord = serde_json::from_value(rewritten).expect("decode");
        let mut without = convened;
        without.convener = None;
        without.temporary_mob_custody = None;
        assert_eq!(reread, without);
    }
}
