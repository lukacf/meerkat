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
//! the first delivery's receipt, and other text is refused.
//!
//! An attention item keeps its reply as the frame the reply would be
//! appended to a thread as: in the form the host's payload policy gives that
//! frame before it is stored, with the request's captured member context.
//! That stays on the server. Each read shows the item as the reading console
//! shows that frame in a thread: under the console's own payload policy and
//! visibility, and only while the reader may view the member the request
//! went to. An item never shows more than its thread would.
//!
//! The attention list is kept in the realm's metadata store as realm
//! documents ([`PersistentMetadataStore::realm_document`]), for as long as
//! the store keeps them, dismissed items too: each item is one record, read
//! by its id, and while it is not dismissed one listing entry, ordered by
//! delivery time. A read lists the entries alone, so it never reads a
//! dismissed item, and reads at most [`CONSOLE_ATTENTION_READ_CAP`] records.
//! A store that keeps no realm documents has no attention list.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::access::AccessView;
use crate::console_aggregator::{
    ConsoleFrame, ConsoleFrameStatus, ConsoleThreadAudience, ConsoleThreadReference,
    KeptReplyServing, MetaConsoleAggregator, NewConsoleFrame, PrivateReplyAppend, RecordedThread,
};
use crate::runtime::{MetadataStoreError, PersistentMetadataStore, RecordRetention};

/// The most items one attention read answers: the newest.
pub(crate) const CONSOLE_ATTENTION_PAGE: usize = 100;

/// The most items one attention read considers: the newest not dismissed. An
/// item the reader may not see is left out of the answer and still takes its
/// place among these.
pub(crate) const CONSOLE_ATTENTION_READ_CAP: usize = 2 * CONSOLE_ATTENTION_PAGE;

/// The key prefix of the attention documents among the realm documents: the
/// addressee's digest follows, then an item's record (`item/` and the item
/// id) or its listing entry (`live/`, the delivery time and the item id).
const ITEM_PREFIX: &str = "console-attention/";

/// An item's record, under its addressee's prefix.
const RECORD_SEGMENT: &str = "item/";

/// The listing entries of the items not dismissed, under their addressee's
/// prefix.
const LISTING_SEGMENT: &str = "live/";

/// The record format this build writes and reads.
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

/// What a person-addressed attention item shows, as `meta/console/attention`
/// answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum ConsoleAttentionContent {
    /// A reply to the person's own request, made in a conversation that is
    /// not private to them, as the reading console shows the reply's frame:
    /// `text` is what the payload policies left of it (none when they left
    /// no text), and `redacted` says a policy rewrote it.
    PrivateReply {
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
        origin: ConsoleReplyOrigin,
    },
}

/// One item of a person's attention list, as `meta/console/attention`
/// answers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ConsoleAttentionItem {
    /// Opaque and stable: names the item to dismiss it.
    pub(crate) id: String,
    /// When the item was delivered, in milliseconds since the epoch.
    pub(crate) created_at_ms: u64,
    #[serde(flatten)]
    pub(crate) content: ConsoleAttentionContent,
}

/// What a stored attention item holds. Never answered as stored: a read
/// shows it through the reading console ([`StoredAttentionItem::shown`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub(crate) enum StoredAttentionContent {
    /// A reply to the person's own request: the frame it would be appended
    /// to its thread as, as the host's payload policy left it, with the
    /// request's captured member context, and the request it answers.
    PrivateReply {
        frame: NewConsoleFrame,
        origin: ConsoleReplyOrigin,
    },
}

/// One stored attention item: whose it is, when it was delivered, what it
/// holds and whether it was dismissed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredAttentionItem {
    v: u32,
    /// The addressee's private-owner key.
    addressee: String,
    /// Opaque and stable: names the item to dismiss it.
    pub(crate) id: String,
    /// When the item was delivered, in milliseconds since the epoch.
    pub(crate) created_at_ms: u64,
    pub(crate) content: StoredAttentionContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dismissed_at_ms: Option<u64>,
}

impl StoredAttentionItem {
    /// The text the item's reply was delivered with; `None` when a host's
    /// payload policy redacted it before it was stored, so the text can no
    /// longer be compared.
    fn delivered_text(&self) -> Option<String> {
        match &self.content {
            StoredAttentionContent::PrivateReply { frame, .. } => {
                delivered_text(frame.status, &frame.payload)
            }
        }
    }

