//! Strict codegen-marker header truthfulness audit.
//!
//! A file earns the codegen marker iff it is emitted by one of our
//! codegen passes. This module enforces both directions:
//!
//! * Header without matching codegen-emit path → `ForbiddenHeader` (hand-
//!   edited file claiming to be generated).
//! * Emit path without header → `MissingHeader` (generator failed to mark
//!   its output).
//!
//! The emit-path set is computed from the same schema the protocol codegen
//! iterates, so adding or removing a composition automatically updates the
//! allowlist.
//!
//! Note: the literal marker string is assembled at runtime from two
//! halves below so this source file does not itself match the scan.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use meerkat_machine_schema::{
    CompositionSchema, canonical_composition_schemas, canonical_machine_schemas,
    compat_composition_schemas,
};

use crate::public_contracts::repo_root;

/// Marker substring every codegen-emitted file must contain on one of
/// its first few lines. Assembled at runtime from two halves so this
/// source file does not itself match the scan — otherwise the audit
/// tool would flag its own implementation as a forbidden header.
pub fn generated_marker() -> String {
    let mut s = String::from("@");
    s.push_str("generated");
    s
}

/// Maximum number of leading lines scanned for the marker.
const HEADER_SCAN_LINES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditFinding {
    /// File contains `@generated` but is not emitted by any codegen pass.
    ForbiddenHeader { path: PathBuf },
    /// File is emitted by codegen but lacks `@generated`.
    MissingHeader { path: PathBuf },
}

impl AuditFinding {
    pub fn path(&self) -> &Path {
        match self {
            AuditFinding::ForbiddenHeader { path } | AuditFinding::MissingHeader { path } => {
                path.as_path()
            }
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            AuditFinding::ForbiddenHeader { .. } => "forbidden `@generated` header",
            AuditFinding::MissingHeader { .. } => "missing `@generated` header",
        }
    }
}

/// Scan every file under `roots` whose path matches the codegen-emit roster.
/// Returns every mismatch in both directions.
///
/// Pure function for test use — takes explicit root + emit set rather than
/// reading the live schema.
pub fn audit_with_emit_set(
    workspace_root: &Path,
    emit_paths: &BTreeSet<PathBuf>,
) -> Result<Vec<AuditFinding>> {
    let mut findings = Vec::new();

    // Direction 1: every emit path must have a header.
    for rel in emit_paths {
        let abs = workspace_root.join(rel);
        if !abs.exists() {
            // File declared by codegen but absent from tree — out of scope
            // for this audit (codegen-run or drift checker catches this).
            continue;
        }
        let contents = fs::read_to_string(&abs)
            .with_context(|| format!("read emit-path {}", abs.display()))?;
        if !has_generated_marker(&contents) {
            findings.push(AuditFinding::MissingHeader { path: rel.clone() });
        }
    }

    // Direction 2: scan workspace for any file carrying the marker and
    // assert it is in the emit set.
    let scan_roots = default_scan_roots();
    for scan_root in &scan_roots {
        let abs_root = workspace_root.join(scan_root);
        if !abs_root.exists() {
            continue;
        }
        visit_rs_files(&abs_root, &mut |abs_path| {
            let rel = abs_path
                .strip_prefix(workspace_root)
                .unwrap_or(abs_path)
                .to_path_buf();
            if emit_paths.contains(&rel) {
                return Ok(());
            }
            let contents = fs::read_to_string(abs_path)
                .with_context(|| format!("read candidate {}", abs_path.display()))?;
            if has_generated_marker(&contents) {
                findings.push(AuditFinding::ForbiddenHeader { path: rel });
            }
            Ok(())
        })?;
    }

    findings.sort();
    Ok(findings)
}

/// Walk the live workspace via `git ls-files` and check both directions
/// against the codegen schema. Returns findings; the xtask binary
/// renders them and exits non-zero on any.
///
/// Scanning every tracked `*.rs` path (instead of a hardcoded crate
/// roster) means a hand-editor sneaking a forbidden `@generated` marker
/// into a file anywhere in the repo is caught — the guard is truly
/// repo-wide rather than "inside the crates we thought to list."
pub fn run_audit_generated_headers() -> Result<Vec<AuditFinding>> {
    let root = repo_root()?;
    let emit_paths = live_emit_paths();
    audit_via_git_ls_files(&root, &emit_paths)
}

