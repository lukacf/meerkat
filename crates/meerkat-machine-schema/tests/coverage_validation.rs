#![allow(clippy::expect_used, clippy::panic)]

//! Parity for the shared coverage validator: the in-repo catalog validates in
//! RequireEntries mode exactly as xtask's private validator did, and every
//! refusal reports the message xtask reported (the expected strings below are
//! the old xtask format strings, filled in).

use std::collections::BTreeSet;

use meerkat_machine_schema::identity::{CompositionId, MachineId, RouteId, TransitionId};
use meerkat_machine_schema::{
    CompositionCoverageManifest, CompositionSchema, CoverageSchemaTarget, CoverageValidationError,
    CoverageValidationMode, MachineCoverageManifest, MachineSchema, SemanticCoverageEntry,
    canonical_composition_coverage_manifests, canonical_composition_schemas,
    canonical_machine_coverage_manifests, canonical_machine_schemas,
    validate_composition_anchor_target, validate_coverage_catalog, validate_machine_anchor_target,
};

const ENTRIES: CoverageValidationMode = CoverageValidationMode::RequireEntries;

struct Catalog {
    machines: Vec<MachineSchema>,
    compositions: Vec<CompositionSchema>,
    machine_manifests: Vec<MachineCoverageManifest>,
    composition_manifests: Vec<CompositionCoverageManifest>,
}

impl Catalog {
    fn canonical() -> Self {
        Self {
            machines: canonical_machine_schemas(),
            compositions: canonical_composition_schemas(),
            machine_manifests: canonical_machine_coverage_manifests(),
            composition_manifests: canonical_composition_coverage_manifests(),
        }
    }

    fn validate(&self, mode: CoverageValidationMode) -> Result<(), CoverageValidationError> {
        validate_coverage_catalog(
            &self.machines,
            &self.compositions,
            &self.machine_manifests,
            &self.composition_manifests,
            mode,
        )
    }

    fn refusal(&self) -> String {
        self.validate(ENTRIES)
            .expect_err("the mutated catalog must be refused")
            .to_string()
    }

    /// The first machine manifest with at least two anchors, two scenarios
    /// and a transition entry, and its machine name.
    fn machine_manifest(&mut self) -> (&mut MachineCoverageManifest, String) {
        let manifest = self
            .machine_manifests
            .iter_mut()
            .find(|manifest| {
                manifest.code_anchors.len() > 1
                    && manifest.scenarios.len() > 1
                    && !manifest.transition_coverage.is_empty()
            })
            .expect("a rich machine manifest");
        let name = manifest.machine.as_str().to_owned();
        (manifest, name)
    }

    fn composition_manifest(&mut self) -> (&mut CompositionCoverageManifest, String) {
        let manifest = self
            .composition_manifests
            .iter_mut()
            .find(|manifest| !manifest.route_coverage.is_empty())
            .expect("a composition manifest with routes");
        let name = manifest.composition.as_str().to_owned();
        (manifest, name)
    }
}

#[test]
fn the_in_repo_catalog_validates_in_require_entries_mode() {
    Catalog::canonical()
        .validate(ENTRIES)
        .expect("the canonical catalog validates exactly as before");
}

#[test]
fn require_claims_refuses_an_honestly_unclaimed_entry_that_require_entries_permits() {
    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    manifest.transition_coverage[0].anchor_ids.clear();
    let entry = manifest.transition_coverage[0].name.clone();
    catalog
        .validate(ENTRIES)
        .expect("an unclaimed entry is permitted");
    assert_eq!(
        catalog
            .validate(CoverageValidationMode::RequireClaims)
            .expect_err("RequireClaims refuses it")
            .to_string(),
        format!(
            "machine {machine} semantic coverage entry `{entry}` names no code anchor or no scenario"
        )
    );
}

