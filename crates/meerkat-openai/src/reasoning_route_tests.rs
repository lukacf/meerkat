//! Route observations of the OpenAI clients and the request-local marker of a
//! reasoning effort level that was resolved for the request's route.
//!
//! A client says, without I/O, which backend and API it serves. A level whose
//! request carries the marker was decided against the route's own facts, so
//! the Responses client does not judge it by the direct-API catalog row and
//! writes it for a model whose reasoning payload the catalog does not enable.
//! A level without the marker keeps the behaviour it always had.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::lifecycle::run_primitive::ReasoningEffort;
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::provider_matrix::OpenAiBackendKind;
use meerkat_core::{
    AccountLevelStatement, Message, ModelApi, ReasoningBackend, ReasoningRouteObservation,
    UserMessage,
};
use meerkat_llm_core::{LlmClient, LlmError, LlmRequest};
use serde_json::{Value, json};

use crate::client_compatible::{OpenAiCompatibleClient, OpenAiCompatibleMode};
use crate::{AzureOpenAiWireConfig, OpenAiClient, OpenAiCompatibleClientOptions};

/// A catalog model with a reasoning row.
const CATALOG_REASONING_MODEL: &str = "gpt-5.4";
/// A catalog model whose reasoning row has no `max`.
const CATALOG_MODEL_WITHOUT_MAX: &str = "gpt-5.5";
/// A model id no catalog row describes.
const UNCATALOGUED_MODEL: &str = "fixture-responses-a";

const EVERY_OPENAI_LEVEL: [ReasoningEffort; 6] = [
    ReasoningEffort::None,
    ReasoningEffort::Low,
    ReasoningEffort::Medium,
    ReasoningEffort::High,
    ReasoningEffort::XHigh,
    ReasoningEffort::Max,
];

fn responses_client(backend: OpenAiBackendKind) -> OpenAiClient {
    let client = OpenAiClient::new("test-key".to_string());
    match backend {
        OpenAiBackendKind::OpenAiApi => client,
        OpenAiBackendKind::ChatGptBackend => client.with_chatgpt_backend_wire(),
        OpenAiBackendKind::AzureOpenAi => {
            client.with_azure_openai_wire(AzureOpenAiWireConfig::default())
        }
        OpenAiBackendKind::Copilot => client.with_copilot_backend_wire(),
    }
}

fn compatible_client(mode: OpenAiCompatibleMode) -> OpenAiCompatibleClient {
    OpenAiCompatibleClient::new_with_options(
        mode,
        "remote-model".to_string(),
        "http://localhost:8000/v1".to_string(),
        None,
        OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_image_tool_results: false,
        },
    )
}

fn request(model: &str) -> LlmRequest {
    LlmRequest::new(
        model,
        vec![Message::User(UserMessage::text("hi".to_string()))],
    )
}

/// A request that carries `level` as Core writes a resolved level: the typed
/// effort and the request-local marker.
fn marked(model: &str, level: ReasoningEffort) -> LlmRequest {
    request(model).with_openai_tag_merge(|tag| {
        tag.reasoning_effort = Some(level);
        tag.reasoning_effort_resolved = true;
    })
}

/// A request that carries `level` as a caller's raw provider param.
fn raw(model: &str, level: ReasoningEffort) -> LlmRequest {
    request(model).with_openai_tag_merge(|tag| tag.reasoning_effort = Some(level))
}

fn observation(backend: OpenAiBackendKind, api: ModelApi) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        backend: ReasoningBackend::OpenAi(backend),
        api,
        account: AccountLevelStatement::NotApplicable,
        operator_levels: None,
    }
}

/// R2.1, the OpenAI family: the Responses client reports the backend of its
/// wire, the compatible client the OpenAI backend it was declared for and
/// the API of its mode. No account list and no operator levels are theirs
/// to state.
#[test]
fn every_adapter_reports_its_route() {
    for backend in OpenAiBackendKind::ALL.iter().copied() {
        assert_eq!(
            responses_client(backend).reasoning_route(),
            Some(observation(backend, ModelApi::Responses)),
            "the Responses client on the {} wire",
            backend.as_str()
        );
    }

    for (mode, api) in [
        (
            OpenAiCompatibleMode::ChatCompletions,
            ModelApi::ChatCompletions,
        ),
        (OpenAiCompatibleMode::Responses, ModelApi::Responses),
    ] {
        assert_eq!(
            compatible_client(mode)
                .with_openai_backend(OpenAiBackendKind::Copilot)
                .reasoning_route(),
            Some(observation(OpenAiBackendKind::Copilot, api)),
            "the compatible client declared for the Copilot backend, {api}"
        );
    }
}

