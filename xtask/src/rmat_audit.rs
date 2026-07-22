use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Expr, ExprAssign, ExprField, ExprIf, ExprMethodCall, ImplItem, ImplItemFn, Item, ItemEnum,
    ItemFn, ItemImpl, ItemStruct, ItemUnion, Member, Type,
};

use crate::ownership_ledger;
use crate::ownership_ledger::{OwnershipFinding, OwnershipFindingKey, format_ownership_finding};
use crate::public_contracts::repo_root;
use crate::rmat_policy::{AuditPolicy, ForbiddenShellReadKind, ForbiddenShellReadRule};

#[derive(Debug, Clone, Args)]
pub struct RmatAuditArgs {
    #[arg(long)]
    pub strict: bool,
    #[arg(long)]
    pub json: bool,
    #[arg(long)]
    pub update_baseline: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FindingKey {
    pub rule: String,
    pub path: String,
    pub symbol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub key: FindingKey,
    pub severity: String,
    pub message: String,
    pub suppressed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Baseline {
    #[serde(default)]
    pub finding: Vec<FindingKey>,
}

pub fn rmat_audit(args: RmatAuditArgs) -> Result<()> {
    let root = repo_root()?;
    let policy = AuditPolicy::load()?;
    let findings = collect_findings(&root, &policy)?;
    let ownership_findings = ownership_ledger::collect_current_findings(&root)?;
    let ownership_doc_in_sync = ownership_ledger::ownership_doc_is_in_sync(&root)?;
    let ownership_rmat_findings = ownership_findings
        .iter()
        .map(|finding| Finding {
            key: FindingKey {
                rule: finding.key.rule.clone(),
                path: finding.key.path.clone(),
                symbol: finding.key.symbol.clone(),
            },
            severity: finding.severity.clone(),
            message: finding.message.clone(),
            suppressed: false,
        })
        .collect::<Vec<_>>();
    let mut combined_findings = findings.clone();
    combined_findings.extend(ownership_rmat_findings);
    if !ownership_doc_in_sync {
        combined_findings.push(Finding {
            key: FindingKey {
                rule: "OwnershipLedgerDocDrift".into(),
                path: "docs-internal/archive/public-docs-removed-2026-05-11/architecture/finite-ownership-ledger.md".into(),
                symbol: "finite-ownership-ledger".into(),
            },
            severity: "error".into(),
            message: "generated ownership ledger doc is stale relative to typed ownership registry"
                .into(),
            suppressed: false,
        });
    }
    combined_findings.sort_by(|a, b| a.key.cmp(&b.key));
    combined_findings.dedup_by(|a, b| a.key == b.key);

    if args.json {
        let combined_findings_json = serde_json::to_string_pretty(&combined_findings)?;
        println!("{combined_findings_json}");
    } else {
        print_findings(&combined_findings);
    }

    if !ownership_doc_in_sync {
        bail!(
            "ownership ledger doc drift:\n- docs-internal/archive/public-docs-removed-2026-05-11/architecture/finite-ownership-ledger.md is stale relative to typed ownership registry"
        );
    }

    let baseline_path = root.join("xtask/rmat-baseline.toml");
    let ownership_baseline_path = root.join("xtask/ownership-baseline.toml");
    if args.update_baseline {
        write_baseline(&baseline_path, &findings)?;
        ownership_ledger::write_baseline(&ownership_baseline_path, &ownership_findings)?;
        let baseline_display = baseline_path.display();
        let ownership_baseline_display = ownership_baseline_path.display();
        println!("updated {baseline_display}");
        println!("updated {ownership_baseline_display}");
        return Ok(());
    }

    let baseline = read_baseline(&baseline_path)?;
    let baseline_set: BTreeSet<_> = baseline.finding.into_iter().collect();
    let hard_unsuppressed: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == "error" && !f.suppressed)
        .collect();

    let new_findings: Vec<_> = hard_unsuppressed
        .iter()
        .filter(|f| !baseline_set.contains(&f.key))
        .collect();

    let stale_baseline: Vec<_> = baseline_set
        .iter()
        .filter(|key| !hard_unsuppressed.iter().any(|finding| &finding.key == *key))
        .collect();
    let (new_ownership_findings, stale_ownership_baseline) =
        ownership_ledger::diff_against_baseline(&ownership_baseline_path, &ownership_findings)?;

    if let Some(details) =
        format_ownership_diff_messages(&new_ownership_findings, &stale_ownership_baseline)
    {
        bail!(
            "ownership audit diff detected (run `xtask ownership-ledger` and update the baseline):\n{details}"
        );
    }

    if args.strict && (!new_findings.is_empty() || !stale_baseline.is_empty()) {
        let mut messages = Vec::new();
        if !new_findings.is_empty() {
            let new_findings_body = new_findings
                .into_iter()
                .map(|finding| format_finding(finding))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(format!("new RMAT findings:\n{new_findings_body}"));
        }
        if !stale_baseline.is_empty() {
            let stale_rmat_baseline = stale_baseline
                .into_iter()
                .map(|finding| format!("- {} {} {}", finding.rule, finding.path, finding.symbol))
                .collect::<Vec<_>>()
                .join("\n");
            messages.push(format!(
                "stale RMAT baseline entries:\n{stale_rmat_baseline}"
            ));
        }
        let joined_messages = messages.join("\n\n");
        bail!("{joined_messages}");
    }

    Ok(())
}

pub fn collect_findings(root: &Path, policy: &AuditPolicy) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    let files = collect_repo_rs_files(root)?;
    let route_coverage = collect_route_realization_findings(root, policy);

    for file in files {
        let relative = file
            .strip_prefix(root)
            .with_context(|| format!("strip repo root from {}", file.display()))?
            .to_string_lossy()
            .to_string();
        let source =
            fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
        let parsed =
            syn::parse_file(&source).with_context(|| format!("parse {}", file.display()))?;

        findings.extend(collect_dead_authority_wiring_findings(
            &relative, &source, &parsed, policy,
        ));

        let mut guarded_apply = GuardedApplyVisitor::new(&relative, &source);
        guarded_apply.visit_file(&parsed);
        findings.extend(guarded_apply.findings);

        let mut parallel = ParallelTransitionVisitor::new(&relative, &source, policy);
        parallel.visit_file(&parsed);
        findings.extend(parallel.findings);

        let mut protected_writes = ProtectedFieldWriteVisitor::new(&relative, &source, policy);
        protected_writes.visit_file(&parsed);
        findings.extend(protected_writes.findings);

        let mut suspicion = SuspicionVisitor::new(&relative, &source);
        suspicion.visit_file(&parsed);
        findings.extend(suspicion.findings);

        let mut typed_carrier =
            crate::typed_carrier::TypedCarrierAdoptionVisitor::new(&relative, &source);
        typed_carrier.visit_file(&parsed);
        findings.extend(typed_carrier.findings);

        let applicable_forbidden_rules: Vec<&ForbiddenShellReadRule> = policy
            .forbidden_shell_reads
            .iter()
            .filter(|rule| relative.ends_with(rule.path_suffix))
            .collect();
        if !applicable_forbidden_rules.is_empty() {
            let mut forbidden =
                ForbiddenShellReadsVisitor::new(&relative, &source, applicable_forbidden_rules);
            forbidden.visit_file(&parsed);
            findings.extend(forbidden.findings);
        }
    }

    findings.extend(route_coverage);

    // Structural seam rules (hard errors)
    findings.extend(collect_handoff_protocol_coverage_findings(policy));
    findings.extend(collect_protocol_realization_site_findings(root, policy));
    findings.extend(collect_protocol_feedback_constraint_findings(root, policy));
    findings.extend(collect_terminal_mapping_constraint_findings(root, policy));
    findings.extend(collect_runtime_comms_bridge_projection_findings(root));
    findings.extend(collect_runtime_external_event_projection_findings(root));
    findings.extend(collect_schema_emitted_request_consumption_findings(root)?);

    findings.sort_by(|a, b| a.key.cmp(&b.key));
    findings.dedup_by(|a, b| a.key == b.key);
    Ok(findings)
}

fn collect_schema_emitted_request_consumption_findings(root: &Path) -> Result<Vec<Finding>> {
    let emit_path = root.join("meerkat-contracts/src/emit.rs");
    if !emit_path.exists() {
        return Ok(Vec::new());
    }
    let emit_source =
        fs::read_to_string(&emit_path).with_context(|| format!("read {}", emit_path.display()))?;
    let request_names = schema_emitted_request_names_from_source(&emit_source);
    let global_aliases = schema_request_global_aliases(root, &request_names)?;
    let mut production_uses: BTreeMap<String, BTreeSet<String>> = request_names
        .iter()
        .cloned()
        .map(|name| (name, BTreeSet::new()))
        .collect();

    for file in collect_repo_rs_files(root)? {
        let relative = file
            .strip_prefix(root)
            .with_context(|| format!("strip repo root from {}", file.display()))?
            .to_string_lossy()
            .to_string();
        if is_schema_request_consumption_excluded_path(&relative) {
            continue;
        }
        let source =
            fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
        let mut parsed =
            syn::parse_file(&source).with_context(|| format!("parse {}", file.display()))?;
        strip_cfg_test_items(&mut parsed.items);
        let mut aliases = global_aliases.clone();
        aliases.extend(schema_request_use_aliases_in_file(&parsed, &request_names));
        for request_name in schema_request_uses_in_file(&parsed, &request_names, &aliases) {
            production_uses
                .entry(request_name)
                .or_default()
                .insert(relative.clone());
        }
    }

    let mut findings = Vec::new();
    for request_name in request_names {
        if production_uses
            .get(&request_name)
            .is_none_or(BTreeSet::is_empty)
        {
            findings.push(error_finding(
                "SchemaEmittedRequestHasProducer",
                "meerkat-contracts/src/emit.rs",
                &request_name,
                format!(
                    "`{request_name}` is emitted as a public request schema but has no non-test production AST consumption path outside contract emission; wire the operation or stop emitting the schema"
                ),
                false,
            ));
        }
    }
    Ok(findings)
}

fn schema_emitted_request_names_from_source(source: &str) -> BTreeSet<String> {
    source
        .lines()
        .filter(|line| line.contains("schema_for!"))
        .filter_map(|line| {
            let start = line.find('"')?;
            let rest = &line[start + 1..];
            let end = rest.find('"')?;
            let name = &rest[..end];
            name.ends_with("Request").then(|| name.to_string())
        })
        .collect()
}

fn is_schema_request_consumption_excluded_path(relative: &str) -> bool {
    relative == "meerkat-contracts/src/emit.rs"
        || relative.starts_with("xtask/")
        || relative.starts_with("tests/")
        || relative.contains("/tests/")
        || relative.ends_with("/tests.rs")
        || relative.ends_with("_test.rs")
}

fn schema_request_global_aliases(
    root: &Path,
    request_names: &BTreeSet<String>,
) -> Result<BTreeMap<String, String>> {
    let mut aliases = BTreeMap::new();
    for file in collect_repo_rs_files(root)? {
        let relative = file
            .strip_prefix(root)
            .with_context(|| format!("strip repo root from {}", file.display()))?
            .to_string_lossy()
            .to_string();
        if is_schema_request_consumption_excluded_path(&relative) {
            continue;
        }
        let source =
            fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
        let mut parsed =
            syn::parse_file(&source).with_context(|| format!("parse {}", file.display()))?;
        strip_cfg_test_items(&mut parsed.items);
        collect_schema_request_type_aliases(&parsed, request_names, &mut aliases);
    }
    Ok(aliases)
}

fn collect_schema_request_type_aliases(
    parsed: &syn::File,
    request_names: &BTreeSet<String>,
    aliases: &mut BTreeMap<String, String>,
) {
    for item in &parsed.items {
        let Item::Type(item_type) = item else {
            continue;
        };
        let request_name = item_type.ident.to_string();
        if !request_names.contains(&request_name) {
            continue;
        }
        let Some(target_name) = type_ident(&item_type.ty) else {
            continue;
        };
        if target_name != request_name {
            aliases.insert(target_name, request_name);
        }
    }
}

