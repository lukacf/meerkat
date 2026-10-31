#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! A dormant member on a host-store realm records its operator-instruction
//! target without waking.
//!
//! The runtime store is the host store adapter over the reference in-memory
//! host store, as a host-store realm keeps WholeBlob session documents. One
//! process creates the member's session and stops; a second process over
//! the same realm, where the session is dormant, records the mob's target
//! store-only; a third reads the committed target back. No process builds an
//! agent for the session after its creation, and none holds it live.

use super::*;
use meerkat_core::host_store::{AcquireMode, HostStore};
use meerkat_core::service::{
    DeferredPromptPolicy, InitialTurnPolicy, SessionService, SessionServiceInstructionExt,
};
use meerkat_core::{
    InstructionKey, InstructionNamespace, InstructionRecordDisposition, InstructionRevision,
    InstructionSlot, InstructionTargetRecord, InstructionTargetRecordSlot,
};
use meerkat_runtime::store::RuntimeStore;
use meerkat_runtime::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_store_conformance::InMemoryHostStore;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts every agent a process builds: a build is a wake.
struct CountingPersistentBuilder {
    builds: Arc<AtomicUsize>,
}

#[async_trait]
impl SessionAgentBuilder for CountingPersistentBuilder {
    type Agent = PersistentMockAgent;

    fn session_comms_identity_root(&self) -> meerkat_core::SessionCommsIdentityRoot {
        meerkat_core::SessionCommsIdentityRoot::NotKept
    }

    async fn build_agent(
        &self,
        req: &CreateSessionRequest,
        event_tx: tokio::sync::mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        PersistentMockBuilder.build_agent(req, event_tx).await
    }
}

/// One process over `host`: it takes the realm over from any predecessor.
async fn open_process(
    host: &Arc<InMemoryHostStore>,
    builds: &Arc<AtomicUsize>,
) -> meerkat_session::PersistentSessionService<CountingPersistentBuilder> {
    let realm = SpiRealm::open(
        Arc::clone(host) as Arc<dyn HostStore>,
        SpiRealmConfig {
            acquire: AcquireMode::Takeover,
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("open the realm");
    let runtime_store = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("open the runtime store"),
    );
    meerkat_session::PersistentSessionService::new(
        CountingPersistentBuilder {
            builds: Arc::clone(builds),
        },
        16,
        Arc::new(meerkat_store::MemoryStore::new()),
        runtime_store as Arc<dyn RuntimeStore>,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    )
}

fn deferred_create() -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: "mock".to_string(),
        prompt: String::new().into(),
        system_prompt: meerkat_core::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    }
}

fn member_snapshot(generation: u64) -> InstructionTargetRecord {
    InstructionTargetRecord {
        mob_id: "mob-1".to_string(),
        agent_identity: "worker".to_string(),
        generation,
        slots: vec![InstructionTargetRecordSlot {
            slot: InstructionSlot {
                namespace: InstructionNamespace::new("app.example").expect("namespace"),
                key: InstructionKey::new("tone").expect("key"),
            },
            revision: InstructionRevision::new(1).expect("revision"),
            body: Some("Answer formally.".to_string()),
        }],
    }
}

#[tokio::test]
async fn a_dormant_member_on_a_host_store_realm_records_the_target_without_waking() {
    let host = Arc::new(InMemoryHostStore::new());
    let builds = Arc::new(AtomicUsize::new(0));

    // The first process creates the member's session, then stops.
    let first = open_process(&host, &builds).await;
    let session_id = first
        .create_session(deferred_create())
        .await
        .expect("create the member's session")
        .session_id;
    first.shutdown().await;
    drop(first);
    let builds_after_create = builds.load(Ordering::SeqCst);

    // A second process over the same realm: the member is dormant there.
    let second = Arc::new(open_process(&host, &builds).await);
    let receipt = Arc::clone(&second)
        .record_instruction_target(&session_id, member_snapshot(1))
        .await
        .expect("a dormant member records its target store-only");
    assert_eq!(receipt.disposition, InstructionRecordDisposition::Recorded);
    assert_eq!(receipt.target_generation, 1);
    assert_eq!(
        builds.load(Ordering::SeqCst),
        builds_after_create,
        "recording builds no agent for the member"
    );
    assert!(
        !second
            .has_live_session(&session_id)
            .await
            .expect("live status"),
        "the member stays dormant"
    );
    let repeated = Arc::clone(&second)
        .record_instruction_target(&session_id, member_snapshot(1))
        .await
        .expect("a repeated delivery answers already recorded");
    assert_eq!(
        repeated.disposition,
        InstructionRecordDisposition::AlreadyRecorded
    );
    second.shutdown().await;
    drop(second);

    // A third process reads the committed target back, still without a wake.
    let third = open_process(&host, &builds).await;
    let read = third
        .read_session_instructions(&session_id)
        .await
        .expect("the committed read");
    let target = read
        .target
        .expect("the host store holds the recorded target");
    assert_eq!(target.target_generation(), 1);
    assert_eq!(target.slots().len(), 1);
    assert!(read.effective.is_empty(), "recording applies nothing");
    assert_eq!(builds.load(Ordering::SeqCst), builds_after_create);
    assert!(
        !third
            .has_live_session(&session_id)
            .await
            .expect("live status")
    );
}
