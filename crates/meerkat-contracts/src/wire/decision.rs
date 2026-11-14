//! Durable decision wire contracts: the `decision/*` JSON-RPC methods.
//!
//! A durable decision holds one operation until an attested party decides
//! it, its requester or the host withdraws it, or its deadline passes. Its
//! lifecycle is owned by the runtime's `DurableDecisionMachine`; these types
//! are the projection of its stored row. Tagged enums use `kind`. Requests
//! are closed: a kind or a field this version does not know is refused,
//! never ignored, so a request never names an approver, an actor, an
//! assurance or an authentication scope. Every vocabulary a result carries
//! has a forward-compatible `unknown` variant that the server never sends,
//! except the refusals ([`WireDecisionRefusal`]), which are closed: they bear
//! authority.

use serde::{Deserialize, Serialize};

/// Where a decision stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionPhase {
    /// Held, and not yet decided, withdrawn or expired.
    Open,
    /// Approved; the held request not yet executed.
    Approved,
    /// Rejected. Terminal.
    Rejected,
    /// Observed at or after its deadline undecided, or approved and never
    /// executed. Terminal.
    Expired,
    /// Withdrawn by its requester or the host before a decision. Terminal.
    Withdrawn,
    /// The held request was executed. Terminal.
    Executed,
    /// The operation's owner refused the held request. Terminal.
    ExecutionRefused,
    /// Forward compatibility: a phase this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// Who decides an open decision now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionStage {
    /// An authenticated human acting for the approver id.
    Human,
    /// The named reviewer.
    Review,
    /// Forward compatibility: a stage this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// Where the requester notice of a decision stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionNoticeState {
    /// The requester is not routable: no notice is ever owed.
    NotRequired,
    /// The decision has not ended.
    NotDue,
    /// The decision ended and its notice is owed.
    Owed,
    /// The notice was delivered.
    Delivered,
    /// The notice failed terminally.
    Failed,
    /// Forward compatibility: a notice state this version does not know.
    /// The server never sends it.
    #[serde(other)]
    Unknown,
}

/// Who decided a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionTier {
    /// An authenticated human acting for the approver id.
    Human,
    /// The named reviewer, through its verdict.
    Review,
    /// Forward compatibility: a tier this version does not know. The server
    /// never sends it.
    #[serde(other)]
    Unknown,
}

/// How the human who decided was authenticated on the decision event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionAssurance {
    /// The person was authenticated on the actual event.
    PersonAuthenticated,
    /// A delegated service credential authenticated the event.
    ServiceCredential,
    /// Only device or network trust was established.
    DeviceTrusted,
    /// Forward compatibility: an assurance this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// A reviewer's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionReviewVerdict {
    /// Approve the held operation.
    Allow,
    /// Reject it.
    Deny,
    /// Hand the decision to the human approver.
    Escalate,
    /// Forward compatibility: a verdict this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// One canonical part of a rendering: consecutive byte ranges on character
/// boundaries of at most the presenter's part size, each with its own
/// digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
// Closed for the request that carries it (`DecisionPresentationRequest`).
// Results carry it too (`WirePresentationRecord`, `DecisionRenderingResult`),
// and so does the `decision/notice` notification
// (`WireDecisionNoticePresentation`). No client parses a result or a notice
// with this type: the server only serializes them, the Python and TypeScript
// SDKs return a result as the server wrote it, and neither parses a notice.
// A client that deserializes results or notices with it needs a form that
// tolerates added fields first.
#[serde(deny_unknown_fields)]
pub struct WireCanonicalPart {
    /// The part's position, from 0.
    pub index: u32,
    /// How many parts the split has.
    pub count: u32,
    /// The digest of the part's bytes (`sha256:` and 64 lowercase hex
    /// digits).
    pub part_digest: String,
}

