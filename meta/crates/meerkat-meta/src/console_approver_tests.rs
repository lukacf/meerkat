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
                    ..crate::access::AccessApprover::default()
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

/// Row 9. A claim's owner releases it from the Console: a caller who is
/// neither its owner nor an access admin is refused and the claim stays; the
/// owner releases it.
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

/// A member the Console of the rows below knows.
const KNOWN_ORIGIN: &str = "carol";

/// A member the Console of the rows below was never told about.
const UNKNOWN_ORIGIN: &str = "dora";

/// An access configuration under which every caller sees and decides gating
/// requests and views every agent, with the admin [`ROOT`]. The Console
/// knows the member [`KNOWN_ORIGIN`].
fn origin_access() -> AccessController {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-views-and-decides".to_string(),
            actions: vec![
                ACTION_GATING_VIEW.to_string(),
                ACTION_GATING_DECIDE.to_string(),
                ACTION_AGENT_VIEW.to_string(),
            ],
            ..AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    access.record_agent_attributes(AgentResourceAttributes {
        identity: KNOWN_ORIGIN.to_string(),
        ..Default::default()
    });
    access
}

/// A pending gate whose action came from the member `origin`.
async fn pending_from(console: &ApprovalConsole, origin: &str) -> String {
    let request = json!({
        "action": "Publish the reviewed report",
        "actor_id": "agent:writer",
        "risk_tier": "r3",
        "approval_timeout_ms": 600_000,
    });
    console
        .owner
        .lock()
        .await
        .evaluate_gating_action_with_origin(
            serde_json::from_value(request).expect("gating request"),
            Some(crate::runtime::GatingOrigin {
                identity: origin.to_string(),
                conversation_id: None,
                interaction_id: None,
            }),
        )
        .pending_id
        .expect("an R3 gate waits for a decision")
}

/// What one caller was answered for one pending request: whether the pending
/// list showed it the request, the decide by the request's id, and whether
/// the request was still pending afterwards.
struct DecideById {
    caller: &'static str,
    listed: bool,
    answer: Value,
    still_pending: bool,
}

impl DecideById {
    /// The decide was refused and changed nothing.
    fn refused(&self) -> bool {
        !self.answer["error"].is_null() && self.still_pending
    }

    /// The decide was taken: the request is decided.
    fn taken(&self) -> bool {
        self.answer["error"].is_null() && !self.still_pending
    }

    fn line(&self) -> String {
        let decide = if self.answer["error"].is_null() {
            "taken".to_string()
        } else {
            format!("refused ({})", self.answer["error"])
        };
        format!(
            "{}: the pending list shows the request: {}; the decide by id: {decide}; still \
             pending: {}",
            self.caller, self.listed, self.still_pending
        )
    }
}

/// A fresh pending request of the member `origin`, then the pending list
/// and a decide by the request's id as the caller of `bearer`.
async fn decide_by_id(
    console: &ApprovalConsole,
    caller: &'static str,
    bearer: Option<&str>,
    origin: &str,
) -> DecideById {
    let pending_id = pending_from(console, origin).await;
    let list = console_rpc_as(&console.app, bearer, "meta/gating/pending", json!({})).await;
    let listed = list["result"]["pending"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["pending_id"].as_str() == Some(pending_id.as_str()))
    });
    let answer = console.approve(bearer, &pending_id).await;
    let still_pending = console.still_pending(&pending_id).await;
    DecideById {
        caller,
        listed,
        answer,
        still_pending,
    }
}

