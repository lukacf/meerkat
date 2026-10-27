//! Pure preparation for an explicitly host-selected local Compose backend.
//!
//! Docker owns execution facts; this module never contacts it. The caller owns
//! admission of the exact returned payload. Neither input hashes nor matching
//! host declarations confer permission or establish component/role readiness.

use crate::{
    ActivationManifest, ActivationScope, ArtifactDependencies, ComponentLifetime, ComponentSource,
    ContentDigest, Entrypoint, Export, HostCapabilities, Id, IntegrationPackage, RollbackContract,
    RuntimeKind, ShutdownTimeout, StateChange, ValidatedManifest, canonical_json, strict_mcpb_json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

pub const COMPOSE_MAX_INPUT: usize = 2 * 1024 * 1024;
pub const COMPOSE_MAX_OUTPUT: usize = 8 * 1024 * 1024;
const MAX_DOCUMENT: usize = 256 * 1024;
const MAX_FILE: usize = 64 * 1024;
const MAX_COMPONENTS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ComposePreparationError {
    #[error("invalid_input")]
    InvalidInput,
    #[error("input_limit")]
    InputLimit,
    #[error("binding_mismatch")]
    BindingMismatch,
    #[error("invalid_profile")]
    InvalidProfile,
    #[error("unsupported_selection")]
    UnsupportedSelection,
    #[error("incompatible_host")]
    IncompatibleHost,
    #[error("output_limit")]
    OutputLimit,
}
type Result<T> = std::result::Result<T, ComposePreparationError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeTool {
    pub path: String,
    pub sha256: ContentDigest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeBindSourcePrefix {
    pub local_root: String,
    pub engine_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeEngine {
    /// Explicit local Unix socket, never an ambient Docker context.
    pub endpoint: String,
    pub id: String,
    pub docker: ComposeTool,
    pub compose: ComposeTool,
    /// Host-declared engine path spelling, not inferred from the local platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_source_prefix: Option<ComposeBindSourcePrefix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeResources {
    pub cpu_millis: u32,
    pub memory_bytes: u64,
    pub pids_limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeHealthcheck {
    /// Executed directly inside the selected image, without CMD-SHELL.
    pub argv: Vec<String>,
    pub interval_ms: u32,
    pub timeout_ms: u32,
    pub retries: u16,
    pub start_period_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ComposeMount {
    /// Source is configuration, policy, or files/<profile-pinned filename>.
    File { source: String, target: String },
    Volume {
        source: Id,
        target: String,
        read_only: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeComponentProfile {
    pub service: String,
    /// Inspected platform image ID. The backend must verify it before execution.
    pub image_id: String,
    pub platform: String,
    pub user: String,
    pub environment: BTreeMap<String, String>,
    pub mounts: Vec<ComposeMount>,
    pub resources: ComposeResources,
    pub healthcheck: Option<ComposeHealthcheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeVolumeProfile {
    /// Exact project-scoped physical name; this does not claim it is unoccupied.
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeStateMigrationProfile {
    pub protocol_version: u16,
    pub namespace: Id,
    pub operation: Id,
    pub apply: Entrypoint,
    pub readback: Entrypoint,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeProfile {
    pub schema_version: u16,
    pub owner: Id,
    pub placement: Id,
    pub host: HostCapabilities,
    pub project: String,
    pub engine: ComposeEngine,
    pub components: BTreeMap<Id, ComposeComponentProfile>,
    pub volumes: BTreeMap<Id, ComposeVolumeProfile>,
    pub files: BTreeMap<String, ContentDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_migration: Option<ComposeStateMigrationProfile>,
}

/// Raw strings retain source-byte bindings, including insignificant whitespace.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeRequest {
    pub schema_version: u16,
    pub package_json: String,
    pub manifest_json: String,
    pub profile_json: String,
    pub profile_sha256: ContentDigest,
    pub configuration: String,
    pub policy: String,
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposePreparedFile {
    pub sha256: ContentDigest,
    /// Exact bytes as UTF-8. Treat these as host configuration, not diagnostics.
    pub contents: String,
    pub mode: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposePreparedMount {
    pub kind: &'static str,
    /// File: fixed workspace-relative path. Volume: exact physical volume name.
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposePreparedComponent {
    pub service: String,
    pub lifetime: ComponentLifetime,
    pub image: String,
    pub image_id: String,
    pub platform: String,
    pub user: String,
    pub entrypoint: Vec<String>,
    pub command: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub resources: ComposeResources,
    pub mounts: Vec<ComposePreparedMount>,
    pub dependencies: Vec<Id>,
    pub deployment_dependencies: Vec<Id>,
    pub readiness: &'static str,
    pub stop_timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComposePreparedVolume {
    pub name: String,
    pub component: Id,
    pub owner: Id,
    pub format: crate::Revision,
}

/// Pinned owner capability only. Ordinary preparation/start never invokes it.
#[derive(Debug, Clone, Serialize)]
pub struct ComposePreparedStateMigration {
    pub protocol_version: u16,
    pub namespace: Id,
    pub component: Id,
    pub owner: Id,
    pub from_format: crate::Revision,
    pub to_format: crate::Revision,
    pub operation: Id,
    pub artifact_digest: ContentDigest,
    pub apply: Entrypoint,
    pub readback: Entrypoint,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComposePlan {
    pub request_sha256: ContentDigest,
    pub package_sha256: ContentDigest,
    pub manifest_source_sha256: ContentDigest,
    pub manifest_sha256: ContentDigest,
    pub profile_sha256: ContentDigest,
    pub configuration_sha256: ContentDigest,
    pub policy_sha256: ContentDigest,
    pub host_owner: Id,
    pub project: String,
    pub network: String,
    pub engine: ComposeEngine,
    pub compose_json: String,
    pub files: BTreeMap<String, ComposePreparedFile>,
    pub components: BTreeMap<Id, ComposePreparedComponent>,
    pub initialization_order: Vec<Id>,
    pub shutdown_order: Vec<Id>,
    pub deployment_order: Vec<Id>,
    pub request_components: Vec<Id>,
    /// Full custody inventory, including state of components removed by a successor.
    pub volumes: BTreeMap<Id, ComposePreparedVolume>,
    /// Original creation labels remain required even when Compose uses external volumes.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub retained_volume_labels: BTreeMap<Id, BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_migration: Option<ComposePreparedStateMigration>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreparedCompose {
    pub schema_version: u16,
    pub status: &'static str,
    pub plan_sha256: ContentDigest,
    pub plan: ComposePlan,
}

fn canonical(value: &impl Serialize) -> Result<Vec<u8>> {
    canonical_json(value).map_err(|_| ComposePreparationError::InvalidInput)
}

fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    if bytes.len() > MAX_DOCUMENT {
        return Err(ComposePreparationError::InputLimit);
    }
    let value = strict_mcpb_json(bytes).map_err(|_| ComposePreparationError::InvalidInput)?;
    serde_json::from_value(value).map_err(|_| ComposePreparationError::InvalidInput)
}

fn label(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
}

fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn absolute(value: &str) -> bool {
    value.strip_prefix('/').is_some_and(relative)
}

fn literal(value: &str, maximum: usize) -> bool {
    value.len() <= maximum && !value.chars().any(char::is_control) && !value.contains('$')
}

fn valid_entrypoint(entrypoint: &Entrypoint) -> bool {
    entrypoint.python_console_script.is_none()
        && entrypoint.executable.contains('/')
        && relative(&entrypoint.executable)
        && entrypoint.arguments.len() <= 128
        && entrypoint.arguments.iter().all(|s| literal(s, 4096))
}

fn overlaps(left: &str, right: &str) -> bool {
    left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}

fn uid_gid(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 2
        && parts.iter().all(|p| {
            p.parse::<u32>()
                .is_ok_and(|v| v <= i32::MAX as u32 && v.to_string() == *p)
        })
}

fn prepared_file(contents: String, mode: u32) -> ComposePreparedFile {
    ComposePreparedFile {
        sha256: ContentDigest::of_bytes(contents.as_bytes()),
        contents,
        mode,
    }
}

fn check_text(value: &str) -> Result<()> {
    if value.len() > MAX_FILE {
        return Err(ComposePreparationError::InputLimit);
    }
    if value.contains('\0') {
        return Err(ComposePreparationError::InvalidInput);
    }
    Ok(())
}

fn validate_profile(profile: &ComposeProfile) -> Result<()> {
    if profile.schema_version != 1
        || !label(&profile.project, 48)
        || profile.components.is_empty()
        || profile.components.len() > MAX_COMPONENTS
        || profile.volumes.len() > 16
        || profile.files.len() > 64
        || !profile
            .engine
            .endpoint
            .strip_prefix("unix://")
            .is_some_and(absolute)
        || profile.engine.id.is_empty()
        || !literal(&profile.engine.id, 128)
        || !absolute(&profile.engine.docker.path)
        || !absolute(&profile.engine.compose.path)
    {
        return Err(ComposePreparationError::InvalidProfile);
    }
    if let Some(migration) = &profile.state_migration
        && (migration.protocol_version != 1
            || !(1000..=120_000).contains(&migration.timeout_ms)
            || !valid_entrypoint(&migration.apply)
            || !valid_entrypoint(&migration.readback))
    {
        return Err(ComposePreparationError::InvalidProfile);
    }
    if let Some(prefix) = &profile.engine.bind_source_prefix
        && (!absolute(&prefix.local_root)
            || !absolute(&prefix.engine_root)
            || overlaps(&prefix.local_root, &prefix.engine_root))
    {
        return Err(ComposePreparationError::InvalidProfile);
    }
    let mut services = BTreeSet::new();
    for component in profile.components.values() {
        let r = &component.resources;
        if !label(&component.service, 48)
            || !services.insert(&component.service)
            || !component
                .image_id
                .strip_prefix("sha256:")
                .is_some_and(|v| ContentDigest::parse(v).is_ok())
            || !matches!(component.platform.as_str(), "linux/arm64" | "linux/amd64")
            || !uid_gid(&component.user)
            || component.environment.len() > 64
            || component.mounts.len() > 64
            || r.cpu_millis == 0
            || r.cpu_millis > 32_000
            || !(8 * 1024 * 1024..=8 * 1024 * 1024 * 1024).contains(&r.memory_bytes)
            || !(16..=4096).contains(&r.pids_limit)
        {
            return Err(ComposePreparationError::InvalidProfile);
        }
        for (key, value) in &component.environment {
            if key.is_empty()
                || key.len() > 128
                || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                || key.as_bytes()[0].is_ascii_digit()
                || !literal(value, 4096)
            {
                return Err(ComposePreparationError::InvalidProfile);
            }
        }
        if let Some(health) = &component.healthcheck
            && (health.argv.is_empty()
                || health.argv.len() > 128
                || health.argv[0].is_empty()
                || health.argv.iter().any(|v| !literal(v, 4096))
                || !(1000..=60_000).contains(&health.interval_ms)
                || !(100..=30_000).contains(&health.timeout_ms)
                || !(1..=60).contains(&health.retries)
                || health.start_period_ms > 120_000)
        {
            return Err(ComposePreparationError::InvalidProfile);
        }
    }
    let prefix = format!("{}-", profile.project);
    let mut volumes = BTreeSet::new();
    for volume in profile.volumes.values() {
        if !label(&volume.name, 96)
            || !volume.name.starts_with(&prefix)
            || !volumes.insert(&volume.name)
        {
            return Err(ComposePreparationError::InvalidProfile);
        }
    }
    for name in profile.files.keys() {
        if !relative(name) {
            return Err(ComposePreparationError::InvalidProfile);
        }
    }
    let names: Vec<_> = profile.files.keys().collect();
    for (index, name) in names.iter().enumerate() {
        if names[index + 1..].iter().any(|other| overlaps(name, other)) {
            return Err(ComposePreparationError::InvalidProfile);
        }
    }
    Ok(())
}

fn prepare(
    request: ComposeRequest,
    request_sha256: ContentDigest,
    validated: ValidatedManifest,
) -> Result<PreparedCompose> {
    if ContentDigest::of_bytes(request.profile_json.as_bytes()) != request.profile_sha256 {
        return Err(ComposePreparationError::BindingMismatch);
    }
    let profile: ComposeProfile = parse(request.profile_json.as_bytes())?;
    validate_profile(&profile)?;
    let manifest = validated.manifest();
    if manifest.components.len() != profile.components.len()
        || manifest
            .components
            .iter()
            .any(|c| !profile.components.contains_key(&c.id))
        || !manifest.required_contracts.is_empty()
        || !manifest.host_services.is_empty()
        || !manifest.portable_mcp_entries.is_empty()
        || manifest
            .exports
            .iter()
            .any(|export| !matches!(export, Export::ServiceConnector(e) if e.events.is_empty()))
    {
        return Err(ComposePreparationError::UnsupportedSelection);
    }
    if !validated.check_host(&profile.host).is_compatible() {
        return Err(ComposePreparationError::IncompatibleHost);
    }
    if ContentDigest::of_bytes(request.configuration.as_bytes())
        != manifest.configuration.content_digest
        || ContentDigest::of_bytes(request.policy.as_bytes()) != manifest.policy.content_digest
        || request.files.len() != profile.files.len()
    {
        return Err(ComposePreparationError::BindingMismatch);
    }
    check_text(&request.configuration)?;
    check_text(&request.policy)?;
    let mut files = BTreeMap::from([
        (
            "inputs/configuration".into(),
            prepared_file(request.configuration.clone(), 0o444),
        ),
        (
            "inputs/policy".into(),
            prepared_file(request.policy.clone(), 0o444),
        ),
    ]);
    for (name, value) in &request.files {
        check_text(value)?;
        if profile.files.get(name) != Some(&ContentDigest::of_bytes(value.as_bytes())) {
            return Err(ComposePreparationError::BindingMismatch);
        }
        files.insert(
            format!("inputs/files/{name}"),
            prepared_file(value.clone(), 0o444),
        );
    }
    let mut volumes = BTreeMap::new();
    let mut state_migration = None;
    let mut closure: BTreeMap<Id, BTreeSet<Id>> = BTreeMap::new();
    let mut edge_count = 0;
    for id in validated.initialization_order() {
        let component = manifest
            .components
            .iter()
            .find(|c| &c.id == id)
            .ok_or(ComposePreparationError::InvalidInput)?;
        if component.runtime != RuntimeKind::Container
            || component.placement != profile.placement
            || component.activation_scope != ActivationScope::Independent
            || component.lifetime == ComponentLifetime::ExternallyManaged
            || !component.required_contracts.is_empty()
            || !component.host_services.is_empty()
            || component.readiness.owner != profile.owner
            || component.shutdown.owner != profile.owner
            || component.shutdown.operation.as_str() != "stop"
            || component.shutdown.timeout != ShutdownTimeout::PreserveUncertainty
            || !(1000..=120_000).contains(&component.shutdown.maximum_wait_ms)
            || component.shutdown.maximum_wait_ms % 1000 != 0
            || !matches!(&component.rollback, RollbackContract::OwnerReconciliation { owner, operation } if owner == &profile.owner && operation.as_str() == "reconcile")
        {
            return Err(ComposePreparationError::UnsupportedSelection);
        }
        let mut dependencies = BTreeSet::new();
        for dependency in &component.dependencies {
            edge_count += 1;
            let target = manifest
                .components
                .iter()
                .find(|c| c.id == dependency.component)
                .ok_or(ComposePreparationError::InvalidInput)?;
            // One-shot dependency completion is not implemented by this profile.
            if target.lifetime != ComponentLifetime::Deployment {
                return Err(ComposePreparationError::UnsupportedSelection);
            }
            dependencies.insert(target.id.clone());
            dependencies.extend(
                closure
                    .get(&target.id)
                    .ok_or(ComposePreparationError::InvalidInput)?
                    .iter()
                    .cloned(),
            );
        }
        closure.insert(component.id.clone(), dependencies);
        for state in &component.state {
            if component.lifetime != ComponentLifetime::Deployment {
                return Err(ComposePreparationError::UnsupportedSelection);
            }
            if !state.migrations.is_empty() {
                let binding = profile
                    .state_migration
                    .as_ref()
                    .filter(|binding| binding.namespace == state.namespace)
                    .ok_or(ComposePreparationError::UnsupportedSelection)?;
                let [migration] = state.migrations.as_slice() else {
                    return Err(ComposePreparationError::UnsupportedSelection);
                };
                let ComponentSource::Artifact { content_digest, .. } = &component.source else {
                    return Err(ComposePreparationError::UnsupportedSelection);
                };
                if state_migration.is_some()
                    || migration.operation != binding.operation
                    || migration.artifact_digest != *content_digest
                {
                    return Err(ComposePreparationError::UnsupportedSelection);
                }
                state_migration = Some(ComposePreparedStateMigration {
                    protocol_version: binding.protocol_version,
                    namespace: state.namespace.clone(),
                    component: component.id.clone(),
                    owner: state.owner.clone(),
                    from_format: migration.from.clone(),
                    to_format: state.format.clone(),
                    operation: migration.operation.clone(),
                    artifact_digest: migration.artifact_digest.clone(),
                    apply: binding.apply.clone(),
                    readback: binding.readback.clone(),
                    timeout_ms: binding.timeout_ms,
                });
            }
            let volume = profile
                .volumes
                .get(&state.namespace)
                .ok_or(ComposePreparationError::InvalidProfile)?;
            volumes.insert(
                state.namespace.clone(),
                ComposePreparedVolume {
                    name: volume.name.clone(),
                    component: component.id.clone(),
                    owner: state.owner.clone(),
                    format: state.format.clone(),
                },
            );
        }
    }
    if edge_count > 64 {
        return Err(ComposePreparationError::InputLimit);
    }
    if volumes.len() != profile.volumes.len() {
        return Err(ComposePreparationError::InvalidProfile);
    }
    if profile.state_migration.is_some() != state_migration.is_some() {
        return Err(ComposePreparationError::InvalidProfile);
    }
    let labels = json!({
        "toolkit.rkat.ai/manifest-sha256": validated.digest(),
        "toolkit.rkat.ai/profile-sha256": request.profile_sha256,
        "toolkit.rkat.ai/request-sha256": request_sha256,
    });
    let network = format!("{}-network", profile.project);
    let mut components = BTreeMap::new();
    let mut services = BTreeMap::new();
    let mut written_volumes = BTreeSet::new();
    for component in &manifest.components {
        let binding = &profile.components[&component.id];
        let ComponentSource::Artifact {
            location,
            content_digest,
            dependencies: ArtifactDependencies::SelfContained,
            entrypoint,
        } = &component.source
        else {
            return Err(ComposePreparationError::UnsupportedSelection);
        };
        let image = if location.scheme() == "docker-image" {
            let image = crate::manifest::local_container_image(location, content_digest)
                .ok_or(ComposePreparationError::UnsupportedSelection)?;
            if image != binding.image_id {
                return Err(ComposePreparationError::BindingMismatch);
            }
            image
        } else {
            crate::gitops::image_reference(location, content_digest)
                .map_err(|_| ComposePreparationError::UnsupportedSelection)?
        };
        if !valid_entrypoint(entrypoint) {
            return Err(ComposePreparationError::UnsupportedSelection);
        }
        let checks: Vec<_> = component.readiness.checks.iter().map(Id::as_str).collect();
        let readiness = match (checks.as_slice(), &binding.healthcheck) {
            (["running"], None) => "running",
            (["healthy"], Some(_)) => "healthy",
            _ => return Err(ComposePreparationError::UnsupportedSelection),
        };
        let mut mounts = Vec::new();
        let mut compose_mounts = Vec::new();
        let mut targets: Vec<String> = Vec::new();
        let mut sources = BTreeSet::new();
        for mount in &binding.mounts {
            let (kind, source, target, read_only) = match mount {
                ComposeMount::File { source, target } => {
                    let path = format!("inputs/{source}");
                    if !files.contains_key(&path) {
                        return Err(ComposePreparationError::InvalidProfile);
                    }
                    ("file", path, target, true)
                }
                ComposeMount::Volume {
                    source,
                    target,
                    read_only,
                } => {
                    let volume = volumes
                        .get(source)
                        .ok_or(ComposePreparationError::InvalidProfile)?;
                    if volume.component != component.id
                        && (!read_only || !closure[&component.id].contains(&volume.component))
                    {
                        return Err(ComposePreparationError::InvalidProfile);
                    }
                    if !read_only {
                        written_volumes.insert(source.clone());
                    }
                    ("volume", volume.name.clone(), target, *read_only)
                }
            };
            // This is lexical protection only. Image filesystem symlinks and
            // executable paths remain part of the host's image review.
            if !absolute(target)
                || targets.iter().any(|previous| overlaps(previous, target))
                || overlaps(target, &format!("/{}", entrypoint.executable))
                || state_migration.as_ref().is_some_and(|migration| {
                    migration.component == component.id
                        && [&migration.apply, &migration.readback]
                            .iter()
                            .any(|entry| overlaps(target, &format!("/{}", entry.executable)))
                })
                || ["/proc", "/sys", "/dev"]
                    .iter()
                    .any(|reserved| overlaps(target, reserved))
                || !sources.insert((kind, source.clone()))
            {
                return Err(ComposePreparationError::InvalidProfile);
            }
            targets.push(target.clone());
            if kind == "file" {
                compose_mounts.push(
                    json!({"type":"bind", "source":format!("./{source}"), "target":target,
                    "read_only":true, "bind":{"create_host_path":false}}),
                );
            } else {
                compose_mounts.push(json!({"type":"volume", "source":source, "target":target,
                    "read_only":read_only, "volume":{"nocopy":true}}));
            }
            mounts.push(ComposePreparedMount {
                kind,
                source,
                target: target.clone(),
                read_only,
            });
        }
        let dependencies: Vec<_> = component
            .dependencies
            .iter()
            .map(|d| d.component.clone())
            .collect();
        let deployment_dependencies: Vec<_> = validated
            .initialization_order()
            .iter()
            .filter(|id| closure[&component.id].contains(*id))
            .cloned()
            .collect();
        let mut depends_on = BTreeMap::new();
        for dependency in &dependencies {
            let dep = &profile.components[dependency];
            depends_on.insert(dep.service.clone(), json!({"condition": if dep.healthcheck.is_some() {"service_healthy"} else {"service_started"}}));
        }
        let mut service = json!({
            "image":image, "platform":binding.platform, "pull_policy":"never", "user":binding.user,
            "working_dir":"/", "entrypoint":[entrypoint.executable], "command":entrypoint.arguments,
            "environment":binding.environment, "init":true, "restart":"no", "tty":false,
            "stdin_open":component.lifetime == ComponentLifetime::Request,
            "cpus":format!("{}.{:03}", binding.resources.cpu_millis / 1000, binding.resources.cpu_millis % 1000),
            "mem_limit":binding.resources.memory_bytes, "memswap_limit":binding.resources.memory_bytes,
            "pids_limit":binding.resources.pids_limit,
            "stop_grace_period":format!("{}ms", component.shutdown.maximum_wait_ms),
            "labels":labels, "networks":{"default":{"aliases":[binding.service]}},
            "volumes":compose_mounts, "depends_on":depends_on,
        });
        if let Some(health) = &binding.healthcheck {
            let mut test = vec!["CMD".to_owned()];
            test.extend(health.argv.iter().cloned());
            service["healthcheck"] = json!({"test":test, "interval":format!("{}ms", health.interval_ms),
                "timeout":format!("{}ms", health.timeout_ms), "retries":health.retries,
                "start_period":format!("{}ms", health.start_period_ms)});
        } else {
            // An image's implicit healthcheck must not add unselected work.
            service["healthcheck"] = json!({"disable":true});
        }
        if component.lifetime == ComponentLifetime::Request {
            service["profiles"] = json!(["request"]);
        }
        services.insert(binding.service.clone(), service);
        components.insert(
            component.id.clone(),
            ComposePreparedComponent {
                service: binding.service.clone(),
                lifetime: component.lifetime.clone(),
                image,
                image_id: binding.image_id.clone(),
                platform: binding.platform.clone(),
                user: binding.user.clone(),
                entrypoint: vec![entrypoint.executable.clone()],
                command: entrypoint.arguments.clone(),
                environment: binding.environment.clone(),
                resources: binding.resources.clone(),
                mounts,
                dependencies,
                deployment_dependencies,
                readiness,
                stop_timeout_ms: component.shutdown.maximum_wait_ms,
            },
        );
    }
    if written_volumes.len() != volumes.len() {
        return Err(ComposePreparationError::InvalidProfile);
    }
    let volume_definitions: BTreeMap<_, _> = volumes
        .values()
        .map(|v| {
            (
                v.name.clone(),
                json!({"name":v.name,"driver":"local","labels":labels}),
            )
        })
        .collect();
    let document = json!({"name":profile.project, "services":services,
        "networks":{"default":{"name":network,"internal":true,"driver":"bridge","labels":labels}},
        "volumes":volume_definitions});
    let mut compose_bytes = canonical(&document)?;
    compose_bytes.push(b'\n');
    let compose_json =
        String::from_utf8(compose_bytes).map_err(|_| ComposePreparationError::InvalidInput)?;
    files.insert(
        "compose.json".into(),
        prepared_file(compose_json.clone(), 0o600),
    );
    let initialization_order = validated.initialization_order().to_vec();
    let plan = ComposePlan {
        request_sha256,
        package_sha256: ContentDigest::of_bytes(request.package_json.as_bytes()),
        manifest_source_sha256: ContentDigest::of_bytes(request.manifest_json.as_bytes()),
        manifest_sha256: validated.digest().clone(),
        profile_sha256: request.profile_sha256,
        configuration_sha256: manifest.configuration.content_digest.clone(),
        policy_sha256: manifest.policy.content_digest.clone(),
        host_owner: profile.owner,
        project: profile.project,
        network,
        engine: profile.engine,
        compose_json,
        files,
        deployment_order: initialization_order
            .iter()
            .filter(|id| components[*id].lifetime == ComponentLifetime::Deployment)
            .cloned()
            .collect(),
        request_components: initialization_order
            .iter()
            .filter(|id| components[*id].lifetime == ComponentLifetime::Request)
            .cloned()
            .collect(),
        shutdown_order: validated.shutdown_order().cloned().collect(),
        initialization_order,
        components,
        volumes,
        retained_volume_labels: BTreeMap::new(),
        state_migration,
    };
    let result = PreparedCompose {
        schema_version: 1,
        status: "prepared",
        plan_sha256: ContentDigest::of_bytes(&canonical(&plan)?),
        plan,
    };
    if canonical(&result)?.len() + 1 > COMPOSE_MAX_OUTPUT {
        return Err(ComposePreparationError::OutputLimit);
    }
    Ok(result)
}

/// Validate and prepare one exact request without reading files or Docker state.
pub fn prepare_compose_json(bytes: &[u8]) -> Result<PreparedCompose> {
    if bytes.len() > COMPOSE_MAX_INPUT {
        return Err(ComposePreparationError::InputLimit);
    }
    let value = strict_mcpb_json(bytes).map_err(|_| ComposePreparationError::InvalidInput)?;
    let request: ComposeRequest =
        serde_json::from_value(value).map_err(|_| ComposePreparationError::InvalidInput)?;
    if request.schema_version != 1 {
        return Err(ComposePreparationError::InvalidInput);
    }
    let package: IntegrationPackage = parse(request.package_json.as_bytes())?;
    let manifest: ActivationManifest = parse(request.manifest_json.as_bytes())?;
    if manifest.components.len() > MAX_COMPONENTS
        || manifest
            .components
            .iter()
            .map(|c| c.dependencies.len())
            .sum::<usize>()
            > 64
    {
        return Err(ComposePreparationError::InputLimit);
    }
    let validated = manifest
        .validate(&package)
        .map_err(|_| ComposePreparationError::UnsupportedSelection)?;
    prepare(request, ContentDigest::of_bytes(bytes), validated)
}

/// Exact source inputs, not a claim that any predecessor or volume exists.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeSuccessorRequest {
    pub schema_version: u16,
    pub origin_request: String,
    pub predecessor_request: String,
    pub successor_request: String,
}

/// Ordered exact selections supplied by the existing runtime custody owner.
/// This pure envelope is not evidence that any historical transition ran.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComposeSuccessorHistoryRequest {
    pub schema_version: u16,
    pub requests: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PreparedComposeSuccessor {
    pub schema_version: u16,
    pub status: &'static str,
    pub transition_sha256: ContentDigest,
    pub predecessor: PreparedCompose,
    pub successor: PreparedCompose,
}

fn successor_compatible(old: &str, new: &str, a: &ComposePlan, b: &ComposePlan) -> Result<bool> {
    let old: ComposeRequest = serde_json::from_value(
        strict_mcpb_json(old.as_bytes()).map_err(|_| ComposePreparationError::InvalidInput)?,
    )
    .map_err(|_| ComposePreparationError::InvalidInput)?;
    let new: ComposeRequest = serde_json::from_value(
        strict_mcpb_json(new.as_bytes()).map_err(|_| ComposePreparationError::InvalidInput)?,
    )
    .map_err(|_| ComposePreparationError::InvalidInput)?;
    let old_package: IntegrationPackage = parse(old.package_json.as_bytes())?;
    let new_package: IntegrationPackage = parse(new.package_json.as_bytes())?;
    let old_manifest: ActivationManifest = parse(old.manifest_json.as_bytes())?;
    let new_manifest: ActivationManifest = parse(new.manifest_json.as_bytes())?;
    let old_valid = old_manifest
        .validate(&old_package)
        .map_err(|_| ComposePreparationError::UnsupportedSelection)?;
    let new_valid = new_manifest
        .validate(&new_package)
        .map_err(|_| ComposePreparationError::UnsupportedSelection)?;
    let changes = new_valid
        .state_changes_from(&old_valid)
        .map_err(|_| ComposePreparationError::UnsupportedSelection)?;
    let old_profile: ComposeProfile = parse(old.profile_json.as_bytes())?;
    let new_profile: ComposeProfile = parse(new.profile_json.as_bytes())?;
    if a.project != b.project
        || a.host_owner != b.host_owner
        || canonical(&a.engine)? != canonical(&b.engine)?
        || old_profile.placement != new_profile.placement
        || canonical(&old_profile.host)? != canonical(&new_profile.host)?
    {
        return Err(ComposePreparationError::UnsupportedSelection);
    }
    let mut migrated = BTreeSet::new();
    for change in &changes {
        match change {
            StateChange::RetainedOnRemoval { namespace, .. }
                if a.volumes
                    .get(namespace)
                    .is_some_and(|volume| !b.components.contains_key(&volume.component)) => {}
            StateChange::Migration {
                namespace,
                owner,
                from,
                to,
                migration,
            } if b.state_migration.as_ref().is_some_and(|binding| {
                binding.namespace == *namespace
                    && binding.owner == *owner
                    && binding.from_format == *from
                    && binding.to_format == *to
                    && binding.operation == migration.operation
                    && binding.artifact_digest == migration.artifact_digest
            }) =>
            {
                migrated.insert(namespace);
            }
            _ => return Err(ComposePreparationError::UnsupportedSelection),
        }
    }
    if migrated.len() > 1 {
        return Err(ComposePreparationError::UnsupportedSelection);
    }
    for (namespace, next) in &b.volumes {
        let prior = a
            .volumes
            .get(namespace)
            .ok_or(ComposePreparationError::UnsupportedSelection)?;
        if prior.name != next.name
            || prior.component != next.component
            || prior.owner != next.owner
            || (prior.format != next.format && !migrated.contains(namespace))
        {
            return Err(ComposePreparationError::UnsupportedSelection);
        }
    }
    // Full predecessor custody includes dormant state. A later raw manifest
    // cannot resurrect its original component, even without declaring state.
    for (namespace, prior) in &a.volumes {
        if !b.volumes.contains_key(namespace) && b.components.contains_key(&prior.component) {
            return Err(ComposePreparationError::UnsupportedSelection);
        }
    }
    Ok(!migrated.is_empty())
}

fn retain_volumes(
    plan: &mut PreparedCompose,
    origin: &ComposePlan,
    prior: &ComposePlan,
) -> Result<()> {
    let original: serde_json::Value = serde_json::from_str(&origin.compose_json)
        .map_err(|_| ComposePreparationError::InvalidInput)?;
    let mut compose: serde_json::Value = serde_json::from_str(&plan.plan.compose_json)
        .map_err(|_| ComposePreparationError::InvalidInput)?;
    // Logical formats follow the immediate predecessor, including dormant
    // namespaces. Immutable creation labels always come from the origin.
    for (namespace, volume) in &prior.volumes {
        plan.plan
            .volumes
            .entry(namespace.clone())
            .or_insert_with(|| volume.clone());
        let physical = origin
            .volumes
            .get(namespace)
            .ok_or(ComposePreparationError::UnsupportedSelection)?;
        let selected = &plan.plan.volumes[namespace];
        if selected.name != physical.name
            || selected.component != physical.component
            || selected.owner != physical.owner
        {
            return Err(ComposePreparationError::UnsupportedSelection);
        }
        let labels = serde_json::from_value(original["volumes"][&physical.name]["labels"].clone())
            .map_err(|_| ComposePreparationError::InvalidInput)?;
        plan.plan
            .retained_volume_labels
            .insert(namespace.clone(), labels);
        compose["volumes"][&volume.name] = json!({"name": volume.name, "external": true});
    }
    let mut bytes = canonical(&compose)?;
    bytes.push(b'\n');
    let text = String::from_utf8(bytes).map_err(|_| ComposePreparationError::InvalidInput)?;
    plan.plan
        .files
        .insert("compose.json".into(), prepared_file(text.clone(), 0o600));
    plan.plan.compose_json = text;
    plan.plan_sha256 = ContentDigest::of_bytes(&canonical(&plan.plan)?);
    Ok(())
}

/// Separate entry point: ordinary planning deliberately rejects custody markers.
pub fn prepare_compose_successor_json(bytes: &[u8]) -> Result<PreparedComposeSuccessor> {
    if bytes.len() > COMPOSE_MAX_INPUT {
        return Err(ComposePreparationError::InputLimit);
    }
    let value = strict_mcpb_json(bytes).map_err(|_| ComposePreparationError::InvalidInput)?;
    let (requests, history) = match value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(1) => {
            let request: ComposeSuccessorRequest =
                serde_json::from_value(value).map_err(|_| ComposePreparationError::InvalidInput)?;
            if request.predecessor_request == request.successor_request {
                return Err(ComposePreparationError::InvalidInput);
            }
            let mut requests = vec![request.origin_request];
            if request.predecessor_request != requests[0] {
                requests.push(request.predecessor_request);
            }
            requests.push(request.successor_request);
            (requests, false)
        }
        Some(2) => {
            let request: ComposeSuccessorHistoryRequest =
                serde_json::from_value(value).map_err(|_| ComposePreparationError::InvalidInput)?;
            (request.requests, true)
        }
        _ => return Err(ComposePreparationError::InvalidInput),
    };
    if requests.len() > 32 {
        return Err(ComposePreparationError::InputLimit);
    }
    if requests.len() < 2 || requests.iter().collect::<BTreeSet<_>>().len() != requests.len() {
        return Err(ComposePreparationError::InvalidInput);
    }
    let origin = prepare_compose_json(requests[0].as_bytes())?;
    let mut predecessor = origin.clone();
    let mut successor = origin.clone();
    let mut migrated = false;
    for pair in requests.windows(2) {
        predecessor = successor;
        successor = prepare_compose_json(pair[1].as_bytes())?;
        migrated |= successor_compatible(&pair[0], &pair[1], &predecessor.plan, &successor.plan)?;
        retain_volumes(&mut successor, &origin.plan, &predecessor.plan)?;
    }
    let result = PreparedComposeSuccessor {
        // Old runtimes reject this before creating their own custody journal.
        schema_version: if history || migrated { 2 } else { 1 },
        status: "prepared_successor",
        transition_sha256: ContentDigest::of_bytes(bytes),
        predecessor,
        successor,
    };
    if canonical(&result)?.len() + 1 > COMPOSE_MAX_OUTPUT {
        return Err(ComposePreparationError::OutputLimit);
    }
    Ok(result)
}
