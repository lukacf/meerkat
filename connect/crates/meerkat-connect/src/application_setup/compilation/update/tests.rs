//! Source controls over the actual read-only validator. The predecessor is a
//! mechanical artifact fixture, not evidence of native construction or finish.
use super::*;
use crate::application_setup::{ApplicationSetupDefinition, ApplicationSetupTools};
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_core::{PolicyId, PolicyProviderId, PolicyRevision};
use meerkat_toolkit::{authority::Ceiling, host_lifetime::policy::{
    CompiledAgentSetupCapability, CompiledArtifactReference, CompiledLocalPolicy,
    CompiledNativeHost, CompiledNativeMember, CompiledNativeSourceOrigin,
    CompiledNativeSourceRoute, PreparedNativeHost,
}};
use serde_json::json;
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}};
use toolkit_package::{Id, canonical_json};

fn pin(path: &Path, bytes: &[u8]) -> SelectedHostArtifact {
    std::fs::write(path, bytes).unwrap();
    SelectedHostArtifact { path: path.into(), sha256: hash(bytes) }
}
fn reference(name: &str, bytes: &[u8]) -> CompiledArtifactReference {
    CompiledArtifactReference { name: name.into(), sha256: hash(bytes) }
}

struct Fixture {
    root: PathBuf,
    input: ApplicationSetupCompilationInput,
    prepared: PreparedApplicationSetup,
    policy: CompiledLocalPolicy,
    acquisition: ApplicationAcquisitionSelection,
    original_policy_bytes: Vec<u8>,
    selection: ApplicationUpdateSelection,
}

