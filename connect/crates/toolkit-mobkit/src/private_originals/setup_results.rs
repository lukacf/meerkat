//! Exact current helper-to-acquisition correlation. Nothing here reconstructs
//! the original agent's WorkAuthorizationContext or issues operation permission.

use super::*;
use crate::{HostAdmissionContext, installed_host::acquisition::InstalledSetupSelection};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{ToolDispatchContext, exact_operation::OperationExecutionScope};
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use toolkit_package::{ContentDigest, Revision};
use toolkit_store::RecordCondition;

pub(crate) struct SetupResultSource {
    pub selected_helper: meerkat_core::MobMemberBinding,
    pub operation: crate::original_ingress::helper_originals::HelperOperation,
    pub helper: InputAuthorityAssociation,
    pub source: RecordCondition,
    pub contributors: Vec<InputAuthorityAssociation>,
    pub contributor_sources: Vec<(InputAuthorityAssociation, RecordCondition)>,
    pub ancestor_sources: Vec<RecordCondition>,
    pub context: HostAdmissionContext,
}

impl PrivateOriginals {
    pub(crate) async fn setup_result_source(
        &self,
        context: &ToolDispatchContext,
        expected: &InstalledSetupSelection,
    ) -> Result<SetupResultSource, PrivateOriginalError> {
        self.setup_result_source_from(context, std::slice::from_ref(expected)).await
    }

    pub(crate) async fn setup_result_source_from(
        &self,
        context: &ToolDispatchContext,
        expected: &[InstalledSetupSelection],
    ) -> Result<SetupResultSource, PrivateOriginalError> {
        let authorization = context
            .work_authorization()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let retained = context
            .retained_work()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let session = context
            .origin_session_id()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let OperationExecutionScope::RuntimeInput {
            owner_session_id,
            canonical_input_id,
            ..
        } = authorization.execution_scope()
        else {
            return Err(PrivateOriginalError::Denied);
        };
        // Native transcript interaction lineage is optional. This lookup is
        // bound to the admitted work scope and exact retained native identity.
        if session != owner_session_id
            || context.run_id().is_some_and(|run| run != retained.run_id())
            || retained.runtime_id()
                != meerkat_runtime::LogicalRuntimeId::for_session(session).to_string()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let selected = retained
            .selected_input_bindings()
            .get(&canonical_input_id.to_string())
            .ok_or(PrivateOriginalError::Denied)?;
        let held = {
            let helpers = self
                .helper_sources
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut matched = None;
            for helper in helpers.iter() {
                let association = helper.original().association();
                let digest = ContentDigest::of_bytes(
                    &serde_json::to_vec(association).map_err(|_| PrivateOriginalError::Conflict)?,
                );
                if !retained
                    .contributors()
                    .iter()
                    .any(|source| source.association_digest == digest.as_str())
                {
                    continue;
                }
                let binding = hex(&association
                    .canonical_bytes()
                    .map_err(|_| PrivateOriginalError::Conflict)?);
                let batch = hex(&association
                    .batch_identity_bytes()
                    .map_err(|_| PrivateOriginalError::Conflict)?);
                if selected.binding == binding && selected.batch_key == batch {
                    if matched.is_some() {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    matched = Some(helper.clone());
                }
            }
            matched.ok_or(PrivateOriginalError::Denied)?
        };
        let association = held.original().association();
        let facts = self
            .helper_policy_source(association)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if expected.iter().filter(|helper| **helper == facts.selection.helper).count() != 1
            || association.candidate().target.context.as_str() != session.to_string()
            || association.candidate().target.logical_runtime.as_str() != retained.runtime_id()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let saved = self
            .store
            .get(&held.source_record().key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        held.check_result_record(&saved, session)?;
        // Match every canonical contributor through this actual source owner's
        // retained handles, then re-read that exact source. Native liveness is
        // checked again below; the typed snapshot is not an atomic permission.
        let mut originals = Vec::new();
        let mut contributor_sources = Vec::new();
        let mut ancestor_sources = Vec::new();
        for contributor in &facts.contributors {
            let original = {
                let retained = self
                    .retained
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                let mut matches = retained
                    .iter()
                    .filter(|held| held.original.association() == contributor);
                let first = matches
                    .next()
                    .ok_or(PrivateOriginalError::Unavailable)?
                    .clone();
                if matches.next().is_some() {
                    return Err(PrivateOriginalError::Conflict);
                }
                first
            };
            let row = self
                .store
                .get(&self.key(&original.reference)?)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let record = self.decode_record(&row, &original.reference)?;
            if record.final_association.as_ref() != Some(contributor) {
                return Err(PrivateOriginalError::Conflict);
            }
            if record.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            self.extend_result_ancestor_sources(&record, &mut ancestor_sources).await?;
            contributor_sources.push((contributor.clone(), RecordCondition {
                key: row.key, revision: row.revision, content_digest: row.content_digest,
                claim: None, dispatch_claim: None,
            }));
            originals.push(original);
        }
        for original in &originals {
            self.require_result_source(original.original.association(), false)?;
            self.installed_setup_facts(&original.reference)
                .map_err(|error| match error {
                    crate::ToolAdmissionError::Denied => PrivateOriginalError::Denied,
                    crate::ToolAdmissionError::Unavailable => PrivateOriginalError::Unavailable,
                })?;
        }
        // This helper association was matched to the exact selected native
        // input above. Its native acceptance may precede the source callback;
        // the originating contributors remain strict retained sources.
        self.require_result_source(association, true)?;
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let current = lifetime
            .with_current_setup(|| {
                use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
                let policy = lifetime.selected_policy();
                let native = policy
                    .native_host
                    .as_ref()
                    .ok_or(HostLifetimeError::Unavailable)?;
                if self.scope.realm != native.realm.as_str()
                    || self.scope.owner != policy.deployment.as_str()
                    || native.ingress_domain != *self.owner.ingress_domain()
                    || facts.selection.grant_caller != native.grant_administrator
                    || !native
                        .members
                        .iter()
                        .any(|member| *member == facts.selection.member)
                {
                    return Err(HostLifetimeError::Denied);
                }
                let revision = ContentDigest::of_bytes(
                    &canonical_json(policy).map_err(|_| HostLifetimeError::Unavailable)?,
                );
                Ok(HostAdmissionContext {
                    tenant: Id::new(self.scope.realm.clone())
                        .map_err(|_| HostLifetimeError::Unavailable)?,
                    deployment: policy.deployment.clone(),
                    realm: native.realm.clone(),
                    target: association.candidate().target.clone(),
                    policy_revision: Revision::new(revision.to_string())
                        .map_err(|_| HostLifetimeError::Unavailable)?,
                    now_ms: self
                        .owner
                        .clock()
                        .now()
                        .map_err(|_| HostLifetimeError::Unavailable)?
                        .unix_ms,
                })
            })
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Denied,
            })?;
        Ok(SetupResultSource {
            selected_helper: facts.selection.helper.member.clone(),
            operation: facts.operation,
            helper: association.clone(),
            source: RecordCondition {
                key: saved.key,
                revision: saved.revision,
                content_digest: saved.content_digest,
                claim: None,
                dispatch_claim: None,
            },
            contributors: facts.contributors,
            contributor_sources,
            ancestor_sources,
            context: current,
        })
    }

