//! A person's operator memory never reaches a member behind the supervisor
//! bridge (a placed member) or an externally bound member.
//!
//! Meta prepares every delivery on its own side, the placed and external
//! ones included, and runs ambient recall for each one that is not
//! autonomous. That recall reads the identity's own scope only. Operator
//! memory reaches a turn only request-only, on a local session-backed
//! member's own run (`RuntimeTurnMetadata::request_only_context`, which the
//! mob refuses on remote and peer-only members). So a Console send to an
//! externally bound member, and a schedule-fired dispatch to a placed or an
//! externally bound member, carry no operator section: not in their
//! injected context, and nowhere else in the delivery.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::Arc;

use meerkat_core::types::{HandlingMode, SessionId};
use meerkat_core::{ContentInput, PrincipalKind, PrincipalRef, TrustDomainId, TurnRequester};

use crate::console_aggregator::{ConsoleCursor, ConsoleFrameStatus, ConsoleInteractionAccepted};
use crate::identity_first::core_test_bridge::{CoreReadsBridge, register_live};
use crate::identity_first::{
    AgentAddressability, AgentIdentity, AgentMemoryConfig, AgentMemoryOperatorScope,
    AgentMemoryProvider, AgentMemoryRuntimeInjector, AgentMemorySelection, DispatchInput,
    DispatchOrigin, DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, NewAgentMemory,
};
use crate::memory::records::{MemoryAuthor, NewMemoryRecord};
use crate::memory::{MemoryKind, MemoryScope, SqliteAgentMemoryStore};

const REALM: &str = "default";
const ISSUER: &str = "https://issuer.example.test";
const IDENTITY_FACT: &str = "Deploys are frozen on Fridays";
const OPERATOR_FACT: &str = "Alice prefers terse status notes";

/// A person as a Console send names its requester: a `Human` with the
/// verified `sub`, in the trust domain of the verified issuer.
fn person(subject: &str) -> Result<PrincipalRef, Box<dyn Error>> {
    let domain = TrustDomainId::new(format!(
        "{}{ISSUER}",
        crate::runtime::CONSOLE_HUMAN_TRUST_DOMAIN_PREFIX
    ))?;
    Ok(PrincipalRef::in_domain(
        PrincipalKind::Human,
        subject,
        domain,
    )?)
}

/// A turn-driven member: the runtime mode that carries injected context,
/// so every delivery to it runs the ambient recall.
fn member(identity: &str) -> Result<DurableAgentSpec, Box<dyn Error>> {
    Ok(DurableAgentSpec {
        identity: AgentIdentity::parse(identity)?,
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    })
}

fn externally_bound(identity: &str) -> Result<DurableAgentSpec, Box<dyn Error>> {
    let mut spec = member(identity)?;
    spec.backend = Some(meerkat_mob::MobBackendKind::External);
    Ok(spec)
}

fn placed(identity: &str) -> Result<DurableAgentSpec, Box<dyn Error>> {
    let mut spec = member(identity)?;
    spec.placement = Some(meerkat_contracts::WireHostRef("remote-host".to_string()));
    Ok(spec)
}

