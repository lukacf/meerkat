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
    /// outcome (an immutably missing or invalid binding, a destination its
    /// session has since bound a later generation of, an actual denial); any
    /// other error is an unavailable owner or an infrastructure failure and
    /// is retryable.
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
        // A resume whose destination its session has since moved can never
        // be admitted, whatever its owners answer now. It is refused for good
        // before any owner is asked, so an owner that is not ready for the
        // work (no client for its controller any more) cannot keep it owed.
        if self
            .retained_destination_superseded(session_id, &contributors)
            .await?
        {
            return Err(RuntimeDriverError::RetainedResumeRefused {
                reason: crate::retained_work::RetainedResumeRefusal::NoAdmissibleWorkBinding,
            });
        }
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

    /// Whether `session_id`'s session document has superseded the
    /// destination of the work whose resolved original contributors are
    /// `contributors` (the canonical, first, original's target): the rule the
    /// driver applies at admission, read now under the session's driver
    /// lock. Generations only advance, so a read that is already stale can
    /// only miss a supersession, which the driver's check then finds.
    async fn retained_destination_superseded(
        &self,
        session_id: &SessionId,
        contributors: &[crate::input_authority::RetainedInputAuthority],
    ) -> Result<bool, RuntimeDriverError> {
        let Some(canonical) = contributors.first() else {
            return Ok(false);
        };
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
        let driver = driver.lock().await;
        let target = &canonical.association().candidate().target;
        match &*driver {
            DriverEntry::Ephemeral(driver) => driver.scoped_destination_superseded(target),
            DriverEntry::Persistent(driver) => {
                driver.inner_ref().scoped_destination_superseded(target)
            }
        }
    }
}

impl MeerkatMachine {
    /// Resolve `identity`'s original contributors from this session's own
    /// rows: the live driver ledger by exact input id, then the archived
    /// store for rows that left it, compared through
    /// [`resolve_contributors`] (runtime, controller, association and
    /// submission digests, selected bindings, every contributor in staged
    /// order). The rows of selected keys that are not contributors (the
    /// staged resume inputs of a resumed run) are read the same way, and
    /// resolution verifies them.
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
        let wanted: Vec<InputId> = identity
            .contributors()
            .iter()
            .map(|contributor| contributor.input_id.clone())
            .chain(
                identity
                    .selected_input_bindings()
                    .keys()
                    .filter_map(|key| crate::retained_work::selected_input_id(key))
                    .filter(|input_id| {
                        !identity
                            .contributors()
                            .iter()
                            .any(|contributor| &contributor.input_id == input_id)
                    }),
            )
            .collect();
        let mut rows: HashMap<InputId, InputState> = HashMap::new();
        {
            let driver = driver.lock().await;
            for input_id in &wanted {
                let live = match &*driver {
                    DriverEntry::Ephemeral(driver) => driver.input_state(input_id),
                    DriverEntry::Persistent(driver) => driver.inner_ref().input_state(input_id),
                };
                if let Some(state) = live {
                    rows.insert(input_id.clone(), state.clone());
                }
            }
        }
        drop(driver);
        // Terminal rows leave the live ledger once durably committed; their
        // retained authority stays on the stored row.
        if let Some(store) = self.store.as_ref() {
            for input_id in &wanted {
                if rows.contains_key(input_id) {
                    continue;
                }
                let stored = store
                    .load_input_state(&runtime_id, input_id)
                    .await
                    .map_err(|error| {
                        RuntimeDriverError::Internal(format!(
                            "retained work contributor could not be read: {error}"
                        ))
                    })?;
                if let Some(stored) = stored {
                    rows.insert(input_id.clone(), stored.state);
                }
            }
        }
        resolve_contributors(&runtime_id, identity, |input_id| {
            Ok(rows.get(input_id).cloned())
        })
    }
}

/// The queued inputs [`DriverEntry::settle_refused_queued_resumes`] ended,
/// each with the directed owner of its carrier when it is directed, and the
/// error that stopped it before the rest, if one did. The ended ones are
/// committed whatever came after: the caller publishes them either way.
#[derive(Debug, Default)]
pub(crate) struct SettledRefusedResumes {
    pub(crate) settled: Vec<(InputId, Option<InputId>)>,
    pub(crate) failure: Option<RuntimeDriverError>,
}

