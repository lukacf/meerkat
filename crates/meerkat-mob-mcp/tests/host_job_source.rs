//! A host's own committed-result owner composed into the job source a mob
//! state binds: the host's exact namespaces are answered by it, its fork_off
//! and council jobs by the state, and any other host namespace by no one.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use std::sync::Arc;

use meerkat::{ContinuationProducer, RetainedJobFacts, RetainedJobLookup, RetainedJobSource};
use meerkat_core::SessionId;
use meerkat_mob_mcp::{HostJobSource, HostJobSourceError, MobMcpState};
use support::{CouncilFixture, ScriptedTurn};

/// A host's setup store: every job it is asked for is its committed one.
struct SetupStore {
    facts: RetainedJobFacts,
}

#[async_trait::async_trait]
impl RetainedJobSource for SetupStore {
    async fn retained_job(
        &self,
        _producer: &ContinuationProducer,
        _job_id: &str,
    ) -> RetainedJobLookup {
        RetainedJobLookup::Found(self.facts.clone())
    }
}

/// A host source that claims every host original in every namespace, and
/// offers a binder for every namespace: only the composition's declared
/// namespaces may reach it.
struct ClaimsEverything {
    facts: meerkat::HostOriginalFacts,
    binder: Arc<dyn meerkat::HostOriginalBinder>,
}

#[async_trait::async_trait]
impl RetainedJobSource for ClaimsEverything {
    async fn retained_job(
        &self,
        _producer: &ContinuationProducer,
        _job_id: &str,
    ) -> RetainedJobLookup {
        RetainedJobLookup::Absent
    }

    async fn host_original(
        &self,
        _namespace: &str,
        _producer_id: &str,
    ) -> meerkat::HostOriginalLookup {
        meerkat::HostOriginalLookup::Found(self.facts.clone())
    }

    fn host_original_binder(
        &self,
        _namespace: &str,
    ) -> Option<Arc<dyn meerkat::HostOriginalBinder>> {
        Some(Arc::clone(&self.binder))
    }
}

/// A binder that binds nothing; only its presence is observed here.
struct InertBinder;

#[async_trait::async_trait]
impl meerkat::HostOriginalBinder for InertBinder {
    async fn bind(
        &self,
        _original: &meerkat::HostOriginalRef,
        _session: &SessionId,
        _input: meerkat_runtime::Input,
    ) -> Result<meerkat::BoundHostOriginal, meerkat::HostOriginalRefusal> {
        Err(meerkat::HostOriginalRefusal::NotOwned)
    }

    async fn readiness(
        &self,
        _namespace: &str,
        _owner: &meerkat::ContinuationOwner,
    ) -> meerkat::HostOriginalBinderReadiness {
        meerkat::HostOriginalBinderReadiness::Ready
    }

    fn retain_unresolved(
        &self,
        _settlement: Box<dyn meerkat::HostOriginalSettlement>,
        _detail: &str,
    ) {
    }
}

fn host_original_facts() -> meerkat::HostOriginalFacts {
    let delivery = meerkat::ContinuationDelivery {
        key: meerkat::ContinuationKey::new("ingest:1").expect("key"),
        result: meerkat::ContinuationResultRef {
            producer: host("app.setup"),
            producer_id: "observation-1".into(),
            result_digest: "sha256:observation-1".into(),
            summary: None,
        },
        body: "a new item arrived".into(),
        handling: meerkat::ContinuationHandling::Queue,
    };
    meerkat::HostOriginalFacts::new(
        meerkat::ContinuationOwner::Session {
            session_id: SessionId::new(),
        },
        delivery,
        meerkat_runtime::OriginalWorkRef {
            authority: meerkat_core::PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::ServiceAccount,
                "collector",
                meerkat_core::TrustDomainId::new("host-test").expect("domain"),
            )
            .expect("qualified"),
            work: meerkat_authorization_contracts::evidence::EvidenceId::new("observation-1")
                .expect("work id"),
        },
    )
}

/// Core owns the "meerkat." namespaces (consent notices among them): no host
/// source may own one, whatever it would answer.
#[test]
fn a_host_job_source_never_owns_a_reserved_namespace() {
    let store: Arc<dyn RetainedJobSource> = Arc::new(SetupStore {
        facts: RetainedJobFacts {
            owner_session_id: SessionId::new(),
            retained_work: None,
            result_digest: "sha256:setup".into(),
        },
    });
    assert_eq!(
        HostJobSource::new(["app.setup", "meerkat.consent"], store).err(),
        Some(HostJobSourceError::ReservedNamespace(
            "meerkat.consent".into()
        ))
    );
}

