//! A bounded stdin/stdout adapter around the existing inert metadata importers.
//! Explicit file inspection uses the canonical anchored reader. It never
//! expands environment variables or runs package commands.

use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::process::ExitCode;
use toolkit_package::{
    ActivationManifest, ComponentSource, ContentDigest, Export, HostCapabilities, ImportedMcpServers,
    ImportedMcpbManifest, ImportedPluginManifest, IntegrationPackage, McpbUnresolvedContract,
    PluginRoot, PortableMcpServer, ValidatedManifest, ValidationError, canonical_json,
};

const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 256 * 1024;
const MAX_DIAGNOSTIC_PATH: usize = 16;

#[derive(Debug)]
struct InspectionFailure {
    validation: ValidationError,
    diagnostic: Option<Value>,
}

impl From<ValidationError> for InspectionFailure {
    fn from(validation: ValidationError) -> Self {
        Self {
            validation,
            diagnostic: None,
        }
    }
}

// This is an output vocabulary, not another deserializer. Only canonical field
// names at their actual model position may leave the inspector. Unknown keys,
// including valid field names in the wrong object, stop the path projection.
#[derive(Clone, Copy)]
enum PackagePath {
    Package,
    Identity,
    Provenance,
    Strategies,
    Strategy,
    Requirements,
    Requirement,
    Version,
    Availability,
    Exports,
    Export,
    Assessments,
    Assessment,
    Evidence,
    EvidenceItem,
    Values,
    Leaf,
}

impl PackagePath {
    fn field(self, key: &str) -> Option<Self> {
        use PackagePath::*;
        Some(match (self, key) {
            (Package, "identity") => Identity,
            (Package, "provenance") => Provenance,
            (Package, "strategies") => Strategies,
            (Package, "known_exports") => Exports,
            (Package, "assessments") => Assessments,
            (Package, "schema_version" | "display_name" | "description") => Leaf,
            (Identity, "publisher" | "name" | "version") => Leaf,
            (Provenance, "source" | "source_revision" | "license") => Leaf,
            (Provenance, "notices" | "local_patches") => Values,
            (Strategy, "id" | "kind" | "instructions") => Leaf,
            (Strategy, "sources" | "unresolved_details") => Values,
            (Strategy, "known_requirements") => Requirements,
            (Requirement, "name") => Leaf,
            (Requirement, "version") => Version,
            (Requirement, "authority") => Values,
            (Requirement, "availability") => Availability,
            (Version, "major" | "minimum_minor") => Leaf,
            (Availability, "kind" | "degraded_behavior") => Leaf,
            (Export, "id" | "role" | "description") => Leaf,
            (Assessment, "status" | "scope" | "source_revision" | "assessed_at_unix_ms") => Leaf,
            (Assessment, "evidence") => Evidence,
            (EvidenceItem, "owner" | "record" | "revision") => Leaf,
            _ => return None,
        })
    }

    fn index(self) -> Option<Self> {
        use PackagePath::*;
        Some(match self {
            Strategies => Strategy,
            Requirements => Requirement,
            Exports => Export,
            Assessments => Assessment,
            Evidence => EvidenceItem,
            Values => Leaf,
            _ => return None,
        })
    }
}

fn package_field_path(path: &serde_path_to_error::Path) -> Vec<Value> {
    use serde_path_to_error::Segment;
    let mut node = PackagePath::Package;
    let mut safe = Vec::new();
    for segment in path.iter().take(MAX_DIAGNOSTIC_PATH) {
        let (next, value) = match segment {
            Segment::Map { key } => (node.field(key), json!(key)),
            Segment::Seq { index } if *index <= MAX_INPUT => (node.index(), json!(index)),
            _ => break,
        };
        let Some(next) = next else { break };
        node = next;
        safe.push(value);
    }
    safe
}

fn package_parse_failure(
    error: &serde_json::Error,
    path: Vec<Value>,
    bytes: &[u8],
) -> InspectionFailure {
    use serde_json::error::Category;
    let category = match error.classify() {
        Category::Data => Some("data"),
        Category::Syntax => Some("syntax"),
        Category::Eof => Some("eof"),
        Category::Io => None,
    };
    InspectionFailure {
        validation: ValidationError {
            path: "package".into(),
            message: "invalid integration package".into(),
        },
        diagnostic: category.map(|category| {
            json!({
                "version": 1, "category": category, "field_path": path,
                "source_digest": ContentDigest::of_bytes(bytes),
            })
        }),
    }
}

fn parse_integration_package(bytes: &[u8]) -> Result<IntegrationPackage, InspectionFailure> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let package = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        package_parse_failure(error.inner(), package_field_path(error.path()), bytes)
    })?;
    // Match from_slice's single-document contract, including trailing garbage.
    deserializer
        .end()
        .map_err(|error| package_parse_failure(&error, vec![], bytes))?;
    Ok(package)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginFilesInput {
    root: String,
    directory: String,
    files: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginInventoryInput {
    root: String,
    directory: String,
    inventory: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PluginInspectionInput {
    Files(PluginFilesInput),
    Inventory(PluginInventoryInput),
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct McpbArchiveInput {
    root: String,
    directory: String,
    artifact_sha256: ContentDigest,
    files: Vec<String>,
}

#[cfg(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
))]
fn mcpb_archive(bytes: &[u8]) -> Result<Value, ValidationError> {
    use std::collections::BTreeSet;
    use toolkit_package::local_artifacts::{PreparationLimits, inspect_zip_selection};
    let invalid = |message: &str| ValidationError {
        path: "mcpb-archive".into(),
        message: message.into(),
    };
    toolkit_package::strict_mcpb_json(bytes)
        .map_err(|_| invalid("invalid or duplicate-key archive request"))?;
    let input: McpbArchiveInput =
        serde_json::from_slice(bytes).map_err(|_| invalid("invalid archive request"))?;
    let child = input.directory.strip_prefix("./").unwrap_or_default();
    if !std::path::Path::new(&input.root).is_absolute()
        || input.root.len() > 4096
        || child.is_empty()
        || child.len() > 128
        || child == "."
        || child == ".."
        || !child
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        || input.files.len() > 16
    {
        return Err(invalid("invalid root, directory or file selection"));
    }
    let mut selected = BTreeSet::new();
    let mut ordered = vec!["manifest.json".to_string()];
    for path in &input.files {
        let path = path
            .strip_prefix("./")
            .ok_or_else(|| invalid("selected paths must start with ./"))?;
        if !selected.insert(path.to_string()) {
            return Err(invalid("duplicate selected path"));
        }
        if path != "manifest.json" {
            ordered.push(path.to_string());
        }
    }
    selected.insert("manifest.json".to_string());
    if selected.len() > 16 {
        return Err(invalid("too many selected files"));
    }
    // Snapshot only the explicitly selected public artifact through the same
    // anchored reader as ordinary Agent Plugins. No extraction or execution.
    let root = PluginRoot::open(&input.root)?.open_relative(&input.directory)?;
    let limits = PreparationLimits {
        maximum_raw_bytes: 64 * 1024 * 1024,
        ..PreparationLimits::default()
    };
    let raw = root.read("./artifact.mcpb", limits.maximum_raw_bytes)?;
    if ContentDigest::of_bytes(&raw) != input.artifact_sha256 {
        return Err(invalid("archive bytes differ from artifact_sha256"));
    }
    let mut archive = inspect_zip_selection(&raw, &selected, &limits, 32 * 1024, 128 * 1024)
        .map_err(|_| invalid("archive or selection violates the bounded ZIP32 profile"))?;
    let mut files = Vec::new();
    for path in ordered {
        let raw = archive
            .files
            .remove(&path)
            .ok_or_else(|| invalid("selected file is absent"))?;
        let text =
            std::str::from_utf8(&raw).map_err(|_| invalid("selected file is not UTF-8 text"))?;
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid(
                "selected file contains unsupported control characters",
            ));
        }
        let inspection = if path == "manifest.json" {
            toolkit_package::strict_mcpb_json(&raw)
                .map_err(|_| invalid("manifest contains invalid or duplicate-key JSON"))?;
            let imported = ImportedMcpbManifest::parse(&raw)?;
            let manifest = imported.manifest();
            let mut report = inspect("mcpb", &raw).map_err(|error| error.validation)?;
            // Source declarations stay inert. They are not a launch plan, an
            // authentication binding, or evidence of installer compatibility.
            report["summary"]["declarations"] = json!({
                "runtime": manifest.server.runtime,
                "entry_point": manifest.server.entry_point,
                "command": manifest.server.mcp_config.command,
                "args": manifest.server.mcp_config.args,
            });
            report
        } else {
            Value::Null
        };
        files.push(json!({
            "path": format!("./{path}"), "text": text, "byte_length": raw.len(),
            "source_digest": ContentDigest::of_bytes(&raw), "inspection": inspection,
        }));
    }
    let source = canonical_json(&files).map_err(|_| invalid("cannot encode file evidence"))?;
    let inventory_entry_count = archive.inventory.len();
    let inventory_truncated = inventory_entry_count > 128;
    let inventory = archive
        .inventory
        .into_iter()
        .take(128)
        .map(|mut entry| {
            entry.path = format!("./{}", entry.path);
            entry
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "schema_version": 1, "scope": "metadata_only", "format": "mcpb-archive",
        "status": "accepted", "artifact_sha256": input.artifact_sha256,
        "artifact_byte_length": raw.len(), "source_digest": ContentDigest::of_bytes(&source),
        "archive_entry_count": archive.archive_entry_count,
        "inventory_entry_count": inventory_entry_count,
        "inventory": inventory, "inventory_truncated": inventory_truncated, "files": files,
    }))
}

