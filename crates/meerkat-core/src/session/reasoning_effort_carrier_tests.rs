//! Rows for the provider-neutral reasoning effort level on the session's
//! LLM identity: the stored form, and the field-level turn rule owned by
//! [`resolve_session_provider_params`] as the live identity override applies
//! it.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::{
    SessionLlmIdentity, SessionLlmIdentityOverride, SessionLlmIdentityOverrideError,
    resolve_session_llm_identity_override, resolve_session_provider_params,
};
use crate::Provider;
use crate::lifecycle::run_primitive::{ProviderParamsOverride, TurnMetadataOverride};
use crate::model_profile::capabilities::EffortLevel;

fn registry() -> crate::ModelRegistry {
    crate::ModelRegistry::from_config(
        &crate::Config::default(),
        *crate::model_profile::test_catalog::TEST_CATALOG,
    )
    .unwrap()
}

fn params(temperature: Option<f32>, level: Option<EffortLevel>) -> ProviderParamsOverride {
    ProviderParamsOverride {
        temperature,
        reasoning_effort: level,
        ..Default::default()
    }
}

fn identity(provider_params: Option<ProviderParamsOverride>) -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: "test-openai-default".to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params,
        auth_binding: None,
    }
}

/// One turn against `current`: the params tri-state and the level tri-state,
/// through the live identity override.
fn turn(
    current: Option<ProviderParamsOverride>,
    params_override: Option<TurnMetadataOverride<&ProviderParamsOverride>>,
    reasoning_effort: Option<TurnMetadataOverride<EffortLevel>>,
) -> Result<Option<ProviderParamsOverride>, SessionLlmIdentityOverrideError> {
    resolve_session_llm_identity_override(
        &identity(current),
        &registry(),
        SessionLlmIdentityOverride {
            model: None,
            provider: None,
            self_hosted_server_id: None,
            provider_params: params_override,
            reasoning_effort,
            auth_binding: None,
        },
    )
    .map(|resolved| resolved.provider_params)
}

#[test]
fn reasoning_effort_round_trips_through_params_identity_and_wire() {
    let with_level = identity(Some(params(None, Some(EffortLevel::Xhigh))));
    let json = serde_json::to_value(&with_level).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "model": "test-openai-default",
            "provider": "openai",
            "provider_params": {"reasoning_effort": "xhigh"},
        })
    );
    let read: SessionLlmIdentity = serde_json::from_value(json).unwrap();
    assert_eq!(read, with_level);

    // An identity stored without the level reads as no level and writes the
    // same JSON again.
    let stored = serde_json::json!({
        "model": "test-openai-default",
        "provider": "openai",
        "provider_params": {"temperature": 0.5},
    });
    let read: SessionLlmIdentity = serde_json::from_value(stored.clone()).unwrap();
    assert_eq!(read.provider_params, Some(params(Some(0.5), None)));
    assert_eq!(serde_json::to_value(&read).unwrap(), stored);

    let error = serde_json::from_value::<SessionLlmIdentity>(serde_json::json!({
        "model": "test-openai-default",
        "provider": "openai",
        "provider_params": {"reasoning_effort": "x_high"},
    }))
    .expect_err("`x_high` is not a level")
    .to_string();
    assert!(error.contains("`xhigh`"), "{error}");
}

#[test]
fn turn_reasoning_effort_is_field_level_and_sticky() {
    use EffortLevel::{High, Low, Medium};
    use TurnMetadataOverride::{Clear, Set};

    // 1. Set: the turn field becomes the session's level, creating a params
    //    value that holds only the level when the session had none.
    assert_eq!(
        turn(None, None, Some(Set(High))).unwrap(),
        Some(params(None, Some(High)))
    );
    assert_eq!(
        turn(Some(params(Some(0.2), Some(Low))), None, Some(Set(High))).unwrap(),
        Some(params(Some(0.2), Some(High)))
    );

    // 2. Clear: the level goes, the other params stay; a params value that
    //    held only the level is dropped.
    assert_eq!(
        turn(Some(params(Some(0.2), Some(High))), None, Some(Clear)).unwrap(),
        Some(params(Some(0.2), None))
    );
    assert_eq!(
        turn(Some(params(None, Some(High))), None, Some(Clear)).unwrap(),
        None
    );

    // 3. Inherit: a turn that names neither keeps the level.
    assert_eq!(
        turn(Some(params(Some(0.2), Some(Medium))), None, None).unwrap(),
        Some(params(Some(0.2), Some(Medium)))
    );

    // 4. `provider_params` set without the key keeps the level.
    let replacement = params(Some(0.7), None);
    assert_eq!(
        turn(
            Some(params(Some(0.2), Some(Medium))),
            Some(Set(&replacement)),
            None
        )
        .unwrap(),
        Some(params(Some(0.7), Some(Medium)))
    );

    // 5. `provider_params` clear keeps the level.
    assert_eq!(
        turn(Some(params(Some(0.2), Some(Medium))), Some(Clear), None).unwrap(),
        Some(params(None, Some(Medium)))
    );

    // 6. The nested key alone sets the level.
    let nested = params(None, Some(Low));
    assert_eq!(
        turn(
            Some(params(Some(0.2), Some(High))),
            Some(Set(&nested)),
            None
        )
        .unwrap(),
        Some(params(None, Some(Low)))
    );

    // 7. Both spellings with the same level are accepted.
    let nested = params(Some(0.7), Some(High));
    assert_eq!(
        turn(None, Some(Set(&nested)), Some(Set(High))).unwrap(),
        Some(params(Some(0.7), Some(High)))
    );

    // 8. Both spellings with different levels are refused, typed.
    let nested = params(None, Some(Low));
    assert_eq!(
        turn(None, Some(Set(&nested)), Some(Set(High))).unwrap_err(),
        SessionLlmIdentityOverrideError::ReasoningEffortSpellingsDisagree {
            top_level: High,
            nested: Low,
        }
    );

    // 9. A params value that holds only the level survives a turn that
    //    changes something else, and survives the stored form.
    let resolved = resolve_session_llm_identity_override(
        &identity(Some(params(None, Some(High)))),
        &registry(),
        SessionLlmIdentityOverride {
            model: Some("test-openai-other"),
            provider: None,
            self_hosted_server_id: None,
            provider_params: None,
            reasoning_effort: None,
            auth_binding: None,
        },
    )
    .unwrap();
    assert_eq!(resolved.provider_params, Some(params(None, Some(High))));
    let stored = serde_json::to_string(&resolved).unwrap();
    let read: SessionLlmIdentity = serde_json::from_str(&stored).unwrap();
    assert_eq!(read.provider_params, Some(params(None, Some(High))));
}

