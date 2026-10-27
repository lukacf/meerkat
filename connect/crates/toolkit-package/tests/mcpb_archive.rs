#![cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]

use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{Cursor, Write};
use std::process::{Command, Stdio};
use toolkit_package::local_artifacts::{PreparationLimits, inspect_zip_selection};
use toolkit_package::{ContentDigest, canonical_json};
use zip::write::SimpleFileOptions;

fn manifest() -> Value {
    json!({
        "manifest_version": "0.3", "name": "fixture", "version": "1.0.0",
        "description": "Public inert source", "author": {"name": "Fixture"},
        "server": {"type": "node", "entry_point": "server/main.js",
            "mcp_config": {"command": "node", "args": ["${__dirname}/server/main.js", "${user_config.label}"]}},
        "user_config": {"label": {"type": "string", "title": "Label", "required": true}}
    })
}

fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        if name.ends_with('/') {
            writer.add_directory(*name, options).unwrap();
        } else {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
    }
    writer.finish().unwrap().into_inner()
}

struct Fixture {
    root: tempfile::TempDir,
    bytes: Vec<u8>,
}

impl Fixture {
    fn new(bytes: Vec<u8>) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("bundle")).unwrap();
        std::fs::write(root.path().join("bundle/artifact.mcpb"), &bytes).unwrap();
        Self { root, bytes }
    }

    fn request(&self, files: Value) -> Value {
        json!({"root": self.root.path(), "directory": "./bundle",
            "artifact_sha256": ContentDigest::of_bytes(&self.bytes), "files": files})
    }

    fn inspect(&self, files: Value) -> (Value, bool) {
        inspect(&serde_json::to_vec(&self.request(files)).unwrap())
    }
}

fn inspect(request: &[u8]) -> (Value, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-package-inspect"))
        .arg("mcpb-archive")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(request).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 256 * 1024);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    (report, output.status.success())
}

fn assert_refused(fixture: &Fixture, files: Value) -> Value {
    let (report, success) = fixture.inspect(files);
    assert!(!success, "{report}");
    assert_eq!(report["status"], "invalid");
    assert!(report.get("files").is_none());
    assert!(report.get("inventory").is_none());
    report
}

#[test]
fn real_cli_returns_exact_public_source_without_configuration_or_execution() {
    let raw_manifest = serde_json::to_vec_pretty(&manifest()).unwrap();
    let script = b"require('fs').writeFileSync('execution-marker', 'unexpected');\n";
    let fixture = Fixture::new(archive(&[
        ("manifest.json", &raw_manifest),
        ("server/main.js", script),
        ("README.md", b"Public guidance\r\n"),
        ("unselected.bin", &[0, 255, 1]),
    ]));
    let before = std::fs::read(fixture.root.path().join("bundle/artifact.mcpb")).unwrap();
    let (report, success) = fixture.inspect(json!(["./README.md", "./server/main.js"]));
    assert!(success, "{report}");
    assert_eq!(report["scope"], "metadata_only");
    assert_eq!(report["format"], "mcpb-archive");
    assert_eq!(report["artifact_byte_length"], before.len());
    assert_eq!(
        report["artifact_sha256"],
        json!(ContentDigest::of_bytes(&before))
    );
    assert_eq!(report["archive_entry_count"], 4);
    assert_eq!(report["inventory_entry_count"], 5);
    assert_eq!(report["inventory_truncated"], false);
    assert_eq!(report["files"][0]["path"], "./manifest.json");
    assert_eq!(
        report["files"][0]["text"].as_str().unwrap().as_bytes(),
        raw_manifest
    );
    assert_eq!(report["files"][1]["text"], "Public guidance\r\n");
    assert_eq!(
        report["files"][2]["text"].as_str().unwrap().as_bytes(),
        script
    );
    assert_eq!(
        report["files"][0]["inspection"]["summary"]["declarations"]["args"],
        manifest()["server"]["mcp_config"]["args"]
    );
    assert_eq!(
        report["files"][0]["inspection"]["summary"]["user_configuration"][0]["required"],
        true
    );
    assert_eq!(
        report["source_digest"],
        json!(ContentDigest::of_bytes(
            &canonical_json(&report["files"]).unwrap()
        ))
    );
    assert_eq!(
        before,
        std::fs::read(fixture.root.path().join("bundle/artifact.mcpb")).unwrap()
    );
    assert_eq!(
        std::fs::read_dir(fixture.root.path().join("bundle"))
            .unwrap()
            .count(),
        1
    );
    assert!(!fixture.root.path().join("execution-marker").exists());
    for key in [
        "ready",
        "activation",
        "launch",
        "authorized",
        "installation",
    ] {
        assert!(report.get(key).is_none());
    }
}

