//! One live voice path per gateway, arbitrated "latest engaged wins".
//!
//! A gateway may register console voice (`runtime_options.console_voice`)
//! and the external live channel (`runtime_options.live`) together. Both
//! doors open member channels through the same [`super::GatewayLiveContext`]
//! (one adapter host, one provider registration, one audio owner), so only
//! one of them may hold an active voice path at a time. Meerkat couples
//! nothing here: channels are per session, and a console call on member B
//! and an external channel on member A are independent upstream. The policy
//! that the newest engagement wins, and the loser learns why, is Meta's.
//!
//! The arbiter never closes a channel itself. Each engagement carries the
//! closer its own door would use (the console slot's cancel path, the
//! external host's generated close), so a preempted owner is closed through
//! exactly the sequence it would have used to close on its own. A close that
//! fails leaves the previous owner in place and fails the newcomer closed;
//! there is never a silent double owner.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

/// Typed reason a preempted owner's channel was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSupersededReason {
    /// A console voice call took the live path.
    SupersededByConsoleVoice,
    /// An external `meta/live/open` took the live path.
    SupersededByExternalLive,
    /// The same owner opened again for the same member while its previous
    /// channel was still bound; the previous channel was closed so the new
    /// one is the only live audio owner.
    ReplacedBySameOwner,
}

impl LiveSupersededReason {
    /// Wire spelling shared by the console error, the external status and
    /// close results, and the `meta/live/superseded` notification.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SupersededByConsoleVoice => "superseded_by_console_voice",
            Self::SupersededByExternalLive => "superseded_by_external_live",
            Self::ReplacedBySameOwner => "replaced_by_same_owner",
        }
    }
}

/// Which door holds the live path, and for which member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveOwner {
    ConsoleVoice {
        principal: String,
        identity: String,
        channel_id: Option<String>,
    },
    ExternalLive {
        identity: String,
        channel_id: Option<String>,
    },
}

impl LiveOwner {
    #[must_use]
    pub fn identity(&self) -> &str {
        match self {
            Self::ConsoleVoice { identity, .. } | Self::ExternalLive { identity, .. } => identity,
        }
    }

    #[must_use]
    pub fn channel_id(&self) -> Option<&str> {
        match self {
            Self::ConsoleVoice { channel_id, .. } | Self::ExternalLive { channel_id, .. } => {
                channel_id.as_deref()
            }
        }
    }

    /// The reason the OTHER owner sees when this owner wins.
    #[must_use]
    pub const fn supersedes_with(&self) -> LiveSupersededReason {
        match self {
            Self::ConsoleVoice { .. } => LiveSupersededReason::SupersededByConsoleVoice,
            Self::ExternalLive { .. } => LiveSupersededReason::SupersededByExternalLive,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ConsoleVoice { .. } => "console_voice",
            Self::ExternalLive { .. } => "external_live",
        }
    }

    /// Two engagements by the same door for the same member are one owner
    /// re-engaging (a reopen), never a preemption.
    fn same_door(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::ConsoleVoice {
                    principal: a,
                    identity: b,
                    ..
                },
                Self::ConsoleVoice {
                    principal: c,
                    identity: d,
                    ..
                },
            ) => a == c && b == d,
            (Self::ExternalLive { identity: a, .. }, Self::ExternalLive { identity: b, .. }) => {
                a == b
            }
            _ => false,
        }
    }

    /// `{owner, identity, channel_id}` as reported in `meta/live/open`
    /// results and the readiness holder.
    #[must_use]
    pub fn to_wire(&self) -> Value {
        let mut wire = json!({
            "owner": self.kind(),
            "identity": self.identity(),
        });
        if let Some(channel_id) = self.channel_id() {
            wire["channel_id"] = Value::String(channel_id.to_string());
        }
        wire
    }

    fn set_channel(&mut self, channel: &str) {
        match self {
            Self::ConsoleVoice { channel_id, .. } | Self::ExternalLive { channel_id, .. } => {
                *channel_id = Some(channel.to_string());
            }
        }
    }
}

