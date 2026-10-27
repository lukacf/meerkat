//! Application setup: legacy artifact preparation and final native compilation phases.
//!
//! The trusted application supplies an unsealed definition, roster and explicit
//! selections. This command prepares the installed resource library and returns
//! one immutable schema3 compiler input bundle. The schema4 Rust phase prepares
//! final profiles and explicit policy before native construction, then collects
//! actual member outputs for the existing producer. Neither path installs policy, initializes
//! identity state, issues grants or opens an agent runtime.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use futures_util::FutureExt;
use meerkat_core::{McpServerConfig, MobMemberBinding, ResolvedStorage};
use meerkat_mob::{MobDefinition, ProfileName};
use meerkat_mobkit::{
    identity_first::DurableAgentSpec, member_comms_id::roster_member_id_for_identity,
};
use meerkat_toolkit::host_lifetime::{
    control::SelectedHostArtifact,
    policy::{CompiledArtifactReference, CompiledNativeMember},
};
use serde::{Deserialize, Serialize};
use toolkit_mobkit::installed_host::acquisition::InstalledSetupSelection;

mod surface;
pub mod compilation;
pub mod inventory;
pub(crate) mod resource;

use crate::{
    application_bootstrap::{InstallerProgress, emit, hash, new_child, write_new},
    application_tool_policy::ApplicationToolPolicyInput,
    realm::RealmOptions,
    resources::{InstalledResources, PreparedResources},
    setup::{PreparedSetupConnection, PreparedSetupHelper, SetupHelperTools},
};

/// Explicit inputs to the existing native storage resolver. The prepared
/// artifacts belong to this realm/root pair; a matching name elsewhere is not
/// the same selection. Resolving these inputs creates no native realm state.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSetupRealm {
    pub realm_id: String,
    pub context_root: PathBuf,
    pub state_root: PathBuf,
    pub user_config_root: Option<PathBuf>,
}

impl ApplicationSetupRealm {
    pub fn resolve(&self) -> Result<ResolvedStorage, crate::ConnectError> {
        if !self.context_root.is_absolute()
            || !self.state_root.is_absolute()
            || self
                .user_config_root
                .as_ref()
                .is_some_and(|path| !path.is_absolute())
        {
            return Err(crate::ConnectError::InvalidInput(
                "Application setup roots must be absolute".into(),
            ));
        }
        RealmOptions {
            context_root: self.context_root.clone(),
            state_root: Some(self.state_root.clone()),
            user_config_root: self.user_config_root.clone(),
        }
        .resolve(&self.realm_id)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationSetupTools {
    Preparation,
    InertAcquisition,
    /// Preparation, inert acquisition and the existing checked software install
    /// workflow. The gateway retains either its selected human presenter or
    /// the actual application's exact installation-policy composition.
    /// Tool allowances and service access remain independent selections.
    CheckedInstallation,
}
impl ApplicationSetupTools {
    pub(crate) fn requires_software_review(self) -> bool {
        matches!(self, Self::CheckedInstallation)
    }

    fn native(self) -> SetupHelperTools {
        match self {
            Self::Preparation => SetupHelperTools::Preparation,
            Self::InertAcquisition => SetupHelperTools::InertAcquisition,
            Self::CheckedInstallation => SetupHelperTools::CheckedInstallation,
        }
    }
}

/// The original application definition source. TOML text is passed verbatim
/// to the native parser and retained verbatim in the compiler bundle. Native
/// typed inputs remain native; neither path serializes a definition to TOML.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplicationSetupDefinition {
    Toml { text: String },
    Native { definition: MobDefinition },
}

impl ApplicationSetupDefinition {
    fn resolve(&self) -> Result<MobDefinition, String> {
        match self {
            Self::Toml { text } => {
                MobDefinition::from_toml(text).map_err(|error| error.to_string())
            }
            Self::Native { definition } => Ok(definition.clone()),
        }
    }
}

/// Explicit configuration choice, never inferred from an application role name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationSetupProfile {
    Dedicated,
    ExclusiveApplicationRole,
}