/// A pending request whose origin the Console was never told about is
/// withheld from every caller the Console keeps apart from other people: the
/// pending list shows it to an admin only. A decide by the request's id is
/// refused to a caller the list withholds the request from, and leaves the
/// request pending: on a Console that requires sign-in and on one where
/// sign-in is optional, under access rules that reach the member, with the
/// rules switched off, and with no access configuration at all.
#[tokio::test]
async fn a_decide_by_id_of_a_request_the_pending_list_withholds_is_refused() {
    let person = person_token();
    let mut answers = Vec::new();

    let console = ApprovalConsole::start(
        "origin-decide-signed-in",
        enforced(),
        detached(),
        Some(origin_access()),
    )
    .await;
    answers.push(
        decide_by_id(
            &console,
            "a person who is not an admin, sign-in required",
            Some(&person),
            UNKNOWN_ORIGIN,
        )
        .await,
    );
    console.stop().await;

    let console = ApprovalConsole::start(
        "origin-decide-open",
        open(),
        detached(),
        Some(origin_access()),
    )
    .await;
    answers.push(
        decide_by_id(
            &console,
            "a person who volunteers a token where sign-in is optional",
            Some(&person),
            UNKNOWN_ORIGIN,
        )
        .await,
    );
    console.stop().await;

    let rules_off = AccessController::new(crate::access::AccessControlConfig {
        enabled: false,
        admins: vec![ROOT.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    let console = ApprovalConsole::start(
        "origin-decide-rules-off",
        enforced(),
        detached(),
        Some(rules_off),
    )
    .await;
    answers.push(
        decide_by_id(
            &console,
            "a person who is not an admin, access rules switched off",
            Some(&person),
            UNKNOWN_ORIGIN,
        )
        .await,
    );
    console.stop().await;

    let console =
        ApprovalConsole::start("origin-decide-no-access", enforced(), detached(), None).await;
    answers.push(
        decide_by_id(
            &console,
            "a person, no access configuration",
            Some(&person),
            UNKNOWN_ORIGIN,
        )
        .await,
    );
    console.stop().await;

    let every_answer = answers
        .iter()
        .map(DecideById::line)
        .collect::<Vec<_>>()
        .join("\n");
    println!("{every_answer}");
    for answer in &answers {
        assert!(
            !answer.listed,
            "{}: the pending list withholds the request:\n{every_answer}",
            answer.caller
        );
        assert!(
            answer.refused(),
            "{}: a decide by id of a request the pending list withholds is refused and \
             leaves it pending:\n{every_answer}",
            answer.caller
        );
    }
}

/// The callers the pending list shows a request decide it by its id, as
/// before: an admin, for a request whose origin the Console was never told
/// about, and a person who is not an admin, for a request of a member the
/// Console knows and for one that names no origin.
#[tokio::test]
async fn a_caller_the_pending_list_shows_a_request_decides_it_by_id() {
    let person = person_token();
    let admin = person_token_for(ROOT);
    let console = ApprovalConsole::start(
        "origin-decide-shown",
        enforced_admitting(&[ROOT]),
        detached(),
        Some(origin_access()),
    )
    .await;
    let by_admin = decide_by_id(&console, "an admin", Some(&admin), UNKNOWN_ORIGIN).await;
    let by_person = decide_by_id(
        &console,
        "a person, a member the Console knows",
        Some(&person),
        KNOWN_ORIGIN,
    )
    .await;
    let without_origin = console.pending(None).await;
    let no_origin = console.approve(Some(&person), &without_origin).await;
    let no_origin_still_pending = console.still_pending(&without_origin).await;
    console.stop().await;

    for answer in [&by_admin, &by_person] {
        assert!(
            answer.listed && answer.taken(),
            "the pending list shows the request and the decide is taken: {}",
            answer.line()
        );
    }
    assert!(no_origin["error"].is_null(), "{no_origin}");
    assert!(!no_origin_still_pending, "{no_origin}");
}

/// A caller with no token decides no request by its id, as before: where
/// sign-in is optional, a decide of a request of a member the Console knows,
/// of one whose origin it was never told about, and of an id that is not
/// pending is each answered `approver_unauthenticated`, and the requests
/// stay pending.
#[tokio::test]
async fn a_caller_with_no_token_decides_no_request_by_id() {
    let console = ApprovalConsole::start(
        "origin-decide-no-token",
        open(),
        detached(),
        Some(origin_access()),
    )
    .await;
    let known = decide_by_id(&console, "no token, a known member", None, KNOWN_ORIGIN).await;
    let unknown = decide_by_id(
        &console,
        "no token, an origin the Console was never told about",
        None,
        UNKNOWN_ORIGIN,
    )
    .await;
    let not_pending = console.approve(None, "gate-pending-never-minted").await;
    console.stop().await;

    for answer in [&known, &unknown] {
        assert_eq!(
            refusal_kind(&answer.answer),
            &json!("approver_unauthenticated"),
            "{}",
            answer.line()
        );
        assert!(answer.still_pending, "{}", answer.line());
    }
    assert_eq!(
        refusal_kind(&not_pending),
        &json!("approver_unauthenticated"),
        "{not_pending}"
    );
}

/// [`origin_access`] with two more members the Console knows: `erin`, whom
/// an access rule keeps from every caller, and `frank`, who is private to
/// another person.
fn withholding_access() -> AccessController {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-views-and-decides".to_string(),
                actions: vec![
                    ACTION_GATING_VIEW.to_string(),
                    ACTION_GATING_DECIDE.to_string(),
                    ACTION_AGENT_VIEW.to_string(),
                ],
                ..AccessRule::default()
            },
            AccessRule {
                id: "erin-is-hidden".to_string(),
                effect: crate::access::AccessEffect::Deny,
                actions: vec![ACTION_AGENT_VIEW.to_string()],
                agents: vec!["erin".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    for identity in [KNOWN_ORIGIN, "erin"] {
        access.record_agent_attributes(AgentResourceAttributes {
            identity: identity.to_string(),
            ..Default::default()
        });
    }
    access.record_agent_attributes(AgentResourceAttributes {
        identity: "frank".to_string(),
        labels: BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:dana".to_string(),
        )]),
        ..Default::default()
    });
    access
}

/// A decide by id of a request the pending list withholds from its caller
/// is answered exactly as a decide of an id that is not pending: the same
/// response, whatever the caller sends with it, so the caller cannot tell a
/// withheld request from an id that names none. The request is withheld
/// because its origin is a member the Console was never told about, a
/// member an access rule keeps from the caller, or a member private to
/// another person. The caller is a person who is not an admin, on a Console
/// that requires sign-in and on one where sign-in is optional, or sends no
/// token at all; it sends a decision, an unsupported decision, none, or the
/// id with blanks around it. Every request stays pending.
#[tokio::test]
async fn a_decide_of_a_withheld_request_is_answered_as_an_id_that_is_not_pending() {
    const NOT_PENDING: &str = "gate-pending-never-minted";
    let person = person_token();
    let mut differing = Vec::new();
    let mut decided = Vec::new();
    for (console_name, decisions, callers) in [
        (
            "origin-withheld-signed-in",
            enforced(),
            vec![("a person, sign-in required", Some(person.as_str()))],
        ),
        (
            "origin-withheld-open",
            open(),
            vec![
                ("a person who volunteers a token", Some(person.as_str())),
                ("a caller with no token", None),
            ],
        ),
    ] {
        let console = ApprovalConsole::start(
            console_name,
            decisions,
            detached(),
            Some(withholding_access()),
        )
        .await;
        for origin in [UNKNOWN_ORIGIN, "erin", "frank"] {
            let pending_id = pending_from(&console, origin).await;
            for (caller, bearer) in &callers {
                let shapes = |id: &str| {
                    [
                        json!({ "pending_id": id, "decision": "approve" }),
                        json!({ "pending_id": id, "decision": "reject" }),
                        json!({ "pending_id": id, "decision": "maybe" }),
                        json!({ "pending_id": id }),
                        json!({ "pending_id": format!("  {id}\t"), "decision": "approve" }),
                    ]
                };
                for (withheld, absent) in shapes(&pending_id).into_iter().zip(shapes(NOT_PENDING)) {
                    let of_withheld =
                        console_rpc_as(&console.app, *bearer, DECIDE, withheld.clone()).await;
                    let of_absent = console_rpc_as(&console.app, *bearer, DECIDE, absent).await;
                    if of_withheld != of_absent || of_withheld["error"].is_null() {
                        differing.push(format!(
                            "{caller}, origin {origin}, {withheld}: {of_withheld} (an id that \
                             is not pending: {of_absent})"
                        ));
                    }
                }
            }
            if !console.still_pending(&pending_id).await {
                decided.push(format!("{console_name}: the request of {origin}"));
            }
        }
        console.stop().await;
    }

    assert!(
        differing.is_empty(),
        "a decide of a withheld request is answered as an id that is not pending:\n{}",
        differing.join("\n")
    );
    assert!(
        decided.is_empty(),
        "every request stays pending: {decided:?}"
    );
}

/// The origin is judged again at the moment the decision is applied. A
/// person starts a decide of a request of a member the Console knows; while
/// the decide waits, the Console stops knowing that member. The decide is
/// answered as an id that is not pending and the request stays pending.
#[tokio::test]
async fn a_request_withheld_while_its_decide_waits_is_answered_as_not_pending() {
    let access = origin_access();
    let console = ApprovalConsole::start(
        "origin-withheld-while-waiting",
        enforced(),
        detached(),
        Some(access.clone()),
    )
    .await;
    let pending_id = pending_from(&console, KNOWN_ORIGIN).await;
    let bearer = person_token();
    let hold = super::console_decide_holds::ConsoleDecideHold::install(&pending_id);
    let decide = tokio::spawn({
        let app = console.app.clone();
        let bearer = bearer.clone();
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
    // The roster the Console reads next no longer holds the member.
    access.replace_agent_attributes([AgentResourceAttributes {
        identity: "another-member".to_string(),
        ..Default::default()
    }]);
    hold.release();
    let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
    let still_pending = console.still_pending(&pending_id).await;
    let not_pending = console
        .approve(Some(&bearer), "gate-pending-never-minted")
        .await;
    console.stop().await;

    reached.expect("the decide reached the gating owner");
    let answer = answer
        .expect("the decide answered")
        .expect("the decide task");
    assert_eq!(answer, not_pending, "{answer}");
    assert!(!not_pending["error"].is_null(), "{not_pending}");
    assert!(still_pending, "{answer}");
}

/// The delivery record of an approval notification for the request
/// `pending_id`, as the gating owner's send keeps it: the request's id and
/// action text are in its payload.
fn approval_notification_record(
    delivery_id: &str,
    pending_id: &str,
) -> crate::runtime::DeliveryRecord {
    crate::runtime::DeliveryRecord {
        delivery_id: delivery_id.to_string(),
        route_id: "route-approvals".to_string(),
        recipient: "approvals-desk".to_string(),
        sink: "email".to_string(),
        target_module: "delivery".to_string(),
        payload: json!({
            "kind": "gating_approval_request",
            "pending_id": pending_id,
            "action_id": "gate-action-of-the-request",
            "action": "Publish the reviewed report",
            "actor_id": "agent:writer",
            "risk_tier": "r3",
            "requested_approver": null,
            "deadline_at_ms": 0,
        }),
        status: "sent".to_string(),
        attempts: Vec::new(),
        first_attempt_ms: 0,
        final_attempt_ms: 0,
        idempotency_key: Some(format!("gating-approval-{pending_id}")),
        sink_adapter: None,
    }
}

/// The ids of the delivery records `meta/delivery/history` serves the
/// caller of `bearer`.
async fn delivery_ids(console: &ApprovalConsole, bearer: &str) -> BTreeSet<String> {
    let read = console_rpc_as(
        &console.app,
        Some(bearer),
        "meta/delivery/history",
        json!({}),
    )
    .await;
    read["result"]["deliveries"]
        .as_array()
        .unwrap_or_else(|| panic!("a delivery history: {read}"))
        .iter()
        .filter_map(|record| record["delivery_id"].as_str().map(str::to_string))
        .collect()
}

/// The Console's delivery history follows the pending list for the record
/// of an approval notification, whose payload carries the request's id and
/// action text. To a person who is not an admin it shows the record of a
/// request the list shows them (a member the Console knows, a request with
/// no origin) and withholds the record of a request the list withholds (an
/// origin the Console was never told about, a member private to another
/// person) and of a request the gating owner no longer holds, which nothing
/// attributes. Another delivery is read as before, and an admin reads every
/// record.
#[tokio::test]
async fn the_delivery_history_shows_an_approval_notification_as_the_pending_list_shows_its_request()
{
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: false,
        admins: vec![ROOT.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    access.record_agent_attributes(AgentResourceAttributes {
        identity: KNOWN_ORIGIN.to_string(),
        ..Default::default()
    });
    access.record_agent_attributes(AgentResourceAttributes {
        identity: "frank".to_string(),
        labels: BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:dana".to_string(),
        )]),
        ..Default::default()
    });
    let console = ApprovalConsole::start(
        "delivery-history-origin",
        enforced_admitting(&[ROOT]),
        detached(),
        Some(access),
    )
    .await;
    let of_unknown = pending_from(&console, UNKNOWN_ORIGIN).await;
    let of_private = pending_from(&console, "frank").await;
    let of_known = pending_from(&console, KNOWN_ORIGIN).await;
    let without_origin = console.pending(None).await;
    // Two requests that are settled before the history is read: their
    // origin is then read from the gating audit.
    let settled_unknown = pending_from(&console, UNKNOWN_ORIGIN).await;
    let settled_known = pending_from(&console, KNOWN_ORIGIN).await;
    let admin = person_token_for(ROOT);
    for pending_id in [&settled_unknown, &settled_known] {
        let settled = console.approve(Some(&admin), pending_id).await;
        assert!(settled["error"].is_null(), "{settled}");
    }
    {
        let mut owner = console.owner.lock().await;
        for (delivery_id, pending_id) in [
            ("delivery-unknown-origin", of_unknown.as_str()),
            ("delivery-private-origin", of_private.as_str()),
            ("delivery-known-origin", of_known.as_str()),
            ("delivery-no-origin", without_origin.as_str()),
            ("delivery-settled-unknown-origin", settled_unknown.as_str()),
            ("delivery-settled-known-origin", settled_known.as_str()),
            ("delivery-request-not-held", "gate-pending-never-minted"),
        ] {
            owner.record_delivery_for_test(approval_notification_record(delivery_id, pending_id));
        }
        owner.record_delivery_for_test(crate::runtime::DeliveryRecord {
            payload: json!({ "kind": "digest", "text": "the weekly summary" }),
            idempotency_key: None,
            ..approval_notification_record("delivery-of-another-kind", "unused")
        });
    }
    let to_person = delivery_ids(&console, &person_token()).await;
    let to_admin = delivery_ids(&console, &admin).await;
    console.stop().await;

    let ids = |ids: &[&str]| {
        ids.iter()
            .map(|id| (*id).to_string())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(
        to_person,
        ids(&[
            "delivery-known-origin",
            "delivery-no-origin",
            "delivery-settled-known-origin",
            "delivery-of-another-kind",
        ]),
        "a person who is not an admin"
    );
    assert_eq!(
        to_admin,
        ids(&[
            "delivery-unknown-origin",
            "delivery-private-origin",
            "delivery-known-origin",
            "delivery-no-origin",
            "delivery-settled-unknown-origin",
            "delivery-settled-known-origin",
            "delivery-request-not-held",
            "delivery-of-another-kind",
        ]),
        "an admin"
    );
}

/// The id the gating owner gives the next request that waits for a
/// decision: one evaluation takes the action's id, then the request's, from
/// the owner's one sequence.
async fn next_pending_id(console: &ApprovalConsole) -> String {
    let snapshot = console.owner.lock().await.gating_state_snapshot();
    let epoch = snapshot.owner_epoch.expect("the owner's epoch");
    format!("gate-pending-v2-{epoch}-{:06}", snapshot.next_sequence + 1)
}

/// A member of the Console's own mob under the name `identity`: one every
/// caller shares, or one private to another person.
async fn seat_member(console: &ApprovalConsole, identity: &str, private_to_another_person: bool) {
    let mut labels = BTreeMap::from([("agent_identity".to_string(), identity.to_string())]);
    if private_to_another_person {
        labels.insert(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:dana".to_string(),
        );
    }
    console
        .runtime
        .handle()
        .spawn_spec(
            SpawnMemberSpec::from_wire(
                "worker".to_string(),
                crate::member_comms_id::mob_member_id_str(identity).into_owned(),
                Some("A member of the Console's mob.".into()),
                None,
                None,
            )
            .with_labels(labels),
        )
        .await
        .expect("a member of the Console's mob");
}

/// A member of the rows below that is private to another person.
const PRIVATE_ORIGIN: &str = "frank";

/// A request is judged when the decision is applied, whenever the call came
/// in. A person's decide names the id the next request will get and waits
/// before it takes the gating owner; meanwhile a request is filed under that
/// id. A request of a member the Console was never told about, or of a
/// member private to another person, is then answered as an id that is not
/// pending and stays pending: on a Console that requires sign-in and has no
/// access configuration, and under access rules that reach the member. A
/// request of a member the Console knows is decided, on both.
#[tokio::test]
async fn a_request_filed_while_its_decide_is_on_its_way_is_judged_when_applied() {
    let person = person_token();
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    // Per Console: its name, its access controller, the member its mob
    // seats (and whether that member is private to another person), the
    // origin of the request, and whether the person is shown the request.
    for (name, access, seated, origin, shown) in [
        (
            "origin-filed-meanwhile-no-access",
            None,
            None,
            UNKNOWN_ORIGIN,
            false,
        ),
        (
            "origin-filed-meanwhile-private-no-access",
            None,
            Some((PRIVATE_ORIGIN, true)),
            PRIVATE_ORIGIN,
            false,
        ),
        (
            "origin-filed-meanwhile-known-no-access",
            None,
            Some((KNOWN_ORIGIN, false)),
            KNOWN_ORIGIN,
            true,
        ),
        (
            "origin-filed-meanwhile-rules",
            Some(origin_access()),
            None,
            UNKNOWN_ORIGIN,
            false,
        ),
        (
            "origin-filed-meanwhile-private-rules",
            Some(withholding_access()),
            None,
            PRIVATE_ORIGIN,
            false,
        ),
        (
            "origin-filed-meanwhile-known",
            Some(origin_access()),
            None,
            KNOWN_ORIGIN,
            true,
        ),
    ] {
        let console = ApprovalConsole::start(name, enforced(), detached(), access).await;
        if let Some((identity, private_to_another_person)) = seated {
            seat_member(&console, identity, private_to_another_person).await;
        }
        let expected = next_pending_id(&console).await;
        let hold = super::console_decide_holds::ConsoleDecideHold::install(
            &super::console_decide_holds::before_owner(&expected),
        );
        let decide = tokio::spawn({
            let app = console.app.clone();
            let person = person.clone();
            let expected = expected.clone();
            async move {
                console_rpc_as(
                    &app,
                    Some(&person),
                    DECIDE,
                    json!({ "pending_id": expected, "decision": "approve" }),
                )
                .await
            }
        });
        let reached = tokio::time::timeout(Duration::from_secs(30), hold.reached()).await;
        let pending_id = pending_from(&console, origin).await;
        let list = console_rpc_as(
            &console.app,
            Some(&person),
            "meta/gating/pending",
            json!({}),
        )
        .await;
        let listed = list["result"]["pending"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["pending_id"].as_str() == Some(pending_id.as_str()))
        });
        hold.release();
        let answer = tokio::time::timeout(Duration::from_secs(30), decide).await;
        let still_pending = console.still_pending(&pending_id).await;
        let not_pending = console
            .approve(Some(&person), "gate-pending-never-minted")
            .await;
        console.stop().await;

        reached.expect("the decide reached the point before it takes the gating owner");
        let answer = answer
            .expect("the decide answered")
            .expect("the decide task");
        assert_eq!(pending_id, expected, "{name}: the request got the next id");
        assert!(!not_pending["error"].is_null(), "{not_pending}");
        lines.push(format!(
            "{name}: origin {origin}: the pending list shows the request: {listed}; answered \
             {answer}; still pending: {still_pending}"
        ));
        let as_expected = if shown {
            listed && answer["error"].is_null() && !still_pending
        } else {
            !listed && answer == not_pending && still_pending
        };
        if !as_expected {
            failures.push(name);
        }
    }
    println!("{}", lines.join("\n"));
    assert!(
        failures.is_empty(),
        "judged when the decision is applied: {failures:?}\n{}",
        lines.join("\n")
    );
}

