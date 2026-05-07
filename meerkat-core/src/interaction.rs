//! Interaction types for the core agent loop.
//!
//! These types provide a simplified adapter layer in core (no comms dependency).
//! `CommsContent` in meerkat-comms remains canonical with richer types.
//! The comms runtime converts at the boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::comms::{
    PeerId, PeerLifecycleKind, PeerName, PeerRoute, SUPERVISOR_BRIDGE_INTENT, TrustedPeerDescriptor,
};
use crate::types::{ContentBlock, HandlingMode, RenderMetadata};

/// Unique identifier for an interaction.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InteractionId(#[cfg_attr(feature = "schema", schemars(with = "String"))] pub Uuid);

impl std::fmt::Display for InteractionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Typed status for response interactions.
///
/// Mirrors `CommsStatus` from `meerkat-comms` — the comms runtime converts at the boundary.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseStatus {
    Accepted,
    Completed,
    Failed,
}

/// Terminality projection for a typed `ResponseStatus`.
///
/// Runtime-backed peer ingress receives this as part of the typed
/// `PeerIngressClassification` emitted by the machine authority. Downstream
/// runtime/public projections must consume that carried terminality instead of
/// re-matching raw response status after admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TerminalityClass {
    Progress,
    Terminal { disposition: TerminalDisposition },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TerminalDisposition {
    Completed,
    Failed,
}

/// Single source of truth for "is this response terminal?".
pub fn classify_response_terminality(status: ResponseStatus) -> TerminalityClass {
    match status {
        ResponseStatus::Accepted => TerminalityClass::Progress,
        ResponseStatus::Completed => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Completed,
        },
        ResponseStatus::Failed => TerminalityClass::Terminal {
            disposition: TerminalDisposition::Failed,
        },
    }
}

/// Simplified interaction content for the core agent loop.
///
/// This is an adapter type — `CommsContent` in meerkat-comms has richer types
/// (`MessageIntent`, `CommsStatus`, etc.). The comms runtime converts at the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionContent {
    /// A simple text message.
    Message {
        body: String,
        /// Optional multimodal content blocks.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
    /// A request for the agent to perform an action.
    Request {
        intent: String,
        params: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocks: Option<Vec<ContentBlock>>,
    },
    /// A response to a previous request.
    Response {
        in_reply_to: InteractionId,
        status: ResponseStatus,
        result: Value,
    },
}

/// An interaction drained from the inbox, ready for classification.
#[derive(Debug, Clone)]
pub struct InboxInteraction {
    /// Unique identifier for this interaction.
    pub id: InteractionId,
    /// Machine route identity for peer senders. Plain external events leave
    /// this unset because they are source-labelled, not peer-routed.
    pub from_route: Option<PeerId>,
    /// Who sent this interaction (peer display name or source label).
    pub from: String,
    /// The interaction content.
    pub content: InteractionContent,
    /// Pre-rendered text suitable for injection into an LLM session.
    pub rendered_text: String,
    /// Runtime-owned handling hint for ordinary work admitted from plain events.
    pub handling_mode: HandlingMode,
    /// Optional normalized rendering metadata carried alongside the interaction.
    pub render_metadata: Option<RenderMetadata>,
}

/// Canonical model-facing text projection for an external event.
///
/// The visible identity of an external event is its source label
/// (`webhook`, `rpc`, `stdin`, etc.). Optional body text may follow, but
/// structured payload remains typed metadata rather than prompt text.
pub fn format_external_event_projection(source_name: &str, body: Option<&str>) -> String {
    let label = format!("[EVENT via {source_name}]");
    let body = body.map(str::trim).filter(|body| !body.is_empty());

    match body {
        Some(body) => format!("{label} {body}"),
        None => label,
    }
}

/// Canonical model-facing text projection for a peer message.
pub fn format_peer_message_projection(from_peer: &str, body: &str) -> String {
    format!("[COMMS MESSAGE from {from_peer}]\n{body}")
}

/// Schema-shaped model-facing `send_response` call affordance.
///
/// This helper owns the field names used when a prompt tells a model how to
/// answer a correlated peer request. The MCP `SendResponseInput` schema must
/// accept the object rendered here; comms tests pin that boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendResponseCallProjection {
    pub peer_id: PeerId,
    pub display_name: Option<String>,
    pub in_reply_to: String,
}

impl SendResponseCallProjection {
    pub const TOOL_NAME: &'static str = "send_response";
    pub const PEER_ID_FIELD: &'static str = "peer_id";
    pub const DISPLAY_NAME_FIELD: &'static str = "display_name";
    pub const IN_REPLY_TO_FIELD: &'static str = "in_reply_to";
    pub const STATUS_FIELD: &'static str = "status";
    pub const RESULT_FIELD: &'static str = "result";

