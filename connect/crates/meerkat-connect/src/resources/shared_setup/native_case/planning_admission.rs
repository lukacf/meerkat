//! Two SDK Originals join one host-selected workflow and dedicated G. The
//! admission-only case selects no transfer; transfer controls additionally
//! require automatic native context receipts, not setup or task completion.
use super::*;
use meerkat_authorization_contracts::{
    constraints::{DelegationDepth, ExecutionRestrictions, LifetimeRestriction},
    grant::GrantLineageRef,
};
use meerkat_core::service::SessionHistoryQuery;
use meerkat_mobkit::rpc_gateway::{GatewayHostComposition, PreparedApplicationRequest};
use meerkat_toolkit::setup::SetupWorkflowTerms;
use toolkit_mobkit::private_originals::{
    InstalledWorkflowMember, PrivateOriginalSubmission, PrivateOriginalSubmit, PrivateOriginals,
};
use toolkit_package::{ContentDigest, Revision};
mod bootstrap_completion_tests;
mod console_consent_case;
mod construction;
mod context_delivery_tests;
mod context_transfer_tests;
mod public_case;

const NAMES: [&str; 4] = [
    "original-a",
    "original-b",
    "planning-parent",
    "planning-lead",
];
const P_TEXT: &str = "SDK host planning parent, without either waiter request";
const G_TEXT: &str = "SDK host planning lead, awaiting independently authorized context delivery";

fn source_event(
    name: &str,
    label: &str,
    content: &str,
    contributors: Vec<EvidenceId>,
) -> PrivateAuthenticatedEvent {
    PrivateAuthenticatedEvent {
        route: id(name),
        original: evidence(label),
        occurrence: evidence(&format!("{label}-occurrence")),
        original_authentication: PrivateSourceEvidence {
            resource_id: evidence(&format!("{label}-authentication")),
            revision: evidence("1"),
            sha256: digest(format!("authenticated {label}").as_bytes()),
        },
        root_event: PrivateSourceEvidence {
            resource_id: evidence(&format!("{label}-root")),
            revision: evidence("1"),
            sha256: digest(content.as_bytes()),
        },
        contributors,
        content_sha256: digest(content.as_bytes()),
    }
}

#[derive(Default)]
struct PlanningScript {
    allow_context_delivery: std::sync::atomic::AtomicBool,
    consent: Option<Arc<console_consent_case::ConsentControl>>,
    fresh_find_once: AtomicBool,
    find_attempt_results: Mutex<BTreeMap<String, meerkat_core::ToolResult>>,
    calls: Mutex<BTreeMap<String, Value>>,
    results: Mutex<BTreeMap<String, Value>>,
}
impl PlanningScript {
    fn response(&self, request: &LlmRequest) -> Vec<LlmEvent> {
        let body = serde_json::to_string(&request.messages).unwrap();
        if body.contains(G_TEXT)
            && body.contains("toolkit-workflow-inline-context-v1")
            && self
                .allow_context_delivery
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return done("The exact completed workflow context was received.");
        }
        if body.contains(P_TEXT) || body.contains(G_TEXT) {
            assert!(
                !body.contains("sdk-planning-a") && !body.contains("sdk-planning-b"),
                "planning receives no waiter request through this admission path"
            );
            return done("Planning input received. Waiter context delivery remains pending.");
        }
        let tag = if body.contains("sdk-planning-a") {
            "a"
        } else if body.contains("sdk-planning-b") {
            "b"
        } else {
            panic!("unexpected controlled native input")
        };
        if self.fresh_find_once.load(Ordering::Acquire) {
            return self.response_with_fresh_find(request, tag);
        }
        let key = format!("{tag}-planning-find");
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message
                && let Some(result) = results.iter().find(|row| row.tool_use_id == key)
            {
                assert!(
                    !result.is_error,
                    "the actual planning Find was refused ({key}): {}",
                    {
                        let body = serde_json::from_str::<Value>(&result.text_content()).ok();
                        let field = |name: &str| {
                            body.as_ref()
                                .and_then(|value| value.get(name))
                                .and_then(Value::as_str)
                                .map(|value| {
                                    value
                                        .chars()
                                        .filter(|ch| !ch.is_control())
                                        .take(512)
                                        .collect::<String>()
                                })
                        };
                        json!({"error":field("error"), "message":field("message")})
                    }
                );
                let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                let mut retained = self.results.lock().unwrap();
                if let Some(old) = retained.get(&key) {
                    assert_eq!(old, &value);
                } else {
                    retained.insert(key.clone(), value);
                }
                return done(
                    "Shared planning admission observed; this waiter still needs its own context delivery and decision.",
                );
            }
        }
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL)
        );
        let arguments = json!({"request":format!("sdk-planning-{tag}: prepare this independently authenticated request")});
        assert!(
            self.calls
                .lock()
                .unwrap()
                .insert(key.clone(), arguments.clone())
                .is_none(),
            "no implicit replay or second model call"
        );
        vec![
            LlmEvent::ToolCallComplete {
                id: key,
                name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                args: arguments,
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }
}