fn schema_request_use_aliases_in_file(
    parsed: &syn::File,
    request_names: &BTreeSet<String>,
) -> BTreeMap<String, String> {
    let mut aliases = BTreeMap::new();
    for item in &parsed.items {
        let Item::Use(item_use) = item else {
            continue;
        };
        collect_schema_request_use_aliases(&item_use.tree, request_names, &mut aliases);
    }
    aliases
}

fn collect_schema_request_use_aliases(
    tree: &syn::UseTree,
    request_names: &BTreeSet<String>,
    aliases: &mut BTreeMap<String, String>,
) {
    match tree {
        syn::UseTree::Path(path) => {
            collect_schema_request_use_aliases(&path.tree, request_names, aliases);
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_schema_request_use_aliases(tree, request_names, aliases);
            }
        }
        syn::UseTree::Rename(rename) => {
            let request_name = rename.ident.to_string();
            if request_names.contains(&request_name) {
                aliases.insert(rename.rename.to_string(), request_name);
            }
        }
        syn::UseTree::Name(_) | syn::UseTree::Glob(_) => {}
    }
}

fn schema_request_uses_in_file(
    parsed: &syn::File,
    request_names: &BTreeSet<String>,
    aliases: &BTreeMap<String, String>,
) -> BTreeSet<String> {
    let mut visitor = SchemaRequestUseVisitor {
        request_names,
        aliases,
        uses: BTreeSet::new(),
    };
    visitor.visit_file(parsed);
    visitor.uses
}

struct SchemaRequestUseVisitor<'a> {
    request_names: &'a BTreeSet<String>,
    aliases: &'a BTreeMap<String, String>,
    uses: BTreeSet<String>,
}

impl SchemaRequestUseVisitor<'_> {
    fn record_path(&mut self, path: &syn::Path) {
        let Some(segment) = path.segments.last() else {
            return;
        };
        let name = segment.ident.to_string();
        if self.request_names.contains(&name) {
            self.uses.insert(name);
        } else if let Some(request_name) = self.aliases.get(&name) {
            self.uses.insert(request_name.clone());
        }
    }
}

impl<'ast> Visit<'ast> for SchemaRequestUseVisitor<'_> {
    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if self.request_names.contains(&node.ident.to_string()) {
            return;
        }
        visit::visit_item_type(self, node);
    }

    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        self.record_path(&node.path);
        visit::visit_type_path(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        self.record_path(&node.path);
        visit::visit_expr_struct(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        self.record_path(&node.path);
        visit::visit_expr_path(self, node);
    }

    fn visit_pat_struct(&mut self, node: &'ast syn::PatStruct) {
        self.record_path(&node.path);
        visit::visit_pat_struct(self, node);
    }
}

fn read_baseline(path: &Path) -> Result<Baseline> {
    if !path.exists() {
        return Ok(Baseline::default());
    }
    let contents = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&contents).with_context(|| format!("parse {}", path.display()))
}

fn write_baseline(path: &Path, findings: &[Finding]) -> Result<()> {
    let baseline = Baseline {
        finding: findings
            .iter()
            .filter(|f| f.severity == "error" && !f.suppressed)
            .map(|f| f.key.clone())
            .collect(),
    };
    let toml = toml::to_string_pretty(&baseline)?;
    fs::write(path, toml).with_context(|| format!("write {}", path.display()))
}

fn print_findings(findings: &[Finding]) {
    if findings.is_empty() {
        println!("RMAT audit: no findings");
        return;
    }
    println!("RMAT audit findings:");
    for finding in findings {
        let formatted_finding = format_finding(finding);
        println!("{formatted_finding}");
    }
}

fn format_finding(finding: &Finding) -> String {
    let suppression = if finding.suppressed {
        " [suppressed]"
    } else {
        ""
    };
    let severity = &finding.severity;
    let rule = &finding.key.rule;
    let path = &finding.key.path;
    let message = &finding.message;
    format!("- [{severity}] {rule} {path} :: {message}{suppression}")
}

fn format_ownership_diff_messages(
    new_findings: &[OwnershipFinding],
    stale_baseline: &[OwnershipFindingKey],
) -> Option<String> {
    let mut sections = Vec::new();

    if !new_findings.is_empty() {
        let body = new_findings
            .iter()
            .map(format_ownership_finding)
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("new ownership findings:\n{body}"));
    }

    if !stale_baseline.is_empty() {
        let body = stale_baseline
            .iter()
            .map(|key| format!("- {} {} {}", key.rule, key.path, key.symbol))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("stale ownership baseline entries:\n{body}"));
    }

    (!sections.is_empty()).then_some(sections.join("\n\n"))
}

fn collect_repo_rs_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_rs_files_recursive(root, root, &mut files)?;
    Ok(files)
}

fn collect_rs_files_recursive(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let skip = rel.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some("target" | "node_modules" | "test-fixtures")
                )
            }) || path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| {
                    // Hidden directories hold tool state (.git, .claude,
                    // .codex, ...), never audited workspace source.
                    name.starts_with('.') || matches!(name, "tests" | "examples" | "benches")
                })
                // A nested git checkout (agent/codex worktree parked inside
                // the repo) is another repository's source, not ours.
                || path.join(".git").exists();
            if !skip {
                collect_rs_files_recursive(root, &path, files)?;
            }
        } else if path.extension().and_then(OsStr::to_str) == Some("rs") {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
            if rel.contains("/src/generated/") {
                continue;
            }
            files.push(path);
        }
    }
    Ok(())
}

fn collect_dead_authority_wiring_findings(
    relative: &str,
    source: &str,
    parsed: &syn::File,
    policy: &AuditPolicy,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let is_authority_module = policy
        .authority_modules
        .iter()
        .any(|rule| relative.ends_with(rule.path_suffix));

    if is_authority_module {
        let mut visitor = DeadAuthorityItemVisitor::new(relative, source);
        visitor.visit_file(parsed);
        findings.extend(visitor.findings);
    }

    findings
}

struct DeadAuthorityItemVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    findings: Vec<Finding>,
}

impl<'a> DeadAuthorityItemVisitor<'a> {
    fn new(relative: &'a str, source: &'a str) -> Self {
        Self {
            relative,
            source,
            findings: Vec::new(),
        }
    }

    fn push_item_finding(&mut self, span: proc_macro2::Span, symbol: String) {
        let suppressed = has_rmat_allow(self.source, span, "NoDeadAuthorityWiring");
        self.findings.push(error_finding(
            "NoDeadAuthorityWiring",
            self.relative,
            &symbol,
            "authority item still carries #[allow(dead_code)]".to_string(),
            suppressed,
        ));
    }
}

impl<'ast> Visit<'ast> for DeadAuthorityItemVisitor<'_> {
    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if attrs_allow_dead_code(&node.attrs) {
            self.push_item_finding(node.span(), node.ident.to_string());
        }
        visit::visit_item_enum(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if attrs_allow_dead_code(&node.attrs) {
            self.push_item_finding(node.span(), node.ident.to_string());
        }
        visit::visit_item_struct(self, node);
    }

    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        if attrs_allow_dead_code(&node.attrs) {
            self.push_item_finding(node.span(), node.sig.ident.to_string());
        }
        visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        if attrs_allow_dead_code(&node.attrs) {
            self.push_item_finding(node.span(), node.sig.ident.to_string());
        }
        visit::visit_impl_item_fn(self, node);
    }
}

fn collect_route_realization_findings(root: &Path, policy: &AuditPolicy) -> Vec<Finding> {
    let mut findings = Vec::new();
    let required = policy.required_route_keys();

    // ---------------------------------------------------------------------
    // Rule-existence + path-existence check (pre-wave-b legacy).
    // ---------------------------------------------------------------------
    for ((producer, effect), rules) in &required {
        if rules.is_empty() {
            findings.push(error_finding(
                "CompositionRouteRealizationCoverage",
                "<policy>",
                &format!("{producer}::{effect}"),
                "routed effect has no realization rule".to_string(),
                false,
            ));
            continue;
        }

        let mut any_path_exists = false;
        for rule in rules {
            for allowed_path in &rule.allowed_paths {
                let full = root.join(allowed_path);
                if full.exists() {
                    any_path_exists = true;
                    let _ = (&rule.effect_variant, &rule.consumer_input);
                }
            }
        }
        if !any_path_exists {
            findings.push(error_finding(
                "CompositionRouteRealizationCoverage",
                "<policy>",
                &format!("{producer}::{effect}"),
                "all configured realization sites are missing".to_string(),
                false,
            ));
        }
    }

    // ---------------------------------------------------------------------
    // B-10 semantic upgrade: for each routed effect, walk the canonical
    // composition schema and verify there is a *typed* `Route` entry that
    // resolves (producer_instance, effect_variant) to the consumer machine
    // and input variant named in the policy rule. Rule-existence alone is
    // not sufficient — wave-b's `CompositionDispatcher` is driven by the
    // schema's `routes` table, not by the policy rules, so a rule without
    // a matching schema route means the dispatcher will refuse at runtime.
    // ---------------------------------------------------------------------
    use meerkat_machine_schema::{canonical_composition_schemas, canonical_machine_schemas};
    let machines = canonical_machine_schemas();
    let compositions = canonical_composition_schemas();
    for ((producer, effect), rules) in &required {
        let Some(producer_schema) = machines.iter().find(|m| m.machine.as_str() == producer) else {
            findings.push(error_finding(
                "CompositionRouteSemanticCoverage",
                "<schema>",
                &format!("{producer}::{effect}"),
                format!("producer machine `{producer}` is not in the canonical schema registry"),
                false,
            ));
            continue;
        };
        let effect_present = producer_schema
            .effect_dispositions
            .iter()
            .any(|d| d.effect_variant.as_str() == effect);
        if !effect_present {
            findings.push(error_finding(
                "CompositionRouteSemanticCoverage",
                "<schema>",
                &format!("{producer}::{effect}"),
                format!(
                    "policy declares routed effect `{producer}::{effect}` but the machine schema does not carry that EffectDispositionRule"
                ),
                false,
            ));
            continue;
        }
        for rule in rules {
            let mut route_found_somewhere = false;
            for composition in &compositions {
                let producer_instance = composition
                    .machines
                    .iter()
                    .find(|inst| inst.machine_name.as_str() == rule.producer_machine.as_str());
                let consumer_instance = composition
                    .machines
                    .iter()
                    .find(|inst| inst.machine_name.as_str() == rule.consumer_machine.as_str());
                let (Some(producer_instance), Some(consumer_instance)) =
                    (producer_instance, consumer_instance)
                else {
                    continue;
                };
                let route = composition.routes.iter().find(|r| {
                    r.from_machine == producer_instance.instance_id
                        && r.effect_variant.as_str() == rule.effect_variant.as_str()
                        && r.to.machine == consumer_instance.instance_id
                });
                if let Some(route) = route {
                    // The route's target input variant must match the policy's
                    // consumer_input. This is the teeth on the B-5 handoff:
                    // the policy rule and the typed schema must agree about
                    // which consumer input a given routed effect realizes.
                    if route.to.input_variant.as_str() == rule.consumer_input.as_str() {
                        route_found_somewhere = true;
                        break;
                    }
                    findings.push(error_finding(
                        "CompositionRouteSemanticCoverage",
                        "<schema>",
                        &format!("{}::{}", rule.producer_machine, rule.effect_variant),
                        format!(
                            "policy expects consumer_input=`{}` but composition `{}` typed Route targets input=`{}`",
                            rule.consumer_input,
                            composition.name,
                            route.to.input_variant.as_str()
                        ),
                        false,
                    ));
                }
            }
            if !route_found_somewhere {
                findings.push(error_finding(
                    "CompositionRouteSemanticCoverage",
                    "<schema>",
                    &format!("{}::{}", rule.producer_machine, rule.effect_variant),
                    format!(
                        "no typed Route in any canonical composition resolves (producer=`{}`, effect=`{}`) to consumer=`{}` with input=`{}` — CompositionDispatcher will refuse this producer→consumer pair",
                        rule.producer_machine,
                        rule.effect_variant,
                        rule.consumer_machine,
                        rule.consumer_input,
                    ),
                    false,
                ));
            }
        }
    }

    // ---------------------------------------------------------------------
    // B-10 semantic upgrade: byte-level scan under `meerkat-runtime/src/`
    // and `meerkat-mob/src/` (excluding the composition module itself and
    // tests) for re-introductions of the deleted wave-a helper names. This
    // complements the crate-local grep canary shipped in B-5 by running as
    // part of every CI `rmat-audit` invocation and covering the mob side
    // too. Re-appearance of any banned name means the consumer-input
    // application path has forked from `CompositionDispatcher::dispatch`
    // and `ConsumerSurface::apply_routed_input`.
    // ---------------------------------------------------------------------
    // `comms_trust_reconcile` was in this list while the identically-named
    // wave-a helper was the forbidden pre-composition shell shortcut.
    // Wave-c (`b0e881535`) re-ported the module as the legitimate
    // mechanical external-effect handler: `MeerkatMachine`'s DSL declares
    // `disposition CommsTrustReconcileRequested => external`
    // (meerkat-machine-schema `dsl/meerkat_machine.rs:732`), so the effect
    // intentionally lands on shell observers rather than transiting
    // `CompositionDispatcher`. The banned-name grep no longer applies —
    // this rule's invariant ("routed-effect dispatch must go through
    // CompositionDispatcher") only governs effects with a `routed`
    // disposition. `composition_dispatch` + `recompute_mob_peer_overlay`
    // remain banned because their dispositions *are* routed.
    const BANNED_LEGACY_HELPERS: &[&str] = &["composition_dispatch", "recompute_mob_peer_overlay"];
    let scan_roots: [&str; 2] = ["meerkat-runtime/src", "meerkat-mob/src"];
    for root_rel in scan_roots {
        let root_path = root.join(root_rel);
        if !root_path.exists() {
            continue;
        }
        let mut files: Vec<PathBuf> = Vec::new();
        if collect_rs_files_recursive(root, &root_path, &mut files).is_err() {
            continue;
        }
        for file in files {
            let rel = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .to_string_lossy()
                .to_string();
            let Ok(source) = fs::read_to_string(&file) else {
                continue;
            };
            // AST-precise: flag a banned helper only when it is *called*
            // (free-fn `composition_dispatch(..)` or method `.composition_dispatch(..)`).
            // String literals, comments, and `use`/path imports that merely
            // mention the name are not the forbidden dispatch fork, so the old
            // `/composition/` skip hack (which masked the positive-identity
            // references in the replacement module) is no longer needed.
            let Ok(parsed) = syn::parse_file(&source) else {
                continue;
            };
            let mut visitor = BannedHelperCallVisitor::new(&rel, &source, BANNED_LEGACY_HELPERS);
            visitor.visit_file(&parsed);
            findings.extend(visitor.findings);
        }
    }

    findings
}

