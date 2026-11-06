//! The thread a console send records, and replies delivered into it.
//!
//! A console send keeps its thread on its send record (the `user_input`
//! frame of the send): the member it went to (the frame's identity), the
//! send's interaction id (the frame's), who may read the replies to it (the
//! audience) and the person they are for (the addressee). Everything is set
//! by the console from the caller it resolved and the route the send took;
//! no request field names any of it.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    AppendDisposition, ConsoleFrame, ConsoleFrameMemberProvenance, ConsoleFrameSource,
    ConsoleFrameSourceKind, ConsoleFrameStatus, ConsoleLogResult, ConsoleTimelineEvent,
    MetaConsoleAggregator, NewConsoleFrame,
};

/// The kind of the frame a private reply is appended to its thread as.
pub(crate) const PRIVATE_REPLY_FRAME_KIND: &str = "private_reply";

/// Who may read the replies to a console request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsoleThreadAudience {
    /// Everyone who may view the member the request was sent to.
    Shared,
    /// The one person the member is private to, who made the request (and
    /// the access admins, who see every private member).
    Person,
}

/// The person the replies to a console send are for: a verified person
/// sending on a console that requires sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleReplyAddressee {
    /// The person's private-owner key ([`crate::access::PRIVATE_TO_LABEL`]),
    /// the `caller_scope` `meta/console/whoami` reports to them.
    pub(crate) private_owner_key: String,
    /// The person's console principal (the access subject: the verified
    /// email, else the `sub`).
    pub(crate) access_principal: String,
}

/// The thread a console send records in its `user_input` payload, under
/// [`ConsoleSendThread::PAYLOAD_KEY`]. The access principal stays on the
/// stored record: no console projection carries it
/// ([`ConsoleSendThread::strip_for_view`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsoleSendThread {
    pub(crate) audience: ConsoleThreadAudience,
    /// The private-owner key of the person the replies to this send are
    /// for: the sender, when the send names a person. Absent for every other
    /// send, whose replies have no one to go to privately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) addressee: Option<String>,
    /// That person's console principal, kept next to the thread so an
    /// approver's `people` entry can be matched against the request's
    /// sender.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) access_principal: Option<String>,
}

impl ConsoleSendThread {
    /// The key of the thread in a `user_input` payload.
    pub(crate) const PAYLOAD_KEY: &'static str = "thread";

    /// The thread of a send to the member `provenance` captured, by
    /// `addressee` (none when the send names no person). It is a person
    /// thread exactly when that member is private to the addressee: its
    /// captured lineage, the input every console surface decides the
    /// member's visibility with, names a private owner, and every owner it
    /// names is the addressee. Every other send, an access admin's send into
    /// another person's private member included, is a shared thread.
    pub(crate) fn of_send(
        provenance: Option<&ConsoleFrameMemberProvenance>,
        addressee: Option<&ConsoleReplyAddressee>,
    ) -> Self {
        let audience = match (provenance, addressee) {
            (Some(provenance), Some(addressee))
                if private_to(provenance, &addressee.private_owner_key) =>
            {
                ConsoleThreadAudience::Person
            }
            _ => ConsoleThreadAudience::Shared,
        };
        Self {
            audience,
            addressee: addressee.map(|addressee| addressee.private_owner_key.clone()),
            access_principal: addressee.map(|addressee| addressee.access_principal.clone()),
        }
    }

    /// The thread a send record's payload holds, if it holds a readable one.
    fn of_payload(payload: &Value) -> Option<Self> {
        serde_json::from_value(payload.get(Self::PAYLOAD_KEY)?.clone()).ok()
    }

    /// Drop from a `user_input` payload served to a viewer what only the
    /// console reads: the addressee and the access principal. The audience
    /// stays.
    pub(crate) fn strip_for_view(payload: &mut Value) {
        if let Some(thread) = payload
            .get_mut(Self::PAYLOAD_KEY)
            .and_then(Value::as_object_mut)
        {
            thread.remove("addressee");
            thread.remove("access_principal");
        }
    }
}

/// Whether the member `provenance` captured is private to the person whose
/// private-owner key is `addressee`.
fn private_to(provenance: &ConsoleFrameMemberProvenance, addressee: &str) -> bool {
    let lineage = super::captured_lineage(provenance);
    let mut owners = lineage
        .iter()
        .filter_map(|link| link.attributes.as_ref())
        .filter_map(|attributes| crate::access::private_owner(&attributes.labels))
        .peekable();
    owners.peek().is_some() && owners.all(|owner| owner == addressee)
}

/// Where a console request came from, read back from its send record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleThreadReference {
    /// The member the request was sent to: an agent, or a side chat.
    pub(crate) identity: String,
    /// The send's interaction id.
    pub(crate) interaction_id: String,
    /// The send record's frame id: the request a reply links to.
    pub(crate) input_frame_id: String,
    pub(crate) audience: ConsoleThreadAudience,
    /// The private-owner key of the person the replies are for, when the
    /// send names one.
    pub(crate) addressee: Option<String>,
    /// The sender's console principal (the access subject: the verified
    /// email, else the `sub`), when the send names a person: what an
    /// approver's `people` entry is matched against.
    // Read where an approver's `people` entry is matched against a request's
    // sender.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) access_principal: Option<String>,
}

/// How a private reply's frame went into its thread.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PrivateReplyAppend {
    /// Appended now.
    Appended(ConsoleFrame),
    /// The reply's frame was appended before.
    Existing(ConsoleFrame),
    /// The thread's member is gone, or no longer private to the person the
    /// reply is for: nothing was appended.
    ThreadGone,
}

