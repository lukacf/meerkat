//! Approval branches: one-time members forked from a grantee at an empty
//! prefix, each running the one approved work of a `grant.child` decision.
//!
//! A branch is the grantee forked with none of its messages (also while the
//! grantee's own turn runs), seated under the grantee's role with the
//! authority label [`APPROVAL_BRANCH_LABEL`] naming the decision, exempt
//! from the definition's wiring and wired to no parent: nothing converses
//! with it but its approved work. Its identity derives from the decision, so
//! opening it again finds it.
//!
//! The branch is private to its opening. Its seat records the decision as
//! the member's typed creation source (its `MemberSpawned` row and roster
//! entry), which only [`MobHandle::open_approval_branch`] sets: the opening
//! adopts an existing member only when that source names its decision. The
//! identity prefix `approval-branch-` and the label are reserved on every
//! other spawn, ensure, respawn and fork path, and nothing wires, unwires or
//! respawns an open branch: each is refused
//! [`MobError::ApprovalBranchReserved`], in the mob's own admission, so every
//! surface inherits the refusal.

use std::collections::BTreeMap;

use meerkat_core::SessionId;
use meerkat_core::approval::decision::{APPROVAL_BRANCH_LABEL, DecisionId};

use super::handle::{MobHandle, SpawnMemberSpec, SpawnSource};
use crate::error::MobError;
use crate::ids::AgentIdentity;

/// The prefix of every approval branch's member identity, reserved for the
/// branches' own opening.
pub(crate) const APPROVAL_BRANCH_IDENTITY_PREFIX: &str = "approval-branch-";

/// Whether `identity` or `labels` take an approval branch's reserved
/// identity prefix or its authority label.
pub(crate) fn takes_approval_branch_reservation(
    identity: &AgentIdentity,
    labels: Option<&BTreeMap<String, String>>,
) -> bool {
    identity
        .as_str()
        .starts_with(APPROVAL_BRANCH_IDENTITY_PREFIX)
        || labels.is_some_and(|labels| labels.contains_key(APPROVAL_BRANCH_LABEL))
}

/// Admit `spec`'s claim on an approval branch's reservation, after the
/// host's spawn customizer ran: a spec that records no branch takes neither
/// the identity prefix nor the label; one that records the branch of a
/// decision is that decision's branch exactly (its identity and label),
/// local, and seated by the branch's own fork seating or by the restore of
/// the member that records it.
///
/// # Errors
/// [`MobError::ApprovalBranchReserved`] otherwise.
pub(crate) fn admit_approval_branch_reservation(
    spec: &SpawnMemberSpec,
    source: SpawnSource,
) -> Result<(), MobError> {
    let reserved = || MobError::ApprovalBranchReserved {
        identity: spec.identity.clone(),
    };
    match &spec.approval_branch {
        None if takes_approval_branch_reservation(&spec.identity, spec.labels.as_ref()) => {
            Err(reserved())
        }
        None => Ok(()),
        Some(decision) => {
            let labelled = spec
                .labels
                .as_ref()
                .and_then(|labels| labels.get(APPROVAL_BRANCH_LABEL))
                .map(String::as_str)
                == Some(decision.as_str());
            if labelled
                && spec.identity == MobHandle::approval_branch_identity(decision)
                && spec.placement.is_none()
                && matches!(
                    source,
                    SpawnSource::PersistedForkResume | SpawnSource::Resume
                )
            {
                Ok(())
            } else {
                Err(reserved())
            }
        }
    }
}

/// Adopt `entry`, found under the identity of `decision`'s branch, as that
/// branch: only when its typed creation source names `decision`. Its
/// identity and label never make it one (a member a journal recorded without
/// the source, or with one this build cannot read, is no branch).
///
/// # Errors
/// [`MobError::ApprovalBranchReserved`] otherwise.
fn adopted_branch(
    entry: &crate::roster::RosterEntry,
    decision: &DecisionId,
) -> Result<(), MobError> {
    if entry.approval_branch() == Some(decision) {
        Ok(())
    } else {
        Err(MobError::ApprovalBranchReserved {
            identity: entry.agent_identity.clone(),
        })
    }
}

