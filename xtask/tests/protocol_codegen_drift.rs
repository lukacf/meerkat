//! Drift test: regenerate each declared protocol and assert the emitted
//! source is bit-for-bit identical to the committed file.
//!
//! Complements `make machine-check-drift` for protocol codegen. If a
//! future change modifies the codegen without regenerating the on-disk
//! protocol files (or vice versa), this test fails with a precise
//! file + diff preview.

#![allow(clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .expect("run git rev-parse");
    let path = String::from_utf8(out.stdout).expect("git output utf8");
    PathBuf::from(path.trim())
}

fn rustfmt(source: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let rustfmt = std::env::var_os("RUSTFMT")
        .map(PathBuf::from)
        .and_then(|path| path.canonicalize().ok().or(Some(path)))
        .unwrap_or_else(|| PathBuf::from("rustfmt"));
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rustfmt");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(source.as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "rustfmt failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8")
}

/// Normalize line endings so the test is stable for contributors whose
/// `core.autocrlf = true` git config check out files with `\r\n`.
/// Rendered output always uses LF; committed files may have either.
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

/// For every declared protocol in canonical + compat composition
/// catalogs, regenerate the source and compare byte-for-byte against
/// the committed on-disk file.
#[test]
fn every_declared_protocol_file_matches_codegen_output() {
    use meerkat_machine_schema::{
        MachineSchema, canonical_composition_schemas, canonical_machine_schemas,
        compat_composition_schemas,
    };

    let root = repo_root();
    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    let machines = canonical_machine_schemas();
    let machine_by_name: std::collections::BTreeMap<&str, &MachineSchema> =
        machines.iter().map(|m| (m.machine.as_str(), m)).collect();

    let mut checked = 0;
    for composition in &compositions {
        for protocol in &composition.handoff_protocols {
            let committed_path = root.join(&protocol.rust.module_path);
            let committed = std::fs::read_to_string(&committed_path)
                .unwrap_or_else(|_| panic!("read {}", committed_path.display()));

            // Re-invoke the codegen render path — match what xtask does
            // in-process rather than shelling out, so the assertion is
            // about the code, not the CLI.
            let producer = composition
                .machines
                .iter()
                .find(|m| m.instance_id == protocol.producer_instance)
                .and_then(|inst| machine_by_name.get(inst.machine_name.as_str()).copied())
                .unwrap_or_else(|| {
                    panic!(
                        "protocol {} missing producer machine in registry",
                        protocol.name
                    )
                });
            let rendered = xtask::protocol_codegen::render_protocol_helpers(
                protocol,
                producer,
                composition,
                &machine_by_name,
            )
            .unwrap_or_else(|err| panic!("render failed for protocol {}: {err}", protocol.name));
            let rendered = rustfmt(&rendered);

            assert_eq!(
                normalize(&committed),
                normalize(&rendered),
                "protocol `{}` ({}) diverged from codegen output.\nIf this is intentional, run `cargo xtask protocol-codegen` and commit the result.",
                protocol.name,
                protocol.rust.module_path
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no protocols declared in canonical + compat catalogs — test is vacuous"
    );
}

/// Drift guard for the standalone `terminal_surface_mapping.rs` file.
/// This file is emitted by the protocol-codegen pass outside the
/// handoff_protocols loop, so the per-protocol drift test above does
/// not cover it. Add this separate test so a hand-edit of the file is
/// caught by the same gate.
#[test]
fn terminal_surface_mapping_matches_codegen_output() {
    use meerkat_machine_schema::{MachineSchema, canonical_machine_schemas};

    let machines: Vec<MachineSchema> = canonical_machine_schemas();
    let meerkat_machine = machines
        .iter()
        .find(|m| m.machine.as_str() == "MeerkatMachine")
        .expect("MeerkatMachine must be a canonical schema");

    let rendered = xtask::protocol_codegen::render_terminal_surface_mapping(meerkat_machine)
        .expect("render terminal_surface_mapping");
    let rendered = rustfmt(&rendered);

    let committed_path = repo_root().join("meerkat-core/src/generated/terminal_surface_mapping.rs");
    let committed = std::fs::read_to_string(&committed_path)
        .unwrap_or_else(|_| panic!("read {}", committed_path.display()));

    assert_eq!(
        normalize(&committed),
        normalize(&rendered),
        "terminal_surface_mapping.rs diverged from codegen output. If this is intentional, run `cargo xtask protocol-codegen` and commit the result."
    );
}

#[test]
fn comms_trust_authority_sources_matches_codegen_output() {
    use meerkat_machine_schema::{canonical_composition_schemas, compat_composition_schemas};

    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    let rendered = xtask::protocol_codegen::render_comms_trust_authority_sources(&compositions)
        .expect("render comms_trust_authority_sources");
    let rendered = rustfmt(&rendered);

    let committed_path =
        repo_root().join("meerkat-core/src/generated/comms_trust_authority_sources.rs");
    let committed = std::fs::read_to_string(&committed_path)
        .unwrap_or_else(|_| panic!("read {}", committed_path.display()));

    assert_eq!(
        normalize(&committed),
        normalize(&rendered),
        "comms_trust_authority_sources.rs diverged from codegen output. If this is intentional, run `cargo xtask protocol-codegen` and commit the result."
    );
}

#[test]
fn auth_lease_transition_authority_sources_matches_codegen_output() {
    use meerkat_machine_schema::{canonical_composition_schemas, compat_composition_schemas};

    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    let rendered =
        xtask::protocol_codegen::render_auth_lease_transition_authority_sources(&compositions)
            .expect("render auth_lease_transition_authority_sources");
    let rendered = rustfmt(&rendered);

    let committed_path =
        repo_root().join("meerkat-core/src/generated/auth_lease_transition_authority_sources.rs");
    let committed = std::fs::read_to_string(&committed_path)
        .unwrap_or_else(|_| panic!("read {}", committed_path.display()));

    assert_eq!(
        normalize(&committed),
        normalize(&rendered),
        "auth_lease_transition_authority_sources.rs diverged from codegen output. If this is intentional, run `cargo xtask protocol-codegen` and commit the result."
    );
}

/// Compile canary for generated protocol helper ownership: every helper
/// emitted by protocol-codegen must land in an owning crate's checked
/// `src/generated/` module tree, not in an ad-hoc bridge path outside a
/// package. This does not invoke cargo; it verifies the ownership boundary
/// that cargo will later compile.
#[test]
fn every_protocol_helper_lands_under_an_owning_crate_generated_module() {
    use meerkat_machine_schema::{canonical_composition_schemas, compat_composition_schemas};

    let root = repo_root();
    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());

    let mut checked = 0;
    for composition in &compositions {
        for protocol in &composition.handoff_protocols {
            let module_path = std::path::Path::new(&protocol.rust.module_path);
            let components: Vec<_> = module_path
                .components()
                .map(|component| component.as_os_str().to_string_lossy().to_string())
                .collect();

            assert!(
                components.len() >= 4
                    && components[1] == "src"
                    && components[2] == "generated"
                    && components
                        .last()
                        .is_some_and(|file| file.starts_with("protocol_") && file.ends_with(".rs")),
                "protocol `{}` helper path `{}` must be <owning-crate>/src/generated/protocol_*.rs",
                protocol.name,
                protocol.rust.module_path
            );

            let crate_manifest = root.join(&components[0]).join("Cargo.toml");
            assert!(
                crate_manifest.exists(),
                "protocol `{}` helper path `{}` must belong to a crate with Cargo.toml",
                protocol.name,
                protocol.rust.module_path
            );
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "no protocols declared in canonical + compat catalogs — test is vacuous"
    );
}

#[test]
fn comms_trust_authority_minting_is_generated_only() {
    fn visit_rs_files(root: &std::path::Path, files: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(root)
            .unwrap_or_else(|err| panic!("read_dir {}: {err}", root.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|err| panic!("read_dir entry: {err}"));
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || name == "target" {
                continue;
            }
            if path.is_dir() {
                visit_rs_files(&path, files);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                files.push(path);
            }
        }
    }

    let root = repo_root();
    let mut files = Vec::new();
    visit_rs_files(&root, &mut files);

    let patterns = [
        concat!("from_generated_", "public_"),
        concat!("from_generated_", "private_"),
        concat!("trait GeneratedCommsTrust", "AuthoritySource"),
        concat!(
            "impl meerkat_core::comms::GeneratedCommsTrust",
            "AuthoritySource"
        ),
        concat!("generated_comms_trust_", "authority::Sealed"),
        concat!("GeneratedCommsTrust", "AuthorityRequest"),
        concat!("GeneratedCommsTrust", "AuthorityGrant"),
        concat!("trait GeneratedCommsTrust", "AuthorityParts"),
        concat!("trait GeneratedAuthLease", "TransitionParts"),
        concat!("pub unsafe fn from_generated_", "authority_parts"),
        concat!("pub unsafe fn from_generated_", "auth_lease_publication("),
        concat!("__meerkat_core_generated_", "comms_trust_authority"),
        concat!("__meerkat_core_generated_", "auth_lease_transition"),
        concat!("generated::comms_trust_", "authority_sources::"),
        concat!(
            "auth_lease_transition_authority_sources::",
            "auth_lease_lifecycle_publication_transition"
        ),
    ];
    let mut violations = Vec::new();
    let mut raw_constructor_violations = Vec::new();
    let mut generated_parts_impl_violations = Vec::new();
    let mut generated_bridge_violations = Vec::new();
    for path in files {
        let relative = path
            .strip_prefix(&root)
            .unwrap_or_else(|_| panic!("strip repo root from {}", path.display()))
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {}", path.display()));
        if patterns.iter().any(|pattern| source.contains(pattern)) {
            violations.push(relative.clone());
        }
        if source.contains(concat!(
            "CommsTrustMutationAuthority::",
            "from_generated_parts"
        )) && relative != "meerkat-core/src/comms.rs"
        {
            raw_constructor_violations.push(relative.clone());
        }
        if source.contains(concat!(
            "AuthLeaseTransition::",
            "from_generated_auth_lease_publication_parts"
        )) && relative != "meerkat-core/src/handles.rs"
        {
            raw_constructor_violations.push(relative.clone());
        }
        let generated_protocol_file = relative.contains("/src/generated/protocol_")
            || relative == "xtask/src/protocol_codegen.rs";
        let codegen_file = relative == "xtask/src/protocol_codegen.rs";
        let drift_test_file = relative == "xtask/tests/protocol_codegen_drift.rs";
        let core_bridge_file =
            relative == "meerkat-core/src/comms.rs" || relative == "meerkat-core/src/handles.rs";
        if (source.contains(concat!(
            "impl meerkat_core::comms::",
            "GeneratedCommsTrustAuthorityParts"
        )) || source.contains(concat!(
            "impl meerkat_core::handles::",
            "GeneratedAuthLeaseTransitionParts"
        ))) && !generated_protocol_file
        {
            generated_parts_impl_violations.push(relative.clone());
        }
        if source.contains("generated_authority_bridge_token()")
            && !(generated_protocol_file || codegen_file || drift_test_file)
        {
            generated_bridge_violations.push(relative.clone());
        }
        if (source.contains("__meerkat_core_runtime_generated_")
            || source.contains("__meerkat_core_mob_generated_")
            || source.contains("__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_")
            || source.contains("__meerkat_mob_generated_authority_bridge_token_is_valid_v1_"))
            && !(generated_protocol_file || codegen_file || core_bridge_file || drift_test_file)
        {
            generated_bridge_violations.push(relative.clone());
        }
        if (source.contains("core_generated_comms_trust_authority_build")
            || source.contains("core_runtime_generated_auth_lease_transition_build"))
            && !(generated_protocol_file || codegen_file || core_bridge_file || drift_test_file)
        {
            generated_bridge_violations.push(relative);
        }
    }

    assert!(
        violations.is_empty(),
        "generated authority must not reintroduce public source traits, grants, requests, legacy public constructors, or raw public generated-module mint helpers; found {violations:?}",
    );
    assert!(
        raw_constructor_violations.is_empty(),
        "raw generated authority field constructors must stay private to core; found direct external calls in {raw_constructor_violations:?}",
    );
    assert!(
        generated_parts_impl_violations.is_empty(),
        "generated authority parts impls must be emitted only by protocol codegen into src/generated/protocol_* helpers; found {generated_parts_impl_violations:?}",
    );
    generated_bridge_violations.sort();
    generated_bridge_violations.dedup();
    assert!(
        generated_bridge_violations.is_empty(),
        "generated authority bridge tokens and hidden bridge calls must stay inside generated protocol helpers, xtask codegen, or core validators; found {generated_bridge_violations:?}",
    );
}
