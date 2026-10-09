//! Governed resume of retained work: an internal completion (a fork_off or
//! council outcome) admitted to the runtime that staged the original run.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;

use super::MeerkatMachine;
use super::driver::DriverEntry;
use crate::accept::AcceptOutcome;
use crate::input::Input;
use crate::input_state::InputState;
use crate::retained_work::{
    RetainedResumeEvidence, RetainedResumeGrant, RetainedResumeRequest, resolve_contributors,
};
use crate::traits::{RuntimeDriver as _, RuntimeDriverError};

impl MeerkatMachine {
    /// Admit `input` as a governed resume of the retained work `request`
    /// names, into `session_id`'s runtime.
    ///
    /// The runtime resolves the work's original contributors from this
    /// runtime's own retained rows (live or archived) and checks they are
    /// exact; the native work authorization host then validates the evidence
    /// and the current original invocation, grant and ceilings, and supplies
    /// the usable controller client. The input is admitted through the normal
    /// governed path with runtime-minted custody bound to it, so the driver
    /// re-validates under its own custody.
    ///
    /// # Errors
    /// [`RuntimeDriverError::RetainedResumeRefused`] for a terminal policy
    /// outcome (an immutably missing or invalid binding, an actual denial);
    /// any other error is an unavailable owner or an infrastructure failure
    /// and is retryable.
    pub async fn accept_retained_resume(
        &self,
        session_id: &SessionId,
        mut input: Input,
        request: RetainedResumeRequest,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        let host = self
            .native_work_authorization_host
            .get()
            .ok_or_else(crate::input_authority::unavailable)?
            .host()
            .clone();
        if input.header().authority_association.is_some()
            || input.header().ingress_context.is_some()
            || input.header().retained_resume.is_some()
        {
            return Err(crate::input_authority::unavailable());
        }
        let runtime_id = Self::logical_runtime_id(session_id);
        let contributors = self
            .resolve_retained_work_contributors(session_id, &request.identity)
            .await?;
        let evidence = RetainedResumeEvidence {
            identity: request.identity,
            delivery: request.delivery,
            contributors,
        };
        let client = host
            .authenticate_retained_resume(&runtime_id, &input, &evidence)
            .map_err(crate::retained_work::classify_resume_error)?;
        if evidence.identity.controller() != Some(client.selection()) {
            return Err(crate::input_authority::unavailable());
        }
        let grant = RetainedResumeGrant {
            submitted_input_id: input.id().clone(),
            replay_digest: crate::input_authority::replay_digest(&input)?,
            evidence,
            controller_client: client,
        };
        input.header_mut().retained_resume = Some(Arc::new(grant));
        self.accept_input_with_completion(session_id, input).await
    }
}

