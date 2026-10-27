//! Mob announcement delivery between gateways, over keyed inject.
//!
//! [`RemoteMemberDelivery`] is the calling side: it delivers a batch whose
//! member lives on a peer gateway with `inject_keyed`, and reads that
//! batch's native status there with `inject_status`, over the signed control
//! channel. [`KeyedInjectServer`] is the serving side: it submits what a peer
//! delivers to this gateway's own continuation owner. See
//! [`crate::runtime::keyed_inject`] for the signing, freshness and evidence
//! rules both sides follow.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::availability::{BlockCause, EvidenceClass, MemberDeparture, SuspendCause};
use super::native::{
    IdentityService, MemberProblem, MobNativeDelivery, NativeReceipt, NativeStatus,
    NativeStatusError, NativeSubmitError, NativeUnavailable, PrepareError, batch_summary,
    continuation_delivery, not_served_here_detail,
};
use super::types::{Batch, BatchId, MAX_ID_BYTES, Member, MemberIdentity, MobId, content_digest};
use crate::runtime::cross_mob_control::ControlSignerSlot;
use crate::runtime::cross_mob_remote::RemoteMobProxy;
use crate::runtime::keyed_inject::{
    KeyedInject, KeyedInjectCallError, KeyedInjectCallOutcome, KeyedInjectCallerScope,
    KeyedInjectFuture, KeyedInjectReceiver, KeyedInjectRefusal, KeyedInjectRefusalCode,
    KeyedInjectStatusAnswer, NotServedCause, StaleRequestCause, deadline_passed,
};

pub(crate) type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub(crate) fn system_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// After a peer could not be reached, calls to it fail fast for this long.
/// This keeps one unreachable peer from stalling a dispatcher round on
/// connection timeouts; it decides nothing about any delivery, which stays
/// owed and is asked about again.
pub(crate) const PEER_BACKOFF_MS: u64 = 5_000;

/// The calling side: delivers to members on peer gateways named in this
/// gateway's contact directory, authenticated as this gateway's own key.
pub struct RemoteMemberDelivery {
    contacts: crate::contact_directory::ContactDirectory,
    keys: ControlSignerSlot,
    clock: Clock,
    /// This gateway's configured request window, which a peer's
    /// `stale_request` is read against: the refusal's text never is.
    window_ms: u64,
    unreachable_until: Mutex<BTreeMap<MobId, u64>>,
}

impl RemoteMemberDelivery {
    pub(crate) fn new(
        contacts: crate::contact_directory::ContactDirectory,
        keys: ControlSignerSlot,
        window_ms: u64,
    ) -> Self {
        Self::with_clock(contacts, keys, window_ms, Arc::new(system_clock))
    }

    /// `clock` stamps each request's issue time: this gateway's wall clock,
    /// which the peer compares with its own.
    pub(crate) fn with_clock(
        contacts: crate::contact_directory::ContactDirectory,
        keys: ControlSignerSlot,
        window_ms: u64,
        clock: Clock,
    ) -> Self {
        Self {
            contacts,
            keys,
            clock,
            window_ms,
            unreachable_until: Mutex::new(BTreeMap::new()),
        }
    }

