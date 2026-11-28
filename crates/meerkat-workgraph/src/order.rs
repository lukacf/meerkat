//! Native work orders: the record, its vocabularies, the bound actor and the
//! input digest.
//!
//! A work order is a versioned agreement between two parties about one work
//! item: an *offerer* offers, the *intended acceptor* accepts or declines, and
//! the acceptor closes the order with evidence. An order is a record of its
//! own beside the [`WorkItem`](crate::WorkItem): it adds no value to any
//! existing WorkGraph vocabulary and no field to the item.
//!
//! This module holds types only. It has no store, no machine and no service;
//! those arrive in later changes and build on the names here. Until the order
//! service exists the module is crate-private and nothing in it is exported
//! from the crate, so no release can publish a surface without behaviour, and
//! its names and vocabularies are not yet frozen.
//!
//! # What the host binds, and what an agent can never supply
//!
//! * **The acting party.** Every order input is made by a [`WorkActor`]. The
//!   host builds it from facts it already holds (a member's host-set binding
//!   and the seat that member is built in, an authenticated principal, a
//!   declared exchange) and binds it on the service. `WorkActor` has no
//!   `Deserialize`, no `Serialize` and no JSON schema, so no tool argument,
//!   wire shape or stored record can produce one. What a record keeps of an
//!   actor is a [`WorkActorRecord`], which is provenance and grants nothing.
//! * **The seat a local party is pinned to.** A request names an intended
//!   acceptor by its owner key only. The commit pins its [`WorkMemberSeat`]:
//!   the incarnation of the mob and the member's own incarnation within it.
//!   A member of the same name in another seat (the mob created again, or
//!   the member reset, respawned or spawned again) is another party, and is
//!   neither the intended acceptor nor the holder.
//! * **The authority a decision stood on.** [`WorkAuthorityBasis`] is stamped
//!   by the host-side commit. A request type has no field for it.
//! * **The input key of an agent's call.** A [`WorkAdmissionKey`] for an
//!   agent's tool call comes from the runtime's own identity of that call,
//!   never from the call's arguments.
//!
//! An agent supplies only *content*: which item, to whom, on what terms, with
//! which evidence. The intended acceptor it names designates a target and
//! grants nothing; acceptance is checked against the bound actor.
//!
//! # `#[non_exhaustive]` in this module
//!
//! Every public enum and struct here is `#[non_exhaustive]`, and so is every
//! struct-like variant of those enums, so that neither a later value nor a
//! later field is a source break for a caller. The reason is this crate's
//! own history: `WorkStatus` and `WorkGraphEventKind` were published
//! exhaustive, and adding a value to either is now a break for every
//! consumer. Records are read through their public fields; requests and the
//! other caller-built types have constructors.

use std::collections::BTreeSet;
use std::fmt;

use chrono::{DateTime, Timelike, Utc};
use meerkat_core::service::MobIncarnationId;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::WorkGraphError;
use crate::types::{
    MAX_WORK_CLAIM_LEASE_SECONDS, WorkAdmission, WorkAdmissionKey, WorkItemId, WorkNamespace,
    WorkOwnerKey, WorkOwnerKind, WorkStatus,
};

/// The order contract version this release speaks and writes.
///
/// Every order records the protocol it was offered under. An acceptor that
/// does not speak an offered protocol declines with
/// [`WorkOrderReasonCode::ProtocolUnsupported`]; that decline path is how two
/// parties on different releases stay correct.
pub const WORK_ORDER_PROTOCOL: u32 = 1;

/// Domain string of the order input digest. It is hashed first, followed by
/// one zero byte and the canonical body; see
/// [`OfferWorkOrderRequest::input_digest`].
pub const WORK_ORDER_INPUT_DIGEST_DOMAIN: &str = "meerkat-workgraph.order-input.v1";

/// The form of the order input digest, written at the front of every digest
/// this module makes (`v1:sha256:<hex>`).
///
/// A stored digest therefore says which form made it. A later release that
/// changes what is hashed gives its form another name and keeps this one:
/// to classify a replay it computes the incoming request under the form the
/// STORED digest names, so a digest recorded by one release still classifies
/// a replay under the next.
pub const WORK_ORDER_INPUT_DIGEST_FORM: &str = "v1";

/// A form of the order input digest that this build can compute.
///
/// The form is the first segment of a digest's text. A stored digest is
/// never compared as bare text: it is read with [`Self::of_stored`], which
/// names its form or refuses it, and the incoming request is then hashed in
/// that form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkOrderDigestForm {
    /// SHA-256 over the domain string, one zero byte and the canonical body
    /// that [`OfferWorkOrderRequest::input_digest`] describes.
    V1,
}

impl WorkOrderDigestForm {
    /// The form this build writes.
    pub const CURRENT: Self = Self::V1;

    /// The form's name, as it stands at the front of a digest.
    pub const fn name(self) -> &'static str {
        match self {
            Self::V1 => WORK_ORDER_INPUT_DIGEST_FORM,
        }
    }

    /// Read the form of a stored digest.
    ///
    /// A digest is three parts, `<form>:<hash name>:<digits>`, and the form's
    /// name is read first. A name, the form's and the hash's alike, is one to
    /// 16 characters, each a lowercase letter or a digit: no hyphen and no
    /// other character. Every later form keeps the three parts and that
    /// limit, since this build reads a text outside them as malformed and
    /// not as a form it does not know. This build writes
    /// `<form>:sha256:<64 lowercase hexadecimal digits>`, and under a form it
    /// knows the text must be exactly that. A form this build does not know
    /// is [`WorkOrderError::DigestFormUnknown`], whatever its hash and its
    /// digits are: the record was written by a release that hashes
    /// differently, and this build can call a request under that key neither
    /// a replay nor a conflict. Text with no form, or not of those three
    /// parts at all, is [`WorkOrderError::DigestMalformed`].
    pub fn of_stored(stored: &str) -> Result<Self, WorkOrderError> {
        let malformed = || WorkOrderError::DigestMalformed;
        // A name is a short run of lowercase letters and digits; anything
        // else in its place is not a form this or any release wrote.
        let is_name = |name: &str| {
            !name.is_empty()
                && name.len() <= 16
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        };
        let (form, rest) = stored.split_once(':').ok_or_else(malformed)?;
        let (hash, digits) = rest.split_once(':').ok_or_else(malformed)?;
        if !is_name(form) || !is_name(hash) || digits.is_empty() {
            return Err(malformed());
        }
        if form != Self::V1.name() {
            return Err(WorkOrderError::DigestFormUnknown {
                form: form.to_string(),
            });
        }
        let well_formed = hash == "sha256"
            && digits.len() == 64
            && digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if well_formed {
            Ok(Self::V1)
        } else {
            Err(malformed())
        }
    }
}

/// Maximum byte length of a note on order terms or on a reason.
pub const WORK_ORDER_NOTE_MAX_BYTES: usize = 4096;

/// Maximum byte length of a proposed item's title.
pub const WORK_ORDER_TITLE_MAX_BYTES: usize = 512;

/// Maximum byte length of a proposed item's description.
pub const WORK_ORDER_DESCRIPTION_MAX_BYTES: usize = 16 * 1024;

/// Maximum byte length of an evidence id, an evidence kind or an evidence
/// digest named by a closure.
pub const WORK_ORDER_EVIDENCE_TEXT_MAX_BYTES: usize = 1024;

/// Maximum byte length of a [`WorkOrderId`], a [`WorkExchangeId`], a
/// [`WorkCounterpartPartyRef`] or a [`WorkCounterpartOrderRef`].
pub const WORK_ORDER_REF_MAX_BYTES: usize = 256;

const fn any_char(_: char) -> bool {
    true
}

const fn exchange_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// Canonical text: non-empty, no surrounding whitespace, no control
/// character, at most `max_bytes`, every character admitted by `admits`.
/// Rejected, never trimmed, so two caller strings cannot collapse onto one
/// identity.
fn canonical_text(
    label: &str,
    value: String,
    max_bytes: usize,
    admits: fn(char) -> bool,
) -> Result<String, WorkGraphError> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > max_bytes
        || value.chars().any(|c| c.is_control() || !admits(c))
    {
        return Err(WorkGraphError::InvalidInput(format!(
            "{label} must be canonical non-empty text of at most {max_bytes} bytes"
        )));
    }
    Ok(value)
}

/// Free text a caller may leave out: when present it is not empty and holds
/// at most `max_bytes`. An empty text is refused, not read as absent, so an
/// absent text and a present one never have one meaning and two digests.
fn optional_text(label: &str, text: Option<&str>, max_bytes: usize) -> Result<(), WorkGraphError> {
    match text {
        Some(text) if text.is_empty() || text.len() > max_bytes => {
            Err(WorkGraphError::InvalidInput(format!(
                "{label} must be left out or hold between 1 and {max_bytes} bytes"
            )))
        }
        _ => Ok(()),
    }
}

/// Text a caller must give: not blank, at most `max_bytes`.
fn required_text(label: &str, text: &str, max_bytes: usize) -> Result<(), WorkGraphError> {
    if text.trim().is_empty() || text.len() > max_bytes {
        return Err(WorkGraphError::InvalidInput(format!(
            "{label} must not be blank and holds at most {max_bytes} bytes"
        )));
    }
    Ok(())
}

macro_rules! canonical_text_newtype {
    ($(#[$meta:meta])* $name:ident, $label:literal, $admits:expr) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(try_from = "String", into = "String")]
        #[non_exhaustive]
        pub struct $name(String);

        impl $name {
            /// Validate and wrap `value`. The text must already be canonical:
            /// it is refused, never trimmed or rewritten.
            pub fn new(value: impl Into<String>) -> Result<Self, WorkGraphError> {
                canonical_text($label, value.into(), WORK_ORDER_REF_MAX_BYTES, $admits).map(Self)
            }

            /// The validated text.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = WorkGraphError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

canonical_text_newtype!(
    /// Identity of one work order within its realm and namespace.
    ///
    /// Minted by the service with [`WorkOrderId::generated`] when an offer
    /// commits. A caller never chooses it: an offer request has no id field,
    /// and a later input names the id the offer's receipt returned.
    WorkOrderId,
    "work order id",
    any_char
);

impl WorkOrderId {
    /// Mint a fresh id (`order_` followed by a time-ordered UUID).
    pub fn generated() -> Self {
        Self(format!("order_{}", Uuid::now_v7()))
    }
}

canonical_text_newtype!(
    /// Name of one host-declared exchange between two mobs.
    ///
    /// The host declares exchanges; this id only selects a declared row and
    /// grants nothing by itself. An agent may name one; whether the bound
    /// actor may use it is the host's check. Limited to ASCII letters,
    /// digits, `.`, `_` and `-`, so a party's canonical key stays
    /// unambiguous.
    WorkExchangeId,
    "work exchange id",
    exchange_id_char
);

canonical_text_newtype!(
    /// The other side's party in a declared exchange, as an opaque reference.
    ///
    /// Core never resolves it, never treats it as a local owner and never
    /// derives a permission from it. The host supplies it from its
    /// declaration.
    WorkCounterpartPartyRef,
    "work counterpart party reference",
    any_char
);

canonical_text_newtype!(
    /// The other side's order in a declared exchange, as an opaque reference.
    ///
    /// It names a record in another mob's graph. Core stores it for lookup
    /// and never reads that graph.
    WorkCounterpartOrderRef,
    "work counterpart order reference",
    any_char
);

/// The seat a local mob member is pinned to: one incarnation of the member
/// inside one incarnation of its mob.
///
/// Both values are minted by the mob runtime and set by the host on the
/// member's build; neither is ever taken from a request. `mob_incarnation`
/// changes when the mob is created again under the same id.
/// `member_generation` is the member's own incarnation counter within that
/// mob incarnation: a restart and a wake keep it, as does the replacement of
/// a placed member's carrier binding on its host; a reset or respawn of the
/// member, and a later member spawned under the same name, get the next one,
/// and no value repeats within a mob incarnation.
///
/// Two seats of the same owner key are different parties: a member of the
/// same name in another seat is neither the intended acceptor of an order
/// pinned to this one nor its holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkMemberSeat {
    /// The incarnation of the mob.
    #[cfg_attr(feature = "schema", schemars(with = "String"))]
    pub mob_incarnation: MobIncarnationId,
    /// The member's own incarnation within that mob incarnation.
    pub member_generation: u64,
}

impl WorkMemberSeat {
    /// The seat of generation `member_generation` in `mob_incarnation`.
    pub const fn new(mob_incarnation: MobIncarnationId, member_generation: u64) -> Self {
        Self {
            mob_incarnation,
            member_generation,
        }
    }
}

/// How a request names a party: by what the caller knows, with nothing the
/// host binds.
///
/// A designation selects a target and grants nothing. It has no seat and no
/// declaration generation: the commit pins those from host facts when it
/// makes the [`WorkOrderParty`] of the record, so neither appears in any
/// request shape and a caller cannot supply one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum WorkOrderDesignation {
    /// An owner in the order's own realm and namespace, by its owner key.
    #[non_exhaustive]
    Local {
        /// The owner key.
        key: WorkOwnerKey,
    },
    /// The declared other side of a host-declared exchange.
    #[non_exhaustive]
    Counterpart {
        /// The exchange. The host checks that the bound actor may use it.
        exchange: WorkExchangeId,
        /// The other side's party, opaque to Core.
        party_ref: WorkCounterpartPartyRef,
    },
}

impl WorkOrderDesignation {
    /// A local owner, by its key.
    pub const fn local(key: WorkOwnerKey) -> Self {
        Self::Local { key }
    }

    /// The declared counterpart of an exchange.
    pub const fn counterpart(exchange: WorkExchangeId, party_ref: WorkCounterpartPartyRef) -> Self {
        Self::Counterpart {
            exchange,
            party_ref,
        }
    }

    /// Whether this names an owner of this graph.
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// One unambiguous text key: `local:<kind>:<id>` or
    /// `counterpart:<exchange>:<party_ref>`. It is what the input digest
    /// hashes for a designated acceptor. It never equals the key of a pinned
    /// [`WorkOrderParty`], whose first segment holds an `@`.
    pub fn canonical(&self) -> String {
        match self {
            Self::Local { key } => format!("local:{}", key.canonical()),
            Self::Counterpart {
                exchange,
                party_ref,
            } => format!("counterpart:{exchange}:{party_ref}"),
        }
    }
}

/// What the host answered when the commit asked who stands behind a
/// designation.
///
/// The lookup itself is the host's: WorkGraph holds no roster and reads no
/// declaration. This is only the answer's shape, so that the pin is one
/// function with one outcome per answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkOrderPinFact {
    /// For a local owner key: the seat that now stands for it, or `None`
    /// when no seat stands (a member that is not seated, and every owner
    /// that is not a mob member).
    LocalSeat(Option<WorkMemberSeat>),
    /// For an exchange: the generation of its current declaration, or
    /// `None` when no declaration stands for the bound actor.
    ExchangeGeneration(Option<u64>),
}

impl WorkOrderDesignation {
    /// Pin this designation to what the host answered, making the party a
    /// record holds.
    ///
    /// A mob member with no standing seat is refused
    /// ([`WorkOrderRefusal::AcceptorNotSeated`]); an exchange with no
    /// standing declaration is refused
    /// ([`WorkOrderRefusal::AuthorityNotLive`]). An owner that is not a mob
    /// member is pinned with no seat. An answer of the wrong kind for the
    /// designation, or a seat for an owner that is not a mob member, is
    /// invalid input: the host asked the wrong question.
    pub fn pin(&self, fact: WorkOrderPinFact) -> Result<WorkOrderParty, WorkOrderError> {
        match (self, fact) {
            (Self::Local { key }, WorkOrderPinFact::LocalSeat(seat)) => {
                match (key.as_mob_agent().is_some(), seat) {
                    (true, Some(seat)) => Ok(WorkOrderParty::mob_member(key.clone(), seat)?),
                    (true, None) => Err(WorkOrderRefusal::AcceptorNotSeated.into()),
                    (false, None) => Ok(WorkOrderParty::local(key.clone())?),
                    (false, Some(_)) => Err(WorkGraphError::InvalidInput(
                        "a seat is pinned only on a mob member".to_string(),
                    )
                    .into()),
                }
            }
            (
                Self::Counterpart {
                    exchange,
                    party_ref,
                },
                WorkOrderPinFact::ExchangeGeneration(generation),
            ) => match generation {
                Some(generation) => Ok(WorkOrderParty::counterpart(
                    exchange.clone(),
                    generation,
                    party_ref.clone(),
                )),
                None => Err(WorkOrderRefusal::AuthorityNotLive.into()),
            },
            _ => Err(WorkGraphError::InvalidInput(
                "the host's answer is not about this kind of designation".to_string(),
            )
            .into()),
        }
    }
}

/// One side of an order, as a record holds it.
///
/// A local mob member is pinned to the seat the commit bound it in; a
/// counterpart is pinned to the generation of the host declaration it was
/// admitted under. A request never carries a party: it carries a
/// [`WorkOrderDesignation`].
///
/// A stored party is decoded through the same checks its constructors make,
/// so a record cannot hold what no constructor builds: a mob member with no
/// seat, or a seat on another kind of owner. The decode passes over a key it
/// does not know at the party's own level, so a record a later release wrote
/// still reads here; a designation, which comes from a request, refuses one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    try_from = "WorkOrderPartyRepr"
)]
#[non_exhaustive]
pub enum WorkOrderParty {
    /// An owner in this order's own realm and namespace.
    #[non_exhaustive]
    Local {
        /// The owner key. For a mob member this is the one shape a mob
        /// runtime lowers a member to ([`WorkOwnerKey::mob_agent`]).
        key: WorkOwnerKey,
        /// The seat of a mob member, pinned by the commit; `None` for an
        /// owner that is not a mob member.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seat: Option<WorkMemberSeat>,
    },
    /// The declared other side of a host-declared exchange. It never holds a
    /// claim, a lease or attention in this graph.
    #[non_exhaustive]
    Counterpart {
        /// The host-declared exchange this party belongs to.
        exchange: WorkExchangeId,
        /// The generation of that declaration the party was admitted under:
        /// the host's revision at which the exchange was last declared or
        /// changed. An exchange declared again under the same id is another
        /// generation, and so another party.
        generation: u64,
        /// The other side's party, opaque to Core.
        party_ref: WorkCounterpartPartyRef,
    },
}

/// The stored form of a party before its shape is checked; see
/// [`WorkOrderParty`].
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WorkOrderPartyRepr {
    Local {
        key: WorkOwnerKey,
        #[serde(default)]
        seat: Option<WorkMemberSeat>,
    },
    Counterpart {
        exchange: WorkExchangeId,
        generation: u64,
        party_ref: WorkCounterpartPartyRef,
    },
}

impl TryFrom<WorkOrderPartyRepr> for WorkOrderParty {
    type Error = WorkGraphError;

    fn try_from(repr: WorkOrderPartyRepr) -> Result<Self, Self::Error> {
        match repr {
            WorkOrderPartyRepr::Local {
                key,
                seat: Some(seat),
            } => Self::mob_member(key, seat),
            WorkOrderPartyRepr::Local { key, seat: None } => Self::local(key),
            WorkOrderPartyRepr::Counterpart {
                exchange,
                generation,
                party_ref,
            } => Ok(Self::counterpart(exchange, generation, party_ref)),
        }
    }
}

impl WorkOrderParty {
    /// A local owner that is not a mob member (a principal), by its key.
    ///
    /// A key of the mob-member shape is refused: a mob member is a party
    /// only with its seat ([`Self::mob_member`]), so an unseated member can
    /// be neither built nor decoded.
    pub fn local(key: WorkOwnerKey) -> Result<Self, WorkGraphError> {
        if key.as_mob_agent().is_some() {
            return Err(WorkGraphError::InvalidInput(
                "a mob member is a party only with its seat; an unseated member holds no order"
                    .to_string(),
            ));
        }
        Ok(Self::Local { key, seat: None })
    }

    /// A mob member pinned to its seat. `key` must have the mob-member shape
    /// ([`WorkOwnerKey::mob_agent`]): a seat is never pinned on another kind
    /// of owner.
    pub fn mob_member(key: WorkOwnerKey, seat: WorkMemberSeat) -> Result<Self, WorkGraphError> {
        if key.as_mob_agent().is_none() {
            return Err(WorkGraphError::InvalidInput(
                "a mob member needs a mob-member owner key (kind agent, id mob/<mob_id>/agent/<identity>)"
                    .to_string(),
            ));
        }
        Ok(Self::Local {
            key,
            seat: Some(seat),
        })
    }

    /// The declared counterpart of an exchange, at one generation of its
    /// declaration.
    pub const fn counterpart(
        exchange: WorkExchangeId,
        generation: u64,
        party_ref: WorkCounterpartPartyRef,
    ) -> Self {
        Self::Counterpart {
            exchange,
            generation,
            party_ref,
        }
    }

    /// The local owner key, `None` for a counterpart.
    pub const fn owner_key(&self) -> Option<&WorkOwnerKey> {
        match self {
            Self::Local { key, .. } => Some(key),
            Self::Counterpart { .. } => None,
        }
    }

    /// The pinned seat of a local mob member, if any.
    pub const fn seat(&self) -> Option<WorkMemberSeat> {
        match self {
            Self::Local { seat, .. } => *seat,
            Self::Counterpart { .. } => None,
        }
    }

    /// Whether this party is an owner of this graph.
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// How a request would name this party: its key or exchange, without
    /// the seat or generation the commit pinned.
    pub fn designation(&self) -> WorkOrderDesignation {
        match self {
            Self::Local { key, .. } => WorkOrderDesignation::Local { key: key.clone() },
            Self::Counterpart {
                exchange,
                party_ref,
                ..
            } => WorkOrderDesignation::Counterpart {
                exchange: exchange.clone(),
                party_ref: party_ref.clone(),
            },
        }
    }

    /// One unambiguous text key for this party:
    /// `local@<mob incarnation>.<member generation>:<kind>:<id>` for a
    /// seated mob member, `local@-:<kind>:<id>` for a local owner with no
    /// seat, `counterpart@<generation>:<exchange>:<party_ref>` for a
    /// counterpart. The segment before the first `:` holds no `:`, an owner
    /// kind and an exchange id hold none either, so distinct parties never
    /// share a key, and no party shares one with a
    /// [`WorkOrderDesignation`]. The input digest hashes the bound actor by
    /// this key and the order machine compares parties by it, so the same
    /// member name in another seat is another party.
    pub fn canonical(&self) -> String {
        match self {
            Self::Local { key, seat: None } => format!("local@-:{}", key.canonical()),
            Self::Local {
                key,
                seat: Some(seat),
            } => format!(
                "local@{}.{}:{}",
                seat.mob_incarnation,
                seat.member_generation,
                key.canonical()
            ),
            Self::Counterpart {
                exchange,
                generation,
                party_ref,
            } => format!("counterpart@{generation}:{exchange}:{party_ref}"),
        }
    }
}

/// How the host established who an actor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkActorSource {
    /// A mob member, from the member binding and the seat the host set on
    /// its build.
    MobMember,
    /// A principal the host authenticated on one of its own doors.
    HostPrincipal,
    /// The declared other side of an exchange, from the host's declaration
    /// and the carrier that delivered its input.
    Counterpart,
}

impl WorkActorSource {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MobMember => "mob_member",
            Self::HostPrincipal => "host_principal",
            Self::Counterpart => "counterpart",
        }
    }
}

/// The authenticated party making an order input.
///
/// **Host-bound, never agent-supplied.** The host builds a `WorkActor` from a
/// fact it already holds and binds it on the service that a surface uses. It
/// has no `Deserialize`, no `Serialize` and no JSON schema (compile-time
/// assertions below this type keep it so): no tool argument, wire request or
/// stored context can produce one, and a recorded [`WorkActorRecord`] cannot
/// be turned back into one.
///
/// Constructing one in process is host authority, like holding the service
/// itself. Each constructor checks the shape its source implies, so a caller
/// cannot label an arbitrary name as a mob member or a principal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkActor {
    party: WorkOrderParty,
    source: WorkActorSource,
}