impl ApplicationSetupProfile {
    fn project(
        self,
        connection: PreparedSetupConnection,
        member: MobMemberBinding,
        tools: ApplicationSetupTools,
    ) -> Result<PreparedSetupHelper, crate::ConnectError> {
        match self {
            Self::Dedicated => connection.for_mobkit_helper(member, tools.native()),
            Self::ExclusiveApplicationRole => {
                connection.for_mobkit_helper_in_place(member, tools.native())
            }
        }
    }
}

/// Trusted, pre-policy application input. `helper.member` still names its
/// original inline research role. Grant references and tool consequences are
/// explicit application selections and remain unchanged. No current permission
/// is inferred from this document.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSetupInput {
    pub schema: u32,
    pub realm: ApplicationSetupRealm,
    pub definition: ApplicationSetupDefinition,
    /// One actual provider result used only to validate the initial selection.
    /// The prepared bundle never freezes or replaces this whole roster.
    pub roster: Vec<DurableAgentSpec>,
    pub native_members: Vec<CompiledNativeMember>,
    pub helper: InstalledSetupSelection,
    /// Actual application-owned source artifact for the selected member's
    /// initial surface. Its name/hash must match that member's reference.
    pub helper_source_surface: SelectedHostArtifact,
    pub helper_tools: ApplicationSetupTools,
    pub helper_profile: ApplicationSetupProfile,
    pub tool_policy: ApplicationToolPolicyInput,
}

/// Pin to the resource owner's retained handoff. The prepared directory stays
/// where that owner created it; moving or preparing it again is not a restart.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedApplicationResources {
    pub artifact_directory: PathBuf,
    pub handoff: SelectedHostArtifact,
}

/// The application's compiler consumes `native_members` and the tool-policy
/// artifact reference when building its own opaque policy. Native bootstrap
/// consumes this SAME `definition` and `helper` before sealing. The application
/// applies `project_roster` to EACH actual live provider result, including
/// refresh/reconcile. It never replaces its provider with a retained roster.
/// This is configuration, not a readiness receipt or a native provider wrapper.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedApplicationSetup {
    pub schema: u32,
    /// Schema4 driver's resolved same-owner facts, including actual issued
    /// parent references. The application compiles these exact facts into its
    /// final selection. They are configuration, not authority or native Ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_owner: Option<meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy>,
    pub input_sha256: String,
    pub realm: ApplicationSetupRealm,
    /// Exact source retained within the existing input provenance. For TOML,
    /// input_sha256 covers this raw text, including ignored keys and comments.
    pub definition_source: ApplicationSetupDefinition,
    pub definition: MobDefinition,
    pub source_helper_roster: DurableAgentSpec,
    pub native_members: Vec<CompiledNativeMember>,
    pub source_helper_member: MobMemberBinding,
    pub helper: InstalledSetupSelection,
    /// Schema3 retains its genuine original helper artifact. Schema4 forbids
    /// this field and uses final member artifacts in resources.artifact_directory.
    /// Omission in schema4 never creates a legacy source artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_source_surface: Option<SelectedHostArtifact>,
    pub helper_tools: ApplicationSetupTools,
    pub helper_profile: ApplicationSetupProfile,
    pub server: McpServerConfig,
    pub resources: PreparedApplicationResources,
    pub application_tool_policy: CompiledArtifactReference,
}