impl Fixture {
    fn new(root: &Path) -> Self {
        let legacy = crate::application_setup::tests::input(root);
        let mut realm = legacy.realm;
        realm.realm_id = meerkat_core::mob_realm_id(&legacy.helper.member.mob_id).unwrap().to_string();
        let mut input = ApplicationSetupCompilationInput {
            schema: 4, realm, definition: legacy.definition, roster: legacy.roster,
            members: legacy.native_members.into_iter().map(|row| super::super::ApplicationMemberSelection {
                member: row.member, logical_owner: row.logical_owner,
                context_generation: row.context_generation, audience: row.audience,
                setup_capability: row.setup_capability,
            }).collect(), helper: legacy.helper, helper_tools: legacy.helper_tools,
            helper_profile: legacy.helper_profile, tool_policy: legacy.tool_policy,
        };
        let parent = input.helper.controller_parent[0].clone();
        input.members[0].setup_capability = Some(CompiledAgentSetupCapability {
            helper: None,
            route: Id::new("original-route").unwrap(), parent, helper_targets: vec![],
        });
        let resources = crate::application_setup::tests::prepared(root);
        let source_helper_member = input.helper.member.clone();
        let projection = input.helper_profile.project(resources.connection,
            source_helper_member.clone(), input.helper_tools).unwrap();
        let mut definition = input.definition.resolve().unwrap();
        projection.compose_definition(&mut definition).unwrap();
        let mut helper = input.helper.clone();
        helper.member = projection.member().clone();
        let mut members = input.members.clone();
        members[1].member = helper.member.clone();
        let principal = members[0].logical_owner.clone();
        let domain = ResourceDomain { authority: principal.clone(), namespace: "original".into() };
        let route = CompiledNativeSourceRoute {
            id: Id::new("original-route").unwrap(),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member: members[0].member.clone(), requester: principal.clone(), ingress_actor: principal.clone(),
            }, application_parent: None, controller_parent: None,
            logical_executor: principal.clone(), represented_subject: None, source_authority: principal.clone(),
            authentication_domain: domain.clone(), event_domain: domain.clone(),
        };
        let owner = PreparedLocalPolicy {
            deployment: Id::new("update-fixture").unwrap(), components: vec![], channels: vec![],
            native_host: PreparedNativeHost {
                realm: meerkat_core::mob_realm_id(&helper.member.mob_id).unwrap(),
                grant_administrator: principal, remote_operator: None, ingress_domain: domain,
                service_parent_administration: None,
                members: members.clone(), routes: vec![route],
            },
        };
        let mut native_members = Vec::new();
        for row in members {
            let mut identities = vec![meerkat_core::ToolDef::new("research", "old surface", json!({})).identity()];
            if row.member == helper.member {
                identities.push(toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity());
            }
            let bytes = canonical_json(&identities).unwrap();
            let name = super::super::member_surface_name(&row.member).unwrap();
            std::fs::write(resources.artifact_directory.join(&name), &bytes).unwrap();
            native_members.push(CompiledNativeMember {
                member: row.member, logical_owner: row.logical_owner, context_generation: row.context_generation,
                audience: row.audience, initial_surface: reference(&name, &bytes), setup_capability: row.setup_capability,
            });
        }
        let original_policy_bytes = input.tool_policy.clone().compile().unwrap();
        let handoff = resources.artifact_directory.join("prepare.stdout");
        let prepared = PreparedApplicationSetup {
            schema: 4, native_owner: Some(owner.clone()), input_sha256: hash(&canonical_json(&input).unwrap()),
            realm: input.realm.clone(), definition_source: input.definition.clone(), definition,
            source_helper_roster: input.roster[1].clone(), native_members: native_members.clone(),
            source_helper_member, helper, helper_source_surface: None,
            helper_tools: input.helper_tools, helper_profile: input.helper_profile, server: projection.server().clone(),
            resources: crate::application_setup::PreparedApplicationResources {
                artifact_directory: resources.artifact_directory,
                handoff: SelectedHostArtifact { path: handoff.clone(), sha256: hash(&std::fs::read(handoff).unwrap()) },
            }, application_tool_policy: reference("application-tool-policy.json", &original_policy_bytes),
        };
        let output_audience = BTreeSet::from([Id::new("operator").unwrap()]);
        let acquisition = ApplicationAcquisitionSelection {
            schema: 1, console_setup: None, local_operator_recovery: None, standing_policy: None,
            prepared_application_setup: reference("setup.json", &canonical_json(&prepared).unwrap()),
            backend_generation: Id::new("retained-backend").unwrap(),
            proposal_ceiling: Ceiling { actions: BTreeSet::new(), services: BTreeSet::new(),
                frontends: BTreeSet::new(), output_audiences: output_audience.clone() },
            output_audience, limits: Default::default(),
        };
        let policy = CompiledLocalPolicy {
            schema: 7, deployment: owner.deployment, source_policy_sha256: hash(b"{}"),
            components: owner.components, channels: owner.channels,
            native_host: Some(CompiledNativeHost {
                realm: owner.native_host.realm, grant_administrator: owner.native_host.grant_administrator,
                remote_operator: owner.native_host.remote_operator, ingress_domain: owner.native_host.ingress_domain,
                service_parent_administration: owner.native_host.service_parent_administration,
                application_tool_policy: prepared.application_tool_policy.clone(),
                setup: Some(reference("acquisition.json", &canonical_json(&acquisition).unwrap())),
                members: native_members, routes: owner.native_host.routes,
            }),
        };
        let placeholder = SelectedHostArtifact { path: root.join("unused"), sha256: String::new() };
        let selection = ApplicationUpdateSelection {
            update_id: "existing-update-1".into(), predecessor: HostBootstrapSelection {
                deployment_root: root.into(), activation: placeholder.clone(),
                policy_manifest: placeholder.clone(), local_policy: placeholder.clone(),
            }, compilation_input: placeholder.clone(), application_tool_policy: placeholder,
        };
        let mut result = Self { root: root.into(), input, prepared, policy, acquisition, original_policy_bytes, selection };
        result.write_predecessor();
        result.write_candidate(true);
        result
    }

    fn write_predecessor(&mut self) {
        let bundle = self.root.join("original-bundle");
        std::fs::create_dir_all(&bundle).unwrap();
        let mut files = BTreeMap::new();
        files.insert("policy.json".to_owned(), b"{}".to_vec());
        files.insert("application-tool-policy.json".into(), self.original_policy_bytes.clone());
        let setup = canonical_json(&self.prepared).unwrap();
        self.acquisition.prepared_application_setup = reference("setup.json", &setup);
        let acquisition = canonical_json(&self.acquisition).unwrap();
        self.policy.native_host.as_mut().unwrap().setup = Some(reference("acquisition.json", &acquisition));
        files.insert("setup.json".into(), setup);
        files.insert("acquisition.json".into(), acquisition);
        for row in &self.prepared.native_members {
            files.insert(row.initial_surface.name.clone(), std::fs::read(
                self.prepared.resources.artifact_directory.join(&row.initial_surface.name)).unwrap());
        }
        files.insert("toolkit-local-policy.json".into(), self.policy.canonical_bytes().unwrap());
        for (name, bytes) in &files { std::fs::write(bundle.join(name), bytes).unwrap(); }
        let manifest = canonical_json(&json!({"artifacts":files.iter().map(|(name,bytes)|
            json!({"path":name,"sha256":hash(bytes)})).collect::<Vec<_>>()})).unwrap();
        let activation = canonical_json(&json!({"policy_bundle":{"path":"original-bundle",
            "policy_digest":format!("hc-policy-v1:sha256:{}",hash(&manifest))}})).unwrap();
        self.selection.predecessor = HostBootstrapSelection {
            deployment_root: self.root.clone(), activation: pin(&self.root.join("original-activation.json"), &activation),
            policy_manifest: pin(&bundle.join("manifest.json"), &manifest),
            local_policy: pin(&bundle.join("toolkit-local-policy.json"), &files["toolkit-local-policy.json"]),
        };
    }

    fn write_candidate(&mut self, policy: bool) {
        self.selection.compilation_input = pin(&self.root.join("update-input.json"), &canonical_json(&self.input).unwrap());
        if policy {
            self.selection.application_tool_policy = pin(&self.root.join("update-policy.json"),
                &self.input.tool_policy.clone().compile().unwrap());
        }
    }
}

