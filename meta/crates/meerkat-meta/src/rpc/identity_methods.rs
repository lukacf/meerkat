//! `meta/identity/*`: the changes and reads of a mob's long-term agents.
//!
//! One family, served by the same handlers on the stdio gateway (to its SDK
//! host, the operator) and on the Console RPC (to its caller, under the
//! access actions). A handler owns no meaning: it attributes the requester
//! from its transport, names the operation
//! ([`OperationId::for_request`]), reads the request as every surface
//! writes it (`identity_creation::family`) and calls the family
//! ([`IdentityFamily`]), which applies the host's rules and commits through
//! the executor seam.
//!
//! Writes: `create`, `update`, `archive`, `unarchive`, `remove`, `adopt`,
//! `withdraw`, `changes_hold`. Reads: `list`, `get`, `operation`,
//! `directory`, `plan`. A read never wakes an agent.
//!
//! A change names its operation with `request_id`, which a host method
//! requires: the same requester and `request_id` with the same request
//! answer the same state again and write nothing, and another request under
//! them is refused `operation_reused`. The id is namespaced by the
//! requester, so two requesters that choose the same `request_id` name two
//! operations.
//!
//! A committed or pending change answers its typed outcome
//! ([`IdentityOutcome::to_json`]: `done`, `pending_decision`,
//! `pending_preparation`). Every refusal, of a write or of a read, is one
//! JSON-RPC error: code [`IDENTITY_REFUSED_CODE`], the refusal's sentence
//! as its message, and the refusal as its `data`
//! ([`IdentityRefusal::to_refused`]): `kind` from the closed vocabulary,
//! `field` (a path into the request, when the refusal concerns one field),
//! `retryable`, `detail`, and the kind's own members. Params that are not
//! what a method takes are that error too, of kind `invalid_request`.
//!
//! `meta/delete_identity` and `meta/identity/remove` differ: the first gives
//! an agent a fresh life and keeps it (its name, its record and its place
//! in the mob stay); the second retires the name for good.

use meerkat_mob::MobHandle;
use serde_json::{Map, Value, json};

use super::{JSONRPC_VERSION, JsonRpcError, JsonRpcResponse};
use crate::access::AccessView;
use crate::identity_creation::{
    AgentRef, ChangeRequester, ChangesHoldRequest, CreateAgents, GetAgent, IdentityChangeCall,
    IdentityChangeSpec, IdentityFamily, IdentityOutcome, IdentityRead, IdentityReadAnswer,
    IdentityRefusal, IdentityRefusalKind, ListAgents, OperationId, OperationRef, OperationState,
    ReadDirectory, RequestProblem, UpdateAgent, WithdrawRequest,
};

/// Release an agent from configuration: from the adoption on it is changed
/// at run time and configuration no longer declares it.
pub(crate) const IDENTITY_ADOPT: &str = "meta/identity/adopt";
/// One agent, in full.
pub(crate) const IDENTITY_GET: &str = "meta/identity/get";
/// A page of the mob's agents.
pub(crate) const IDENTITY_LIST: &str = "meta/identity/list";
/// The state of one operation.
pub(crate) const IDENTITY_OPERATION: &str = "meta/identity/operation";
/// Create 1 to 256 agents, all or nothing.
pub(crate) const IDENTITY_CREATE: &str = "meta/identity/create";
/// Update one agent.
pub(crate) const IDENTITY_UPDATE: &str = "meta/identity/update";
/// Archive one agent.
pub(crate) const IDENTITY_ARCHIVE: &str = "meta/identity/archive";
/// Unarchive one archived agent.
pub(crate) const IDENTITY_UNARCHIVE: &str = "meta/identity/unarchive";
/// Remove one agent for good.
pub(crate) const IDENTITY_REMOVE: &str = "meta/identity/remove";
/// Withdraw a change whose decision is still held.
pub(crate) const IDENTITY_WITHDRAW: &str = "meta/identity/withdraw";
/// Hold or release the mob's changes of agents.
pub(crate) const IDENTITY_CHANGES_HOLD: &str = "meta/identity/changes_hold";
/// What the caller may name in a create or an update.
pub(crate) const IDENTITY_DIRECTORY: &str = "meta/identity/directory";
/// The outcome and the rendering a change would get; nothing is held.
pub(crate) const IDENTITY_PLAN: &str = "meta/identity/plan";

