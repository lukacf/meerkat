//! Schema4 separates final configuration/policy preparation from member outputs.
//! The existing native producer must keep publication withheld, finish every
//! captured preparation successfully, then invoke complete_bundle. Capturing a
//! surface does not establish finish or activation. That native handoff is not
//! supplied by this module and ordinary startup must not substitute for it.
use super::*;
use async_trait::async_trait;
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_core::{
    PreparedToolCatalog, PreparedToolSurface, ToolConsequencePolicyRegistry, ToolIdentity,
    ToolSurfaceSelection,
};
use meerkat_mob::{
    MemberDestinationSelection, MemberRuntimeBuildIdentity, MemberRuntimeRegistrationHook, MobError,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, OnceLock},
};
use toolkit_mobkit::installed_host::tool_policy::PreparedInstalledToolPolicy;

pub mod update;

/// The same member facts later matched by the installed host at adoption.
/// No placeholder surface reference or independent destination codec is used.
pub use meerkat_toolkit::host_lifetime::policy::PreparedNativeMember as ApplicationMemberSelection;

/// Separate schema4 input. Legacy ApplicationSetupInput schema3 is unchanged.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationSetupCompilationInput {
    pub schema: u32,
    pub realm: ApplicationSetupRealm,
    pub definition: ApplicationSetupDefinition,
    pub roster: Vec<DurableAgentSpec>,
    pub members: Vec<ApplicationMemberSelection>,
    pub helper: InstalledSetupSelection,
    pub helper_tools: ApplicationSetupTools,
    pub helper_profile: ApplicationSetupProfile,
    pub tool_policy: ApplicationToolPolicyInput,
}

/// Validated before the resource owner is invoked. No runtime, policy worker or
/// native member construction is opened by validation.
pub struct ValidatedApplicationCompilation {
    input: ApplicationSetupCompilationInput,
    input_sha256: String,
    storage: ResolvedStorage,
    definition: MobDefinition,
    helper_index: usize,
    roster_index: usize,
    policy_bytes: Vec<u8>,
}
impl ApplicationSetupCompilationInput {
    pub fn validate(self) -> Result<ValidatedApplicationCompilation, String> {
        let definition = self.definition.resolve()?;
        if self.schema != 4
            || definition.source_identity.is_some()
            || definition.id.as_str() != self.helper.member.mob_id
            || self.members.is_empty()
        {
            return Err("schema4 requires the selected unsealed application definition".into());
        }
        let mut identities = BTreeSet::new();
        for row in &self.members {
            row.member.comms_name().map_err(|error| error.to_string())?;
            if row.member.mob_id != definition.id.as_str()
                || row.context_generation == 0
                || row.logical_owner.validate_qualified().is_err()
                || !identities.insert(row.member.member.clone())
                || !self
                    .tool_policy
                    .members
                    .iter()
                    .any(|policy| policy.member_identity == row.member.member)
            {
                return Err(
                    "member destination selection is duplicate, foreign or lacks explicit policy"
                        .into(),
                );
            }
            let audience_owner = match &row.audience {
                AudienceRef::Principal { principal } => principal,
                AudienceRef::Destination {
                    authority,
                    destination_id,
                } => {
                    if destination_id.is_empty() {
                        return Err("member audience destination is empty".into());
                    }
                    authority
                }
            };
            audience_owner
                .validate_qualified()
                .map_err(|error| error.to_string())?;
            // A native member selection must correspond to this actual provider
            // result, not an invented row that the projection later resurrects.
            let index = matching_roster(&self.roster, &row.member)?;
            if self.roster[index].profile.as_str() != row.member.role {
                return Err("member selection differs from its current roster profile".into());
            }
        }
        if self.members.len() != self.roster.len() {
            return Err("selected member inventory does not cover the current roster".into());
        }
        let helper_index = self
            .members
            .iter()
            .position(|row| row.member == self.helper.member)
            .ok_or("selected helper is absent from member selections")?;
        let roster_index = matching_roster(&self.roster, &self.helper.member)?;
        surface::require_report_policy(&self.tool_policy, &self.helper.member)?;
        if self.helper_profile == ApplicationSetupProfile::ExclusiveApplicationRole
            && (self.roster.iter().enumerate().any(|(index, row)| {
                index != roster_index && row.profile.as_str() == self.helper.member.role
            }) || self.members.iter().enumerate().any(|(index, row)| {
                index != helper_index && row.member.role == self.helper.member.role
            }))
        {
            return Err("application helper role is not exclusive".into());
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
            return Err("source helper profile already contains setup resources".into());
        }
        let storage = self.realm.resolve().map_err(|error| error.to_string())?;
        let expected_realm = meerkat_core::mob_realm_id(&self.helper.member.mob_id)
            .map_err(|error| error.to_string())?;
        if storage.locator.realm != expected_realm {
            return Err("selected storage realm differs from the native mob realm".into());
        }
        let policy_bytes = self
            .tool_policy
            .clone()
            .compile()
            .map_err(|error| error.to_string())?;
        let input_sha256 =
            hash(&toolkit_package::canonical_json(&self).map_err(|error| error.to_string())?);
        Ok(ValidatedApplicationCompilation {
            input: self,
            input_sha256,
            storage,
            definition,
            helper_index,
            roster_index,
            policy_bytes,
        })
    }
}

