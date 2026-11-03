//! Seeded-memory reference gateway for the console memory e2e
//! (`console/memory-e2e.cjs`).
//!
//! Boots the same library-mode runtime as `library_mode_reference`, but
//! wires a bundled sqlite agent-memory store seeded through the real store
//! APIs (mirroring `tests/access_control.rs::seeded_memory_store`): active
//! records across identity/mob/realm/operator scopes, a three-record
//! supersede chain, a quarantined LLM write with a secret-shaped reason,
//! dream audit rows, injection-ledger rows, two gated promotions parked in
//! the queue, and a spread of `memory.*` events on the console timeline.
//!
//! Environment:
//! - `META_MEMORY_E2E_ADDR`   listen address (default `127.0.0.1:3230`)
//! - `META_MEMORY_E2E_STATE`  persistent-state dir for the sqlite store
//!   (default: fresh dir under the system temp dir)
//! - `META_MEMORY_E2E_ACCESS` `open` (default, no access controller —
//!   the lane browser-e2e's reference proof runs in), `reader` (memory
//!   read grants but no quarantine review), `partial` (agent+mob reads,
//!   no operator.memory.read — operator scope must render "no grant"),
//!   `scoped` (agent.memory.read limited to one agent — panel/dreams
//!   denies, DREAMS tile lands "no-grant"), or `none` (console visible,
//!   no memory grants — `nav:memory` must not render)
//! - `META_MEMORY_E2E_SEED_ONLY=1` seed + verify readback + print the
//!   summary, then exit without serving (fixture self-test lane)
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::too_many_lines
)]
use std::sync::Arc;
use std::time::Duration;

use meerkat_client::TestClient;
use meerkat_meta::memory::events::MemoryTimelineEvent;
use meerkat_meta::memory::records::RecordStatus;
use meerkat_meta::memory::records::{
    InjectionLogEntry, InjectionSurface, MemoryAuthor, MemoryKind,
};
use meerkat_meta::memory::staged::StagedBatchKind;
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessEffect, AccessRule, AgentMemoryProvider,
    AuthPolicy, BigQueryNaming, ConsolePolicy, DiscoverySpec, MemoryPanelStore, MemoryScope,
    MetaConfig, NewMemoryRecord, PreSpawnData, RuntimeDecisionInputs, RuntimeOpsPolicy,
    SqliteAgentMemoryStore, StagedMemoryStore, StagedMutationBatch, StagedOp, StewardStore,
    TaintableStore, TrustTier, TrustedOidcRuntimeConfig, UnifiedRuntime,
    build_runtime_decision_state,
};
use meerkat_mob::ids::AgentIdentity as MeerkatId;
use meerkat_mob::{MobDefinition, ProfileName, SpawnMemberSpec};
use serde_json::json;

const REALM: &str = "default";
const MOB: &str = "memory-e2e-mob";

const MOB_TOML: &str = r#"
[mob]
id = "memory-e2e-mob"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#;

/// Quarantine every agent-authored write with a §10.4-shaped reason.
/// Operator/steward writes pass so the rest of the seed lands active —
/// the real secret detector *refuses* secret bodies at the staged
/// chokepoint, so a quarantined-with-secret-reason record can only be
/// produced through the write gate, exactly like the access-control test.
struct QuarantineAgentWrites;

impl meerkat_meta::memory::taint::LlmWriteGate for QuarantineAgentWrites {
    fn quarantine_reason(
        &self,
        author: &MemoryAuthor,
        _kind: StagedBatchKind,
        _evidence: &[meerkat_meta::memory::records::EvidenceRef],
    ) -> Option<String> {
        matches!(author, MemoryAuthor::Agent { .. })
            .then(|| "matches the 'credential-assignment' secret pattern class (§10.4)".to_string())
    }
}