    /// The one reading of a peer's signed refusal of a submission, over
    /// every code this build knows and any it does not. Each signed "nothing
    /// was committed" is an availability state that can change; only
    /// `member_departed` ends anything, and a code this build cannot read
    /// leaves the outcome unknown. The causes of `not_served` and
    /// `stale_request` are read from the refusal's own signed field, never
    /// from its message: only a request outside the peer's window is clock
    /// skew.
    pub(super) fn submit_refusal(
        peer: &MobId,
        issued_at_ms: u64,
        window_ms: u64,
        code: &str,
        message: String,
        cause: Option<&str>,
    ) -> NativeSubmitError {
        use KeyedInjectRefusalCode as Code;
        let signed = |cause| NativeSubmitError::NotCommitted {
            cause,
            evidence: EvidenceClass::Verified,
        };
        let Some(known) = Code::parse(code) else {
            return NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerRefusedUnknown {
                    code: code.to_string(),
                },
                evidence: EvidenceClass::Verified,
                detail: message,
            };
        };
        match known {
            Code::Unavailable => signed(SuspendCause::PeerComposing),
            Code::NotServed => signed(SuspendCause::PeerNotServing {
                cause: NotServedCause::read(cause),
            }),
            Code::StaleRequest => match StaleRequestCause::read(cause) {
                StaleRequestCause::OutsideWindow => signed(SuspendCause::ClockSkew {
                    issued_at_ms,
                    window_ms,
                }),
                cause => signed(SuspendCause::StaleRequest { cause }),
            },
            Code::InvalidRequest => NativeSubmitError::Blocked {
                cause: BlockCause::PeerInvalidRequest,
                detail: message,
            },
            Code::UnknownMember => signed(SuspendCause::MemberUnknown),
            Code::UnusableMember => signed(SuspendCause::MemberNotServedHere { cause: None }),
            Code::Governed => signed(SuspendCause::Governed),
            Code::Conflict => NativeSubmitError::Blocked {
                cause: BlockCause::Conflict,
                detail: message,
            },
            Code::OwnerRetired => signed(SuspendCause::MemberNotLive),
            Code::NativeUnavailable => NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerNativeUnavailable,
                evidence: EvidenceClass::Verified,
                detail: message,
            },
            Code::MemberDeparted => {
                NativeSubmitError::MemberGone(MemberDeparture::from_signed_departure(peer.clone()))
            }
        }
    }

    /// The one reading of a call that brought no signed answer. Nothing in
    /// an unsigned answer is evidence: its code is kept as text and never
    /// branched on. Only the local failures say that the peer was never
    /// asked.
    pub(super) fn call_error(error: KeyedInjectCallError) -> NativeSubmitError {
        match error {
            KeyedInjectCallError::Oversize { bytes } => NativeSubmitError::Blocked {
                cause: BlockCause::RequestOversize,
                detail: format!(
                    "the delivery request is {bytes} bytes, more than one control frame"
                ),
            },
            KeyedInjectCallError::NotAuthenticatable(_) => NativeSubmitError::NotCommitted {
                cause: SuspendCause::LocalNotAuthenticatable,
                evidence: EvidenceClass::Local,
            },
            KeyedInjectCallError::Unauthenticated(detail) => NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::AnswerFailedVerification,
                evidence: EvidenceClass::Unverified,
                detail,
            },
            KeyedInjectCallError::Rejected { code, message } => NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerUnanswered { code: Some(code) },
                evidence: EvidenceClass::Unverified,
                detail: message,
            },
            KeyedInjectCallError::Unexpected(detail) | KeyedInjectCallError::Transport(detail) => {
                NativeSubmitError::OutcomeUnknown {
                    cause: SuspendCause::PeerUnanswered { code: None },
                    evidence: EvidenceClass::Unverified,
                    detail,
                }
            }
        }
    }

    /// What a read that brought no status leaves: the batch's outcome is as
    /// unknown as it was, and it keeps waiting with the reason. Only a
    /// signed `invalid_request` rules the batch out, and only a signed
    /// `member_departed` is final; a conflict rules out a submission, never
    /// a read.
    pub(super) fn status_error(answer: NativeSubmitError, code: Option<&str>) -> NativeStatusError {
        match answer {
            NativeSubmitError::NotCommitted { cause, evidence } => NativeStatusError::Unavailable {
                cause,
                evidence,
                detail: String::new(),
            },
            NativeSubmitError::OutcomeUnknown {
                cause,
                evidence,
                detail,
            } => NativeStatusError::Unavailable {
                cause,
                evidence,
                detail,
            },
            NativeSubmitError::Blocked {
                cause: BlockCause::PeerInvalidRequest,
                detail,
            } => NativeStatusError::Blocked {
                cause: BlockCause::PeerInvalidRequest,
                detail,
            },
            NativeSubmitError::Blocked { detail, .. } => match code {
                // A code this build knows is named as known; only one it
                // cannot read is unknown.
                Some(code) => NativeStatusError::Unavailable {
                    cause: if crate::runtime::keyed_inject::KeyedInjectRefusalCode::parse(code)
                        .is_some()
                    {
                        SuspendCause::PeerRefused {
                            code: code.to_string(),
                        }
                    } else {
                        SuspendCause::PeerRefusedUnknown {
                            code: code.to_string(),
                        }
                    },
                    evidence: EvidenceClass::Verified,
                    detail,
                },
                None => NativeStatusError::local(detail),
            },
            NativeSubmitError::MemberGone(departure) => NativeStatusError::MemberGone(departure),
        }
    }

    fn reachable(&self, peer: &MobId) -> Result<(), KeyedInjectCallError> {
        let now = (self.clock)();
        let unreachable = self
            .unreachable_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match unreachable.get(peer) {
            Some(until) if now < *until => Err(KeyedInjectCallError::Transport(format!(
                "'{peer}' could not be reached moments ago; the next attempt is at {until} ms"
            ))),
            _ => Ok(()),
        }
    }

    /// Remember whether the peer answered at all. Any answer, a refusal
    /// included, means it is reachable.
    fn note<T>(&self, peer: &MobId, outcome: &Result<T, KeyedInjectCallError>) {
        let mut unreachable = self
            .unreachable_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match outcome {
            Err(KeyedInjectCallError::Transport(_)) => {
                unreachable.insert(peer.clone(), (self.clock)().saturating_add(PEER_BACKOFF_MS));
            }
            _ => {
                unreachable.remove(peer);
            }
        }
    }

    fn proxy(&self, peer: &MobId) -> Result<RemoteMobProxy, KeyedInjectCallError> {
        let entry = self.contacts.get(peer.as_str()).ok_or_else(|| {
            KeyedInjectCallError::NotAuthenticatable(format!(
                "the contact directory has no entry for '{peer}'"
            ))
        })?;
        let keys = self
            .keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let proxy = RemoteMobProxy::from_entry_with_caller(entry, keys)
            .map_err(|error| KeyedInjectCallError::NotAuthenticatable(error.to_string()))?
            .ok_or_else(|| {
                KeyedInjectCallError::NotAuthenticatable(format!(
                    "the contact for '{peer}' is in-process, not a peer gateway"
                ))
            })?;
        proxy.keyed_inject_ready()?;
        Ok(proxy)
    }

    /// The peer gateway a member's mob is filed under in the contact
    /// directory: the member's own mob id.
    fn peer_of(member: &Member) -> &MobId {
        &member.mob_id
    }

    /// Whether the contact directory files a peer under `mob_id`.
    pub(crate) fn files(&self, mob_id: &MobId) -> bool {
        self.contacts.get(mob_id.as_str()).is_some()
    }

    /// A member on a peer is deliverable when its identity is in public
    /// form and the peer can be called authenticated. Nothing is sent.
    pub(crate) fn check_member(&self, member: &Member) -> Result<(), MemberProblem> {
        let identity = member.identity.as_str();
        if crate::member_comms_id::runtime_alias_str(identity) != identity
            || crate::member_comms_id::is_reserved_generated_alias(identity)
        {
            return Err(MemberProblem::Invalid(format!(
                "'{identity}' is not a durable identity in its public form"
            )));
        }
        let peer = Self::peer_of(member);
        self.proxy(peer)
            .map(|_| ())
            .map_err(|error| MemberProblem::UnusablePeer(error.to_string()))
    }

    /// Deliver the batch to its peer. Only the peer's signed answer is
    /// evidence about its member; a call that brought none leaves the
    /// outcome unknown, except where this gateway never asked.
    pub(crate) async fn submit(&self, batch: &Batch) -> Result<NativeReceipt, NativeSubmitError> {
        let peer = Self::peer_of(&batch.member);
        let issued_at_ms = (self.clock)();
        let outcome = match self.reachable(peer).and_then(|()| self.proxy(peer)) {
            Ok(proxy) => {
                let summary = batch_summary(batch);
                let outcome = proxy
                    .inject_keyed(
                        KeyedInject {
                            member: batch.member.identity.as_str(),
                            delivery_key: batch.id.as_str(),
                            content_digest: &batch.content_digest,
                            summary: &summary,
                            body: &batch.body,
                        },
                        issued_at_ms,
                    )
                    .await;
                self.note(peer, &outcome);
                outcome
            }
            Err(error) => Err(error),
        };
        match outcome {
            Ok(KeyedInjectCallOutcome::Answered(receipt)) => Ok(receipt),
            Ok(KeyedInjectCallOutcome::Refused {
                code,
                message,
                cause,
            }) => Err(Self::submit_refusal(
                peer,
                issued_at_ms,
                self.window_ms,
                &code,
                message,
                cause.as_deref(),
            )),
            Err(error) => Err(Self::call_error(error)),
        }
    }

    async fn read(
        &self,
        batch: &Batch,
        with_input: bool,
    ) -> Result<KeyedInjectStatusAnswer, NativeStatusError> {
        let peer = Self::peer_of(&batch.member);
        let issued_at_ms = (self.clock)();
        let outcome = match self.reachable(peer).and_then(|()| self.proxy(peer)) {
            Ok(proxy) => {
                let outcome = proxy
                    .inject_status(
                        batch.member.identity.as_str(),
                        batch.id.as_str(),
                        with_input,
                        issued_at_ms,
                    )
                    .await;
                self.note(peer, &outcome);
                outcome
            }
            Err(error) => Err(error),
        };
        match outcome {
            Ok(KeyedInjectCallOutcome::Answered(answer)) => Ok(answer),
            Ok(KeyedInjectCallOutcome::Refused {
                code,
                message,
                cause,
            }) => Err(Self::status_error(
                Self::submit_refusal(
                    peer,
                    issued_at_ms,
                    self.window_ms,
                    &code,
                    message,
                    cause.as_deref(),
                ),
                Some(&code),
            )),
            Err(error) => Err(Self::status_error(Self::call_error(error), None)),
        }
    }

    /// The batch's native status on its peer.
    pub(crate) async fn status(&self, batch: &Batch) -> Result<NativeStatus, NativeStatusError> {
        self.read(batch, false).await.map(|answer| answer.status)
    }

    /// The admitted input's state on the batch's peer.
    pub(crate) async fn input(&self, batch: &Batch) -> Result<Option<Value>, NativeUnavailable> {
        self.read(batch, true)
            .await
            .map(|answer| answer.input)
            .map_err(|error| NativeUnavailable(error.to_string()))
    }
}

