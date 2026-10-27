//! Prepared setup MCP configuration on one host-selected native helper.
//!
//! The selected profile is composed before policy and roster compilation,
//! so a compatible native identity compiler can retain the exact MCP config.
//! A spawn customizer alone is insufficient: native IdentityReconcile uses its
//! sealed material. The selected native release must preserve McpServerAccess
//! in that carrier; a release that rejects it cannot use this identity path.

use std::sync::Arc;

use meerkat_core::{
    CompiledApplicationToolPolicy, CompiledMemberToolAction, McpServerConfig, MobMemberBinding,
    mcp_config::McpServerAccess,
};
use meerkat_mob::{
    MobDefinition, MobError, MobId, Profile, ProfileBinding, ProfileName,
    SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec,
};
use meerkat_mobkit::MobBootstrapSpec;
use sha2::{Digest, Sha256};

use super::{ACQUISITION_TOOLS, PREPARATION_TOOLS, PreparedSetupConnection, SERVER_NAME};
use crate::ConnectError;

const HELPER_ROLE_PREFIX: &str = "toolkit-setup-";

/// The prepared server's raw MCP operations, selected by the application.
/// These are ceilings, not grants or a replacement for native operation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupHelperTools {
    /// Catalog, package inspection, preparation and local setup job controls.
    Preparation,
    /// Also materialize and inspect real inert Python wheel artifacts. This
    /// does not activate an acquired plugin or make its operations callable.
    InertAcquisition,
    /// Also expose checked software installation. The application's gateway
    /// must compose its existing exact-member software review owner; this
    /// ceiling grants neither native operation authority nor account access.
    CheckedInstallation,
}

/// Pure configuration for a helper in the application's existing mob.
///
/// Obtain the connection from `InstalledResources::prepare`. Then:
/// 1. Construct this projection from the host-selected helper identity and its
///    existing inline research profile.
/// 2. Compose the definition and use `member()` for BOTH compiled native policy
///    and the identity roster/declaration, before either is installed or sealed.
/// 3. Compose the bootstrap spec before starting the existing runtime.
///
/// Dedicated composition leaves the source profile untouched. In-place
/// composition preserves the application's exclusive role key. Both preserve
/// research tools, model settings, skills and restrictions, adding only the
/// selected setup server and result bundle. The role name is a stable configuration key, never
/// an authorization proof. All effects still require native current authority.
///
/// A restart must supply the same selected config, including its retained
/// artifact path. A conflicting profile refuses. This does not migrate or edit
/// an already sealed identity; native reconciliation uses its original material.
/// Native portable-profile support for the server access ceiling is required;
/// this constructor is not evidence of successful identity materialization.
#[derive(Clone)]
pub struct PreparedSetupHelper {
    source_member: MobMemberBinding,
    member: MobMemberBinding,
    profile_name: ProfileName,
    server: McpServerConfig,
    in_place: bool,
    public_research_ceiling: bool,
}

impl PreparedSetupConnection {
    /// Reserve one dedicated profile for the exact host-selected helper.
    /// `source_member.role` names its existing inline research profile. Use the
    /// returned `member()` rather than that source binding for new policy and
    /// identity compilation; its mob and identity stay the same.
    pub fn for_mobkit_helper(
        self,
        source_member: MobMemberBinding,
        tools: SetupHelperTools,
    ) -> Result<PreparedSetupHelper, ConnectError> {
        self.helper_projection(source_member, tools, false)
    }

    /// Add the prepared server to an application's exclusive helper role,
    /// preserving its exact binding and all existing role-keyed grants/wiring.
    /// The application must validate every current roster result for exclusive
    /// use of this role before native compilation/reconciliation. The native
    /// spawn guard also rejects any other identity requesting the role.
    pub fn for_mobkit_helper_in_place(
        self,
        source_member: MobMemberBinding,
        tools: SetupHelperTools,
    ) -> Result<PreparedSetupHelper, ConnectError> {
        self.helper_projection(source_member, tools, true)
    }

