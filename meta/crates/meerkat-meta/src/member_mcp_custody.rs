//! Custody of the MCP servers a gateway's members own.
//!
//! A member's MCP router adapter belongs to its agent, which releases it when
//! its session ends. That release is not a cleanup receipt: it returns unit,
//! and a panicked or aborted session skips it. The member teardown that drives
//! it is also bounded ([`MOB_TERMINAL_SHUTDOWN_BUDGET`]) independently of a
//! stdio close's own bound ([`McpStdioCloseBounds::close_bound`]), so a
//! release can be cut off while its router's closes still run.
//!
//! So every member factory adopts each adapter into a gateway's custody at
//! the factory's ownership seam, before any server is staged or connected
//! ([`AgentFactory::with_mcp_adapter_custody`]). Custody keeps the adapter
//! itself until the adapter's own shutdown reported, and then only that
//! report's native outcomes, so no release, cut off or skipped, loses the
//! closes its router owns. Once custody is an adapter's last owner and no
//! shutdown reported, custody starts that shutdown itself, as an abandoned
//! construction's cleanup does, so it never keeps a server running past its
//! owners. Settlement freezes the custody and, for every adapter whose
//! shutdown has not reported, starts its closes at once
//! ([`McpRouterAdapter::begin_close`], behind no lock) and waits for each
//! close's native outcome until the deadline Meerkat derives from the closes
//! it started or joined. The settlement runs again at the gateway's
//! executor-exit boundary, after the runtime shutdown's observation ends for
//! whatever reason, and there no deadline ends a wait: the boundary keeps
//! waiting on each original close until its native outcome reports, so
//! neither a horizon cut-off nor a bound Meerkat broke leaves a close running
//! when the executor is destroyed. A native attempt that returned unsettled
//! is reported incomplete by its class, and a close that had not reported by
//! its deadline is reported under `deadline` as the bound Meerkat broke.
//!
//! [`MOB_TERMINAL_SHUTDOWN_BUDGET`]: crate::unified_runtime::lifecycle::MOB_TERMINAL_SHUTDOWN_BUDGET

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use meerkat::{AgentFactory, McpAdapterCustody, McpAdapterCustodyClosed};
use meerkat_mcp::{
    McpCloseTicket, McpRouterAdapter, McpRouterShutdownObservation, McpRouterShutdownObserver,
    McpRouterShutdownReport, McpServerCloseOutcome, McpServerCloseRecord, McpStdioCloseBounds,
    McpStdioLeaderOutcome, McpStdioWitnessOutcome,
};

use crate::unified_runtime::{McpCloseUnsettledClass, MemberMcpSettlement, MemberMcpUnsettled};

/// The longest one settlement can take: the stdio close bound of the members'
/// routers (Meta never sets one, so routers keep
/// [`McpStdioCloseBounds::DEFAULT`]) plus Meerkat's join allowance. A
/// settlement waits on each ticket's own deadline, which is at most this
/// after the settlement started: it is an upper bound, never a timer that
/// cuts a close. It is a bounded phase inside `UnifiedRuntime::shutdown`, and
/// again at a gateway's executor-exit boundary, so the advertised shutdown
/// horizon counts it twice. The exit boundary waits past a ticket's deadline
/// for its native outcome, which outlasts this budget only when Meerkat broke
/// its own close bound.
pub const MEMBER_MCP_SETTLEMENT_BUDGET: Duration = McpStdioCloseBounds::DEFAULT
    .close_bound()
    .saturating_add(McpCloseTicket::JOIN_ALLOWANCE);

/// One member adapter in custody whose own shutdown has not reported.
struct CustodyEntry {
    /// Kept, so its router's closes can still be started or joined here
    /// whatever happened to its release.
    adapter: Arc<McpRouterAdapter>,
    /// The adapter's one router shutdown, whoever starts it.
    shutdown: McpRouterShutdownObserver,
    /// Custody started that shutdown itself, as the adapter's last owner.
    shutdown_started: bool,
    /// A settlement reached the deadline Meerkat derived from this adapter's
    /// closes before they all reported: Meerkat broke its own bound, which
    /// counts under `deadline` whenever its outcomes are collected.
    missed_deadline: bool,
}

/// What a settlement does with a close whose native outcome has not reported
/// by the deadline Meerkat derived from it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PastDeadline {
    /// Leave it pending in custody for a later settlement: the runtime
    /// shutdown's own phase, which stays bounded.
    Leave,
    /// Keep waiting until its native outcome reports: the gateway's
    /// executor-exit boundary, after which no settlement runs.
    Join,
}

/// One adapter's close wait in a settlement.
enum CloseWait {
    /// The native outcomes reported, `late` when only after the deadline.
    Reported {
        records: Vec<McpServerCloseRecord>,
        late: bool,
    },
    /// Still pending at the deadline, and left in custody.
    Pending,
}

impl CustodyEntry {
    /// The adapter's own shutdown report, once it reported.
    fn reported(&self) -> Option<Arc<McpRouterShutdownReport>> {
        match self.shutdown.current() {
            Some(McpRouterShutdownObservation::Reported(report)) => Some(report),
            _ => None,
        }
    }
}

#[derive(Default)]
struct CustodyState {
    /// Adapters whose native outcomes have not reported.
    entries: Vec<CustodyEntry>,
    /// The routers whose native outcomes reported, and those outcomes by
    /// class. Under `deadline`, those of them that a settlement's deadline
    /// had already reached.
    reported: MemberMcpUnsettled,
    /// Set by the first settlement: no adapter is adopted after it, so its
    /// snapshot is every adapter that can own a server.
    frozen: bool,
}

impl CustodyState {
    /// Release every adapter whose own shutdown reported, keeping that
    /// shutdown's native outcomes, and start the shutdown of every adapter
    /// custody alone still owns.
    fn release_reported(&mut self) {
        let reported = &mut self.reported;
        self.entries.retain_mut(|entry| match entry.reported() {
            Some(report) => {
                reported.routers += 1;
                reported.deadline += usize::from(entry.missed_deadline);
                count_records(reported, report.servers());
                false
            }
            None => {
                if !entry.shutdown_started
                    && Arc::strong_count(&entry.adapter) == 1
                    && let Ok(runtime) = tokio::runtime::Handle::try_current()
                {
                    // Its owners dropped it without a release that reported
                    // (a panicked or aborted session skips it). The adapter's
                    // own shutdown joins one already under way.
                    let adapter = Arc::clone(&entry.adapter);
                    runtime.spawn(async move { adapter.shutdown().await });
                    entry.shutdown_started = true;
                }
                true
            }
        });
    }
}

/// A gateway's custody of its members' MCP router adapters.
///
/// Install it on every member factory with [`Self::install`], and hand it to
/// the runtime with
/// [`MobBootstrapSpec::with_member_mcp_custody`](crate::MobBootstrapSpec::with_member_mcp_custody).
/// It grants nothing: it holds closes the factory's adapters already own.
#[derive(Default)]
pub struct MemberMcpCustody {
    state: Mutex<CustodyState>,
    /// Serializes settlements: the runtime's own and the executor-exit one
    /// can overlap after a horizon cut-off.
    settling: tokio::sync::Mutex<()>,
    /// Test seam: every observation expires at once.
    #[cfg(test)]
    expire_observations: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for MemberMcpCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.lock();
        f.debug_struct("MemberMcpCustody")
            .field("unreported_adapters", &state.entries.len())
            .field("reported_routers", &state.reported.routers)
            .field("frozen", &state.frozen)
            .finish()
    }
}