    pub fn new(
        peer_id: PeerId,
        display_name: Option<&str>,
        in_reply_to: impl Into<String>,
    ) -> Self {
        Self {
            peer_id,
            display_name: display_name
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToOwned::to_owned),
            in_reply_to: in_reply_to.into(),
        }
    }

    /// A concrete, schema-valid example argument object for a completed reply.
    ///
    /// The model may replace `status` with `"failed"`. Public result payloads
    /// are typed by the comms contract, so the generic projection omits a
    /// result body instead of advertising arbitrary JSON.
    pub fn completed_example_args(&self) -> Value {
        let mut args = serde_json::Map::new();
        args.insert(
            Self::PEER_ID_FIELD.to_string(),
            Value::String(self.peer_id.to_string()),
        );
        if let Some(display_name) = &self.display_name {
            args.insert(
                Self::DISPLAY_NAME_FIELD.to_string(),
                Value::String(display_name.clone()),
            );
        }
        args.insert(
            Self::IN_REPLY_TO_FIELD.to_string(),
            Value::String(self.in_reply_to.clone()),
        );
        args.insert(
            Self::STATUS_FIELD.to_string(),
            Value::String("completed".to_string()),
        );
        Value::Object(args)
    }

    pub fn instruction_text(&self) -> String {
        let args = serde_json::to_string(&self.completed_example_args())
            .unwrap_or_else(|_| "{}".to_string());
        format!(
            "Reply with {} with arguments {args}. Use status=\"failed\" instead of \"completed\" when the request cannot be fulfilled, and include result only when the request contract provides a typed result payload.",
            Self::TOOL_NAME
        )
    }
}

/// Canonical model-facing text projection for a correlated peer request.
pub fn format_peer_request_projection(
    from_peer_id: PeerId,
    display_name: Option<&str>,
    request_id: impl std::fmt::Display,
    intent: &str,
    params: &Value,
) -> String {
    let params_str = if params.is_null() || matches!(params, Value::Object(map) if map.is_empty()) {
        String::new()
    } else {
        format!(
            "\nParams: {}",
            serde_json::to_string_pretty(params).unwrap_or_default()
        )
    };
    let request_id = request_id.to_string();
    let display_suffix = display_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| format!(" (display_name: {name})"))
        .unwrap_or_default();
    let response_call =
        SendResponseCallProjection::new(from_peer_id, display_name, request_id.clone());

    format!(
        "[COMMS REQUEST from peer_id {from_peer_id}{display_suffix} (id: {request_id})]\n\
         Intent: {intent}{params_str}\n\
         \n\
         This is a correlated peer request. {} \
         Do not answer this request with send_message.",
        response_call.instruction_text()
    )
}

/// Canonical model-facing text projection for a peer response.
pub fn format_peer_response_projection(
    from_peer: &str,
    in_reply_to: impl std::fmt::Display,
    status: ResponseStatus,
    result: &Value,
) -> String {
    let status_str = match status {
        ResponseStatus::Accepted => "accepted",
        ResponseStatus::Completed => "completed",
        ResponseStatus::Failed => "failed",
    };
    let result_str = if result.is_null() || matches!(result, Value::Object(map) if map.is_empty()) {
        String::new()
    } else {
        format!(
            "\nResult: {}",
            serde_json::to_string_pretty(result).unwrap_or_default()
        )
    };

    format!(
        "[COMMS RESPONSE from {from_peer} (to request: {in_reply_to})]\n\
         Status: {status_str}{result_str}"
    )
}

/// Canonical model-facing text projection for a peer ack.
pub fn format_peer_ack_projection(from_peer: &str, in_reply_to: impl std::fmt::Display) -> String {
    format!("[COMMS ACK from {from_peer} (to request: {in_reply_to})]")
}

/// Classification result for incoming peer/event traffic.
///
/// Stored with each inbox entry at ingress time. Downstream consumers
/// switch on this enum instead of re-classifying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerInputClass {
    /// A peer message that should route through canonical runtime admission.
    ActionableMessage,
    /// A peer request that should route through canonical runtime admission.
    ActionableRequest,
    /// A non-terminal response to a previous outbound request.
    ResponseProgress,
    /// A terminal response to a previous outbound request.
    ResponseTerminal,
    /// Peer added lifecycle event.
    PeerLifecycleAdded,
    /// Peer retired lifecycle event.
    PeerLifecycleRetired,
    /// Peer unwired lifecycle event.
    PeerLifecycleUnwired,
    /// Member kickoff failed lifecycle event.
    PeerLifecycleKickoffFailed,
    /// Member kickoff cancelled lifecycle event.
    PeerLifecycleKickoffCancelled,
    /// A request whose intent is in the silent-intents set (inline-only, no LLM turn).
    SilentRequest,
    /// An ack envelope (filtered at ingress, never reaches agent loop).
    Ack,
    /// A plain (unauthenticated) event from an external source.
    PlainEvent,
}