/// The close an owner would run on itself. The arbiter calls it with the
/// reason the owner is being superseded and the channel it had bound, if any.
pub type LiveOwnerCloser = Arc<
    dyn Fn(
            LiveSupersededReason,
            Option<String>,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;

/// Fire-and-forget notification sink (`method`, `params`); the stdio gateway
/// installs its stdout writer here, HTTP-only hosts leave it empty.
pub type LiveOwnerNotifier = Arc<dyn Fn(&str, Value) + Send + Sync>;

/// Whether a bound channel is still active on the machine. A channel can end
/// without passing through either door (a dropped external WebSocket closes
/// it inside meerkat-live; a console call can die with its provider), so the
/// arbiter asks before it trusts a holder or tries to close one.
pub type LiveOwnerLiveness =
    Arc<dyn Fn(String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// Handle to one engagement. Releasing or binding a stale lease is a no-op,
/// so a slow loser can never disturb the owner that replaced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveOwnerLease(u64);

#[derive(Debug)]
pub enum LiveOwnerArbiterError {
    /// The previous owner's own close sequence failed. It remains the owner.
    CloseFailed {
        kind: &'static str,
        identity: String,
        error: String,
    },
}

impl std::fmt::Display for LiveOwnerArbiterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CloseFailed {
                kind,
                identity,
                error,
            } => write!(
                f,
                "could not close the active {kind} voice path for {identity}: {error}"
            ),
        }
    }
}

impl std::error::Error for LiveOwnerArbiterError {}

struct Engagement {
    lease: LiveOwnerLease,
    owner: LiveOwner,
    closer: LiveOwnerCloser,
}

#[derive(Default)]
struct Ledger {
    current: Option<Engagement>,
    /// Leases preempted before they bound a channel. The owner discovers
    /// this on bind and closes what it just opened.
    superseded_leases: VecDeque<(LiveOwnerLease, LiveSupersededReason)>,
    /// Typed close reasons for channels the arbiter closed, retained so a
    /// late `meta/live/status` or `meta/live/close` can report them.
    close_reasons: VecDeque<(String, LiveSupersededReason, Instant)>,
}

const CLOSE_REASON_RETENTION: Duration = Duration::from_mins(10);
const CLOSE_REASON_CAPACITY: usize = 64;
const SUPERSEDED_LEASE_CAPACITY: usize = 64;

/// See the module docs.
pub struct LiveOwnerArbiter {
    /// Serializes engagements so two newcomers cannot both preempt the same
    /// owner or each other concurrently. Never held while the ledger lock is.
    engage: tokio::sync::Mutex<()>,
    ledger: StdMutex<Ledger>,
    next_lease: AtomicU64,
    notifier: StdMutex<Option<LiveOwnerNotifier>>,
    liveness: StdMutex<Option<LiveOwnerLiveness>>,
}

impl Default for LiveOwnerArbiter {
    fn default() -> Self {
        Self {
            engage: tokio::sync::Mutex::new(()),
            ledger: StdMutex::new(Ledger::default()),
            next_lease: AtomicU64::new(1),
            notifier: StdMutex::new(None),
            liveness: StdMutex::new(None),
        }
    }
}

impl std::fmt::Debug for LiveOwnerArbiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveOwnerArbiter")
            .field("holder", &self.recorded_holder())
            .finish_non_exhaustive()
    }
}

impl LiveOwnerArbiter {
    fn ledger(&self) -> std::sync::MutexGuard<'_, Ledger> {
        self.ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Install the notification sink for `meta/live/superseded`.
    pub fn set_notifier(&self, notifier: LiveOwnerNotifier) {
        *self
            .notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(notifier);
    }

    /// Install the channel liveness probe (see [`LiveOwnerLiveness`]).
    pub fn set_liveness(&self, liveness: LiveOwnerLiveness) {
        *self
            .liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(liveness);
    }