impl DriverEntry {
    /// Ask the installed host again for the controller client of every kept
    /// retained-resume row that holds none, and realize each answer on the
    /// machine (see
    /// [`crate::driver::ephemeral::EphemeralRuntimeDriver::ask_kept_resume_controllers`]).
    pub(crate) fn ask_kept_resume_controllers(
        &mut self,
        now: meerkat_core::time_compat::Instant,
    ) -> Result<crate::retained_work::KeptResumeControllerAsks, RuntimeDriverError> {
        self.shell_driver_mut().ask_kept_resume_controllers(now)
    }

    /// Whether the runtime loop has an ask to make for kept retained-resume
    /// rows without a controller client at `now`.
    pub(crate) fn kept_resume_controller_ask_due(
        &self,
        now: meerkat_core::time_compat::Instant,
    ) -> bool {
        self.shell_driver().kept_resume_controller_ask_due(now)
    }

    /// When the runtime loop next asks again for the clients of the rows the
    /// machine holds for their controller; `None` while it holds none.
    pub(crate) fn controller_reask_due_at(&self) -> Option<meerkat_core::time_compat::Instant> {
        self.shell_driver().controller_reask_due_at()
    }

    /// Whether the machine holds an input bound to `run_id` for its
    /// controller.
    pub(crate) fn run_inputs_controller_held(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        self.shell_driver().run_inputs_controller_held(run_id)
    }

    /// Whether `input_id` is a kept resume of retained work.
    pub(crate) fn is_kept_retained_resume(&self, input_id: &InputId) -> bool {
        self.shell_driver().is_kept_retained_resume(input_id)
    }

    /// Whether `input_id` is a kept resume of retained work that holds no
    /// controller client (yet).
    pub(crate) fn kept_retained_resume_without_client(&self, input_id: &InputId) -> bool {
        self.shell_driver()
            .kept_retained_resume_without_client(input_id)
    }

    /// Whether `input_id` is a kept resume of retained work whose destination
    /// its session has since superseded.
    pub(crate) fn kept_retained_resume_destination_superseded(
        &self,
        input_id: &InputId,
    ) -> Result<bool, RuntimeDriverError> {
        self.shell_driver()
            .kept_retained_resume_destination_superseded(input_id)
    }

