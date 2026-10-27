//! Session-compaction wiring on Meta's built session path.
//!
//! # Why this file exists
//!
//! A production Meta deployment sent 616_000 input tokens to produce a
//! five-token reply, with turn latency scaling from 3.4s at 13 messages to
//! 400s at 466 messages. The reported diagnosis was "no compactor is
//! installed anywhere in meta's session-build path".
//!
//! That diagnosis is wrong, and nothing in the repo could prove it either
//! way — hence this file. Meerkat's `AgentFactory::build_agent` always
//! installs a `meerkat_session::DefaultCompactor` (the `session-compaction`
//! feature is unified on through `meerkat-mob`'s dependency declaration), and
//! Meta routes every session build through it. The real defect is the
//! *trigger*: with no host policy, meerkat replaces its documented `100_000`
//! default with `context_window * 4 / 5`, which on a million-token model is
//! `840_000` tokens — so 616_000 legitimately sails through.
//!
//! These tests pin both halves:
//!
//! * `built_member_session_path_carries_a_compactor` fails if the compactor
//!   ever stops being installed (feature drift, or a Meta path that stops
//!   routing through `AgentFactory`).
//! * `inherited_policy_lets_a_200k_token_turn_through` pins the inherited
//!   model-aware trigger that made the knob look dead.
//! * `host_compaction_policy_pins_the_trigger` proves
//!   `UnifiedRuntimeBuilder::compaction()` reaches the compactor the member's
//!   session is actually built with.

#![allow(clippy::expect_used, clippy::panic, clippy::uninlined_format_args)]

use std::sync::Arc;
use std::time::Duration;

use meerkat_client::{LlmDoneOutcome, LlmEvent, TestClient};
use meerkat_core::types::{ContentInput, HandlingMode};
use meerkat_core::{AgentEvent, StopReason, Usage};
use meerkat_meta::{MemberTurnOptions, UnifiedRuntime};
use meerkat_mob::{MobDefinition, SpawnMemberSpec};

/// The one definition of the normalized-provider-accounting contract every
/// Meta LLM double must satisfy under meerkat 0.8.22. See the module docs.
#[path = "support/llm_usage.rs"]
mod llm_usage;

/// A catalogued million-token model, so the inherited (model-aware) trigger
/// is the production one: `context_window * 4 / 5`.
const FIXTURE_MODEL: &str = "gpt-5.5";
/// Larger than four fifths of any catalogued context window, so a turn
/// reporting it must compact under ANY inherited threshold.
const ALWAYS_OVER_THRESHOLD: u64 = 10_000_000;
/// Enormous by every practical measure and still under four fifths of a
/// million-token window — the exact production condition.
const HUGE_BUT_UNDER_INHERITED_THRESHOLD: u64 = 200_000;
/// Half the fixture turn's reported tokens — structurally under it, so
/// declaring it flips the same turn from "never compacts" to "compacts".
const PINNED_THRESHOLD: u64 = HUGE_BUT_UNDER_INHERITED_THRESHOLD / 2;

const MEMBER_ALIAS: &str = "compaction-worker";
const TURN_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the event drain waits for a straggler before concluding the turn
/// produced no further events.
const DRAIN_QUIET_PERIOD: Duration = Duration::from_millis(500);

/// A deterministic client that reports a fixed provider input-token count.
///
/// Provider-reported `input_tokens` is the signal meerkat's `DefaultCompactor`
/// prefers, so driving it directly exercises the real trigger without a
/// multi-megabyte fixture transcript. `TestClient` declares `Provider::Other`,
/// which the agent factory accepts against any catalogued model.
///
/// 0.8.22: `UsageUpdate` now carries a `TurnUsage`, and the agent checks its
/// accounting identity against the ADAPTER's provider/model, not the raw
/// client's - which differ here, since `TestClient` declares `Provider::Other`
/// while the adapter is bound to the canonical identity the catalog resolves
/// for [`FIXTURE_MODEL`]. Declaring `Provider::Other` in the accounting would
/// fail every turn with `normalized_provider_accounting_identity_mismatch`, so
/// the shared helper resolves the accounting provider from the MODEL through
/// the same catalog authority `AgentFactory` used, taking the client's own
/// declaration only as the uncatalogued fallback.
///
/// Note also that `TestClient::new` (unlike `TestClient::default`) leaves
/// `synthesize_usage` OFF, so nothing backfills a missing `UsageUpdate` here:
/// the event below is the only accounting this fixture will ever report.
/// `host_declared` is the honest convention for it - an embedded test client
/// asserting its own inclusive input total, which is exactly what the trigger
/// reads back as `last_input_tokens`.
fn usage_reporting_client(input_tokens: u64) -> Arc<TestClient> {
    Arc::new(TestClient::new(vec![
        LlmEvent::TextDelta {
            delta: "ok".to_string(),
            meta: None,
        },
        LlmEvent::UsageUpdate {
            usage: llm_usage::turn_usage(
                FIXTURE_MODEL,
                meerkat_core::Provider::Other,
                Usage {
                    input_tokens,
                    output_tokens: 1,
                    ..Default::default()
                },
            ),
        },
        LlmEvent::Done {
            outcome: LlmDoneOutcome::success(StopReason::EndTurn),
        },
    ]))
}

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "{FIXTURE_MODEL}"
runtime_mode = "turn_driven"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
    .expect("compaction fixture definition parses")
}

