//! Consume the actual native child-issuance owner. Intent storage is the
//! non-admitting phase of the same canonical helper source row.
use super::*;
use crate::{installed_host::application_operations,
    original_ingress::helper_originals::HelperControllerIntent,
    private_host::{ConsoleLoginHandle, ConsoleLoginRefusal}};
use meerkat::governed_realm::helper_child_issuance::{
    GovernedHelperChildIssuance, HelperChildIssuanceRefusal,
};
use meerkat_authorization_contracts::constraints::DelegationDepth;
use meerkat_toolkit::host_lifetime::{control::HostLifetimeError, policy::CompiledNativeSourceOrigin};

impl PrivateOriginals {
    pub(crate) async fn issue_or_reconcile_setup_helper_grants(
        self: &Arc<Self>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        preparation: &meerkat_toolkit::setup::NativeWorkPreparation,
        prepared: &PreparedHelperController,
    ) -> Result<(Vec<GrantLineageRef>, WorkAuthorityBasis), PrivateOriginalError> {
        if !prepared.source.upgrade().is_some_and(|source| Arc::ptr_eq(&source, self))
            || context.work_authorization().is_none()
            || prepared.admitted.admission().key != *preparation.admission_key()
            || prepared.admitted.operation().acquisition != preparation.effect_key().record
            || prepared.admitted.operation().effect != preparation.effect_id()
            || prepared.admitted.operation().input_digest != preparation.input_reference().content_digest
        { return Err(PrivateOriginalError::Denied); }
        self.owner.check_admitted_helper(&prepared.admitted).map_err(helper_error)?;
        self.require_helper_selection(prepared.admitted.selection())?;
        let selected = prepared.admitted.selection();
        let facts = prepared.controller.plain_facts().map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut requested = application_operations::parent_restrictions(application_operations::controller(
            &selected.grant_caller, &selected.helper.logical_executor, &selected.member.audience,
            &facts.route_key(), application_operations::ControllerAction::Inference,
        ));
        requested.delegation_depth = DelegationDepth::remaining(0);
        let candidate = HelperControllerIntent::new(&prepared.admitted, prepared.target.clone(),
            prepared.controller.selection().clone(), requested).map_err(helper_error)?;
        // This durable write/read must finish before native issuance. A changed
        // actual session, model, route or intent refuses the existing row.
        let (intent, report_intent, account_intent, public_intent, catalog_intent) = self.owner.retain_helper_operation_intents(
            self.store.as_ref(), &prepared.admitted, candidate,
        ).await.map_err(helper_error)?;
        self.require_helper_selection(prepared.admitted.selection())?;
        let logins = self.helper_controller_logins(&prepared.admitted)?;
        let issuance = GovernedHelperChildIssuance::from_dispatch(
            call, context, self.owner.clone(), prepared.admitted.qualified_contributors(), intent,
        ).map_err(child_error)?;
        let report_issuance = report_intent.map(|intent| GovernedHelperChildIssuance::from_dispatch(
            call, context, self.owner.clone(), prepared.admitted.qualified_contributors(), intent,
        )).transpose().map_err(child_error)?;
        let account_issuance = account_intent.map(|intent| GovernedHelperChildIssuance::from_dispatch(
            call, context, self.owner.clone(), prepared.admitted.qualified_contributors(), intent,
        )).transpose().map_err(child_error)?;
        let public_issuance = public_intent.map(|intent| GovernedHelperChildIssuance::from_dispatch(
            call, context, self.owner.clone(), prepared.admitted.qualified_contributors(), intent,
        )).transpose().map_err(child_error)?;
        let catalog_issuance = catalog_intent.map(|intent| GovernedHelperChildIssuance::from_dispatch(
            call, context, self.owner.clone(), prepared.admitted.qualified_contributors(), intent,
        )).transpose().map_err(child_error)?;
        let report_capability = selected.helper.report_capability.clone();
        let catalog_capability = selected.helper.catalog_capability.clone();
        let configured_basis = selected.helper.operation_basis.clone();
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let source = self.clone();
        let admitted = prepared.admitted.clone();
        let controller = prepared.controller.clone();
        // Native owns the synchronous append/reconciliation. Retain the exact
        // client, source and same logins in that writer if the caller is lost.
        // No source-store mutation guard crosses this native reservation.
        let result = tokio::task::spawn_blocking(move || {
            let _controller = controller;
            source.owner.check_admitted_helper(&admitted).map_err(helper_error)?;
            lifetime.validate_current_selection().map_err(|error| match error {
                HostLifetimeError::Unavailable => PrivateOriginalError::Unavailable,
                _ => PrivateOriginalError::Denied,
            })?;
            // Do not hold with_current_setup here: the native operation calls
            // InstalledApplicationPolicy.current_invocation, whose actual phase
            // check would reenter that mutex. Native reserves the shared grant
            // publication before that policy check and retains it through the
            // append. Activation/component close uses this same publication.
            with_helper_logins(&logins, || {
                let controller = issuance.issue_or_recover()?;
                // Each append uses the actual operation's own current native
                // publication reservation. A known controller result never
                // implies that the separate report append settled.
                let append_operation = |issue: &GovernedHelperChildIssuance| {
                    // Every independent append rechecks the exact retained
                    // human owners; a prior append does not settle the next.
                    for login in &logins {
                        login.check().map_err(|refusal| match refusal {
                            ConsoleLoginRefusal::Retired | ConsoleLoginRefusal::Expired => HelperChildIssuanceRefusal::Denied,
                            ConsoleLoginRefusal::Unavailable | ConsoleLoginRefusal::Unknown => HelperChildIssuanceRefusal::Unavailable,
                        })?;
                    }
                    issue.issue_or_recover()
                };
                let report = report_issuance.as_ref().map(&append_operation).transpose()?;
                let account = account_issuance.as_ref().map(&append_operation).transpose()?;
                let public = public_issuance.as_ref().map(&append_operation).transpose()?;
                let catalog = catalog_issuance.as_ref().map(&append_operation).transpose()?;
                Ok((controller, report, account, public, catalog))
            }).map_err(child_error)
        }).await.map_err(|_| PrivateOriginalError::Unavailable)??;
        let configured_basis = match (catalog_capability, result.4) {
            (None, None) => configured_basis,
            (Some(capability), Some(catalog)) => capability.project(&configured_basis, catalog.lineage().to_vec())
                .map_err(|_| PrivateOriginalError::Conflict)?,
            _ => return Err(PrivateOriginalError::Conflict),
        };
        let basis = match (report_capability, result.1, result.2, result.3) {
            (None, None, None, None) => configured_basis,
            (Some(capability), Some(report), account, public) => capability.project_with_operations(&configured_basis,
                report.lineage().to_vec(), account.map(|grant| grant.lineage().to_vec()),
                public.map(|grant| grant.lineage().to_vec()))
                .map_err(|_| PrivateOriginalError::Conflict)?,
            _ => return Err(PrivateOriginalError::Conflict),
        };
        Ok((result.0.lineage().to_vec(), basis))
    }