    /// End each still queued input of `input_ids` as a refused resume
    /// (`Abandoned { ResumeRefused }`) through the machine's `AbandonInput`,
    /// before any of them stages: kept retained-resume rows whose installed
    /// host answered definitely that it supplies no controller for the
    /// retained work they resume (or did not answer within their budget),
    /// and kept governed inputs with no way back to their controller client.
    /// Each ends in its own durable commit with the runless
    /// runtime-termination carrier it is owed (`reason`), as an exact
    /// queued-input cancellation ends one, and `owe` runs as soon as each
    /// one commits, before the next is tried, so an error after it never
    /// loses the announcement of a committed one. An input that is no longer
    /// queued is left as it is. The caller publishes and resolves the ended
    /// inputs once it releases the driver.
    pub(crate) async fn settle_refused_queued_resumes(
        &mut self,
        input_ids: &[InputId],
        reason: &str,
        mut owe: impl FnMut(),
    ) -> SettledRefusedResumes {
        let mut outcome = SettledRefusedResumes::default();
        for input_id in input_ids {
            if self.input_phase(input_id) != Some(crate::input_state::InputLifecycleState::Queued) {
                continue;
            }
            let prepared =
                match self.prepare_sole_runless_runtime_termination(input_id, reason.to_string()) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        outcome.failure = Some(error);
                        return outcome;
                    }
                };
            match self
                .abandon_queued_input(
                    input_id,
                    crate::input_state::InputAbandonReason::ResumeRefused,
                )
                .await
            {
                Ok(true) => {
                    let directed_owner =
                        DriverEntry::commit_prepared_sole_runless_termination(prepared);
                    owe();
                    outcome.settled.push((input_id.clone(), directed_owner));
                }
                Ok(false) => self.rollback_prepared_sole_runless_termination(prepared),
                Err(error) => {
                    self.rollback_prepared_sole_runless_termination(prepared);
                    outcome.failure = Some(error);
                    return outcome;
                }
            }
        }
        outcome
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
        machine: Arc<MeerkatMachine>,
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
            machine: Arc::new(machine),
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

    /// A governed fixture whose session has a runtime binding, so its rows
    /// can be staged into runs the way the runtime loop stages them.
    async fn governed_staging() -> Governed {
        let machine = MeerkatMachine::ephemeral();
        let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
        let installed: Arc<dyn crate::input_authority::NativeWorkAuthorizationHost> =
            Arc::clone(&host) as Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>;
        let machine = machine
            .with_native_work_authorization_host(installed)
            .expect("configured host");
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register");
        machine
            .prepare_bindings(session_id.clone())
            .await
            .expect("runtime binding");
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
            machine: Arc::new(machine),
            session_id,
            host,
            driver,
            runtime_id,
        }
    }

    /// Stage `inputs` as one run the way the runtime loop does, and return
    /// the identity the driver minted for that staged work context.
    async fn stage_run(
        fixture: &Governed,
        inputs: &[InputId],
    ) -> meerkat_core::retained_work::RetainedWorkIdentity {
        let run = meerkat_core::lifecycle::RunId::new();
        let mut driver = fixture.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *driver else {
            panic!("storeless")
        };
        let context = driver
            .batch_work_authorization(&run, inputs)
            .expect("staged batch")
            .expect("work context");
        driver
            .contract_begin_run_authority(run.clone())
            .expect("actual run");
        driver
            .machine_realize_authorized_stage_batch(
                crate::meerkat_machine::driver::test_authorized_stage_for_run(
                    inputs.to_vec(),
                    run.clone(),
                ),
            )
            .expect("staged");
        context.retained_work().cloned().expect("minted identity")
    }

    /// End `run` the way a completed turn does (run terminal, then the
    /// service turn commit), so a later input is admitted and staged.
    async fn finish_run(fixture: &Governed, run: &meerkat_core::lifecycle::RunId) {
        use crate::meerkat_machine::dsl as mm;
        let entry = fixture.driver.lock().await;
        let shared = entry.shared_dsl_authority();
        let mut machine = shared.lock().expect("generated owner");
        mm::MeerkatMachineMutator::apply(
            &mut *machine,
            mm::MeerkatMachineInput::RunCompleted {
                run_id: mm::RunId::from_domain(run),
            },
        )
        .expect("run terminal");
        mm::MeerkatMachineMutator::apply(
            &mut *machine,
            mm::MeerkatMachineInput::ServiceTurnCommitted {
                run_id: mm::RunId::from_domain(run),
            },
        )
        .expect("service turn committed");
    }

    /// The request minted from a committed continuation row of its own
    /// delivery, carrying `identity`.
    async fn request_from_delivery(
        runtime_id: &LogicalRuntimeId,
        identity: meerkat_core::retained_work::RetainedWorkIdentity,
        delivery_id: &str,
        admission_key: &str,
    ) -> RetainedResumeRequest {
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        inbox
            .submit(
                runtime_id,
                RuntimeDeliverySubmission::new(
                    RuntimeDeliveryId::new(delivery_id).expect("id"),
                    RuntimeDeliveryKind::Continuation,
                    "fork_off",
                    1,
                    admission_key,
                    delivery_id.as_bytes().to_vec(),
                )
                .expect("submission"),
            )
            .await
            .expect("commit");
        inbox
            .retained_resume_request(
                runtime_id,
                &RuntimeDeliveryId::new(delivery_id).expect("id"),
                1,
                move |_stored| Ok(identity),
            )
            .await
            .expect("request from the head row")
    }

    fn keyed_continuation(admission_key: &str, body: &str) -> Input {
        Input::Prompt(crate::input::PromptInput::continuation(
            InputId::new(),
            admission_key,
            body.into(),
            meerkat_core::types::HandlingMode::Queue,
        ))
    }

    /// The exact chain a governed host runs: an original run retains work, a
    /// producer's completion resumes it, the resumed run is staged and
    /// retains work again, and that run's completion resumes the same
    /// original. The resumed run's identity, minted by the driver when it
    /// staged the resume input, names the original contributors; its
    /// continuation is admitted with them, never refused as an invalid
    /// binding.
    #[tokio::test]
    async fn a_continuation_of_a_resumed_run_resumes_the_same_original() {
        let fixture = governed_staging().await;
        let a = admit_original(&fixture, "caller-a").await;
        let original_run = stage_run(&fixture, std::slice::from_ref(a.input_id())).await;
        finish_run(&fixture, original_run.run_id()).await;

        let first = request_from_delivery(
            &fixture.runtime_id,
            original_run.clone(),
            "continuation:first",
            "fork_off:job-1",
        )
        .await;
        let (outcome, _) = fixture
            .machine
            .accept_retained_resume(
                &fixture.session_id,
                keyed_continuation("fork_off:job-1", "the first fork finished"),
                first,
            )
            .await
            .expect("the original run's continuation resumes it");
        let AcceptOutcome::Accepted {
            state: resumed_input,
            ..
        } = outcome
        else {
            panic!("accepted, got {outcome:?}")
        };

        let resumed_run = stage_run(&fixture, std::slice::from_ref(&resumed_input.input_id)).await;
        finish_run(&fixture, resumed_run.run_id()).await;
        assert_eq!(
            resumed_run.contributors(),
            original_run.contributors(),
            "the resumed run keeps the original's contributors"
        );

        let second = request_from_delivery(
            &fixture.runtime_id,
            resumed_run.clone(),
            "continuation:second",
            "fork_off:job-2",
        )
        .await;
        let (outcome, _) = fixture
            .machine
            .accept_retained_resume(
                &fixture.session_id,
                keyed_continuation("fork_off:job-2", "the second fork finished"),
                second,
            )
            .await
            .expect("the resumed run's continuation resumes the same original");
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        assert_eq!(state.authority_contributors, vec![a]);
        assert_eq!(
            state.retained_resume.map(|record| record.identity),
            Some(resumed_run)
        );
    }

    /// The chain up to a resumed run: original A staged as a run, a
    /// completion carrying that run's identity admitted as resume R1, and R1
    /// staged as the resumed run. Returns A, R1's id and the resumed run's
    /// minted identity.
    async fn resumed_run(
        fixture: &Governed,
        requester: &str,
        job: &str,
    ) -> (
        RetainedInputAuthority,
        InputId,
        meerkat_core::retained_work::RetainedWorkIdentity,
    ) {
        let original = admit_original(fixture, requester).await;
        let original_run = stage_run(fixture, std::slice::from_ref(original.input_id())).await;
        finish_run(fixture, original_run.run_id()).await;
        let request = request_from_delivery(
            &fixture.runtime_id,
            original_run,
            &format!("continuation:{job}"),
            job,
        )
        .await;
        let (outcome, _) = fixture
            .machine
            .accept_retained_resume(
                &fixture.session_id,
                keyed_continuation(job, "the fork finished"),
                request,
            )
            .await
            .expect("the original run's continuation resumes it");
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        let resumed = stage_run(fixture, std::slice::from_ref(&state.input_id)).await;
        finish_run(fixture, resumed.run_id()).await;
        (original, state.input_id, resumed)
    }

    /// Admit a continuation carrying `identity` under the admission key `job`.
    async fn resume_with(
        fixture: &Governed,
        identity: meerkat_core::retained_work::RetainedWorkIdentity,
        job: &str,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let request = request_from_delivery(
            &fixture.runtime_id,
            identity,
            &format!("continuation:{job}"),
            job,
        )
        .await;
        fixture
            .machine
            .accept_retained_resume(
                &fixture.session_id,
                keyed_continuation(job, "the fork finished"),
                request,
            )
            .await
            .map(|(outcome, _)| outcome)
    }

    /// `identity` with its selected rows replaced.
    fn with_selected(
        identity: &meerkat_core::retained_work::RetainedWorkIdentity,
        selected: BTreeMap<String, meerkat_core::retained_work::SelectedInputBinding>,
    ) -> meerkat_core::retained_work::RetainedWorkIdentity {
        meerkat_core::retained_work::RetainedWorkIdentity::new(
            identity.runtime_id(),
            identity.run_id().clone(),
            identity.contributors().to_vec(),
            selected,
            identity.controller().cloned(),
        )
    }

    /// `identity` with the selected row `from` keyed `to` instead, keeping
    /// its binding unless `binding` replaces it.
    fn rekeyed(
        identity: &meerkat_core::retained_work::RetainedWorkIdentity,
        from: &InputId,
        to: &str,
        binding: Option<meerkat_core::retained_work::SelectedInputBinding>,
    ) -> meerkat_core::retained_work::RetainedWorkIdentity {
        let mut selected = identity.selected_input_bindings().clone();
        let kept = selected
            .remove(&from.to_string())
            .expect("the selected resume row");
        selected.insert(to.to_string(), binding.unwrap_or(kept));
        with_selected(identity, selected)
    }

    fn no_admissible_binding(result: &Result<AcceptOutcome, RuntimeDriverError>) -> bool {
        matches!(
            result,
            Err(RuntimeDriverError::RetainedResumeRefused {
                reason: RetainedResumeRefusal::NoAdmissibleWorkBinding
            })
        )
    }

    /// A chain one level deeper: the run that staged the second resume
    /// retains work again, and its continuation still resumes the original.
    #[tokio::test]
    async fn a_continuation_two_resumes_deep_resumes_the_same_original() {
        let fixture = governed_staging().await;
        let (a, _, resumed) = resumed_run(&fixture, "caller-a", "fork_off:job-1").await;
        let outcome = resume_with(&fixture, resumed, "fork_off:job-2")
            .await
            .expect("the resumed run's continuation resumes the original");
        let AcceptOutcome::Accepted {
            state: second_resume,
            ..
        } = outcome
        else {
            panic!("accepted, got {outcome:?}")
        };
        let twice_resumed =
            stage_run(&fixture, std::slice::from_ref(&second_resume.input_id)).await;
        finish_run(&fixture, twice_resumed.run_id()).await;
        let outcome = resume_with(&fixture, twice_resumed.clone(), "fork_off:job-3")
            .await
            .expect("a continuation two resumes deep resumes the same original");
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        assert_eq!(state.authority_contributors, vec![a]);
        assert_eq!(
            state.retained_resume.map(|record| record.identity),
            Some(twice_resumed)
        );
    }

    /// A selected key that is not a contributor is accepted only as this
    /// runtime's own resume row of exactly the contributors, bound as staging
    /// bound it; every other key or binding is no admissible work binding.
    #[tokio::test]
    async fn a_selected_row_that_is_not_a_verified_resume_of_the_contributors_is_refused() {
        let fixture = governed_staging().await;
        let (_, r1, resumed) = resumed_run(&fixture, "caller-a", "fork_off:job-a").await;
        // An unrelated original B of this runtime, and a resume of B.
        let (b, rb, _) = resumed_run(&fixture, "caller-b", "fork_off:job-b").await;
        // Another runtime's resumed run, and its resume row.
        let elsewhere = governed_staging().await;
        let (_, foreign_resume, foreign_run) =
            resumed_run(&elsewhere, "caller-a", "fork_off:job-x").await;

        // B's own binding: a resume of B bound exactly as staging binds it, so
        // only the window check (its carried originals are not the
        // contributors) refuses it.
        let (b_binding, b_batch) =
            crate::input_authority::association_binding(b.association()).expect("binding");
        let bound_under_b = meerkat_core::retained_work::SelectedInputBinding {
            binding: b_binding.expect("binding"),
            batch_key: b_batch.expect("batch key"),
        };
        let mut changed_binding = resumed.selected_input_bindings()[&r1.to_string()].clone();
        changed_binding.binding = "another-binding".into();
        let mut changed_batch = resumed.selected_input_bindings()[&r1.to_string()].clone();
        changed_batch.batch_key = "another-batch".into();
        let cases = [
            (
                "another runtime's resumed run",
                foreign_run,
                "fork_off:case-foreign-run",
            ),
            (
                "another runtime's resume row",
                rekeyed(&resumed, &r1, &foreign_resume.to_string(), None),
                "fork_off:case-foreign-row",
            ),
            (
                "a selected key with no row",
                rekeyed(&resumed, &r1, &InputId::new().to_string(), None),
                "fork_off:case-no-row",
            ),
            (
                "a non-canonical key for the resume row",
                rekeyed(&resumed, &r1, &r1.to_string().to_uppercase(), None),
                "fork_off:case-non-canonical",
            ),
            (
                "a row that is not a resume",
                rekeyed(&resumed, &r1, &b.input_id().to_string(), None),
                "fork_off:case-not-a-resume",
            ),
            (
                "a resume of other originals",
                rekeyed(&resumed, &r1, &rb.to_string(), None),
                "fork_off:case-other-originals",
            ),
            (
                "a resume of other originals, bound under them",
                rekeyed(&resumed, &r1, &rb.to_string(), Some(bound_under_b)),
                "fork_off:case-other-originals-own-binding",
            ),
            (
                "a changed binding",
                rekeyed(&resumed, &r1, &r1.to_string(), Some(changed_binding)),
                "fork_off:case-binding",
            ),
            (
                "a changed batch key",
                rekeyed(&resumed, &r1, &r1.to_string(), Some(changed_batch)),
                "fork_off:case-batch-key",
            ),
        ];
        for (case, identity, job) in cases {
            let result = resume_with(&fixture, identity, job).await;
            assert!(no_admissible_binding(&result), "{case}: {result:?}");
        }
        // The exact resumed run still resumes its original.
        assert!(matches!(
            resume_with(&fixture, resumed, "fork_off:case-exact").await,
            Ok(AcceptOutcome::Accepted { .. })
        ));
    }

    /// A resumed run's continuation replayed is admitted once: the replay of
    /// the same input with the same delivery is deduplicated.
    #[tokio::test]
    async fn a_resumed_runs_continuation_replayed_is_admitted_once() {
        let fixture = governed_staging().await;
        let (_, _, resumed) = resumed_run(&fixture, "caller-a", "fork_off:job-1").await;
        let continuation = keyed_continuation("fork_off:job-2", "the second fork finished");
        let mut admitted = None;
        for attempt in ["first", "replay"] {
            let request = request_from_delivery(
                &fixture.runtime_id,
                resumed.clone(),
                "continuation:second",
                "fork_off:job-2",
            )
            .await;
            let (outcome, _) = fixture
                .machine
                .accept_retained_resume(&fixture.session_id, continuation.clone(), request)
                .await
                .expect("answered");
            match (attempt, outcome) {
                ("first", AcceptOutcome::Accepted { input_id, .. }) => admitted = Some(input_id),
                ("replay", AcceptOutcome::Deduplicated { existing_id, .. }) => {
                    assert_eq!(
                        Some(existing_id),
                        admitted,
                        "the replay names the admitted input"
                    );
                }
                (attempt, outcome) => panic!("{attempt}: {outcome:?}"),
            }
        }
    }

    /// The retained work of a resumed run resolves for MCP observation like
    /// the original's: the observation reaches the registration (which has
    /// no MCP adapter installed here). A resumed identity whose resume row is
    /// not held here is not current.
    #[tokio::test]
    async fn a_resumed_runs_retained_work_resolves_for_mcp_observation() {
        use crate::meerkat_machine::McpObservationRefusal;
        let fixture = governed_staging().await;
        let (_, r1, resumed) = resumed_run(&fixture, "caller-a", "fork_off:job-1").await;
        assert!(matches!(
            fixture
                .machine
                .observe_original_mcp_operation(&resumed, "server", "tool")
                .await,
            Err(McpObservationRefusal::NotInstalled)
        ));
        let unheld = rekeyed(&resumed, &r1, &InputId::new().to_string(), None);
        assert!(matches!(
            fixture
                .machine
                .observe_original_mcp_operation(&unheld, "server", "tool")
                .await,
            Err(McpObservationRefusal::OriginalUnavailable)
        ));
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

    /// Admit, as the governed ingress does, an original from `requester`
    /// whose input id and raw key are both `id`: a supervisor delivery's
    /// shape (it keeps its payload input id, its event key).
    async fn admit_original_as(
        fixture: &Governed,
        requester: &str,
        id: &uuid::Uuid,
    ) -> RetainedInputAuthority {
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
        prompt.header_mut().id = InputId::from_uuid(*id);
        prompt.header_mut().idempotency_key =
            Some(crate::identifiers::IdempotencyKey::new(id.to_string()));
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

    /// A tracked delivery is named by its payload input id, never by another
    /// input that carries that id as its key. On a governed member a governed
    /// row whose input id and raw key are K (a supervisor delivery's shape,
    /// admitted under K's qualification) sits beside a retained resume keyed
    /// K: a resume has no association of its own, so the machine binds K to
    /// it. The tracked cancel addressed by K ends the governed row and leaves
    /// the resume queued, and the tracked read of K is the governed row. RED
    /// while the cancel resolved K through the binding first: it cancelled
    /// the resume, and the delivery kept running.
    #[tokio::test]
    async fn a_tracked_cancel_ends_its_delivery_and_never_an_input_carrying_its_id() {
        let fixture = governed_staging().await;
        let original = admit_original(&fixture, "caller-a").await;
        let run = stage_run(&fixture, std::slice::from_ref(original.input_id())).await;
        finish_run(&fixture, run.run_id()).await;

        let key = uuid::Uuid::new_v4();
        let key_text = key.to_string();
        let delivery = admit_original_as(&fixture, "caller-b", &key).await;
        let request = request_from_delivery(
            &fixture.runtime_id,
            run,
            "continuation:collision",
            &key_text,
        )
        .await;
        let (outcome, _) = fixture
            .machine
            .accept_retained_resume(
                &fixture.session_id,
                keyed_continuation(&key_text, "the fork finished"),
                request,
            )
            .await
            .expect("the resume is admitted under its raw key");
        let AcceptOutcome::Accepted { state: resume, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        assert_ne!(&resume.input_id, delivery.input_id());
        let bound = crate::service_ext::SessionServiceRuntimeExt::input_state_by_idempotency_key(
            fixture.machine.as_ref(),
            &fixture.session_id,
            &key_text,
        )
        .await
        .expect("key read")
        .expect("the machine binds the key");
        assert_eq!(
            bound.state.input_id, resume.input_id,
            "the machine binds K to the resume"
        );

        let member = meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
            mob_id: "tracked-collision".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host-a".to_string(),
            binding_generation: 1,
            member_session_id: fixture.session_id.to_string(),
            generation: 0,
            fence_token: 1,
        };
        drop(
            fixture
                .machine
                .begin_member_residency_update(fixture.session_id.clone())
                .await
                .commit(member.clone(), None)
                .await
                .expect("publish the member residency"),
        );
        fixture
            .machine
            .cancel_tracked_input_for_member_incarnation(&fixture.session_id, &key_text, &member)
            .await
            .expect("the tracked cancel converges");

        let read = |input_id: InputId| {
            let machine = fixture.machine.as_ref();
            let session_id = &fixture.session_id;
            async move {
                crate::service_ext::SessionServiceRuntimeExt::input_state(
                    machine, session_id, &input_id,
                )
                .await
                .expect("row read")
                .expect("the row")
            }
        };
        let delivery_row = read(delivery.input_id().clone()).await;
        assert_eq!(
            delivery_row.seed.terminal_outcome,
            Some(crate::input_state::InputTerminalOutcome::Abandoned {
                reason: crate::input_state::InputAbandonReason::Cancelled,
            }),
            "the tracked cancel ends the delivery"
        );
        let resume_row = read(resume.input_id.clone()).await;
        assert_eq!(
            (resume_row.seed.phase, resume_row.seed.terminal_outcome),
            (crate::input_state::InputLifecycleState::Queued, None),
            "the resume carrying the delivery's id as its key stays queued"
        );
        let tracked = fixture
            .machine
            .tracked_delivery_input_state(&fixture.session_id, &key_text)
            .await
            .expect("tracked read")
            .expect("the tracked read of K");
        assert_eq!(&tracked.state.input_id, delivery.input_id());
    }
}
