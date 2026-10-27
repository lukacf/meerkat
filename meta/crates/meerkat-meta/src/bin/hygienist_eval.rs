//! hygienist-eval — calibration-harness seam for the memory Hygienist's
//! curation pass (docs/design/agent-memory-architecture.md §8.6/§11).
//!
//! Reads one fixture-shaped JSON object on stdin:
//!
//! ```json
//! {"transcript": [{"role": "user|assistant|assistant_tool_call|tool_results|
//!                  system|system_notice", "text": "...", "tool_use_id"?}],
//!  "span_references": [{"record_id", "quarantined", "range": [s, e]?}],
//!  "distilled_cursor": 7,
//!  "sequenced_after_harvest": false,
//!  "scripted_reply": {"ops": [...]},
//!  "profile_path": "optional/path.toml"}
//! ```
//!
//! and writes the parsed/validated outcome on stdout:
//! `{"validator": "ok" | "blocked: ...", "ops": [{"op", "range"}],
//!   "flagged_active_records": [...], "hull"?: [start, end],
//!   "replacement_len"?: N}`.
//!
//! `--mock` uses the fixture's `scripted_reply` (or a no-op when absent)
//! instead of a live model — the deterministic lane: the reply flows
//! through the REAL parse → §8.6 validator → replacement-construction path,
//! so quarantine hard-blocks, role law, and the §8.4 ordering invariant
//! gate without credentials. Live mode renders the production prompt over
//! the fixture and asks the profile's model.
//!
//! Exit codes: 0 ok, 1 hygienist error, 2 usage/config error, 3 live
//! requested but no auth is resolvable (harness treats 3 as SKIP).

use std::io::Read;
use std::path::PathBuf;

use serde::Deserialize;

use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, SystemMessage, SystemNoticeKind,
    SystemNoticeMessage, ToolResult, UserMessage,
};
use meerkat_meta::memory::hygienist::{
    self, FactoryHygienistHandle, HygieneRole, HygienistClientHandle, HygienistError,
    HygienistProfile, OrderingContext, RevisionAction, SpanReference,
};

const EXIT_HYGIENIST_ERROR: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_NO_AUTH: i32 = 3;

#[derive(Deserialize)]
struct FixtureMessage {
    role: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    tool_use_id: Option<String>,
}

#[derive(Deserialize)]
struct FixtureSpan {
    record_id: String,
    #[serde(default)]
    quarantined: bool,
    #[serde(default)]
    range: Option<(u64, u64)>,
}

#[derive(Deserialize)]
struct EvalInput {
    transcript: Vec<FixtureMessage>,
    #[serde(default)]
    span_references: Vec<FixtureSpan>,
    #[serde(default)]
    distilled_cursor: Option<u64>,
    #[serde(default)]
    sequenced_after_harvest: bool,
    #[serde(default)]
    scripted_reply: Option<serde_json::Value>,
    #[serde(default)]
    profile_path: Option<PathBuf>,
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("hygienist-eval: {message}");
    std::process::exit(code);
}

