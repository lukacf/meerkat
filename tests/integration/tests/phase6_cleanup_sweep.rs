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

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn grep_count_rust_source(pattern: &str, root: &Path, exclude_globs: &[&str]) -> (usize, String) {
    let mut args = vec![
        "--no-ignore-vcs",
        "--glob=*.rs",
        // Exclude generated artifacts and archival fixtures.
        "--glob=!target/**",
        "--glob=!**/generated/**",
        "--glob=!tests/integration/tests/phase6_cleanup_sweep.rs",
    ];
    for glob in exclude_globs {
        args.push(glob);
    }
    args.push("--count-matches");
    args.push(pattern);
    args.push(".");

    let out = Command::new("rg")
        .args(&args)
        .current_dir(root)
        .output()
        .expect("rg is available on the runner");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    // rg --count-matches emits `path:count` lines; total is the sum.
    let total: usize = stdout
        .lines()
        .filter_map(|line| line.rsplit(':').next())
        .filter_map(|s| s.parse::<usize>().ok())
        .sum();
    (total, stdout)
}

/// Symbols Phase 6 commits to deleting. Each entry is
/// `(pattern, human_label, exclude_extra_globs)`.
///
/// `exclude_extra_globs` omits files that are *allowed* to keep the
/// symbol — typically tests asserting legacy-path behavior that ship
/// with the legacy path and will be removed at the same commit.
fn phase6_deletions() -> Vec<(&'static str, &'static str, Vec<&'static str>)> {
    vec![
        (
            r"\bresolve_provider_credentials\b",
            "meerkat/src/factory.rs::resolve_provider_credentials (plan §6.1)",
            vec![],
        ),
        (
            r"\bbuild_direct_session_request\b",
            "meerkat-web-runtime::build_direct_session_request (plan §6.14)",
            vec![],
        ),
        (
            r"\bShimCredential\b",
            "meerkat-client::runtime::binding::ShimCredential (plan §6.11)",
            vec![],
        ),
        (
            r"\bStaticLease::empty\b",
            "StaticLease::empty (plan §6.11)",
            vec![],
        ),
        (
            r"ProviderConfig::(Anthropic|OpenAI|Gemini)\b",
            "ProviderConfig::{Anthropic,OpenAI,Gemini} variants (plan §6.9)",
            vec![],
        ),
        (
            r"\bproviders\.api_keys\b",
            "ProviderSettings.api_keys (plan §6.10)",
            vec![],
        ),
        (
            r"\bproviders\.base_urls\b",
            "ProviderSettings.base_urls (plan §6.10)",
            vec![],
        ),
        (
            r"BuildAgentError::MissingApiKey\b",
            "BuildAgentError::MissingApiKey (plan §6.3)",
            vec![],
        ),
        (
            r"FactoryError::MissingApiKey\b",
            "FactoryError::MissingApiKey (plan §6.3)",
            vec![],
        ),
        (
            r"\bfrom_env\(\)",
            "{OpenAi,Anthropic,Gemini}Client::from_env (plan §6.5)",
            vec![
                "--glob=!**/rpc_auth_methods.rs",
                "--glob=!**/rest_auth_endpoints.rs",
            ],
        ),
        (
            r"ProviderResolver::api_key_for\b",
            "ProviderResolver::api_key_for (plan §6.6)",
            vec![],
        ),
        (
            r"LlmClientFactory::create_client\b",
            "LlmClientFactory::create_client (plan §6.7)",
            vec![],
        ),
    ]
}

#[test]
#[ignore = "plan §6.16 — flips green once Phase 6 slices 6.1–6.15 ship"]
fn phase6_deleted_symbols_have_zero_production_hits() {
    let root = repo_root();
    let mut failures: Vec<(String, usize, String)> = Vec::new();

    for (pattern, label, excludes) in phase6_deletions() {
        let (count, raw) = grep_count_rust_source(pattern, &root, &excludes);
        println!(
            "\n=== {label} ===\npattern: {pattern}\nhits:    {count}\n{raw}",
            label = label,
            pattern = pattern,
            count = count,
            raw = raw
        );
        if count > 0 {
            failures.push((label.to_string(), count, raw));
        }
    }

    if !failures.is_empty() {
        let mut summary =
            String::from("Phase 6 deletion punch list (plan §Phase 6) — symbols still present:\n");
        for (label, count, raw) in &failures {
            summary.push_str(&format!("\n  [{count}] {label}\n{raw}"));
        }
        panic!("{summary}\n\nPhase 6 is not complete until every line above prints 0 hits.");
    }
}

/// Sanity check that the sweep infrastructure itself works — searches
/// for a pattern that we know exists in the workspace. Runs by default;
/// if this fails, `rg` plumbing is broken, not Phase 6.
#[test]
fn sweep_infrastructure_finds_known_pattern() {
    let root = repo_root();
    // `#[tokio::test]` appears hundreds of times across the tree.
    let (count, _) = grep_count_rust_source(r"#\[tokio::test\]", &root, &[]);
    assert!(
        count >= 10,
        "rg plumbing is broken — expected at least 10 hits for #[tokio::test], got {count}"
    );
}