/// The serving side: submits what a peer delivers to this gateway's own
/// continuation owner, as the continuation the peer's batch is, keyed under
/// the scope of the caller the grant admitted.
pub struct KeyedInjectServer {
    native: Arc<MobNativeDelivery>,
    window_ms: u64,
    clock: Clock,
    /// When this receiver was built: no request issued earlier is served.
    started_at_ms: u64,
}

impl KeyedInjectServer {
    pub(crate) fn new(native: Arc<MobNativeDelivery>, window_ms: u64) -> Self {
        Self::with_clock(native, window_ms, Arc::new(system_clock))
    }

    pub(crate) fn with_clock(native: Arc<MobNativeDelivery>, window_ms: u64, clock: Clock) -> Self {
        let started_at_ms = clock();
        Self {
            native,
            window_ms,
            clock,
            started_at_ms,
        }
    }
}

fn invalid(detail: impl std::fmt::Display) -> KeyedInjectRefusal {
    KeyedInjectRefusal::new(KeyedInjectRefusalCode::InvalidRequest, detail.to_string())
}

fn native_unavailable(error: NativeUnavailable) -> KeyedInjectRefusal {
    KeyedInjectRefusal::new(KeyedInjectRefusalCode::NativeUnavailable, error.0)
}

/// The signed answer for a member this gateway cannot deliver to now. Each
/// is "nothing was committed"; the caller's batch waits.
fn waiting_refusal(identity: &MemberIdentity, cause: &SuspendCause) -> KeyedInjectRefusal {
    match cause {
        SuspendCause::MemberUnknown => KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::UnknownMember,
            format!("{identity} is not an identity registered with this gateway"),
        ),
        SuspendCause::MemberNotServedHere { cause: Some(cause) } => KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::UnusableMember,
            not_served_here_detail(identity, *cause),
        ),
        SuspendCause::MemberNotServedHere { cause: None } => KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::UnusableMember,
            format!("{identity} is not served by this gateway's runtime"),
        ),
        SuspendCause::Governed => KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::Governed,
            "the member's runtime refuses host continuations",
        ),
        SuspendCause::MemberNotLive => KeyedInjectRefusal::new(
            KeyedInjectRefusalCode::OwnerRetired,
            format!("{identity} has no live incarnation; nothing was committed"),
        ),
        // Every other wait is this gateway's own native state: nothing is
        // implied, and the caller reads the status next.
        other => {
            KeyedInjectRefusal::new(KeyedInjectRefusalCode::NativeUnavailable, other.to_string())
        }
    }
}

