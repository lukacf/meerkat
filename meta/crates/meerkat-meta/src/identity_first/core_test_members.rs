//! Test fixture: the members Core holds, for test doubles of
//! [`SessionBridge`](super::bridge::SessionBridge) whose Core reads
//! (`live_member`, `live_member_scope`, `last_member_binding`,
//! `member_binding_history`, `durable_identity_members`, `member_standing`)
//! answer from what the double's create, resume, reset and retire calls
//! committed. Once [`CoreTestMembers::keep_journal`] was called, the member
//! lifecycle journal reads answer too, with the facts Core journals: each
//! bind journals a seat, each retire a retirement, each rotation
//! ([`CoreTestMembers::rotate`]) the retirement and the seat that name each
//! other, and each started retirement ([`CoreTestMembers::start_retirement`])
//! its start. [`CoreTestMembers::mark_host_made`] makes an identity's member
//! one a host made directly on the mob under the identity's member id, and
//! [`CoreTestMembers::read_retired_in_history`] has Core's member history
//! disagree with Core's standing of a member.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use meerkat_core::types::SessionId;

use super::bridge::{
    BridgeError, MemberLifecycleFact, MemberLifecycleFactRecord, MemberLifecycleJournalHead,
    MemberLifecycleJournalPage, MemberRuntimeBinding, MemberStanding,
};
use super::types::{AgentIdentity, AgentRuntimeId};

/// The mob incarnation the journal of [`CoreTestMembers`] names.
pub(crate) const JOURNAL_INCARNATION: &str = "core-test-members";

#[derive(Clone)]
struct TestMember {
    runtime_id: AgentRuntimeId,
    session_id: Option<SessionId>,
    generation: u64,
    fence: u64,
    profile: meerkat_mob::ProfileName,
    live: bool,
    /// Core started the incarnation's retirement and has not ended it.
    retiring: bool,
}

#[derive(Default)]
pub(crate) struct CoreTestMembers {
    members: Mutex<BTreeMap<AgentIdentity, TestMember>>,
    /// Bindings a rebind ended: Core's history still names the sessions an
    /// identity's member was bound to before (who owned a session).
    ended: Mutex<Vec<(AgentIdentity, TestMember)>>,
    /// The member lifecycle facts the binds and retires journaled, in order;
    /// a fact's cursor is its position from one.
    facts: Mutex<Vec<MemberLifecycleFact>>,
    /// Set by [`Self::keep_journal`]: the journal reads answer.
    journal: AtomicBool,
    /// The identities whose member a host made directly on the mob
    /// ([`Self::mark_host_made`]).
    host_made: Mutex<BTreeSet<AgentIdentity>>,
    /// The identities whose member Core's member history reads as retired
    /// ([`Self::read_retired_in_history`]).
    retired_in_history: Mutex<BTreeSet<AgentIdentity>>,
}

