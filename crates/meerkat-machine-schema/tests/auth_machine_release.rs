// Every AuthMachine phase transition must emit `EmitLifecycleEvent` so external
// observers — the auth-lease registry, external event bus, etc. — never miss a
// lifecycle change, including the terminal move to `Released`.
#![allow(clippy::panic, clippy::semicolon_if_nothing_returned)]

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
fn credential_release_terminality_is_generated_from_oauth_membership() {
    let schema = dsl_auth_machine();
    for (transition_name, target_phase, guard_name) in [
        (
            "ReleaseCredentialLifecycleWithOAuth",
            "ReauthRequired",
            "oauth_membership_present",
        ),
        (
            "ReleaseCredentialLifecycleWithoutOAuth",
            "Released",
            "oauth_membership_absent",
        ),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == transition_name)
            .unwrap_or_else(|| panic!("AuthMachine must declare `{transition_name}`"));
        match &transition.on {
            TriggerMatch::Input { variant, .. } => {
                assert_eq!(variant.as_str(), "ReleaseCredentialLifecycle")
            }
            other => panic!("`{transition_name}` must be input-triggered, got {other:?}"),
        }
        assert_eq!(transition.to.as_str(), target_phase);
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == guard_name),
            "`{transition_name}` must carry generated guard `{guard_name}`"
        );
    }
}

#[test]
fn restore_snapshot_no_credential_terminality_is_generated_from_oauth_observation() {
    let schema = dsl_auth_machine();
    for (transition_name, target_phase, guard_name) in [
        (
            "RestoreCredentialLifecycleSnapshotNoCredentialWithOAuth",
            "ReauthRequired",
            "restore_oauth_membership_present",
        ),
        (
            "RestoreCredentialLifecycleSnapshotNoCredentialWithoutOAuth",
            "Released",
            "restore_oauth_membership_absent",
        ),
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == transition_name)
            .unwrap_or_else(|| panic!("AuthMachine must declare `{transition_name}`"));
        match &transition.on {
            TriggerMatch::Input { variant, .. } => {
                assert_eq!(variant.as_str(), "RestoreCredentialLifecycleSnapshot")
            }
            other => panic!("`{transition_name}` must be input-triggered, got {other:?}"),
        }
        assert_eq!(transition.to.as_str(), target_phase);
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == "restore_snapshot_has_no_credential"),
            "`{transition_name}` must identify the no-credential restore case in generated authority"
        );
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| guard.name == guard_name),
            "`{transition_name}` must carry generated guard `{guard_name}`"
        );
    }
}

#[test]
fn release_transition_emits_lifecycle_event_with_publication_facts() -> Result<(), String> {
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

    for required in [
        "new_state",
        "expires_at",
        "credential_generation",
        "credential_published_at_millis",
    ] {
        assert!(
            fields.iter().any(|(field, _)| field.as_str() == required),
            "EmitLifecycleEvent must carry `{required}` for generated auth lease publication; got {fields:?}",
        );
    }
    let Some((new_state_field, new_state_expr)) = fields
        .iter()
        .find(|(field, _)| field.as_str() == "new_state")
    else {
        return Err("EmitLifecycleEvent field list missing new_state".to_string());
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
fn every_auth_machine_lifecycle_event_carries_publication_facts() {
    let schema = dsl_auth_machine();
    for transition in &schema.transitions {
        let Some(lifecycle_event) = transition
            .emit
            .iter()
            .find(|e| e.variant.as_str() == "EmitLifecycleEvent")
        else {
            continue;
        };
        for required in [
            "new_state",
            "expires_at",
            "credential_generation",
            "credential_published_at_millis",
        ] {
            assert!(
                lifecycle_event
                    .fields
                    .iter()
                    .any(|(field, _)| field.as_str() == required),
                "AuthMachine transition {:?} lifecycle event must carry `{required}` for generated publication; fields: {:?}",
                transition.name,
                lifecycle_event.fields,
            );
        }
    }
}