impl PeerInputClass {
    /// Returns true if this class is actionable runtime ingress.
    pub fn is_actionable(&self) -> bool {
        matches!(
            self,
            Self::ActionableMessage
                | Self::ActionableRequest
                | Self::ResponseProgress
                | Self::ResponseTerminal
                | Self::PlainEvent
                | Self::PeerLifecycleKickoffFailed
                | Self::PeerLifecycleKickoffCancelled
        )
    }
}

/// Typed auth exemption recognized by peer ingress authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PeerIngressAuthExemption {
    /// Supervisor bridge bootstrap request.
    SupervisorBridge,
}

impl PeerIngressAuthExemption {
    pub const fn intent(self) -> &'static str {
        match self {
            Self::SupervisorBridge => SUPERVISOR_BRIDGE_INTENT,
        }
    }

    pub fn matches_intent(self, intent: &str) -> bool {
        self.intent() == intent
    }
}

/// Auth decision attached to a classified peer ingress item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerIngressAuthDecision {
    /// Sender must be trusted when peer auth is required.
    Required,
    /// The item is allowed through the trust gate for a typed bootstrap reason.
    Exempt(PeerIngressAuthExemption),
}

impl PeerIngressAuthDecision {
    pub const fn is_exempt(self) -> bool {
        matches!(self, Self::Exempt(_))
    }
}

/// Typed peer convention admitted at the peer-ingress seam.
///
/// This is the core-side ingress convention, not a rendered prompt. Runtime
/// prompt/schema projections derive from it after admission so `InboxInteraction::from`
/// never has to carry both display and canonical identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerIngressConvention {
    Message,
    Request {
        request_id: String,
        intent: String,
    },
    Response {
        in_reply_to: InteractionId,
        status: ResponseStatus,
    },
    Ack {
        in_reply_to: InteractionId,
    },
    Lifecycle {
        kind: PeerLifecycleKind,
        peer: String,
    },
    PlainEvent {
        source_name: String,
    },
}

/// Typed fact admitted at the peer-ingress seam.
///
/// The legacy `InboxInteraction::from` field remains a compatibility display
/// label. Runtime routing, trust, bridge response resolution, and prompt/schema
/// projection must consume the matching typed field on this fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressFact {
    /// Interaction/correlation identifier stamped at ingress.
    pub interaction_id: InteractionId,
    /// Pre-computed ingress class.
    pub class: PeerInputClass,
    /// Coarse admitted kind.
    pub kind: PeerIngressKind,
    /// Canonical comms peer id. This is the runtime prompt/schema peer id.
    pub canonical_peer_id: Option<PeerId>,
    /// Human-facing display label for diagnostics and legacy rendered text.
    pub display_name: Option<PeerName>,
    /// Ed25519 signing public key / trust subject when ingress was signed.
    pub signing_pubkey: Option<[u8; 32]>,
    /// Resolved route/binding handle for replies to this sender.
    pub route: Option<PeerRoute>,
    /// Auth decision used by peer ingress admission.
    pub auth: Option<PeerIngressAuthDecision>,
    /// Typed peer convention admitted at ingress.
    pub convention: PeerIngressConvention,
}

/// Sender identity admitted with a peer ingress fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressIdentity {
    pub canonical_peer_id: PeerId,
    pub display_label: String,
    pub signing_pubkey: Option<[u8; 32]>,
    pub convention: PeerIngressConvention,
}

impl PeerIngressIdentity {
    pub fn new(
        canonical_peer_id: PeerId,
        display_label: impl Into<String>,
        convention: PeerIngressConvention,
    ) -> Self {
        Self {
            canonical_peer_id,
            display_label: display_label.into(),
            signing_pubkey: None,
            convention,
        }
    }

    pub fn with_signing_pubkey(mut self, signing_pubkey: [u8; 32]) -> Self {
        self.signing_pubkey = Some(signing_pubkey);
        self
    }
}

impl PeerIngressFact {
    pub fn peer(
        interaction_id: InteractionId,
        class: PeerInputClass,
        kind: PeerIngressKind,
        auth: Option<PeerIngressAuthDecision>,
        identity: PeerIngressIdentity,
    ) -> Self {
        let PeerIngressIdentity {
            canonical_peer_id,
            display_label,
            signing_pubkey,
            convention,
        } = identity;
        let display_name = PeerName::new(display_label).ok();
        let route = Some(match &display_name {
            Some(name) => PeerRoute::with_display_name(canonical_peer_id, name.clone()),
            None => PeerRoute::new(canonical_peer_id),
        });
        Self {
            interaction_id,
            class,
            kind,
            canonical_peer_id: Some(canonical_peer_id),
            display_name,
            signing_pubkey,
            route,
            auth,
            convention,
        }
    }

    pub fn plain_event(
        interaction_id: InteractionId,
        source_name: impl Into<String>,
        class: PeerInputClass,
        kind: PeerIngressKind,
    ) -> Self {
        let source_name = source_name.into();
        Self {
            interaction_id,
            class,
            kind,
            canonical_peer_id: None,
            display_name: None,
            signing_pubkey: None,
            route: None,
            auth: None,
            convention: PeerIngressConvention::PlainEvent { source_name },
        }
    }

