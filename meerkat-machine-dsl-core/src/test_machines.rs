//! Test machine definitions for verifying DSL parsing and code generation.
//!
//! These are token streams that exercise every DSL feature. Tests parse them
//! into `MachineDef` ASTs and verify the structure, then (once generators are
//! built) expand them and verify the generated code compiles and behaves correctly.

/// Minimal stored-phase machine: traffic light with 2 phases, 1 input.
pub const TRAFFIC_LIGHT: &str = r#"
machine TrafficLight {
    version: 1,
    rust: "test" / "traffic_light",

    state {
        phase: TrafficPhase,
    }

    init(Green) {
        // phase field is set automatically for stored-phase machines
    }

    terminal []

    phase TrafficPhase {
        Green,
        Red,
    }

    input TrafficInput {
        Toggle,
    }

    effect TrafficEffect {
        Switched,
    }

    disposition Switched => local,

    transition ToggleGreen {
        on input Toggle
        guard { self.phase == Phase::Green }
        update {}
        to Red
        emit Switched
    }

    transition ToggleRed {
        on input Toggle
        guard { self.phase == Phase::Red }
        update {}
        to Green
        emit Switched
    }
}
"#;

/// Convoluted stored-phase machine exercising the full DSL vocabulary:
/// - Stored phase (OrderPhase field)
/// - Multiple transitions per input variant with different guards
/// - Option fields with is_some/is_none guards
/// - u64 fields with increment/decrement/comparison
/// - Set fields with insert/remove/contains
/// - Map fields with insert/get
/// - Conditional updates
/// - Helper functions
/// - Invariants
/// - Multiple effect variants with payload fields
/// - Quantifier in invariant (for_all)
/// - String fields
pub const ORDER_LIFECYCLE: &str = r#"
machine OrderLifecycle {
    version: 1,
    rust: "test" / "order_lifecycle",

    state {
        lifecycle_phase: OrderPhase,
        order_id: String,
        item_count: u64,
        total_price: u64,
        assigned_to: Option<String>,
        paid_at: Option<u64>,
        tags: Set<String>,
        metadata: Map<String, String>,
        attempt_count: u64,
        failure_reason: Option<String>,
    }

    init(Draft) {
        order_id = "order-0",
        item_count = 0,
        total_price = 0,
        assigned_to = None,
        paid_at = None,
        tags = EmptySet,
        metadata = EmptyMap,
        attempt_count = 0,
        failure_reason = None,
    }

    terminal [Completed, Cancelled]

    phase OrderPhase {
        Draft,
        Submitted,
        Assigned,
        Paid,
        Completed,
        Cancelled,
    }

    input OrderInput {
        AddItem { price: u64 },
        RemoveItem { price: u64 },
        Submit,
        Assign { assignee: String },
        Pay { at_utc_ms: u64, receipt: String },
        Complete { note: String },
        Cancel { reason: String },
        Tag { tag: String },
        Untag { tag: String },
        Retry,
        SetMeta { key: String, value: String },
    }

    signal OrderSignal {
        ExternalValidation { valid: bool },
    }

    effect OrderEffect {
        OrderSubmitted { order_id: String },
        OrderAssigned { order_id: String, assignee: String },
        OrderPaid { order_id: String, amount: u64 },
        OrderCompleted { order_id: String },
        OrderCancelled { order_id: String, reason: String },
        RetryAttempted { attempt: u64 },
    }

    disposition OrderSubmitted => local,
    disposition OrderAssigned => local,
    disposition OrderPaid => local,
    disposition OrderCompleted => external,
    disposition OrderCancelled => external,
    disposition RetryAttempted => local,

    helper is_active_phase(p: OrderPhase) -> bool {
        p == Phase::Draft || p == Phase::Submitted || p == Phase::Assigned || p == Phase::Paid
    }

    invariant assigned_requires_assignee {
        self.lifecycle_phase != Phase::Assigned || self.assigned_to.is_some()
    }

    invariant paid_requires_payment {
        self.lifecycle_phase != Phase::Paid || self.paid_at.is_some()
    }

    invariant cancelled_requires_reason {
        self.lifecycle_phase != Phase::Cancelled || self.failure_reason.is_some()
    }

    invariant active_tags_bounded {
        for_all(tag in self.tags, tag != "")
    }

    // --- Draft transitions ---

    transition AddItemDraft {
        on input AddItem { price }
        guard { self.lifecycle_phase == Phase::Draft }
        update {
            self.item_count += 1;
            self.total_price = self.total_price + price;
        }
        to Draft
    }

    transition RemoveItemDraft {
        on input RemoveItem { price }
        guard { self.lifecycle_phase == Phase::Draft && self.item_count > 0 }
        update {
            self.item_count -= 1;
            if self.total_price >= price {
                self.total_price = self.total_price - price;
            } else {
                self.total_price = 0;
            }
        }
        to Draft
    }

    transition SubmitDraft {
        on input Submit
        guard { self.lifecycle_phase == Phase::Draft && self.item_count > 0 }
        update {
            self.attempt_count += 1;
        }
        to Submitted
        emit OrderSubmitted { order_id: self.order_id }
    }

    // --- Submitted transitions ---

    transition AssignSubmitted {
        on input Assign { assignee }
        guard { self.lifecycle_phase == Phase::Submitted }
        update {
            self.assigned_to = Some(assignee);
        }
        to Assigned
        emit OrderAssigned { order_id: self.order_id, assignee: assignee }
    }

    // --- Assigned transitions ---

    transition PayAssigned {
        on input Pay { at_utc_ms, receipt }
        guard { self.lifecycle_phase == Phase::Assigned && self.assigned_to.is_some() }
        update {
            self.paid_at = Some(at_utc_ms);
            self.metadata.insert("receipt", receipt);
        }
        to Paid
        emit OrderPaid { order_id: self.order_id, amount: self.total_price }
    }

    // --- Paid transitions ---

    transition CompletePaid {
        on input Complete { note }
        guard { self.lifecycle_phase == Phase::Paid }
        update {
            self.metadata.insert("completion_note", note);
        }
        to Completed
        emit OrderCompleted { order_id: self.order_id }
    }

    // --- Cancel from any active phase ---

    transition CancelActive {
        on input Cancel { reason }
        guard { is_active_phase(self.lifecycle_phase) }
        update {
            self.failure_reason = Some(reason);
            self.assigned_to = None;
            self.paid_at = None;
        }
        to Cancelled
        emit OrderCancelled { order_id: self.order_id, reason: reason }
    }

    // --- Tagging (any active phase, self-loop) ---

    transition TagActive {
        on input Tag { tag }
        guard { is_active_phase(self.lifecycle_phase) && !self.tags.contains(tag) }
        update {
            self.tags.insert(tag);
        }
        to Draft
    }

    transition UntagActive {
        on input Untag { tag }
        guard { is_active_phase(self.lifecycle_phase) && self.tags.contains(tag) }
        update {
            self.tags.remove(tag);
        }
        to Draft
    }

    // --- Retry from submitted (self-loop with counter) ---

    transition RetrySubmitted {
        on input Retry
        guard { self.lifecycle_phase == Phase::Submitted && self.attempt_count < 3 }
        update {
            self.attempt_count += 1;
        }
        to Submitted
        emit RetryAttempted { attempt: self.attempt_count }
    }

    // --- Metadata (any active phase) ---

    transition SetMetaActive {
        on input SetMeta { key, value }
        guard { is_active_phase(self.lifecycle_phase) }
        update {
            self.metadata.insert(key, value);
        }
        to Draft
    }

    // --- Signal handling ---

    transition ValidationFailed {
        on signal ExternalValidation { valid }
        guard { self.lifecycle_phase == Phase::Submitted && !valid }
        update {
            self.failure_reason = Some("validation_failed");
        }
        to Cancelled
        emit OrderCancelled { order_id: self.order_id, reason: "validation_failed" }
    }
}
"#;

