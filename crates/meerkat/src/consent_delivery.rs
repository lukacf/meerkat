//! Consent notices through the durable continuation owner.
//!
//! The issuing approval owner commits one delivery obligation per committed
//! consent transition (Approved, Denied, Withdrawn) in the same conditional
//! row write as the transition, owed to the exact owner scope (hosting
//! domain and session) and the retained work of the admitting run. Each
//! obligation is delivered as a `Host` retained completion in
//! [`CONSENT_DELIVERY_NAMESPACE`], job id `{approval_id}/{transition}`:
//!
//! - [`ConsentNoticeJobSource`] is the approval owner as the job's committed
//!   owner. A governed admission confirms the notice against it, exactly as
//!   it confirms a fork_off or council completion against theirs.
//! - [`ConsentNoticeProjector`] commits the owed notices of one hosting
//!   domain as continuation rows, idempotently by key, one finite page at a
//!   time, when the host's delivery owner runs a round
//!   ([`crate::RuntimeDeliveryHost::project_owed_continuations`]): on its
//!   arming reconcile, every reconcile and every source change. An
//!   obligation committed without a source change (an unannounced write)
//!   waits for the next of those; the store watch never finds it.
//!
//! The same rounds watch the issuing owner's parked consent whose approved
//! notice was admitted as an input of one of this domain's sessions: when
//! the runtime reports that input abandoned, superseded or coalesced (read
//! outside the approval owner), no resumed run can take the consent over,
//! and the owner withdraws it after checking the exact parked row, receipt
//! and destination again
//! ([`ApprovalService::settle_consent_notice_input_ended`]). Any other
//! report changes nothing.
//!
//! There is no consent producer variant and no mirrored job table. The
//! obligation stays owed until its admission is recorded; this module never
//! settles it.

use std::sync::{Arc, Weak};

use meerkat_core::approval::consent::{
    CONSENT_DELIVERY_NAMESPACE, ConsentDeliveryFacts, ParkedNoticeInput, consent_delivery_job_id,
    parse_consent_delivery_job_id,
};
use meerkat_core::session_hosting::HostingDomainRef;
use meerkat_core::types::{SystemNoticeKind, SystemNoticeMessage};
use meerkat_core::{
    ApprovalConsentDeliveryState, ApprovalConsentTransition, ApprovalId, ApprovalService,
};
use meerkat_runtime::input_state::InputTerminalOutcome;
use meerkat_runtime::terminal_status::InteractionSelector;
use meerkat_runtime::{
    MeerkatMachine, RuntimeDeliveryInbox, RuntimeDriverError, SessionServiceRuntimeExt as _,
};

use crate::{
    ContinuationBody, ContinuationDelivery, ContinuationHandling, ContinuationKey,
    ContinuationObservation, ContinuationOwner, ContinuationOwnerService, ContinuationProducer,
    ContinuationResultRef, OwedContinuationCursor, OwedContinuationProjection, RetainedJobFacts,
    RetainedJobLookup, RetainedJobRecord, RetainedJobSource, SessionAddressResolver,
};

/// The continuation producer of consent notices.
pub fn consent_notice_producer() -> ContinuationProducer {
    ContinuationProducer::Host {
        namespace: CONSENT_DELIVERY_NAMESPACE.to_string(),
    }
}

/// The issuing approval owner as the committed owner of the consent notices
/// it owes to sessions of one hosting domain: answers
/// [`CONSENT_DELIVERY_NAMESPACE`] jobs by their `{approval_id}/{transition}`
/// ids from the owner's committed rows. A notice committed for another
/// domain is not this domain's job, whatever its session id.
pub struct ConsentNoticeJobSource {
    approvals: Arc<ApprovalService>,
    domain: HostingDomainRef,
}

impl ConsentNoticeJobSource {
    pub fn new(approvals: Arc<ApprovalService>, domain: HostingDomainRef) -> Self {
        Self { approvals, domain }
    }
}

impl std::fmt::Debug for ConsentNoticeJobSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsentNoticeJobSource")
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl RetainedJobSource for ConsentNoticeJobSource {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        if *producer != consent_notice_producer() {
            return RetainedJobLookup::Absent;
        }
        let Some((approval_id, transition)) = parse_consent_delivery_job_id(job_id) else {
            return RetainedJobLookup::Absent;
        };
        match self.approvals.consent_delivery(&approval_id, transition) {
            Ok(Some(facts)) if facts.scope.domain == self.domain => {
                RetainedJobLookup::Found(RetainedJobFacts {
                    owner_session_id: facts.scope.session_id,
                    retained_work: Some(facts.retained_work),
                    result_digest: facts.result_digest,
                })
            }
            // Committed for another hosting domain, or not at all: an
            // unknown or unscoped approval, or a transition that never
            // committed.
            Ok(_) => RetainedJobLookup::Absent,
            // An unavailable owner, or a row awaiting reconciliation: retry
            // later, never absent.
            Err(error) => RetainedJobLookup::Unavailable(error.to_string()),
        }
    }
}

