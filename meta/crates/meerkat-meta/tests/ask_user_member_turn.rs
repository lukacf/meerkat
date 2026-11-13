#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The `ask_user` tool on a real member turn: a scripted model calls the
//! tool, the call returns `asked` at once, the set is listed on the Console
//! and on stdio, a signed-in person answers it through
//! `meta/gating/decide`, and the member's next turn receives the typed
//! result as an `ask_user_result` input. Without a Console the call
//! answers `no_human_surface` and writes nothing. A runtime that holds no
//! set creates no file of the decision store, and the driver of the kind's
//! executor makes no pass before the first set.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPhase,
    DecisionPrincipalKey, DecisionRenderer, DecisionRendering, DecisionRequestBody,
    DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::types::StopReason;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{
    AuthPolicy, BigQueryNaming, ConsolePolicy, RuntimeDecisionInputs, RuntimeOpsPolicy,
    TrustedOidcRuntimeConfig, UnifiedRuntime, UnifiedRuntimeBuilder, build_runtime_decision_state,
};
use meerkat_mob::{MobDefinition, ProfileName};
use serde_json::{Value, json};
use tower::ServiceExt;

const ISSUER: &str = "https://trusted.meta.localhost";
const PERSON: &str = "person@example.test";
const IDENTITY: &str = "agent-a";
/// What the row sends the member to make it ask.
const ASK_NOW: &str = "ASK-NOW-KILO-3: ask which day to release.";
const CALL_ID: &str = "call-ask-1";

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

fn definition() -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "ask-user-member-turn-{}-{}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#,
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    ))
    .expect("definition")
}

fn spec() -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
        profile: ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

struct OneIdentityRoster;

#[async_trait]
impl RosterProvider for OneIdentityRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![spec()])
    }
}

fn decision_state() -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "ask_dataset".to_string(),
            table: "ask_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy {
            default_provider: meerkat_meta::AuthProvider::GoogleOAuth,
            email_allowlist: [PERSON.to_string()].into_iter().collect(),
        },
        trusted_oidc: TrustedOidcRuntimeConfig {
            discovery_json: json!({
                "issuer": ISSUER,
                "jwks_uri": format!("{ISSUER}/.well-known/jwks.json"),
            })
            .to_string(),
            jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
                .to_string(),
            audience: "meerkat-console".to_string(),
            require_verified_email: false,
            shared_account_tokens: false,
        },
        console: ConsolePolicy {
            require_app_auth: true,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

/// A model that calls `ask_user` when its last message is the row's
/// prompt, and answers `ok` to anything else. It keeps every request's
/// messages and the tools the first request offered.
#[derive(Clone)]
struct AskingModel {
    requests: Arc<Mutex<Vec<String>>>,
    tools: Arc<Mutex<Vec<String>>>,
}

impl AskingModel {
    fn new() -> Self {
        Self {
            requests: Arc::default(),
            tools: Arc::default(),
        }
    }

    /// The first request whose messages hold every one of `needles`, once
    /// there is one.
    async fn request_with(&self, needles: &[&str]) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let found = self
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|request| needles.iter().all(|needle| request.contains(needle)))
                .cloned();
            if let Some(found) = found {
                return found;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no model request holds {needles:?}: {:?}",
                self.requests.lock().unwrap()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

impl LlmClient for AskingModel {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let asks = request.messages.last().is_some_and(|last| {
            serde_json::to_string(last).is_ok_and(|text| text.contains(ASK_NOW))
        });
        {
            let mut tools = self.tools.lock().unwrap();
            if tools.is_empty() {
                *tools = request
                    .tools
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect();
            }
        }
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(&request.messages).expect("request messages serialize"));
        if asks {
            return Box::pin(async_stream::stream! {
                yield Ok(LlmEvent::ToolCallComplete {
                    id: CALL_ID.to_string(),
                    name: "ask_user".to_string(),
                    args: json!({
                        "title": "Release plan",
                        "questions": [{
                            "id": "day",
                            "prompt": "Which day?",
                            "choices": [
                                {"id": "mon", "label": "Monday"},
                                {"id": "fri", "label": "Friday"},
                            ],
                            "recommended": "mon",
                        }],
                    }),
                    meta: None,
                });
                yield Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                });
            });
        }
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            });
            yield Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            });
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The gateway's runtime on `state`, with one long-term agent whose model
/// is `model`.
async fn runtime(state: &Path, model: &AskingModel) -> UnifiedRuntime {
    Box::pin(
        UnifiedRuntimeBuilder::default()
            .definition(definition())
            .persistent_state(state)
            .continuity_from_state_dir(state)
            .await
            .expect("open the state-dir identity substrate")
            .roster_provider(Arc::new(OneIdentityRoster))
            .identity_runtime_instance_id("ask-user-member-turn")
            .default_llm_client(Arc::new(model.clone()))
            .build(),
    )
    .await
    .expect("runtime builds")
}