/// Derived-phase machine: counter with phase computed from field values.
/// Exercises the phase_projection block.
pub const COUNTER: &str = r#"
machine Counter {
    version: 1,
    rust: "test" / "counter",

    state {
        value: u64,
        active: bool,
        limit: u64,
    }

    init(Idle) {
        value = 0,
        active = false,
        limit = 10,
    }

    terminal [Stopped]

    phase CounterPhase {
        Idle,
        Counting,
        AtLimit,
        Stopped,
    }

    phase_projection {
        Stopped     when !self.active,
        AtLimit     when self.value >= self.limit,
        Counting    when self.value > 0,
        Idle,
    }

    input CounterInput {
        Start,
        Increment { amount: u64 },
        Decrement { amount: u64 },
        Reset,
        Stop,
    }

    effect CounterEffect {
        Started,
        LimitReached { value: u64, limit: u64 },
        CounterStopped { final_value: u64 },
    }

    transition StartIdle {
        on input Start
        guard { !self.active }
        update {
            self.active = true;
        }
        to Idle
        emit Started
    }

    transition IncrementActive {
        on input Increment { amount }
        guard { self.active && self.value + amount < self.limit }
        update {
            self.value = self.value + amount;
        }
        to Counting
    }

    transition IncrementToLimit {
        on input Increment { amount }
        guard { self.active && self.value + amount >= self.limit }
        update {
            self.value = self.limit;
        }
        to AtLimit
        emit LimitReached { value: self.limit, limit: self.limit }
    }

    transition DecrementActive {
        on input Decrement { amount }
        guard { self.active && self.value >= amount }
        update {
            self.value = self.value - amount;
        }
        to Counting
    }

    transition ResetActive {
        on input Reset
        guard { self.active }
        update {
            self.value = 0;
        }
        to Idle
    }

    transition StopActive {
        on input Stop
        guard { self.active }
        update {
            self.active = false;
        }
        to Stopped
        emit CounterStopped { final_value: self.value }
    }
}
"#;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn parse(source: &str) -> crate::ast::MachineDef {
        let tokens: proc_macro2::TokenStream = source.parse().expect("tokenize");
        crate::parse::parse_machine(tokens).expect("parse")
    }

    #[test]
    fn parse_traffic_light() {
        let def = parse(TRAFFIC_LIGHT);
        assert_eq!(def.name, "TrafficLight");
        assert_eq!(def.phase_enum.variants.len(), 2);
        assert_eq!(def.transitions.len(), 2);
        assert!(def.is_stored_phase());
        assert_eq!(def.phase_field_name().unwrap(), "phase");
    }

    #[test]
    fn parse_order_lifecycle() {
        let def = parse(ORDER_LIFECYCLE);
        assert_eq!(def.name, "OrderLifecycle");
        assert_eq!(def.version, 1);
        assert_eq!(def.phase_enum.variants.len(), 6);
        assert_eq!(def.state_fields.len(), 10);
        assert_eq!(def.inputs.variants.len(), 11);
        assert_eq!(def.signals.variants.len(), 1);
        assert_eq!(def.effects.variants.len(), 6);
        assert_eq!(def.helpers.len(), 1);
        assert_eq!(def.invariants.len(), 4);
        assert_eq!(def.transitions.len(), 12);
        assert_eq!(def.dispositions.len(), 6);
        assert!(def.is_stored_phase());
        assert_eq!(def.phase_field_name().unwrap(), "lifecycle_phase");
        assert_eq!(def.terminal_phases.len(), 2);
    }

    #[test]
    fn parse_counter() {
        let def = parse(COUNTER);
        assert_eq!(def.name, "Counter");
        assert_eq!(def.phase_enum.variants.len(), 4);
        assert!(!def.is_stored_phase());
        assert!(def.phase_projection.is_some());
        let proj = def.phase_projection.as_ref().unwrap();
        assert_eq!(proj.rules.len(), 4);
        // Last rule (Idle) should be the fallback — no condition
        assert!(proj.rules[3].condition.is_none());
        assert_eq!(def.transitions.len(), 6);
    }

    #[test]
    fn order_lifecycle_transition_structure() {
        let def = parse(ORDER_LIFECYCLE);

        // Find CancelActive — should have a helper call guard
        let cancel = def
            .transitions
            .iter()
            .find(|t| t.name == "CancelActive")
            .expect("CancelActive transition");
        assert!(!cancel.guards.is_empty());
        assert_eq!(cancel.effects.len(), 1);
        assert_eq!(cancel.effects[0].variant, "OrderCancelled");
        assert_eq!(cancel.updates.len(), 3); // failure_reason, assigned_to, paid_at

        // Find PayAssigned — should have map insert in updates
        let pay = def
            .transitions
            .iter()
            .find(|t| t.name == "PayAssigned")
            .expect("PayAssigned transition");
        assert_eq!(pay.updates.len(), 2); // paid_at assign + metadata insert

        // Find RemoveItemDraft — should have conditional update
        let remove = def
            .transitions
            .iter()
            .find(|t| t.name == "RemoveItemDraft")
            .expect("RemoveItemDraft transition");
        // item_count decrement + conditional total_price update
        assert_eq!(remove.updates.len(), 2);
    }

    #[test]
    fn counter_phase_projection_conditions() {
        let def = parse(COUNTER);
        let proj = def.phase_projection.as_ref().unwrap();

        assert_eq!(proj.rules[0].phase, "Stopped");
        assert!(proj.rules[0].condition.is_some()); // !self.active

        assert_eq!(proj.rules[1].phase, "AtLimit");
        assert!(proj.rules[1].condition.is_some()); // self.value >= self.limit

        assert_eq!(proj.rules[2].phase, "Counting");
        assert!(proj.rules[2].condition.is_some()); // self.value > 0

        assert_eq!(proj.rules[3].phase, "Idle");
        assert!(proj.rules[3].condition.is_none()); // fallback
    }
}