/// Decides of the id the next request will get, sent by eight callers at
/// once as one person while a request of a member the Console was never told
/// about is filed. Returns the id the request got, the id the decides named,
/// how many decides were sent, the first decide that was taken (if any) and
/// whether the request is still pending.
async fn decides_sent_while_a_request_is_filed(
    console: &ApprovalConsole,
    person: &str,
) -> (String, String, u64, Option<Value>, bool) {
    let expected = next_pending_id(console).await;
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut callers = Vec::new();
    for _ in 0..8 {
        let app = console.app.clone();
        let person = person.to_string();
        let expected = expected.clone();
        let stop = stop.clone();
        callers.push(tokio::spawn(async move {
            let mut sent = 0_u64;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let answer = console_rpc_as(
                    &app,
                    Some(&person),
                    DECIDE,
                    json!({ "pending_id": expected, "decision": "approve" }),
                )
                .await;
                sent += 1;
                if answer["error"].is_null() {
                    return (sent, Some(answer));
                }
            }
            (sent, None)
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let pending_id = pending_from(console, UNKNOWN_ORIGIN).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut sent = 0;
    let mut taken = None;
    for caller in callers {
        let (count, answer) = caller.await.expect("a deciding caller");
        sent += count;
        if taken.is_none() {
            taken = answer;
        }
    }
    let still_pending = console.still_pending(&pending_id).await;
    (pending_id, expected, sent, taken, still_pending)
}

/// On a Console that requires sign-in and has no access configuration, no
/// decide sent while a request of a member the Console was never told about
/// is being filed is taken, whenever it was sent: the request stays pending.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn decides_sent_while_a_request_is_filed_take_nothing_with_no_access_configuration() {
    let console = ApprovalConsole::start(
        "origin-filed-concurrently-no-access",
        enforced(),
        detached(),
        None,
    )
    .await;
    let person = person_token();
    let (pending_id, expected, sent, taken, still_pending) =
        decides_sent_while_a_request_is_filed(&console, &person).await;
    console.stop().await;
    println!(
        "no access configuration: decides sent {sent}, taken {}, still pending {still_pending}",
        taken.is_some()
    );
    assert_eq!(pending_id, expected, "the request got the next id");
    assert!(
        taken.is_none() && still_pending,
        "nobody decides it there: taken {taken:?}, still pending {still_pending}"
    );
}

/// The same under access rules that reach the member: only an admin decides
/// the request.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn decides_sent_while_a_request_is_filed_take_nothing_under_access_rules() {
    let console = ApprovalConsole::start(
        "origin-filed-concurrently-rules",
        enforced(),
        detached(),
        Some(origin_access()),
    )
    .await;
    let person = person_token();
    let (pending_id, expected, sent, taken, still_pending) =
        decides_sent_while_a_request_is_filed(&console, &person).await;
    console.stop().await;
    println!(
        "access rules: decides sent {sent}, taken {}, still pending {still_pending}",
        taken.is_some()
    );
    assert_eq!(pending_id, expected, "the request got the next id");
    assert!(
        taken.is_none() && still_pending,
        "only an admin decides it: taken {taken:?}, still pending {still_pending}"
    );
}

/// On a Console that requires sign-in and has no access configuration, the
/// pending list shows a person a request of a member the Console knows, and
/// the person decides it by its id; a request of a member the Console was
/// never told about, or of a member private to another person, is withheld
/// from the person and its decide is answered as an id that is not pending.
#[tokio::test]
async fn a_request_of_a_member_the_console_knows_is_decided_with_no_access_configuration() {
    let console =
        ApprovalConsole::start("origin-known-no-access", enforced(), detached(), None).await;
    seat_member(&console, KNOWN_ORIGIN, false).await;
    seat_member(&console, PRIVATE_ORIGIN, true).await;
    let person = person_token();
    let known = decide_by_id(
        &console,
        "a person, a member the Console knows, no access configuration",
        Some(&person),
        KNOWN_ORIGIN,
    )
    .await;
    let unknown = decide_by_id(
        &console,
        "a person, an origin the Console was never told about, no access configuration",
        Some(&person),
        UNKNOWN_ORIGIN,
    )
    .await;
    let private = decide_by_id(
        &console,
        "a person, a member private to another person, no access configuration",
        Some(&person),
        PRIVATE_ORIGIN,
    )
    .await;
    let not_pending = console
        .approve(Some(&person), "gate-pending-never-minted")
        .await;
    console.stop().await;

    println!("{}\n{}\n{}", known.line(), unknown.line(), private.line());
    assert!(known.listed && known.taken(), "{}", known.line());
    for withheld in [&unknown, &private] {
        assert!(
            !withheld.listed && withheld.refused(),
            "{}",
            withheld.line()
        );
        assert_eq!(withheld.answer, not_pending, "{}", withheld.line());
    }
}

/// The raw answer of one Console RPC call: status, content type and the
/// body's bytes.
async fn raw_rpc(
    app: &Router,
    bearer: Option<&str>,
    method: &str,
    params: Value,
) -> (u16, String, Vec<u8>) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/console/rpc")
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        request = request.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(
                    json!({ "jsonrpc": "2.0", "id": "test", "method": method, "params": params })
                        .to_string(),
                ))
                .expect("console request"),
        )
        .await
        .expect("console response");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("console body")
        .to_vec();
    (status, content_type, body)
}