    /// Compatibility constructor for tests and legacy non-classified seams.
    ///
    /// Prefer constructing a full `peer(...)` fact with canonical peer id,
    /// signing subject, and route when the ingress came from comms.
    pub fn legacy_peer_label(
        interaction_id: InteractionId,
        label: impl Into<String>,
        class: PeerInputClass,
        kind: PeerIngressKind,
        auth: Option<PeerIngressAuthDecision>,
        convention: PeerIngressConvention,
    ) -> Self {
        let label = label.into();
        let canonical_peer_id = PeerId::parse(&label).ok();
        let display_name = PeerName::new(label).ok();
        let route = canonical_peer_id.map(|peer_id| match &display_name {
            Some(name) => PeerRoute::with_display_name(peer_id, name.clone()),
            None => PeerRoute::new(peer_id),
        });
        Self {
            interaction_id,
            class,
            kind,
            canonical_peer_id,
            display_name,
            signing_pubkey: None,
            route,
            auth,
            convention,
        }
    }

    pub fn canonical_peer_id_string(&self) -> Option<String> {
        self.canonical_peer_id.map(|peer_id| peer_id.as_str())
    }

    pub fn display_label(&self) -> Option<String> {
        self.display_name.as_ref().map(PeerName::as_string)
    }

    pub fn diagnostic_label(&self) -> String {
        self.display_label()
            .or_else(|| self.canonical_peer_id_string())
            .unwrap_or_else(|| "<unknown-peer-ingress>".to_string())
    }

    pub fn plain_event_source_name(&self) -> Option<&str> {
        match &self.convention {
            PeerIngressConvention::PlainEvent { source_name } => Some(source_name.as_str()),
            _ => None,
        }
    }
}

/// Typed output of machine-owned peer ingress classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressClassification {
    pub class: PeerInputClass,
    pub kind: PeerIngressKind,
    pub auth: PeerIngressAuthDecision,
    pub lifecycle_kind: Option<PeerLifecycleKind>,
    pub response_terminality: Option<TerminalityClass>,
}

impl PeerIngressClassification {
    pub const fn required(class: PeerInputClass, kind: PeerIngressKind) -> Self {
        Self {
            class,
            kind,
            auth: PeerIngressAuthDecision::Required,
            lifecycle_kind: None,
            response_terminality: None,
        }
    }

    pub const fn lifecycle(kind: PeerLifecycleKind) -> Self {
        let class = match kind {
            PeerLifecycleKind::PeerAdded => PeerInputClass::PeerLifecycleAdded,
            PeerLifecycleKind::PeerRetired => PeerInputClass::PeerLifecycleRetired,
            PeerLifecycleKind::PeerUnwired => PeerInputClass::PeerLifecycleUnwired,
        };
        Self {
            class,
            kind: PeerIngressKind::Request,
            auth: PeerIngressAuthDecision::Required,
            lifecycle_kind: Some(kind),
            response_terminality: None,
        }
    }
}

/// Parsed transport facts for one peer-envelope ingress item.
///
/// This is intentionally a typed adapter shape: comms may parse the envelope
/// mechanics into this struct, but the policy below owns all semantic
/// classification derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerIngressEnvelopeFacts {
    pub item_id: String,
    pub from_peer: String,
    pub from_peer_id: PeerId,
    pub kind: PeerIngressEnvelopeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PeerIngressEnvelopeKind {
    Message {
        body: String,
    },
    Request {
        intent: String,
        params: Value,
    },
    Lifecycle {
        kind: PeerLifecycleKind,
        params: Value,
    },
    Response {
        in_reply_to: String,
        status: ResponseStatus,
        result: Value,
    },
    Ack {
        in_reply_to: String,
    },
}

/// Parsed transport facts for one plain external event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressPlainEventFacts {
    pub source_name: String,
    pub body: String,
}

/// Complete typed admission facts produced by peer-ingress classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressAdmission {
    pub classification: PeerIngressClassification,
    pub lifecycle_peer: Option<String>,
    pub request_id: Option<String>,
    pub rendered_text: String,
}

/// Standalone compatibility adapter for peer ingress classification.
///
/// Runtime-backed comms must use the MeerkatMachine
/// `PeerIngressClassified` effect as authority. This adapter exists only for
/// standalone comms runtimes without a session DSL and for tests that need a
/// wire-compatible projection of machine behavior. Raw inbox ingress and
/// runtime-required classified ingress must not use it as a second authority
/// for auth exemptions, lifecycle intent, or response terminality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressMachinePolicy {
    silent_request_intents: BTreeSet<String>,
    auth_exemptions: BTreeSet<PeerIngressAuthExemption>,
}