/// One Console JSON-RPC call, signed in as [`PERSON`].
async fn console_rpc(app: &axum::Router, method: &str, params: Value) -> Value {
    let mut jwt_header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    jwt_header.kid = Some("kid-current".to_string());
    let token = jsonwebtoken::encode(
        &jwt_header,
        &json!({
            "iss": ISSUER,
            "aud": "meerkat-console",
            "sub": PERSON,
            "email": PERSON,
            "provider": "google_oauth",
            "exp": chrono::Utc::now().timestamp() + 300,
        }),
        &jsonwebtoken::EncodingKey::from_secret(b"phase7-trusted-current-secret"),
    )
    .expect("signed fixture token");
    let request = Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(
            json!({"jsonrpc": "2.0", "id": "ask", "method": method, "params": params}).to_string(),
        ))
        .expect("rpc request");
    let response = tokio::time::timeout(Duration::from_mins(1), app.clone().oneshot(request))
        .await
        .expect("bounded rpc")
        .expect("rpc response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("rpc body");
    serde_json::from_slice(&body).expect("rpc json")
}

/// One stdio JSON-RPC call.
async fn stdio_rpc(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let request =
        json!({"jsonrpc": "2.0", "id": "ask", "method": method, "params": params}).to_string();
    let answer = meerkat_meta::rpc::handle_unified_rpc_json(
        runtime,
        &request,
        Duration::from_secs(30),
        None,
        None,
    )
    .await;
    serde_json::from_str(&answer).expect("rpc json")
}

/// Send the member the prompt that makes it ask.
async fn send_ask(runtime: &UnifiedRuntime) {
    runtime
        .identity_runtime()
        .expect("identity runtime installed")
        .send_keyed(
            &AgentIdentity::parse(IDENTITY).unwrap(),
            None,
            &meerkat_core::ContentInput::Text(ASK_NOW.to_string()),
            "ask-now",
        )
        .await
        .expect("the keyed send commits");
}

