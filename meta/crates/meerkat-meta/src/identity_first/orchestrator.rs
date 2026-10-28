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
use super::runtime::{EmbodimentOverrides, IdentityRuntime, IdentityRuntimeError, SeatOutcome};
use super::types::{
    AgentBuildDraft, AgentIdentity, DurableAgentSpec, IdentityRestoreProgress, ManagedPeerEdge,
    TopologyContext,
};

/// How many identities a roster restores at once when
/// `META_IDENTITY_RESTORE_CONCURRENCY` does not say: the eager wakes of a
/// roster pass, the respawns of members without the durable-identity
/// marker, a held pass's construction slots, and the first-boot rows an
/// import or a storage migration leaves for the first roster compile.
///
/// 16, up from 4, since startup time is what a large roster waits on. What
/// made 4 the safe default no longer holds:
/// - Meerkat saves a session in O(delta) writes (upstream ask 11) and a
///   long-lived session's snapshot no longer grows quadratically (upstream
///   ask 35), so concurrent resumes no longer hold the store's one writer
///   for multi-hundred-MB commits;
/// - a resume that fails late is rolled back without touching the durable
///   session (upstream ask 31): a busy store refuses that one identity's
///   wake, typed and retryable, and nothing is lost;
/// - Core bounds its own side of a spawn: at most
///   `SPAWN_PREPARATION_CONCURRENCY` (8 in Core 0.9.6) spawn preparations
///   read sessions at once, and the mob actor commits spawn admissions one
///   at a time.
///
/// Upstream ask 33 (the 60 s SQLite busy timeout) is what let the knob
/// return to its default of 4; it is not the ground for 16.
/// `META_IDENTITY_RESTORE_CONCURRENCY` tunes it.
pub(crate) const IDENTITY_RESTORE_CONCURRENCY: usize = 16;

/// The most identities a roster restores at once, whatever
/// `META_IDENTITY_RESTORE_CONCURRENCY` says.
pub(crate) const IDENTITY_RESTORE_CONCURRENCY_MAX: usize = 16;

/// Effective restore concurrency: `META_IDENTITY_RESTORE_CONCURRENCY`
/// overrides the default ([`IDENTITY_RESTORE_CONCURRENCY`]), clamped to
/// [1, [`IDENTITY_RESTORE_CONCURRENCY_MAX`]]. `1` serializes restores, for
/// a store whose one writer still cannot take concurrent session resumes.
pub(crate) fn identity_restore_concurrency() -> usize {
    parse_identity_restore_concurrency(
        std::env::var("META_IDENTITY_RESTORE_CONCURRENCY")
            .ok()
            .as_deref(),
    )
}

fn parse_identity_restore_concurrency(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<usize>().ok())
        .map(|value| value.clamp(1, IDENTITY_RESTORE_CONCURRENCY_MAX))
        .unwrap_or(IDENTITY_RESTORE_CONCURRENCY)
}

#[cfg(test)]
mod restore_concurrency_tests {
    use super::{
        IDENTITY_RESTORE_CONCURRENCY, IDENTITY_RESTORE_CONCURRENCY_MAX,
        parse_identity_restore_concurrency,
    };

