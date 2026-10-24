//! fork_off and council outcomes through the durable continuation owner.
//!
//! A detached job's outcome is recorded once by its committed owner (the
//! fork job's `ForkJobTerminal` event, the council's custody record) with the
//! outcome's result digest, then submitted as a continuation: durable from
//! submission and applied by the host's delivery owner on its wakes, whether
//! or not the owner is live now. The applied input is the same
//! `BackgroundJob` completion record a live delivery admitted, under the
//! same `{tool}:{job_id}` key, so the transcript is unchanged and a
//! completion admitted before an upgrade deduplicates against it.
//!
//! On a runtime with a native work authorization host the continuation
//! carries the dispatching run's retained work, read back from the job's
//! committed record, and is admitted as a resume of exactly that work.

use std::sync::{Arc, Weak};

use meerkat::{
    AddressObservation, AddressResolution, ContinuationAddressResolver, ContinuationBody,
    ContinuationDelivery, ContinuationHandling, ContinuationKey, ContinuationOwner,
    ContinuationOwnerService, ContinuationProducer, ContinuationResultRef, ContinuationSubmitError,
    HostOriginalBinder, HostOriginalLookup, RetainedJobFacts, RetainedJobLookup, RetainedJobRecord,
    RetainedJobSource,
};
use meerkat_core::SessionId;
use meerkat_core::event::BackgroundJobTerminalStatus;
use meerkat_mob::{MobHandle, MobId};
use meerkat_runtime::LogicalRuntimeId;

use crate::MobMcpState;
use crate::detached_delivery::{
    DetachedCompletionDelivered, DetachedCompletionError, DetachedCompletionOwner,
    DetachedOwnerError, admitted_detached_completion, detached_completion_key,
    detached_completion_notice,
};

/// The continuation producer of a detached tool.
pub(crate) fn producer_for(tool: &'static str) -> ContinuationProducer {
    if tool == crate::agent_tools::TOOL_COUNCIL {
        ContinuationProducer::Council
    } else {
        ContinuationProducer::ForkOff
    }
}

/// Where this host submits detached outcomes: the continuation owner over
/// its delivery inbox, the committed job owner, and the runtime whose input
/// ledger already holds any completion delivered before continuations.
#[derive(Clone)]
pub(crate) struct DetachedCompletionSink {
    pub(crate) continuations: Arc<ContinuationOwnerService>,
    pub(crate) jobs: Arc<dyn RetainedJobSource>,
    pub(crate) runtime: Arc<meerkat_runtime::MeerkatMachine>,
}

impl std::fmt::Debug for DetachedCompletionSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DetachedCompletionSink")
            .finish_non_exhaustive()
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        meerkat_core::time_compat::SystemTime::now()
            .duration_since(meerkat_core::time_compat::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

impl DetachedCompletionSink {
    /// Submit job `job_id`'s outcome, already recorded by its owner under
    /// `result_digest`, for its owner.
    ///
    /// A completion the job's owner session already admitted (live, before
    /// an upgrade, or before the member was repointed) is not submitted
    /// again. Submission is idempotent by the job's key.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn submit(
        &self,
        owner: &DetachedCompletionOwner,
        owner_session_id: &SessionId,
        tool: &'static str,
        job_id: &str,
        status: BackgroundJobTerminalStatus,
        outcome: &serde_json::Value,
        result_digest: &str,
    ) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
        if admitted_detached_completion(&self.runtime, owner_session_id, tool, job_id)
            .await
            .is_some()
        {
            return Ok(DetachedCompletionDelivered::AlreadyDelivered);
        }
        let producer = producer_for(tool);
        let delivery = ContinuationDelivery {
            key: ContinuationKey::new(detached_completion_key(tool, job_id)).map_err(|error| {
                DetachedCompletionError::Encode {
                    tool,
                    detail: error.to_string(),
                }
            })?,
            result: ContinuationResultRef {
                producer: producer.clone(),
                producer_id: job_id.to_string(),
                result_digest: result_digest.to_string(),
                summary: None,
            },
            body: ContinuationBody::notice(detached_completion_notice(
                tool, job_id, status, outcome,
            )?),
            handling: ContinuationHandling::Queue,
        };
        let committed_at_ms = now_ms();
        let submitted = if self.continuations.governs_work_authority() {
            let job = RetainedJobRecord::from_owner(self.jobs.as_ref(), producer, job_id)
                .await
                .map_err(|detail| DetachedCompletionError::Runtime { tool, detail })?
                .ok_or_else(|| DetachedCompletionError::Rejected {
                    tool,
                    detail: "the job's owner holds no committed outcome for it".into(),
                })?;
            match owner {
                DetachedCompletionOwner::Member(handle, identity) => {
                    handle
                        .submit_retained_continuation(
                            &self.continuations,
                            identity,
                            delivery,
                            &job,
                            committed_at_ms,
                        )
                        .await
                }
                DetachedCompletionOwner::Session => {
                    self.continuations
                        .submit_retained_completion(
                            &ContinuationOwner::Session {
                                session_id: owner_session_id.clone(),
                            },
                            delivery,
                            &job,
                            committed_at_ms,
                        )
                        .await
                }
            }
        } else {
            match owner {
                DetachedCompletionOwner::Member(handle, identity) => {
                    handle
                        .submit_continuation(
                            &self.continuations,
                            identity,
                            delivery,
                            committed_at_ms,
                        )
                        .await
                }
                DetachedCompletionOwner::Session => {
                    self.continuations
                        .submit(
                            &ContinuationOwner::Session {
                                session_id: owner_session_id.clone(),
                            },
                            delivery,
                            committed_at_ms,
                        )
                        .await
                }
            }
        };
        submitted
            .map(|_| DetachedCompletionDelivered::Delivered)
            .map_err(|error| match error {
                ContinuationSubmitError::OwnerRetired => DetachedCompletionError::OwnerGone {
                    tool,
                    detail: "the owner's incarnation is retired".into(),
                },
                ContinuationSubmitError::Unavailable(detail) => {
                    DetachedCompletionError::Runtime { tool, detail }
                }
                other => DetachedCompletionError::Rejected {
                    tool,
                    detail: other.to_string(),
                },
            })
    }
}

