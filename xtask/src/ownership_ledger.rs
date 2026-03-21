use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};
use syn::{ImplItem, Item, ItemFn, ItemImpl, Type};

use crate::public_contracts::repo_root;

const DOC_PATH: &str = "docs/architecture/finite-ownership-ledger.md";
const BASELINE_PATH: &str = "xtask/ownership-baseline.toml";

#[derive(Debug, Clone, Args, Default)]
pub struct OwnershipLedgerArgs {
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub check_drift: bool,
    #[arg(long)]
    pub update_baseline: bool,
    #[arg(long)]
    pub write_doc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subsystem {
    Runtime,
    Mcp,
    Mob,
}

impl fmt::Display for Subsystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime => write!(f, "runtime"),
            Self::Mcp => write!(f, "mcp"),
            Self::Mob => write!(f, "mob"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    Open,
    Closed,
}

impl fmt::Display for EntryStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::Closed => write!(f, "closed"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateClass {
    MachineOwned,
    DerivedProjection,
    CapabilityHandle,
    CapabilityIndex,
    TransportBuffer,
    UnownedSemantic,
}

impl fmt::Display for StateClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MachineOwned => write!(f, "machine-owned"),
            Self::DerivedProjection => write!(f, "derived-projection"),
            Self::CapabilityHandle => write!(f, "capability-handle"),
            Self::CapabilityIndex => write!(f, "capability-index"),
            Self::TransportBuffer => write!(f, "transport-buffer"),
            Self::UnownedSemantic => write!(f, "unowned-semantic"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StalenessPolicy {
    Forbidden,
    ObservabilityOnly,
    BoundedReadOnly,
}

impl fmt::Display for StalenessPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forbidden => write!(f, "forbidden"),
            Self::ObservabilityOnly => write!(f, "observability_only"),
            Self::BoundedReadOnly => write!(f, "bounded_read_only"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectionContract {
    pub rebuild_source: String,
    pub rebuild_trigger: String,
    pub staleness_policy: StalenessPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateCellEntry {
    pub path: String,
    pub symbol: String,
    pub subsystem: Subsystem,
    pub class: StateClass,
    pub canonical_anchor: String,
    pub projection: Option<ProjectionContract>,
    pub status: EntryStatus,
    pub closure_action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoundaryKind {
    TraitImpl,
    PublicInherent,
    EnumDispatch,
    ManualCallback,
}

impl fmt::Display for BoundaryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TraitImpl => write!(f, "trait-impl"),
            Self::PublicInherent => write!(f, "public-inherent"),
            Self::EnumDispatch => write!(f, "enum-dispatch"),
            Self::ManualCallback => write!(f, "manual-callback"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticOperationEntry {
    pub path: String,
    pub symbol: String,
    pub boundary_kind: BoundaryKind,
    pub owner_shell: String,
    pub writeset: Vec<String>,
    pub anchor: String,
    pub required_postconditions: Vec<String>,
    pub preserved_invariants: Vec<String>,
    pub status: EntryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CouplingInvariantEntry {
    pub name: String,
    pub subsystem: Subsystem,
    pub stores: Vec<String>,
    pub invariant: String,
    pub anchor: String,
    pub enforcement_mode: String,
    pub status: EntryStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitImplBoundary {
    pub family_name: String,
    pub path_suffix: String,
    pub type_name: String,
    pub trait_name: String,
    pub method_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicInherentBoundary {
    pub family_name: String,
    pub path_suffix: String,
    pub type_name: String,
    pub method_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumDispatchBoundary {
    pub family_name: String,
    pub path_suffix: String,
    pub owner_type_name: String,
    pub enum_name: String,
    pub handler_methods: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackBoundary {
    pub path_suffix: String,
    pub owner_type_name: Option<String>,
    pub method_name: String,
    pub compensating_family: String,
    pub why_manual: String,
    pub sunset_condition: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryDiscoveryManifest {
    pub trait_impls: Vec<TraitImplBoundary>,
    pub public_inherent: Vec<PublicInherentBoundary>,
    pub enum_dispatch: Vec<EnumDispatchBoundary>,
    pub callbacks: Vec<CallbackBoundary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipRegistry {
    pub manifest: BoundaryDiscoveryManifest,
    pub state_cells: Vec<StateCellEntry>,
    pub semantic_operations: Vec<SemanticOperationEntry>,
    pub coupling_invariants: Vec<CouplingInvariantEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OwnershipFindingKey {
    pub rule: String,
    pub path: String,
    pub symbol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnershipFinding {
    pub key: OwnershipFindingKey,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OwnershipBaseline {
    #[serde(default)]
    pub finding: Vec<OwnershipFindingKey>,
}

#[derive(Debug, Clone)]
pub struct OwnershipBaselineStatus {
    pub findings: Vec<OwnershipFinding>,
    pub new_findings: Vec<OwnershipFinding>,
    pub stale_baseline: Vec<OwnershipFindingKey>,
}

#[derive(Debug, Clone, Serialize)]
struct SubsystemSummary {
    subsystem: Subsystem,
    state_cells: usize,
    semantic_operations: usize,
    coupling_invariants: usize,
    open_state_cells: usize,
    open_semantic_operations: usize,
    open_coupling_invariants: usize,
}

#[derive(Debug, Clone, Serialize)]
struct OwnershipReport {
    manifest: BoundaryDiscoveryManifest,
    state_cells: Vec<StateCellEntry>,
    semantic_operations: Vec<SemanticOperationEntry>,
    coupling_invariants: Vec<CouplingInvariantEntry>,
    findings: Vec<OwnershipFinding>,
    summaries: Vec<SubsystemSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredBoundary {
    pub family: String,
    pub path: String,
    pub symbol: String,
    pub boundary_kind: BoundaryKind,
}

pub fn run_ownership_ledger(args: OwnershipLedgerArgs) -> Result<()> {
    let root = repo_root()?;
    let registry = ownership_registry();
    let findings = collect_ownership_findings(&root, &registry)?;
    let report = build_report(&registry, findings.clone());

    let markdown = render_markdown(&report);
    let doc_path = root.join(DOC_PATH);
    let baseline_path = root.join(BASELINE_PATH);

    if args.check_drift {
        let current = fs::read_to_string(&doc_path)
            .with_context(|| format!("read {}", doc_path.display()))?;
        if current != markdown {
            bail!(
                "ownership ledger doc is stale: regenerate {} from typed registry",
                doc_path.display()
            );
        }
    }

    if args.write_doc {
        fs::write(&doc_path, &markdown).with_context(|| format!("write {}", doc_path.display()))?;
    }

    if args.update_baseline {
        write_baseline(&baseline_path, &findings)?;
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_summary(&report);
    }

    let baseline_status = ownership_baseline_status(&root)?;
    let new_findings = &baseline_status.new_findings;
    let stale_baseline = &baseline_status.stale_baseline;

    if !new_findings.is_empty() || !stale_baseline.is_empty() {
        let mut messages = Vec::new();
        if !new_findings.is_empty() {
            messages.push(format!(
                "new ownership findings:\n{}",
                new_findings
                    .iter()
                    .map(format_ownership_finding)
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        if !stale_baseline.is_empty() {
            messages.push(format!(
                "stale ownership baseline entries:\n{}",
                stale_baseline
                    .iter()
                    .map(|finding| format!(
                        "- {} {} {}",
                        finding.rule, finding.path, finding.symbol
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        bail!("{}", messages.join("\n\n"));
    }

    Ok(())
}

pub fn ownership_baseline_path(root: &Path) -> PathBuf {
    root.join(BASELINE_PATH)
}

pub fn ownership_baseline_status(root: &Path) -> Result<OwnershipBaselineStatus> {
    let registry = ownership_registry();
    let findings = collect_ownership_findings(root, &registry)?;
    let baseline = read_baseline(&ownership_baseline_path(root))?;
    let baseline_set: BTreeSet<_> = baseline.finding.into_iter().collect();
    let new_findings = findings
        .iter()
        .filter(|finding| !baseline_set.contains(&finding.key))
        .cloned()
        .collect();
    let stale_baseline = baseline_set
        .into_iter()
        .filter(|key| !findings.iter().any(|finding| finding.key == *key))
        .collect();
    Ok(OwnershipBaselineStatus {
        findings,
        new_findings,
        stale_baseline,
    })
}

pub fn ownership_doc_is_in_sync(root: &Path) -> Result<bool> {
    let registry = ownership_registry();
    let findings = collect_ownership_findings(root, &registry)?;
    let report = build_report(&registry, findings);
    let expected = render_markdown(&report);
    let current = fs::read_to_string(root.join(DOC_PATH))
        .with_context(|| format!("read {}", root.join(DOC_PATH).display()))?;
    Ok(current == expected)
}

pub fn collect_ownership_findings(
    root: &Path,
    registry: &OwnershipRegistry,
) -> Result<Vec<OwnershipFinding>> {
    let discovered = discover_boundaries(root, &registry.manifest)?;
    let mut findings = Vec::new();

    let mut seen_state_keys = BTreeSet::new();
    for state in &registry.state_cells {
        let key = (state.path.clone(), state.symbol.clone());
        if !seen_state_keys.insert(key.clone()) {
            findings.push(error_finding(
                "OwnershipDuplicateStateEntry",
                key.0,
                key.1,
                "duplicate StateCellEntry key",
            ));
        }
    }

    let mut seen_operation_keys = BTreeSet::new();
    for operation in &registry.semantic_operations {
        let key = (operation.path.clone(), operation.symbol.clone());
        if !seen_operation_keys.insert(key.clone()) {
            findings.push(error_finding(
                "OwnershipDuplicateOperationEntry",
                key.0,
                key.1,
                "duplicate SemanticOperationEntry key",
            ));
        }
    }

    let mut seen_invariant_names = BTreeSet::new();
    for invariant in &registry.coupling_invariants {
        if !seen_invariant_names.insert((invariant.subsystem, invariant.name.clone())) {
            findings.push(error_finding(
                "OwnershipDuplicateInvariantEntry",
                format!("<{}>", invariant.subsystem),
                invariant.name.clone(),
                "duplicate CouplingInvariantEntry name within subsystem",
            ));
        }
    }

    let discovered_keys = discovered
        .iter()
        .map(|boundary| (boundary.path.clone(), boundary.symbol.clone()))
        .collect::<BTreeSet<_>>();
    let discovered_kind_by_key = discovered
        .iter()
        .map(|boundary| {
            (
                (boundary.path.clone(), boundary.symbol.clone()),
                boundary.boundary_kind,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let entry_keys = registry
        .semantic_operations
        .iter()
        .map(|entry| (entry.path.clone(), entry.symbol.clone()))
        .collect::<BTreeSet<_>>();

    for boundary in &discovered {
        if !entry_keys.contains(&(boundary.path.clone(), boundary.symbol.clone())) {
            findings.push(error_finding(
                "OwnershipMissingOperationEntry",
                &boundary.path,
                &boundary.symbol,
                format!(
                    "discovered {} boundary `{}` in family `{}` has no SemanticOperationEntry",
                    boundary.boundary_kind, boundary.symbol, boundary.family
                ),
            ));
        }
    }

    for entry in &registry.semantic_operations {
        if entry.owner_shell.trim().is_empty() {
            findings.push(error_finding(
                "OwnershipOperationOwnerMissing",
                &entry.path,
                &entry.symbol,
                "semantic operation entry must declare a non-empty owner_shell",
            ));
        }
        if entry.anchor.trim().is_empty() {
            findings.push(error_finding(
                "OwnershipOperationAnchorMissing",
                &entry.path,
                &entry.symbol,
                "semantic operation entry must declare a non-empty anchor",
            ));
        }
        if entry.writeset.is_empty() {
            findings.push(error_finding(
                "OwnershipOperationWritesetMissing",
                &entry.path,
                &entry.symbol,
                "semantic operation entry must declare at least one writeset item",
            ));
        }
        if entry.writeset.iter().any(|item| item.trim().is_empty()) {
            findings.push(error_finding(
                "OwnershipOperationWritesetInvalid",
                &entry.path,
                &entry.symbol,
                "semantic operation writeset entries must be non-empty strings",
            ));
        }
        let mut writeset_symbols = BTreeSet::new();
        for writeset_item in &entry.writeset {
            if !writeset_symbols.insert(writeset_item) {
                findings.push(error_finding(
                    "OwnershipOperationWritesetDuplicate",
                    &entry.path,
                    &entry.symbol,
                    format!(
                        "semantic operation writeset contains duplicate symbol `{writeset_item}`",
                    ),
                ));
            }
        }
        if entry.required_postconditions.is_empty() {
            findings.push(error_finding(
                "OwnershipOperationPostconditionsMissing",
                &entry.path,
                &entry.symbol,
                "semantic operation entry must declare at least one required_postcondition",
            ));
        }
        if entry.preserved_invariants.is_empty() {
            findings.push(error_finding(
                "OwnershipOperationInvariantsMissing",
                &entry.path,
                &entry.symbol,
                "semantic operation entry must declare at least one preserved_invariant",
            ));
        }
        if entry
            .required_postconditions
            .iter()
            .any(|item| item.trim().is_empty())
        {
            findings.push(error_finding(
                "OwnershipOperationPostconditionsInvalid",
                &entry.path,
                &entry.symbol,
                "required_postconditions entries must be non-empty strings",
            ));
        }
        if entry
            .preserved_invariants
            .iter()
            .any(|item| item.trim().is_empty())
        {
            findings.push(error_finding(
                "OwnershipOperationInvariantsInvalid",
                &entry.path,
                &entry.symbol,
                "preserved_invariants entries must be non-empty strings",
            ));
        }
        if !discovered_keys.contains(&(entry.path.clone(), entry.symbol.clone())) {
            findings.push(error_finding(
                "OwnershipExtraOperationEntry",
                &entry.path,
                &entry.symbol,
                format!(
                    "semantic operation entry `{}` is not backed by any discovered boundary",
                    entry.symbol
                ),
            ));
        }
        if let Some(discovered_kind) =
            discovered_kind_by_key.get(&(entry.path.clone(), entry.symbol.clone()))
            && *discovered_kind != entry.boundary_kind
        {
            findings.push(error_finding(
                "OwnershipBoundaryKindMismatch",
                &entry.path,
                &entry.symbol,
                format!(
                    "semantic operation `{}` declares boundary kind `{}` but manifest discovery is `{}`",
                    entry.symbol, entry.boundary_kind, discovered_kind
                ),
            ));
        }
        if entry.status == EntryStatus::Open {
            findings.push(error_finding(
                "OwnershipOperationOpen",
                &entry.path,
                &entry.symbol,
                format!(
                    "semantic operation `{}` remains open against anchor `{}`",
                    entry.symbol, entry.anchor
                ),
            ));
        }
    }

    let mut parsed_files = BTreeMap::<String, syn::File>::new();
    for state in &registry.state_cells {
        if state.canonical_anchor.trim().is_empty() {
            findings.push(error_finding(
                "OwnershipStateAnchorMissing",
                &state.path,
                &state.symbol,
                "state cell entry must declare a non-empty canonical_anchor",
            ));
        }
        if state.closure_action.trim().is_empty() {
            findings.push(error_finding(
                "OwnershipStateClosureActionMissing",
                &state.path,
                &state.symbol,
                "state cell entry must declare a non-empty closure_action",
            ));
        }
        let symbol = state.symbol.as_str();
        let Some((type_name, field_name)) = parse_struct_field_symbol(symbol) else {
            findings.push(error_finding(
                "OwnershipStateSymbolInvalid",
                &state.path,
                &state.symbol,
                "state symbol must be `TypeName.field_name`",
            ));
            continue;
        };
        if !parsed_files.contains_key(&state.path) {
            parsed_files.insert(state.path.clone(), parse_repo_file(root, &state.path)?);
        }
        let parsed = parsed_files
            .get(&state.path)
            .expect("parsed file present after insert");
        if !struct_has_named_field(parsed, type_name, field_name) {
            findings.push(error_finding(
                "OwnershipStateSymbolMissing",
                &state.path,
                &state.symbol,
                format!(
                    "state symbol `{}` does not match a named field on struct `{}` in `{}`",
                    state.symbol, type_name, state.path
                ),
            ));
        }

        match state.class {
            StateClass::UnownedSemantic => {
                findings.push(error_finding(
                    "OwnershipStateUnowned",
                    &state.path,
                    &state.symbol,
                    format!(
                        "state cell `{}` remains unowned semantic truth; closure action: {}",
                        state.symbol, state.closure_action
                    ),
                ));
            }
            StateClass::DerivedProjection | StateClass::CapabilityIndex => {
                if state.projection.is_none() {
                    findings.push(error_finding(
                        "OwnershipProjectionContractMissing",
                        &state.path,
                        &state.symbol,
                        format!(
                            "state cell `{}` is `{}` but has no rebuild/freshness contract",
                            state.symbol, state.class
                        ),
                    ));
                } else if state.projection.as_ref().is_some_and(|projection| {
                    projection.staleness_policy != StalenessPolicy::Forbidden
                }) {
                    findings.push(error_finding(
                        "OwnershipProjectionStalenessPolicyWeak",
                        &state.path,
                        &state.symbol,
                        format!(
                            "state cell `{}` is `{}` but staleness policy is `{}` (expected `forbidden`)",
                            state.symbol,
                            state.class,
                            state
                                .projection
                                .as_ref()
                                .map(|projection| projection.staleness_policy.to_string())
                                .unwrap_or_default()
                        ),
                    ));
                } else if state.projection.as_ref().is_some_and(|projection| {
                    projection.rebuild_source.trim().is_empty()
                        || projection.rebuild_trigger.trim().is_empty()
                }) {
                    findings.push(error_finding(
                        "OwnershipProjectionContractInvalid",
                        &state.path,
                        &state.symbol,
                        format!(
                            "state cell `{}` has an incomplete projection contract (missing rebuild_source and/or rebuild_trigger)",
                            state.symbol
                        ),
                    ));
                }
            }
            _ => {}
        }
        if state.status == EntryStatus::Open && state.class != StateClass::UnownedSemantic {
            findings.push(error_finding(
                "OwnershipStateOpen",
                &state.path,
                &state.symbol,
                format!(
                    "state cell `{}` is still open against anchor `{}`",
                    state.symbol, state.canonical_anchor
                ),
            ));
        }
    }

    for invariant in &registry.coupling_invariants {
        if invariant.stores.is_empty() {
            findings.push(error_finding(
                "OwnershipInvariantStoresMissing",
                format!("<{}>", invariant.subsystem),
                &invariant.name,
                "coupling invariant entry must declare at least one store",
            ));
        }
        if invariant.stores.iter().any(|store| store.trim().is_empty()) {
            findings.push(error_finding(
                "OwnershipInvariantStoreInvalid",
                format!("<{}>", invariant.subsystem),
                &invariant.name,
                "coupling invariant stores must be non-empty strings",
            ));
        }
        let mut store_symbols = BTreeSet::new();
        for store in &invariant.stores {
            if !store_symbols.insert(store) {
                findings.push(error_finding(
                    "OwnershipInvariantStoreDuplicate",
                    format!("<{}>", invariant.subsystem),
                    &invariant.name,
                    format!("coupling invariant store list contains duplicate symbol `{store}`"),
                ));
            }
        }
        if invariant.invariant.trim().is_empty()
            || invariant.anchor.trim().is_empty()
            || invariant.enforcement_mode.trim().is_empty()
        {
            findings.push(error_finding(
                "OwnershipInvariantMetadataMissing",
                format!("<{}>", invariant.subsystem),
                &invariant.name,
                "coupling invariant must declare non-empty invariant text, anchor, and enforcement_mode",
            ));
        }
        if invariant.status == EntryStatus::Open {
            findings.push(error_finding(
                "OwnershipInvariantOpen",
                &format!("<{}>", invariant.subsystem),
                &invariant.name,
                format!(
                    "coupling invariant `{}` remains open against anchor `{}`",
                    invariant.name, invariant.anchor
                ),
            ));
        }
    }

    let known_manifest_families = registry
        .manifest
        .trait_impls
        .iter()
        .map(|family| family.family_name.clone())
        .chain(
            registry
                .manifest
                .public_inherent
                .iter()
                .map(|family| family.family_name.clone()),
        )
        .chain(
            registry
                .manifest
                .enum_dispatch
                .iter()
                .map(|family| family.family_name.clone()),
        )
        .collect::<BTreeSet<_>>();
    for family in &registry.manifest.trait_impls {
        let mut seen = BTreeSet::new();
        for method in &family.method_names {
            if !seen.insert(method) {
                findings.push(error_finding(
                    "OwnershipManifestMethodDuplicate",
                    &family.path_suffix,
                    method,
                    format!(
                        "trait-impl family `{}` declares duplicate method `{}`",
                        family.family_name, method
                    ),
                ));
            }
        }
    }
    for family in &registry.manifest.public_inherent {
        let mut seen = BTreeSet::new();
        for method in &family.method_names {
            if !seen.insert(method) {
                findings.push(error_finding(
                    "OwnershipManifestMethodDuplicate",
                    &family.path_suffix,
                    method,
                    format!(
                        "public-inherent family `{}` declares duplicate method `{}`",
                        family.family_name, method
                    ),
                ));
            }
        }
    }
    for family in &registry.manifest.enum_dispatch {
        let mut seen = BTreeSet::new();
        for handler in &family.handler_methods {
            if !seen.insert(handler) {
                findings.push(error_finding(
                    "OwnershipManifestMethodDuplicate",
                    &family.path_suffix,
                    handler,
                    format!(
                        "enum-dispatch family `{}` declares duplicate handler `{}`",
                        family.family_name, handler
                    ),
                ));
            }
        }
    }
    for callback in &registry.manifest.callbacks {
        if callback.compensating_family.trim().is_empty()
            || callback.why_manual.trim().is_empty()
            || callback.sunset_condition.trim().is_empty()
        {
            findings.push(error_finding(
                "OwnershipManualCallbackJustificationMissing",
                &callback.path_suffix,
                &callback.method_name,
                format!(
                    "manual callback `{}` must declare compensating_family, why_manual, and sunset_condition",
                    callback.method_name
                ),
            ));
        }
        if !callback.compensating_family.trim().is_empty()
            && !known_manifest_families.contains(&callback.compensating_family)
        {
            findings.push(error_finding(
                "OwnershipManualCallbackUnknownFamily",
                &callback.path_suffix,
                &callback.method_name,
                format!(
                    "manual callback `{}` references unknown compensating family `{}`",
                    callback.method_name, callback.compensating_family
                ),
            ));
        }
    }

    findings.sort_by(|a, b| a.key.cmp(&b.key));
    findings.dedup_by(|a, b| a.key == b.key);
    Ok(findings)
}

pub fn read_baseline(path: &Path) -> Result<OwnershipBaseline> {
    if !path.exists() {
        return Ok(OwnershipBaseline::default());
    }
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

pub fn write_baseline(path: &Path, findings: &[OwnershipFinding]) -> Result<()> {
    let baseline = OwnershipBaseline {
        finding: findings.iter().map(|finding| finding.key.clone()).collect(),
    };
    let toml = toml::to_string_pretty(&baseline)?;
    fs::write(path, toml).with_context(|| format!("write {}", path.display()))
}

fn error_finding(
    rule: impl Into<String>,
    path: impl Into<String>,
    symbol: impl Into<String>,
    message: impl Into<String>,
) -> OwnershipFinding {
    OwnershipFinding {
        key: OwnershipFindingKey {
            rule: rule.into(),
            path: path.into(),
            symbol: symbol.into(),
        },
        severity: "error".into(),
        message: message.into(),
    }
}

pub fn format_ownership_finding(finding: &OwnershipFinding) -> String {
    format!(
        "- [{}] {} {} :: {}",
        finding.severity, finding.key.rule, finding.key.path, finding.message
    )
}

fn build_report(registry: &OwnershipRegistry, findings: Vec<OwnershipFinding>) -> OwnershipReport {
    let mut summaries = Vec::new();
    for subsystem in [Subsystem::Runtime, Subsystem::Mcp, Subsystem::Mob] {
        summaries.push(SubsystemSummary {
            subsystem,
            state_cells: registry
                .state_cells
                .iter()
                .filter(|entry| entry.subsystem == subsystem)
                .count(),
            semantic_operations: registry
                .semantic_operations
                .iter()
                .filter(|entry| subsystem_of_path(&entry.path) == subsystem)
                .count(),
            coupling_invariants: registry
                .coupling_invariants
                .iter()
                .filter(|entry| entry.subsystem == subsystem)
                .count(),
            open_state_cells: registry
                .state_cells
                .iter()
                .filter(|entry| entry.subsystem == subsystem && entry.status == EntryStatus::Open)
                .count(),
            open_semantic_operations: registry
                .semantic_operations
                .iter()
                .filter(|entry| {
                    subsystem_of_path(&entry.path) == subsystem && entry.status == EntryStatus::Open
                })
                .count(),
            open_coupling_invariants: registry
                .coupling_invariants
                .iter()
                .filter(|entry| entry.subsystem == subsystem && entry.status == EntryStatus::Open)
                .count(),
        });
    }
    OwnershipReport {
        manifest: registry.manifest.clone(),
        state_cells: registry.state_cells.clone(),
        semantic_operations: registry.semantic_operations.clone(),
        coupling_invariants: registry.coupling_invariants.clone(),
        findings,
        summaries,
    }
}

fn render_markdown(report: &OwnershipReport) -> String {
    let mut out = String::new();
    out.push_str("# Finite Ownership Ledger\n\n");
    out.push_str("**Status**: Generated\n");
    out.push_str("**Source**: `xtask ownership-ledger`\n\n");
    out.push_str("This document is generated from the typed ownership registry in `xtask`.\n");
    out.push_str("It is the authoritative inventory of semantic state, semantic-operation boundaries, and keyed-store invariants for the current closure program.\n\n");
    out.push_str("## Summary\n\n");
    out.push_str("| Subsystem | State Cells | Semantic Operations | Coupling Invariants | Open State Cells | Open Operations | Open Invariants |\n");
    out.push_str("| --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for summary in &report.summaries {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            summary.subsystem,
            summary.state_cells,
            summary.semantic_operations,
            summary.coupling_invariants,
            summary.open_state_cells,
            summary.open_semantic_operations,
            summary.open_coupling_invariants
        ));
    }
    out.push_str("\n## Boundary Manifest\n\n");
    out.push_str("| Family | Kind | Path | Type / Trait | Methods |\n");
    out.push_str("| --- | --- | --- | --- | --- |\n");
    for family in &report.manifest.trait_impls {
        out.push_str(&format!(
            "| {} | trait-impl | `{}` | `{}` / `{}` | `{}` |\n",
            family.family_name,
            family.path_suffix,
            family.type_name,
            family.trait_name,
            family.method_names.join("`, `")
        ));
    }
    for family in &report.manifest.public_inherent {
        out.push_str(&format!(
            "| {} | public-inherent | `{}` | `{}` | `{}` |\n",
            family.family_name,
            family.path_suffix,
            family.type_name,
            family.method_names.join("`, `")
        ));
    }
    for family in &report.manifest.enum_dispatch {
        out.push_str(&format!(
            "| {} | enum-dispatch | `{}` | `{}` / `{}` | `{}` |\n",
            family.family_name,
            family.path_suffix,
            family.owner_type_name,
            family.enum_name,
            family.handler_methods.join("`, `")
        ));
    }
    for callback in &report.manifest.callbacks {
        out.push_str(&format!(
            "| manual-callback | manual-callback | `{}` | `{}` | `{}` |\n",
            callback.path_suffix,
            callback.owner_type_name.as_deref().unwrap_or("<free fn>"),
            callback.method_name
        ));
    }

    for subsystem in [Subsystem::Runtime, Subsystem::Mcp, Subsystem::Mob] {
        out.push_str(&format!(
            "\n## {} State Cells\n\n",
            title_case_subsystem(subsystem)
        ));
        out.push_str("| Path | Symbol | Class | Status | Anchor | Contract |\n");
        out.push_str("| --- | --- | --- | --- | --- | --- |\n");
        for entry in report
            .state_cells
            .iter()
            .filter(|entry| entry.subsystem == subsystem)
        {
            let contract = entry.projection.as_ref().map_or_else(
                || "-".to_string(),
                |projection| {
                    format!(
                        "src: `{}`; trigger: `{}`; stale: `{}`",
                        projection.rebuild_source,
                        projection.rebuild_trigger,
                        projection.staleness_policy
                    )
                },
            );
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` | {} |\n",
                entry.path,
                entry.symbol,
                entry.class,
                entry.status,
                entry.canonical_anchor,
                contract
            ));
        }

        out.push_str(&format!(
            "\n## {} Semantic Operations\n\n",
            title_case_subsystem(subsystem)
        ));
        out.push_str("| Path | Symbol | Boundary | Status | Anchor |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for entry in report
            .semantic_operations
            .iter()
            .filter(|entry| subsystem_of_path(&entry.path) == subsystem)
        {
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` |\n",
                entry.path, entry.symbol, entry.boundary_kind, entry.status, entry.anchor
            ));
        }

        out.push_str(&format!(
            "\n## {} Coupling Invariants\n\n",
            title_case_subsystem(subsystem)
        ));
        out.push_str("| Name | Stores | Status | Anchor |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        for entry in report
            .coupling_invariants
            .iter()
            .filter(|entry| entry.subsystem == subsystem)
        {
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` |\n",
                entry.name,
                entry.stores.join("`, `"),
                entry.status,
                entry.anchor
            ));
        }
    }

    out.push_str("\n## Open Findings\n\n");
    if report.findings.is_empty() {
        out.push_str("No ownership findings.\n");
    } else {
        for finding in &report.findings {
            out.push_str(&format!("{}\n", format_ownership_finding(finding)));
        }
    }

    out
}

fn print_summary(report: &OwnershipReport) {
    println!("# Ownership Ledger");
    println!("# Generated by `xtask ownership-ledger`");
    println!();
    println!("## Summary");
    for summary in &report.summaries {
        println!(
            "  {:7} state={} ops={} invariants={} open_state={} open_ops={} open_invariants={}",
            summary.subsystem,
            summary.state_cells,
            summary.semantic_operations,
            summary.coupling_invariants,
            summary.open_state_cells,
            summary.open_semantic_operations,
            summary.open_coupling_invariants
        );
    }
    println!();
    println!("## Findings");
    if report.findings.is_empty() {
        println!("  none");
    } else {
        for finding in &report.findings {
            println!("  {}", format_ownership_finding(finding));
        }
    }
}

fn title_case_subsystem(subsystem: Subsystem) -> &'static str {
    match subsystem {
        Subsystem::Runtime => "Runtime",
        Subsystem::Mcp => "MCP",
        Subsystem::Mob => "Mob",
    }
}

fn subsystem_of_path(path: &str) -> Subsystem {
    if path.starts_with("meerkat-runtime/") {
        Subsystem::Runtime
    } else if path.starts_with("meerkat-mcp/") {
        Subsystem::Mcp
    } else {
        Subsystem::Mob
    }
}

fn discover_boundaries(
    root: &Path,
    manifest: &BoundaryDiscoveryManifest,
) -> Result<Vec<DiscoveredBoundary>> {
    let mut boundaries = Vec::new();
    for family in &manifest.trait_impls {
        let parsed = parse_repo_file(root, &family.path_suffix)?;
        let methods = discover_trait_impl_methods(&parsed, &family.type_name, &family.trait_name);
        for method_name in &family.method_names {
            if !methods.contains(method_name) {
                bail!(
                    "boundary manifest family `{}` expected trait method `{}` in `{}`",
                    family.family_name,
                    method_name,
                    family.path_suffix
                );
            }
            boundaries.push(DiscoveredBoundary {
                family: family.family_name.clone(),
                path: family.path_suffix.clone(),
                symbol: method_name.clone(),
                boundary_kind: BoundaryKind::TraitImpl,
            });
        }
    }
    for family in &manifest.public_inherent {
        let parsed = parse_repo_file(root, &family.path_suffix)?;
        let methods = discover_public_inherent_methods(&parsed, &family.type_name);
        for method_name in &family.method_names {
            if !methods.contains(method_name) {
                bail!(
                    "boundary manifest family `{}` expected public method `{}` in `{}`",
                    family.family_name,
                    method_name,
                    family.path_suffix
                );
            }
            boundaries.push(DiscoveredBoundary {
                family: family.family_name.clone(),
                path: family.path_suffix.clone(),
                symbol: method_name.clone(),
                boundary_kind: BoundaryKind::PublicInherent,
            });
        }
    }
    for family in &manifest.enum_dispatch {
        let parsed = parse_repo_file(root, &family.path_suffix)?;
        let methods = discover_inherent_methods(&parsed, &family.owner_type_name);
        for handler_name in &family.handler_methods {
            if !methods.contains(handler_name) {
                bail!(
                    "boundary manifest family `{}` expected enum-dispatch handler `{}` in `{}`",
                    family.family_name,
                    handler_name,
                    family.path_suffix
                );
            }
            boundaries.push(DiscoveredBoundary {
                family: family.family_name.clone(),
                path: family.path_suffix.clone(),
                symbol: handler_name.clone(),
                boundary_kind: BoundaryKind::EnumDispatch,
            });
        }
    }
    for callback in &manifest.callbacks {
        let parsed = parse_repo_file(root, &callback.path_suffix)?;
        let methods = callback
            .owner_type_name
            .as_ref()
            .map(|type_name| discover_inherent_methods(&parsed, type_name))
            .unwrap_or_else(|| discover_free_functions(&parsed));
        if !methods.contains(&callback.method_name) {
            bail!(
                "boundary manifest expected manual callback `{}` in `{}`",
                callback.method_name,
                callback.path_suffix
            );
        }
        boundaries.push(DiscoveredBoundary {
            family: callback.compensating_family.clone(),
            path: callback.path_suffix.clone(),
            symbol: callback.method_name.clone(),
            boundary_kind: BoundaryKind::ManualCallback,
        });
    }
    boundaries.sort_by(|a, b| {
        (&a.path, &a.symbol, &a.family, &a.boundary_kind).cmp(&(
            &b.path,
            &b.symbol,
            &b.family,
            &b.boundary_kind,
        ))
    });
    Ok(boundaries)
}

fn parse_repo_file(root: &Path, rel: &str) -> Result<syn::File> {
    let path = root.join(rel);
    let source = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    syn::parse_file(&source).with_context(|| format!("parse {}", path.display()))
}

fn discover_trait_impl_methods(
    parsed: &syn::File,
    type_name: &str,
    trait_name: &str,
) -> BTreeSet<String> {
    let mut methods = BTreeSet::new();
    for item in &parsed.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if self_type_name(item_impl).as_deref() != Some(type_name) {
            continue;
        }
        let Some((_, path, _)) = &item_impl.trait_ else {
            continue;
        };
        let Some(last_segment) = path.segments.last() else {
            continue;
        };
        if last_segment.ident != trait_name {
            continue;
        }
        for impl_item in &item_impl.items {
            if let ImplItem::Fn(method) = impl_item {
                methods.insert(method.sig.ident.to_string());
            }
        }
    }
    methods
}

fn discover_public_inherent_methods(parsed: &syn::File, type_name: &str) -> BTreeSet<String> {
    let mut methods = BTreeSet::new();
    for item in &parsed.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if item_impl.trait_.is_some() || self_type_name(item_impl).as_deref() != Some(type_name) {
            continue;
        }
        for impl_item in &item_impl.items {
            if let ImplItem::Fn(method) = impl_item
                && matches!(method.vis, syn::Visibility::Public(_))
            {
                methods.insert(method.sig.ident.to_string());
            }
        }
    }
    methods
}

fn discover_inherent_methods(parsed: &syn::File, type_name: &str) -> BTreeSet<String> {
    let mut methods = BTreeSet::new();
    for item in &parsed.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        if item_impl.trait_.is_some() || self_type_name(item_impl).as_deref() != Some(type_name) {
            continue;
        }
        for impl_item in &item_impl.items {
            if let ImplItem::Fn(method) = impl_item {
                methods.insert(method.sig.ident.to_string());
            }
        }
    }
    methods
}

fn discover_free_functions(parsed: &syn::File) -> BTreeSet<String> {
    parsed
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(ItemFn { sig, .. }) => Some(sig.ident.to_string()),
            _ => None,
        })
        .collect()
}

fn self_type_name(item_impl: &ItemImpl) -> Option<String> {
    type_name(item_impl.self_ty.as_ref())
}

fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Type::Reference(reference) => type_name(reference.elem.as_ref()),
        _ => None,
    }
}

fn parse_struct_field_symbol(symbol: &str) -> Option<(&str, &str)> {
    let (type_name, field_name) = symbol.split_once('.')?;
    if type_name.is_empty() || field_name.is_empty() {
        return None;
    }
    Some((type_name, field_name))
}

fn struct_has_named_field(parsed: &syn::File, type_name: &str, field_name: &str) -> bool {
    parsed.items.iter().any(|item| {
        let Item::Struct(item_struct) = item else {
            return false;
        };
        if item_struct.ident != type_name {
            return false;
        }
        item_struct
            .fields
            .iter()
            .filter_map(|field| field.ident.as_ref())
            .any(|field_ident| field_ident == field_name)
    })
}

fn ownership_registry() -> OwnershipRegistry {
    OwnershipRegistry {
        manifest: boundary_manifest(),
        state_cells: state_cells(),
        semantic_operations: semantic_operations(),
        coupling_invariants: coupling_invariants(),
    }
}

pub fn collect_current_findings(root: &Path) -> Result<Vec<OwnershipFinding>> {
    let registry = ownership_registry();
    collect_ownership_findings(root, &registry)
}

pub fn diff_against_baseline(
    path: &Path,
    findings: &[OwnershipFinding],
) -> Result<(Vec<OwnershipFinding>, Vec<OwnershipFindingKey>)> {
    let baseline = read_baseline(path)?;
    let baseline_set: BTreeSet<_> = baseline.finding.into_iter().collect();
    let new_findings = findings
        .iter()
        .filter(|finding| !baseline_set.contains(&finding.key))
        .cloned()
        .collect();
    let stale_baseline = baseline_set
        .into_iter()
        .filter(|key| !findings.iter().any(|finding| finding.key == *key))
        .collect();
    Ok((new_findings, stale_baseline))
}

fn boundary_manifest() -> BoundaryDiscoveryManifest {
    BoundaryDiscoveryManifest {
        trait_impls: vec![
            TraitImplBoundary {
                family_name: "runtime-control-plane".into(),
                path_suffix: "meerkat-runtime/src/session_adapter.rs".into(),
                type_name: "RuntimeSessionAdapter".into(),
                trait_name: "RuntimeControlPlane".into(),
                method_names: vec![
                    "ingest",
                    "publish_event",
                    "retire",
                    "recycle",
                    "reset",
                    "recover",
                    "destroy",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
        ],
        public_inherent: vec![
            PublicInherentBoundary {
                family_name: "runtime-session-adapter".into(),
                path_suffix: "meerkat-runtime/src/session_adapter.rs".into(),
                type_name: "RuntimeSessionAdapter".into(),
                method_names: vec![
                    "set_comms_drain_control",
                    "register_session",
                    "set_session_silent_intents",
                    "register_session_with_executor",
                    "ensure_session_with_executor",
                    "unregister_session",
                    "interrupt_current_run",
                    "stop_runtime_executor",
                    "accept_input_and_run",
                    "accept_input_with_completion",
                    "maybe_spawn_comms_drain",
                    "abort_comms_drains",
                    "abort_comms_drain",
                    "wait_comms_drain",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
            PublicInherentBoundary {
                family_name: "mcp-router".into(),
                path_suffix: "meerkat-mcp/src/router.rs".into(),
                type_name: "McpRouter".into(),
                method_names: vec![
                    "set_removal_timeout",
                    "add_server",
                    "stage_add",
                    "stage_remove",
                    "stage_reload",
                    "apply_staged",
                    "take_lifecycle_actions",
                    "take_external_updates",
                    "progress_removals",
                    "call_tool",
                    "shutdown",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
            PublicInherentBoundary {
                family_name: "mcp-router-adapter".into(),
                path_suffix: "meerkat-mcp/src/adapter.rs".into(),
                type_name: "McpRouterAdapter".into(),
                method_names: vec![
                    "refresh_tools",
                    "stage_add",
                    "stage_remove",
                    "stage_reload",
                    "apply_staged",
                    "poll_lifecycle_actions",
                    "progress_removals",
                    "wait_until_ready",
                    "shutdown",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
            PublicInherentBoundary {
                family_name: "mob-handle".into(),
                path_suffix: "meerkat-mob/src/runtime/handle.rs".into(),
                type_name: "MobHandle".into(),
                method_names: vec![
                    "spawn",
                    "spawn_with_backend",
                    "spawn_with_options",
                    "attach_existing_session",
                    "attach_existing_session_as_orchestrator",
                    "attach_existing_session_as_member",
                    "spawn_spec",
                    "spawn_spec_receipt",
                    "spawn_many",
                    "spawn_many_receipts",
                    "retire",
                    "respawn",
                    "retire_all",
                    "wire",
                    "unwire",
                    "internal_turn",
                    "run_flow",
                    "run_flow_with_stream",
                    "cancel_flow",
                    "stop",
                    "resume",
                    "complete",
                    "reset",
                    "destroy",
                    "task_create",
                    "task_update",
                    "set_spawn_policy",
                    "shutdown",
                    "force_cancel_member",
                    "wait_one",
                    "wait_all",
                    "spawn_helper",
                    "fork_helper",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            },
        ],
        enum_dispatch: vec![EnumDispatchBoundary {
            family_name: "mob-command-dispatch".into(),
            path_suffix: "meerkat-mob/src/runtime/actor.rs".into(),
            owner_type_name: "MobActor".into(),
            enum_name: "MobCommand".into(),
            handler_methods: vec![
                "enqueue_spawn",
                "handle_force_cancel",
                "handle_retire",
                "handle_respawn",
                "handle_wire",
                "handle_unwire",
                "handle_external_turn",
                "handle_internal_turn",
                "retire_all_members",
                "handle_task_create",
                "handle_task_update",
                "handle_run_flow",
                "handle_cancel_flow",
                "handle_flow_cleanup",
                "handle_complete",
                "handle_destroy",
                "handle_reset",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }],
        callbacks: vec![
            CallbackBoundary {
                path_suffix: "meerkat-runtime/src/session_adapter.rs".into(),
                owner_type_name: Some("RuntimeSessionAdapter".into()),
                method_name: "notify_comms_drain_exited".into(),
                compensating_family: "runtime-session-adapter".into(),
                why_manual: "drain exit is an async callback path, not a direct public boundary".into(),
                sunset_condition:
                    "remove once drain callback boundaries are auto-derived from protocol metadata"
                        .into(),
            },
            CallbackBoundary {
                path_suffix: "meerkat-mcp/src/router.rs".into(),
                owner_type_name: Some("McpRouter".into()),
                method_name: "process_pending_result".into(),
                compensating_family: "mcp-router".into(),
                why_manual:
                    "pending result completion is an async shell callback outside public API discovery"
                        .into(),
                sunset_condition:
                    "remove once pending-task callbacks are auto-derived from boundary-machine protocol bindings"
                        .into(),
            },
            CallbackBoundary {
                path_suffix: "meerkat-mob/src/runtime/actor.rs".into(),
                owner_type_name: Some("MobActor".into()),
                method_name: "handle_spawn_provisioned_batch".into(),
                compensating_family: "mob-command-dispatch".into(),
                why_manual:
                    "spawn completions arrive asynchronously from provisioner tasks outside direct command dispatch discovery"
                        .into(),
                sunset_condition:
                    "remove once spawn-provisioned callback boundaries are auto-derived from runtime bridge protocol metadata"
                        .into(),
            },
        ],
    }
}

fn state_cells() -> Vec<StateCellEntry> {
    vec![
        state(
            "meerkat-runtime/src/session_adapter.rs",
            "RuntimeSessionAdapter.sessions",
            Subsystem::Runtime,
            StateClass::CapabilityIndex,
            "RuntimeSessionAdapter registered-session + attachment publication contract",
            Some(contract(
                "registered session entries with recovered driver/completion capabilities",
                "register/ensure/attach/detach/unregister/destroy transitions + dead-attachment normalization",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "session map is identity-to-runtime-capability reachability only; registration, stale attachment normalization, and teardown are enforced by adapter publication rules rather than ad hoc shell pre-checks",
        ),
        state(
            "meerkat-runtime/src/session_adapter.rs",
            "RuntimeSessionAdapter.comms_drain_slots",
            Subsystem::Runtime,
            StateClass::CapabilityIndex,
            "RuntimeSessionAdapter registered-session contract + CommsDrainLifecycleMachine",
            Some(contract(
                "registered session keys + comms drain lifecycle slot allocation",
                "register/unregister/destroy + drain lifecycle transitions + control installation",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "drain slots are capability reachability only; unregister aborts and removes slots before dropping session binding, and spawn/control updates ignore unregistered sessions",
        ),
        state(
            "meerkat-runtime/src/session_adapter.rs",
            "RuntimeSessionEntry.driver",
            Subsystem::Runtime,
            StateClass::CapabilityHandle,
            "RuntimeControlMachine + RuntimeIngressMachine + InputLifecycleMachine",
            None,
            EntryStatus::Closed,
            "driver is an opaque capability handle; semantic state transitions are mediated through driver authorities and adapter publication rules rather than raw handle identity",
        ),
        state(
            "meerkat-runtime/src/session_adapter.rs",
            "RuntimeSessionEntry.attachment",
            Subsystem::Runtime,
            StateClass::CapabilityHandle,
            "RuntimeSessionAdapter attachment publication contract",
            None,
            EntryStatus::Closed,
            "attachment publication is liveness-gated by loop channels; stop paths do not pre-clear attachment ahead of canonical driver control transitions, and stale attached-driver states are repaired before re-publication",
        ),
        state(
            "meerkat-runtime/src/session_adapter.rs",
            "RuntimeSessionEntry.completions",
            Subsystem::Runtime,
            StateClass::CapabilityHandle,
            "InputLifecycle terminal wait plumbing",
            None,
            EntryStatus::Closed,
            "completion registry is crate-private waiter plumbing; runtime surfaces expose only completion handles/outcomes and do not branch on waiter presence/count",
        ),
        state(
            "meerkat-runtime/src/driver/ephemeral.rs",
            "EphemeralRuntimeDriver.queue",
            Subsystem::Runtime,
            StateClass::DerivedProjection,
            "RuntimeIngressMachine queue lane",
            Some(contract(
                "RuntimeIngressMachine.queue entries",
                "any ingress queue mutation or rollback/recovery rebuild",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "physical queue is rebuilt from canonical ingress queue entries after every queue mutation, and persistent recovery now fails closed instead of shell-repairing projection drift",
        ),
        state(
            "meerkat-runtime/src/driver/ephemeral.rs",
            "EphemeralRuntimeDriver.steer_queue",
            Subsystem::Runtime,
            StateClass::DerivedProjection,
            "RuntimeIngressMachine steer lane",
            Some(contract(
                "RuntimeIngressMachine.steer entries",
                "any ingress steer mutation or rollback/recovery rebuild",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "physical steer queue is rebuilt from canonical ingress steer entries after every queue mutation, and persistent recovery now fails closed instead of shell-repairing projection drift",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.servers",
            Subsystem::Mcp,
            StateClass::CapabilityIndex,
            "ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract",
            Some(contract(
                "canonical surface state + live server handles",
                "apply_staged completion, pending completion, removal finalization, shutdown",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "used strictly for identity-to-handle reachability after projection-based routing selection",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.projection",
            Subsystem::Mcp,
            StateClass::DerivedProjection,
            "RouterProjectionSnapshot",
            Some(contract(
                "ExternalToolSurfaceAuthority visibility + server manifests",
                "snapshot rebuild at every visibility/routing invalidation",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "atomically publish projection snapshot after every authority-driven visibility mutation",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "RouterProjectionSnapshot.tool_to_server",
            Subsystem::Mcp,
            StateClass::DerivedProjection,
            "RouterProjectionSnapshot",
            Some(contract(
                "ExternalToolSurfaceAuthority visibility + server manifests",
                "snapshot rebuild at every visibility/routing invalidation",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "routing map is rebuilt from the same snapshot publication path used by tool visibility",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "RouterProjectionSnapshot.visible_tools",
            Subsystem::Mcp,
            StateClass::DerivedProjection,
            "RouterProjectionSnapshot",
            Some(contract(
                "ExternalToolSurfaceAuthority visibility + server manifests",
                "snapshot rebuild at every visibility/routing invalidation",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "tool listing uses only atomically published snapshot-visible tool set",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "RouterProjectionSnapshot.epoch",
            Subsystem::Mcp,
            StateClass::DerivedProjection,
            "ExternalToolSurfaceAuthority snapshot_epoch",
            Some(contract(
                "ExternalToolSurfaceAuthority snapshot publication epoch",
                "projection snapshot rebuild/publication",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "projection epoch lineage is machine-derived directly from authority snapshot_epoch",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.pending_obligations",
            Subsystem::Mcp,
            StateClass::CapabilityIndex,
            "surface_completion handoff protocol obligation identity",
            Some(contract(
                "generated SurfaceCompletionObligation tokens from authority effects",
                "schedule-surface-completion spawn + pending-result consumption",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "obligation tokens are capability handles consumed only through generated protocol feedback paths",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.pending_snapshot_alignment",
            Subsystem::Mcp,
            StateClass::CapabilityIndex,
            "surface_snapshot_alignment handoff protocol obligation identity",
            Some(contract(
                "generated SurfaceSnapshotAlignmentObligation token from authority effects",
                "snapshot-alignment scheduling + alignment application",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "snapshot-alignment token is an opaque capability consumed only through generated bridge helpers",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.pending_tx",
            Subsystem::Mcp,
            StateClass::CapabilityHandle,
            "surface handoff protocol async completion transport",
            None,
            EntryStatus::Closed,
            "background pending-result sender is an opaque transport capability with no independent semantic truth",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.pending_rx",
            Subsystem::Mcp,
            StateClass::TransportBuffer,
            "surface handoff protocol async completion transport",
            None,
            EntryStatus::Closed,
            "pending-result receiver queue is transport-only buffering for obligation completion delivery",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.completed_updates",
            Subsystem::Mcp,
            StateClass::TransportBuffer,
            "ExternalToolSurfaceAuthority lifecycle deltas",
            None,
            EntryStatus::Closed,
            "queued lifecycle actions are transport-only buffers sourced from authority transitions",
        ),
        state(
            "meerkat-mcp/src/router.rs",
            "McpRouter.staged_payloads",
            Subsystem::Mcp,
            StateClass::TransportBuffer,
            "ExternalToolSurfaceAuthority staged intent sequence",
            None,
            EntryStatus::Closed,
            "treat staged config payloads as transport-only buffers keyed by machine-owned staged intent, not as authoritative staged-order truth",
        ),
        state(
            "meerkat-mob/src/runtime/actor.rs",
            "MobActor.roster",
            Subsystem::Mob,
            StateClass::DerivedProjection,
            "RosterAuthority + spawn/retire/wire event projection contract",
            Some(contract(
                "MeerkatSpawned/Retired + PeersWired/PeersUnwired event lineage + session-bridge assignment updates",
                "spawn finalization, disposal retirement, wire/unwire mutation, resume replay",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "sealed RosterAuthority is now the sole mutator for the roster projection and helper/runtime reads consume authority snapshots only",
        ),
        state(
            "meerkat-mob/src/runtime/actor.rs",
            "MobActor.pending_spawns",
            Subsystem::Mob,
            StateClass::DerivedProjection,
            "PendingSpawnLineage + MobOrchestratorAuthority.pending_spawn_count",
            Some(contract(
                "staged spawn receipts + reply obligations + provision task handles",
                "enqueue_spawn, spawn completion, respawn cancellation, lifecycle drain",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "PendingSpawnLineage now owns metadata/task coupling and all pending-spawn semantics go through its sealed helpers plus orchestrator-count alignment",
        ),
        state(
            "meerkat-mob/src/runtime/pending_spawn_lineage.rs",
            "PendingSpawnLineage.tasks",
            Subsystem::Mob,
            StateClass::CapabilityIndex,
            "PendingSpawnLineage metadata + MobOrchestratorAuthority.pending_spawn_count",
            Some(contract(
                "machine-owned pending spawn set",
                "spawn begin/complete/rollback transitions",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "ticket-to-task-handle reachability only; insertion/removal is coupled to pending spawn lineage helpers and never carries spawn semantics independently",
        ),
        state(
            "meerkat-mob/src/runtime/provisioner.rs",
            "SessionBackend.runtime_sessions",
            Subsystem::Mob,
            StateClass::CapabilityIndex,
            "RuntimeSessionAdapter registered sessions",
            Some(contract(
                "runtime adapter registration truth + runtime bridge sidecar handles",
                "runtime session ensure/reattach + retire/unregister + interrupt stale-bridge cleanup",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "reduced to identity-to-bridge-sidecar reachability; runtime adapter remains canonical for registration lifecycle",
        ),
        state(
            "meerkat-mob/src/runtime/provisioner.rs",
            "RuntimeSessionState.queued_turns",
            Subsystem::Mob,
            StateClass::TransportBuffer,
            "InputLifecycle canonical input identity + runtime primitive contributing ids",
            Some(contract(
                "event transport handles keyed by canonical input ids",
                "accept/dedup rekey + primitive contributing-id consumption + retire/unregister clear",
                StalenessPolicy::Forbidden,
            )),
            EntryStatus::Closed,
            "transport-only turn context buffering; no lifecycle truth or independent sequencing semantics",
        ),
        state(
            "meerkat-mob/src/runtime/ops_adapter.rs",
            "MobOpsAdapter.registry",
            Subsystem::Mob,
            StateClass::CapabilityHandle,
            "RuntimeOpsLifecycleRegistry",
            None,
            EntryStatus::Closed,
            "treat ops registry as opaque access to canonical operation lifecycle truth; spawn receipts carry canonical operation ids and live member-op lookups target non-terminal lifecycle state only",
        ),
    ]
}

fn semantic_operations() -> Vec<SemanticOperationEntry> {
    vec![
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "set_comms_drain_control",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "registered-session contract + CommsDrainLifecycleMachine",
            &[
                "drain control reachability follows registered-session lifetime and active drain slot truth",
            ],
            &["drain slot control linkage cannot outlive registered session"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "register_session",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["sessions", "driver", "ops_lifecycle", "completions"],
            "RuntimeSessionAdapter registration + recovery publication contract",
            &[
                "registered session exists only after recovery succeeds, with stale attachment publication normalized before reuse",
            ],
            &[
                "session keys, recovered driver capability, and completion plumbing remain aligned across registration races",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "set_session_silent_intents",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["driver"],
            "RuntimeIngressMachine + RuntimeControlMachine policy truth",
            &[
                "silent comms intent policy updates are applied only to the canonical driver runtime for that registered session",
            ],
            &["session-scoped policy overrides cannot outlive session registration truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "register_session_with_executor",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["sessions", "attachment", "driver"],
            "RuntimeSessionAdapter registration + attachment publication contract",
            &[
                "session registration and executor attachment establish one canonical runtime identity",
            ],
            &["attachment publication and registered-session truth cannot drift"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "ensure_session_with_executor",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["sessions", "attachment", "driver"],
            "RuntimeSessionAdapter attachment publication contract + RuntimeControl transitions",
            &[
                "executor attachment is published only after driver attach succeeds, with stale attached-driver states repaired via detach/reattach before publication",
            ],
            &["attachment state and loop handle liveness do not drift across ensure/publish paths"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "unregister_session",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["sessions", "comms_drain_slots"],
            "registered-session contract + CommsDrainLifecycleMachine",
            &["session removed and no drain remains live or suppressing"],
            &["drain slots subset session registrations"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "interrupt_current_run",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["driver", "attachment"],
            "RuntimeControlMachine + runtime attachment publication contract",
            &[
                "interrupt requests target only the canonical attached runtime for the registered session",
            ],
            &[
                "attachment publication and control-plane reachability remain aligned with runtime state",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "stop_runtime_executor",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["driver", "completions", "attachment"],
            "RuntimeControlMachine + runtime attachment publication contract",
            &["runtime stop command and fallback path produce canonical stopped/reset semantics"],
            &["attachment and completion waiter surfaces remain aligned with runtime state"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "accept_input_and_run",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["driver", "completions"],
            "RuntimeIngressMachine + InputLifecycleMachine + RuntimeControlMachine",
            &[
                "synchronous compatibility path preserves canonical input acceptance, run staging, boundary commit, and terminalization semantics, and rejects deduplicated admissions deterministically",
            ],
            &[
                "compatibility path cannot bypass authority-owned input/run transition guards or invent execution ownership for deduplicated inputs",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "accept_input_with_completion",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["driver", "completions", "attachment"],
            "RuntimeIngressMachine + InputLifecycleMachine",
            &[
                "accept-with-completion registers waiters strictly from canonical input lifecycle non-terminal states",
            ],
            &[
                "completion handles and wake/process signals remain projection-only against canonical input truth",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "maybe_spawn_comms_drain",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "CommsDrainLifecycleMachine",
            &["drain spawn follows handoff protocol and updates slot projection"],
            &["no live drain without registered session"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "notify_comms_drain_exited",
            BoundaryKind::ManualCallback,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "CommsDrainLifecycleMachine",
            &["exit feedback closes drain lifecycle and updates slot projection"],
            &["drain protocol closure and liveness remain satisfied"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "abort_comms_drains",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "CommsDrainLifecycleMachine",
            &["abort requests transition all tracked drains through canonical abort protocol"],
            &["no slot remains suppressing turn-boundary drain after abort completion"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "abort_comms_drain",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "CommsDrainLifecycleMachine",
            &["single-session abort request follows canonical drain abort protocol"],
            &["aborted drain slot cannot retain stale running/suppression state"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "wait_comms_drain",
            BoundaryKind::PublicInherent,
            "RuntimeSessionAdapter",
            &["comms_drain_slots"],
            "CommsDrainLifecycleMachine",
            &["wait path preserves canonical drain terminalization and safety-net exit reporting"],
            &["drain phase cannot remain Running after joined completion path"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "publish_event",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "input_state"],
            "RuntimeControlMachine + InputLifecycleMachine",
            &[
                "runtime event publication is applied against canonical runtime/input state machines",
            ],
            &["event publication cannot bypass authority-owned transition guards"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "retire",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "completions", "attachment"],
            "RuntimeControlMachine",
            &["retire transitions preserve canonical pending-drain and abandonment semantics"],
            &["retire report and completion closure remain aligned with runtime truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "recycle",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "completions", "attachment"],
            "RuntimeControlMachine",
            &["recoverable work preserved according to driver kind"],
            &["recycle contract matches pre-recycle truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "reset",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "completions"],
            "RuntimeControlMachine",
            &["queued work and waiters terminate according to canonical reset semantics"],
            &["completion registry remains waiter-only"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "recover",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "attachment"],
            "RuntimeControlMachine",
            &["recovery transitions and wake semantics follow canonical runtime lifecycle truth"],
            &["recovered runtime state and attachment wake signaling remain aligned"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "destroy",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "completions", "attachment"],
            "RuntimeControlMachine",
            &[
                "destroy terminalizes runtime and completion waiters according to canonical semantics",
            ],
            &["destroyed runtime cannot leak active input/completion truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-runtime/src/session_adapter.rs",
            "ingest",
            BoundaryKind::TraitImpl,
            "RuntimeSessionAdapter",
            &["driver", "queue", "steer_queue"],
            "RuntimeIngressMachine + InputLifecycleMachine",
            &["accepted work is reflected in canonical ingress/input lifecycle truth"],
            &["driver queues are projections of ingress truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "set_removal_timeout",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["authority"],
            "ExternalToolSurfaceAuthority",
            &[
                "removal-timeout policy mutation is applied only through canonical authority mutator with operating-phase guard",
            ],
            &[
                "shell config surface cannot mutate removal policy after shutdown or invent independent removing-timeout semantics",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "add_server",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["servers", "projection", "pending_obligations"],
            "ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract",
            &["compatibility add path still drives authority and publishes projection snapshot"],
            &["compatibility surface cannot diverge from canonical staged/boundary semantics"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "stage_add",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["staged_payloads", "authority"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &[
                "stage-add intent and payload binding remain consistent with canonical staged intent truth",
            ],
            &["staged payload bookkeeping cannot invent staged operation order"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "stage_remove",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["staged_payloads", "authority"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &["stage-remove intent is machine-owned and shell payload cache is kept consistent"],
            &["remove staging cannot bypass authority ordering semantics"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "stage_reload",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["staged_payloads", "authority"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &[
                "stage-reload intent and payload replacement remain coupled to machine-owned staged truth",
            ],
            &["reload staging cannot create lineage drift against authority staged sequences"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "apply_staged",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &[
                "staged_payloads",
                "pending_obligations",
                "servers",
                "projection",
            ],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &[
                "snapshot and boundary truth reflect applied staged intent through canonical staged ordering, obligation lineage, and published projection snapshots",
            ],
            &["routing/listing come only from published snapshot"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "process_pending_result",
            BoundaryKind::ManualCallback,
            "McpRouter",
            &[
                "pending_obligations",
                "servers",
                "projection",
                "completed_updates",
            ],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &["stale results are rejected and snapshot is atomically rebuilt"],
            &["pending lineage and snapshot freshness remain canonical"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "take_lifecycle_actions",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["completed_updates"],
            "ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract",
            &["lifecycle action draining surfaces only canonical authority-derived deltas"],
            &["drained updates do not mutate canonical lifecycle truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "take_external_updates",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["completed_updates", "projection", "servers"],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &[
                "external update surface returns notices/pending strictly from canonical snapshot and pending lineage",
            ],
            &["external update draining cannot read semantic routing state from raw server table"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "progress_removals",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["servers", "projection"],
            "ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract",
            &[
                "removal finalization uses authority-owned timing/inflight truth and publishes canonical snapshot updates before read return",
            ],
            &["no semantic read from raw servers"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "call_tool",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &["servers", "projection", "active_calls"],
            "ExternalToolSurfaceAuthority",
            &["routing uses published snapshot and inflight truth remains canonical"],
            &["visibility/routing do not read raw servers directly"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/router.rs",
            "shutdown",
            BoundaryKind::PublicInherent,
            "McpRouter",
            &[
                "servers",
                "pending_tx",
                "pending_obligations",
                "completed_updates",
            ],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &["shutdown closes canonical surface lifecycle and drains shell transport resources"],
            &["no pending completion obligations survive terminal shutdown"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "refresh_tools",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "RouterProjectionSnapshot",
            &[
                "compatibility refresh surface preserves canonical snapshot-based routing without introducing side caches",
            ],
            &["refresh path cannot invent independent adapter tool visibility truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "stage_add",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &[
                "adapter stage-add forwards to canonical router boundary without introducing side truth",
            ],
            &["adapter state remains projection-only"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "stage_remove",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &[
                "adapter stage-remove forwards to canonical router boundary without introducing side truth",
            ],
            &["adapter state remains projection-only"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "stage_reload",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority staged intent sequence",
            &[
                "adapter stage-reload forwards to canonical router boundary without introducing side truth",
            ],
            &["adapter state remains projection-only"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "apply_staged",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["has_pending"],
            "RouterProjectionSnapshot",
            &["adapter projections mirror router snapshot only"],
            &["adapter carries no independent lifecycle truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "poll_lifecycle_actions",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority + RouterProjectionSnapshot publication contract",
            &["adapter lifecycle polling drains canonical router completion notices only"],
            &["adapter pending flag must mirror router pending/notices truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "progress_removals",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract",
            &[
                "adapter forwards removal progression to canonical router and refreshes pending projection",
            ],
            &["adapter pending flag must mirror router pending/notices truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "wait_until_ready",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &["wait loop reports readiness from canonical pending/notices state only"],
            &[
                "timeout and readiness signaling cannot classify availability from non-canonical side state",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mcp/src/adapter.rs",
            "shutdown",
            BoundaryKind::PublicInherent,
            "McpRouterAdapter",
            &["router", "has_pending"],
            "ExternalToolSurfaceAuthority + surface_completion/snapshot_alignment handoff protocols + RouterProjectionSnapshot publication contract",
            &["adapter shutdown drains router and clears pending projection state"],
            &["adapter cannot report pending work after router shutdown"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &[
                "spawn wrapper delegates to canonical spawn receipt path without introducing helper-owned lifecycle truth",
            ],
            &["spawn convenience surface cannot diverge from canonical spawn lineage"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_with_backend",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &["backend override wrapper delegates to canonical spawn receipt path"],
            &["backend convenience surface cannot diverge from canonical spawn lineage"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_with_options",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &["option-rich spawn wrapper delegates to canonical spawn receipt path"],
            &[
                "spawn option synthesis cannot invent lifecycle truth outside canonical spawn command",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "attach_existing_session",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns", "runtime_bridge"],
            "PendingSpawnLineage + SessionBackend runtime bridge",
            &[
                "resume-session attach path reuses canonical spawn/bridge lifecycle command semantics",
            ],
            &["attach convenience path cannot bypass canonical member/runtime bridge ownership"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "attach_existing_session_as_orchestrator",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns", "runtime_bridge"],
            "PendingSpawnLineage + SessionBackend runtime bridge",
            &["orchestrator attach wrapper delegates to canonical attach-existing-session path"],
            &["orchestrator attach convenience surface cannot fork bridge truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "attach_existing_session_as_member",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns", "runtime_bridge"],
            "PendingSpawnLineage + SessionBackend runtime bridge",
            &["member attach wrapper delegates to canonical attach-existing-session path"],
            &["member attach convenience surface cannot fork bridge truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_spec",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &["spawn-spec convenience path delegates to canonical spawn receipt boundary"],
            &["spawn-spec wrapper cannot diverge from canonical spawn lineage"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_spec_receipt",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &["spawn lineage is authority-owned and receipt reflects canonical member truth"],
            &["pending spawn lineage cannot drift from canonical roster truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_many",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &[
                "batch spawn wrapper preserves per-spec canonical spawn lineage and ordering contract",
            ],
            &["batch convenience path cannot classify spawn success outside canonical receipts"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_many_receipts",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["pending_spawns"],
            "PendingSpawnLineage + RosterAuthority",
            &[
                "batch receipt path preserves per-spec canonical spawn lineage and ordering contract",
            ],
            &[
                "batch receipt convenience path cannot classify spawn outcomes outside canonical receipts",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "retire",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "retired_event_index"],
            "RosterAuthority + disposal pipeline",
            &["retire surface reflects canonical member lifecycle truth"],
            &["member/session removal follows lifecycle invariants"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "respawn",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "pending_spawns", "wiring"],
            "respawn helper contract + PendingSpawnLineage + RosterAuthority",
            &["respawn helper follows documented composed contract"],
            &["helper does not invent lifecycle truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "retire_all",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "pending_spawns"],
            "PendingSpawnLineage + RosterAuthority + disposal pipeline",
            &["bulk retire path preserves canonical member lifecycle transitions for each member"],
            &["bulk retire cannot leave member lifecycle truth partially applied"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "wire",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "wiring", "edge_locks"],
            "RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline",
            &["wire mutation reflects canonical peer-graph lifecycle and trust mutation rules"],
            &["wiring projection and trust edge mutation remain aligned"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "unwire",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "wiring", "edge_locks"],
            "RosterAuthority wiring projection contract + trust-edge mutation + edge-lock discipline",
            &["unwire mutation reflects canonical peer-graph lifecycle and trust mutation rules"],
            &["wiring projection and trust edge mutation remain aligned"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "internal_turn",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["runtime_bridge", "roster"],
            "SessionBackend runtime bridge + InputLifecycle truth",
            &[
                "internal-turn wrapper submits work only through canonical member runtime bridge path",
            ],
            &["internal-turn convenience surface cannot invent session routing truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "run_flow",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["orchestrator", "flow_streams", "run_store"],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &[
                "flow run submission aligns with canonical orchestrator and lifecycle transition truth",
            ],
            &["flow wrapper cannot fabricate run lifecycle state outside canonical tracker"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "run_flow_with_stream",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["orchestrator", "flow_streams", "run_store"],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &["streaming flow run submission aligns with canonical orchestrator/run-store truth"],
            &[
                "streaming flow wrapper cannot fork lifecycle classification from canonical run state",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "cancel_flow",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["run_cancel_tokens", "orchestrator", "run_store"],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &["flow cancellation surface targets canonical in-flight run truth"],
            &["cancel surface cannot mark runs canceled outside canonical cleanup path"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "stop",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "pending_spawns", "runtime_bridge"],
            "MobLifecycleAuthority + RosterAuthority",
            &[
                "stop command transitions lifecycle through canonical authority and drains pending spawn/runtime work",
            ],
            &["stop convenience surface cannot leave helper-owned lifecycle truth behind"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "resume",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "runtime_bridge"],
            "MobLifecycleAuthority + RosterAuthority",
            &["resume command transitions lifecycle through canonical authority"],
            &["resume convenience surface cannot bypass canonical lifecycle guards"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "complete",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "roster", "runtime_bridge"],
            "MobLifecycleAuthority + RosterAuthority",
            &[
                "complete command transitions lifecycle and retirement semantics through canonical authority paths",
            ],
            &[
                "complete convenience surface cannot classify completion outside canonical lifecycle state",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "reset",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "roster", "runtime_bridge", "pending_spawns"],
            "MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge",
            &["reset command clears runtime/member state through canonical authority transitions"],
            &["reset convenience surface cannot preserve stale helper-owned member/runtime truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "destroy",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "roster", "runtime_bridge", "pending_spawns"],
            "MobLifecycleAuthority + RosterAuthority + SessionBackend runtime bridge",
            &[
                "destroy command terminalizes runtime/member state through canonical authority transitions",
            ],
            &["destroy convenience surface cannot leak active member/runtime truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "task_create",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["task_board"],
            "MobTaskBoardService event + projection contract",
            &["task create command applies through canonical task-board authority semantics"],
            &["task helper surface cannot invent independent task truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "task_update",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["task_board"],
            "MobTaskBoardService event + projection contract",
            &["task update command applies through canonical task-board authority semantics"],
            &["task helper surface cannot invent independent task truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "set_spawn_policy",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["spawn_policy"],
            "MobSpawnPolicySurface",
            &["spawn-policy updates route through canonical actor command path"],
            &["spawn policy convenience surface cannot create side-owned auto-spawn semantics"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "shutdown",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["lifecycle", "runtime_bridge", "pending_spawns", "run_store"],
            "MobLifecycleAuthority + SessionBackend runtime bridge",
            &[
                "shutdown command drains runtime/flow resources through canonical actor shutdown path",
            ],
            &[
                "shutdown convenience surface cannot leave live semantic resources without canonical closure",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "force_cancel_member",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "runtime_bridge"],
            "SessionBackend runtime bridge + InputLifecycle truth",
            &["force-cancel requests target canonical in-flight member run truth"],
            &["cancel path cannot retire or rewire members implicitly"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "spawn_helper",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "pending_spawns"],
            "MobMemberTerminalClassifier",
            &["helper result class is machine-derived"],
            &["helper does not classify from snapshots"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "fork_helper",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster", "pending_spawns"],
            "MobMemberTerminalClassifier",
            &["helper result class is machine-derived"],
            &["helper does not classify from snapshots"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "wait_one",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster"],
            "MobMemberTerminalClassifier",
            &["wait helper observes canonical terminal truth only"],
            &["wait helper does not infer completion from side maps"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/handle.rs",
            "wait_all",
            BoundaryKind::PublicInherent,
            "MobHandle",
            &["roster"],
            "MobMemberTerminalClassifier",
            &["wait helper observes canonical terminal truth only"],
            &["wait helper does not infer completion from side maps"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_spawn_provisioned_batch",
            BoundaryKind::ManualCallback,
            "MobActor",
            &["pending_spawns", "roster"],
            "PendingSpawnLineage + RosterAuthority + PendingProvision rollback contract",
            &[
                "spawn completion callback finalizes or rolls back pending spawn lineage against canonical roster/runtime truth",
            ],
            &[
                "pending spawn lineage remains aligned with orchestrator counts and committed roster state",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "enqueue_spawn",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["pending_spawns", "roster"],
            "PendingSpawnLineage + MobOrchestratorAuthority + RosterAuthority",
            &[
                "spawn admission allocates canonical pending-spawn lineage before async provisioning and rejects duplicate member identities against pending+committed roster truth",
            ],
            &[
                "pending spawn lineage stays aligned with orchestrator counts and cannot diverge from committed roster membership",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_force_cancel",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["runtime_bridge", "roster"],
            "MobLifecycleAuthority active-member gate + SessionBackend::interrupt_member runtime-adapter ownership contract + InputLifecycle cancellation semantics",
            &[
                "force-cancel command resolves target membership in roster and routes runtime-backed cancellation through RuntimeSessionAdapter when adapter registration exists",
            ],
            &["force-cancel cannot mutate member lifecycle or wiring truth"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_retire",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["roster", "wiring", "runtime_bridge"],
            "RosterAuthority + disposal pipeline + SessionBackend retire contract",
            &[
                "retire command tears down wiring/runtime state and removes the member from the canonical roster projection through disposal sequencing",
            ],
            &["member removal, wiring cleanup, archive, and bridge teardown remain aligned"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_respawn",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["roster", "pending_spawns"],
            "respawn helper contract + PendingSpawnLineage + RosterAuthority",
            &[
                "respawn snapshots canonical member inputs, retires through canonical teardown, stages replacement lineage, and restores peer wiring only through canonical roster/wiring truth",
            ],
            &[
                "respawn does not invent atomic rollback semantics beyond the documented composed contract",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_wire",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["roster", "wiring", "edge_locks"],
            "Roster wiring projection contract + trust-edge mutation + edge-lock discipline",
            &[
                "wire command establishes trust edges, emits canonical wiring events, and only then publishes reciprocal roster projection updates",
            ],
            &["wiring projection, trust-edge mutation, and edge-lock discipline remain coupled"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_unwire",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["roster", "wiring", "edge_locks"],
            "Roster wiring projection contract + trust-edge mutation + edge-lock discipline",
            &[
                "unwire command tears down trust edges and emits canonical unwire events before publishing reciprocal roster projection updates",
            ],
            &["wiring projection, trust-edge mutation, and edge-lock discipline remain coupled"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_external_turn",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["runtime_bridge", "pending_spawns", "roster"],
            "RosterAuthority + SessionBackend runtime bridge + spawn_from_policy_inline contract",
            &[
                "external turn routing and optional auto-spawn path preserve canonical runtime/member membership truth before dispatch",
            ],
            &[
                "delivery semantics cannot branch on shell-only side maps or bypass staged pending-spawn lineage",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_internal_turn",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["runtime_bridge", "roster"],
            "RosterAuthority + SessionBackend runtime bridge",
            &["internal turn routing preserves canonical runtime/member truth before dispatch"],
            &["delivery semantics cannot branch on shell-only side maps"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "retire_all_members",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["roster", "runtime_bridge", "pending_spawns"],
            "PendingSpawnLineage + RosterAuthority + disposal pipeline",
            &[
                "bulk-retire first drains pending spawn lineage, then retires every canonical roster member through the same teardown path",
            ],
            &[
                "bulk-retire cannot leave partially retired member/runtime linkage or orphan pending spawns",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_task_create",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["task_board"],
            "MobTaskBoardService event + projection contract",
            &["task-create command applies canonical task board transition"],
            &["task-create command cannot bypass canonical task-board validation"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_task_update",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["task_board"],
            "MobTaskBoardService event + projection contract",
            &["task-update command applies canonical task board transition"],
            &["task-update command cannot bypass canonical task-board validation"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_run_flow",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &[
                "orchestrator",
                "run_tasks",
                "run_cancel_tokens",
                "run_store",
            ],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &["run-flow command starts canonical run lifecycle and orchestrator accounting"],
            &[
                "run-flow command cannot fabricate lifecycle/tracker truth outside canonical authorities",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_cancel_flow",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["run_tasks", "run_cancel_tokens", "run_store"],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &["cancel-flow command updates canonical run lifecycle and orchestrator accounting"],
            &[
                "cancel-flow command cannot classify run terminal state outside canonical authorities",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_flow_cleanup",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["run_tasks", "run_cancel_tokens", "flow_streams"],
            "MobOrchestratorAuthority + MobLifecycleAuthority",
            &["flow cleanup command closes run trackers and applies canonical completion inputs"],
            &[
                "cleanup commands cannot complete run lifecycle outside canonical authority transitions",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_complete",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["lifecycle", "roster", "runtime_bridge"],
            "MobLifecycleAuthority + retire_all_members + PendingSpawnLineage",
            &[
                "complete command cancels flow work, drains pending spawn lineage, retires canonical roster members, and only then publishes lifecycle completion",
            ],
            &[
                "completed lifecycle cannot be published while canonical member/runtime teardown remains open",
            ],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_destroy",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["lifecycle", "roster", "runtime_bridge", "pending_spawns"],
            "MobLifecycleAuthority + retire_all_members + PendingSpawnLineage",
            &[
                "destroy command drains pending lineage, retires canonical members, and only then publishes lifecycle destroy semantics",
            ],
            &["destroy command cannot leak active member/runtime truth after terminalization"],
            EntryStatus::Closed,
        ),
        op(
            "meerkat-mob/src/runtime/actor.rs",
            "handle_reset",
            BoundaryKind::EnumDispatch,
            "MobActor",
            &["lifecycle", "roster", "runtime_bridge", "pending_spawns"],
            "MobLifecycleAuthority + retire_all_members + PendingSpawnLineage",
            &[
                "reset command drains pending lineage, retires canonical members, and only then returns lifecycle to the reset-prepared state",
            ],
            &["reset command cannot preserve stale pending/member/runtime truth"],
            EntryStatus::Closed,
        ),
    ]
}

fn coupling_invariants() -> Vec<CouplingInvariantEntry> {
    vec![
        invariant(
            "runtime_session_drain_subset",
            Subsystem::Runtime,
            &[
                "RuntimeSessionAdapter.sessions",
                "RuntimeSessionAdapter.comms_drain_slots",
            ],
            "keys(comms_drain_slots) subset keys(sessions)",
            "registered-session contract + CommsDrainLifecycleMachine",
            "shell synchronization + drain lifecycle protocol + ownership-ledger",
            EntryStatus::Closed,
        ),
        invariant(
            "runtime_attachment_alignment",
            Subsystem::Runtime,
            &[
                "RuntimeSessionEntry.attachment",
                "RuntimeSessionEntry.driver",
            ],
            "live attachment publication is aligned with driver attachment/control transitions on stop/ensure paths",
            "RuntimeSessionAdapter attachment publication contract + RuntimeControl transitions",
            "driver attachment semantics + ownership-ledger",
            EntryStatus::Closed,
        ),
        invariant(
            "runtime_queue_projection_alignment",
            Subsystem::Runtime,
            &[
                "RuntimeIngressMachine.queue",
                "EphemeralRuntimeDriver.queue",
                "EphemeralRuntimeDriver.steer_queue",
            ],
            "physical driver queues are projections of canonical ingress lanes and ordering metadata",
            "RuntimeIngressMachine",
            "runtime ingress authority + driver boundary rebuild + ownership-ledger",
            EntryStatus::Closed,
        ),
        invariant(
            "mcp_snapshot_alignment",
            Subsystem::Mcp,
            &["McpRouter.servers", "RouterProjectionSnapshot"],
            "routing and visible tools derive only from the atomically published snapshot",
            "ExternalToolSurfaceAuthority + surface_snapshot_alignment handoff protocol + RouterProjectionSnapshot publication contract",
            "machine + ownership-ledger + router snapshot publication path",
            EntryStatus::Closed,
        ),
        invariant(
            "mcp_pending_lineage_alignment",
            Subsystem::Mcp,
            &[
                "McpRouter.pending_obligations",
                "ExternalToolSurfaceAuthority pending lineage + surface_completion obligations",
            ],
            "pending result lineage and stale-result rejection are machine-owned",
            "ExternalToolSurfaceAuthority pending lineage + surface_completion handoff protocol obligations",
            "machine + handoff protocols + ownership-ledger + generated pending-result checks",
            EntryStatus::Closed,
        ),
        invariant(
            "mob_pending_spawn_alignment",
            Subsystem::Mob,
            &[
                "MobActor.pending_spawns",
                "PendingSpawnLineage.tasks",
                "MobOrchestratorAuthority.pending_spawn_count",
            ],
            "pending spawn tables and orchestrator counts must describe the same spawn lineage",
            "pending spawn lineage helpers + MobOrchestratorAuthority.pending_spawn_count",
            "actor-side alignment enforcement + orchestrator authority + ownership-ledger",
            EntryStatus::Closed,
        ),
        invariant(
            "mob_runtime_bridge_alignment",
            Subsystem::Mob,
            &[
                "SessionBackend.runtime_sessions",
                "RuntimeSessionState.queued_turns",
                "MobOpsAdapter.registry",
            ],
            "runtime-backed bridge tables must agree on member/session/runtime association",
            "SessionBackend runtime session sidecar contract + RuntimeSessionAdapter registration truth + RuntimeOpsLifecycleRegistry",
            "runtime sidecar repair/clear paths + canonical input-id keyed queued-turn transport + fail-closed ops adapter bridge checks",
            EntryStatus::Closed,
        ),
        invariant(
            "mob_wiring_alignment",
            Subsystem::Mob,
            &["Roster.wired_to", "trust edges", "edge locks"],
            "roster wiring projection, trust-edge mutation, and lock discipline must describe one canonical peer graph",
            "Roster wiring projection contract + do_wire/handle_unwire edge-lock discipline",
            "roster projection helpers + lifecycle rollback + ownership-ledger",
            EntryStatus::Closed,
        ),
    ]
}

fn contract(
    rebuild_source: &str,
    rebuild_trigger: &str,
    staleness_policy: StalenessPolicy,
) -> ProjectionContract {
    ProjectionContract {
        rebuild_source: rebuild_source.into(),
        rebuild_trigger: rebuild_trigger.into(),
        staleness_policy,
    }
}

fn state(
    path: &str,
    symbol: &str,
    subsystem: Subsystem,
    class: StateClass,
    canonical_anchor: &str,
    projection: Option<ProjectionContract>,
    status: EntryStatus,
    closure_action: &str,
) -> StateCellEntry {
    StateCellEntry {
        path: path.into(),
        symbol: symbol.into(),
        subsystem,
        class,
        canonical_anchor: canonical_anchor.into(),
        projection,
        status,
        closure_action: closure_action.into(),
    }
}

fn op(
    path: &str,
    symbol: &str,
    boundary_kind: BoundaryKind,
    owner_shell: &str,
    writeset: &[&str],
    anchor: &str,
    required_postconditions: &[&str],
    preserved_invariants: &[&str],
    status: EntryStatus,
) -> SemanticOperationEntry {
    SemanticOperationEntry {
        path: path.into(),
        symbol: symbol.into(),
        boundary_kind,
        owner_shell: owner_shell.into(),
        writeset: writeset.iter().map(|item| (*item).to_string()).collect(),
        anchor: anchor.into(),
        required_postconditions: required_postconditions
            .iter()
            .map(|item| (*item).to_string())
            .collect(),
        preserved_invariants: preserved_invariants
            .iter()
            .map(|item| (*item).to_string())
            .collect(),
        status,
    }
}

fn invariant(
    name: &str,
    subsystem: Subsystem,
    stores: &[&str],
    invariant_text: &str,
    anchor: &str,
    enforcement_mode: &str,
    status: EntryStatus,
) -> CouplingInvariantEntry {
    CouplingInvariantEntry {
        name: name.into(),
        subsystem,
        stores: stores.iter().map(|item| (*item).to_string()).collect(),
        invariant: invariant_text.into(),
        anchor: anchor.into(),
        enforcement_mode: enforcement_mode.into(),
        status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public_contracts::repo_root;

    #[test]
    fn capability_index_requires_contract_when_keyed() {
        let registry = ownership_registry();
        assert!(registry.state_cells.iter().any(|entry| {
            entry.class == StateClass::CapabilityIndex && entry.projection.is_some()
        }));
    }

    #[test]
    fn manifest_callback_entries_are_justified() {
        let manifest = boundary_manifest();
        assert!(manifest.callbacks.iter().all(|callback| {
            !callback.compensating_family.is_empty()
                && !callback.why_manual.is_empty()
                && !callback.sunset_condition.is_empty()
        }));
    }

    #[test]
    fn manifest_boundaries_match_operation_entries() {
        let root = repo_root().expect("repo root");
        let findings = collect_current_findings(&root).expect("ownership findings");
        let structural = findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.key.rule.as_str(),
                    "OwnershipMissingOperationEntry" | "OwnershipExtraOperationEntry"
                )
            })
            .collect::<Vec<_>>();
        assert!(
            structural.is_empty(),
            "manifest/operation drift: {structural:#?}"
        );
    }
}
