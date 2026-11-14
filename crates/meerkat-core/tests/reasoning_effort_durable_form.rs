#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Durable forms of the provider-neutral reasoning effort level.
//!
//! The level is stored inside `provider_params` (session identity, session
//! metadata) and, for a turn, beside it on the pending input's turn
//! metadata. These rows read and write those forms as JSON only, so they
//! state what a stored record looks like:
//!
//! - a record written without the level reads back and re-serializes to the
//!   same bytes;
//! - a record that carries the level reads back with it. A release without
//!   the field refuses such a record, because these structs deny unknown
//!   fields: that is the downgrade boundary.

use meerkat_core::SessionLlmIdentity;
use meerkat_core::lifecycle::run_primitive::{ProviderParamsOverride, RuntimeTurnMetadata};

fn reserialize<T>(stored: &str) -> String
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let parsed: T = serde_json::from_str(stored)
        .unwrap_or_else(|error| panic!("stored record must read: {error}\n{stored}"));
    serde_json::to_string(&parsed).expect("a parsed record serializes")
}

/// Records written before the level existed read unchanged: parsing and
/// writing them again gives the same bytes.
#[test]
fn stored_forms_without_reasoning_effort_reserialize_byte_identically() {
    let params = r#"{"temperature":0.2,"max_output_tokens":4096,"provider_tag":{"provider":"open_ai","reasoning_effort":"high"}}"#;
    assert_eq!(reserialize::<ProviderParamsOverride>(params), params);

    let identity = r#"{"model":"test-openai-default","provider":"openai","provider_params":{"temperature":0.2,"provider_tag":{"provider":"anthropic","effort":"x_high"}}}"#;
    assert_eq!(reserialize::<SessionLlmIdentity>(identity), identity);

    let bare_identity = r#"{"model":"test-openai-default","provider":"openai"}"#;
    assert_eq!(
        reserialize::<SessionLlmIdentity>(bare_identity),
        bare_identity
    );

    let turn = r#"{"model":"test-openai-default","provider_params":{"action":"set","value":{"temperature":0.2}},"auth_binding":{"action":"clear"}}"#;
    assert_eq!(reserialize::<RuntimeTurnMetadata>(turn), turn);
}

/// Records that carry the level read back with it and keep it when written
/// again. A release without the field refuses each of them with serde's
/// unknown-field error.
#[test]
fn stored_forms_that_carry_reasoning_effort_read_and_keep_it() {
    let params = r#"{"temperature":0.2,"reasoning_effort":"xhigh"}"#;
    assert_eq!(reserialize::<ProviderParamsOverride>(params), params);

    let identity = r#"{"model":"test-openai-default","provider":"openai","provider_params":{"reasoning_effort":"high"}}"#;
    assert_eq!(reserialize::<SessionLlmIdentity>(identity), identity);

    let turn_set = r#"{"reasoning_effort":{"action":"set","value":"low"}}"#;
    assert_eq!(reserialize::<RuntimeTurnMetadata>(turn_set), turn_set);

    let turn_clear = r#"{"reasoning_effort":{"action":"clear"}}"#;
    assert_eq!(reserialize::<RuntimeTurnMetadata>(turn_clear), turn_clear);

    // A bare level is the set form on input, like a bare params object.
    assert_eq!(
        reserialize::<RuntimeTurnMetadata>(r#"{"reasoning_effort":"low"}"#),
        turn_set
    );
}

/// The level has one spelling per value. `x_high` (a spelling the
/// Claude-specific effort setting also reads) is not a level, and the error
/// lists the levels.
#[test]
fn reasoning_effort_refuses_x_high_and_lists_the_levels() {
    for stored in [
        r#"{"reasoning_effort":"x_high"}"#,
        r#"{"reasoning_effort":"XHIGH"}"#,
        r#"{"reasoning_effort":"highest"}"#,
    ] {
        let error = serde_json::from_str::<ProviderParamsOverride>(stored)
            .expect_err("a word outside the level set is refused")
            .to_string();
        for level in ["none", "minimal", "low", "medium", "high", "xhigh", "max"] {
            assert!(
                error.contains(&format!("`{level}`")),
                "the refusal lists `{level}`: {error}"
            );
        }
    }
    let error = serde_json::from_str::<RuntimeTurnMetadata>(r#"{"reasoning_effort":"x_high"}"#)
        .expect_err("a turn refuses the word too")
        .to_string();
    assert!(error.contains("`xhigh`"), "{error}");
}