fn file_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() { result.extend(file_bytes(&path)); }
        else { result.insert(path.clone(), std::fs::read(path).unwrap()); }
    }
    result
}

#[test]
fn changed_input_is_valid_before_replacement_surfaces_and_does_not_publish_or_open_owners() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut fixture = Fixture::new(&root);
    fixture.input.tool_policy.revision = PolicyRevision(4);
    fixture.input.tool_policy.members[0].grants.push(meerkat_core::CompiledMemberToolGrant {
        tool_name: "research".into(), action: meerkat_core::CompiledMemberToolAction::Invoke,
        consequence: meerkat_core::CompiledToolConsequence::R2,
    });
    fixture.input.roster[0].additional_instructions.push("Updated ordinary instructions".into());
    let mut definition = serde_json::to_value(&fixture.input.definition).unwrap();
    definition["definition"]["profiles"]["research"]["model"] = json!("claude-sonnet-4-6");
    fixture.input.definition = serde_json::from_value::<ApplicationSetupDefinition>(definition).unwrap();
    // Exact identity joins tolerate reordering; no position selects authority.
    fixture.input.roster.swap(0, 1);
    fixture.input.members.swap(0, 1);
    fixture.write_candidate(true);
    let before = file_bytes(&root);
    let result = fixture.selection.clone().validate().unwrap();
    assert_eq!(result.selection(), &fixture.selection);
    assert_eq!(result.tool_policy_bytes(), fixture.input.tool_policy.clone().compile().unwrap());
    assert_eq!(result.tool_policy().revision, PolicyRevision(4));
    assert!(result.tool_policy().members.iter().any(|member|
        member.member_identity == "ordinary" && !member.grants.is_empty()));
    assert_eq!(result.original_owner(), fixture.prepared.native_owner.as_ref().unwrap());
    assert!(result.original_setup().helper == fixture.prepared.helper);
    assert_eq!(result.original_setup().resources.handoff, fixture.prepared.resources.handoff);
    assert_eq!(result.original_acquisition().backend_generation, fixture.acquisition.backend_generation);
    assert_eq!(result.roster()[0].profile.as_str(), fixture.prepared.helper.member.role);
    assert_ne!(result.definition(), &fixture.prepared.definition);
    assert_eq!(file_bytes(&root), before);
    assert!(!root.join("current").exists(), "a historical input receipt does not require or publish current");
    assert!(!root.join("state").exists(), "validation cannot open a native realm owner");
    assert!(!root.join("replacement-bundle").exists());
}

