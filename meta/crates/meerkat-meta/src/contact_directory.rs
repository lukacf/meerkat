//! Contact directory — maps mob IDs to transport info for cross-mob communication.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::auth::peer_keys::decode_pubkey_b64;

/// Transport for reaching an external mob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MobTransport {
    /// Same process — use InprocRegistry namespace lookup.
    Inproc,
    /// Remote process — TCP connection.
    Tcp(String),
    /// Remote process — Unix domain socket.
    Uds(String),
}

/// Entry in the contact directory for one external mob.
///
/// The optional `pubkey` is the peer gateway's Ed25519 signing pubkey —
/// 32 bytes, used by meerkat-comms to verify envelope signatures on real
/// (TCP/UDS) transports. Inproc peers leave it unset and rely on the
/// router's identity map. For non-inproc peers, callers that want signed
/// envelopes either populate `pubkey` from a TOFU bootstrap (fetched via
/// `meta/peer_pubkey`) or fail closed at wire time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactEntry {
    pub mob_id: String,
    pub transport: MobTransport,
    /// 32-byte Ed25519 signing pubkey. `None` is allowed for inproc; for
    /// real transports, [`crate::UnifiedRuntime::wire_local`] rejects when this
    /// is `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<[u8; 32]>,
    /// Whether control responses from this peer must be signed by the
    /// pinned `pubkey`. `None` (the default) means STRICT WHEN PINNED: a
    /// contact that pins a pubkey requires signed control responses, a
    /// contact without one performs no verification. Set to `false` only
    /// to keep talking to a pre-0.8.16 gateway that pins a key but cannot
    /// sign control responses yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_signed_control: Option<bool>,
}

/// Error loading or parsing a contact directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContactDirectoryError {
    /// TOML parsing failed.
    Parse(String),
    /// Invalid transport string.
    InvalidTransport { mob_id: String, value: String },
    /// Pubkey field present but did not decode as 32 bytes of base64.
    InvalidPubkey { mob_id: String, reason: String },
}

impl std::fmt::Display for ContactDirectoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(reason) => write!(f, "contact directory parse error: {reason}"),
            Self::InvalidTransport { mob_id, value } => {
                write!(f, "invalid transport for mob '{mob_id}': {value}")
            }
            Self::InvalidPubkey { mob_id, reason } => {
                write!(f, "invalid pubkey for mob '{mob_id}': {reason}")
            }
        }
    }
}

impl std::error::Error for ContactDirectoryError {}

/// The contact directory — maps mob IDs to connection info.
///
/// Loaded from TOML config at startup. Immutable after construction.
#[derive(Debug, Clone, Default)]
pub struct ContactDirectory {
    entries: BTreeMap<String, ContactEntry>,
}

impl ContactDirectory {
    /// Parse a contact directory from TOML.
    ///
    /// Two value shapes per mob are accepted, side by side:
    ///
    /// ```toml
    /// [mobs]
    /// # Bare-string form (backward compatible, no pubkey).
    /// google-workspace = "inproc"
    ///
    /// # Table form — required for non-inproc peers that want signed
    /// # envelopes. `pubkey` is base64 of the 32-byte Ed25519 verifying
    /// # key, optionally prefixed with `ed25519:` for parity with
    /// # meerkat-comms trust files.
    /// automation-hub = { transport = "tcp://192.168.1.50:9002", pubkey = "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=" }
    /// ```
    pub fn from_toml(text: &str) -> Result<Self, ContactDirectoryError> {
        let table: toml::Value =
            toml::from_str(text).map_err(|e| ContactDirectoryError::Parse(e.to_string()))?;

        let mobs = table
            .get("mobs")
            .and_then(|v| v.as_table())
            .cloned()
            .unwrap_or_default();

        let mut entries = BTreeMap::new();
        for (mob_id, value) in mobs {
            let entry = parse_entry(&mob_id, &value)?;
            entries.insert(mob_id, entry);
        }

        Ok(Self { entries })
    }