    #[test]
    fn defaults_when_unset_or_invalid() {
        // A roster restores 16 identities at once by default, the most the
        // knob admits.
        assert_eq!(IDENTITY_RESTORE_CONCURRENCY, 16);
        assert_eq!(IDENTITY_RESTORE_CONCURRENCY_MAX, 16);
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
    /// Core bound the identity to a fresh session it minted: a new
    /// identity, or a fresh life. Meta never starts one in place of a
    /// session Core holds no durable copy of.
    Created {
        binding: MemberRuntimeBinding,
        draft: AgentBuildDraft,
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
    // One read of Core's roster answers every identity (no Core round trip
    // per identity at boot).
    runtime
        .ensure_raw_member_aliases_available(&ordered)
        .await?;

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
                        ..EmbodimentOverrides::default()
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
/// 2. Make every registered identity a member of Core's mob without
///    materializing it ([`IdentityRuntime::seat_identities`]): an identity
///    Core holds needs nothing, and one it does not hold is seated Dormant,
///    its pending first-boot row carried out with it. Core materializes each
///    member on its first delivery, or at boot when its own obligation facts
///    say the member owes work. Meta wakes nothing.
/// 3. A held pass creates its fresh members itself, held, through the
///    identity's create ([`IdentityRuntime::embody_identity`]).
/// 4. A refusal is a typed `Refused` outcome for that identity only; the
///    remaining identities go on.
///
/// Without a session bridge the flow is validation-only: every outcome is
/// `Dormant` and nothing is seated.
pub async fn restore_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    // Fleet-level declaration and topology remain fail-closed: without a
    // valid unique roster there is no truthful set of identities to seat.
    let registered = register_roster_metadata(
        runtime,
        roster,
        topology_provider,
        RosterPass::Default,
        customizer,
    )
    .await?;
    let managed_edges = registered.managed_edges;
    let mut outcomes =
        seated_restore_outcomes(runtime, registered.outcomes, &registered.seats).await;
    // Typed per-member progress on the bootstrap status: every member is
    // settled together once Core answers the pass's seat call, each with
    // its own typed outcome.
    for (identity, outcome) in &outcomes {
        let error = match outcome {
            RestoreOutcome::Refused(error) => {
                if runtime.defer_bootstrap_identity_on_candidate_refusal(identity, error) {
                    continue;
                }
                Some(error.to_string())
            }
            _ => None,
        };
        runtime.publish_restore_progress(identity, outcome.restore_progress(), error);
    }

    // A held pass decides its members only once every one of them is held
    // or settled. Its members wait on the pass's construction slots (the
    // ordinary restore concurrency) and give a slot back when they report
    // held, so a held member never blocks a later member's construction.
    if let Some(held_pass) = runtime.held_create_pass() {
        let wakes = runtime.has_session_bridge();
        let held = roster
            .iter()
            .filter(|spec| held_pass.covers(&spec.identity))
            .cloned()
            .collect::<Vec<_>>();
        tracing::info!(
            member_count = held.len(),
            "creating the held pass's members"
        );
        let started_at = Instant::now();
        let concurrency = held.len().max(1);
        let created = stream::iter(held)
            .map(|spec| {
                let initial = outcomes.get(&spec.identity).cloned();
                let held_pass = Arc::clone(&held_pass);
                async move {
                    let identity = spec.identity.clone();
                    let outcome = restore_registered_member(
                        runtime,
                        spec,
                        initial,
                        Some(held_pass),
                        customizer,
                        wakes,
                    )
                    .await;
                    (identity, outcome)
                }
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        outcomes.extend(created);
        tracing::info!(
            elapsed_ms = started_at.elapsed().as_millis(),
            "the held pass's members are settled"
        );
    }

    // Topology is a fleet-level projection over the seated members. Core
    // wires a Dormant member's edges without materializing it. Member
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

/// Register the roster and topology metadata, and seat every identity Core
/// does not hold, without materializing any.
///
/// The explicit lazy bootstrap modes run this. Every listed identity Meta
/// can seat is a member of Core's mob afterwards, as under the default, so
/// peers reach it; Core materializes it on demand. A placed or externally
/// bound identity Core does not hold is left for its first use, as these
/// modes always left it: its host creates it, so a create at the pass would
/// read broken while that host is not bound yet. A lazy reconcile also
/// reconciles the managed topology, since no restore pass follows it.
pub async fn lazy_register_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    let registered =
        register_roster_metadata(runtime, roster, topology_provider, RosterPass::Lazy, None)
            .await?;
    Ok(RestoreFlowResult {
        outcomes: registered.outcomes,
        managed_edges: registered.managed_edges,
    })
}

/// Which bootstrap pass registers the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RosterPass {
    /// The default pass (`eager_materialize`, the reported default): every
    /// listed identity becomes a member of Core's mob, a hosted one through
    /// its host.
    Default,
    /// The explicit lazy modes: as the default, but a hosted identity Core
    /// does not hold waits for its first use, and the pass reconciles the
    /// managed topology itself.
    Lazy,
}

/// What a roster registration made of its identities.
struct RegisteredRoster {
    /// Each identity's outcome: refused typed, or registered (`Dormant`
    /// with the spec's base draft).
    outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    /// What the seat step left of each identity it seated or found seated.
    seats: BTreeMap<AgentIdentity, SeatOutcome>,
    managed_edges: Vec<ManagedPeerEdge>,
}

/// Validate the roster and topology, register every roster spec, and make
/// every registered identity a member of Core's mob without materializing
/// it. Meta keeps no Core fact: the registration is the roster's policy
/// only, and every Core fact is read per call. A roster entry naming a
/// host-adopted identity (Core holds its identity intent row) is registered
/// for the views but reported as a typed refusal: no Meta verb acts on it. A
/// lazy reconcile (`reconcile_edges`) also reconciles the managed topology,
/// since no restore pass follows it.
///
/// The first-boot manifest is read first. A manifest the reader
/// refuses refuses every identity of the compile, typed, and nothing is
/// seated: which identities it covers is unknown. The compile itself goes
/// on, so the gateway stays up and its status names the file and the
/// refusal. The pending row of each identity the roster lists is carried out
/// by the identity's seat; a row that fails or is refused makes that
/// identity's outcome a typed refusal, and the rest of the roster goes on.
/// A row whose identity the roster does not list stays pending, unless
/// Core's member history shows it done (a row for a helper or a fork child
/// the host made on the mob and retired, which `storage-migrate-native`
/// before 0.9.5 wrote): such a row is settled
/// ([`IdentityRuntime::settle_done_unlisted_first_boot_rows`]). Then each
/// listed identity whose Core member lacks the durable-identity marker
/// (Meta 0.9.3 spawned it) is respawned onto the same session
/// ([`IdentityRuntime::respawn_unmarked_identities`]); a failed respawn is
/// that identity's typed refusal.
async fn register_roster_metadata(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    pass: RosterPass,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RegisteredRoster, IdentityRuntimeError> {
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

    let registered = register_identity_metadata(
        runtime,
        roster,
        first_boot_unreadable.as_ref(),
        customizer,
        pass,
    )
    .await?;
    // The rows of identities this roster does not list wait for a roster
    // that lists them, unless Core's history already shows them done.
    if first_boot_unreadable.is_none() {
        runtime.settle_done_unlisted_first_boot_rows().await;
    }

    if pass == RosterPass::Lazy
        && let Err(err) = runtime.reconcile_managed_peer_edges(&managed_edges).await
    {
        tracing::warn!(
            error = %err,
            "identity lazy register flow completed with topology reconcile warning"
        );
    }

    Ok(RegisteredRoster {
        outcomes: registered.outcomes,
        seats: registered.seats,
        managed_edges,
    })
}

/// What [`register_identity_metadata`] made of its identities.
struct RegisteredIdentities {
    outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    seats: BTreeMap<AgentIdentity, SeatOutcome>,
}

/// Register exactly `specs`' identities: each one's spec, its typed
/// refusal when Core holds its identity intent row or the first-boot
/// manifest is unreadable, and its seat in Core's mob, which carries its
/// pending first-boot row out. A held pass creates the fresh identities it
/// covers itself, so they are not seated. The roster-wide half (uniqueness,
/// topology, desired edges, the manifest read) stays with the caller.
///
/// One read of Core's member history answers every identity's host
/// adoption, and the seat step plans every identity from one more: no Core
/// read per identity.
async fn register_identity_metadata(
    runtime: &IdentityRuntime,
    specs: &[DurableAgentSpec],
    first_boot_unreadable: Option<&Arc<IdentityRuntimeError>>,
    customizer: Option<&dyn AgentCustomizer>,
    pass: RosterPass,
) -> Result<RegisteredIdentities, IdentityRuntimeError> {
    let roster = specs;
    let identities: Vec<AgentIdentity> = roster.iter().map(|s| s.identity.clone()).collect();
    // Reserve the full roster in one stable order, so a queued reset, delete
    // or raw-member creation cannot interleave with the registration.
    let mut authority_guards = acquire_roster_authority_guards(runtime, &identities).await?;
    let history = runtime.core_view_history().await;
    let held_pass = runtime.held_create_pass();

    let mut outcomes = BTreeMap::new();
    let mut to_seat = Vec::new();
    let mut listed = Vec::new();
    for spec in roster {
        let identity = &spec.identity;
        // Each reservation ends with its identity's registration: the seat
        // step takes the lifecycle locks of the identities it seats for its
        // Core step alone, so a host's call into another identity (a
        // customizer that sends to it while it builds a draft) never waits
        // on this registration.
        let (_lifecycle_guard, _alias_guard) =
            authority_guards.remove(identity).ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "validated roster reservation disappeared for {identity}"
                ))
            })?;
        runtime.register_locked(spec.clone()).await;
        let adopted = match &history {
            Ok(history) => IdentityRuntime::refuse_host_adopted_in(history.as_ref(), identity),
            Err(error) => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's identity management: {error}"),
            }),
        };
        if let Err(error) = adopted {
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
        // A lazy pass leaves a hosted identity Core does not hold for its
        // first use: it stays registered and dormant, and its first
        // delivery, or the background warm, creates it through its host.
        let hosted_left_for_first_use = pass == RosterPass::Lazy
            && super::runtime::durable_spec_is_hosted(spec)
            && !matches!(&history, Ok(Some(history)) if history
                .last_binding(&crate::member_comms_id::roster_member_id_for_identity(
                    identity.as_str(),
                ))
                .is_some_and(meerkat_mob::MemberBinding::is_live));
        if !hosted_left_for_first_use
            && held_pass.as_ref().is_none_or(|pass| !pass.covers(identity))
        {
            to_seat.push(spec.clone());
        }
    }

    // The roster is registered: the runtime is composed. Every member Core
    // built held before now has its customizer tools published and its hold
    // released, and every later build of a registered identity is prepared
    // inline ([`IdentityRuntime::compose_member_builds`]).
    runtime.compose_member_builds().await;

    let mut seats = BTreeMap::new();
    if runtime.has_session_bridge() {
        for (identity, result) in runtime.seat_identities(&to_seat, customizer).await {
            match result {
                Ok(seat) => {
                    seats.insert(identity, seat);
                }
                Err(error) => {
                    // A candidate launch's refusal defers the identity; it
                    // is no failure.
                    if super::runtime::candidate_deferral_of(&error).is_none() {
                        tracing::warn!(
                            %identity,
                            %error,
                            "the identity's seat step in Core's mob was refused; the next \
                             roster compile tries it again, and so does the identity's next \
                             delivery while Core does not hold it"
                        );
                    }
                    outcomes.insert(identity, RestoreOutcome::Refused(Arc::new(error)));
                }
            }
        }
    }
    for (identity, error) in runtime.respawn_unmarked_identities(&listed).await {
        // A candidate launch's refusal defers the identity; it is no failure.
        if super::runtime::candidate_deferral_of(&error).is_none() {
            tracing::warn!(
                %identity,
                %error,
                "the identity's member lacks the durable-identity marker and its respawn \
                 failed; the next roster compile tries it again"
            );
        }
        outcomes.insert(identity, RestoreOutcome::Refused(Arc::new(error)));
    }

    Ok(RegisteredIdentities { outcomes, seats })
}

