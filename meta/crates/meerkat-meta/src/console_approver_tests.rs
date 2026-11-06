//! Console approval decisions take the approver from the connection alone:
//! a verified, current person decides as the access principal its token
//! names, with the approver ids it holds through the live access
//! configuration. Every other caller (no token on an open Console, a
//! service token, a token that names no human, a shared-account token) is
//! refused with `approver_unauthenticated`, and no request field names an
//! approver.
//!
//! The same rule holds for a memory quarantine review and a WorkGraph goal
//! confirmation, which also re-read the caller's grants right before they
//! are applied. A WorkGraph claim is owned by its caller: only the claim's
//! owner, or an access admin, releases it, and an open Console's caller
//! without a token claims nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::console_human_admission_tests::{BOB, state};
use super::console_token_lapse_tests::{console_rpc_as, human_claims, now_seconds, token};
use super::*;

const DECIDE: &str = "meta/gating/decide";

/// A Console over a gating owner with no modules, an empty mob runtime and
/// an in-memory work graph.
struct ApprovalConsole {
    app: Router,
    owner: Arc<tokio::sync::Mutex<MetaRuntimeHandle>>,
    runtime: MobRuntime,
    workgraph: meerkat::WorkGraphService,
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
        Self::start_with_memory(name, decisions, fences, access, None).await
    }

    /// [`Self::start`] with `memory` as the Console's memory panel store.
    async fn start_with_memory(
        name: &str,
        decisions: RuntimeDecisionState,
        fences: crate::auth::session_fence::ConsoleSessionFences,
        access: Option<AccessController>,
        memory: Option<Arc<dyn crate::memory::capabilities::MemoryPanelStore>>,
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
        let workgraph = crate::workgraph_wiring::ephemeral_workgraph_service(name);
        let app = console_json_router_with_state(ConsoleJsonState {
            decisions,
            session_fences: fences,
            runtime: Some(runtime.clone()),
            module_runtime: Some(owner.clone()),
            access,
            memory_panel: memory,
            workgraph: Some(workgraph.clone()),
            ..state()
        });
        Self {
            app,
            owner,
            runtime,
            workgraph,
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

    /// A goal on the Console's work graph: its binding id and the revision
    /// of its item.
    async fn goal(&self) -> (String, u64) {
        let request: meerkat::GoalCreateRequest = serde_json::from_value(json!({
            "title": "Ship the reviewed report",
            "target": { "kind": "owner", "owner_key": { "kind": "agent", "id": "writer" } },
        }))
        .expect("goal request");
        let created = self.workgraph.create_goal(request).await.expect("goal");
        let created = serde_json::to_value(created).expect("goal json");
        (
            created["attention"]["binding_id"]
                .as_str()
                .expect("binding id")
                .to_string(),
            created["item"]["revision"].as_u64().expect("revision"),
        )
    }

    /// The revision of the goal item of `binding_id`: a confirmation adds
    /// its evidence to the item, a new revision.
    async fn goal_revision(&self, binding_id: &str) -> u64 {
        let request: meerkat::GoalStatusRequest =
            serde_json::from_value(json!({ "binding_id": binding_id })).expect("status request");
        let status = self
            .workgraph
            .goal_status(request)
            .await
            .expect("goal status");
        serde_json::to_value(status).expect("status json")["item"]["revision"]
            .as_u64()
            .expect("goal item revision")
    }

    /// An open work item: its id and revision.
    async fn item(&self) -> (String, u64) {
        let item = self
            .workgraph
            .create(meerkat::CreateWorkItemRequest {
                title: "Draft the summary".to_string(),
                ..Default::default()
            })
            .await
            .expect("work item");
        (item.id.to_string(), item.revision)
    }

    /// The owner key of the claim on `id` (`null` when nobody holds one).
    async fn claim_owner(&self, id: &str) -> Value {
        let id: meerkat::WorkItemId = serde_json::from_value(json!(id)).expect("item id");
        let item = self.workgraph.get(None, None, id).await.expect("work item");
        serde_json::to_value(item).expect("item json")["claim"]["owner"]["key"].clone()
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
/// revoked while the decide held the gating owner, it is refused and the
/// entry stays pending.
#[tokio::test]
async fn decide_rechecks_human_after_last_await() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())
            .expect("fence record");
    let console =
        ApprovalConsole::start("approvers-recheck", enforced(), fences.clone(), None).await;
    let pending_id = console.pending(None).await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&pending_id);
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

// The same rule for a memory quarantine review and a WorkGraph goal
// confirmation, and WorkGraph claim ownership.

const QUARANTINE_DECIDE: &str = "meta/memory/quarantine/decide";
const CONFIRM: &str = "meta/workgraph/goal/confirm";
const CLAIM: &str = "meta/workgraph/claim";
const RELEASE: &str = "meta/workgraph/release";

/// The callers of row 2 that name no person: a service token and a token
/// that names no human.
fn non_person_tokens() -> [(&'static str, String); 2] {
    let expiry = now_seconds() + 3600;
    [
        (
            "service",
            token(json!({ "email": "svc:robot" }), Some(expiry)),
        ),
        (
            "no subject",
            token(
                json!({ "email": BOB, "provider": "google_oauth" }),
                Some(expiry),
            ),
        ),
    ]
}

/// A memory store holding one quarantined record of the identity `writer`:
/// the store, the record's id and the content hash a review is bound to.
async fn quarantined_record(
    directory: &std::path::Path,
) -> (
    Arc<dyn crate::memory::capabilities::MemoryPanelStore>,
    String,
    String,
) {
    use crate::AgentMemoryProvider as _;
    use crate::memory::TaintableStore as _;
    use crate::memory::records::MemoryAuthor;
    use crate::memory::staged::StagedBatchKind;

    struct AlwaysQuarantine;
    impl crate::memory::taint::LlmWriteGate for AlwaysQuarantine {
        fn quarantine_reason(
            &self,
            author: &MemoryAuthor,
            _kind: StagedBatchKind,
            _evidence: &[crate::memory::records::EvidenceRef],
        ) -> Option<String> {
            author.is_llm().then(|| "test taint".to_string())
        }
    }

    let store = crate::SqliteAgentMemoryStore::open(directory).expect("memory store");
    store.set_llm_write_gate(Arc::new(AlwaysQuarantine));
    let record = store
        .remember_authored(
            &crate::MemoryScope::Identity {
                realm: "default".to_string(),
                identity: "writer".to_string(),
            },
            crate::NewMemoryRecord {
                kind: crate::MemoryKind::Fact,
                title: "Release cadence".to_string(),
                description: "When releases ship".to_string(),
                body: "Releases ship on Thursdays.".to_string(),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            },
            MemoryAuthor::Agent {
                identity: "writer".to_string(),
            },
        )
        .await
        .expect("quarantined record");
    let memory_id = serde_json::to_value(&record.memory_id).expect("memory id");
    let memory_id = memory_id.as_str().expect("memory id text").to_string();
    let hash =
        crate::memory::records::content_hash("Release cadence", "Releases ship on Thursdays.");
    (Arc::new(store), memory_id, hash)
}

/// A release verdict on `memory_id`, bound to `hash`.
fn quarantine_release(memory_id: &str, hash: &str) -> Value {
    json!({
        "identity": "writer",
        "memory_id": memory_id,
        "verdict": "release",
        "expected_content_hash": hash,
        "rationale": "checked with the owner",
    })
}

/// Row 2 (memory quarantine). A service token and a token that names no
/// human review no quarantined record; a person does.
#[tokio::test]
async fn non_person_cannot_decide_quarantine() {
    let memory = tempfile::tempdir().expect("memory directory");
    let (store, memory_id, hash) = quarantined_record(memory.path()).await;
    let console = ApprovalConsole::start_with_memory(
        "approvers-quarantine-non-person",
        enforced(),
        detached(),
        None,
        Some(store),
    )
    .await;
    let mut refused = Vec::new();
    for (label, bearer) in non_person_tokens() {
        let answer = console_rpc_as(
            &console.app,
            Some(&bearer),
            QUARANTINE_DECIDE,
            quarantine_release(&memory_id, &hash),
        )
        .await;
        refused.push((label, answer));
    }
    let decided = console_rpc_as(
        &console.app,
        Some(&person_token()),
        QUARANTINE_DECIDE,
        quarantine_release(&memory_id, &hash),
    )
    .await;
    console.stop().await;

    for (label, answer) in refused {
        assert_eq!(
            refusal_kind(&answer),
            &json!("approver_unauthenticated"),
            "{label}: {answer}"
        );
    }
    assert!(decided["error"].is_null(), "{decided}");
    assert!(decided["result"]["decision"].is_object(), "{decided}");
}

/// Row 8. The reviewer of a quarantine decision is the signed-in person: an
/// open Console's caller without a token reviews nothing, and a person's
/// decision records the person.
#[tokio::test]
async fn quarantine_reviewer_is_the_person() {
    let open_memory = tempfile::tempdir().expect("memory directory");
    let (open_store, open_id, open_hash) = quarantined_record(open_memory.path()).await;
    let open_console = ApprovalConsole::start_with_memory(
        "approvers-quarantine-open",
        open(),
        detached(),
        None,
        Some(open_store),
    )
    .await;
    let anonymous = console_rpc_as(
        &open_console.app,
        None,
        QUARANTINE_DECIDE,
        quarantine_release(&open_id, &open_hash),
    )
    .await;
    open_console.stop().await;

    let memory = tempfile::tempdir().expect("memory directory");
    let (store, memory_id, hash) = quarantined_record(memory.path()).await;
    let console = ApprovalConsole::start_with_memory(
        "approvers-quarantine-person",
        enforced(),
        detached(),
        None,
        Some(store),
    )
    .await;
    let decided = console_rpc_as(
        &console.app,
        Some(&person_token()),
        QUARANTINE_DECIDE,
        quarantine_release(&memory_id, &hash),
    )
    .await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&anonymous),
        &json!("approver_unauthenticated"),
        "{anonymous}"
    );
    assert!(decided["error"].is_null(), "{decided}");
    let decision = decided["result"]["decision"].to_string();
    assert!(
        decision.contains(&format!("\"principal\":\"{BOB}\"")),
        "{decided}"
    );
}

