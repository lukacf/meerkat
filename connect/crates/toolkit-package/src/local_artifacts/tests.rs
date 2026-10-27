use super::*;
use crate::*;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use url::Url;
use zip::write::SimpleFileOptions;

// Exercise actual preparer-process death without killing the test owner. The
// independently held root and selection represent its pre-recorded intent.
#[test]
fn killed_preparer_is_recovered_from_the_original_attempt_root() {
    use std::io::BufRead;
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    const CHILD_ROOT: &str = "TOOLKIT_TEST_PREPARER_CRASH_ROOT";
    const CHILD_POINT: &str = "TOOLKIT_TEST_PREPARER_CRASH_POINT";
    const READY: &str = "toolkit-preparer-crash-boundary-ready";
    const PAYLOAD: &[u8] = b"selected process-crash payload";

    fn point_name(point: Point) -> &'static str {
        match point {
            Point::Frozen => "frozen",
            Point::Materialized => "materialized",
            Point::BeforePublish => "before-publish",
            Point::AfterPublish => "after-publish",
        }
    }

    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let expected = std::env::var(CHILD_POINT).expect("child checkpoint");
        let preparer = LocalArtifactPreparer::new(
            cap_std::fs::Dir::open_ambient_dir(root, cap_std::ambient_authority())
                .expect("host-recorded child root"),
        );
        let mut artifact = tempfile::tempfile().expect("child input file");
        artifact.write_all(PAYLOAD).expect("child input bytes");
        let _ = preparer
            .prepare_with(
                &selection_for(PAYLOAD, "server"),
                &id("component"),
                LocalArtifactLayout::SingleFile { executable: false },
                LocalArtifactInputs {
                    artifact,
                    pinned_dependencies: BTreeMap::new(),
                },
                &PreparationLimits::default(),
                &mut |point| {
                    if point_name(point) == expected {
                        // The parent keeps stdin open and kills this process
                        // after this flushed signal. No unwinding is involved.
                        let mut output = std::io::stdout().lock();
                        writeln!(output, "\n{READY}")?;
                        output.flush()?;
                        let mut byte = [0];
                        std::io::stdin().read_exact(&mut byte)?;
                        panic!("parent must kill the preparer at the boundary");
                    }
                    Ok(())
                },
            )
            .expect("child must reach the requested checkpoint");
        panic!("child must not return a preparation acknowledgment");
    }

    for checkpoint in ["frozen", "materialized", "before-publish", "after-publish"] {
        let fixture = Fixture::new();
        let expected = selection_for(PAYLOAD, "server");
        let mut child = Command::new(std::env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "local_artifacts::tests::killed_preparer_is_recovered_from_the_original_attempt_root",
                "--nocapture",
            ])
            .env(CHILD_ROOT, fixture.root.path())
            .env(CHILD_POINT, checkpoint)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn preparer child");
        let output = child.stdout.take().expect("child output");
        let (sender, receiver) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in std::io::BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if line.trim() == READY {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        let reached = receiver.recv_timeout(Duration::from_secs(15));
        // Always reap before assertions, including a failed checkpoint wait.
        let killed = child.kill();
        let status = child.wait().expect("reap preparer child");
        reader.join().expect("join child output reader");
        reached.expect("child reached the actual requested boundary");
        killed.expect("kill the still-blocked preparer");
        assert_eq!(status.signal(), Some(libc::SIGKILL));

        let published = fixture.root.path().join("prepared");
        if checkpoint == "after-publish" {
            assert!(!fixture.root.path().join("incomplete").exists());
            let evidence: PreparedArtifactEvidence = serde_json::from_slice(
                &std::fs::read(published.join("evidence.json")).expect("published evidence"),
            )
            .expect("inert receipt");
            let restarted = LocalArtifactPreparer::new(
                cap_std::fs::Dir::open_ambient_dir(
                    fixture.root.path(),
                    cap_std::ambient_authority(),
                )
                .expect("reopen host-recorded attempt root"),
            );
            let reopened = restarted
                .reopen_verified(
                    &expected,
                    &id("component"),
                    &LocalArtifactLayout::SingleFile { executable: false },
                    &evidence,
                    &PreparationLimits::default(),
                )
                .expect("recover selected bytes after lost acknowledgment");
            assert_eq!(reopened.read("server", 1024).unwrap(), PAYLOAD);
        } else {
            assert!(!published.exists());
            assert!(fixture.root.path().join("incomplete").is_dir());
            // A new attempt cannot silently overwrite unacknowledged work in
            // the same recorded root. Cleanup belongs to the stopped owner.
            assert!(
                fixture
                    .prepare(
                        PAYLOAD,
                        LocalArtifactLayout::SingleFile { executable: false },
                        "server"
                    )
                    .is_err()
            );
            assert!(!published.exists());
        }
    }
}

fn id(value: &str) -> Id {
    Id::new(value).unwrap()
}
fn revision(value: &str) -> Revision {
    Revision::new(value).unwrap()
}
fn digest(value: &str) -> ContentDigest {
    ContentDigest::of_bytes(value.as_bytes())
}
fn contract(value: &str) -> ContractRequirement {
    ContractRequirement {
        name: id(value),
        version: VersionRequirement {
            major: 1,
            minimum_minor: 0,
        },
    }
}