#[test]
fn predecessor_revision_fence_is_checked_without_accepting_into_a_live_provider() {
    for case in ["rollback", "same_revision_different_digest"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut fixture = Fixture::new(&root);
        if case == "rollback" { fixture.input.tool_policy.revision = PolicyRevision(2); }
        else { fixture.input.tool_policy.source.source_id = "changed-source".into(); }
        fixture.write_candidate(true);
        let before = file_bytes(&root);
        assert!(fixture.selection.validate().is_err(), "{case}");
        assert_eq!(file_bytes(&root), before);
        assert!(!root.join("state").exists());
    }
}

#[test]
fn compilation_inputs_use_the_existing_sixteen_mib_bound_not_the_resource_artifact_bound() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let mut fixture = Fixture::new(&root);
    fixture.input.roster[0].additional_instructions.push("x".repeat(300 * 1024));
    fixture.write_candidate(true);
    assert!(std::fs::metadata(&fixture.selection.compilation_input.path).unwrap().len() > 256 * 1024);
    let before = file_bytes(&root);
    fixture.selection.clone().validate().unwrap();
    assert_eq!(file_bytes(&root), before);
    fixture.selection.compilation_input = pin(&root.join("update-input.json"), &vec![b' '; 16 * 1024 * 1024 + 1]);
    let error = fixture.selection.validate().err().unwrap();
    assert!(error.contains("input exceeds 16 MiB"), "{error}");
    assert!(!root.join("state").exists());
}

#[test]
fn selected_bytes_and_native_policy_must_match_the_exact_input() {
    for case in ["input_pin", "policy_pin", "policy_mismatch", "noncanonical_policy", "foreign_provider", "foreign_policy"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut fixture = Fixture::new(&root);
        match case {
            "input_pin" => { std::fs::write(&fixture.selection.compilation_input.path, b"{}").unwrap(); }
            "policy_pin" => { std::fs::write(&fixture.selection.application_tool_policy.path, b"{}").unwrap(); }
            "policy_mismatch" => { fixture.input.tool_policy.revision = PolicyRevision(4); fixture.write_candidate(false); }
            "noncanonical_policy" => {
                let value: serde_json::Value = serde_json::from_slice(&fixture.original_policy_bytes).unwrap();
                fixture.selection.application_tool_policy = pin(&root.join("update-policy.json"), &serde_json::to_vec_pretty(&value).unwrap());
            }
            "foreign_provider" => { fixture.input.tool_policy.provider_id = PolicyProviderId::new("foreign").unwrap(); fixture.write_candidate(true); }
            "foreign_policy" => { fixture.input.tool_policy.policy_id = PolicyId::new("foreign").unwrap(); fixture.write_candidate(true); }
            _ => unreachable!(),
        }
        let before = file_bytes(&root);
        let error = fixture.selection.validate().err().unwrap();
        assert!(error.contains(if case.starts_with("foreign") { "original provider and policy slot" }
            else if case.ends_with("pin") { "selected update artifact changed" }
            else { "exact compiler input" }), "{case}: {error}");
        assert_eq!(file_bytes(&root), before);
        assert!(!root.join("state").exists());
    }
}

#[test]
fn exact_original_authority_and_helper_resource_cannot_be_reselected_by_candidate_input() {
    for case in ["destination", "member_grant", "helper_parent", "helper_basis", "realm", "helper_tools", "helper_roster"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut fixture = Fixture::new(&root);
        match case {
            "destination" => fixture.input.members[0].context_generation += 1,
            "member_grant" => fixture.input.members[0].setup_capability = None,
            "helper_parent" => fixture.input.helper.controller_parent.clear(),
            "helper_basis" => fixture.input.helper.operation_basis = meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] },
            "realm" => fixture.input.realm.state_root = root.join("other-state"),
            "helper_tools" => fixture.input.helper_tools = ApplicationSetupTools::Preparation,
            "helper_roster" => fixture.input.roster[1].additional_instructions.push("Changed helper".into()),
            _ => unreachable!(),
        }
        fixture.write_candidate(true);
        let before = file_bytes(&root);
        assert!(fixture.selection.validate().is_err(), "{case}");
        assert_eq!(file_bytes(&root), before);
        assert!(!root.join("state").exists());
    }
}

