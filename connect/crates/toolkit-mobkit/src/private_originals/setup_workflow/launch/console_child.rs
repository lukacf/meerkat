//! One authenticated Console send selected by the existing workflow owner.
use super::*;
use crate::private_host::ConsoleLoginHandle;
use meerkat_mobkit::identity_first::AgentIdentity;
use meerkat_toolkit::setup::SetupWorkBackend;
use meerkat_workgraph::{WorkItemRef, WorkOwnerKey};
use std::collections::BTreeSet;

/// Trusted application proposal only. The send key and enrollment select a
/// candidate; the native Console human and actual bound input supply custody.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleWorkflowChildRequest {
    pub parent: PrivateOriginalRef,
    pub planning: PrivateOriginalRef,
    pub route: Id,
    pub send_key: String,
    pub content_sha256: String,
    pub child: WorkItemRef,
    pub child_owner: WorkOwnerKey,
    pub issuance_id: EvidenceId,
    pub targets: Vec<crate::installed_host::operator_setup::SetupTargetRequest>,
    pub output_audiences: BTreeSet<Id>,
}

/// Process-only input proof created by the installed Console binder. It has
/// no wire constructor and does not replace native admission or a task grant.
pub struct ConsoleWorkflowChildInput {
    pub(super) request: WorkflowChildLaunchRequest,
    pub(super) controller: crate::private_originals::LocalControllerSetup,
    pub(super) raw: Input,
    pub(super) entry: crate::private_originals::console_ingress::ConsoleEntry,
}
impl ConsoleWorkflowChildInput {
    pub fn controller(&self) -> &crate::private_originals::LocalControllerSetup {
        &self.controller
    }
    pub fn request(&self) -> &WorkflowChildLaunchRequest {
        &self.request
    }
}

/// The already selected application launcher supplies its existing backend
/// and operator. It must use the captured controller, never send recursively
/// while the native Console bind owns the target's lifecycle lease.
#[async_trait::async_trait]
pub trait ConsoleWorkflowChildPreparer: Send + Sync {
    async fn prepare(
        &self,
        input: &ConsoleWorkflowChildInput,
    ) -> Result<(), WorkflowChildPreparationError>;
}