impl PreparedApplicationSetup {
    /// Reconstruct the exact profile projection on restart from the retained
    /// preparer handoff. The application must first authenticate/pin this bundle
    /// through its existing installer/activation owner. This method checks
    /// consistency only; arbitrary JSON never grants execution authority.
    /// This verifies the retained compiler input only. Each live roster result
    /// must separately pass project_roster before native reconciliation. Runtime
    /// binding fields are backend declarations, not proof of sealing.
    pub fn helper_projection(&self) -> Result<PreparedSetupHelper, String> {
        if !matches!(self.schema, 3 | 4)
            || self.definition.source_identity.is_some()
            || !self.resources.artifact_directory.is_absolute()
            || self.resources.handoff.path
                != self.resources.artifact_directory.join("prepare.stdout")
        {
            return Err("invalid prepared application resource selection".into());
        }
        if (self.schema == 4) != self.native_owner.is_some() {
            return Err("schema4 requires its resolved native owner; schema3 cannot carry one".into());
        }
        if let Some(owner) = &self.native_owner {
            owner.validate().map_err(|error| error.to_string())?;
            let members = self.native_members.iter().map(|member| meerkat_toolkit::host_lifetime::policy::PreparedNativeMember {
                member: member.member.clone(), logical_owner: member.logical_owner.clone(),
                context_generation: member.context_generation, audience: member.audience.clone(),
                setup_capability: member.setup_capability.clone(),
            }).collect::<Vec<_>>();
            if self.schema != 4 || owner.native_host.members != members
                || owner.native_host.realm.as_str() != self.realm.realm_id
            { return Err("resolved native owner differs from the completed application".into()); }
        }
        let bytes = read_regular_artifact(&self.resources.handoff.path)?;
        if hash(&bytes) != self.resources.handoff.sha256 {
            return Err("retained application preparer handoff changed".into());
        }
        let projection = PreparedSetupConnection::from_preparer_output(&bytes)
            .and_then(|connection| {
                self.helper_profile.project(
                    connection,
                    self.source_helper_member.clone(),
                    self.helper_tools,
                )
            })
            .map_err(|error| error.to_string())?
            .with_retained_public_research(&self.server);
        if projection.member() != &self.helper.member || projection.server() != &self.server {
            return Err("prepared application helper differs from retained resources".into());
        }
        let mut expected = self.definition_source.resolve()?;
        if expected.source_identity.is_some() {
            return Err("prepared application source was already verified/sealed".into());
        }
        projection
            .compose_definition(&mut expected)
            .map_err(|error| error.to_string())?;
        if expected != self.definition {
            return Err(
                "prepared application definition is missing its exact helper profile".into(),
            );
        }
        let member = matching_member(&self.native_members, &self.helper.member)?;
        match self.schema {
            3 => {
                let original = self.helper_source_surface.as_ref()
                    .ok_or("schema3 requires its original helper surface artifact")?;
                let compiled = surface::compile(original)?;
                if self.native_members[member].initial_surface != surface::reference(&compiled) {
                    return Err("prepared helper initial surface differs from its compiler input".into());
                }
                surface::require_exclusive_output_name(&self.native_members, member)?;
            }
            4 => {
                if self.helper_source_surface.is_some() {
                    return Err("schema4 cannot relabel an original helper surface".into());
                }
                compilation::validate_final_artifacts(self)?;
            }
            _ => unreachable!(),
        }
        if self.native_members.iter().enumerate().any(|(index, row)| {
            index != member
                && row.member.mob_id == self.helper.member.mob_id
                && row.member.role == self.helper.member.role
        }) {
            return Err("another native policy member borrows the setup helper role".into());
        }
        // Validate the retained selected row's identity/profile/backend shape.
        // This does not stand in for a call on the actual live provider result.
        self.project_roster(std::slice::from_ref(&self.source_helper_roster))?;
        Ok(projection)
    }

    /// Bind retained configuration to the separately authenticated application
    /// policy. Neither absent nor extra public-research ceiling entries can be
    /// repaired during restoration or prewrite completion.
    pub(crate) fn helper_projection_for_policy(
        &self, policy: &meerkat_core::CompiledApplicationToolPolicy,
    ) -> Result<PreparedSetupHelper, String> {
        let helper = self.helper_projection()?.with_public_research_from_policy(policy);
        if helper.server() != &self.server {
            return Err("retained helper ceiling differs from its compiled policy".into());
        }
        Ok(helper)
    }