impl ValidatedApplicationCompilation {
    pub fn storage(&self) -> &ResolvedStorage {
        &self.storage
    }

    /// Consume the same resource owner's actual result, then construct the real
    /// immutable policy registry before any native member preparation.
    pub fn prepare(
        self,
        prepared: PreparedResources,
    ) -> Result<ApplicationSetupCompilation, String> {
        let policy = PreparedInstalledToolPolicy::from_canonical(&self.policy_bytes)
            .map_err(|error| error.to_string())?;
        let mut input = self.input;
        let source_helper_member = input.helper.member.clone();
        let source_helper_roster = input.roster[self.roster_index].clone();
        let projection = input
            .helper_profile
            .project(
                prepared.connection,
                source_helper_member.clone(),
                input.helper_tools,
            )
            .map_err(|error| error.to_string())?
            .with_public_research_from_policy(policy.compiled());
        let mut definition = self.definition;
        projection
            .compose_definition(&mut definition)
            .map_err(|error| error.to_string())?;
        input.helper.member = projection.member().clone();
        input.members[self.helper_index].member = projection.member().clone();
        let handoff_path = prepared.artifact_directory.join("prepare.stdout");
        let bytes = read_regular_artifact(&handoff_path)?;
        let retained = input
            .helper_profile
            .project(
                PreparedSetupConnection::from_preparer_output(&bytes)
                    .map_err(|error| error.to_string())?,
                source_helper_member.clone(),
                input.helper_tools,
            )
            .map_err(|error| error.to_string())?
            .with_public_research_from_policy(policy.compiled());
        if retained.member() != projection.member()
            || retained.server() != projection.server()
            || !prepared.artifact_directory.is_absolute()
        {
            return Err("resource result differs from its retained handoff".into());
        }
        let resources = PreparedApplicationResources {
            artifact_directory: prepared.artifact_directory,
            handoff: SelectedHostArtifact { path: handoff_path, sha256: hash(&bytes) },
        };
        let resource = Arc::new(super::resource::SetupResourceSelection::from_preparation(
            &input, source_helper_member.clone(), resources.clone(), projection.server().clone(),
        ));
        let phase = ApplicationSetupCompilation {
            input,
            input_sha256: self.input_sha256,
            definition,
            source_helper_member,
            source_helper_roster,
            server: projection.server().clone(),
            resources,
            resource,
            policy,
            policy_bytes: self.policy_bytes,
            captured: Mutex::new(BTreeMap::new()),
            final_authorities: OnceLock::new(),
        };
        phase.project_roster(&phase.input.roster)?;
        Ok(phase)
    }
}

