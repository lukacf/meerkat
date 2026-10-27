//! Orchestrator: restore flow, reconciliation, and topology wiring.
//!
//! Implements REQ-12 (restore flow), REQ-13 (broken continuity), REQ-19/REQ-21
//! (topology reconciliation), REQ-20 (static wiring preserved), REQ-22 (topology
//! not continuity truth), and REQ-33 (identity-keyed reconciliation).

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use futures::{StreamExt, stream};

use std::sync::Arc;

use super::bridge::MemberRuntimeBinding;
use super::contracts::{AgentCustomizer, TopologyProvider};
use super::runtime::{EmbodimentOverrides, IdentityRuntime, IdentityRuntimeError};
use super::types::{
    AgentBuildDraft, AgentIdentity, DurableAgentSpec, IdentityRestoreProgress, ManagedPeerEdge,
    TopologyContext,
};

pub(crate) const IDENTITY_RESTORE_CONCURRENCY: usize = 4;

/// Effective restore concurrency: `META_IDENTITY_RESTORE_CONCURRENCY`
/// overrides the default, clamped to [1, 16]. `1` serializes restores — the
/// field mitigation for Bug I, where concurrent multi-hundred-MB session
/// resumes against one SQLite store exceed the 5s writer busy_timeout and the
/// resulting mid-spawn store failures feed meerkat's destructive resume
/// rollback (upstream ask 31).
pub(crate) fn identity_restore_concurrency() -> usize {
    parse_identity_restore_concurrency(
        std::env::var("META_IDENTITY_RESTORE_CONCURRENCY")
            .ok()
            .as_deref(),
    )
}

fn parse_identity_restore_concurrency(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, 16))
        .unwrap_or(IDENTITY_RESTORE_CONCURRENCY)
}

#[cfg(test)]
mod restore_concurrency_tests {
    use super::{IDENTITY_RESTORE_CONCURRENCY, parse_identity_restore_concurrency};

    #[test]
    fn defaults_when_unset_or_invalid() {
        assert_eq!(
            parse_identity_restore_concurrency(None),
            IDENTITY_RESTORE_CONCURRENCY
        );
        assert_eq!(
            parse_identity_restore_concurrency(Some("")),
            IDENTITY_RESTORE_CONCURRENCY
        );
        assert_eq!(
            parse_identity_restore_concurrency(Some("not-a-number")),
            IDENTITY_RESTORE_CONCURRENCY
        );
    }

    #[test]
    fn parses_and_clamps() {
        assert_eq!(parse_identity_restore_concurrency(Some("1")), 1);
        assert_eq!(parse_identity_restore_concurrency(Some(" 8 ")), 8);
        assert_eq!(parse_identity_restore_concurrency(Some("0")), 1);
        assert_eq!(parse_identity_restore_concurrency(Some("64")), 16);
    }

    // The former `should_load_resume_snapshot` / `RestoreSnapshotPolicy` unit
    // tests are gone with the policy twin they described. The surviving rule
    // ("load the continuity payload only when no bridge is composed") is
    // guarded end to end where it can actually regress:
    // `identity_first_public_eager_refresh_elides_snapshot_load` counts real
    // store calls, and the bridge-less `restore_flow` tests in
    // identity_first_runtime / identity_first_choke / identity_first_e2e pin
    // the classification path that still depends on the payload.
}

fn trace_identity_restore_completed(identity: &AgentIdentity, started_at: Instant) {
    let elapsed = started_at.elapsed();
    if elapsed >= Duration::from_secs(1) {
        tracing::info!(
            %identity,
            elapsed_ms = elapsed.as_millis(),
            "identity restore completed"
        );
    } else {
        tracing::debug!(
            %identity,
            elapsed_ms = elapsed.as_millis(),
            "identity restore completed"
        );
    }
}

// ---------------------------------------------------------------------------
// Restore flow result
// ---------------------------------------------------------------------------

