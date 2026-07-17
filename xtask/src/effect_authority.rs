//! Effect-authority audit: runtime interrupt / runtime-effect authority must
//! stay machine-owned.
//!
//! This is the structural (syn AST) port of the former
//! `scripts/audit-effect-authority.sh` regex/awk gate. Every source-structure
//! rule is resolved on the parsed AST — banned idents, enum variants, struct
//! constructions, method calls with specific receivers, visibilities, and
//! `#[cfg(test)]` / trait-impl scoping — so a banned token inside a comment
//! or string literal no longer false-positives, a call split across lines is
//! still caught, and brace-counting cannot desynchronize.
//!
//! Two legs stay deliberately textual:
//! - the tombstone-name bans (`RunControlCommand`, `CoreExecutorControl`)
//!   cover *all* tracked files including docs and comments — re-introducing
//!   the deleted names anywhere is the violation, so text is the contract;
//! - banned log-message strings (e.g. warn-only boundary-cancel text) are
//!   checked as string-literal *content* via the AST (`visit_lit_str`),
//!   because the emitted text itself is the banned artifact.
//!
//! The audit also owns the exhaustive `CoreExecutor` decorator ratchet. It
//! cross-checks the core trait against the runtime decorator so a permissive
//! default can never silently replace forwarding.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// Run the audit against the repository root and fail on any finding.
pub fn run_effect_authority() -> Result<()> {
    let root = crate::public_contracts::repo_root()?;
    let findings = collect_effect_authority_findings(&root)?;
    if findings.is_empty() {
        println!("effect-authority audit passed");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    bail!(
        "effect-authority audit failed: {} finding(s)",
        findings.len()
    );
}

/// Idents whose mere structural presence in peer-admission / comms-drain code
/// proves reach into hard interrupt authority.
const HARD_INTERRUPT_AUTHORITY_IDENTS: [&str; 8] = [
    "hard_cancel_current_run",
    "hard_cancel_run_if_current",
    "interrupt_handle",
    "interrupt_handle_for",
    "interrupt_current_run_with_reason",
    "interrupt_current_run_inner",
    "dispatch_user_interrupt",
    "apply_user_interrupt_live_cancel",
];

/// Service-shaped receivers whose `.interrupt(..)` call on a public surface
/// bypasses the runtime-owned interrupt authority.
const SURFACE_SERVICE_RECEIVERS: [&str; 4] = ["service", "svc", "session_service", "cancel_svc"];

/// Surface files where direct service-interrupt bypasses are banned.
const SURFACE_INTERRUPT_FILES: [&str; 9] = [
    "meerkat-rest/src/lib.rs",
    "meerkat-mcp-server/src/lib.rs",
    "meerkat-rpc/src/handlers/session.rs",
    "meerkat-rpc/src/handlers/turn.rs",
    "meerkat-rpc/src/realtime_ws.rs",
    "meerkat-cli/src/main.rs",
    "meerkat-openai/src/realtime_attachment.rs",
    "meerkat-mob/src/runtime/local_bridge.rs",
    "meerkat-mob/src/runtime/provisioner.rs",
];

/// Standalone demo server: ForceState intentionally uses
/// EphemeralSessionService and has no runtime-owned MeerkatMachine authority
/// to route through.
const EXAMPLE_INTERRUPT_ALLOWLIST: [&str; 1] = ["examples/034-codemob-mcp/src/tools/consult.rs"];

const CORE_EXECUTOR_TRAIT_REL: &str = "meerkat-core/src/lifecycle/core_executor.rs";
const MACHINE_MANAGED_EXECUTOR_REL: &str =
    "meerkat-runtime/src/meerkat_machine/session_management.rs";

/// Methods for which `MachineManagedPostStopExecutor` is a transparent
/// decorator. The structural gate below requires the body to remain a single
/// call to the same method on `self.inner`.
const CORE_EXECUTOR_PURE_FORWARD_METHODS: &[&str] = &[
    "boundary_handle",
    "interrupt_handle",
    "publication_handle",
    "turn_finalization_boundary_handle",
    "apply",
    "checkpoint_committed_session_snapshot",
    "reconcile_committed_compaction_projections",
    "abort_uncommitted_compaction_projections",
    "abort_rejected_run_projections",
    "publish_interaction_terminals",
    "cancel_after_boundary",
];

/// Methods that must forward to the inner executor before adding decorator
/// mechanics.
const CORE_EXECUTOR_FORWARD_PLUS_OVERRIDE_METHODS: &[&str] = &["stop_runtime_executor"];

/// Methods whose semantics are intentionally owned by the decorator.
const CORE_EXECUTOR_LOCAL_OVERRIDE_METHODS: &[&str] = &["cleanup_after_runtime_stop_terminalized"];

/// Capabilities sampled from the inner executor before decoration. The
/// decorator implements these explicitly as false/None so later trait-default
/// changes cannot silently alter its already-consumed capability surface.
const CORE_EXECUTOR_CONSUMED_BEFORE_WRAP_METHODS: &[&str] = &[
    "machine_managed_post_stop_unregister",
    "post_stop_cleanup_handle",
];

/// Files that constitute this gate itself: its rule tables necessarily name
/// the banned tokens, so they are excluded from the tombstone text scan
/// (the bash predecessor excluded itself the same way).
fn is_effect_gate_own_source(rel: &str) -> bool {
    rel == "xtask/src/effect_authority.rs" || rel == "xtask/tests/effect_authority.rs"
}

fn is_test_rust_path(rel: &str) -> bool {
    rel.contains("/tests/") || rel.ends_with("tests.rs")
}

pub fn collect_effect_authority_findings(root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    let files = workspace_files(root)?;

    // Tombstone-name bans (deliberately textual, see module docs): the
    // deleted `RunControl`+`Command` and `CoreExecutor`+`Control` surfaces
    // must not reappear anywhere — code, comments, or docs.
    let tombstones = [
        format!("{}{}", "RunControl", "Command"),
        format!("{}{}", "CoreExecutor", "Control"),
    ];
    for file in &files {
        if is_effect_gate_own_source(&file.rel) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&file.abs) else {
            // Non-UTF-8 payloads (images, archives) carry no tombstone names.
            continue;
        };
        for token in &tombstones {
            for (index, line) in text.lines().enumerate() {
                if line_has_word(line, token) {
                    findings.push(format!(
                        "{}:{}: tombstone name `{token}` references remain",
                        file.rel,
                        index + 1
                    ));
                }
            }
        }
    }

    // All remaining rules are structural over parsed production Rust.
    for file in &files {
        if !file.rel.ends_with(".rs") || is_effect_gate_own_source(&file.rel) {
            continue;
        }
        let source = fs::read_to_string(&file.abs)
            .with_context(|| format!("read {}", file.abs.display()))?;
        let Ok(parsed) = syn::parse_file(&source) else {
            // Fail closed on unparseable production Rust: a file the gate
            // cannot see structurally must not silently pass.
            bail!("effect-authority audit cannot parse {}", file.rel);
        };
        audit_rust_file(&file.rel, parsed, &mut findings);
    }

    collect_core_executor_decorator_findings(root, &mut findings)?;

    findings.sort();
    findings.dedup();
    Ok(findings)
}

struct WorkspaceFile {
    rel: String,
    abs: PathBuf,
}

