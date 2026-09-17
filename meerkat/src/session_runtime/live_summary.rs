//! Read-only, bounded summary production for a channel's exact opening snapshot.
//!
//! This owner mints the summary/provenance pair. Hosts supply content production,
//! not a cursor or a purported canonical summary. No transcript row is committed.

use std::sync::Arc;
use std::time::Duration;

use meerkat_core::{CanonicalContextRevision, Message, Session, SessionId, SessionLlmIdentity};
use meerkat_llm_core::realtime_session::RealtimeSessionOpenConfig;

/// Content-only producer. Treat transcript instructions as source data, not as
/// instructions to execute; do not run tools or mutate the source session.
#[async_trait::async_trait]
pub trait LiveContextSummarizer: Send + Sync {
    async fn summarize(
        &self,
        snapshot: LiveContextSummarySnapshot<'_>,
    ) -> Result<String, LiveContextSummaryError>;
}

/// Current model-boundary context plus document-owned unmeasured voice
/// observations, borrowed from one exact owner snapshot.
pub struct LiveContextSummarySnapshot<'a> {
    session_id: &'a SessionId,
    messages: &'a [Message],
    llm_identity: &'a SessionLlmIdentity,
    canonical_message_cursor: u64,
    max_output_bytes: usize,
}

impl LiveContextSummarySnapshot<'_> {
    pub fn session_id(&self) -> &SessionId {
        self.session_id
    }

    pub fn messages(&self) -> &[Message] {
        self.messages
    }

    /// Existing text identity, including its exact configured auth binding.
    /// A host may use its factory to build a separate, tool-free summary client.
    pub fn llm_identity(&self) -> &SessionLlmIdentity {
        self.llm_identity
    }

    pub fn canonical_message_cursor(&self) -> u64 {
        self.canonical_message_cursor
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }
}

/// Host opt-in policy. Both sizes are UTF-8 bytes (input is serialized JSON);
/// overflow refuses rather than selecting an unannounced partial window.
#[derive(Clone)]
pub struct LiveContextSummaryPolicy {
    summarizer: Arc<dyn LiveContextSummarizer>,
    max_input_bytes: usize,
    max_output_bytes: usize,
    timeout: Duration,
}

impl LiveContextSummaryPolicy {
    pub fn new(
        summarizer: Arc<dyn LiveContextSummarizer>,
        max_input_bytes: usize,
        max_output_bytes: usize,
        timeout: Duration,
    ) -> Result<Self, LiveContextSummaryError> {
        if max_input_bytes == 0 || max_output_bytes == 0 || timeout.is_zero() {
            return Err(LiveContextSummaryError::InvalidBounds);
        }
        Ok(Self {
            summarizer,
            max_input_bytes,
            max_output_bytes,
            timeout,
        })
    }

    pub(crate) async fn summarize(
        &self,
        session: Session,
        config: &RealtimeSessionOpenConfig,
        source_reader: Arc<dyn LiveSummarySource>,
    ) -> Result<LiveContextSummary, LiveContextSummaryError> {
        let mut size = BoundedSize {
            remaining: self.max_input_bytes,
            exceeded: false,
        };
        if let Err(error) = serde_json::to_writer(&mut size, config.seed_messages()) {
            return Err(if size.exceeded {
                LiveContextSummaryError::InputTooLarge {
                    max_bytes: self.max_input_bytes,
                }
            } else {
                LiveContextSummaryError::Serialization(error)
            });
        }
        let revision = session.canonical_context_revision()?;
        let projection_digest = LiveContextSummarySourceDigest(
            meerkat_core::session::transcript_messages_digest(config.seed_messages())?,
        );
        let rewrite_generation = session.transcript_rewrite_generation()?;
        let text = tokio::time::timeout(
            self.timeout,
            self.summarizer.summarize(LiveContextSummarySnapshot {
                session_id: session.id(),
                messages: config.seed_messages(),
                llm_identity: &config.llm_identity,
                canonical_message_cursor: config.canonical_message_cursor(),
                max_output_bytes: self.max_output_bytes,
            }),
        )
        .await
        .map_err(|_| LiveContextSummaryError::TimedOut)??;
        if text.len() > self.max_output_bytes {
            return Err(LiveContextSummaryError::OutputTooLarge {
                max_bytes: self.max_output_bytes,
            });
        }
        if text.trim().is_empty() {
            return Err(LiveContextSummaryError::Empty);
        }
        Ok(LiveContextSummary {
            source: Arc::new(session),
            source_identity: config.llm_identity.clone(),
            revision,
            projection_digest,
            rewrite_generation,
            text,
            source_reader,
        })
    }
}

