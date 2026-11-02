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

    /// Whether the run `run_id` marked resumable waits for its destination
    /// (see
    /// [`crate::driver::ephemeral::EphemeralRuntimeDriver::resumed_run_waits_for_destination`]).
    pub(crate) fn resumed_run_waits_for_destination(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        self.shell_driver()
            .resumed_run_waits_for_destination(run_id)
    }

    /// Whether a kept retained-resume row without its controller client is
    /// not held by the machine yet.
    pub(crate) fn has_unheld_kept_resume_without_client(&self) -> bool {
        self.shell_driver().has_unheld_kept_resume_without_client()
    }

    /// Hold every kept retained-resume row without its controller client
    /// that the machine does not hold yet, without asking its host (see
    /// [`crate::driver::ephemeral::EphemeralRuntimeDriver::hold_kept_resumes_without_client`]).
    pub(crate) fn hold_kept_resumes_without_client(
        &mut self,
        now: meerkat_core::time_compat::Instant,
    ) -> Result<bool, RuntimeDriverError> {
        self.shell_driver_mut()
            .hold_kept_resumes_without_client(now)
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

    /// The queued kept governed inputs with no way back to their controller
    /// client, in admission order.
    pub(crate) fn queued_kept_governed_inputs_without_controller(&self) -> Vec<InputId> {
        self.shell_driver()
            .queued_kept_governed_inputs_without_controller()
    }

    /// Whether the first input of the run marked resumable `run_id` is kept
    /// governed work with no way back to its controller client.
    pub(crate) fn resumed_run_first_input_lacks_controller(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        self.shell_driver()
            .resumed_run_first_input_lacks_controller(run_id)
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

    /// A kept resume row's controller client, asked for again, is classified
    /// as the kept-row contract maps the host's answer: the recorded
    /// selection's client pins; an actual verdict, a client for another
    /// selection and an owner with no client for the resume
    /// (`Readiness(UnsupportedScope)`) are definite refusals; every other
    /// readiness answer, and a policy observation to refresh, cannot say now
    /// and keeps the row's attempts.
    #[tokio::test]
    async fn a_reacquired_client_is_classified_by_the_kept_row_contract() {
        use crate::retained_work::{
            RetainedClientReacquisition, RetainedResumeError, classify_reacquired_client,
        };
        use crate::traits::ControllerReadinessFailure;
        use meerkat_core::{OperationRefusalKind, OperationRefused};
        let fixture = governed().await;
        let elsewhere = governed().await;
        let selection = fixture.host.selection().clone();
        let identity = meerkat_core::retained_work::RetainedWorkIdentity::new(
            fixture.runtime_id.to_string(),
            meerkat_core::lifecycle::RunId::new(),
            Vec::new(),
            BTreeMap::new(),
            Some(selection.clone()),
        );
        let client = |selection: &ControllerModelSelection| {
            meerkat_core::ControllerModelClient::new(
                selection.clone(),
                Arc::new(Client(selection.clone())),
            )
        };
        let refused = |kind| Err(RetainedResumeError::Refused(OperationRefused::new(kind)));
        assert!(matches!(
            classify_reacquired_client(&identity, Ok(client(&selection))),
            RetainedClientReacquisition::Pinned(pinned) if pinned.selection() == &selection
        ));
        for (answer, what) in [
            (
                Ok(client(elsewhere.host.selection())),
                "a client for another selection",
            ),
            (refused(OperationRefusalKind::Denied), "an actual denial"),
            (
                refused(OperationRefusalKind::MalformedFacts),
                "malformed evidence",
            ),
            (
                Err(RetainedResumeError::AuthorizationUnavailable),
                "an actual authorization unavailability",
            ),
            (
                Err(RetainedResumeError::Readiness(
                    ControllerReadinessFailure::UnsupportedScope,
                )),
                "an owner with no client for the resume",
            ),
        ] {
            assert!(
                matches!(
                    classify_reacquired_client(&identity, answer),
                    RetainedClientReacquisition::Refused
                ),
                "{what} is definite"
            );
        }
        for (answer, expected) in [
            (
                refused(OperationRefusalKind::ReprepareRequired),
                ControllerReadinessFailure::PolicyChanged,
            ),
            (
                Err(RetainedResumeError::Readiness(
                    ControllerReadinessFailure::PolicyUnavailable,
                )),
                ControllerReadinessFailure::PolicyUnavailable,
            ),
            (
                Err(RetainedResumeError::Readiness(
                    ControllerReadinessFailure::OriginalAdmissionPending,
                )),
                ControllerReadinessFailure::OriginalAdmissionPending,
            ),
        ] {
            assert!(
                matches!(
                    classify_reacquired_client(&identity, answer),
                    RetainedClientReacquisition::NotReady(reason) if reason == expected
                ),
                "{expected:?} cannot say now"
            );
        }
    }

    /// Drop `input_id`'s controller client from its live row, as the store
    /// does: a client never survives persistence.
    async fn lose_client(fixture: &Governed, input_id: &InputId) {
        let mut driver = fixture.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *driver else {
            panic!("storeless")
        };
        driver
            .ledger_mut()
            .get_mut(input_id)
            .expect("live row")
            .controller_client = None;
    }

    /// Whether `input_id`'s live row holds a controller client.
    async fn holds_client(fixture: &Governed, input_id: &InputId) -> bool {
        let driver = fixture.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*driver else {
            panic!("storeless")
        };
        driver
            .ledger()
            .get(input_id)
            .expect("live row")
            .controller_client
            .is_some()
    }

    /// Ask the host again, once, for `input_id`'s controller client, as the
    /// runtime loop asks for a kept resume row without one, and pin a client
    /// it supplies. `None` for a row with no retained-resume path.
    async fn ask_again(
        fixture: &Governed,
        input_id: &InputId,
    ) -> Option<crate::retained_work::RetainedClientReacquisition> {
        let mut driver = fixture.driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *driver else {
            panic!("storeless")
        };
        let answer = driver
            .ledger()
            .get(input_id)
            .and_then(|row| driver.reacquire_retained_resume_client(row));
        if let Some(crate::retained_work::RetainedClientReacquisition::Pinned(client)) = &answer {
            driver
                .ledger_mut()
                .get_mut(input_id)
                .expect("live row")
                .controller_client = Some(client.clone());
        }
        answer
    }

    /// Admit, after a staged run of the original of `requester` finished, a
    /// resume of that run's retained work, kept queued (nothing stages it
    /// here). Returns the original and the resume's id.
    async fn queued_resume(
        fixture: &Governed,
        requester: &str,
        job: &str,
    ) -> (RetainedInputAuthority, InputId) {
        let original = admit_original(fixture, requester).await;
        let run = stage_run(fixture, std::slice::from_ref(original.input_id())).await;
        finish_run(fixture, run.run_id()).await;
        let outcome = resume_with(fixture, run, job)
            .await
            .expect("the resume is admitted");
        let AcceptOutcome::Accepted { state, .. } = outcome else {
            panic!("accepted, got {outcome:?}")
        };
        (original, state.input_id)
    }

    /// A kept resume row (its client lost, as in the store) is asked for its
    /// client again and pinned when its host supplies one, and is answered,
    /// still without a client, refused for an owner with no client for it
    /// and not ready for an owner that cannot say now. An ordinary governed
    /// prompt has no such path: the host contract has no way to supply its
    /// client without a fresh authentication of its caller, so it is never
    /// asked for and stays without one (an attachment ends such a kept
    /// prompt before it serves).
    #[tokio::test]
    async fn a_kept_resume_row_is_asked_for_its_client_again() {
        use crate::retained_work::{RetainedClientReacquisition, RetainedResumeError};
        use crate::traits::ControllerReadinessFailure;
        let fixture = governed_staging().await;
        let (_, resume) = queued_resume(&fixture, "caller-a", "fork_off:kept").await;
        let prompt = admit_original(&fixture, "caller-b")
            .await
            .input_id()
            .clone();
        assert!(
            holds_client(&fixture, &resume).await,
            "an admitted resume holds the client its host supplied"
        );

        lose_client(&fixture, &resume).await;
        lose_client(&fixture, &prompt).await;
        let answer = ask_again(&fixture, &resume).await;
        assert!(
            matches!(answer, Some(RetainedClientReacquisition::Pinned(_))),
            "{answer:?}"
        );
        assert!(
            holds_client(&fixture, &resume).await,
            "the host's client is pinned on the kept resume again"
        );
        let answer = ask_again(&fixture, &prompt).await;
        assert!(answer.is_none(), "{answer:?}");
        assert!(
            !holds_client(&fixture, &prompt).await,
            "a governed prompt has no re-pin path"
        );

        lose_client(&fixture, &resume).await;
        fixture
            .host
            .refuse_retained_resume(RetainedResumeError::Readiness(
                ControllerReadinessFailure::PolicyUnavailable,
            ));
        let answer = ask_again(&fixture, &resume).await;
        assert!(
            matches!(
                answer,
                Some(RetainedClientReacquisition::NotReady(
                    ControllerReadinessFailure::PolicyUnavailable
                ))
            ),
            "{answer:?}"
        );
        assert!(!holds_client(&fixture, &resume).await);

        fixture
            .host
            .refuse_retained_resume(RetainedResumeError::Readiness(
                ControllerReadinessFailure::UnsupportedScope,
            ));
        let answer = ask_again(&fixture, &resume).await;
        assert!(
            matches!(answer, Some(RetainedClientReacquisition::Refused)),
            "{answer:?}"
        );
        assert!(!holds_client(&fixture, &resume).await);
    }

    /// A kept resume row whose destination its session has since superseded
    /// can never be admitted again: it is refused before its host is asked,
    /// whatever the host would answer, as a fresh resume of it is.
    #[tokio::test]
    async fn a_kept_resume_whose_destination_moved_on_is_refused_before_its_host_is_asked() {
        use crate::retained_work::RetainedResumeError;
        use crate::traits::ControllerReadinessFailure;
        let fixture = governed_staging().await;
        let (original, resume) = queued_resume(&fixture, "caller-a", "fork_off:moved").await;
        lose_client(&fixture, &resume).await;
        let target = original.association().candidate().target.clone();
        let registration = fixture
            .machine
            .current_session_registration_witness(&fixture.session_id)
            .await
            .expect("current registration");
        fixture
            .machine
            .bind_scoped_context(&registration, &target)
            .await
            .expect("bind the original's destination");
        let mut moved = target;
        moved.context_generation += 1;
        fixture
            .machine
            .bind_scoped_context(&registration, &moved)
            .await
            .expect("bind a later generation of it");
        // The host itself would only answer that it cannot say now.
        fixture
            .host
            .refuse_retained_resume(RetainedResumeError::Readiness(
                ControllerReadinessFailure::PolicyUnavailable,
            ));
        let answer = ask_again(&fixture, &resume).await;
        assert!(
            matches!(
                answer,
                Some(crate::retained_work::RetainedClientReacquisition::Refused)
            ),
            "{answer:?}"
        );
    }

    /// One queued input of a controller-hold state: in the Queue lane at
    /// `sequence`, a content prompt, governed under its own batch key when
    /// `governed`, and recovered by the registration when `kept`.
    fn queued_input(
        state: &mut crate::meerkat_machine::dsl::MeerkatMachineState,
        input_id: &str,
        sequence: u64,
        governed: bool,
        kept: bool,
    ) {
        use crate::meerkat_machine::dsl::{
            InputLane, InputPhase, RecoveredRunApplyBoundary, RecoveredRuntimeExecutionKind,
        };
        let id = input_id.to_string();
        state.input_phases.insert(id.clone(), InputPhase::Queued);
        state.input_lane.insert(id.clone(), InputLane::Queue);
        state
            .input_recovery_lanes
            .insert(id.clone(), InputLane::Queue);
        state.input_admission_seq.insert(id.clone(), sequence);
        state.input_attempt_counts.insert(id.clone(), 0);
        state.input_is_prompt.insert(id.clone(), true);
        state
            .input_runtime_boundary
            .insert(id.clone(), RecoveredRunApplyBoundary::RunStart);
        state
            .input_runtime_execution_kind
            .insert(id.clone(), RecoveredRuntimeExecutionKind::ContentTurn);
        if governed {
            state
                .input_authority_bindings
                .insert(id.clone(), format!("binding-{input_id}"));
            state
                .input_authority_batch_keys
                .insert(id.clone(), format!("batch-{input_id}"));
        }
        if kept {
            state.recovered_admitted_inputs.insert(id);
        }
    }

    /// One kept governed input bound to `run`, applied, as a run a stop or a
    /// restart left open keeps it.
    fn bound_kept_input(
        state: &mut crate::meerkat_machine::dsl::MeerkatMachineState,
        input_id: &str,
        run: &crate::meerkat_machine::dsl::RunId,
    ) {
        use crate::meerkat_machine::dsl::{InputLane, InputPhase};
        let id = input_id.to_string();
        state.input_phases.insert(id.clone(), InputPhase::Applied);
        state.input_run_associations.insert(id.clone(), run.clone());
        state
            .input_recovery_lanes
            .insert(id.clone(), InputLane::Queue);
        state.input_attempt_counts.insert(id.clone(), 1);
        state
            .input_authority_bindings
            .insert(id.clone(), format!("binding-{input_id}"));
        state
            .input_authority_batch_keys
            .insert(id.clone(), format!("batch-{input_id}"));
        state.recovered_admitted_inputs.insert(id);
    }

    /// The machine's plans skip an input it holds for its controller: the
    /// batch plan selects the later work of its lane instead, a requested
    /// stage of the held input is refused, and the resumed-run plan refuses a
    /// run with a held input. Once released, the input is the head of its lane
    /// again and the run resumes.
    #[test]
    fn a_held_input_is_skipped_by_the_plans_and_later_work_runs_first() {
        use crate::meerkat_machine::dsl::command_capabilities::{
            AuthorizedResumedRun, AuthorizedRuntimeLoopBatch, AuthorizedStageForRun,
            RuntimeLoopBatchSource,
        };
        use crate::meerkat_machine::dsl::{MeerkatMachineState, RunId};
        let selected = |state: &MeerkatMachineState| {
            AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(state)
                .map(|plan| plan.into_parts().1)
        };
        let mut state = MeerkatMachineState::default();
        queued_input(&mut state, "kept", 1, true, true);
        queued_input(&mut state, "later", 2, true, false);
        assert_eq!(selected(&state), Some(vec!["kept".to_string()]));

        state.input_controller_holds.insert("kept".to_string());
        assert_eq!(
            selected(&state),
            Some(vec!["later".to_string()]),
            "later work of the held input's lane goes first"
        );
        let run = RunId::from("current-run");
        state.current_run_id = Some(run.clone());
        assert!(
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                &state,
                &["kept".to_string()],
                &run,
                RuntimeLoopBatchSource::Queue,
            )
            .is_none(),
            "a held input is never staged"
        );
        assert!(
            AuthorizedStageForRun::authorize_stage_for_run_from_state(
                &state,
                &["later".to_string()],
                &run,
                RuntimeLoopBatchSource::Queue,
            )
            .is_some()
        );
        state.current_run_id = None;
        state.input_controller_holds.remove("kept");
        assert_eq!(
            selected(&state),
            Some(vec!["kept".to_string()]),
            "released, the input is the head of its lane again"
        );

        let open = RunId::from("open-run");
        let mut resumable = MeerkatMachineState {
            resumable_run_id: Some(open.clone()),
            ..Default::default()
        };
        bound_kept_input(&mut resumable, "bound", &open);
        resumable.input_controller_holds.insert("bound".to_string());
        assert!(
            AuthorizedResumedRun::authorize_resumed_run_from_state(&resumable, &open).is_none(),
            "a run does not resume while an input bound to it is held"
        );
        resumable.input_controller_holds.clear();
        let (_, input_ids) =
            AuthorizedResumedRun::authorize_resumed_run_from_state(&resumable, &open)
                .expect("released, the run resumes")
                .into_parts();
        assert_eq!(input_ids, vec!["bound".to_string()]);
    }

    /// The generated machine holds only kept governed work that waits
    /// unstaged (`HoldInputController`), never stages a held input, releases
    /// a held input with a wake (`InputControllerReleased`), drops the hold of
    /// an input that ends, and does not resume a run with a held input; a
    /// settled or released run drops its inputs' holds. Each step runs the
    /// machine's invariants, among them
    /// `input_controller_holds_are_kept_unstaged_governed_inputs`.
    #[test]
    fn the_machine_holds_a_kept_governed_input_until_its_controller_is_released() {
        use crate::meerkat_machine::dsl::{
            InputAbandonReason, MeerkatMachineAuthority, MeerkatMachineEffect, MeerkatMachineInput,
            MeerkatMachineMutator, MeerkatMachineState, MeerkatPhase, RunId, SessionId,
        };
        let hold = |input_id: &str| MeerkatMachineInput::HoldInputController {
            input_id: input_id.to_string(),
        };
        let release = |input_id: &str| MeerkatMachineInput::ReleaseInputController {
            input_id: input_id.to_string(),
        };
        let registered = || MeerkatMachineState {
            lifecycle_phase: MeerkatPhase::Idle,
            session_id: Some(SessionId::from("held-session")),
            ..Default::default()
        };

        let mut state = registered();
        queued_input(&mut state, "kept", 1, true, true);
        queued_input(&mut state, "staged", 2, true, true);
        queued_input(&mut state, "live", 3, true, false);
        queued_input(&mut state, "plain", 4, false, true);
        let mut authority =
            MeerkatMachineAuthority::recover_from_state(state).expect("a valid machine state");
        let mut apply = |input| MeerkatMachineMutator::apply(&mut authority, input);
        assert!(
            apply(hold("live")).is_err(),
            "work the registration did not recover is never held"
        );
        assert!(
            apply(hold("plain")).is_err(),
            "ungoverned work is never held"
        );
        apply(hold("kept")).expect("kept governed work waiting unstaged is held");
        apply(hold("kept")).expect("holding a held input again changes nothing");
        let run = RunId::from("current-run");
        apply(MeerkatMachineInput::Prepare {
            session_id: SessionId::from("held-session"),
            run_id: run.clone(),
        })
        .expect("a run starts");
        assert!(
            apply(MeerkatMachineInput::StageForRun {
                input_id: "kept".to_string(),
                run_id: run.clone(),
            })
            .is_err(),
            "a held input never stages"
        );
        assert!(
            apply(MeerkatMachineInput::MarkApplied {
                input_id: "kept".to_string(),
            })
            .is_err(),
            "a held input is never applied"
        );
        apply(MeerkatMachineInput::StageForRun {
            input_id: "staged".to_string(),
            run_id: run.clone(),
        })
        .expect("later work stages");
        assert!(
            apply(hold("staged")).is_err(),
            "staged work is not waiting unstaged: it is never held"
        );
        let released = apply(release("kept")).expect("the held input is released");
        assert!(
            released
                .effects()
                .iter()
                .any(|effect| matches!(effect, MeerkatMachineEffect::InputControllerReleased)),
            "the release wakes the runtime"
        );
        assert!(
            apply(release("kept")).is_err(),
            "only a held input is released"
        );
        apply(hold("kept")).expect("held again");
        apply(MeerkatMachineInput::AbandonInput {
            input_id: "kept".to_string(),
            reason: InputAbandonReason::ResumeRefused,
            attempt_count: 0,
        })
        .expect("a refused resume ends it");
        assert!(
            authority.state().input_controller_holds.is_empty(),
            "an input that ends drops its hold"
        );

        let open = RunId::from("open-run");
        let mut resumable = registered();
        resumable.resumable_run_id = Some(open.clone());
        bound_kept_input(&mut resumable, "bound", &open);

        let mut resumed =
            MeerkatMachineAuthority::recover_from_state(resumable.clone()).expect("valid");
        MeerkatMachineMutator::apply(&mut resumed, hold("bound"))
            .expect("an input bound to the run marked resumable is held");
        MeerkatMachineMutator::apply(
            &mut resumed,
            MeerkatMachineInput::Prepare {
                session_id: SessionId::from("held-session"),
                run_id: open.clone(),
            },
        )
        .expect("the marked run starts");
        assert!(
            MeerkatMachineMutator::apply(
                &mut resumed,
                MeerkatMachineInput::ResumeRun {
                    run_id: open.clone(),
                },
            )
            .is_err(),
            "a run does not resume while an input bound to it is held"
        );

        let mut settled =
            MeerkatMachineAuthority::recover_from_state(resumable.clone()).expect("valid");
        MeerkatMachineMutator::apply(&mut settled, hold("bound")).expect("held");
        MeerkatMachineMutator::apply(
            &mut settled,
            MeerkatMachineInput::SettleResumableRun {
                run_id: open.clone(),
                reason: InputAbandonReason::ResumeRefused,
            },
        )
        .expect("the marked run settles");
        assert!(
            settled.state().input_controller_holds.is_empty(),
            "a settled run drops its inputs' holds"
        );

        let mut released_run =
            MeerkatMachineAuthority::recover_from_state(resumable).expect("valid");
        MeerkatMachineMutator::apply(&mut released_run, hold("bound")).expect("held");
        MeerkatMachineMutator::apply(
            &mut released_run,
            MeerkatMachineInput::ReleaseResumableRun {
                run_id: open.clone(),
            },
        )
        .expect("the marked run is released");
        assert!(
            released_run.state().input_controller_holds.is_empty(),
            "a released run drops its inputs' holds"
        );
    }

    /// A run marked resumable, with `bound` (an input applied in it, with the
    /// destination `target`) and `stray` (a queued input that still names the
    /// run, as a discarded live join or a recovered queued row does, with the
    /// destination `stray_target` when set), on a registered session.
    fn resumable_with_stray(
        bound: &str,
        target: Option<&str>,
        stray: &str,
        stray_target: Option<&str>,
    ) -> (
        crate::meerkat_machine::dsl::MeerkatMachineState,
        crate::meerkat_machine::dsl::RunId,
    ) {
        use crate::meerkat_machine::dsl::{MeerkatMachineState, MeerkatPhase, RunId, SessionId};
        let open = RunId::from("open-run");
        let mut state = MeerkatMachineState {
            lifecycle_phase: MeerkatPhase::Idle,
            session_id: Some(SessionId::from("resumed-session")),
            resumable_run_id: Some(open.clone()),
            ..Default::default()
        };
        bound_kept_input(&mut state, bound, &open);
        if let Some(target) = target {
            state
                .input_context_targets
                .insert(bound.to_string(), target.to_string());
        }
        queued_input(&mut state, stray, 2, true, false);
        state
            .input_run_associations
            .insert(stray.to_string(), open.clone());
        if let Some(stray_target) = stray_target {
            state
                .input_context_targets
                .insert(stray.to_string(), stray_target.to_string());
        }
        (state, open)
    }

    /// Prepare and resume `run` on the machine recovered from `state`: the
    /// machine's state once the run resumed, or `None` when `ResumeRun` was
    /// refused.
    fn resume(
        state: crate::meerkat_machine::dsl::MeerkatMachineState,
        run: &crate::meerkat_machine::dsl::RunId,
    ) -> Option<crate::meerkat_machine::dsl::MeerkatMachineState> {
        use crate::meerkat_machine::dsl::{
            MeerkatMachineAuthority, MeerkatMachineInput, MeerkatMachineMutator, SessionId,
        };
        let mut authority =
            MeerkatMachineAuthority::recover_from_state(state).expect("a valid machine state");
        MeerkatMachineMutator::apply(
            &mut authority,
            MeerkatMachineInput::Prepare {
                session_id: SessionId::from("resumed-session"),
                run_id: run.clone(),
            },
        )
        .expect("the marked run starts");
        MeerkatMachineMutator::apply(
            &mut authority,
            MeerkatMachineInput::ResumeRun {
                run_id: run.clone(),
            },
        )
        .ok()
        .map(|_| authority.state().clone())
    }

    /// A resumed run runs under the destination and batch of the input bound
    /// to it, never of a queued input that only still names the run (a
    /// discarded live join keeps its association, and recovery restores a
    /// queued row's last run). Here the stray input sorts after the bound one
    /// and names another destination and batch. RED while `ResumeRun` took
    /// the destination of every input associated with the run: the stray
    /// input came last and the run took its destination and batch.
    #[test]
    fn a_resumed_run_takes_its_bound_inputs_destination_not_a_stray_queued_inputs() {
        let (state, open) = resumable_with_stray(
            "bound-a",
            Some("the-runs-destination"),
            "stray-z",
            Some("another-destination"),
        );
        let resumed = resume(state, &open).expect("the marked run resumes");
        assert_eq!(
            resumed.authority_staged_run.as_ref(),
            Some(&open),
            "the resumed run is the staged run"
        );
        assert_eq!(
            resumed.run_context_binding.as_deref(),
            Some("the-runs-destination"),
            "the run's destination is its bound input's, not the stray input's"
        );
        assert_eq!(
            resumed.authority_staged_batch.as_deref(),
            Some("batch-bound-a"),
            "the run's authority batch is its bound input's, not the stray input's"
        );
    }

    /// A Keep rollback (the unregister of a registration refused before it
    /// served) keeps every input the registration recovered, but not an
    /// input of the run marked resumable whose stop was asked: the Abandon
    /// arm answers it, so the stop ends it and the next registration never
    /// resumes the stopped run. RED on the round 4 kernel: the Keep arm kept
    /// it. Unrun: written while Cargo is not run.
    #[test]
    fn a_keep_rollback_keeps_no_input_of_a_resumable_run_whose_stop_was_asked() {
        use crate::meerkat_machine::dsl::{
            InputDurabilityKind, MeerkatMachineAuthority, MeerkatMachineEffect,
            MeerkatMachineInput, MeerkatMachineMutator, MeerkatMachineState, MeerkatPhase,
            RegistrationPhase, RunId, SessionId, StopInputDisposition,
            UnregisterQueuedInputsDisposition,
        };
        let open = RunId::from("open-run");
        let verdict = |stopped: bool| {
            let mut state = MeerkatMachineState {
                lifecycle_phase: MeerkatPhase::Idle,
                session_id: Some(SessionId::from("resumed-session")),
                registration_phase: RegistrationPhase::Active,
                resumable_run_id: Some(open.clone()),
                ..Default::default()
            };
            bound_kept_input(&mut state, "bound", &open);
            let mut authority =
                MeerkatMachineAuthority::recover_from_state(state).expect("a valid machine state");
            if stopped {
                MeerkatMachineMutator::apply(
                    &mut authority,
                    MeerkatMachineInput::StopCurrentRunForRun {
                        run_id: open.clone(),
                    },
                )
                .expect("a stop of the resumable run is recorded");
            }
            let begin = authority.state().clone();
            MeerkatMachineMutator::apply(
                &mut authority,
                MeerkatMachineInput::BeginUnregisterSession {
                    session_id: SessionId::from("resumed-session"),
                    agent_runtime_id: begin.active_runtime_id.clone(),
                    fence_token: begin.active_fence_token,
                    generation: begin.active_runtime_generation,
                    runtime_epoch_id: begin.active_runtime_epoch_id.clone(),
                    queued_inputs: UnregisterQueuedInputsDisposition::Keep,
                    requeue_max_age_ms: 0,
                },
            )
            .expect("the rollback's unregister begins");
            MeerkatMachineMutator::apply(
                &mut authority,
                MeerkatMachineInput::ClassifyStopInputDisposition {
                    input_id: "bound".to_string(),
                    durability: InputDurabilityKind::Durable,
                    directed: false,
                    objective_bound: false,
                },
            )
            .expect("the stop classifies the input")
            .into_effects()
            .into_iter()
            .find_map(|effect| match effect {
                MeerkatMachineEffect::StopInputDispositionClassified { disposition, .. } => {
                    Some(disposition)
                }
                _ => None,
            })
        };
        assert_eq!(
            verdict(false),
            Some(StopInputDisposition::Keep),
            "a Keep rollback keeps the input it recovered"
        );
        assert_eq!(
            verdict(true),
            Some(StopInputDisposition::Abandon),
            "but not the input of a resumable run whose stop was asked"
        );
    }

    /// The classifier's verdict for each of `inputs`, in order, under a Keep
    /// rollback of the machine recovered from `state` (a registered session
    /// whose run `run` is marked resumable), after a recorded stop of `run`
    /// when `stopped`.
    fn keep_rollback_verdicts(
        state: crate::meerkat_machine::dsl::MeerkatMachineState,
        run: &crate::meerkat_machine::dsl::RunId,
        stopped: bool,
        inputs: &[&str],
    ) -> Vec<Option<crate::meerkat_machine::dsl::StopInputDisposition>> {
        use crate::meerkat_machine::dsl::{
            InputDurabilityKind, MeerkatMachineAuthority, MeerkatMachineEffect,
            MeerkatMachineInput, MeerkatMachineMutator, SessionId,
            UnregisterQueuedInputsDisposition,
        };
        let mut authority =
            MeerkatMachineAuthority::recover_from_state(state).expect("a valid machine state");
        if stopped {
            MeerkatMachineMutator::apply(
                &mut authority,
                MeerkatMachineInput::StopCurrentRunForRun {
                    run_id: run.clone(),
                },
            )
            .expect("a stop of the resumable run is recorded");
        }
        let begin = authority.state().clone();
        MeerkatMachineMutator::apply(
            &mut authority,
            MeerkatMachineInput::BeginUnregisterSession {
                session_id: SessionId::from("resumed-session"),
                agent_runtime_id: begin.active_runtime_id.clone(),
                fence_token: begin.active_fence_token,
                generation: begin.active_runtime_generation,
                runtime_epoch_id: begin.active_runtime_epoch_id.clone(),
                queued_inputs: UnregisterQueuedInputsDisposition::Keep,
                requeue_max_age_ms: 0,
            },
        )
        .expect("the rollback's unregister begins");
        inputs
            .iter()
            .map(|input_id| {
                MeerkatMachineMutator::apply(
                    &mut authority,
                    MeerkatMachineInput::ClassifyStopInputDisposition {
                        input_id: (*input_id).to_string(),
                        durability: InputDurabilityKind::Durable,
                        directed: false,
                        objective_bound: false,
                    },
                )
                .expect("the stop classifies the input")
                .into_effects()
                .into_iter()
                .find_map(|effect| match effect {
                    MeerkatMachineEffect::StopInputDispositionClassified {
                        disposition, ..
                    } => Some(disposition),
                    _ => None,
                })
            })
            .collect()
    }

    /// The Keep exception of a stopped run marked resumable ends that run's
    /// own inputs only, the inputs bound to it (Staged, Applied or
    /// AppliedPendingConsumption), as every other stop path does. A queued
    /// input the registration recovered that only still names the run (a
    /// discarded live join keeps its association, and recovery restores a
    /// queued row's last run) is kept like any other recovered input. RED
    /// while the exception had no phase test: the rollback ended the queued
    /// input as stopped too. Unrun: written while Cargo is not run.
    #[test]
    fn a_keep_rollback_after_a_stop_keeps_a_queued_input_that_only_names_the_run() {
        use crate::meerkat_machine::dsl::{
            MeerkatMachineState, MeerkatPhase, RegistrationPhase, RunId, SessionId,
            StopInputDisposition,
        };
        let open = RunId::from("open-run");
        let mut state = MeerkatMachineState {
            lifecycle_phase: MeerkatPhase::Idle,
            session_id: Some(SessionId::from("resumed-session")),
            registration_phase: RegistrationPhase::Active,
            resumable_run_id: Some(open.clone()),
            ..Default::default()
        };
        bound_kept_input(&mut state, "bound", &open);
        // Recovered by the registration, queued, and still naming the run.
        queued_input(&mut state, "stray", 2, true, true);
        state
            .input_run_associations
            .insert("stray".to_string(), open.clone());
        assert_eq!(
            keep_rollback_verdicts(state.clone(), &open, false, &["bound", "stray"]),
            vec![
                Some(StopInputDisposition::Keep),
                Some(StopInputDisposition::Keep)
            ],
            "a Keep rollback keeps every input the registration recovered"
        );
        assert_eq!(
            keep_rollback_verdicts(state, &open, true, &["bound", "stray"]),
            vec![
                Some(StopInputDisposition::Abandon),
                Some(StopInputDisposition::Keep)
            ],
            "after a stop of the run, the rollback ends the input bound to it and keeps \
             the queued input that only names it"
        );
    }

    /// The destination wait counts the governed inputs bound to the run only.
    /// On a document that requires a destination and has none bound, a run
    /// whose bound input is ungoverned resumes, whatever governed queued
    /// input still names it; with a governed bound input it waits. The
    /// machine's guard, its resumed-run plan and the runtime's mirror agree.
    /// RED while they counted every input associated with the run: the
    /// stray governed input held the run back.
    #[test]
    fn the_destination_wait_counts_only_the_governed_inputs_bound_to_the_run() {
        use crate::meerkat_machine::dsl::command_capabilities::{
            AuthorizedResumedRun, resumed_run_destination_attached,
        };
        let (mut state, open) = resumable_with_stray("bound-a", None, "stray-z", None);
        state.input_authority_bindings.remove("bound-a");
        state.input_authority_batch_keys.remove("bound-a");
        state.scoped_context_required = true;
        assert!(
            resumed_run_destination_attached(&state, &open),
            "no governed input is bound to the run"
        );
        assert!(
            AuthorizedResumedRun::authorize_resumed_run_from_state(&state, &open).is_some(),
            "the resumed-run plan does not wait for the destination"
        );
        assert!(
            resume(state.clone(), &open).is_some(),
            "the machine resumes the run"
        );

        state
            .input_authority_bindings
            .insert("bound-a".to_string(), "binding-bound-a".to_string());
        state
            .input_authority_batch_keys
            .insert("bound-a".to_string(), "batch-bound-a".to_string());
        assert!(
            !resumed_run_destination_attached(&state, &open),
            "a governed input bound to the run waits for the destination"
        );
        assert!(
            AuthorizedResumedRun::authorize_resumed_run_from_state(&state, &open).is_none(),
            "the resumed-run plan waits"
        );
        assert!(resume(state, &open).is_none(), "the machine waits");
    }
}
