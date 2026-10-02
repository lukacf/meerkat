//! Semantic coverage validation, shared by every catalog owner.
//!
//! These checks are pure: they validate attribution (every schema element has
//! a coverage entry, every claim names a real element of the right kind, no
//! entry is tautological, every anchor targets a real schema element). They do
//! not touch the filesystem, so they do not prove that an anchor's source file
//! exists or realizes the anchored semantics; the catalog owner checks that
//! (for Meerkat's own catalog, `xtask machine-check-drift`).
//!
//! The mode is always explicit. Meerkat's in-repo catalog validates with
//! [`CoverageValidationMode::RequireEntries`].

use std::collections::BTreeSet;

use super::coverage::{
    CompositionCoverageManifest, CoverageAnchor, CoverageClaims, CoverageSchemaTarget,
    MachineCoverageManifest, SemanticCoverageEntry, scheduler_rule_coverage_name,
};
use crate::{CompositionSchema, MachineSchema};

/// How strictly coverage entries must attribute their element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageValidationMode {
    /// Every schema element has a coverage entry and every claim resolves; an
    /// entry may list no anchors or scenarios when nothing fully describes the
    /// element (honestly unclaimed rather than mis-attributed). Meerkat's
    /// in-repo catalog validates in this mode.
    RequireEntries,
    /// As [`Self::RequireEntries`], and every entry also names at least one
    /// code anchor and at least one scenario.
    RequireClaims,
}

/// A coverage manifest or anchor that fails validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoverageValidationError {
    #[error("missing machine coverage manifest for {machine}")]
    MissingMachineManifest { machine: String },
    #[error("machine coverage manifest {machine} has no code anchors")]
    MachineManifestWithoutAnchors { machine: String },
    #[error("machine coverage manifest {machine} has no scenarios")]
    MachineManifestWithoutScenarios { machine: String },
    #[error("machine coverage manifest {machine} does not match a canonical machine")]
    UnmatchedMachineManifest { machine: String },
    #[error("missing composition coverage manifest for {composition}")]
    MissingCompositionManifest { composition: String },
    #[error("composition coverage manifest {composition} has no code anchors")]
    CompositionManifestWithoutAnchors { composition: String },
    #[error("composition coverage manifest {composition} has no scenarios")]
    CompositionManifestWithoutScenarios { composition: String },
    #[error("composition coverage manifest {composition} does not match a canonical composition")]
    UnmatchedCompositionManifest { composition: String },
    #[error("{owner} missing semantic coverage entry for {item_kind} `{name}`")]
    MissingEntry {
        owner: String,
        item_kind: &'static str,
        name: String,
    },
    #[error("{owner} semantic coverage entry `{name}` does not match a declared {item_kind}")]
    UndeclaredEntry {
        owner: String,
        item_kind: &'static str,
        name: String,
    },
    #[error(
        "{owner} semantic coverage entry `{name}` maps to every code anchor and every scenario; coverage must be semantic, not tautological"
    )]
    TautologicalEntry { owner: String, name: String },
    #[error(
        "{owner} semantic coverage entry `{name}` references unknown code anchor `{anchor_id}`"
    )]
    UnknownAnchorReference {
        owner: String,
        name: String,
        anchor_id: String,
    },
    #[error("{owner} semantic coverage entry `{name}` references unknown scenario `{scenario_id}`")]
    UnknownScenarioReference {
        owner: String,
        name: String,
        scenario_id: String,
    },
    #[error("{owner} semantic coverage entry `{name}` names no code anchor or no scenario")]
    UnclaimedEntry { owner: String, name: String },
    #[error(
        "{owner} {claimant_kind} `{claimant_id}` claims {kind} `{name}` but this manifest type declares no {kind}s"
    )]
    ClaimOfUndeclaredKind {
        owner: String,
        claimant_kind: &'static str,
        claimant_id: String,
        kind: &'static str,
        name: String,
    },
    #[error("{owner} {claimant_kind} `{claimant_id}` claims nonexistent {kind} `{name}`")]
    NonexistentClaim {
        owner: String,
        claimant_kind: &'static str,
        claimant_id: String,
        kind: &'static str,
        name: String,
    },
    #[error("machine coverage anchor `{anchor}` for {machine} targets unknown machine `{target}`")]
    MachineAnchorTargetsUnknownMachine {
        anchor: String,
        machine: String,
        target: String,
    },
    #[error(
        "machine coverage anchor `{anchor}` for {machine} targets route `{route}` but a machine coverage manifest declares no routes"
    )]
    MachineAnchorTargetsRoute {
        anchor: String,
        machine: String,
        route: String,
    },
    #[error(
        "composition coverage anchor `{anchor}` for {composition} targets undeclared route `{route}`"
    )]
    CompositionAnchorTargetsUndeclaredRoute {
        anchor: String,
        composition: String,
        route: String,
    },
    #[error(
        "composition coverage anchor `{anchor}` for {composition} targets unknown machine `{target}`"
    )]
    CompositionAnchorTargetsUnknownMachine {
        anchor: String,
        composition: String,
        target: String,
    },
}

