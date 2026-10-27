//! Mob member operations on `UnifiedRuntime`.
//!
//! Kept intentionally small: a pair of accessors (`mob_handle`, `mob_runtime`)
//! and hook-aware variants of `spawn` / `spawn_many` (which fire `post_spawn_hook`
//! and report errors via the shared error hook). All other member-lifecycle
//! operations — status, discover, reconcile, retire, helpers, etc. — are now
//! on `MobHandle` directly; callers reach through `runtime.mob_handle()`.

use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_mob::{
    MemberTurnEventSender, MemberTurnHandle, MemberTurnOptions, MobError, MobHandle,
    SpawnMemberSpec, SpawnResult,
};
use std::future::Future;

use crate::mob_handle_runtime::MobRuntimeError;

use super::UnifiedRuntime;

// Upstream routed runtime-ready signals use a bounded actor queue with
// fail-fast enqueue in meerkat-mob 0.6.x. Keep bulk discovery bootstrap
// serialized until the upstream signal path is backpressured.
const MAX_CONCURRENT_SPAWN_MANY: usize = 1;

/// Canonical admission for a completion-bearing member turn.
///
/// `session_id` identifies the exact bridge session captured with the
/// admitted member runtime binding. Hosts can start correlated history reads
/// immediately while concurrently selecting over their event receiver and
/// [`MemberTurnHandle::wait`].
#[derive(Debug)]
pub struct MemberTurnAdmission {
    /// Exact bridge session captured with the member runtime binding.
    pub session_id: String,
    /// Completion-bearing turn; select its event receiver alongside `wait()`.
    pub turn: MemberTurnHandle,
}

/// Default ceiling for `meta/wait_ready` when the caller omits `timeout_ms`.
///
/// meerkat-mob 0.7.9 (#798, reactive-readiness redesign) lowered its own
/// internal `DEFAULT_READY_WAIT_TIMEOUT` from 600s to 60s; that default is
/// applied inside `MobHandle::wait_for_ready` whenever the caller passes
/// `None`. meta's SDK contract for `wait_ready` is "wait until the mob is
/// ready", so a caller that omits a timeout keeps the prior generous ceiling
/// rather than silently inheriting meerkat's lowered 60s — the reactive wait
/// still returns promptly when members converge, so this is only the safety
/// wall for genuinely slow startups. Pass an explicit `timeout_ms` to override.
pub(crate) const DEFAULT_WAIT_READY_TIMEOUT: std::time::Duration =
    std::time::Duration::from_mins(10);

/// Whether a `MobHandle::wait_for_ready` error is a readiness-deadline timeout
/// (which maps to the documented `{ ready: [], timeout: true }` envelope) as
/// opposed to a genuine failure (which surfaces as an RPC error).
///
/// Matches the typed variant rather than the Display string: meerkat-mob's
/// [`MobError::ReadyWaitTimedOut`] renders as `"member ready wait timed out"`,
/// which does NOT contain the substring `"timeout"` (only `"timed out"`), so
/// the previous `message.to_lowercase().contains("timeout")` check always fell
/// through to the error branch and returned `-32000` instead of the envelope.
pub(crate) fn is_ready_wait_timeout(err: &MobError) -> bool {
    matches!(err, MobError::ReadyWaitTimedOut { .. })
}

impl UnifiedRuntime {
    /// Whether this launch speaks for the durable mob composition.
    ///
    /// Delegates to the mob runtime. Surfaces need it to refuse durable-state
    /// authorship from a certification pass.
    #[must_use]
    pub fn speaks_for_composition(&self) -> bool {
        self.mob_runtime.speaks_for_composition()
    }

    /// A public, owning handle to the mob: while it (or any clone) lives,
    /// the mob stays publicly owned and is never closed for idleness.
    pub fn mob_handle(&self) -> MobHandle {
        self.mob_runtime().handle()
    }

    /// Core's members it holds Broken, each with Core's reason: the members
    /// it could not restore at this start (a restored member whose comms
    /// identity is missing on this host, say), which stay broken until they
    /// are repaired. A host's readiness probe reads it: while it lists one,
    /// the runtime does not serve that member, and `/healthz` (in JSON) and
    /// `meta/status` list it too. Read without the mob actor.
    pub async fn broken_members(&self) -> Vec<(meerkat_mob::AgentIdentity, String)> {
        crate::mob_handle_runtime::broken_members_of(&self.mob_handle()).await
    }

    /// Access the underlying `MobRuntime` (owns the session service + ephemeral dir).
    /// It is the mob's public owner: clones of it keep the mob publicly owned.
    pub fn mob_runtime(&self) -> &crate::mob_handle_runtime::MobRuntime {
        match self.public_owner.as_ref() {
            Some(owner) => owner,
            // Only the retained teardown runs without a public owner.
            None => &self.mob_runtime,
        }
    }