fn workspace_files(root: &Path) -> Result<Vec<WorkspaceFile>> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).with_context(|| format!("read dir {}", dir.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| format!("read dir entry in {}", dir.display()))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let file_type = entry
                .file_type()
                .with_context(|| format!("read file type {}", path.display()))?;
            if file_type.is_dir() {
                if name == "target" || name == "node_modules" {
                    continue;
                }
                stack.push(path);
            } else if name == "nohup.out" {
                // Stray process logs are runtime junk, not workspace surface
                // (the bash predecessor excluded nohup.out the same way).
                continue;
            } else if file_type.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .with_context(|| format!("relativize {}", path.display()))?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                files.push(WorkspaceFile { rel, abs: path });
            }
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(files)
}

/// `true` when `line` contains `word` bounded by non-ident characters.
fn line_has_word(line: &str, word: &str) -> bool {
    let mut start = 0;
    while let Some(found) = line[start..].find(word) {
        let begin = start + found;
        let end = begin + word.len();
        let before_ok = begin == 0
            || !line[..begin]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        let after_ok = end == line.len()
            || !line[end..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if before_ok && after_ok {
            return true;
        }
        start = end;
    }
    false
}

fn audit_rust_file(rel: &str, mut parsed: syn::File, findings: &mut Vec<String>) {
    // Workspace-wide structural rules.
    if !is_test_rust_path(rel) && rel != "meerkat-runtime/src/meerkat_machine_tests.rs" {
        let mut visitor = LegacyInterruptFnDefVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    if rel != "meerkat-runtime/src/user_interrupt.rs" {
        let mut visitor = PathPairVisitor {
            rel,
            pairs: &[("UserInterruptAuthority", "new")],
            label: "UserInterruptAuthority may only be minted by the command-owned interrupt path",
            findings,
        };
        visitor.visit_file(&parsed);
    }

    if !is_test_rust_path(rel)
        && rel != "meerkat-runtime/src/meerkat_machine/session_management.rs"
        && rel != "meerkat-runtime/src/user_interrupt.rs"
        && rel != "meerkat-runtime/src/meerkat_machine_tests.rs"
    {
        let mut visitor = InterruptCurrentRunCallVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    if !is_test_rust_path(rel)
        && rel != "meerkat-runtime/src/meerkat_machine/session_management.rs"
        && rel != "meerkat-runtime/src/meerkat_machine/dispatch_session.rs"
        && rel != "meerkat-runtime/src/meerkat_machine_tests.rs"
    {
        let mut visitor = InterruptCommandConstructionVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    {
        let mut visitor = PathPairVisitor {
            rel,
            pairs: &[
                ("RuntimeEffect", "cancel_after_boundary"),
                ("RuntimeEffect", "stop_runtime_executor"),
            ],
            label: "direct RuntimeEffect constructor helpers are forbidden",
            findings,
        };
        visitor.visit_file(&parsed);
    }

    if !rel.ends_with("/effect.rs") {
        let mut visitor = RuntimeEffectAssocVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    // Per-file structural rules.
    if rel == "meerkat-runtime/src/meerkat_machine_types.rs" {
        audit_interrupt_command_variant(rel, &parsed, findings);
    }
    if rel == "meerkat-runtime/src/meerkat_machine/mod.rs" {
        audit_user_interrupt_module_mount(rel, &parsed, findings);
    }

    let is_peer_admission = {
        let file_name = rel.rsplit('/').next().unwrap_or(rel);
        file_name.starts_with("peer_admission") || rel.contains("/peer_admission/")
    };
    if is_peer_admission
        || rel == "meerkat-runtime/src/meerkat_machine/dispatch_control.rs"
        || rel == "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs"
    {
        let mut visitor = HardInterruptAuthorityVisitor {
            rel,
            label: "peer-admission code can reach hard interrupt authority",
            interrupt_receivers: &["runtime", "session_service"],
            ban_interrupt_current_run_calls: false,
            allowed_fns: &[],
            findings,
        };
        visitor.visit_file(&parsed);
    }

    if rel == "meerkat-runtime/src/comms_drain.rs" {
        let mut visitor = HardInterruptAuthorityVisitor {
            rel,
            label: "comms-drain code can reach hard interrupt authority",
            interrupt_receivers: &["runtime", "adapter", "session_service"],
            ban_interrupt_current_run_calls: true,
            // Phase 6 (DEC-P6E-7 / §10.6): the machine-admitted hard-cancel
            // serving arm is the single sanctioned reach.
            allowed_fns: &["serve_hard_cancel_member"],
            findings,
        };
        visitor.visit_file(&parsed);
    }

    if rel == "meerkat-rpc/src/session_executor.rs" {
        let mut visitor = RuntimeFieldInterruptVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    if rel == "meerkat-runtime/src/user_interrupt.rs" {
        audit_user_interrupt_file(rel, &parsed, findings);
    }

    if rel == "meerkat-runtime/src/control_plane.rs" {
        let mut visitor = WarnOnlyBoundaryCancelVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    if rel == "meerkat-runtime/src/effect.rs" {
        audit_effect_module_visibility(rel, &parsed, findings);
    }

    if matches!(
        rel,
        "meerkat-runtime/src/meerkat_machine/dispatch_ingress.rs"
            | "meerkat-runtime/src/meerkat_machine/dispatch_control.rs"
            | "meerkat-runtime/src/meerkat_machine/runtime_control.rs"
    ) {
        let mut visitor = InterruptEffectDropVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    // Rules below operate on the production view: `#[cfg(test)]` items are
    // dropped structurally before the walk.
    strip_cfg_test_items(&mut parsed.items);

    if rel == "meerkat-runtime/src/runtime_loop.rs" {
        let mut visitor = CalleeNameVisitor {
            rel,
            banned: &["stop_runtime_executor"],
            label: "runtime_loop must not call executor stop_runtime_executor directly",
            findings,
        };
        visitor.visit_file(&parsed);
    }

    // Phase 6 (§10.6) sanctions exactly one hard-cancel SENDER (the
    // provisioner's hard_cancel_member/hard_cancel_placed_member verbs) and
    // one RECEIVER (comms_drain's serve_hard_cancel_member arm + honest
    // capability report) — those two files come off this ban. actor.rs stays
    // banned: the actor must route through the provisioner verb, never
    // construct bridge hard-cancel commands directly.
    if rel == "meerkat-mob/src/runtime/actor.rs" {
        let mut visitor = BridgeHardCancelVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }

    if rel == "meerkat-mob/src/runtime/local_bridge.rs" {
        let mut visitor = CalleeNameVisitor {
            rel,
            banned: &["hard_cancel_current_run"],
            label: "local mob bridge interrupt must not hard-cancel member sessions",
            findings,
        };
        visitor.visit_file(&parsed);
    }

    if rel.starts_with("meerkat-runtime/src/")
        && !rel.contains("/generated/")
        && !rel.ends_with("/effect.rs")
        && !is_test_rust_path(rel)
        && rel != "meerkat-runtime/src/meerkat_machine_tests.rs"
    {
        let mut visitor = PathPairVisitor {
            rel,
            pairs: &[
                ("RuntimeEffectFact", "CancelAfterBoundary"),
                ("RuntimeEffectFact", "StopRuntimeExecutor"),
                ("MeerkatMachineEffect", "RuntimeEffectFact"),
                ("RuntimeEffectKind", "CancelAfterBoundary"),
                ("RuntimeEffectKind", "StopRuntimeExecutor"),
            ],
            label: "runtime shell files must not construct RuntimeEffectFact literals",
            findings,
        };
        visitor.visit_file(&parsed);
    }

    let in_surface_scope = SURFACE_INTERRUPT_FILES.contains(&rel)
        || (rel.starts_with("examples/") && !EXAMPLE_INTERRUPT_ALLOWLIST.contains(&rel));
    if in_surface_scope {
        // Public-surface interrupt bypass: the `CoreExecutorInterruptHandle`
        // impls are the sanctioned adapter seam, so they are removed
        // structurally before the walk (the predecessor stripped them with
        // awk brace-counting).
        strip_trait_impls(&mut parsed.items, "CoreExecutorInterruptHandle");
        let mut visitor = SurfaceInterruptBypassVisitor { rel, findings };
        visitor.visit_file(&parsed);
    }
}

pub(crate) fn push_finding(
    findings: &mut Vec<String>,
    rel: &str,
    span: proc_macro2::Span,
    label: &str,
) {
    findings.push(format!("{rel}:{}: {label}", span.start().line));
}

fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect()
}

pub(crate) fn path_has_pair(path: &syn::Path, first: &str, second: &str) -> bool {
    let segments = path_segments(path);
    segments
        .windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

fn path_tail_is(path: &syn::Path, name: &str) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == name)
}

/// Tail identifier of a receiver expression: `service` for `service`,
/// `self.service`, `state.service`, `(&service)`, etc.
fn receiver_tail_ident(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        syn::Expr::Field(field) => match &field.member {
            syn::Member::Named(ident) => Some(ident.to_string()),
            syn::Member::Unnamed(_) => None,
        },
        syn::Expr::Reference(reference) => receiver_tail_ident(&reference.expr),
        syn::Expr::Paren(paren) => receiver_tail_ident(&paren.expr),
        syn::Expr::Group(group) => receiver_tail_ident(&group.expr),
        syn::Expr::Await(await_expr) => receiver_tail_ident(&await_expr.base),
        syn::Expr::Try(try_expr) => receiver_tail_ident(&try_expr.expr),
        _ => None,
    }
}

/// `true` when the expression subtree contains a call whose callee name
/// matches `name` (method call or call-path tail).
fn subtree_has_call_named(expr: &syn::Expr, name: &str) -> bool {
    struct CallFinder<'n> {
        name: &'n str,
        found: bool,
    }
    impl<'ast> Visit<'ast> for CallFinder<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == self.name {
                self.found = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = node.func.as_ref()
                && path_tail_is(&path.path, self.name)
            {
                self.found = true;
            }
            syn::visit::visit_expr_call(self, node);
        }
    }
    let mut finder = CallFinder { name, found: false };
    finder.visit_expr(expr);
    finder.found
}

/// Exhaustive delegation ratchet for the runtime-owned CoreExecutor decorator.
///
/// `CoreExecutor` deliberately has permissive defaults for optional
/// capabilities and compatibility projections. That makes an ordinary manual
/// decorator unsafe: a newly added default method compiles while silently
/// bypassing the wrapped executor. Keep the trait surface, decorator impl, and
/// the four semantic delegation categories in exact set equality.
fn collect_core_executor_decorator_findings(root: &Path, findings: &mut Vec<String>) -> Result<()> {
    let trait_path = root.join(CORE_EXECUTOR_TRAIT_REL);
    let decorator_path = root.join(MACHINE_MANAGED_EXECUTOR_REL);
    let trait_exists = trait_path.is_file();
    let decorator_exists = decorator_path.is_file();

    if !trait_exists && !decorator_exists {
        // Small fixture workspaces for unrelated effect-authority rules do not
        // need to synthesize the complete cross-crate delegation contract.
        return Ok(());
    }
    if !trait_exists {
        findings.push(format!(
            "{CORE_EXECUTOR_TRAIT_REL}:1: CoreExecutor decorator delegation ratchet is missing the trait source"
        ));
    }
    if !decorator_exists {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:1: CoreExecutor decorator delegation ratchet is missing the decorator source"
        ));
    }
    if !trait_exists || !decorator_exists {
        return Ok(());
    }

    let trait_source = fs::read_to_string(&trait_path)
        .with_context(|| format!("read {}", trait_path.display()))?;
    let trait_file = syn::parse_file(&trait_source)
        .with_context(|| format!("parse {CORE_EXECUTOR_TRAIT_REL}"))?;
    let decorator_source = fs::read_to_string(&decorator_path)
        .with_context(|| format!("read {}", decorator_path.display()))?;
    let decorator_file = syn::parse_file(&decorator_source)
        .with_context(|| format!("parse {MACHINE_MANAGED_EXECUTOR_REL}"))?;

    let mut trait_declarations = Vec::new();
    collect_named_traits(&trait_file.items, "CoreExecutor", &mut trait_declarations);
    if trait_declarations.len() != 1 {
        findings.push(format!(
            "{CORE_EXECUTOR_TRAIT_REL}:1: CoreExecutor decorator delegation ratchet expected exactly one CoreExecutor trait, found {}",
            trait_declarations.len()
        ));
        return Ok(());
    }
    let trait_declaration = trait_declarations[0];

    let mut decorator_impls = Vec::new();
    collect_named_trait_impls(
        &decorator_file.items,
        "CoreExecutor",
        "MachineManagedPostStopExecutor",
        &mut decorator_impls,
    );
    if decorator_impls.len() != 1 {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:1: CoreExecutor decorator delegation ratchet expected exactly one MachineManagedPostStopExecutor impl, found {}",
            decorator_impls.len()
        ));
        return Ok(());
    }
    let decorator_impl = decorator_impls[0];

    let categories = [
        ("pure-forward", CORE_EXECUTOR_PURE_FORWARD_METHODS),
        (
            "forward-plus-override",
            CORE_EXECUTOR_FORWARD_PLUS_OVERRIDE_METHODS,
        ),
        ("local-override", CORE_EXECUTOR_LOCAL_OVERRIDE_METHODS),
        (
            "consumed-before-wrap",
            CORE_EXECUTOR_CONSUMED_BEFORE_WRAP_METHODS,
        ),
    ];
    let mut classified_methods = BTreeSet::new();
    for (category, methods) in categories {
        for method in methods {
            if !classified_methods.insert((*method).to_string()) {
                findings.push(format!(
                    "{MACHINE_MANAGED_EXECUTOR_REL}:1: CoreExecutor decorator delegation method `{method}` is classified more than once (latest category `{category}`)"
                ));
            }
        }
    }

    let mut trait_methods = BTreeSet::new();
    for item in &trait_declaration.items {
        if let syn::TraitItem::Fn(method) = item {
            let name = method.sig.ident.to_string();
            if !trait_methods.insert(name.clone()) {
                findings.push(format!(
                    "{CORE_EXECUTOR_TRAIT_REL}:{}: CoreExecutor trait declares duplicate method `{name}`",
                    method.sig.ident.span().start().line
                ));
            }
        }
    }
    push_core_executor_method_set_drift(
        findings,
        CORE_EXECUTOR_TRAIT_REL,
        trait_declaration.ident.span().start().line,
        "trait/category",
        &classified_methods,
        &trait_methods,
    );

    let mut decorator_methods = BTreeMap::new();
    for item in &decorator_impl.items {
        if let syn::ImplItem::Fn(method) = item {
            let name = method.sig.ident.to_string();
            if decorator_methods.insert(name.clone(), method).is_some() {
                findings.push(format!(
                    "{MACHINE_MANAGED_EXECUTOR_REL}:{}: MachineManagedPostStopExecutor declares duplicate method `{name}`",
                    method.sig.ident.span().start().line
                ));
            }
        }
    }
    let decorator_method_names = decorator_methods.keys().cloned().collect();
    push_core_executor_method_set_drift(
        findings,
        MACHINE_MANAGED_EXECUTOR_REL,
        decorator_impl.impl_token.span.start().line,
        "decorator/category",
        &classified_methods,
        &decorator_method_names,
    );

    for name in CORE_EXECUTOR_PURE_FORWARD_METHODS {
        let Some(method) = decorator_methods.get(*name) else {
            continue;
        };
        if !is_exact_inner_forward(method, name) {
            findings.push(format!(
                "{MACHINE_MANAGED_EXECUTOR_REL}:{}: pure CoreExecutor decorator method `{name}` must be exactly one call to `self.inner.{name}(...)`",
                method.sig.ident.span().start().line
            ));
        }
    }

    if let Some(method) = decorator_methods.get("stop_runtime_executor") {
        if !block_has_method_call_on_receiver(&method.block, "inner", "stop_runtime_executor") {
            findings.push(format!(
                "{MACHINE_MANAGED_EXECUTOR_REL}:{}: CoreExecutor stop override must forward to `self.inner.stop_runtime_executor(...)`",
                method.sig.ident.span().start().line
            ));
        }
        if !block_has_method_call_on_receiver(
            &method.block,
            "machine",
            "lock_post_stop_cleanup_attachment",
        ) {
            findings.push(format!(
                "{MACHINE_MANAGED_EXECUTOR_REL}:{}: CoreExecutor stop override must acquire the exact post-stop attachment fence",
                method.sig.ident.span().start().line
            ));
        }
    }

    if let Some(method) = decorator_methods.get("cleanup_after_runtime_stop_terminalized")
        && !block_has_method_call_on_receiver(
            &method.block,
            "machine",
            "complete_terminalized_runtime_loop_cleanup_if_current_with_guard",
        )
    {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:{}: CoreExecutor cleanup override must hand off through the exact machine unregister seam",
            method.sig.ident.span().start().line
        ));
    }

    if let Some(method) = decorator_methods.get("machine_managed_post_stop_unregister")
        && !block_is_false(&method.block)
    {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:{}: consumed machine-managed capability must be explicitly false after decoration",
            method.sig.ident.span().start().line
        ));
    }
    if let Some(method) = decorator_methods.get("post_stop_cleanup_handle")
        && !block_is_none(&method.block)
    {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:{}: consumed post-stop cleanup handle must be explicitly None after decoration",
            method.sig.ident.span().start().line
        ));
    }

    check_pre_wrap_core_executor_capability_capture(&decorator_file, findings);
    Ok(())
}