/// Why a consent-aware host answers no other owner yet.
#[cfg(feature = "session-store")]
const UNBOUND_SERVICES: &str = "the host has not bound its continuation services yet";

/// Answers the consent namespace from the approval owner and every other
/// job from the host's bound job source. Until the host binds its
/// continuation services, every other job and every host original outside
/// the reserved namespaces is unavailable (retry later), never absent: a
/// delivery drained in that window (for example by the arming reconcile after
/// a restart) stays pending and is decided by the bound owner once bound, as
/// the setup signal (`Unbound`) says. Built only by the session-store runtime
/// delivery host, so it is compiled with it.
#[cfg(feature = "session-store")]
pub(crate) struct ConsentAwareJobSource {
    pub(crate) consent: ConsentNoticeJobSource,
    pub(crate) others: Option<Arc<dyn RetainedJobSource>>,
}

#[cfg(feature = "session-store")]
#[async_trait::async_trait]
impl RetainedJobSource for ConsentAwareJobSource {
    async fn retained_job(
        &self,
        producer: &ContinuationProducer,
        job_id: &str,
    ) -> RetainedJobLookup {
        if *producer == consent_notice_producer() {
            return self.consent.retained_job(producer, job_id).await;
        }
        match &self.others {
            Some(others) => others.retained_job(producer, job_id).await,
            None => RetainedJobLookup::Unavailable(UNBOUND_SERVICES.to_string()),
        }
    }

    /// Host originals come from the host's bound source only, never in a
    /// reserved (Core-owned) namespace such as the consent namespace.
    async fn host_original(&self, namespace: &str, producer_id: &str) -> crate::HostOriginalLookup {
        if crate::is_reserved_host_namespace(namespace) {
            return crate::HostOriginalLookup::Absent;
        }
        match &self.others {
            Some(others) => others.host_original(namespace, producer_id).await,
            None => crate::HostOriginalLookup::Unavailable(UNBOUND_SERVICES.to_string()),
        }
    }

    fn host_original_binder(&self, namespace: &str) -> Option<Arc<dyn crate::HostOriginalBinder>> {
        if crate::is_reserved_host_namespace(namespace) {
            return None;
        }
        self.others.as_ref()?.host_original_binder(namespace)
    }
}

/// One entry of a consent round, as the issuing owner's whole-owner
/// discovery reports it, in the round's stable order (its cursor key).
/// Every entry is acted on in the same finite page that visits it; a later
/// kind of entry joins this enumeration rather than another queue.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConsentRoundEntry {
    /// A committed transition's notice, still owed.
    OwedNotice {
        approval_id: ApprovalId,
        transition: ApprovalConsentTransition,
    },
    /// Parked consent whose approved notice was admitted as an input of one
    /// of this domain's sessions: that input's native terminal can end it.
    AdmittedNoticeInput(ParkedNoticeInput),
}

impl ConsentRoundEntry {
    /// The entry's position in the round.
    fn cursor_key(&self) -> String {
        match self {
            Self::OwedNotice {
                approval_id,
                transition,
            } => format!(
                "notice:{}",
                consent_delivery_job_id(approval_id, *transition)
            ),
            Self::AdmittedNoticeInput(parked) => format!("input:{}", parked.approval_id),
        }
    }
}

/// Commits, for a host's delivery owner, the continuation rows of the
/// consent notices the issuing approval owner owes to sessions of one
/// hosting domain, into the host's delivery inbox. Discovery is the owner's
/// whole-owner enumeration; the projector pages over it in a stable order.
///
/// It keeps the issuing approval owner and the inbox, and holds the host's
/// runtime only weakly, as the host does: the delivery owner's loop holds
/// its host for as long as it runs, so a strong runtime here would keep the
/// loop's own runtime, and with it the runtime's attachment signal, alive.
/// Each page takes the runtime for that page only.
pub struct ConsentNoticeProjector {
    approvals: Arc<ApprovalService>,
    domain: HostingDomainRef,
    jobs: ConsentNoticeJobSource,
    inbox: RuntimeDeliveryInbox,
    runtime: Weak<MeerkatMachine>,
}