// Exact Core projection of this one documented refusal, certified before the
// Find's effect (tool_invoked false). An uncertified ("unknown") result makes
// no claim that the effect did not run, so it never permits a fresh Find.
fn planning_find_reprepare(result: &meerkat_core::ToolResult) -> bool {
    find_feedback::certified_reprepare(result)
}

impl PlanningScript {
    // Opt-in controlled-model policy for case57 only. Every fresh call goes
    // through ordinary Core preparation; no original is relaunched or ordered
    // behind the other original. The existing native assertions remain final.
    fn response_with_fresh_find(&self, request: &LlmRequest, tag: &str) -> Vec<LlmEvent> {
        assert!(matches!(tag, "a" | "b"));
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL)
        );
        let first = format!("{tag}-planning-find");
        let fresh = format!("{first}-fresh");
        let arguments = json!({"request":format!("sdk-planning-{tag}: prepare this independently authenticated request")});
        let mut calls = self.calls.lock().unwrap();
        let mut seen = self.find_attempt_results.lock().unwrap();
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results
                    .iter()
                    .filter(|row| row.tool_use_id == first || row.tool_use_id == fresh)
                {
                    assert!(
                        calls.get(&result.tool_use_id) == Some(&arguments),
                        "a Find result must belong to this original's actual issued call"
                    );
                    if let Some(prior) = seen.get(&result.tool_use_id) {
                        assert!(
                            prior == result,
                            "an observed Find result must remain exact on history replay"
                        );
                    } else {
                        seen.insert(result.tool_use_id.clone(), result.clone());
                    }
                }
            }
        }
        let latest = if calls.contains_key(&fresh) {
            Some(&fresh)
        } else if calls.contains_key(&first) {
            Some(&first)
        } else {
            None
        };
        let next = if let Some(latest) = latest {
            let result = seen
                .get(latest)
                .expect("a new model choice requires the latest issued Find result");
            if !result.is_error {
                let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                let key = latest.clone();
                drop(seen);
                drop(calls);
                let mut retained = self.results.lock().unwrap();
                if let Some(prior) = retained.get(&key) {
                    assert!(
                        prior == &value,
                        "the successful Find result must remain exact"
                    );
                } else {
                    retained.insert(key, value);
                }
                return done(
                    "Shared planning admission observed; this waiter still needs its own context delivery and decision.",
                );
            }
            assert!(
                planning_find_reprepare(result),
                "only the exact Core reprepare-required result permits this fresh Find proposal"
            );
            assert!(
                latest == &first,
                "one fresh Find proposal per original is the complete bound"
            );
            fresh
        } else {
            first
        };
        assert!(
            calls.insert(next.clone(), arguments.clone()).is_none(),
            "never reissue an existing Find call ID"
        );
        drop(seen);
        drop(calls);
        vec![
            LlmEvent::ToolCallComplete {
                id: next,
                name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                args: arguments,
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }

    fn assert_fresh_find_attempts(&self) {
        let calls = self.calls.lock().unwrap();
        let seen = self.find_attempt_results.lock().unwrap();
        let results = self.results.lock().unwrap();
        let mut fresh_count = 0;
        for tag in ["a", "b"] {
            let first = format!("{tag}-planning-find");
            let fresh = format!("{first}-fresh");
            let arguments = json!({"request":format!("sdk-planning-{tag}: prepare this independently authenticated request")});
            assert!(calls.get(&first) == Some(&arguments));
            let latest = if let Some(actual) = calls.get(&fresh) {
                assert!(
                    actual == &arguments,
                    "fresh Find keeps the complete original request"
                );
                assert!(planning_find_reprepare(seen.get(&first).unwrap()));
                fresh_count += 1;
                &fresh
            } else {
                &first
            };
            let result = seen.get(latest).unwrap();
            assert!(
                !result.is_error,
                "each original must obtain an actual successful final Find"
            );
            let value: Value = serde_json::from_str(&result.text_content()).unwrap();
            assert!(results.get(latest) == Some(&value));
        }
        assert!(fresh_count <= 2);
        assert_eq!(
            calls.len(),
            2 + fresh_count,
            "retain every actual initial and fresh call"
        );
        assert_eq!(
            seen.len(),
            calls.len(),
            "every actual attempt has its retained result"
        );
        assert_eq!(results.len(), 2, "exactly one successful Find per original");
    }
}

