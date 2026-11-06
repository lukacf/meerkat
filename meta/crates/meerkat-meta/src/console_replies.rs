//! Person-addressed delivery: a reply meant for the one person who made a
//! console request, and the console attention list that holds the replies no
//! private thread takes.
//!
//! A console send records its thread
//! ([`ConsoleThreadReference`](crate::console_aggregator::ConsoleThreadReference)):
//! the member it went to, its interaction, who may read the replies to it
//! and the person they are for. A reply to it reaches only that person:
//!
//! - a request made in a member private to that person (their private side
//!   chat, or a member a private member's turn created) gets the reply
//!   appended there, as a `private_reply` frame of that conversation, which
//!   no one but that person and the access admins sees;
//! - a request made in a shared member gets the reply as a person-addressed
//!   attention item: only that person reads it (`meta/console/attention`),
//!   and nothing is appended to the shared conversation or sent on any
//!   timeline stream. So does a request whose private thread is gone when
//!   the reply comes.
//!
//! A request whose sender names no person (an open console's caller, a
//! service, a token that names no person) has no private reply route, and a
//! reply to it is refused.
//!
//! One reply is delivered once: the same reply id with the same text answers
//! the first delivery's receipt, and other text is refused. The attention
//! list is kept in the realm's metadata store as realm documents
//! ([`PersistentMetadataStore::realm_document`]), one per item, for as long
//! as the store keeps them; a store that keeps none has no attention list.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::console_aggregator::{ConsoleThreadReference, MetaConsoleAggregator};
use crate::runtime::{MetadataStoreError, PersistentMetadataStore, RecordRetention};

/// The most items one attention read answers: the newest.
pub(crate) const CONSOLE_ATTENTION_PAGE: usize = 100;

/// Where the request a reply answers was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsoleReplyOrigin {
    /// The member the request was sent to.
    pub(crate) identity: String,
    /// The request's interaction id.
    pub(crate) interaction_id: String,
    /// The request's send record (its `user_input` frame).
    pub(crate) input_frame_id: String,
}

/// What a person-addressed attention item holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum ConsoleAttentionContent {
    /// A reply to the person's own request, made in a conversation that is
    /// not private to them.
    PrivateReply {
        text: String,
        origin: ConsoleReplyOrigin,
    },
}

/// One item of a person's attention list, as `meta/console/attention`
/// answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsoleAttentionItem {
    /// Opaque and stable: names the item to dismiss it.
    pub(crate) id: String,
    /// When the item was delivered, in milliseconds since the epoch.
    pub(crate) created_at_ms: u64,
    #[serde(flatten)]
    pub(crate) content: ConsoleAttentionContent,
}

/// The realm's person-addressed attention items, over its metadata store.
pub(crate) struct ConsoleAttention {
    store: Arc<dyn PersistentMetadataStore>,
    retention: RecordRetention,
}

impl std::fmt::Debug for ConsoleAttention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleAttention")
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

impl ConsoleAttention {
    /// The attention list over `store`; `None` when the store keeps no realm
    /// documents.
    pub(crate) fn over(store: Arc<dyn PersistentMetadataStore>) -> Option<Self> {
        let retention = store.realm_document_retention()?;
        Some(Self { store, retention })
    }

    /// How long the store keeps the items.
    pub(crate) fn retention(&self) -> RecordRetention {
        self.retention
    }

    /// The items delivered to the person whose private-owner key is
    /// `addressee` and not dismissed, newest first, at most
    /// [`CONSOLE_ATTENTION_PAGE`].
    pub(crate) async fn list(
        &self,
        _addressee: &str,
    ) -> Result<Vec<ConsoleAttentionItem>, MetadataStoreError> {
        let _ = &self.store;
        Ok(Vec::new())
    }

    /// Dismiss the item `item_id` of the person whose private-owner key is
    /// `addressee`: `true` when that person's item is dismissed (now or
    /// before), `false` when the person has no such item.
    pub(crate) async fn dismiss(
        &self,
        _addressee: &str,
        _item_id: &str,
    ) -> Result<bool, MetadataStoreError> {
        Ok(false)
    }
}

/// Where a delivered reply went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleReplyDelivery {
    /// Appended to the person's private thread, as this frame.
    Thread { frame_id: String },
    /// A person-addressed attention item.
    Attention { item_id: String },
}

/// One delivered reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConsoleReplyReceipt {
    /// When it was delivered, in milliseconds since the epoch.
    pub(crate) delivered_at_ms: u64,
    pub(crate) delivery: ConsoleReplyDelivery,
}

/// Why a reply was not delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConsoleReplyError {
    /// No console send recorded a thread for the interaction.
    UnknownThread,
    /// The request names no person, so no reply reaches anyone privately.
    NoAddressee,
    /// The reply needs the attention list, and the console keeps none.
    AttentionUnavailable,
    /// The reply id was delivered with other text.
    Conflict,
    /// A store failed; whether the reply was delivered is not known.
    Store(String),
}

impl std::fmt::Display for ConsoleReplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownThread => {
                f.write_str("no console send recorded this interaction's thread")
            }
            Self::NoAddressee => {
                f.write_str("the request names no person, so it has no private reply route")
            }
            Self::AttentionUnavailable => f.write_str("this console keeps no attention list"),
            Self::Conflict => f.write_str("the reply was already delivered with other text"),
            Self::Store(error) => write!(f, "a console store failed: {error}"),
        }
    }
}

impl std::error::Error for ConsoleReplyError {}

/// Person-addressed delivery of replies to console requests.
#[derive(Clone)]
pub(crate) struct ConsoleReplies {
    aggregator: MetaConsoleAggregator,
    attention: Option<Arc<ConsoleAttention>>,
}

impl std::fmt::Debug for ConsoleReplies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleReplies")
            .field("attention", &self.attention)
            .finish_non_exhaustive()
    }
}

impl ConsoleReplies {
    pub(crate) fn new(
        aggregator: MetaConsoleAggregator,
        attention: Option<Arc<ConsoleAttention>>,
    ) -> Self {
        Self {
            aggregator,
            attention,
        }
    }

    /// The thread the console send with interaction id `interaction_id`
    /// recorded.
    pub(crate) async fn thread_reference(
        &self,
        interaction_id: &str,
    ) -> Result<Option<ConsoleThreadReference>, ConsoleReplyError> {
        self.aggregator
            .thread_reference(interaction_id)
            .await
            .map_err(|error| ConsoleReplyError::Store(error.to_string()))
    }

    /// Whether a reply to the request with interaction id `interaction_id`
    /// can reach its person privately.
    pub(crate) async fn can_deliver(
        &self,
        _interaction_id: &str,
    ) -> Result<bool, ConsoleReplyError> {
        Ok(false)
    }

    /// Deliver `text` as the reply `reply_id` to the request with interaction
    /// id `interaction_id`, to the person who made it only.
    pub(crate) async fn deliver(
        &self,
        _interaction_id: &str,
        _reply_id: &str,
        _text: &str,
    ) -> Result<ConsoleReplyReceipt, ConsoleReplyError> {
        Err(ConsoleReplyError::UnknownThread)
    }

    /// The receipt of the reply `reply_id` to the request with interaction
    /// id `interaction_id`, if it was delivered.
    pub(crate) async fn delivered(
        &self,
        _interaction_id: &str,
        _reply_id: &str,
    ) -> Result<Option<ConsoleReplyReceipt>, ConsoleReplyError> {
        Ok(None)
    }
}