fn collect_named_traits<'a>(
    items: &'a [syn::Item],
    trait_name: &str,
    output: &mut Vec<&'a syn::ItemTrait>,
) {
    for item in items {
        match item {
            syn::Item::Trait(item_trait) if item_trait.ident == trait_name => {
                output.push(item_trait);
            }
            syn::Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_named_traits(nested, trait_name, output);
                }
            }
            _ => {}
        }
    }
}

fn collect_named_trait_impls<'a>(
    items: &'a [syn::Item],
    trait_name: &str,
    type_name: &str,
    output: &mut Vec<&'a syn::ItemImpl>,
) {
    for item in items {
        match item {
            syn::Item::Impl(item_impl)
                if item_impl
                    .trait_
                    .as_ref()
                    .is_some_and(|(_, path, _)| path_tail_is(path, trait_name))
                    && type_tail_is(&item_impl.self_ty, type_name) =>
            {
                output.push(item_impl);
            }
            syn::Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_named_trait_impls(nested, trait_name, type_name, output);
                }
            }
            _ => {}
        }
    }
}

fn type_tail_is(ty: &syn::Type, name: &str) -> bool {
    match ty {
        syn::Type::Path(path) => path.qself.is_none() && path_tail_is(&path.path, name),
        syn::Type::Group(group) => type_tail_is(&group.elem, name),
        syn::Type::Paren(paren) => type_tail_is(&paren.elem, name),
        _ => false,
    }
}