impl Default for PeerIngressMachinePolicy {
    fn default() -> Self {
        Self::from_silent_intents(std::iter::empty::<String>())
    }
}

impl PeerIngressMachinePolicy {
    pub fn from_silent_intents<I, S>(silent_intents: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            silent_request_intents: silent_intents.into_iter().map(Into::into).collect(),
            auth_exemptions: BTreeSet::from([PeerIngressAuthExemption::SupervisorBridge]),
        }
    }

    pub fn classify_message(&self) -> PeerIngressClassification {
        PeerIngressClassification::required(
            PeerInputClass::ActionableMessage,
            PeerIngressKind::Message,
        )
    }

    pub fn classify_request_intent(&self, intent: &str) -> PeerIngressClassification {
        let auth = self
            .auth_exemptions
            .iter()
            .copied()
            .find(|exemption| exemption.matches_intent(intent))
            .map(PeerIngressAuthDecision::Exempt)
            .unwrap_or(PeerIngressAuthDecision::Required);

        let mut classification = if let Some(kind) = classify_lifecycle_intent(intent) {
            PeerIngressClassification::lifecycle(kind)
        } else if self.silent_request_intents.contains(intent) {
            PeerIngressClassification::required(
                PeerInputClass::SilentRequest,
                PeerIngressKind::Request,
            )
        } else {
            PeerIngressClassification::required(
                PeerInputClass::ActionableRequest,
                PeerIngressKind::Request,
            )
        };
        classification.auth = auth;
        classification
    }

    pub fn classify_lifecycle(&self, kind: PeerLifecycleKind) -> PeerIngressClassification {
        PeerIngressClassification::lifecycle(kind)
    }

    pub fn classify_response(&self, status: ResponseStatus) -> PeerIngressClassification {
        let terminality = classify_response_terminality(status);
        let class = match terminality {
            TerminalityClass::Progress => PeerInputClass::ResponseProgress,
            TerminalityClass::Terminal { .. } => PeerInputClass::ResponseTerminal,
        };
        let mut classification =
            PeerIngressClassification::required(class, PeerIngressKind::Response);
        classification.response_terminality = Some(terminality);
        classification
    }

    pub fn classify_ack(&self) -> PeerIngressClassification {
        PeerIngressClassification::required(PeerInputClass::Ack, PeerIngressKind::Ack)
    }

    pub fn classify_plain_event(&self) -> PeerIngressClassification {
        PeerIngressClassification::required(PeerInputClass::PlainEvent, PeerIngressKind::PlainEvent)
    }

    pub fn classify_external_envelope(
        &self,
        facts: &PeerIngressEnvelopeFacts,
    ) -> PeerIngressAdmission {
        match &facts.kind {
            PeerIngressEnvelopeKind::Message { .. } => {
                let classification = self.classify_message();
                PeerIngressAdmission {
                    rendered_text: render_peer_ingress_admitted_text(facts, &classification),
                    classification,
                    lifecycle_peer: None,
                    request_id: None,
                }
            }
            PeerIngressEnvelopeKind::Request { intent, params } => {
                let classification = self.classify_request_intent(intent);
                let lifecycle_peer = classification
                    .lifecycle_kind
                    .map(|_| peer_lifecycle_subject(params, facts.from_peer.as_str()));
                let rendered_text = render_peer_ingress_admitted_text(facts, &classification);
                PeerIngressAdmission {
                    classification,
                    lifecycle_peer,
                    request_id: Some(facts.item_id.clone()),
                    rendered_text,
                }
            }
            PeerIngressEnvelopeKind::Lifecycle { kind, params } => {
                let classification = self.classify_lifecycle(*kind);
                PeerIngressAdmission {
                    rendered_text: render_peer_ingress_admitted_text(facts, &classification),
                    classification,
                    lifecycle_peer: Some(peer_lifecycle_subject(params, facts.from_peer.as_str())),
                    request_id: None,
                }
            }
            PeerIngressEnvelopeKind::Response {
                in_reply_to,
                status,
                ..
            } => {
                let classification = self.classify_response(*status);
                PeerIngressAdmission {
                    rendered_text: render_peer_ingress_admitted_text(facts, &classification),
                    classification,
                    lifecycle_peer: None,
                    request_id: Some(in_reply_to.clone()),
                }
            }
            PeerIngressEnvelopeKind::Ack { in_reply_to } => {
                let classification = self.classify_ack();
                PeerIngressAdmission {
                    rendered_text: render_peer_ingress_admitted_text(facts, &classification),
                    classification,
                    lifecycle_peer: None,
                    request_id: Some(in_reply_to.clone()),
                }
            }
        }
    }

    pub fn classify_plain_event_facts(
        &self,
        facts: &PeerIngressPlainEventFacts,
    ) -> PeerIngressAdmission {
        PeerIngressAdmission {
            classification: self.classify_plain_event(),
            lifecycle_peer: None,
            request_id: None,
            rendered_text: format_external_event_projection(&facts.source_name, Some(&facts.body)),
        }
    }
}