impl MemberMcpCustody {
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// `factory`, adopting every MCP router adapter it constructs into this
    /// custody before any of its servers is staged.
    #[must_use]
    pub fn install(self: &Arc<Self>, factory: AgentFactory) -> AgentFactory {
        factory.with_mcp_adapter_custody(Arc::clone(self) as Arc<dyn McpAdapterCustody>)
    }

    /// Adapters ever adopted.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        let state = self.lock();
        state.entries.len() + state.reported.routers
    }

    /// Adapters in custody whose native outcomes have not reported.
    #[cfg(test)]
    pub(crate) fn adapters(&self) -> Vec<Arc<McpRouterAdapter>> {
        self.lock()
            .entries
            .iter()
            .map(|entry| Arc::clone(&entry.adapter))
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CustodyState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Make every later observation expire at once, as if Meerkat broke its
    /// own close bound: the closes it observes are still pending at their
    /// deadline.
    #[cfg(test)]
    pub(crate) fn expire_observations_for_test(&self) {
        self.expire_observations
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Freeze the custody, start the closes of every adapter in it whose own
    /// shutdown has not reported, and wait for each close's native outcome.
    ///
    /// Initiation waits behind nothing: [`McpRouterAdapter::begin_close`]
    /// starts every stdio close through its custody, without the router lock
    /// that in-flight dispatch holds, and joins a close already running. Each
    /// adapter's wait ends at its ticket's deadline, which Meerkat derives
    /// from the closes it started or joined, so no admitted close is cut
    /// before its native attempt reports; one still pending then stays in
    /// custody, counted under `deadline`. A settlement is safe to repeat: an
    /// adapter whose outcomes reported is counted from them, and one whose
    /// closes did not report is started (or joined) again. The gateway's
    /// executor-exit boundary relies on that, with [`Self::settle_to_exit`].
    pub(crate) async fn settle(&self) -> MemberMcpSettlement {
        self.settle_with(PastDeadline::Leave).await
    }

    /// The settlement at a gateway's executor-exit boundary: [`Self::settle`],
    /// except that a close still pending at its deadline is waited for on its
    /// own ticket until its native outcome reports. No settlement runs after
    /// this one, so no deadline may end its wait: the executor destroyed
    /// after it owns no pending close. Such a close counts in its native
    /// outcome's class, and its router under `deadline`, as the bound Meerkat
    /// broke. The wait outlasts the ticket's deadline only then.
    pub(crate) async fn settle_to_exit(&self) -> MemberMcpSettlement {
        self.settle_with(PastDeadline::Join).await
    }

    async fn settle_with(&self, past_deadline: PastDeadline) -> MemberMcpSettlement {
        let _settling = self.settling.lock().await;
        let pending: Vec<Arc<McpRouterAdapter>> = {
            let mut state = self.lock();
            state.frozen = true;
            state.release_reported();
            state
                .entries
                .iter()
                .map(|entry| Arc::clone(&entry.adapter))
                .collect()
        };
        // Every ticket is begun here, before the first await.
        let waits = futures::future::join_all(pending.iter().map(|adapter| {
            let ticket = adapter.begin_close();
            let deadline = ticket.deadline();
            #[cfg(test)]
            let deadline = if self
                .expire_observations
                .load(std::sync::atomic::Ordering::Acquire)
            {
                tokio::time::Instant::now()
            } else {
                deadline
            };
            async move {
                let mut outcomes = std::pin::pin!(ticket.outcomes());
                match tokio::time::timeout_at(deadline, outcomes.as_mut()).await {
                    Ok(records) => CloseWait::Reported {
                        records,
                        late: false,
                    },
                    Err(_) if past_deadline == PastDeadline::Join => {
                        tracing::warn!(
                            "a member MCP close had not reported by the deadline Meerkat derived \
                             from it; the executor-exit boundary waits for its native outcome"
                        );
                        CloseWait::Reported {
                            records: outcomes.await,
                            late: true,
                        }
                    }
                    Err(_) => CloseWait::Pending,
                }
            }
        }))
        .await;

        let mut state = self.lock();
        // Once frozen, only a settlement (serialized) changes the entries, so
        // they are still `pending`, in order.
        let mut unreported = Vec::new();
        for (mut entry, wait) in std::mem::take(&mut state.entries).into_iter().zip(waits) {
            match wait {
                CloseWait::Reported { records, late } => {
                    state.reported.routers += 1;
                    state.reported.deadline += usize::from(late || entry.missed_deadline);
                    count_records(&mut state.reported, &records);
                }
                CloseWait::Pending => {
                    entry.missed_deadline = true;
                    unreported.push(entry);
                }
            }
        }
        state.entries = unreported;
        let mut counts = state.reported.clone();
        // Meerkat broke its own bound: these adapters' closes had not all
        // reported by the deadline it derived from them, and still have not.
        counts.routers += state.entries.len();
        counts.deadline += state.entries.len();
        drop(state);
        if counts.routers == 0 {
            return MemberMcpSettlement::NothingOwned;
        }
        let settlement = MemberMcpSettlement::from_counts(counts);
        if !settlement.is_settled() {
            tracing::warn!(
                ?settlement,
                "member MCP servers did not settle before shutdown reported"
            );
        }
        settlement
    }
}

/// One member adapter's answer when the host reports a credential change for
/// an MCP target ([`MemberMcpCustody::credential_landed`],
/// [`MemberMcpCustody::credential_released`]).
#[derive(Debug)]
pub struct MemberMcpCredentialOutcome<T> {
    /// What the member's MCP router recorded for exactly that target.
    pub recorded: Result<T, meerkat::McpError>,
    /// The MCP boundaries the custody then applied in that member, in order,
    /// each with its native result ([`McpRouterAdapter::apply_staged`]);
    /// empty when the record left nothing to apply. It stops at the first
    /// that fails, and whatever that boundary did not apply stays staged in
    /// the member. Each is the member's whole MCP boundary: it also applies
    /// anything else staged there, such as the release of a server one of
    /// its calls found without a usable credential.
    pub applied: Vec<Result<meerkat::McpApplyResult, String>>,
}

impl MemberMcpCustody {
    /// After a successful MCP login for `target`, reconnect the server that
    /// awaits exactly that target in every member adapter this custody holds
    /// whose own shutdown has not reported, in the same runtime.
    ///
    /// Per adapter, in adoption order, the member's router stages the failed
    /// operation again with the config it attempted
    /// ([`McpRouterAdapter::retry_authorized`], the call Meerkat's own RPC
    /// and REST surfaces make after their completions), and the custody
    /// applies it at once. Those surfaces apply staged MCP changes at the
    /// start of each turn; nothing applies them in a gateway member after
    /// its construction, so the custody applies them as Meerkat's live MCP
    /// attachment applies an add. The reconnect then runs in the background
    /// and reads the stored credential itself (nothing is copied from the
    /// login); the member's agent sees the server's tools when it next polls
    /// its tools, between model calls. Pass the completion's `target`
    /// ([`meerkat::McpOAuthLoginComplete`]), which is scoped to the realm
    /// whose credential slot the login filled: the identity the member's
    /// connections report.
    ///
    /// [`MemberMcpCredentialOutcome::recorded`] is the router's
    /// [`meerkat::AuthorizedRetry`]:
    /// - `Staged`: staged, then applied by one boundary.
    /// - `AfterRelease`: a release of that server has not settled; the first
    ///   boundary settles it, and the second stages the server again with the
    ///   sign-in it kept and applies it.
    /// - `AlreadyPending`: a newer staged intent or an operation in flight
    ///   owns the server; one boundary applies whatever is staged.
    /// - `NotAwaiting`: nothing in that member awaits exactly this target.
    /// - `RouterClosed`: that member's router was shut down.
    ///
    /// The last two, and an `Err` (the router's typed refusal to stage it),
    /// apply nothing. Best effort per adapter: the login's success stands
    /// whatever a member answers.
    ///
    /// Each adapter's record and each of its boundaries take its router's
    /// write lock, which waits for every MCP call the member has in flight
    /// (a call holds the read lock until it returns), one adapter after
    /// another. So this can take as long as the longest of those calls:
    /// answer the browser's https callback first, and run this as owned work
    /// after it. A boundary also advances the member's MCP binding epoch: a
    /// call the member planned before it and dispatches after it is refused
    /// as unavailable (its execution owner changed), as after any live MCP
    /// change.
    pub async fn credential_landed(
        &self,
        target: &meerkat::McpServerIdentity,
    ) -> Vec<MemberMcpCredentialOutcome<meerkat::AuthorizedRetry>> {
        let adapters = self.live_adapters();
        let mut outcomes = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            let recorded = adapter.retry_authorized(target).await;
            let boundaries = match &recorded {
                Ok(meerkat::AuthorizedRetry::Staged | meerkat::AuthorizedRetry::AlreadyPending) => {
                    1
                }
                Ok(meerkat::AuthorizedRetry::AfterRelease) => 2,
                Ok(
                    meerkat::AuthorizedRetry::NotAwaiting | meerkat::AuthorizedRetry::RouterClosed,
                )
                | Err(_) => 0,
            };
            let applied = apply_boundaries(&adapter, boundaries).await;
            outcomes.push(MemberMcpCredentialOutcome { recorded, applied });
        }
        outcomes
    }

    /// After the credential for `target` is gone, stop offering the server
    /// that used exactly that credential in every member adapter this
    /// custody holds whose own shutdown has not reported, in the same
    /// runtime.
    ///
    /// Call it only when the credential itself is gone: after a successful
    /// [`meerkat::HostAuthService::mcp_logout`] for the target (on
    /// [`crate::gateway_mcp_composition::GatewayFactoryConstruction::host_auth_service`]),
    /// or after another removal of that credential from the realm's store.
    /// Never call it when a login attempt, its callback or a transport is
    /// only disposed of (cancelled, expired or closed): the member's server
    /// would then stop and wait for a sign-in its account does not need.
    /// Pass the identity the member's connections report, scoped to the
    /// realm whose credential slot it is,
    /// `host_auth_service()?.mcp_target_in_service_realm(&target)?`, as
    /// Meerkat's RPC logout does; a realm-less identity addresses no member.
    ///
    /// It never signs the account out. It does not log out, revoke a token,
    /// or remove or change a stored credential or account: a member's router
    /// holds its credential owner only as a [`meerkat::McpAuthResolver`],
    /// which reads a credential or starts a login and has no way to remove
    /// one, and neither the router nor this custody calls a logout. (The
    /// logout itself removes the stored credential and revokes nothing at
    /// the provider.)
    ///
    /// Per adapter, in adoption order, the member's router finds each
    /// connected server whose identity (server name, URL, account selection,
    /// and the realm its credential owner serves) equals `target` exactly,
    /// stages its removal and records it as awaiting a sign-in with the
    /// config it connected with ([`McpRouterAdapter::credential_released`],
    /// the call Meerkat's RPC logout makes), and the custody applies the
    /// member's MCP boundary at once, as [`Self::credential_landed`] does.
    /// The boundary removes the server: its connection closes (which ends
    /// its MCP session at the server, a transport action, not an account
    /// one) and its tools leave the member's catalog; the member's agent
    /// sees that when it next polls its tools. A later login and
    /// [`Self::credential_landed`] for the same target add the server back.
    ///
    /// A call in flight is not cancelled: the record waits for it to return.
    /// A request that a call sends once the credential has left the store,
    /// and any call the member makes before the boundary, finds no usable
    /// credential: for an account-selected target it is refused before
    /// anything is sent; for an unselected one it is sent without a
    /// credential and the server's `401` refuses it. Either way the member's
    /// tool call fails as awaiting authorization; it is never retried and
    /// never sent with the old credential.
    ///
    /// [`MemberMcpCredentialOutcome::recorded`] is the router's
    /// [`meerkat::ReleasedCredential`]:
    /// - `Unpublished`: a connected server used exactly that credential; its
    ///   removal was staged, then applied by one boundary.
    /// - `AlreadyPending`: a newer staged intent or an operation in flight
    ///   owns the server; one boundary applies whatever is staged.
    /// - `NotActive`: no connected server in that member uses exactly this
    ///   credential (absent, another URL, account or realm, or not
    ///   connected).
    /// - `RouterClosed`: that member's router was shut down.
    ///
    /// The last two, and an `Err` (the router's typed refusal to stage the
    /// removal), apply nothing. Calling it again for the same target finds
    /// the server already removed: `NotActive`, nothing applied.
    ///
    /// It takes the locks [`Self::credential_landed`] takes: each record and
    /// boundary waits for the member's in-flight MCP calls, one adapter after
    /// another, so run it as owned work, never inside a response the host
    /// owes; and a boundary refuses a call the member planned before it and
    /// dispatches after it.
    pub async fn credential_released(
        &self,
        target: &meerkat::McpServerIdentity,
    ) -> Vec<MemberMcpCredentialOutcome<meerkat::ReleasedCredential>> {
        let adapters = self.live_adapters();
        let mut outcomes = Vec::with_capacity(adapters.len());
        for adapter in adapters {
            let recorded = adapter.credential_released(target).await;
            let boundaries = match &recorded {
                Ok(
                    meerkat::ReleasedCredential::Unpublished
                    | meerkat::ReleasedCredential::AlreadyPending,
                ) => 1,
                Ok(
                    meerkat::ReleasedCredential::NotActive
                    | meerkat::ReleasedCredential::RouterClosed,
                )
                | Err(_) => 0,
            };
            let applied = apply_boundaries(&adapter, boundaries).await;
            outcomes.push(MemberMcpCredentialOutcome { recorded, applied });
        }
        outcomes
    }

    /// Every adapter this custody holds whose own shutdown has not reported,
    /// in adoption order.
    fn live_adapters(&self) -> Vec<Arc<McpRouterAdapter>> {
        self.lock()
            .entries
            .iter()
            .filter(|entry| entry.reported().is_none())
            .map(|entry| Arc::clone(&entry.adapter))
            .collect()
    }
}