impl CoreTestMembers {
    /// Core committed `binding` as `identity`'s live member. The
    /// generation is the runtime id's `rt:{identity}:{generation}` suffix.
    pub(crate) fn bind(
        &self,
        identity: &AgentIdentity,
        binding: &MemberRuntimeBinding,
        profile: &meerkat_mob::ProfileName,
    ) {
        let generation = generation_of(&binding.agent_runtime_id);
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(previous) = members.get(identity)
            && previous.session_id.is_some()
            && previous.session_id != binding.session_id
        {
            self.ended
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    identity.clone(),
                    TestMember {
                        live: false,
                        ..previous.clone()
                    },
                ));
        }
        let fence = members.get(identity).map_or(1, |member| member.fence + 1);
        self.journal_fact(
            MemberLifecycleFact::seated(member_id(identity), generation, profile.clone())
                .with_session(binding.session_id.clone()),
        );
        members.insert(
            identity.clone(),
            TestMember {
                runtime_id: binding.agent_runtime_id.clone(),
                session_id: binding.session_id.clone(),
                generation,
                fence,
                profile: profile.clone(),
                live: true,
                retiring: false,
            },
        );
    }

    /// Core's rotation of `identity`'s member onto `binding` under `profile`,
    /// as `rotate_member` and a respawn are: one step retires the live
    /// incarnation and seats the next one, and its two journal rows name each
    /// other (onto a fresh session, unless `binding` keeps the live
    /// incarnation's session). With no live member, the seat is a fresh life
    /// after the identity's last incarnation, when it had one. The
    /// generation is the runtime id's `rt:{identity}:{generation}` suffix.
    pub(crate) fn rotate(
        &self,
        identity: &AgentIdentity,
        binding: &MemberRuntimeBinding,
        profile: &meerkat_mob::ProfileName,
    ) {
        let generation = generation_of(&binding.agent_runtime_id);
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = members.get(identity).cloned();
        let seat_rotation = match previous.as_ref() {
            Some(live) if live.live => {
                let target = if live.session_id == binding.session_id {
                    meerkat_mob::RotationTarget::SameSession
                } else {
                    meerkat_mob::RotationTarget::FreshSession
                };
                self.journal_fact(
                    MemberLifecycleFact::retired(
                        member_id(identity),
                        live.generation,
                        live.profile.clone(),
                    )
                    .with_session(live.session_id.clone())
                    .with_retired_rotation(Some(
                        meerkat_mob::RetiredRotation::new(
                            target,
                            meerkat_mob::Generation::new(generation),
                            profile.clone(),
                        ),
                    )),
                );
                Some(meerkat_mob::SeatRotation::successor(
                    meerkat_mob::Generation::new(live.generation),
                    target,
                ))
            }
            Some(retired) => Some(meerkat_mob::SeatRotation::fresh_life_after(
                meerkat_mob::Generation::new(retired.generation),
            )),
            None => None,
        };
        if let Some(previous) = previous
            && previous.session_id.is_some()
            && previous.session_id != binding.session_id
        {
            self.ended
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((
                    identity.clone(),
                    TestMember {
                        live: false,
                        retiring: false,
                        ..previous
                    },
                ));
        }
        let fence = members.get(identity).map_or(1, |member| member.fence + 1);
        self.journal_fact(
            MemberLifecycleFact::seated(member_id(identity), generation, profile.clone())
                .with_session(binding.session_id.clone())
                .with_seat_rotation(seat_rotation),
        );
        members.insert(
            identity.clone(),
            TestMember {
                runtime_id: binding.agent_runtime_id.clone(),
                session_id: binding.session_id.clone(),
                generation,
                fence,
                profile: profile.clone(),
                live: true,
                retiring: false,
            },
        );
    }

    /// Core started the retirement of the live incarnation `runtime_id`,
    /// and has not ended it: its start is journaled, carrying out a rotation
    /// to `rotation` when it names one, and the member reads retiring.
    pub(crate) fn start_retirement(
        &self,
        runtime_id: &AgentRuntimeId,
        rotation: Option<meerkat_mob::RotationTarget>,
    ) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for (identity, member) in members.iter_mut() {
            if member.runtime_id == *runtime_id && member.live && !member.retiring {
                member.retiring = true;
                self.journal_fact(MemberLifecycleFact::retirement_started(
                    member_id(identity),
                    member.generation,
                    rotation,
                    None,
                ));
            }
        }
    }

    /// Core retired the member whose runtime id is `runtime_id`; its
    /// binding stays in the history as retired.
    pub(crate) fn retire(&self, runtime_id: &AgentRuntimeId) {
        let mut members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        for (identity, member) in members.iter_mut() {
            if member.runtime_id == *runtime_id && member.live {
                member.live = false;
                member.retiring = false;
                self.journal_fact(
                    MemberLifecycleFact::retired(
                        member_id(identity),
                        member.generation,
                        member.profile.clone(),
                    )
                    .with_session(member.session_id.clone()),
                );
            }
        }
    }

    /// Where Core's member of `identity` stands: the incarnation that holds
    /// the identity, retiring once its retirement started, or none.
    pub(crate) fn member_standing(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberStanding, BridgeError> {
        Ok(match self.live(identity) {
            Some(member) if member.retiring => MemberStanding::retiring(member.generation),
            Some(member) => MemberStanding::seated(member.generation),
            None => MemberStanding::unseated(),
        })
    }

    fn reads_retired_in_history(&self, identity: &AgentIdentity) -> bool {
        self.retired_in_history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity)
    }

    /// `identity`'s `member` as Core's member history reads it.
    fn history_binding(
        &self,
        identity: &AgentIdentity,
        member: &TestMember,
    ) -> meerkat_mob::MemberBinding {
        let read = TestMember {
            live: member.live && !self.reads_retired_in_history(identity),
            ..member.clone()
        };
        member_binding(identity, &read, self.is_host_made(identity))
    }

    /// From now on `identity`'s member, live or ended, reads as one a host
    /// made directly on the mob under the identity's member id: its binding
    /// carries neither the durable-identity marker nor the identity's label,
    /// so no identity-first spawn made it.
    pub(crate) fn mark_host_made(&self, identity: &AgentIdentity) {
        self.host_made
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(identity.clone());
    }

    fn is_host_made(&self, identity: &AgentIdentity) -> bool {
        self.host_made
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(identity)
    }

    /// From now on the member lifecycle journal reads answer, with every
    /// fact journaled so far; until then they answer `Unsupported`, as a
    /// bridge without the journal does.
    pub(crate) fn keep_journal(&self) {
        self.journal.store(true, Ordering::SeqCst);
    }

    fn journal_fact(&self, fact: MemberLifecycleFact) {
        self.facts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(fact);
    }

    pub(crate) fn journal_head(&self) -> Result<MemberLifecycleJournalHead, BridgeError> {
        if !self.journal.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "member_lifecycle_journal_head",
            });
        }
        let facts = self.facts.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(MemberLifecycleJournalHead {
            incarnation: JOURNAL_INCARNATION.to_string(),
            cursor: facts.len() as u64,
        })
    }

    pub(crate) fn journal_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<MemberLifecycleJournalPage, BridgeError> {
        if !self.journal.load(Ordering::SeqCst) {
            return Err(BridgeError::Unsupported {
                operation: "member_lifecycle_journal_page",
            });
        }
        let facts = self.facts.lock().unwrap_or_else(PoisonError::into_inner);
        let records = facts
            .iter()
            .enumerate()
            .map(|(index, fact)| MemberLifecycleFactRecord {
                cursor: index as u64 + 1,
                fact: fact.clone(),
            })
            .filter(|record| record.cursor > after)
            .take(limit.max(1))
            .collect::<Vec<_>>();
        let next = records.last().map_or(after, |record| record.cursor);
        Ok(MemberLifecycleJournalPage { records, next })
    }

    pub(crate) fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        Ok(self.live(identity).map(|member| MemberRuntimeBinding {
            agent_runtime_id: member.runtime_id,
            session_id: member.session_id,
        }))
    }

    pub(crate) fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        let Some(member) = self.live(identity) else {
            return Ok(None);
        };
        let Some(session_id) = member.session_id else {
            return Err(BridgeError::Mob(format!(
                "{identity} is a member bound to no session"
            )));
        };
        meerkat_mob::MemberDeliveryScope::from_json_value(serde_json::json!({
            "version": 1,
            "runtime_id": {
                "identity": crate::member_comms_id::roster_member_id_for_identity(
                    identity.as_str()
                )
                .to_string(),
                "generation": member.generation,
            },
            "fence_token": member.fence,
            "session_id": session_id.to_string(),
        }))
        .map(Some)
        .map_err(|error| BridgeError::Mob(error.to_string()))
    }

    pub(crate) fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(members
            .get(identity)
            .map(|member| self.history_binding(identity, member)))
    }

    pub(crate) fn history(&self) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        let bindings = members
            .iter()
            .map(|(identity, member)| self.history_binding(identity, member))
            .collect::<Vec<_>>();
        // Ended bindings first, so a session a live binding still names
        // resolves to that binding.
        let sessions = self
            .ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(identity, member)| self.history_binding(identity, member))
            .chain(bindings.iter().cloned())
            .collect::<Vec<_>>();
        Ok(meerkat_mob::MemberBindingHistory::for_test(
            bindings,
            sessions,
            [],
            members.len() as u64,
        ))
    }

    /// Core's live members spawned with a durable identity intent, in
    /// identity order: Core's own filter over [`Self::history`].
    pub(crate) fn durable_identity_members(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
        Ok(self
            .history()?
            .durable_identity_members()
            .cloned()
            .collect())
    }

    /// The session and generation of the incarnation `runtime_id` while it
    /// is its identity's live member; `None` once it is retired or replaced.
    pub(crate) fn live_incarnation(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Option<(Option<SessionId>, u64)> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        members
            .values()
            .find(|member| member.live && member.runtime_id == *runtime_id)
            .map(|member| (member.session_id.clone(), member.generation))
    }

    fn live(&self, identity: &AgentIdentity) -> Option<TestMember> {
        let members = self.members.lock().unwrap_or_else(PoisonError::into_inner);
        members.get(identity).filter(|member| member.live).cloned()
    }
}

