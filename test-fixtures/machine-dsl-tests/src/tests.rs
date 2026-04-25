use meerkat_machine_dsl::machine;

// ============================================================
// End-to-end: DSL → runtime code + schema → TLA+ rendering
// ============================================================

mod e2e_tla {
    use super::*;

    machine! {
        machine Turnstile {
            version: 1,
            rust: "test" / "turnstile",

            state {
                phase: TurnstilePhase,
                coin_count: u64,
                pass_count: u64,
            }

            init(Locked) {
                coin_count = 0,
                pass_count = 0,
            }

            terminal []

            phase TurnstilePhase {
                Locked,
                Unlocked,
            }

            input TurnstileInput {
                InsertCoin,
                Push,
            }

            effect TurnstileEffect {
                CoinAccepted { total: u64 },
                PersonPassed { total: u64 },
                PushRejected,
            }

            invariant unlocked_requires_coin {
                self.phase != Phase::Unlocked || self.coin_count > 0
            }

            disposition CoinAccepted => local,
            disposition PersonPassed => local,
            disposition PushRejected => external,

            transition InsertCoinLocked {
                on input InsertCoin
                guard { self.phase == Phase::Locked }
                update {
                    self.coin_count += 1;
                }
                to Unlocked
                emit CoinAccepted { total: self.coin_count }
            }

            transition InsertCoinUnlocked {
                on input InsertCoin
                guard { self.phase == Phase::Unlocked }
                update {
                    self.coin_count += 1;
                }
                to Unlocked
                emit CoinAccepted { total: self.coin_count }
            }

            transition PushUnlocked {
                on input Push
                guard { self.phase == Phase::Unlocked }
                update {
                    self.pass_count += 1;
                }
                to Locked
                emit PersonPassed { total: self.pass_count }
            }

            transition PushLocked {
                on input Push
                guard { self.phase == Phase::Locked }
                update {}
                to Locked
                emit PushRejected
            }
        }
    }

    // ---- Direction 1: Runtime dispatch works ----

    #[test]
    fn runtime_dispatch_full_cycle() {
        let mut auth = TurnstileAuthority::new();
        assert_eq!(auth.state.phase(), TurnstilePhase::Locked);

        // Insert coin → Unlocked
        let r = TurnstileMutator::apply(&mut auth, TurnstileInput::InsertCoin).unwrap();
        assert_eq!(r.to_phase, TurnstilePhase::Unlocked);
        assert_eq!(auth.state.coin_count, 1);

        // Push → Locked (person passes)
        let r = TurnstileMutator::apply(&mut auth, TurnstileInput::Push).unwrap();
        assert_eq!(r.to_phase, TurnstilePhase::Locked);
        assert_eq!(auth.state.pass_count, 1);

        // Push while locked → stays Locked (rejected)
        let r = TurnstileMutator::apply(&mut auth, TurnstileInput::Push).unwrap();
        assert_eq!(r.to_phase, TurnstilePhase::Locked);
        assert_eq!(r.effects, vec![TurnstileEffect::PushRejected]);
    }

    // ---- Direction 2: Schema validates ----

    #[test]
    fn schema_validates() {
        let schema = TurnstileState::schema();
        schema.validate().expect("turnstile schema should validate");
    }

    #[test]
    fn schema_structure() {
        let schema = TurnstileState::schema();
        assert_eq!(schema.machine.as_str(), "Turnstile");
        assert_eq!(schema.state.phase.variants.len(), 2);
        assert_eq!(schema.state.fields.len(), 2); // phase field excluded from schema
        assert_eq!(schema.transitions.len(), 4);
        assert_eq!(schema.effects.variants.len(), 3);
        assert_eq!(schema.invariants.len(), 1);
        assert_eq!(schema.effect_dispositions.len(), 3);

        // Verify from-phases are derived correctly
        let insert_locked = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "InsertCoinLocked")
            .unwrap();
        assert_eq!(
            insert_locked
                .from
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["Locked"]
        );