    fn helper_projection(
        self,
        source_member: MobMemberBinding,
        tools: SetupHelperTools,
        in_place: bool,
    ) -> Result<PreparedSetupHelper, ConnectError> {
        source_member
            .comms_name()
            .map_err(|error| ConnectError::native("validate selected setup helper", error))?;
        if source_member.role.starts_with(HELPER_ROLE_PREFIX) {
            return Err(invalid(
                "Select the original helper research profile, not a reserved setup role",
            ));
        }
        // The typed JSON tuple makes the generated key unambiguous.
        // No current grant, custody or member existence is inferred from it.
        let encoded = serde_json::to_vec(&(
            "meerkat-connect/setup-helper-profile/v1",
            &source_member.mob_id,
            &source_member.role,
            &source_member.member,
        ))
        .map_err(|error| ConnectError::native("encode selected helper profile key", error))?;
        let role = if in_place {
            source_member.role.clone()
        } else {
            format!("{HELPER_ROLE_PREFIX}{:x}", Sha256::digest(encoded))
        };
        let member = MobMemberBinding {
            mob_id: source_member.mob_id.clone(),
            role: role.clone(),
            member: source_member.member.clone(),
        };
        member
            .comms_name()
            .map_err(|error| ConnectError::native("validate composed setup helper", error))?;
        let mut server = self.into_server();
        let mut operations = PREPARATION_TOOLS.to_vec();
        if matches!(tools, SetupHelperTools::InertAcquisition | SetupHelperTools::CheckedInstallation) {
            operations.extend_from_slice(ACQUISITION_TOOLS);
        }
        if tools == SetupHelperTools::CheckedInstallation {
            operations.extend_from_slice(crate::service_install::INSTALL_TOOLS);
            operations.push(crate::service_install::SELECTED_CHECK);
        }
        server.access = Some(McpServerAccess::allowing(operations));
        Ok(PreparedSetupHelper {
            source_member,
            member,
            profile_name: ProfileName::from(role),
            server,
            in_place,
            public_research_ceiling: tools != SetupHelperTools::Preparation,
        })
    }
}

impl PreparedSetupHelper {
    /// Select only explicitly declared anonymous research operations on the
    /// existing setup MCP server. No stock builtin or nested model is enabled.
    /// This finite ceiling is not authority: native target lineage and the
    /// compiled application policy still admit each call independently.
    pub fn with_public_research_from_policy(
        self,
        policy: &CompiledApplicationToolPolicy,
    ) -> Self {
        let member_identity = self.member.member.clone();
        self.with_public_research_operations(|name| policy.members.iter().any(|member| {
            member.member_identity == member_identity
                && member.grants.iter().any(|grant| {
                    grant.tool_name == name && grant.action == CompiledMemberToolAction::Invoke
                })
        }))
    }

    /// Reconstruct only the two optional ceiling entries from the already
    /// retained application configuration. Callers still compare the complete
    /// member, server and definition. This does not establish policy authority.
    pub(crate) fn with_retained_public_research(self, server: &McpServerConfig) -> Self {
        self.with_public_research_operations(|name| server.access.as_ref()
            .is_some_and(|access| access.allowed_operations.contains(name)))
    }

    fn with_public_research_operations(mut self, permits: impl Fn(&str) -> bool) -> Self {
        if self.public_research_ceiling {
            let access = self.server.access.as_mut().expect("prepared helper MCP ceiling");
            for name in ["setup_search_github", "setup_read_github"] {
                access.allowed_operations.remove(name);
                if permits(name) {
                    access.allowed_operations.insert(name.into());
                }
            }
        }
        self
    }

    pub fn source_member(&self) -> &MobMemberBinding {
        &self.source_member
    }

    /// Exact resulting binding for the compiler, roster and native policy.
    pub fn member(&self) -> &MobMemberBinding {
        &self.member
    }

    pub fn profile_name(&self) -> &ProfileName {
        &self.profile_name
    }

    /// Exact prepared destination for the host's native operation policy join.
    /// Neither this config nor the profile key grants permission to call it.
    pub fn server(&self) -> &McpServerConfig {
        &self.server
    }

