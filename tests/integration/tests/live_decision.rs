#![cfg(all(feature = "integration-real-tests", not(target_arch = "wasm32")))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Live decision-service lane: real provider routes and the real Jev endpoint.
//!
//! Every assertion reads typed judgments or the committed transcript, never
//! model narration. Semantic expectations are limited to facts the sample
//! state determines unambiguously (a duplicate charge needing a refund before
//! Friday is urgent, is a billing matter, and the sender is not calm) so the lane measures the contract, not a
//! model's taste.
//!
//! Run with the relevant keys set:
//!   ANTHROPIC_API_KEY=... OPENAI_API_KEY=... GEMINI_API_KEY=... JEV_API_KEY=... \
//!     cargo test -p meerkat-integration-tests --features integration-real-tests \
//!     --test live_decision -- --ignored --nocapture

use std::sync::Arc;

use meerkat::{AgentBuildConfig, AgentFactory};
use meerkat_client::{AnthropicClient, GeminiClient, LlmClientAdapter, OpenAiClient};
use meerkat_core::types::{ContentInput, Message};
use meerkat_core::{AgentLlmClient, Config, DecisionLimitsConfig, Provider, ToolCategoryOverride};
use meerkat_decision::{
    BackendKind, BinaryAnswer, BinaryCriteria, BinaryJudgment, ChoiceJudgment, ChoiceOption,
    DECIDE_TOOL_NAME, DecisionAccounting, DecisionAdmission, DecisionRequest, DecisionResult,
    DecisionService, DecisionState, GradeJudgment, GradeLevel, Instructions, Judgment,
    LlmRouteBackend, NativeSignal, OptionId, Question, QuestionId, RouteProvenance,
};

/// The lane spec (`decision-live` in `e2e_lanes.rs`) declares every key group
/// as required, so inside the lane a missing key is a broken environment, not
/// a reason to report a silent pass.
fn require_env(vars: &[&str]) -> String {
    vars.iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| panic!("decision-live lane requires one of {vars:?} to be set"))
}

