//! Durable per-session MCP attachment intent.
//!
//! A session's accepted request to have an MCP server attached is recorded in
//! the session's own durable build state, so a restarted host can re-stage it
//! through the native attachment owners. The record is a request, never a
//! permit: it holds a bounded, nonsecret descriptor identity (no header or
//! environment values, credentials, tokens or prepared controls; arguments
//! and credential-bearing URL parts only as an HMAC under the realm's
//! protected identity key, never in the clear or as a plain digest). Restore
//! reacquires the current installation, account and native control and
//! rechecks them; a changed installation or account is refused locally.

use serde::{Deserialize, Serialize};

use crate::mcp_config::{McpOAuthAccountSelection, McpServerConfig, McpTransportConfig};

/// Upper bound on recorded attachment intents per session.
pub const MAX_MCP_ATTACHMENT_INTENTS: usize = 64;

/// Every new intent revision is above this base. It separates new revisions
/// from the ordinary predecessor history (counted from 1 per entry), but the
/// guarantee against reuse is the session counter, which is raised to every
/// retained revision before any change and keeps that floor after removal.
pub const MCP_ATTACHMENT_REVISION_BASE: u64 = 1 << 32;

/// Where an attached server is reached, without secrets.
///
/// Only nonsecret coordinates are recorded in the clear. Credential-bearing
/// parts of the native descriptor (URL userinfo, query and fragment; command
/// arguments) are `withheld`: never recorded, not even as a plain digest.
/// They are bound exactly by an HMAC-SHA-256 under the realm's protected
/// identity key ([`crate::auth::McpAttachmentIdentityKey`]), which lives in
/// the realm's token store, outside session state. A withheld target without
/// that binding (no key was available) cannot prove the target unchanged, and
/// restore asks for a fresh explicit attachment request instead of
/// re-staging it. Environment and header values are not part of the identity.
/// A stdio server's working directory is, in the clear like its command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpAttachmentTarget {
    Http {
        /// The URL with userinfo, query and fragment redacted.
        location: String,
        /// Whether redaction removed anything.
        withheld: bool,
        /// The keyed binding of the whole URL, when anything was withheld.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<McpAttachmentTargetBinding>,
    },
    Stdio {
        command: String,
        /// Sorted environment variable names; their values are not recorded.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        env_names: Vec<String>,
        /// Sorted names of the declared service secrets; their values are
        /// never part of configuration or of this record.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        secret_env_names: Vec<String>,
        /// Whether the command has arguments, which are not recorded.
        withheld: bool,
        /// The keyed binding of the command and its ordered arguments, when
        /// there are arguments.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binding: Option<McpAttachmentTargetBinding>,
        /// The configured working directory, in the clear like the command;
        /// absent when none is configured (and in records written before
        /// the field existed), so a changed directory is a changed target.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// A record written before this identity format. It is kept so its name,
    /// disposition and revision still fence callbacks, but it proves nothing
    /// about the target: restore asks for a fresh explicit request.
    Predecessor,
}

/// An opaque keyed binding of a target's withheld parts: the id of the
/// realm's identity key it was made with and the MAC. Comparable only under
/// that same key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAttachmentTargetBinding {
    pub key_id: String,
    pub mac: String,
}

/// Domain of a target's keyed binding.
const TARGET_BINDING_DOMAIN: &str = "meerkat.mcp_attachment.target.v1";

impl McpAttachmentTarget {
    fn of(
        transport: &McpTransportConfig,
        key: Option<&crate::auth::McpAttachmentIdentityKey>,
    ) -> Self {
        let bind = |parts: Vec<&str>| {
            let key = key?;
            Some(McpAttachmentTargetBinding {
                key_id: key.id().to_owned(),
                mac: key.mac(TARGET_BINDING_DOMAIN, parts)?,
            })
        };
        match transport {
            McpTransportConfig::Http(http) => {
                let location = crate::redact::redacted_url(&http.url);
                let withheld = location != http.url;
                let binding = if withheld {
                    bind(vec!["http", http.url.as_str()])
                } else {
                    None
                };
                Self::Http {
                    location,
                    withheld,
                    binding,
                }
            }
            McpTransportConfig::Stdio(stdio) => {
                let mut env_names: Vec<String> = stdio.env.keys().cloned().collect();
                env_names.sort_unstable();
                let withheld = !stdio.args.is_empty();
                let binding = if withheld {
                    bind(
                        ["stdio", stdio.command.as_str()]
                            .into_iter()
                            .chain(stdio.args.iter().map(String::as_str))
                            .collect(),
                    )
                } else {
                    None
                };
                Self::Stdio {
                    command: stdio.command.clone(),
                    env_names,
                    secret_env_names: stdio.secret_env.iter().cloned().collect(),
                    withheld,
                    binding,
                    cwd: stdio.cwd.clone(),
                }
            }
        }
    }

