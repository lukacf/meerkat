//! Same-owner original lookup for an actual tool dispatch.
//!
//! Native retained-work fields are identity only. Matching them never admits a
//! helper, creates source custody, copies a grant or replaces native permission.

use super::*;
use meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES;
use meerkat_core::{ToolDispatchContext, exact_operation::OperationExecutionScope};
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use toolkit_package::ContentDigest;

/// The existing writer handle and its exact canonical source observation.
/// Callers must still use the actual borrowed native dispatch authorization at
/// every effect boundary. This is not a continuation or helper admission.
pub(crate) struct AcquisitionOriginal {
    pub reference: PrivateOriginalRef,
    pub original: Arc<RetainedToolkitOriginal>,
    pub source_record: Arc<StoredRecord>,
    /// Exact source handles matched to this live native invocation's staged
    /// associations. Native owns their accepted binding and current permission;
    /// source receipt processing may still be pending. This subset is not
    /// retained in helper admission facts or reused for later child input.
    pub own_originals: Vec<Arc<RetainedToolkitOriginal>>,
    /// Complete same-owner original set for a future helper: each staged
    /// source in native order, then its recorded ancestors in first-seen order. Native
    /// admitted_original_basis returns one basis, never recursive contributor custody.
    /// Each exact handle retains the canonical recorded contributor edges. The
    /// set preserves source identity and relationships, not a union of grants;
    /// each original remains a separate native conjunct at helper admission.
    pub contributors: Vec<Arc<RetainedToolkitOriginal>>,
}

impl AcquisitionOriginal {
    pub(crate) fn setup_origin(&self) -> crate::original_ingress::helper_originals::HelperSetupOrigin {
        crate::original_ingress::helper_originals::HelperSetupOrigin {
            realm: self.original.association().candidate().ingress_namespace.realm.clone(),
            source_owner: self.original.association().candidate().ingress_namespace.ingress.clone(),
            original_authority: self.original.association().candidate().original_work.authority.clone(),
            original: self.reference.clone(),
        }
    }
}

impl PrivateOriginals {
    /// Compare two validated observations around an application admission.
    /// Only source-owned accounting for the same already-recorded attempt may
    /// advance. Every other typed source field remains exact; this comparison
    /// supplies neither native admission nor contributor currentness.
    pub(crate) fn same_acquisition_source(
        &self,
        reference: &PrivateOriginalRef,
        prior: &StoredRecord,
        current: &StoredRecord,
    ) -> Result<bool, PrivateOriginalError> {
        let mut before = self.decode_record(prior, reference)?;
        let mut after = self.decode_record(current, reference)?;
        if current.key != prior.key || current.data.schema != prior.data.schema
            || current.data.version != prior.data.version
        { return Ok(false); }
        if current.revision == prior.revision {
            return Ok(current.content_digest == prior.content_digest && before == after);
        }
        if current.revision < prior.revision { return Ok(false); }
        let (Some(first), Some(last)) =
            (before.custody_attempt.take(), after.custody_attempt.take())
        else { return Ok(false); };
        if before != after || first == last || !first.same_attempt(&last) {
            return Ok(false);
        }
        use crate::original_ingress::custody_attempt::{CustodyChange, CustodyDecision};
        // A newly withdrawn source preserves the previous changed-row refusal.
        // This is not evidence of native cancellation or nonadmission.
        if last.decision() == CustodyDecision::Withdrawn { return Ok(false); }
        let mut reachable = first;
        let mut revisions_left = current.revision - prior.revision;
        loop {
            if reachable == last { return Ok(true); }
            if revisions_left == 0 { return Ok(false); }
            if reachable.changed(CustodyChange::Promote).ok().as_ref() == Some(&last) {
                return Ok(true);
            }
            // Walk only the existing policy's bounded charge/settle chain.
            // An inner await can span several steps, not just one CAS. Each
            // step still requires an observed canonical revision increment.
            let change = if reachable.observation_entered() {
                CustodyChange::ResolutionSettled
            } else { CustodyChange::ChargeResolution };
            match reachable.changed(change) {
                Ok(next) => { reachable = next; revisions_left -= 1; }
                Err(_) => return Ok(false),
            }
        }
    }

