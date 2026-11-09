use serde_json::{Value, json};
use toolkit_package::*;

const FIXTURE: &str = r###"{"base_documents":{"deploy/settings-template.json":"{\"apiVersion\":\"v1\",\"data\":{\"configuration\":\"old\",\"policy\":\"old\",\"untouched\":\"retain\"},\"kind\":\"ConfigMap\",\"metadata\":{\"labels\":{\"preserved\":\"yes\"},\"name\":\"settings-template\",\"namespace\":\"fixture\"}}","deploy/worker.json":"{\"apiVersion\":\"apps/v1\",\"kind\":\"Deployment\",\"metadata\":{\"labels\":{\"preserved\":\"yes\"},\"name\":\"worker\",\"namespace\":\"fixture\"},\"spec\":{\"replicas\":2,\"selector\":{\"matchLabels\":{\"app\":\"worker\"}},\"template\":{\"metadata\":{\"annotations\":{\"preserved\":\"yes\"},\"labels\":{\"app\":\"worker\"}},\"spec\":{\"containers\":[{\"args\":[\"old\"],\"command\":[\"old/server\"],\"image\":\"old.example/worker@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"name\":\"worker\",\"ports\":[{\"containerPort\":8080}],\"volumeMounts\":[{\"mountPath\":\"/etc/toolkit\",\"name\":\"settings\",\"readOnly\":true}],\"workingDir\":\"/\"}],\"volumes\":[{\"configMap\":{\"items\":[{\"key\":\"configuration\",\"path\":\"config.json\"},{\"key\":\"policy\",\"path\":\"policy.json\"}],\"name\":\"settings-template\"},\"name\":\"settings\"}]}}}}"},"configuration":"{\"port\":8080}\n","manifest_json":"{\"components\":[{\"activation_scope\":{\"kind\":\"independent\"},\"dependencies\":[],\"host_services\":[],\"id\":\"server\",\"lifetime\":\"deployment\",\"placement\":\"kubernetes\",\"readiness\":{\"checks\":[\"rollout-and-role\"],\"owner\":\"host\"},\"required_contracts\":[],\"rollback\":{\"kind\":\"owner_reconciliation\",\"operation\":\"reconcile\",\"owner\":\"host\"},\"runtime\":\"container\",\"shutdown\":{\"maximum_wait_ms\":30000,\"operation\":\"stop\",\"owner\":\"host\",\"timeout\":\"preserve_uncertainty\"},\"source\":{\"content_digest\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"dependencies\":{\"kind\":\"self_contained\"},\"entrypoint\":{\"arguments\":[\"--config\",\"/etc/toolkit/config.json\",\"--policy\",\"/etc/toolkit/policy.json\"],\"executable\":\"usr/local/bin/server\"},\"kind\":\"artifact\",\"location\":\"https://registry.example/v2/example/server/manifests/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"},\"state\":[],\"version\":\"1\"}],\"configuration\":{\"content_digest\":\"3c6ed5470654cc81dac66f76a7abb23403301bd80843ffc7c365da118d61db13\",\"owner\":\"host\",\"reference\":\"config\",\"revision\":\"1\"},\"exports\":[{\"authentication\":{\"kind\":\"none\"},\"cleanup\":\"release\",\"components\":[\"server\"],\"diagnostics\":\"inspect\",\"enforcement_contract\":{\"name\":\"host/enforcement\",\"version\":{\"major\":1,\"minimum_minor\":0}},\"events\":[],\"id\":\"service\",\"operations\":[{\"declared_effect\":\"read\",\"id\":\"read\",\"required_authority\":[],\"resources\":{\"kind\":\"connection_wide\"},\"result_limits\":{\"maximum_bytes\":1024,\"maximum_items\":10},\"schema_digest\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"}],\"role\":\"service_connector\"}],\"host_services\":[],\"package\":{\"name\":\"connector\",\"publisher\":\"test\",\"version\":\"1.0.0\"},\"package_content_digest\":\"9dc47799d27265323dce75f52a874f472b3cfceca0eb90051acd18dae237419c\",\"policy\":{\"content_digest\":\"0d6d941f38793d84250df0a980285772833d6fbb484a282c0aaa1b3b17d14006\",\"owner\":\"host\",\"revision\":\"1\"},\"portable_mcp_entries\":[],\"required_contracts\":[],\"schema_version\":1,\"strategy\":\"container\"}","package_json":"{\"assessments\":[],\"description\":\"Offline artifact preparation fixture\",\"display_name\":\"Test connector\",\"identity\":{\"name\":\"connector\",\"publisher\":\"test\",\"version\":\"1.0.0\"},\"known_exports\":[],\"provenance\":{\"license\":\"MIT\",\"local_patches\":[],\"notices\":[],\"source\":\"https://example.org/source\",\"source_revision\":\"commit1\"},\"schema_version\":1,\"strategies\":[{\"id\":\"container\",\"instructions\":\"Use the pinned host profile.\",\"kind\":\"native_adapter\",\"known_requirements\":[],\"sources\":[],\"unresolved_details\":[]}]}","policy":"{\"mode\":\"read_only\"}\n","profile_json":"{\"config_map\":{\"configuration_file\":\"config.json\",\"configuration_key\":\"configuration\",\"mount_path\":\"/etc/toolkit\",\"name\":\"settings-template\",\"namespace\":\"fixture\",\"path\":\"deploy/settings-template.json\",\"policy_file\":\"policy.json\",\"policy_key\":\"policy\",\"sha256\":\"3364da751f6adc1ea6e6f2f389bd9a226700c0c476a7aa3678470e9409d2e9de\",\"snapshot_directory\":\"deploy/snapshots\",\"volume\":\"settings\"},\"deployment\":{\"container\":\"worker\",\"name\":\"worker\",\"namespace\":\"fixture\",\"path\":\"deploy/worker.json\",\"sha256\":\"2321c101c2bc7213ff476aa12685a8b7ddfffdf25c9f75df74cfa72e1068b43f\"},\"host\":{\"contracts\":{\"host/enforcement\":{\"major\":1,\"minor\":0}},\"placements\":[\"kubernetes\"],\"runtimes\":[\"container\"],\"services\":{}},\"owner\":\"host\",\"placement\":\"kubernetes\",\"schema_version\":1,\"source_commit\":\"dddddddddddddddddddddddddddddddddddddddd\",\"source_repository\":\"https://example.org/deployment.git\"}","profile_sha256":"aa3a791d5897baf6aa18fb823c655ab5705522709b13839b52c2828ecc8c06ef","schema_version":1}"###;

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).unwrap()
}
fn encoded(value: &Value) -> String {
    String::from_utf8(canonical_json(value).unwrap()).unwrap()
}
fn set_profile(request: &mut Value, profile: Value) {
    let raw = encoded(&profile);
    request["profile_sha256"] = json!(ContentDigest::of_bytes(raw.as_bytes()));
    request["profile_json"] = json!(raw);
}
fn inner(request: &Value, field: &str) -> Value {
    serde_json::from_str(request[field].as_str().unwrap()).unwrap()
}
fn set_base(request: &mut Value, target: &str, document: Value) {
    let mut profile = inner(request, "profile_json");
    let path = profile[target]["path"].as_str().unwrap().to_owned();
    let raw = encoded(&document);
    profile[target]["sha256"] = json!(ContentDigest::of_bytes(raw.as_bytes()));
    request["base_documents"][path] = json!(raw);
    set_profile(request, profile);
}
fn base(request: &Value, target: &str) -> Value {
    let profile = inner(request, "profile_json");
    let path = profile[target]["path"].as_str().unwrap();
    serde_json::from_str(request["base_documents"][path].as_str().unwrap()).unwrap()
}
fn prepare(request: &Value) -> Result<PreparedKubernetesGitOps, GitOpsPreparationError> {
    prepare_kubernetes_gitops_json(&canonical_json(request).unwrap())
}

