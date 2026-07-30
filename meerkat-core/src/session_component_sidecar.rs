//! Authenticated append-only sidecar event prefixes for session components.
//!
//! Head-canonical persistence cannot put history-growing component state back
//! into the session head without recreating the whole-document write cost it is
//! meant to remove. This module supplies the shared mechanical primitive used
//! by typed component reducers instead:
//!
//! - component events have one exact canonical serialized representation;
//! - an ordered, session- and component-bound SHA-256 prefix commits those
//!   bytes and their sequence numbers;
//! - a prepared suffix is valid by construction and contains only new events;
//! - durable rows must be verified against the expected prefix before a
//!   reducer can replay them.
//!
//! The prefix is an integrity/continuity witness, not an authenticity
//! signature. A principal able to rewrite both rows and the authoritative head
//! can derive a matching unkeyed digest. Store fencing and runtime ownership
//! remain responsible for write authorization.

use crate::types::SessionId;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;
use std::sync::Arc;

const COMPONENT_EVENT_PREFIX_VERSION: u16 = 1;
const COMPONENT_EVENT_PREFIX_EMPTY_DOMAIN: &[u8] = b"meerkat.session-component-prefix.empty.v1\0";
const COMPONENT_EVENT_PREFIX_STEP_DOMAIN: &[u8] = b"meerkat.session-component-prefix.step.v1\0";
const COMPONENT_EVENT_DIGEST_DOMAIN: &[u8] = b"meerkat.session-component-event.v1\0";
const SHA256_TOKEN_PREFIX: &str = "sha256:";

/// Session component whose exact event rows are covered by one prefix.
///
/// The enum is closed for the v1 prefix contract. Adding a component requires
/// an explicit new variant and therefore a stable, domain-separated durable
/// spelling; unknown spellings fail serde decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionComponentKind {
    /// Realtime transcript reducer state.
    Realtime,
}

impl SessionComponentKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Realtime => "realtime",
        }
    }
}

impl fmt::Display for SessionComponentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Canonical SHA-256 digest of one exact serialized component event.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ComponentEventDigest(String);

impl ComponentEventDigest {
    /// Parse a canonical lowercase `sha256:<64 hex digits>` token.
    pub fn parse(value: impl Into<String>) -> Result<Self, SessionComponentSidecarError> {
        let value = value.into();
        validate_sha256_token(&value).map_err(|()| {
            SessionComponentSidecarError::MalformedDigest {
                subject: "component event",
                value: value.clone(),
            }
        })?;
        Ok(Self(value))
    }

    fn from_raw(raw: &[u8; 32]) -> Self {
        Self(format!("{SHA256_TOKEN_PREFIX}{}", encode_lower_hex(raw)))
    }

