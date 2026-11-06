//! [`ForkToolDispatcher`]: the `fork` tool of the main session's build.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentToolDispatcher, SessionId, ToolCallView, ToolDef, ToolDispatchContext, ToolError,
    ToolMutationClass, ToolResult,
};
use meerkat_mob::{
    AgentIdentity, BoundedResultSpec, DelegationExecutionRequest, DelegationExecutionService,
    DelegationMemberOptions, MobId,
};
use serde::Deserialize;

use super::context::{CONTEXT_BUDGET, prompt, render};
use super::{FORK, FORK_LABEL_PREFIX, FORK_SOURCE, MAX_RESULT_BYTES};
use crate::approval::ApprovalEngine;
use crate::bootstrap::TuiService;
use crate::mobs::Helpers;

/// Label of the child's bounded result.
const RESULT_LABEL: &str = "fork";

/// The profile of the implicit mob's helpers.
const DELEGATE_PROFILE: &str = "delegate";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct ForkArgs {
    #[schemars(
        description = "The one task the fork does. It starts with this conversation as context, so refer to it freely."
    )]
    task: String,
    #[schemars(description = "What the fork's final report should contain, and in what form.")]
    #[serde(default)]
    expected_output: Option<String>,
}

/// The `fork` tool: runs a child, which starts with the calling session's
/// conversation as context, as a helper of that session (see
/// [`crate::fork`]) and returns its final answer.
pub struct ForkToolDispatcher {
    helpers: Arc<Helpers>,
    service: Arc<TuiService>,
    engine: Arc<ApprovalEngine>,
    /// The number of the next fork child's label.
    next: AtomicU64,
    tools: Arc<[Arc<ToolDef>]>,
}

impl std::fmt::Debug for ForkToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForkToolDispatcher").finish_non_exhaustive()
    }
}

impl ForkToolDispatcher {
    /// A dispatcher running fork children through `helpers`, reading the
    /// caller's conversation from `service` and the request of its running
    /// turn from `engine` (the words the user submitted).
    #[must_use]
    pub fn new(
        helpers: Arc<Helpers>,
        service: Arc<TuiService>,
        engine: Arc<ApprovalEngine>,
    ) -> Self {
        let def = ToolDef::new(
            FORK,
            "Fork yourself: a child agent that starts with this conversation as its context \
             does one task (a side quest: an investigation, a review, an attempt you may \
             discard) and returns its final report as this tool's result. The child works in \
             the same project behind the same approval policy, cannot ask the user, and cannot \
             fork again. Give it one well-scoped task."
                .to_string(),
            meerkat_tools::schema_for::<ForkArgs>(),
        )
        .with_provenance(crate::tools::provenance(FORK_SOURCE));
        Self {
            helpers,
            service,
            engine,
            next: AtomicU64::new(1),
            tools: vec![Arc::new(def)].into(),
        }
    }

    /// A member identity no member of `mob` holds yet.
    async fn fresh_label(&self, mob: &MobId) -> AgentIdentity {
        let taken: Vec<AgentIdentity> = match self.helpers.state().handle_for(mob).await {
            Ok(handle) => handle
                .list_members()
                .await
                .into_iter()
                .map(|member| member.agent_identity)
                .collect(),
            Err(_) => Vec::new(),
        };
        loop {
            let n = self.next.fetch_add(1, Ordering::Relaxed);
            let identity = AgentIdentity::from(format!("{FORK_LABEL_PREFIX}{n}"));
            if !taken.contains(&identity) {
                return identity;
            }
        }
    }

