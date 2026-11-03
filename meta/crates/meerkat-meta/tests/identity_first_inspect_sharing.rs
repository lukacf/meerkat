#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Member-status load (K1): `IdentityRuntime::inspect`, behind
//! `meta/inspect_identity` and `wait_for_output`, shares its bridge read.
//!
//! The bridge's inspection is a full meerkat member-status read, and a mob
//! admits one such read at a time (before meerkat 0.8.45 the rest were
//! refused as `observation_lane_saturated`). Concurrent inspections of one
//! member incarnation now join one read in flight. A settled read is never
//! reused, and a caller never joins a read that started before a completion
//! it already counts: `meta/inspect_identity` returns the completion
//! cursor next to the output, and SDK completion waits take the output as
//! soon as the cursor moves.

#[path = "support/core_members.rs"]
mod core_members;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeDelivery,
    BridgeError, CompletionCursor, DurabilityPolicy, DurableAgentSpec, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MemberInspection,
    ResumeSessionOutcome, SessionBridge, SessionSnapshot,
};

/// Stands in for the bridge's member-status read: counts reads, takes
/// `delay` per read (the way a busy member's status read does), and reports
/// the output that was committed when the read started.
struct CountingInspectBridge {
    reads: AtomicUsize,
    delay: Duration,
    fail_next: AtomicBool,
    committed_output: std::sync::Mutex<Option<String>>,
    /// The members Core holds, as the tests bind them and this double's
    /// creates, resumes and retires leave them.
    core: core_members::CoreMembers,
}

impl CountingInspectBridge {
    fn new(delay: Duration) -> Self {
        Self {
            reads: AtomicUsize::new(0),
            delay,
            fail_next: AtomicBool::new(false),
            committed_output: std::sync::Mutex::new(None),
            core: core_members::CoreMembers::default(),
        }
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn commit_output(&self, output: &str) {
        *self.committed_output.lock().unwrap() = Some(output.to_string());
    }
}

#[async_trait]
impl SessionBridge for CountingInspectBridge {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<meerkat_meta::identity_first::MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let session_id = &meerkat_core::types::SessionId::new();
        let binding = meerkat_meta::identity_first::MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id.clone()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.core.bind(
            identity,
            &meerkat_meta::identity_first::MemberRuntimeBinding {
                agent_runtime_id: agent_runtime_id.clone(),
                session_id: Some(session_id.clone()),
            },
            &spec.profile,
        );
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        _runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        Ok(meerkat_core::types::SessionId::new())
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn inspect_member(
        &self,
        runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        // Capture the output before counting the read: a test that sees the
        // count knows what this read reports.
        let committed = self.committed_output.lock().unwrap().clone();
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::time::sleep(self.delay).await;
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(BridgeError::Mob(
                "mob lifecycle operation admission is still pending at \
                 observation_lane_saturated: member_status_observation"
                    .to_string(),
            ));
        }
        Ok(MemberInspection {
            output_preview: committed.or_else(|| Some(format!("{runtime_id} read {read}"))),
            preview_unavailable: None,
            is_final: false,
            peer_reachable_count: 0,
        })
    }
}

fn make_runtime(bridge: Arc<CountingInspectBridge>) -> Arc<IdentityRuntime> {
    let store: Arc<dyn ContinuityStore> = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "inspect-sharing-test".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }))
}

/// Register `name` with Core holding its member live at generation 0
/// (runtime id `rt:{name}:0`). Core binds the member first, so a wait woken
/// by the registration already reads it.
async fn register_active(
    runtime: &IdentityRuntime,
    bridge: &CountingInspectBridge,
    name: &str,
) -> AgentIdentity {
    let identity = AgentIdentity::parse(name).unwrap();
    let profile = meerkat_mob::ProfileName::from("default");
    bridge.core.bind_live(
        &identity,
        &profile,
        0,
        Some(meerkat_core::types::SessionId::new()),
    );
    runtime
        .register(DurableAgentSpec {
            identity: identity.clone(),
            profile,
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
        .await;
    identity
}

/// Two concurrent `inspect_identity` calls for one identity make a single
/// member-status read, and both get its result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_inspections_of_one_identity_share_one_read() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::from_millis(300)));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    let (first, second) = tokio::join!(runtime.inspect(&calendar), runtime.inspect(&calendar));
    let (first, second) = (first.expect("first"), second.expect("second"));
    assert_eq!(bridge.reads(), 1, "one member-status read for both callers");
    assert_eq!(first.output_preview, second.output_preview);
}

/// A settled inspection is never reused: the next caller reads afresh.
#[tokio::test]
async fn a_settled_inspection_is_never_reused() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    runtime.inspect(&calendar).await.expect("first read");
    runtime.inspect(&calendar).await.expect("second read");
    assert_eq!(bridge.reads(), 2);
}

/// What an SDK completion wait does against `meta/inspect_identity`:
/// read the cursor, then the inspection, and return the output as soon as
/// the cursor has moved past `baseline`.
async fn sdk_wait_for_completion(
    runtime: &IdentityRuntime,
    identity: &AgentIdentity,
    baseline: CompletionCursor,
) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let cursor = runtime.completion_cursor(identity).await;
            let inspection = runtime.inspect(identity).await.expect("inspect");
            if cursor > baseline {
                return inspection.output_preview;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the turn completes")
}

