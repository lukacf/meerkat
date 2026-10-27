//! Trusted application installer commands for the existing gateway executable.
//! These never run as ordinary gateway bootstrap or through an app RPC request.

use meerkat_toolkit::host_lifetime::{
    control::{HostBootstrapSelection, HostLifetimeError, SelectedHostArtifact},
    policy::{CompiledArtifactReference, CompiledLocalPolicy},
};
use std::{collections::BTreeMap, fs::OpenOptions, io::Write, path::{Path, PathBuf}};
use toolkit_mobkit::installed_host::operator_setup::OperatorSetup;

pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, String> {
    let path = Path::new(path);
    if !path.is_absolute() || path.is_symlink() || !path.is_file() { return Err("input must be an absolute regular path".into()); }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}
pub(super) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)
        .map_err(|error| error.to_string())?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|error| error.to_string())
}
pub(super) fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn emit(value: &impl serde::Serialize) -> Result<(), String> {
    let mut bytes = toolkit_package::canonical_json(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    std::io::stdout().lock().write_all(&bytes).map_err(|error| error.to_string())
}

pub(super) struct InstallerProgress {
    pub(super) phase: &'static str,
    pub(super) effects_may_have_occurred: bool,
}
impl Default for InstallerProgress {
    fn default() -> Self { Self { phase: "arguments", effects_may_have_occurred: false } }
}
impl InstallerProgress {
    fn failure(&self, command: &str, detail: &str) -> serde_json::Value {
        serde_json::json!({
            "kind":"application_installer_failed", "command":command,
            "phase":self.phase, "effects_may_have_occurred":self.effects_may_have_occurred,
            "detail":detail,
        })
    }
}

/// Explicit trusted installer modes only. Candidate provisioning initializes
/// an empty pair and releases its native owner lease before returning. It never
/// publishes current, issues a grant, or starts a candidate/runtime session.
pub fn run(args: &[String]) -> Option<std::process::ExitCode> {
    let kind = args.first()?.as_str();
    if !matches!(kind, "--provision-application-owner" | "--prepare-application-policy"
        | "--validate-application-policy" | "--validate-application-update" | "--compile-application-tool-policy" | "--prepare-application-setup" | "--lower-application-inventory" | "--configure-agent-setup-capability" | "--configure-helper-report-capability") { return None; }
    let mut progress = InstallerProgress::default();
    let result = match kind {
        "--configure-agent-setup-capability" => configure_agent_setup(args, &mut progress),
        "--configure-helper-report-capability" => configure_helper_report(args, &mut progress),
        "--prepare-application-setup" => crate::application_setup::command(args, &mut progress),
        "--lower-application-inventory" => crate::application_setup::inventory::command(args, &mut progress),
        "--provision-application-owner" => provision(args, &mut progress),
        "--prepare-application-policy" => prepare(args, &mut progress),
        "--validate-application-policy" => validate(args, &mut progress),
        "--validate-application-update" => crate::application_setup::compilation::update::command(args, &mut progress),
        "--compile-application-tool-policy" => compile_tool_policy(args, &mut progress),
        _ => unreachable!(),
    };
    Some(match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // A failed/absent receipt never proves that initialization or a
            // write did not happen. The app retains its exact attempt journal.
            let _ = emit(&progress.failure(kind, &error));
            eprintln!("toolkit_gateway installer: {error}");
            std::process::ExitCode::FAILURE
        }
    })
}

// The trusted application holds its existing stopped-host transaction and
// retains both input and issuance ID across lost/uncertain command replies.
fn configure_agent_setup(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, selection_flag, selection_path, request_flag, request_path] = args else {
        return Err("expected --configure-agent-setup-capability --selection FILE --request FILE".into());
    };
    if selection_flag != "--selection" || request_flag != "--request" {
        return Err("invalid explicit acquisition configuration arguments".into());
    }
    progress.phase = "validation";
    let selection: HostBootstrapSelection = read_json(selection_path)?;
    let request: toolkit_mobkit::installed_host::operator_setup::AgentSetupCapabilityRequest = read_json(request_path)?;
    progress.phase = "native_parent_issuance";
    progress.effects_may_have_occurred = true;
    let capability = OperatorSetup::configure_application_agent_setup_with_helper(&selection, &request, |native| {
        if request.helper_targets.is_empty() { return Ok(None); }
        Ok(crate::application_acquisition::read_application_acquisition(native, |reference| {
            toolkit_mobkit::installed_host::read_bundle_artifact(&selection, reference)
        })?.map(|selected| selected.helper_selection().clone()))
    }).map_err(|error| error.to_string())?;
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"agent_setup_capability_configured",
        "member":request.member, "setup_capability":capability}))
}

