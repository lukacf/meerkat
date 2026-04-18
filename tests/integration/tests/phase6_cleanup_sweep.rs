//! Phase 6 T19 — cleanup grep sweep.
//!
//! Plan §Top-down integration tests T19 + §Phase 6 slice 6.16 describe
//! this test as the observable proof for contract C13 ("Phase 6 cleanup
//! completeness — zero production hits for deleted symbols"). Until
//! Phase 6 slices 6.1–6.15 ship, this test documents what remains by
//! emitting a readable diff and asserting the expected zero-count per
//! symbol.
//!
//! Running the sweep:
//!
//!   cargo test -p meerkat-integration-tests --test phase6_cleanup_sweep \
//!       -- --ignored --nocapture
//!
//! It is `#[ignore]` by default so CI does not red on an in-flight
//! Phase 6. The test's assertion *is* the plan-defined gate: the gate
//! will flip green only when each deleted symbol has zero production
//! hits. Until then, the output is the living punch list.
//!
//! Uses std::fs directly (no rg dependency) so it runs on every CI
//! runner whether or not ripgrep is installed.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::type_complexity
)]

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR → tests/integration; parent ×2 = workspace root.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("tests/integration has a parent")
        .parent()
        .expect("tests has a parent (workspace root)")
        .to_path_buf()
}

/// Recursively walk a directory collecting all `*.rs` file paths,
/// skipping `target/`, `.git/`, and generated-artifact directories.
fn walk_rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_rec(root, &mut out);
    out
}