fn anthropic_key() -> String {
    require_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

fn openai_key() -> String {
    require_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

fn gemini_key() -> String {
    require_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

fn require_jev_key() {
    let _ = require_env(&["RKAT_JEV_API_KEY", "JEV_API_KEY"]);
}

fn anthropic_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

/// The state is chosen so each contract assertion rests on a fact the text
/// determines outright: a duplicate subscription charge is a billing matter
/// (never a product bug), the sender is pressed for time, is clearly not
/// calm, and names no refund amount.
const STATE: &str = "Help! I was charged twice for my subscription this month and I need the \
                     duplicate charge refunded before Friday. Nobody has answered my two \
                     support tickets and I am really annoyed.";

fn qid(raw: &str) -> QuestionId {
    QuestionId::new(raw).unwrap()
}

fn oid(raw: &str) -> OptionId {
    OptionId::new(raw).unwrap()
}

/// Triage request plus one speculative branch question whose premise is
/// absent from the state (no refund amount is mentioned).
fn triage_request() -> DecisionRequest {
    DecisionRequest {
        task: Some("Triage an inbound support message".into()),
        state: DecisionState::text(STATE),
        questions: vec![
            Question::Binary {
                id: qid("is_urgent"),
                instructions: Instructions::text(
                    "Does the message convey urgency, i.e. the sender needs action soon?",
                ),
                criteria: Some(BinaryCriteria {
                    yes: Instructions::text("Explicitly time-sensitive or an ongoing outage"),
                    no: Instructions::text("No urgency expressed"),
                }),
            },
            Question::ChooseOne {
                id: qid("department"),
                instructions: Instructions::text("Which team should handle this message?"),
                options: vec![
                    ChoiceOption {
                        id: oid("billing"),
                        description: Instructions::text(
                            "Charges, invoices, refunds, and subscription payments",
                        ),
                    },
                    ChoiceOption {
                        id: oid("technical"),
                        description: Instructions::text(
                            "Product bugs: app crashes, login failures, broken features, API errors",
                        ),
                    },
                    ChoiceOption {
                        id: oid("sales"),
                        description: Instructions::text("Pricing, upgrades, new accounts"),
                    },
                ],
            },
            Question::Grade {
                id: qid("frustration"),
                instructions: Instructions::text("How frustrated is the sender?"),
                levels: vec![
                    GradeLevel {
                        description: Instructions::text("Calm and neutral"),
                    },
                    GradeLevel {
                        description: Instructions::text("Frustrated but civil"),
                    },
                    GradeLevel {
                        description: Instructions::text("Very angry or hostile"),
                    },
                ],
            },
            Question::Binary {
                id: qid("states_refund_amount"),
                instructions: Instructions::text(
                    "Does the message state a specific refund amount in a currency?",
                ),
                criteria: None,
            },
        ],
    }
}

fn binary_leans_yes(judgment: &Judgment) -> bool {
    match judgment {
        Judgment::Binary(BinaryJudgment::Categorical { answer }) => *answer == BinaryAnswer::Yes,
        Judgment::Binary(BinaryJudgment::NativeProbability { yes }) => yes.get() >= 0.5,
        _ => false,
    }
}

fn binary_leans_no_or_abstains(judgment: &Judgment) -> bool {
    match judgment {
        Judgment::Binary(BinaryJudgment::Categorical { answer }) => {
            matches!(answer, BinaryAnswer::No | BinaryAnswer::Abstain)
        }
        Judgment::Binary(BinaryJudgment::NativeProbability { yes }) => yes.get() < 0.5,
        _ => false,
    }
}

/// Contract assertions shared by every route.
fn assert_triage_contract(result: &DecisionResult, backend: BackendKind) {
    assert_eq!(
        result.route.backend(),
        backend,
        "route provenance: {:?}",
        result.route
    );
    let ids: Vec<&str> = result.judgments.keys().map(QuestionId::as_str).collect();
    assert_eq!(
        ids,
        [
            "is_urgent",
            "department",
            "frustration",
            "states_refund_amount"
        ],
        "every question is answered exactly once, in request order"
    );
    assert!(
        matches!(result.accounting, DecisionAccounting::Measured { input_tokens, .. } if input_tokens > 0),
        "a real route reports measured accounting: {:?}",
        result.accounting
    );

    let urgent = &result.judgments["is_urgent"].judgment;
    assert!(
        binary_leans_yes(urgent),
        "a refund needed before Friday is urgent: {urgent:?}"
    );

    let department = &result.judgments["department"].judgment;
    match department {
        Judgment::Choice(ChoiceJudgment::Selected { option }) => {
            assert_eq!(
                option.as_str(),
                "billing",
                "a duplicate charge is a billing matter"
            );
        }
        other => panic!("department must be a selected option: {other:?}"),
    }

    let frustration = &result.judgments["frustration"].judgment;
    match frustration {
        Judgment::Grade(GradeJudgment::Level { index }) => {
            assert!(
                (1..=2).contains(&index.get()),
                "the sender is not calm: {index:?}"
            );
        }
        Judgment::Grade(GradeJudgment::NativeWeighted { position }) => {
            assert!(*position >= 0.5, "the sender is not calm: {position}");
        }
        other => panic!("frustration must be graded: {other:?}"),
    }

    let refund = &result.judgments["states_refund_amount"].judgment;
    assert!(
        binary_leans_no_or_abstains(refund),
        "a speculative question with an absent premise must not be answered yes: {refund:?}"
    );
}

fn service_over(client: Arc<dyn AgentLlmClient>) -> DecisionService {
    // The default allowance leaves room for thinking models, which spend
    // output tokens on reasoning before the answer envelope.
    let limits = DecisionLimitsConfig::default();
    DecisionService::new(
        Arc::new(LlmRouteBackend::fixed(client, limits.max_output_tokens)),
        limits,
    )
}

async fn judge_over_route(client: Arc<dyn AgentLlmClient>) -> DecisionResult {
    service_over(client)
        .evaluate(&DecisionAdmission::host_unbudgeted(), triage_request())
        .await
        .expect("live evaluation succeeds")
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_decision_anthropic_llm_route_returns_typed_judgments() {
    let key = anthropic_key();
    let model = anthropic_model();
    let client: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(
            Arc::new(AnthropicClient::new(key).unwrap()),
            model,
            Provider::Anthropic,
        )
        .unwrap(),
    );
    let result = judge_over_route(client).await;
    assert_triage_contract(&result, BackendKind::Llm);
    assert!(matches!(
        result.route,
        RouteProvenance::Llm {
            provider: Provider::Anthropic,
            ..
        }
    ));
    assert!(
        result
            .judgments
            .values()
            .all(|judgment| judgment.native_signals.is_empty()),
        "an ordinary LLM route never invents native signals"
    );
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_decision_openai_llm_route_returns_typed_judgments() {
    let key = openai_key();
    let model = std::env::var("SMOKE_OPENAI_MODEL").unwrap_or_else(|_| "gpt-5.5".to_string());
    let client: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(
            Arc::new(OpenAiClient::new(key)),
            model,
            Provider::OpenAI,
        )
        .unwrap(),
    );
    let result = judge_over_route(client).await;
    assert_triage_contract(&result, BackendKind::Llm);
}

#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_decision_gemini_llm_route_returns_typed_judgments() {
    let key = gemini_key();
    let model =
        std::env::var("SMOKE_GEMINI_MODEL").unwrap_or_else(|_| "gemini-3.5-flash".to_string());
    let client: Arc<dyn AgentLlmClient> = Arc::new(
        LlmClientAdapter::try_for_provider_identity(
            Arc::new(GeminiClient::new(key)),
            model,
            Provider::Gemini,
        )
        .unwrap(),
    );
    let result = judge_over_route(client).await;
    assert_triage_contract(&result, BackendKind::Llm);
}

/// The Jev route through the facade's own composition seam: realm config with
/// a typed `Env` credential source, resolved by the shared env-precedence
/// owner, disclosure explicitly permitted.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_decision_jev_route_via_facade_config_keeps_native_signals() {
    require_jev_key();
    let mut config = Config::default();
    config.tools.decision_enabled = true;
    config.decision.backend = meerkat_core::DecisionBackendSelection::Jev;
    config.decision.jev = Some(meerkat_core::JevBackendConfig {
        endpoint: meerkat_decision::DEFAULT_JEV_ENDPOINT.into(),
        model: meerkat_decision::DEFAULT_JEV_MODEL.into(),
        allow_disclosure: true,
        credential: meerkat_core::CredentialSourceSpec::Env {
            env: "JEV_API_KEY".into(),
            fallback: vec!["RKAT_JEV_API_KEY".into()],
        },
    });
    let service = meerkat::build_decision_service(&config, None).unwrap();

    let result = service
        .evaluate(&DecisionAdmission::host_unbudgeted(), triage_request())
        .await
        .expect("live Jev evaluation succeeds");
    assert_triage_contract(&result, BackendKind::Jev);
    assert!(matches!(
        result.route,
        RouteProvenance::Jev { ref served_model, .. } if served_model.starts_with("jev-")
    ));
    assert!(matches!(
        result.judgments["is_urgent"].judgment,
        Judgment::Binary(BinaryJudgment::NativeProbability { .. })
    ));
    assert!(matches!(
        result.judgments["department"].native_signals.as_slice(),
        [NativeSignal::ChoiceDistribution { backend: BackendKind::Jev, probabilities, .. }]
            if probabilities.len() == 3
    ));
    assert!(matches!(
        result.judgments["frustration"].native_signals.as_slice(),
        [NativeSignal::GradeDistribution { backend: BackendKind::Jev, probabilities, .. }]
            if probabilities.len() == 3
    ));
}

/// Full agent path: the facade composes `decide` on the admitted route, a
/// real model calls it, and the committed transcript carries a typed result.
#[tokio::test]
#[ignore = "lane:e2e-live"]
async fn live_decision_agent_calls_decide_tool_on_the_admitted_route() {
    let key = anthropic_key();
    let temp = tempfile::tempdir().unwrap();
    let factory = AgentFactory::new(temp.path().join("sessions"))
        .builtins(false)
        .decision(ToolCategoryOverride::Enable);
    let mut build = AgentBuildConfig::new(anthropic_model());
    build.provider = Some(Provider::Anthropic);
    build.llm_client_override = Some(Arc::new(AnthropicClient::new(key).unwrap()));
    build.override_builtins = ToolCategoryOverride::Disable;
    build.system_prompt = meerkat_core::SystemPromptOverride::Set(
        "You are a support triage assistant. When asked to classify a message you MUST call the \
         `decide` tool exactly once with the questions the user specifies, then answer with only \
         the selected department id and nothing else."
            .to_string(),
    );

    let mut agent = factory
        .build_agent(build, &Config::default())
        .await
        .unwrap();
    assert!(
        agent
            .tool_scope()
            .visible_tool_names()
            .unwrap()
            .contains(DECIDE_TOOL_NAME)
    );

    let prompt = format!(
        "Classify this support message by calling the `decide` tool once. Put the message in \
         `state`. Ask exactly these questions: a `binary` question with id `is_urgent` \
         (\"Does the message convey urgency?\"), a `choose_one` question with id `department` \
         with options `billing` (charges, invoices, refunds), `technical` (bugs and crashes), \
         and `sales` (pricing), and a `grade` question with id `frustration` with levels \
         \"Calm\", \"Frustrated\", \"Very angry\". Then reply with only the selected department \
         id.\n\nMessage: {STATE}"
    );
    let result = agent
        .run(ContentInput::Text(prompt))
        .await
        .expect("agent turn completes");

    // Authoritative evidence: the committed transcript, not the reply text.
    let mut decide_call_ids = Vec::new();
    let mut decision_results = Vec::new();
    for message in agent.session().messages() {
        match message {
            Message::BlockAssistant(assistant) => {
                for call in assistant.tool_calls() {
                    if call.name == DECIDE_TOOL_NAME {
                        decide_call_ids.push(call.id.to_string());
                    }
                }
            }
            Message::ToolResults { results, .. } => {
                for tool_result in results {
                    if decide_call_ids.contains(&tool_result.tool_use_id) {
                        assert!(
                            !tool_result.is_error,
                            "decide failed: {}",
                            tool_result.text_content()
                        );
                        let parsed: DecisionResult =
                            serde_json::from_str(&tool_result.text_content()).unwrap();
                        decision_results.push(parsed);
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        decide_call_ids.len(),
        1,
        "the model must call decide exactly once"
    );
    assert_eq!(
        decision_results.len(),
        1,
        "the decide result is committed to the transcript"
    );
    let decision = &decision_results[0];
    assert!(matches!(
        decision.route,
        RouteProvenance::Llm {
            provider: Provider::Anthropic,
            ..
        }
    ));
    assert!(matches!(
        decision.judgments.get(&qid("department")).map(|judgment| &judgment.judgment),
        Some(Judgment::Choice(ChoiceJudgment::Selected { option })) if option.as_str() == "billing"
    ));
    assert!(
        matches!(
            decision.budget,
            meerkat_decision::BudgetParticipation::Charged { .. }
                | meerkat_decision::BudgetParticipation::Unmeasured
        ),
        "the nested call participates through the owner-issued budget handle: {:?}",
        decision.budget
    );
    assert!(
        result.text.to_ascii_lowercase().contains("billing"),
        "final answer should name billing: {}",
        result.text
    );
}
