//! Storage-unification anti-ambient-resolution gate (plan Phase 5).
//!
//! One path authority: durable storage roots come from
//! `meerkat_core::StorageLayout` / the bootstrap resolver, not from ambient
//! process state. This gate bans ambient ROOT resolution — `dirs::*` calls
//! and `HOME`/`XDG_*`/`LOCALAPPDATA`/`APPDATA` environment reads — in
//! production code outside the allowlisted bootstrap/layout/convention
//! modules.
//!
//! Deliberately NOT banned (scoped per the plan): feature-owned *relative*
//! paths (blob dirs, per-realm databases, projection files — banning every
//! path literal would create a storage god-module), `std::env::current_dir`
//! (legitimate execution-context uses: shell working dirs, tool project
//! dirs), and non-path environment reads (API keys, trace sinks).

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::effect_authority::{push_finding, strip_cfg_test_items};

/// Files allowed to resolve ambient roots, with the sanctioned reason.
/// Everything here is either the layout/bootstrap authority itself or a
/// documented storage convention the plan preserves in place.
const AMBIENT_ALLOWLIST: [(&str, &str); 10] = [
    // Deprecated ambient wrapper kept through its compatibility window
    // (no in-repo production callers; removal follows the deprecation
    // cadence).
    (
        "meerkat-skills/src/resolve.rs",
        "deprecated ambient wrapper (resolve_repositories)",
    ),
    // The path authority and the bootstrap resolver: where ambient inputs
    // are turned into the immutable layout.
    ("meerkat-core/src/storage_layout.rs", "the path authority"),
    (
        "meerkat-core/src/runtime_bootstrap.rs",
        "default_state_root / dual-root candidates",
    ),
    // User-global config/mcp document conventions (home-rooted `.rkat`),
    // consumed via the layout's user_home_root at surface level; the
    // ambient forms remain for SDK compatibility.
    (
        "meerkat-core/src/config.rs",
        "global config doc convention (~/.rkat/config.toml)",
    ),
    (
        "meerkat-core/src/mcp_config.rs",
        "user mcp.toml convention (~/.rkat/mcp.toml)",
    ),
    // Durable key material with an exactly-preserved hand-rolled platform
    // resolution (porting it to `dirs` would silently relocate — and thereby
    // rotate — comms identity keys).
    (
        "meerkat/src/sdk.rs",
        "session-comms identity root (resolution preserved exactly)",
    ),
    // Credentials convention: config_dir/meerkat/credentials, contractually
    // unchanged by the storage unification.
    (
        "meerkat-auth-core/src/auth_store/mod.rs",
        "credentials root convention",
    ),
    // Foreign credential convention (Google ADC reads gcloud's own path).
    (
        "meerkat-auth-core/src/authorizers/google.rs",
        "third-party ADC convention",
    ),
    // Surface bootstrap entrypoints: ambient inputs (home default for
    // --user-config-root, ~ expansion of user-typed paths) are gathered
    // here and threaded explicitly from then on.
    ("meerkat-cli/src/main.rs", "CLI bootstrap inputs"),
    ("meerkat-rpc/src/main.rs", "RPC bootstrap inputs"),
];

/// Env vars whose reads constitute ambient root resolution.
const BANNED_ENV_VARS: [&str; 4] = ["HOME", "XDG_STATE_HOME", "LOCALAPPDATA", "APPDATA"];

const GATE_LABEL: &str = "storage-ambient gate: ambient root resolution (dirs::* / HOME / XDG / LOCALAPPDATA) belongs in the bootstrap/layout modules — thread roots from meerkat_core::StorageLayout instead";

/// Crate source roots scanned by the gate (production library/binary code;
/// the walker skips tests/, examples/, benches/, generated code).
const SCAN_ROOTS: [&str; 20] = [
    "meerkat-core/src",
    "meerkat-store/src",
    "meerkat-sqlite/src",
    "meerkat-session/src",
    "meerkat-memory/src",
    "meerkat-tools/src",
    "meerkat-schedule/src",
    "meerkat-workgraph/src",
    "meerkat-mob/src",
    "meerkat-mob-mcp/src",
    "meerkat-mob-pack/src",
    "meerkat-runtime/src",
    "meerkat-skills/src",
    "meerkat-hooks/src",
    "meerkat-comms/src",
    "meerkat/src",
    "meerkat-cli/src",
    "meerkat-rpc/src",
    "meerkat-rest/src",
    "meerkat-mcp-server/src",
];

