//! Recipient routing on a store shared by several processes (#1813).
//!
//! Each recipient of a delivery row is applied by the runtime owner hosting
//! the recipient's own session, never by the origin row's process. These
//! tests drive the applier with a scripted router standing in for each
//! process:
//! - a pass with nothing it may apply issues no delivery-authority input;
//! - a cold claim another process holds is refused before any input;
//! - the residual race inside a sink degrades to a skip, never a settlement;
//! - two processes serving different recipients of one row finish it
//!   between them, each recipient applied exactly once;
//! - cold recipients wait for the cold-delivery owner.

use std::collections::HashMap;

use super::*;
use meerkat::{DeliveryRoute, JobDeliveryRouter};
use meerkat_runtime::{
    HostingCapability, HostingClaim, HostingOwner, HostingRefused, RuntimeStore, ServedElsewhere,
};

/// How one scripted process routes a recipient session.
#[derive(Clone, Copy, Debug)]
enum Route {
    Here,
    Elsewhere,
    /// Not held in this process, and free: a cold owner's claim succeeds.
    Unserved,
    /// Not held in this process, and another process holds the session's
    /// claim: a cold owner's claim attempt is refused.
    UnservedHeldByPeer,
    /// Not held in this process and claimable, but another runtime owner
    /// takes the session inside the sink: the sink refuses `ServedElsewhere`.
    UnservedRaced,
}

struct RacedSink;

#[async_trait::async_trait]
impl JobDeliverySink for RacedSink {
    async fn apply(
        &self,
        application: JobDeliveryApplication,
    ) -> Result<(), JobDeliveryApplyError> {
        let session_id = match &application {
            JobDeliveryApplication::Record { subscription, .. }
            | JobDeliveryApplication::Notification { subscription, .. }
            | JobDeliveryApplication::Event { subscription, .. } => {
                subscription.session_id().clone()
            }
        };
        Err(JobDeliveryApplyError::ServedElsewhere { session_id })
    }
}

struct ScriptedRouter {
    routes: HashMap<SessionId, Route>,
    sink: Arc<RecordingDeliverySink>,
    owner: HostingOwner,
    cold_claims: std::sync::Mutex<Vec<SessionId>>,
}

impl ScriptedRouter {
    fn route_of(&self, recipient: &SessionId) -> Route {
        self.routes.get(recipient).copied().unwrap_or(Route::Here)
    }
}

#[async_trait::async_trait]
impl JobDeliveryRouter for ScriptedRouter {
    async fn route(&self, recipient: &SessionId) -> Option<DeliveryRoute> {
        let sink: Arc<dyn JobDeliverySink> = self.sink.clone();
        Some(match self.route_of(recipient) {
            Route::Here => DeliveryRoute::ServedHere(sink),
            Route::Elsewhere => DeliveryRoute::ServedElsewhere,
            Route::Unserved | Route::UnservedHeldByPeer => DeliveryRoute::Unserved(sink),
            Route::UnservedRaced => DeliveryRoute::Unserved(Arc::new(RacedSink)),
        })
    }

    async fn claim_cold(
        &self,
        recipient: &SessionId,
    ) -> Option<Result<HostingClaim, HostingRefused>> {
        self.cold_claims
            .lock()
            .expect("cold claims")
            .push(recipient.clone());
        Some(match self.route_of(recipient) {
            Route::UnservedHeldByPeer => Err(HostingRefused::ServedElsewhere(ServedElsewhere {
                session_id: recipient.clone(),
            })),
            _ => meerkat_runtime::grant_session_hosting(
                &HostingCapability::ProcessLocal,
                &self.owner,
                recipient,
            ),
        })
    }
}

struct Fixture {
    store: Arc<InMemoryRuntimeStore>,
    inbox: RuntimeDeliveryInbox,
    runtime_id: LogicalRuntimeId,
    x: SessionId,
    y: SessionId,
}