/// The family, one registry every surface derives its dispatch arm, its
/// read/write split and its advertised methods from.
pub(crate) const IDENTITY_FAMILY_METHODS: &[&str] = &[
    IDENTITY_ADOPT,
    IDENTITY_GET,
    IDENTITY_LIST,
    IDENTITY_OPERATION,
    IDENTITY_CREATE,
    IDENTITY_UPDATE,
    IDENTITY_ARCHIVE,
    IDENTITY_UNARCHIVE,
    IDENTITY_REMOVE,
    IDENTITY_WITHDRAW,
    IDENTITY_CHANGES_HOLD,
    IDENTITY_DIRECTORY,
    IDENTITY_PLAN,
];

/// The JSON-RPC code of every refusal of the family. Its `data` is the
/// refusal as every surface writes it: `kind` (the closed vocabulary),
/// `field` when the refusal concerns one field of the request, `retryable`,
/// `detail`, and the kind's own members.
pub const IDENTITY_REFUSED_CODE: i64 = -32058;

/// The host surface of the stdio gateway's caller: its SDK host, the
/// operator. A rule selects it with `requester: {"kind": "host", "surface":
/// "rpc"}`.
pub const HOST_RPC_SURFACE: &str = "rpc";

/// The member of a plan's params that names the change it plans: `create`,
/// `update`, `archive`, `unarchive`, `remove` or `adopt`. The other members
/// are that change's own params.
const PLAN_CHANGE_MEMBER: &str = "change";

/// The `outcome` of an operation no change committed under and no decision
/// is held for.
const OPERATION_UNKNOWN: &str = "unknown";

/// Whether `method` belongs to the family.
#[must_use]
pub(crate) fn is_identity_family_method(method: &str) -> bool {
    IDENTITY_FAMILY_METHODS.contains(&method)
}

/// Whether `method` changes something: a read-only Console and a candidate
/// launch refuse it.
#[must_use]
pub(crate) fn is_identity_family_mutating_method(method: &str) -> bool {
    matches!(
        method,
        IDENTITY_ADOPT
            | IDENTITY_CREATE
            | IDENTITY_UPDATE
            | IDENTITY_ARCHIVE
            | IDENTITY_UNARCHIVE
            | IDENTITY_REMOVE
            | IDENTITY_WITHDRAW
            | IDENTITY_CHANGES_HOLD
    )
}

/// The requester of the stdio gateway's caller: the host's operator, on
/// the surface [`HOST_RPC_SURFACE`].
#[must_use]
pub(crate) fn stdio_requester() -> ChangeRequester {
    ChangeRequester::host_operator(HOST_RPC_SURFACE)
}

/// The requester of a Console caller: the verified person, by the key a
/// decision records that person under, or the host on its console surface
/// when the caller is no verified person. Never the host's operator.
#[must_use]
pub(crate) fn console_requester(person: Option<&str>) -> ChangeRequester {
    match person.map(str::trim).filter(|person| !person.is_empty()) {
        Some(person) => ChangeRequester::human(person),
        None => ChangeRequester::host(crate::identity_creation::CONSOLE_SURFACE),
    }
}

/// The agents a Console caller may view, as the family asks which agents a
/// caller may see: a peer the caller may not view is answered as a name no
/// agent holds.
pub(crate) struct ViewedAgents(pub(crate) AccessView);

impl crate::identity_creation::PeerVisibility for ViewedAgents {
    fn sees(&self, name: &str) -> bool {
        self.0.can_view_agent(name)
    }

    fn sees_every_agent(&self) -> bool {
        self.0.allows(crate::access::ACTION_RUNTIME_ADMIN)
    }
}