/// Run what must happen before a delivery's commit within the time left
/// before `deadline_ms` on `clock`, then check the deadline once more: what
/// follows is the commit, which is never cut short. A deadline that passes
/// first is refused as stale (`deadline_passed`), and nothing is committed.
pub(super) async fn within_deadline<T>(
    deadline_ms: u64,
    clock: &Clock,
    before_commit: impl std::future::Future<Output = Result<T, KeyedInjectRefusal>>,
) -> Result<T, KeyedInjectRefusal> {
    let left = deadline_ms.saturating_sub(clock());
    let ready = tokio::time::timeout(std::time::Duration::from_millis(left), before_commit)
        .await
        .map_err(|_| deadline_passed(deadline_ms, clock()))??;
    let now = clock();
    if now > deadline_ms {
        return Err(deadline_passed(deadline_ms, now));
    }
    Ok(ready)
}

fn hosted_identity(member: &str) -> Result<MemberIdentity, KeyedInjectRefusal> {
    let identity = MemberIdentity::new(member).map_err(invalid)?;
    crate::identity_first::AgentIdentity::parse(identity.as_str()).map_err(invalid)?;
    Ok(identity)
}

/// The checks a delivery passes before anything native is touched: valid
/// ids, a bounded summary, and a body that matches its digest.
pub(crate) fn validate_delivery(
    delivery: &KeyedInject<'_>,
) -> Result<MemberIdentity, KeyedInjectRefusal> {
    let identity = hosted_identity(delivery.member)?;
    BatchId::new(delivery.delivery_key).map_err(invalid)?;
    if delivery.summary.len() > MAX_ID_BYTES {
        return Err(invalid(format!(
            "the summary is longer than {MAX_ID_BYTES} bytes"
        )));
    }
    if content_digest(delivery.body) != delivery.content_digest {
        return Err(invalid("the body does not match its content digest"));
    }
    Ok(identity)
}

