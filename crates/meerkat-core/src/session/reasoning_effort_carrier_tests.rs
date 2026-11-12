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
            top_level: Some(High),
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
        // The refusal says that the field cleared the level: it carries no
        // top-level level, and the level the params name.
        assert_eq!(
            refused,
            SessionLlmIdentityOverrideError::ReasoningEffortSpellingsDisagree {
                top_level: None,
                nested: nested_level,
            },
            "{current:?} / {nested_level:?}"
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

/// A build folds a request's own `reasoning_effort` parameter into its
/// params by the one rule, and a resumed build that claims the level decides
/// the session's level with the result: the level it holds, or none.
#[test]
fn build_provider_params_fold_the_level_by_the_one_rule() {
    use super::{resolve_build_provider_params, resolve_resumed_session_llm_identity};
    use TurnMetadataOverride::{Clear, Set};

    let plain = params(Some(0.7), None);
    let with_high = params(Some(0.7), Some(EffortLevel::High));
    for (request_params, field, folded) in [
        (None, None, None),
        (
            None,
            Some(Set(EffortLevel::High)),
            Some(params(None, Some(EffortLevel::High))),
        ),
        (
            Some(&plain),
            Some(Set(EffortLevel::High)),
            Some(with_high.clone()),
        ),
        // Both spellings, equal.
        (
            Some(&with_high),
            Some(Set(EffortLevel::High)),
            Some(with_high.clone()),
        ),
        // The nested key alone is the same setting.
        (Some(&with_high), None, Some(with_high.clone())),
        (Some(&plain), None, Some(plain.clone())),
        (None, Some(Clear), None),
        (Some(&plain), Some(Clear), Some(plain.clone())),
    ] {
        assert_eq!(
            resolve_build_provider_params(request_params, field.clone()).unwrap(),
            folded,
            "{request_params:?} / {field:?}"
        );
    }

    // On a resumed build: the durable session holds `medium` beside a
    // temperature. A request that names the level claims it.
    let durable = identity(Some(params(Some(0.2), Some(EffortLevel::Medium))));
    let resumed = |request_params: Option<&ProviderParamsOverride>,
                   field: Option<TurnMetadataOverride<EffortLevel>>,
                   claims_params: bool| {
        let mask = crate::service::ResumeOverrideMask {
            provider_params: claims_params,
            reasoning_effort: field.is_some(),
            ..Default::default()
        };
        let candidate = identity(resolve_build_provider_params(request_params, field).unwrap());
        resolve_resumed_session_llm_identity(candidate, &durable, &mask).provider_params
    };
    // A level replaces the durable one and keeps the other durable params.
    assert_eq!(
        resumed(None, Some(Set(EffortLevel::High)), false),
        Some(params(Some(0.2), Some(EffortLevel::High)))
    );
    // The explicit clear removes it.
    assert_eq!(
        resumed(None, Some(Clear), false),
        Some(params(Some(0.2), None))
    );
    // A resume that names neither keeps it.
    assert_eq!(
        resumed(None, None, false),
        Some(params(Some(0.2), Some(EffortLevel::Medium)))
    );
    // A resume that replaces the params without the key keeps it.
    assert_eq!(
        resumed(Some(&plain), None, true),
        Some(params(Some(0.7), Some(EffortLevel::Medium)))
    );
    // A resume that replaces the params and clears the level removes it.
    assert_eq!(
        resumed(Some(&plain), Some(Clear), true),
        Some(plain.clone())
    );
    // A clear on a session whose params hold nothing else leaves no params.
    let only_level = identity(Some(params(None, Some(EffortLevel::Medium))));
    let mask = crate::service::ResumeOverrideMask {
        reasoning_effort: true,
        ..Default::default()
    };
    assert_eq!(
        resolve_resumed_session_llm_identity(
            identity(resolve_build_provider_params(None, Some(Clear)).unwrap()),
            &only_level,
            &mask
        )
        .provider_params,
        None
    );
}

/// A request that names the level twice, as its own parameter and as the
/// nested key of its params, must ask for one outcome.
#[test]
fn a_build_that_names_the_level_twice_must_agree() {
    use super::resolve_build_provider_params;
    use TurnMetadataOverride::{Clear, Set};

    let nested_low = params(Some(0.7), Some(EffortLevel::Low));
    let refused = resolve_build_provider_params(Some(&nested_low), Some(Set(EffortLevel::High)))
        .expect_err("two different levels are refused");
    assert_eq!(
        refused,
        SessionLlmIdentityOverrideError::ReasoningEffortSpellingsDisagree {
            top_level: Some(EffortLevel::High),
            nested: EffortLevel::Low,
        }
    );
    assert_eq!(
        refused.to_string(),
        "reasoning_effort is 'high' but provider_params.reasoning_effort is 'low': set one of \
         them, or the same level in both"
    );
    let refused = resolve_build_provider_params(Some(&nested_low), Some(Clear))
        .expect_err("a clear beside a nested level is refused");
    assert_eq!(
        refused,
        SessionLlmIdentityOverrideError::ReasoningEffortSpellingsDisagree {
            top_level: None,
            nested: EffortLevel::Low,
        }
    );
}