    /// The item as `serving` shows it, `None` when the reading console or
    /// the reader's access hides it.
    async fn shown(self, serving: &mut KeptReplyServing<'_>) -> Option<ConsoleAttentionItem> {
        match self.content {
            StoredAttentionContent::PrivateReply { frame, origin } => {
                let shown = serving.serve(&self.id, frame).await?;
                Some(ConsoleAttentionItem {
                    id: self.id,
                    created_at_ms: self.created_at_ms,
                    content: ConsoleAttentionContent::PrivateReply {
                        text: shown
                            .payload
                            .get("text")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        redacted: shown.status == ConsoleFrameStatus::Redacted,
                        origin,
                    },
                })
            }
        }
    }
}

/// How an item delivery went.
#[derive(Debug, Clone)]
pub(crate) enum ItemDelivery {
    /// Stored now.
    Stored(StoredAttentionItem),
    /// Stored before: the earlier item, unchanged.
    Existing(StoredAttentionItem),
}

/// The realm's person-addressed attention items, over its metadata store.
pub(crate) struct ConsoleAttention {
    store: Arc<dyn PersistentMetadataStore>,
    retention: RecordRetention,
    /// Serializes this process's writes, so a delivery's check and writes and
    /// a dismissal's read and writes are each one step.
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

/// The key of the record of `addressee`'s item `item_id`.
fn record_key(addressee: &str, item_id: &str) -> String {
    format!("{}{RECORD_SEGMENT}{item_id}", addressee_prefix(addressee))
}

/// The key prefix of `addressee`'s listing entries.
fn listing_prefix(addressee: &str) -> String {
    format!("{}{LISTING_SEGMENT}", addressee_prefix(addressee))
}

/// The key of the listing entry of `addressee`'s item `item_id`, delivered at
/// `created_at_ms`: the entries' keys order as their items were delivered
/// (then by item id), so the newest are read first.
fn listing_key(addressee: &str, created_at_ms: u64, item_id: &str) -> String {
    format!("{}{created_at_ms:020}-{item_id}", listing_prefix(addressee))
}

/// The item id the listing entry `key` under `prefix` names, if it names
/// one.
fn listed_item_id<'a>(prefix: &str, key: &'a str) -> Option<&'a str> {
    let (_, item_id) = key.strip_prefix(prefix)?.split_once('-')?;
    is_item_id(item_id).then_some(item_id)
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

/// The record `document` under `key`, read strictly: a record this build
/// cannot read is an error.
fn decode(key: &str, document: &str) -> Result<StoredAttentionItem, MetadataStoreError> {
    let stored: StoredAttentionItem = serde_json::from_str(document)
        .map_err(|error| MetadataStoreError::Decode(format!("{key}: {error}")))?;
    if stored.v != ITEM_VERSION {
        return Err(MetadataStoreError::Decode(format!(
            "{key}: attention item version {} is not {ITEM_VERSION}",
            stored.v
        )));
    }
    Ok(stored)
}

/// The record `document` under `key` as a read lists it: `None` for one this
/// build cannot read (a later format, or a damaged document), which is
/// logged by its key and left out. The log names no content: the key holds
/// digests only, and a decoding error is reported by its kind and place.
fn readable(key: &str, document: &str) -> Option<StoredAttentionItem> {
    #[derive(Deserialize)]
    struct Version {
        v: u32,
    }
    match serde_json::from_str::<Version>(document) {
        Ok(Version { v }) if v != ITEM_VERSION => {
            tracing::warn!(
                key,
                version = v,
                "an attention item of another format is left out of the list"
            );
            return None;
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(
                key,
                category = ?error.classify(),
                line = error.line(),
                column = error.column(),
                "an attention item that does not decode is left out of the list"
            );
            return None;
        }
    }
    match serde_json::from_str::<StoredAttentionItem>(document) {
        Ok(stored) => Some(stored),
        Err(error) => {
            tracing::warn!(
                key,
                category = ?error.classify(),
                line = error.line(),
                column = error.column(),
                "an attention item that does not decode is left out of the list"
            );
            None
        }
    }
}