/// Sealed factual content and its exact source snapshot. Only the shared
/// summary owner can construct this value; the callback cannot choose a cursor.
#[derive(Clone)]
pub struct LiveContextSummary {
    source: Arc<Session>,
    source_identity: SessionLlmIdentity,
    revision: CanonicalContextRevision,
    projection_digest: LiveContextSummarySourceDigest,
    rewrite_generation: u64,
    text: String,
    source_reader: Arc<dyn LiveSummarySource>,
}

/// Channel-scoped, read-only provenance retained after source snapshot custody
/// is released. It is not a canonical transcript row or admission authority.
#[derive(Clone)]
pub struct LiveContextSummaryProvenance {
    source_revision: CanonicalContextRevision,
    source_projection_digest: LiveContextSummarySourceDigest,
    canonical_message_cursor: u64,
    text: String,
}

impl LiveContextSummaryProvenance {
    pub fn source_revision(&self) -> &CanonicalContextRevision {
        &self.source_revision
    }

    pub fn canonical_message_cursor(&self) -> u64 {
        self.canonical_message_cursor
    }

    /// Covers the complete summarized projection, including retained
    /// observation data that does not advance the canonical message cursor.
    pub fn source_projection_digest(&self) -> &LiveContextSummarySourceDigest {
        &self.source_projection_digest
    }

    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LiveContextSummarySourceDigest(String);

impl LiveContextSummarySourceDigest {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for LiveContextSummarySourceDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LiveContextSummarySourceDigest([REDACTED])")
    }
}

impl std::fmt::Debug for LiveContextSummaryProvenance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LiveContextSummaryProvenance([REDACTED])")
    }
}

impl std::fmt::Debug for LiveContextSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveContextSummary")
            .field("source", &"[REDACTED]")
            .field("text", &"[REDACTED]")
            .finish()
    }
}