fn build_message(fixture: &FixtureMessage, index: usize) -> Result<Message, String> {
    let text = fixture.text.clone();
    match fixture.role.as_str() {
        "user" => Ok(Message::User(UserMessage::text(text))),
        "system" => Ok(Message::System(SystemMessage::new(text))),
        "system_notice" => Ok(Message::SystemNotice(SystemNoticeMessage::new(
            SystemNoticeKind::Generic,
            text,
        ))),
        "assistant" => Ok(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text { text, meta: None }],
            meerkat_core::StopReason::EndTurn,
        ))),
        "assistant_tool_call" => {
            let args = serde_json::value::RawValue::from_string("{}".to_string())
                .map_err(|err| err.to_string())?;
            Ok(Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::ToolUse {
                    id: fixture
                        .tool_use_id
                        .clone()
                        .unwrap_or_else(|| format!("call-{index}")),
                    name: "tool".to_string(),
                    args,
                    meta: None,
                }],
                meerkat_core::StopReason::ToolUse,
            )))
        }
        "tool_results" => Ok(Message::tool_results(vec![ToolResult::new(
            fixture
                .tool_use_id
                .clone()
                .unwrap_or_else(|| format!("call-{}", index.saturating_sub(1))),
            text,
            false,
        )])),
        other => Err(format!("unknown transcript role '{other}'")),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mock = false;
    for arg in &args {
        match arg.as_str() {
            "--mock" => mock = true,
            "--help" | "-h" => {
                eprintln!(
                    "usage: hygienist-eval [--mock] < fixture.json\n\
                     stdin:  {{transcript, span_references?, distilled_cursor?,\n\
                              sequenced_after_harvest?, scripted_reply?, profile_path?}}\n\
                     stdout: {{validator, ops, flagged_active_records, hull?, replacement_len?}}\n\
                     exit:   0 ok, 1 error, 2 usage, 3 live-without-auth (SKIP)"
                );
                std::process::exit(0);
            }
            other => fail(EXIT_USAGE, &format!("unknown argument '{other}'")),
        }
    }

    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() || raw.trim().is_empty() {
        fail(EXIT_USAGE, "expected a fixture JSON object on stdin");
    }
    let input: EvalInput = match serde_json::from_str(&raw) {
        Ok(input) => input,
        Err(err) => fail(EXIT_USAGE, &format!("invalid fixture JSON: {err}")),
    };

    let messages: Vec<Message> = input
        .transcript
        .iter()
        .enumerate()
        .map(|(index, fixture)| {
            build_message(fixture, index)
                .unwrap_or_else(|err| fail(EXIT_USAGE, &format!("invalid fixture message: {err}")))
        })
        .collect();
    let spans: Vec<SpanReference> = input
        .span_references
        .iter()
        .map(|span| SpanReference {
            record_id: span.record_id.clone(),
            quarantined: span.quarantined,
            range: span.range,
        })
        .collect();

    let profile = match input.profile_path.as_deref() {
        Some(path) => match HygienistProfile::load(path) {
            Ok(profile) => profile,
            Err(err) => fail(EXIT_USAGE, &err.to_string()),
        },
        None => HygienistProfile::embedded_default(),
    };

    let reply = if mock {
        input
            .scripted_reply
            .clone()
            .unwrap_or_else(|| serde_json::json!({"ops": []}))
            .to_string()
    } else {
        // Live mode: render the production prompt over the fixture and ask
        // the profile's model. Auth resolution failure is the SKIP signal,
        // same contract as the other eval bins.
        let prompt = hygienist::render_prompt(&profile, &messages, &spans);
        let state_dir = match tempfile::tempdir() {
            Ok(dir) => dir.keep(),
            Err(err) => fail(
                EXIT_HYGIENIST_ERROR,
                &format!("cannot create scratch state dir: {err}"),
            ),
        };
        let handle =
            FactoryHygienistHandle::new(state_dir, meerkat::Config::default(), "default", &profile);
        let client = match handle.client().await {
            Ok(client) => client,
            Err(HygienistError::Auth(message)) => fail(
                EXIT_NO_AUTH,
                &format!("no resolvable auth for live hygienist run: {message}"),
            ),
            Err(err) => fail(EXIT_HYGIENIST_ERROR, &err.to_string()),
        };
        match hygienist::complete_text(&*client, &profile, prompt).await {
            Ok(reply) => reply,
            Err(HygienistError::Auth(message)) => fail(
                EXIT_NO_AUTH,
                &format!("auth rejected during live hygienist run: {message}"),
            ),
            Err(err) => fail(EXIT_HYGIENIST_ERROR, &err.to_string()),
        }
    };

    let proposal = match hygienist::parse_revision_reply(&reply) {
        Ok(proposal) => proposal,
        Err(err) => fail(
            EXIT_HYGIENIST_ERROR,
            &format!("revision reply did not parse: {err}"),
        ),
    };
    let roles: Vec<HygieneRole> = messages.iter().map(HygieneRole::of).collect();
    let ordering = if input.sequenced_after_harvest {
        OrderingContext::SequencedAfterHarvest
    } else {
        OrderingContext::Cursor(input.distilled_cursor)
    };
    let (validator, validated) =
        match hygienist::validate_revision(&proposal, &roles, &spans, ordering) {
            Ok(validated) => ("ok".to_string(), Some(validated)),
            Err(reject) => (format!("blocked: {reject}"), None),
        };

    let rendered_ops: Vec<serde_json::Value> = proposal
        .ops
        .iter()
        .map(|op| {
            serde_json::json!({
                "op": match op.action {
                    RevisionAction::PruneToolResults => "prune_tool_results",
                    RevisionAction::Collapse { .. } => "collapse",
                },
                "range": [op.start, op.end],
            })
        })
        .collect();
    let mut output = serde_json::json!({
        "validator": validator,
        "ops": rendered_ops,
        "flagged_active_records": validated
            .as_ref()
            .map(|validated| validated.flagged_active_records.clone())
            .unwrap_or_default(),
    });
    if let Some(validated) = validated
        && let Some((hull_start, hull_end, replacement)) =
            hygienist::build_replacement(&messages, &validated.ops)
    {
        output["hull"] = serde_json::json!([hull_start, hull_end]);
        output["replacement_len"] = serde_json::json!(replacement.len());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&output).unwrap_or_else(|err| fail(
            EXIT_HYGIENIST_ERROR,
            &format!("cannot serialize output: {err}")
        ))
    );
}