/// Result of the restore flow for a single identity.
#[derive(Debug, Clone)]
pub enum RestoreOutcome {
    /// Registered without a wake: a lazy bootstrap, or a runtime with no
    /// session bridge (validation only).
    Dormant { draft: AgentBuildDraft },
    /// Core spawned the identity fresh. `replaced_session` names the last
    /// session it replaced because Core holds no durable session for it
    ///; `None` for an identity Core never spawned.
    Created {
        binding: MemberRuntimeBinding,
        draft: AgentBuildDraft,
        replaced_session: Option<meerkat_core::types::SessionId>,
    },
    /// Core resumed the identity onto its committed session (or revived the
    /// member it already held). `draft` is what this wake built: for a member
    /// Core already held live nothing is built, so it is the spec's base
    /// draft, and the member keeps the system prompt it was built with.
    Resumed {
        binding: MemberRuntimeBinding,
        draft: AgentBuildDraft,
    },
    /// The wake was refused, typed: a host-adopted identity, a
    /// session Core cannot resume, a held resume, or a transient fault.
    /// Nothing is parked: the next wake asks Core again, and unrelated
    /// members keep restoring.
    Refused(Arc<IdentityRuntimeError>),
}

impl RestoreOutcome {
    /// The typed restore progress this settled outcome publishes.
    pub fn restore_progress(&self) -> IdentityRestoreProgress {
        match self {
            Self::Dormant { .. } => IdentityRestoreProgress::Registered,
            Self::Created { .. } => IdentityRestoreProgress::Minted,
            Self::Resumed { .. } => IdentityRestoreProgress::Resumed,
            Self::Refused(error) => IdentityRestoreProgress::Refused {
                code: crate::rpc::identity_error_code(error),
                kind: error.structured_data().and_then(|data| {
                    data.get("kind")
                        .and_then(serde_json::Value::as_str)
                        .map(ToString::to_string)
                }),
            },
        }
    }
}

/// Result of the full restore flow.
#[derive(Debug)]
pub struct RestoreFlowResult {
    pub outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    pub managed_edges: Vec<ManagedPeerEdge>,
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

/// Actions produced by reconciliation (REQ-33).
#[derive(Debug, Clone)]
pub enum ReconcileAction {
    /// New identity: register and wake.
    Activate(DurableAgentSpec),
    /// Removed identity: retire.
    Retire(AgentIdentity),
    /// Changed spec: hot-reload metadata fields.
    HotReload {
        identity: AgentIdentity,
        new_spec: DurableAgentSpec,
    },
    /// Profile changed: requires respawn.
    Respawn {
        identity: AgentIdentity,
        new_spec: DurableAgentSpec,
    },
}

/// Compute reconciliation actions by comparing desired roster to current active set.
pub fn compute_reconcile_actions(
    desired: &[DurableAgentSpec],
    current: &BTreeMap<AgentIdentity, DurableAgentSpec>,
) -> Vec<ReconcileAction> {
    let mut actions = Vec::new();

    let desired_map: BTreeMap<&AgentIdentity, &DurableAgentSpec> =
        desired.iter().map(|s| (&s.identity, s)).collect();

    // New identities: in desired, not in current
    for (id, spec) in &desired_map {
        if !current.contains_key(*id) {
            actions.push(ReconcileAction::Activate((*spec).clone()));
        }
    }

    // Removed identities: in current, not in desired
    for id in current.keys() {
        if !desired_map.contains_key(id) {
            actions.push(ReconcileAction::Retire(id.clone()));
        }
    }

    // Changed identities: in both, spec differs
    for (id, new_spec) in &desired_map {
        if let Some(old_spec) = current.get(*id)
            && old_spec != *new_spec
        {
            // Profile change → respawn (REQ-33)
            if old_spec.profile == new_spec.profile {
                // labels, display_name, addressability → hot-reload
                // context, additional_instructions → registry update
                actions.push(ReconcileAction::HotReload {
                    identity: (*id).clone(),
                    new_spec: (*new_spec).clone(),
                });
            } else {
                actions.push(ReconcileAction::Respawn {
                    identity: (*id).clone(),
                    new_spec: (*new_spec).clone(),
                });
            }
        }
    }

    actions
}

type RosterAuthorityGuards = (
    tokio::sync::OwnedMutexGuard<()>,
    tokio::sync::OwnedMutexGuard<()>,
);

/// Reserve every identity and its public alias in one stable order before a
/// roster-wide registration. Keeping the guards owned lets the
/// restore work remain concurrent after the all-or-nothing authority gate,
/// while reset, delete, materialize, and raw-member creation cannot invalidate
/// any member's freshly resolved generation.
async fn acquire_roster_authority_guards(
    runtime: &IdentityRuntime,
    identities: &[AgentIdentity],
) -> Result<BTreeMap<AgentIdentity, RosterAuthorityGuards>, IdentityRuntimeError> {
    let ordered = identities.iter().cloned().collect::<BTreeSet<_>>();
    let mut lifecycle_guards = BTreeMap::new();
    for identity in &ordered {
        lifecycle_guards.insert(
            identity.clone(),
            runtime
                .lifecycle_lock_for(identity)
                .await
                .lock_owned()
                .await,
        );
    }

    let mut alias_guards = BTreeMap::new();
    for identity in &ordered {
        alias_guards.insert(
            identity.clone(),
            runtime
                .raw_member_alias_lock(identity.as_str())
                .await
                .lock_owned()
                .await,
        );
    }
    for identity in &ordered {
        runtime.ensure_raw_member_alias_available(identity).await?;
    }

    ordered
        .into_iter()
        .map(|identity| {
            let lifecycle = lifecycle_guards.remove(&identity).ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "authority gate lost lifecycle reservation for {identity}"
                ))
            })?;
            let alias = alias_guards.remove(&identity).ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "authority gate lost alias reservation for {identity}"
                ))
            })?;
            Ok((identity, (lifecycle, alias)))
        })
        .collect::<Result<_, IdentityRuntimeError>>()
}