/// Detect *calls* to a banned legacy helper (free function or method). Unlike
/// the prior `source.contains(name)` byte scan, this ignores string-literal
/// and comment occurrences of the name and only fires on a real call
/// expression, so a rename that preserves the banned dispatch relationship
/// still trips while a doc/import mention of the name does not.
struct BannedHelperCallVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    banned: &'a [&'a str],
    findings: Vec<Finding>,
}

impl<'a> BannedHelperCallVisitor<'a> {
    fn new(relative: &'a str, source: &'a str, banned: &'a [&'a str]) -> Self {
        Self {
            relative,
            source,
            banned,
            findings: Vec::new(),
        }
    }

    fn push(&mut self, name: &str, span: proc_macro2::Span) {
        let suppressed = has_rmat_allow(self.source, span, "CompositionDispatchIsThePath");
        self.findings.push(error_finding(
            "CompositionDispatchIsThePath",
            self.relative,
            name,
            format!(
                "forbidden legacy helper `{name}` is called here; routed-effect dispatch must go through meerkat_runtime::composition::CompositionDispatcher + ConsumerSurface::apply_routed_input"
            ),
            suppressed,
        ));
    }
}

impl<'ast> Visit<'ast> for BannedHelperCallVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && let Some(segment) = path.path.segments.last()
        {
            let name = segment.ident.to_string();
            if self.banned.contains(&name.as_str()) {
                self.push(&name, node.span());
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let name = node.method.to_string();
        if self.banned.contains(&name.as_str()) {
            self.push(&name, node.span());
        }
        visit::visit_expr_method_call(self, node);
    }
}

struct GuardedApplyVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    findings: Vec<Finding>,
}

impl<'a> GuardedApplyVisitor<'a> {
    fn new(relative: &'a str, source: &'a str) -> Self {
        Self {
            relative,
            source,
            findings: Vec::new(),
        }
    }
}

/// Field/method names that name a canonical authority state read. A guard
/// condition that touches one of these and then conditionally calls `apply()`
/// is a shell pre-check the authority must own instead.
const STATE_READ_NAMES: &[&str] = &[
    "current_state",
    "phase",
    "snapshot",
    "state",
    "terminal_outcome",
    "is_terminal",
];

/// True when the expression subtree contains a method call or field access to
/// a canonical authority state name — matched on AST node kinds, not on a
/// stringified token soup, so a `// phase` comment or a `"phase"` string
/// literal in the condition does not count.
struct StateReadDetector {
    found: bool,
}

impl<'ast> Visit<'ast> for StateReadDetector {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if STATE_READ_NAMES.contains(&node.method.to_string().as_str()) {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(ident) = &node.member
            && STATE_READ_NAMES.contains(&ident.to_string().as_str())
        {
            self.found = true;
        }
        visit::visit_expr_field(self, node);
    }
}

/// True when the block subtree contains an `.apply(...)` method call.
struct ApplyCallDetector {
    found: bool,
}

impl<'ast> Visit<'ast> for ApplyCallDetector {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method == "apply" {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }
}

impl<'ast> Visit<'ast> for GuardedApplyVisitor<'_> {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        let mut state_read = StateReadDetector { found: false };
        state_read.visit_expr(&node.cond);

        let mut apply_in_body = ApplyCallDetector { found: false };
        apply_in_body.visit_block(&node.then_branch);

        let else_empty = node.else_branch.is_none();
        if state_read.found && apply_in_body.found && else_empty {
            let suppress = has_rmat_allow(self.source, node.span(), "NoGuardedApply");
            self.findings.push(error_finding(
                "NoGuardedApply",
                self.relative,
                "if",
                "authority apply() is conditionally gated by a state read; submit unconditionally and let the authority reject".to_string(),
                suppress,
            ));
        }
        visit::visit_expr_if(self, node);
    }
}

struct ParallelTransitionVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    policy: &'a AuditPolicy,
    findings: Vec<Finding>,
}

impl<'a> ParallelTransitionVisitor<'a> {
    fn new(relative: &'a str, source: &'a str, policy: &'a AuditPolicy) -> Self {
        Self {
            relative,
            source,
            policy,
            findings: Vec::new(),
        }
    }
}

impl<'ast> Visit<'ast> for ParallelTransitionVisitor<'_> {
    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        let Some(type_name) = type_ident(&node.self_ty) else {
            return visit::visit_item_impl(self, node);
        };
        let Some(rule) = self
            .policy
            .projection_types
            .iter()
            .find(|rule| rule.type_name == type_name)
        else {
            return visit::visit_item_impl(self, node);
        };

        for item in &node.items {
            let ImplItem::Fn(method) = item else {
                continue;
            };
            if rule
                .forbidden_methods
                .iter()
                .any(|name| method.sig.ident == *name)
            {
                let suppressed =
                    has_rmat_allow(self.source, method.span(), "NoParallelTransitionTable");
                self.findings.push(error_finding(
                    "NoParallelTransitionTable",
                    self.relative,
                    &format!("{type_name}::{}", method.sig.ident),
                    format!(
                        "projection/parallel type `{type_name}` still defines forbidden transition-like method `{}`",
                        method.sig.ident
                    ),
                    suppressed,
                ));
            }
        }

        visit::visit_item_impl(self, node);
    }
}

struct ProtectedFieldWriteVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    policy: &'a AuditPolicy,
    findings: Vec<Finding>,
    fn_stack: Vec<String>,
}

impl<'a> ProtectedFieldWriteVisitor<'a> {
    fn new(relative: &'a str, source: &'a str, policy: &'a AuditPolicy) -> Self {
        Self {
            relative,
            source,
            policy,
            findings: Vec::new(),
            fn_stack: Vec::new(),
        }
    }

    fn current_fn(&self) -> &str {
        self.fn_stack.last().map_or("<module>", String::as_str)
    }
}

impl<'ast> Visit<'ast> for ProtectedFieldWriteVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.fn_stack.pop();
    }

    fn visit_expr_assign(&mut self, node: &'ast ExprAssign) {
        if self.relative.contains("authority") {
            return visit::visit_expr_assign(self, node);
        }
        if let Expr::Field(field) = &*node.left
            && let Member::Named(name) = &field.member
            && let Some(rule) = self
                .policy
                .protected_fields
                .iter()
                .find(|rule| name == rule.field_name)
        {
            let writer = self.current_fn();
            if !rule.allowed_writers.contains(&writer) {
                let suppressed =
                    has_rmat_allow(self.source, node.span(), "NoShellSemanticFlagWrites");
                self.findings.push(error_finding(
                    "NoShellSemanticFlagWrites",
                    self.relative,
                    &format!("{writer}::{name}"),
                    format!(
                        "protected shell semantic field `{}` written outside allowed writer(s): {}",
                        name,
                        rule.allowed_writers.join(", ")
                    ),
                    suppressed,
                ));
            }
        }
        visit::visit_expr_assign(self, node);
    }
}

struct SuspicionVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    findings: Vec<Finding>,
}

impl<'a> SuspicionVisitor<'a> {
    fn new(relative: &'a str, source: &'a str) -> Self {
        Self {
            relative,
            source,
            findings: Vec::new(),
        }
    }
}

impl<'ast> Visit<'ast> for SuspicionVisitor<'_> {
    // The identifier-substring heuristics below operate on AST-resolved type
    // and field names (never raw source text) and emit warn-severity
    // suspicion reports only — they are review prompts, not governance
    // decisions, so fuzzy name matching is acceptable here.
    fn visit_item(&mut self, node: &'ast Item) {
        match node {
            Item::Enum(item_enum) if item_enum.variants.len() >= 3 => {
                let name = item_enum.ident.to_string();
                let lifecycleish = ["State", "Status", "Reservation"]
                    .iter()
                    .any(|needle| name.contains(needle));
                if lifecycleish && !name.ends_with("Authority") {
                    let suppressed =
                        has_rmat_allow(self.source, item_enum.span(), "LifecycleSuspicionReport");
                    self.findings.push(warn_finding(
                        "LifecycleSuspicionReport",
                        self.relative,
                        &name,
                        "local enum looks lifecycle-shaped; verify it is not a missing machine"
                            .to_string(),
                        suppressed,
                    ));
                }
            }
            Item::Struct(item_struct) => {
                for field in &item_struct.fields {
                    if let Some(ident) = &field.ident
                        && ["active", "attached", "pending", "running", "interrupted"]
                            .iter()
                            .any(|needle| ident.to_string().contains(needle))
                    {
                        let suppressed = has_rmat_allow(
                            self.source,
                            item_struct.span(),
                            "LifecycleSuspicionReport",
                        );
                        self.findings.push(warn_finding(
                            "LifecycleSuspicionReport",
                            self.relative,
                            &format!("{}::{}", item_struct.ident, ident),
                            "field name looks lifecycle-significant; verify this is legitimate shell state".to_string(),
                            suppressed,
                        ));
                    }
                }
            }
            _ => {}
        }
        visit::visit_item(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if method == "phase" || method == "current_state" || method == "snapshot" {
            let suppressed = has_rmat_allow(self.source, node.span(), "LifecycleSuspicionReport");
            self.findings.push(warn_finding(
                "LifecycleSuspicionReport",
                self.relative,
                &method,
                "raw semantic state read in shell code; verify this is not driving business logic outside authority".to_string(),
                suppressed,
            ));
        }
        visit::visit_expr_method_call(self, node);
    }
}

