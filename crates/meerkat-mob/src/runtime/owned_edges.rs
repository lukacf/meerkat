//! Owned edges (0.9.11): the edges an identity change declares, each owned
//! by the identity whose change declared it.
//!
//! An identity's create or update lists its peers
//! ([`super::CreatedIdentityRequest::peers`],
//! [`super::StoreOwnedIdentityUpdate::peers_add`]), each with the peer rule
//! that admitted it, and the change's step declares each edge on MobMachine
//! (`DeclareOwnedIdentityEdge`) and writes it into the owner's row
//! ([`crate::CreatedIdentityRecord::owned_edges`]) in the change's store
//! transaction. MobMachine keeps the one edge custody map of the mob
//! (`identity_edge_owners`): an owner is a store-owned identity, and the
//! edge's other end an identity of either custody. A second declaration of
//! an owned edge gives no custody.
//!
//! The map is the edge's desired record; its realization is the mob's
//! ordinary wiring. The edge is wired once both its ends are seated: by the
//! change's own step when they are seated already, and otherwise by the seat
//! of the end that was not (a create's seat, an unarchive, the re-listing of
//! a roster-owned identity, a start). An archive or an unlisting of an end
//! leaves the edge declared and unrealized, and the end's seat realizes it
//! again; no durable record of the unlisting is needed, since the owner's
//! row holds the edge. An edge enables nothing: a member reaches its peers
//! only with the tools its profile grants.
//!
//! An owned edge ends only by its owner's change
//! ([`super::StoreOwnedIdentityUpdate::peers_remove`]), by the host's policy
//! reload that denies it ([`super::MobHandle::end_denied_identity_edge`]), by
//! the removal of either end, which ends it in the removal's own step and
//! store transaction with the tombstone, and by the mob's reset or destroy.
//! A plain unwire of an owned edge is refused
//! ([`crate::MobError::OwnedIdentityEdge`]): no other identity's change, the
//! requester's included, ends it. The start recovers each edge from its
//! owner's row (`RecoverOwnedIdentityEdge`) and realizes the edges a crash
//! left unrealized.

use crate::MobError;
use crate::ids::AgentIdentity;

use super::identity_change::IdentityChangeCommand;

/// What the end of an owned edge by the host's policy reload answers
/// ([`super::MobHandle::end_denied_identity_edge`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OwnedEdgeEndOutcome {
    /// The edge ended: its owner's row no longer holds it, and its
    /// realization is unwired.
    Ended,
    /// Another identity owns the edge than the one named: nothing changed.
    NotOwner,
    /// No identity owns the edge: nothing changed.
    Absent,
}

impl super::MobHandle {
    /// End the owned edge `owner` holds to `peer` because the host's policy
    /// reload denies it under the peer rules now in force (0.9.11). The host
    /// names the edge's owner, as its read of the owner's row shows it; the
    /// edge ends whatever the custody of its other end, and an adopted edge
    /// ends as any other. MobMachine decides the end, the owner's row is
    /// written without the edge before the machine commits, and the edge's
    /// realization is unwired: the members of a wired edge get the unwire's
    /// notices, and the host notifies whom its policy names.
    ///
    /// An edge another identity owns answers
    /// [`OwnedEdgeEndOutcome::NotOwner`] and one no identity owns
    /// [`OwnedEdgeEndOutcome::Absent`], both with nothing changed. Refused
    /// typed ([`MobError::IdentityChangeRefused`]): the mob's identity
    /// changes held (retryable), a candidate composition, a mob that is not
    /// running and a custom storage composition.
    pub async fn end_denied_identity_edge(
        &self,
        owner: AgentIdentity,
        peer: AgentIdentity,
    ) -> Result<OwnedEdgeEndOutcome, MobError> {
        self.send_actor_command(|reply_tx| {
            super::MobCommand::IdentityChange(Box::new(IdentityChangeCommand::EndDeniedEdge {
                owner,
                peer,
                reply_tx,
            }))
        })
        .await?
    }

    /// Realize every owned edge whose ends are both seated and which the mob
    /// has not wired yet: a start's and a resume's pass after their seats.
    /// Never fails its caller: an edge it cannot realize stays owed its
    /// repair, and the failure is logged.
    pub(super) async fn realize_owned_identity_edges(&self) {
        let realized = self
            .send_actor_command(|reply_tx| {
                super::MobCommand::IdentityChange(Box::new(
                    IdentityChangeCommand::RealizeOwnedEdges { reply_tx },
                ))
            })
            .await;
        if let Err(error) = realized {
            tracing::warn!(
                mob_id = %self.mob_id(),
                error = %error,
                "the owned edges a start left unrealized were not realized"
            );
        }
    }
}
