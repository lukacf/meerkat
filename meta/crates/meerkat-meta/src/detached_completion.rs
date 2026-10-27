//! Detached job completion entries in an owner's transcript.
//!
//! A background `fork_off` or council job records its outcome in the owner's
//! durable transcript when it finishes: a `BackgroundJob` system notice with
//! at least one persisted `SystemNoticeBlock::BackgroundJob` block. The entry
//! is a fact the owner refers back to on later turns, not scaffolding, so Meta
//! paths that collapse or drop transcript rows must leave it alone or at
//! least account for it. They all decide through
//! [`is_detached_completion_entry`], which reads typed transcript data only,
//! never message text.

use meerkat_core::Message;
#[cfg(test)]
use meerkat_core::types::{SystemNoticeBlock, SystemNoticeKind};

/// Whether a transcript row is a detached job's durable completion entry.
///
/// True for a `BackgroundJob` system notice carrying at least one persisted
/// background-job block, even when the row also includes refresh blocks.
/// Meerkat owns this typed distinction through the notice's persisted-job
/// predicate. Text lookalikes, refresh-only notices and empty notices are not
/// completion entries.
pub fn is_detached_completion_entry(message: &Message) -> bool {
    let Message::SystemNotice(notice) = message else {
        return false;
    };
    notice.persisted_background_job_id().is_some()
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::event::BackgroundJobTerminalStatus;
    use meerkat_core::types::{SystemMessage, SystemNoticeMessage, UserMessage};

    /// The completion entry exactly as meerkat's detached delivery builds it.
    fn completion_entry(tool: &'static str, job_id: &str) -> Message {
        match meerkat_mob_mcp::detached_delivery::detached_completion_notice(
            tool,
            job_id,
            BackgroundJobTerminalStatus::Completed,
            &serde_json::json!({ "text": "done" }),
        ) {
            Ok(notice) => Message::SystemNotice(notice),
            Err(error) => panic!("meerkat could not build the completion notice: {error}"),
        }
    }

    fn background_job_notice(blocks: Vec<SystemNoticeBlock>) -> Message {
        Message::SystemNotice(SystemNoticeMessage::with_blocks(
            SystemNoticeKind::BackgroundJob,
            Some("Background fork_off job job-a finished".to_string()),
            blocks,
        ))
    }

    fn job_block(job_id: &str, persisted: bool) -> SystemNoticeBlock {
        SystemNoticeBlock::BackgroundJob {
            job_id: job_id.to_string(),
            display_name: Some("fork_off".to_string()),
            status: BackgroundJobTerminalStatus::Completed,
            detail: None,
            persisted,
        }
    }

    #[test]
    fn meerkat_completion_notices_are_completion_entries() {
        assert!(is_detached_completion_entry(&completion_entry(
            "fork_off", "job-a"
        )));
        assert!(is_detached_completion_entry(&completion_entry(
            "temporary_council",
            "job-b"
        )));
        assert!(is_detached_completion_entry(&background_job_notice(vec![
            job_block("job-a", true),
            job_block("job-b", true),
        ])));
    }

    #[test]
    fn mixed_background_job_notices_are_completion_entries_in_either_order() {
        for blocks in [
            vec![job_block("job-a", true), job_block("shell-1", false)],
            vec![job_block("shell-1", false), job_block("job-a", true)],
        ] {
            let message = background_job_notice(blocks);
            assert!(is_detached_completion_entry(&message));
        }
    }

    /// Refresh projections share the `BackgroundJob` kind but are rebuilt at
    /// each model call and never stored; they are not completion entries.
    #[test]
    fn non_persisted_background_job_notices_are_not_completion_entries() {
        assert!(!is_detached_completion_entry(&background_job_notice(vec![
            job_block("shell-1", false)
        ])));
        assert!(!is_detached_completion_entry(&background_job_notice(vec![
            job_block("shell-1", false),
            job_block("shell-2", false),
        ])));
        assert!(!is_detached_completion_entry(&background_job_notice(
            Vec::new()
        )));
    }

    /// Text never decides: rows that read like a completion but lack the
    /// typed marker are not completion entries.
    #[test]
    fn lookalike_text_is_never_a_completion_entry() {
        let text = "Background fork_off job job-a finished (completed):\n{}";
        assert!(!is_detached_completion_entry(&Message::SystemNotice(
            SystemNoticeMessage::new(SystemNoticeKind::Generic, text)
        )));
        assert!(!is_detached_completion_entry(&Message::SystemNotice(
            SystemNoticeMessage::with_blocks(
                SystemNoticeKind::Generic,
                Some(text.to_string()),
                vec![job_block("job-a", true)],
            )
        )));
        assert!(!is_detached_completion_entry(&Message::System(
            SystemMessage::new(text)
        )));
        assert!(!is_detached_completion_entry(&Message::User(
            UserMessage::text(text)
        )));
    }
}