    /// The owner currently holding the live path, if any, without asking the
    /// machine whether its channel still exists. Prefer [`Self::holder`].
    #[must_use]
    pub fn recorded_holder(&self) -> Option<LiveOwner> {
        self.ledger()
            .current
            .as_ref()
            .map(|engagement| engagement.owner.clone())
    }

    /// The owner currently holding the live path, if any. A holder whose
    /// bound channel is no longer active on the machine (its socket dropped,
    /// its provider ended the call) is released here and not reported, so a
    /// path nobody is using can never lock the other door out.
    pub async fn holder(&self) -> Option<LiveOwner> {
        let holder = self.recorded_holder()?;
        if self.channel_is_live(&holder).await {
            return Some(holder);
        }
        if let Some(channel) = holder.channel_id() {
            self.release_channel(channel);
        }
        None
    }

    /// `true` when the owner has no bound channel yet (its open is in flight)
    /// or its bound channel is still active; `false` only for a bound channel
    /// the machine no longer knows.
    async fn channel_is_live(&self, owner: &LiveOwner) -> bool {
        let Some(channel) = owner.channel_id() else {
            return true;
        };
        let probe = self
            .liveness
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        match probe {
            Some(probe) => probe(channel.to_string()).await,
            None => true,
        }
    }

    /// Take the live path for `owner`, closing whatever held it first.
    ///
    /// Returns the lease and the owner that was preempted, if any. Runs the
    /// loser's closer with no lock held other than the engagement serializer.
    /// A holder whose bound channel already ended outside both doors is
    /// simply released. The same door re-engaging for the same member is not
    /// a preemption (nothing is reported as superseded): with no channel
    /// bound yet it replaces its own engagement, and with one bound the old
    /// channel is closed as `replaced_by_same_owner` so the new open is the
    /// only live audio owner even if it later fails.
    pub async fn engage(
        &self,
        owner: LiveOwner,
        closer: LiveOwnerCloser,
    ) -> Result<(LiveOwnerLease, Option<LiveOwner>), LiveOwnerArbiterError> {
        let _serialized = self.engage.lock().await;
        let previous = self.ledger().current.as_ref().map(|engagement| {
            (
                engagement.lease,
                engagement.owner.clone(),
                Arc::clone(&engagement.closer),
            )
        });
        let mut preempted = None;
        if let Some((lease, previous_owner, previous_closer)) = previous {
            let same_door = previous_owner.same_door(&owner);
            let bound = previous_owner.channel_id().map(ToString::to_string);
            let alive = self.channel_is_live(&previous_owner).await;
            let needs_close = alive && !(same_door && bound.is_none());
            let reason = if same_door {
                LiveSupersededReason::ReplacedBySameOwner
            } else {
                owner.supersedes_with()
            };
            if needs_close && let Err(error) = previous_closer(reason, bound.clone()).await {
                return Err(LiveOwnerArbiterError::CloseFailed {
                    kind: previous_owner.kind(),
                    identity: previous_owner.identity().to_string(),
                    error,
                });
            }
            {
                let mut ledger = self.ledger();
                // A different engagement may have replaced the loser while
                // its close ran (it released and someone re-engaged). Only
                // retire the exact engagement we handled.
                if ledger
                    .current
                    .as_ref()
                    .is_some_and(|engagement| engagement.lease == lease)
                {
                    ledger.current = None;
                }
                match bound.as_deref() {
                    Some(channel) if needs_close => {
                        remember_close_reason(&mut ledger, channel, reason);
                    }
                    // Already dead on the machine: nothing was closed here.
                    Some(_) => {}
                    // Still opening: the owner learns on bind that it lost and
                    // closes what it opened, whichever door it belongs to.
                    None => {
                        ledger.superseded_leases.push_back((lease, reason));
                        while ledger.superseded_leases.len() > SUPERSEDED_LEASE_CAPACITY {
                            ledger.superseded_leases.pop_front();
                        }
                    }
                }
            }
            if !same_door && alive {
                self.notify_superseded(&previous_owner, reason, &owner);
                preempted = Some(previous_owner);
            }
        }
        let lease = LiveOwnerLease(self.next_lease.fetch_add(1, Ordering::Relaxed));
        self.ledger().current = Some(Engagement {
            lease,
            owner,
            closer,
        });
        Ok((lease, preempted))
    }

