//! Context upkeep: `/remember` and the compaction trigger.
//!
//! `/remember <text>` (or a single-line `#text`) appends `- <text>` to the
//! workspace `AGENTS.md` (created with a header when missing) and pushes
//! the line into the live session as durable system context
//! (`append_system_context`, idempotent per session and line), so the model
//! reads it from its next request; new sessions load it with `AGENTS.md`.
//!
//! Compaction is native (see [`crate::ui::compaction`]): every session build
//! carries the policy's trigger for the model's context window
//! ([`Runner::session_trigger`]). Upstream reads the trigger when the agent
//! is built and exposes no call to change it on a live session or to
//! compact on demand, so every change goes through one workaround,
//! [`Runner::rebuild_session_workaround`]: retire the idle session and
//! resume it with the new trigger. It runs only on what the user does: a
//! model switch or a resume that moves the trigger ([`Runner::ensure_trigger`]),
//! `/compact now` (a one-turn trigger, then back to the policy's after that
//! turn), and a model switch that needed a new profile. A background list
//! fetch never rebuilds. The upstream asks (an on-demand compaction call, a
//! live trigger update) are recorded in `.rct/rkat-tui/evidence.md`.

use sha2::{Digest, Sha256};

use super::Runner;
use crate::bootstrap::instructions::{
    append_remember_line, remember_line, remember_target, remembered_context,
};
use crate::error::TuiError;
use crate::session::SessionHandle;
use crate::ui::{COMPACT_NOW, CompactFailure, CompactionTrigger, Msg, NoticeLevel};

/// Why the session is rebuilt (see [`Runner::rebuild_session_workaround`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RebuildReason {
    /// The compaction trigger moved (model switch, resume onto another
    /// model). Upstream ask: a live trigger update.
    CompactionTrigger,
    /// `/compact now`, or returning from it. Upstream ask: an on-demand
    /// compaction call.
    CompactNow,
    /// The agent's model registry predates a profile the switch needs.
    /// Upstream ask: profiles read at reconfigure time.
    ModelProfile,
}

impl Runner {
    /// The native auto-compaction trigger the policy gives the current
    /// model (its profile's window; an estimated window counts as unknown).
    pub(super) fn session_trigger(&self) -> CompactionTrigger {
        let (window, estimated) = self.context_window();
        self.template.policy.trigger((!estimated).then_some(window))
    }

    /// The one place the TUI retires and resumes its own idle session (see
    /// the module docs): the agent is rebuilt from the current config with
    /// `trigger`, the event stream followed again, and the identity re-read.
    pub(super) async fn rebuild_session_workaround(
        &mut self,
        reason: RebuildReason,
        trigger: CompactionTrigger,
    ) -> Result<(), TuiError> {
        tracing::debug!(?reason, ?trigger, "rebuilding the session (upstream workaround)");
        let id = self.session.id().clone();
        self.session.retire().await?;
        self.session =
            SessionHandle::resume(&self.runtime, &id, self.template.options(None, trigger)).await?;
        self.follow_events().await;
        self.refresh_identity().await;
        Ok(())
    }

    /// Rebuild the idle session when its agent runs on another trigger than
    /// [`Runner::session_trigger`] (after a model switch or a resume onto
    /// another model). A running turn or a pending `/compact now` keeps the
    /// session as it is.
    pub(super) async fn ensure_trigger(&mut self) {
        let wanted = self.session_trigger();
        if self.compact_now_pending
            || self.turn.is_active()
            || self.session.compaction() == wanted
        {
            return;
        }
        if let Err(err) = self
            .rebuild_session_workaround(RebuildReason::CompactionTrigger, wanted)
            .await
        {
            self.notice(
                NoticeLevel::Warn,
                format!("could not apply the compaction trigger to this session: {err}"),
            );
        }
    }

    /// [`crate::ui::Effect::CompactNow`]: rebuild the idle session with a
    /// one-turn trigger so its next model call compacts; answered with
    /// [`Msg::CompactRebuilt`].
    pub(super) async fn compact_now(&mut self) {
        let result = if self.turn.is_active() {
            Err(CompactFailure::Busy)
        } else {
            match self
                .rebuild_session_workaround(RebuildReason::CompactNow, COMPACT_NOW)
                .await
            {
                Ok(()) => {
                    self.compact_now_pending = true;
                    Ok(())
                }
                Err(err) => Err(CompactFailure::Rebuild(err.to_string())),
            }
        };
        self.send(Msg::CompactRebuilt(result));
    }

    /// After a turn: return a session rebuilt for `/compact now` to the
    /// policy's trigger.
    pub(super) async fn end_compact_now(&mut self) {
        if !self.compact_now_pending {
            return;
        }
        self.compact_now_pending = false;
        let trigger = self.session_trigger();
        if let Err(err) = self
            .rebuild_session_workaround(RebuildReason::CompactNow, trigger)
            .await
        {
            self.notice(
                NoticeLevel::Warn,
                format!(
                    "could not restore the compaction trigger after /compact now: {err}; /resume the session to restore it"
                ),
            );
        }
    }

    /// [`crate::ui::Effect::Remember`].
    pub(super) async fn remember(&mut self, text: &str) {
        let Some(line) = remember_line(text) else {
            self.notice(NoticeLevel::Warn, "nothing to remember");
            return;
        };
        let path = remember_target(&self.runtime.scope.context_root);
        if let Err(err) = append_remember_line(&path, &line) {
            self.notice(
                NoticeLevel::Error,
                format!("could not write {}: {err}", path.display()),
            );
            return;
        }
        let key = remember_key(&line);
        match self
            .session
            .append_system_context(remembered_context(&line), key)
            .await
        {
            Ok(_) => self.notice(
                NoticeLevel::Info,
                format!(
                    "remembered in {}: this session reads it from its next model call; it is fully applied (as part of AGENTS.md) from the next session",
                    path.display()
                ),
            ),
            Err(err) => self.notice(
                NoticeLevel::Warn,
                format!(
                    "remembered in {}, but this session could not take it now ({err}); it applies from the next session",
                    path.display()
                ),
            ),
        }
    }
}

/// The idempotency key of a remembered line (per session: the same line
/// twice is one system message). A SHA-256 digest of the line, so the key
/// is the same across processes, builds and toolchains.
fn remember_key(line: &str) -> String {
    let digest = Sha256::digest(line.as_bytes());
    let mut key = String::with_capacity(18 + 64);
    key.push_str("rkat-tui:remember:");
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(key, "{byte:02x}");
    }
    key
}

#[cfg(test)]
mod tests {
    use super::remember_key;

    #[test]
    fn remember_keys_are_stable_digests_per_line() {
        assert_eq!(remember_key("a"), remember_key("a"));
        assert_ne!(remember_key("a"), remember_key("b"));
        // Pinned: SHA-256("a"), the same in every process and build.
        assert_eq!(
            remember_key("a"),
            "rkat-tui:remember:ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb"
        );
    }
}
