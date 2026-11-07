//! The member host's operator-instruction serving (supervisor bridge
//! protocol 7): a controlling mob records a placed member's snapshot in the
//! member's session (`RecordMemberInstructionTarget`), and releases its
//! target when the member retired and the host keeps the session
//! (`ReleaseMemberInstructionTarget`).
//!
//! Both are admitted on the responder turn (host authority, then the
//! member's exact incarnation for a record, and for a release that no
//! member holds the session), then served off it: a live member session
//! records under its turn-finalization boundary, which a run in progress
//! holds, so the responder never waits on it. The writes of one session run
//! one at a time, in admission order, and a release admitted after a record
//! moves the session's epoch, so a record that had not started by then
//! writes nothing after the release. Each write is bounded on the host; one
//! past its bound answers `Unavailable` and the controller retries it (a
//! write whose session had already taken its turn boundary still lands: the
//! session service finishes it, and the retry answers "already recorded").

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeInstructionTargetRecordedResponse, BridgeInstructionTargetReleasedResponse,
    BridgeRecordInstructionTargetPayload, BridgeRejectionCause,
    BridgeReleaseInstructionTargetPayload, BridgeReply,
};
use meerkat_core::interaction::PeerInputCandidate;
use meerkat_core::service::{SessionError, SessionInstructionError};
use meerkat_core::types::SessionId;

use super::{
    AuthorityAgentIdentity, AuthorityMobId, MemberKey, MobHostActor, MobHostBindingAuthorityState,
};
use crate::operator_instruction::{MemberInstructionDelivery, MemberInstructionRelease};
use crate::runtime::host_reply::HostBridgeReply;
use crate::runtime::session_service::MobSessionService;

/// How long the host waits for one record or release to land before it
/// answers `Unavailable`. Well inside the controller's round-trip bound
/// (`PLACED_INSTRUCTION_BRIDGE_TIMEOUT`), so a write the host gave up on has
/// ended before the controller sends the next one.
const HOST_INSTRUCTION_WRITE_BOUND: Duration = Duration::from_secs(10);

/// An operator-instruction write in flight off the responder turn.
pub(super) type InstructionWriteFuture = Pin<Box<dyn Future<Output = InstructionWriteDone> + Send>>;

/// A finished write: the request it answers and its reply.
pub(super) struct InstructionWriteDone {
    candidate: PeerInputCandidate,
    reply: HostBridgeReply,
}

/// The host's operator-instruction writes: one lane per member session and
/// the writes admitted since the responder last collected them. The actor
/// holds the admitted writes as data; the responder runs them.
#[derive(Default)]
pub(super) struct InstructionWriteLanes {
    lanes: BTreeMap<SessionId, Arc<InstructionWriteLane>>,
    admitted: Vec<AdmittedInstructionWrite>,
}

/// One admitted write: the request it answers, its session's lane and what
/// it writes.
pub(super) struct AdmittedInstructionWrite {
    candidate: PeerInputCandidate,
    lane: Arc<InstructionWriteLane>,
    service: Arc<dyn MobSessionService>,
    write: InstructionWrite,
}

enum InstructionWrite {
    /// Record a snapshot, unless a release was admitted after it (the
    /// lane's epoch moved on from `epoch`).
    Record {
        epoch: u64,
        delivery: MemberInstructionDelivery,
    },
    Release(MemberInstructionRelease),
}

impl AdmittedInstructionWrite {
    /// The write, run off the responder turn: it answers its request when it
    /// finishes.
    pub(super) fn run(self) -> InstructionWriteFuture {
        let Self {
            candidate,
            lane,
            service,
            write,
        } = self;
        Box::pin(async move {
            let reply = match write {
                InstructionWrite::Record { epoch, delivery } => {
                    lane.record(epoch, service, delivery).await
                }
                InstructionWrite::Release(release) => lane.release(service, release).await,
            };
            InstructionWriteDone { candidate, reply }
        })
    }
}

/// One member session's lane: its writes run one at a time, and its epoch
/// moves on at every admitted release.
#[derive(Default)]
struct InstructionWriteLane {
    order: tokio::sync::Mutex<()>,
    epoch: AtomicU64,
}