struct ForbiddenShellReadsVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    rules: Vec<&'a ForbiddenShellReadRule>,
    findings: Vec<Finding>,
}

impl<'a> ForbiddenShellReadsVisitor<'a> {
    fn new(relative: &'a str, source: &'a str, rules: Vec<&'a ForbiddenShellReadRule>) -> Self {
        Self {
            relative,
            source,
            rules,
            findings: Vec::new(),
        }
    }

    fn push_finding(
        &mut self,
        rule: &ForbiddenShellReadRule,
        span: proc_macro2::Span,
        symbol: String,
        detail: String,
    ) {
        let suppressed =
            forbidden_shell_authority_read_suppressed(self.relative, self.source, span);
        let hint = rule.hint;
        let relative = self.relative;
        self.findings.push(error_finding(
            "ForbiddenShellAuthorityReads",
            relative,
            &symbol,
            format!("{detail}. Fix: {hint}."),
            suppressed,
        ));
    }

    fn check_struct_fields(&mut self, ident: &str, fields: &syn::Fields) {
        for field in fields {
            let Some(field_ident) = &field.ident else {
                continue;
            };
            let field_name = field_ident.to_string();
            for rule in self.rules.clone() {
                if let ForbiddenShellReadKind::FieldDeclared { field_name: target } = &rule.kind
                    && field_name == *target
                {
                    self.push_finding(
                        rule,
                        field.span(),
                        format!("{ident}::{field_name}"),
                        format!(
                            "shell struct `{ident}` declares shadow semantic field `{field_name}`"
                        ),
                    );
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for ForbiddenShellReadsVisitor<'_> {
    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        let ident = node.ident.to_string();
        self.check_struct_fields(&ident, &node.fields);
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        let ident = node.ident.to_string();
        for variant in &node.variants {
            self.check_struct_fields(&format!("{ident}::{}", variant.ident), &variant.fields);
        }
        visit::visit_item_enum(self, node);
    }

    fn visit_item_union(&mut self, node: &'ast ItemUnion) {
        let ident = node.ident.to_string();
        for field in &node.fields.named {
            let Some(field_ident) = &field.ident else {
                continue;
            };
            let field_name = field_ident.to_string();
            for rule in self.rules.clone() {
                if let ForbiddenShellReadKind::FieldDeclared { field_name: target } = &rule.kind
                    && field_name == *target
                {
                    self.push_finding(
                        rule,
                        field.span(),
                        format!("{ident}::{field_name}"),
                        format!(
                            "shell union `{ident}` declares shadow semantic field `{field_name}`"
                        ),
                    );
                }
            }
        }
        visit::visit_item_union(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method_name = node.method.to_string();
        let receiver_tail = tail_path_ident(&node.receiver);
        for rule in self.rules.clone() {
            if let ForbiddenShellReadKind::MethodCall {
                method,
                allowed_receivers,
            } = &rule.kind
                && method_name == *method
            {
                let allowed = receiver_tail
                    .as_deref()
                    .is_some_and(|ident| allowed_receivers.contains(&ident));
                if !allowed {
                    let symbol = match receiver_tail.as_deref() {
                        Some(ident) => format!("{ident}.{method_name}()"),
                        None => format!("<expr>.{method_name}()"),
                    };
                    self.push_finding(
                        rule,
                        node.span(),
                        symbol.clone(),
                        format!("shell call `{symbol}` reads canonical authority state"),
                    );
                }
            }
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        let Member::Named(field_ident) = &node.member else {
            return visit::visit_expr_field(self, node);
        };
        let field_name = field_ident.to_string();
        let base_tail = tail_path_ident(&node.base);
        for rule in self.rules.clone() {
            if let ForbiddenShellReadKind::FieldAccess {
                base_ident,
                field_name: target_field,
            } = &rule.kind
                && field_name == *target_field
                && base_tail.as_deref() == Some(*base_ident)
            {
                let symbol = format!("{base_ident}.{field_name}");
                self.push_finding(
                    rule,
                    node.span(),
                    symbol.clone(),
                    format!("shell reads authority classification field `{symbol}`"),
                );
            }
        }
        visit::visit_expr_field(self, node);
    }
}

/// Return the tail identifier of an expression, where "tail" is the rightmost
/// name that disambiguates the binding being read. For field expressions
/// (`self.lifecycle_authority`) that's the field name; for paths (`policy`,
/// `self`, `crate::foo::bar`) it's the final path segment.
fn tail_path_ident(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Path(path) => path.path.segments.last().map(|seg| seg.ident.to_string()),
        Expr::Field(field) => match &field.member {
            Member::Named(ident) => Some(ident.to_string()),
            Member::Unnamed(_) => None,
        },
        Expr::Reference(inner) => tail_path_ident(&inner.expr),
        Expr::Paren(inner) => tail_path_ident(&inner.expr),
        Expr::Group(inner) => tail_path_ident(&inner.expr),
        _ => None,
    }
}

fn type_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(type_path) => type_path
            .path
            .segments
            .last()
            .map(|seg| seg.ident.to_string()),
        _ => None,
    }
}

fn attrs_allow_dead_code(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("allow")
            && attr
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
                )
                .map(|paths| paths.iter().any(|path| path.is_ident("dead_code")))
                .unwrap_or(false)
    })
}

/// Scan for the `// RMAT-ALLOW(<rule>)` suppression marker on or above the
/// span's line. Necessarily textual: the marker is a comment convention and
/// comments are not AST nodes — this asserts an authored suppression marker,
/// not source structure.
fn has_rmat_allow(source: &str, span: proc_macro2::Span, rule: &str) -> bool {
    let needle = format!("RMAT-ALLOW({rule})");
    let start_line = span.start().line;
    if start_line == 0 {
        return source.contains(&needle);
    }

    let lines: Vec<&str> = source.lines().collect();
    let index = start_line
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));

    if let Some(line) = lines.get(index)
        && line.contains(&needle)
    {
        return true;
    }

    let mut current = index;
    while current > 0 {
        current -= 1;
        let line = lines[current].trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("//") {
            if line.contains(&needle) {
                return true;
            }
            continue;
        }
        break;
    }

    false
}

/// Public wrapper over [`has_rmat_allow`] so sibling audit modules (e.g. the
/// typed-carrier gate) can honor the same `// RMAT-ALLOW(<rule>)` suppression
/// convention without duplicating the span/line scan.
pub(crate) fn rmat_allow_at(source: &str, span: proc_macro2::Span, rule: &str) -> bool {
    has_rmat_allow(source, span, rule)
}

fn forbidden_shell_authority_read_suppressed(
    relative: &str,
    source: &str,
    span: proc_macro2::Span,
) -> bool {
    is_test_fixture_path(relative) && has_rmat_allow(source, span, "ForbiddenShellAuthorityReads")
}

fn is_test_fixture_path(relative: &str) -> bool {
    relative.starts_with("rmat-test-fixtures/")
        || relative.starts_with("test-fixtures/")
        || relative.starts_with("tests/fixtures/")
        || relative.contains("/rmat-test-fixtures/")
        || relative.contains("/test-fixtures/")
        || relative.contains("/tests/fixtures/")
}

/// Structural seam rule: every effect with `handoff_protocol: Some(name)` must have
/// a corresponding `EffectHandoffProtocol` declared in compositions that include
/// the producing machine. This is validated at the composition schema level; the audit
/// cross-checks that the annotation on the machine schema side is consistent.
fn collect_handoff_protocol_coverage_findings(policy: &AuditPolicy) -> Vec<Finding> {
    use meerkat_machine_schema::canonical_composition_schemas;
    let mut findings = Vec::new();
    let compositions = canonical_composition_schemas();

    for rule in &policy.handoff_protocol_coverage {
        // For every composition that includes the annotated machine,
        // verify the protocol is declared.
        let mut found_in_any_composition = false;
        for composition in &compositions {
            let machine_present = composition
                .machines
                .iter()
                .any(|m| m.machine_name.as_str() == rule.machine);
            if !machine_present {
                continue;
            }
            found_in_any_composition = true;
            let protocol_present = composition.handoff_protocols.iter().any(|p| {
                p.name.as_str() == rule.protocol_name
                    && p.effect_variant.as_str() == rule.effect_variant
            });
            if !protocol_present {
                findings.push(error_finding(
                    "HandoffProtocolCoverage",
                    "<schema>",
                    &format!("{}::{}", rule.machine, rule.effect_variant),
                    format!(
                        "effect `{}::{}` declares handoff_protocol `{}` but composition `{}` has no matching EffectHandoffProtocol",
                        rule.machine, rule.effect_variant, rule.protocol_name, composition.name
                    ),
                    false,
                ));
            }
        }
        if !found_in_any_composition {
            // Machine with handoff annotation not present in any composition is also a problem.
            findings.push(error_finding(
                "HandoffProtocolCoverage",
                "<schema>",
                &format!("{}::{}", rule.machine, rule.effect_variant),
                format!(
                    "effect `{}::{}` declares handoff_protocol `{}` but machine is not present in any composition",
                    rule.machine, rule.effect_variant, rule.protocol_name
                ),
                false,
            ));
        }
    }
    findings
}

/// Structural seam rule: every declared `EffectHandoffProtocol` must have a generated
/// protocol helper file at the expected path.
fn collect_protocol_realization_site_findings(root: &Path, policy: &AuditPolicy) -> Vec<Finding> {
    let mut findings = Vec::new();
    for rule in &policy.protocol_realization_sites {
        let found = rule.candidate_paths.iter().any(|p| root.join(p).exists());
        if !found {
            let paths_display = rule.candidate_paths.join(", ");
            findings.push(error_finding(
                "ProtocolRealizationSiteCoverage",
                rule.candidate_paths.first().map(String::as_str).unwrap_or(""),
                &rule.protocol_name,
                format!(
                    "protocol `{}` has no generated helper file at any of [{}]; run `xtask protocol-codegen`",
                    rule.protocol_name, paths_display
                ),
                false,
            ));
        }
    }
    findings
}

