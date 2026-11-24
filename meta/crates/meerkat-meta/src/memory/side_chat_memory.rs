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
//! a side chat carry the marker too, so their memory is theirs.
//!
//! The rebinding is late-bound. The overlay a side chat is built over is its
//! source's, and its source's build publishes into it at any time: after a
//! restart an agent's tools are published in the new process only once the
//! agent is built there, which may be after its side chat's build. And a
//! side chat may be built before its composition has bound its recorder at
//! all: a member that owes work when its mob starts is built while the mob
//! starts. So the seam never looks at what the overlay holds, or at what is
//! bound, when the side chat is built. In a composition that says it binds
//! a recorder ([`SideChatMemorySlot::expect_recorder`], or a fill), a side
//! chat's overlay is always wrapped ([`RecorderToolDispatcher::rebinding`]),
//! and the wrapper decides at every read of the side chat's tools:
//!
//! - it never passes a `memory` tool of the overlay through;
//! - while the recorder is not bound yet, it offers no `memory` tool;
//! - once it is bound, it offers the side chat's own `memory` tool when its
//!   source's build has one, by the source's own gate
//!   ([`SideChatMemoryGate`]): whether or not its source was built in this
//!   process yet, and nothing wakes the source for it;
//! - else it offers the side chat's own while the overlay holds a `memory`
//!   tool (a tool of that name a host gave the source).
//!
//! The gate is the source's: where an agent's recorder is attached per
//! profile, a side chat is offered `memory` up front only when the member
//! it was forked from is a registered agent's member (Core's identity
//! entries tell, as below) on a profile that enables agent memory. A side
//! chat of any other member, which has no recorder of its own, is offered
//! `memory` only while the tools it carries hold one.
//!
//! A side chat whose source's build has no `memory` tool (memory is not
//! configured, the recorder is off, or the source's profile has no agent
//! memory) gains none. A forged marker on another member only binds that
//! member's memory tool to its own identity, as its own build does.
//!
//! A member private to a person ([`crate::access::PRIVATE_TO_LABEL`]: a
//! private side chat and every member it creates) keeps its memory to
//! itself the same way, and its recorder runs inside no mob: its
//! `propose_to_mob`, which would move its memory into a scope its mob
//! shares, is refused as the tool's call outside a mob. A private member
//! that is no side chat keeps a `memory` tool only while the tools it
//! carries hold one.
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
//! attaches a recorder, as it fills the dispatch-time taint slot, and the
//! mob's bootstrap binds the definition the mob runs.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};

use meerkat_core::service::CreateSessionRequest;

use crate::identity_first::agent_memory::{
    AgentMemoryConfig, AgentMemoryProvider, COUNCIL_PARTICIPANT_KEEPS_NO_MEMORY, MemberMemoryKept,
    MemoryRecorder, ReboundMemory, ReboundRecorder, RecorderToolDispatcher,
    definition_profile_enables_agent_memory,
};

/// Whether the build of a side chat's source has the recorder: the gate the
/// composition's own recorders are attached by.
#[derive(Clone)]
pub(crate) enum SideChatMemoryGate {
    /// Every member's build has the recorder: the composition attaches it
    /// to each member spawn (`crate::memory::MemorySpawnCustomizer`).
    EveryMember,
    /// A registered agent's build has the recorder when its profile enables
    /// agent memory (`definition_profile_enables_agent_memory`): the host's
    /// explicit answers per profile, else the mob definition's inline
    /// profile. No other member's build has it.
    AgentProfiles(BTreeMap<meerkat_mob::ProfileName, bool>),
}

/// The memory surface a side chat's own recorder is built from: the
/// provider and configuration the composition's recorders use, and their
/// gate.
#[derive(Clone)]
struct SideChatRecorderSource {
    provider: Arc<dyn AgentMemoryProvider>,
    config: AgentMemoryConfig,
    gate: SideChatMemoryGate,
}

/// What a composition has said of the recorder its side chats are bound to.
#[derive(Clone, Default)]
enum RecorderSourceState {
    /// The composition binds no recorder (it has no agent memory, its
    /// recorder is off, or its provider takes no authored writes): no build
    /// is touched, and a build wrapped before this was known serves its
    /// tools as they are.
    #[default]
    Unbound,
    /// The composition binds a recorder and has not yet: a build is
    /// wrapped and offers no `memory` tool until it is bound.
    Awaited,
    /// The recorder is bound.
    Bound(SideChatRecorderSource),
}

/// Late-bound recorder source for side chat builds; empty until the
/// composition's memory stack attaches a recorder.
///
/// A launch that hosts mobs beside its launch mob binds a member from the
/// memory its mob's members are members of: the launch's own for the
/// launch mob's and for the mobs its members created, a hosted mob's own
/// for that mob's ([`Self::register_hosted_mob`]). Which it is is asked at
/// each read of the member's tools, of the launch's record of its mobs
/// ([`crate::private_owners::PrivateOwners::memory_home_of`]) for the mob
/// the member's build is bound to: never of a name, a label or an id a
/// caller supplied. A member of a mob whose members are members of no
/// memory, or whose memory is not decided, is given none.
#[derive(Clone, Default)]
pub(crate) struct SideChatMemorySlot {
    source: Arc<RwLock<RecorderSourceState>>,
    /// The definition of the mob these builds are members of, bound when
    /// the mob is built: what [`SideChatMemoryGate::AgentProfiles`] reads a
    /// profile from, as an agent's own recorder does.
    definition: Arc<RwLock<Option<Arc<meerkat_mob::MobDefinition>>>>,
    /// What tells a registered agent's member from every other member.
    entries: Arc<RwLock<IdentityEntriesReader>>,
    /// What says which memory the members of a mob are members of: the
    /// launch's record of its mobs. A slot without one binds every member
    /// from itself.
    homes: Arc<RwLock<Option<crate::private_owners::PrivateOwners>>>,
    /// What the members of each mob the launch composes beside its launch
    /// mob are bound from, by mob id ([`HostedMobSource`]).
    of_hosted_mobs: Arc<RwLock<BTreeMap<String, HostedMobSource>>>,
}

/// What the members of a mob the launch composes beside its launch mob are
/// bound from: the mob's own memory, its own definition and its own
/// identity entries.
#[derive(Clone)]
struct HostedMobSource {
    /// Which registration of the mob this is. A member's recorder is built
    /// for one registration and serves no other: a mob that is unloaded
    /// and loaded again registers anew, with a new memory value.
    registration: u64,
    /// Whether the mob's own identity runtime still lives: the registration
    /// serves while it does and no longer. A member's recorder that was
    /// built from it is asked through here at every read too, so it serves
    /// nothing once the mob's entry ended.
    lives: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The mob's own provider, held weakly: the mob's composed identity
    /// plane owns it, and this slot keeps no store value of a mob whose
    /// entry has ended.
    provider: std::sync::Weak<dyn AgentMemoryProvider>,
    config: AgentMemoryConfig,
    gate: SideChatMemoryGate,
    /// The mob's definition and its identity entries, in a slot of the
    /// mob's own that is never filled.
    of_the_mob: SideChatMemorySlot,
}