impl WorkActor {
    /// A mob member in one seat: one incarnation of the member inside one
    /// incarnation of its mob.
    ///
    /// `key` must have the mob-member shape ([`WorkOwnerKey::mob_agent`]);
    /// the host lowers it from the member binding it set on the member's
    /// build. `seat` is the seat that build was made for, from the mob
    /// runtime's own facts. A member of the same name in another seat (the
    /// mob created again, or the member reset, respawned or spawned again)
    /// is another actor.
    pub fn mob_member(key: WorkOwnerKey, seat: WorkMemberSeat) -> Result<Self, WorkGraphError> {
        Ok(Self {
            party: WorkOrderParty::mob_member(key, seat)?,
            source: WorkActorSource::MobMember,
        })
    }

    /// A principal the host authenticated. `key` must be of kind
    /// [`WorkOwnerKind::Principal`].
    pub fn host_principal(key: WorkOwnerKey) -> Result<Self, WorkGraphError> {
        if key.kind != WorkOwnerKind::Principal {
            return Err(WorkGraphError::InvalidInput(
                "a host principal actor needs an owner key of kind principal".to_string(),
            ));
        }
        Ok(Self {
            party: WorkOrderParty::local(key)?,
            source: WorkActorSource::HostPrincipal,
        })
    }

    /// The declared counterpart of a host-declared exchange, at the
    /// generation of the declaration the host admitted it under. Host code
    /// builds it for the caller it authenticated; it is never derived from
    /// delivered content.
    pub const fn counterpart(
        exchange: WorkExchangeId,
        generation: u64,
        party_ref: WorkCounterpartPartyRef,
    ) -> Self {
        Self {
            party: WorkOrderParty::counterpart(exchange, generation, party_ref),
            source: WorkActorSource::Counterpart,
        }
    }

    /// The party this actor is, pinned to its seat or to its declaration's
    /// generation.
    pub const fn party(&self) -> &WorkOrderParty {
        &self.party
    }

    /// How the host established it.
    pub const fn source(&self) -> WorkActorSource {
        self.source
    }

    /// The local owner key, `None` for a counterpart.
    pub const fn owner_key(&self) -> Option<&WorkOwnerKey> {
        self.party.owner_key()
    }

    /// The provenance form a decision record keeps. It can be stored and
    /// read; it cannot be turned back into a `WorkActor`.
    pub fn record(&self) -> WorkActorRecord {
        WorkActorRecord {
            party: self.party.clone(),
            source: self.source,
        }
    }
}

/// Compile-time proof that a type does NOT implement a trait: if it did, the
/// call below would have two candidate impls and fail to compile.
macro_rules! assert_not_impl {
    ($ty:ty, $($bound:tt)+) => {
        const _: fn() = || {
            trait AmbiguousIfImpl<A> {
                fn witness() {}
            }
            impl<T: ?Sized> AmbiguousIfImpl<()> for T {}
            #[allow(dead_code)]
            struct Implemented;
            impl<T: ?Sized + $($bound)+> AmbiguousIfImpl<Implemented> for T {}
            let _ = <$ty as AmbiguousIfImpl<_>>::witness;
        };
    };
}

// A `WorkActor` has no wire form of any kind. These hold in every build of
// the crate, not only in its tests.
assert_not_impl!(WorkActor, serde::de::DeserializeOwned);
assert_not_impl!(WorkActor, serde::Serialize);
#[cfg(feature = "schema")]
assert_not_impl!(WorkActor, schemars::JsonSchema);

/// What a record keeps of the actor that made a decision.
///
/// Provenance only: it grants nothing, and no constructor turns it back into
/// a [`WorkActor`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkActorRecord {
    /// The party that acted, with its pinned seat.
    pub party: WorkOrderParty,
    /// How the host had established it.
    pub source: WorkActorSource,
}

/// The authority a committed decision stood on, stamped by the host-side
/// commit. No request type carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkAuthorityBasis {
    /// The namespace grant the service was built with, and nothing more. It
    /// is a fact fixed when the service was built, not a check made at the
    /// commit.
    #[non_exhaustive]
    NamespaceGrant {
        /// Realm of the grant.
        realm_id: String,
        /// Namespace of the grant.
        namespace: WorkNamespace,
    },
    /// A host fence held across the commit. `reference` is the host's own
    /// name for what it checked (a seat, a grant lineage, a declaration at a
    /// revision); Core records it and never interprets it.
    #[non_exhaustive]
    HostFence {
        /// The host's opaque reference.
        reference: String,
    },
}

/// What an order is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderKind {
    /// The subject is an existing item the offerer holds; accepting moves the
    /// whole item to the acceptor.
    Transfer,
    /// The subject is an item the offer proposes; accepting creates it and
    /// gives the acceptor its claim.
    Assign,
}

impl WorkOrderKind {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::Assign => "assign",
        }
    }
}

/// Lifecycle state of an order.
///
/// The transitions are owned by the order's lifecycle machine; this is the
/// projected vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderState {
    /// Offered; the intended acceptor has not decided.
    Offered,
    /// Accepted; the acceptor holds the subject until closure.
    Accepted,
    /// The intended acceptor declined. Terminal.
    Declined,
    /// The offerer withdrew before a decision. Terminal.
    Withdrawn,
    /// The offerer replaced it with a newer order before a decision.
    /// Terminal.
    Superseded,
    /// The offer ran out before a decision. Terminal.
    Expired,
    /// Closed by the acceptor, or handed on. Terminal.
    Closed,
    /// The holder's hold on the subject ended without closure. Terminal.
    Lapsed,
}

impl WorkOrderState {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Closed => "closed",
            Self::Lapsed => "lapsed",
        }
    }

    /// Offered or accepted: the order still binds its parties.
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Offered | Self::Accepted)
    }

    /// No further input changes the order.
    pub const fn is_terminal(self) -> bool {
        !self.is_live()
    }
}

/// How a read classifies an order against the present, without writing.
///
/// A commit that ends a holder's claim also ends its order. A write made by
/// an older release, which knows no orders, does not; until an input
/// observes what such a write did, a read reports it here, so an order is
/// never listed as pending or held when it no longer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderStanding {
    /// The stored state is the present state.
    Current,
    /// Still stored as offered, but the offer has run out.
    OfferExpiredUnobserved,
    /// Still stored as accepted, but the holder's claim on the subject is
    /// gone.
    HolderLapsedUnobserved,
    /// Still stored as accepted, but the subject item was ended outside the
    /// order.
    ItemEndedUnobserved,
}

impl WorkOrderStanding {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::OfferExpiredUnobserved => "offer_expired_unobserved",
            Self::HolderLapsedUnobserved => "holder_lapsed_unobserved",
            Self::ItemEndedUnobserved => "item_ended_unobserved",
        }
    }
}

/// Why an accepted order lapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderLapseCause {
    /// The holder's claim on the subject ended: it released the item, its
    /// lease ran out, or the host released it.
    HolderClaimEnded,
    /// The subject item reached a terminal status outside the order.
    ItemEndedOutsideOrder,
}

impl WorkOrderLapseCause {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HolderClaimEnded => "holder_claim_ended",
            Self::ItemEndedOutsideOrder => "item_ended_outside_order",
        }
    }
}

/// One of the at most three keyed steps of an order's life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderStep {
    /// The offer that creates the order.
    Offer,
    /// The one decision that ends the offered state.
    Decision,
    /// The closure of an accepted order.
    Closure,
}

impl WorkOrderStep {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Decision => "decision",
            Self::Closure => "closure",
        }
    }
}

/// Every input an order's lifecycle takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderInputKind {
    /// The offerer offers.
    Offer,
    /// The intended acceptor accepts.
    Accept,
    /// The intended acceptor declines.
    Decline,
    /// The offerer withdraws before a decision.
    Withdraw,
    /// The offerer replaces a still-offered order with a newer one.
    Supersede,
    /// The holder closes.
    Close,
    /// A successor order on the same item was accepted while this order's
    /// holder still held it. Never requested by a caller.
    HandOver,
    /// A commit observed that the offer had run out.
    Expire,
    /// A commit ended, or observed the end of, the holder's hold on the
    /// subject.
    Lapse,
}

impl WorkOrderInputKind {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offer => "offer",
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Withdraw => "withdraw",
            Self::Supersede => "supersede",
            Self::Close => "close",
            Self::HandOver => "hand_over",
            Self::Expire => "expire",
            Self::Lapse => "lapse",
        }
    }

    /// The step this input takes.
    pub const fn step(self) -> WorkOrderStep {
        match self {
            Self::Offer => WorkOrderStep::Offer,
            Self::Accept | Self::Decline | Self::Withdraw | Self::Supersede | Self::Expire => {
                WorkOrderStep::Decision
            }
            Self::Close | Self::HandOver | Self::Lapse => WorkOrderStep::Closure,
        }
    }

    /// The state the order is in after this input commits.
    pub const fn resulting_state(self) -> WorkOrderState {
        match self {
            Self::Offer => WorkOrderState::Offered,
            Self::Accept => WorkOrderState::Accepted,
            Self::Decline => WorkOrderState::Declined,
            Self::Withdraw => WorkOrderState::Withdrawn,
            Self::Supersede => WorkOrderState::Superseded,
            Self::Close | Self::HandOver => WorkOrderState::Closed,
            Self::Expire => WorkOrderState::Expired,
            Self::Lapse => WorkOrderState::Lapsed,
        }
    }
}

/// How an accepted order ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderCloseOutcome {
    /// The ordered work is done. Requires evidence.
    Completed,
    /// The ordered work failed.
    Failed,
    /// The ordered work was called off.
    Cancelled,
    /// The holder handed the item on under a successor order. Recorded by
    /// that successor's acceptance; a caller cannot request it.
    HandedOver,
}

impl WorkOrderCloseOutcome {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::HandedOver => "handed_over",
        }
    }
}

/// A typed reason code on a decline, a withdrawal or a closure.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderReasonCode {
    /// No typed cause; see the note.
    #[default]
    Unspecified,
    /// The deciding party does not speak the offered order protocol.
    ProtocolUnsupported,
    /// The host declaration that carried this order between two mobs was
    /// withdrawn.
    ExchangeWithdrawn,
}

impl WorkOrderReasonCode {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::ProtocolUnsupported => "protocol_unsupported",
            Self::ExchangeWithdrawn => "exchange_withdrawn",
        }
    }
}

/// Why a party declined, withdrew or closed without success.
///
/// Build one with [`WorkOrderReason::new`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkOrderReason {
    /// The typed cause.
    #[serde(default)]
    pub code: WorkOrderReasonCode,
    /// Free text for the other party. Untrusted content, never an
    /// instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl WorkOrderReason {
    /// A reason with `code` and no note.
    pub const fn new(code: WorkOrderReasonCode) -> Self {
        Self { code, note: None }
    }

    /// The same reason with a note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    fn validate(&self) -> Result<(), WorkGraphError> {
        optional_text(
            "work order reason note",
            self.note.as_deref(),
            WORK_ORDER_NOTE_MAX_BYTES,
        )
    }

    /// No typed cause and no note: a reason that says nothing. Where a
    /// reason may be left out, this one is hashed as left out, so the two
    /// have one digest.
    fn says_nothing(&self) -> bool {
        self.code == WorkOrderReasonCode::Unspecified && self.note.is_none()
    }
}

/// The terms an offer states.
///
/// Start from [`WorkOrderTerms::default`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkOrderTerms {
    /// When the offer runs out. WorkGraph starts no timer: an accept after
    /// this time is refused, and the expiry is committed by the input that
    /// observes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offer_expires_at: Option<DateTime<Utc>>,
    /// The lease the acceptor's claim receives at acceptance, in seconds;
    /// `None` for a claim without a lease. At most
    /// [`MAX_WORK_CLAIM_LEASE_SECONDS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_lease_seconds: Option<u64>,
    /// Free text for the acceptor. Untrusted content, never an instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl WorkOrderTerms {
    /// No expiry, no lease, no note.
    pub const fn is_empty(&self) -> bool {
        self.offer_expires_at.is_none()
            && self.holder_lease_seconds.is_none()
            && self.note.is_none()
    }

    /// The same terms with an offer expiry.
    #[must_use]
    pub fn with_offer_expires_at(mut self, at: DateTime<Utc>) -> Self {
        self.offer_expires_at = Some(at);
        self
    }

    /// The same terms with a holder lease.
    #[must_use]
    pub fn with_holder_lease_seconds(mut self, seconds: u64) -> Self {
        self.holder_lease_seconds = Some(seconds);
        self
    }

    /// The same terms with a note.
    #[must_use]
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }

    fn validate(&self) -> Result<(), WorkGraphError> {
        if let Some(seconds) = self.holder_lease_seconds
            && (seconds == 0 || seconds > MAX_WORK_CLAIM_LEASE_SECONDS)
        {
            return Err(WorkGraphError::InvalidInput(format!(
                "work order holder lease must be between 1 and {MAX_WORK_CLAIM_LEASE_SECONDS} seconds"
            )));
        }
        // The digest hashes the expiry in whole milliseconds. A finer expiry
        // is refused, so two offers that differ never share a digest.
        if let Some(expires_at) = self.offer_expires_at
            && expires_at.nanosecond() % 1_000_000 != 0
        {
            return Err(WorkGraphError::InvalidInput(
                "work order offer expiry must be a whole number of milliseconds".to_string(),
            ));
        }
        // A time written with second 60 keeps its own text and reads as the
        // same instant as the second after it: two expiries, one digest. It
        // is refused; the caller names the next second.
        if let Some(expires_at) = self.offer_expires_at
            && expires_at.nanosecond() >= 1_000_000_000
        {
            return Err(WorkGraphError::InvalidInput(
                "work order offer expiry must not be written as a leap second".to_string(),
            ));
        }
        optional_text(
            "work order terms note",
            self.note.as_deref(),
            WORK_ORDER_NOTE_MAX_BYTES,
        )
    }
}

/// The item an assign offer proposes.
///
/// It is content only. No item exists for it until the offer is accepted:
/// an offer that is declined, withdrawn, superseded or runs out leaves
/// nothing behind in the graph.
///
/// Build one with [`ProposedWorkItem::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ProposedWorkItem {
    /// Title of the item to create.
    pub title: String,
    /// Description of the item to create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl ProposedWorkItem {
    /// A proposed item titled `title`, with no description.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: None,
        }
    }

    /// The same item with a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    fn validate(&self) -> Result<(), WorkGraphError> {
        required_text(
            "a proposed work item title",
            &self.title,
            WORK_ORDER_TITLE_MAX_BYTES,
        )?;
        optional_text(
            "a proposed work item description",
            self.description.as_deref(),
            WORK_ORDER_DESCRIPTION_MAX_BYTES,
        )
    }
}

/// What an order concerns.
///
/// A transfer names an existing item at the revision the offer pins: the
/// acceptor accepts the item as it was at that revision, and if the item has
/// moved the acceptance is refused and the offerer offers again. An assign
/// offer proposes an item; the acceptance creates it, and from then on the
/// order's subject is that item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum WorkOrderSubject {
    /// An item of this graph.
    #[non_exhaustive]
    Item {
        /// The item.
        item_id: WorkItemId,
        /// For an offered transfer, the item revision the offer pins; after
        /// an acceptance, the item's revision as that commit left it.
        item_revision: u64,
    },
    /// An item the acceptance will create.
    Proposed(ProposedWorkItem),
}

impl WorkOrderSubject {
    /// The existing item `item_id` at `item_revision`.
    pub const fn item(item_id: WorkItemId, item_revision: u64) -> Self {
        Self::Item {
            item_id,
            item_revision,
        }
    }

    /// The item, once there is one.
    pub const fn item_id(&self) -> Option<&WorkItemId> {
        match self {
            Self::Item { item_id, .. } => Some(item_id),
            Self::Proposed(_) => None,
        }
    }
}

/// The parent a delegated child order preserves.
///
/// The delegating owner keeps the parent item and the order it holds that
/// item under; neither is changed by anything that happens to the child.
/// `order_id` is provenance: it names the order the parent was held under
/// when the child was delegated, whatever became of that order since.
///
/// Build one with [`WorkOrderParent::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkOrderParent {
    /// The parent item. The offerer must hold it.
    pub item_id: WorkItemId,
    /// The accepted order the offerer held the parent item under when it
    /// delegated, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<WorkOrderId>,
}

impl WorkOrderParent {
    /// The parent item `item_id`, held under no order.
    pub const fn new(item_id: WorkItemId) -> Self {
        Self {
            item_id,
            order_id: None,
        }
    }

    /// The same parent, held under `order_id`.
    #[must_use]
    pub fn with_order(mut self, order_id: WorkOrderId) -> Self {
        self.order_id = Some(order_id);
        self
    }
}

/// One piece of evidence a closure is bound to.
///
/// Which form a closure may carry follows who closes. A holder of this
/// graph names evidence on the subject item, by id. Only the mirrored
/// closure of a counterpart names evidence by digest, because that evidence
/// is in another graph and its content never crosses. A closure that
/// carries the other form is refused
/// ([`WorkOrderInputRequest::validate_for`]).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum WorkOrderEvidenceRef {
    /// Evidence already recorded on the subject item, by its id.
    #[non_exhaustive]
    Item {
        /// Id of a [`WorkEvidenceRef`](crate::WorkEvidenceRef) on the
        /// subject item.
        evidence_id: String,
    },
    /// Evidence held on the other side of an exchange, by digest only. Its
    /// content never crosses.
    #[non_exhaustive]
    CounterpartDigest {
        /// The other side's label for the evidence.
        evidence_kind: String,
        /// Digest of the evidence as the other side reported it.
        digest: String,
    },
}

impl WorkOrderEvidenceRef {
    /// Evidence on the subject item, by its id.
    pub fn item(evidence_id: impl Into<String>) -> Self {
        Self::Item {
            evidence_id: evidence_id.into(),
        }
    }

    /// Evidence held on the other side of an exchange, by its digest.
    pub fn counterpart_digest(evidence_kind: impl Into<String>, digest: impl Into<String>) -> Self {
        Self::CounterpartDigest {
            evidence_kind: evidence_kind.into(),
            digest: digest.into(),
        }
    }

    /// The text that names this evidence in a refusal: the id, or the
    /// digest.
    fn name(&self) -> &str {
        match self {
            Self::Item { evidence_id } => evidence_id,
            Self::CounterpartDigest { digest, .. } => digest,
        }
    }

    fn validate(&self) -> Result<(), WorkGraphError> {
        match self {
            Self::Item { evidence_id } => required_text(
                "a work order evidence id",
                evidence_id,
                WORK_ORDER_EVIDENCE_TEXT_MAX_BYTES,
            ),
            Self::CounterpartDigest {
                evidence_kind,
                digest,
            } => {
                required_text(
                    "a work order evidence kind",
                    evidence_kind,
                    WORK_ORDER_EVIDENCE_TEXT_MAX_BYTES,
                )?;
                required_text(
                    "a work order evidence digest",
                    digest,
                    WORK_ORDER_EVIDENCE_TEXT_MAX_BYTES,
                )
            }
        }
    }
}

/// How an accepted order was closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderClosure {
    /// The outcome.
    pub outcome: WorkOrderCloseOutcome,
    /// The evidence the closure is bound to. Never empty for
    /// [`WorkOrderCloseOutcome::Completed`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<WorkOrderEvidenceRef>,
    /// The order the item was handed on under, for
    /// [`WorkOrderCloseOutcome::HandedOver`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<WorkOrderId>,
}

/// Provenance of one decision on an order.
///
/// Written in the commit that makes the decision and never edited. It is
/// stamped by the commit: `actor` from the bound [`WorkActor`], `authority`
/// from the host, `identity` from the input's key and digest. No request
/// carries any of them.
///
/// The receipt of a keyed input is a function of this record and the order
/// it sits on ([`WorkOrder::receipt`]); nothing else is stored for a replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderDecision {
    /// The step this decision took.
    pub step: WorkOrderStep,
    /// The input that made it.
    pub input: WorkOrderInputKind,
    /// Who made it; `None` when no party did: an expiry, a lapse or a
    /// hand-over that a commit recorded on this order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<WorkActorRecord>,
    /// The authority it stood on.
    pub authority: WorkAuthorityBasis,
    /// The input's key and digest; `None` when no party made it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<WorkAdmission>,
    /// The order's version after this decision.
    pub order_version: u64,
    /// The subject item's revision before the commit, when the commit
    /// touched an existing item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_revision_before: Option<u64>,
    /// The subject item's revision after the commit, when the commit touched
    /// or created the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_revision_after: Option<u64>,
    /// The stated reason, for a decline, a withdrawal or an unsuccessful
    /// closure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<WorkOrderReason>,
    /// Why the order lapsed, for [`WorkOrderInputKind::Lapse`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lapse_cause: Option<WorkOrderLapseCause>,
    /// When the commit made it.
    pub at: DateTime<Utc>,
}

/// One work order.
///
/// Every field is written by the service's commit. A caller's request
/// supplies content only (see the request types); identity, version, state,
/// the parties' authentication and seats, and provenance are never
/// taken from a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrder {
    /// Identity, minted at the offer.
    pub id: WorkOrderId,
    /// Realm the order lives in.
    pub realm_id: String,
    /// Namespace the order lives in. An order and its subject item share one
    /// realm and namespace.
    pub namespace: WorkNamespace,
    /// The order contract version it was offered under.
    pub protocol: u32,
    /// The record's version: 1 at the offer, plus one per committed input.
    /// Every later input states the version it expects.
    pub version: u64,
    /// What the order is about.
    pub kind: WorkOrderKind,
    /// Who offered. Authenticated: it is the bound actor of the offer, with
    /// its pinned seat.
    pub offerer: WorkOrderParty,
    /// The intended acceptor the offerer named, pinned by the commit. Naming
    /// grants nothing: acceptance requires the bound actor to be this party.
    pub acceptor: WorkOrderParty,
    /// What the order concerns.
    pub subject: WorkOrderSubject,
    /// For a delegated child: the parent it preserves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<WorkOrderParent>,
    /// The terms of the offer.
    #[serde(default, skip_serializing_if = "WorkOrderTerms::is_empty")]
    pub terms: WorkOrderTerms,
    /// Lifecycle state.
    pub state: WorkOrderState,
    /// How it was closed, once closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closure: Option<WorkOrderClosure>,
    /// The earlier order this one replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<WorkOrderId>,
    /// The later order that replaced this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<WorkOrderId>,
    /// For the receiving half of an exchange: the other side's order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterpart_order: Option<WorkCounterpartOrderRef>,
    /// Provenance of each decision, in the order made. At most one per
    /// [`WorkOrderStep`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<WorkOrderDecision>,
    /// When the offer committed.
    pub created_at: DateTime<Utc>,
    /// When the last input committed.
    pub updated_at: DateTime<Utc>,
    /// When the order reached a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<DateTime<Utc>>,
}

impl WorkOrder {
    /// The recorded decision of `step`, if that step was taken.
    pub fn decision(&self, step: WorkOrderStep) -> Option<&WorkOrderDecision> {
        self.decisions.iter().find(|decision| decision.step == step)
    }

    /// The party that holds the subject under this order: the acceptor while
    /// the order is accepted, otherwise none.
    pub fn holder(&self) -> Option<&WorkOrderParty> {
        (self.state == WorkOrderState::Accepted).then_some(&self.acceptor)
    }

    /// Offered or accepted.
    pub const fn is_live(&self) -> bool {
        self.state.is_live()
    }