/// An access configuration with the rules switched off, the admin [`ROOT`],
/// the known member [`KNOWN_ORIGIN`] and `frank`, private to another person.
fn rules_off_access() -> AccessController {
    let access = AccessController::new(crate::access::AccessControlConfig {
        enabled: false,
        admins: vec![ROOT.to_string()],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("controller");
    access.record_agent_attributes(AgentResourceAttributes {
        identity: KNOWN_ORIGIN.to_string(),
        ..Default::default()
    });
    access.record_agent_attributes(AgentResourceAttributes {
        identity: "frank".to_string(),
        labels: BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "person:dana".to_string(),
        )]),
        ..Default::default()
    });
    access
}

/// One Console of the row below: its name, its decisions, its access
/// controller, its callers (a description and a token), the origins its
/// requests are withheld for, and whether it has an admin.
type WithholdingConsole<'a> = (
    &'a str,
    RuntimeDecisionState,
    Option<AccessController>,
    Vec<(&'a str, Option<&'a str>)>,
    Vec<&'a str>,
    bool,
);

/// For every kind of caller and every kind of withheld origin, a decide of
/// the withheld request and a decide of an id that is not pending are
/// answered with the same status, content type and body bytes, whatever the
/// caller sends with the id; the gating owner's state (requests, audit, ids)
/// is the same before and after. An admin is then shown the request and
/// decides it; where there is no admin, the host's own decide takes it.
#[tokio::test]
async fn a_withheld_decide_is_the_absent_id_answer_in_bytes_for_every_caller_kind() {
    const NOT_PENDING: &str = "gate-pending-never-minted";
    let person = person_token();
    let admin = person_token_for(ROOT);
    let shapes = |id: &str| -> Vec<Value> {
        vec![
            json!({ "pending_id": id, "decision": "approve" }),
            json!({ "pending_id": id, "decision": "reject" }),
            json!({ "pending_id": id, "decision": "deny" }),
            json!({ "pending_id": id, "decision": "escalate" }),
            json!({ "pending_id": id, "decision": "approve", "reason": "looks fine" }),
            json!({ "pending_id": id, "decision": "maybe" }),
            json!({ "pending_id": id, "decision": 5 }),
            json!({ "pending_id": id }),
            json!({ "pending_id": format!("  {id}\t"), "decision": "approve" }),
            json!({ "pending_id": id, "decision": "approve", "approver_id": "someone" }),
        ]
    };
    let mut lines = Vec::new();
    let mut differing = Vec::new();
    let mut changed = Vec::new();
    let mut admin_refused = Vec::new();
    let mut host_refused = Vec::new();
    let consoles: Vec<WithholdingConsole<'_>> = vec![
        (
            "origin-bytes-rules-on",
            enforced_admitting(&[ROOT]),
            Some(withholding_access()),
            vec![(
                "a person, rules that reach the member",
                Some(person.as_str()),
            )],
            vec![UNKNOWN_ORIGIN, "erin", "frank"],
            true,
        ),
        (
            "origin-bytes-rules-off",
            enforced_admitting(&[ROOT]),
            Some(rules_off_access()),
            vec![("a person, rules switched off", Some(person.as_str()))],
            vec![UNKNOWN_ORIGIN, "frank"],
            true,
        ),
        (
            "origin-bytes-no-access",
            enforced(),
            None,
            vec![
                ("a person, no access configuration", Some(person.as_str())),
                ("no token, sign-in required", None),
            ],
            vec![UNKNOWN_ORIGIN],
            false,
        ),
        (
            "origin-bytes-open",
            open(),
            Some(withholding_access()),
            vec![
                ("a person who volunteers a token", Some(person.as_str())),
                ("no token, sign-in optional", None),
            ],
            vec![UNKNOWN_ORIGIN, "erin", "frank"],
            false,
        ),
    ];
    for (console_name, decisions, access, callers, origins, has_admin) in consoles {
        let console = ApprovalConsole::start(console_name, decisions, detached(), access).await;
        for origin in origins {
            let pending_id = pending_from(&console, origin).await;
            let before = console.owner.lock().await.gating_state_snapshot();
            for (caller, bearer) in &callers {
                let mut first = None;
                for (withheld, absent) in shapes(&pending_id).into_iter().zip(shapes(NOT_PENDING)) {
                    let of_withheld =
                        raw_rpc(&console.app, *bearer, DECIDE, withheld.clone()).await;
                    let of_absent = raw_rpc(&console.app, *bearer, DECIDE, absent).await;
                    let text = String::from_utf8_lossy(&of_withheld.2).to_string();
                    let refused = of_withheld.0 != 200
                        || serde_json::from_slice::<Value>(&of_withheld.2)
                            .is_ok_and(|body| !body["error"].is_null());
                    if of_withheld != of_absent || !refused {
                        differing.push(format!(
                            "{console_name}, {caller}, origin {origin}, {withheld}: {} {text} \
                             (an id that is not pending: {} {})",
                            of_withheld.0,
                            of_absent.0,
                            String::from_utf8_lossy(&of_absent.2)
                        ));
                    }
                    first.get_or_insert((of_withheld.0, text));
                }
                let (status, text) = first.expect("ten shapes");
                lines.push(format!(
                    "{console_name} | {caller} | origin {origin} | approve: {status} {text}"
                ));
            }
            let after = console.owner.lock().await.gating_state_snapshot();
            if after != before {
                changed.push(format!("{console_name}, origin {origin}"));
            }
            if has_admin {
                let list =
                    console_rpc_as(&console.app, Some(&admin), "meta/gating/pending", json!({}))
                        .await;
                let listed = list["result"]["pending"].as_array().is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|entry| entry["pending_id"].as_str() == Some(pending_id.as_str()))
                });
                let taken = console.approve(Some(&admin), &pending_id).await;
                let still_pending = console.still_pending(&pending_id).await;
                lines.push(format!(
                    "{console_name} | an admin | origin {origin} | listed {listed}, taken {}, \
                     still pending {still_pending}",
                    taken["error"].is_null()
                ));
                if !listed || !taken["error"].is_null() || still_pending {
                    admin_refused.push(format!("{console_name}, origin {origin}: {taken}"));
                }
            } else {
                // The host's own plane: no caller rule.
                let taken = console
                    .owner
                    .lock()
                    .await
                    .decide_gating_action(GatingDecideRequest {
                        pending_id: pending_id.clone(),
                        approver_id: "host-approver".to_string(),
                        decision: GatingDecision::Approve,
                        reason: None,
                    });
                lines.push(format!(
                    "{console_name} | the host's own decide | origin {origin} | taken {}",
                    taken.is_ok()
                ));
                if taken.is_err() {
                    host_refused.push(format!("{console_name}, origin {origin}: {taken:?}"));
                }
            }
        }
        console.stop().await;
    }
    println!("{}", lines.join("\n"));
    assert!(
        differing.is_empty(),
        "withheld and absent answers differ:\n{}",
        differing.join("\n")
    );
    assert!(changed.is_empty(), "the owner's state changed: {changed:?}");
    assert!(admin_refused.is_empty(), "an admin: {admin_refused:?}");
    assert!(host_refused.is_empty(), "the host: {host_refused:?}");
}

