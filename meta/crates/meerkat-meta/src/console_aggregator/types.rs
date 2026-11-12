use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConsoleCursor(String);

impl ConsoleCursor {
    pub(crate) fn from_seq(seq: u64) -> Self {
        Self(format!("console:{seq}"))
    }

    pub(crate) fn seq(&self) -> Option<u64> {
        self.0.strip_prefix("console:")?.parse::<u64>().ok()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConsoleCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ConsoleCursor {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ConsoleCursor {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleFrameStatus {
    Accepted,
    Dispatching,
    Delivered,
    DeliveryFailed,
    Completed,
    Redacted,
    /// A governed send's bind answered bound and its input is being
    /// submitted: from here it is delivered or fails, and is never held.
    /// Declared last so the earlier variants keep their discriminants.
    Submitted,
}

impl ConsoleFrameStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Dispatching => "dispatching",
            Self::Submitted => "submitted",
            Self::Delivered => "delivered",
            Self::DeliveryFailed => "delivery_failed",
            Self::Completed => "completed",
            Self::Redacted => "redacted",
        }
    }

    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "accepted" => Self::Accepted,
            "dispatching" => Self::Dispatching,
            "submitted" => Self::Submitted,
            "delivered" => Self::Delivered,
            "delivery_failed" => Self::DeliveryFailed,
            "redacted" => Self::Redacted,
            _ => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleFrameSourceKind {
    ConsoleEvent,
    SessionHistory,
    ToolApplication,
    Send,
    Synthetic,
}

impl ConsoleFrameSourceKind {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::ConsoleEvent => "console_event",
            Self::SessionHistory => "session_history",
            Self::ToolApplication => "tool_application",
            Self::Send => "send",
            Self::Synthetic => "synthetic",
        }
    }