/// Row 2 (goal confirmation). A service token and a token that names no
/// human confirm no goal; the goal waits, and a person confirms it.
#[tokio::test]
async fn non_person_cannot_confirm_goal() {
    let console =
        ApprovalConsole::start("approvers-confirm-non-person", enforced(), detached(), None).await;
    let (binding_id, revision) = console.goal().await;
    let confirm = json!({ "binding_id": binding_id, "expected_revision": revision });
    let mut refused = Vec::new();
    for (label, bearer) in non_person_tokens() {
        let answer = console_rpc_as(&console.app, Some(&bearer), CONFIRM, confirm.clone()).await;
        refused.push((label, answer, console.goal_revision(&binding_id).await));
    }
    let confirmed = console_rpc_as(&console.app, Some(&person_token()), CONFIRM, confirm).await;
    let confirmed_revision = console.goal_revision(&binding_id).await;
    console.stop().await;

    for (label, answer, after) in refused {
        assert_eq!(
            refusal_kind(&answer),
            &json!("approver_unauthenticated"),
            "{label}: {answer}"
        );
        assert_eq!(after, revision, "{label}: {answer}");
    }
    assert!(confirmed["error"].is_null(), "{confirmed}");
    assert!(confirmed_revision > revision, "{confirmed}");
}