    /// The mobs bound without extension authority, each with its typed
    /// reason. A mob activates (a create, a restore, a held child's resume)
    /// under an id that live extension documents name for another holder, or
    /// while the document store cannot tell who holds the id; it binds, but
    /// its members resolve no document caller, it is no valid document owner
    /// and nobody manages it, until it is destroyed or a later restore of it
    /// binds normally. Empty without extension tool bundles or document
    /// bindings.
    #[cfg(feature = "extension-state")]
    pub fn extension_mob_quarantines(&self) -> Vec<crate::extension_state::MobIdQuarantine> {
        self.extension_authority
            .as_ref()
            .map(|registry| registry.quarantines())
            .unwrap_or_default()
    }

    /// Start a completion-bearing turn for a public member alias.
    ///
    /// Admission proves that the runtime input was accepted; it does not prove
    /// that a requested per-turn LLM identity has reached the serialized
    /// executor boundary. Call
    /// [`MemberTurnHandle::wait_for_applied_llm_identity`](meerkat_mob::MemberTurnHandle::wait_for_applied_llm_identity)
    /// for that distinct fact. Nonterminal events stream through `event_tx` as
    /// they occur; terminal events and `turn.wait()` remain gated on the
    /// committed runtime completion boundary.
    pub async fn start_member_turn(
        &self,
        member_alias: &str,
        content: ContentInput,
        handling_mode: HandlingMode,
        options: MemberTurnOptions,
        event_tx: Option<MemberTurnEventSender>,
    ) -> Result<MemberTurnAdmission, MobRuntimeError> {
        // Member-id alias rule: decode first, so an encoded roster id is not
        // encoded twice, and refuse a generated runtime alias, which this
        // direct turn start has no identity authority to pin to a generation.
        let alias = crate::member_comms_id::runtime_alias_str(member_alias);
        if crate::member_comms_id::is_reserved_generated_alias(&alias) {
            return Err(MobRuntimeError::InvalidInput(
                "generated member alias requires identity authority",
            ));
        }
        let identity = crate::member_comms_id::mob_member_id(&alias);
        let member = self.mob_handle().member(&identity).await?;
        let turn = member
            .start_turn(content, handling_mode, options, event_tx)
            .await?;
        let session_id = turn
            .session_id()
            .ok_or(MobRuntimeError::InvalidInput(
                "completion-bearing member turns require a session-backed member",
            ))?
            .to_string();
        Ok(MemberTurnAdmission { session_id, turn })
    }

    /// Spawn a member, firing `post_spawn_hook` on success and the shared error
    /// hook on failure. For raw spawning without hooks, use `mob_handle().spawn_spec(...)`.
    ///
    /// The spec's member id is a public alias; it is encoded into the
    /// comms-safe roster id here (meerkat 0.7 `MemberCommsName` rejects `:`,
    /// which Meta's identity-first aliases like `rt:review:singleton:0`
    /// contain). Hooks and error events keep speaking the alias.
    pub async fn spawn(&self, mut spec: SpawnMemberSpec) -> Result<SpawnResult, MobRuntimeError> {
        if let Some(labels) = spec.labels.as_ref() {
            crate::member_comms_id::validate_raw_identity_labels(labels)
                .map_err(|message| MobRuntimeError::InvalidConfig(message.to_string()))?;
        }
        let raw_reservation = crate::member_comms_id::reserve_raw_member_target(
            self.identity_runtime_ref(),
            spec.identity.as_str(),
        )
        .await
        .map_err(MobRuntimeError::InvalidConfig)?;
        let member_id = raw_reservation.alias().to_string();
        let profile = spec.role_name.to_string();
        spec.identity = crate::member_comms_id::mob_member_id(member_id.as_str());
        let spawn_result = Box::pin(self.mob_handle().spawn_spec(spec)).await;
        drop(raw_reservation);
        match spawn_result {
            Ok(result) => {
                if let Some(hook) = &self.post_spawn_hook {
                    hook(vec![member_id]).await;
                }
                Ok(result)
            }
            Err(err) => {
                self.fire_error(super::types::ErrorEvent::SpawnFailure {
                    member_id,
                    profile,
                    error: format!("{err}"),
                });
                Err(err.into())
            }
        }
    }