#[test]
fn prepares_exact_immutable_snapshot_preserving_host_fields_and_inputs() {
    let request = fixture();
    let original = request.clone();
    let result = prepare(&request).unwrap();
    assert_eq!(result.status, "prepared");
    assert_eq!(result.schema_version, 1);
    assert_eq!(request, original);
    assert_eq!(result.proposal.files.len(), 2);
    assert_eq!(result.proposal.base_sha256.len(), 2);
    assert!(
        !result
            .proposal
            .files
            .contains_key("deploy/settings-template.json")
    );
    let snapshot = result
        .proposal
        .files
        .iter()
        .find(|(_, file)| file.action == "create")
        .unwrap();
    assert!(snapshot.0.starts_with("deploy/snapshots/toolkit-"));
    let snapshot_value: Value = serde_json::from_str(&snapshot.1.contents).unwrap();
    let name = snapshot_value["metadata"]["name"].as_str().unwrap();
    assert_eq!(name.len(), 56);
    let mut expected_map = base(&request, "config_map");
    expected_map["metadata"]["name"] = json!(name);
    expected_map["immutable"] = json!(true);
    expected_map["data"]["configuration"] = request["configuration"].clone();
    expected_map["data"]["policy"] = request["policy"].clone();
    assert_eq!(snapshot_value, expected_map);
    let mut expected_deployment = base(&request, "deployment");
    let container = &mut expected_deployment["spec"]["template"]["spec"]["containers"][0];
    container["image"] = json!(format!(
        "registry.example/example/server@sha256:{}",
        "a".repeat(64)
    ));
    container["command"] = json!(["usr/local/bin/server"]);
    container["args"] = json!([
        "--config",
        "/etc/toolkit/config.json",
        "--policy",
        "/etc/toolkit/policy.json"
    ]);
    expected_deployment["spec"]["template"]["spec"]["volumes"][0]["configMap"]["name"] =
        json!(name);
    let annotations = &mut expected_deployment["spec"]["template"]["metadata"]["annotations"];
    annotations["toolkit.rkat.ai/manifest-sha256"] = json!(result.proposal.manifest_sha256);
    annotations["toolkit.rkat.ai/configuration-sha256"] =
        json!(result.proposal.configuration_sha256);
    annotations["toolkit.rkat.ai/policy-sha256"] = json!(result.proposal.policy_sha256);
    let actual: Value =
        serde_json::from_str(&result.proposal.files["deploy/worker.json"].contents).unwrap();
    assert_eq!(actual, expected_deployment);
    for file in result.proposal.files.values() {
        assert!(file.contents.ends_with('\n'));
        assert_eq!(
            file.sha256,
            ContentDigest::of_bytes(file.contents.as_bytes())
        );
    }
    assert_eq!(
        result.proposal_sha256,
        ContentDigest::of_bytes(&canonical_json(&result.proposal).unwrap())
    );
    assert_eq!(
        result.proposal_sha256,
        prepare(&request).unwrap().proposal_sha256
    );
}

