//! Identity, token, reference and digest types of durable decisions.
//!
//! Every type here validates on construction and on deserialization, so a
//! stored row that carries an invalid value fails to decode and is kept
//! unreadable instead of being read as another value.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use sha2::{Digest, Sha256};

use super::DecisionContractError;
use crate::{PrincipalKind, PrincipalQualification, PrincipalRef};

/// Largest token (a decision kind, a renderer, an ingress), in bytes.
pub const DECISION_TOKEN_MAX_BYTES: usize = 64;
/// Largest reference (a subject key, an operation id, an event, channel,
/// message or verdict call reference), in bytes.
pub const DECISION_REFERENCE_MAX_BYTES: usize = 1024;
/// Largest approver id, in bytes.
pub const DECISION_APPROVER_ID_MAX_BYTES: usize = 256;
/// Largest rendering of a held request, in bytes.
pub const DECISION_RENDERING_MAX_BYTES: usize = 256 * 1024;
/// Largest held request body, in bytes of its JSON text.
pub const DECISION_REQUEST_MAX_BYTES: usize = 256 * 1024;

const DECISION_ID_PREFIX: &str = "decision:";
const SHA256_PREFIX: &str = "sha256:";
const PRINCIPAL_KEY_PREFIX: &str = "principal:";

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= DECISION_TOKEN_MAX_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn is_reference(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= DECISION_REFERENCE_MAX_BYTES
        && !value.chars().any(char::is_control)
}

fn is_approver_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= DECISION_APPROVER_ID_MAX_BYTES
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn is_sha256(value: &str) -> bool {
    value
        .strip_prefix(SHA256_PREFIX)
        .is_some_and(is_lower_hex_64)
}

fn is_decision_id(value: &str) -> bool {
    value
        .strip_prefix(DECISION_ID_PREFIX)
        .is_some_and(is_lower_hex_64)
}

fn is_rendering(value: &str) -> bool {
    !value.is_empty() && value.len() <= DECISION_RENDERING_MAX_BYTES
}

fn principal_kind_token(kind: PrincipalKind) -> &'static str {
    match kind {
        PrincipalKind::Human => "human",
        PrincipalKind::PersonalAgent => "personal_agent",
        PrincipalKind::SharedAgent => "shared_agent",
        PrincipalKind::RuntimeHost => "runtime_host",
        PrincipalKind::ServiceAccount => "service_account",
    }
}

fn principal_key_text(kind: &str, domain: &str, id: &str) -> String {
    format!(
        "{PRINCIPAL_KEY_PREFIX}{}",
        serde_json::json!([kind, domain, id])
    )
}

fn is_principal_key(value: &str) -> bool {
    let Some(body) = value.strip_prefix(PRINCIPAL_KEY_PREFIX) else {
        return false;
    };
    let Ok((kind, domain, id)) = serde_json::from_str::<(String, String, String)>(body) else {
        return false;
    };
    let known_kind = [
        PrincipalKind::Human,
        PrincipalKind::PersonalAgent,
        PrincipalKind::SharedAgent,
        PrincipalKind::RuntimeHost,
        PrincipalKind::ServiceAccount,
    ]
    .into_iter()
    .any(|candidate| principal_kind_token(candidate) == kind);
    known_kind
        && !domain.trim().is_empty()
        && !id.trim().is_empty()
        && principal_key_text(&kind, &domain, &id) == value
}

/// A string newtype that validates on construction and on deserialization.
macro_rules! validated_string {
    ($(#[$meta:meta])* $name:ident, $check:path, $what:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            #[doc = concat!("Validate one ", $what, ".")]
            ///
            /// # Errors
            /// [`DecisionContractError::Invalid`] when the value is not one.
            pub fn new(value: impl Into<String>) -> Result<Self, DecisionContractError> {
                let value = value.into();
                if $check(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err(DecisionContractError::Invalid { what: $what })
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = DecisionContractError;

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
                f.write_str(&self.0)
            }
        }
    };
}

validated_string!(
    /// An opaque host token naming what kind of operation a decision holds
    /// (`[a-z0-9._-]{1,64}`, for example `release`). Core never interprets
    /// it.
    DecisionKind,
    is_token,
    "decision kind"
);

validated_string!(
    /// The host's key of the subject a decision is held for. With the kind
    /// it derives the decision id, so one subject of one kind is held by one
    /// decision.
    DecisionSubjectKey,
    is_reference,
    "subject key"
);

validated_string!(
    /// The id of the held operation, as its owner names it.
    DecisionOperationId,
    is_reference,
    "operation id"
);

validated_string!(
    /// An opaque reference into a host system: the event a decision arrived
    /// on, the channel and message a presentation was shown in, a reviewer's
    /// verdict call, an imported consent's record.
    DecisionReference,
    is_reference,
    "reference"
);

validated_string!(
    /// The host's id of who may approve a decision: a person, a role or a
    /// group. The host maps it to the humans who may act for it; Core only
    /// compares it.
    ApproverId,
    is_approver_id,
    "approver id"
);

validated_string!(
    /// The token of the renderer that produced a decision's rendering.
    DecisionRenderer,
    is_token,
    "renderer"
);

validated_string!(
    /// How an attested decision reached the host (`[a-z0-9._-]{1,64}`, for
    /// example `web` or a channel name).
    DecisionIngress,
    is_token,
    "ingress"
);

validated_string!(
    /// A content digest: `sha256:` and 64 lowercase hex digits.
    Sha256Digest,
    is_sha256,
    "sha256 digest"
);

impl Sha256Digest {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(format!("{SHA256_PREFIX}{:x}", Sha256::digest(bytes)))
    }
}