/// The Core member id of `identity`'s member.
fn member_id(identity: &AgentIdentity) -> meerkat_mob::AgentIdentity {
    crate::member_comms_id::roster_member_id_for_identity(identity.as_str())
}

fn generation_of(runtime_id: &AgentRuntimeId) -> u64 {
    runtime_id
        .as_str()
        .rsplit(':')
        .next()
        .and_then(|generation| generation.parse().ok())
        .unwrap_or(0)
}

/// `identity`'s binding as Core's history reads it; a member a host made
/// (`host_made`) carries neither the durable-identity marker nor the label.
fn member_binding(
    identity: &AgentIdentity,
    member: &TestMember,
    host_made: bool,
) -> meerkat_mob::MemberBinding {
    let binding = meerkat_mob::MemberBinding::for_test(
        crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
        meerkat_mob::Generation::new(member.generation),
        member.session_id.clone(),
        if member.live {
            meerkat_mob::MemberBindingState::Live
        } else {
            meerkat_mob::MemberBindingState::Retired
        },
        member.profile.clone(),
        if host_made {
            meerkat_mob::SpawnContinuityIntent::Ephemeral
        } else {
            meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: identity.as_str().to_string(),
            }
        },
        meerkat_mob::MemberIdentityManagement::CommandDriven,
    );
    if host_made {
        return binding;
    }
    // Every identity-first spawn labels its member with the identity.
    binding.with_labels(BTreeMap::from([(
        "agent_identity".to_owned(),
        identity.as_str().to_owned(),
    )]))
}