#[test]
fn exact_byte_and_authority_bindings_cannot_be_replayed_with_changed_inputs() {
    let request = fixture();
    let original = prepare(&request).unwrap();
    for field in ["configuration", "policy", "profile_json"] {
        let mut altered = request.clone();
        altered[field] = json!(format!("{} ", request[field].as_str().unwrap()));
        assert_eq!(
            prepare(&altered).unwrap_err(),
            GitOpsPreparationError::BindingMismatch
        );
    }
    let mut altered = request.clone();
    altered["base_documents"]["deploy/worker.json"] = json!(format!(
        "{} ",
        request["base_documents"]["deploy/worker.json"]
            .as_str()
            .unwrap()
    ));
    assert_eq!(
        prepare(&altered).unwrap_err(),
        GitOpsPreparationError::BindingMismatch
    );
    let mut altered = request.clone();
    let mut template = base(&altered, "config_map");
    template["data"]["untouched"] = json!("new host content");
    set_base(&mut altered, "config_map", template);
    let changed = prepare(&altered).unwrap();
    assert_ne!(changed.proposal_sha256, original.proposal_sha256);
    assert_ne!(
        changed.proposal.files.keys().collect::<Vec<_>>(),
        original.proposal.files.keys().collect::<Vec<_>>()
    );
    let mut profile = inner(&altered, "profile_json");
    profile["source_repository"] = json!("https://example.org/other.git");
    set_profile(&mut altered, profile);
    assert_ne!(
        prepare(&altered).unwrap().proposal_sha256,
        changed.proposal_sha256
    );
}

