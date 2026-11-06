//! Console approval decisions take the approver from the connection alone:
//! a verified, current person decides as the access principal its token
//! names, with the approver ids it holds through the live access
//! configuration. Every other caller (no token on an open Console, a
//! service token, a token that names no human, a shared-account token) is
//! refused with `approver_unauthenticated`, and no request field names an
//! approver.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
use super::*;

const DECIDE: &str = "meta/gating/decide";

/// A Console over a gating owner with no modules and an empty mob runtime.
struct ApprovalConsole {
    app: Router,
    owner: Arc<tokio::sync::Mutex<MetaRuntimeHandle>>,
    runtime: MobRuntime,
    _directory: tempfile::TempDir,
}

impl ApprovalConsole {
    /// The Console `name` (its mob's id, unique per Console in a process)
    /// with `decisions`, the session fence record `fences` and the access
    /// controller `access`.
    async fn start(
        name: &str,
        decisions: RuntimeDecisionState,
        fences: crate::auth::session_fence::ConsoleSessionFences,
        access: Option<AccessController>,
    ) -> Self {
        let owner = crate::start_meta_runtime(
            crate::MetaConfig {
                modules: vec![],
                discovery: crate::DiscoverySpec {
                    namespace: name.to_string(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            },
            vec![],
            Duration::from_secs(1),
        )
        .expect("gating owner");
        let owner = Arc::new(tokio::sync::Mutex::new(owner));
        let (directory, runtime) = super::tests::build_empty_console_test_runtime(name)
            .await
            .expect("console runtime");
        let app = console_json_router_with_state(ConsoleJsonState {
            decisions,
            session_fences: fences,
            runtime: Some(runtime.clone()),
            module_runtime: Some(owner.clone()),
            access,
            ..state()
        });
        Self {
            app,
            owner,
            runtime,
            _directory: directory,
        }
    }

    /// A pending R3 gate, routed to `requested_approver` when one is named.
    async fn pending(&self, requested_approver: Option<&str>) -> String {
        let mut request = json!({
            "action": "Publish the reviewed report",
            "actor_id": "agent:writer",
            "risk_tier": "r3",
            "approval_timeout_ms": 600_000,
        });
        if let Some(approver) = requested_approver {
            request["requested_approver"] = json!(approver);
        }
        self.owner
            .lock()
            .await
            .evaluate_gating_action(serde_json::from_value(request).expect("gating request"))
            .pending_id
            .expect("an R3 gate waits for a decision")
    }

    async fn still_pending(&self, pending_id: &str) -> bool {
        self.owner
            .lock()
            .await
            .list_gating_pending()
            .iter()
            .any(|entry| entry.pending_id == pending_id)
    }

    /// Approve `pending_id` with `token`, or with no credentials at all.
    async fn approve(&self, token: Option<&str>, pending_id: &str) -> Value {
        console_rpc_as(
            &self.app,
            token,
            DECIDE,
            json!({ "pending_id": pending_id, "decision": "approve" }),
        )
        .await
    }

    /// The audit entry of the decision on `pending_id`, if one was recorded.
    async fn decision_audit(&self, pending_id: &str) -> Option<crate::runtime::GatingAuditEntry> {
        self.owner
            .lock()
            .await
            .gating_state_snapshot()
            .audit
            .into_iter()
            .find(|entry| {
                entry.event_type == "approval_decided"
                    && entry.pending_id.as_deref() == Some(pending_id)
            })
    }

    /// Stop the mob runtime. Every row stops it before it asserts, so a
    /// failing row still ends.
    async fn stop(self) {
        let _ = self.runtime.handle().stop().await;
    }
}

/// The enforced Console's decisions: verified tokens only.
fn enforced() -> RuntimeDecisionState {
    state().decisions
}

/// An open Console's decisions: callers without a token are anonymous.
fn open() -> RuntimeDecisionState {
    let mut decisions = state().decisions;
    decisions.console.require_app_auth = false;
    decisions
}

fn detached() -> crate::auth::session_fence::ConsoleSessionFences {
    crate::auth::session_fence::ConsoleSessionFences::detached()
}

fn refusal_kind(answer: &Value) -> &Value {
    &answer["error"]["data"]["kind"]
}

/// A person's verified token, with the signed login `sid`.
fn person_token() -> String {
    let mut claims = human_claims();
    claims["sid"] = json!("login-7");
    token(claims, Some(now_seconds() + 3600))
}

/// Row 1. On an open Console a caller without a token decides nothing: a
/// decide that names the requested approver is invalid params (no request
/// field names an approver), and one that names none is
/// `approver_unauthenticated`. The entry stays pending.
#[tokio::test]
async fn open_decide_is_unauthenticated() {
    let console = ApprovalConsole::start("approvers-open", open(), detached(), None).await;
    let pending_id = console.pending(Some("person:bob")).await;
    let named = console_rpc_as(
        &console.app,
        None,
        DECIDE,
        json!({ "pending_id": pending_id, "approver_id": "person:bob", "decision": "approve" }),
    )
    .await;
    let pending_after_named = console.still_pending(&pending_id).await;
    let unnamed = console.approve(None, &pending_id).await;
    let pending_after_unnamed = console.still_pending(&pending_id).await;
    console.stop().await;

    assert_eq!(named["error"]["code"], json!(-32602), "{named}");
    assert!(pending_after_named, "{named}");
    assert_eq!(
        refusal_kind(&unnamed),
        &json!("approver_unauthenticated"),
        "{unnamed}"
    );
    assert!(pending_after_unnamed, "{unnamed}");
}

/// Row 2 (gating). A service token and a token that names no human carry
/// no person: neither decides a gate.
#[tokio::test]
async fn non_person_cannot_decide() {
    let console =
        ApprovalConsole::start("approvers-non-person", enforced(), detached(), None).await;
    let expiry = now_seconds() + 3600;
    let mut answers = Vec::new();
    for (label, claims) in [
        ("service", json!({ "email": "svc:robot" })),
        (
            "no subject",
            json!({ "email": BOB, "provider": "google_oauth" }),
        ),
    ] {
        let pending_id = console.pending(None).await;
        let answer = console
            .approve(Some(&token(claims, Some(expiry))), &pending_id)
            .await;
        answers.push((label, answer, console.still_pending(&pending_id).await));
    }
    console.stop().await;

    for (label, answer, pending) in answers {
        assert_eq!(
            refusal_kind(&answer),
            &json!("approver_unauthenticated"),
            "{label}: {answer}"
        );
        assert!(pending, "{label}: {answer}");
    }
}

/// Row 3. A token minted with an `auth_config` shared secret names a shared
/// account, not a person, unless the configuration declares
/// `accounts: "personal"`: it decides nothing.
#[tokio::test]
async fn shared_account_token_cannot_decide() {
    let issuer = "http://127.0.0.1/meta-gateway";
    let secret = "shared-front-secret";
    let decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "provider": "jwt",
        "shared_secret": secret,
        "issuer": issuer,
        "audience": "shared-front",
        "email_allowlist": ["owner@example.test"],
    }))
    .expect("auth config");
    let shared = jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256),
        &json!({
            "iss": issuer,
            "aud": "shared-front",
            "sub": "owner@example.test",
            "email": "owner@example.test",
            "email_verified": true,
            "sid": "login-1",
            "exp": now_seconds() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("signed fixture JWT");
    let console =
        ApprovalConsole::start("approvers-shared-account", decisions, detached(), None).await;
    let pending_id = console.pending(None).await;
    let answer = console.approve(Some(&shared), &pending_id).await;
    let pending = console.still_pending(&pending_id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert!(pending, "{answer}");
}

/// Row 4. A decide checks its person again after its last await, right
/// before the decision is applied: once the person's Console sessions were
/// revoked while the decide waited for the gating owner, it is refused and
/// the entry stays pending.
#[tokio::test]
async fn decide_rechecks_human_after_last_await() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())
            .expect("fence record");
    let console =
        ApprovalConsole::start("approvers-recheck", enforced(), fences.clone(), None).await;
    let pending_id = console.pending(None).await;
    let hold = super::gating_decide_holds::GatingDecideHold::install(&pending_id);
    let decide = tokio::spawn({
        let app = console.app.clone();
        let bearer = person_token();
        let pending_id = pending_id.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                DECIDE,
                json!({ "pending_id": pending_id, "decision": "approve" }),
            )
            .await
        }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    let revoked =
        fences.revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5));
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let pending = console.still_pending(&pending_id).await;
    console.stop().await;

    reached.expect("the decide reached the gating owner");
    revoked.expect("revoke");
    let answer = answer
        .expect("the decide answered")
        .expect("the decide task");
    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert!(pending, "{answer}");
}