/// What the surface a call arrived on hands the family.
pub(crate) struct IdentityFamilySurface<'a> {
    /// The mob the call acts in.
    pub(crate) mob: &'a MobHandle,
    /// The family: the host's rules over the executor seam.
    pub(crate) family: &'a dyn IdentityFamily,
    /// Who asks, as this transport attributed it.
    pub(crate) requester: ChangeRequester,
    /// The Console caller's access view: a list leaves out the agents the
    /// caller may not view. `None` on the stdio gateway.
    pub(crate) access: Option<&'a AccessView>,
}

/// Serve `method` of the family: `None` for a method outside it.
pub(crate) async fn handle_identity_family_rpc(
    surface: &IdentityFamilySurface<'_>,
    method: &str,
    response_id: Value,
    params: &Value,
) -> Option<JsonRpcResponse> {
    let answer = match method {
        IDENTITY_ADOPT => {
            change_of(surface, params, |agent: AgentRef| {
                IdentityChangeSpec::Adopt(agent)
            })
            .await
        }
        IDENTITY_GET => get(surface, params).await,
        IDENTITY_LIST => list(surface, params).await,
        IDENTITY_OPERATION => operation(surface, params).await,
        IDENTITY_CREATE => change_of(surface, params, IdentityChangeSpec::Create).await,
        IDENTITY_UPDATE => change_of(surface, params, IdentityChangeSpec::Update).await,
        IDENTITY_ARCHIVE => {
            change_of(surface, params, |agent: AgentRef| {
                IdentityChangeSpec::Archive(agent)
            })
            .await
        }
        IDENTITY_UNARCHIVE => {
            change_of(surface, params, |agent: AgentRef| {
                IdentityChangeSpec::Unarchive(agent)
            })
            .await
        }
        IDENTITY_REMOVE => {
            change_of(surface, params, |agent: AgentRef| {
                IdentityChangeSpec::Remove(agent)
            })
            .await
        }
        IDENTITY_WITHDRAW => withdraw(surface, params).await,
        IDENTITY_CHANGES_HOLD => changes_hold(surface, params).await,
        IDENTITY_DIRECTORY => directory(surface, params).await,
        IDENTITY_PLAN => plan(surface, params).await,
        _ => return None,
    };
    let (result, error) = match answer {
        Ok(result) => (Some(result), None),
        Err(refusal) => (None, Some(refused(&refusal))),
    };
    Some(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: response_id,
        result,
        error,
    })
}

/// The one error of the family: [`IDENTITY_REFUSED_CODE`], the refusal's
/// sentence as the message, and the refusal as `data`.
#[must_use]
pub(crate) fn refused(refusal: &IdentityRefusal) -> JsonRpcError {
    JsonRpcError {
        code: IDENTITY_REFUSED_CODE,
        message: refusal.detail(),
        data: Some(refusal.to_refused()),
    }
}

/// `params` as a `T`. Params that are not what the method takes (a missing
/// or unknown member, a value of another type) are refused
/// `invalid_request`, with the parser's words as the detail.
fn parse<T: serde::de::DeserializeOwned>(params: &Value) -> Result<T, IdentityRefusal> {
    let params = if params.is_null() {
        Value::Object(Map::new())
    } else {
        params.clone()
    };
    serde_json::from_value(params).map_err(|error| {
        IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Malformed,
        })
        .with_detail(format!("The request is malformed: {error}."))
    })
}

/// The change `change` as the requester's operation. A host method's
/// change names its operation: one without a `request_id` is refused.
fn change_call(
    surface: &IdentityFamilySurface<'_>,
    change: IdentityChangeSpec,
) -> Result<IdentityChangeCall, IdentityRefusal> {
    let Some(request_id) = change.request_id() else {
        return Err(IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Empty,
        })
        .at("request_id")
        .with_detail(
            "A change names its operation: send a request_id, and the same one again to read \
             the change's result.",
        ));
    };
    let operation_id = OperationId::for_request(&surface.requester.recorded(), request_id)?;
    Ok(IdentityChangeCall::new(operation_id, change))
}