fn push_core_executor_method_set_drift(
    findings: &mut Vec<String>,
    rel: &str,
    line: usize,
    relation: &str,
    expected: &BTreeSet<String>,
    actual: &BTreeSet<String>,
) {
    let missing = expected.difference(actual).cloned().collect::<Vec<_>>();
    let unclassified = actual.difference(expected).cloned().collect::<Vec<_>>();
    if missing.is_empty() && unclassified.is_empty() {
        return;
    }
    findings.push(format!(
        "{rel}:{line}: CoreExecutor {relation} method-set drift: missing={missing:?}, unclassified={unclassified:?}"
    ));
}

fn is_exact_inner_forward(method: &syn::ImplItemFn, forwarded_name: &str) -> bool {
    struct CallShape<'a> {
        forwarded_name: &'a str,
        total_calls: usize,
        matching_inner_calls: usize,
        macros: usize,
    }

    impl<'ast> Visit<'ast> for CallShape<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            self.total_calls += 1;
            if node.method == self.forwarded_name
                && receiver_tail_ident(&node.receiver).as_deref() == Some("inner")
            {
                self.matching_inner_calls += 1;
            }
            syn::visit::visit_expr_method_call(self, node);
        }

        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            self.total_calls += 1;
            syn::visit::visit_expr_call(self, node);
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            self.macros += 1;
            syn::visit::visit_macro(self, node);
        }
    }

    let mut shape = CallShape {
        forwarded_name,
        total_calls: 0,
        matching_inner_calls: 0,
        macros: 0,
    };
    shape.visit_block(&method.block);
    let [syn::Stmt::Expr(expression, None)] = method.block.stmts.as_slice() else {
        return false;
    };
    let expression = peel_forward_expression(expression);
    let syn::Expr::MethodCall(call) = expression else {
        return false;
    };
    call.method == forwarded_name
        && expression_is_self_inner(&call.receiver)
        && shape.total_calls == 1
        && shape.matching_inner_calls == 1
        && shape.macros == 0
}

fn peel_forward_expression(mut expression: &syn::Expr) -> &syn::Expr {
    loop {
        expression = match expression {
            syn::Expr::Await(awaited) => &awaited.base,
            syn::Expr::Try(tried) => &tried.expr,
            syn::Expr::Group(grouped) => &grouped.expr,
            syn::Expr::Paren(parenthesized) => &parenthesized.expr,
            _ => return expression,
        };
    }
}

fn expression_is_self_inner(expression: &syn::Expr) -> bool {
    match expression {
        syn::Expr::Group(grouped) => expression_is_self_inner(&grouped.expr),
        syn::Expr::Paren(parenthesized) => expression_is_self_inner(&parenthesized.expr),
        syn::Expr::Field(field) => {
            let syn::Member::Named(member) = &field.member else {
                return false;
            };
            let syn::Expr::Path(base) = field.base.as_ref() else {
                return false;
            };
            member == "inner" && base.qself.is_none() && base.path.is_ident("self")
        }
        _ => false,
    }
}

