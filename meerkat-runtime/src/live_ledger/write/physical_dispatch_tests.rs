use super::*;
use crate::live_ledger::completion::{LiveCompletionEvent, LivePhysicalEffectOutcome};
use crate::store::live_history::LiveHistoryReadRequest;
use meerkat_core::{
    AgentToolDispatcher, ToolDispatchContext, ToolExecutionPolicy, ToolMutationClass,
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct PhysicalReadTool {
    calls: AtomicUsize,
    epoch: AtomicU64,
    entered: tokio::sync::Notify,
    pending: bool,
    support: meerkat_core::execution_scope::ScopedToolEffectSupport,
}

impl PhysicalReadTool {
    fn new(pending: bool) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            entered: tokio::sync::Notify::new(),
            pending,
            support: meerkat_core::execution_scope::ScopedToolEffectSupport::RequiresOuterClaim,
        }
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for PhysicalReadTool {
    fn scoped_tool_effect_support(&self) -> meerkat_core::execution_scope::ScopedToolEffectSupport {
        self.support
    }

    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(meerkat_core::ToolDef::new(
            "allowed_tool",
            "Physical read test",
            serde_json::json!({"type":"object"}),
        ))]
        .into()
    }

    fn tool_mutation_class(&self, _name: &str) -> ToolMutationClass {
        ToolMutationClass::ReadOnly
    }

    fn execution_binding_epoch(&self, _name: &str) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.pending {
            std::future::pending::<()>().await;
        }

        Ok(meerkat_core::ToolResult::new(call.id.into(), "actual result".into(), false).into())
    }
}

struct ReplaceBindingAfterClaim {
    native: Arc<dyn meerkat_core::execution_scope::ScopedEffectHost>,
    tool: Arc<PhysicalReadTool>,
    fault: PostClaimFault,
}

#[derive(Clone, Copy)]
enum PostClaimFault {
    ReplaceBinding,
    SubstituteEffectIdentity,
}

#[async_trait::async_trait]
impl meerkat_core::execution_scope::ScopedEffectHost for ReplaceBindingAfterClaim {
    async fn claim_tool_effect(
        &self,
        scope: meerkat_core::execution_scope::ScopedRunAuthority,
        effect_id: meerkat_core::ops::OperationId,
        evaluation: meerkat_core::EvaluatedToolExecutionPolicy,
    ) -> Result<
        meerkat_core::execution_scope::ScopedEffectStartPermit<
            meerkat_core::execution_scope::ScopedEffectTarget,
        >,
        meerkat_core::ToolError,
    > {
        let effect_id = match self.fault {
            PostClaimFault::ReplaceBinding => effect_id,
            PostClaimFault::SubstituteEffectIdentity => meerkat_core::ops::OperationId::new(),
        };
        let permit = self
            .native
            .claim_tool_effect(scope, effect_id, evaluation)
            .await?;
        if matches!(self.fault, PostClaimFault::ReplaceBinding) {
            self.tool.epoch.fetch_add(1, Ordering::SeqCst);
        }
        Ok(permit)
    }

    fn submit_feedback(
        &self,
        feedback: meerkat_core::execution_scope::ScopedEffectFeedback,
    ) -> meerkat_core::execution_scope::ScopedEffectSettlement {
        self.native.submit_feedback(feedback)
    }
}

fn resolution_context() -> TestResult<meerkat_core::ToolExecutionResolutionContext> {
    Ok(meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            std::time::Duration::from_secs(5),
        )])?,
    ))
}