impl LiveContextSummary {
    pub(crate) fn provenance(&self) -> LiveContextSummaryProvenance {
        LiveContextSummaryProvenance {
            source_revision: self.revision.clone(),
            source_projection_digest: self.projection_digest.clone(),
            canonical_message_cursor: self.canonical_message_cursor(),
            text: self.text.clone(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn session_id(&self) -> &SessionId {
        self.source.id()
    }

    pub fn canonical_message_cursor(&self) -> u64 {
        self.source.messages().len() as u64
    }

    pub fn source_revision(&self) -> &CanonicalContextRevision {
        &self.revision
    }

    /// Recheck the exact source prefix at the deferred provider boundary.
    /// Appends after the summary was accepted are caught up by the ordinary
    /// live-context owner; rewrites, replacement bodies and model/auth changes
    /// invalidate this opening snapshot even when the row count is unchanged.
    pub(crate) async fn validate_provider_source(&self) -> Result<(), LiveContextSummaryError> {
        let (current, identity) = self.source_reader.read(self.session_id()).await?;
        if current.id() != self.source.id()
            || current.messages().get(..self.source.messages().len())
                != Some(self.source.messages())
            || current.transcript_rewrite_generation()? != self.rewrite_generation
            || !current
                .live_unmeasured_assistant_observations()
                .eq(self.source.live_unmeasured_assistant_observations())
            || identity != self.source_identity
        {
            return Err(LiveContextSummaryError::StaleSnapshot);
        }
        Ok(())
    }

    pub(super) fn validate_current(
        &self,
        session: &Session,
        identity: &SessionLlmIdentity,
    ) -> Result<(), LiveContextSummaryError> {
        if session.id() != self.source.id()
            || session.canonical_context_revision()? != self.revision
            || session.transcript_rewrite_generation()? != self.rewrite_generation
            || session.messages().len() as u64 != self.canonical_message_cursor()
            || !session
                .live_unmeasured_assistant_observations()
                .eq(self.source.live_unmeasured_assistant_observations())
            || identity != &self.source_identity
        {
            return Err(LiveContextSummaryError::StaleSnapshot);
        }
        Ok(())
    }

    pub(crate) fn validate_projection(
        &self,
        session_id: &SessionId,
        config: &RealtimeSessionOpenConfig,
    ) -> Result<(), LiveContextSummaryError> {
        if session_id != self.source.id()
            || config.canonical_message_cursor() != self.canonical_message_cursor()
            || config.transcript_rewrite_generation != self.rewrite_generation
            || config.seed_messages() != self.source.messages_for_live_context()
            || config.canonical_system_messages_ref()
                != RealtimeSessionOpenConfig::canonical_system_messages(self.source.messages())
        {
            return Err(LiveContextSummaryError::ConflictingProjection);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveContextSummaryError {
    #[error("live summary byte bounds and timeout must be nonzero")]
    InvalidBounds,
    #[error("a generated full-window summary cannot also select a replay seed window")]
    ConflictingSeedPolicy,
    #[error("live summary input exceeds {max_bytes} bytes")]
    InputTooLarge { max_bytes: usize },
    #[error("live summary output exceeds {max_bytes} bytes")]
    OutputTooLarge { max_bytes: usize },
    #[error("live summary producer timed out")]
    TimedOut,
    #[error("live summary producer returned empty content")]
    Empty,
    #[error("live summary producer failed: {0}")]
    Producer(String),
    #[error("live summary source snapshot is no longer current")]
    StaleSnapshot,
    #[error("live summary and opening projection belong to different snapshots")]
    ConflictingProjection,
    #[error("prepared live provider does not support snapshot summaries")]
    Unsupported,
    #[error("live summary source serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("live summary source read failed: {0}")]
    Session(#[from] meerkat_core::service::SessionError),
}

#[async_trait::async_trait]
pub(crate) trait LiveSummarySource: Send + Sync {
    async fn read(
        &self,
        id: &SessionId,
    ) -> Result<(Session, SessionLlmIdentity), LiveContextSummaryError>;
}

pub(super) struct ServiceLiveSummarySource<B: crate::SessionAgentBuilder>(
    pub Arc<crate::PersistentSessionService<B>>,
);

#[async_trait::async_trait]
impl<B: crate::SessionAgentBuilder + 'static> LiveSummarySource for ServiceLiveSummarySource<B> {
    async fn read(
        &self,
        id: &SessionId,
    ) -> Result<(Session, SessionLlmIdentity), LiveContextSummaryError> {
        Ok((
            self.0.export_realtime_refresh_session_snapshot(id).await?,
            self.0.live_session_llm_identity(id).await?,
        ))
    }
}

struct BoundedSize {
    remaining: usize,
    exceeded: bool,
}

impl std::io::Write for BoundedSize {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.remaining {
            self.exceeded = true;
            return Err(std::io::Error::other("live summary input bound exceeded"));
        }
        self.remaining -= bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Source(Session, SessionLlmIdentity);

    #[async_trait::async_trait]
    impl LiveSummarySource for Source {
        async fn read(
            &self,
            _: &SessionId,
        ) -> Result<(Session, SessionLlmIdentity), LiveContextSummaryError> {
            Ok((self.0.clone(), self.1.clone()))
        }
    }

    impl LiveContextSummaryPolicy {
        async fn produce(
            &self,
            session: Session,
            config: &RealtimeSessionOpenConfig,
        ) -> Result<LiveContextSummary, LiveContextSummaryError> {
            let reader = Arc::new(Source(session.clone(), config.llm_identity.clone()));
            self.summarize(session, config, reader).await
        }
    }

    struct Producer {
        calls: AtomicUsize,
        text: String,
        delay: Duration,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl LiveContextSummarizer for Producer {
        async fn summarize(
            &self,
            snapshot: LiveContextSummarySnapshot<'_>,
        ) -> Result<String, LiveContextSummaryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(snapshot.messages().len(), 2);
            assert_eq!(snapshot.canonical_message_cursor(), 2);
            assert_eq!(snapshot.llm_identity().model, "gpt-5.5");
            tokio::time::sleep(self.delay).await;
            if self.fail {
                return Err(LiveContextSummaryError::Producer("model failed".into()));
            }
            Ok(self.text.clone())
        }
    }

    fn source(text: &str) -> (Session, RealtimeSessionOpenConfig) {
        let mut session = Session::new();
        session.append_system_message("background instructions");
        session.push(Message::User(meerkat_core::types::UserMessage::text(text)));
        let identity = SessionLlmIdentity {
            provider: meerkat_core::Provider::OpenAI,
            model: "gpt-5.5".into(),
            auth_binding: None,
            provider_params: None,
            self_hosted_server_id: None,
        };
        let config = RealtimeSessionOpenConfig::for_open_from_messages(
            meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            identity,
            Vec::new(),
            session.messages_for_live_context(),
            session.messages(),
        )
        .unwrap();
        (session, config)
    }

    fn producer(text: &str) -> Arc<Producer> {
        Arc::new(Producer {
            calls: AtomicUsize::new(0),
            text: text.into(),
            delay: Duration::ZERO,
            fail: false,
        })
    }

    fn observe_voice(session: &mut Session, item: &str, text: &str) {
        let channel = meerkat_core::LiveChannelId::new("summary-observations");
        let interaction = meerkat_core::InteractionId::new();
        session
            .admit_live_assistant_playback_target(&channel, interaction, item, item, 0)
            .unwrap();
        session.append_realtime_transcript_event(
            meerkat_core::RealtimeTranscriptEvent::AssistantPlaybackTerminalSettled {
                channel_id: channel.to_string(),
                interaction_id: interaction,
                response_id: item.into(),
                item_id: item.into(),
                content_index: 0,
                settlement: Box::new(meerkat_core::LiveAssistantPlaybackSettlement {
                    evidence:
                        meerkat_core::LiveAssistantPlaybackEvidence::ProviderManagedUnmeasured(
                            text.into(),
                        ),
                    authoritative_final: None,
                    completion: None,
                    observed_after_message_count: Some(session.messages().len() as u64),
                }),
            },
        );
    }

    struct ObservationProducer;

    #[async_trait::async_trait]
    impl LiveContextSummarizer for ObservationProducer {
        async fn summarize(
            &self,
            snapshot: LiveContextSummarySnapshot<'_>,
        ) -> Result<String, LiveContextSummaryError> {
            assert!(snapshot.messages().iter().any(|message| matches!(message,
                Message::User(user) if user.transcript_role.is_injected_context()
                    && user.text_content().contains("voice blueprint discussion")
                    && user.text_content().contains("UNMEASURED")
            )));
            assert_eq!(snapshot.canonical_message_cursor(), 2);
            Ok("The voice blueprint discussion was observed; playback is unmeasured.".into())
        }
    }

    #[tokio::test]
    async fn summary_binds_observed_dialogue_beyond_the_canonical_cursor() {
        let (mut session, config) = source("Compare tables.");
        let canonical_revision = session.canonical_context_revision().unwrap();
        observe_voice(&mut session, "first", "voice blueprint discussion");
        let config = RealtimeSessionOpenConfig::for_open_from_messages(
            config.turning_mode,
            config.llm_identity.clone(),
            Vec::new(),
            session.messages_for_live_context(),
            session.messages(),
        )
        .unwrap();
        let policy = LiveContextSummaryPolicy::new(
            Arc::new(ObservationProducer),
            8192,
            1024,
            Duration::from_secs(1),
        )
        .unwrap();
        let summary = policy.produce(session.clone(), &config).await.unwrap();
        assert_eq!(
            canonical_revision,
            session.canonical_context_revision().unwrap()
        );
        assert_eq!(summary.canonical_message_cursor(), 2);
        assert_eq!(config.seed_messages().len(), 3);
        summary.validate_projection(session.id(), &config).unwrap();
        summary.validate_provider_source().await.unwrap();
        let provenance = summary.provenance();
        observe_voice(&mut session, "second", "Another observed detail.");
        assert_eq!(
            canonical_revision,
            session.canonical_context_revision().unwrap()
        );
        assert!(matches!(
            summary.validate_current(&session, &config.llm_identity),
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
        let delayed = policy
            .summarize(
                (*summary.source).clone(),
                &config,
                Arc::new(Source(session.clone(), config.llm_identity.clone())),
            )
            .await
            .unwrap();
        assert!(matches!(
            delayed.validate_provider_source().await,
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
        let newer_config = RealtimeSessionOpenConfig::for_open_from_messages(
            config.turning_mode,
            config.llm_identity,
            Vec::new(),
            session.messages_for_live_context(),
            session.messages(),
        )
        .unwrap();
        let newer = policy.produce(session, &newer_config).await.unwrap();
        assert_ne!(
            provenance.source_projection_digest(),
            newer.provenance().source_projection_digest()
        );
        assert_eq!(provenance.source_revision(), newer.source_revision());
    }

    #[tokio::test]
    async fn unmeasured_observation_change_invalidates_summary_without_canonical_row_change() {
        let (mut session, config) = source("canonical user");
        let policy = LiveContextSummaryPolicy::new(
            producer("factual summary"),
            4096,
            512,
            Duration::from_secs(1),
        )
        .unwrap();
        let mut summary = policy.produce(session.clone(), &config).await.unwrap();
        let revision = session.canonical_context_revision().unwrap();
        observe_voice(&mut session, "observed-item", "unmeasured speech");
        assert_eq!(session.canonical_context_revision().unwrap(), revision);
        assert_eq!(session.messages().len(), 2);
        assert!(matches!(
            summary.validate_current(&session, &config.llm_identity),
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
        summary.source_reader = Arc::new(Source(session, config.llm_identity.clone()));
        assert!(matches!(
            summary.validate_provider_source().await,
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
    }

    #[tokio::test]
    async fn summary_is_invoked_once_bounded_and_sealed_to_exact_body() {
        let (session, config) = source("Compare tables.");
        let original = serde_json::to_vec(&session).unwrap();
        let producer = producer("Comparing tables.");
        let policy =
            LiveContextSummaryPolicy::new(producer.clone(), 4096, 100, Duration::from_secs(1))
                .unwrap();
        let summary = policy.produce(session.clone(), &config).await.unwrap();
        assert_eq!(producer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(summary.text(), "Comparing tables.");
        assert_eq!(
            summary.source_revision(),
            &session.canonical_context_revision().unwrap()
        );
        summary
            .validate_current(&session, &config.llm_identity)
            .unwrap();
        summary.validate_projection(session.id(), &config).unwrap();
        assert_eq!(serde_json::to_vec(&session).unwrap(), original);
        assert!(!format!("{summary:?}").contains("Comparing tables"));

        let mut changed = Session::with_id(session.id().clone());
        changed.append_system_message("background instructions");
        changed.push(Message::User(meerkat_core::types::UserMessage::text(
            "Delete tables.",
        )));
        assert_eq!(changed.messages().len(), session.messages().len());
        assert!(matches!(
            summary.validate_current(&changed, &config.llm_identity),
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
        let mut appended = session.clone();
        appended.push(Message::User(meerkat_core::types::UserMessage::text(
            "One more.",
        )));
        assert!(matches!(
            summary.validate_current(&appended, &config.llm_identity),
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
        let changed_config = config
            .clone()
            .with_seed_messages(changed.messages().to_vec())
            .unwrap();
        assert!(matches!(
            summary.validate_projection(session.id(), &changed_config),
            Err(LiveContextSummaryError::ConflictingProjection)
        ));
        assert!(matches!(
            summary.validate_projection(&SessionId::new(), &config),
            Err(LiveContextSummaryError::ConflictingProjection)
        ));
    }

    #[tokio::test]
    async fn provider_source_witness_accepts_only_append_only_catch_up() {
        let (session, config) = source("Compare tables.");
        let policy = LiveContextSummaryPolicy::new(
            producer("Comparing tables."),
            4096,
            100,
            Duration::from_secs(1),
        )
        .unwrap();
        let mut appended = session.clone();
        appended.push(Message::User(meerkat_core::types::UserMessage::text(
            "Later text.",
        )));
        let summary = policy
            .summarize(
                session.clone(),
                &config,
                Arc::new(Source(appended, config.llm_identity.clone())),
            )
            .await
            .unwrap();
        summary.validate_provider_source().await.unwrap();

        let mut replacement = Session::with_id(session.id().clone());
        replacement.append_system_message("background instructions");
        replacement.push(Message::User(meerkat_core::types::UserMessage::text(
            "Different body.",
        )));
        assert_eq!(replacement.messages().len(), session.messages().len());
        let summary = policy
            .summarize(
                session.clone(),
                &config,
                Arc::new(Source(replacement, config.llm_identity.clone())),
            )
            .await
            .unwrap();
        assert!(matches!(
            summary.validate_provider_source().await,
            Err(LiveContextSummaryError::StaleSnapshot)
        ));

        let mut new_identity = config.llm_identity.clone();
        new_identity.model = "gpt-5.4".into();
        let summary = policy
            .summarize(
                session.clone(),
                &config,
                Arc::new(Source(session, new_identity)),
            )
            .await
            .unwrap();
        assert!(matches!(
            summary.validate_provider_source().await,
            Err(LiveContextSummaryError::StaleSnapshot)
        ));
    }

    #[tokio::test]
    async fn input_bound_is_exact_serialized_bytes_and_never_truncates() {
        let (session, config) = source("Compare tables.");
        let size = serde_json::to_vec(config.seed_messages()).unwrap().len();
        let producer = producer("Facts");
        let rejected =
            LiveContextSummaryPolicy::new(producer.clone(), size - 1, 100, Duration::from_secs(1))
                .unwrap();
        assert!(matches!(
            rejected.produce(session.clone(), &config).await,
            Err(LiveContextSummaryError::InputTooLarge { .. })
        ));
        assert_eq!(producer.calls.load(Ordering::SeqCst), 0);
        let accepted =
            LiveContextSummaryPolicy::new(producer.clone(), size, 100, Duration::from_secs(1))
                .unwrap();
        accepted.produce(session, &config).await.unwrap();
        assert_eq!(producer.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn output_bytes_empty_failure_and_timeout_are_explicit() {
        let (session, config) = source("Compare tables.");
        let exact =
            LiveContextSummaryPolicy::new(producer("\u{e9}"), 4096, 2, Duration::from_secs(1))
                .unwrap();
        assert_eq!(
            exact
                .produce(session.clone(), &config)
                .await
                .unwrap()
                .text(),
            "\u{e9}"
        );
        for (text, limit, oversize) in [("abc", 2, true), ("   ", 3, false), ("\u{e9}", 1, true)] {
            let policy =
                LiveContextSummaryPolicy::new(producer(text), 4096, limit, Duration::from_secs(1))
                    .unwrap();
            let result = policy.produce(session.clone(), &config).await;
            if oversize {
                assert!(matches!(
                    result,
                    Err(LiveContextSummaryError::OutputTooLarge { .. })
                ));
            } else {
                assert!(matches!(result, Err(LiveContextSummaryError::Empty)));
            }
        }
        let timed = LiveContextSummaryPolicy::new(
            Arc::new(Producer {
                calls: AtomicUsize::new(0),
                text: "late".into(),
                delay: Duration::from_secs(60),
                fail: false,
            }),
            4096,
            100,
            Duration::from_millis(1),
        )
        .unwrap();
        assert!(matches!(
            timed.produce(session.clone(), &config).await,
            Err(LiveContextSummaryError::TimedOut)
        ));
        let failed = LiveContextSummaryPolicy::new(
            Arc::new(Producer {
                calls: AtomicUsize::new(0),
                text: String::new(),
                delay: Duration::ZERO,
                fail: true,
            }),
            4096,
            100,
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(matches!(
            failed.produce(session, &config).await,
            Err(LiveContextSummaryError::Producer(_))
        ));
    }

    #[test]
    fn zero_bounds_are_rejected() {
        for (input, output, timeout) in [
            (0, 1, Duration::from_secs(1)),
            (1, 0, Duration::from_secs(1)),
            (1, 1, Duration::ZERO),
        ] {
            assert!(matches!(
                LiveContextSummaryPolicy::new(producer("x"), input, output, timeout),
                Err(LiveContextSummaryError::InvalidBounds)
            ));
        }
    }
}