/// Live-workspace audit using `git ls-files` for scan scope. Returns
/// the same `Vec<AuditFinding>` shape as `audit_with_emit_set` and
/// shares the `MissingHeader` direction for declared emit paths.
pub fn audit_via_git_ls_files(
    workspace_root: &Path,
    emit_paths: &BTreeSet<PathBuf>,
) -> Result<Vec<AuditFinding>> {
    let mut findings = Vec::new();

    // Direction 1: every emit path must have a header.
    for rel in emit_paths {
        let abs = workspace_root.join(rel);
        if !abs.exists() {
            continue;
        }
        let contents = fs::read_to_string(&abs)
            .with_context(|| format!("read emit-path {}", abs.display()))?;
        if !has_generated_marker(&contents) {
            findings.push(AuditFinding::MissingHeader { path: rel.clone() });
        }
    }

    // Direction 2: every tracked `.rs` file outside the emit set that
    // carries the marker is a violation.
    let output = std::process::Command::new("git")
        .current_dir(workspace_root)
        .args(["ls-files", "-z", "*.rs"])
        .output()
        .context("invoke git ls-files")?;
    if !output.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for chunk in output.stdout.split(|byte| *byte == 0) {
        if chunk.is_empty() {
            continue;
        }
        let rel = PathBuf::from(std::str::from_utf8(chunk).context("non-utf8 path")?);
        if emit_paths.contains(&rel) {
            continue;
        }
        let abs = workspace_root.join(&rel);
        if !abs.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&abs)
            .with_context(|| format!("read candidate {}", abs.display()))?;
        if has_generated_marker(&contents) {
            findings.push(AuditFinding::ForbiddenHeader { path: rel });
        }
    }

    findings.sort();
    Ok(findings)
}

/// Relative workspace paths every codegen pass is expected to write.
pub fn live_emit_paths() -> BTreeSet<PathBuf> {
    let mut set = BTreeSet::new();

    // Protocol codegen writes one file per declared handoff protocol.
    let mut compositions: Vec<CompositionSchema> = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    for composition in &compositions {
        for protocol in &composition.handoff_protocols {
            set.insert(PathBuf::from(protocol.rust.module_path.as_str()));
        }
        // Track-B (R5): composition driver codegen writes one
        // descriptor module per declared composition driver. The
        // `module_path` on the driver's Rust binding is the emit
        // target.
        if let Some(driver) = &composition.driver {
            set.insert(PathBuf::from(driver.rust.module_path.as_str()));
        }
    }

    // Standalone terminal surface mapping emitted by protocol codegen.
    set.insert(PathBuf::from(
        "meerkat-core/src/generated/terminal_surface_mapping.rs",
    ));

    set.insert(PathBuf::from(
        "meerkat-machine-kernels/src/generated/mod.rs",
    ));
    for machine in canonical_machine_schemas() {
        set.insert(PathBuf::from(format!(
            "meerkat-machine-kernels/src/generated/{}.rs",
            machine_slug(machine.machine.as_str())
        )));
    }

    set
}

fn default_scan_roots() -> Vec<&'static str> {
    // Limit the walk to crates where we actually emit generated code; this
    // keeps the audit fast and excludes target/, vendored deps, node_modules,
    // etc. Add more as new emit roots appear.
    vec![
        "meerkat-core/src/generated",
        "meerkat-machine-kernels/src/generated",
        "meerkat-mob/src/generated",
        "meerkat-mcp/src/generated",
        "meerkat-runtime/src/generated",
        "meerkat-session/src/generated",
        "meerkat-tools/src/generated",
    ]
}

fn has_generated_marker(contents: &str) -> bool {
    contents
        .lines()
        .take(HEADER_SCAN_LINES)
        .any(|line| line.contains(&generated_marker()))
}

fn visit_rs_files<F>(root: &Path, visitor: &mut F) -> Result<()>
where
    F: FnMut(&Path) -> Result<()>,
{
    let entries = fs::read_dir(root)
        .with_context(|| format!("read_dir {}", root.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("enumerate {}", root.display()))?;
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            visit_rs_files(&path, visitor)?;
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            visitor(&path)?;
        }
    }
    Ok(())
}

/// Render findings as a human-readable error report (for xtask CLI output).
pub fn render_findings(findings: &[AuditFinding]) -> String {
    if findings.is_empty() {
        return String::new();
    }
    let mut buf = String::new();
    buf.push_str("audit-generated-headers: truthfulness violations\n");
    for finding in findings {
        buf.push_str(&format!(
            "  - {} at {}\n",
            finding.kind_label(),
            finding.path().display()
        ));
    }
    buf.push_str(
        "\nEvery `@generated` file must be written by xtask codegen; every codegen-emitted file must carry `@generated`.\n",
    );
    buf
}