fn selection_parts(
    payload: &[u8],
    dependencies: ArtifactDependencies,
    entrypoint: &str,
) -> (IntegrationPackage, ActivationManifest) {
    let package = IntegrationPackage {
        schema_version: 1,
        identity: PackageIdentity {
            publisher: id("fixture"),
            name: id("local-package"),
            version: revision("1"),
        },
        display_name: "Local byte fixture".into(),
        description: "No executable integration".into(),
        provenance: Provenance {
            source: Url::parse("https://example.org/source").unwrap(),
            source_revision: revision("commit"),
            license: "MIT".into(),
            notices: vec![],
            local_patches: vec![],
        },
        strategies: vec![SetupStrategy {
            id: id("local"),
            kind: StrategyKind::LocalMcp,
            instructions: "Select the local artifact".into(),
            sources: vec![],
            known_requirements: vec![],
            unresolved_details: vec![],
        }],
        known_exports: vec![],
        assessments: vec![],
    };
    let component = Component {
        id: id("component"),
        version: revision("1"),
        source: ComponentSource::Artifact {
            location: Url::parse("file:///selected.zip").unwrap(),
            content_digest: ContentDigest::of_bytes(payload),
            dependencies,
            entrypoint: Entrypoint {
                executable: entrypoint.into(),
                arguments: vec!["literal;not-a-shell".into()],
                python_console_script: None,
            },
        },
        runtime: RuntimeKind::Native,
        placement: id("local"),
        lifetime: ComponentLifetime::Deployment,
        activation_scope: ActivationScope::Independent,
        dependencies: vec![],
        required_contracts: vec![],
        host_services: vec![],
        state: vec![],
        readiness: ReadinessContract {
            owner: id("runtime"),
            checks: [id("ready")].into(),
        },
        shutdown: ShutdownContract {
            owner: id("runtime"),
            operation: id("stop"),
            maximum_wait_ms: 1000,
            timeout: ShutdownTimeout::PreserveUncertainty,
        },
        rollback: RollbackContract::OwnerReconciliation {
            owner: id("runtime"),
            operation: id("reconcile"),
        },
    };
    let manifest = ActivationManifest {
        schema_version: 1,
        package: package.identity.clone(),
        package_content_digest: package.content_digest().unwrap(),
        strategy: id("local"),
        configuration: ConfigurationSelection {
            owner: id("config"),
            reference: id("selected"),
            revision: revision("1"),
            content_digest: digest("config"),
        },
        policy: PolicySelection {
            owner: id("policy"),
            revision: revision("1"),
            content_digest: digest("policy"),
        },
        components: vec![component],
        exports: vec![Export::AgentFrontend(AgentFrontendExport {
            id: id("frontend"),
            components: [id("component")].into(),
            modalities: [Modality::Text].into(),
            admission_contract: contract("admission"),
            conversation_contract: contract("conversation"),
            delivery_contract: contract("delivery"),
            control_contract: None,
            live_contract: None,
            authentication: AuthenticationRequirement::None,
            diagnostics: id("check"),
            cleanup: id("stop"),
        })],
        required_contracts: vec![],
        host_services: vec![],
        portable_mcp_entries: vec![],
    };
    (package, manifest)
}
fn selection_for(payload: &[u8], entrypoint: &str) -> ValidatedManifest {
    let (package, manifest) =
        selection_parts(payload, ArtifactDependencies::SelfContained, entrypoint);
    manifest.validate(&package).unwrap()
}

