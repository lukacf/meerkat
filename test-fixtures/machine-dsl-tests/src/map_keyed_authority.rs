//! Authored proof machine for Meerkat-shaped map-keyed guard/update patterns.
//!
//! Exercises: `.contains_key()` in guards, `.get()` in guards,
//! `.insert()`/`.remove()` in updates, monotonic sequence counters.
use meerkat_machine_dsl::machine;

machine! {
    machine MapKeyedAuthority {
        version: 1,
        rust: "machine-dsl-tests" / "map_keyed_authority",

        state {
            lifecycle_phase: AuthPhase,
            entity_statuses: Map<String, String>,
            entity_seq: Map<String, u64>,
            next_seq: u64,
            active_count: u64,
        }

        init(Active) {
            entity_statuses = EmptyMap,
            entity_seq = EmptyMap,
            next_seq = 0,
            active_count = 0,
        }

        terminal [Destroyed]

        phase AuthPhase {
            Active,
            Draining,
            Destroyed,
        }

        input AuthInput {
            Register { entity_id: String },
            Advance { entity_id: String },
            Complete { entity_id: String },
            Remove { entity_id: String },
            Drain,
            Destroy,
        }

        signal AuthSignal {
            Tick,
        }

        effect AuthEffect {
            EntityRegistered { entity_id: String },
            EntityAdvanced { entity_id: String, seq: u64 },
            EntityCompleted { entity_id: String },
            EntityRemoved { entity_id: String },
        }

        disposition EntityRegistered => local,
        disposition EntityAdvanced => local,
        disposition EntityCompleted => local,
        disposition EntityRemoved => local,

        // Register: guard on key NOT present, insert into map
        transition RegisterEntity {
            on input Register { entity_id }
            guard { self.lifecycle_phase == Phase::Active }
            guard "not_already_registered" { !self.entity_statuses.contains_key(entity_id) }
            update {
                self.entity_statuses.insert(entity_id, "registered");
                self.entity_seq.insert(entity_id, self.next_seq);
                self.next_seq = self.next_seq + 1;
                self.active_count = self.active_count + 1;
            }
            to Active
            emit EntityRegistered { entity_id: entity_id }
        }

        // Advance: guard on key present (contains_key pattern)
        transition AdvanceEntity {
            on input Advance { entity_id }
            guard { self.lifecycle_phase == Phase::Active }
            guard "is_registered" { self.entity_statuses.contains_key(entity_id) }
            update {
                self.entity_statuses.insert(entity_id, "running");
            }
            to Active
            emit EntityAdvanced { entity_id: entity_id, seq: self.next_seq }
        }

        // Complete: guard on key present
        transition CompleteEntity {
            on input Complete { entity_id }
            guard "is_running" { self.entity_statuses.contains_key(entity_id) }
            update {
                self.entity_statuses.insert(entity_id, "completed");
                self.active_count = self.active_count - 1;
            }
            to Active
            emit EntityCompleted { entity_id: entity_id }
        }

        // Remove: guard on key present, remove from map
        transition RemoveEntity {
            on input Remove { entity_id }
            guard "exists" { self.entity_statuses.contains_key(entity_id) }
            update {
                self.entity_statuses.remove(entity_id);
                self.entity_seq.remove(entity_id);
            }
            to Active
            emit EntityRemoved { entity_id: entity_id }
        }

        // Drain: no new registrations
        transition StartDrain {
            on input Drain
            guard { self.lifecycle_phase == Phase::Active }
            update {}
            to Draining
        }

        // Destroy: terminal
        transition DestroyMachine {
            on input Destroy
            guard { self.active_count == 0 }
            update {}
            to Destroyed
        }

        // Tick: self-loop on Active
        transition TickActive {
            on signal Tick
            guard { self.lifecycle_phase == Phase::Active }
            update {}
            to Active
        }
    }
}