#[test]
fn predecessor_owner_routes_and_retained_setup_bytes_are_checked_without_an_alternate_owner() {
    for case in ["source_route", "owner", "resource", "helper_authority", "foreign_predecessor"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut fixture = Fixture::new(&root);
        match case {
            "source_route" => {
                fixture.policy.native_host.as_mut().unwrap().routes[0].controller_parent = Some(fixture.input.helper.controller_parent.clone());
                fixture.write_predecessor();
            }
            "owner" => { fixture.prepared.native_owner.as_mut().unwrap().native_host.members[0].context_generation += 1; fixture.write_predecessor(); }
            "resource" => { std::fs::write(&fixture.prepared.resources.handoff.path, b"{}").unwrap(); }
            "helper_authority" => { fixture.prepared.helper.controller_parent.clear(); fixture.write_predecessor(); }
            "foreign_predecessor" => {
                let foreign = root.join("foreign"); std::fs::create_dir(&foreign).unwrap();
                fixture.selection.predecessor = Fixture::new(&foreign).selection.predecessor;
            }
            _ => unreachable!(),
        }
        let before = file_bytes(&root);
        let error = fixture.selection.validate().err().unwrap();
        if case == "source_route" { assert!(error.contains("exact owner and source routes"), "{error}"); }
        assert_eq!(file_bytes(&root), before);
        assert!(!root.join("state").exists());
    }
}

#[test]
fn installer_receipt_echoes_exact_selection_and_refusal_stays_before_effects() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let fixture = Fixture::new(&root);
    let selected = root.join("update-selection.json");
    std::fs::write(&selected, canonical_json(&fixture.selection).unwrap()).unwrap();
    let args = vec!["--validate-application-update".into(), "--selection".into(), selected.to_str().unwrap().into()];
    let before = file_bytes(&root);
    let mut progress = InstallerProgress::default();
    assert_eq!(receipt(&args, &mut progress).unwrap(), json!({
        "kind":"application_update_validated", "selection":fixture.selection,
    }));
    assert_eq!(progress.phase, "receipt");
    assert!(!progress.effects_may_have_occurred);
    assert_eq!(file_bytes(&root), before);
    let mut invalid = serde_json::to_value(&fixture.selection).unwrap();
    invalid["owner"] = json!({"replacement":true});
    std::fs::write(&selected, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let mut refusal = InstallerProgress::default();
    assert!(receipt(&args, &mut refusal).is_err());
    assert_eq!(refusal.phase, "validation");
    assert!(!refusal.effects_may_have_occurred);
    let mut arguments = InstallerProgress::default();
    assert!(receipt(&args[..1], &mut arguments).is_err());
    assert_eq!(arguments.phase, "arguments");
    assert!(!arguments.effects_may_have_occurred);
    assert!(!root.join("current").exists());
    assert!(!root.join("state").exists());
}

