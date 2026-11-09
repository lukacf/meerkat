//! Artifact publication for the explicit SDK-owned native fixture.
//! The selected policy and captured native identities supply every compiled
//! fact. Artifact hashes establish consistency, not grants or member readiness.

use meerkat_core::{SessionId, ToolIdentity};
use meerkat_mob::MobDefinition;
use meerkat_toolkit::host_lifetime::{
    control::{HostBootstrapSelection, SelectedHostArtifact},
    policy::{
        CompiledArtifactReference, CompiledLocalPolicy, CompiledNativeHost, CompiledNativeMember,
        PreparedLocalPolicy, validate_candidate_selection,
    },
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Write,
    os::unix::fs::{OpenOptionsExt, symlink},
    path::Path,
};
use toolkit_mobkit::installed_host::{
    read_bundle_artifact, tool_policy::PreparedInstalledToolPolicy, validate_installed_artifacts,
};
use toolkit_package::canonical_json;

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn write_new(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("fresh SDK artifact path");
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn artifact(bundle: &Path, name: &str, bytes: &[u8]) -> CompiledArtifactReference {
    let reference = CompiledArtifactReference {
        name: name.into(),
        sha256: digest(bytes),
    };
    reference.validate().unwrap();
    write_new(&bundle.join(name), bytes);
    reference
}

pub(super) fn publish(
    root: &Path,
    selected: &PreparedLocalPolicy,
    policy: &PreparedInstalledToolPolicy,
    captured: &BTreeMap<String, (SessionId, Vec<ToolIdentity>)>,
    definition: &MobDefinition,
    setup_bytes: &[u8],
) -> (HostBootstrapSelection, CompiledArtifactReference, Vec<u8>) {
    selected.validate().unwrap();
    assert_eq!(selected.native_host.members.len(), 4);
    assert_eq!(captured.len(), 4);
    assert!(
        selected
            .native_host
            .members
            .iter()
            .all(|member| { captured.contains_key(&member.member.member) })
    );
    let sessions: BTreeSet<_> = captured
        .values()
        .map(|(session, _)| session.to_string())
        .collect();
    assert_eq!(sessions.len(), 4, "four distinct captured native sessions");

    let root = root.canonicalize().unwrap();
    let bundle = root.join("shared/policy/bundle.1");
    assert!(bundle.is_dir());
    assert!(root.join("shared/activations").is_dir());

    let source = artifact(&bundle, "policy.json", &canonical_json(selected).unwrap());
    let tool_policy = artifact(
        &bundle,
        "application-tool-policy.json",
        &policy.compiled().canonical_json().unwrap(),
    );
    let setup = artifact(&bundle, "application-setup.json", setup_bytes);
    let mut artifacts = vec![source.clone(), tool_policy.clone(), setup.clone()];
    let mut capture_records = Vec::new();
    let members = selected
        .native_host
        .members
        .iter()
        .map(|member| {
            let (session, identities) = &captured[&member.member.member];
            let name = format!(
                "member-surface-{}.json",
                digest(&canonical_json(&member.member).unwrap()),
            );
            let surface = artifact(&bundle, &name, &canonical_json(identities).unwrap());
            capture_records.push(json!({
                "member": member.member,
                "session_id": session,
                "initial_surface": surface,
            }));
            artifacts.push(surface.clone());
            CompiledNativeMember {
                member: member.member.clone(),
                logical_owner: member.logical_owner.clone(),
                context_generation: member.context_generation,
                audience: member.audience.clone(),
                initial_surface: surface,
                setup_capability: member.setup_capability.clone(),
            }
        })
        .collect();
    artifacts.push(artifact(
        &bundle,
        "native-surface-captures.json",
        &canonical_json(&capture_records).unwrap(),
    ));

    // PreparedSetupApplication accepts the trusted factory's artifact bytes;
    // it has no Forge-specific schema. This SDK factory serializes its actual
    // composed definition and verifies that exact typed payload on readback.
    let application_bytes = canonical_json(definition).unwrap();
    let application = artifact(&bundle, "sdk-application.json", &application_bytes);
    artifacts.push(application.clone());
    let local = CompiledLocalPolicy {
        schema: 7,
        deployment: selected.deployment.clone(),
        source_policy_sha256: source.sha256,
        components: selected.components.clone(),
        channels: selected.channels.clone(),
        native_host: Some(CompiledNativeHost {
            authentication_domain: selected.native_host.authentication_domain.clone(),
            event_domain: selected.native_host.event_domain.clone(),
            realm: selected.native_host.realm.clone(),
            grant_administrator: selected.native_host.grant_administrator.clone(),
            remote_operator: selected.native_host.remote_operator.clone(),
            service_parent_administration: selected
                .native_host
                .service_parent_administration
                .clone(),
            ingress_domain: selected.native_host.ingress_domain.clone(),
            application_tool_policy: tool_policy,
            setup: Some(setup),
            member_classes: selected.native_host.member_classes.clone(),
            members,
            routes: selected.native_host.routes.clone(),
        }),
    };
    assert!(selected.matches(&local));
    let local_reference = artifact(
        &bundle,
        "toolkit-local-policy.json",
        &local.canonical_bytes().unwrap(),
    );
    artifacts.push(local_reference.clone());
    artifacts.sort_by(|left, right| left.name.cmp(&right.name));
    assert!(
        artifacts
            .windows(2)
            .all(|pair| pair[0].name != pair[1].name)
    );
    let rows: Vec<_> = artifacts
        .iter()
        .map(|reference| json!({"path": reference.name, "sha256": reference.sha256}))
        .collect();
    let manifest = artifact(
        &bundle,
        "manifest.json",
        &canonical_json(&json!({"artifacts": rows})).unwrap(),
    );
    let activation_bytes = canonical_json(&json!({
        "activation_id": "1",
        "policy_bundle": {
            "path": "shared/policy/bundle.1",
            "policy_digest": format!("policy-v1:sha256:{}", manifest.sha256),
        },
    }))
    .unwrap();
    let activation_path = root.join("shared/activations/1.json");
    write_new(&activation_path, &activation_bytes);
    symlink("shared/activations/1.json", root.join("current")).unwrap();
    let selection = HostBootstrapSelection {
        deployment_root: root,
        activation: SelectedHostArtifact {
            path: activation_path,
            sha256: digest(&activation_bytes),
        },
        policy_manifest: SelectedHostArtifact {
            path: bundle.join("manifest.json"),
            sha256: manifest.sha256,
        },
        local_policy: SelectedHostArtifact {
            path: bundle.join(&local_reference.name),
            sha256: local_reference.sha256,
        },
    };

    let (read_local, _) = validate_candidate_selection(&selection).unwrap();
    assert_eq!(read_local, local);
    for reference in &artifacts {
        read_bundle_artifact(&selection, reference).unwrap();
    }
    validate_installed_artifacts(read_local.native_host.as_ref().unwrap(), |reference| {
        read_bundle_artifact(&selection, reference)
    })
    .unwrap();
    let verified_application = read_bundle_artifact(&selection, &application).unwrap();
    assert_eq!(verified_application, application_bytes);
    let read_definition: MobDefinition = serde_json::from_slice(&verified_application).unwrap();
    assert_eq!(&read_definition, definition);
    (selection, application, verified_application)
}