#[cfg(not(all(
    feature = "local-artifacts",
    any(target_os = "linux", target_os = "macos")
)))]
fn mcpb_archive(_bytes: &[u8]) -> Result<Value, ValidationError> {
    Err(ValidationError {
        path: "mcpb-archive".into(),
        message: "archive inspection requires a supported local-artifacts build".into(),
    })
}

fn plugin_directory(root: &str, directory: &str) -> Result<PluginRoot, ValidationError> {
    let child = directory.strip_prefix("./").unwrap_or_default();
    if !std::path::Path::new(root).is_absolute()
        || root.len() > 4096
        || child.is_empty()
        || child.len() > 128
        || child == "."
        || child == ".."
        || !child
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(ValidationError {
            path: "agent-plugin-files".into(),
            message: "invalid root or directory selection".into(),
        });
    }
    PluginRoot::open(root)?.open_relative(directory)
}

fn plugin_files(bytes: &[u8]) -> Result<Value, ValidationError> {
    let invalid = |message: &str| ValidationError {
        path: "agent-plugin-files".into(),
        message: message.into(),
    };
    let input: PluginInspectionInput = serde_json::from_value(
        toolkit_package::strict_mcpb_json(bytes)
            .map_err(|_| invalid("invalid or duplicate-key plugin inspection request"))?,
    )
    .map_err(|_| invalid("invalid plugin inspection request"))?;
    let input = match input {
        PluginInspectionInput::Inventory(input) => {
            let root = plugin_directory(&input.root, &input.directory)?;
            let inventory = root.inventory(&input.inventory)?;
            return Ok(json!({
                "schema_version": 1, "scope": "metadata_only", "format": "agent-plugin-files",
                "status": "inventoried", "directory": input.inventory,
                "inventory": inventory, "complete": true,
            }));
        }
        PluginInspectionInput::Files(input) => input,
    };
    if input.files.is_empty()
        || input.files.len() > 16
        || input.files.iter().filter(|p| *p == "./plugin.json").count() != 1
    {
        return Err(invalid("invalid file selection"));
    }
    let mut names = std::collections::BTreeSet::new();
    for path in &input.files {
        let tail = path.strip_prefix("./").unwrap_or_default();
        if tail.is_empty()
            || path.len() > 1024
            || tail
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
            || !names.insert(path)
        {
            return Err(invalid("invalid or duplicate file path"));
        }
    }
    let root = plugin_directory(&input.root, &input.directory)?;
    let mut files = Vec::new();
    let mut total = 0;
    let mut diagnostics = false;
    for path in &input.files {
        let raw = root.read(path, 32 * 1024)?;
        total += raw.len();
        if total > 128 * 1024 {
            return Err(invalid("selected files exceed the aggregate read limit"));
        }
        let text =
            std::str::from_utf8(&raw).map_err(|_| invalid("selected file is not UTF-8 text"))?;
        if text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid(
                "selected file contains unsupported control characters",
            ));
        }
        let inspection = match path.as_str() {
            "./plugin.json" => inspect("agent-plugin", &raw).map_err(|error| error.validation)?,
            "./mcp.json" => inspect("mcp-servers", &raw).map_err(|error| error.validation)?,
            _ => Value::Null,
        };
        diagnostics |=
            inspection.get("status").and_then(Value::as_str) == Some("accepted_with_diagnostics");
        files.push(json!({
            "path": path, "text": text, "byte_length": raw.len(),
            "source_digest": ContentDigest::of_bytes(&raw), "inspection": inspection,
        }));
    }
    let source = canonical_json(&files).map_err(|_| invalid("cannot encode file evidence"))?;
    Ok(json!({
        "schema_version": 1, "scope": "metadata_only", "format": "agent-plugin-files",
        "status": if diagnostics { "accepted_with_diagnostics" } else { "accepted" },
        "source_digest": ContentDigest::of_bytes(&source), "files": files,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationInput {
    package: IntegrationPackage,
    manifest: ActivationManifest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationUpdateInput {
    previous: ActivationInput,
    next: ActivationInput,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivationHostInput {
    selection: ActivationInput,
    host: HostCapabilities,
}

impl ActivationInput {
    fn validate(self) -> Result<ValidatedManifest, ValidationError> {
        self.manifest.validate(&self.package)
    }
}

fn activation_summary(validated: &ValidatedManifest) -> Value {
    let manifest = validated.manifest();
    json!({
        "package": manifest.package,
        "package_content_digest": manifest.package_content_digest,
        "manifest_digest": validated.digest(),
        "components": manifest.components.iter().map(|component| {
            let source = match &component.source {
                ComponentSource::Artifact { content_digest, .. } => json!({
                    "kind": "artifact", "content_digest": content_digest,
                }),
                ComponentSource::RemoteService { .. } => json!({"kind": "remote_service"}),
                ComponentSource::HostProvided { .. } => json!({"kind": "host_provided"}),
            };
            json!({
                "id": component.id, "version": component.version,
                "source": source, "state": component.state,
            })
        }).collect::<Vec<_>>(),
        "initialization_order": validated.initialization_order(),
        "shutdown_order": validated.shutdown_order().collect::<Vec<_>>(),
        "declared_frontends": manifest.exports.iter().filter_map(|export| {
            let Export::AgentFrontend(frontend) = export else { return None };
            Some(json!({
                "id": frontend.id, "components": frontend.components,
                "modalities": frontend.modalities,
                "admission_contract": frontend.admission_contract,
                "conversation_contract": frontend.conversation_contract,
                "delivery_contract": frontend.delivery_contract,
                "control_contract": frontend.control_contract,
                "live_contract": frontend.live_contract,
            }))
        }).collect::<Vec<_>>(),
    })
}

fn inspect(format: &str, bytes: &[u8]) -> Result<Value, InspectionFailure> {
    if format == "agent-plugin-files" {
        return plugin_files(bytes).map_err(Into::into);
    }
    if format == "mcpb-archive" {
        return mcpb_archive(bytes).map_err(Into::into);
    }
    let (summary, diagnostics) = match format {
        "activation-host" => {
            let input: ActivationHostInput =
                serde_json::from_slice(bytes).map_err(|_| ValidationError {
                    path: "activation-host".into(),
                    message: "invalid activation host input".into(),
                })?;
            let validated = input.selection.validate()?;
            let compatibility = validated.check_host(&input.host);
            let host_bytes = canonical_json(&input.host).map_err(|_| ValidationError {
                path: "host".into(),
                message: "cannot encode host capabilities".into(),
            })?;
            (
                json!({
                    "activation": activation_summary(&validated),
                    "host_digest": ContentDigest::of_bytes(&host_bytes),
                    "compatible": compatibility.is_compatible(),
                    "compatibility": compatibility,
                    "services": validated.host_service_compatibility(&input.host),
                }),
                json!([]),
            )
        }
        "activation" => {
            let input: ActivationInput =
                serde_json::from_slice(bytes).map_err(|_| ValidationError {
                    path: "activation".into(),
                    message: "invalid activation input".into(),
                })?;
            (activation_summary(&input.validate()?), json!([]))
        }
        "activation-update" => {
            let input: ActivationUpdateInput =
                serde_json::from_slice(bytes).map_err(|_| ValidationError {
                    path: "activation-update".into(),
                    message: "invalid activation update input".into(),
                })?;
            let previous = input.previous.validate()?;
            let next = input.next.validate()?;
            let changes = next.state_changes_from(&previous)?;
            (
                json!({
                    "previous": activation_summary(&previous),
                    "next": activation_summary(&next),
                    "state_changes": changes,
                }),
                json!([]),
            )
        }
        "integration-package" => {
            let package = parse_integration_package(bytes)?;
            let package_content_digest = package.content_digest()?;
            (
                json!({
                    "identity": package.identity,
                    "package_content_digest": package_content_digest,
                    "strategies": package.strategies.iter().map(|strategy| json!({
                        "id": strategy.id, "kind": strategy.kind,
                    })).collect::<Vec<_>>(),
                    "exports": package.known_exports.iter().map(|export| json!({
                        "id": export.id, "role": export.role,
                    })).collect::<Vec<_>>(),
                }),
                json!([]),
            )
        }
        "agent-plugin" => {
            let imported = ImportedPluginManifest::parse(bytes)?;
            let package = imported.toolkit_package()?;
            let toolkit = package.map(|package| {
                json!({
                    "identity": package.identity,
                    "strategies": package.strategies.iter().map(|strategy| json!({
                        "id": strategy.id, "kind": strategy.kind,
                    })).collect::<Vec<_>>(),
                    "exports": package.known_exports.iter().map(|export| json!({
                        "id": export.id, "role": export.role,
                    })).collect::<Vec<_>>(),
                })
            });
            (
                json!({
                    "name": imported.manifest.name,
                    "version": imported.manifest.version,
                    "extension_namespaces": imported.manifest.extensions.keys().collect::<Vec<_>>(),
                    "toolkit": toolkit,
                }),
                json!(imported.diagnostics),
            )
        }
        "mcp-servers" => {
            let imported = ImportedMcpServers::parse(bytes)?;
            let servers = imported
                .servers
                .iter()
                .map(|(name, server)| match server {
                    PortableMcpServer::Stdio { args, env, cwd, .. } => json!({
                        "name": name, "transport": "stdio", "argument_count": args.len(),
                        "environment_fields": env.keys().collect::<Vec<_>>(),
                        "working_directory_configured": cwd.is_some(),
                    }),
                    PortableMcpServer::StreamableHttp { headers, .. } => json!({
                        "name": name, "transport": "streamable-http",
                        "header_fields": headers.keys().collect::<Vec<_>>(),
                    }),
                    PortableMcpServer::LegacySse { headers, .. } => json!({
                        "name": name, "transport": "sse",
                        "header_fields": headers.keys().collect::<Vec<_>>(),
                    }),
                })
                .collect::<Vec<_>>();
            (
                json!({"accepted_server_count": servers.len(), "servers": servers}),
                json!(imported.diagnostics),
            )
        }
        "mcpb" => {
            let imported = ImportedMcpbManifest::parse(bytes)?;
            let manifest = imported.manifest();
            let unresolved = imported
                .unresolved_contracts()
                .iter()
                .map(|contract| match contract {
                    McpbUnresolvedContract::BundleArtifactAndDependencyPins => {
                        "bundle_artifact_and_dependency_pins"
                    }
                    McpbUnresolvedContract::OperationSchemasAndClassification => {
                        "operation_schemas_and_classification"
                    }
                    McpbUnresolvedContract::AuthenticationAndAuthority => {
                        "authentication_and_authority"
                    }
                    McpbUnresolvedContract::LifecycleAndReadiness => "lifecycle_and_readiness",
                    McpbUnresolvedContract::SelectedPlatformCompatibility => {
                        "selected_platform_compatibility"
                    }
                    McpbUnresolvedContract::HostOwnedConfigurationAndSecretBindings => {
                        "host_owned_configuration_and_secret_bindings"
                    }
                })
                .collect::<Vec<_>>();
            (
                json!({
                    "name": manifest.name, "version": manifest.version,
                    "runtime": manifest.server.runtime,
                    "user_configuration": manifest.user_config.iter().map(|(name, field)| json!({
                        "name": name, "type": field.value_type,
                        "required": field.required, "sensitive": field.sensitive,
                        "multiple": field.multiple,
                    })).collect::<Vec<_>>(),
                    "unresolved_contracts": unresolved,
                }),
                json!([]),
            )
        }
        _ => unreachable!("caller validates the format"),
    };
    Ok(json!({
        "schema_version": 1, "scope": "metadata_only", "format": format,
        "status": if diagnostics.as_array().is_some_and(|items| items.is_empty()) {
            "accepted"
        } else { "accepted_with_diagnostics" },
        "source_digest": ContentDigest::of_bytes(bytes),
        "summary": summary, "diagnostics": diagnostics,
    }))
}

fn failure(kind: &str) -> Value {
    json!({"schema_version": 1, "scope": "metadata_only", "status": "invalid", "error": {"kind": kind}})
}

fn run(format: &str, input: impl Read) -> (Value, u8) {
    if !matches!(
        format,
        "integration-package"
            | "agent-plugin"
            | "agent-plugin-files"
            | "mcp-servers"
            | "mcpb"
            | "mcpb-archive"
            | "activation"
            | "activation-update"
            | "activation-host"
    ) {
        return (failure("unsupported_format"), 2);
    }
    let mut bytes = Vec::new();
    if input
        .take(MAX_INPUT as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return (failure("input_read_failed"), 2);
    }
    if bytes.len() > MAX_INPUT {
        return (failure("input_too_large"), 2);
    }
    let (result, code) = match inspect(format, &bytes) {
        Ok(value) => (value, 0),
        Err(error) => {
            let mut report = json!({
                "schema_version": 1, "scope": "metadata_only", "status": "invalid",
                "error": {"kind": "invalid_metadata", "path": error.validation.path, "message": error.validation.message},
            });
            if let Some(diagnostic) = error.diagnostic {
                report["error"]["diagnostic"] = diagnostic;
            }
            (report, 2)
        }
    };
    match serde_json::to_vec(&result) {
        // Reserve one byte for the final newline, including error reports.
        Ok(encoded) if encoded.len() < MAX_OUTPUT => (result, code),
        _ => (failure("output_too_large"), 2),
    }
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--help"] {
        println!(
            "Usage: toolkit-package-inspect <integration-package|agent-plugin|agent-plugin-files|mcp-servers|mcpb|mcpb-archive|activation|activation-update|activation-host> < public-metadata.json\nReads at most 1 MiB; emits one JSON report. Explicit agent-plugin-files reads or one-directory inventories and mcpb-archive reads stay below the selected trusted root. MCPB archive inspection requires local-artifacts on Linux or macOS. Metadata inspection only, with no package execution or activation. Host compatibility is a declaration check, not authorization or readiness."
        );
        return ExitCode::SUCCESS;
    }
    let (value, code) = if args.len() == 1 {
        run(&args[0], io::stdin().lock())
    } else {
        (failure("invalid_arguments"), 2)
    };
    let mut output = io::stdout().lock();
    if serde_json::to_writer(&mut output, &value).is_err() || output.write_all(b"\n").is_err() {
        return ExitCode::from(1);
    }
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_tree() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let child = root.path().join("package");
        std::fs::create_dir_all(child.join("skills/example")).unwrap();
        std::fs::write(
            child.join("plugin.json"),
            serde_json::to_vec(&json!({
                "$schema": toolkit_package::AGENT_PLUGIN_SCHEMA, "name": "example",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            child.join("skills/example/SKILL.md"),
            "Read this package.\n",
        )
        .unwrap();
        root
    }

    fn file_report(root: &std::path::Path, files: Value) -> (Value, u8) {
        report(
            "agent-plugin-files",
            json!({"root": root, "directory": "./package", "files": files}),
        )
    }

    #[test]
    fn plugin_files_preserve_bytes_and_root_independent_evidence() {
        let first = plugin_tree();
        let second = plugin_tree();
        let files = json!(["./plugin.json", "./skills/example/SKILL.md"]);
        let (value, code) = file_report(first.path(), files.clone());
        assert_eq!(code, 0);
        assert_eq!(value["files"][1]["text"], "Read this package.\n");
        assert_eq!(value["files"][1]["byte_length"], 19);
        assert_eq!(
            value["files"][0]["inspection"]["summary"]["toolkit"],
            Value::Null
        );
        assert_eq!(value, file_report(second.path(), files).0);
    }

    #[test]
    fn plugin_files_require_manifest_and_safe_unique_paths() {
        let root = plugin_tree();
        for files in [
            json!([]),
            json!(["./skills/example/SKILL.md"]),
            json!(["./plugin.json", "./plugin.json"]),
            json!(["./plugin.json", "./../outside"]),
            json!(["./plugin.json", "./skills//example/SKILL.md"]),
            json!(["./plugin.json", "./absent"]),
        ] {
            let (value, code) = file_report(root.path(), files);
            assert_eq!(code, 2);
            assert!(value.get("files").is_none());
        }
    }

    #[test]
    fn plugin_files_bound_text_and_reject_invalid_namespace() {
        let root = plugin_tree();
        let path = root.path().join("package/skills/example/SKILL.md");
        for contents in [vec![b'x'; 32 * 1024 + 1], vec![0xff], vec![0], vec![27]] {
            std::fs::write(&path, contents).unwrap();
            assert_eq!(
                file_report(
                    root.path(),
                    json!(["./plugin.json", "./skills/example/SKILL.md"])
                )
                .1,
                2
            );
        }
        std::fs::write(
            root.path().join("package/plugin.json"),
            serde_json::to_vec(&json!({
                "$schema": toolkit_package::AGENT_PLUGIN_SCHEMA, "name": "example",
                "extensions": {"ai.rkat.toolkit": {"not": "a package"}},
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(file_report(root.path(), json!(["./plugin.json"])).1, 2);
    }

    #[cfg(unix)]
    #[test]
    fn plugin_files_refuse_escape_at_child_and_file_boundaries() {
        use std::os::unix::fs::symlink;
        let root = plugin_tree();
        let outside = plugin_tree();
        std::fs::rename(root.path().join("package"), root.path().join("saved")).unwrap();
        symlink(outside.path().join("package"), root.path().join("package")).unwrap();
        assert_eq!(file_report(root.path(), json!(["./plugin.json"])).1, 2);
        std::fs::remove_file(root.path().join("package")).unwrap();
        std::fs::rename(root.path().join("saved"), root.path().join("package")).unwrap();
        symlink(
            outside.path().join("package/plugin.json"),
            root.path().join("package/escape"),
        )
        .unwrap();
        assert_eq!(
            file_report(root.path(), json!(["./plugin.json", "./escape"])).1,
            2
        );
        std::fs::create_dir(root.path().join("sibling")).unwrap();
        std::fs::write(root.path().join("sibling/private"), b"private").unwrap();
        symlink("../sibling/private", root.path().join("package/sibling")).unwrap();
        assert_eq!(
            file_report(root.path(), json!(["./plugin.json", "./sibling"])).1,
            2
        );
    }

    fn report(format: &str, input: Value) -> (Value, u8) {
        run(format, serde_json::to_vec(&input).unwrap().as_slice())
    }

    fn integration_package() -> Value {
        serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/python-install/package.json"
        ))
        .unwrap()
    }

    fn activation_input() -> Value {
        let mut manifest: Value = serde_json::from_slice(include_bytes!(
            "../../tests/fixtures/python-install/activation.json"
        ))
        .unwrap();
        // Stateful components are not portable client configuration entries.
        manifest["portable_mcp_entries"] = json!([]);
        manifest["components"][0]["state"] = json!([{
            "namespace": "channels/egress", "owner": "frontend/egress",
            "format": "1", "migrations": [],
        }]);
        json!({"package": integration_package(), "manifest": manifest})
    }

    fn bind_package(input: &mut Value) {
        let package: IntegrationPackage = serde_json::from_value(input["package"].clone()).unwrap();
        input["manifest"]["package"] = json!(package.identity);
        input["manifest"]["package_content_digest"] = json!(package.content_digest().unwrap());
    }

    fn validated(input: &Value) -> ValidatedManifest {
        serde_json::from_value::<ActivationInput>(input.clone())
            .unwrap()
            .validate()
            .unwrap()
    }

    fn host_input() -> Value {
        let mut selection = activation_input();
        selection["manifest"]["components"][0]["host_services"] = json!([{
            "name": "fixture/media", "version": {"major": 1, "minimum_minor": 2},
            "authority": ["store", "fetch"], "availability": {"kind": "required"},
        }]);
        json!({
            "selection": selection,
            "host": {
                "contracts": {"fixture/mcp-client-policy": {"major": 1, "minor": 0}},
                "services": {"fixture/media": {
                    "version": {"major": 1, "minor": 3}, "authority": ["store", "fetch"],
                }},
                "runtimes": ["python"], "placements": ["local"],
            },
        })
    }

    #[test]
    fn activation_inspection_reports_declared_frontends_without_implying_host_support() {
        let mut input = activation_input();
        input["package"]["known_exports"][0] = json!({
            "id": "frontend", "role": "agent_frontend",
            "description": "Inert frontend declaration.",
        });
        input["manifest"]["components"][0]["lifetime"] = json!("deployment");
        let contract = |name: &str| json!({
            "name": name, "version": {"major": 1, "minimum_minor": 0},
        });
        input["manifest"]["exports"] = json!([{
            "role": "agent_frontend", "id": "frontend", "components": ["python-server"],
            "modalities": ["text"], "admission_contract": contract("fixture/admission"),
            "conversation_contract": contract("fixture/conversation"),
            "delivery_contract": contract("fixture/delivery"),
            "control_contract": null, "live_contract": null,
            "authentication": {"kind": "none"}, "diagnostics": "check", "cleanup": "close",
        }]);
        bind_package(&mut input);
        let selected = validated(&input);
        let (first, code) = report("activation", input.clone());
        assert_eq!(code, 0);
        assert_eq!(first["scope"], "metadata_only");
        assert_eq!(first["summary"]["manifest_digest"], json!(selected.digest()));
        let declarations = first["summary"]["declared_frontends"]
            .as_array()
            .expect("inspection must expose the selected frontend declarations");
        assert_eq!(declarations.len(), 1);
        let declared = &declarations[0];
        assert_eq!(declared["id"], "frontend");
        assert_eq!(declared["components"], json!(["python-server"]));
        assert_eq!(declared["modalities"], json!(["text"]));
        for field in [
            "admission_contract", "conversation_contract", "delivery_contract",
            "control_contract", "live_contract",
        ] {
            assert_eq!(declared[field], input["manifest"]["exports"][0][field]);
            assert!(declared.get(field).is_some());
        }

        let mut changed = input.clone();
        changed["manifest"]["exports"][0]["modalities"] = json!(["text", "image"]);
        changed["manifest"]["exports"][0]["control_contract"] = contract("fixture/control");
        let (second, code) = report("activation", changed.clone());
        assert_eq!(code, 0);
        assert_eq!(second["summary"]["declared_frontends"][0]["modalities"],
                   json!(["text", "image"]));
        assert_eq!(second["summary"]["declared_frontends"][0]["control_contract"],
                   contract("fixture/control"));
        assert_ne!(first["summary"]["manifest_digest"], second["summary"]["manifest_digest"]);
        assert_eq!(second["summary"]["manifest_digest"], json!(validated(&changed).digest()));

        let host_input = json!({
            "selection": changed,
            "host": {"contracts": {}, "services": {}, "runtimes": ["python"], "placements": ["local"]},
        });
        let host: HostCapabilities = serde_json::from_value(host_input["host"].clone()).unwrap();
        let selected = validated(&host_input["selection"]);
        let (host_report, code) = report("activation-host", host_input);
        assert_eq!(code, 0);
        assert_eq!(host_report["summary"]["compatible"], false);
        assert_eq!(host_report["summary"]["compatibility"], json!(selected.check_host(&host)));
        assert_eq!(host_report["summary"]["activation"]["declared_frontends"],
                   second["summary"]["declared_frontends"]);
        for summary in [&first["summary"], &second["summary"], &host_report["summary"]["activation"]] {
            for absent in ["ready", "authorized", "configuration", "policy"] {
                assert!(summary.get(absent).is_none());
            }
            assert!(!summary.to_string().contains("fixture_server/__main__.py"));
        }
        let (service, code) = report("activation", activation_input());
        assert_eq!(code, 0);
        assert_eq!(service["summary"]["declared_frontends"], json!([]));
    }

    #[test]
    fn activation_host_preserves_exact_selection_and_canonical_host_bindings() {
        let input = host_input();
        let host: HostCapabilities = serde_json::from_value(input["host"].clone()).unwrap();
        let selected = validated(&input["selection"]);
        let (value, code) = report("activation-host", input.clone());
        assert_eq!(code, 0);
        assert_eq!(value["scope"], "metadata_only");
        assert_eq!(value["status"], "accepted");
        assert_eq!(value["summary"]["compatible"], true);
        assert_eq!(
            value["summary"]["activation"],
            activation_summary(&selected)
        );
        assert_eq!(
            value["summary"]["compatibility"],
            json!(selected.check_host(&host))
        );
        assert_eq!(
            value["summary"]["host_digest"],
            json!(ContentDigest::of_bytes(&canonical_json(&host).unwrap()))
        );
        assert_eq!(
            value["summary"]["services"][0]["scope"],
            "component:python-server"
        );
        assert_eq!(value["summary"]["services"][0]["available"], true);
        assert_eq!(
            value["summary"]["services"][0]["requirement"]["authority"],
            json!(["fetch", "store"])
        );
        assert!(value["summary"].get("ready").is_none());

        let pretty = serde_json::to_vec_pretty(&input).unwrap();
        let (second, code) = run("activation-host", pretty.as_slice());
        assert_eq!(code, 0);
        assert_eq!(value["summary"], second["summary"]);
        assert_ne!(value["source_digest"], second["source_digest"]);
        let mut changed = input;
        changed["host"]["services"]["fixture/media"]["version"]["minor"] = json!(4);
        let (second, code) = report("activation-host", changed);
        assert_eq!(code, 0);
        assert_eq!(
            value["summary"]["activation"],
            second["summary"]["activation"]
        );
        assert_ne!(
            value["summary"]["host_digest"],
            second["summary"]["host_digest"]
        );
    }

    #[test]
    fn activation_host_returns_all_required_gaps_even_when_services_match() {
        let mut input = host_input();
        input["host"]["contracts"] = json!({});
        input["host"]["runtimes"] = json!([]);
        input["host"]["placements"] = json!([]);
        input["selection"]["manifest"]["required_contracts"] = json!([{
            "name": "fixture/manifest", "version": {"major": 1, "minimum_minor": 0},
        }]);
        input["selection"]["manifest"]["components"][0]["required_contracts"] = json!([{
            "name": "fixture/component", "version": {"major": 1, "minimum_minor": 0},
        }]);
        input["selection"]["manifest"]["components"][0]["activation_scope"] = json!({
            "kind": "toolkit_required", "activation_contract": {
                "name": "fixture/activate", "version": {"major": 1, "minimum_minor": 0},
            },
        });
        let (value, code) = report("activation-host", input);
        // Accepted metadata must never be mistaken for permission to import.
        assert_eq!(code, 0);
        assert_eq!(value["status"], "accepted");
        assert_eq!(value["summary"]["compatible"], false);
        assert_eq!(value["summary"]["services"][0]["available"], true);
        let gaps = value["summary"]["compatibility"]["required_gaps"]
            .as_array()
            .unwrap();
        assert_eq!(gaps.len(), 6);
        assert!(gaps.iter().any(|gap| gap["scope"] == "export:service"));
        assert!(gaps.iter().any(|gap| gap["scope"] == "manifest"));
    }

    #[test]
    fn activation_host_compares_versions_authority_and_optional_absence_canonically() {
        for mismatch in ["missing", "major", "minor", "authority"] {
            for optional in [false, true] {
                let mut input = host_input();
                match mismatch {
                    "missing" => input["host"]["services"] = json!({}),
                    "major" => {
                        input["host"]["services"]["fixture/media"]["version"]["major"] = json!(2)
                    }
                    "minor" => {
                        input["host"]["services"]["fixture/media"]["version"]["minor"] = json!(1)
                    }
                    "authority" => {
                        input["host"]["services"]["fixture/media"]["authority"] = json!(["fetch"])
                    }
                    _ => unreachable!(),
                }
                if optional {
                    input["selection"]["manifest"]["components"][0]["host_services"][0]["availability"] = json!({
                        "kind": "optional", "degraded_behavior": "media is disabled",
                    });
                }
                let (value, code) = report("activation-host", input);
                assert_eq!(code, 0);
                assert_eq!(value["summary"]["services"][0]["available"], false);
                assert_eq!(value["summary"]["compatible"], optional);
                let field = if optional {
                    "degraded"
                } else {
                    "required_gaps"
                };
                assert_eq!(
                    value["summary"]["compatibility"][field]
                        .as_array()
                        .unwrap()
                        .len(),
                    1
                );
            }
        }
    }

    #[test]
    fn activation_host_projects_each_scope_without_widening_sibling_requirements() {
        let mut input = host_input();
        let mut extra = input["selection"]["manifest"]["components"][0].clone();
        extra["id"] = json!("sibling");
        extra["state"] = json!([]);
        extra["host_services"][0]["authority"] = json!(["fetch"]);
        extra["source"] = json!({"kind": "host_provided", "service": extra["host_services"][0]});
        extra["runtime"] = json!("host_provided");
        extra["activation_scope"] = json!({
            "kind": "toolkit_required", "activation_contract": {
                "name": "fixture/activate", "version": {"major": 1, "minimum_minor": 0},
            },
        });
        input["host"]["contracts"]["fixture/activate"] = json!({"major": 1, "minor": 0});
        input["host"]["runtimes"] = json!(["python", "host_provided"]);
        input["selection"]["manifest"]["components"][0]["dependencies"] = json!([{
            "component": "sibling", "selected_version": "0.0.0-fixture",
        }]);
        input["selection"]["manifest"]["host_services"] = json!([extra["host_services"][0]]);
        input["selection"]["manifest"]["components"]
            .as_array_mut()
            .unwrap()
            .push(extra);
        let (value, code) = report("activation-host", input);
        assert_eq!(code, 0);
        let services = value["summary"]["services"].as_array().unwrap();
        assert_eq!(services.len(), 4);
        assert_eq!(services[0]["scope"], "manifest");
        assert_eq!(services[1]["scope"], "component:python-server");
        assert_eq!(
            services[1]["requirement"]["authority"],
            json!(["fetch", "store"])
        );
        for service in &services[2..] {
            assert_eq!(service["scope"], "component:sibling");
            assert_eq!(service["requirement"]["authority"], json!(["fetch"]));
        }
    }

    #[test]
    fn activation_host_is_bounded_strict_and_rejects_tampered_selections() {
        for pointer in [
            "",
            "/host",
            "/host/services/fixture~1media",
            "/selection",
            "/selection/manifest",
        ] {
            let mut input = host_input();
            input.pointer_mut(pointer).unwrap()["PRIVATE-FIXTURE"] = json!(true);
            let (value, code) = report("activation-host", input);
            assert_eq!(code, 2);
            assert!(value.get("summary").is_none());
            assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        }
        let mut input = host_input();
        input["selection"]["manifest"]["package_content_digest"] = json!("f".repeat(64));
        assert_eq!(report("activation-host", input).1, 2);
        let (value, code) = run("activation-host", vec![b' '; MAX_INPUT + 1].as_slice());
        assert_eq!(code, 2);
        assert_eq!(value["error"]["kind"], "input_too_large");
    }

    #[test]
    fn activation_projects_canonical_state_source_bindings_and_dependency_order() {
        let mut input = activation_input();
        input["package"]["description"] = json!("PRIVATE-FIXTURE");
        input["package"]["strategies"][0]["instructions"] = json!("PRIVATE-FIXTURE");
        bind_package(&mut input);
        input["manifest"]["configuration"]["reference"] = json!("PRIVATE-FIXTURE");
        input["manifest"]["components"][0]["source"]["location"] =
            json!("https://example.invalid/PRIVATE-FIXTURE.whl");
        input["manifest"]["components"][0]["source"]["entrypoint"]["arguments"] =
            json!(["PRIVATE-FIXTURE"]);
        let mut dependency = input["manifest"]["components"][0].clone();
        dependency["id"] = json!("storage");
        dependency["state"] = json!([]);
        input["manifest"]["components"][0]["dependencies"] = json!([{
            "component": "storage", "selected_version": "0.0.0-fixture",
        }]);
        input["manifest"]["components"]
            .as_array_mut()
            .unwrap()
            .push(dependency);
        let selected = validated(&input);
        let raw = serde_json::to_vec(&input).unwrap();
        let (value, code) = run("activation", raw.as_slice());
        assert_eq!(code, 0);
        assert_eq!(value["scope"], "metadata_only");
        assert_eq!(value["source_digest"], json!(ContentDigest::of_bytes(&raw)));
        assert_eq!(value["summary"]["package"], input["package"]["identity"]);
        assert_eq!(
            value["summary"]["package_content_digest"],
            input["manifest"]["package_content_digest"]
        );
        assert_eq!(
            value["summary"]["manifest_digest"],
            json!(selected.digest())
        );
        assert_eq!(
            value["summary"]["components"][0],
            json!({
                "id": "python-server", "version": "0.0.0-fixture",
                "source": {"kind": "artifact", "content_digest": "1".repeat(64)},
                "state": input["manifest"]["components"][0]["state"],
            })
        );
        assert_eq!(
            value["summary"]["initialization_order"],
            json!(["storage", "python-server"])
        );
        assert_eq!(
            value["summary"]["shutdown_order"],
            json!(["python-server", "storage"])
        );
        assert_eq!(value["diagnostics"], json!([]));
        assert!(value.get("ready").is_none());
        assert!(value["summary"].get("state_changes").is_none());
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    #[test]
    fn activation_remote_and_host_sources_do_not_invent_artifact_pins() {
        let mut remote = activation_input();
        remote["manifest"]["components"][0]["source"] = json!({
            "kind": "remote_service", "endpoint": "https://example.invalid/PRIVATE-FIXTURE",
            "publisher": "fixture", "configuration_revision": "1",
            "observed_contract": "2".repeat(64), "transport": "http_api",
        });
        remote["manifest"]["components"][0]["runtime"] = json!("remote");
        remote["manifest"]["components"][0]["lifetime"] = json!("externally_managed");
        let mut host = activation_input();
        host["manifest"]["components"][0]["source"] = json!({
            "kind": "host_provided", "service": {
                "name": "fixture/host", "version": {"major": 1, "minimum_minor": 0},
                "authority": ["read"], "availability": {"kind": "required"},
            },
        });
        host["manifest"]["components"][0]["runtime"] = json!("host_provided");
        host["manifest"]["components"][0]["activation_scope"] = json!({
            "kind": "toolkit_required", "activation_contract": {
                "name": "fixture/activate", "version": {"major": 1, "minimum_minor": 0},
            },
        });
        for (input, kind) in [(remote, "remote_service"), (host, "host_provided")] {
            let (value, code) = report("activation", input);
            assert_eq!(code, 0);
            assert_eq!(
                value["summary"]["components"][0]["source"],
                json!({"kind": kind})
            );
            assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        }
    }

    #[test]
    fn activation_wrappers_and_nested_documents_reject_unknown_fields_without_echo() {
        for pointer in [
            "",
            "/package",
            "/manifest",
            "/manifest/components/0/state/0",
        ] {
            let mut pair = activation_input();
            pair.pointer_mut(pointer).unwrap()["PRIVATE-FIXTURE"] = json!("PRIVATE-FIXTURE");
            let (value, code) = report("activation", pair.clone());
            assert_eq!(code, 2);
            assert_eq!(value["error"]["path"], "activation");
            assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
            for side in ["previous", "next"] {
                let mut update =
                    json!({"previous": activation_input(), "next": activation_input()});
                update[side] = pair.clone();
                let (value, code) = report("activation-update", update);
                assert_eq!(code, 2);
                assert_eq!(value["error"]["path"], "activation-update");
                assert!(value.get("summary").is_none());
                assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
            }
        }
        let (value, code) = report(
            "activation-update",
            json!({
                "previous": activation_input(), "next": activation_input(), "PRIVATE-FIXTURE": true,
            }),
        );
        assert_eq!(code, 2);
        assert_eq!(value["error"]["path"], "activation-update");
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    #[test]
    fn activation_refuses_wrong_package_pin_and_invalid_selected_closure() {
        for change in [
            "package_pin",
            "package_identity",
            "empty_closure",
            "missing_dependency",
            "duplicate_state",
        ] {
            let mut input = activation_input();
            match change {
                "package_pin" => {
                    input["manifest"]["package_content_digest"] = json!("0".repeat(64))
                }
                "package_identity" => input["manifest"]["package"]["name"] = json!("other"),
                "empty_closure" => {
                    input["manifest"]["components"][0]["source"]["dependencies"]["artifacts"] =
                        json!({})
                }
                "missing_dependency" => {
                    input["manifest"]["components"][0]["dependencies"] = json!([{
                        "component": "absent", "selected_version": "1",
                    }])
                }
                "duplicate_state" => {
                    let state = input["manifest"]["components"][0]["state"][0].clone();
                    input["manifest"]["components"][0]["state"]
                        .as_array_mut()
                        .unwrap()
                        .push(state);
                }
                _ => unreachable!(),
            }
            let (value, code) = report("activation", input.clone());
            assert_eq!(code, 2, "{change}");
            assert_eq!(value["error"]["kind"], "invalid_metadata");
            assert!(value.get("summary").is_none());
            for side in ["previous", "next"] {
                let mut update =
                    json!({"previous": activation_input(), "next": activation_input()});
                update[side] = input.clone();
                let (value, code) = report("activation-update", update);
                assert_eq!(code, 2, "{side}: {change}");
                assert!(value.get("summary").is_none());
            }
        }
    }

    #[test]
    fn activation_update_validates_the_prior_selection_and_package_identity() {
        for side in ["previous", "next"] {
            let mut input = json!({"previous": activation_input(), "next": activation_input()});
            input[side]["manifest"]["strategy"] = json!("absent");
            let (value, code) = report("activation-update", input);
            assert_eq!(code, 2);
            assert_eq!(value["error"]["path"], "strategy");
            assert!(value.get("summary").is_none());
        }
        for field in ["publisher", "name"] {
            let mut next = activation_input();
            next["package"]["identity"][field] = json!("other");
            bind_package(&mut next);
            let (value, code) = report(
                "activation-update",
                json!({"previous": activation_input(), "next": next}),
            );
            assert_eq!(code, 2);
            assert_eq!(value["error"]["message"], "update changes package identity");
            assert!(value.get("summary").is_none());
        }
    }

    #[test]
    fn activation_update_refuses_owner_transfer_and_missing_migration() {
        for field in ["owner", "format"] {
            let mut next = activation_input();
            next["manifest"]["components"][0]["state"][0][field] = json!("changed");
            let (value, code) = report(
                "activation-update",
                json!({"previous": activation_input(), "next": next}),
            );
            assert_eq!(code, 2);
            assert_eq!(value["error"]["path"], "state:channels/egress");
            assert_eq!(
                value["error"]["message"],
                if field == "owner" {
                    "state owner transfer requires a separately authorized migration"
                } else {
                    "no declared migration from the active state format"
                }
            );
            assert!(value.get("summary").is_none());
        }
    }

    #[test]
    fn activation_update_reports_only_the_canonical_migration_obligation() {
        let previous = activation_input();
        let mut next = previous.clone();
        next["package"]["identity"]["version"] = json!("2");
        bind_package(&mut next);
        next["manifest"]["components"][0]["version"] = json!("2");
        next["manifest"]["components"][0]["state"][0]["format"] = json!("2");
        let migration = json!({
            "from": "1", "operation": "migrate-egress", "artifact_digest": "3".repeat(64),
            "reversible": false,
        });
        next["manifest"]["components"][0]["state"][0]["migrations"] = json!([migration]);
        let prior = validated(&previous);
        let selected = validated(&next);
        let (value, code) = report(
            "activation-update",
            json!({"previous": previous, "next": next}),
        );
        assert_eq!(code, 0);
        assert_eq!(
            value["summary"]["previous"]["manifest_digest"],
            json!(prior.digest())
        );
        assert_eq!(
            value["summary"]["next"]["manifest_digest"],
            json!(selected.digest())
        );
        assert_eq!(
            value["summary"]["state_changes"],
            json!(selected.state_changes_from(&prior).unwrap())
        );
        assert_eq!(
            value["summary"]["state_changes"],
            json!([{
                "kind": "migration", "namespace": "channels/egress", "owner": "frontend/egress",
                "from": "1", "to": "2", "migration": migration,
            }])
        );
        assert!(value.get("ready").is_none());
        assert!(value["summary"].get("receipt").is_none());
    }

    #[test]
    fn activation_update_retains_removed_state_and_reports_new_state() {
        let mut next = activation_input();
        next["manifest"]["components"][0]["state"][0]["namespace"] = json!("channels/inbox");
        let (value, code) = report(
            "activation-update",
            json!({"previous": activation_input(), "next": next}),
        );
        assert_eq!(code, 0);
        assert_eq!(
            value["summary"]["state_changes"],
            json!([
                {"kind": "retained_on_removal", "namespace": "channels/egress", "owner": "frontend/egress", "format": "1"},
                {"kind": "initialize", "namespace": "channels/inbox", "owner": "frontend/egress", "format": "1"},
            ])
        );
    }

    #[test]
    fn activation_update_unchanged_format_still_binds_changed_manifest_and_package() {
        let previous = activation_input();
        for field in ["configuration", "policy"] {
            let mut next = previous.clone();
            next["manifest"][field]["content_digest"] = json!("4".repeat(64));
            let (value, code) = report(
                "activation-update",
                json!({"previous": previous, "next": next}),
            );
            assert_eq!(code, 0);
            assert_eq!(value["summary"]["state_changes"], json!([]));
            assert_eq!(
                value["summary"]["previous"]["package_content_digest"],
                value["summary"]["next"]["package_content_digest"]
            );
            assert_ne!(
                value["summary"]["previous"]["manifest_digest"],
                value["summary"]["next"]["manifest_digest"]
            );
        }
        let mut next = previous.clone();
        next["package"]["description"] = json!("changed public description");
        bind_package(&mut next);
        let (value, code) = report(
            "activation-update",
            json!({"previous": previous, "next": next}),
        );
        assert_eq!(code, 0);
        assert_eq!(value["summary"]["state_changes"], json!([]));
        assert_ne!(
            value["summary"]["previous"]["package_content_digest"],
            value["summary"]["next"]["package_content_digest"]
        );
        assert_ne!(
            value["summary"]["previous"]["manifest_digest"],
            value["summary"]["next"]["manifest_digest"]
        );
    }

    #[test]
    fn activation_aggregate_raw_digest_is_distinct_from_canonical_pair_bindings() {
        let input = json!({"previous": activation_input(), "next": activation_input()});
        let compact = serde_json::to_vec(&input).unwrap();
        let pretty = serde_json::to_vec_pretty(&input).unwrap();
        let (first, first_code) = run("activation-update", compact.as_slice());
        let (second, second_code) = run("activation-update", pretty.as_slice());
        assert_eq!((first_code, second_code), (0, 0));
        assert_eq!(first["summary"], second["summary"]);
        assert_ne!(first["source_digest"], second["source_digest"]);
        assert_eq!(
            first["source_digest"],
            json!(ContentDigest::of_bytes(&compact))
        );
        assert_eq!(
            second["source_digest"],
            json!(ContentDigest::of_bytes(&pretty))
        );
    }

    #[test]
    fn activation_modes_keep_input_output_bounds_and_never_emit_partial_summaries() {
        for format in ["activation", "activation-update"] {
            let (value, code) = run(format, vec![b' '; MAX_INPUT + 1].as_slice());
            assert_eq!(code, 2);
            assert_eq!(value["error"]["kind"], "input_too_large");
            let (value, code) = run(format, b"PRIVATE-FIXTURE".as_slice());
            assert_eq!(code, 2);
            assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        }
        let mut input = activation_input();
        input["manifest"]["components"][0]["state"] = json!(
            (0..700)
                .map(|index| json!({
                    "namespace": format!("state-{index:04}-{}", "x".repeat(150)),
                    "owner": "frontend/egress", "format": "y".repeat(200), "migrations": [],
                }))
                .collect::<Vec<_>>()
        );
        for (format, input) in [
            ("activation", input.clone()),
            (
                "activation-update",
                json!({"previous": activation_input(), "next": input}),
            ),
        ] {
            assert!(serde_json::to_vec(&input).unwrap().len() < MAX_INPUT);
            let (value, code) = report(format, input);
            assert_eq!(code, 2);
            assert_eq!(value["error"]["kind"], "output_too_large");
            assert!(value.get("summary").is_none());
        }
    }

    #[test]
    fn integration_package_uses_the_canonical_owner_and_projects_only_metadata() {
        let mut input = integration_package();
        input["description"] = json!("PRIVATE-FIXTURE");
        input["strategies"][0]["instructions"] = json!("PRIVATE-FIXTURE");
        let package: IntegrationPackage = serde_json::from_value(input.clone()).unwrap();
        let (value, code) = report("integration-package", input);
        assert_eq!(code, 0);
        assert_eq!(value["status"], "accepted");
        assert_eq!(value["scope"], "metadata_only");
        assert_eq!(value["summary"]["identity"], json!(package.identity));
        assert_eq!(
            value["summary"]["package_content_digest"],
            json!(package.content_digest().unwrap())
        );
        assert_eq!(
            value["summary"]["strategies"],
            json!([{"id": "python-local", "kind": "local_mcp"}])
        );
        assert_eq!(
            value["summary"]["exports"],
            json!([{"id": "service", "role": "service_connector"}])
        );
        assert_eq!(value["diagnostics"], json!([]));
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    #[test]
    fn integration_package_defaults_have_one_canonical_digest_and_distinct_raw_digests() {
        let mut explicit = integration_package();
        explicit["known_exports"] = json!([]);
        let mut omitted = explicit.clone();
        for key in ["known_exports", "assessments"] {
            omitted.as_object_mut().unwrap().remove(key);
        }
        for key in ["notices", "local_patches"] {
            omitted["provenance"].as_object_mut().unwrap().remove(key);
        }
        for key in ["sources", "known_requirements", "unresolved_details"] {
            // The fixture's source list is nonempty, so make the explicit
            // counterpart empty before comparing it with the default.
            explicit["strategies"][0][key] = json!([]);
            omitted["strategies"][0]
                .as_object_mut()
                .unwrap()
                .remove(key);
        }
        let (first, first_code) = report("integration-package", explicit);
        let (second, second_code) = report("integration-package", omitted);
        assert_eq!((first_code, second_code), (0, 0));
        assert_eq!(first["summary"], second["summary"]);
        assert_ne!(first["source_digest"], second["source_digest"]);
    }

    #[test]
    fn integration_package_key_order_and_whitespace_do_not_change_canonical_digest() {
        let input = integration_package();
        let original = serde_json::to_vec(&input).unwrap();
        // Reverse both the outer object's order and the nested identity's order.
        let identity = &input["identity"];
        let reversed_identity = format!(
            "{{\"version\":{},\"publisher\":{},\"name\":{}}}",
            identity["version"], identity["publisher"], identity["name"]
        );
        let members = input
            .as_object()
            .unwrap()
            .iter()
            .rev()
            .map(|(key, value)| {
                let encoded = if key == "identity" {
                    reversed_identity.clone()
                } else {
                    value.to_string()
                };
                format!("  {} : {encoded}", serde_json::to_string(key).unwrap())
            })
            .collect::<Vec<_>>();
        let reordered = format!("{{\n{}\n}}\n", members.join(",\n"));
        let (first, first_code) = run("integration-package", original.as_slice());
        let (second, second_code) = run("integration-package", reordered.as_bytes());
        assert_eq!((first_code, second_code), (0, 0));
        assert_eq!(first["summary"], second["summary"]);
        assert_ne!(first["source_digest"], second["source_digest"]);
        assert_eq!(
            first["source_digest"],
            json!(ContentDigest::of_bytes(&original))
        );
        assert_eq!(
            second["source_digest"],
            json!(ContentDigest::of_bytes(reordered.as_bytes()))
        );
    }

    #[test]
    fn integration_package_parse_and_semantic_errors_refuse_without_document_echo() {
        let (malformed, code) = run("integration-package", b"PRIVATE-FIXTURE".as_slice());
        assert_eq!(code, 2);
        assert_eq!(
            malformed["error"]["diagnostic"],
            json!({"version": 1, "category": "syntax", "field_path": [],
                "source_digest": ContentDigest::of_bytes(b"PRIVATE-FIXTURE")})
        );
        for nested in [false, true] {
            let mut unknown = integration_package();
            if nested {
                unknown["identity"]["PRIVATE-FIXTURE"] = json!("PRIVATE-FIXTURE");
            } else {
                unknown["PRIVATE-FIXTURE"] = json!("PRIVATE-FIXTURE");
            }
            let (value, code) = report("integration-package", unknown);
            assert_eq!(code, 2);
            assert_eq!(value["error"]["diagnostic"]["category"], "data");
            assert_eq!(
                value["error"]["diagnostic"]["field_path"],
                if nested {
                    json!(["identity"])
                } else {
                    json!([])
                }
            );
            assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        }
        let mut invalid = integration_package();
        invalid["schema_version"] = json!(2);
        invalid["description"] = json!("PRIVATE-FIXTURE");
        let (value, code) = report("integration-package", invalid);
        assert_eq!(code, 2);
        assert_eq!(value["error"]["path"], "schema_version");
        assert!(value.get("summary").is_none());
        assert!(value.get("source_digest").is_none());
        assert!(value["error"].get("diagnostic").is_none());
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    #[test]
    fn package_requirement_prose_has_a_bound_path_and_lossless_authored_repair() {
        let mut input = integration_package();
        let prose = json!([
            "Node runtime and npm dependency closure.",
            "Primary bin entrypoint is dist/index.js; ESM package includes compiled dist files.",
            "Runtime dependencies: @modelcontextprotocol/sdk ^1.30.0, diff ^8.0.3, glob ^13.0.6, minimatch ^10.0.1. Exact acquired closure retained by host.",
            "At least one accessible allowed directory via command-line arguments or MCP roots.",
            "Host must keep any MCP roots restricted to the task directory; roots replace command-line allowed directories.",
            "Whole endpoint exposes read and write operations; read declarations do not filter tools."
        ]);
        input["strategies"][0]["known_requirements"] = prose.clone();
        input["strategies"][0]["unresolved_details"] =
            json!(["PRIVATE pending readiness", "PRIVATE absent license text"]);
        input["provenance"]["notices"] = json!([
            "PRIVATE author attribution",
            "PRIVATE source inspection evidence"
        ]);
        let raw = serde_json::to_vec(&input).unwrap();
        let (value, code) = run("integration-package", raw.as_slice());
        assert_eq!(code, 2);
        assert_eq!(
            value["error"]["diagnostic"],
            json!({
                "version": 1, "category": "data", "field_path": ["strategies", 0, "known_requirements", 0],
                "source_digest": ContentDigest::of_bytes(&raw),
            })
        );
        assert!(!value.to_string().contains("PRIVATE"));
        assert!(!value.to_string().contains("Node runtime"));

        // The agent authors this migration. The inspector never repairs input.
        let mut repaired = input.clone();
        let instructions = repaired["strategies"][0]["instructions"]
            .as_str()
            .unwrap()
            .to_owned();
        repaired["strategies"][0]["instructions"] = json!(format!(
            "{} {}",
            instructions,
            prose
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        repaired["strategies"][0]["known_requirements"] = json!([]);
        let (accepted, code) = report("integration-package", repaired.clone());
        assert_eq!(code, 0);
        assert_eq!(accepted["status"], "accepted");
        assert_eq!(repaired["provenance"], input["provenance"]);
        assert_eq!(
            repaired["strategies"][0]["unresolved_details"],
            input["strategies"][0]["unresolved_details"]
        );
        let canonical: IntegrationPackage = serde_json::from_value(repaired).unwrap();
        assert_eq!(
            accepted["summary"]["package_content_digest"],
            json!(canonical.content_digest().unwrap())
        );
    }

    #[test]
    fn package_typed_requirements_and_nested_failure_paths_use_existing_model() {
        let mut input = integration_package();
        input["strategies"][0]["known_requirements"] = json!([{
            "name": "fixture/service", "version": {"major": 1, "minimum_minor": 0},
            "authority": ["read"], "availability": {"kind": "required"}
        }, {
            "name": "fixture/optional", "version": {"major": 1, "minimum_minor": 0},
            "authority": [], "availability": {"kind": "optional", "degraded_behavior": "Report unavailable."}
        }]);
        assert_eq!(report("integration-package", input.clone()).1, 0);
        input["strategies"][0]["known_requirements"][0]["version"]["major"] = json!("PRIVATE");
        let (value, code) = report("integration-package", input.clone());
        assert_eq!(code, 2);
        assert_eq!(
            value["error"]["diagnostic"]["field_path"],
            json!(["strategies", 0, "known_requirements", 0, "version", "major"])
        );
        assert!(!value.to_string().contains("PRIVATE"));
        input["strategies"][0]["known_requirements"][0]["version"] = json!({"major": 1});
        let (value, code) = report("integration-package", input);
        assert_eq!(code, 2);
        assert_eq!(
            value["error"]["diagnostic"]["field_path"],
            json!(["strategies", 0, "known_requirements", 0, "version"])
        );
    }

    #[test]
    fn package_paths_stop_at_unknown_or_contextually_wrong_keys_and_values() {
        for key in ["PRIVATE\nsecret", "known_requirements", "source"] {
            let mut input = integration_package();
            input["identity"][key] = json!({"description": "PRIVATE"});
            let (value, code) = report("integration-package", input);
            assert_eq!(code, 2);
            assert_eq!(
                value["error"]["diagnostic"]["field_path"],
                json!(["identity"])
            );
            assert!(!value.to_string().contains("PRIVATE"));
        }
        for (pointer, path) in [
            ("/identity/publisher", json!(["identity", "publisher"])),
            ("/provenance/source", json!(["provenance", "source"])),
            ("/strategies/0/kind", json!(["strategies", 0, "kind"])),
        ] {
            let mut input = integration_package();
            *input.pointer_mut(pointer).unwrap() = json!("PRIVATE invalid value");
            let (value, code) = report("integration-package", input);
            assert_eq!(code, 2);
            assert_eq!(value["error"]["diagnostic"]["category"], "data");
            assert_eq!(value["error"]["diagnostic"]["field_path"], path);
            assert!(!value.to_string().contains("PRIVATE"));
        }
    }

    #[test]
    fn package_syntax_trailing_duplicate_depth_and_size_failures_stay_bounded() {
        let valid = serde_json::to_string(&integration_package()).unwrap();
        let duplicate = valid.replacen("{", "{\"description\":\"PRIVATE\",", 1);
        let (value, code) = run("integration-package", duplicate.as_bytes());
        assert_eq!(code, 2);
        assert_eq!(value["error"]["diagnostic"]["category"], "data");
        assert!(!value.to_string().contains("PRIVATE"));
        for (input, category) in [
            (format!("{valid} PRIVATE"), "syntax"),
            ("{\"description\": \"PRIVATE".to_owned(), "eof"),
            (
                format!(
                    "{{\"description\": {}\"PRIVATE\"{}}}",
                    "[".repeat(256),
                    "]".repeat(256)
                ),
                "data",
            ),
        ] {
            let (value, code) = run("integration-package", input.as_bytes());
            assert_eq!(code, 2);
            assert_eq!(value["error"]["diagnostic"]["category"], category);
            assert!(
                value["error"]["diagnostic"]["field_path"]
                    .as_array()
                    .unwrap()
                    .len()
                    <= MAX_DIAGNOSTIC_PATH
            );
            assert!(value.to_string().len() < 1024);
            assert!(!value.to_string().contains("PRIVATE"));
        }
        let (value, code) = run("integration-package", vec![b' '; MAX_INPUT + 1].as_slice());
        assert_eq!(code, 2);
        assert_eq!(value["error"], json!({"kind": "input_too_large"}));
    }

    #[test]
    fn real_retained_plugin_preserves_metadata_role_and_strategy() {
        let bytes =
            include_bytes!("../../integrations/connectors/playwright-mcp-setup/0.1.0/plugin.json");
        let (value, code) = run("agent-plugin", bytes.as_slice());
        assert_eq!(code, 0);
        assert_eq!(
            value["summary"]["toolkit"]["identity"]["name"],
            "playwright-mcp-setup"
        );
        assert_eq!(
            value["summary"]["toolkit"]["exports"][0]["role"],
            "service_connector"
        );
        assert_eq!(
            value["summary"]["toolkit"]["strategies"][0]["kind"],
            "local_mcp"
        );
        assert_eq!(
            value["source_digest"],
            serde_json::to_value(ContentDigest::of_bytes(bytes)).unwrap()
        );
    }

    #[test]
    fn invalid_mcp_sibling_is_visible_and_process_or_auth_values_are_not_projected() {
        let (value, code) = report(
            "mcp-servers",
            json!({
                "$schema": toolkit_package::AGENT_PLUGIN_MCP_SCHEMA,
                "mcpServers": {
                    "local": {"type": "stdio", "command": "node", "args": ["PRIVATE-FIXTURE"], "env": {"TOKEN": "PRIVATE-FIXTURE"}},
                    "remote": {"type": "streamable-http", "url": "https://example.invalid/PRIVATE-FIXTURE", "headers": {"Authorization": "PRIVATE-FIXTURE"}},
                    "invalid": {"type": "stdio", "command": "sh -c", "args": ["PRIVATE-FIXTURE"]},
                },
            }),
        );
        assert_eq!(code, 0);
        assert_eq!(value["status"], "accepted_with_diagnostics");
        assert_eq!(value["summary"]["accepted_server_count"], 2);
        assert_eq!(value["diagnostics"][0]["scope"], "invalid");
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    fn bundle() -> Value {
        json!({
            "manifest_version": "0.3", "name": "fixture", "version": "1", "description": "Fixture", "author": {"name": "Fixture"},
            "server": {"type": "node", "entry_point": "server.js", "mcp_config": {"command": "node", "args": ["PRIVATE-FIXTURE"], "env": {"TOKEN": "PRIVATE-FIXTURE"}}},
            "user_config": {"token": {"type": "string", "title": "Token", "sensitive": true, "required": true}},
        })
    }

    #[test]
    fn mcpb_reports_required_fields_without_executable_values_or_readiness() {
        let (value, code) = report("mcpb", bundle());
        assert_eq!(code, 0);
        assert_eq!(value["summary"]["runtime"], "node");
        assert_eq!(value["summary"]["user_configuration"][0]["sensitive"], true);
        assert!(
            value["summary"]["unresolved_contracts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item == "authentication_and_authority")
        );
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        assert!(value.get("ready").is_none());
        let mut invalid = bundle();
        invalid["user_config"]["token"]["default"] = json!("PRIVATE-FIXTURE");
        let (value, code) = report("mcpb", invalid);
        assert_eq!(code, 2);
        assert_eq!(value["error"]["path"], "user_config.default");
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
    }

    #[cfg(not(all(
        feature = "local-artifacts",
        any(target_os = "linux", target_os = "macos")
    )))]
    #[test]
    fn archive_mode_without_local_artifacts_refuses_explicitly() {
        let (value, code) = run("mcpb-archive", b"{}".as_slice());
        assert_eq!(code, 2);
        assert_eq!(value["error"]["path"], "mcpb-archive");
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("local-artifacts")
        );
    }

    #[test]
    fn malformed_and_oversize_input_refuse_without_echo_or_partial_output() {
        let (value, code) = run("mcpb", b"PRIVATE-FIXTURE".as_slice());
        assert_eq!(code, 2);
        assert!(!value.to_string().contains("PRIVATE-FIXTURE"));
        let (value, code) = run("mcpb", vec![b' '; MAX_INPUT + 1].as_slice());
        assert_eq!(code, 2);
        assert_eq!(value["error"]["kind"], "input_too_large");
        let (value, code) = run("execute", b"{}".as_slice());
        assert_eq!(code, 2);
        assert_eq!(value["error"]["kind"], "unsupported_format");
    }

    #[test]
    fn excessive_metadata_projection_refuses_instead_of_silently_truncating() {
        let (value, code) = report(
            "agent-plugin",
            json!({
                "$schema": toolkit_package::AGENT_PLUGIN_SCHEMA,
                "name": "fixture", "version": "x".repeat(MAX_OUTPUT),
            }),
        );
        assert_eq!(code, 2);
        assert_eq!(value["error"]["kind"], "output_too_large");
    }
}