struct CapturedMember {
    // Set only by the actual native preparation hook, never by input JSON.
    session: Option<meerkat_core::SessionId>,
    member: CompiledNativeMember,
    surface: PreparedToolSurface,
    selection: ToolSurfaceSelection,
}

/// Configuration and collected immutable compiler outputs, not a runtime or an
/// activation owner. The existing producer uses this exact hook and registry.
/// The schema4 driver holds publication until every native finish and this
/// bundle completion have succeeded. Later registrations use the installed hook.
pub struct ApplicationSetupCompilation {
    input: ApplicationSetupCompilationInput,
    input_sha256: String,
    definition: MobDefinition,
    source_helper_member: MobMemberBinding,
    source_helper_roster: DurableAgentSpec,
    server: McpServerConfig,
    resources: PreparedApplicationResources,
    policy: PreparedInstalledToolPolicy,
    policy_bytes: Vec<u8>,
    captured: Mutex<BTreeMap<usize, CapturedMember>>,
    final_authorities: OnceLock<FinalAuthoritySelection>,
    resource: Arc<super::resource::SetupResourceSelection>,
}

/// One final artifact projection from the existing preparation owner. The
/// initial compiler input remains the immutable resource/destination selection;
/// late grants are never read from that early capture during bundle creation.
struct FinalAuthoritySelection {
    owner: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
    helper: InstalledSetupSelection,
}