/// Structural seam rule: feedback inputs declared in a protocol must only be submitted
/// through generated helper code (files containing `// @generated` marker).
/// This checks that no non-generated file constructs the feedback input variant
/// in a way that bypasses the protocol helper.
fn collect_protocol_feedback_constraint_findings(
    root: &Path,
    policy: &AuditPolicy,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    if policy.protocol_feedback_constraints.is_empty() {
        return findings;
    }

    let files = match collect_repo_rs_files(root) {
        Ok(f) => f,
        Err(_) => return findings,
    };

    for file in &files {
        let relative = file
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        // Generated files are allowed to reference the feedback variant.
        if relative.contains("/src/generated/") {
            continue;
        }
        // Authority files are allowed — they are the machine truth.
        if relative.contains("authority") {
            continue;
        }
        // Schema/catalog files are allowed — they declare the variant.
        if relative.contains("meerkat-machine-schema/") {
            continue;
        }
        // xtask files are allowed — they generate/audit.
        if relative.starts_with("xtask/") {
            continue;
        }
        // These files are the typed authority/owner adapter implementations
        // for their respective feedback surfaces. They may destructure or
        // construct the local authority input while generated helpers remain
        // the public handoff entrypoint for non-owner shell code.
        if is_feedback_authority_implementation_path(&relative) {
            continue;
        }
        // Unit tests exercise authority inputs directly; the production
        // bypass rule is scoped to non-test files.
        if is_test_source_path(&relative) {
            continue;
        }

        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Every repo .rs file is parsed (and fails the run) earlier in
        // `collect_findings`; an unparseable file here cannot silently pass.
        let Ok(mut parsed) = syn::parse_file(&source) else {
            continue;
        };
        // Inline `#[cfg(test)]` items are test code: the production bypass
        // rule is scoped to non-test code, mirroring `is_test_source_path`.
        strip_cfg_test_items(&mut parsed.items);

        for rule in &policy.protocol_feedback_constraints {
            let variant_pattern = &rule.feedback_input_variant;
            // AST detection of feedback-variant *construction* (struct
            // expression, call, or value path ending in `::<Variant>`).
            // Replaces the prior line/text scan: match arms (patterns) are
            // structurally not expressions, comments and string literals are
            // not AST nodes, and word boundaries are ident equality — so a
            // rename that keeps the relationship still fails and a substring
            // in a longer identifier no longer false-positives. Companion
            // `*InputVariant` metadata enums are excluded by the qualifying
            // path segment, as before.
            let mut visitor = FeedbackVariantConstructionVisitor {
                variant: variant_pattern,
                found: false,
            };
            visitor.visit_file(&parsed);
            if visitor.found {
                findings.push(error_finding(
                    "ProtocolFeedbackThroughGeneratedHelpers",
                    &relative,
                    &format!("{}::{}", rule.protocol_name, variant_pattern),
                    format!(
                        "feedback input `{}` for protocol `{}` is constructed in non-generated code; \
                         use the generated protocol helper instead",
                        variant_pattern, rule.protocol_name
                    ),
                    false,
                ));
            }
        }
    }
    findings
}

/// Structurally drop every `#[cfg(test)]` item (top-level, nested in `mod`,
/// or impl member) so production-only audits never see test fixtures.
fn strip_cfg_test_items(items: &mut Vec<syn::Item>) {
    fn attr_is_cfg_test(attr: &syn::Attribute) -> bool {
        if !attr.path().is_ident("cfg") {
            return false;
        }
        let mut is_test = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                is_test = true;
            }
            Ok(())
        });
        is_test
    }
    fn item_is_cfg_test(item: &syn::Item) -> bool {
        let attrs: &[syn::Attribute] = match item {
            syn::Item::Const(item) => &item.attrs,
            syn::Item::Enum(item) => &item.attrs,
            syn::Item::Fn(item) => &item.attrs,
            syn::Item::Impl(item) => &item.attrs,
            syn::Item::Mod(item) => &item.attrs,
            syn::Item::Static(item) => &item.attrs,
            syn::Item::Struct(item) => &item.attrs,
            syn::Item::Trait(item) => &item.attrs,
            syn::Item::Type(item) => &item.attrs,
            syn::Item::Use(item) => &item.attrs,
            syn::Item::Macro(item) => &item.attrs,
            _ => return false,
        };
        attrs.iter().any(attr_is_cfg_test)
    }
    items.retain(|item| !item_is_cfg_test(item));
    for item in items.iter_mut() {
        match item {
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = module.content.as_mut() {
                    strip_cfg_test_items(inner);
                }
            }
            syn::Item::Impl(item_impl) => {
                item_impl.items.retain(|impl_item| {
                    let attrs: &[syn::Attribute] = match impl_item {
                        syn::ImplItem::Const(item) => &item.attrs,
                        syn::ImplItem::Fn(item) => &item.attrs,
                        syn::ImplItem::Type(item) => &item.attrs,
                        _ => return true,
                    };
                    !attrs.iter().any(attr_is_cfg_test)
                });
            }
            _ => {}
        }
    }
}

/// AST visitor for value-position construction of a protocol feedback input
/// variant: `…::<Variant> { .. }`, `…::<Variant>(..)`, or a bare
/// `…::<Variant>` value path. Pattern positions (match arms, destructuring)
/// are not expressions and are therefore not flagged.
struct FeedbackVariantConstructionVisitor<'n> {
    variant: &'n str,
    found: bool,
}

impl FeedbackVariantConstructionVisitor<'_> {
    fn path_constructs_variant(&self, path: &syn::Path) -> bool {
        let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
        let Some(last) = segments.last() else {
            return false;
        };
        if last != self.variant || segments.len() < 2 {
            return false;
        }
        // Companion mirror enums are metadata facts, not feedback
        // submissions: `*InputVariant` (hand-declared companions) and
        // `*CatalogInput` (generated parameterless variant-name mirrors used
        // for classification records).
        let qualifier = &segments[segments.len() - 2];
        !qualifier.ends_with("InputVariant") && !qualifier.ends_with("CatalogInput")
    }
}

impl<'a> Visit<'a> for FeedbackVariantConstructionVisitor<'_> {
    fn visit_expr_struct(&mut self, node: &'a syn::ExprStruct) {
        if self.path_constructs_variant(&node.path) {
            self.found = true;
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_expr_path(&mut self, node: &'a syn::ExprPath) {
        if self.path_constructs_variant(&node.path) {
            self.found = true;
        }
        syn::visit::visit_expr_path(self, node);
    }
}

fn is_feedback_authority_implementation_path(relative: &str) -> bool {
    matches!(
        relative,
        "meerkat-core/src/agent/state.rs"
            | "meerkat-runtime/src/handles/turn_state.rs"
            | "meerkat-mcp/src/router.rs"
            | "meerkat-mcp/src/external_tool_surface_authority.rs"
    ) || relative.contains("/authority")
        || relative.contains("/handles/")
}

fn is_test_source_path(relative: &str) -> bool {
    relative.contains("/tests/")
        || relative.ends_with("_tests.rs")
        || relative.ends_with("/tests.rs")
        || relative.contains("/test_")
}

/// Structural seam rule: terminal outcome classification for protocol-producing machines
/// must use generated classification helpers, not handwritten match arms.
/// This checks that a generated terminal classifier exists when a protocol requires one.
fn collect_terminal_mapping_constraint_findings(root: &Path, policy: &AuditPolicy) -> Vec<Finding> {
    let mut findings = Vec::new();
    for rule in &policy.terminal_mapping_constraints {
        let path = rule.helper_path;
        let source = match fs::read_to_string(root.join(path)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        // Marker check stays textual by necessity: `// @generated` is the
        // emitted generated-file marker contract and comments are not AST
        // nodes. This asserts emitted text, not source structure.
        if !source.contains("// @generated") {
            findings.push(error_finding(
                "TerminalMappingThroughGeneratedHelpers",
                path,
                &rule.protocol_name,
                format!(
                    "terminal mapping file for producer `{}` is missing `// @generated` marker; \
                     the file must be machine-generated, not handwritten",
                    rule.producer_machine
                ),
                false,
            ));
        }
        // AST-precise: the generated terminal classifier must exist as a real
        // `fn classify_terminal` item, not as a token mentioned in a comment
        // or string literal.
        let has_classifier = syn::parse_file(&source)
            .map(|parsed| named_fn_exists(&parsed, "classify_terminal"))
            .unwrap_or(false);
        if !has_classifier {
            findings.push(error_finding(
                "TerminalMappingThroughGeneratedHelpers",
                path,
                &rule.protocol_name,
                format!(
                    "generated terminal mapping for producer `{}` declares no `fn classify_terminal`; \
                     the protocol/codegen surface must generate terminal classification for `{}`",
                    rule.producer_machine, rule.protocol_name
                ),
                false,
            ));
        }
    }
    findings
}

/// Structural runtime bridge rule: classified comms ingress must project into
/// runtime input families without re-deriving class from raw peer names, and
/// must preserve rendered peer body plus multimodal blocks.
fn collect_runtime_comms_bridge_projection_findings(root: &Path) -> Vec<Finding> {
    let bridge_path = "meerkat-runtime/src/comms_bridge.rs";
    let drain_path = "meerkat-runtime/src/comms_drain.rs";

    let bridge_source = match fs::read_to_string(root.join(bridge_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };
    let drain_source = match fs::read_to_string(root.join(drain_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };

    runtime_comms_bridge_projection_findings_for_sources(
        bridge_path,
        &bridge_source,
        drain_path,
        &drain_source,
    )
}

/// Structural runtime external-event rule: all external-event producers must
/// preserve canonical text-vs-JSON mode, plain-event multimodal blocks, and
/// runtime block rendering for ExternalEvent inputs.
fn collect_runtime_external_event_projection_findings(root: &Path) -> Vec<Finding> {
    let stdin_path = "meerkat-cli/src/stdin_events.rs";
    let bridge_path = "meerkat-runtime/src/comms_bridge.rs";
    let input_path = "meerkat-runtime/src/input.rs";

    let stdin_source = match fs::read_to_string(root.join(stdin_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };
    let bridge_source = match fs::read_to_string(root.join(bridge_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };
    let input_source = match fs::read_to_string(root.join(input_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };

    runtime_external_event_projection_findings_for_sources(
        stdin_path,
        &stdin_source,
        bridge_path,
        &bridge_source,
        input_path,
        &input_source,
    )
}

fn runtime_external_event_projection_findings_for_sources(
    stdin_path: &str,
    stdin_source: &str,
    bridge_path: &str,
    bridge_source: &str,
    input_path: &str,
    input_source: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    let stdin_parsed = parse_audited_source(
        "RuntimeExternalEventProjectionAlignment",
        stdin_path,
        stdin_source,
        &mut findings,
    );
    let bridge_parsed = parse_audited_source(
        "RuntimeExternalEventProjectionAlignment",
        bridge_path,
        bridge_source,
        &mut findings,
    );
    let input_parsed = parse_audited_source(
        "RuntimeExternalEventProjectionAlignment",
        input_path,
        input_source,
        &mut findings,
    );

    // stdin mode discipline, AST-resolved on the `StdinLineFormat` match arms
    // inside `make_stdin_external_event_input`: the Text arm must not contain
    // a `from_str` reparse call, and the Json arm must be the (only) arm that
    // does.
    let stdin_arms = stdin_parsed.as_ref().and_then(|file| {
        find_named_fn(file, "make_stdin_external_event_input").map(|item| {
            let mut detector = StdinFormatArmDetector::default();
            detector.visit_item_fn(item);
            detector
        })
    });
    let text_arm_literal = stdin_arms
        .as_ref()
        .is_some_and(|arms| arms.text_arm_found && !arms.text_arm_reparses);
    if !text_arm_literal {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            stdin_path,
            "make_stdin_external_event_input",
            "stdin text mode must preserve literal lines instead of reparsing JSON-looking input"
                .to_string(),
            false,
        ));
    }
    let json_arm_reparses = stdin_arms
        .as_ref()
        .is_some_and(|arms| arms.json_arm_found && arms.json_arm_reparses);
    if !json_arm_reparses {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            stdin_path,
            "make_stdin_external_event_input",
            "stdin JSON mode must remain the only mode that reparses structured JSON bodies"
                .to_string(),
            false,
        ));
    }

    if !fn_contains_call(
        bridge_parsed.as_ref(),
        "classified_interaction_to_runtime_input",
        "external_event_blocks",
    ) {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "plain-event bridge must derive ExternalEvent blocks through external_event_blocks"
                .to_string(),
            false,
        ));
    }

    if !fn_contains_call(
        bridge_parsed.as_ref(),
        "classified_interaction_to_runtime_input",
        "external_event_payload",
    ) {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "plain-event bridge must derive ExternalEvent payload through external_event_payload"
                .to_string(),
            false,
        ));
    }

    if !fn_has_message_arm_binding_blocks(bridge_parsed.as_ref(), "external_event_blocks") {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "external_event_blocks",
            "plain-event bridge must preserve multimodal message blocks when building ExternalEvent inputs"
                .to_string(),
            false,
        ));
    }

    // The runtime loop's typed external-event notice: an
    // `SystemNoticeBlock::ExternalEvent { .. }` struct expression whose
    // `content` field is derived from the event's `blocks` (with the
    // empty-default fallback), resolved on the AST instead of a pinned line.
    let notice_preserves_blocks = input_parsed.as_ref().is_some_and(|file| {
        let mut detector = ExternalEventNoticeDetector::default();
        detector.visit_file(file);
        if !detector.found {
            // `syn` does not descend into macro token streams; the notice is
            // constructed inside `vec![...]`, so scan expression-shaped macro
            // bodies as well.
            for expr in collect_macro_body_exprs(file) {
                detector.visit_expr(&expr);
            }
        }
        detector.found
    });
    if !notice_preserves_blocks {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            input_path,
            "external_event_notice_renderable",
            "runtime loop must preserve ExternalEvent blocks inside typed system notices instead of flattening them to text"
                .to_string(),
            false,
        ));
    }

    findings
}