impl Fixture {
    /// One completed job whose row (in the origin's inbox) has two
    /// Notification recipients, sessions `x` and `y`.
    async fn new() -> Self {
        let job_store = Arc::new(MemoryDetachedJobStore::new());
        let jobs = DetachedJobService::new(job_store.clone());
        let store = Arc::new(InMemoryRuntimeStore::new());
        let inbox = RuntimeDeliveryInbox::new(store.clone());
        let origin = SessionId::new();
        let (x, y) = (SessionId::new(), SessionId::new());
        let receipt = jobs
            .submit(spec("routed-recipients", origin.clone()))
            .await
            .expect("submit job");
        for (id, session) in [("x", &x), ("y", &y)] {
            jobs.subscribe(
                &receipt.job_id,
                JobSubscription::new(
                    JobSubscriptionId::new(id).expect("subscription id"),
                    session.clone(),
                    JobDeliveryKind::Notification,
                ),
            )
            .await
            .expect("subscribe");
        }
        let claim = jobs
            .claim_attempt(
                &receipt.job_id,
                AttemptClaim::new(
                    WorkerId::new("routing-worker").expect("worker"),
                    1,
                    100,
                    RunnerHandleRef::new("routing-handle").expect("handle"),
                ),
            )
            .await
            .expect("claim");
        jobs.complete_attempt(
            &receipt.job_id,
            (&claim).into(),
            2,
            Some(JobResultRef::new("routed-result").expect("result")),
        )
        .await
        .expect("complete");
        let projected = JobOutboxProjector::new(job_store, inbox.clone())
            .project_pending(10)
            .await
            .expect("project");
        assert_eq!(projected.projected.len(), 1);
        Self {
            store,
            inbox,
            runtime_id: LogicalRuntimeId::for_session(&origin),
            x,
            y,
        }
    }

    fn applier(
        &self,
        routes: &[(&SessionId, Route)],
        applies_cold_deliveries: bool,
    ) -> (JobRuntimeDeliveryApplier, Arc<RecordingDeliverySink>) {
        let (applier, sink, _router) = self.applier_with_router(routes, applies_cold_deliveries);
        (applier, sink)
    }

    fn applier_with_router(
        &self,
        routes: &[(&SessionId, Route)],
        applies_cold_deliveries: bool,
    ) -> (
        JobRuntimeDeliveryApplier,
        Arc<RecordingDeliverySink>,
        Arc<ScriptedRouter>,
    ) {
        let sink = Arc::new(RecordingDeliverySink::default());
        let router = Arc::new(ScriptedRouter {
            routes: routes
                .iter()
                .map(|(session, route)| ((*session).clone(), *route))
                .collect(),
            sink: sink.clone(),
            owner: HostingOwner::mint(),
            cold_claims: std::sync::Mutex::new(Vec::new()),
        });
        (
            JobRuntimeDeliveryApplier::routed(
                self.inbox.clone(),
                Arc::clone(&router) as Arc<dyn JobDeliveryRouter>,
                applies_cold_deliveries,
            ),
            sink,
            router,
        )
    }

    async fn authority_revision(&self) -> Option<u64> {
        self.store
            .load_runtime_delivery_authority(&self.runtime_id)
            .await
            .expect("read delivery authority")
            .map(|authority| authority.revision())
    }

    async fn pending(&self) -> usize {
        self.inbox
            .list_pending(&self.runtime_id, 10)
            .await
            .expect("list pending")
            .len()
    }
}

async fn applied_recipients(sink: &RecordingDeliverySink) -> Vec<String> {
    sink.applications
        .lock()
        .await
        .iter()
        .map(|application| match application {
            JobDeliveryApplication::Record { subscription, .. }
            | JobDeliveryApplication::Notification { subscription, .. }
            | JobDeliveryApplication::Event { subscription, .. } => {
                subscription.subscription_id().as_str().to_string()
            }
        })
        .collect()
}

/// tlc-gate pin 2a: a pass whose recipients are all served elsewhere issues
/// no delivery-authority input at all (no bind, settle, finish or
/// acknowledge): the authority revision is unchanged and the row stays
/// pending.
#[tokio::test]
async fn a_pass_that_only_skips_issues_no_delivery_authority_input() {
    let fixture = Fixture::new().await;
    let before = fixture.authority_revision().await;
    let (applier, sink) = fixture.applier(
        &[
            (&fixture.x, Route::Elsewhere),
            (&fixture.y, Route::Elsewhere),
        ],
        true,
    );
    let drain = applier
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    let awaiting = drain
        .awaiting_other_hosts
        .expect("the row awaits other hosts");
    assert!(!awaiting.touched_authority, "{awaiting:?}");
    assert_eq!(
        awaiting.skipped_recipients,
        vec![fixture.x.clone(), fixture.y.clone()]
    );
    assert!(drain.blocked.is_none() && drain.applied.is_empty());
    assert!(applied_recipients(&sink).await.is_empty());
    assert_eq!(fixture.authority_revision().await, before);
    assert_eq!(fixture.pending().await, 1);
}