/// Derive model-facing text after typed peer ingress admission.
///
/// Classification is the authority. This renderer only projects already
/// admitted facts into prompt text, so callers cannot change routing or auth
/// by editing prose formatting.
pub fn render_peer_ingress_admitted_text(
    facts: &PeerIngressEnvelopeFacts,
    classification: &PeerIngressClassification,
) -> String {
    match &facts.kind {
        PeerIngressEnvelopeKind::Message { body } => {
            format_peer_message_projection(&facts.from_peer, body)
        }
        PeerIngressEnvelopeKind::Request { intent, params } => {
            if classification.lifecycle_kind.is_some() {
                String::new()
            } else {
                format_peer_request_projection(
                    facts.from_peer_id,
                    Some(&facts.from_peer),
                    facts.item_id.as_str(),
                    intent,
                    params,
                )
            }
        }
        PeerIngressEnvelopeKind::Lifecycle { .. } => String::new(),
        PeerIngressEnvelopeKind::Response {
            in_reply_to,
            status,
            result,
        } => format_peer_response_projection(&facts.from_peer, in_reply_to, *status, result),
        PeerIngressEnvelopeKind::Ack { in_reply_to } => {
            format_peer_ack_projection(&facts.from_peer, in_reply_to)
        }
    }
}

/// Extract the lifecycle subject from typed request/lifecycle parameters.
pub fn peer_lifecycle_subject(params: &Value, fallback_peer: &str) -> String {
    params
        .get("peer")
        .and_then(Value::as_str)
        .filter(|peer| !peer.is_empty())
        .unwrap_or(fallback_peer)
        .to_string()
}

fn classify_lifecycle_intent(intent: &str) -> Option<PeerLifecycleKind> {
    if intent == PeerLifecycleKind::PeerAdded.as_str() {
        Some(PeerLifecycleKind::PeerAdded)
    } else if intent == PeerLifecycleKind::PeerRetired.as_str() {
        Some(PeerLifecycleKind::PeerRetired)
    } else if intent == PeerLifecycleKind::PeerUnwired.as_str() {
        Some(PeerLifecycleKind::PeerUnwired)
    } else {
        None
    }
}

/// Canonical peer/event ingress candidate handed to runtime admission.
///
/// This is the typed, machine-authored drain unit for runtime-backed peer
/// ingress. It preserves ingress classification so downstream code does not
/// re-derive semantics after drain.
#[derive(Debug, Clone)]
pub struct PeerInputCandidate {
    /// The original interaction data.
    pub interaction: InboxInteraction,
    /// Typed admitted ingress fact. Consumers must use this for canonical peer
    /// identity, display labels, trust subjects, route handles, and convention.
    pub ingress: PeerIngressFact,
    /// For lifecycle events, the peer name that was added/retired.
    pub lifecycle_peer: Option<String>,
    /// For response events, the machine-owned progress/terminal classifier.
    pub response_terminality: Option<TerminalityClass>,
}

impl PeerInputCandidate {
    pub fn new(
        interaction: InboxInteraction,
        ingress: PeerIngressFact,
        lifecycle_peer: Option<String>,
    ) -> Self {
        Self {
            interaction,
            ingress,
            lifecycle_peer,
            response_terminality: None,
        }
    }

    pub fn class(&self) -> PeerInputClass {
        self.ingress.class
    }

    pub fn kind(&self) -> PeerIngressKind {
        self.ingress.kind
    }

    pub fn auth(&self) -> Option<PeerIngressAuthDecision> {
        self.ingress.auth
    }
}

/// Back-compat alias for older runtime and diagnostic seams.
pub type ClassifiedInboxInteraction = PeerInputCandidate;

/// Coarse source kind for a queued peer-ingress item.
///
/// This is a diagnostic shape for MeerkatMachine mapping work. It records the
/// kind that was admitted at ingress without exposing transport internals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerIngressKind {
    Message,
    Request,
    Response,
    Ack,
    PlainEvent,
}

/// Display-only peer or source label captured for ingress diagnostics.
///
/// This is deliberately not a routing, trust, or admission identity. Canonical
/// peer authority lives in the admitted ingress fact and runtime/machine
/// admission state; snapshot rows only expose this label so operators can read
/// queue diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressDiagnosticDisplay(String);

impl PeerIngressDiagnosticDisplay {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PeerIngressDiagnosticDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Diagnostic copy of the admission-time trust observation for a queued item.
///
/// This records what admission observed when the item was queued. It is not a
/// live trust oracle and must not be used to reconstruct routing or admission
/// authority from a snapshot row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerIngressAdmissionDiagnostic {
    TrustedAtAdmission,
    UntrustedAtAdmission,
}