/// Where a member is bound from at one read of its tools.
struct MemberHome {
    /// The registration the answer is of: 0 for the launch's own memory.
    registration: u64,
    /// The slot whose definition and identity entries answer for the
    /// member's mob.
    of_the_mob: SideChatMemorySlot,
    /// What that memory has said of its recorder.
    recorder: RecorderSourceState,
}

static NEXT_HOSTED_MOB_REGISTRATION: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(1);

impl std::fmt::Debug for SideChatMemorySlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let recorder = match self.recorder_state() {
            RecorderSourceState::Unbound => "unbound",
            RecorderSourceState::Awaited => "awaited",
            RecorderSourceState::Bound(_) => "bound",
        };
        f.debug_struct("SideChatMemorySlot")
            .field("recorder", &recorder)
            .field("definition_bound", &self.bound_definition().is_some())
            .finish()
    }
}

/// What the seam reads of a member's build for its late-bound recorder.
struct MemberRecorderFacts {
    /// The member's own memory identity.
    identity: crate::identity_first::AgentIdentity,
    /// The person the member is private to, if it is private.
    owner: Option<String>,
    /// Whether the member is a temporary council's participant.
    council_participant: bool,
    /// The member's roster id.
    member_id: meerkat_mob::ids::AgentIdentity,
    /// The mob the member is built in.
    mob_id: String,
    /// The member's profile.
    profile: String,
    /// The roster id of the member a side chat was forked from; `None` for
    /// a member that is no side chat.
    side_chat_of: Option<meerkat_mob::ids::AgentIdentity>,
}

/// The late-bound recorder of one wrapped member build: asked at every read
/// of the member's tools, it answers from what the slot holds then.
struct MemberRebinding {
    slot: SideChatMemorySlot,
    member: MemberRecorderFacts,
    /// The member's recorder, built at the first read that offers it, with
    /// the registration of the memory it was built from.
    recorder: OnceLock<(u64, MemoryRecorder)>,
}

impl ReboundRecorder for MemberRebinding {
    fn memory(&self, inner_holds_memory: bool) -> ReboundMemory<'_> {
        // A member of a mob that has no memory, or whose memory is not
        // decided, is given none: no `memory` tool of the tools it carries
        // is listed or reached either.
        let Some(home) = self.slot.home_of(&self.member.mob_id) else {
            return ReboundMemory::Withheld;
        };
        let source = match home.recorder {
            RecorderSourceState::Unbound => return ReboundMemory::Untouched,
            RecorderSourceState::Awaited => return ReboundMemory::Withheld,
            RecorderSourceState::Bound(source) => source,
        };
        if !(inner_holds_memory
            || home
                .of_the_mob
                .source_build_has_memory(&source.gate, &self.member))
        {
            return ReboundMemory::Withheld;
        }
        let (built_from, recorder) = self.recorder.get_or_init(|| {
            (
                home.registration,
                home.of_the_mob.member_recorder(source, &self.member),
            )
        });
        // A recorder serves the registration it was built from and no
        // other: it keeps that registration's memory value.
        if *built_from != home.registration {
            return ReboundMemory::Withheld;
        }
        ReboundMemory::Own(recorder)
    }
}

impl SideChatMemorySlot {
    /// From now on which memory a member is bound from is asked of
    /// `owners`, the launch's record of its mobs, for the mob the member's
    /// build is bound to.
    pub(crate) fn decide_by(&self, owners: crate::private_owners::PrivateOwners) {
        *self
            .homes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(owners);
    }

    /// From now on the members of the mob `mob_id`, which the launch
    /// composes beside its launch mob, are bound from that mob's own
    /// memory: its provider and configuration, the gate its agents'
    /// recorders are attached by, its definition and the session bridge of
    /// its identity runtime, which reads its identity entries. A
    /// registration of the mob that stands is replaced. The provider and
    /// the bridge are held weakly: the registration serves while the mob's
    /// identity runtime lives, and this slot keeps none of the mob's values
    /// alive.
    ///
    /// A configuration whose recorder is off, or a provider without
    /// authored writes, gives the mob's side chats no recorder, as it gives
    /// none of its agents one: the mob is then registered with none.
    pub(crate) fn register_hosted_mob(
        &self,
        mob_id: &str,
        provider: &Arc<dyn AgentMemoryProvider>,
        config: AgentMemoryConfig,
        gate: SideChatMemoryGate,
        definition: &meerkat_mob::MobDefinition,
        bridge: &Arc<dyn crate::identity_first::SessionBridge>,
    ) {
        let of_the_mob = SideChatMemorySlot::default();
        of_the_mob.bind_definition(definition);
        of_the_mob.watch_identity_entries(bridge);
        let bridge = Arc::downgrade(bridge);
        self.register_hosted_mob_while(
            mob_id,
            provider,
            config,
            gate,
            of_the_mob,
            Arc::new(move || bridge.strong_count() > 0),
        );
    }

