use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use quote::ToTokens;
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Expr, ExprAssign, ExprIf, ExprMethodCall, ImplItem, ImplItemFn, Item, ItemFn, ItemImpl, Member,
    Type,
};

use crate::ownership_ledger;
use crate::ownership_ledger::{OwnershipFinding, OwnershipFindingKey, format_ownership_finding};
use crate::public_contracts::repo_root;
use crate::rmat_policy::AuditPolicy;

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
    let policy = AuditPolicy::load();
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
                path: "docs/architecture/finite-ownership-ledger.md".into(),
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
            "ownership ledger doc drift:\n- docs/architecture/finite-ownership-ledger.md is stale relative to typed ownership registry"
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
        .filter(|key| !findings.iter().any(|finding| &finding.key == *key))
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
    }

    findings.extend(route_coverage);

    // Structural seam rules (hard errors)
    findings.extend(collect_handoff_protocol_coverage_findings(policy));
    findings.extend(collect_protocol_realization_site_findings(root, policy));
    findings.extend(collect_protocol_feedback_constraint_findings(root, policy));
    findings.extend(collect_terminal_mapping_constraint_findings(root, policy));

    // Contract-driven findings from ownership registry
    findings.extend(collect_fallback_contract_findings(root, policy)?);
    findings.extend(collect_projection_contract_findings(root)?);
    findings.extend(collect_derived_field_findings(root)?);
    findings.extend(collect_surface_conformance_findings(root, policy));
    findings.extend(collect_atomic_bundle_findings(root));

    findings.sort_by(|a, b| a.key.cmp(&b.key));
    findings.dedup_by(|a, b| a.key == b.key);
    Ok(findings)
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
                    Some("target" | ".git" | ".claude" | "node_modules" | "test-fixtures")
                )
            }) || path
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| matches!(name, "tests" | "examples" | "benches"));
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

    for symbol in &policy.required_live_symbols {
        let ref_count = source.matches(symbol.symbol).count();
        if ref_count == 0 {
            let symbol_name = symbol.symbol;
            findings.push(error_finding(
                "NoDeadAuthorityWiring",
                relative,
                symbol.symbol,
                format!("required live symbol `{symbol_name}` has zero production references in this file set"),
                false,
            ));
        }
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

    for ((producer, effect), rules) in required {
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

    findings
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

impl<'ast> Visit<'ast> for GuardedApplyVisitor<'_> {
    fn visit_expr_if(&mut self, node: &'ast ExprIf) {
        let cond_text = node.cond.to_token_stream().to_string();
        let body_text = node.then_branch.to_token_stream().to_string();
        let apply_count =
            body_text.matches(". apply").count() + body_text.matches(".apply").count();
        let else_empty = node.else_branch.is_none();
        let suppress = has_rmat_allow(self.source, node.span(), "NoGuardedApply");
        let stateish = [
            "current_state",
            "phase",
            "snapshot",
            "state()",
            "terminal_outcome",
            "is_terminal",
        ]
        .iter()
        .any(|needle| cond_text.contains(needle));
        if stateish && apply_count > 0 && else_empty {
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
                .any(|m| m.machine_name == rule.machine);
            if !machine_present {
                continue;
            }
            found_in_any_composition = true;
            let protocol_present = composition
                .handoff_protocols
                .iter()
                .any(|p| p.name == rule.protocol_name && p.effect_variant == rule.effect_variant);
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

    for rule in &policy.protocol_feedback_constraints {
        let variant_pattern = &rule.feedback_input_variant;
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

            let source = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Look for construction of the feedback variant (e.g., `Input::VariantName`)
            // in non-generated, non-authority code.
            let construction_pattern = format!("::{variant_pattern}");
            if source.contains(&construction_pattern) {
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
        if !source.contains("classify_terminal") {
            findings.push(error_finding(
                "TerminalMappingThroughGeneratedHelpers",
                path,
                &rule.protocol_name,
                format!(
                    "generated terminal mapping for producer `{}` is missing `classify_terminal`; \
                     the protocol/codegen surface must generate terminal classification for `{}`",
                    rule.producer_machine, rule.protocol_name
                ),
                false,
            ));
        }
    }
    findings
}

/// Contract-driven audit: fallback policy violations.
///
/// For `HardError` boundaries, the declared function must not contain `warn!` followed
/// by a fallback return pattern -- it should propagate the error instead.
///
/// For `TestOnlyLegacy` boundaries, the declared function should only be called from
/// test modules or should be gated with `#[cfg(test)]`.
fn collect_fallback_contract_findings(root: &Path, policy: &AuditPolicy) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();

    for rule in &policy.fallback_policy_rules {
        let file_path = root.join(&rule.boundary_path);
        let source = match fs::read_to_string(&file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        match rule.policy.as_str() {
            "hard_error" => {
                // For HardError boundaries, check that the function does not contain
                // `warn!` or `tracing::warn!` followed by a default/fallback return.
                if let Some(fn_body) = extract_fn_body(&source, &rule.symbol) {
                    let has_warn = fn_body.contains("warn!") || fn_body.contains("tracing::warn!");
                    let has_fallback_return = fn_body.contains("return")
                        && (fn_body.contains("default()")
                            || fn_body.contains("Default::default")
                            || fn_body.contains("unwrap_or"));
                    if has_warn && has_fallback_return {
                        findings.push(error_finding(
                            "FallbackPolicyViolation",
                            &rule.boundary_path,
                            &rule.symbol,
                            format!(
                                "boundary `{}::{}` has fallback_policy=hard_error but contains \
                                 warn+fallback pattern; should propagate error via Result instead",
                                rule.boundary_path, rule.symbol
                            ),
                            false,
                        ));
                    }
                }
            }
            "test_only_legacy" => {
                // For TestOnlyLegacy, check if the function is used outside test modules.
                let repo_files = collect_repo_rs_files(root)?;
                for file in &repo_files {
                    let relative = file
                        .strip_prefix(root)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                    if relative == rule.boundary_path {
                        continue;
                    }
                    let file_source = match fs::read_to_string(file) {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    let call_pattern = format!(".{}(", rule.symbol);
                    if file_source.contains(&call_pattern) {
                        let in_test_module = is_call_in_test_context(&file_source, &call_pattern);
                        if !in_test_module {
                            findings.push(warn_finding(
                                "FallbackPolicyViolation",
                                &relative,
                                &rule.symbol,
                                format!(
                                    "test_only_legacy function `{}` is called from non-test \
                                     code in `{}`; migrate to the canonical API",
                                    rule.symbol, relative
                                ),
                                false,
                            ));
                        }
                    }
                }
            }
            "observable_best_effort" => {
                // For ObservableBestEffort, check that the function has observability
                // (warn!/info!/tracing) at the fallback point.
                if let Some(fn_body) = extract_fn_body(&source, &rule.symbol) {
                    let has_observability = fn_body.contains("warn!")
                        || fn_body.contains("info!")
                        || fn_body.contains("tracing::");
                    if !has_observability {
                        findings.push(warn_finding(
                            "FallbackPolicyViolation",
                            &rule.boundary_path,
                            &rule.symbol,
                            format!(
                                "boundary `{}::{}` has fallback_policy=observable_best_effort \
                                 but contains no observability (warn!/info!/tracing) at \
                                 fallback point",
                                rule.boundary_path, rule.symbol
                            ),
                            false,
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    Ok(findings)
}

/// Contract-driven audit: projection reclassification violations.
///
/// For each `ProjectionContractEntry` where `reclassification_forbidden: true`,
/// check the sink files for raw string pattern matching that bypasses the typed
/// canonical source.
fn collect_projection_contract_findings(root: &Path) -> Result<Vec<Finding>> {
    let entries = ownership_ledger::projection_contracts_snapshot();
    let mut findings = Vec::new();

    let reclassification_patterns = [
        ".starts_with(",
        ".ends_with(",
        ".contains(\":",
        "as_str() {",
        ".as_str(),",
        "kind_str",
        "type_str",
        "event_type ==",
    ];

    let repo_files = collect_repo_rs_files(root)?;

    for entry in &entries {
        if !entry.reclassification_forbidden {
            continue;
        }

        for file in &repo_files {
            let relative = file
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            let source = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(_) => continue,
            };

            if !source.contains(&entry.sink_type) {
                continue;
            }

            if relative.contains("/src/generated/") || relative.starts_with("xtask/") {
                continue;
            }

            for pattern in &reclassification_patterns {
                if source.contains(pattern) {
                    let canonical_nearby = entry
                        .preserved_fields
                        .iter()
                        .any(|field| source.contains(field));
                    if canonical_nearby {
                        findings.push(warn_finding(
                            "ProjectionReclassificationBypass",
                            &relative,
                            &entry.sink_type,
                            format!(
                                "file references sink type `{}` and contains raw string \
                                 classification pattern `{}`; reclassification is forbidden \
                                 -- consume the typed canonical source `{}` instead",
                                entry.sink_type, pattern, entry.canonical_type
                            ),
                            false,
                        ));
                        break;
                    }
                }
            }
        }
    }

    Ok(findings)
}

/// Contract-driven audit: dead field spread detection.
///
/// For each `DerivedFieldEntry` where `dead_field: true`, check that the field
/// does not appear in any NEW code outside of its original declaration.
fn collect_derived_field_findings(root: &Path) -> Result<Vec<Finding>> {
    let entries = ownership_ledger::derived_fields_snapshot();
    let mut findings = Vec::new();

    let repo_files = collect_repo_rs_files(root)?;

    for entry in &entries {
        if !entry.dead_field {
            continue;
        }

        let field_name = entry
            .field_path
            .rsplit("::")
            .next()
            .unwrap_or(&entry.field_path);

        let declaring_type = entry
            .field_path
            .split("::")
            .next()
            .unwrap_or(&entry.field_path);

        for file in &repo_files {
            let relative = file
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();

            if relative.contains("/src/generated/") || relative.starts_with("xtask/") {
                continue;
            }

            let source = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(_) => continue,
            };

            if source.contains(&format!("struct {declaring_type}")) {
                continue;
            }

            let construction_pattern = format!("{field_name}:");
            let access_pattern = format!(".{field_name}");

            let has_construction = source.lines().any(|line| {
                let trimmed = line.trim();
                trimmed.starts_with(&construction_pattern) && !trimmed.starts_with("//")
            });
            let has_access = source.contains(&access_pattern);

            if has_construction || has_access {
                findings.push(warn_finding(
                    "DeadFieldSpread",
                    &relative,
                    &entry.field_path,
                    format!(
                        "dead field `{}` (always {}) is referenced in `{}`; \
                         avoid spreading dead fields to new code",
                        entry.field_path, entry.derivation_rule, relative
                    ),
                    false,
                ));
            }
        }
    }

    Ok(findings)
}

/// Extract the body of a function with the given name from source code.
/// Returns `None` if the function is not found. Uses a simple brace-counting
/// heuristic rather than full syn parsing for efficiency.
fn extract_fn_body(source: &str, fn_name: &str) -> Option<String> {
    let pattern = format!("fn {fn_name}");
    let start = source.find(&pattern)?;
    let rest = &source[start..];
    let brace_start = rest.find('{')?;
    let body_start = start + brace_start;

    let mut depth = 0u32;
    let mut end = body_start;
    for (i, ch) in source[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = body_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    Some(source[body_start..end].to_string())
}

/// Check if a call pattern appears exclusively within `#[cfg(test)]` or `mod tests` blocks.
/// Returns `true` if ALL occurrences of the pattern are in test context.
fn is_call_in_test_context(source: &str, call_pattern: &str) -> bool {
    let mut in_test_region = false;
    let mut all_in_test = true;

    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed == "#[cfg(test)]" {
            in_test_region = true;
        }
        if line.contains(call_pattern) && !trimmed.starts_with("//") && !in_test_region {
            all_in_test = false;
        }
    }

    all_in_test
}

fn error_finding(
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

/// Validate surface family metadata integrity.
///
/// This checks that registry entries are well-formed: policy rules reference
/// real families, IntentionalDivergence entries have reasons, and Open
/// families have at least one Canonical surface.
///
/// This does NOT verify live code conformance across surfaces — the
/// canonical_contract field is a description string, not a machine-readable
/// schema. Cross-surface conformance verification requires a manifest-backed
/// snapshot/diff system, which is a future mechanism.
fn collect_surface_conformance_findings(_root: &Path, policy: &AuditPolicy) -> Vec<Finding> {
    let families = ownership_ledger::surface_families_snapshot();
    let mut findings = Vec::new();

    // Cross-check: every rule in policy must reference a declared family
    for rule in &policy.surface_conformance_rules {
        let family_exists = families.iter().any(|f| f.family == rule.family);
        if !family_exists {
            findings.push(error_finding(
                "SurfaceConformanceOrphanRule",
                "<policy>",
                &rule.family,
                format!(
                    "surface conformance rule references family `{}` which has no SurfaceFamilyEntry",
                    rule.family
                ),
                false,
            ));
        }
    }

    // Validate family entries: IntentionalDivergence must have a reason,
    // and every Open family must have at least one Canonical surface.
    for family in &families {
        if family.status == ownership_ledger::EntryStatus::Open {
            let has_canonical = family
                .surfaces
                .iter()
                .any(|s| s.conformance == ownership_ledger::SurfaceConformance::Canonical);
            if !has_canonical {
                findings.push(warn_finding(
                    "SurfaceConformanceNoCanonical",
                    "<surface-families>",
                    &family.family,
                    format!(
                        "surface family `{}` has no Canonical surface — cannot verify conformance",
                        family.family
                    ),
                    false,
                ));
            }

            for member in &family.surfaces {
                if member.conformance == ownership_ledger::SurfaceConformance::IntentionalDivergence
                    && member
                        .divergence_reason
                        .as_ref()
                        .map_or(true, |r| r.is_empty())
                {
                    findings.push(error_finding(
                        "SurfaceConformanceMissingReason",
                        "<surface-families>",
                        &format!("{}::{}", family.family, member.surface),
                        format!(
                            "surface `{}` in family `{}` is IntentionalDivergence but has no reason",
                            member.surface, family.family
                        ),
                        false,
                    ));
                }
            }
        }
    }

    findings
}

/// Verify that declared atomic bundles have canonical paths that exist.
///
/// For each AtomicBundleEntry, check that the declared canonical_path file
/// exists and contains a function or type related to the bundle.
/// Verify declared atomic bundles against live code.
///
/// For each bundle: reads the canonical path file, extracts the scope block
/// identified by `scope_anchor`, and verifies every step's `anchor_pattern`
/// appears inside that scope. This is manifest-driven — the patterns come
/// from the registry, not hardcoded here.
fn collect_atomic_bundle_findings(root: &Path) -> Vec<Finding> {
    let bundles = ownership_ledger::atomic_bundles_snapshot();
    let mut findings = Vec::new();

    for bundle in &bundles {
        if bundle.status != ownership_ledger::EntryStatus::Open {
            continue;
        }

        // Metadata checks
        if bundle.steps.len() < 2 {
            findings.push(warn_finding(
                "AtomicBundleTrivial",
                &bundle.canonical_path,
                &bundle.bundle_name,
                format!(
                    "atomic bundle `{}` has fewer than 2 steps",
                    bundle.bundle_name
                ),
                false,
            ));
        }
        if bundle.rollback_contract.trim().is_empty() {
            findings.push(error_finding(
                "AtomicBundleNoRollback",
                &bundle.canonical_path,
                &bundle.bundle_name,
                format!(
                    "atomic bundle `{}` has no rollback contract declared",
                    bundle.bundle_name
                ),
                false,
            ));
        }

        // Live code verification
        let canonical_file = root.join(&bundle.canonical_path);
        let source = match fs::read_to_string(&canonical_file) {
            Ok(s) => s,
            Err(_) => {
                findings.push(error_finding(
                    "AtomicBundleCanonicalPathMissing",
                    &bundle.canonical_path,
                    &bundle.bundle_name,
                    format!(
                        "atomic bundle `{}` declares canonical path `{}` which does not exist",
                        bundle.bundle_name, bundle.canonical_path
                    ),
                    false,
                ));
                continue;
            }
        };

        // Extract the scope block using brace-counting from the scope_anchor
        let scope_block = if bundle.scope_anchor.is_empty() {
            // No scope anchor — use entire file
            source.clone()
        } else {
            match extract_scope_block(&source, &bundle.scope_anchor) {
                Some(block) => block,
                None => {
                    findings.push(error_finding(
                        "AtomicBundleScopeNotFound",
                        &bundle.canonical_path,
                        &bundle.bundle_name,
                        format!(
                            "atomic bundle `{}` scope anchor `{}` not found in `{}`",
                            bundle.bundle_name, bundle.scope_anchor, bundle.canonical_path
                        ),
                        false,
                    ));
                    continue;
                }
            }
        };

        // Verify every step's anchor pattern appears inside the scope block
        for step in &bundle.steps {
            if !scope_block.contains(&step.anchor_pattern) {
                findings.push(error_finding(
                    "AtomicBundleStepMissing",
                    &bundle.canonical_path,
                    &format!("{}::{}", bundle.bundle_name, step.name),
                    format!(
                        "atomic bundle `{}` step `{}` anchor `{}` not found inside scope `{}`",
                        bundle.bundle_name, step.name, step.anchor_pattern, bundle.scope_anchor
                    ),
                    false,
                ));
            }
        }
    }

    findings
}

/// Extract a brace-delimited block starting at the given anchor pattern.
/// Returns the content between the opening `{` after the anchor and its
/// matching closing `}`.
fn extract_scope_block(source: &str, anchor: &str) -> Option<String> {
    let anchor_pos = source.find(anchor)?;
    let rest = &source[anchor_pos..];
    let brace_start = rest.find('{')?;
    let block_start = anchor_pos + brace_start;

    let mut depth = 0u32;
    let mut end = block_start;
    for (i, ch) in source[block_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = block_start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }

    Some(source[block_start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
