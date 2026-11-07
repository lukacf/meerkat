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
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::console_aggregator::{
    ConsoleFrame, ConsoleFrameStatus, ConsoleThreadAudience, ConsoleThreadReference,
    MetaConsoleAggregator, PrivateReplyAppend,
};
use crate::runtime::{MetadataStoreError, PersistentMetadataStore, RecordRetention};

/// The most items one attention read answers: the newest.
pub(crate) const CONSOLE_ATTENTION_PAGE: usize = 100;

/// The key prefix of the attention items among the realm documents: the
/// addressee's digest and the item id follow.
const ITEM_PREFIX: &str = "console-attention/";

/// The item format this build writes and reads.
const ITEM_VERSION: u32 = 1;

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
    /// not private to them, kept as a host's payload policy keeps the
    /// reply's frame: `text` is what the policy left of it (none when it
    /// left no text), and `redacted` says the policy rewrote it.
    PrivateReply {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
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

/// One stored item: the item, whose it is, and whether it was dismissed.
#[derive(Debug, Serialize, Deserialize)]
struct StoredItem {
    v: u32,
    /// The addressee's private-owner key.
    addressee: String,
    item: ConsoleAttentionItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dismissed_at_ms: Option<u64>,
}

/// How an item delivery went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemDelivery {
    /// Stored now.
    Stored(ConsoleAttentionItem),
    /// The item was stored before, with the same content.
    Existing(ConsoleAttentionItem),
    /// The item was stored before, with other content: nothing changed.
    Conflict,
}

/// The realm's person-addressed attention items, over its metadata store.
pub(crate) struct ConsoleAttention {
    store: Arc<dyn PersistentMetadataStore>,
    retention: RecordRetention,
    /// Serializes this process's writes, so a delivery's check and write and
    /// a dismissal's read and write are each one step.
    writes: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for ConsoleAttention {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleAttention")
            .field("retention", &self.retention)
            .finish_non_exhaustive()
    }
}

/// A hex SHA-256 of `parts` under `domain`: keys that carry neither the
/// addressee nor the item's source in clear.
fn digest(domain: &str, parts: &[&str]) -> String {
    format!(
        "{:x}",
        Sha256::digest(json!([domain, parts]).to_string().as_bytes())
    )
}

fn addressee_prefix(addressee: &str) -> String {
    format!(
        "{ITEM_PREFIX}{}/",
        digest("meta-console-attention-addressee-v1", &[addressee])
    )
}

/// The id of the item that delivers the reply `reply_id` to the request
/// `interaction_id`.
pub(crate) fn private_reply_item_id(interaction_id: &str, reply_id: &str) -> String {
    digest(
        "meta-console-attention-reply-v1",
        &[interaction_id, reply_id],
    )
}