/// Applies `boundaries` MCP boundaries in `adapter`, in order, stopping at
/// the first that fails.
async fn apply_boundaries(
    adapter: &McpRouterAdapter,
    boundaries: usize,
) -> Vec<Result<meerkat::McpApplyResult, String>> {
    let mut applied = Vec::with_capacity(boundaries);
    for _ in 0..boundaries {
        let boundary = adapter.apply_staged().await;
        let failed = boundary.is_err();
        applied.push(boundary);
        if failed {
            break;
        }
    }
    applied
}

impl McpAdapterCustody for MemberMcpCustody {
    fn adopt(&self, adapter: &Arc<McpRouterAdapter>) -> Result<(), McpAdapterCustodyClosed> {
        let mut state = self.lock();
        // A frozen custody refuses: the construction then fails at the seam,
        // before any server process exists, so nothing escapes the snapshot
        // a settlement took.
        if state.frozen {
            return Err(McpAdapterCustodyClosed);
        }
        // Adapters whose own shutdown reported since leave custody with their
        // outcomes, and an adapter custody alone still owns is shut down, so
        // custody keeps no adapter longer than its closes need.
        state.release_reported();
        state.entries.push(CustodyEntry {
            adapter: Arc::clone(adapter),
            shutdown: adapter.shutdown_observer(),
            shutdown_started: false,
            missed_deadline: false,
        });
        Ok(())
    }
}

