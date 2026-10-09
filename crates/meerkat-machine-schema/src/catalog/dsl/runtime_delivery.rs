use meerkat_machine_dsl::machine;

use super::OptionValueExt;

/// Why a committed delivery was settled as refused: a terminal policy
/// outcome, never an infrastructure failure, an unavailable owner or a store
/// error (those stay pending and retry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DeliveryRefusalReason {
    /// The delivery's original native work binding is missing or invalid, and
    /// immutably so: it can never be admitted on a governed runtime.
    #[default]
    NoAdmissibleWorkBinding,
    /// The native work authorization owner's current verdict on the
    /// delivery's original work is an actual denial.
    AuthorityDenied,
    /// The native work authorization owner actually reported the operation
    /// authorization as unavailable for this delivery (a settled verdict,
    /// not an observation or store failure).
    OperationAuthorizationUnavailable,
}

/// A locally settled recipient. Infrastructure and unknown effects have no
/// settlement variant and remain pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DeliveryRecipientOutcome {
    #[default]
    Applied,
    Refused,
    OperationAuthorizationUnavailable,
}

/// Exact summary of all recipients after the group is completely settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum DeliveryRecipientGroupOutcome {
    #[default]
    AllApplied,
    AllRefused,
    AllAuthorizationUnavailable,
    Mixed,
}

