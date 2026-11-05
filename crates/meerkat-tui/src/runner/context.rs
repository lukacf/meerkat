//! Context upkeep: `/remember` and the compaction threshold.
//!
//! `/remember <text>` (or a single-line `#text`) appends `- <text>` to the
//! workspace `AGENTS.md` (created with a header when missing) and pushes
//! the line into the live session as durable system context
//! (`append_system_context`, idempotent per session and line), so the model
//! reads it from its next request; new sessions load it with `AGENTS.md`.
//!
//! Compaction is native (see [`crate::ui::compaction`]): every session build
//! carries the policy's threshold for the model's context window
//! ([`Runner::session_threshold`]). A model switch that moves it rebuilds
//! the idle session ([`Runner::ensure_threshold`]). `/compact now` rebuilds
//! it with a one-turn threshold of one token; the first model call of the
//! next turn compacts, and when that turn finishes the session is rebuilt
//! on the policy's threshold again.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroU64;

use super::Runner;
use crate::bootstrap::instructions::{
    append_remember_line, remember_line, remember_target, remembered_context,
};
use crate::ui::compaction::COMPACT_NOW_THRESHOLD;
use crate::ui::{CompactFailure, Msg, NoticeLevel};

impl Runner {
    /// The native auto-compaction threshold the policy gives the current
    /// model.
    pub(super) fn session_threshold(&self) -> NonZeroU64 {
        self.template
            .policy
            .native_threshold(Some(self.context_window().0))
    }

    /// Rebuild the idle session when its agent runs on another threshold
    /// than [`Runner::session_threshold`] (after a model switch or a resume
    /// onto another model). A running turn or an armed `/compact now`
    /// keeps the session as it is.
    pub(super) async fn ensure_threshold(&mut self) {
        let wanted = self.session_threshold();
        if self.compact_armed
            || self.turn.is_active()
            || self.session.compaction_threshold() == Some(wanted)
        {
            return;
        }
        if let Err(err) = self.rebuild_session(wanted).await {
            self.notice(
                NoticeLevel::Warn,
                format!("could not apply the compaction threshold to this session: {err}"),
            );
        }
    }

    /// [`crate::ui::Effect::CompactNow`]: rebuild the idle session with a
    /// one-turn threshold; answered with [`Msg::CompactArmed`].
    pub(super) async fn compact_now(&mut self) {
        let result = if self.turn.is_active() {
            Err(CompactFailure::Busy)
        } else {
            match self.rebuild_session(COMPACT_NOW_THRESHOLD).await {
                Ok(()) => {
                    self.compact_armed = true;
                    Ok(())
                }
                Err(err) => Err(CompactFailure::Rebuild(err.to_string())),
            }
        };
        self.send(Msg::CompactArmed(result));
    }

    /// After a turn: return a session armed by `/compact now` to the
    /// policy's threshold.
    pub(super) async fn disarm_compaction(&mut self) {
        if !self.compact_armed {
            return;
        }
        self.compact_armed = false;
        if let Err(err) = self.rebuild_session(self.session_threshold()).await {
            self.notice(
                NoticeLevel::Warn,
                format!(
                    "could not restore the compaction threshold after /compact now: {err}; /resume the session to restore it"
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
/// twice is one system message).
fn remember_key(line: &str) -> String {
    let mut hasher = DefaultHasher::new();
    line.hash(&mut hasher);
    format!("rkat-tui:remember:{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::remember_key;

    #[test]
    fn remember_keys_are_stable_per_line() {
        assert_eq!(remember_key("a"), remember_key("a"));
        assert_ne!(remember_key("a"), remember_key("b"));
        assert!(remember_key("a").starts_with("rkat-tui:remember:"));
    }
}