    fn require_result_source(
        &self,
        association: &InputAuthorityAssociation,
        own: bool,
    ) -> Result<(), PrivateOriginalError> {
        let claims = association.candidate();
        match self
            .owner
            .original_disposition(&claims.ingress_namespace, &claims.original_work)
        {
            Ok(OriginalDisposition::Active) => (),
            Ok(OriginalDisposition::OperationCancelled { .. }) => {
                return Err(PrivateOriginalError::Denied);
            }
            Err(_) => return Err(PrivateOriginalError::Unavailable),
        }
        if own {
            return Ok(());
        }
        let basis = self
            .owner
            .admitted_original_basis(&claims.ingress_namespace, &claims.original_work)
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if basis != claims.authority_basis {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_ingress::tests::{claims, initialized, principal, private_originals, source};
    use meerkat_authorization_contracts::{constraints::ExecutionRestrictions, evidence::EvidenceId};
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn result_source_currentness_does_not_create_named_custody() {
        // Source-currentness control only. This fixture does not claim a real
        // helper dispatch, matched result, native admission or operation grant.
        let directory = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
        let owner = initialized(&directory.path().join("grants"));
        let adapter = private_originals(owner.clone(), &directory.path().join("source.sqlite3")).await;
        let base = owner.grants().issue_root(
            &principal("admin"), EvidenceId::new("result-source-base").unwrap(),
            principal("admin"), None, ExecutionRestrictions::unrestricted(),
        ).unwrap();
        let association = claims("result-source", &base);
        let original = owner.enroll_new_authenticated(
            association.clone(), source(&association), &principal("user"),
            &principal("frontend"), &principal("admin"),
            EvidenceId::new("result-source-child").unwrap(),
            ExecutionRestrictions::unrestricted(), |_| Ok(None),
        ).unwrap();
        let association = original.association();
        adapter.require_result_source(association, true).unwrap();
        assert!(matches!(adapter.require_result_source(association, false),
            Err(PrivateOriginalError::Unavailable)));
        let c = association.candidate();
        assert!(owner.admitted_original_basis(&c.ingress_namespace, &c.original_work).unwrap().is_none());
        let mut foreign = c.clone();
        foreign.ingress_namespace.realm = meerkat_core::RealmId::parse("foreign-realm").unwrap();
        let foreign = InputAuthorityAssociation::new(foreign).unwrap();
        assert!(matches!(adapter.require_result_source(&foreign, true),
            Err(PrivateOriginalError::Unavailable)));
    }
}