#[test]
fn manifest_is_automatic_and_explicit_selection_normalizes_to_same_report() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let fixture = Fixture::new(archive(&[("manifest.json", &raw), ("README.md", b"guide")]));
    assert_eq!(
        fixture.inspect(json!([])),
        fixture.inspect(json!(["./manifest.json"]))
    );
    assert_eq!(
        fixture.inspect(json!(["./README.md"])),
        fixture.inspect(json!(["./README.md", "./manifest.json"]))
    );
    for selection in [
        json!(["./manifest.json", "./manifest.json"]),
        json!(["./README.md", "./README.md"]),
    ] {
        assert_refused(&fixture, selection);
    }
}

#[test]
fn stored_and_deflated_archives_preserve_the_same_selected_source_evidence() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let stored = Fixture::new(archive(&[("manifest.json", &raw)]));
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            "manifest.json",
            SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .unix_permissions(0o644),
        )
        .unwrap();
    writer.write_all(&raw).unwrap();
    let deflated = Fixture::new(writer.finish().unwrap().into_inner());
    let (first, first_ok) = stored.inspect(json!([]));
    let (second, second_ok) = deflated.inspect(json!([]));
    assert!(first_ok && second_ok);
    assert_ne!(first["artifact_sha256"], second["artifact_sha256"]);
    assert_eq!(first["files"], second["files"]);
    assert_eq!(first["source_digest"], second["source_digest"]);
}

#[test]
fn sensitive_and_unsupported_configuration_remains_source_evidence() {
    let mut source = manifest();
    source["server"]["type"] = json!("python");
    source["user_config"]["token"] =
        json!({"type":"string", "title":"Token", "required":true, "sensitive":true});
    source["user_config"]["folder"] =
        json!({"type":"directory", "title":"Folder", "required":true});
    let fixture = Fixture::new(archive(&[(
        "manifest.json",
        &serde_json::to_vec(&source).unwrap(),
    )]));
    let (report, success) = fixture.inspect(json!([]));
    assert!(success, "{report}");
    let summary = &report["files"][0]["inspection"]["summary"];
    assert_eq!(summary["declarations"]["runtime"], "python");
    assert!(
        summary["user_configuration"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["name"] == "token" && item["sensitive"] == true)
    );
    assert!(
        summary["unresolved_contracts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item == "authentication_and_authority")
    );
    source["user_config"]["token"]["default"] = json!("DO-NOT-PRINT-SECRET");
    let fixture = Fixture::new(archive(&[(
        "manifest.json",
        &serde_json::to_vec(&source).unwrap(),
    )]));
    assert!(
        !assert_refused(&fixture, json!([]))
            .to_string()
            .contains("DO-NOT-PRINT-SECRET")
    );
}

#[test]
fn wrong_hash_missing_manifest_invalid_json_and_duplicate_json_keys_refuse() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let fixture = Fixture::new(archive(&[("manifest.json", &raw)]));
    let mut request = fixture.request(json!([]));
    request["artifact_sha256"] = json!("0".repeat(64));
    assert!(!inspect(&serde_json::to_vec(&request).unwrap()).1);
    request["artifact_sha256"] = json!("NOT-A-HASH");
    assert!(!inspect(&serde_json::to_vec(&request).unwrap()).1);
    let duplicate = format!(
        "{{\"files\":[],{}",
        String::from_utf8(serde_json::to_vec(&fixture.request(json!([]))).unwrap())
            .unwrap()
            .strip_prefix('{')
            .unwrap()
    );
    assert!(!inspect(duplicate.as_bytes()).1);
    assert_refused(
        &Fixture::new(archive(&[("README.md", b"missing manifest")])),
        json!([]),
    );
    for raw in [
        b"{PRIVATE-INVALID}".as_slice(),
        b"{\"name\":\"a\",\"name\":\"b\"}",
    ] {
        assert!(
            !assert_refused(&Fixture::new(archive(&[("manifest.json", raw)])), json!([]))
                .to_string()
                .contains("PRIVATE")
        );
    }
}