    pub(crate) fn from_str(value: &str) -> Self {
        match value {
            "console_event" => Self::ConsoleEvent,
            "tool_application" => Self::ToolApplication,
            "session_history" => Self::SessionHistory,
            "send" => Self::Send,
            _ => Self::Synthetic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleFrameSource {
    /// Owner-captured member context for applying policy after a member retires.
    /// Absent on frames written before provenance support. Storage serialization
    /// preserves this witness; policy-checked transport projections remove it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_provenance: Option<ConsoleFrameMemberProvenance>,
    pub kind: ConsoleFrameSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_cursor: Option<String>,
}

/// Member context captured when the canonical console owner admits a frame.
/// Policy decisions remain per-view; this stores their inputs, not a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleFrameMemberProvenance {
    pub identity: ConsoleIdentityRecord,
    pub member: crate::runtime::ConsoleMember,
    pub primary_mob_id: String,
    pub source_mob_id: String,
    /// The member's spawn lineage as the owner captured it: the member
    /// first, then its spawn ancestors, parent first. Each ancestor is
    /// captured only from the incarnation the member's lineage names; one
    /// that could not be confirmed is recorded without attributes. Empty on
    /// frames written before lineage was captured: those are decided with
    /// any ancestor unconfirmed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lineage: Vec<ConsoleFrameLineageLink>,
}

/// One captured link of a member's spawn lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleFrameLineageLink {
    /// Console identity, in the access rules' namespace.
    pub identity: String,
    /// Runtime member id, in the access rules' namespace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Labels with registry-verified lineage. `None` for an ancestor whose
    /// incarnation was not confirmed when the frame was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleFrame {
    pub id: String,
    pub cursor: ConsoleCursor,
    pub dedupe_key: String,
    pub timestamp_ms: u64,
    pub runtime_key: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: String,
    pub status: ConsoleFrameStatus,
    pub frame_version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at_ms: Option<u64>,
    pub payload: Value,
    pub source: ConsoleFrameSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_frame_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by_frame_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewConsoleFrame {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub dedupe_key: String,
    pub timestamp_ms: u64,
    pub runtime_key: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub kind: String,
    pub status: ConsoleFrameStatus,
    pub payload: Value,
    pub source: ConsoleFrameSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_frame_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caused_by_frame_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppendDisposition {
    Inserted,
    Existing,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppendOutcome {
    pub disposition: AppendDisposition,
    pub frame: ConsoleFrame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleTimelineQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConsoleCursor>,
    /// Same wire/programmatic default alignment as
    /// [`ConsoleTimelineWindowQuery::limit`]: a bare `#[serde(default)]`
    /// deserialized 0 for omitted limits while [`Default`] said 200.
    #[serde(default = "default_timeline_window_limit")]
    pub limit: usize,
}

impl Default for ConsoleTimelineQuery {
    fn default() -> Self {
        Self {
            identity: None,
            conversation_id: None,
            after: None,
            limit: 200,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleTimelineWindowQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ConsoleCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ConsoleCursor>,
    #[serde(default)]
    pub mode: ConsoleTimelineMode,
    /// Maximum frames returned. Serde default matches [`Default`] (200):
    /// `#[serde(default)]` alone yielded 0, which the visibility scan clamps
    /// to 1 — so every wire caller that omitted `limit` got exactly ONE frame
    /// while programmatic construction got 200. Only the reference front-end
    /// (which always sends limit=400) never noticed.
    #[serde(default = "default_timeline_window_limit")]
    pub limit: usize,
}

fn default_timeline_window_limit() -> usize {
    200
}

impl Default for ConsoleTimelineWindowQuery {
    fn default() -> Self {
        Self {
            identity: None,
            conversation_id: None,
            after: None,
            before: None,
            mode: ConsoleTimelineMode::Since,
            limit: 200,
        }
    }
}

impl From<ConsoleTimelineQuery> for ConsoleTimelineWindowQuery {
    fn from(query: ConsoleTimelineQuery) -> Self {
        Self {
            identity: query.identity,
            conversation_id: query.conversation_id,
            after: query.after,
            before: None,
            mode: ConsoleTimelineMode::Since,
            limit: query.limit,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleTimelineMode {
    #[default]
    Since,
    Recent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleTimelinePage {
    pub frames: Vec<ConsoleFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<ConsoleCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleTimelineWindowPage {
    pub frames: Vec<ConsoleFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<ConsoleCursor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_cursor: Option<ConsoleCursor>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub exhausted: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A kind of streamed block: the console log stores one row per provider
/// chunk, then the whole block once, in a complete row that supersedes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ConsoleDeltaKind {
    /// `text_delta` rows, superseded by the message's `text_complete` row.
    Text,
    /// `reasoning_delta` rows, superseded by the block's `reasoning_complete`
    /// row. A message can stream several reasoning blocks, one after another.
    Reasoning,
}

impl ConsoleDeltaKind {
    /// The frame kind of the block's streamed rows.
    #[must_use]
    pub const fn delta_frame_kind(self) -> &'static str {
        match self {
            Self::Text => "text_delta",
            Self::Reasoning => "reasoning_delta",
        }
    }

    /// The frame kind of the row that supersedes them.
    #[must_use]
    pub const fn complete_frame_kind(self) -> &'static str {
        match self {
            Self::Text => "text_complete",
            Self::Reasoning => "reasoning_complete",
        }
    }

    pub(crate) fn of_delta_frame_kind(kind: &str) -> Option<Self> {
        [Self::Text, Self::Reasoning]
            .into_iter()
            .find(|candidate| candidate.delta_frame_kind() == kind)
    }

    pub(crate) fn of_complete_frame_kind(kind: &str) -> Option<Self> {
        [Self::Text, Self::Reasoning]
            .into_iter()
            .find(|candidate| candidate.complete_frame_kind() == kind)
    }
}

/// The streamed block a console-event row's payload names: its assistant
/// message id and the member stream's sequence. Only rows that carry both take
/// part in delta compaction, in the projection that tracks blocks and in the
/// stores that remove their rows alike.
pub(crate) fn streamed_block_payload(payload: &Value) -> Option<(&str, u64)> {
    let assistant_message_id = payload.get("assistant_message_id")?.as_str()?;
    let source_sequence = payload.get("source_sequence")?.as_u64()?;
    Some((assistant_message_id, source_sequence))
}

/// One streamed block's delta rows, which the block's durable complete row
/// supersedes: what
/// [`ConsoleLogStore::compact_message_deltas`](crate::console_aggregator::ConsoleLogStore::compact_message_deltas)
/// removes. The first delta row stays and holds the block's place in the
/// timeline; the rows after it, up to the complete row, go.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConsoleDeltaCompaction {
    /// The runtime the rows were projected for.
    pub runtime_key: String,
    /// The identity the rows are stored under.
    pub identity: String,
    /// Which streamed rows, and which complete row supersedes them.
    pub kind: ConsoleDeltaKind,
    /// The assistant message the rows' payloads name.
    pub assistant_message_id: String,
    /// The block's first delta row the projection observed; it stays.
    pub first_delta: ConsoleCursor,
    /// The block's complete row, durable before the removal.
    pub complete: ConsoleCursor,
}

impl ConsoleDeltaCompaction {
    pub(crate) fn new(
        runtime_key: impl Into<String>,
        identity: impl Into<String>,
        kind: ConsoleDeltaKind,
        assistant_message_id: impl Into<String>,
        first_delta: ConsoleCursor,
        complete: ConsoleCursor,
    ) -> Self {
        Self {
            runtime_key: runtime_key.into(),
            identity: identity.into(),
            kind,
            assistant_message_id: assistant_message_id.into(),
            first_delta,
            complete,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleVisibility {
    Addressable,
    Hidden,
    RetiredReadable,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleIdentityRecord {
    pub identity: String,
    pub display_name: String,
    pub runtime_key: String,
    pub runtime_member_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Core's mob member id of the identity's last binding, for an
    /// identity-first identity Core holds a binding of; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    pub visibility: ConsoleVisibility,
    pub addressable: bool,
    pub health: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topology_peers: Vec<String>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleIdentityInspection {
    pub identity: ConsoleIdentityRecord,
    #[serde(default)]
    pub peers: Vec<String>,
}

/// Typed kind of the caller behind a console send.
///
/// `origin` stays the caller's free-form identifier for audit and routing;
/// this is the closed, typed classification a transcript renders from
/// ("Operator probe", "Scheduled turn", ...). Absent means the caller did not
/// declare one. Unknown values are refused at deserialization rather than
/// guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ConsoleTurnOrigin {
    /// A human operator typing into a console or client.
    Operator,
    /// An automated operator check that asks the agent for a proof reply
    /// (release gates, health probes).
    OperatorProbe,
    /// An external connector delivering an event as a turn.
    Connector,
    /// A scheduled job.
    Scheduler,
    /// A policy-driven turn.
    Policy,
    /// A flow step.
    Flow,
    /// Host-internal system work.
    System,
}

impl ConsoleTurnOrigin {
    /// The snake_case wire value, as persisted on the `user_input` frame.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
            Self::OperatorProbe => "operator_probe",
            Self::Connector => "connector",
            Self::Scheduler => "scheduler",
            Self::Policy => "policy",
            Self::Flow => "flow",
            Self::System => "system",
        }
    }

    /// Whether this kind names one of Meerkat's own internal emitters (a
    /// schedule, a connector, a flow, a policy, the host), which no HTTP send
    /// may claim. An operator's kind and an operator probe's (a caller's own
    /// label for an automated check) are not reserved.
    pub const fn reserved_for_internal_senders(self) -> bool {
        !matches!(self, Self::Operator | Self::OperatorProbe)
    }
}

/// What kind of caller the Console acts as for one request, derived from the
/// connection alone: the token the Console verified, or none on an open
/// Console. No request field names it. `meta/console/whoami` reports it as
/// `actor.kind`, and a console message's [`ConsoleSender`] names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleActorKind {
    /// A verified human token: an OIDC issuer and `sub`.
    Person,
    /// A verified service token (`svc:` principal or service actor).
    Service,
    /// A verified token that names no human (no `sub`).
    TokenPrincipal,
    /// A caller of an open Console without a verified token.
    Anonymous,
}

impl ConsoleActorKind {
    /// The kind's wire name, as `meta/console/whoami` reports it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Service => "service",
            Self::TokenPrincipal => "token_principal",
            Self::Anonymous => "anonymous",
        }
    }

    /// The authentication assurance the Console established for this kind
    /// of caller; a token that names no human claims none.
    pub const fn assurance(self) -> Option<meerkat_core::ApprovalAssurance> {
        match self {
            Self::Person => Some(meerkat_core::ApprovalAssurance::PersonAuthenticated),
            Self::Service => Some(meerkat_core::ApprovalAssurance::ServiceCredential),
            Self::TokenPrincipal | Self::Anonymous => None,
        }
    }
}

/// Who sent a console message, as the Console authenticated the caller of
/// the send: never taken from the request body. It is recorded on the
/// `user_input` frame the send makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleSender {
    /// What kind of caller sent it, as `meta/console/whoami` reports the
    /// caller.
    pub kind: ConsoleActorKind,
    /// The authentication assurance the Console established for the caller,
    /// if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assurance: Option<meerkat_core::ApprovalAssurance>,
    /// The name the Console knows the caller by (the authenticated principal,
    /// such as a verified email); absent for an anonymous caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The caller's opaque scope, as `meta/console/whoami` reports it, so a
    /// client tells its own messages from another caller's.
    pub caller_scope: String,
    /// The person's qualified principal, which the send carries to the
    /// agent's run as its original requester. Kept with the stored frame and
    /// never projected to a view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<meerkat_core::PrincipalRef>,
    /// The label the agent's model reads right before a person's message on
    /// a console that requires sign-in: "Message from" and the display name.
    /// Attribution for the model, never control data. Kept with the stored
    /// frame (a redispatch reads it there) and never projected to a view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_label: Option<String>,
}

impl ConsoleSender {
    /// The sender a stored `user_input` frame recorded, if it recorded one.
    pub(crate) fn of_frame_payload(payload: &Value) -> Option<Self> {
        payload
            .get("sender")
            .and_then(|sender| serde_json::from_value(sender.clone()).ok())
    }

    /// What this sender's send carries to the agent: its requester and its
    /// sender label.
    pub(crate) fn attribution(&self) -> ConsoleHumanAttribution {
        ConsoleHumanAttribution {
            requester: self.requester.clone(),
            sender_label: self
                .model_label
                .clone()
                .and_then(meerkat_mob::SenderLabel::new),
            typed_text: false,
        }
    }
}

/// What a console send carries to the agent's run for the person who sent
/// it: the requester the run works for and the sender label its model reads
/// before the message. Both are absent for every send that names no person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ConsoleHumanAttribution {
    pub(crate) requester: Option<meerkat_core::PrincipalRef>,
    pub(crate) sender_label: Option<meerkat_mob::SenderLabel>,
    /// Whether a signed-in person typed THIS turn's text: a send with a
    /// person requester whose origin is a live operator send (`operator` or
    /// no kind). A scheduled fire, a probe or any internal emitter carries the
    /// person as requester but is not typed text.
    pub(crate) typed_text: bool,
}

impl ConsoleHumanAttribution {
    /// The attribution a send's recorded sender carries; none without one.
    /// It claims no typed text: see [`Self::of_send`].
    pub(crate) fn of_sender(sender: Option<&ConsoleSender>) -> Self {
        sender.map(ConsoleSender::attribution).unwrap_or_default()
    }

    /// The attribution of a send by `sender` declaring `origin_kind`: typed
    /// text only for a person's live operator send.
    pub(crate) fn of_send(
        sender: Option<&ConsoleSender>,
        origin_kind: Option<ConsoleTurnOrigin>,
    ) -> Self {
        let mut attribution = Self::of_sender(sender);
        attribution.typed_text = attribution.requester.is_some()
            && matches!(origin_kind, None | Some(ConsoleTurnOrigin::Operator));
        attribution
    }

    /// The attribution a send's own `user_input` frame recorded: its sender
    /// and its origin kind, so a redispatch carries exactly what the send had.
    pub(crate) fn of_frame_payload(payload: &serde_json::Value) -> Self {
        let origin_kind = payload
            .get("origin_kind")
            .and_then(|kind| serde_json::from_value::<ConsoleTurnOrigin>(kind.clone()).ok());
        Self::of_send(
            ConsoleSender::of_frame_payload(payload).as_ref(),
            origin_kind,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleSendRequest {
    pub identity: String,
    pub content: Value,
    pub origin: String,
    pub idempotency_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handling_mode: Option<String>,
    /// Typed caller kind; persisted on the `user_input` frame as
    /// `origin_kind` so the transcript labels the turn from data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_kind: Option<ConsoleTurnOrigin>,
    /// Source-pinned skills selected for this exact turn, in the platform's
    /// typed `skill_refs` wire shape (`{"kind": "structured", "source_uuid",
    /// "skill_name"}`, as on meerkat `turn/start` and `mob/turn_start`). The
    /// target member resolves them natively (typed `SkillsResolved` /
    /// `SkillResolutionFailed` events and a durable `SkillContext`); Meta
    /// never renders them into the content. A nonempty selection is part of
    /// the idempotency identity: a replay must repeat it, and a changed
    /// selection under the same key is an idempotency conflict. Only local
    /// session-backed members accept a selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skill_refs: Vec<meerkat_core::skills::SkillRef>,
}

impl ConsoleSendRequest {
    /// The canonical skill keys of [`Self::skill_refs`], in request order.
    pub fn selected_skill_keys(&self) -> Vec<meerkat_core::skills::SkillKey> {
        self.skill_refs
            .iter()
            .map(|skill| skill.key().clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleInteractionAccepted {
    pub interaction_id: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub input_frame_id: String,
    pub cursor: ConsoleCursor,
    pub status: ConsoleFrameStatus,
}

/// Outcome of reserving an identity-first console interaction.
///
/// `Fresh` is a newly minted interaction the caller owns and must dispatch.
/// `Existing` is the original acceptance of an idempotent replay (same
/// `idempotency_key`, same origin, content and handling mode): the turn it
/// names was already dispatched once and must not be dispatched again.
#[derive(Debug, Clone, PartialEq)]
pub enum IdentityFirstReservation {
    Fresh(ConsoleInteractionAccepted),
    Existing(ConsoleInteractionAccepted),
}

impl IdentityFirstReservation {
    pub fn into_accepted(self) -> ConsoleInteractionAccepted {
        match self {
            Self::Fresh(accepted) | Self::Existing(accepted) => accepted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConsoleTimelineEvent {
    SnapshotStarted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<ConsoleCursor>,
    },
    ConsoleFrame {
        frame: ConsoleFrame,
    },
    FrameUpdated {
        frame: ConsoleFrame,
    },
    SnapshotComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cursor: Option<ConsoleCursor>,
    },
    ReplayUnavailable {
        requested_cursor: String,
        latest_cursor: Option<ConsoleCursor>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleReplayUnavailable {
    pub error: String,
    pub requested_cursor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_cursor: Option<ConsoleCursor>,
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod timeline_window_query_defaults {
    use super::*;

    /// An omitted `limit` on the wire must match the programmatic default.
    /// Regression: `#[serde(default)]` alone deserialized 0, the scan clamped
    /// it to 1, and every SDK/test caller that omitted `limit` silently got a
    /// single frame (the reference front-end always sends 400, hiding it).
    #[test]
    fn omitted_limit_deserializes_to_the_struct_default() {
        let q: ConsoleTimelineWindowQuery = serde_json::from_str("{}").expect("empty query");
        assert_eq!(q.limit, ConsoleTimelineWindowQuery::default().limit);
        assert_eq!(q.limit, 200);
    }

    #[test]
    fn omitted_limit_on_the_plain_query_matches_its_default_too() {
        let q: ConsoleTimelineQuery = serde_json::from_str("{}").expect("empty query");
        assert_eq!(q.limit, ConsoleTimelineQuery::default().limit);
    }

    #[test]
    fn explicit_limit_is_preserved() {
        let q: ConsoleTimelineWindowQuery =
            serde_json::from_str(r#"{"limit": 7}"#).expect("explicit limit");
        assert_eq!(q.limit, 7);
    }
}

#[cfg(test)]
mod console_turn_origin_tests {
    use super::{ConsoleSendRequest, ConsoleTurnOrigin};

    #[test]
    fn origin_kind_is_optional_typed_and_fail_closed() -> Result<(), serde_json::Error> {
        let absent: ConsoleSendRequest = serde_json::from_value(serde_json::json!({
            "identity": "agent:a", "content": "hi", "origin": "console:p", "idempotency_key": "k"
        }))?;
        assert_eq!(absent.origin_kind, None);
        assert!(serde_json::to_value(&absent)?.get("origin_kind").is_none());

        let probe: ConsoleSendRequest = serde_json::from_value(serde_json::json!({
            "identity": "agent:a", "content": "hi", "origin": "example:gate",
            "idempotency_key": "k", "origin_kind": "operator_probe"
        }))?;
        assert_eq!(probe.origin_kind, Some(ConsoleTurnOrigin::OperatorProbe));

        let unknown = serde_json::from_value::<ConsoleSendRequest>(serde_json::json!({
            "identity": "agent:a", "content": "hi", "origin": "x",
            "idempotency_key": "k", "origin_kind": "gate_probe"
        }));
        assert!(
            unknown.is_err(),
            "unknown origin kinds are refused, not guessed"
        );

        for kind in [
            ConsoleTurnOrigin::Operator,
            ConsoleTurnOrigin::OperatorProbe,
            ConsoleTurnOrigin::Connector,
            ConsoleTurnOrigin::Scheduler,
            ConsoleTurnOrigin::Policy,
            ConsoleTurnOrigin::Flow,
            ConsoleTurnOrigin::System,
        ] {
            assert_eq!(serde_json::to_value(kind)?, kind.as_str());
        }
        Ok(())
    }
}