fn is_item_id(item_id: &str) -> bool {
    item_id.len() == 64 && item_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn decode(key: &str, document: &str) -> Result<StoredItem, MetadataStoreError> {
    let stored: StoredItem = serde_json::from_str(document)
        .map_err(|error| MetadataStoreError::Decode(format!("{key}: {error}")))?;
    if stored.v != ITEM_VERSION {
        return Err(MetadataStoreError::Decode(format!(
            "{key}: attention item version {} is not {ITEM_VERSION}",
            stored.v
        )));
    }
    Ok(stored)
}

fn encode(key: &str, stored: &StoredItem) -> Result<String, MetadataStoreError> {
    serde_json::to_string(stored).map_err(|error| MetadataStoreError::Io(format!("{key}: {error}")))
}

impl ConsoleAttention {
    /// The attention list over `store`; `None` when the store keeps no realm
    /// documents.
    pub(crate) fn over(store: Arc<dyn PersistentMetadataStore>) -> Option<Self> {
        let retention = store.realm_document_retention()?;
        Some(Self {
            store,
            retention,
            writes: tokio::sync::Mutex::new(()),
        })
    }

    /// How long the store keeps the items.
    pub(crate) fn retention(&self) -> RecordRetention {
        self.retention
    }

    /// The stored item `item_id` of the person whose private-owner key is
    /// `addressee`, read whole.
    async fn stored(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<Option<(String, StoredItem)>, MetadataStoreError> {
        if !is_item_id(item_id) {
            return Ok(None);
        }
        let key = format!("{}{item_id}", addressee_prefix(addressee));
        let Some(document) = self.store.realm_document(&key).await? else {
            return Ok(None);
        };
        let stored = decode(&key, &document)?;
        // A key names its addressee by digest; the stored item names it in
        // full, and only that addressee's item answers.
        Ok((stored.addressee == addressee).then_some((key, stored)))
    }

    /// The item `item_id` of the person whose private-owner key is
    /// `addressee`, dismissed or not.
    pub(crate) async fn item(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<Option<ConsoleAttentionItem>, MetadataStoreError> {
        Ok(self
            .stored(addressee, item_id)
            .await?
            .map(|(_, stored)| stored.item))
    }

    /// Deliver `content` to the person whose private-owner key is
    /// `addressee`, as the item `item_id`, once.
    pub(crate) async fn deliver(
        &self,
        addressee: &str,
        item_id: &str,
        content: ConsoleAttentionContent,
    ) -> Result<ItemDelivery, MetadataStoreError> {
        let _writes = self.writes.lock().await;
        if let Some((_, stored)) = self.stored(addressee, item_id).await? {
            return Ok(if stored.item.content == content {
                ItemDelivery::Existing(stored.item)
            } else {
                ItemDelivery::Conflict
            });
        }
        let key = format!("{}{item_id}", addressee_prefix(addressee));
        let item = ConsoleAttentionItem {
            id: item_id.to_string(),
            created_at_ms: now_ms(),
            content,
        };
        let stored = StoredItem {
            v: ITEM_VERSION,
            addressee: addressee.to_string(),
            item: item.clone(),
            dismissed_at_ms: None,
        };
        self.store
            .set_realm_document(&key, Some(&encode(&key, &stored)?))
            .await?;
        Ok(ItemDelivery::Stored(item))
    }

    /// The items delivered to the person whose private-owner key is
    /// `addressee` and not dismissed, newest first, at most
    /// [`CONSOLE_ATTENTION_PAGE`].
    pub(crate) async fn list(
        &self,
        addressee: &str,
    ) -> Result<Vec<ConsoleAttentionItem>, MetadataStoreError> {
        let prefix = addressee_prefix(addressee);
        let mut items = Vec::new();
        for key in self.store.realm_document_keys(&prefix).await? {
            let Some(document) = self.store.realm_document(&key).await? else {
                continue;
            };
            let stored = decode(&key, &document)?;
            if stored.addressee == addressee && stored.dismissed_at_ms.is_none() {
                items.push(stored.item);
            }
        }
        items.sort_by(|a, b| {
            b.created_at_ms
                .cmp(&a.created_at_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        items.truncate(CONSOLE_ATTENTION_PAGE);
        Ok(items)
    }

    /// Dismiss the item `item_id` of the person whose private-owner key is
    /// `addressee`: `true` when that person's item is dismissed (now or
    /// before), `false` when the person has no such item. A dismissed item
    /// is no longer listed; it stays stored, so its delivery stays answered.
    pub(crate) async fn dismiss(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<bool, MetadataStoreError> {
        let _writes = self.writes.lock().await;
        let Some((key, mut stored)) = self.stored(addressee, item_id).await? else {
            return Ok(false);
        };
        if stored.dismissed_at_ms.is_none() {
            stored.dismissed_at_ms = Some(now_ms());
            self.store
                .set_realm_document(&key, Some(&encode(&key, &stored)?))
                .await?;
        }
        Ok(true)
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

fn store_error(error: impl std::fmt::Display) -> ConsoleReplyError {
    ConsoleReplyError::Store(error.to_string())
}

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

/// The receipt of a reply appended to its thread as `frame`.
fn thread_receipt(frame: &ConsoleFrame) -> ConsoleReplyReceipt {
    ConsoleReplyReceipt {
        delivered_at_ms: frame.timestamp_ms,
        delivery: ConsoleReplyDelivery::Thread {
            frame_id: frame.id.clone(),
        },
    }
}

/// The receipt of a reply delivered as `item`.
fn attention_receipt(item: &ConsoleAttentionItem) -> ConsoleReplyReceipt {
    ConsoleReplyReceipt {
        delivered_at_ms: item.created_at_ms,
        delivery: ConsoleReplyDelivery::Attention {
            item_id: item.id.clone(),
        },
    }
}

/// The text a reply's frame was delivered with; `None` when a host's
/// payload policy redacted the frame before it was stored, so the text can
/// no longer be compared.
fn delivered_text(frame: &ConsoleFrame) -> Option<String> {
    if frame.status == ConsoleFrameStatus::Redacted {
        return None;
    }
    Some(
        frame
            .payload
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
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
            .map_err(store_error)
    }

    /// The request's thread, with the person its replies are for.
    async fn addressed_thread(
        &self,
        interaction_id: &str,
    ) -> Result<(ConsoleThreadReference, String), ConsoleReplyError> {
        let reference = self
            .thread_reference(interaction_id)
            .await?
            .ok_or(ConsoleReplyError::UnknownThread)?;
        let addressee = reference
            .addressee
            .clone()
            .ok_or(ConsoleReplyError::NoAddressee)?;
        Ok((reference, addressee))
    }

    /// Whether a reply to the request with interaction id `interaction_id`
    /// can reach its person privately: the request recorded a thread that
    /// names a person, and the reply has somewhere to go (the person's
    /// private thread, or the attention list).
    pub(crate) async fn can_deliver(
        &self,
        interaction_id: &str,
    ) -> Result<bool, ConsoleReplyError> {
        match self.addressed_thread(interaction_id).await {
            Ok((reference, _)) => {
                Ok(reference.audience == ConsoleThreadAudience::Person || self.attention.is_some())
            }
            Err(ConsoleReplyError::UnknownThread | ConsoleReplyError::NoAddressee) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The receipt of the reply `reply_id` to the request `reference`, and
    /// the text it was delivered with (`None` for a frame or an item a host's
    /// payload policy redacted), if it was delivered.
    async fn find(
        &self,
        reference: &ConsoleThreadReference,
        addressee: &str,
        reply_id: &str,
    ) -> Result<Option<(ConsoleReplyReceipt, Option<String>)>, ConsoleReplyError> {
        if let Some(frame) = self
            .aggregator
            .private_reply_frame(&reference.interaction_id, reply_id)
            .await
            .map_err(store_error)?
        {
            return Ok(Some((thread_receipt(&frame), delivered_text(&frame))));
        }
        let Some(attention) = &self.attention else {
            return Ok(None);
        };
        let item_id = private_reply_item_id(&reference.interaction_id, reply_id);
        Ok(attention
            .item(addressee, &item_id)
            .await
            .map_err(store_error)?
            .map(|item| {
                let ConsoleAttentionContent::PrivateReply { text, redacted, .. } = &item.content;
                let text = (!*redacted).then(|| text.clone().unwrap_or_default());
                (attention_receipt(&item), text)
            }))
    }

    /// Deliver `text` as the reply `reply_id` to the request with interaction
    /// id `interaction_id`, to the person who made it only: appended to their
    /// private thread when the request was made there and the thread is
    /// still theirs, else as an item of their attention list. The same reply
    /// again answers the first delivery's receipt; other text under the same
    /// reply id is refused, unless a host's payload policy redacted the
    /// reply's frame, whose text can no longer be compared.
    pub(crate) async fn deliver(
        &self,
        interaction_id: &str,
        reply_id: &str,
        text: &str,
    ) -> Result<ConsoleReplyReceipt, ConsoleReplyError> {
        let (reference, addressee) = self.addressed_thread(interaction_id).await?;
        if let Some((receipt, delivered)) = self.find(&reference, &addressee, reply_id).await? {
            return if delivered.is_none_or(|delivered| delivered == text) {
                Ok(receipt)
            } else {
                Err(ConsoleReplyError::Conflict)
            };
        }
        if reference.audience == ConsoleThreadAudience::Person {
            match self
                .aggregator
                .append_private_reply(&reference, reply_id, text)
                .await
                .map_err(store_error)?
            {
                PrivateReplyAppend::Appended(frame) | PrivateReplyAppend::Existing(frame) => {
                    return if delivered_text(&frame).is_none_or(|delivered| delivered == text) {
                        Ok(thread_receipt(&frame))
                    } else {
                        Err(ConsoleReplyError::Conflict)
                    };
                }
                PrivateReplyAppend::ThreadGone => {}
            }
        }
        let attention = self
            .attention
            .as_ref()
            .ok_or(ConsoleReplyError::AttentionUnavailable)?;
        // Kept as the reply's frame would be: a host's payload policy that
        // rewrites the frame rewrites the item.
        let (payload, redacted) = self
            .aggregator
            .kept_private_reply(&reference.interaction_id, reply_id, text)
            .await
            .map_err(store_error)?
            .ok_or(ConsoleReplyError::UnknownThread)?;
        let content = ConsoleAttentionContent::PrivateReply {
            text: payload
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            redacted,
            origin: ConsoleReplyOrigin {
                identity: reference.identity.clone(),
                interaction_id: reference.interaction_id.clone(),
                input_frame_id: reference.input_frame_id.clone(),
            },
        };
        let item_id = private_reply_item_id(&reference.interaction_id, reply_id);
        match attention
            .deliver(&addressee, &item_id, content)
            .await
            .map_err(store_error)?
        {
            ItemDelivery::Stored(item) | ItemDelivery::Existing(item) => {
                Ok(attention_receipt(&item))
            }
            ItemDelivery::Conflict => Err(ConsoleReplyError::Conflict),
        }
    }

    /// The receipt of the reply `reply_id` to the request with interaction
    /// id `interaction_id`, if it was delivered: what a host's committed
    /// check reads.
    pub(crate) async fn delivered(
        &self,
        interaction_id: &str,
        reply_id: &str,
    ) -> Result<Option<ConsoleReplyReceipt>, ConsoleReplyError> {
        let (reference, addressee) = match self.addressed_thread(interaction_id).await {
            Ok(thread) => thread,
            Err(ConsoleReplyError::UnknownThread | ConsoleReplyError::NoAddressee) => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        Ok(self
            .find(&reference, &addressee, reply_id)
            .await?
            .map(|(receipt, _)| receipt))
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}
