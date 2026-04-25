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
    disposition OrderCompleted => external handoff order_completion,
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
        assert_eq!(
            def.dispositions
                .iter()
                .find(|d| d.effect == "OrderCompleted")
                .and_then(|d| d.handoff_protocol.as_ref())
                .map(ToString::to_string)
                .as_deref(),
            Some("order_completion")
        );
        assert!(def.is_stored_phase());
        assert_eq!(def.phase_field_name().unwrap(), "lifecycle_phase");
        assert_eq!(def.terminal_phases.len(), 2);
    }

    #[test]
    fn schema_lowering_preserves_handoff_annotation() {
        let tokens: proc_macro2::TokenStream = ORDER_LIFECYCLE.parse().expect("tokenize");
        let expanded = crate::expand_machine(tokens).expect("expand machine");
        let rendered = expanded.to_string();

        assert!(
            rendered.contains("handoff_protocol : Some"),
            "generated schema must carry handoff_protocol = Some(...)"
        );
        assert!(
            rendered.contains("order_completion"),
            "generated schema must include the declared protocol id"
        );
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

    /// Non-literal arithmetic amount must be rejected by `validate`
    /// rather than silently coerced to `1` in schema codegen.
    pub const COUNTER_NON_LITERAL_INCREMENT: &str = r#"
machine CounterBad {
    version: 1,
    rust: "test" / "counter_bad",

    state {
        lifecycle_phase: CounterBadPhase,
        value: u64,
    }

    init(Idle) {
    }

    terminal []

    phase CounterBadPhase {
        Idle,
    }

    input CounterBadInput {
        Bump { amount: u64 },
    }

    effect CounterBadEffect {
        Bumped,
    }

    transition BumpIdle {
        on input Bump { amount }
        guard { self.lifecycle_phase == Phase::Idle }
        update {
            self.value += amount;
        }
        to Idle
    }
}
"#;

    #[test]
    fn validate_rejects_non_literal_increment_amount() {
        let tokens: proc_macro2::TokenStream =
            COUNTER_NON_LITERAL_INCREMENT.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        let err = crate::validate::validate(&def).expect_err("should reject non-literal amount");
        let msg = err.to_string();
        assert!(
            msg.contains("must be a u64 literal"),
            "expected literal-only error, got: {msg}"
        );
    }

    /// Non-literal map-increment amount must likewise be rejected.
    pub const MAP_NON_LITERAL_INCREMENT: &str = r#"
machine MapBad {
    version: 1,
    rust: "test" / "map_bad",

    state {
        lifecycle_phase: MapBadPhase,
        counts: Map<String, u64>,
        step: u64,
    }

    init(Idle) {
        step = 1,
    }

    terminal []

    phase MapBadPhase {
        Idle,
    }

    input MapBadInput {
        Tick { key: String },
    }

    effect MapBadEffect {
        Ticked,
    }

    transition TickIdle {
        on input Tick { key }
        guard { self.lifecycle_phase == Phase::Idle }
        update {
            self.counts.increment(key, self.step);
        }
        to Idle
    }
}
"#;

    #[test]
    fn validate_rejects_non_literal_map_increment_amount() {
        let tokens: proc_macro2::TokenStream = MAP_NON_LITERAL_INCREMENT.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        let err = crate::validate::validate(&def).expect_err("should reject non-literal amount");
        let msg = err.to_string();
        assert!(
            msg.contains("must be a u64 literal"),
            "expected literal-only error, got: {msg}"
        );
    }

    /// Literal arithmetic amount must parse AND validate.
    pub const COUNTER_LITERAL_INCREMENT: &str = r#"
machine CounterOk {
    version: 1,
    rust: "test" / "counter_ok",

    state {
        lifecycle_phase: CounterOkPhase,
        value: u64,
    }

    init(Idle) {
    }

    terminal []

    phase CounterOkPhase {
        Idle,
    }

    input CounterOkInput {
        Bump,
    }

    effect CounterOkEffect {
        Bumped,
    }

    transition BumpIdle {
        on input Bump
        guard { self.lifecycle_phase == Phase::Idle }
        update {
            self.value += 5;
        }
        to Idle
    }
}
"#;

    #[test]
    fn validate_accepts_literal_increment_amount() {
        let tokens: proc_macro2::TokenStream = COUNTER_LITERAL_INCREMENT.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        crate::validate::validate(&def).expect("literal amount should validate");
    }

    /// `self.phase != Phase::X` must derive to `all_phases \ {X}`, not
    /// silently widen to `all_phases`.
    pub const NEQ_PHASE_GUARD: &str = r#"
machine NeqPhase {
    version: 1,
    rust: "test" / "neq_phase",

    state {
        lifecycle_phase: NeqPhasePhase,
    }

    init(A) {
    }

    terminal [C]

    phase NeqPhasePhase {
        A,
        B,
        C,
    }

    input NeqPhaseInput {
        Ping,
    }

    effect NeqPhaseEffect {
        Pinged,
    }

    transition PingAnywhereButC {
        on input Ping
        guard { self.lifecycle_phase != Phase::C }
        update {}
        to A
    }
}
"#;

    #[test]
    fn neq_phase_guard_narrows_from_set() {
        let tokens: proc_macro2::TokenStream = NEQ_PHASE_GUARD.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        crate::validate::validate(&def).expect("valid machine");
        let t = def
            .transitions
            .iter()
            .find(|t| t.name == "PingAnywhereButC")
            .expect("transition");
        let from = crate::gen_schema::derive_from_phases(&def, t).expect("phase set should derive");
        // Expected: {A, B}, i.e. all phases minus C.
        assert_eq!(from.len(), 2, "expected A and B, got {from:?}");
        assert!(from.contains(&"A".to_string()));
        assert!(from.contains(&"B".to_string()));
        assert!(!from.contains(&"C".to_string()));
    }

    /// Conjunction of two phase-narrowing guards should intersect, not
    /// union (the old behavior). `phase == A && phase == B` is
    /// unsatisfiable and must be flagged.
    pub const AND_PHASE_GUARD_UNSAT: &str = r#"
machine AndPhaseUnsat {
    version: 1,
    rust: "test" / "and_phase_unsat",

    state {
        lifecycle_phase: AndPhaseUnsatPhase,
    }

    init(A) {
    }

    terminal []

    phase AndPhaseUnsatPhase {
        A,
        B,
    }

    input AndPhaseUnsatInput {
        Ping,
    }

    effect AndPhaseUnsatEffect {
        Pinged,
    }

    transition Impossible {
        on input Ping
        guard "a" { self.lifecycle_phase == Phase::A }
        guard "b" { self.lifecycle_phase == Phase::B }
        update {}
        to A
    }
}
"#;

    #[test]
    fn and_phase_guard_unsat_is_flagged() {
        let tokens: proc_macro2::TokenStream = AND_PHASE_GUARD_UNSAT.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        let err = crate::validate::validate(&def).expect_err("unsat phase conjunction");
        let msg = err.to_string();
        assert!(
            msg.contains("empty phase set"),
            "expected empty-phase-set error, got: {msg}"
        );
    }

    /// A guard that references the phase field in a shape this compiler
    /// does not analyze must be flagged, not silently widened to all
    /// phases. Here we use a comparison between the phase field and a
    /// bound variant from another enum — nonsensical, but the compiler
    /// should reject it rather than shrug and emit `from = all_phases`.
    pub const UNANALYZABLE_PHASE_GUARD: &str = r#"
machine Unanalyzable {
    version: 1,
    rust: "test" / "unanalyzable",

    state {
        lifecycle_phase: UnanalyzablePhase,
        counter: u64,
    }

    init(A) {
    }

    terminal []

    phase UnanalyzablePhase {
        A,
        B,
    }

    input UnanalyzableInput {
        Ping,
    }

    effect UnanalyzableEffect {
        Pinged,
    }

    transition Weird {
        on input Ping
        guard { self.lifecycle_phase == self.lifecycle_phase }
        update {}
        to A
    }
}
"#;

    #[test]
    fn unanalyzable_phase_guard_is_flagged() {
        let tokens: proc_macro2::TokenStream = UNANALYZABLE_PHASE_GUARD.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        let err = crate::validate::validate(&def).expect_err("unanalyzable phase guard");
        let msg = err.to_string();
        assert!(
            msg.contains("Phase::Variant") || msg.contains("not yet supported"),
            "expected precise/unsupported-shape error, got: {msg}"
        );
    }

    /// Helper-call guard (`helper(self.lifecycle_phase)`) should still
    /// resolve to the union of phase literals in the helper body.
    pub const HELPER_PHASE_GUARD: &str = r#"
machine HelperPhase {
    version: 1,
    rust: "test" / "helper_phase",

    state {
        lifecycle_phase: HelperPhasePhase,
    }

    init(A) {
    }

    terminal [D]

    phase HelperPhasePhase {
        A,
        B,
        C,
        D,
    }

    input HelperPhaseInput {
        Ping,
    }

    effect HelperPhaseEffect {
        Pinged,
    }

    helper is_active(p: HelperPhasePhase) -> bool {
        p == Phase::A || p == Phase::B
    }

    transition PingActive {
        on input Ping
        guard { is_active(self.lifecycle_phase) }
        update {}
        to A
    }
}
"#;

    #[test]
    fn helper_phase_guard_resolves_via_body() {
        let tokens: proc_macro2::TokenStream = HELPER_PHASE_GUARD.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        crate::validate::validate(&def).expect("valid machine");
        let t = def
            .transitions
            .iter()
            .find(|t| t.name == "PingActive")
            .expect("transition");
        let from = crate::gen_schema::derive_from_phases(&def, t).expect("phase set should derive");
        assert_eq!(from.len(), 2, "expected A and B, got {from:?}");
        assert!(from.contains(&"A".to_string()));
        assert!(from.contains(&"B".to_string()));
    }

    /// Non-phase guards alone leave `from` as all phases (legitimate —
    /// the guard doesn't constrain the phase).
    pub const NONPHASE_GUARD: &str = r#"
machine NonPhase {
    version: 1,
    rust: "test" / "non_phase",

    state {
        lifecycle_phase: NonPhasePhase,
        counter: u64,
    }

    init(A) {
    }

    terminal []

    phase NonPhasePhase {
        A,
        B,
    }

    input NonPhaseInput {
        Ping,
    }

    effect NonPhaseEffect {
        Pinged,
    }

    transition PingIfPositive {
        on input Ping
        guard { self.counter > 0 }
        update {}
        to A
    }
}
"#;

    #[test]
    fn nonphase_guard_leaves_from_as_all_phases() {
        let tokens: proc_macro2::TokenStream = NONPHASE_GUARD.parse().expect("tokenize");
        let def = crate::parse::parse_machine(tokens).expect("parse");
        crate::validate::validate(&def).expect("valid machine");
        let t = def
            .transitions
            .iter()
            .find(|t| t.name == "PingIfPositive")
            .expect("transition");
        let from = crate::gen_schema::derive_from_phases(&def, t).expect("phase set should derive");
        assert_eq!(from, vec!["A".to_string(), "B".to_string()]);
    }
}
