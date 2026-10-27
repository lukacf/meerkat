use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// An exact logical selection. It contains references to configuration and
/// policy, not credentials, grants, backend rollout state, or runtime receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationManifest {
    pub schema_version: u16,
    pub package: PackageIdentity,
    pub package_content_digest: ContentDigest,
    pub strategy: Id,
    pub configuration: ConfigurationSelection,
    pub policy: PolicySelection,
    pub components: Vec<Component>,
    pub exports: Vec<Export>,
    #[serde(default)]
    pub required_contracts: Vec<ContractRequirement>,
    #[serde(default)]
    pub host_services: Vec<HostServiceRequirement>,
    #[serde(default)]
    pub portable_mcp_entries: Vec<PortableMcpEntry>,
}

/// Only validation constructs this value. Deserialization cannot bypass checks.
/// Compatibility is still not authorization, installation, or readiness.
#[derive(Debug, Clone)]
pub struct ValidatedManifest {
    manifest: ActivationManifest,
    digest: ContentDigest,
    initialization_order: Vec<Id>,
}

impl ValidatedManifest {
    pub fn manifest(&self) -> &ActivationManifest {
        &self.manifest
    }

    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }

    pub fn initialization_order(&self) -> &[Id] {
        &self.initialization_order
    }

    pub fn shutdown_order(&self) -> impl Iterator<Item = &Id> {
        self.initialization_order.iter().rev()
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        canonical_json(&self.manifest)
    }

    /// Check declarations against host capabilities before importing any code.
    /// A matching authority vocabulary says what the host can enforce, not what
    /// an integration or its caller may do. Grants remain the policy owner's job.
    pub fn check_host(&self, host: &HostCapabilities) -> CompatibilityReport {
        let mut report = CompatibilityReport::default();
        for contract in &self.manifest.required_contracts {
            check_contract(contract, "manifest", host, &mut report);
        }
        for service in &self.manifest.host_services {
            check_service(service, "manifest", host, &mut report);
        }
        for component in &self.manifest.components {
            let scope = format!("component:{}", component.id);
            if !host.runtimes.contains(&component.runtime) {
                report.required_gaps.push(CompatibilityGap {
                    scope: scope.clone(),
                    kind: CompatibilityGapKind::Runtime(component.runtime.clone()),
                });
            }
            if !host.placements.contains(&component.placement) {
                report.required_gaps.push(CompatibilityGap {
                    scope: scope.clone(),
                    kind: CompatibilityGapKind::Placement(component.placement.clone()),
                });
            }
            for contract in &component.required_contracts {
                check_contract(contract, &scope, host, &mut report);
            }
            if let ActivationScope::ToolkitRequired {
                activation_contract,
            } = &component.activation_scope
            {
                check_contract(activation_contract, &scope, host, &mut report);
            }
            for service in &component.host_services {
                check_service(service, &scope, host, &mut report);
            }
            if let ComponentSource::HostProvided { service } = &component.source {
                check_service(service, &scope, host, &mut report);
            }
        }
        for export in &self.manifest.exports {
            let scope = format!("export:{}", export.id());
            match export {
                Export::ServiceConnector(export) => {
                    check_contract(&export.enforcement_contract, &scope, host, &mut report);
                    for event in &export.events {
                        check_contract(
                            &event.authenticated_source_contract,
                            &scope,
                            host,
                            &mut report,
                        );
                        check_contract(&event.consumer_contract, &scope, host, &mut report);
                    }
                }
                Export::AgentFrontend(export) => {
                    check_contract(&export.admission_contract, &scope, host, &mut report);
                    check_contract(&export.conversation_contract, &scope, host, &mut report);
                    check_contract(&export.delivery_contract, &scope, host, &mut report);
                    if let Some(contract) = &export.control_contract {
                        check_contract(contract, &scope, host, &mut report);
                    }
                    if let Some(contract) = &export.live_contract {
                        check_contract(contract, &scope, host, &mut report);
                    }
                }
            }
        }
        report
    }

    fn scoped_host_services(&self) -> Vec<(String, &HostServiceRequirement)> {
        let mut services = self
            .manifest
            .host_services
            .iter()
            .map(|service| ("manifest".into(), service))
            .collect::<Vec<_>>();
        for component in &self.manifest.components {
            let scope = format!("component:{}", component.id);
            services.extend(
                component
                    .host_services
                    .iter()
                    .map(|service| (scope.clone(), service)),
            );
            if let ComponentSource::HostProvided { service } = &component.source {
                services.push((scope, service));
            }
        }
        services
    }

    /// Project each declared service using the same comparison as `check_host`.
    /// Hosts must also honor the full compatibility report and their own policy
    /// before supplying any handles. Availability here is not an access grant.
    pub fn host_service_compatibility(
        &self,
        host: &HostCapabilities,
    ) -> Vec<HostServiceCompatibility> {
        self.scoped_host_services()
            .into_iter()
            .map(|(scope, requirement)| {
                let mut report = CompatibilityReport::default();
                check_service(requirement, &scope, host, &mut report);
                HostServiceCompatibility {
                    scope,
                    requirement: requirement.clone(),
                    available: report.required_gaps.is_empty() && report.degraded.is_empty(),
                }
            })
            .collect()
    }

    /// Compute state migration obligations without executing or authorizing them.
    /// The caller still needs migration receipts before switching selections.
    pub fn state_changes_from(
        &self,
        previous: &ValidatedManifest,
    ) -> Result<Vec<StateChange>, ValidationError> {
        if self.manifest.package.publisher != previous.manifest.package.publisher
            || self.manifest.package.name != previous.manifest.package.name
        {
            return Err(ValidationError::new(
                "package",
                "update changes package identity",
            ));
        }
        let old: BTreeMap<_, _> = previous
            .manifest
            .components
            .iter()
            .flat_map(|component| component.state.iter())
            .map(|state| (state.namespace.clone(), state))
            .collect();
        let new: BTreeMap<_, _> = self
            .manifest
            .components
            .iter()
            .flat_map(|component| component.state.iter())
            .map(|state| (state.namespace.clone(), state))
            .collect();
        let mut changes = Vec::new();
        for (namespace, prior) in &old {
            match new.get(namespace) {
                None => changes.push(StateChange::RetainedOnRemoval {
                    namespace: namespace.clone(),
                    owner: prior.owner.clone(),
                    format: prior.format.clone(),
                }),
                Some(next) if next.owner != prior.owner => {
                    return Err(ValidationError::new(
                        format!("state:{namespace}"),
                        "state owner transfer requires a separately authorized migration",
                    ));
                }
                Some(next) if next.format != prior.format => {
                    let migration = next
                        .migrations
                        .iter()
                        .find(|migration| migration.from == prior.format)
                        .ok_or_else(|| {
                            ValidationError::new(
                                format!("state:{namespace}"),
                                "no declared migration from the active state format",
                            )
                        })?;
                    changes.push(StateChange::Migration {
                        namespace: namespace.clone(),
                        owner: next.owner.clone(),
                        from: prior.format.clone(),
                        to: next.format.clone(),
                        migration: migration.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        for (namespace, next) in &new {
            if !old.contains_key(namespace) {
                changes.push(StateChange::Initialize {
                    namespace: namespace.clone(),
                    owner: next.owner.clone(),
                    format: next.format.clone(),
                });
            }
        }
        Ok(changes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateChange {
    Initialize {
        namespace: Id,
        owner: Id,
        format: Revision,
    },
    Migration {
        namespace: Id,
        owner: Id,
        from: Revision,
        to: Revision,
        migration: StateMigration,
    },
    /// Removing a component never implicitly deletes its durable state.
    RetainedOnRemoval {
        namespace: Id,
        owner: Id,
        format: Revision,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityReport {
    pub required_gaps: Vec<CompatibilityGap>,
    pub degraded: Vec<DegradedCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServiceCompatibility {
    pub scope: String,
    pub requirement: HostServiceRequirement,
    pub available: bool,
}

impl CompatibilityReport {
    pub fn is_compatible(&self) -> bool {
        self.required_gaps.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompatibilityGap {
    pub scope: String,
    pub kind: CompatibilityGapKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum CompatibilityGapKind {
    Runtime(RuntimeKind),
    Placement(Id),
    Contract {
        name: Id,
        required: VersionRequirement,
        offered: Option<ContractVersion>,
    },
    Service {
        name: Id,
        required: VersionRequirement,
        offered: Option<ContractVersion>,
        unsupported_authority: BTreeSet<Id>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DegradedCapability {
    pub gap: CompatibilityGap,
    pub behavior: String,
}

fn check_contract(
    contract: &ContractRequirement,
    scope: &str,
    host: &HostCapabilities,
    report: &mut CompatibilityReport,
) {
    let offered = host.contracts.get(&contract.name).copied();
    if !offered.is_some_and(|version| contract.version.accepts(version)) {
        report.required_gaps.push(CompatibilityGap {
            scope: scope.into(),
            kind: CompatibilityGapKind::Contract {
                name: contract.name.clone(),
                required: contract.version,
                offered,
            },
        });
    }
}

fn check_service(
    service: &HostServiceRequirement,
    scope: &str,
    host: &HostCapabilities,
    report: &mut CompatibilityReport,
) {
    let offered = host.services.get(&service.name);
    let unsupported_authority = offered.map_or_else(
        || service.authority.clone(),
        |offer| {
            service
                .authority
                .difference(&offer.authority)
                .cloned()
                .collect()
        },
    );
    if offered.is_some_and(|offer| service.version.accepts(offer.version))
        && unsupported_authority.is_empty()
    {
        return;
    }
    let gap = CompatibilityGap {
        scope: scope.into(),
        kind: CompatibilityGapKind::Service {
            name: service.name.clone(),
            required: service.version,
            offered: offered.map(|offer| offer.version),
            unsupported_authority,
        },
    };
    match &service.availability {
        RequirementAvailability::Required => report.required_gaps.push(gap),
        RequirementAvailability::Optional { degraded_behavior } => {
            report.degraded.push(DegradedCapability {
                gap,
                behavior: degraded_behavior.clone(),
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid package contract at {path}: {message}")]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl ActivationManifest {
    pub fn validate(
        self,
        package: &IntegrationPackage,
    ) -> Result<ValidatedManifest, ValidationError> {
        let components = self.components.iter().map(ComponentContract::from).collect::<Vec<_>>();
        let initialization_order = validate_selection_contract(self.schema_version, &self.package,
            &self.package_content_digest, &self.strategy, &components, &self.exports,
            &self.required_contracts, &self.host_services, &self.portable_mcp_entries, package)?;
        let digest = ContentDigest::of_bytes(
            &canonical_json(&self)
                .map_err(|_| ValidationError::new("manifest", "canonical serialization failed"))?,
        );
        Ok(ValidatedManifest {
            manifest: self,
            digest,
            initialization_order,
        })
    }
}

pub(crate) struct ComponentContract {
    pub id: Id,
    pub version: Revision,
    pub source: ContractSource,
    pub runtime: RuntimeKind,
    pub lifetime: ComponentLifetime,
    pub activation_scope: ActivationScope,
    pub dependencies: Vec<ComponentDependency>,
    pub required_contracts: Vec<ContractRequirement>,
    pub host_services: Vec<HostServiceRequirement>,
    pub state: Vec<StateContract>,
    pub readiness: ReadinessContract,
    pub shutdown: ShutdownContract,
    pub rollback: RollbackContract,
}
pub(crate) enum ContractSource {
    Artifact { location: url::Url, content_digest: ContentDigest, dependencies: ArtifactDependencies, entrypoint: Entrypoint },
    RemoteService { endpoint: url::Url, transport: RemoteTransport },
    HostProvided { service: HostServiceRequirement },
}
impl From<&Component> for ComponentContract {
    fn from(value: &Component) -> Self {
        Self {
            id: value.id.clone(),
            version: value.version.clone(),
            runtime: value.runtime.clone(),
            lifetime: value.lifetime.clone(),
            activation_scope: value.activation_scope.clone(),
            dependencies: value.dependencies.clone(),
            required_contracts: value.required_contracts.clone(),
            host_services: value.host_services.clone(),
            state: value.state.clone(),
            readiness: value.readiness.clone(),
            shutdown: value.shutdown.clone(),
            rollback: value.rollback.clone(),
            source: match &value.source {
                ComponentSource::Artifact { location, content_digest, dependencies, entrypoint } => ContractSource::Artifact {
                    location: location.clone(), content_digest: content_digest.clone(), dependencies: dependencies.clone(), entrypoint: entrypoint.clone() },
                ComponentSource::RemoteService { endpoint, transport, .. } => ContractSource::RemoteService { endpoint: endpoint.clone(), transport: transport.clone() },
                ComponentSource::HostProvided { service } => ContractSource::HostProvided { service: service.clone() },
            },
        }
    }
}
impl From<&RemoteExportComponent> for ComponentContract {
    fn from(value: &RemoteExportComponent) -> Self {
        Self {
            id: value.id.clone(),
            version: value.version.clone(),
            runtime: value.runtime.clone(),
            lifetime: value.lifetime.clone(),
            activation_scope: value.activation_scope.clone(),
            dependencies: value.dependencies.clone(),
            required_contracts: value.required_contracts.clone(),
            host_services: value.host_services.clone(),
            state: value.state.clone(),
            readiness: value.readiness.clone(),
            shutdown: value.shutdown.clone(),
            rollback: value.rollback.clone(),
            source: match &value.source {
                RemoteExportSource::RemoteService { endpoint, transport, .. } => ContractSource::RemoteService { endpoint: endpoint.clone(), transport: transport.clone() },
            },
        }
    }
}

/// Shared immutable package/strategy checks. This does not select a host.
pub(crate) fn validate_package_selection<'a>(
    schema_version: u16,
    selected_package: &PackageIdentity,
    package_content_digest: &ContentDigest,
    selected_strategy: &Id,
    package: &'a IntegrationPackage,
) -> Result<&'a SetupStrategy, ValidationError> {
    package.validate()?;
    if schema_version != 1 {
        return Err(ValidationError::new(
            "schema_version",
            "unsupported activation schema",
        ));
    }
    if selected_package != &package.identity {
        return Err(ValidationError::new(
            "package",
            "selection does not match package identity",
        ));
    }
    if package_content_digest != &package.content_digest()? {
        return Err(ValidationError::new(
            "package_content_digest",
            "selection does not match immutable package bytes",
        ));
    }
    let strategy = package
        .strategies
        .iter()
        .find(|strategy| &strategy.id == selected_strategy)
        .ok_or_else(|| {
            ValidationError::new(
                "strategy",
                "selected strategy is absent from the pinned package",
            )
        })?;
    Ok(strategy)
}

pub(crate) fn validate_selection_contract(
    schema_version: u16,
    selected_package: &PackageIdentity,
    package_content_digest: &ContentDigest,
    selected_strategy: &Id,
    selected_components: &[ComponentContract],
    selected_exports: &[Export],
    required_contracts: &[ContractRequirement],
    host_services: &[HostServiceRequirement],
    portable_mcp_entries: &[PortableMcpEntry],
    package: &IntegrationPackage,
) -> Result<Vec<Id>, ValidationError> {
    let strategy = validate_package_selection(schema_version, selected_package,
        package_content_digest, selected_strategy, package)?;
    for service in host_services {
        validate_service(service, "host_services")?;
    }
    for requirement in &strategy.known_requirements {
        let declared = host_services
            .iter()
            .chain(
                selected_components
                    .iter()
                    .flat_map(|component| component.host_services.iter()),
            )
            .chain(
                selected_components
                    .iter()
                    .filter_map(|component| match &component.source {
                        ContractSource::HostProvided { service } => Some(service),
                        _ => None,
                    }),
            )
            .any(|selected| selected == requirement);
        if !declared {
            return Err(ValidationError::new(
                "strategy",
                "selected activation omits or changes a known strategy requirement",
            ));
        }
    }
    if selected_exports.is_empty() || selected_components.is_empty() {
        return Err(ValidationError::new(
            "selection",
            "at least one selected export and component are required",
        ));
    }
    let mut components = BTreeMap::new();
    let mut state = BTreeMap::new();
    for component in selected_components {
        let path = format!("component:{}", component.id);
        if components.insert(component.id.clone(), component).is_some() {
            return Err(ValidationError::new(path, "duplicate component identity"));
        }
        validate_component(component)?;
        for contract in &component.state {
            if state.insert(contract.namespace.clone(), contract).is_some() {
                return Err(ValidationError::new(
                    format!("state:{}", contract.namespace),
                    "state namespace has more than one declaration",
                ));
            }
        }
    }
    for component in selected_components {
        let mut dependencies = BTreeSet::new();
        for dependency in &component.dependencies {
            if !dependencies.insert(&dependency.component) {
                return Err(ValidationError::new(
                    format!("component:{}", component.id),
                    "duplicate dependency",
                ));
            }
            let selected = components.get(&dependency.component).ok_or_else(|| {
                ValidationError::new(
                    format!("component:{}", component.id),
                    "dependency is absent from the selected closure",
                )
            })?;
            if selected.version != dependency.selected_version {
                return Err(ValidationError::new(
                    format!("component:{}", component.id),
                    "dependency version differs from the selected version",
                ));
            }
            if component.lifetime != ComponentLifetime::Request
                && selected.lifetime == ComponentLifetime::Request
            {
                return Err(ValidationError::new(
                    format!("component:{}", component.id),
                    "persistent component depends on a request-lifetime component",
                ));
            }
        }
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut initialization_order = Vec::new();
    for id in components.keys() {
        visit(
            id,
            &components,
            &mut visiting,
            &mut visited,
            &mut initialization_order,
        )?;
    }
    let mut exports = BTreeSet::new();
    let mut reached = BTreeSet::new();
    for export in selected_exports {
        if !exports.insert(export.id()) {
            return Err(ValidationError::new(
                format!("export:{}", export.id()),
                "duplicate export identity, including across roles",
            ));
        }
        if export.components().is_empty() {
            return Err(ValidationError::new(
                format!("export:{}", export.id()),
                "export has no realization components",
            ));
        }
        for id in export.components() {
            let component = components.get(id).ok_or_else(|| {
                ValidationError::new(
                    format!("export:{}", export.id()),
                    "export component is absent from selection",
                )
            })?;
            if matches!(export, Export::AgentFrontend(_))
                && component.lifetime == ComponentLifetime::Request
            {
                return Err(ValidationError::new(
                    format!("export:{}", export.id()),
                    "frontend realization must outlive its setup request",
                ));
            }
            collect_closure(id, &components, &mut reached);
        }
        validate_export_role(package, export.id(), export.role())?;
        validate_export(export)?;
    }
    if reached.len() != components.len() {
        return Err(ValidationError::new(
            "components",
            "unrequested component is not a dependency of any selected export",
        ));
    }
    let mut portable_names = BTreeSet::new();
    for portable in portable_mcp_entries {
        if !portable_names.insert(&portable.name) {
            return Err(ValidationError::new(
                "portable_mcp_entries",
                "duplicate portable entry name",
            ));
        }
        let component = components.get(&portable.component).ok_or_else(|| {
            ValidationError::new("portable_mcp_entries", "portable component is absent")
        })?;
        if component.activation_scope != ActivationScope::Independent
            || !required_contracts.is_empty()
            || host_services.iter().any(|service| {
                matches!(service.availability, RequirementAvailability::Required)
            })
            || !component.host_services.is_empty()
            || !component.required_contracts.is_empty()
            || !component.dependencies.is_empty()
            || !component.state.is_empty()
            || matches!(component.source, ContractSource::HostProvided { .. })
        {
            return Err(ValidationError::new(
                "portable_mcp_entries",
                "toolkit-dependent component cannot be exported to clients that ignore toolkit contracts",
            ));
        }
        if portable.operation_tools.is_some() || portable.environment_secret_slots.is_some() {
            let mut exports = selected_exports.iter().filter_map(|export| match export {
                Export::ServiceConnector(service) if service.components.contains(&component.id) => Some(service),
                _ => None,
            });
            let service = exports.next().ok_or_else(|| ValidationError::new(
                "portable_mcp_entries.operation_tools", "raw names require a service export"))?;
            if exports.next().is_some() {
                return Err(ValidationError::new("portable_mcp_entries.operation_tools",
                    "raw names must map every operation of one service export exactly once"));
            }
            if let Some(names) = &portable.operation_tools { validate_operation_names(names, &service.operations)?; }
            validate_environment_secret_scope(portable, &service.authentication,
                matches!(component.source, ContractSource::Artifact { .. }))?;
        }
        if matches!(&component.source, ContractSource::RemoteService { transport, .. } if *transport != RemoteTransport::McpStreamableHttp)
        {
            return Err(ValidationError::new(
                "portable_mcp_entries",
                "remote portable entry must use MCP transport",
            ));
        }
    }
    Ok(initialization_order)
}

pub(crate) fn validate_export_role(
    package: &IntegrationPackage, id: &Id, role: ExportRole,
) -> Result<(), ValidationError> {
    if package.known_exports.iter().any(|hint| hint.id == *id && hint.role != role) {
        return Err(ValidationError::new(format!("export:{id}"),
            "selected role contradicts the pinned export declaration"));
    }
    Ok(())
}

pub(crate) fn validate_operation_names(
    names: &BTreeMap<Id, String>, operations: &[OperationDeclaration],
) -> Result<(), ValidationError> {
    let expected = operations.iter().map(|operation| &operation.id).collect::<BTreeSet<_>>();
    let raw = names.values().collect::<BTreeSet<_>>();
    if names.is_empty() || names.len() > 128
        || names.keys().collect::<BTreeSet<_>>() != expected || raw.len() != names.len()
        || names.values().any(|name| name.is_empty() || name.len() > 128
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
    {
        return Err(ValidationError::new("portable_mcp_entries.operation_tools",
            "raw names must map every operation of one service export exactly once"));
    }
    Ok(())
}

fn visit(
    id: &Id,
    components: &BTreeMap<Id, &ComponentContract>,
    visiting: &mut BTreeSet<Id>,
    visited: &mut BTreeSet<Id>,
    order: &mut Vec<Id>,
) -> Result<(), ValidationError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.clone()) {
        return Err(ValidationError::new(
            format!("component:{id}"),
            "dependency cycle",
        ));
    }
    for dependency in &components[id].dependencies {
        visit(&dependency.component, components, visiting, visited, order)?;
    }
    visiting.remove(id);
    visited.insert(id.clone());
    order.push(id.clone());
    Ok(())
}

fn collect_closure(id: &Id, components: &BTreeMap<Id, &ComponentContract>, reached: &mut BTreeSet<Id>) {
    if reached.insert(id.clone()) {
        for dependency in &components[id].dependencies {
            collect_closure(&dependency.component, components, reached);
        }
    }
}

pub(crate) fn validate_artifact_source(
    location: &url::Url,
    content_digest: &ContentDigest,
    dependencies: &ArtifactDependencies,
    entrypoint: &Entrypoint,
    runtime: &RuntimeKind,
    path: &str,
) -> Result<(), ValidationError> {
    if location.scheme() == "docker-image" {
        if *runtime != RuntimeKind::Container
            || local_container_image(location, content_digest).is_none()
        {
            return Err(ValidationError::new(
                path,
                "local Docker artifact requires an exact container image digest",
            ));
        }
    } else {
        validate_url(location, path, true)?;
    }
    if matches!(
        *runtime,
        RuntimeKind::Remote | RuntimeKind::HostProvided
    ) {
        return Err(ValidationError::new(
            path,
            "artifact source requires an executable runtime",
        ));
    }
    if entrypoint.executable.is_empty()
        || entrypoint.executable.starts_with('/')
        || entrypoint.executable.contains('\\')
        || entrypoint.executable.contains(':')
        || entrypoint.executable.chars().any(char::is_control)
        || entrypoint
            .executable
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(ValidationError::new(
            path,
            "entrypoint must be a safe relative artifact path",
        ));
    }
    if let Some(script) = &entrypoint.python_console_script
        && (*runtime != RuntimeKind::Python
            || !script
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            || !script
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
    {
        return Err(ValidationError::new(
            path,
            "console script requires Python and a safe explicit entry-point name",
        ));
    }
    if entrypoint
        .arguments
        .iter()
        .any(|argument| argument.contains('\0'))
    {
        return Err(ValidationError::new(path, "argv cannot contain NUL"));
    }
    if let ArtifactDependencies::PinnedClosure { artifacts } = dependencies
        && artifacts.is_empty()
    {
        return Err(ValidationError::new(
            path,
            "pinned dependency closure is empty; declare self-contained if applicable",
        ));
    }
    Ok(())
}

fn validate_component(component: &ComponentContract) -> Result<(), ValidationError> {
    let path = format!("component:{}", component.id);
    for service in &component.host_services {
        validate_service(service, &path)?;
    }
    if component.readiness.checks.is_empty() || component.shutdown.maximum_wait_ms == 0 {
        return Err(ValidationError::new(
            &path,
            "readiness checks and bounded shutdown are required",
        ));
    }
    if let RollbackContract::Irreversible { explanation } = &component.rollback {
        nonempty(
            explanation,
            &path,
            "irreversible effects need an explanation",
        )?;
    }
    match &component.source {
        ContractSource::Artifact {
            location,
            content_digest,
            dependencies,
            entrypoint,
        } => {
            validate_artifact_source(location, content_digest, dependencies, entrypoint,
                &component.runtime, &path)?;
        }
        ContractSource::RemoteService { endpoint, .. } => {
            validate_url(endpoint, &path, false)?;
            if component.runtime != RuntimeKind::Remote
                || component.lifetime != ComponentLifetime::ExternallyManaged
            {
                return Err(ValidationError::new(
                    &path,
                    "remote implementation is externally managed, not a local process",
                ));
            }
        }
        ContractSource::HostProvided { service } => {
            validate_service(service, &path)?;
            if component.runtime != RuntimeKind::HostProvided
                || !matches!(service.availability, RequirementAvailability::Required)
            {
                return Err(ValidationError::new(
                    &path,
                    "host-provided component requires its named host service",
                ));
            }
            if component.activation_scope == ActivationScope::Independent {
                return Err(ValidationError::new(
                    &path,
                    "host-provided component requires toolkit activation contracts",
                ));
            }
        }
    }
    for state in &component.state {
        let mut from = BTreeSet::new();
        for migration in &state.migrations {
            if migration.from == state.format || !from.insert(&migration.from) {
                return Err(ValidationError::new(
                    &path,
                    "state migrations must have unique prior formats distinct from the selected format",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_operations(
    operations: &[OperationDeclaration], path: &str,
) -> Result<(), ValidationError> {
    let mut ids = BTreeSet::new();
    for operation in operations {
        if !ids.insert(&operation.id) {
            return Err(ValidationError::new(path, "duplicate operation identity"));
        }
        if operation.result_limits.maximum_bytes == 0
            || operation.result_limits.maximum_items == 0
        {
            return Err(ValidationError::new(
                path,
                "operation results need positive bounds",
            ));
        }
    }
    Ok(())
}

fn validate_export(export: &Export) -> Result<(), ValidationError> {
    let path = format!("export:{}", export.id());
    match export {
        Export::ServiceConnector(export) => {
            if export.operations.is_empty() && export.events.is_empty() {
                return Err(ValidationError::new(
                    &path,
                    "connector must declare operations or events",
                ));
            }
            validate_operations(&export.operations, &path)?;
            let mut events = BTreeSet::new();
            for event in &export.events {
                if !events.insert(&event.id) {
                    return Err(ValidationError::new(&path, "duplicate event identity"));
                }
                let semantics = &event.semantics;
                if semantics.maximum_age_ms == 0 || semantics.maximum_in_flight == 0 {
                    return Err(ValidationError::new(
                        &path,
                        "event freshness and concurrency must be bounded",
                    ));
                }
                if let EventDelivery::Durable { retention_ms, .. } = semantics.delivery
                    && (retention_ms < semantics.maximum_age_ms
                        || semantics.backpressure == Backpressure::DeclaredLoss)
                {
                    return Err(ValidationError::new(
                        &path,
                        "durable stream cannot silently lose events or expire storage before its freshness window",
                    ));
                }
            }
        }
        Export::AgentFrontend(export) => {
            if export.modalities.is_empty() {
                return Err(ValidationError::new(
                    &path,
                    "frontend must declare at least one modality",
                ));
            }
            if export.modalities.contains(&Modality::RealtimeVoice)
                && export.live_contract.is_none()
            {
                return Err(ValidationError::new(
                    &path,
                    "realtime voice requires a live owner contract",
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_service(
    service: &HostServiceRequirement,
    path: &str,
) -> Result<(), ValidationError> {
    if let RequirementAvailability::Optional { degraded_behavior } = &service.availability {
        nonempty(
            degraded_behavior,
            path,
            "optional service must declare degraded behavior",
        )?;
    }
    Ok(())
}

pub(crate) fn nonempty(value: &str, path: &str, message: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        Err(ValidationError::new(path, message))
    } else {
        Ok(())
    }
}

/// An already-present image object in an explicitly bound local Docker engine.
/// This is not a registry reference or a claim about an archive/config digest.
pub(crate) fn local_container_image(
    location: &url::Url,
    expected: &ContentDigest,
) -> Option<String> {
    (location.as_str() == format!("docker-image:///sha256/{}", expected.as_str()))
        .then(|| format!("sha256:{}", expected.as_str()))
}

pub(crate) fn validate_url(
    url: &url::Url,
    path: &str,
    artifact: bool,
) -> Result<(), ValidationError> {
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ValidationError::new(
            path,
            "URL must not embed user information or fragments; credentials require a secret binding",
        ));
    }
    let loopback = match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if url.scheme() == "https"
        || (url.scheme() == "http" && loopback)
        || (artifact && url.scheme() == "file" && url.host().is_none())
    {
        Ok(())
    } else {
        Err(ValidationError::new(
            path,
            "URL must use HTTPS, loopback HTTP, or a local artifact file",
        ))
    }
}

/// Secret names describe local process requirements, not native account identity.
/// Ecosystem-specific installer support remains with the selected installer.
pub(crate) fn validate_environment_secret_scope(portable: &PortableMcpEntry,
    authentication: &AuthenticationRequirement, local_artifact: bool) -> Result<(), ValidationError> {
    if portable.environment_secret_slots.is_some() && (!local_artifact || portable.operation_tools.is_none()
        || !matches!(authentication, AuthenticationRequirement::Binding { .. })) {
        return Err(ValidationError::new("portable_mcp_entries.environment_secret_slots",
            "secret environment names require a local artifact service, binding authentication and explicit raw operations"));
    }
    Ok(())
}