impl InstructionWriteLanes {
    fn lane(&mut self, session_id: &SessionId) -> Arc<InstructionWriteLane> {
        Arc::clone(self.lanes.entry(session_id.clone()).or_default())
    }

    /// The writes admitted since the last call, for the responder to run.
    pub(super) fn take_admitted(&mut self) -> Vec<AdmittedInstructionWrite> {
        std::mem::take(&mut self.admitted)
    }

    /// Drop the lanes no write holds any more.
    fn prune(&mut self) {
        self.lanes.retain(|_, lane| Arc::strong_count(lane) > 1);
    }
}

/// Whether the host holds `payload`'s member at exactly its incarnation: its
/// materialized row names the same member generation, fence and session, on
/// this host under the payload's binding generation.
fn incarnation_held(
    state: &MobHostBindingAuthorityState,
    host_id: Option<&str>,
    payload: &BridgeRecordInstructionTargetPayload,
) -> bool {
    let member = &payload.member;
    let key = MemberKey::new(
        AuthorityMobId::from(member.mob_id.as_str()),
        AuthorityAgentIdentity::from(member.agent_identity.as_str()),
    );
    host_id == Some(member.host_id.as_str())
        && member.binding_generation == payload.binding_generation
        && state
            .materialized_generations
            .get(&key)
            .map(|generation| generation.0)
            == Some(member.generation)
        && state.materialized_fences.get(&key).map(|fence| fence.0) == Some(member.fence_token)
        && state
            .materialized_sessions
            .get(&key)
            .map(|session| session.0.as_str())
            == Some(member.member_session_id.as_str())
}

/// Whether any member this host holds is bound to `session_id`.
fn session_held(state: &MobHostBindingAuthorityState, session_id: &str) -> bool {
    state
        .materialized_sessions
        .values()
        .any(|session| session.0 == session_id)
}

/// The typed rejection of a session's refusal: a closed refusal code travels
/// as `InstructionTargetRefused`; a session that cannot be read or written
/// now is `Unavailable` (the controller retries).
fn refusal_reply(error: &SessionInstructionError) -> HostBridgeReply {
    match error.code() {
        Some(code) => HostBridgeReply::rejected(
            BridgeRejectionCause::InstructionTargetRefused { code },
            format!("the member session refused it: {error}"),
        ),
        None => HostBridgeReply::rejected(
            BridgeRejectionCause::Unavailable,
            format!("the member session cannot take it now: {error}"),
        ),
    }
}

fn past_bound() -> HostBridgeReply {
    HostBridgeReply::rejected(
        BridgeRejectionCause::Unavailable,
        "the member session did not take the write within the host's bound (its turn \
         boundary was held); retry",
    )
}

impl InstructionWriteLane {
    async fn record(
        self: Arc<Self>,
        epoch: u64,
        service: Arc<dyn MobSessionService>,
        delivery: MemberInstructionDelivery,
    ) -> HostBridgeReply {
        let _turn = self.order.lock().await;
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return HostBridgeReply::rejected(
                BridgeRejectionCause::StaleFence,
                "the member session's target was released after this record was admitted",
            );
        }
        match tokio::time::timeout(
            HOST_INSTRUCTION_WRITE_BOUND,
            service.record_member_instruction_target(delivery),
        )
        .await
        {
            Ok(Ok(receipt)) => {
                HostBridgeReply::completed(BridgeReply::MemberInstructionTargetRecorded(
                    BridgeInstructionTargetRecordedResponse {
                        disposition: receipt.disposition,
                        target_generation: receipt.target_generation,
                    },
                ))
            }
            Ok(Err(error)) => refusal_reply(&error),
            Err(_) => past_bound(),
        }
    }

    async fn release(
        self: Arc<Self>,
        service: Arc<dyn MobSessionService>,
        release: MemberInstructionRelease,
    ) -> HostBridgeReply {
        let _turn = self.order.lock().await;
        let released = |disposition| {
            HostBridgeReply::completed(BridgeReply::MemberInstructionTargetReleased(
                BridgeInstructionTargetReleasedResponse { disposition },
            ))
        };
        match tokio::time::timeout(
            HOST_INSTRUCTION_WRITE_BOUND,
            service.release_member_instruction_target(release),
        )
        .await
        {
            Ok(Ok(disposition)) => released(Some(disposition)),
            // An archived or gone session holds no target to release.
            Ok(Err(SessionInstructionError::Session(SessionError::NotFound { .. }))) => {
                released(None)
            }
            Ok(Err(error)) => refusal_reply(&error),
            Err(_) => past_bound(),
        }
    }
}