    pub(crate) async fn acquisition_original(
        &self,
        context: &ToolDispatchContext,
    ) -> Result<AcquisitionOriginal, PrivateOriginalError> {
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
            || retained.runtime_id()
                != meerkat_runtime::LogicalRuntimeId::for_session(session).to_string()
            || context.run_id().is_some_and(|run| run != retained.run_id())
        {
            return Err(PrivateOriginalError::Denied);
        }
        if retained.contributors().is_empty() {
            return Err(PrivateOriginalError::Denied);
        }
        let selected_row = retained
            .selected_input_bindings()
            .get(&canonical_input_id.to_string())
            .ok_or(PrivateOriginalError::Denied)?;
        let (held, mut pending) = {
            let originals = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut anchor = None;
            let mut staged = Vec::new();
            for (index, contributor) in retained.contributors().iter().enumerate() {
                // Repeated native references may name the same staged original.
                // A different association/submission for the same input is not
                // an identity match and cannot be silently deduplicated.
                if retained.contributors()[..index]
                    .iter()
                    .any(|prior| prior.input_id == contributor.input_id && prior != contributor)
                {
                    return Err(PrivateOriginalError::Conflict);
                }
                let mut matched = None;
                for original in originals.iter() {
                    // Exact retained_work::identity_from codec in the native
                    // owner. Identity lookup never substitutes for custody.
                    let bytes = serde_json::to_vec(original.original.association())
                        .map_err(|_| PrivateOriginalError::Conflict)?;
                    if ContentDigest::of_bytes(&bytes).as_str() == contributor.association_digest {
                        if matched.is_some() {
                            return Err(PrivateOriginalError::Conflict);
                        }
                        matched = Some(original.clone());
                    }
                }
                let current = matched.ok_or(PrivateOriginalError::Unavailable)?;
                let association = current.original.association();
                let claims = association.candidate();
                if claims.target.logical_runtime.as_str() != retained.runtime_id()
                    || claims.target.context.as_str() != session.to_string()
                    || claims.ingress_namespace.realm.to_string() != self.scope.realm
                    || claims.ingress_namespace.ingress != *self.owner.ingress_domain()
                {
                    return Err(PrivateOriginalError::Denied);
                }
                // The native driver chose canonical_input_id and retained this
                // row's exact association binding. On resume it binds that row
                // to the canonical original contributor. This is its anchor,
                // not a Toolkit policy for choosing from the staged batch.
                if selected_row.binding
                    == hex_bytes(
                        &association
                            .canonical_bytes()
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                    )
                    && selected_row.batch_key
                        == hex_bytes(
                            &association
                                .batch_identity_bytes()
                                .map_err(|_| PrivateOriginalError::Conflict)?,
                        )
                {
                    if anchor
                        .as_ref()
                        .is_some_and(|prior| !Arc::ptr_eq(prior, &current))
                    {
                        return Err(PrivateOriginalError::Conflict);
                    }
                    anchor = Some(current.clone());
                }
                // Retain every distinct source handle. Exact repeated runtime
                // references stay in the borrowed native context; they do not
                // create a second original or a different source ceiling.
                if !staged.iter().any(|prior| Arc::ptr_eq(prior, &current)) {
                    if staged.len() >= MAX_ASSOCIATION_REFERENCES {
                        return Err(PrivateOriginalError::Denied);
                    }
                    staged.push(current);
                }
            }
            (anchor.ok_or(PrivateOriginalError::Denied)?, staged)
        };
        // Freeze membership before following named ancestor edges. A named
        // ancestor is not an own original merely because this lookup reopens it.
        let own_originals: Vec<_> = pending.iter().map(|held| held.original.clone()).collect();
        let claims = held.original.association().candidate();
        let mut visited = Vec::new();
        let mut source_record = None;
        let mut index = 0;
        while index < pending.len() {
            let current = pending[index].clone();
            index += 1;
            let association = current.original.association();
            let current_claims = association.candidate();
            if visited.contains(&current_claims.original_work) {
                continue;
            }
            if visited.len() >= MAX_ASSOCIATION_REFERENCES
                || current_claims.ingress_namespace.realm != claims.ingress_namespace.realm
                || current_claims.ingress_namespace.ingress != claims.ingress_namespace.ingress
                || current_claims.requester != claims.requester
            {
                return Err(PrivateOriginalError::Denied);
            }
            let stored = self
                .store
                .get(&self.key(&current.reference)?)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let record = self.decode_record(&stored, &current.reference)?;
            if record.final_association.as_ref() != Some(association) {
                return Err(PrivateOriginalError::Conflict);
            }
            if record.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            self.require_acquisition_source(
                association,
                own_originals.iter().any(|own| Arc::ptr_eq(own, &current.original)),
            )?;
            if Arc::ptr_eq(&current, &held) {
                source_record = Some(Arc::new(stored));
            }
            visited.push(current_claims.original_work.clone());
            // Source custody is not recursive in native. Follow only edges
            // already recorded in this exact canonical association. Reopening
            // their real owner handles preserves the accepted relationship;
            // it does not infer ancestry or combine permissions from names.
            let originals = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            for ancestor in &current_claims.contributing_work {
                let mut matches = originals.iter().filter(|candidate| {
                    let candidate = candidate.original.association().candidate();
                    candidate.original_work == *ancestor
                        && candidate.ingress_namespace.realm == claims.ingress_namespace.realm
                        && candidate.ingress_namespace.ingress == claims.ingress_namespace.ingress
                });
                let parent = matches
                    .next()
                    .ok_or(PrivateOriginalError::Unavailable)?
                    .clone();
                if matches.next().is_some() {
                    return Err(PrivateOriginalError::Conflict);
                }
                if !pending
                    .iter()
                    .any(|previous| Arc::ptr_eq(previous, &parent))
                {
                    if pending.len() >= MAX_ASSOCIATION_REFERENCES {
                        return Err(PrivateOriginalError::Denied);
                    }
                    pending.push(parent);
                }
            }
        }
        // Recheck all source owners after the final storage await. This is not
        // an atomic permission snapshot: the actual effect/fence must recheck
        // every contributor under native publication when admitting the helper.
        for original in &pending {
            self.require_acquisition_source(
                original.original.association(),
                own_originals.iter().any(|own| Arc::ptr_eq(own, &original.original)),
            )?;
        }
        Ok(AcquisitionOriginal {
            reference: held.reference.clone(),
            original: held.original.clone(),
            source_record: source_record.ok_or(PrivateOriginalError::Unavailable)?,
            own_originals,
            contributors: pending
                .into_iter()
                .map(|held| held.original.clone())
                .collect(),
        })
    }