/// The class `outcome` counts in, or `None` when it settled. Settled means
/// the final facts Meerkat reports ([`McpServerCloseOutcome::is_settled`]),
/// never a phase.
pub(crate) fn unsettled_class(outcome: &McpServerCloseOutcome) -> Option<McpCloseUnsettledClass> {
    if outcome.is_settled() {
        return None;
    }
    Some(match outcome {
        McpServerCloseOutcome::Stdio(receipt) => match (&receipt.leader, &receipt.witness) {
            (McpStdioLeaderOutcome::WaitFailed(_), _) => McpCloseUnsettledClass::WaitFailed,
            (McpStdioLeaderOutcome::NotReaped, _) => McpCloseUnsettledClass::NotReaped,
            (_, McpStdioWitnessOutcome::StillOpen) => McpCloseUnsettledClass::StdoutOpen,
            // Absent (off Unix included), ReadError, or a witness this
            // version cannot read: nothing showed that every stdout holder
            // closed it.
            _ => McpCloseUnsettledClass::StdoutUnobserved,
        },
        McpServerCloseOutcome::NotHeld(_) => McpCloseUnsettledClass::NotHeld,
        McpServerCloseOutcome::TaskFailed { .. } => McpCloseUnsettledClass::TaskFailed,
        // CloseFailed on any transport, or an outcome this version cannot
        // read.
        _ => McpCloseUnsettledClass::CloseFailed,
    })
}

