use super::*;
use crate::service_collection::{CollectionCursor, CollectionTarget};
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
};

const PACKAGE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../toolkit-package/tests/fixtures/python-install/package.json"
));
const MANIFEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../toolkit-package/tests/fixtures/python-install/activation.json"
));
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// Synthetic receipt and placeholder wheel hashes exercise binding only. They
// are not installed-software, credential, native-dispatch or live-read proof.
struct Fixture {
    installed: InstalledSelection,
    config: McpServerConfig,
    inputs: SelectedServiceInputs,
    package: IntegrationPackage,
    plan: Value,
    operations: Vec<OperationDeclaration>,
    receipt: Value,
}
impl Fixture {
    fn new(root: &Path, software: bool, effect: &str) -> Self {
        let package: IntegrationPackage = serde_json::from_slice(PACKAGE).unwrap();
        let mut document: Value = serde_json::from_slice(MANIFEST).unwrap();
        document["exports"][0]["authentication"] = json!({"kind":"binding","owner":"fixture/process",
            "requirement":"api-key","credential_audience":"fixture/service"});
        document["exports"][0]["operations"][0]["declared_effect"] = json!(effect);
        document["portable_mcp_entries"][0]["operation_tools"] =
            json!({"fixture-read":"actual_read"});
        document["portable_mcp_entries"][0]["environment_secret_slots"] = json!(["TOKEN"]);
        if software {
            // Deliberately construct the smaller software schema. Production
            // code never converts a manifest or fabricates these host fields.
            for key in [
                "configuration",
                "policy",
                "required_contracts",
                "host_services",
            ] {
                document.as_object_mut().unwrap().remove(key);
            }
            for key in [
                "placement",
                "lifetime",
                "activation_scope",
                "dependencies",
                "required_contracts",
                "host_services",
                "state",
                "readiness",
                "shutdown",
                "rollback",
            ] {
                document["components"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
            }
            for key in ["events", "enforcement_contract", "diagnostics", "cleanup"] {
                document["exports"][0].as_object_mut().unwrap().remove(key);
            }
            document["document_kind"] = json!("local_software_declaration");
        }
        let bytes = serde_json::to_vec(&document).unwrap();
        let selected = if software {
            let validated = serde_json::from_slice::<LocalSoftwareDeclaration>(&bytes)
                .unwrap()
                .validate(&package)
                .unwrap();
            SelectedDocument::Software {
                selection_sha256: hash(&bytes),
                declaration_digest: validated.digest().as_str().into(),
            }
        } else {
            let validated = serde_json::from_slice::<ActivationManifest>(&bytes)
                .unwrap()
                .validate(&package)
                .unwrap();
            SelectedDocument::Manifest {
                selection_sha256: hash(&bytes),
                manifest_digest: validated.digest().as_str().into(),
            }
        };
        let (plan, operations) = plan(&package, &bytes, &selected).unwrap();
        let inputs = SelectedServiceInputs {
            package_sha256: hash(PACKAGE),
            document: selected,
            environment_secret_slots: Some(serde_json::from_value(json!(["TOKEN"])).unwrap()),
            effect_sha256: "a".repeat(64),
            provenance: Value::Null,
        };
        let storage = crate::RealmOptions {
            context_root: root.into(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
        .resolve("python-read-declaration")
        .unwrap();
        let destination = root.join("installed");
        let mut installed = InstalledSelection {
            realm: storage.locator,
            session: SessionId::new(),
            job_id: "job".into(),
            validation_id: "validation".into(),
            family: "selected-python".into(),
            profile: "python".into(),
            mcp_path: destination.join("ready/mcp.json"),
            receipt_path: destination.join("ready/installed.json"),
            receipt_sha256: None,
            configuration_sha256: None,
            server_name: "fixture-python".into(),
            mcp_document_sha256: "b".repeat(64),
            recovery_request_sha256: None,
        };
        let command = destination.join("runtime/bin/python");
        let args = vec!["-I", "-B", "-m", "fixture_server.__main__"];
        let launch = json!({"mcpServers":{"fixture-python":{"command":command,"args":args,"cwd":destination,"env":{}}}});
        let access = ["actual_read".to_owned()].into_iter().collect();
        let config = secret_launch_configuration(
            &installed,
            &launch,
            &access,
            &[ServiceCredentialSlot::SelectedEnvironment("TOKEN".into())],
        )
        .unwrap();
        let mut receipt = json!({"schema_version":1,"scope":"observed_software_installation_only",
            "destination":destination,"mcp_configuration":launch,
            "command":[command,"-I","-B","-m","fixture_server.__main__"],
            "selection_sha256":inputs.document.sha256(),
            "inputs":{"package.json":inputs.package_sha256,"artifact.whl":plan["artifact_digest"]}});
        for key in ["manifest_digest", "document_kind", "declaration_digest"] {
            if let Some(value) = plan.get(key) {
                receipt[key] = value.clone();
            }
        }
        receipt["inputs"][inputs.document.filename()] = json!(inputs.document.sha256());
        for (name, digest) in plan["closure"].as_object().unwrap() {
            receipt["inputs"][format!("closure/{name}")] = digest.clone();
        }
        std::fs::create_dir_all(destination.join("inputs")).unwrap();
        std::fs::create_dir_all(destination.join("ready")).unwrap();
        std::fs::write(destination.join("inputs/package.json"), PACKAGE).unwrap();
        std::fs::write(
            destination.join("inputs").join(inputs.document.filename()),
            bytes,
        )
        .unwrap();
        std::fs::write(&installed.mcp_path, serde_json::to_vec(&launch).unwrap()).unwrap();
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        installed.receipt_sha256 = Some(hash(&receipt_bytes));
        std::fs::write(&installed.receipt_path, receipt_bytes).unwrap();
        Self {
            installed,
            config,
            inputs,
            package,
            plan,
            operations,
            receipt,
        }
    }
    fn identity(&self) -> Result<Option<SelectedPackageIdentity<'_>>, crate::ConnectError> {
        identity(
            &self.installed,
            &self.config,
            "actual_read",
            &self.inputs,
            &self.package,
            &self.plan,
            &self.operations,
            &self.receipt,
        )
    }
}

struct DefaultProgram;
impl CollectionProgram for DefaultProgram {
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("no cursor in classification controls")
    }
}
struct Program<F>(F);
impl<F> CollectionProgram for Program<F>
where
    F: Fn(&SelectedPackageIdentity<'_>, &str) -> bool + Send + Sync,
{
    fn accepts_target(&self, _: &CollectionTarget<'_>) -> bool {
        false
    }
    fn start(&self, _: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        panic!("no cursor in classification controls")
    }
    fn declares_read_only(&self, identity: &SelectedPackageIdentity<'_>, operation: &str) -> bool {
        (self.0)(identity, operation)
    }
}
fn allow(_: &SelectedPackageIdentity<'_>, _: &str) -> bool {
    true
}

#[tokio::test]
async fn both_document_forms_require_positive_trusted_exact_identity() {
    for software in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let f = Fixture::new(root.path(), software, "read");
        let calls = AtomicUsize::new(0);
        let program = Program(|actual: &SelectedPackageIdentity<'_>, operation: &str| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(actual.package(), &f.package.identity);
            assert_eq!(actual.package_sha256(), f.inputs.package_sha256);
            assert_eq!(actual.selection_document(), f.inputs.document.filename());
            assert_eq!(actual.selection_sha256(), f.inputs.document.sha256());
            assert_eq!(actual.effect_sha256(), f.inputs.effect_sha256);
            assert_eq!(
                actual.receipt_sha256(),
                f.installed.receipt_sha256.as_deref().unwrap()
            );
            assert_eq!(actual.component(), "python-server");
            assert_eq!(actual.service(), "service");
            assert_eq!(actual.artifact_sha256(), "1".repeat(64));
            actual.installed_connection() == &f.config && operation == "actual_read"
        });
        let declaration = selected(
            &f.installed,
            &f.config,
            "actual_read",
            Some(&f.inputs),
            &program,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(declaration.matches(&f.config, "actual_read"));
        assert!(!declaration.matches(&f.config, "other"));
        let mut other = f.config.clone();
        other.name = "different-installation".into();
        assert!(!declaration.matches(&other, "actual_read"));
        assert!(
            selected(
                &f.installed,
                &f.config,
                "actual_read",
                Some(&f.inputs),
                &DefaultProgram
            )
            .await
            .unwrap()
            .is_none()
        );
        let wrong = Program(|actual: &SelectedPackageIdentity<'_>, _: &str| {
            actual.artifact_sha256() == "0".repeat(64)
        });
        assert!(
            selected(
                &f.installed,
                &f.config,
                "actual_read",
                Some(&f.inputs),
                &wrong
            )
            .await
            .unwrap()
            .is_none()
        );
    }
}