#[async_trait]
impl LlmClient for PlanningScript {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        Ok(())
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(model_selection())
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        PlainModelRoute::new(ENDPOINT, MODEL)
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed native request required")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let authorization = request
            .authorization()
            .expect("actual native model authorization");
        let prepared = match authorization.prepare(model_facts()) {
            Ok(value) => value,
            Err(error) => {
                return Box::pin(futures_util::stream::iter([Err(
                    LlmError::from_operation_authorization(error),
                )]));
            }
        };
        let current = match prepared.current() {
            Ok(value) => value,
            Err(error) => {
                return Box::pin(futures_util::stream::iter([Err(
                    LlmError::from_operation_authorization(error),
                )]));
            }
        };
        if let Err(error) =
            current.observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
        {
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::from_operation_observation(error),
            )]));
        }
        if let Err(error) = current.observe_outcome(
            meerkat_core::authorization::OperationObservedOutcome::HttpResponse { status: 200 },
        ) {
            return Box::pin(futures_util::stream::iter([Err(
                LlmError::from_operation_observation(error),
            )]));
        }
        if let Some(consent) = &self.consent {
            use futures_util::StreamExt;
            return Box::pin(
                futures_util::stream::once(async move {
                    consent.response(self, request.request()).await
                })
                .flat_map(|events| futures_util::stream::iter(events.into_iter().map(Ok))),
            );
        }
        Box::pin(futures_util::stream::iter(
            self.response(request.request()).into_iter().map(Ok),
        ))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct PlanningCase {
    source: Arc<PrivateOriginals>,
    terms: SetupWorkflowTerms,
    parent: PrivateOriginalSubmit,
    lead: PrivateOriginalSubmit,
    lineage: Vec<GrantLineageRef>,
    member: Arc<InstalledWorkflowMember>,
    script: Arc<PlanningScript>,
}

async fn await_turn(case: &Case, name: &str, ticket: &str) {
    use meerkat_mobkit::identity_first::{TurnOutcome, TurnOutput, TurnTicket};
    let outcome = case
        .native
        .as_ref()
        .unwrap()
        .identity_runtime()
        .unwrap()
        .wait_for_turn(
            &AgentIdentity::parse(name).unwrap(),
            TurnTicket::parse(ticket).unwrap(),
            BOUND,
        )
        .await;
    assert!(
        matches!(
            outcome,
            TurnOutcome::Completed {
                output: TurnOutput::Text { .. }
            }
        ),
        "actual native turn must settle"
    );
}
async fn history(case: &Case, name: &str) -> Value {
    let service = case
        .native
        .as_ref()
        .unwrap()
        .mob_runtime()
        .session_service()
        .unwrap();
    let page = service
        .read_history(
            &case.audit_sessions[name],
            SessionHistoryQuery {
                offset: 0,
                limit: Some(100),
            },
        )
        .await
        .unwrap();
    assert!(!page.has_more);
    serde_json::to_value(page.messages).unwrap()
}
async fn input_ids(case: &Case, name: &str) -> BTreeSet<String> {
    // The live ledger archives terminal rows after durable completion. Count
    // exact durable inputs when checking admission and replay after a turn.
    let runtime =
        meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&case.audit_sessions[name]);
    let rows = case
        .audit_store
        .as_ref()
        .unwrap()
        .load_input_states_strict(&runtime)
        .await
        .unwrap();
    assert!(
        rows.len() <= 16,
        "controlled session input evidence must remain bounded"
    );
    let ids = rows
        .iter()
        .map(|row| row.state.input_id.to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        rows.len(),
        "durable input identities must be unique"
    );
    ids
}
async fn input_count(case: &Case, name: &str) -> usize {
    input_ids(case, name).await.len()
}

