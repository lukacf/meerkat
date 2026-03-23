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
    findings.extend(collect_runtime_comms_bridge_projection_findings(root));
    findings.extend(collect_runtime_external_event_projection_findings(root));

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
            // in non-generated, non-authority code. Check line-by-line to skip
            // comments, and require the variant to appear at a word boundary to
            // avoid false positives from substring matches in longer identifiers.
            let construction_pattern = format!("::{variant_pattern}");
            let has_non_comment_match = source.lines().any(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("//")
                    || trimmed.starts_with("/*")
                    || trimmed.starts_with('*')
                {
                    return false;
                }
                if let Some(pos) = trimmed.find(&construction_pattern) {
                    // Verify the variant name ends at a word boundary (not a substring
                    // of a longer identifier like `::VariantNameExtra`).
                    let end = pos + construction_pattern.len();
                    end >= trimmed.len() || !trimmed.as_bytes()[end].is_ascii_alphanumeric()
                } else {
                    false
                }
            });
            if has_non_comment_match {
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
    let loop_path = "meerkat-runtime/src/runtime_loop.rs";

    let stdin_source = match fs::read_to_string(root.join(stdin_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };
    let bridge_source = match fs::read_to_string(root.join(bridge_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };
    let loop_source = match fs::read_to_string(root.join(loop_path)) {
        Ok(source) => source,
        Err(_) => return Vec::new(),
    };

    runtime_external_event_projection_findings_for_sources(
        stdin_path,
        &stdin_source,
        bridge_path,
        &bridge_source,
        loop_path,
        &loop_source,
    )
}

fn runtime_external_event_projection_findings_for_sources(
    stdin_path: &str,
    stdin_source: &str,
    bridge_path: &str,
    bridge_source: &str,
    loop_path: &str,
    loop_source: &str,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    if !stdin_source.contains("StdinLineFormat::Text => serde_json::json!({ \"body\": body })") {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            stdin_path,
            "make_stdin_external_event_input",
            "stdin text mode must preserve literal lines instead of reparsing JSON-looking input"
                .to_string(),
            false,
        ));
    }

    if !stdin_source
        .contains("StdinLineFormat::Json => match serde_json::from_str::<serde_json::Value>(&body)")
    {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            stdin_path,
            "make_stdin_external_event_input",
            "stdin JSON mode must remain the only mode that reparses structured JSON bodies"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("let blocks = external_event_blocks(interaction);") {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "plain-event bridge must derive ExternalEvent blocks through external_event_blocks"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("payload: external_event_payload(interaction),") {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "plain-event bridge must derive ExternalEvent payload through external_event_payload"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("InteractionContent::Message { blocks, .. } => blocks.clone()") {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            bridge_path,
            "external_event_blocks",
            "plain-event bridge must preserve multimodal message blocks when building ExternalEvent inputs"
                .to_string(),
            false,
        ));
    }

    if !loop_source
        .contains("Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks")
    {
        findings.push(error_finding(
            "RuntimeExternalEventProjectionAlignment",
            loop_path,
            "input_to_append",
            "runtime loop must preserve ExternalEvent blocks as block renderables instead of flattening them to text"
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

    if !contains_identifier_call(bridge_source, "classified_interaction_to_runtime_input") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must route from classified ingress truth instead of a raw interaction-only adapter".to_string(),
            false,
        ));
    }

    if !bridge_source.contains("classified.class == PeerInputClass::PlainEvent") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must derive ExternalEvent routing from PeerInputClass::PlainEvent".to_string(),
            false,
        ));
    }

    if bridge_source.contains(".starts_with(\"event:\")") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "classified_interaction_to_runtime_input",
            "runtime comms bridge must not classify external events from sender-name prefixes"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("body: peer_rendered_body(interaction)") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "interaction_to_peer_input",
            "runtime comms bridge must project peer body from the rendered peer text helper"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("interaction.rendered_text") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_rendered_body",
            "runtime comms bridge must preserve rendered peer text from classified ingress"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("blocks: peer_blocks(interaction)") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "interaction_to_peer_input",
            "runtime comms bridge must project multimodal blocks through the peer block helper"
                .to_string(),
            false,
        ));
    }

    if !bridge_source.contains("InteractionContent::Message { blocks, .. } => blocks.clone()") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            bridge_path,
            "peer_blocks",
            "runtime comms bridge must preserve message blocks from drained peer interactions"
                .to_string(),
            false,
        ));
    }

    if !contains_identifier_call(drain_source, "classified_interaction_to_runtime_input") {
        findings.push(error_finding(
            "RuntimeCommsBridgeProjectionAlignment",
            drain_path,
            "run_comms_drain",
            "runtime comms drain must submit classified interactions through the classified bridge adapter".to_string(),
            false,
        ));
    }

    if contains_identifier_call(drain_source, "interaction_to_runtime_input") {
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

fn contains_identifier_call(source: &str, ident: &str) -> bool {
    let pattern = format!("{ident}(");
    let mut search_start = 0;
    while let Some(offset) = source[search_start..].find(&pattern) {
        let start = search_start + offset;
        let boundary_ok = source[..start]
            .chars()
            .next_back()
            .is_none_or(|ch| !(ch.is_ascii_alphanumeric() || ch == '_'));
        if boundary_ok {
            return true;
        }
        search_start = start + 1;
    }
    false
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

    #[test]
    fn runtime_comms_bridge_projection_rule_accepts_current_shape() {
        let bridge = r#"
            pub fn classified_interaction_to_runtime_input(
                classified: &ClassifiedInboxInteraction,
                runtime_id: &LogicalRuntimeId,
            ) -> Input {
                if classified.class == PeerInputClass::PlainEvent {
                    let source_name = interaction
                        .from
                        .strip_prefix("event:")
                        .unwrap_or(interaction.from.as_str());
                    return Input::ExternalEvent(todo!());
                }

                interaction_to_peer_input(interaction, runtime_id)
            }

            pub fn interaction_to_peer_input(
                interaction: &InboxInteraction,
                runtime_id: &LogicalRuntimeId,
            ) -> Input {
                Input::Peer(PeerInput {
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
                    InteractionContent::Message { blocks, .. } => blocks.clone(),
                    _ => None,
                }
            }
        "#;
        let drain = r"
            fn run_comms_drain(ci: ClassifiedInboxInteraction, runtime_id: LogicalRuntimeId) {
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
                Input::Peer(PeerInput { body: String::new(), blocks: None })
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
                Input::ExternalEvent(ExternalEventInput {
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
            pub fn classified_interaction_to_runtime_input(classified: &ClassifiedInboxInteraction) -> Input {
                let interaction = &classified.interaction;
                if classified.class == PeerInputClass::PlainEvent {
                    let blocks = external_event_blocks(interaction);
                    return Input::ExternalEvent(ExternalEventInput {
                        payload: external_event_payload(interaction),
                        blocks,
                    });
                }
                todo!()
            }

            fn external_event_blocks(interaction: &InboxInteraction) -> Option<Vec<ContentBlock>> {
                match &interaction.content {
                    InteractionContent::Message { blocks, .. } => blocks.clone(),
                    _ => None,
                }
            }
        ";
        let loop_source = r"
            fn input_to_append(input: &Input) -> Option<ConversationAppend> {
                let content = match input {
                    Input::ExternalEvent(e) if e.blocks.is_some() => CoreRenderable::Blocks {
                        blocks: e.blocks.clone().unwrap_or_default(),
                    },
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
                Input::ExternalEvent(ExternalEventInput {
                    payload: serde_json::json!({ "body": parsed }),
                    blocks: None,
                })
            }
        "#;
        let bridge = r#"
            pub fn classified_interaction_to_runtime_input(classified: &ClassifiedInboxInteraction) -> Input {
                let interaction = &classified.interaction;
                if classified.class == PeerInputClass::PlainEvent {
                    return Input::ExternalEvent(ExternalEventInput {
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
}