/// Validate a whole catalog: every machine and composition has exactly the
/// manifest it needs, and every manifest validates against its schema.
pub fn validate_coverage_catalog(
    machines: &[MachineSchema],
    compositions: &[CompositionSchema],
    machine_manifests: &[MachineCoverageManifest],
    composition_manifests: &[CompositionCoverageManifest],
    mode: CoverageValidationMode,
) -> Result<(), CoverageValidationError> {
    for schema in machines {
        let manifest = machine_manifests
            .iter()
            .find(|manifest| manifest.machine == schema.machine)
            .ok_or_else(|| CoverageValidationError::MissingMachineManifest {
                machine: schema.machine.as_str().to_owned(),
            })?;
        validate_machine_coverage(schema, manifest, mode)?;
    }
    for manifest in machine_manifests {
        if !machines
            .iter()
            .any(|schema| schema.machine == manifest.machine)
        {
            return Err(CoverageValidationError::UnmatchedMachineManifest {
                machine: manifest.machine.as_str().to_owned(),
            });
        }
    }
    for schema in compositions {
        let manifest = composition_manifests
            .iter()
            .find(|manifest| manifest.composition == schema.name)
            .ok_or_else(|| CoverageValidationError::MissingCompositionManifest {
                composition: schema.name.as_str().to_owned(),
            })?;
        validate_composition_coverage(schema, manifest, mode)?;
    }
    for manifest in composition_manifests {
        if !compositions
            .iter()
            .any(|schema| schema.name == manifest.composition)
        {
            return Err(CoverageValidationError::UnmatchedCompositionManifest {
                composition: manifest.composition.as_str().to_owned(),
            });
        }
    }
    Ok(())
}