/// A turn that clears the field while its new params name a level asks for
/// two outcomes at once. It is refused like two different levels, whatever
/// level the session holds and whatever level the params name, and nothing
/// of the turn applies.
#[test]
fn a_cleared_turn_level_with_a_nested_level_is_refused() {
    use EffortLevel::{High, Low};
    use TurnMetadataOverride::{Clear, Set};

    for (current, nested_level) in [
        (Some(params(None, Some(High))), Low),
        // The nested level is the session's own.
        (Some(params(Some(0.2), Some(High))), High),
        // The session holds no level.
        (None, Low),
        // `none` is a level, not the absence of one.
        (Some(params(None, Some(High))), EffortLevel::None),
    ] {
        let nested = params(Some(0.7), Some(nested_level));
        let refused = turn(current.clone(), Some(Set(&nested)), Some(Clear))
            .expect_err("a clear beside a nested level is refused");
        assert!(
            matches!(
                &refused,
                SessionLlmIdentityOverrideError::ReasoningEffortSpellingsDisagree {
                    nested: named,
                    ..
                } if *named == nested_level
            ),
            "{current:?} / {nested_level:?}: {refused:?}"
        );
        let text = refused.to_string();
        assert!(
            text.contains("reasoning_effort is cleared")
                && text.contains(&format!(
                    "provider_params.reasoning_effort is '{}'",
                    nested_level.as_wire_str()
                )),
            "{text}"
        );
    }

    // A clear beside new params that name no level is still a clear, and so
    // is a clear beside cleared params.
    let replacement = params(Some(0.7), None);
    assert_eq!(
        turn(
            Some(params(None, Some(High))),
            Some(Set(&replacement)),
            Some(Clear)
        )
        .unwrap(),
        Some(params(Some(0.7), None))
    );
    assert_eq!(
        turn(
            Some(params(Some(0.2), Some(High))),
            Some(Clear),
            Some(Clear)
        )
        .unwrap(),
        None
    );
}

/// With no level anywhere the rule is the params tri-state it always was,
/// an empty params value included.
#[test]
fn a_session_without_a_level_resolves_params_as_before() {
    use TurnMetadataOverride::{Clear, Set};
    let current = Some(params(Some(0.2), None));
    assert_eq!(turn(current.clone(), None, None).unwrap(), current);
    assert_eq!(turn(current.clone(), Some(Clear), None).unwrap(), None);
    let replacement = params(Some(0.7), None);
    assert_eq!(
        turn(current.clone(), Some(Set(&replacement)), None).unwrap(),
        Some(replacement)
    );
    let empty = ProviderParamsOverride::default();
    assert_eq!(
        turn(current, Some(Set(&empty)), None).unwrap(),
        Some(ProviderParamsOverride::default())
    );
    // Clearing a level the session never had changes nothing.
    assert_eq!(
        turn(Some(empty.clone()), None, Some(Clear)).unwrap(),
        Some(empty)
    );
}

/// The rule function itself, as the other seams call it.
#[test]
fn resolve_session_provider_params_is_the_rule_the_identity_override_applies() {
    use TurnMetadataOverride::{Clear, Set};
    let current = params(Some(0.2), Some(EffortLevel::Medium));
    let replacement = params(Some(0.7), None);
    for (params_override, level) in [
        (None, None),
        (None, Some(Set(EffortLevel::High))),
        (None, Some(Clear)),
        (Some(Set(&replacement)), None),
        (Some(Clear), None),
        (Some(Set(&replacement)), Some(Clear)),
        (Some(Clear), Some(Set(EffortLevel::Low))),
    ] {
        assert_eq!(
            resolve_session_provider_params(Some(&current), params_override.clone(), level.clone())
                .unwrap(),
            turn(
                Some(current.clone()),
                params_override.clone(),
                level.clone()
            )
            .unwrap(),
            "{params_override:?} / {level:?}"
        );
    }
}