/// Everything the seed produced that the e2e (and the readback check)
/// needs to reference by id.
struct SeededIds {
    chain_tip: String,
    quarantined: String,
    delivery_fact: String,
    dream_run: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let state_dir = match std::env::var("META_MEMORY_E2E_STATE") {
        Ok(path) => std::path::PathBuf::from(path),
        Err(_) => {
            let dir = std::env::temp_dir().join(format!("meta-memory-e2e-{}", std::process::id()));
            std::fs::create_dir_all(&dir)?;
            dir
        }
    };
    // Mirror the gateway layout: the store lives under
    // `<persistent_state>/agent-memory`.
    let store_path = state_dir.join("agent-memory");
    std::fs::create_dir_all(&store_path)?;

    let store = SqliteAgentMemoryStore::open(&store_path)?;
    let ids = seed_store(&store).await;
    verify_seeded_store(&store, &ids).await;

    if std::env::var("META_MEMORY_E2E_SEED_ONLY").ok().as_deref() == Some("1") {
        println!("memory e2e fixture: seed-only mode, exiting before serve");
        return Ok(());
    }

    let definition = MobDefinition::from_toml(MOB_TOML)
        .map_err(|e| std::io::Error::other(format!("bad mob definition: {e}")))?;
    let mut runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .default_llm_client(Arc::new(TestClient::default()))
            .module_config(MetaConfig {
                modules: vec![],
                discovery: DiscoverySpec {
                    namespace: "memory-e2e".to_string(),
                    modules: vec![],
                },
                pre_spawn: Vec::<PreSpawnData>::new(),
            })
            .timeout(Duration::from_secs(5))
            .build(),
    )
    .await?;

    // Roster identities match the seeded identity scopes so the sidebar,
    // memory groups, and injection rows all reference the same names.
    runtime
        .reconcile(vec![
            SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("router")),
            SpawnMemberSpec::new(ProfileName::from("lead"), MeerkatId::from("delivery")),
        ])
        .await?;

    let access_mode =
        std::env::var("META_MEMORY_E2E_ACCESS").unwrap_or_else(|_| "open".to_string());
    if let Some(controller) = access_controller(&access_mode) {
        runtime.set_access_controller(controller);
    }
    println!("memory e2e fixture: access mode '{access_mode}'");

    runtime.set_memory_panel_store(Arc::new(store.clone()));
    emit_timeline_events(&runtime, &ids);

    let decisions = build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "memory_e2e_dataset".to_string(),
            table: "memory_e2e_table".to_string(),
        },
        trusted_meta_toml: r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
        .to_string(),
        auth: AuthPolicy::default(),
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .map_err(|err| std::io::Error::other(format!("failed to build console decisions: {err:?}")))?;

    let listen_addr =
        std::env::var("META_MEMORY_E2E_ADDR").unwrap_or_else(|_| "127.0.0.1:3230".to_string());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    let bound = listener.local_addr()?;
    println!("memory e2e fixture listening on http://{bound}");
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");

    let run_report = runtime
        .run(listener, decisions, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    run_report
        .shutdown
        .mob_stop
        .map_err(|err| std::io::Error::other(format!("failed to stop mob runtime: {err}")))?;
    run_report.serve_result?;
    Ok(())
}