/// Validate one machine's coverage manifest against its schema.
pub fn validate_machine_coverage(
    schema: &MachineSchema,
    manifest: &MachineCoverageManifest,
    mode: CoverageValidationMode,
) -> Result<(), CoverageValidationError> {
    let machine = manifest.machine.as_str().to_owned();
    if manifest.code_anchors.is_empty() {
        return Err(CoverageValidationError::MachineManifestWithoutAnchors { machine });
    }
    if manifest.scenarios.is_empty() {
        return Err(CoverageValidationError::MachineManifestWithoutScenarios { machine });
    }
    let owner = format!("machine {}", schema.machine);
    let anchor_ids = id_set(
        manifest
            .code_anchors
            .iter()
            .map(|anchor| anchor.id.as_str()),
    );
    let scenario_ids = id_set(
        manifest
            .scenarios
            .iter()
            .map(|scenario| scenario.id.as_str()),
    );
    let transitions = id_set(
        schema
            .transitions
            .iter()
            .map(|transition| transition.name.as_str()),
    );
    let effects = id_set(
        schema
            .effects
            .variants
            .iter()
            .map(|variant| variant.name.as_str()),
    );
    let invariants = id_set(
        schema
            .invariants
            .iter()
            .map(|invariant| invariant.name.as_str()),
    );
    let declared = DeclaredKinds {
        transitions: Some(&transitions),
        effects: Some(&effects),
        invariants: &invariants,
        routes: None,
        scheduler_rules: None,
    };
    for (claimant_kind, claimant_id, claims) in
        claimants(&manifest.code_anchors, &manifest.scenarios)
    {
        validate_claims(&owner, claimant_kind, claimant_id, claims, &declared)?;
    }
    let refs = EntryRefs {
        owner: &owner,
        anchor_ids: &anchor_ids,
        scenario_ids: &scenario_ids,
        mode,
    };
    refs.validate("transition", &transitions, &manifest.transition_coverage)?;
    refs.validate("effect", &effects, &manifest.effect_coverage)?;
    refs.validate("invariant", &invariants, &manifest.invariant_coverage)?;
    Ok(())
}

/// Validate one composition's coverage manifest against its schema.
pub fn validate_composition_coverage(
    schema: &CompositionSchema,
    manifest: &CompositionCoverageManifest,
    mode: CoverageValidationMode,
) -> Result<(), CoverageValidationError> {
    let composition = manifest.composition.as_str().to_owned();
    if manifest.code_anchors.is_empty() {
        return Err(CoverageValidationError::CompositionManifestWithoutAnchors { composition });
    }
    if manifest.scenarios.is_empty() {
        return Err(CoverageValidationError::CompositionManifestWithoutScenarios { composition });
    }
    let owner = format!("composition {}", schema.name);
    let anchor_ids = id_set(
        manifest
            .code_anchors
            .iter()
            .map(|anchor| anchor.id.as_str()),
    );
    let scenario_ids = id_set(
        manifest
            .scenarios
            .iter()
            .map(|scenario| scenario.id.as_str()),
    );
    let routes = id_set(schema.routes.iter().map(|route| route.name.as_str()));
    let scheduler_rule_names = schema
        .scheduler_rules
        .iter()
        .map(scheduler_rule_coverage_name)
        .collect::<Vec<_>>();
    let scheduler_rules = id_set(scheduler_rule_names.iter().map(String::as_str));
    let invariants = id_set(
        schema
            .invariants
            .iter()
            .map(|invariant| invariant.name.as_str()),
    );
    let declared = DeclaredKinds {
        transitions: None,
        effects: None,
        invariants: &invariants,
        routes: Some(&routes),
        scheduler_rules: Some(&scheduler_rules),
    };
    for (claimant_kind, claimant_id, claims) in
        claimants(&manifest.code_anchors, &manifest.scenarios)
    {
        validate_claims(&owner, claimant_kind, claimant_id, claims, &declared)?;
    }
    let refs = EntryRefs {
        owner: &owner,
        anchor_ids: &anchor_ids,
        scenario_ids: &scenario_ids,
        mode,
    };
    refs.validate("route", &routes, &manifest.route_coverage)?;
    refs.validate(
        "scheduler rule",
        &scheduler_rules,
        &manifest.scheduler_rule_coverage,
    )?;
    refs.validate("invariant", &invariants, &manifest.invariant_coverage)?;
    Ok(())
}

/// Resolve a machine coverage anchor's target: it must name a known machine
/// (a machine manifest declares no routes).
pub fn validate_machine_anchor_target(
    machine: &str,
    anchor: &CoverageAnchor,
    known_machines: &BTreeSet<&str>,
) -> Result<(), CoverageValidationError> {
    match &anchor.target {
        CoverageSchemaTarget::Machine(target) if known_machines.contains(target.as_str()) => Ok(()),
        CoverageSchemaTarget::Machine(target) => Err(
            CoverageValidationError::MachineAnchorTargetsUnknownMachine {
                anchor: anchor.id.clone(),
                machine: machine.to_owned(),
                target: target.as_str().to_owned(),
            },
        ),
        CoverageSchemaTarget::Route(route) => {
            Err(CoverageValidationError::MachineAnchorTargetsRoute {
                anchor: anchor.id.clone(),
                machine: machine.to_owned(),
                route: route.as_str().to_owned(),
            })
        }
    }
}