fn runtime_comms_bridge_projection_findings_for_sources(
    bridge_path: &str,
    bridge_source: &str,
    drain_path: &str,
    drain_source: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    let bridge_parsed = parse_audited_source(
        "RuntimeCommsBridgeProjectionAlignment",
        bridge_path,
        bridge_source,
        &mut findings,
    );
    let drain_parsed = parse_audited_source(
        "RuntimeCommsBridgeProjectionAlignment",
        drain_path,
        drain_source,
        &mut findings,
    );

    let classified_seam_present = bridge_parsed.as_ref().is_some_and(|file| {
        named_fn_exists(file, "classified_interaction_to_runtime_input")
            || file_contains_call(file, "classified_interaction_to_runtime_input")
    });
    if !classified_seam_present {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must route from classified ingress truth instead of a raw interaction-only adapter".to_string(),
            false,
        ));
    }

    // `classified.class() == PeerInputClass::PlainEvent`, resolved as an AST
    // equality between a `.class()` method call and the typed
    // `PeerInputClass::PlainEvent` path — not a pinned source line.
    let plain_event_comparison = bridge_parsed.as_ref().is_some_and(|file| {
        find_named_fn(file, "classified_interaction_to_runtime_input").is_some_and(|item| {
            let mut detector = ClassComparisonDetector::default();
            detector.visit_item_fn(item);
            detector.found
        })
    });
    if !plain_event_comparison {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must derive ExternalEvent routing from PeerInputClass::PlainEvent".to_string(),
            false,
        ));
    }

    let raw_peer_adapter = bridge_parsed.as_ref().is_some_and(|file| {
        named_fn_exists(file, "interaction_to_peer_input")
            || file_contains_call(file, "interaction_to_peer_input")
    });
    if raw_peer_adapter {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must not expose a raw interaction-only peer input adapter"
                .to_string(),
            false,
        ));
    }

    let sender_prefix_classification = bridge_parsed.as_ref().is_some_and(|file| {
        let mut detector = MethodCallWithStrLitDetector::new("starts_with", "event:");
        detector.visit_file(file);
        detector.found
    });
    if sender_prefix_classification {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must not classify external events from sender-name prefixes"
                .to_string(),
            false,
        ));
    }

    if !fn_contains_call(
        bridge_parsed.as_ref(),
        "peer_input_from_ingress_fact",
        "peer_rendered_body",
    ) {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_input_from_ingress_fact",
            "runtime comms bridge must project peer body from the rendered peer text helper"
                .to_string(),
            false,
        ));
    }

    // `peer_rendered_body` must read the classified ingress' rendered text:
    // an AST field access named `rendered_text` inside that fn.
    let reads_rendered_text = bridge_parsed.as_ref().is_some_and(|file| {
        find_named_fn(file, "peer_rendered_body").is_some_and(|item| {
            let mut detector = FieldReadDetector::new("rendered_text");
            detector.visit_item_fn(item);
            detector.found
        })
    });
    if !reads_rendered_text {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_rendered_body",
            "runtime comms bridge must preserve rendered peer text from classified ingress"
                .to_string(),
            false,
        ));
    }

    if !fn_contains_call(
        bridge_parsed.as_ref(),
        "peer_input_from_ingress_fact",
        "peer_blocks",
    ) {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_input_from_ingress_fact",
            "runtime comms bridge must project multimodal blocks through the peer block helper"
                .to_string(),
            false,
        ));
    }

    if !fn_has_message_arm_binding_blocks(bridge_parsed.as_ref(), "peer_blocks") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_blocks",
            "runtime comms bridge must preserve message blocks from drained peer interactions"
                .to_string(),
            false,
        ));
    }

    let drain_routes_classified = drain_parsed
        .as_ref()
        .is_some_and(|file| file_contains_call(file, "classified_interaction_to_runtime_input"));
    if !drain_routes_classified {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            drain_path,
            "run_comms_drain",
            "runtime comms drain must submit classified interactions through the classified bridge adapter".to_string(),
            false,
        ));
    }

    let drain_calls_raw_adapter = drain_parsed
        .as_ref()
        .is_some_and(|file| file_contains_call(file, "interaction_to_runtime_input"));
    if drain_calls_raw_adapter {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            drain_path,
            "run_comms_drain",
            "runtime comms drain still references the raw interaction-only bridge adapter"
                .to_string(),
            false,
        ));
    }

    findings
}

/// Parse an audited source file fail-closed: a file an alignment rule cannot
/// parse cannot be certified, so the parse failure itself is a finding.
fn parse_audited_source(
    rule: &str,
    path: &str,
    source: &str,
    findings: &mut Vec<Finding>,
) -> Option<syn::File> {
    match syn::parse_file(source) {
        Ok(parsed) => Some(parsed),
        Err(err) => {
            findings.push(error_finding(
                rule,
                path,
                "<parse>",
                format!("audited source failed to parse: {err}"),
                false,
            ));
            None
        }
    }
}

/// Walk every fn item in a file, including fns nested in inline modules.
fn visit_file_fns<'a>(items: &'a [syn::Item], out: &mut Vec<&'a ItemFn>) {
    for item in items {
        match item {
            Item::Fn(item_fn) => out.push(item_fn),
            Item::Mod(module) => {
                if let Some((_, inner)) = &module.content {
                    visit_file_fns(inner, out);
                }
            }
            _ => {}
        }
    }
}

fn find_named_fn<'a>(file: &'a syn::File, name: &str) -> Option<&'a ItemFn> {
    let mut fns = Vec::new();
    visit_file_fns(&file.items, &mut fns);
    fns.into_iter().find(|item| item.sig.ident == name)
}

fn named_fn_exists(file: &syn::File, name: &str) -> bool {
    find_named_fn(file, name).is_some()
}

/// True when the named fn exists in the file and contains a call (free fn or
/// method) to `callee`.
fn fn_contains_call(file: Option<&syn::File>, fn_name: &str, callee: &str) -> bool {
    file.is_some_and(|file| {
        find_named_fn(file, fn_name).is_some_and(|item| {
            let mut detector = CallDetector::new(callee);
            detector.visit_item_fn(item);
            detector.found
        })
    })
}

/// True when the file contains a call (free fn or method) to `callee`
/// anywhere. Ident equality on the AST — a longer identifier that merely
/// ends with the name does not match, and comments/strings are invisible.
fn file_contains_call(file: &syn::File, callee: &str) -> bool {
    let mut detector = CallDetector::new(callee);
    detector.visit_file(file);
    detector.found
}

/// True when the named fn exists and contains a match arm over
/// `InteractionContent::Message { blocks, .. }`, including as one case in an
/// or-pattern — the structural shape that preserves multimodal message blocks.
fn fn_has_message_arm_binding_blocks(file: Option<&syn::File>, fn_name: &str) -> bool {
    file.is_some_and(|file| {
        find_named_fn(file, fn_name).is_some_and(|item| {
            let mut detector = MessageArmBindsBlocksDetector::default();
            detector.visit_item_fn(item);
            detector.found
        })
    })
}

/// Collect expressions written inside macro bodies (e.g. `vec![...]`),
/// recursively. `syn`'s visitor does not descend into macro token streams, so
/// AST detectors that must see into expression-shaped macros scan these
/// parsed bodies as a second pass. Macros whose bodies are not
/// comma-separated expressions (`json!`, `format!` with a literal, …) simply
/// fail the parse and contribute nothing.
fn collect_macro_body_exprs(file: &syn::File) -> Vec<Expr> {
    struct MacroExprCollector {
        out: Vec<Expr>,
    }
    impl<'ast> Visit<'ast> for MacroExprCollector {
        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            if let Ok(exprs) = node.parse_body_with(
                syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
            ) {
                self.out.extend(exprs);
            }
            visit::visit_macro(self, node);
        }
    }
    let mut collector = MacroExprCollector { out: Vec::new() };
    collector.visit_file(file);
    let mut index = 0;
    while index < collector.out.len() {
        let expr = collector.out[index].clone();
        let mut inner = MacroExprCollector { out: Vec::new() };
        inner.visit_expr(&expr);
        collector.out.extend(inner.out);
        index += 1;
    }
    collector.out
}

fn path_ends_with(path: &syn::Path, suffix: &[&str]) -> bool {
    let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    segments.len() >= suffix.len()
        && segments[segments.len() - suffix.len()..]
            .iter()
            .zip(suffix)
            .all(|(seg, expected)| seg == expected)
}

/// AST detector for a call expression (free fn path or method) whose callee
/// ident equals `name`.
struct CallDetector<'n> {
    name: &'n str,
    found: bool,
}

impl<'n> CallDetector<'n> {
    fn new(name: &'n str) -> Self {
        Self { name, found: false }
    }
}

impl<'ast> Visit<'ast> for CallDetector<'_> {
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(path) = node.func.as_ref()
            && let Some(segment) = path.path.segments.last()
            && segment.ident == self.name
        {
            self.found = true;
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method == self.name {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }
}

/// AST detector for a method call whose first argument is a specific string
/// literal (e.g. `.starts_with("event:")`).
struct MethodCallWithStrLitDetector<'n> {
    method: &'n str,
    literal: &'n str,
    found: bool,
}

impl<'n> MethodCallWithStrLitDetector<'n> {
    fn new(method: &'n str, literal: &'n str) -> Self {
        Self {
            method,
            literal,
            found: false,
        }
    }
}

impl<'ast> Visit<'ast> for MethodCallWithStrLitDetector<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method == self.method && node.args.iter().any(|arg| {
            matches!(
                arg,
                Expr::Lit(lit) if matches!(&lit.lit, syn::Lit::Str(s) if s.value() == self.literal)
            )
        }) {
            self.found = true;
        }
        visit::visit_expr_method_call(self, node);
    }
}

/// AST detector for a named field access (`<expr>.rendered_text`).
struct FieldReadDetector<'n> {
    name: &'n str,
    found: bool,
}

impl<'n> FieldReadDetector<'n> {
    fn new(name: &'n str) -> Self {
        Self { name, found: false }
    }
}

impl<'ast> Visit<'ast> for FieldReadDetector<'_> {
    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        if let Member::Named(ident) = &node.member
            && ident == self.name
        {
            self.found = true;
        }
        visit::visit_expr_field(self, node);
    }
}

/// AST detector for the typed peer-class comparison: an equality whose one
/// side is a `.class()` method call and whose other side is the
/// `PeerInputClass::PlainEvent` path.
#[derive(Default)]
struct ClassComparisonDetector {
    found: bool,
}

impl ClassComparisonDetector {
    fn is_class_call(expr: &Expr) -> bool {
        matches!(expr, Expr::MethodCall(call) if call.method == "class")
    }