machine! {
    machine RuntimeDeliveryMachine {
        version: 1,
        rust: "self" / "catalog::dsl::runtime_delivery",

        state {
            lifecycle_phase: RuntimeDeliveryPhase,
            delivery_ids: Set<String>,
            delivery_sequences: Map<String, u64>,
            delivery_source_sequences: Map<String, u64>,
            committed_sequences: Set<u64>,
            next_sequence: u64,
            applied_cursor: u64,
            // Committed sequences ahead of the cursor whose effect already
            // reached their runtime out of band (for example the shell's own
            // completion projection). The cursor still advances strictly in
            // order; an acknowledged row is consumed when the cursor reaches
            // it, so an out-of-order acknowledgement never wedges the queue.
            acknowledged_sequences: Set<u64>,
            // Deliveries settled as refused at the cursor: a terminal policy
            // outcome. The cursor passed them, so later rows proceed, and
            // they are never applied.
            refused_deliveries: Map<String, DeliveryRefusalReason>,
            // Bound once from the producer owner's complete committed payload.
            // Both maps are scoped by delivery, so subscriptions can repeat
            // across deliveries without sharing settlement state.
            delivery_recipient_bindings: Map<String, Map<String, String>>,
            recipient_outcomes: Map<String, Map<String, Enum<DeliveryRecipientOutcome>>>,
            recipient_group_outcomes: Map<String, Enum<DeliveryRecipientGroupOutcome>>,
        }

        init(Active) {
            delivery_ids = EmptySet,
            delivery_sequences = EmptyMap,
            delivery_source_sequences = EmptyMap,
            committed_sequences = EmptySet,
            next_sequence = 0,
            applied_cursor = 0,
            acknowledged_sequences = EmptySet,
            refused_deliveries = EmptyMap,
            delivery_recipient_bindings = EmptyMap,
            recipient_outcomes = EmptyMap,
            recipient_group_outcomes = EmptyMap,
        }

        terminal []

        phase RuntimeDeliveryPhase {
            Active,
        }

        input RuntimeDeliveryInput {
            BindDeliveryRecipients {
                delivery_id: String,
                delivery_sequence: u64,
                recipients: Map<String, String>,
            },
            SettleDeliveryRecipient {
                delivery_id: String,
                delivery_sequence: u64,
                recipient_id: String,
                target_binding: String,
                outcome: Enum<DeliveryRecipientOutcome>,
            },
            FinishDeliveryRecipients {
                delivery_id: String,
                delivery_sequence: u64,
            },
            CommitDelivery {
                delivery_id: String,
                source_sequence: u64,
            },
            MarkDeliveryApplied {
                delivery_id: String,
                delivery_sequence: u64,
            },
            // The delivery's effect reached its runtime by another path. At
            // the cursor this applies it; ahead of the cursor it is recorded
            // and consumed later by AdvanceAcknowledgedPrefix.
            AcknowledgeDelivery {
                delivery_id: String,
                delivery_sequence: u64,
            },
            // Advance the cursor over the next sequence when it was already
            // acknowledged. Total in Active: when nothing is parked at the
            // cursor it is a typed no-op, so the shell drives it until the
            // machine reports the prefix at rest and never pre-checks the
            // guard itself.
            AdvanceAcknowledgedPrefix {},
            // Settle the delivery at the cursor as refused for `reason`: the
            // cursor passes it without applying it. Repeating the settlement
            // of a refused delivery observes it.
            SettleRefusedDelivery {
                delivery_id: String,
                delivery_sequence: u64,
                reason: Enum<DeliveryRefusalReason>,
            },
            // Read-only: classify one delivery id against this authority.
            // Exactly one verdict arm holds for every id in Active.
            ClassifyDeliveryStatus {
                delivery_id: String,
            },
        }

        effect RuntimeDeliveryEffect {
            DeliveryRecipientsBound {
                delivery_id: String,
                delivery_sequence: u64,
            },
            DeliveryRecipientSettled {
                delivery_id: String,
                delivery_sequence: u64,
                recipient_id: String,
                outcome: Enum<DeliveryRecipientOutcome>,
            },
            DeliveryRecipientsSettled {
                delivery_id: String,
                delivery_sequence: u64,
                outcome: Enum<DeliveryRecipientGroupOutcome>,
            },
            DeliveryStatusMixed {
                delivery_id: String,
                delivery_sequence: u64,
                bindings: Map<String, String>,
                outcomes: Map<String, Enum<DeliveryRecipientOutcome>>,
            },
            DeliveryCommitted {
                delivery_id: String,
                source_sequence: u64,
                delivery_sequence: u64,
            },
            DeliveryReused {
                delivery_id: String,
                source_sequence: u64,
                delivery_sequence: u64,
            },
            DeliveryApplied {
                delivery_id: String,
                delivery_sequence: u64,
            },
            DeliveryAcknowledged {
                delivery_id: String,
                delivery_sequence: u64,
            },
            AcknowledgedPrefixAdvanced {
                delivery_sequence: u64,
            },
            AcknowledgedPrefixAtRest {
                applied_cursor: u64,
            },
            DeliveryRefused {
                delivery_id: String,
                delivery_sequence: u64,
                reason: Enum<DeliveryRefusalReason>,
            },
            // The same delivery id was committed with another source
            // sequence; the existing row stands.
            CommitRejectedSourceSequenceConflict {
                delivery_id: String,
                source_sequence: u64,
                committed_source_sequence: u64,
            },
            DeliveryStatusNotCommitted {
                delivery_id: String,
            },
            DeliveryStatusApplied {
                delivery_id: String,
                delivery_sequence: u64,
            },
            // Committed ahead of the cursor and acknowledged out of band: its
            // effect reached the runtime by another path while an earlier row
            // is still pending.
            DeliveryStatusAcknowledgedAhead {
                delivery_id: String,
                delivery_sequence: u64,
            },
            DeliveryStatusPending {
                delivery_id: String,
                delivery_sequence: u64,
            },
            // Settled as refused: the cursor passed it and it was never
            // applied.
            DeliveryStatusRefused {
                delivery_id: String,
                delivery_sequence: u64,
                reason: Enum<DeliveryRefusalReason>,
            },
        }

        // Every guard reads delivery_sequences / delivery_source_sequences
        // behind delivery_ids.contains(id); this pins the key sets those
        // strict reads rely on (#1811).
        invariant delivery_maps_cover_exactly_the_committed_ids {
            self.delivery_sequences.keys() == self.delivery_ids
                && self.delivery_source_sequences.keys() == self.delivery_ids
        }

        invariant applied_cursor_does_not_pass_committed_sequence {
            self.applied_cursor <= self.next_sequence
        }

        invariant empty_delivery_set_has_zero_sequence {
            self.delivery_ids.len() != 0 || self.next_sequence == 0
        }

        invariant delivery_identity_and_sequence_cardinality_match {
            self.delivery_ids.len() == self.committed_sequences.len()
        }

        invariant committed_sequence_cardinality_tracks_high_water {
            self.committed_sequences.len() == self.next_sequence
        }

        invariant applied_cursor_is_never_acknowledged_pending {
            self.acknowledged_sequences.contains(self.applied_cursor) == false
        }

        invariant acknowledged_sequences_are_ahead_of_the_cursor {
            for_all(sequence in self.acknowledged_sequences,
                sequence > self.applied_cursor && sequence <= self.next_sequence)
        }

        invariant acknowledged_sequences_are_committed {
            for_all(sequence in self.acknowledged_sequences,
                self.committed_sequences.contains(sequence))
        }

        invariant refused_deliveries_are_committed_and_passed {
            for_all(id in self.refused_deliveries.keys(),
                self.delivery_ids.contains(id)
                    && self.delivery_sequences.get_cloned(id).get("value") <= self.applied_cursor)
        }

        invariant recipient_bindings_belong_to_committed_deliveries {
            for_all(id in self.delivery_recipient_bindings.keys(),
                self.delivery_ids.contains(id)
                    && self.acknowledged_sequences.contains(self.delivery_sequences.get_cloned(id).get("value")) == false)
        }

        invariant enrolled_row_refusal_matches_its_exact_group {
            for_all(id in self.delivery_recipient_bindings.keys(),
                if self.recipient_group_outcomes.get_cloned(id) == Some(DeliveryRecipientGroupOutcome::AllRefused) {
                    self.refused_deliveries.get_cloned(id) == Some(DeliveryRefusalReason::AuthorityDenied)
                } else {
                    if self.recipient_group_outcomes.get_cloned(id) == Some(DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable) {
                        self.refused_deliveries.get_cloned(id) == Some(DeliveryRefusalReason::OperationAuthorizationUnavailable)
                    } else {
                        self.refused_deliveries.contains_key(id) == false
                    }
                })
        }

        invariant recipient_settlements_have_bound_targets {
            for_all(id in self.recipient_outcomes.keys(),
                self.delivery_recipient_bindings.contains_key(id)
                    && for_all(recipient in self.recipient_outcomes.get_cloned(id).get("value").keys(),
                        self.delivery_recipient_bindings.get_cloned(id).get("value").contains_key(recipient)))
        }

        invariant recipient_bindings_retain_their_outcome_map {
            for_all(id in self.delivery_recipient_bindings.keys(),
                self.recipient_outcomes.contains_key(id))
        }

        invariant recipient_groups_pass_only_after_every_recipient_settles {
            for_all(id in self.delivery_recipient_bindings.keys(),
                (self.delivery_sequences.get_cloned(id).get("value") <= self.applied_cursor)
                    == self.recipient_group_outcomes.contains_key(id))
        }

        invariant recipient_group_outcomes_have_complete_bindings {
            for_all(id in self.recipient_group_outcomes.keys(),
                self.delivery_recipient_bindings.contains_key(id)
                    && for_all(recipient in self.delivery_recipient_bindings.get_cloned(id).get("value").keys(),
                        self.recipient_outcomes.get_cloned(id).get("value").contains_key(recipient)))
        }

        invariant recipient_group_summary_matches_each_retained_outcome {
            for_all(id in self.recipient_group_outcomes.keys(),
                self.recipient_group_outcomes.get_cloned(id).get("value") ==
                    if for_all(recipient in self.delivery_recipient_bindings.get_cloned(id).get("value").keys(),
                        self.recipient_outcomes.get_cloned(id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::Applied)) {
                        DeliveryRecipientGroupOutcome::AllApplied
                    } else {
                        if for_all(recipient in self.delivery_recipient_bindings.get_cloned(id).get("value").keys(),
                            self.recipient_outcomes.get_cloned(id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::Refused)) {
                            DeliveryRecipientGroupOutcome::AllRefused
                        } else {
                            if for_all(recipient in self.delivery_recipient_bindings.get_cloned(id).get("value").keys(),
                                self.recipient_outcomes.get_cloned(id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::OperationAuthorizationUnavailable)) {
                                DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable
                            } else { DeliveryRecipientGroupOutcome::Mixed }
                        }
                    })
        }

        disposition DeliveryRecipientsBound => local seam OwnerRealizationOnly,
        disposition DeliveryRecipientSettled => local seam OwnerRealizationOnly,
        disposition DeliveryRecipientsSettled => local seam OwnerRealizationOnly,
        disposition DeliveryStatusMixed => local seam SurfaceResultAlignment,

        disposition DeliveryCommitted => routed [DetachedJobMachine] seam NoOwnerRealization,
        disposition DeliveryReused => routed [DetachedJobMachine] seam NoOwnerRealization,
        disposition DeliveryApplied => local seam OwnerRealizationOnly,
        disposition DeliveryAcknowledged => local seam OwnerRealizationOnly,
        disposition AcknowledgedPrefixAdvanced => local seam OwnerRealizationOnly,
        disposition AcknowledgedPrefixAtRest => local seam OwnerRealizationOnly,
        disposition DeliveryRefused => local seam OwnerRealizationOnly,
        disposition CommitRejectedSourceSequenceConflict => local seam SurfaceResultAlignment,
        disposition DeliveryStatusNotCommitted => local seam SurfaceResultAlignment,
        disposition DeliveryStatusApplied => local seam SurfaceResultAlignment,
        disposition DeliveryStatusAcknowledgedAhead => local seam SurfaceResultAlignment,
        disposition DeliveryStatusPending => local seam SurfaceResultAlignment,
        disposition DeliveryStatusRefused => local seam SurfaceResultAlignment,

        transition CommitNewDelivery {
            on input CommitDelivery { delivery_id, source_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id) == false
                    && source_sequence > 0
                    && self.next_sequence < u64::MAX
            }
            update {
                self.next_sequence += 1;
                self.delivery_ids.insert(delivery_id);
                self.delivery_sequences.insert(delivery_id, self.next_sequence);
                self.delivery_source_sequences.insert(delivery_id, source_sequence);
                self.committed_sequences.insert(self.next_sequence);
            }
            to Active
            emit DeliveryCommitted {
                delivery_id: delivery_id,
                source_sequence: source_sequence,
                delivery_sequence: self.next_sequence
            }
        }

        transition ReuseCommittedDelivery {
            on input CommitDelivery { delivery_id, source_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_source_sequences.get_cloned(delivery_id).get("value") == source_sequence
            }
            update {}
            to Active
            emit DeliveryReused {
                delivery_id: delivery_id,
                source_sequence: source_sequence,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value")
            }
        }

        transition RejectSourceSequenceConflict {
            on input CommitDelivery { delivery_id, source_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_source_sequences.get_cloned(delivery_id).get("value") != source_sequence
            }
            update {}
            to Active
            emit CommitRejectedSourceSequenceConflict {
                delivery_id: delivery_id,
                source_sequence: source_sequence,
                committed_source_sequence: self.delivery_source_sequences.get_cloned(delivery_id).get("value")
            }
        }

        transition ApplyNextDelivery {
            on input MarkDeliveryApplied { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
            }
            update {
                self.applied_cursor = delivery_sequence;
                self.acknowledged_sequences.remove(delivery_sequence);
            }
            to Active
            emit DeliveryApplied {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }

        transition ObserveAlreadyAppliedDelivery {
            on input MarkDeliveryApplied { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence <= self.applied_cursor
                    && self.refused_deliveries.contains_key(delivery_id) == false
            }
            update {}
            to Active
            emit DeliveryApplied {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }

        transition AcknowledgeNextDelivery {
            on input AcknowledgeDelivery { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
            }
            update {
                self.applied_cursor = delivery_sequence;
                self.acknowledged_sequences.remove(delivery_sequence);
            }
            to Active
            emit DeliveryApplied {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }

        transition AcknowledgeAheadOfCursor {
            on input AcknowledgeDelivery { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 > self.applied_cursor
            }
            update {
                self.acknowledged_sequences.insert(delivery_sequence);
            }
            to Active
            emit DeliveryAcknowledged {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }

        transition ObserveAlreadyAppliedAcknowledgement {
            on input AcknowledgeDelivery { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence <= self.applied_cursor
                    && self.refused_deliveries.contains_key(delivery_id) == false
            }
            update {}
            to Active
            emit DeliveryApplied {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }

        transition AdvanceOverAcknowledgedDelivery {
            on input AdvanceAcknowledgedPrefix {}
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.applied_cursor < self.next_sequence
                    && self.acknowledged_sequences.contains(self.applied_cursor + 1)
            }
            update {
                self.acknowledged_sequences.remove(self.applied_cursor + 1);
                self.applied_cursor += 1;
            }
            to Active
            emit AcknowledgedPrefixAdvanced {
                delivery_sequence: self.applied_cursor
            }
        }

        // The exact complement of AdvanceOverAcknowledgedDelivery in Active:
        // no acknowledged delivery is parked at the cursor, so the prefix is
        // at rest and nothing changes.
        transition AdvanceAcknowledgedPrefixNothingParked {
            on input AdvanceAcknowledgedPrefix {}
            guard {
                self.lifecycle_phase == Phase::Active
                    && (self.applied_cursor >= self.next_sequence
                        || !self.acknowledged_sequences.contains(self.applied_cursor + 1))
            }
            update {}
            to Active
            emit AcknowledgedPrefixAtRest {
                applied_cursor: self.applied_cursor
            }
        }
        // Refusal settles only the row at the cursor, and never a row whose
        // effect already reached its runtime out of band.
        transition SettleRefusedDeliveryAtCursor {
            on input SettleRefusedDelivery { delivery_id, delivery_sequence, reason }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
                    && self.acknowledged_sequences.contains(delivery_sequence) == false
            }
            update {
                self.applied_cursor = delivery_sequence;
                self.refused_deliveries.insert(delivery_id, reason);
            }
            to Active
            emit DeliveryRefused {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence,
                reason: reason
            }
        }

        // Membership first: a missing key's lookup falls back to the enum
        // default, so equality alone would hold for any unrefused row.
        transition ObserveAlreadyRefusedDelivery {
            on input SettleRefusedDelivery { delivery_id, delivery_sequence, reason }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && self.refused_deliveries.contains_key(delivery_id)
                    && self.refused_deliveries.get_cloned(delivery_id).get("value") == reason
            }
            update {}
            to Active
            emit DeliveryRefused {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence,
                reason: reason
            }
        }

        // ClassifyDeliveryStatus partitions Active by these facts:
        //   committed(id)      = delivery_ids.contains(id)
        //   refused(id)        = refused_deliveries.contains_key(id)
        //   cursor_applied(id) = committed(id) && seq(id) <= applied_cursor
        //   acknowledged(id) and the optional exact recipient-group summary.
        // Unanimous groups use the existing applied/refused verdicts. Mixed
        // groups expose only their own exact bindings and outcomes.
        // The guards partition even inconsistent assignments of these facts:
        // pairwise disjoint and jointly total (pinned by a schema contract
        // test).
        transition ClassifyNotCommitted {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id) == false
            }
            update {}
            to Active
            emit DeliveryStatusNotCommitted {
                delivery_id: delivery_id
            }
        }

        transition ClassifyRefused {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && (self.refused_deliveries.contains_key(delivery_id)
                        || (self.delivery_sequences.get_cloned(delivery_id).get("value") <= self.applied_cursor
                            && (self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllRefused)
                                || self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable))))
            }
            update {}
            to Active
            emit DeliveryStatusRefused {
                delivery_id: delivery_id,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value"),
                reason: if self.refused_deliveries.contains_key(delivery_id) {
                    self.refused_deliveries.get_cloned(delivery_id).get("value")
                } else {
                    if self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllRefused) {
                        DeliveryRefusalReason::AuthorityDenied
                    } else { DeliveryRefusalReason::OperationAuthorizationUnavailable }
                }
            }
        }

        transition ClassifyApplied {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.refused_deliveries.contains_key(delivery_id) == false
                    && (self.recipient_group_outcomes.contains_key(delivery_id) == false
                        || self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllApplied))
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") <= self.applied_cursor
            }
            update {}
            to Active
            emit DeliveryStatusApplied {
                delivery_id: delivery_id,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value")
            }
        }

        transition ClassifyAcknowledgedAhead {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.refused_deliveries.contains_key(delivery_id) == false
                    && (self.delivery_ids.contains(delivery_id)
                        && self.delivery_sequences.get_cloned(delivery_id).get("value") <= self.applied_cursor) == false
                    && self.acknowledged_sequences.contains(self.delivery_sequences.get_cloned(delivery_id).get("value"))
            }
            update {}
            to Active
            emit DeliveryStatusAcknowledgedAhead {
                delivery_id: delivery_id,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value")
            }
        }

        transition ClassifyPending {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.refused_deliveries.contains_key(delivery_id) == false
                    && (self.delivery_ids.contains(delivery_id)
                        && self.delivery_sequences.get_cloned(delivery_id).get("value") <= self.applied_cursor) == false
                    && self.acknowledged_sequences.contains(self.delivery_sequences.get_cloned(delivery_id).get("value")) == false
            }
            update {}
            to Active
            emit DeliveryStatusPending {
                delivery_id: delivery_id,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value")
            }
        }

        transition ClassifyMixed {
            on input ClassifyDeliveryStatus { delivery_id }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.refused_deliveries.contains_key(delivery_id) == false
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") <= self.applied_cursor
                    && self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::Mixed)
            }
            update {}
            to Active
            emit DeliveryStatusMixed {
                delivery_id: delivery_id,
                delivery_sequence: self.delivery_sequences.get_cloned(delivery_id).get("value"),
                bindings: self.delivery_recipient_bindings.get_cloned(delivery_id).get("value"),
                outcomes: self.recipient_outcomes.get_cloned(delivery_id).get("value")
            }
        }

        transition BindNewDeliveryRecipients {
            on input BindDeliveryRecipients { delivery_id, delivery_sequence, recipients }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
                    && self.acknowledged_sequences.contains(delivery_sequence) == false
                    && self.delivery_recipient_bindings.contains_key(delivery_id) == false
            }
            update {
                self.delivery_recipient_bindings.insert(delivery_id, recipients);
                self.recipient_outcomes.insert(delivery_id, EmptyMap);
            }
            to Active
            emit DeliveryRecipientsBound { delivery_id: delivery_id, delivery_sequence: delivery_sequence }
        }

        transition ObserveBoundDeliveryRecipients {
            on input BindDeliveryRecipients { delivery_id, delivery_sequence, recipients }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_sequences.get_cloned(delivery_id) == Some(delivery_sequence)
                    && self.delivery_recipient_bindings.contains_key(delivery_id)
                    && self.delivery_recipient_bindings.get_cloned(delivery_id) == Some(recipients)
            }
            update {}
            to Active
            emit DeliveryRecipientsBound { delivery_id: delivery_id, delivery_sequence: delivery_sequence }
        }

        transition SettlePendingDeliveryRecipient {
            on input SettleDeliveryRecipient { delivery_id, delivery_sequence, recipient_id, target_binding, outcome }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_sequences.get_cloned(delivery_id) == Some(delivery_sequence)
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
                    && self.delivery_recipient_bindings.contains_key(delivery_id)
                    && self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").get_cloned(recipient_id) == Some(target_binding)
                    && self.recipient_outcomes.get_cloned(delivery_id).get("value").contains_key(recipient_id) == false
            }
            update {
                self.recipient_outcomes.insert(delivery_id,
                    runtime_delivery_recipient_outcomes_after_set(
                        self.recipient_outcomes.get_cloned(delivery_id).get("value"),
                        recipient_id, outcome));
            }
            to Active
            emit DeliveryRecipientSettled { delivery_id: delivery_id, delivery_sequence: delivery_sequence, recipient_id: recipient_id, outcome: outcome }
        }

        transition ObserveSettledDeliveryRecipient {
            on input SettleDeliveryRecipient { delivery_id, delivery_sequence, recipient_id, target_binding, outcome }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_sequences.get_cloned(delivery_id) == Some(delivery_sequence)
                    && self.delivery_recipient_bindings.contains_key(delivery_id)
                    && self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").get_cloned(recipient_id) == Some(target_binding)
                    && self.recipient_outcomes.get_cloned(delivery_id).get("value").get_cloned(recipient_id) == Some(outcome)
            }
            update {}
            to Active
            emit DeliveryRecipientSettled { delivery_id: delivery_id, delivery_sequence: delivery_sequence, recipient_id: recipient_id, outcome: outcome }
        }

        transition FinishSettledDeliveryRecipients {
            on input FinishDeliveryRecipients { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_sequences.get_cloned(delivery_id) == Some(delivery_sequence)
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
                    && self.delivery_recipient_bindings.contains_key(delivery_id)
                    && for_all(recipient in self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").keys(),
                        self.recipient_outcomes.get_cloned(delivery_id).get("value").contains_key(recipient))
            }
            update {
                self.recipient_group_outcomes.insert(delivery_id,
                    if for_all(recipient in self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").keys(),
                        self.recipient_outcomes.get_cloned(delivery_id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::Applied)) {
                        DeliveryRecipientGroupOutcome::AllApplied
                    } else {
                        if for_all(recipient in self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").keys(),
                            self.recipient_outcomes.get_cloned(delivery_id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::Refused)) {
                            DeliveryRecipientGroupOutcome::AllRefused
                        } else {
                            if for_all(recipient in self.delivery_recipient_bindings.get_cloned(delivery_id).get("value").keys(),
                                self.recipient_outcomes.get_cloned(delivery_id).get("value").get_cloned(recipient) == Some(DeliveryRecipientOutcome::OperationAuthorizationUnavailable)) {
                                DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable
                            } else { DeliveryRecipientGroupOutcome::Mixed }
                        }
                    });
                if self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllRefused) {
                    self.refused_deliveries.insert(delivery_id, DeliveryRefusalReason::AuthorityDenied);
                }
                if self.recipient_group_outcomes.get_cloned(delivery_id) == Some(DeliveryRecipientGroupOutcome::AllAuthorizationUnavailable) {
                    self.refused_deliveries.insert(delivery_id, DeliveryRefusalReason::OperationAuthorizationUnavailable);
                }
                self.applied_cursor = delivery_sequence;
            }
            to Active
            emit DeliveryRecipientsSettled {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence,
                outcome: self.recipient_group_outcomes.get_cloned(delivery_id).get("value")
            }
        }

        transition ObserveFinishedDeliveryRecipients {
            on input FinishDeliveryRecipients { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_sequences.get_cloned(delivery_id) == Some(delivery_sequence)
                    && self.recipient_group_outcomes.contains_key(delivery_id)
            }
            update {}
            to Active
            emit DeliveryRecipientsSettled {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence,
                outcome: self.recipient_group_outcomes.get_cloned(delivery_id).get("value")
            }
        }
    }
}

impl RuntimeDeliveryMachineAuthority {
    // Pure construction of one delivery's map. All admission, exact-target,
    // repeat and finalization decisions remain in the canonical transitions.
    fn runtime_delivery_recipient_outcomes_after_set(
        current: &std::collections::BTreeMap<String, DeliveryRecipientOutcome>,
        recipient: &str,
        outcome: &DeliveryRecipientOutcome,
    ) -> std::collections::BTreeMap<String, DeliveryRecipientOutcome> {
        let mut updated = current.clone();
        updated.insert(recipient.to_owned(), *outcome);
        updated
    }
}