// Read-only evidence from the selected SQLite database. This is never used to
// produce workflow terms, grant access, join membership or reconcile effects.
fn workflow_evidence(case: &Case) -> (Value, Vec<toolkit_store::StoredEffect>, Vec<Value>) {
    use std::os::unix::fs::MetadataExt;
    let realm = meerkat_core::mob_realm_id(MOB).unwrap().to_string();
    let path = case
        .directory
        .path()
        .join("shared/data/toolkit-native")
        .join(digest(realm.as_bytes()))
        .join("originals.sqlite3")
        .canonicalize()
        .unwrap();
    let file = fs::File::open(&path).unwrap();
    let before = file.metadata().unwrap();
    let db = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    db.busy_timeout(Duration::from_secs(1)).unwrap();
    let mut query = db
        .prepare(
            "SELECT owner,id,data FROM toolkit_records WHERE realm=?1 ORDER BY owner,id LIMIT 65",
        )
        .unwrap();
    let rows = query
        .query_map([&realm], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(rows.len() < 65);
    let mut workflows = Vec::new();
    let mut acquisitions = Vec::new();
    for (owner, record, raw) in rows {
        let data: toolkit_store::RecordData = serde_json::from_str(&raw).unwrap();
        match data.schema.as_str() {
            "meerkat_toolkit.setup_workflow" => workflows.push((owner, record, data.body)),
            "meerkat_toolkit.acquisition" => acquisitions.push(json!({
                "key": toolkit_store::StoreKey { realm: realm.clone(), owner, id: record },
                "body": data.body,
            })),
            _ => (),
        }
    }
    assert_eq!(workflows.len(), 1);
    let (owner, record, workflow) = workflows.remove(0);
    let mut query = db.prepare("SELECT data FROM toolkit_effects WHERE realm=?1 AND owner=?2 AND record_id=?3 ORDER BY id LIMIT 17").unwrap();
    let effects = query
        .query_map(rusqlite::params![realm, owner, record], |row| {
            row.get::<_, String>(0)
        })
        .unwrap()
        .map(|row| serde_json::from_str::<toolkit_store::StoredEffect>(&row.unwrap()).unwrap())
        .collect::<Vec<_>>();
    assert!(effects.len() < 17);
    let after = fs::metadata(path).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    (workflow, effects, acquisitions)
}

impl PlanningCase {
    async fn await_automatic_context_delivery(&self, case: &Case) {
        self.await_automatic_context_deliveries(case, 2).await;
    }

    async fn await_automatic_context_deliveries(&self, case: &Case, expected: usize) {
        // The ordinary coordinator and host recovery must drive these effects.
        // This loop only reads canonical evidence; it never calls deliver().
        let effects = tokio::time::timeout(BOUND, async {
            loop {
                let (_, effects, _) = workflow_evidence(case);
                let deliveries = effects
                    .into_iter()
                    .filter(|effect| effect.intent.kind == "setup_workflow_delivery")
                    .collect::<Vec<_>>();
                if deliveries.len() == expected
                    && deliveries
                        .iter()
                        .all(|effect| effect.status == toolkit_store::EffectStatus::Succeeded)
                {
                    break deliveries;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("ordinary host owners must acknowledge the exact context inputs");
        let mut inputs = BTreeSet::new();
        for effect in effects {
            assert_eq!(effect.attempt, 1);
            let Some(toolkit_store::EffectOutcome::Succeeded { receipt }) = effect.outcome else {
                panic!("automatic native admission receipt required")
            };
            assert_eq!(receipt.schema, "toolkit.workflow-context-input");
            assert_eq!(receipt.version, 1);
            let input: meerkat_core::lifecycle::InputId =
                serde_json::from_value(receipt.body["input"].clone()).unwrap();
            let session: meerkat_core::SessionId =
                serde_json::from_value(receipt.body["session"].clone()).unwrap();
            assert_eq!(session, self.member.session().unwrap());
            let observation =
                toolkit_mobkit::private_originals::WorkflowDeliveryObservation::Admitted {
                    session_id: session.to_string(),
                    input_id: input.to_string(),
                };
            let (actual, _) =
                context_delivery_tests::accepted_input(case, self, &observation).await;
            assert_eq!(actual, input);
            assert!(inputs.insert(input.to_string()));
        }
        assert_eq!(input_count(case, "planning-lead").await, expected + 1);
    }

    async fn run(&self, case: &mut Case) {
        self.run_open(case).await;
        self.cancel_parent(case).await;
    }

    async fn run_open(
        &self,
        case: &mut Case,
    ) -> (
        toolkit_mobkit::private_originals::PrivateOriginalRef,
        toolkit_mobkit::private_originals::PrivateOriginalRef,
    ) {
        self.run_open_with_stagger(case, None).await
    }

    // Only the expiry control staggers two real calls to obtain distinct
    // canonical deadlines; the original admission control remains concurrent.
    async fn run_open_with_stagger(
        &self,
        case: &mut Case,
        stagger: Option<Duration>,
    ) -> (
        toolkit_mobkit::private_originals::PrivateOriginalRef,
        toolkit_mobkit::private_originals::PrivateOriginalRef,
    ) {
        let launch = case.launch.as_ref().unwrap().clone();
        assert!(
            launch.selected_planning().is_err(),
            "configured shared mode must not fall back while G authority is absent"
        );
        let reply = case
            .client
            .lock()
            .await
            .call(json!({"kind":"submit_authenticated_original", "submission": self.parent}))
            .await
            .unwrap();
        let parent_outcome: PrivateOriginalSubmission =
            serde_json::from_value(reply["outcome"].clone()).unwrap();
        let PrivateOriginalSubmission::Admitted {
            turn_ticket: Some(ticket),
            ..
        } = parent_outcome
        else {
            panic!("P was not admitted")
        };
        await_turn(case, "planning-parent", &ticket).await;
        assert_eq!(input_count(case, "planning-lead").await, 0);
        let planning = self
            .source
            .prepare_workflow_planning(
                &self.parent.original,
                self.lead.clone(),
                self.terms.clone(),
                self.lineage.clone(),
            )
            .await
            .unwrap();
        let replay = self
            .source
            .prepare_workflow_planning(
                &self.parent.original,
                self.lead.clone(),
                self.terms.clone(),
                self.lineage.clone(),
            )
            .await
            .unwrap();
        assert!(
            Arc::ptr_eq(&planning, &replay),
            "canonical host selection replay keeps actual custody"
        );
        launch.select_planning(planning.clone()).unwrap();
        launch.select_planning(replay).unwrap();
        let mut mismatched = self.terms.clone();
        mismatched.cycle_label.push_str("-foreign");
        assert!(
            self.source
                .prepare_workflow_planning(
                    &self.parent.original,
                    self.lead.clone(),
                    mismatched,
                    self.lineage.clone()
                )
                .await
                .is_err()
        );
        let make = |tag: &str| {
            let content =
                format!("sdk-planning-{tag}: prepare this independently authenticated request");
            (
                source_event(
                    &format!("original-{tag}"),
                    &format!("sdk-planning-{tag}"),
                    &content,
                    vec![],
                ),
                content,
            )
        };
        let (a_event, a_content) = make("a");
        let (b_event, b_content) = make("b");
        let a_auth = a_event.original_authentication.clone();
        let a = launch.launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            a_event,
            a_content.clone(),
        );
        // launch() spawns the launch at once, so B is launched only where this
        // driver means B to start: together with A, or after A's settled turn
        // and the stagger.
        let launch_b = || {
            launch.launch(
                meerkat_workgraph::WorkOwnerKey::agent("sdk-original-b").unwrap(),
                b_event,
                b_content,
            )
        };
        let (a, b) = tokio::time::timeout(BOUND, async {
            if let Some(delay) = stagger {
                let a = a.await;
                let observed = launch.observations();
                let row = observed
                    .iter()
                    .find(|row| row["original"]["route"] == "original-a")
                    .unwrap();
                await_turn(case, "original-a", row["ticket"].as_str().unwrap()).await;
                tokio::time::sleep(delay).await;
                (a, launch_b().await)
            } else {
                tokio::join!(a, launch_b())
            }
        })
        .await
        .unwrap();
        let a = a.unwrap().unwrap();
        let b = b.unwrap().unwrap();
        assert!(a != b);
        let observations = launch.observations();
        assert_eq!(observations.len(), 2);
        if let Some(consent) = &self.script.consent {
            consent.before_waiter_turns(self, case, &observations).await;
        }
        for row in &observations {
            let name = row["original"]["route"].as_str().unwrap();
            assert_eq!(row["session"], case.audit_sessions[name].to_string());
            await_turn(case, name, row["ticket"].as_str().unwrap()).await;
        }
        if self.script.fresh_find_once.load(Ordering::Acquire) {
            self.script.assert_fresh_find_attempts();
        } else {
            assert_eq!(self.script.calls.lock().unwrap().len(), 2);
        }
        let results = self.script.results.lock().unwrap().clone();
        assert_eq!(results.len(), 2);
        let mut admitted = Vec::new();
        for result in results.values() {
            assert_eq!(result["worker"]["state"], "workflow_lead");
            let observation = &result["worker"]["observation"];
            assert_eq!(observation["state"], "native");
            let actual: PrivateOriginalSubmission =
                serde_json::from_value(observation["observation"].clone()).unwrap();
            if let PrivateOriginalSubmission::Admitted {
                session_id,
                input_id,
                turn_ticket,
                ..
            } = actual
            {
                assert_eq!(session_id, self.member.session().unwrap().to_string());
                if let Some(ticket) = turn_ticket {
                    await_turn(case, "planning-lead", &ticket).await;
                }
                admitted.push(input_id);
            }
        }
        assert!(
            !admitted.is_empty(),
            "at least the launch winner observes exact native acceptance"
        );
        assert!(admitted.iter().all(|input| input == &admitted[0]));
        let automatic = self
            .script
            .allow_context_delivery
            .load(std::sync::atomic::Ordering::Acquire);
        if automatic {
            self.await_automatic_context_delivery(case).await;
        }
        let expected_inputs = if automatic { 3 } else { 1 };
        let native_inputs = input_ids(case, "planning-lead").await;
        assert_eq!(native_inputs.len(), expected_inputs);
        assert!(
            admitted.iter().all(|input| native_inputs.contains(input)),
            "actual G admission must name its exact durable input"
        );
        let (workflow, effects, acquisitions) = workflow_evidence(case);
        assert_eq!(workflow["terms"], json!(self.terms));
        let waiters = workflow["waiters"].as_object().unwrap();
        assert_eq!(waiters.len(), 2);
        assert_eq!(acquisitions.len(), 2);
        for original in [&a, &b] {
            let key = self.source.source_record_key(original).await.unwrap();
            let matches = waiters
                .values()
                .filter(|waiter| waiter["identity"]["source"] == json!(key))
                .collect::<Vec<_>>();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0]["departed"], false);
            let observed = observations
                .iter()
                .find(|row| row["original"] == json!(original))
                .unwrap();
            assert_eq!(matches[0]["identity"]["work"], observed["work"]);
            let acquisition = acquisitions
                .iter()
                .filter(|row| row["key"] == matches[0]["identity"]["acquisition"])
                .collect::<Vec<_>>();
            assert_eq!(
                acquisition.len(),
                1,
                "membership names one exact canonical acquisition"
            );
            let request = &acquisition[0]["body"]["request"];
            assert_eq!(request["host"]["work_context"], observed["work"]);
            assert_eq!(
                request["host"]["continuation"]["revision"],
                observed["input"]
            );
            let association = self
                .source
                .binding_evidence(original)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(request["evidence"]["association"], json!(association));
        }
        assert_ne!(
            observations[0]["work"]["child"],
            observations[1]["work"]["child"]
        );
        assert_eq!(
            observations[0]["work"]["parent"],
            observations[1]["work"]["parent"]
        );
        let launches = effects
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_launch")
            .collect::<Vec<_>>();
        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].status, toolkit_store::EffectStatus::Succeeded);
        assert_eq!(launches[0].attempt, 1);
        let deliveries = effects
            .iter()
            .filter(|effect| effect.intent.kind == "setup_workflow_delivery")
            .collect::<Vec<_>>();
        assert_eq!(deliveries.len(), 2);
        for waiter in waiters.values() {
            assert_eq!(
                deliveries
                    .iter()
                    .filter(|effect| effect.intent.request.body["payload"]["identity"]
                        == waiter["identity"])
                    .count(),
                1
            );
        }
        for effect in deliveries {
            if automatic {
                assert_eq!(effect.status, toolkit_store::EffectStatus::Succeeded);
                assert_eq!(effect.attempt, 1);
                assert!(matches!(
                    &effect.outcome,
                    Some(toolkit_store::EffectOutcome::Succeeded { .. })
                ));
            } else {
                assert_eq!(effect.status, toolkit_store::EffectStatus::Pending);
                assert_eq!(effect.attempt, 0);
                assert!(
                    effect.outcome.is_none(),
                    "membership is not context delivery"
                );
            }
        }
        let g_history = history(case, "planning-lead").await;
        let g_text = serde_json::to_string(&g_history).unwrap();
        assert!(g_text.contains(G_TEXT));
        if automatic {
            assert!(g_text.contains("sdk-planning-a") && g_text.contains("sdk-planning-b"));
            for name in ["planning-parent", "original-a", "original-b"] {
                assert!(
                    !history(case, name)
                        .await
                        .to_string()
                        .contains("toolkit-workflow-inline-context-v1")
                );
            }
        } else {
            assert!(!g_text.contains("sdk-planning-a") && !g_text.contains("sdk-planning-b"));
        }
        assert_ne!(g_history, history(case, "planning-parent").await);
        assert_ne!(g_history, history(case, "original-a").await);
        assert_ne!(g_history, history(case, "original-b").await);
        // Exact SDK source replay must not add a second G input or reset outbox.
        let replay = case.client.lock().await.call(json!({"kind":"submit_authenticated_original", "submission":
            PrivateOriginalSubmit { original: a.clone(), content: a_content, transport_authentication: a_auth }})).await.unwrap();
        let replay: PrivateOriginalSubmission =
            serde_json::from_value(replay["outcome"].clone()).unwrap();
        assert!(matches!(
            replay,
            PrivateOriginalSubmission::Admitted {
                deduplicated: true,
                ..
            }
        ));
        assert_eq!(
            input_ids(case, "planning-lead").await,
            native_inputs,
            "source replay must preserve the exact durable input identities"
        );
        let after = workflow_evidence(case);
        assert_eq!(after.0, workflow);
        assert_eq!(after.1, effects);
        assert_eq!(after.2, acquisitions);
        assert_eq!(history(case, "planning-lead").await, g_history);
        (a, b)
    }

    async fn cancel_parent(&self, case: &mut Case) {
        // Current P source owns this selection; cancellation refuses a new
        // planning lookup rather than turning a stored locator into authority.
        let cancelled = case
            .client
            .lock()
            .await
            .call(json!({"kind":"cancel_original_operation", "original":self.parent.original}))
            .await
            .unwrap();
        assert_eq!(cancelled["kind"], "original_operation_cancelled");
        assert!(
            self.source
                .prepare_workflow_planning(
                    &self.parent.original,
                    self.lead.clone(),
                    self.terms.clone(),
                    self.lineage.clone()
                )
                .await
                .is_err()
        );
    }
}