    /// The receipt of the keyed input that took `step`, rebuilt from the
    /// order's own decision record.
    ///
    /// A replay of that input's key with the same content is answered with
    /// exactly this value, whatever the order's state has become since: the
    /// decision record is never edited, so the receipt needs no stored copy.
    /// The receipt of a fresh commit is made by this same function, so the
    /// two cannot differ.
    ///
    /// `None` when the step was not taken, when no party took it, and for
    /// the decision of an order that was replaced: a replacement is one
    /// keyed input on two orders, the new order owns the key, and its offer's
    /// receipt (which names the replaced order) is that input's only
    /// receipt. A conflict on that key likewise names the new order and its
    /// offer step.
    pub fn receipt(&self, step: WorkOrderStep) -> Option<WorkOrderReceipt> {
        let decision = self.decision(step)?;
        // A replacement is one keyed input on two orders. The new order owns
        // the key, and its offer's receipt is that input's one receipt; the
        // replaced order records the same key and has no receipt for it.
        if decision.input == WorkOrderInputKind::Supersede {
            return None;
        }
        let identity = decision.identity.clone()?;
        let item = match (self.subject.item_id(), decision.item_revision_after) {
            (Some(item_id), Some(revision_after)) => Some(WorkOrderItemChange {
                item_id: item_id.clone(),
                revision_before: decision.item_revision_before,
                revision_after,
            }),
            _ => None,
        };
        let supersedes = match step {
            WorkOrderStep::Offer => self.supersedes.clone(),
            _ => None,
        };
        Some(WorkOrderReceipt {
            order_id: self.id.clone(),
            step,
            input: decision.input,
            identity,
            order_version: decision.order_version,
            state: decision.input.resulting_state(),
            item,
            supersedes,
            at: decision.at,
        })
    }
}

/// The record of one whole-item transfer.
///
/// Carried by the `accepted` event of a transfer order. It is the durable
/// account of who held the item before and after, at which revisions, and on
/// what authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderTransfer {
    /// The item that changed hands.
    pub item_id: WorkItemId,
    /// The owner before.
    pub from_owner: WorkOwnerKey,
    /// The owner after: the authenticated acceptor.
    pub to_owner: WorkOwnerKey,
    /// The item's revision before the transfer.
    pub item_revision_before: u64,
    /// The item's revision after the transfer.
    pub item_revision_after: u64,
    /// The order's version before the acceptance.
    pub order_version_before: u64,
    /// The order's version after the acceptance.
    pub order_version_after: u64,
    /// The authority the transfer stood on.
    pub authority: WorkAuthorityBasis,
}

/// What happened, on the order ledger.
///
/// On the wire this vocabulary is declared tolerant from its first release:
/// an SDK reader shows the kinds it knows and skips the rest, so a later
/// kind is not a break for a client. THIS type is closed on purpose: a
/// ledger entry whose kind this build does not know was written by a later
/// build, and reading the ledger must fail on it, never drop it, because a
/// projection rebuilt from a ledger with an entry left out would be wrong
/// without saying so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderEventKind {
    /// An offer committed.
    Offered,
    /// An acceptance committed.
    Accepted,
    /// A decline committed.
    Declined,
    /// A withdrawal committed.
    Withdrawn,
    /// The order was replaced by a newer one.
    Superseded,
    /// The offer's expiry was committed.
    Expired,
    /// The order was closed or handed on.
    Closed,
    /// The holder's lapse was committed.
    Lapsed,
}

impl WorkOrderEventKind {
    /// The stable wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offered => "offered",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::Closed => "closed",
            Self::Lapsed => "lapsed",
        }
    }
}

/// One entry of the order ledger.
///
/// Each entry carries the whole order as the commit left it, so the order
/// projection can be rebuilt from the ledger alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderEvent {
    /// Position in the ledger; assigned by the store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    /// Realm of the order.
    pub realm_id: String,
    /// Namespace of the order.
    pub namespace: WorkNamespace,
    /// What happened.
    pub kind: WorkOrderEventKind,
    /// When the commit made it.
    pub at: DateTime<Utc>,
    /// The order as the commit left it.
    pub order: WorkOrder,
    /// The transfer record, on the `accepted` event of a transfer order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<WorkOrderTransfer>,
}

/// What a committed input did to the subject item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderItemChange {
    /// The item.
    pub item_id: WorkItemId,
    /// Its revision before the commit; `None` when the commit created it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_before: Option<u64>,
    /// Its revision after the commit.
    pub revision_after: u64,
}

/// The receipt of one committed keyed input.
///
/// A replay of the same key and content returns this receipt unchanged,
/// whatever the order's state has become since: it is evidence of what that
/// input did, not a view of the present. It is rebuilt from the order's
/// decision record ([`WorkOrder::receipt`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct WorkOrderReceipt {
    /// The order the input acted on (for an offer, the order it created).
    pub order_id: WorkOrderId,
    /// The step the input took.
    pub step: WorkOrderStep,
    /// The input.
    pub input: WorkOrderInputKind,
    /// The input's key and digest.
    pub identity: WorkAdmission,
    /// The order's version after the commit.
    pub order_version: u64,
    /// The order's state after the commit.
    pub state: WorkOrderState,
    /// What the commit did to the subject item, if anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<WorkOrderItemChange>,
    /// For an offer that replaced an earlier order: that order. One key then
    /// names two orders; the new order owns the key, and its receipt names
    /// the one it replaced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<WorkOrderId>,
    /// When the commit happened.
    pub at: DateTime<Utc>,
}

/// Result of a keyed order input that was not refused.
///
/// The replay verdict has three cases today and a caller should handle
/// each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "outcome", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderOutcome {
    /// The key was new and the input committed.
    #[non_exhaustive]
    Committed {
        /// The receipt of the commit.
        receipt: WorkOrderReceipt,
    },
    /// The key was already recorded with identical content. Nothing was
    /// written; this is the first receipt.
    #[non_exhaustive]
    Replayed {
        /// The receipt of the original commit.
        receipt: WorkOrderReceipt,
    },
    /// The key was already recorded with other content. Nothing was written.
    #[non_exhaustive]
    Conflict {
        /// The key both inputs used.
        input_key: WorkAdmissionKey,
        /// The order that owns the key.
        order_id: WorkOrderId,
        /// The step that owns the key.
        step: WorkOrderStep,
    },
}

impl WorkOrderOutcome {
    /// The receipt for `Committed` and `Replayed`; `None` for a conflict.
    pub const fn receipt(&self) -> Option<&WorkOrderReceipt> {
        match self {
            Self::Committed { receipt } | Self::Replayed { receipt } => Some(receipt),
            Self::Conflict { .. } => None,
        }
    }
}

/// Every refusal code, in declaration order. SDKs and skill text pin
/// against it once the surface is released.
pub const WORK_ORDER_REFUSAL_CODES: &[&str] = &[
    "actor_required",
    "not_intended_acceptor",
    "not_offerer",
    "not_holder",
    "stale_order_version",
    "order_state_mismatch",
    "offer_expired",
    "item_revision_moved",
    "old_owner_mismatch",
    "old_owner_lease_expired",
    "item_not_transferable",
    "acceptor_not_seated",
    "parent_not_held",
    "order_pending",
    "order_held",
    "order_close_required",
    "evidence_required",
    "evidence_unknown",
    "attention_target_occupied",
    "authority_not_live",
    "authority_fence_unsupported",
    "protocol_unsupported",
    "custody_stays_local",
    "counterpart_decides",
    "order_not_found",
];

/// A typed refusal of an order input, or of an item write on an item held
/// under an accepted order. A refused input writes nothing.
///
/// A replayed key with other content is not a refusal: it is
/// [`WorkOrderOutcome::Conflict`].
///
/// On the wire the code set ([`WORK_ORDER_REFUSAL_CODES`]) is meant to be
/// closed at its first release: released SDKs reject a code they do not
/// know, so a code added after that is a breaking change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "code", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderRefusal {
    /// The service is not bound to a [`WorkActor`]. Only the host can bind
    /// one.
    #[error("this operation needs a host-bound actor")]
    ActorRequired,
    /// The bound actor is not the order's intended acceptor. A member of the
    /// same name in another seat is not: neither after its mob was created
    /// again, nor after the member itself was reset, respawned or spawned
    /// again.
    #[error("the bound actor is not the intended acceptor of this order")]
    NotIntendedAcceptor,
    /// The bound actor is not the order's offerer.
    #[error("the bound actor is not the offerer of this order")]
    NotOfferer,
    /// The item is held under an accepted order by another party. This is
    /// what a former owner gets when it writes the item at its current
    /// revision.
    #[error("work order {order_id} gives this item to another holder")]
    #[non_exhaustive]
    NotHolder {
        /// The accepted order the item is held under.
        order_id: WorkOrderId,
        /// The party that holds it.
        holder: WorkOrderParty,
    },
    /// The input expected another version of the order.
    #[error("stale work order version for {order_id}: expected {expected}, current {current}")]
    #[non_exhaustive]
    StaleOrderVersion {
        /// The order.
        order_id: WorkOrderId,
        /// The version the input expected.
        expected: u64,
        /// The order's version now.
        current: u64,
    },
    /// The order is not in a state that takes this input. When the order is
    /// accepted the refusal names its holder, so a caller that repeats an
    /// acceptance under a new key can see that the order is already its
    /// own.
    #[error("work order {order_id} is {state:?} at version {version} and does not take this input")]
    #[non_exhaustive]
    OrderStateMismatch {
        /// The order.
        order_id: WorkOrderId,
        /// Its state now.
        state: WorkOrderState,
        /// Its version now.
        version: u64,
        /// Its holder now, while it is accepted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holder: Option<WorkOrderParty>,
    },
    /// The offer ran out before the decision.
    #[error("the offer of work order {order_id} expired at {expired_at}")]
    #[non_exhaustive]
    OfferExpired {
        /// The order.
        order_id: WorkOrderId,
        /// When the offer ran out.
        expired_at: DateTime<Utc>,
    },
    /// The item moved after the offer pinned its revision.
    #[error("work item {item_id} moved from the offered revision {pinned} to {current}")]
    #[non_exhaustive]
    ItemRevisionMoved {
        /// The item.
        item_id: WorkItemId,
        /// The revision the offer pinned.
        pinned: u64,
        /// The item's revision now.
        current: u64,
    },
    /// The offerer does not hold the item's claim.
    #[error("the offerer does not hold the claim of work item {item_id}")]
    #[non_exhaustive]
    OldOwnerMismatch {
        /// The item.
        item_id: WorkItemId,
    },
    /// The offerer's claim on the item has a lease that already ran out.
    #[error("the offerer's lease on work item {item_id} has expired")]
    #[non_exhaustive]
    OldOwnerLeaseExpired {
        /// The item.
        item_id: WorkItemId,
    },
    /// The item is not in a status that can change hands.
    #[error("work item {item_id} is {status:?} and cannot be transferred")]
    #[non_exhaustive]
    ItemNotTransferable {
        /// The item.
        item_id: WorkItemId,
        /// Its status now.
        status: WorkStatus,
    },
    /// An offer designates a mob member that has no seat now, so the commit
    /// has nothing to pin the offer to. An offer is made to one incarnation
    /// of a member, never to a name.
    #[error("the designated acceptor has no seat now")]
    AcceptorNotSeated,
    /// A delegated child was offered or accepted, and its offerer does not
    /// hold the parent item.
    #[error("the offerer does not hold the parent work item {item_id}")]
    #[non_exhaustive]
    ParentNotHeld {
        /// The parent item.
        item_id: WorkItemId,
    },
    /// The item already has an order in the slot this input needs.
    #[error("work item {item_id} already has work order {order_id} pending")]
    #[non_exhaustive]
    OrderPending {
        /// The item.
        item_id: WorkItemId,
        /// The order that occupies the slot.
        order_id: WorkOrderId,
    },
    /// The item is held under an accepted order whose holder's lease has not
    /// run out, so it cannot be claimed.
    #[error("work item {item_id} is held under work order {order_id}")]
    #[non_exhaustive]
    OrderHeld {
        /// The item.
        item_id: WorkItemId,
        /// The accepted order.
        order_id: WorkOrderId,
    },
    /// The item is held under an accepted order and must be closed through
    /// that order, with evidence.
    #[error("work item {item_id} must be closed through work order {order_id}")]
    #[non_exhaustive]
    OrderCloseRequired {
        /// The item.
        item_id: WorkItemId,
        /// The accepted order.
        order_id: WorkOrderId,
    },
    /// A completed closure named no evidence.
    #[error("closing a work order as completed needs evidence")]
    EvidenceRequired,
    /// A closure named evidence the subject item does not hold, or evidence
    /// that was withdrawn.
    #[error("evidence {evidence_id} is not current evidence of the subject item")]
    #[non_exhaustive]
    EvidenceUnknown {
        /// The evidence id that did not resolve.
        evidence_id: String,
    },
    /// The acceptor already has attention that excludes the item's.
    #[error("the acceptor's attention target is occupied")]
    AttentionTargetOccupied,
    /// The host's authority for this act is not current. Nothing was written
    /// and nothing is disclosed.
    #[error("the authority for this order input is not current")]
    AuthorityNotLive,
    /// The host installed an authority fence and the store cannot hold it
    /// across the commit.
    #[error("the store cannot hold the host's authority fence across this commit")]
    AuthorityFenceUnsupported,
    /// The offered order protocol is not one this release speaks.
    #[error("order protocol {offered} is not supported (this release speaks {supported})")]
    #[non_exhaustive]
    ProtocolUnsupported {
        /// The protocol the input named.
        offered: u32,
        /// The protocol this release speaks.
        supported: u32,
    },
    /// A whole-item transfer names a counterpart of another mob. Custody
    /// never crosses an exchange.
    #[error("a whole item cannot be transferred to a counterpart of another mob")]
    CustodyStaysLocal,
    /// The order's acceptor is the counterpart of an exchange, so the other
    /// side decides it. The offerer cannot withdraw, replace or expire it on
    /// its own side; it asks through the exchange, and its order changes
    /// only when the other side's answer is admitted.
    #[error("work order {order_id} is decided on the other side of its exchange")]
    #[non_exhaustive]
    CounterpartDecides {
        /// The order.
        order_id: WorkOrderId,
    },
    /// No such order for this caller. An order the caller is not a party to
    /// answers exactly like one that does not exist.
    #[error("work order {order_id} not found")]
    #[non_exhaustive]
    OrderNotFound {
        /// The order id the input named.
        order_id: WorkOrderId,
    },
}

impl WorkOrderRefusal {
    /// The stable wire code, one of [`WORK_ORDER_REFUSAL_CODES`].
    pub const fn code(&self) -> &'static str {
        match self {
            Self::ActorRequired => "actor_required",
            Self::NotIntendedAcceptor => "not_intended_acceptor",
            Self::NotOfferer => "not_offerer",
            Self::NotHolder { .. } => "not_holder",
            Self::StaleOrderVersion { .. } => "stale_order_version",
            Self::OrderStateMismatch { .. } => "order_state_mismatch",
            Self::OfferExpired { .. } => "offer_expired",
            Self::ItemRevisionMoved { .. } => "item_revision_moved",
            Self::OldOwnerMismatch { .. } => "old_owner_mismatch",
            Self::OldOwnerLeaseExpired { .. } => "old_owner_lease_expired",
            Self::ItemNotTransferable { .. } => "item_not_transferable",
            Self::AcceptorNotSeated => "acceptor_not_seated",
            Self::ParentNotHeld { .. } => "parent_not_held",
            Self::OrderPending { .. } => "order_pending",
            Self::OrderHeld { .. } => "order_held",
            Self::OrderCloseRequired { .. } => "order_close_required",
            Self::EvidenceRequired => "evidence_required",
            Self::EvidenceUnknown { .. } => "evidence_unknown",
            Self::AttentionTargetOccupied => "attention_target_occupied",
            Self::AuthorityNotLive => "authority_not_live",
            Self::AuthorityFenceUnsupported => "authority_fence_unsupported",
            Self::ProtocolUnsupported { .. } => "protocol_unsupported",
            Self::CustodyStaysLocal => "custody_stays_local",
            Self::CounterpartDecides { .. } => "counterpart_decides",
            Self::OrderNotFound { .. } => "order_not_found",
        }
    }
}

/// Why an order operation did not complete.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WorkOrderError {
    /// A typed refusal. Nothing was written.
    #[error(transparent)]
    Refused(#[from] WorkOrderRefusal),
    /// A WorkGraph error: malformed input, a store failure, a stale item
    /// revision.
    #[error(transparent)]
    Graph(#[from] WorkGraphError),
    /// A stored input digest names a form this build cannot compute. It was
    /// written by a release that hashes differently. The request under that
    /// key is neither a replay nor a conflict here, and nothing is written.
    #[error(
        "the stored work order input digest has the form `{form}`, which this build cannot compute"
    )]
    #[non_exhaustive]
    DigestFormUnknown {
        /// The form the stored digest names.
        form: String,
    },
    /// A stored input digest has no form, is not of the three parts
    /// `<form>:<hash name>:<digits>`, or names a form this build writes and
    /// is not `<form>:sha256:<64 lowercase hexadecimal digits>`. The record
    /// is not one this module wrote.
    #[error("the stored work order input digest is not of a known shape")]
    DigestMalformed,
}

/// The resolved realm and namespace an order input acts in.
///
/// The service resolves it from the request's optional scope fields and its
/// own namespace grant; the digest hashes this resolved form, so a request
/// that names its scope and one that leaves it to the default digest alike.
///
/// Build one with [`WorkOrderScope::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WorkOrderScope {
    /// The realm.
    pub realm_id: String,
    /// The namespace.
    pub namespace: WorkNamespace,
}

impl WorkOrderScope {
    /// A scope of `realm_id` and `namespace`.
    pub fn new(realm_id: impl Into<String>, namespace: WorkNamespace) -> Self {
        Self {
            realm_id: realm_id.into(),
            namespace,
        }
    }
}

const fn current_protocol() -> u32 {
    WORK_ORDER_PROTOCOL
}

/// A request to offer a work order.
///
/// **Content only.** The offerer is the bound [`WorkActor`], never a field
/// of this request. The order's id, version and state are assigned by the
/// commit. `acceptor` designates the intended acceptor and grants nothing;
/// the commit pins its seat, or its declaration's generation.
///
/// Build one with [`OfferWorkOrderRequest::transfer`] or
/// [`OfferWorkOrderRequest::assign`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct OfferWorkOrderRequest {
    /// Realm, when the caller states it; otherwise the service's own. Never
    /// widens the namespace grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    /// Namespace, when the caller states it; otherwise the service's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    /// What the order is about.
    pub kind: WorkOrderKind,
    /// The intended acceptor, as a designation.
    pub acceptor: WorkOrderDesignation,
    /// The subject: an existing item for a transfer, a proposed item for an
    /// assign.
    pub subject: WorkOrderSubject,
    /// For a delegated child (an assign offer): the parent item the offerer
    /// holds, and the order it holds it under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<WorkOrderParent>,
    /// The terms.
    #[serde(default, skip_serializing_if = "WorkOrderTerms::is_empty")]
    pub terms: WorkOrderTerms,
    /// A still-offered order of the same offerer that this offer replaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<WorkOrderId>,
    /// The order contract version offered. Defaults to
    /// [`WORK_ORDER_PROTOCOL`].
    #[serde(default = "current_protocol")]
    pub protocol: u32,
    /// For the receiving half of an exchange: the other side's order. Set by
    /// host code that admits a counterpart's offer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterpart_order: Option<WorkCounterpartOrderRef>,
}

impl OfferWorkOrderRequest {
    fn new(kind: WorkOrderKind, acceptor: WorkOrderDesignation, subject: WorkOrderSubject) -> Self {
        Self {
            realm_id: None,
            namespace: None,
            kind,
            acceptor,
            subject,
            parent: None,
            terms: WorkOrderTerms::default(),
            supersedes: None,
            protocol: WORK_ORDER_PROTOCOL,
            counterpart_order: None,
        }
    }

    /// Offer the whole of `item_id`, as it is at `item_revision`, to
    /// `acceptor`.
    pub fn transfer(
        acceptor: WorkOrderDesignation,
        item_id: WorkItemId,
        item_revision: u64,
    ) -> Self {
        Self::new(
            WorkOrderKind::Transfer,
            acceptor,
            WorkOrderSubject::item(item_id, item_revision),
        )
    }

    /// Offer the proposed `item` to `acceptor`. Nothing is created until the
    /// offer is accepted.
    pub fn assign(acceptor: WorkOrderDesignation, item: ProposedWorkItem) -> Self {
        Self::new(
            WorkOrderKind::Assign,
            acceptor,
            WorkOrderSubject::Proposed(item),
        )
    }

    /// The same request with an explicit scope.
    #[must_use]
    pub fn with_scope(mut self, realm_id: impl Into<String>, namespace: WorkNamespace) -> Self {
        self.realm_id = Some(realm_id.into());
        self.namespace = Some(namespace);
        self
    }

    /// The same request with `terms`.
    #[must_use]
    pub fn with_terms(mut self, terms: WorkOrderTerms) -> Self {
        self.terms = terms;
        self
    }

    /// The same request as a delegated child of `parent`.
    #[must_use]
    pub fn with_parent(mut self, parent: WorkOrderParent) -> Self {
        self.parent = Some(parent);
        self
    }

    /// The same request, replacing the still-offered order `supersedes`.
    #[must_use]
    pub fn with_supersedes(mut self, supersedes: WorkOrderId) -> Self {
        self.supersedes = Some(supersedes);
        self
    }

    /// The same request as the receiving half of the other side's
    /// `counterpart_order`.
    #[must_use]
    pub fn with_counterpart_order(mut self, counterpart_order: WorkCounterpartOrderRef) -> Self {
        self.counterpart_order = Some(counterpart_order);
        self
    }

    /// Check the request's own shape, before any store is read.
    ///
    /// Refused: a protocol this release does not speak
    /// ([`WorkOrderRefusal::ProtocolUnsupported`]); a transfer to a
    /// counterpart ([`WorkOrderRefusal::CustodyStaysLocal`]). Invalid: a
    /// transfer without an existing item, an assign without a proposed item,
    /// a parent on a transfer, a blank or oversized title, an empty or
    /// oversized description or note, a lease outside its bounds, an expiry
    /// finer than a millisecond.
    pub fn validate(&self) -> Result<(), WorkOrderError> {
        if self.protocol != WORK_ORDER_PROTOCOL {
            return Err(WorkOrderRefusal::ProtocolUnsupported {
                offered: self.protocol,
                supported: WORK_ORDER_PROTOCOL,
            }
            .into());
        }
        match (self.kind, &self.subject) {
            (WorkOrderKind::Transfer, WorkOrderSubject::Item { .. }) => {
                if !self.acceptor.is_local() {
                    return Err(WorkOrderRefusal::CustodyStaysLocal.into());
                }
                if self.parent.is_some() {
                    return Err(WorkGraphError::InvalidInput(
                        "a transfer offer has no parent; only an assign offer delegates a child"
                            .to_string(),
                    )
                    .into());
                }
            }
            (WorkOrderKind::Assign, WorkOrderSubject::Proposed(item)) => item.validate()?,
            (WorkOrderKind::Transfer, WorkOrderSubject::Proposed(_)) => {
                return Err(WorkGraphError::InvalidInput(
                    "a transfer offer names an existing item, not a proposed one".to_string(),
                )
                .into());
            }
            (WorkOrderKind::Assign, WorkOrderSubject::Item { .. }) => {
                return Err(WorkGraphError::InvalidInput(
                    "an assign offer proposes an item, it does not name an existing one"
                        .to_string(),
                )
                .into());
            }
        }
        self.terms.validate()?;
        Ok(())
    }