/// Who a recorded presentation was shown to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WirePresentationAudience {
    /// Whoever acts for the decision's approver id.
    Approver { approver_id: String },
    /// The decision's named reviewer (a principal key).
    Reviewer { reviewer: String },
    /// Forward compatibility: an audience this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// One recorded presentation of a decision's rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WirePresentationRecord {
    pub audience: WirePresentationAudience,
    /// The digest of the rendering that was presented.
    pub subject_digest: String,
    /// The presenter's part size; absent when the rendering was presented
    /// as one part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_part_bytes: Option<u32>,
    /// The parts the audience was shown.
    pub parts_shown: Vec<WireCanonicalPart>,
    /// For a summary presentation, the link it carried to the full
    /// presentation and the digest of the rendering that link served;
    /// absent for a presentation of the parts themselves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<WirePresentationLink>,
    /// Where the presentation was shown.
    pub channel_ref: String,
    pub message_ref: String,
    /// The host's clock reading of the presentation, in UNIX ms.
    pub at_ms: u64,
}

/// The link a summary presentation carried to the full presentation of a
/// decision's rendering, and the digest of the rendering that link served
/// when it was followed. A surface with a size limit shows a summary that
/// says it is shortened, with such a link; the summary counts as a complete
/// presentation only when the link served the held rendering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
// Closed for the request that carries it (`DecisionPresentationRequest`).
// Results carry it too (`WirePresentationRecord`), which no client parses
// with this type: the server only serializes them.
#[serde(deny_unknown_fields)]
pub struct WirePresentationLink {
    /// Where the link points: the page or document that serves the full
    /// presentation.
    pub reference: String,
    /// The digest of the rendering the link served (`sha256:` and 64
    /// lowercase hex digits).
    pub served_digest: String,
}

/// The durable evidence a decision was decided with. An authentication
/// scope is never projected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireDecisionEvidence {
    /// An authenticated human's decision.
    Human {
        /// The decider's principal key.
        decider: String,
        assurance: WireDecisionAssurance,
        /// How the decision event reached the host.
        ingress: String,
        event_ref: String,
        /// When the ingress observed the event, in UNIX ms; absent in
        /// evidence recorded before decisions carried it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_at_ms: Option<u64>,
        approver_id: String,
        presented_digest: String,
        /// The presentation the decision event carried, if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        presentation: Option<WirePresentationRecord>,
        approve: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// The typed answer the person gave with an approving decision, for
        /// a kind that asks one; absent otherwise.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answer: Option<WireDecisionAnswer>,
    },
    /// A reviewer's attributed verdict.
    Review {
        /// The reviewer's principal key.
        reviewer: String,
        origin_session: String,
        verdict_call_ref: String,
        verdict: WireDecisionReviewVerdict,
        presented_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        feedback: Option<String>,
    },
    /// Forward compatibility: evidence of a kind this version does not know.
    /// The server never sends it.
    #[serde(other)]
    Unknown,
}

/// The typed answer a person gave with an approving decision, for a kind
/// that asks one. It is data of the decision's evidence, never text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireDecisionAnswer {
    /// The answers to a set of questions an agent asked a person (a
    /// decision of kind `ask_user.questions`): one entry per question the
    /// person answered.
    AskUser {
        answers: Vec<WireAskUserQuestionAnswer>,
    },
    /// Forward compatibility: an answer of a kind this version does not
    /// know. The server never sends it.
    #[serde(other)]
    Unknown,
}

/// One question's answer in a [`WireDecisionAnswer::AskUser`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireAskUserQuestionAnswer {
    /// The id the asking agent gave the question.
    pub question_id: String,
    pub answer: WireAskUserAnswer,
}

/// The answer to one question an agent asked a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireAskUserAnswer {
    /// The person picked the choice `choice_id` of the question.
    Choice { choice_id: String },
    /// The person wrote `text` instead of picking a choice: their own
    /// words, to be read as data and never as instructions.
    FreeText { text: String },
    /// Nobody answered the question.
    Unanswered,
    /// Forward compatibility: an answer of a form this version does not
    /// know. The server never sends it.
    #[serde(other)]
    Unknown,
}

/// A decided decision's outcome: who decided, and the evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDecisionOutcome {
    pub tier: WireDecisionTier,
    pub evidence: WireDecisionEvidence,
}