/// An open Console's caller without a token confirms no goal.
#[tokio::test]
async fn open_goal_confirm_is_unauthenticated() {
    let console = ApprovalConsole::start("approvers-confirm-open", open(), detached(), None).await;
    let (binding_id, revision) = console.goal().await;
    let answer = console_rpc_as(
        &console.app,
        None,
        CONFIRM,
        json!({ "binding_id": binding_id, "expected_revision": revision }),
    )
    .await;
    let after = console.goal_revision(&binding_id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert_eq!(after, revision, "{answer}");
}

/// A goal confirmation checks its person again after its last await: once
/// the person's Console sessions were revoked while the confirmation
/// waited, it is refused and the goal stays open.
#[tokio::test]
async fn goal_confirm_rechecks_person_after_last_await() {
    let state_dir = tempfile::tempdir().expect("state directory");
    let fences =
        crate::auth::session_fence::ConsoleSessionFences::open_in_state_dir(state_dir.path())
            .expect("fence record");
    let console = ApprovalConsole::start(
        "approvers-confirm-recheck",
        enforced(),
        fences.clone(),
        None,
    )
    .await;
    let (binding_id, revision) = console.goal().await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&binding_id);
    let confirm = tokio::spawn({
        let app = console.app.clone();
        let bearer = person_token();
        let binding_id = binding_id.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                CONFIRM,
                json!({ "binding_id": binding_id, "expected_revision": revision }),
            )
            .await
        }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    let revoked =
        fences.revoke_sessions(BOB, std::time::SystemTime::now() + Duration::from_secs(5));
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), confirm).await;
    let after = console.goal_revision(&binding_id).await;
    console.stop().await;

    reached.expect("the confirmation reached its last await");
    revoked.expect("revoke");
    let answer = answer
        .expect("the confirmation answered")
        .expect("the confirmation task");
    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert_eq!(after, revision, "{answer}");
}

/// Row 9. A Console claim is owned by its caller: a claim that names no
/// owner is the caller's, one that names anyone else is refused, and one
/// that names the caller is accepted.
#[tokio::test]
async fn claim_owner_is_caller() {
    let console =
        ApprovalConsole::start("approvers-claim-owner", enforced(), detached(), None).await;
    let bearer = person_token();
    let (unnamed, unnamed_revision) = console.item().await;
    let (foreign, foreign_revision) = console.item().await;
    let (named, named_revision) = console.item().await;
    let claimed = console_rpc_as(
        &console.app,
        Some(&bearer),
        CLAIM,
        json!({ "id": unnamed, "expected_revision": unnamed_revision }),
    )
    .await;
    let unnamed_owner = console.claim_owner(&unnamed).await;
    let refused = console_rpc_as(
        &console.app,
        Some(&bearer),
        CLAIM,
        json!({
            "id": foreign,
            "expected_revision": foreign_revision,
            "owner": { "kind": "agent", "id": "writer" },
        }),
    )
    .await;
    let foreign_owner = console.claim_owner(&foreign).await;
    let accepted = console_rpc_as(
        &console.app,
        Some(&bearer),
        CLAIM,
        json!({
            "id": named,
            "expected_revision": named_revision,
            "owner": { "kind": "principal", "id": BOB },
        }),
    )
    .await;
    console.stop().await;

    assert!(claimed["error"].is_null(), "{claimed}");
    assert_eq!(
        unnamed_owner,
        json!({ "kind": "principal", "id": BOB }),
        "{claimed}"
    );
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(foreign_owner, Value::Null, "{refused}");
    assert!(accepted["error"].is_null(), "{accepted}");
}

/// Row 9. Only a claim's owner releases it from the Console: another caller
/// is refused and the claim stays; the owner releases it.
#[tokio::test]
async fn release_by_non_owner_refused() {
    let console =
        ApprovalConsole::start("approvers-release-owner", enforced(), detached(), None).await;
    let bearer = person_token();
    let (id, revision) = console.item().await;
    let claimed = console_rpc_as(
        &console.app,
        Some(&bearer),
        CLAIM,
        json!({
            "id": id,
            "expected_revision": revision,
            "owner": { "kind": "principal", "id": BOB },
        }),
    )
    .await;
    let claimed_revision = claimed["result"]["item"]["revision"]
        .as_u64()
        .unwrap_or_default();
    let service = token(json!({ "email": "svc:robot" }), Some(now_seconds() + 3600));
    let refused = console_rpc_as(
        &console.app,
        Some(&service),
        RELEASE,
        json!({ "id": id, "expected_revision": claimed_revision }),
    )
    .await;
    let owner_after_refusal = console.claim_owner(&id).await;
    let released = console_rpc_as(
        &console.app,
        Some(&bearer),
        RELEASE,
        json!({ "id": id, "expected_revision": claimed_revision }),
    )
    .await;
    console.stop().await;

    assert!(claimed["error"].is_null(), "{claimed}");
    assert_eq!(refused["error"]["code"], json!(-32030), "{refused}");
    assert_eq!(
        owner_after_refusal,
        json!({ "kind": "principal", "id": BOB }),
        "{refused}"
    );
    assert!(released["error"].is_null(), "{released}");
}