// ---------------------------------------------------------------------------
// Restore flow orchestration — REQ-12
// ---------------------------------------------------------------------------

/// Wake one registered roster member in a restore pass: wait for a
/// construction slot of the held pass covering it, if any, wake it through
/// the one wake door, and account for the outcome (the pass's held
/// bookkeeping, the bootstrap progress). Without a session bridge
/// (`wakes` false) a Dormant member stays Dormant.
async fn restore_registered_member(
    runtime: &IdentityRuntime,
    spec: DurableAgentSpec,
    initial: Option<RestoreOutcome>,
    held_pass: Option<Arc<super::held_bootstrap::HeldCreatePass>>,
    customizer: Option<&dyn AgentCustomizer>,
    wakes: bool,
) -> RestoreOutcome {
    let identity = spec.identity.clone();
    // A held member waits for a construction slot only within the setup's
    // bound: once the pass's terminal decision is `Abort`, it is never
    // constructed.
    let construction_refused = match held_pass.as_ref() {
        Some(pass) => !pass.take_construction_slot(&identity).await,
        None => false,
    };
    let member_started_at = Instant::now();

    let outcome = match initial {
        _ if construction_refused => {
            RestoreOutcome::Refused(Arc::new(IdentityRuntimeError::Internal(format!(
                "the held identity bootstrap ended before {identity}'s construction started"
            ))))
        }
        Some(RestoreOutcome::Refused(error)) => RestoreOutcome::Refused(error),
        Some(dormant @ RestoreOutcome::Dormant { .. }) if !wakes => dormant,
        Some(
            RestoreOutcome::Dormant { .. }
            | RestoreOutcome::Created { .. }
            | RestoreOutcome::Resumed { .. },
        ) => {
            runtime.publish_restore_progress(&identity, IdentityRestoreProgress::Resuming, None);
            let mut bound_bootstrap_generation = None;
            match runtime
                .embody_identity(
                    &identity,
                    None,
                    None,
                    None,
                    &mut bound_bootstrap_generation,
                    EmbodimentOverrides {
                        spec: Some(&spec),
                        customizer,
                        expected_spec_digest: None,
                    },
                )
                .await
            {
                Ok(wake) if wake.resumed => RestoreOutcome::Resumed {
                    binding: wake.binding,
                    draft: wake.draft,
                },
                Ok(wake) => RestoreOutcome::Created {
                    binding: wake.binding,
                    draft: wake.draft,
                    replaced_session: wake.replaced_session,
                },
                Err(error) => {
                    tracing::warn!(
                        %identity,
                        %error,
                        "identity wake refused; the fleet restore continues"
                    );
                    RestoreOutcome::Refused(Arc::new(error))
                }
            }
        }
        None => {
            // register_identity_metadata proves one result for every
            // registered identity. A violated internal correspondence is this
            // member's visible failure, never the fleet's.
            RestoreOutcome::Refused(Arc::new(IdentityRuntimeError::Internal(format!(
                "validated registration outcome disappeared for {identity}"
            ))))
        }
    };

    // A held pass settles what its host's completion covers: a published
    // member owes nothing, and a refused one owes its host a fresh held
    // completion before it is created again.
    if held_pass.is_some() {
        match &outcome {
            RestoreOutcome::Created { .. } => {
                runtime.record_held_completion(&identity, true);
                runtime.clear_held_completion_mark(&identity).await;
            }
            RestoreOutcome::Refused(_) => {
                runtime.record_held_completion(&identity, false);
            }
            RestoreOutcome::Dormant { .. } | RestoreOutcome::Resumed { .. } => {}
        }
    }
    // A candidate's refusal of a roster change defers the identity to the
    // authoritative launch instead of breaking it.
    let deferred = match &outcome {
        RestoreOutcome::Refused(error) => {
            runtime.defer_bootstrap_identity_on_candidate_refusal(&identity, error)
        }
        _ => false,
    };
    if !deferred {
        let error = match &outcome {
            RestoreOutcome::Refused(error) => Some(error.to_string()),
            _ => None,
        };
        runtime.publish_restore_progress(&identity, outcome.restore_progress(), error);
    }
    trace_identity_restore_completed(&identity, member_started_at);
    if let Some(pass) = held_pass.as_ref() {
        pass.settle(&identity);
    }
    outcome
}

