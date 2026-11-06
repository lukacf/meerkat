//! Exact protected parent protocol-1 shape codec. Parsing is not sender authentication.
//! Only the separately verified protected transport may supply authority.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub(super) const MAX_PACKET: usize = 4096;

/// No packet bytes or identity fields appear in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Refused;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub(super) struct Nonce(String);
impl TryFrom<String> for Nonce {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("invalid protocol identity");
        }
        Ok(Self(value))
    }
}
impl From<Nonce> for String {
    fn from(value: Nonce) -> String {
        value.0
    }
}
impl Nonce {
    pub(super) fn fresh() -> Self {
        Self(format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ))
    }
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Envelope {
    pub(super) channel: Nonce,
    pub(super) invocation: Nonce,
    pub(super) reservation: Nonce,
    pub(super) mapping_sha256: Nonce,
    pub(super) request_id: Nonce,
}
impl Envelope {
    /// Correlation only. The caller must separately authenticate every packet.
    pub(super) fn same_owner(&self, other: &Self) -> bool {
        self.channel == other.channel
            && self.invocation == other.invocation
            && self.reservation == other.reservation
            && self.mapping_sha256 == other.mapping_sha256
    }
    fn fields(&self, kind: &str) -> Value {
        serde_json::json!({"version":1,"kind":kind,"channel":self.channel,
            "invocation":self.invocation,"reservation":self.reservation,
            "mapping_sha256":self.mapping_sha256,"request_id":self.request_id})
    }
    pub(super) fn bound(&self, control: &Nonce) -> Result<Vec<u8>, Refused> {
        let mut value = self.fields("BOUND");
        value["control_id"] = serde_json::json!(control);
        encode(value)
    }
    pub(super) fn fence_ack(&self, control: &Nonce) -> Result<Vec<u8>, Refused> {
        let mut value = self.fields("FENCE_ACK");
        value["control_id"] = serde_json::json!(control);
        encode(value)
    }
    pub(super) fn disposed(
        &self,
        control: &Nonce,
        disposition: Disposition,
    ) -> Result<Vec<u8>, Refused> {
        let mut value = self.fields("DISPOSED_ACK");
        value["control_id"] = serde_json::json!(control);
        value["disposition"] = Value::String(
            match disposition {
                Disposition::Joined => "joined",
                Disposition::Unconfirmed => "unconfirmed",
            }
            .into(),
        );
        encode(value)
    }
}

#[derive(Clone, Copy)]
pub(super) enum Disposition {
    Joined,
    Unconfirmed,
}

pub(super) struct Attestation {
    pub(super) envelope: Envelope,
    pub(super) broker_instance: Nonce,
    pub(super) challenge: Nonce,
    pub(super) issuer: String,
    pub(super) subject: String,
    pub(super) scope: String,
    pub(super) workspace: Nonce,
    pub(super) origin: String,
    pub(super) evidence_id: Nonce,
    pub(super) expires_at_ms: u64,
}
pub(super) struct Control {
    pub(super) envelope: Envelope,
    pub(super) control_id: Nonce,
}
pub(super) enum ParentRecord {
    Attest(Attestation),
    Activate(Control),
    Fence(Control),
}

// Flattened serde fields deliberately avoided: the closed records reject every
// unknown field and duplicate before a typed value is made.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttestWire {
    version: u8,
    kind: String,
    channel: Nonce,
    invocation: Nonce,
    reservation: Nonce,
    mapping_sha256: Nonce,
    request_id: Nonce,
    broker_instance: Nonce,
    challenge: Nonce,
    issuer: String,
    subject: String,
    scope: String,
    workspace: Nonce,
    origin: String,
    evidence_id: Nonce,
    evidence_revision: u64,
    expires_at_ms: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlWire {
    version: u8,
    kind: String,
    channel: Nonce,
    invocation: Nonce,
    reservation: Nonce,
    mapping_sha256: Nonce,
    request_id: Nonce,
    control_id: Nonce,
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

// Match the original login gateway's opaque scope shape. This validates the
// packet syntax only; it cannot establish ownership of a workspace login.
fn scope_shape(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

struct FlatObject(Map<String, Value>);
impl<'de> Deserialize<'de> for FlatObject {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = FlatObject;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("one flat protocol object")
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<Self::Value, A::Error> {
                let mut fields = Map::new();
                while let Some((key, value)) = access.next_entry::<String, Value>()? {
                    if fields.contains_key(&key)
                        || fields.len() >= 18
                        || !(value.is_string() || value.as_u64().is_some())
                    {
                        return Err(serde::de::Error::custom("invalid protocol fields"));
                    }
                    fields.insert(key, value);
                }
                Ok(FlatObject(fields))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<ParentRecord, Refused> {
    if bytes.is_empty() || bytes.len() > MAX_PACKET {
        return Err(Refused);
    }
    let FlatObject(fields) = serde_json::from_slice(bytes).map_err(|_| Refused)?;
    match fields.get("kind").and_then(Value::as_str) {
        Some("PARENT_ATTEST") => {
            let w: AttestWire =
                serde_json::from_value(Value::Object(fields)).map_err(|_| Refused)?;
            if w.version != 1
                || w.kind != "PARENT_ATTEST"
                || w.evidence_revision != 1
                || w.expires_at_ms == 0
                || !bounded_text(&w.issuer, 1024)
                || !bounded_text(&w.subject, 256)
                || !scope_shape(&w.scope)
                || !bounded_text(&w.origin, 2048)
            {
                return Err(Refused);
            }
            Ok(ParentRecord::Attest(Attestation {
                envelope: Envelope {
                    channel: w.channel,
                    invocation: w.invocation,
                    reservation: w.reservation,
                    mapping_sha256: w.mapping_sha256,
                    request_id: w.request_id,
                },
                broker_instance: w.broker_instance,
                challenge: w.challenge,
                issuer: w.issuer,
                subject: w.subject,
                scope: w.scope,
                workspace: w.workspace,
                origin: w.origin,
                evidence_id: w.evidence_id,
                expires_at_ms: w.expires_at_ms,
            }))
        }
        Some("ACTIVATE" | "FENCE") => {
            let w: ControlWire =
                serde_json::from_value(Value::Object(fields)).map_err(|_| Refused)?;
            if w.version != 1 {
                return Err(Refused);
            }
            let c = Control {
                envelope: Envelope {
                    channel: w.channel,
                    invocation: w.invocation,
                    reservation: w.reservation,
                    mapping_sha256: w.mapping_sha256,
                    request_id: w.request_id,
                },
                control_id: w.control_id,
            };
            match w.kind.as_str() {
                "ACTIVATE" => Ok(ParentRecord::Activate(c)),
                "FENCE" => Ok(ParentRecord::Fence(c)),
                _ => Err(Refused),
            }
        }
        _ => Err(Refused),
    }
}

pub(super) fn hello(
    invocation: &Nonce,
    challenge: &Nonce,
    mapping: &Nonce,
) -> Result<Vec<u8>, Refused> {
    encode(
        serde_json::json!({"version":1,"kind":"HELLO","invocation":invocation,
        "challenge":challenge,"mapping_sha256":mapping}),
    )
}
fn encode(value: Value) -> Result<Vec<u8>, Refused> {
    let bytes = serde_json::to_vec(&value).map_err(|_| Refused)?;
    if bytes.is_empty() || bytes.len() > MAX_PACKET {
        return Err(Refused);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