/// A request whose origin the Console learns of after it was filed is then
/// shown to a person who may view it, and that person's decide is taken.
#[tokio::test]
async fn a_request_whose_origin_the_console_learns_of_later_is_then_shown_and_decided() {
    let access = origin_access();
    let console = ApprovalConsole::start(
        "origin-learned-later",
        enforced(),
        detached(),
        Some(access.clone()),
    )
    .await;
    let person = person_token();
    let pending_id = pending_from(&console, "gwen").await;
    let listed = |answer: &Value| {
        answer["result"]["pending"]
            .as_array()
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|entry| entry["pending_id"].as_str() == Some(pending_id.as_str()))
            })
    };
    let before = console_rpc_as(
        &console.app,
        Some(&person),
        "meta/gating/pending",
        json!({}),
    )
    .await;
    let refused = console.approve(Some(&person), &pending_id).await;
    let pending_after_refusal = console.still_pending(&pending_id).await;
    access.record_agent_attributes(AgentResourceAttributes {
        identity: "gwen".to_string(),
        ..Default::default()
    });
    let after = console_rpc_as(
        &console.app,
        Some(&person),
        "meta/gating/pending",
        json!({}),
    )
    .await;
    let taken = console.approve(Some(&person), &pending_id).await;
    let still_pending = console.still_pending(&pending_id).await;
    console.stop().await;

    assert!(!listed(&before), "{before}");
    assert!(!refused["error"].is_null(), "{refused}");
    assert!(pending_after_refusal);
    assert!(listed(&after), "{after}");
    assert!(taken["error"].is_null(), "{taken}");
    assert!(!still_pending);
}

