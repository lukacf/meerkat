//! Peers keep the original requester, behaviour form: a governed work's
//! in-process delivery to another governed agent is never admitted there
//! under another requester than the sending work's.
//!
//! The receiving side installs the kind of peer ingress binder a governed
//! host had to write itself while the bind request named no requester: it
//! binds every delivery under the host's own peer requester. Admitting that
//! binding loses the requester the sending work served, and the receiving
//! agent then runs under a requester nobody asked for.
#![cfg(all(
    not(target_arch = "wasm32"),
    feature = "local-authorization",
    feature = "comms"
))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "support/governed_peers.rs"]
mod governed_peers;

use std::sync::{Arc, Mutex};

use governed_peers::{
    PeerHost, Peers, evidence, ingress, ingress_domain, realm, receiver_agent, requester,
};
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, InputAuthorityAssociationCandidate, OriginalWorkRef,
    QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::PeerIngressSenderAuthentication;
use meerkat_runtime::input_authority::{
    PeerIngressBindFuture, PeerIngressBindRefusal, PeerIngressBindRequest, PeerIngressBinder,
    PeerIngressBinding, PeerIngressOrigin,
};

/// The requester a host's own binder names for every peer delivery.
fn host_peer_requester() -> meerkat_core::PrincipalRef {
    governed_peers::principal("peer-host")
}

/// A governed host's own binder of its agents' peer deliveries: it pins the
/// receiving agent's live controller and binds Core's exact input under the
/// host's own peer requester, the only requester it can name.
struct HostRequesterBinder {
    host: PeerHost,
    /// How the transport established the sender of each delivery offered.
    offered: Mutex<Vec<PeerIngressSenderAuthentication>>,
}

impl HostRequesterBinder {
    async fn bind(
        &self,
        request: PeerIngressBindRequest,
    ) -> Result<PeerIngressBinding, PeerIngressBindRefusal> {
        let PeerIngressOrigin::Comms(origin) = &request.origin else {
            return Err(PeerIngressBindRefusal::NotOffered);
        };
        self.offered
            .lock()
            .expect("offers")
            .push(origin.sender_authentication);
        let pin = self.host.try_pin(&request.session_id).await.ok_or(
            PeerIngressBindRefusal::NotReady(
                meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
            ),
        )?;
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: host_peer_requester(),
            ingress_actor: ingress(),
            represented_subject: None,
            original_authentication: evidence("peer-host-authentication"),
            logical_executor: receiver_agent(),
            target: self.host.target(&request.runtime_id),
            original_work: OriginalWorkRef {
                authority: ingress(),
                work: request.delivery_work.clone(),
            },
            root_event: evidence("peer-delivery"),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![self.host.receiver_basis.clone()],
            },
            controller_grant_lineage: vec![self.host.receiver_controller.clone()],
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ingress_domain(),
                occurrence_scope: request.delivery_work.clone(),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .map_err(|error| PeerIngressBindRefusal::Unavailable(error.to_string()))?;
        let bound = meerkat_runtime::meerkat_machine::bind_scoped_original(
            request.input,
            association,
            request.event_key,
            host_peer_requester(),
            ingress(),
            realm(),
            evidence("peer-host-authentication"),
            pin,
        )
        .map_err(|error| PeerIngressBindRefusal::Unavailable(error.to_string()))?;
        Ok(PeerIngressBinding::new(bound))
    }
}

impl PeerIngressBinder for HostRequesterBinder {
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
        Box::pin(self.bind(request))
    }
}

/// The requester asks the sending agent, whose governed run asks the
/// receiving agent with `send_message` over the in-process lane. The
/// receiving side's binder is offered the delivery (an in-process sender),
/// and whatever it binds, nothing is admitted on the receiving agent under
/// another requester than the one the sending work served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_governed_in_process_delivery_is_never_admitted_under_another_requester() {
    let binder = Arc::new(std::sync::OnceLock::<Arc<HostRequesterBinder>>::new());
    let installed = Arc::clone(&binder);
    let peers = Peers::new(move |host| {
        let host_binder = Arc::new(HostRequesterBinder {
            host: host.clone(),
            offered: Mutex::new(Vec::new()),
        });
        assert!(installed.set(Arc::clone(&host_binder)).is_ok());
        host_binder as Arc<dyn PeerIngressBinder>
    })
    .await;

    peers.ask().await;

    let offered = binder
        .get()
        .expect("installed binder")
        .offered
        .lock()
        .expect("offers")
        .clone();
    assert_eq!(
        offered,
        vec![PeerIngressSenderAuthentication::InProcess],
        "the sending agent's one delivery reached the receiving side's binder from an in-process sender"
    );
    for (input, claims) in peers.admitted(&peers.receiver).await {
        assert!(
            claims.requester == requester(),
            "the receiving agent admitted input {input} under requester {:?}, not under the \
             requester its sending work served ({:?}): the peer delivery lost its original requester",
            claims.requester,
            requester(),
        );
    }
}
