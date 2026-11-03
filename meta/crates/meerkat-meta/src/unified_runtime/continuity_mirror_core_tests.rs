//! The records-and-sessions half of the continuity mirror's round, over a fake
//! read seam: one failing identity does not stall the others, the put
//! sequence rises, and the copy never follows a loss.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Mutex;

use meerkat_core::Session;
use meerkat_core::types::{Message, SessionId, UserMessage};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// What the fake answers for one session.
#[derive(Clone)]
enum FakeLoad {
    Active(Box<Session>),
    Absent,
    Fails,
}

/// A read seam whose bindings and sessions a test sets between rounds.
#[derive(Default)]
struct FakeRead {
    bindings: Mutex<Vec<MirrorBinding>>,
    loads: Mutex<BTreeMap<String, FakeLoad>>,
}

impl FakeRead {
    fn bind(&self, identity: &str, session_id: &SessionId, generation: u64) {
        let mut bindings = self.bindings.lock().unwrap();
        bindings.retain(|binding| binding.identity != identity);
        bindings.push(MirrorBinding {
            identity: identity.to_string(),
            session_id: session_id.clone(),
            generation,
        });
    }

    fn answer(&self, session_id: &SessionId, load: FakeLoad) {
        self.loads
            .lock()
            .unwrap()
            .insert(session_id.to_string(), load);
    }
}

#[async_trait::async_trait]
impl BlobRead for FakeRead {
    async fn read_blob(&self, _blob_id: &str) -> Result<Option<ContinuityMirrorBlob>, String> {
        Ok(None)
    }
}

#[async_trait::async_trait]
impl MirrorRead for FakeRead {
    async fn bindings(&self) -> Result<Vec<MirrorBinding>, String> {
        Ok(self.bindings.lock().unwrap().clone())
    }

    async fn authority(&self, _session_id: &SessionId) -> Result<SessionResumeAuthority, String> {
        // The empty bundle: a change is told by the document digest.
        Ok(SessionResumeAuthority::default())
    }

    async fn load(&self, session_id: &SessionId) -> Result<ResumeSessionLoad, String> {
        match self
            .loads
            .lock()
            .unwrap()
            .get(&session_id.to_string())
            .cloned()
        {
            Some(FakeLoad::Active(session)) => Ok(ResumeSessionLoad::Active(session)),
            Some(FakeLoad::Absent) | None => Ok(ResumeSessionLoad::Absent),
            Some(FakeLoad::Fails) => Err(format!("session {session_id}: the store failed")),
        }
    }

    async fn current_binding(&self, identity: &str) -> Result<Option<(SessionId, u64)>, String> {
        Ok(self
            .bindings
            .lock()
            .unwrap()
            .iter()
            .find(|binding| binding.identity == identity)
            .map(|binding| (binding.session_id.clone(), binding.generation)))
    }
}

/// A sink that keeps every update it accepts.
#[derive(Default)]
struct Updates {
    accepted: Mutex<Vec<ContinuityMirrorUpdate>>,
}

#[async_trait::async_trait]
impl ContinuityMirrorSink for Updates {
    async fn put(&self, update: &ContinuityMirrorUpdate) -> Result<(), String> {
        self.accepted.lock().unwrap().push(update.clone());
        Ok(())
    }

    async fn put_blob(&self, _blob: &ContinuityMirrorBlob) -> Result<(), String> {
        Ok(())
    }
}

fn session(text: &str) -> (SessionId, Session) {
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(text)));
    (session.id().clone(), session)
}

fn with_message(mut session: Session, text: &str) -> Session {
    session.push(Message::User(UserMessage::text(text)));
    session
}

fn active(session: Session) -> FakeLoad {
    FakeLoad::Active(Box::new(session))
}

async fn run_round(
    read: &FakeRead,
    sink: &Updates,
    state: &mut MirrorState,
    counts: &ContinuityMirrorCounts,
) {
    round(read, sink, Duration::from_secs(10), state, counts).await;
}

fn identities(update: &ContinuityMirrorUpdate) -> Vec<&str> {
    update
        .records
        .iter()
        .map(|record| record.identity.as_str())
        .collect()
}

fn changed(update: &ContinuityMirrorUpdate) -> Vec<&str> {
    update
        .changed_sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect()
}