    /// Compose the selected inline profile before native policy/identity sealing.
    ///
    /// The host must supply an inline source profile resolved by its existing
    /// profile owner. This function does not resolve realm references. Matching
    /// restart composition is idempotent; any existing conflicting role refuses
    /// before the definition changes.
    pub fn compose_definition(&self, definition: &mut MobDefinition) -> Result<(), ConnectError> {
        if definition.id.as_str() != self.member.mob_id {
            return Err(invalid("Prepared setup helper belongs to a different mob"));
        }
        let source = definition
            .profiles
            .get(&ProfileName::from(self.source_member.role.clone()))
            .and_then(ProfileBinding::as_inline)
            .ok_or_else(|| {
                invalid("Prepared setup helper requires its selected inline research profile")
            })?;
        if self.in_place {
            let mut profile = source.clone();
            retain_selected_server(&mut profile, &self.server)
                .map_err(|error| ConnectError::native("compose application helper role", error))?;
            definition.profiles.insert(
                self.profile_name.clone(),
                ProfileBinding::Inline(Box::new(profile)),
            );
            return Ok(());
        }
        if source.tools.mcp.iter().any(|name| name == SERVER_NAME)
            || source
                .tools
                .mcp_servers
                .iter()
                .any(|server| server.name == SERVER_NAME)
        {
            return Err(invalid(
                "The source research profile already selects the reserved setup server",
            ));
        }
        let mut profile = source.clone();
        retain_selected_server(&mut profile, &self.server)
            .map_err(|error| ConnectError::native("compose setup helper tools", error))?;
        let desired = ProfileBinding::Inline(Box::new(profile));
        match definition.profiles.get(&self.profile_name) {
            Some(existing) if existing != &desired => Err(invalid(
                "The reserved setup helper profile conflicts with this exact preparation",
            )),
            Some(_) => Ok(()),
            None => {
                definition
                    .profiles
                    .insert(self.profile_name.clone(), desired);
                Ok(())
            }
        }
    }

    /// Compose the existing native bootstrap, retaining all earlier customizers.
    /// This performs no runtime start, member spawn or identity migration.
    pub fn compose(&self, mut spec: MobBootstrapSpec) -> Result<MobBootstrapSpec, ConnectError> {
        self.compose_definition(&mut spec.definition)?;
        Ok(spec.with_spawn_member_customizer(Arc::new(SelectedHelperCustomizer(self.clone()))))
    }
}

fn invalid(message: &str) -> ConnectError {
    ConnectError::InvalidInput(message.into())
}

struct SelectedHelperCustomizer(PreparedSetupHelper);

impl SelectedHelperCustomizer {
    fn apply(
        &self,
        mob: &MobId,
        requested_profile: &ProfileName,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        let selected = &self.0;
        let exact_identity = mob.as_str() == selected.member.mob_id
            && spec.identity.as_str() == selected.member.member;
        if spec.role_name != selected.profile_name {
            if exact_identity {
                return Err(MobError::WiringError(
                    "selected setup helper must use its composed profile before native sealing"
                        .into(),
                ));
            }
            // Ordinary users of the source research profile keep their exact
            // existing inputs, including launch policy and customizer overlays.
            return Ok(());
        }
        if !exact_identity || requested_profile != &selected.profile_name {
            return Err(MobError::WiringError(
                "reserved setup profile does not match the selected native member".into(),
            ));
        }
        // An earlier host customizer may have resolved a per-member profile.
        // Preserve that profile and its research tools, adding only our exact
        // server. Never overwrite the spawn's global tool access policy.
        if let Some(profile) = &mut spec.override_profile {
            retain_selected_server(profile, &selected.server)?;
        }
        Ok(())
    }
}

fn retain_selected_server(
    profile: &mut Profile,
    selected: &McpServerConfig,
) -> Result<(), MobError> {
    if profile.tools.mcp.iter().any(|name| name == SERVER_NAME) {
        return Err(MobError::WiringError(
            "setup server also selected through a named MCP provider".into(),
        ));
    }
    let mut existing = profile
        .tools
        .mcp_servers
        .iter()
        .filter(|server| server.name == SERVER_NAME);
    match (existing.next(), existing.next()) {
        (None, None) => {
            profile.tools.mcp_servers.push(selected.clone());
        }
        (Some(server), None) if server == selected => (),
        _ => {
            return Err(MobError::WiringError(
                "setup helper override conflicts with its prepared MCP server".into(),
            ));
        }
    }
    let results = meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE;
    if !profile
        .tools
        .rust_bundles
        .iter()
        .any(|name| name == results)
    {
        profile.tools.rust_bundles.push(results.into());
    }
    Ok(())
}

impl SpawnMemberCustomizer for SelectedHelperCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.apply(&ctx.mob_id, &ctx.requested_profile, spec)
    }
}

#[cfg(test)]
mod tests;
