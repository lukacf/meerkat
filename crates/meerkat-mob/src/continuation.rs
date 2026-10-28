//! Original-task continuations addressed to mob members.
//!
//! A member continuation is delivered to the member's incarnation, not to a
//! session: the incarnation (`identity` at `generation`, in one incarnation of
//! its mob) survives a repoint to a successor session, and a respawn under the
//! same identity is a new generation that never receives an older
//! incarnation's rows. A mob destroyed and created again under the same id is
//! a new mob incarnation whose generations start over, so its members never
//! receive the rows of the old mob's members either.

use std::sync::Arc;

use meerkat::{
    AddressObservation, AddressResolution, ContinuationAddressResolver, ContinuationOwner,
};
use meerkat_core::SessionId;
use meerkat_core::service::MobIncarnationId;
use meerkat_runtime::LogicalRuntimeId;

use crate::ids::{AgentIdentity, MobId};
use crate::runtime::MobHandle;

/// Finds the live handle of a mob by id; `None` when the host holds no such
/// mob (it was destroyed, or never existed here).
#[async_trait::async_trait]
pub trait MobHandleLookup: Send + Sync {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle>;

    /// Every mob the host holds now, with its id: the rosters a session's
    /// member is looked up in. The default holds none.
    async fn mob_handles(&self) -> Vec<(MobId, MobHandle)> {
        Vec::new()
    }
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
        meerkat::member_delivery_address(
            mob_id,
            handle.incarnation(),
            identity,
            entry.generation.get(),
        )
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
        match handle
            .serves_member_address_incarnation(member.incarnation)
            .await
        {
            Ok(true) => {}
            // The addressed member belonged to an earlier mob under this id,
            // destroyed since: it never serves again.
            Ok(false) => return Ok(AddressResolution::Retired),
            // Whether this mob serves an address from before 0.9.4 cannot
            // be read now: its rows wait.
            Err(_) => return Ok(AddressResolution::NotServed),
        }
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
        // Serving means live: the delivery wakes the member through
        // MobMachine (`WakeMember`, cause HostDelivery), dormant or with an
        // executor the runtime retired, once per resolution. A wake that
        // cannot happen now leaves the address unserved (its rows wait for
        // the next resolution); a member or mob that is gone for good
        // retires it.
        let identity = AgentIdentity::from(member.identity.as_str());
        match handle.wake_member_for_delivery(&identity).await {
            Ok(()) => Ok(AddressResolution::Session(session_id)),
            Err(
                crate::MobError::MemberNotFound(_)
                | crate::MobError::InvalidTransition {
                    from: crate::MobState::Completed | crate::MobState::Destroyed,
                    ..
                },
            ) => Ok(AddressResolution::Retired),
            // A mob that is not running refuses every wake; only one that
            // completed or was destroyed never serves the address again.
            Err(crate::MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::MobNotRunning,
                ..
            }) => match handle.status().await {
                Ok(crate::MobState::Completed | crate::MobState::Destroyed) => {
                    Ok(AddressResolution::Retired)
                }
                _ => Ok(AddressResolution::NotServed),
            },
            Err(_) => Ok(AddressResolution::NotServed),
        }
    }

    /// The member whose current session is `session_id`, by roster: its
    /// incarnation's address. A superseded session names no member.
    async fn member_address_of_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        for (mob_id, handle) in self.mobs.mob_handles().await {
            let roster = handle.roster().await;
            if let Some(entry) = roster.find_by_bridge_session_id(session_id) {
                return meerkat::member_delivery_address(
                    mob_id.as_str(),
                    handle.incarnation(),
                    entry.agent_identity.as_str(),
                    entry.generation.get(),
                )
                .map(Some)
                .map_err(|error| error.to_string());
            }
        }
        Ok(None)
    }

    /// Observed from the roster and the mob's published lifecycle only, with
    /// nothing revived: an incarnation is retired when the mob under its id
    /// is another mob incarnation (its own mob was destroyed and created
    /// again), when its identity left the roster or was respawned at another
    /// generation, or when its mob completed or was destroyed. A completed
    /// mob can be reset, but a reset first retires every member incarnation,
    /// so an incarnation addressed in a completed mob never serves again. A
    /// mob this host does not manage now may still be registered, so its
    /// members are not proven retired.
    async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressObservation, String> {
        let Some(member) = address
            .member_address()
            .map_err(|error| error.to_string())?
        else {
            return meerkat::SessionAddressResolver
                .observe_address(address)
                .await;
        };
        let Some(handle) = self
            .mobs
            .mob_handle(&MobId::from(member.mob_id.as_str()))
            .await
        else {
            return Ok(AddressObservation::MayServe);
        };
        if !handle
            .serves_member_address_incarnation(member.incarnation)
            .await
            .map_err(|error| error.to_string())?
        {
            return Ok(AddressObservation::Retired);
        }
        let roster = handle.roster().await;
        let Some(entry) = roster.get_by_identity(&AgentIdentity::from(member.identity.as_str()))
        else {
            return Ok(AddressObservation::Retired);
        };
        if entry.generation.get() != member.generation {
            return Ok(AddressObservation::Retired);
        }
        // The phase alone decides. A mob whose actor has stopped (it
        // fail-stopped, or its task ended) keeps its last live phase and its
        // members' addresses stay `MayServe`: `Retired` means the records
        // prove the incarnation never serves again, and recovery resumes this
        // one, so a continuation bound to it stays pending for it.
        Ok(match handle.status_observation_snapshot() {
            crate::MobState::Completed | crate::MobState::Destroyed => AddressObservation::Retired,
            _ => AddressObservation::MayServe,
        })
    }
}