/// Resolve a composition coverage anchor's target: a route the composition
/// declares, or a known machine.
pub fn validate_composition_anchor_target(
    schema: &CompositionSchema,
    anchor: &CoverageAnchor,
    known_machines: &BTreeSet<&str>,
) -> Result<(), CoverageValidationError> {
    match &anchor.target {
        CoverageSchemaTarget::Route(route) => {
            if schema.routes.iter().any(|declared| declared.name == *route) {
                Ok(())
            } else {
                Err(
                    CoverageValidationError::CompositionAnchorTargetsUndeclaredRoute {
                        anchor: anchor.id.clone(),
                        composition: schema.name.as_str().to_owned(),
                        route: route.as_str().to_owned(),
                    },
                )
            }
        }
        CoverageSchemaTarget::Machine(target) if known_machines.contains(target.as_str()) => Ok(()),
        CoverageSchemaTarget::Machine(target) => Err(
            CoverageValidationError::CompositionAnchorTargetsUnknownMachine {
                anchor: anchor.id.clone(),
                composition: schema.name.as_str().to_owned(),
                target: target.as_str().to_owned(),
            },
        ),
    }
}

/// Validate the coverage entries of one element kind: every declared element
/// has an entry, every entry names a declared element, no entry maps to every
/// anchor and every scenario, and every referenced anchor and scenario exists.
pub fn validate_semantic_entries(
    owner: &str,
    item_kind: &'static str,
    expected_names: &BTreeSet<&str>,
    entries: &[SemanticCoverageEntry],
    anchor_ids: &BTreeSet<&str>,
    scenario_ids: &BTreeSet<&str>,
    mode: CoverageValidationMode,
) -> Result<(), CoverageValidationError> {
    EntryRefs {
        owner,
        anchor_ids,
        scenario_ids,
        mode,
    }
    .validate(item_kind, expected_names, entries)
}

fn id_set<'a>(ids: impl Iterator<Item = &'a str>) -> BTreeSet<&'a str> {
    ids.collect()
}

fn claimants<'a>(
    anchors: &'a [CoverageAnchor],
    scenarios: &'a [super::coverage::ScenarioCoverage],
) -> impl Iterator<Item = (&'static str, &'a str, &'a CoverageClaims)> {
    anchors
        .iter()
        .map(|anchor| ("code anchor", anchor.id.as_str(), &anchor.claims))
        .chain(
            scenarios
                .iter()
                .map(|scenario| ("scenario", scenario.id.as_str(), &scenario.claims)),
        )
}

/// The element names a manifest type owns, per claim kind. `None` means the
/// manifest type structurally owns no elements of that kind, so any claim of
/// it is a mismatch.
struct DeclaredKinds<'a> {
    transitions: Option<&'a BTreeSet<&'a str>>,
    effects: Option<&'a BTreeSet<&'a str>>,
    invariants: &'a BTreeSet<&'a str>,
    routes: Option<&'a BTreeSet<&'a str>>,
    scheduler_rules: Option<&'a BTreeSet<&'a str>>,
}

