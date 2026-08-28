//! Durable store contract for temporary-council orchestration custody.
//!
//! One row per council. The row carries four separable things:
//!
//! * the **immutable request binding** — the council id, the canonical request
//!   fingerprint, and the deterministic temporary [`MobId`];
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
    TemporaryCouncilCleanupReceipt, TemporaryCouncilDurability, TemporaryCouncilExchangeReceipt,
    TemporaryCouncilId, TemporaryCouncilParticipantCustody, TemporaryCouncilResult,
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
}