#[test]
fn catalog_level_refusals_keep_their_messages() {
    let mut catalog = Catalog::canonical();
    let removed = catalog.machine_manifests.remove(0);
    assert_eq!(
        catalog.refusal(),
        format!("missing machine coverage manifest for {}", removed.machine)
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    manifest.code_anchors.clear();
    assert_eq!(
        catalog.refusal(),
        format!("machine coverage manifest {machine} has no code anchors")
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    manifest.scenarios.clear();
    assert_eq!(
        catalog.refusal(),
        format!("machine coverage manifest {machine} has no scenarios")
    );

    let mut catalog = Catalog::canonical();
    let mut extra = catalog.machine_manifests[0].clone();
    extra.machine = MachineId::parse("NotAMachine").expect("machine slug");
    catalog.machine_manifests.push(extra);
    assert_eq!(
        catalog.refusal(),
        "machine coverage manifest NotAMachine does not match a canonical machine"
    );

    let mut catalog = Catalog::canonical();
    let removed = catalog.composition_manifests.remove(0);
    assert_eq!(
        catalog.refusal(),
        format!(
            "missing composition coverage manifest for {}",
            removed.composition
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, composition) = catalog.composition_manifest();
    manifest.code_anchors.clear();
    assert_eq!(
        catalog.refusal(),
        format!("composition coverage manifest {composition} has no code anchors")
    );

    let mut catalog = Catalog::canonical();
    let (manifest, composition) = catalog.composition_manifest();
    manifest.scenarios.clear();
    assert_eq!(
        catalog.refusal(),
        format!("composition coverage manifest {composition} has no scenarios")
    );

    let mut catalog = Catalog::canonical();
    let mut extra = catalog.composition_manifests[0].clone();
    extra.composition = CompositionId::parse("not_a_composition").expect("composition slug");
    catalog.composition_manifests.push(extra);
    assert_eq!(
        catalog.refusal(),
        "composition coverage manifest not_a_composition does not match a canonical composition"
    );
}

#[test]
fn entry_and_claim_refusals_keep_their_messages() {
    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let removed = manifest.transition_coverage.remove(0);
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} missing semantic coverage entry for transition `{}`",
            removed.name
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    manifest.transition_coverage.push(SemanticCoverageEntry {
        name: "NoSuchTransition".into(),
        anchor_ids: vec![],
        scenario_ids: vec![],
    });
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} semantic coverage entry `NoSuchTransition` does not match a declared transition"
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let anchors = manifest
        .code_anchors
        .iter()
        .map(|anchor| anchor.id.clone())
        .collect::<Vec<_>>();
    let scenarios = manifest
        .scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect::<Vec<_>>();
    let entry = &mut manifest.transition_coverage[0];
    entry.anchor_ids = anchors;
    entry.scenario_ids = scenarios;
    let name = entry.name.clone();
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} semantic coverage entry `{name}` maps to every code anchor and every scenario; coverage must be semantic, not tautological"
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let entry = &mut manifest.transition_coverage[0];
    entry.anchor_ids = vec!["no_such_anchor".into()];
    let name = entry.name.clone();
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} semantic coverage entry `{name}` references unknown code anchor `no_such_anchor`"
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let entry = &mut manifest.transition_coverage[0];
    entry.anchor_ids.clear();
    entry.scenario_ids = vec!["no_such_scenario".into()];
    let name = entry.name.clone();
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} semantic coverage entry `{name}` references unknown scenario `no_such_scenario`"
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let anchor = &mut manifest.code_anchors[0];
    anchor
        .claims
        .routes
        .push(RouteId::parse("some_route").expect("route slug"));
    let anchor_id = anchor.id.clone();
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} code anchor `{anchor_id}` claims route `some_route` but this manifest type declares no routes"
        )
    );

    let mut catalog = Catalog::canonical();
    let (manifest, machine) = catalog.machine_manifest();
    let scenario = &mut manifest.scenarios[0];
    scenario
        .claims
        .transitions
        .push(TransitionId::parse("NoSuchTransition").expect("transition slug"));
    let scenario_id = scenario.id.clone();
    assert_eq!(
        catalog.refusal(),
        format!(
            "machine {machine} scenario `{scenario_id}` claims nonexistent transition `NoSuchTransition`"
        )
    );
}

#[test]
fn anchor_target_refusals_keep_their_messages() {
    let catalog = Catalog::canonical();
    let known = catalog
        .machines
        .iter()
        .map(|machine| machine.machine.as_str())
        .collect::<BTreeSet<_>>();

    let machine_manifest = &catalog.machine_manifests[0];
    let machine = machine_manifest.machine.as_str();
    let mut anchor = machine_manifest.code_anchors[0].clone();
    validate_machine_anchor_target(machine, &anchor, &known).expect("canonical target resolves");
    anchor.target = CoverageSchemaTarget::Machine(MachineId::parse("NotAMachine").expect("slug"));
    assert_eq!(
        validate_machine_anchor_target(machine, &anchor, &known)
            .expect_err("unknown machine")
            .to_string(),
        format!(
            "machine coverage anchor `{}` for {machine} targets unknown machine `NotAMachine`",
            anchor.id
        )
    );
    anchor.target = CoverageSchemaTarget::Route(RouteId::parse("some_route").expect("slug"));
    assert_eq!(
        validate_machine_anchor_target(machine, &anchor, &known)
            .expect_err("route in a machine manifest")
            .to_string(),
        format!(
            "machine coverage anchor `{}` for {machine} targets route `some_route` but a machine coverage manifest declares no routes",
            anchor.id
        )
    );

    let composition = catalog
        .compositions
        .iter()
        .find(|composition| !composition.routes.is_empty())
        .expect("a composition with routes");
    let manifest = catalog
        .composition_manifests
        .iter()
        .find(|manifest| manifest.composition == composition.name)
        .expect("its manifest");
    let mut anchor = manifest.code_anchors[0].clone();
    validate_composition_anchor_target(composition, &anchor, &known)
        .expect("canonical target resolves");
    anchor.target = CoverageSchemaTarget::Route(RouteId::parse("undeclared_route").expect("slug"));
    assert_eq!(
        validate_composition_anchor_target(composition, &anchor, &known)
            .expect_err("undeclared route")
            .to_string(),
        format!(
            "composition coverage anchor `{}` for {} targets undeclared route `undeclared_route`",
            anchor.id, composition.name
        )
    );
    anchor.target = CoverageSchemaTarget::Machine(MachineId::parse("NotAMachine").expect("slug"));
    assert_eq!(
        validate_composition_anchor_target(composition, &anchor, &known)
            .expect_err("unknown machine")
            .to_string(),
        format!(
            "composition coverage anchor `{}` for {} targets unknown machine `NotAMachine`",
            anchor.id, composition.name
        )
    );
}