    /// Whether this target proves an unchanged installation when equal: it
    /// recorded every part that defines it, in the clear or bound under the
    /// realm's identity key.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        match self {
            Self::Http {
                withheld, binding, ..
            }
            | Self::Stdio {
                withheld, binding, ..
            } => !withheld || binding.is_some(),
            Self::Predecessor => false,
        }
    }

    /// The keyed binding of this target's withheld parts, if it has one.
    #[must_use]
    pub fn binding(&self) -> Option<&McpAttachmentTargetBinding> {
        match self {
            Self::Http { binding, .. } | Self::Stdio { binding, .. } => binding.as_ref(),
            Self::Predecessor => None,
        }
    }
}

/// The connector credential slot, account and stable credential context an
/// installation is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConnectorAccountRef {
    pub realm_id: String,
    pub slot_id: String,
    pub issuer: String,
    pub client: String,
    pub resource: String,
    pub strategy_id: String,
    pub account: String,
}

/// The nonsecret identity of the server descriptor an intent was accepted
/// for. Two descriptors with equal identities name the same installation,
/// target and account binding.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpAttachmentDescriptorIdentity {
    pub server_name: String,
    pub target: McpAttachmentTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_account_selection: Option<McpOAuthAccountSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_account: Option<McpConnectorAccountRef>,
}

/// Redacts the OAuth account, as the native HTTP config does.
impl std::fmt::Debug for McpAttachmentDescriptorIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAttachmentDescriptorIdentity")
            .field("server_name", &self.server_name)
            .field("target", &self.target)
            .field(
                "oauth_account",
                &crate::redact::optional(&self.oauth_account),
            )
            .field("oauth_account_selection", &self.oauth_account_selection)
            .field("connector_account", &self.connector_account)
            .finish()
    }
}

impl McpAttachmentDescriptorIdentity {
    /// The identity of `config`. Header and environment values are not part
    /// of it; arguments and credential-bearing URL parts only as a keyed
    /// binding under `key`, the realm's identity key (resolved, and persisted
    /// if new, before the intent is recorded). Without a key such a target is
    /// unbound and restores as renewal-required.
    #[must_use]
    pub fn of(
        config: &McpServerConfig,
        key: Option<&crate::auth::McpAttachmentIdentityKey>,
    ) -> Self {
        let (oauth_account, oauth_account_selection) = match &config.transport {
            McpTransportConfig::Http(http) => {
                (http.oauth_account.clone(), http.oauth_account_selection)
            }
            McpTransportConfig::Stdio(_) => (None, None),
        };
        Self {
            server_name: config.name.clone(),
            target: McpAttachmentTarget::of(&config.transport, key),
            oauth_account,
            oauth_account_selection,
            connector_account: config
                .connector
                .as_ref()
                .map(|binding| McpConnectorAccountRef {
                    realm_id: binding.realm_id.clone(),
                    slot_id: binding.slot_id.clone(),
                    issuer: binding.issuer.clone(),
                    client: binding.client.clone(),
                    resource: binding.resource.clone(),
                    strategy_id: binding.strategy_id.clone(),
                    account: binding.account.clone(),
                }),
        }
    }

    /// Whether `current` keeps this identity's account binding.
    #[must_use]
    pub fn same_account(&self, current: &Self) -> bool {
        self.oauth_account == current.oauth_account
            && self.oauth_account_selection == current.oauth_account_selection
            && self.connector_account == current.connector_account
    }
}

