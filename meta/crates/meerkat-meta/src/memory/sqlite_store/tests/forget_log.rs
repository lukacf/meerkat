//! What the store logs, and reports on the timeline, when the write gate
//! fires for a batch: a write that lands quarantined, or a change that is
//! applied at once.

use std::sync::{Arc, Mutex, PoisonError};

use super::*;
use crate::memory::events::CollectingEventSink;

#[derive(Clone, Default)]
struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

impl CaptureWriter {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    }
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
    type Writer = CaptureWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The gate's verdict for `ops` from an agent whose session the gate
/// quarantines, with the log lines and the timeline events it produced.
fn gate_verdict(ops: Vec<StagedOp>) -> (Option<String>, String, Vec<&'static str>) {
    let batch = StagedMutationBatch {
        kind: StagedBatchKind::FreshWrite,
        realm: "team".to_string(),
        author: MemoryAuthor::Agent {
            identity: "identity:luka".to_string(),
        },
        ops,
    };
    let events = CollectingEventSink::new();
    let writer = CaptureWriter::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer.clone())
        .with_ansi(false)
        .finish();
    // A second registered dispatch keeps the callsite's interest from being
    // cached as `never` by a thread without a subscriber.
    let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
    let verdict = tracing::subscriber::with_default(subscriber, || {
        batch_quarantine_reason(&batch, Some(&AlwaysQuarantine), Some(&events))
    });
    (verdict, writer.contents(), events.types())
}

fn fact(title: &str, body: &str) -> NewMemoryRecord {
    NewMemoryRecord {
        kind: MemoryKind::Fact,
        title: title.to_string(),
        description: String::new(),
        body: body.to_string(),
        tags: Vec::new(),
        evidence: Vec::new(),
        verification: None,
    }
}

/// A forget is a tombstone, and a tombstone is applied at once whatever the
/// author's session: nothing of it lands quarantined. The line and the
/// timeline must not say it did.
#[test]
fn a_forget_the_gate_fires_for_is_logged_as_applied_at_once() {
    let (verdict, log, events) = gate_verdict(vec![StagedOp::Tombstone {
        id: "mem-1".to_string(),
        rationale: None,
    }]);

    assert!(
        !log.contains("landing quarantined"),
        "a forget does not land quarantined: {log}"
    );
    assert!(log.contains("applied at once"), "{log}");
    assert!(
        log.contains("tombstone"),
        "the line names what was applied: {log}"
    );
    assert!(
        !events.contains(&"memory.write.quarantined"),
        "no quarantined write is reported for a forget: {events:?}"
    );
    // The gate's verdict itself is unchanged: the audit row keeps it.
    assert!(verdict.is_some());
}

/// A create or an update is what lands quarantined, and is still said to.
#[test]
fn a_write_the_gate_fires_for_is_still_logged_as_landing_quarantined() {
    let (verdict, log, events) = gate_verdict(vec![StagedOp::Create {
        id: None,
        scope: MemoryScope::Identity {
            realm: "team".to_string(),
            identity: "identity:luka".to_string(),
        },
        record: fact("A note", "a body"),
        trust: TrustTier::AgentObserved,
        derived_from: Vec::new(),
        rationale: None,
        created_at_ms: None,
        updated_at_ms: None,
    }]);

    assert!(log.contains("landing quarantined"), "{log}");
    assert!(!log.contains("applied at once"), "{log}");
    assert_eq!(events, vec!["memory.write.quarantined"]);
    assert!(verdict.is_some());
}

/// The forget itself, through the store: the record is tombstoned, and the
/// timeline carries no quarantined write for it.
#[tokio::test]
async fn an_agents_forget_from_a_gated_session_reports_no_quarantined_write()
-> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let store = SqliteAgentMemoryStore::open(dir.path())?;
    let scope = MemoryScope::Identity {
        realm: "team".to_string(),
        identity: "identity:luka".to_string(),
    };
    let written = store
        .remember_authored(&scope, fact("A note", "a body"), MemoryAuthor::Application)
        .await?;
    store.set_llm_write_gate(Arc::new(AlwaysQuarantine));
    let events = Arc::new(CollectingEventSink::new());
    store.set_event_sink(events.clone());

    let forgotten = store
        .forget_authored(
            &scope,
            &written.memory_id,
            MemoryAuthor::Agent {
                identity: "identity:luka".to_string(),
            },
        )
        .await?;

    assert!(forgotten.deleted);
    let record = store
        .records_by_ids("team", &[written.memory_id])
        .await?
        .remove(0);
    assert_eq!(record.status, RecordStatus::Tombstoned);
    assert!(
        !events.types().contains(&"memory.write.quarantined"),
        "{:?}",
        events.types()
    );
    Ok(())
}