    /// The canonical digest of this offer by `actor` in `scope`.
    ///
    /// # What is hashed
    ///
    /// SHA-256 over these bytes, in this order:
    ///
    /// 1. [`WORK_ORDER_INPUT_DIGEST_DOMAIN`] as UTF-8;
    /// 2. one zero byte;
    /// 3. the canonical JSON of the body below.
    ///
    /// Canonical JSON here means: UTF-8; object keys in ascending byte
    /// order; no whitespace; arrays in the given order; integers in decimal;
    /// a field that is absent is omitted, never written as `null`. Because
    /// absent fields are omitted, a field added in a later release leaves
    /// the digest of every request that does not use it unchanged. The
    /// object and array encoding is written out by hand in this module; it
    /// does not follow the derived serde form of any type.
    ///
    /// Strings are escaped as JSON requires and no further: `"` and `\`
    /// with a backslash; backspace, form feed, line feed, carriage return
    /// and tab as `\b`, `\f`, `\n`, `\r`, `\t`; every other character below
    /// U+0020 as `\u00XX` with lowercase hexadecimal digits; everything
    /// else, U+007F, U+2028, U+2029 and characters outside the basic plane
    /// included, as its own UTF-8 bytes. A fixed digest over a text that
    /// holds each of those pins it, so a change of the JSON library cannot
    /// move a digest unseen.
    ///
    /// Two requests of one meaning have one digest: an empty note or
    /// description is refused rather than read as absent, an expiry finer
    /// than a millisecond is refused, and a reason that may be left out and
    /// says nothing is hashed as left out.
    ///
    /// The body of an offer:
    ///
    /// ```text
    /// { "actor": <canonical key of the bound actor's party>,
    ///   "content": {
    ///     "acceptor": <canonical key of the designated acceptor>,
    ///     "counterpart_order": <text, if set>,
    ///     "kind": "transfer" | "assign",
    ///     "parent": { "item_id": <text>, "order_id": <text, if set> }, if set
    ///     "protocol": <integer>,
    ///     "subject": { "item": { "item_id": <text>, "item_revision": <integer> } }
    ///              | { "proposed": { "description": <text, if set>,
    ///                                "title": <text> } },
    ///     "supersedes": <text, if set>,
    ///     "terms": { "holder_lease_seconds": <integer, if set>,
    ///                "note": <text, if set>,
    ///                "offer_expires_at_ms": <integer UTC milliseconds, if set> } },
    ///   "input": "offer",
    ///   "namespace": <text>,
    ///   "realm_id": <text> }
    /// ```
    ///
    /// The actor's canonical key holds its pinned seat
    /// ([`WorkOrderParty::canonical`]), so the same request by a member of
    /// the same name in another seat has another digest.
    /// The request's own optional `realm_id` and `namespace` are NOT hashed;
    /// the resolved `scope` is. The result is the digest's form
    /// ([`WORK_ORDER_INPUT_DIGEST_FORM`]), `:sha256:`, and the lowercase
    /// hexadecimal digest, so a stored digest says which form made it.
    ///
    /// Only a request that passes [`Self::validate_for`] has a digest: the
    /// check runs first, here, so nothing can hash a request that would be
    /// refused for its shape, and no caller has to remember the order of
    /// two calls.
    pub fn input_digest(
        &self,
        scope: &WorkOrderScope,
        actor: &WorkActor,
    ) -> Result<String, WorkOrderError> {
        self.validate_for(actor)?;
        Ok(digest_in(
            WorkOrderDigestForm::CURRENT,
            &self.digest_body(scope, actor),
        )?)
    }

    /// The exact identity of this offer under `key`: the key and
    /// [`Self::input_digest`]. It is what a host authority fence is handed
    /// and what the order records for the step; it grants nothing.
    pub fn input_identity(
        &self,
        key: WorkAdmissionKey,
        scope: &WorkOrderScope,
        actor: &WorkActor,
    ) -> Result<WorkAdmission, WorkOrderError> {
        Ok(WorkAdmission {
            key,
            request_digest: self.input_digest(scope, actor)?,
        })
    }

    /// Whether this offer by `actor` is the request a stored digest was
    /// made from: `true` is a replay, `false` is other content under the
    /// same key (a conflict).
    ///
    /// The stored digest is READ: its form is taken from its own text
    /// ([`WorkOrderDigestForm::of_stored`]) and this request is hashed in
    /// that form. A digest of a form this build does not know, or of no
    /// form, is an error and never `false`: it is not this build's to call
    /// a conflict.
    pub fn matches_stored_digest(
        &self,
        scope: &WorkOrderScope,
        actor: &WorkActor,
        stored: &str,
    ) -> Result<bool, WorkOrderError> {
        let form = WorkOrderDigestForm::of_stored(stored)?;
        self.validate_for(actor)?;
        Ok(digest_in(form, &self.digest_body(scope, actor))? == stored)
    }

    /// [`Self::validate`], and the one rule that depends on who asks: the
    /// other side's order id.
    ///
    /// `counterpart_order` says that this offer is the receiving half of an
    /// order of the other side. Only the host-built counterpart actor of an
    /// admitted envelope may say so; from any other actor it is refused as
    /// invalid input. An agent's tool has no argument for it.
    pub fn validate_for(&self, actor: &WorkActor) -> Result<(), WorkOrderError> {
        self.validate()?;
        if self.counterpart_order.is_some() && actor.source() != WorkActorSource::Counterpart {
            return Err(WorkGraphError::InvalidInput(
                "only a counterpart's offer names the other side's order".to_string(),
            )
            .into());
        }
        Ok(())
    }

    fn digest_body(&self, scope: &WorkOrderScope, actor: &WorkActor) -> Value {
        let mut terms = Map::new();
        put_opt(
            &mut terms,
            "holder_lease_seconds",
            self.terms.holder_lease_seconds.map(Value::from),
        );
        put_opt(&mut terms, "note", self.terms.note.clone().map(Value::from));
        put_opt(
            &mut terms,
            "offer_expires_at_ms",
            self.terms
                .offer_expires_at
                .map(|at| Value::from(at.timestamp_millis())),
        );

        let subject = match &self.subject {
            WorkOrderSubject::Item {
                item_id,
                item_revision,
            } => {
                let mut item = Map::new();
                item.insert("item_id".into(), item_id.as_str().into());
                item.insert("item_revision".into(), (*item_revision).into());
                single_entry("item", item)
            }
            WorkOrderSubject::Proposed(proposed) => {
                let mut item = Map::new();
                put_opt(
                    &mut item,
                    "description",
                    proposed.description.clone().map(Value::from),
                );
                item.insert("title".into(), proposed.title.as_str().into());
                single_entry("proposed", item)
            }
        };

        let mut content = Map::new();
        content.insert("acceptor".into(), self.acceptor.canonical().into());
        put_opt(
            &mut content,
            "counterpart_order",
            self.counterpart_order
                .as_ref()
                .map(|order| Value::from(order.as_str())),
        );
        content.insert("kind".into(), self.kind.as_str().into());
        put_opt(
            &mut content,
            "parent",
            self.parent.as_ref().map(|parent| {
                let mut map = Map::new();
                map.insert("item_id".into(), parent.item_id.as_str().into());
                put_opt(
                    &mut map,
                    "order_id",
                    parent.order_id.as_ref().map(|id| Value::from(id.as_str())),
                );
                Value::Object(map)
            }),
        );
        content.insert("protocol".into(), self.protocol.into());
        content.insert("subject".into(), subject);
        put_opt(
            &mut content,
            "supersedes",
            self.supersedes.as_ref().map(|id| Value::from(id.as_str())),
        );
        content.insert("terms".into(), Value::Object(terms));

        digest_envelope(
            WorkOrderInputKind::Offer,
            scope,
            actor,
            None,
            Value::Object(content),
        )
    }
}

/// What a party asks of an existing order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum WorkOrderInput {
    /// The intended acceptor accepts.
    #[non_exhaustive]
    Accept {},
    /// The intended acceptor declines.
    #[non_exhaustive]
    Decline {
        /// Why.
        reason: WorkOrderReason,
    },
    /// The offerer withdraws before a decision.
    #[non_exhaustive]
    Withdraw {
        /// Why, if stated. A reason that says nothing (no typed cause, no
        /// note) is the same as none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<WorkOrderReason>,
    },
    /// The holder closes the accepted order.
    #[non_exhaustive]
    Close {
        /// The outcome. [`WorkOrderCloseOutcome::HandedOver`] cannot be
        /// requested.
        outcome: WorkOrderCloseOutcome,
        /// The evidence the closure is bound to; required for
        /// [`WorkOrderCloseOutcome::Completed`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        evidence: Vec<WorkOrderEvidenceRef>,
        /// Why, for an unsuccessful closure. A reason that says nothing is
        /// the same as none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<WorkOrderReason>,
    },
}

impl WorkOrderInput {
    /// The intended acceptor accepts.
    pub const fn accept() -> Self {
        Self::Accept {}
    }

    /// The intended acceptor declines, for `reason`.
    pub const fn decline(reason: WorkOrderReason) -> Self {
        Self::Decline { reason }
    }

    /// The offerer withdraws, stating no reason.
    pub const fn withdraw() -> Self {
        Self::Withdraw { reason: None }
    }

    /// The offerer withdraws, for `reason`.
    pub const fn withdraw_because(reason: WorkOrderReason) -> Self {
        Self::Withdraw {
            reason: Some(reason),
        }
    }

    /// The holder closes with `outcome`, bound to `evidence`.
    pub const fn close(
        outcome: WorkOrderCloseOutcome,
        evidence: Vec<WorkOrderEvidenceRef>,
    ) -> Self {
        Self::Close {
            outcome,
            evidence,
            reason: None,
        }
    }

    /// The holder closes with `outcome`, bound to `evidence`, for `reason`.
    pub const fn close_because(
        outcome: WorkOrderCloseOutcome,
        evidence: Vec<WorkOrderEvidenceRef>,
        reason: WorkOrderReason,
    ) -> Self {
        Self::Close {
            outcome,
            evidence,
            reason: Some(reason),
        }
    }

    /// The input kind.
    pub const fn kind(&self) -> WorkOrderInputKind {
        match self {
            Self::Accept {} => WorkOrderInputKind::Accept,
            Self::Decline { .. } => WorkOrderInputKind::Decline,
            Self::Withdraw { .. } => WorkOrderInputKind::Withdraw,
            Self::Close { .. } => WorkOrderInputKind::Close,
        }
    }
}

/// A request to make one input on an existing order.
///
/// **Content only.** Who makes the input is the bound [`WorkActor`]; whether
/// it may is decided against the order (the intended acceptor accepts,
/// declines and closes; the offerer withdraws). `expected_version` is the
/// order version the caller read.
///
/// Build one with [`WorkOrderInputRequest::new`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkOrderInputRequest {
    /// Realm, when the caller states it; otherwise the service's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    /// Namespace, when the caller states it; otherwise the service's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    /// The order.
    pub order_id: WorkOrderId,
    /// The order version the caller read.
    pub expected_version: u64,
    /// The input.
    pub input: WorkOrderInput,
}

impl WorkOrderInputRequest {
    /// `input` on `order_id` at `expected_version`, in the service's own
    /// scope.
    pub const fn new(order_id: WorkOrderId, expected_version: u64, input: WorkOrderInput) -> Self {
        Self {
            realm_id: None,
            namespace: None,
            order_id,
            expected_version,
            input,
        }
    }

    /// The same request with an explicit scope.
    #[must_use]
    pub fn with_scope(mut self, realm_id: impl Into<String>, namespace: WorkNamespace) -> Self {
        self.realm_id = Some(realm_id.into());
        self.namespace = Some(namespace);
        self
    }

    /// Check the request's own shape, before any store is read.
    ///
    /// Refused: a completed closure without evidence
    /// ([`WorkOrderRefusal::EvidenceRequired`]). Invalid: a requested
    /// hand-over, a blank or oversized evidence reference, an empty or
    /// oversized note.
    pub fn validate(&self) -> Result<(), WorkOrderError> {
        match &self.input {
            WorkOrderInput::Accept {} => {}
            WorkOrderInput::Decline { reason } => reason.validate()?,
            WorkOrderInput::Withdraw { reason } => {
                if let Some(reason) = reason {
                    reason.validate()?;
                }
            }
            WorkOrderInput::Close {
                outcome,
                evidence,
                reason,
            } => {
                if *outcome == WorkOrderCloseOutcome::HandedOver {
                    return Err(WorkGraphError::InvalidInput(
                        "a hand-over is recorded by the acceptance of a successor order and cannot be requested"
                            .to_string(),
                    )
                    .into());
                }
                if *outcome == WorkOrderCloseOutcome::Completed && evidence.is_empty() {
                    return Err(WorkOrderRefusal::EvidenceRequired.into());
                }
                for reference in evidence {
                    reference.validate()?;
                }
                if let Some(reason) = reason {
                    reason.validate()?;
                }
            }
        }
        Ok(())
    }

    /// [`Self::validate`], and the one rule that depends on who asks: the
    /// form of a closure's evidence.
    ///
    /// A holder of this graph binds its closure to evidence on the subject
    /// item, by id; a digest it typed is not evidence of anything here. A
    /// counterpart's mirrored closure carries digests, because the other
    /// side's evidence ids name nothing in this graph. Either form from the
    /// wrong kind of actor is refused
    /// ([`WorkOrderRefusal::EvidenceUnknown`], naming the reference).
    pub fn validate_for(&self, actor: &WorkActor) -> Result<(), WorkOrderError> {
        self.validate()?;
        if let WorkOrderInput::Close { evidence, .. } = &self.input {
            let by_counterpart = actor.source() == WorkActorSource::Counterpart;
            for reference in evidence {
                let is_digest = matches!(reference, WorkOrderEvidenceRef::CounterpartDigest { .. });
                if is_digest != by_counterpart {
                    return Err(WorkOrderRefusal::EvidenceUnknown {
                        evidence_id: reference.name().to_string(),
                    }
                    .into());
                }
            }
        }
        Ok(())
    }

    /// The canonical digest of this input by `actor` in `scope`.
    ///
    /// Hashed exactly as [`OfferWorkOrderRequest::input_digest`] describes
    /// (domain string, one zero byte, canonical JSON with absent fields
    /// omitted), over this body:
    ///
    /// ```text
    /// { "actor": <canonical key of the bound actor's party>,
    ///   "content": <see below>,
    ///   "expected_version": <integer>,
    ///   "input": "accept" | "decline" | "withdraw" | "close",
    ///   "namespace": <text>,
    ///   "order_id": <text>,
    ///   "realm_id": <text> }
    /// ```
    ///
    /// with `content`:
    ///
    /// ```text
    /// accept:   { }
    /// decline:  { "reason": { "code": <text>, "note": <text, if set> } }
    /// withdraw: { "reason": { ... }, if set }
    /// close:    { "evidence": [ { "evidence_id": <text>, "kind": "item" }
    ///                         | { "digest": <text>, "evidence_kind": <text>,
    ///                             "kind": "counterpart_digest" }, ... ],
    ///             "outcome": <text>,
    ///             "reason": { ... }, if set }
    /// ```
    ///
    /// The evidence array is always written for a closure, in the order
    /// given.
    ///
    /// Only a request that passes [`Self::validate_for`] has a digest; the
    /// check runs first, here.
    pub fn input_digest(
        &self,
        scope: &WorkOrderScope,
        actor: &WorkActor,
    ) -> Result<String, WorkOrderError> {
        self.validate_for(actor)?;
        Ok(digest_in(
            WorkOrderDigestForm::CURRENT,
            &self.digest_body(scope, actor),
        )?)
    }

    /// The exact identity of this input under `key`: the key and
    /// [`Self::input_digest`]. It grants nothing.
    pub fn input_identity(
        &self,
        key: WorkAdmissionKey,
        scope: &WorkOrderScope,
        actor: &WorkActor,
    ) -> Result<WorkAdmission, WorkOrderError> {
        Ok(WorkAdmission {
            key,
            request_digest: self.input_digest(scope, actor)?,
        })
    }

    /// Whether this input by `actor` is the request a stored digest was made
    /// from; see [`OfferWorkOrderRequest::matches_stored_digest`].
    pub fn matches_stored_digest(
        &self,
        scope: &WorkOrderScope,
        actor: &WorkActor,
        stored: &str,
    ) -> Result<bool, WorkOrderError> {
        let form = WorkOrderDigestForm::of_stored(stored)?;
        self.validate_for(actor)?;
        Ok(digest_in(form, &self.digest_body(scope, actor))? == stored)
    }

    /// The reason this input states, in the one form a decision record
    /// keeps and the digest hashes.
    ///
    /// A decline always has one. On a withdrawal and on a closure a reason
    /// may be left out, and one that says nothing (no code beyond
    /// unspecified, no note) IS left out: the commit records `None` for it,
    /// never the empty reason, so one meaning has one stored form as it has
    /// one digest.
    pub fn stated_reason(&self) -> Option<&WorkOrderReason> {
        match &self.input {
            WorkOrderInput::Accept {} => None,
            WorkOrderInput::Decline { reason } => Some(reason),
            WorkOrderInput::Withdraw { reason } | WorkOrderInput::Close { reason, .. } => {
                reason.as_ref().filter(|reason| !reason.says_nothing())
            }
        }
    }

    fn digest_body(&self, scope: &WorkOrderScope, actor: &WorkActor) -> Value {
        // The reason is hashed as it is stored: one meaning, one digest.
        let stated = self.stated_reason().map(reason_value);
        let mut content = Map::new();
        match &self.input {
            WorkOrderInput::Accept {} => {}
            WorkOrderInput::Decline { .. } | WorkOrderInput::Withdraw { .. } => {
                put_opt(&mut content, "reason", stated);
            }
            WorkOrderInput::Close {
                outcome, evidence, ..
            } => {
                content.insert(
                    "evidence".into(),
                    Value::Array(evidence.iter().map(evidence_value).collect()),
                );
                content.insert("outcome".into(), outcome.as_str().into());
                put_opt(&mut content, "reason", stated);
            }
        }
        digest_envelope(
            self.input.kind(),
            scope,
            actor,
            Some((&self.order_id, self.expected_version)),
            Value::Object(content),
        )
    }
}

/// The part the bound actor plays in an order, for a read of its own
/// orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WorkOrderRole {
    /// The actor is the order's intended acceptor.
    Acceptor,
    /// The actor offered the order.
    Offerer,
    /// The actor holds the order: it accepted it and the order is accepted.
    Holder,
}

impl WorkOrderRole {
    /// The wire name.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Acceptor => "acceptor",
            Self::Offerer => "offerer",
            Self::Holder => "holder",
        }
    }
}

/// Selects orders for a read.
///
/// A read grants nothing and writes nothing. Like every request shape it
/// holds no seat and no declaration generation: `mine` selects by the BOUND
/// ACTOR'S own party, which the service takes from the actor and compares
/// seat included, so `mine` = acceptor with `states` = offered is "what is
/// pending for me" and never lists an order of an earlier seat of the same
/// name. `acceptor` and `offerer` select by designation, whatever seat the
/// record pinned.
///
/// Start from [`WorkOrderFilter::default`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct WorkOrderFilter {
    /// Realm; the service's own when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realm_id: Option<String>,
    /// Namespace; the service's own when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<WorkNamespace>,
    /// Orders in which the bound actor itself plays this part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mine: Option<WorkOrderRole>,
    /// Orders whose intended acceptor has this designation, in any seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptor: Option<WorkOrderDesignation>,
    /// Orders offered under this designation, in any seat.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offerer: Option<WorkOrderDesignation>,
    /// Orders about this item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<WorkItemId>,
    /// Orders delegated under this parent item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_item_id: Option<WorkItemId>,
    /// Orders that are a half of this exchange.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exchange: Option<WorkExchangeId>,
    /// The order that is the receiving half of this order of the other side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterpart_order: Option<WorkCounterpartOrderRef>,
    /// Orders in one of these states; every state when empty.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub states: BTreeSet<WorkOrderState>,
    /// At most this many orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Continue after this cursor of an earlier page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

fn put_opt(map: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        map.insert(key.to_string(), value);
    }
}

fn single_entry(key: &str, entry: Map<String, Value>) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), Value::Object(entry));
    Value::Object(map)
}

fn reason_value(reason: &WorkOrderReason) -> Value {
    let mut map = Map::new();
    map.insert("code".into(), reason.code.as_str().into());
    put_opt(&mut map, "note", reason.note.clone().map(Value::from));
    Value::Object(map)
}

fn evidence_value(reference: &WorkOrderEvidenceRef) -> Value {
    let mut map = Map::new();
    match reference {
        WorkOrderEvidenceRef::Item { evidence_id } => {
            map.insert("evidence_id".into(), evidence_id.as_str().into());
            map.insert("kind".into(), "item".into());
        }
        WorkOrderEvidenceRef::CounterpartDigest {
            evidence_kind,
            digest,
        } => {
            map.insert("digest".into(), digest.as_str().into());
            map.insert("evidence_kind".into(), evidence_kind.as_str().into());
            map.insert("kind".into(), "counterpart_digest".into());
        }
    }
    Value::Object(map)
}

fn digest_envelope(
    input: WorkOrderInputKind,
    scope: &WorkOrderScope,
    actor: &WorkActor,
    order: Option<(&WorkOrderId, u64)>,
    content: Value,
) -> Value {
    let mut body = Map::new();
    body.insert("actor".into(), actor.party().canonical().into());
    body.insert("content".into(), content);
    if let Some((order_id, expected_version)) = order {
        body.insert("expected_version".into(), expected_version.into());
        body.insert("order_id".into(), order_id.as_str().into());
    }
    body.insert("input".into(), input.as_str().into());
    body.insert("namespace".into(), scope.namespace.as_str().into());
    body.insert("realm_id".into(), scope.realm_id.as_str().into());
    Value::Object(body)
}