    fn is_plain_event_path(expr: &Expr) -> bool {
        matches!(
            expr,
            Expr::Path(path) if path_ends_with(&path.path, &["PeerInputClass", "PlainEvent"])
        )
    }
}

impl<'ast> Visit<'ast> for ClassComparisonDetector {
    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, syn::BinOp::Eq(_)) {
            let (left, right) = (node.left.as_ref(), node.right.as_ref());
            if (Self::is_class_call(left) && Self::is_plain_event_path(right))
                || (Self::is_class_call(right) && Self::is_plain_event_path(left))
            {
                self.found = true;
            }
        }
        visit::visit_expr_binary(self, node);
    }
}

/// AST detector for a match arm whose pattern contains
/// `InteractionContent::Message { blocks, .. }` (the block-preserving shape).
#[derive(Default)]
struct MessageArmBindsBlocksDetector {
    found: bool,
}

impl MessageArmBindsBlocksDetector {
    fn pattern_binds_message_blocks(pattern: &syn::Pat) -> bool {
        match pattern {
            syn::Pat::Struct(pattern) => {
                path_ends_with(&pattern.path, &["InteractionContent", "Message"])
                    && pattern.fields.iter().any(
                        |field| matches!(&field.member, Member::Named(ident) if ident == "blocks"),
                    )
            }
            syn::Pat::Or(pattern) => pattern.cases.iter().any(Self::pattern_binds_message_blocks),
            _ => false,
        }
    }
}

impl<'ast> Visit<'ast> for MessageArmBindsBlocksDetector {
    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        if Self::pattern_binds_message_blocks(&node.pat) {
            self.found = true;
        }
        visit::visit_arm(self, node);
    }
}

/// AST detector for the stdin `StdinLineFormat` mode match: records whether
/// the `Text` / `Json` arms exist and whether each arm's body contains a
/// `from_str` reparse call.
#[derive(Default)]
struct StdinFormatArmDetector {
    text_arm_found: bool,
    text_arm_reparses: bool,
    json_arm_found: bool,
    json_arm_reparses: bool,
}

impl StdinFormatArmDetector {
    fn arm_pattern_is(pat: &syn::Pat, variant: &str) -> bool {
        match pat {
            syn::Pat::Path(path) => path_ends_with(&path.path, &["StdinLineFormat", variant]),
            syn::Pat::TupleStruct(pat) => path_ends_with(&pat.path, &["StdinLineFormat", variant]),
            syn::Pat::Struct(pat) => path_ends_with(&pat.path, &["StdinLineFormat", variant]),
            _ => false,
        }
    }
}

impl<'ast> Visit<'ast> for StdinFormatArmDetector {
    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        let mut reparse = CallDetector::new("from_str");
        reparse.visit_expr(&node.body);
        if Self::arm_pattern_is(&node.pat, "Text") {
            self.text_arm_found = true;
            self.text_arm_reparses |= reparse.found;
        }
        if Self::arm_pattern_is(&node.pat, "Json") {
            self.json_arm_found = true;
            self.json_arm_reparses |= reparse.found;
        }
        visit::visit_arm(self, node);
    }
}

/// AST detector for the typed external-event notice: a
/// `SystemNoticeBlock::ExternalEvent { .. }` struct expression whose
/// `content` field expression derives from the event's `blocks` field with
/// the empty-default fallback (`unwrap_or_default`).
#[derive(Default)]
struct ExternalEventNoticeDetector {
    found: bool,
}

impl<'ast> Visit<'ast> for ExternalEventNoticeDetector {
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if path_ends_with(&node.path, &["SystemNoticeBlock", "ExternalEvent"]) {
            for field in &node.fields {
                let Member::Named(ident) = &field.member else {
                    continue;
                };
                if ident != "content" {
                    continue;
                }
                let mut blocks_read = FieldReadDetector::new("blocks");
                blocks_read.visit_expr(&field.expr);
                let mut default_fallback = CallDetector::new("unwrap_or_default");
                default_fallback.visit_expr(&field.expr);
                if blocks_read.found && default_fallback.found {
                    self.found = true;
                }
            }
        }
        visit::visit_expr_struct(self, node);
    }
}

pub(crate) fn error_finding(
    rule: &str,
    path: &str,
    symbol: &str,
    message: String,
    suppressed: bool,
) -> Finding {
    Finding {
        key: FindingKey {
            rule: rule.to_string(),
            path: path.to_string(),
            symbol: symbol.to_string(),
        },
        severity: "error".to_string(),
        message,
        suppressed,
    }
}