/// The answer of a change: its typed outcome, or its refusal.
fn outcome_answer(outcome: IdentityOutcome) -> Result<Value, IdentityRefusal> {
    match outcome {
        IdentityOutcome::Refused(refusal) => Err(refusal),
        outcome => Ok(outcome.to_json()),
    }
}

/// One change: `params` read as the change's request `T`, asked of the
/// family as the requester's operation.
async fn change_of<T: serde::de::DeserializeOwned>(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
    spec: impl FnOnce(T) -> IdentityChangeSpec,
) -> Result<Value, IdentityRefusal> {
    let call = change_call(surface, spec(parse(params)?))?;
    outcome_answer(
        surface
            .family
            .change(surface.mob, &surface.requester, call)
            .await,
    )
}

fn serialized<T: serde::Serialize>(value: &T) -> Result<Value, IdentityRefusal> {
    serde_json::to_value(value).map_err(|error| {
        IdentityRefusal::new(IdentityRefusalKind::Unsupported {
            feature: format!("an answer this build cannot write: {error}"),
        })
    })
}

/// The operation `params` name, for the caller: its own by `request_id`, or
/// the one an outcome named by `operation_id`.
fn named_operation(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<OperationId, IdentityRefusal> {
    parse::<OperationRef>(params)?.resolve(&surface.requester.recorded())
}

impl IdentityFamilySurface<'_> {
    /// Whether the caller reads the operation `operation_id`: its own
    /// always, and another requester's on the stdio gateway or as a Console
    /// caller that administers the runtime. To every other caller another
    /// requester's operation reads as one nobody asked.
    fn reads_operation(&self, operation_id: &OperationId) -> bool {
        operation_id.is_of(&self.requester.recorded())
            || self
                .access
                .is_none_or(|view| view.allows(crate::access::ACTION_RUNTIME_ADMIN))
    }
}

async fn withdraw(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    // A held change is withdrawn by the verified requester that asked for
    // it, or by the host's operator. A Console caller that is no verified
    // person is neither, and withdraws nothing.
    if matches!(
        &surface.requester,
        ChangeRequester::Host {
            operator: false,
            ..
        }
    ) {
        return Err(
            IdentityRefusal::new(IdentityRefusalKind::DecisionNotFound).with_detail(
                "A held change is withdrawn by the signed-in person that asked for it, or by                  the host: this caller is neither, and nothing was withdrawn.",
            ),
        );
    }
    let operation_id = named_operation(surface, params)?;
    let withdrawn = surface
        .family
        .withdraw(
            surface.mob,
            &surface.requester,
            WithdrawRequest::new(operation_id),
        )
        .await?;
    serialized(&withdrawn)
}

async fn changes_hold(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let request: ChangesHoldRequest = parse(params)?;
    let hold = surface
        .family
        .changes_hold(surface.mob, &surface.requester, request)
        .await?;
    serialized(&hold)
}

async fn read(
    surface: &IdentityFamilySurface<'_>,
    read: IdentityRead,
) -> Result<IdentityReadAnswer, IdentityRefusal> {
    surface
        .family
        .read(surface.mob, &surface.requester, read)
        .await
}

async fn get(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let params: GetAgent = parse(params)?;
    match read(surface, IdentityRead::Get(params)).await? {
        IdentityReadAnswer::Get(record) => serialized(&record),
        _ => Err(unexpected_answer()),
    }
}

async fn list(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let params: ListAgents = parse(params)?;
    match read(surface, IdentityRead::List(params)).await? {
        IdentityReadAnswer::List(mut page) => {
            // A Console caller is listed the agents it may view.
            if let Some(view) = surface.access {
                page.agents.retain(|agent| {
                    view.allows_agent(crate::access::ACTION_AGENT_VIEW, &agent.name)
                });
            }
            serialized(&page)
        }
        _ => Err(unexpected_answer()),
    }
}