    /// Look up a mob by ID.
    pub fn get(&self, mob_id: &str) -> Option<&ContactEntry> {
        self.entries.get(mob_id)
    }

    /// Check if a mob ID is in the directory.
    pub fn contains(&self, mob_id: &str) -> bool {
        self.entries.contains_key(mob_id)
    }

    /// List all entries.
    pub fn list(&self) -> Vec<&ContactEntry> {
        self.entries.values().collect()
    }
}

fn parse_entry(mob_id: &str, value: &toml::Value) -> Result<ContactEntry, ContactDirectoryError> {
    if let Some(s) = value.as_str() {
        let transport =
            parse_transport(s).ok_or_else(|| ContactDirectoryError::InvalidTransport {
                mob_id: mob_id.to_string(),
                value: s.to_string(),
            })?;
        return Ok(ContactEntry {
            mob_id: mob_id.to_string(),
            transport,
            pubkey: None,
            require_signed_control: None,
        });
    }

    if let Some(tbl) = value.as_table() {
        let transport_str = tbl
            .get("transport")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ContactDirectoryError::InvalidTransport {
                mob_id: mob_id.to_string(),
                value: format!("{value}"),
            })?;
        let transport = parse_transport(transport_str).ok_or_else(|| {
            ContactDirectoryError::InvalidTransport {
                mob_id: mob_id.to_string(),
                value: transport_str.to_string(),
            }
        })?;
        let pubkey =
            match tbl.get("pubkey").and_then(|v| v.as_str()) {
                Some(s) => Some(decode_pubkey_b64(s).map_err(|err| {
                    ContactDirectoryError::InvalidPubkey {
                        mob_id: mob_id.to_string(),
                        reason: err.to_string(),
                    }
                })?),
                None => None,
            };
        let require_signed_control =
            match tbl.get("require_signed_control") {
                None => None,
                Some(value) => Some(value.as_bool().ok_or_else(|| {
                    ContactDirectoryError::InvalidTransport {
                        mob_id: mob_id.to_string(),
                        value: format!("require_signed_control must be a boolean, got {value}"),
                    }
                })?),
            };
        return Ok(ContactEntry {
            mob_id: mob_id.to_string(),
            transport,
            pubkey,
            require_signed_control,
        });
    }

    Err(ContactDirectoryError::InvalidTransport {
        mob_id: mob_id.to_string(),
        value: format!("{value}"),
    })
}

/// Parse a transport string (`inproc`, `tcp://host:port`, `uds:///path`).
/// Shared with the control-listen address parser so the two surfaces never
/// drift on address spelling.
pub(crate) fn parse_transport(s: &str) -> Option<MobTransport> {
    if s == "inproc" {
        return Some(MobTransport::Inproc);
    }
    if let Some(addr) = s.strip_prefix("tcp://") {
        return Some(MobTransport::Tcp(addr.to_string()));
    }
    if let Some(path) = s.strip_prefix("uds://") {
        return Some(MobTransport::Uds(path.to_string()));
    }
    None
}