        let push_unlocked = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "PushUnlocked")
            .unwrap();
        assert_eq!(
            push_unlocked
                .from
                .iter()
                .map(|p| p.as_str())
                .collect::<Vec<_>>(),
            vec!["Unlocked"]
        );
    }

    // ---- Direction 3: Schema → TLA+ rendering ----

    #[test]
    fn schema_renders_to_tla_module() {
        let schema = TurnstileState::schema();
        let tla = meerkat_machine_codegen::render_machine_module(&schema);

        // Verify the TLA+ module has expected structure
        assert!(
            tla.contains("---- MODULE Machine_Turnstile ----"),
            "TLA+ should contain module header"
        );
        assert!(tla.contains("STATE"), "TLA+ should contain STATE section");
        assert!(
            tla.contains("phase : "),
            "TLA+ should declare phase variable"
        );
        assert!(
            tla.contains("coin_count"),
            "TLA+ should reference coin_count field"
        );
        assert!(
            tla.contains("pass_count"),
            "TLA+ should reference pass_count field"
        );
        assert!(
            tla.contains("TRANSITIONS"),
            "TLA+ should contain TRANSITIONS section"
        );
        assert!(
            tla.contains("InsertCoinLocked"),
            "TLA+ should contain InsertCoinLocked transition"
        );
        assert!(
            tla.contains("PushUnlocked"),
            "TLA+ should contain PushUnlocked transition"
        );
        assert!(
            tla.contains("INVARIANTS"),
            "TLA+ should contain INVARIANTS section"
        );
        assert!(
            tla.contains("unlocked_requires_coin"),
            "TLA+ should contain invariant"
        );
        assert!(
            tla.contains("EFFECTS"),
            "TLA+ should contain EFFECTS section"
        );
        assert!(
            tla.contains("===="),
            "TLA+ should end with module terminator"
        );
    }

    // ---- Direction 4: Schema → kernel interpreter round-trip ----

    #[test]
    fn schema_feeds_kernel_interpreter() {
        let schema = TurnstileState::schema();

        // The kernel interpreter can construct from the schema and run transitions
        let kernel = meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(schema);
        let state = kernel.initial_state().expect("initial state should work");

        assert_eq!(state.phase.as_str(), "Locked");

        // Feed InsertCoin input
        let input = meerkat_machine_kernels::test_oracle::KernelInput {
            variant: meerkat_machine_schema::identity::InputVariantId::parse("InsertCoin")
                .expect("valid input variant slug"),
            fields: std::collections::BTreeMap::new(),
        };
        let outcome = kernel
            .transition(&state, &input)
            .expect("InsertCoin should succeed");
        assert_eq!(outcome.next_state.phase.as_str(), "Unlocked");
        assert_eq!(outcome.effects.len(), 1);
        assert_eq!(outcome.effects[0].variant.as_str(), "CoinAccepted");
    }

    // ---- Full e2e: DSL dispatch == kernel dispatch ----

    #[test]
    fn dsl_dispatch_matches_kernel_dispatch() {
        let schema = TurnstileState::schema();
        let kernel = meerkat_machine_kernels::test_oracle::GeneratedMachineKernel::new(schema);

        // Run the same sequence through both dispatchers
        let mut auth = TurnstileAuthority::new();
        let mut kernel_state = kernel.initial_state().unwrap();

        let inputs = vec![
            ("InsertCoin", TurnstileInput::InsertCoin),
            ("Push", TurnstileInput::Push),
            ("Push", TurnstileInput::Push),
            ("InsertCoin", TurnstileInput::InsertCoin),
            ("InsertCoin", TurnstileInput::InsertCoin),
            ("Push", TurnstileInput::Push),
        ];

        for (variant_name, dsl_input) in inputs {
            let kernel_input = meerkat_machine_kernels::test_oracle::KernelInput {
                variant: meerkat_machine_schema::identity::InputVariantId::parse(variant_name)
                    .expect("valid input variant slug"),
                fields: std::collections::BTreeMap::new(),
            };

            let dsl_result = TurnstileMutator::apply(&mut auth, dsl_input);
            let kernel_result = kernel.transition(&kernel_state, &kernel_input);

            // Both should agree on accept/reject
            assert_eq!(
                dsl_result.is_ok(),
                kernel_result.is_ok(),
                "DSL and kernel disagree on {variant_name}: dsl={}, kernel={}",
                dsl_result.is_ok(),
                kernel_result.is_ok()
            );

            if let (Ok(dsl_out), Ok(kernel_out)) = (&dsl_result, &kernel_result) {
                // Same target phase
                assert_eq!(
                    format!("{:?}", dsl_out.to_phase),
                    kernel_out.next_state.phase.as_str(),
                    "Phase mismatch on {variant_name}"
                );

                // Same number of effects
                assert_eq!(
                    dsl_out.effects.len(),
                    kernel_out.effects.len(),
                    "Effect count mismatch on {variant_name}"
                );

                // Advance both states
                kernel_state = kernel_out.next_state.clone();
            }
        }
    }
}