fn walk_rec(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            // Skip build artifacts, VCS, node_modules, and all `generated`
            // directories (SDK codegen output, machine codegen output).
            if matches!(
                name,
                "target" | ".git" | "node_modules" | "generated" | ".cargo" | "dist"
            ) {
                continue;
            }
            walk_rec(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn count_substring_hits(
    sources: &[PathBuf],
    needle: &str,
    skip_self: &Path,
    skip_relative: &[&str],
) -> Vec<(PathBuf, usize)> {
    let mut hits = Vec::new();
    for path in sources {
        if path == skip_self {
            continue;
        }
        if skip_relative
            .iter()
            .any(|suffix| path.to_string_lossy().ends_with(suffix))
        {
            continue;
        }
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let count = content.matches(needle).count();
        if count > 0 {
            hits.push((path.clone(), count));
        }
    }
    hits
}

/// Symbols Phase 6 commits to deleting. Each entry is
/// `(needle, human_label, skip_suffixes)`. Substring match (not regex)
/// so every needle must be specific enough to avoid false positives —
/// which all plan-§6 deletion targets are.
///
/// `skip_suffixes` allowlists files whose paths end with any of the
/// listed suffixes — for cases where a newer, unrelated subsystem
/// (e.g. the #250 realtime SDK) legitimately shares a signature the
/// needle catches. Each skip must point at a specific file and carry
/// an inline comment explaining why it's out-of-scope for Phase 6.
fn phase6_deletions() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    const NO_SKIP: &[&str] = &[];
    // `OpenAiLiveClient::from_env` in openai_live.rs (#250 realtime SDK)
    // reuses the exact signature the `from_env` needle catches. The
    // realtime path is not yet wired through the ProviderRuntimeRegistry
    // — a follow-up PR (plan-deferral §7) will either delete it or route
    // it through the registry. Until then it's out-of-scope for the
    // Phase 6 sweep.
    const FROM_ENV_SKIP: &[&str] = &["meerkat-client/src/openai_live.rs"];
    vec![
        (
            "resolve_provider_credentials",
            "meerkat/src/factory.rs::resolve_provider_credentials (plan §6.1)",
            NO_SKIP,
        ),
        (
            "build_direct_session_request",
            "meerkat-web-runtime::build_direct_session_request (plan §6.14)",
            NO_SKIP,
        ),
        (
            "ShimCredential",
            "meerkat-client::runtime::binding::ShimCredential (plan §6.11)",
            NO_SKIP,
        ),
        // The needle "StaticLease::empty(" (with open paren) matches the
        // deleted `StaticLease::empty()` constructor but not the
        // task #24 replacement `StaticLease::empty_lease(...)` (which
        // constructs a lease with ResolvedAuthKind::None for
        // authorizer-backed paths).
        (
            "StaticLease::empty(",
            "StaticLease::empty(...) (plan §6.11)",
            NO_SKIP,
        ),
        (
            "ProviderConfig::Anthropic",
            "ProviderConfig::Anthropic (plan §6.9)",
            NO_SKIP,
        ),
        (
            "ProviderConfig::OpenAI",
            "ProviderConfig::OpenAI (plan §6.9)",
            NO_SKIP,
        ),
        (
            "ProviderConfig::Gemini",
            "ProviderConfig::Gemini (plan §6.9)",
            NO_SKIP,
        ),
        (
            "providers.api_keys",
            "ProviderSettings.api_keys (plan §6.10)",
            NO_SKIP,
        ),
        (
            "providers.base_urls",
            "ProviderSettings.base_urls (plan §6.10)",
            NO_SKIP,
        ),
        (
            "BuildAgentError::MissingApiKey",
            "BuildAgentError::MissingApiKey (plan §6.3)",
            NO_SKIP,
        ),
        (
            "FactoryError::MissingApiKey",
            "FactoryError::MissingApiKey (plan §6.3)",
            NO_SKIP,
        ),
        (
            "ProviderResolver::api_key_for",
            "ProviderResolver::api_key_for (plan §6.6)",
            NO_SKIP,
        ),
        (
            "LlmClientFactory::create_client",
            "LlmClientFactory::create_client (plan §6.7)",
            NO_SKIP,
        ),
        (
            "pub enum LlmProvider",
            "LlmProvider enum declaration (plan §6.8)",
            NO_SKIP,
        ),
        (
            "pub trait LlmClientFactory",
            "LlmClientFactory trait declaration (plan §6.7)",
            NO_SKIP,
        ),
        (
            "DefaultClientFactory",
            "DefaultClientFactory struct (plan §6.7)",
            NO_SKIP,
        ),
        (
            "DefaultFactoryConfig",
            "DefaultFactoryConfig struct (plan §6.7)",
            NO_SKIP,
        ),
        (
            "pub struct ProviderResolver",
            "ProviderResolver struct (plan §6.6)",
            NO_SKIP,
        ),
        // Plan §6.5: provider clients' legacy env-reading constructors
        // (OpenAiClient::from_env, AnthropicClient::from_env,
        // GeminiClient::from_env) — zero callers; env fallback belongs
        // to the resolver env_lookup seam, not bypass constructors.
        (
            "pub fn from_env() -> Result<Self, LlmError>",
            "provider client from_env constructors (plan §6.5)",
            FROM_ENV_SKIP,
        ),
        // Plan §6 dogma §5: no bare `api_key` / `base_url` catch-all
        // fields on browser-facing init structs — the "anthropic by
        // convention" fallback is string-convention folklore.
        (
            "api_key: Option<String>,\n    /// Per-provider API keys",
            "Credentials/RuntimeConfig bare api_key field (plan §6 dogma §5)",
            NO_SKIP,
        ),
        (
            "base_url: Option<String>,\n    /// Per-provider base URLs",
            "Credentials/RuntimeConfig bare base_url field (plan §6 dogma §5)",
            NO_SKIP,
        ),
        (
            "ConfigError::MissingApiKey",
            "ConfigError::MissingApiKey orphan variant (plan §6.3 spirit)",
            NO_SKIP,
        ),
    ]
}

// Plan §Phase 6 migration gate. Not a canonical CI test — this is a
// temporary migration guard, runnable on demand, scoped to detecting
// regressions in the Phase 6 flat-path deletion set. Do not add to the
// canonical CI lane. Invoke explicitly:
//   cargo test -p meerkat-integration-tests --test phase6_cleanup_sweep \
//       -- --ignored --nocapture
#[test]
#[ignore = "plan §Phase 6 migration guard — manual-only, not CI-canonical"]
fn phase6_deleted_symbols_have_zero_production_hits() {
    let root = repo_root();
    let self_path = root.join("tests/integration/tests/phase6_cleanup_sweep.rs");
    let sources = walk_rust_sources(&root);
    assert!(
        !sources.is_empty(),
        "no Rust source files discovered under {}",
        root.display()
    );

    let mut failures: Vec<(String, usize, Vec<(PathBuf, usize)>)> = Vec::new();

    for (needle, label, skip_suffixes) in phase6_deletions() {
        let hits = count_substring_hits(&sources, needle, &self_path, skip_suffixes);
        let total: usize = hits.iter().map(|(_, c)| c).sum();
        println!("\n=== {label} ===\nneedle: {needle}\nhits:   {total}");
        for (path, count) in &hits {
            let rel = path.strip_prefix(&root).unwrap_or(path);
            println!("  {count:>4}  {}", rel.display());
        }
        if total > 0 {
            failures.push((label.to_string(), total, hits));
        }
    }

    if !failures.is_empty() {
        let mut summary =
            String::from("Phase 6 deletion punch list (plan §Phase 6) — symbols still present:\n");
        for (label, count, hits) in &failures {
            summary.push_str(&format!("\n  [{count}] {label}\n"));
            for (path, c) in hits {
                let rel = path.strip_prefix(&root).unwrap_or(path);
                summary.push_str(&format!("        {c:>4}  {}\n", rel.display()));
            }
        }
        panic!("{summary}\nPhase 6 is not complete until every line above prints 0 hits.");
    }
}

/// Baseline check: the walker finds a non-trivial number of Rust sources
/// and can detect a pattern known to appear widely in the workspace.
/// If this fails, the sweep plumbing itself is broken (walker, file
/// reads, etc.) — not Phase 6. Safe to run in CI on any runner.
#[test]
fn sweep_infrastructure_finds_known_pattern() {
    let root = repo_root();
    let sources = walk_rust_sources(&root);
    assert!(
        sources.len() >= 100,
        "expected at least 100 .rs files under workspace root, found {} — walker broken?",
        sources.len()
    );
    let hits: usize = sources
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .map(|s| s.matches("#[tokio::test]").count())
        .sum();
    assert!(
        hits >= 10,
        "plumbing broken — expected at least 10 hits for #[tokio::test], got {hits}"
    );
}

/// Proves the deletion-needle list is non-empty and each entry compiles
/// (guards against regressions where someone deletes the list entirely).
#[test]
fn phase6_deletion_list_is_non_empty() {
    let deletions = phase6_deletions();
    assert!(
        deletions.len() >= 10,
        "Phase 6 deletion list looks wrong: {} entries",
        deletions.len()
    );
}