/// The composed source forwards host originals and their binder for the
/// host's declared namespaces only, even when the host's own source claims
/// every namespace.
#[tokio::test]
async fn the_bound_job_source_forwards_host_originals_for_declared_namespaces_only() {
    let fixture = CouncilFixture::new_runtime_backed(|_| ScriptedTurn::Text("ok".into()));
    let facts = host_original_facts();
    let host_source = HostJobSource::new(
        ["app.setup"],
        Arc::new(ClaimsEverything {
            facts: facts.clone(),
            binder: Arc::new(InertBinder),
        }),
    )
    .expect("a named namespace");
    let state = MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        fixture.runtime_adapter.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .expect("construct runtime authority")
    .with_host_job_source(host_source)
    .into_shared();
    let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
        meerkat_runtime::InMemoryRuntimeStore::new(),
    ));
    let bindings = meerkat::ContinuationHostBindings::default();
    state
        .bind_continuations(inbox, &bindings)
        .expect("bind the state's continuations");
    let bound = bindings.job_source().expect("a live binding");

    assert_eq!(
        bound.host_original("app.setup", "observation-1").await,
        meerkat::HostOriginalLookup::Found(facts),
        "the declared namespace reaches the host's source"
    );
    assert!(bound.host_original_binder("app.setup").is_some());
    for undeclared in ["app.other", "meerkat.consent"] {
        assert_eq!(
            bound.host_original(undeclared, "observation-1").await,
            meerkat::HostOriginalLookup::Absent,
            "{undeclared} is owned by no one here"
        );
        assert!(
            bound.host_original_binder(undeclared).is_none(),
            "{undeclared} has no binder here"
        );
    }
}

fn host(namespace: &str) -> ContinuationProducer {
    ContinuationProducer::Host {
        namespace: namespace.into(),
    }
}

#[test]
fn a_host_job_source_must_own_a_named_namespace() {
    let store: Arc<dyn RetainedJobSource> = Arc::new(SetupStore {
        facts: RetainedJobFacts {
            owner_session_id: SessionId::new(),
            retained_work: None,
            result_digest: "sha256:setup".into(),
        },
    });
    assert_eq!(
        HostJobSource::new(Vec::<String>::new(), Arc::clone(&store)).err(),
        Some(HostJobSourceError::NoNamespaces)
    );
    assert_eq!(
        HostJobSource::new([""], store).err(),
        Some(HostJobSourceError::EmptyNamespace)
    );
}

#[tokio::test]
async fn the_bound_job_source_composes_the_hosts_namespaces_with_the_states_jobs() {
    let fixture = CouncilFixture::new_runtime_backed(|_| ScriptedTurn::Text("ok".into()));
    let facts = RetainedJobFacts {
        owner_session_id: SessionId::new(),
        retained_work: None,
        result_digest: "sha256:setup".into(),
    };
    let setup = HostJobSource::new(
        ["app.setup"],
        Arc::new(SetupStore {
            facts: facts.clone(),
        }),
    )
    .expect("a named namespace");
    let state = MobMcpState::new_with_runtime_adapter(
        fixture.service.clone(),
        fixture.runtime_adapter.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    )
    .expect("construct runtime authority")
    .with_host_job_source(setup)
    .into_shared();
    assert!(state.continuation_service().is_none(), "not bound yet");

    let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
        meerkat_runtime::InMemoryRuntimeStore::new(),
    ));
    let bindings = meerkat::ContinuationHostBindings::default();
    state
        .bind_continuations(inbox, &bindings)
        .expect("bind the state's continuations");
    assert!(
        state.continuation_service().is_some(),
        "the bound service is the host's to submit through"
    );
    let bound = bindings.job_source().expect("a live binding");

    assert_eq!(
        bound.retained_job(&host("app.setup"), "setup-1").await,
        RetainedJobLookup::Found(facts),
        "the host's namespace is answered by the host's owner"
    );
    assert_eq!(
        bound.retained_job(&host("app.other"), "setup-1").await,
        RetainedJobLookup::Absent,
        "a namespace the host did not declare is owned by no one"
    );
    assert!(
        !matches!(
            bound
                .retained_job(&ContinuationProducer::ForkOff, "setup-1")
                .await,
            RetainedJobLookup::Found(_)
        ),
        "fork_off jobs stay with the state's own owner, which holds none here"
    );
}