fn machine_slug(machine_name: &str) -> String {
    let trimmed = machine_name.strip_suffix("Machine").unwrap_or(machine_name);
    to_snake_case(trimmed)
}

fn to_snake_case(value: &str) -> String {
    let mut out = String::new();
    let mut previous_is_sep = true;

    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() {
                if !previous_is_sep {
                    out.push('_');
                }
                out.push(ch.to_ascii_lowercase());
            } else {
                out.push(ch);
            }
            previous_is_sep = false;
        } else if !previous_is_sep {
            out.push('_');
            previous_is_sep = true;
        }
    }

    out.trim_matches('_').to_string()
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_closure_for_method_calls
)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, contents).expect("write fixture");
    }

    #[test]
    fn clean_workspace_passes() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let mut emit = BTreeSet::new();
        emit.insert(PathBuf::from("meerkat-core/src/generated/good.rs"));
        write(
            root,
            "meerkat-core/src/generated/good.rs",
            "// @generated — test fixture\n",
        );
        let findings = audit_with_emit_set(root, &emit).expect("audit");
        assert!(
            findings.is_empty(),
            "expected no findings, got {findings:?}"
        );
    }

    #[test]
    fn missing_header_on_emit_path_is_flagged() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let mut emit = BTreeSet::new();
        emit.insert(PathBuf::from("meerkat-core/src/generated/oops.rs"));
        write(
            root,
            "meerkat-core/src/generated/oops.rs",
            "// no marker here\n",
        );
        let findings = audit_with_emit_set(root, &emit).expect("audit");
        assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
        assert!(
            matches!(
                &findings[0],
                AuditFinding::MissingHeader { path } if path == Path::new("meerkat-core/src/generated/oops.rs")
            ),
            "expected MissingHeader, got {:?}",
            findings[0]
        );
    }

    #[test]
    fn forbidden_header_on_non_emit_path_is_flagged() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let emit = BTreeSet::new();
        write(
            root,
            "meerkat-core/src/generated/sneaky.rs",
            "// @generated — lying about this\npub fn x() {}\n",
        );
        let findings = audit_with_emit_set(root, &emit).expect("audit");
        assert_eq!(findings.len(), 1, "expected one finding: {findings:?}");
        assert!(
            matches!(
                &findings[0],
                AuditFinding::ForbiddenHeader { path } if path == Path::new("meerkat-core/src/generated/sneaky.rs")
            ),
            "expected ForbiddenHeader, got {:?}",
            findings[0]
        );
    }

    #[test]
    fn marker_check_scans_only_header_window() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let emit = BTreeSet::new();
        // Marker deep in the file (past header window) does NOT count.
        let mut contents = String::new();
        for _ in 0..HEADER_SCAN_LINES + 2 {
            contents.push_str("//\n");
        }
        contents.push_str("// @generated buried past the window\n");
        write(root, "meerkat-core/src/generated/buried.rs", &contents);
        let findings = audit_with_emit_set(root, &emit).expect("audit");
        assert!(
            findings.is_empty(),
            "marker past header window should be ignored: {findings:?}"
        );
    }

    #[test]
    fn both_directions_report_together() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path();
        let mut emit = BTreeSet::new();
        emit.insert(PathBuf::from("meerkat-core/src/generated/missing.rs"));
        write(
            root,
            "meerkat-core/src/generated/missing.rs",
            "// no marker\n",
        );
        write(
            root,
            "meerkat-core/src/generated/forbidden.rs",
            "// @generated — hand-authored liar\n",
        );
        let findings = audit_with_emit_set(root, &emit).expect("audit");
        assert_eq!(findings.len(), 2, "expected both findings: {findings:?}");
        let kinds: BTreeSet<&'static str> = findings.iter().map(|f| f.kind_label()).collect();
        assert!(kinds.contains("missing `@generated` header"));
        assert!(kinds.contains("forbidden `@generated` header"));
    }

    #[test]
    fn render_empty_is_empty_string() {
        assert_eq!(render_findings(&[]), "");
    }

    #[test]
    fn render_lists_every_finding() {
        let findings = vec![
            AuditFinding::MissingHeader {
                path: PathBuf::from("a.rs"),
            },
            AuditFinding::ForbiddenHeader {
                path: PathBuf::from("b.rs"),
            },
        ];
        let rendered = render_findings(&findings);
        assert!(rendered.contains("a.rs"));
        assert!(rendered.contains("b.rs"));
        assert!(rendered.contains("missing `@generated`"));
        assert!(rendered.contains("forbidden `@generated`"));
    }
}