    /// Run one fork child for `caller`; `Err` is the model-facing reason
    /// it did not run or did not finish.
    async fn fork(
        &self,
        caller: &SessionId,
        args: ForkArgs,
        current_turn: Option<String>,
    ) -> Result<String, String> {
        let model = self
            .helpers
            .registry()
            .model_of(caller)
            .ok_or_else(|| format!("session {caller} is not open in rkat-tui"))?;
        // The caller's turn is running: its committed snapshot, read
        // through the run's provisional tail (a reader's view).
        let session = self
            .service
            .load_committed_session_tolerating_tail(caller)
            .await
            .map_err(|err| format!("cannot read this conversation: {err}"))?
            .ok_or_else(|| format!("session {caller} has no committed conversation"))?;
        let conversation = render(session.messages(), current_turn.as_deref(), CONTEXT_BUDGET);
        let task = prompt(&conversation, &args.task, args.expected_output.as_deref());
        let state = self.helpers.state();
        let (mob, _) = state
            .ensure_implicit_mob_for_model(&caller.to_string(), &model, None)
            .await
            .map_err(|err| format!("cannot host the fork: {err}"))?;
        let identity = self.fresh_label(&mob).await;
        let handle = state
            .handle_for(&mob)
            .await
            .map_err(|err| format!("cannot reach the fork's mob: {err}"))?;
        let result_spec = BoundedResultSpec::new(RESULT_LABEL, MAX_RESULT_BYTES)
            .map_err(|err| format!("invalid fork result bound: {err}"))?;
        let mut member = DelegationMemberOptions::default();
        // The helper customizer would apply the same policy; naming it here
        // keeps `fork_off` out of the child whatever the defaults become.
        member.tool_access_policy = Some(crate::session::tool_access_policy());
        // The implicit mob's profile enables comms only (a `delegate` helper
        // inherits its parent's visible tools instead): a fork child works
        // like its parent, with the builtins and the shell.
        member.override_profile = Some(working_profile(handle.definition(), &identity)?);
        let mut request = DelegationExecutionRequest::new(identity.clone(), task, result_spec);
        request.member = member;
        let outcome = DelegationExecutionService::new(handle)
            .execute(request)
            .await
            .map_err(|err| format!("{identity} failed: {err}"))?;
        if let Some(err) = outcome.retirement_error() {
            tracing::warn!(fork = %identity, error = %err, "fork child not retired");
        }
        Ok(outcome.turn().result().result().text().to_string())
    }

    async fn dispatch_for(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != FORK {
            return Err(ToolError::NotFound {
                name: call.name.to_string(),
            });
        }
        let args: ForkArgs = call
            .parse_args()
            .map_err(|err| ToolError::InvalidArguments {
                name: call.name.to_string(),
                reason: err.to_string(),
            })?;
        let caller = context
            .and_then(ToolDispatchContext::origin_session_id)
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "fork needs the calling session".to_string(),
            })?;
        // The running turn's request is not committed yet: the newest
        // words the user submitted, as the approval gate recorded them.
        let current_turn = self.engine.latest_user_input(caller);
        let id = call.id.to_string();
        let result = match self.fork(caller, args, current_turn).await {
            Ok(text) => ToolResult::new(id, text, false),
            Err(reason) => ToolResult::new(id, reason, true),
        };
        Ok(ToolDispatchOutcome::from(result))
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ForkToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_mutation_class(&self, _tool_name: &str) -> ToolMutationClass {
        ToolMutationClass::Unknown
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_for(call, None).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_for(call, Some(context)).await
    }
}

/// The implicit mob's `delegate` profile with the builtins and the shell
/// enabled.
fn working_profile(
    definition: &meerkat_mob::MobDefinition,
    identity: &AgentIdentity,
) -> Result<meerkat_mob::Profile, String> {
    let binding = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from(DELEGATE_PROFILE))
        .ok_or_else(|| format!("{identity}: the implicit mob has no `{DELEGATE_PROFILE}` profile"))?;
    let meerkat_mob::ProfileBinding::Inline(profile) = binding else {
        return Err(format!(
            "{identity}: the implicit mob's `{DELEGATE_PROFILE}` profile is not inline"
        ));
    };
    let mut profile = profile.as_ref().clone();
    profile.tools.builtins = true;
    profile.tools.shell = true;
    Ok(profile)
}