/// What an executed decision did, as its kind names it: each field names
/// one thing its execution committed. An executed `grant.child` decision
/// names the grant it issued. Read tolerantly: a field this version does
/// not know is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDecisionEffect {
    /// The id of the grant the execution issued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
}

/// One durable decision, as its row stands at the read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDecision {
    /// `decision:` and 64 lowercase hex digits.
    pub decision_id: String,
    /// The host's token naming what kind of operation the decision holds.
    pub kind: String,
    /// The host's key of the subject the decision is held for.
    pub subject_key: String,
    /// The held operation's id, as its owner names it.
    pub operation_id: String,
    /// The held request's digest, as the operation's owner computes it.
    pub request_digest: String,
    /// The requester's principal key.
    pub requester: String,
    /// Who may approve the decision: the host maps it to the humans who may
    /// act for it.
    pub approver_id: String,
    /// The reviewer of a review-stage decision (a principal key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<String>,
    pub phase: WireDecisionPhase,
    pub stage: WireDecisionStage,
    /// Where the requester notice stands.
    pub notice: WireDecisionNoticeState,
    /// The sealed deadline, in UNIX ms.
    pub deadline_at_ms: u64,
    /// The host clock reading of the hold, in UNIX ms.
    pub created_at_ms: u64,
    /// The digest of the decision's rendering, computed by the server.
    pub subject_digest: String,
    /// The token of the renderer that produced the rendering.
    pub renderer: String,
    /// When a complete presentation first reached the approver, in UNIX ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_presented_to_approver_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<u64>,
    /// Whether an execution permit is outstanding.
    pub permit_issued: bool,
    /// The latest complete presentation per audience.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presentations: Vec<WirePresentationRecord>,
    /// Set exactly when the decision was decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<WireDecisionOutcome>,
    /// The owner's typed refusal of an execution-refused decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_refusal: Option<String>,
    /// What the executed decision did, as its kind names it; absent for a
    /// decision that is not executed and for a kind that names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<WireDecisionEffect>,
    /// The row's revision: 1 at the hold, one more at each write.
    pub revision: u64,
    /// The held request, when the read asked for it (`include_request`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<serde_json::Value>,
}

/// A stored decision row the server cannot read (written by a newer
/// version, or torn). Nothing executes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireUnreadableDecision {
    pub decision_id: String,
    pub detail: String,
}

/// Why a decision refused a call. Closed: a refusal bears authority, so a
/// client never reads one it does not know as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireDecisionRefusal {
    /// The hold's deadline is not after the hold's clock reading.
    InvalidWindow,
    /// The hold names the requester as its own reviewer.
    SelfReview,
    /// The decision is held with another binding.
    HoldConflict,
    /// The decision is no longer open: the first valid decision won.
    NotOpen,
    /// The deadline passed; the call recorded the expiry.
    Expired,
    /// The presented digest is not the held subject digest.
    SubjectChanged,
    /// The presentation did not cover the whole held subject, none reached
    /// the deciding party, or it was shown at 0 (no reading of when).
    PresentationIncomplete,
    /// The presentation was shown to a party that does not decide now.
    WrongAudience,
    /// A human decision while the review stage is open.
    ReviewStage,
    /// A review verdict while the human stage is open.
    HumanStage,
    /// The decision carries no attested evidence.
    EvidenceRequired,
    /// The decider does not act for the approver id, or is not currently
    /// eligible.
    NotEligible,
    /// The requester tried to decide its own request.
    SelfApproval,
    /// The verdict is not the named reviewer's.
    WrongReviewer,
    /// Only the requester or the host may withdraw.
    NotRequester,
    /// The decision is not approved.
    NotApproved,
    /// The execution names another operation or request.
    BindingMismatch,
    /// No execution permit is outstanding.
    NotAuthorized,
    /// The deadline passed while a permit is outstanding: the permit is
    /// settled first.
    SettlementOwed,
    /// No notice is owed, or the receipt contradicts the settled one.
    NotOwed,
    /// The human decision was observed before the decision's first
    /// presentation to the approver.
    DecidedBeforePresentation,
}

