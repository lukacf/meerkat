//! The durable worker resume gate over the mob realm's WorkGraph.
//!
//! After a crash, kill or restart of a persisted mob, Meerkat rebuilds a
//! durable worker on its session and resumes the run it was in from its
//! last committed round. A durable worker is any member spawned with a
//! durable identity: the members of a `durable = true` profile, and every
//! identity-first member, whose spawn always carries one. Before that resume
//! Meerkat asks the host's [`meerkat_mob::DurableResumeGate`] whether the
//! run's work is still the worker's. Work leases stay host-side: Meerkat
//! renews no WorkGraph claim, so a worker that was down longer than its lease
//! can come back to an item that was released or taken.
//! [`WorkGraphResumeGate`] reads the mob realm's WorkGraph and answers:
//!
//! - **With the run's attention projection** (the
//!   `workgraph.attention_projection` one of its bound inputs was dispatched
//!   under), the projected item decides. The run is superseded when the item
//!   lives outside the mob's realm, is gone or terminal, when another owner
//!   holds its claim, when the worker's own claim lease has lapsed, or when
//!   the projection's attention binding has stopped or now targets another
//!   owner. It continues otherwise.
//! - **With a projection that does not decode**, the gate can neither name
//!   the item nor treat the run as one without attention, so it answers
//!   unavailable.
//! - **Without one**, every non-terminal item whose claim the worker holds is
//!   checked. A lapsed lease supersedes the run; otherwise, including when
//!   the worker holds no claim, the run continues.
//! - A lapsed lease is first committed with `observe_lease_expiry`, as a
//!   lease sweep would, so the item can be claimed again.
//! - A WorkGraph read fault answers unavailable. Meerkat asks again a bounded
//!   number of times and then settles the run as superseded.
//!
//! Meerkat drops an attempt that outlasts its timeout and asks again, so the
//! gate is idempotent across the attempts of one resume: a superseded verdict
//! is remembered for the run before anything is written, and every later
//! attempt answers it.
//!
//! The worker is "this worker" under either of its WorkGraph names: its
//! member owner key (`agent:mob/<mob_id>/agent/<identity>`, the only form a
//! mob lowers a member identity to) or its session (`session:<id>`).
//!
//! A candidate launch binds no gate ([`launch_resume_gate`]): it holds every
//! stored member's run starts, and Meerkat asks the gate before it checks
//! the hold, so a verdict on a candidate could settle the authoritative
//! launch's run. A launch whose backend has no WorkGraph binds none either:
//! there is no item and no claim to check, and the run resumes unchecked, as
//! on a launch without a WorkGraph service.
//!
//! Known limit: take a run with no attention projection whose claim lapsed
//! and was taken by another worker while it was down. It holds no claim any
//! more, so the gate cannot see the move and the run continues. Its later
//! WorkGraph writes are still revision-checked and conflict.
//!
//! The claim scan pages through every open item with the WorkGraph list
//! cursor and does not use the pages' `changed_since_anchor`. A claimed item
//! that a write moves ahead of the cursor before the scan reaches it is not
//! seen, so the run continues, as in the known limit, and its later writes
//! conflict on revision. An item comes back twice only after the store clock
//! moved back; the scan then keeps the newest claimed sighting.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use chrono::{DateTime, Utc};
use meerkat::{
    AttentionBindingRequest, AttentionContextProjection, ObserveLeaseExpiryRequest,
    WorkAttentionTarget, WorkClaim, WorkGraphError, WorkGraphService, WorkItem, WorkItemFilter,
    WorkItemId, WorkOwnerKey, WorkStatus,
};
use meerkat_core::SessionId;
use meerkat_core::lifecycle::RunId;

/// How many non-terminal items one page of the claim scan reads: the
/// WorkGraph's largest list page. The scan pages through every open item
/// with the list cursor.
const CLAIM_SCAN_PAGE: usize = 1000;

/// How many superseded runs the gate remembers. A superseded run is settled
/// and never resumed again, so only the attempts of a resume still in
/// flight need the record.
const REMEMBERED_SUPERSEDED_RUNS: usize = 256;

/// The gate's answer before it is handed to Meerkat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResumeCheck {
    /// The worker still holds its work: resume the run.
    Continue,
    /// The work moved: Meerkat settles the run the way a stop does, and it
    /// never resumes.
    Superseded(String),
    /// The gate cannot decide: Meerkat asks again, then settles the run as
    /// superseded.
    Unavailable(String),
}

impl ResumeCheck {
    fn into_verdict(self) -> meerkat_mob::DurableResumeVerdict {
        match self {
            Self::Continue => meerkat_mob::DurableResumeVerdict::Continue,
            Self::Superseded(reason) => meerkat_mob::DurableResumeVerdict::Superseded { reason },
            Self::Unavailable(reason) => meerkat_mob::DurableResumeVerdict::Unavailable { reason },
        }
    }
}

/// The worker whose interrupted run Meerkat is about to resume, as the gate
/// reads it off the request.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResumingWorker<'a> {
    pub mob_id: &'a meerkat_mob::MobId,
    pub agent_identity: &'a meerkat_mob::AgentIdentity,
    pub session_id: &'a SessionId,
    pub run_id: &'a RunId,
    pub attention_projection: Option<&'a AttentionContextProjection>,
    /// Why the run's attention projection does not decode, when its overlay
    /// carries one that does not.
    pub attention_projection_unreadable: Option<&'a str>,
}

/// The runs this gate already superseded, so every later attempt of the
/// same resume answers the same. Meerkat drops an attempt that outlasts its
/// timeout, and a dropped attempt may already have committed a lapsed
/// lease's expiry, after which the item reads as unclaimed: a fresh decision
/// would then let the run continue. The verdict is remembered before that
/// write, keyed by the worker's session and run.
#[derive(Default)]
struct SupersededRuns {
    runs: Mutex<VecDeque<(String, String)>>,
}

impl SupersededRuns {
    fn key(worker: ResumingWorker<'_>) -> String {
        format!("{}/{}", worker.session_id, worker.run_id)
    }

    fn recall(&self, run: &str) -> Option<String> {
        self.lock()
            .iter()
            .find(|(key, _)| key == run)
            .map(|(_, reason)| reason.clone())
    }