fn encode(key: &str, stored: &StoredAttentionItem) -> Result<String, MetadataStoreError> {
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
    /// `addressee`, read whole and strictly: a record this build cannot read
    /// is an error, so nothing is delivered or dismissed over it.
    async fn stored(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<Option<StoredAttentionItem>, MetadataStoreError> {
        if !is_item_id(item_id) {
            return Ok(None);
        }
        let key = record_key(addressee, item_id);
        let Some(document) = self.store.realm_document(&key).await? else {
            return Ok(None);
        };
        let stored = decode(&key, &document)?;
        // A key names its addressee by digest; the stored item names it in
        // full, and only that addressee's item answers.
        Ok((stored.addressee == addressee).then_some(stored))
    }

    /// Whether the delivery of `stored` completed: it was dismissed, or it is
    /// listed.
    async fn delivery_complete(
        &self,
        stored: &StoredAttentionItem,
    ) -> Result<bool, MetadataStoreError> {
        if stored.dismissed_at_ms.is_some() {
            return Ok(true);
        }
        let entry = listing_key(&stored.addressee, stored.created_at_ms, &stored.id);
        Ok(self.store.realm_document(&entry).await?.is_some())
    }

    /// List `stored`, which is not dismissed.
    async fn list_entry(&self, stored: &StoredAttentionItem) -> Result<(), MetadataStoreError> {
        let entry = listing_key(&stored.addressee, stored.created_at_ms, &stored.id);
        // The entry's key names the item; the document holds nothing more.
        self.store
            .set_realm_document(&entry, Some(&json!({ "v": ITEM_VERSION }).to_string()))
            .await
    }

    /// The item `item_id` of the person whose private-owner key is
    /// `addressee`, dismissed or not, once its delivery completed. A delivery
    /// that stopped between the item's record and its listing entry has not
    /// delivered the item: delivering it again completes it.
    pub(crate) async fn item(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<Option<StoredAttentionItem>, MetadataStoreError> {
        let Some(stored) = self.stored(addressee, item_id).await? else {
            return Ok(None);
        };
        Ok(self.delivery_complete(&stored).await?.then_some(stored))
    }

    /// Deliver `content` to the person whose private-owner key is
    /// `addressee`, as the item `item_id`, once: its record first, then its
    /// listing entry, so no entry names an item without a record. An item
    /// stored before answers as it is, its listing entry written when its
    /// delivery stopped before the entry.
    pub(crate) async fn deliver(
        &self,
        addressee: &str,
        item_id: &str,
        content: StoredAttentionContent,
    ) -> Result<ItemDelivery, MetadataStoreError> {
        let _writes = self.writes.lock().await;
        if let Some(stored) = self.stored(addressee, item_id).await? {
            if !self.delivery_complete(&stored).await? {
                self.list_entry(&stored).await?;
            }
            return Ok(ItemDelivery::Existing(stored));
        }
        let stored = StoredAttentionItem {
            v: ITEM_VERSION,
            addressee: addressee.to_string(),
            id: item_id.to_string(),
            created_at_ms: now_ms(),
            content,
            dismissed_at_ms: None,
        };
        let key = record_key(addressee, item_id);
        self.store
            .set_realm_document(&key, Some(&encode(&key, &stored)?))
            .await?;
        self.list_entry(&stored).await?;
        Ok(ItemDelivery::Stored(stored))
    }

    /// The items of the person whose private-owner key is `addressee` that
    /// are not dismissed, as the console `view` shows them to a reader whose
    /// agent audience is `audience`, newest first: at most
    /// [`CONSOLE_ATTENTION_PAGE`] of them, from at most
    /// [`CONSOLE_ATTENTION_READ_CAP`] records read. The store lists the
    /// listing entries alone, which name no dismissed item, and the records
    /// are read newest first until a page is shown. An item the view or the
    /// audience hides is left out, and so is a record this build cannot read
    /// (logged by its key): neither fails the list.
    pub(crate) async fn list(
        &self,
        addressee: &str,
        view: &MetaConsoleAggregator,
        audience: &AccessView,
    ) -> Result<Vec<ConsoleAttentionItem>, MetadataStoreError> {
        let prefix = listing_prefix(addressee);
        let mut entries = self.store.realm_document_keys(&prefix).await?;
        // Newest first: the keys order by delivery time.
        entries.sort_unstable_by(|a, b| b.cmp(a));
        let mut serving = view.kept_reply_serving(audience).await;
        let mut items = Vec::new();
        for entry in entries.iter().take(CONSOLE_ATTENTION_READ_CAP) {
            if items.len() == CONSOLE_ATTENTION_PAGE {
                break;
            }
            let Some(item_id) = listed_item_id(&prefix, entry) else {
                continue;
            };
            let key = record_key(addressee, item_id);
            let Some(document) = self.store.realm_document(&key).await? else {
                continue;
            };
            let Some(stored) = readable(&key, &document) else {
                continue;
            };
            if stored.addressee != addressee || stored.dismissed_at_ms.is_some() {
                continue;
            }
            if let Some(item) = stored.shown(&mut serving).await {
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Dismiss the item `item_id` of the person whose private-owner key is
    /// `addressee`: `true` when that person's item is dismissed (now or
    /// before), `false` when the person has no such item. The record says so
    /// first, then the item's listing entry is removed: a dismissed item is
    /// no longer listed or read by a list, and stays stored, so its delivery
    /// stays answered.
    pub(crate) async fn dismiss(
        &self,
        addressee: &str,
        item_id: &str,
    ) -> Result<bool, MetadataStoreError> {
        let _writes = self.writes.lock().await;
        let Some(mut stored) = self.stored(addressee, item_id).await? else {
            return Ok(false);
        };
        if stored.dismissed_at_ms.is_none() {
            stored.dismissed_at_ms = Some(now_ms());
            let key = record_key(addressee, item_id);
            self.store
                .set_realm_document(&key, Some(&encode(&key, &stored)?))
                .await?;
        }
        self.store
            .set_realm_document(&listing_key(addressee, stored.created_at_ms, item_id), None)
            .await?;
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

/// A request's recorded thread and the person its replies are for: what one
/// delivery reads once.
struct AddressedThread {
    thread: RecordedThread,
    /// The person's private-owner key.
    addressee: String,
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
fn attention_receipt(item: &StoredAttentionItem) -> ConsoleReplyReceipt {
    ConsoleReplyReceipt {
        delivered_at_ms: item.created_at_ms,
        delivery: ConsoleReplyDelivery::Attention {
            item_id: item.id.clone(),
        },
    }
}

/// The text a reply's frame was delivered with, from its status and
/// payload; `None` when a host's payload policy redacted the frame before it
/// was stored, so the text can no longer be compared.
fn delivered_text(status: ConsoleFrameStatus, payload: &Value) -> Option<String> {
    if status == ConsoleFrameStatus::Redacted {
        return None;
    }
    Some(
        payload
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

/// The answer to a delivery of `text` under a reply id delivered before as
/// `receipt` with `delivered`: that receipt when the text is the same or can
/// no longer be compared, else a conflict.
fn answered(
    receipt: ConsoleReplyReceipt,
    delivered: Option<String>,
    text: &str,
) -> Result<ConsoleReplyReceipt, ConsoleReplyError> {
    if delivered.is_none_or(|delivered| delivered == text) {
        Ok(receipt)
    } else {
        Err(ConsoleReplyError::Conflict)
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

    /// How long the attention list keeps its items, and with them the
    /// receipts of the replies delivered as items; `None` when the console
    /// keeps no attention list. A list kept for the process (a runtime whose
    /// metadata store lasts for the process only, as on a gateway launch
    /// without persistent state) forgets its deliveries at a restart: a route
    /// that must stay exactly-once across restarts holds no reply there.
    pub(crate) fn attention_retention(&self) -> Option<RecordRetention> {
        self.attention
            .as_ref()
            .map(|attention| attention.retention())
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

    /// The request's recorded thread, with the person its replies are for:
    /// its send record is read once.
    async fn addressed_thread(
        &self,
        interaction_id: &str,
    ) -> Result<AddressedThread, ConsoleReplyError> {
        let thread = self
            .aggregator
            .recorded_thread(interaction_id)
            .await
            .map_err(store_error)?
            .ok_or(ConsoleReplyError::UnknownThread)?;
        let addressee = thread
            .reference
            .addressee
            .clone()
            .ok_or(ConsoleReplyError::NoAddressee)?;
        Ok(AddressedThread { thread, addressee })
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
            Ok(addressed) => Ok(addressed.thread.reference.audience
                == ConsoleThreadAudience::Person
                || self.attention.is_some()),
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
            let delivered = delivered_text(frame.status, &frame.payload);
            return Ok(Some((thread_receipt(&frame), delivered)));
        }
        let Some(attention) = &self.attention else {
            return Ok(None);
        };
        let item_id = private_reply_item_id(&reference.interaction_id, reply_id);
        Ok(attention
            .item(addressee, &item_id)
            .await
            .map_err(store_error)?
            .map(|item| (attention_receipt(&item), item.delivered_text())))
    }

    /// Deliver `text` as the reply `reply_id` to the request with interaction
    /// id `interaction_id`, to the person who made it only: appended to their
    /// private thread when the request was made there and the thread is
    /// still theirs, else as an item of their attention list. The same reply
    /// again answers the first delivery's receipt; other text under the same
    /// reply id is refused, unless a host's payload policy redacted the
    /// reply's frame, whose text can no longer be compared. The request's
    /// send record is read once.
    pub(crate) async fn deliver(
        &self,
        interaction_id: &str,
        reply_id: &str,
        text: &str,
    ) -> Result<ConsoleReplyReceipt, ConsoleReplyError> {
        let AddressedThread { thread, addressee } = self.addressed_thread(interaction_id).await?;
        if let Some((receipt, delivered)) =
            self.find(&thread.reference, &addressee, reply_id).await?
        {
            return answered(receipt, delivered, text);
        }
        if thread.reference.audience == ConsoleThreadAudience::Person {
            match self
                .aggregator
                .append_private_reply(&thread, reply_id, text)
                .await
                .map_err(store_error)?
            {
                PrivateReplyAppend::Appended(frame) | PrivateReplyAppend::Existing(frame) => {
                    let delivered = delivered_text(frame.status, &frame.payload);
                    return answered(thread_receipt(&frame), delivered, text);
                }
                PrivateReplyAppend::ThreadGone => {}
            }
        }
        let attention = self
            .attention
            .as_ref()
            .ok_or(ConsoleReplyError::AttentionUnavailable)?;
        // Kept as the reply's frame would be stored: a host's payload policy
        // that rewrites the frame rewrites the item, and each read shows it
        // as the reading console shows that frame.
        let frame = self
            .aggregator
            .kept_private_reply(&thread, reply_id, text)
            .map_err(store_error)?;
        let content = StoredAttentionContent::PrivateReply {
            frame,
            origin: ConsoleReplyOrigin {
                identity: thread.reference.identity.clone(),
                interaction_id: thread.reference.interaction_id.clone(),
                input_frame_id: thread.reference.input_frame_id.clone(),
            },
        };
        let item_id = private_reply_item_id(&thread.reference.interaction_id, reply_id);
        match attention
            .deliver(&addressee, &item_id, content)
            .await
            .map_err(store_error)?
        {
            ItemDelivery::Stored(item) => Ok(attention_receipt(&item)),
            ItemDelivery::Existing(item) => {
                answered(attention_receipt(&item), item.delivered_text(), text)
            }
        }
    }

    /// The receipt of the reply `reply_id` to the request with interaction
    /// id `interaction_id`, if it was delivered: what a host's committed
    /// check reads. `None` also when the console no longer knows the request
    /// or its delivery. A console whose console log and metadata store last
    /// for the process only (a gateway launch without persistent state)
    /// forgets both at a restart: every earlier reply then reads as not
    /// delivered, and a delivery of it is refused
    /// ([`ConsoleReplyError::UnknownThread`]), so no reply reaches its person
    /// twice. [`Self::attention_retention`] says how long the attention list
    /// keeps its items.
    pub(crate) async fn delivered(
        &self,
        interaction_id: &str,
        reply_id: &str,
    ) -> Result<Option<ConsoleReplyReceipt>, ConsoleReplyError> {
        let AddressedThread { thread, addressee } =
            match self.addressed_thread(interaction_id).await {
                Ok(addressed) => addressed,
                Err(ConsoleReplyError::UnknownThread | ConsoleReplyError::NoAddressee) => {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
        Ok(self
            .find(&thread.reference, &addressee, reply_id)
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