// This selected test consumes an already-built final-pair executable. It does
// not replace V1-V7's library checks or establish live authority/revocation.
#[test]
#[ignore = "requires an explicit final-pair toolkit_gateway and ext4 fixture parent"]
fn final_pair_gateway_validates_update_and_refuses_unknown_owner_without_effects() {
    use std::{
        io::Write,
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };

    // The same bounded polling and kill/wait ownership used by the existing
    // preparation signal test. Entered children are settled before assertions.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }
    let selected_path = |name| {
        let path = PathBuf::from(std::env::var_os(name).expect(name));
        assert!(path.is_absolute(), "{name} must select an absolute path");
        path.canonicalize().expect(name)
    };
    let gateway = selected_path("MEERKAT_CONNECT_VALIDATOR_GATEWAY");
    let parent = selected_path("MEERKAT_CONNECT_VALIDATOR_FIXTURE_PARENT");
    assert!(gateway.is_file(), "selected gateway must be a regular file");
    assert!(parent.is_dir(), "selected fixture parent must exist");
    let directory = tempfile::Builder::new().prefix("update-validator-").tempdir_in(&parent).unwrap();
    let root = directory.path().canonicalize().unwrap();
    // Construct at the final child path: nested selected paths and hashes
    // cannot be moved afterward. Both documents precede the byte baseline.
    let fixture = Fixture::new(&root);
    let valid = root.join("valid-selection.json");
    let refused = root.join("refused-selection.json");
    crate::application_bootstrap::write_new(&valid, &canonical_json(&fixture.selection).unwrap()).unwrap();
    let mut invalid = serde_json::to_value(&fixture.selection).unwrap();
    invalid["owner"] = json!({"replacement":true});
    crate::application_bootstrap::write_new(&refused, &serde_json::to_vec(&invalid).unwrap()).unwrap();
    let before = file_bytes(&root);

    for (selected, expected_exit) in [(&valid, 0), (&refused, 1)] {
        // File-backed output outside the fixture avoids pipe backpressure and
        // does not turn the command's own logs into fixture mutations.
        let stdout_file = tempfile::NamedTempFile::new_in(&parent).unwrap();
        let stderr_file = tempfile::NamedTempFile::new_in(&parent).unwrap();
        let mut child = OwnedChild(Command::new(&gateway)
            .args(["--validate-application-update", "--selection"])
            .arg(selected)
            .stdin(Stdio::null())
            .stdout(stdout_file.reopen().unwrap())
            .stderr(stderr_file.reopen().unwrap())
            .spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(15);
        let observed = loop {
            match child.0.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                Ok(None) => break Err("validator child exceeded its deadline".to_owned()),
                Err(error) => break Err(format!("validator child observation failed: {error}")),
            }
        };
        if observed.is_err() { let _ = child.0.kill(); }
        let joined = child.0.wait();
        drop(child);
        let stdout = std::fs::read(stdout_file.path()).unwrap();
        let stderr = std::fs::read(stderr_file.path()).unwrap();
        eprintln!("validator executable={gateway:?} selection={selected:?} sha256={} stdout_sha256={} stderr_sha256={} observed={observed:?} joined={joined:?}",
            hash(before.get(selected).unwrap()), hash(&stdout), hash(&stderr));
        // Preserve the original bytes in the existing lane's captured output.
        std::io::stdout().lock().write_all(&stdout).unwrap();
        std::io::stderr().lock().write_all(&stderr).unwrap();
        assert_eq!(file_bytes(&root), before, "validator mutated its fixture");
        assert!(std::fs::symlink_metadata(root.join("current")).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound));
        assert!(std::fs::symlink_metadata(root.join("state")).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound));
        let status = observed.unwrap();
        assert_eq!(joined.unwrap(), status);
        assert_eq!(status.code(), Some(expected_exit));
        let actual: serde_json::Value = serde_json::from_slice(&stdout).unwrap();
        let expected = if expected_exit == 0 {
            assert!(stderr.is_empty());
            json!({"kind":"application_update_validated", "selection":fixture.selection})
        } else {
            let detail = actual["detail"].as_str().unwrap();
            assert!(detail.contains("unknown field `owner`"), "{detail}");
            assert_eq!(stderr, format!("toolkit_gateway installer: {detail}\n").as_bytes());
            json!({"kind":"application_installer_failed", "command":"--validate-application-update",
                "phase":"validation", "effects_may_have_occurred":false, "detail":detail})
        };
        assert_eq!(actual, expected);
        let mut expected_stdout = canonical_json(&expected).unwrap();
        expected_stdout.push(b'\n');
        assert_eq!(stdout, expected_stdout);
    }
}