    fn require_acquisition_source(
        &self,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        own: bool,
    ) -> Result<(), PrivateOriginalError> {
        let claims = association.candidate();
        match self
            .owner
            .original_disposition(&claims.ingress_namespace, &claims.original_work)
        {
            Ok(OriginalDisposition::Active) => {}
            Ok(OriginalDisposition::OperationCancelled { .. }) => {
                return Err(PrivateOriginalError::Denied);
            }
            Err(_) => return Err(PrivateOriginalError::Unavailable),
        }
        // An actual staged association is already bound by the native input
        // owner. Its own current disposition is separate from this source's
        // later acceptance receipt. Named-only contributors still require the
        // source's retained mandate. Neither check grants operation permission.
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

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    // Source-owner and identity checks only. The fixture explicitly acknowledges
    // its real source guard; it runs no native input, model, helper or effect.
    use super::*;
    use crate::original_ingress::tests::{
        claims, initialized, principal, private_event, private_originals, source,
    };
    use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
    use meerkat_core::{
        AgentError, AgentLlmClient, ControllerModelClient, ControllerModelSelection, InputId,
        LlmStreamResult, Message, Provider, ProviderParamsOverride, RunId, RuntimeEpochId,
        SessionId, SessionLlmIdentity, ToolDef, WorkAuthorizationContext,
        authorization::{
            OperationAuthorizationError, PreparedAuthorizationBinding,
            PreparedOperationAuthorization, WorkAuthorization,
        },
        interaction::InteractionId,
        retained_work::{RetainedContributorRef, RetainedWorkIdentity, SelectedInputBinding},
    };
    use meerkat_runtime::{Input, PromptInput, identifiers::IdempotencyKey};
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

    struct NoOperations;
    impl WorkAuthorization for NoOperations {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
            Err(OperationAuthorizationError::Unavailable)
        }
    }

