//! distiller-eval — calibration-harness seam for the memory Distiller
//! (docs/design/agent-memory-architecture.md §8.4/§11).
//!
//! Reads one fixture-shaped JSON object on stdin:
//!
//! ```json
//! {"transcript": [{"role": "user", "text": "..."}],
//!  "existing_manifest": [RecordMeta...],
//!  "tombstones": [{"title": "...", "kind": "Fact"}],
//!  "taint": {"session_tainted": false, "source": "..."},
//!  "profile_path": "optional/path.toml"}
//! ```
//!
//! and writes `{"ops": [...], "quarantined": bool}` on stdout — the parsed,
//! validated op list plus the deterministic write-gate verdict the fixture's
//! taint context produces (the gate law is deterministic, so the harness can
//! score it without a store). `--mock` swaps the live model for a scripted
//! client that extracts nothing (`[]`) — the doctrine's preferred output —
//! so the noop scorecard plumbing runs end-to-end without credentials.
//!
//! Exit codes: 0 ok, 1 distiller error, 2 usage/config error, 3 live
//! requested but no auth is resolvable (the harness treats 3 as
//! SKIP-with-notice, not failure).

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use serde::Deserialize;

use meerkat_client::types::LlmStream;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::Provider;
use meerkat_meta::memory::distiller::{
    DistillerClientHandle, DistillerError, DistillerProfile, FactoryDistillerHandle, TombstoneMeta,
    TranscriptMessage, TranscriptSlice, extract, render_transcript, validate_op,
};
use meerkat_meta::memory::records::{EvidenceRef, MemoryAuthor, MemoryKind, RecordMeta};
use meerkat_meta::memory::taint::{ContentTrustConfig, LlmWriteGate, TaintLlmWriteGate};

const EXIT_DISTILLER_ERROR: i32 = 1;
const EXIT_USAGE: i32 = 2;
const EXIT_NO_AUTH: i32 = 3;

/// Manifest entry in the calibration-fixture format (capitalized kinds,
/// same convention as the selector fixtures).
#[derive(Deserialize)]
struct FixtureRecordMeta {
    id: String,
    kind: String,
    title: String,
    #[serde(default)]
    description: String,
    age_days: u64,
    #[serde(default)]
    rank: Option<u32>,
}

fn parse_kind(kind: &str) -> Result<MemoryKind, String> {
    match kind {
        "Preference" => Ok(MemoryKind::Preference),
        "Fact" => Ok(MemoryKind::Fact),
        "Gotcha" => Ok(MemoryKind::Gotcha),
        "Procedure" => Ok(MemoryKind::Procedure),
        "Relationship" => Ok(MemoryKind::Relationship),
        "OpenLoop" => Ok(MemoryKind::OpenLoop),
        "Reference" => Ok(MemoryKind::Reference),
        other => MemoryKind::parse(other).ok_or_else(|| format!("unknown record kind '{other}'")),
    }
}

impl FixtureRecordMeta {
    fn into_record_meta(self) -> Result<RecordMeta, String> {
        Ok(RecordMeta {
            id: self.id,
            kind: parse_kind(&self.kind)?,
            title: self.title,
            description: self.description,
            age_days: self.age_days,
            rank: self.rank,
        })
    }
}

#[derive(Deserialize)]
struct FixtureTranscriptMessage {
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct FixtureTombstone {
    title: String,
    kind: String,
}

#[derive(Deserialize, Default)]
struct FixtureTaint {
    #[serde(default)]
    session_tainted: bool,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Deserialize)]
struct EvalInput {
    transcript: Vec<FixtureTranscriptMessage>,
    #[serde(default)]
    existing_manifest: Vec<FixtureRecordMeta>,
    #[serde(default)]
    tombstones: Vec<FixtureTombstone>,
    #[serde(default)]
    taint: Option<FixtureTaint>,
    #[serde(default)]
    profile_path: Option<PathBuf>,
}

/// Deterministic stand-in for the live model: extracts nothing — the
/// doctrine's preferred output — through the real prompt/parse path.
struct MockDistillerLlm;

