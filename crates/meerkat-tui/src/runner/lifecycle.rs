//! Which session the TUI opens and switches to: a fresh one, `--resume
//! <ref>`, `-c` (the workspace's most recent), `/new` and `/resume`; plus
//! the transcript and status seeding of a resumed session.

use meerkat_core::{AssistantBlock, Message, Session, SessionMeta, TranscriptUserRole};

use super::Runner;
use super::cost::session_cost;
use crate::bootstrap::Runtime;
use crate::cli::Args;
use crate::endpoints::{ENDPOINT_REALM, EndpointRef, load_choice, restore_choice};
use crate::error::TuiError;
use crate::session::{SessionHandle, SessionOptions, list_sessions, resolve_session_ref};
use crate::ui::{Block, Msg, NoticeLevel, SessionCost, StatusUpdate, ToolState};

/// The session the TUI starts with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Start {
    /// A fresh session.
    New,
    /// Resume by reference (`last`, `~N`, id, id prefix).
    Resume(String),
}

impl Start {
    /// `-c` is `--resume last`; neither starts a fresh session.
    #[must_use]
    pub fn from_args(args: &Args) -> Self {
        if args.continue_last {
            Self::Resume("last".to_string())
        } else {
            args.resume.clone().map_or(Self::New, Self::Resume)
        }
    }
}

/// Open the start session. A resumed session comes back with its
/// committed transcript, for seeding the UI.
///
/// # Errors
///
/// [`TuiError::SessionRef`] when the reference does not resolve, otherwise
/// as [`SessionHandle::create`].
pub async fn open_start(
    runtime: &Runtime,
    options: SessionOptions,
    start: &Start,
) -> Result<(SessionHandle, Option<Session>), TuiError> {
    match start {
        Start::New => Ok((SessionHandle::create(runtime, options).await?, None)),
        Start::Resume(reference) => {
            let id = resolve_session_ref(&*runtime.store, reference).await?;
            let session = SessionHandle::resume(runtime, &id, options).await?;
            let transcript = session.read().await?;
            Ok((session, Some(transcript)))
        }
    }
}

/// UI messages that show `session` on `endpoint` (status bar, transcript,
/// cost totals `cost`). A resumed session's transcript comes from
/// `transcript`; a fresh one clears the transcript when `reset` is set.
pub fn session_messages(
    session: &SessionHandle,
    endpoint: &str,
    transcript: Option<&Session>,
    cost: SessionCost,
    reset: bool,
) -> Vec<Msg> {
    let usage = transcript.map(Session::total_usage).unwrap_or_default();
    let mut msgs = vec![
        Msg::Status(StatusUpdate::Session(session.id().to_string())),
        Msg::Status(StatusUpdate::Model {
            provider: endpoint.to_string(),
            model: session.model().to_string(),
        }),
        Msg::Status(StatusUpdate::Tokens {
            input: usage.input_tokens,
            output: usage.output_tokens,
        }),
        Msg::Status(StatusUpdate::CostTotals(cost)),
    ];
    match transcript {
        Some(transcript) => msgs.push(Msg::ResetTranscript(blocks_from_session(transcript))),
        None if reset => msgs.push(Msg::ResetTranscript(Vec::new())),
        None => {}
    }
    msgs
}

/// The endpoint a session runs on: a configured endpoint's binding names
/// it; else the session's sidecar, else its provider's built-in endpoint
/// (see [`restore_choice`]). Returns a notice when it had to fall back.
#[must_use]
pub fn session_endpoint(runtime: &Runtime, session: &SessionHandle) -> (String, Option<String>) {
    if let Some(binding) = session.auth_binding()
        && binding.realm.as_str() == ENDPOINT_REALM
        && runtime.endpoints.get(binding.binding.as_str()).is_some()
    {
        return (binding.binding.as_str().to_string(), None);
    }
    let realm = runtime.scope.realm_root();
    let stored = load_choice(&realm, &session.id().to_string())
        .filter(|stored| stored.model == session.model());
    let (choice, notice) = restore_choice(
        stored,
        Some((session.provider(), session.model().to_string())),
        runtime.endpoints.as_slice(),
        |name| runtime.endpoints.get(name).is_some(),
        &EndpointRef::new(runtime.choice.endpoint.clone(), session.model()),
    );
    (choice.endpoint, notice)
}

