//! One native source/grant owner retained from preparation through adoption.
//!
//! The trusted application supplies the early facts through its inherited
//! channel. No activation is fabricated. Native invocation remains unavailable
//! until the actual final lifetime and installed registration are adopted.

use super::*;
#[cfg(test)]
use super::controller_first_input_fixture::PrepareOriginalSource;
use meerkat_toolkit::host_lifetime::policy::PreparedLocalPolicy;

/// The actual canonical stores used by this preparation, not a snapshot of
/// them. Dropping a failed preparation preserves durable native state.
pub struct PreparedInstalledHost {
    deployment_root: PathBuf,
    authorities: PreparedAuthorityState,
    originals: Arc<PrivateOriginals>,
    source: Arc<ToolkitOriginalOwner>,
    store: Arc<SqliteStore>,
    source_scope: StoreScope,
    tool_policy: PreparedInstalledToolPolicy,
    policy: Arc<PreparedApplicationPolicy>,
    configuration: std::sync::Mutex<Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>>,
}

/// Authority selection stays on the same preparation owner until the native
/// held caller supplies its actual controller observations. Finalization is a
/// one-way transition; the selected policy is never replaced after publication.
enum PreparedAuthorityState {
    Pending(PreparedInstallationOwner),
    Finalized(PreparedLocalPolicy),
}

mod authorities;
mod helper_mcp;
mod helper_catalog;
pub use authorities::{PreparationAuthorities, PreparationAuthorityIntent, PreparedControllerIntent,
    PreparedControllerRequest, PreparedControllerTarget, PreparedHelperMcpRequest, PreparedHelperCatalogRequest,
    PreparedHelperCatalogScope};

/// The same native pair opened before configuration has native surface outputs.
/// All optional parent requests are explicit trusted-admin operations, retained
/// by their existing native issuance IDs. No installed lifetime exists yet.
pub struct PreparedInstallationOwner {
    deployment_root: PathBuf,
    selected: PreparedLocalPolicy,
    source: Arc<ToolkitOriginalOwner>,
    store: Arc<SqliteStore>,
    source_scope: StoreScope,
}

impl PreparedInstallationOwner {
    pub fn open(deployment_root: PathBuf, selected: PreparedLocalPolicy,
        startup: operator_setup::OperatorOwnerStartup) -> Result<Self, HostLifetimeError>
    {
        selected.validate()?;
        let deployment_root = deployment_root.canonicalize()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let native = &selected.native_host;
        let operator = native.remote_operator.as_ref();
        let directory = state_directory_in(
            &deployment_root,
            &native.realm,
            if matches!(startup, operator_setup::OperatorOwnerStartup::Initialize { .. }) {
                NativeOwnerStartup::Initialize
            } else {
                NativeOwnerStartup::Restore
            },
        )?;
        let (source, store) = operator_setup::open_owner_pair_in(
            &directory, &native.ingress_domain, operator, startup, true,
        )?;
        let store = Arc::new(store);
        let source_scope = StoreScope {
            realm: native.realm.to_string(),
            owner: selected.deployment.to_string(),
        };
        Ok(Self { deployment_root, selected, source, store, source_scope })
    }

    pub fn selected(&self) -> &PreparedLocalPolicy { &self.selected }

