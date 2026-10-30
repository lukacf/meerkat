//! The stdio pipe's part of R3 human consent on a stock-governed realm.
//!
//! In the local-operator posture the pipe's operator is its only approver:
//! the operating-system user of this process, who authenticated the pipe by
//! starting it. That is device trust ([`ApprovalAssurance::DeviceTrusted`]),
//! not a person authenticated on the decision event, so a rule that
//! requires person authentication refuses it. Each decision is bound to the
//! exact pipe's [`HostAuthenticationScope`] (the operator and a process-unique
//! pipe id); the scope retires when the pipe closes, inside an owner change
//! on the composition's publication, so no eligibility observed for it
//! stays current. JSON never names the approver, its assurance or its scope:
//! the connection derives all three.
//!
//! Before any decision the pipe shows its operator what is asked: the
//! `requested` notice carries the approval's presentation, projected from
//! the persistence's one approval issuer's own record of it (the exact
//! action, its risk, the account and mandate the operation's owners stated,
//! the assurance the rule accepts and the expiry). An approval is issued
//! only while the pipe is open with its notices attached, a pending,
//! unexpired record is all it presents, and a decision on the pipe is
//! eligible only for an approval this pipe presented. The pipe answers for
//! its operator on an approval until a terminal transition (denied, used or
//! withdrawn) or its retirement: an approved consent is spent later, when
//! the approval owner may ask again whether its approver was revoked.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use meerkat::governed_realm::stock::StockGovernance;
use meerkat::governed_realm::stock::caller::StockLoginLifetime;
use meerkat::governed_realm::stock::consent::{
    STOCK_ACTION_ACCOUNT_LABEL, STOCK_CONSENT_ASSURANCE_LABEL, STOCK_REVIEW_MANDATE_LABEL,
    StockConsentSurface,
};
use meerkat_contracts::{ConsentNoticeParams, ConsentPresentationParams};
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentNotice, ConsentNoticeKind, ConsentPresentation,
    ConsentPresentationCandidate, ConsentRouteUnavailable, HostAuthenticationScope,
};
use meerkat_core::{
    ApprovalAssurance, ApprovalError, ApprovalId, ApprovalPrincipalId, ApprovalRecord,
    ApprovalRisk, ApprovalService, ApprovalStatus, PrincipalRef, StockConsentAssurance,
    SurfaceMetadata,
};

use crate::router::NotificationSink;

/// The notification carrying one recorded consent transition to the pipe.
pub use meerkat_contracts::CONSENT_NOTICE_NOTIFICATION;

/// The stdio pipe's consent surface; see the module documentation.
pub struct StdioConsentSurface {
    stock: Arc<StockGovernance>,
    /// The persistence's one approval issuer, the owner of every record the
    /// pipe presents, while the pipe is open. The issuer retains each
    /// pending consent candidate, and a candidate's consent route holds this
    /// surface: the pipe lets go of the issuer when it retires, so neither
    /// keeps the other (nor a candidate's session custody) alive after it.
    issuer: std::sync::Mutex<Option<ApprovalService>>,
    operator: PrincipalRef,
    requester: ApprovalPrincipalId,
    scope: HostAuthenticationScope,
    open: AtomicBool,
    notices: std::sync::Mutex<Option<NotificationSink>>,
    /// The approvals this pipe presented to its operator, from their
    /// `requested` notice until a terminal one (denied, used or withdrawn):
    /// the only ones a decision on it may decide, and, once its operator
    /// approved one, the one the pipe keeps answering for (its operator was
    /// not revoked) until it is spent, withdrawn or the pipe retires.
    presented: std::sync::Mutex<HashSet<ApprovalId>>,
}

impl std::fmt::Debug for StdioConsentSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioConsentSurface")
            .finish_non_exhaustive()
    }
}