fn block_has_method_call_on_receiver(block: &syn::Block, receiver: &str, method: &str) -> bool {
    struct MethodCallFinder<'a> {
        receiver: &'a str,
        method: &'a str,
        found: bool,
    }

    impl<'ast> Visit<'ast> for MethodCallFinder<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == self.method
                && receiver_tail_ident(&node.receiver).as_deref() == Some(self.receiver)
            {
                self.found = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }

    let mut finder = MethodCallFinder {
        receiver,
        method,
        found: false,
    };
    finder.visit_block(block);
    finder.found
}

fn block_is_false(block: &syn::Block) -> bool {
    let [syn::Stmt::Expr(expr, None)] = block.stmts.as_slice() else {
        return false;
    };
    matches!(
        expr,
        syn::Expr::Lit(literal)
            if matches!(&literal.lit, syn::Lit::Bool(value) if !value.value)
    )
}

fn block_is_none(block: &syn::Block) -> bool {
    let [syn::Stmt::Expr(expr, None)] = block.stmts.as_slice() else {
        return false;
    };
    matches!(
        expr,
        syn::Expr::Path(path) if path.qself.is_none() && path.path.is_ident("None")
    )
}

fn check_pre_wrap_core_executor_capability_capture(
    decorator_file: &syn::File,
    findings: &mut Vec<String>,
) {
    let mut materializers = Vec::new();
    collect_impl_methods_named(
        &decorator_file.items,
        "ensure_session_with_executor_factory_inner",
        &mut materializers,
    );
    if materializers.len() != 1 {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:1: CoreExecutor delegation ratchet expected exactly one pre-wrap materializer, found {}",
            materializers.len()
        ));
        return;
    }

    struct CaptureVisitor {
        wrapper_lines: Vec<usize>,
        capture_lines: BTreeMap<String, Vec<usize>>,
    }

    impl<'ast> Visit<'ast> for CaptureVisitor {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if receiver_tail_ident(&node.receiver).as_deref() == Some("executor")
                && CORE_EXECUTOR_CONSUMED_BEFORE_WRAP_METHODS
                    .iter()
                    .any(|method| node.method == *method)
            {
                self.capture_lines
                    .entry(node.method.to_string())
                    .or_default()
                    .push(node.method.span().start().line);
            }
            syn::visit::visit_expr_method_call(self, node);
        }

        fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
            if path_tail_is(&node.path, "MachineManagedPostStopExecutor") {
                self.wrapper_lines.push(node.path.span().start().line);
            }
            syn::visit::visit_expr_struct(self, node);
        }
    }

    let materializer = materializers[0];
    let mut visitor = CaptureVisitor {
        wrapper_lines: Vec::new(),
        capture_lines: BTreeMap::new(),
    };
    visitor.visit_block(&materializer.block);
    let Some(wrapper_line) = visitor.wrapper_lines.into_iter().min() else {
        findings.push(format!(
            "{MACHINE_MANAGED_EXECUTOR_REL}:{}: CoreExecutor pre-wrap materializer never constructs MachineManagedPostStopExecutor",
            materializer.sig.ident.span().start().line
        ));
        return;
    };

    for capability in CORE_EXECUTOR_CONSUMED_BEFORE_WRAP_METHODS {
        let captured_before_wrap = visitor
            .capture_lines
            .get(*capability)
            .is_some_and(|lines| lines.iter().any(|line| *line < wrapper_line));
        if !captured_before_wrap {
            findings.push(format!(
                "{MACHINE_MANAGED_EXECUTOR_REL}:{wrapper_line}: CoreExecutor capability `{capability}` must be captured from `executor` before MachineManagedPostStopExecutor construction"
            ));
        }
    }
}

fn collect_impl_methods_named<'a>(
    items: &'a [syn::Item],
    method_name: &str,
    output: &mut Vec<&'a syn::ImplItemFn>,
) {
    for item in items {
        match item {
            syn::Item::Impl(item_impl) => {
                for item in &item_impl.items {
                    if let syn::ImplItem::Fn(method) = item
                        && method.sig.ident == method_name
                    {
                        output.push(method);
                    }
                }
            }
            syn::Item::Mod(module) => {
                if let Some((_, nested)) = &module.content {
                    collect_impl_methods_named(nested, method_name, output);
                }
            }
            _ => {}
        }
    }
}

/// String-literal contents inside a macro invocation's token stream
/// (macro interiors have no typed AST, so the parsed token stream is the
/// structural truth available — same descent discipline as the
/// peer-response-terminal visitors).
fn macro_string_literals(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Literal(literal) => {
                if let Ok(syn::Lit::Str(lit)) = syn::parse_str::<syn::Lit>(&literal.to_string()) {
                    out.push(lit.value());
                }
            }
            proc_macro2::TokenTree::Group(group) => macro_string_literals(group.stream(), out),
            _ => {}
        }
    }
}

fn attr_is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("test") {
            found = true;
        }
        Ok(())
    });
    found
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::ExternCrate(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        syn::Item::ForeignMod(item) => &item.attrs,
        syn::Item::Impl(item) => &item.attrs,
        syn::Item::Macro(item) => &item.attrs,
        syn::Item::Mod(item) => &item.attrs,
        syn::Item::Static(item) => &item.attrs,
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Trait(item) => &item.attrs,
        syn::Item::TraitAlias(item) => &item.attrs,
        syn::Item::Type(item) => &item.attrs,
        syn::Item::Union(item) => &item.attrs,
        syn::Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

pub(crate) fn strip_cfg_test_items(items: &mut Vec<syn::Item>) {
    items.retain(|item| !item_attrs(item).iter().any(attr_is_cfg_test));
    for item in items.iter_mut() {
        match item {
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = module.content.as_mut() {
                    strip_cfg_test_items(inner);
                }
            }
            syn::Item::Impl(item_impl) => {
                item_impl.items.retain(|impl_item| {
                    let attrs = match impl_item {
                        syn::ImplItem::Const(item) => &item.attrs,
                        syn::ImplItem::Fn(item) => &item.attrs,
                        syn::ImplItem::Type(item) => &item.attrs,
                        syn::ImplItem::Macro(item) => &item.attrs,
                        _ => return true,
                    };
                    !attrs.iter().any(attr_is_cfg_test)
                });
            }
            _ => {}
        }
    }
}

/// Remove every `impl <Trait> for ..` block whose trait tail ident matches.
fn strip_trait_impls(items: &mut Vec<syn::Item>, trait_name: &str) {
    items.retain(|item| {
        if let syn::Item::Impl(item_impl) = item
            && let Some((_, path, _)) = &item_impl.trait_
        {
            return !path_tail_is(path, trait_name);
        }
        true
    });
    for item in items.iter_mut() {
        if let syn::Item::Mod(module) = item
            && let Some((_, inner)) = module.content.as_mut()
        {
            strip_trait_impls(inner, trait_name);
        }
    }
}

/// Banned legacy `fn interrupt_current_run(..)` / `_with_reason` definitions
/// (free, impl-member, or trait-method declarations).
struct LegacyInterruptFnDefVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl LegacyInterruptFnDefVisitor<'_> {
    fn check(&mut self, ident: &syn::Ident) {
        if ident == "interrupt_current_run" || ident == "interrupt_current_run_with_reason" {
            push_finding(
                self.findings,
                self.rel,
                ident.span(),
                "legacy interrupt_current_run alias definitions are forbidden",
            );
        }
    }
}

