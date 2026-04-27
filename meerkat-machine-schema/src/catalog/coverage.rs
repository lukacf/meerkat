use crate::{CompositionSchema, MachineSchema};

use super::{
    compositions::{
        auth_lease_bundle_composition, meerkat_mob_seam_composition, schedule_bundle_composition,
        schedule_mob_bundle_composition, schedule_runtime_bundle_composition,
    },
    dsl::{
        dsl_auth_machine, dsl_meerkat_machine, dsl_mob_machine, dsl_occurrence_lifecycle_machine,
        dsl_schedule_lifecycle_machine,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeAnchor {
    pub id: String,
    pub path: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioCoverage {
    pub id: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCoverageEntry {
    pub name: String,
    pub anchor_ids: Vec<String>,
    pub scenario_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineCoverageManifest {
    pub machine: crate::identity::MachineId,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub transition_coverage: Vec<SemanticCoverageEntry>,
    pub effect_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionCoverageManifest {
    pub composition: crate::identity::CompositionId,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub route_coverage: Vec<SemanticCoverageEntry>,
    pub scheduler_rule_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

pub fn canonical_machine_coverage_manifests() -> Vec<MachineCoverageManifest> {
    vec![
        machine_manifest_from_schema(
            &dsl_meerkat_machine(),
            &[
                anchor(
                    "meerkat_machine",
                    "meerkat-runtime/src/meerkat_machine/mod.rs",
                    "authoritative MeerkatMachine command dispatch and state ownership for initialize, register, unregister, reconfigure, stage filters and tools, prepare bindings, drain, interrupt, cancel boundary, cancellation, abort, wait, ingest, publish event, accept input, classify envelope, append/context starts, run preparation, commit, fail, pending/call/finalize tool surface, retire/retired, reset, stop/stopped executor, destroy/destroyed, ensure executor, runtime notice, silent intents, recycle, realtime binding, MCP server, interaction stream, product turn, live topology, ingress, supervisor, trust reconcile, ops barrier, local endpoint, admission, completion, compaction, submit op event, notify op watcher, collect/enqueue, terminal records, model routing status, set model routing baseline, finite switch turn, until changed switch turn, assistant turn admission, image operation begin activate complete restore, routing approval, routing denial, scoped override, and persistent reconfigure",
                ),
                anchor(
                    "meerkat_public_surface",
                    "meerkat/src/meerkat_machine.rs",
                    "MeerkatMachine snapshot/diagnostic facade",
                ),
                anchor(
                    "peer_directory_reachability_authority",
                    "meerkat-comms/src/peer_directory_reachability_authority.rs",
                    "peer directory reachability state now owned as a MeerkatMachine-internal region",
                ),
            ],
            &[
                scenario(
                    "bind-run-boundary-terminal",
                    "runtime binds, runs work, applies a boundary, and reports a terminal outcome",
                ),
                scenario(
                    "retire-reset-destroy",
                    "runtime retires, resets, stops, and destroys without reopening superseded work",
                ),
                scenario(
                    "staged_visibility_apply",
                    "tool visibility staged state promotes into the committed visible revision at a boundary",
                ),
                scenario(
                    "turn_interrupt_and_shutdown",
                    "running work records interrupt and shutdown intent without escaping the Meerkat authority boundary",
                ),
                scenario(
                    "peer_reachability_probe",
                    "resolved peer directory updates and send outcomes mutate Meerkat-owned peer reachability state",
                ),
                scenario(
                    "session_registration_and_binding",
                    "initialize, register, unregister, reconfigure session identity, prepare bindings, ensure executor, attach session ingress, detach ingress, drain exit, and runtime bound/retired/destroyed notices",
                ),
                scenario(
                    "input_admission_and_queueing",
                    "ingest and publish event, accept input with or without completion, classify external envelope or plain event, prepare run work, enqueue classified entry, resolve admission, submit admitted ingress effect, post admission signal, and input or ingress notices",
                ),
                scenario(
                    "ops_completion_and_waiters",
                    "abort, wait, abort all, request cancellation at boundary, completion produced/resolved, wait all satisfied, collect completed result, submit op event, notify op watcher, reject surface call, retain or evict completed terminal records",
                ),
                scenario(
                    "realtime_connection_projection",
                    "project realtime intent, begin replace detach binding, require reattach, publish signal, reconnect progress, MCP server connect/connected/failed/disconnected/reload, advance session context, interaction stream reserved/attached/completed/expired/closed early, freshness, policy, and binding rotation",
                ),
                scenario(
                    "product_turn_streaming",
                    "product turn in flight, committed, output started, interrupted, terminal, realtime projection advance/refreshed/reset, client input submitted, mid turn activity, and turn terminated classification",
                ),
                scenario(
                    "recycle_and_compaction",
                    "recycle from idle or retired, initiate recycle, check compaction, and re-enter ready runtime ownership without preserving stale completed records",
                ),
                scenario(
                    "model_routing_and_image_operation",
                    "set model routing baseline, request finite switch turn, request until changed switch turn, admit model routing assistant turn, begin image operation, activate image operation override, complete image operation, restore image operation override, project model routing status changed, switch turn denied, switch turn persistent reconfigure requested, switch turn finite override activated/restored, image operation phase changed/denied, and model routing approval terminalized",
                ),
                scenario(
                    "live_topology_and_supervision",
                    "begin live topology reconfigure, mark detached, apply identity or visibility, complete/abort/fail topology, bind/authorize/revoke supervisor, publish/revoke trust edge, comms trust reconcile, and local endpoint publish or clear",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_mob_machine(),
            &[
                anchor(
                    "mob_handle_surface",
                    "meerkat-mob/src/runtime/handle.rs",
                    "identity-first public MobMachine handle surface",
                ),
                anchor(
                    "mob_actor_authority",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine actor authority and command execution for wire, unwire, bind, rotate, release, spawn, observe runtime, submit work, retire, reset, respawn, complete, mark completed, stop/stopped, resume, task, force cancel, subscribe events, shutdown, destroy, terminalized member, record operator action provenance, flow, run, orchestrator, coordinator, cleanup, append failure ledger, escalate supervisor, peer, progress, notices, wiring graph, and session binding",
                ),
            ],
            &[
                scenario(
                    "spawn-work-terminal",
                    "member spawn, runtime-ready observation, work submission, and terminal work closure",
                ),
                scenario(
                    "retire-respawn-destroy",
                    "member retires, resets, respawns with a new runtime incarnation, stops/stopped, resumes, shuts down, destroys cleanly, and resets to running when reusable",
                ),
                scenario(
                    "wiring-and-session-binding",
                    "wire and unwire members, bind rotate release member session, enforce known identity for bindings, expose pending spawn, member session binding changed, and wiring lifecycle notices",
                ),
                scenario(
                    "task-flow-and-run-lifecycle",
                    "task create or update pending/in progress/completed/cancelled, run flow, start flow, create run, start run, complete flow, finish run, mark completed, flow terminalized, and force cancel running work",
                ),
                scenario(
                    "event-subscriptions-and-notices",
                    "subscribe agent, all agent, and mob events; emit member, run, flow, progress, task, terminal, and wiring notices",
                ),
                scenario(
                    "orchestrator-coordinator-cleanup",
                    "initialize, stop, resume, and destroy orchestrator; bind or unbind coordinator; begin and finish cleanup; notify coordinator and escalate supervisor",
                ),
                scenario(
                    "operator-provenance-and-peer-input",
                    "record operator action provenance, trust operation peer, admit peer input, append failure ledger, and surface peer-exposed member inputs",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_schedule_lifecycle_machine(),
            &[anchor(
                "schedule_lifecycle",
                "meerkat-schedule/src/lifecycle.rs",
                "Schedule::apply domain-facing lifecycle transition seam over create, revise, planning window, pause, resume, delete, supersede pending occurrences, revision, and planning cursor rules",
            )],
            &[
                scenario(
                    "schedule_pause_resume_delete",
                    "schedule transitions through create, pause, resume, and delete while advancing revision",
                ),
                scenario(
                    "schedule_revision_and_planning",
                    "active or paused schedules revise, record planning windows, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_occurrence_lifecycle_machine(),
            &[anchor(
                "occurrence_lifecycle",
                "meerkat-schedule/src/lifecycle.rs",
                "Occurrence::apply domain-facing lifecycle transition seam over claim, claimed, dispatch, await completion, complete, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification",
            )],
            &[
                scenario(
                    "occurrence_start_complete_fail",
                    "occurrence transitions through pending, running, and terminal lifecycle states",
                ),
                scenario(
                    "occurrence_claim_dispatch_completion",
                    "claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, and record claimed/dispatch/awaiting/completed effects",
                ),
                scenario(
                    "occurrence_terminal_classification",
                    "skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes",
                ),
                scenario(
                    "occurrence_lease_recovery",
                    "lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &dsl_auth_machine(),
            &[anchor(
                "auth_lease_handle",
                "meerkat-runtime/src/handles/auth_lease.rs",
                "per-binding AuthMachine registry; AuthLeaseHandle trait impl drives acquire, expiring, refresh, reauth, release, lifecycle event, and wake loop DSL transitions through it",
            )],
            &[
                scenario(
                    "acquire_expire_refresh_complete",
                    "lease transitions through valid, expiring, refreshing, and back to valid on successful refresh",
                ),
                scenario(
                    "reauth_release_and_publication",
                    "reauth required from valid/expiring/refreshing, release lease, emit lifecycle event, and wake refresh loop publication",
                ),
            ],
        ),
    ]
}

pub fn canonical_composition_coverage_manifests() -> Vec<CompositionCoverageManifest> {
    vec![
        composition_manifest_from_schema(
            &meerkat_mob_seam_composition(),
            &[
                anchor(
                    "mob_meerkat_seam",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine to MeerkatMachine seam realization for binding requests, work submission, cancellation, lifecycle notices, terminal outcomes, and peer ingress",
                ),
                anchor(
                    "meerkat_runtime_entry",
                    "meerkat-runtime/src/meerkat_machine/mod.rs",
                    "MeerkatMachine command authority consuming runtime binding, admitted work, cancellation, lifecycle, terminal, and peer ingress seam traffic",
                ),
            ],
            &[
                scenario(
                    "binding_round_trip",
                    "mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob",
                ),
                scenario(
                    "work_round_trip",
                    "mob submits work into Meerkat and observes terminal work outcomes back across the seam",
                ),
                scenario(
                    "peer-ingress-and-cancellation",
                    "peer input admission and cancellation requests cross the MobMachine to MeerkatMachine seam with explicit lifecycle notice feedback",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_bundle_composition(),
            &[
                anchor(
                    "schedule_service",
                    "meerkat-schedule/src/service.rs",
                    "schedule service precursor for revision supersession, rolling planning, occurrence materialization, pause resume, and delete lifecycle routing",
                ),
                anchor(
                    "schedule_store",
                    "meerkat-schedule/src/store.rs",
                    "schedule store contract precursor for transactional claim, supersede persistence, occurrence progress, and revision-aware planning cursor updates",
                ),
                anchor(
                    "schedule_bundle_schema",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule bundle composition",
                ),
            ],
            &[
                scenario(
                    "revision-supersede-route",
                    "revision-affecting schedule updates supersede pending future occurrences through the explicit route",
                ),
                scenario(
                    "pause-resume-without-revision",
                    "pause and resume leave schedule revision unchanged while preserving typed ownership",
                ),
                scenario(
                    "rolling-planning-occurrence-materialization",
                    "rolling planning records a planning window and materializes or supersedes pending occurrences through revision-aware schedule routes",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_runtime_bundle_composition(),
            &[
                anchor(
                    "schedule_driver",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for runtime-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback",
                ),
                anchor(
                    "runtime_delivery_precursor",
                    "meerkat-rpc/src/session_runtime.rs",
                    "runtime-owned prompt/event delivery precursor that scheduling must hand off into for dispatch, completion, failure, and lease recovery",
                ),
                anchor(
                    "schedule_runtime_bundle_schema",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule runtime bundle composition",
                ),
            ],
            &[
                scenario(
                    "runtime-delivery-feedback",
                    "DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback",
                ),
                scenario(
                    "runtime-lease-expiry",
                    "runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable",
                ),
                scenario(
                    "runtime-revision-supersede",
                    "schedule revision supersede enters occurrence authority before runtime handoff so stale pending work is cancelled explicitly",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &schedule_mob_bundle_composition(),
            &[
                anchor(
                    "schedule_driver",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for mob-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback",
                ),
                anchor(
                    "mob_delivery_precursor",
                    "meerkat-mob-mcp/src/lib.rs",
                    "mob-owned action delivery precursor that scheduling must hand off into for dispatch, completion, target materialization failure, and lease recovery",
                ),
                anchor(
                    "schedule_mob_bundle_schema",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal schedule mob bundle composition",
                ),
            ],
            &[
                scenario(
                    "mob-delivery-feedback",
                    "DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback",
                ),
                scenario(
                    "materialization-failure-classification",
                    "mob-side delivery failure preserves explicit TargetMaterializationFailed classification",
                ),
                scenario(
                    "mob-revision-supersede",
                    "schedule revision supersede enters occurrence authority before mob handoff so stale pending work is cancelled explicitly",
                ),
            ],
        ),
        composition_manifest_from_schema(
            &auth_lease_bundle_composition(),
            &[
                anchor(
                    "auth_lease_handle",
                    "meerkat-runtime/src/handles/auth_lease.rs",
                    "runtime auth lease owner consumes canonical AuthMachine lifecycle acquire, refresh, reauth, release, wake, and publication events",
                ),
                anchor(
                    "auth_lease_bundle_schema",
                    "meerkat-machine-schema/src/catalog/compositions.rs",
                    "formal AuthMachine lifecycle publication handoff composition",
                ),
            ],
            &[scenario(
                "auth-lease-lifecycle-publication",
                "AuthMachine acquire, refresh, reauth, release, wake, and lifecycle transitions publish through the explicit auth lease handoff protocol",
            )],
        ),
    ]
}

fn machine_manifest_from_schema(
    schema: &MachineSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> MachineCoverageManifest {
    MachineCoverageManifest {
        machine: schema.machine.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        transition_coverage: schema
            .transitions
            .iter()
            .map(|transition| SemanticCoverageEntry {
                name: transition.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(transition.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(transition.name.as_str(), scenarios),
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|effect| SemanticCoverageEntry {
                name: effect.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(effect.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(effect.name.as_str(), scenarios),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: semantic_anchor_ids(&invariant.name, code_anchors),
                scenario_ids: semantic_scenario_ids(&invariant.name, scenarios),
            })
            .collect(),
    }
}

fn composition_manifest_from_schema(
    schema: &CompositionSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> CompositionCoverageManifest {
    CompositionCoverageManifest {
        composition: schema.name.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        route_coverage: schema
            .routes
            .iter()
            .map(|route| SemanticCoverageEntry {
                name: route.name.as_str().to_owned(),
                anchor_ids: semantic_anchor_ids(route.name.as_str(), code_anchors),
                scenario_ids: semantic_scenario_ids(route.name.as_str(), scenarios),
            })
            .collect(),
        scheduler_rule_coverage: schema
            .scheduler_rules
            .iter()
            .map(|rule| SemanticCoverageEntry {
                name: format!("{rule:?}"),
                anchor_ids: semantic_anchor_ids(&format!("{rule:?}"), code_anchors),
                scenario_ids: semantic_scenario_ids(&format!("{rule:?}"), scenarios),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: semantic_anchor_ids(&invariant.name, code_anchors),
                scenario_ids: semantic_scenario_ids(&invariant.name, scenarios),
            })
            .collect(),
    }
}

fn semantic_anchor_ids(name: &str, anchors: &[CodeAnchor]) -> Vec<String> {
    semantic_ids(
        name,
        anchors,
        |anchor| anchor.id.as_str(),
        |anchor| anchor.note.as_str(),
    )
}

fn semantic_scenario_ids(name: &str, scenarios: &[ScenarioCoverage]) -> Vec<String> {
    semantic_ids(
        name,
        scenarios,
        |scenario| scenario.id.as_str(),
        |scenario| scenario.summary.as_str(),
    )
}

fn semantic_ids<T>(
    name: &str,
    items: &[T],
    id: impl Fn(&T) -> &str,
    description: impl Fn(&T) -> &str,
) -> Vec<String> {
    if items.is_empty() {
        return Vec::new();
    }

    let tokens = semantic_tokens(name);
    let scored = items
        .iter()
        .map(|item| {
            let haystack = format!("{} {}", id(item), description(item)).to_ascii_lowercase();
            let score = tokens
                .iter()
                .filter(|token| haystack.contains(token.as_str()))
                .count();
            (item, score)
        })
        .collect::<Vec<_>>();
    let max_score = scored.iter().map(|(_, score)| *score).max().unwrap_or(0);
    if max_score == 0 {
        return Vec::new();
    }

    scored
        .into_iter()
        .filter(|(_, score)| *score == max_score)
        .map(|(item, _)| id(item).to_owned())
        .collect::<Vec<_>>()
}

fn semantic_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in name.chars() {
        if ch == '_' || ch == '-' || ch == ' ' || ch == '(' || ch == ')' || ch == ',' {
            if current.len() >= 3 {
                tokens.push(current.to_ascii_lowercase());
            }
            current.clear();
            previous_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower && current.len() >= 3 {
            tokens.push(current.to_ascii_lowercase());
            current.clear();
        }
        previous_lower = ch.is_ascii_lowercase();
        current.push(ch);
    }
    if current.len() >= 3 {
        tokens.push(current.to_ascii_lowercase());
    }
    tokens
}

fn anchor(id: &str, path: &str, note: &str) -> CodeAnchor {
    CodeAnchor {
        id: id.into(),
        path: path.into(),
        note: note.into(),
    }
}

fn scenario(id: &str, summary: &str) -> ScenarioCoverage {
    ScenarioCoverage {
        id: id.into(),
        summary: summary.into(),
    }
}
