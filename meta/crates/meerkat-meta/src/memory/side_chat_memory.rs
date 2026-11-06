//! A side chat's memory stays its own.
//!
//! A durable fork seats its child with its source's per-spawn tool overlay,
//! and with agent memory configured that overlay carries the source's
//! recorder: the `memory` tool, bound to the source's identity. A console
//! side chat is one person's branch of that conversation, so its `memory`
//! writes must land in its own identity scope, never in its source's, which
//! everyone who may see the source reads. Meta rebinds the `memory` tool of
//! every build of a member that carries the side chat marker
//! ([`crate::side_chat::SIDE_CHAT_OF_LABEL`]) to that member's own identity,
//! over the overlay the build carries, at the one seam every member session
//! create passes (the pre-build service wrapper, which the fork's seating
//! and every rebuild of the side chat go through). The `fork_off` children of
//! a side chat carry the marker too, so their memory is theirs. The tools
//! list keeps its order and shape: a side chat whose overlay has no `memory`
//! tool (memory is not configured, the recorder is off, or the profile has
//! no memory) gains none. A forged marker on another member only binds that
//! member's memory tool to its own identity, as its own build does.
//!
//! The slot is late-bound: a composition fills it when its memory stack
//! attaches a recorder, as it fills the dispatch-time taint slot.

use std::sync::{Arc, RwLock};

use meerkat_core::service::CreateSessionRequest;

use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, MEMORY_TOOL_NAME, MemoryRecorder,
    RecorderToolDispatcher,
};

/// The memory surface a side chat's own recorder is built from: the
/// provider and configuration the composition's recorders use.
#[derive(Clone)]
struct SideChatRecorderSource {
    provider: Arc<dyn AgentMemoryProvider>,
    config: AgentMemoryConfig,
}

/// Late-bound recorder source for side chat builds; empty until the
/// composition's memory stack attaches a recorder.
#[derive(Clone, Default)]
pub(crate) struct SideChatMemorySlot {
    source: Arc<RwLock<Option<SideChatRecorderSource>>>,
}

impl std::fmt::Debug for SideChatMemorySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SideChatMemorySlot")
            .field("filled", &self.recorder_source().is_some())
            .finish()
    }
}

impl SideChatMemorySlot {
    /// Fill the slot with the provider and configuration the composition's
    /// member recorders use. A configuration whose recorder is off, or a
    /// provider without authored writes, gives side chats no recorder, as it
    /// gives no member one.
    pub(crate) fn fill(&self, provider: Arc<dyn AgentMemoryProvider>, config: AgentMemoryConfig) {
        if !(config.recorder_tool && provider.supports_authored_writes()) {
            return;
        }
        *self
            .source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(SideChatRecorderSource { provider, config });
    }

    fn recorder_source(&self) -> Option<SideChatRecorderSource> {
        self.source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// `hook`, then [`Self::bind_side_chat_memory`]: the pre-build hook of
    /// the base session-service layer, which every member create passes
    /// last, after the hooks of every layer around it.
    pub(crate) fn after_hook(
        &self,
        hook: crate::mob_handle_runtime::PreBuildHook,
    ) -> crate::mob_handle_runtime::PreBuildHook {
        let slot = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let slot = slot.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                slot.bind_side_chat_memory(req);
                Ok(())
            })
        })
    }

    /// Bind the `memory` tool of a side chat's build to the side chat's own
    /// identity (see the module docs). Nothing changes for a request that is
    /// not a member build, a member that carries no side chat marker, an
    /// overlay without a `memory` tool, or an unfilled slot.
    pub(crate) fn bind_side_chat_memory(&self, req: &mut CreateSessionRequest) {
        let is_side_chat = req
            .labels
            .as_ref()
            .is_some_and(|labels| labels.contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL));
        if !is_side_chat {
            return;
        }
        let Some(source) = self.recorder_source() else {
            return;
        };
        let Some(build) = req.build.as_mut() else {
            return;
        };
        let Some(binding) = build.mob_member_binding.as_ref() else {
            return;
        };
        let Some(overlay) = build.external_tools.as_ref() else {
            return;
        };
        if !overlay
            .tools()
            .iter()
            .any(|tool| tool.name.as_ref() == MEMORY_TOOL_NAME)
        {
            return;
        }
        let alias = crate::member_comms_id::logical_memory_identity(&binding.member);
        let identity = match crate::identity_first::AgentIdentity::parse(&alias) {
            Ok(identity) => identity,
            Err(error) => {
                // A console fork names its side chat with a validated public
                // alias, so this does not happen; if it did, the side chat
                // keeps none of its source's per-spawn tools rather than its
                // source's memory.
                tracing::warn!(
                    member = %binding.member,
                    %error,
                    "a side chat's inherited per-spawn tools are withheld: its identity fails \
                     memory-scope validation"
                );
                build.external_tools = None;
                return;
            }
        };
        let recorder = MemoryRecorder::new(
            source.provider,
            source.config,
            identity,
            Some(binding.mob_id.clone()),
        );
        build.external_tools = Some(Arc::new(RecorderToolDispatcher::rebinding(
            Arc::clone(overlay),
            recorder,
        )));
    }
}