    fn controller_selection() -> ControllerModelSelection {
        ControllerModelSelection::new(
            SessionLlmIdentity {
                model: "lookup-fixture".into(),
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            serde_json::from_value(
                serde_json::json!({"realm": "test-realm", "account": "lookup-fixture"}),
            )
            .unwrap(),
            "lookup-fixture".into(),
            "lookup-fixture".into(),
        )
    }

    struct UnusedClient;
    #[async_trait::async_trait]
    impl AgentLlmClient for UnusedClient {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(controller_selection())
        }
        async fn stream_response(
            &self,
            _: &[Message],
            _: &[Arc<ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            panic!("lookup must never execute a model")
        }
        fn provider(&self) -> Provider {
            Provider::Anthropic
        }
        fn model(&self) -> &str {
            "lookup-fixture"
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        adapter: Arc<PrivateOriginals>,
        original: Arc<RetainedToolkitOriginal>,
        reference: PrivateOriginalRef,
        session: SessionId,
        input: InputId,
        pending: Option<crate::original_ingress::PendingToolkitOriginal>,
    }

    impl Fixture {
        async fn new(promoted: bool) -> Self {
            let directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let owner = initialized(&directory.path().join("grants"));
            let adapter =
                private_originals(owner.clone(), &directory.path().join("source.sqlite3")).await;
            let session = SessionId::new();
            let base = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    EvidenceId::new("lookup-base").unwrap(),
                    principal("admin"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let reference = adapter.retain_event(private_event("lookup")).await.unwrap();
            let mut candidate = claims("lookup", &base).candidate().clone();
            candidate.target.logical_runtime = EvidenceId::new(
                meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string(),
            )
            .unwrap();
            candidate.target.context = EvidenceId::new(session.to_string()).unwrap();
            candidate.controller_model = Some(controller_selection());
            candidate.controller_grant_lineage = vec![base];
            let association =
                meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                    candidate,
                )
                .unwrap();
            let original = owner
                .enroll_new_authenticated_async(
                    association.clone(),
                    source(&association),
                    &principal("user"),
                    &principal("frontend"),
                    &principal("admin"),
                    EvidenceId::new("lookup-child").unwrap(),
                    ExecutionRestrictions::unrestricted(),
                    |association| {
                        let adapter = adapter.clone();
                        let reference = reference.clone();
                        async move { adapter.persist_binding(&reference, &association).await }
                    },
                )
                .await
                .unwrap();
            adapter
                .hold_binding(reference.clone(), original.clone())
                .await
                .unwrap();
            let input = Input::Prompt(PromptInput::new("source fixture", None));
            let input_id = input.id().clone();
            let (_, pending) = owner
                .bind_authenticated_input(
                    &original,
                    &session,
                    input,
                    IdempotencyKey::new("lookup-event"),
                    association.candidate().original_authentication.clone(),
                    ControllerModelClient::new(controller_selection(), Arc::new(UnusedClient)),
                )
                .unwrap();
            let pending = if promoted {
                pending.admit().unwrap();
                None
            } else {
                Some(pending)
            };
            Self {
                _directory: directory,
                adapter,
                original,
                reference,
                session,
                input: input_id,
                pending,
            }
        }

        async fn descendant(
            &self,
            label: &str,
            parent: &Arc<RetainedToolkitOriginal>,
        ) -> (Arc<RetainedToolkitOriginal>, PrivateOriginalRef, InputId) {
            let owner = &self.adapter.owner;
            let base = owner
                .grants()
                .issue_root(
                    &principal("admin"),
                    EvidenceId::new(format!("{label}-base")).unwrap(),
                    principal("admin"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .unwrap();
            let mut event = private_event(label);
            event
                .contributors
                .push(parent.association().candidate().original_work.work.clone());
            let reference = self.adapter.retain_event(event).await.unwrap();
            let mut candidate = claims(label, &base).candidate().clone();
            candidate.target = self.original.association().candidate().target.clone();
            candidate
                .contributing_work
                .push(parent.association().candidate().original_work.clone());
            candidate.controller_model = Some(controller_selection());
            candidate.controller_grant_lineage = vec![base];
            let association =
                meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                    candidate,
                )
                .unwrap();
            let original = owner
                .enroll_new_authenticated_async(
                    association.clone(),
                    source(&association),
                    &principal("user"),
                    &principal("frontend"),
                    &principal("admin"),
                    EvidenceId::new(format!("{label}-child")).unwrap(),
                    ExecutionRestrictions::unrestricted(),
                    |association| {
                        let adapter = self.adapter.clone();
                        let reference = reference.clone();
                        async move { adapter.persist_binding(&reference, &association).await }
                    },
                )
                .await
                .unwrap();
            self.adapter
                .hold_binding(reference.clone(), original.clone())
                .await
                .unwrap();
            let input = Input::Prompt(PromptInput::new("source fixture", None));
            let input_id = input.id().clone();
            let (_, pending) = owner
                .bind_authenticated_input(
                    &original,
                    &self.session,
                    input,
                    IdempotencyKey::new(format!("{label}-event")),
                    association.candidate().original_authentication.clone(),
                    ControllerModelClient::new(controller_selection(), Arc::new(UnusedClient)),
                )
                .unwrap();
            pending.admit().unwrap();
            (original, reference, input_id)
        }

        fn identity(&self, selected_input: &InputId) -> RetainedWorkIdentity {
            self.identity_for(&self.original, &self.input, selected_input)
        }

        fn identity_for(
            &self,
            original: &RetainedToolkitOriginal,
            contributing_input: &InputId,
            selected_input: &InputId,
        ) -> RetainedWorkIdentity {
            let association = original.association();
            RetainedWorkIdentity::new(
                meerkat_runtime::LogicalRuntimeId::for_session(&self.session).to_string(),
                RunId::new(),
                vec![RetainedContributorRef {
                    input_id: contributing_input.clone(),
                    association_digest: ContentDigest::of_bytes(
                        &serde_json::to_vec(association).unwrap(),
                    )
                    .to_string(),
                    submission_digest: "a".repeat(64),
                }],
                BTreeMap::from([(
                    selected_input.to_string(),
                    SelectedInputBinding {
                        binding: hex_bytes(&association.canonical_bytes().unwrap()),
                        batch_key: hex_bytes(&association.batch_identity_bytes().unwrap()),
                    },
                )]),
                Some(controller_selection()),
            )
        }

        fn context(
            &self,
            origin: SessionId,
            current_input: InputId,
            identity: RetainedWorkIdentity,
        ) -> ToolDispatchContext {
            ToolDispatchContext::default()
                .with_runtime_identity(origin, Some(InteractionId::new()))
                .with_work_authorization(Some(
                    WorkAuthorizationContext::new(
                        Arc::new(NoOperations),
                        OperationExecutionScope::RuntimeInput {
                            owner_session_id: self.session.clone(),
                            runtime_epoch_id: RuntimeEpochId::new(),
                            submitted_input_id: current_input.clone(),
                            canonical_input_id: current_input,
                        },
                    )
                    .with_retained_work(identity),
                ))
        }
    }

    #[tokio::test]
    async fn returns_exact_retained_handle_and_canonical_row_without_authorizing_operations() {
        let fixture = Fixture::new(true).await;
        let context = fixture.context(
            fixture.session.clone(),
            fixture.input.clone(),
            fixture.identity(&fixture.input),
        );
        let resolved = fixture
            .adapter
            .acquisition_original(&context)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&resolved.original, &fixture.original));
        assert_eq!(resolved.contributors.len(), 1);
        assert!(Arc::ptr_eq(&resolved.contributors[0], &fixture.original));
        assert!(resolved.reference == fixture.reference);
        assert!(resolved.source_record.key == fixture.adapter.key(&fixture.reference).unwrap());
        assert!(
            fixture
                .adapter
                .decode_record(&resolved.source_record, &resolved.reference)
                .unwrap()
                .final_association
                .as_ref()
                == Some(fixture.original.association())
        );
        // The selected delivery input of a resume may differ from its original.
        let resumed_input = InputId::new();
        let resumed = fixture.context(
            fixture.session.clone(),
            resumed_input.clone(),
            fixture.identity(&resumed_input),
        );
        assert!(fixture.adapter.acquisition_original(&resumed).await.is_ok());
    }

    #[tokio::test]
    async fn correct_digest_cannot_select_a_different_session_or_current_input() {
        let fixture = Fixture::new(true).await;
        let wrong_session = fixture.context(
            SessionId::new(),
            fixture.input.clone(),
            fixture.identity(&fixture.input),
        );
        let wrong_input = fixture.context(
            fixture.session.clone(),
            InputId::new(),
            fixture.identity(&fixture.input),
        );
        for context in [wrong_session, wrong_input] {
            assert!(matches!(
                fixture.adapter.acquisition_original(&context).await,
                Err(PrivateOriginalError::Denied)
            ));
        }
        assert!(matches!(
            fixture
                .adapter
                .acquisition_original(&ToolDispatchContext::default())
                .await,
            Err(PrivateOriginalError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn missing_or_conflicting_staged_identity_never_chooses_an_original() {
        let fixture = Fixture::new(true).await;
        let retained = fixture.identity(&fixture.input);
        let mut conflicting = retained.contributors()[0].clone();
        conflicting.submission_digest = "b".repeat(64);
        for (contributors, expected) in [
            (vec![], PrivateOriginalError::Denied),
            (
                vec![retained.contributors()[0].clone(), conflicting],
                PrivateOriginalError::Conflict,
            ),
        ] {
            let changed = RetainedWorkIdentity::new(
                retained.runtime_id(),
                retained.run_id().clone(),
                contributors,
                retained.selected_input_bindings().clone(),
                retained.controller().cloned(),
            );
            let context = fixture.context(fixture.session.clone(), fixture.input.clone(), changed);
            assert!(
                matches!(fixture.adapter.acquisition_original(&context).await, Err(error) if error == expected)
            );
        }
        let repeated = RetainedWorkIdentity::new(
            retained.runtime_id(),
            retained.run_id().clone(),
            vec![
                retained.contributors()[0].clone(),
                retained.contributors()[0].clone(),
            ],
            retained.selected_input_bindings().clone(),
            retained.controller().cloned(),
        );
        let context = fixture.context(fixture.session.clone(), fixture.input.clone(), repeated);
        let resolved = fixture
            .adapter
            .acquisition_original(&context)
            .await
            .unwrap();
        assert_eq!(resolved.contributors.len(), 1);
        assert!(Arc::ptr_eq(&resolved.contributors[0], &fixture.original));
    }

    #[tokio::test]
    async fn batched_and_resumed_work_keep_every_staged_source_under_the_native_anchor() {
        let fixture = Fixture::new(true).await;
        let (child, child_reference, child_input) =
            fixture.descendant("child", &fixture.original).await;
        let first = fixture.identity(&fixture.input);
        let second = fixture.identity_for(&child, &child_input, &child_input);
        let contributors = vec![
            first.contributors()[0].clone(),
            second.contributors()[0].clone(),
        ];
        let mut bindings = first.selected_input_bindings().clone();
        bindings.extend(second.selected_input_bindings().clone());
        let batched = RetainedWorkIdentity::new(
            first.runtime_id(),
            first.run_id().clone(),
            contributors.clone(),
            bindings,
            first.controller().cloned(),
        );
        let context = fixture.context(fixture.session.clone(), fixture.input.clone(), batched);
        let resolved = fixture
            .adapter
            .acquisition_original(&context)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&resolved.original, &fixture.original));
        assert_eq!(resolved.contributors.len(), 2);
        assert!(Arc::ptr_eq(&resolved.contributors[0], &fixture.original));
        assert!(Arc::ptr_eq(&resolved.contributors[1], &child));
        let coalesced = RetainedWorkIdentity::new(
            first.runtime_id(),
            first.run_id().clone(),
            vec![
                second.contributors()[0].clone(),
                first.contributors()[0].clone(),
            ],
            second.selected_input_bindings().clone(),
            first.controller().cloned(),
        );
        let coalesced_context =
            fixture.context(fixture.session.clone(), child_input.clone(), coalesced);
        let coalesced = fixture
            .adapter
            .acquisition_original(&coalesced_context)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&coalesced.original, &child));
        assert_eq!(coalesced.contributors.len(), 2);
        assert!(Arc::ptr_eq(&coalesced.contributors[1], &fixture.original));
        let resumed_input = InputId::new();
        let resumed = RetainedWorkIdentity::new(
            first.runtime_id(),
            first.run_id().clone(),
            contributors,
            BTreeMap::from([(
                resumed_input.to_string(),
                first.selected_input_bindings()[&fixture.input.to_string()].clone(),
            )]),
            first.controller().cloned(),
        );
        let context = fixture.context(fixture.session.clone(), resumed_input, resumed);
        let resolved = fixture
            .adapter
            .acquisition_original(&context)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&resolved.original, &fixture.original));
        assert_eq!(resolved.contributors.len(), 2);
        assert!(Arc::ptr_eq(&resolved.contributors[1], &child));
        fixture
            .adapter
            .record_operation_cancelled(&child_reference)
            .await
            .unwrap();
        assert!(matches!(
            fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Denied)
        ));
    }

    #[tokio::test]
    async fn foreign_staged_source_is_not_dropped_when_the_anchor_matches() {
        let fixture = Fixture::new(true).await;
        let foreign = Fixture::new(true).await;
        let first = fixture.identity(&fixture.input);
        let other = foreign.identity(&foreign.input);
        let context = fixture.context(
            fixture.session.clone(),
            fixture.input.clone(),
            RetainedWorkIdentity::new(
                first.runtime_id(),
                first.run_id().clone(),
                vec![
                    first.contributors()[0].clone(),
                    other.contributors()[0].clone(),
                ],
                first.selected_input_bindings().clone(),
                first.controller().cloned(),
            ),
        );
        assert!(matches!(
            fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn live_own_lookup_does_not_wait_for_or_promote_source_acknowledgement() {
        let mut fixture = Fixture::new(false).await;
        let context = fixture.context(
            fixture.session.clone(),
            fixture.input.clone(),
            fixture.identity(&fixture.input),
        );
        let c = fixture.original.association().candidate();
        let retained_basis = || fixture.adapter.owner
            .admitted_original_basis(&c.ingress_namespace, &c.original_work).unwrap();
        assert!(retained_basis().is_none());
        let resolved = fixture.adapter.acquisition_original(&context).await.unwrap();
        assert!(Arc::ptr_eq(&resolved.original, &fixture.original));
        assert_eq!(resolved.own_originals.len(), 1);
        assert!(Arc::ptr_eq(&resolved.own_originals[0], &fixture.original));
        assert!(retained_basis().is_none());
        // A later named use still waits for the real acknowledgement. Lookup
        // did not perform that source transition on behalf of native admission.
        assert!(matches!(fixture.adapter.require_acquisition_source(
            fixture.original.association(), false,
        ), Err(PrivateOriginalError::Unavailable)));
        fixture.pending.take().unwrap().admit().unwrap();
        fixture.adapter.require_acquisition_source(fixture.original.association(), false).unwrap();
        let after = fixture.adapter.acquisition_original(&context).await.unwrap();
        assert_eq!(after.source_record.content_digest, resolved.source_record.content_digest);
    }

    #[tokio::test]
    async fn own_lookup_never_recreates_withdrawn_custody_or_ignores_cancellation() {
        let mut fixture = Fixture::new(false).await;
        let context = fixture.context(
            fixture.session.clone(), fixture.input.clone(), fixture.identity(&fixture.input),
        );
        drop(fixture.pending.take());
        fixture.adapter.acquisition_original(&context).await.unwrap();
        let c = fixture.original.association().candidate();
        assert!(fixture.adapter.owner.admitted_original_basis(
            &c.ingress_namespace, &c.original_work,
        ).unwrap().is_none());
        fixture.adapter.record_operation_cancelled(&fixture.reference).await.unwrap();
        assert!(matches!(fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Denied)));
    }

    #[tokio::test]
    async fn named_only_ancestor_requires_retention_but_exact_staged_ancestor_is_own() {
        let fixture = Fixture::new(true).await;
        let (child, _, input) = fixture.descendant("child", &fixture.original).await;
        let child_identity = fixture.identity_for(&child, &input, &input);
        let child_context = fixture.context(
            fixture.session.clone(), input.clone(), child_identity.clone(),
        );
        fixture.adapter.owner.release_original(&fixture.original).unwrap();
        // The child is current and retained, but its named-only parent is not.
        assert!(matches!(fixture.adapter.acquisition_original(&child_context).await,
            Err(PrivateOriginalError::Unavailable)));
        let root_identity = fixture.identity(&fixture.input);
        let context = fixture.context(
            fixture.session.clone(), input,
            RetainedWorkIdentity::new(
                child_identity.runtime_id(), child_identity.run_id().clone(),
                vec![child_identity.contributors()[0].clone(), root_identity.contributors()[0].clone()],
                child_identity.selected_input_bindings().clone(), child_identity.controller().cloned(),
            ),
        );
        let resolved = fixture.adapter.acquisition_original(&context).await.unwrap();
        assert_eq!(resolved.own_originals.len(), 2);
        assert_eq!(resolved.contributors.len(), 2);
        assert!(Arc::ptr_eq(&resolved.original, &child));
        assert!(Arc::ptr_eq(&resolved.own_originals[1], &fixture.original));
        let c = fixture.original.association().candidate();
        assert!(fixture.adapter.owner.admitted_original_basis(
            &c.ingress_namespace, &c.original_work,
        ).unwrap().is_none());
    }

    #[tokio::test]
    async fn keeps_every_recorded_ancestor_and_refuses_grandparent_cancellation() {
        let fixture = Fixture::new(true).await;
        let (parent, _, _) = fixture.descendant("parent", &fixture.original).await;
        let (child, _, input) = fixture.descendant("child", &parent).await;
        let context = fixture.context(
            fixture.session.clone(),
            input.clone(),
            fixture.identity_for(&child, &input, &input),
        );
        let resolved = fixture
            .adapter
            .acquisition_original(&context)
            .await
            .unwrap();
        assert_eq!(resolved.contributors.len(), 3);
        for (actual, expected) in
            resolved
                .contributors
                .iter()
                .zip([&child, &parent, &fixture.original])
        {
            assert!(Arc::ptr_eq(actual, expected));
        }
        // The direct child's own retained basis remains available. Its exact
        // canonical relationship must not erase the grandparent's cancellation.
        fixture
            .adapter
            .record_operation_cancelled(&fixture.reference)
            .await
            .unwrap();
        let child_claims = child.association().candidate();
        assert!(
            fixture
                .adapter
                .owner
                .admitted_original_basis(&child_claims.ingress_namespace, &child_claims.original_work)
                .unwrap()
                .is_some()
        );
        assert!(matches!(
            fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Denied)
        ));
    }

    #[tokio::test]
    async fn a_missing_recorded_ancestor_cannot_be_replaced_by_the_child_basis() {
        let fixture = Fixture::new(true).await;
        let (child, _, input) = fixture.descendant("child", &fixture.original).await;
        let context = fixture.context(
            fixture.session.clone(),
            input.clone(),
            fixture.identity_for(&child, &input, &input),
        );
        assert!(fixture.adapter.acquisition_original(&context).await.is_ok());
        // Remove only this adapter's retained parent handle. Native still holds
        // the parent and child, which is insufficient for canonical source readback.
        fixture
            .adapter
            .retained
            .lock()
            .unwrap()
            .retain(|held| !Arc::ptr_eq(&held.original, &fixture.original));
        assert!(matches!(
            fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn cancellation_and_foreign_source_owner_refuse_even_with_matching_native_identity() {
        let fixture = Fixture::new(true).await;
        let context = fixture.context(
            fixture.session.clone(),
            fixture.input.clone(),
            fixture.identity(&fixture.input),
        );
        let other = Fixture::new(true).await;
        assert!(matches!(
            other.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        fixture
            .adapter
            .record_operation_cancelled(&fixture.reference)
            .await
            .unwrap();
        assert!(matches!(
            fixture.adapter.acquisition_original(&context).await,
            Err(PrivateOriginalError::Denied)
        ));
    }
}