/// The restore outcome each seated identity reports: Core binds it to its
/// own committed session (`Resumed`), or to a fresh session it minted
/// (`Created`). Both describe a member Core holds, materialized now or on
/// first touch. One read of Core's member history names each binding's
/// incarnation; an identity whose binding does not read keeps its
/// registration outcome.
async fn seated_restore_outcomes(
    runtime: &IdentityRuntime,
    mut outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    seats: &BTreeMap<AgentIdentity, SeatOutcome>,
) -> BTreeMap<AgentIdentity, RestoreOutcome> {
    if seats.is_empty() {
        return outcomes;
    }
    let history = runtime.core_view_history().await.ok().flatten();
    for (identity, seat) in seats {
        if !matches!(outcomes.get(identity), Some(RestoreOutcome::Dormant { .. })) {
            continue;
        }
        let Some(binding) = history
            .as_ref()
            .and_then(|history| seated_binding(history, identity))
        else {
            continue;
        };
        let outcome = if seat.minted {
            RestoreOutcome::Created {
                binding,
                draft: seat.draft.clone(),
            }
        } else {
            RestoreOutcome::Resumed {
                binding,
                draft: seat.draft.clone(),
            }
        };
        outcomes.insert(identity.clone(), outcome);
    }
    outcomes
}