async fn assert_feedback(owned: &OwnedFixture, expected: LivePhysicalEffectOutcome) -> TestResult {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
            assert_eq!(state.claim_records.len(), 1);
            if let Some(sequence) = state
                .claim_terminal_sequences
                .values()
                .find(|seq| **seq > 0)
            {
                let window = owned
                    .fixture
                    .ops()?
                    .read_live_history(&LiveHistoryReadRequest::new(
                        head.reference,
                        None,
                        sequence - 1,
                        1,
                    )?)
                    .await?;
                let [LiveLedgerRecord::Completion(record)] = window.records() else {
                    return Err("expected actual physical completion record".into());
                };
                assert!(
                    matches!(&record.event, LiveCompletionEvent::EffectTerminal { outcome, .. }
                        if *outcome == expected),
                    "{:?}",
                    record.event
                );
                assert_eq!(state.claim_credit_spent_records.values().next(), Some(&1));
                return Ok(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await?
}

async fn model_request(
    owned: &OwnedFixture,
) -> TestResult<Arc<meerkat_core::execution_scope::ScopedModelRequest>> {
    model_request_for_provider(owned, meerkat_core::Provider::OpenAI).await
}

async fn model_request_for_provider(
    owned: &OwnedFixture,
    provider: meerkat_core::Provider,
) -> TestResult<Arc<meerkat_core::execution_scope::ScopedModelRequest>> {
    let scope = owned.staged_scope().await?;
    Ok(Arc::new(
        meerkat_core::execution_scope::ScopedModelRequest::new(
            owned.machine.scoped_execution_context(scope)?,
            meerkat_core::ops::OperationId::new(),
            meerkat_core::SessionLlmIdentity {
                model: "fixture-model".into(),
                provider,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
        )?,
    ))
}

fn model_provenance() -> meerkat_core::LoweredRequestProvenance {
    meerkat_core::LoweredRequestProvenance::from_body(
        meerkat_core::Provider::OpenAI,
        meerkat_core::LoweredRequestEncoding::OpenAiResponsesJson,
        br#"{"model":"fixture-model","tools":[]}"#,
    )
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_model_attempt_lineage_survives_closed_store_and_rejects_stale_quotes() -> TestResult
{
    use super::claim_tests::read_only_observation;
    use crate::live_ledger::authority::store::LiveRequestStoreOwner;
    use meerkat_core::execution_scope::{
        ScopedEffectOutcome, ScopedEffectTarget, ScopedModelRequest,
    };
    use meerkat_core::ops::OperationId;

    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for (outcome, measured) in [
            (ScopedEffectOutcome::Succeeded, false),
            (ScopedEffectOutcome::Failed, false),
            (ScopedEffectOutcome::Unknown, false),
            (ScopedEffectOutcome::Succeeded, true),
            (ScopedEffectOutcome::Unknown, true),
        ] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let request_id = OperationId::new();
            let request = Arc::new(
                ScopedModelRequest::restore(
                    owned.machine.scoped_execution_context(scope.clone())?,
                    request_id.clone(),
                    meerkat_core::SessionLlmIdentity {
                        model: "fixture-model".into(),
                        provider: meerkat_core::Provider::OpenAI,
                        self_hosted_server_id: None,
                        provider_params: None,
                        auth_binding: None,
                    },
                )
                .await?,
            );
            assert_eq!(request.physical_attempt_ordinal(), 0);
            let mut custody = Arc::clone(&request)
                .claim_request(
                    "fixture-model",
                    "fixture-route",
                    model_provenance(),
                    meerkat_core::ProviderNativeToolPolicy::DisableAll,
                )
                .await?;
            custody.begin_invocation()?;
            assert!(
                owned
                    .machine
                    .scoped_effect_host()?
                    .resolve_model_attempt(scope.clone(), request_id.clone())
                    .await
                    .is_err()
            );
            if measured {
                let usage = meerkat_core::TurnUsage::host_declared(
                    meerkat_core::Provider::Anthropic,
                    "reported-model",
                    meerkat_core::Usage {
                        input_tokens: 999,
                        output_tokens: 1,
                        ..Default::default()
                    },
                );
                custody.observe_usage(&usage);
                custody.observe_usage(&usage);
            }
            match outcome {
                ScopedEffectOutcome::Succeeded => custody.settle_completed().await?,
                ScopedEffectOutcome::Failed => {
                    custody.settle_rejected().await?;
                }
                ScopedEffectOutcome::Unknown if measured => {
                    drop(custody);
                    tokio::time::timeout(std::time::Duration::from_secs(3), async {
                        loop {
                            let head = owned
                                .fixture
                                .ops()?
                                .load_live_head(owned.fixture.session.id())
                                .await?
                                .ok_or("head")?;
                            let state = crate::generated::live_request_state::decode(
                                &head.payload.request_snapshot,
                            )?;
                            if state.claim_phases.values().next()
                                == Some(&dsl::LiveEffectPhase::Unknown)
                            {
                                return TestResult::Ok(());
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                        }
                    })
                    .await??;
                }
                ScopedEffectOutcome::Unknown => custody.settle(outcome).await?,
            }
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            if measured {
                let state =
                    crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
                assert_eq!(
                    state
                        .request_known_tokens
                        .get(&scope.record().request_id.to_string()),
                    Some(&1000)
                );
                let claim: meerkat_core::execution_scope::ScopedEffectClaimRecord<
                    ScopedEffectTarget,
                > = serde_json::from_str(state.claim_records.values().next().ok_or("claim")?)?;
                let accounting: meerkat_core::execution_scope::ScopedEffectTokenAccounting =
                    serde_json::from_str(
                        state
                            .claim_accounting_records
                            .values()
                            .next()
                            .ok_or("accounting")?,
                    )?;
                assert!(matches!(
                    accounting,
                    meerkat_core::execution_scope::ScopedEffectTokenAccounting::Measured {
                        normalized_tokens: 1000,
                        reported_provider: meerkat_core::Provider::Anthropic,
                        identity_disputed: true,
                        ..
                    }
                ));
                owned
                    .machine
                    .settle_live_effect_with_token_accounting(
                        claim,
                        if outcome == ScopedEffectOutcome::Unknown {
                            LivePhysicalEffectOutcome::Unknown
                        } else {
                            LivePhysicalEffectOutcome::Succeeded
                        },
                        accounting,
                        crate::live_ledger::completion::LiveCompletionText::new(
                            "scoped physical effect feedback",
                        )?,
                    )
                    .await?;
                assert_eq!(
                    owned
                        .fixture
                        .ops()?
                        .load_live_head(owned.fixture.session.id())
                        .await?,
                    Some(before.clone())
                );
            }
            drop(request);
            let OwnedFixture {
                fixture,
                machine,
                grant,
                source,
                channel,
            } = owned;
            drop(channel);
            drop(machine);
            drop(grant);
            drop(source);
            let Fixture {
                store,
                session,
                _directory,
                path,
            } = fixture;
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while Arc::strong_count(&store) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            drop(store);
            let store: Arc<dyn RuntimeStore> = Arc::new(match backend {
                Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("expected durable backend".into()),
            });
            let ops = store.live_ledger_ops().ok_or("Live ops")?;
            assert_eq!(ops.load_live_head(session.id()).await?, Some(before));
            let owner = LiveRequestStoreOwner::new(Arc::clone(&store), session.id().clone());
            let restored = owner
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await?;
            drop(scope);
            let quoted_head = ops
                .load_live_head(session.id())
                .await?
                .ok_or("restored head")?;
            let quote = owner.resolve_model_attempt(&restored, &request_id).await;
            assert_eq!(
                ops.load_live_head(session.id()).await?,
                Some(quoted_head.clone())
            );
            let target = |attempt| ScopedEffectTarget::ModelComputation {
                request_id: request_id.clone(),
                attempt,
                invocation_digest: [81; 32],
            };
            if outcome == ScopedEffectOutcome::Unknown || measured {
                if measured {
                    assert_eq!(quote?, meerkat_core::execution_scope::ScopedModelAttemptResolution::TokenBudgetExhausted {
                        used: 1000, limit: 1000,
                    });
                    let state = crate::generated::live_request_state::decode(
                        &quoted_head.payload.request_snapshot,
                    )?;
                    assert_eq!(
                        state
                            .request_known_tokens
                            .get(&restored.record().request_id.to_string()),
                        Some(&1000)
                    );
                } else {
                    assert!(quote.is_err());
                }
                for attempt in [0, 1, 2] {
                    assert!(
                        owner
                            .prepare_effect_claim(
                                restored.clone(),
                                OperationId::new(),
                                target(attempt),
                                read_only_observation()?
                            )
                            .await
                            .is_err()
                    );
                }
                assert_eq!(ops.load_live_head(session.id()).await?, Some(quoted_head));
                continue;
            }
            assert_eq!(
                quote?,
                meerkat_core::execution_scope::ScopedModelAttemptResolution::Ready { ordinal: 1 }
            );
            for attempt in [0, 2] {
                assert!(
                    owner
                        .prepare_effect_claim(
                            restored.clone(),
                            OperationId::new(),
                            target(attempt),
                            read_only_observation()?
                        )
                        .await
                        .is_err()
                );
            }
            let first = owner
                .prepare_effect_claim(
                    restored.clone(),
                    OperationId::new(),
                    target(1),
                    read_only_observation()?,
                )
                .await?;
            let stale = owner
                .prepare_effect_claim(
                    restored.clone(),
                    OperationId::new(),
                    target(1),
                    read_only_observation()?,
                )
                .await?;
            let _permit = owner.commit_effect_claim(first).await?;
            let claimed_head = ops
                .load_live_head(session.id())
                .await?
                .ok_or("claimed head")?;
            assert!(owner.commit_effect_claim(stale).await.is_err());
            assert!(
                owner
                    .resolve_model_attempt(&restored, &request_id)
                    .await
                    .is_err()
            );
            assert_eq!(
                ops.load_live_head(session.id()).await?,
                Some(claimed_head.clone())
            );
            let state = crate::generated::live_request_state::decode(
                &claimed_head.payload.request_snapshot,
            )?;
            assert_eq!(state.claim_ids.len(), 2);
            assert_eq!(state.chain_latest_claims.len(), 1);
            let mut attempts = state.claim_attempts.values().copied().collect::<Vec<_>>();
            attempts.sort_unstable();
            assert_eq!(attempts, [0, 1]);
        }
    }
    Ok(())
}

#[cfg(feature = "live")]
async fn read_model_http_request(
    stream: &mut tokio::net::TcpStream,
) -> TestResult<(String, Vec<u8>)> {
    use tokio::io::AsyncReadExt;
    let mut bytes = Vec::new();
    loop {
        if bytes.len() > 65_536 {
            return Err("model fixture request exceeded its read bound".into());
        }
        if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&bytes[..header_end])?;
            let length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .ok_or("missing content-length")?
                .1
                .trim()
                .parse::<usize>()?;
            let body_start = header_end + 4;
            let request_end = body_start
                .checked_add(length)
                .ok_or("request length overflow")?;
            if bytes.len() >= request_end {
                return Ok((headers.to_owned(), bytes[body_start..request_end].to_vec()));
            }
        }
        let mut chunk = [0; 2048];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err("model fixture connection closed before its body".into());
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

#[cfg(feature = "live")]
#[derive(Debug, Clone, Copy)]
enum ModelFixtureTransport {
    OpenAi,
    Anthropic,
    CompatibleChat,
    CompatibleResponses,
    Gemini,
    GeminiCodeAssist,
}

#[cfg(feature = "live")]
impl ModelFixtureTransport {
    const ALL: [Self; 6] = [
        Self::OpenAi,
        Self::Anthropic,
        Self::CompatibleChat,
        Self::CompatibleResponses,
        Self::Gemini,
        Self::GeminiCodeAssist,
    ];

    const WITH_AUTH_RETRY: [Self; 4] = [
        Self::OpenAi,
        Self::Anthropic,
        Self::CompatibleChat,
        Self::CompatibleResponses,
    ];

    const fn provider(self) -> meerkat_core::Provider {
        match self {
            Self::OpenAi => meerkat_core::Provider::OpenAI,
            Self::Anthropic => meerkat_core::Provider::Anthropic,
            Self::CompatibleChat | Self::CompatibleResponses => meerkat_core::Provider::SelfHosted,
            Self::Gemini | Self::GeminiCodeAssist => meerkat_core::Provider::Gemini,
        }
    }

    const fn wire_model(self) -> &'static str {
        match self {
            Self::OpenAi | Self::Anthropic | Self::Gemini | Self::GeminiCodeAssist => {
                "fixture-model"
            }
            Self::CompatibleChat | Self::CompatibleResponses => "remote-fixture-model",
        }
    }
}

#[cfg(feature = "live")]
fn model_success_sse(transport: ModelFixtureTransport) -> TestResult<String> {
    match transport {
        ModelFixtureTransport::Gemini | ModelFixtureTransport::GeminiCodeAssist => {
            let response = serde_json::json!({
                "candidates": [{
                    "content": {"parts": [{"text": "ok"}], "role": "model"},
                    "finishReason": "STOP"
                }],
                "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1}
            });
            let response = if matches!(transport, ModelFixtureTransport::GeminiCodeAssist) {
                serde_json::json!({"response": response})
            } else {
                response
            };
            Ok(format!("data: {response}\n\n"))
        }
        ModelFixtureTransport::OpenAi | ModelFixtureTransport::CompatibleResponses => Ok(format!(
            "data: {}\n\n",
            serde_json::json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_fixture", "status": "completed",
                    "output": [{
                        "id": "msg_fixture", "type": "message", "role": "assistant",
                        "status": "completed",
                        "content": [{"type": "output_text", "text": "ok", "annotations": []}]
                    }],
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                }
            })
        )),
        ModelFixtureTransport::CompatibleChat => Ok(format!(
            "data: {}\n\ndata: [DONE]\n\n",
            serde_json::json!({
                "id": "chat_fixture", "object": "chat.completion.chunk",
                "model": "remote-fixture-model",
                "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
        )),
        ModelFixtureTransport::Anthropic => {
            let events = [
                serde_json::json!({"type": "message_start", "message": {
                    "id": "msg_fixture", "type": "message", "role": "assistant",
                    "model": "fixture-model", "content": [], "stop_reason": null,
                    "usage": {"input_tokens": 1, "output_tokens": 0}
                }}),
                serde_json::json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": "text", "text": ""}}),
                serde_json::json!({"type": "content_block_delta", "index": 0,
                    "delta": {"type": "text_delta", "text": "ok"}}),
                serde_json::json!({"type": "content_block_stop", "index": 0}),
                serde_json::json!({"type": "message_delta",
                    "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
                serde_json::json!({"type": "message_stop"}),
            ];
            let mut sse = String::new();
            for event in events {
                use std::fmt::Write;
                write!(&mut sse, "data: {event}\n\n")?;
            }
            Ok(sse)
        }
    }
}

#[cfg(feature = "live")]
fn model_client(
    transport: ModelFixtureTransport,
    base: String,
    authorizer: Option<Arc<dyn meerkat_core::HttpAuthorizer>>,
) -> TestResult<(
    Arc<dyn meerkat_llm_core::LlmClient>,
    &'static str,
    meerkat_core::LoweredRequestEncoding,
)> {
    use meerkat_core::LoweredRequestEncoding;
    match transport {
        ModelFixtureTransport::Gemini | ModelFixtureTransport::GeminiCodeAssist => {
            let mut client =
                meerkat_gemini::GeminiClient::new_with_base_url("fixture-key".into(), base);
            if let Some(authorizer) = authorizer {
                client = client.with_authorizer(authorizer);
            }
            let path = if matches!(transport, ModelFixtureTransport::GeminiCodeAssist) {
                client = client.with_code_assist_wire();
                "/v1internal:streamGenerateContent?alt=sse"
            } else {
                "/v1beta/models/fixture-model:streamGenerateContent?alt=sse"
            };
            Ok((
                Arc::new(client),
                path,
                LoweredRequestEncoding::GeminiGenerateContentJson,
            ))
        }
        ModelFixtureTransport::OpenAi => {
            let mut client =
                meerkat_openai::OpenAiClient::new("fixture-key".into()).with_base_url(base);
            if let Some(authorizer) = authorizer {
                client = client.with_authorizer(authorizer);
            }
            Ok((
                Arc::new(client),
                "/v1/responses",
                LoweredRequestEncoding::OpenAiResponsesJson,
            ))
        }
        ModelFixtureTransport::Anthropic => {
            let mut builder =
                meerkat_anthropic::client::AnthropicClientBuilder::new("fixture-key".into())
                    .base_url(base);
            if let Some(authorizer) = authorizer {
                builder = builder.authorizer(authorizer);
            }
            Ok((
                Arc::new(builder.build()?),
                "/v1/messages",
                LoweredRequestEncoding::AnthropicMessagesJson,
            ))
        }
        ModelFixtureTransport::CompatibleChat | ModelFixtureTransport::CompatibleResponses => {
            use meerkat_openai::client_compatible::OpenAiCompatibleMode;
            let (mode, path, encoding) =
                if matches!(transport, ModelFixtureTransport::CompatibleChat) {
                    (
                        OpenAiCompatibleMode::ChatCompletions,
                        "/chat/completions",
                        LoweredRequestEncoding::OpenAiChatCompletionsJson,
                    )
                } else {
                    (
                        OpenAiCompatibleMode::Responses,
                        "/v1/responses",
                        LoweredRequestEncoding::OpenAiResponsesJson,
                    )
                };
            let mut client = meerkat_openai::OpenAiCompatibleClient::new_with_options(
                mode,
                transport.wire_model().into(),
                base,
                Some("fixture-key".into()),
                meerkat_openai::OpenAiCompatibleClientOptions::default(),
            )
            .with_provider(transport.provider());
            if let Some(authorizer) = authorizer {
                client = client.with_authorizer(authorizer);
            }
            Ok((Arc::new(client), path, encoding))
        }
    }
}

#[cfg(feature = "live")]
#[derive(Default)]
struct ModelRetryAuthorizer {
    authorizations: AtomicUsize,
    rejected: AtomicUsize,
    first_entered: tokio::sync::Notify,
    first_release: tokio::sync::Notify,
}

#[cfg(feature = "live")]
#[async_trait::async_trait]
impl meerkat_core::HttpAuthorizer for ModelRetryAuthorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        let ordinal = self.authorizations.fetch_add(1, Ordering::SeqCst);
        if ordinal == 0 {
            self.first_entered.notify_one();
            self.first_release.notified().await;
        }
        request
            .headers
            .push(("Authorization".into(), format!("Bearer fixture-{ordinal}")));
        Ok(())
    }

    async fn observe_response(
        &self,
        response: &meerkat_core::HttpAuthorizationResponse<'_>,
    ) -> Result<meerkat_core::HttpAuthorizationResponseAction, meerkat_core::AuthError> {
        if response.status == 401 {
            self.rejected.fetch_add(1, Ordering::SeqCst);
            Ok(meerkat_core::HttpAuthorizationResponseAction::RetryWithFreshAuthorization)
        } else {
            Ok(meerkat_core::HttpAuthorizationResponseAction::Propagate)
        }
    }

    fn label(&self) -> &'static str {
        "native-model-retry-fixture"
    }
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_actual_provider_auth_await_and_rejection_retry_preserve_claim_order()
-> TestResult {
    use meerkat_core::AgentLlmClient;
    use tokio::io::AsyncWriteExt;

    for backend in backends() {
        for transport in ModelFixtureTransport::WITH_AUTH_RETRY {
            let provider = transport.provider();
            let owned = OwnedFixture::new(backend).await?;
            let scope = model_request_for_provider(&owned, provider).await?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}", listener.local_addr()?);
            let authorizer = Arc::new(ModelRetryAuthorizer::default());
            let (client, path, _) = model_client(transport, base, Some(authorizer.clone()))?;
            let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
                client,
                "fixture-model".into(),
            ));
            let attempt = adapter.prepare_scoped_request_attempt(
                Arc::new(vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("request"),
                )]),
                Arc::from([]),
                128,
                None,
                None,
                scope,
            )?;
            let sse = model_success_sse(transport)?;
            let server = async {
                authorizer.first_entered.notified().await;
                let head = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert!(
                    state.claim_records.is_empty(),
                    "authorization is still awaited"
                );
                authorizer.first_release.notify_one();
                let mut bodies = Vec::new();
                for ordinal in 0..2 {
                    let (mut stream, _) = listener.accept().await?;
                    let (headers, body) = read_model_http_request(&mut stream).await?;
                    assert!(headers.starts_with(&format!("POST {path} ")));
                    bodies.push(body);
                    let head = owned
                        .fixture
                        .ops()?
                        .load_live_head(owned.fixture.session.id())
                        .await?
                        .ok_or("head")?;
                    let state = crate::generated::live_request_state::decode(
                        &head.payload.request_snapshot,
                    )?;
                    assert_eq!(state.claim_records.len(), ordinal + 1);
                    assert_eq!(
                        state
                            .claim_terminal_sequences
                            .values()
                            .filter(|sequence| **sequence > 0)
                            .count(),
                        ordinal,
                        "earlier rejection must be committed before the next physical send"
                    );
                    let (status, body, content_type) = if ordinal == 0 {
                        (
                            "401 Unauthorized",
                            r#"{"error":{"type":"authentication_error","message":"expired fixture credential"}}"#,
                            "application/json",
                        )
                    } else {
                        ("200 OK", sse.as_str(), "text/event-stream")
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len(),
                    );
                    stream.write_all(response.as_bytes()).await?;
                }
                assert_eq!(bodies[0], bodies[1]);
                Ok::<_, Box<dyn std::error::Error>>(())
            };
            let (response, served) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(attempt.stream_response(), server)
                })
                .await?;
            served?;
            let response = response?;
            assert!(
                matches!(response.blocks(), [meerkat_core::AssistantBlock::Text { text, .. }] if text == "ok")
            );
            assert_eq!(authorizer.authorizations.load(Ordering::SeqCst), 2);
            assert_eq!(authorizer.rejected.load(Ordering::SeqCst), 1);
            assert!(attempt.settled_scoped_model_successor()?.is_some());
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
            assert_eq!(state.claim_records.len(), 2);
            assert_eq!(state.claim_terminal_sequences.len(), 2);
            assert!(
                state
                    .claim_terminal_sequences
                    .values()
                    .all(|sequence| *sequence > 0)
            );
            let mut sequences = state
                .claim_terminal_sequences
                .values()
                .copied()
                .collect::<Vec<_>>();
            sequences.sort_unstable();
            for (sequence, expected) in sequences.into_iter().zip([
                LivePhysicalEffectOutcome::Failed,
                LivePhysicalEffectOutcome::Succeeded,
            ]) {
                let history = owned
                    .fixture
                    .ops()?
                    .read_live_history(&LiveHistoryReadRequest::new(
                        head.reference.clone(),
                        None,
                        sequence - 1,
                        1,
                    )?)
                    .await?;
                let [LiveLedgerRecord::Completion(record)] = history.records() else {
                    return Err("missing provider retry completion".into());
                };
                assert!(
                    matches!(&record.event, LiveCompletionEvent::EffectTerminal { outcome, .. }
                    if *outcome == expected)
                );
            }
        }
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_actual_provider_adapters_bind_wire_pressure_claim_and_terminal() -> TestResult
{
    use meerkat_core::AgentLlmClient;
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncWriteExt;

    for backend in backends() {
        for transport in ModelFixtureTransport::ALL {
            let provider = transport.provider();
            let owned = OwnedFixture::new(backend).await?;
            let scope = model_request_for_provider(&owned, provider).await?;
            let logical_request_id = scope.request_id().clone();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let base = format!("http://{}", listener.local_addr()?);
            let (client, path, encoding) = model_client(transport, base.clone(), None)?;
            let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
                client,
                "fixture-model".into(),
            ));
            let attempt = adapter.prepare_scoped_request_attempt(
                Arc::new(vec![meerkat_core::Message::User(
                    meerkat_core::UserMessage::text("line\nquoted \"request\"\\tail"),
                )]),
                Arc::from([]),
                128,
                None,
                None,
                scope,
            )?;
            let pressure = attempt.request_pressure()?.ok_or("request pressure")?;
            assert_eq!(attempt.request_pressure()?.as_ref(), Some(&pressure));
            let sse = model_success_sse(transport)?;
            let route = format!("{base}{path}");
            let server = async {
                let (mut stream, _) = listener.accept().await?;
                let (headers, body) = read_model_http_request(&mut stream).await?;
                assert!(
                    headers.starts_with(&format!("POST {path} ")),
                    "unexpected {provider:?} request line: {:?}",
                    headers.lines().next()
                );
                assert_eq!(body.len() as u64, pressure.encoded_bytes);
                let payload: serde_json::Value = serde_json::from_slice(&body)?;
                if matches!(transport, ModelFixtureTransport::GeminiCodeAssist) {
                    assert_eq!(payload["request"]["tools"], serde_json::json!([]));
                    assert!(payload["user_prompt_id"].as_str().is_some());
                } else {
                    assert_eq!(payload["tools"], serde_json::json!([]));
                }
                if matches!(transport, ModelFixtureTransport::Gemini) {
                    assert!(payload.get("model").is_none());
                } else {
                    assert_eq!(payload["model"], transport.wire_model());
                }
                let head = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert_eq!(state.claim_records.len(), 1);
                let claim: meerkat_core::execution_scope::ScopedEffectClaimRecord<
                    meerkat_core::execution_scope::ScopedEffectTarget,
                > = serde_json::from_str(
                    state.claim_records.values().next().ok_or("model claim")?,
                )?;
                let provenance =
                    meerkat_core::LoweredRequestProvenance::from_body(provider, encoding, &body);
                assert_eq!(
                    pressure.lowered_request_provenance.as_ref(),
                    Some(&provenance)
                );
                let digest = Sha256::digest(serde_json::to_vec(&(
                    "meerkat.scoped-model-invocation.v1",
                    &claim.effect_id,
                    "fixture-model",
                    &route,
                    provenance,
                ))?);
                assert_eq!(
                    claim.target,
                    meerkat_core::execution_scope::ScopedEffectTarget::ModelComputation {
                        request_id: logical_request_id.clone(),
                        attempt: 0,
                        invocation_digest: digest.into(),
                    }
                );
                assert!(
                    state
                        .claim_terminal_sequences
                        .values()
                        .all(|sequence| *sequence == 0)
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{sse}",
                    sse.len(),
                );
                stream.write_all(response.as_bytes()).await?;
                Ok::<_, Box<dyn std::error::Error>>(())
            };
            let (response, served) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(attempt.stream_response(), server)
                })
                .await?;
            served?;
            let response = response?;
            assert!(matches!(response.blocks(),
                [meerkat_core::AssistantBlock::Text { text, .. }] if text == "ok"));
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
            assert!(
                state
                    .claim_terminal_sequences
                    .values()
                    .all(|sequence| *sequence > 0)
            );
            assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(5), attempt.stream_response())
                    .await?
                    .is_err(),
                "a completed physical request cannot be replayed as a fresh effect"
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                    .await
                    .is_err(),
                "replayed request reached the physical transport"
            );
        }
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_actual_adapters_reject_foreign_selected_identity_before_send() -> TestResult {
    use meerkat_core::AgentLlmClient;

    for backend in backends() {
        for transport in ModelFixtureTransport::ALL {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let scope = Arc::new(meerkat_core::execution_scope::ScopedModelRequest::new(
                owned.machine.scoped_execution_context(scope)?,
                meerkat_core::ops::OperationId::new(),
                meerkat_core::SessionLlmIdentity {
                    model: "foreign-selected-model".into(),
                    provider: transport.provider(),
                    self_hosted_server_id: None,
                    provider_params: None,
                    auth_binding: None,
                },
            )?);
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let (client, _, _) = model_client(
                transport,
                format!("http://{}", listener.local_addr()?),
                None,
            )?;
            let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
                client,
                "fixture-model".into(),
            ));
            let attempt = adapter.prepare_scoped_request_attempt(
                Arc::new(Vec::new()),
                Arc::from([]),
                128,
                None,
                None,
                scope,
            )?;
            let error =
                tokio::time::timeout(std::time::Duration::from_secs(5), attempt.stream_response())
                    .await?
                    .err()
                    .ok_or("foreign model policy was accepted")?;
            assert!(
                error.to_string().contains("selected scoped policy"),
                "{error}"
            );
            assert_eq!(
                before,
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
            );
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                    .await
                    .is_err()
            );
        }
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_gemini_authorization_precedes_claim_and_rejection_is_committed() -> TestResult
{
    use meerkat_core::AgentLlmClient;
    use tokio::io::AsyncWriteExt;

    for backend in backends() {
        for transport in [
            ModelFixtureTransport::Gemini,
            ModelFixtureTransport::GeminiCodeAssist,
        ] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = model_request_for_provider(&owned, transport.provider()).await?;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let authorizer = Arc::new(ModelRetryAuthorizer::default());
            let (client, path, _) = model_client(
                transport,
                format!("http://{}", listener.local_addr()?),
                Some(authorizer.clone()),
            )?;
            let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
                client,
                "fixture-model".into(),
            ));
            let attempt = adapter.prepare_scoped_request_attempt(
                Arc::new(Vec::new()),
                Arc::from([]),
                128,
                None,
                None,
                scope,
            )?;
            let server = async {
                authorizer.first_entered.notified().await;
                let head = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert!(state.claim_records.is_empty());
                authorizer.first_release.notify_one();
                let (mut stream, _) = listener.accept().await?;
                let (headers, _) = read_model_http_request(&mut stream).await?;
                assert!(headers.starts_with(&format!("POST {path} ")));
                let head = owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
                    .ok_or("head")?;
                let state =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert_eq!(state.claim_records.len(), 1);
                assert!(
                    state
                        .claim_terminal_sequences
                        .values()
                        .all(|sequence| *sequence == 0)
                );
                let body = r#"{"error":{"code":401,"message":"fixture credential rejected"}}"#;
                stream.write_all(format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                ).as_bytes()).await?;
                Ok::<_, Box<dyn std::error::Error>>(())
            };
            let (response, served) =
                tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(attempt.stream_response(), server)
                })
                .await?;
            served?;
            assert!(response.is_err());
            assert_eq!(authorizer.authorizations.load(Ordering::SeqCst), 1);
            assert_feedback(&owned, LivePhysicalEffectOutcome::Failed).await?;
            assert!(attempt.settled_scoped_model_successor()?.is_some());
        }
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_gemini_refuses_unowned_video_preparation_before_auth_or_io() -> TestResult {
    use futures::StreamExt;
    use meerkat_core::AgentLlmClient;
    use meerkat_llm_core::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};

    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = model_request_for_provider(&owned, meerkat_core::Provider::Gemini).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let authorizer = Arc::new(ModelRetryAuthorizer::default());
        let client = Arc::new(
            meerkat_gemini::GeminiClient::new_with_base_url(
                String::new(),
                format!("http://{}", listener.local_addr()?),
            )
            .with_authorizer(authorizer.clone())
            .with_google_backend_kind(
                meerkat_core::provider_matrix::google::GoogleBackendKind::GoogleGenAi,
            ),
        );
        let messages = vec![meerkat_core::Message::User(
            meerkat_core::UserMessage::with_blocks(vec![meerkat_core::ContentBlock::Video {
                media_type: "video/mp4".into(),
                duration_ms: 1000,
                data: meerkat_core::VideoData::Uri {
                    uri: "gs://bucket/fixture.mp4".into(),
                },
            }]),
        )];
        let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            client.clone(),
            "fixture-model".into(),
        ));
        assert!(
            adapter
                .prepare_scoped_request_attempt(
                    Arc::new(messages.clone()),
                    Arc::from([]),
                    128,
                    None,
                    None,
                    scope.clone(),
                )
                .is_err()
        );
        let raw = LlmRequest::new("fixture-model", messages)
            .with_native_tool_policy(meerkat_core::ProviderNativeToolPolicy::DisableAll);
        let mut stream = client.stream_with_execution_context(&raw, Some(scope));
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await?;
        assert!(matches!(
            event,
            Some(Ok(LlmEvent::Done { outcome: LlmDoneOutcome::Error {
                error: LlmError::InvalidRequest { message },
            }})) if message.contains("referenced-video preparation")
        ));
        assert_eq!(authorizer.authorizations.load(Ordering::SeqCst), 0);
        assert_eq!(
            before,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
    Ok(())
}