impl ApplicationSetupCompilation {
    /// Retain the owner returned by this same ordinary resource preparation,
    /// before sharing this compiler with native registration. The returned
    /// factory can be installed using Core's existing McpAttemptGuards API.
    /// This does not configure a gateway or authorize a successor by itself.
    #[cfg(all(unix, feature = "console"))]
    pub fn retain_local_round_owner(
        &mut self, owner: crate::resources::LocalRoundOwner,
    ) -> Result<Arc<crate::resources::LocalRoundAttempts>, (crate::resources::LocalRoundOwner, String)> {
        self.resource.retain_local_round(owner)
    }
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }
    pub fn helper(&self) -> &InstalledSetupSelection {
        &self.input.helper
    }
    pub(crate) fn project_policy(&self, mut selected: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy)
        -> Result<meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy, String>
    {
        let mut expected = self.input.members.clone();
        let helper = expected.iter_mut().find(|member| member.member == self.input.helper.member)
            .ok_or("selected helper is missing")?;
        helper.member = self.source_helper_member.clone();
        if selected.native_host.members != expected || selected.native_host.realm.as_str() != self.input.realm.realm_id {
            return Err("early host selection differs from the actual compilation input".into());
        }
        selected.native_host.members = self.input.members.clone();
        for channel in &mut selected.channels {
            if channel.member == self.source_helper_member { channel.member = self.input.helper.member.clone(); }
        }
        for route in &mut selected.native_host.routes {
            if let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { member, .. } = &mut route.origin {
                if member == &self.source_helper_member { *member = self.input.helper.member.clone(); }
            }
        }
        selected.validate().map_err(|error| error.to_string())?;
        Ok(selected)
    }
    /// Existing preparation paths already have their final helper selection.
    pub(crate) fn retain_owner(&self, selected: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy) -> Result<(), String> {
        self.retain_authorities(selected, self.input.helper.clone())
    }

    /// Freeze the final owner and helper together after actual held route
    /// observation and native issuance/recovery. Member destinations, helper
    /// identity and every existing operation selection remain unchanged. The
    /// helper-access entry below admits only the separately requested additions.
    pub(crate) fn retain_authorities(
        &self, selected: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
        helper: InstalledSetupSelection,
    ) -> Result<(), String> {
        self.retain_authorities_with_helper_mcp(selected, helper, None)
    }

    /// The preparation issuer may insert only the explicitly requested bundled
    /// server target. All early base/target selections remain byte-identical;
    /// the compiler does not derive or authorize any native grant itself.
    pub(crate) fn retain_authorities_with_helper_mcp(
        &self, selected: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
        helper: InstalledSetupSelection,
        request: Option<&toolkit_mobkit::installed_host::preparation::PreparedHelperMcpRequest>,
    ) -> Result<(), String> {
        self.retain_authorities_with_helper_access(selected, helper, request, None)
    }

    /// Retain only the separately requested catalog base and MCP target. An
    /// occupied base is never replaced, and unrelated targets remain exact.
    pub(crate) fn retain_authorities_with_helper_access(
        &self, selected: meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
        helper: InstalledSetupSelection,
        mcp: Option<&toolkit_mobkit::installed_host::preparation::PreparedHelperMcpRequest>,
        catalog: Option<&toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogRequest>,
    ) -> Result<(), String> {
        selected.validate().map_err(|error| error.to_string())?;
        if selected.native_host.members.len() != self.input.members.len()
            || selected.native_host.realm.as_str() != self.input.realm.realm_id
        { return Err("resolved owner differs from prepared members".into()); }
        for (actual, early) in selected.native_host.members.iter().zip(&self.input.members) {
            let mut unchanged = actual.clone();
            if early.setup_capability.is_some() && actual.setup_capability != early.setup_capability {
                return Err("resolved owner replaces an existing setup capability".into());
            }
            unchanged.setup_capability = early.setup_capability.clone();
            if &unchanged != early { return Err("resolved owner changes a prepared destination".into()); }
        }
        let early = &self.input.helper;
        if (!early.controller_parent.is_empty() && helper.controller_parent != early.controller_parent)
            || (early.report_capability.is_some() && helper.report_capability != early.report_capability)
            || (early.catalog_capability.is_some() && helper.catalog_capability != early.catalog_capability)
        { return Err("resolved helper replaces an existing parent".into()); }
        let mut unchanged = helper.clone();
        unchanged.controller_parent = early.controller_parent.clone();
        unchanged.report_capability = early.report_capability.clone();
        if helper.catalog_capability != early.catalog_capability {
            use toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogScope;
            let request = catalog.ok_or("unrequested helper catalog successor parent")?;
            let added = helper.catalog_capability.as_ref().ok_or("resolved helper drops catalog parent")?;
            let operator = selected.native_host.remote_operator.as_ref().ok_or("missing helper operator")?;
            if request.scope != PreparedHelperCatalogScope::CurrentHelperSuccessors
                || !matches!(early.operation_basis, meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages { base: None, .. })
                || added.parent.len() != 1 || added.parent[0].validate().is_err()
                || added.parent[0].grant_id != request.issuance_id || &added.parent[0].root_authority != operator
            { return Err("resolved helper catalog parent differs from the retained request".into()); }
            unchanged.catalog_capability = early.catalog_capability.clone();
        }
        if helper.operation_basis != early.operation_basis {
            self.validate_helper_access_insertion(&selected, early, &helper, mcp, catalog)?;
            unchanged.operation_basis = early.operation_basis.clone();
        }
        if unchanged != *early { return Err("resolved helper changes the prepared selection".into()); }
        self.final_authorities.set(FinalAuthoritySelection { owner: selected, helper })
            .map_err(|_| "resolved owner already retained".into())
    }
    fn validate_helper_access_insertion(
        &self, selected: &meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy,
        early: &InstalledSetupSelection, helper: &InstalledSetupSelection,
        mcp: Option<&toolkit_mobkit::installed_host::preparation::PreparedHelperMcpRequest>,
        catalog: Option<&toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogRequest>,
    ) -> Result<(), String> {
        use meerkat_authorization_contracts::work_association::{GrantTarget, WorkAuthorityBasis};
        let operator = selected.native_host.remote_operator.as_ref().ok_or("missing helper operator")?;
        let target = GrantTarget::McpServer { server: self.server().name.clone() };
        let (WorkAuthorityBasis::TargetedGrantLineages { base: old_base, targets: old },
            WorkAuthorityBasis::TargetedGrantLineages { base: new_base, targets: new }) =
            (&early.operation_basis, &helper.operation_basis) else {
                return Err("helper operation authority changed selection kind".into());
            };
        if old_base != new_base {
            let request = catalog.ok_or("unrequested helper catalog base")?;
            if request.scope != toolkit_mobkit::installed_host::preparation::PreparedHelperCatalogScope::ExactSession {
                return Err("successor catalog parent is not an executable base".into());
            }
            let added = new_base.as_ref().ok_or("resolved helper drops its base")?;
            if old_base.is_some() || added.len() != 1 || added[0].validate().is_err()
                || added[0].grant_id != request.issuance_id || &added[0].root_authority != operator
            { return Err("resolved helper replaces or widens its selected base".into()); }
        }
        if old == new { return Ok(()); }
        let request = mcp.ok_or("unrequested helper operation target")?;
        if old.iter().any(|row| row.target == target) || new.len() != old.len() + 1 {
            return Err("resolved helper replaces existing operation authority".into());
        }
        let mut unchanged = new.clone();
        let at = unchanged.binary_search_by(|row| row.target.cmp(&target))
            .map_err(|_| "missing exact bundled helper operation target")?;
        let added = unchanged.remove(at);
        if unchanged != *old || added.lineage.len() != 1
            || added.lineage[0].validate().is_err() || added.lineage[0].grant_id != request.issuance_id
            || &added.lineage[0].root_authority != operator
        { return Err("resolved helper operation target differs from the retained request".into()); }
        Ok(())
    }

    pub(crate) fn server(&self) -> &McpServerConfig { self.resource.server() }
    pub(crate) fn resource_selection(&self) -> Arc<super::resource::SetupResourceSelection> { self.resource.clone() }
    pub(crate) fn selected_destination(&self, identity: &MemberRuntimeBuildIdentity,
        destination: &MemberDestinationSelection) -> Result<(), MobError>
    {
        self.resource.selected_destination(identity, destination)
    }
    pub(crate) fn helper_projection(&self) -> Result<PreparedSetupHelper, String> {
        self.resource.helper_projection()
    }

    /// Driver composition reuses the exact policy-bound resource selected
    /// before the reserved definition was captured. It cannot expand it later.
    pub(crate) fn prepare_helper_bootstrap(
        &self, spec: meerkat_mobkit::mob_handle_runtime::MobBootstrapSpec,
    ) -> Result<meerkat_mobkit::mob_handle_runtime::MobBootstrapSpec, String> {
        let helper = self.helper_projection()?
            .with_public_research_from_policy(self.policy.compiled());
        if helper.server() != &self.server {
            return Err("prepared helper ceiling differs from its compiled policy".into());
        }
        helper.compose(spec).map_err(|error| error.to_string())
    }
    pub fn registry(&self) -> Arc<ToolConsequencePolicyRegistry> {
        self.policy.registry()
    }

    /// Retain the same policy and registry for this process's installed-host
    /// adoption after native finish, bundle completion and host selection.
    /// This clone does not establish any of those completion conditions.
    pub fn prepared_tool_policy(&self) -> PreparedInstalledToolPolicy {
        self.policy.clone()
    }

    /// Apply to every actual live provider result, never replace its provider.
    pub fn project_roster(
        &self,
        current: &[DurableAgentSpec],
    ) -> Result<Vec<DurableAgentSpec>, String> {
        project_roster_selection(
            &self.source_helper_member,
            &self.input.helper.member,
            &self.source_helper_roster,
            self.input.helper_profile,
            current,
        )
    }

    fn selected(&self, identity: &MemberRuntimeBuildIdentity) -> Result<usize, String> {
        let expected_realm = meerkat_core::mob_realm_id(&self.input.helper.member.mob_id)
            .map_err(|error| error.to_string())?;
        if identity.realm_id() != &expected_realm {
            return Err("prepared native member belongs to another realm".into());
        }
        self.input
            .members
            .iter()
            .position(|row| &row.member == identity.member())
            .ok_or_else(|| "prepared native member is not selected by this compiler".into())
    }

    fn destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, String> {
        let row = &self.input.members[self.selected(identity)?];
        Ok(MemberDestinationSelection {
            logical_owner: row.logical_owner.clone(),
            context: identity.session_document().clone(),
            context_generation: row.context_generation,
            audience: row.audience.clone(),
        })
    }

    #[cfg(test)]
    fn capture(&self, index: usize, surface: &PreparedToolSurface) -> Result<ToolSurfaceSelection, String> {
        // Inert compiler fixtures have no native session observation.
        self.capture_selected(index, surface, None)
    }

    fn capture_selected(
        &self, index: usize, surface: &PreparedToolSurface,
        session: Option<meerkat_core::SessionId>,
    ) -> Result<ToolSurfaceSelection, String> {
        let row = &self.input.members[index];
        let identities = full_identities(surface)?;
        if row.member == self.input.helper.member
            && !identities.contains(
                &toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity(),
            )
        {
            return Err(
                "actual prepared helper is missing its exact setup_report definition".into(),
            );
        }
        let selection = self
            .policy
            .select(&row.member, surface)
            .map_err(|error| error.to_string())?;
        let bytes =
            toolkit_package::canonical_json(&identities).map_err(|error| error.to_string())?;
        if bytes.len() > 256 * 1024 {
            return Err("member surface exceeds the artifact limit".into());
        }
        let name = member_surface_name(&row.member)?;
        let mut captured = self
            .captured
            .lock()
            .map_err(|_| "compiler output owner unavailable")?;
        if captured.contains_key(&index) {
            return Err("a final member was already prepared by this compiler".into());
        }
        // Append-only facts. No overwrite or retry can transform an old/native
        // failed preparation into the output of another construction.
        write_new(&self.resources.artifact_directory.join(&name), &bytes)?;
        captured.insert(
            index,
            CapturedMember {
                session,
                member: CompiledNativeMember {
                    member: row.member.clone(),
                    logical_owner: row.logical_owner.clone(),
                    context_generation: row.context_generation,
                    audience: row.audience.clone(),
                    setup_capability: row.setup_capability.clone(),
                    initial_surface: CompiledArtifactReference {
                        name,
                        sha256: hash(&bytes),
                    },
                },
                surface: surface.clone(),
                selection: selection.clone(),
            },
        );
        Ok(selection)
    }

    /// Compare the existing native held pass's actual finished inventory with
    /// this compiler's exact returned selections. Value equality alone is not
    /// evidence of native finish: the production caller supplies these values
    /// from the same native held completion while publication remains withheld.
    pub fn validate_finished<'a>(
        &self,
        finished: impl IntoIterator<Item = (&'a meerkat_mobkit::identity_first::AgentIdentity, &'a ToolSurfaceSelection)>,
    ) -> Result<(), String> {
        let captured = self.captured.lock().map_err(|_| "compiler output owner unavailable")?;
        let mut observed = BTreeSet::new();
        for (identity, selection) in finished {
            let index = self.input.roster.iter().position(|spec| &spec.identity == identity)
                .ok_or("finished native identity is absent from the selected roster")?;
            if !observed.insert(index) {
                return Err("finished native inventory repeats an identity".into());
            }
            let member = self.input.members.iter().position(|member| {
                matching_roster(&self.input.roster, &member.member).ok() == Some(index)
            }).ok_or("finished native identity has no selected member")?;
            if captured.get(&member).map(|output| &output.selection) != Some(selection) {
                return Err("native finish differs from this construction's captured selection".into());
            }
        }
        if observed.len() != self.input.roster.len() || captured.len() != self.input.members.len() {
            return Err("native finish does not cover this compiler's complete inventory".into());
        }
        Ok(())
    }

    /// Read only the session captured by this helper's actual native build,
    /// after the complete held inventory has matched its finished surfaces.
    pub(crate) fn held_helper_session<'a>(
        &self,
        finished: impl IntoIterator<Item = (&'a meerkat_mobkit::identity_first::AgentIdentity, &'a ToolSurfaceSelection)>,
    ) -> Result<meerkat_core::SessionId, String> {
        self.validate_finished(finished)?;
        let index = self.input.members.iter().position(|row| row.member == self.input.helper.member)
            .ok_or("selected helper is absent")?;
        self.captured.lock().map_err(|_| "compiler output owner unavailable")?
            .get(&index).and_then(|member| member.session.clone())
            .ok_or_else(|| "actual native helper session was not captured".into())
    }

    /// The exact roster identity for a selected, projected member. The Driver
    /// uses this same association after validating the full finished inventory.
    /// A matching role or an unselected member alias is insufficient.
    pub(crate) fn held_member_identity(&self, member: &MobMemberBinding)
        -> Result<&meerkat_mobkit::identity_first::AgentIdentity, String>
    {
        if !self.input.members.iter().any(|row| &row.member == member) {
            return Err("controller target is not a selected prepared member".into());
        }
        Ok(&self.input.roster[matching_roster(&self.input.roster, member)?].identity)
    }

    /// Called only by the existing native producer after all captured native
    /// preparations have finished successfully, while it still withholds
    /// publication. Callback count is not evidence of that condition. This
    /// method only completes compiler artifacts and never activates them.
    pub fn complete_bundle(
        &self,
        output: &Path,
    ) -> Result<(PreparedApplicationSetup, SelectedHostArtifact), String> {
        if !output.is_absolute() {
            return Err("compiler output must be absolute".into());
        }
        let authorities = self.final_authorities.get()
            .ok_or("schema4 authority selections were not finalized")?;
        let parent = output
            .parent()
            .ok_or("compiler output has no parent")?
            .canonicalize()
            .map_err(|error| error.to_string())?;
        let output = new_child(&parent, output)?;
        let captured = self
            .captured
            .lock()
            .map_err(|_| "compiler output owner unavailable")?;
        if captured.len() != self.input.members.len() {
            return Err("application member artifact inventory is incomplete".into());
        }
        let mut native_members = Vec::with_capacity(self.input.members.len());
        for index in 0..self.input.members.len() {
            let mut member = captured.get(&index)
                .ok_or("selected member has no native surface output")?.member.clone();
            // Capture proves the actual surface. Final authority comes from
            // the same preparation owner's resolved selection, never the
            // setup_capability clone taken before held route observation.
            member.setup_capability = authorities.owner.native_host.members[index].setup_capability.clone();
            native_members.push(member);
        }
        let setup = PreparedApplicationSetup {
            schema: 4,
            native_owner: Some(authorities.owner.clone()),
            input_sha256: self.input_sha256.clone(),
            realm: self.input.realm.clone(),
            definition_source: self.input.definition.clone(),
            definition: self.definition.clone(),
            source_helper_roster: self.source_helper_roster.clone(),
            native_members,
            source_helper_member: self.source_helper_member.clone(),
            helper: authorities.helper.clone(),
            helper_source_surface: None,
            helper_tools: self.input.helper_tools,
            helper_profile: self.input.helper_profile,
            server: self.server.clone(),
            resources: self.resources.clone(),
            application_tool_policy: CompiledArtifactReference {
                name: "application-tool-policy.json".into(),
                sha256: hash(&self.policy_bytes),
            },
        };
        setup.helper_projection_for_policy(self.policy.compiled())?;
        let manifest = write_bundle(&output, &setup, &self.policy_bytes)?;
        Ok((setup, manifest))
    }
}