/// Seed through the real store APIs — same shapes as
/// `tests/access_control.rs::seeded_memory_store`, widened to the volumes
/// the e2e flows page through.
async fn seed_store(store: &SqliteAgentMemoryStore) -> SeededIds {
    let record = |title: &str, body: &str| NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: format!("{title} — seeded for the memory console e2e"),
        body: body.to_string(),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    };
    let identity_scope = |identity: &str| MemoryScope::Identity {
        realm: REALM.to_string(),
        identity: identity.to_string(),
    };
    let mob_scope = MemoryScope::Mob {
        realm: REALM.to_string(),
        mob: MOB.to_string(),
    };

    // 60 filler facts with OLD explicit timestamps (staged batch — the only
    // write path that accepts created_at_ms). They push the store past the
    // panel's 50-row default page so `memory-load-more` renders, while the
    // interesting records below (wall-clock timestamps) stay in the newest
    // page. Application author: never gated, never counted as a dream run.
    let filler_ops: Vec<StagedOp> = (1..=60u64)
        .map(|index| StagedOp::Create {
            id: None,
            scope: identity_scope("router"),
            record: record(
                &format!("Router ops note {index:02}"),
                &format!("Routine operational note number {index} for paging."),
            ),
            trust: TrustTier::AgentObserved,
            derived_from: Vec::new(),
            rationale: Some("seed filler".to_string()),
            created_at_ms: Some(1_000 + index),
            updated_at_ms: Some(1_000 + index),
        })
        .collect();
    let filler_token = store
        .stage(StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REALM.to_string(),
            author: MemoryAuthor::Application,
            ops: filler_ops,
        })
        .await
        .expect("stage filler batch");
    store
        .commit(filler_token)
        .await
        .expect("commit filler batch");

    // Three-record supersede chain on identity:router.
    let chain_root = store
        .remember_authored(
            &identity_scope("router"),
            record(
                "Router deploy cadence",
                "Deploys happen ad hoc whenever a fix lands.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("chain root");
    let chain_mid = store
        .supersede_authored(
            &identity_scope("router"),
            &chain_root.memory_id,
            record(
                "Router deploy cadence",
                "Deploys moved to a weekly Tuesday window.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("chain mid");
    // The tip carries an evidence ref naming a session that is NOT in the
    // console timeline — the Biography click-through must take the degrade
    // path (label-only fallback), which the e2e asserts.
    let mut tip_record = record(
        "Router deploy cadence",
        "Deploys are gated on the release train: Tuesdays, after CI green.",
    );
    tip_record.evidence = vec![meerkat_meta::memory::records::EvidenceRef {
        session_id: "sess-router-archived-1".to_string(),
        generation: 2,
        revision: None,
        range: Some((3, 9)),
    }];
    let chain_tip = store
        .supersede_authored(
            &identity_scope("router"),
            &chain_mid.memory_id,
            tip_record,
            MemoryAuthor::Operator,
        )
        .await
        .expect("chain tip");

    // Additional active facts across scopes.
    for (title, body) in [
        (
            "Router escalation contact",
            "Escalations for routing incidents go to the on-call channel first.",
        ),
        (
            "Router retry budget",
            "Retries are capped at 2 with 25ms backoff before dead-lettering.",
        ),
    ] {
        store
            .remember_authored(
                &identity_scope("router"),
                record(title, body),
                MemoryAuthor::Operator,
            )
            .await
            .expect("router fact");
    }
    let delivery_fact = store
        .remember_authored(
            &identity_scope("delivery"),
            record(
                "Delivery sink preference",
                "The delivery sink prefers batched flushes over per-message writes.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("delivery fact");
    store
        .remember_authored(
            &identity_scope("delivery"),
            record(
                "Delivery quiet hours",
                "No non-urgent delivery notifications between 22:00 and 07:00.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("delivery fact 2");
    for (title, body) in [
        (
            "Mob review convention",
            "Cross-agent changes need a second reviewer from another domain.",
        ),
        (
            "Mob incident channel",
            "Incidents coordinate in the shared mob channel, not DMs.",
        ),
    ] {
        store
            .remember_authored(&mob_scope, record(title, body), MemoryAuthor::Operator)
            .await
            .expect("mob fact");
    }
    store
        .remember_authored(
            &MemoryScope::Realm {
                realm: REALM.to_string(),
            },
            record(
                "Realm data residency",
                "All realm data stays in the EU region unless a ticket says otherwise.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("realm fact");
    store
        .remember_authored(
            &MemoryScope::Operator {
                realm: REALM.to_string(),
                operator: "op-luka".to_string(),
            },
            record(
                "Operator briefing preference",
                "Prefers terse morning summaries with links over long prose.",
            ),
            MemoryAuthor::Operator,
        )
        .await
        .expect("operator fact");

    // Quarantined agent write (secret-shaped reason) through the gate.
    store.set_llm_write_gate(Arc::new(QuarantineAgentWrites));
    let quarantined = store
        .remember_authored(
            &identity_scope("router"),
            record(
                "Router upstream credential",
                "The upstream accepts a static token configured at deploy time.",
            ),
            MemoryAuthor::Agent {
                identity: "router".to_string(),
            },
        )
        .await
        .expect("quarantined record");
    assert!(
        matches!(
            quarantined.status,
            meerkat_meta::memory::records::RecordStatus::Quarantined { .. }
        ),
        "seed record should land quarantined: {quarantined:?}"
    );

    // Injection-ledger rows: build-surface for the chain tip, turn-surface
    // for the delivery fact.
    store
        .log_injections(
            REALM,
            &[
                InjectionLogEntry {
                    record_id: chain_tip.memory_id.clone(),
                    identity: "router".to_string(),
                    session_key: Some("sess-router-1".to_string()),
                    surface: InjectionSurface::Build,
                    at_ms: 1_000,
                },
                InjectionLogEntry {
                    record_id: delivery_fact.memory_id.clone(),
                    identity: "delivery".to_string(),
                    session_key: Some("sess-delivery-1".to_string()),
                    surface: InjectionSurface::Turn,
                    at_ms: 2_000,
                },
            ],
        )
        .await
        .expect("injection rows");

    // One committed steward dream (two creates) → audit rows.
    let dream_run = "run-dream-e2e-1".to_string();
    let token = store
        .stage(StagedMutationBatch {
            kind: StagedBatchKind::FreshWrite,
            realm: REALM.to_string(),
            author: MemoryAuthor::Steward {
                run_id: dream_run.clone(),
            },
            ops: vec![
                StagedOp::Create {
                    id: None,
                    scope: mob_scope.clone(),
                    record: record(
                        "Consolidated deploy learnings",
                        "Three deploy retros distilled: gate on CI, announce in channel.",
                    ),
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: Some("consolidated during dream".to_string()),
                    created_at_ms: None,
                    updated_at_ms: None,
                },
                StagedOp::Create {
                    id: None,
                    scope: mob_scope.clone(),
                    record: record(
                        "Consolidated escalation map",
                        "Escalation paths across router/delivery merged into one map.",
                    ),
                    trust: TrustTier::AgentObserved,
                    derived_from: Vec::new(),
                    rationale: Some("consolidated during dream".to_string()),
                    created_at_ms: None,
                    updated_at_ms: None,
                },
            ],
        })
        .await
        .expect("stage dream batch");
    store.commit(token).await.expect("commit dream batch");

    // Two gated promotions parked in the queue (staged, never committed).
    let stage_promotion = |scope: MemoryScope, title: &str| {
        let store = store.clone();
        let record = record(title, "Pending gated promotion payload.");
        async move {
            store
                .stage(StagedMutationBatch {
                    kind: StagedBatchKind::FreshWrite,
                    realm: REALM.to_string(),
                    author: MemoryAuthor::Steward {
                        run_id: "run-gate-e2e-1".to_string(),
                    },
                    ops: vec![StagedOp::Create {
                        id: None,
                        scope,
                        record,
                        trust: TrustTier::AgentObserved,
                        derived_from: Vec::new(),
                        rationale: Some("gated promotion".to_string()),
                        created_at_ms: None,
                        updated_at_ms: None,
                    }],
                })
                .await
                .expect("stage promotion batch")
        }
    };
    let mob_stage = stage_promotion(mob_scope.clone(), "Promoted mob claim").await;
    store
        .record_pending_promotion(
            REALM,
            meerkat_meta::memory::PendingPromotion {
                pending_id: "gate-mob-promotion".to_string(),
                stage_token: mob_stage.token,
                record_id: quarantined.memory_id.clone(),
                scope_kind: "mob".to_string(),
                scope_key: MOB.to_string(),
                rationale: Some("steward: mob-wide convention".to_string()),
                status: "pending".to_string(),
                created_at_ms: now_ms(),
            },
        )
        .await
        .expect("mob promotion row");
    let delivery_stage =
        stage_promotion(identity_scope("delivery"), "Promoted delivery claim").await;
    store
        .record_pending_promotion(
            REALM,
            meerkat_meta::memory::PendingPromotion {
                pending_id: "gate-delivery-promotion".to_string(),
                stage_token: delivery_stage.token,
                record_id: quarantined.memory_id.clone(),
                scope_kind: "identity".to_string(),
                scope_key: "delivery".to_string(),
                rationale: Some("steward: delivery personal fact".to_string()),
                status: "pending".to_string(),
                created_at_ms: now_ms(),
            },
        )
        .await
        .expect("delivery promotion row");

    SeededIds {
        chain_tip: chain_tip.memory_id,
        quarantined: quarantined.memory_id,
        delivery_fact: delivery_fact.memory_id,
        dream_run,
    }
}

/// Read the seed back through the same panel-facing store APIs and fail
/// loudly if any row is missing — the e2e must never run against a
/// silently half-seeded store.
async fn verify_seeded_store(store: &SqliteAgentMemoryStore, ids: &SeededIds) {
    let page = store
        .records_page(REALM, None, None, None, 500, None)
        .await
        .expect("records page");
    let active = page
        .records
        .iter()
        .filter(|record| matches!(record.status, RecordStatus::Active))
        .count();
    let superseded = page
        .records
        .iter()
        .filter(|record| matches!(record.status, RecordStatus::Superseded { .. }))
        .count();
    let quarantined_count = page
        .records
        .iter()
        .filter(|record| matches!(record.status, RecordStatus::Quarantined { .. }))
        .count();
    assert_eq!(page.records.len(), 74, "total seeded records");
    assert_eq!(active, 71, "active records");
    assert_eq!(superseded, 2, "superseded chain records");
    assert_eq!(quarantined_count, 1, "quarantined record");

    let quarantined = store
        .quarantined_records(REALM, 10)
        .await
        .expect("quarantined records");
    assert_eq!(quarantined.len(), 1);
    assert_eq!(quarantined[0].id, ids.quarantined);

    let promotions = store
        .pending_promotions(REALM)
        .await
        .expect("pending promotions");
    assert_eq!(promotions.len(), 2, "pending promotions");

    let dreams = store.dream_history(REALM, 10).await.expect("dream history");
    assert!(
        dreams.iter().any(|run| run.run_id == ids.dream_run),
        "dream audit rows missing run {}: {dreams:?}",
        ids.dream_run
    );
    // The filler batch (Application author) and the staged-only promotion
    // batches must not surface as dream runs.
    assert_eq!(dreams.len(), 1, "exactly one dream run: {dreams:?}");

    let tip_injections = store
        .injection_log_for_record(REALM, &ids.chain_tip, 10)
        .await
        .expect("tip injections");
    assert_eq!(tip_injections.len(), 1, "chain-tip injection row");
    let delivery_injections = store
        .injection_log_for_record(REALM, &ids.delivery_fact, 10)
        .await
        .expect("delivery injections");
    assert_eq!(delivery_injections.len(), 1, "delivery injection row");

    println!(
        "memory e2e fixture seeded: {} records ({active} active, {superseded} superseded, \
         {quarantined_count} quarantined), {} pending promotions, dream run '{}', 2 injection rows",
        page.records.len(),
        promotions.len(),
        ids.dream_run,
    );
}

/// Project a representative spread of `memory.*` events onto the console
/// timeline: the signal rail, verdict strip, and pivot flows all read
/// these.
fn emit_timeline_events(runtime: &UnifiedRuntime, ids: &SeededIds) {
    let sink = runtime.memory_event_sink();
    for event in [
        MemoryTimelineEvent::DreamStarted {
            realm: REALM.to_string(),
            run_id: ids.dream_run.clone(),
        },
        MemoryTimelineEvent::DreamCompleted {
            realm: REALM.to_string(),
            run_id: ids.dream_run.clone(),
            ops_committed: 2,
            detail: json!({ "phase": "consolidation", "verdicts": { "release": 0 } }),
        },
        MemoryTimelineEvent::QuarantinedWrite {
            realm: REALM.to_string(),
            author: "agent router".to_string(),
            reason: "matches the 'credential-assignment' secret pattern class (§10.4)".to_string(),
        },
        MemoryTimelineEvent::QuarantineVerdict {
            realm: REALM.to_string(),
            record_id: ids.quarantined.clone(),
            verdict: "unverifiable".to_string(),
            rationale: Some("no evidence span reaches the claimed credential".to_string()),
            successor_id: None,
        },
        MemoryTimelineEvent::QuarantineReleaseBlocked {
            realm: REALM.to_string(),
            record_id: ids.quarantined.clone(),
            verdict: "release".to_string(),
            class: "credential-assignment".to_string(),
        },
        MemoryTimelineEvent::PromotionPendingGate {
            realm: REALM.to_string(),
            pending_id: "gate-mob-promotion".to_string(),
            record_id: ids.quarantined.clone(),
            scope_kind: "mob".to_string(),
            scope_key: MOB.to_string(),
        },
        MemoryTimelineEvent::ConflictSignal {
            realm: REALM.to_string(),
            entity: "router".to_string(),
            topic: "deploy cadence".to_string(),
            reason: "weekly window contradicts ad-hoc deploy claim".to_string(),
        },
        MemoryTimelineEvent::TaintTransition {
            identity: Some("router".to_string()),
            session_key: "sess-router-1".to_string(),
            kind: "tainted".to_string(),
            source: "web_search".to_string(),
        },
    ] {
        sink.emit(event);
    }
}

/// `open`: no controller (everything allowed — matches how browser-e2e's
/// reference proof runs). `reader`: console + memory reads for everyone,
/// but no `memory.quarantine.review`. `none`: console for everyone, zero
/// memory grants — the memory nav entry must not render.
fn access_controller(mode: &str) -> Option<AccessController> {
    let everyone = |id: &str, actions: &[&str]| AccessRule {
        id: id.to_string(),
        actions: actions
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        ..AccessRule::default()
    };
    let mut rules = vec![
        everyone("everyone-views-agents", &["agent.view"]),
        everyone("everyone-observes-mob", &["mob.observe"]),
    ];
    match mode {
        "open" => return None,
        "reader" => rules.push(everyone(
            "everyone-reads-memory",
            &[
                "agent.memory.read",
                "mob.memory.read",
                "operator.memory.read",
            ],
        )),
        // Unscoped agent/mob reads but NO operator.memory.read: the panel's
        // operator-scope probes and the scope=operator filter hit the entry
        // gate (-32030) and must render "no grant", never the empty-store
        // copy or leaked rows.
        "partial" => rules.push(everyone(
            "everyone-reads-agent-and-mob-memory",
            &["agent.memory.read", "mob.memory.read"],
        )),
        // agent.memory.read scoped to a single agent: the unscoped listing
        // row-filters fine (rows still render), but panel/dreams requires
        // the UNSCOPED read grant and denies — the DREAMS verdict tile must
        // land "no-grant".
        "scoped" => rules.push(AccessRule {
            id: "router-only-memory-read".to_string(),
            actions: vec!["agent.memory.read".to_string()],
            agents: vec!["router".to_string()],
            ..AccessRule::default()
        }),
        // The explicit deny both expresses intent and opts out of the
        // §10.3 memory-naive compat rewrite ("read rides view"), which
        // would otherwise extend the agent.view rule with
        // agent.memory.read on a config that mentions no memory action.
        "none" => rules.push(AccessRule {
            id: "no-memory-access".to_string(),
            effect: AccessEffect::Deny,
            actions: vec![
                "agent.memory.read".to_string(),
                "mob.memory.read".to_string(),
                "operator.memory.read".to_string(),
                "memory.quarantine.review".to_string(),
            ],
            ..AccessRule::default()
        }),
        other => panic!("unknown META_MEMORY_E2E_ACCESS mode: {other}"),
    }
    Some(
        AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["root@memory-e2e.test".to_string()],
            rules,
            ..AccessControlConfig::default()
        })
        .expect("access controller"),
    )
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}