/// A host's own committed-result owner for exactly these `Host` namespaces
/// (its committed setup results), composed into the job source a
/// [`MobMcpState`] binds ([`MobMcpState::with_host_job_source`]).
#[derive(Clone)]
pub struct HostJobSource {
    namespaces: std::collections::BTreeSet<String>,
    source: Arc<dyn RetainedJobSource>,
}

impl std::fmt::Debug for HostJobSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostJobSource")
            .field("namespaces", &self.namespaces)
            .finish_non_exhaustive()
    }
}

/// Why a [`HostJobSource`] was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostJobSourceError {
    /// No namespace was named: the source would own nothing.
    #[error("a host job source must own at least one namespace")]
    NoNamespaces,
    /// An empty namespace names no host operation.
    #[error("a host job source namespace must not be empty")]
    EmptyNamespace,
    /// The namespace is Core-owned ([`meerkat::RESERVED_HOST_NAMESPACE_PREFIX`],
    /// for example consent notices): no host source may own it.
    #[error("host namespace {0:?} is reserved for Core")]
    ReservedNamespace(String),
}

impl HostJobSource {
    /// `source` answers for the `Host` producer in exactly `namespaces`:
    /// their committed setup results and their host originals.
    ///
    /// # Errors
    /// No namespace, an empty one, or a reserved (Core-owned) one.
    pub fn new(
        namespaces: impl IntoIterator<Item = impl Into<String>>,
        source: Arc<dyn RetainedJobSource>,
    ) -> Result<Self, HostJobSourceError> {
        let namespaces = namespaces
            .into_iter()
            .map(Into::into)
            .collect::<std::collections::BTreeSet<String>>();
        if namespaces.is_empty() {
            return Err(HostJobSourceError::NoNamespaces);
        }
        if namespaces.iter().any(String::is_empty) {
            return Err(HostJobSourceError::EmptyNamespace);
        }
        if let Some(reserved) = namespaces
            .iter()
            .find(|namespace| meerkat::is_reserved_host_namespace(namespace))
        {
            return Err(HostJobSourceError::ReservedNamespace(reserved.clone()));
        }
        Ok(Self { namespaces, source })
    }

    fn owns(&self, producer: &ContinuationProducer) -> bool {
        matches!(producer, ContinuationProducer::Host { namespace } if self.owns_namespace(namespace))
    }

    fn owns_namespace(&self, namespace: &str) -> bool {
        self.namespaces.contains(namespace)
    }
}

/// The job source a state binds: its own fork_off and council owner, and the
/// host's owner for exactly its declared namespaces. Any other host namespace
/// is owned by no one here.
pub(crate) struct ComposedJobSource {
    pub(crate) native: Arc<MobJobOwner>,
    pub(crate) host: Option<HostJobSource>,
}