impl StdioConsentSurface {
    /// The surface of one stdio pipe served by `stock`'s composition, for
    /// its local operator, presenting the records of `issuer` (the
    /// persistence's one approval issuer).
    ///
    /// # Errors
    /// The operator cannot be named as an approval requester.
    pub fn new(
        stock: &Arc<StockGovernance>,
        issuer: ApprovalService,
    ) -> Result<Arc<Self>, ApprovalError> {
        let operator = stock.principals().operator.clone();
        let requester = ApprovalPrincipalId::new(format!("human:{}", operator.id.as_str()))?;
        let scope = HostAuthenticationScope::new(format!(
            "stdio:{}:{}",
            operator.id.as_str(),
            uuid::Uuid::new_v4()
        ));
        Ok(Arc::new(Self {
            stock: Arc::clone(stock),
            issuer: std::sync::Mutex::new(Some(issuer)),
            operator,
            requester,
            scope,
            open: AtomicBool::new(true),
            notices: std::sync::Mutex::new(None),
            presented: std::sync::Mutex::default(),
        }))
    }

    /// The approver of a decision made on this pipe: its operator, device
    /// trusted, in this pipe's scope.
    ///
    /// # Errors
    /// The operator is not a qualified human.
    pub fn approver(&self) -> Result<AuthenticatedApprover, ApprovalError> {
        Ok(
            AuthenticatedApprover::new(self.operator.clone(), ApprovalAssurance::DeviceTrusted)?
                .with_authentication_scope(self.scope.clone()),
        )
    }

    /// Deliver this pipe's consent notices through `sink`, the connection's
    /// own notification channel.
    pub(crate) fn attach_notices(&self, sink: &NotificationSink) {
        let mut notices = self
            .notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if notices.is_none() {
            *notices = Some(sink.clone());
        }
    }

    /// The pipe's notification channel while the pipe is open; `None` once
    /// it closed or before its notices are attached.
    fn open_sink(&self) -> Option<NotificationSink> {
        if !self.open.load(Ordering::SeqCst) {
            return None;
        }
        self.notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn presented(&self) -> std::sync::MutexGuard<'_, HashSet<ApprovalId>> {
        self.presented
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The issuer slot. It is locked before `presented`, never the other way
    /// round, and no issuer method runs while it is held.
    fn issuer(&self) -> std::sync::MutexGuard<'_, Option<ApprovalService>> {
        self.issuer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Show the operator the approval a `requested` notice names, as the
    /// issuer recorded it, and make it decidable on this pipe. Nothing is
    /// shown, and nothing becomes decidable, when the record is not a
    /// pending, unexpired stock consent approval or the pipe cannot take
    /// the notice. The issuer's read holds no lock of the pipe; the final
    /// check and the presentation hold the issuer slot, so a retirement
    /// either came first and nothing is shown, or waits until the
    /// presentation is complete.
    fn present_requested(&self, sink: &NotificationSink, notice: &ConsentNotice) {
        let issuer = self.issuer().clone();
        let Some(issuer) = issuer else {
            return;
        };
        let read = issuer.get(&notice.approval_id);
        drop(issuer);
        let presentation = match read {
            Ok(record) => issued_presentation(&record),
            Err(error) => {
                tracing::warn!(%error, "a requested consent approval could not be read");
                None
            }
        };
        let Some(presentation) = presentation else {
            tracing::warn!("a requested consent approval is not presentable; it is not shown");
            return;
        };
        let held = self.issuer();
        if held.is_none() || !self.open.load(Ordering::SeqCst) {
            return;
        }
        self.presented().insert(notice.approval_id.clone());
        let params = ConsentNoticeParams::from(notice).with_presentation(presentation);
        if !sink.emit_notification(CONSENT_NOTICE_NOTIFICATION, &params) {
            self.presented().remove(&notice.approval_id);
        }
        drop(held);
    }

    /// The pipe closed: its scope retires, inside an owner change on the
    /// composition's publication, so eligibility observed for it is no
    /// longer current and no later decision on it is eligible. A publication
    /// that cannot change is already unavailable to every observation. A
    /// retired pipe stays retired: retiring it again changes nothing.
    pub fn retire(&self) {
        if self.open.load(Ordering::SeqCst) {
            self.stock
                .apply_host_change(|| self.open.store(false, Ordering::SeqCst));
        }
        // A closed pipe presents nothing more: it lets go of the issuer once
        // a presentation it admitted is complete.
        let released = self.issuer().take();
        drop(released);
    }

    /// The guard that retires this pipe's scope when it is dropped, held by
    /// the owner of the pipe's serving future: a future dropped before its
    /// connection reaches the close boundary still retires the scope.
    pub fn retirement(self: &Arc<Self>) -> StdioConsentRetirement {
        StdioConsentRetirement(Arc::clone(self))
    }
}

/// Retires its pipe's consent scope when dropped; see
/// [`StdioConsentSurface::retirement`].
#[must_use = "dropping the guard retires the pipe's consent scope"]
pub struct StdioConsentRetirement(Arc<StdioConsentSurface>);

impl Drop for StdioConsentRetirement {
    fn drop(&mut self) {
        self.0.retire();
    }
}

/// The presentation of `record` as its issuer holds it. `None` unless it
/// is a pending stock consent approval, stating the assurance its rule
/// accepts, whose expiry is still ahead.
fn issued_presentation(record: &ApprovalRecord) -> Option<ConsentPresentationParams> {
    if record.status != ApprovalStatus::Pending {
        return None;
    }
    let expires_at = record
        .expires_at
        .filter(|expires_at| *expires_at > chrono::Utc::now())?;
    let labels = &record.metadata.labels;
    Some(ConsentPresentationParams {
        action: record.proposed_action.clone(),
        risk: record.risk,
        account: labels.get(STOCK_ACTION_ACCOUNT_LABEL).cloned(),
        mandate: labels.get(STOCK_REVIEW_MANDATE_LABEL).cloned(),
        accepted_assurance: labels
            .get(STOCK_CONSENT_ASSURANCE_LABEL)
            .and_then(|name| StockConsentAssurance::from_name(name))?,
        expires_at,
    })
}

impl StockConsentSurface for StdioConsentSurface {
    /// An approval is issued only for an operator this pipe can show it to:
    /// while the pipe is open with its notices attached. The presented
    /// requester is that operator, to whom the approval is presented; the
    /// work's own requester, which for an outcome the stock ingress reports
    /// is the ingress, is stated in the mandate label
    /// ([`STOCK_REVIEW_MANDATE_LABEL`]).
    fn present(
        &self,
        _candidate: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        if self.open_sink().is_none() {
            return Err(ConsentRouteUnavailable);
        }
        Ok(ConsentPresentation {
            requester: self.requester.clone(),
            risk: ApprovalRisk::High,
            metadata: SurfaceMetadata::default(),
        })
    }