/// Row 9. An open Console's caller without a token owns nothing: its claim
/// is refused with `person_authentication_required` and the item stays
/// open.
#[tokio::test]
async fn anonymous_claim_refused() {
    let console =
        ApprovalConsole::start("approvers-claim-anonymous", open(), detached(), None).await;
    let (id, revision) = console.item().await;
    let answer = console_rpc_as(
        &console.app,
        None,
        CLAIM,
        json!({
            "id": id,
            "expected_revision": revision,
            "owner": { "kind": "principal", "id": "someone@example.test" },
        }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&answer),
        &json!("person_authentication_required"),
        "{answer}"
    );
    assert_eq!(owner, Value::Null, "{answer}");
}

// What the console offers to whom, the grants re-read after the last await,
// the access admin's release, and pins for what already holds.

/// Another signed-in person, and an access admin.
const CAROL: &str = "carol@example.test";
const ROOT: &str = "root@example.test";

/// The enforced Console's decisions, also admitting `subjects` (the
/// fixture admits BOB and `svc:robot` only).
fn enforced_admitting(subjects: &[&str]) -> RuntimeDecisionState {
    let mut decisions = enforced();
    decisions
        .auth
        .email_allowlist
        .extend(subjects.iter().map(|subject| (*subject).to_string()));
    decisions
}

/// A person's verified token for `email`, with a subject of its own.
fn person_token_for(email: &str) -> String {
    token(
        json!({ "sub": format!("oidc-sub-{email}"), "email": email, "provider": "google_oauth" }),
        Some(now_seconds() + 3600),
    )
}

/// An enforcing access configuration: `admins` administer access, and every
/// caller may read and manage the work graph.
fn workgraph_access_config(admins: &[&str]) -> crate::access::AccessControlConfig {
    crate::access::AccessControlConfig {
        enabled: true,
        admins: admins.iter().map(|admin| (*admin).to_string()).collect(),
        rules: vec![crate::access::AccessRule {
            id: "workgraph".to_string(),
            actions: vec![
                crate::access::ACTION_WORKGRAPH_VIEW.to_string(),
                crate::access::ACTION_WORKGRAPH_MANAGE.to_string(),
            ],
            ..crate::access::AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    }
}

fn workgraph_access(admins: &[&str]) -> AccessController {
    AccessController::new(workgraph_access_config(admins)).expect("access controller")
}

/// An access controller that enforces nothing: an open Console verifies a
/// volunteered token only with one.
fn non_enforcing_access() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig::default()).expect("access controller")
}

impl ApprovalConsole {
    /// Claim `id` at `revision` with `bearer`: the claimed revision.
    async fn claim_as(&self, bearer: &str, id: &str, revision: u64) -> u64 {
        let claimed = console_rpc_as(
            &self.app,
            Some(bearer),
            CLAIM,
            json!({ "id": id, "expected_revision": revision }),
        )
        .await;
        claimed["result"]["item"]["revision"]
            .as_u64()
            .unwrap_or_else(|| panic!("the claim: {claimed}"))
    }

    /// The work item `id` as the work graph holds it.
    async fn work_item(&self, id: &str) -> Value {
        let id: meerkat::WorkItemId = serde_json::from_value(json!(id)).expect("item id");
        let item = self.workgraph.get(None, None, id).await.expect("work item");
        serde_json::to_value(item).expect("item json")
    }

    /// The break-glass record of the last release of `id` the work graph's
    /// events hold, if one names it.
    async fn release_audit(&self, id: &str) -> Option<Value> {
        let events = self
            .workgraph
            .events(meerkat::WorkGraphEventFilter::default())
            .await
            .expect("work graph events");
        events
            .into_iter()
            .rev()
            .find(|event| {
                event.kind == meerkat::WorkGraphEventKind::Released
                    && event
                        .item_id
                        .as_ref()
                        .is_some_and(|item| item.to_string() == id)
            })
            .map(|event| event.payload["break_glass"].clone())
            .filter(|record| !record.is_null())
    }
}