#[test]
fn selection_paths_text_limits_and_aggregate_limits_are_enforced() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let mut entries = vec![("manifest.json", raw.as_slice())];
    let payload = vec![b'x'; 32 * 1024];
    for name in ["a", "b", "c", "d"] {
        entries.push((name, payload.as_slice()));
    }
    let fixture = Fixture::new(archive(&entries));
    for selection in [
        json!(["../a"]),
        json!(["./../a"]),
        json!(["/a"]),
        json!(["./a//b"]),
        json!(["./a/./b"]),
        json!(["./a\\b"]),
        json!(["./absent"]),
        json!(["./a", "./b", "./c", "./d"]),
    ] {
        assert_refused(&fixture, selection);
    }
    assert!(fixture.inspect(json!(["./a"])).1);
    assert_refused(
        &fixture,
        json!((0..16).map(|i| format!("./file-{i}")).collect::<Vec<_>>()),
    );
    for payload in [vec![0xff], vec![0], vec![27], vec![b'x'; 32 * 1024 + 1]] {
        let fixture = Fixture::new(archive(&[("manifest.json", &raw), ("a", &payload)]));
        assert_refused(&fixture, json!(["./a"]));
    }
    let fixture = Fixture::new(archive(&[("manifest.json", &vec![b' '; 32 * 1024 + 1])]));
    assert_refused(&fixture, json!([]));
}

#[test]
fn inventory_is_bounded_sorted_and_does_not_hide_selected_members() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let names = (0..140)
        .map(|i| format!("docs/{i:03}.md"))
        .collect::<Vec<_>>();
    let mut entries = vec![("manifest.json", raw.as_slice())];
    for name in &names {
        entries.push((name.as_str(), b"guide"));
    }
    let fixture = Fixture::new(archive(&entries));
    let (report, success) = fixture.inspect(json!(["./docs/139.md"]));
    assert!(success, "{report}");
    assert_eq!(report["archive_entry_count"], 141);
    assert_eq!(report["inventory_entry_count"], 142);
    assert_eq!(report["inventory"].as_array().unwrap().len(), 128);
    assert_eq!(report["inventory_truncated"], true);
    let paths = report["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["path"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(paths.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(report["files"][1]["path"], "./docs/139.md");
}

#[test]
fn the_full_archive_is_validated_even_when_only_manifest_is_requested() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    for name in [
        "../escape",
        "/absolute",
        "nested/../escape",
        "back\\slash",
        "CON",
    ] {
        let fixture = Fixture::new(archive(&[("manifest.json", &raw), (name, b"inert")]));
        assert_refused(&fixture, json!([]));
    }
    let fixture = Fixture::new(archive(&[
        ("manifest.json", &raw),
        ("same", b"a"),
        ("SAME", b"b"),
    ]));
    assert_refused(&fixture, json!([]));
    let mut bytes = archive(&[("manifest.json", &raw), ("unselected", b"TRIPWIRE-BYTES")]);
    let offset = bytes
        .windows(14)
        .position(|part| part == b"TRIPWIRE-BYTES")
        .unwrap();
    bytes[offset] ^= 1;
    assert_refused(&Fixture::new(bytes), json!([]));
}