/// Drive two real member turns and report whether compaction fired.
///
/// Two turns are the minimum the contract allows: meerkat never compacts on a
/// session's first LLM boundary, and `last_input_tokens` is only populated
/// once a response has been observed.
async fn compaction_fired(runtime: &UnifiedRuntime) -> bool {
    let (tx, mut rx) = tokio::sync::mpsc::channel(2048);
    for prompt in ["first", "second"] {
        let admission = runtime
            .start_member_turn(
                MEMBER_ALIAS,
                ContentInput::Text(prompt.to_string()),
                HandlingMode::Queue,
                MemberTurnOptions::new(),
                Some(tx.clone()),
            )
            .await
            .unwrap_or_else(|error| panic!("turn '{prompt}' must be admitted: {error}"));
        tokio::time::timeout(TURN_TIMEOUT, admission.turn.wait())
            .await
            .unwrap_or_else(|error| panic!("turn '{prompt}' timed out: {error}"))
            .unwrap_or_else(|error| panic!("turn '{prompt}' failed: {error}"));
    }
    drop(tx);

    // Bounded drain rather than `try_recv`: nonterminal events are forwarded
    // live, and the runtime may hold its own sender clone, so neither "the
    // channel is closed" nor "the channel is empty right now" is a sound
    // stopping condition on its own.
    let mut fired = false;
    while let Ok(Some(envelope)) = tokio::time::timeout(DRAIN_QUIET_PERIOD, rx.recv()).await {
        if matches!(envelope.payload, AgentEvent::CompactionStarted { .. }) {
            fired = true;
        }
    }
    fired
}

/// Build a runtime whose members answer with `usage_reporting_client`, and
/// optionally declare a host compaction policy.
async fn runtime_with(
    mob_id: &str,
    input_tokens: u64,
    compaction: Option<meerkat_core::config::CompactionRuntimeConfig>,
) -> UnifiedRuntime {
    let mut builder = UnifiedRuntime::builder()
        .definition(definition(mob_id))
        .default_llm_client(usage_reporting_client(input_tokens))
        .max_sessions(4);
    if let Some(policy) = compaction {
        builder = builder.compaction(policy);
    }
    let runtime = Box::pin(builder.build())
        .await
        .unwrap_or_else(|error| panic!("runtime builds: {error}"));
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            MEMBER_ALIAS.to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("member spawns: {error}"));
    runtime
}

fn pinned_policy(threshold: u64) -> meerkat_core::config::CompactionRuntimeConfig {
    meerkat_core::config::CompactionRuntimeConfig {
        auto_compact_threshold: threshold,
        auto_compact_threshold_explicit: true,
        ..Default::default()
    }
}

/// The load-bearing assertion: the session every Meta member turn runs on
/// carries a compactor. If `session-compaction` ever stops being unified on,
/// or a Meta path stops routing through `AgentFactory::build_agent`,
/// `should_compact` is never consulted and this fails.
#[tokio::test]
async fn built_member_session_path_carries_a_compactor() {
    let runtime = runtime_with("compactor-installed", ALWAYS_OVER_THRESHOLD, None).await;
    assert!(
        compaction_fired(&runtime).await,
        "a turn reporting more input tokens than any plausible threshold must \
         compact; no compactor is installed on the built session path",
    );
}

/// The other half of the production diagnosis: with no host policy the trigger
/// is `context_window * 4 / 5`, so a transcript that is enormous by every
/// practical measure still sails through. This is why `auto_compact_threshold`
/// reads as dead config in a Meta deployment even though a compactor is
/// present.
#[tokio::test]
async fn inherited_policy_lets_a_200k_token_turn_through() {
    let runtime = runtime_with(
        "compactor-inherited",
        HUGE_BUT_UNDER_INHERITED_THRESHOLD,
        None,
    )
    .await;
    assert!(
        !compaction_fired(&runtime).await,
        "200k input tokens is under four fifths of a million-token window, so \
         the inherited policy does nothing; if this starts firing, meerkat's \
         model-aware default changed",
    );
}