/// The typed outcome a resumed session's recorded MCP attachment reports on
/// the wire when restore did not simply re-stage it.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAttachmentRestoreWireOutcome {
    // Suppressed by an explicit sign-out; not connected.
    Suppressed,
    // No current installation of that name.
    Retired,
    // The current installation reaches a different target.
    Rebound,
    // The current installation is bound to a different account.
    AccountChanged,
    // Native attachment refusals.
    Denied,
    Unavailable,
    NotCurrent,
    ReviewRequired,
    Mismatch,
    // Staging the current descriptor failed.
    StageFailed,
    // The record cannot prove the current installation unchanged (its
    // credential-bearing parts are unbound or bound under a lost identity
    // key, or it predates the identity format); a fresh explicit attachment
    // request is required.
    RenewalRequired,
}

impl From<crate::mcp_attachment::McpAttachmentRefusal> for McpAttachmentRestoreWireOutcome {
    fn from(refusal: crate::mcp_attachment::McpAttachmentRefusal) -> Self {
        use crate::mcp_attachment::McpAttachmentRefusal;
        match refusal {
            McpAttachmentRefusal::Denied => Self::Denied,
            McpAttachmentRefusal::Unavailable => Self::Unavailable,
            McpAttachmentRefusal::NotCurrent => Self::NotCurrent,
            McpAttachmentRefusal::ReviewRequired => Self::ReviewRequired,
            McpAttachmentRefusal::Mismatch => Self::Mismatch,
        }
    }
}

/// Whether the recorded request may be revived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAttachmentIntentDisposition {
    /// Accepted and still requested: restore re-stages it (an absent
    /// credential re-enters the ordinary awaiting-authorization path).
    Requested,
    /// The session's explicit sign-out suppressed it: restore does not
    /// connect it until a fresh explicit login reinstates it.
    SuppressedByLogout,
}

/// One accepted attachment request of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpSessionAttachmentIntent {
    pub descriptor: McpAttachmentDescriptorIdentity,
    pub disposition: McpAttachmentIntentDisposition,
    /// Taken from the session's intent revision counter by every change of
    /// this entry. The counter outlives removal and is never reused, so a
    /// stale callback cannot match a later entry of the same name.
    pub revision: u64,
}

/// A change to a session's recorded attachment intents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpAttachmentIntentMutation {
    /// Record an accepted request, before connection or awaiting sign-in.
    Request {
        descriptor: McpAttachmentDescriptorIdentity,
    },
    /// Explicit removal withdraws the request.
    Withdraw { server_name: String },
    /// Explicit sign-out suppresses revival of the request.
    SuppressByLogout { server_name: String },
    /// A fresh explicit login reinstates a suppressed request, only while it
    /// is still at the revision the caller observed.
    Reinstate {
        server_name: String,
        expected_revision: u64,
    },
}

/// The recorded result of one mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAttachmentIntentRecorded {
    pub server_name: String,
    /// The entry's revision after the mutation; `None` when it was withdrawn
    /// or did not exist.
    pub revision: Option<u64>,
    /// Whether the mutation changed the recorded intents.
    pub changed: bool,
}

/// Why a mutation could not be recorded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpAttachmentIntentError {
    #[error("session records at most {MAX_MCP_ATTACHMENT_INTENTS} MCP attachment intents")]
    TooMany,
    #[error("the session's MCP attachment intent revisions are exhausted")]
    RevisionsExhausted,
    #[error("MCP attachment intent for '{server_name}' is at revision {current:?}, not {expected}")]
    StaleRevision {
        server_name: String,
        expected: u64,
        current: Option<u64>,
    },
}

/// Apply `mutation` to `intents`. `last_revision` is the session's intent
/// revision counter: it is first raised to every retained revision, and every
/// change takes the next value above it and above
/// [`MCP_ATTACHMENT_REVISION_BASE`], so a value is never reused, even after
/// its row is removed; exhausting it fails closed.
pub fn apply_mcp_attachment_intent_mutation(
    intents: &mut Vec<McpSessionAttachmentIntent>,
    last_revision: &mut u64,
    mutation: McpAttachmentIntentMutation,
) -> Result<McpAttachmentIntentRecorded, McpAttachmentIntentError> {
    // The session counter is first raised to every retained revision (a
    // predecessor state had no counter, and its revisions may be any u64),
    // so removing a row never loses its high-water mark; a raised counter is
    // a change to persist even when the mutation itself changes nothing.
    let observed = intents
        .iter()
        .map(|intent| intent.revision)
        .fold(*last_revision, u64::max);
    let normalized = observed != *last_revision;
    *last_revision = observed;
    let next_revision = observed
        .max(MCP_ATTACHMENT_REVISION_BASE)
        .checked_add(1)
        .ok_or(McpAttachmentIntentError::RevisionsExhausted);
    apply_normalized(intents, last_revision, next_revision, mutation).map(|mut recorded| {
        recorded.changed |= normalized;
        recorded
    })
}