/// Execute the full restore flow per REQ-12 sequencing.
///
/// Steps:
/// 1. Validate the roster, compute topology, and register every roster spec
///    (a host-adopted identity is a typed refusal in the report).
/// 2. Wake each identity through `IdentityRuntime::embody_identity`, the one
///    wake door: Core resumes it onto its committed session, or spawns it
///    fresh when Core never spawned it.
/// 3. A refused wake is a typed `Refused` outcome for that identity only;
///    the remaining identities keep restoring.
///
/// Without a session bridge the flow is validation-only: every outcome is
/// `Dormant` and nothing is woken.
pub async fn restore_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    // Fleet-level declaration and topology remain fail-closed: without a
    // valid unique roster there is no truthful set of identities to wake.
    let registered = register_roster_metadata(runtime, roster, topology_provider, false).await?;
    let managed_edges = registered.managed_edges;
    let registered_outcomes = registered.outcomes;
    // Typed per-member progress on the bootstrap status, so a host follows
    // each member instead of waiting on the whole pass.
    for (identity, outcome) in &registered_outcomes {
        let (progress, error) = match outcome {
            RestoreOutcome::Refused(error) => {
                if runtime.defer_bootstrap_identity_on_candidate_refusal(identity, error) {
                    continue;
                }
                (outcome.restore_progress(), Some(error.to_string()))
            }
            _ => (IdentityRestoreProgress::Registered, None),
        };
        runtime.publish_restore_progress(identity, progress, error);
    }
    let wakes = runtime.has_session_bridge();

    // Each identity wakes on its own. A refused wake is that identity's
    // typed outcome; it never aborts an unrelated member's boot.
    // A held pass decides its members only once every one of them is held
    // or settled. Its members wait on the pass's construction slots (the
    // ordinary restore concurrency) and give a slot back when they report
    // held, so a held member never blocks a later member's construction.
    let held_pass = runtime.held_create_pass();
    let restore_concurrency = if held_pass.is_some() {
        roster.len().max(1)
    } else {
        identity_restore_concurrency()
    };
    tracing::info!(
        member_count = roster.len(),
        concurrency = restore_concurrency,
        "starting identity restore"
    );
    let restore_started_at = Instant::now();
    let mut restored = stream::iter(roster.iter().cloned().enumerate())
        .map(|(index, spec)| {
            let initial = registered_outcomes.get(&spec.identity).cloned();
            let held_pass = held_pass.clone();
            async move {
                let identity = spec.identity.clone();
                let outcome =
                    restore_registered_member(runtime, spec, initial, held_pass, customizer, wakes)
                        .await;
                (index, identity, outcome)
            }
        })
        .buffer_unordered(restore_concurrency)
        .collect::<Vec<_>>()
        .await;
    restored.sort_by_key(|(index, _, _)| *index);

    let outcomes = restored
        .into_iter()
        .map(|(_, identity, outcome)| (identity, outcome))
        .collect();
    tracing::info!(
        member_count = roster.len(),
        elapsed_ms = restore_started_at.elapsed().as_millis(),
        "identity restore completed"
    );

    // Topology is a fleet-level projection over the woken set. Member
    // refusals are typed outcomes already, but an invalid fleet projection
    // remains a pass failure rather than being attributed to any one
    // identity.
    runtime.set_desired_peer_edges(managed_edges.clone()).await;
    runtime.reconcile_managed_peer_edges(&managed_edges).await?;

    Ok(RestoreFlowResult {
        outcomes,
        managed_edges,
    })
}