impl<'ast> Visit<'ast> for LegacyInterruptFnDefVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.check(&node.sig.ident);
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.check(&node.sig.ident);
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.check(&node.sig.ident);
        syn::visit::visit_trait_item_fn(self, node);
    }
}

/// Bans any path containing one of the configured consecutive segment pairs.
struct PathPairVisitor<'a> {
    rel: &'a str,
    pairs: &'a [(&'a str, &'a str)],
    label: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for PathPairVisitor<'_> {
    fn visit_path(&mut self, node: &'ast syn::Path) {
        for (first, second) in self.pairs {
            if path_has_pair(node, first, second) {
                push_finding(self.findings, self.rel, node.span(), self.label);
            }
        }
        syn::visit::visit_path(self, node);
    }
}

/// Bans `RuntimeEffect::<assoc>` paths outside the sealed effect module.
struct RuntimeEffectAssocVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for RuntimeEffectAssocVisitor<'_> {
    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segments = path_segments(node);
        if segments
            .windows(2)
            .any(|window| window[0] == "RuntimeEffect")
        {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                "RuntimeEffect associated constructors must stay inside the sealed effect module",
            );
        }
        syn::visit::visit_path(self, node);
    }
}

/// Bans `.interrupt_current_run(..)` / `_with_reason(..)` callsites
/// (method-call and UFCS forms).
struct InterruptCurrentRunCallVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

const DIRECT_INTERRUPT_CALL_LABEL: &str =
    "direct interrupt_current_run callsites are forbidden outside runtime tests";

impl<'ast> Visit<'ast> for InterruptCurrentRunCallVisitor<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "interrupt_current_run"
            || node.method == "interrupt_current_run_with_reason"
        {
            push_finding(
                self.findings,
                self.rel,
                node.method.span(),
                DIRECT_INTERRUPT_CALL_LABEL,
            );
        }
        syn::visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && (path_tail_is(&path.path, "interrupt_current_run")
                || path_tail_is(&path.path, "interrupt_current_run_with_reason"))
        {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                DIRECT_INTERRUPT_CALL_LABEL,
            );
        }
        syn::visit::visit_expr_call(self, node);
    }
}

/// Bans `MeerkatMachineCommand::InterruptCurrentRun { .. }` *constructions*
/// (struct expressions). Match patterns destructuring the variant are not
/// constructions and stay allowed, mirroring the predecessor's `{ .. }`
/// rest-pattern carve-out.
struct InterruptCommandConstructionVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for InterruptCommandConstructionVisitor<'_> {
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        if path_has_pair(&node.path, "MeerkatMachineCommand", "InterruptCurrentRun") {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                "direct InterruptCurrentRun command construction is forbidden outside the session command API",
            );
        }
        syn::visit::visit_expr_struct(self, node);
    }
}

fn audit_interrupt_command_variant(rel: &str, parsed: &syn::File, findings: &mut Vec<String>) {
    struct VariantVisitor<'a> {
        rel: &'a str,
        findings: &'a mut Vec<String>,
    }
    impl<'ast> Visit<'ast> for VariantVisitor<'_> {
        fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
            if node.ident == "MeerkatMachineCommand" {
                for variant in &node.variants {
                    if variant.ident == "InterruptCurrentRun" {
                        push_finding(
                            self.findings,
                            self.rel,
                            variant.ident.span(),
                            "MeerkatMachineCommand must not expose an InterruptCurrentRun variant",
                        );
                    }
                }
            }
            syn::visit::visit_item_enum(self, node);
        }
    }
    VariantVisitor { rel, findings }.visit_file(parsed);
}

fn audit_user_interrupt_module_mount(rel: &str, parsed: &syn::File, findings: &mut Vec<String>) {
    struct MountVisitor<'a> {
        rel: &'a str,
        findings: &'a mut Vec<String>,
    }
    impl<'ast> Visit<'ast> for MountVisitor<'_> {
        fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
            if node.ident == "user_interrupt" {
                push_finding(
                    self.findings,
                    self.rel,
                    node.ident.span(),
                    "user_interrupt authority module must not be mounted as a meerkat_machine sibling",
                );
            }
            syn::visit::visit_item_mod(self, node);
        }
        fn visit_attribute(&mut self, node: &'ast syn::Attribute) {
            if node.path().is_ident("path")
                && let syn::Meta::NameValue(meta) = &node.meta
                && let syn::Expr::Lit(lit) = &meta.value
                && let syn::Lit::Str(value) = &lit.lit
                && value.value().ends_with("user_interrupt.rs")
            {
                push_finding(
                    self.findings,
                    self.rel,
                    node.span(),
                    "user_interrupt authority module must not be mounted as a meerkat_machine sibling",
                );
            }
            syn::visit::visit_attribute(self, node);
        }
    }
    MountVisitor { rel, findings }.visit_file(parsed);
}