    /// Pure projection over one actual RosterProvider result. Authenticate the
    /// prepared bundle and call helper_projection once when selecting resources,
    /// then use this on every provider read before native compilation/reconcile.
    /// Missing or changed selected identity/profile/backend refuses. No retained
    /// row is inserted; unrelated current rows and their order are preserved.
    pub fn project_roster(
        &self,
        current: &[DurableAgentSpec],
    ) -> Result<Vec<DurableAgentSpec>, String> {
        project_roster_selection(&self.source_helper_member, &self.helper.member,
            &self.source_helper_roster, self.helper_profile, current)
    }
}

fn project_roster_selection(
    source_member: &MobMemberBinding,
    helper_member: &MobMemberBinding,
    source_roster: &DurableAgentSpec,
    profile: ApplicationSetupProfile,
    current: &[DurableAgentSpec],
) -> Result<Vec<DurableAgentSpec>, String> {
        if source_member.mob_id != helper_member.mob_id
            || source_member.member != helper_member.member
            || source_roster.profile.as_str() != source_member.role
            || (profile == ApplicationSetupProfile::ExclusiveApplicationRole
                && source_member != helper_member)
        {
            return Err("prepared helper roster projection changed its selected binding".into());
        }
        let index = matching_roster(current, source_member)?;
        let selected = &current[index];
        let mut expected = (*source_roster).clone();
        // A provider may already return the exact projected dedicated role on
        // restart. Every other selected row field must still match its source.
        if selected.profile.as_str() == helper_member.role {
            expected.profile = selected.profile.clone();
        }
        if selected != &expected
            || current.iter().enumerate().any(|(other, row)| {
                other != index && row.profile.as_str() == helper_member.role
            })
        {
            return Err("current roster no longer has the exact exclusive setup helper".into());
        }
        let mut projected = current.to_vec();
        projected[index].profile = ProfileName::from(helper_member.role.clone());
        Ok(projected)
}

struct ValidatedInput {
    input: ApplicationSetupInput,
    input_sha256: String,
    storage: ResolvedStorage,
    definition: MobDefinition,
    roster_index: usize,
    member_index: usize,
    tool_policy: Vec<u8>,
    helper_surface: Vec<u8>,
}

fn matching_roster(
    roster: &[DurableAgentSpec],
    member: &MobMemberBinding,
) -> Result<usize, String> {
    let mut identities = BTreeSet::new();
    let mut matched = None;
    for (index, row) in roster.iter().enumerate() {
        let alias = roster_member_id_for_identity(row.identity.as_str());
        if !identities.insert(alias.to_string()) {
            return Err("duplicate native roster identity".into());
        }
        if alias.as_str() == member.member {
            matched = Some(index);
        }
    }
    matched.ok_or_else(|| "selected helper is absent from the native roster".into())
}

fn matching_member(
    members: &[CompiledNativeMember],
    selected: &MobMemberBinding,
) -> Result<usize, String> {
    let mut identities = BTreeSet::new();
    let mut matched = None;
    for (index, row) in members.iter().enumerate() {
        row.member.comms_name().map_err(|error| error.to_string())?;
        if !identities.insert((&row.member.mob_id, &row.member.member)) {
            return Err("duplicate native member identity".into());
        }
        if &row.member == selected {
            matched = Some(index);
        }
    }
    matched.ok_or_else(|| "selected helper is absent from native policy members".into())
}

