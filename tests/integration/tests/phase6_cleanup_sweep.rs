//! Phase 6 T19 — cleanup grep sweep.
//!
//! Plan T19 and Phase 6 slice 6.16 made this test the observable proof for
//! contract C13 ("Phase 6 cleanup completeness: zero production hits for
//! deleted symbols"). Phase 6 has shipped, so the sweep now guards the
//! result: a deleted symbol that comes back fails it wherever this test
//! binary runs (CI's integration lanes, the pre-push hook's).
//!
//! Running the sweep alone:
//!
//!   cargo test -p meerkat-integration-tests --test phase6_cleanup_sweep \
//!       -- --nocapture
//!
//! A needle counts only as a whole token (`token_hits`): a deleted name
//! that is the prefix of a live identifier is not a hit.
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
/// skipping `target/`, `.git/`, generated-artifact directories, other
/// checkouts nested below the root and the roots of the repository's
/// other products (`ci/products.json`), whose sources are theirs.
fn walk_rust_sources(root: &Path) -> Vec<PathBuf> {
    let registry: serde_json::Value = fs::read_to_string(root.join("ci/products.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    let products: Vec<PathBuf> = registry["products"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|product| product["root"].as_str())
        .filter(|product_root| !product_root.is_empty())
        .map(|product_root| root.join(product_root))
        .collect();
    let mut out = Vec::new();
    walk_rec(root, &products, &mut out);
    out
}

fn walk_rec(dir: &Path, products: &[PathBuf], out: &mut Vec<PathBuf>) {
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
            // A directory with a `.git` entry of its own is another checkout
            // (a worktree under the gitignored `.claude/worktrees/`, say):
            // its sources are another branch's, not this tree's.
            if path.join(".git").exists() || products.contains(&path) {
                continue;
            }
            walk_rec(&path, products, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// Occurrences of `needle` in `content` that stand as whole tokens. Where the
/// needle starts (ends) with an identifier character, the character before
/// (after) the occurrence must not be one: `pub enum LlmProvider` finds the
/// deleted enum, not `pub enum LlmProviderErrorKind`, while a needle that
/// ends in punctuation, such as `StaticLease::empty(`, matches whatever
/// follows it.
fn token_hits(content: &str, needle: &str) -> usize {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let open_start = needle.starts_with(ident);
    let open_end = needle.ends_with(ident);
    content
        .match_indices(needle)
        .filter(|&(at, _)| {
            let joins_before = open_start && content[..at].chars().next_back().is_some_and(ident);
            let joins_after = open_end
                && content[at + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(ident);
            !(joins_before || joins_after)
        })
        .count()
}

fn count_token_hits(
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
        let count = token_hits(&content, needle);
        if count > 0 {
            hits.push((path.clone(), count));
        }
    }
    hits
}

/// Symbols Phase 6 deleted. Each entry is
/// `(needle, human_label, skip_suffixes)`. A needle matches as a whole
/// token (`token_hits`, not a regex), so it must still be specific enough
/// that only the deleted symbol spells it.
///
/// `skip_suffixes` allowlists files whose paths end with any of the
/// listed suffixes — for cases where a newer, unrelated subsystem
/// (e.g. the #250 realtime SDK) legitimately shares a signature the
/// needle catches. Each skip must point at a specific file and carry
/// an inline comment explaining why it's out-of-scope for Phase 6.
fn phase6_deletions() -> Vec<(&'static str, &'static str, &'static [&'static str])> {
    const NO_SKIP: &[&str] = &[];
    vec![
        (
            "resolve_provider_credentials",
            "crates/meerkat/src/factory.rs::resolve_provider_credentials (plan §6.1)",
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
            "provider client from_env constructors (plan §6.5 + deferral §7)",
            NO_SKIP,
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

/// Every deleted symbol still present in the Rust sources under `root`
/// (`self_path` excluded): its label, its total and the files that hold it.
fn phase6_hits(root: &Path, self_path: &Path) -> Vec<(String, usize, Vec<(PathBuf, usize)>)> {
    let sources = walk_rust_sources(root);
    assert!(
        !sources.is_empty(),
        "no Rust source files discovered under {}",
        root.display()
    );

    let mut failures: Vec<(String, usize, Vec<(PathBuf, usize)>)> = Vec::new();

    for (needle, label, skip_suffixes) in phase6_deletions() {
        let hits = count_token_hits(&sources, needle, self_path, skip_suffixes);
        let total: usize = hits.iter().map(|(_, c)| c).sum();
        println!("\n=== {label} ===\nneedle: {needle}\nhits:   {total}");
        for (path, count) in &hits {
            let rel = path.strip_prefix(root).unwrap_or(path);
            println!("  {count:>4}  {}", rel.display());
        }
        if total > 0 {
            failures.push((label.to_string(), total, hits));
        }
    }
    failures
}

// The Phase 6 regression guard: every deleted symbol stays at zero
// production hits.
#[test]
fn phase6_deleted_symbols_have_zero_production_hits() {
    let root = repo_root();
    let self_path = root.join("tests/integration/tests/phase6_cleanup_sweep.rs");
    let failures = phase6_hits(&root, &self_path);

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
        panic!("{summary}\nA deleted Phase 6 symbol is back: every line above must print 0 hits.");
    }
}

/// Needles match whole tokens: the hits this sweep reported for the live
/// `LlmProviderError*` enums are gone, while the deleted enum, a deleted
/// name followed by punctuation, and a needle ending in punctuation still
/// match.
#[test]
fn needles_match_whole_tokens_only() {
    let needle = "pub enum LlmProvider";
    assert_eq!(token_hits("pub enum LlmProviderErrorKind {", needle), 0);
    assert_eq!(
        token_hits("pub enum LlmProviderErrorRetryability {", needle),
        0
    );
    assert_eq!(token_hits("pub enum LlmProvider {", needle), 1);
    assert_eq!(token_hits("pub enum LlmProvider\n{", needle), 1);
    assert_eq!(
        token_hits("let c = my_ShimCredential;", "ShimCredential"),
        0
    );
    assert_eq!(
        token_hits("let c = ShimCredential::new();", "ShimCredential"),
        1
    );
    assert_eq!(
        token_hits("StaticLease::empty(kind)", "StaticLease::empty("),
        1
    );
    assert_eq!(
        token_hits("StaticLease::empty_lease(kind)", "StaticLease::empty("),
        0
    );
}

/// A deleted symbol in production code still fails the sweep: in a tree
/// whose one source declares the deleted `LlmProvider` enum beside the live
/// `LlmProviderErrorKind`, the sweep reports exactly that enum, once.
#[test]
fn a_deleted_symbol_in_production_code_fails_the_sweep() {
    let tree = tempfile::tempdir().expect("temp dir");
    let src = tree.path().join("crates/demo/src");
    fs::create_dir_all(&src).expect("source dir");
    fs::write(
        src.join("lib.rs"),
        "pub enum LlmProviderErrorKind {\n    Auth,\n}\n\npub enum LlmProvider {\n    OpenAi,\n}\n",
    )
    .expect("source file");
    let failures = phase6_hits(tree.path(), &tree.path().join("absent.rs"));
    assert_eq!(failures.len(), 1, "exactly one deleted symbol is reported");
    let (label, total, hits) = &failures[0];
    assert!(
        label.starts_with("LlmProvider enum declaration"),
        "reported: {label}"
    );
    assert_eq!(*total, 1);
    assert_eq!(hits.len(), 1);
}

/// Another checkout nested below the root is not swept: the deleted enum in
/// a directory with its own `.git` entry (a worktree's `.git` file) is no
/// hit, and the same tree without that entry reports it.
#[test]
fn a_nested_checkout_is_not_swept() {
    let tree = tempfile::tempdir().expect("temp dir");
    let live = tree.path().join("crates/demo/src");
    fs::create_dir_all(&live).expect("source dir");
    fs::write(
        live.join("lib.rs"),
        "pub enum LlmProviderErrorKind {\n    Auth,\n}\n",
    )
    .expect("source file");
    let nested = tree.path().join(".claude/worktrees/older");
    let nested_src = nested.join("crates/demo/src");
    fs::create_dir_all(&nested_src).expect("nested source dir");
    fs::write(
        nested_src.join("lib.rs"),
        "pub enum LlmProvider {\n    OpenAi,\n}\n",
    )
    .expect("nested source file");
    fs::write(
        nested.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/older\n",
    )
    .expect("worktree .git file");

    let absent = tree.path().join("absent.rs");
    assert!(
        phase6_hits(tree.path(), &absent).is_empty(),
        "a nested checkout's sources are not swept"
    );

    fs::remove_file(nested.join(".git")).expect("remove .git file");
    let failures = phase6_hits(tree.path(), &absent);
    assert_eq!(
        failures.len(),
        1,
        "without its .git entry the directory is swept"
    );
    assert_eq!(failures[0].1, 1);
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