#[tokio::test]
async fn package_effect_never_suffices_or_overrides_a_write_declaration() {
    for effect in ["read", "write"] {
        let root = tempfile::tempdir().unwrap();
        let f = Fixture::new(root.path(), true, effect);
        assert!(
            selected(
                &f.installed,
                &f.config,
                "actual_read",
                Some(&f.inputs),
                &DefaultProgram
            )
            .await
            .unwrap()
            .is_none()
        );
        if effect == "write" {
            let never = Program(|_: &SelectedPackageIdentity<'_>, _: &str| {
                panic!("write must not reach host classification")
            });
            assert!(
                selected(
                    &f.installed,
                    &f.config,
                    "actual_read",
                    Some(&f.inputs),
                    &never
                )
                .await
                .unwrap()
                .is_none()
            );
        }
    }
}

#[tokio::test]
async fn missing_inputs_altered_bytes_and_callback_time_replacement_refuse() {
    let root = tempfile::tempdir().unwrap();
    let f = Fixture::new(root.path(), true, "read");
    assert!(
        selected(
            &f.installed,
            &f.config,
            "actual_read",
            None,
            &Program(allow)
        )
        .await
        .is_err()
    );
    for path in [
        f.installed.receipt_path.clone(),
        root.path().join("installed/inputs/package.json"),
        root.path().join("installed/inputs/declaration.json"),
    ] {
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, b"{}").unwrap();
        assert!(
            selected(
                &f.installed,
                &f.config,
                "actual_read",
                Some(&f.inputs),
                &Program(allow)
            )
            .await
            .is_err()
        );
        std::fs::write(&path, &bytes).unwrap();
        let replace = Program(|_: &SelectedPackageIdentity<'_>, _: &str| {
            std::fs::write(&path, b"{}").unwrap();
            true
        });
        assert!(
            selected(
                &f.installed,
                &f.config,
                "actual_read",
                Some(&f.inputs),
                &replace
            )
            .await
            .is_err()
        );
        std::fs::write(&path, bytes).unwrap();
    }
    let panics =
        Program(|_: &SelectedPackageIdentity<'_>, _: &str| panic!("trusted callback failure"));
    assert!(
        selected(
            &f.installed,
            &f.config,
            "actual_read",
            Some(&f.inputs),
            &panics
        )
        .await
        .is_err()
    );
}