// ============================================================
// Traffic Light: minimal stored-phase machine
// ============================================================

mod traffic_light {
    use super::*;

    machine! {
        machine TrafficLight {
            version: 1,
            rust: "test" / "traffic_light",

            state {
                phase: TrafficPhase,
            }

            init(Green) {}

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
    }

    // ---- Runtime direction tests ----

    #[test]
    fn initial_state_is_green() {
        let auth = TrafficLightAuthority::new();
        assert_eq!(auth.state.phase(), TrafficPhase::Green);
    }

    #[test]
    fn toggle_green_to_red() {
        let mut auth = TrafficLightAuthority::new();
        let result = TrafficLightMutator::apply(&mut auth, TrafficInput::Toggle).unwrap();
        assert_eq!(result.from_phase, TrafficPhase::Green);
        assert_eq!(result.to_phase, TrafficPhase::Red);
        assert_eq!(result.effects.len(), 1);
        assert_eq!(result.effects[0], TrafficEffect::Switched);
    }

    #[test]
    fn toggle_red_to_green() {
        let mut auth = TrafficLightAuthority::new();
        TrafficLightMutator::apply(&mut auth, TrafficInput::Toggle).unwrap();
        let result = TrafficLightMutator::apply(&mut auth, TrafficInput::Toggle).unwrap();
        assert_eq!(result.from_phase, TrafficPhase::Red);
        assert_eq!(result.to_phase, TrafficPhase::Green);
    }

    #[test]
    fn round_trip_preserves_state() {
        let mut auth = TrafficLightAuthority::new();
        TrafficLightMutator::apply(&mut auth, TrafficInput::Toggle).unwrap();
        TrafficLightMutator::apply(&mut auth, TrafficInput::Toggle).unwrap();
        assert_eq!(auth.state.phase(), TrafficPhase::Green);
    }

    // ---- Schema direction tests ----

    #[test]
    fn schema_is_valid() {
        let schema = TrafficLightState::schema();
        schema.validate().expect("schema should be valid");
    }

    #[test]
    fn schema_has_correct_structure() {
        let schema = TrafficLightState::schema();
        assert_eq!(schema.machine.as_str(), "TrafficLight");
        assert_eq!(schema.version, 1);
        assert_eq!(schema.state.phase.variants.len(), 2);
        assert_eq!(schema.transitions.len(), 2);
        assert_eq!(schema.effects.variants.len(), 1);
    }
}

// ============================================================
// Counter: derived-phase machine
// ============================================================

mod counter {
    use super::*;

    machine! {
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
    }

    // ---- Runtime direction tests ----

    #[test]
    fn initial_state_is_stopped() {
        // active=false → phase projection returns Stopped
        let auth = CounterAuthority::new();
        assert_eq!(auth.state.phase(), CounterPhase::Stopped);
    }

    #[test]
    fn start_then_increment() {
        let mut auth = CounterAuthority::new();
        let r = CounterMutator::apply(&mut auth, CounterInput::Start).unwrap();
        assert_eq!(r.to_phase, CounterPhase::Idle);
        assert_eq!(r.effects, vec![CounterEffect::Started]);

        let r = CounterMutator::apply(&mut auth, CounterInput::Increment { amount: 3 }).unwrap();
        assert_eq!(r.to_phase, CounterPhase::Counting);
        assert_eq!(auth.state.value, 3);
    }

    #[test]
    fn increment_to_limit_emits_effect() {
        let mut auth = CounterAuthority::new();
        CounterMutator::apply(&mut auth, CounterInput::Start).unwrap();
        let r = CounterMutator::apply(&mut auth, CounterInput::Increment { amount: 15 }).unwrap();
        assert_eq!(r.to_phase, CounterPhase::AtLimit);
        assert_eq!(auth.state.value, 10); // clamped to limit
        assert_eq!(
            r.effects,
            vec![CounterEffect::LimitReached {
                value: 10,
                limit: 10
            }]
        );
    }

