//! Independent regressions from the package contract adversarial review.
use serde_json::{Value, json};
use toolkit_package::*;

fn package() -> IntegrationPackage {
    serde_json::from_value(json!({
        "schema_version": 1,
        "identity": {"publisher": "review", "name": "connector", "version": "1"},
        "display_name": "Review fixture", "description": "Adversarial package fixture",
        "provenance": {"source": "https://example.org/source", "source_revision": "rev1", "license": "MIT"},
        "strategies": [{"id": "research", "kind": "research_skill", "instructions": "Inspect the request"}]
    })).unwrap()
}

fn contract(name: &str) -> Value {
    json!({"name": name, "version": {"major": 1, "minimum_minor": 0}})
}

fn selection() -> Value {
    let package = package();
    let digest = ContentDigest::of_bytes(b"review fixture");
    json!({
        "schema_version": 1,
        "package": package.identity,
        "package_content_digest": package.content_digest().unwrap(),
        "strategy": "research",
        "configuration": {"owner": "config", "reference": "selected", "revision": "1", "content_digest": digest},
        "policy": {"owner": "policy", "revision": "1", "content_digest": digest},
        "components": [{
            "id": "server", "version": "1",
            "source": {"kind": "artifact", "location": "https://example.org/package.tar",
                "content_digest": digest, "dependencies": {"kind": "self_contained"},
                "entrypoint": {"executable": "bin/server", "arguments": []}},
            "runtime": "native", "placement": "local", "lifetime": "deployment",
            "activation_scope": {"kind": "independent"},
            "readiness": {"owner": "runtime", "checks": ["process"]},
            "shutdown": {"owner": "runtime", "operation": "stop", "maximum_wait_ms": 1000,
                "timeout": "preserve_uncertainty"},
            "rollback": {"kind": "owner_reconciliation", "owner": "runtime", "operation": "reconcile"}
        }],
        "exports": [{
            "role": "service_connector", "id": "service", "components": ["server"],
            "authentication": {"kind": "none"},
            "operations": [{"id": "read", "schema_digest": digest, "declared_effect": "read",
                "required_authority": [], "resources": {"kind": "connection_wide"},
                "result_limits": {"maximum_bytes": 1000, "maximum_items": 10}}],
            "enforcement_contract": contract("policy"), "diagnostics": "check", "cleanup": "release"
        }]
    })
}

fn validated(value: Value) -> Result<ValidatedManifest, ValidationError> {
    serde_json::from_value::<ActivationManifest>(value)
        .unwrap()
        .validate(&package())
}

#[test]
fn review_fixture_is_valid() {
    assert!(validated(selection()).is_ok());
}

#[test]
fn persistent_frontend_cannot_use_request_lifetime_component() {
    let mut selected = selection();
    selected["components"][0]["lifetime"] = json!("request");
    selected["exports"] = json!([{
        "role": "agent_frontend", "id": "frontend", "components": ["server"],
        "modalities": ["text"], "admission_contract": contract("admission"),
        "conversation_contract": contract("conversation"), "delivery_contract": contract("delivery"),
        "authentication": {"kind": "none"}, "diagnostics": "check", "cleanup": "pause"
    }]);
    assert!(
        validated(selected).is_err(),
        "request cleanup would stop an active frontend"
    );
}

#[test]
fn deployment_frontend_and_request_connector_remain_valid() {
    let mut selected = selection();
    selected["components"][0]["lifetime"] = json!("request");
    assert!(validated(selected).is_ok());

    let mut selected = selection();
    selected["exports"] = json!([{
        "role": "agent_frontend", "id": "frontend", "components": ["server"],
        "modalities": ["text"], "admission_contract": contract("admission"),
        "conversation_contract": contract("conversation"), "delivery_contract": contract("delivery"),
        "authentication": {"kind": "none"}, "diagnostics": "check", "cleanup": "pause"
    }]);
    assert!(validated(selected).is_ok());
}

#[test]
fn portable_entry_cannot_omit_manifest_required_contract() {
    let mut selected = selection();
    selected["required_contracts"] = json!([contract("required-domain-contract")]);
    selected["portable_mcp_entries"] = json!([{"name": "mcp", "component": "server"}]);
    assert!(
        validated(selected).is_err(),
        "generic client ignores a required toolkit contract"
    );
}

#[test]
fn portable_entry_cannot_omit_manifest_required_host_service() {
    let mut selected = selection();
    selected["host_services"] = json!([{
        "name": "required-domain-service", "version": {"major": 1, "minimum_minor": 0},
        "authority": ["device-control"], "availability": {"kind": "required"}
    }]);
    selected["portable_mcp_entries"] = json!([{"name": "mcp", "component": "server"}]);
    assert!(
        validated(selected).is_err(),
        "generic client ignores a required host service"
    );
}

#[test]
fn portable_entry_can_omit_an_explicitly_optional_manifest_host_service() {
    let mut selected = selection();
    selected["host_services"] = json!([{
        "name": "optional-diagnostics", "version": {"major": 1, "minimum_minor": 0},
        "authority": [], "availability": {
            "kind": "optional", "degraded_behavior": "Use the connector without host diagnostics"
        }
    }]);
    selected["portable_mcp_entries"] = json!([{"name": "mcp", "component": "server"}]);
    assert!(validated(selected).is_ok());
}

#[test]
fn artifact_entrypoint_cannot_be_windows_absolute_or_drive_relative_path() {
    for path in [
        "C:/Windows/System32/cmd.exe",
        "C:outside.exe",
        "bin/server:alternate",
    ] {
        let mut selected = selection();
        selected["components"][0]["source"]["entrypoint"]["executable"] = json!(path);
        assert!(
            validated(selected).is_err(),
            "accepted non-relative path: {path}"
        );
    }
}

#[cfg(unix)]
#[test]
fn fifo_child_probe() {
    let Ok(root) = std::env::var("TOOLKIT_PACKAGE_REVIEW_FIFO_ROOT") else {
        return;
    };
    let result = PluginRoot::open(root).unwrap().read("./pipe", 1024);
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn plugin_metadata_fifo_is_rejected_without_blocking() {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    assert!(
        Command::new("mkfifo")
            .arg(directory.path().join("pipe"))
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fifo_child_probe", "--nocapture"])
        .env("TOOLKIT_PACKAGE_REVIEW_FIFO_ROOT", directory.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("PluginRoot::read blocked opening a FIFO before checking its type");
        }
        thread::sleep(Duration::from_millis(20));
    }
}
