//! Shutdown: interrupt the running turn, take the session off the runtime,
//! and save every message the model never received to
//! `<realm>/tui_unsent.txt`.
//!
//! Steered messages are durable runtime inputs: left alone, a queued one
//! would run the next time the session is resumed. Quitting cancels them
//! with the turn instead, and saves their text.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::Runner;
use crate::delivery::PendingId;

/// How long quitting waits for an interrupted turn to wind down.
pub const QUIT_TURN_TIMEOUT: Duration = Duration::from_secs(10);

/// Upper bound on the whole runner shutdown (turn, teardown, file).
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(20);

/// File in the realm directory that collects unsent messages.
pub const UNSENT_FILE: &str = "tui_unsent.txt";

/// Line written in front of every saved message.
const UNSENT_HEADER: &str = "--- unsent message ---";

/// Where unsent messages were saved on quit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedUnsent {
    /// Messages written by this shutdown.
    pub count: usize,
    /// The file they were appended to.
    pub path: PathBuf,
}

impl Runner {
    /// Quit: interrupt a running turn and wait (up to [`QUIT_TURN_TIMEOUT`])
    /// for its inputs to end, take the session off the runtime and shut the
    /// service down, then append every message the model never got to
    /// [`UNSENT_FILE`].
    pub(super) async fn shutdown(mut self) -> Option<SavedUnsent> {
        if self.turn.is_active() {
            self.interrupt();
            let deadline = tokio::time::sleep(QUIT_TURN_TIMEOUT);
            tokio::pin!(deadline);
            while self.turn.is_active() && !self.turn.all_done() {
                tokio::select! {
                    Some(done) = self.completion_rx.recv() => self.on_completed(done).await,
                    () = &mut deadline => {
                        tracing::warn!("the interrupted turn did not end in time on quit");
                        break;
                    }
                }
            }
            if self.turn.is_active() && self.turn.all_done() {
                self.finish_turn().await;
            }
        }
        if let Err(err) = self.session.retire().await {
            tracing::warn!(error = %err, "could not take the session off the runtime on quit");
        }
        if let Err(err) = self.runtime.service.try_shutdown().await {
            tracing::warn!(error = %err, "session service shutdown failed");
        }
        let unsent = unsent_texts(std::mem::take(&mut self.held_unsent), &self.delivered);
        if unsent.is_empty() {
            return None;
        }
        let path = self.runtime.scope.realm_root().join(UNSENT_FILE);
        match append_unsent(&path, &unsent) {
            Ok(()) => Some(SavedUnsent {
                count: unsent.len(),
                path,
            }),
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "could not save unsent messages");
                None
            }
        }
    }
}

/// Texts of the held messages the runtime did not deliver, oldest first
/// (the Ctrl-X Ctrl-S prompt, which has no id, last).
fn unsent_texts(
    mut held: Vec<(Option<PendingId>, String)>,
    delivered: &[PendingId],
) -> Vec<String> {
    held.retain(|(id, _)| id.is_none_or(|id| !delivered.contains(&id)));
    held.sort_by_key(|(id, _)| id.map_or(u64::MAX, |id| id.0));
    held.into_iter().map(|(_, text)| text).collect()
}

/// Append `texts` to the unsent-messages file, each after a header line.
fn append_unsent(path: &Path, texts: &[String]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for text in texts {
        writeln!(file, "{UNSENT_HEADER}")?;
        writeln!(file, "{}", text.trim_end())?;
    }
    file.flush()
}

/// Number of messages in an unsent-messages file.
#[must_use]
pub fn count_unsent(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .map(|content| content.lines().filter(|l| *l == UNSENT_HEADER).count())
        .unwrap_or(0)
}

/// Start-up notice when earlier unsent messages are waiting in the realm.
#[must_use]
pub fn unsent_notice(realm_root: &Path) -> Option<String> {
    let path = realm_root.join(UNSENT_FILE);
    match count_unsent(&path) {
        0 => None,
        n => Some(format!(
            "{n} unsent message{} from an earlier run saved in {} (delete the file once handled)",
            if n == 1 { "" } else { "s" },
            path.display()
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn unsent_file_appends_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        assert!(unsent_notice(dir.path()).is_none());
        let path = dir.path().join(UNSENT_FILE);
        append_unsent(&path, &["one".into(), "two\nlines\n".into()]).unwrap();
        append_unsent(&path, &["three".into()]).unwrap();
        assert_eq!(count_unsent(&path), 3);
        let notice = unsent_notice(dir.path()).unwrap();
        assert!(notice.starts_with("3 unsent messages"), "{notice}");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("two\nlines\n---"), "{content}");
    }

    #[test]
    fn delivered_messages_are_not_saved() {
        let held = vec![
            (None, "next".to_string()),
            (Some(PendingId(2)), "two".to_string()),
            (Some(PendingId(1)), "one".to_string()),
        ];
        assert_eq!(
            unsent_texts(held, &[PendingId(2)]),
            vec!["one".to_string(), "next".to_string()]
        );
    }
}