    /// Remember `reason` for `run`; the first reason stays.
    fn remember(&self, run: &str, reason: &str) {
        let mut runs = self.lock();
        if runs.iter().any(|(key, _)| key == run) {
            return;
        }
        if runs.len() >= REMEMBERED_SUPERSEDED_RUNS {
            runs.pop_front();
        }
        runs.push_back((run.to_string(), reason.to_string()));
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<(String, String)>> {
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The worker's two WorkGraph names.
struct WorkerKeys<'a> {
    /// `None` when the identity cannot be lowered to a member owner key (it
    /// contains `/`): such a member can hold no member-key claim.
    member: Option<WorkOwnerKey>,
    session: Option<WorkOwnerKey>,
    session_id: &'a SessionId,
}

impl<'a> WorkerKeys<'a> {
    fn of(worker: ResumingWorker<'a>) -> Self {
        Self {
            member: meerkat_mob::lower_agent_identity_owner_key(
                worker.mob_id,
                worker.agent_identity,
            )
            .ok(),
            session: WorkOwnerKey::session(worker.session_id.to_string()).ok(),
            session_id: worker.session_id,
        }
    }

    fn names(&self, key: &WorkOwnerKey) -> bool {
        self.member.as_ref() == Some(key) || self.session.as_ref() == Some(key)
    }

    fn is_target(&self, target: &WorkAttentionTarget) -> bool {
        match target {
            WorkAttentionTarget::Session { session_id } => session_id == self.session_id,
            WorkAttentionTarget::LoweredOwner { owner_key } => self.names(owner_key),
        }
    }
}

/// [`meerkat_mob::DurableResumeGate`] over one mob realm's WorkGraph: the
/// same mob-scoped service the mob's attention overlays read.
pub(crate) struct WorkGraphResumeGate {
    service: WorkGraphService,
    superseded: SupersededRuns,
}

/// The resume gate a launch binds over its mob realm's `service`: none
/// without a service, none over a backend that has no WorkGraph, and none
/// on a candidate launch. With no gate bound Meerkat asks no resumed-run
/// admission. A store without a WorkGraph refuses every read, so a gate
/// over it could only answer unavailable until Meerkat settles the run as
/// superseded; it holds no item and no claim, so there is nothing to
/// check, as without a service. On a candidate, a run it holds stays
/// parked with its mark: the gate's verdict, and the lease expiry it
/// commits, belong to the authoritative launch.
pub(crate) fn launch_resume_gate(
    service: Option<WorkGraphService>,
    authority: crate::mob_composition_manifest::CompositionAuthority,
) -> Option<WorkGraphResumeGate> {
    if !authority.speaks_for_composition() {
        return None;
    }
    service
        .filter(|service| service.store().kind() != meerkat::WorkGraphStoreKind::Disabled)
        .map(WorkGraphResumeGate::new)
}

/// The resume gate of the mob `mob_id`, one of the mobs a launch builds,
/// over the launch's WorkGraph `host_service`: none without a service, none
/// over a backend that has no WorkGraph, and none on a candidate launch
/// ([`launch_resume_gate`]). The gate reads the
/// realm of the mob it is for ([`realm_service_of`]): a member's items,
/// claims and attention live in its own mob's realm, so a gate over another
/// mob's realm would find none of them.
pub(crate) fn mob_resume_gate(
    host_service: Option<&WorkGraphService>,
    mob_id: &meerkat_mob::MobId,
    authority: crate::mob_composition_manifest::CompositionAuthority,
) -> Option<WorkGraphResumeGate> {
    launch_resume_gate(
        host_service.map(|host| realm_service_of(host, mob_id)),
        authority,
    )
}

/// The launch's WorkGraph `host` scoped to the realm of the mob `mob_id`,
/// by the formula the mob's members are built with
/// ([`meerkat_mob::mob_scoped_workgraph_service`]). The launch scopes its
/// service to the launch mob, so for that mob this is `host` as it is; it
/// is also `host` as it is where the formula yields no service of another
/// realm (a mob id that forms no realm, a backend without a WorkGraph).
fn realm_service_of(host: &WorkGraphService, mob_id: &meerkat_mob::MobId) -> WorkGraphService {
    match meerkat_mob::mob_scoped_workgraph_service(host, mob_id) {
        Ok(Some(scoped)) if scoped.default_realm_id() != host.default_realm_id() => scoped,
        _ => host.clone(),
    }
}

impl WorkGraphResumeGate {
    pub(crate) fn new(service: WorkGraphService) -> Self {
        Self {
            service,
            superseded: SupersededRuns::default(),
        }
    }

    /// The verdict for `worker`. Never fails: a WorkGraph fault is
    /// [`ResumeCheck::Unavailable`]. A run once superseded stays superseded
    /// on every later attempt of its resume.
    pub(crate) async fn check_worker(&self, worker: ResumingWorker<'_>) -> ResumeCheck {
        let run = SupersededRuns::key(worker);
        if let Some(reason) = self.superseded.recall(&run) {
            return ResumeCheck::Superseded(reason);
        }
        let check = match self.decide(worker, &run).await {
            Ok(check) => check,
            Err(error) => {
                ResumeCheck::Unavailable(format!("the WorkGraph could not be read: {error}"))
            }
        };
        if let ResumeCheck::Superseded(reason) = &check {
            self.superseded.remember(&run, reason);
        }
        check
    }

    async fn decide(
        &self,
        worker: ResumingWorker<'_>,
        run: &str,
    ) -> Result<ResumeCheck, WorkGraphError> {
        // The run executes under attention the gate cannot read, so it can
        // neither name the run's item nor treat the run as one without
        // attention: it cannot decide.
        if let Some(reason) = worker.attention_projection_unreadable {
            return Ok(ResumeCheck::Unavailable(format!(
                "the run's WorkGraph attention projection does not decode: {reason}"
            )));
        }
        let keys = WorkerKeys::of(worker);
        match worker.attention_projection {
            Some(projection) => self.decide_projected(&keys, projection, run).await,
            None => self.decide_by_claims(&keys, run).await,
        }
    }

    /// The run was dispatched for one item: that item and its attention
    /// binding decide.
    async fn decide_projected(
        &self,
        keys: &WorkerKeys<'_>,
        projection: &AttentionContextProjection,
        run: &str,
    ) -> Result<ResumeCheck, WorkGraphError> {
        let work_ref = &projection.work_ref;
        // A member resolves its attention only in its mob's realm, so a
        // projection from elsewhere is not this worker's work any more. The
        // condition never changes, so it supersedes at once: an unavailable
        // answer would only make Meerkat retry the same refusal.
        if work_ref.realm_id != self.service.default_realm_id()
            || work_ref.namespace != *self.service.default_namespace()
        {
            return Ok(ResumeCheck::Superseded(format!(
                "the run's work item {} lives in realm '{}' namespace '{}', which this gate \
                 cannot read (it reads realm '{}' namespace '{}')",
                work_ref.item_id,
                work_ref.realm_id,
                work_ref.namespace,
                self.service.default_realm_id(),
                self.service.default_namespace(),
            )));
        }
        let item = match self
            .service
            .get(
                Some(work_ref.realm_id.clone()),
                Some(work_ref.namespace.clone()),
                work_ref.item_id.clone(),
            )
            .await
        {
            Ok(item) => item,
            Err(WorkGraphError::NotFound { .. }) => {
                return Ok(ResumeCheck::Superseded(format!(
                    "its work item {} no longer exists",
                    work_ref.item_id
                )));
            }
            Err(error) => return Err(error),
        };
        if let Some(check) = self.decide_item(keys, &item, run).await? {
            return Ok(check);
        }
        let binding_id = &projection.binding_id;
        let binding = match self
            .service
            .attention_binding(AttentionBindingRequest {
                binding_id: binding_id.clone(),
                realm_id: Some(work_ref.realm_id.clone()),
                namespace: Some(work_ref.namespace.clone()),
            })
            .await
        {
            Ok(result) => result.attention,
            // Terminal bindings are pruned: a missing one has ended.
            Err(WorkGraphError::AttentionNotFound { .. }) => {
                return Ok(ResumeCheck::Superseded(format!(
                    "its attention binding {binding_id} no longer exists"
                )));
            }
            Err(error) => return Err(error),
        };
        if binding.status.is_terminal() {
            return Ok(ResumeCheck::Superseded(format!(
                "its attention binding {binding_id} is {}",
                binding.status.status_key()
            )));
        }
        if !keys.is_target(&binding.target) {
            return Ok(ResumeCheck::Superseded(format!(
                "its attention binding {binding_id} now targets {}",
                binding.target.target_key()
            )));
        }
        Ok(ResumeCheck::Continue)
    }

    /// The run named no item: every claim the worker holds decides. The scan
    /// pages through every open item with the list cursor.
    async fn decide_by_claims(
        &self,
        keys: &WorkerKeys<'_>,
        run: &str,
    ) -> Result<ResumeCheck, WorkGraphError> {
        if keys.member.is_none() && keys.session.is_none() {
            return Ok(ResumeCheck::Continue);
        }
        // The worker's claimed items by id. A walk returns an item twice only
        // when a write moved it back past the cursor; the newer revision
        // decides.
        let mut held = BTreeMap::<WorkItemId, WorkItem>::new();
        let mut cursor = None;
        loop {
            let page = self
                .service
                .list_page(WorkItemFilter {
                    limit: Some(CLAIM_SCAN_PAGE),
                    cursor,
                    ..WorkItemFilter::default()
                })
                .await?;
            // A page that took no item cannot move the walk on.
            let advanced = !page.items.is_empty();
            for item in page.items {
                let claimed = item
                    .claim
                    .as_ref()
                    .is_some_and(|claim| keys.names(&claim.owner.key));
                if claimed
                    && held
                        .get(&item.id)
                        .is_none_or(|seen| seen.revision < item.revision)
                {
                    held.insert(item.id.clone(), item);
                }
            }
            match page.next_cursor {
                Some(next) if advanced => cursor = Some(next),
                _ => break,
            }
        }
        if held.is_empty() {
            return Ok(ResumeCheck::Continue);
        }
        let now = self.service.store().get_store_time_utc().await?;
        // Observe every lapsed claim, not only the first: the superseded run
        // gives all of them up.
        let mut verdict = ResumeCheck::Continue;
        for item in held.values() {
            let Some(claim) = item.claim.as_ref() else {
                continue;
            };
            if let Some(lapsed) = self.lapsed_claim(item, claim, now, run).await
                && verdict == ResumeCheck::Continue
            {
                verdict = lapsed;
            }
        }
        Ok(verdict)
    }

    /// The item-level verdict for the run's item: terminal, claimed by
    /// another owner, or the worker's own lapsed claim. `None` when none of
    /// those holds.
    async fn decide_item(
        &self,
        keys: &WorkerKeys<'_>,
        item: &WorkItem,
        run: &str,
    ) -> Result<Option<ResumeCheck>, WorkGraphError> {
        if let Some(terminal) = terminal_status_name(item.status) {
            return Ok(Some(ResumeCheck::Superseded(format!(
                "its work item {} is {terminal}",
                item.id
            ))));
        }
        let Some(claim) = item.claim.as_ref() else {
            return Ok(None);
        };
        if !keys.names(&claim.owner.key) {
            return Ok(Some(ResumeCheck::Superseded(format!(
                "its work item {} is claimed by {}",
                item.id,
                claim.owner.key.canonical()
            ))));
        }
        let now = self.service.store().get_store_time_utc().await?;
        Ok(self.lapsed_claim(item, claim, now, run).await)
    }

    /// Superseded when the worker's claim on `item` has lapsed at `now` (the
    /// store's clock, which stamped the claim). The verdict is remembered for
    /// `run` before anything is written. The expiry is then committed, so the
    /// item can be claimed again; a commit that fails (the item moved
    /// meanwhile) still supersedes, because the lease has lapsed all the
    /// same. A claim without a lease never lapses.
    async fn lapsed_claim(
        &self,
        item: &WorkItem,
        claim: &WorkClaim,
        now: DateTime<Utc>,
        run: &str,
    ) -> Option<ResumeCheck> {
        let lease_expires_at = claim.lease_expires_at?;
        if lease_expires_at > now {
            return None;
        }
        let reason = format!(
            "its claim on work item {} lapsed at {}",
            item.id,
            lease_expires_at.to_rfc3339()
        );
        self.superseded.remember(run, &reason);
        if let Err(error) = self
            .service
            .observe_lease_expiry(ObserveLeaseExpiryRequest {
                id: item.id.clone(),
                realm_id: Some(item.realm_id.clone()),
                namespace: Some(item.namespace.clone()),
                expected_revision: item.revision,
                observed_at: now,
            })
            .await
        {
            tracing::warn!(
                item = %item.id,
                %error,
                "durable resume gate: the lapsed claim's expiry was not committed; the run is \
                 superseded all the same"
            );
        }
        Some(ResumeCheck::Superseded(reason))
    }
}

/// The wire name of a terminal work status; `None` for a live one.
fn terminal_status_name(status: WorkStatus) -> Option<&'static str> {
    match status {
        WorkStatus::Completed => Some("completed"),
        WorkStatus::Cancelled => Some("cancelled"),
        WorkStatus::Failed => Some("failed"),
        WorkStatus::Open | WorkStatus::InProgress | WorkStatus::Blocked => None,
    }
}

#[async_trait::async_trait]
impl meerkat_mob::DurableResumeGate for WorkGraphResumeGate {
    async fn check(
        &self,
        request: &meerkat_mob::DurableResumeRequest,
    ) -> meerkat_mob::DurableResumeVerdict {
        let check = self
            .check_worker(ResumingWorker {
                mob_id: &request.mob_id,
                agent_identity: &request.agent_identity,
                session_id: &request.session_id,
                run_id: &request.run_id,
                attention_projection: request.attention_projection.as_ref(),
                attention_projection_unreadable: request.attention_projection_unreadable.as_deref(),
            })
            .await;
        match &check {
            ResumeCheck::Continue => tracing::debug!(
                mob_id = %request.mob_id,
                agent_identity = %request.agent_identity,
                session_id = %request.session_id,
                run_id = %request.run_id,
                "durable resume gate: the worker still holds its work; the run resumes"
            ),
            ResumeCheck::Superseded(reason) => tracing::info!(
                mob_id = %request.mob_id,
                agent_identity = %request.agent_identity,
                session_id = %request.session_id,
                run_id = %request.run_id,
                %reason,
                "durable resume gate: the run's work moved; the run is superseded"
            ),
            ResumeCheck::Unavailable(reason) => tracing::warn!(
                mob_id = %request.mob_id,
                agent_identity = %request.agent_identity,
                session_id = %request.session_id,
                run_id = %request.run_id,
                %reason,
                "durable resume gate: the gate cannot decide; Meerkat asks again"
            ),
        }
        check.into_verdict()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use meerkat::{
        AttentionListRequest, AttentionProjectionRequest, ClaimWorkItemRequest,
        CloseWorkItemRequest, DisabledWorkGraphStore, GoalAttentionTarget, GoalCreateRequest,
        MemoryWorkGraphStore, ReleaseWorkItemRequest, WorkAttentionBinding, WorkEdge,
        WorkGraphEvent, WorkGraphEventFilter, WorkGraphStore, WorkGraphStoreKind, WorkItemId,
        WorkNamespace, WorkOwner,
    };

    use super::*;

    const MOB: &str = "durable-mob";
    const WORKER: &str = "worker-1";
    const OTHER: &str = "worker-2";

    fn mob_realm() -> String {
        meerkat_core::mob_realm_id(MOB)
            .expect("mob realm")
            .as_str()
            .to_string()
    }

    fn service() -> WorkGraphService {
        WorkGraphService::with_scope(
            Arc::new(MemoryWorkGraphStore::new()),
            mob_realm(),
            WorkNamespace::default(),
        )
    }

    fn member_key(identity: &str) -> WorkOwnerKey {
        WorkOwnerKey::mob_agent(MOB, identity).expect("member owner key")
    }

    /// A second mob the launch of [`MOB`] hosts.
    const HOSTED: &str = "hosted-mob";

    fn hosted_mob() -> meerkat_mob::MobId {
        meerkat_mob::MobId::from(HOSTED)
    }

    fn hosted_member_key(identity: &str) -> WorkOwnerKey {
        WorkOwnerKey::mob_agent(HOSTED, identity).expect("member owner key")
    }

    /// The launch's WorkGraph `host` as the members of the hosted mob see
    /// it: the realm their tools, their attention and their claims live in.
    fn hosted_view(host: &WorkGraphService) -> WorkGraphService {
        meerkat_mob::mob_scoped_workgraph_service(host, &hosted_mob())
            .expect("the hosted mob has a realm")
            .expect("the backend is not disabled")
    }

    fn authoritative() -> crate::mob_composition_manifest::CompositionAuthority {
        crate::mob_composition_manifest::CompositionAuthority::Authoritative
    }

    struct Worker {
        mob_id: meerkat_mob::MobId,
        identity: meerkat_mob::AgentIdentity,
        session_id: SessionId,
        run_id: RunId,
    }

    impl Worker {
        fn named(identity: &str) -> Self {
            Self::of(MOB, identity)
        }

        /// The worker `identity` of the mob `mob`.
        fn of(mob: &str, identity: &str) -> Self {
            Self {
                mob_id: meerkat_mob::MobId::from(mob),
                identity: meerkat_mob::AgentIdentity::from(identity),
                session_id: SessionId::new(),
                run_id: RunId::new(),
            }
        }

        /// The same worker, resuming another run of its session.
        fn next_run(&self) -> Self {
            Self {
                mob_id: self.mob_id.clone(),
                identity: self.identity.clone(),
                session_id: self.session_id.clone(),
                run_id: RunId::new(),
            }
        }

        fn resuming<'a>(
            &'a self,
            projection: Option<&'a AttentionContextProjection>,
        ) -> ResumingWorker<'a> {
            ResumingWorker {
                mob_id: &self.mob_id,
                agent_identity: &self.identity,
                session_id: &self.session_id,
                run_id: &self.run_id,
                attention_projection: projection,
                attention_projection_unreadable: None,
            }
        }
    }

