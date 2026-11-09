//! A hosted-search delegate is minted only by a governed fenced dispatch of
//! its own builtin call. It binds only its role's own facts, under that
//! call's exact work and coordinates, and never moves to another run.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::authorization::{
    ModelAuthorizationFacts, OperationObservation, OperationObservationError, OperationReviewTier,
    PreparedOperationAuthorization, PreparedOperationCheck, WorkAuthorization,
};
use crate::exact_operation::OperationExecutionScope;
use crate::live_execution::{CanonicalContextRevision, LiveChannelId};
use crate::{LlmRequestAuthorization, OperationId, RunId, ServerToolKind, SessionLlmIdentity};
use std::sync::Mutex;

struct Allowed;
impl PreparedOperationAuthorization for Allowed {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R1
    }

    fn check_current(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }

    fn observe(
        &self,
        _binding: &PreparedAuthorizationBinding,
        _observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        Ok(())
    }

    fn spend_at_entry(&self) -> Result<(), crate::OperationAuthorizationError> {
        Ok(())
    }
}

/// Allows every operation and records each model operation it prepares.
#[derive(Default)]
struct RecordingWork {
    models: Mutex<Vec<PreparedAuthorizationBinding>>,
}

impl WorkAuthorization for RecordingWork {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        if matches!(binding.facts().operation, AuthorizationOperation::Model(_)) {
            self.models.lock().unwrap().push(binding.clone());
        }
        Ok(Arc::new(Allowed))
    }
}

/// The helper's own route; the request authorization sets its usage.
fn search(hosted: Vec<ServerToolKind>) -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: "search-model".into(),
            provider: crate::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: Arc::from("search-wire-model"),
        hosted_capabilities: hosted.into(),
        backend_profile_id: Some(Arc::from("search-profile")),
        backend_kind: Arc::from("openai"),
        endpoint: Arc::from("https://search.invalid/responses"),
        credential: None,
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

fn malformed<T: std::fmt::Debug>(result: Result<T, OperationAuthorizationError>) -> bool {
    matches!(
        result,
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::MalformedFacts
    )
}

