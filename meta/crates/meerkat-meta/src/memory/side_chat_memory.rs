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
//! A member private to a person ([`crate::access::PRIVATE_TO_LABEL`]: a
//! private side chat and every member it creates) keeps its memory to
//! itself the same way, and its recorder runs inside no mob: its
//! `propose_to_mob`, which would move its memory into a scope its mob
//! shares, is refused as the tool's call outside a mob.
//!
//! Where such a member's records are kept depends on what it is. A member
//! that is no registered agent's keeps them in the identity scope of its
//! owner and its name
//! ([`crate::memory::records::IdentityScopeKey::PrivateMember`]), which no
//! member of another owner and no member private to nobody is bound to,
//! whatever its name. A registered agent's member keeps them under the
//! agent's name, as every reader of an agent's memory expects. Core's
//! identity entries tell the two apart, and so does a roster id that is a
//! generated runtime alias of an identity, which no other member can hold.
//! No label does: Core's build of a member stamps `agent_identity` with the
//! member's own id on every member's build request, so there the label
//! names the member being built, whatever it is. Where the composition has
//! an identity layer whose reader is not bound yet, the member's `memory`
//! tool answers that memory is not ready and reads and writes nothing, and
//! the first call after the reader is bound decides for the build.
//!
//! A temporary council's participant forked from a side chat or from a
//! member private to a person keeps no memory of its own: its `memory` tool
//! stays on its tools list, in its place, and answers every action with an
//! error. Its name is the same in every council, so a scope under that name
//! would be every council's.
//!
//! The slot is late-bound: a composition fills it when its memory stack
//! attaches a recorder, as it fills the dispatch-time taint slot.

use std::sync::{Arc, RwLock};

use meerkat_core::service::CreateSessionRequest;

use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, COUNCIL_PARTICIPANT_KEEPS_NO_MEMORY, MEMORY_TOOL_NAME,
    MemberMemoryKept, MemoryRecorder, RecorderToolDispatcher,
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
    /// What tells a registered agent's member from every other member.
    entries: Arc<RwLock<IdentityEntriesReader>>,
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
    /// identity (see the module docs), and the `memory` tool of a member
    /// private to a person to a recorder inside no mob: its
    /// `propose_to_mob`, which would move its memory into a scope its mob
    /// shares, is refused. A private member that is no registered agent's
    /// keeps its records under its owner and its name, and a temporary
    /// council's participant keeps none (see the module docs). Nothing
    /// changes for a request that is not a member build, a member that is
    /// neither a side chat nor private, an overlay without a `memory` tool,
    /// or an unfilled slot.
    pub(crate) fn bind_side_chat_memory(&self, req: &mut CreateSessionRequest) {
        let is_side_chat = req
            .labels
            .as_ref()
            .is_some_and(|labels| labels.contains_key(crate::side_chat::SIDE_CHAT_OF_LABEL));
        let owner = req
            .labels
            .as_ref()
            .and_then(crate::access::private_owner)
            .map(str::to_string);
        if !is_side_chat && owner.is_none() {
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
        // A temporary council's participant is a fork seated in the
        // council's own mob: its fork source names a member of another mob.
        let council_participant = build
            .fork_source
            .as_ref()
            .is_some_and(|fork| fork.source_member.mob_id != binding.mob_id);
        let recorder = MemoryRecorder::new(
            source.provider,
            source.config,
            identity,
            owner.is_none().then(|| binding.mob_id.clone()),
        );
        let recorder = match owner {
            _ if council_participant => recorder.kept(MemberMemoryKept::Nowhere(
                COUNCIL_PARTICIPANT_KEEPS_NO_MEMORY,
            )),
            None => recorder,
            Some(owner) => {
                let entries = self.identity_entries();
                let member_id = meerkat_mob::ids::AgentIdentity::from(binding.member.as_str());
                recorder.kept_as_decided_later(Arc::new(move || {
                    entries
                        .names_registered_agent(&member_id)
                        .map(|registered| {
                            if registered {
                                MemberMemoryKept::UnderItsName
                            } else {
                                MemberMemoryKept::UnderItsOwner(owner.clone())
                            }
                        })
                }))
            }
        };
        build.external_tools = Some(Arc::new(RecorderToolDispatcher::rebinding(
            Arc::clone(overlay),
            recorder,
        )));
    }

    /// From now on this slot's composition has an identity layer whose
    /// reader of Core's identity entries is not bound yet: until
    /// [`Self::watch_identity_entries`] binds it, a member private to a
    /// person whose roster id is no generated runtime alias cannot be told
    /// from a registered agent, and its `memory` tool answers that memory
    /// is not ready. A reader bound before stays bound.
    pub(crate) fn await_identity_entries(&self) {
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*entries, IdentityEntriesReader::NoIdentityLayer) {
            *entries = IdentityEntriesReader::Awaited;
        }
    }

    /// From now on whether Core holds a member as an identity entry is
    /// read through `bridge` (a read of Core's published machine state: no
    /// store read, no actor command, no wake).
    pub(crate) fn watch_identity_entries(
        &self,
        bridge: &Arc<dyn crate::identity_first::SessionBridge>,
    ) {
        *self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            IdentityEntriesReader::Bridge(Arc::downgrade(bridge));
    }

    /// From now on the members of `members` are the ones Core is taken to
    /// hold as identity entries: what a test lends the slot in place of a
    /// mob.
    #[cfg(test)]
    pub(crate) fn hold_identity_entries(
        &self,
        members: std::collections::BTreeSet<meerkat_mob::ids::AgentIdentity>,
    ) {
        *self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            IdentityEntriesReader::Fixed(members);
    }

    fn identity_entries(&self) -> IdentityEntriesSlot {
        IdentityEntriesSlot(Arc::clone(&self.entries))
    }
}

