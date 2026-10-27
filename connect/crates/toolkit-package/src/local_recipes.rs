//! Host-selected local reuse of exact integration bundles and local selections.
//!
//! An agent investigates an integration outside the catalog and selects a
//! concrete bundle or supported Python/Node/MCPB selection; the host saves those
//! exact documents with decision and historical observation references in a
//! durable store namespace it chooses. Reopening returns the same bytes after
//! revalidation through the bundle or canonical package/selection owners.
//! Saving is not publication, maintainer support, installation, activation,
//! credential capture or readiness. No setup journal is written.
//!
//! Store record ids are `local-recipe/v1/` followed by the publisher, name and
//! version, each written as `<UTF-8 byte length>:<value>`. Identity components
//! may contain `/`, so the length prefixes keep distinct identities distinct.
//! This key encoding is shared by all stored versions: version 1 bundles,
//! version 2 Python/Node selections, version 3 MCPB selections and version 4
//! software declarations cannot shadow
//! or replace one another.

use crate::{
    ActivationManifest, CatalogOrigin, ContentDigest, EvidenceReference, Id, ImportedMcpServers,
    ImportedPluginManifest, IntegrationBundle, IntegrationPackage, LocalSoftwareDeclaration,
    LocalSoftwareDocumentKind, McpbInstallPlan, NodeInstallPlan, PackageIdentity,
    PortablePluginExport, PythonInstallPlan, Revision, ValidatedLocalSoftware, ValidatedManifest,
    ValidationError, canonical_json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use toolkit_store::{
    AdmissionOutcome, AdmissionRequest, Durability, RecordData, StoreError, StoreKey, StoreScope,
    StoredRecord, ToolkitStore,
};

pub const LOCAL_RECIPE_SCHEMA: &str = "toolkit.local-recipe";
const LOCAL_RECIPE_VERSION: u32 = 1;
const LOCAL_SELECTION_VERSION: u32 = 2;
const MCPB_SELECTION_VERSION: u32 = 3;
const LOCAL_SOFTWARE_VERSION: u32 = 4;
const MAX_SELECTION_DOCUMENT: usize = 1024 * 1024;
const MAX_VERIFICATION_REFERENCES: usize = 256;
/// Every local recipe record id starts with this, in any format version.
const KEY_FAMILY: &str = "local-recipe/";
const KEY_PREFIX: &str = "local-recipe/v1/";
const LIST_PAGE: u32 = 200;
const MAX_PAGE_ROWS: u32 = 50;
const MAX_PAGE_SCANNED: u32 = 200;
const MAX_PAGE_BYTES: usize = 256 * 1024;
const MAX_SUMMARY_BYTES: usize = 64 * 1024;
const MAX_CURSOR_BYTES: usize = 4096;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LocalRecipeError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("the store does not declare durable unique admission")]
    StoreNotDurable,
    #[error("a different recipe is already saved under this immutable identity")]
    ImmutableConflict,
    /// The store did not acknowledge the save. The entry is not reported as
    /// saved; a later load still verifies whatever the store holds.
    #[error("the store did not confirm the save: {0}")]
    Unconfirmed(#[source] StoreError),
    #[error(transparent)]
    Store(StoreError),
    /// Stored content failed verification. It is never returned as reusable.
    #[error("stored local recipe failed verification: {0}")]
    Unverified(&'static str),
    #[error("invalid local recipe page request")]
    InvalidPage,
    /// The record found for the requested identity names another identity.
    #[error("stored local recipe is for {stored:?}, not the requested {requested:?}")]
    IdentityMismatch {
        requested: Box<PackageIdentity>,
        stored: Box<PackageIdentity>,
    },
}

/// One verified local recipe: the exact saved documents, the bundle parsed
/// from them, and the agent decision that selected it.
#[derive(Debug, Clone)]
pub struct LocalRecipe {
    bundle: IntegrationBundle,
    documents: SavedDocuments,
}

impl LocalRecipe {
    /// Carries its local decision, so a non-local library refuses it.
    pub fn bundle(&self) -> &IntegrationBundle {
        &self.bundle
    }

    pub fn decision(&self) -> &EvidenceReference {
        &self.documents.decision
    }

    pub fn origin(&self) -> CatalogOrigin {
        CatalogOrigin::LocalInvestigation {
            decision: self.documents.decision.clone(),
        }
    }

    pub fn plugin_json(&self) -> &[u8] {
        self.documents.plugin_json.as_bytes()
    }

    pub fn mcp_json(&self) -> Option<&[u8]> {
        self.documents.mcp_json.as_deref().map(str::as_bytes)
    }

    pub fn recipes_json(&self) -> &[u8] {
        self.documents.recipes_json.as_bytes()
    }
}

#[derive(Debug)]
pub struct LocalRecipeListing {
    pub verified: Vec<LocalRecipe>,
    /// Stored local recipe records that failed verification, by store id.
    pub unverified: Vec<(String, LocalRecipeError)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedDocuments {
    identity: PackageIdentity,
    bundle_digest: ContentDigest,
    plugin_json: String,
    mcp_json: Option<String>,
    recipes_json: String,
    decision: EvidenceReference,
    // Omitted for historical and caller-supplied references. Older readers
    // reject marked rows because SavedDocuments denies unknown fields.
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_current_submission")]
    current_submission: Option<CurrentSubmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum CurrentSubmission {
    #[serde(rename = "v1")]
    V1,
}

fn deserialize_current_submission<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<CurrentSubmission>, D::Error> {
    match String::deserialize(deserializer)?.as_str() {
        "v1" => Ok(Some(CurrentSubmission::V1)),
        _ => Err(serde::de::Error::custom("unsupported current submission marker")),
    }
}

/// Reference to the exact submission retained inside this owner's recipe row.
/// Its record is an owner-defined digest address, not the raw database key.
/// No timestamp, workspace path or decision reference enters either digest.
fn current_submission_reference(
    scope: &StoreScope,
    identity: &PackageIdentity,
    plugin_json: &str,
    mcp_json: Option<&str>,
    recipes_json: &str,
) -> Result<EvidenceReference, ValidationError> {
    let address = canonical_json(&(
        "toolkit.local-recipe.current-submission.address/v1",
        scope,
        identity_key(identity),
    ))
    .map_err(|_| ValidationError::new("local recipe", "cannot encode submission address"))?;
    let revision = canonical_json(&(
        "toolkit.local-recipe.current-submission.documents/v1",
        plugin_json,
        mcp_json,
        recipes_json,
    ))
    .map_err(|_| ValidationError::new("local recipe", "cannot encode submission documents"))?;
    let invalid = |_| ValidationError::new("local recipe", "invalid submission reference");
    Ok(EvidenceReference {
        owner: Id::new("toolkit.local-recipes.current-submission").map_err(invalid)?,
        record: Id::new(format!("submission/{}", ContentDigest::of_bytes(&address)))
            .map_err(invalid)?,
        revision: Revision::new(ContentDigest::of_bytes(&revision).to_string())
            .map_err(invalid)?,
    })
}

/// Supported metadata profiles, checked by their existing installation-plan owners.
/// This does not verify artifacts, runtime compatibility or installation results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalSelectionProfile {
    Python,
    Node,
    Mcpb,
}

// A retained observation, not a second plugin declaration. The exact historical
// inspection summaries are opaque; package meaning is checked by the existing
// importers below. None of these paths is opened while retaining or reading it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSourceObservation {
    schema_version: u32,
    scope: String,
    staged_id: String,
    inspector_sha256: ContentDigest,
    report: PluginSourceReport,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSourceReport {
    schema_version: u32,
    scope: String,
    format: String,
    status: String,
    source_digest: ContentDigest,
    files: Vec<PluginSourceFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSourceFile {
    path: String,
    text: String,
    byte_length: usize,
    source_digest: ContentDigest,
    inspection: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginSourceInspection {
    schema_version: u32,
    scope: String,
    format: String,
    status: String,
    source_digest: ContentDigest,
    summary: serde_json::Value,
    diagnostics: Vec<crate::ImportDiagnostic>,
}

fn validate_plugin_source(bytes: &[u8]) -> Result<(), LocalRecipeError> {
    parse_plugin_source(bytes).map(|_| ())
}

fn parse_plugin_source(bytes: &[u8]) -> Result<PluginSourceReport, LocalRecipeError> {
    let invalid =
        || ValidationError::new("plugin source", "invalid retained public file observation");
    let source: PluginSourceObservation =
        serde_json::from_value(crate::strict_mcpb_json(bytes)?).map_err(|_| invalid())?;
    let child = source.staged_id.as_bytes();
    if source.schema_version != 1
        || source.scope != "observed_public_plugin_files"
        || child.is_empty()
        || child.len() > 128
        || !child[0].is_ascii_alphanumeric()
        || !child
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(b))
    {
        return Err(invalid().into());
    }
    // Deserialization validates the digest syntax. It identifies the historical
    // inspector bytes; it is not a signature or authority to execute that file.
    let _inspector = source.inspector_sha256;
    let report = source.report;
    if report.schema_version != 1
        || report.scope != "metadata_only"
        || report.format != "agent-plugin-files"
        || report.files.is_empty()
        || report.files.len() > 16
        || report
            .files
            .iter()
            .filter(|f| f.path == "./plugin.json")
            .count()
            != 1
        || ContentDigest::of_bytes(&canonical_json(&report.files).map_err(|_| invalid())?)
            != report.source_digest
    {
        return Err(invalid().into());
    }
    let mut names = BTreeSet::new();
    let mut total = 0;
    let mut diagnostics = false;
    for file in &report.files {
        let tail = file.path.strip_prefix("./").unwrap_or_default();
        let raw = file.text.as_bytes();
        total += raw.len();
        if tail.is_empty()
            || file.path.len() > 1024
            || tail.contains(['\\', '\0', ':'])
            || tail
                .split('/')
                .any(|part| part.is_empty() || matches!(part, "." | ".."))
            || !names.insert(&file.path)
            || raw.len() > 32 * 1024
            || total > 128 * 1024
            || file.byte_length != raw.len()
            || file.source_digest != ContentDigest::of_bytes(raw)
            || file
                .text
                .chars()
                .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        {
            return Err(invalid().into());
        }
        let format = match file.path.as_str() {
            "./plugin.json" => {
                // Includes validation of a present Toolkit namespace; a plain
                // interoperable plugin remains a valid source observation.
                ImportedPluginManifest::parse(raw)?.toolkit_package()?;
                Some("agent-plugin")
            }
            "./mcp.json" => {
                ImportedMcpServers::parse(raw)?;
                Some("mcp-servers")
            }
            _ => None,
        };
        if let Some(format) = format {
            let inspection: PluginSourceInspection =
                serde_json::from_value(file.inspection.clone()).map_err(|_| invalid())?;
            let has_diagnostics = !inspection.diagnostics.is_empty();
            if inspection.schema_version != 1
                || inspection.scope != "metadata_only"
                || inspection.format != format
                || inspection.source_digest != file.source_digest
                || !inspection.summary.is_object()
                || inspection.status
                    != if has_diagnostics {
                        "accepted_with_diagnostics"
                    } else {
                        "accepted"
                    }
            {
                return Err(invalid().into());
            }
            diagnostics |= has_diagnostics;
        } else if !file.inspection.is_null() {
            return Err(invalid().into());
        }
    }
    if report.status
        != if diagnostics {
            "accepted_with_diagnostics"
        } else {
            "accepted"
        }
    {
        return Err(invalid().into());
    }
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSelection {
    identity: PackageIdentity,
    package_content_digest: ContentDigest,
    manifest_digest: ContentDigest,
    package_json: String,
    selection_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    configuration_json: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_source_json: Option<String>,
    profile: LocalSelectionProfile,
    component: Id,
    decision: EvidenceReference,
    verification: Vec<EvidenceReference>,
}

/// Exact historical local selection with separately bound evidence references.
///
/// No referenced evidence owner is contacted or authenticated. Configuration,
/// policy, lifecycle and readiness remain historical declarations. Reuse needs
/// current host admission and the existing artifact/installer checks; loading
/// this value never grants access, follows a path or executes selected code.
#[derive(Debug, Clone)]
pub struct RetainedLocalSelection {
    package: IntegrationPackage,
    validated: ValidatedManifest,
    documents: SavedSelection,
    digest: ContentDigest,
}

impl RetainedLocalSelection {
    /// Validate before opening a store. Each UTF-8 document is limited to 1 MiB,
    /// evidence to 1-256 distinct references, and the encoded record to the
    /// existing store limit. Raw document formatting and evidence order matter.
    pub fn parse(
        package_json: &[u8],
        selection_json: &[u8],
        profile: LocalSelectionProfile,
        component: Id,
        decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<Self, LocalRecipeError> {
        Self::parse_documents(
            package_json,
            selection_json,
            profile,
            None,
            None,
            component,
            decision,
            verification,
        )
    }

    /// Retain exact caller-designated nonsecret MCPB configuration as metadata.
    /// No archive is read. Bundled sensitivity/completeness/launch compatibility
    /// must still be checked by the existing installer against verified bytes.
    pub fn parse_mcpb(
        package_json: &[u8],
        selection_json: &[u8],
        configuration_json: &[u8],
        component: Id,
        decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<Self, LocalRecipeError> {
        Self::parse_documents(
            package_json,
            selection_json,
            LocalSelectionProfile::Mcpb,
            Some(configuration_json),
            None,
            component,
            decision,
            verification,
        )
    }

    #[allow(clippy::too_many_arguments)] // Exact optional source accompanies the existing documents.
    fn parse_documents(
        package_json: &[u8],
        selection_json: &[u8],
        profile: LocalSelectionProfile,
        configuration_json: Option<&[u8]>,
        plugin_source_json: Option<&[u8]>,
        component: Id,
        decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<Self, LocalRecipeError> {
        let invalid = |message| ValidationError::new("local selection", message);
        if package_json.len() > MAX_SELECTION_DOCUMENT
            || selection_json.len() > MAX_SELECTION_DOCUMENT
            || configuration_json.is_some_and(|bytes| bytes.len() > MAX_SELECTION_DOCUMENT)
            || plugin_source_json.is_some_and(|bytes| bytes.len() > MAX_SELECTION_DOCUMENT)
        {
            return Err(invalid("document exceeds the selection read limit").into());
        }
        if (profile == LocalSelectionProfile::Mcpb) != configuration_json.is_some() {
            return Err(
                invalid("MCPB requires its exact configuration; other profiles forbid it").into(),
            );
        }
        if let Some(source) = plugin_source_json {
            validate_plugin_source(source)?;
        }
        validate_selection_evidence(&verification)?;
        if profile == LocalSelectionProfile::Mcpb {
            crate::strict_mcpb_json(package_json)?;
            crate::strict_mcpb_json(selection_json)?;
        }
        let package: IntegrationPackage = serde_json::from_slice(package_json)
            .map_err(|_| invalid("invalid package document"))?;
        let selection: ActivationManifest = serde_json::from_slice(selection_json)
            .map_err(|_| invalid("invalid selection document"))?;
        let validated = selection.validate(&package)?;
        match profile {
            LocalSelectionProfile::Python => {
                PythonInstallPlan::new(&package, &validated, &component)?;
            }
            LocalSelectionProfile::Node => {
                NodeInstallPlan::new(&package, &validated, &component)?;
            }
            LocalSelectionProfile::Mcpb => {
                McpbInstallPlan::validate_selection(&package, &validated, &component)?;
                McpbInstallPlan::validate_configuration(
                    &validated,
                    configuration_json.expect("MCPB configuration checked above"),
                )?;
            }
        }
        let text = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec()).map_err(|_| invalid("documents must be UTF-8"))
        };
        let documents = SavedSelection {
            identity: package.identity.clone(),
            package_content_digest: package.content_digest()?,
            manifest_digest: validated.digest().clone(),
            package_json: text(package_json)?,
            selection_json: text(selection_json)?,
            configuration_json: configuration_json.map(text).transpose()?,
            plugin_source_json: plugin_source_json.map(text).transpose()?,
            profile,
            component,
            decision,
            verification,
        };
        selection_data(&documents)?
            .validate()
            .map_err(|_| invalid("retained selection exceeds the store record limit"))?;
        // Domain and version are part of this exact-retention identity, distinct
        // from the canonical package and manifest digests exposed above.
        let digest = ContentDigest::of_bytes(
            &canonical_json(&(
                LOCAL_RECIPE_SCHEMA,
                documents.storage_version()?,
                &documents,
            ))
            .map_err(|_| invalid("cannot encode retained selection"))?,
        );
        Ok(Self {
            package,
            validated,
            documents,
            digest,
        })
    }

    /// Attach exact host-designated public plugin evidence to this historical
    /// selection. The source is data, not proof of authorship or build origin.
    /// Source bytes participate in the immutable retention digest. Omitting
    /// them preserves the earlier record format and its exact digest.
    pub fn with_plugin_source(self, bytes: &[u8]) -> Result<Self, LocalRecipeError> {
        Self::parse_documents(
            self.documents.package_json.as_bytes(),
            self.documents.selection_json.as_bytes(),
            self.documents.profile,
            self.documents
                .configuration_json
                .as_deref()
                .map(str::as_bytes),
            Some(bytes),
            self.documents.component.clone(),
            self.documents.decision.clone(),
            self.documents.verification.clone(),
        )
    }

    /// Exact historical file report, revalidated without opening any file or
    /// executing an inspector. Optional on records written by earlier hosts.
    pub fn plugin_source_json(&self) -> Option<&[u8]> {
        self.documents
            .plugin_source_json
            .as_deref()
            .map(str::as_bytes)
    }

    pub fn package(&self) -> &IntegrationPackage {
        &self.package
    }

    pub fn validated_selection(&self) -> &ValidatedManifest {
        &self.validated
    }

    /// Binds the exact documents, selected profile/component and all references.
    pub fn digest(&self) -> &ContentDigest {
        &self.digest
    }

    pub fn package_json(&self) -> &[u8] {
        self.documents.package_json.as_bytes()
    }

    pub fn selection_json(&self) -> &[u8] {
        self.documents.selection_json.as_bytes()
    }

    /// Exact explicit configuration exists only for a version-3 MCPB record.
    pub fn configuration_json(&self) -> Option<&[u8]> {
        self.documents
            .configuration_json
            .as_deref()
            .map(str::as_bytes)
    }

    pub fn configuration_digest(&self) -> Option<&ContentDigest> {
        self.documents
            .configuration_json
            .as_ref()
            .map(|_| &self.validated.manifest().configuration.content_digest)
    }

    pub fn profile(&self) -> LocalSelectionProfile {
        self.documents.profile
    }

    pub fn component(&self) -> &Id {
        &self.documents.component
    }

    pub fn decision(&self) -> &EvidenceReference {
        &self.documents.decision
    }

    pub fn verification(&self) -> &[EvidenceReference] {
        &self.documents.verification
    }

    pub fn origin(&self) -> CatalogOrigin {
        CatalogOrigin::LocalInvestigation {
            decision: self.documents.decision.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedSoftware {
    document_kind: LocalSoftwareDocumentKind,
    identity: PackageIdentity,
    package_content_digest: ContentDigest,
    declaration_digest: ContentDigest,
    package_json: String,
    declaration_json: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugin_source_json: Option<String>,
    profile: LocalSelectionProfile,
    component: Id,
    decision: EvidenceReference,
    verification: Vec<EvidenceReference>,
}

/// Exact historical software documents, with no fabricated host selection.
/// The same store owns admission, isolation and immutable identity. This type
/// cannot be converted into the legacy ValidatedManifest proof.
#[derive(Debug, Clone)]
pub struct RetainedLocalSoftware {
    package: IntegrationPackage,
    validated: ValidatedLocalSoftware,
    documents: SavedSoftware,
    digest: ContentDigest,
}

impl RetainedLocalSoftware {
    pub fn parse(
        package_json: &[u8], declaration_json: &[u8], profile: LocalSelectionProfile,
        component: Id, decision: EvidenceReference, verification: Vec<EvidenceReference>,
    ) -> Result<Self, LocalRecipeError> {
        Self::parse_documents(package_json, declaration_json, profile, None,
            component, decision, verification)
    }

    fn parse_documents(
        package_json: &[u8], declaration_json: &[u8], profile: LocalSelectionProfile,
        plugin_source_json: Option<&[u8]>, component: Id, decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<Self, LocalRecipeError> {
        let invalid = |message| ValidationError::new("local software", message);
        if package_json.len() > MAX_SELECTION_DOCUMENT || declaration_json.len() > MAX_SELECTION_DOCUMENT
            || plugin_source_json.is_some_and(|bytes| bytes.len() > MAX_SELECTION_DOCUMENT) {
            return Err(invalid("document exceeds the selection read limit").into());
        }
        if profile == LocalSelectionProfile::Mcpb {
            return Err(invalid("plain software supports only Python and Node").into());
        }
        if let Some(source) = plugin_source_json { validate_plugin_source(source)?; }
        validate_selection_evidence(&verification)?;
        let package: IntegrationPackage = serde_json::from_slice(package_json)
            .map_err(|_| invalid("invalid package document"))?;
        let declaration: LocalSoftwareDeclaration = serde_json::from_slice(declaration_json)
            .map_err(|_| invalid("invalid software document"))?;
        let validated = declaration.validate(&package)?;
        match profile {
            LocalSelectionProfile::Python => { PythonInstallPlan::from_declaration(&package, &validated, &component)?; }
            LocalSelectionProfile::Node => { NodeInstallPlan::from_declaration(&package, &validated, &component)?; }
            LocalSelectionProfile::Mcpb => unreachable!("profile refused above"),
        }
        let text = |bytes: &[u8]| String::from_utf8(bytes.to_vec())
            .map_err(|_| invalid("documents must be UTF-8"));
        let documents = SavedSoftware {
            document_kind: LocalSoftwareDocumentKind::LocalSoftwareDeclaration,
            identity: package.identity.clone(), package_content_digest: package.content_digest()?,
            declaration_digest: validated.digest().clone(),
            package_json: text(package_json)?, declaration_json: text(declaration_json)?,
            plugin_source_json: plugin_source_json.map(text).transpose()?,
            profile, component, decision, verification,
        };
        software_data(&documents)?.validate()
            .map_err(|_| invalid("retained software exceeds the store record limit"))?;
        let digest = ContentDigest::of_bytes(&canonical_json(&(
            LOCAL_RECIPE_SCHEMA, LOCAL_SOFTWARE_VERSION, &documents))
            .map_err(|_| invalid("cannot encode retained software"))?);
        Ok(Self { package, validated, documents, digest })
    }

    pub fn with_plugin_source(self, bytes: &[u8]) -> Result<Self, LocalRecipeError> {
        Self::parse_documents(self.package_json(), self.declaration_json(), self.documents.profile,
            Some(bytes), self.documents.component.clone(), self.documents.decision.clone(),
            self.documents.verification.clone())
    }
    pub fn package(&self) -> &IntegrationPackage { &self.package }
    pub fn validated_declaration(&self) -> &ValidatedLocalSoftware { &self.validated }
    pub fn digest(&self) -> &ContentDigest { &self.digest }
    pub fn package_json(&self) -> &[u8] { self.documents.package_json.as_bytes() }
    pub fn declaration_json(&self) -> &[u8] { self.documents.declaration_json.as_bytes() }
    pub fn plugin_source_json(&self) -> Option<&[u8]> { self.documents.plugin_source_json.as_deref().map(str::as_bytes) }
    pub fn profile(&self) -> LocalSelectionProfile { self.documents.profile }
    pub fn component(&self) -> &Id { &self.documents.component }
    pub fn decision(&self) -> &EvidenceReference { &self.documents.decision }
    pub fn verification(&self) -> &[EvidenceReference] { &self.documents.verification }
}

/// Explicit public text selected from one exact retained source observation.
/// Paths are literal, not patterns. This cannot select root assets or source
/// directories, and it declares no MCP configuration or executable entrypoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedGuidanceSelection {
    pub plugin_source_digest: ContentDigest,
    pub files: Vec<String>,
}

/// All stored forms share one immutable package-identity namespace.
#[derive(Debug, Clone)]
pub enum RetainedLocalRecipe {
    Recipe(LocalRecipe),
    Selection(RetainedLocalSelection),
    Software(RetainedLocalSoftware),
}

impl RetainedLocalRecipe {
    /// Export only explicitly selected skill text from retained source bytes.
    /// Callers reopen the exact identity and recipe digest through
    /// `load_retained_exact` first. Source metadata is canonicalized with all
    /// namespaces preserved; no MCP document or activation is produced.
    pub fn export_guidance(
        &self,
        selection: &RetainedGuidanceSelection,
    ) -> Result<PortablePluginExport, LocalRecipeError> {
        let invalid =
            || ValidationError::new("guidance export", "invalid retained guidance selection");
        let source = match self {
            Self::Recipe(_) => None,
            Self::Selection(recipe) => recipe.plugin_source_json(),
            Self::Software(recipe) => recipe.plugin_source_json(),
        }
        .ok_or_else(invalid)?;
        if ContentDigest::of_bytes(source) != selection.plugin_source_digest
            || selection.files.is_empty()
            || selection.files.len() > 15
        {
            return Err(invalid().into());
        }
        let report = parse_plugin_source(source)?;
        let mut names = BTreeSet::new();
        for path in &selection.files {
            let parts = path
                .strip_prefix("./")
                .ok_or_else(invalid)?
                .split('/')
                .collect::<Vec<_>>();
            if path.len() > 1024
                || path.contains(['\\', ':'])
                || path.chars().any(char::is_control)
                || parts.len() < 3
                || parts.len() > 16
                || parts[0] != "skills"
                || parts
                    .iter()
                    .any(|part| part.is_empty() || matches!(*part, "." | ".."))
                || !names.insert(path.as_str())
            {
                return Err(invalid().into());
            }
        }
        for path in &selection.files {
            let skill = path[2..]
                .split('/')
                .nth(1)
                .expect("selected skill path checked above");
            if !names.contains(format!("./skills/{skill}/SKILL.md").as_str())
                || path[2..]
                    .match_indices('/')
                    .any(|(at, _)| names.contains(&path[..at + 2]))
                || !report.files.iter().any(|file| file.path == *path)
            {
                return Err(invalid().into());
            }
        }
        let manifest = report
            .files
            .iter()
            .find(|file| file.path == "./plugin.json")
            .ok_or_else(invalid)?;
        let imported = ImportedPluginManifest::parse(manifest.text.as_bytes())?;
        // The existing constructor validates any Toolkit namespace and permits
        // metadata-only export without weakening executable selection checks.
        let mut export = PortablePluginExport::new(&imported.manifest, &Default::default(), None)
            .map_err(|_| invalid())?;
        for path in &selection.files {
            let file = report
                .files
                .iter()
                .find(|file| file.path == *path)
                .ok_or_else(invalid)?;
            export = export.with_retained_text(path, file.text.as_bytes())?;
        }
        Ok(export)
    }

    /// Version 1 retains its existing bundle digest; versions 2-4 use their exact
    /// document-and-evidence retention digest. Neither is a readiness receipt.
    pub fn digest(&self) -> &ContentDigest {
        match self {
            Self::Recipe(recipe) => recipe.bundle().digest(),
            Self::Selection(selection) => selection.digest(),
            Self::Software(software) => software.digest(),
        }
    }

    /// The same nonsecret metadata projection used by exact reads and listings.
    /// This is historical data, not installation or current authority evidence.
    pub fn summary(&self) -> Result<serde_json::Value, LocalRecipeError> {
        use serde_json::json;
        match self {
            Self::Recipe(recipe) => recipe.summary(),
            Self::Software(recipe) => {
                let package = recipe.package();
                let mut value = json!({
                    "kind": "local_software_selection", "document_kind": "local_software_declaration",
                    "identity": package.identity, "digest": recipe.digest(),
                    "package_content_digest": package.content_digest()?,
                    "declaration_digest": recipe.validated_declaration().digest(),
                    "profile": recipe.profile(), "component": recipe.component(),
                    "origin": {"kind": "local_investigation", "decision": recipe.decision()},
                    "verification": recipe.verification(), "display_name": package.display_name,
                    "reuse": "requires_current_host_selection_and_artifact_validation",
                });
                if let Some(source) = recipe.plugin_source_json() {
                    value["plugin_source_digest"] = json!(ContentDigest::of_bytes(source));
                }
                Ok(value)
            }
            Self::Selection(recipe) => {
                let package = recipe.package();
                let mut value = json!({
                    "kind": "local_selection", "identity": package.identity,
                    "digest": recipe.digest(),
                    "package_content_digest": package.content_digest()?,
                    "manifest_digest": recipe.validated_selection().digest(),
                    "profile": recipe.profile(), "component": recipe.component(),
                    "origin": {"kind": "local_investigation", "decision": recipe.decision()},
                    "verification": recipe.verification(),
                    "display_name": package.display_name,
                    "reuse": "requires_current_host_selection_and_artifact_validation",
                });
                if let Some(source) = recipe.plugin_source_json() {
                    value["plugin_source_digest"] = json!(ContentDigest::of_bytes(source));
                }
                if let Some(configuration) = recipe.configuration_json() {
                    value["configuration_digest"] = json!(ContentDigest::of_bytes(configuration));
                }
                Ok(value)
            }
        }
    }

    fn identity(&self) -> &PackageIdentity {
        match self {
            Self::Recipe(recipe) => &recipe.bundle().package().identity,
            Self::Selection(recipe) => &recipe.package().identity,
            Self::Software(recipe) => &recipe.package().identity,
        }
    }
}

impl LocalRecipe {
    pub fn summary(&self) -> Result<serde_json::Value, LocalRecipeError> {
        use serde_json::json;
        let package = self.bundle().package();
        Ok(json!({
            "identity": package.identity,
            "digest": self.bundle().digest(),
            "package_content_digest": package.content_digest()?,
            "origin": self.origin(),
            "display_name": package.display_name,
            "strategies": package.strategies.iter().map(|s| json!({
                "id": s.id, "kind": s.kind,
            })).collect::<Vec<_>>(),
            "exports": package.known_exports.iter().map(|e| json!({
                "id": e.id, "role": e.role,
            })).collect::<Vec<_>>(),
        }))
    }
}

/// One bounded keyset observation. A cursor is query data, not authorization or
/// a snapshot. A non-null cursor must be followed even when no recipe was found.
#[derive(Debug, Serialize)]
pub struct RetainedLocalRecipePage {
    pub request_cursor: Option<String>,
    pub limit: u32,
    pub recipes: Vec<serde_json::Value>,
    pub unverified: Vec<UnverifiedRecipeSummary>,
    pub oversized: Vec<OversizedRecipeSummary>,
    pub next_cursor: Option<String>,
    /// Fetched records, including unrelated records and a final deferred row.
    pub scanned_records: u32,
}

#[derive(Debug, Serialize)]
pub struct UnverifiedRecipeSummary {
    pub error: RecipeSummaryError,
}

#[derive(Debug, Serialize)]
pub struct OversizedRecipeSummary {
    pub identity: PackageIdentity,
    pub digest: ContentDigest,
    pub error: RecipeSummaryError,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeSummaryError {
    StoredRecipeUnverified,
    SummaryTooLarge,
}

impl RetainedLocalRecipePage {
    /// The complete bounded wire envelope. The CLI forwards this projection;
    /// it does not choose rows or derive a second continuation cursor.
    pub fn wire_value(&self) -> serde_json::Value {
        let mut value = serde_json::to_value(self).expect("serializable recipe page");
        value["schema_version"] = serde_json::json!(1);
        value["scope"] = serde_json::json!("retained_recipe_metadata");
        value["status"] = serde_json::json!("listed_page");
        value
    }

    fn within_byte_limit(&self) -> bool {
        // Include the CLI's terminating newline in the pipe-output budget.
        serde_json::to_vec(&self.wire_value()).is_ok_and(|bytes| bytes.len() < MAX_PAGE_BYTES)
    }

    fn rows(&self) -> usize {
        self.recipes.len() + self.unverified.len() + self.oversized.len()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipePageCursor {
    version: u32,
    scope_digest: ContentDigest,
    after_id: String,
}

fn page_scope_digest(scope: &StoreScope) -> ContentDigest {
    ContentDigest::of_bytes(
        &canonical_json(&serde_json::json!({
            "kind": "toolkit.local_recipe_page.v1", "realm": scope.realm, "owner": scope.owner,
        }))
        .expect("serializable store scope"),
    )
}

fn page_cursor(scope: &StoreScope, after_id: &str) -> String {
    hex::encode(
        serde_json::to_vec(&RecipePageCursor {
            version: 1,
            scope_digest: page_scope_digest(scope),
            after_id: after_id.to_owned(),
        })
        .expect("serializable recipe cursor"),
    )
}

fn page_after(
    scope: &StoreScope,
    cursor: Option<&str>,
) -> Result<Option<String>, LocalRecipeError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    if cursor.is_empty()
        || cursor.len() > MAX_CURSOR_BYTES
        || cursor.len() % 2 != 0
        || !cursor
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(LocalRecipeError::InvalidPage);
    }
    let bytes = hex::decode(cursor).map_err(|_| LocalRecipeError::InvalidPage)?;
    let decoded: RecipePageCursor =
        serde_json::from_slice(&bytes).map_err(|_| LocalRecipeError::InvalidPage)?;
    let key = StoreKey {
        realm: scope.realm.clone(),
        owner: scope.owner.clone(),
        id: decoded.after_id.clone(),
    };
    if decoded.version != 1
        || decoded.scope_digest != page_scope_digest(scope)
        || key.validate().is_err()
        || page_cursor(scope, &decoded.after_id) != cursor
    {
        return Err(LocalRecipeError::InvalidPage);
    }
    Ok(Some(decoded.after_id))
}

#[derive(Debug)]
pub struct RetainedLocalRecipeListing {
    pub verified: Vec<RetainedLocalRecipe>,
    pub unverified: Vec<(String, LocalRecipeError)>,
}

impl SavedSelection {
    fn storage_version(&self) -> Result<u32, LocalRecipeError> {
        match (self.profile, self.configuration_json.is_some()) {
            (LocalSelectionProfile::Python | LocalSelectionProfile::Node, false) => {
                Ok(LOCAL_SELECTION_VERSION)
            }
            (LocalSelectionProfile::Mcpb, true) => Ok(MCPB_SELECTION_VERSION),
            _ => Err(LocalRecipeError::Unverified(
                "profile and configuration format differ",
            )),
        }
    }
}

fn validate_selection_evidence(verification: &[EvidenceReference]) -> Result<(), LocalRecipeError> {
    let invalid = |message| ValidationError::new("local selection", message);
    let distinct: BTreeSet<_> = verification
        .iter()
        .map(|reference| (&reference.owner, &reference.record, &reference.revision))
        .collect();
    if verification.is_empty()
        || verification.len() > MAX_VERIFICATION_REFERENCES
        || distinct.len() != verification.len()
    {
        return Err(invalid("verification requires 1-256 distinct evidence references").into());
    }
    Ok(())
}

fn software_data(documents: &SavedSoftware) -> Result<RecordData, LocalRecipeError> {
    Ok(RecordData {
        schema: LOCAL_RECIPE_SCHEMA.into(), version: LOCAL_SOFTWARE_VERSION,
        body: serde_json::to_value(documents).map_err(|_| {
            ValidationError::new("local software", "cannot encode saved documents")
        })?,
    })
}

fn selection_data(documents: &SavedSelection) -> Result<RecordData, LocalRecipeError> {
    Ok(RecordData {
        schema: LOCAL_RECIPE_SCHEMA.into(),
        version: documents.storage_version()?,
        body: serde_json::to_value(documents).map_err(|_| {
            ValidationError::new("local selection", "cannot encode saved documents")
        })?,
    })
}

/// Local recipes in one host-selected store namespace. The scope comes from
/// the authenticated host, never from tool input.
pub struct LocalRecipeStore<'a> {
    store: &'a dyn ToolkitStore,
    scope: StoreScope,
}

impl<'a> LocalRecipeStore<'a> {
    pub fn new(store: &'a dyn ToolkitStore, scope: StoreScope) -> Result<Self, LocalRecipeError> {
        let capabilities = store.capabilities();
        if capabilities.durability != Durability::Durable || !capabilities.unique_admission {
            return Err(LocalRecipeError::StoreNotDurable);
        }
        scope.validate().map_err(LocalRecipeError::Store)?;
        Ok(Self { store, scope })
    }

    /// Verify and save an agent-selected bundle. Replaying the same documents
    /// and decision is idempotent; anything else under the same identity is an
    /// immutable conflict.
    pub async fn save(
        &self,
        plugin_json: &[u8],
        mcp_json: Option<&[u8]>,
        recipes_json: &[u8],
        decision: EvidenceReference,
    ) -> Result<LocalRecipe, LocalRecipeError> {
        self.save_with_decision(plugin_json, mcp_json, recipes_json, Some(decision))
            .await
    }

    /// Retain the agent's present concrete selection with its own stable
    /// investigation reference in this same durable row. This is provenance,
    /// not approval, source authenticity, installation or account readiness.
    /// Exact replay keeps the reference across setup workspaces and restarts.
    pub async fn save_current_submission(
        &self,
        plugin_json: &[u8],
        mcp_json: Option<&[u8]>,
        recipes_json: &[u8],
    ) -> Result<LocalRecipe, LocalRecipeError> {
        self.save_with_decision(plugin_json, mcp_json, recipes_json, None)
            .await
    }

    async fn save_with_decision(
        &self,
        plugin_json: &[u8],
        mcp_json: Option<&[u8]>,
        recipes_json: &[u8],
        decision: Option<EvidenceReference>,
    ) -> Result<LocalRecipe, LocalRecipeError> {
        let bundle = IntegrationBundle::parse(plugin_json, mcp_json, recipes_json)?;
        let text = |bytes: &[u8]| {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| ValidationError::new("local recipe", "documents must be UTF-8"))
        };
        let plugin_json = text(plugin_json)?;
        let mcp_json = mcp_json.map(text).transpose()?;
        let recipes_json = text(recipes_json)?;
        let current_submission = decision.is_none().then_some(CurrentSubmission::V1);
        let decision = match decision {
            Some(reference) => reference,
            None => current_submission_reference(
                &self.scope,
                &bundle.package().identity,
                &plugin_json,
                mcp_json.as_deref(),
                &recipes_json,
            )?,
        };
        let documents = SavedDocuments {
            identity: bundle.package().identity.clone(),
            bundle_digest: bundle.digest().clone(),
            plugin_json,
            mcp_json,
            recipes_json,
            decision,
            current_submission,
        };
        let key = self.key(&documents.identity);
        let request = AdmissionRequest {
            admission_key: key.id.clone(),
            key,
            data: RecordData {
                schema: LOCAL_RECIPE_SCHEMA.into(),
                version: LOCAL_RECIPE_VERSION,
                body: serde_json::to_value(&documents).map_err(|_| {
                    ValidationError::new("local recipe", "cannot encode saved documents")
                })?,
            },
            events: Vec::new(),
            effects: Vec::new(),
        };
        let record = match self.store.admit(request).await {
            Ok(AdmissionOutcome::Created(record) | AdmissionOutcome::Existing(record)) => record,
            Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => {
                return Err(LocalRecipeError::ImmutableConflict);
            }
            Err(error @ (StoreError::Synchronization | StoreError::Worker)) => {
                return Err(LocalRecipeError::Unconfirmed(error));
            }
            Err(error) => return Err(LocalRecipeError::Store(error)),
        };
        let saved = verify(&self.scope, &record, Some(&documents.identity))?;
        // An idempotent admission returns the current record, which may have
        // changed since its initial admission. Confirm the exact submitted
        // documents and decision before acknowledging this save or replay.
        if saved.documents != documents {
            return Err(LocalRecipeError::Unverified(
                "stored recipe differs from the submitted documents or decision",
            ));
        }
        Ok(saved)
    }

    /// Load and re-verify one saved recipe. `None` means this namespace has no
    /// recipe for the identity; other namespaces are never consulted.
    pub async fn load(
        &self,
        identity: &PackageIdentity,
    ) -> Result<Option<LocalRecipe>, LocalRecipeError> {
        self.store
            .get(&self.key(identity))
            .await
            .map_err(LocalRecipeError::Store)?
            .map(|record| verify(&self.scope, &record, Some(identity)))
            .transpose()
    }

    /// Load the recipe an agent selected by identity and bundle digest. Stored
    /// content that verifies as a different bundle is refused, not returned.
    pub async fn load_exact(
        &self,
        identity: &PackageIdentity,
        digest: &ContentDigest,
    ) -> Result<Option<LocalRecipe>, LocalRecipeError> {
        match self.load(identity).await? {
            Some(recipe) if recipe.bundle.digest() != digest => Err(LocalRecipeError::Unverified(
                "stored recipe differs from the selected digest",
            )),
            loaded => Ok(loaded),
        }
    }

    /// Retain a supported local selection as historical metadata. The same
    /// immutable key is used by version-1 recipes, so neither form replaces
    /// the other. An unconfirmed save requires readback, not assumed failure.
    pub async fn save_selection(
        &self,
        package_json: &[u8],
        selection_json: &[u8],
        profile: LocalSelectionProfile,
        component: Id,
        decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<RetainedLocalSelection, LocalRecipeError> {
        let selected = RetainedLocalSelection::parse(
            package_json,
            selection_json,
            profile,
            component,
            decision,
            verification,
        )?;
        self.save_retained_selection(selected).await
    }

    /// Save caller-designated nonsecret MCPB metadata without preparing or
    /// executing an archive. Readback never restores installation authority.
    pub async fn save_mcpb_selection(
        &self,
        package_json: &[u8],
        selection_json: &[u8],
        configuration_json: &[u8],
        component: Id,
        decision: EvidenceReference,
        verification: Vec<EvidenceReference>,
    ) -> Result<RetainedLocalSelection, LocalRecipeError> {
        let selected = RetainedLocalSelection::parse_mcpb(
            package_json,
            selection_json,
            configuration_json,
            component,
            decision,
            verification,
        )?;
        self.save_retained_selection(selected).await
    }

    /// Save an already validated selection, including optional original plugin
    /// evidence. Admission and readback use the same immutable store boundary
    /// as the convenience save methods; this cannot replace an existing record.
    pub async fn save_retained_selection(
        &self,
        selected: RetainedLocalSelection,
    ) -> Result<RetainedLocalSelection, LocalRecipeError> {
        let key = self.key(&selected.package.identity);
        let request = AdmissionRequest {
            admission_key: key.id.clone(),
            key,
            data: selection_data(&selected.documents)?,
            events: Vec::new(),
            effects: Vec::new(),
        };
        let record = match self.store.admit(request).await {
            Ok(AdmissionOutcome::Created(record) | AdmissionOutcome::Existing(record)) => record,
            Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => {
                return Err(LocalRecipeError::ImmutableConflict);
            }
            Err(error @ (StoreError::Synchronization | StoreError::Worker)) => {
                return Err(LocalRecipeError::Unconfirmed(error));
            }
            Err(error) => return Err(LocalRecipeError::Store(error)),
        };
        let saved = verify_selection(&self.scope, &record, Some(&selected.package.identity))?;
        if saved.documents != selected.documents {
            return Err(LocalRecipeError::Unverified(
                "stored selection differs from submitted documents or evidence",
            ));
        }
        Ok(saved)
    }

    /// Save exact software declarations in the same immutable namespace. Old
    /// readers reject version 4; no old record is migrated or overwritten.
    pub async fn save_retained_software(
        &self, selected: RetainedLocalSoftware,
    ) -> Result<RetainedLocalSoftware, LocalRecipeError> {
        let key = self.key(&selected.package.identity);
        let request = AdmissionRequest {
            admission_key: key.id.clone(), key,
            data: software_data(&selected.documents)?, events: Vec::new(), effects: Vec::new(),
        };
        let record = match self.store.admit(request).await {
            Ok(AdmissionOutcome::Created(record) | AdmissionOutcome::Existing(record)) => record,
            Err(StoreError::AdmissionConflict | StoreError::AlreadyExists) => return Err(LocalRecipeError::ImmutableConflict),
            Err(error @ (StoreError::Synchronization | StoreError::Worker)) => return Err(LocalRecipeError::Unconfirmed(error)),
            Err(error) => return Err(LocalRecipeError::Store(error)),
        };
        let saved = verify_software(&self.scope, &record, Some(&selected.package.identity))?;
        if saved.documents != selected.documents {
            return Err(LocalRecipeError::Unverified("stored software differs from submitted documents or evidence"));
        }
        Ok(saved)
    }

    /// Reopen either form using its exact lookup digest. A different, even
    /// internally consistent, stored value cannot replace the caller's choice.
    pub async fn load_retained_exact(
        &self,
        identity: &PackageIdentity,
        digest: &ContentDigest,
    ) -> Result<Option<RetainedLocalRecipe>, LocalRecipeError> {
        let retained = self
            .store
            .get(&self.key(identity))
            .await
            .map_err(LocalRecipeError::Store)?
            .map(|record| verify_retained(&self.scope, &record, Some(identity)))
            .transpose()?;
        match retained {
            Some(recipe) if recipe.digest() != digest => Err(LocalRecipeError::Unverified(
                "stored recipe differs from the selected digest",
            )),
            loaded => Ok(loaded),
        }
    }

    pub async fn load_selection_exact(
        &self,
        identity: &PackageIdentity,
        digest: &ContentDigest,
    ) -> Result<Option<RetainedLocalSelection>, LocalRecipeError> {
        match self.load_retained_exact(identity, digest).await? {
            Some(RetainedLocalRecipe::Selection(selection)) => Ok(Some(selection)),
            Some(RetainedLocalRecipe::Recipe(_) | RetainedLocalRecipe::Software(_)) => Err(LocalRecipeError::Unverified(
                "stored recipe is not a local selection",
            )),
            None => Ok(None),
        }
    }

    /// List both stored forms. The original `list` remains version-1-only;
    /// unsupported or corrupt records are reported, never silently hidden.
    pub async fn list_retained(&self) -> Result<RetainedLocalRecipeListing, LocalRecipeError> {
        let mut listing = RetainedLocalRecipeListing {
            verified: Vec::new(),
            unverified: Vec::new(),
        };
        let mut after = None::<String>;
        loop {
            let page = self
                .store
                .list_records(&self.scope, after.as_deref(), LIST_PAGE)
                .await
                .map_err(LocalRecipeError::Store)?;
            let Some(last) = page.last() else { break };
            after = Some(last.key.id.clone());
            for record in &page {
                if !record.key.id.starts_with(KEY_FAMILY)
                    && record.data.schema != LOCAL_RECIPE_SCHEMA
                {
                    continue;
                }
                match verify_retained(&self.scope, record, None) {
                    Ok(recipe) => listing.verified.push(recipe),
                    Err(error) => listing.unverified.push((record.key.id.clone(), error)),
                }
            }
            if page.len() < LIST_PAGE as usize {
                break;
            }
        }
        Ok(listing)
    }

    /// Observe a bounded page without first loading the whole namespace.
    ///
    /// The opaque cursor binds this host-selected scope and the last consumed
    /// store key. Each call fetches at most 200 records, one at a time, and emits
    /// at most `limit` rows. Invalid recipe contents produce fixed observations;
    /// malformed storage envelopes still fail through the store owner. Error
    /// rows omit keys and malformed content, but the operator cursor encodes a
    /// consumed store key. Cursor encoding provides no confidentiality.
    /// A summary exceeding 64 KiB is
    /// represented by its verified identity and digest so traversal can advance.
    /// The complete wire envelope is limited to 256 KiB of compact JSON.
    ///
    /// End-of-list is observed only by an empty store read. Concurrent insertion
    /// before the cursor is not included; start again without a cursor to rescan.
    pub async fn list_retained_page(
        &self,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<RetainedLocalRecipePage, LocalRecipeError> {
        Self::validate_page_request(&self.scope, cursor, limit)?;
        let mut after = page_after(&self.scope, cursor)?;
        let mut page = RetainedLocalRecipePage {
            request_cursor: cursor.map(str::to_owned),
            limit,
            recipes: Vec::new(),
            unverified: Vec::new(),
            oversized: Vec::new(),
            next_cursor: cursor.map(str::to_owned),
            scanned_records: 0,
        };
        while page.scanned_records < MAX_PAGE_SCANNED && page.rows() < limit as usize {
            let mut records = self
                .store
                .list_records(&self.scope, after.as_deref(), 1)
                .await
                .map_err(LocalRecipeError::Store)?;
            if records.is_empty() {
                page.next_cursor = None;
                return Ok(page);
            }
            if records.len() != 1 {
                return Err(LocalRecipeError::Unverified(
                    "store exceeded page read bound",
                ));
            }
            let record = records.pop().expect("one bounded store record");
            if record.key.scope() != self.scope
                || record.key.validate().is_err()
                || after.as_ref().is_some_and(|prior| record.key.id <= *prior)
            {
                return Err(LocalRecipeError::Unverified(
                    "store page ordering or scope differs",
                ));
            }
            page.scanned_records += 1;
            let previous_cursor = page.next_cursor.clone();
            page.next_cursor = Some(page_cursor(&self.scope, &record.key.id));
            let row_kind = if !record.key.id.starts_with(KEY_FAMILY)
                && record.data.schema != LOCAL_RECIPE_SCHEMA
            {
                0
            } else {
                match verify_retained(&self.scope, &record, None) {
                    Ok(recipe) => {
                        let summary = recipe.summary()?;
                        if serde_json::to_vec(&summary)
                            .expect("serializable recipe summary")
                            .len()
                            > MAX_SUMMARY_BYTES
                        {
                            page.oversized.push(OversizedRecipeSummary {
                                identity: recipe.identity().clone(),
                                digest: recipe.digest().clone(),
                                error: RecipeSummaryError::SummaryTooLarge,
                            });
                            1
                        } else {
                            page.recipes.push(summary);
                            2
                        }
                    }
                    Err(_) => {
                        page.unverified.push(UnverifiedRecipeSummary {
                            error: RecipeSummaryError::StoredRecipeUnverified,
                        });
                        3
                    }
                }
            };
            if !page.within_byte_limit() {
                match row_kind {
                    1 => {
                        page.oversized.pop();
                    }
                    2 => {
                        page.recipes.pop();
                    }
                    3 => {
                        page.unverified.pop();
                    }
                    _ => {}
                }
                page.next_cursor = previous_cursor;
                // A bounded single summary always fits an otherwise empty page.
                // Refuse instead of returning a cursor that cannot make progress.
                if page.next_cursor.as_deref() == cursor || !page.within_byte_limit() {
                    return Err(LocalRecipeError::Unverified(
                        "page cannot advance within bound",
                    ));
                }
                return Ok(page);
            }
            after = Some(record.key.id);
        }
        Ok(page)
    }

    /// Validate operator paging input without accessing or opening a store.
    pub fn validate_page_request(
        scope: &StoreScope,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<(), LocalRecipeError> {
        if !(1..=MAX_PAGE_ROWS).contains(&limit) || scope.validate().is_err() {
            return Err(LocalRecipeError::InvalidPage);
        }
        page_after(scope, cursor)?;
        Ok(())
    }

    /// Every local recipe record in this namespace, verified independently.
    /// Records of other schemas in the namespace are not local recipes.
    pub async fn list(&self) -> Result<LocalRecipeListing, LocalRecipeError> {
        let mut listing = LocalRecipeListing {
            verified: Vec::new(),
            unverified: Vec::new(),
        };
        let mut after = None::<String>;
        loop {
            let page = self
                .store
                .list_records(&self.scope, after.as_deref(), LIST_PAGE)
                .await
                .map_err(LocalRecipeError::Store)?;
            let Some(last) = page.last() else { break };
            after = Some(last.key.id.clone());
            for record in &page {
                if !record.key.id.starts_with(KEY_FAMILY)
                    && record.data.schema != LOCAL_RECIPE_SCHEMA
                {
                    continue;
                }
                match verify(&self.scope, record, None) {
                    Ok(recipe) => listing.verified.push(recipe),
                    Err(error) => listing.unverified.push((record.key.id.clone(), error)),
                }
            }
            if page.len() < LIST_PAGE as usize {
                break;
            }
        }
        Ok(listing)
    }

    fn key(&self, identity: &PackageIdentity) -> StoreKey {
        StoreKey {
            realm: self.scope.realm.clone(),
            owner: self.scope.owner.clone(),
            id: identity_key(identity),
        }
    }
}

/// See the module documentation for this stable, unambiguous encoding.
fn identity_key(identity: &PackageIdentity) -> String {
    let mut key = String::from(KEY_PREFIX);
    for component in [
        identity.publisher.as_str(),
        identity.name.as_str(),
        identity.version.as_str(),
    ] {
        key.push_str(&format!("{}:{component}", component.len()));
    }
    key
}

fn verify_retained(
    scope: &StoreScope,
    record: &StoredRecord,
    requested: Option<&PackageIdentity>,
) -> Result<RetainedLocalRecipe, LocalRecipeError> {
    if matches!(
        record.data.version,
        LOCAL_SELECTION_VERSION | MCPB_SELECTION_VERSION
    ) {
        verify_selection(scope, record, requested).map(RetainedLocalRecipe::Selection)
    } else if record.data.version == LOCAL_SOFTWARE_VERSION {
        verify_software(scope, record, requested).map(RetainedLocalRecipe::Software)
    } else {
        verify(scope, record, requested).map(RetainedLocalRecipe::Recipe)
    }
}

fn verify_software(
    scope: &StoreScope, record: &StoredRecord, requested: Option<&PackageIdentity>,
) -> Result<RetainedLocalSoftware, LocalRecipeError> {
    if record.key.scope() != *scope {
        return Err(LocalRecipeError::Unverified("record is from another namespace"));
    }
    if record.data.schema != LOCAL_RECIPE_SCHEMA || record.data.version != LOCAL_SOFTWARE_VERSION
        || record.data.body.get("plugin_source_json").is_some_and(serde_json::Value::is_null) {
        return Err(LocalRecipeError::Unverified("unsupported local software format"));
    }
    if record.data.digest().ok().as_deref() != Some(record.content_digest.as_str()) {
        return Err(LocalRecipeError::Unverified("record digest does not match its content"));
    }
    let documents: SavedSoftware = serde_json::from_value(record.data.body.clone())
        .map_err(|_| LocalRecipeError::Unverified("invalid saved software"))?;
    if let Some(requested) = requested && documents.identity != *requested {
        return Err(LocalRecipeError::IdentityMismatch {
            requested: Box::new(requested.clone()), stored: Box::new(documents.identity),
        });
    }
    let software = RetainedLocalSoftware::parse_documents(
        documents.package_json.as_bytes(), documents.declaration_json.as_bytes(), documents.profile,
        documents.plugin_source_json.as_deref().map(str::as_bytes), documents.component.clone(),
        documents.decision.clone(), documents.verification.clone(),
    ).map_err(|_| LocalRecipeError::Unverified("saved software no longer validates"))?;
    if software.documents != documents || record.key.id != identity_key(&documents.identity) {
        return Err(LocalRecipeError::Unverified("saved software does not match its recorded identity or digests"));
    }
    Ok(software)
}

fn verify_selection(
    scope: &StoreScope,
    record: &StoredRecord,
    requested: Option<&PackageIdentity>,
) -> Result<RetainedLocalSelection, LocalRecipeError> {
    if record.key.scope() != *scope {
        return Err(LocalRecipeError::Unverified(
            "record is from another namespace",
        ));
    }
    if record.data.schema != LOCAL_RECIPE_SCHEMA
        || !matches!(
            record.data.version,
            LOCAL_SELECTION_VERSION | MCPB_SELECTION_VERSION
        )
        || (record.data.version == LOCAL_SELECTION_VERSION
            && record.data.body.get("configuration_json").is_some())
        || record
            .data
            .body
            .get("plugin_source_json")
            .is_some_and(serde_json::Value::is_null)
    {
        return Err(LocalRecipeError::Unverified(
            "unsupported local selection format",
        ));
    }
    if record.data.digest().ok().as_deref() != Some(record.content_digest.as_str()) {
        return Err(LocalRecipeError::Unverified(
            "record digest does not match its content",
        ));
    }
    let documents: SavedSelection = serde_json::from_value(record.data.body.clone())
        .map_err(|_| LocalRecipeError::Unverified("invalid saved selection"))?;
    if documents.storage_version()? != record.data.version {
        return Err(LocalRecipeError::Unverified(
            "profile and stored version differ",
        ));
    }
    if let Some(requested) = requested
        && documents.identity != *requested
    {
        return Err(LocalRecipeError::IdentityMismatch {
            requested: Box::new(requested.clone()),
            stored: Box::new(documents.identity),
        });
    }
    let selected = RetainedLocalSelection::parse_documents(
        documents.package_json.as_bytes(),
        documents.selection_json.as_bytes(),
        documents.profile,
        documents.configuration_json.as_deref().map(str::as_bytes),
        documents.plugin_source_json.as_deref().map(str::as_bytes),
        documents.component.clone(),
        documents.decision.clone(),
        documents.verification.clone(),
    )
    .map_err(|_| LocalRecipeError::Unverified("saved selection no longer validates"))?;
    if selected.documents != documents || record.key.id != identity_key(&documents.identity) {
        return Err(LocalRecipeError::Unverified(
            "saved selection does not match its recorded identity or digests",
        ));
    }
    Ok(selected)
}

/// Everything is re-derived from the stored documents: the store envelope,
/// the record digest, the parsed bundle digest and the identity key.
/// `requested` is the identity a caller asked for; a record that names any
/// other identity is refused rather than returned in its place.
fn verify(
    scope: &StoreScope,
    record: &StoredRecord,
    requested: Option<&PackageIdentity>,
) -> Result<LocalRecipe, LocalRecipeError> {
    if record.key.scope() != *scope {
        return Err(LocalRecipeError::Unverified(
            "record is from another namespace",
        ));
    }
    if record.data.schema != LOCAL_RECIPE_SCHEMA || record.data.version != LOCAL_RECIPE_VERSION {
        return Err(LocalRecipeError::Unverified(
            "unsupported local recipe format",
        ));
    }
    if record.data.digest().ok().as_deref() != Some(record.content_digest.as_str()) {
        return Err(LocalRecipeError::Unverified(
            "record digest does not match its content",
        ));
    }
    let documents: SavedDocuments = serde_json::from_value(record.data.body.clone())
        .map_err(|_| LocalRecipeError::Unverified("invalid saved documents"))?;
    if let Some(requested) = requested
        && documents.identity != *requested
    {
        return Err(LocalRecipeError::IdentityMismatch {
            requested: Box::new(requested.clone()),
            stored: Box::new(documents.identity),
        });
    }
    let bundle = IntegrationBundle::parse(
        documents.plugin_json.as_bytes(),
        documents.mcp_json.as_deref().map(str::as_bytes),
        documents.recipes_json.as_bytes(),
    )
    .map_err(|_| LocalRecipeError::Unverified("saved documents no longer parse"))?;
    if bundle.digest() != &documents.bundle_digest
        || bundle.package().identity != documents.identity
        || record.key.id != identity_key(&documents.identity)
    {
        return Err(LocalRecipeError::Unverified(
            "saved documents do not match their recorded identity or digest",
        ));
    }
    if documents.current_submission == Some(CurrentSubmission::V1)
        && documents.decision != current_submission_reference(
            scope,
            &documents.identity,
            &documents.plugin_json,
            documents.mcp_json.as_deref(),
            &documents.recipes_json,
        )?
    {
        return Err(LocalRecipeError::Unverified(
            "current submission reference does not match its scope, key or documents",
        ));
    }
    Ok(LocalRecipe {
        bundle: bundle.with_local_decision(documents.decision.clone()),
        documents,
    })
}

#[cfg(test)]
mod page_framing_tests {
    use super::*;

    #[test]
    fn page_budget_reserves_the_actual_cli_newline() {
        let mut page = RetainedLocalRecipePage {
            request_cursor: None,
            limit: 1,
            recipes: vec![serde_json::json!("")],
            unverified: vec![],
            oversized: vec![],
            next_cursor: None,
            scanned_records: 1,
        };
        let base = serde_json::to_vec(&page.wire_value()).unwrap().len();
        page.recipes[0] = serde_json::json!("x".repeat(MAX_PAGE_BYTES - base - 1));
        assert_eq!(
            serde_json::to_vec(&page.wire_value()).unwrap().len() + 1,
            MAX_PAGE_BYTES
        );
        assert!(page.within_byte_limit());
        page.recipes[0] = serde_json::json!("x".repeat(MAX_PAGE_BYTES - base));
        assert_eq!(
            serde_json::to_vec(&page.wire_value()).unwrap().len(),
            MAX_PAGE_BYTES
        );
        assert!(!page.within_byte_limit());
    }
}