/// The delegate of the dispatch's own builtin web_search call binds only an
/// inference operation with exactly one hosted search capability and no
/// live channel, under the parent's own work, execution scope, run and
/// context revision and with a fresh operation id; anything else is
/// malformed facts before the work owner prepares anything. It carries no
/// review attribution, is prepared only under its own work and never moves
/// to another run.
#[tokio::test]
async fn a_hosted_search_delegate_binds_only_its_role_facts() {
    let recorder = Arc::new(RecordingWork::default());
    let work = WorkAuthorizationContext::new(recorder.clone(), OperationExecutionScope::Domain);
    let run = RunId::new();
    let attribution = __test_mint_tool_delegate(
        &work,
        Some(run.clone()),
        ToolDelegateRole::HostedSearchInference,
    )
    .await
    .expect("the governed dispatch of the builtin call mints its delegate");
    let parent = attribution.parent_binding().clone();
    assert!(attribution.work_authorization().same_context(&work));
    assert_eq!(attribution.role(), ToolDelegateRole::HostedSearchInference);
    assert_eq!(parent.facts().run_id.as_ref(), Some(&run));
    assert!(matches!(
        parent.facts().operation,
        AuthorizationOperation::Tool(_)
    ));
    assert_eq!(
        format!("{attribution:?}"),
        "ToolDelegateAttribution([REDACTED])"
    );

    let request = LlmRequestAuthorization::for_tool_delegate(attribution.clone());
    assert!(request.usage() == ModelAuthorizationUse::Inference);
    let mut live = search(vec![ServerToolKind::WebSearch]);
    live.live_channel = Some(LiveChannelId::new("live-search"));
    for (label, facts) in [
        ("no hosted capability", search(vec![])),
        (
            "two hosted capabilities",
            search(vec![
                ServerToolKind::WebSearch,
                ServerToolKind::GoogleSearch,
            ]),
        ),
        (
            "a provider-native capability",
            search(vec![ServerToolKind::ProviderNative {
                name: "code_execution".into(),
            }]),
        ),
        ("a live channel", live),
    ] {
        assert!(malformed(request.prepare(facts)), "{label}");
    }
    assert!(
        malformed(
            request
                .clone()
                .with_coordinates(Some(RunId::new()), None)
                .prepare(search(vec![ServerToolKind::WebSearch]))
        ),
        "another run"
    );
    assert!(
        malformed(
            request
                .clone()
                .with_coordinates(
                    Some(run.clone()),
                    Some(CanonicalContextRevision::from_transcript_revision(
                        "other".into()
                    )),
                )
                .prepare(search(vec![ServerToolKind::WebSearch]))
        ),
        "another context revision"
    );
    let mut inference = search(vec![ServerToolKind::WebSearch]);
    inference.usage = ModelAuthorizationUse::Inference;
    let facts = OperationAuthorizationFacts {
        operation_id: OperationId::new(),
        execution_scope: OperationExecutionScope::Domain,
        run_id: Some(run.clone()),
        context_revision: None,
        operation: AuthorizationOperation::Model(inference),
    };
    let lookalike =
        WorkAuthorizationContext::new(recorder.clone(), OperationExecutionScope::Domain);
    assert!(
        malformed(attribution.bind_child(facts.clone(), &lookalike)),
        "an equal-looking work context"
    );
    let mut reused = facts.clone();
    reused.operation_id = parent.facts().operation_id.clone();
    assert!(
        malformed(attribution.bind_child(reused, &work)),
        "the parent's own operation id"
    );
    let mut other_scope = facts.clone();
    other_scope.execution_scope = OperationExecutionScope::RuntimeInput {
        owner_session_id: crate::SessionId::new(),
        runtime_epoch_id: crate::RuntimeEpochId::new(),
        submitted_input_id: crate::InputId::new(),
        canonical_input_id: crate::InputId::new(),
    };
    assert!(
        malformed(attribution.bind_child(other_scope, &work)),
        "another execution scope"
    );
    let mut tool = facts.clone();
    tool.operation = parent.facts().operation.clone();
    assert!(
        malformed(attribution.bind_child(tool, &work)),
        "not a model operation"
    );
    let mut compaction = facts;
    if let AuthorizationOperation::Model(model) = &mut compaction.operation {
        model.usage = ModelAuthorizationUse::Compaction;
    }
    assert!(
        malformed(attribution.bind_child(compaction, &work)),
        "another model usage"
    );
    assert!(
        recorder.models.lock().unwrap().is_empty(),
        "the work owner prepared nothing for refused delegate facts"
    );

    let prepared = request
        .prepare(search(vec![ServerToolKind::WebSearch]))
        .expect("the role's own facts");
    let bound = {
        let models = recorder.models.lock().unwrap();
        assert_eq!(models.len(), 1, "one owner preparation");
        models[0].clone()
    };
    assert!(bound.same_operation(prepared.binding()));
    let link = bound
        .delegate_attribution()
        .expect("the owner sees the attribution");
    assert!(link.parent_binding().same_operation(&parent));
    assert_eq!(link.role(), ToolDelegateRole::HostedSearchInference);
    assert!(bound.review_attribution().is_none());
    assert!(matches!(&bound.facts().operation,
        AuthorizationOperation::Model(model) if model.usage == ModelAuthorizationUse::Inference));
    assert_ne!(
        bound.facts().operation_id,
        parent.facts().operation_id,
        "a fresh operation id"
    );
    assert_eq!(bound.facts().run_id.as_ref(), Some(&run));
    assert_eq!(
        bound.facts().execution_scope,
        parent.facts().execution_scope
    );
    assert!(bound.facts().context_revision.is_none());
    request
        .prepare(search(vec![ServerToolKind::GoogleSearch]))
        .expect("a Gemini helper's search grounding is the same role");

    assert!(
        malformed(PreparedOperationCheck::prepare(
            lookalike,
            prepared.binding().clone()
        )),
        "a delegate is prepared only under its own work"
    );
    assert_eq!(recorder.models.lock().unwrap().len(), 2);
    assert!(
        malformed(prepared.prepare_for_resumed_run(work.clone(), &RunId::new())),
        "a delegate never moves to another run"
    );
    assert_eq!(recorder.models.lock().unwrap().len(), 2);
}

/// A parent call its work refuses never reaches its leaf: nothing is minted.
#[tokio::test]
async fn a_refused_parent_call_mints_no_delegate() {
    struct Refusing;
    impl WorkAuthorization for Refusing {
        fn prepare(
            &self,
            _binding: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Err(OperationRefused::new(OperationRefusalKind::Denied).into())
        }
    }
    let refusing =
        WorkAuthorizationContext::new(Arc::new(Refusing), OperationExecutionScope::Domain);
    assert!(
        matches!(
            __test_mint_tool_delegate(&refusing, None, ToolDelegateRole::HostedSearchInference)
                .await,
            Err(crate::ToolError::AuthorizationRefused { refusal })
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "the refused parent's own refusal, before any leaf"
    );
}