/// What tells a registered agent's member from every other member for a
/// composition: Core's identity entries, where the composition has an
/// identity layer.
#[derive(Clone, Default)]
enum IdentityEntriesReader {
    /// The composition has no identity layer: no member is a registered
    /// agent's.
    #[default]
    NoIdentityLayer,
    /// The composition has an identity layer whose reader is not bound yet.
    Awaited,
    /// Core's identity entries, read through the identity runtime's session
    /// bridge.
    Bridge(std::sync::Weak<dyn crate::identity_first::SessionBridge>),
    /// A fixed set of roster member ids, in place of a mob.
    #[cfg(test)]
    Fixed(std::collections::BTreeSet<meerkat_mob::ids::AgentIdentity>),
}

/// A slot's [`IdentityEntriesReader`], read at each `memory` call that
/// still has to decide where a member's records are kept.
#[derive(Clone)]
struct IdentityEntriesSlot(Arc<RwLock<IdentityEntriesReader>>);

impl IdentityEntriesSlot {
    /// Whether the member with roster id `member_id` is a registered
    /// agent's member; `None` while that cannot be told: the composition
    /// has an identity layer, and Core's entries cannot be read yet or can
    /// no longer be read.
    ///
    /// A registered agent's member is one whose roster id is a generated
    /// runtime alias of an identity (a namespace the creation of no other
    /// member is admitted under), or one Core holds as an identity entry.
    /// The labels of the member's build request are not asked: Core stamps
    /// `agent_identity` with the member's own id on every member's build
    /// request, so there the label is no mark of a registered agent.
    fn names_registered_agent(&self, member_id: &meerkat_mob::ids::AgentIdentity) -> Option<bool> {
        use crate::member_comms_id::{durable_identity_from_runtime_alias, runtime_alias_str};
        if durable_identity_from_runtime_alias(&runtime_alias_str(member_id.as_str())).is_some() {
            return Some(true);
        }
        let reader = self
            .0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match reader {
            IdentityEntriesReader::NoIdentityLayer => Some(false),
            IdentityEntriesReader::Awaited => None,
            IdentityEntriesReader::Bridge(bridge) => bridge.upgrade().map(|bridge| {
                crate::identity_first::SessionBridge::holds_identity_entry(&*bridge, member_id)
            }),
            #[cfg(test)]
            IdentityEntriesReader::Fixed(members) => Some(members.contains(member_id)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    /// `req` with the standard member labels of its own binding stamped
    /// over its labels, as Core's build of a member stamps them on every
    /// member's build request: `mob_id`, `role`, `profile_name`, and
    /// `meerkat_id` and `agent_identity`, which name the member itself.
    fn as_core_builds_it(mut req: CreateSessionRequest) -> CreateSessionRequest {
        let Some(binding) = req
            .build
            .as_ref()
            .and_then(|build| build.mob_member_binding.clone())
        else {
            return req;
        };
        let labels = req.labels.get_or_insert_with(BTreeMap::new);
        for (key, value) in [
            ("mob_id", &binding.mob_id),
            ("role", &binding.role),
            ("profile_name", &binding.role),
            ("meerkat_id", &binding.member),
            ("agent_identity", &binding.member),
        ] {
            labels.insert(key.to_string(), value.clone());
        }
        req
    }

    /// A member build of `member` in mob `team`, with `labels` as its
    /// roster labels and `overlay` as its per-spawn tools.
    fn member_build(
        member: &str,
        labels: BTreeMap<String, String>,
        overlay: Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) -> CreateSessionRequest {
        as_core_builds_it(CreateSessionRequest {
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
        })
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

    // ---- where a member keeps its memory ----

    use crate::memory::capabilities::MemoryPanelStore as _;
    use crate::memory::records::{MemoryAuthor, MemoryRecord, MemoryScope, RecordStatus};

    const ALICE: &str = "console-caller:v1:aa";
    const BOB: &str = "console-caller:v1:bb";
    /// The member name the rows reuse.
    const NAME: &str = "ledger-side";

    /// A store, as a provider, and a slot filled with it.
    struct Memory {
        _dir: tempfile::TempDir,
        store: Arc<crate::memory::SqliteAgentMemoryStore>,
        provider: Arc<dyn AgentMemoryProvider>,
        config: AgentMemoryConfig,
        slot: SideChatMemorySlot,
    }

    impl Memory {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
            let provider: Arc<dyn AgentMemoryProvider> = store.clone();
            let config = AgentMemoryConfig::default();
            let slot = SideChatMemorySlot::default();
            slot.fill(Arc::clone(&provider), config.clone());
            Self {
                _dir: dir,
                store,
                provider,
                config,
                slot,
            }
        }

        /// The build of `member` in mob `mob` with `labels`, over an
        /// inherited overlay whose `memory` tool is bound to
        /// `inherited_from`, as the seam leaves it.
        fn build(
            &self,
            mob: &str,
            member: &str,
            labels: BTreeMap<String, String>,
            inherited_from: &str,
        ) -> CreateSessionRequest {
            let inherited: Arc<dyn meerkat_core::AgentToolDispatcher> =
                Arc::new(RecorderToolDispatcher::new(
                    None,
                    MemoryRecorder::new(
                        Arc::clone(&self.provider),
                        self.config.clone(),
                        crate::identity_first::AgentIdentity::parse(inherited_from).unwrap(),
                        Some(mob.to_string()),
                    ),
                ));
            let mut req = member_build(member, labels, inherited);
            if let Some(binding) = req
                .build
                .as_mut()
                .and_then(|build| build.mob_member_binding.as_mut())
            {
                binding.mob_id = mob.to_string();
            }
            as_core_builds_it(req)
        }

        /// The `memory` tool of that build.
        fn tool(
            &self,
            mob: &str,
            member: &str,
            labels: BTreeMap<String, String>,
            inherited_from: &str,
        ) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
            let mut req = self.build(mob, member, labels, inherited_from);
            self.slot.bind_side_chat_memory(&mut req);
            req.build
                .and_then(|build| build.external_tools)
                .expect("the build keeps a memory tool")
        }

        /// Every record the store holds, of every scope and status.
        async fn records(&self) -> Vec<MemoryRecord> {
            self.store
                .records_page(&self.config.realm, None, None, None, 100, None)
                .await
                .unwrap()
                .records
        }

        fn scope(&self, key: &str) -> MemoryScope {
            MemoryScope::Identity {
                realm: self.config.realm.clone(),
                identity: key.to_string(),
            }
        }
    }

    /// The labels of a side chat of `alpha` private to `owner`.
    fn private_side_chat(owner: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                "alpha".to_string(),
            ),
            (
                crate::access::PRIVATE_TO_LABEL.to_string(),
                owner.to_string(),
            ),
        ])
    }

    /// One `memory` call: whether it failed, and its text.
    async fn call(
        tool: &Arc<dyn meerkat_core::AgentToolDispatcher>,
        args: serde_json::Value,
    ) -> (bool, String) {
        let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        let outcome = tool
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
        (outcome.result.is_error, text)
    }

    fn remember(fact: &str) -> serde_json::Value {
        serde_json::json!({
            "action": "remember",
            "title": format!("Where {fact} is"),
            "body": format!("{fact} is at the north pond."),
        })
    }

    /// A private member's records lie in the scope of its owner and its
    /// name; the call's operation target names that scope; the record's
    /// author is the member's name.
    #[tokio::test]
    async fn a_private_members_memory_is_kept_under_its_owner_and_its_name() {
        let memory = Memory::new();
        let tool = memory.tool("team", NAME, private_side_chat(ALICE), "alpha");
        let (failed, text) = call(&tool, remember("the heron")).await;
        assert!(!failed, "the member stores its note: {text}");

        let records = memory.records().await;
        assert_eq!(records.len(), 1, "one record is stored");
        let scope = memory.scope("private/console-caller:v1:aa/ledger-side");
        assert_eq!(records[0].scope, scope);
        assert_eq!(
            records[0].provenance.author,
            MemoryAuthor::Agent {
                identity: NAME.to_string()
            }
        );

        let args =
            serde_json::value::RawValue::from_string(remember("the heron").to_string()).unwrap();
        let plan = tool
            .resolve_execution_plan(
                meerkat_core::types::ToolCallView {
                    id: "call-2",
                    name: MEMORY_TOOL_NAME,
                    args: &args,
                },
                &meerkat_core::ToolDispatchContext::default(),
                &crate::tool_application_test_support::resolution(),
            )
            .unwrap();
        let targets: Vec<_> = plan
            .owner_witnesses()
            .iter()
            .filter_map(|witness| witness.operation_target().cloned())
            .collect();
        assert_eq!(
            targets,
            vec![meerkat_core::ToolOperationTarget::MemoryOperation {
                scope: meerkat_core::MemoryScopeRef::Host {
                    owner: "meta.agent_memory".to_string(),
                    scope: format!(
                        "identity/{}/private%2Fconsole-caller:v1:aa%2Fledger-side",
                        memory.config.realm
                    ),
                },
                operation: meerkat_core::MemoryOperation::Write,
            }]
        );
    }

    /// An owner value is escaped into the key as the scope reference
    /// escapes its parts, after the label's own trim.
    #[tokio::test]
    async fn an_owner_value_with_a_slash_a_percent_and_outer_spaces_keys_one_scope() {
        let memory = Memory::new();
        let tool = memory.tool("team", NAME, private_side_chat("  a/b%c  "), "alpha");
        let (failed, text) = call(&tool, remember("the heron")).await;
        assert!(!failed, "the member stores its note: {text}");
        let records = memory.records().await;
        assert_eq!(records.len(), 1, "one record is stored");
        assert_eq!(
            records[0].scope,
            memory.scope("private/a%2Fb%25c/ledger-side")
        );
    }

    /// Two live members of one name, each private to another person, in two
    /// mobs of one memory realm: neither recalls, changes or removes a
    /// record of the other.
    #[tokio::test]
    async fn members_of_two_owners_under_one_name_keep_their_memory_apart_while_both_live() {
        let memory = Memory::new();
        let alices = memory.tool("team-a", NAME, private_side_chat(ALICE), "alpha");
        let bobs = memory.tool("team-b", NAME, private_side_chat(BOB), "alpha");
        let (failed, text) = call(&alices, remember("the heron")).await;
        assert!(!failed, "the first member stores its note: {text}");
        let (failed, text) = call(&bobs, remember("the otter")).await;
        assert!(!failed, "the second member stores its note: {text}");

        let records = memory.records().await;
        let id_of = |fact: &str| {
            records
                .iter()
                .find(|record| record.body.contains(fact))
                .map(|record| record.id.clone())
                .expect("the note is stored")
        };
        let (herons, otters) = (id_of("the heron"), id_of("the otter"));

        for (who, tool, own, others, others_id) in [
            ("the first", &alices, "the heron", "the otter", &otters),
            ("the second", &bobs, "the otter", "the heron", &herons),
        ] {
            let (failed, text) = call(tool, serde_json::json!({ "action": "recall" })).await;
            assert!(
                !failed && text.contains(own) && !text.contains(others),
                "{who} member recalls its own note and not the other's: {text}"
            );
            let (failed, text) = call(
                tool,
                serde_json::json!({
                    "action": "update",
                    "memory_id": others_id,
                    "title": "Changed",
                    "body": "Changed by the other member.",
                }),
            )
            .await;
            assert!(failed, "{who} member changes no note of the other: {text}");
            let (failed, text) = call(
                tool,
                serde_json::json!({ "action": "forget", "memory_id": others_id }),
            )
            .await;
            assert!(failed, "{who} member removes no note of the other: {text}");
        }

        let records = memory.records().await;
        assert_eq!(records.len(), 2, "no record was added");
        assert!(
            records
                .iter()
                .all(|record| record.status == RecordStatus::Active && record.supersedes.is_none()),
            "both notes stand as their members left them: {records:#?}"
        );
    }

    /// A private member recalls, changes and removes no record kept under
    /// its bare name.
    #[tokio::test]
    async fn a_private_member_reads_no_record_kept_under_its_bare_name() {
        let memory = Memory::new();
        let kept = memory
            .provider
            .remember(
                &memory.config.realm,
                &crate::identity_first::AgentIdentity::parse(NAME).unwrap(),
                crate::identity_first::agent_memory::NewAgentMemory {
                    title: "Where the heron is".to_string(),
                    body: "the heron is at the north pond.".to_string(),
                    tags: Vec::new(),
                },
            )
            .await
            .unwrap();
        let tool = memory.tool("team", NAME, private_side_chat(ALICE), "alpha");

        let (failed, text) = call(&tool, serde_json::json!({ "action": "recall" })).await;
        assert!(
            !failed && !text.contains("the heron"),
            "the member recalls nothing kept under its bare name: {text}"
        );
        let (failed, text) = call(
            &tool,
            serde_json::json!({
                "action": "update",
                "memory_id": kept.memory_id,
                "title": "Changed",
                "body": "Changed by the private member.",
            }),
        )
        .await;
        assert!(
            failed,
            "the member changes nothing kept under its bare name: {text}"
        );
        let (failed, text) = call(
            &tool,
            serde_json::json!({ "action": "forget", "memory_id": kept.memory_id }),
        )
        .await;
        assert!(
            failed,
            "the member removes nothing kept under its bare name: {text}"
        );

        let records = memory.records().await;
        assert_eq!(records.len(), 1, "no record was added");
        assert_eq!(records[0].scope, memory.scope(NAME));
        assert_eq!(records[0].status, RecordStatus::Active);
    }

    /// The write gate is asked about a private member's write under the
    /// member's name, so a write it holds lands quarantined.
    #[tokio::test]
    async fn a_tainted_private_members_write_lands_quarantined() {
        use crate::memory::TaintableStore as _;

        struct HoldsTheMember;
        impl crate::memory::taint::LlmWriteGate for HoldsTheMember {
            fn quarantine_reason(
                &self,
                author: &MemoryAuthor,
                _kind: crate::memory::staged::StagedBatchKind,
                _evidence: &[crate::memory::records::EvidenceRef],
            ) -> Option<String> {
                (*author
                    == MemoryAuthor::Agent {
                        identity: NAME.to_string(),
                    })
                .then(|| "the member read untrusted content".to_string())
            }
        }

        let memory = Memory::new();
        memory.store.set_llm_write_gate(Arc::new(HoldsTheMember));
        let tool = memory.tool("team", NAME, private_side_chat(ALICE), "alpha");
        let (failed, text) = call(&tool, remember("the heron")).await;
        assert!(
            !failed && text.contains("QUARANTINED"),
            "the write is stored quarantined: {text}"
        );
        let records = memory.records().await;
        assert_eq!(records.len(), 1, "one record is stored");
        assert!(
            matches!(records[0].status, RecordStatus::Quarantined { .. }),
            "{:?}",
            records[0].status
        );
    }

    /// A temporary council's participant is a fork seated in the council's
    /// own mob: its fork source names a member of another mob. Forked from a
    /// side chat, it keeps no memory under its own name; its `memory` tool
    /// stays on its tools list and answers every action with an error.
    #[tokio::test]
    async fn a_council_participant_forked_from_a_side_chat_keeps_no_memory_under_its_own_name() {
        let memory = Memory::new();
        for labels in [
            BTreeMap::from([(
                crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                "alpha".to_string(),
            )]),
            private_side_chat(ALICE),
        ] {
            let mut req = memory.build("council-1", "council-p1", labels, "alpha-side");
            if let Some(build) = req.build.as_mut() {
                build.fork_source = Some(meerkat_core::ForkBuildSource::new(
                    meerkat_core::MobMemberBinding {
                        mob_id: "team".to_string(),
                        role: "worker".to_string(),
                        member: "alpha-side".to_string(),
                    },
                    meerkat_core::types::SessionId::new(),
                ));
            }
            memory.slot.bind_side_chat_memory(&mut req);
            let tool = req
                .build
                .and_then(|build| build.external_tools)
                .expect("the build keeps its tools");
            assert!(
                tool.tools()
                    .iter()
                    .any(|tool| tool.name.as_ref() == MEMORY_TOOL_NAME),
                "the memory tool stays on the tools list"
            );
            for args in [
                remember("the heron"),
                serde_json::json!({ "action": "recall" }),
                serde_json::json!({ "action": "forget", "memory_id": "mem-1" }),
            ] {
                let (failed, text) = call(&tool, args.clone()).await;
                assert!(
                    failed && text.contains("a council participant keeps no memory of its own"),
                    "{args} is answered with the error: {text}"
                );
            }
        }
        assert_eq!(
            memory.records().await.len(),
            0,
            "nothing is stored for a council participant"
        );
    }

    /// A member that is a side chat private to nobody keeps its memory under
    /// its bare name, and a later such member of that name continues it.
    #[tokio::test]
    async fn a_shared_members_memory_follows_its_name() {
        let memory = Memory::new();
        let shared = || {
            BTreeMap::from([(
                crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                "alpha".to_string(),
            )])
        };
        let first = memory.tool("team", NAME, shared(), "alpha");
        let (failed, text) = call(&first, remember("the heron")).await;
        assert!(!failed, "the first member stores its note: {text}");
        let records = memory.records().await;
        assert_eq!(records.len(), 1, "one record is stored");
        assert_eq!(records[0].scope, memory.scope(NAME));

        let later = memory.tool("team", NAME, shared(), "alpha");
        let (failed, text) = call(&later, serde_json::json!({ "action": "recall" })).await;
        assert!(
            !failed && text.contains("the heron"),
            "a later shared member of the name recalls the note: {text}"
        );
    }

    /// No member name is a scope kept under an owner: a member named as
    /// that shape gets no memory binding, private or not.
    #[tokio::test]
    async fn no_name_reaches_a_scope_kept_under_an_owner() {
        let memory = Memory::new();
        let shaped = "private/console-caller:v1:aa/ledger-side";
        assert!(crate::identity_first::AgentIdentity::parse(shaped).is_err());
        for labels in [
            private_side_chat(BOB),
            BTreeMap::from([(
                crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
                "alpha".to_string(),
            )]),
        ] {
            let mut req = memory.build("team", shaped, labels, "alpha");
            memory.slot.bind_side_chat_memory(&mut req);
            assert!(
                req.build
                    .is_some_and(|build| build.external_tools.is_none()),
                "a member named as the shape keeps no memory tool"
            );
        }
        assert_eq!(memory.records().await.len(), 0);
    }

    /// Where the composition has an identity layer whose reader of Core's
    /// identity entries is not bound yet, a private member's `memory` tool
    /// answers that memory is not ready, and nothing is read or written
    /// under the member's name or under its owner. The first call after the
    /// reader is bound decides where the member's records are kept, for the
    /// build.
    #[tokio::test]
    async fn a_private_member_built_before_the_identity_reader_is_bound_stores_nothing_until_it_is()
    {
        let memory = Memory::new();
        memory
            .provider
            .remember(
                &memory.config.realm,
                &crate::identity_first::AgentIdentity::parse(NAME).unwrap(),
                crate::identity_first::agent_memory::NewAgentMemory {
                    title: "Where the otter is".to_string(),
                    body: "the otter is at the weir.".to_string(),
                    tags: Vec::new(),
                },
            )
            .await
            .unwrap();
        memory.slot.await_identity_entries();
        let tool = memory.tool("team", NAME, private_side_chat(ALICE), "alpha");
        for args in [
            remember("the heron"),
            serde_json::json!({ "action": "recall" }),
        ] {
            let (failed, text) = call(&tool, args.clone()).await;
            assert!(
                failed && text.contains("memory is not ready yet") && !text.contains("the otter"),
                "{args} is answered as not ready: {text}"
            );
        }
        assert_eq!(memory.records().await.len(), 1, "nothing was stored");

        // The reader is bound, and Core holds no identity entry for the
        // member: its records are kept under its owner.
        memory
            .slot
            .hold_identity_entries(std::collections::BTreeSet::new());
        let (failed, text) = call(&tool, remember("the heron")).await;
        assert!(!failed, "the member stores its note: {text}");
        // What the reader answers later changes nothing for this build.
        memory
            .slot
            .hold_identity_entries(std::collections::BTreeSet::from([
                meerkat_mob::ids::AgentIdentity::from(NAME),
            ]));
        let (failed, text) = call(&tool, remember("the wren")).await;
        assert!(!failed, "the member stores a second note: {text}");
        let kept = memory.scope("private/console-caller:v1:aa/ledger-side");
        let records = memory.records().await;
        for fact in ["the heron", "the wren"] {
            assert!(
                records
                    .iter()
                    .any(|record| record.body.contains(fact) && record.scope == kept),
                "the note on {fact} is kept under the member's owner: {records:#?}"
            );
        }
    }

    /// A registered agent that is private to a person keeps its memory
    /// under its name: the member Core holds as an identity entry, and the
    /// member whose roster id is a generated runtime alias of an identity,
    /// which no other member can hold, also before the reader of Core's
    /// entries is bound.
    #[tokio::test]
    async fn a_registered_agent_keeps_its_memory_under_its_name() {
        let private_to = || {
            (
                crate::access::PRIVATE_TO_LABEL.to_string(),
                ALICE.to_string(),
            )
        };
        let by_entry = Memory::new();
        by_entry
            .slot
            .hold_identity_entries(std::collections::BTreeSet::from([
                meerkat_mob::ids::AgentIdentity::from(NAME),
            ]));
        let by_alias = Memory::new();
        by_alias.slot.await_identity_entries();
        let alias = crate::member_comms_id::mob_member_id(&format!("rt:{NAME}:1"));
        for (memory, member) in [(&by_entry, NAME), (&by_alias, alias.as_str())] {
            let tool = memory.tool("team", member, BTreeMap::from([private_to()]), NAME);
            let (failed, text) = call(&tool, remember("the heron")).await;
            assert!(!failed, "the agent stores its note: {text}");
            let records = memory.records().await;
            assert_eq!(records.len(), 1, "one record is stored");
            assert_eq!(records[0].scope, memory.scope(NAME));
            let (failed, text) = call(
                &tool,
                serde_json::json!({
                    "action": "propose_to_mob",
                    "title": "Shared fact",
                    "body": "For the mob.",
                }),
            )
            .await;
            assert!(
                failed && text.contains("not running inside a mob"),
                "a private agent proposes nothing to its mob: {text}"
            );
        }
    }

    /// A provider that implements no recall in a given scope answers a
    /// private member's recall as unsupported: never with the records under
    /// the member's bare name, which its plain recall would return.
    #[tokio::test]
    async fn a_provider_without_a_recall_in_a_given_scope_answers_a_private_members_recall_as_unsupported()
     {
        use crate::identity_first::agent_memory::{
            AgentMemoryError, AgentMemoryForgetResult, AgentMemoryRecallRequest, AgentMemoryRecord,
            AuthoredWriteReceipt,
        };
        use crate::memory::records::NewMemoryRecord;

        /// The bundled store's recall and authored writes, and nothing else.
        struct NoScopedRecall(Arc<crate::memory::SqliteAgentMemoryStore>);

        #[async_trait::async_trait]
        impl AgentMemoryProvider for NoScopedRecall {
            async fn recall(
                &self,
                request: AgentMemoryRecallRequest,
            ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
                self.0.recall(request).await
            }

            async fn remember_authored(
                &self,
                scope: &MemoryScope,
                record: NewMemoryRecord,
                author: MemoryAuthor,
            ) -> Result<AuthoredWriteReceipt, AgentMemoryError> {
                self.0.remember_authored(scope, record, author).await
            }

            async fn forget_authored(
                &self,
                scope: &MemoryScope,
                memory_id: &str,
                author: MemoryAuthor,
            ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
                self.0.forget_authored(scope, memory_id, author).await
            }

            fn supports_authored_writes(&self) -> bool {
                true
            }
        }

        let memory = Memory::new();
        memory
            .provider
            .remember(
                &memory.config.realm,
                &crate::identity_first::AgentIdentity::parse(NAME).unwrap(),
                crate::identity_first::agent_memory::NewAgentMemory {
                    title: "Where the otter is".to_string(),
                    body: "the otter is at the weir.".to_string(),
                    tags: Vec::new(),
                },
            )
            .await
            .unwrap();
        let slot = SideChatMemorySlot::default();
        slot.fill(
            Arc::new(NoScopedRecall(Arc::clone(&memory.store))),
            memory.config.clone(),
        );
        let mut req = memory.build("team", NAME, private_side_chat(ALICE), "alpha");
        slot.bind_side_chat_memory(&mut req);
        let tool = req
            .build
            .and_then(|build| build.external_tools)
            .expect("the build keeps a memory tool");

        let (failed, text) = call(&tool, remember("the heron")).await;
        assert!(!failed, "the member stores its note: {text}");
        let (failed, text) = call(&tool, serde_json::json!({ "action": "recall" })).await;
        assert!(
            failed && text.contains("unsupported") && !text.contains("the otter"),
            "the recall is answered as unsupported: {text}"
        );
    }
}