/// The typed `data` of a `decision/*` error. Closed, like the refusals it
/// carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireDecisionErrorData {
    /// The decision refused the call (code -32602); nothing changed, except
    /// an expiry the call recorded.
    DecisionRefused {
        decision_refusal: WireDecisionRefusal,
    },
    /// No decision has the id (code -32602).
    DecisionNotFound,
    /// The decision's row cannot be read (code -32603). Nothing executes it.
    DecisionUnreadable,
    /// The runtime serves a candidate launch, which writes no decision
    /// (code -32020).
    CandidateLaunch,
    /// The runtime keeps no durable decisions: an ephemeral persistence
    /// (code -32020).
    NoDecisionStore,
}

/// `decision/list` parameters: every field set must match. The page holds
/// at most `limit` decisions (100 when absent, at most 500), in decision id
/// order after `after`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<WireDecisionPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approver_id: Option<String>,
    /// The requester's principal key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<String>,
    /// The decision id to read the page after.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// `decision/list` result. Each decision is read at the server's clock: one
/// past its deadline reads as expired.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionListResult {
    pub decisions: Vec<WireDecision>,
    /// The rows of the page the server cannot read.
    #[serde(default)]
    pub unreadable: Vec<WireUnreadableDecision>,
    /// The id to read the next page after, when there may be more rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
}

/// `decision/get` parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionGetParams {
    pub decision_id: String,
    /// Whether the result carries the held request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub include_request: bool,
}

/// The result of `decision/get`, `decision/present`, `decision/withdraw`
/// and `decision/decide`: the decision as it stands after the call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionResult {
    pub decision: WireDecision,
}

/// `decision/rendering` parameters. `max_part_bytes` splits the rendering
/// into canonical parts of at most that many bytes; absent, it is one part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionRenderingParams {
    pub decision_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_part_bytes: Option<u32>,
}

/// `decision/rendering` result: the one canonical rendering of the held
/// request and its canonical parts for the asked part size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct DecisionRenderingResult {
    pub decision_id: String,
    pub rendering: String,
    pub renderer: String,
    pub subject_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_part_bytes: Option<u32>,
    pub parts: Vec<WireCanonicalPart>,
}

/// Who a presenter showed a rendering to. Closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DecisionAudienceRequest {
    /// Whoever acts for the decision's approver id.
    Approver { approver_id: String },
    /// The decision's named reviewer (a principal key).
    Reviewer { reviewer: String },
}

/// What a host presenter showed. Closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionPresentationRequest {
    pub audience: DecisionAudienceRequest,
    /// The digest of the rendering that was presented.
    pub subject_digest: String,
    /// The presenter's part size; absent when it presented one part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_part_bytes: Option<u32>,
    pub parts_shown: Vec<WireCanonicalPart>,
    /// For a summary a surface with a size limit showed, the link it
    /// carried to the full presentation and the digest of the rendering
    /// that link served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<WirePresentationLink>,
    /// Where the rendering was shown.
    pub channel_ref: String,
    pub message_ref: String,
    /// The host's clock reading of the presentation, in UNIX ms.
    pub at_ms: u64,
}

/// `decision/present` parameters: record what a host presenter showed. It
/// counts only when it shows every canonical part of the held rendering, or
/// is a summary whose link served the held rendering, to the party that
/// decides now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionPresentParams {
    pub decision_id: String,
    pub presentation: DecisionPresentationRequest,
}

/// `decision/withdraw` parameters: the connection withdraws as the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionWithdrawParams {
    pub decision_id: String,
}

/// `decision/decide` parameters. They name only the decision, the answer,
/// the digest of the rendering the human saw and a reason; who decided
/// comes from the authenticated event, never from the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DecisionDecideParams {
    pub decision_id: String,
    pub approve: bool,
    pub presented_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// The notification an `rkat-rpc` stdio connection sends its operator about
/// one durable decision.
pub const DECISION_NOTICE_NOTIFICATION: &str = "decision/notice";