#[test]
fn replacement_startup_pins_actual_prior_policy_and_output_custody_before_owner_open() {
    use crate::application_update::ValidatedReplacementStartup;
    use toolkit_mobkit::private_host::ApplicationUpdateStartup;
    use std::os::unix::fs::PermissionsExt;
    for case in ["valid", "rollback_after_prior_update", "equal_prior_conflict", "foreign_prior_slot", "changed_selection", "public_output", "symlink_output", "missing_output"] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let mut fixture = Fixture::new(&root);
        fixture.input.tool_policy.revision = PolicyRevision(6);
        fixture.input.roster[0].additional_instructions.push("Replacement source".into());
        fixture.write_candidate(true);
        let mut prior = fixture.input.tool_policy.clone();
        prior.revision = PolicyRevision(match case { "rollback_after_prior_update" => 7, "equal_prior_conflict" => 6, _ => 5 });
        if case == "equal_prior_conflict" { prior.source.source_id = "another-prior-source".into(); }
        if case == "foreign_prior_slot" { prior.policy_id = PolicyId::new("other-slot").unwrap(); }
        let prior_pin = pin(&root.join("prior-desired.json"), &prior.compile().unwrap());
        let selected = pin(&root.join("selection.json"), &canonical_json(&fixture.selection).unwrap());
        let output = root.join("associations");
        if case != "missing_output" {
            std::fs::create_dir(&output).unwrap();
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(if case == "public_output" { 0o755 } else { 0o700 })).unwrap();
        }
        let mut startup = ApplicationUpdateStartup { selection: selected, prior_tool_policy: prior_pin, association_directory: output.clone() };
        if case == "changed_selection" { std::fs::write(&startup.selection.path, b"{}").unwrap(); }
        if case == "symlink_output" {
            let alias = root.join("alias"); std::os::unix::fs::symlink(&output, &alias).unwrap(); startup.association_directory = alias;
        }
        let before = file_bytes(&root);
        let result = ValidatedReplacementStartup::validate(&startup);
        if case == "valid" {
            let validated = result.unwrap();
            assert_eq!(validated.predecessor(), &fixture.selection.predecessor);
            assert_eq!(validated.update.original_setup().resources.handoff, fixture.prepared.resources.handoff);
            assert_eq!(validated.update.original_owner(), fixture.prepared.native_owner.as_ref().unwrap());
            assert_eq!(validated.carrier_bytes, std::fs::read(&startup.selection.path).unwrap());
            assert_eq!(validated.update.tool_policy().revision, PolicyRevision(6));
        } else { assert!(result.is_err(), "{case}"); }
        assert_eq!(file_bytes(&root), before, "startup validation must write nothing: {case}");
        assert!(!root.join("current").exists());
        assert!(!root.join("shared/data/toolkit-native").exists());
    }
}

#[test]
fn actual_replacement_roster_projection_keeps_live_changes_and_original_helper_ceiling() {
    use meerkat_mobkit::identity_first::{RosterContext, contracts::RosterProjection};
    let dir = tempfile::tempdir().unwrap(); let root = dir.path().canonicalize().unwrap();
    let mut fixture = Fixture::new(&root);
    let mut definition = serde_json::to_value(&fixture.input.definition).unwrap();
    definition["definition"]["profiles"]["research"]["model"] = json!("claude-sonnet-4-6");
    fixture.input.definition = serde_json::from_value(definition).unwrap(); fixture.write_candidate(true);
    let selected = fixture.selection.clone().validate().unwrap();
    let projection = crate::application_acquisition::PreparedApplicationRoster::new(
        selected.original_setup().clone(), selected.definition().clone());
    let context = RosterContext { mob_definition: Some(selected.definition().clone()), previous_identities: vec![] };
    let mut actual = selected.roster().to_vec();
    actual[0].additional_instructions.push("Live provider refresh, after startup".into());
    actual.swap(0, 1);
    assert_eq!(projection.project(&context, actual.clone()).unwrap(), actual, "current provider results are not replaced by initial selected rows");
    let helper = actual.iter_mut().find(|row| row.profile.as_str() == selected.original_setup().helper.member.role).unwrap();
    helper.additional_instructions.push("changed helper resource instructions".into());
    assert!(projection.project(&context, actual).is_err());
    let foreign = RosterContext { mob_definition: Some(fixture.prepared.definition), previous_identities: vec![] };
    assert!(projection.project(&foreign, selected.roster().to_vec()).is_err());
}