#[test]
fn rejects_shared_or_ambiguous_mounts_and_controller_owned_templates() {
    for family in ["containers", "initContainers", "ephemeralContainers"] {
        let mut request = fixture();
        let mut document = base(&request, "deployment");
        let shared = json!({"name":"other", "image":"example", "volumeMounts":[{"name":"settings", "mountPath":"/etc/other"}]});
        if family == "containers" {
            document["spec"]["template"]["spec"][family]
                .as_array_mut()
                .unwrap()
                .push(shared);
        } else {
            document["spec"]["template"]["spec"][family] = json!([shared]);
        }
        set_base(&mut request, "deployment", document);
        assert_eq!(
            prepare(&request).unwrap_err(),
            GitOpsPreparationError::InvalidTarget
        );
    }
    for field in [
        "uid",
        "resourceVersion",
        "ownerReferences",
        "finalizers",
        "deletionTimestamp",
        "generateName",
    ] {
        let mut request = fixture();
        let mut document = base(&request, "config_map");
        document["metadata"][field] = json!("must not be inherited");
        set_base(&mut request, "config_map", document);
        assert_eq!(
            prepare(&request).unwrap_err(),
            GitOpsPreparationError::InvalidTarget
        );
    }
    for target in ["namespace", "name"] {
        let mut request = fixture();
        let mut document = base(&request, "deployment");
        document["metadata"][target] = json!("wrong");
        set_base(&mut request, "deployment", document);
        assert_eq!(
            prepare(&request).unwrap_err(),
            GitOpsPreparationError::InvalidTarget
        );
    }
}

#[test]
fn rejects_paths_missing_targets_duplicates_and_host_capability_mismatch() {
    for path in [
        "../escape",
        "/absolute",
        "deploy\\escape",
        "deploy/worker.json/child",
    ] {
        let mut request = fixture();
        let mut profile = inner(&request, "profile_json");
        profile["config_map"]["snapshot_directory"] = json!(path);
        set_profile(&mut request, profile);
        assert!(prepare(&request).is_err());
    }
    let mut request = fixture();
    let mut profile = inner(&request, "profile_json");
    profile["host"]["contracts"] = json!({});
    set_profile(&mut request, profile);
    assert_eq!(
        prepare(&request).unwrap_err(),
        GitOpsPreparationError::IncompatibleHost
    );
    let mut request = fixture();
    request["base_documents"]
        .as_object_mut()
        .unwrap()
        .remove("deploy/worker.json");
    assert!(prepare(&request).is_err());
    let mut request = fixture();
    let mut document = base(&request, "deployment");
    let duplicate = document["spec"]["template"]["spec"]["containers"][0].clone();
    document["spec"]["template"]["spec"]["containers"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    set_base(&mut request, "deployment", document);
    assert_eq!(
        prepare(&request).unwrap_err(),
        GitOpsPreparationError::InvalidTarget
    );
    assert_eq!(
        prepare_kubernetes_gitops_json(b"{\"schema_version\":1,\"schema_version\":1}").unwrap_err(),
        GitOpsPreparationError::InvalidInput
    );
}

#[test]
fn refuses_unsupported_execution_and_preserves_full_lifecycle_intent() {
    for repository in ["example/a..b", "example/a__-b", "example/a_", "example/.a"] {
        let mut request = fixture();
        let mut manifest = inner(&request, "manifest_json");
        manifest["components"][0]["source"]["location"] = json!(format!(
            "https://registry.example/v2/{repository}/manifests/sha256:{}",
            "a".repeat(64)
        ));
        request["manifest_json"] = json!(encoded(&manifest));
        assert_eq!(
            prepare(&request).unwrap_err(),
            GitOpsPreparationError::UnsupportedSelection
        );
    }
    for replacement in [
        json!("https://registry.example/v2/example/server/manifests/latest"),
        json!(
            "https://registry.example:8443/v2/example/server/manifests/sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ),
    ] {
        let mut request = fixture();
        let mut manifest = inner(&request, "manifest_json");
        manifest["components"][0]["source"]["location"] = replacement;
        request["manifest_json"] = json!(encoded(&manifest));
        assert!(prepare(&request).is_err());
    }
    let mut request = fixture();
    let mut manifest = inner(&request, "manifest_json");
    manifest["components"][0]["source"]["entrypoint"]["arguments"] = json!(["$(SECRET)"]);
    request["manifest_json"] = json!(encoded(&manifest));
    assert_eq!(
        prepare(&request).unwrap_err(),
        GitOpsPreparationError::UnsupportedSelection
    );
    let request = fixture();
    let result = prepare(&request).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&result.proposal.manifest_json).unwrap(),
        inner(&request, "manifest_json")
    );
}

#[test]
fn cli_has_bounded_fixed_error_surface_and_no_partial_proposal() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-gitops-prepare"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"private_path\":\"/host/secret\"}")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"invalid_input\n");
    assert_eq!(
        prepare_kubernetes_gitops_json(&vec![b' '; GITOPS_MAX_INPUT + 1]).unwrap_err(),
        GitOpsPreparationError::InputLimit
    );
}