#[tokio::test]
async fn one_failing_identity_does_not_stall_the_others() -> TestResult {
    let read = FakeRead::default();
    let sink = Updates::default();
    let counts = ContinuityMirrorCounts::default();
    let mut state = MirrorState::default();
    let (alice_session_id, alice_session) = session("alice");
    let (bob_session_id, bob_session) = session("bob");
    read.bind("agent:alice", &alice_session_id, 1);
    read.bind("agent:bob", &bob_session_id, 1);
    read.answer(&alice_session_id, active(alice_session.clone()));
    read.answer(&bob_session_id, FakeLoad::Fails);

    // Bob's read fails: alice still goes out, bob is counted.
    run_round(&read, &sink, &mut state, &counts).await;
    {
        let accepted = sink.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 1);
        assert_eq!(identities(&accepted[0]), ["agent:alice"]);
        assert_eq!(
            changed(&accepted[0]),
            [alice_session_id.to_string().as_str()]
        );
    }
    assert_eq!(counts.failures.load(Ordering::Relaxed), 1);

    // Bob recovers: he goes out, and alice's unchanged session is not sent
    // again.
    read.answer(&bob_session_id, active(bob_session));
    run_round(&read, &sink, &mut state, &counts).await;
    {
        let accepted = sink.accepted.lock().unwrap();
        assert_eq!(accepted.len(), 2);
        assert_eq!(identities(&accepted[1]), ["agent:alice", "agent:bob"]);
        assert_eq!(changed(&accepted[1]), [bob_session_id.to_string().as_str()]);
    }

    // Then bob fails again after being copied: he keeps his last rows.
    read.answer(&bob_session_id, FakeLoad::Fails);
    read.answer(
        &alice_session_id,
        active(with_message(alice_session, "alice again")),
    );
    run_round(&read, &sink, &mut state, &counts).await;
    let accepted = sink.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 3);
    assert_eq!(identities(&accepted[2]), ["agent:alice", "agent:bob"]);
    assert_eq!(
        changed(&accepted[2]),
        [alice_session_id.to_string().as_str()]
    );
    Ok(())
}

#[tokio::test]
async fn the_put_sequence_rises_over_puts() -> TestResult {
    let read = FakeRead::default();
    let sink = Updates::default();
    let counts = ContinuityMirrorCounts::default();
    let mut state = MirrorState::default();
    let (session_id, first) = session("first");
    read.bind("agent:alice", &session_id, 1);
    read.answer(&session_id, active(first.clone()));
    run_round(&read, &sink, &mut state, &counts).await;
    // Nothing changed: no put.
    run_round(&read, &sink, &mut state, &counts).await;
    read.answer(&session_id, active(with_message(first, "second")));
    run_round(&read, &sink, &mut state, &counts).await;
    let accepted = sink.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2, "an unchanged round puts nothing");
    assert!(
        accepted[1].sequence > accepted[0].sequence,
        "{} then {}",
        accepted[0].sequence,
        accepted[1].sequence
    );
    // A restart starts from the wall clock, above any earlier put.
    assert!(next_sequence(0) >= accepted[0].sequence);
    Ok(())
}

#[tokio::test]
async fn an_absent_session_under_the_same_binding_keeps_its_rows() -> TestResult {
    let read = FakeRead::default();
    let sink = Updates::default();
    let counts = ContinuityMirrorCounts::default();
    let mut state = MirrorState::default();
    let (session_id, copied) = session("keep me");
    read.bind("agent:alice", &session_id, 1);
    read.answer(&session_id, active(copied));
    run_round(&read, &sink, &mut state, &counts).await;

    // The store loses the session, the binding stays: the record stays.
    read.answer(&session_id, FakeLoad::Absent);
    run_round(&read, &sink, &mut state, &counts).await;
    assert_eq!(counts.kept_after_loss.load(Ordering::Relaxed), 1);
    assert_eq!(sink.accepted.lock().unwrap().len(), 1, "nothing to put");
    assert_eq!(
        state
            .identities
            .get("agent:alice")
            .map(|mirrored| mirrored.record.session_id.clone()),
        Some(session_id.to_string())
    );

    // The binding moves to a new session: now the old rows are replaced.
    let (moved_id, moved) = session("after a reset");
    read.bind("agent:alice", &moved_id, 2);
    read.answer(&moved_id, active(moved));
    run_round(&read, &sink, &mut state, &counts).await;
    let accepted = sink.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[1].records[0].session_id, moved_id.to_string());
    assert_eq!(
        accepted[1].session_ids,
        [moved_id.to_string()].into_iter().collect::<BTreeSet<_>>()
    );
    Ok(())
}
