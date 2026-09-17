//! Opt-in S99 diagnostic evidence. This is not playback or context authority.
//! One private, continuously flushed file lives outside the scenario TempDir.
//! Any Fault invalidates the entire journal, including an earlier Passed row.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use meerkat::experimental_gpt_live::thinking_capture;
use serde::{Deserialize, Serialize};

use super::AudioEvidence;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Setup,
    Opening,
    Connected,
    InitialUnknown,
    TypedCorrection,
    SpokenCorrection,
    DelegatedWork,
    ReleasingSummary,
    ProviderAcknowledged,
    HistoricalRecall,
    CurrentFactsRecall,
    Closing,
    Reopening,
    ObsoleteJobRelease,
    ReplacementUnknown,
    ReplacementRecall,
    Finished,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    TimedOut,
    CancelledOrPanicked,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preparation {
    NotRequested,
    Capturing,
    Generating,
    Delivering,
    ProviderAcknowledged,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    RecordLimit,
    StringLimit,
    RecordBytesLimit,
    FileBytesLimit,
    Io,
    Poisoned,
    BrowserQueueLimit,
    BrowserStringLimit,
    BrowserReaderClosed,
    InvalidBrowserEvidence,
    ProviderOverflow,
    ProviderStringLimit,
    ProviderContention,
    MissingScopedProviderCapture,
    AfterFinish,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub records: usize,
    pub file_bytes: usize,
    pub record_bytes: usize,
    pub string_bytes: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct CaptureLimits {
    pub files: usize,
    pub browser_pending: usize,
    pub browser_records: usize,
    pub browser_string_bytes: usize,
    pub provider_records: usize,
    pub provider_string_bytes: usize,
    pub provider_id_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            records: 20_000,
            file_bytes: 16 * 1024 * 1024,
            record_bytes: 128 * 1024,
            string_bytes: 32 * 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceFact {
    pub source_row: usize,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerRevision {
    NotExposedBySummarySnapshot,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Input,
    Output,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeRecord {
    Transcript {
        direction: Direction,
        delta: String,
        event_index: u64,
        browser_ms: f64,
        provider_start_ms: Option<f64>,
        audio: AudioEvidence,
    },
    Audio {
        browser_ms: f64,
        audio: AudioEvidence,
    },
    Fault {
        fault: BrowserFault,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserFault {
    QueueLimit,
    StringLimit,
    CaptureFailure,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAction {
    OpenRequested,
    Connected,
    CloseRequested,
    Closed,
    BrowserDropping,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobAction {
    ReleaseRequested,
    PermissionReleased,
    ReleaseRejected,
    Returned,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    Fixture {
        expected_phrase: String,
        limits: Limits,
        capture_limits: CaptureLimits,
    },
    Stage {
        stage: Stage,
    },
    Preparation {
        channel: u32,
        status: Preparation,
    },
    Source {
        job: u32,
        channel: u32,
        canonical_cursor: u64,
        projection_sha256: String,
        owner_revision: OwnerRevision,
        facts: Vec<SourceFact>,
    },
    Summary {
        job: u32,
        text: String,
        expected_fact_present: bool,
        input_tokens: u64,
        output_tokens: u64,
    },
    Job {
        job: u32,
        action: JobAction,
    },
    Channel {
        channel: u32,
        action: ChannelAction,
    },
    Thinking {
        event: thinking_capture::Event,
    },
    Native {
        channel: u32,
        record: NativeRecord,
    },
    Exchange {
        exchange: u32,
        channel: u32,
        stage: Stage,
        start_event_index: usize,
        input_timeout_ms: u64,
        response_timeout_ms: u64,
        baseline: AudioEvidence,
    },
    ResponseWindow {
        exchange: u32,
        timeout_ms: u64,
    },
    ExchangeEnd {
        exchange: u32,
        matched: bool,
        audio: AudioEvidence,
    },
    Fault {
        fault: Fault,
    },
    Outcome {
        outcome: Outcome,
        last_stage: Stage,
    },
}

#[derive(Serialize, Deserialize)]
pub struct Envelope {
    pub sequence: usize,
    pub elapsed_ms: u64,
    pub record: Record,
}

struct State {
    file: File,
    count: usize,
    bytes: usize,
    fault: Option<Fault>,
    finished: bool,
    stage: Stage,
    channel: u32,
    job: u32,
    exchange: u32,
    attached_channels: Vec<u32>,
}

struct Inner {
    state: Mutex<State>,
    started: Instant,
    path: PathBuf,
    expected_phrase: String,
    secrets: Vec<String>,
    limits: Limits,
    wire: thinking_capture::Capture,
}

#[derive(Clone)]
pub struct Journal(Arc<Inner>);

impl Journal {
    pub fn create(expected_phrase: String) -> Result<Self, Fault> {
        let directory = super::workspace_root()
            .join("target/e2e-live-audio-artifacts/s99")
            .join(uuid::Uuid::new_v4().to_string());
        let secrets = [
            "OPENAI_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY_OLD",
        ]
        .into_iter()
        .filter_map(|name| std::env::var(name).ok())
        .filter(|secret| !secret.is_empty())
        .collect();
        Self::at(&directory, expected_phrase, secrets, Limits::default())
    }

    fn at(
        directory: &Path,
        expected_phrase: String,
        mut secrets: Vec<String>,
        limits: Limits,
    ) -> Result<Self, Fault> {
        if limits.records < 5 || limits.file_bytes < 8192 || limits.record_bytes < 512 {
            return Err(Fault::RecordLimit);
        }
        std::fs::create_dir_all(directory).map_err(|_| Fault::Io)?;
        let path = directory.join("journal.jsonl");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&path).map_err(|_| Fault::Io)?;
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        let journal = Self(Arc::new(Inner {
            state: Mutex::new(State {
                file,
                count: 0,
                bytes: 0,
                fault: None,
                finished: false,
                stage: Stage::Setup,
                channel: 0,
                job: 0,
                exchange: 0,
                attached_channels: Vec::new(),
            }),
            started: Instant::now(),
            path,
            expected_phrase: expected_phrase.clone(),
            secrets,
            limits,
            wire: thinking_capture::Capture::new(),
        }));
        journal.record(Record::Fixture {
            expected_phrase,
            limits,
            capture_limits: CaptureLimits {
                files: 1,
                browser_pending: 128,
                browser_records: 20_000,
                browser_string_bytes: 16_384,
                provider_records: thinking_capture::Capture::MAX_EVENTS,
                provider_string_bytes: thinking_capture::Capture::MAX_TEXT_BYTES,
                provider_id_bytes: thinking_capture::Capture::MAX_ID_BYTES,
            },
        })?;
        println!("S99_EVIDENCE_JOURNAL path={}", journal.path().display());
        Ok(journal)
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }
    pub fn expected_phrase(&self) -> &str {
        &self.0.expected_phrase
    }
    pub fn wire(&self, channel: u32) -> thinking_capture::Capture {
        self.0.wire.for_channel(channel)
    }

    pub fn stage(&self, stage: Stage) -> Result<(), Fault> {
        self.flush_wire()?;
        self.record(Record::Stage { stage })
    }

    pub fn next_channel(&self) -> Result<u32, Fault> {
        let channel = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.channel = state.channel.checked_add(1).ok_or(Fault::RecordLimit)?;
            state.channel
        };
        self.channel(channel, ChannelAction::OpenRequested)?;
        Ok(channel)
    }

    pub fn current_channel(&self) -> Result<u32, Fault> {
        Ok(self.0.state.lock().map_err(|_| Fault::Poisoned)?.channel)
    }

    pub fn channel(&self, channel: u32, action: ChannelAction) -> Result<(), Fault> {
        self.flush_wire()?;
        self.record(Record::Channel { channel, action })
    }

    pub fn capture_source(
        &self,
        snapshot: &meerkat::session_runtime::live_summary::LiveContextSummarySnapshot<'_>,
    ) -> Result<u32, Fault> {
        let (job, channel) = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.job = state.job.checked_add(1).ok_or(Fault::RecordLimit)?;
            (state.job, state.channel)
        };
        self.record(Record::Source {
            job,
            channel,
            canonical_cursor: snapshot.canonical_message_cursor(),
            projection_sha256: meerkat_core::session::transcript_messages_digest(
                snapshot.messages(),
            )
            .map_err(|_| Fault::Io)?,
            owner_revision: OwnerRevision::NotExposedBySummarySnapshot,
            facts: selected_source_facts(snapshot.messages(), self.expected_phrase()),
        })?;
        Ok(job)
    }

    pub fn exchange(
        &self,
        start_event_index: usize,
        baseline: AudioEvidence,
    ) -> Result<u32, Fault> {
        self.flush_wire()?;
        let (exchange, channel, stage) = {
            let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
            state.exchange = state.exchange.checked_add(1).ok_or(Fault::RecordLimit)?;
            (state.exchange, state.channel, state.stage)
        };
        self.record(Record::Exchange {
            exchange,
            channel,
            stage,
            start_event_index,
            input_timeout_ms: 120_000,
            response_timeout_ms: 90_000,
            baseline,
        })?;
        Ok(exchange)
    }

    pub fn native(&self, channel: u32, record: NativeRecord) -> Result<(), Fault> {
        if let NativeRecord::Fault { fault } = record {
            return self.fail(match fault {
                BrowserFault::QueueLimit => Fault::BrowserQueueLimit,
                BrowserFault::StringLimit => Fault::BrowserStringLimit,
                BrowserFault::CaptureFailure => Fault::InvalidBrowserEvidence,
            });
        }
        self.record(Record::Native { channel, record })
    }

    pub fn flush_wire(&self) -> Result<(), Fault> {
        let events = self.0.wire.drain().map_err(|_| Fault::ProviderContention)?;
        for event in events {
            if matches!(event.event, thinking_capture::EventKind::SessionAttached) {
                self.0
                    .state
                    .lock()
                    .map_err(|_| Fault::Poisoned)?
                    .attached_channels
                    .push(event.channel_ordinal);
            }
            self.record(Record::Thinking { event })?;
        }
        if let Some(fault) = self.0.wire.fault() {
            return self.fail(match fault {
                thinking_capture::Fault::Overflow => Fault::ProviderOverflow,
                thinking_capture::Fault::StringLimit => Fault::ProviderStringLimit,
                thinking_capture::Fault::Contention => Fault::ProviderContention,
            });
        }
        self.check()
    }

    pub fn require_attached(&self, channel: u32) -> Result<(), Fault> {
        self.flush_wire()?;
        let present = self
            .0
            .state
            .lock()
            .map_err(|_| Fault::Poisoned)?
            .attached_channels
            .contains(&channel);
        if !present {
            return self.fail(Fault::MissingScopedProviderCapture);
        }
        Ok(())
    }

    pub fn check(&self) -> Result<(), Fault> {
        let state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        state.fault.map_or(Ok(()), Err)
    }

    pub fn record(&self, record: Record) -> Result<(), Fault> {
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        if let Some(fault) = state.fault {
            return Err(fault);
        }
        if state.finished {
            self.fail_locked(&mut state, Fault::AfterFinish);
            return Err(Fault::AfterFinish);
        }
        let encoded = self.encode(state.count, &record, true);
        let fault = match &encoded {
            Err(fault) => Some(*fault),
            Ok(_) if state.count >= self.0.limits.records - 3 => Some(Fault::RecordLimit),
            Ok(bytes) if bytes.len() > self.0.limits.record_bytes => Some(Fault::RecordBytesLimit),
            Ok(bytes) if state.bytes + bytes.len() > self.0.limits.file_bytes - 4096 => {
                Some(Fault::FileBytesLimit)
            }
            Ok(_) => None,
        };
        if let Some(fault) = fault {
            self.fail_locked(&mut state, fault);
            return Err(fault);
        }
        if let Record::Stage { stage } = record {
            state.stage = stage;
        }
        let bytes = encoded?;
        if self.write_locked(&mut state, &bytes).is_err() {
            self.fail_locked(&mut state, Fault::Io);
            return Err(Fault::Io);
        }
        Ok(())
    }

    fn encode(
        &self,
        sequence: usize,
        record: &Record,
        enforce_text_limit: bool,
    ) -> Result<Vec<u8>, Fault> {
        let mut value = serde_json::to_value(record).map_err(|_| Fault::Io)?;
        redact_and_bound(
            &mut value,
            &self.0.secrets,
            if enforce_text_limit {
                self.0.limits.string_bytes
            } else {
                usize::MAX
            },
            false,
        )?;
        let mut bytes = serde_json::to_vec(&serde_json::json!({
            "sequence": sequence,
            "elapsed_ms": u64::try_from(self.0.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            "record": value,
        }))
        .map_err(|_| Fault::Io)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn write_locked(&self, state: &mut State, bytes: &[u8]) -> Result<(), Fault> {
        state
            .file
            .write_all(bytes)
            .and_then(|()| state.file.sync_data())
            .map_err(|_| Fault::Io)?;
        state.bytes += bytes.len();
        state.count += 1;
        Ok(())
    }

    fn fail_locked(&self, state: &mut State, fault: Fault) {
        if state.fault.is_none() {
            state.fault = Some(fault);
            let result = self
                .encode(state.count, &Record::Fault { fault }, false)
                .and_then(|bytes| self.write_locked(state, &bytes));
            if result.is_err() {
                eprintln!("S99_EVIDENCE_WRITE_FAILURE path={}", self.path().display());
            }
            eprintln!(
                "S99_EVIDENCE_FAULT fault={fault:?} path={}",
                self.path().display()
            );
        }
    }

    pub fn fail(&self, fault: Fault) -> Result<(), Fault> {
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        self.fail_locked(&mut state, fault);
        Err(fault)
    }

    pub fn finish(&self, outcome: Outcome) -> Result<(), Fault> {
        if let Err(fault) = self.flush_wire() {
            let _ = self.fail(fault);
        }
        let mut state = self.0.state.lock().map_err(|_| Fault::Poisoned)?;
        if state.finished {
            return state.fault.map_or(Ok(()), Err);
        }
        let outcome = if state.fault.is_some() {
            Outcome::Failed
        } else {
            outcome
        };
        let bytes = self.encode(
            state.count,
            &Record::Outcome {
                outcome,
                last_stage: state.stage,
            },
            false,
        )?;
        self.write_locked(&mut state, &bytes)?;
        state.finished = true;
        state.fault.map_or(Ok(()), Err)
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "S99 evidence failure: {self:?}")
    }
}
impl std::error::Error for Fault {}

pub struct FailureGuard(pub Journal);
impl Drop for FailureGuard {
    fn drop(&mut self) {
        if let Err(fault) = self.0.finish(Outcome::CancelledOrPanicked) {
            eprintln!("{fault}; journal={}", self.0.path().display());
        }
    }
}

pub fn selected_source_facts(messages: &[meerkat_core::Message], phrase: &str) -> Vec<SourceFact> {
    messages
        .iter()
        .enumerate()
        .flat_map(|(source_row, message)| {
            let meerkat_core::Message::User(user) = message else {
                return Vec::new();
            };
            if !user.transcript_role.is_conversational() {
                return Vec::new();
            }
            user.content
                .iter()
                .filter_map(|block| match block {
                    meerkat_core::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .flat_map(|text| text.split_inclusive(['.', '\n']))
                .filter_map(|sentence| {
                    let lower = sentence.to_lowercase();
                    let fact = (!phrase.is_empty() && sentence.contains(phrase))
                        || (lower.contains("code word")
                            && ["tangerine", "violet", "cobalt"]
                                .iter()
                                .any(|word| lower.contains(word)))
                        || (lower.contains("favorite flower")
                            && ["daffodil", "marigold"]
                                .iter()
                                .any(|word| lower.contains(word)));
                    fact.then(|| SourceFact {
                        source_row,
                        text: sentence.trim().to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn redact_and_bound(
    value: &mut serde_json::Value,
    secrets: &[String],
    limit: usize,
    redact: bool,
) -> Result<(), Fault> {
    match value {
        serde_json::Value::String(text) => {
            if text.len() > limit {
                return Err(Fault::StringLimit);
            }
            if redact {
                for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
                    *text = text.replace(secret, "[REDACTED_CREDENTIAL]");
                }
            }
            if text.len() > limit {
                return Err(Fault::StringLimit);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_and_bound(item, secrets, limit, redact)?;
            }
        }
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                redact_and_bound(
                    value,
                    secrets,
                    limit,
                    matches!(
                        key.as_str(),
                        "text" | "delta" | "expected_phrase" | "client_event_id"
                    ),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        let root = super::super::workspace_root().join("target/e2e-live-audio-artifacts/offline");
        std::fs::create_dir_all(&root).unwrap();
        tempfile::Builder::new()
            .prefix("journal-")
            .tempdir_in(root)
            .unwrap()
    }

    fn records(journal: &Journal) -> Vec<Envelope> {
        std::fs::read_to_string(journal.path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn required_evidence(journal: &Journal) {
        let source = vec![
            meerkat_core::Message::System(meerkat_core::SystemMessage::new("FORBIDDEN_SYSTEM")),
            meerkat_core::Message::User(meerkat_core::UserMessage::text(
                "Historical vault phrase: amber otter copper. Current code word: Tangerine. Do not run anything.",
            )),
            meerkat_core::Message::BlockAssistant(meerkat_core::BlockAssistantMessage::new(
                vec![meerkat_core::AssistantBlock::Reasoning {
                    text: "FORBIDDEN_REASONING".into(),
                    meta: None,
                }],
                meerkat_core::StopReason::EndTurn,
            )),
        ];
        journal.stage(Stage::HistoricalRecall).unwrap();
        journal
            .record(Record::Source {
                job: 1,
                channel: 1,
                canonical_cursor: 3,
                projection_sha256: meerkat_core::session::transcript_messages_digest(&source)
                    .unwrap(),
                owner_revision: OwnerRevision::NotExposedBySummarySnapshot,
                facts: selected_source_facts(&source, journal.expected_phrase()),
            })
            .unwrap();
        journal
            .record(Record::Summary {
                job: 1,
                text: "amber otter copper; credential-fixture-not-a-real-key".into(),
                expected_fact_present: true,
                input_tokens: 100,
                output_tokens: 20,
            })
            .unwrap();
        for event in [
            thinking_capture::EventKind::ThinkingAppendAttempt {
                client_event_id: "owned-1".into(),
                text: "amber otter copper".into(),
            },
            thinking_capture::EventKind::ThinkingAppended {
                client_event_id: Some("owned-1".into()),
                matched_owned: true,
                accepted: true,
            },
        ] {
            journal
                .record(Record::Thinking {
                    event: thinking_capture::Event {
                        channel_ordinal: 1,
                        elapsed_ms: 30,
                        event,
                    },
                })
                .unwrap();
        }
        let audio = AudioEvidence {
            decoded_non_silent_frames: 4800,
            decoded_non_silent_seconds: 0.2,
            bytes_received: 3000,
            packets_received: 30,
            ..Default::default()
        };
        journal
            .record(Record::Exchange {
                exchange: 1,
                channel: 1,
                stage: Stage::HistoricalRecall,
                start_event_index: 8,
                input_timeout_ms: 120_000,
                response_timeout_ms: 90_000,
                baseline: AudioEvidence::default(),
            })
            .unwrap();
        for (direction, delta) in [
            (Direction::Input, "What was my historical vault phrase?"),
            (
                Direction::Output,
                "I don't know yet credential-fixture-not-a-real-key",
            ),
        ] {
            journal
                .native(
                    1,
                    NativeRecord::Transcript {
                        direction,
                        delta: delta.into(),
                        event_index: 9,
                        browser_ms: 42.0,
                        provider_start_ms: Some(40.0),
                        audio,
                    },
                )
                .unwrap();
        }
        journal
            .record(Record::ExchangeEnd {
                exchange: 1,
                matched: false,
                audio,
            })
            .unwrap();
        journal.channel(1, ChannelAction::Closed).unwrap();
        journal.channel(2, ChannelAction::OpenRequested).unwrap();
        journal
            .record(Record::Job {
                job: 1,
                action: JobAction::Cancelled,
            })
            .unwrap();
    }

    fn assert_retained(journal: &Journal) {
        let text = std::fs::read_to_string(journal.path()).unwrap();
        assert!(text.contains("amber otter copper"));
        assert!(text.contains("[REDACTED_CREDENTIAL]"));
        for forbidden in [
            "credential-fixture-not-a-real-key",
            "FORBIDDEN_SYSTEM",
            "FORBIDDEN_REASONING",
            "Do not run anything",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        let records = records(journal);
        assert!(
            records
                .windows(2)
                .all(|pair| pair[0].sequence + 1 == pair[1].sequence)
        );
        assert!(
            records
                .iter()
                .any(|row| matches!(&row.record, Record::Source {
            canonical_cursor: 3, projection_sha256, facts, ..
        } if !projection_sha256.is_empty() && facts.len() == 2))
        );
        assert!(records.iter().any(|row| matches!(&row.record,
            Record::Thinking { event: thinking_capture::Event {
                event: thinking_capture::EventKind::ThinkingAppended {
                    client_event_id: Some(id), matched_owned: true, accepted: true,
                }, ..
            }} if id == "owned-1"
        )));
        assert!(records.iter().any(|row| matches!(&row.record,
            Record::Native { record: NativeRecord::Transcript { audio, .. }, .. }
                if audio.decoded_non_silent_frames == 4800
        )));
        assert!(records.iter().any(|row| matches!(
            &row.record,
            Record::Outcome {
                last_stage: Stage::HistoricalRecall,
                ..
            }
        )));
    }

    #[test]
    fn forced_failure_retains_whitelisted_evidence_after_scenario_cleanup() {
        let root = root();
        let scenario = tempfile::Builder::new().tempdir_in(root.path()).unwrap();
        let journal = Journal::at(
            &root.path().join("retained"),
            "amber otter copper".into(),
            vec!["credential-fixture-not-a-real-key".into()],
            Limits::default(),
        )
        .unwrap();
        assert!(!journal.path().starts_with(scenario.path()));
        required_evidence(&journal);
        journal.finish(Outcome::Failed).unwrap();
        drop(scenario);
        assert_retained(&journal);
    }

    #[tokio::test]
    async fn cancellation_flushes_before_tempdir_and_owner_drop() {
        let root = root();
        let journal = Journal::at(
            &root.path().join("retained"),
            "amber otter copper".into(),
            vec!["credential-fixture-not-a-real-key".into()],
            Limits::default(),
        )
        .unwrap();
        let (ready, started) = tokio::sync::oneshot::channel();
        let copy = journal.clone();
        let scenario = tempfile::Builder::new().tempdir_in(root.path()).unwrap();
        let scenario_path = scenario.path().to_owned();
        let task = tokio::spawn(async move {
            let _scenario = scenario;
            let _guard = FailureGuard(copy.clone());
            required_evidence(&copy);
            ready.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        started.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!scenario_path.exists());
        assert_retained(&journal);
        assert!(records(&journal).iter().any(|row| matches!(
            row.record,
            Record::Outcome {
                outcome: Outcome::CancelledOrPanicked,
                ..
            }
        )));
    }

    #[test]
    fn every_bound_is_visible_and_can_never_finish_as_success() {
        for (fault, limits, text) in [
            (
                Fault::StringLimit,
                Limits {
                    string_bytes: 128,
                    ..Default::default()
                },
                "x".repeat(129),
            ),
            (
                Fault::RecordBytesLimit,
                Limits {
                    record_bytes: 512,
                    ..Default::default()
                },
                "x".repeat(1024),
            ),
            (
                Fault::FileBytesLimit,
                Limits {
                    file_bytes: 8192,
                    ..Default::default()
                },
                "x".repeat(4096),
            ),
            (
                Fault::RecordLimit,
                Limits {
                    records: 5,
                    ..Default::default()
                },
                "short".into(),
            ),
        ] {
            let root = root();
            let journal =
                Journal::at(root.path(), "amber otter copper".into(), Vec::new(), limits).unwrap();
            let mut failed = false;
            for _ in 0..8 {
                let result = journal.record(Record::Summary {
                    job: 1,
                    text: text.clone(),
                    expected_fact_present: false,
                    input_tokens: 1,
                    output_tokens: 1,
                });
                if result.is_err() {
                    assert_eq!(result, Err(fault));
                    failed = true;
                    break;
                }
            }
            assert!(failed);
            assert_eq!(journal.finish(Outcome::Passed), Err(fault));
            let rows = records(&journal);
            assert!(rows.iter().any(
                |row| matches!(row.record, Record::Fault { fault: actual } if actual == fault)
            ));
            assert!(!rows.iter().any(|row| matches!(
                row.record,
                Record::Outcome {
                    outcome: Outcome::Passed,
                    ..
                }
            )));
            assert!(rows.len() <= limits.records);
            assert!(std::fs::metadata(journal.path()).unwrap().len() <= limits.file_bytes as u64);
        }
    }

    #[test]
    fn browser_overflow_and_late_records_invalidate_the_journal() {
        let root = root();
        let journal = Journal::at(
            root.path(),
            "amber otter copper".into(),
            Vec::new(),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(
            journal.native(
                1,
                NativeRecord::Fault {
                    fault: BrowserFault::QueueLimit
                }
            ),
            Err(Fault::BrowserQueueLimit)
        );
        assert_eq!(
            journal.finish(Outcome::Passed),
            Err(Fault::BrowserQueueLimit)
        );
        let other = root.path().join("late");
        let journal = Journal::at(
            &other,
            "amber otter copper".into(),
            Vec::new(),
            Limits::default(),
        )
        .unwrap();
        journal.finish(Outcome::Failed).unwrap();
        assert_eq!(
            journal.record(Record::Stage {
                stage: Stage::Finished
            }),
            Err(Fault::AfterFinish)
        );
        assert_eq!(journal.check(), Err(Fault::AfterFinish));
    }

    #[test]
    fn source_selection_excludes_injected_and_compaction_rows() {
        let mut compacted = meerkat_core::UserMessage::text("amber otter copper");
        compacted.transcript_role = meerkat_core::types::TranscriptUserRole::CompactionSummary;
        let messages = [
            meerkat_core::Message::User(meerkat_core::UserMessage::injected_context(
                "amber otter copper",
            )),
            meerkat_core::Message::User(compacted),
        ];
        assert!(selected_source_facts(&messages, "amber otter copper").is_empty());
    }

    #[test]
    fn redaction_only_removes_the_explicit_known_credential_values() {
        let root = root();
        let secrets = ["known-key-one", "known-key-two", "known-key-three"];
        let journal = Journal::at(
            root.path(),
            "amber otter copper".into(),
            secrets.iter().map(|value| (*value).to_owned()).collect(),
            Limits::default(),
        )
        .unwrap();
        journal.record(Record::Summary {
            job: 1,
            text: "amber otter copper known-key-one known-key-two known-key-three sk-synthetic-fact".into(),
            expected_fact_present: true,
            input_tokens: 1,
            output_tokens: 1,
        }).unwrap();
        journal.finish(Outcome::Failed).unwrap();
        let text = std::fs::read_to_string(journal.path()).unwrap();
        for secret in secrets {
            assert!(!text.contains(secret));
        }
        assert!(text.contains("amber otter copper"));
        assert!(text.contains("sk-synthetic-fact"));
        assert_eq!(text.matches("[REDACTED_CREDENTIAL]").count(), 3);
    }
}
