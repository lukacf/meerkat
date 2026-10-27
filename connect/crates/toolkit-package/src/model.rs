use crate::{ContentDigest, Id, Revision};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

/// Immutable identity of one package description, not a runtime instance.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIdentity {
    pub publisher: Id,
    pub name: Id,
    pub version: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source: Url,
    pub source_revision: Revision,
    pub license: String,
    #[serde(default)]
    pub notices: Vec<String>,
    #[serde(default)]
    pub local_patches: Vec<ContentDigest>,
}

/// Metadata is inert and may be read before any integration code is imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationPackage {
    pub schema_version: u16,
    pub identity: PackageIdentity,
    pub display_name: String,
    pub description: String,
    pub provenance: Provenance,
    pub strategies: Vec<SetupStrategy>,
    /// Known exports are information for agents, not an activation declaration.
    #[serde(default)]
    pub known_exports: Vec<ExportHint>,
    #[serde(default)]
    pub assessments: Vec<SupportAssessment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupStrategy {
    pub id: Id,
    pub kind: StrategyKind,
    pub instructions: String,
    #[serde(default)]
    pub sources: Vec<Url>,
    #[serde(default)]
    pub known_requirements: Vec<HostServiceRequirement>,
    #[serde(default)]
    pub unresolved_details: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyKind {
    RemoteMcp,
    LocalMcp,
    NativeAdapter,
    ConfigurationRecipe,
    ResearchSkill,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportHint {
    pub id: Id,
    pub role: ExportRole,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportRole {
    ServiceConnector,
    AgentFrontend,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupportAssessment {
    pub status: SupportStatus,
    pub scope: String,
    pub source_revision: Revision,
    pub assessed_at_unix_ms: u64,
    pub evidence: Vec<EvidenceReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportStatus {
    Supported,
    Compatible,
    Experimental,
    Unavailable,
}

/// Evidence is a reference to its owner. Metadata cannot certify that owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub owner: Id,
    pub record: Id,
    pub revision: Revision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractVersion {
    pub major: u16,
    pub minor: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionRequirement {
    pub major: u16,
    pub minimum_minor: u16,
}

impl VersionRequirement {
    pub fn accepts(&self, version: ContractVersion) -> bool {
        version.major == self.major && version.minor >= self.minimum_minor
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractRequirement {
    pub name: Id,
    pub version: VersionRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServiceRequirement {
    pub name: Id,
    pub version: VersionRequirement,
    pub authority: BTreeSet<Id>,
    pub availability: RequirementAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequirementAvailability {
    Required,
    Optional { degraded_behavior: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostServiceOffer {
    pub version: ContractVersion,
    /// Declares supported authority vocabulary, not a grant to the package.
    pub authority: BTreeSet<Id>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostCapabilities {
    pub contracts: BTreeMap<Id, ContractVersion>,
    pub services: BTreeMap<Id, HostServiceOffer>,
    pub runtimes: BTreeSet<RuntimeKind>,
    pub placements: BTreeSet<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    Native,
    Python,
    Node,
    Container,
    Remote,
    HostProvided,
}

/// Exact backend-owned configuration reference. Secret values never belong here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationSelection {
    pub owner: Id,
    pub reference: Id,
    pub revision: Revision,
    pub content_digest: ContentDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicySelection {
    pub owner: Id,
    pub revision: Revision,
    pub content_digest: ContentDigest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Component {
    pub id: Id,
    pub version: Revision,
    pub source: ComponentSource,
    pub runtime: RuntimeKind,
    pub placement: Id,
    pub lifetime: ComponentLifetime,
    pub activation_scope: ActivationScope,
    #[serde(default)]
    pub dependencies: Vec<ComponentDependency>,
    #[serde(default)]
    pub required_contracts: Vec<ContractRequirement>,
    #[serde(default)]
    pub host_services: Vec<HostServiceRequirement>,
    #[serde(default)]
    pub state: Vec<StateContract>,
    pub readiness: ReadinessContract,
    pub shutdown: ShutdownContract,
    pub rollback: RollbackContract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ComponentSource {
    Artifact {
        location: Url,
        content_digest: ContentDigest,
        dependencies: ArtifactDependencies,
        entrypoint: Entrypoint,
    },
    RemoteService {
        endpoint: Url,
        publisher: Id,
        configuration_revision: Revision,
        observed_contract: ContentDigest,
        transport: RemoteTransport,
    },
    HostProvided {
        service: HostServiceRequirement,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactDependencies {
    SelfContained,
    Locked {
        ecosystem: Id,
        lock_digest: ContentDigest,
    },
    PinnedClosure {
        artifacts: BTreeMap<Id, ContentDigest>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entrypoint {
    /// Relative path within the verified artifact; never a shell expression.
    pub executable: String,
    /// Literal argv. The deployment backend must not perform shell expansion.
    pub arguments: Vec<String>,
    /// Explicit primary-wheel console_scripts name. Python selected installer only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_console_script: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTransport {
    McpStreamableHttp,
    HttpApi,
    Websocket,
    Grpc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationScope {
    Independent,
    ToolkitRequired {
        activation_contract: ContractRequirement,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentLifetime {
    Request,
    Deployment,
    ExternallyManaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDependency {
    pub component: Id,
    pub selected_version: Revision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateContract {
    pub namespace: Id,
    pub owner: Id,
    pub format: Revision,
    /// Each accepted prior format has its own owner-bound migration declaration.
    #[serde(default)]
    pub migrations: Vec<StateMigration>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateMigration {
    pub from: Revision,
    pub operation: Id,
    pub artifact_digest: ContentDigest,
    pub reversible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadinessContract {
    pub owner: Id,
    pub checks: BTreeSet<Id>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownContract {
    pub owner: Id,
    pub operation: Id,
    pub maximum_wait_ms: u64,
    pub timeout: ShutdownTimeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownTimeout {
    PreserveUncertainty,
    OwnerControlledFencing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RollbackContract {
    Reversible { owner: Id, operation: Id },
    Irreversible { explanation: String },
    OwnerReconciliation { owner: Id, operation: Id },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case", deny_unknown_fields)]
pub enum Export {
    ServiceConnector(ServiceConnectorExport),
    AgentFrontend(AgentFrontendExport),
}

impl Export {
    pub fn id(&self) -> &Id {
        match self {
            Self::ServiceConnector(export) => &export.id,
            Self::AgentFrontend(export) => &export.id,
        }
    }

    pub fn role(&self) -> ExportRole {
        match self {
            Self::ServiceConnector(_) => ExportRole::ServiceConnector,
            Self::AgentFrontend(_) => ExportRole::AgentFrontend,
        }
    }

    pub fn components(&self) -> &BTreeSet<Id> {
        match self {
            Self::ServiceConnector(export) => &export.components,
            Self::AgentFrontend(export) => &export.components,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConnectorExport {
    pub id: Id,
    pub components: BTreeSet<Id>,
    pub authentication: AuthenticationRequirement,
    pub operations: Vec<OperationDeclaration>,
    #[serde(default)]
    pub events: Vec<EventDeclaration>,
    pub enforcement_contract: ContractRequirement,
    pub diagnostics: Id,
    pub cleanup: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthenticationRequirement {
    None,
    Binding {
        owner: Id,
        requirement: Id,
        credential_audience: Id,
    },
}

/// These are untrusted package declarations. A policy owner must classify the
/// selected operation separately before granting or invoking it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationDeclaration {
    pub id: Id,
    pub schema_digest: ContentDigest,
    pub declared_effect: DeclaredEffect,
    pub required_authority: BTreeSet<Id>,
    pub resources: ResourceContract,
    pub result_limits: ResultLimits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeclaredEffect {
    Read,
    Write,
    Destructive,
    DeviceActuation,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceContract {
    ConnectionWide,
    Selected { selector: Id, enforcement_owner: Id },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultLimits {
    pub maximum_bytes: u64,
    pub maximum_items: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentFrontendExport {
    pub id: Id,
    pub components: BTreeSet<Id>,
    pub modalities: BTreeSet<Modality>,
    pub admission_contract: ContractRequirement,
    pub conversation_contract: ContractRequirement,
    pub delivery_contract: ContractRequirement,
    #[serde(default)]
    pub control_contract: Option<ContractRequirement>,
    /// Voice/live support is a contract requirement, not a live-session receipt.
    #[serde(default)]
    pub live_contract: Option<ContractRequirement>,
    pub authentication: AuthenticationRequirement,
    pub diagnostics: Id,
    pub cleanup: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Text,
    Image,
    Audio,
    RealtimeVoice,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventDeclaration {
    pub id: Id,
    pub payload_schema: ContentDigest,
    pub authenticated_source_contract: ContractRequirement,
    pub consumer_contract: ContractRequirement,
    pub semantics: EventSemantics,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventSemantics {
    pub identity: Id,
    pub ordering: EventOrdering,
    pub maximum_age_ms: u64,
    pub maximum_in_flight: u32,
    pub backpressure: Backpressure,
    pub delivery: EventDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventOrdering {
    Unordered,
    PerKey { key: Id },
    SourceSequence { sequence: Id },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backpressure {
    PauseProducer,
    DurableBuffer,
    DeclaredLoss,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventDelivery {
    Durable {
        producer_handoff: Id,
        consumer_acknowledgment: Id,
        cursor: Id,
        deduplication: Id,
        retention_ms: u64,
        recovery: Id,
    },
    BestEffort {
        loss: LossBehavior,
        recovery: BestEffortRecovery,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LossBehavior {
    DropOldest,
    DropNewest,
    CoalesceLatest { key: Id },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BestEffortRecovery {
    NextEvent,
    Resynchronize { operation: Id },
}

/// Optional generic-client projection. Every entry must work independently of
/// toolkit metadata; toolkit-dependent entrypoints are never included here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableMcpEntry {
    pub name: Id,
    pub component: Id,
    /// Optional declared mapping from service operation IDs to raw MCP names.
    /// This is not tools/list evidence, effect classification or authorization.
    /// Omitting it preserves ordinary software installation and portable export.
    #[serde(default, deserialize_with = "deserialize_operation_tools", skip_serializing_if = "Option::is_none")]
    pub operation_tools: Option<BTreeMap<Id, String>>,
    /// Complete researched secret environment requirements of this local process.
    /// Names only; not credentials, a native account or permission to launch.
    #[serde(default, deserialize_with = "deserialize_environment_secret_slots", skip_serializing_if = "Option::is_none")]
    pub environment_secret_slots: Option<EnvironmentSecretSlots>,
}

fn deserialize_operation_tools<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BTreeMap<Id, String>>, D::Error> {
    struct Names(BTreeMap<Id, String>);
    impl<'de> Deserialize<'de> for Names {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Names;
                fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str("a bounded map of distinct operation IDs to raw MCP names")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(self, mut access: A) -> Result<Names, A::Error> {
                    let mut names = BTreeMap::new();
                    while let Some((id, name)) = access.next_entry::<Id, String>()? {
                        if names.len() >= 128 || names.insert(id, name).is_some() {
                            return Err(serde::de::Error::custom("duplicate or excessive raw operation mapping"));
                        }
                    }
                    Ok(Names(names))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }
    Option::<Names>::deserialize(deserializer).map(|value| value.map(|names| names.0))
}

/// A bounded declaration of distinct environment names. Present null or empty
/// declarations are invalid; omission preserves legacy software-only metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct EnvironmentSecretSlots(Vec<String>);
impl EnvironmentSecretSlots {
    pub fn names(&self) -> &[String] { &self.0 }
}
impl<'de> Deserialize<'de> for EnvironmentSecretSlots {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = EnvironmentSecretSlots;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("1-256 distinct secret environment variable names")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut names = BTreeSet::new();
                while let Some(name) = access.next_element::<String>()? {
                    let mut bytes = name.bytes();
                    if names.len() >= 256 || !(1..=128).contains(&name.len())
                        || !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        || !names.insert(name) {
                        return Err(serde::de::Error::custom("invalid, duplicate or excessive secret environment name"));
                    }
                }
                if names.is_empty() { return Err(serde::de::Error::custom("secret environment names cannot be empty")); }
                Ok(EnvironmentSecretSlots(names.into_iter().collect()))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}
fn deserialize_environment_secret_slots<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<EnvironmentSecretSlots>, D::Error> {
    EnvironmentSecretSlots::deserialize(deserializer).map(Some)
}
