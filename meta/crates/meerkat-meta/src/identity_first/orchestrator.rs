//! Orchestrator: restore flow, reconciliation, and topology wiring.
//!
//! Implements REQ-12 (restore flow), REQ-13 (broken continuity), REQ-19/REQ-21
//! (topology reconciliation), REQ-20 (static wiring preserved), REQ-22 (topology
//! not continuity truth), and REQ-33 (identity-keyed reconciliation).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use futures::{StreamExt, stream};

use std::sync::Arc;

use super::bridge::{MemberRuntimeBinding, UnlistedIdentityAnswer};
use super::contracts::{AgentCustomizer, TopologyProvider};
use super::runtime::{
    EmbodimentOverrides, HostedIdentities, IdentityRuntime, IdentityRuntimeError,
};
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
    /// What the host's whole roster did with each identity Core held
    /// declared that it does not list: retired, removed, or failed to retire
    /// (the next pass retires it again). Empty for a partial restore.
    pub unlisted: BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
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

/// Execute the full restore flow per REQ-12 sequencing, for part of the
/// host's roster: no identity it does not list is unlisted (the complete
/// roster's flow is the private `restore_roster`).
pub async fn restore_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    restore_roster(runtime, roster, topology_provider, customizer, false).await
}

/// Execute the full restore flow per REQ-12 sequencing.
///
/// Steps:
/// 1. Validate the roster and compute topology.
/// 2. Declare the roster in Core's mob, through Core's declared apply
///    (`IdentityRuntime::declare_roster_pass`): the host's whole roster when
///    `complete`, so Core also unlists each identity it holds declared that
///    the roster does not list and retires its member. Core's MobMachine
///    decides each identity's verdict and Core realizes it: an identity Core
///    keeps needs nothing, one it holds no member for is seated Dormant (on
///    its archived session, or its pending first-boot row's), a declared
///    role migration is carried out, an undeclared one held. Core
///    materializes each member on its first delivery, or at boot when its
///    own obligation facts say the member owes work. Meta wakes nothing,
///    and creates only an identity hosted on its first use, through its
///    host.
/// 3. A held pass creates its fresh members itself, held, through the
///    identity's create (`IdentityRuntime::embody_identity`).
/// 4. A refusal is a typed `Refused` outcome for that identity only; the
///    remaining identities go on.
///
/// Without a session bridge the flow is validation-only: every outcome is
/// `Dormant` and nothing is declared.
pub(crate) async fn restore_roster(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    customizer: Option<&dyn AgentCustomizer>,
    complete: bool,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    // Fleet-level declaration and topology remain fail-closed: without a
    // valid unique roster there is no truthful set of identities to seat.
    let registered = register_roster_metadata(
        runtime,
        roster,
        topology_provider,
        RosterPass::Default,
        customizer,
        complete,
    )
    .await?;
    let managed_edges = registered.managed_edges;
    let mut outcomes = registered.outcomes;
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
        unlisted: registered.unlisted,
    })
}

/// Register the roster and topology metadata, and make every identity a
/// member of Core's mob without materializing any, for part of the host's
/// roster: no identity it does not list is unlisted (the complete roster's
/// registration is the private `lazy_register_roster`).
pub async fn lazy_register_flow(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    lazy_register_roster(runtime, roster, topology_provider, false).await
}