/// `decision/notice` parameters: what an `rkat-rpc` stdio connection showed
/// its operator about one durable decision. The connection presents each
/// open decision addressed to an approver id its operator acts for that it
/// has not presented yet: when the decision opens while the connection is
/// open, and when its client lists (`decision/list`) a page that holds it.
/// It writes a `presented` notice carrying the whole rendering, then records
/// that presentation (to the approver, with this notice's id as its message
/// reference); a listed page's notices precede the list's answer. A notice
/// grants nothing: the operator decides with `decision/decide` on the same
/// connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDecisionNotice {
    /// The notice's id: the message reference of the presentation it
    /// records.
    pub notice_id: String,
    pub decision_id: String,
    /// The decision's kind token.
    pub kind: String,
    pub event: WireDecisionNoticeEvent,
    /// The decision's phase when the notice was written.
    pub phase: WireDecisionPhase,
    /// The approver id the decision is held for.
    pub approver_id: String,
    /// The requester's principal key.
    pub requester: String,
    /// What the operator was shown; set on a `presented` notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<WireDecisionNoticePresentation>,
}

/// What a decision notice reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireDecisionNoticeEvent {
    /// The connection presented the decision to its operator.
    Presented,
    /// Forward compatibility: an event this version does not know. The
    /// server never sends it.
    #[serde(other)]
    Unknown,
}

