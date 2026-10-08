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
    AddressResolution, ContinuationAddressResolver, ContinuationBody, ContinuationDelivery,
    ContinuationHandling, ContinuationKey, ContinuationOwner, ContinuationOwnerService,
    ContinuationProducer, ContinuationResultRef, ContinuationSubmitError, RetainedJobFacts,
    RetainedJobLookup, RetainedJobRecord, RetainedJobSource,
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
        // A session that no longer reads (archived, deleted, or never
        // persisted) never serves again.
        if let Err(meerkat_core::service::SessionError::NotFound { .. }) =
            state.session_service().read(session_id).await
        {
            return Ok(AddressResolution::Retired);
        }
        let Some(host) = state.detached_owner_host() else {
            return Ok(resolution);
        };
        Ok(match host.ensure_owner_live(session_id).await {
            Ok(()) => resolution,
            Err(DetachedOwnerError::OwnerGone { .. }) => AddressResolution::Retired,
            Err(DetachedOwnerError::Failed { .. }) => AddressResolution::NotServed,
        })
    }
}
