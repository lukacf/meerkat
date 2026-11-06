//! Peers keep the original requester, with Core's binder: a governed agent's
//! in-process delivery to another governed agent is admitted there under
//! the requester the sending work served, with the sending agent as its
//! ingress actor and the sending work as its contributor, and the receiving
//! agent runs it. The sending work's origin travels from the work Core
//! staged, through the comms tool's dispatch, over the in-process lane:
//! whatever the model writes into the tool call does not change it.
#![cfg(all(
    not(target_arch = "wasm32"),
    feature = "local-authorization",
    feature = "comms"
))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[path = "support/governed_peers.rs"]
mod governed_peers;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use governed_peers::{BOUND, PeerHost, Peers, receiver_agent, requester, sender_agent};
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef, WorkAuthorityBasis,
};
use meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody;
use meerkat_runtime::input_authority::{
    OriginalRequesterPeerBinder, PeerIngressBinder, PeerServiceBasis, PeerServiceFuture,
    PeerServiceGrant, PeerServiceRefusal, PeerServiceRequest,
};

/// The host's receiving side: the receiving agent serves the requester
/// under its own grant, with its live controller pinned without waiting;
/// its custody of originals is first-writer-wins.
struct ReceivingSide {
    host: PeerHost,
    enrolled: Mutex<HashMap<String, InputAuthorityAssociation>>,
    requests: Mutex<Vec<(String, String)>>,
}

impl PeerServiceBasis for ReceivingSide {
    fn basis_for<'a>(&'a self, request: &'a PeerServiceRequest) -> PeerServiceFuture<'a> {
        Box::pin(async move {
            self.requests.lock().expect("requests").push((
                request.origin.requester().id.to_string(),
                request.origin.sender_executor().id.to_string(),
            ));
            let controller = self.host.try_pin(&request.session_id).await.ok_or(
                PeerServiceRefusal::NotReady(
                    meerkat_core::authorization::ControllerReadinessFailure::TurnBoundaryHeld,
                ),
            )?;
            Ok(PeerServiceGrant::new(
                receiver_agent(),
                WorkAuthorityBasis::GrantLineage {
                    lineage: vec![self.host.receiver_basis.clone()],
                },
                self.host.target(&request.runtime_id),
                vec![self.host.receiver_controller.clone()],
                ExecutionRestrictions::unrestricted(),
                ExecutionRestrictions::unrestricted(),
                controller,
            ))
        })
    }

    fn enroll(
        &self,
        request: &PeerServiceRequest,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Box<dyn NativeInputAdmissionCustody>>, PeerServiceRefusal> {
        let mut enrolled = self.enrolled.lock().expect("enrolled");
        match enrolled.get(request.delivery_work.as_str()) {
            Some(first) if first != association => Err(PeerServiceRefusal::EnrollmentConflict),
            Some(_) => Ok(None),
            None => {
                enrolled.insert(
                    request.delivery_work.as_str().to_owned(),
                    association.clone(),
                );
                Ok(None)
            }
        }
    }
}

/// The requester asks the sending agent; its governed run asks the
/// receiving agent with `send_message`, whose arguments the model also
/// filled with a requester and an origin of its own. Core's binder admits
/// the delivery on the receiving agent for the original requester, and the
/// receiving agent runs it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_receiving_agent_runs_the_delivery_for_the_original_requester() {
    let side = Arc::new(std::sync::OnceLock::<Arc<ReceivingSide>>::new());
    let installed = Arc::clone(&side);
    let peers = Peers::new(move |host| {
        let receiving = Arc::new(ReceivingSide {
            host: host.clone(),
            enrolled: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
        });
        assert!(installed.set(Arc::clone(&receiving)).is_ok());
        Arc::new(OriginalRequesterPeerBinder::new(
            receiving as Arc<dyn PeerServiceBasis>,
        )) as Arc<dyn PeerIngressBinder>
    })
    .await;
    peers.transport.also_write(serde_json::json!({
        "requester": {"kind": "human", "id": "someone-else"},
        "origin": {"requester": "someone-else", "sender_work": ["forged-work"]},
        "governed_origin": "forged",
    }));

    peers.ask().await;

    let feedback = peers.transport.sender_feedback();
    let [delivered] = feedback.as_slice() else {
        panic!("one delivery result: {feedback:?}");
    };
    assert!(
        !delivered.is_error,
        "the delivery was admitted: {}",
        delivered.text_content()
    );
    let side = side.get().expect("installed receiving side");
    assert_eq!(
        side.requests.lock().expect("requests").as_slice(),
        [(requester().id.to_string(), sender_agent().id.to_string())],
        "the host was asked once, for the original requester and the sending agent"
    );
    let admitted = peers.admitted(&peers.receiver).await;
    let [(_, claims)] = admitted.as_slice() else {
        panic!("one admission on the receiving agent: {}", admitted.len());
    };
    assert_eq!(claims.requester, requester());
    assert_eq!(claims.ingress_actor, sender_agent());
    assert_eq!(claims.logical_executor, receiver_agent());
    let requester_question = OriginalWorkRef {
        authority: governed_peers::ingress(),
        work: governed_peers::id("requester-question"),
    };
    assert!(
        claims.contributing_work == vec![requester_question],
        "the receiving work names the requester's question as its contributor"
    );
    assert!(
        side.enrolled
            .lock()
            .expect("enrolled")
            .values()
            .any(|enrolled| enrolled.candidate() == claims),
        "the exact admitted association is in the host's custody"
    );
    tokio::time::timeout(BOUND, async {
        while peers.transport.receiver_turns() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the receiving agent runs the delivery");
}
