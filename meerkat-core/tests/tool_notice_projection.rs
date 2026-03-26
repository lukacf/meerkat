#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 1 red-ok typed-notice projection tests.

use meerkat_core::event::ToolConfigChangeOperation;
use meerkat_core::{ExternalToolDelta, ExternalToolDeltaPhase, ExternalToolUpdate};

#[test]
#[ignore = "Phase 1 red-ok external-tool integration suite"]
fn tool_notice_projection_red_ok_typed_notices_remain_separate_from_pending_state() {
    let update = ExternalToolUpdate {
        notices: vec![
            ExternalToolDelta::new(
                "phase1-tool",
                ToolConfigChangeOperation::Reload,
                ExternalToolDeltaPhase::Applied,
            )
            .with_tool_count(Some(3)),
        ],
        pending: vec!["phase1-tool".into()],
    };

    assert_eq!(update.notices.len(), 1);
    assert_eq!(update.notices[0].target, "phase1-tool");
    assert_eq!(
        update.notices[0].operation,
        ToolConfigChangeOperation::Reload
    );
    assert_eq!(update.notices[0].phase, ExternalToolDeltaPhase::Applied);
    assert_eq!(update.notices[0].tool_count, Some(3));
    assert_eq!(update.pending, vec!["phase1-tool".to_string()]);
}