#[async_trait]
impl LlmClient for MockDistillerLlm {
    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "[]".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(meerkat_core::StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        Provider::Other
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn fail(code: i32, message: &str) -> ! {
    eprintln!("distiller-eval: {message}");
    std::process::exit(code);
}

fn role_label(role: &str) -> &'static str {
    match role {
        "user" => "user",
        "assistant" => "assistant",
        "system notice" => "system notice",
        "tool results" => "tool results",
        _ => "user",
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
                    "usage: distiller-eval [--mock] < fixture.json\n\
                     stdin:  {{transcript, existing_manifest?, tombstones?, taint?, profile_path?}}\n\
                     stdout: {{ops, quarantined}}\n\
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
    let manifest: Vec<RecordMeta> = match input
        .existing_manifest
        .into_iter()
        .map(FixtureRecordMeta::into_record_meta)
        .collect::<Result<_, _>>()
    {
        Ok(manifest) => manifest,
        Err(err) => fail(EXIT_USAGE, &format!("invalid fixture manifest: {err}")),
    };
    let tombstones: Vec<TombstoneMeta> = match input
        .tombstones
        .into_iter()
        .map(|tombstone| {
            Ok::<_, String>(TombstoneMeta {
                kind: parse_kind(&tombstone.kind)?,
                title: tombstone.title,
                tombstoned_at_ms: 0,
            })
        })
        .collect::<Result<_, _>>()
    {
        Ok(tombstones) => tombstones,
        Err(err) => fail(EXIT_USAGE, &format!("invalid fixture tombstones: {err}")),
    };
    if input.transcript.is_empty() {
        fail(EXIT_USAGE, "fixture transcript must not be empty");
    }
    let slice = TranscriptSlice {
        session_key: "fixture-session".to_string(),
        start_index: 0,
        end_index: input.transcript.len() as u64,
        head_revision: None,
        messages: input
            .transcript
            .iter()
            .enumerate()
            .map(|(index, message)| TranscriptMessage {
                index: index as u64,
                role: role_label(&message.role),
                text: message.text.clone(),
            })
            .collect(),
    };

    let profile = match input.profile_path.as_deref() {
        Some(path) => match DistillerProfile::load(path) {
            Ok(profile) => profile,
            Err(err) => fail(EXIT_USAGE, &err.to_string()),
        },
        None => DistillerProfile::embedded_default(),
    };

    let client: Arc<dyn LlmClient> = if mock {
        Arc::new(MockDistillerLlm)
    } else {
        // Auth resolves through meerkat's factory seam against the process
        // environment. A resolution failure is the SKIP signal, not an
        // error — same contract as selector-eval.
        let state_dir = match tempfile::tempdir() {
            Ok(dir) => dir.keep(),
            Err(err) => fail(
                EXIT_DISTILLER_ERROR,
                &format!("cannot create scratch state dir: {err}"),
            ),
        };
        let handle =
            FactoryDistillerHandle::new(state_dir, meerkat::Config::default(), "default", &profile);
        match handle.client().await {
            Ok(client) => client,
            Err(DistillerError::Auth(message)) => fail(
                EXIT_NO_AUTH,
                &format!("no resolvable auth for live distiller run: {message}"),
            ),
            Err(err) => fail(EXIT_DISTILLER_ERROR, &err.to_string()),
        }
    };

    let transcript_text = render_transcript(&slice);
    let raw_ops = match extract(&profile, &*client, &manifest, &tombstones, &transcript_text).await
    {
        Ok(ops) => ops,
        Err(DistillerError::Auth(message)) if !mock => fail(
            EXIT_NO_AUTH,
            &format!("auth rejected during live distiller run: {message}"),
        ),
        Err(err) => fail(EXIT_DISTILLER_ERROR, &err.to_string()),
    };

    let manifest_ids: Vec<String> = manifest.iter().map(|meta| meta.id.clone()).collect();
    let mut ops = Vec::new();
    for raw in raw_ops {
        match validate_op(raw, &manifest_ids) {
            Ok(op) => ops.push(op),
            Err(reason) => eprintln!("distiller-eval: op dropped: {reason}"),
        }
    }

    // Deterministic write-gate verdict for the fixture's taint context: the
    // same `TaintLlmWriteGate` law the store enforces, over the fixture
    // session's evidence — session-tainted ⇒ every distillate quarantines.
    let taint = input.taint.unwrap_or_default();
    let quarantined = {
        let tracker =
            meerkat_meta::memory::taint::SessionTaintTracker::new(ContentTrustConfig::default());
        tracker.note_current_session("fixture-identity", &slice.session_key);
        if taint.session_tainted {
            // Route through the same classification path production taint
            // takes: an untrusted web-tool ingestion on the session. The
            // fixture's `source` is documentation for the reader; the
            // always-untrusted web class is the mechanism.
            let _ = &taint.source;
            tracker.observe_agent_event(
                "fixture-identity",
                &meerkat_core::event::AgentEvent::ToolResultReceived {
                    id: "fixture-tool".to_string(),
                    name: "web_fetch".to_string(),
                    content: vec![],
                    is_error: false,
                },
            );
        }
        let gate =
            TaintLlmWriteGate::new(Some(tracker), meerkat_meta::AgentMemoryLlmWrites::Observed);
        let evidence = vec![EvidenceRef {
            session_id: slice.session_key.clone(),
            generation: 0,
            revision: None,
            range: Some((0, slice.end_index.saturating_sub(1))),
        }];
        gate.quarantine_reason(
            &MemoryAuthor::Distiller {
                run_id: "distiller-eval".to_string(),
            },
            meerkat_meta::memory::staged::StagedBatchKind::FreshWrite,
            &evidence,
        )
        .is_some()
    };

    let rendered_ops: Vec<serde_json::Value> = ops
        .iter()
        .map(|op| {
            serde_json::json!({
                "action": match &op.action {
                    meerkat_meta::memory::distiller::ProposedAction::Remember => "remember".to_string(),
                    meerkat_meta::memory::distiller::ProposedAction::Update { target_id } =>
                        format!("update:{target_id}"),
                },
                "kind": op.kind.as_str(),
                "title": op.title,
                "description": op.description,
                "body": op.body,
                "tags": op.tags,
                "epistemic": match op.epistemic {
                    meerkat_meta::memory::distiller::Epistemic::OperatorSaid => "operator_said",
                    meerkat_meta::memory::distiller::Epistemic::Observed => "observed",
                },
                "evidence_range": op.evidence_range,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::json!({
            "ops": rendered_ops,
            "quarantined": quarantined,
        })
    );
}