impl ApplicationSetupInput {
    fn validate(self) -> Result<ValidatedInput, String> {
        let definition = self.definition.resolve()?;
        if self.schema != 3 || definition.id.as_str() != self.helper.member.mob_id {
            return Err("application setup schema or selected mob differs".into());
        }
        // A verified mobpack is already content-bound by its own owner. This
        // command accepts the application's pre-artifact definition only.
        if definition.source_identity.is_some() {
            return Err(
                "prepare the application definition before artifact verification/sealing".into(),
            );
        }
        self.helper
            .member
            .comms_name()
            .map_err(|error| error.to_string())?;
        let roster_index = matching_roster(&self.roster, &self.helper.member)?;
        let member_index = matching_member(&self.native_members, &self.helper.member)?;
        surface::require_source_selection(
            &self.helper_source_surface,
            &self.native_members[member_index].initial_surface,
        )?;
        surface::require_exclusive_output_name(&self.native_members, member_index)?;
        let helper_surface = surface::compile(&self.helper_source_surface)?;
        surface::require_report_policy(&self.tool_policy, &self.helper.member)?;
        if self.roster[roster_index].profile.as_str() != self.helper.member.role {
            return Err("selected helper must have its original research profile".into());
        }
        // DurableAgentSpec.binding selects a backend (including an explicit
        // Session), not whether an identity was sealed. Preserve it verbatim.
        // Only the trusted application owner establishes pre-seal ordering.
        if self.helper_profile == ApplicationSetupProfile::ExclusiveApplicationRole
            && (self.roster.iter().enumerate().any(|(index, row)| {
                index != roster_index && row.profile.as_str() == self.helper.member.role
            }) || self.native_members.iter().enumerate().any(|(index, row)| {
                index != member_index
                    && row.member.mob_id == self.helper.member.mob_id
                    && row.member.role == self.helper.member.role
            }))
        {
            return Err("application helper role is not exclusive to its selected member".into());
        }
        let source = definition
            .resolve_inline_profile(&ProfileName::from(self.helper.member.role.clone()))
            .ok_or("selected helper requires an inline application research profile")?;
        if self.helper.member.role.starts_with("toolkit-setup-")
            || source.tools.mcp.iter().any(|name| name == "toolkit-setup")
            || source
                .tools
                .mcp_servers
                .iter()
                .any(|server| server.name == "toolkit-setup")
        {
            return Err("selected source profile already contains a setup provider".into());
        }
        let storage = self.realm.resolve().map_err(|error| error.to_string())?;
        let tool_policy = self
            .tool_policy
            .clone()
            .compile()
            .map_err(|error| error.to_string())?;
        // This is the exact membership condition used by InstalledToolkitHost.
        // A missing explicit policy member is not replaced by an empty grant.
        if self.native_members.iter().any(|member| {
            !self
                .tool_policy
                .members
                .iter()
                .any(|entry| entry.member_identity == member.member.member)
        }) {
            return Err("native member has no explicit application tool-policy entry".into());
        }
        let input_sha256 =
            hash(&toolkit_package::canonical_json(&self).map_err(|error| error.to_string())?);
        Ok(ValidatedInput {
            input: self,
            input_sha256,
            storage,
            definition,
            roster_index,
            member_index,
            tool_policy,
            helper_surface,
        })
    }
}