/// R2.1: a compatible client that was not told which backend it speaks to
/// reports no route. Nothing guesses a backend from a base URL.
#[test]
fn a_compatible_client_without_a_declared_backend_reports_no_route() {
    for mode in [
        OpenAiCompatibleMode::ChatCompletions,
        OpenAiCompatibleMode::Responses,
    ] {
        assert_eq!(compatible_client(mode).reasoning_route(), None, "{mode:?}");
    }
    // Operator levels alone name no backend.
    assert_eq!(
        compatible_client(OpenAiCompatibleMode::ChatCompletions)
            .with_operator_reasoning_effort_levels(Some(vec![EffortLevel::Low]))
            .reasoning_route(),
        None
    );
}

/// R2.1 and R2.3, self-hosted servers: the compatible client reports the
/// self-hosted backend kind it was built for, the API of its mode, and the
/// effort levels the operator declared for the model, as it was given them.
#[test]
fn a_self_hosted_client_reports_its_backend_and_the_operators_levels() {
    use meerkat_core::provider_matrix::SelfHostedBackendKind;

    for kind in SelfHostedBackendKind::ALL.iter().copied() {
        for (mode, api) in [
            (
                OpenAiCompatibleMode::ChatCompletions,
                ModelApi::ChatCompletions,
            ),
            (OpenAiCompatibleMode::Responses, ModelApi::Responses),
        ] {
            let undeclared = compatible_client(mode).with_self_hosted_backend(kind);
            assert_eq!(
                undeclared.reasoning_route(),
                Some(ReasoningRouteObservation {
                    backend: ReasoningBackend::SelfHosted(kind),
                    api,
                    account: AccountLevelStatement::NotApplicable,
                    operator_levels: None,
                }),
                "{} {api}: no levels declared",
                kind.as_str()
            );
            for levels in [
                vec![EffortLevel::Low, EffortLevel::Medium, EffortLevel::High],
                Vec::new(),
            ] {
                assert_eq!(
                    compatible_client(mode)
                        .with_self_hosted_backend(kind)
                        .with_operator_reasoning_effort_levels(Some(levels.clone()))
                        .reasoning_route(),
                    Some(ReasoningRouteObservation {
                        backend: ReasoningBackend::SelfHosted(kind),
                        api,
                        account: AccountLevelStatement::NotApplicable,
                        operator_levels: Some(levels),
                    }),
                    "{} {api}",
                    kind.as_str()
                );
            }
        }
    }

    // A client declared for an OpenAI backend speaks to that backend, not
    // to a self-hosted server.
    assert_eq!(
        compatible_client(OpenAiCompatibleMode::ChatCompletions)
            .with_self_hosted_backend(SelfHostedBackendKind::SelfHosted)
            .with_openai_backend(OpenAiBackendKind::Copilot)
            .reasoning_route(),
        Some(observation(
            OpenAiBackendKind::Copilot,
            ModelApi::ChatCompletions
        )),
    );
}

/// The backend and the operator's levels are observations only: a
/// self-hosted client that declares them builds the bodies it built before.
#[test]
fn declaring_a_backend_and_operator_levels_changes_no_body() {
    use meerkat_core::provider_matrix::SelfHostedBackendKind;

    for candidate in [
        request(UNCATALOGUED_MODEL),
        raw(UNCATALOGUED_MODEL, ReasoningEffort::High),
        marked(UNCATALOGUED_MODEL, ReasoningEffort::High),
    ] {
        let plain = compatible_client(OpenAiCompatibleMode::ChatCompletions);
        let declared = compatible_client(OpenAiCompatibleMode::ChatCompletions)
            .with_self_hosted_backend(SelfHostedBackendKind::SelfHosted)
            .with_operator_reasoning_effort_levels(Some(vec![EffortLevel::Low]));
        assert_eq!(
            declared
                .build_chat_completions_body(&candidate)
                .expect("the request builds"),
            plain
                .build_chat_completions_body(&candidate)
                .expect("the request builds"),
        );
    }
}

/// R2.2, Responses: on every backend a resolved level of a catalogued
/// reasoning model replaces the `effort` value of the payload Meerkat
/// already sends, and nothing else.
#[test]
fn translated_level_reaches_the_wire() {
    for backend in OpenAiBackendKind::ALL.iter().copied() {
        let client = responses_client(backend);
        let baseline = client
            .build_request_body(&request(CATALOG_REASONING_MODEL))
            .expect("the request without a level builds");
        assert_eq!(
            baseline["reasoning"],
            json!({"effort": "medium", "summary": "auto"})
        );
        for level in [
            ReasoningEffort::Low,
            ReasoningEffort::High,
            ReasoningEffort::XHigh,
        ] {
            let body = client
                .build_request_body(&marked(CATALOG_REASONING_MODEL, level))
                .expect("a resolved level builds");
            let mut expected = baseline.clone();
            expected["reasoning"]["effort"] = json!(level.as_legacy_str());
            assert_eq!(
                body,
                expected,
                "{} wire, level {}",
                backend.as_str(),
                level.as_legacy_str()
            );
        }
    }
}