#[async_trait::async_trait]
impl RetainedJobSource for ComposedJobSource {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        match producer {
            ContinuationProducer::Host { .. } => match &self.host {
                Some(host) if host.owns(producer) => {
                    host.source.retained_job(producer, job_id).await
                }
                _ => RetainedJobLookup::Absent,
            },
            _ => self.native.retained_job(producer, job_id).await,
        }
    }

    /// Host originals come from the host's source for exactly its declared
    /// namespaces; any other namespace is owned by no one here.
    async fn host_original(&self, namespace: &str, producer_id: &str) -> HostOriginalLookup {
        match &self.host {
            Some(host) if host.owns_namespace(namespace) => {
                host.source.host_original(namespace, producer_id).await
            }
            _ => HostOriginalLookup::Absent,
        }
    }

    fn host_original_binder(&self, namespace: &str) -> Option<Arc<dyn HostOriginalBinder>> {
        match &self.host {
            Some(host) if host.owns_namespace(namespace) => {
                host.source.host_original_binder(namespace)
            }
            _ => None,
        }
    }
}

/// The committed owner of this host's fork_off and council jobs: the mob
/// event streams (`ForkJobTerminal`) and the council custody store.
pub(crate) struct MobJobOwner {
    pub(crate) state: Weak<MobMcpState>,
}

#[async_trait::async_trait]
impl RetainedJobSource for MobJobOwner {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        let Some(state) = self.state.upgrade() else {
            return RetainedJobLookup::Unavailable("the mob host is gone".into());
        };
        match producer {
            ContinuationProducer::ForkOff => {
                let handles: Vec<MobHandle> = state
                    .managed_mob_handles()
                    .await
                    .into_iter()
                    .map(|(_, handle)| handle)
                    .collect();
                let mut found = Vec::new();
                for handle in handles {
                    match handle.fork_job_terminals().await {
                        Ok(terminals) => found.extend(
                            terminals
                                .into_iter()
                                .filter(|terminal| terminal.job_id == job_id),
                        ),
                        Err(meerkat_mob::ForkJobTerminalError::Store(error)) => {
                            return RetainedJobLookup::Unavailable(error.to_string());
                        }
                        // A stream whose terminals fail validation confirms
                        // no committed outcome.
                        Err(_) => return RetainedJobLookup::Absent,
                    }
                }
                match <[_; 1]>::try_from(found) {
                    Ok([terminal]) => RetainedJobLookup::Found(RetainedJobFacts {
                        owner_session_id: terminal.owner_session_id,
                        retained_work: terminal.retained_work,
                        result_digest: terminal.result_digest,
                    }),
                    // Not found while the host may still restore the job's
                    // mob (its persistent restore, or a host inserting its
                    // restored handles, is not done): the completion waits,
                    // never refused for a mob that comes back.
                    Err(found) if found.is_empty() && !state.mob_set_complete() => {
                        RetainedJobLookup::Unavailable(format!(
                            "no mob of this host holds fork_off job {job_id} before its mobs are restored"
                        ))
                    }
                    // None, or a job id a host gave to more than one child:
                    // no single committed job confirms the completion.
                    Err(_) => RetainedJobLookup::Absent,
                }
            }
            ContinuationProducer::Council => {
                match state.council_job_binding(job_id).await {
                    Ok(Some(binding)) => match binding.terminal {
                        Some(terminal) => RetainedJobLookup::Found(RetainedJobFacts {
                            owner_session_id: binding.owner_session_id,
                            retained_work: binding.retained_work,
                            result_digest: terminal.result_digest,
                        }),
                        // A council recorded before committed outcomes
                        // existed has no digest to confirm against.
                        None => RetainedJobLookup::Absent,
                    },
                    Ok(None) => RetainedJobLookup::Absent,
                    Err(error) => RetainedJobLookup::Unavailable(error),
                }
            }
            ContinuationProducer::Host { .. } => RetainedJobLookup::Absent,
        }
    }
}

#[async_trait::async_trait]
impl meerkat_mob::continuation::MobHandleLookup for MobJobOwner {
    async fn mob_handle(&self, mob_id: &MobId) -> Option<MobHandle> {
        let state = self.state.upgrade()?;
        state
            .managed_mob_handles()
            .await
            .into_iter()
            .find_map(|(id, handle)| (&id == mob_id).then_some(handle))
    }

    async fn mob_handles(&self) -> Vec<(MobId, MobHandle)> {
        match self.state.upgrade() {
            Some(state) => state.managed_mob_handles().await,
            None => Vec::new(),
        }
    }
}

/// The host's continuation resolver: members through their mobs (revived
/// by them), plain sessions through the host's owner hook
/// ([`crate::DetachedOwnerHost`]), which makes a session whose executor the
/// runtime retired live again before its continuation is applied.
pub(crate) struct HostContinuationResolver {
    pub(crate) mobs: meerkat_mob::continuation::MobContinuationResolver,
    pub(crate) state: Weak<MobMcpState>,
}

#[async_trait::async_trait]
impl ContinuationAddressResolver for HostContinuationResolver {
    async fn current_address(
        &self,
        owner: &ContinuationOwner,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        self.mobs.current_address(owner).await
    }

    async fn member_address_of_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<LogicalRuntimeId>, String> {
        self.mobs.member_address_of_session(session_id).await
    }