validated_string!(
    /// One durable decision's id: `decision:` and the lowercase hex sha256
    /// of the canonical JSON array `[kind, subject_key]`.
    DecisionId,
    is_decision_id,
    "decision id"
);

impl DecisionId {
    /// The id of the decision held for `subject_key` of `kind`.
    #[must_use]
    pub fn derive(kind: &DecisionKind, subject_key: &DecisionSubjectKey) -> Self {
        let material = serde_json::json!([kind.as_str(), subject_key.as_str()]).to_string();
        Self(format!(
            "{DECISION_ID_PREFIX}{:x}",
            Sha256::digest(material.as_bytes())
        ))
    }
}

validated_string!(
    /// The canonical key of a qualified principal: `principal:` and the
    /// canonical JSON array `[kind, trust_domain, id]`. Two principals have
    /// the same key exactly when their kind, trust domain and id are equal.
    DecisionPrincipalKey,
    is_principal_key,
    "principal key"
);

impl DecisionPrincipalKey {
    /// The key of `principal`, which must name an explicit trust domain.
    ///
    /// # Errors
    /// [`DecisionContractError::UnqualifiedPrincipal`] for a principal
    /// without a trust domain or with an invalid id.
    pub fn of(principal: &PrincipalRef) -> Result<Self, DecisionContractError> {
        principal
            .validate_qualified()
            .map_err(|_| DecisionContractError::UnqualifiedPrincipal)?;
        let PrincipalQualification::Qualified(domain) = &principal.qualification else {
            return Err(DecisionContractError::UnqualifiedPrincipal);
        };
        Ok(Self(principal_key_text(
            principal_kind_token(principal.kind),
            domain.as_str(),
            principal.id.as_str(),
        )))
    }
}

validated_string!(
    /// The one canonical rendering of a held request, at most 256 KiB. Its
    /// sha256 is the decision's subject digest: the only digest a decision
    /// binds to.
    DecisionRendering,
    is_rendering,
    "rendering"
);

impl DecisionRendering {
    /// The subject digest of this rendering, computed by Core.
    #[must_use]
    pub fn subject_digest(&self) -> Sha256Digest {
        Sha256Digest::of(self.0.as_bytes())
    }
}

/// The held request: opaque JSON the operation's owner executes, at most
/// 256 KiB of JSON text. Core stores and returns it and never reads it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "Box<RawValue>", into = "Box<RawValue>")]
pub struct DecisionRequestBody(Box<RawValue>);

impl DecisionRequestBody {
    /// Keep `raw` as a held request body.
    ///
    /// # Errors
    /// [`DecisionContractError::TooLarge`] past 256 KiB.
    pub fn new(raw: Box<RawValue>) -> Result<Self, DecisionContractError> {
        if raw.get().len() > DECISION_REQUEST_MAX_BYTES {
            return Err(DecisionContractError::TooLarge {
                what: "request",
                max_bytes: DECISION_REQUEST_MAX_BYTES,
            });
        }
        Ok(Self(raw))
    }

    /// The held request body of `value`.
    ///
    /// # Errors
    /// [`DecisionContractError::Encoding`] when `value` cannot be encoded,
    /// and [`DecisionContractError::TooLarge`] past 256 KiB.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, DecisionContractError> {
        let raw = serde_json::value::to_raw_value(value)
            .map_err(|error| DecisionContractError::Encoding(error.to_string()))?;
        Self::new(raw)
    }

    /// The request's JSON text, exactly as it was held.
    #[must_use]
    pub fn get(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for DecisionRequestBody {
    fn eq(&self, other: &Self) -> bool {
        self.0.get() == other.0.get()
    }
}

impl Eq for DecisionRequestBody {}

impl TryFrom<Box<RawValue>> for DecisionRequestBody {
    type Error = DecisionContractError;

    fn try_from(raw: Box<RawValue>) -> Result<Self, Self::Error> {
        Self::new(raw)
    }
}

impl From<DecisionRequestBody> for Box<RawValue> {
    fn from(body: DecisionRequestBody) -> Self {
        body.0
    }
}