/// R2.2, Responses, a model the catalog has no row for, on the Copilot wire:
/// the account's list stated the level, so it reaches the wire.
#[test]
fn translated_level_reaches_the_wire_for_a_model_without_a_catalog_row() {
    let client = responses_client(OpenAiBackendKind::Copilot);
    for level in EVERY_OPENAI_LEVEL {
        let body = client
            .build_request_body(&marked(UNCATALOGUED_MODEL, level))
            .expect("a resolved level builds");
        assert_eq!(
            body["reasoning"]["effort"],
            json!(level.as_legacy_str()),
            "level {}",
            level.as_legacy_str()
        );
    }
}

/// R2.2, Chat Completions: the level is written in both spellings the
/// compatible servers read, `reasoning_effort` and `reasoning.effort`.
#[test]
fn translated_level_reaches_the_chat_completions_wire() {
    let client = compatible_client(OpenAiCompatibleMode::ChatCompletions)
        .with_openai_backend(OpenAiBackendKind::Copilot);
    let baseline = client
        .build_chat_completions_body(&request(UNCATALOGUED_MODEL))
        .expect("the request without a level builds");
    for level in EVERY_OPENAI_LEVEL {
        let body = client
            .build_chat_completions_body(&marked(UNCATALOGUED_MODEL, level))
            .expect("a resolved level builds");
        let mut expected = baseline.clone();
        expected["reasoning_effort"] = json!(level.as_legacy_str());
        expected["reasoning"] = json!({"effort": level.as_legacy_str()});
        assert_eq!(body, expected, "level {}", level.as_legacy_str());
        // The marker changes nothing on this wire: a raw level is written
        // the same way.
        assert_eq!(
            client
                .build_chat_completions_body(&raw(UNCATALOGUED_MODEL, level))
                .expect("a raw level builds"),
            expected
        );
    }
}

/// R2.8: for a model whose reasoning payload the catalog does not enable, a
/// resolved level is sent as `reasoning: {"effort": <level>}` and nothing
/// else of Meerkat's reasoning payload: no `summary`, no `include`. The
/// rest of the body is the body of the same request without a level.
#[test]
fn effort_only_payload_for_a_model_without_a_reasoning_row() {
    let client = responses_client(OpenAiBackendKind::Copilot);
    let baseline = client
        .build_request_body(&request(UNCATALOGUED_MODEL))
        .expect("the request without a level builds");
    assert!(baseline.get("reasoning").is_none());
    assert!(baseline.get("include").is_none());

    for level in EVERY_OPENAI_LEVEL {
        let body = client
            .build_request_body(&marked(UNCATALOGUED_MODEL, level))
            .expect("a resolved level builds");
        assert_eq!(
            body["reasoning"],
            json!({"effort": level.as_legacy_str()}),
            "level {}",
            level.as_legacy_str()
        );
        assert!(body["reasoning"].get("summary").is_none());
        assert!(body.get("include").is_none());
        let mut expected = baseline.clone();
        expected["reasoning"] = json!({"effort": level.as_legacy_str()});
        assert_eq!(body, expected, "only the reasoning object is added");
    }

    // A model the catalog describes without reasoning takes the same body.
    let overridden = marked(CATALOG_REASONING_MODEL, ReasoningEffort::Low)
        .with_openai_tag_merge(|tag| tag.supports_reasoning_override = Some(false));
    let body = client
        .build_request_body(&overridden)
        .expect("a resolved level builds");
    assert_eq!(body["reasoning"], json!({"effort": "low"}));
    assert!(body.get("include").is_none());
}

fn refusal_message(result: Result<Value, LlmError>) -> String {
    match result {
        Err(LlmError::InvalidRequest { message }) => message,
        other => panic!("expected the catalog row's refusal, got {other:?}"),
    }
}

