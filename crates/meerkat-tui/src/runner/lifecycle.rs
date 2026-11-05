//! Which session the TUI opens and switches to: a fresh one, `--resume
//! <ref>`, `-c` (the workspace's most recent), `/new` and `/resume`; plus
//! the transcript and status seeding of a resumed session.

use meerkat_core::{AssistantBlock, Message, Session, SessionMeta, TranscriptUserRole};

use super::Runner;
use crate::bootstrap::Runtime;
use crate::cli::Args;
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

/// UI messages that show `session` (status bar, transcript, cost); the
/// transcript and cost come from `transcript` when resuming.
pub fn session_messages(session: &SessionHandle, transcript: Option<&Session>) -> Vec<Msg> {
    let mut msgs = vec![
        Msg::Status(StatusUpdate::Session(session.id().to_string())),
        Msg::Status(StatusUpdate::Model {
            provider: session.provider().as_str().to_string(),
            model: session.model().to_string(),
        }),
    ];
    match transcript {
        Some(transcript) => {
            let usage = transcript.total_usage();
            msgs.push(Msg::Status(StatusUpdate::Tokens {
                input: usage.input_tokens,
                output: usage.output_tokens,
            }));
            // Per-model cost sidecars arrive with the cost phase (P4); until
            // then the resumed totals are derived and marked approximate.
            msgs.push(Msg::Status(StatusUpdate::CostTotals(SessionCost::derived(
                session.model(),
                &usage,
            ))));
            msgs.push(Msg::ResetTranscript(blocks_from_session(transcript)));
        }
        None => {
            msgs.push(Msg::Status(StatusUpdate::Tokens {
                input: 0,
                output: 0,
            }));
            msgs.push(Msg::Status(
                StatusUpdate::CostTotals(SessionCost::default()),
            ));
            msgs.push(Msg::ResetTranscript(Vec::new()));
        }
    }
    msgs
}

impl Runner {
    /// `/new` (`/clear`): open a fresh session and retire the current one.
    pub(super) async fn new_session(&mut self) {
        if self.refuse_switch_while_busy() {
            return;
        }
        match SessionHandle::create(&self.runtime, self.template.options()).await {
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
        let opened = match SessionHandle::resume(&self.runtime, &id, self.template.options()).await
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
        self.follow_events().await;
        if let Err(err) = previous.retire().await {
            tracing::warn!(error = %err, session = %previous.id(), "could not retire the previous session");
        }
        for msg in session_messages(&self.session, transcript.as_ref()) {
            self.send(msg);
        }
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