impl MobHandle {
    /// The member identity of the approval branch of `decision`.
    #[must_use]
    pub fn approval_branch_identity(decision: &DecisionId) -> AgentIdentity {
        let digest = decision
            .as_str()
            .strip_prefix("decision:")
            .unwrap_or(decision.as_str());
        AgentIdentity::from(format!("{APPROVAL_BRANCH_IDENTITY_PREFIX}{digest}"))
    }

    /// Open the one-time approval branch of `grantee` for `decision`: the
    /// grantee forked at an empty prefix ([`Self::fork_member_at_empty_prefix`]),
    /// seated as a member of the grantee's role labelled
    /// [`APPROVAL_BRANCH_LABEL`] with the decision id, exempt from the
    /// definition's wiring and wired to no parent, and recorded with the
    /// decision as its typed creation source. The branch's member and session
    /// are returned. Opening it again answers the branch already open for
    /// `decision`: an existing member is adopted only when its recorded
    /// creation source is this decision's branch.
    ///
    /// # Errors
    /// [`MobError::MemberNotFound`] for an absent grantee;
    /// [`MobError::ApprovalBranchReserved`] when the branch's identity is
    /// held by a member this path did not open for `decision`; the fork's
    /// own errors.
    pub async fn open_approval_branch(
        &self,
        grantee: &AgentIdentity,
        decision: &DecisionId,
    ) -> Result<(AgentIdentity, SessionId), MobError> {
        let identity = Self::approval_branch_identity(decision);
        if let Some(entry) = self.get_member(&identity).await? {
            adopted_branch(&entry, decision)?;
            let session = self
                .resolve_bridge_session_id(&identity)
                .await
                .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
            return Ok((identity, session));
        }
        let role = self
            .get_member(grantee)
            .await?
            .ok_or_else(|| MobError::MemberNotFound(grantee.clone()))?
            .role;
        let labels = BTreeMap::from([(
            APPROVAL_BRANCH_LABEL.to_owned(),
            decision.as_str().to_owned(),
        )]);
        let mut spec = SpawnMemberSpec::new(role, identity)
            .with_labels(labels)
            .with_definition_wiring(crate::DefinitionWiring::Exempt)
            .with_auto_wire_parent(false);
        spec.approval_branch = Some(decision.clone());
        let forked = self.fork_member_at_empty_prefix(grantee, spec).await?;
        Ok((forked.agent_identity, forked.session_id))
    }

    /// Retire the approval branch of `decision` through the ordinary member
    /// retire path ([`Self::retire`]). A retired or never opened branch is
    /// done.
    ///
    /// # Errors
    /// The retirement's own errors.
    pub async fn retire_approval_branch(&self, decision: &DecisionId) -> Result<(), MobError> {
        self.retire(Self::approval_branch_identity(decision)).await
    }