    /// The resource compiler changes only this selected helper's role. Its
    /// member identity, source principals and native parent references remain
    /// those already selected by the same preparation owner.
    pub fn project_helper_member(&mut self, source: &meerkat_core::MobMemberBinding,
        projected: &meerkat_core::MobMemberBinding) -> Result<(), HostLifetimeError>
    {
        if source.mob_id != projected.mob_id || source.member != projected.member {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let member = self.selected.native_host.members.iter_mut().find(|row| &row.member == source)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        member.member = projected.clone();
        for channel in &mut self.selected.channels {
            if &channel.member == source { channel.member = projected.clone(); }
        }
        for route in &mut self.selected.native_host.routes {
            if let meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { member, .. } = &mut route.origin {
                if member == source { *member = projected.clone(); }
            }
        }
        self.selected.validate()
    }

    /// Construct the source and policy participants once, with route access
    /// still unavailable. The same native owner remains inside the prepared
    /// host for authority resolution during the native held completion.
    pub async fn prepare(self, tool_policy: PreparedInstalledToolPolicy)
        -> Result<PreparedInstalledHost, HostLifetimeError>
    {
        self.prepare_selected(tool_policy, None, #[cfg(test)] None).await
    }

    pub async fn prepare_with_application_service_policy(self, tool_policy: PreparedInstalledToolPolicy,
        participant: Arc<dyn crate::private_originals::ApplicationServicePolicy>,
    ) -> Result<PreparedInstalledHost, HostLifetimeError> {
        self.prepare_selected(tool_policy, Some(participant), #[cfg(test)] None).await
    }

    async fn prepare_selected(self, tool_policy: PreparedInstalledToolPolicy,
        participant: Option<Arc<dyn crate::private_originals::ApplicationServicePolicy>>,
        #[cfg(test)] prepare_source: Option<PrepareOriginalSource>,
    ) -> Result<PreparedInstalledHost, HostLifetimeError> {
        let source_store: Arc<dyn toolkit_store::ToolkitStore> = self.store.clone();
        #[cfg(test)]
        let source_store = match prepare_source {
            Some(decorate) => decorate(source_store), None => source_store,
        };
        let originals = Arc::new(PrivateOriginals::prepare(
            self.source_scope.clone(), source_store, self.source.clone(),
        ).await.map_err(|_| HostLifetimeError::InvalidSelection)?);
        // PreparedInstallationOwner::open owns the intact leased pair. This
        // fact survives adoption with this exact source; generic new/prepare
        // never receives it merely by attaching a runtime.
        originals.select_installed_pair_recovery()?;
        let service_policy = tool_policy.bind_service_owner(self.store.clone(), self.source_scope.clone(),
            self.selected.native_host.grant_administrator.clone(), participant).await?;
        originals.bind_service_tool_policy(service_policy)?;
        let policy = Arc::new(PreparedApplicationPolicy::new(originals.clone()));
        let configuration = std::sync::Mutex::new(Some(originals.configuration(policy.clone())));
        Ok(PreparedInstalledHost {
            deployment_root: self.deployment_root.clone(), originals,
            source: self.source.clone(), store: self.store.clone(),
            source_scope: self.source_scope.clone(), tool_policy, policy, configuration,
            authorities: PreparedAuthorityState::Pending(self),
        })
    }

    #[cfg(test)]
    pub(crate) async fn complete_with_source_store(self, tool_policy: PreparedInstalledToolPolicy,
        prepare_source: PrepareOriginalSource,
    ) -> Result<PreparedInstalledHost, HostLifetimeError> {
        let mut prepared = self.prepare_selected(tool_policy, None, Some(prepare_source)).await?;
        prepared.finalize_selection()?;
        Ok(prepared)
    }

    /// Existing callers with already resolved authority selections retain the
    /// same complete preparation path. Deferred held callers use prepare and
    /// finalize_authorities around the actual native controller observation.
    pub async fn complete(self, tool_policy: PreparedInstalledToolPolicy)
        -> Result<PreparedInstalledHost, HostLifetimeError>
    {
        let mut prepared = self.prepare(tool_policy).await?;
        prepared.finalize_selection()?;
        Ok(prepared)
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy;

    pub(super) fn inputs(selection: &HostBootstrapSelection)
        -> (PreparedLocalPolicy, PreparedInstalledToolPolicy)
    {
        let local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let native = local.native_host.as_ref().unwrap();
        let policy = PreparedInstalledToolPolicy::from_canonical(
            &read_bundle_artifact(selection, &native.application_tool_policy).unwrap(),
        ).unwrap();
        (PreparedLocalPolicy {
            deployment: local.deployment.clone(),
            components: local.components.clone(),
            channels: local.channels.clone(),
            native_host: native.preparation(),
        }, policy)
    }

    // These are real canonical-store/adoption component controls. They do not
    // stand in for native held construction, private IPC or live callbacks.
    #[tokio::test]
    async fn pending_routes_refuse_until_the_same_owner_finalizes_once() {
        let (_directory, mut selection) = super::super::tests::fixture();
        super::super::tests::provision_fixture(&mut selection);
        let (selected, policy) = inputs(&selection);
        let owner = PreparedInstallationOwner::open(selection.deployment_root.clone(),
            selected.clone(), operator_setup::OperatorOwnerStartup::Restore).unwrap();
        let mut prepared = owner.prepare(policy).await.unwrap();
        let originals = prepared.originals();
        assert!(matches!(originals.routes(), Err(crate::private_originals::PrivateOriginalError::Unavailable)));
        assert!(matches!(prepared.selected(), Err(HostLifetimeError::Unavailable)));
        // Native construction takes its one configuration before route binding.
        // This control does not construct or report a controller.
        let configuration = prepared.take_configuration().unwrap();
        assert!(prepared.take_configuration().is_err());
        let member = &selected.native_host.members[0];
        let mut helper = acquisition::InstalledSetupSelection {
            catalog_capability: None,
            report_capability: None, member: member.member.clone(),
            ingress_actor: member.logical_owner.clone(), logical_executor: member.logical_owner.clone(),
            represented_subject: None, controller_parent: vec![],
            operation_basis: WorkAuthorityBasis::TargetedGrantLineages { base: None, targets: vec![] },
        };
        let source = prepared.source.clone();
        let store = prepared.store.clone();
        prepared.configure_held_authorities_with_helper_resources(
            &mut helper, &PreparationAuthorities::default(), None, None).unwrap();
        assert!(Arc::ptr_eq(&source, &prepared.source));
        assert!(Arc::ptr_eq(&store, &prepared.store));
        assert!(Arc::ptr_eq(&originals, &prepared.originals()));
        assert!(matches!(originals.routes(), Err(crate::private_originals::PrivateOriginalError::Unavailable)));
        assert!(matches!(prepared.selected(), Err(HostLifetimeError::Unavailable)));
        let finalized = prepared.finalize_authorities(&mut helper, &PreparationAuthorities::default()).unwrap();
        assert_eq!(finalized, selected);
        assert!(Arc::ptr_eq(&originals, &prepared.originals()));
        let routes = originals.routes().unwrap().to_vec();
        assert_eq!(routes.len(), selected.native_host.routes.len());
        assert!(prepared.finalize_authorities(&mut helper, &PreparationAuthorities::default()).is_err());
        assert!(prepared.configure_held_authorities_with_helper_resources(
            &mut helper, &PreparationAuthorities::default(), None, None).is_err());
        assert!(originals.finalize_routes(routes).is_err(), "an installed route set cannot be replaced");
        assert_eq!(prepared.selected().unwrap(), &selected);
        drop(configuration);
    }

    #[tokio::test]
    async fn adoption_refuses_pending_authority_before_opening_a_lifetime() {
        let (_directory, mut selection) = super::super::tests::fixture();
        super::super::tests::provision_fixture(&mut selection);
        let (selected, policy) = inputs(&selection);
        let prepared = PreparedInstallationOwner::open(selection.deployment_root.clone(), selected,
            operator_setup::OperatorOwnerStartup::Restore).unwrap().prepare(policy).await.unwrap();
        assert!(matches!(prepared.adopt(selection), Err(HostLifetimeError::Unavailable)));
    }

    #[tokio::test]
    async fn final_adoption_preserves_both_leased_owners_and_the_prepared_registry() {
        let (_directory, mut selection) = super::super::tests::fixture();
        super::super::tests::provision_fixture(&mut selection);
        let (selected, policy) = inputs(&selection);
        let registry = policy.registry();
        let prepared = PreparedInstalledHost::open(
            selection.deployment_root.clone(), selected,
            operator_setup::OperatorOwnerStartup::Restore, policy,
        ).await.unwrap();
        let originals = prepared.originals();
        let store = prepared.store.clone();
        let source = prepared.source.clone();
        let publication: Arc<dyn toolkit_store::RecordChangePublication> = source.clone();
        assert!(store.record_publication_is(&publication));
        assert!(InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
            .await.is_err(), "preparation retains the actual native owner lease");
        let (_owner, installed) = prepared.adopt(selection.clone()).unwrap();
        assert!(Arc::ptr_eq(&originals, &installed.originals));
        assert!(Arc::ptr_eq(&store, &installed.store));
        assert!(Arc::ptr_eq(&registry, &installed.tool_policy.registry()));
        assert!(installed.store.record_publication_is(&publication));
        assert!(InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore)
            .await.is_err(), "adoption did not release/reopen the canonical owner");
    }

    #[tokio::test]
    async fn changed_final_destination_refuses_without_replacing_the_retained_stores() {
        let (_directory, mut selection) = super::super::tests::fixture();
        super::super::tests::provision_fixture(&mut selection);
        let (selected, policy) = inputs(&selection);
        let prepared = PreparedInstalledHost::open(
            selection.deployment_root.clone(), selected,
            operator_setup::OperatorOwnerStartup::Restore, policy,
        ).await.unwrap();
        let originals = prepared.originals();
        let mut changed: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        changed.native_host.as_mut().unwrap().members[0].context_generation += 1;
        super::super::tests::replace_local(&mut selection, &changed);
        assert!(matches!(prepared.adopt(selection.clone()), Err(HostLifetimeError::InvalidSelection)));
        assert!(InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore)
            .await.is_err(), "the retained source still owns the original store pair");
        drop(originals);
    }
}

// Failure-only local diagnostic. Fixed labels and unit variants contain no owner data.
// Diagnostic I/O cannot change the original refusal or its cleanup path.
fn adoption_refusal(stage: &'static str, error: HostLifetimeError) -> HostLifetimeError {
    use std::io::Write as _;
    let cause = match error {
        HostLifetimeError::InvalidSelection => "InvalidSelection",
        HostLifetimeError::Unavailable => "Unavailable",
        HostLifetimeError::Denied => "Denied",
        HostLifetimeError::WrongOwner => "WrongOwner",
        HostLifetimeError::NotSettled => "NotSettled",
    };
    let _ = writeln!(std::io::stderr().lock(),
        "{{\"event\":\"toolkit_application_adoption_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}");
    error
}

impl PreparedInstalledHost {
    /// Resolve explicit requests only after the caller has joined each route
    /// to the actual held native report. This method does not obtain reports or
    /// treat a configured route key as evidence of a live controller. Failed
    /// partial issuance retains this same owner for exact-ID recovery.
    pub fn finalize_authorities(
        &mut self, helper: &mut acquisition::InstalledSetupSelection,
        requests: &PreparationAuthorities,
    ) -> Result<PreparedLocalPolicy, HostLifetimeError> {
        self.finalize_authorities_with_helper_server(helper, requests, None)
    }

    /// Finalize using only the actual resource compiler's bundled helper server.
    /// The optional request is still required, and the same owner issues access
    /// before deriving the original's correlated helper targets.
    pub fn finalize_authorities_with_helper_server(
        &mut self, helper: &mut acquisition::InstalledSetupSelection,
        requests: &PreparationAuthorities, server: Option<&meerkat_core::McpServerConfig>,
    ) -> Result<PreparedLocalPolicy, HostLifetimeError> {
        self.finalize_authorities_with_helper_resources(helper, requests, server, None)
    }

    /// The same held compiler supplies its actual selected resource and captured
    /// helper session. Caller JSON supplies neither observation.
    pub fn finalize_authorities_with_helper_resources(
        &mut self, helper: &mut acquisition::InstalledSetupSelection,
        requests: &PreparationAuthorities, server: Option<&meerkat_core::McpServerConfig>,
        session: Option<(&meerkat_core::MobMemberBinding, &meerkat_core::SessionId)>,
    ) -> Result<PreparedLocalPolicy, HostLifetimeError> {
        self.configure_held_authorities_with_helper_resources(helper, requests, server, session)?;
        self.finalize_selection()?;
        self.selected().cloned()
    }

    /// Configure one explicitly selected helper on the existing pending owner
    /// after native materialization supplies its actual member/session pair.
    /// Multiple helpers may be configured before the existing finalization call
    /// publishes the complete route selection once. This does not finalize or
    /// permit invocation/adoption, and a finalized owner rejects further calls.
    /// Partial issuance retains this same owner and native issuance IDs for
    /// exact recovery; a failure is not rollback of earlier configured helpers.
    pub fn configure_held_authorities_with_helper_resources(
        &mut self, helper: &mut acquisition::InstalledSetupSelection,
        requests: &PreparationAuthorities, server: Option<&meerkat_core::McpServerConfig>,
        session: Option<(&meerkat_core::MobMemberBinding, &meerkat_core::SessionId)>,
    ) -> Result<(), HostLifetimeError> {
        let PreparedAuthorityState::Pending(owner) = &mut self.authorities else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        owner.configure_authorities_with_helper_resources(helper, requests, server, session)
    }

    fn selected(&self) -> Result<&PreparedLocalPolicy, HostLifetimeError> {
        match &self.authorities {
            PreparedAuthorityState::Finalized(selected) => Ok(selected),
            PreparedAuthorityState::Pending(_) => Err(HostLifetimeError::Unavailable),
        }
    }

    fn finalize_selection(&mut self) -> Result<(), HostLifetimeError> {
        let PreparedAuthorityState::Pending(owner) = &self.authorities else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        let selected = owner.selected().clone();
        selected.validate()?;
        let native = &selected.native_host;
        let routes = native.routes.iter().map(|route| {
            let (target, requester, actor) = route.source_binding_in(&selected.channels)?;
            let member = native.members.iter().find(|member| &member.member == target)
                .ok_or(HostLifetimeError::InvalidSelection)?;
            Ok(PrivateOriginalRoute {
                id: route.id.clone(),
                grant_caller: native.grant_administrator.clone(),
                application_parent: route.application_parent.clone(),
                controller_parent: route.controller_parent.clone(),
                grant_target: toolkit_package::Id::new(target.member.clone())
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
                grant_audience: member.audience.clone(),
                realm: native.realm.clone(),
                requester: requester.clone(),
                ingress_actor: actor.clone(),
                logical_executor: route.logical_executor.clone(),
                represented_subject: route.represented_subject.clone(),
                source_authority: route.source_authority.clone(),
                authentication_domain: route.authentication_domain.clone(),
                event_domain: route.event_domain.clone(),
            })
        }).collect::<Result<Vec<_>, HostLifetimeError>>()?;
        self.originals.finalize_routes(routes).map_err(|_| HostLifetimeError::InvalidSelection)?;
        self.authorities = PreparedAuthorityState::Finalized(selected);
        Ok(())
    }

    /// The same explicitly selected initialize/restore operation used by the
    /// operator owner. This is called only by the stopped application's private
    /// preparation entry. An occupied, partial or uncertain pair is refused.
    /// Opening stores is not native bootstrap or permission to invoke a tool.
    pub async fn open(
        deployment_root: PathBuf,
        selected: PreparedLocalPolicy,
        startup: operator_setup::OperatorOwnerStartup,
        tool_policy: PreparedInstalledToolPolicy,
    ) -> Result<Self, HostLifetimeError> {
        PreparedInstallationOwner::open(deployment_root, selected, startup)?.complete(tool_policy).await
    }

    /// Used by the same gateway construction participant. The ordinary source
    /// entry points still require their actual final lifetime/runtime bindings.
    pub fn originals(&self) -> Arc<PrivateOriginals> { self.originals.clone() }

    pub fn policy(&self) -> Arc<PreparedApplicationPolicy> { self.policy.clone() }

    /// Retain the actual native construction participants before bootstrap.
    /// The returned value forwards the same leased source, policy registry,
    /// early leaves and continuation owner into final installed composition.
    pub fn gateway_runtime(
        &self,
        registration: Arc<prepared_registration::PreparedMemberRegistration>,
        tools: acquisition::InstalledAcquisitionTools,
        output_audience: std::collections::BTreeSet<toolkit_package::Id>,
    ) -> Result<crate::host_gateway::PreparedGatewayRuntime, HostLifetimeError> {
        let continuation = acquisition::InstalledNativeContinuation::new(
            self.store.clone(), self.source_scope.clone(), self.originals.clone(),
            toolkit_package::Id::new("toolkit.setup").map_err(|_| HostLifetimeError::InvalidSelection)?,
            output_audience,
        )?;
        Ok(crate::host_gateway::PreparedGatewayRuntime::new(
            self.originals.clone(), registration, self.tool_policy.registry(), tools, continuation,
        ))
    }

    /// The actual gateway consumes its native owner configuration once. This
    /// retains the same policy participant that later adopts the final lifetime.
    pub fn take_configuration(&self)
        -> Result<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration, HostLifetimeError>
    {
        self.configuration.lock().map_err(|_| HostLifetimeError::Unavailable)?
            .take().ok_or(HostLifetimeError::InvalidSelection)
    }

    /// Adopt a published selection after every native finish and compiler
    /// output has been verified by the driver. Reuse both leased stores and the
    /// policy registry. No second owner is opened and no rollback is asserted.
    pub fn adopt(
        self,
        selection: HostBootstrapSelection,
    ) -> Result<(NativeHostLifetime, InstalledToolkitHost), HostLifetimeError> {
        // Refuse incomplete authority finalization before opening a lifetime.
        self.selected().map_err(|error| adoption_refusal("installation.authorities_finalized", error))?;
        if selection.deployment_root.canonicalize()
            .map_err(|_| adoption_refusal("installation.canonical_deployment", HostLifetimeError::Unavailable))? != self.deployment_root
        { return Err(adoption_refusal("installation.deployment_matches", HostLifetimeError::InvalidSelection)); }
        let mut owner = NativeHostLifetime::bootstrap(selection.clone())
            .map_err(|error| adoption_refusal("installation.bootstrap_lifetime", error))?;
        if !self.selected().map_err(|error| adoption_refusal("installation.selected_authorities", error))?
            .matches(owner.selected_policy()) {
            return Err(adoption_refusal("installation.policy_matches", HostLifetimeError::InvalidSelection));
        }
        let native = owner.selected_policy().native_host.clone()
            .ok_or_else(|| adoption_refusal("installation.native_policy", HostLifetimeError::InvalidSelection))?;
        let ValidatedInstalledArtifacts { tool_policy, surfaces } =
            validate_installed_artifacts(&native, |reference| {
                read_bundle_artifact(&selection, reference)
            }).map_err(|error| adoption_refusal("installation.validate_artifacts", error))?;
        let tool_policy = tool_policy.adopt(self.tool_policy)
            .map_err(|error| adoption_refusal("installation.adopt_tool_policy", error))?;
        owner.bind_native_authorization_publication(self.source.publication())
            .map_err(|error| adoption_refusal("installation.bind_publication", error))?;
        owner.validate_current_selection()
            .map_err(|error| adoption_refusal("installation.current_selection", error))?;
        Ok((owner, InstalledToolkitHost {
            originals: self.originals,
            store: self.store,
            source_scope: self.source_scope,
            acquisition: acquisition::AcquisitionBootstrap::default(),
            native,
            tool_policy,
            surfaces,
            update_associations: None,
        }))
    }
}

#[cfg(test)]
mod authority_tests;