#[cfg(feature = "live")]
struct UnscopedModelClient(AtomicUsize);

#[cfg(feature = "live")]
#[async_trait::async_trait]
impl meerkat_llm_core::LlmClient for UnscopedModelClient {
    fn native_tool_policy_support(&self) -> meerkat_core::NativeToolPolicySupport {
        meerkat_core::NativeToolPolicySupport::RequestScoped
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(futures::stream::pending())
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        Ok(())
    }
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_scope_cannot_fall_through_legacy_client_or_native_only_declaration()
-> TestResult {
    use futures::StreamExt;
    use meerkat_core::AgentLlmClient;
    use meerkat_llm_core::LlmClient;

    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = model_request(&owned).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let client = Arc::new(UnscopedModelClient(AtomicUsize::new(0)));
        let raw = meerkat_llm_core::LlmRequest::new("fixture-model", Vec::new())
            .with_native_tool_policy(meerkat_core::ProviderNativeToolPolicy::DisableAll);
        let prepared = meerkat_llm_core::PreparedLlmRequest::from_projection(
            raw,
            meerkat_llm_core::LlmReplayProjection::new(Vec::new()),
        )
        .with_scoped_model(scope.clone())?;
        let mut events = client.stream_prepared(&prepared);
        assert!(matches!(
            events.next().await,
            Some(Err(meerkat_llm_core::LlmError::InvalidRequest { .. }))
        ));
        let adapter = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            client.clone(),
            "fixture-model".into(),
        ));
        assert!(
            adapter
                .prepare_scoped_request_attempt(
                    Arc::new(Vec::new()),
                    Arc::from([]),
                    128,
                    None,
                    None,
                    scope,
                )
                .is_err()
        );
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
        assert_eq!(
            before,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_send_linearizes_before_bytes_and_settles_before_terminal() -> TestResult {
    use futures::StreamExt;
    use meerkat_llm_core::{LlmDoneOutcome, LlmEvent};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let request = model_request(&owned).await?;
        let body = serde_json::json!({"model": "fixture-model", "tools": []});
        let encoded = serde_json::to_vec(&body)?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let route = format!("http://{address}/responses");
        let mut wire = format!(
            "POST /responses HTTP/1.1\r\nHost: {address}\r\nContent-Length: {}\r\n\r\n",
            encoded.len(),
        )
        .into_bytes();
        wire.extend_from_slice(&encoded);
        let expected_wire = wire.clone();
        let feedback = meerkat_llm_core::streaming::ScopedModelStreamFeedback::new(Some(&request));
        let send = meerkat_llm_core::http::send_model_request(
            Some(request),
            meerkat_llm_core::http::ModelRequestSendEvidence {
                provider: meerkat_core::Provider::OpenAI,
                encoding: meerkat_core::LoweredRequestEncoding::OpenAiResponsesJson,
                model: "fixture-model",
                route: &route,
                body: &body,
                native_tools: meerkat_core::ProviderNativeToolPolicy::DisableAll,
            },
            async {
                let result = async {
                    let mut stream = tokio::net::TcpStream::connect(address).await?;
                    stream.write_all(&wire).await?;
                    let mut response = Vec::new();
                    stream.read_to_end(&mut response).await?;
                    Ok::<_, std::io::Error>(response)
                }
                .await;
                result.map_err(|error| meerkat_llm_core::LlmError::Unknown {
                    message: error.to_string(),
                })
            },
        );
        let server = async {
            let (mut stream, _) = listener.accept().await?;
            let mut received = vec![0; expected_wire.len()];
            stream.read_exact(&mut received).await?;
            assert_eq!(received, expected_wire);
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
            assert_eq!(
                state.claim_records.len(),
                1,
                "claim must exist when bytes arrive"
            );
            assert!(
                state
                    .claim_terminal_sequences
                    .values()
                    .all(|sequence| *sequence == 0)
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (sent, served) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(send, server)
        })
        .await?;
        served?;
        let (response, custody) = sent?;
        assert!(response.ends_with(b"\r\n\r\nok"));
        feedback.install(custody)?;
        let mut events = feedback.wrap(Box::pin(futures::stream::iter([Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::EndTurn,
            },
        })])));
        assert!(matches!(
            events.next().await,
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }))
        ));
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(
            state
                .claim_terminal_sequences
                .values()
                .all(|sequence| *sequence > 0),
            "terminal must already be durable when the stream exposes success"
        );
        assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
    }
    Ok(())
}