pub(crate) fn read_regular_artifact(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if !path.is_absolute() || path.is_symlink() || !path.is_file() {
        return Err("retained compiler artifact must be an absolute regular file".into());
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 256 * 1024 {
        return Err("retained compiler artifact exceeds the supported limit".into());
    }
    Ok(bytes)
}

fn finish(
    validated: ValidatedInput,
    prepared: PreparedResources,
) -> Result<(PreparedApplicationSetup, Vec<u8>), String> {
    let mut input = validated.input;
    let mut definition = validated.definition;
    let source_helper_member = input.helper.member.clone();
    let source_helper_roster = input.roster[validated.roster_index].clone();
    let projection = input
        .helper_profile
        .project(
            prepared.connection,
            source_helper_member.clone(),
            input.helper_tools,
        )
        .map_err(|error| error.to_string())?;
    projection
        .compose_definition(&mut definition)
        .map_err(|error| error.to_string())?;
    input.native_members[validated.member_index].member = projection.member().clone();
    input.native_members[validated.member_index].initial_surface =
        surface::reference(&validated.helper_surface);
    input.helper.member = projection.member().clone();
    let handoff_path = prepared.artifact_directory.join("prepare.stdout");
    let handoff = SelectedHostArtifact {
        sha256: hash(&read_regular_artifact(&handoff_path)?),
        path: handoff_path,
    };
    let result = PreparedApplicationSetup {
        schema: 3,
        native_owner: None,
        input_sha256: validated.input_sha256,
        realm: input.realm,
        definition_source: input.definition,
        definition,
        source_helper_roster,
        native_members: input.native_members,
        source_helper_member,
        helper: input.helper,
        helper_source_surface: Some(input.helper_source_surface),
        helper_tools: input.helper_tools,
        helper_profile: input.helper_profile,
        server: projection.server().clone(),
        resources: PreparedApplicationResources {
            artifact_directory: prepared.artifact_directory,
            handoff,
        },
        application_tool_policy: CompiledArtifactReference {
            name: "application-tool-policy.json".into(),
            sha256: hash(&validated.tool_policy),
        },
    };
    // Re-open the retained owner result and compare it with the in-process
    // result before publishing any compiler bundle.
    result.helper_projection()?;
    result.project_roster(&input.roster)?;
    Ok((result, validated.tool_policy))
}

fn write_bundle(
    output: &Path,
    setup: &PreparedApplicationSetup,
    policy: &[u8],
) -> Result<SelectedHostArtifact, String> {
    use std::os::unix::fs::DirBuilderExt;
    // Reconstruct and validate every generated artifact before creating the
    // output directory. The source may have changed while preparation awaited.
    setup.helper_projection()?;
    let artifacts = match setup.schema {
        3 => vec![(surface::OUTPUT_NAME.to_owned(), surface::compile(
            setup.helper_source_surface.as_ref().ok_or("schema3 source is missing")?,
        )?)],
        4 => compilation::validate_final_artifacts(setup)?,
        _ => return Err("unsupported prepared application schema".into()),
    };
    if hash(policy) != setup.application_tool_policy.sha256 {
        return Err("prepared application tool-policy bytes differ".into());
    }
    surface::require_compiled_report_policy(policy, &setup.helper.member)?;
    let setup_bytes = toolkit_package::canonical_json(setup).map_err(|error| error.to_string())?;
    let mut manifest_artifacts = vec![
        serde_json::json!({"path":"application-setup.json", "sha256":hash(&setup_bytes)}),
        serde_json::json!({"path":setup.application_tool_policy.name, "sha256":hash(policy)}),
    ];
    manifest_artifacts.extend(artifacts.iter().map(|(name, bytes)|
        serde_json::json!({"path":name, "sha256":hash(bytes)})));
    let manifest = toolkit_package::canonical_json(&serde_json::json!({
        "schema":1, "kind":"application_setup_preparation", "artifacts":manifest_artifacts
    }))
    .map_err(|error| error.to_string())?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(output)
        .map_err(|error| error.to_string())?;
    write_new(&output.join("application-setup.json"), &setup_bytes)?;
    write_new(&output.join(&setup.application_tool_policy.name), policy)?;
    for (name, bytes) in &artifacts { write_new(&output.join(name), bytes)?; }
    let path = output.join("manifest.json");
    // Last file: a partial bundle has no successful manifest/receipt. Existing
    // files are retained and never overwritten or retried automatically.
    write_new(&path, &manifest)?;
    std::fs::File::open(output)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| error.to_string())?;
    Ok(SelectedHostArtifact {
        path,
        sha256: hash(&manifest),
    })
}

/// This private seam awaits the resource owner's full outcome. Cancellation
/// belongs to that owner: never race/drop this future with a second timeout.
async fn compose_after_preparation(
    validated: ValidatedInput,
    preparation: impl std::future::Future<Output = Result<PreparedResources, crate::ConnectError>>,
) -> Result<(PreparedApplicationSetup, Vec<u8>), String> {
    let prepared = preparation.await.map_err(|error| error.to_string())?;
    finish(validated, prepared)
}

/// One shutdown observation feeds the existing preparer's cancellation owner.
/// Keep both registrations through the pre-bundle check, including a signal
/// already consumed while observing preparation.
struct PreparationShutdown {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    observed: bool,
}

