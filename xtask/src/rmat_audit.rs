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

    if args.json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
    } else {
        print_findings(&findings);
    }

    let baseline_path = root.join("xtask/rmat-baseline.toml");
    if args.update_baseline {
        write_baseline(&baseline_path, &findings)?;
        println!("updated {}", baseline_path.display());
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
        .filter(|key| !findings.iter().any(|f| &f.key == *key))
        .collect();

    if args.strict && (!new_findings.is_empty() || !stale_baseline.is_empty()) {
        let mut messages = Vec::new();
        if !new_findings.is_empty() {
            messages.push(format!(
                "new RMAT findings:\n{}",
                new_findings
                    .into_iter()
                    .map(|finding| format_finding(finding))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        if !stale_baseline.is_empty() {
            messages.push(format!(
                "stale RMAT baseline entries:\n{}",
                stale_baseline
                    .into_iter()
                    .map(|f| format!("- {} {} {}", f.rule, f.path, f.symbol))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        bail!("{}", messages.join("\n\n"));
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
        println!("{}", format_finding(finding));
    }
}

fn format_finding(finding: &Finding) -> String {
    let suppression = if finding.suppressed {
        " [suppressed]"
    } else {
        ""
    };
    format!(
        "- [{}] {} {} :: {}{}",
        finding.severity, finding.key.rule, finding.key.path, finding.message, suppression
    )
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
            findings.push(error_finding(
                "NoDeadAuthorityWiring",
                relative,
                symbol.symbol,
                format!(
                    "required live symbol `{}` has zero production references in this file set",
                    symbol.symbol
                ),
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
                    if let Some(ident) = &field.ident {
                        let suspicious =
                            ["active", "attached", "pending", "running", "interrupted"]
                                .iter()
                                .any(|needle| ident.to_string().contains(needle));
                        if suspicious {
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
}