impl Runner {
    /// `/new` (`/clear`): open a fresh session and retire the current one.
    pub(super) async fn new_session(&mut self) {
        if self.refuse_switch_while_busy() {
            return;
        }
        let choice = self
            .runtime
            .choice_for(&self.current_choice())
            .unwrap_or_else(|_| self.runtime.choice.clone());
        let threshold = self.session_threshold();
        match SessionHandle::create(
            &self.runtime,
            self.template.options(Some(choice), threshold),
        )
        .await
        {
            Ok(session) => self.switch_to(session, None).await,
            Err(err) => self.notice(
                NoticeLevel::Error,
                format!("could not start a new session: {err}"),
            ),
        }
    }

    /// `/resume <ref>`: resume another session of the realm.
    pub(super) async fn resume_session(&mut self, reference: &str) {
        if self.refuse_switch_while_busy() {
            return;
        }
        let id = match resolve_session_ref(&*self.runtime.store, reference).await {
            Ok(id) => id,
            Err(err) => {
                self.notice(NoticeLevel::Warn, err.to_string());
                return;
            }
        };
        if &id == self.session.id() {
            self.notice(NoticeLevel::Info, "already in this session");
            return;
        }
        let threshold = self.session_threshold();
        let opened =
            match SessionHandle::resume(&self.runtime, &id, self.template.options(None, threshold))
                .await
            {
                Ok(session) => session,
                Err(err) => {
                    self.notice(
                        NoticeLevel::Error,
                        format!("could not resume {reference}: {err}"),
                    );
                    return;
                }
            };
        match opened.read().await {
            Ok(transcript) => self.switch_to(opened, Some(transcript)).await,
            Err(err) => {
                self.notice(
                    NoticeLevel::Error,
                    format!("could not read the resumed session: {err}"),
                );
                if let Err(err) = opened.retire().await {
                    tracing::warn!(error = %err, "could not retire the unread session");
                }
            }
        }
    }

    /// `/resume` without a reference: offer the realm's sessions.
    pub(super) async fn show_resume_picker(&mut self) {
        let store = std::sync::Arc::clone(&self.runtime.store);
        let ui_tx = self.ui_tx.clone();
        let msg = match list_sessions(&*store).await {
            Ok(sessions) => Msg::ShowResumePicker(sessions.iter().map(picker_item).collect()),
            Err(err) => Msg::Notice {
                level: NoticeLevel::Error,
                text: format!("could not list sessions: {err}"),
            },
        };
        let _ = ui_tx.send(msg);
    }

    fn refuse_switch_while_busy(&self) -> bool {
        let busy = self.turn.is_active();
        if busy {
            self.notice(
                NoticeLevel::Warn,
                "a turn is running; interrupt it first (Ctrl-C)",
            );
        }
        busy
    }

    /// Make `session` current: follow its events, retire the previous one
    /// and reseed the UI.
    async fn switch_to(&mut self, session: SessionHandle, transcript: Option<Session>) {
        let previous = std::mem::replace(&mut self.session, session);
        self.compact_armed = false;
        self.follow_events().await;
        if let Err(err) = previous.retire().await {
            tracing::warn!(error = %err, session = %previous.id(), "could not retire the previous session");
        }
        self.show_session(transcript.as_ref(), true);
        // A resumed session on another model than the current one.
        self.ensure_threshold().await;
    }