/// `UnifiedRuntimeBuilder::compaction()` makes the threshold real: the same
/// turn that inherits its way past compaction fires immediately once the host
/// pins a threshold under it.
#[tokio::test]
async fn host_compaction_policy_pins_the_trigger() {
    let runtime = runtime_with(
        "compaction-policy-pinned",
        HUGE_BUT_UNDER_INHERITED_THRESHOLD,
        Some(pinned_policy(PINNED_THRESHOLD)),
    )
    .await;
    assert!(
        compaction_fired(&runtime).await,
        "a declared host threshold must reach the compactor the member's \
         session is built with",
    );
}

/// Provider input tokens one long-turn boundary reports: over [`LONG_TURN_THRESHOLD`].
const LONG_TURN_INPUT_TOKENS: u64 = 60_000;
const LONG_TURN_THRESHOLD: u64 = 50_000;
/// Tool-calling boundaries of the one long turn before its final answer.
const LONG_TURN_TOOL_BOUNDARIES: usize = 12;
/// More than Core's default retention (4), past which a Core 0.9.4 turn
/// failed.
const LONG_TURN_MIN_COMPACTIONS: usize = 6;
/// The explicitly narrow transcript history window of the second run.
const NARROW_RETAINED_REWRITES: usize = 2;

/// One turn that keeps calling a tool: every tool-offering request answers
/// with a tool call over the threshold until [`LONG_TURN_TOOL_BOUNDARIES`],
/// then with the final answer. A request without tools is the compactor's
/// summary call and gets a summary.
struct LongTurnClient {
    boundaries: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for LongTurnClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let usage = |input_tokens| LlmEvent::UsageUpdate {
            usage: llm_usage::turn_usage(
                FIXTURE_MODEL,
                meerkat_core::Provider::Other,
                Usage {
                    input_tokens,
                    output_tokens: 1,
                    ..Default::default()
                },
            ),
        };
        let events = match request.tools.first() {
            None => vec![
                LlmEvent::TextDelta {
                    delta: "summary of the long turn so far".to_string(),
                    meta: None,
                },
                usage(1),
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                },
            ],
            Some(tool) => {
                let boundary = self
                    .boundaries
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if boundary < LONG_TURN_TOOL_BOUNDARIES {
                    vec![
                        LlmEvent::ToolCallComplete {
                            id: format!("long-turn-call-{boundary}"),
                            name: tool.name.to_string(),
                            args: serde_json::json!({}),
                            meta: None,
                        },
                        usage(LONG_TURN_INPUT_TOKENS),
                        LlmEvent::Done {
                            outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                        },
                    ]
                } else {
                    vec![
                        LlmEvent::TextDelta {
                            delta: "done".to_string(),
                            meta: None,
                        },
                        usage(LONG_TURN_INPUT_TOKENS),
                        LlmEvent::Done {
                            outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                        },
                    ]
                }
            }
        };
        Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// One long turn: its outcome, the compactions it completed, and what Core
/// stored for its session once the runtime shut down.
struct LongTurn {
    outcome: Result<(), String>,
    compactions: usize,
    /// Rewrites the session's durable head has adopted.
    adopted_rewrites: u64,
    /// Rewrite bodies Core still serves for the session.
    stored_rewrites: usize,
}

/// Run one persistent member turn through [`LongTurnClient`] with a policy
/// that compacts at every boundary, shut the runtime down, and read Core's
/// stored rewrites for the member's session.
async fn long_turn(mob_id: &str, retained: Option<usize>) -> LongTurn {
    let state = tempfile::tempdir().expect("state directory");
    let mut policy = meerkat_core::config::CompactionRuntimeConfig {
        auto_compact_threshold: LONG_TURN_THRESHOLD,
        auto_compact_threshold_explicit: true,
        recent_turn_budget: 1,
        min_turns_between_compactions: 1,
        ..Default::default()
    };
    if let Some(retained) = retained {
        policy.history_retained_rewrites = retained;
    }
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition(mob_id))
            .default_llm_client(Arc::new(LongTurnClient {
                boundaries: std::sync::atomic::AtomicUsize::new(0),
            }))
            .persistent_state(state.path())
            .compaction(policy)
            .max_sessions(4)
            .build(),
    )
    .await
    .unwrap_or_else(|error| panic!("runtime builds: {error}"));
    runtime
        .spawn(SpawnMemberSpec::from_wire(
            "worker".to_string(),
            MEMBER_ALIAS.to_string(),
            None,
            None,
            None,
        ))
        .await
        .unwrap_or_else(|error| panic!("member spawns: {error}"));
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let admission = runtime
        .start_member_turn(
            MEMBER_ALIAS,
            ContentInput::Text("one long turn".to_string()),
            HandlingMode::Queue,
            MemberTurnOptions::new(),
            Some(tx.clone()),
        )
        .await
        .unwrap_or_else(|error| panic!("the long turn must be admitted: {error}"));
    let session_id = admission.session_id.clone();
    let outcome = tokio::time::timeout(Duration::from_mins(2), admission.turn.wait())
        .await
        .unwrap_or_else(|error| panic!("the long turn timed out: {error}"))
        .map(|_| ())
        .map_err(|error| error.to_string());
    drop(tx);
    let mut completed = 0;
    while let Ok(Some(envelope)) = tokio::time::timeout(DRAIN_QUIET_PERIOD, rx.recv()).await {
        if matches!(envelope.payload, AgentEvent::CompactionCompleted { .. }) {
            completed += 1;
        }
    }
    runtime.shutdown().await;
    drop(runtime);
    let (adopted_rewrites, stored_rewrites) = core_rewrites(state.path(), &session_id).await;
    LongTurn {
        outcome,
        compactions: completed,
        adopted_rewrites,
        stored_rewrites,
    }
}