/// The dedupe key of the frame the reply `reply_id` to the request
/// `interaction_id` is appended as: one frame per reply.
pub(crate) fn private_reply_dedupe_key(interaction_id: &str, reply_id: &str) -> String {
    let digest = Sha256::digest(
        json!(["meta-console-private-reply-v1", interaction_id, reply_id])
            .to_string()
            .as_bytes(),
    );
    format!("private-reply:{digest:x}")
}

impl MetaConsoleAggregator {
    /// The thread the console send whose interaction id is `interaction_id`
    /// recorded; `None` when no send record with that interaction id holds a
    /// thread (an unknown interaction, a send made before threads were
    /// recorded, or a send of the public send API, which has no console
    /// caller).
    pub(crate) async fn thread_reference(
        &self,
        interaction_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleThreadReference>> {
        let Some(record) = self
            .inner
            .store
            .send_frame_by_interaction(interaction_id)
            .await?
        else {
            return Ok(None);
        };
        Ok(thread_reference_of(&record))
    }

    /// The frame of the reply `reply_id` to the request `interaction_id`, if
    /// one was appended to its thread.
    pub(crate) async fn private_reply_frame(
        &self,
        interaction_id: &str,
        reply_id: &str,
    ) -> ConsoleLogResult<Option<ConsoleFrame>> {
        self.inner
            .store
            .frame_by_dedupe_key(&private_reply_dedupe_key(interaction_id, reply_id))
            .await
    }

    /// Append `text` as the reply `reply_id` to its request's person thread
    /// (`reference`, a [`ConsoleThreadAudience::Person`] thread): a
    /// `private_reply` frame of that member's conversation, caused by the
    /// request's send record and carrying its captured member provenance, so
    /// every surface decides it as that member's own frames, which only the
    /// person and the access admins see. Nothing is appended when the
    /// member is gone or is no longer private to the person
    /// ([`PrivateReplyAppend::ThreadGone`]).
    pub(crate) async fn append_private_reply(
        &self,
        reference: &ConsoleThreadReference,
        reply_id: &str,
        text: &str,
    ) -> ConsoleLogResult<PrivateReplyAppend> {
        let dedupe_key = private_reply_dedupe_key(&reference.interaction_id, reply_id);
        if let Some(existing) = self.inner.store.frame_by_dedupe_key(&dedupe_key).await? {
            return Ok(PrivateReplyAppend::Existing(existing));
        }
        let (Some(addressee), Some(record)) = (
            reference.addressee.as_deref(),
            self.inner
                .store
                .send_frame_by_interaction(&reference.interaction_id)
                .await?,
        ) else {
            return Ok(PrivateReplyAppend::ThreadGone);
        };
        if !Box::pin(self.member_still_private_to(&record.identity, addressee)).await {
            return Ok(PrivateReplyAppend::ThreadGone);
        }
        let mut frame = NewConsoleFrame {
            id: None,
            dedupe_key,
            timestamp_ms: super::current_time_ms(),
            runtime_key: record.runtime_key.clone(),
            identity: record.identity.clone(),
            conversation_id: record.conversation_id.clone(),
            session_id: record.session_id.clone(),
            kind: PRIVATE_REPLY_FRAME_KIND.to_string(),
            status: ConsoleFrameStatus::Completed,
            payload: json!({
                "text": text,
                "in_reply_to": {
                    "interaction_id": reference.interaction_id,
                    "input_frame_id": record.id,
                },
            }),
            source: ConsoleFrameSource {
                member_provenance: record.source.member_provenance.clone(),
                kind: ConsoleFrameSourceKind::Synthetic,
                source_cursor: None,
            },
            source_event_id: None,
            interaction_id: None,
            turn_id: None,
            run_id: None,
            parent_frame_id: None,
            caused_by_frame_id: Some(record.id.clone()),
        };
        let policy = super::capture_host_payload_policy(&self.inner, &record.runtime_key, None)?;
        super::redact_frame(policy.as_ref(), &mut frame);
        let outcome = super::append_frame(&self.inner, frame).await?;
        if outcome.disposition == AppendDisposition::Existing {
            return Ok(PrivateReplyAppend::Existing(outcome.frame));
        }
        let _ = self
            .inner
            .event_tx
            .send(ConsoleTimelineEvent::ConsoleFrame {
                frame: outcome.frame.clone(),
            });
        Ok(PrivateReplyAppend::Appended(outcome.frame))
    }

    /// Whether `identity` names a member now, private to the person whose
    /// private-owner key is `addressee`, as its member context reads now.
    async fn member_still_private_to(&self, identity: &str, addressee: &str) -> bool {
        let Ok(Some(resolved)) = Box::pin(self.resolve_send_member(identity)).await else {
            return false;
        };
        let Some(record) = Box::pin(super::identity_record_for_resolved_member(&resolved)).await
        else {
            return false;
        };
        let owner = self
            .inner
            .projection_owner
            .as_deref()
            .unwrap_or(&self.inner);
        let provenance =
            Box::pin(super::capture_member_provenance(owner, &resolved, &record)).await;
        private_to(&provenance, addressee)
    }
}

/// The thread reference a send record holds, read from its frame.
fn thread_reference_of(record: &ConsoleFrame) -> Option<ConsoleThreadReference> {
    let thread = ConsoleSendThread::of_payload(&record.payload)?;
    Some(ConsoleThreadReference {
        identity: record.identity.clone(),
        interaction_id: record.interaction_id.clone()?,
        input_frame_id: record.id.clone(),
        audience: thread.audience,
        addressee: thread.addressee,
        access_principal: thread.access_principal,
    })
}