/// Register the roster/topology metadata without waking members.
///
/// This is the identity-first lazy bootstrap path: it validates the roster,
/// registers each spec, records desired topology, and exposes status and
/// inspection surfaces (which read Core). It calls no customizer and asks
/// Core for no session.
pub async fn lazy_register_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    register_roster_metadata(runtime, roster, topology_provider, true).await
}

/// Validate the roster and topology and register every roster spec, without
/// waking anything. Meta keeps no Core fact: the registration is the
/// roster's policy only, and every Core fact is read per call. A roster
/// entry naming a host-adopted identity (Core holds its identity intent
/// row) is registered for the views but reported as a typed refusal: no
/// Meta verb acts on it. A lazy reconcile (`reconcile_edges`) also
/// reconciles the managed topology, since no wake pass follows it.
///
/// The first-boot manifest is read first. A manifest the reader
/// refuses refuses every identity of the compile, typed, and nothing wakes:
/// which identities it covers is unknown. The compile itself goes on, so the
/// gateway stays up and its status names the file and the refusal. The
/// pending row of each identity the roster lists is carried out right after
/// its registration, before anything wakes the identity; a row that fails
/// or is refused makes that identity's outcome a typed refusal, and the rest
/// of the roster goes on. A row whose identity the roster does not list
/// stays pending, unless Core's member history shows it done (a row for a
/// helper or a fork child the host made on the mob and retired, which
/// `storage-migrate-native` before 0.9.5 wrote): such a row is settled
/// ([`IdentityRuntime::settle_done_unlisted_first_boot_rows`]). Then each
/// listed identity whose live Core member lacks
/// the durable-identity marker (Meta 0.9.3 spawned it) is respawned onto the
/// same session ([`IdentityRuntime::respawn_unmarked_identities`]); a failed
/// respawn is that identity's typed refusal.
async fn register_roster_metadata(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    reconcile_edges: bool,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    IdentityRuntime::validate_roster_uniqueness(roster)?;
    let first_boot_unreadable = runtime.load_first_boot_manifest().await.err().map(Arc::new);

    let identities: Vec<AgentIdentity> = roster.iter().map(|s| s.identity.clone()).collect();

    let topology_context = TopologyContext {
        roster: roster.to_vec(),
    };
    let managed_edges = if let Some(tp) = topology_provider {
        tp.compute_edges(&identities, &topology_context)
            .await
            .map_err(|e| IdentityRuntimeError::Internal(format!("topology: {e}")))?
    } else {
        Vec::new()
    };
    runtime.set_identity_edge_ownership(
        super::runtime::IdentityEdgeOwnership::for_topology_provider(topology_provider.is_some()),
    );
    runtime.set_desired_peer_edges(managed_edges.clone()).await;

    let outcomes =
        register_identity_metadata(runtime, roster, first_boot_unreadable.as_ref()).await?;
    // The rows of identities this roster does not list wait for a roster
    // that lists them, unless Core's history already shows them done.
    if first_boot_unreadable.is_none() {
        runtime.settle_done_unlisted_first_boot_rows().await;
    }

    if reconcile_edges && let Err(err) = runtime.reconcile_managed_peer_edges(&managed_edges).await
    {
        tracing::warn!(
            error = %err,
            "identity lazy register flow completed with topology reconcile warning"
        );
    }

    Ok(RestoreFlowResult {
        outcomes,
        managed_edges,
    })
}