// Select the actual helper through the same typed artifact reader used at
// gateway composition, inside the one stopped-host native-owner transaction.
fn configure_helper_report(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, selection_flag, selection_path, request_flag, request_path] = args else {
        return Err("expected --configure-helper-report-capability --selection FILE --request FILE".into());
    };
    if selection_flag != "--selection" || request_flag != "--request" {
        return Err("invalid explicit report configuration arguments".into());
    }
    progress.phase = "validation";
    let selection: HostBootstrapSelection = read_json(selection_path)?;
    let request: toolkit_mobkit::installed_host::operator_setup::HelperReportCapabilityRequest = read_json(request_path)?;
    progress.phase = "native_parent_issuance";
    progress.effects_may_have_occurred = true;
    let capability = OperatorSetup::configure_application_helper_report(&selection, &request, |native| {
        let selected = crate::application_acquisition::read_application_acquisition(native, |reference| {
            toolkit_mobkit::installed_host::read_bundle_artifact(&selection, reference)
        })?.ok_or(HostLifetimeError::Unavailable)?;
        Ok(selected.helper_selection().clone())
    }).map_err(|error| error.to_string())?;
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"helper_report_capability_configured",
        "member":request.member, "report_capability":capability}))
}

fn provision(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, selection_flag, selection_path, namespace_flag, namespace, generation_flag, generation] = args else {
        return Err("expected --provision-application-owner --selection FILE --namespace ID --generation NUMBER".into());
    };
    if selection_flag != "--selection" || namespace_flag != "--namespace" || generation_flag != "--generation" {
        return Err("invalid explicit application provisioning arguments".into());
    }
    progress.phase = "validation";
    let selection: HostBootstrapSelection = read_json(selection_path)?;
    let generation = generation.parse().map_err(|_| "invalid native owner generation")?;
    toolkit_mobkit::installed_host::validate_application_candidate(&selection)
        .map_err(|error| error.to_string())?;
    progress.phase = "owner_initialization";
    progress.effects_may_have_occurred = true;
    OperatorSetup::provision_application_candidate(&selection, namespace, generation)
        .map_err(|error| error.to_string())?;
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"application_owner_provisioned","selection":selection}))
}

fn validate(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, flag, path] = args else {
        return Err("expected --validate-application-policy --selection FILE".into());
    };
    if flag != "--selection" { return Err("invalid candidate validation arguments".into()); }
    progress.phase = "validation";
    let selection: HostBootstrapSelection = read_json(path)?;
    toolkit_mobkit::installed_host::validate_application_candidate(&selection)
        .map_err(|error| error.to_string())?;
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"application_policy_validated","selection":selection}))
}

fn compile_tool_policy(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, input_flag, input, output_flag, output] = args else {
        return Err("expected --compile-application-tool-policy --input FILE --output FRESH_FILE".into());
    };
    if input_flag != "--input" || output_flag != "--output" {
        return Err("invalid native policy compilation arguments".into());
    }
    progress.phase = "validation";
    let input: crate::application_tool_policy::ApplicationToolPolicyInput = read_json(input)?;
    let bytes = input.compile().map_err(|error| error.to_string())?;
    let output = Path::new(output);
    if !output.is_absolute() { return Err("output must be an absolute fresh path".into()); }
    let parent = output.parent().ok_or("output has no parent")?.canonicalize()
        .map_err(|error| error.to_string())?;
    let output = new_child(&parent, output)?;
    progress.phase = "artifact_write";
    progress.effects_may_have_occurred = true;
    write_new(&output, &bytes)?;
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"application_tool_policy_compiled",
        "artifact":{"path":output,"sha256":hash(&bytes)}}))
}