    /// A goal whose attention binding targets `target`, and the projection
    /// a run dispatched for it carries.
    async fn goal_for(
        service: &WorkGraphService,
        target: GoalAttentionTarget,
    ) -> (WorkItem, AttentionContextProjection) {
        let request: GoalCreateRequest = serde_json::from_value(serde_json::json!({
            "title": "durable work",
            "target": target,
        }))
        .expect("goal request");
        let created = service.create_goal(request).await.expect("create goal");
        let projection = service
            .attention_projection(AttentionProjectionRequest {
                binding_id: created.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
            })
            .await
            .expect("attention projection")
            .projection;
        (created.item, projection)
    }

    async fn plain_item(service: &WorkGraphService) -> WorkItem {
        service
            .create(
                serde_json::from_value(serde_json::json!({ "title": "plain work" }))
                    .expect("create request"),
            )
            .await
            .expect("create item")
    }

    async fn claim(
        service: &WorkGraphService,
        item: &WorkItem,
        owner: WorkOwnerKey,
        lease_expires_at: Option<DateTime<Utc>>,
    ) -> WorkItem {
        service
            .claim(ClaimWorkItemRequest {
                id: item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                owner: WorkOwner::new(owner),
                lease_seconds: None,
                lease_expires_at,
            })
            .await
            .expect("claim")
    }