    /// Record the channel the leaseholder opened. Stale leases are ignored.
    pub fn bind_channel(&self, lease: LiveOwnerLease, channel_id: &str) {
        let mut ledger = self.ledger();
        if let Some(engagement) = ledger.current.as_mut()
            && engagement.lease == lease
        {
            engagement.owner.set_channel(channel_id);
        }
    }

    /// Whether `lease` was preempted before it bound a channel. Consumes the
    /// record; the caller closes what it opened and reports the reason.
    pub fn take_superseded(&self, lease: LiveOwnerLease) -> Option<LiveSupersededReason> {
        let mut ledger = self.ledger();
        let index = ledger
            .superseded_leases
            .iter()
            .position(|(candidate, _)| *candidate == lease)?;
        ledger
            .superseded_leases
            .remove(index)
            .map(|(_, reason)| reason)
    }

    /// Give the live path back if `lease` still holds it.
    pub fn release(&self, lease: LiveOwnerLease) {
        let mut ledger = self.ledger();
        if ledger
            .current
            .as_ref()
            .is_some_and(|engagement| engagement.lease == lease)
        {
            ledger.current = None;
        }
    }

    /// Give the live path back if the holder's bound channel is `channel_id`.
    pub fn release_channel(&self, channel_id: &str) {
        let mut ledger = self.ledger();
        if ledger
            .current
            .as_ref()
            .is_some_and(|engagement| engagement.owner.channel_id() == Some(channel_id))
        {
            ledger.current = None;
        }
    }

    /// Typed reason if the arbiter closed `channel_id` recently.
    #[must_use]
    pub fn close_reason(&self, channel_id: &str) -> Option<LiveSupersededReason> {
        let mut ledger = self.ledger();
        let now = Instant::now();
        ledger.close_reasons.retain(|(_, _, recorded)| {
            now.saturating_duration_since(*recorded) < CLOSE_REASON_RETENTION
        });
        ledger
            .close_reasons
            .iter()
            .rev()
            .find(|(candidate, _, _)| candidate == channel_id)
            .map(|(_, reason, _)| *reason)
    }

    /// Record a close reason for a channel closed outside `engage` (a loser
    /// that lost the race before binding and closed itself on bind).
    pub fn record_close_reason(&self, channel_id: &str, reason: LiveSupersededReason) {
        remember_close_reason(&mut self.ledger(), channel_id, reason);
    }

    fn notify_superseded(
        &self,
        loser: &LiveOwner,
        reason: LiveSupersededReason,
        winner: &LiveOwner,
    ) {
        let notifier = self
            .notifier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(notifier) = notifier else {
            return;
        };
        let mut params = json!({
            "owner": loser.kind(),
            "identity": loser.identity(),
            "reason": reason.as_str(),
            "superseded_by": winner.to_wire(),
        });
        if let Some(channel_id) = loser.channel_id() {
            params["channel_id"] = Value::String(channel_id.to_string());
        }
        notifier("meta/live/superseded", params);
    }
}

