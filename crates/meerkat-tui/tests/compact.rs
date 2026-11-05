//! The compaction policy on Meerkat's native auto-compaction, end to end
//! through the real runtime-backed service with a scripted LLM.
//!
//! Six short turns build history; the sixth reports 60k input tokens. With
//! `compact_auto_percent = 5` of the model's window (1M tokens) the threshold is
//! below that:
//! - `auto`: the next turn's first model call compacts first (a summary
//!   call; the model then sees the summary instead of the old turns), and
//!   `CompactionCompleted` (published when the session next runs, once the
//!   runtime finalized the memory projection) reports a smaller history,
//!   the number the gauge shows;
//! - `warn`: the native trigger is off, nothing compacts, and the gauge
//!   warns; `/compact now` arms the next turn, which compacts, and the
//!   session is back on the policy afterwards.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Rig, RigOptions, ScriptedLlmClient, Step, Workspace};
use meerkat_core::AgentEvent;
use meerkat_tui::runner::Start;
use meerkat_tui::ui::compaction::CompactionGauge;
use meerkat_tui::ui::{CompactMode, Effect, Msg, StatusUpdate};

/// Input tokens the sixth turn reports.
const BIG: u64 = 60_000;

const SHORT_TURNS: usize = 5;

/// Five short turns, then one reporting [`BIG`] input tokens.
fn history() -> Vec<Step> {
    let mut steps = vec![Step::Text("ok"); SHORT_TURNS];
    steps.push(Step::TextUsage("big", BIG));
    steps
}

async fn rig(llm: &std::sync::Arc<ScriptedLlmClient>, ws: &Workspace, mode: &str) -> Rig {
    let args = [
        "--set",
        &format!("auto.compact={mode}"),
        "--set",
        "auto.compact_warn_percent=5",
        "--set",
        "auto.compact_auto_percent=5",
    ];
    Rig::start_opts(
        &ws.root,
        llm,
        Start::New,
        RigOptions {
            args: args.iter().map(ToString::to_string).collect(),
            ..RigOptions::default()
        },
    )
    .await
}

async fn turns(rig: &mut Rig, n: usize, prefix: &str) {
    for i in 0..n {
        rig.send(Effect::StartTurn(format!("{prefix} {i}")));
        rig.turn_finished().await.result.expect("turn succeeds");
    }
}