impl MobHostActor {
    /// V7 `RecordMemberInstructionTarget`: admitted for the member's exact
    /// incarnation, then recorded off the responder turn.
    pub(super) async fn serve_record_member_instruction_target(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeRecordInstructionTargetPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.member.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        let Some(materializer) = self.materializer.as_ref() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "host composed without a member substrate",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        };
        let host_id = self.host_comms.peer_id().map(|peer| peer.to_string());
        if !incarnation_held(self.binding_authority.state(), host_id.as_deref(), &payload) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::StaleFence,
                "the member is not materialized on this host at that incarnation",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let session_id = match SessionId::parse(&payload.member.member_session_id) {
            Ok(session_id) => session_id,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("the recorded member session id is invalid: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let service = Arc::clone(&materializer.substrate().session_service);
        let lane = self.instruction_writes.lane(&session_id);
        let epoch = lane.epoch.load(Ordering::SeqCst);
        let record = payload.target.into_record(
            payload.member.mob_id.clone(),
            payload.member.agent_identity.clone(),
        );
        let delivery = MemberInstructionDelivery::authorized(session_id, record);
        self.instruction_writes
            .admitted
            .push(AdmittedInstructionWrite {
                candidate: candidate.clone(),
                lane,
                service,
                write: InstructionWrite::Record { epoch, delivery },
            });
    }

    /// V7 `ReleaseMemberInstructionTarget`: admitted once no member this host
    /// holds is bound to the session, then released off the responder turn.
    pub(super) async fn serve_release_member_instruction_target(
        &mut self,
        candidate: &PeerInputCandidate,
        payload: BridgeReleaseInstructionTargetPayload,
    ) {
        let reply_address = payload.supervisor.address.clone();
        if !self
            .admit_host_command(
                candidate,
                &payload.mob_id,
                payload.epoch,
                payload.binding_generation,
                &reply_address,
            )
            .await
        {
            return;
        }
        let Some(materializer) = self.materializer.as_ref() else {
            self.send_failure(
                candidate,
                BridgeRejectionCause::Unavailable,
                "host composed without a member substrate",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        };
        if session_held(self.binding_authority.state(), &payload.member_session_id) {
            self.send_failure(
                candidate,
                BridgeRejectionCause::StaleFence,
                "a member this host holds is bound to the session",
                Some(reply_address.as_str()),
            )
            .await;
            return;
        }
        let session_id = match SessionId::parse(&payload.member_session_id) {
            Ok(session_id) => session_id,
            Err(error) => {
                self.send_failure(
                    candidate,
                    BridgeRejectionCause::Internal,
                    format!("the member session id is invalid: {error}"),
                    Some(reply_address.as_str()),
                )
                .await;
                return;
            }
        };
        let service = Arc::clone(&materializer.substrate().session_service);
        let lane = self.instruction_writes.lane(&session_id);
        lane.epoch.fetch_add(1, Ordering::SeqCst);
        let release = MemberInstructionRelease::authorized(
            session_id,
            crate::ids::MobId::from(payload.mob_id.as_str()),
        );
        self.instruction_writes
            .admitted
            .push(AdmittedInstructionWrite {
                candidate: candidate.clone(),
                lane,
                service,
                write: InstructionWrite::Release(release),
            });
    }

    /// Answer a finished write and drop the lanes no write holds.
    pub(super) async fn finish_instruction_write(&mut self, done: InstructionWriteDone) {
        self.send_reply(&done.candidate, done.reply, None).await;
        self.instruction_writes.prune();
    }
}
