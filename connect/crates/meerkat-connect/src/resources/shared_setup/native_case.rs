//! Controlled native SDK qualification of physical sharing and delivery.
//! Two real Original routes/helpers use one inert worker. No service activation,
//! useful business tool, stock application restart or large fan-out is claimed.
use super::native_case_artifacts::publish;
mod authenticated_helper_roles;
mod find_feedback;
mod observe_controls;
mod planning_admission;
mod post_effect_controls;
mod work_step_controls;
use super::sdk_host::{SdkClient, SdkLaunchOwner, SdkParent};
use super::*;
use async_trait::async_trait;
use futures_util::FutureExt;
use meerkat::{AgentFactory, Config, FactoryAgentBuilder};
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, ResourceDomain},
    evidence::EvidenceId,
    work_association::{GrantTarget, WorkAuthorityBasis},
};
use meerkat_core::{
    AgentToolDispatcher, CompiledApplicationToolPolicy, CompiledMemberToolAction,
    CompiledMemberToolGrant, CompiledMemberToolGrants, CompiledPolicySourceProvenance,
    CompiledToolConsequence, ControllerModelFacts, ControllerModelSelection, Message,
    MobMemberBinding, PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision,
    PreparedToolCatalog, PreparedToolSurface, Provider, RealmLocator, SessionId,
    SessionLlmIdentity, StopReason, ToolDef, ToolIdentity, ToolSurfaceSelection,
    auth::{PrincipalKind, PrincipalRef, TrustDomainId},
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PlainModelRoute,
    PreparedLlmRequest,
};
use meerkat_mob::{
    AgentIdentity as MemberIdentity, MemberDestinationSelection, MemberRuntimeBuildIdentity,
    MemberRuntimeRegistrationHook, MobDefinition, MobError, MobId, MobRuntimeMode, MobStorage,
    Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_mobkit::{
    DiscoverySpec, MetaConfig, MobBootstrapSpec, UnifiedRuntime,
    identity_first::{
        AgentAddressability, AgentIdentity, DurabilityPolicy, DurableAgentSpec,
        IdentityFirstRuntimeContext, IdentityLifecycleState, IdentityRuntime,
        IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
        MutableRosterProvider,
    },
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_toolkit::{
    authority::{Action, ActionPermission, Ceiling},
    host_lifetime::policy::*,
    setup::WorkAdmissionStepOptions,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::PermissionsExt,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use toolkit_mobkit::{
    host_gateway::{PreparedSetupApplication, ToolkitGatewayHost},
    installed_host::{
        PreparedInstallationOwner,
        acquisition::{
            InstalledAcquisitionTools, InstalledSetupBootstrap, InstalledSetupLimits,
            InstalledSetupSelection,
        },
        operator_setup::{
            AgentSetupCapabilityRequest, HelperReportCapabilityRequest, OperatorOwnerStartup,
            SetupTargetRequest,
        },
        preparation::{
            PreparationAuthorities, PreparedControllerRequest, PreparedControllerTarget,
            PreparedHelperCatalogRequest, PreparedHelperCatalogScope, PreparedHelperMcpRequest,
        },
        tool_policy::PreparedInstalledToolPolicy,
    },
    private_host::{PrivateHostConnection, PrivateHostStartup},
    private_originals::{PrivateAuthenticatedEvent, PrivateSourceEvidence},
};
use toolkit_package::{Id, canonical_json};

const BOUND: Duration = Duration::from_secs(45);
const MOB: &str = "sdk-shared-preparation";
const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://sdk-controlled-model.invalid/messages";
const IDENTITIES: [&str; 4] = ["original-a", "original-b", "helper-a", "helper-b"];
const DETAIL: &str =
    "The inert software worker completed; this controlled fixture has no service activation.";
fn id(s: &str) -> Id {
    Id::new(s).unwrap()
}
fn evidence(s: &str) -> EvidenceId {
    EvidenceId::new(s).unwrap()
}
fn digest(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
fn principal(s: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        s,
        TrustDomainId::new("sdk-shared-control").unwrap(),
    )
    .unwrap()
}
fn member(s: &str) -> MobMemberBinding {
    MobMemberBinding {
        mob_id: MOB.into(),
        role: s.into(),
        member: meerkat_mobkit::member_comms_id::roster_member_id_for_identity(s).to_string(),
    }
}
fn model_selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        SessionLlmIdentity {
            model: MODEL.into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(json!({"realm":"sdk-shared-test","account":"controlled-model"}))
            .unwrap(),
        "sdk-controlled-profile".into(),
        "sdk-controlled-transport".into(),
    )
}
fn controller_facts() -> ControllerModelFacts {
    ControllerModelFacts::new(model_selection(), ENDPOINT.into(), MODEL.into())
}
fn model_facts() -> meerkat_core::authorization::ModelAuthorizationFacts {
    let selected = model_selection();
    meerkat_core::authorization::ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: MODEL.into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("sdk-controlled-profile".into()),
        backend_kind: "sdk-controlled-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(selected.credential().clone()),
        usage: meerkat_core::authorization::ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

#[derive(Default)]
struct Script {
    authenticated_helper: Option<Arc<authenticated_helper_roles::Control>>,
    selected: std::sync::OnceLock<Value>,
    seen_results: Mutex<BTreeMap<String, meerkat_core::ToolResult>>,
    find_deadline: std::sync::OnceLock<tokio::time::Instant>,
    check_inputs:
        Mutex<BTreeMap<String, (meerkat_core::OperationExecutionScope, meerkat_core::RunId)>>,
    find_only: bool,
    find_refusals: find_feedback::RefusalMode,
    injected_find: Mutex<BTreeMap<String, String>>,
    calls: Mutex<Vec<(String, String, Value)>>,
    results: Mutex<BTreeMap<String, Value>>,
    finished: AtomicUsize,
    cancel_requested: AtomicBool,
    // Held by A's model across one whole response (its check of the cancel
    // flag, its recording of results and its emission of a call), and by
    // the case across setting the flag and recording A's pending calls: a
    // response is wholly before the cancel or wholly after it.
    cancel_section: Mutex<()>,
    // Find answers of retry_safe this case met, each replayed by a fresh Find.
    retry_safe_answers: AtomicUsize,
    // The runs each member's input was served in, in order: a later run of
    // the same input is Core's rerun of it.
    runs: Mutex<BTreeMap<(String, String), Vec<String>>>,
    // Each member's call ID prefixes, one per run attempt, in order.
    prefixes: Mutex<BTreeMap<String, Vec<String>>>,
    // A's helper's calls still pending when the case cancelled A: emitted in
    // its current run, with no result recorded then.
    pending_at_cancel: Mutex<std::collections::BTreeSet<String>>,
    // A control's own condition for cancelling A. Never set in the case
    // itself, so the case's timing is unchanged.
    cancel_gate: std::sync::OnceLock<CancelGate>,
}

/// A control's condition for the case's cancel of A: once both helpers have
/// joined, the case waits for it within BOUND, before it reviews the
/// approvals and cancels A. A gate never met fails the case with its name.
struct CancelGate {
    name: &'static str,
    open: Box<dyn Fn() -> bool + Send + Sync>,
    opened: AtomicBool,
}
/// The input and the run a model request was made in, as Core's binding
/// names them.
struct ServedRun {
    input: String,
    run: String,
}
/// Whether call `id` was emitted in the run whose call ID prefix is
/// `prefix`: a later run's prefix extends its member's with `-run{attempt}`.
fn in_run(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|step| {
            !step
                .strip_prefix("run")
                .and_then(|rest| rest.split('-').next())
                .is_some_and(|attempt| {
                    !attempt.is_empty() && attempt.bytes().all(|byte| byte.is_ascii_digit())
                })
        })
}
impl Script {
    /// The call ID prefix of `member`'s run: the member itself in the first
    /// run of an input, `{member}-run{attempt}` in a later run of the same
    /// input, as a provider issues fresh call IDs in a new run. Core runs
    /// an input again after a run ended without committing its turn (a
    /// typed refusal of its model request): each such run is written as
    /// `sdk_model_turn_rerun`, and the census counts it.
    fn run_prefix(&self, member: &str, served: Option<&ServedRun>) -> String {
        let attempt = served.map_or(1, |served| {
            let mut runs = self.runs.lock().unwrap();
            let seen = runs
                .entry((member.to_owned(), served.input.clone()))
                .or_default();
            if let Some(index) = seen.iter().position(|run| run == &served.run) {
                return index + 1;
            }
            seen.push(served.run.clone());
            let attempt = seen.len();
            if attempt > 1 {
                eprintln!(
                    "{}",
                    json!({
                        "event":"sdk_model_turn_rerun", "member":member,
                        "input":served.input, "run":served.run, "attempt":attempt,
                    })
                );
            }
            attempt
        });
        let prefix = if attempt == 1 {
            member.to_owned()
        } else {
            format!("{member}-run{attempt}")
        };
        let mut prefixes = self.prefixes.lock().unwrap();
        let known = prefixes.entry(member.to_owned()).or_default();
        if !known.contains(&prefix) {
            known.push(prefix.clone());
        }
        prefix
    }
    /// `member`'s call ID prefixes, one per run attempt served, in order.
    fn run_prefixes(&self, member: &str) -> Vec<String> {
        self.prefixes
            .lock()
            .unwrap()
            .get(member)
            .cloned()
            .unwrap_or_else(|| vec![member.to_owned()])
    }
    /// The call ID prefix of `member`'s latest run.
    fn current_prefix(&self, member: &str) -> String {
        self.run_prefixes(member)
            .pop()
            .unwrap_or_else(|| member.to_owned())
    }
    /// Record `member`'s calls still pending as the case cancels its
    /// original: emitted in its latest run, with no result recorded yet.
    fn record_pending_at_cancel(&self, member: &str) {
        let prefix = self.current_prefix(member);
        let emitted = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(id, _, _)| in_run(id, &prefix))
            .map(|(id, _, _)| id.clone())
            .collect::<Vec<_>>();
        let seen = self.seen_results.lock().unwrap();
        *self.pending_at_cancel.lock().unwrap() = emitted
            .into_iter()
            .filter(|id| !seen.contains_key(id))
            .collect();
    }
    fn require_loaded_catalog(prefix: &str, value: Value, names: &[&str]) {
        #[derive(serde::Deserialize)]
        struct LoadedCatalog {
            catalog_exact: bool,
            accepted_names: Vec<String>,
            noop_names: Vec<String>,
            resolutions: Vec<meerkat_core::tool_catalog::ToolCatalogLoadResolution>,
        }
        let loaded: LoadedCatalog = serde_json::from_value(value)
            .unwrap_or_else(|_| panic!("{prefix}-load: malformed catalog response"));
        let expected: BTreeSet<_> = names.iter().copied().collect();
        let actual: BTreeSet<_> = loaded
            .resolutions
            .iter()
            .map(|row| row.name.as_str())
            .collect();
        assert!(
            actual == expected && loaded.resolutions.len() == expected.len(),
            "{prefix}-load: catalog resolutions must cover each exact requested name once"
        );
        // Loading is a successful control-tool result even when individual
        // names are refused. Retain the typed resolution before asserting it.
        for row in &loaded.resolutions {
            eprintln!(
                "{}",
                json!({
                    "event":"d5_sdk_catalog_resolution", "call":format!("{prefix}-load"),
                    "catalog_exact":loaded.catalog_exact, "name":row.name,
                    "accepted":row.accepted, "accepted_noop":row.accepted_noop,
                    "rejected_reason":row.rejected_reason,
                })
            );
        }
        assert!(loaded.catalog_exact, "{prefix}-load: catalog must be exact");
        assert!(
            loaded
                .resolutions
                .iter()
                .all(|row| row.accepted && row.rejected_reason.is_none()),
            "{prefix}-load: every requested name must be accepted; see typed resolutions"
        );
        let accepted: BTreeSet<_> = loaded
            .resolutions
            .iter()
            .filter(|row| !row.accepted_noop)
            .map(|row| row.name.as_str())
            .collect();
        let noop: BTreeSet<_> = loaded
            .resolutions
            .iter()
            .filter(|row| row.accepted_noop)
            .map(|row| row.name.as_str())
            .collect();
        assert!(
            loaded
                .accepted_names
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == accepted
                && loaded.accepted_names.len() == accepted.len()
                && loaded
                    .noop_names
                    .iter()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>()
                    == noop
                && loaded.noop_names.len() == noop.len(),
            "{prefix}-load: accepted and noop inventories must match their resolutions"
        );
    }

    fn response(&self, request: &LlmRequest) -> Vec<LlmEvent> {
        self.response_in_run(request, None)
    }

    /// The controlled answer to `request`, made in `served` when Core's
    /// binding names its input and run.
    fn response_in_run(&self, request: &LlmRequest, served: Option<&ServedRun>) -> Vec<LlmEvent> {
        let body = serde_json::to_string(&request.messages).unwrap();
        let tag = if body.contains("sdk-child-a") {
            "a"
        } else if body.contains("sdk-child-b") {
            "b"
        } else {
            panic!("controlled response has no exact child input")
        };
        // A's whole response, from its check of the cancel flag to its
        // emission, holds the cancel section: the case's cancel of A falls
        // wholly before or wholly after it.
        let _cancel_section = (tag == "a").then(|| self.cancel_section.lock().unwrap());
        let helper = request
            .tools
            .iter()
            .any(|tool| tool.name == meerkat_toolkit::setup::SETUP_REPORT);
        let member = format!("{tag}-{}", if helper { "helper" } else { "original" });
        // This run's call IDs: a later run of the same input gets fresh ones.
        let prefix = self.run_prefix(&member, served);
        let detail_present = body.contains(DETAIL);
        if tag == "b" {
            eprintln!(
                "{}",
                json!({
                    "event":"sdk_shared_model_request", "tag":tag,
                    "helper":helper, "detail_present":detail_present,
                })
            );
        }
        if helper && self.find_only {
            return done("The focused Find control performs no helper tools.");
        }
        let mut returned = BTreeMap::new();
        for message in &request.messages {
            if let Message::ToolResults { results, .. } = message {
                for result in results {
                    if in_run(&result.tool_use_id, &prefix) {
                        if tag == "a"
                            && result.is_error
                            && self.cancel_requested.load(Ordering::SeqCst)
                        {
                            self.results.lock().unwrap().insert(
                                result.tool_use_id.clone(),
                                json!({"cancelled_result":result.text_content()}),
                            );
                            return done("The caller cancelled this independent setup request.");
                        }
                        let value = serde_json::from_str::<Value>(&result.text_content())
                            .unwrap_or_else(|_| json!({"text":result.text_content()}));
                        let operation = self
                            .calls
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(key, _, _)| key == &result.tool_use_id)
                            .map(|(_, name, _)| name.clone())
                            .expect("result must name an actual controlled call");
                        let code = match value.get("error").and_then(Value::as_str) {
                            Some("operation_authorization_unavailable") => {
                                "operation_authorization_unavailable"
                            }
                            Some("operation_refused") => "operation_refused",
                            Some("access_denied") => "access_denied",
                            Some("tool_not_found") => "tool_not_found",
                            Some("tool_unavailable") => "tool_unavailable",
                            Some(_) => "other_error",
                            None => "none",
                        };
                        // Keep only typed, bounded error fields from the actual
                        // result. Non-JSON text and all other fields stay private.
                        let refusal = if result.is_error {
                            let field = |name: &str| {
                                value.get(name).and_then(Value::as_str).map(|text| {
                                    text.chars()
                                        .filter(|ch| !ch.is_control())
                                        .take(512)
                                        .collect::<String>()
                                })
                            };
                            json!({"error":field("error"), "message":field("message")})
                        } else {
                            Value::Null
                        };
                        let first_observation = self.observe_result(result);
                        if first_observation {
                            eprintln!(
                                "{}",
                                json!({
                                    "event":"d5_sdk_tool_result", "member":member,
                                    "call":result.tool_use_id, "operation":operation,
                                    "is_error":result.is_error, "error_code":code,
                                    "refusal":refusal,
                                })
                            );
                        }
                        // Counted for the census, never a gate: the model
                        // replays a retry_safe Find with its one fresh Find.
                        if first_observation
                            && !helper
                            && operation == toolkit_mobkit::FIND_AND_INSTALL
                            && find_feedback::retry_safe(result)
                        {
                            let answers =
                                self.retry_safe_answers.fetch_add(1, Ordering::SeqCst) + 1;
                            eprintln!(
                                "{}",
                                json!({
                                    "event":"sdk_find_retry_safe", "member":member,
                                    "call":result.tool_use_id, "answers":answers,
                                })
                            );
                        }
                        let bounded_find_feedback = !helper
                            && operation == toolkit_mobkit::FIND_AND_INSTALL
                            && (find_feedback::canonical_unavailable(result)
                                || find_feedback::certified_reprepare(result));
                        // The public refusal does not identify its cause. The
                        // settled test audit must prove pre-entry change later.
                        let bounded_check_feedback = helper
                            && operation == "setup_selected_check"
                            && result.tool_use_id == format!("{prefix}-check")
                            && find_feedback::canonical_check_refusal(result);
                        // Core's certified NotDispatched refusal of a helper
                        // call's first attempt (the tool did not run, nothing
                        // was recorded) permits that call's one fresh
                        // attempt. An uncertified refusal, or a refusal of
                        // the fresh attempt, still fails the case.
                        let bounded_retry_feedback =
                            helper && find_feedback::helper_retry_allowed(result);
                        assert!(
                            !result.is_error
                                || bounded_find_feedback
                                || bounded_check_feedback
                                || bounded_retry_feedback,
                            "{prefix}: native call {} refused with {code}: {refusal}",
                            result.tool_use_id
                        );
                        if !result.is_error {
                            returned.insert(result.tool_use_id.clone(), value.clone());
                        }
                        self.results
                            .lock()
                            .unwrap()
                            .insert(result.tool_use_id.clone(), value);
                    }
                }
            }
        }
        let call = |suffix: &str, name: &str, args: Value| {
            let key = format!("{prefix}-{suffix}");
            let offered = request.tools.iter().any(|tool| tool.name == name);
            eprintln!(
                "{}",
                json!({
                    "event":"d5_sdk_tool_selection", "member":member,
                    "call":key, "operation":name, "offered":offered,
                })
            );
            assert!(offered, "{key}: the actual request must offer {name}");
            let mut calls = self.calls.lock().unwrap();
            assert!(
                !calls.iter().any(|(prior, _, _)| prior == &key),
                "the controlled model must emit each actual call ID once"
            );
            calls.push((key.clone(), name.into(), args.clone()));
            drop(calls);
            vec![
                LlmEvent::ToolCallComplete {
                    id: key,
                    name: name.into(),
                    args,
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
            ]
        };
        let has = |suffix: &str| returned.contains_key(&format!("{prefix}-{suffix}"));
        let get = |suffix: &str| returned.get(&format!("{prefix}-{suffix}")).unwrap().clone();
        if !helper {
            // The follow-up is a distinct native input in the same Original.
            // Its actual injected setup result, not a tool being attached, is
            // what makes this controlled response terminal.
            if detail_present {
                if tag == "b" {
                    eprintln!(
                        "{}",
                        json!({
                            "event":"sdk_shared_original_response", "tag":tag,
                            "branch":"setup_result",
                        })
                    );
                }
                self.finished.fetch_add(1, Ordering::SeqCst);
                return done(
                    "The shared physical preparation completed, but no service was activated.",
                );
            }
            let args = find_feedback::request_arguments(tag);
            match self.find_choice(&prefix, &args) {
                find_feedback::FindChoice::Emit(suffix) => {
                    assert!(
                        tokio::time::Instant::now()
                            < *self
                                .find_deadline
                                .get()
                                .expect("the existing scenario deadline is retained before launch")
                    );
                    return call(suffix, toolkit_mobkit::FIND_AND_INSTALL, args);
                }
                find_feedback::FindChoice::Accepted => (),
                find_feedback::FindChoice::Exhausted if self.find_only => {
                    return done("Both bounded Find proposals were refused before entry.");
                }
                find_feedback::FindChoice::Exhausted => {
                    panic!(
                        "{prefix}: both bounded Find proposals were spent (refused before entry or answered retry_safe)"
                    )
                }
            }
            if tag == "b" {
                eprintln!(
                    "{}",
                    json!({
                        "event":"sdk_shared_original_response", "tag":tag,
                        "branch":"awaiting_setup_result",
                    })
                );
            }
            return done(
                "The setup helper is running; its existing owner will deliver the result.",
            );
        }
        let names = [
            "setup_selected_check",
            "setup_selected_install",
            "setup_job",
        ];
        // Each helper call's current attempt: its one fresh attempt once the
        // first was refused (only a tolerated refusal gets this far).
        let load = self.retry_suffix(&prefix, "load");
        if !has(&load) {
            return call(
                &load,
                meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME,
                json!({"names":names}),
            );
        }
        Self::require_loaded_catalog(&prefix, get(&load), &names);
        let check = self.check_suffix(&prefix);
        if !has(check) {
            assert!(tokio::time::Instant::now() < *self.find_deadline.get().unwrap());
            return call(
                check,
                "setup_selected_check",
                self.selected.get().unwrap().clone(),
            );
        }
        let checked = get(check);
        let install = self.retry_suffix(&prefix, "install");
        if !has(&install) {
            return call(
                &install,
                "setup_selected_install",
                json!({
            "validation_id":checked["validation_id"], "effect_sha256":checked["effect_sha256"]}),
            );
        }
        let installed = get(&install);
        let job = installed.get("job").unwrap_or(&installed);
        let job_call = self.retry_suffix(&prefix, "job");
        if !has(&job_call) {
            return call(&job_call, "setup_job", json!({"job_id":job["id"]}));
        }
        let observed = get(&job_call);
        let observed = observed.get("job").unwrap_or(&observed);
        assert_eq!(
            observed["status"], "succeeded",
            "current member readback must succeed"
        );
        let report = self.retry_suffix(&prefix, "report");
        if !has(&report) {
            return call(
                &report,
                meerkat_toolkit::setup::SETUP_REPORT,
                json!({"kind":"unavailable","detail":DETAIL}),
            );
        }
        done("Physical preparation was observed; activation remains unavailable.")
    }
}
fn done(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta {
            delta: text.into(),
            meta: None,
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::success(StopReason::EndTurn),
        },
    ]
}
#[async_trait]
impl LlmClient for Script {
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
        let check_input = (
            prepared.binding().facts().execution_scope.clone(),
            prepared.binding().facts().run_id.clone(),
        );
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
        if let Some(control) = &self.authenticated_helper {
            use futures_util::StreamExt;
            return Box::pin(
                futures_util::stream::once(async move {
                    control.response(self, request.request()).await
                })
                .flat_map(|events| futures_util::stream::iter(events.into_iter().map(Ok))),
            );
        }
        let served = match &check_input {
            (
                meerkat_core::OperationExecutionScope::RuntimeInput {
                    canonical_input_id, ..
                },
                Some(run),
            ) => Some(ServedRun {
                input: canonical_input_id.to_string(),
                run: run.to_string(),
            }),
            _ => None,
        };
        let events = self.response_in_run(request.request(), served.as_ref());
        for event in &events {
            if let LlmEvent::ToolCallComplete { id, name, .. } = event
                && name == "setup_selected_check"
            {
                assert!(
                    self.check_inputs
                        .lock()
                        .unwrap()
                        .insert(
                            id.clone(),
                            (
                                check_input.0.clone(),
                                check_input.1.clone().expect("actual helper run")
                            )
                        )
                        .is_none()
                );
            }
        }
        Box::pin(futures_util::stream::iter(events.into_iter().map(Ok)))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

// This host deliberately has no business credentials or other MCP destination.
// The public composition still takes and preserves its actual prior provider.
struct NoBusinessContext;
#[async_trait]
impl meerkat_mcp::McpCallContextProvider for NoBusinessContext {
    async fn prepare(
        &self,
        _: meerkat_mcp::McpCallTarget<'_>,
        _: meerkat_core::ToolCallView<'_>,
        _: &meerkat_core::ToolDispatchContext,
    ) -> Result<Option<meerkat_mcp::McpCallContext>, meerkat_mcp::McpCallContextError> {
        Ok(None)
    }
}

struct CurrentRoster;
impl meerkat_mobkit::identity_first::RosterProjection for CurrentRoster {
    fn project(
        &self,
        _: &meerkat_mobkit::identity_first::RosterContext,
        rows: Vec<DurableAgentSpec>,
    ) -> Result<Vec<DurableAgentSpec>, String> {
        Ok(rows)
    }
}

struct SurfaceOwner {
    members: Vec<PreparedNativeMember>,
    policy: PreparedInstalledToolPolicy,
    surfaces: Mutex<BTreeMap<String, (SessionId, Vec<ToolIdentity>)>>,
}
#[async_trait]
impl MemberRuntimeRegistrationHook for SurfaceOwner {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        let selected = self
            .members
            .iter()
            .find(|row| &row.member == identity.member())
            .ok_or_else(|| MobError::WiringError("SDK member not selected".into()))?;
        Ok(MemberDestinationSelection {
            logical_owner: selected.logical_owner.clone(),
            context: identity.session_document().clone(),
            context_generation: selected.context_generation,
            audience: selected.audience.clone(),
        })
    }
    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        // This immutable selected owner has no destination mutation in either
        // hook. Reopened members must match the same exact selected facts.
        self.select_destination(identity).await
    }
    async fn prepare_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, MobError> {
        let mut full = surface.tools.clone();
        if let PreparedToolCatalog::Complete(entries) = &surface.catalog {
            for entry in entries {
                match full.iter().find(|old| old.name == entry.identity.name) {
                    Some(old) if old != &entry.identity => {
                        return Err(MobError::WiringError("conflicting native catalog".into()));
                    }
                    Some(_) => (),
                    None => full.push(entry.identity.clone()),
                }
            }
        }
        let session = SessionId::parse(identity.session_document().as_str())
            .map_err(|_| MobError::WiringError("native session".into()))?;
        let mut captures = self.surfaces.lock().unwrap();
        assert!(
            captures
                .insert(identity.member().member.clone(), (session, full))
                .is_none(),
            "one native preparation per member"
        );
        self.policy
            .select(identity.member(), surface)
            .map_err(|_| MobError::WiringError("explicit SDK tool policy".into()))
    }
    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[ToolDef],
    ) -> Result<(), MobError> {
        let captures = self.surfaces.lock().unwrap();
        let (session, full) = captures
            .get(&identity.member().member)
            .ok_or_else(|| MobError::WiringError("missing native preparation".into()))?;
        if session.to_string() != identity.session_document().as_str()
            || surface.iter().any(|tool| !full.contains(&tool.identity()))
        {
            return Err(MobError::WiringError(
                "changed actual native surface".into(),
            ));
        }
        Ok(())
    }
}

