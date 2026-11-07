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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// A member build of `member` in mob `team`, with `labels` and `overlay`
    /// as its per-spawn tools.
    fn member_build(
        member: &str,
        labels: BTreeMap<String, String>,
        overlay: Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) -> CreateSessionRequest {
        CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::types::ContentInput::Text("hello".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: Some(meerkat_core::service::SessionBuildOptions {
                mob_member_binding: Some(meerkat_core::MobMemberBinding {
                    mob_id: "team".to_string(),
                    role: "worker".to_string(),
                    member: member.to_string(),
                }),
                external_tools: Some(overlay),
                ..Default::default()
            }),
            labels: Some(labels),
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        }
    }

    /// A private member's `memory` tool proposes nothing to its mob: its
    /// recorder runs inside no mob, so `propose_to_mob`, which would move its
    /// memory into a scope its mob shares, is refused. That holds for a
    /// private side chat and for a member a private member created.
    #[tokio::test]
    async fn a_private_members_memory_proposes_nothing_to_its_mob() {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.fill(Arc::clone(&provider), config.clone());
        let private_to = (
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "console-caller:v1:aa".to_string(),
        );
        for (member, labels) in [
            (
                "alpha-side",
                BTreeMap::from([
                    (
                        crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                        "alpha".to_string(),
                    ),
                    private_to.clone(),
                ]),
            ),
            ("alpha-side-helper", BTreeMap::from([private_to.clone()])),
        ] {
            let inherited: Arc<dyn meerkat_core::AgentToolDispatcher> =
                Arc::new(RecorderToolDispatcher::new(
                    None,
                    MemoryRecorder::new(
                        Arc::clone(&provider),
                        config.clone(),
                        crate::identity_first::AgentIdentity::parse(member).unwrap(),
                        Some("team".to_string()),
                    ),
                ));
            let mut req = member_build(member, labels, inherited);
            slot.bind_side_chat_memory(&mut req);
            let overlay = req
                .build
                .and_then(|build| build.external_tools)
                .expect("the build keeps a memory tool");
            let args = serde_json::value::RawValue::from_string(
                serde_json::json!({
                    "action": "propose_to_mob",
                    "title": "Shared fact",
                    "body": "For the mob.",
                })
                .to_string(),
            )
            .unwrap();
            let outcome = overlay
                .dispatch(meerkat_core::types::ToolCallView {
                    id: "call-1",
                    name: MEMORY_TOOL_NAME,
                    args: &args,
                })
                .await
                .unwrap();
            let text = outcome
                .result
                .content
                .iter()
                .filter_map(|block| match block {
                    meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                outcome.result.is_error && text.contains("not running inside a mob"),
                "{member} proposes nothing to its mob: {text}"
            );
        }
    }
}
