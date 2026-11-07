//! Approval branches: one-time members forked from a grantee at an empty
//! prefix, each running the one approved work of a `grant.child` decision.
//!
//! A branch is the grantee forked with none of its messages (also while the
//! grantee's own turn runs), seated under the grantee's role with the
//! authority label [`APPROVAL_BRANCH_LABEL`] naming the decision, exempt
//! from the definition's wiring and wired to no parent: nothing converses
//! with it but its approved work. Its identity derives from the decision, so
//! opening it again finds it.

use std::collections::BTreeMap;

use meerkat_core::SessionId;
use meerkat_core::approval::decision::{APPROVAL_BRANCH_LABEL, DecisionId};

use super::handle::{MobHandle, SpawnMemberSpec};
use crate::error::MobError;
use crate::ids::AgentIdentity;

impl MobHandle {
    /// The member identity of the approval branch of `decision`.
    #[must_use]
    pub fn approval_branch_identity(decision: &DecisionId) -> AgentIdentity {
        let digest = decision
            .as_str()
            .strip_prefix("decision:")
            .unwrap_or(decision.as_str());
        AgentIdentity::from(format!("approval-branch-{digest}"))
    }

    /// Open the one-time approval branch of `grantee` for `decision`: the
    /// grantee forked at an empty prefix ([`Self::fork_member_at_empty_prefix`]),
    /// seated as a member of the grantee's role labelled
    /// [`APPROVAL_BRANCH_LABEL`] with the decision id, exempt from the
    /// definition's wiring and wired to no parent. The branch's member and
    /// session are returned. Opening it again answers the branch already open
    /// for `decision`.
    ///
    /// # Errors
    /// [`MobError::MemberNotFound`] for an absent grantee;
    /// [`MobError::MemberAlreadyExists`] when the branch's identity is held
    /// by a member that is no branch of `decision`; the fork's own errors.
    pub async fn open_approval_branch(
        &self,
        grantee: &AgentIdentity,
        decision: &DecisionId,
    ) -> Result<(AgentIdentity, SessionId), MobError> {
        let identity = Self::approval_branch_identity(decision);
        if let Some(entry) = self.get_member(&identity).await? {
            if entry.labels.get(APPROVAL_BRANCH_LABEL).map(String::as_str)
                != Some(decision.as_str())
            {
                return Err(MobError::MemberAlreadyExists(identity));
            }
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
        let spec = SpawnMemberSpec::new(role, identity)
            .with_labels(labels)
            .with_definition_wiring(crate::DefinitionWiring::Exempt)
            .with_auto_wire_parent(false);
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

    /// Every open approval branch of this mob, with the decision it runs.
    pub async fn approval_branches(&self) -> Vec<(DecisionId, AgentIdentity)> {
        self.list_all_members()
            .await
            .into_iter()
            .filter_map(|entry| {
                let decision =
                    DecisionId::new(entry.labels.get(APPROVAL_BRANCH_LABEL)?.as_str()).ok()?;
                (entry.agent_identity == Self::approval_branch_identity(&decision))
                    .then_some((decision, entry.agent_identity))
            })
            .collect()
    }
}
