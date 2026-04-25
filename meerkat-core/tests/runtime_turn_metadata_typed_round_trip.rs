#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! B-6: typed round-trip + merge-conflict tests for `RuntimeTurnMetadata`.
//!
//! Every field round-trips through JSON losslessly and `merge()` refuses
//! distinct scalar overrides rather than silently last-wins.

use std::time::Duration;

use meerkat_core::connection::ConnectionRef;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicProviderTag, GeminiProviderTag, KeepAliveMode, KeepAlivePolicy, ModelId,
    OpenAiProviderTag, ProviderParamsOverride, ProviderTag, ReasoningEffort, ReasoningMode,
    RuntimeExecutionKind, RuntimeTurnMetadata, TurnInstruction, TurnInstructionKind,
    TurnMetadataMergeConflict,
};
use meerkat_core::provider::Provider;
use meerkat_core::service::TurnToolOverlay;
use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};
use meerkat_core::types::{HandlingMode, RenderClass, RenderMetadata, RenderSalience};

fn sample_metadata() -> RuntimeTurnMetadata {
    let skill_key = SkillKey {
        source_uuid: SourceUuid::from_uuid(uuid::Uuid::new_v4()),
        skill_name: SkillName::parse("demo-skill").expect("valid slug"),
    };
    RuntimeTurnMetadata {
        handling_mode: Some(HandlingMode::Steer),
        skill_references: Some(vec![skill_key]),
        flow_tool_overlay: Some(TurnToolOverlay::default()),
        additional_instructions: Some(vec![
            TurnInstruction {
                kind: TurnInstructionKind::User,
                body: "please be concise".into(),
            },
            TurnInstruction {
                kind: TurnInstructionKind::Host,
                body: "host-authored steering".into(),
            },
        ]),
        model: Some(ModelId::new("claude-opus-4-7")),
        provider: Some(Provider::Anthropic),
        provider_params: Some(ProviderParamsOverride {
            temperature: Some(0.2),
            top_p: Some(0.9),
            max_output_tokens: Some(1024),
            reasoning: Some(ReasoningMode::Emit),
            thinking_budget_tokens: Some(2048),
            provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                thinking_budget_tokens: Some(2048),
                ..Default::default()
            })),
        }),
        connection_ref: Some(ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::connection::BindingId::parse("default_anthropic")
                .expect("valid binding"),
            profile: None,
        }),
        keep_alive: Some(KeepAlivePolicy {
            ttl: Duration::from_secs(60),
            policy: KeepAliveMode::Pinned,
        }),
        render_metadata: Some(RenderMetadata {
            class: RenderClass::ExternalEvent,
            salience: RenderSalience::Urgent,
        }),
        execution_kind: Some(RuntimeExecutionKind::ContentTurn),
    }
}

#[test]
fn every_field_round_trips_through_json() {
    let meta = sample_metadata();
    let json = serde_json::to_value(&meta).expect("serialize");
    let parsed: RuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(meta, parsed, "typed round-trip must be lossless");
}

#[test]
fn provider_tag_openai_round_trips() {
    let meta = RuntimeTurnMetadata {
        provider: Some(Provider::OpenAI),
        provider_params: Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::OpenAi(OpenAiProviderTag {
                reasoning_effort: Some(ReasoningEffort::High),
                ..Default::default()
            })),
            ..Default::default()
        }),
        ..Default::default()
    };
    let json = serde_json::to_value(&meta).expect("serialize");
    let parsed: RuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(meta, parsed);
}

#[test]
fn provider_tag_gemini_round_trips() {
    let meta = RuntimeTurnMetadata {
        provider: Some(Provider::Gemini),
        provider_params: Some(ProviderParamsOverride {
            provider_tag: Some(ProviderTag::Gemini(GeminiProviderTag {
                candidate_count: Some(4),
                ..Default::default()
            })),
            ..Default::default()
        }),
        ..Default::default()
    };
    let json = serde_json::to_value(&meta).expect("serialize");
    let parsed: RuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(meta, parsed);
}