#[test]
fn receipt_selection_launch_access_and_complete_secret_slots_are_bound() {
    let root = tempfile::tempdir().unwrap();
    let mut f = Fixture::new(root.path(), true, "read");
    assert!(f.identity().unwrap().is_some());
    for field in [
        "schema_version",
        "scope",
        "destination",
        "selection_sha256",
        "declaration_digest",
        "inputs",
        "command",
        "mcp_configuration",
    ] {
        let original = f.receipt[field].clone();
        f.receipt[field] = json!("changed");
        assert!(f.identity().is_err(), "{field}");
        f.receipt[field] = original;
    }
    let original = f.config.clone();
    f.config.name = "different".into();
    assert!(f.identity().is_err());
    f.config = original.clone();
    f.config.access.as_mut().unwrap().allowed_operations.clear();
    assert!(f.identity().is_err());
    f.config = original.clone();
    f.config
        .access
        .as_mut()
        .unwrap()
        .allowed_operations
        .insert("undeclared".into());
    assert!(f.identity().is_err());
    f.config = original.clone();
    for mutation in ["argument", "environment", "secret-slot"] {
        let McpTransportConfig::Stdio(launch) = &mut f.config.transport else {
            panic!("stdio");
        };
        match mutation {
            "argument" => launch.args.push("--other".into()),
            "environment" => {
                launch.env.insert("INJECTED".into(), "value".into());
            }
            _ => {
                launch.secret_env.insert("OTHER_TOKEN".into());
            }
        }
        assert!(f.identity().is_err(), "{mutation}");
        f.config = original.clone();
    }
    f.inputs.environment_secret_slots = None;
    assert!(f.identity().is_err());
}

#[test]
fn typed_selection_digest_artifact_and_operation_map_cannot_drift() {
    let root = tempfile::tempdir().unwrap();
    let mut f = Fixture::new(root.path(), false, "read");
    for (field, value) in [
        ("manifest_digest", json!("0".repeat(64))),
        ("mcp_name", json!("other")),
        ("artifact_digest", json!("0".repeat(64))),
        ("closure", json!({})),
    ] {
        let original = f.plan[field].clone();
        f.plan[field] = value;
        assert!(f.identity().is_err(), "{field}");
        f.plan[field] = original;
    }
    f.plan["declared_service"]["operation_tools"]["fixture-read"] = json!("different");
    assert!(f.identity().is_err());
}

#[tokio::test]
async fn bound_documents_reject_duplicate_keys_nonfiles_and_oversize() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.json");
    let duplicates = br#"{"effect":"read","effect":"write"}"#;
    std::fs::write(&path, duplicates).unwrap();
    assert!(bound_bytes(&path, &hash(duplicates)).await.is_err());
    let oversized = vec![b' '; MAX_DOCUMENT + 1];
    std::fs::write(&path, &oversized).unwrap();
    assert!(bound_bytes(&path, &hash(&oversized)).await.is_err());
    assert!(bound_bytes(root.path(), &hash(b"{}")).await.is_err());
}