pub(in crate::private_originals) struct PreparedConsoleChild {
    request: ConsoleWorkflowChildRequest,
    request_bytes: Vec<u8>,
    plan: Weak<InstalledWorkflowPlanning>,
    identity: AgentIdentity,
    requester: meerkat_core::PrincipalRef,
    login: ConsoleLoginHandle,
    preparer: Arc<dyn ConsoleWorkflowChildPreparer>,
    claimed: OnceLock<meerkat_core::lifecycle::InputId>,
    original: OnceLock<PrivateOriginalRef>,
}
impl PreparedConsoleChild {
    pub(in crate::private_originals) fn route(&self) -> &Id {
        &self.request.route
    }
    fn observe(
        &self,
        request_bytes: &[u8],
        login: &ConsoleLoginHandle,
    ) -> Result<Option<PrivateOriginalRef>, PrivateOriginalError> {
        if self.request_bytes != request_bytes || !self.login.same_owner(login) {
            return Err(PrivateOriginalError::Conflict);
        }
        // This is a retained locator, not renewed preparation authority.
        Ok(self.original.get().cloned())
    }
    pub(in crate::private_originals) fn check(
        &self,
        source: &Arc<PrivateOriginals>,
        identity: &AgentIdentity,
        requester: &meerkat_core::PrincipalRef,
        login: &ConsoleLoginHandle,
    ) -> Result<(), PrivateOriginalError> {
        self.check_console(identity, requester, login)?;
        self.plan
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?
            .check_child_preparation(source, &self.request.planning)
            .map_err(|error| match error {
                WorkflowChildPreparationError::Refused(error) => error,
                WorkflowChildPreparationError::ObservationChanged => {
                    PrivateOriginalError::Unavailable
                }
            })
    }
    fn check_console(
        &self,
        identity: &AgentIdentity,
        requester: &meerkat_core::PrincipalRef,
        login: &ConsoleLoginHandle,
    ) -> Result<(), PrivateOriginalError> {
        if &self.identity != identity
            || &self.requester != requester
            || !self.login.same_owner(login)
        {
            return Err(PrivateOriginalError::Denied);
        }
        login.check().map_err(|_| PrivateOriginalError::Denied)
    }
    pub(in crate::private_originals) fn claim(
        &self,
        input: &Input,
    ) -> Result<(), PrivateOriginalError> {
        // Principal/login/identity were checked before selecting this holder.
        // Even wrong content consumes its first actual input claim; a failed
        // bind must not use another Console frame as a retry.
        self.claimed
            .set(input.id().clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        if !matches!(input, Input::Prompt(prompt)
            if matches!(&prompt.content, meerkat_core::ContentInput::Text(text)
                if digest(text.as_bytes()) == self.request.content_sha256))
            || input.header().authority_association.is_some()
            || input.header().retained_resume.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }
    pub(in crate::private_originals) fn retain_source_locator(
        &self,
        reference: PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        // The locator is retained before the first awaited source write. Its
        // presence does not assert that this row, preparation or input exists.
        self.original
            .set(reference)
            .map_err(|_| PrivateOriginalError::Conflict)
    }
    pub(in crate::private_originals) async fn prepare(
        &self,
        source: &Arc<PrivateOriginals>,
        reference: PrivateOriginalRef,
        entry: crate::private_originals::console_ingress::ConsoleEntry,
        context: &GovernedBindContext,
        raw: &Input,
        authentication: PrivateSourceEvidence,
    ) -> Result<(), PrivateOriginalError> {
        if self.claimed.get() != Some(raw.id()) || context.spec.identity != self.identity {
            return Err(PrivateOriginalError::Denied);
        }
        self.check(
            source,
            &context.spec.identity,
            &self.requester,
            entry.login(),
        )?;
        if self.original.get() != Some(&reference) {
            return Err(PrivateOriginalError::Conflict);
        }
        let Input::Prompt(prompt) = raw else {
            return Err(PrivateOriginalError::Denied);
        };
        let meerkat_core::ContentInput::Text(content) = &prompt.content else {
            return Err(PrivateOriginalError::Denied);
        };
        let input = ConsoleWorkflowChildInput {
            request: WorkflowChildLaunchRequest {
                parent: self.request.parent.clone(),
                planning: self.request.planning.clone(),
                submission: PrivateOriginalSubmit {
                    original: reference.clone(),
                    content: content.clone(),
                    transport_authentication: authentication,
                },
                child: self.request.child.clone(),
                child_owner: self.request.child_owner.clone(),
                issuance_id: self.request.issuance_id.clone(),
                targets: self.request.targets.clone(),
                output_audiences: self.request.output_audiences.clone(),
            },
            controller: crate::private_originals::LocalControllerSetup::from_console_bind(
                source,
                reference.clone(),
                context,
            ),
            raw: raw.clone(),
            entry,
        };
        self.preparer
            .prepare(&input)
            .await
            .map_err(|error| match error {
                WorkflowChildPreparationError::Refused(error) => error,
                WorkflowChildPreparationError::ObservationChanged => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.check(
            source,
            &context.spec.identity,
            &self.requester,
            input.entry.login(),
        )?;
        Ok(())
    }
}

impl PrivateOriginals {
    pub(in crate::private_originals) fn console_child_for_send(
        &self,
        key: &str,
    ) -> Result<Option<Arc<PreparedConsoleChild>>, PrivateOriginalError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut selected = None;
        for parent in retained.iter() {
            let plans = parent
                .workflow_plans
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            for plan in plans.iter() {
                for child in plan
                    .console_children
                    .lock()
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .iter()
                {
                    if child.request.send_key == key {
                        if selected.is_some() {
                            return Err(PrivateOriginalError::Conflict);
                        }
                        selected = Some(child.clone());
                    }
                }
            }
        }
        Ok(selected)
    }

    /// Retain one bounded proposal on the actual planning owner. Repetition is
    /// observation of the same holder, not another issue or input submission.
    pub async fn prepare_console_workflow_child(
        self: &Arc<Self>,
        request: ConsoleWorkflowChildRequest,
        login: ConsoleLoginHandle,
        backend: &SetupWorkBackend,
        allowed_outputs: &BTreeSet<Id>,
        preparer: Arc<dyn ConsoleWorkflowChildPreparer>,
    ) -> Result<Option<PrivateOriginalRef>, PrivateOriginalError> {
        if request.send_key.is_empty()
            || request.send_key.len() > 256
            || !valid_digest(&request.content_sha256)
            || request.output_audiences.is_empty()
            || !request.output_audiences.is_subset(allowed_outputs)
        {
            return Err(PrivateOriginalError::Denied);
        }
        let request_bytes = canonical_json(&request).map_err(|_| PrivateOriginalError::Conflict)?;
        if let Some(existing) = self.console_child_for_send(&request.send_key)? {
            // Observation of this exact retained proposal does not refresh
            // plan/login/work authority. Failed and retired binds stay claimed.
            return existing.observe(&request_bytes, &login);
        }
        let plan = self.retained_child_plan(&request.parent, &request.planning)?;
        plan.check_for_binding(self, &request.planning)?;
        if backend.pin() != &plan.terms.backend || plan.late_helper.is_none() {
            return Err(PrivateOriginalError::Denied);
        }
        let work = SetupWorkContext::from_owner(
            backend.pin().clone(),
            plan.terms.parent.clone(),
            request.child.clone(),
            plan.terms.parent_owner.clone(),
            request.child_owner.clone(),
            plan.terms.cycle_label.clone(),
        )
        .map_err(|_| PrivateOriginalError::Denied)?;
        work.check_current(backend)
            .await
            .map_err(|_| PrivateOriginalError::Denied)?;
        let (compiled, member, _) = self.selected_source_route(&request.route)?;
        if !matches!(
            compiled.origin,
            CompiledNativeSourceOrigin::LocalConsole { .. }
        ) {
            return Err(PrivateOriginalError::Denied);
        }
        let route = self.route(&request.route)?;
        if route.requester.kind != meerkat_core::PrincipalKind::Human
            || route.represented_subject.as_ref() != Some(&route.requester)
        {
            return Err(PrivateOriginalError::Denied);
        }
        let identity = self
            .bound_identity_runtime()?
            .owned_identity_for_member_alias(&member.member.member)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let _writer = self.owner.source_writer().await;
        plan.check_for_binding(self, &request.planning)?;
        login.check().map_err(|_| PrivateOriginalError::Denied)?;
        if let Some(existing) = self.console_child_for_send(&request.send_key)? {
            if existing.request_bytes != request_bytes {
                return Err(PrivateOriginalError::Conflict);
            }
            existing.check(self, &identity, &route.requester, &login)?;
            return Ok(existing.original.get().cloned());
        }
        let mut children = plan
            .console_children
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if children.len() >= 64 {
            return Err(PrivateOriginalError::Unavailable);
        }
        children.push(Arc::new(PreparedConsoleChild {
            request,
            request_bytes,
            plan: Arc::downgrade(&plan),
            identity,
            requester: route.requester.clone(),
            login,
            preparer,
            claimed: OnceLock::new(),
            original: OnceLock::new(),
        }));
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::private_host::{ConsoleLoginSelection, ConsoleLogins};
    use meerkat_runtime::identifiers::IdempotencyKey;

    struct RefusePreparation;
    #[async_trait::async_trait]
    impl ConsoleWorkflowChildPreparer for RefusePreparation {
        async fn prepare(
            &self,
            _: &ConsoleWorkflowChildInput,
        ) -> Result<(), WorkflowChildPreparationError> {
            Err(PrivateOriginalError::Denied.into())
        }
    }
    fn proposal(login: ConsoleLoginHandle) -> PreparedConsoleChild {
        let request: ConsoleWorkflowChildRequest = serde_json::from_value(serde_json::json!({
            "parent": {"route":"parent","original":"P","event_sha256":"a".repeat(64)},
            "planning": {"route":"planning","original":"G","event_sha256":"b".repeat(64)},
            "route":"console", "send_key":"one-console-send", "content_sha256":digest(b"selected text"),
            "child":{"realm_id":"realm","namespace":"work","item_id":"child"},
            "child_owner":{"kind":"agent","id":"child"}, "issuance_id":"child-issue",
            "targets":[], "output_audiences":["audience"]
        })).unwrap();
        PreparedConsoleChild {
            request_bytes: canonical_json(&request).unwrap(),
            request,
            plan: Weak::new(),
            identity: AgentIdentity::parse("console").unwrap(),
            requester: crate::private_originals::console_ingress::auth_fixture::selected_human(),
            login,
            preparer: Arc::new(RefusePreparation),
            claimed: OnceLock::new(),
            original: OnceLock::new(),
        }
    }
    fn selection(scope: &str) -> ConsoleLoginSelection {
        ConsoleLoginSelection {
            issuer: "issuer".into(),
            subject: "subject".into(),
            login_scope_id: scope.into(),
            expires_at_ms: 9_000_000_000_000,
        }
    }
    fn raw(content: &str) -> Input {
        let mut input = PromptInput::new(content, None);
        input.header.idempotency_key = Some(IdempotencyKey::new(
            "console:00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002",
        ));
        Input::Prompt(input)
    }

    #[tokio::test]
    async fn claimed_console_proposal_keeps_locator_after_retirement_without_rearming() {
        // Actual enrollment/claim owners, but no planning authority or native
        // admission is fabricated here. Full child issuance is installed-only.
        let (_connection, logins) = ConsoleLogins::fixture_owner();
        let login = logins.fixture_enroll(1, selection("selected")).unwrap();
        let selected = proposal(login.clone());
        selected
            .check_console(&selected.identity, &selected.requester, &login)
            .unwrap();
        assert!(
            selected
                .observe(&selected.request_bytes, &login)
                .unwrap()
                .is_none()
        );
        let input = raw("selected text");
        selected.claim(&input).unwrap();
        let locator = PrivateOriginalRef {
            route: selected.request.route.clone(),
            original: EvidenceId::new(input.id().to_string()).unwrap(),
            event_sha256: digest(b"retained raw source"),
        };
        selected.retain_source_locator(locator.clone()).unwrap();
        // This is exactly the holder state retained if the first awaited
        // source write or child preparation fails or is cancelled.
        let retired = logins.fixture_retire(1).unwrap().await.unwrap();
        assert!(retired.fenced && retired.reservations_drained && !retired.uncertain);
        assert!(login.check().is_err());
        assert!(matches!(
            selected.check_console(&selected.identity, &selected.requester, &login),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(
            selected.observe(&selected.request_bytes, &login).unwrap() == Some(locator.clone())
        );
        assert!(matches!(
            selected.claim(&input),
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(matches!(
            selected.claim(&raw("selected text")),
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(matches!(
            selected.retain_source_locator(locator),
            Err(PrivateOriginalError::Conflict)
        ));
        assert_eq!(selected.claimed.get(), Some(input.id()));
    }

    #[tokio::test]
    async fn console_proposal_observation_refuses_changed_request_and_distinct_login_owner() {
        let (_connection, logins) = ConsoleLogins::fixture_owner();
        let login = logins.fixture_enroll(1, selection("selected")).unwrap();
        let other = logins.fixture_enroll(2, selection("other")).unwrap();
        let selected = proposal(login.clone());
        selected
            .check_console(&selected.identity, &selected.requester, &login)
            .unwrap();
        assert!(matches!(
            selected.check_console(
                &AgentIdentity::parse("another-console").unwrap(),
                &selected.requester,
                &login
            ),
            Err(PrivateOriginalError::Denied)
        ));
        let mut foreign_human = serde_json::to_value(&selected.requester).unwrap();
        foreign_human["id"] = serde_json::Value::String("another-human".into());
        let foreign_human = serde_json::from_value(foreign_human).unwrap();
        assert!(matches!(
            selected.check_console(&selected.identity, &foreign_human, &login),
            Err(PrivateOriginalError::Denied)
        ));
        assert!(matches!(
            selected.check_console(&selected.identity, &selected.requester, &other),
            Err(PrivateOriginalError::Denied)
        ));
        for field in [
            "parent",
            "planning",
            "route",
            "send_key",
            "content_sha256",
            "child",
            "child_owner",
            "issuance_id",
            "targets",
            "output_audiences",
        ] {
            let mut changed = serde_json::to_value(&selected.request).unwrap();
            changed[field] = serde_json::Value::Null;
            assert!(matches!(
                selected.observe(&canonical_json(&changed).unwrap(), &login),
                Err(PrivateOriginalError::Conflict)
            ));
        }
        assert!(matches!(
            selected.observe(&selected.request_bytes, &other),
            Err(PrivateOriginalError::Conflict)
        ));
        let wrong_content = raw("other text");
        assert!(matches!(
            selected.claim(&wrong_content),
            Err(PrivateOriginalError::Denied)
        ));
        assert_eq!(selected.claimed.get(), Some(wrong_content.id()));
        assert!(matches!(
            selected.claim(&raw("selected text")),
            Err(PrivateOriginalError::Conflict)
        ));
        assert!(
            selected.original.get().is_none(),
            "claim alone is not a source write or admission"
        );
    }
}
