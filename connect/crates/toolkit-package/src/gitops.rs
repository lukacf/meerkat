//! Exact Kubernetes replacement documents, prepared without applying them.
//!
//! The trusted host supplies and pins the profile. Its capabilities are a
//! compatibility declaration, not a grant. This backend does not fetch images,
//! verify registry bytes, access Git, approve changes, or observe a deployment.

use crate::{
    ActivationManifest, ActivationScope, ArtifactDependencies, AuthenticationRequirement,
    ComponentLifetime, ComponentSource, ContentDigest, Export, HostCapabilities, Id,
    IntegrationPackage, RuntimeKind, ValidatedManifest, canonical_json, strict_mcpb_json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub const GITOPS_MAX_INPUT: usize = 2 * 1024 * 1024;
pub const GITOPS_MAX_OUTPUT: usize = 2 * 1024 * 1024;
const MAX_DOCUMENT: usize = 256 * 1024;
const MAX_CONFIGURATION: usize = 64 * 1024;
const MANIFEST_ANNOTATION: &str = "toolkit.rkat.ai/manifest-sha256";
const CONFIGURATION_ANNOTATION: &str = "toolkit.rkat.ai/configuration-sha256";
const POLICY_ANNOTATION: &str = "toolkit.rkat.ai/policy-sha256";

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GitOpsPreparationError {
    #[error("invalid_input")]
    InvalidInput,
    #[error("input_limit")]
    InputLimit,
    #[error("binding_mismatch")]
    BindingMismatch,
    #[error("unsupported_selection")]
    UnsupportedSelection,
    #[error("incompatible_host")]
    IncompatibleHost,
    #[error("invalid_target")]
    InvalidTarget,
    #[error("output_limit")]
    OutputLimit,
}
type Result<T> = std::result::Result<T, GitOpsPreparationError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitOpsDeploymentTarget {
    pub path: String,
    pub sha256: ContentDigest,
    pub namespace: String,
    pub name: String,
    pub container: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitOpsConfigMapTarget {
    pub path: String,
    pub sha256: ContentDigest,
    pub namespace: String,
    pub name: String,
    pub configuration_key: String,
    pub policy_key: String,
    pub volume: String,
    pub mount_path: String,
    pub configuration_file: String,
    pub policy_file: String,
    pub snapshot_directory: String,
}

/// Host-owned exact target selection. Never infer this profile from agent text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesGitOpsProfile {
    pub schema_version: u16,
    pub owner: Id,
    pub source_commit: String,
    pub source_repository: String,
    pub host: HostCapabilities,
    pub placement: Id,
    pub deployment: GitOpsDeploymentTarget,
    pub config_map: GitOpsConfigMapTarget,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitOpsReplacement {
    /// Replace requires the recorded base digest; create requires no prior path.
    pub action: &'static str,
    pub sha256: ContentDigest,
    /// Exact replacement UTF-8 bytes. Apply only against the pinned base.
    pub contents: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KubernetesGitOpsProposal {
    pub source_commit: String,
    pub source_repository: String,
    pub host_owner: Id,
    pub profile_sha256: ContentDigest,
    pub host_capabilities_sha256: ContentDigest,
    pub manifest_sha256: ContentDigest,
    /// Full canonical selection, including still-unfulfilled lifecycle intent.
    pub manifest_json: String,
    pub configuration_sha256: ContentDigest,
    pub policy_sha256: ContentDigest,
    pub base_sha256: BTreeMap<String, ContentDigest>,
    pub files: BTreeMap<String, GitOpsReplacement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedKubernetesGitOps {
    pub schema_version: u16,
    pub status: &'static str,
    pub proposal_sha256: ContentDigest,
    pub proposal: KubernetesGitOpsProposal,
}

/// Raw strings preserve source-byte bindings, including JSON whitespace.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KubernetesGitOpsRequest {
    pub schema_version: u16,
    pub package_json: String,
    pub manifest_json: String,
    pub profile_json: String,
    pub profile_sha256: ContentDigest,
    pub base_documents: BTreeMap<String, String>,
    pub configuration: String,
    pub policy: String,
}

fn json(bytes: &[u8]) -> Result<Value> {
    if bytes.len() > MAX_DOCUMENT {
        return Err(GitOpsPreparationError::InputLimit);
    }
    // The existing generic JSON reader also refuses duplicate object keys.
    strict_mcpb_json(bytes).map_err(|_| GitOpsPreparationError::InvalidInput)
}

fn text(bytes: &[u8]) -> Result<String> {
    if bytes.len() > MAX_CONFIGURATION {
        return Err(GitOpsPreparationError::InputLimit);
    }
    let value = std::str::from_utf8(bytes).map_err(|_| GitOpsPreparationError::InvalidInput)?;
    if value.contains('\0') {
        return Err(GitOpsPreparationError::InvalidInput);
    }
    Ok(value.into())
}

fn canonical(value: &impl Serialize) -> Result<Vec<u8>> {
    canonical_json(value).map_err(|_| GitOpsPreparationError::InvalidInput)
}

fn label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

fn name(value: &str) -> bool {
    value.len() <= 253 && value.split('.').all(label)
}

fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

fn relative(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && value.split('/').all(key)
}

fn paths_conflict(first: &str, second: &str) -> bool {
    let first = first.to_ascii_lowercase();
    let second = second.to_ascii_lowercase();
    first == second
        || first.starts_with(&format!("{second}/"))
        || second.starts_with(&format!("{first}/"))
}

fn object(value: &Value) -> Result<&Map<String, Value>> {
    value
        .as_object()
        .ok_or(GitOpsPreparationError::InvalidTarget)
}

fn string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(GitOpsPreparationError::InvalidTarget)
}

fn array<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>> {
    value
        .get(field)
        .and_then(Value::as_array)
        .ok_or(GitOpsPreparationError::InvalidTarget)
}

fn named_index(values: &[Value], selected: &str) -> Result<usize> {
    let mut names = BTreeSet::new();
    let mut found = None;
    for (index, value) in values.iter().enumerate() {
        let current = string(value, "name")?;
        if !label(current) || !names.insert(current) {
            return Err(GitOpsPreparationError::InvalidTarget);
        }
        if current == selected {
            found = Some(index);
        }
    }
    found.ok_or(GitOpsPreparationError::InvalidTarget)
}

fn target(
    bytes: &str,
    sha256: &ContentDigest,
    version: &str,
    kind: &str,
    namespace: &str,
    name: &str,
) -> Result<Value> {
    if ContentDigest::of_bytes(bytes.as_bytes()) != *sha256 {
        return Err(GitOpsPreparationError::BindingMismatch);
    }
    let value = json(bytes.as_bytes())?;
    if string(&value, "apiVersion")? != version
        || string(&value, "kind")? != kind
        || string(&value["metadata"], "namespace")? != namespace
        || string(&value["metadata"], "name")? != name
        || value.get("status").is_some()
        || [
            "uid",
            "resourceVersion",
            "generation",
            "creationTimestamp",
            "deletionTimestamp",
            "deletionGracePeriodSeconds",
            "managedFields",
            "generateName",
        ]
        .iter()
        .any(|field| value["metadata"].get(field).is_some())
    {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    Ok(value)
}

fn repository_part(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    loop {
        let start = index;
        while index < bytes.len()
            && (bytes[index].is_ascii_lowercase() || bytes[index].is_ascii_digit())
        {
            index += 1;
        }
        if index == start {
            return false;
        }
        if index == bytes.len() {
            return true;
        }
        match bytes[index] {
            b'.' | b'_' => index += 1,
            b'-' => {
                while index < bytes.len() && bytes[index] == b'-' {
                    index += 1;
                }
            }
            _ => return false,
        }
    }
}

pub(crate) fn image_reference(location: &url::Url, expected: &ContentDigest) -> Result<String> {
    let invalid = GitOpsPreparationError::UnsupportedSelection;
    let Some(host) = location.host_str() else {
        return Err(invalid);
    };
    if location.scheme() != "https"
        || !name(host)
        || location.port().is_some()
        || location.username() != ""
        || location.password().is_some()
        || location.query().is_some()
        || location.fragment().is_some()
        || location.as_str().contains('%')
    {
        return Err(invalid);
    }
    let path = location.path().strip_prefix("/v2/").ok_or(invalid)?;
    let (repository, digest) = path.split_once("/manifests/sha256:").ok_or(invalid)?;
    if digest != expected.as_str()
        || host.len() + 1 + repository.len() > 255
        || !repository.split('/').all(repository_part)
    {
        return Err(invalid);
    }
    Ok(format!("{host}/{repository}@sha256:{digest}"))
}

fn check_mount(pod: &Value, container: &Value, target: &GitOpsConfigMapTarget) -> Result<()> {
    // Repointing a shared volume would change an unselected container's input.
    for family in ["containers", "initContainers", "ephemeralContainers"] {
        if let Some(others) = pod.get(family) {
            for other in others
                .as_array()
                .ok_or(GitOpsPreparationError::InvalidTarget)?
            {
                if family == "containers" && std::ptr::eq(other, container) {
                    continue;
                }
                if let Some(mounts) = other.get("volumeMounts") {
                    for mount in mounts
                        .as_array()
                        .ok_or(GitOpsPreparationError::InvalidTarget)?
                    {
                        if string(mount, "name")? == target.volume {
                            return Err(GitOpsPreparationError::InvalidTarget);
                        }
                    }
                }
            }
        }
    }
    let volumes = array(pod, "volumes")?;
    let volume = &volumes[named_index(volumes, &target.volume)?];
    if object(volume)?
        .keys()
        .any(|key| key != "name" && key != "configMap")
    {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let source = &volume["configMap"];
    if string(source, "name")? != target.name
        || source
            .get("optional")
            .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let items = array(source, "items")?;
    let actual = items
        .iter()
        .map(|item| Ok((string(item, "key")?, string(item, "path")?)))
        .collect::<Result<BTreeSet<_>>>()?;
    let expected = BTreeSet::from([
        (
            target.configuration_key.as_str(),
            target.configuration_file.as_str(),
        ),
        (target.policy_key.as_str(), target.policy_file.as_str()),
    ]);
    if items.len() != 2 || actual != expected {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let mounts = array(container, "volumeMounts")?;
    let selected = named_index(mounts, &target.volume)?;
    for (index, mount) in mounts.iter().enumerate() {
        let path = string(mount, "mountPath")?;
        if !path.starts_with('/') || !relative(&path[1..]) {
            return Err(GitOpsPreparationError::InvalidTarget);
        }
        if index == selected {
            if path != target.mount_path
                || mount.get("readOnly") != Some(&Value::Bool(true))
                || mount.get("subPath").is_some()
                || mount.get("subPathExpr").is_some()
            {
                return Err(GitOpsPreparationError::InvalidTarget);
            }
        } else if path == target.mount_path
            || path.starts_with(&format!("{}/", target.mount_path))
            || target.mount_path.starts_with(&format!("{path}/"))
        {
            return Err(GitOpsPreparationError::InvalidTarget);
        }
    }
    Ok(())
}

/// Produce replacements only. The caller owns profile admission, Git-base
/// verification and later approval/apply; matching hashes do not confer those.
pub fn prepare_kubernetes_gitops(
    manifest: &ValidatedManifest,
    profile_bytes: &[u8],
    profile_sha256: &ContentDigest,
    base_documents: &BTreeMap<String, String>,
    configuration: &[u8],
    policy: &[u8],
) -> Result<PreparedKubernetesGitOps> {
    if ContentDigest::of_bytes(profile_bytes) != *profile_sha256 {
        return Err(GitOpsPreparationError::BindingMismatch);
    }
    let profile: KubernetesGitOpsProfile = serde_json::from_value(json(profile_bytes)?)
        .map_err(|_| GitOpsPreparationError::InvalidInput)?;
    let d = &profile.deployment;
    let c = &profile.config_map;
    let repository = url::Url::parse(&profile.source_repository)
        .map_err(|_| GitOpsPreparationError::InvalidInput)?;
    if profile.schema_version != 1
        || !matches!(profile.source_commit.len(), 40 | 64)
        || profile.source_repository.len() > 1024
        || repository.scheme() != "https"
        || repository.host_str().is_none()
        || !repository.username().is_empty()
        || repository.password().is_some()
        || repository.query().is_some()
        || repository.fragment().is_some()
        || !profile
            .source_commit
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || !relative(&d.path)
        || !relative(&c.path)
        || paths_conflict(&d.path, &c.path)
        || !relative(&c.snapshot_directory)
        || [&d.path, &c.path].iter().any(|path| {
            let path = path.to_ascii_lowercase();
            let directory = c.snapshot_directory.to_ascii_lowercase();
            directory == path || directory.starts_with(&format!("{path}/"))
        })
        || d.namespace != c.namespace
        || !label(&d.namespace)
        || !name(&d.name)
        || !name(&c.name)
        || !label(&d.container)
        || !label(&c.volume)
        || !key(&c.configuration_key)
        || !key(&c.policy_key)
        || c.configuration_key == c.policy_key
        || !key(&c.configuration_file)
        || !key(&c.policy_file)
        || c.configuration_file == c.policy_file
        || !c.mount_path.starts_with('/')
        || !relative(&c.mount_path[1..])
        || base_documents.len() != 2
    {
        return Err(GitOpsPreparationError::InvalidInput);
    }
    let selected = manifest.manifest();
    if selected.components.len() != 1
        || selected.exports.len() != 1
        || !selected.required_contracts.is_empty()
        || !selected.host_services.is_empty()
        || !selected.portable_mcp_entries.is_empty()
    {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    }
    let component = &selected.components[0];
    if component.runtime != RuntimeKind::Container
        || component.placement != profile.placement
        || component.lifetime != ComponentLifetime::Deployment
        || component.activation_scope != ActivationScope::Independent
        || !component.dependencies.is_empty()
        || !component.state.is_empty()
        || !component.required_contracts.is_empty()
        || !component.host_services.is_empty()
    {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    }
    let Export::ServiceConnector(export) = &selected.exports[0] else {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    };
    if export.authentication != AuthenticationRequirement::None || !export.events.is_empty() {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    }
    if !manifest.check_host(&profile.host).is_compatible() {
        return Err(GitOpsPreparationError::IncompatibleHost);
    }
    let ComponentSource::Artifact {
        location,
        content_digest,
        dependencies: ArtifactDependencies::SelfContained,
        entrypoint,
    } = &component.source
    else {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    };
    let image = image_reference(location, content_digest)?;
    if !entrypoint.executable.contains('/')
        || entrypoint.arguments.len() > 128
        || entrypoint.executable.contains('$')
        || entrypoint
            .arguments
            .iter()
            .any(|arg| arg.len() > 4096 || arg.contains('$') || arg.contains('\0'))
    {
        return Err(GitOpsPreparationError::UnsupportedSelection);
    }
    let configuration_sha256 = ContentDigest::of_bytes(configuration);
    let policy_sha256 = ContentDigest::of_bytes(policy);
    if configuration_sha256 != selected.configuration.content_digest
        || policy_sha256 != selected.policy.content_digest
    {
        return Err(GitOpsPreparationError::BindingMismatch);
    }
    let configuration = text(configuration)?;
    let policy = text(policy)?;
    let mut deployment = target(
        base_documents
            .get(&d.path)
            .ok_or(GitOpsPreparationError::InvalidTarget)?,
        &d.sha256,
        "apps/v1",
        "Deployment",
        &d.namespace,
        &d.name,
    )?;
    let mut config_map = target(
        base_documents
            .get(&c.path)
            .ok_or(GitOpsPreparationError::InvalidTarget)?,
        &c.sha256,
        "v1",
        "ConfigMap",
        &c.namespace,
        &c.name,
    )?;
    if config_map.get("immutable").is_some_and(|v| !v.is_boolean())
        || ["ownerReferences", "finalizers"]
            .iter()
            .any(|field| config_map["metadata"].get(field).is_some())
        || config_map["metadata"]["annotations"]
            .get("kubectl.kubernetes.io/last-applied-configuration")
            .is_some()
    {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let data = object(&config_map["data"])?;
    if !data.values().all(Value::is_string)
        || !data.contains_key(&c.configuration_key)
        || !data.contains_key(&c.policy_key)
        || config_map.get("binaryData").is_some_and(|value| {
            value.get(&c.configuration_key).is_some() || value.get(&c.policy_key).is_some()
        })
    {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let pod = &deployment["spec"]["template"]["spec"];
    let containers = array(pod, "containers")?;
    let index = named_index(containers, &d.container)?;
    let container = &containers[index];
    if string(container, "workingDir")? != "/" {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    check_mount(pod, container, c)?;
    let metadata = deployment["spec"]["template"]
        .get("metadata")
        .ok_or(GitOpsPreparationError::InvalidTarget)?;
    object(metadata)?;
    if let Some(annotations) = metadata.get("annotations") {
        let annotations = object(annotations)?;
        if !annotations.values().all(Value::is_string) {
            return Err(GitOpsPreparationError::InvalidTarget);
        }
        for key in [
            MANIFEST_ANNOTATION,
            CONFIGURATION_ANNOTATION,
            POLICY_ANNOTATION,
        ] {
            if let Some(value) = annotations.get(key) {
                ContentDigest::parse(
                    value
                        .as_str()
                        .ok_or(GitOpsPreparationError::InvalidTarget)?,
                )
                .map_err(|_| GitOpsPreparationError::InvalidTarget)?;
            }
        }
    }
    let snapshot_digest = ContentDigest::of_bytes(&canonical(&serde_json::json!({
        "manifest": manifest.digest(), "configuration": configuration_sha256, "policy": policy_sha256,
        "profile": profile_sha256,
    }))?);
    let snapshot_name = format!("toolkit-{}", &snapshot_digest.as_str()[..48]);
    let snapshot_path = format!("{}/{}.json", c.snapshot_directory, snapshot_name);
    if paths_conflict(&snapshot_path, &d.path) || paths_conflict(&snapshot_path, &c.path) {
        return Err(GitOpsPreparationError::InvalidTarget);
    }
    let container = &mut deployment["spec"]["template"]["spec"]["containers"][index];
    container["image"] = Value::String(image);
    container["command"] = serde_json::json!([entrypoint.executable]);
    container["args"] = serde_json::json!(entrypoint.arguments);
    let metadata = deployment["spec"]["template"]["metadata"]
        .as_object_mut()
        .ok_or(GitOpsPreparationError::InvalidTarget)?;
    let annotations = metadata
        .entry("annotations")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or(GitOpsPreparationError::InvalidTarget)?;
    for (key, value) in [
        (MANIFEST_ANNOTATION, manifest.digest()),
        (CONFIGURATION_ANNOTATION, &configuration_sha256),
        (POLICY_ANNOTATION, &policy_sha256),
    ] {
        annotations.insert(key.into(), Value::String(value.to_string()));
    }
    let volumes = array(&deployment["spec"]["template"]["spec"], "volumes")?;
    let volume_index = named_index(volumes, &c.volume)?;
    deployment["spec"]["template"]["spec"]["volumes"][volume_index]["configMap"]["name"] =
        Value::String(snapshot_name.clone());
    config_map["metadata"]["name"] = Value::String(snapshot_name);
    config_map["immutable"] = Value::Bool(true);
    config_map["data"][&c.configuration_key] = Value::String(configuration);
    config_map["data"][&c.policy_key] = Value::String(policy);
    let mut files = BTreeMap::new();
    if canonical(&config_map)?.len() > 1024 * 1024 {
        return Err(GitOpsPreparationError::OutputLimit);
    }
    for (path, value, action) in [
        (&d.path, deployment, "replace"),
        (&snapshot_path, config_map, "create"),
    ] {
        let mut bytes = canonical(&value)?;
        bytes.push(b'\n');
        let sha256 = ContentDigest::of_bytes(&bytes);
        let contents =
            String::from_utf8(bytes).map_err(|_| GitOpsPreparationError::InvalidInput)?;
        files.insert(
            path.clone(),
            GitOpsReplacement {
                action,
                sha256,
                contents,
            },
        );
    }
    let proposal = KubernetesGitOpsProposal {
        source_commit: profile.source_commit,
        source_repository: profile.source_repository,
        host_owner: profile.owner,
        profile_sha256: profile_sha256.clone(),
        host_capabilities_sha256: ContentDigest::of_bytes(&canonical(&profile.host)?),
        manifest_sha256: manifest.digest().clone(),
        manifest_json: String::from_utf8(
            manifest
                .canonical_bytes()
                .map_err(|_| GitOpsPreparationError::InvalidInput)?,
        )
        .map_err(|_| GitOpsPreparationError::InvalidInput)?,
        configuration_sha256,
        policy_sha256,
        base_sha256: BTreeMap::from([
            (d.path.clone(), d.sha256.clone()),
            (c.path.clone(), c.sha256.clone()),
        ]),
        files,
    };
    let result = PreparedKubernetesGitOps {
        schema_version: 1,
        status: "prepared",
        proposal_sha256: ContentDigest::of_bytes(&canonical(&proposal)?),
        proposal,
    };
    if canonical(&result)?.len() + 1 > GITOPS_MAX_OUTPUT {
        return Err(GitOpsPreparationError::OutputLimit);
    }
    Ok(result)
}

/// Bounded wire entry point shared by the CLI and ordinary library consumers.
pub fn prepare_kubernetes_gitops_json(bytes: &[u8]) -> Result<PreparedKubernetesGitOps> {
    if bytes.len() > GITOPS_MAX_INPUT {
        return Err(GitOpsPreparationError::InputLimit);
    }
    let value = strict_mcpb_json(bytes).map_err(|_| GitOpsPreparationError::InvalidInput)?;
    let request: KubernetesGitOpsRequest =
        serde_json::from_value(value).map_err(|_| GitOpsPreparationError::InvalidInput)?;
    if request.schema_version != 1 {
        return Err(GitOpsPreparationError::InvalidInput);
    }
    let package: IntegrationPackage =
        serde_json::from_value(json(request.package_json.as_bytes())?)
            .map_err(|_| GitOpsPreparationError::InvalidInput)?;
    let manifest: ActivationManifest =
        serde_json::from_value(json(request.manifest_json.as_bytes())?)
            .map_err(|_| GitOpsPreparationError::InvalidInput)?;
    let validated = manifest
        .validate(&package)
        .map_err(|_| GitOpsPreparationError::UnsupportedSelection)?;
    prepare_kubernetes_gitops(
        &validated,
        request.profile_json.as_bytes(),
        &request.profile_sha256,
        &request.base_documents,
        request.configuration.as_bytes(),
        request.policy.as_bytes(),
    )
}