/// Register exactly `specs`' identities: each one's spec, its typed
/// refusal when Core holds its identity intent row or the first-boot
/// manifest is unreadable, and its pending first-boot row carried out
/// before anything wakes it. The roster-wide half (uniqueness, topology,
/// desired edges, the manifest read) stays with the caller.
async fn register_identity_metadata(
    runtime: &IdentityRuntime,
    specs: &[DurableAgentSpec],
    first_boot_unreadable: Option<&Arc<IdentityRuntimeError>>,
) -> Result<BTreeMap<AgentIdentity, RestoreOutcome>, IdentityRuntimeError> {
    let roster = specs;
    let identities: Vec<AgentIdentity> = roster.iter().map(|s| s.identity.clone()).collect();
    // Reserve the full roster in one stable order, so a queued reset, delete
    // or raw-member creation cannot interleave with the registration.
    let mut authority_guards = acquire_roster_authority_guards(runtime, &identities).await?;

    let mut outcomes = BTreeMap::new();
    let mut first_boot = Vec::new();
    let mut listed = Vec::new();
    for spec in roster {
        let identity = &spec.identity;
        let (lifecycle_guard, alias_guard) =
            authority_guards.remove(identity).ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "validated roster reservation disappeared for {identity}"
                ))
            })?;
        runtime.register_locked(spec.clone()).await;
        if let Err(error) = runtime.refuse_host_adopted(identity).await {
            tracing::warn!(
                %identity,
                %error,
                "roster entry names an identity Meta may not act on; it is registered for the \
                 views and reported refused"
            );
            outcomes.insert(identity.clone(), RestoreOutcome::Refused(Arc::new(error)));
            continue;
        }
        if let Some(error) = first_boot_unreadable {
            outcomes.insert(identity.clone(), RestoreOutcome::Refused(Arc::clone(error)));
            continue;
        }
        outcomes.insert(
            identity.clone(),
            RestoreOutcome::Dormant {
                draft: IdentityRuntime::base_build_draft(spec),
            },
        );
        listed.push(spec.clone());
        // The identity's first-boot row is carried out below, before
        // anything wakes it: its lifecycle reservation is kept until then
        // (the wake that carries a `Resume` row out takes the alias lock
        // itself).
        if runtime.has_pending_first_boot_row(identity) {
            drop(alias_guard);
            first_boot.push((spec.clone(), lifecycle_guard));
        }
    }
    for (identity, error) in runtime.carry_out_first_boot_rows(first_boot).await {
        tracing::warn!(
            %identity,
            %error,
            "the identity's first-boot row was not carried out; it stays pending and the \
             identity does not wake until it is"
        );
        outcomes.insert(identity, RestoreOutcome::Refused(Arc::new(error)));
    }
    for (identity, error) in runtime.respawn_unmarked_identities(&listed).await {
        // A candidate launch's refusal defers the identity; it is no failure.
        if super::runtime::candidate_deferral_of(&error).is_none() {
            tracing::warn!(
                %identity,
                %error,
                "the identity's live member lacks the durable-identity marker and its respawn \
                 failed; the next roster compile tries it again"
            );
        }
        outcomes.insert(identity, RestoreOutcome::Refused(Arc::new(error)));
    }

    Ok(outcomes)
}

/// Register and wake one fresh member while the rest of the roster keeps
/// running: the per-identity half of [`restore_flow`] for exactly `spec`,
/// under the held pass that covers it. Nothing else is reconciled and the
/// managed peer edges are not touched, so the result carries this member's
/// outcome and no edges.
pub(crate) async fn add_member_flow(
    runtime: &IdentityRuntime,
    spec: &DurableAgentSpec,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    let first_boot_unreadable = runtime.load_first_boot_manifest().await.err().map(Arc::new);
    let mut registered = register_identity_metadata(
        runtime,
        std::slice::from_ref(spec),
        first_boot_unreadable.as_ref(),
    )
    .await?;
    let initial = registered.remove(&spec.identity);
    let outcome = restore_registered_member(
        runtime,
        spec.clone(),
        initial,
        runtime.held_create_pass(),
        customizer,
        runtime.has_session_bridge(),
    )
    .await;
    Ok(RestoreFlowResult {
        outcomes: BTreeMap::from([(spec.identity.clone(), outcome)]),
        managed_edges: Vec::new(),
    })
}