    pub(super) fn helper_controller_logins(&self, admitted: &AdmittedHelperWork)
        -> Result<Vec<ConsoleLoginHandle>, PrivateOriginalError>
    {
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        let native = lifetime.selected_policy().native_host.as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
            .map_err(|_| PrivateOriginalError::Unavailable)?.operator;
        let retained = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let helpers = self.helper_sources.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut logins: Vec<ConsoleLoginHandle> = vec![];
        for contributor in admitted.contributors() {
            let Some(held) = retained.iter().find(|held| Arc::ptr_eq(&held.original, contributor)) else {
                // Internal helpers have no independent login. The source owner
                // requires the complete flattened set, including every external
                // ancestor whose exact handle is checked in this same loop.
                if helpers.iter().any(|held| Arc::ptr_eq(held.original(), contributor)) { continue; }
                return Err(PrivateOriginalError::Unavailable);
            };
            if let Some(login) = held.console_login.get() {
                login.check().map_err(|_| PrivateOriginalError::Denied)?;
                if !logins.iter().any(|prior| prior.same_owner(login)) { logins.push(login.clone()); }
                continue;
            }
            let route = self.route(&held.reference.route)?;
            let compiled = native.routes.iter().find(|row| row.id == held.reference.route)
                .ok_or(PrivateOriginalError::Denied)?;
            if matches!(&compiled.origin, CompiledNativeSourceOrigin::LocalConsole { .. })
                && route.requester.kind == meerkat_core::PrincipalKind::Human
                && !(route.requester == operator
                    && route.represented_subject.as_ref().is_none_or(|subject| subject == &operator))
            { return Err(PrivateOriginalError::Unavailable); }
        }
        Ok(logins)
    }
}

/// The same owners decide reserve versus retirement before the native call.
/// A known refusal settles these counts; unknown append/drop keeps uncertainty.
fn with_helper_logins<T>(logins: &[ConsoleLoginHandle], issue: impl FnOnce()
    -> Result<T, HelperChildIssuanceRefusal>) -> Result<T, HelperChildIssuanceRefusal>
{
    let mut reservations = vec![];
    for login in logins {
        match login.reserve() {
            Ok(reservation) => reservations.push(reservation),
            Err(refusal) => {
                for reservation in reservations { reservation.settled(); }
                return Err(match refusal {
                    ConsoleLoginRefusal::Retired | ConsoleLoginRefusal::Expired => HelperChildIssuanceRefusal::Denied,
                    ConsoleLoginRefusal::Unavailable | ConsoleLoginRefusal::Unknown => HelperChildIssuanceRefusal::Unavailable,
                });
            }
        }
    }
    let result = issue();
    for reservation in reservations {
        if matches!(&result, Err(HelperChildIssuanceRefusal::Uncertain)) { reservation.uncertain(); }
        else { reservation.settled(); }
    }
    result
}

fn child_error(refusal: HelperChildIssuanceRefusal) -> PrivateOriginalError {
    match refusal {
        HelperChildIssuanceRefusal::Denied => PrivateOriginalError::Denied,
        HelperChildIssuanceRefusal::Unavailable | HelperChildIssuanceRefusal::Uncertain => PrivateOriginalError::Unavailable,
    }
}

#[cfg(test)]
mod tests;