// Prepare immutable candidate files. The application's stopped-host deployment
// transaction alone publishes current. Native selections in the supplied policy
// remain subject to the native owner's validation when used.
pub(super) fn prepare(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, policy_flag, policy_path, artifacts_flag, artifacts_path, root_flag, root_path,
        bundle_flag, bundle_path, template_flag, template_path, activation_flag, activation_path] = args else {
        return Err("expected --prepare-application-policy --policy FILE --artifacts DIRECTORY --deployment-root DIRECTORY --bundle DIRECTORY --activation-template FILE --activation FILE".into());
    };
    if policy_flag != "--policy" || artifacts_flag != "--artifacts" || root_flag != "--deployment-root"
        || bundle_flag != "--bundle" || template_flag != "--activation-template" || activation_flag != "--activation" {
        return Err("invalid policy preparation arguments".into());
    }
    progress.phase = "validation";
    let policy: CompiledLocalPolicy = read_json(policy_path)?;
    let local = policy.canonical_bytes().map_err(|error| error.to_string())?;
    let root = Path::new(root_path).canonicalize().map_err(|error| error.to_string())?;
    let bundle = new_child(&root, Path::new(bundle_path))?;
    let activation = new_child(&root, Path::new(activation_path))?;
    let artifacts = Path::new(artifacts_path).canonicalize().map_err(|error| error.to_string())?;
    let mut files = BTreeMap::new();
    read_installer_artifact(&artifacts, &mut files, &CompiledArtifactReference {
        name: "policy.json".into(), sha256: policy.source_policy_sha256.clone(),
    }).map_err(|error| error.to_string())?;
    if let Some(native) = &policy.native_host {
        // The existing readers define the complete selected artifact closure,
        // including setup's typed prepared-application reference. Retain each
        // exact read once, then write only after every owner validator succeeds.
        let mut read = |reference: &CompiledArtifactReference| {
            read_installer_artifact(&artifacts, &mut files, reference)
        };
        let _setup = crate::application_acquisition::read_application_acquisition(native, &mut read)
            .map_err(|error| error.to_string())?;
        let _validated = toolkit_mobkit::installed_host::validate_installed_artifacts(native, &mut read)
            .map_err(|error| error.to_string())?;
    }
    files.insert("toolkit-local-policy.json".into(), local.clone());
    let manifest = toolkit_package::canonical_json(&serde_json::json!({"artifacts": files.iter()
        .map(|(name, bytes)| serde_json::json!({"path":name,"sha256":hash(bytes)})).collect::<Vec<_>>() }))
        .map_err(|error| error.to_string())?;
    let manifest_digest = hash(&manifest);
    let mut activation_value: serde_json::Value = read_json(template_path)?;
    let object = activation_value.as_object_mut().ok_or("activation template must be an object")?;
    object.insert("policy_bundle".into(), serde_json::json!({
        "path":bundle.strip_prefix(&root).map_err(|error| error.to_string())?.to_str().ok_or("invalid bundle path")?,
        "policy_digest":format!("hc-policy-v1:sha256:{manifest_digest}")
    }));
    let activation_bytes = toolkit_package::canonical_json(&activation_value).map_err(|error| error.to_string())?;
    // All input validation precedes effects. A partial failed preparation is
    // never selected or overwritten; the deployment owner retains its receipt.
    progress.phase = "candidate_write";
    progress.effects_may_have_occurred = true;
    std::fs::create_dir(&bundle).map_err(|error| error.to_string())?;
    for (name, bytes) in files { write_new(&bundle.join(name), &bytes)?; }
    write_new(&bundle.join("manifest.json"), &manifest)?;
    write_new(&activation, &activation_bytes)?;
    let selection = HostBootstrapSelection {
        deployment_root: root,
        activation: SelectedHostArtifact { path: activation, sha256: hash(&activation_bytes) },
        policy_manifest: SelectedHostArtifact { path: bundle.join("manifest.json"), sha256: manifest_digest },
        local_policy: SelectedHostArtifact { path: bundle.join("toolkit-local-policy.json"), sha256: hash(&local) },
    };
    progress.phase = "receipt";
    emit(&serde_json::json!({"kind":"application_policy_prepared","selection":selection}))
}
// Reusing a name is allowed only for the same exact bytes. This applies to
// nested setup references as well as the original policy and member artifacts.
fn read_installer_artifact(
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    reference: &CompiledArtifactReference,
) -> Result<Vec<u8>, HostLifetimeError> {
    reference.validate()?;
    if matches!(reference.name.as_str(), "manifest.json" | "toolkit-local-policy.json") {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let bytes = if let Some(bytes) = files.get(&reference.name) {
        bytes.clone()
    } else {
        let path = directory.join(&reference.name);
        if path.is_symlink() || !path.is_file() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        std::fs::read(path).map_err(|_| HostLifetimeError::InvalidSelection)?
    };
    if hash(&bytes) != reference.sha256 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    files.entry(reference.name.clone()).or_insert_with(|| bytes.clone());
    Ok(bytes)
}

pub(super) fn new_child(root: &Path, path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() || path.exists() || path.is_symlink() { return Err("output must be a fresh absolute path".into()); }
    let parent = path.parent().ok_or("output has no parent")?.canonicalize().map_err(|error| error.to_string())?;
    if !parent.starts_with(root) { return Err("output parent is outside deployment root".into()); }
    let name = path.file_name().ok_or("output has no file name")?;
    Ok(parent.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_policy_compile_cli_is_callable_and_never_overwrites() {
        use meerkat_core::*;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let input_path = root.join("input.json");
        let output = root.join("compiled.json");
        let selected = crate::application_tool_policy::ApplicationToolPolicyInput {
            provider_id: PolicyProviderId::new("app").unwrap(),
            policy_id: PolicyId::new("tools").unwrap(), revision: PolicyRevision(1),
            source: CompiledPolicySourceProvenance { source_id:"app-owned".into(),
                source_digest:PolicyDigest::from_canonical_bytes(b"app-owned") },
            members: vec![CompiledMemberToolGrants { member_identity:"agent".into(), grants:vec![] }],
        };
        std::fs::write(&input_path, serde_json::to_vec(&selected).unwrap()).unwrap();
        let args = vec!["--compile-application-tool-policy".into(), "--input".into(),
            input_path.to_str().unwrap().into(), "--output".into(), output.to_str().unwrap().into()];
        let mut progress = InstallerProgress::default();
        compile_tool_policy(&args, &mut progress).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let compiled = CompiledApplicationToolPolicy::parse_canonical_json(&bytes).unwrap();
        assert_eq!(compiled.members, selected.members);
        assert_eq!(bytes, selected.clone().compile().unwrap());
        assert_eq!(progress.phase, "receipt");
        assert!(progress.effects_may_have_occurred);
        let mut refusal = InstallerProgress::default();
        assert!(compile_tool_policy(&args, &mut refusal).is_err());
        assert!(!refusal.effects_may_have_occurred);
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
        // The actual native constructor rejects duplicate input before creating
        // a second artifact; no locally reimplemented policy validator.
        let mut duplicate = selected;
        duplicate.members.push(duplicate.members[0].clone());
        std::fs::write(&input_path, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        let mut invalid_args = args;
        let invalid_output = root.join("invalid.json");
        invalid_args[4] = invalid_output.to_str().unwrap().into();
        assert!(compile_tool_policy(&invalid_args, &mut refusal).is_err());
        assert!(!invalid_output.exists());
        assert!(!refusal.effects_may_have_occurred);
    }

    #[test]
    fn installer_failure_receipt_preserves_phase_and_uncertainty() {
        let mut progress = InstallerProgress::default();
        assert!(provision(&["--provision-application-owner".into()], &mut progress).is_err());
        assert_eq!(progress.failure("--provision-application-owner", "invalid arguments"),
            serde_json::json!({"kind":"application_installer_failed",
                "command":"--provision-application-owner", "phase":"arguments",
                "effects_may_have_occurred":false, "detail":"invalid arguments"}));
    }

    #[test]
    fn preparation_validates_actual_native_artifacts_before_any_output() {
        use meerkat_core::*;
        for case in [
            "valid",
            "tool_malformed",
            "tool_noncanonical",
            "tool_missing_member",
            "surface_malformed",
            "surface_noncanonical",
            "surface_duplicate_name",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            let inputs = root.join("inputs");
            std::fs::create_dir(&inputs).unwrap();
            let source = b"{\"schema\":1}";
            std::fs::write(inputs.join("policy.json"), source).unwrap();
            let policy = CompiledApplicationToolPolicy::new(
                PolicyProviderId::new("fixture").unwrap(),
                PolicyId::new("fixture").unwrap(),
                PolicyRevision(1),
                CompiledPolicySourceProvenance {
                    source_id: "fixture".into(),
                    source_digest: PolicyDigest::from_canonical_bytes(b"fixture"),
                },
                vec![CompiledMemberToolGrants {
                    member_identity: "agent".into(),
                    grants: vec![],
                }],
            )
            .unwrap();
            let mut tool = policy.canonical_json().unwrap();
            let mut surface = b"[]".to_vec();
            match case {
                "tool_malformed" => tool = b"not-json".to_vec(),
                "tool_noncanonical" => {
                    tool = serde_json::to_vec_pretty(
                        &serde_json::from_slice::<serde_json::Value>(&tool).unwrap(),
                    )
                    .unwrap()
                }
                "tool_missing_member" => {
                    tool = CompiledApplicationToolPolicy::new(
                        policy.provider_id.clone(),
                        policy.policy_id.clone(),
                        policy.revision,
                        policy.source.clone(),
                        vec![CompiledMemberToolGrants {
                            member_identity: "other-member".into(),
                            grants: vec![],
                        }],
                    )
                    .unwrap()
                    .canonical_json()
                    .unwrap();
                }
                "surface_malformed" => surface = b"[{}]".to_vec(),
                "surface_noncanonical" => surface = b"[\n]\n".to_vec(),
                "surface_duplicate_name" => {
                    let identity =
                        ToolDef::new("same-tool", "fixture", serde_json::json!({})).identity();
                    surface =
                        toolkit_package::canonical_json(&vec![identity.clone(), identity]).unwrap();
                }
                _ => {}
            }
            std::fs::write(inputs.join("tools.json"), &tool).unwrap();
            std::fs::write(inputs.join("surface.json"), &surface).unwrap();
            let principal = PrincipalRef::in_domain(
                PrincipalKind::ServiceAccount,
                "application",
                TrustDomainId::new("factory-test").unwrap(),
            )
            .unwrap();
            let member = MobMemberBinding {
                mob_id: "factory".into(),
                role: "identity".into(),
                member: "agent".into(),
            };
            let native = serde_json::from_value(serde_json::json!({
                "realm":mob_realm_id("factory").unwrap(), "grant_administrator":principal,
                "remote_operator":null,
                "ingress_domain":{"authority":principal,"namespace":"source"},
                "application_tool_policy":{"name":"tools.json","sha256":hash(&tool)},
                "members":[{"member":member,"logical_owner":principal,"context_generation":1,
                    "audience":{"audience":"principal","principal":principal},
                    "initial_surface":{"name":"surface.json","sha256":hash(&surface)}}],
                "routes":[]
            }))
            .unwrap();
            let policy = CompiledLocalPolicy {
                schema: 7,
                deployment: toolkit_package::Id::new("application").unwrap(),
                source_policy_sha256: hash(source),
                components: vec![],
                channels: vec![],
                native_host: Some(native),
            };
            // Correct hashes and valid compiled selection for every control.
            let bytes = policy.canonical_bytes().unwrap();
            let policy_path = root.join("input.json");
            std::fs::write(&policy_path, bytes).unwrap();
            let template = root.join("template.json");
            std::fs::write(&template, b"{\"activation_id\":\"owned-by-app\"}").unwrap();
            let bundle = root.join("bundle.1");
            let activation = root.join("activation.1.json");
            let args = vec![
                "--prepare-application-policy".into(),
                "--policy".into(),
                policy_path.display().to_string(),
                "--artifacts".into(),
                inputs.display().to_string(),
                "--deployment-root".into(),
                root.display().to_string(),
                "--bundle".into(),
                bundle.display().to_string(),
                "--activation-template".into(),
                template.display().to_string(),
                "--activation".into(),
                activation.display().to_string(),
            ];
            let mut progress = InstallerProgress::default();
            let result = prepare(&args, &mut progress);
            if case != "valid" {
                assert!(!progress.effects_may_have_occurred, "{case}");
                assert_eq!(progress.failure("--prepare-application-policy", "refused")["phase"], "validation");
            }
            if case == "valid" {
                result.unwrap();
                assert_eq!(
                    std::fs::read(bundle.join("toolkit-local-policy.json")).unwrap(),
                    policy.canonical_bytes().unwrap()
                );
            } else {
                assert!(result.is_err(), "{case}");
                assert!(!bundle.exists(), "{case}");
                assert!(!activation.exists(), "{case}");
            }
            assert!(!root.join("current").exists());
            assert!(!root.join("shared/data/toolkit-native").exists());
        }
    }

    #[test]
    fn preparation_uses_bootstrap_canonical_bytes_and_never_publishes_current() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let inputs = root.join("inputs"); std::fs::create_dir(&inputs).unwrap();
        let source = b"{\"schema\":1}";
        std::fs::write(inputs.join("policy.json"),source).unwrap();
        let policy = CompiledLocalPolicy { schema:7,
            deployment:toolkit_package::Id::new("application").unwrap(),
            source_policy_sha256:hash(source), components:vec![],channels:vec![],native_host:None };
        let policy_path=root.join("input.json");
        std::fs::write(&policy_path,serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
        let template=root.join("template.json");
        std::fs::write(&template,b"{\"activation_id\":\"owned-by-app\"}").unwrap();
        let bundle=root.join("bundle.1"); let activation=root.join("activation.1.json");
        let args=vec!["--prepare-application-policy".into(),"--policy".into(),policy_path.display().to_string(),
            "--artifacts".into(),inputs.display().to_string(),"--deployment-root".into(),root.display().to_string(),
            "--bundle".into(),bundle.display().to_string(),"--activation-template".into(),template.display().to_string(),
            "--activation".into(),activation.display().to_string()];
        prepare(&args, &mut InstallerProgress::default()).unwrap();
        assert_eq!(std::fs::read(bundle.join("toolkit-local-policy.json")).unwrap(),policy.canonical_bytes().unwrap());
        assert!(!root.join("current").exists());
        let before=std::fs::read(&activation).unwrap();
        assert!(prepare(&args, &mut InstallerProgress::default()).is_err());
        assert_eq!(std::fs::read(&activation).unwrap(),before);
        // Only the existing application deployment transaction performs this
        // publication. Its resulting selectors pass the real host validator.
        std::os::unix::fs::symlink("activation.1.json",root.join("current")).unwrap();
        let selection=HostBootstrapSelection { deployment_root:root,
            activation:SelectedHostArtifact {path:activation,sha256:hash(&before)},
            policy_manifest:SelectedHostArtifact {path:bundle.join("manifest.json"),sha256:hash(&std::fs::read(bundle.join("manifest.json")).unwrap())},
            local_policy:SelectedHostArtifact {path:bundle.join("toolkit-local-policy.json"),sha256:hash(&policy.canonical_bytes().unwrap())} };
        let control = selection.deployment_root.join("shared/control");
        assert!(!control.exists(), "preparation must not provision host control state");
        assert!(matches!(
            meerkat_toolkit::host_lifetime::NativeHostLifetime::bootstrap(selection.clone()),
            Err(HostLifetimeError::Unavailable)
        ));
        // The deployment owner provisions its control directory. Bootstrap
        // opens the existing entry fence; candidate preparation does not.
        std::fs::create_dir_all(&control).unwrap();
        let owner=meerkat_toolkit::host_lifetime::NativeHostLifetime::bootstrap(selection).unwrap();
        assert_eq!(owner.selected_policy(),&policy);
    }
}