/// Pin. A login that expires (by `exp`, not by a
/// revocation) while the decide holds the gating owner is refused like a
/// revoked one, and the entry stays pending.
#[tokio::test]
async fn login_expiring_while_waiting_is_refused() {
    let console = ApprovalConsole::start("approvers-expiry", enforced(), detached(), None).await;
    let pending_id = console.pending(None).await;
    // The verifier accepts a token through second `exp + 30` and refuses it
    // from `exp + 31`: this one has ten seconds left.
    let expiry = now_seconds() - 20;
    let mut claims = human_claims();
    claims["sid"] = json!("login-expiring");
    let bearer = token(claims, Some(expiry));
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&pending_id);
    let decide = tokio::spawn({
        let app = console.app.clone();
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
    while now_seconds() < expiry + 32 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let pending = console.still_pending(&pending_id).await;
    console.stop().await;

    reached.expect("the decide reached the gating owner");
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

/// Pin. An `oidc` issuer's tokens name persons
/// unless its configuration declares `accounts: "shared"`: the default and
/// `"personal"` decide, `"shared"` decides nothing. The issuer's key is made
/// for the row.
#[tokio::test]
async fn oidc_accounts_decide_by_kind() {
    use base64::Engine as _;
    use ring::signature::KeyPair as _;
    const ISSUER: &str = "https://oidc.approvers.example";
    const AUDIENCE: &str = "approvers-console";
    const KID: &str = "ec-approvers";
    let rng = ring::rand::SystemRandom::new();
    let algorithm = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(algorithm, &rng).expect("P-256 key");
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(algorithm, pkcs8.as_ref(), &rng)
        .expect("P-256 key pair");
    // The public key is an uncompressed point: 0x04, then x and y.
    let point = pair.public_key().as_ref();
    let coordinate = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let jwks = json!({ "keys": [{
        "kid": KID, "kty": "EC", "alg": "ES256", "crv": "P-256",
        "x": coordinate(&point[1..33]), "y": coordinate(&point[33..65]),
    }] });
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some(KID.to_string());
    let bearer = jsonwebtoken::encode(
        &header,
        &json!({
            "sub": "sub-alice",
            "email": "alice@example.com",
            "email_verified": true,
            "iss": ISSUER,
            "aud": AUDIENCE,
            "sid": "login-oidc",
            "exp": now_seconds() + 3600,
        }),
        &jsonwebtoken::EncodingKey::from_ec_der(pkcs8.as_ref()),
    )
    .expect("ES256 token");
    let decisions = |accounts: Option<&str>| {
        let mut config = json!({
            "provider": "oidc",
            "issuer": ISSUER,
            "audience": AUDIENCE,
            "jwks_json": jwks.clone(),
            "email_allowlist": ["alice@example.com"],
        });
        if let Some(accounts) = accounts {
            config["accounts"] = json!(accounts);
        }
        crate::console_auth_config::parse_console_auth_config(&config).expect("oidc auth config")
    };
    let mut outcomes = Vec::new();
    for (name, accounts, decides) in [
        ("approvers-oidc-default", None, true),
        ("approvers-oidc-personal", Some("personal"), true),
        ("approvers-oidc-shared", Some("shared"), false),
    ] {
        let console = ApprovalConsole::start(name, decisions(accounts), detached(), None).await;
        let pending_id = console.pending(None).await;
        let answer = console.approve(Some(&bearer), &pending_id).await;
        let pending = console.still_pending(&pending_id).await;
        console.stop().await;
        outcomes.push((name, decides, answer, pending));
    }

    for (name, decides, answer, pending) in outcomes {
        if decides {
            assert!(answer["error"].is_null(), "{name}: {answer}");
            assert!(!pending, "{name}: {answer}");
        } else {
            assert_eq!(
                refusal_kind(&answer),
                &json!("approver_unauthenticated"),
                "{name}: {answer}"
            );
            assert!(pending, "{name}: {answer}");
        }
    }
}

/// Pin. A service token that carries a subject
/// (`actor_type: "service"`) is no person: it decides nothing.
#[tokio::test]
async fn service_token_with_subject_cannot_decide() {
    let console =
        ApprovalConsole::start("approvers-service-subject", enforced(), detached(), None).await;
    let pending_id = console.pending(None).await;
    let bearer = token(
        json!({ "sub": "svc:robot", "email": "svc:robot", "actor_type": "service" }),
        Some(now_seconds() + 3600),
    );
    let answer = console.approve(Some(&bearer), &pending_id).await;
    let pending = console.still_pending(&pending_id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&answer),
        &json!("approver_unauthenticated"),
        "{answer}"
    );
    assert!(pending, "{answer}");
}

/// Pin. An open Console with an access controller
/// verifies a volunteered token: a person's decides as the person, and a
/// shared-account token, like no token, decides nothing.
#[tokio::test]
async fn open_console_volunteered_tokens_decide_by_kind() {
    let console = ApprovalConsole::start(
        "approvers-open-volunteered",
        open(),
        detached(),
        Some(non_enforcing_access()),
    )
    .await;
    let anonymous_pending = console.pending(None).await;
    let anonymous = console.approve(None, &anonymous_pending).await;
    let anonymous_still = console.still_pending(&anonymous_pending).await;
    let person_pending = console.pending(None).await;
    let person = console
        .approve(Some(&person_token()), &person_pending)
        .await;
    let person_still = console.still_pending(&person_pending).await;
    let audit = console.decision_audit(&person_pending).await;
    console.stop().await;

    let issuer = "http://127.0.0.1/meta-gateway";
    let secret = "shared-front-secret";
    let mut shared_decisions = crate::console_auth_config::parse_console_auth_config(&json!({
        "provider": "jwt",
        "shared_secret": secret,
        "issuer": issuer,
        "audience": "shared-front",
        "email_allowlist": ["owner@example.test"],
    }))
    .expect("auth config");
    shared_decisions.console.require_app_auth = false;
    let shared_token = jsonwebtoken::encode(
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
    let shared_console = ApprovalConsole::start(
        "approvers-open-shared",
        shared_decisions,
        detached(),
        Some(non_enforcing_access()),
    )
    .await;
    let shared_pending = shared_console.pending(None).await;
    let shared = shared_console
        .approve(Some(&shared_token), &shared_pending)
        .await;
    let shared_still = shared_console.still_pending(&shared_pending).await;
    shared_console.stop().await;

    assert_eq!(
        refusal_kind(&anonymous),
        &json!("approver_unauthenticated"),
        "{anonymous}"
    );
    assert!(anonymous_still, "{anonymous}");
    assert!(person["error"].is_null(), "{person}");
    assert!(!person_still, "{person}");
    let audit = audit.unwrap_or_else(|| panic!("the decision's audit entry: {person}"));
    assert_eq!(audit.detail["deciding_principal"], json!(BOB), "{audit:?}");
    assert_eq!(
        audit.detail["deciding_assurance"],
        json!("person_authenticated"),
        "{audit:?}"
    );
    assert_eq!(
        refusal_kind(&shared),
        &json!("approver_unauthenticated"),
        "{shared}"
    );
    assert!(shared_still, "{shared}");
}

/// `meta/capabilities` lists a
/// decision (a gate, a quarantine review, a goal confirmation) only to a
/// person, and a claim and its release only to a caller with a principal:
/// nobody else could ever use them.
#[tokio::test]
async fn capabilities_offer_decisions_only_to_a_person() {
    let listed = |answer: Value| -> BTreeSet<String> {
        answer["result"]["methods"]
            .as_array()
            .unwrap_or_else(|| panic!("the capabilities: {answer}"))
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect()
    };
    let memory = tempfile::tempdir().expect("memory directory");
    let (store, _, _) = quarantined_record(memory.path()).await;
    let console = ApprovalConsole::start_with_memory(
        "approvers-capabilities",
        enforced(),
        detached(),
        None,
        Some(store),
    )
    .await;
    let expiry = now_seconds() + 3600;
    let mut callers = Vec::new();
    for (label, bearer) in [
        ("person", person_token()),
        (
            "service",
            token(json!({ "email": "svc:robot" }), Some(expiry)),
        ),
        (
            "no subject",
            token(
                json!({ "email": BOB, "provider": "google_oauth" }),
                Some(expiry),
            ),
        ),
    ] {
        let answer =
            console_rpc_as(&console.app, Some(&bearer), "meta/capabilities", json!({})).await;
        callers.push((label, listed(answer)));
    }
    console.stop().await;
    let open_memory = tempfile::tempdir().expect("memory directory");
    let (open_store, _, _) = quarantined_record(open_memory.path()).await;
    let open_console = ApprovalConsole::start_with_memory(
        "approvers-capabilities-open",
        open(),
        detached(),
        None,
        Some(open_store),
    )
    .await;
    let anonymous = console_rpc_as(&open_console.app, None, "meta/capabilities", json!({})).await;
    callers.push(("anonymous", listed(anonymous)));
    open_console.stop().await;

    for (label, methods) in &callers {
        let person = *label == "person";
        let identified = *label != "anonymous";
        for method in [DECIDE, QUARANTINE_DECIDE, CONFIRM] {
            assert_eq!(
                methods.contains(method),
                person,
                "{label}: {method}: {methods:?}"
            );
        }
        for method in [CLAIM, RELEASE] {
            assert_eq!(
                methods.contains(method),
                identified,
                "{label}: {method}: {methods:?}"
            );
        }
    }
}

/// A goal confirmation re-reads the caller's grants
/// after its last await: once `workgraph.manage` was withdrawn while the
/// confirmation waited on its evidence read, it is refused (`-32030`) and
/// the goal stays as it was.
#[tokio::test]
async fn goal_confirm_grant_removed_while_waiting_is_refused() {
    let access = workgraph_access(&[ROOT]);
    let console = ApprovalConsole::start(
        "approvers-confirm-grant",
        enforced(),
        detached(),
        Some(access.clone()),
    )
    .await;
    let (binding_id, revision) = console.goal().await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&binding_id);
    let confirm = tokio::spawn({
        let app = console.app.clone();
        let bearer = person_token();
        let binding_id = binding_id.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                CONFIRM,
                json!({ "binding_id": binding_id, "expected_revision": revision }),
            )
            .await
        }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    let withdrawn = access.upsert_rule(crate::access::AccessRule {
        id: "workgraph".to_string(),
        actions: vec![crate::access::ACTION_WORKGRAPH_VIEW.to_string()],
        ..crate::access::AccessRule::default()
    });
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), confirm).await;
    let after = console.goal_revision(&binding_id).await;
    console.stop().await;

    reached.expect("the confirmation reached its last await");
    withdrawn.expect("withdraw workgraph.manage");
    let answer = answer
        .expect("the confirmation answered")
        .expect("the confirmation task");
    assert_eq!(answer["error"]["code"], json!(-32030), "{answer}");
    assert_eq!(after, revision, "{answer}");
}