/// Write `value` as canonical JSON: keys in ascending byte order, no
/// whitespace, integers only. The order is imposed here, so it does not
/// depend on how `serde_json` happens to keep a map.
fn write_canonical_json(value: &Value, out: &mut Vec<u8>) -> Result<(), WorkGraphError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(flag) => out.extend_from_slice(if *flag {
            b"true".as_slice()
        } else {
            b"false".as_slice()
        }),
        Value::Number(number) => {
            if !(number.is_u64() || number.is_i64()) {
                return Err(WorkGraphError::InvalidInput(
                    "a work order digest body holds integers only".to_string(),
                ));
            }
            out.extend_from_slice(number.to_string().as_bytes());
        }
        Value::String(text) => write_canonical_string(text, out)?,
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical_json(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by_key(|entry| entry.0.as_bytes());
            out.push(b'{');
            for (index, (key, entry)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical_string(key, out)?;
                out.push(b':');
                write_canonical_json(entry, out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn write_canonical_string(text: &str, out: &mut Vec<u8>) -> Result<(), WorkGraphError> {
    serde_json::to_writer(out, text).map_err(|error| {
        WorkGraphError::InvalidInput(format!(
            "a work order digest string is not encodable: {error}"
        ))
    })
}

fn canonical_input_bytes(body: &Value) -> Result<Vec<u8>, WorkGraphError> {
    let mut bytes = Vec::new();
    write_canonical_json(body, &mut bytes)?;
    Ok(bytes)
}

fn digest_in(form: WorkOrderDigestForm, body: &Value) -> Result<String, WorkGraphError> {
    match form {
        WorkOrderDigestForm::V1 => {
            let canonical = canonical_input_bytes(body)?;
            let mut hasher = Sha256::new();
            hasher.update(WORK_ORDER_INPUT_DIGEST_DOMAIN.as_bytes());
            hasher.update([0u8]);
            hasher.update(&canonical);
            Ok(format!("{}:sha256:{:x}", form.name(), hasher.finalize()))
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    const INCARNATION: &str = "0190a7c8-1111-4222-8333-444455556666";
    const LATER_INCARNATION: &str = "0190a7c8-9999-4aaa-8bbb-ccccddddeeee";

    fn incarnation() -> MobIncarnationId {
        INCARNATION.parse().expect("incarnation")
    }

    /// The lead sits in generation 3 of its identity, the reviewer in 1.
    fn lead_seat() -> WorkMemberSeat {
        WorkMemberSeat::new(incarnation(), 3)
    }

    fn reviewer_seat() -> WorkMemberSeat {
        WorkMemberSeat::new(incarnation(), 1)
    }

    fn lead() -> WorkOwnerKey {
        WorkOwnerKey::mob_agent("alpha", "lead").expect("lead key")
    }

    fn reviewer() -> WorkOwnerKey {
        WorkOwnerKey::mob_agent("alpha", "reviewer").expect("reviewer key")
    }

    fn lead_actor() -> WorkActor {
        WorkActor::mob_member(lead(), lead_seat()).expect("lead actor")
    }

    fn reviewer_actor() -> WorkActor {
        WorkActor::mob_member(reviewer(), reviewer_seat()).expect("reviewer actor")
    }

    fn exchange() -> WorkExchangeId {
        WorkExchangeId::new("ops-to-research").expect("exchange id")
    }

    /// The other side of the exchange, admitted under generation 4 of its
    /// declaration.
    fn counterpart_actor() -> WorkActor {
        WorkActor::counterpart(
            exchange(),
            4,
            WorkCounterpartPartyRef::new("ops").expect("party ref"),
        )
    }

    fn lead_party_json() -> Value {
        json!({
            "kind": "local",
            "key": {"kind": "agent", "id": "mob/alpha/agent/lead"},
            "seat": {"mob_incarnation": INCARNATION, "member_generation": 3}
        })
    }

    fn reviewer_party_json() -> Value {
        json!({
            "kind": "local",
            "key": {"kind": "agent", "id": "mob/alpha/agent/reviewer"},
            "seat": {"mob_incarnation": INCARNATION, "member_generation": 1}
        })
    }

    /// The reviewer as a request designates it: by owner key and nothing
    /// else.
    fn reviewer_designation() -> WorkOrderDesignation {
        WorkOrderDesignation::local(reviewer())
    }

    fn reviewer_designation_json() -> Value {
        json!({"kind": "local", "key": {"kind": "agent", "id": "mob/alpha/agent/reviewer"}})
    }

    fn scope() -> WorkOrderScope {
        WorkOrderScope::new("mob.alpha", WorkNamespace::default())
    }

    fn order_id() -> WorkOrderId {
        WorkOrderId::new("order_0001").expect("order id")
    }

    fn item_id() -> WorkItemId {
        WorkItemId::new("work_0001").expect("item id")
    }

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 24, 17, 18, second)
            .single()
            .expect("fixed time")
    }

    fn admission(key: &str, digest: &str) -> WorkAdmission {
        WorkAdmission {
            key: WorkAdmissionKey::new(key).expect("admission key"),
            request_digest: digest.to_string(),
        }
    }

    fn grant() -> WorkAuthorityBasis {
        WorkAuthorityBasis::NamespaceGrant {
            realm_id: "mob.alpha".to_string(),
            namespace: WorkNamespace::default(),
        }
    }

    fn offer_decision() -> WorkOrderDecision {
        WorkOrderDecision {
            step: WorkOrderStep::Offer,
            input: WorkOrderInputKind::Offer,
            actor: Some(lead_actor().record()),
            authority: grant(),
            identity: Some(admission("offer-key", "v1:sha256:aa")),
            order_version: 1,
            item_revision_before: None,
            item_revision_after: None,
            reason: None,
            lapse_cause: None,
            at: at(0),
        }
    }

    /// An accepted transfer order with its two decision records.
    fn accepted_transfer() -> WorkOrder {
        WorkOrder {
            id: order_id(),
            realm_id: "mob.alpha".to_string(),
            namespace: WorkNamespace::default(),
            protocol: WORK_ORDER_PROTOCOL,
            version: 2,
            kind: WorkOrderKind::Transfer,
            offerer: lead_actor().party().clone(),
            acceptor: reviewer_actor().party().clone(),
            subject: WorkOrderSubject::item(item_id(), 8),
            parent: None,
            terms: WorkOrderTerms::default().with_holder_lease_seconds(3600),
            state: WorkOrderState::Accepted,
            closure: None,
            supersedes: None,
            superseded_by: None,
            counterpart_order: None,
            decisions: vec![
                offer_decision(),
                WorkOrderDecision {
                    step: WorkOrderStep::Decision,
                    input: WorkOrderInputKind::Accept,
                    actor: Some(reviewer_actor().record()),
                    authority: WorkAuthorityBasis::HostFence {
                        reference: "declaration/7".to_string(),
                    },
                    identity: Some(admission("accept-key", "v1:sha256:bb")),
                    order_version: 2,
                    item_revision_before: Some(7),
                    item_revision_after: Some(8),
                    reason: None,
                    lapse_cause: None,
                    at: at(30),
                },
            ],
            created_at: at(0),
            updated_at: at(30),
            terminal_at: None,
        }
    }

    fn offer_decision_json() -> Value {
        json!({
            "step": "offer",
            "input": "offer",
            "actor": {"party": lead_party_json(), "source": "mob_member"},
            "authority": {"kind": "namespace_grant", "realm_id": "mob.alpha", "namespace": "default"},
            "identity": {"key": "offer-key", "request_digest": "v1:sha256:aa"},
            "order_version": 1,
            "at": "2026-11-24T17:18:00Z"
        })
    }

    fn accepted_transfer_json() -> Value {
        json!({
            "id": "order_0001",
            "realm_id": "mob.alpha",
            "namespace": "default",
            "protocol": 1,
            "version": 2,
            "kind": "transfer",
            "offerer": lead_party_json(),
            "acceptor": reviewer_party_json(),
            "subject": {"item": {"item_id": "work_0001", "item_revision": 8}},
            "terms": {"holder_lease_seconds": 3600},
            "state": "accepted",
            "decisions": [
                offer_decision_json(),
                {
                    "step": "decision",
                    "input": "accept",
                    "actor": {"party": reviewer_party_json(), "source": "mob_member"},
                    "authority": {"kind": "host_fence", "reference": "declaration/7"},
                    "identity": {"key": "accept-key", "request_digest": "v1:sha256:bb"},
                    "order_version": 2,
                    "item_revision_before": 7,
                    "item_revision_after": 8,
                    "at": "2026-11-24T17:18:30Z"
                }
            ],
            "created_at": "2026-11-24T17:18:00Z",
            "updated_at": "2026-11-24T17:18:30Z"
        })
    }

    /// Both directions against one fixed JSON document.
    fn assert_fixed_json<T>(value: &T, fixed: &Value)
    where
        T: Serialize + serde::de::DeserializeOwned + PartialEq + fmt::Debug,
    {
        assert_eq!(&serde_json::to_value(value).expect("serialize"), fixed);
        assert_eq!(
            &serde_json::from_value::<T>(fixed.clone()).expect("deserialize"),
            value
        );
    }

    #[test]
    fn order_ids_are_minted_and_refuse_non_canonical_text() {
        let minted = WorkOrderId::generated();
        assert!(minted.as_str().starts_with("order_"));
        assert_ne!(minted, WorkOrderId::generated());
        for bad in ["", " order_1", "order_1 ", "order\n1"] {
            assert!(WorkOrderId::new(bad).is_err(), "accepted {bad:?}");
            assert!(serde_json::from_value::<WorkOrderId>(json!(bad)).is_err());
        }
        assert!(WorkOrderId::new("x".repeat(WORK_ORDER_REF_MAX_BYTES + 1)).is_err());
        assert_fixed_json(&order_id(), &json!("order_0001"));
    }

    #[test]
    fn exchange_ids_keep_a_closed_character_set() {
        assert_fixed_json(
            &WorkExchangeId::new("ops-to-research.v2_a").expect("exchange id"),
            &json!("ops-to-research.v2_a"),
        );
        for bad in [
            "",
            "ops:research",
            "ops/research",
            "ops research",
            "ops@4",
            "caf\u{e9}",
        ] {
            assert!(WorkExchangeId::new(bad).is_err(), "accepted {bad:?}");
            assert!(serde_json::from_value::<WorkExchangeId>(json!(bad)).is_err());
        }
    }

    #[test]
    fn work_actor_constructors_check_the_shape_of_their_source() {
        let member = lead_actor();
        assert_eq!(member.source(), WorkActorSource::MobMember);
        assert_eq!(member.owner_key(), Some(&lead()));
        assert_eq!(member.party().seat(), Some(lead_seat()));

        // A name that is not the mob-member shape cannot be bound as a
        // member, as an actor or as a party of a record.
        let plain_agent = WorkOwnerKey::agent("lead").expect("plain agent key");
        assert!(WorkActor::mob_member(plain_agent.clone(), lead_seat()).is_err());
        assert!(WorkOrderParty::mob_member(plain_agent.clone(), lead_seat()).is_err());
        // A member key is not a principal, and the reverse.
        assert!(WorkActor::host_principal(lead()).is_err());
        assert!(WorkActor::host_principal(plain_agent).is_err());
        let principal = WorkOwnerKey::principal("user-17").expect("principal key");
        assert!(WorkActor::mob_member(principal.clone(), lead_seat()).is_err());
        assert!(WorkOrderParty::mob_member(principal.clone(), lead_seat()).is_err());
        let person = WorkActor::host_principal(principal).expect("principal actor");
        assert_eq!(person.source(), WorkActorSource::HostPrincipal);
        assert_eq!(person.party().seat(), None);

        let other_side = counterpart_actor();
        assert_eq!(other_side.source(), WorkActorSource::Counterpart);
        assert_eq!(other_side.owner_key(), None);
        assert_eq!(other_side.party().seat(), None);
    }

    #[test]
    fn work_actor_has_no_wire_form_and_its_record_is_provenance_only() {
        // That `WorkActor` implements neither `Deserialize`, `Serialize` nor
        // `JsonSchema` is proven at compile time by the `assert_not_impl!`
        // lines under the type: this crate does not build if one is added.
        // The record a decision keeps does round-trip, and is a different
        // type with no way back.
        let record = lead_actor().record();
        assert_fixed_json(
            &record,
            &json!({"party": lead_party_json(), "source": "mob_member"}),
        );
        assert_eq!(record.party, *lead_actor().party());
    }

    #[test]
    fn a_member_of_the_same_name_in_another_seat_is_another_party() {
        let later_mob: MobIncarnationId = LATER_INCARNATION.parse().expect("later incarnation");
        let now = lead_actor();
        // The mob was created again under the same id: the member's
        // generation there may even be the same number.
        let in_recreated_mob =
            WorkActor::mob_member(lead(), WorkMemberSeat::new(later_mob, 3)).expect("later actor");
        // The member was retired and spawned again, or reset or respawned,
        // inside the one living mob: the next generation of its identity.
        let respawned_member = WorkActor::mob_member(lead(), WorkMemberSeat::new(incarnation(), 4))
            .expect("respawned actor");

        let pending = WorkOrderParty::mob_member(lead(), lead_seat()).expect("pinned party");
        let request = WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::accept());
        let digest_now = request.input_digest(&scope(), &now).expect("digest");
        for other in [&in_recreated_mob, &respawned_member] {
            // Same owner key, another party: not the intended acceptor of a
            // pending offer pinned to the first seat, and not the holder of
            // an order accepted in it.
            assert_eq!(now.owner_key(), other.owner_key());
            assert_ne!(now.party(), other.party());
            assert_ne!(pending, *other.party());
            assert_ne!(accepted_transfer().offerer, *other.party());
            assert_ne!(now.party().canonical(), other.party().canonical());
            // And another digest for the same request.
            assert_ne!(
                request.input_digest(&scope(), other).expect("digest"),
                digest_now
            );
            // A request names all of them the same way.
            assert_eq!(now.party().designation(), other.party().designation());
        }
        assert_ne!(in_recreated_mob.party(), respawned_member.party());

        // The seat the order pinned still matches the actor built in it.
        assert_eq!(pending, *now.party());

        // A counterpart admitted under a later generation of its
        // declaration is another party too.
        let redeclared = WorkActor::counterpart(
            exchange(),
            5,
            WorkCounterpartPartyRef::new("ops").expect("party ref"),
        );
        assert_ne!(counterpart_actor().party(), redeclared.party());
        assert_ne!(
            counterpart_actor().party().canonical(),
            redeclared.party().canonical()
        );
        assert_eq!(
            counterpart_actor().party().designation(),
            redeclared.party().designation()
        );
    }

    #[test]
    fn party_and_designation_keys_are_fixed_and_distinct() {
        let designation = WorkOrderDesignation::local(lead());
        assert_eq!(designation.canonical(), "local:agent:mob/alpha/agent/lead");
        let pinned = WorkOrderParty::mob_member(lead(), lead_seat()).expect("pinned party");
        assert_eq!(
            pinned.canonical(),
            "local@0190a7c8-1111-4222-8333-444455556666.3:agent:mob/alpha/agent/lead"
        );
        let principal = WorkOrderParty::local(WorkOwnerKey::principal("user-17").expect("key"))
            .expect("principal party");
        assert_eq!(principal.canonical(), "local@-:principal:user-17");
        let counterpart = counterpart_actor().party().clone();
        assert_eq!(counterpart.canonical(), "counterpart@4:ops-to-research:ops");
        let counterpart_designation = WorkOrderDesignation::counterpart(
            exchange(),
            WorkCounterpartPartyRef::new("ops").expect("party ref"),
        );
        assert_eq!(
            counterpart_designation.canonical(),
            "counterpart:ops-to-research:ops"
        );
        // An exchange id may hold a dot and digits; the generation sits
        // before the first colon, so the two cannot be confused.
        let dotted = WorkOrderDesignation::counterpart(
            WorkExchangeId::new("4.ops-to-research").expect("exchange id"),
            WorkCounterpartPartyRef::new("ops").expect("party ref"),
        );
        let keys = BTreeSet::from([
            designation.canonical(),
            pinned.canonical(),
            principal.canonical(),
            counterpart.canonical(),
            counterpart_designation.canonical(),
            dotted.canonical(),
        ]);
        assert_eq!(keys.len(), 6);
        assert_eq!(pinned.designation(), designation);
        assert_eq!(counterpart.designation(), counterpart_designation);
        assert!(designation.is_local());
        assert!(pinned.is_local());
        assert!(!counterpart.is_local());
        assert!(!counterpart_designation.is_local());

        assert_fixed_json(&pinned, &lead_party_json());
        assert_fixed_json(
            &principal,
            &json!({"kind": "local", "key": {"kind": "principal", "id": "user-17"}}),
        );
        assert_fixed_json(
            &counterpart,
            &json!({
                "kind": "counterpart",
                "exchange": "ops-to-research",
                "generation": 4,
                "party_ref": "ops"
            }),
        );
        assert_fixed_json(
            &designation,
            &json!({"kind": "local", "key": {"kind": "agent", "id": "mob/alpha/agent/lead"}}),
        );
        assert_fixed_json(
            &counterpart_designation,
            &json!({"kind": "counterpart", "exchange": "ops-to-research", "party_ref": "ops"}),
        );
        // A stored incarnation has one spelling, and a seat has no other
        // field.
        assert!(
            serde_json::from_value::<WorkOrderParty>(json!({
                "kind": "local",
                "key": {"kind": "agent", "id": "mob/alpha/agent/lead"},
                "seat": {
                    "mob_incarnation": "0190A7C8-1111-4222-8333-444455556666",
                    "member_generation": 3
                }
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<WorkMemberSeat>(json!({
                "mob_incarnation": INCARNATION,
                "member_generation": 3,
                "fence_token": 9
            }))
            .is_err()
        );
        assert_fixed_json(
            &lead_seat(),
            &json!({"mob_incarnation": INCARNATION, "member_generation": 3}),
        );
    }

    #[test]
    fn a_designation_has_no_place_for_what_the_host_binds() {
        // Neither a seat nor a declaration generation can ride a request:
        // each is an unknown field of a designation, as is anything else.
        for (smuggled, value) in [
            (
                "seat",
                json!({"mob_incarnation": INCARNATION, "member_generation": 1}),
            ),
            ("mob_incarnation", json!(INCARNATION)),
            ("member_generation", json!(1)),
            ("generation", json!(4)),
            ("source", json!("mob_member")),
        ] {
            let mut local = reviewer_designation_json();
            local
                .as_object_mut()
                .expect("designation object")
                .insert(smuggled.to_string(), value.clone());
            assert!(
                serde_json::from_value::<WorkOrderDesignation>(local).is_err(),
                "a local designation accepted `{smuggled}`"
            );
            let mut counterpart =
                json!({"kind": "counterpart", "exchange": "ops-to-research", "party_ref": "ops"});
            counterpart
                .as_object_mut()
                .expect("designation object")
                .insert(smuggled.to_string(), value);
            assert!(
                serde_json::from_value::<WorkOrderDesignation>(counterpart).is_err(),
                "a counterpart designation accepted `{smuggled}`"
            );
        }
    }

    #[test]
    fn a_stored_party_tolerates_a_later_key_and_a_designation_refuses_one() {
        // A record may have been written by a later release. A key that
        // release added at the party's own level is passed over, and the
        // party decodes to what this build knows of it.
        for party in [
            reviewer_actor().party().clone(),
            counterpart_actor().party().clone(),
        ] {
            let mut stored = serde_json::to_value(&party).expect("party json");
            stored
                .as_object_mut()
                .expect("party object")
                .insert("added_later".to_string(), json!("x"));
            assert_eq!(
                serde_json::from_value::<WorkOrderParty>(stored).expect("stored party"),
                party
            );
        }
        // A designation comes from a request: the same key is refused.
        let mut requested = reviewer_designation_json();
        requested
            .as_object_mut()
            .expect("designation object")
            .insert("added_later".to_string(), json!("x"));
        assert!(serde_json::from_value::<WorkOrderDesignation>(requested).is_err());
    }

    #[test]
    fn order_record_round_trips_fixed_json() {
        let order = accepted_transfer();
        assert_fixed_json(&order, &accepted_transfer_json());
        assert_eq!(order.holder(), Some(reviewer_actor().party()));
        assert!(order.is_live());
        assert_eq!(
            order
                .decision(WorkOrderStep::Decision)
                .map(|decision| decision.input),
            Some(WorkOrderInputKind::Accept)
        );
        assert!(order.decision(WorkOrderStep::Closure).is_none());
    }

    /// The receiving half of an exchanged assign offer, as its offer left
    /// it: a counterpart offers, nothing is created yet, and the parent is
    /// preserved.
    fn offered_child() -> WorkOrder {
        let mut order = accepted_transfer();
        order.kind = WorkOrderKind::Assign;
        order.version = 1;
        order.offerer = counterpart_actor().party().clone();
        order.subject = WorkOrderSubject::Proposed(
            ProposedWorkItem::new("Review the notes").with_description("Second pass"),
        );
        order.parent = Some(
            WorkOrderParent::new(WorkItemId::new("work_0000").expect("parent item"))
                .with_order(WorkOrderId::new("order_0000").expect("parent order")),
        );
        order.terms = WorkOrderTerms::default();
        order.state = WorkOrderState::Offered;
        order.supersedes = Some(WorkOrderId::new("order_older").expect("older order"));
        order.counterpart_order =
            Some(WorkCounterpartOrderRef::new("order_remote_9").expect("counterpart order"));
        order.decisions.truncate(1);
        order.decisions[0].actor = Some(counterpart_actor().record());
        order.updated_at = at(0);
        order
    }

    #[test]
    fn offered_child_order_of_an_exchange_round_trips_fixed_json() {
        let order = offered_child();

        let mut fixed = accepted_transfer_json();
        let object = fixed.as_object_mut().expect("order object");
        object.insert("kind".into(), json!("assign"));
        object.insert("version".into(), json!(1));
        let counterpart = json!({
            "kind": "counterpart",
            "exchange": "ops-to-research",
            "generation": 4,
            "party_ref": "ops"
        });
        object.insert("offerer".into(), counterpart.clone());
        object.insert(
            "subject".into(),
            json!({"proposed": {"title": "Review the notes", "description": "Second pass"}}),
        );
        object.insert(
            "parent".into(),
            json!({"item_id": "work_0000", "order_id": "order_0000"}),
        );
        object.remove("terms");
        object.insert("state".into(), json!("offered"));
        object.insert("supersedes".into(), json!("order_older"));
        object.insert("counterpart_order".into(), json!("order_remote_9"));
        object.insert("updated_at".into(), json!("2026-11-24T17:18:00Z"));
        let decisions = object
            .get_mut("decisions")
            .and_then(Value::as_array_mut)
            .expect("decisions");
        decisions.truncate(1);
        decisions[0]["actor"] = json!({"party": counterpart, "source": "counterpart"});

        assert_fixed_json(&order, &fixed);
        assert_eq!(order.subject.item_id(), None);
        assert_eq!(order.holder(), None);
        assert!(order.is_live());
    }

    #[test]
    fn ended_orders_round_trip_fixed_json() {
        let mut closed = accepted_transfer();
        closed.version = 3;
        closed.state = WorkOrderState::Closed;
        closed.closure = Some(WorkOrderClosure {
            outcome: WorkOrderCloseOutcome::Completed,
            evidence: vec![
                WorkOrderEvidenceRef::item("ev-1"),
                WorkOrderEvidenceRef::counterpart_digest("report", "sha256:cc"),
            ],
            successor: None,
        });
        closed.terminal_at = Some(at(45));
        let mut fixed = accepted_transfer_json();
        let object = fixed.as_object_mut().expect("order object");
        object.insert("version".into(), json!(3));
        object.insert("state".into(), json!("closed"));
        object.insert(
            "closure".into(),
            json!({
                "outcome": "completed",
                "evidence": [
                    {"kind": "item", "evidence_id": "ev-1"},
                    {"kind": "counterpart_digest", "evidence_kind": "report", "digest": "sha256:cc"}
                ]
            }),
        );
        object.insert("terminal_at".into(), json!("2026-11-24T17:18:45Z"));
        assert_fixed_json(&closed, &fixed);
        assert_eq!(closed.holder(), None);
        assert!(!closed.is_live());

        // A hand-over names the order the item was handed on under.
        assert_fixed_json(
            &WorkOrderClosure {
                outcome: WorkOrderCloseOutcome::HandedOver,
                evidence: Vec::new(),
                successor: Some(WorkOrderId::new("order_0002").expect("successor")),
            },
            &json!({"outcome": "handed_over", "successor": "order_0002"}),
        );

        // A replaced order names its successor.
        let mut replaced = offered_child();
        replaced.state = WorkOrderState::Superseded;
        replaced.superseded_by = Some(WorkOrderId::new("order_0002").expect("successor"));
        let wire = serde_json::to_value(&replaced).expect("replaced order");
        assert_eq!(wire["state"], json!("superseded"));
        assert_eq!(wire["superseded_by"], json!("order_0002"));
        assert_eq!(
            serde_json::from_value::<WorkOrder>(wire).expect("replaced order"),
            replaced
        );

        // What a commit records when no party acted: a lapse with its
        // cause, an expiry, a hand-over. None has an actor or an identity.
        let lapse = WorkOrderDecision {
            step: WorkOrderStep::Closure,
            input: WorkOrderInputKind::Lapse,
            actor: None,
            authority: grant(),
            identity: None,
            order_version: 3,
            item_revision_before: Some(8),
            item_revision_after: Some(9),
            reason: None,
            lapse_cause: Some(WorkOrderLapseCause::HolderClaimEnded),
            at: at(50),
        };
        let unattended = json!({
            "step": "closure",
            "input": "lapse",
            "authority": {"kind": "namespace_grant", "realm_id": "mob.alpha", "namespace": "default"},
            "order_version": 3,
            "item_revision_before": 8,
            "item_revision_after": 9,
            "lapse_cause": "holder_claim_ended",
            "at": "2026-11-24T17:18:50Z"
        });
        assert_fixed_json(&lapse, &unattended);

        let expiry = WorkOrderDecision {
            step: WorkOrderStep::Decision,
            input: WorkOrderInputKind::Expire,
            actor: None,
            authority: grant(),
            identity: None,
            order_version: 2,
            item_revision_before: None,
            item_revision_after: None,
            reason: None,
            lapse_cause: None,
            at: at(50),
        };
        assert_fixed_json(
            &expiry,
            &json!({
                "step": "decision",
                "input": "expire",
                "authority": {"kind": "namespace_grant", "realm_id": "mob.alpha", "namespace": "default"},
                "order_version": 2,
                "at": "2026-11-24T17:18:50Z"
            }),
        );

        let hand_over = WorkOrderDecision {
            input: WorkOrderInputKind::HandOver,
            lapse_cause: None,
            ..lapse
        };
        let mut handed = unattended;
        let object = handed.as_object_mut().expect("decision object");
        object.insert("input".into(), json!("hand_over"));
        object.remove("lapse_cause");
        assert_fixed_json(&hand_over, &handed);
    }

    #[test]
    fn order_event_with_its_transfer_record_round_trips_fixed_json() {
        let event = WorkOrderEvent {
            seq: Some(12),
            realm_id: "mob.alpha".to_string(),
            namespace: WorkNamespace::default(),
            kind: WorkOrderEventKind::Accepted,
            at: at(30),
            order: accepted_transfer(),
            transfer: Some(WorkOrderTransfer {
                item_id: item_id(),
                from_owner: lead(),
                to_owner: reviewer(),
                item_revision_before: 7,
                item_revision_after: 8,
                order_version_before: 1,
                order_version_after: 2,
                authority: WorkAuthorityBasis::HostFence {
                    reference: "declaration/7".to_string(),
                },
            }),
        };
        let fixed = json!({
            "seq": 12,
            "realm_id": "mob.alpha",
            "namespace": "default",
            "kind": "accepted",
            "at": "2026-11-24T17:18:30Z",
            "order": accepted_transfer_json(),
            "transfer": {
                "item_id": "work_0001",
                "from_owner": {"kind": "agent", "id": "mob/alpha/agent/lead"},
                "to_owner": {"kind": "agent", "id": "mob/alpha/agent/reviewer"},
                "item_revision_before": 7,
                "item_revision_after": 8,
                "order_version_before": 1,
                "order_version_after": 2,
                "authority": {"kind": "host_fence", "reference": "declaration/7"}
            }
        });
        assert_fixed_json(&event, &fixed);

        // A ledger entry of a kind this build does not know is refused
        // whole. It is never read as an entry without a kind, and never
        // skipped: a projection rebuilt past it would be silently wrong.
        let mut later_kind = fixed;
        later_kind["kind"] = json!("severed");
        assert!(serde_json::from_value::<WorkOrderEvent>(later_kind).is_err());
    }

    #[test]
    fn a_receipt_is_rebuilt_from_the_decision_record_alone() {
        let order = accepted_transfer();
        let accept = order
            .receipt(WorkOrderStep::Decision)
            .expect("accept receipt");
        let accept_json = json!({
            "order_id": "order_0001",
            "step": "decision",
            "input": "accept",
            "identity": {"key": "accept-key", "request_digest": "v1:sha256:bb"},
            "order_version": 2,
            "state": "accepted",
            "item": {"item_id": "work_0001", "revision_before": 7, "revision_after": 8},
            "at": "2026-11-24T17:18:30Z"
        });
        assert_fixed_json(&accept, &accept_json);

        // The offer touched no item.
        let offer = order.receipt(WorkOrderStep::Offer).expect("offer receipt");
        assert_fixed_json(
            &offer,
            &json!({
                "order_id": "order_0001",
                "step": "offer",
                "input": "offer",
                "identity": {"key": "offer-key", "request_digest": "v1:sha256:aa"},
                "order_version": 1,
                "state": "offered",
                "at": "2026-11-24T17:18:00Z"
            }),
        );
        assert!(order.receipt(WorkOrderStep::Closure).is_none());

        // The receipt of the acceptance does not change when the order
        // moves on: a replay after the closure is answered with the same
        // value.
        let mut later = order;
        later.version = 3;
        later.state = WorkOrderState::Closed;
        later.updated_at = at(45);
        later.terminal_at = Some(at(45));
        assert_eq!(later.receipt(WorkOrderStep::Decision), Some(accept));

        // A step no party took has no receipt.
        let mut lapsed = accepted_transfer();
        lapsed.decisions.push(WorkOrderDecision {
            step: WorkOrderStep::Closure,
            input: WorkOrderInputKind::Lapse,
            actor: None,
            authority: grant(),
            identity: None,
            order_version: 3,
            item_revision_before: None,
            item_revision_after: None,
            reason: None,
            lapse_cause: Some(WorkOrderLapseCause::ItemEndedOutsideOrder),
            at: at(50),
        });
        assert!(lapsed.receipt(WorkOrderStep::Closure).is_none());
    }

    #[test]
    fn an_assign_offer_keeps_its_receipt_across_the_acceptance() {
        // Offered: the subject is proposed content and there is no item.
        let offered = offered_child();
        let offer_receipt = offered
            .receipt(WorkOrderStep::Offer)
            .expect("offer receipt");
        assert_eq!(offer_receipt.item, None);
        assert_eq!(offer_receipt.state, WorkOrderState::Offered);

        // Accepted: the commit created the item and the subject is now it.
        let mut accepted = offered;
        accepted.version = 2;
        accepted.state = WorkOrderState::Accepted;
        accepted.subject = WorkOrderSubject::item(item_id(), 2);
        accepted.decisions.push(WorkOrderDecision {
            step: WorkOrderStep::Decision,
            input: WorkOrderInputKind::Accept,
            actor: Some(reviewer_actor().record()),
            authority: grant(),
            identity: Some(admission("accept-key", "v1:sha256:bb")),
            order_version: 2,
            item_revision_before: None,
            item_revision_after: Some(2),
            reason: None,
            lapse_cause: None,
            at: at(30),
        });
        accepted.updated_at = at(30);

        // A replay of the offer's key is answered with the receipt the
        // offer had, although the order now has an item.
        assert_eq!(accepted.receipt(WorkOrderStep::Offer), Some(offer_receipt));
        // The acceptance created the item: no revision before it.
        let accept_receipt = accepted
            .receipt(WorkOrderStep::Decision)
            .expect("accept receipt");
        assert_fixed_json(
            &accept_receipt.item.expect("item change"),
            &json!({"item_id": "work_0001", "revision_after": 2}),
        );
    }

    #[test]
    fn a_replacement_has_one_receipt_and_the_new_order_owns_it() {
        let older = WorkOrderId::new("order_older").expect("older order");
        let replacing = admission("replace-key", "v1:sha256:dd");

        // The new order: its offer step holds the key and names the order
        // it replaced.
        let mut successor = accepted_transfer();
        successor.supersedes = Some(older.clone());
        successor.decisions[0].identity = Some(replacing.clone());
        let receipt = successor
            .receipt(WorkOrderStep::Offer)
            .expect("the replacement's receipt");
        assert_eq!(receipt.identity, replacing);
        assert_eq!(receipt.supersedes, Some(older.clone()));
        assert_eq!(receipt.order_id, order_id());
        // Only the offer names it.
        assert_eq!(
            successor
                .receipt(WorkOrderStep::Decision)
                .and_then(|receipt| receipt.supersedes),
            None
        );

        // The replaced order: its decision record holds the same key and
        // digest, and it has NO receipt for it. One key, one receipt.
        let mut replaced = accepted_transfer();
        replaced.id = older;
        replaced.version = 2;
        replaced.state = WorkOrderState::Superseded;
        replaced.superseded_by = Some(order_id());
        replaced.decisions.truncate(1);
        replaced.decisions.push(WorkOrderDecision {
            step: WorkOrderStep::Decision,
            input: WorkOrderInputKind::Supersede,
            actor: Some(lead_actor().record()),
            authority: grant(),
            identity: Some(replacing),
            order_version: 2,
            item_revision_before: None,
            item_revision_after: None,
            reason: None,
            lapse_cause: None,
            at: at(30),
        });
        assert_eq!(
            replaced
                .decision(WorkOrderStep::Decision)
                .map(|decision| decision.input),
            Some(WorkOrderInputKind::Supersede)
        );
        assert_eq!(replaced.receipt(WorkOrderStep::Decision), None);
        // Its own offer keeps its own receipt under its own key.
        assert!(replaced.receipt(WorkOrderStep::Offer).is_some());
    }

    #[test]
    fn the_three_outcomes_round_trip_fixed_json() {
        let receipt = accepted_transfer()
            .receipt(WorkOrderStep::Decision)
            .expect("accept receipt");
        let receipt_json = serde_json::to_value(&receipt).expect("receipt json");

        let committed = WorkOrderOutcome::Committed {
            receipt: receipt.clone(),
        };
        assert_fixed_json(
            &committed,
            &json!({"outcome": "committed", "receipt": receipt_json}),
        );
        let replayed = WorkOrderOutcome::Replayed { receipt };
        assert_fixed_json(
            &replayed,
            &json!({"outcome": "replayed", "receipt": receipt_json}),
        );
        // A replay hands back the first receipt, unchanged.
        assert_eq!(committed.receipt(), replayed.receipt());

        let conflict = WorkOrderOutcome::Conflict {
            input_key: WorkAdmissionKey::new("accept-key").expect("key"),
            order_id: order_id(),
            step: WorkOrderStep::Decision,
        };
        assert_fixed_json(
            &conflict,
            &json!({
                "outcome": "conflict",
                "input_key": "accept-key",
                "order_id": "order_0001",
                "step": "decision"
            }),
        );
        assert!(conflict.receipt().is_none());
    }

    /// The place of each refusal in [`WORK_ORDER_REFUSAL_CODES`]. The match
    /// is exhaustive, so a variant added to the enum does not compile until
    /// it has a place here, and the row below then requires its code in the
    /// list and a sample.
    const fn refusal_index(refusal: &WorkOrderRefusal) -> usize {
        match refusal {
            WorkOrderRefusal::ActorRequired => 0,
            WorkOrderRefusal::NotIntendedAcceptor => 1,
            WorkOrderRefusal::NotOfferer => 2,
            WorkOrderRefusal::NotHolder { .. } => 3,
            WorkOrderRefusal::StaleOrderVersion { .. } => 4,
            WorkOrderRefusal::OrderStateMismatch { .. } => 5,
            WorkOrderRefusal::OfferExpired { .. } => 6,
            WorkOrderRefusal::ItemRevisionMoved { .. } => 7,
            WorkOrderRefusal::OldOwnerMismatch { .. } => 8,
            WorkOrderRefusal::OldOwnerLeaseExpired { .. } => 9,
            WorkOrderRefusal::ItemNotTransferable { .. } => 10,
            WorkOrderRefusal::AcceptorNotSeated => 11,
            WorkOrderRefusal::ParentNotHeld { .. } => 12,
            WorkOrderRefusal::OrderPending { .. } => 13,
            WorkOrderRefusal::OrderHeld { .. } => 14,
            WorkOrderRefusal::OrderCloseRequired { .. } => 15,
            WorkOrderRefusal::EvidenceRequired => 16,
            WorkOrderRefusal::EvidenceUnknown { .. } => 17,
            WorkOrderRefusal::AttentionTargetOccupied => 18,
            WorkOrderRefusal::AuthorityNotLive => 19,
            WorkOrderRefusal::AuthorityFenceUnsupported => 20,
            WorkOrderRefusal::ProtocolUnsupported { .. } => 21,
            WorkOrderRefusal::CustodyStaysLocal => 22,
            WorkOrderRefusal::CounterpartDecides { .. } => 23,
            WorkOrderRefusal::OrderNotFound { .. } => 24,
        }
    }

    #[test]
    fn refusal_vocabulary_is_pinned_code_by_code() {
        let refusals: Vec<(WorkOrderRefusal, Value)> = vec![
            (
                WorkOrderRefusal::ActorRequired,
                json!({"code": "actor_required"}),
            ),
            (
                WorkOrderRefusal::NotIntendedAcceptor,
                json!({"code": "not_intended_acceptor"}),
            ),
            (WorkOrderRefusal::NotOfferer, json!({"code": "not_offerer"})),
            (
                WorkOrderRefusal::NotHolder {
                    order_id: order_id(),
                    holder: reviewer_actor().party().clone(),
                },
                json!({
                    "code": "not_holder",
                    "order_id": "order_0001",
                    "holder": reviewer_party_json()
                }),
            ),
            (
                WorkOrderRefusal::StaleOrderVersion {
                    order_id: order_id(),
                    expected: 1,
                    current: 2,
                },
                json!({"code": "stale_order_version", "order_id": "order_0001", "expected": 1, "current": 2}),
            ),
            (
                WorkOrderRefusal::OrderStateMismatch {
                    order_id: order_id(),
                    state: WorkOrderState::Accepted,
                    version: 2,
                    holder: Some(reviewer_actor().party().clone()),
                },
                json!({
                    "code": "order_state_mismatch",
                    "order_id": "order_0001",
                    "state": "accepted",
                    "version": 2,
                    "holder": reviewer_party_json()
                }),
            ),
            (
                WorkOrderRefusal::OfferExpired {
                    order_id: order_id(),
                    expired_at: at(0),
                },
                json!({"code": "offer_expired", "order_id": "order_0001", "expired_at": "2026-11-24T17:18:00Z"}),
            ),
            (
                WorkOrderRefusal::ItemRevisionMoved {
                    item_id: item_id(),
                    pinned: 7,
                    current: 9,
                },
                json!({"code": "item_revision_moved", "item_id": "work_0001", "pinned": 7, "current": 9}),
            ),
            (
                WorkOrderRefusal::OldOwnerMismatch { item_id: item_id() },
                json!({"code": "old_owner_mismatch", "item_id": "work_0001"}),
            ),
            (
                WorkOrderRefusal::OldOwnerLeaseExpired { item_id: item_id() },
                json!({"code": "old_owner_lease_expired", "item_id": "work_0001"}),
            ),
            (
                WorkOrderRefusal::ItemNotTransferable {
                    item_id: item_id(),
                    status: WorkStatus::Blocked,
                },
                json!({"code": "item_not_transferable", "item_id": "work_0001", "status": "blocked"}),
            ),
            (
                WorkOrderRefusal::AcceptorNotSeated,
                json!({"code": "acceptor_not_seated"}),
            ),
            (
                WorkOrderRefusal::ParentNotHeld { item_id: item_id() },
                json!({"code": "parent_not_held", "item_id": "work_0001"}),
            ),
            (
                WorkOrderRefusal::OrderPending {
                    item_id: item_id(),
                    order_id: order_id(),
                },
                json!({"code": "order_pending", "item_id": "work_0001", "order_id": "order_0001"}),
            ),
            (
                WorkOrderRefusal::OrderHeld {
                    item_id: item_id(),
                    order_id: order_id(),
                },
                json!({"code": "order_held", "item_id": "work_0001", "order_id": "order_0001"}),
            ),
            (
                WorkOrderRefusal::OrderCloseRequired {
                    item_id: item_id(),
                    order_id: order_id(),
                },
                json!({"code": "order_close_required", "item_id": "work_0001", "order_id": "order_0001"}),
            ),
            (
                WorkOrderRefusal::EvidenceRequired,
                json!({"code": "evidence_required"}),
            ),
            (
                WorkOrderRefusal::EvidenceUnknown {
                    evidence_id: "ev-9".to_string(),
                },
                json!({"code": "evidence_unknown", "evidence_id": "ev-9"}),
            ),
            (
                WorkOrderRefusal::AttentionTargetOccupied,
                json!({"code": "attention_target_occupied"}),
            ),
            (
                WorkOrderRefusal::AuthorityNotLive,
                json!({"code": "authority_not_live"}),
            ),
            (
                WorkOrderRefusal::AuthorityFenceUnsupported,
                json!({"code": "authority_fence_unsupported"}),
            ),
            (
                WorkOrderRefusal::ProtocolUnsupported {
                    offered: 2,
                    supported: 1,
                },
                json!({"code": "protocol_unsupported", "offered": 2, "supported": 1}),
            ),
            (
                WorkOrderRefusal::CustodyStaysLocal,
                json!({"code": "custody_stays_local"}),
            ),
            (
                WorkOrderRefusal::CounterpartDecides {
                    order_id: order_id(),
                },
                json!({"code": "counterpart_decides", "order_id": "order_0001"}),
            ),
            (
                WorkOrderRefusal::OrderNotFound {
                    order_id: order_id(),
                },
                json!({"code": "order_not_found", "order_id": "order_0001"}),
            ),
        ];

        // One sample per variant, each at the place the exhaustive match
        // gives it, each place holding the variant's code, and no place of
        // the list left without a sample.
        assert_eq!(WORK_ORDER_REFUSAL_CODES.len(), 25);
        assert_eq!(refusals.len(), WORK_ORDER_REFUSAL_CODES.len());
        for (place, (refusal, fixed)) in refusals.iter().enumerate() {
            assert_eq!(refusal_index(refusal), place, "{refusal:?}");
            assert_eq!(WORK_ORDER_REFUSAL_CODES[place], refusal.code());
            assert_fixed_json(refusal, fixed);
            assert_eq!(fixed["code"], json!(refusal.code()));
            assert!(!refusal.to_string().is_empty());
        }
        let unique: BTreeSet<&str> = WORK_ORDER_REFUSAL_CODES.iter().copied().collect();
        assert_eq!(unique.len(), WORK_ORDER_REFUSAL_CODES.len());

        assert!(serde_json::from_value::<WorkOrderRefusal>(json!({"code": "not_a_code"})).is_err());
        // The holder is named only while there is one.
        assert_fixed_json(
            &WorkOrderRefusal::OrderStateMismatch {
                order_id: order_id(),
                state: WorkOrderState::Declined,
                version: 2,
                holder: None,
            },
            &json!({"code": "order_state_mismatch", "order_id": "order_0001", "state": "declined", "version": 2}),
        );
    }

    #[test]
    fn state_kind_step_and_input_vocabularies_are_pinned() {
        fn names<T: Serialize + Copy>(values: &[T], as_str: fn(T) -> &'static str) -> Vec<String> {
            values
                .iter()
                .map(|value| {
                    let wire = serde_json::to_value(value).expect("serialize");
                    assert_eq!(wire, json!(as_str(*value)));
                    as_str(*value).to_string()
                })
                .collect()
        }

        let states = [
            WorkOrderState::Offered,
            WorkOrderState::Accepted,
            WorkOrderState::Declined,
            WorkOrderState::Withdrawn,
            WorkOrderState::Superseded,
            WorkOrderState::Expired,
            WorkOrderState::Closed,
            WorkOrderState::Lapsed,
        ];
        assert_eq!(
            names(&states, WorkOrderState::as_str),
            [
                "offered",
                "accepted",
                "declined",
                "withdrawn",
                "superseded",
                "expired",
                "closed",
                "lapsed"
            ]
        );
        let live: Vec<WorkOrderState> = states
            .iter()
            .copied()
            .filter(|state| state.is_live())
            .collect();
        assert_eq!(live, [WorkOrderState::Offered, WorkOrderState::Accepted]);
        assert!(
            states
                .iter()
                .all(|state| state.is_live() != state.is_terminal())
        );

        assert_eq!(
            names(
                &[WorkOrderKind::Transfer, WorkOrderKind::Assign],
                WorkOrderKind::as_str
            ),
            ["transfer", "assign"]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderStep::Offer,
                    WorkOrderStep::Decision,
                    WorkOrderStep::Closure
                ],
                WorkOrderStep::as_str
            ),
            ["offer", "decision", "closure"]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderCloseOutcome::Completed,
                    WorkOrderCloseOutcome::Failed,
                    WorkOrderCloseOutcome::Cancelled,
                    WorkOrderCloseOutcome::HandedOver
                ],
                WorkOrderCloseOutcome::as_str
            ),
            ["completed", "failed", "cancelled", "handed_over"]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderStanding::Current,
                    WorkOrderStanding::OfferExpiredUnobserved,
                    WorkOrderStanding::HolderLapsedUnobserved,
                    WorkOrderStanding::ItemEndedUnobserved
                ],
                WorkOrderStanding::as_str
            ),
            [
                "current",
                "offer_expired_unobserved",
                "holder_lapsed_unobserved",
                "item_ended_unobserved"
            ]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderLapseCause::HolderClaimEnded,
                    WorkOrderLapseCause::ItemEndedOutsideOrder
                ],
                WorkOrderLapseCause::as_str
            ),
            ["holder_claim_ended", "item_ended_outside_order"]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderReasonCode::Unspecified,
                    WorkOrderReasonCode::ProtocolUnsupported,
                    WorkOrderReasonCode::ExchangeWithdrawn
                ],
                WorkOrderReasonCode::as_str
            ),
            ["unspecified", "protocol_unsupported", "exchange_withdrawn"]
        );
        assert_eq!(
            names(
                &[
                    WorkActorSource::MobMember,
                    WorkActorSource::HostPrincipal,
                    WorkActorSource::Counterpart
                ],
                WorkActorSource::as_str
            ),
            ["mob_member", "host_principal", "counterpart"]
        );
        assert_eq!(
            names(
                &[
                    WorkOrderEventKind::Offered,
                    WorkOrderEventKind::Accepted,
                    WorkOrderEventKind::Declined,
                    WorkOrderEventKind::Withdrawn,
                    WorkOrderEventKind::Superseded,
                    WorkOrderEventKind::Expired,
                    WorkOrderEventKind::Closed,
                    WorkOrderEventKind::Lapsed
                ],
                WorkOrderEventKind::as_str
            ),
            [
                "offered",
                "accepted",
                "declined",
                "withdrawn",
                "superseded",
                "expired",
                "closed",
                "lapsed"
            ]
        );

        // Each input: its wire name, its step, the state it leaves.
        let inputs = [
            (
                WorkOrderInputKind::Offer,
                "offer",
                WorkOrderStep::Offer,
                WorkOrderState::Offered,
            ),
            (
                WorkOrderInputKind::Accept,
                "accept",
                WorkOrderStep::Decision,
                WorkOrderState::Accepted,
            ),
            (
                WorkOrderInputKind::Decline,
                "decline",
                WorkOrderStep::Decision,
                WorkOrderState::Declined,
            ),
            (
                WorkOrderInputKind::Withdraw,
                "withdraw",
                WorkOrderStep::Decision,
                WorkOrderState::Withdrawn,
            ),
            (
                WorkOrderInputKind::Supersede,
                "supersede",
                WorkOrderStep::Decision,
                WorkOrderState::Superseded,
            ),
            (
                WorkOrderInputKind::Close,
                "close",
                WorkOrderStep::Closure,
                WorkOrderState::Closed,
            ),
            (
                WorkOrderInputKind::HandOver,
                "hand_over",
                WorkOrderStep::Closure,
                WorkOrderState::Closed,
            ),
            (
                WorkOrderInputKind::Expire,
                "expire",
                WorkOrderStep::Decision,
                WorkOrderState::Expired,
            ),
            (
                WorkOrderInputKind::Lapse,
                "lapse",
                WorkOrderStep::Closure,
                WorkOrderState::Lapsed,
            ),
        ];
        for (input, name, step, state) in inputs {
            assert_eq!(serde_json::to_value(input).expect("serialize"), json!(name));
            assert_eq!(input.as_str(), name);
            assert_eq!(input.step(), step, "{name}");
            assert_eq!(input.resulting_state(), state, "{name}");
            // Only an offer and an acceptance leave the order live.
            assert_eq!(
                state.is_terminal(),
                !matches!(
                    input,
                    WorkOrderInputKind::Offer | WorkOrderInputKind::Accept
                ),
                "{name}"
            );
        }
        // What a request can ask for maps onto four of them.
        assert_eq!(WorkOrderInput::accept().kind(), WorkOrderInputKind::Accept);
        assert_eq!(
            WorkOrderInput::decline(WorkOrderReason::default()).kind(),
            WorkOrderInputKind::Decline
        );
        assert_eq!(
            WorkOrderInput::withdraw().kind(),
            WorkOrderInputKind::Withdraw
        );
        assert_eq!(
            WorkOrderInput::close(WorkOrderCloseOutcome::Failed, Vec::new()).kind(),
            WorkOrderInputKind::Close
        );
    }

    #[test]
    fn requests_round_trip_fixed_json_and_refuse_unknown_fields() {
        let offer = OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 7)
            .with_terms(WorkOrderTerms::default().with_holder_lease_seconds(3600));
        let offer_json = json!({
            "kind": "transfer",
            "acceptor": reviewer_designation_json(),
            "subject": {"item": {"item_id": "work_0001", "item_revision": 7}},
            "terms": {"holder_lease_seconds": 3600},
            "protocol": 1
        });
        assert_fixed_json(&offer, &offer_json);

        // The protocol defaults to the current one when a caller omits it.
        let mut without_protocol = offer_json.clone();
        without_protocol
            .as_object_mut()
            .expect("offer object")
            .remove("protocol");
        assert_eq!(
            serde_json::from_value::<OfferWorkOrderRequest>(without_protocol).expect("offer"),
            offer
        );

        let assign = OfferWorkOrderRequest::assign(
            reviewer_designation(),
            ProposedWorkItem::new("Review the notes").with_description("Second pass"),
        )
        .with_parent(
            WorkOrderParent::new(WorkItemId::new("work_0000").expect("parent item"))
                .with_order(WorkOrderId::new("order_0000").expect("parent order")),
        )
        .with_scope("mob.alpha", WorkNamespace::default());
        assert_fixed_json(
            &assign,
            &json!({
                "realm_id": "mob.alpha",
                "namespace": "default",
                "kind": "assign",
                "acceptor": reviewer_designation_json(),
                "subject": {"proposed": {"title": "Review the notes", "description": "Second pass"}},
                "parent": {"item_id": "work_0000", "order_id": "order_0000"},
                "protocol": 1
            }),
        );

        let close = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(
                WorkOrderCloseOutcome::Completed,
                vec![WorkOrderEvidenceRef::item("ev-1")],
            ),
        );
        assert_fixed_json(
            &close,
            &json!({
                "order_id": "order_0001",
                "expected_version": 2,
                "input": {
                    "kind": "close",
                    "outcome": "completed",
                    "evidence": [{"kind": "item", "evidence_id": "ev-1"}]
                }
            }),
        );
        assert_fixed_json(
            &WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::accept()),
            &json!({"order_id": "order_0001", "expected_version": 1, "input": {"kind": "accept"}}),
        );
        assert_fixed_json(
            &WorkOrderInputRequest::new(
                order_id(),
                1,
                WorkOrderInput::withdraw_because(
                    WorkOrderReason::new(WorkOrderReasonCode::ExchangeWithdrawn)
                        .with_note("the pair was withdrawn"),
                ),
            ),
            &json!({
                "order_id": "order_0001",
                "expected_version": 1,
                "input": {
                    "kind": "withdraw",
                    "reason": {"code": "exchange_withdrawn", "note": "the pair was withdrawn"}
                }
            }),
        );

        // A caller cannot smuggle in what the commit assigns or the host
        // binds: every such name is an unknown field and the request is
        // refused whole, at the top of a request and inside each part.
        for smuggled in ["offerer", "actor", "id", "state", "version", "authority"] {
            let mut with_field = offer_json.clone();
            with_field
                .as_object_mut()
                .expect("offer object")
                .insert(smuggled.to_string(), json!("x"));
            assert!(
                serde_json::from_value::<OfferWorkOrderRequest>(with_field).is_err(),
                "offer accepted `{smuggled}`"
            );
            assert!(
                serde_json::from_value::<WorkOrderInputRequest>(json!({
                    "order_id": "order_0001",
                    "expected_version": 1,
                    "input": {"kind": "accept"},
                    smuggled: "x"
                }))
                .is_err(),
                "input request accepted `{smuggled}`"
            );
            // Inside the input itself, an acceptance included: it has no
            // fields, and takes none.
            for input in [
                json!({"kind": "accept"}),
                json!({"kind": "decline", "reason": {}}),
                json!({"kind": "withdraw"}),
                json!({"kind": "close", "outcome": "failed"}),
            ] {
                let mut with_field = input.clone();
                with_field
                    .as_object_mut()
                    .expect("input object")
                    .insert(smuggled.to_string(), json!("x"));
                assert!(
                    serde_json::from_value::<WorkOrderInput>(input).is_ok(),
                    "the plain input must decode"
                );
                assert!(
                    serde_json::from_value::<WorkOrderInput>(with_field.clone()).is_err(),
                    "input {with_field} accepted `{smuggled}`"
                );
            }
            // Inside the subject, the terms, a reason and an evidence
            // reference.
            for (part, value) in [
                (
                    "subject",
                    json!({"item": {"item_id": "work_0001", "item_revision": 7, smuggled: "x"}}),
                ),
                (
                    "terms",
                    json!({"holder_lease_seconds": 3600, smuggled: "x"}),
                ),
            ] {
                let mut with_field = offer_json.clone();
                with_field
                    .as_object_mut()
                    .expect("offer object")
                    .insert(part.to_string(), value);
                assert!(
                    serde_json::from_value::<OfferWorkOrderRequest>(with_field).is_err(),
                    "offer accepted `{smuggled}` inside its {part}"
                );
            }
            assert!(
                serde_json::from_value::<WorkOrderReason>(
                    json!({"code": "unspecified", smuggled: "x"})
                )
                .is_err()
            );
            assert!(
                serde_json::from_value::<WorkOrderEvidenceRef>(
                    json!({"kind": "item", "evidence_id": "ev-1", smuggled: "x"})
                )
                .is_err()
            );
        }
        // A hand-over cannot even be spelled as an input.
        assert!(
            serde_json::from_value::<WorkOrderInputRequest>(json!({
                "order_id": "order_0001",
                "expected_version": 1,
                "input": {"kind": "hand_over"}
            }))
            .is_err()
        );
    }

    #[test]
    fn filter_round_trips_fixed_json() {
        assert_fixed_json(&WorkOrderFilter::default(), &json!({}));
        // "Pending for me": the bound actor's own part. The filter holds no
        // party; the service takes it from the actor, seat included.
        let pending_for_me = WorkOrderFilter {
            mine: Some(WorkOrderRole::Acceptor),
            states: BTreeSet::from([WorkOrderState::Offered]),
            limit: Some(20),
            ..WorkOrderFilter::default()
        };
        assert_fixed_json(
            &pending_for_me,
            &json!({"mine": "acceptor", "states": ["offered"], "limit": 20}),
        );
        // Another party is named as a request names it: by designation.
        let offered_by_lead = WorkOrderFilter {
            offerer: Some(WorkOrderDesignation::local(lead())),
            acceptor: Some(reviewer_designation()),
            ..WorkOrderFilter::default()
        };
        assert_fixed_json(
            &offered_by_lead,
            &json!({
                "acceptor": reviewer_designation_json(),
                "offerer": {"kind": "local", "key": {"kind": "agent", "id": "mob/alpha/agent/lead"}}
            }),
        );
        // No read shape holds a seat or a generation: a pinned party in the
        // place of a designation is refused whole.
        assert!(
            serde_json::from_value::<WorkOrderFilter>(json!({"acceptor": reviewer_party_json()}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<WorkOrderFilter>(json!({"offerer": {
                "kind": "counterpart",
                "exchange": "ops-to-research",
                "generation": 4,
                "party_ref": "ops"
            }}))
            .is_err()
        );
        for (role, name) in [
            (WorkOrderRole::Acceptor, "acceptor"),
            (WorkOrderRole::Offerer, "offerer"),
            (WorkOrderRole::Holder, "holder"),
        ] {
            assert_eq!(role.as_str(), name);
            assert_eq!(serde_json::to_value(role).expect("serialize"), json!(name));
        }
        let children_and_halves = WorkOrderFilter {
            parent_item_id: Some(WorkItemId::new("work_0000").expect("parent item")),
            exchange: Some(exchange()),
            counterpart_order: Some(
                WorkCounterpartOrderRef::new("order_remote_9").expect("counterpart order"),
            ),
            ..WorkOrderFilter::default()
        };
        assert_fixed_json(
            &children_and_halves,
            &json!({
                "parent_item_id": "work_0000",
                "exchange": "ops-to-research",
                "counterpart_order": "order_remote_9"
            }),
        );
        assert!(serde_json::from_value::<WorkOrderFilter>(json!({"owner": "x"})).is_err());
    }

    fn refusal_of(result: Result<(), WorkOrderError>) -> WorkOrderRefusal {
        match result {
            Err(WorkOrderError::Refused(refusal)) => refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn is_invalid_input(result: Result<(), WorkOrderError>) -> bool {
        matches!(
            result,
            Err(WorkOrderError::Graph(WorkGraphError::InvalidInput(_)))
        )
    }

    #[test]
    fn offer_validation_binds_the_kind_to_its_subject_and_keeps_custody_local() {
        let transfer = OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 7);
        assert!(transfer.validate().is_ok());
        let assign =
            OfferWorkOrderRequest::assign(reviewer_designation(), ProposedWorkItem::new("Review"));
        assert!(assign.validate().is_ok());
        let parent = WorkOrderParent::new(WorkItemId::new("work_0000").expect("parent item"));
        assert!(
            assign
                .clone()
                .with_parent(parent.clone())
                .validate()
                .is_ok()
        );

        // Only an assign offer delegates a child.
        assert!(is_invalid_input(
            transfer.clone().with_parent(parent).validate()
        ));
        let mut transfer_of_proposed = assign;
        transfer_of_proposed.kind = WorkOrderKind::Transfer;
        assert!(is_invalid_input(transfer_of_proposed.validate()));
        let mut assign_of_existing = transfer.clone();
        assign_of_existing.kind = WorkOrderKind::Assign;
        assert!(is_invalid_input(assign_of_existing.validate()));

        let other_side = WorkOrderDesignation::counterpart(
            exchange(),
            WorkCounterpartPartyRef::new("intake").expect("party ref"),
        );
        assert_eq!(
            refusal_of(
                OfferWorkOrderRequest::transfer(other_side.clone(), item_id(), 7).validate()
            ),
            WorkOrderRefusal::CustodyStaysLocal
        );
        // Work may be assigned across an exchange; only custody may not cross.
        assert!(
            OfferWorkOrderRequest::assign(other_side, ProposedWorkItem::new("Review"))
                .validate()
                .is_ok()
        );

        // Only the counterpart's own offer names the other side's order.
        let inbound = golden_inbound();
        assert!(inbound.validate_for(&counterpart_actor()).is_ok());
        assert!(is_invalid_input(inbound.validate_for(&lead_actor())));
        // And a request that is refused for its shape has no digest.
        assert!(inbound.input_digest(&scope(), &lead_actor()).is_err());
        assert!(
            inbound
                .input_identity(
                    WorkAdmissionKey::new("k").expect("key"),
                    &scope(),
                    &lead_actor()
                )
                .is_err()
        );

        let mut newer_protocol = transfer.clone();
        newer_protocol.protocol = WORK_ORDER_PROTOCOL + 1;
        assert_eq!(
            refusal_of(newer_protocol.validate()),
            WorkOrderRefusal::ProtocolUnsupported {
                offered: WORK_ORDER_PROTOCOL + 1,
                supported: WORK_ORDER_PROTOCOL,
            }
        );

        for seconds in [0, MAX_WORK_CLAIM_LEASE_SECONDS + 1] {
            assert!(is_invalid_input(
                transfer
                    .clone()
                    .with_terms(WorkOrderTerms::default().with_holder_lease_seconds(seconds))
                    .validate()
            ));
        }
        assert!(
            transfer
                .with_terms(
                    WorkOrderTerms::default()
                        .with_holder_lease_seconds(MAX_WORK_CLAIM_LEASE_SECONDS)
                )
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn free_text_and_times_have_one_spelling_and_a_bound() {
        let transfer = OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 7);
        let with_note = |note: String| {
            transfer
                .clone()
                .with_terms(WorkOrderTerms::default().with_note(note))
                .validate()
        };
        // A note that is present is not empty: an empty one and an absent
        // one would have one meaning and two digests.
        assert!(is_invalid_input(with_note(String::new())));
        assert!(with_note("n".repeat(WORK_ORDER_NOTE_MAX_BYTES)).is_ok());
        assert!(is_invalid_input(with_note(
            "n".repeat(WORK_ORDER_NOTE_MAX_BYTES + 1)
        )));

        // An expiry is a whole number of milliseconds, which is what the
        // digest hashes: two offers that differ below that never share one.
        let whole = Utc
            .with_ymd_and_hms(2026, 12, 1, 0, 0, 0)
            .single()
            .expect("fixed expiry");
        let with_expiry = |at: DateTime<Utc>| {
            transfer
                .clone()
                .with_terms(WorkOrderTerms::default().with_offer_expires_at(at))
                .validate()
        };
        assert!(with_expiry(whole).is_ok());
        assert!(with_expiry(whole + chrono::Duration::milliseconds(250)).is_ok());
        assert!(is_invalid_input(with_expiry(
            whole + chrono::Duration::microseconds(250)
        )));
        assert!(is_invalid_input(with_expiry(
            whole + chrono::Duration::nanoseconds(1)
        )));
        // A time written as second 60 reads as the same instant as the
        // second after it and keeps another text: refused, so the two never
        // share a digest.
        let second_sixty = (whole - chrono::Duration::seconds(1))
            .with_nanosecond(1_000_000_000)
            .expect("a leap second");
        assert_eq!(second_sixty.timestamp_millis(), whole.timestamp_millis());
        assert_ne!(second_sixty, whole);
        assert!(is_invalid_input(with_expiry(second_sixty)));

        // A proposed item: a title that says something, within a bound; a
        // description that is absent or says something, within a bound.
        let assign = |item: ProposedWorkItem| {
            OfferWorkOrderRequest::assign(reviewer_designation(), item).validate()
        };
        assert!(is_invalid_input(assign(ProposedWorkItem::new("  "))));
        assert!(
            assign(ProposedWorkItem::new(
                "t".repeat(WORK_ORDER_TITLE_MAX_BYTES)
            ))
            .is_ok()
        );
        assert!(is_invalid_input(assign(ProposedWorkItem::new(
            "t".repeat(WORK_ORDER_TITLE_MAX_BYTES + 1)
        ))));
        assert!(is_invalid_input(assign(
            ProposedWorkItem::new("Review").with_description("")
        )));
        assert!(is_invalid_input(assign(
            ProposedWorkItem::new("Review")
                .with_description("d".repeat(WORK_ORDER_DESCRIPTION_MAX_BYTES + 1))
        )));

        // A reason's note follows the same rule.
        let decline = |reason: WorkOrderReason| {
            WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::decline(reason)).validate()
        };
        assert!(decline(WorkOrderReason::default()).is_ok());
        assert!(is_invalid_input(decline(
            WorkOrderReason::default().with_note("")
        )));
        assert!(is_invalid_input(decline(
            WorkOrderReason::default().with_note("n".repeat(WORK_ORDER_NOTE_MAX_BYTES + 1))
        )));
    }

    #[test]
    fn closure_validation_binds_completion_to_evidence() {
        let close = |outcome, evidence: Vec<WorkOrderEvidenceRef>| {
            WorkOrderInputRequest::new(order_id(), 2, WorkOrderInput::close(outcome, evidence))
        };
        let evidence = || vec![WorkOrderEvidenceRef::item("ev-1")];

        assert_eq!(
            refusal_of(close(WorkOrderCloseOutcome::Completed, Vec::new()).validate()),
            WorkOrderRefusal::EvidenceRequired
        );
        assert!(
            close(WorkOrderCloseOutcome::Completed, evidence())
                .validate()
                .is_ok()
        );
        // An unsuccessful closure needs none.
        assert!(
            close(WorkOrderCloseOutcome::Failed, Vec::new())
                .validate()
                .is_ok()
        );
        assert!(
            close(WorkOrderCloseOutcome::Cancelled, Vec::new())
                .validate()
                .is_ok()
        );
        // A hand-over is recorded by a successor's acceptance, never asked for.
        assert!(is_invalid_input(
            close(WorkOrderCloseOutcome::HandedOver, evidence()).validate()
        ));
        // An evidence reference says something, within a bound.
        for bad in [
            WorkOrderEvidenceRef::item(" "),
            WorkOrderEvidenceRef::item("e".repeat(WORK_ORDER_EVIDENCE_TEXT_MAX_BYTES + 1)),
            WorkOrderEvidenceRef::counterpart_digest("", "sha256:cc"),
            WorkOrderEvidenceRef::counterpart_digest("report", ""),
        ] {
            assert!(
                is_invalid_input(
                    close(WorkOrderCloseOutcome::Completed, vec![bad.clone()]).validate()
                ),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn the_form_of_closure_evidence_follows_who_closes() {
        let by_id = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(
                WorkOrderCloseOutcome::Completed,
                vec![WorkOrderEvidenceRef::item("ev-1")],
            ),
        );
        let by_digest = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(
                WorkOrderCloseOutcome::Completed,
                vec![WorkOrderEvidenceRef::counterpart_digest(
                    "report",
                    "sha256:cc",
                )],
            ),
        );

        // A holder of this graph names evidence on the item. A digest it
        // typed is evidence of nothing here.
        assert!(by_id.validate_for(&reviewer_actor()).is_ok());
        assert_eq!(
            refusal_of(by_digest.validate_for(&reviewer_actor())),
            WorkOrderRefusal::EvidenceUnknown {
                evidence_id: "sha256:cc".to_string(),
            }
        );
        // A counterpart's mirrored closure carries digests. An id of the
        // other graph names nothing here.
        assert!(by_digest.validate_for(&counterpart_actor()).is_ok());
        assert_eq!(
            refusal_of(by_id.validate_for(&counterpart_actor())),
            WorkOrderRefusal::EvidenceUnknown {
                evidence_id: "ev-1".to_string(),
            }
        );
        // The shape rules still come first, whoever asks.
        let empty = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(WorkOrderCloseOutcome::Completed, Vec::new()),
        );
        assert_eq!(
            refusal_of(empty.validate_for(&counterpart_actor())),
            WorkOrderRefusal::EvidenceRequired
        );
        // Other inputs have nothing that depends on the actor.
        assert!(
            WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::accept())
                .validate_for(&counterpart_actor())
                .is_ok()
        );
    }

    /// The offer every digest row below starts from.
    fn golden_offer() -> OfferWorkOrderRequest {
        OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 7).with_terms(
            WorkOrderTerms::default()
                .with_offer_expires_at(
                    Utc.with_ymd_and_hms(2026, 12, 1, 0, 0, 0)
                        .single()
                        .expect("fixed expiry"),
                )
                .with_holder_lease_seconds(3600)
                .with_note("handover of the release notes"),
        )
    }

    /// A delegated child across an exchange: every optional part present.
    fn golden_assign() -> OfferWorkOrderRequest {
        OfferWorkOrderRequest::assign(
            WorkOrderDesignation::counterpart(
                exchange(),
                WorkCounterpartPartyRef::new("intake").expect("party ref"),
            ),
            ProposedWorkItem::new("Review the notes").with_description("Second pass"),
        )
        .with_parent(
            WorkOrderParent::new(WorkItemId::new("work_0000").expect("parent item"))
                .with_order(WorkOrderId::new("order_0000").expect("parent order")),
        )
        .with_supersedes(WorkOrderId::new("order_older").expect("older order"))
    }

    /// The receiving half of the other side's order: offered by the
    /// counterpart actor, which alone may name the other side's order.
    fn golden_inbound() -> OfferWorkOrderRequest {
        OfferWorkOrderRequest::assign(
            reviewer_designation(),
            ProposedWorkItem::new("Review the notes"),
        )
        .with_counterpart_order(
            WorkCounterpartOrderRef::new("order_remote_9").expect("counterpart order"),
        )
    }

    fn canonical_text_of(body: &Value) -> String {
        String::from_utf8(canonical_input_bytes(body).expect("canonical bytes")).expect("utf-8")
    }

    // The eight fixed digests below were computed outside this crate, by
    // SHA-256 over the domain string, one zero byte and the canonical text
    // written out in each row. A change to what is hashed, to the order of
    // the keys or to the escaping fails these rows. Once the order surface
    // is released they must never be updated to follow such a change: a
    // change of what is hashed is a new form with a new name, and the form
    // named here keeps these values.

    const GOLDEN_OFFER_DIGEST: &str =
        "v1:sha256:79837652526f82e70314a90e57e34f2c4e8dceed5fc68981d1dcbe582bfbed15";
    const GOLDEN_ASSIGN_DIGEST: &str =
        "v1:sha256:fb553fecf9fa9471a7b0c13e6d938cc6e6664895b3868dbe708579802995eabb";
    const GOLDEN_INBOUND_DIGEST: &str =
        "v1:sha256:fff87600e21adf913b487b72ce28a7795665a4fd625d9e980261330ccbfc31da";
    const GOLDEN_ACCEPT_DIGEST: &str =
        "v1:sha256:d19f131146e53ac4ea0b7e5d1e04af613165d4ab6757fb8745c644f7233544f7";
    const GOLDEN_DECLINE_DIGEST: &str =
        "v1:sha256:db832877c61f423cbb2d47410df2f88a918a59d107308f0a656bf705ec8e176f";
    const GOLDEN_CLOSE_DIGEST: &str =
        "v1:sha256:0d64771d0cf42111933d513d7bfa2753332f71760d03ba4f32f89a55a2c793e7";
    const GOLDEN_MIRRORED_CLOSE_DIGEST: &str =
        "v1:sha256:46586689dfe7bc1f7e696a589a328a3baa27d38bdb38ea3db5d946d5fd902ed1";
    const GOLDEN_ESCAPES_DIGEST: &str =
        "v1:sha256:542fc6f5709f08ebc9bd6ff904b1bdd7961969e6a4c85c736cec921f0eace6ae";

    #[test]
    fn offer_input_digests_are_pinned_to_fixed_bytes() {
        let offer = golden_offer();
        assert_eq!(
            canonical_text_of(&offer.digest_body(&scope(), &lead_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.3:agent:mob/alpha/agent/lead","#,
                r#""content":{"acceptor":"local:agent:mob/alpha/agent/reviewer","#,
                r#""kind":"transfer","protocol":1,"#,
                r#""subject":{"item":{"item_id":"work_0001","item_revision":7}},"#,
                r#""terms":{"holder_lease_seconds":3600,"#,
                r#""note":"handover of the release notes","#,
                r#""offer_expires_at_ms":1796083200000}},"#,
                r#""input":"offer","namespace":"default","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            offer
                .input_digest(&scope(), &lead_actor())
                .expect("offer digest"),
            GOLDEN_OFFER_DIGEST
        );
        // The digest says which form made it.
        assert!(
            GOLDEN_OFFER_DIGEST.starts_with(&format!("{WORK_ORDER_INPUT_DIGEST_FORM}:sha256:"))
        );
        let identity = offer
            .input_identity(
                WorkAdmissionKey::new("offer-key").expect("key"),
                &scope(),
                &lead_actor(),
            )
            .expect("offer identity");
        assert_eq!(identity.key.as_str(), "offer-key");
        assert_eq!(identity.request_digest, GOLDEN_OFFER_DIGEST);

        let assign = golden_assign();
        assert_eq!(
            canonical_text_of(&assign.digest_body(&scope(), &lead_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.3:agent:mob/alpha/agent/lead","#,
                r#""content":{"acceptor":"counterpart:ops-to-research:intake","#,
                r#""kind":"assign","#,
                r#""parent":{"item_id":"work_0000","order_id":"order_0000"},"protocol":1,"#,
                r#""subject":{"proposed":{"description":"Second pass","title":"Review the notes"}},"#,
                r#""supersedes":"order_older","terms":{}},"#,
                r#""input":"offer","namespace":"default","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            assign
                .input_digest(&scope(), &lead_actor())
                .expect("assign digest"),
            GOLDEN_ASSIGN_DIGEST
        );

        let inbound = golden_inbound();
        assert_eq!(
            canonical_text_of(&inbound.digest_body(&scope(), &counterpart_actor())),
            concat!(
                r#"{"actor":"counterpart@4:ops-to-research:ops","#,
                r#""content":{"acceptor":"local:agent:mob/alpha/agent/reviewer","#,
                r#""counterpart_order":"order_remote_9","kind":"assign","protocol":1,"#,
                r#""subject":{"proposed":{"title":"Review the notes"}},"terms":{}},"#,
                r#""input":"offer","namespace":"default","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            inbound
                .input_digest(&scope(), &counterpart_actor())
                .expect("inbound digest"),
            GOLDEN_INBOUND_DIGEST
        );
    }

    #[test]
    fn accept_decline_and_close_input_digests_are_pinned_to_fixed_bytes() {
        let accept = WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::accept());
        assert_eq!(
            canonical_text_of(&accept.digest_body(&scope(), &reviewer_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.1:agent:mob/alpha/agent/reviewer","#,
                r#""content":{},"#,
                r#""expected_version":1,"input":"accept","namespace":"default","#,
                r#""order_id":"order_0001","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            accept
                .input_digest(&scope(), &reviewer_actor())
                .expect("accept digest"),
            GOLDEN_ACCEPT_DIGEST
        );

        // Non-ASCII text is written as is; a quote and a line break are
        // escaped as JSON requires.
        let decline = WorkOrderInputRequest::new(
            order_id(),
            1,
            WorkOrderInput::decline(
                WorkOrderReason::new(WorkOrderReasonCode::ProtocolUnsupported)
                    .with_note("caf\u{e9} \"quoted\"\nline"),
            ),
        );
        assert_eq!(
            canonical_text_of(&decline.digest_body(&scope(), &reviewer_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.1:agent:mob/alpha/agent/reviewer","#,
                r#""content":{"reason":{"code":"protocol_unsupported","#,
                "\"note\":\"caf\u{e9} \\\"quoted\\\"\\nline\"}},",
                r#""expected_version":1,"input":"decline","namespace":"default","#,
                r#""order_id":"order_0001","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            decline
                .input_digest(&scope(), &reviewer_actor())
                .expect("decline digest"),
            GOLDEN_DECLINE_DIGEST
        );

        let close = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(
                WorkOrderCloseOutcome::Completed,
                vec![
                    WorkOrderEvidenceRef::item("ev-1"),
                    WorkOrderEvidenceRef::item("ev-2"),
                ],
            ),
        );
        assert_eq!(
            canonical_text_of(&close.digest_body(&scope(), &reviewer_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.1:agent:mob/alpha/agent/reviewer","#,
                r#""content":{"evidence":[{"evidence_id":"ev-1","kind":"item"},"#,
                r#"{"evidence_id":"ev-2","kind":"item"}],"#,
                r#""outcome":"completed"},"#,
                r#""expected_version":2,"input":"close","namespace":"default","#,
                r#""order_id":"order_0001","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            close
                .input_digest(&scope(), &reviewer_actor())
                .expect("close digest"),
            GOLDEN_CLOSE_DIGEST
        );

        let mirrored = WorkOrderInputRequest::new(
            order_id(),
            2,
            WorkOrderInput::close(
                WorkOrderCloseOutcome::Completed,
                vec![WorkOrderEvidenceRef::counterpart_digest(
                    "report",
                    "sha256:cc",
                )],
            ),
        );
        assert_eq!(
            canonical_text_of(&mirrored.digest_body(&scope(), &counterpart_actor())),
            concat!(
                r#"{"actor":"counterpart@4:ops-to-research:ops","#,
                r#""content":{"evidence":["#,
                r#"{"digest":"sha256:cc","evidence_kind":"report","kind":"counterpart_digest"}],"#,
                r#""outcome":"completed"},"#,
                r#""expected_version":2,"input":"close","namespace":"default","#,
                r#""order_id":"order_0001","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            mirrored
                .input_digest(&scope(), &counterpart_actor())
                .expect("mirrored close digest"),
            GOLDEN_MIRRORED_CLOSE_DIGEST
        );
    }

    #[test]
    fn every_class_of_string_escape_is_pinned_to_fixed_bytes() {
        // One note that holds each class the canonical form treats in its
        // own way: a backslash; a tab; a control character with no short
        // form; U+007F; the two line separators that some writers escape;
        // a character outside the basic plane; a quote; a line feed; a
        // carriage return; a backspace; a form feed.
        let note = "a\\b\tc\u{1}d\u{7f}e\u{2028}f\u{2029}g\u{1F600}h\"i\nj\rk\u{8}l\u{c}m";
        let decline = WorkOrderInputRequest::new(
            order_id(),
            1,
            WorkOrderInput::decline(WorkOrderReason::default().with_note(note)),
        );
        assert_eq!(
            canonical_text_of(&decline.digest_body(&scope(), &reviewer_actor())),
            concat!(
                r#"{"actor":"local@0190a7c8-1111-4222-8333-444455556666.1:agent:mob/alpha/agent/reviewer","#,
                r#""content":{"reason":{"code":"unspecified","#,
                "\"note\":\"a\\\\b\\tc\\u0001d\u{7f}e\u{2028}f\u{2029}g\u{1F600}h\\\"i\\nj\\rk\\bl\\fm\"}},",
                r#""expected_version":1,"input":"decline","namespace":"default","#,
                r#""order_id":"order_0001","realm_id":"mob.alpha"}"#
            )
        );
        assert_eq!(
            decline
                .input_digest(&scope(), &reviewer_actor())
                .expect("escapes digest"),
            GOLDEN_ESCAPES_DIGEST
        );
    }

    #[test]
    fn input_digest_hashes_the_resolved_scope_and_every_part_of_the_input() {
        let offer = golden_offer();
        let base = offer
            .input_digest(&scope(), &lead_actor())
            .expect("base digest");

        // The request's own scope spelling is not hashed: the resolved scope is.
        assert_eq!(
            offer
                .clone()
                .with_scope("mob.alpha", WorkNamespace::default())
                .input_digest(&scope(), &lead_actor())
                .expect("explicit scope digest"),
            base
        );

        let mut different = BTreeSet::from([base]);
        let mut expect_new = |digest: String, what: &str| {
            assert!(different.insert(digest), "{what} did not change the digest");
        };
        let by_lead = |request: &OfferWorkOrderRequest| {
            request
                .input_digest(&scope(), &lead_actor())
                .expect("digest")
        };

        // The envelope of an offer.
        expect_new(
            offer
                .input_digest(
                    &WorkOrderScope::new("mob.beta", WorkNamespace::default()),
                    &lead_actor(),
                )
                .expect("digest"),
            "realm",
        );
        expect_new(
            offer
                .input_digest(
                    &WorkOrderScope::new("mob.alpha", WorkNamespace::new("other").expect("ns")),
                    &lead_actor(),
                )
                .expect("digest"),
            "namespace",
        );
        expect_new(
            offer
                .input_digest(&scope(), &reviewer_actor())
                .expect("digest"),
            "actor",
        );

        // Each part of a transfer offer, one at a time.
        let terms = offer.terms.clone();
        expect_new(
            by_lead(
                &OfferWorkOrderRequest::transfer(WorkOrderDesignation::local(lead()), item_id(), 7)
                    .with_terms(terms.clone()),
            ),
            "acceptor",
        );
        expect_new(
            by_lead(
                &OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 8)
                    .with_terms(terms.clone()),
            ),
            "item revision",
        );
        expect_new(
            by_lead(
                &OfferWorkOrderRequest::transfer(
                    reviewer_designation(),
                    WorkItemId::new("work_0002").expect("other item"),
                    7,
                )
                .with_terms(terms.clone()),
            ),
            "item id",
        );
        let mut no_expiry = terms.clone();
        no_expiry.offer_expires_at = None;
        expect_new(by_lead(&offer.clone().with_terms(no_expiry)), "expiry");
        let mut no_lease = terms.clone();
        no_lease.holder_lease_seconds = None;
        expect_new(by_lead(&offer.clone().with_terms(no_lease)), "lease");
        let mut no_note = terms;
        no_note.note = None;
        expect_new(by_lead(&offer.clone().with_terms(no_note)), "terms note");
        expect_new(
            by_lead(&offer.with_supersedes(WorkOrderId::new("order_older").expect("older"))),
            "supersedes",
        );
        // The other side's order id, which only a counterpart's offer names.
        let by_counterpart = |request: &OfferWorkOrderRequest| {
            request
                .input_digest(&scope(), &counterpart_actor())
                .expect("digest")
        };
        let inbound = golden_inbound();
        expect_new(by_counterpart(&inbound), "an inbound offer");
        expect_new(
            by_counterpart(&inbound.clone().with_counterpart_order(
                WorkCounterpartOrderRef::new("order_remote_8").expect("counterpart order"),
            )),
            "counterpart order",
        );
        let mut unnamed = inbound;
        unnamed.counterpart_order = None;
        expect_new(by_counterpart(&unnamed), "no counterpart order");

        // Each part of an assign offer.
        let assign = golden_assign();
        expect_new(by_lead(&assign), "an assign offer");
        let reproposed = |item: ProposedWorkItem| {
            let mut request = golden_assign();
            request.subject = WorkOrderSubject::Proposed(item);
            request
        };
        expect_new(
            by_lead(&reproposed(
                ProposedWorkItem::new("Review the notes again").with_description("Second pass"),
            )),
            "title",
        );
        expect_new(
            by_lead(&reproposed(
                ProposedWorkItem::new("Review the notes").with_description("Third pass"),
            )),
            "description",
        );
        expect_new(
            by_lead(&reproposed(ProposedWorkItem::new("Review the notes"))),
            "no description",
        );
        let mut other_parent = golden_assign();
        other_parent.parent = Some(WorkOrderParent::new(
            WorkItemId::new("work_0009").expect("parent item"),
        ));
        expect_new(by_lead(&other_parent), "parent");
        let mut no_parent = golden_assign();
        no_parent.parent = None;
        expect_new(by_lead(&no_parent), "no parent");

        // The envelope of an input on an existing order.
        let by_reviewer = |request: &WorkOrderInputRequest| {
            request
                .input_digest(&scope(), &reviewer_actor())
                .expect("digest")
        };
        let input = |version: u64, input: WorkOrderInput| {
            WorkOrderInputRequest::new(order_id(), version, input)
        };
        expect_new(
            by_reviewer(&input(1, WorkOrderInput::accept())),
            "input kind",
        );
        expect_new(
            by_reviewer(&input(2, WorkOrderInput::accept())),
            "expected version",
        );
        expect_new(
            by_reviewer(&WorkOrderInputRequest::new(
                WorkOrderId::new("order_0002").expect("other order"),
                1,
                WorkOrderInput::accept(),
            )),
            "order id",
        );
        // A withdrawal without a reason and an acceptance differ only by the
        // input kind; both have empty content.
        expect_new(
            by_reviewer(&input(1, WorkOrderInput::withdraw())),
            "withdraw",
        );

        // A reason: its code, its note.
        let reason = WorkOrderReason::new(WorkOrderReasonCode::ProtocolUnsupported);
        expect_new(
            by_reviewer(&input(1, WorkOrderInput::decline(reason.clone()))),
            "decline",
        );
        expect_new(
            by_reviewer(&input(
                1,
                WorkOrderInput::decline(WorkOrderReason::new(
                    WorkOrderReasonCode::ExchangeWithdrawn,
                )),
            )),
            "reason code",
        );
        expect_new(
            by_reviewer(&input(
                1,
                WorkOrderInput::decline(reason.clone().with_note("not this release")),
            )),
            "reason note",
        );
        expect_new(
            by_reviewer(&input(1, WorkOrderInput::withdraw_because(reason.clone()))),
            "a stated withdraw reason",
        );

        // A closure: its outcome, its evidence, the order of its evidence,
        // its reason.
        let first = WorkOrderEvidenceRef::item("ev-1");
        let second = WorkOrderEvidenceRef::item("ev-2");
        let close = |outcome, evidence: Vec<WorkOrderEvidenceRef>| {
            input(2, WorkOrderInput::close(outcome, evidence))
        };
        expect_new(
            by_reviewer(&close(
                WorkOrderCloseOutcome::Completed,
                vec![first.clone(), second.clone()],
            )),
            "close",
        );
        expect_new(
            by_reviewer(&close(
                WorkOrderCloseOutcome::Failed,
                vec![first.clone(), second.clone()],
            )),
            "close outcome",
        );
        expect_new(
            by_reviewer(&close(
                WorkOrderCloseOutcome::Completed,
                vec![second.clone(), first.clone()],
            )),
            "evidence order",
        );
        expect_new(
            by_reviewer(&close(
                WorkOrderCloseOutcome::Completed,
                vec![first.clone()],
            )),
            "evidence count",
        );
        // A counterpart's mirrored closure: the evidence kind and digest.
        let mirrored = |kind: &str, digest: &str| {
            close(
                WorkOrderCloseOutcome::Completed,
                vec![WorkOrderEvidenceRef::counterpart_digest(kind, digest)],
            )
            .input_digest(&scope(), &counterpart_actor())
            .expect("digest")
        };
        expect_new(mirrored("report", "sha256:cc"), "a mirrored closure");
        expect_new(mirrored("log", "sha256:cc"), "evidence kind");
        expect_new(mirrored("report", "sha256:dd"), "evidence digest");
        expect_new(
            by_reviewer(&input(
                2,
                WorkOrderInput::close_because(
                    WorkOrderCloseOutcome::Completed,
                    vec![first, second],
                    reason,
                ),
            )),
            "close reason",
        );
    }

    #[test]
    fn two_requests_of_one_meaning_have_one_digest() {
        let by_reviewer = |input: WorkOrderInput| {
            WorkOrderInputRequest::new(order_id(), 1, input)
                .input_digest(&scope(), &reviewer_actor())
                .expect("digest")
        };
        // A reason that may be left out and says nothing is the same as
        // none: on a withdrawal and on a closure.
        assert_eq!(
            by_reviewer(WorkOrderInput::withdraw_because(WorkOrderReason::default())),
            by_reviewer(WorkOrderInput::withdraw())
        );
        assert_eq!(
            by_reviewer(WorkOrderInput::close_because(
                WorkOrderCloseOutcome::Failed,
                Vec::new(),
                WorkOrderReason::default(),
            )),
            by_reviewer(WorkOrderInput::close(
                WorkOrderCloseOutcome::Failed,
                Vec::new()
            ))
        );
        // A reason that says something is hashed.
        assert_ne!(
            by_reviewer(WorkOrderInput::withdraw_because(WorkOrderReason::new(
                WorkOrderReasonCode::ExchangeWithdrawn
            ))),
            by_reviewer(WorkOrderInput::withdraw())
        );
        // What a decision record keeps is the same one form.
        let stated = |input: WorkOrderInput| {
            WorkOrderInputRequest::new(order_id(), 1, input)
                .stated_reason()
                .cloned()
        };
        assert_eq!(stated(WorkOrderInput::withdraw()), None);
        assert_eq!(
            stated(WorkOrderInput::withdraw_because(WorkOrderReason::default())),
            None
        );
        assert_eq!(
            stated(WorkOrderInput::close_because(
                WorkOrderCloseOutcome::Failed,
                Vec::new(),
                WorkOrderReason::default(),
            )),
            None
        );
        assert_eq!(stated(WorkOrderInput::accept()), None);
        let said = WorkOrderReason::new(WorkOrderReasonCode::ExchangeWithdrawn);
        assert_eq!(
            stated(WorkOrderInput::withdraw_because(said.clone())),
            Some(said)
        );
        assert_eq!(
            stated(WorkOrderInput::decline(WorkOrderReason::default())),
            Some(WorkOrderReason::default())
        );

        // A decline always states its reason, in one spelling.
        let text = canonical_text_of(
            &WorkOrderInputRequest::new(
                order_id(),
                1,
                WorkOrderInput::decline(WorkOrderReason::default()),
            )
            .digest_body(&scope(), &reviewer_actor()),
        );
        assert!(
            text.contains(r#""content":{"reason":{"code":"unspecified"}}"#),
            "{text}"
        );
    }

    #[test]
    fn a_stored_digest_is_read_by_its_form_and_never_compared_as_text() {
        let accept = WorkOrderInputRequest::new(order_id(), 1, WorkOrderInput::accept());
        let matches = |request: &WorkOrderInputRequest, stored: &str| {
            request.matches_stored_digest(&scope(), &reviewer_actor(), stored)
        };

        // The form is read from the stored text.
        assert_eq!(
            WorkOrderDigestForm::of_stored(GOLDEN_ACCEPT_DIGEST).expect("form"),
            WorkOrderDigestForm::V1
        );
        assert_eq!(WorkOrderDigestForm::CURRENT.name(), "v1");

        // The same request: a replay. Other content under the key: a conflict.
        assert!(matches(&accept, GOLDEN_ACCEPT_DIGEST).expect("replay"));
        let other = WorkOrderInputRequest::new(order_id(), 2, WorkOrderInput::accept());
        assert!(!matches(&other, GOLDEN_ACCEPT_DIGEST).expect("conflict"));
        // The same for an offer.
        assert!(
            golden_offer()
                .matches_stored_digest(&scope(), &lead_actor(), GOLDEN_OFFER_DIGEST)
                .expect("replay")
        );
        assert!(
            !golden_offer()
                .matches_stored_digest(&scope(), &lead_actor(), GOLDEN_ACCEPT_DIGEST)
                .expect("conflict")
        );

        // A form this build does not know is never called a conflict.
        let hex = GOLDEN_ACCEPT_DIGEST
            .strip_prefix("v1:sha256:")
            .expect("golden digest");
        let later = format!("v2:sha256:{hex}");
        for result in [
            matches(&accept, &later),
            matches(&other, &later),
            golden_offer().matches_stored_digest(&scope(), &lead_actor(), &later),
        ] {
            assert!(
                matches!(&result, Err(WorkOrderError::DigestFormUnknown { form }) if form == "v2"),
                "{result:?}"
            );
        }
        assert!(matches!(
            WorkOrderDigestForm::of_stored(&later),
            Err(WorkOrderError::DigestFormUnknown { .. })
        ));
        // The form's name decides: a later form may hash otherwise and write
        // another number of digits, and is still a form this build does not
        // know, never a digest this module did not write.
        let later_hash = format!("v2:sha512:{hex}{hex}");
        let later_length = format!("v2:sha256:{}", &hex[..32]);
        for later in [later_hash.as_str(), later_length.as_str()] {
            let result = matches(&accept, later);
            assert!(
                matches!(&result, Err(WorkOrderError::DigestFormUnknown { form }) if form == "v2"),
                "{later:?}: {result:?}"
            );
        }

        // A name is at most 16 lowercase letters and digits, the form's and
        // the hash's alike. At the limit a later form is still read as one.
        let at_limit = format!("{}:sha256:{hex}", "v".repeat(16));
        let result = matches(&accept, &at_limit);
        assert!(
            matches!(&result, Err(WorkOrderError::DigestFormUnknown { form }) if form.len() == 16),
            "{result:?}"
        );

        // No form, or not a digest of this module at all. A name past the
        // limit, or with any other character, is no name: a later form that
        // wrote one would be read as malformed, so none may.
        let upper = format!("v1:sha256:{}", hex.to_uppercase());
        let short = format!("v1:sha256:{}", &hex[..63]);
        let other_hash = format!("v1:md5:{hex}");
        let upper_form = format!("V1:sha256:{hex}");
        let no_form = format!("sha256:{hex}");
        let empty_form = format!(":sha256:{hex}");
        let long_form = format!("{}:sha256:{hex}", "v".repeat(17));
        let long_hash = format!("v2:{}:{hex}", "s".repeat(17));
        let hyphen_hash = format!("v2:sha3-256:{hex}");
        for malformed in [
            no_form.as_str(),
            hex,
            "",
            "v1",
            "v1:sha256:",
            upper.as_str(),
            short.as_str(),
            other_hash.as_str(),
            upper_form.as_str(),
            empty_form.as_str(),
            long_form.as_str(),
            long_hash.as_str(),
            hyphen_hash.as_str(),
        ] {
            let result = matches(&accept, malformed);
            assert!(
                matches!(result, Err(WorkOrderError::DigestMalformed)),
                "{malformed:?}: {result:?}"
            );
        }
    }

    #[test]
    fn a_designation_is_pinned_by_one_function_and_an_unseated_member_is_no_party() {
        let member = WorkOrderDesignation::local(reviewer());
        // A member with a standing seat is pinned to it.
        assert_eq!(
            member
                .pin(WorkOrderPinFact::LocalSeat(Some(reviewer_seat())))
                .expect("pinned"),
            *reviewer_actor().party()
        );
        // A member with no standing seat is refused, typed.
        assert!(matches!(
            member.pin(WorkOrderPinFact::LocalSeat(None)),
            Err(WorkOrderError::Refused(WorkOrderRefusal::AcceptorNotSeated))
        ));
        // An owner that is no mob member is pinned with no seat, and never
        // with one.
        let person = WorkOrderDesignation::local(WorkOwnerKey::principal("user-17").expect("key"));
        let pinned = person
            .pin(WorkOrderPinFact::LocalSeat(None))
            .expect("principal party");
        assert_eq!(pinned.seat(), None);
        assert_eq!(pinned.canonical(), "local@-:principal:user-17");
        assert!(matches!(
            person.pin(WorkOrderPinFact::LocalSeat(Some(reviewer_seat()))),
            Err(WorkOrderError::Graph(WorkGraphError::InvalidInput(_)))
        ));
        // The other side is pinned to its declaration's generation; with no
        // standing declaration it is refused.
        let other_side = WorkOrderDesignation::counterpart(
            exchange(),
            WorkCounterpartPartyRef::new("ops").expect("party ref"),
        );
        assert_eq!(
            other_side
                .pin(WorkOrderPinFact::ExchangeGeneration(Some(4)))
                .expect("pinned"),
            *counterpart_actor().party()
        );
        assert!(matches!(
            other_side.pin(WorkOrderPinFact::ExchangeGeneration(None)),
            Err(WorkOrderError::Refused(WorkOrderRefusal::AuthorityNotLive))
        ));
        // An answer about the other kind of designation is the host's error.
        assert!(matches!(
            other_side.pin(WorkOrderPinFact::LocalSeat(Some(reviewer_seat()))),
            Err(WorkOrderError::Graph(WorkGraphError::InvalidInput(_)))
        ));
        assert!(matches!(
            member.pin(WorkOrderPinFact::ExchangeGeneration(Some(4))),
            Err(WorkOrderError::Graph(WorkGraphError::InvalidInput(_)))
        ));
        // The pin and the designation are inverse on every party.
        for party in [
            reviewer_actor().party(),
            counterpart_actor().party(),
            &pinned,
        ] {
            let fact = match party.seat() {
                Some(seat) => WorkOrderPinFact::LocalSeat(Some(seat)),
                None if party.is_local() => WorkOrderPinFact::LocalSeat(None),
                None => WorkOrderPinFact::ExchangeGeneration(Some(4)),
            };
            assert_eq!(&party.designation().pin(fact).expect("pinned"), party);
        }

        // An unseated mob member can be neither built nor decoded, as a
        // party of a record or anywhere a party is stored.
        assert!(WorkOrderParty::local(reviewer()).is_err());
        let unseated = json!({
            "kind": "local",
            "key": {"kind": "agent", "id": "mob/alpha/agent/reviewer"}
        });
        assert!(serde_json::from_value::<WorkOrderParty>(unseated.clone()).is_err());
        let mut record = accepted_transfer_json();
        record["acceptor"] = unseated;
        assert!(serde_json::from_value::<WorkOrder>(record).is_err());
        // Nor a seat on an owner that is no mob member.
        assert!(
            serde_json::from_value::<WorkOrderParty>(json!({
                "kind": "local",
                "key": {"kind": "principal", "id": "user-17"},
                "seat": {"mob_incarnation": INCARNATION, "member_generation": 1}
            }))
            .is_err()
        );
    }

    #[test]
    fn canonical_json_orders_keys_by_bytes_omits_absent_fields_and_refuses_fractions() {
        let body = json!({
            "b": [3, {"z": true, "a": null}],
            "a": {"k2": "v", "k10": -4},
            "B": "upper sorts before lower"
        });
        assert_eq!(
            canonical_text_of(&body),
            r#"{"B":"upper sorts before lower","a":{"k10":-4,"k2":"v"},"b":[3,{"a":null,"z":true}]}"#
        );
        assert!(canonical_input_bytes(&json!({"ratio": 0.5})).is_err());

        // An absent optional is omitted from the body, not written as null,
        // so a field added later leaves earlier digests as they were.
        let bare = OfferWorkOrderRequest::transfer(reviewer_designation(), item_id(), 7);
        let text = canonical_text_of(&bare.digest_body(&scope(), &lead_actor()));
        assert!(!text.contains("null"), "{text}");
        for absent in [
            "supersedes",
            "parent",
            "counterpart_order",
            "note",
            "offer_expires_at_ms",
        ] {
            assert!(!text.contains(absent), "{absent} was written: {text}");
        }
        assert!(text.contains(r#""terms":{}"#), "{text}");
    }
}