/// Review of #451 (P1): an inspection of turn A must not go out next to
/// the cursor of a later turn B. With the old one-second reuse, an
/// inspection primed with A was returned after B completed, and an SDK
/// completion wait returned A's text for turn B.
#[tokio::test]
async fn an_inspection_after_a_completion_reports_the_new_output() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    bridge.commit_output("A");
    let baseline = runtime.completion_cursor(&calendar).await;
    let primed = runtime.inspect(&calendar).await.expect("prime with A");
    assert_eq!(primed.output_preview.as_deref(), Some("A"));

    // Turn B completes well within the old reuse window.
    bridge.commit_output("B");
    let completed = runtime.record_turn_completed(&calendar).await;
    assert!(completed > baseline);

    assert_eq!(
        sdk_wait_for_completion(&runtime, &calendar, baseline)
            .await
            .as_deref(),
        Some("B")
    );
    let inspection = runtime.inspect(&calendar).await.expect("inspect");
    assert_eq!(inspection.output_preview.as_deref(), Some("B"));
    assert_eq!(runtime.completion_cursor(&calendar).await, completed);
}

/// A read still in flight when a turn completes may report the previous
/// output; a caller that already counts the new turn starts its own read
/// instead of joining it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_in_flight_across_a_completion_is_not_joined_by_a_later_caller() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::from_millis(300)));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    bridge.commit_output("A");
    let before = tokio::spawn({
        let runtime = Arc::clone(&runtime);
        let calendar = calendar.clone();
        async move { runtime.inspect(&calendar).await }
    });
    // Turn B completes only once the first read is in flight (it has captured A).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while bridge.reads() == 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the first read never started"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    bridge.commit_output("B");
    runtime.record_turn_completed(&calendar).await;

    let after = runtime.inspect(&calendar).await.expect("after the turn");
    assert_eq!(after.output_preview.as_deref(), Some("B"));
    let before = before.await.expect("join").expect("before the turn");
    assert_eq!(before.output_preview.as_deref(), Some("A"));
    assert_eq!(bridge.reads(), 2);
}

/// A failed read is shared only with the callers that waited on it; the
/// next caller reads again.
#[tokio::test]
async fn a_failed_inspection_is_not_reused() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    bridge.fail_next.store(true, Ordering::SeqCst);
    assert!(runtime.inspect(&calendar).await.is_err());
    runtime.inspect(&calendar).await.expect("read again");
    assert_eq!(bridge.reads(), 2);
}

/// Different members are never shared.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inspections_of_different_identities_are_read_separately() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::from_millis(100)));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;
    let mail = register_active(&runtime, &bridge, "mail").await;

    let (calendar_view, mail_view) =
        tokio::join!(runtime.inspect(&calendar), runtime.inspect(&mail));
    assert_eq!(bridge.reads(), 2);
    assert_ne!(
        calendar_view.expect("calendar").output_preview,
        mail_view.expect("mail").output_preview
    );
}

/// `wait_for_output*` never polls. Under a paused clock with no completion,
/// a wait reads the member exactly once and then sleeps to its deadline; a
/// sleep-and-retry loop would read once per interval (1200 reads for a
/// 500 ms poll over ten minutes) and fail this.
#[tokio::test(start_paused = true)]
async fn an_output_wait_with_nothing_committed_reads_once_and_times_out() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    // The guard is virtual time too: a loop whose deadline is not on the
    // tokio clock, or that keeps re-reading, overruns it and fails here.
    let error = tokio::time::timeout(
        Duration::from_mins(11),
        runtime.wait_for_output_containing(&calendar, "DONE", Duration::from_mins(10)),
    )
    .await
    .expect("the wait ends at its own deadline")
    .expect_err("nothing ever contains DONE");
    assert!(error.to_string().contains("timed out"), "{error}");
    assert_eq!(
        bridge.reads(),
        1,
        "one read, then a typed wait to the deadline"
    );
}

/// A completion wakes the wait, which reads the new output and returns it,
/// without a timer.
#[tokio::test(start_paused = true)]
async fn an_output_wait_wakes_on_the_completion_that_commits_the_output() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let calendar = register_active(&runtime, &bridge, "calendar").await;

    let wait = runtime.wait_for_output_containing(&calendar, "DONE", Duration::from_mins(10));
    let commit = async {
        tokio::task::yield_now().await;
        bridge.commit_output("all DONE");
        runtime.record_turn_completed(&calendar).await;
    };
    let (output, ()) = tokio::join!(wait, commit);
    assert_eq!(output.expect("woken by the completion"), "all DONE");
    assert!(
        (1..=2).contains(&bridge.reads()),
        "at most one read before and one after the completion: {}",
        bridge.reads()
    );
}

/// A wait started before the identity is registered reads again when the
/// registration (an identity-table write) lands.
#[tokio::test(start_paused = true)]
async fn an_output_wait_started_before_registration_reads_once_it_registers() {
    let bridge = Arc::new(CountingInspectBridge::new(Duration::ZERO));
    let runtime = make_runtime(Arc::clone(&bridge));
    let late = AgentIdentity::parse("late").unwrap();

    let wait = runtime.wait_for_output(&late, Duration::from_mins(10));
    let register = async {
        tokio::task::yield_now().await;
        register_active(&runtime, &bridge, "late").await;
    };
    let (output, ()) = tokio::join!(wait, register);
    assert_eq!(
        output.expect("woken by the registration"),
        "rt:late:0 read 1"
    );
    assert_eq!(
        bridge.reads(),
        1,
        "the unregistered reads never reached the bridge"
    );
}