impl MeerkatMachine {
    /// Resolve `identity`'s original contributors from this session's own
    /// rows: the live driver ledger by exact input id, then the archived
    /// store for rows that left it, compared through
    /// [`resolve_contributors`] (runtime, controller, association and
    /// submission digests, selected bindings, every contributor in staged
    /// order).
    ///
    /// The owned row data is extracted under the session and driver guards,
    /// which are dropped before any archived store I/O. This proves the rows
    /// still exist and are exact; it does not prove the run was admitted or
    /// that the contributor set is complete, which only an admission against
    /// the actual staged batch does. A session that is not registered is
    /// `NotReady` (infrastructure), not a missing row.
    pub(crate) async fn resolve_retained_work_contributors(
        &self,
        session_id: &SessionId,
        identity: &meerkat_core::retained_work::RetainedWorkIdentity,
    ) -> Result<Vec<crate::input_authority::RetainedInputAuthority>, RuntimeDriverError> {
        let runtime_id = Self::logical_runtime_id(session_id);
        let driver = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: crate::runtime_state::RuntimeState::Destroyed,
                })?
                .driver
                .clone()
        };
        let mut rows: HashMap<InputId, InputState> = HashMap::new();
        {
            let driver = driver.lock().await;
            for contributor in identity.contributors() {
                let live = match &*driver {
                    DriverEntry::Ephemeral(driver) => driver.input_state(&contributor.input_id),
                    DriverEntry::Persistent(driver) => {
                        driver.inner_ref().input_state(&contributor.input_id)
                    }
                };
                if let Some(state) = live {
                    rows.insert(contributor.input_id.clone(), state.clone());
                }
            }
        }
        drop(driver);
        // Terminal rows leave the live ledger once durably committed; their
        // retained authority stays on the stored row.
        if let Some(store) = self.store.as_ref() {
            for contributor in identity.contributors() {
                if rows.contains_key(&contributor.input_id) {
                    continue;
                }
                let stored = store
                    .load_input_state(&runtime_id, &contributor.input_id)
                    .await
                    .map_err(|error| {
                        RuntimeDriverError::Internal(format!(
                            "retained work contributor could not be read: {error}"
                        ))
                    })?;
                if let Some(stored) = stored {
                    rows.insert(contributor.input_id.clone(), stored.state);
                }
            }
        }
        resolve_contributors(&runtime_id, identity, |input_id| {
            Ok(rows.get(input_id).cloned())
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::BTreeMap;

    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
    use meerkat_core::ControllerModelSelection;

    use super::*;
    use crate::delivery_inbox::{
        RuntimeDeliveryId, RuntimeDeliveryInbox, RuntimeDeliveryKind, RuntimeDeliverySubmission,
    };
    use crate::identifiers::LogicalRuntimeId;
    use crate::input_authority::tests::TestIngress;
    use crate::input_authority::{NativeIngressContext, RetainedInputAuthority};
    use crate::retained_work::RetainedResumeRefusal;
    use crate::store::InMemoryRuntimeStore;

    struct Client(ControllerModelSelection);
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl meerkat_core::AgentLlmClient for Client {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(self.0.clone())
        }
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            panic!("retained resume fixture never dispatches a model")
        }
        fn provider(&self) -> meerkat_core::Provider {
            self.0.provider()
        }
        fn model(&self) -> &str {
            self.0.model()
        }
    }

    type Driver = Arc<crate::tokio::sync::Mutex<DriverEntry>>;

    struct Governed {
        machine: MeerkatMachine,
        session_id: SessionId,
        host: Arc<TestIngress>,
        driver: Driver,
        runtime_id: LogicalRuntimeId,
    }

    async fn governed_with(
        host: impl FnOnce(
            &MeerkatMachine,
        ) -> (
            Arc<TestIngress>,
            Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>,
        ),
    ) -> Governed {
        let machine = MeerkatMachine::ephemeral();
        let (test_host, installed) = host(&machine);
        let machine = machine
            .with_native_work_authorization_host(installed)
            .expect("configured host");
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register");
        let (driver, runtime_id) = {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("session");
            (Arc::clone(&entry.driver), entry.runtime_id.clone())
        };
        {
            let mut driver = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *driver else {
                panic!("storeless")
            };
            driver.set_executor_work_authorization_support(true);
        }
        Governed {
            machine,
            session_id,
            host: test_host,
            driver,
            runtime_id,
        }
    }

    async fn governed() -> Governed {
        governed_with(|machine| {
            let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
            (Arc::clone(&host), host)
        })
        .await
    }

    /// Admit an authenticated original prompt from `requester`, as the
    /// governed ingress does, and return its retained authority.
    async fn admit_original(fixture: &Governed, requester: &str) -> RetainedInputAuthority {
        let mut prompt = fixture.host.input(requester);
        let ingress = Arc::clone(prompt.header().ingress_context.as_ref().expect("ingress"));
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        candidate.target.logical_runtime =
            EvidenceId::new(fixture.runtime_id.to_string()).expect("runtime id");
        let selection = candidate.controller_model.clone().expect("selection");
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        let actual = NativeIngressContext::from_trusted_ingress(
            &prompt,
            ingress.requester().clone(),
            ingress.ingress_actor().clone(),
            ingress.realm().clone(),
            ingress.authentication().clone(),
        )
        .expect("bound submission")
        .with_controller_client(
            &prompt,
            meerkat_core::ControllerModelClient::new(
                selection.clone(),
                Arc::new(Client(selection)),
            ),
        )
        .expect("same controller");
        let prompt = prompt.with_ingress_context(actual).expect("context");
        let mut driver = fixture.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *driver else {
            panic!("storeless")
        };
        let AcceptOutcome::Accepted { state, .. } = driver
            .accept_input(prompt)
            .await
            .expect("original admitted")
        else {
            panic!("original admitted")
        };
        state
            .authority_contributors
            .iter()
            .find(|retained| retained.input_id() == &state.input_id)
            .cloned()
            .expect("retained authority")
    }

    fn identity_for(
        runtime_id: &LogicalRuntimeId,
        originals: &[RetainedInputAuthority],
        selection: &ControllerModelSelection,
    ) -> meerkat_core::retained_work::RetainedWorkIdentity {
        let selected: BTreeMap<String, (String, String)> = originals
            .iter()
            .map(|original| {
                let (binding, batch) =
                    crate::input_authority::association_binding(original.association())
                        .expect("binding");
                (
                    original.input_id().to_string(),
                    (binding.expect("binding"), batch.expect("batch")),
                )
            })
            .collect();
        crate::retained_work::identity_from(
            runtime_id,
            meerkat_core::lifecycle::RunId::new(),
            originals,
            &selected,
            Some(selection.clone()),
        )
        .expect("identity")
    }

    /// The committed head row a delivery owner would be draining, and the
    /// request the inbox mints from it.
    async fn request_for(
        runtime_id: &LogicalRuntimeId,
        identity: meerkat_core::retained_work::RetainedWorkIdentity,
    ) -> RetainedResumeRequest {
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        inbox
            .submit(
                runtime_id,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new("continuation:resume").expect("id"),
                    RuntimeDeliveryKind::Continuation,
                    "fork_off",
                    1,
                    "fork_off:job-1",
                    b"completion".to_vec(),
                )
                .expect("submission"),
            )
            .await
            .expect("commit");
        inbox
            .retained_resume_request(
                runtime_id,
                &RuntimeDeliveryId::new("continuation:resume").expect("id"),
                1,
                move |_stored| Ok(identity),
            )
            .await
            .expect("request from the head row")
    }

    fn continuation(body: &str) -> Input {
        Input::Prompt(crate::input::PromptInput::continuation(
            InputId::new(),
            "fork_off:job-1",
            body.into(),
            meerkat_core::types::HandlingMode::Queue,
        ))
    }

    /// Launch A, admit an unrelated B, complete A: the resume carries A's
    /// originals and controller, never B's.
    #[tokio::test]
    async fn a_resume_retains_its_own_originals_never_a_later_input() {
        let fixture = governed().await;
        let a = admit_original(&fixture, "caller-a").await;
        let b = admit_original(&fixture, "caller-b").await;
        let identity = identity_for(
            &fixture.runtime_id,
            std::slice::from_ref(&a),
            fixture.host.selection(),
        );
        let request = request_for(&fixture.runtime_id, identity.clone()).await;
        let (outcome, _) = fixture
            .machine
            .accept_retained_resume(&fixture.session_id, continuation("done"), request)
            .await
            .expect("resume admitted");
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        assert_eq!(state.authority_contributors, vec![a]);
        assert!(!state.authority_contributors.contains(&b));
        assert_eq!(
            state.retained_resume.map(|record| record.identity),
            Some(identity)
        );
    }

    /// An actual native denial is AuthorityDenied; nothing is admitted.
    #[tokio::test]
    async fn a_denied_resume_is_refused_as_authority_denied() {
        let fixture = governed().await;
        let a = admit_original(&fixture, "caller-a").await;
        fixture.host.deny_retained_resume();
        let identity = identity_for(
            &fixture.runtime_id,
            std::slice::from_ref(&a),
            fixture.host.selection(),
        );
        let request = request_for(&fixture.runtime_id, identity).await;
        assert!(matches!(
            fixture
                .machine
                .accept_retained_resume(&fixture.session_id, continuation("done"), request)
                .await,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: RetainedResumeRefusal::AuthorityDenied
            })
        ));
    }

    /// Originals this runtime never admitted (another runtime's rows) are an
    /// immutably invalid binding.
    #[tokio::test]
    async fn originals_this_runtime_does_not_hold_are_no_admissible_binding() {
        let fixture = governed().await;
        let elsewhere = governed().await;
        let foreign = admit_original(&elsewhere, "caller-a").await;
        let identity = identity_for(
            &fixture.runtime_id,
            std::slice::from_ref(&foreign),
            fixture.host.selection(),
        );
        let request = request_for(&fixture.runtime_id, identity).await;
        assert!(matches!(
            fixture
                .machine
                .accept_retained_resume(&fixture.session_id, continuation("done"), request)
                .await,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: RetainedResumeRefusal::NoAdmissibleWorkBinding
            })
        ));
    }

    /// A host that does not implement retained resume admits none, and that
    /// is retryable (an unavailable owner), never a refusal.
    #[tokio::test]
    async fn a_host_without_retained_resume_support_is_unavailable_not_refused() {
        struct WithoutResume(Arc<TestIngress>);
        impl crate::input_authority::NativeWorkAuthorizationHost for WithoutResume {
            fn authenticate_association(
                &self,
                runtime_id: &LogicalRuntimeId,
                input: &Input,
                ingress: &NativeIngressContext,
                association: &InputAuthorityAssociation,
            ) -> Result<(), crate::input_authority::NativeAdmissionError> {
                self.0
                    .authenticate_association(runtime_id, input, ingress, association)
            }
            fn work_context(
                &self,
                batch: &crate::input_authority::NativeWorkBatch,
            ) -> Result<
                meerkat_core::WorkAuthorizationContext,
                crate::input_authority::NativeWorkContextError,
            > {
                self.0.work_context(batch)
            }
        }
        let fixture = governed_with(|machine| {
            let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
            (Arc::clone(&host), Arc::new(WithoutResume(host)))
        })
        .await;
        let a = admit_original(&fixture, "caller-a").await;
        let identity = identity_for(
            &fixture.runtime_id,
            std::slice::from_ref(&a),
            fixture.host.selection(),
        );
        let request = request_for(&fixture.runtime_id, identity).await;
        let error = fixture
            .machine
            .accept_retained_resume(&fixture.session_id, continuation("done"), request)
            .await
            .expect_err("no retained resume support");
        assert!(
            matches!(
                error,
                RuntimeDriverError::ControllerReadinessUnavailable { .. }
            ),
            "retryable, got {error:?}"
        );
    }

    /// Only the committed head row mints a request, reread from the store: a
    /// later row, another sequence or an uncommitted id never does.
    #[tokio::test]
    async fn only_the_unchanged_head_row_mints_a_request() {
        let fixture = governed().await;
        let a = admit_original(&fixture, "caller-a").await;
        let identity = identity_for(
            &fixture.runtime_id,
            std::slice::from_ref(&a),
            fixture.host.selection(),
        );
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        for (id, payload) in [
            ("continuation:first", b"first".to_vec()),
            ("continuation:second", b"second".to_vec()),
        ] {
            inbox
                .submit(
                    &fixture.runtime_id,
                    RuntimeDeliverySubmission::new(
                        RuntimeDeliveryId::new(id).expect("id"),
                        RuntimeDeliveryKind::Continuation,
                        "fork_off",
                        1,
                        id,
                        payload,
                    )
                    .expect("submission"),
                )
                .await
                .expect("commit");
        }
        for (delivery_id, sequence, what) in [
            ("continuation:second", 2, "a row behind the head"),
            (
                "continuation:first",
                2,
                "the head's id under another sequence",
            ),
            ("continuation:never", 1, "an id the inbox never committed"),
        ] {
            let identity = identity.clone();
            assert!(
                inbox
                    .retained_resume_request(
                        &fixture.runtime_id,
                        &RuntimeDeliveryId::new(delivery_id).expect("id"),
                        sequence,
                        move |_stored| Ok(identity),
                    )
                    .await
                    .is_err(),
                "{what}"
            );
        }
        let request = inbox
            .retained_resume_request(
                &fixture.runtime_id,
                &RuntimeDeliveryId::new("continuation:first").expect("id"),
                1,
                move |stored| {
                    assert_eq!(
                        stored.payload(),
                        b"first",
                        "the extractor reads the stored row"
                    );
                    Ok(identity)
                },
            )
            .await
            .expect("the committed head row");
        assert_eq!(request.delivery().delivery_sequence, 1);
    }
}