#[async_trait]
impl MemberRuntimeRegistrationHook for ApplicationSetupCompilation {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.destination(identity).map_err(MobError::WiringError)
    }
    async fn expected_destination(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        Err(MobError::WiringError(
            "schema4 producer does not recover already published members".into(),
        ))
    }
    async fn prepare_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, MobError> {
        let index = self.selected(identity).map_err(MobError::WiringError)?;
        let session = meerkat_core::SessionId::parse(identity.session_document().as_str())
            .map_err(|error| MobError::WiringError(error.to_string()))?;
        self.capture_selected(index, surface, Some(session)).map_err(MobError::WiringError)
    }
    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        let index = self.selected(identity).map_err(MobError::WiringError)?;
        let captured = self
            .captured
            .lock()
            .map_err(|_| MobError::WiringError("compiler output owner unavailable".into()))?;
        let output = captured.get(&index).ok_or_else(|| {
            MobError::WiringError("member was not prepared by this compiler".into())
        })?;
        if output.session.as_ref().is_some_and(|session| session.to_string() != identity.session_document().as_str()) {
            return Err(MobError::WiringError("finished member has a different captured native session".into()));
        }
        if surface
            .iter()
            .any(|tool| !output.surface.contains(&tool.identity()))
        {
            return Err(MobError::WiringError(
                "finished member exceeds its actual prepared surface".into(),
            ));
        }
        Ok(())
    }
}