    async fn resolve_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressResolution, String> {
        let resolution = self.mobs.resolve_address(address).await?;
        let member = address
            .member_address()
            .map_err(|error| error.to_string())?;
        let Some(state) = self.state.upgrade() else {
            return Ok(match resolution {
                AddressResolution::Session(_) => AddressResolution::NotServed,
                other => other,
            });
        };
        if let Some(member) = member {
            // A member of a mob this host does not manage waits for its
            // handle, unless the mob is gone for good.
            if resolution == AddressResolution::NotServed
                && state
                    .mob_is_retired(&MobId::from(member.mob_id.as_str()))
                    .await
            {
                return Ok(AddressResolution::Retired);
            }
            return Ok(resolution);
        }
        let AddressResolution::Session(session_id) = &resolution else {
            return Ok(resolution);
        };
        // A session that does not read is not served now, and its absence
        // proves nothing (see `observe_address`): retired only on its own
        // lifecycle proof, otherwise it waits unserved with its rows owed
        // until the next wake retries it. A read that fails answers nothing
        // about the owner: the address is unavailable, and the owner hook is
        // not asked.
        match state.session_service().read(session_id).await {
            Ok(_) => {}
            Err(meerkat_core::service::SessionError::NotFound { .. }) => {
                return retired_or_unserved(&state, session_id).await;
            }
            Err(error) => {
                return Err(format!("the owner session could not be read: {error}"));
            }
        }
        let Some(host) = state.detached_owner_host() else {
            return Ok(resolution);
        };
        match host.ensure_owner_live(session_id).await {
            Ok(()) => Ok(resolution),
            // The host holds no such session now, which a host also answers
            // for a session that is only absent for the moment: retired only
            // on the same proof.
            Err(DetachedOwnerError::OwnerGone { .. }) => {
                retired_or_unserved(&state, session_id).await
            }
            Err(DetachedOwnerError::Failed { .. }) => Ok(AddressResolution::NotServed),
        }
    }

    /// Observed, never served: the member's roster and mob, the host's
    /// retired mobs, and a session owner's durable lifecycle, with no owner
    /// hook. Making an owner live is delivery work, so a status read through
    /// a service that outlived its binding revives nothing.
    async fn observe_address(
        &self,
        address: &LogicalRuntimeId,
    ) -> Result<AddressObservation, String> {
        if self.mobs.observe_address(address).await? == AddressObservation::Retired {
            return Ok(AddressObservation::Retired);
        }
        let member = address
            .member_address()
            .map_err(|error| error.to_string())?;
        let Some(state) = self.state.upgrade() else {
            return Ok(AddressObservation::MayServe);
        };
        if let Some(member) = member {
            // A member of a mob this host does not manage is retired only
            // once the mob is gone for good.
            return Ok(
                if state
                    .mob_is_retired(&MobId::from(member.mob_id.as_str()))
                    .await
                {
                    AddressObservation::Retired
                } else {
                    AddressObservation::MayServe
                },
            );
        }
        let AddressResolution::Session(session_id) = meerkat::default_address_resolution(address)
        else {
            return Ok(AddressObservation::MayServe);
        };
        Ok(if session_proven_retired(&state, &session_id).await? {
            AddressObservation::Retired
        } else {
            AddressObservation::MayServe
        })
    }
}

/// Whether a session owner's own records prove it retired: its durable
/// lifecycle is archived over a destroyed runtime, which the owner's resume
/// verdict holds stably not revivable. Mere absence proves nothing: an
/// ephemeral session whose live actor was discarded reads as absent and is
/// restored under the same id, an archived session over a live runtime may
/// be revived, and a session may not be persisted yet. Read only: nothing is
/// materialized, recovered or served. `Err` when the lifecycle cannot be
/// observed now.
async fn session_proven_retired(
    state: &MobMcpState,
    session_id: &SessionId,
) -> Result<bool, String> {
    let authority = state
        .session_service()
        .observe_session_resume_authority(session_id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(matches!(
        authority.lifecycle(),
        meerkat_mob::SessionResumeLifecycle::Archived {
            runtime_state: Some(meerkat_runtime::RuntimeState::Destroyed),
            ..
        }
    ))
}

/// A session owner no session serves now: [`AddressResolution::Retired`] on
/// its own retirement proof ([`session_proven_retired`]), otherwise
/// [`AddressResolution::NotServed`], so its rows stay owed and its runtime
/// blocked for the next wake.
async fn retired_or_unserved(
    state: &MobMcpState,
    session_id: &SessionId,
) -> Result<AddressResolution, String> {
    Ok(if session_proven_retired(state, session_id).await? {
        AddressResolution::Retired
    } else {
        AddressResolution::NotServed
    })
}