impl PeerIngressAdmissionDiagnostic {
    pub const fn from_trusted(trusted: bool) -> Self {
        if trusted {
            Self::TrustedAtAdmission
        } else {
            Self::UntrustedAtAdmission
        }
    }

    pub const fn trusted_at_admission(self) -> bool {
        matches!(self, Self::TrustedAtAdmission)
    }
}

/// Snapshot of one queued peer-ingress item.
///
/// Snapshot rows are diagnostics derived from the canonical admitted ingress
/// candidate. They are intentionally incomplete for route/trust reconstruction:
/// peer labels are display-only, correlation ids are typed, and admission
/// details are diagnostic copies rather than authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressEntrySnapshot {
    /// Stable typed ingress-time identity for this queued raw item.
    pub raw_item_id: InteractionId,
    /// Interaction/correlation identifier when one exists.
    pub interaction_id: Option<InteractionId>,
    /// Pre-computed ingress classification.
    pub class: PeerInputClass,
    /// Coarse admitted kind.
    pub kind: PeerIngressKind,
    /// Display-only sender label, if applicable. Not route/trust authority.
    pub from_peer_display: Option<PeerIngressDiagnosticDisplay>,
    /// Canonical sender peer id fixed at ingress time, if applicable.
    pub canonical_peer_id: Option<PeerId>,
    /// Display peer name fixed at ingress time, if applicable.
    pub display_name: Option<PeerName>,
    /// Signing public key / trust subject fixed at ingress time, if applicable.
    pub signing_pubkey: Option<[u8; 32]>,
    /// Resolved reply route fixed at ingress time, if applicable.
    pub route: Option<PeerRoute>,
    /// Display-only lifecycle peer label, if applicable. Not route/trust authority.
    pub lifecycle_peer_display: Option<PeerIngressDiagnosticDisplay>,
    /// Request envelope id or reply-to correlation when one exists.
    pub request_correlation_id: Option<InteractionId>,
    /// Auth decision used by peer ingress admission, if this queued entry came
    /// from authenticated peer transport. Plain events leave this unset.
    pub auth: Option<PeerIngressAuthDecision>,
    /// Admission-time trust diagnostic, when peer authority owns the entry.
    /// Plain external events leave this unset.
    pub admission_diagnostic: Option<PeerIngressAdmissionDiagnostic>,
    /// Machine-owned response progress/terminal classifier when this entry is
    /// a response.
    pub response_terminality: Option<TerminalityClass>,
}

/// Non-destructive snapshot of the queued peer-ingress surface.
///
/// This is intentionally queue-shaped rather than a full PeerComms model. It
/// is the current honest owner-visible slice of peer ingress while the broader
/// MeerkatMachine refactor proceeds.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PeerIngressQueueSnapshot {
    pub total_count: usize,
    pub actionable_count: usize,
    pub response_count: usize,
    pub lifecycle_count: usize,
    pub silent_request_count: usize,
    pub ack_count: usize,
    pub plain_event_count: usize,
    pub queued_entries: Vec<PeerIngressEntrySnapshot>,
}

/// Canonical phase of the peer-ingress authority.
///
/// This is distinct from the raw classified queue snapshot: plain external
/// events can be queued while the peer authority itself remains `Absent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerIngressAuthorityPhase {
    #[default]
    Absent,
    Received,
    Dropped,
    Delivered,
}