fn prepared_inputs() -> PreparedLocalPolicy {
    let realm = meerkat_core::mob_realm_id(MOB).unwrap();
    let audience = AudienceRef::Principal {
        principal: principal("human"),
    };
    let members = IDENTITIES
        .iter()
        .map(|name| PreparedNativeMember {
            member: member(name),
            logical_owner: principal(name),
            context_generation: 1,
            audience: audience.clone(),
            setup_capability: None,
        })
        .collect();
    let routes = IDENTITIES[..2]
        .iter()
        .map(|name| CompiledNativeSourceRoute {
            id: id(name),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member: member(name),
                requester: principal("human"),
                ingress_actor: principal("sdk"),
            },
            application_parent: None,
            controller_parent: None,
            logical_executor: principal(name),
            represented_subject: None,
            source_authority: principal("sdk"),
            authentication_domain: ResourceDomain {
                authority: principal("sdk"),
                namespace: "login".into(),
            },
            event_domain: ResourceDomain {
                authority: principal("sdk"),
                namespace: "events".into(),
            },
        })
        .collect();
    PreparedLocalPolicy {
        deployment: id("sdk-shared-host"),
        components: vec![],
        channels: vec![],
        native_host: PreparedNativeHost {
            authentication_domain: None,
            event_domain: None,
            realm,
            grant_administrator: principal("application"),
            remote_operator: Some(principal("admin")),
            service_parent_administration: None,
            ingress_domain: ResourceDomain {
                authority: principal("sdk"),
                namespace: "ingress".into(),
            },
            members,
            routes,
            member_classes: vec![],
        },
    }
}
fn policy() -> PreparedInstalledToolPolicy {
    let mut helper_names = BTreeSet::new();
    helper_names.extend(crate::setup::PREPARATION_TOOLS.iter().copied());
    helper_names.extend(crate::setup::ACQUISITION_TOOLS.iter().copied());
    helper_names.extend(crate::service_install::INSTALL_TOOLS.iter().copied());
    helper_names.extend([
        "setup_selected_check",
        meerkat_toolkit::setup::SETUP_REPORT,
        meerkat_core::tool_catalog::TOOL_CATALOG_LOAD_NAME,
        meerkat_core::tool_catalog::TOOL_CATALOG_SEARCH_NAME,
    ]);
    let mut members: Vec<_> = IDENTITIES
        .iter()
        .enumerate()
        .map(|(index, name)| CompiledMemberToolGrants {
            member_identity: member(name).member,
            grants: if index < 2 {
                vec![CompiledMemberToolGrant {
                    tool_name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                    action: CompiledMemberToolAction::Invoke,
                    consequence: CompiledToolConsequence::R2,
                }]
            } else {
                helper_names
                    .iter()
                    .map(|name| CompiledMemberToolGrant {
                        tool_name: (*name).into(),
                        action: CompiledMemberToolAction::Invoke,
                        consequence: CompiledToolConsequence::R2,
                    })
                    .collect()
            },
        })
        .collect();
    // Sort complete rows after assigning each selected member its own grants.
    members.sort_by(|left, right| left.member_identity.cmp(&right.member_identity));
    let value = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("sdk-control").unwrap(),
        PolicyId::new("sdk-control").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "sdk-explicit-construction".into(),
            source_digest: PolicyDigest::from_canonical_bytes(
                b"two explicit independent SDK routes",
            ),
        },
        members,
    )
    .unwrap();
    PreparedInstalledToolPolicy::from_canonical(&value.canonical_json().unwrap()).unwrap()
}
fn definition() -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from(MOB));
    for (index, name) in IDENTITIES.iter().enumerate() {
        definition.profiles.insert(
            ProfileName::from(*name),
            ProfileBinding::Inline(Box::new(Profile {
                model_fallback: None,
                model: MODEL.into(),
                provider: None,
                self_hosted_server_id: None,
                auth_binding: None,
                image_generation_provider: None,
                auto_compact_threshold: None,
                resume_overrides: vec![],
                skills: vec![],
                tools: ToolConfig {
                    comms: true,
                    rust_bundles: if index < 2 {
                        vec![toolkit_mobkit::FIND_AND_INSTALL_BUNDLE.into()]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                peer_description: format!("Controlled SDK {name}"),
                external_addressable: true,
                backend: None,
                runtime_mode: MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
                durable: false,
            })),
        );
    }
    definition
}
fn selection(name: &str) -> InstalledSetupSelection {
    InstalledSetupSelection {
        catalog_capability: None,
        report_capability: None,
        member: member(name),
        ingress_actor: principal("sdk"),
        logical_executor: principal(name),
        represented_subject: None,
        controller_parent: vec![],
        operation_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![],
        },
    }
}
fn authorities(index: usize) -> PreparationAuthorities {
    let original = IDENTITIES[index];
    let helper = IDENTITIES[index + 2];
    let target = |suffix: &str, target| SetupTargetRequest {
        target,
        issuance_id: evidence(&format!("{original}-{suffix}")),
    };
    let mut helper_targets = vec![
        target("catalog", None),
        target(
            "mcp",
            Some(GrantTarget::McpServer {
                server: "toolkit-setup".into(),
            }),
        ),
        target(
            "report",
            Some(GrantTarget::RegisteredToolBundleOperation {
                bundle: meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE.into(),
                operation: meerkat_toolkit::setup::SETUP_REPORT.into(),
            }),
        ),
    ];
    helper_targets.sort_by(|a, b| a.target.cmp(&b.target));
    PreparationAuthorities {
        shared_workflow_setup: vec![],
        helper_mcp: Some(PreparedHelperMcpRequest {
            issuance_id: evidence(&format!("{helper}-mcp")),
        }),
        helper_catalog: Some(PreparedHelperCatalogRequest {
            issuance_id: evidence(&format!("{helper}-catalog")),
            scope: PreparedHelperCatalogScope::CurrentHelperSuccessors,
        }),
        controllers: vec![
            PreparedControllerRequest {
                target: PreparedControllerTarget::OriginalRoute {
                    route: id(original),
                },
                issuance_id: evidence(&format!("{original}-controller")),
                controller_route: controller_facts().route_key().clone(),
            },
            PreparedControllerRequest {
                target: PreparedControllerTarget::Helper,
                issuance_id: evidence(&format!("{helper}-controller")),
                controller_route: controller_facts().route_key().clone(),
            },
        ],
        agent_setup: vec![AgentSetupCapabilityRequest {
            member: member(original),
            route: id(original),
            issuance_id: evidence(&format!("{original}-find")),
            helper: Some(member(helper)),
            helper_targets,
        }],
        helper_report: Some(HelperReportCapabilityRequest {
            member: member(helper),
            issuance_id: evidence(&format!("{helper}-report")),
            account_issuance_id: None,
            public_issuance_id: None,
        }),
    }
}

