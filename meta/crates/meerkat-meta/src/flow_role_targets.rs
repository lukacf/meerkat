//! `role_targets` on `meta/run_flow`.
//!
//! A run started with `role_targets` dispatches each role of its flow only to
//! the members the caller bound to that role. Meerkat's mob actor admits the
//! binding and refuses it typed; this module reads the parameter, resolves
//! each member id to its roster row by the member-id alias rule
//! ([`crate::member_comms_id`]), and spells the echo and a refusal back in
//! public aliases. It also reads the optional `idempotency_key`, which
//! admits the run under a caller key, and starts the run through the one
//! Meerkat admission that fits. Both `meta/run_flow` implementations (the
//! stdio RPC server and the console) go through it, so they cannot disagree.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::member_comms_id::{
    durable_identity_from_runtime_alias, mob_member_id, roster_member_id_for_identity,
    roster_member_id_for_supplied_id, runtime_alias_str, validate_public_member_alias,
};

/// The `meta/capabilities` feature capability that says `meta/run_flow`
/// accepts `role_targets`. A server without it ignores the parameter and
/// runs the flow role-wide, so a client must not send `role_targets` to a
/// server that does not advertise it.
pub(crate) const FLOW_ROLE_TARGETS_FEATURE: &str = "flow_role_targets";

/// The `meta/run_flow` parameter, and the result field, that carry the
/// binding.
pub(crate) const ROLE_TARGETS_PARAM: &str = "role_targets";

/// The `meta/capabilities` feature capability that says `meta/run_flow`
/// accepts `idempotency_key`. A server without it ignores the key, so a
/// retry would admit a second run: a client must not send a key to a server
/// that does not advertise it.
pub(crate) const FLOW_RUN_IDEMPOTENCY_KEY_FEATURE: &str = "flow_run_idempotency_key";

/// The `meta/run_flow` parameter that carries the caller's idempotency key.
pub(crate) const IDEMPOTENCY_KEY_PARAM: &str = "idempotency_key";

/// The prefix of every `meta/run_flow` failure message.
const RUN_FLOW_FAILED: &str = "run_flow failed: ";

/// Why `meta/run_flow` refused its `role_targets` before asking the runtime.
#[derive(Debug)]
pub(crate) enum RoleTargetsRefusal {
    /// The parameter is malformed: invalid params (`-32602`), without data.
    Invalid(String),
    /// A generated runtime alias that is not its identity's current
    /// incarnation: the typed `stale_identity_runtime_binding` refusal every
    /// other member action answers for it.
    StaleAlias(crate::rpc::JsonRpcError),
}

/// Read the optional `role_targets` parameter of `meta/run_flow` and resolve
/// it into the binding the mob actor admits.
///
/// Absent or `null` is a role-wide run (`Ok(None)`). Otherwise it is an
/// object mapping each role to a non-empty array of member ids, none naming
/// the same roster row twice for one role. Each id is a public alias, and
/// resolves by the member-id alias rule: the encoded roster spelling is
/// refused; a durable identity or plain member name names its own row; a
/// generated runtime alias names its identity's durable row only when the
/// identity runtime confirms it is the current incarnation, is refused typed
/// when it is stale, and without an identity runtime keeps its exact
/// spelling, which names no row (the mob actor then refuses it as
/// `not_a_member`). Membership, role and lifecycle are not checked here; the
/// mob actor admits the binding.
pub(crate) async fn resolve_role_targets(
    params: &Value,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
) -> Result<Option<meerkat_mob::FlowRoleTargets>, RoleTargetsRefusal> {
    let Some(requested) = role_targets_request(params).map_err(RoleTargetsRefusal::Invalid)? else {
        return Ok(None);
    };
    let mut bound = Vec::with_capacity(requested.len());
    for (role, members) in requested {
        let mut roster_ids = Vec::with_capacity(members.len());
        for member in &members {
            let roster_id = role_target_roster_id(identity_runtime, member).await?;
            if roster_ids.contains(&roster_id) {
                return Err(RoleTargetsRefusal::Invalid(format!(
                    "role_targets.{role} names member '{member}' more than once"
                )));
            }
            roster_ids.push(roster_id);
        }
        bound.push((meerkat_mob::ProfileName::from(role.as_str()), roster_ids));
    }
    meerkat_mob::FlowRoleTargets::new(bound)
        .map(Some)
        .map_err(|error| RoleTargetsRefusal::Invalid(error.to_string()))
}

