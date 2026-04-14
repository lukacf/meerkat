use crate::{CompositionSchema, MachineSchema};

use super::{
    compositions::{
        meerkat_mob_seam_composition, schedule_bundle_composition, schedule_mob_bundle_composition,
        schedule_runtime_bundle_composition,
    },
    meerkat_machine::meerkat_machine,
    mob_machine::mob_machine,
    occurrence_lifecycle::occurrence_lifecycle_machine,
    schedule_lifecycle::schedule_lifecycle_machine,
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
    pub machine: String,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub transition_coverage: Vec<SemanticCoverageEntry>,
    pub effect_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionCoverageManifest {
    pub composition: String,
    pub code_anchors: Vec<CodeAnchor>,
    pub scenarios: Vec<ScenarioCoverage>,
    pub route_coverage: Vec<SemanticCoverageEntry>,
    pub scheduler_rule_coverage: Vec<SemanticCoverageEntry>,
    pub invariant_coverage: Vec<SemanticCoverageEntry>,
}

pub fn canonical_machine_coverage_manifests() -> Vec<MachineCoverageManifest> {
    vec![
        machine_manifest_from_schema(
            &meerkat_machine(),
            &[
                anchor(
                    "meerkat_machine",
                    "meerkat-runtime/src/meerkat_machine.rs",
                    "authoritative MeerkatMachine command dispatch and state ownership",
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
            ],
        ),
        machine_manifest_from_schema(
            &mob_machine(),
            &[
                anchor(
                    "mob_handle_surface",
                    "meerkat-mob/src/runtime/handle.rs",
                    "identity-first public MobMachine handle surface",
                ),
                anchor(
                    "mob_actor_authority",
                    "meerkat-mob/src/runtime/actor.rs",
                    "MobMachine actor authority and command execution",
                ),
            ],
            &[
                scenario(
                    "spawn-work-terminal",
                    "member spawn, runtime-ready observation, work submission, and terminal work closure",
                ),
                scenario(
                    "retire-respawn-destroy",
                    "member retires, respawns with a new runtime incarnation, and destroys cleanly",
                ),
            ],
        ),
        machine_manifest_from_schema(
            &schedule_lifecycle_machine(),
            &[anchor(
                "schedule_authority",
                "meerkat-schedule/src/authority.rs",
                "schedule lifecycle authority and revision ownership",
            )],
            &[scenario(
                "schedule_pause_resume_delete",
                "schedule transitions through create, pause, resume, and delete while advancing revision",
            )],
        ),
        machine_manifest_from_schema(
            &occurrence_lifecycle_machine(),
            &[anchor(
                "occurrence_authority",
                "meerkat-schedule/src/authority.rs",
                "occurrence lifecycle authority",
            )],
            &[scenario(
                "occurrence_start_complete_fail",
                "occurrence transitions through pending, running, and terminal lifecycle states",
            )],
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
                    "MobMachine to MeerkatMachine seam realization",
                ),
                anchor(
                    "meerkat_runtime_entry",
                    "meerkat-runtime/src/meerkat_machine.rs",
                    "MeerkatMachine command authority consuming seam traffic",
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
            ],
        ),
        composition_manifest_from_schema(
            &schedule_bundle_composition(),
            &[
                anchor(
                    "schedule_service",
                    "meerkat-schedule/src/service.rs",
                    "schedule service precursor for revision supersession and rolling planning",
                ),
                anchor(
                    "schedule_store",
                    "meerkat-schedule/src/store.rs",
                    "schedule store contract precursor for transactional claim and supersede persistence",
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
            ],
        ),
        composition_manifest_from_schema(
            &schedule_runtime_bundle_composition(),
            &[
                anchor(
                    "schedule_driver",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for runtime-target claim, handoff, and feedback",
                ),
                anchor(
                    "runtime_delivery_precursor",
                    "meerkat-rpc/src/session_runtime.rs",
                    "runtime-owned prompt/event delivery precursor that scheduling must hand off into",
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
            ],
        ),
        composition_manifest_from_schema(
            &schedule_mob_bundle_composition(),
            &[
                anchor(
                    "schedule_driver",
                    "meerkat-schedule/src/driver.rs",
                    "mechanical scheduler driver precursor for mob-target claim, handoff, and feedback",
                ),
                anchor(
                    "mob_delivery_precursor",
                    "meerkat-mob-mcp/src/lib.rs",
                    "mob-owned action delivery precursor that scheduling must hand off into",
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
            ],
        ),
    ]
}

fn machine_manifest_from_schema(
    schema: &MachineSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> MachineCoverageManifest {
    let scenario_ids: Vec<String> = scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect();
    let anchor_ids: Vec<String> = code_anchors
        .iter()
        .map(|anchor| anchor.id.clone())
        .collect();

    MachineCoverageManifest {
        machine: schema.machine.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        transition_coverage: schema
            .transitions
            .iter()
            .map(|transition| SemanticCoverageEntry {
                name: transition.name.clone(),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
        effect_coverage: schema
            .effects
            .variants
            .iter()
            .map(|effect| SemanticCoverageEntry {
                name: effect.name.clone(),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
    }
}

fn composition_manifest_from_schema(
    schema: &CompositionSchema,
    code_anchors: &[CodeAnchor],
    scenarios: &[ScenarioCoverage],
) -> CompositionCoverageManifest {
    let scenario_ids: Vec<String> = scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect();
    let anchor_ids: Vec<String> = code_anchors
        .iter()
        .map(|anchor| anchor.id.clone())
        .collect();

    CompositionCoverageManifest {
        composition: schema.name.clone(),
        code_anchors: code_anchors.to_vec(),
        scenarios: scenarios.to_vec(),
        route_coverage: schema
            .routes
            .iter()
            .map(|route| SemanticCoverageEntry {
                name: route.name.clone(),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
        scheduler_rule_coverage: schema
            .scheduler_rules
            .iter()
            .map(|rule| SemanticCoverageEntry {
                name: format!("{rule:?}"),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
        invariant_coverage: schema
            .invariants
            .iter()
            .map(|invariant| SemanticCoverageEntry {
                name: invariant.name.clone(),
                anchor_ids: anchor_ids.clone(),
                scenario_ids: scenario_ids.clone(),
            })
            .collect(),
    }
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