impl std::fmt::Debug for ConsentNoticeProjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsentNoticeProjector")
            .field("domain", &self.domain)
            .finish_non_exhaustive()
    }
}

impl ConsentNoticeProjector {
    /// A projector of `approvals`' notices for `domain`, committing into
    /// `inbox` (the inbox the host's delivery owner drains) for `runtime`
    /// (the host's runtime, held weakly).
    pub fn new(
        approvals: Arc<ApprovalService>,
        domain: HostingDomainRef,
        inbox: RuntimeDeliveryInbox,
        runtime: &Arc<MeerkatMachine>,
    ) -> Self {
        Self {
            jobs: ConsentNoticeJobSource::new(Arc::clone(&approvals), domain.clone()),
            approvals,
            domain,
            inbox,
            runtime: Arc::downgrade(runtime),
        }
    }

    /// Changes of the issuing approval owner's consent notice obligations
    /// ([`ApprovalService::subscribe_consent_deliveries`]): every change the
    /// owner announces. Only a wake: a round reads the owner's committed
    /// facts again.
    pub fn subscribe_owed_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.approvals.subscribe_consent_deliveries()
    }

    /// The committed owner of this projector's notices, for the host's
    /// retained-job confirmation.
    pub fn job_source(&self) -> ConsentNoticeJobSource {
        ConsentNoticeJobSource::new(Arc::clone(&self.approvals), self.domain.clone())
    }

    /// One page of a round: discover what the owner owes, then act on at
    /// most `limit` entries after `after` in the round's order. A notice
    /// whose row is already committed counts nothing and stays owed until
    /// its admission. An unavailable owner fails the page, never reports
    /// nothing owed; rows the owner could not reconcile are carried as
    /// failures (on the round's first page), so the owner's next wake starts
    /// another round and its discovery reconciles them again.
    pub async fn project(
        &self,
        after: Option<&OwedContinuationCursor>,
        limit: usize,
    ) -> OwedContinuationProjection {
        let mut projection = OwedContinuationProjection::default();
        let discovery = match self.approvals.owed_consent_deliveries() {
            Ok(discovery) => discovery,
            Err(error) => {
                projection.failures.push(format!(
                    "the consent owner's notices could not be discovered: {error}"
                ));
                return projection;
            }
        };
        if after.is_none() {
            projection
                .failures
                .extend(discovery.unreconciled.iter().map(|approval_id| {
                    format!(
                        "consent approval {approval_id} is not reconciled yet: any notice it \
                         owes stays owed until a later discovery"
                    )
                }));
        }
        let mut entries: Vec<(String, ConsentRoundEntry)> = discovery
            .owed
            .into_iter()
            .map(|(approval_id, transition)| ConsentRoundEntry::OwedNotice {
                approval_id,
                transition,
            })
            .chain(
                discovery
                    .parked_notice_inputs
                    .into_iter()
                    // Only this domain's runtime admitted these inputs, and
                    // only it can report how they ended.
                    .filter(|parked| parked.scope.domain == self.domain)
                    .map(ConsentRoundEntry::AdmittedNoticeInput),
            )
            .map(|entry| (entry.cursor_key(), entry))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        let start = after.map_or(0, |after| {
            entries.partition_point(|(key, _)| key.as_str() <= after.as_str())
        });
        let end = start.saturating_add(limit).min(entries.len());
        let page = entries.get(start..end).unwrap_or_default();
        if page.is_empty() {
            return projection;
        }
        // This page's custody of the host runtime: the continuation service
        // that commits the page's rows and the page's native reads hold it,
        // and all end with the page.
        let Some(runtime) = self.runtime.upgrade() else {
            projection
                .failures
                .push("the host runtime is gone: its consent notices stay owed".to_string());
            return projection;
        };
        let continuations = ContinuationOwnerService::new(
            self.inbox.clone(),
            Arc::new(SessionAddressResolver),
            Arc::clone(&runtime),
        );
        for (_, entry) in page {
            match entry {
                ConsentRoundEntry::OwedNotice {
                    approval_id,
                    transition,
                } => match self
                    .project_notice(&continuations, approval_id, *transition)
                    .await
                {
                    Ok(true) => projection.committed += 1,
                    Ok(false) => {}
                    Err(error) => projection.failures.push(format!(
                        "consent notice {} was not committed: {error}",
                        consent_delivery_job_id(approval_id, *transition)
                    )),
                },
                ConsentRoundEntry::AdmittedNoticeInput(parked) => {
                    if let Err(error) = self.settle_ended_notice_input(&runtime, parked).await {
                        projection.failures.push(format!(
                            "parked consent {} was not settled for its admitted notice input \
                             {}: {error}",
                            parked.approval_id, parked.input_id
                        ));
                    }
                }
            }
        }
        if end < entries.len()
            && let Some((key, _)) = page.last()
        {
            projection.next = Some(OwedContinuationCursor::new(key.clone()));
        }
        projection
    }

    /// Read, outside the approval owner, how the runtime ended `parked`'s
    /// admitted notice input. An input it abandoned, superseded or coalesced
    /// can never be staged for a resumed run, so the owner withdraws the
    /// parked consent, after checking the exact parked row, its receipt and
    /// the full destination again under its own reservation. An input still
    /// pending, one a run consumed (that run's close decides), and an input
    /// or session this runtime does not know prove nothing and change
    /// nothing; the next round asks again.
    async fn settle_ended_notice_input(
        &self,
        runtime: &MeerkatMachine,
        parked: &ParkedNoticeInput,
    ) -> Result<(), String> {
        let terminal = match runtime
            .interaction_terminal_status(
                &parked.scope.session_id,
                InteractionSelector::InputId(parked.input_id.clone()),
            )
            .await
        {
            Ok(status) => status.and_then(|sourced| sourced.report.terminal),
            Err(RuntimeDriverError::NotFound { .. } | RuntimeDriverError::NotReady { .. }) => {
                return Ok(());
            }
            Err(error) => return Err(error.to_string()),
        };
        if !terminal.as_ref().is_some_and(ends_notice_input) {
            return Ok(());
        }
        self.approvals
            .settle_consent_notice_input_ended(&parked.approval_id, &parked.scope, &parked.input_id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// Commit one owed notice's row, owed to its committed scope's session.
    /// `Ok(false)` when it is no longer owed, owed to another hosting
    /// domain, or its row is already committed. The domain-scoped job source
    /// is the one place that decides a notice is this domain's: reading the
    /// notice's record through it finds no other domain's notice, and
    /// neither does an admission's confirmation.
    async fn project_notice(
        &self,
        continuations: &ContinuationOwnerService,
        approval_id: &ApprovalId,
        transition: ApprovalConsentTransition,
    ) -> Result<bool, String> {
        let Some(facts) = self
            .approvals
            .consent_delivery(approval_id, transition)
            .map_err(|error| error.to_string())?
        else {
            return Ok(false);
        };
        if facts.state != ApprovalConsentDeliveryState::Owed {
            return Ok(false);
        }
        let job_id = consent_delivery_job_id(approval_id, transition);
        let Some(record) =
            RetainedJobRecord::from_owner(&self.jobs, consent_notice_producer(), job_id.clone())
                .await?
        else {
            return Ok(false);
        };
        let owner = ContinuationOwner::Session {
            session_id: facts.scope.session_id.clone(),
        };
        let delivery = consent_notice_delivery(&job_id, approval_id, transition, &facts)?;
        match continuations
            .observe_retained_completion(&owner, &delivery, &record)
            .await
        {
            Ok(ContinuationObservation::NotSubmitted) => {}
            Ok(ContinuationObservation::Matched { .. }) => return Ok(false),
            Err(error) => return Err(error.to_string()),
        }
        continuations
            .submit_retained_completion(&owner, delivery, &record, now_ms())
            .await
            .map(|_| true)
            .map_err(|error| error.to_string())
    }
}

/// The continuation of one committed consent transition's notice: keyed by
/// its job id, naming the approval owner's committed digest, with a body
/// that depends on nothing else, so every submission is the same content.
fn consent_notice_delivery(
    job_id: &str,
    approval_id: &ApprovalId,
    transition: ApprovalConsentTransition,
    facts: &ConsentDeliveryFacts,
) -> Result<ContinuationDelivery, String> {
    Ok(ContinuationDelivery {
        key: ContinuationKey::new(format!("consent:{job_id}"))
            .map_err(|error| error.to_string())?,
        result: ContinuationResultRef {
            producer: consent_notice_producer(),
            producer_id: job_id.to_string(),
            result_digest: facts.result_digest.clone(),
            summary: None,
        },
        body: ContinuationBody::notice(SystemNoticeMessage::new(
            SystemNoticeKind::Generic,
            format!(
                "Consent for approval {approval_id} was {}.",
                transition.as_str()
            ),
        )),
        handling: ContinuationHandling::Queue,
    })
}

/// Whether the runtime's terminal of an admitted notice input means no
/// resumed run can take its parked consent over: it was abandoned, or
/// superseded or coalesced into another input, which never inherits the
/// consent. A consumed input proves nothing here (the close of the run that
/// consumed it decides), and neither does any other terminal.
fn ends_notice_input(terminal: &InputTerminalOutcome) -> bool {
    matches!(
        terminal,
        InputTerminalOutcome::Abandoned { .. }
            | InputTerminalOutcome::Superseded { .. }
            | InputTerminalOutcome::Coalesced { .. }
    )
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

#[cfg(test)]
mod tests {
    use meerkat_core::lifecycle::InputId;
    use meerkat_runtime::input_state::{InputAbandonReason, InputTerminalOutcome};

    use super::ends_notice_input;

    /// Every abandonment, a supersession and a coalescing end a parked
    /// consent's notice input; a consumption does not.
    #[test]
    fn only_an_input_no_run_can_take_ends_its_parked_consent() {
        for reason in [
            InputAbandonReason::Retired,
            InputAbandonReason::Reset,
            InputAbandonReason::Stopped,
            InputAbandonReason::Destroyed,
            InputAbandonReason::Cancelled,
            InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
            InputAbandonReason::NeverExecuted,
            InputAbandonReason::ToolProcessInterrupted,
            InputAbandonReason::RequeueExpired,
            InputAbandonReason::ResumeRefused,
        ] {
            assert!(
                ends_notice_input(&InputTerminalOutcome::Abandoned {
                    reason: reason.clone()
                }),
                "{reason:?}"
            );
        }
        assert!(ends_notice_input(&InputTerminalOutcome::Superseded {
            superseded_by: InputId::new(),
        }));
        assert!(ends_notice_input(&InputTerminalOutcome::Coalesced {
            aggregate_id: InputId::new(),
        }));
        assert!(!ends_notice_input(&InputTerminalOutcome::Consumed));
    }
}

#[cfg(all(test, feature = "session-store"))]
#[allow(clippy::expect_used)]
mod host_original_tests {
    use std::sync::Arc;

    use meerkat_core::SessionId;
    use meerkat_core::session_hosting::HostingPaths;

    use super::{ConsentAwareJobSource, ConsentNoticeJobSource};
    use crate::{
        BoundHostOriginal, ContinuationDelivery, ContinuationHandling, ContinuationKey,
        ContinuationOwner, ContinuationProducer, ContinuationResultRef, HostOriginalBinder,
        HostOriginalBinderReadiness, HostOriginalFacts, HostOriginalLookup, HostOriginalRef,
        HostOriginalRefusal, HostOriginalSettlement, RetainedJobLookup, RetainedJobSource,
    };

    /// A host source that claims every host original and offers a binder for
    /// every namespace.
    struct ClaimsEverything(HostOriginalFacts);

    #[async_trait::async_trait]
    impl RetainedJobSource for ClaimsEverything {
        async fn retained_job(
            &self,
            _producer: &ContinuationProducer,
            _job_id: &str,
        ) -> RetainedJobLookup {
            RetainedJobLookup::Absent
        }

        async fn host_original(&self, _namespace: &str, _producer_id: &str) -> HostOriginalLookup {
            HostOriginalLookup::Found(self.0.clone())
        }

        fn host_original_binder(&self, _namespace: &str) -> Option<Arc<dyn HostOriginalBinder>> {
            Some(Arc::new(InertBinder))
        }
    }

    struct InertBinder;

    #[async_trait::async_trait]
    impl HostOriginalBinder for InertBinder {
        async fn bind(
            &self,
            _original: &HostOriginalRef,
            _session: &SessionId,
            _input: meerkat_runtime::Input,
        ) -> Result<BoundHostOriginal, HostOriginalRefusal> {
            Err(HostOriginalRefusal::NotOwned)
        }

        async fn readiness(
            &self,
            _namespace: &str,
            _owner: &ContinuationOwner,
        ) -> HostOriginalBinderReadiness {
            HostOriginalBinderReadiness::Ready
        }

        fn retain_unresolved(&self, _settlement: Box<dyn HostOriginalSettlement>, _detail: &str) {}
    }

    fn facts() -> HostOriginalFacts {
        HostOriginalFacts::new(
            ContinuationOwner::Session {
                session_id: SessionId::new(),
            },
            ContinuationDelivery {
                key: ContinuationKey::new("ingest:1").expect("key"),
                result: ContinuationResultRef {
                    producer: ContinuationProducer::Host {
                        namespace: "app.ingest".into(),
                    },
                    producer_id: "observation-1".into(),
                    result_digest: "sha256:observation-1".into(),
                    summary: None,
                },
                body: "a new item arrived".into(),
                handling: ContinuationHandling::Queue,
            },
            meerkat_runtime::OriginalWorkRef {
                authority: meerkat_core::PrincipalRef::in_domain(
                    meerkat_core::PrincipalKind::ServiceAccount,
                    "collector",
                    meerkat_core::TrustDomainId::new("host-test").expect("domain"),
                )
                .expect("qualified"),
                work: meerkat_authorization_contracts::evidence::EvidenceId::new("observation-1")
                    .expect("work id"),
            },
        )
    }

    /// The consent-aware composition forwards host originals and their binder
    /// to the host's source, but never in a reserved (Core-owned) namespace,
    /// the consent namespace included, whatever the host's source claims.
    #[tokio::test]
    async fn the_consent_aware_source_never_forwards_a_reserved_namespace() {
        let lock_dir = "/tmp/consent-aware-host-original-test";
        let domain = HostingPaths {
            hosting_lock_dir: lock_dir.into(),
            cold_delivery_lock: format!("{lock_dir}/cold-delivery.lock").into(),
            database: None,
        }
        .domain_ref();
        let facts = facts();
        let source = ConsentAwareJobSource {
            consent: ConsentNoticeJobSource::new(
                Arc::new(meerkat_core::ApprovalService::new()),
                domain,
            ),
            others: Some(Arc::new(ClaimsEverything(facts.clone()))),
        };
        assert_eq!(
            source.host_original("app.ingest", "observation-1").await,
            HostOriginalLookup::Found(facts)
        );
        assert!(source.host_original_binder("app.ingest").is_some());
        for reserved in [
            meerkat_core::approval::consent::CONSENT_DELIVERY_NAMESPACE,
            "meerkat.other",
        ] {
            assert_eq!(
                source.host_original(reserved, "observation-1").await,
                HostOriginalLookup::Absent,
                "{reserved}"
            );
            assert!(
                source.host_original_binder(reserved).is_none(),
                "{reserved}"
            );
        }
    }

    fn unbound_source() -> ConsentAwareJobSource {
        let lock_dir = "/tmp/consent-aware-unbound-test";
        ConsentAwareJobSource {
            consent: ConsentNoticeJobSource::new(
                Arc::new(meerkat_core::ApprovalService::new()),
                HostingPaths {
                    hosting_lock_dir: lock_dir.into(),
                    cold_delivery_lock: format!("{lock_dir}/cold-delivery.lock").into(),
                    database: None,
                }
                .domain_ref(),
            ),
            others: None,
        }
    }

    /// Before the host binds its continuation services, a host original
    /// outside the reserved namespaces is unavailable (retry later), never
    /// absent: a delivery drained in that window stays pending, as the
    /// readiness signal (`Unbound`) promises. A reserved namespace stays
    /// absent.
    #[tokio::test]
    async fn an_unbound_consent_aware_source_answers_a_host_original_unavailable() {
        let source = unbound_source();
        assert!(
            matches!(
                source.host_original("app.ingest", "observation-1").await,
                HostOriginalLookup::Unavailable(_)
            ),
            "unavailable until the host binds its services, never absent"
        );
        assert!(source.host_original_binder("app.ingest").is_none());
        assert_eq!(
            source
                .host_original(
                    meerkat_core::approval::consent::CONSENT_DELIVERY_NAMESPACE,
                    "observation-1"
                )
                .await,
            HostOriginalLookup::Absent,
            "a reserved namespace is never anyone's"
        );
    }

    /// The retained-completion twin: before the host binds its continuation
    /// services, another producer's job is unavailable (retry later), never
    /// absent, so its retained completion stays pending instead of being
    /// refused.
    #[tokio::test]
    async fn an_unbound_consent_aware_source_answers_another_producers_job_unavailable() {
        let source = unbound_source();
        assert!(
            matches!(
                source
                    .retained_job(&ContinuationProducer::ForkOff, "job-1")
                    .await,
                RetainedJobLookup::Unavailable(_)
            ),
            "unavailable until the host binds its services, never absent"
        );
    }
}