impl MobHandle {
    /// A member continuation submitted through this handle names its mob by
    /// id, and the service binds whichever mob holds the id when it
    /// submits. A handle kept from a destroyed mob (a fork_off or council
    /// completion that ends after the destroy) must not reach a mob created
    /// again under the id, so it submits nothing. The phase is published
    /// before the destroy returns, to every clone.
    ///
    /// A handle of a mob whose actor has stopped (it fail-stopped, or its
    /// task ended) still submits: that mob is not retired, the continuation
    /// commits durably for the member's incarnation, and recovery resumes
    /// that incarnation. Refusing would drop the producer's completion.
    fn require_undestroyed_continuation_owner(
        &self,
    ) -> Result<(), meerkat::ContinuationSubmitError> {
        if self.status_observation_snapshot() == crate::MobState::Destroyed {
            return Err(meerkat::ContinuationSubmitError::OwnerRetired);
        }
        Ok(())
    }

    /// Whether this mob serves the members of a delivery address that names
    /// mob incarnation `incarnation`: only the one it runs at. An address
    /// minted before 0.9.4 names none (`None`), and only the incarnation a
    /// mob created before 0.9.4 got at its first resume serves it; a mob
    /// created since is a later incarnation under its id.
    pub(crate) async fn serves_member_address_incarnation(
        &self,
        incarnation: Option<MobIncarnationId>,
    ) -> Result<bool, crate::MobError> {
        match incarnation {
            Some(incarnation) => Ok(incarnation == self.incarnation()),
            None => self.incarnation_is_migrated().await,
        }
    }

    /// Submit a continuation for one of this mob's members, by identity.
    ///
    /// The member's incarnation current at the key's first submission is
    /// bound by the key ledger; `continuations` must resolve members through
    /// this host's mobs (for example with [`MobContinuationResolver`]). A
    /// handle of a destroyed mob submits nothing and answers
    /// [`meerkat::ContinuationSubmitError::OwnerRetired`], even once a mob is
    /// created again under its id. The commit is bound to this handle's mob
    /// incarnation
    /// ([`meerkat::ContinuationOwnerService::submit_for_member_incarnation`]),
    /// so a handle of a mob whose actor stopped commits nothing for a mob
    /// the host holds again under its id, and answers `OwnerRetired` too.
    pub async fn submit_continuation(
        &self,
        continuations: &meerkat::ContinuationOwnerService,
        identity: &AgentIdentity,
        delivery: meerkat::ContinuationDelivery,
        committed_at_ms: u64,
    ) -> Result<meerkat::ContinuationReceipt, meerkat::ContinuationSubmitError> {
        self.require_undestroyed_continuation_owner()?;
        continuations
            .submit_for_member_incarnation(
                &ContinuationOwner::Member {
                    mob_id: self.mob_id().as_str().to_string(),
                    identity: identity.as_str().to_string(),
                },
                self.incarnation(),
                delivery,
                committed_at_ms,
            )
            .await
    }

    /// Submit a Meerkat producer job's outcome for one of this mob's members,
    /// carrying the job's retained work (see
    /// [`meerkat::ContinuationOwnerService::submit_retained_completion`]). A
    /// handle of a destroyed mob submits nothing, and the commit is bound to
    /// this handle's mob incarnation, as for [`Self::submit_continuation`].
    pub async fn submit_retained_continuation(
        &self,
        continuations: &meerkat::ContinuationOwnerService,
        identity: &AgentIdentity,
        delivery: meerkat::ContinuationDelivery,
        job: &meerkat::RetainedJobRecord,
        committed_at_ms: u64,
    ) -> Result<meerkat::ContinuationReceipt, meerkat::ContinuationSubmitError> {
        self.require_undestroyed_continuation_owner()?;
        continuations
            .submit_retained_completion_for_member_incarnation(
                &ContinuationOwner::Member {
                    mob_id: self.mob_id().as_str().to_string(),
                    identity: identity.as_str().to_string(),
                },
                self.incarnation(),
                delivery,
                job,
                committed_at_ms,
            )
            .await
    }
}