fn member_surface_name(member: &MobMemberBinding) -> Result<String, String> {
    let bytes = toolkit_package::canonical_json(member).map_err(|error| error.to_string())?;
    Ok(format!("member-surface-{}.json", hash(&bytes)))
}

fn full_identities(surface: &PreparedToolSurface) -> Result<Vec<ToolIdentity>, String> {
    let mut identities = surface.tools.clone();
    if identities.iter().enumerate().any(|(index, identity)| {
        identities[..index]
            .iter()
            .any(|prior| prior.name == identity.name)
    }) || surface
        .projected
        .iter()
        .any(|identity| !surface.tools.contains(identity))
    {
        return Err("native prepared surface has inconsistent full identities".into());
    }
    if let PreparedToolCatalog::Complete(entries) = &surface.catalog {
        let mut catalog_names = BTreeSet::new();
        for entry in entries {
            if !catalog_names.insert(entry.identity.name.clone()) {
                return Err("native complete catalog repeats a name".into());
            }
            match identities
                .iter()
                .find(|identity| identity.name == entry.identity.name)
            {
                Some(identity) if identity != &entry.identity => {
                    return Err("native catalog conflicts with full tool provenance".into());
                }
                Some(_) => (),
                None => identities.push(entry.identity.clone()),
            }
        }
    }
    Ok(identities)
}

