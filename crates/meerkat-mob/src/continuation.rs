//! Original-task continuations addressed to mob members.
//!
//! A member continuation is delivered to the member's incarnation, not to a
//! session: the incarnation (`identity` at `generation`) survives a repoint to
//! a successor session, and a respawn under the same identity is a new
//! generation that never receives an older incarnation's rows.

use std::sync::Arc;

use meerkat::{AddressResolution, ContinuationAddressResolver, ContinuationOwner};
use meerkat_runtime::LogicalRuntimeId;

use crate::ids::{AgentIdentity, MobId};
use crate::runtime::MobHandle;

/// Finds the live handle of a mob by id; `None` when the host holds no such
/// mob (it was destroyed, or never existed here).
#[async_trait::async_trait]
pub trait MobHandleLookup: Send + Sync {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle>;
}

/// Resolves continuation owners and delivery addresses through mob rosters,
/// and standalone sessions as their own runtimes.
#[derive(Clone)]
pub struct MobContinuationResolver {
    mobs: Arc<dyn MobHandleLookup>,
}

impl std::fmt::Debug for MobContinuationResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobContinuationResolver")
            .finish_non_exhaustive()
    }
}

impl MobContinuationResolver {
    pub fn new(mobs: Arc<dyn MobHandleLookup>) -> Self {
        Self { mobs }
    }
}

#[async_trait::async_trait]
impl ContinuationAddressResolver for MobContinuationResolver {
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        let ContinuationOwner::Member { mob_id, identity } = owner else {
            return meerkat::SessionAddressResolver.current_address(owner).await;
        };
        let Some(handle) = self.mobs.mob_handle(&MobId::from(mob_id.as_str())).await else {
            return Ok(None);
        };
        let roster = handle.roster().await;
        let Some(entry) = roster.get_by_identity(&AgentIdentity::from(identity.as_str())) else {
            return Ok(None);
        };
        meerkat::member_delivery_address(mob_id, identity, entry.generation.get())
            .map(Some)
            .map_err(|error| error.to_string())
    }

    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String> {
        let Some(member) = address
            .member_address()
            .map_err(|error| error.to_string())?
        else {
            return meerkat::SessionAddressResolver
                .resolve_address(address)
                .await;
        };
        let Some(handle) = self
            .mobs
            .mob_handle(&MobId::from(member.mob_id.as_str()))
            .await
        else {
            // A mob this host does not manage now may be registered later (a
            // host restores mob handles after building its mob state), so
            // its members' rows wait rather than strand.
            return Ok(AddressResolution::NotServed);
        };
        let roster = handle.roster().await;
        let Some(entry) = roster.get_by_identity(&AgentIdentity::from(member.identity.as_str()))
        else {
            return Ok(AddressResolution::Retired);
        };
        if entry.generation.get() != member.generation {
            // The identity was respawned: the addressed incarnation is gone.
            return Ok(AddressResolution::Retired);
        }
        let Some(session_id) = entry.bridge_session_id().cloned() else {
            return Ok(AddressResolution::NotServed);
        };
        // Serving means live: revive a member whose executor the runtime
        // retired, once per resolution. A revival that cannot happen now
        // leaves the address unserved (its rows wait for the next wake); a
        // member or mob that is gone for good retires it.
        let identity = AgentIdentity::from(member.identity.as_str());
        match handle.ensure_member_live(&identity).await {
            Ok(()) => Ok(AddressResolution::Session(session_id)),
            Err(
                crate::MobError::MemberNotFound(_)
                | crate::MobError::InvalidTransition {
                    from: crate::MobState::Completed | crate::MobState::Destroyed,
                    ..
                },
            ) => Ok(AddressResolution::Retired),
            Err(_) => Ok(AddressResolution::NotServed),
        }
    }
}

impl MobHandle {
    /// Submit a continuation for one of this mob's members, by identity.
    ///
    /// The member's incarnation current at the key's first submission is
    /// bound by the key ledger; `continuations` must resolve members through
    /// this host's mobs (for example with [`MobContinuationResolver`]).
    pub async fn submit_continuation(
        &self,
        continuations: &meerkat::ContinuationOwnerService,
        identity: &AgentIdentity,
        delivery: meerkat::ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<meerkat::ContinuationReceipt, meerkat::ContinuationSubmitError> {
        continuations
            .submit(
                &ContinuationOwner::Member {
                    mob_id: self.mob_id().as_str().to_string(),
                    identity: identity.as_str().to_string(),
                },
                delivery,
                committed_at_ms,
            )
            .await
    }

    /// Submit a Meerkat producer job's outcome for one of this mob's members,
    /// carrying the job's retained work (see
    /// [`meerkat::ContinuationOwnerService::submit_retained_completion`]).
    pub async fn submit_retained_continuation(
        &self,
        continuations: &meerkat::ContinuationOwnerService,
        identity: &AgentIdentity,
        delivery: meerkat::ContinuationDelivery,
        job: &meerkat::RetainedJobRecord,
        committed_at_ms: u64,
    ) -> Result<meerkat::ContinuationReceipt, meerkat::ContinuationSubmitError> {
        continuations
            .submit_retained_completion(
                &ContinuationOwner::Member {
                    mob_id: self.mob_id().as_str().to_string(),
                    identity: identity.as_str().to_string(),
                },
                delivery,
                job,
                committed_at_ms,
            )
            .await
    }
}