async fn operation(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let operation_id = named_operation(surface, params)?;
    if !surface.reads_operation(&operation_id) {
        return Ok(operation_answer(&operation_id, &OperationState::Unknown));
    }
    match read(surface, IdentityRead::Operation(operation_id.clone())).await? {
        IdentityReadAnswer::Operation(state) => Ok(operation_answer(&operation_id, &state)),
        _ => Err(unexpected_answer()),
    }
}

/// The answer of `meta/identity/operation`: the operation's typed outcome,
/// as a change answers it (a refused one included, as a result), or
/// `{"outcome": "unknown", "operation_id"}` for an operation no change
/// committed under and no decision is held for: asking it commits it at
/// most once.
fn operation_answer(operation_id: &OperationId, state: &OperationState) -> Value {
    match state {
        OperationState::Outcome(outcome) => outcome.to_json(),
        _ => json!({
            "outcome": OPERATION_UNKNOWN,
            "operation_id": operation_id.as_str(),
        }),
    }
}

async fn directory(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let params: ReadDirectory = parse(params)?;
    match read(surface, IdentityRead::Directory(params)).await? {
        IdentityReadAnswer::Directory(mut page) => {
            if let Some(view) = surface.access {
                page.peers
                    .retain(|peer| view.allows_agent(crate::access::ACTION_AGENT_VIEW, &peer.name));
            }
            serialized(&page)
        }
        _ => Err(unexpected_answer()),
    }
}

/// The change a plan's params name: `change` is its token, and the other
/// members are that change's own params.
fn planned_change(params: &Value) -> Result<IdentityChangeSpec, IdentityRefusal> {
    let unnamed = || {
        IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
            problem: RequestProblem::Malformed,
        })
        .at(PLAN_CHANGE_MEMBER)
        .with_detail(
            "Name the change to plan: change is create, update, archive, unarchive, remove or \
             adopt, beside that change's own params.",
        )
    };
    let mut members = params.as_object().cloned().ok_or_else(unnamed)?;
    let token = members.remove(PLAN_CHANGE_MEMBER).ok_or_else(unnamed)?;
    let body = Value::Object(members);
    Ok(match token.as_str().ok_or_else(unnamed)? {
        "create" => IdentityChangeSpec::Create(parse::<CreateAgents>(&body)?),
        "update" => IdentityChangeSpec::Update(parse::<UpdateAgent>(&body)?),
        "archive" => IdentityChangeSpec::Archive(parse::<AgentRef>(&body)?),
        "unarchive" => IdentityChangeSpec::Unarchive(parse::<AgentRef>(&body)?),
        "remove" => IdentityChangeSpec::Remove(parse::<AgentRef>(&body)?),
        "adopt" => IdentityChangeSpec::Adopt(parse::<AgentRef>(&body)?),
        _ => return Err(unnamed()),
    })
}

async fn plan(
    surface: &IdentityFamilySurface<'_>,
    params: &Value,
) -> Result<Value, IdentityRefusal> {
    let call = change_call(surface, planned_change(params)?)?;
    match read(surface, IdentityRead::Plan(Box::new(call))).await? {
        IdentityReadAnswer::Plan(plan) => serialized(&plan),
        _ => Err(unexpected_answer()),
    }
}