/// The binding Core holds `identity` on, from its member history.
fn seated_binding(
    history: &meerkat_mob::MemberBindingHistory,
    identity: &AgentIdentity,
) -> Option<MemberRuntimeBinding> {
    let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
    let binding = history.last_binding(&member)?;
    Some(MemberRuntimeBinding {
        agent_runtime_id: super::runtime::live_binding_runtime_id(binding)?,
        session_id: binding.session_id().cloned(),
    })
}

/// Register one fresh member while the rest of the roster keeps running:
/// the per-identity half of [`restore_flow`] for exactly `spec`. A held pass
/// that covers it creates it held; otherwise it is seated in Core's mob
/// without being materialized. Nothing else is reconciled and the managed
/// peer edges are not touched, so the result carries this member's outcome
/// and no edges.
pub(crate) async fn add_member_flow(
    runtime: &IdentityRuntime,
    spec: &DurableAgentSpec,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    let first_boot_unreadable = runtime.load_first_boot_manifest().await.err().map(Arc::new);
    let registered = register_identity_metadata(
        runtime,
        std::slice::from_ref(spec),
        first_boot_unreadable.as_ref(),
        customizer,
        RosterPass::Default,
    )
    .await?;
    let mut outcomes =
        seated_restore_outcomes(runtime, registered.outcomes, &registered.seats).await;
    let initial = outcomes.remove(&spec.identity);
    let held_pass = runtime
        .held_create_pass()
        .filter(|pass| pass.covers(&spec.identity));
    let outcome = match held_pass {
        Some(held_pass) => {
            restore_registered_member(
                runtime,
                spec.clone(),
                initial,
                Some(held_pass),
                customizer,
                runtime.has_session_bridge(),
            )
            .await
        }
        None => {
            let outcome = initial.unwrap_or_else(|| {
                RestoreOutcome::Refused(Arc::new(IdentityRuntimeError::Internal(format!(
                    "validated registration outcome disappeared for {}",
                    spec.identity
                ))))
            });
            let error = match &outcome {
                RestoreOutcome::Refused(error) => Some(error.to_string()),
                _ => None,
            };
            if !matches!(
                &outcome,
                RestoreOutcome::Refused(error)
                    if runtime.defer_bootstrap_identity_on_candidate_refusal(&spec.identity, error)
            ) {
                runtime.publish_restore_progress(&spec.identity, outcome.restore_progress(), error);
            }
            outcome
        }
    };
    Ok(RestoreFlowResult {
        outcomes: BTreeMap::from([(spec.identity.clone(), outcome)]),
        managed_edges: Vec::new(),
    })
}