#[cfg(feature = "live")]
#[tokio::test]
async fn native_model_stream_drop_releases_custody_despite_retained_producer() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let request = model_request(&owned).await?;
        let feedback = meerkat_llm_core::streaming::ScopedModelStreamFeedback::new(Some(&request));
        let mut custody = request
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                meerkat_core::ProviderNativeToolPolicy::DisableAll,
            )
            .await?;
        custody.begin_invocation()?;
        feedback.install(Some(custody))?;
        let events = feedback.wrap(Box::pin(futures::stream::pending()));
        drop(events);
        assert_feedback(&owned, LivePhysicalEffectOutcome::Unknown).await?;
        assert!(feedback.install(None).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn native_model_claim_binds_selected_policy_and_refuses_replay() -> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::{DisableAll, Inherit};
    use meerkat_core::execution_scope::ScopedEffectOutcome;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let request = model_request(&owned).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        for (model, native) in [("different-model", DisableAll), ("fixture-model", Inherit)] {
            assert!(
                request
                    .clone()
                    .claim_request(model, "/responses", model_provenance(), native)
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            before,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
        let mut custody = request
            .clone()
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                DisableAll,
            )
            .await?;
        custody.begin_invocation()?;
        custody.settle(ScopedEffectOutcome::Succeeded).await?;
        assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
        let committed = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            request
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll
                )
                .await
                .is_err()
        );
        assert_eq!(
            committed,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_model_claim_drop_distinguishes_unused_and_unknown() -> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::DisableAll;
    for backend in backends() {
        for invoked in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let request = model_request(&owned).await?;
            let mut custody = request
                .clone()
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll,
                )
                .await?;
            if invoked {
                custody.begin_invocation()?;
            }
            drop(custody);
            assert_feedback(
                &owned,
                if invoked {
                    LivePhysicalEffectOutcome::Unknown
                } else {
                    LivePhysicalEffectOutcome::NotStarted
                },
            )
            .await?;
            assert!(
                request
                    .claim_request(
                        "fixture-model",
                        "/responses",
                        model_provenance(),
                        DisableAll
                    )
                    .await
                    .is_err()
            );
        }
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_model_failed_conclusive_feedback_never_publishes_a_successor() -> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::DisableAll;
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for rejected in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let request = model_request(&owned).await?;
            let mut custody = request
                .clone()
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll,
                )
                .await?;
            custody.begin_invocation()?;
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            let conn = meerkat_sqlite::open(
                &owned.fixture.path,
                meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            )?;
            conn.execute_batch(
                "CREATE TRIGGER refuse_model_feedback BEFORE INSERT ON runtime_live_events
                 BEGIN SELECT RAISE(ABORT, 'injected model feedback failure'); END;",
            )?;
            drop(conn);
            let result = if rejected {
                custody.settle_rejected().await.map(|_| ())
            } else {
                custody.settle_completed().await
            };
            let error = result
                .err()
                .ok_or("refused feedback unexpectedly succeeded")?;
            assert!(
                error
                    .to_string()
                    .contains("injected model feedback failure"),
                "{error}"
            );
            assert!(request.settled_successor()?.is_none());
            let conn = meerkat_sqlite::open(
                &owned.fixture.path,
                meerkat_sqlite::ConnectionProfile::Maintenance { write: true },
            )?;
            conn.execute_batch("DROP TRIGGER refuse_model_feedback")?;
            drop(conn);
            assert_eq!(
                before,
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_model_rejection_commits_before_returning_retry_successor() -> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::DisableAll;
    use meerkat_core::execution_scope::ScopedEffectOutcome;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let original = model_request(&owned).await?;
        let mut first = original
            .clone()
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                DisableAll,
            )
            .await?;
        first.begin_invocation()?;
        let retry = first.settle_rejected().await?;
        assert_feedback(&owned, LivePhysicalEffectOutcome::Failed).await?;
        assert!(original.settled_successor()?.is_some());
        assert!(
            original
                .clone()
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll
                )
                .await
                .is_err()
        );
        let mut second = retry
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                DisableAll,
            )
            .await?;
        assert!(original.settled_successor()?.is_none());
        second.begin_invocation()?;
        second.settle(ScopedEffectOutcome::Succeeded).await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.claim_records.len(), 2);
        assert_eq!(state.claim_terminal_sequences.len(), 2);
        assert!(
            state
                .claim_terminal_sequences
                .values()
                .all(|sequence| *sequence > 0)
        );
        assert!(
            state
                .claim_credit_spent_records
                .values()
                .all(|records| *records == 1)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_model_rejection_handoff_rebinds_policy_without_resetting_attempt_or_leaking()
-> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::DisableAll;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let original = model_request(&owned).await?;
        let mut first = original
            .clone()
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                DisableAll,
            )
            .await?;
        first.begin_invocation()?;
        let retry = first.settle_rejected().await?;
        let weak = Arc::downgrade(&retry);
        drop(retry);
        assert!(
            weak.upgrade().is_none(),
            "receipt mailbox must not retain request cycles"
        );
        let retry = original.settled_successor()?.ok_or("committed successor")?;
        let fallback = retry.with_retry_identity(meerkat_core::SessionLlmIdentity {
            provider: meerkat_core::Provider::Anthropic,
            model: "fallback-model".into(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        })?;
        let provenance = meerkat_core::LoweredRequestProvenance::from_body(
            meerkat_core::Provider::Anthropic,
            meerkat_core::LoweredRequestEncoding::AnthropicMessagesJson,
            br#"{"model":"fallback-model","tools":[]}"#,
        );
        let mut second = fallback
            .claim_request("fallback-model", "/messages", provenance, DisableAll)
            .await?;
        assert!(original.settled_successor()?.is_none());
        assert!(
            retry
                .clone()
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll,
                )
                .await
                .is_err(),
            "rebinding shares the same physical claim slot"
        );
        second.begin_invocation()?;
        second
            .settle(meerkat_core::execution_scope::ScopedEffectOutcome::Unknown)
            .await?;
        assert!(
            original.settled_successor()?.is_none(),
            "unknown has no retry successor"
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.claim_records.len(), 2);
        let claims = state
            .claim_records
            .values()
            .map(|record| {
                serde_json::from_str::<
                    meerkat_core::execution_scope::ScopedEffectClaimRecord<
                        meerkat_core::execution_scope::ScopedEffectTarget,
                    >,
                >(record)
            })
            .collect::<Result<Vec<_>, _>>()?;
        assert_ne!(
            claims[0].candidate_policy_revision,
            claims[1].candidate_policy_revision
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_model_recreated_handle_cannot_replay_a_durable_claim() -> TestResult {
    use meerkat_core::ProviderNativeToolPolicy::DisableAll;
    use meerkat_core::execution_scope::ScopedModelRequest;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let context = owned.machine.scoped_execution_context(scope)?;
        let request_id = meerkat_core::ops::OperationId::new();
        let identity = meerkat_core::SessionLlmIdentity {
            provider: meerkat_core::Provider::OpenAI,
            model: "fixture-model".into(),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let first = Arc::new(ScopedModelRequest::new(
            context.clone(),
            request_id.clone(),
            identity.clone(),
        )?);
        let mut claim = first
            .claim_request(
                "fixture-model",
                "/responses",
                model_provenance(),
                DisableAll,
            )
            .await?;
        claim.begin_invocation()?;
        claim
            .settle(meerkat_core::execution_scope::ScopedEffectOutcome::Unknown)
            .await?;
        let replay = Arc::new(ScopedModelRequest::new(context, request_id, identity)?);
        assert!(
            replay
                .claim_request(
                    "fixture-model",
                    "/responses",
                    model_provenance(),
                    DisableAll
                )
                .await
                .is_err()
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.claim_records.len(), 1);
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_actor_handoff_binds_registered_cursor_and_refuses_reload_required()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let bindings = owned
            .machine
            .prepare_bindings(owned.fixture.session.id().clone())
            .await?;
        let context = owned
            .machine
            .scoped_actor_execution_context(scope.clone())
            .await?;
        assert_eq!(context.scope(), &scope);
        assert!(context.matches_actor_cursor(bindings.cursor_state()));
        assert!(!context.matches_actor_cursor(&Arc::new(meerkat_core::EpochCursorState::new())));
        assert!(
            !owned
                .machine
                .scoped_execution_context(scope.clone())?
                .matches_actor_cursor(bindings.cursor_state())
        );
        assert_eq!(context, context.clone());
        assert!(
            owned
                .machine
                .force_session_durability_reload_required_for_test(owned.fixture.session.id(),)
                .await
        );
        assert!(matches!(
            owned.machine.scoped_actor_execution_context(scope).await,
            Err(crate::RuntimeDriverError::RecoveryRepairBlocked { .. })
        ));
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_dispatch_refuses_undeclared_and_nested_owners_before_claim() -> TestResult {
    for backend in backends() {
        for nested in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let context = ToolDispatchContext::default()
                .with_scoped_execution(owned.machine.scoped_execution_context(scope)?)?;
            let mut leaf = PhysicalReadTool::new(false);
            if !nested {
                leaf.support = meerkat_core::execution_scope::ScopedToolEffectSupport::Unsupported;
            }
            let leaf = Arc::new(leaf);
            let policy =
                ToolExecutionPolicy::resolve(meerkat_core::ops::ToolAccessPolicy::ReadOnly)?;
            let inner: Arc<dyn AgentToolDispatcher> = if nested {
                Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
                    leaf.clone(),
                    policy.clone(),
                ))
            } else {
                leaf.clone()
            };
            let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
                inner, policy,
            ));
            assert_eq!(
                dispatcher.scoped_tool_effect_support(),
                meerkat_core::execution_scope::ScopedToolEffectSupport::Unsupported,
            );
            let args = serde_json::value::RawValue::from_string("{}".into())?;
            let call = meerkat_core::ToolCallView {
                id: "fc_0",
                name: "allowed_tool",
                args: &args,
            };
            let plan = meerkat_core::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &context,
                &resolution_context()?,
            )?;
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            assert!(
                meerkat_core::dispatch_tool_execution_plan_fenced(
                    &dispatcher,
                    call,
                    &context,
                    &plan,
                )
                .await
                .is_err()
            );
            assert_eq!(leaf.calls.load(Ordering::SeqCst), 0);
            assert_eq!(
                before,
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_turn_boundaries_distinguish_reused_call_ids_and_reject_old_plans()
-> TestResult {
    use meerkat_core::turn_execution_authority::{ContentShape, TurnPrimitiveKind};

    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let run_id = scope.record().run_id.clone();
        let bindings = owned
            .machine
            .prepare_bindings(owned.fixture.session.id().clone())
            .await?;
        let turn = bindings.turn_state();
        turn.start_conversation_run(
            run_id.clone(),
            TurnPrimitiveKind::ConversationTurn,
            ContentShape::Conversation,
            false,
            false,
            0,
        )?;
        turn.primitive_applied(run_id.clone())?;
        let actor = owned.machine.scoped_actor_execution_context(scope).await?;
        let first = actor.at_turn_boundary(turn.as_ref(), bindings.cursor_state())?;
        assert_eq!(
            first,
            actor.at_turn_boundary(turn.as_ref(), bindings.cursor_state())?
        );
        let first = ToolDispatchContext::default().with_scoped_execution(first)?;
        let inner = Arc::new(PhysicalReadTool::new(false));
        let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::resolve(meerkat_core::ops::ToolAccessPolicy::ReadOnly)?,
        ));
        let args = serde_json::value::RawValue::from_string("{}".into())?;
        let call = meerkat_core::ToolCallView {
            id: "fc_0",
            name: "allowed_tool",
            args: &args,
        };
        turn.llm_returned_tool_calls(run_id.clone(), 1)?;
        let old_plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &first,
            &resolution_context()?,
        )?;
        meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &first, &old_plan)
            .await?;
        turn.register_pending_ops(run_id.clone(), Default::default(), Default::default())?;
        turn.tool_calls_resolved(run_id.clone())?;
        turn.boundary_continue(run_id.clone())?;
        let next = actor.at_turn_boundary(turn.as_ref(), bindings.cursor_state())?;
        assert_ne!(first.scoped_execution(), Some(&next));
        let next = ToolDispatchContext::default().with_scoped_execution(next)?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &next, &old_plan,)
                .await
                .is_err()
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            before,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
        turn.llm_returned_tool_calls(run_id, 1)?;
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &next,
            &resolution_context()?,
        )?;
        meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &next, &plan).await?;
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.claim_records.len(), 2);
        assert_eq!(state.claim_terminal_sequences.len(), 2);
        assert!(
            state
                .claim_terminal_sequences
                .values()
                .all(|sequence| *sequence > 0)
        );
        assert!(meerkat_core::dispatch_tool_execution_plan_fenced(
            &dispatcher, call, &first, &old_plan,
        ).await.is_err());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_dispatch_invokes_actual_tool_and_commits_exact_feedback() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let context = ToolDispatchContext::default()
            .with_scoped_execution(owned.machine.scoped_execution_context(scope)?)?;
        let inner = Arc::new(PhysicalReadTool::new(false));
        let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::resolve(meerkat_core::ops::ToolAccessPolicy::ReadOnly)?,
        ));
        let args = serde_json::value::RawValue::from_string("{}".into())?;
        let call = meerkat_core::ToolCallView {
            id: "physical-call",
            name: "allowed_tool",
            args: &args,
        };
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            dispatcher
                .dispatch_with_context(call, &context)
                .await
                .is_err()
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            before,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
        let unbound = dispatcher.resolve_execution_plan(call, &context, &resolution_context()?)?;
        assert!(
            dispatcher
                .dispatch_resolved_with_context(call, &context, &unbound)
                .await
                .is_err()
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context()?,
        )?;
        let result =
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await?;
        assert!(!result.result.is_error);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            dispatcher
                .dispatch_resolved_with_context(call, &context, &plan)
                .await
                .is_err(),
            "a repeated scoped call must not claim a fresh physical attempt"
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            after,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_concurrent_same_effect_invokes_physical_tool_once() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let context = ToolDispatchContext::default()
            .with_scoped_execution(owned.machine.scoped_execution_context(scope)?)?;
        let inner = Arc::new(PhysicalReadTool::new(false));
        let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::resolve(meerkat_core::ops::ToolAccessPolicy::ReadOnly)?,
        ));
        let args = serde_json::value::RawValue::from_string("{}".into())?;
        let call = meerkat_core::ToolCallView {
            id: "same-physical-call",
            name: "allowed_tool",
            args: &args,
        };
        let first = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context()?,
        )?;
        let second = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context()?,
        )?;
        let (first, second) = tokio::join!(
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &first),
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &second),
        );
        assert_ne!(first.is_ok(), second.is_ok());
        let completed = first.or(second)?;
        assert!(!completed.result.is_error);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_dispatch_rejects_foreign_stale_and_repaired_call_plans() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let context = ToolDispatchContext::default()
            .with_scoped_execution(owned.machine.scoped_execution_context(scope)?)?;
        let inner = Arc::new(PhysicalReadTool::new(false));
        let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::unrestricted(),
        ));
        let other = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::unrestricted(),
        ));
        let args = serde_json::value::RawValue::from_string("{}".into())?;
        let changed_args = serde_json::value::RawValue::from_string(r#"{"changed":true}"#.into())?;
        let call = meerkat_core::ToolCallView {
            id: "planned-call",
            name: "allowed_tool",
            args: &args,
        };
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let foreign = meerkat_core::resolve_tool_execution_plan_fenced(
            &other,
            call,
            &context,
            &resolution_context()?,
        )?;
        assert!(
            dispatcher
                .dispatch_resolved_with_context(call, &context, &foreign)
                .await
                .is_err()
        );
        let plan = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context()?,
        )?;
        for changed in [
            meerkat_core::ToolCallView {
                id: "other-call",
                ..call
            },
            meerkat_core::ToolCallView {
                args: &changed_args,
                ..call
            },
        ] {
            assert!(
                dispatcher
                    .dispatch_resolved_with_context(changed, &context, &plan)
                    .await
                    .is_err()
            );
        }
        inner.epoch.fetch_add(1, Ordering::SeqCst);
        assert!(
            dispatcher
                .dispatch_resolved_with_context(call, &context, &plan)
                .await
                .is_err()
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            head,
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
        );
        let fresh = meerkat_core::resolve_tool_execution_plan_fenced(
            &dispatcher,
            call,
            &context,
            &resolution_context()?,
        )?;
        meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &fresh)
            .await?;
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_feedback(&owned, LivePhysicalEffectOutcome::Succeeded).await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_dispatch_drop_persists_unknown_on_owned_cleanup_runtime() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let context = ToolDispatchContext::default()
            .with_scoped_execution(owned.machine.scoped_execution_context(scope)?)?;
        let inner = Arc::new(PhysicalReadTool::new(true));
        let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
            inner.clone(),
            ToolExecutionPolicy::unrestricted(),
        ));
        let task = tokio::spawn(async move {
            let args = serde_json::value::RawValue::from_string("{}".into()).expect("test args");
            let call = meerkat_core::ToolCallView {
                id: "dropped-physical-call",
                name: "allowed_tool",
                args: &args,
            };
            let plan = meerkat_core::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &context,
                &resolution_context().expect("test deadlines"),
            )
            .expect("resolved plan");
            meerkat_core::dispatch_tool_execution_plan_fenced(&dispatcher, call, &context, &plan)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(5), inner.entered.notified()).await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(
            state.claim_records.len(),
            1,
            "physical invocation follows durable claim"
        );
        assert_eq!(state.claim_terminal_sequences.len(), 1);
        assert_eq!(state.claim_terminal_sequences.values().next(), Some(&0));
        assert_eq!(state.claim_credit_spent_records.values().next(), Some(&0));
        task.abort();
        assert!(
            task.await
                .expect_err("cancelled physical call")
                .is_cancelled()
        );
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        assert_feedback(&owned, LivePhysicalEffectOutcome::Unknown).await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_dispatch_rejects_changed_binding_or_effect_identity_without_invocation()
-> TestResult {
    for backend in backends() {
        for fault in [
            PostClaimFault::ReplaceBinding,
            PostClaimFault::SubstituteEffectIdentity,
        ] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let inner = Arc::new(PhysicalReadTool::new(false));
            let context = ToolDispatchContext::default().with_scoped_execution(
                meerkat_core::execution_scope::ScopedExecutionContext::new(
                    scope,
                    Arc::new(ReplaceBindingAfterClaim {
                        native: owned.machine.scoped_effect_host()?,
                        tool: inner.clone(),
                        fault,
                    }),
                ),
            )?;
            let dispatcher = Arc::new(meerkat_core::ExecutionPolicyGatedDispatcher::new(
                inner.clone(),
                ToolExecutionPolicy::unrestricted(),
            ));
            let args = serde_json::value::RawValue::from_string("{}".into())?;
            let call = meerkat_core::ToolCallView {
                id: "replaced-binding",
                name: "allowed_tool",
                args: &args,
            };
            let plan = meerkat_core::resolve_tool_execution_plan_fenced(
                &dispatcher,
                call,
                &context,
                &resolution_context()?,
            )?;
            let error = dispatcher
                .dispatch_resolved_with_context(call, &context, &plan)
                .await
                .expect_err("replaced binding cannot consume permit");
            let expected = match fault {
                PostClaimFault::ReplaceBinding => "binding changed",
                PostClaimFault::SubstituteEffectIdentity => "different effect identity",
            };
            assert!(error.to_string().contains(expected), "{error}");
            assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
            assert_feedback(&owned, LivePhysicalEffectOutcome::NotStarted).await?;
        }
    }
    Ok(())
}