impl KeyedInjectServer {
    /// Make the member ready for a delivery: a runtime that admits work only
    /// through a native work authorization host refuses it, the member must
    /// be served here, and its preparation must succeed.
    ///
    /// The governed check and the identity service lookup only read, so a
    /// deadline that drops this future between them leaves nothing behind.
    /// The preparation runs under the identity runtime's own task: the
    /// deadline drops only the wait for it.
    async fn ready(&self, identity: &MemberIdentity) -> Result<(), KeyedInjectRefusal> {
        match self.native.governs_work_authority() {
            Ok(false) => {}
            Ok(true) => {
                return Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::Governed,
                    "this gateway's runtime admits work only through a native work \
                     authorization host, which refuses host continuations",
                ));
            }
            Err(error) => return Err(native_unavailable(error)),
        }
        match self
            .native
            .identity_service(identity)
            .await
            .map_err(native_unavailable)?
        {
            IdentityService::Local => {}
            IdentityService::Unknown => {
                return Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::UnknownMember,
                    format!("{identity} is not an identity registered with this gateway"),
                ));
            }
            IdentityService::Elsewhere(cause) => {
                return Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::UnusableMember,
                    not_served_here_detail(identity, cause),
                ));
            }
        }
        self.native
            .prepare_identity(identity)
            .await
            .map_err(|error| match error {
                PrepareError::Suspended { cause, .. } => waiting_refusal(identity, &cause),
                // A member of this gateway's own mob has no final fact
                // before its departure is recorded; nothing is implied.
                PrepareError::Gone(end) => KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::NativeUnavailable,
                    end.to_string(),
                ),
                PrepareError::Unavailable(detail) => {
                    KeyedInjectRefusal::new(KeyedInjectRefusalCode::NativeUnavailable, detail)
                }
            })
    }
}