fn warn_finding(
    rule: &str,
    path: &str,
    symbol: &str,
    message: String,
    suppressed: bool,
) -> Finding {
    Finding {
        key: FindingKey {
            rule: rule.to_string(),
            path: path.to_string(),
            symbol: symbol.to_string(),
        },
        severity: "warn".to_string(),
        message,
        suppressed,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;

    fn feedback_variant_constructed(source: &str, variant: &str) -> bool {
        let mut parsed = syn::parse_file(source).expect("test source parses");
        strip_cfg_test_items(&mut parsed.items);
        let mut visitor = FeedbackVariantConstructionVisitor {
            variant,
            found: false,
        };
        visitor.visit_file(&parsed);
        visitor.found
    }

    #[test]
    fn schema_emitted_request_names_ignore_non_requests_and_tests() {
        let source = r#"
            "GoalCreateRequest": schema_for!(meerkat_workgraph::GoalCreateRequest),
            "GoalCreateResult": schema_for!(meerkat_workgraph::GoalCreateResult),
            assert!(schemas.contains_key("TestOnlyRequest"));
        "#;
        let names = schema_emitted_request_names_from_source(source);
        assert_eq!(names, BTreeSet::from(["GoalCreateRequest".to_string()]));
    }

    #[test]
    fn schema_request_use_detection_ignores_imports_strings_and_cfg_test() {
        let mut names = BTreeSet::new();
        names.insert("AttentionReassignRequest".to_string());
        names.insert("DeadRequest".to_string());
        let mut parsed = syn::parse_file(
            r#"
                use crate::{AttentionReassignRequest, DeadRequest};
                fn handle(request: AttentionReassignRequest) {
                    let _again = AttentionReassignRequest { binding_id: todo!() };
                    let _literal = "DeadRequest";
                }
                #[cfg(test)]
                fn test_only(_: DeadRequest) {}
            "#,
        )
        .expect("test source parses");
        strip_cfg_test_items(&mut parsed.items);
        let uses = schema_request_uses_in_file(&parsed, &names, &BTreeMap::new());
        assert_eq!(
            uses,
            BTreeSet::from(["AttentionReassignRequest".to_string()])
        );
    }

    #[test]
    fn schema_request_use_detection_follows_rest_and_wire_aliases() {
        let mut names = BTreeSet::new();
        names.insert("RestCreateSessionRequest".to_string());
        names.insert("WireGenerateImageRequest".to_string());

        let parsed = syn::parse_file(
            r"
                use meerkat_contracts::wire::RestCreateSessionRequest as CreateSessionRequest;
                pub type WireGenerateImageRequest = meerkat_core::GenerateImageRequest;

                fn rest(req: CreateSessionRequest) {}
                fn tool(req: GenerateImageRequest) {}
            ",
        )
        .expect("test source parses");
        let mut aliases = BTreeMap::new();
        collect_schema_request_type_aliases(&parsed, &names, &mut aliases);
        aliases.extend(schema_request_use_aliases_in_file(&parsed, &names));
        let uses = schema_request_uses_in_file(&parsed, &names, &aliases);
        assert_eq!(
            uses,
            BTreeSet::from([
                "RestCreateSessionRequest".to_string(),
                "WireGenerateImageRequest".to_string(),
            ])
        );
    }

    #[test]
    fn schema_request_consumption_path_filter_excludes_tests() {
        assert!(is_schema_request_consumption_excluded_path(
            "meerkat-workgraph/tests/attention_contracts.rs"
        ));
        assert!(is_schema_request_consumption_excluded_path(
            "meerkat-workgraph/src/tests.rs"
        ));
        assert!(is_schema_request_consumption_excluded_path(
            "meerkat-workgraph/src/store/tests/helpers.rs"
        ));
        assert!(is_schema_request_consumption_excluded_path(
            "meerkat-workgraph/src/attention_test.rs"
        ));
        assert!(!is_schema_request_consumption_excluded_path(
            "meerkat-workgraph/src/service.rs"
        ));
    }

    #[test]
    fn feedback_variant_detection_flags_value_position_construction() {
        // Struct construction, call construction, and bare value path all flag.
        assert!(feedback_variant_constructed(
            "fn f() { submit(Input::OpsBarrierSatisfied { op_id }); }",
            "OpsBarrierSatisfied",
        ));
        assert!(feedback_variant_constructed(
            "fn f() { submit(Input::OpsBarrierSatisfied(op_id)); }",
            "OpsBarrierSatisfied",
        ));
        assert!(feedback_variant_constructed(
            "fn f() -> Input { Input::OpsBarrierSatisfied }",
            "OpsBarrierSatisfied",
        ));
    }

    #[test]
    fn feedback_variant_detection_skips_patterns_comments_and_companions() {
        // Match-arm patterns are not constructions.
        assert!(!feedback_variant_constructed(
            "fn f(i: Input) { match i { Input::OpsBarrierSatisfied { .. } => {} _ => {} } }",
            "OpsBarrierSatisfied",
        ));
        // Comments and string literals are not AST constructions.
        assert!(!feedback_variant_constructed(
            "fn f() { // Input::OpsBarrierSatisfied\n let s = \"Input::OpsBarrierSatisfied\"; }",
            "OpsBarrierSatisfied",
        ));
        // Companion metadata mirrors are excluded by qualifying segment.
        assert!(!feedback_variant_constructed(
            "fn f() { record(MeerkatInputVariant::OpsBarrierSatisfied); }",
            "OpsBarrierSatisfied",
        ));
        assert!(!feedback_variant_constructed(
            "fn f() { record(MobMachineCatalogInput::SessionIngressDetachedForMobDestroy); }",
            "SessionIngressDetachedForMobDestroy",
        ));
        // A longer identifier that merely contains the variant is not a match.
        assert!(!feedback_variant_constructed(
            "fn f() { submit(Input::OpsBarrierSatisfiedExtra); }",
            "OpsBarrierSatisfied",
        ));
    }

    #[test]
    fn feedback_variant_detection_evasion_via_match_wrapper_still_flags() {
        // The retired line scanner skipped any line containing `=>`, so a
        // `match () { () => Input::Variant }` wrapper dodged it. The AST
        // detector sees the arm BODY as an expression and still flags it.
        assert!(feedback_variant_constructed(
            "const fn f() -> Input { match () { () => Input::OpsBarrierSatisfied } }",
            "OpsBarrierSatisfied",
        ));
    }

    #[test]
    fn feedback_variant_detection_skips_cfg_test_items() {
        assert!(!feedback_variant_constructed(
            "#[cfg(test)] mod tests { fn f() { submit(Input::OpsBarrierSatisfied { op_id }); } }",
            "OpsBarrierSatisfied",
        ));
    }

    #[test]
    fn baseline_roundtrip() -> Result<()> {
        let baseline = Baseline {
            finding: vec![FindingKey {
                rule: "NoParallelTransitionTable".to_string(),
                path: "foo.rs".to_string(),
                symbol: "LoopState::transition".to_string(),
            }],
        };
        let text = toml::to_string_pretty(&baseline)?;
        let parsed: Baseline = toml::from_str(&text)?;
        assert_eq!(parsed.finding.len(), 1);
        assert_eq!(parsed.finding[0].symbol, "LoopState::transition");
        Ok(())
    }

    #[test]
    fn runtime_comms_bridge_projection_rule_accepts_current_shape() {
        let bridge = r#"
	            pub fn classified_interaction_to_runtime_input(
	                classified: &PeerInputCandidate,
	                runtime_id: &LogicalRuntimeId,
	            ) -> Input {
	                let interaction = &classified.interaction;
	                if classified.class() == PeerInputClass::PlainEvent {
	                    let source_name = classified
	                        .ingress
	                        .plain_event_source_name()
	                        .unwrap_or("unknown");
	                    return Input::ExternalEvent(todo!());
	                }

	                peer_input_from_ingress_fact(interaction, runtime_id, &classified.ingress)
	            }

	            fn peer_input_from_ingress_fact(
	                interaction: &InboxInteraction,
	                runtime_id: &LogicalRuntimeId,
	                ingress: &PeerIngressFact,
	            ) -> Input {
	                Input::Peer(PeerInput { objective_id: None,
	                    body: peer_rendered_body(interaction),
                    blocks: peer_blocks(interaction),
                })
            }

            fn peer_rendered_body(interaction: &InboxInteraction) -> String {
                if !interaction.rendered_text.is_empty() {
                    return interaction.rendered_text.clone();
                }
                String::new()
            }

            fn peer_blocks(interaction: &InboxInteraction) -> Option<Vec<ContentBlock>> {
                match &interaction.content {
                    InteractionContent::Message { blocks, .. }
                    | InteractionContent::IncarnationFencedMessage { blocks, .. } => blocks.clone(),
                    _ => None,
                }
            }
        "#;
        let drain = r"
            fn run_comms_drain(ci: PeerInputCandidate, runtime_id: LogicalRuntimeId) {
                let input = classified_interaction_to_runtime_input(&ci, &runtime_id);
            }
        ";

        let findings = runtime_comms_bridge_projection_findings_for_sources(
            "meerkat-runtime/src/comms_bridge.rs",
            bridge,
            "meerkat-runtime/src/comms_drain.rs",
            drain,
        );
        assert!(findings.is_empty(), "unexpected findings: {findings:#?}");
    }

    #[test]
    fn runtime_comms_bridge_projection_rule_flags_raw_prefix_routing_and_dropped_blocks() {
        let bridge = r#"
            pub fn interaction_to_runtime_input(interaction: &InboxInteraction, runtime_id: &LogicalRuntimeId) -> Input {
                if interaction.from.starts_with("event:") {
                    return Input::ExternalEvent(todo!());
                }
                Input::Peer(PeerInput { objective_id: None, body: String::new(), blocks: None })
            }
        "#;
        let drain = r"
            fn run_comms_drain(ci: InboxInteraction, runtime_id: LogicalRuntimeId) {
                let input = interaction_to_runtime_input(&ci, &runtime_id);
            }
        ";

        let findings = runtime_comms_bridge_projection_findings_for_sources(
            "meerkat-runtime/src/comms_bridge.rs",
            bridge,
            "meerkat-runtime/src/comms_drain.rs",
            drain,
        );
        let rules = findings
            .iter()
            .map(|finding| finding.key.rule.as_str())
            .collect::<Vec<_>>();
        assert!(
            rules
                .iter()
                .all(|rule| *rule == "RuntimeCommsBridgeProjectionAlignment")
        );
        assert!(
            findings.len() >= 5,
            "expected multiple projection findings, got {findings:#?}"
        );
    }

    #[test]
    fn runtime_external_event_projection_rule_accepts_literal_text_and_blocks() {
        let stdin = r#"
            fn make_stdin_external_event_input(body: String, format: StdinLineFormat) -> Input {
                Input::ExternalEvent(ExternalEventInput { objective_id: None,
                    payload: match format {
                        StdinLineFormat::Text => serde_json::json!({ "body": body }),
                        StdinLineFormat::Json => match serde_json::from_str::<serde_json::Value>(&body) {
                            Ok(parsed) => serde_json::json!({ "body": parsed }),
                            Err(_) => serde_json::json!({ "body": body }),
                        },
                    },
                    blocks: None,
                })
            }
        "#;
        let bridge = r"
	            pub fn classified_interaction_to_runtime_input(classified: &PeerInputCandidate) -> Input {
	                let interaction = &classified.interaction;
	                if classified.class() == PeerInputClass::PlainEvent {
	                    let blocks = external_event_blocks(interaction);
	                    return Input::ExternalEvent(ExternalEventInput { objective_id: None,
	                        payload: external_event_payload(interaction),
                        blocks,
                    });
                }
                todo!()
            }

            fn external_event_blocks(interaction: &InboxInteraction) -> Option<Vec<ContentBlock>> {
                match &interaction.content {
                    InteractionContent::Message { blocks, .. }
                    | InteractionContent::IncarnationFencedMessage { blocks, .. } => blocks.clone(),
                    _ => None,
                }
            }
        ";
        let loop_source = r"
            fn external_event_notice_renderable(event: &ExternalEventInput) -> CoreRenderable {
                CoreRenderable::SystemNotice {
                    kind: SystemNoticeKind::ExternalEvent,
                    body: Some(String::new()),
                    blocks: vec![SystemNoticeBlock::ExternalEvent {
                        source: String::new(),
                        event_type: String::new(),
                        summary: None,
                        body: None,
                        payload: Some(event.payload.clone()),
                        content: event.blocks.clone().unwrap_or_default(),
                    }],
                }
            }

            fn input_to_append(input: &Input) -> Option<ConversationAppend> {
                let content = match input {
                    Input::ExternalEvent(e) => external_event_notice_renderable(e),
                    _ => todo!(),
                };
                Some(todo!())
            }
        ";

        let findings = runtime_external_event_projection_findings_for_sources(
            "meerkat-cli/src/stdin_events.rs",
            stdin,
            "meerkat-runtime/src/comms_bridge.rs",
            bridge,
            "meerkat-runtime/src/runtime_loop.rs",
            loop_source,
        );
        assert!(findings.is_empty(), "unexpected findings: {findings:#?}");
    }

    #[test]
    fn runtime_external_event_projection_rule_flags_json_reparse_and_dropped_blocks() {
        let stdin = r#"
            fn make_stdin_external_event_input(body: String, _format: StdinLineFormat) -> Input {
                let parsed = serde_json::from_str::<serde_json::Value>(&body).unwrap_or(serde_json::Value::String(body));
                Input::ExternalEvent(ExternalEventInput { objective_id: None,
                    payload: serde_json::json!({ "body": parsed }),
                    blocks: None,
                })
            }
        "#;
        let bridge = r#"
	            pub fn classified_interaction_to_runtime_input(classified: &PeerInputCandidate) -> Input {
	                let interaction = &classified.interaction;
	                if classified.class() == PeerInputClass::PlainEvent {
	                    return Input::ExternalEvent(ExternalEventInput { objective_id: None,
                        payload: serde_json::json!({ "body": "flattened" }),
                        blocks: None,
                    });
                }
                todo!()
            }
        "#;
        let loop_source = r"
            fn input_to_append(input: &Input) -> Option<ConversationAppend> {
                let content = match input {
                    Input::ExternalEvent(_) => CoreRenderable::Text { text: String::new() },
                    _ => todo!(),
                };
                Some(todo!())
            }
        ";

        let findings = runtime_external_event_projection_findings_for_sources(
            "meerkat-cli/src/stdin_events.rs",
            stdin,
            "meerkat-runtime/src/comms_bridge.rs",
            bridge,
            "meerkat-runtime/src/runtime_loop.rs",
            loop_source,
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.key.rule == "RuntimeExternalEventProjectionAlignment"),
            "unexpected rule set: {findings:#?}"
        );
        assert!(
            findings.len() >= 4,
            "expected multiple projection findings, got {findings:#?}"
        );
    }

    fn guarded_apply_findings(source: &str) -> Vec<Finding> {
        let parsed = syn::parse_file(source).expect("parse guarded-apply fixture");
        let mut visitor = GuardedApplyVisitor::new("meerkat-mob/src/runtime/actor.rs", source);
        visitor.visit_file(&parsed);
        visitor.findings
    }

    #[test]
    fn guarded_apply_flags_state_gated_apply_via_ast_not_token_soup() {
        let flagged = guarded_apply_findings(
            r"
            fn drive(orch: &Orchestrator) {
                if orch.phase().is_running() {
                    authority.apply(input);
                }
            }
        ",
        );
        assert_eq!(
            flagged.len(),
            1,
            "state-read-gated apply must flag: {flagged:#?}"
        );
        assert_eq!(flagged[0].key.rule, "NoGuardedApply");
    }

    #[test]
    fn guarded_apply_ignores_state_names_in_strings_and_comments() {
        // `phase`/`apply` appear only inside a string literal and a comment in
        // the condition; the AST carries no state-read method/field there, so
        // the token-soup false positive is gone.
        let clean = guarded_apply_findings(
            r#"
            fn drive(label: &str) {
                // phase check used to live here; apply() now unconditional
                if label == "phase" {
                    authority.apply(input);
                }
            }
        "#,
        );
        assert!(
            clean.is_empty(),
            "string/comment mentions must not flag: {clean:#?}"
        );
    }

    #[test]
    fn guarded_apply_ignores_apply_with_else_branch() {
        let clean = guarded_apply_findings(
            r"
            fn drive(orch: &Orchestrator) {
                if orch.phase().is_running() {
                    authority.apply(input);
                } else {
                    record_skip();
                }
            }
        ",
        );
        assert!(
            clean.is_empty(),
            "an else branch is not a silent gate: {clean:#?}"
        );
    }

    fn banned_helper_findings(rel: &str, source: &str) -> Vec<Finding> {
        let parsed = syn::parse_file(source).expect("parse banned-helper fixture");
        let mut visitor = BannedHelperCallVisitor::new(
            rel,
            source,
            &["composition_dispatch", "recompute_mob_peer_overlay"],
        );
        visitor.visit_file(&parsed);
        visitor.findings
    }

    #[test]
    fn banned_helper_flags_call_not_string_or_comment() {
        let flagged = banned_helper_findings(
            "meerkat-mob/src/runtime/actor.rs",
            r"
            fn run(&self) {
                self.composition_dispatch(effect);
                let _ = recompute_mob_peer_overlay(state);
            }
        ",
        );
        assert_eq!(
            flagged.len(),
            2,
            "both banned calls must flag: {flagged:#?}"
        );
        assert!(
            flagged
                .iter()
                .all(|f| f.key.rule == "CompositionDispatchIsThePath")
        );
    }

    #[test]
    fn banned_helper_ignores_name_in_string_and_comment() {
        // Names appear only in a comment and a string literal — no call site,
        // so the prior `/composition/`-skip byte scan false positive is gone
        // even in a file under the composition module.
        let clean = banned_helper_findings(
            "meerkat-runtime/src/composition/dispatcher.rs",
            r#"
            fn run(&self) {
                // composition_dispatch was the legacy fork; gone now.
                let label = "recompute_mob_peer_overlay";
                let _ = label;
            }
        "#,
        );
        assert!(
            clean.is_empty(),
            "doc/import mentions must not flag: {clean:#?}"
        );
    }

    #[test]
    fn repo_walker_skips_hidden_dirs_and_nested_checkouts() {
        let root = tempfile::tempdir().expect("create temp dir");
        let write = |rel: &str| {
            let path = root.path().join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(&path, "fn f() {}\n").expect("write source");
        };
        write("meerkat-core/src/agent.rs");
        write(".codex/worktrees/stale/meerkat-core/src/agent.rs");
        write(".claude/worktrees/agent/src/lib.rs");
        write("wt/parked/src/lib.rs");
        fs::create_dir_all(root.path().join("wt/parked/.git")).expect("create nested .git");

        let files = collect_repo_rs_files(root.path()).expect("walk temp repo");
        let rels = files
            .iter()
            .map(|path| {
                path.strip_prefix(root.path())
                    .expect("under root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rels,
            vec!["meerkat-core/src/agent.rs".to_string()],
            "hidden dirs and nested git checkouts must be excluded"
        );
    }
}