    fn in_an_hour() -> Option<DateTime<Utc>> {
        Some(Utc::now() + chrono::Duration::hours(1))
    }

    /// A lease that lapses before [`let_the_lease_lapse`] returns.
    fn lapsing_soon() -> Option<DateTime<Utc>> {
        Some(Utc::now() + chrono::Duration::milliseconds(300))
    }

    async fn let_the_lease_lapse() {
        tokio::time::sleep(Duration::from_millis(700)).await;
    }

    fn superseded_reason(check: &ResumeCheck) -> &str {
        match check {
            ResumeCheck::Superseded(reason) => reason,
            other => panic!("expected a superseded verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_projected_item_the_worker_still_holds_resumes() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&service, &item, member_key(WORKER), in_an_hour()).await;

        let gate = WorkGraphResumeGate::new(service);
        assert_eq!(
            gate.check_worker(worker.resuming(Some(&projection))).await,
            ResumeCheck::Continue
        );
    }

    #[tokio::test]
    async fn a_claim_under_the_workers_session_key_is_its_own() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Session {
                session_id: worker.session_id.clone(),
            },
        )
        .await;
        let session_key = WorkOwnerKey::session(worker.session_id.to_string()).expect("key");
        claim(&service, &item, session_key, in_an_hour()).await;

        let gate = WorkGraphResumeGate::new(service);
        assert_eq!(
            gate.check_worker(worker.resuming(Some(&projection))).await,
            ResumeCheck::Continue
        );
    }

