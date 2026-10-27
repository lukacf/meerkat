use super::*;

fn proposal() -> Value {
    json!({"job_id":"checked-job", "agent":"selected-agent", "expires_in_ms":60000,
        "connector":{"slot_id":"selected-slot", "issuer":"https://issuer.test",
            "client":"public-client", "resource":"https://api.test", "scopes":["read"],
            "strategy_id":"oidc-userinfo-v1", "account_selection":null},
        "operations":{"list":{"required_scopes":["read"],"destination_origin":"https://api.test"}}})
}

#[test]
fn closed_request_preserves_explicit_selection_and_refuses_authority_fields() {
    let raw = proposal();
    let parsed = parse(&raw.to_string()).unwrap();
    assert_eq!(parsed.agent, "selected-agent");
    assert_eq!(parsed.expires_in_ms, Some(60000));
    assert!(parsed.connector.account_selection.is_discover());
    let operation: McpConnectorOperation = parsed.operations.into_values().next().unwrap().into();
    assert!(!operation.read_only);
    for field in [
        "realm",
        "route",
        "original",
        "grant",
        "callback_code",
        "client_secret",
    ] {
        let mut changed = raw.clone();
        changed[field] = json!("forged");
        assert!(parse(&changed.to_string()).is_err(), "{field}");
    }
    let mut changed = raw.clone();
    changed["operations"]["list"]["read_only"] = json!(true);
    assert!(parse(&changed.to_string()).is_err());
    let mut changed = raw.clone();
    changed["connector"]["account_selection"] = json!({"mode":"unverified"});
    assert!(parse(&changed.to_string()).is_err());
}

#[test]
fn omitted_duplicate_or_zero_lifetime_never_widens_access() {
    for omitted in ["expires_in_ms", "agent"] {
        let mut changed = proposal();
        changed.as_object_mut().unwrap().remove(omitted);
        assert!(parse(&changed.to_string()).is_err());
    }
    let mut changed = proposal();
    changed["expires_in_ms"] = Value::Null;
    assert_eq!(parse(&changed.to_string()).unwrap().expires_in_ms, None);
    changed["expires_in_ms"] = json!(0);
    assert!(parse(&changed.to_string()).is_err());
    let raw = proposal().to_string();
    assert!(parse(&format!("{{\"expires_in_ms\":null,{}", &raw[1..])).is_err());
    let mut changed = proposal();
    changed["connector"]
        .as_object_mut()
        .unwrap()
        .remove("account_selection");
    assert!(parse(&changed.to_string()).is_err());
}

#[test]
fn public_reply_contains_no_account_or_authority_and_never_claims_ready() {
    let reply =
        serde_json::to_value(AccountReply::new(AccountState::Reviewed, true, true)).unwrap();
    assert_eq!(
        reply,
        json!({"state":"reviewed", "login_completed":true,
        "access_reviewed":true, "grant_issued":false, "ready":false, "fresh_instruction_retained":false})
    );
}

#[tokio::test]
async fn current_request_waits_for_its_own_live_response_and_refuses_a_dropped_dispatcher() {
    // Channel transport only, not a native authorization fixture. Production
    // responds only from the same borrowed call's check_tool_authorization.
    let (send, mut receive) = mpsc::channel(1);
    let current = CurrentRequest(send);
    let checking = current.check();
    tokio::pin!(checking);
    use std::{
        future::{Future, poll_fn},
        task::Poll,
    };
    assert!(
        poll_fn(|cx| Poll::Ready(checking.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let response = receive.recv().await.unwrap();
    drop(response);
    assert!(!checking.await);
    drop(receive);
    assert!(!current.check().await);
}

#[test]
fn optional_tool_policy_preserves_legacy_selection_and_cannot_supply_candidate_authority() {
    let base = proposal();
    assert!(parse(&base.to_string()).unwrap().tool_policy.is_none());
    let policy = json!({"tools":[{"name":"actual.alias","consequence":"r1"}],"catalog":"r0"});
    let mut requested = base.clone(); requested["tool_policy"] = policy.clone();
    let parsed = parse(&requested.to_string()).unwrap();
    assert_eq!(serde_json::to_value(parsed.tool_policy.unwrap()).unwrap(), policy);
    for field in ["member", "provider_id", "candidate", "approval", "lifetime", "catalog_session"] {
        let mut changed = requested.clone(); changed["tool_policy"][field] = json!("caller-owned");
        assert!(parse(&changed.to_string()).is_err(), "{field}");
    }
    let parsed = parse(&base.to_string()).unwrap();
    let selection = InstalledAccountSelection { agent: parsed.agent, expires_in_ms: parsed.expires_in_ms,
        operations: parsed.operations.into_iter().map(|(name, operation)| (name, operation.into())).collect(),
        tool_policy: None };
    assert!(serde_json::to_value(selection).unwrap().get("tool_policy").is_none());
}

#[test]
fn captured_consumer_mismatch_returns_exact_model_visible_guidance_without_resolution() {
    let captured = "mk--fixture_cmain";
    for proposed in ["Fixture assistant", "original", "fixture:main", "mk--fixture_cmain "] {
        let error = validate_captured_consumer(SETUP_PUBLIC_SERVICE_REQUEST, proposed, captured).unwrap_err();
        let ToolError::InvalidArguments { name, reason } = error else {
            panic!("a validated name mismatch must explain the exact input correction")
        };
        assert_eq!(name, SETUP_PUBLIC_SERVICE_REQUEST);
        assert!(reason.contains("\"mk--fixture_cmain\""));
        assert!(reason.contains("same job, raw operations and requested lifetime"));
        assert!(reason.contains("has not entered access review"));
    }
    assert!(validate_captured_consumer(SETUP_PUBLIC_SERVICE_REQUEST, captured, captured).is_ok());
    assert!(consumer_agent_schema()["description"].as_str().unwrap().contains("not a display name"));
    let public = serde_json::to_value(public::installed_public_service_request_definition()).unwrap();
    let account = serde_json::to_value(installed_account_request_definition()).unwrap();
    assert!(public.to_string().contains("Exact native member identifier"));
    assert!(account.to_string().contains("Exact native member identifier"));
}