    /// Spawn many members, firing `post_spawn_hook` once on success with all ids.
    pub async fn spawn_many(
        &self,
        mut specs: Vec<SpawnMemberSpec>,
    ) -> Result<Vec<SpawnResult>, MobRuntimeError> {
        let requested_member_ids = specs
            .iter()
            .map(|spec| spec.identity.to_string())
            .collect::<Vec<_>>();
        for spec in &specs {
            if let Some(labels) = spec.labels.as_ref() {
                crate::member_comms_id::validate_raw_identity_labels(labels)
                    .map_err(|message| MobRuntimeError::InvalidConfig(message.to_string()))?;
            }
        }
        let raw_reservation = if specs.is_empty() {
            None
        } else {
            Some(
                crate::member_comms_id::reserve_raw_member_targets(
                    self.identity_runtime_ref(),
                    requested_member_ids.iter().map(String::as_str),
                )
                .await
                .map_err(MobRuntimeError::InvalidConfig)?,
            )
        };
        let member_ids = raw_reservation
            .as_ref()
            .map(|reservation| reservation.aliases().to_vec())
            .unwrap_or(requested_member_ids);
        // As in `spawn`: wire aliases become comms-safe roster ids.
        for (spec, member_id) in specs.iter_mut().zip(&member_ids) {
            spec.identity = crate::member_comms_id::mob_member_id(member_id);
        }
        let handle = self.mob_handle();
        let refs = try_join_in_batches(specs, MAX_CONCURRENT_SPAWN_MANY, |spec| {
            let handle = handle.clone();
            async move { Box::pin(handle.spawn_spec(spec)).await }
        })
        .await
        .map_err(MobRuntimeError::from)?;
        drop(raw_reservation);
        if !member_ids.is_empty()
            && let Some(hook) = &self.post_spawn_hook
        {
            hook(member_ids).await;
        }
        Ok(refs)
    }
}

async fn try_join_in_batches<I, F, T, E, Build>(
    items: Vec<I>,
    batch_size: usize,
    mut build: Build,
) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, E>>,
    Build: FnMut(I) -> F,
{
    let batch_size = batch_size.max(1);
    let mut results = Vec::with_capacity(items.len());
    let mut iter = items.into_iter();

    loop {
        let batch: Vec<I> = iter.by_ref().take(batch_size).collect();
        if batch.is_empty() {
            break;
        }

        let futures = batch.into_iter().map(&mut build);
        let mut batch_results = futures::future::try_join_all(futures).await?;
        results.append(&mut batch_results);
        tokio::task::yield_now().await;
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{is_ready_wait_timeout, try_join_in_batches};
    use meerkat_mob::MobError;

    #[tokio::test]
    async fn spawn_many_batch_size_stays_serial_until_upstream_backpressure_exists() {
        assert_eq!(super::MAX_CONCURRENT_SPAWN_MANY, 1);
    }

    #[test]
    fn ready_wait_timeout_is_classified_as_envelope_not_error() {
        // Regression (meerkat-mob 0.7.9 #798): the default ready-wait dropped
        // 600s -> 60s, so `wait_for_ready` hits this timeout far more often.
        // The prior `message.to_lowercase().contains("timeout")` never matched
        // the Display "member ready wait timed out", so timeouts wrongly
        // surfaced as a -32000 RPC error instead of `{ ready: [], timeout:true }`.
        let timed_out = MobError::ReadyWaitTimedOut {
            pending_member_ids: vec![],
        };
        assert!(is_ready_wait_timeout(&timed_out));

        // Pin the exact failure mode that motivated the typed match.
        let display = timed_out.to_string().to_lowercase();
        assert!(
            !display.contains("timeout"),
            "old substring check would have missed this timeout"
        );
        assert!(display.contains("timed out"));

        // Precision: a *kickoff* timeout also Displays "...timed out" but is not
        // a readiness timeout — it must NOT be folded into the ready envelope.
        assert!(!is_ready_wait_timeout(&MobError::KickoffWaitTimedOut {
            pending_member_ids: vec![],
        }));
    }

    #[tokio::test]
    async fn try_join_in_batches_can_run_serially() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let items: Vec<usize> = (0..25).collect();

        let results = try_join_in_batches(items.clone(), 1, |item| {
            let active = active.clone();
            let max_active = max_active.clone();
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, ()>(item)
            }
        })
        .await;

        assert_eq!(results, Ok(items));
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn try_join_in_batches_limits_concurrent_work_and_preserves_order() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let items: Vec<usize> = (0..75).collect();

        let results = try_join_in_batches(items.clone(), 16, |item| {
            let active = active.clone();
            let max_active = max_active.clone();
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, ()>(item)
            }
        })
        .await;

        assert_eq!(results, Ok(items));
        assert!(max_active.load(Ordering::SeqCst) <= 16);
    }

    #[tokio::test]
    async fn try_join_in_batches_stops_before_starting_later_batches_after_error() {
        let started = Arc::new(AtomicUsize::new(0));
        let items: Vec<usize> = (0..40).collect();

        let result = try_join_in_batches(items, 16, |item| {
            let started = started.clone();
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                if item == 20 { Err(item) } else { Ok(item) }
            }
        })
        .await;

        assert_eq!(result, Err(20));
        assert_eq!(started.load(Ordering::SeqCst), 32);
    }
}