/// Each requested role with its member ids, as sent.
type RequestedRoleTargets = Vec<(String, Vec<String>)>;

/// The shape of the `role_targets` parameter: each role with its member ids,
/// as sent. `Ok(None)` when it is absent or `null`.
fn role_targets_request(params: &Value) -> Result<Option<RequestedRoleTargets>, String> {
    let raw = match params.get(ROLE_TARGETS_PARAM) {
        None | Some(Value::Null) => return Ok(None),
        Some(raw) => raw,
    };
    let Some(roles) = raw.as_object() else {
        return Err(
            "role_targets must be an object mapping each role to an array of member ids"
                .to_string(),
        );
    };
    let mut requested = Vec::with_capacity(roles.len());
    for (role, members) in roles {
        let Some(members) = members.as_array() else {
            return Err(format!(
                "role_targets.{role} must be an array of member ids"
            ));
        };
        let mut ids = Vec::with_capacity(members.len());
        for member in members {
            let Some(member) = member.as_str() else {
                return Err(format!("role_targets.{role} must hold member id strings"));
            };
            if member.trim().is_empty() {
                return Err(format!("role_targets.{role} holds an empty member id"));
            }
            validate_public_member_alias(ROLE_TARGETS_PARAM, member)?;
            ids.push(member.to_owned());
        }
        requested.push((role.clone(), ids));
    }
    Ok(Some(requested))
}

/// The roster row one `role_targets` member id names, by the member-id alias
/// rule.
async fn role_target_roster_id(
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    supplied: &str,
) -> Result<meerkat_mob::AgentIdentity, RoleTargetsRefusal> {
    let alias = runtime_alias_str(supplied).into_owned();
    // A durable identity or a plain member name names its own row.
    let Some(durable) = durable_identity_from_runtime_alias(&alias) else {
        return Ok(roster_member_id_for_supplied_id(&alias));
    };
    // A generated alias never collapses onto the current incarnation without
    // the identity authority: it keeps its exact spelling.
    let Some(identity_runtime) = identity_runtime else {
        return Ok(mob_member_id(&alias));
    };
    let Ok(identity) = crate::identity_first::AgentIdentity::parse(&durable) else {
        return Ok(mob_member_id(&alias));
    };
    let status = identity_runtime.status(&identity).await.ok();
    let registered = status
        .as_ref()
        .and_then(|status| status.agent_runtime_id.as_ref())
        .map(crate::identity_first::AgentRuntimeId::as_str);
    if registered == Some(alias.as_str()) {
        return Ok(roster_member_id_for_identity(&durable));
    }
    Err(RoleTargetsRefusal::StaleAlias(
        crate::identity_control_target::stale_identity_runtime_binding_error(
            RUN_FLOW_FAILED,
            &identity,
            status.as_ref(),
            &alias,
            None,
        ),
    ))
}

/// Read the optional `idempotency_key` parameter of `meta/run_flow` into the
/// caller identity Meerkat admits a keyed run under.
///
/// Absent or `null` is an unkeyed run (`Ok(None)`). A key must be canonical
/// key text, which Meerkat's identity checks. A keyed flow run neither stores
/// nor compares a correlation id, so one is minted per call.
pub(crate) fn run_flow_delivery_identity(
    params: &Value,
) -> Result<Option<meerkat_mob::MobExternalDeliveryIdentity>, String> {
    let key = match params.get(IDEMPOTENCY_KEY_PARAM) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(key)) => key,
        Some(_) => return Err("idempotency_key must be a string".to_string()),
    };
    meerkat_mob::MobExternalDeliveryIdentity::new(key.as_str(), uuid::Uuid::new_v4().to_string())
        .map(Some)
        .map_err(|error| format!("idempotency_key: {error}"))
}

/// Start a `meta/run_flow` run through the Meerkat admission its parameters
/// select: role targets, a caller key, both or neither.
pub(crate) async fn start_run_flow(
    handle: &meerkat_mob::MobHandle,
    flow_id: meerkat_mob::FlowId,
    params: Value,
    role_targets: Option<meerkat_mob::FlowRoleTargets>,
    identity: Option<meerkat_mob::MobExternalDeliveryIdentity>,
) -> Result<meerkat_mob::RunId, meerkat_mob::MobError> {
    match (role_targets, identity) {
        (Some(role_targets), Some(identity)) => {
            Box::pin(handle.run_flow_with_role_targets_and_external_identity(
                flow_id,
                params,
                role_targets,
                &identity,
            ))
            .await
        }
        (Some(role_targets), None) => {
            Box::pin(handle.run_flow_with_role_targets(flow_id, params, role_targets, None)).await
        }
        (None, Some(identity)) => {
            Box::pin(handle.run_flow_with_external_identity(flow_id, params, &identity)).await
        }
        (None, None) => Box::pin(handle.run_flow(flow_id, params)).await,
    }
}

