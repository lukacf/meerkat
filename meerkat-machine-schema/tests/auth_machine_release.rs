// Every AuthMachine phase transition must emit `EmitLifecycleEvent` so external
// observers — the auth-lease registry, external event bus, etc. — never miss a
// lifecycle change, including the terminal move to `Released`.

use meerkat_machine_schema::catalog::dsl::dsl_auth_machine;
use meerkat_machine_schema::identity::FieldId;
use meerkat_machine_schema::{EffectEmit, Expr, TransitionSchema, TriggerMatch};

fn release_transition() -> Result<TransitionSchema, String> {
    let schema = dsl_auth_machine();
    schema
        .transitions
        .into_iter()
        .find(|t| t.name.as_str() == "Release")
        .ok_or_else(|| "AuthMachine must declare a Release transition".to_string())
}

#[test]
fn release_transition_is_valid_on_input_release() -> Result<(), String> {
    let release = release_transition()?;
    if let TriggerMatch::Input { variant, .. } = release.on {
        assert_eq!(variant.as_str(), "Release");
    } else {
        return Err("Release must fire on an input, not a signal".to_string());
    }
    assert_eq!(release.to.as_str(), "Released");
    Ok(())
}

#[test]
fn release_transition_emits_lifecycle_event_with_lifecycle_phase() -> Result<(), String> {
    let release = release_transition()?;

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
    let Some((new_state_field, new_state_expr)) = fields.iter().next() else {
        return Err("EmitLifecycleEvent field list non-empty by prior assertion".to_string());
    };
    let (new_state_field, new_state_expr): (&FieldId, &Expr) = (new_state_field, new_state_expr);
    assert_eq!(new_state_field.as_str(), "new_state");

    // The DSL macro rewrites `self.lifecycle_phase` (the declared stored-phase
    // field) as `Expr::CurrentPhase`, so the canonical shape for every other
    // AuthMachine transition's `new_state` lands on this variant.
    assert!(
        matches!(new_state_expr, Expr::CurrentPhase),
        "EmitLifecycleEvent.new_state must be Expr::CurrentPhase (matching every \
         other AuthMachine transition); got {new_state_expr:?}",
    );
    Ok(())
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