/// One final inventory owns every artifact reference. Paths are derived from it
/// within the existing retained resource directory; no second inventory exists.
pub(super) fn validate_final_artifacts(
    setup: &PreparedApplicationSetup,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    if setup.schema != 4 || setup.helper_source_surface.is_some() || setup.native_members.is_empty()
    {
        return Err("invalid final schema4 artifact selection".into());
    }
    let mut artifacts = Vec::new();
    let mut members = BTreeSet::new();
    for row in &setup.native_members {
        row.initial_surface
            .validate()
            .map_err(|error| error.to_string())?;
        if !members.insert((&row.member.mob_id, &row.member.member))
            || row.initial_surface.name != member_surface_name(&row.member)?
        {
            return Err("final member surface association is duplicate or changed".into());
        }
        let bytes = read_regular_artifact(
            &setup
                .resources
                .artifact_directory
                .join(&row.initial_surface.name),
        )?;
        let identities: Vec<ToolIdentity> =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if hash(&bytes) != row.initial_surface.sha256
            || toolkit_package::canonical_json(&identities).map_err(|error| error.to_string())?
                != bytes
            || identities.iter().enumerate().any(|(index, identity)| {
                identities[..index]
                    .iter()
                    .any(|prior| prior.name == identity.name)
            })
        {
            return Err("final member surface differs from its canonical compiler artifact".into());
        }
        if row.member == setup.helper.member
            && !identities.contains(
                &toolkit_mobkit::installed_host::acquisition::setup_report_definition().identity(),
            )
        {
            return Err("final helper has no exact setup_report association".into());
        }
        artifacts.push((row.initial_surface.name.clone(), bytes));
    }
    Ok(artifacts)
}

#[cfg(test)]
pub(crate) mod tests;