    #[tokio::test]
    async fn a_projected_item_another_owner_claimed_supersedes_the_run() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&service, &item, member_key(OTHER), in_an_hour()).await;

        let gate = WorkGraphResumeGate::new(service);
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        let reason = superseded_reason(&check);
        assert!(
            reason.contains(&member_key(OTHER).canonical()),
            "the reason names the new owner: {reason}"
        );
    }

    #[tokio::test]
    async fn the_workers_lapsed_claim_is_observed_and_supersedes_the_run() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&service, &item, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;

        let gate = WorkGraphResumeGate::new(service.clone());
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(
            superseded_reason(&check).contains("lapsed"),
            "the reason says the claim lapsed: {check:?}"
        );
        let after = service
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.claim, None, "the gate committed the lease expiry");
        assert_eq!(
            after.status,
            WorkStatus::Open,
            "the item can be claimed again"
        );
    }

    /// A candidate launch binds no resume gate, so Meerkat asks no admission
    /// before the run-start hold parks the run: a worker whose claim lapsed,
    /// which the gate would supersede, keeps its run marked and its claim
    /// unobserved. The authoritative launch over the same WorkGraph binds the
    /// gate, which supersedes the run and commits the lease expiry as before.
    #[tokio::test]
    async fn a_candidate_launch_binds_no_resume_gate_and_the_authoritative_one_does() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&service, &item, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;

        assert!(
            launch_resume_gate(
                Some(service.clone()),
                CompositionAuthority::NonAuthoritative
            )
            .is_none(),
            "a candidate launch binds no resume gate"
        );
        let held = service
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert!(
            held.claim.is_some(),
            "nothing on a candidate observed the lapsed lease"
        );

        let gate = launch_resume_gate(Some(service.clone()), CompositionAuthority::Authoritative)
            .expect("an authoritative launch with a WorkGraph binds the gate");
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(superseded_reason(&check).contains("lapsed"), "{check:?}");
        let after = service
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.claim, None, "the gate committed the lease expiry");
        assert!(
            launch_resume_gate(None, CompositionAuthority::Authoritative).is_none(),
            "without a WorkGraph there is nothing to check"
        );
    }

    /// A launch builds every mob it hosts with a gate of that mob. A member
    /// of a hosted mob works in that mob's realm: its interrupted run, which
    /// was dispatched for an item it still holds there, continues.
    #[tokio::test]
    async fn a_hosted_mobs_interrupted_worker_under_attention_continues() {
        let host = service();
        let hosted = hosted_view(&host);
        let worker = Worker::of(HOSTED, WORKER);
        let (item, projection) = goal_for(
            &hosted,
            GoalAttentionTarget::Owner {
                owner_key: hosted_member_key(WORKER),
            },
        )
        .await;
        claim(&hosted, &item, hosted_member_key(WORKER), in_an_hour()).await;

        let gate = mob_resume_gate(Some(&host), &hosted_mob(), authoritative())
            .expect("an authoritative launch with a WorkGraph binds a hosted mob's gate");
        assert_eq!(
            gate.check_worker(worker.resuming(Some(&projection))).await,
            ResumeCheck::Continue
        );
    }

    /// A hosted mob's gate reads that mob's realm: a run whose item another
    /// member of the hosted mob claimed is superseded, and the reason names
    /// that owner.
    #[tokio::test]
    async fn a_hosted_mobs_worker_whose_item_another_owner_took_is_superseded_for_that_reason() {
        let host = service();
        let hosted = hosted_view(&host);
        let worker = Worker::of(HOSTED, WORKER);
        let (item, projection) = goal_for(
            &hosted,
            GoalAttentionTarget::Owner {
                owner_key: hosted_member_key(WORKER),
            },
        )
        .await;
        claim(&hosted, &item, hosted_member_key(OTHER), in_an_hour()).await;

        let gate = mob_resume_gate(Some(&host), &hosted_mob(), authoritative())
            .expect("an authoritative launch with a WorkGraph binds a hosted mob's gate");
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        let reason = superseded_reason(&check);
        assert!(
            reason.contains(&hosted_member_key(OTHER).canonical()),
            "the reason names the new owner: {reason}"
        );
    }

    /// The launch mob's gate reads the launch mob's realm: its worker's run
    /// continues on an item it holds there, and a run that names an item of
    /// the hosted mob's realm is not that worker's work.
    #[tokio::test]
    async fn the_launch_mobs_resume_gate_reads_the_launch_mobs_realm() {
        let host = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &host,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&host, &item, member_key(WORKER), in_an_hour()).await;
        let (_hosted_item, hosted_projection) = goal_for(
            &hosted_view(&host),
            GoalAttentionTarget::Owner {
                owner_key: hosted_member_key(WORKER),
            },
        )
        .await;

        let gate = mob_resume_gate(Some(&host), &meerkat_mob::MobId::from(MOB), authoritative())
            .expect("an authoritative launch with a WorkGraph binds the gate");
        assert_eq!(
            gate.check_worker(worker.resuming(Some(&projection))).await,
            ResumeCheck::Continue
        );
        let foreign = gate
            .check_worker(worker.next_run().resuming(Some(&hosted_projection)))
            .await;
        assert!(
            superseded_reason(&foreign)
                .contains(&hosted_view(&host).default_realm_id().to_string()),
            "{foreign:?}"
        );
    }

    /// A hosted mob has no gate where the launch mob has none: on a
    /// candidate launch, and on a launch without a WorkGraph.
    #[tokio::test]
    async fn a_hosted_mob_has_no_resume_gate_on_a_candidate_launch_or_without_a_service() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let host = service();
        assert!(
            mob_resume_gate(
                Some(&host),
                &hosted_mob(),
                CompositionAuthority::NonAuthoritative
            )
            .is_none(),
            "a candidate launch binds no resume gate"
        );
        assert!(
            mob_resume_gate(None, &hosted_mob(), authoritative()).is_none(),
            "without a WorkGraph there is nothing to check"
        );
    }

    /// The service of a launch whose storage backend has no WorkGraph.
    fn disabled_service() -> WorkGraphService {
        WorkGraphService::with_scope(
            Arc::new(DisabledWorkGraphStore),
            mob_realm(),
            WorkNamespace::default(),
        )
    }

    /// A mob the launch hosts has no gate over a backend without a WorkGraph,
    /// as the launch and its launch mob have none.
    #[test]
    fn a_hosted_mob_of_a_launch_with_a_disabled_workgraph_backend_binds_no_resume_gate() {
        assert!(
            mob_resume_gate(Some(&disabled_service()), &hosted_mob(), authoritative()).is_none(),
            "a hosted mob has no gate over a backend without a WorkGraph"
        );
    }

    #[tokio::test]
    async fn a_projected_item_that_ended_or_is_gone_supersedes_the_run() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        service
            .close(CloseWorkItemRequest {
                id: item.id.clone(),
                realm_id: None,
                namespace: None,
                expected_revision: item.revision,
                status: WorkStatus::Cancelled,
            })
            .await
            .expect("cancel the item");

        let gate = WorkGraphResumeGate::new(service);
        let ended = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(
            superseded_reason(&ended).contains("cancelled"),
            "the reason names the terminal status: {ended:?}"
        );

        // Another run of the worker: the run checked above is superseded,
        // and every later attempt of its resume answers that same verdict.
        let mut gone = projection.clone();
        gone.work_ref.item_id = WorkItemId::generated();
        let next = worker.next_run();
        let missing = gate.check_worker(next.resuming(Some(&gone))).await;
        assert!(
            superseded_reason(&missing).contains("no longer exists"),
            "{missing:?}"
        );
    }

    #[tokio::test]
    async fn a_binding_that_now_targets_another_owner_supersedes_the_run() {
        let service = service();
        let (_item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(OTHER),
            },
        )
        .await;
        let gate = WorkGraphResumeGate::new(service);

        // Control: the member the binding targets resumes on the unclaimed
        // item.
        let target = Worker::named(OTHER);
        assert_eq!(
            gate.check_worker(target.resuming(Some(&projection))).await,
            ResumeCheck::Continue
        );

        let worker = Worker::named(WORKER);
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(
            superseded_reason(&check).contains("now targets"),
            "{check:?}"
        );
    }

    #[tokio::test]
    async fn a_projection_outside_the_gates_realm_supersedes_the_run() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (_item, mut projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        projection.work_ref.realm_id = "another-realm".to_string();

        let gate = WorkGraphResumeGate::new(service);
        let check = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(
            superseded_reason(&check).contains("another-realm"),
            "a deterministic refusal settles at once: {check:?}"
        );
    }

    #[tokio::test]
    async fn without_a_projection_a_worker_holding_no_claim_resumes() {
        let service = service();
        let worker = Worker::named(WORKER);
        let item = plain_item(&service).await;
        // Someone else's claim is not this worker's business without a
        // projection.
        claim(&service, &item, member_key(OTHER), in_an_hour()).await;

        let gate = WorkGraphResumeGate::new(service);
        assert_eq!(
            gate.check_worker(worker.resuming(None)).await,
            ResumeCheck::Continue
        );
    }

    #[tokio::test]
    async fn without_a_projection_every_lapsed_claim_is_observed_and_supersedes() {
        let service = service();
        let worker = Worker::named(WORKER);
        let live = plain_item(&service).await;
        claim(&service, &live, member_key(WORKER), in_an_hour()).await;

        let gate = WorkGraphResumeGate::new(service.clone());
        assert_eq!(
            gate.check_worker(worker.resuming(None)).await,
            ResumeCheck::Continue,
            "a live claim keeps the run"
        );

        let first = plain_item(&service).await;
        let second = plain_item(&service).await;
        claim(&service, &first, member_key(WORKER), lapsing_soon()).await;
        claim(&service, &second, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;

        let check = gate.check_worker(worker.resuming(None)).await;
        assert!(superseded_reason(&check).contains("lapsed"), "{check:?}");
        for lapsed in [&first, &second] {
            let after = service
                .get(None, None, lapsed.id.clone())
                .await
                .expect("read the item");
            assert_eq!(after.claim, None, "every lapsed claim is observed");
        }
        let kept = service
            .get(None, None, live.id.clone())
            .await
            .expect("read the live item");
        assert!(kept.claim.is_some(), "a live claim is left alone");
    }

    /// A lapsed claim on an item past the first page of the claim scan still
    /// supersedes the run: the scan reads every page.
    #[tokio::test]
    async fn resume_gate_checks_claims_past_the_first_page() {
        let service = service();
        let worker = Worker::named(WORKER);
        let lapsed = plain_item(&service).await;
        claim(&service, &lapsed, member_key(WORKER), lapsing_soon()).await;
        // A full page of newer open items (the WorkGraph maximum, 1000) puts
        // the claimed item on the second page.
        for _ in 0..1000 {
            plain_item(&service).await;
        }
        let_the_lease_lapse().await;

        let gate = WorkGraphResumeGate::new(service.clone());
        let check = gate.check_worker(worker.resuming(None)).await;
        assert!(superseded_reason(&check).contains("lapsed"), "{check:?}");
        let after = service
            .get(None, None, lapsed.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.claim, None, "the lapsed claim is observed");
    }

    #[tokio::test]
    async fn a_workgraph_read_fault_is_unavailable() {
        let disabled = WorkGraphService::with_scope(
            Arc::new(DisabledWorkGraphStore),
            mob_realm(),
            WorkNamespace::default(),
        );
        let worker = Worker::named(WORKER);
        let gate = WorkGraphResumeGate::new(disabled);

        let check = gate.check_worker(worker.resuming(None)).await;
        assert!(matches!(check, ResumeCheck::Unavailable(_)), "{check:?}");

        // A real projection, read back through a store that refuses every
        // read.
        let (_item, projection) = goal_for(
            &service(),
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        let projected = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert!(
            matches!(projected, ResumeCheck::Unavailable(_)),
            "{projected:?}"
        );
    }

    /// A backend without a WorkGraph holds no item and no claim, and every
    /// read of it fails. A launch over it binds no gate, as a launch without
    /// a WorkGraph service binds none, and a durable worker's run resumes
    /// unchecked.
    #[test]
    fn a_launch_with_a_disabled_workgraph_backend_binds_no_resume_gate() {
        use crate::mob_composition_manifest::CompositionAuthority;

        let disabled = WorkGraphService::with_scope(
            Arc::new(DisabledWorkGraphStore),
            mob_realm(),
            WorkNamespace::default(),
        );
        assert!(
            launch_resume_gate(Some(disabled.clone()), CompositionAuthority::Authoritative)
                .is_none(),
            "a backend without a WorkGraph leaves nothing to check"
        );
        assert!(
            mob_resume_gate(
                Some(&disabled),
                &meerkat_mob::MobId::from(MOB),
                CompositionAuthority::Authoritative
            )
            .is_none(),
            "the launch mob has no gate over a backend without a WorkGraph"
        );
    }

    /// Meerkat drops an attempt that outlasts its timeout and asks again. An
    /// attempt dropped after it committed the lapsed lease's expiry leaves the
    /// item unclaimed and never returns its answer, and the next attempt of
    /// the same resume must still answer superseded; another run of the
    /// worker is decided afresh.
    #[tokio::test]
    async fn a_superseded_run_answers_the_same_on_every_attempt() {
        let service = service();
        let worker = Worker::named(WORKER);
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        let claimed = claim(&service, &item, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;

        // Attempt 1, as Meerkat drops it at its timeout: the lapse is decided
        // and the expiry committed, and the answer never reaches Meerkat, so
        // nothing but the gate's own record before that write remains.
        let gate = WorkGraphResumeGate::new(service.clone());
        let run = SupersededRuns::key(worker.resuming(Some(&projection)));
        let now = service
            .store()
            .get_store_time_utc()
            .await
            .expect("store time");
        let held = claimed.claim.as_ref().expect("the worker's claim");
        let dropped = gate
            .lapsed_claim(&claimed, held, now, &run)
            .await
            .expect("the lease lapsed");
        let after = service
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.claim, None, "the dropped attempt released the claim");

        let again = gate.check_worker(worker.resuming(Some(&projection))).await;
        assert_eq!(again, dropped, "the next attempt answers the same");
        assert!(superseded_reason(&again).contains("lapsed"), "{again:?}");

        let next = worker.next_run();
        assert_eq!(
            gate.check_worker(next.resuming(Some(&projection))).await,
            ResumeCheck::Continue,
            "another run of the worker is decided afresh"
        );
    }

    /// The memory store behind a list of the open items that can be made
    /// slow: from a chosen `list_items` call on, each list waits a set time
    /// before it reads. Nothing fails. It counts the writes it passes on, so
    /// a row can assert that the gate wrote nothing.
    struct SlowListStore {
        inner: Arc<MemoryWorkGraphStore>,
        /// The first slow `list_items` call, counted from 1; 0 for none.
        slow_from_call: AtomicUsize,
        slow_for: Mutex<Duration>,
        list_calls: AtomicUsize,
        writes: AtomicUsize,
        /// Signalled each time a slow list starts to wait.
        stalled: tokio::sync::Notify,
    }

    impl SlowListStore {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: Arc::new(MemoryWorkGraphStore::new()),
                slow_from_call: AtomicUsize::new(0),
                slow_for: Mutex::new(Duration::ZERO),
                list_calls: AtomicUsize::new(0),
                writes: AtomicUsize::new(0),
                stalled: tokio::sync::Notify::new(),
            })
        }

        /// The service a gate reads this store through.
        fn service(self: &Arc<Self>) -> WorkGraphService {
            let store: Arc<dyn WorkGraphStore> = self.clone();
            WorkGraphService::with_scope(store, mob_realm(), WorkNamespace::default())
        }

        /// The same items through the memory store itself: a row's setup,
        /// and its reads of what the gate left.
        fn direct(&self) -> WorkGraphService {
            let store: Arc<dyn WorkGraphStore> = self.inner.clone();
            WorkGraphService::with_scope(store, mob_realm(), WorkNamespace::default())
        }

        /// From the `call`th list made after this on, each list of the items
        /// waits `wait` before it reads.
        fn slow_from(&self, call: usize, wait: Duration) {
            self.list_calls.store(0, Ordering::SeqCst);
            *self.slow_for.lock().unwrap() = wait;
            self.slow_from_call.store(call, Ordering::SeqCst);
        }

        fn fast_again(&self) {
            self.slow_from_call.store(0, Ordering::SeqCst);
        }

        /// The lists of the items made since the last [`Self::slow_from`].
        fn lists(&self) -> usize {
            self.list_calls.load(Ordering::SeqCst)
        }

        /// The writes that went through this store, the gate's only way in.
        fn writes(&self) -> usize {
            self.writes.load(Ordering::SeqCst)
        }

        /// The newest event of the gate's namespace: every write appends one.
        async fn ledger_mark(&self) -> Option<i64> {
            self.inner
                .latest_event_seq(WorkGraphEventFilter {
                    realm_id: Some(mob_realm()),
                    namespace: Some(WorkNamespace::default()),
                    all_namespaces: false,
                    after_seq: None,
                    limit: None,
                })
                .await
                .expect("read the ledger")
        }
    }

    #[async_trait::async_trait]
    impl WorkGraphStore for SlowListStore {
        fn kind(&self) -> WorkGraphStoreKind {
            self.inner.kind()
        }

        async fn get_store_time_utc(&self) -> Result<DateTime<Utc>, WorkGraphError> {
            self.inner.get_store_time_utc().await
        }

        async fn insert_item(
            &self,
            item: WorkItem,
            event: WorkGraphEvent,
        ) -> Result<WorkItem, WorkGraphError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner.insert_item(item, event).await
        }

        async fn update_item_cas(
            &self,
            item: WorkItem,
            expected_previous_revision: u64,
            event: WorkGraphEvent,
        ) -> Result<WorkItem, WorkGraphError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner
                .update_item_cas(item, expected_previous_revision, event)
                .await
        }

        async fn update_item_and_attention_cas(
            &self,
            item: WorkItem,
            expected_previous_revision: u64,
            item_event: WorkGraphEvent,
            attention_updates: Vec<(WorkAttentionBinding, u64, WorkGraphEvent)>,
        ) -> Result<WorkItem, WorkGraphError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner
                .update_item_and_attention_cas(
                    item,
                    expected_previous_revision,
                    item_event,
                    attention_updates,
                )
                .await
        }

        async fn get_item(
            &self,
            realm_id: &str,
            namespace: &WorkNamespace,
            id: &WorkItemId,
        ) -> Result<Option<WorkItem>, WorkGraphError> {
            self.inner.get_item(realm_id, namespace, id).await
        }

        async fn list_items(
            &self,
            filter: WorkItemFilter,
        ) -> Result<Vec<WorkItem>, WorkGraphError> {
            let call = self.list_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let slow_from = self.slow_from_call.load(Ordering::SeqCst);
            if slow_from != 0 && call >= slow_from {
                let wait = *self.slow_for.lock().unwrap();
                self.stalled.notify_one();
                tokio::time::sleep(wait).await;
            }
            self.inner.list_items(filter).await
        }

        async fn list_attention_matching_bounded(
            &self,
            filter: AttentionListRequest,
            observed_at: DateTime<Utc>,
            limit: usize,
        ) -> Result<Vec<WorkAttentionBinding>, WorkGraphError> {
            self.inner
                .list_attention_matching_bounded(filter, observed_at, limit)
                .await
        }

        async fn insert_edge(
            &self,
            edge: WorkEdge,
            event: WorkGraphEvent,
        ) -> Result<WorkEdge, WorkGraphError> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.inner.insert_edge(edge, event).await
        }

        async fn list_edges(
            &self,
            realm_id: &str,
            namespace: &WorkNamespace,
        ) -> Result<Vec<WorkEdge>, WorkGraphError> {
            self.inner.list_edges(realm_id, namespace).await
        }

        async fn list_events(
            &self,
            filter: WorkGraphEventFilter,
        ) -> Result<Vec<WorkGraphEvent>, WorkGraphError> {
            self.inner.list_events(filter).await
        }

        async fn latest_event_seq(
            &self,
            filter: WorkGraphEventFilter,
        ) -> Result<Option<i64>, WorkGraphError> {
            self.inner.latest_event_seq(filter).await
        }
    }

    /// Meerkat's request for a resume of [`WORKER`] with no attention
    /// projection.
    fn plain_resume_request() -> meerkat_mob::DurableResumeRequest {
        meerkat_mob::DurableResumeRequest::new(
            meerkat_mob::MobId::from(MOB),
            meerkat_mob::AgentIdentity::from(WORKER),
            meerkat_mob::ProfileName::from("worker"),
            SessionId::new(),
            RunId::new(),
            vec![meerkat_core::lifecycle::InputId::new()],
        )
    }

    /// One list of the open items that takes longer than the gate allows its
    /// claim scan. Asked the way Meerkat asks (one attempt, cut at
    /// `RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT`), the gate answers before that
    /// cut: unavailable, with a reason that names the list, the pages it had
    /// read and how long it waited. It wrote nothing, and the same run asked
    /// again once the list answers is decided as ever.
    #[tokio::test(start_paused = true)]
    async fn a_claim_scan_slower_than_its_budget_answers_unavailable_and_says_why() {
        use meerkat_mob::DurableResumeGate as _;

        let store = SlowListStore::new();
        let direct = store.direct();
        let item = plain_item(&direct).await;
        let claimed = claim(&direct, &item, member_key(WORKER), in_an_hour()).await;
        let ledger = store.ledger_mark().await;
        let gate = WorkGraphResumeGate::new(store.service());
        let request = plain_resume_request();

        store.slow_from(1, Duration::from_secs(45));
        let asked = tokio::time::Instant::now();
        let answer = tokio::time::timeout(
            meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT,
            gate.check(&request),
        )
        .await;
        let waited = asked.elapsed();
        let reason = match answer {
            Ok(meerkat_mob::DurableResumeVerdict::Unavailable { reason }) => reason,
            other => panic!(
                "the gate answers unavailable itself, before Meerkat cuts the attempt: {other:?}"
            ),
        };
        for part in [
            "open work item list".to_string(),
            format!("realm '{}'", mob_realm()),
            format!("namespace '{}'", WorkNamespace::default()),
            "0 page(s)".to_string(),
            format!("read in {} ms", waited.as_millis()),
        ] {
            assert!(
                reason.contains(&part),
                "the reason holds \"{part}\": {reason}"
            );
        }
        assert_eq!(store.writes(), 0, "a cut scan wrote nothing");
        assert_eq!(
            store.ledger_mark().await,
            ledger,
            "the ledger gained no event"
        );
        let after = direct
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.revision, claimed.revision, "the item is as it was");
        assert_eq!(after.claim, claimed.claim, "the claim is as it was");

        store.fast_again();
        let again = gate.check(&request).await;
        assert!(
            matches!(again, meerkat_mob::DurableResumeVerdict::Continue),
            "the same run, asked again over a list that answers, continues: {again:?}"
        );
    }

    /// The control of the rows around it: a list that answers in time, at
    /// once or slowly, is read to its end and decides as before. A live claim
    /// keeps the run; a lapsed one supersedes it, and its expiry is written
    /// through the same store (so the rows' write count is a live reading).
    #[tokio::test]
    async fn a_claim_scan_that_ends_in_time_decides_as_before() {
        let store = SlowListStore::new();
        let direct = store.direct();
        let live = plain_item(&direct).await;
        claim(&direct, &live, member_key(WORKER), in_an_hour()).await;
        let gate = WorkGraphResumeGate::new(store.service());
        let worker = Worker::named(WORKER);

        assert_eq!(
            gate.check_worker(worker.resuming(None)).await,
            ResumeCheck::Continue,
            "a list that answers at once"
        );

        // A slow list that still ends inside an attempt: a quarter of
        // Meerkat's attempt timeout.
        let slow = meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT / 4;
        tokio::time::pause();
        store.slow_from(1, slow);
        let asked = tokio::time::Instant::now();
        assert_eq!(
            gate.check_worker(worker.resuming(None)).await,
            ResumeCheck::Continue,
            "a slow list that ends in time"
        );
        assert!(asked.elapsed() >= slow, "the list was waited for");
        tokio::time::resume();
        store.fast_again();
        assert_eq!(store.writes(), 0, "a run that continues writes nothing");

        let lapsing = plain_item(&direct).await;
        claim(&direct, &lapsing, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;
        let check = gate.check_worker(worker.resuming(None)).await;
        assert!(superseded_reason(&check).contains("lapsed"), "{check:?}");
        assert_eq!(
            store.writes(),
            1,
            "the lapsed claim's expiry went through this store"
        );
    }

    /// A scan cut after its first page answers unavailable, whatever that
    /// page held. Here it held the worker's live claim, which is released
    /// while the second page is awaited: the sighting is older than the item,
    /// and a verdict drawn from it would let the run continue on work the
    /// worker no longer holds.
    #[tokio::test(start_paused = true)]
    async fn a_cut_claim_scan_never_continues_on_a_claim_it_sighted_earlier() {
        let store = SlowListStore::new();
        let direct = store.direct();
        // A full page of older open items puts one item on a second page.
        for _ in 0..CLAIM_SCAN_PAGE {
            plain_item(&direct).await;
        }
        let item = plain_item(&direct).await;
        let claimed = claim(&direct, &item, member_key(WORKER), in_an_hour()).await;
        let gate = WorkGraphResumeGate::new(store.service());
        let worker = Worker::named(WORKER);

        store.slow_from(2, Duration::from_secs(45));
        let (check, ()) = tokio::join!(gate.check_worker(worker.resuming(None)), async {
            store.stalled.notified().await;
            direct
                .release(ReleaseWorkItemRequest {
                    id: claimed.id.clone(),
                    realm_id: None,
                    namespace: None,
                    expected_revision: claimed.revision,
                })
                .await
                .expect("release the claim");
        });
        assert_eq!(store.lists(), 2, "the scan was on its second page");
        assert!(
            matches!(&check, ResumeCheck::Unavailable(reason) if reason.contains("1 page(s)")),
            "a cut scan draws no verdict from the page it read: {check:?}"
        );
        assert_eq!(store.writes(), 0, "a cut scan wrote nothing");
    }

    /// As above, with the worker's lapsed claim on the first page: a cut scan
    /// answers no superseded verdict, writes no expiry and remembers nothing
    /// for the run, so the same run asked again over a list that answers is
    /// decided then.
    #[tokio::test]
    async fn a_cut_claim_scan_never_supersedes_and_writes_nothing() {
        let store = SlowListStore::new();
        let direct = store.direct();
        for _ in 0..CLAIM_SCAN_PAGE {
            plain_item(&direct).await;
        }
        let item = plain_item(&direct).await;
        let claimed = claim(&direct, &item, member_key(WORKER), lapsing_soon()).await;
        let_the_lease_lapse().await;
        let ledger = store.ledger_mark().await;
        let gate = WorkGraphResumeGate::new(store.service());
        let worker = Worker::named(WORKER);

        tokio::time::pause();
        store.slow_from(2, Duration::from_secs(45));
        let check = gate.check_worker(worker.resuming(None)).await;
        assert_eq!(store.lists(), 2, "the scan was on its second page");
        assert!(
            matches!(&check, ResumeCheck::Unavailable(reason) if reason.contains("1 page(s)")),
            "a cut scan draws no verdict from the page it read: {check:?}"
        );
        assert_eq!(store.writes(), 0, "a cut scan wrote nothing");
        assert_eq!(
            store.ledger_mark().await,
            ledger,
            "the ledger gained no event"
        );
        let after = direct
            .get(None, None, item.id.clone())
            .await
            .expect("read the item");
        assert_eq!(after.revision, claimed.revision, "the item is as it was");
        assert_eq!(after.claim, claimed.claim, "the lapsed claim is unobserved");

        store.fast_again();
        let again = gate.check_worker(worker.resuming(None)).await;
        assert!(
            superseded_reason(&again).contains("lapsed"),
            "nothing was remembered for the run: it is decided now: {again:?}"
        );
        assert_eq!(store.writes(), 1, "and the expiry is written now");
    }

    /// The scan's budget is Meerkat's attempt timeout less a margin: against
    /// a list that does not answer, the gate's own answer comes after more
    /// than half of an attempt and before its end.
    #[tokio::test(start_paused = true)]
    async fn the_claim_scan_ends_inside_meerkats_attempt_timeout() {
        let attempt = meerkat_runtime::RESUMED_RUN_ADMISSION_ATTEMPT_TIMEOUT;
        let store = SlowListStore::new();
        let gate = WorkGraphResumeGate::new(store.service());
        let worker = Worker::named(WORKER);

        store.slow_from(1, attempt * 100);
        let asked = tokio::time::Instant::now();
        let check = gate.check_worker(worker.resuming(None)).await;
        let waited = asked.elapsed();
        assert!(
            matches!(check, ResumeCheck::Unavailable(_)),
            "the gate ends its scan itself: {check:?}"
        );
        assert!(
            waited < attempt,
            "the gate answers before Meerkat cuts the attempt: waited {waited:?} of {attempt:?}"
        );
        assert!(
            waited > attempt / 2,
            "most of an attempt is left to the read: waited {waited:?} of {attempt:?}"
        );
    }

    #[tokio::test]
    async fn an_unreadable_projection_is_unavailable() {
        let service = service();
        let worker = Worker::named(WORKER);
        let gate = WorkGraphResumeGate::new(service);
        let resuming = ResumingWorker {
            attention_projection_unreadable: Some("missing field `binding_id`"),
            ..worker.resuming(None)
        };

        let check = gate.check_worker(resuming).await;
        assert!(
            matches!(&check, ResumeCheck::Unavailable(reason) if reason.contains("binding_id")),
            "never read as a run without attention: {check:?}"
        );
    }

    /// The trait impl reads Meerkat's request as it is built.
    #[tokio::test]
    async fn the_gate_answers_meerkats_request() {
        use meerkat_mob::DurableResumeGate as _;

        let service = service();
        let (item, projection) = goal_for(
            &service,
            GoalAttentionTarget::Owner {
                owner_key: member_key(WORKER),
            },
        )
        .await;
        claim(&service, &item, member_key(OTHER), in_an_hour()).await;
        let gate = WorkGraphResumeGate::new(service);
        let request = || {
            meerkat_mob::DurableResumeRequest::new(
                meerkat_mob::MobId::from(MOB),
                meerkat_mob::AgentIdentity::from(WORKER),
                meerkat_mob::ProfileName::from("worker"),
                SessionId::new(),
                RunId::new(),
                vec![meerkat_core::lifecycle::InputId::new()],
            )
        };

        let projected = gate
            .check(&request().with_attention_projection(Some(projection)))
            .await;
        assert!(
            matches!(
                &projected,
                meerkat_mob::DurableResumeVerdict::Superseded { reason }
                    if reason.contains(&member_key(OTHER).canonical())
            ),
            "{projected:?}"
        );

        let unreadable = gate
            .check(&request().with_attention_projection_unreadable(Some("bad".to_string())))
            .await;
        assert!(
            matches!(
                unreadable,
                meerkat_mob::DurableResumeVerdict::Unavailable { .. }
            ),
            "{unreadable:?}"
        );

        // No projection: the worker holds no claim, so the run continues.
        let plain = gate.check(&request()).await;
        assert!(
            matches!(plain, meerkat_mob::DurableResumeVerdict::Continue),
            "{plain:?}"
        );
    }

    #[test]
    fn the_verdicts_map_onto_meerkats_vocabulary() {
        assert!(matches!(
            ResumeCheck::Continue.into_verdict(),
            meerkat_mob::DurableResumeVerdict::Continue
        ));
        assert!(matches!(
            ResumeCheck::Superseded("moved".to_string()).into_verdict(),
            meerkat_mob::DurableResumeVerdict::Superseded { reason } if reason == "moved"
        ));
        assert!(matches!(
            ResumeCheck::Unavailable("down".to_string()).into_verdict(),
            meerkat_mob::DurableResumeVerdict::Unavailable { reason } if reason == "down"
        ));
    }
}