/// The binding as `meta/run_flow` echoes it: every bound role with its
/// members as public aliases. A client compares it with what it sent; a
/// server that does not echo a binding did not apply one.
pub(crate) fn role_targets_echo(role_targets: &meerkat_mob::FlowRoleTargets) -> Value {
    Value::Object(
        role_targets
            .bound_roles()
            .map(|(role, members)| {
                (
                    role.as_str().to_owned(),
                    Value::Array(
                        members
                            .iter()
                            .map(|member| {
                                Value::String(runtime_alias_str(member.as_str()).into_owned())
                            })
                            .collect(),
                    ),
                )
            })
            .collect::<Map<String, Value>>(),
    )
}

/// The `meta/run_flow` result: the run id, and the echoed binding for a
/// role-targeted run.
pub(crate) fn run_flow_result(run_id: &meerkat_mob::RunId, echo: Option<Value>) -> Value {
    match echo {
        Some(echo) => serde_json::json!({
            "run_id": run_id.to_string(),
            ROLE_TARGETS_PARAM: echo,
        }),
        None => serde_json::json!({ "run_id": run_id.to_string() }),
    }
}

/// The JSON-RPC error data of a failed `meta/run_flow`.
///
/// A refused binding carries Meerkat's typed refusal, a closed vocabulary:
/// `{"kind": "flow_role_targets_refused", "flow_id", "refusal": {"kind", ...}}`
/// with the refused member as a public alias. A key already used with other
/// activation params or another binding carries
/// `{"kind": "flow_run_idempotency_conflict", "run_id", "flow_id"}`. Any
/// other failure carries no data, as before.
pub(crate) fn run_flow_error_data(error: &meerkat_mob::MobError) -> Option<Value> {
    if !matches!(
        error,
        meerkat_mob::MobError::FlowRoleTargetsRefused { .. }
            | meerkat_mob::MobError::FlowRunIdempotencyConflict { .. }
    ) {
        return None;
    }
    let mut data = error.structured_data()?;
    let alias = data
        .pointer("/refusal/member")
        .and_then(Value::as_str)
        .map(|roster_id| runtime_alias_str(roster_id).into_owned());
    if let Some(alias) = alias
        && let Some(member) = data.pointer_mut("/refusal/member")
    {
        *member = Value::String(alias);
    }
    Some(data)
}