fn apply_normalized(
    intents: &mut Vec<McpSessionAttachmentIntent>,
    last_revision: &mut u64,
    next_revision: Result<u64, McpAttachmentIntentError>,
    mutation: McpAttachmentIntentMutation,
) -> Result<McpAttachmentIntentRecorded, McpAttachmentIntentError> {
    fn position(intents: &[McpSessionAttachmentIntent], name: &str) -> Option<usize> {
        intents
            .iter()
            .position(|intent| intent.descriptor.server_name == name)
    }
    match mutation {
        McpAttachmentIntentMutation::Request { descriptor } => {
            let server_name = descriptor.server_name.clone();
            let index = position(intents, &server_name);
            if index.is_none() && intents.len() >= MAX_MCP_ATTACHMENT_INTENTS {
                return Err(McpAttachmentIntentError::TooMany);
            }
            let revision = next_revision?;
            match index {
                Some(index) => {
                    let entry = &mut intents[index];
                    entry.descriptor = descriptor;
                    entry.disposition = McpAttachmentIntentDisposition::Requested;
                    entry.revision = revision;
                }
                None => intents.push(McpSessionAttachmentIntent {
                    descriptor,
                    disposition: McpAttachmentIntentDisposition::Requested,
                    revision,
                }),
            }
            *last_revision = revision;
            Ok(McpAttachmentIntentRecorded {
                server_name,
                revision: Some(revision),
                changed: true,
            })
        }
        McpAttachmentIntentMutation::Withdraw { server_name } => {
            let changed = match position(intents, &server_name) {
                Some(index) => {
                    intents.remove(index);
                    true
                }
                None => false,
            };
            Ok(McpAttachmentIntentRecorded {
                server_name,
                revision: None,
                changed,
            })
        }
        McpAttachmentIntentMutation::SuppressByLogout { server_name } => {
            let Some(index) = position(intents, &server_name) else {
                return Ok(McpAttachmentIntentRecorded {
                    server_name,
                    revision: None,
                    changed: false,
                });
            };
            let revision = next_revision?;
            let entry = &mut intents[index];
            entry.disposition = McpAttachmentIntentDisposition::SuppressedByLogout;
            entry.revision = revision;
            *last_revision = revision;
            Ok(McpAttachmentIntentRecorded {
                server_name,
                revision: Some(revision),
                changed: true,
            })
        }
        McpAttachmentIntentMutation::Reinstate {
            server_name,
            expected_revision,
        } => {
            let Some(index) = position(intents, &server_name) else {
                return Err(McpAttachmentIntentError::StaleRevision {
                    server_name,
                    expected: expected_revision,
                    current: None,
                });
            };
            let current = intents[index].revision;
            if current != expected_revision {
                return Err(McpAttachmentIntentError::StaleRevision {
                    server_name,
                    expected: expected_revision,
                    current: Some(current),
                });
            }
            let revision = next_revision?;
            let entry = &mut intents[index];
            entry.disposition = McpAttachmentIntentDisposition::Requested;
            entry.revision = revision;
            *last_revision = revision;
            Ok(McpAttachmentIntentRecorded {
                server_name,
                revision: Some(revision),
                changed: true,
            })
        }
    }
}