    fn to_raw(&self) -> Result<[u8; 32], SessionComponentSidecarError> {
        decode_sha256_token(&self.0).map_err(|()| SessionComponentSidecarError::MalformedDigest {
            subject: "component event",
            value: self.0.clone(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ComponentEventDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ComponentEventDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Canonical SHA-256 rolling root of one ordered component event prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ComponentEventPrefixDigest(String);

impl ComponentEventPrefixDigest {
    /// Parse a canonical lowercase `sha256:<64 hex digits>` token.
    pub fn parse(value: impl Into<String>) -> Result<Self, SessionComponentSidecarError> {
        let value = value.into();
        validate_sha256_token(&value).map_err(|()| {
            SessionComponentSidecarError::MalformedDigest {
                subject: "component event prefix",
                value: value.clone(),
            }
        })?;
        Ok(Self(value))
    }

    fn from_raw(raw: &[u8; 32]) -> Self {
        Self(format!("{SHA256_TOKEN_PREFIX}{}", encode_lower_hex(raw)))
    }

    fn to_raw(&self) -> Result<[u8; 32], SessionComponentSidecarError> {
        decode_sha256_token(&self.0).map_err(|()| SessionComponentSidecarError::MalformedDigest {
            subject: "component event prefix",
            value: self.0.clone(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ComponentEventPrefixDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ComponentEventPrefixDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Compact authority for one exact ordered component event prefix.
///
/// `root_digest` commits the session id, component kind, every sequence
/// number, and every event's exact canonical bytes. Extending the authority is
/// O(total bytes in the new events), independent of the retained prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ComponentEventPrefixAuthority {
    version: u16,
    session_id: SessionId,
    component: SessionComponentKind,
    event_count: u64,
    root_digest: ComponentEventPrefixDigest,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ComponentEventPrefixAuthorityWire {
    version: u16,
    session_id: SessionId,
    component: SessionComponentKind,
    event_count: u64,
    root_digest: ComponentEventPrefixDigest,
}

impl<'de> Deserialize<'de> for ComponentEventPrefixAuthority {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ComponentEventPrefixAuthorityWire::deserialize(deserializer)?;
        Self::from_parts(
            wire.version,
            wire.session_id,
            wire.component,
            wire.event_count,
            wire.root_digest,
        )
        .map_err(serde::de::Error::custom)
    }
}

impl ComponentEventPrefixAuthority {
    pub const VERSION: u16 = COMPONENT_EVENT_PREFIX_VERSION;

    /// Construct the domain-separated empty prefix for a session component.
    #[must_use]
    pub fn empty(session_id: SessionId, component: SessionComponentKind) -> Self {
        let root_digest = empty_prefix_digest(&session_id, component);
        Self {
            version: Self::VERSION,
            session_id,
            component,
            event_count: 0,
            root_digest,
        }
    }

    /// Restore authority fields read from a durable compact head.
    ///
    /// This validates the schema and digest encoding. For the empty prefix it
    /// additionally recomputes the one legal root; a non-empty root is verified
    /// against event rows by [`VerifiedComponentEventSequence::verify_full`]
    /// or [`VerifiedComponentEventSequence::verify_suffix`].
    pub fn from_parts(
        version: u16,
        session_id: SessionId,
        component: SessionComponentKind,
        event_count: u64,
        root_digest: ComponentEventPrefixDigest,
    ) -> Result<Self, SessionComponentSidecarError> {
        if version != Self::VERSION {
            return Err(SessionComponentSidecarError::UnsupportedPrefixVersion {
                found: version,
                supported: Self::VERSION,
            });
        }
        root_digest.to_raw()?;
        let authority = Self {
            version,
            session_id,
            component,
            event_count,
            root_digest,
        };
        if authority.event_count == 0 {
            let expected = Self::empty(authority.session_id.clone(), authority.component);
            if authority.root_digest != expected.root_digest {
                return Err(SessionComponentSidecarError::InvalidEmptyPrefixRoot {
                    session_id: authority.session_id.clone(),
                    component: authority.component,
                    expected: expected.root_digest,
                    actual: authority.root_digest,
                });
            }
        }
        Ok(authority)
    }

    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn component(&self) -> SessionComponentKind {
        self.component
    }

    #[must_use]
    pub const fn event_count(&self) -> u64 {
        self.event_count
    }

    #[must_use]
    pub fn root_digest(&self) -> &ComponentEventPrefixDigest {
        &self.root_digest
    }

    /// Derive the successor prefix by folding exact serialized events.
    pub fn extend_serialized_events(
        &self,
        events: &[SerializedComponentEvent],
    ) -> Result<Self, SessionComponentSidecarError> {
        let mut event_count = self.event_count;
        let mut root = self.root_digest.to_raw()?;
        for event in events {
            let successor_count = event_count.checked_add(1).ok_or(
                SessionComponentSidecarError::EventCountOverflow {
                    session_id: self.session_id.clone(),
                    component: self.component,
                },
            )?;
            root = fold_prefix_step(
                &self.session_id,
                self.component,
                event_count,
                &root,
                event.digest_raw()?,
            );
            event_count = successor_count;
        }
        Ok(Self {
            version: self.version,
            session_id: self.session_id.clone(),
            component: self.component,
            event_count,
            root_digest: ComponentEventPrefixDigest::from_raw(&root),
        })
    }

    /// Verify an exact suffix against this predecessor and one expected
    /// successor authority.
    pub fn verify_serialized_suffix(
        &self,
        base_seq: u64,
        events: &[SerializedComponentEvent],
        expected_successor: &Self,
    ) -> Result<(), SessionComponentSidecarError> {
        if base_seq != self.event_count {
            return Err(SessionComponentSidecarError::BaseSequenceMismatch {
                expected: self.event_count,
                actual: base_seq,
            });
        }
        ensure_same_prefix_identity(self, expected_successor)?;
        let actual_successor = self.extend_serialized_events(events)?;
        if &actual_successor != expected_successor {
            return Err(SessionComponentSidecarError::SuccessorPrefixMismatch {
                session_id: self.session_id.clone(),
                component: self.component,
                expected_count: expected_successor.event_count,
                expected_root: expected_successor.root_digest.clone(),
                actual_count: actual_successor.event_count,
                actual_root: actual_successor.root_digest,
            });
        }
        Ok(())
    }
}

/// One self-describing canonical component event.
///
/// The exact bytes are an envelope:
/// `{"payload":...,"schema_version":N}` under canonical JSON key ordering.
/// Component-specific modules own the payload type and schema semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializedComponentEvent {
    schema_version: u16,
    bytes: Arc<[u8]>,
    digest: ComponentEventDigest,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentEventEnvelope {
    schema_version: u16,
    payload: serde_json::Value,
}

impl SerializedComponentEvent {
    /// Canonically serialize one typed component event payload.
    pub fn canonical_json<T>(
        schema_version: u16,
        payload: &T,
    ) -> Result<Self, SessionComponentSidecarError>
    where
        T: Serialize + ?Sized,
    {
        if schema_version == 0 {
            return Err(SessionComponentSidecarError::InvalidEventSchemaVersion(
                schema_version,
            ));
        }
        let payload = serde_json::to_value(payload)
            .map_err(|error| SessionComponentSidecarError::EventSerialization(error.to_string()))?;
        let envelope = ComponentEventEnvelope {
            schema_version,
            payload,
        };
        let value = serde_json::to_value(envelope)
            .map_err(|error| SessionComponentSidecarError::EventSerialization(error.to_string()))?;
        let bytes = canonical_json_bytes(&value)?;
        Self::from_verified_parts(schema_version, bytes)
    }

    /// Validate and adopt exact canonical event bytes read from durable rows.
    pub fn from_canonical_bytes(bytes: Vec<u8>) -> Result<Self, SessionComponentSidecarError> {
        let envelope: ComponentEventEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| SessionComponentSidecarError::MalformedEvent(error.to_string()))?;
        if envelope.schema_version == 0 {
            return Err(SessionComponentSidecarError::InvalidEventSchemaVersion(
                envelope.schema_version,
            ));
        }
        let value = serde_json::to_value(&envelope)
            .map_err(|error| SessionComponentSidecarError::EventSerialization(error.to_string()))?;
        let canonical = canonical_json_bytes(&value)?;
        if canonical != bytes {
            return Err(SessionComponentSidecarError::NonCanonicalEvent);
        }
        Self::from_verified_parts(envelope.schema_version, bytes)
    }

    fn from_verified_parts(
        schema_version: u16,
        bytes: Vec<u8>,
    ) -> Result<Self, SessionComponentSidecarError> {
        let digest = component_event_digest(schema_version, &bytes);
        Ok(Self {
            schema_version,
            bytes: Arc::from(bytes),
            digest,
        })
    }

    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_ref()
    }

    #[must_use]
    pub fn digest(&self) -> &ComponentEventDigest {
        &self.digest
    }

    fn digest_raw(&self) -> Result<[u8; 32], SessionComponentSidecarError> {
        let recomputed = component_event_digest(self.schema_version, self.bytes());
        if recomputed != self.digest {
            return Err(SessionComponentSidecarError::EventDigestMismatch {
                expected: self.digest.clone(),
                actual: recomputed,
            });
        }
        self.digest.to_raw()
    }

    /// Decode the typed payload after checking its exact schema version.
    pub fn decode_payload<T>(
        &self,
        expected_schema_version: u16,
    ) -> Result<T, SessionComponentSidecarError>
    where
        T: serde::de::DeserializeOwned,
    {
        if self.schema_version != expected_schema_version {
            return Err(SessionComponentSidecarError::UnexpectedEventSchemaVersion {
                expected: expected_schema_version,
                actual: self.schema_version,
            });
        }
        let envelope: ComponentEventEnvelope = serde_json::from_slice(self.bytes())
            .map_err(|error| SessionComponentSidecarError::MalformedEvent(error.to_string()))?;
        serde_json::from_value(envelope.payload)
            .map_err(|error| SessionComponentSidecarError::MalformedEvent(error.to_string()))
    }
}

/// Valid-by-construction exact event suffix for one head-canonical boundary.
///
/// The carrier has no raw-parts constructor. `successor` is derived from the
/// predecessor and event bytes, so a backend receives one paired continuity
/// fact rather than independent caller attestations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedComponentEventSuffix {
    predecessor: ComponentEventPrefixAuthority,
    successor: ComponentEventPrefixAuthority,
    events: Arc<[SerializedComponentEvent]>,
}

impl PreparedComponentEventSuffix {
    /// Seal a non-empty event suffix and derive its exact successor prefix.
    pub fn prepare(
        predecessor: ComponentEventPrefixAuthority,
        events: Vec<SerializedComponentEvent>,
    ) -> Result<Self, SessionComponentSidecarError> {
        if events.is_empty() {
            return Err(SessionComponentSidecarError::EmptyPreparedSuffix {
                session_id: predecessor.session_id.clone(),
                component: predecessor.component,
            });
        }
        let successor = predecessor.extend_serialized_events(&events)?;
        Ok(Self {
            predecessor,
            successor,
            events: events.into(),
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.predecessor.session_id()
    }

    #[must_use]
    pub const fn component(&self) -> SessionComponentKind {
        self.predecessor.component()
    }

    #[must_use]
    pub const fn base_seq(&self) -> u64 {
        self.predecessor.event_count()
    }

    #[must_use]
    pub fn predecessor(&self) -> &ComponentEventPrefixAuthority {
        &self.predecessor
    }

    #[must_use]
    pub fn successor(&self) -> &ComponentEventPrefixAuthority {
        &self.successor
    }

    #[must_use]
    pub fn events(&self) -> &[SerializedComponentEvent] {
        self.events.as_ref()
    }
}

/// Untrusted durable event row supplied to a verification ingress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredComponentEventRow {
    sequence: u64,
    event_bytes: Vec<u8>,
}

impl StoredComponentEventRow {
    #[must_use]
    pub fn new(sequence: u64, event_bytes: Vec<u8>) -> Self {
        Self {
            sequence,
            event_bytes,
        }
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub fn event_bytes(&self) -> &[u8] {
        &self.event_bytes
    }
}

/// Opaque proof that exact durable event rows form one authenticated sequence.
///
/// Construction checks contiguity, canonical event encoding, and the rolling
/// successor root. Reducers accept this proof instead of unverified row bytes.
#[derive(Debug, Clone)]
pub struct VerifiedComponentEventSequence {
    predecessor: ComponentEventPrefixAuthority,
    successor: ComponentEventPrefixAuthority,
    events: Arc<[SerializedComponentEvent]>,
}

impl VerifiedComponentEventSequence {
    /// Verify the complete event log from its component's empty root.
    pub fn verify_full(
        expected_successor: ComponentEventPrefixAuthority,
        rows: Vec<StoredComponentEventRow>,
    ) -> Result<Self, SessionComponentSidecarError> {
        let predecessor = ComponentEventPrefixAuthority::empty(
            expected_successor.session_id.clone(),
            expected_successor.component,
        );
        Self::verify_suffix(predecessor, expected_successor, rows)
    }

    /// Verify exact durable suffix rows against known predecessor/successor
    /// authorities.
    pub fn verify_suffix(
        predecessor: ComponentEventPrefixAuthority,
        expected_successor: ComponentEventPrefixAuthority,
        rows: Vec<StoredComponentEventRow>,
    ) -> Result<Self, SessionComponentSidecarError> {
        ensure_same_prefix_identity(&predecessor, &expected_successor)?;
        let mut next_sequence = predecessor.event_count;
        let mut events = Vec::with_capacity(rows.len());
        for row in rows {
            if row.sequence != next_sequence {
                return Err(SessionComponentSidecarError::EventSequenceMismatch {
                    expected: next_sequence,
                    actual: row.sequence,
                });
            }
            events.push(SerializedComponentEvent::from_canonical_bytes(
                row.event_bytes,
            )?);
            next_sequence = next_sequence.checked_add(1).ok_or(
                SessionComponentSidecarError::EventCountOverflow {
                    session_id: predecessor.session_id.clone(),
                    component: predecessor.component,
                },
            )?;
        }
        predecessor.verify_serialized_suffix(
            predecessor.event_count,
            &events,
            &expected_successor,
        )?;
        Ok(Self {
            predecessor,
            successor: expected_successor,
            events: events.into(),
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        self.predecessor.session_id()
    }

    #[must_use]
    pub const fn component(&self) -> SessionComponentKind {
        self.predecessor.component()
    }

    #[must_use]
    pub const fn base_seq(&self) -> u64 {
        self.predecessor.event_count()
    }

    #[must_use]
    pub fn predecessor(&self) -> &ComponentEventPrefixAuthority {
        &self.predecessor
    }

    #[must_use]
    pub fn successor(&self) -> &ComponentEventPrefixAuthority {
        &self.successor
    }

    #[must_use]
    pub fn events(&self) -> &[SerializedComponentEvent] {
        self.events.as_ref()
    }

    /// Replay the already-verified sequence through a component-owned reducer.
    ///
    /// Verification and semantic reduction stay separate: this module owns
    /// exact bytes and continuity, while the system-context/realtime machines
    /// own what each event means.
    pub fn replay<S, E, F>(&self, mut state: S, mut apply: F) -> Result<S, E>
    where
        F: FnMut(&mut S, u64, &SerializedComponentEvent) -> Result<(), E>,
    {
        for (offset, event) in self.events.iter().enumerate() {
            let sequence = self.base_seq() + offset as u64;
            apply(&mut state, sequence, event)?;
        }
        Ok(state)
    }
}

/// Typed failures from component-event serialization, continuity, and replay
/// admission.
#[derive(Debug, thiserror::Error)]
pub enum SessionComponentSidecarError {
    #[error("unsupported component event prefix version {found}; supported version is {supported}")]
    UnsupportedPrefixVersion { found: u16, supported: u16 },

    #[error("invalid component event schema version {0}; versions start at 1")]
    InvalidEventSchemaVersion(u16),

    #[error("expected component event schema version {expected}, found {actual}")]
    UnexpectedEventSchemaVersion { expected: u16, actual: u16 },

    #[error("malformed {subject} digest '{value}'")]
    MalformedDigest {
        subject: &'static str,
        value: String,
    },

    #[error(
        "empty {component} component prefix for session {session_id} has root {actual}, expected {expected}"
    )]
    InvalidEmptyPrefixRoot {
        session_id: SessionId,
        component: SessionComponentKind,
        expected: ComponentEventPrefixDigest,
        actual: ComponentEventPrefixDigest,
    },

    #[error(
        "component prefix identity mismatch: predecessor is {predecessor_component} for session {predecessor_session_id}, successor is {successor_component} for session {successor_session_id}"
    )]
    PrefixIdentityMismatch {
        predecessor_session_id: SessionId,
        predecessor_component: SessionComponentKind,
        successor_session_id: SessionId,
        successor_component: SessionComponentKind,
    },

    #[error("component event base sequence mismatch: expected {expected}, found {actual}")]
    BaseSequenceMismatch { expected: u64, actual: u64 },

    #[error("component event sequence mismatch: expected {expected}, found {actual}")]
    EventSequenceMismatch { expected: u64, actual: u64 },

    #[error("component event count overflow for {component} in session {session_id}")]
    EventCountOverflow {
        session_id: SessionId,
        component: SessionComponentKind,
    },

    #[error(
        "{component} component successor mismatch for session {session_id}: expected count/root {expected_count}/{expected_root}, got {actual_count}/{actual_root}"
    )]
    SuccessorPrefixMismatch {
        session_id: SessionId,
        component: SessionComponentKind,
        expected_count: u64,
        expected_root: ComponentEventPrefixDigest,
        actual_count: u64,
        actual_root: ComponentEventPrefixDigest,
    },

    #[error("component event serialization failed: {0}")]
    EventSerialization(String),

    #[error("malformed serialized component event: {0}")]
    MalformedEvent(String),

    #[error("serialized component event is valid JSON but not its exact canonical encoding")]
    NonCanonicalEvent,

    #[error("component event digest mismatch: expected {expected}, got {actual}")]
    EventDigestMismatch {
        expected: ComponentEventDigest,
        actual: ComponentEventDigest,
    },

    #[error(
        "prepared {component} component suffix for session {session_id} contains no events; unchanged components must be omitted"
    )]
    EmptyPreparedSuffix {
        session_id: SessionId,
        component: SessionComponentKind,
    },
}

fn ensure_same_prefix_identity(
    predecessor: &ComponentEventPrefixAuthority,
    successor: &ComponentEventPrefixAuthority,
) -> Result<(), SessionComponentSidecarError> {
    if predecessor.version != successor.version {
        return Err(SessionComponentSidecarError::UnsupportedPrefixVersion {
            found: successor.version,
            supported: predecessor.version,
        });
    }
    if predecessor.session_id != successor.session_id
        || predecessor.component != successor.component
    {
        return Err(SessionComponentSidecarError::PrefixIdentityMismatch {
            predecessor_session_id: predecessor.session_id.clone(),
            predecessor_component: predecessor.component,
            successor_session_id: successor.session_id.clone(),
            successor_component: successor.component,
        });
    }
    Ok(())
}

fn empty_prefix_digest(
    session_id: &SessionId,
    component: SessionComponentKind,
) -> ComponentEventPrefixDigest {
    let mut hasher = Sha256::new();
    hasher.update(COMPONENT_EVENT_PREFIX_EMPTY_DOMAIN);
    update_framed(&mut hasher, session_id.to_string().as_bytes());
    update_framed(&mut hasher, component.as_str().as_bytes());
    ComponentEventPrefixDigest::from_raw(&hasher.finalize().into())
}

fn component_event_digest(schema_version: u16, bytes: &[u8]) -> ComponentEventDigest {
    let mut hasher = Sha256::new();
    hasher.update(COMPONENT_EVENT_DIGEST_DOMAIN);
    hasher.update(schema_version.to_be_bytes());
    update_framed(&mut hasher, bytes);
    ComponentEventDigest::from_raw(&hasher.finalize().into())
}

fn fold_prefix_step(
    session_id: &SessionId,
    component: SessionComponentKind,
    sequence: u64,
    predecessor_root: &[u8; 32],
    event_digest: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(COMPONENT_EVENT_PREFIX_STEP_DOMAIN);
    update_framed(&mut hasher, session_id.to_string().as_bytes());
    update_framed(&mut hasher, component.as_str().as_bytes());
    hasher.update(sequence.to_be_bytes());
    hasher.update(predecessor_root);
    hasher.update(event_digest);
    hasher.finalize().into()
}

fn update_framed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn canonical_json_bytes(
    value: &serde_json::Value,
) -> Result<Vec<u8>, SessionComponentSidecarError> {
    let mut bytes = Vec::new();
    crate::digest_observability::write_canonical_json(value, &mut bytes)
        .map_err(|error| SessionComponentSidecarError::EventSerialization(error.to_string()))?;
    Ok(bytes)
}

fn validate_sha256_token(value: &str) -> Result<(), ()> {
    let Some(hex) = value.strip_prefix(SHA256_TOKEN_PREFIX) else {
        return Err(());
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(());
    }
    Ok(())
}

fn decode_sha256_token(value: &str) -> Result<[u8; 32], ()> {
    validate_sha256_token(value)?;
    let encoded = value
        .strip_prefix(SHA256_TOKEN_PREFIX)
        .ok_or(())?
        .as_bytes();
    let mut decoded = [0_u8; 32];
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        decoded[index] = (decode_hex_digit(pair[0])? << 4) | decode_hex_digit(pair[1])?;
    }
    Ok(decoded)
}

fn decode_hex_digit(value: u8) -> Result<u8, ()> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(()),
    }
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use std::collections::BTreeMap;

    fn session_id() -> SessionId {
        SessionId::parse("018f6f4f-4231-7d0d-8d91-b8d25cfa90a1").expect("fixed session id")
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct TestEvent {
        kind: String,
        values: BTreeMap<String, u64>,
    }

    fn event(kind: &str, values: &[(&str, u64)]) -> SerializedComponentEvent {
        SerializedComponentEvent::canonical_json(
            1,
            &TestEvent {
                kind: kind.to_string(),
                values: values
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), *value))
                    .collect(),
            },
        )
        .expect("serialize test component event")
    }

    #[test]
    fn empty_roots_are_domain_separated_by_session() {
        let session = session_id();
        let other_session =
            SessionId::parse("018f6f4f-4231-7d0d-8d91-b8d25cfa90a2").expect("fixed session id");
        let realtime =
            ComponentEventPrefixAuthority::empty(session, SessionComponentKind::Realtime);
        let other =
            ComponentEventPrefixAuthority::empty(other_session, SessionComponentKind::Realtime);

        assert_ne!(realtime.root_digest(), other.root_digest());
        assert_eq!(realtime.event_count(), 0);
    }

    #[test]
    fn prepared_suffix_derives_the_only_legal_successor() {
        let predecessor =
            ComponentEventPrefixAuthority::empty(session_id(), SessionComponentKind::Realtime);
        let events = vec![event("snapshot", &[("entries", 2)]), event("apply", &[])];
        let prepared = PreparedComponentEventSuffix::prepare(predecessor.clone(), events.clone())
            .expect("prepare suffix");

        assert_eq!(prepared.base_seq(), 0);
        assert_eq!(prepared.events(), events);
        assert_eq!(prepared.successor().event_count(), 2);
        predecessor
            .verify_serialized_suffix(0, &events, prepared.successor())
            .expect("successor verifies");
    }

    #[test]
    fn durable_full_log_is_verified_before_replay() {
        let predecessor =
            ComponentEventPrefixAuthority::empty(session_id(), SessionComponentKind::Realtime);
        let events = vec![event("snapshot", &[("items", 1)]), event("delta", &[])];
        let successor = predecessor
            .extend_serialized_events(&events)
            .expect("derive successor");
        let rows = events
            .iter()
            .enumerate()
            .map(|(sequence, event)| {
                StoredComponentEventRow::new(sequence as u64, event.bytes().to_vec())
            })
            .collect();
        let verified = VerifiedComponentEventSequence::verify_full(successor, rows)
            .expect("verify durable log");

        let replayed = verified
            .replay(Vec::new(), |seen, sequence, event| {
                let payload: TestEvent = event.decode_payload(1)?;
                seen.push((sequence, payload.kind));
                Ok::<_, SessionComponentSidecarError>(())
            })
            .expect("replay verified rows");
        assert_eq!(
            replayed,
            vec![(0, "snapshot".to_string()), (1, "delta".to_string())]
        );
    }

    #[test]
    fn missing_or_reordered_durable_rows_fail_contiguity() {
        let predecessor =
            ComponentEventPrefixAuthority::empty(session_id(), SessionComponentKind::Realtime);
        let events = vec![event("snapshot", &[]), event("delta", &[])];
        let successor = predecessor
            .extend_serialized_events(&events)
            .expect("derive successor");
        let rows = vec![
            StoredComponentEventRow::new(0, events[0].bytes().to_vec()),
            StoredComponentEventRow::new(2, events[1].bytes().to_vec()),
        ];

        assert!(matches!(
            VerifiedComponentEventSequence::verify_full(successor, rows),
            Err(SessionComponentSidecarError::EventSequenceMismatch {
                expected: 1,
                actual: 2
            })
        ));
    }

    #[test]
    fn semantically_valid_noncanonical_event_bytes_are_rejected() {
        let noncanonical =
            br#"{"schema_version":1,"payload":{"values":{"b":2,"a":1},"kind":"snapshot"}}"#
                .to_vec();
        assert!(matches!(
            SerializedComponentEvent::from_canonical_bytes(noncanonical),
            Err(SessionComponentSidecarError::NonCanonicalEvent)
        ));
    }

    #[test]
    fn payload_tamper_cannot_reuse_the_old_successor_root() {
        let predecessor =
            ComponentEventPrefixAuthority::empty(session_id(), SessionComponentKind::Realtime);
        let original = event("apply", &[("count", 1)]);
        let original_successor = predecessor
            .extend_serialized_events(std::slice::from_ref(&original))
            .expect("derive original successor");
        let replacement = event("apply", &[("count", 2)]);

        assert!(matches!(
            predecessor.verify_serialized_suffix(
                0,
                std::slice::from_ref(&replacement),
                &original_successor
            ),
            Err(SessionComponentSidecarError::SuccessorPrefixMismatch { .. })
        ));
    }

    #[test]
    fn persisted_authority_rejects_a_forged_empty_root() {
        let authority =
            ComponentEventPrefixAuthority::empty(session_id(), SessionComponentKind::Realtime);
        let forged = ComponentEventPrefixAuthority::from_parts(
            authority.version(),
            authority.session_id().clone(),
            authority.component(),
            0,
            ComponentEventPrefixDigest::parse(format!("{SHA256_TOKEN_PREFIX}{}", "0".repeat(64)))
                .expect("canonical token"),
        );
        assert!(matches!(
            forged,
            Err(SessionComponentSidecarError::InvalidEmptyPrefixRoot { .. })
        ));
    }
}