#[test]
fn unselected_duplicate_symlink_descriptor_and_zip64_members_refuse() {
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let base = archive(&[
        ("manifest.json", &raw),
        ("one.txt", b"a"),
        ("two.txt", b"b"),
    ]);
    let mut duplicate = base.clone();
    for index in (0..duplicate.len() - 7).collect::<Vec<_>>() {
        if &duplicate[index..index + 7] == b"two.txt" {
            duplicate[index..index + 7].copy_from_slice(b"one.txt");
        }
    }
    assert_refused(&Fixture::new(duplicate), json!([]));
    let central = base
        .windows(4)
        .enumerate()
        .filter_map(|(i, part)| (part == b"PK\x01\x02").then_some(i))
        .collect::<Vec<_>>();
    let mut symlink = base.clone();
    symlink[central[1] + 38..central[1] + 42].copy_from_slice(&(0o120777_u32 << 16).to_le_bytes());
    assert_refused(&Fixture::new(symlink), json!([]));
    let mut descriptor = base.clone();
    descriptor[central[1] + 8..central[1] + 10].copy_from_slice(&8_u16.to_le_bytes());
    assert_refused(&Fixture::new(descriptor), json!([]));
    let mut zip64 = base;
    zip64[central[1] + 24..central[1] + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_refused(&Fixture::new(zip64), json!([]));
}

#[test]
fn anchored_child_and_artifact_symlink_escape_refuse_without_echo() {
    use std::os::unix::fs::symlink;
    let raw = serde_json::to_vec(&manifest()).unwrap();
    let outside = Fixture::new(archive(&[("manifest.json", &raw)]));
    let inside = Fixture::new(outside.bytes.clone());
    std::fs::remove_file(inside.root.path().join("bundle/artifact.mcpb")).unwrap();
    symlink(
        outside.root.path().join("bundle/artifact.mcpb"),
        inside.root.path().join("bundle/artifact.mcpb"),
    )
    .unwrap();
    assert_refused(&inside, json!([]));
    std::fs::remove_file(inside.root.path().join("bundle/artifact.mcpb")).unwrap();
    std::fs::remove_dir(inside.root.path().join("bundle")).unwrap();
    symlink(
        outside.root.path().join("bundle"),
        inside.root.path().join("bundle"),
    )
    .unwrap();
    assert_refused(&inside, json!([]));
    let mut request = outside.request(json!([]));
    for directory in ["bundle", "./..", "./bundle/../bundle", "/bundle"] {
        request["directory"] = json!(directory);
        assert!(!inspect(&serde_json::to_vec(&request).unwrap()).1);
    }
    request = outside.request(json!([]));
    request["root"] = json!("relative-root");
    assert!(!inspect(&serde_json::to_vec(&request).unwrap()).1);
}

#[test]
fn archive_owner_applies_raw_expanded_entry_and_metadata_bounds() {
    let bytes = archive(&[("a", b"12345"), ("b", b"12345")]);
    let paths = BTreeSet::from(["a".to_string()]);
    let defaults = PreparationLimits::default();
    assert!(inspect_zip_selection(&bytes, &paths, &defaults, 5, 5).is_ok());
    for limits in [
        PreparationLimits {
            maximum_raw_bytes: bytes.len() as u64 - 1,
            ..defaults.clone()
        },
        PreparationLimits {
            maximum_file_bytes: 4,
            ..defaults.clone()
        },
        PreparationLimits {
            maximum_expanded_bytes: 9,
            ..defaults.clone()
        },
        PreparationLimits {
            maximum_entries: 1,
            ..defaults.clone()
        },
        PreparationLimits {
            maximum_archive_metadata_bytes: 1,
            ..defaults.clone()
        },
    ] {
        assert!(inspect_zip_selection(&bytes, &paths, &limits, 5, 5).is_err());
    }
    assert!(inspect_zip_selection(&bytes, &paths, &defaults, 4, 5).is_err());
    assert!(inspect_zip_selection(&bytes, &paths, &defaults, 5, 4).is_err());
}