/// R2.9: a level without the marker (a caller's raw provider param) is
/// judged by the catalog row with the message it always had, and is dropped
/// for a model without a reasoning row, on every backend.
#[test]
fn a_raw_level_keeps_todays_adapter_behaviour() {
    for backend in OpenAiBackendKind::ALL.iter().copied() {
        let client = responses_client(backend);
        assert_eq!(
            refusal_message(
                client.build_request_body(&raw(CATALOG_MODEL_WITHOUT_MAX, ReasoningEffort::Max))
            ),
            "OpenAI model 'gpt-5.5' does not support reasoning effort 'max'",
            "{} wire",
            backend.as_str()
        );

        let baseline = client
            .build_request_body(&request(UNCATALOGUED_MODEL))
            .expect("the request without a level builds");
        let body = client
            .build_request_body(&raw(UNCATALOGUED_MODEL, ReasoningEffort::High))
            .expect("a raw level on a model without a row builds");
        assert_eq!(body, baseline, "{} wire: dropped", backend.as_str());
        assert!(body.get("reasoning").is_none());
    }
}

/// R2.10: a resolved level is not judged by the direct-API catalog row. The
/// account's list stated it for the route, so the body carries it.
#[test]
fn a_marked_level_is_not_judged_by_the_direct_row() {
    let client = responses_client(OpenAiBackendKind::Copilot);
    let body = client
        .build_request_body(&marked(CATALOG_MODEL_WITHOUT_MAX, ReasoningEffort::Max))
        .expect("a resolved level is not refused by the catalog row");
    assert_eq!(
        body["reasoning"],
        json!({"effort": "max", "summary": "auto"})
    );
    assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
}

/// The marker without a level changes no request: every body is the body
/// of the unmarked request.
#[test]
fn the_marker_alone_changes_no_body() {
    for backend in OpenAiBackendKind::ALL.iter().copied() {
        let client = responses_client(backend);
        for model in [CATALOG_REASONING_MODEL, UNCATALOGUED_MODEL] {
            let unmarked = client
                .build_request_body(&request(model))
                .expect("the request builds");
            let marker_only = client
                .build_request_body(
                    &request(model)
                        .with_openai_tag_merge(|tag| tag.reasoning_effort_resolved = true),
                )
                .expect("the request builds");
            assert_eq!(marker_only, unmarked, "{} wire, {model}", backend.as_str());
        }
    }
    let chat = compatible_client(OpenAiCompatibleMode::ChatCompletions);
    assert_eq!(
        chat.build_chat_completions_body(
            &request(UNCATALOGUED_MODEL)
                .with_openai_tag_merge(|tag| tag.reasoning_effort_resolved = true)
        )
        .expect("the request builds"),
        chat.build_chat_completions_body(
            &request(UNCATALOGUED_MODEL).with_openai_tag_merge(|_| {})
        )
        .expect("the request builds"),
    );
}

/// A level that was resolved for the request's route is sent or refused,
/// never dropped. A Chat Completions client built for a model without
/// reasoning support has no place to write one, so it refuses the request
/// before anything is sent and names the level. A caller's raw level on the
/// same client is dropped as it always was, and the marker without a level
/// refuses nothing.
#[test]
fn a_resolved_level_is_never_dropped_on_the_chat_completions_wire() {
    let client = OpenAiCompatibleClient::new_with_options(
        OpenAiCompatibleMode::ChatCompletions,
        "remote-model".to_string(),
        "http://localhost:8000/v1".to_string(),
        None,
        OpenAiCompatibleClientOptions {
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_image_tool_results: false,
        },
    );
    let baseline = client
        .build_chat_completions_body(&request(UNCATALOGUED_MODEL).with_openai_tag_merge(|_| {}))
        .expect("the request without a level builds");
    assert!(baseline.get("reasoning").is_none());
    assert!(baseline.get("reasoning_effort").is_none());

    for level in EVERY_OPENAI_LEVEL {
        match client.build_chat_completions_body(&marked(UNCATALOGUED_MODEL, level)) {
            Err(LlmError::InvalidRequest { message }) => assert_eq!(
                message,
                format!(
                    "reasoning effort '{}' was resolved for this request, but this client \
                     was built for a model without reasoning support and cannot send it",
                    level.as_legacy_str()
                )
            ),
            other => panic!(
                "a resolved level {} must be refused, not dropped: {other:?}",
                level.as_legacy_str()
            ),
        }
        assert_eq!(
            client
                .build_chat_completions_body(&raw(UNCATALOGUED_MODEL, level))
                .expect("a raw level builds"),
            baseline,
            "a raw level {} is dropped as before",
            level.as_legacy_str()
        );
    }
    assert_eq!(
        client
            .build_chat_completions_body(
                &request(UNCATALOGUED_MODEL)
                    .with_openai_tag_merge(|tag| tag.reasoning_effort_resolved = true)
            )
            .expect("the marker without a level builds"),
        baseline
    );
}
