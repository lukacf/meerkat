//! Which session the TUI opens and switches to: a fresh one, `--resume
//! <ref>`, `-c` (the workspace's most recent), `/new` and `/resume`; plus
//! the transcript and status seeding of a resumed session.

use meerkat_core::{
    AssistantBlock, Message, Session, SessionId, SessionMeta, TranscriptUserRole,
};

use super::cost::session_cost;
use super::{InputKind, Runner, Turn};
use crate::bootstrap::Runtime;
use crate::cli::Args;
use crate::endpoints::EndpointName;
use crate::error::TuiError;
use crate::session::{
    SessionHandle, SessionIdentity, SessionOptions, list_sessions, resolve_session_ref,
};
use crate::ui::{Block, Msg, NoticeLevel, SessionCost, StatusUpdate, ToolState, TurnFailure};

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

/// UI messages that show `session` (its `identity`, on `endpoint`, with
/// cost totals `cost`) in the status bar and transcript. A resumed
/// session's transcript comes from `transcript`; a fresh one clears the
/// transcript when `reset` is set. Without an endpoint the status shows the
/// provider.
pub fn session_messages(
    session: &SessionHandle,
    identity: &SessionIdentity,
    endpoint: Option<&EndpointName>,
    transcript: Option<&Session>,
    cost: SessionCost,
    reset: bool,
) -> Vec<Msg> {
    let usage = transcript.map(Session::total_usage).unwrap_or_default();
    let mut msgs = vec![
        Msg::Status(StatusUpdate::Session(session.id().to_string())),
        Msg::Status(StatusUpdate::Model {
            provider: endpoint.map_or_else(
                || identity.provider.as_str().to_string(),
                ToString::to_string,
            ),
            model: identity.model.clone(),
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

/// What the runner sends in place of a prompt that repeats the one a failed
/// turn already committed to the session.
pub(super) const RETRY_INPUT: &str = "(retrying the request above)";

/// The prompt of a turn that failed (not interrupted), in the session it
/// failed in. A failed run keeps its committed user message, so sending the
/// same text again would put it in the transcript twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FailedPrompt {
    session: SessionId,
    text: String,
}

impl FailedPrompt {
    /// The prompt of `turn` when it was the turn's only input and it failed.
    pub(super) fn of(session: &SessionId, turn: &Turn) -> Option<Self> {
        let [input] = turn.inputs.as_slice() else {
            return None;
        };
        match (&input.kind, &input.end) {
            (InputKind::Prompt { text }, Some(Err(TurnFailure::Failed(_))))
                if !turn.cancelled.contains(&input.input_id) =>
            {
                Some(Self {
                    session: session.clone(),
                    text: text.clone(),
                })
            }
            _ => None,
        }
    }

    /// `text` repeats this prompt in `session`.
    fn repeated_by(&self, session: &SessionId, text: &str) -> bool {
        &self.session == session && self.text.trim() == text.trim()
    }
}

/// The session's last conversational message is the user's `text`: nothing
/// answered it.
fn ends_with_user_text(session: &Session, text: &str) -> bool {
    let last = session.messages().iter().rev().find(|message| {
        !matches!(message, Message::System(_) | Message::SystemNotice(_))
    });
    matches!(
        last,
        Some(Message::User(user))
            if user.transcript_role == TranscriptUserRole::Conversational
                && user.text_content().trim() == text.trim()
    )
}

impl Runner {
    /// The input to send for the prompt `text`: [`RETRY_INPUT`] when `text`
    /// repeats the prompt of the turn that just failed and that prompt is
    /// still the session's last message; `None` sends `text` itself. Applies
    /// once: the failed prompt is forgotten either way.
    pub(super) async fn retry_input(&mut self, text: &str) -> Option<&'static str> {
        let failed = self.failed_prompt.take()?;
        if self.turn.is_active() || !failed.repeated_by(self.session.id(), text) {
            return None;
        }
        match self.session.read().await {
            Ok(session) if ends_with_user_text(&session, text) => {
                self.notice(
                    NoticeLevel::Info,
                    "your message is already in the transcript from the failed turn; retrying it",
                );
                Some(RETRY_INPUT)
            }
            Ok(_) => None,
            Err(err) => {
                tracing::warn!(error = %err, "cannot read the session to check for a retry");
                None
            }
        }
    }

    /// `/new` (`/clear`): open a fresh session and retire the current one.
    pub(super) async fn new_session(&mut self) {
        if self.refuse_switch_while_busy() {
            return;
        }
        let choice = self
            .current_choice()
            .and_then(|current| self.runtime.choice_for(&current).ok())
            .unwrap_or_else(|| self.runtime.choice.clone());
        let trigger = self.session_trigger();
        match SessionHandle::create(&self.runtime, self.template.options(Some(choice), trigger))
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
        let trigger = self.session_trigger();
        let opened =
            match SessionHandle::resume(&self.runtime, &id, self.template.options(None, trigger))
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
                if let Err(err) = opened.close().await {
                    tracing::warn!(error = %err, "could not retire the unread session");
                }
            }
        }
    }

    /// [`crate::ui::Effect::TrustProjectMcp`]: record the project
    /// `.rkat/mcp.toml`'s digest in the host trust record and reload the MCP
    /// set, so its servers start from the next session.
    pub(super) async fn trust_project_mcp(&mut self) {
        let path = crate::bootstrap::mcp::project_file(&self.runtime.scope);
        let recorded = self.runtime.approval.trust.lock().record_file(&path);
        if let Err(err) = recorded {
            self.notice(NoticeLevel::Error, format!("not trusted: {err}"));
            return;
        }
        let config = self.runtime.config.clone();
        match crate::bootstrap::mcp::load(
            &self.runtime.scope,
            &config,
            crate::bootstrap::mcp::ProjectMcpTrust::Recorded,
        )
        .await
        {
            Ok(loaded) => {
                self.runtime.mcp_servers = loaded.servers;
                self.notice(
                    NoticeLevel::Info,
                    format!(
                        "trusted {} as it is now; its servers start from the next session (/new or /resume)",
                        path.display()
                    ),
                );
            }
            Err(err) => self.notice(
                NoticeLevel::Error,
                format!("trusted {}, but it cannot be loaded: {err}", path.display()),
            ),
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

    /// Make `session` current: follow its events, leave the previous one
    /// (its waiters and interrupt tasks aborted, its approval state
    /// dropped) and reseed the UI.
    async fn switch_to(&mut self, session: SessionHandle, transcript: Option<Session>) {
        let previous = std::mem::replace(&mut self.session, session);
        self.compact_now_pending = false;
        self.tasks.abort_all();
        self.follow_events().await;
        if let Err(err) = previous.close().await {
            tracing::warn!(error = %err, session = %previous.id(), "could not retire the previous session");
        }
        self.refresh_identity().await;
        self.show_session(transcript.as_ref(), true);
        // A resumed session on another model than the current one.
        self.ensure_trigger().await;
    }

    /// Seed the UI with the current session: its endpoint (derived from its
    /// identity), status, cost totals (a resumed session's record,
    /// reconciled with its authoritative usage) and transcript.
    pub(super) fn show_session(&mut self, transcript: Option<&Session>, reset: bool) {
        if self.current_endpoint.is_none() {
            self.notice(
                NoticeLevel::Warn,
                format!(
                    "no endpoint of this run serves this session ({} model `{}`); /model switches it to one",
                    self.identity.provider.as_str(),
                    self.identity.model
                ),
            );
        }
        let cost = transcript.map_or_else(SessionCost::default, |transcript| {
            session_cost(
                &self.runtime.scope.realm_root(),
                transcript,
                &self.identity.model,
            )
        });
        for msg in session_messages(
            &self.session,
            &self.identity,
            self.current_endpoint.as_ref(),
            transcript,
            cost,
            reset,
        ) {
            self.send(msg);
        }
        self.push_context_window();
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