/// Authored proof machine for Meerkat-shaped map-keyed guard/update patterns.
///
/// This test machine exercises the EXACT DSL patterns needed for absorbing
/// input lifecycle, ops lifecycle, and ingress state into MeerkatMachine:
/// - Map field with enum-typed values
/// - `.contains_key(key)` in guards (emits BTreeMap::contains_key)
/// - `.get(key)` in guards (emits BTreeMap::get)
/// - `.insert(key, value)` in updates
/// - `.remove(key)` in updates
/// - Monotonic sequence counter for FIFO ordering
pub const MAP_KEYED_AUTHORITY: &str = r#"
machine MapKeyedAuthority {
    version: 1,
    rust: "test" / "map_keyed_authority",

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
        Register { entity_id },
        Advance { entity_id },
        Complete { entity_id },
        Remove { entity_id },
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

    // Advance: guard on key present AND value matches expected
    transition AdvanceEntity {
        on input Advance { entity_id }
        guard { self.lifecycle_phase == Phase::Active }
        guard "is_registered" { self.entity_statuses.get(entity_id) == Some("registered") }
        update {
            self.entity_statuses.insert(entity_id, "running");
        }
        to Active
        emit EntityAdvanced { entity_id: entity_id, seq: self.entity_seq.get(entity_id) }
    }

    // Complete: guard on key present with specific value, then update
    transition CompleteEntity {
        on input Complete { entity_id }
        guard "is_running" { self.entity_statuses.get(entity_id) == Some("running") }
        update {
            self.entity_statuses.insert(entity_id, "completed");
            self.active_count = self.active_count - 1;
        }
        to Active
        emit EntityCompleted { entity_id: entity_id }
    }

    // Remove: remove key from map
    transition RemoveEntity {
        on input Remove { entity_id }
        guard "exists" { self.entity_statuses.contains_key(entity_id) }
        guard "is_terminal" { self.entity_statuses.get(entity_id) == Some("completed") }
        update {
            self.entity_statuses.remove(entity_id);
            self.entity_seq.remove(entity_id);
        }
        to Active
        emit EntityRemoved { entity_id: entity_id }
    }

    // Drain: no new registrations allowed
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
"#;