    /// [`Self::register_hosted_mob`] with the slot that holds the mob's
    /// definition and identity entries, and what says whether the mob's
    /// identity runtime still lives.
    fn register_hosted_mob_while(
        &self,
        mob_id: &str,
        provider: &Arc<dyn AgentMemoryProvider>,
        config: AgentMemoryConfig,
        gate: SideChatMemoryGate,
        of_the_mob: SideChatMemorySlot,
        lives: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        if !(config.recorder_tool && provider.supports_authored_writes()) {
            self.forget_hosted_mob(mob_id);
            return;
        }
        let registration =
            NEXT_HOSTED_MOB_REGISTRATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.of_hosted_mobs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                mob_id.to_string(),
                HostedMobSource {
                    registration,
                    lives,
                    provider: Arc::downgrade(provider),
                    config,
                    gate,
                    of_the_mob,
                },
            );
    }

    /// Forget what the members of the mob `mob_id` are bound from: the mob
    /// is destroyed, or a mob is created anew under its id.
    pub(crate) fn forget_hosted_mob(&self, mob_id: &str) {
        self.of_hosted_mobs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(mob_id);
    }

    /// Where a member of the mob `mob_id` is bound from now; `None` for a
    /// member that is given no memory.
    ///
    /// A member of the launch's memory is bound from this slot, and a
    /// member of a mob with a memory of its own from that mob's
    /// registration while the mob's memory value lives. A member of a mob
    /// whose members are members of no memory is given none; so is a
    /// member of a mob whose memory is not decided, because a `memory` tool
    /// is access to a store and what cannot be told gives none. The next
    /// read asks again.
    fn home_of(&self, mob_id: &str) -> Option<MemberHome> {
        use crate::private_owners::MemoryHome;
        let own = || MemberHome {
            registration: 0,
            of_the_mob: self.clone(),
            recorder: self.recorder_state(),
        };
        let homes = self
            .homes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(homes) = homes else {
            return Some(own());
        };
        match homes.memory_home_of(mob_id) {
            MemoryHome::Launch => Some(own()),
            MemoryHome::Hosted(mob) => {
                let hosted = self
                    .of_hosted_mobs
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get(&mob)
                    .cloned()?;
                // The mob's identity runtime has ended: its entry has, and
                // nothing of its memory is served any more.
                if !(hosted.lives)() {
                    return None;
                }
                let provider = hosted.provider.upgrade()?;
                Some(MemberHome {
                    registration: hosted.registration,
                    of_the_mob: hosted.of_the_mob,
                    recorder: RecorderSourceState::Bound(SideChatRecorderSource {
                        provider,
                        config: hosted.config,
                        gate: hosted.gate,
                    }),
                })
            }
            MemoryHome::Nowhere | MemoryHome::NotKnown => None,
        }
    }

    /// From now on this slot's composition binds a recorder: a side chat
    /// built before the fill is wrapped and offers no `memory` tool until
    /// then. What a composition says before its mob is built when it fills
    /// the slot only afterwards. A slot that is filled stays filled.
    pub(crate) fn expect_recorder(&self) {
        let mut source = self
            .source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*source, RecorderSourceState::Unbound) {
            *source = RecorderSourceState::Awaited;
        }
    }

    /// Fill the slot with the provider and configuration the composition's
    /// agent recorders use, gated per agent profile by the mob definition
    /// alone ([`Self::fill_gated`] with no explicit answer).
    pub(crate) fn fill(&self, provider: Arc<dyn AgentMemoryProvider>, config: AgentMemoryConfig) {
        self.fill_gated(
            provider,
            config,
            SideChatMemoryGate::AgentProfiles(BTreeMap::new()),
        );
    }

    /// Fill the slot with the provider and configuration the composition's
    /// member recorders use and the gate they are attached by. A
    /// configuration whose recorder is off, or a provider without authored
    /// writes, gives side chats no recorder, as it gives no member one: the
    /// slot then says that the composition binds none.
    pub(crate) fn fill_gated(
        &self,
        provider: Arc<dyn AgentMemoryProvider>,
        config: AgentMemoryConfig,
        gate: SideChatMemoryGate,
    ) {
        let state = if config.recorder_tool && provider.supports_authored_writes() {
            RecorderSourceState::Bound(SideChatRecorderSource {
                provider,
                config,
                gate,
            })
        } else {
            RecorderSourceState::Unbound
        };
        *self
            .source
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    /// Bind the definition of the mob these builds are members of: the one
    /// an agent's own recorder reads its profile from.
    pub(crate) fn bind_definition(&self, definition: &meerkat_mob::MobDefinition) {
        *self
            .definition
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(definition.clone()));
    }

    fn recorder_state(&self) -> RecorderSourceState {
        self.source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn bound_definition(&self) -> Option<Arc<meerkat_mob::MobDefinition>> {
        self.definition
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether the build of the member a side chat was forked from has the
    /// recorder, by `gate`; `false` for a member that is no side chat.
    ///
    /// Per agent profile the answer is the source's own: the source is a
    /// registered agent's member (asked of Core's identity entries, as
    /// [`IdentityEntriesSlot::names_registered_agent`] does) and its
    /// profile enables agent memory
    /// (`definition_profile_enables_agent_memory`). While that cannot be
    /// told (the entries' reader or the mob's definition is not bound yet)
    /// the answer is `false`, and the next read asks again.
    fn source_build_has_memory(
        &self,
        gate: &SideChatMemoryGate,
        member: &MemberRecorderFacts,
    ) -> bool {
        let Some(source_member) = member.side_chat_of.as_ref() else {
            return false;
        };
        match gate {
            SideChatMemoryGate::EveryMember => true,
            SideChatMemoryGate::AgentProfiles(explicit) => {
                if self
                    .identity_entries()
                    .names_registered_agent(source_member)
                    != Some(true)
                {
                    return false;
                }
                let profile = meerkat_mob::ProfileName::from(member.profile.as_str());
                match self.bound_definition() {
                    Some(definition) => {
                        definition_profile_enables_agent_memory(&definition, &profile, explicit)
                    }
                    None => explicit.get(&profile).copied().unwrap_or(false),
                }
            }
        }
    }

    /// The recorder of `member`, built from `source`: where the member's
    /// records are kept is its own (see the module docs).
    fn member_recorder(
        &self,
        source: SideChatRecorderSource,
        member: &MemberRecorderFacts,
    ) -> MemoryRecorder {
        let recorder = MemoryRecorder::new(
            source.provider,
            source.config,
            member.identity.clone(),
            member.owner.is_none().then(|| member.mob_id.clone()),
        );
        match member.owner.clone() {
            _ if member.council_participant => recorder.kept(MemberMemoryKept::Nowhere(
                COUNCIL_PARTICIPANT_KEEPS_NO_MEMORY,
            )),
            None => recorder,
            Some(owner) => {
                let entries = self.identity_entries();
                let member_id = member.member_id.clone();
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
        }
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

    /// Wrap the tools of a side chat's build, and of the build of a member
    /// private to a person, so that their `memory` tool is the member's own
    /// (see the module docs): bound to the member's own identity, for a
    /// private member inside no mob (its `propose_to_mob`, which would move
    /// its memory into a scope its mob shares, is refused). A private
    /// member that is no registered agent's keeps its records under its
    /// owner and its name, and a temporary council's participant keeps
    /// none. The recorder itself is built late, at the first read of the
    /// member's tools that offers it.
    ///
    /// Nothing changes for a request that is not a member build, a member
    /// that is neither a side chat nor private, a composition that binds no
    /// recorder, or a private member that is no side chat and carries no
    /// tools.
    pub(crate) fn bind_side_chat_memory(&self, req: &mut CreateSessionRequest) {
        let side_chat_of = req.labels.as_ref().and_then(|labels| {
            labels
                .get(crate::side_chat::SIDE_CHAT_OF_LABEL)
                .map(|source| crate::member_comms_id::mob_member_id(source.as_str()))
        });
        let is_side_chat = side_chat_of.is_some();
        let owner = req
            .labels
            .as_ref()
            .and_then(crate::access::private_owner)
            .map(str::to_string);
        if !is_side_chat && owner.is_none() {
            return;
        }
        if matches!(self.recorder_state(), RecorderSourceState::Unbound) {
            return;
        }
        let Some(build) = req.build.as_mut() else {
            return;
        };
        let Some(binding) = build.mob_member_binding.as_ref() else {
            return;
        };
        let overlay = build.external_tools.clone();
        // A private member that is no side chat has a `memory` tool only
        // while the tools it carries hold one: without tools nothing is
        // rebound.
        if !is_side_chat && overlay.is_none() {
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
        let member = MemberRecorderFacts {
            identity,
            owner,
            council_participant,
            member_id: meerkat_mob::ids::AgentIdentity::from(binding.member.as_str()),
            mob_id: binding.mob_id.clone(),
            profile: binding.role.clone(),
            side_chat_of,
        };
        build.external_tools = Some(Arc::new(RecorderToolDispatcher::rebinding(
            overlay,
            Arc::new(MemberRebinding {
                slot: self.clone(),
                member,
                recorder: OnceLock::new(),
            }),
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
    use crate::identity_first::agent_memory::MEMORY_TOOL_NAME;

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

    // -----------------------------------------------------------------
    // A side chat's tools follow its source's: the tools an agent's build
    // publishes reach its side chat whenever they are published, before or
    // after the side chat's own build, and the side chat's `memory` tool is
    // bound to the side chat at every moment.
    // -----------------------------------------------------------------

    /// A filled slot over a store in `dir`, with the provider and the
    /// configuration its recorders use.
    fn filled_slot(
        dir: &std::path::Path,
    ) -> (
        SideChatMemorySlot,
        Arc<dyn AgentMemoryProvider>,
        AgentMemoryConfig,
    ) {
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.fill(Arc::clone(&provider), config.clone());
        (slot, provider, config)
    }

    /// The labels of a side chat of `source`.
    fn side_chat_of(source: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(
            crate::side_chat::SIDE_CHAT_OF_LABEL.to_string(),
            source.to_string(),
        )])
    }

    /// The names of the per-spawn tools `req`'s build hands its member, in
    /// order; none without an overlay.
    fn overlay_tool_names(req: &CreateSessionRequest) -> Vec<String> {
        req.build
            .as_ref()
            .and_then(|build| build.external_tools.as_ref())
            .map(|overlay| {
                overlay
                    .tools()
                    .iter()
                    .map(|tool| tool.name.to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// One `remember` call of the `memory` tool of `req`'s build, titled
    /// `title`: whether the tool answered an error, or the dispatch's refusal.
    async fn remember_note(req: &CreateSessionRequest, title: &str) -> Result<bool, String> {
        let overlay = req
            .build
            .as_ref()
            .and_then(|build| build.external_tools.as_ref())
            .ok_or("the build holds no per-spawn tools")?;
        let args = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "action": "remember",
                "kind": "fact",
                "title": title,
                "description": "a note made in a side chat",
                "body": format!("The note {title} was asked for."),
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        overlay
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-remember",
                name: MEMORY_TOOL_NAME,
                args: &args,
            })
            .await
            .map(|outcome| outcome.result.is_error)
            .map_err(|error| error.to_string())
    }

    /// The titles of the records `identity`'s own scope holds.
    async fn titles_of(
        provider: &Arc<dyn AgentMemoryProvider>,
        config: &AgentMemoryConfig,
        identity: &str,
    ) -> Vec<String> {
        provider
            .manifest(
                &[crate::memory::records::MemoryScope::Identity {
                    realm: config.realm.clone(),
                    identity: identity.to_string(),
                }],
                crate::memory::records::ManifestTier::Full,
            )
            .await
            .unwrap()
            .into_iter()
            .map(|record| record.title)
            .collect()
    }

    /// A host tool an agent's build publishes beside its recorder.
    struct LookupTool;

    #[async_trait::async_trait]
    impl meerkat_core::AgentToolDispatcher for LookupTool {
        fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
            vec![Arc::new(meerkat_core::ToolDef {
                audience: Default::default(),
                name: "lookup".into(),
                description: "lookup".to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                provenance: None,
            })]
            .into()
        }

        async fn dispatch(
            &self,
            call: meerkat_core::types::ToolCallView<'_>,
        ) -> Result<meerkat_core::ops::ToolDispatchOutcome, meerkat_core::error::ToolError>
        {
            Ok(meerkat_core::ToolResult {
                host_metadata: Default::default(),
                settlement_failures: Default::default(),
                tool_use_id: call.id.to_string(),
                content: vec![],
                is_error: false,
            }
            .into())
        }
    }

    /// The per-identity tools of the agent `source`, with nothing published:
    /// its registry entry before any build of the agent in this process.
    fn unpublished_source(
        source: &str,
    ) -> (
        Arc<crate::identity_first::CustomizerToolRegistry>,
        crate::identity_first::AgentIdentity,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) {
        let registry = crate::identity_first::CustomizerToolRegistry::new();
        let identity = crate::identity_first::AgentIdentity::parse(source).unwrap();
        let overlay: Arc<dyn meerkat_core::AgentToolDispatcher> = registry.ensure(&identity);
        (registry, identity, overlay)
    }

    /// The agent's own recorder, as its build composes it over `inner`.
    fn source_recorder(
        inner: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
        source: &crate::identity_first::AgentIdentity,
        provider: &Arc<dyn AgentMemoryProvider>,
        config: &AgentMemoryConfig,
    ) -> Arc<dyn meerkat_core::AgentToolDispatcher> {
        Arc::new(RecorderToolDispatcher::new(
            inner,
            MemoryRecorder::new(
                Arc::clone(provider),
                config.clone(),
                source.clone(),
                Some("team".to_string()),
            ),
        ))
    }

    /// A side chat built on its source's published tools: a tool the source
    /// publishes later reaches the side chat, and the side chat still holds
    /// one `memory` tool, whose note is kept in the side chat's own scope.
    #[tokio::test]
    async fn a_side_chats_tools_follow_what_its_source_publishes_later() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, provider, config) = filled_slot(dir.path());
        let (registry, source, overlay) = unpublished_source("alpha");
        registry.publish(
            &source,
            Some(source_recorder(None, &source, &provider, &config)),
        );
        let mut req = member_build("alpha-side", side_chat_of("alpha"), overlay);
        slot.bind_side_chat_memory(&mut req);
        assert_eq!(
            overlay_tool_names(&req),
            vec![MEMORY_TOOL_NAME.to_string()],
            "the side chat is built with a `memory` tool"
        );
        registry.publish(
            &source,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &source,
                &provider,
                &config,
            )),
        );
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()],
            "the tool its source published later reaches the side chat, beside one `memory` tool"
        );
        assert_eq!(remember_note(&req, "Side note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Side note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
    }

    /// A side chat built before its source published anything: once the
    /// source's tools are published, they reach the side chat, and the note
    /// the side chat's `memory` call stores is kept in the side chat's own
    /// scope.
    #[tokio::test]
    async fn a_side_chat_built_before_its_source_published_keeps_its_notes_in_its_own_scope() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, provider, config) = filled_slot(dir.path());
        let (registry, source, overlay) = unpublished_source("alpha");
        let mut req = member_build("alpha-side", side_chat_of("alpha"), overlay);
        slot.bind_side_chat_memory(&mut req);
        registry.publish(
            &source,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &source,
                &provider,
                &config,
            )),
        );
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()],
            "the tools its source published reach the side chat, with one `memory` tool"
        );
        assert_eq!(remember_note(&req, "Side note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Side note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
    }

    /// Control: a side chat over tools that never hold a `memory` tool gains
    /// none, and the tools its source publishes reach it.
    #[tokio::test]
    async fn a_side_chat_over_tools_without_memory_gains_none() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, _provider, _config) = filled_slot(dir.path());
        let (registry, source, overlay) = unpublished_source("alpha");
        let mut req = member_build("alpha-side", side_chat_of("alpha"), overlay);
        slot.bind_side_chat_memory(&mut req);
        assert!(
            overlay_tool_names(&req).is_empty(),
            "no tool before the source published one"
        );
        registry.publish(&source, Some(Arc::new(LookupTool)));
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string()],
            "the source's tool reaches the side chat, and no `memory` tool"
        );
    }

    /// Control: a member private to a person that is no side chat, built
    /// without per-spawn tools, gains none.
    #[tokio::test]
    async fn a_private_member_without_per_spawn_tools_gains_none() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, _provider, _config) = filled_slot(dir.path());
        let (_registry, _source, overlay) = unpublished_source("alpha");
        let labels = BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "console-caller:v1:aa".to_string(),
        )]);
        let mut req = member_build("alpha-side-helper", labels, overlay);
        req.build.as_mut().unwrap().external_tools = None;
        slot.bind_side_chat_memory(&mut req);
        assert!(
            overlay_tool_names(&req).is_empty(),
            "a private member's tools are what its own build carries"
        );
    }

    // -----------------------------------------------------------------
    // The gate: a side chat is offered `memory` from its first build when
    // its source's build has the recorder, by the source's own rule.
    // -----------------------------------------------------------------

    /// A definition whose `worker` profile enables agent memory inline,
    /// whose `plain` profile does not, and whose `referenced` profile the
    /// definition only references.
    fn gate_definition() -> meerkat_mob::MobDefinition {
        let mut definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "team"

[profiles.worker]
model = "gpt-5.5"
[profiles.worker.tools]
memory = true

[profiles.plain]
model = "gpt-5.5"
"#,
        )
        .unwrap();
        definition.profiles.insert(
            meerkat_mob::ProfileName::from("referenced"),
            meerkat_mob::ProfileBinding::RealmRef {
                realm_profile: "stored-profile".to_string(),
            },
        );
        definition
    }

    /// A slot filled for agent recorders gated per profile, with `explicit`
    /// as the host's answers, and bound to [`gate_definition`].
    fn gated_slot(
        dir: &std::path::Path,
        explicit: &[(&str, bool)],
    ) -> (
        SideChatMemorySlot,
        Arc<dyn AgentMemoryProvider>,
        AgentMemoryConfig,
    ) {
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.fill_gated(
            Arc::clone(&provider),
            config.clone(),
            SideChatMemoryGate::AgentProfiles(
                explicit
                    .iter()
                    .map(|(profile, enabled)| (meerkat_mob::ProfileName::from(*profile), *enabled))
                    .collect(),
            ),
        );
        slot.bind_definition(&gate_definition());
        // `alpha`, the source of the rows' side chats, is a registered
        // agent's member.
        slot.hold_identity_entries(std::collections::BTreeSet::from([
            crate::member_comms_id::mob_member_id("alpha"),
        ]));
        (slot, provider, config)
    }

    /// A side chat of `alpha` on the profile `role`, built over `overlay`.
    fn side_chat_build(
        role: &str,
        overlay: Option<Arc<dyn meerkat_core::AgentToolDispatcher>>,
    ) -> CreateSessionRequest {
        let (_registry, _source, placeholder) = unpublished_source("alpha");
        let mut req = member_build("alpha-side", side_chat_of("alpha"), placeholder);
        let build = req.build.as_mut().unwrap();
        build.external_tools = overlay;
        build.mob_member_binding.as_mut().unwrap().role = role.to_string();
        req
    }

    /// A side chat of an agent whose profile enables agent memory, built
    /// while its source's tools hold no publication, is offered its own
    /// `memory` tool at that build, and its note is kept in its own scope.
    #[tokio::test]
    async fn a_side_chat_of_an_agent_with_memory_is_offered_memory_before_its_source_published() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, provider, config) = gated_slot(dir.path(), &[]);
        let (_registry, _source, unpublished) = unpublished_source("alpha");
        let mut req = side_chat_build("worker", Some(unpublished));
        slot.bind_side_chat_memory(&mut req);
        assert_eq!(
            overlay_tool_names(&req),
            vec![MEMORY_TOOL_NAME.to_string()],
            "the side chat is offered `memory` before its source published anything"
        );
        assert_eq!(remember_note(&req, "Early note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Early note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
    }

    /// A side chat of an agent whose build carries none of its source's
    /// tools (a side chat forked from a source this process did not build)
    /// is offered its own `memory` tool by the gate.
    #[tokio::test]
    async fn a_side_chat_built_without_its_sources_tools_is_offered_memory_by_the_gate() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, provider, config) = gated_slot(dir.path(), &[]);
        let mut req = side_chat_build("worker", None);
        slot.bind_side_chat_memory(&mut req);
        assert_eq!(
            overlay_tool_names(&req),
            vec![MEMORY_TOOL_NAME.to_string()],
            "the side chat is offered `memory` with none of its source's tools"
        );
        assert_eq!(remember_note(&req, "Early note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Early note".to_string()]
        );
    }

    /// A side chat whose tools are first read before the reader of Core's
    /// identity entries is bound is offered no `memory` at that read. The
    /// answer is asked again at every read: once the reader is bound and
    /// names the side chat's source a registered agent's member, the same
    /// build's tools offer the side chat's own `memory`, and its note is
    /// kept in its own scope.
    #[tokio::test]
    async fn a_side_chat_read_before_the_identity_reader_is_bound_is_offered_memory_once_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.fill_gated(
            Arc::clone(&provider),
            config.clone(),
            SideChatMemoryGate::AgentProfiles(std::collections::BTreeMap::new()),
        );
        slot.bind_definition(&gate_definition());
        slot.await_identity_entries();
        let mut req = side_chat_build("worker", None);
        slot.bind_side_chat_memory(&mut req);
        assert!(
            overlay_tool_names(&req).is_empty(),
            "no `memory` while it cannot be told whether the side chat's source is an agent"
        );

        slot.hold_identity_entries(std::collections::BTreeSet::from([
            crate::member_comms_id::mob_member_id("alpha"),
        ]));
        assert_eq!(
            overlay_tool_names(&req),
            vec![MEMORY_TOOL_NAME.to_string()],
            "the same build's tools offer the side chat's own `memory` once the reader is bound"
        );
        assert_eq!(remember_note(&req, "Later note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Later note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
    }

    /// A side chat of an agent whose own build has no `memory` tool is
    /// offered none: a profile without agent memory, a profile the
    /// definition only references (whatever that profile declares), and a
    /// profile the host switched agent memory off for.
    #[tokio::test]
    async fn a_side_chat_of_an_agent_without_memory_is_offered_none() {
        let dir = tempfile::tempdir().unwrap();
        for (role, explicit) in [
            ("plain", &[][..]),
            ("referenced", &[][..]),
            ("worker", &[("worker", false)][..]),
        ] {
            let (slot, _provider, _config) = gated_slot(dir.path(), explicit);
            let (_registry, _source, unpublished) = unpublished_source("alpha");
            let mut req = side_chat_build(role, Some(unpublished));
            req.build.as_mut().unwrap().override_memory =
                meerkat_core::ToolCategoryOverride::Enable;
            slot.bind_side_chat_memory(&mut req);
            assert!(
                overlay_tool_names(&req).is_empty(),
                "a side chat on `{role}` is offered no `memory` tool over its source's tools"
            );
            let mut req = side_chat_build(role, None);
            slot.bind_side_chat_memory(&mut req);
            assert!(
                overlay_tool_names(&req).is_empty(),
                "a side chat on `{role}` is offered no `memory` tool without its source's tools"
            );
        }
    }

    /// The host's explicit answer for a profile decides before the
    /// definition does, as it does for the agent's own recorder.
    #[tokio::test]
    async fn the_hosts_answer_for_a_profile_decides_a_side_chats_memory() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, _provider, _config) = gated_slot(dir.path(), &[("referenced", true)]);
        let mut req = side_chat_build("referenced", None);
        slot.bind_side_chat_memory(&mut req);
        assert_eq!(
            overlay_tool_names(&req),
            vec![MEMORY_TOOL_NAME.to_string()],
            "the host switched agent memory on for the profile"
        );
    }

    /// A slot whose mob's definition is not bound yet offers no `memory`
    /// tool by a profile: it knows no profile.
    #[tokio::test]
    async fn a_slot_without_its_mobs_definition_offers_no_memory_by_profile() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, _provider, _config) = filled_slot(dir.path());
        let mut req = side_chat_build("worker", None);
        slot.bind_side_chat_memory(&mut req);
        assert!(overlay_tool_names(&req).is_empty());
    }

    /// In a composition that attaches the recorder to every member, a side
    /// chat is offered its own `memory` tool whatever its profile.
    #[tokio::test]
    async fn a_side_chat_is_offered_memory_where_every_member_has_the_recorder() {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let slot = SideChatMemorySlot::default();
        slot.fill_gated(
            provider,
            AgentMemoryConfig::default(),
            SideChatMemoryGate::EveryMember,
        );
        let mut req = side_chat_build("plain", None);
        slot.bind_side_chat_memory(&mut req);
        assert_eq!(overlay_tool_names(&req), vec![MEMORY_TOOL_NAME.to_string()]);
    }

    /// A side chat of a member that is no registered agent's is offered
    /// `memory` only while the tools it carries hold one, as its source is:
    /// on a profile that enables agent memory it is offered none without
    /// tools, none over tools without a `memory` tool, and its own over
    /// tools that hold one.
    #[tokio::test]
    async fn a_side_chat_of_a_member_that_is_no_agent_is_offered_memory_only_with_its_tools() {
        let dir = tempfile::tempdir().unwrap();
        let (slot, provider, config) = gated_slot(dir.path(), &[]);
        let side_chat_of_a_helper = |overlay| {
            let mut req = side_chat_build("worker", overlay);
            req.labels = Some(side_chat_of("helper"));
            req
        };
        let mut req = side_chat_of_a_helper(None);
        slot.bind_side_chat_memory(&mut req);
        assert!(
            overlay_tool_names(&req).is_empty(),
            "no `memory` tool without its source's tools"
        );
        let (registry, helper, tools) = unpublished_source("helper");
        let mut req = side_chat_of_a_helper(Some(tools));
        slot.bind_side_chat_memory(&mut req);
        registry.publish(&helper, Some(Arc::new(LookupTool)));
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string()],
            "no `memory` tool over tools that hold none"
        );
        registry.publish(
            &helper,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &helper,
                &provider,
                &config,
            )),
        );
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()],
            "one `memory` tool over tools that hold one"
        );
        assert_eq!(remember_note(&req, "Side note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Side note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
    }

    /// A side chat built before its composition has bound its recorder is
    /// wrapped all the same: it is offered no `memory` tool, also once the
    /// tools it carries hold one, and a call of `memory` is refused; from
    /// the read after the recorder is bound it is offered its own.
    #[tokio::test]
    async fn a_side_chat_built_before_the_recorder_is_bound_is_offered_its_own_once_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.expect_recorder();
        let (registry, source, tools) = unpublished_source("alpha");
        let mut req = side_chat_build("worker", Some(tools));
        slot.bind_side_chat_memory(&mut req);
        assert!(
            overlay_tool_names(&req).is_empty(),
            "no tool before the source published one"
        );
        registry.publish(
            &source,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &source,
                &provider,
                &config,
            )),
        );
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string()],
            "the source's tool reaches the side chat, and no `memory` tool while no recorder \
             is bound"
        );
        assert!(
            remember_note(&req, "Too early").await.is_err(),
            "a call of `memory` is refused while no recorder is bound"
        );
        slot.fill(Arc::clone(&provider), config.clone());
        assert_eq!(
            overlay_tool_names(&req),
            vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()],
            "one `memory` tool once the recorder is bound"
        );
        assert_eq!(remember_note(&req, "Side note").await, Ok(false));
        assert_eq!(
            titles_of(&provider, &config, "alpha-side").await,
            vec!["Side note".to_string()],
            "the side chat's note is kept in the side chat's own scope"
        );
        assert!(
            titles_of(&provider, &config, "alpha").await.is_empty(),
            "the refused call stored nothing"
        );
    }

    /// A composition that binds no recorder leaves a side chat's tools as
    /// they are, a tool named `memory` that a host gave its source
    /// included: when it never says it binds one, and when a side chat was
    /// wrapped before its recorder turned out to be off.
    #[tokio::test]
    async fn a_composition_without_a_recorder_leaves_a_side_chats_tools_as_they_are() {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let config = AgentMemoryConfig::default();
        let recorder_off = AgentMemoryConfig {
            recorder_tool: false,
            ..AgentMemoryConfig::default()
        };
        for announced in [false, true] {
            let slot = SideChatMemorySlot::default();
            if announced {
                slot.expect_recorder();
            }
            let (registry, source, tools) = unpublished_source("alpha");
            let mut req = side_chat_build("worker", Some(tools));
            slot.bind_side_chat_memory(&mut req);
            slot.fill(Arc::clone(&provider), recorder_off.clone());
            registry.publish(
                &source,
                Some(source_recorder(
                    Some(Arc::new(LookupTool)),
                    &source,
                    &provider,
                    &config,
                )),
            );
            assert_eq!(
                overlay_tool_names(&req),
                vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()],
                "the side chat's tools are its source's as they are (announced: {announced})"
            );
        }
    }

    /// One call of the `memory` tool of `req`'s build with `args`: whether
    /// the tool answered an error, or the dispatch's refusal.
    async fn memory_verb(
        req: &CreateSessionRequest,
        args: serde_json::Value,
    ) -> Result<bool, String> {
        let overlay = req
            .build
            .as_ref()
            .and_then(|build| build.external_tools.as_ref())
            .ok_or("the build holds no per-spawn tools")?;
        let args = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
        overlay
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-verb",
                name: MEMORY_TOOL_NAME,
                args: &args,
            })
            .await
            .map(|outcome| outcome.result.is_error)
            .map_err(|error| error.to_string())
    }

    /// A side chat of the agent `alpha` built before its composition has
    /// bound its recorder, over `alpha`'s tools, which hold `alpha`'s own
    /// recorder and one note of `alpha`'s: the verb `verb` of the side
    /// chat's `memory` tool is refused, the agent's notes are as they were,
    /// and the side chat holds none.
    async fn a_verb_before_the_recorder_is_bound_is_refused(verb: &str) {
        let dir = tempfile::tempdir().unwrap();
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir.path()).unwrap());
        let config = AgentMemoryConfig::default();
        let slot = SideChatMemorySlot::default();
        slot.expect_recorder();
        let (registry, source, tools) = unpublished_source("alpha");
        let mut req = side_chat_build("worker", Some(tools));
        slot.bind_side_chat_memory(&mut req);
        registry.publish(
            &source,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &source,
                &provider,
                &config,
            )),
        );
        let agents_note = provider
            .remember_authored(
                &crate::memory::records::MemoryScope::Identity {
                    realm: config.realm.clone(),
                    identity: "alpha".to_string(),
                },
                crate::memory::records::NewMemoryRecord {
                    kind: crate::memory::records::MemoryKind::Fact,
                    title: "Agent note".to_string(),
                    description: "a note of the agent's".to_string(),
                    body: "The agent's own note.".to_string(),
                    tags: vec![],
                    evidence: vec![],
                    verification: None,
                },
                crate::memory::records::MemoryAuthor::Agent {
                    identity: "alpha".to_string(),
                },
            )
            .await
            .unwrap()
            .memory_id;
        let args = match verb {
            "update" => serde_json::json!({
                "action": verb,
                "memory_id": agents_note,
                "kind": "fact",
                "title": "Rewritten",
                "description": "a rewritten note",
                "body": "The side chat asked to rewrite a note.",
                "tags": [],
            }),
            "forget" => serde_json::json!({ "action": verb, "memory_id": agents_note }),
            "recall" => serde_json::json!({ "action": verb }),
            _ => serde_json::json!({
                "action": verb,
                "kind": "fact",
                "title": "Side note",
                "description": "a note made in a side chat",
                "body": "The side chat asked to store a note.",
                "tags": [],
            }),
        };
        assert!(
            memory_verb(&req, args).await.is_err(),
            "`{verb}` is refused while no recorder is bound"
        );
        assert_eq!(
            titles_of(&provider, &config, "alpha").await,
            vec!["Agent note".to_string()],
            "the source agent's notes are as they were after `{verb}`"
        );
        assert!(
            titles_of(&provider, &config, "alpha-side").await.is_empty(),
            "the side chat holds no note after `{verb}`"
        );
    }

    #[tokio::test]
    async fn a_remember_before_the_recorder_is_bound_is_refused() {
        a_verb_before_the_recorder_is_bound_is_refused("remember").await;
    }

    #[tokio::test]
    async fn an_update_before_the_recorder_is_bound_is_refused() {
        a_verb_before_the_recorder_is_bound_is_refused("update").await;
    }

    #[tokio::test]
    async fn a_forget_before_the_recorder_is_bound_is_refused() {
        a_verb_before_the_recorder_is_bound_is_refused("forget").await;
    }

    #[tokio::test]
    async fn a_recall_before_the_recorder_is_bound_is_refused() {
        a_verb_before_the_recorder_is_bound_is_refused("recall").await;
    }

    #[tokio::test]
    async fn a_proposal_before_the_recorder_is_bound_is_refused() {
        a_verb_before_the_recorder_is_bound_is_refused("propose_to_mob").await;
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

    /// The record of a launch whose mob is `team` and which composes
    /// `second` beside it, with a mob a member of `second` created and a
    /// mob no member created.
    fn a_launch_of_two_mobs() -> crate::private_owners::PrivateOwners {
        let owners = crate::private_owners::PrivateOwners::default();
        owners.bind_launch_mob("team");
        owners.hosted_mobs().name("second");
        owners.hold_the_creation_in_tests("of-second", Some("second"));
        owners.hold_the_creation_in_tests("unowned", None);
        owners
    }

    /// A memory of a mob's own, in the realm `mob.second`, over `dir`.
    fn memory_of_the_second_mob(
        dir: &std::path::Path,
    ) -> (Arc<dyn AgentMemoryProvider>, AgentMemoryConfig) {
        let provider: Arc<dyn AgentMemoryProvider> =
            Arc::new(crate::memory::SqliteAgentMemoryStore::open(dir).unwrap());
        let config = AgentMemoryConfig {
            realm: "mob.second".to_string(),
            ..AgentMemoryConfig::default()
        };
        (provider, config)
    }

    /// Register `provider` as the memory of the mob `second` with `slot`:
    /// the registration serves while the returned value lives.
    fn register_the_second_mob(
        slot: &SideChatMemorySlot,
        provider: &Arc<dyn AgentMemoryProvider>,
        config: &AgentMemoryConfig,
    ) -> Arc<()> {
        let lives = Arc::new(());
        let seen = Arc::downgrade(&lives);
        slot.register_hosted_mob_while(
            "second",
            provider,
            config.clone(),
            SideChatMemoryGate::AgentProfiles(BTreeMap::new()),
            SideChatMemorySlot::default(),
            Arc::new(move || seen.strong_count() > 0),
        );
        lives
    }

    /// The build of the member `alpha-side` of the mob `mob_id`, a side
    /// chat of `alpha`, over tools that hold a host's `lookup` and a
    /// `memory` tool of another build, after `slot`. The registry is the
    /// tools' own and is kept with the build.
    fn side_chat_in(
        slot: &SideChatMemorySlot,
        mob_id: &str,
        provider: &Arc<dyn AgentMemoryProvider>,
        config: &AgentMemoryConfig,
    ) -> (
        CreateSessionRequest,
        Arc<crate::identity_first::CustomizerToolRegistry>,
    ) {
        let (registry, source, overlay) = unpublished_source("alpha");
        registry.publish(
            &source,
            Some(source_recorder(
                Some(Arc::new(LookupTool)),
                &source,
                provider,
                config,
            )),
        );
        let mut req = member_build("alpha-side", side_chat_of("alpha"), overlay);
        if let Some(binding) = req
            .build
            .as_mut()
            .and_then(|build| build.mob_member_binding.as_mut())
        {
            binding.mob_id = mob_id.to_string();
        }
        slot.bind_side_chat_memory(&mut req);
        (req, registry)
    }

    fn offered() -> Vec<String> {
        vec!["lookup".to_string(), MEMORY_TOOL_NAME.to_string()]
    }

    fn withheld() -> Vec<String> {
        vec!["lookup".to_string()]
    }

    /// A member is bound from the memory its mob's members are members
    /// of: a side chat of a mob with a memory of its own keeps its notes
    /// in that memory, one of the launch mob in the launch's, and one of a
    /// mob with no memory, or whose memory is not decided, is offered no
    /// `memory` tool and writes nothing, though the tools it carries hold
    /// one. A mob whose memory comes to be decided is served at the next
    /// read.
    #[tokio::test]
    async fn a_member_is_bound_from_the_memory_its_mobs_members_are_members_of() {
        let (launch_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (slot, of_the_launch, config) = filled_slot(launch_dir.path());
        let owners = a_launch_of_two_mobs();
        slot.decide_by(owners.clone());
        let (of_the_second, config_of_the_second) = memory_of_the_second_mob(second_dir.path());
        let _lives = register_the_second_mob(&slot, &of_the_second, &config_of_the_second);

        let (in_the_second, _tools) = side_chat_in(&slot, "second", &of_the_launch, &config);
        assert_eq!(overlay_tool_names(&in_the_second), offered());
        assert_eq!(
            remember_note(&in_the_second, "Second note").await,
            Ok(false)
        );
        let (in_the_launch, _tools) = side_chat_in(&slot, "team", &of_the_launch, &config);
        assert_eq!(overlay_tool_names(&in_the_launch), offered());
        assert_eq!(
            remember_note(&in_the_launch, "Launch note").await,
            Ok(false)
        );
        for mob in ["of-second", "unowned", "never-read"] {
            let (in_no_memory, _tools) = side_chat_in(&slot, mob, &of_the_launch, &config);
            assert_eq!(
                overlay_tool_names(&in_no_memory),
                withheld(),
                "a side chat of the mob {mob} is offered no `memory` tool"
            );
            assert!(
                remember_note(&in_no_memory, "No note").await.is_err(),
                "and a call of `memory` there is served by no tool ({mob})"
            );
        }

        assert_eq!(
            titles_of(&of_the_second, &config_of_the_second, "alpha-side").await,
            vec!["Second note".to_string()],
            "the second mob's memory holds its side chat's note and no other"
        );
        assert_eq!(
            titles_of(&of_the_launch, &config, "alpha-side").await,
            vec!["Launch note".to_string()],
            "the launch's memory holds the launch mob's side chat's note and no other"
        );

        // A build made while its mob's memory was not decided is served
        // once the record holds the mob's creation.
        let (late, _tools) = side_chat_in(&slot, "late", &of_the_launch, &config);
        assert_eq!(overlay_tool_names(&late), withheld());
        owners.hold_the_creation_in_tests("late", Some("team"));
        assert_eq!(overlay_tool_names(&late), offered());
    }

    /// A hosted mob's registration serves while the mob's identity runtime
    /// lives, and a member's recorder serves the registration it was built
    /// from and no other: after the mob's entry ended its members' builds
    /// are offered no `memory` tool, a build made before stays so when the
    /// mob registers anew, and a build made after is served by the new
    /// registration. A forgotten mob serves none.
    #[tokio::test]
    async fn a_hosted_mobs_registration_serves_while_its_mob_lives() {
        let (launch_dir, second_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (slot, of_the_launch, config) = filled_slot(launch_dir.path());
        slot.decide_by(a_launch_of_two_mobs());
        let (of_the_second, config_of_the_second) = memory_of_the_second_mob(second_dir.path());

        let lives = register_the_second_mob(&slot, &of_the_second, &config_of_the_second);
        let (built_before, _tools) = side_chat_in(&slot, "second", &of_the_launch, &config);
        assert_eq!(remember_note(&built_before, "Before").await, Ok(false));

        // The mob's entry ends.
        drop(lives);
        assert_eq!(overlay_tool_names(&built_before), withheld());
        assert!(remember_note(&built_before, "Late").await.is_err());

        // The mob is loaded again: a new memory value over the same store.
        let (loaded_again, _) = memory_of_the_second_mob(second_dir.path());
        let _lives = register_the_second_mob(&slot, &loaded_again, &config_of_the_second);
        assert_eq!(
            overlay_tool_names(&built_before),
            withheld(),
            "a build of the earlier registration is not served by the new one"
        );
        let (built_after, _tools) = side_chat_in(&slot, "second", &of_the_launch, &config);
        assert_eq!(overlay_tool_names(&built_after), offered());
        assert_eq!(remember_note(&built_after, "After").await, Ok(false));
        let mut titles = titles_of(&loaded_again, &config_of_the_second, "alpha-side").await;
        titles.sort();
        assert_eq!(titles, vec!["After".to_string(), "Before".to_string()]);
        assert_eq!(
            titles_of(&of_the_launch, &config, "alpha-side").await,
            Vec::<String>::new(),
            "the launch's memory holds none of it"
        );

        slot.forget_hosted_mob("second");
        assert_eq!(overlay_tool_names(&built_after), withheld());
    }
}