/// tlc-gate pin 2b at the claim: the cold-delivery owner takes a cold
/// recipient's hosting claim before any delivery-authority input; when
/// another process holds it, the recipient is skipped and a pass with
/// nothing else to apply leaves the row's authority untouched (no bind, no
/// settlement), pending for the session's host.
#[tokio::test]
async fn a_refused_cold_claim_issues_no_delivery_authority_input() {
    let fixture = Fixture::new().await;
    let before = fixture.authority_revision().await;
    let (cold_owner, cold_sink, router) = fixture.applier_with_router(
        &[
            (&fixture.x, Route::UnservedHeldByPeer),
            (&fixture.y, Route::UnservedHeldByPeer),
        ],
        true,
    );
    let drain = cold_owner
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    let awaiting = drain
        .awaiting_other_hosts
        .expect("the row awaits the sessions' host");
    assert!(!awaiting.touched_authority, "{awaiting:?}");
    assert!(drain.blocked.is_none() && drain.applied.is_empty());
    assert!(applied_recipients(&cold_sink).await.is_empty());
    assert_eq!(
        router.cold_claims.lock().expect("cold claims").len(),
        2,
        "both cold claims were attempted, before any input"
    );
    assert_eq!(fixture.authority_revision().await, before);
    assert_eq!(fixture.pending().await, 1);
}

/// tlc-gate pin 2b inside the sink: a recipient whose cold claim succeeded,
/// but whose session another runtime owner takes before the sink applies it
/// (the sink refuses `ServedElsewhere`), is skipped: not settled, not
/// blocked. The sibling served here is applied and settled, and the row
/// stays pending for the recipient's host.
#[tokio::test]
async fn the_unserved_then_hosted_race_degrades_to_a_skip() {
    let fixture = Fixture::new().await;
    let (cold_owner, cold_sink) = fixture.applier(
        &[
            (&fixture.x, Route::Here),
            (&fixture.y, Route::UnservedRaced),
        ],
        true,
    );
    let drain = cold_owner
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    assert!(
        drain.blocked.is_none(),
        "a race is never a blocked delivery"
    );
    let awaiting = drain.awaiting_other_hosts.expect("y stays pending");
    assert_eq!(awaiting.skipped_recipients, vec![fixture.y.clone()]);
    assert_eq!(applied_recipients(&cold_sink).await, vec!["x".to_string()]);
    assert_eq!(fixture.pending().await, 1);

    // The process that attached y applies it; x is not applied again.
    let (host, host_sink) = fixture.applier(
        &[(&fixture.x, Route::Elsewhere), (&fixture.y, Route::Here)],
        false,
    );
    let drain = host
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    assert!(drain.awaiting_other_hosts.is_none() && drain.blocked.is_none());
    assert_eq!(applied_recipients(&host_sink).await, vec!["y".to_string()]);
    assert_eq!(
        fixture.pending().await,
        0,
        "the last settlement finished the row"
    );
}

/// Origin and recipients hosted by different processes: neither process
/// strands the other. Each applies the recipient it hosts exactly once, and
/// whichever settles last finishes the row.
#[tokio::test]
async fn two_processes_serving_different_recipients_finish_the_row_between_them() {
    let fixture = Fixture::new().await;
    let (a, a_sink) = fixture.applier(
        &[(&fixture.x, Route::Here), (&fixture.y, Route::Elsewhere)],
        false,
    );
    let (b, b_sink) = fixture.applier(
        &[(&fixture.x, Route::Elsewhere), (&fixture.y, Route::Here)],
        false,
    );
    let first = a
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("a drains");
    assert!(first.awaiting_other_hosts.is_some());
    // A second pass by A changes nothing: x is settled, y is not A's.
    let again = a
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("a drains");
    assert!(again.awaiting_other_hosts.is_some());
    let second = b
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("b drains");
    assert!(second.awaiting_other_hosts.is_none() && second.blocked.is_none());
    assert_eq!(applied_recipients(&a_sink).await, vec!["x".to_string()]);
    assert_eq!(applied_recipients(&b_sink).await, vec!["y".to_string()]);
    assert_eq!(fixture.pending().await, 0);
}

/// Recipients no process hosts are applied only by the cold-delivery owner.
#[tokio::test]
async fn cold_recipients_wait_for_the_cold_delivery_owner() {
    let fixture = Fixture::new().await;
    let routes = [(&fixture.x, Route::Unserved), (&fixture.y, Route::Unserved)];
    let before = fixture.authority_revision().await;
    let (non_owner, non_owner_sink) = fixture.applier(&routes, false);
    let drain = non_owner
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    assert!(
        !drain
            .awaiting_other_hosts
            .expect("cold rows wait")
            .touched_authority
    );
    assert!(applied_recipients(&non_owner_sink).await.is_empty());
    assert_eq!(fixture.authority_revision().await, before);

    let (cold_owner, cold_sink) = fixture.applier(&routes, true);
    let drain = cold_owner
        .apply_pending(&fixture.runtime_id, 10)
        .await
        .expect("drain");
    assert!(drain.awaiting_other_hosts.is_none() && drain.blocked.is_none());
    let mut applied = applied_recipients(&cold_sink).await;
    applied.sort();
    assert_eq!(applied, vec!["x".to_string(), "y".to_string()]);
    assert_eq!(fixture.pending().await, 0);
}