/// The decision store's files under `state`: the store itself and the lock
/// files of its executors.
fn decision_store_files(state: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(state)
        .expect("read the state directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with("approval_decisions"))
        .collect();
    names.sort();
    names
}

/// Long enough for a pass of the driver of the members' question sets to
/// show, if one were made.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// The error of `answer` with the pending id it names replaced.
fn refusal_shape(answer: &Value, pending_id: &str) -> Value {
    json!({
        "result": answer["result"],
        "code": answer["error"]["code"],
        "message": answer["error"]["message"]
            .as_str()
            .map(|message| message.replace(pending_id, "<id>")),
        "data": answer["error"]["data"],
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_asks_a_person_and_reads_the_answer_on_its_next_turn() {
    let state = tempfile::tempdir().unwrap();
    let model = AskingModel::new();
    let runtime = runtime(state.path(), &model).await;
    let app = runtime.build_console_json_router(decision_state());

    // Lazy: nothing is opened for a runtime that holds no set yet.
    settle().await;
    let passes_before = runtime.ask_user_driver_passes();
    let files_before = decision_store_files(state.path());
    let listed_before = console_rpc(&app, "meta/gating/questions", json!({})).await;
    let files_after_read = decision_store_files(state.path());

    // The member's turn calls the tool: the call answers `asked` at once.
    send_ask(&runtime).await;
    let asked = model
        .request_with(&[CALL_ID, r#"\"outcome\":\"asked\""#])
        .await;
    let listed = console_rpc(&app, "meta/gating/questions", json!({})).await;
    let stdio_listed = stdio_rpc(&runtime, "meta/gating/questions", json!({})).await;
    let stdio_capabilities = stdio_rpc(&runtime, "meta/capabilities", json!({})).await;
    let set = listed["result"]["question_sets"][0].clone();
    let pending_id = set["pending_id"].as_str().expect("pending id").to_string();

    // On stdio, an open decision of another kind is an unknown pending id.
    let other = runtime
        .decision_owner()
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("made_up.kind").expect("kind"),
                DecisionSubjectKey::new("subject").expect("subject"),
                DecisionOperationId::new("operation:made-up").expect("operation"),
                Sha256Digest::of(br#"{"build":7}"#),
                DecisionRequestBody::from_value(&json!({"build": 7})).expect("body"),
                DecisionPrincipalKey::of(
                    &PrincipalRef::in_domain(
                        PrincipalKind::SharedAgent,
                        "builder",
                        TrustDomainId::new("example.org").expect("domain"),
                    )
                    .expect("principal"),
                )
                .expect("key"),
                ApproverId::new("team:owners").expect("approver id"),
                u64::MAX / 2,
                DecisionRendering::new("Release build 7.").expect("rendering"),
                DecisionRenderer::new("render.v1").expect("renderer"),
            ),
            false,
        )
        .expect("hold");
    let other_id = other.decision_id.to_string();
    let unknown_id = format!("decision:{}", "0".repeat(64));
    let stdio_decide = |pending_id: &str, digest: String| {
        json!({
            "pending_id": pending_id,
            "approver_id": "person:host",
            "decision": "approve",
            "presented_digest": digest,
            "answers": [{"question_id": "day", "answer": {"kind": "choice", "choice_id": "mon"}}],
        })
    };
    let stdio_other = stdio_rpc(
        &runtime,
        "meta/gating/decide",
        stdio_decide(&other_id, other.descriptor.subject_digest.to_string()),
    )
    .await;
    let stdio_unknown = stdio_rpc(
        &runtime,
        "meta/gating/decide",
        stdio_decide(&unknown_id, other.descriptor.subject_digest.to_string()),
    )
    .await;
    let other_after = runtime
        .decision_owner()
        .get(&other.decision_id)
        .expect("the other decision");

    // The person answers on the Console; the member's next turn reads it.
    let decided = console_rpc(
        &app,
        "meta/gating/decide",
        json!({
            "pending_id": pending_id,
            "decision": "approve",
            "presented_digest": set["subject_digest"],
            "answers": [{"question_id": "day", "answer": {"kind": "choice", "choice_id": "fri"}}],
        }),
    )
    .await;
    let told = model
        .request_with(&[
            "ask_user_result\\n",
            r#"\"outcome\":\"answered\""#,
            r#"\"choice_id\":\"fri\""#,
        ])
        .await;
    let listed_after = console_rpc(&app, "meta/gating/questions", json!({})).await;
    let passes_after = runtime.ask_user_driver_passes();
    let tools = model.tools.lock().unwrap().clone();
    runtime.shutdown().await;

    assert_eq!(passes_before, None, "no pass before the first set");
    assert_eq!(
        files_before,
        Vec::<String>::new(),
        "no file of the decision store before the first set"
    );
    assert_eq!(
        listed_before["result"],
        json!({ "question_sets": [] }),
        "{listed_before}"
    );
    assert_eq!(
        files_after_read,
        Vec::<String>::new(),
        "a read creates no file of the decision store"
    );
    assert!(
        tools.iter().any(|tool| tool == "ask_user"),
        "the member's model is offered the tool: {tools:?}"
    );
    assert!(asked.contains("answer_by"), "{asked}");
    assert_eq!(set["origin"], json!({ "identity": IDENTITY }), "{listed}");
    assert_eq!(set["title"], json!("Release plan"), "{listed}");
    assert_eq!(set["questions"][0]["recommended"], json!("mon"), "{listed}");
    assert_eq!(
        stdio_listed["result"]["question_sets"][0]["pending_id"],
        json!(pending_id),
        "{stdio_listed}"
    );
    assert!(
        stdio_capabilities["result"]["methods"]
            .as_array()
            .is_some_and(|methods| methods.contains(&json!("meta/gating/questions"))),
        "{stdio_capabilities}"
    );
    assert_eq!(
        refusal_shape(&stdio_other, &other_id),
        refusal_shape(&stdio_unknown, &unknown_id),
        "{stdio_other}"
    );
    assert!(stdio_unknown["error"]["code"].is_i64(), "{stdio_unknown}");
    assert_eq!(other_after.state.phase, DecisionPhase::Open);
    assert!(other_after.outcome.is_none());
    assert!(other_after.presentations.is_empty());
    assert_eq!(
        decided["result"]["pending_id"],
        json!(pending_id),
        "{decided}"
    );
    assert_eq!(decided["result"]["kind"], json!("question_set"));
    assert_eq!(decided["result"]["outcome"], json!("answered"));
    assert!(told.contains(&pending_id), "{told}");
    assert_eq!(
        listed_after["result"],
        json!({ "question_sets": [] }),
        "an answered set is no longer listed"
    );
    assert!(
        passes_after.is_some_and(|passes| passes >= 1),
        "{passes_after:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_console_the_call_answers_that_no_person_can_be_asked() {
    let state = tempfile::tempdir().unwrap();
    let model = AskingModel::new();
    let runtime = runtime(state.path(), &model).await;

    send_ask(&runtime).await;
    let answered = model
        .request_with(&[CALL_ID, r#"\"outcome\":\"no_human_surface\""#])
        .await;
    let stdio_listed = stdio_rpc(&runtime, "meta/gating/questions", json!({})).await;
    let files = decision_store_files(state.path());
    settle().await;
    let passes = runtime.ask_user_driver_passes();
    runtime.shutdown().await;

    assert!(!answered.contains(r#"\"request_id\""#), "{answered}");
    assert_eq!(
        stdio_listed["result"],
        json!({ "question_sets": [] }),
        "{stdio_listed}"
    );
    assert_eq!(
        files,
        Vec::<String>::new(),
        "nothing is held: no file of the decision store"
    );
    assert_eq!(passes, None, "no pass is made for the call");
}