struct Fixture {
    root: TempDir,
    source: TempDir,
    preparer: LocalArtifactPreparer,
}
impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let source = TempDir::new().unwrap();
        let preparer = LocalArtifactPreparer::new(
            cap_std::fs::Dir::open_ambient_dir(root.path(), cap_std::ambient_authority()).unwrap(),
        );
        Self {
            root,
            source,
            preparer,
        }
    }
    fn input(&self, bytes: &[u8]) -> LocalArtifactInputs {
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        LocalArtifactInputs {
            artifact: source,
            pinned_dependencies: BTreeMap::new(),
        }
    }
    fn prepare(
        &self,
        bytes: &[u8],
        layout: LocalArtifactLayout,
        entrypoint: &str,
    ) -> Result<PreparedLocalArtifact> {
        self.preparer.prepare(
            &selection_for(bytes, entrypoint),
            &id("component"),
            layout,
            self.input(bytes),
            &PreparationLimits::default(),
        )
    }
    fn path(&self, evidence: &PreparedArtifactEvidence) -> PathBuf {
        self.root.path().join(evidence.locator())
    }
    fn published(&self) -> usize {
        std::fs::read_dir(self.root.path())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().eq("prepared"))
            .count()
    }
}
// Read-only trees deliberately survive handle drops. Test teardown owns its
// temporary root, so it may restore permissions before TempDir removes it.
impl Drop for Fixture {
    fn drop(&mut self) {
        make_writable(self.root.path());
    }
}
fn make_writable(path: &Path) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_symlink() {
        return;
    }
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode(if metadata.is_dir() { 0o700 } else { 0o600 }),
    )
    .unwrap();
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            make_writable(&entry.path());
        }
    }
}
fn overwrite(path: &Path, bytes: &[u8]) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE)).unwrap();
}
fn archive(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(method)
        .unix_permissions(0o755);
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
fn central(bytes: &[u8]) -> usize {
    bytes
        .windows(4)
        .position(|part| part == b"PK\x01\x02")
        .unwrap()
}
fn put16(bytes: &mut [u8], position: usize, value: u16) {
    bytes[position..position + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], position: usize, value: u32) {
    bytes[position..position + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn file_preparation_rewinds_input_and_reopens_real_bytes() {
    let fixture = Fixture::new();
    let bytes = b"inert executable bytes";
    let manifest = selection_for(bytes, "bin/server");
    let mut inputs = fixture.input(bytes);
    inputs.artifact.seek(SeekFrom::End(0)).unwrap();
    let prepared = fixture
        .preparer
        .prepare(
            &manifest,
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: true },
            inputs,
            &PreparationLimits::default(),
        )
        .unwrap();
    assert_eq!(prepared.read("bin/server", 1024).unwrap(), bytes);
    assert_eq!(prepared.entrypoint().arguments, ["literal;not-a-shell"]);
    assert!(prepared.read("bin/server", 1).is_err());
    assert!(prepared.read("../payload", 1024).is_err());
    let receipt =
        serde_json::from_slice(&serde_json::to_vec(prepared.evidence()).unwrap()).unwrap();
    drop(prepared);
    let reopened = fixture
        .preparer
        .reopen_verified(
            &manifest,
            &id("component"),
            &LocalArtifactLayout::SingleFile { executable: true },
            &receipt,
            &PreparationLimits::default(),
        )
        .unwrap();
    assert_eq!(reopened.read("bin/server", 1024).unwrap(), bytes);
    assert_eq!(fixture.published(), 1);
}

#[test]
fn zip_is_materialized_and_existing_mcpb_parser_reads_verified_manifest() {
    let metadata = br#"{"manifest_version":"0.3","name":"fixture","version":"1.0.0","description":"Inert fixture","author":{"name":"Fixture"},"server":{"type":"binary","entry_point":"bin/server","mcp_config":{"command":"${__dirname}/bin/server"}}}"#;
    for method in [
        zip::CompressionMethod::Stored,
        zip::CompressionMethod::Deflated,
    ] {
        let fixture = Fixture::new();
        let bytes = archive(
            &[
                ("bin/", b""),
                ("bin/server", b"never executed"),
                ("manifest.json", metadata),
            ],
            method,
        );
        let prepared = fixture
            .prepare(&bytes, LocalArtifactLayout::Zip, "bin/server")
            .unwrap();
        assert_eq!(
            prepared.read("bin/server", 1024).unwrap(),
            b"never executed"
        );
        ImportedMcpbManifest::parse(&prepared.read("manifest.json", 8192).unwrap()).unwrap();
        fixture
            .preparer
            .reopen_verified(
                &selection_for(&bytes, "bin/server"),
                &id("component"),
                &LocalArtifactLayout::Zip,
                prepared.evidence(),
                &PreparationLimits::default(),
            )
            .unwrap();
    }
}

#[test]
fn descriptor_substitution_after_freeze_does_not_change_output() {
    let fixture = Fixture::new();
    let selected = b"selected source";
    let original = fixture.source.path().join("source");
    std::fs::write(&original, selected).unwrap();
    let inputs = LocalArtifactInputs {
        artifact: File::open(&original).unwrap(),
        pinned_dependencies: BTreeMap::new(),
    };
    let prepared = fixture
        .preparer
        .prepare_with(
            &selection_for(selected, "server"),
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            inputs,
            &PreparationLimits::default(),
            &mut |point| {
                if matches!(point, Point::Frozen) {
                    std::fs::remove_file(&original)?;
                    std::fs::write(&original, b"replacement")?;
                }
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(prepared.read("server", 1024).unwrap(), selected);
}

#[test]
fn selected_pin_and_all_dependency_bytes_are_required() {
    let fixture = Fixture::new();
    let bytes = b"server";
    let dependency = b"opaque dependency artifact";
    let pins = BTreeMap::from([(id("dep/name"), ContentDigest::of_bytes(dependency))]);
    let (package, manifest) = selection_parts(
        bytes,
        ArtifactDependencies::PinnedClosure { artifacts: pins },
        "server",
    );
    let manifest = manifest.validate(&package).unwrap();
    assert!(
        fixture
            .preparer
            .prepare(
                &manifest,
                &id("component"),
                LocalArtifactLayout::SingleFile { executable: false },
                fixture.input(bytes),
                &PreparationLimits::default()
            )
            .is_err()
    );
    let mut input = fixture.input(bytes);
    input
        .pinned_dependencies
        .insert(id("dep/name"), fixture.input(dependency).artifact);
    let prepared = fixture
        .preparer
        .prepare(
            &manifest,
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            input,
            &PreparationLimits::default(),
        )
        .unwrap();
    fixture
        .preparer
        .reopen_verified(
            &manifest,
            &id("component"),
            &LocalArtifactLayout::SingleFile { executable: false },
            prepared.evidence(),
            &PreparationLimits::default(),
        )
        .unwrap();
    let fixture = Fixture::new();
    let mut wrong = fixture.input(bytes);
    wrong
        .pinned_dependencies
        .insert(id("dep/name"), fixture.input(b"wrong").artifact);
    assert!(matches!(
        fixture.preparer.prepare(
            &manifest,
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            wrong,
            &PreparationLimits::default()
        ),
        Err(PreparationError::DigestMismatch)
    ));
    let fixture = Fixture::new();
    let mut extra = fixture.input(bytes);
    extra
        .pinned_dependencies
        .insert(id("unselected"), fixture.input(dependency).artifact);
    assert!(
        fixture
            .preparer
            .prepare(
                &selection_for(bytes, "server"),
                &id("component"),
                LocalArtifactLayout::SingleFile { executable: false },
                extra,
                &PreparationLimits::default()
            )
            .is_err()
    );
    let fixture = Fixture::new();
    assert!(matches!(
        fixture.preparer.prepare(
            &selection_for(b"other pin", "server"),
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            fixture.input(bytes),
            &PreparationLimits::default()
        ),
        Err(PreparationError::DigestMismatch)
    ));
    let (package, manifest) = selection_parts(
        bytes,
        ArtifactDependencies::Locked {
            ecosystem: id("python"),
            lock_digest: digest("lock"),
        },
        "server",
    );
    assert!(matches!(
        fixture.preparer.prepare(
            &manifest.validate(&package).unwrap(),
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            fixture.input(bytes),
            &PreparationLimits::default()
        ),
        Err(PreparationError::DependencyRealizationRequired)
    ));
}

fn rejects_zip(bytes: &[u8], entrypoint: &str) {
    let fixture = Fixture::new();
    assert!(
        fixture
            .prepare(bytes, LocalArtifactLayout::Zip, entrypoint)
            .is_err()
    );
    assert_eq!(fixture.published(), 0);
    assert!(!fixture.root.path().join("escape").exists());
}

#[test]
fn archive_paths_types_and_local_metadata_fail_before_publication() {
    let paths = [
        "../escape",
        "/escape",
        "C:/escape",
        "a\\b",
        "a//b",
        "a/./b",
        "CON",
        "aux.txt",
        "file.",
        "file ",
        "caf\u{e9}",
        "a\0b",
    ];
    for path in paths {
        rejects_zip(
            &archive(
                &[(path, b"bad"), ("server", b"ok")],
                zip::CompressionMethod::Stored,
            ),
            "server",
        );
    }
    for paths in [["A/x", "a/y"], ["a", "a/b"], ["server", "SERVER"]] {
        rejects_zip(
            &archive(
                &[(paths[0], b"x"), (paths[1], b"y")],
                zip::CompressionMethod::Stored,
            ),
            paths[0],
        );
    }
    let base = archive(&[("server", b"ok")], zip::CompressionMethod::Stored);
    for file_type in [0o120777_u32, 0o010600, 0o020600, 0o060600] {
        let mut bytes = base.clone();
        let cd = central(&bytes);
        put32(&mut bytes, cd + 38, file_type << 16);
        rejects_zip(&bytes, "server");
    }
    let mut mismatch = base.clone();
    mismatch[30] = b'X';
    rejects_zip(&mismatch, "server");
    let mut encrypted = base.clone();
    let cd = central(&encrypted);
    put16(&mut encrypted, cd + 8, 1);
    put16(&mut encrypted, 6, 1);
    rejects_zip(&encrypted, "server");
    let mut descriptor = base.clone();
    let cd = central(&descriptor);
    put16(&mut descriptor, cd + 8, 8);
    put16(&mut descriptor, 6, 8);
    rejects_zip(&descriptor, "server");
    let mut bad_crc = base.clone();
    let cd = central(&bad_crc);
    put32(&mut bad_crc, cd + 16, 1);
    put32(&mut bad_crc, 14, 1);
    rejects_zip(&bad_crc, "server");
    let fixture = Fixture::new();
    let outside = fixture.source.path().join("must-not-be-created");
    let absolute = archive(
        &[(outside.to_str().unwrap(), b"bad"), ("server", b"ok")],
        zip::CompressionMethod::Stored,
    );
    assert!(
        fixture
            .prepare(&absolute, LocalArtifactLayout::Zip, "server")
            .is_err()
    );
    assert!(!outside.exists());
}

#[test]
fn duplicate_zip_members_and_overlaps_are_not_hidden_by_an_index() {
    let base = archive(
        &[("server", b"one"), ("second", b"two")],
        zip::CompressionMethod::Stored,
    );
    let first_cd = central(&base);
    let second_cd = first_cd + 46 + 6;
    let second_local = 30 + 6 + 3;
    let mut duplicate = base.clone();
    duplicate[second_local + 30..second_local + 36].copy_from_slice(b"server");
    duplicate[second_cd + 46..second_cd + 52].copy_from_slice(b"server");
    rejects_zip(&duplicate, "server");
    let mut overlap = base.clone();
    put32(&mut overlap, second_cd + 42, 0);
    rejects_zip(&overlap, "server");
    let mut zip64 = base.clone();
    let end = zip64.len() - 22;
    put16(&mut zip64, end + 10, u16::MAX);
    rejects_zip(&zip64, "server");
    let mut trailing = base.clone();
    trailing.extend_from_slice(b"hidden suffix");
    rejects_zip(&trailing, "server");
}

#[test]
fn actual_inflation_and_metadata_limits_are_enforced() {
    let bomb = archive(
        &[("server", &vec![b'x'; 200_000])],
        zip::CompressionMethod::Deflated,
    );
    let reject = |bytes: &[u8], limits: &PreparationLimits| {
        let fixture = Fixture::new();
        assert!(
            fixture
                .preparer
                .prepare(
                    &selection_for(bytes, "server"),
                    &id("component"),
                    LocalArtifactLayout::Zip,
                    fixture.input(bytes),
                    limits
                )
                .is_err()
        );
        assert_eq!(fixture.published(), 0);
    };
    let limits = PreparationLimits {
        maximum_file_bytes: 1024,
        ..PreparationLimits::default()
    };
    reject(&bomb, &limits);
    let mut lied = bomb.clone();
    let cd = central(&lied);
    put32(&mut lied, 22, 1);
    put32(&mut lied, cd + 24, 1);
    reject(&lied, &limits);
    let ordinary = archive(&[("server", b"ok")], zip::CompressionMethod::Stored);
    reject(
        &ordinary,
        &PreparationLimits {
            maximum_archive_metadata_bytes: 8,
            ..PreparationLimits::default()
        },
    );
    reject(
        &ordinary,
        &PreparationLimits {
            maximum_raw_bytes: 1,
            ..PreparationLimits::default()
        },
    );
}

#[test]
fn recovery_rederives_pins_not_self_consistent_forged_evidence() {
    let fixture = Fixture::new();
    let bytes = archive(&[("server", b"selected")], zip::CompressionMethod::Stored);
    let manifest = selection_for(&bytes, "server");
    let prepared = fixture
        .prepare(&bytes, LocalArtifactLayout::Zip, "server")
        .unwrap();
    let path = fixture.path(prepared.evidence());
    let mut body = prepared.evidence().body.clone();
    body.inventory.insert(
        "server".into(),
        InventoryValue::File {
            size: 6,
            digest: ContentDigest::of_bytes(b"forged"),
            executable: true,
        },
    );
    let forged = PreparedArtifactEvidence::new(body).unwrap();
    overwrite(&path.join("tree/server"), b"forged");
    std::fs::set_permissions(
        path.join("tree/server"),
        std::fs::Permissions::from_mode(EXECUTABLE_MODE),
    )
    .unwrap();
    overwrite(
        &path.join("evidence.json"),
        &canonical_json(&forged).unwrap(),
    );
    assert!(
        fixture
            .preparer
            .reopen_verified(
                &manifest,
                &id("component"),
                &LocalArtifactLayout::Zip,
                &forged,
                &PreparationLimits::default()
            )
            .is_err()
    );
    assert!(prepared.read("server", 1024).is_err());
}

#[test]
fn recovery_rejects_missing_added_changed_and_special_entries() {
    for mutation in 0..7 {
        let fixture = Fixture::new();
        let prepared = fixture
            .prepare(
                b"selected",
                LocalArtifactLayout::SingleFile { executable: false },
                "nested/server",
            )
            .unwrap();
        let evidence = prepared.evidence().clone();
        let path = fixture.path(&evidence);
        let tree = path.join("tree");
        match mutation {
            0 => {
                std::fs::set_permissions(
                    tree.join("nested"),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();
                std::fs::remove_file(tree.join("nested/server")).unwrap();
            }
            1 => {
                std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::create_dir(tree.join("unexpected-empty")).unwrap();
                std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(DIRECTORY_MODE))
                    .unwrap();
            }
            2 => overwrite(&tree.join("nested/server"), b"modified"),
            3 => {
                std::fs::set_permissions(
                    tree.join("nested/server"),
                    std::fs::Permissions::from_mode(0o600),
                )
                .unwrap();
            }
            4 => {
                std::fs::set_permissions(
                    tree.join("nested"),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();
                std::fs::remove_file(tree.join("nested/server")).unwrap();
                std::os::unix::fs::symlink(fixture.source.path(), tree.join("nested/server"))
                    .unwrap();
                std::fs::set_permissions(
                    tree.join("nested"),
                    std::fs::Permissions::from_mode(DIRECTORY_MODE),
                )
                .unwrap();
            }
            5 => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
                std::fs::rename(path.join("dependencies"), path.join("old-deps")).unwrap();
                std::os::unix::fs::symlink(path.join("old-deps"), path.join("dependencies"))
                    .unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(DIRECTORY_MODE))
                    .unwrap();
            }
            _ => overwrite(&path.join("payload"), b"changed raw"),
        }
        assert!(
            fixture
                .preparer
                .reopen_verified(
                    &selection_for(b"selected", "nested/server"),
                    &id("component"),
                    &LocalArtifactLayout::SingleFile { executable: false },
                    &evidence,
                    &PreparationLimits::default()
                )
                .is_err(),
            "mutation {mutation}"
        );
    }
}

#[test]
fn different_selection_and_layout_cannot_reuse_a_receipt() {
    let fixture = Fixture::new();
    let prepared = fixture
        .prepare(
            b"selected",
            LocalArtifactLayout::SingleFile { executable: false },
            "server",
        )
        .unwrap();
    let (package, mut manifest) =
        selection_parts(b"selected", ArtifactDependencies::SelfContained, "server");
    manifest.policy.revision = revision("new-policy");
    assert!(
        fixture
            .preparer
            .reopen_verified(
                &manifest.validate(&package).unwrap(),
                &id("component"),
                &LocalArtifactLayout::SingleFile { executable: false },
                prepared.evidence(),
                &PreparationLimits::default()
            )
            .is_err()
    );
    let mut changed = prepared.evidence().body.clone();
    changed.layout = LocalArtifactLayout::Zip;
    let changed = PreparedArtifactEvidence::new(changed).unwrap();
    overwrite(
        &fixture.path(prepared.evidence()).join("evidence.json"),
        &canonical_json(&changed).unwrap(),
    );
    assert!(
        fixture
            .preparer
            .reopen_verified(
                &selection_for(b"selected", "server"),
                &id("component"),
                &LocalArtifactLayout::SingleFile { executable: false },
                &changed,
                &PreparationLimits::default()
            )
            .is_err()
    );
}

// Hosts pass a cap-std ambient root. On Linux that descriptor is O_PATH, which
// cannot be fsynced itself; publication must still confirm durability.
#[test]
fn publication_is_confirmed_through_an_ambient_cap_std_root() {
    let fixture = Fixture::new();
    #[cfg(target_os = "linux")]
    assert!(
        fs::fcntl_getfl(&fixture.preparer.root)
            .unwrap()
            .contains(OFlags::PATH)
    );
    let prepared = fixture.prepare(
        b"durable",
        LocalArtifactLayout::SingleFile { executable: false },
        "server",
    );
    assert!(
        prepared.is_ok(),
        "publication was not confirmed: {:?}",
        prepared.err()
    );
    assert_eq!(fixture.published(), 1);
}

#[test]
fn publication_faults_leave_no_false_success_and_lost_ack_is_recoverable() {
    for failure in [
        Point::Frozen,
        Point::Materialized,
        Point::BeforePublish,
        Point::AfterPublish,
    ] {
        let fixture = Fixture::new();
        let manifest = selection_for(b"selected", "server");
        let result = fixture.preparer.prepare_with(
            &manifest,
            &id("component"),
            LocalArtifactLayout::SingleFile { executable: false },
            fixture.input(b"selected"),
            &PreparationLimits::default(),
            &mut |point| {
                if std::mem::discriminant(&point) == std::mem::discriminant(&failure) {
                    return Err(PreparationError::Invalid("injected boundary failure"));
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        if matches!(failure, Point::AfterPublish) {
            let Err(PreparationError::PublishedButUnconfirmed { locator }) = result else {
                panic!("must retain publication uncertainty")
            };
            let receipt: PreparedArtifactEvidence = serde_json::from_slice(
                &std::fs::read(fixture.root.path().join("prepared").join("evidence.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(locator, "prepared");
            let restarted = LocalArtifactPreparer::new(
                cap_std::fs::Dir::open_ambient_dir(
                    fixture.root.path(),
                    cap_std::ambient_authority(),
                )
                .unwrap(),
            );
            restarted
                .reopen_verified(
                    &manifest,
                    &id("component"),
                    &LocalArtifactLayout::SingleFile { executable: false },
                    &receipt,
                    &PreparationLimits::default(),
                )
                .unwrap();
            assert_eq!(fixture.published(), 1);
        } else {
            assert_eq!(fixture.published(), 0);
        }
    }
}

#[test]
fn concurrent_preparations_do_not_overwrite_and_noreplace_is_enforced() {
    let fixture = Fixture::new();
    // One recorded attempt root has one published result, even if its caller
    // accidentally enters twice. A losing worker cannot replace that result.
    std::thread::scope(|scope| {
        let handles = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    fixture.prepare(
                        b"selected",
                        LocalArtifactLayout::SingleFile { executable: false },
                        "server",
                    )
                })
            })
            .collect::<Vec<_>>();
        let successes = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().is_ok())
            .filter(|success| *success)
            .count();
        assert_eq!(successes, 1);
    });
    assert_eq!(fixture.published(), 1);
    create_directory(&fixture.preparer.root, "occupied").unwrap();
    create_directory(&fixture.preparer.root, "candidate").unwrap();
    assert!(
        fs::renameat_with(
            &fixture.preparer.root,
            "candidate",
            &fixture.preparer.root,
            "occupied",
            RenameFlags::NOREPLACE
        )
        .is_err()
    );
    // Independent host-recorded roots can prepare the same bytes concurrently.
    std::thread::scope(|scope| {
        let handles = (0..3)
            .map(|_| {
                scope.spawn(|| {
                    let attempt = Fixture::new();
                    let prepared = attempt
                        .prepare(
                            b"selected",
                            LocalArtifactLayout::SingleFile { executable: false },
                            "server",
                        )
                        .unwrap();
                    assert_eq!(prepared.read("server", 1024).unwrap(), b"selected");
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }
    });
}

#[test]
fn one_preparer_handles_distinct_service_and_frontend_components() {
    let bytes = b"inert component";
    let (package, mut manifest) =
        selection_parts(bytes, ArtifactDependencies::SelfContained, "server");
    let mut daemon = manifest.components[0].clone();
    daemon.id = id("device-daemon");
    manifest.components.push(daemon);
    manifest
        .exports
        .push(Export::ServiceConnector(ServiceConnectorExport {
            id: id("device-service"),
            components: [id("device-daemon")].into(),
            authentication: AuthenticationRequirement::None,
            operations: vec![OperationDeclaration {
                id: id("observe"),
                schema_digest: digest("observation schema"),
                declared_effect: DeclaredEffect::Read,
                required_authority: [id("device.read")].into(),
                resources: ResourceContract::Selected {
                    selector: id("device"),
                    enforcement_owner: id("device-policy"),
                },
                result_limits: ResultLimits {
                    maximum_bytes: 1024,
                    maximum_items: 1,
                },
            }],
            events: vec![],
            enforcement_contract: contract("service-policy"),
            diagnostics: id("check-device"),
            cleanup: id("release-device"),
        }));
    let selection = manifest.validate(&package).unwrap();
    for component in ["component", "device-daemon"] {
        let fixture = Fixture::new();
        let prepared = fixture
            .preparer
            .prepare(
                &selection,
                &id(component),
                LocalArtifactLayout::SingleFile { executable: false },
                fixture.input(bytes),
                &PreparationLimits::default(),
            )
            .unwrap();
        assert_eq!(prepared.evidence().component(), &id(component));
        assert_eq!(prepared.read("server", 1024).unwrap(), bytes);
        assert_eq!(fixture.published(), 1);
    }
}

#[test]
fn read_rejects_parent_symlinks_and_does_not_follow_outside_tree() {
    let fixture = Fixture::new();
    let prepared = fixture
        .prepare(
            b"selected",
            LocalArtifactLayout::SingleFile { executable: false },
            "nested/server",
        )
        .unwrap();
    let tree = fixture.path(prepared.evidence()).join("tree");
    std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::rename(tree.join("nested"), tree.join("previous")).unwrap();
    std::fs::write(fixture.source.path().join("server"), b"outside").unwrap();
    std::os::unix::fs::symlink(fixture.source.path(), tree.join("nested")).unwrap();
    std::fs::set_permissions(&tree, std::fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
    assert!(prepared.read("nested/server", 1024).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn read_rejects_fifo_without_waiting_for_a_writer() {
    let fixture = Fixture::new();
    let prepared = fixture
        .prepare(
            b"selected",
            LocalArtifactLayout::SingleFile { executable: false },
            "server",
        )
        .unwrap();
    let tree_path = fixture.path(prepared.evidence()).join("tree");
    std::fs::set_permissions(&tree_path, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::remove_file(tree_path.join("server")).unwrap();
    fs::mkfifoat(&prepared.tree, "server", Mode::from_raw_mode(FILE_MODE)).unwrap();
    std::fs::set_permissions(&tree_path, std::fs::Permissions::from_mode(DIRECTORY_MODE)).unwrap();
    assert!(prepared.read("server", 1024).is_err());
}

#[test]
fn recovery_cannot_reinterpret_same_pin_as_executable_or_another_component() {
    let fixture = Fixture::new();
    let bytes = b"same pinned file";
    let (package, mut selected) =
        selection_parts(bytes, ArtifactDependencies::SelfContained, "server");
    let mut other = selected.components[0].clone();
    other.id = id("other-component");
    selected.components.push(other);
    let Export::AgentFrontend(frontend) = &mut selected.exports[0] else {
        panic!("fixture frontend")
    };
    frontend.components.insert(id("other-component"));
    let manifest = selected.validate(&package).unwrap();
    let original_layout = LocalArtifactLayout::SingleFile { executable: false };
    let prepared = fixture
        .preparer
        .prepare(
            &manifest,
            &id("component"),
            original_layout.clone(),
            fixture.input(bytes),
            &PreparationLimits::default(),
        )
        .unwrap();
    let path = fixture.path(prepared.evidence());
    // This is a valid alternate interpretation of identical raw bytes, and the
    // forged disk record is internally consistent. Only original host intent
    // can distinguish it from the requested non-executable materialization.
    let mut changed = prepared.evidence().body.clone();
    changed.layout = LocalArtifactLayout::SingleFile { executable: true };
    let Some(InventoryValue::File { executable, .. }) = changed.inventory.get_mut("server") else {
        panic!("fixture file")
    };
    *executable = true;
    let changed = PreparedArtifactEvidence::new(changed).unwrap();
    std::fs::set_permissions(
        path.join("tree/server"),
        std::fs::Permissions::from_mode(EXECUTABLE_MODE),
    )
    .unwrap();
    overwrite(
        &path.join("evidence.json"),
        &canonical_json(&changed).unwrap(),
    );
    assert!(
        fixture
            .preparer
            .reopen_verified(
                &manifest,
                &id("component"),
                &original_layout,
                &changed,
                &PreparationLimits::default()
            )
            .is_err()
    );

    let mut swapped = changed.body.clone();
    swapped.component = id("other-component");
    let swapped = PreparedArtifactEvidence::new(swapped).unwrap();
    overwrite(
        &path.join("evidence.json"),
        &canonical_json(&swapped).unwrap(),
    );
    assert!(
        fixture
            .preparer
            .reopen_verified(
                &manifest,
                &id("component"),
                &LocalArtifactLayout::SingleFile { executable: true },
                &swapped,
                &PreparationLimits::default()
            )
            .is_err()
    );
}

#[test]
fn aggregate_expansion_dependencies_entries_and_paths_obey_limits() {
    let bytes = archive(
        &[("server", &vec![b'a'; 600]), ("second", &vec![b'b'; 600])],
        zip::CompressionMethod::Deflated,
    );
    for limits in [
        PreparationLimits {
            maximum_file_bytes: 1024,
            maximum_expanded_bytes: 1000,
            ..PreparationLimits::default()
        },
        PreparationLimits {
            maximum_entries: 1,
            ..PreparationLimits::default()
        },
    ] {
        let fixture = Fixture::new();
        assert!(
            fixture
                .preparer
                .prepare(
                    &selection_for(&bytes, "server"),
                    &id("component"),
                    LocalArtifactLayout::Zip,
                    fixture.input(&bytes),
                    &limits
                )
                .is_err()
        );
        assert_eq!(fixture.published(), 0);
    }
    let nested = archive(&[("nested/server", b"x")], zip::CompressionMethod::Stored);
    let fixture = Fixture::new();
    assert!(
        fixture
            .preparer
            .prepare(
                &selection_for(&nested, "nested/server"),
                &id("component"),
                LocalArtifactLayout::Zip,
                fixture.input(&nested),
                &PreparationLimits {
                    maximum_entries: 1,
                    ..PreparationLimits::default()
                }
            )
            .is_err()
    );
    let long_name = archive(
        &[("server", b"x"), ("long-filename", b"y")],
        zip::CompressionMethod::Stored,
    );
    let fixture = Fixture::new();
    assert!(
        fixture
            .preparer
            .prepare(
                &selection_for(&long_name, "server"),
                &id("component"),
                LocalArtifactLayout::Zip,
                fixture.input(&long_name),
                &PreparationLimits {
                    maximum_path_bytes: 6,
                    ..PreparationLimits::default()
                }
            )
            .is_err()
    );

    let dependency = vec![b'd'; 600];
    let pins = BTreeMap::from([
        (id("first"), ContentDigest::of_bytes(&dependency)),
        (id("second"), ContentDigest::of_bytes(&dependency)),
    ]);
    let (package, manifest) = selection_parts(
        b"server",
        ArtifactDependencies::PinnedClosure { artifacts: pins },
        "server",
    );
    let manifest = manifest.validate(&package).unwrap();
    for limits in [
        PreparationLimits {
            maximum_dependency_bytes: 1000,
            ..PreparationLimits::default()
        },
        PreparationLimits {
            maximum_entries: 1,
            ..PreparationLimits::default()
        },
    ] {
        let fixture = Fixture::new();
        let mut input = fixture.input(b"server");
        input
            .pinned_dependencies
            .insert(id("first"), fixture.input(&dependency).artifact);
        input
            .pinned_dependencies
            .insert(id("second"), fixture.input(&dependency).artifact);
        assert!(matches!(
            fixture.preparer.prepare(
                &manifest,
                &id("component"),
                LocalArtifactLayout::SingleFile { executable: false },
                input,
                &limits
            ),
            Err(PreparationError::Limit(_))
        ));
        assert_eq!(fixture.published(), 0);
        if limits.maximum_entries == 1 {
            assert!(!fixture.root.path().join("incomplete").exists());
        }
    }
}

#[test]
fn raw_artifact_preparation_refuses_python_console_selector_before_materialization() {
    let fixture = Fixture::new();
    let payload = b"inert artifact";
    let (package, mut manifest) = selection_parts(payload, ArtifactDependencies::SelfContained, "main.py");
    manifest.components[0].runtime = RuntimeKind::Python;
    let ComponentSource::Artifact { entrypoint, .. } = &mut manifest.components[0].source else {
        unreachable!()
    };
    entrypoint.python_console_script = Some("serve-it".into());
    let manifest = manifest.validate(&package).unwrap();
    let result = fixture.preparer.prepare(
        &manifest, &id("component"), LocalArtifactLayout::SingleFile { executable: false },
        fixture.input(payload), &PreparationLimits::default(),
    );
    assert!(matches!(result, Err(PreparationError::Unsupported(_))));
    assert_eq!(fixture.published(), 0);
}