impl PreparationShutdown {
    fn register() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
            observed: false,
        })
    }

    async fn cancelled(&mut self) -> std::io::Result<()> {
        if self.observed {
            return Ok(());
        }
        let received = tokio::select! {
            received = self.interrupt.recv() => received,
            received = self.terminate.recv() => received,
        };
        self.observed = true;
        received.ok_or_else(|| std::io::Error::other("application setup shutdown stream closed"))
    }

    fn observed_before_bundle(&mut self) -> bool {
        self.cancelled().now_or_never().is_some()
    }
}

/// Inspect the schema without replacing the original serialized input. The
/// legacy typed decoder must still reject duplicate known fields.
fn read_command_input(path: &str) -> Result<ApplicationSetupInput, String> {
    let bytes = read_application_input(Path::new(path))?;
    #[derive(Deserialize)]
    struct SchemaHeader {
        schema: u16,
    }
    let header: SchemaHeader = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if header.schema == 4 {
        return Err("schema4 requires the existing native producer/activation bridge; this command cannot finish native members".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

/// Shared input-byte custody for compiler and update selections. Resource
/// output artifacts keep their separate, smaller read_regular_artifact limit.
pub(crate) fn read_application_input(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;

    const MAX_INPUT_BYTES: u64 = 16 * 1024 * 1024;
    if !path.is_absolute() || path.is_symlink() || !path.is_file() {
        return Err("input must be an absolute regular path".into());
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_INPUT_BYTES {
        return Err("application setup input exceeds 16 MiB".into());
    }
    Ok(bytes)
}

pub(crate) fn command(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    let [_, input_flag, input_path, output_flag, output_path] = args else {
        return Err(
            "expected --prepare-application-setup --input FILE --output FRESH_DIRECTORY".into(),
        );
    };
    if input_flag != "--input" || output_flag != "--output" {
        return Err("invalid explicit application setup arguments".into());
    }
    progress.phase = "validation";
    let input = read_command_input(input_path)?;
    let validated = input.validate()?;
    let path = Path::new(output_path);
    if !path.is_absolute() {
        return Err("output must be an absolute fresh directory".into());
    }
    let parent = path
        .parent()
        .ok_or("output has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let output = new_child(&parent, path)?;
    let resources = InstalledResources::locate().map_err(|error| error.to_string())?;
    let runtime = meerkat_mobkit::gateway_composition::gateway_tokio_runtime()
        .map_err(|error| error.to_string())?;
    // Installer mode returns before private pipes or the ordinary gateway
    // runtime are opened. This is its sole async executor, not a second mob.
    let (setup, manifest) = runtime.block_on(async {
        let mut shutdown = PreparationShutdown::register().map_err(|error| error.to_string())?;
        progress.phase = "resource_preparation";
        progress.effects_may_have_occurred = true;
        let storage = validated.storage.clone();
        let (setup, policy) =
            compose_after_preparation(validated, resources.prepare(&storage, shutdown.cancelled()))
                .await?;
        // A signal queued just as preparation joined still prevents a new
        // bundle write. Already-entered preparer cleanup is always awaited.
        if shutdown.observed_before_bundle() {
            return Err(
                "application setup cancelled after resource preparation; retained artifacts remain"
                    .into(),
            );
        }
        progress.phase = "bundle_write";
        let manifest = write_bundle(&output, &setup, &policy)?;
        Ok::<_, String>((setup, manifest))
    })?;
    progress.phase = "receipt";
    emit(
        &serde_json::json!({"kind":"application_setup_prepared", "manifest":manifest,
        "setup":{"path":output.join("application-setup.json"),
            "sha256":hash(&toolkit_package::canonical_json(&setup).map_err(|error| error.to_string())?)},
        "application_tool_policy":{"path":output.join(&setup.application_tool_policy.name),
            "sha256":setup.application_tool_policy.sha256},
        "helper_initial_surface":{"path":output.join(surface::OUTPUT_NAME),
            "sha256":setup.native_members[matching_member(&setup.native_members, &setup.helper.member)?].initial_surface.sha256},
        "helper":setup.helper.member,
        "prepared_artifact_directory":setup.resources.artifact_directory}),
    )
}

#[cfg(test)]
mod tests;