/// The rendering a `presented` notice showed: the decision's whole
/// rendering, its renderer and digest, and its canonical parts (one part:
/// the whole rendering).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireDecisionNoticePresentation {
    pub rendering: String,
    pub renderer: String,
    pub subject_digest: String,
    pub parts: Vec<WireCanonicalPart>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_result_vocabulary_this_version_does_not_know_reads_as_unknown() {
        let phase: WireDecisionPhase =
            serde_json::from_value(serde_json::json!("archived")).expect("an unknown phase reads");
        assert_eq!(phase, WireDecisionPhase::Unknown);
        let notice: WireDecisionNoticeState =
            serde_json::from_value(serde_json::json!("queued")).expect("unknown notice state");
        assert_eq!(notice, WireDecisionNoticeState::Unknown);
        let evidence: WireDecisionEvidence =
            serde_json::from_value(serde_json::json!({"kind": "imported", "source": "x"}))
                .expect("unknown evidence");
        assert_eq!(evidence, WireDecisionEvidence::Unknown);
    }

    /// A human decision's evidence carries the typed answer the person gave
    /// through a read and a write; evidence without one omits the field;
    /// and an answer kind, or an answer form, this version does not know
    /// reads as unknown instead of failing the decision's read.
    #[test]
    fn a_decision_answer_is_optional_typed_and_read_tolerantly() {
        let without = serde_json::json!({
            "kind": "human",
            "decider": "principal:0",
            "assurance": "person_authenticated",
            "ingress": "console",
            "event_ref": "event-1",
            "approver_id": "ask_user",
            "presented_digest": "sha256:1",
            "approve": true,
        });
        let decoded: WireDecisionEvidence =
            serde_json::from_value(without.clone()).expect("evidence without an answer reads");
        assert!(matches!(
            &decoded,
            WireDecisionEvidence::Human { answer: None, .. }
        ));
        assert_eq!(
            serde_json::to_value(&decoded).expect("encodes"),
            without,
            "and writes no answer field"
        );

        let mut with = without.clone();
        with["answer"] = serde_json::json!({
            "kind": "ask_user",
            "answers": [
                {"question_id": "channel", "answer": {"kind": "choice", "choice_id": "beta"}},
                {"question_id": "notes", "answer": {"kind": "free_text", "text": "Nightly first"}},
                {"question_id": "notify", "answer": {"kind": "unanswered"}},
            ],
        });
        let decoded: WireDecisionEvidence =
            serde_json::from_value(with.clone()).expect("evidence with an answer reads");
        let expected = WireDecisionAnswer::AskUser {
            answers: vec![
                WireAskUserQuestionAnswer {
                    question_id: "channel".into(),
                    answer: WireAskUserAnswer::Choice {
                        choice_id: "beta".into(),
                    },
                },
                WireAskUserQuestionAnswer {
                    question_id: "notes".into(),
                    answer: WireAskUserAnswer::FreeText {
                        text: "Nightly first".into(),
                    },
                },
                WireAskUserQuestionAnswer {
                    question_id: "notify".into(),
                    answer: WireAskUserAnswer::Unanswered,
                },
            ],
        };
        assert!(
            matches!(
                &decoded,
                WireDecisionEvidence::Human { answer: Some(answer), .. } if *answer == expected
            ),
            "{decoded:?}"
        );
        assert_eq!(serde_json::to_value(&decoded).expect("encodes"), with);

        let mut unknown_kind = without.clone();
        unknown_kind["answer"] = serde_json::json!({"kind": "survey", "score": 4});
        let decoded: WireDecisionEvidence =
            serde_json::from_value(unknown_kind).expect("an unknown answer kind reads");
        assert!(matches!(
            decoded,
            WireDecisionEvidence::Human {
                answer: Some(WireDecisionAnswer::Unknown),
                ..
            }
        ));

        let mut unknown_form = without;
        unknown_form["answer"] = serde_json::json!({
            "kind": "ask_user",
            "answers": [{"question_id": "channel", "answer": {"kind": "ranking", "order": []}}],
        });
        let decoded: WireDecisionEvidence =
            serde_json::from_value(unknown_form).expect("an unknown answer form reads");
        assert!(matches!(
            &decoded,
            WireDecisionEvidence::Human {
                answer: Some(WireDecisionAnswer::AskUser { answers }),
                ..
            } if answers[0].answer == WireAskUserAnswer::Unknown
        ));
    }

    #[test]
    fn refusals_are_closed() {
        let known: WireDecisionRefusal =
            serde_json::from_value(serde_json::json!({"kind": "subject_changed"}))
                .expect("a known refusal");
        assert_eq!(known, WireDecisionRefusal::SubjectChanged);
        assert!(
            serde_json::from_value::<WireDecisionRefusal>(
                serde_json::json!({"kind": "approver_revoked"})
            )
            .is_err()
        );
        let data = serde_json::to_value(WireDecisionErrorData::DecisionRefused {
            decision_refusal: WireDecisionRefusal::PresentationIncomplete,
        })
        .expect("error data");
        assert_eq!(
            data,
            serde_json::json!({
                "kind": "decision_refused",
                "decision_refusal": {"kind": "presentation_incomplete"},
            })
        );
    }

    #[test]
    fn a_decide_request_never_names_who_decided() {
        for field in ["approver", "actor", "assurance", "scope", "approver_id"] {
            let mut body = serde_json::json!({
                "decision_id": "decision:0",
                "approve": true,
                "presented_digest": "sha256:0",
            });
            body[field] = serde_json::json!("someone");
            assert!(
                serde_json::from_value::<DecisionDecideParams>(body).is_err(),
                "a decide body naming {field} is refused"
            );
        }
    }

    /// A presenter's request may carry a summary's link, closed like the
    /// request; a recorded presentation without a link omits it.
    #[test]
    fn a_presentation_link_is_optional_and_closed() {
        let request = serde_json::json!({
            "audience": {"kind": "approver", "approver_id": "team:owners"},
            "subject_digest": "sha256:1",
            "parts_shown": [],
            "link": {"reference": "decision-page", "served_digest": "sha256:1"},
            "channel_ref": "chat:owners",
            "message_ref": "message-1",
            "at_ms": 2_000,
        });
        let decoded: DecisionPresentationRequest =
            serde_json::from_value(request.clone()).expect("a request with a link reads");
        assert_eq!(
            decoded.link,
            Some(WirePresentationLink {
                reference: "decision-page".into(),
                served_digest: "sha256:1".into(),
            })
        );
        let mut unknown = request;
        unknown["link"]["note"] = serde_json::json!("shortened");
        assert!(
            serde_json::from_value::<DecisionPresentationRequest>(unknown).is_err(),
            "a link field this version does not know is refused"
        );

        let record = WirePresentationRecord {
            audience: WirePresentationAudience::Approver {
                approver_id: "team:owners".into(),
            },
            subject_digest: "sha256:1".into(),
            max_part_bytes: None,
            parts_shown: Vec::new(),
            link: None,
            channel_ref: "chat:owners".into(),
            message_ref: "message-1".into(),
            at_ms: 2_000,
        };
        let encoded = serde_json::to_value(&record).expect("encodes");
        assert!(encoded.get("link").is_none(), "{encoded}");
    }

    /// A decision's effect is optional and read tolerantly: a decision
    /// without one omits it, an executed decision's effect keeps the grant
    /// id it names through a read and a write, a field of the effect this
    /// version does not know is ignored, and an effect that names nothing
    /// this version knows still reads.
    #[test]
    fn a_decision_effect_is_optional_and_read_tolerantly() {
        let decision = serde_json::json!({
            "decision_id": "decision:0",
            "kind": "grant.child",
            "subject_key": "subject:0",
            "operation_id": "grant.child:decision:0",
            "request_digest": "sha256:0",
            "requester": "principal:0",
            "approver_id": "data-owner",
            "phase": "executed",
            "stage": "human",
            "notice": "owed",
            "deadline_at_ms": 2,
            "created_at_ms": 1,
            "subject_digest": "sha256:1",
            "renderer": "grant.render-v1",
            "permit_issued": false,
            "revision": 5,
        });
        let without: WireDecision =
            serde_json::from_value(decision.clone()).expect("a decision without an effect reads");
        let written = serde_json::to_value(&without).expect("encodes");
        assert!(written.get("effect").is_none(), "{written}");

        let mut with_effect = decision.clone();
        with_effect["effect"] = serde_json::json!({
            "grant_id": "decision:0/approval-grant",
            "issued_at_ms": 3,
        });
        let read: WireDecision =
            serde_json::from_value(with_effect).expect("a decision with an effect reads");
        let written = serde_json::to_value(&read).expect("encodes");
        assert_eq!(
            written["effect"],
            serde_json::json!({"grant_id": "decision:0/approval-grant"}),
            "{written}"
        );

        let mut later = decision;
        later["effect"] = serde_json::json!({"answer_id": "answer:0"});
        let read: WireDecision =
            serde_json::from_value(later).expect("an effect of a later version reads");
        let written = serde_json::to_value(&read).expect("encodes");
        assert_eq!(written["effect"], serde_json::json!({}), "{written}");
    }

    /// A presented notice carries the whole rendering as one canonical
    /// part; an event this version does not know reads as unknown, and a
    /// notice without a presentation omits it.
    #[test]
    fn a_decision_notice_has_its_documented_shape() {
        let notice = WireDecisionNotice {
            notice_id: "notice-1".into(),
            decision_id: "decision:0".into(),
            kind: "release".into(),
            event: WireDecisionNoticeEvent::Presented,
            phase: WireDecisionPhase::Open,
            approver_id: "operator".into(),
            requester: "principal:0".into(),
            presentation: Some(WireDecisionNoticePresentation {
                rendering: "Release build 7.".into(),
                renderer: "render.v1".into(),
                subject_digest: "sha256:1".into(),
                parts: vec![WireCanonicalPart {
                    index: 0,
                    count: 1,
                    part_digest: "sha256:1".into(),
                }],
            }),
        };
        let encoded = serde_json::to_value(&notice).expect("encodes");
        assert_eq!(
            encoded,
            serde_json::json!({
                "notice_id": "notice-1",
                "decision_id": "decision:0",
                "kind": "release",
                "event": "presented",
                "phase": "open",
                "approver_id": "operator",
                "requester": "principal:0",
                "presentation": {
                    "rendering": "Release build 7.",
                    "renderer": "render.v1",
                    "subject_digest": "sha256:1",
                    "parts": [{"index": 0, "count": 1, "part_digest": "sha256:1"}],
                },
            })
        );
        let decoded: WireDecisionNotice = serde_json::from_value(encoded).expect("decodes");
        assert_eq!(decoded, notice);

        let mut later = serde_json::to_value(WireDecisionNotice {
            presentation: None,
            ..notice
        })
        .expect("encodes");
        assert!(later.get("presentation").is_none(), "{later}");
        later["event"] = serde_json::json!("closed");
        let decoded: WireDecisionNotice = serde_json::from_value(later).expect("decodes");
        assert_eq!(decoded.event, WireDecisionNoticeEvent::Unknown);
    }
}
