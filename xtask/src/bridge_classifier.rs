//! W2-F bridge-classifier gate: the supervisor bridge transports typed
//! commands/replies and must not re-interpret `ResponseStatus` — all
//! terminal-vs-progress decisions go through
//! `meerkat_core::interaction::classify_response_terminality` /
//! `TerminalityClass`, so the canonical classifier stays the single source
//! of truth. See issue #264 (W2-F).
//!
//! This is the structural (syn AST) port of the former
//! `scripts/pre-push-bridge-no-responsestatus.sh` grep gate. The banned
//! relationships are resolved on the parsed AST — a `match` whose scrutinee
//! reaches `ResponseStatus`, or a `ResponseStatus::<terminal-variant>` path
//! anywhere in production bridge code — so doc comments and string literals
//! cannot false-positive, multi-line `match` headers are still caught, and
//! `#[cfg(test)]` items are excluded structurally instead of by the prior
//! truncate-at-first-test-marker line heuristic.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::effect_authority::{path_has_pair, push_finding, strip_cfg_test_items};

/// Production bridge files that must not interpret `ResponseStatus`.
const BRIDGE_CLASSIFIER_FILES: [&str; 3] = [
    "meerkat-mob/src/runtime/supervisor_bridge.rs",
    "meerkat-mob/src/runtime/local_bridge.rs",
    "meerkat-contracts/src/wire/supervisor_bridge.rs",
];

const BRIDGE_CLASSIFIER_LABEL: &str = "W2-F violation: bridge code interprets `ResponseStatus` directly — route through `classify_response_terminality` + `TerminalityClass`";

/// Run the gate against the repository root and fail on any finding.
pub fn run_bridge_classifier() -> Result<()> {
    let root = crate::public_contracts::repo_root()?;
    let findings = collect_bridge_classifier_findings(&root)?;
    if findings.is_empty() {
        println!("W2-F bridge-classifier gate clean");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    bail!(
        "W2-F bridge-classifier gate failed: {} finding(s)",
        findings.len()
    );
}

pub fn collect_bridge_classifier_findings(root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    for rel in BRIDGE_CLASSIFIER_FILES {
        let path = root.join(rel);
        if !path.exists() {
            continue;
        }
        let source =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        // Fail closed on unparseable bridge code: a file the gate cannot see
        // structurally must not silently pass.
        let mut parsed = syn::parse_file(&source)
            .with_context(|| format!("bridge-classifier gate cannot parse {rel}"))?;
        // Test code may name `ResponseStatus` (it exercises the canonical
        // classifier directly); drop `#[cfg(test)]` items structurally.
        strip_cfg_test_items(&mut parsed.items);
        let mut visitor = ResponseStatusVisitor {
            rel,
            findings: &mut findings,
        };
        visitor.visit_file(&parsed);
    }
    findings.sort();
    findings.dedup();
    Ok(findings)
}

struct ResponseStatusVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for ResponseStatusVisitor<'_> {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if subtree_mentions_response_status(&node.expr) {
            push_finding(
                self.findings,
                self.rel,
                node.match_token.span(),
                BRIDGE_CLASSIFIER_LABEL,
            );
        }
        syn::visit::visit_expr_match(self, node);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        // Terminality re-derivation: naming a terminal `ResponseStatus`
        // variant (construction, comparison, or pattern) in bridge code.
        // Importing/transporting the *type* stays allowed.
        for variant in ["Completed", "Failed", "Accepted"] {
            if path_has_pair(node, "ResponseStatus", variant) {
                push_finding(
                    self.findings,
                    self.rel,
                    node.span(),
                    BRIDGE_CLASSIFIER_LABEL,
                );
            }
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Macro interiors (e.g. `matches!(status, ResponseStatus::Completed)`)
        // have no typed AST, so the parsed token stream is the structural
        // truth available: flag a `ResponseStatus :: <terminal-variant>`
        // token sequence anywhere inside the invocation.
        if macro_tokens_name_terminal_variant(node.tokens.clone()) {
            push_finding(
                self.findings,
                self.rel,
                node.span(),
                BRIDGE_CLASSIFIER_LABEL,
            );
        }
        syn::visit::visit_macro(self, node);
    }
}

/// `true` when the token stream contains `ResponseStatus :: Completed`,
/// `:: Failed`, or `:: Accepted` (descending into groups).
fn macro_tokens_name_terminal_variant(tokens: proc_macro2::TokenStream) -> bool {
    let mut flat: Vec<String> = Vec::new();
    flatten_tokens(tokens, &mut flat);
    flat.windows(4).any(|window| {
        window[0] == "ResponseStatus"
            && window[1] == ":"
            && window[2] == ":"
            && matches!(window[3].as_str(), "Completed" | "Failed" | "Accepted")
    })
}

fn flatten_tokens(tokens: proc_macro2::TokenStream, out: &mut Vec<String>) {
    for token in tokens {
        match token {
            proc_macro2::TokenTree::Ident(ident) => out.push(ident.to_string()),
            proc_macro2::TokenTree::Punct(punct) => out.push(punct.as_char().to_string()),
            proc_macro2::TokenTree::Group(group) => flatten_tokens(group.stream(), out),
            proc_macro2::TokenTree::Literal(_) => out.push(String::new()),
        }
    }
}

/// `true` when the expression subtree names `ResponseStatus` anywhere
/// (path segment or bare ident).
fn subtree_mentions_response_status(expr: &syn::Expr) -> bool {
    struct MentionFinder {
        found: bool,
    }
    impl<'ast> Visit<'ast> for MentionFinder {
        fn visit_ident(&mut self, node: &'ast proc_macro2::Ident) {
            if node == "ResponseStatus" {
                self.found = true;
            }
            syn::visit::visit_ident(self, node);
        }
    }
    let mut finder = MentionFinder { found: false };
    finder.visit_expr(expr);
    finder.found
}