pub fn run_storage_ambient_gate() -> Result<()> {
    let root = crate::public_contracts::repo_root()?;
    let findings = collect_storage_ambient_findings(&root)?;
    if findings.is_empty() {
        println!("storage-ambient gate clean");
        return Ok(());
    }
    for finding in &findings {
        eprintln!("{finding}");
    }
    bail!("storage-ambient gate failed: {} finding(s)", findings.len());
}

pub fn collect_storage_ambient_findings(root: &Path) -> Result<Vec<String>> {
    let mut findings = Vec::new();
    for scan_root in SCAN_ROOTS {
        let dir = root.join(scan_root);
        if !dir.exists() {
            continue;
        }
        walk_rs_files(&dir, &mut |path| {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if AMBIENT_ALLOWLIST.iter().any(|(allowed, _)| rel == *allowed) {
                return Ok(());
            }
            let source =
                fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
            // Cheap pre-filter before paying for a parse.
            if !source.contains("dirs::") && !BANNED_ENV_VARS.iter().any(|var| source.contains(var))
            {
                return Ok(());
            }
            // Fail closed on unparseable production code.
            let mut parsed = syn::parse_file(&source)
                .with_context(|| format!("storage-ambient gate cannot parse {rel}"))?;
            strip_cfg_test_items(&mut parsed.items);
            let mut visitor = AmbientVisitor {
                rel: &rel,
                findings: &mut findings,
            };
            visitor.visit_file(&parsed);
            Ok(())
        })?;
    }
    findings.sort();
    findings.dedup();
    Ok(findings)
}

fn walk_rs_files(dir: &Path, visit: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if matches!(
                name.as_ref(),
                "tests" | "examples" | "benches" | "generated" | "target"
            ) {
                continue;
            }
            walk_rs_files(&path, visit)?;
        } else if name.ends_with(".rs") {
            visit(&path)?;
        }
    }
    Ok(())
}

struct AmbientVisitor<'a> {
    rel: &'a str,
    findings: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for AmbientVisitor<'_> {
    fn visit_path(&mut self, node: &'ast syn::Path) {
        // `dirs::home_dir()` / `dirs::data_dir()` / `dirs::config_dir()` ...
        let segments: Vec<String> = node
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect();
        if segments.len() >= 2
            && segments[segments.len() - 2] == "dirs"
            && segments[segments.len() - 1].ends_with("_dir")
        {
            push_finding(self.findings, self.rel, node.span(), GATE_LABEL);
        }
        syn::visit::visit_path(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        // `env::var("HOME")` / `env::var_os("XDG_STATE_HOME")` ... — an env
        // read whose callee path ends in var/var_os and whose first argument
        // is a banned literal.
        if let syn::Expr::Path(callee) = node.func.as_ref()
            && let Some(last) = callee.path.segments.last()
            && matches!(last.ident.to_string().as_str(), "var" | "var_os")
            && let Some(syn::Expr::Lit(lit)) = node.args.first()
            && let syn::Lit::Str(name) = &lit.lit
            && (BANNED_ENV_VARS.contains(&name.value().as_str())
                || name.value().starts_with("XDG_"))
        {
            push_finding(self.findings, self.rel, node.span(), GATE_LABEL);
        }
        syn::visit::visit_expr_call(self, node);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn gate_is_clean_on_the_current_tree() {
        let root = crate::public_contracts::repo_root().expect("repo root");
        let findings = collect_storage_ambient_findings(&root).expect("scan");
        assert!(
            findings.is_empty(),
            "storage-ambient gate found ambient root resolution:\n{}",
            findings.join("\n")
        );
    }

    #[test]
    fn visitor_flags_dirs_calls_and_home_reads() {
        let source = r#"
            pub fn bad_root() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
            pub fn bad_env() -> Option<std::ffi::OsString> {
                std::env::var_os("HOME")
            }
            pub fn fine_env() -> Result<String, std::env::VarError> {
                std::env::var("ANTHROPIC_API_KEY")
            }
            #[cfg(test)]
            pub fn test_only() -> Option<std::path::PathBuf> {
                dirs::home_dir()
            }
        "#;
        let mut parsed = syn::parse_file(source).expect("parse");
        strip_cfg_test_items(&mut parsed.items);
        let mut findings = Vec::new();
        let mut visitor = AmbientVisitor {
            rel: "fixture.rs",
            findings: &mut findings,
        };
        visitor.visit_file(&parsed);
        assert_eq!(findings.len(), 2, "{findings:?}");
    }
}
