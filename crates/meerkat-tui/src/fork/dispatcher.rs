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
    DelegationMemberOptions, MobError, MobHandle, MobId,
};
use meerkat_mob::runtime::OwnerBridgeSessionLifecycleAuthority;
use meerkat_mob_mcp::ChildToolPolicyRefused;
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

    /// A member identity no member of `handle`'s mob holds yet, retiring
    /// members included (the counter restarts with every process, and a
    /// retiring child still holds its identity).
    async fn fresh_label(&self, handle: &MobHandle) -> AgentIdentity {
        let taken: Vec<AgentIdentity> = handle
            .list_members_including_retiring()
            .await
            .into_iter()
            .map(|member| member.agent_identity)
            .collect();
        loop {
            let n = self.next.fetch_add(1, Ordering::Relaxed);
            let identity = AgentIdentity::from(format!("{FORK_LABEL_PREFIX}{n}"));
            if !taken.contains(&identity) {
                return identity;
            }
        }
    }

    /// Run one fork child for `caller` and return its final answer.
    async fn fork(
        &self,
        caller: &SessionId,
        args: ForkArgs,
        current_turn: Option<String>,
    ) -> Result<String, ForkError> {
        let model = self
            .helpers
            .registry()
            .model_of(caller)
            .ok_or_else(|| ForkError::NotOpen(caller.clone()))?;
        // The caller's turn is running: its committed snapshot, read
        // through the run's provisional tail (a reader's view).
        let session = self
            .service
            .load_committed_session_tolerating_tail(caller)
            .await
            .map_err(|err| ForkError::Conversation(err.to_string()))?
            .ok_or_else(|| ForkError::NoConversation(caller.clone()))?;
        let tooling = ParentTooling::of(&session)?;
        let conversation = render(session.messages(), current_turn.as_deref(), CONTEXT_BUDGET);
        let task = prompt(&conversation, &args.task, args.expected_output.as_deref());
        let state = self.helpers.state();
        // The child runs in a child mob under the host's child policy:
        // refused before the implicit mob is created, as `delegate` is.
        state.admit_child_tool_policy()?;
        let (mob, _) = state
            .ensure_implicit_mob_for_model(&caller.to_string(), &model, None)
            .await
            .map_err(ForkError::Host)?;
        let handle = state.handle_for(&mob).await.map_err(ForkError::Host)?;
        // The mob under the implicit id must still be the caller's own
        // implicit mob: one registered under the id since (after a
        // destroy) is never acted on.
        if !is_implicit_mob_of(handle.owner_bridge_session_lifecycle_authority(), caller) {
            return Err(ForkError::NotCallersMob(mob));
        }
        let identity = self.fresh_label(&handle).await;
        let result_spec = BoundedResultSpec::new(RESULT_LABEL, MAX_RESULT_BYTES)
            .map_err(|err| ForkError::ResultBound(err.to_string()))?;
        let mut member = DelegationMemberOptions::default();
        // The helper customizer would apply the same policy; naming it here
        // keeps `fork_off` out of the child whatever the defaults become.
        member.tool_access_policy = Some(crate::session::tool_access_policy());
        // The implicit mob's profile enables comms only (a `delegate` helper
        // inherits its parent's visible tools instead): a fork child works
        // like its parent, with the builtins and the shell the parent has.
        member.override_profile = Some(working_profile(handle.definition(), &identity, tooling)?);
        let mut request = DelegationExecutionRequest::new(identity.clone(), task, result_spec);
        request.member = member;
        let outcome = DelegationExecutionService::new(handle)
            .execute(request)
            .await
            .map_err(|err| ForkError::Failed {
                identity: identity.clone(),
                reason: err.to_string(),
            })?;
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
            Err(err) => ToolResult::new(id, err.to_string(), true),
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

/// Why a fork did not run or did not finish; rendered as the model-facing
/// error result of the `fork` call.
#[derive(Debug, thiserror::Error)]
enum ForkError {
    #[error("session {0} is not open in rkat-tui")]
    NotOpen(SessionId),
    #[error("cannot read this conversation: {0}")]
    Conversation(String),
    #[error("session {0} has no committed conversation")]
    NoConversation(SessionId),
    #[error("cannot read this session's tool settings: {0}")]
    Tooling(#[source] serde_json::Error),
    #[error("cannot fork: {0}")]
    ChildPolicy(#[from] ChildToolPolicyRefused),
    #[error("cannot host the fork: {0}")]
    Host(#[source] MobError),
    #[error("cannot fork: mob {0} is no longer this session's own helper mob")]
    NotCallersMob(MobId),
    #[error("invalid fork result bound: {0}")]
    ResultBound(String),
    #[error("{identity}: {reason}")]
    Profile {
        identity: AgentIdentity,
        reason: String,
    },
    #[error("{identity} failed: {reason}")]
    Failed {
        identity: AgentIdentity,
        reason: String,
    },
}

/// Whether a mob with this owner authority is `caller`'s implicit
/// delegation mob (the engine's `delegate` check).
fn is_implicit_mob_of(
    authority: Option<OwnerBridgeSessionLifecycleAuthority>,
    caller: &SessionId,
) -> bool {
    authority.is_some_and(|authority| {
        authority.implicit_delegation_mob && authority.bridge_session_id == *caller
    })
}

/// The builtins and shell the calling session has: its persisted tooling
/// overrides resolved against the TUI factory's defaults (both on; see
/// `build_factory` in [`crate::bootstrap`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParentTooling {
    builtins: bool,
    shell: bool,
}

impl ParentTooling {
    fn of(session: &meerkat_core::Session) -> Result<Self, ForkError> {
        let tooling = session
            .try_session_metadata()
            .map_err(ForkError::Tooling)?
            .map(|metadata| metadata.tooling)
            .unwrap_or_default();
        Ok(Self::resolve(&tooling))
    }

    fn resolve(tooling: &meerkat_core::SessionTooling) -> Self {
        Self {
            builtins: tooling.builtins.resolve(true),
            shell: tooling.shell.resolve(true),
        }
    }
}

/// The implicit mob's `delegate` profile with the parent's builtins and
/// shell.
fn working_profile(
    definition: &meerkat_mob::MobDefinition,
    identity: &AgentIdentity,
    tooling: ParentTooling,
) -> Result<meerkat_mob::Profile, ForkError> {
    let profile_error = |reason: String| ForkError::Profile {
        identity: identity.clone(),
        reason,
    };
    let binding = definition
        .profiles
        .get(&meerkat_mob::ProfileName::from(DELEGATE_PROFILE))
        .ok_or_else(|| profile_error(format!("the implicit mob has no `{DELEGATE_PROFILE}` profile")))?;
    let meerkat_mob::ProfileBinding::Inline(profile) = binding else {
        return Err(profile_error(format!(
            "the implicit mob's `{DELEGATE_PROFILE}` profile is not inline"
        )));
    };
    let mut profile = profile.as_ref().clone();
    profile.tools.builtins = tooling.builtins;
    profile.tools.shell = tooling.shell;
    Ok(profile)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::ToolCategoryOverride;

    #[test]
    fn only_the_callers_own_implicit_mob_is_acted_on() {
        let caller = SessionId::new();
        let authority = |session: &SessionId, implicit: bool| {
            Some(OwnerBridgeSessionLifecycleAuthority {
                bridge_session_id: session.clone(),
                destroy_on_owner_archive: true,
                implicit_delegation_mob: implicit,
            })
        };
        assert!(is_implicit_mob_of(authority(&caller, true), &caller));
        // A squatter: another owner's mob, or the caller's explicit one.
        assert!(!is_implicit_mob_of(authority(&SessionId::new(), true), &caller));
        assert!(!is_implicit_mob_of(authority(&caller, false), &caller));
        assert!(!is_implicit_mob_of(None, &caller));
        let err = ForkError::NotCallersMob(MobId::from("implicit-x"));
        assert!(err.to_string().contains("no longer this session's own"), "{err}");
    }

    #[test]
    fn the_child_gets_the_parents_builtins_and_shell() {
        let mut tooling = meerkat_core::SessionTooling::default();
        assert_eq!(
            ParentTooling::resolve(&tooling),
            ParentTooling { builtins: true, shell: true }
        );
        tooling.shell = ToolCategoryOverride::Disable;
        assert_eq!(
            ParentTooling::resolve(&tooling),
            ParentTooling { builtins: true, shell: false }
        );
    }
}
