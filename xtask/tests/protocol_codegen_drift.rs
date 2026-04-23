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
    let mut child = Command::new("rustfmt")
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
        compat_composition_schemas, external_tool_surface_bridge_machine, flow_frame_machine,
        flow_run_machine, loop_iteration_machine, ops_barrier_bridge_machine,
    };

    let root = repo_root();
    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    let mut machines = canonical_machine_schemas();
    machines.extend([
        flow_frame_machine(),
        flow_run_machine(),
        loop_iteration_machine(),
        ops_barrier_bridge_machine(),
        external_tool_surface_bridge_machine(),
    ]);
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
        .find(|m| m.machine == "MeerkatMachine")
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