    #[test]
    fn stop_emits_final_value() {
        let mut auth = CounterAuthority::new();
        CounterMutator::apply(&mut auth, CounterInput::Start).unwrap();
        CounterMutator::apply(&mut auth, CounterInput::Increment { amount: 5 }).unwrap();
        let r = CounterMutator::apply(&mut auth, CounterInput::Stop).unwrap();
        assert_eq!(
            r.effects,
            vec![CounterEffect::CounterStopped { final_value: 5 }]
        );
        assert_eq!(r.to_phase, CounterPhase::Stopped);
    }

    #[test]
    fn cannot_increment_when_stopped() {
        let auth = CounterAuthority::new();
        assert_eq!(auth.state.phase(), CounterPhase::Stopped);
        let mut auth = auth;
        let r = CounterMutator::apply(&mut auth, CounterInput::Increment { amount: 1 });
        assert!(r.is_err());
    }

    // ---- Schema direction tests ----

    #[test]
    fn schema_is_valid() {
        let schema = CounterState::schema();
        schema.validate().expect("counter schema should be valid");
    }

    #[test]
    fn schema_has_correct_structure() {
        let schema = CounterState::schema();
        assert_eq!(schema.machine.as_str(), "Counter");
        assert_eq!(schema.state.phase.variants.len(), 4);
        assert_eq!(schema.state.fields.len(), 3);
        assert_eq!(schema.transitions.len(), 5);
    }

    #[test]
    fn schema_derived_from_phases() {
        let schema = CounterState::schema();

        // StartIdle guards on !self.active → phase projection: Stopped is when !active
        // So this transition fires from Stopped (active=false)
        let start = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "StartIdle")
            .unwrap();
        fn from_strs(t: &meerkat_machine_schema::TransitionSchema) -> Vec<&str> {
            t.from.iter().map(|p| p.as_str()).collect()
        }
        assert!(
            from_strs(start).contains(&"Stopped"),
            "StartIdle should be from Stopped, got {:?}",
            start.from
        );
        // Should NOT include Counting or AtLimit (those require active=true)
        assert!(!from_strs(start).contains(&"Counting"));
        assert!(!from_strs(start).contains(&"AtLimit"));

        // StopActive guards on self.active → fires from Idle, Counting, AtLimit (all active=true)
        let stop = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "StopActive")
            .unwrap();
        assert!(
            !from_strs(stop).contains(&"Stopped"),
            "StopActive should not be from Stopped"
        );
        // Should include at least Idle and Counting
        assert!(
            from_strs(stop).contains(&"Idle") || from_strs(stop).contains(&"Counting"),
            "StopActive should fire from active phases, got {:?}",
            stop.from
        );
    }
}

// ============================================================
// Order Lifecycle: convoluted stored-phase machine exercising
// the full DSL vocabulary (sets, maps, helpers, invariants,
// conditional updates, quantifiers, signals, multiple effects)
// ============================================================

mod order_lifecycle {
    use super::*;

    machine! {
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
                Submit,
                Assign { assignee: String },
                Pay { at_utc_ms: u64, receipt: String },
                Complete { note: String },
                Cancel { reason: String },
                Tag { tag: String },
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

            // --- Tagging (draft only, self-loop) ---

            transition TagDraft {
                on input Tag { tag }
                guard { self.lifecycle_phase == Phase::Draft && !self.tags.contains(tag) }
                update {
                    self.tags.insert(tag);
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

            // --- Metadata (draft only) ---

            transition SetMetaDraft {
                on input SetMeta { key, value }
                guard { self.lifecycle_phase == Phase::Draft }
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
    }

    // ---- Runtime direction tests ----

    #[test]
    fn initial_state_is_draft() {
        let auth = OrderLifecycleAuthority::new();
        assert_eq!(auth.state.phase(), OrderPhase::Draft);
        assert_eq!(auth.state.item_count, 0);
        assert_eq!(auth.state.tags.len(), 0);
        assert_eq!(auth.state.metadata.len(), 0);
    }

    #[test]
    fn add_items_and_submit() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 100 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 200 }).unwrap();
        assert_eq!(auth.state.item_count, 2);
        assert_eq!(auth.state.total_price, 300);

        let r = OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();
        assert_eq!(r.to_phase, OrderPhase::Submitted);
        assert_eq!(r.effects.len(), 1);
        match &r.effects[0] {
            OrderEffect::OrderSubmitted { order_id } => assert_eq!(order_id, "order-0"),
            other => panic!("unexpected effect: {other:?}"),
        }
    }