#[test]
fn empty_metadata_round_trips_without_allocating_fields() {
    let meta = RuntimeTurnMetadata::default();
    assert!(meta.is_empty());
    let json = serde_json::to_value(&meta).expect("serialize");
    assert_eq!(json, serde_json::json!({}), "empty metadata must emit {{}}");
    let parsed: RuntimeTurnMetadata = serde_json::from_value(json).expect("deserialize");
    assert_eq!(meta, parsed);
}

#[test]
fn merge_collection_fields_accumulate() {
    let mut left = RuntimeTurnMetadata {
        skill_references: Some(vec![SkillKey {
            source_uuid: SourceUuid::from_uuid(uuid::Uuid::new_v4()),
            skill_name: SkillName::parse("first").unwrap(),
        }]),
        additional_instructions: Some(vec![TurnInstruction {
            kind: TurnInstructionKind::User,
            body: "left".into(),
        }]),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        skill_references: Some(vec![SkillKey {
            source_uuid: SourceUuid::from_uuid(uuid::Uuid::new_v4()),
            skill_name: SkillName::parse("second").unwrap(),
        }]),
        additional_instructions: Some(vec![TurnInstruction {
            kind: TurnInstructionKind::Host,
            body: "right".into(),
        }]),
        ..Default::default()
    };
    left.merge(right).expect("collection merge never conflicts");
    assert_eq!(left.skill_references.as_ref().map(Vec::len), Some(2));
    assert_eq!(left.additional_instructions.as_ref().map(Vec::len), Some(2));
}

#[test]
fn merge_identical_scalars_is_ok() {
    let mut left = RuntimeTurnMetadata {
        model: Some(ModelId::new("claude-opus-4-7")),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        model: Some(ModelId::new("claude-opus-4-7")),
        ..Default::default()
    };
    left.merge(right).expect("matching scalars do not conflict");
    assert_eq!(left.model, Some(ModelId::new("claude-opus-4-7")));
}

#[test]
fn merge_scalar_conflict_refuses_model() {
    let mut left = RuntimeTurnMetadata {
        model: Some(ModelId::new("claude-opus-4-7")),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        model: Some(ModelId::new("claude-sonnet-4-6")),
        ..Default::default()
    };
    let err: TurnMetadataMergeConflict = left.merge(right).expect_err("conflict expected");
    assert_eq!(err.field, "model");
}

#[test]
fn merge_scalar_conflict_refuses_provider() {
    let mut left = RuntimeTurnMetadata {
        provider: Some(Provider::Anthropic),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        provider: Some(Provider::OpenAI),
        ..Default::default()
    };
    let err = left.merge(right).expect_err("conflict expected");
    assert_eq!(err.field, "provider");
}

#[test]
fn merge_scalar_conflict_refuses_connection_ref() {
    let mut left = RuntimeTurnMetadata {
        connection_ref: Some(ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::connection::BindingId::parse("a").expect("valid binding"),
            profile: None,
        }),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        connection_ref: Some(ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::connection::BindingId::parse("b").expect("valid binding"),
            profile: None,
        }),
        ..Default::default()
    };
    let err = left.merge(right).expect_err("conflict expected");
    assert_eq!(err.field, "connection_ref");
}

#[test]
fn merge_scalar_conflict_refuses_keep_alive() {
    let mut left = RuntimeTurnMetadata {
        keep_alive: Some(KeepAlivePolicy {
            ttl: Duration::from_secs(10),
            policy: KeepAliveMode::Pinned,
        }),
        ..Default::default()
    };
    let right = RuntimeTurnMetadata {
        keep_alive: Some(KeepAlivePolicy {
            ttl: Duration::from_secs(60),
            policy: KeepAliveMode::PolicyDriven,
        }),
        ..Default::default()
    };
    let err = left.merge(right).expect_err("conflict expected");
    assert_eq!(err.field, "keep_alive");
}