/// Row 6. A person's decision records the assurance the Console
/// established and a digest of the login it was made in, never the token
/// or the login itself.
#[tokio::test]
async fn person_decision_records_assurance_and_login_digest() {
    use sha2::Digest as _;
    let console = ApprovalConsole::start("approvers-audit", enforced(), detached(), None).await;
    let pending_id = console.pending(None).await;
    let bearer = person_token();
    let decided = console.approve(Some(&bearer), &pending_id).await;
    let entry = console.decision_audit(&pending_id).await;
    console.stop().await;

    assert!(decided["error"].is_null(), "{decided}");
    let entry = entry.unwrap_or_else(|| panic!("the decision's audit entry: {decided}"));
    assert_eq!(entry.detail["deciding_principal"], json!(BOB), "{entry:?}");
    assert_eq!(
        entry.detail["deciding_assurance"],
        json!("person_authenticated"),
        "{entry:?}"
    );
    let login_digest = format!("sha256:{:x}", sha2::Sha256::digest(b"login-7"));
    assert_eq!(
        entry.detail["login_scope_digest"],
        json!(login_digest),
        "{entry:?}"
    );
    let text = serde_json::to_string(&entry).expect("audit entry");
    assert!(!text.contains(&bearer), "no token in the audit: {text}");
    assert!(
        !text.contains("login-7"),
        "no raw login in the audit: {text}"
    );
}