    /// Seed the UI with the current session: its endpoint, status, cost
    /// totals (the sidecar's for a resumed session) and transcript; record
    /// the choice in the sidecar.
    pub(super) fn show_session(&mut self, transcript: Option<&Session>, reset: bool) {
        let (endpoint, notice) = match transcript {
            Some(_) => session_endpoint(&self.runtime, &self.session),
            None => (self.current_endpoint.clone(), None),
        };
        self.current_endpoint = endpoint;
        if let Some(text) = notice {
            self.notice(NoticeLevel::Warn, text);
        }
        let cost = transcript.map_or_else(SessionCost::default, |transcript| {
            session_cost(
                &self.runtime.scope.realm_root(),
                transcript,
                self.session.model(),
            )
        });
        for msg in session_messages(
            &self.session,
            &self.current_endpoint,
            transcript,
            cost,
            reset,
        ) {
            self.send(msg);
        }
        self.push_context_window();
        self.save_current_choice();
    }
}

/// Transcript blocks for a resumed session: user text, assistant text and
/// tool calls. Tool results are summarized as completed calls; runtime
/// notices, injected context and compaction summaries are not shown.
#[must_use]
pub fn blocks_from_session(session: &Session) -> Vec<Block> {
    let mut blocks = Vec::new();
    for message in session.messages() {
        match message {
            Message::User(user) if user.transcript_role == TranscriptUserRole::Conversational => {
                let text = user.text_content();
                if !text.is_empty() {
                    blocks.push(Block::User { text });
                }
            }
            Message::BlockAssistant(assistant) => {
                for block in &assistant.blocks {
                    if let AssistantBlock::Text { text, .. } = block
                        && !text.is_empty()
                    {
                        blocks.push(Block::Assistant { md: text.clone() });
                    }
                }
                for call in assistant.tool_calls() {
                    let args = serde_json::from_str(call.args.get())
                        .unwrap_or_else(|_| serde_json::Value::String(call.args.get().to_string()));
                    blocks.push(restored_tool_block(call.name, args));
                }
            }
            _ => {}
        }
    }
    blocks
}

fn restored_tool_block(name: &str, args: serde_json::Value) -> Block {
    Block::Tool {
        name: name.to_string(),
        args,
        state: ToolState::Done {
            is_error: false,
            duration_ms: 0,
        },
        result_preview: None,
        diff: None,
        depth: 0,
        collapsed: true,
    }
}

fn picker_item(meta: &SessionMeta) -> (String, String) {
    let age = meta
        .updated_at
        .elapsed()
        .map_or_else(|_| "just now".to_string(), humanize);
    let id = meta.id.to_string();
    (
        id.clone(),
        format!(
            "{} · {age} ago · {} msgs",
            short_id(&id),
            meta.message_count
        ),
    )
}

fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn humanize(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;
    use meerkat_core::UserMessage;

    #[test]
    fn start_follows_the_flags() {
        let parse = |argv: &[&str]| Args::try_parse_from(argv).unwrap();
        assert_eq!(Start::from_args(&parse(&["rkat-tui"])), Start::New);
        assert_eq!(
            Start::from_args(&parse(&["rkat-tui", "-c"])),
            Start::Resume("last".into())
        );
        assert_eq!(
            Start::from_args(&parse(&["rkat-tui", "--resume", "~2"])),
            Start::Resume("~2".into())
        );
    }

    #[test]
    fn humanize_buckets() {
        use std::time::Duration;
        assert_eq!(humanize(Duration::from_secs(5)), "5s");
        assert_eq!(humanize(Duration::from_mins(2)), "2m");
        assert_eq!(humanize(Duration::from_hours(2)), "2h");
        assert_eq!(humanize(Duration::from_hours(48)), "2d");
    }

    #[test]
    fn resumed_blocks_skip_injected_context() {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text("hello")));
        session.push(Message::User(UserMessage::injected_context("ambient")));
        let blocks = blocks_from_session(&session);
        assert_eq!(
            blocks,
            vec![Block::User {
                text: "hello".into()
            }]
        );
    }
}