/// Peer-admission / comms-drain reach into hard interrupt authority: any
/// structural occurrence of a hard-interrupt authority ident, the
/// `MeerkatMachineCommand::InterruptCurrentRun` path, or `.interrupt(..)`
/// on the configured receivers.
struct HardInterruptAuthorityVisitor<'a> {
    rel: &'a str,
    label: &'a str,
    interrupt_receivers: &'a [&'a str],
    ban_interrupt_current_run_calls: bool,
    /// Functions sanctioned to reach hard interrupt authority (phase 6: the
    /// member drain's extracted `serve_hard_cancel_member` arm is the ONE
    /// machine-admitted bridge reach). The visitor skips their bodies and
    /// keeps the ban everywhere else in the file.
    allowed_fns: &'a [&'a str],
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for HardInterruptAuthorityVisitor<'_> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if self
            .allowed_fns
            .contains(&node.sig.ident.to_string().as_str())
        {
            return;
        }
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if self
            .allowed_fns
            .contains(&node.sig.ident.to_string().as_str())
        {
            return;
        }
        syn::visit::visit_impl_item_fn(self, node);
    }
    fn visit_ident(&mut self, node: &'ast proc_macro2::Ident) {
        let name = node.to_string();
        if HARD_INTERRUPT_AUTHORITY_IDENTS.contains(&name.as_str()) {
            push_finding(self.findings, self.rel, node.span(), self.label);
        }
        syn::visit::visit_ident(self, node);
    }
    fn visit_path(&mut self, node: &'ast syn::Path) {
        if path_has_pair(node, "MeerkatMachineCommand", "InterruptCurrentRun") {
            push_finding(self.findings, self.rel, node.span(), self.label);
        }
        syn::visit::visit_path(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "interrupt"
            && receiver_tail_ident(&node.receiver)
                .is_some_and(|tail| self.interrupt_receivers.contains(&tail.as_str()))
        {
            push_finding(self.findings, self.rel, node.method.span(), self.label);
        }
        if self.ban_interrupt_current_run_calls
            && (node.method == "interrupt_current_run"
                || node.method == "interrupt_current_run_with_reason")
        {
            push_finding(self.findings, self.rel, node.method.span(), self.label);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// RPC executor interrupt-handle recursion: `.runtime.interrupt(..)` —
/// an `interrupt` method call whose receiver is a field named `runtime`.
struct RuntimeFieldInterruptVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for RuntimeFieldInterruptVisitor<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "interrupt"
            && let syn::Expr::Field(field) = node.receiver.as_ref()
            && matches!(&field.member, syn::Member::Named(ident) if ident == "runtime")
        {
            push_finding(
                self.findings,
                self.rel,
                node.method.span(),
                "RPC executor interrupt handle must not re-enter public SessionRuntime::interrupt",
            );
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// The authorized interior of `user_interrupt.rs`: outside the bodies of the
/// command-owned interior functions, hard-cancel reach and authority minting
/// are banned, and the authority type/constructor must stay private.
const USER_INTERRUPT_AUTHORIZED_FNS: [&str; 3] = [
    "hard_cancel_current_run_authorized",
    "interrupt_current_run_inner",
    "apply_user_interrupt_live_cancel",
];

fn audit_user_interrupt_file(rel: &str, parsed: &syn::File, findings: &mut Vec<String>) {
    let mut production = parsed.clone();
    strip_named_fns(&mut production.items, &USER_INTERRUPT_AUTHORIZED_FNS);

    struct BypassVisitor<'a> {
        rel: &'a str,
        findings: &'a mut Vec<String>,
    }
    impl<'ast> Visit<'ast> for BypassVisitor<'_> {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            let banned = node.method == "hard_cancel_current_run"
                || node.method == "interrupt_handle_for"
                || (node.method == "hard_cancel_current_run_authorized"
                    && matches!(node.receiver.as_ref(), syn::Expr::Path(path) if path.path.is_ident("self")));
            if banned {
                push_finding(
                    self.findings,
                    self.rel,
                    node.method.span(),
                    "public user-interrupt API must route through the command/DSL path",
                );
            }
            syn::visit::visit_expr_method_call(self, node);
        }
        fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
            if let syn::Expr::Path(path) = node.func.as_ref()
                && (path_has_pair(&path.path, "UserInterruptAuthority", "new")
                    || path_tail_is(&path.path, "interrupt_handle_for"))
            {
                push_finding(
                    self.findings,
                    self.rel,
                    node.span(),
                    "public user-interrupt API must route through the command/DSL path",
                );
            }
            syn::visit::visit_expr_call(self, node);
        }
    }
    BypassVisitor { rel, findings }.visit_file(&production);

    struct VisibilityVisitor<'a> {
        rel: &'a str,
        findings: &'a mut Vec<String>,
    }
    impl<'ast> Visit<'ast> for VisibilityVisitor<'_> {
        fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
            if node.ident == "UserInterruptAuthority"
                && !matches!(node.vis, syn::Visibility::Inherited)
            {
                push_finding(
                    self.findings,
                    self.rel,
                    node.ident.span(),
                    "UserInterruptAuthority type and constructor must remain private",
                );
            }
            syn::visit::visit_item_struct(self, node);
        }
        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            if node.sig.ident == "new" && !matches!(node.vis, syn::Visibility::Inherited) {
                push_finding(
                    self.findings,
                    self.rel,
                    node.sig.ident.span(),
                    "UserInterruptAuthority type and constructor must remain private",
                );
            }
            syn::visit::visit_impl_item_fn(self, node);
        }
        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            if node.sig.ident == "new" && !matches!(node.vis, syn::Visibility::Inherited) {
                push_finding(
                    self.findings,
                    self.rel,
                    node.sig.ident.span(),
                    "UserInterruptAuthority type and constructor must remain private",
                );
            }
            syn::visit::visit_item_fn(self, node);
        }
    }
    VisibilityVisitor { rel, findings }.visit_file(parsed);
}

/// Remove named functions (free or impl members) so the remaining file is
/// the "outside the authorized interior" view.
fn strip_named_fns(items: &mut Vec<syn::Item>, names: &[&str]) {
    items.retain(|item| {
        if let syn::Item::Fn(item_fn) = item {
            return !names.iter().any(|name| item_fn.sig.ident == name);
        }
        true
    });
    for item in items.iter_mut() {
        match item {
            syn::Item::Impl(item_impl) => {
                item_impl.items.retain(|impl_item| {
                    if let syn::ImplItem::Fn(impl_fn) = impl_item {
                        return !names.iter().any(|name| impl_fn.sig.ident == name);
                    }
                    true
                });
            }
            syn::Item::Mod(module) => {
                if let Some((_, inner)) = module.content.as_mut() {
                    strip_named_fns(inner, names);
                }
            }
            _ => {}
        }
    }
}

/// Warn-only boundary-cancel handling in the control plane: the executor
/// effect must fail closed. Bans the banned log text as string-literal
/// content and the `if let Err(..) = executor.cancel_after_boundary(..)`
/// shape structurally.
struct WarnOnlyBoundaryCancelVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

const WARN_ONLY_CANCEL_LABEL: &str =
    "cancel-after-boundary executor effects must fail closed, not warn-only";

impl<'ast> Visit<'ast> for WarnOnlyBoundaryCancelVisitor<'_> {
    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        if node
            .value()
            .contains("failed to apply runtime executor effect")
        {
            push_finding(self.findings, self.rel, node.span(), WARN_ONLY_CANCEL_LABEL);
        }
        syn::visit::visit_lit_str(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let mut literals = Vec::new();
        macro_string_literals(node.tokens.clone(), &mut literals);
        if literals
            .iter()
            .any(|value| value.contains("failed to apply runtime executor effect"))
        {
            push_finding(self.findings, self.rel, node.span(), WARN_ONLY_CANCEL_LABEL);
        }
        syn::visit::visit_macro(self, node);
    }
    fn visit_expr_let(&mut self, node: &'ast syn::ExprLet) {
        let err_pattern = matches!(
            node.pat.as_ref(),
            syn::Pat::TupleStruct(pat) if path_tail_is(&pat.path, "Err")
        );
        if err_pattern && subtree_has_call_named(&node.expr, "cancel_after_boundary") {
            push_finding(self.findings, self.rel, node.span(), WARN_ONLY_CANCEL_LABEL);
        }
        syn::visit::visit_expr_let(self, node);
    }
}

/// Dropped or trace-only interrupt-yielding runtime effects: bans
/// `let _ = ..try_send(..into_effect()..)..` on the projected effect and the
/// trace-only log texts as string-literal content.
struct InterruptEffectDropVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

const INTERRUPT_EFFECT_DROP_LABEL: &str =
    "interrupt-yielding runtime effects must not be dropped or trace-only";

impl InterruptEffectDropVisitor<'_> {
    fn value_is_banned(value: &str) -> bool {
        value.contains("boundary cancel was not applied")
            || value.contains("runtime effect channel full after live boundary cancel")
            || value.contains("runtime effect channel closed after live boundary cancel")
    }
}

impl<'ast> Visit<'ast> for InterruptEffectDropVisitor<'_> {
    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        if Self::value_is_banned(&node.value()) {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                INTERRUPT_EFFECT_DROP_LABEL,
            );
        }
        syn::visit::visit_lit_str(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        let mut literals = Vec::new();
        macro_string_literals(node.tokens.clone(), &mut literals);
        if literals.iter().any(|value| Self::value_is_banned(value)) {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                INTERRUPT_EFFECT_DROP_LABEL,
            );
        }
        syn::visit::visit_macro(self, node);
    }
    fn visit_local(&mut self, node: &'ast syn::Local) {
        let wild = matches!(node.pat, syn::Pat::Wild(_));
        if wild
            && let Some(init) = &node.init
            && discards_projected_effect_send(&init.expr)
        {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                INTERRUPT_EFFECT_DROP_LABEL,
            );
        }
        syn::visit::visit_local(self, node);
    }
}