/// A quarantine review re-reads the caller's grants
/// right before it is handed to the store: once the reviewer grant was
/// withdrawn while the review waited, it is refused (`-32030`) and the
/// record stays quarantined.
#[tokio::test]
async fn quarantine_grant_removed_while_waiting_is_refused() {
    use crate::access::{
        ACTION_AGENT_MEMORY_READ, ACTION_AGENT_MEMORY_WRITE, ACTION_AGENT_VIEW,
        ACTION_MEMORY_QUARANTINE_REVIEW,
    };
    let reviewer = |actions: &[&str]| crate::access::AccessRule {
        id: "reviewer".to_string(),
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        ..crate::access::AccessRule::default()
    };
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![reviewer(&[
            ACTION_MEMORY_QUARANTINE_REVIEW,
            ACTION_AGENT_VIEW,
            ACTION_AGENT_MEMORY_READ,
            ACTION_AGENT_MEMORY_WRITE,
        ])],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller");
    let memory = tempfile::tempdir().expect("memory directory");
    let (store, memory_id, hash) = quarantined_record(memory.path()).await;
    let console = ApprovalConsole::start_with_memory(
        "approvers-quarantine-grant",
        enforced(),
        detached(),
        Some(access.clone()),
        Some(store.clone()),
    )
    .await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&memory_id);
    let decide = tokio::spawn({
        let app = console.app.clone();
        let bearer = person_token();
        let release = quarantine_release(&memory_id, &hash);
        async move { console_rpc_as(&app, Some(&bearer), QUARANTINE_DECIDE, release).await }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    let withdrawn = access.upsert_rule(reviewer(&[
        ACTION_AGENT_VIEW,
        ACTION_AGENT_MEMORY_READ,
        ACTION_AGENT_MEMORY_WRITE,
    ]));
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let record = store
        .record_by_id("default", &memory_id)
        .await
        .expect("record read")
        .expect("the record");
    console.stop().await;

    reached.expect("the review reached its last step");
    withdrawn.expect("withdraw the reviewer grant");
    let answer = answer
        .expect("the review answered")
        .expect("the review task");
    assert_eq!(answer["error"]["code"], json!(-32030), "{answer}");
    assert_eq!(record.status.kind_str(), "quarantined", "{answer}");
}

/// An access admin releases someone else's claim from
/// the Console: the item returns to the pool (open, unclaimed), and the work
/// graph's Released event records the admin and the claim it released.
#[tokio::test]
async fn admin_releases_someone_elses_claim() {
    let console = ApprovalConsole::start(
        "approvers-admin-release",
        enforced_admitting(&[ROOT]),
        detached(),
        Some(workgraph_access(&[ROOT])),
    )
    .await;
    let (id, revision) = console.item().await;
    let claimed = console.claim_as(&person_token(), &id, revision).await;
    let released = console_rpc_as(
        &console.app,
        Some(&person_token_for(ROOT)),
        RELEASE,
        json!({ "id": id, "expected_revision": claimed }),
    )
    .await;
    let item = console.work_item(&id).await;
    let audit = console.release_audit(&id).await;
    console.stop().await;

    assert!(released["error"].is_null(), "{released}");
    assert_eq!(item["status"], json!("open"), "{item}");
    assert_eq!(item["claim"], Value::Null, "{item}");
    let audit = audit.unwrap_or_else(|| panic!("the release's break-glass record: {released}"));
    assert_eq!(audit["principal"], json!(ROOT), "{audit}");
    assert_eq!(
        audit["released_claim"]["owner"]["key"],
        json!({ "kind": "principal", "id": BOB }),
        "{audit}"
    );
}

/// A claim an earlier Console made for its default
/// owner (`console-ops-lead`, which nobody signs in as) is released by an
/// access admin, and the Released event names that owner.
#[tokio::test]
async fn admin_releases_a_legacy_claim() {
    let console = ApprovalConsole::start(
        "approvers-admin-legacy",
        enforced_admitting(&[ROOT]),
        detached(),
        Some(workgraph_access(&[ROOT])),
    )
    .await;
    let (id, revision) = console.item().await;
    let legacy: meerkat::ClaimWorkItemRequest = serde_json::from_value(json!({
        "id": id,
        "expected_revision": revision,
        "owner": { "key": { "kind": "principal", "id": "console-ops-lead" } },
    }))
    .expect("claim request");
    let claimed = console
        .workgraph
        .claim(legacy)
        .await
        .expect("legacy claim")
        .revision;
    let released = console_rpc_as(
        &console.app,
        Some(&person_token_for(ROOT)),
        RELEASE,
        json!({ "id": id, "expected_revision": claimed }),
    )
    .await;
    let item = console.work_item(&id).await;
    let audit = console.release_audit(&id).await;
    console.stop().await;

    assert!(released["error"].is_null(), "{released}");
    assert_eq!(item["status"], json!("open"), "{item}");
    assert_eq!(item["claim"], Value::Null, "{item}");
    let audit = audit.unwrap_or_else(|| panic!("the release's break-glass record: {released}"));
    assert_eq!(audit["principal"], json!(ROOT), "{audit}");
    assert_eq!(
        audit["released_claim"]["owner"]["key"],
        json!({ "kind": "principal", "id": "console-ops-lead" }),
        "{audit}"
    );
}

/// Pin. A signed-in person who is no access admin does
/// not release someone else's claim (`-32030`), and the claim stays.
#[tokio::test]
async fn non_admin_release_of_someone_elses_claim_is_refused() {
    let console = ApprovalConsole::start(
        "approvers-non-admin-release",
        enforced_admitting(&[CAROL, ROOT]),
        detached(),
        Some(workgraph_access(&[ROOT])),
    )
    .await;
    let (id, revision) = console.item().await;
    let claimed = console.claim_as(&person_token(), &id, revision).await;
    let refused = console_rpc_as(
        &console.app,
        Some(&person_token_for(CAROL)),
        RELEASE,
        json!({ "id": id, "expected_revision": claimed }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert_eq!(refused["error"]["code"], json!(-32030), "{refused}");
    assert_eq!(
        owner,
        json!({ "kind": "principal", "id": BOB }),
        "{refused}"
    );
}

/// The admin check reads the access configuration
/// committed after the release's last await: an admin whose administration
/// passed to someone else while the release waited on its item read is
/// refused (`-32030`), and the claim stays.
#[tokio::test]
async fn admin_release_rechecks_admin_after_last_await() {
    let access = workgraph_access(&[ROOT]);
    let console = ApprovalConsole::start(
        "approvers-admin-recheck",
        enforced_admitting(&[CAROL, ROOT]),
        detached(),
        Some(access.clone()),
    )
    .await;
    let (id, revision) = console.item().await;
    let claimed = console.claim_as(&person_token(), &id, revision).await;
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&id);
    let release = tokio::spawn({
        let app = console.app.clone();
        let bearer = person_token_for(ROOT);
        let id = id.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                RELEASE,
                json!({ "id": id, "expected_revision": claimed }),
            )
            .await
        }
    });
    let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
    // An enforcing configuration keeps an admin: administration passes on.
    let handed_over = access.replace_config(workgraph_access_config(&[CAROL]));
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), release).await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    reached.expect("the release read its item");
    handed_over.expect("hand administration over");
    let answer = answer
        .expect("the release answered")
        .expect("the release task");
    assert_eq!(answer["error"]["code"], json!(-32030), "{answer}");
    assert_eq!(owner, json!({ "kind": "principal", "id": BOB }), "{answer}");
}