impl KeyedInjectReceiver for KeyedInjectServer {
    fn window_ms(&self) -> u64 {
        self.window_ms
    }

    fn now_ms(&self) -> u64 {
        (self.clock)()
    }

    fn started_at_ms(&self) -> u64 {
        self.started_at_ms
    }

    fn deliver<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        delivery: KeyedInject<'a>,
        deadline_ms: u64,
    ) -> KeyedInjectFuture<'a, Result<NativeReceipt, KeyedInjectRefusal>> {
        Box::pin(async move {
            let identity = validate_delivery(&delivery)?;
            let continuation = continuation_delivery(
                &scope.native_key(delivery.delivery_key),
                delivery.delivery_key,
                delivery.content_digest,
                delivery.summary,
                delivery.body,
            )
            .map_err(invalid)?;
            // The member checks and its preparation, which can wait on a
            // member that is not running yet, are bounded by the request's
            // deadline, and the deadline is checked once more just before the
            // commit: a delivery committed after it could land behind later
            // ones its caller already moved on to.
            within_deadline(deadline_ms, &self.clock, self.ready(&identity)).await?;
            match self
                .native
                .submit_delivery(&identity, continuation, self.now_ms())
                .await
            {
                Ok(receipt) => {
                    super::faults::after_remote_commit();
                    Ok(receipt)
                }
                Err(NativeSubmitError::NotCommitted { cause, .. }) => {
                    Err(waiting_refusal(&identity, &cause))
                }
                Err(NativeSubmitError::OutcomeUnknown { detail, .. }) => Err(
                    KeyedInjectRefusal::new(KeyedInjectRefusalCode::NativeUnavailable, detail),
                ),
                Err(NativeSubmitError::Blocked {
                    cause: BlockCause::Conflict,
                    detail,
                }) => Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::Conflict,
                    detail,
                )),
                Err(NativeSubmitError::Blocked { cause, detail }) => {
                    Err(invalid(cause.with_detail(&detail)))
                }
                // Only this gateway's own record of the member's departure
                // answers `member_departed`.
                Err(NativeSubmitError::MemberGone(departure)) => Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::MemberDeparted,
                    departure.to_string(),
                )),
            }
        })
    }

    fn status<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        member: &'a str,
        delivery_key: &'a str,
        with_input: bool,
    ) -> KeyedInjectFuture<'a, Result<KeyedInjectStatusAnswer, KeyedInjectRefusal>> {
        Box::pin(async move {
            let identity = hosted_identity(member)?;
            BatchId::new(delivery_key).map_err(invalid)?;
            let status = self
                .native
                .status_of(&identity, &scope.native_key(delivery_key))
                .await
                .map_err(|error| match error {
                    NativeStatusError::MemberGone(departure) => KeyedInjectRefusal::new(
                        KeyedInjectRefusalCode::MemberDeparted,
                        departure.to_string(),
                    ),
                    other => KeyedInjectRefusal::new(
                        KeyedInjectRefusalCode::NativeUnavailable,
                        other.to_string(),
                    ),
                })?;
            let input = match (&status, with_input) {
                (
                    NativeStatus::Applied {
                        session_id,
                        input_id,
                        ..
                    },
                    true,
                ) => self
                    .native
                    .input_state(session_id, input_id)
                    .await
                    .unwrap_or_else(|error| Some(serde_json::json!({ "unavailable": error.0 }))),
                _ => None,
            };
            Ok(KeyedInjectStatusAnswer { status, input })
        })
    }
}
