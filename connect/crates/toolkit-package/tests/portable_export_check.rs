#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use toolkit_package::{
    AGENT_PLUGIN_SCHEMA, ContentDigest, ImportedPluginManifest, PluginRoot,
    PortableExportCheckState as State, PortableExportFingerprint, PortablePluginExport,
};

fn export() -> PortablePluginExport {
    let document = serde_json::to_vec(&serde_json::json!({
        "$schema": AGENT_PLUGIN_SCHEMA, "name": "inspection-fixture"
    }))
    .unwrap();
    let manifest = ImportedPluginManifest::parse(&document).unwrap();
    PortablePluginExport::new(&manifest.manifest, &BTreeMap::new(), None).unwrap()
}

#[test]
fn inspection_reports_current_missing_matching_and_changed_bytes_without_writes() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("export");
    let export = export();
    let missing = export.inspect_existing(&destination);
    assert_eq!(missing.state, State::Missing);
    assert_eq!(missing.files["plugin.json"].state, State::Missing);
    assert!(missing.inventory.complete);
    assert!(!destination.exists());

    export.write_new(&destination).unwrap();
    let original = fs::read(destination.join("plugin.json")).unwrap();
    let matched = export.inspect_existing(&destination);
    assert_eq!(matched.state, State::Matches);
    assert_eq!(matched.inventory.state, State::Matches);
    assert_eq!(matched.inventory.unexpected_entries, 0);
    assert_eq!(
        matched.files["plugin.json"].expected,
        matched.files["plugin.json"].observed.clone().unwrap()
    );
    assert_eq!(fs::read(destination.join("plugin.json")).unwrap(), original);

    let altered = b"different current bytes";
    fs::write(destination.join("plugin.json"), altered).unwrap();
    let changed = export.inspect_existing(&destination);
    assert_eq!(changed.state, State::Mismatch);
    assert_eq!(
        changed.files["plugin.json"]
            .observed
            .as_ref()
            .unwrap()
            .sha256,
        ContentDigest::of_bytes(altered)
    );
    assert_eq!(fs::read(destination.join("plugin.json")).unwrap(), altered);
    fs::remove_file(destination.join("plugin.json")).unwrap();
    assert_eq!(export.inspect_existing(&destination).state, State::Missing);
}