fn remember_close_reason(ledger: &mut Ledger, channel_id: &str, reason: LiveSupersededReason) {
    ledger
        .close_reasons
        .push_back((channel_id.to_string(), reason, Instant::now()));
    while ledger.close_reasons.len() > CLOSE_REASON_CAPACITY {
        ledger.close_reasons.pop_front();
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn console(identity: &str) -> LiveOwner {
        LiveOwner::ConsoleVoice {
            principal: "voice@example.com".to_string(),
            identity: identity.to_string(),
            channel_id: None,
        }
    }

    fn external(identity: &str) -> LiveOwner {
        LiveOwner::ExternalLive {
            identity: identity.to_string(),
            channel_id: None,
        }
    }

    type CloseCalls = Arc<StdMutex<Vec<(LiveSupersededReason, Option<String>)>>>;

    fn recording_closer(calls: CloseCalls, result: Result<(), String>) -> LiveOwnerCloser {
        Arc::new(move |reason, channel| {
            let calls = Arc::clone(&calls);
            let result = result.clone();
            Box::pin(async move {
                calls.lock().expect("calls").push((reason, channel));
                result
            })
        })
    }

    fn noop_closer() -> LiveOwnerCloser {
        Arc::new(|_, _| Box::pin(async { Ok(()) }))
    }

    #[tokio::test]
    async fn newest_engagement_closes_the_previous_owner_with_a_typed_reason() {
        let arbiter = LiveOwnerArbiter::default();
        let notified = Arc::new(StdMutex::new(Vec::<(String, Value)>::new()));
        let sink = Arc::clone(&notified);
        arbiter.set_notifier(Arc::new(move |method, params| {
            sink.lock()
                .expect("notified")
                .push((method.to_string(), params));
        }));
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (console_lease, preempted) = arbiter
            .engage(
                console("agent-a"),
                recording_closer(Arc::clone(&closes), Ok(())),
            )
            .await
            .expect("first engagement");
        assert!(preempted.is_none());
        arbiter.bind_channel(console_lease, "console-channel");
        assert_eq!(
            arbiter
                .holder()
                .await
                .and_then(|owner| owner.channel_id().map(ToString::to_string)),
            Some("console-channel".to_string())
        );

        let (external_lease, preempted) = arbiter
            .engage(external("agent-b"), noop_closer())
            .await
            .expect("external preempts console");
        let preempted = preempted.expect("console was preempted");
        assert_eq!(preempted.kind(), "console_voice");
        assert_eq!(preempted.channel_id(), Some("console-channel"));
        assert_eq!(
            closes.lock().expect("closes").as_slice(),
            &[(
                LiveSupersededReason::SupersededByExternalLive,
                Some("console-channel".to_string())
            )]
        );
        assert_eq!(
            arbiter.close_reason("console-channel"),
            Some(LiveSupersededReason::SupersededByExternalLive)
        );
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
        {
            let notified = notified.lock().expect("notified");
            assert_eq!(notified.len(), 1);
            assert_eq!(notified[0].0, "meta/live/superseded");
            assert_eq!(notified[0].1["reason"], "superseded_by_external_live");
            assert_eq!(notified[0].1["channel_id"], "console-channel");
            assert_eq!(notified[0].1["superseded_by"]["owner"], "external_live");
        }

        // A stale release from the loser must not evict the winner.
        arbiter.release(console_lease);
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
        arbiter.release(external_lease);
        assert!(arbiter.holder().await.is_none());
    }

    #[tokio::test]
    async fn same_door_reengaging_is_never_reported_as_a_preemption() {
        let arbiter = LiveOwnerArbiter::default();
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (first, _) = arbiter
            .engage(
                console("agent-a"),
                recording_closer(Arc::clone(&closes), Ok(())),
            )
            .await
            .expect("first");
        arbiter.bind_channel(first, "channel-1");
        let (second, preempted) = arbiter
            .engage(console("agent-a"), noop_closer())
            .await
            .expect("reopen");
        assert!(preempted.is_none(), "a reopen supersedes nobody");
        assert_ne!(first, second);
        // ... but the previous call's channel is closed rather than left live.
        assert_eq!(
            closes.lock().expect("closes").as_slice(),
            &[(
                LiveSupersededReason::ReplacedBySameOwner,
                Some("channel-1".to_string())
            )]
        );
        // The console switching to ANOTHER agent is a different owner and
        // closes the previous call, matching the browser's close-before-switch.
        arbiter.bind_channel(second, "channel-2");
        let (_, preempted) = arbiter
            .engage(console("agent-b"), noop_closer())
            .await
            .expect("switch");
        assert_eq!(
            preempted.map(|owner| owner.identity().to_string()),
            Some("agent-a".to_string())
        );
        assert_eq!(
            arbiter.close_reason("channel-2"),
            Some(LiveSupersededReason::SupersededByConsoleVoice)
        );
    }

    #[tokio::test]
    async fn a_failed_close_keeps_the_previous_owner_and_fails_the_newcomer() {
        let arbiter = LiveOwnerArbiter::default();
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (lease, _) = arbiter
            .engage(
                external("agent-a"),
                recording_closer(Arc::clone(&closes), Err("provider hung".to_string())),
            )
            .await
            .expect("first");
        arbiter.bind_channel(lease, "external-channel");
        let error = arbiter
            .engage(console("agent-b"), noop_closer())
            .await
            .expect_err("close failure fails the newcomer");
        assert!(matches!(
            error,
            LiveOwnerArbiterError::CloseFailed {
                kind: "external_live",
                ..
            }
        ));
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );
        assert!(arbiter.close_reason("external-channel").is_none());
    }

    #[tokio::test]
    async fn preempting_an_unbound_lease_is_discovered_on_bind() {
        let arbiter = LiveOwnerArbiter::default();
        let (opening, _) = arbiter
            .engage(external("agent-a"), noop_closer())
            .await
            .expect("opening");
        let (_, preempted) = arbiter
            .engage(console("agent-b"), noop_closer())
            .await
            .expect("console wins the race");
        assert_eq!(preempted.map(|owner| owner.kind()), Some("external_live"));
        assert_eq!(
            arbiter.take_superseded(opening),
            Some(LiveSupersededReason::SupersededByConsoleVoice)
        );
        assert!(arbiter.take_superseded(opening).is_none());
        // Binding after the loss must not overwrite the winner.
        arbiter.bind_channel(opening, "late-channel");
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("console_voice")
        );
    }

    #[tokio::test]
    async fn release_by_channel_only_frees_the_matching_holder() {
        let arbiter = LiveOwnerArbiter::default();
        let (lease, _) = arbiter
            .engage(external("agent-a"), noop_closer())
            .await
            .expect("engage");
        arbiter.bind_channel(lease, "channel-a");
        arbiter.release_channel("channel-other");
        assert!(arbiter.holder().await.is_some());
        arbiter.release_channel("channel-a");
        assert!(arbiter.holder().await.is_none());
    }

    /// A liveness probe backed by a set of channel ids the "machine" knows.
    fn liveness(live: Arc<StdMutex<std::collections::HashSet<String>>>) -> LiveOwnerLiveness {
        Arc::new(move |channel| {
            let live = Arc::clone(&live);
            Box::pin(async move { live.lock().expect("live").contains(&channel) })
        })
    }

    #[tokio::test]
    async fn a_holder_whose_channel_ended_outside_the_doors_is_released_not_trusted() {
        let arbiter = LiveOwnerArbiter::default();
        let live = Arc::new(StdMutex::new(std::collections::HashSet::new()));
        arbiter.set_liveness(liveness(Arc::clone(&live)));
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (lease, _) = arbiter
            .engage(
                external("agent-a"),
                recording_closer(Arc::clone(&closes), Err("BindingMismatch".to_string())),
            )
            .await
            .expect("engage");
        arbiter.bind_channel(lease, "dropped-socket");
        live.lock()
            .expect("live")
            .insert("dropped-socket".to_string());
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("external_live")
        );

        // The WebSocket drops: meerkat-live closes the channel itself and
        // neither door hears about it.
        live.lock().expect("live").clear();
        assert!(
            arbiter.holder().await.is_none(),
            "a dead holder is not reported"
        );
        assert!(arbiter.recorded_holder().is_none(), "and it is released");

        // The reverse order: engage while the dead holder is still recorded.
        let (lease, _) = arbiter
            .engage(
                external("agent-a"),
                recording_closer(Arc::clone(&closes), Err("BindingMismatch".to_string())),
            )
            .await
            .expect("engage again");
        arbiter.bind_channel(lease, "dropped-again");
        let (_, preempted) = arbiter
            .engage(console("agent-b"), noop_closer())
            .await
            .expect("console engages over a dead external holder");
        assert!(preempted.is_none(), "nothing live was preempted");
        assert!(
            closes.lock().expect("closes").is_empty(),
            "a closer that would fail on a gone channel is never run"
        );
        assert_eq!(
            arbiter.holder().await.map(|owner| owner.kind()),
            Some("console_voice")
        );
    }

    #[tokio::test]
    async fn same_owner_reopening_with_a_bound_channel_closes_the_old_one_first() {
        let arbiter = LiveOwnerArbiter::default();
        let notified = Arc::new(StdMutex::new(Vec::<(String, Value)>::new()));
        let sink = Arc::clone(&notified);
        arbiter.set_notifier(Arc::new(move |method, params| {
            sink.lock()
                .expect("notified")
                .push((method.to_string(), params));
        }));
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (first, _) = arbiter
            .engage(
                external("agent-a"),
                recording_closer(Arc::clone(&closes), Ok(())),
            )
            .await
            .expect("first");
        arbiter.bind_channel(first, "channel-1");
        let (second, preempted) = arbiter
            .engage(external("agent-a"), noop_closer())
            .await
            .expect("reopen");
        assert!(
            preempted.is_none(),
            "a self-replacement is not a preemption"
        );
        assert_eq!(
            closes.lock().expect("closes").as_slice(),
            &[(
                LiveSupersededReason::ReplacedBySameOwner,
                Some("channel-1".to_string())
            )],
            "the previous channel is closed so two audio owners never coexist"
        );
        assert_eq!(
            arbiter.close_reason("channel-1"),
            Some(LiveSupersededReason::ReplacedBySameOwner)
        );
        assert!(
            notified.lock().expect("notified").is_empty(),
            "no supersession is announced"
        );
        // If the reopen then fails and releases, nothing live is left behind,
        // so a console open finds the path free and preempts nothing.
        arbiter.release(second);
        let (_, preempted) = arbiter
            .engage(console("agent-b"), noop_closer())
            .await
            .expect("console after failed reopen");
        assert!(preempted.is_none());
    }

    #[tokio::test]
    async fn same_owner_reopening_before_binding_replaces_without_closing() {
        let arbiter = LiveOwnerArbiter::default();
        let closes = Arc::new(StdMutex::new(Vec::new()));
        let (first, _) = arbiter
            .engage(
                external("agent-a"),
                recording_closer(Arc::clone(&closes), Ok(())),
            )
            .await
            .expect("first");
        let (second, preempted) = arbiter
            .engage(external("agent-a"), noop_closer())
            .await
            .expect("reopen while the first is still opening");
        assert!(preempted.is_none());
        assert!(closes.lock().expect("closes").is_empty());
        assert_ne!(first, second);
        assert_eq!(
            arbiter.take_superseded(first),
            Some(LiveSupersededReason::ReplacedBySameOwner),
            "the first open learns on bind that it must close what it opened"
        );
    }

    #[tokio::test]
    async fn engagements_are_serialized() {
        let arbiter = Arc::new(LiveOwnerArbiter::default());
        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for index in 0..8 {
            let arbiter = Arc::clone(&arbiter);
            let in_flight = Arc::clone(&in_flight);
            let max_seen = Arc::clone(&max_seen);
            tasks.push(tokio::spawn(async move {
                let owner = if index % 2 == 0 {
                    external(&format!("agent-{index}"))
                } else {
                    console(&format!("agent-{index}"))
                };
                let closer: LiveOwnerCloser = Arc::new(move |_, _| {
                    let in_flight = Arc::clone(&in_flight);
                    let max_seen = Arc::clone(&max_seen);
                    Box::pin(async move {
                        let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        tokio::task::yield_now().await;
                        in_flight.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                });
                arbiter.engage(owner, closer).await.expect("engage")
            }));
        }
        for task in tasks {
            task.await.expect("task");
        }
        assert!(
            max_seen.load(Ordering::SeqCst) <= 1,
            "closers ran concurrently"
        );
        assert!(arbiter.holder().await.is_some());
    }
}