/// The bound members a `meta/run_flow` request names, as the public aliases
/// the console scopes its `agent.send` check to. Malformed entries are
/// skipped here, and a generated alias is checked as its durable identity;
/// the handler refuses a malformed binding and a stale alias before anything
/// runs.
pub(crate) fn role_target_members_for_access(params: &Value) -> Vec<String> {
    let Some(roles) = params.get(ROLE_TARGETS_PARAM).and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut members = roles
        .values()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_str)
        .map(|member| {
            runtime_alias_str(roster_member_id_for_supplied_id(member).as_str()).into_owned()
        })
        .collect::<Vec<_>>();
    members.sort();
    members.dedup();
    members
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn resolved(
        params: &Value,
        identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    ) -> Result<Option<meerkat_mob::FlowRoleTargets>, String> {
        resolve_role_targets(params, identity_runtime)
            .await
            .map_err(|refusal| match refusal {
                RoleTargetsRefusal::Invalid(message) => message,
                RoleTargetsRefusal::StaleAlias(error) => {
                    panic!("no identity runtime, no stale refusal: {error:?}")
                }
            })
    }

    async fn bound(params: &Value) -> meerkat_mob::FlowRoleTargets {
        resolved(params, None)
            .await
            .expect("well-formed role targets resolve")
            .expect("present role targets resolve to a binding")
    }

    /// An identity runtime that knows no identity: every generated alias is
    /// stale to it.
    fn empty_identity_runtime() -> Arc<crate::identity_first::IdentityRuntime> {
        Arc::new(crate::identity_first::IdentityRuntime::new(
            crate::identity_first::IdentityRuntimeConfig {
                continuity_store: Arc::new(
                    crate::identity_first::LocalContinuityStore::in_memory()
                        .expect("in-memory continuity store"),
                ),
                lease_provider: Arc::new(crate::identity_first::LocalLeaseProvider::new()),
                runtime_instance_id: "flow-role-targets-resolver-test".to_string(),
                has_runtime_store: true,
                durability_policy: crate::identity_first::DurabilityPolicy::SyncWriteThrough,
                bridge: None,
                default_timeout: None,
            },
        ))
    }

    #[tokio::test]
    async fn absent_or_null_role_targets_is_a_role_wide_run() {
        assert_eq!(
            resolved(&json!({"flow_id": "review"}), None).await,
            Ok(None)
        );
        assert_eq!(
            resolved(&json!({"flow_id": "review", "role_targets": null}), None).await,
            Ok(None)
        );
    }

    #[tokio::test]
    async fn malformed_role_targets_are_refused_before_the_runtime_is_asked() {
        for (role_targets, expected) in [
            (json!(["rev-a1"]), "must be an object"),
            (json!({"reviewer": "rev-a1"}), "must be an array"),
            (json!({"reviewer": [1]}), "member id strings"),
            (json!({"reviewer": [" "]}), "empty member id"),
            (
                json!({"reviewer": ["mk--rev_ca1"]}),
                "reserved encoded roster-id",
            ),
            (json!({"reviewer": ["rev-a1", "rev-a1"]}), "more than once"),
            (json!({"reviewer": []}), "to no members"),
            (json!({}), "at least one role"),
        ] {
            let error = resolved(&json!({ "role_targets": role_targets }), None)
                .await
                .expect_err("malformed role targets are refused");
            assert!(
                error.contains(expected),
                "{role_targets} should be refused with `{expected}`, got: {error}"
            );
        }
    }

    /// Without identity authority a generated alias keeps its exact spelling:
    /// it never collapses onto the current incarnation, so it names no row
    /// and is no duplicate of its identity's own id.
    #[tokio::test]
    async fn member_ids_take_the_roster_spelling_and_echo_as_public_aliases() {
        let params = json!({
            "role_targets": {
                "reviewer": ["rev-a2", "rev-a1", "rt:rev-a1:3", "review:issue-7"],
                "coder": ["coder-a"],
            }
        });
        let role_targets = bound(&params).await;
        let reviewers = role_targets
            .members(&meerkat_mob::ProfileName::from("reviewer"))
            .expect("the reviewer role is bound");
        assert!(reviewers.contains(&roster_member_id_for_supplied_id("rev-a1")));
        assert!(reviewers.contains(&roster_member_id_for_supplied_id("review:issue-7")));
        assert!(
            reviewers.contains(&mob_member_id("rt:rev-a1:3")),
            "a generated alias keeps its exact spelling without identity authority"
        );

        let echo = role_targets_echo(&role_targets);
        let mut echoed_reviewers = echo["reviewer"]
            .as_array()
            .expect("the echo maps each role to an array")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        echoed_reviewers.sort();
        assert_eq!(
            echoed_reviewers,
            ["rev-a1", "rev-a2", "review:issue-7", "rt:rev-a1:3"],
            "the echo speaks public aliases, never the encoded roster spelling"
        );
        assert_eq!(echo["coder"], json!(["coder-a"]));

        let run_id = meerkat_mob::RunId::new();
        assert_eq!(
            run_flow_result(&run_id, Some(echo.clone())),
            json!({"run_id": run_id.to_string(), "role_targets": echo})
        );
        assert_eq!(
            run_flow_result(&run_id, None),
            json!({"run_id": run_id.to_string()}),
            "a role-wide run's result is unchanged"
        );
    }

    /// With identity authority a generated alias that is not its identity's
    /// current incarnation is refused with the typed stale-binding refusal;
    /// a durable identity or plain name needs no confirmation.
    #[tokio::test]
    async fn a_stale_generated_alias_is_refused_typed_under_identity_authority() {
        let identity_runtime = empty_identity_runtime();

        let refusal = resolve_role_targets(
            &json!({ "role_targets": { "reviewer": ["rev-a2", "rt:rev-a1:2"] } }),
            Some(&identity_runtime),
        )
        .await;
        let error = match refusal {
            Err(RoleTargetsRefusal::StaleAlias(error)) => error,
            other => panic!("a stale generated alias is refused typed: {other:?}"),
        };
        assert_eq!(error.code, -32000);
        assert!(
            error.message.starts_with("run_flow failed: "),
            "{}",
            error.message
        );
        assert_eq!(
            error.data,
            Some(json!({
                "kind": "stale_identity_runtime_binding",
                "identity": "rev-a1",
                "registered_runtime_member_id": null,
                "live_runtime_member_id": "rt:rev-a1:2",
                "registered_session_id": null,
                "live_session_id": null,
            }))
        );

        let plain = resolve_role_targets(
            &json!({ "role_targets": { "reviewer": ["rev-a1", "review:issue-7"] } }),
            Some(&identity_runtime),
        )
        .await
        .expect("durable identities and plain names resolve")
        .expect("a binding");
        let reviewers = plain
            .members(&meerkat_mob::ProfileName::from("reviewer"))
            .expect("the reviewer role is bound");
        assert!(reviewers.contains(&roster_member_id_for_supplied_id("rev-a1")));
        assert!(reviewers.contains(&roster_member_id_for_supplied_id("review:issue-7")));
    }

    #[test]
    fn a_refused_binding_carries_the_typed_refusal_with_public_member_spelling() {
        let refused = meerkat_mob::MobError::FlowRoleTargetsRefused {
            flow_id: meerkat_mob::FlowId::from("review"),
            refusal: meerkat_mob::FlowRoleTargetRefusal::NotAMember {
                role: meerkat_mob::ProfileName::from("reviewer"),
                member: roster_member_id_for_supplied_id("review:issue-7"),
            },
        };
        let data = run_flow_error_data(&refused).expect("a refused binding carries data");
        assert_eq!(data["kind"], json!("flow_role_targets_refused"));
        assert_eq!(data["flow_id"], json!("review"));
        assert_eq!(
            data["refusal"],
            json!({"kind": "not_a_member", "role": "reviewer", "member": "review:issue-7"})
        );

        assert_eq!(
            run_flow_error_data(&meerkat_mob::MobError::FlowNotFound(
                meerkat_mob::FlowId::from("missing")
            )),
            None,
            "other run_flow failures keep their data-less error"
        );
    }

    #[test]
    fn idempotency_key_absent_or_null_is_an_unkeyed_run() {
        assert_eq!(
            run_flow_delivery_identity(&json!({"flow_id": "review"})),
            Ok(None)
        );
        assert_eq!(
            run_flow_delivery_identity(&json!({"flow_id": "review", "idempotency_key": null})),
            Ok(None)
        );
    }

    #[test]
    fn malformed_idempotency_keys_are_invalid_params() {
        for key in [json!(7), json!(""), json!(" padded "), json!("line\nbreak")] {
            let error = run_flow_delivery_identity(&json!({ "idempotency_key": key }))
                .expect_err("a malformed key is refused before Meerkat is asked");
            assert!(error.contains("idempotency_key"), "{key}: {error}");
        }
    }

    #[test]
    fn a_key_becomes_the_caller_identity_with_a_fresh_correlation() {
        let identity = run_flow_delivery_identity(&json!({ "idempotency_key": "issue-7-review" }))
            .expect("a canonical key is accepted")
            .expect("a key admits a keyed run");
        assert_eq!(identity.idempotency_key, "issue-7-review");
        assert!(
            uuid::Uuid::parse_str(&identity.correlation_id).is_ok(),
            "the minted correlation id is a UUID"
        );
    }

    #[test]
    fn a_key_conflict_carries_its_typed_data() {
        let run_id = meerkat_mob::RunId::new();
        let conflict = meerkat_mob::MobError::FlowRunIdempotencyConflict {
            run_id: run_id.clone(),
            flow_id: meerkat_mob::FlowId::from("review"),
        };
        assert_eq!(
            run_flow_error_data(&conflict),
            Some(json!({
                "kind": "flow_run_idempotency_conflict",
                "run_id": run_id.to_string(),
                "flow_id": "review",
            }))
        );
    }

    #[test]
    fn access_targets_are_every_bound_member_once_in_public_spelling() {
        let params = json!({
            "role_targets": {
                "reviewer": ["rev-a1", "rt:rev-a1:2", "review:issue-7"],
                "coder": ["coder-a", 7],
            }
        });
        assert_eq!(
            role_target_members_for_access(&params),
            ["coder-a", "rev-a1", "review:issue-7"]
        );
        assert!(role_target_members_for_access(&json!({"flow_id": "review"})).is_empty());
        assert!(role_target_members_for_access(&json!({"role_targets": ["rev-a1"]})).is_empty());
    }
}