/// A family that answered a read with another read's answer.
fn unexpected_answer() -> IdentityRefusal {
    IdentityRefusal::new(IdentityRefusalKind::Unsupported {
        feature: "a read this build's family does not answer".to_owned(),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A Console caller may name as a peer exactly the agents its access
    /// rules let it view: every agent while access control is not
    /// enforced, and under enforced rules the agents a rule grants
    /// `agent.view` on.
    #[test]
    #[allow(clippy::expect_used)]
    fn a_console_caller_sees_the_agents_its_rules_let_it_view() {
        use crate::access::{ACTION_AGENT_VIEW, AccessControlConfig, AccessController, AccessRule};
        use crate::identity_creation::PeerVisibility;

        let open = AccessController::new(AccessControlConfig::default()).expect("controller");
        let everyone = ViewedAgents(open.view_for_subject(Some("person@example.test")));
        assert!(everyone.sees("scout") && everyone.sees("ledger"));

        let enforced = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            rules: vec![AccessRule {
                id: "views-scout".to_string(),
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                agents: vec!["scout".to_string()],
                ..AccessRule::default()
            }],
            ..AccessControlConfig::default()
        })
        .expect("controller");
        let person = ViewedAgents(enforced.view_for_subject(Some("person@example.test")));
        assert!(person.sees("scout"));
        assert!(!person.sees("ledger"), "no rule lets the caller view it");
        let admin = ViewedAgents(enforced.view_for_subject(Some("root@example.test")));
        assert!(admin.sees("ledger"), "an admin views every agent");
    }
    use crate::identity_creation::{AgentChangeState, AgentOutcome, Requester, Sha256Digest};

    const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000001";

    /// The 13 methods, in the order an application needs them: the four it
    /// moves its existing agents with first.
    #[test]
    fn the_family_registry_is_the_thirteen_methods_and_its_write_split() {
        assert_eq!(
            IDENTITY_FAMILY_METHODS,
            &[
                "meta/identity/adopt",
                "meta/identity/get",
                "meta/identity/list",
                "meta/identity/operation",
                "meta/identity/create",
                "meta/identity/update",
                "meta/identity/archive",
                "meta/identity/unarchive",
                "meta/identity/remove",
                "meta/identity/withdraw",
                "meta/identity/changes_hold",
                "meta/identity/directory",
                "meta/identity/plan",
            ]
        );
        let writers: Vec<&str> = IDENTITY_FAMILY_METHODS
            .iter()
            .copied()
            .filter(|method| is_identity_family_mutating_method(method))
            .collect();
        assert_eq!(
            writers,
            [
                "meta/identity/adopt",
                "meta/identity/create",
                "meta/identity/update",
                "meta/identity/archive",
                "meta/identity/unarchive",
                "meta/identity/remove",
                "meta/identity/withdraw",
                "meta/identity/changes_hold",
            ]
        );
        // The identity reads the family does not own stay outside it.
        for method in [
            "meta/identity/warm",
            "meta/identity/resolved_tools",
            "meta/identity/routing_status",
        ] {
            assert!(!is_identity_family_method(method), "{method}");
        }
    }

    /// A candidate launch refuses every writer of the family and serves
    /// every read.
    #[test]
    fn a_candidate_launch_refuses_the_writers_and_serves_the_reads() {
        for method in IDENTITY_FAMILY_METHODS {
            let refusal =
                crate::rpc::candidate_launch_refusal(false, None, method, &json!({}), json!(7));
            assert_eq!(
                refusal.is_some(),
                is_identity_family_mutating_method(method),
                "{method}"
            );
            if let Some(refusal) = refusal {
                let error = refusal.error.expect("a refusal is an error");
                assert_eq!(error.code, -32047, "{method}");
                assert_eq!(error.data, Some(json!({ "kind": "candidate_launch" })));
            }
            assert!(
                crate::rpc::candidate_launch_refusal(true, None, method, &json!({}), json!(7))
                    .is_none(),
                "an authoritative launch serves {method}"
            );
        }
    }

    /// The refusal envelope: one code, the refusal's sentence, and `data`
    /// with `kind`, `field`, `retryable`, `detail` and the kind's members.
    #[test]
    fn every_refusal_is_the_one_code_with_the_typed_data() {
        let refusal = IdentityRefusal::new(IdentityRefusalKind::StaleSpec {
            current_digest: Sha256Digest::parse(DIGEST).expect("a digest"),
            current_revision: Some(4),
        })
        .at("expected_spec_digest");
        let error = refused(&refusal);
        assert_eq!(error.code, IDENTITY_REFUSED_CODE);
        assert_eq!(error.code, -32058);
        assert_eq!(error.message, refusal.detail());
        assert_eq!(
            error.data,
            Some(json!({
                "kind": "stale_spec",
                "field": "expected_spec_digest",
                "retryable": false,
                "detail": refusal.detail(),
                "current_digest": DIGEST,
                "current_revision": 4,
            }))
        );
        let held = refused(&IdentityRefusal::new(IdentityRefusalKind::ChangesHeld));
        assert_eq!(
            held.data.expect("data")["retryable"],
            json!(true),
            "a retryable kind says so"
        );
    }

    /// Params that are not what a method takes are refused typed, before
    /// the family is asked: an unknown member, a missing one, and a plan
    /// that names no change.
    #[test]
    fn malformed_params_are_refused_invalid_request_before_the_family() {
        let agent: AgentRef = parse(&json!({
            "request_id": "r-1",
            "name": "scout",
            "expected_spec_digest": DIGEST,
        }))
        .expect("the change of one agent");
        assert_eq!(agent.name, "scout");
        let unknown = parse::<AgentRef>(&json!({
            "request_id": "r-1",
            "name": "scout",
            "expected_spec_digest": DIGEST,
            "identity": "scout",
        }))
        .expect_err("an unknown member");
        assert_eq!(unknown.kind_token(), "invalid_request");
        assert!(unknown.detail().contains("identity"), "{unknown}");
        let missing = parse::<AgentRef>(&json!({ "request_id": "r-1", "name": "scout" }))
            .expect_err("no digest");
        assert_eq!(missing.kind_token(), "invalid_request");
        assert!(
            missing.detail().contains("expected_spec_digest"),
            "{missing}"
        );
        let not_an_object = parse::<AgentRef>(&json!(["r-1"])).expect_err("params are an object");
        assert_eq!(not_an_object.kind_token(), "invalid_request");

        let planned = planned_change(&json!({
            "change": "remove",
            "request_id": "r-2",
            "name": "scout",
            "expected_spec_digest": DIGEST,
        }))
        .expect("a planned removal");
        assert_eq!(planned.token(), "remove");
        assert_eq!(planned.request_id(), Some("r-2"));
        for params in [
            json!({ "name": "scout", "expected_spec_digest": DIGEST }),
            json!({ "change": "reset", "name": "scout", "expected_spec_digest": DIGEST }),
            json!({ "change": 7 }),
        ] {
            let refusal = planned_change(&params).expect_err("no change named");
            assert_eq!(refusal.kind_token(), "invalid_request", "{params}");
            assert_eq!(refusal.field.as_deref(), Some("change"), "{params}");
        }
    }

    /// The operation a read or a withdrawal names: the caller's own by
    /// `request_id`, namespaced by the requester, or the one an outcome
    /// named; and what the operation read answers.
    #[test]
    fn an_operation_is_named_by_the_callers_request_id_or_by_an_outcomes_operation_id() {
        let stdio = stdio_requester();
        assert_eq!(
            stdio.recorded(),
            Requester::Host {
                surface: "rpc".to_owned()
            }
        );
        let own = OperationId::for_request(&stdio.recorded(), "r-1").expect("an id");
        assert!(own.as_str().starts_with("host."), "{own}");
        assert!(own.as_str().ends_with(".r-1"), "{own}");
        let person = console_requester(Some("person-1"));
        assert_eq!(
            person.recorded(),
            Requester::Human {
                principal: "person-1".to_owned()
            }
        );
        let theirs = OperationId::for_request(&person.recorded(), "r-1").expect("an id");
        assert_ne!(
            own, theirs,
            "one request id, two requesters, two operations"
        );
        assert_eq!(
            console_requester(None).recorded(),
            Requester::Host {
                surface: "console".to_owned()
            }
        );
        assert_eq!(
            console_requester(Some("  ")).recorded(),
            console_requester(None).recorded(),
            "a blank key is no person"
        );
        assert_eq!(
            operation_answer(&own, &OperationState::Unknown),
            json!({ "outcome": "unknown", "operation_id": own.as_str() })
        );
        let done = IdentityOutcome::Done {
            operation_id: own.clone(),
            agents: vec![AgentOutcome::new("scout", AgentChangeState::Committed)],
        };
        assert_eq!(
            operation_answer(&own, &OperationState::Outcome(done.clone())),
            done.to_json()
        );
        assert_eq!(done.to_json()["outcome"], json!("done"));
    }
}
