//! Publish selected definitions before native construction. No SessionId or
//! native capture is serialized here. Actual held reports are checked later.
use super::*;
pub(in crate::resources::shared_setup) fn publish_expected(
    root: &Path,
    selected: &PreparedLocalPolicy,
    policy: &PreparedInstalledToolPolicy,
    expected: &BTreeMap<String, Vec<ToolIdentity>>,
    definition: &MobDefinition,
) -> (HostBootstrapSelection, CompiledArtifactReference, Vec<u8>) {
    selected.validate().unwrap();
    assert_eq!(selected.native_host.members.len(), expected.len());
    assert!(
        selected
            .native_host
            .members
            .iter()
            .all(|member| expected.contains_key(&member.member.member))
    );
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
    let mut artifacts = vec![source.clone(), tool_policy.clone()];
    let members = selected
        .native_host
        .members
        .iter()
        .map(|member| {
            let identities = &expected[&member.member.member];
            let name = format!(
                "member-surface-{}.json",
                digest(&canonical_json(&member.member).unwrap()),
            );
            let surface = artifact(&bundle, &name, &canonical_json(identities).unwrap());
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
            setup: None,
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