/// Pin. An owner that names the caller but carries
/// a display name is refused (`-32602`): a free name could misattribute the
/// claim.
#[tokio::test]
async fn claim_owner_with_display_name_is_refused() {
    let console =
        ApprovalConsole::start("approvers-claim-display-name", enforced(), detached(), None).await;
    let (id, revision) = console.item().await;
    let refused = console_rpc_as(
        &console.app,
        Some(&person_token()),
        CLAIM,
        json!({
            "id": id,
            "expected_revision": revision,
            "owner": { "kind": "principal", "id": BOB, "display_name": "Bob" },
        }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert_eq!(owner, Value::Null, "{refused}");
}

/// Pin. Any identified caller claims as itself: a
/// service token's claim is owned by the service's principal.
#[tokio::test]
async fn service_token_claims_as_itself() {
    let console =
        ApprovalConsole::start("approvers-claim-service", enforced(), detached(), None).await;
    let (id, revision) = console.item().await;
    let service = token(json!({ "email": "svc:robot" }), Some(now_seconds() + 3600));
    let claimed = console_rpc_as(
        &console.app,
        Some(&service),
        CLAIM,
        json!({ "id": id, "expected_revision": revision }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert!(claimed["error"].is_null(), "{claimed}");
    assert_eq!(
        owner,
        json!({ "kind": "principal", "id": "svc:robot" }),
        "{claimed}"
    );
}

/// Pin. An open Console's caller without a token
/// releases nothing (`person_authentication_required`), and the claim stays.
#[tokio::test]
async fn anonymous_release_refused() {
    let console = ApprovalConsole::start(
        "approvers-release-anonymous",
        open(),
        detached(),
        Some(non_enforcing_access()),
    )
    .await;
    let (id, revision) = console.item().await;
    let claimed = console.claim_as(&person_token(), &id, revision).await;
    let refused = console_rpc_as(
        &console.app,
        None,
        RELEASE,
        json!({ "id": id, "expected_revision": claimed }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&refused),
        &json!("person_authentication_required"),
        "{refused}"
    );
    assert_eq!(
        owner,
        json!({ "kind": "principal", "id": BOB }),
        "{refused}"
    );
}

/// Pin. A release is bound to the revision it
/// names: after the owner released and someone else claimed the item, a
/// release at the old revision is a stale-revision conflict, and the new
/// claim stays.
#[tokio::test]
async fn release_is_bound_to_its_revision() {
    let console = ApprovalConsole::start(
        "approvers-release-revision",
        enforced_admitting(&[CAROL]),
        detached(),
        None,
    )
    .await;
    let (id, revision) = console.item().await;
    let bob = person_token();
    let first = console.claim_as(&bob, &id, revision).await;
    let released = console_rpc_as(
        &console.app,
        Some(&bob),
        RELEASE,
        json!({ "id": id, "expected_revision": first }),
    )
    .await;
    let released_revision = released["result"]["item"]["revision"]
        .as_u64()
        .unwrap_or_else(|| panic!("the release: {released}"));
    console
        .claim_as(&person_token_for(CAROL), &id, released_revision)
        .await;
    let stale = console_rpc_as(
        &console.app,
        Some(&bob),
        RELEASE,
        json!({ "id": id, "expected_revision": first }),
    )
    .await;
    let owner = console.claim_owner(&id).await;
    console.stop().await;

    assert_eq!(
        refusal_kind(&stale),
        &json!("workgraph_conflict"),
        "{stale}"
    );
    assert_eq!(
        owner,
        json!({ "kind": "principal", "id": CAROL }),
        "{stale}"
    );
}