    /// Every open approval branch of this mob, with the decision it runs:
    /// the members whose recorded creation source is a decision's branch,
    /// read from the roster as it stands (no member or mob is woken).
    pub async fn approval_branches(&self) -> Vec<(DecisionId, AgentIdentity)> {
        self.list_all_members()
            .await
            .into_iter()
            .filter_map(|entry| {
                let decision = entry.approval_branch()?.clone();
                Some((decision, entry.agent_identity))
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ids::{AgentRuntimeId, FenceToken, Generation, ProfileName};
    use crate::roster::{Roster, RosterAddEntry, RosterEntry};

    fn a_decision(subject: &str) -> DecisionId {
        DecisionId::derive(
            &meerkat_core::approval::decision::DecisionKind::new("grant.child").expect("kind"),
            &meerkat_core::approval::decision::DecisionSubjectKey::new(subject)
                .expect("subject key"),
        )
    }

    fn label_of(decision: &DecisionId) -> BTreeMap<String, String> {
        BTreeMap::from([(
            APPROVAL_BRANCH_LABEL.to_owned(),
            decision.as_str().to_owned(),
        )])
    }

    /// The roster entry of a member seated under `decision`'s branch identity
    /// and label, whose recorded creation source is `source`.
    fn seated(decision: &DecisionId, source: Option<DecisionId>) -> RosterEntry {
        let identity = MobHandle::approval_branch_identity(decision);
        let mut roster = Roster::new();
        roster.add(RosterAddEntry {
            agent_identity: identity.clone(),
            generation: Generation::INITIAL,
            fence_token: FenceToken::new(1),
            agent_runtime_id: AgentRuntimeId::initial(identity.clone()),
            role: ProfileName::from("worker"),
            runtime_mode: crate::MobRuntimeMode::TurnDriven,
            member_ref: crate::event::MemberRef::from_bridge_session_id(SessionId::new()),
            peer_id: None,
            transport_public_key: None,
            direct_member_fence: None,
            labels: label_of(decision),
            effective_profile_override: None,
            effective_model_override: None,
            spawned_by: None,
            fork_job: None,
            fork_source: None,
            fork_overlay: Default::default(),
            definition_wiring: crate::DefinitionWiring::Exempt,
            approval_branch: source,
            dispatch_admission_lineage: false,
        });
        roster.get(&identity).cloned().expect("the seated entry")
    }

    fn assert_reserved(result: Result<(), MobError>, identity: &AgentIdentity, case: &str) {
        match result {
            Err(MobError::ApprovalBranchReserved { identity: refused }) => {
                assert_eq!(&refused, identity, "{case}");
            }
            other => panic!("{case}: expected the reservation's refusal, got {other:?}"),
        }
    }

    /// The opening adopts a member under its branch identity only when the
    /// member's recorded creation source is its decision: the identity and
    /// the label make no branch, so a member a journal recorded without the
    /// source (or with one this build cannot read) is never adopted.
    #[test]
    fn only_the_recorded_creation_source_makes_a_member_its_decisions_branch() {
        let decision = a_decision("opened");
        let identity = MobHandle::approval_branch_identity(&decision);
        assert!(adopted_branch(&seated(&decision, Some(decision.clone())), &decision).is_ok());
        for source in [None, Some(a_decision("another"))] {
            assert_reserved(
                adopted_branch(&seated(&decision, source.clone()), &decision),
                &identity,
                &format!("recorded source {source:?}"),
            );
        }
    }

    /// A spawn takes an approval branch's identity or label only as that
    /// branch: its own fork seating or the restore of the member that
    /// records it, local, with the decision's identity and label exactly.
    #[test]
    fn a_spawn_takes_a_branch_identity_or_label_only_as_that_branch() {
        let decision = a_decision("opened");
        let identity = MobHandle::approval_branch_identity(&decision);
        let branch = || {
            let mut spec =
                SpawnMemberSpec::new("worker", identity.clone()).with_labels(label_of(&decision));
            spec.approval_branch = Some(decision.clone());
            spec
        };
        for source in [SpawnSource::PersistedForkResume, SpawnSource::Resume] {
            assert!(
                admit_approval_branch_reservation(&branch(), source).is_ok(),
                "{source:?}"
            );
        }
        for source in [
            SpawnSource::Consumer,
            SpawnSource::AgentSpawnMember,
            SpawnSource::HelperSpawn,
            SpawnSource::BatchItem,
            SpawnSource::FlowProvisioning,
            SpawnSource::PolicySpawn,
            SpawnSource::Respawn,
            SpawnSource::Fork,
            SpawnSource::IdentityReconcile,
            SpawnSource::AttachedForkedParticipant,
        ] {
            assert_reserved(
                admit_approval_branch_reservation(&branch(), source),
                &identity,
                &format!("the branch spawned as {source:?}"),
            );
        }
        // The branch of another decision, placed, or labelled otherwise.
        let mut another = branch();
        another.approval_branch = Some(a_decision("another"));
        assert_reserved(
            admit_approval_branch_reservation(&another, SpawnSource::PersistedForkResume),
            &identity,
            "another decision's source",
        );
        let placed = branch().with_placement(crate::machines::mob_machine::HostId::from("host-a"));
        assert_reserved(
            admit_approval_branch_reservation(&placed, SpawnSource::PersistedForkResume),
            &identity,
            "a placed branch",
        );
        let unlabelled = branch().with_labels(BTreeMap::new());
        assert_reserved(
            admit_approval_branch_reservation(&unlabelled, SpawnSource::PersistedForkResume),
            &identity,
            "a branch without its label",
        );
        let renamed = {
            let mut spec = branch();
            spec.identity = AgentIdentity::from("approval-branch-renamed");
            spec
        };
        assert_reserved(
            admit_approval_branch_reservation(&renamed, SpawnSource::PersistedForkResume),
            &AgentIdentity::from("approval-branch-renamed"),
            "a branch under another identity",
        );
        // With no recorded source: the identity prefix or the label.
        assert_reserved(
            admit_approval_branch_reservation(
                &SpawnMemberSpec::new("worker", identity.clone()),
                SpawnSource::Consumer,
            ),
            &identity,
            "the identity prefix",
        );
        let ordinary = AgentIdentity::from("ordinary-member");
        assert_reserved(
            admit_approval_branch_reservation(
                &SpawnMemberSpec::new("worker", ordinary.clone()).with_labels(label_of(&decision)),
                SpawnSource::PersistedForkResume,
            ),
            &ordinary,
            "the label",
        );
        assert!(
            admit_approval_branch_reservation(
                &SpawnMemberSpec::new("worker", ordinary),
                SpawnSource::Consumer,
            )
            .is_ok(),
            "an ordinary spawn takes neither"
        );
    }

    /// The recorded creation source survives the stored event codec and the
    /// roster's replay; a row without it (a journal written before it) or
    /// with a value this build cannot read replays as no branch, and the
    /// public form of the row never shows it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn the_recorded_creation_source_survives_the_journal_and_its_replay() {
        let decision = a_decision("opened");
        let identity = MobHandle::approval_branch_identity(&decision);
        let row = |source: Option<DecisionId>| {
            let mut spawned = crate::event::MemberSpawnedEvent::new(
                identity.clone(),
                Generation::INITIAL,
                FenceToken::new(1),
                AgentRuntimeId::initial(identity.clone()),
                ProfileName::from("worker"),
            )
            .with_bridge_member_ref(Some(crate::event::MemberRef::from_bridge_session_id(
                SessionId::new(),
            )))
            .with_approval_branch(source);
            spawned.labels = label_of(&decision);
            crate::MobEvent {
                cursor: 1,
                timestamp: chrono::Utc::now(),
                mob_id: crate::MobId::from("mob"),
                kind: crate::MobEventKind::MemberSpawned(spawned),
            }
        };
        let replayed = |stored: &[u8]| {
            let restored = crate::event::decode_stored_mob_event(stored).expect("the row decodes");
            Roster::project(&[restored])
                .get(&identity)
                .cloned()
                .expect("the replayed entry")
        };
        let recorded =
            crate::event::encode_stored_mob_event(&row(Some(decision.clone()))).expect("encodes");
        assert_eq!(replayed(&recorded).approval_branch(), Some(&decision));
        let without = crate::event::encode_stored_mob_event(&row(None)).expect("encodes");
        assert_eq!(replayed(&without).approval_branch(), None);
        let mut unreadable: serde_json::Value =
            serde_json::from_slice(&recorded).expect("the stored row is JSON");
        unreadable["event"]["kind"]["approval_branch"] =
            serde_json::Value::from("not a decision id");
        let unreadable = serde_json::to_vec(&unreadable).expect("encodes");
        assert_eq!(replayed(&unreadable).approval_branch(), None);
        let public = serde_json::to_value(row(Some(decision.clone()))).expect("the public form");
        assert!(public["kind"].get("approval_branch").is_none(), "{public}");
    }
}