    #[test]
    fn cannot_submit_empty_order() {
        let mut auth = OrderLifecycleAuthority::new();
        let r = OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit);
        assert!(r.is_err(), "should not submit order with 0 items");
    }

    #[test]
    fn full_lifecycle_draft_to_completed() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 50 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Assign {
                assignee: "alice".into(),
            },
        )
        .unwrap();
        assert_eq!(auth.state.assigned_to, Some("alice".into()));

        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Pay {
                at_utc_ms: 1000,
                receipt: "rcpt-1".into(),
            },
        )
        .unwrap();
        assert_eq!(auth.state.paid_at, Some(1000));
        assert_eq!(
            auth.state.metadata.get("receipt"),
            Some(&"rcpt-1".to_string())
        );

        let r = OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Complete {
                note: "done".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OrderPhase::Completed);
        assert_eq!(
            auth.state.metadata.get("completion_note"),
            Some(&"done".to_string())
        );
    }

    #[test]
    fn cancel_clears_assignment_and_payment() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 50 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Assign {
                assignee: "bob".into(),
            },
        )
        .unwrap();

        let r = OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Cancel {
                reason: "changed mind".into(),
            },
        )
        .unwrap();
        assert_eq!(r.to_phase, OrderPhase::Cancelled);
        assert_eq!(auth.state.assigned_to, None);
        assert_eq!(auth.state.paid_at, None);
        assert_eq!(auth.state.failure_reason, Some("changed mind".into()));
    }

    #[test]
    fn tag_operations() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Tag {
                tag: "urgent".into(),
            },
        )
        .unwrap();
        assert!(auth.state.tags.contains("urgent"));

        // Can't add duplicate
        let r = OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Tag {
                tag: "urgent".into(),
            },
        );
        assert!(r.is_err());
    }

    #[test]
    fn retry_increments_and_caps_at_3() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 10 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();
        assert_eq!(auth.state.attempt_count, 1);

        OrderLifecycleMutator::apply(&mut auth, OrderInput::Retry).unwrap();
        assert_eq!(auth.state.attempt_count, 2);

        OrderLifecycleMutator::apply(&mut auth, OrderInput::Retry).unwrap();
        assert_eq!(auth.state.attempt_count, 3);

        // At limit — should fail
        let r = OrderLifecycleMutator::apply(&mut auth, OrderInput::Retry);
        assert!(r.is_err());
    }

    #[test]
    fn map_metadata_operations() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::SetMeta {
                key: "priority".into(),
                value: "high".into(),
            },
        )
        .unwrap();
        assert_eq!(
            auth.state.metadata.get("priority"),
            Some(&"high".to_string())
        );
    }

    #[test]
    fn signal_validation_failed() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 10 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();

        let r = auth
            .apply_signal(OrderSignal::ExternalValidation { valid: false })
            .unwrap();
        assert_eq!(r.to_phase, OrderPhase::Cancelled);
        assert_eq!(auth.state.failure_reason, Some("validation_failed".into()));
    }

    #[test]
    fn cannot_act_on_terminal_state() {
        let mut auth = OrderLifecycleAuthority::new();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 10 }).unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Submit).unwrap();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Assign {
                assignee: "x".into(),
            },
        )
        .unwrap();
        OrderLifecycleMutator::apply(
            &mut auth,
            OrderInput::Pay {
                at_utc_ms: 1,
                receipt: "r".into(),
            },
        )
        .unwrap();
        OrderLifecycleMutator::apply(&mut auth, OrderInput::Complete { note: "n".into() }).unwrap();

        // Now in Completed — nothing should work
        assert!(OrderLifecycleMutator::apply(&mut auth, OrderInput::AddItem { price: 1 }).is_err());
        assert!(
            OrderLifecycleMutator::apply(&mut auth, OrderInput::Cancel { reason: "x".into() })
                .is_err()
        );
    }

    // ---- Schema direction tests ----

    #[test]
    fn schema_is_valid() {
        // Order-lifecycle is an in-module test fixture (not catalogued in
        // `meerkat-machine-schema/src/catalog/dsl/mod.rs`). Its only
        // named-type reference is the `OrderPhase` enum it declares
        // locally. B-4 (`c0cb12071`) made `named_types` validation-gated;
        // populate inline.
        use meerkat_machine_schema::identity::NamedTypeBinding;
        let mut schema = OrderLifecycleState::schema();
        schema.named_types = vec![NamedTypeBinding::string("OrderPhase")];
        schema
            .validate()
            .expect("order lifecycle schema should be valid");
    }

    #[test]
    fn schema_has_correct_structure() {
        let schema = OrderLifecycleState::schema();
        assert_eq!(schema.machine.as_str(), "OrderLifecycle");
        assert_eq!(schema.version, 1);
        assert_eq!(schema.state.phase.variants.len(), 6);
        assert_eq!(schema.state.fields.len(), 9); // lifecycle_phase excluded from schema
        assert_eq!(schema.inputs.variants.len(), 9);
        assert_eq!(schema.signals.variants.len(), 1);
        assert_eq!(schema.effects.variants.len(), 6);
        assert_eq!(schema.transitions.len(), 10);
        assert_eq!(schema.helpers.len(), 1);
        assert_eq!(schema.invariants.len(), 3);
        assert_eq!(schema.effect_dispositions.len(), 6);
    }

    #[test]
    fn schema_from_phases_are_derived() {
        let schema = OrderLifecycleState::schema();

        // AddItemDraft guards on lifecycle_phase == Draft
        let add_item = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "AddItemDraft")
            .unwrap();
        assert_eq!(
            add_item.from.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["Draft"]
        );

        // SubmitDraft guards on lifecycle_phase == Draft
        let submit = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "SubmitDraft")
            .unwrap();
        assert_eq!(
            submit.from.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["Draft"]
        );

        // AssignSubmitted guards on lifecycle_phase == Submitted
        let assign = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "AssignSubmitted")
            .unwrap();
        assert_eq!(
            assign.from.iter().map(|p| p.as_str()).collect::<Vec<_>>(),
            vec!["Submitted"]
        );

        // CancelActive guards on is_active_phase(lifecycle_phase) — expands to Draft, Submitted, Assigned, Paid
        let cancel = schema
            .transitions
            .iter()
            .find(|t| t.name.as_str() == "CancelActive")
            .unwrap();
        assert_eq!(cancel.from.len(), 4);
        let cancel_strs: Vec<&str> = cancel.from.iter().map(|p| p.as_str()).collect();
        assert!(cancel_strs.contains(&"Draft"));
        assert!(cancel_strs.contains(&"Submitted"));
        assert!(cancel_strs.contains(&"Assigned"));
        assert!(cancel_strs.contains(&"Paid"));
    }
}

// ============================================================
// Aggregate schema equivalence gate: REMOVED
// ============================================================
//
// The `all_dsl_schemas_match_hand_written_catalog` test was the
// migration gate for replacing the hand-written catalog with DSL-
// generated schemas. That migration completed in commit c74b7ca42
// ("Delete old hand-written machine catalog: DSL is now sole source
// of truth"). Both `canonical_machine_schemas()` and
// `dsl_machine_schemas()` now read DSL sources — the first from
// `meerkat-machine-schema/src/catalog/dsl/` (production) and the
// second from `test-fixtures/machine-dsl-tests/src/` (simplified
// integration fixture). They are intentionally divergent: the fixture
// DSL is a minimal surrogate for exercising the `machine!` macro
// codegen path without carrying the full production surface.
//
// Keeping the aggregate equivalence assertion here would require
// mirroring every production DSL extension into the fixture, which
// defeats the fixture's purpose. The per-machine `schema_matches_
// hand_written` drift tests were already deleted in that commit; this
// aggregate was an oversight cleanup that lands here.
