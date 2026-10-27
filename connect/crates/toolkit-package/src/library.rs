//! Retained integration recipes, portable imports and exact selection checks.
//! Loading and validation never install code, contact providers or grant access.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

const MAX_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Exact validated remote selection metadata, not observed tools or readiness.
/// Authentication=None means documented no sign-in requirement, not proof that
/// a host will ignore its existing stored credentials for this endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeclaredRemoteMcpService {
    #[serde(flatten)]
    selection: RemoteSelectionDigest,
    component: Id,
    export: Id,
    mcp_name: String,
    #[serde(flatten)]
    service: crate::local_install::DeclaredMcpService,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
enum RemoteSelectionDigest {
    Activation { manifest_digest: ContentDigest },
    Declaration { selection_kind: &'static str, declaration_digest: ContentDigest },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibrarySource {
    pub id: Id,
    pub url: Url,
    pub observed_sha256: ContentDigest,
    /// Documentation observations do not pin a hosted implementation.
    pub upstream: UpstreamIdentity,
    pub license: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpstreamIdentity {
    DocumentationObservation,
    SourceRevision {
        commit: Revision,
        release: Option<Revision>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewcomerPrerequisite {
    pub kind: PrerequisiteKind,
    pub instruction: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrerequisiteKind {
    Account,
    ClientAuthentication,
    Administrator,
    DeveloperRegistration,
    HostDeployment,
    ArtifactPreparation,
    ApplicationBinding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecipe {
    pub strategy: Id,
    pub export: Id,
    pub role: ExportRole,
    pub prerequisites: Vec<NewcomerPrerequisite>,
    pub plan: RecipePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RemoteAuthentication {
    #[default]
    Binding,
    /// A retained source documents anonymous access; this is not live proof.
    Unauthenticated { evidence: Id },
}

impl RemoteAuthentication {
    fn is_binding(&self) -> bool {
        matches!(self, Self::Binding)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecipePlan {
    /// Concrete portable endpoint configuration. Its implementation is mutable;
    /// a selection must supply its newly observed contract and matching auth mode.
    RemoteMcp {
        server: String,
        publisher: Id,
        documented_api_version: Option<Revision>,
        /// Omitted for legacy recipes so their serialized content and digest stay stable.
        #[serde(default, skip_serializing_if = "RemoteAuthentication::is_binding")]
        authentication: RemoteAuthentication,
        /// Only set from explicit provider documentation, never user intent.
        read_only: bool,
        read_only_evidence: Option<Id>,
    },
    /// Useful setup knowledge without a complete executable selection.
    GuidanceOnly { blockers: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecipeDocument {
    pub schema_version: u16,
    pub maintainer: Id,
    pub reviewed_at_unix_ms: u64,
    pub sources: Vec<LibrarySource>,
    pub recipes: Vec<LibraryRecipe>,
}

/// The constructor validates all retained documents together. It is deliberately
/// not deserializable: skipped portable entries cannot become supported recipes.
#[derive(Debug, Clone, Serialize)]
pub struct IntegrationBundle {
    plugin: AgentPluginManifest,
    package: IntegrationPackage,
    document: LibraryRecipeDocument,
    servers: BTreeMap<String, PortableMcpServer>,
    digest: ContentDigest,
    /// Set only when a local recipe owner verified this bundle. It is not part
    /// of the bundle content or digest.
    #[serde(skip)]
    local_decision: Option<EvidenceReference>,
}

impl IntegrationBundle {
    pub fn parse(
        plugin_json: &[u8],
        mcp_json: Option<&[u8]>,
        recipes_json: &[u8],
    ) -> Result<Self, ValidationError> {
        if [Some(plugin_json), mcp_json, Some(recipes_json)]
            .into_iter()
            .flatten()
            .any(|bytes| bytes.len() > MAX_DOCUMENT_BYTES)
        {
            return Err(invalid("document exceeds the library read limit"));
        }
        let plugin = ImportedPluginManifest::parse(plugin_json)?;
        if !plugin.diagnostics.is_empty() {
            return Err(invalid("retained plugin contains ignored metadata"));
        }
        let package = plugin
            .toolkit_package()?
            .ok_or_else(|| invalid("retained plugin lacks toolkit package metadata"))?;
        if plugin.manifest.version.as_deref() != Some(package.identity.version.as_str()) {
            return Err(invalid("plugin and package versions differ"));
        }
        let servers = match mcp_json {
            Some(bytes) => {
                let imported = ImportedMcpServers::parse(bytes)?;
                if !imported.diagnostics.is_empty() {
                    return Err(invalid("retained MCP configuration has skipped entries"));
                }
                imported.servers
            }
            None => BTreeMap::new(),
        };
        let document: LibraryRecipeDocument = serde_json::from_slice(recipes_json)
            .map_err(|_| ValidationError::new("recipes.json", "invalid recipe document"))?;
        validate_document(&package, &document, &servers)?;
        let digest = ContentDigest::of_bytes(
            &canonical_json(&(&plugin.manifest, &document, &servers))
                .map_err(|_| invalid("cannot encode retained recipe"))?,
        );
        Ok(Self {
            plugin: plugin.manifest,
            package,
            document,
            servers,
            digest,
            local_decision: None,
        })
    }

    /// The agent decision of a locally saved recipe, if this bundle came from
    /// one. Such a bundle can only enter a local-investigation library.
    pub fn local_decision(&self) -> Option<&EvidenceReference> {
        self.local_decision.as_ref()
    }

    #[cfg_attr(not(feature = "local-recipes"), allow(dead_code))]
    pub(crate) fn with_local_decision(mut self, decision: EvidenceReference) -> Self {
        self.local_decision = Some(decision);
        self
    }

    pub fn package(&self) -> &IntegrationPackage {
        &self.package
    }

    pub fn recipes(&self) -> &LibraryRecipeDocument {
        &self.document
    }

    pub fn servers(&self) -> &BTreeMap<String, PortableMcpServer> {
        &self.servers
    }

    /// Covers plugin metadata, source observations, prerequisites and MCP values
    /// after canonical parsing. It is distinct from the catalog package digest.
    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }

    /// Export exactly one selected recipe's independent MCP endpoint. Parsing
    /// a bundle alone does not prove independence from Toolkit requirements.
    /// This uses the same recipe validation as `validate_selection`, without
    /// claiming host compatibility, authentication, grants or readiness.
    ///
    /// Generic clients see the whole endpoint. Toolkit operation declarations,
    /// resource restrictions and result limits do not become a portable proxy.
    pub fn export_portable(
        &self,
        selected_bundle: &ContentDigest,
        selection: ActivationManifest,
    ) -> Result<PortablePluginExport, LibraryPortableExportError> {
        self.export_portable_with_selection(selected_bundle, selection)
            .map(|(export, _)| export)
    }

    /// Export and project the same validated activation. Legacy selections
    /// without an explicit raw-operation map remain exportable, but have no
    /// service declaration for a host to review. No raw name is inferred.
    pub fn export_portable_with_selection(
        &self,
        selected_bundle: &ContentDigest,
        selection: ActivationManifest,
    ) -> Result<(PortablePluginExport, Option<DeclaredRemoteMcpService>), LibraryPortableExportError> {
        let validated = self.validate_selection_contract(selected_bundle, selection)?;
        let recipe = self
            .document
            .recipes
            .iter()
            .find(|recipe| recipe.strategy == validated.manifest().strategy)
            .ok_or_else(|| invalid("selected recipe is absent"))?;
        let RecipePlan::RemoteMcp { server, .. } = &recipe.plan else {
            return Err(LibrarySelectionError::GuidanceOnly.into());
        };
        let endpoint = self
            .servers
            .get(server)
            .ok_or_else(|| invalid("selected portable endpoint is absent"))?;
        // Do not project arbitrary names from the selection or include the
        // other recipes. The export owner checks this exact name against all
        // declared portable entries, including missing and additional names.
        let servers = BTreeMap::from([(server.clone(), endpoint.clone())]);
        let export = PortablePluginExport::new(&self.plugin, &servers, Some(&validated))?;
        let selected = validated.manifest();
        let portable = selected.portable_mcp_entries.iter()
            .find(|entry| entry.name.as_str() == server.as_str())
            .ok_or_else(|| invalid("selected portable entry is absent"))?;
        let Export::ServiceConnector(service) = &selected.exports[0] else {
            return Err(invalid("selected remote service is absent").into());
        };
        let declaration = crate::local_install::DeclaredMcpService::from_export(service, portable)
            .map(|service| DeclaredRemoteMcpService {
                selection: RemoteSelectionDigest::Activation { manifest_digest: validated.digest().clone() },
                component: portable.component.clone(),
                export: recipe.export.clone(),
                mcp_name: server.clone(),
                service,
            });
        Ok((export, declaration))
    }

    /// Validate a concrete agent-selected manifest through the existing owner.
    /// A successful result remains a declaration, never an activation receipt.
    pub fn validate_selection(
        &self,
        selected_bundle: &ContentDigest,
        selection: ActivationManifest,
        host: &HostCapabilities,
    ) -> Result<ValidatedManifest, LibrarySelectionError> {
        let validated = self.validate_selection_contract(selected_bundle, selection)?;
        let report = validated.check_host(host);
        if !report.is_compatible() {
            return Err(LibrarySelectionError::HostIncompatible(report));
        }
        Ok(validated)
    }

    fn validate_selection_contract(
        &self,
        selected_bundle: &ContentDigest,
        selection: ActivationManifest,
    ) -> Result<ValidatedManifest, LibrarySelectionError> {
        // Preserve legacy typed-refusal order before inspecting selected fields.
        let (recipe, retained_endpoint) = self.retained_remote_recipe(selected_bundle, &selection.strategy)?;
        if selection.components.len() != 1 || selection.exports.len() != 1 {
            return Err(invalid("remote recipe selects one component and one service export").into());
        }
        let ComponentSource::RemoteService { endpoint, publisher, configuration_revision, transport, .. } = &selection.components[0].source else {
            return Err(invalid("selection changes the retained endpoint, publisher or configuration").into());
        };
        if configuration_revision != &selection.configuration.revision {
            return Err(invalid("selection changes the retained endpoint, publisher or configuration").into());
        }
        self.validate_remote_recipe_fields(recipe, retained_endpoint, endpoint, publisher, transport, &selection.exports)?;
        Ok(selection.validate(&self.package)?)
    }

    /// Export complete independent remote declarations without host snapshots.
    /// This does not observe schemas, contact the endpoint or authorize a call.
    pub fn export_portable_declaration(
        &self,
        selected_bundle: &ContentDigest,
        declaration: RemoteExportDeclaration,
    ) -> Result<(PortablePluginExport, Option<DeclaredRemoteMcpService>), LibraryPortableExportError> {
        if declaration.components.len() != 1 {
            return Err(invalid("remote recipe selects one component and one service export").into());
        }
        let RemoteExportSource::RemoteService { endpoint, publisher, transport, .. } = &declaration.components[0].source;
        let recipe = self.validate_remote_recipe(selected_bundle, &declaration.strategy,
            endpoint, publisher, transport, &declaration.exports)?;
        let RecipePlan::RemoteMcp { server, .. } = &recipe.plan else {
            return Err(LibrarySelectionError::GuidanceOnly.into());
        };
        let validated = declaration.validate(&self.package)?;
        let endpoint = self.servers.get(server).ok_or_else(|| invalid("selected portable endpoint is absent"))?;
        let servers = BTreeMap::from([(server.clone(), endpoint.clone())]);
        let export = PortablePluginExport::from_remote_declaration(&self.plugin, &servers, &validated)?;
        let selected = validated.declaration();
        let portable = selected.portable_mcp_entries.iter()
            .find(|entry| entry.name.as_str() == server.as_str())
            .ok_or_else(|| invalid("selected portable entry is absent"))?;
        let Export::ServiceConnector(service) = &selected.exports[0] else {
            return Err(invalid("selected remote service is absent").into());
        };
        let declaration = crate::local_install::DeclaredMcpService::from_export(service, portable)
            .map(|service| DeclaredRemoteMcpService {
                selection: RemoteSelectionDigest::Declaration {
                    selection_kind: REMOTE_EXPORT_DECLARATION, declaration_digest: validated.digest().clone() },
                component: portable.component.clone(), export: recipe.export.clone(),
                mcp_name: server.clone(), service,
            });
        Ok((export, declaration))
    }

    fn retained_remote_recipe(
        &self,
        selected_bundle: &ContentDigest,
        strategy: &Id,
    ) -> Result<(&LibraryRecipe, &Url), LibrarySelectionError> {
        if selected_bundle != &self.digest {
            return Err(LibrarySelectionError::StaleBundle);
        }
        let recipe = self.document.recipes.iter().find(|recipe| &recipe.strategy == strategy)
            .ok_or_else(|| invalid("selected recipe is absent"))?;
        let RecipePlan::RemoteMcp { server, .. } = &recipe.plan else {
            return Err(LibrarySelectionError::GuidanceOnly);
        };
        let Some(PortableMcpServer::StreamableHttp { url, .. }) = self.servers.get(server) else {
            return Err(invalid("selected portable endpoint is absent").into());
        };
        Ok((recipe, url))
    }

    fn validate_remote_recipe(
        &self,
        selected_bundle: &ContentDigest,
        strategy: &Id,
        endpoint: &Url,
        publisher: &Id,
        transport: &RemoteTransport,
        exports: &[Export],
    ) -> Result<&LibraryRecipe, LibrarySelectionError> {
        let (recipe, retained_endpoint) = self.retained_remote_recipe(selected_bundle, strategy)?;
        self.validate_remote_recipe_fields(recipe, retained_endpoint, endpoint, publisher, transport, exports)?;
        Ok(recipe)
    }

    fn validate_remote_recipe_fields(
        &self,
        recipe: &LibraryRecipe,
        retained_endpoint: &Url,
        endpoint: &Url,
        publisher: &Id,
        transport: &RemoteTransport,
        exports: &[Export],
    ) -> Result<(), LibrarySelectionError> {
        let RecipePlan::RemoteMcp { publisher: retained_publisher, read_only, authentication, .. } = &recipe.plan else {
            return Err(LibrarySelectionError::GuidanceOnly);
        };
        if endpoint != retained_endpoint || publisher != retained_publisher || *transport != RemoteTransport::McpStreamableHttp {
            return Err(invalid("selection changes the retained endpoint, publisher or configuration").into());
        }
        if exports.len() != 1 {
            return Err(invalid("remote recipe selects one component and one service export").into());
        }
        let Export::ServiceConnector(export) = &exports[0] else {
            return Err(invalid("MCP service recipes cannot expose an agent frontend").into());
        };
        if export.id != recipe.export
            || !matches!((authentication, &export.authentication),
                (RemoteAuthentication::Binding, AuthenticationRequirement::Binding { .. }) |
                (RemoteAuthentication::Unauthenticated { .. }, AuthenticationRequirement::None))
            || !export.events.is_empty()
            || (*read_only && export.operations.iter().any(|op| op.declared_effect != DeclaredEffect::Read))
        {
            return Err(invalid("selection changes the recipe export, auth or documented operation scope").into());
        }
        Ok(())
    }

}

#[derive(Debug, thiserror::Error)]
pub enum LibraryPortableExportError {
    #[error(transparent)]
    Selection(#[from] LibrarySelectionError),
    #[error(transparent)]
    Export(#[from] PortableExportError),
    #[error(transparent)]
    Invalid(#[from] ValidationError),
}

#[derive(Debug, thiserror::Error)]
pub enum LibrarySelectionError {
    #[error("selected library bundle digest is stale")]
    StaleBundle,
    #[error("guidance recipe has no executable selection; resolve its listed blockers first")]
    GuidanceOnly,
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("host lacks required runtime, placement, contract or service capabilities")]
    HostIncompatible(CompatibilityReport),
}

fn invalid(message: &str) -> ValidationError {
    ValidationError::new("integration_library", message)
}

fn validate_document(
    package: &IntegrationPackage,
    document: &LibraryRecipeDocument,
    servers: &BTreeMap<String, PortableMcpServer>,
) -> Result<(), ValidationError> {
    if document.schema_version != 1 || document.reviewed_at_unix_ms == 0 {
        return Err(invalid("unsupported recipe schema or missing review date"));
    }
    let mut sources = BTreeSet::new();
    for source in &document.sources {
        crate::manifest::validate_url(&source.url, "library source", true)?;
        if let UpstreamIdentity::SourceRevision { commit, .. } = &source.upstream
            && (commit.as_str().len() != 40
                || !commit.as_str().bytes().all(|b| b.is_ascii_hexdigit())
                || !source
                    .url
                    .path()
                    .split('/')
                    .any(|part| part == commit.as_str()))
        {
            return Err(invalid(
                "pinned source must use its exact commit in the source URL",
            ));
        }
        if source.license.trim().is_empty() || !sources.insert(source.id.clone()) {
            return Err(invalid("source lacks license scope or repeats an identity"));
        }
    }
    if !document.sources.iter().any(|source| {
        source.url == package.provenance.source
            && package.provenance.source_revision.as_str()
                == format!("sha256:{}", source.observed_sha256)
    }) {
        return Err(invalid(
            "package provenance is not an exact source observation",
        ));
    }
    for strategy in &package.strategies {
        if strategy.sources.is_empty()
            || strategy
                .sources
                .iter()
                .any(|url| !document.sources.iter().any(|s| &s.url == url))
        {
            return Err(invalid("strategy source lacks a retained observation"));
        }
    }
    let mut strategies = BTreeSet::new();
    let mut used_servers = BTreeSet::new();
    for recipe in &document.recipes {
        let strategy = package
            .strategies
            .iter()
            .find(|s| s.id == recipe.strategy)
            .ok_or_else(|| invalid("recipe names an absent strategy"))?;
        if !strategies.insert(&recipe.strategy)
            || !package
                .known_exports
                .iter()
                .any(|e| e.id == recipe.export && e.role == recipe.role)
            || recipe.prerequisites.is_empty()
            || recipe
                .prerequisites
                .iter()
                .any(|p| p.instruction.trim().is_empty())
        {
            return Err(invalid(
                "recipe repeats a strategy or lacks its role, export or prerequisites",
            ));
        }
        match &recipe.plan {
            RecipePlan::RemoteMcp {
                server,
                read_only,
                read_only_evidence,
                authentication,
                ..
            } => {
                if let RemoteAuthentication::Unauthenticated { evidence } = authentication
                    && (!document.sources.iter().any(|source| {
                        &source.id == evidence && strategy.sources.contains(&source.url)
                    }) || recipe.prerequisites.iter().any(|prerequisite| {
                        prerequisite.kind == PrerequisiteKind::ClientAuthentication
                    }))
                {
                    return Err(invalid(
                        "unauthenticated access requires a strategy source and no client-authentication prerequisite",
                    ));
                }
                if *read_only != read_only_evidence.is_some()
                    || read_only_evidence
                        .as_ref()
                        .is_some_and(|id| !sources.contains(id))
                {
                    return Err(invalid(
                        "read-only claim lacks its exact source observation",
                    ));
                }
                if recipe.role != ExportRole::ServiceConnector
                    || strategy.kind != StrategyKind::RemoteMcp
                {
                    return Err(invalid(
                        "portable MCP configuration is only a service connector",
                    ));
                }
                let Some(PortableMcpServer::StreamableHttp { url, headers }) = servers.get(server)
                else {
                    return Err(invalid(
                        "remote recipe requires a retained Streamable HTTP entry",
                    ));
                };
                if !headers.is_empty() || url.query().is_some() || url.scheme() != "https" {
                    return Err(invalid(
                        "library endpoint must use HTTPS without inline headers or query credentials",
                    ));
                }
                used_servers.insert(server);
            }
            RecipePlan::GuidanceOnly { blockers } => {
                if blockers.is_empty() || blockers.iter().any(|b| b.trim().is_empty()) {
                    return Err(invalid(
                        "guidance-only recipe must name unresolved blockers",
                    ));
                }
            }
        }
    }
    if strategies.len() != package.strategies.len() || used_servers.len() != servers.len() {
        return Err(invalid(
            "unrepresented strategy or unselected portable server",
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum IntegrationLibraryError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("immutable integration bundle identity has different content")]
    ImmutableConflict,
    #[error("a locally investigated bundle cannot enter a curated or imported library")]
    LocalPromotion,
    #[error("immutable integration bundle identity has different provenance")]
    ProvenanceConflict,
}

/// Exact inventory for agent inspection. It contains no search heuristic or
/// automatic choice, and is not a readiness or authorization registry.
#[derive(Debug)]
pub struct IntegrationLibrary {
    origin: CatalogOrigin,
    catalog: PackageCatalog,
    bundles: BTreeMap<PackageIdentity, IntegrationBundle>,
}

impl IntegrationLibrary {
    pub fn new(origin: CatalogOrigin) -> Self {
        Self {
            origin,
            catalog: PackageCatalog::default(),
            bundles: BTreeMap::new(),
        }
    }

    /// A locally saved bundle keeps its own decision as its catalog origin and
    /// is refused by a curated or imported library rather than relabeled.
    pub fn insert(&mut self, bundle: IntegrationBundle) -> Result<(), IntegrationLibraryError> {
        let origin = match (&bundle.local_decision, &self.origin) {
            (None, origin) => origin.clone(),
            (Some(decision), CatalogOrigin::LocalInvestigation { .. }) => {
                CatalogOrigin::LocalInvestigation {
                    decision: decision.clone(),
                }
            }
            (Some(_), _) => return Err(IntegrationLibraryError::LocalPromotion),
        };
        let identity = bundle.package.identity.clone();
        if let Some(existing) = self.bundles.get(&identity) {
            if existing.digest != bundle.digest {
                return Err(IntegrationLibraryError::ImmutableConflict);
            }
            if existing.local_decision != bundle.local_decision
                || self.catalog.get(&identity).map(|entry| &entry.origin) != Some(&origin)
            {
                return Err(IntegrationLibraryError::ProvenanceConflict);
            }
            return Ok(());
        }
        self.catalog.insert(bundle.package.clone(), origin)?;
        self.bundles.insert(identity, bundle);
        Ok(())
    }

    pub fn catalog(&self) -> &PackageCatalog {
        &self.catalog
    }

    pub fn entries(&self) -> impl Iterator<Item = &IntegrationBundle> {
        self.bundles.values()
    }

    pub fn get(
        &self,
        identity: &PackageIdentity,
        digest: &ContentDigest,
    ) -> Option<&IntegrationBundle> {
        self.bundles
            .get(identity)
            .filter(|bundle| &bundle.digest == digest)
    }
}

/// Load versioned retained files through the public parser. Remote services have
/// no pinned implementation claim. Guidance-only recipes cannot be selected
/// through this library validation path.
pub fn bundled_integration_library() -> Result<IntegrationLibrary, IntegrationLibraryError> {
    let mut library = IntegrationLibrary::new(CatalogOrigin::Curated {
        catalog: Id::new("meerkat-toolkit/integrations")
            .map_err(|_| invalid("invalid bundled origin"))?,
        revision: Revision::new("2026-10-01.3").map_err(|_| invalid("invalid bundled revision"))?,
    });
    for (plugin, mcp, recipes) in bundled_documents() {
        library.insert(IntegrationBundle::parse(plugin, mcp, recipes)?)?;
    }
    Ok(library)
}

type BundleDocuments = (&'static [u8], Option<&'static [u8]>, &'static [u8]);

fn bundled_documents() -> Vec<BundleDocuments> {
    // Filled with literal include paths so packaging preserves every input.
    vec![
        (
            include_bytes!("../integrations/connectors/github-readonly/0.1.0/plugin.json"),
            Some(include_bytes!(
                "../integrations/connectors/github-readonly/0.1.0/mcp.json"
            )),
            include_bytes!("../integrations/connectors/github-readonly/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/linear-readonly/0.1.0/plugin.json"),
            Some(include_bytes!(
                "../integrations/connectors/linear-readonly/0.1.0/mcp.json"
            )),
            include_bytes!("../integrations/connectors/linear-readonly/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/notion-mcp/0.1.0/plugin.json"),
            Some(include_bytes!(
                "../integrations/connectors/notion-mcp/0.1.0/mcp.json"
            )),
            include_bytes!("../integrations/connectors/notion-mcp/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/atlassian-mcp/0.1.0/plugin.json"),
            Some(include_bytes!(
                "../integrations/connectors/atlassian-mcp/0.1.0/mcp.json"
            )),
            include_bytes!("../integrations/connectors/atlassian-mcp/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/slack-mcp-setup/0.1.0/plugin.json"),
            None,
            include_bytes!("../integrations/connectors/slack-mcp-setup/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/slack-mcp-setup/0.2.0/plugin.json"),
            None,
            include_bytes!("../integrations/connectors/slack-mcp-setup/0.2.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/playwright-mcp-setup/0.1.0/plugin.json"),
            None,
            include_bytes!("../integrations/connectors/playwright-mcp-setup/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/home-assistant-mcp-setup/0.1.0/plugin.json"),
            None,
            include_bytes!(
                "../integrations/connectors/home-assistant-mcp-setup/0.1.0/recipes.json"
            ),
        ),
        (
            include_bytes!("../integrations/connectors/gmail-api-setup/0.1.0/plugin.json"),
            None,
            include_bytes!("../integrations/connectors/gmail-api-setup/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/gmail-api-setup/0.2.0/plugin.json"),
            Some(include_bytes!(
                "../integrations/connectors/gmail-api-setup/0.2.0/mcp.json"
            )),
            include_bytes!("../integrations/connectors/gmail-api-setup/0.2.0/recipes.json"),
        ),
        (
            include_bytes!(
                "../integrations/connectors/google-calendar-api-setup/0.1.0/plugin.json"
            ),
            None,
            include_bytes!(
                "../integrations/connectors/google-calendar-api-setup/0.1.0/recipes.json"
            ),
        ),
        (
            include_bytes!(
                "../integrations/connectors/google-calendar-api-setup/0.2.0/plugin.json"
            ),
            Some(include_bytes!(
                "../integrations/connectors/google-calendar-api-setup/0.2.0/mcp.json"
            )),
            include_bytes!(
                "../integrations/connectors/google-calendar-api-setup/0.2.0/recipes.json"
            ),
        ),
        (
            include_bytes!("../integrations/frontends/telegram-frontend-setup/0.1.0/plugin.json"),
            None,
            include_bytes!("../integrations/frontends/telegram-frontend-setup/0.1.0/recipes.json"),
        ),
        (
            include_bytes!("../integrations/connectors/agno-brave-donor/0.1.0/plugin.json"),
            None,
            include_bytes!("../integrations/connectors/agno-brave-donor/0.1.0/recipes.json"),
        ),
    ]
}