/// Row 7 (pin). A routed entry is decided only by a person who holds its
/// approver id through the live access configuration: without the binding
/// the person's decide is refused and the entry stays pending; a holder
/// decides as the routed id.
#[tokio::test]
async fn routed_entry_needs_held_binding() {
    const ROUTED: &str = "role:release-manager";
    let access = |holders: Vec<String>| {
        AccessController::new(crate::access::AccessControlConfig {
            enabled: true,
            admins: vec!["root@example.test".to_string()],
            approvers: BTreeMap::from([(
                ROUTED.to_string(),
                crate::access::AccessApprover {
                    subjects: holders,
                    groups: Vec::new(),
                },
            )]),
            rules: vec![crate::access::AccessRule {
                id: "deciders".to_string(),
                subjects: vec![BOB.to_string()],
                actions: vec![
                    crate::access::ACTION_GATING_VIEW.to_string(),
                    crate::access::ACTION_GATING_DECIDE.to_string(),
                ],
                ..crate::access::AccessRule::default()
            }],
            ..crate::access::AccessControlConfig::default()
        })
        .expect("access controller")
    };
    let mut outcomes = Vec::new();
    // An approver id needs a holder: first someone else holds it, then the
    // person does.
    for (name, holders) in [
        (
            "approvers-other-holder",
            vec!["carol@example.test".to_string()],
        ),
        ("approvers-holder", vec![BOB.to_string()]),
    ] {
        let console =
            ApprovalConsole::start(name, enforced(), detached(), Some(access(holders.clone())))
                .await;
        let pending_id = console.pending(Some(ROUTED)).await;
        let answer = console.approve(Some(&person_token()), &pending_id).await;
        let pending = console.still_pending(&pending_id).await;
        let audit = console.decision_audit(&pending_id).await;
        console.stop().await;
        outcomes.push((holders, answer, pending, audit));
    }

    let (holders, refused, pending, audit) = &outcomes[0];
    assert_eq!(
        refused["error"]["code"],
        json!(-32602),
        "{holders:?}: {refused}"
    );
    assert!(refused["result"].is_null(), "{holders:?}: {refused}");
    assert!(*pending, "{holders:?}: {refused}");
    assert!(audit.is_none(), "{holders:?}: {audit:?}");

    let (holders, decided, pending, audit) = &outcomes[1];
    assert!(decided["error"].is_null(), "{holders:?}: {decided}");
    assert_eq!(decided["result"]["approver_id"], json!(ROUTED), "{decided}");
    assert!(!*pending, "{holders:?}: {decided}");
    let audit = audit.as_ref().expect("the decision's audit entry");
    assert_eq!(audit.detail["approver_binding"], json!("held"), "{audit:?}");
    assert_eq!(audit.detail["deciding_principal"], json!(BOB), "{audit:?}");
}