    fn approver_login(
        &self,
        approver: &AuthenticatedApprover,
        approval: &ApprovalRecord,
    ) -> Option<StockLoginLifetime> {
        let this_pipe = self.open.load(Ordering::SeqCst)
            && approver.principal() == &self.operator
            && approver.assurance() == ApprovalAssurance::DeviceTrusted
            && approver.authentication_scope() == Some(&self.scope)
            && self.presented().contains(&approval.approval_id);
        this_pipe.then_some(StockLoginLifetime::UntilRetired)
    }

    /// A closed pipe shows nothing more. A `requested` notice carries the
    /// approval's presentation. An approval stays the pipe's to answer for
    /// through its approval: the approval owner asks again, before the
    /// consent is spent, whether its approver was revoked since deciding,
    /// and only the pipe's retirement revokes its operator. A terminal
    /// transition (denied, used or withdrawn) ends it.
    fn notice(&self, notice: ConsentNotice) {
        let Some(sink) = self.open_sink() else {
            return;
        };
        if notice.kind == ConsentNoticeKind::Requested {
            self.present_requested(&sink, &notice);
            return;
        }
        if matches!(
            notice.kind,
            ConsentNoticeKind::Denied | ConsentNoticeKind::Used | ConsentNoticeKind::Withdrawn
        ) {
            self.presented().remove(&notice.approval_id);
        }
        sink.emit_notification(
            CONSENT_NOTICE_NOTIFICATION,
            &ConsentNoticeParams::from(&notice),
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use meerkat::governed_realm::stock::StockPrincipals;
    use meerkat_core::connection::RealmId;
    use meerkat_core::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalProposedAction,
        ApprovalRequest, ApprovalResourceId, ApprovalResourceKind, ApprovalResourceRef,
        InMemoryApprovalStore, ToolCallId,
    };
    use serde_json::json;
    use tokio::sync::mpsc;

    use super::*;
    use crate::protocol::RpcNotification;

    fn stock() -> Arc<StockGovernance> {
        let realm = RealmId::parse("governed").expect("realm");
        let principals = StockPrincipals::for_user(&realm, 501).expect("principals");
        StockGovernance::new(realm, principals)
    }

    /// A pending stock consent approval on `issuer`, as the route issues
    /// it: the rule's accepted assurance, the owners' account and mandate,
    /// and an expiry ahead.
    fn issue(issuer: &ApprovalService) -> ApprovalRecord {
        let mut metadata = SurfaceMetadata::default();
        for (label, value) in [
            (STOCK_CONSENT_ASSURANCE_LABEL, "device_trusted"),
            (
                STOCK_ACTION_ACCOUNT_LABEL,
                "alice at https://issuer.example",
            ),
            (STOCK_REVIEW_MANDATE_LABEL, "the work's mandate"),
        ] {
            metadata.labels.insert(label.to_owned(), value.to_owned());
        }
        let validity =
            chrono::Duration::from_std(std::time::Duration::from_secs(600)).expect("validity");
        issuer
            .request(ApprovalRequest {
                requester: ApprovalPrincipalId::new("human:uid-501").expect("requester"),
                owner: ApprovalOwnerRef::ToolCall {
                    tool_call_id: ToolCallId::new("call-1"),
                },
                resource: ApprovalResourceRef {
                    kind: ApprovalResourceKind::ToolCall,
                    id: ApprovalResourceId::new("mail_send"),
                },
                proposed_action: ApprovalProposedAction {
                    kind: ApprovalActionKind::ToolCall,
                    summary: "mail_send".to_owned(),
                    body: Some(json!({"to": "bob@example.com"})),
                },
                risk: ApprovalRisk::High,
                request_body: None,
                allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                    .into_iter()
                    .collect(),
                expires_at: Some(chrono::Utc::now() + validity),
                metadata,
                request_provenance: None,
            })
            .expect("approval")
    }

    fn notice(approval: &ApprovalRecord, kind: ConsentNoticeKind) -> ConsentNotice {
        ConsentNotice {
            approval_id: approval.approval_id.clone(),
            tool_call_id: "call-1".into(),
            kind,
        }
    }

    /// Attach a notice channel to `pipe` and deliver the `requested` notice
    /// of `approval` on it; returns the channel's receiving end.
    fn present_on(
        pipe: &StdioConsentSurface,
        approval: &ApprovalRecord,
    ) -> mpsc::Receiver<RpcNotification> {
        let (tx, rx) = mpsc::channel(8);
        pipe.attach_notices(&NotificationSink::new(tx));
        pipe.notice(notice(approval, ConsentNoticeKind::Requested));
        rx
    }

    fn notice_params(notification: &RpcNotification) -> ConsentNoticeParams {
        assert_eq!(notification.method, CONSENT_NOTICE_NOTIFICATION);
        serde_json::from_str(notification.params.get()).expect("a typed consent notice")
    }

    /// The pipe's own operator, device trusted in this pipe's scope, is
    /// current until the pipe closes. Another pipe's scope, a claimed person
    /// authentication and another principal are never this pipe's login.
    #[test]
    fn only_this_pipes_operator_is_current_and_only_until_it_closes() {
        let stock = stock();
        let issuer = ApprovalService::new();
        let approval = issue(&issuer);
        let pipe = StdioConsentSurface::new(&stock, issuer.clone()).expect("pipe");
        let other_pipe = StdioConsentSurface::new(&stock, issuer).expect("other pipe");
        let _shown = present_on(&pipe, &approval);
        let _shown_elsewhere = present_on(&other_pipe, &approval);
        let own = pipe.approver().expect("approver");
        assert_eq!(own.assurance(), ApprovalAssurance::DeviceTrusted);
        assert_eq!(
            pipe.approver_login(&own, &approval),
            Some(StockLoginLifetime::UntilRetired)
        );

        let cross_pipe = other_pipe.approver().expect("other approver");
        assert_eq!(pipe.approver_login(&cross_pipe, &approval), None);
        let person = AuthenticatedApprover::new(
            stock.principals().operator.clone(),
            ApprovalAssurance::PersonAuthenticated,
        )
        .expect("person")
        .with_authentication_scope(own.authentication_scope().expect("scope").clone());
        assert_eq!(pipe.approver_login(&person, &approval), None);
        let stranger = AuthenticatedApprover::new(
            PrincipalRef::in_domain(
                meerkat_core::PrincipalKind::Human,
                "uid-502",
                meerkat_core::TrustDomainId::new("rkat-realm-governed").expect("domain"),
            )
            .expect("stranger"),
            ApprovalAssurance::DeviceTrusted,
        )
        .expect("stranger approver")
        .with_authentication_scope(own.authentication_scope().expect("scope").clone());
        assert_eq!(pipe.approver_login(&stranger, &approval), None);

        pipe.retire();
        assert_eq!(pipe.approver_login(&own, &approval), None);
        // The other pipe is still open.
        assert_eq!(
            other_pipe.approver_login(&cross_pipe, &approval),
            Some(StockLoginLifetime::UntilRetired)
        );
    }

    /// Retiring the pipe publishes an owner change on the composition's
    /// publication, so eligibility observed before it stops being current,
    /// and the pipe lets go of the issuer, which retains the candidates whose
    /// consent route holds the pipe.
    #[test]
    fn retiring_the_pipe_moves_the_composition_publication() {
        let stock = stock();
        let pipe = StdioConsentSurface::new(&stock, ApprovalService::new()).expect("pipe");
        let publication = stock.authorization_publication();
        let ((), before) = publication.observe(|| ()).expect("observed");
        assert!(before.check_current().is_ok());
        assert!(pipe.issuer.lock().unwrap().is_some());
        pipe.retire();
        assert!(before.check_current().is_err());
        assert!(
            pipe.issuer.lock().unwrap().is_none(),
            "a closed pipe lets go of the issuer"
        );
    }

    /// The serving future's guard retires the pipe when it is dropped: the
    /// pipe, though still held, lets go of the issuer, so nothing of the
    /// issuer outlives it. Retiring a retired pipe again publishes nothing.
    #[test]
    fn dropping_the_pipes_retirement_guard_retires_it_once() {
        let stock = stock();
        let store = Arc::new(InMemoryApprovalStore::new());
        let issuer_alive = Arc::downgrade(&store);
        let issuer = ApprovalService::with_store(store).expect("issuer");
        let approval = issue(&issuer);
        let pipe = StdioConsentSurface::new(&stock, issuer).expect("pipe");
        let _shown = present_on(&pipe, &approval);
        let own = pipe.approver().expect("approver");
        let publication = stock.authorization_publication();
        let guard = pipe.retirement();
        let ((), before) = publication.observe(|| ()).expect("observed");
        assert_eq!(
            pipe.approver_login(&own, &approval),
            Some(StockLoginLifetime::UntilRetired)
        );
        assert!(
            issuer_alive.upgrade().is_some(),
            "the open pipe holds the issuer"
        );
        drop(guard);
        assert!(
            issuer_alive.upgrade().is_none(),
            "the retired pipe holds nothing of the issuer"
        );
        assert_eq!(pipe.approver_login(&own, &approval), None);
        assert!(before.check_current().is_err());
        let ((), after) = publication.observe(|| ()).expect("observed");
        pipe.retire();
        drop(pipe.retirement());
        assert!(
            after.check_current().is_ok(),
            "a retired pipe stays retired without another owner change"
        );
    }

    /// A `requested` notice carries the issuer's own record of the approval
    /// (the action, the risk, the owners' account and mandate, the accepted
    /// assurance and the expiry). Only that presentation makes the approval
    /// decidable on the pipe, and a terminal transition ends it.
    #[test]
    fn a_requested_notice_presents_the_issuers_record_before_any_decision() {
        let stock = stock();
        let issuer = ApprovalService::new();
        let approval = issue(&issuer);
        let pipe = StdioConsentSurface::new(&stock, issuer).expect("pipe");
        let own = pipe.approver().expect("approver");
        assert_eq!(
            pipe.approver_login(&own, &approval),
            None,
            "an approval the pipe never presented is not its to decide"
        );

        let mut shown = present_on(&pipe, &approval);
        let params = notice_params(&shown.try_recv().expect("the requested notice"));
        assert_eq!(params.kind, meerkat_contracts::ConsentNoticeKind::Requested);
        let presentation = params.presentation.expect("the presentation");
        assert_eq!(presentation.action, approval.proposed_action);
        assert_eq!(presentation.risk, approval.risk);
        assert_eq!(
            presentation.account.as_deref(),
            Some("alice at https://issuer.example")
        );
        assert_eq!(presentation.mandate.as_deref(), Some("the work's mandate"));
        assert_eq!(
            presentation.accepted_assurance,
            StockConsentAssurance::DeviceTrusted
        );
        assert_eq!(Some(presentation.expires_at), approval.expires_at);
        assert_eq!(
            pipe.approver_login(&own, &approval),
            Some(StockLoginLifetime::UntilRetired)
        );

        pipe.notice(notice(&approval, ConsentNoticeKind::Withdrawn));
        let params = notice_params(&shown.try_recv().expect("the withdrawn notice"));
        assert!(params.presentation.is_none());
        assert_eq!(
            pipe.approver_login(&own, &approval),
            None,
            "a terminal transition ends what the pipe answers for"
        );
    }

    /// An approval stays the pipe's to answer for through its approval: the
    /// consent is spent later, and the approval owner may ask again then
    /// whether its operator was revoked. Only a terminal transition (used,
    /// withdrawn or denied) or the pipe's retirement ends it.
    #[test]
    fn an_approved_approval_is_answered_for_until_it_ends_or_the_pipe_retires() {
        for end in ["used", "withdrawn", "denied", "retired"] {
            let stock = stock();
            let issuer = ApprovalService::new();
            let approval = issue(&issuer);
            let pipe = StdioConsentSurface::new(&stock, issuer).expect("pipe");
            let own = pipe.approver().expect("approver");
            let mut shown = present_on(&pipe, &approval);
            let _requested = shown.try_recv().expect("the requested notice");
            if end != "denied" {
                pipe.notice(notice(&approval, ConsentNoticeKind::Approved));
                let params = notice_params(&shown.try_recv().expect("the approved notice"));
                assert_eq!(params.kind, meerkat_contracts::ConsentNoticeKind::Approved);
                assert_eq!(
                    pipe.approver_login(&own, &approval),
                    Some(StockLoginLifetime::UntilRetired),
                    "approved, its operator is still answered for"
                );
            }
            match end {
                "used" => pipe.notice(notice(&approval, ConsentNoticeKind::Used)),
                "withdrawn" => pipe.notice(notice(&approval, ConsentNoticeKind::Withdrawn)),
                "denied" => pipe.notice(notice(&approval, ConsentNoticeKind::Denied)),
                _ => pipe.retire(),
            }
            assert_eq!(pipe.approver_login(&own, &approval), None, "{end}");
        }
    }

    /// A pipe without its notices attached can show nothing, so no approval
    /// is issued for it. A record that is no longer pending is never
    /// presented, and a closed pipe shows nothing more.
    #[test]
    fn a_pipe_presents_only_pending_records_and_only_while_open() {
        let stock = stock();
        let issuer = ApprovalService::new();
        let approval = issue(&issuer);
        let decided = issue(&issuer);
        issuer
            .decide(
                &decided.approval_id,
                ApprovalDecision::Deny,
                ApprovalPrincipalId::new("human:uid-501").expect("actor"),
                None,
                None,
            )
            .expect("decided");
        let pipe = StdioConsentSurface::new(&stock, issuer).expect("pipe");
        let own = pipe.approver().expect("approver");
        assert!(
            pipe.open_sink().is_none(),
            "without its notices the pipe shows nothing"
        );

        let (tx, mut shown) = mpsc::channel(8);
        pipe.attach_notices(&NotificationSink::new(tx));
        assert!(pipe.open_sink().is_some());
        pipe.notice(notice(&decided, ConsentNoticeKind::Requested));
        assert!(
            shown.try_recv().is_err(),
            "a decided record is not presented"
        );
        assert_eq!(pipe.approver_login(&own, &decided), None);

        pipe.retire();
        assert!(pipe.open_sink().is_none(), "a closed pipe shows nothing");
        pipe.notice(notice(&approval, ConsentNoticeKind::Requested));
        assert!(shown.try_recv().is_err());
        assert_eq!(pipe.approver_login(&own, &approval), None);
    }
}