/// `true` for a subtree containing `tx.try_send(<args containing
/// projected_effect.into_effect()>)`.
fn discards_projected_effect_send(expr: &syn::Expr) -> bool {
    struct SendFinder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for SendFinder {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "try_send"
                && receiver_tail_ident(&node.receiver).as_deref() == Some("tx")
                && node.args.iter().any(|arg| {
                    let mut inner = IntoEffectFinder { found: false };
                    inner.visit_expr(arg);
                    inner.found
                })
            {
                self.found = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    struct IntoEffectFinder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for IntoEffectFinder {
        fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
            if node.method == "into_effect"
                && receiver_tail_ident(&node.receiver).as_deref() == Some("projected_effect")
            {
                self.found = true;
            }
            syn::visit::visit_expr_method_call(self, node);
        }
    }
    let mut finder = SendFinder { found: false };
    finder.visit_expr(expr);
    finder.found
}

fn audit_effect_module_visibility(rel: &str, parsed: &syn::File, findings: &mut Vec<String>) {
    struct EffectVisibilityVisitor<'a> {
        rel: &'a str,
        findings: &'a mut Vec<String>,
    }
    const LABEL: &str = "RuntimeEffectFact and RuntimeEffect::from_fact must not be crate-visible";
    impl<'ast> Visit<'ast> for EffectVisibilityVisitor<'_> {
        fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
            if node.ident == "RuntimeEffectFact" && !matches!(node.vis, syn::Visibility::Inherited)
            {
                push_finding(self.findings, self.rel, node.ident.span(), LABEL);
            }
            syn::visit::visit_item_enum(self, node);
        }
        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            if node.sig.ident == "from_fact" && !matches!(node.vis, syn::Visibility::Inherited) {
                push_finding(self.findings, self.rel, node.sig.ident.span(), LABEL);
            }
            syn::visit::visit_impl_item_fn(self, node);
        }
        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            if node.sig.ident == "from_fact" && !matches!(node.vis, syn::Visibility::Inherited) {
                push_finding(self.findings, self.rel, node.sig.ident.span(), LABEL);
            }
            syn::visit::visit_item_fn(self, node);
        }
    }
    EffectVisibilityVisitor { rel, findings }.visit_file(parsed);
}

/// Supervisor bridge hard-cancel exposure: `BridgeCommand::HardCancelMember`
/// paths, the `BridgeHardCancelPayload` ident, and a `hard_cancel_member:
/// true` field (construction or pattern).
struct BridgeHardCancelVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

const BRIDGE_HARD_CANCEL_LABEL: &str =
    "supervisor bridge must not expose hard-cancel member authority";

impl<'ast> Visit<'ast> for BridgeHardCancelVisitor<'_> {
    fn visit_path(&mut self, node: &'ast syn::Path) {
        if path_has_pair(node, "BridgeCommand", "HardCancelMember") {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                BRIDGE_HARD_CANCEL_LABEL,
            );
        }
        syn::visit::visit_path(self, node);
    }
    fn visit_ident(&mut self, node: &'ast proc_macro2::Ident) {
        if node == "BridgeHardCancelPayload" {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                BRIDGE_HARD_CANCEL_LABEL,
            );
        }
        syn::visit::visit_ident(self, node);
    }
    fn visit_field_value(&mut self, node: &'ast syn::FieldValue) {
        if matches!(&node.member, syn::Member::Named(ident) if ident == "hard_cancel_member")
            && matches!(
                &node.expr,
                syn::Expr::Lit(lit) if matches!(&lit.lit, syn::Lit::Bool(value) if value.value)
            )
        {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                BRIDGE_HARD_CANCEL_LABEL,
            );
        }
        syn::visit::visit_field_value(self, node);
    }
    fn visit_field_pat(&mut self, node: &'ast syn::FieldPat) {
        if matches!(&node.member, syn::Member::Named(ident) if ident == "hard_cancel_member")
            && matches!(
                node.pat.as_ref(),
                syn::Pat::Lit(lit) if matches!(&lit.lit, syn::Lit::Bool(value) if value.value)
            )
        {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                BRIDGE_HARD_CANCEL_LABEL,
            );
        }
        syn::visit::visit_field_pat(self, node);
    }
}

/// Bans calls (method or UFCS) whose callee name is in the configured set,
/// plus definitions of functions with those names.
struct CalleeNameVisitor<'a> {
    rel: &'a str,
    banned: &'a [&'a str],
    label: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for CalleeNameVisitor<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        if self.banned.contains(&name.as_str()) {
            push_finding(self.findings, self.rel, node.method.span(), self.label);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref()
            && let Some(last) = path.path.segments.last()
        {
            let name = last.ident.to_string();
            if self.banned.contains(&name.as_str()) {
                push_finding(self.findings, self.rel, node.span(), self.label);
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let name = node.sig.ident.to_string();
        if self.banned.contains(&name.as_str()) {
            push_finding(self.findings, self.rel, node.sig.ident.span(), self.label);
        }
        syn::visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        let name = node.sig.ident.to_string();
        if self.banned.contains(&name.as_str()) {
            push_finding(self.findings, self.rel, node.sig.ident.span(), self.label);
        }
        syn::visit::visit_impl_item_fn(self, node);
    }
}

/// Public-surface interrupt bypass: `.interrupt(..)` on a service-shaped
/// receiver (including `session_service()` call results) and any
/// `.interrupt_current_run(..)` call. The predecessor additionally banned a
/// `.interrupt(` token at the start of a line — a formatting-dependent leg
/// that this structural receiver classification subsumes: the receiver is
/// resolved from the AST regardless of how the chain is wrapped.
struct SurfaceInterruptBypassVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

const SURFACE_INTERRUPT_LABEL: &str =
    "public surface interrupt paths must route through MeerkatMachine::hard_cancel_current_run";

impl SurfaceInterruptBypassVisitor<'_> {
    fn receiver_is_service_shaped(expr: &syn::Expr) -> bool {
        if receiver_tail_ident(expr)
            .is_some_and(|tail| SURFACE_SERVICE_RECEIVERS.contains(&tail.as_str()))
        {
            return true;
        }
        // `session_service().interrupt(..)` / `state.session_service().interrupt(..)`
        match expr {
            syn::Expr::MethodCall(call) => call.method == "session_service",
            syn::Expr::Call(call) => {
                matches!(call.func.as_ref(), syn::Expr::Path(path) if path_tail_is(&path.path, "session_service"))
            }
            syn::Expr::Await(await_expr) => Self::receiver_is_service_shaped(&await_expr.base),
            syn::Expr::Try(try_expr) => Self::receiver_is_service_shaped(&try_expr.expr),
            syn::Expr::Paren(paren) => Self::receiver_is_service_shaped(&paren.expr),
            syn::Expr::Reference(reference) => Self::receiver_is_service_shaped(&reference.expr),
            _ => false,
        }
    }
}

impl<'ast> Visit<'ast> for SurfaceInterruptBypassVisitor<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "interrupt" && Self::receiver_is_service_shaped(&node.receiver) {
            push_finding(
                self.findings,
                self.rel,
                node.method.span(),
                SURFACE_INTERRUPT_LABEL,
            );
        }
        if node.method == "interrupt_current_run"
            || node.method == "interrupt_current_run_with_reason"
        {
            push_finding(
                self.findings,
                self.rel,
                node.method.span(),
                SURFACE_INTERRUPT_LABEL,
            );
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}