/// Runtime-owned peer snapshot for the current Meerkat session.
///
/// This wraps the queued ingress surface with the trust membership that governs
/// which peer identities are admitted into that queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIngressRuntimeSnapshot {
    /// This runtime's public peer identity.
    pub self_peer_id: crate::comms::PeerId,
    /// Whether unauthenticated peer envelopes are rejected at ingress.
    pub auth_required: bool,
    /// Current phase of the peer-ingress authority.
    pub authority_phase: PeerIngressAuthorityPhase,
    /// Current trusted peer set visible to this runtime.
    pub trusted_peers: Vec<TrustedPeerDescriptor>,
    /// Current length of the authority-owned typed peer submission queue.
    pub submission_queue_len: usize,
    /// Non-destructive snapshot of the queued ingress surface.
    pub queue: PeerIngressQueueSnapshot,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn interaction_id_json_roundtrip() {
        let id = InteractionId(Uuid::new_v4());
        let json = serde_json::to_string(&id).unwrap();
        let parsed: InteractionId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn interaction_content_message_json_roundtrip() {
        let content = InteractionContent::Message {
            body: "hello".to_string(),
            blocks: None,
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "message");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn interaction_content_request_json_roundtrip() {
        let content = InteractionContent::Request {
            intent: "review".to_string(),
            params: serde_json::json!({"pr": 42}),
            blocks: None,
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "request");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn interaction_content_response_json_roundtrip() {
        let id = InteractionId(Uuid::new_v4());
        let content = InteractionContent::Response {
            in_reply_to: id,
            status: ResponseStatus::Completed,
            result: serde_json::json!({"ok": true}),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "response");
        assert_eq!(json["status"], "completed");
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn response_status_json_roundtrip_all_variants() {
        for (variant, expected_str) in [
            (ResponseStatus::Accepted, "accepted"),
            (ResponseStatus::Completed, "completed"),
            (ResponseStatus::Failed, "failed"),
        ] {
            let json = serde_json::to_value(variant).unwrap();
            assert_eq!(json, expected_str);
            let parsed: ResponseStatus = serde_json::from_value(json).unwrap();
            assert_eq!(variant, parsed);
        }
    }

    #[test]
    fn classify_response_terminality_covers_all_variants() {
        assert_eq!(
            classify_response_terminality(ResponseStatus::Accepted),
            TerminalityClass::Progress
        );
        assert_eq!(
            classify_response_terminality(ResponseStatus::Completed),
            TerminalityClass::Terminal {
                disposition: TerminalDisposition::Completed
            }
        );
        assert_eq!(
            classify_response_terminality(ResponseStatus::Failed),
            TerminalityClass::Terminal {
                disposition: TerminalDisposition::Failed
            }
        );
    }

    #[test]
    fn peer_ingress_policy_owns_response_terminal_classification() {
        let policy = PeerIngressMachinePolicy::default();

        assert_eq!(
            policy.classify_response(ResponseStatus::Accepted).class,
            PeerInputClass::ResponseProgress
        );
        assert_eq!(
            policy.classify_response(ResponseStatus::Completed).class,
            PeerInputClass::ResponseTerminal
        );
        assert_eq!(
            policy.classify_response(ResponseStatus::Failed).class,
            PeerInputClass::ResponseTerminal
        );
    }

    #[test]
    fn peer_ingress_policy_auth_exempts_supervisor_bridge() {
        let policy = PeerIngressMachinePolicy::default();
        let classification = policy.classify_request_intent(crate::SUPERVISOR_BRIDGE_INTENT);

        assert_eq!(classification.class, PeerInputClass::ActionableRequest);
        assert_eq!(
            classification.auth,
            PeerIngressAuthDecision::Exempt(PeerIngressAuthExemption::SupervisorBridge)
        );
    }

    #[test]
    fn peer_ingress_policy_owns_lifecycle_and_silent_routing() {
        let policy = PeerIngressMachinePolicy::from_silent_intents(["probe.silent"]);

        let lifecycle = policy.classify_request_intent(PeerLifecycleKind::PeerUnwired.as_str());
        assert_eq!(lifecycle.class, PeerInputClass::PeerLifecycleUnwired);
        assert_eq!(
            lifecycle.lifecycle_kind,
            Some(PeerLifecycleKind::PeerUnwired)
        );

        let silent = policy.classify_request_intent("probe.silent");
        assert_eq!(silent.class, PeerInputClass::SilentRequest);
        assert_eq!(silent.auth, PeerIngressAuthDecision::Required);
    }

    #[test]
    fn interaction_message_with_blocks_roundtrip() {
        let content = InteractionContent::Message {
            body: "hello".to_string(),
            blocks: Some(vec![
                ContentBlock::Text {
                    text: "hello".to_string(),
                },
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: "iVBORw0KGgo=".into(),
                },
            ]),
        };
        let json = serde_json::to_value(&content).unwrap();
        assert_eq!(json["type"], "message");
        assert!(json["blocks"].is_array());
        let parsed: InteractionContent = serde_json::from_value(json).unwrap();
        assert_eq!(content, parsed);
    }

    #[test]
    fn inbox_interaction_preserves_runtime_hints() {
        let interaction = InboxInteraction {
            id: InteractionId(Uuid::new_v4()),
            from_route: None,
            from: "event:webhook".into(),
            content: InteractionContent::Message {
                body: "hello".into(),
                blocks: None,
            },
            rendered_text: "[EVENT via webhook] hello".into(),
            handling_mode: HandlingMode::Steer,
            render_metadata: Some(RenderMetadata {
                class: crate::types::RenderClass::SystemNotice,
                salience: crate::types::RenderSalience::Urgent,
            }),
        };

        assert_eq!(interaction.handling_mode, HandlingMode::Steer);
        assert!(interaction.render_metadata.is_some());
    }

    #[test]
    fn interaction_message_without_blocks_compat() {
        // Old format (no blocks field) should deserialize with blocks: None
        let old_json = r#"{"type":"message","body":"hello"}"#;
        let parsed: InteractionContent = serde_json::from_str(old_json).unwrap();
        match parsed {
            InteractionContent::Message { body, blocks } => {
                assert_eq!(body, "hello");
                assert_eq!(blocks, None);
            }
            other => panic!("Expected Message, got {other:?}"),
        }

        // Serialize with blocks: None should omit the field
        let content = InteractionContent::Message {
            body: "test".to_string(),
            blocks: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            value.get("blocks").is_none(),
            "blocks: None should not appear in JSON"
        );
    }
}
