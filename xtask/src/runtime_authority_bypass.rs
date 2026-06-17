//! Runtime authority bypass audit for the queue-to-run pilot.
//!
//! This is a structural syn-AST gate for raw accepted-input enqueue, raw
//! dequeue, and raw staging calls. Comments and strings are ignored; method
//! calls split across formatting still count.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use quote::ToTokens;
use syn::visit::Visit;

pub fn run_runtime_authority_bypass() -> Result<()> {
    let root = crate::public_contracts::repo_root()?;
    let findings = collect_runtime_authority_bypass_findings(&root)?;
    if findings.is_empty() {
        println!("runtime-authority-bypass audit passed");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    bail!(
        "runtime-authority-bypass audit failed: {} finding(s)",
        findings.len()
    );
}

pub fn collect_runtime_authority_bypass_findings(root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    audit_public_runtime_queue_surface(root, &mut findings)?;
    for file in runtime_source_files(root)? {
        let source = fs::read_to_string(&file.abs).with_context(|| format!("read {}", file.rel))?;
        let parsed = syn::parse_file(&source)
            .with_context(|| format!("parse {} for runtime authority bypass audit", file.rel))?;
        let mut visitor = RuntimeAuthorityBypassVisitor {
            rel: &file.rel,
            findings: &mut findings,
            fn_stack: Vec::new(),
            skip_test_depth: 0,
        };
        visitor.visit_file(&parsed);
    }
    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn audit_public_runtime_queue_surface(root: &Path, findings: &mut Vec<String>) -> Result<()> {
    let lib = root.join("meerkat-runtime/src/lib.rs");
    if !lib.exists() {
        return Ok(());
    }
    let source = fs::read_to_string(&lib).with_context(|| format!("read {}", lib.display()))?;
    let parsed = syn::parse_file(&source).context("parse meerkat-runtime/src/lib.rs")?;
    for item in parsed.items {
        match item {
            syn::Item::Mod(module)
                if module.ident == "queue" && matches!(module.vis, syn::Visibility::Public(_)) =>
            {
                findings.push(
                    "meerkat-runtime/src/lib.rs: public raw InputQueue module `queue` bypasses generated authority"
                        .to_owned(),
                );
            }
            syn::Item::Use(item_use)
                if matches!(item_use.vis, syn::Visibility::Public(_))
                    && use_tree_contains_ident(&item_use.tree, "InputQueue") =>
            {
                findings.push(
                    "meerkat-runtime/src/lib.rs: public raw InputQueue re-export bypasses generated authority"
                        .to_owned(),
                );
            }
            _ => {}
        }
    }
    Ok(())
}

struct SourceFile {
    rel: String,
    abs: PathBuf,
}

fn runtime_source_files(root: &Path) -> Result<Vec<SourceFile>> {
    let runtime_src = root.join("meerkat-runtime/src");
    if !runtime_src.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mob_runtime_src = root.join("meerkat-mob/src/runtime");
    let mut stack = vec![runtime_src];
    if mob_runtime_src.exists() {
        stack.push(mob_runtime_src);
    }
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("read dir {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push(SourceFile { rel, abs: path });
            }
        }
    }
    Ok(files)
}

struct RuntimeAuthorityBypassVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
    fn_stack: Vec<String>,
    skip_test_depth: usize,
}

