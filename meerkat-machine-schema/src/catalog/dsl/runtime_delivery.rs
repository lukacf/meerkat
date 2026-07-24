use meerkat_machine_dsl::machine;

use super::OptionValueExt;

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
        }

        init(Active) {
            delivery_ids = EmptySet,
            delivery_sequences = EmptyMap,
            delivery_source_sequences = EmptyMap,
            committed_sequences = EmptySet,
            next_sequence = 0,
            applied_cursor = 0,
        }

        terminal []

        phase RuntimeDeliveryPhase {
            Active,
        }

        input RuntimeDeliveryInput {
            CommitDelivery {
                delivery_id: String,
                source_sequence: u64,
            },
            MarkDeliveryApplied {
                delivery_id: String,
                delivery_sequence: u64,
            },
        }

        effect RuntimeDeliveryEffect {
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

        disposition DeliveryCommitted => routed [DetachedJobMachine] seam NoOwnerRealization,
        disposition DeliveryReused => routed [DetachedJobMachine] seam NoOwnerRealization,
        disposition DeliveryApplied => local seam OwnerRealizationOnly,

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

        transition ApplyNextDelivery {
            on input MarkDeliveryApplied { delivery_id, delivery_sequence }
            guard {
                self.lifecycle_phase == Phase::Active
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence > self.applied_cursor
                    && delivery_sequence - 1 == self.applied_cursor
            }
            update {
                self.applied_cursor = delivery_sequence;
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
                    && self.delivery_ids.contains(delivery_id)
                    && self.delivery_sequences.get_cloned(delivery_id).get("value") == delivery_sequence
                    && delivery_sequence <= self.applied_cursor
            }
            update {}
            to Active
            emit DeliveryApplied {
                delivery_id: delivery_id,
                delivery_sequence: delivery_sequence
            }
        }
    }
}