struct Case {
    directory: tempfile::TempDir,
    resource: Option<SharedSetupOwner>,
    native: Option<UnifiedRuntime>,
    private: Option<PrivateHostConnection>,
    host: Option<ToolkitGatewayHost>,
    launch: Option<Arc<SdkLaunchOwner>>,
    client: Arc<tokio::sync::Mutex<SdkClient>>,
    script: Arc<Script>,
    audit_store: Option<Arc<dyn meerkat_runtime::store::RuntimeStore>>,
    audit_sessions: BTreeMap<String, SessionId>,
    reviews: Vec<crate::approval::ApprovalPresenter>,
    review_tasks: Vec<tokio::task::JoinHandle<Result<(), String>>>,
    review_close: tokio::sync::watch::Sender<bool>,
    approved: Arc<Mutex<Vec<meerkat_core::ApprovalRecord>>>,
    lifetime: Arc<Mutex<Option<Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>>>>,
}
impl Case {
    fn new(client: Arc<tokio::sync::Mutex<SdkClient>>) -> Self {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        for name in ["data", "control", "activations", "policy/bundle.1"] {
            fs::create_dir_all(directory.path().join("shared").join(name)).unwrap();
        }
        Self {
            directory,
            resource: None,
            native: None,
            private: None,
            host: None,
            launch: None,
            client,
            script: Arc::new(Script::default()),
            audit_store: None,
            audit_sessions: BTreeMap::new(),
            reviews: vec![],
            review_tasks: vec![],
            review_close: tokio::sync::watch::channel(false).0,
            approved: Arc::new(Mutex::new(vec![])),
            lifetime: Arc::new(Mutex::new(None)),
        }
    }
    async fn resource(&mut self) -> Vec<PreparedSharedConnection> {
        let python = PathBuf::from(
            std::env::var_os("TOOLKIT_D5_TEST_PYTHON")
                .expect("executor selects the retained MCP test interpreter"),
        );
        assert!(python.is_absolute());
        let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk/consumer-setup")
            .canonicalize()
            .unwrap();
        let output = self.directory.path().join("physical");
        fs::create_dir(&output).unwrap();
        let stderr = fs::File::create(output.join("owner.stderr")).unwrap();
        let mut child = tokio::process::Command::new(&python)
            .args(["-I", "-B"])
            .arg(source.join("tests/shared_native_owner.py"))
            .arg(&output)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output_reader = BufReader::new(child.stdout.take().unwrap());
        // Retain the process before the first readiness await. The outer case
        // joins this owner even when preparation panics or readiness refuses.
        self.resource = Some(SharedSetupOwner {
            child,
            channel: Arc::new(tokio::sync::Mutex::new(Channel::new(input, output_reader))),
            entries: Arc::new(entry::EntryTasks::default()),
            attempts: std::sync::OnceLock::new(),
            artifact_directory: output.clone(),
            realm: RealmLocator {
                state_root: self.directory.path().to_path_buf(),
                realm: meerkat_core::mob_realm_id(MOB).unwrap(),
            },
            close_identity: None,
            lifetime_sha256: None,
            physical: None,
        });
        let owner = self.resource.as_ref().unwrap();
        let ready = {
            let mut channel = owner.channel.lock().await;
            tokio::time::timeout(BOUND, read_ready(&mut channel.output, 2)).await
        };
        // An owner that is never ready says why on its own stderr, for one an
        // interpreter without the pinned Python set.
        let paths = match ready {
            Ok(Ok(paths)) => paths,
            other => panic!(
                "the shared native owner was not ready ({other:?}); its stderr: {}",
                fs::read_to_string(output.join("owner.stderr")).unwrap_or_default()
            ),
        };
        self.script
            .selected
            .set(
                serde_json::from_slice(&fs::read(output.join("SELECTED-INPUT.json")).unwrap())
                    .unwrap(),
            )
            .unwrap();
        paths.into_iter().enumerate().map(|(index,path)|{
            let bytes=canonical_json(&json!({"mcpServers":{"toolkit-setup":{"command":"/usr/bin/env","args":["-i","PATH=/usr/bin:/bin","PYTHONDONTWRITEBYTECODE=1",python,"-I","-B",source.join("local_session.py"),"--shared-peer",path]}}})).unwrap();
            let connection=crate::setup::PreparedSetupConnection::from_preparer_output(&bytes).unwrap();
            let configuration=crate::setup::PreparedSetupConnection::from_preparer_output(&bytes).unwrap().into_server();
            PreparedSharedConnection{connection,peer:SharedSetupPeer::new(owner.channel.clone(),index,configuration,owner.entries.clone())}
        }).collect()
    }
    async fn construct(&mut self, pipes: toolkit_mobkit::private_host::InheritedHostPipes) {
        let authenticated = self.script.authenticated_helper.clone();
        let authority_requests = |index| match &authenticated {
            Some(_) => authenticated_helper_roles::authorities(index),
            None => authorities(index),
        };
        let peers = self.resource().await;
        let mut prepared_helpers = Vec::new();
        let mut selections = Vec::new();
        let mut installation = Vec::new();
        let owner = PreparedInstallationOwner::open(
            self.directory.path().to_path_buf(),
            prepared_inputs(),
            OperatorOwnerStartup::fresh("sdk-shared-native", 1).unwrap(),
        )
        .unwrap();
        for (index, peer) in peers.into_iter().enumerate() {
            let helper = peer
                .connection
                .for_mobkit_helper_in_place(
                    member(IDENTITIES[index + 2]),
                    crate::setup::SetupHelperTools::CheckedInstallation,
                )
                .unwrap();
            // Native catalog scope needs the materialized helper session.
            // Retain the exact selection now; configure it on the same pending
            // installation owner only after native registration has captured it.
            let selected = selection(IDENTITIES[index + 2]);
            let (approvals, presenter) =
                crate::approval::LocalApprovals::create(self.directory.path()).unwrap();
            self.reviews.push(presenter);
            selections.push(selected);
            prepared_helpers.push(helper.clone());
            installation.push(crate::installed_service_review::SharedSetupInstallation {
                helper,
                peer: peer.peer,
                approvals,
            });
        }
        if let Some(control) = &authenticated {
            control.prepare(&owner, &self.directory.path().canonicalize().unwrap());
        }
        let selected = owner.selected().clone();
        let policy = policy();
        let capture = Arc::new(SurfaceOwner {
            members: selected.native_host.members.clone(),
            policy: policy.clone(),
            surfaces: Mutex::new(BTreeMap::new()),
        });
        let shared = crate::installed_service_review::prepare_shared_setup_installation(
            &owner,
            RealmLocator {
                state_root: self.directory.path().to_path_buf(),
                realm: selected.native_host.realm.clone(),
            },
            installation,
            policy.clone(),
            Arc::new(NoBusinessContext),
        )
        .unwrap();
        let tools = InstalledAcquisitionTools::new(Duration::from_millis(
            InstalledSetupLimits::default().admission_timeout_ms,
        ))
        .unwrap();
        let mut prepared = owner.prepare(policy.clone()).await.unwrap();
        if let Some(control) = &authenticated {
            control.retain_readback(prepared.helper_readback_for_test());
        }
        // The explicit operator is present, so missing session refusal reaches
        // native catalog preflight before issuance on this same pending owner.
        assert!(matches!(
            prepared.configure_held_authorities_with_helper_resources(
                &mut selections[0],
                &authority_requests(0),
                Some(prepared_helpers[0].server()),
                None,
            ),
            Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)
        ));
        let shared = shared.bind_native(
            prepared.take_configuration().unwrap(),
            capture.clone(),
            policy.registry(),
        );
        let registration = shared.registration();
        let participant = Arc::new(
            prepared
                .gateway_runtime(registration.clone(), tools.clone(), [id("human")].into())
                .unwrap(),
        );
        let originals = prepared.originals();
        if let Some(control) = &authenticated {
            control.arm_promotion_diagnostic(&originals);
        }
        let member_mcp_custody = meerkat_mobkit::MemberMcpCustody::new();
        let (factory, mut configuration) = shared
            .configure_factory_with_attempt_guards(
                member_mcp_custody.install(AgentFactory::minimal().comms(true)),
                participant.service_tasks(),
                self.resource.as_ref().unwrap(),
            )
            .unwrap();
        let factory = if let Some(control) = &authenticated {
            factory.with_operation_review(control.review())
        } else {
            factory
        };
        if self.script.find_refusals != find_feedback::RefusalMode::None {
            configuration.operation_owner = Arc::new(find_feedback::FindRefusalOwner {
                inner: configuration.operation_owner.clone(),
                script: self.script.clone(),
            });
        }
        let mut builder = FactoryAgentBuilder::new(factory, Config::default());
        builder.default_llm_client = Some(self.script.clone());
        *builder.default_agent_llm_client_decorator.write().unwrap() = Some(Arc::new(|client| {
            assert!(client.controller_model_selection().as_ref() == Some(&model_selection()));
            client
        }));
        let mob_tools = builder.default_mob_tools.clone();
        let mut runtime_store = meerkat_runtime::store::SqliteRuntimeStore::new(
            self.directory.path().join("native-runtime.sqlite3"),
        )
        .unwrap();
        if authenticated.is_some() {
            // Authenticated consent needs the actual case-owned hosting domain,
            // selected before this same store becomes the native machine.
            let root = self.directory.path().canonicalize().unwrap();
            let hosting_lock_dir = root.join("native-hosting");
            std::fs::create_dir_all(&hosting_lock_dir).unwrap();
            runtime_store =
                runtime_store.with_hosting_paths(meerkat_core::session_hosting::HostingPaths {
                    hosting_lock_dir,
                    cold_delivery_lock: root.join("native-cold-delivery.lock"),
                    database: None,
                });
            assert!(
                meerkat_runtime::store::RuntimeStore::hosting_capability(&runtime_store)
                    .domain_ref()
                    .is_some(),
                "authenticated helper consent needs an actual hosting domain"
            );
        }
        let persistence = meerkat::PersistenceBundle::new_with_local_grant_authorization(
            Arc::new(
                meerkat::SqliteSessionStore::open(
                    self.directory.path().join("native-sessions.sqlite3"),
                )
                .unwrap(),
            ),
            Arc::new(runtime_store),
            Arc::new(meerkat::MemoryBlobStore::new()),
            configuration,
        )
        .unwrap();
        let history = persistence.runtime_store();
        self.audit_store = Some(history.clone());
        let jobs = persistence.job_store();
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service(builder, 4, persistence).unwrap();
        let service = Arc::new(service);
        let graph = meerkat_workgraph::WorkGraphService::with_namespace_grant(
            Arc::new(
                meerkat_workgraph::SqliteWorkGraphStore::open(
                    self.directory.path().join("workgraph.sqlite3"),
                )
                .unwrap(),
            ),
            meerkat_core::service::WorkGraphNamespaceGrant::new(
                selected.native_host.realm.as_str(),
                "setup",
            )
            .unwrap(),
        )
        .unwrap();
        let mut census = meerkat_mobkit::storage_health::ResolvedStorageSummary::new(
            meerkat_mobkit::storage_health::BlobDurability::DeclaredEphemeral,
            None,
        );
        census.slots.push(
            meerkat_mobkit::storage_health::StorageSlotSummary::persistent(
                "workgraph",
                "sdk-sqlite-workgraph",
            ),
        );
        let mut spec =
            MobBootstrapSpec::new(definition(), MobStorage::in_memory(), service.clone())
                .with_member_mcp_custody(member_mcp_custody)
                .with_session_runtime_adapter(machine.clone())
                .unwrap()
                .with_declared_ephemeral_mob_storage()
                .with_workgraph_service(Some(graph.clone()))
                .with_runtime_delivery(meerkat_mobkit::MobRuntimeDelivery::new(
                    history.clone(),
                    jobs,
                ))
                .with_agent_mob_tools(mob_tools)
                .unwrap();
        spec.resolved_storage = Some(census.clone());
        for helper in &prepared_helpers {
            spec = helper.compose(spec).unwrap();
        }
        let actual_definition = spec.definition.clone();
        let spec = participant.prepare_bootstrap(spec).unwrap();
        assert!(spec.workgraph_service().unwrap().namespace_grant() == graph.namespace_grant());
        self.native = Some(
            UnifiedRuntime::bootstrap(
                spec,
                MetaConfig {
                    modules: vec![],
                    discovery: DiscoverySpec {
                        namespace: MOB.into(),
                        modules: vec![],
                    },
                    pre_spawn: vec![],
                },
                BOUND,
            )
            .await
            .unwrap(),
        );
        let native = self.native.as_mut().unwrap();
        // The legacy constructor returns an active runtime that has no
        // bootstrap completion yet; activating it without an identity context
        // mints one (nothing is pending). The identity context below is
        // attached, not installed: this runtime does not bootstrap an attached
        // roster, so the attach keeps that completion for the host to publish.
        native.activate_without_identity_context().await.unwrap();
        participant.bind_runtime(native).unwrap();
        let handle = native.mob_runtime().handle();
        let leases = Arc::new(LocalLeaseProvider::new());
        let bridge = MobSessionBridge::with_session_service(
            handle.clone(),
            native.mob_runtime().session_service().cloned().unwrap(),
        )
        .with_controller_pin_source(service.clone());
        let identities = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
                lease_provider: leases.clone(),
                runtime_instance_id: "sdk-shared-native".into(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(Arc::new(bridge)),
                default_timeout: None,
            })
            .with_runtime_services(
                meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
            ),
        );
        let mut roster = Vec::new();
        for name in IDENTITIES {
            let binding = member(name);
            let native_id = MemberIdentity::from(binding.member.clone());
            // The identity label marks this spawn as the identity's member, as
            // Meta marks its own: registering the identity below then adopts
            // the member as the identity's entry, where an unmarked one is
            // refused as a worker holding the name.
            handle
                .spawn_spec(SpawnMemberSpec::new(name, native_id.clone()).with_labels(
                    BTreeMap::from([("agent_identity".to_owned(), name.to_owned())]),
                ))
                .await
                .unwrap();
            let session = handle
                .get_member(&native_id)
                .await
                .unwrap()
                .unwrap()
                .bridge_session_id()
                .cloned()
                .unwrap();
            self.audit_sessions.insert(name.into(), session.clone());
            let boundary = service
                .acquire_live_session_actor_turn_boundary_lease(&session)
                .await
                .unwrap();
            let pin = service
                .pin_controller_client_for_actor(&boundary)
                .await
                .unwrap();
            drop(boundary);
            assert!(pin.selection() == &model_selection());
            meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
                &machine.generated_auth_lease_handle(),
                pin.selection().credential(),
                &meerkat_core::auth::PersistedTokens::api_key("synthetic-local-native-control"),
                tokio::time::Instant::now() + BOUND,
            )
            .await
            .unwrap();
            let identity = AgentIdentity::parse(name).unwrap();
            let row = DurableAgentSpec {
                identity: identity.clone(),
                profile: ProfileName::from(name),
                addressability: AgentAddressability::Addressable,
                display_name: None,
                labels: BTreeMap::new(),
                context: None,
                additional_instructions: vec![],
                initial_message: None,
                runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
                backend: None,
                binding: None,
                placement: None,
            };
            // Registration retains only the desired identity row. Active
            // state and continuity come from the actual Core member already
            // created above, observed through this runtime's real bridge.
            identities
                .register(row.clone())
                .await
                .expect("the identity registers");
            let observed = identities.status(&identity).await.unwrap();
            assert_eq!(observed.state, IdentityLifecycleState::Active);
            assert_eq!(observed.session_id.as_ref(), Some(&session));
            roster.push(row);
        }
        let roster = Arc::new(MutableRosterProvider::new(roster));
        native.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
            identities,
            roster.clone(),
            None,
            None,
            Some(actual_definition.clone()),
        )));
        if let Some(control) = &authenticated {
            control.configure_runtime(native);
        }
        participant.bind_identity_runtime(native).unwrap();
        let captured = capture.surfaces.lock().unwrap().clone();
        assert!(
            self.script.calls.lock().unwrap().is_empty(),
            "materialization ran no agent turn"
        );
        for (index, helper) in prepared_helpers.iter().enumerate() {
            let (session, _) = captured.get(&helper.member().member).unwrap();
            let native_member = handle
                .get_member(&MemberIdentity::from(helper.member().member.clone()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(native_member.bridge_session_id(), Some(session));
            prepared
                .configure_held_authorities_with_helper_resources(
                    &mut selections[index],
                    &authority_requests(index),
                    Some(helper.server()),
                    Some((helper.member(), session)),
                )
                .unwrap();
            // Pending non-publication is covered by the owning-crate control;
            // this consumer has no private route or source-store access.
        }
        // Exact-ID reobservation of the last helper is idempotent; only this
        // existing operation finalizes the complete two-helper selection.
        let last = prepared_helpers.len() - 1;
        let helper = &prepared_helpers[last];
        let (session, _) = captured.get(&helper.member().member).unwrap();
        let selected = prepared
            .finalize_authorities_with_helper_resources(
                &mut selections[last],
                &authority_requests(last),
                Some(helper.server()),
                Some((helper.member(), session)),
            )
            .unwrap();
        assert_eq!(selected.native_host.routes.len(), 2);
        assert!(
            prepared
                .configure_held_authorities_with_helper_resources(
                    &mut selections[last],
                    &authority_requests(last),
                    Some(helper.server()),
                    Some((helper.member(), session)),
                )
                .is_err(),
            "finalized authority cannot be replaced"
        );
        assert!(
            self.script.calls.lock().unwrap().is_empty(),
            "authority finalization ran no agent turn"
        );
        let (selection, application_reference, application_bytes) = publish(
            self.directory.path(),
            &selected,
            &policy,
            &captured,
            &actual_definition,
        );
        let client = self.client.clone();
        let finished = Arc::new(Mutex::new(None));
        let saved = finished.clone();
        let final_participant = participant.clone();
        let final_registration = registration.clone();
        let final_policy = prepared.policy();
        if let Some(control) = &authenticated {
            control.retain_selections(selections.clone());
        }
        let helper = selections.remove(0);
        let additional = selections;
        let launch_client = self.client.clone();
        let retained_lifetime = self.lifetime.clone();
        let application = PreparedSetupApplication::from_verified_artifact(
            application_reference,
            application_bytes,
            actual_definition.clone(),
            Arc::new(CurrentRoster),
        )
        .unwrap();
        let expected = actual_definition.clone();
        let projection: toolkit_mobkit::host_gateway::SetupBootstrapProjection = Arc::new(
            move |mut spec| {
                for helper in &prepared_helpers {
                    spec = helper.compose(spec).map_err(|_| {
                        meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection
                    })?;
                }
                if spec.definition != expected {
                    return Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::InvalidSelection);
                }
                Ok(spec)
            },
        );
        let authenticated_host = authenticated.clone();
        let server = async move {
            let startup = PrivateHostStartup::read(pipes).await?;
            PrivateHostConnection::bootstrap_prepared_with_controller_reviews(
                startup,
                move |selection| async move {
                    let (owner, installed) = prepared.adopt(selection)?;
                    let originals = installed.originals();
                    *finished.lock().unwrap() = Some(installed);
                    Ok((owner, originals))
                },
                move |owner, closed, logins| async move {
                    // Retain the actual returned owner before any fallible
                    // adoption. A panic cannot leave only HostLossGuard's
                    // background join holding cleanup custody.
                    *retained_lifetime.lock().unwrap() = Some(owner.clone());
                    let launch = SdkLaunchOwner::new(
                        final_participant.acquisition().unwrap().1,
                        owner.clone(),
                        id("sdk-workgraph-generation"),
                        launch_client,
                        meerkat_workgraph::WorkOwnerKey::agent("sdk-parent").unwrap(),
                        "sdk-independent-children".into(),
                    );
                    launch.capture_prepared_backend(graph, &census)?;
                    let installed = saved.lock().unwrap().take().unwrap();
                    let mut actions = BTreeSet::new();
                    for name in IDENTITIES[..2].iter() {
                        for action in [Action::FindAndInstall, Action::Delegate] {
                            actions.insert(ActionPermission {
                                action,
                                resource: id(&member(name).member),
                            });
                        }
                    }
                    let installed = installed.with_enrolled_bootstrap_acquisition_helpers(
                        InstalledSetupBootstrap {
                            helper,
                            proposal_ceiling: Ceiling {
                                actions,
                                services: BTreeSet::new(),
                                frontends: BTreeSet::new(),
                                output_audiences: [id("human")].into(),
                            },
                            continuation: launch.clone(),
                            backend_generation: id("sdk-workgraph-generation"),
                            references: None,
                            limits: InstalledSetupLimits::default(),
                            work_step: WorkAdmissionStepOptions {
                                worker: id("sdk-setup-worker"),
                                lease_ms: 60_000,
                                owner_timeout_ms: 10_000,
                                settlement_budget_ms: 10_000,
                            },
                        },
                        tools,
                        additional,
                    )?;
                    let installed = if let Some(control) = &authenticated_host {
                        installed.with_operation_review_policy(Some(control.clone()))
                    } else {
                        installed
                    };
                    let host = installed
                        .compose_with_prepared_registration(
                            owner,
                            closed,
                            final_registration,
                            final_policy,
                        )?
                        .with_console_logins(logins)?
                        .with_native_setup_bootstrap(
                            final_participant.acquisition().unwrap().1,
                            projection,
                            application,
                        )?;
                    let host = if let Some(control) = &authenticated_host {
                        control.compose(host)?
                    } else {
                        host
                    };
                    let host = final_participant.adopt(host).await?;
                    Ok((host, launch))
                },
            )
            .await
        };
        let request = async move {
            let mut client = client.lock().await;
            let reply = client
                .call(json!({"kind":"bootstrap","selection":selection}))
                .await;
            if reply.is_err() {
                let _ = client.close().await;
            }
            reply
        };
        // A failed response observer closes only its SDK side, then continues
        // awaiting the real bootstrap owner. No timeout drops that owner.
        let (reply, server) =
            tokio::join!(request, std::panic::AssertUnwindSafe(server).catch_unwind());
        let ((host, launch), private, local, remote) = match server {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => panic!("private bootstrap refused after owner settlement: {error}"),
            Err(payload) => std::panic::resume_unwind(payload),
        };
        self.host = Some(host);
        self.launch = Some(launch);
        self.private = Some(private);
        drop(local);
        drop(remote);
        assert_eq!(reply.unwrap()["kind"], "bootstrap_accepted");
        assert_eq!(capture.surfaces.lock().unwrap().len(), 4);
        assert!(Arc::strong_count(&originals) > 1);
        if let Some(control) = &authenticated {
            control.bind_routes(self.native.as_ref().unwrap(), self.host.as_ref().unwrap());
        }
    }

    fn approve_independently(&mut self) {
        for mut presenter in std::mem::take(&mut self.reviews) {
            let mut close = self.review_close.subscribe();
            let approved = self.approved.clone();
            self.review_tasks.push(tokio::spawn(async move {
                // Every prompt of this helper's presenter is approved: its
                // install, and the one fresh install the case allows after a
                // first install refused certified not dispatched after its
                // review. The proofs check which calls were reviewed.
                let mut decided = Vec::new();
                loop {
                    let prompt = tokio::select! {
                        _ = close.changed() => break,
                        prompt = presenter.next() => prompt,
                    };
                    let Some(mut prompt) = prompt else {
                        if decided.is_empty() {
                            return Err("software presenter closed before review".into());
                        }
                        break;
                    };
                    let record = prompt.record().clone();
                    if record.status != meerkat_core::ApprovalStatus::Pending {
                        return Err("software review is not a fresh native decision".into());
                    }
                    prompt
                        .decide(meerkat_core::ApprovalDecision::Approve)
                        .map_err(|error| error.to_string())?;
                    approved.lock().unwrap().push(record);
                    // The exact presenter and its decided prompts stay alive
                    // through the entered calls. Dropping them immediately
                    // after decide would retire their owner.
                    decided.push(prompt);
                }
                if !*close.borrow() {
                    let _ = close.changed().await;
                }
                drop(decided);
                drop(presenter);
                Ok(())
            }));
        }
    }

    /// Whether helper `tag`'s current install call (its first, or its one
    /// fresh install after a certified refusal, in its latest run) was
    /// approved.
    fn install_approved(&self, tag: &str) -> bool {
        let prefix = self.script.current_prefix(&format!("{tag}-helper"));
        let call = format!("{prefix}-{}", self.script.retry_suffix(&prefix, "install"));
        self.approved.lock().unwrap().iter().any(|record| {
            record
                .proposed_action
                .body
                .as_ref()
                .and_then(|body| body["call_id"].as_str())
                == Some(call.as_str())
        })
    }

    async fn preparation(&self) -> Option<Value> {
        use std::os::unix::fs::MetadataExt;
        // Backend evidence only. The installed host remains the sole Toolkit
        // store/publication owner. This read-only connection cannot issue a
        // transition, admit a member, or stand in for consumer API coverage.
        let realm = meerkat_core::mob_realm_id(MOB).unwrap().to_string();
        let path = self
            .directory
            .path()
            .join("shared/data/toolkit-native")
            .join(digest(realm.as_bytes()))
            .join("originals.sqlite3")
            .canonicalize()
            .unwrap();
        let held = fs::File::open(&path).unwrap();
        let before = held.metadata().unwrap();
        assert!(before.is_file());
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        connection.busy_timeout(Duration::from_secs(1)).unwrap();
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
        let rows = connection
            .prepare(
                "SELECT data FROM toolkit_records WHERE realm=?1 AND owner=?2 ORDER BY id LIMIT 2",
            )
            .unwrap()
            .query_map(
                rusqlite::params![
                    realm,
                    "toolkit/acquisition/sdk-shared-host/shared-preparation"
                ],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            (before.dev(), before.ino()),
            (after.dev(), after.ino()),
            "the evidence read must retain the exact installed database inode"
        );
        assert!(rows.len() <= 1, "one exact selected physical preparation");
        rows.into_iter().next().map(|raw| {
            let data: toolkit_store::RecordData = serde_json::from_str(&raw).unwrap();
            assert_eq!(data.schema, "meerkat_toolkit.shared_preparation");
            assert_eq!(data.version, 1);
            data.body
        })
    }

    fn worker(&self) -> Option<Value> {
        let path = self.directory.path().join("physical/WORKER-ENTERED.json");
        path.exists()
            .then(|| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
    }

    async fn run(&mut self) {
        self.script
            .find_deadline
            .set(tokio::time::Instant::now() + BOUND)
            .unwrap();
        self.approve_independently();
        // Publish this in-process runtime's actual bootstrap completion, as
        // production does once its runtime completes. That starts this host's
        // acquisition recovery owner, whose sweep records the settled shared
        // preparation and delivers the current member's continuation.
        self.host
            .as_ref()
            .unwrap()
            .publish_runtime_bootstrap_completed(self.native.as_ref().unwrap())
            .unwrap();
        let launch = self.launch.as_ref().unwrap().clone();
        launch.create_parent().await;
        let event = |tag: &str| {
            let content = format!(
                "sdk-child-{tag}: inspect and prepare the selected inert software; report activation honestly"
            );
            let authentication = PrivateSourceEvidence {
                resource_id: evidence(&format!("sdk-auth-{tag}")),
                revision: evidence("1"),
                sha256: digest(format!("authenticated SDK occurrence {tag}").as_bytes()),
            };
            (
                PrivateAuthenticatedEvent {
                    route: id(&format!("original-{tag}")),
                    original: evidence(&format!("sdk-child-{tag}")),
                    occurrence: evidence(&format!("sdk-occurrence-{tag}")),
                    original_authentication: authentication,
                    root_event: PrivateSourceEvidence {
                        resource_id: evidence(&format!("sdk-root-{tag}")),
                        revision: evidence("1"),
                        sha256: digest(content.as_bytes()),
                    },
                    contributors: vec![],
                    content_sha256: digest(content.as_bytes()),
                },
                content,
            )
        };
        let (a, content_a) = event("a");
        let (b, content_b) = event("b");
        let receive_a = launch.launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            a,
            content_a,
        );
        let receive_b = launch.launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-b").unwrap(),
            b,
            content_b,
        );
        let (a, b) = tokio::time::timeout(BOUND, async { tokio::join!(receive_a, receive_b) })
            .await
            .unwrap();
        let a = a.unwrap().unwrap();
        let b = b.unwrap().unwrap();
        assert_ne!(a.route, b.route);
        assert_ne!(a.original, b.original);
        let entered = tokio::time::timeout(BOUND, async {
            loop {
                if let (Some(row), Some(worker)) = (self.preparation().await, self.worker())
                    && row["members"].as_object().unwrap().len() == 2
                    && ["a", "b"].into_iter().all(|tag| self.install_approved(tag))
                {
                    assert_eq!(worker["entries"].as_array().unwrap().len(), 1);
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("two ordinary helpers must join while one physical worker is held");
        if let Some(gate) = self.script.cancel_gate.get() {
            tokio::time::timeout(BOUND, async {
                while !(gate.open)() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("the control's cancel gate was never met: {}", gate.name));
            gate.opened.store(true, Ordering::SeqCst);
        }
        assert_eq!(entered["closed"], false);
        assert!(entered["completion"].is_null());
        let observations = launch.observations();
        assert_eq!(observations.len(), 2);
        let child = |tag: &str| {
            observations
                .iter()
                .find(|row| row["original"]["route"] == format!("original-{tag}"))
                .unwrap()
        };
        let row_for = |row: &Value, tag: &str| {
            let selected = row["members"]
                .as_object()
                .unwrap()
                .values()
                .filter(|value| value["work"] == child(tag)["work"])
                .cloned()
                .collect::<Vec<_>>();
            assert_eq!(selected.len(), 1);
            selected[0].clone()
        };
        for tag in ["a", "b"] {
            let row = row_for(&entered, tag);
            assert_eq!(row["helper"], json!(member(&format!("helper-{tag}"))));
            assert_eq!(row["left"], false);
            assert_eq!(row["leaving"], false);
            assert!(row["delivered"].is_null());
            assert!(child(tag)["input"].is_string());
            assert!(child(tag)["session"].is_string());
        }
        assert_eq!(child("a")["work"]["parent"], child("b")["work"]["parent"]);
        assert_ne!(child("a")["work"]["child"], child("b")["work"]["child"]);
        let approved = self.approved.lock().unwrap().clone();
        // One approval per install call that reached its review: each
        // helper's install that went through, and a first install refused
        // certified not dispatched after its review, whose one fresh install
        // has its own.
        let approval_ids = approved
            .iter()
            .map(|record| &record.approval_id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(approval_ids.len(), approved.len());
        assert_ne!(
            self.audit_sessions["helper-a"],
            self.audit_sessions["helper-b"]
        );
        let emitted = self.script.calls.lock().unwrap().clone();
        let returned = self.script.results.lock().unwrap().clone();
        let installed = |tag: &str| {
            let prefix = self.script.current_prefix(&format!("{tag}-helper"));
            format!("{prefix}-{}", self.script.retry_suffix(&prefix, "install"))
        };
        let reviewed = approved
            .iter()
            .map(|record| {
                let meerkat_core::ApprovalOwnerRef::Session { session_id } = &record.owner
                else {
                    panic!("each native approval must retain its exact helper session owner");
                };
                let tag = ["a", "b"].into_iter().find(|tag|
                    self.audit_sessions.get(&format!("helper-{tag}")) == Some(session_id))
                    .expect("approval owner must be an actual captured helper session");
                assert_eq!(record.resource.kind, meerkat_core::ApprovalResourceKind::ToolCall);
                assert_eq!(record.resource.id.as_str(), "setup_selected_install");
                assert_eq!(record.proposed_action.kind, meerkat_core::ApprovalActionKind::ToolCall);
                assert_eq!(record.proposed_action.summary,
                    "Ask the selected package owner to install this exact checked software configuration once. The returned job and receipt are software-only observations; this does not authenticate an account, attach usable tools, grant access or complete the original task.");
                let call_id = record
                    .proposed_action
                    .body
                    .as_ref()
                    .and_then(|body| body["call_id"].as_str())
                    .expect("each approval names its install call")
                    .to_owned();
                // The run of this install call: each run of the helper's
                // input emits its own calls, and its install is the run's
                // first install or its one fresh install after that first
                // was refused.
                let run = self
                    .script
                    .run_prefixes(&format!("{tag}-helper"))
                    .into_iter()
                    .find(|prefix| {
                        call_id == format!("{prefix}-install")
                            || call_id
                                == format!("{prefix}-{}", self.script.retry_suffix(prefix, "install"))
                    })
                    .unwrap_or_else(|| {
                        panic!("an approval reviews the helper's install or its refused first attempt: {call_id}")
                    });
                let matching = emitted.iter().filter(|(id, _, _)| id == &call_id).collect::<Vec<_>>();
                assert_eq!(matching.len(), 1, "each approval must match one emitted call");
                assert_eq!(matching[0].1, "setup_selected_install");
                let check_id = format!("{run}-{}", self.script.check_suffix(&run));
                let checks = emitted.iter().filter(|(id, _, _)| id == &check_id).collect::<Vec<_>>();
                assert_eq!(checks.len(), 1, "each proposal must retain its actual check call");
                assert_eq!(checks[0].1, "setup_selected_check");
                let checked = returned.get(&check_id).expect("successful owner check must be retained");
                assert_eq!(matching[0].2, json!({
                    "validation_id":checked["validation_id"], "effect_sha256":checked["effect_sha256"]}));
                // The approval reviews the owner's checked software proposal,
                // not merely the validation key used by the install call.
                let proposal = json!({"kind":"service_software_install", "check_tool":checks[0].1,
                    "selection":checks[0].2, "owner_proposal":checked, "scope":"software_installation_only"});
                let mut expected = proposal.clone();
                expected["tool"] = matching[0].1.clone().into();
                expected["call_id"] = call_id.clone().into();
                assert_eq!(record.proposed_action.body.as_ref(), Some(&expected));
                (call_id, proposal)
            })
            .collect::<Vec<_>>();
        let call_ids = reviewed
            .iter()
            .map(|(call_id, _)| call_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            call_ids.len(),
            approved.len(),
            "one approval per install call"
        );
        for tag in ["a", "b"] {
            assert!(
                call_ids.contains(installed(tag).as_str()),
                "helper {tag}'s install was reviewed"
            );
        }
        assert!(
            reviewed.windows(2).all(|pair| pair[0].1 == pair[1].1),
            "each exact native call independently reviews the same checked software proposal"
        );
        // The worker is released only after A's cancellation is acknowledged,
        // so an install of A that joined it is still pending here, and a
        // cancelled member need not see its result. The check proof holds
        // such a call to its safety proofs instead. The flag and the record
        // are taken together under the cancel section, which A's model holds
        // across each whole response: a response before them has emitted
        // its calls, and one after them answers an error result as cancelled
        // and emits nothing.
        {
            let _cancel_section = self.script.cancel_section.lock().unwrap();
            self.script.cancel_requested.store(true, Ordering::SeqCst);
            self.script.record_pending_at_cancel("a-helper");
        }
        let cancelled = self
            .client
            .lock()
            .await
            .call(json!({"kind":"cancel_original_operation","original":a}))
            .await
            .unwrap();
        assert_eq!(cancelled["kind"], "original_operation_cancelled");
        assert_eq!(cancelled["state"], "operations_cancelled");
        assert!(
            !self
                .directory
                .path()
                .join("physical/RELEASE-WORKER")
                .exists(),
            "A acknowledgement must precede B worker release"
        );
        let left = self.preparation().await.unwrap();
        assert_eq!(row_for(&left, "a")["left"], true);
        assert_eq!(row_for(&left, "b")["left"], false);
        assert_eq!(left["closed"], false);
        assert!(left["completion"].is_null());
        assert_eq!(
            self.worker().unwrap()["entries"].as_array().unwrap().len(),
            1
        );
        fs::write(
            self.directory.path().join("physical/RELEASE-WORKER"),
            b"B remains current after A cancellation acknowledgement\n",
        )
        .unwrap();
        let mut last_readback = None;
        let completed = tokio::time::timeout(BOUND, async {
            loop {
                // Drive that recovery owner within BOUND: end its pause, so
                // its next sweep runs now rather than after its backoff, which
                // grows to a minute while work is pending. The sweep's own
                // reads decide what it records; the wake seeds no fact.
                self.host
                    .as_ref()
                    .unwrap()
                    .wake_acquisition_recovery_for_test()
                    .unwrap();
                let row = self.preparation().await.unwrap();
                let completion_present = !row["completion"].is_null();
                let b_delivered_present = !row_for(&row, "b")["delivered"].is_null();
                let finished = self.script.finished.load(Ordering::SeqCst);
                last_readback = Some((completion_present, b_delivered_present, finished));
                if completion_present && b_delivered_present && finished > 0 {
                    break row;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        if completed.is_err() {
            // Preserve the last real canonical read; do not replace the
            // original timeout with an additional read or inferred result.
            let observed = last_readback.map(|(completion, delivered, finished)| {
                json!({
                    "completion_present":completion, "b_delivered_present":delivered,
                    "finished":finished,
                })
            });
            eprintln!(
                "{}",
                json!({
                    "event":"sdk_shared_completion_timeout", "last_readback":observed,
                    "finished_at_timeout":self.script.finished.load(Ordering::SeqCst),
                })
            );
        }
        let completed =
            completed.expect("current B readback and honest native continuation must settle");
        assert_eq!(row_for(&completed, "a")["left"], true);
        assert!(row_for(&completed, "a")["delivered"].is_null());
        let current = row_for(&completed, "b");
        assert_eq!(current["left"], false);
        let delivered = &current["delivered"]["evidence"];
        assert_eq!(delivered["session"], child("b")["session"]);
        assert_ne!(
            delivered["durable_input"]["id"],
            child("b")["input"],
            "continuation has its own native input"
        );
        assert_eq!(completed["completion"]["job"]["status"], "succeeded");
        // The script records a helper's tool result when that helper's next
        // model request carries it, which can come after the continuation
        // the result produced reached B's original. Wait for B's job and
        // report results within the bound; the assertions below still
        // decide, so a result that never arrives still fails here. The calls
        // are those of B's helper's latest run.
        let helper = self.script.current_prefix("b-helper");
        let job = format!("{helper}-{}", self.script.retry_suffix(&helper, "job"));
        let report = format!("{helper}-{}", self.script.retry_suffix(&helper, "report"));
        let _ = tokio::time::timeout(BOUND, async {
            loop {
                {
                    let results = self.script.results.lock().unwrap();
                    if results.contains_key(&job) && results.contains_key(&report) {
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        {
            let results = self.script.results.lock().unwrap();
            assert!(results.contains_key(&job), "{job}");
            assert!(results.contains_key(&report), "{report}");
        }
        self.assert_find_feedback(&["a", "b"], true).await;
        assert_eq!(
            self.worker().unwrap()["entries"].as_array().unwrap().len(),
            1
        );
    }

    async fn cleanup(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        let physical_started = self.resource.is_some();
        // Release before any native shutdown can join an entered installer.
        // This branch is failure cleanup, never cancellation acceptance evidence.
        let release = self.directory.path().join("physical/RELEASE-WORKER");
        if release.parent().is_some_and(Path::is_dir)
            && !release.exists()
            && let Err(error) = fs::write(&release, b"failure cleanup release\n")
        {
            failures.push(error.to_string());
        }
        self.review_close.send_replace(true);
        self.reviews.clear();
        for task in std::mem::take(&mut self.review_tasks) {
            match task.await {
                Ok(Ok(())) => (),
                Ok(Err(error)) => failures.push(error),
                Err(error) => failures.push(error.to_string()),
            }
        }
        if let Some(launch) = self.launch.take() {
            for result in launch.join().await {
                if let Err(error) = result {
                    failures.push(error);
                }
            }
        }
        // Accepted private commands are drained by the existing connection
        // JoinSet. All SDK launchers are joined before this stream closes.
        if let Err(error) = self.client.lock().await.close().await {
            failures.push(error);
        }
        if let Some(private) = self.private.take()
            && let Err(error) = private.finish().await
        {
            failures.push(error.to_string());
        }
        let lifetime = self.lifetime.lock().unwrap().take();
        if let Some(owner) = lifetime {
            use meerkat_toolkit::host_lifetime::control::{
                HostCloseState, PrivateHostLifetimeControl,
            };
            match owner.host_lost() {
                Ok(close) => match owner.join_close(&close.handle).await {
                    Ok(value) if value.state == HostCloseState::Settled => {}
                    Ok(_) => failures.push("private lifetime close remains unresolved".into()),
                    Err(error) => failures.push(error.to_string()),
                },
                Err(error) => failures.push(error.to_string()),
            }
        }
        if let Some(native) = self.native.take() {
            let report = native.shutdown().await;
            let terminal = match &report.mob_terminal_shutdown {
                meerkat_mobkit::MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
                meerkat_mobkit::MobTerminalShutdownOutcome::AlreadyShutDown => true,
                _ => false,
            };
            if !report.cleanup_completed() || !terminal {
                failures.push(format!("native shutdown unresolved: {report:?}"));
            }
        }
        self.host.take();
        if let Some(owner) = self.resource.take() {
            if owner.attempts.get().is_some() {
                match owner
                    .join_attempt_guards(tokio::time::Instant::now() + Duration::from_secs(20))
                    .await
                {
                    Ok(report) if report.is_complete() => (),
                    value => failures.push(format!("bounded attempt join unresolved: {value:?}")),
                }
            }
            // The bounded observation above is independently reportable.
            // close retains unsaved or pending custody and is not itself a
            // bounded settlement guarantee when evidence storage keeps failing.
            if let Err(error) = owner.close().await {
                failures.push(error.to_string());
            }
        }
        let joined = self.directory.path().join("physical/OWNER-JOINED.json");
        if physical_started {
            match fs::read(joined)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            {
                Some(value) if value["workers_alive"] == 0 => (),
                _ => failures.push("physical owner lacks joined worker observation".into()),
            }
        }
        failures
    }

    /// Cleanup once the host published its runtime's bootstrap completion:
    /// its acquisition recovery owner then has a real retained worker. Close
    /// the actual host first, keep it across native shutdown, and join it
    /// before dropping it, after a panic as well.
    async fn cleanup_completed_runtime(&mut self) -> Vec<String> {
        use meerkat_mobkit::rpc_gateway::{GatewayHostCloseReason, GatewayHostComposition};
        let host = self.host.take();
        let mut failures = Vec::new();
        if let Some(host) = &host
            && let Err(error) = host.begin_close(GatewayHostCloseReason::Shutdown)
        {
            failures.push(format!("host begin close: {error:?}"));
        }
        let cleanup = std::panic::AssertUnwindSafe(self.cleanup())
            .catch_unwind()
            .await;
        if let Some(host) = &host
            && let Err(error) = host.join_close().await
        {
            failures.push(format!("host join close: {error:?}"));
        }
        match cleanup {
            Ok(mut cleanup) => {
                cleanup.extend(failures);
                cleanup
            }
            Err(payload) => {
                eprintln!("host cleanup error count after unwind: {}", failures.len());
                std::panic::resume_unwind(payload)
            }
        }
    }
}

#[test]
fn two_sdk_children_share_one_preparation_and_cancel_independently() {
    let (pipes, parent) = SdkParent::pair();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async move {
        let mut case = Case::new(parent.enter());
        let result = std::panic::AssertUnwindSafe(async {
            case.construct(pipes).await;
            case.run().await;
        })
        .catch_unwind()
        .await;
        if result.is_err() {
            case.retain_failed_audits().await;
        }
        let cleanup = case.cleanup_completed_runtime().await;
        assert!(
            cleanup.is_empty(),
            "all SDK/native/physical owners must settle: {cleanup:?}"
        );
        if let Err(payload) = result {
            std::panic::resume_unwind(payload);
        }
        // Native shutdown above commits the completed helper inputs. No model
        // store read, polling or extra execution is needed to qualify feedback.
        case.assert_check_feedback().await;
    });
}