/// The session's adopted rewrite count on its Core head, and the number of
/// rewrite bodies Core serves for it, read from Core's session store on
/// `runtime.sqlite` the way `identity_first_repeated_compaction.rs` reads
/// them. Read only after the runtime has shut down.
async fn core_rewrites(state: &std::path::Path, session_id: &str) -> (u64, usize) {
    use meerkat_core::session_store::IncrementalSessionStore as _;
    let store = meerkat_store::SqliteSessionStore::open(state.join("runtime.sqlite"))
        .expect("open Core's session store");
    let id = meerkat_core::types::SessionId::parse(session_id).expect("a session id");
    let head = store
        .load_canonical_head(&id)
        .await
        .expect("read Core's head")
        .unwrap_or_else(|| panic!("Core holds no canonical head for {session_id}"));
    let rewrites = store
        .load_rewrites(&id)
        .await
        .expect("read Core's adopted rewrites")
        .len();
    (head.rewrite_count, rewrites)
}

/// Core 0.9.4 retired rewrite bodies past `history_retained_rewrites` after
/// each compaction, so one turn that compacted more than
/// `history_retained_rewrites` times could not prove its own intra-turn
/// checkpoint and was abandoned; Meta 0.9.4 built every session with a
/// 64-rewrite window to keep long turns inside it. Core 0.9.5 fixes that: a
/// session its durable HeadCanonical head describes keeps every rewrite the
/// head does not cover yet, and the run moves the head at each of its round
/// commits. The same long turn now completes under Meta's window and under an
/// explicitly narrow one alike, compacting well past the narrow one.
///
/// The declared window still reaches Core's session: the narrow run's
/// committed session keeps no more rewrite bodies than its window plus the one
/// a run may hold beyond it, and fewer than Meta's window keeps for the same
/// turn.
#[tokio::test]
async fn one_turn_with_many_compactions_completes_under_meta_retention() {
    let meta = long_turn("compaction-long-turn-meta", None).await;
    assert!(
        meta.outcome.is_ok(),
        "the long turn must complete under Meta's retention: {:?}",
        meta.outcome
    );
    assert!(
        meta.compactions >= LONG_TURN_MIN_COMPACTIONS,
        "the long turn must compact at least {LONG_TURN_MIN_COMPACTIONS} times, saw {}",
        meta.compactions
    );
    let narrow = long_turn(
        "compaction-long-turn-narrow",
        Some(NARROW_RETAINED_REWRITES),
    )
    .await;
    assert!(
        narrow.outcome.is_ok(),
        "a {NARROW_RETAINED_REWRITES}-rewrite window must not refuse the long turn's \
         checkpoint ({} compactions): {:?}",
        narrow.compactions,
        narrow.outcome
    );
    assert!(
        narrow.compactions >= LONG_TURN_MIN_COMPACTIONS,
        "the narrow turn must compact past its window, at least \
         {LONG_TURN_MIN_COMPACTIONS} times, saw {}",
        narrow.compactions
    );
    assert!(
        narrow.stored_rewrites <= NARROW_RETAINED_REWRITES + 1,
        "the narrow run must keep at most {} rewrite bodies, kept {} of {} adopted",
        NARROW_RETAINED_REWRITES + 1,
        narrow.stored_rewrites,
        narrow.adopted_rewrites
    );
    assert!(
        narrow.stored_rewrites < meta.stored_rewrites,
        "the narrow window must keep fewer rewrite bodies than Meta's for the same turn: \
         {} of {} adopted against {} of {}",
        narrow.stored_rewrites,
        narrow.adopted_rewrites,
        meta.stored_rewrites,
        meta.adopted_rewrites
    );
}