/// Count every record once, in its class.
fn count_records(unsettled: &mut MemberMcpUnsettled, records: &[McpServerCloseRecord]) {
    for record in records {
        unsettled.servers += 1;
        match unsettled_class(&record.outcome) {
            None => {}
            Some(McpCloseUnsettledClass::WaitFailed) => unsettled.wait_failed += 1,
            Some(McpCloseUnsettledClass::NotReaped) => unsettled.not_reaped += 1,
            Some(McpCloseUnsettledClass::StdoutOpen) => unsettled.stdout_open += 1,
            Some(McpCloseUnsettledClass::StdoutUnobserved) => unsettled.stdout_unobserved += 1,
            Some(McpCloseUnsettledClass::NotHeld) => unsettled.not_held += 1,
            Some(McpCloseUnsettledClass::CloseFailed) => unsettled.close_failed += 1,
            Some(McpCloseUnsettledClass::TaskFailed) => unsettled.task_failed += 1,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn adapter() -> Arc<McpRouterAdapter> {
        Arc::new(McpRouterAdapter::new(
            meerkat_mcp::McpRouter::new_with_stdio_profile(
                meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
            ),
        ))
    }

    #[test]
    fn the_budget_is_the_default_close_bound_plus_the_join_allowance() {
        assert_eq!(
            MEMBER_MCP_SETTLEMENT_BUDGET,
            McpStdioCloseBounds::DEFAULT.close_bound() + McpCloseTicket::JOIN_ALLOWANCE
        );
        assert_eq!(MEMBER_MCP_SETTLEMENT_BUDGET, Duration::from_secs(20));
    }

    #[tokio::test]
    async fn an_empty_custody_owns_nothing() {
        let custody = MemberMcpCustody::new();
        assert_eq!(custody.settle().await, MemberMcpSettlement::NothingOwned);
    }

    /// An adapter its agent never released settles through its close
    /// ticket, and a later settlement (the executor-exit one) repeats the
    /// same outcome from what was retained.
    #[tokio::test]
    async fn an_unreleased_adapter_settles_and_settlement_repeats() {
        let custody = MemberMcpCustody::new();
        let adapter = adapter();
        custody.adopt(&adapter).expect("adopt");
        let settled = MemberMcpSettlement::Settled {
            routers: 1,
            servers: 0,
        };
        assert_eq!(custody.settle().await, settled);
        assert_eq!(custody.settle().await, settled);
    }

    /// Custody keeps an adapter its agent dropped unreleased, so its closes
    /// are still started and joined at settlement.
    #[tokio::test]
    async fn an_adapter_dropped_unreleased_stays_in_custody_and_settles() {
        let custody = MemberMcpCustody::new();
        let adapter = adapter();
        custody.adopt(&adapter).expect("adopt");
        drop(adapter);
        assert_eq!(custody.adapters().len(), 1);
        assert_eq!(
            custody.settle().await,
            MemberMcpSettlement::Settled {
                routers: 1,
                servers: 0
            }
        );
    }

    /// Once custody is an adapter's last owner and no shutdown reported, the
    /// next adoption starts that adapter's own shutdown, so custody never
    /// keeps its servers running past their owners.
    #[tokio::test]
    async fn custody_shuts_down_an_adapter_it_alone_still_owns() {
        let custody = MemberMcpCustody::new();
        let abandoned = adapter();
        let mut shutdown = abandoned.shutdown_observer();
        custody.adopt(&abandoned).expect("adopt");
        drop(abandoned);
        custody.adopt(&adapter()).expect("adopt");
        let observed = tokio::time::timeout(Duration::from_secs(10), shutdown.observe())
            .await
            .expect("custody started the abandoned adapter's shutdown");
        assert!(
            matches!(observed, McpRouterShutdownObservation::Reported(_)),
            "{observed:?}"
        );
        assert_eq!(
            custody.settle().await,
            MemberMcpSettlement::Settled {
                routers: 2,
                servers: 0
            }
        );
    }

    /// An adapter whose own shutdown reported leaves custody at the next
    /// adoption, and its native outcomes still count.
    #[tokio::test]
    async fn a_shut_down_adapter_leaves_custody_with_its_outcomes() {
        let custody = MemberMcpCustody::new();
        let released = adapter();
        custody.adopt(&released).expect("adopt");
        released.shutdown().await;
        let live = adapter();
        custody.adopt(&live).expect("adopt");
        assert_eq!(
            Arc::strong_count(&released),
            1,
            "custody released the shut-down adapter"
        );
        assert_eq!(custody.adapters().len(), 1);
        assert_eq!(custody.len(), 2);
        assert_eq!(
            custody.settle().await,
            MemberMcpSettlement::Settled {
                routers: 2,
                servers: 0
            }
        );
    }

    /// The first settlement freezes the custody: a later adoption is refused,
    /// so the construction fails before any server exists.
    #[tokio::test]
    async fn a_settled_custody_refuses_later_adoption() {
        let custody = MemberMcpCustody::new();
        assert_eq!(custody.settle().await, MemberMcpSettlement::NothingOwned);
        assert!(custody.adopt(&adapter()).is_err());
        assert_eq!(custody.len(), 0);
    }

    fn record(outcome: McpServerCloseOutcome) -> McpServerCloseRecord {
        McpServerCloseRecord {
            server: "fixture".to_string(),
            outcome,
        }
    }

    fn receipt(
        leader: McpStdioLeaderOutcome,
        witness: McpStdioWitnessOutcome,
    ) -> McpServerCloseOutcome {
        McpServerCloseOutcome::Stdio(Arc::new(meerkat_mcp::McpStdioCloseReceipt {
            settled_in: None,
            leader_exit: meerkat_mcp::McpStdioLeaderExitObservation::NotSeen,
            witness,
            sigterm: None,
            sigkill: None,
            final_group_signal: None,
            leader,
            elapsed: Duration::from_secs(1),
        }))
    }

    fn reaped() -> McpStdioLeaderOutcome {
        use std::os::unix::process::ExitStatusExt as _;
        McpStdioLeaderOutcome::Reaped(std::process::ExitStatus::from_raw(9))
    }

    /// Settled means the final facts: a reaped leader and a closed stdout
    /// witness, a process-free transport closed, or a server never spawned.
    /// Every other outcome counts in its own class, never as settled; a
    /// missing witness (off Unix too) is unobserved, not success, and only an
    /// actual close error is `close_failed`.
    #[test]
    fn every_unsettled_outcome_counts_in_its_own_class() {
        let kill = meerkat_mcp::McpStdioClosePhase::Kill;
        let records = vec![
            record(receipt(reaped(), McpStdioWitnessOutcome::Closed(kill))),
            record(McpServerCloseOutcome::NoProcess),
            record(McpServerCloseOutcome::NeverSpawned),
            record(receipt(
                McpStdioLeaderOutcome::WaitFailed(std::io::Error::other("reaped elsewhere")),
                McpStdioWitnessOutcome::Closed(kill),
            )),
            record(receipt(
                McpStdioLeaderOutcome::NotReaped,
                McpStdioWitnessOutcome::Closed(kill),
            )),
            record(receipt(reaped(), McpStdioWitnessOutcome::StillOpen)),
            record(receipt(reaped(), McpStdioWitnessOutcome::Absent)),
            record(receipt(
                reaped(),
                McpStdioWitnessOutcome::ReadError {
                    phase: kill,
                    kind: std::io::ErrorKind::Other,
                },
            )),
            record(McpServerCloseOutcome::NotHeld(
                meerkat_mcp::McpCustodyNotHeld::AbandonedElsewhere,
            )),
            record(McpServerCloseOutcome::TaskFailed { panicked: true }),
            record(McpServerCloseOutcome::CloseFailed(Arc::new(
                meerkat_mcp::McpError::ConnectionFailed {
                    reason: "cancel did not finish".to_string(),
                },
            ))),
        ];
        let mut unsettled = MemberMcpUnsettled::default();
        count_records(&mut unsettled, &records);
        assert_eq!(unsettled.servers, 11);
        assert_eq!(
            (
                unsettled.wait_failed,
                unsettled.not_reaped,
                unsettled.stdout_open,
                unsettled.stdout_unobserved,
                unsettled.not_held,
                unsettled.task_failed,
                unsettled.close_failed,
                unsettled.deadline,
            ),
            (1, 1, 1, 2, 1, 1, 1, 0)
        );
    }

    #[cfg(target_os = "linux")]
    mod with_servers {
        use std::sync::Arc;
        use std::time::Duration;

        use meerkat::{AgentFactory, FactoryAgentBuilder};
        use meerkat_core::AgentToolDispatcher as _;
        use meerkat_session::SessionAgentBuilder as _;
        use tokio::task::JoinHandle;

        use super::super::*;
        use crate::test_mcp_fixture::{
            FixtureMode, McpFixture, ProcessState, process_state, recorded_pid,
            runtime_with_member_server, wait_for_recorded_pid,
        };

        const STARTUP: Duration = Duration::from_mins(1);

        fn request_with(
            servers: Vec<meerkat_core::McpServerConfig>,
        ) -> meerkat_core::service::CreateSessionRequest {
            let build = meerkat_core::service::SessionBuildOptions {
                mcp_servers: servers,
                ..Default::default()
            };
            meerkat_core::service::CreateSessionRequest {
                model: "mock-model".to_string(),
                prompt: meerkat_core::ContentInput::Text("noop".to_string()),
                system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                build: Some(build),
                labels: None,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                injected_context: Vec::new(),
            }
        }

        fn builder(factory: AgentFactory) -> FactoryAgentBuilder {
            let mut builder = FactoryAgentBuilder::new(factory, meerkat::Config::default());
            builder.default_llm_client = Some(Arc::new(meerkat_client::TestClient::default()));
            builder
        }

        struct Refusing;

        impl meerkat::McpAdapterComposer for Refusing {
            fn compose(
                &self,
                _adapter: &Arc<McpRouterAdapter>,
                _session_id: &meerkat_core::types::SessionId,
                _member: Option<&meerkat_core::MobMemberBinding>,
            ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, meerkat::McpAdapterComposeRefusal>
            {
                Err(meerkat::McpAdapterComposeRefusal::new(
                    "refused for the test",
                ))
            }
        }

        /// A construction that fails after its adapter was adopted (here the
        /// host composer refuses, after staging) is still in custody, and
        /// settles with its native outcomes.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_construction_failing_after_adoption_is_settled_in_custody() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let factory = custody.install(
                AgentFactory::new(fixture.dir.path().join("sessions"))
                    .with_mcp_adapter_composer(Arc::new(Refusing)),
            );
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            let built = builder(factory)
                .build_agent(
                    &request_with(vec![fixture.server("stubborn", FixtureMode::Stubborn)]),
                    event_tx,
                )
                .await;
            assert!(built.is_err(), "the host composer refused the build");
            assert_eq!(custody.len(), 1, "the adapter was adopted before staging");
            let settlement = custody.settle().await;
            assert!(settlement.is_settled(), "{settlement:?}");
            if let Some(pid) = recorded_pid(&fixture.pid_path()) {
                assert_eq!(process_state(pid), ProcessState::Gone);
            }
        }

        /// Adoption after the freeze is refused at the seam, so the build
        /// fails before any server process exists.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_build_after_the_freeze_fails_before_any_server_starts() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            assert_eq!(custody.settle().await, MemberMcpSettlement::NothingOwned);
            let factory = custody.install(AgentFactory::new(fixture.dir.path().join("sessions")));
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            let built = builder(factory)
                .build_agent(
                    &request_with(vec![fixture.server("stubborn", FixtureMode::Stubborn)]),
                    event_tx,
                )
                .await;
            assert!(built.is_err(), "a frozen custody refuses the construction");
            tokio::time::sleep(Duration::from_millis(500)).await;
            assert_eq!(
                recorded_pid(&fixture.pid_path()),
                None,
                "no server process was started"
            );
        }

        /// Start a call on the custody's adapter that its `HangCall` server
        /// never answers, and prove the call holds the router's read guard:
        /// the call reached the server (its marker is written), it has not
        /// returned, and a writer queued after it cannot enter. The only path
        /// from a dispatch to the server holds `router.read()` across the call
        /// (`McpRouterAdapter::dispatch_with_context`). Returns the call (true
        /// when it succeeded) and the queued writer.
        async fn hold_the_router(
            fixture: &McpFixture,
            custody: &MemberMcpCustody,
        ) -> (JoinHandle<bool>, JoinHandle<()>) {
            let adapter = custody
                .adapters()
                .into_iter()
                .next()
                .expect("the adapter is in custody");
            // Nothing else drives a pending connection here: settle it, so the
            // server's tool is listed.
            adapter
                .wait_until_ready(STARTUP)
                .await
                .expect("the server connected");
            let tool = adapter
                .tools()
                .first()
                .map(|tool| tool.name.to_string())
                .expect("the server's tool is listed");
            let call = {
                let adapter = Arc::clone(&adapter);
                tokio::spawn(async move {
                    let args =
                        serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
                    meerkat_core::AgentToolDispatcher::dispatch(
                        &*adapter,
                        meerkat_core::ToolCallView {
                            id: "call-1",
                            name: &tool,
                            args: &args,
                        },
                    )
                    .await
                    .is_ok()
                })
            };
            wait_for_recorded_pid(&fixture.call_marker_path(), STARTUP)
                .await
                .expect("the call reached the server");
            let writer = {
                let adapter = Arc::clone(&adapter);
                tokio::spawn(async move {
                    let _ = adapter.poll_lifecycle_actions().await;
                })
            };
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(
                !writer.is_finished(),
                "a writer is held off: the call holds the router's read guard"
            );
            assert!(
                !call.is_finished(),
                "the call is still in flight when the close starts"
            );
            (call, writer)
        }

        /// The held call ends with an error once its server is closed, and
        /// the queued writer then enters.
        async fn assert_released(call: JoinHandle<bool>, writer: JoinHandle<()>) {
            let call_succeeded = tokio::time::timeout(STARTUP, call)
                .await
                .expect("the held call ends once its server is closed")
                .expect("the call task");
            assert!(!call_succeeded, "the closed server's call fails");
            tokio::time::timeout(STARTUP, writer)
                .await
                .expect("the writer enters once the call released the router")
                .expect("the writer task");
        }

        /// Delayed initiation at the custody: while a call holds the router's
        /// read guard, settlement still starts the close at once; the close
        /// takes its full escalation (the server ignores EOF and SIGTERM) and
        /// settles within the budget; the held call then fails.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_close_starts_while_a_dispatch_holds_the_router() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let factory = custody.install(AgentFactory::new(fixture.dir.path().join("sessions")));
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            let _built = builder(factory)
                .build_agent(
                    &request_with(vec![fixture.server("hanging", FixtureMode::HangCall)]),
                    event_tx,
                )
                .await
                .expect("build");
            let pid = wait_for_recorded_pid(&fixture.pid_path(), STARTUP)
                .await
                .expect("the server started");
            let (call, writer) = hold_the_router(&fixture, &custody).await;
            let started = tokio::time::Instant::now();
            let settlement = custody.settle().await;
            assert_eq!(
                settlement,
                MemberMcpSettlement::Settled {
                    routers: 1,
                    servers: 1
                }
            );
            assert!(started.elapsed() <= MEMBER_MCP_SETTLEMENT_BUDGET);
            assert_eq!(process_state(pid), ProcessState::Gone);
            assert_released(call, writer).await;
        }

        /// Delayed initiation at the runtime: a member's call holds its
        /// router's read guard when the runtime shuts down. The runtime's
        /// teardown cannot release that guard (the call is not the member's
        /// turn), so the close starts under it, from the member's release or
        /// the settlement, whichever begins it first (neither takes the
        /// router lock). Shutdown reports the server settled and cleanup
        /// complete only once the native attempt reported it reaped with its
        /// stdout closed.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn shutdown_settles_a_member_server_whose_call_holds_the_router() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let (runtime, pid) =
                runtime_with_member_server(&fixture, FixtureMode::HangCall, &custody).await;
            let (call, writer) = hold_the_router(&fixture, &custody).await;
            let report = runtime.shutdown().await;
            assert!(
                matches!(
                    report.member_mcp_settlement,
                    MemberMcpSettlement::Settled { servers: 1, .. }
                ),
                "{:?}",
                report.member_mcp_settlement
            );
            assert!(report.cleanup_completed(), "{report:?}");
            assert_eq!(process_state(pid), ProcessState::Gone);
            assert_released(call, writer).await;
        }

        /// The counterexample at the runtime: a server that ignores EOF and
        /// SIGTERM outlives the 5 s member teardown. Shutdown now reports
        /// complete cleanup only once the native attempt reported the server
        /// reaped and its stdout closed.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn shutdown_reports_a_stubborn_member_server_settled_and_reaped() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let (runtime, pid) =
                runtime_with_member_server(&fixture, FixtureMode::Stubborn, &custody).await;
            let report = runtime.shutdown().await;
            assert!(
                matches!(
                    report.member_mcp_settlement,
                    MemberMcpSettlement::Settled { servers: 1, .. }
                ),
                "{:?}",
                report.member_mcp_settlement
            );
            assert!(report.cleanup_completed(), "{report:?}");
            assert_eq!(process_state(pid), ProcessState::Gone);
        }

        /// A holder of the server's stdout outside its group keeps the close
        /// from settling. The native attempt runs its whole bound and returns
        /// `stdout_open` (never `deadline`), with the direct leader reaped. No
        /// claim is made about any other descendant.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn shutdown_reports_an_escaped_stdout_holder_from_the_native_attempt() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let (runtime, pid) =
                runtime_with_member_server(&fixture, FixtureMode::Escaping, &custody).await;
            let escaped = wait_for_recorded_pid(&fixture.escaped_pid_path(), STARTUP)
                .await
                .expect("the stdout holder started");
            let started = tokio::time::Instant::now();
            let report = runtime.shutdown().await;
            let MemberMcpSettlement::Unsettled(unsettled) = &report.member_mcp_settlement else {
                panic!("an escaped stdout holder settled: {report:?}");
            };
            assert_eq!(
                (
                    unsettled.servers,
                    unsettled.stdout_open,
                    unsettled.deadline,
                    unsettled.not_reaped,
                    unsettled.wait_failed,
                ),
                (1, 1, 0, 0, 0),
                "{unsettled:?}"
            );
            assert!(
                started.elapsed() >= McpStdioCloseBounds::DEFAULT.close_bound(),
                "the native attempt reported only after its whole bound"
            );
            assert!(!report.cleanup_completed());
            assert_eq!(process_state(pid), ProcessState::Gone);
            assert!(matches!(process_state(escaped), ProcessState::Live(_)));
        }

        /// A deadline ends an observation, never the exit boundary's wait.
        /// With every observation expiring at once, as if Meerkat broke its
        /// own bound, the runtime's settlement leaves the stubborn server's
        /// close pending under `deadline`. The executor-exit settlement then
        /// waits on that same close until its native outcome reports the
        /// server reaped, counting the router under `deadline` once, and a
        /// later settlement repeats that outcome.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn the_exit_settlement_waits_past_the_deadline_for_the_native_outcome() {
            let fixture = McpFixture::new();
            let custody = MemberMcpCustody::new();
            let factory = custody.install(AgentFactory::new(fixture.dir.path().join("sessions")));
            let (event_tx, _event_rx) = tokio::sync::mpsc::channel(8);
            let _built = builder(factory)
                .build_agent(
                    &request_with(vec![fixture.server("stubborn", FixtureMode::Stubborn)]),
                    event_tx,
                )
                .await
                .expect("build");
            let pid = wait_for_recorded_pid(&fixture.pid_path(), STARTUP)
                .await
                .expect("the server started");
            custody.expire_observations_for_test();
            let pending = MemberMcpUnsettled {
                routers: 1,
                deadline: 1,
                ..MemberMcpUnsettled::default()
            };
            assert_eq!(
                custody.settle().await,
                MemberMcpSettlement::Unsettled(pending)
            );
            assert!(
                matches!(process_state(pid), ProcessState::Live(_)),
                "the close is still in its native attempt"
            );
            let joined = MemberMcpUnsettled {
                routers: 1,
                servers: 1,
                deadline: 1,
                ..MemberMcpUnsettled::default()
            };
            assert_eq!(
                custody.settle_to_exit().await,
                MemberMcpSettlement::Unsettled(joined.clone())
            );
            assert_eq!(process_state(pid), ProcessState::Gone);
            assert_eq!(
                custody.settle().await,
                MemberMcpSettlement::Unsettled(joined)
            );
        }
    }

    /// A streamable-HTTP MCP endpoint that serves one tool, `echo`, only to
    /// requests carrying `Bearer member-token`; any other request is refused
    /// with `401`.
    async fn authorized_endpoint() -> String {
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        async fn mcp(headers: HeaderMap, body: axum::body::Bytes) -> axum::response::Response {
            if headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                != Some("Bearer member-token")
            {
                return (StatusCode::UNAUTHORIZED, [("www-authenticate", "Bearer")])
                    .into_response();
            }
            let request: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            let Some(id) = request.get("id").cloned() else {
                return StatusCode::ACCEPTED.into_response();
            };
            let result = match request["method"].as_str() {
                Some("initialize") => serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "member-mcp", "version": "0.1.0" },
                }),
                Some("tools/list") => serde_json::json!({
                    "tools": [{
                        "name": "echo",
                        "description": "Echo input",
                        "inputSchema": { "type": "object", "properties": {} },
                    }]
                }),
                _ => serde_json::json!({}),
            };
            axum::Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                .into_response()
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}/mcp", listener.local_addr().expect("addr"));
        let app = axum::Router::new().route("/mcp", axum::routing::post(mcp));
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        url
    }

    /// The native credential owner of a host without a browser: the test
    /// stores and removes the credential (a login and a logout), an
    /// interactive login needs a human (and is counted), and it serves
    /// `realm`'s credential slot when it has one.
    struct StoredCredential {
        token: Mutex<Option<String>>,
        realm: Option<meerkat_core::RealmId>,
        logins: std::sync::atomic::AtomicUsize,
    }

    impl StoredCredential {
        fn new(token: Option<&str>) -> Arc<Self> {
            Self::serving(token, None)
        }

        fn in_realm(token: Option<&str>, realm: meerkat_core::RealmId) -> Arc<Self> {
            Self::serving(token, Some(realm))
        }

        fn serving(token: Option<&str>, realm: Option<meerkat_core::RealmId>) -> Arc<Self> {
            Arc::new(Self {
                token: Mutex::new(token.map(str::to_owned)),
                realm,
                logins: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        fn set(&self, token: Option<&str>) {
            *self.token.lock().unwrap_or_else(PoisonError::into_inner) = token.map(str::to_owned);
        }

        fn stored(&self) -> Option<String> {
            self.token
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }

        fn logins(&self) -> usize {
            self.logins.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl meerkat_mcp::McpAuthResolver for StoredCredential {
        async fn stored_bearer_token(
            &self,
            _target: &meerkat::McpServerIdentity,
        ) -> Result<Option<String>, meerkat::McpOAuthError> {
            Ok(self.stored())
        }
        async fn interactive_login(
            &self,
            target: &meerkat::McpServerIdentity,
            _www_authenticate: Option<&str>,
        ) -> Result<String, meerkat::McpOAuthError> {
            self.logins
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(meerkat::McpOAuthError::HumanAuthorizationRequired {
                server_name: target.server_name().to_owned(),
            })
        }
        fn credential_realm(&self) -> Option<meerkat_core::RealmId> {
            self.realm.clone()
        }
    }

    /// The configured `guarded` server at `url`, for `account` when one is
    /// named.
    fn guarded_config(url: &str, account: Option<&str>) -> meerkat_core::McpServerConfig {
        let mut config = meerkat_core::McpServerConfig::streamable_http(
            "guarded",
            url.to_owned(),
            std::collections::HashMap::new(),
        );
        if let meerkat_core::mcp_config::McpTransportConfig::Http(http) = &mut config.transport {
            http.oauth_account = account.map(str::to_owned);
        }
        config
    }

    /// A member adapter for the `guarded` server at `url` under `credential`,
    /// once its first connect settled: connected while a credential is
    /// stored, else waiting for a login. Returns the adapter and the identity
    /// its connection reports (scoped to the credential owner's realm).
    async fn member_adapter(
        url: String,
        credential: Arc<StoredCredential>,
    ) -> (Arc<McpRouterAdapter>, meerkat::McpServerIdentity) {
        member_adapter_for(guarded_config(&url, None), credential).await
    }

    async fn member_adapter_for(
        config: meerkat_core::McpServerConfig,
        credential: Arc<StoredCredential>,
    ) -> (Arc<McpRouterAdapter>, meerkat::McpServerIdentity) {
        let configured = meerkat::McpServerIdentity::from_config(&config).expect("identity");
        let target = match &credential.realm {
            Some(realm) => configured.in_realm(realm.clone()),
            None => configured,
        };
        let connects = credential.stored().is_some();
        let mut router = meerkat_mcp::McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
        )
        .with_mcp_auth(meerkat::McpAuthMode::Interactive, Some(credential));
        router.stage_add(config).expect("stage add");
        router.apply_staged().await.expect("apply staged add");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let _ = router.take_external_updates();
            let settled = if connects {
                router.active_server_names() == vec!["guarded".to_string()]
            } else {
                router.servers_awaiting_authorization() == vec![target.clone()]
            };
            if settled {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the first connect never settled"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        (Arc::new(McpRouterAdapter::new(router)), target)
    }

    /// Polls `member` as its agent loop does between model calls
    /// (`poll_external_updates`, which drains settled connects and applies
    /// nothing) until exactly `servers` are active.
    async fn until_member_serves(member: &McpRouterAdapter, servers: &[&str]) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let _ = meerkat_core::AgentToolDispatcher::poll_external_updates(member).await;
            if member.active_server_names().await == servers {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the member never served {servers:?}: {:?}",
                member.active_server_names().await
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Whether `member`'s current catalog offers `tool`.
    fn offers(member: &McpRouterAdapter, tool: &str) -> bool {
        meerkat_core::AgentToolDispatcher::tools(member)
            .iter()
            .any(|offered| offered.name == tool)
    }

    /// A gateway member gets no MCP boundary after its construction: its
    /// agent loop only drains settled connects. So a credential landed
    /// through the custody reconnects the server that awaited it with no
    /// other boundary, and the member's catalog offers its tool again.
    #[tokio::test]
    async fn a_landed_credential_reconnects_the_member_without_another_boundary() {
        let credential = StoredCredential::new(None);
        let (member, target) =
            member_adapter(authorized_endpoint().await, Arc::clone(&credential)).await;
        let custody = MemberMcpCustody::new();
        custody.adopt(&member).expect("adopted");
        assert!(member.active_server_names().await.is_empty());
        assert!(!offers(&member, "echo"));

        // The login stored the credential; the host reports it landed.
        credential.set(Some("member-token"));
        let _ = custody.credential_landed(&target).await;

        until_member_serves(&member, &["guarded"]).await;
        assert!(offers(&member, "echo"));
        assert!(member.servers_awaiting_authorization().await.is_empty());
        member.shutdown().await;
    }

    /// Each member's recorded outcome and whether each boundary the custody
    /// applied after it succeeded.
    fn answers<T: Copy>(outcomes: &[MemberMcpCredentialOutcome<T>]) -> Vec<(Option<T>, Vec<bool>)> {
        outcomes
            .iter()
            .map(|outcome| {
                (
                    outcome.recorded.as_ref().ok().copied(),
                    outcome.applied.iter().map(Result::is_ok).collect(),
                )
            })
            .collect()
    }

    /// A credential landed through the custody answers for every member
    /// adapter it holds, in adoption order, and applies a boundary only where
    /// the record staged something: the member awaiting exactly that target
    /// is staged and applied, an idle member is untouched, a second landing
    /// after the reconnect finds nothing awaiting, and an adapter whose own
    /// shutdown reported is no longer addressed.
    #[tokio::test]
    async fn a_landed_credential_answers_per_member_and_applies_what_it_staged() {
        use meerkat::AuthorizedRetry::{NotAwaiting, Staged};
        let credential = StoredCredential::new(None);
        let (member, target) =
            member_adapter(authorized_endpoint().await, Arc::clone(&credential)).await;
        let idle = adapter();
        let custody = MemberMcpCustody::new();
        custody.adopt(&member).expect("adopted");
        custody.adopt(&idle).expect("adopted");

        credential.set(Some("member-token"));
        let landed = custody.credential_landed(&target).await;
        assert_eq!(
            answers(&landed),
            vec![(Some(Staged), vec![true]), (Some(NotAwaiting), vec![])],
            "{landed:?}"
        );
        until_member_serves(&member, &["guarded"]).await;

        let again = custody.credential_landed(&target).await;
        assert_eq!(
            answers(&again),
            vec![(Some(NotAwaiting), vec![]), (Some(NotAwaiting), vec![])],
            "{again:?}"
        );

        member.shutdown().await;
        let after = custody.credential_landed(&target).await;
        assert_eq!(
            answers(&after),
            vec![(Some(NotAwaiting), vec![])],
            "only the live adapter is addressed: {after:?}"
        );
        assert!(
            MemberMcpCustody::new()
                .credential_landed(&target)
                .await
                .is_empty(),
            "an empty custody addresses nothing"
        );
    }

    /// A credential released through the custody after its logout removes,
    /// at once and with no other boundary, the member's server that used
    /// exactly it: the member's catalog no longer offers its tool and the
    /// server awaits a sign-in to exactly its target. Members serve their
    /// realm's credential slot, so only the realm-scoped identity addresses
    /// them. A later login and landing add the server back.
    #[tokio::test]
    async fn a_released_credential_removes_the_members_server_at_once() {
        use meerkat::AuthorizedRetry::Staged;
        use meerkat::ReleasedCredential::{NotActive, Unpublished};
        let url = authorized_endpoint().await;
        let realm = meerkat_core::RealmId::parse("gateway-realm").expect("realm");
        let credential = StoredCredential::in_realm(Some("member-token"), realm);
        let (member, target) = member_adapter(url.clone(), Arc::clone(&credential)).await;
        let realm_less =
            meerkat::McpServerIdentity::from_config(&guarded_config(&url, None)).expect("identity");
        let custody = MemberMcpCustody::new();
        custody.adopt(&member).expect("adopted");
        assert!(offers(&member, "echo"));

        // The logout removed the credential; the host reports it.
        credential.set(None);
        let unscoped = custody.credential_released(&realm_less).await;
        assert_eq!(
            answers(&unscoped),
            vec![(Some(NotActive), vec![])],
            "a realm-less identity addresses no member: {unscoped:?}"
        );
        assert!(offers(&member, "echo"));
        let released = custody.credential_released(&target).await;
        assert_eq!(
            answers(&released),
            vec![(Some(Unpublished), vec![true])],
            "{released:?}"
        );
        assert!(member.active_server_names().await.is_empty());
        assert!(!offers(&member, "echo"));
        assert_eq!(
            member.servers_awaiting_authorization().await,
            vec![target.clone()]
        );

        // A later login stores a credential again and the host lands it.
        credential.set(Some("member-token"));
        let landed = custody.credential_landed(&target).await;
        assert_eq!(
            answers(&landed),
            vec![(Some(Staged), vec![true])],
            "{landed:?}"
        );
        until_member_serves(&member, &["guarded"]).await;
        assert!(offers(&member, "echo"));
        member.shutdown().await;
    }

    /// A credential released through the custody answers for every member
    /// adapter it holds, in adoption order, and applies a boundary only where
    /// the record staged something: another URL's credential touches
    /// nothing, the member using exactly that credential is unpublished and
    /// applied, a second release finds the server already removed, and an
    /// adapter whose own shutdown reported is no longer addressed.
    #[tokio::test]
    async fn a_released_credential_answers_per_member_and_applies_what_it_staged() {
        use meerkat::ReleasedCredential::{NotActive, Unpublished};
        let credential = StoredCredential::new(Some("member-token"));
        let (member, target) =
            member_adapter(authorized_endpoint().await, Arc::clone(&credential)).await;
        let idle = adapter();
        let custody = MemberMcpCustody::new();
        custody.adopt(&member).expect("adopted");
        custody.adopt(&idle).expect("adopted");
        credential.set(None);

        let elsewhere =
            meerkat::McpServerIdentity::from_server_config("guarded", "http://127.0.0.1:9/mcp");
        let untouched = custody.credential_released(&elsewhere).await;
        assert_eq!(
            answers(&untouched),
            vec![(Some(NotActive), vec![]), (Some(NotActive), vec![])],
            "{untouched:?}"
        );
        assert_eq!(
            member.active_server_names().await,
            vec!["guarded".to_string()]
        );

        let released = custody.credential_released(&target).await;
        assert_eq!(
            answers(&released),
            vec![(Some(Unpublished), vec![true]), (Some(NotActive), vec![])],
            "{released:?}"
        );
        let again = custody.credential_released(&target).await;
        assert_eq!(
            answers(&again),
            vec![(Some(NotActive), vec![]), (Some(NotActive), vec![])],
            "a second release finds the server already removed: {again:?}"
        );

        member.shutdown().await;
        let after = custody.credential_released(&target).await;
        assert_eq!(
            answers(&after),
            vec![(Some(NotActive), vec![])],
            "only the live adapter is addressed: {after:?}"
        );
        assert!(
            MemberMcpCustody::new()
                .credential_released(&target)
                .await
                .is_empty(),
            "an empty custody addresses nothing"
        );
    }

    /// A release only tells the members; it never signs the account out.
    /// Reported while the credential is still stored (as if a ceremony's
    /// disposal were mistaken for a logout), it removes the member's server,
    /// but the stored credential and the account it was stored for stay and
    /// no login is asked: landing the same account's target reconnects the
    /// server with that very credential.
    #[tokio::test]
    async fn a_release_never_signs_the_account_out() {
        use meerkat::AuthorizedRetry::Staged;
        use meerkat::ReleasedCredential::Unpublished;
        let credential = StoredCredential::new(Some("member-token"));
        let (member, target) = member_adapter_for(
            guarded_config(&authorized_endpoint().await, Some("subject-7")),
            Arc::clone(&credential),
        )
        .await;
        assert!(target.is_selected(), "the target names its account");
        let custody = MemberMcpCustody::new();
        custody.adopt(&member).expect("adopted");

        let released = custody.credential_released(&target).await;
        assert_eq!(
            answers(&released),
            vec![(Some(Unpublished), vec![true])],
            "{released:?}"
        );
        assert!(member.active_server_names().await.is_empty());
        // The stored credential and its account are untouched, and nothing
        // asked for a login.
        assert_eq!(credential.stored().as_deref(), Some("member-token"));
        assert_eq!(credential.logins(), 0);
        assert_eq!(
            member.servers_awaiting_authorization().await,
            vec![target.clone()],
            "the server waits for exactly that account's target"
        );

        let landed = custody.credential_landed(&target).await;
        assert_eq!(
            answers(&landed),
            vec![(Some(Staged), vec![true])],
            "{landed:?}"
        );
        // The endpoint serves only the stored credential: the reconnect used it.
        until_member_serves(&member, &["guarded"]).await;
        assert_eq!(credential.stored().as_deref(), Some("member-token"));
        assert_eq!(credential.logins(), 0, "no login was asked");
        member.shutdown().await;
    }
}