fn validate_claims(
    owner: &str,
    claimant_kind: &'static str,
    claimant_id: &str,
    claims: &CoverageClaims,
    declared: &DeclaredKinds<'_>,
) -> Result<(), CoverageValidationError> {
    let rows: [(&'static str, Vec<&str>, Option<&BTreeSet<&str>>); 5] = [
        (
            "transition",
            claims.transitions.iter().map(|id| id.as_str()).collect(),
            declared.transitions,
        ),
        (
            "effect",
            claims.effects.iter().map(|id| id.as_str()).collect(),
            declared.effects,
        ),
        (
            "invariant",
            claims.invariants.iter().map(String::as_str).collect(),
            Some(declared.invariants),
        ),
        (
            "route",
            claims.routes.iter().map(|id| id.as_str()).collect(),
            declared.routes,
        ),
        (
            "scheduler rule",
            claims.scheduler_rules.iter().map(String::as_str).collect(),
            declared.scheduler_rules,
        ),
    ];
    for (kind, claimed, names) in rows {
        match names {
            None => {
                if let Some(first) = claimed.first() {
                    return Err(CoverageValidationError::ClaimOfUndeclaredKind {
                        owner: owner.to_owned(),
                        claimant_kind,
                        claimant_id: claimant_id.to_owned(),
                        kind,
                        name: (*first).to_owned(),
                    });
                }
            }
            Some(names) => {
                if let Some(missing) = claimed.iter().find(|name| !names.contains(*name)) {
                    return Err(CoverageValidationError::NonexistentClaim {
                        owner: owner.to_owned(),
                        claimant_kind,
                        claimant_id: claimant_id.to_owned(),
                        kind,
                        name: (*missing).to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

struct EntryRefs<'a> {
    owner: &'a str,
    anchor_ids: &'a BTreeSet<&'a str>,
    scenario_ids: &'a BTreeSet<&'a str>,
    mode: CoverageValidationMode,
}

impl EntryRefs<'_> {
    fn validate(
        &self,
        item_kind: &'static str,
        expected: &BTreeSet<&str>,
        entries: &[SemanticCoverageEntry],
    ) -> Result<(), CoverageValidationError> {
        let owner = || self.owner.to_owned();
        let seen = id_set(entries.iter().map(|entry| entry.name.as_str()));
        if let Some(name) = expected.iter().find(|name| !seen.contains(*name)) {
            return Err(CoverageValidationError::MissingEntry {
                owner: owner(),
                item_kind,
                name: (*name).to_owned(),
            });
        }
        for entry in entries {
            let name = || entry.name.clone();
            if !expected.contains(entry.name.as_str()) {
                return Err(CoverageValidationError::UndeclaredEntry {
                    owner: owner(),
                    item_kind,
                    name: name(),
                });
            }
            // An entry that lists no anchors or scenarios is honestly
            // unclaimed (nothing fully describes the element) rather than
            // mis-attributed; only RequireClaims refuses it. Claims that name
            // every anchor and every scenario are tautological in any mode.
            if self.anchor_ids.len() > 1
                && self.scenario_ids.len() > 1
                && entry.anchor_ids.len() == self.anchor_ids.len()
                && entry.scenario_ids.len() == self.scenario_ids.len()
            {
                return Err(CoverageValidationError::TautologicalEntry {
                    owner: owner(),
                    name: name(),
                });
            }
            if let Some(anchor_id) = entry
                .anchor_ids
                .iter()
                .find(|anchor_id| !self.anchor_ids.contains(anchor_id.as_str()))
            {
                return Err(CoverageValidationError::UnknownAnchorReference {
                    owner: owner(),
                    name: name(),
                    anchor_id: anchor_id.clone(),
                });
            }
            if let Some(scenario_id) = entry
                .scenario_ids
                .iter()
                .find(|scenario_id| !self.scenario_ids.contains(scenario_id.as_str()))
            {
                return Err(CoverageValidationError::UnknownScenarioReference {
                    owner: owner(),
                    name: name(),
                    scenario_id: scenario_id.clone(),
                });
            }
            if self.mode == CoverageValidationMode::RequireClaims
                && (entry.anchor_ids.is_empty() || entry.scenario_ids.is_empty())
            {
                return Err(CoverageValidationError::UnclaimedEntry {
                    owner: owner(),
                    name: name(),
                });
            }
        }
        Ok(())
    }
}
