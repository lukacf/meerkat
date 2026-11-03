//! Deterministic fixtures shared by the chapters.

use meerkat_meta::console_aggregator::{
    ConsoleFrameSource, ConsoleFrameSourceKind, ConsoleFrameStatus, NewConsoleFrame,
};
use meerkat_meta::memory::{MemoryKind, NewMemoryRecord};
use meerkat_meta::types::UnifiedEvent;
use meerkat_meta::unified_runtime::PersistedEvent;

/// A deterministic console frame keyed by `dedupe_key`.
pub fn console_frame(dedupe_key: &str, identity: &str, timestamp_ms: u64) -> NewConsoleFrame {
    NewConsoleFrame {
        id: None,
        dedupe_key: dedupe_key.to_string(),
        timestamp_ms,
        runtime_key: "conformance-runtime".to_string(),
        identity: identity.to_string(),
        conversation_id: None,
        session_id: None,
        kind: "conformance_probe".to_string(),
        status: ConsoleFrameStatus::Completed,
        payload: serde_json::json!({ "marker": dedupe_key }),
        source: ConsoleFrameSource {
            member_provenance: None,
            kind: ConsoleFrameSourceKind::Synthetic,
            source_cursor: None,
        },
        source_event_id: None,
        interaction_id: None,
        turn_id: None,
        run_id: None,
        parent_frame_id: None,
        caused_by_frame_id: None,
    }
}

/// A member-scoped frame whose owner witness must survive every storage path.
pub fn console_frame_with_member_provenance() -> NewConsoleFrame {
    use meerkat_meta::runtime::ConsoleMember;
    use meerkat_meta::{ConsoleFrameMemberProvenance, ConsoleIdentityRecord, ConsoleVisibility};
    let mut frame = console_frame("conformance-provenance", "console:private-member", 1_500);
    frame.session_id = Some("session-private-member".to_string());
    let labels = [("visibility".to_string(), "private".to_string())]
        .into_iter()
        .collect();
    frame.source.member_provenance = Some(ConsoleFrameMemberProvenance {
        identity: ConsoleIdentityRecord {
            identity: frame.identity.clone(),
            display_name: "Private member".to_string(),
            runtime_key: frame.runtime_key.clone(),
            runtime_member_id: "private-member".to_string(),
            session_id: frame.session_id.clone(),
            member_id: None,
            visibility: ConsoleVisibility::Addressable,
            addressable: true,
            health: "ready".to_string(),
            topology_peers: vec![],
            labels,
        },
        member: ConsoleMember {
            agent_identity: "private-member".to_string(),
            role: "delegate".to_string(),
            state: "running".to_string(),
            model_capabilities: Default::default(),
            runtime_mode: None,
            session_id: frame.session_id.clone(),
            wired_to: vec![],
            labels: [("visibility".to_string(), "private".to_string())]
                .into_iter()
                .collect(),
            progress: None,
        },
        primary_mob_id: "primary".to_string(),
        source_mob_id: "private-lower-mob".to_string(),
        lineage: Vec::new(),
    });
    frame
}

/// A deterministic persisted operational event with a caller-chosen id and
/// ingestion sequence number.
pub fn persisted_event(id: &str, seq: u64) -> PersistedEvent {
    PersistedEvent {
        id: id.to_string(),
        seq,
        timestamp_ms: 1_000 + seq,
        member_id: None,
        event: UnifiedEvent::Agent {
            agent_id: "conformance-agent".to_string(),
            event_type: "conformance_probe".to_string(),
            payload: Some(serde_json::json!({ "id": id })),
        },
    }
}

/// A deterministic new memory record for staged/authored write chapters.
pub fn new_memory_record(title: &str, body: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: String::new(),
        body: body.to_string(),
        tags: vec!["conformance".to_string()],
        evidence: Vec::new(),
        verification: None,
    }
}