/// A page of the Console's delivery history is cut to its limit after the
/// records withheld from the caller are left out, so its size tells nothing
/// of them: a person's page holds the newest records that person may read,
/// as if the withheld ones had never been kept, and an admin's page holds
/// the newest records.
#[tokio::test]
async fn a_delivery_history_page_is_cut_after_withheld_records_are_left_out() {
    let console = ApprovalConsole::start(
        "delivery-history-page",
        enforced_admitting(&[ROOT]),
        detached(),
        Some(rules_off_access()),
    )
    .await;
    let records = [
        ("delivery-oldest-shown", KNOWN_ORIGIN),
        ("delivery-older-withheld", UNKNOWN_ORIGIN),
        ("delivery-newer-shown", KNOWN_ORIGIN),
        ("delivery-newest-withheld", UNKNOWN_ORIGIN),
    ];
    for (delivery_id, origin) in records {
        let pending_id = pending_from(&console, origin).await;
        console
            .owner
            .lock()
            .await
            .record_delivery_for_test(approval_notification_record(delivery_id, &pending_id));
    }
    let person = person_token();
    let admin = person_token_for(ROOT);
    let mut pages = Vec::new();
    for (bearer, limit) in [
        (&person, 1_u64),
        (&person, 2),
        (&person, 3),
        (&admin, 1),
        (&admin, 2),
        (&admin, 4),
    ] {
        let answer = console_rpc_as(
            &console.app,
            Some(bearer.as_str()),
            "meta/delivery/history",
            json!({ "limit": limit }),
        )
        .await;
        let ids = answer["result"]["deliveries"]
            .as_array()
            .unwrap_or_else(|| panic!("a delivery history: {answer}"))
            .iter()
            .filter_map(|record| record["delivery_id"].as_str().map(str::to_string))
            .collect::<Vec<_>>();
        pages.push(ids);
    }
    console.stop().await;

    println!("{pages:?}");
    assert_eq!(
        pages[0],
        ["delivery-newer-shown"],
        "a person, a page of one"
    );
    assert_eq!(
        pages[1],
        ["delivery-oldest-shown", "delivery-newer-shown"],
        "a person, a page of two"
    );
    assert_eq!(pages[2], pages[1], "a person, a page of three");
    assert_eq!(
        pages[3],
        ["delivery-newest-withheld"],
        "an admin, a page of one"
    );
    assert_eq!(
        pages[4],
        ["delivery-newer-shown", "delivery-newest-withheld"],
        "an admin, a page of two"
    );
    assert_eq!(
        pages[5],
        [
            "delivery-oldest-shown",
            "delivery-older-withheld",
            "delivery-newer-shown",
            "delivery-newest-withheld",
        ],
        "an admin, a page of four"
    );
}