#[test]
fn inspection_detects_executable_mode_drift_with_identical_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("server"), b"inert executable fixture\n").unwrap();
    fs::set_permissions(source.join("server"), fs::Permissions::from_mode(0o700)).unwrap();
    let export = export()
        .with_file(&PluginRoot::open(&source).unwrap(), "./server")
        .unwrap();
    let destination = temp.path().join("export");
    export.write_new(&destination).unwrap();
    assert_eq!(export.inspect_existing(&destination).state, State::Matches);
    let original = fs::read(destination.join("server")).unwrap();
    fs::set_permissions(
        destination.join("server"),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    let check = export.inspect_existing(&destination);
    assert_eq!(check.state, State::Mismatch);
    assert_eq!(check.files["server"].state, State::Mismatch);
    assert_eq!(
        check.files["server"].expected.sha256,
        check.files["server"].observed.as_ref().unwrap().sha256
    );
    assert_eq!(check.files["server"].expected.executable, Some(true));
    assert_eq!(
        check.files["server"].observed.as_ref().unwrap().executable,
        Some(false)
    );
    assert_eq!(fs::read(destination.join("server")).unwrap(), original);
    assert_eq!(
        fs::metadata(destination.join("server"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );

    fs::set_permissions(
        destination.join("server"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert_eq!(export.inspect_existing(&destination).state, State::Matches);
    fs::set_permissions(
        destination.join("plugin.json"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert_eq!(
        export.inspect_existing(&destination).files["plugin.json"].state,
        State::Mismatch
    );
}

#[test]
fn legacy_fingerprint_does_not_invent_an_executable_observation() {
    let legacy = serde_json::json!({
        "sha256": ContentDigest::of_bytes(b"legacy observation"), "bytes": 18,
    });
    let imported: PortableExportFingerprint = serde_json::from_value(legacy.clone()).unwrap();
    assert_eq!(imported.executable, None);
    assert_eq!(serde_json::to_value(&imported).unwrap(), legacy);
    for executable in [false, true] {
        let mut current = legacy.clone();
        current["executable"] = serde_json::json!(executable);
        let imported: PortableExportFingerprint = serde_json::from_value(current.clone()).unwrap();
        assert_eq!(imported.executable, Some(executable));
        assert_eq!(serde_json::to_value(&imported).unwrap(), current);
    }
}

#[test]
fn nested_expected_inventory_is_checked_without_echoing_or_traversing_unknown_names() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir_all(source.join("skills/setup")).unwrap();
    fs::write(source.join("skills/setup/SKILL.md"), "inert setup text").unwrap();
    let export = export()
        .with_file(
            &PluginRoot::open(source).unwrap(),
            "./skills/setup/SKILL.md",
        )
        .unwrap();
    let destination = temp.path().join("export");
    export.write_new(&destination).unwrap();
    assert_eq!(export.inspect_existing(&destination).state, State::Matches);
    fs::create_dir(destination.join("private-unexpected-name")).unwrap();
    symlink(
        "/unavailable-fixture",
        destination.join("private-unexpected-name/ignored-link"),
    )
    .unwrap();
    let check = export.inspect_existing(&destination);
    assert_eq!(check.state, State::Mismatch);
    assert_eq!(check.inventory.unexpected_entries, 1);
    assert!(check.inventory.complete);
    assert!(
        !serde_json::to_string(&check)
            .unwrap()
            .contains("private-unexpected-name")
    );
    fs::remove_dir_all(destination.join("private-unexpected-name")).unwrap();
    fs::remove_dir_all(destination.join("skills")).unwrap();
    symlink(temp.path(), destination.join("skills")).unwrap();
    assert_eq!(
        export.inspect_existing(&destination).state,
        State::Unavailable
    );
}

#[test]
fn root_and_member_symlinks_or_nonregular_files_are_unavailable() {
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join("real");
    let export = export();
    export.write_new(&real).unwrap();
    let link = temp.path().join("link");
    symlink(&real, &link).unwrap();
    for suffix in ["", "/", "/.", "//"] {
        let mut alias = link.as_os_str().to_owned();
        alias.push(suffix);
        let check = export.inspect_existing(std::path::Path::new(&alias));
        assert_eq!(check.state, State::Unavailable);
        assert_eq!(check.files.len(), 1);
        assert!(!check.inventory.complete);
    }
    for suffix in ["/", "/.", "//", "/../real"] {
        let mut alias = real.as_os_str().to_owned();
        alias.push(suffix);
        assert_eq!(
            export.inspect_existing(std::path::Path::new(&alias)).state,
            State::Unavailable
        );
    }
    assert_eq!(
        export.inspect_existing(real.join("plugin.json")).state,
        State::Unavailable
    );
    let original = fs::read(real.join("plugin.json")).unwrap();
    fs::rename(real.join("plugin.json"), real.join("saved.json")).unwrap();
    symlink("saved.json", real.join("plugin.json")).unwrap();
    let check = export.inspect_existing(&real);
    assert_eq!(check.state, State::Unavailable);
    assert!(check.files["plugin.json"].observed.is_none());
    assert_eq!(fs::read(real.join("saved.json")).unwrap(), original);
    fs::remove_file(real.join("plugin.json")).unwrap();
    fs::create_dir(real.join("plugin.json")).unwrap();
    assert_eq!(export.inspect_existing(real).state, State::Unavailable);
}

#[test]
fn oversized_file_and_excessive_inventory_are_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let destination = temp.path().join("export");
    let export = export();
    export.write_new(&destination).unwrap();
    fs::write(destination.join("plugin.json"), vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let oversized = export.inspect_existing(&destination);
    assert_eq!(oversized.state, State::Mismatch);
    assert!(oversized.files["plugin.json"].observed.is_none());
    for index in 0..513 {
        fs::write(destination.join(format!("extra-{index}")), []).unwrap();
    }
    let excessive = export.inspect_existing(&destination);
    assert_eq!(excessive.state, State::Unavailable);
    assert!(!excessive.inventory.complete);
    assert_eq!(excessive.files.len(), 1);
}

#[test]
fn aggregate_read_budget_does_not_claim_unread_files_match() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let mut export = export();
    let root = PluginRoot::open(&source).unwrap();
    for index in 0..9 {
        let name = format!("large-{index}");
        fs::write(source.join(&name), vec![b'x'; 1024 * 1024]).unwrap();
        export = export.with_file(&root, &format!("./{name}")).unwrap();
    }
    let destination = temp.path().join("export");
    export.write_new(&destination).unwrap();
    let check = export.inspect_existing(&destination);
    assert_eq!(check.state, State::Unavailable);
    assert_eq!(check.inventory.state, State::Matches);
    assert!(
        check
            .files
            .values()
            .any(|file| file.state == State::Unavailable)
    );
    assert!(
        check
            .files
            .values()
            .any(|file| file.state == State::Matches)
    );
}

#[test]
fn repeated_dot_prefix_cannot_replace_canonical_document_expectations() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("plugin.json"), b"aliased inert bytes").unwrap();
    let export = export()
        .with_file(&PluginRoot::open(&source).unwrap(), "././plugin.json")
        .unwrap();
    let destination = temp.path().join("export");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("plugin.json"), b"aliased inert bytes").unwrap();
    let check = export.inspect_existing(&destination);
    assert_eq!(check.state, State::Unavailable);
    assert_eq!(
        check.files["plugin.json"].expected.sha256,
        ContentDigest::of_bytes(export.plugin_json())
    );
    assert!(check.files["plugin.json"].observed.is_none());
    assert!(!check.inventory.complete);
}