impl RuntimeAuthorityBypassVisitor<'_> {
    fn current_fn(&self) -> &str {
        self.fn_stack.last().map(String::as_str).unwrap_or("<root>")
    }

    fn in_skipped_test_scope(&self) -> bool {
        self.skip_test_depth > 0
    }

    fn push_finding(&mut self, kind: &str, method: &str) {
        self.findings.push(format!(
            "{}: {} `{method}` in `{}` bypasses generated authority",
            self.rel,
            kind,
            self.current_fn()
        ));
    }

    fn audit_method_call(&mut self, node: &syn::ExprMethodCall) {
        if self.in_skipped_test_scope() {
            return;
        }
        let method = node.method.to_string();
        match method.as_str() {
            "enqueue" | "enqueue_front"
                if is_queue_projection_receiver(&node.receiver)
                    && !matches!(
                        self.current_fn(),
                        "apply_persist_and_queue" | "rebuild_queue_projections"
                    ) =>
            {
                self.push_finding("raw accepted-input enqueue", &method);
            }
            "dequeue"
                if is_queue_projection_receiver(&node.receiver)
                    && self.current_fn() != "dequeue_next" =>
            {
                self.push_finding("raw dequeue", &method);
            }
            "dequeue_exact_prefix" if self.current_fn() != "dequeue_batch_exact" => {
                self.push_finding("raw exact dequeue", &method);
            }
            "machine_realize_stage_batch"
                if self.current_fn() != "machine_realize_authorized_stage_batch" =>
            {
                self.push_finding("raw stage", &method);
            }
            "mint_from_generated_command_plan" => {
                self.push_finding("generated capability mint outside runtime bridge", &method);
            }
            _ => {}
        }
    }

    fn audit_call(&mut self, node: &syn::ExprCall) {
        if self.in_skipped_test_scope() {
            return;
        }
        let syn::Expr::Path(path) = node.func.as_ref() else {
            return;
        };
        if path_ends_with(&path.path, "mint_from_generated_command_plan") {
            self.push_finding(
                "generated capability mint outside runtime bridge",
                "mint_from_generated_command_plan",
            );
        }
    }

    fn audit_impl_method_definition(&mut self, node: &syn::ImplItemFn) {
        if self.in_skipped_test_scope() {
            return;
        }
        let method = node.sig.ident.to_string();
        if method == "dequeue_next" && matches!(node.vis, syn::Visibility::Public(_)) {
            self.push_finding("public raw dequeue API", &method);
        }
    }
}

impl<'ast> Visit<'ast> for RuntimeAuthorityBypassVisitor<'_> {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if attrs_are_test_only(&node.attrs) {
            self.skip_test_depth += 1;
            syn::visit::visit_item_mod(self, node);
            self.skip_test_depth -= 1;
        } else {
            syn::visit::visit_item_mod(self, node);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        if attrs_are_test_only(&node.attrs) {
            self.skip_test_depth += 1;
            syn::visit::visit_item_fn(self, node);
            self.skip_test_depth -= 1;
        } else {
            syn::visit::visit_item_fn(self, node);
        }
        self.fn_stack.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.fn_stack.push(node.sig.ident.to_string());
        if attrs_are_test_only(&node.attrs) {
            self.skip_test_depth += 1;
            syn::visit::visit_impl_item_fn(self, node);
            self.skip_test_depth -= 1;
        } else {
            self.audit_impl_method_definition(node);
            syn::visit::visit_impl_item_fn(self, node);
        }
        self.fn_stack.pop();
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        self.audit_method_call(node);
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        self.audit_call(node);
        syn::visit::visit_expr_call(self, node);
    }
}

fn attrs_are_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        let meta = attr.meta.to_token_stream().to_string();
        let compact_meta = meta.replace(char::is_whitespace, "");
        path.is_ident("test")
            || path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "test")
            || compact_meta.contains("cfg(test)")
    })
}

fn use_tree_contains_ident(tree: &syn::UseTree, needle: &str) -> bool {
    match tree {
        syn::UseTree::Path(path) => {
            path.ident == needle || use_tree_contains_ident(&path.tree, needle)
        }
        syn::UseTree::Name(name) => name.ident == needle,
        syn::UseTree::Rename(rename) => rename.ident == needle || rename.rename == needle,
        syn::UseTree::Glob(_) => false,
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .any(|item| use_tree_contains_ident(item, needle)),
    }
}

fn path_ends_with(path: &syn::Path, needle: &str) -> bool {
    path.segments
        .last()
        .is_some_and(|segment| segment.ident == needle)
}

fn is_queue_projection_receiver(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Field(field) => matches!(
            &field.member,
            syn::Member::Named(ident) if ident == "queue" || ident == "steer_queue"
        ),
        syn::Expr::Reference(reference) => is_queue_projection_receiver(&reference.expr),
        syn::Expr::Paren(paren) => is_queue_projection_receiver(&paren.expr),
        syn::Expr::Group(group) => is_queue_projection_receiver(&group.expr),
        _ => false,
    }
}
