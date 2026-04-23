// Every AuthMachine phase transition must emit `EmitLifecycleEvent` so external
// observers — the auth-lease registry, external event bus, etc. — never miss a
// lifecycle change, including the terminal move to `Released`.

use meerkat_machine_schema::catalog::dsl::dsl_auth_machine;
use meerkat_machine_schema::identity::FieldId;
use meerkat_machine_schema::{EffectEmit, Expr, TransitionSchema, TriggerMatch};

fn release_transition() -> TransitionSchema {
    let schema = dsl_auth_machine();
    schema
        .transitions
        .into_iter()
        .find(|t| t.name.as_str() == "Release")
        .expect("AuthMachine must declare a Release transition")
}

#[test]
fn release_transition_is_valid_on_input_release() {
    let release = release_transition();
    match release.on {
        TriggerMatch::Input { variant, .. } => {
            assert_eq!(variant.as_str(), "Release");
        }
        TriggerMatch::Signal { .. } => {
            panic!("Release must fire on an input, not a signal");
        }
    }
    assert_eq!(release.to.as_str(), "Released");
}

#[test]
fn release_transition_emits_lifecycle_event_with_lifecycle_phase() {
    let release = release_transition();

    assert_eq!(
        release.emit.len(),
        1,
        "Release must emit exactly one effect (EmitLifecycleEvent); got {:?}",
        release.emit
    );

    let EffectEmit { variant, fields } = &release.emit[0];
    assert_eq!(
        variant.as_str(),
        "EmitLifecycleEvent",
        "Release must emit EmitLifecycleEvent so the terminal phase is observable",
    );

    assert_eq!(
        fields.len(),
        1,
        "EmitLifecycleEvent carries exactly one field (new_state); got {fields:?}",
    );
    let (new_state_field, new_state_expr): (&FieldId, &Expr) = fields
        .iter()
        .next()
        .expect("EmitLifecycleEvent field list non-empty by prior assertion");
    assert_eq!(new_state_field.as_str(), "new_state");

    // The DSL macro rewrites `self.lifecycle_phase` (the declared stored-phase
    // field) as `Expr::CurrentPhase`, so the canonical shape for every other
    // AuthMachine transition's `new_state` lands on this variant.
    assert!(
        matches!(new_state_expr, Expr::CurrentPhase),
        "EmitLifecycleEvent.new_state must be Expr::CurrentPhase (matching every \
         other AuthMachine transition); got {new_state_expr:?}",
    );
}

#[test]
fn every_auth_machine_transition_emits_lifecycle_event() {
    let schema = dsl_auth_machine();
    for transition in &schema.transitions {
        let emits_lifecycle = transition
            .emit
            .iter()
            .any(|e| e.variant.as_str() == "EmitLifecycleEvent");
        assert!(
            emits_lifecycle,
            "AuthMachine transition {:?} must emit EmitLifecycleEvent so the \
             phase change is externally observable; emit list: {:?}",
            transition.name, transition.emit,
        );
    }
}