/// Decode a session's recorded intents, keeping a row whose descriptor is not
/// in the current identity format (a predecessor record) as a
/// [`McpAttachmentTarget::Predecessor`] with its name, disposition and
/// revision, so an old record never fails the session's build state and is
/// never treated as an approved current descriptor. A row without even a
/// readable name, disposition and revision is dropped: absent, it is never
/// restored.
pub(crate) fn deserialize_recorded_intents<'de, D>(
    deserializer: D,
) -> Result<Vec<McpSessionAttachmentIntent>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let rows = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            if let Ok(intent) = serde_json::from_value::<McpSessionAttachmentIntent>(row.clone()) {
                return Some(intent);
            }
            let server_name = row.get("descriptor")?.get("server_name")?.as_str()?;
            let disposition = serde_json::from_value(row.get("disposition")?.clone()).ok()?;
            let revision = row.get("revision")?.as_u64()?;
            Some(McpSessionAttachmentIntent {
                descriptor: McpAttachmentDescriptorIdentity {
                    server_name: server_name.to_string(),
                    target: McpAttachmentTarget::Predecessor,
                    oauth_account: None,
                    oauth_account_selection: None,
                    connector_account: None,
                },
                disposition,
                revision,
            })
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn descriptor(name: &str, url: &str) -> McpAttachmentDescriptorIdentity {
        McpAttachmentDescriptorIdentity {
            server_name: name.to_string(),
            target: McpAttachmentTarget::Http {
                location: url.to_string(),
                withheld: false,
                binding: None,
            },
            oauth_account: Some("acct-1".to_string()),
            oauth_account_selection: None,
            connector_account: None,
        }
    }

    fn apply(
        intents: &mut Vec<McpSessionAttachmentIntent>,
        last_revision: &mut u64,
        mutation: McpAttachmentIntentMutation,
    ) -> Result<McpAttachmentIntentRecorded, McpAttachmentIntentError> {
        apply_mcp_attachment_intent_mutation(intents, last_revision, mutation)
    }

    fn request(name: &str, url: &str) -> McpAttachmentIntentMutation {
        McpAttachmentIntentMutation::Request {
            descriptor: descriptor(name, url),
        }
    }

    fn suppress(name: &str) -> McpAttachmentIntentMutation {
        McpAttachmentIntentMutation::SuppressByLogout {
            server_name: name.to_string(),
        }
    }

    fn reinstate(name: &str, expected_revision: u64) -> McpAttachmentIntentMutation {
        McpAttachmentIntentMutation::Reinstate {
            server_name: name.to_string(),
            expected_revision,
        }
    }

    #[test]
    fn request_suppress_reinstate_and_withdraw_are_revision_fenced() {
        let mut intents = Vec::new();
        let mut last_revision = 0;
        let recorded = apply(
            &mut intents,
            &mut last_revision,
            request("docs", "https://a.example/mcp"),
        )
        .unwrap();
        const B: u64 = MCP_ATTACHMENT_REVISION_BASE;
        assert_eq!(recorded.revision, Some(B + 1));
        let suppressed = apply(&mut intents, &mut last_revision, suppress("docs")).unwrap();
        assert_eq!(suppressed.revision, Some(B + 2));
        assert_eq!(
            intents[0].disposition,
            McpAttachmentIntentDisposition::SuppressedByLogout
        );
        // A login callback that observed the request before the sign-out is
        // stale and cannot revive it.
        assert!(matches!(
            apply(&mut intents, &mut last_revision, reinstate("docs", B + 1)),
            Err(McpAttachmentIntentError::StaleRevision { .. })
        ));
        let reinstated = apply(&mut intents, &mut last_revision, reinstate("docs", B + 2)).unwrap();
        assert_eq!(reinstated.revision, Some(B + 3));
        assert_eq!(
            intents[0].disposition,
            McpAttachmentIntentDisposition::Requested
        );
        let withdrawn = apply(
            &mut intents,
            &mut last_revision,
            McpAttachmentIntentMutation::Withdraw {
                server_name: "docs".to_string(),
            },
        )
        .unwrap();
        assert!(withdrawn.changed);
        assert!(intents.is_empty());

        // Removal does not reset the counter: a callback held from before the
        // withdrawal (revision 2) cannot match the same name's next entry,
        // even after it is re-requested and suppressed again.
        apply(
            &mut intents,
            &mut last_revision,
            request("docs", "https://b.example/mcp"),
        )
        .unwrap();
        let resuppressed = apply(&mut intents, &mut last_revision, suppress("docs")).unwrap();
        let before = intents.clone();
        for stale in [B + 1, B + 2, B + 3] {
            assert!(matches!(
                apply(&mut intents, &mut last_revision, reinstate("docs", stale)),
                Err(McpAttachmentIntentError::StaleRevision { .. })
            ));
        }
        assert_eq!(intents, before, "a stale callback changes nothing");
        // The fresh callback for the new entry still works.
        let fresh = apply(
            &mut intents,
            &mut last_revision,
            reinstate("docs", resuppressed.revision.unwrap()),
        )
        .unwrap();
        assert!(fresh.changed);
        assert_eq!(
            intents[0].disposition,
            McpAttachmentIntentDisposition::Requested
        );
    }

    #[test]
    fn an_exhausted_revision_counter_fails_closed() {
        let mut intents = Vec::new();
        let mut last_revision = u64::MAX - 1;
        let recorded = apply(
            &mut intents,
            &mut last_revision,
            request("docs", "https://a.example/mcp"),
        )
        .unwrap();
        assert_eq!(recorded.revision, Some(u64::MAX));
        let before = intents.clone();
        assert_eq!(
            apply(&mut intents, &mut last_revision, suppress("docs")),
            Err(McpAttachmentIntentError::RevisionsExhausted)
        );
        assert_eq!(intents, before);
        assert_eq!(last_revision, u64::MAX);
    }

    #[test]
    fn descriptor_identity_omits_secrets_and_tracks_account() {
        let config: McpServerConfig = serde_json::from_value(serde_json::json!({
            "name": "docs",
            "url": "https://a.example/mcp",
            "headers": { "Authorization": "Bearer secret" },
            "oauth_account": "acct-1"
        }))
        .unwrap();
        let identity = McpAttachmentDescriptorIdentity::of(&config, None);
        let encoded = serde_json::to_string(&identity).unwrap();
        assert!(!encoded.contains("secret"));
        assert!(!format!("{identity:?}").contains("secret"));
        assert_eq!(identity, descriptor("docs", "https://a.example/mcp"));
        let mut other = identity.clone();
        other.oauth_account = Some("acct-2".to_string());
        assert!(!identity.same_account(&other));

        // Credential material in the URL is neither persisted nor printed,
        // not even as a plain digest: it is bound only by a MAC under the
        // realm's identity key, which tells a changed URL apart. Without the
        // key the identity proves nothing about the target.
        let key = crate::auth::McpAttachmentIdentityKey::generate();
        let with_url = |url: &str, key: Option<&crate::auth::McpAttachmentIdentityKey>| {
            McpAttachmentDescriptorIdentity::of(
                &serde_json::from_value(serde_json::json!({ "name": "docs", "url": url })).unwrap(),
                key,
            )
        };
        let url = "https://user-mark:pass-mark@a.example/mcp?token=query-mark#frag-mark";
        let identity = with_url(url, Some(&key));
        let encoded = serde_json::to_string(&identity).unwrap();
        let printed = format!("{identity:?}");
        for marker in [
            "user-mark",
            "pass-mark",
            "query-mark",
            "frag-mark",
            "\"sha256:",
        ] {
            assert!(!encoded.contains(marker), "{marker} serialized: {encoded}");
            assert!(!printed.contains(marker), "{marker} printed: {printed}");
        }
        assert!(encoded.contains("a.example/mcp") && encoded.contains("hmac-sha256:"));
        assert!(identity.target.is_exact());
        assert_eq!(identity, with_url(url, Some(&key)));
        assert_ne!(
            identity,
            with_url(
                "https://user-mark:pass-mark@a.example/mcp?token=other#frag-mark",
                Some(&key)
            ),
            "a changed query is a different target"
        );
        assert_ne!(
            identity,
            with_url(
                url,
                Some(&crate::auth::McpAttachmentIdentityKey::generate())
            ),
            "a binding is only comparable under its own key"
        );
        assert!(!with_url(url, None).target.is_exact());
        assert!(with_url("https://a.example/mcp", None).target.is_exact());
    }

    /// A stdio target records the configured working directory in the
    /// clear, like its command, so a changed directory is a changed target
    /// (restore refuses it as rebound). A record written before the field
    /// existed reads as a target without one, and a target without one
    /// serializes as before. (Fails-old: the directory was not part of the
    /// target, so a moved installation restored as unchanged.)
    #[test]
    fn descriptor_identity_binds_the_stdio_working_directory() {
        let at = |cwd: Option<&str>| {
            let mut config = McpServerConfig::stdio(
                "github",
                "/opt/github/bin/github-mcp-server",
                vec![],
                std::collections::HashMap::new(),
            );
            if let (Some(cwd), McpTransportConfig::Stdio(stdio)) = (cwd, &mut config.transport) {
                stdio.cwd = Some(cwd.to_owned());
            }
            McpAttachmentDescriptorIdentity::of(&config, None)
        };
        let none = at(None);
        let first = at(Some("/opt/github"));
        assert!(first.target.is_exact());
        assert_eq!(first, at(Some("/opt/github")));
        assert_ne!(
            first.target,
            at(Some("/srv/moved")).target,
            "a moved directory"
        );
        assert_ne!(first.target, none.target, "a directory set or removed");

        let recorded = serde_json::to_value(&first.target).unwrap();
        assert_eq!(recorded["cwd"], "/opt/github");
        let earlier = serde_json::json!({
            "kind": "stdio",
            "command": "/opt/github/bin/github-mcp-server",
            "withheld": false
        });
        assert_eq!(serde_json::to_value(&none.target).unwrap(), earlier);
        assert_eq!(
            serde_json::from_value::<McpAttachmentTarget>(earlier).unwrap(),
            none.target
        );
    }

    #[test]
    fn descriptor_identity_binds_stdio_arguments_and_tracks_connector_context() {
        let stdio = |args: &[&str]| -> McpServerConfig {
            serde_json::from_value(serde_json::json!({
                "name": "local",
                "command": "python",
                "args": args,
                "env": { "TOKEN": "env-secret" }
            }))
            .unwrap()
        };
        let key = crate::auth::McpAttachmentIdentityKey::generate();
        let of = |args: &[&str]| McpAttachmentDescriptorIdentity::of(&stdio(args), Some(&key));
        // Arguments may carry secrets: none is recorded in the clear or as a
        // plain digest, yet a changed script is a different target.
        let before = of(&["/old/server.py", "--api-key", "arg-secret"]);
        let encoded = serde_json::to_string(&before).unwrap();
        let printed = format!("{before:?}");
        for marker in ["/old/server.py", "arg-secret", "env-secret", "\"sha256:"] {
            assert!(!encoded.contains(marker), "{marker} serialized: {encoded}");
            assert!(!printed.contains(marker), "{marker} printed: {printed}");
        }
        assert!(encoded.contains("TOKEN"), "env names stay visible");
        assert!(before.target.is_exact());
        assert_eq!(before, of(&["/old/server.py", "--api-key", "arg-secret"]));
        assert_ne!(
            before,
            of(&["/different/server.py", "--api-key", "arg-secret"]),
            "a changed script is a different target"
        );
        assert_ne!(
            of(&["a", "b"]),
            of(&["ab"]),
            "argument boundaries are part of the binding"
        );
        assert!(
            !McpAttachmentDescriptorIdentity::of(&stdio(&["/old/server.py"]), None)
                .target
                .is_exact(),
            "without the key, arguments cannot be bound"
        );
        assert!(
            McpAttachmentDescriptorIdentity::of(&stdio(&[]), None)
                .target
                .is_exact(),
            "a command without arguments is exact"
        );

        let connector = |resource: &str| -> McpServerConfig {
            serde_json::from_value(serde_json::json!({
                "name": "mail",
                "command": "mail-mcp",
                "connector": {
                    "realm_id": "realm",
                    "slot_id": "slot",
                    "issuer": "https://issuer.example",
                    "client": "client",
                    "resource": resource,
                    "strategy_id": "strategy",
                    "account": "subject",
                    "operations": {}
                }
            }))
            .unwrap()
        };
        let before = McpAttachmentDescriptorIdentity::of(&connector("https://mail.example"), None);
        let after = McpAttachmentDescriptorIdentity::of(&connector("https://other.example"), None);
        assert!(
            !before.same_account(&after),
            "a changed connector credential context is a different account binding"
        );
        assert!(before.same_account(&McpAttachmentDescriptorIdentity::of(
            &connector("https://mail.example"),
            None
        )));
    }
}