/// Wait (bounded) until the session published a completed compaction.
async fn completed(rig: &Rig) -> bool {
    for _ in 0..100 {
        if compactions(rig)
            .iter()
            .any(|e| matches!(e, AgentEvent::CompactionCompleted { .. }))
        {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    false
}

fn started(rig: &Rig) -> usize {
    compactions(rig)
        .iter()
        .filter(|e| matches!(e, AgentEvent::CompactionStarted { .. }))
        .count()
}

fn compactions(rig: &Rig) -> Vec<AgentEvent> {
    rig.events
        .lock()
        .iter()
        .filter(|e| {
            matches!(
                e,
                AgentEvent::CompactionStarted { .. }
                    | AgentEvent::CompactionCompleted { .. }
                    | AgentEvent::CompactionFailed { .. }
            )
        })
        .cloned()
        .collect()
}

/// The context window the runner reported for the session's model.
fn window(rig: &Rig) -> u64 {
    rig.seen
        .iter()
        .rev()
        .find_map(|msg| match msg {
            Msg::Status(
                StatusUpdate::ContextWindow(Some(w)) | StatusUpdate::ApproxContextWindow(w),
            ) => Some(*w),
            _ => None,
        })
        .expect("a context window")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_mode_compacts_natively_at_the_threshold() {
    let ws = Workspace::new();
    let mut script = history();
    script.push(Step::Text("SUMMARY of the earlier turns"));
    script.push(Step::Text("after"));
    script.push(Step::Text("later"));
    let llm = ScriptedLlmClient::new(script);
    let mut rig = rig(&llm, &ws, "auto").await;
    let policy = rig.approval.settings().compaction_policy(None);
    assert_eq!(policy.mode, CompactMode::Auto);
    turns(&mut rig, SHORT_TURNS + 1, "turn").await;
    assert_eq!(started(&rig), 0, "nothing crossed yet");
    let window = window(&rig);
    assert!(policy.auto_tokens(Some(window)) < BIG, "window {window}");

    rig.send(Effect::StartTurn("next".into()));
    let outcome = rig.turn_finished().await;
    assert_eq!(outcome.result.expect("turn succeeds").text, "after");
    assert_eq!(started(&rig), 1);
    let requests = llm.recorded();
    assert_eq!(
        requests.len(),
        SHORT_TURNS + 3,
        "one extra call: the summary"
    );
    let after = requests.last().unwrap();
    assert!(after.messages.len() < requests[SHORT_TURNS].messages.len());
    assert!(
        after.user().contains("SUMMARY of the earlier turns"),
        "{}",
        after.user()
    );
    assert!(!after.user().contains("turn 0"), "{}", after.user());

    // The completion is published when the session runs again.
    rig.send(Effect::StartTurn("again".into()));
    rig.turn_finished().await.result.expect("turn succeeds");
    assert!(completed(&rig).await, "{:?}", compactions(&rig));
    let left = compactions(&rig)
        .iter()
        .find_map(|e| match e {
            AgentEvent::CompactionCompleted {
                estimated_history_tokens_after,
                messages_before,
                messages_after,
                ..
            } => {
                assert!(messages_after < messages_before);
                *estimated_history_tokens_after
            }
            _ => None,
        })
        .expect("a measured history");
    assert!(left < BIG, "{left}");
    assert_eq!(started(&rig), 1, "no second compaction");
    rig.quit().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn warn_mode_warns_and_compacts_only_on_compact_now() {
    let ws = Workspace::new();
    let mut script = history();
    // A turn past the threshold that does not compact.
    script.push(Step::Text("still whole"));
    // `/compact now`: the next turn's first call is the summary.
    script.push(Step::Text("SUMMARY of the earlier turns"));
    script.push(Step::TextUsage("compacted", BIG));
    // Back on the policy: no compaction despite the size.
    script.push(Step::Text("whole again"));
    let llm = ScriptedLlmClient::new(script);
    let mut rig = rig(&llm, &ws, "warn").await;
    let policy = rig.approval.settings().compaction_policy(None);
    assert_eq!(policy.mode, CompactMode::Warn);
    turns(&mut rig, SHORT_TURNS + 2, "turn").await;
    assert_eq!(started(&rig), 0, "warn mode never compacts by size");
    assert_eq!(llm.recorded().len(), SHORT_TURNS + 2);
    // The gauge the UI runs with this policy warns at this usage.
    let mut gauge = CompactionGauge::new(policy);
    let warning = gauge
        .after_turn(BIG + 5, Some(window(&rig)))
        .expect("a warning");
    assert!(warning.contains("auto-compaction is off"), "{warning}");

    rig.send(Effect::CompactNow);
    let msg = rig
        .wait_for(|msg| matches!(msg, Msg::CompactArmed(_)))
        .await;
    assert!(matches!(msg, Msg::CompactArmed(Ok(()))), "{msg:?}");
    rig.send(Effect::StartTurn("compact first".into()));
    assert_eq!(
        rig.turn_finished()
            .await
            .result
            .expect("turn succeeds")
            .text,
        "compacted"
    );
    assert_eq!(started(&rig), 1);
    let last = llm.recorded().last().unwrap().clone();
    assert!(
        last.user().contains("SUMMARY of the earlier turns"),
        "{}",
        last.user()
    );

    rig.send(Effect::StartTurn("one more".into()));
    assert_eq!(
        rig.turn_finished()
            .await
            .result
            .expect("turn succeeds")
            .text,
        "whole again"
    );
    assert_eq!(started(&rig), 1, "disarmed after the turn");
    assert_eq!(llm.recorded().len(), SHORT_TURNS + 5);
    // The summary survived the rebuild back to the policy's threshold.
    let last = llm.recorded().last().unwrap().clone();
    assert!(
        last.user().contains("SUMMARY of the earlier turns"),
        "{}",
        last.user()
    );
    assert!(!last.user().contains("turn 0"), "{}", last.user());
    rig.quit().await;
}