/// Parse `"member::mob_id"` into `(member, mob_id)`.
///
/// Uses `::` as separator (not `@`) to avoid collision with email-based
/// member IDs like `personal:user@example.com`.
///
/// Only matches if `mob_id` is a known entry in the directory —
/// bare member names and unknown mob IDs return `None`.
pub fn parse_cross_mob_address<'a>(
    address: &'a str,
    directory: &ContactDirectory,
) -> Option<(&'a str, &'a str)> {
    let sep = address.rfind("::")?;
    let member = &address[..sep];
    let mob_id = &address[sep + 2..];
    if member.is_empty() || mob_id.is_empty() {
        return None;
    }
    if !directory.contains(mob_id) {
        return None;
    }
    Some((member, mob_id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_toml() {
        let dir = ContactDirectory::from_toml(
            r#"
            [mobs]
            google-workspace = "inproc"
            automation-hub = "tcp://192.168.1.50:9002"
            device-hub = "uds:///var/run/meerkat/device-hub.sock"
            "#,
        )
        .unwrap();
        assert_eq!(dir.list().len(), 3);
        assert_eq!(
            dir.get("google-workspace").unwrap().transport,
            MobTransport::Inproc
        );
        assert_eq!(
            dir.get("automation-hub").unwrap().transport,
            MobTransport::Tcp("192.168.1.50:9002".to_string())
        );
        assert_eq!(
            dir.get("device-hub").unwrap().transport,
            MobTransport::Uds("/var/run/meerkat/device-hub.sock".to_string())
        );
        // Bare-string form leaves pubkey unset (backward compatible).
        for entry in dir.list() {
            assert!(entry.pubkey.is_none(), "{} pubkey", entry.mob_id);
        }
    }

    #[test]
    fn parse_table_form_carries_pubkey() {
        let pubkey_b64 = "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=";
        let dir = ContactDirectory::from_toml(&format!(
            r#"
            [mobs]
            automation-hub = {{ transport = "tcp://192.168.1.50:9002", pubkey = "{pubkey_b64}" }}
            "#,
        ))
        .unwrap();
        let entry = dir.get("automation-hub").unwrap();
        assert!(matches!(entry.transport, MobTransport::Tcp(_)));
        assert_eq!(entry.pubkey, Some([42u8; 32]));
    }

    #[test]
    fn parse_table_form_accepts_ed25519_prefix() {
        let dir = ContactDirectory::from_toml(
            r#"
            [mobs]
            automation-hub = { transport = "tcp://1.2.3.4:9000", pubkey = "ed25519:KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=" }
            "#,
        )
        .unwrap();
        assert_eq!(dir.get("automation-hub").unwrap().pubkey, Some([42u8; 32]));
    }

    #[test]
    fn parse_table_form_rejects_bad_pubkey() {
        let result = ContactDirectory::from_toml(
            r#"
            [mobs]
            automation-hub = { transport = "tcp://1.2.3.4:9000", pubkey = "not-base64!!" }
            "#,
        );
        assert!(matches!(
            result,
            Err(ContactDirectoryError::InvalidPubkey { .. })
        ));
    }

    #[test]
    fn parse_empty_toml() {
        let dir = ContactDirectory::from_toml("[mobs]").unwrap();
        assert!(dir.list().is_empty());
    }

    #[test]
    fn parse_missing_mobs_section() {
        let dir = ContactDirectory::from_toml("").unwrap();
        assert!(dir.list().is_empty());
    }

    #[test]
    fn parse_invalid_transport() {
        let result = ContactDirectory::from_toml(
            r#"
            [mobs]
            bad = "ftp://nope"
            "#,
        );
        assert!(matches!(
            result,
            Err(ContactDirectoryError::InvalidTransport { .. })
        ));
    }

    #[test]
    fn cross_mob_address_parsing() {
        let dir = ContactDirectory::from_toml(
            r#"
            [mobs]
            google-workspace = "inproc"
            "#,
        )
        .unwrap();

        // Valid cross-mob address
        assert_eq!(
            parse_cross_mob_address("calendar::google-workspace", &dir),
            Some(("calendar", "google-workspace"))
        );

        // Bare member name — no match
        assert_eq!(parse_cross_mob_address("calendar", &dir), None);

        // Unknown mob — no match
        assert_eq!(parse_cross_mob_address("calendar::unknown-mob", &dir), None);

        // Email-based member ID — no match (no :: separator)
        assert_eq!(
            parse_cross_mob_address("personal:user@example.com", &dir),
            None
        );

        // Empty parts
        assert_eq!(parse_cross_mob_address("::google-workspace", &dir), None);
        assert_eq!(parse_cross_mob_address("calendar::", &dir), None);
    }
}