/// Alice's Console send to an externally bound member (it names her as its
/// requester), and a schedule-fired dispatch to a placed and to an
/// externally bound member, while Alice's operator scope holds a fact and
/// the operator scope is on. Each delivery carries its member's own
/// recalled memory (the ambient recall ran) and no operator memory anywhere
/// in it. Alice's fact is recallable: her own request-only section holds it.
#[tokio::test]
async fn bridged_deliveries_carry_no_operator_memory() -> Result<(), Box<dyn Error>> {
    let alice = person("alice")?;
    let console_send_target = externally_bound("external-console")?;
    let scheduled_targets = [
        placed("placed-schedule")?,
        externally_bound("external-schedule")?,
    ];

    let store_dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteAgentMemoryStore::open(store_dir.path())?);
    let operator =
        crate::http_console::person_caller_scope(&alice).ok_or("a person has a caller scope")?;
    store
        .remember_authored(
            &MemoryScope::Operator {
                realm: REALM.to_string(),
                operator,
            },
            NewMemoryRecord {
                kind: MemoryKind::Preference,
                title: OPERATOR_FACT.to_string(),
                description: String::new(),
                body: format!("{OPERATOR_FACT}."),
                tags: Vec::new(),
                evidence: Vec::new(),
                verification: None,
            },
            MemoryAuthor::Operator,
        )
        .await?;
    for spec in std::iter::once(&console_send_target).chain(scheduled_targets.iter()) {
        store
            .remember(
                REALM,
                &spec.identity,
                NewAgentMemory {
                    title: "Deploy policy".to_string(),
                    body: format!("{IDENTITY_FACT}."),
                    tags: Vec::new(),
                },
            )
            .await?;
    }
    let provider: Arc<dyn AgentMemoryProvider> = store.clone();
    let injector = AgentMemoryRuntimeInjector::new(
        provider,
        AgentMemoryConfig {
            realm: REALM.to_string(),
            selection: AgentMemorySelection::Always,
            operator_scope: AgentMemoryOperatorScope::Provisional,
            ..AgentMemoryConfig::default()
        },
    );
    let alice_section = injector
        .operator_context_for_turn(
            &console_send_target.identity,
            None,
            Some(&TurnRequester::direct(alice.clone())),
        )
        .await?
        .ok_or("Alice's own turn has a request-only section")?;
    assert!(
        alice_section.as_str().contains(OPERATOR_FACT),
        "the fixture's operator fact is recallable: {alice_section:?}"
    );

    let bridge = Arc::new(CoreReadsBridge::with_deliveries());
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "operator-memory-bridged".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge.clone()),
        default_timeout: None,
    }));
    runtime.set_agent_memory(Some(injector)).await;
    for spec in std::iter::once(&console_send_target).chain(scheduled_targets.iter()) {
        register_live(&runtime, &bridge.core, spec.clone(), 0, SessionId::new()).await?;
    }

    // A Console interaction id is a canonical UUID: the send's delivery
    // identity correlates by it.
    let accepted = ConsoleInteractionAccepted {
        interaction_id: "6f1c2a9e-3b4d-4e5f-8a7b-9c0d1e2f3a4b".to_string(),
        identity: console_send_target.identity.to_string(),
        conversation_id: None,
        session_id: None,
        input_frame_id: "frame-1".to_string(),
        cursor: ConsoleCursor::from_seq(1),
        status: ConsoleFrameStatus::Accepted,
    };
    runtime
        .send_console_human_input_tracked(
            &console_send_target.identity,
            None,
            &ContentInput::Text("What is the deploy policy?".to_string()),
            HandlingMode::Queue,
            &[],
            &accepted,
            Some(&alice),
            None,
        )
        .await?;
    for spec in &scheduled_targets {
        runtime
            .dispatch(
                &spec.identity,
                &DispatchInput {
                    content: ContentInput::Text("Run the weekly deploy check.".to_string()),
                    origin: DispatchOrigin::Scheduler,
                    correlation_id: None,
                    idempotency_key: None,
                },
            )
            .await?;
    }

    let deliveries = bridge.deliveries();
    assert_eq!(
        deliveries.len(),
        3,
        "one delivery per door: {deliveries:#?}"
    );
    for delivery in &deliveries {
        let injected = delivery
            .injected_context
            .iter()
            .map(ContentInput::text_content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            injected.contains(IDENTITY_FACT),
            "the ambient recall ran for the member's own memory: {delivery:#?}"
        );
    }
    // Every door is judged, so a leak names each delivery that carries one.
    let leaking = deliveries
        .iter()
        .filter(|delivery| format!("{delivery:?}").contains(OPERATOR_FACT))
        .collect::<Vec<_>>();
    assert!(
        leaking.is_empty(),
        "a bridged delivery carries no operator memory: {leaking:#?}"
    );
    Ok(())
}