#[test]
fn two_sdk_originals_admit_one_fresh_dedicated_planning_input() {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let checked = std::panic::AssertUnwindSafe(async {
            let planning = construction::construct(&mut case, pipes).await;
            planning
                .script
                .fresh_find_once
                .store(true, Ordering::Release);
            planning.run(&mut case).await;
        })
        .catch_unwind()
        .await;
        if checked.is_err() {
            case.retain_failed_audits().await;
        }
        let cleanup = case.cleanup().await;
        assert!(
            cleanup.is_empty(),
            "actual SDK and native owners must be joined: {cleanup:?}"
        );
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    });
}

#[cfg(test)]
mod fresh_find_tests {
    use super::*;
    use meerkat_core::authorization::{OperationRefusalKind, OperationRefused};
    use meerkat_core::{ToolError, ToolResult};

    fn request(tag: &str, results: Vec<ToolResult>) -> LlmRequest {
        let mut request = LlmRequest::new(
            MODEL,
            vec![
                Message::User(meerkat_core::UserMessage::text(format!(
                    "sdk-planning-{tag}"
                ))),
                Message::tool_results(results),
            ],
        );
        request.tools.push(Arc::new(ToolDef::new(
            toolkit_mobkit::FIND_AND_INSTALL,
            "Controlled existing Find registration",
            json!({"type":"object"}),
        )));
        request
    }
    fn refusal(call: &str, kind: OperationRefusalKind) -> ToolResult {
        meerkat_core::ops::terminal_tool_outcome_for_error(
            call,
            ToolError::AuthorizationRefused {
                refusal: OperationRefused::new(kind),
            },
        )
        .result
    }
    // Core's certified form of the same refusal, met before the Find's effect.
    fn certified(call: &str) -> ToolResult {
        let mut result = refusal(call, OperationRefusalKind::ReprepareRequired);
        let mut body: Value = serde_json::from_str(&result.text_content()).unwrap();
        body["data"]["tool_invoked"] = json!(false);
        result.set_text_content(body.to_string());
        result
    }
    fn emitted(events: &[LlmEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                LlmEvent::ToolCallComplete { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect()
    }
    fn script() -> PlanningScript {
        let script = PlanningScript::default();
        script.fresh_find_once.store(true, Ordering::Release);
        script
    }
    // Exercise whole model conversations through the real Core error renderer.
    // These are fixture-policy controls, not synthetic native acceptance.
    #[test]
    fn fresh_find_keeps_parallel_originals_and_latest_result_through_history_replay() {
        let script = script();
        assert_eq!(
            emitted(&script.response(&request("a", vec![]))),
            ["a-planning-find"]
        );
        assert_eq!(
            emitted(&script.response(&request("b", vec![]))),
            ["b-planning-find"]
        );
        let refused = certified("a-planning-find");
        assert_eq!(
            emitted(&script.response(&request("a", vec![refused.clone()]))),
            ["a-planning-find-fresh"]
        );
        let a = ToolResult::new(
            "a-planning-find-fresh".into(),
            json!({"worker":{"state":"workflow_lead"}}).to_string(),
            false,
        );
        let b = ToolResult::new(
            "b-planning-find".into(),
            json!({"worker":{"state":"workflow_lead"}}).to_string(),
            false,
        );
        assert!(emitted(&script.response(&request("b", vec![b]))).is_empty());
        let history = request("a", vec![refused.clone(), a.clone(), refused, a]);
        assert!(emitted(&script.response(&history)).is_empty());
        assert!(emitted(&script.response(&history)).is_empty());
        script.assert_fresh_find_attempts();
        assert_eq!(script.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn fresh_find_refuses_unissued_changed_or_incomplete_attempt_history() {
        let run = |results: Vec<ToolResult>| {
            let script = script();
            script.response(&request("a", vec![]));
            let refused = certified("a-planning-find");
            script.response(&request("a", vec![refused]));
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                script.response(&request("a", results))
            }))
        };
        assert!(
            run(vec![certified("a-planning-find")]).is_err(),
            "replayed first refusal cannot issue another fresh attempt before its result"
        );
        assert!(
            run(vec![ToolResult::new(
                "a-planning-find".into(),
                "{}".into(),
                false
            )])
            .is_err(),
            "a prior result cannot change into success"
        );
        let script = script();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| script.response(&request(
                "a",
                vec![ToolResult::new(
                    "a-planning-find-fresh".into(),
                    "{}".into(),
                    false
                )]
            ))))
            .is_err(),
            "an unissued fresh success is not accepted"
        );
    }

    #[test]
    fn fresh_find_is_opt_in_and_refuses_other_errors_and_exhausted_retry() {
        let mut changed = certified("a-planning-find");
        let mut body: Value = serde_json::from_str(&changed.text_content()).unwrap();
        body["data"]["kind"] = json!("unknown");
        changed.set_text_content(body.to_string());
        let mut missing = certified("a-planning-find");
        let mut body: Value = serde_json::from_str(&missing.text_content()).unwrap();
        body.as_object_mut().unwrap().remove("data");
        missing.set_text_content(body.to_string());
        let errors = vec![
            refusal("a-planning-find", OperationRefusalKind::Denied),
            refusal("a-planning-find", OperationRefusalKind::MalformedFacts),
            meerkat_core::ops::terminal_tool_outcome_for_error(
                "a-planning-find",
                ToolError::OperationAuthorizationUnavailable,
            )
            .result,
            ToolResult::new("a-planning-find".into(), "not JSON".into(), true),
            changed,
            missing,
            // Uncertified: no claim that the effect did not run.
            refusal("a-planning-find", OperationRefusalKind::ReprepareRequired),
        ];
        for error in errors {
            let script = script();
            script.response(&request("a", vec![]));
            assert!(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || script.response(&request("a", vec![error]))
                ))
                .is_err()
            );
        }
        let strict = PlanningScript::default();
        strict.response(&request("a", vec![]));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || strict.response(&request("a", vec![certified("a-planning-find")]))
            ))
            .is_err()
        );
        let bounded = script();
        bounded.response(&request("a", vec![]));
        bounded.response(&request("a", vec![certified("a-planning-find")]));
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || bounded.response(&request("a", vec![certified("a-planning-find-fresh")]))
            ))
            .is_err()
        );
    }
}