/// Register the roster and topology metadata, and declare the roster in
/// Core's mob, the host's whole roster when `complete`, without
/// materializing any member.
///
/// The explicit lazy bootstrap modes run this. Every listed identity Core
/// can seat is a member of Core's mob afterwards, as under the default, so
/// peers reach it; Core materializes it on demand. A placed or externally
/// bound identity Core does not hold is left for its first use, as these
/// modes always left it: its host creates it, so a create at the pass would
/// read broken while that host is not bound yet. A lazy reconcile also
/// reconciles the managed topology, since no restore pass follows it.
pub(crate) async fn lazy_register_roster(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    complete: bool,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    let registered = register_roster_metadata(
        runtime,
        roster,
        topology_provider,
        RosterPass::Lazy,
        None,
        complete,
    )
    .await?;
    // A lazy pass wakes nothing: each identity it registered is reported
    // registered without a wake (`Dormant`, with the build it was declared
    // with), whatever Core seated or kept for it, as the lazy modes have
    // always reported it; a refusal stays typed.
    let outcomes = registered
        .outcomes
        .into_iter()
        .map(|(identity, outcome)| {
            let outcome = match outcome {
                RestoreOutcome::Created { draft, .. } | RestoreOutcome::Resumed { draft, .. } => {
                    RestoreOutcome::Dormant { draft }
                }
                outcome @ (RestoreOutcome::Dormant { .. } | RestoreOutcome::Refused(_)) => outcome,
            };
            (identity, outcome)
        })
        .collect();
    Ok(RestoreFlowResult {
        outcomes,
        managed_edges: registered.managed_edges,
        unlisted: registered.unlisted,
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
    /// Each identity's outcome: refused typed, registered (`Dormant` with
    /// the spec's build), or a member Core holds.
    outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    /// What a complete roster did with each identity it does not list.
    unlisted: BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
    managed_edges: Vec<ManagedPeerEdge>,
}

/// Validate the roster and topology, then declare the roster in Core's mob
/// ([`IdentityRuntime::declare_roster_pass`]), the host's whole roster when
/// `complete`, registering each listed identity whatever Core answered for
/// it: every listed identity Core can seat is a member of Core's
/// mob afterwards, and none is materialized. Meta keeps no Core fact: the
/// registration is the roster's policy only, and every Core fact is read
/// per call. A roster entry naming a host-adopted identity is registered
/// for the views and reported as a typed refusal: no Meta verb acts on it.
/// A lazy reconcile (`RosterPass::Lazy`) leaves an identity hosted on its
/// first use for it, and also reconciles the managed topology, since no
/// restore pass follows it.
///
/// The first-boot manifest is read first. A manifest the reader refuses
/// refuses every identity of the compile, typed, and nothing is declared:
/// which identities it covers is unknown. The compile itself goes on, so
/// the gateway stays up and its status names the file and the refusal. The
/// pending row of each identity the roster lists is carried out by the
/// identity's declaration; a row Core's history contradicts makes that
/// identity's outcome a typed refusal, and the rest of the roster goes on
/// (a complete roster then unlists nothing this time). A row whose
/// identity the roster does not list stays pending, unless Core's member
/// history shows it done (a row for a helper or a fork child the host made
/// on the mob and retired, which `storage-migrate-native` before 0.9.5
/// wrote): such a row is settled
/// ([`IdentityRuntime::settle_done_unlisted_first_boot_rows`]).
async fn register_roster_metadata(
    runtime: &IdentityRuntime,
    roster: &[DurableAgentSpec],
    topology_provider: Option<&dyn TopologyProvider>,
    pass: RosterPass,
    customizer: Option<&dyn AgentCustomizer>,
    complete: bool,
) -> Result<RegisteredRoster, IdentityRuntimeError> {
    IdentityRuntime::validate_roster_uniqueness(roster)?;

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

    let hosted = match pass {
        RosterPass::Default => HostedIdentities::CreateNow,
        RosterPass::Lazy => HostedIdentities::LeaveForFirstUse,
    };
    let declared = runtime
        .declare_roster_pass(roster, complete, true, customizer, hosted)
        .await;
    // The rows of identities this roster does not list wait for a roster
    // that lists them, unless Core's history already shows them done.
    if runtime.load_first_boot_manifest().await.is_ok() {
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
        outcomes: declared.outcomes,
        unlisted: declared.unlisted,
        managed_edges,
    })
}

/// Register one fresh member while the rest of the roster keeps running:
/// the per-identity half of [`restore_flow`] for exactly `spec`, declared in
/// Core's mob as a one-identity partial roster. A held pass that covers it
/// creates it held; otherwise Core seats it without materializing it.
/// Nothing else is reconciled and the managed peer edges are not touched, so
/// the result carries this member's outcome and no edges.
pub(crate) async fn add_member_flow(
    runtime: &IdentityRuntime,
    spec: &DurableAgentSpec,
    customizer: Option<&dyn AgentCustomizer>,
) -> Result<RestoreFlowResult, IdentityRuntimeError> {
    let mut declared = runtime
        .declare_roster_pass(
            std::slice::from_ref(spec),
            false,
            true,
            customizer,
            HostedIdentities::CreateNow,
        )
        .await;
    let initial = declared.outcomes.remove(&spec.identity);
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
        unlisted: BTreeMap::new(),
    })
}
