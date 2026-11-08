//! Realm custody of MCP stdio servers' service secrets.
//!
//! A stdio server may declare environment variables whose values are
//! secrets ([`crate::mcp_config::McpStdioConfig::secret_env`]). Only the
//! names are configuration. The realm whose own configuration document
//! defines the server keeps each value in its credential store under one
//! nonsecret reference ([`McpServiceSecretRef`]): that defining realm, the
//! server, the environment slot and the server's canonical launch meaning
//! ([`McpServerConfig::stdio_launch_meaning`]). A changed definition is a
//! different reference, so a stored value is only ever read for the exact
//! definition it was stored for; an identical definition in another realm (a
//! child realm repeating its parent's server, or an unrelated realm) has its
//! own reference and never reads this one.
//!
//! The value is server configuration, not a sign-in credential: it lives
//! under its own key kind and mode ([`TokenKey::mcp_service_secret`],
//! [`PersistedAuthMode::McpServiceSecret`]) with no lease or lifecycle. A
//! launch reads it unattended, never from the host environment; only the
//! defining realm's owner writes or removes it. A read is a snapshot: a
//! rotation or removal after it does not change a launch that already read
//! the value.
//!
//! Every write is named: it draws a fresh write identity
//! ([`McpServiceSecretWriteId`], 128 bits of OS entropy, never derived from
//! the value) and stores it in the same row, beside the reference. The
//! write's [`McpServiceSecretWitness`] (the reference and that identity) is
//! what the write returns, what a status observation reports as the slot's
//! current write ([`observe_mcp_service_secret_witness`]) and what a launch
//! records for each slot it injected ([`McpServiceSecretValues::launch`]):
//! a value and its write are always read from the same row, so the witness
//! names exactly the value a launch injected. A host keeps a witness with
//! whatever it binds to that credential and later asks the store whether it
//! is still current ([`recheck_mcp_service_secret`]), without reading,
//! hashing or numbering the value. A row stored before writes were named
//! carries no identity: it is read and launched as before, observed with no
//! witness, and every witness rechecks against it as rotated.

use std::collections::BTreeMap;

use super::token_store::{
    McpServiceSecretKey, PersistedAuthMode, PersistedTokens, TokenKey, TokenStore,
    TokenStoreAccess, TokenStoreError, TokenStoreObservation,
};
use crate::connection::RealmId;
use crate::mcp_config::{McpSecretEnvDeclarationError, McpServerConfig, McpTransportConfig};
use crate::realm_admission::AdmissionGuard;

/// The metadata field binding a stored value to its exact reference.
const REFERENCE_FIELD: &str = "mcp_service_secret";

/// The metadata field naming the write that stored the value, beside the
/// reference record. A row without it was stored before writes were named.
const WRITE_FIELD: &str = "mcp_service_secret_write";

/// The one nonsecret reference of a stored MCP service secret: the realm
/// whose own configuration defines the server, the server name, the
/// environment slot and the server's canonical launch meaning. It is
/// configuration equality, not the identity of an executable, an external
/// service or a verified account.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct McpServiceSecretRef {
    server: String,
    key: McpServiceSecretKey,
}

impl std::fmt::Debug for McpServiceSecretRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServiceSecretRef")
            .field("realm", self.key.realm())
            .field("server", &self.server)
            .field("env", &self.key.env())
            .field("meaning", &self.key.meaning())
            .finish()
    }
}

impl McpServiceSecretRef {
    /// The reference of slot `env` of `server` as `realm`'s own
    /// configuration defines it. Refused for a server that is not stdio, an
    /// invalid declaration ([`McpServerConfig::validate_secret_env`]) or a
    /// slot `server` does not declare.
    pub fn for_definition(
        realm: RealmId,
        server: &McpServerConfig,
        env: &str,
    ) -> Result<Self, McpSecretEnvDeclarationError> {
        let refused = |reason| McpSecretEnvDeclarationError {
            server: server.name.clone(),
            reason,
        };
        let McpTransportConfig::Stdio(stdio) = &server.transport else {
            return Err(refused("only a stdio server declares service secrets"));
        };
        server.validate_secret_env()?;
        if !stdio.secret_env.contains(env) {
            return Err(refused("the slot is not declared in secret_env"));
        }
        let meaning = server
            .stdio_launch_meaning()
            .ok_or_else(|| refused("only a stdio server declares service secrets"))?;
        let key = McpServiceSecretKey::new(realm, env, &meaning)
            .ok_or_else(|| refused("the slot is not an environment variable name"))?;
        Ok(Self {
            server: server.name.clone(),
            key,
        })
    }

    /// The realm whose own configuration defines the server.
    pub fn realm(&self) -> &RealmId {
        self.key.realm()
    }

    pub fn server(&self) -> &str {
        &self.server
    }

    /// The environment variable the value is launched in.
    pub fn env(&self) -> &str {
        self.key.env()
    }

    /// The definition's canonical launch meaning (lowercase hex SHA-256).
    pub fn meaning(&self) -> &str {
        self.key.meaning()
    }

    pub(crate) fn key(&self) -> &McpServiceSecretKey {
        &self.key
    }

    /// The record binding a stored value to exactly this reference.
    fn record(&self) -> serde_json::Value {
        serde_json::json!({
            "server": self.server,
            "env": self.key.env(),
            "meaning": self.key.meaning(),
        })
    }

    /// The metadata of a row `write` stores for this reference: the
    /// reference record and the write's identity, beside it.
    fn metadata(&self, write: McpServiceSecretWriteId) -> serde_json::Value {
        serde_json::json!({
            REFERENCE_FIELD: self.record(),
            WRITE_FIELD: write.to_string(),
        })
    }

    /// The write `tokens` records, when `tokens` is a service secret stored
    /// for exactly this reference: its metadata is exactly the reference
    /// record, with nothing beside it (stored before writes were named) or
    /// with exactly one well-formed write identity. Any other row is not
    /// this reference's.
    fn recorded_write(&self, tokens: &PersistedTokens) -> Option<RecordedWrite> {
        if tokens.auth_mode != PersistedAuthMode::McpServiceSecret {
            return None;
        }
        let serde_json::Value::Object(fields) = &tokens.metadata else {
            return None;
        };
        if fields.get(REFERENCE_FIELD) != Some(&self.record()) {
            return None;
        }
        match (fields.len(), fields.get(WRITE_FIELD)) {
            (1, None) => Some(RecordedWrite::Unnamed),
            (2, Some(serde_json::Value::String(write))) => {
                McpServiceSecretWriteId::parse(write).map(RecordedWrite::Named)
            }
            _ => None,
        }
    }

    /// The witness of the write that stored `tokens` for exactly this
    /// reference, read from the row's metadata (never its value). `None` for
    /// a row that is not this reference's, or that was stored before writes
    /// were named.
    pub fn witness_of(&self, tokens: &PersistedTokens) -> Option<McpServiceSecretWitness> {
        self.recorded_write(tokens)
            .and_then(RecordedWrite::named)
            .map(|write| self.witness(write))
    }

    fn witness(&self, write: McpServiceSecretWriteId) -> McpServiceSecretWitness {
        McpServiceSecretWitness {
            reference: self.clone(),
            write,
        }
    }
}

/// The write a stored row records for its reference.
#[derive(Clone, Copy)]
enum RecordedWrite {
    /// Stored before writes were named: the row carries no write identity.
    Unnamed,
    Named(McpServiceSecretWriteId),
}

impl RecordedWrite {
    fn named(self) -> Option<McpServiceSecretWriteId> {
        match self {
            Self::Unnamed => None,
            Self::Named(write) => Some(write),
        }
    }
}

/// The identity of one write of a service secret: 128 bits of OS entropy
/// the write draws and stores in its row beside the reference, never derived
/// from the value. Nonsecret and opaque; shown as 32 lowercase hexadecimal
/// characters. Writing the same value again is another write, with another
/// identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct McpServiceSecretWriteId(u128);

impl McpServiceSecretWriteId {
    /// A fresh identity, drawn from OS entropy.
    fn mint() -> Result<Self, crate::secret_entropy::SecretEntropyError> {
        let mut bytes = [0_u8; 16];
        crate::secret_entropy::fill_secret_entropy(&mut bytes)?;
        Ok(Self(u128::from_be_bytes(bytes)))
    }

    /// The identity `text` renders: exactly 32 lowercase hexadecimal
    /// characters.
    fn parse(text: &str) -> Option<Self> {
        let rendered = text.len() == 32
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !rendered {
            return None;
        }
        u128::from_str_radix(text, 16).ok().map(Self)
    }
}

impl std::fmt::Display for McpServiceSecretWriteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl std::fmt::Debug for McpServiceSecretWriteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "McpServiceSecretWriteId({self})")
    }
}

/// One write of exactly one reference: the reference (the defining realm,
/// the server, the slot and the server's launch meaning) and the identity of
/// the write that stored the value.
///
/// It is what a write returns, what a status observation reports as a
/// slot's current write and what a launch records for each slot it injected
/// ([`McpServiceSecretLaunch`]). It names the write, never the value, and is
/// not derived from it. It serializes (and deserializes, checked) as
/// `{realm, server, env, meaning, write}`, so a host can keep it with
/// whatever it binds to that credential and recheck it later, in this or
/// another process ([`recheck_mcp_service_secret`]). Holding one grants
/// nothing.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct McpServiceSecretWitness {
    reference: McpServiceSecretRef,
    write: McpServiceSecretWriteId,
}

impl McpServiceSecretWitness {
    /// The reference the write stored a value for.
    pub fn reference(&self) -> &McpServiceSecretRef {
        &self.reference
    }

    /// The identity of the write.
    pub fn write(&self) -> McpServiceSecretWriteId {
        self.write
    }
}

impl std::fmt::Debug for McpServiceSecretWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServiceSecretWitness")
            .field("reference", &self.reference)
            .field("write", &self.write)
            .finish()
    }
}

/// The serialized form of a [`McpServiceSecretWitness`].
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WitnessWire {
    realm: RealmId,
    server: String,
    env: String,
    meaning: String,
    write: String,
}

impl serde::Serialize for McpServiceSecretWitness {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let wire = WitnessWire {
            realm: self.reference.realm().clone(),
            server: self.reference.server().to_owned(),
            env: self.reference.env().to_owned(),
            meaning: self.reference.meaning().to_owned(),
            write: self.write.to_string(),
        };
        serde::Serialize::serialize(&wire, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for McpServiceSecretWitness {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = <WitnessWire as serde::Deserialize<'de>>::deserialize(deserializer)?;
        let key =
            McpServiceSecretKey::new(wire.realm, &wire.env, &wire.meaning).ok_or_else(|| {
                serde::de::Error::custom(
                    "MCP service secret witness has an invalid slot or meaning",
                )
            })?;
        let write = McpServiceSecretWriteId::parse(&wire.write).ok_or_else(|| {
            serde::de::Error::custom("MCP service secret witness has an invalid write identity")
        })?;
        Ok(Self {
            reference: McpServiceSecretRef {
                server: wire.server,
                key,
            },
            write,
        })
    }
}

/// One service-secret value on its way to its slot. It has no `Debug`,
/// `Display`, `Clone` or serialization, and nothing returns it, so it never
/// reaches a log, an error, a wire or a record; the write that stores it
/// consumes it.
pub struct McpServiceSecretValue(String);

impl McpServiceSecretValue {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// The value, for the write that stores it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// The service-secret values resolved for one launch, by environment
/// variable name, each with the witness of the write it was read from when
/// it was read from the defining realm's custody
/// ([`load_mcp_service_secret_into`]). `Debug` shows only the names.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpServiceSecretValues(BTreeMap<String, LaunchValue>);

/// One slot's value for a launch, and the write it was read from, if any.
#[derive(Clone, PartialEq, Eq)]
struct LaunchValue {
    value: String,
    write: Option<McpServiceSecretWitness>,
}

impl std::fmt::Debug for McpServiceSecretValues {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map()
            .entries(self.0.keys().map(|name| (name, crate::redact::REDACTED)))
            .finish()
    }
}

impl McpServiceSecretValues {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the value launched in `env`. It names no write: a launch with it
    /// records the slot without a witness.
    pub fn insert(&mut self, env: impl Into<String>, value: impl Into<String>) {
        self.0.insert(
            env.into(),
            LaunchValue {
                value: value.into(),
                write: None,
            },
        );
    }

    /// The environment variable names, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// What a launch with these values injects, without a value: each slot
    /// with the witness of the write its value was read from.
    pub fn launch(&self) -> McpServiceSecretLaunch {
        McpServiceSecretLaunch(
            self.0
                .iter()
                .map(|(name, slot)| (name.clone(), slot.write.clone()))
                .collect(),
        )
    }

    /// The `(name, value)` pairs, consuming the values.
    pub fn into_env(self) -> impl Iterator<Item = (String, String)> {
        self.0.into_iter().map(|(name, slot)| (name, slot.value))
    }
}

/// What one stdio launch injected for its server's declared service
/// secrets, without a value: for each slot (environment variable name), the
/// witness of the write whose value the launch read, or `None` for a value
/// that names no write (stored before writes were named, or supplied by a
/// host's own credential source).
///
/// It is fixed for the life of the launched connection: a running server
/// keeps the environment it was launched with, so every call over that
/// connection is served by these writes' values, and a later write or
/// removal reaches only a later launch. [`recheck_mcp_service_secret`] tells
/// whether a recorded write is still its slot's current value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServiceSecretLaunch(BTreeMap<String, Option<McpServiceSecretWitness>>);

impl McpServiceSecretLaunch {
    /// Each injected slot, in name order, with the witness of the write its
    /// value was read from.
    pub fn slots(&self) -> impl Iterator<Item = (&str, Option<&McpServiceSecretWitness>)> {
        self.0
            .iter()
            .map(|(name, write)| (name.as_str(), write.as_ref()))
    }

    /// The witness of the write slot `env` was launched with: `None` when
    /// the launch injected no such slot or its value names no write.
    pub fn witness(&self, env: &str) -> Option<&McpServiceSecretWitness> {
        self.0.get(env).and_then(Option::as_ref)
    }
}

/// Why a stdio server's declared service secrets were not resolved for its
/// launch. Each is local and typed, names no value, and comes before the
/// server's process exists; no value is ever taken from the host
/// environment instead.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpServiceSecretRefusal {
    /// The connection's credential source holds no service-secret custody.
    #[error("no service-secret custody serves this connection")]
    NoCustody,
    /// No realm's own configuration defines the server (an `mcp.toml`, host
    /// or live-added server has no defining realm).
    #[error("no realm's own configuration defines this server")]
    NoOwner,
    /// The launched definition is not the defining realm's current
    /// definition of the server.
    #[error("the launched definition is not its defining realm's current definition")]
    DefinitionChanged,
    /// The defining realm stores no value for the slot of the current
    /// definition.
    #[error("no secret is stored for slot '{env}' of the current definition")]
    NotStored { env: String },
    /// The configuration or the credential store could not be read.
    #[error("the service-secret custody is unavailable: {reason}")]
    Unavailable { reason: String },
}

/// What a status observation saw for one reference. It never shows the
/// value; the write that stored it is reported beside it
/// ([`observe_mcp_service_secret_witness`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServiceSecretPresence {
    /// A value is stored for exactly this reference.
    Stored,
    /// The store answered and holds no value for this reference.
    NotStored,
    /// The store did not answer without operating-system interaction, so
    /// nothing is known.
    NotObserved,
}

/// Whether a write is still the value stored for its reference, as a
/// recheck answers it ([`recheck_mcp_service_secret`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServiceSecretRecheck {
    /// The write's value is still the one stored for its reference: a
    /// launch of the definition now injects exactly that value.
    Current,
    /// Another write stored the reference's value since: a rotation or a
    /// replacement, including a value stored by a version that names no
    /// write.
    Rotated,
    /// No value is stored for the reference.
    Removed,
    /// The server is no longer defined as the write's reference names it:
    /// its defining realm changed its definition, another realm's definition
    /// now serves its launches, or it was uninstalled. Only the realm
    /// custody that resolves definitions answers it (the facade's
    /// `RealmMcpServiceSecrets::recheck`, before it reads the store); the
    /// store read never does.
    DefinitionChanged,
}

/// The value stored for exactly `reference`, with the write that stored it:
/// one unattended read of one row. Only a row stored as an MCP service
/// secret for exactly this reference, with a non-empty value, counts.
async fn load_stored(
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
) -> Result<Option<(String, RecordedWrite)>, TokenStoreError> {
    let Some(tokens) = store
        .load_for(
            &TokenKey::mcp_service_secret(reference),
            TokenStoreAccess::Unattended,
        )
        .await?
    else {
        return Ok(None);
    };
    let Some(write) = reference.recorded_write(&tokens) else {
        return Ok(None);
    };
    Ok(tokens
        .primary_secret
        .filter(|value| !value.is_empty())
        .map(|value| (value, write)))
}

/// Read the value stored for exactly `reference`, for a launch: an
/// unattended read that raises no operating-system UI. Only a row stored as
/// an MCP service secret for exactly this reference, with a non-empty value,
/// is returned; anything else is `None`.
pub async fn load_mcp_service_secret(
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
) -> Result<Option<String>, TokenStoreError> {
    Ok(load_stored(store, reference).await?.map(|(value, _)| value))
}

/// Read the value stored for exactly `reference` for a launch, exactly as
/// [`load_mcp_service_secret`] does, and set it in `values` for the
/// reference's slot with the witness of the write that stored it: both come
/// from the same read of the same row, so the witness always names the
/// value beside it. Returns whether a value is stored; when none is,
/// `values` is unchanged.
pub async fn load_mcp_service_secret_into(
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
    values: &mut McpServiceSecretValues,
) -> Result<bool, TokenStoreError> {
    let Some((value, write)) = load_stored(store, reference).await? else {
        return Ok(false);
    };
    values.0.insert(
        reference.env().to_owned(),
        LaunchValue {
            value,
            write: write.named().map(|write| reference.witness(write)),
        },
    );
    Ok(true)
}

/// Store (or replace) the value of `reference`: an explicit operator write.
/// `value` is not empty (the caller refuses an empty value). The write draws
/// a fresh identity from OS entropy, never from the value, and stores it in
/// the same row beside the reference ([`McpServiceSecretRef::witness_of`]
/// reads it back); when no identity can be drawn, nothing is written
/// ([`TokenStoreError::Unavailable`]). Returns the exact row the store
/// persisted.
///
/// `guard` is the update's admission guard, which its credential
/// coordinator acquired for the reference's realm: a persisted store refuses
/// a guard that does not cover it ([`TokenStoreError::AdmissionFence`]) with
/// nothing written. Its one caller is the realm custody's update.
pub async fn save_mcp_service_secret(
    guard: &AdmissionGuard,
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
    value: &str,
) -> Result<PersistedTokens, TokenStoreError> {
    let write = McpServiceSecretWriteId::mint()
        .map_err(|error| TokenStoreError::Unavailable(error.to_string()))?;
    let mut tokens = PersistedTokens::api_key(value);
    tokens.auth_mode = PersistedAuthMode::McpServiceSecret;
    tokens.metadata = reference.metadata(write);
    store
        .save_for(
            guard,
            &TokenKey::mcp_service_secret(reference),
            &tokens,
            TokenStoreAccess::Interactive,
        )
        .await?;
    Ok(tokens)
}

/// Remove the value of `reference`, if any: an explicit operator write,
/// under the update's admission guard as [`save_mcp_service_secret`] is.
pub async fn clear_mcp_service_secret(
    guard: &AdmissionGuard,
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
) -> Result<(), TokenStoreError> {
    store
        .clear_for(
            guard,
            &TokenKey::mcp_service_secret(reference),
            TokenStoreAccess::Interactive,
        )
        .await
}

/// Whether a value is stored for `reference`, observed as a status read: no
/// operating-system UI and no keychain read.
pub async fn observe_mcp_service_secret(
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
) -> Result<McpServiceSecretPresence, TokenStoreError> {
    Ok(observe_mcp_service_secret_witness(store, reference)
        .await?
        .0)
}

/// [`observe_mcp_service_secret`], with the witness of the slot's current
/// write: present only for a value stored for exactly `reference` by a
/// write that named itself. Reads the row's metadata, never returns the
/// value.
pub async fn observe_mcp_service_secret_witness(
    store: &dyn TokenStore,
    reference: &McpServiceSecretRef,
) -> Result<(McpServiceSecretPresence, Option<McpServiceSecretWitness>), TokenStoreError> {
    Ok(
        match store
            .observe_status(&TokenKey::mcp_service_secret(reference))
            .await?
        {
            TokenStoreObservation::Present(tokens) => match reference.recorded_write(&tokens) {
                Some(write) => (
                    McpServiceSecretPresence::Stored,
                    write.named().map(|write| reference.witness(write)),
                ),
                None => (McpServiceSecretPresence::NotStored, None),
            },
            TokenStoreObservation::Missing => (McpServiceSecretPresence::NotStored, None),
            _ => (McpServiceSecretPresence::NotObserved, None),
        },
    )
}

/// Whether `witness`'s write is still the value stored for its reference,
/// read from the store as a launch reads it (unattended, no operating-system
/// UI, nothing cached) and returning no value: [`Current`] when the stored
/// row is that write's, [`Rotated`] when another write's (or one that names
/// no write), [`Removed`] when no value is stored. It never answers
/// [`DefinitionChanged`]: whether the reference is still its server's
/// current definition is the realm custody's to answer first.
///
/// [`Current`]: McpServiceSecretRecheck::Current
/// [`Rotated`]: McpServiceSecretRecheck::Rotated
/// [`Removed`]: McpServiceSecretRecheck::Removed
/// [`DefinitionChanged`]: McpServiceSecretRecheck::DefinitionChanged
pub async fn recheck_mcp_service_secret(
    store: &dyn TokenStore,
    witness: &McpServiceSecretWitness,
) -> Result<McpServiceSecretRecheck, TokenStoreError> {
    Ok(match load_stored(store, witness.reference()).await? {
        None => McpServiceSecretRecheck::Removed,
        Some((_, RecordedWrite::Named(write))) if write == witness.write() => {
            McpServiceSecretRecheck::Current
        }
        Some(_) => McpServiceSecretRecheck::Rotated,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn server(name: &str, secret_env: &[&str]) -> McpServerConfig {
        let mut server = McpServerConfig::stdio(
            name,
            "search-server",
            vec!["--stdio".to_owned()],
            HashMap::from([("LOG_LEVEL".to_owned(), "info".to_owned())]),
        );
        if let McpTransportConfig::Stdio(stdio) = &mut server.transport {
            stdio.secret_env = secret_env.iter().map(|name| (*name).to_owned()).collect();
        }
        server
    }

    fn realm(id: &str) -> RealmId {
        RealmId::parse(id).expect("realm id")
    }

    #[test]
    fn a_reference_names_the_defining_realm_slot_and_exact_definition() {
        let defined = server("search", &["SEARCH_API_KEY"]);
        let reference =
            McpServiceSecretRef::for_definition(realm("team"), &defined, "SEARCH_API_KEY")
                .expect("declared slot");
        assert_eq!(reference.realm(), &realm("team"));
        assert_eq!(reference.server(), "search");
        assert_eq!(reference.env(), "SEARCH_API_KEY");
        assert_eq!(
            Some(reference.meaning()),
            defined.stdio_launch_meaning().as_deref()
        );
        let key = TokenKey::mcp_service_secret(&reference);
        assert!(key.is_mcp_service_secret());
        assert!(key.credential_identity().is_none());
        assert_eq!(key.realm(), &realm("team"));
        assert_eq!(
            TokenKey::mcp_service_secret_from_stem(&realm("team"), &key.storage_stem()),
            Some(key.clone()),
            "the storage stem names the same key"
        );
        let wire = serde_json::to_value(&key).expect("serialize");
        assert_eq!(
            serde_json::from_value::<TokenKey>(wire).expect("deserialize"),
            key
        );

        // Another realm, another slot or any changed launch field is another
        // reference.
        let other_realm =
            McpServiceSecretRef::for_definition(realm("child"), &defined, "SEARCH_API_KEY")
                .expect("declared slot");
        assert_ne!(
            TokenKey::mcp_service_secret(&other_realm),
            TokenKey::mcp_service_secret(&reference)
        );
        let mut changed = defined.clone();
        if let McpTransportConfig::Stdio(stdio) = &mut changed.transport {
            stdio.args.push("--verbose".to_owned());
        }
        let changed_ref =
            McpServiceSecretRef::for_definition(realm("team"), &changed, "SEARCH_API_KEY")
                .expect("declared slot");
        assert_ne!(changed_ref.meaning(), reference.meaning());

        // An access ceiling is a separate check, not part of the meaning.
        let mut ceiling = defined.clone();
        ceiling.access = Some(crate::mcp_config::McpServerAccess::allowing(["search"]));
        assert_eq!(
            ceiling.stdio_launch_meaning(),
            defined.stdio_launch_meaning()
        );
    }

    #[test]
    fn undeclared_slots_and_invalid_declarations_have_no_reference() {
        let defined = server("search", &["SEARCH_API_KEY"]);
        assert!(McpServiceSecretRef::for_definition(realm("team"), &defined, "OTHER").is_err());
        for invalid in [
            server("search", &["9LIVES"]),
            server("search", &["HAS-DASH"]),
            server("search", &["MEERKAT_CONNECTOR_BINDING"]),
            server("search", &["log_level"]),
            server("search", &["TOKEN", "token"]),
        ] {
            let McpTransportConfig::Stdio(stdio) = &invalid.transport else {
                unreachable!("stdio fixture");
            };
            let env = stdio.secret_env.iter().next().cloned().unwrap_or_default();
            assert!(invalid.validate_secret_env().is_err(), "{invalid:?}");
            assert!(McpServiceSecretRef::for_definition(realm("team"), &invalid, &env).is_err());
        }
        let http =
            McpServerConfig::streamable_http("remote", "https://mcp.example", HashMap::new());
        assert!(http.validate_secret_env().is_ok());
        assert!(http.stdio_launch_meaning().is_none());
        assert!(McpServiceSecretRef::for_definition(realm("team"), &http, "TOKEN").is_err());
    }

    #[test]
    fn values_and_declarations_print_only_names() {
        let mut values = McpServiceSecretValues::new();
        values.insert("SEARCH_API_KEY", "sk-live-value");
        let rendered = format!("{values:?}");
        assert!(rendered.contains("SEARCH_API_KEY"), "{rendered}");
        assert!(!rendered.contains("sk-live-value"), "{rendered}");
        let declared = server("search", &["SEARCH_API_KEY"]);
        let rendered = format!("{declared:?}");
        assert!(rendered.contains("SEARCH_API_KEY"), "{rendered}");
        assert_eq!(
            values.names().collect::<BTreeSet<_>>(),
            BTreeSet::from(["SEARCH_API_KEY"])
        );
    }

    const SLOT: &str = "SEARCH_API_KEY";

    /// Rows by key, answering status reads: a store a test can inspect.
    #[derive(Default)]
    struct Rows(std::sync::Mutex<HashMap<TokenKey, PersistedTokens>>);

    impl Rows {
        fn row(&self, reference: &McpServiceSecretRef) -> Option<PersistedTokens> {
            self.0
                .lock()
                .unwrap()
                .get(&TokenKey::mcp_service_secret(reference))
                .cloned()
        }

        fn put(&self, reference: &McpServiceSecretRef, tokens: PersistedTokens) {
            self.0
                .lock()
                .unwrap()
                .insert(TokenKey::mcp_service_secret(reference), tokens);
        }
    }

    #[async_trait::async_trait]
    impl TokenStore for Rows {
        async fn load(&self, key: &TokenKey) -> Result<Option<PersistedTokens>, TokenStoreError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        async fn save(
            &self,
            _guard: &AdmissionGuard,
            key: &TokenKey,
            tokens: &PersistedTokens,
        ) -> Result<(), TokenStoreError> {
            self.0.lock().unwrap().insert(key.clone(), tokens.clone());
            Ok(())
        }

        async fn clear(
            &self,
            _guard: &AdmissionGuard,
            key: &TokenKey,
        ) -> Result<(), TokenStoreError> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }

        async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
            Ok(self.0.lock().unwrap().keys().cloned().collect())
        }

        fn backend_name(&self) -> &'static str {
            "rows"
        }

        async fn observe_status(
            &self,
            key: &TokenKey,
        ) -> Result<TokenStoreObservation, TokenStoreError> {
            Ok(match self.0.lock().unwrap().get(key) {
                Some(tokens) => TokenStoreObservation::Present(Box::new(tokens.clone())),
                None => TokenStoreObservation::Missing,
            })
        }
    }

    fn search_reference() -> McpServiceSecretRef {
        McpServiceSecretRef::for_definition(realm("team"), &server("search", &[SLOT]), SLOT)
            .expect("declared slot")
    }

    /// Store `value` for `reference` and return the write's witness. `Rows`
    /// has no fence, so it accepts the guard over none.
    async fn write(
        store: &Rows,
        reference: &McpServiceSecretRef,
        value: &str,
    ) -> McpServiceSecretWitness {
        let row = save_mcp_service_secret(&AdmissionGuard::none(), store, reference, value)
            .await
            .expect("stored");
        assert_eq!(
            store.row(reference).as_ref(),
            Some(&row),
            "the row persisted"
        );
        reference.witness_of(&row).expect("a named write")
    }

    /// What a launch reads for `reference` now.
    async fn launched(store: &Rows, reference: &McpServiceSecretRef) -> McpServiceSecretValues {
        let mut values = McpServiceSecretValues::new();
        assert!(
            load_mcp_service_secret_into(store, reference, &mut values)
                .await
                .expect("read")
        );
        values
    }

    #[tokio::test]
    async fn a_write_is_named_and_observed_launched_and_rechecked_by_that_name() {
        let store = Rows::default();
        let reference = search_reference();
        let witness = write(&store, &reference, "sk-first").await;
        assert_eq!(witness.reference(), &reference);

        // The status observation reports the same write as the current one.
        assert_eq!(
            observe_mcp_service_secret_witness(&store, &reference)
                .await
                .expect("observed"),
            (McpServiceSecretPresence::Stored, Some(witness.clone()))
        );
        assert_eq!(
            observe_mcp_service_secret(&store, &reference)
                .await
                .expect("observed"),
            McpServiceSecretPresence::Stored
        );

        // A launch reads the value and its write from the one row, and
        // records the write for the slot.
        let values = launched(&store, &reference).await;
        let launch = values.launch();
        assert_eq!(launch.witness(SLOT), Some(&witness));
        assert_eq!(
            launch.slots().collect::<Vec<_>>(),
            vec![(SLOT, Some(&witness))]
        );
        assert_eq!(
            values.into_env().collect::<Vec<_>>(),
            vec![(SLOT.to_owned(), "sk-first".to_owned())]
        );
        assert_eq!(
            load_mcp_service_secret(&store, &reference)
                .await
                .expect("read")
                .as_deref(),
            Some("sk-first")
        );
        assert_eq!(
            recheck_mcp_service_secret(&store, &witness)
                .await
                .expect("rechecked"),
            McpServiceSecretRecheck::Current
        );

        // A value inserted by a host names no write.
        let mut host = McpServiceSecretValues::new();
        host.insert(SLOT, "host-value");
        assert_eq!(
            host.launch().slots().collect::<Vec<_>>(),
            vec![(SLOT, None)]
        );
    }

    #[tokio::test]
    async fn a_rewrite_rotates_the_earlier_write_and_a_removal_removes_both() {
        let store = Rows::default();
        let reference = search_reference();
        let first = write(&store, &reference, "sk-first").await;
        let launched_first = launched(&store, &reference).await.launch();
        let second = write(&store, &reference, "sk-second").await;
        assert_ne!(first, second, "every write is named afresh");

        assert_eq!(
            recheck_mcp_service_secret(&store, &first).await.unwrap(),
            McpServiceSecretRecheck::Rotated
        );
        assert_eq!(
            recheck_mcp_service_secret(&store, &second).await.unwrap(),
            McpServiceSecretRecheck::Current
        );
        // A launch that read the first value keeps its record; the next
        // launch records the second write.
        assert_eq!(launched_first.witness(SLOT), Some(&first));
        assert_eq!(
            launched(&store, &reference).await.launch().witness(SLOT),
            Some(&second)
        );
        assert_eq!(
            observe_mcp_service_secret_witness(&store, &reference)
                .await
                .unwrap()
                .1,
            Some(second.clone())
        );

        clear_mcp_service_secret(&AdmissionGuard::none(), &store, &reference)
            .await
            .unwrap();
        for witness in [&first, &second] {
            assert_eq!(
                recheck_mcp_service_secret(&store, witness).await.unwrap(),
                McpServiceSecretRecheck::Removed
            );
        }
        assert_eq!(
            observe_mcp_service_secret_witness(&store, &reference)
                .await
                .unwrap(),
            (McpServiceSecretPresence::NotStored, None)
        );
        let mut values = McpServiceSecretValues::new();
        assert!(
            !load_mcp_service_secret_into(&store, &reference, &mut values)
                .await
                .unwrap()
        );
        assert!(values.is_empty());

        // A value stored for another definition is another reference: it
        // never answers for this one.
        let mut changed = server("search", &[SLOT]);
        if let McpTransportConfig::Stdio(stdio) = &mut changed.transport {
            stdio.args.push("--verbose".to_owned());
        }
        let other = McpServiceSecretRef::for_definition(realm("team"), &changed, SLOT).unwrap();
        let other_write = write(&store, &other, "sk-other").await;
        assert_eq!(
            recheck_mcp_service_secret(&store, &second).await.unwrap(),
            McpServiceSecretRecheck::Removed
        );
        assert_eq!(reference.witness_of(&store.row(&other).unwrap()), None);
        assert_eq!(
            recheck_mcp_service_secret(&store, &other_write)
                .await
                .unwrap(),
            McpServiceSecretRecheck::Current
        );
    }

    #[tokio::test]
    async fn a_value_stored_before_writes_were_named_launches_unnamed_and_rechecks_as_rotated() {
        let store = Rows::default();
        let reference = search_reference();
        let named = write(&store, &reference, "sk-named").await;

        // The exact row an earlier version stores: the reference record and
        // nothing beside it.
        let mut legacy = PersistedTokens::api_key("sk-legacy");
        legacy.auth_mode = PersistedAuthMode::McpServiceSecret;
        legacy.metadata = serde_json::json!({
            "mcp_service_secret": {
                "server": "search",
                "env": SLOT,
                "meaning": reference.meaning(),
            }
        });
        store.put(&reference, legacy.clone());

        // It still launches and observes as stored, with no write.
        assert_eq!(reference.witness_of(&legacy), None);
        assert_eq!(
            observe_mcp_service_secret_witness(&store, &reference)
                .await
                .unwrap(),
            (McpServiceSecretPresence::Stored, None)
        );
        let values = launched(&store, &reference).await;
        assert_eq!(
            values.launch().slots().collect::<Vec<_>>(),
            vec![(SLOT, None)]
        );
        assert_eq!(
            values.into_env().collect::<Vec<_>>(),
            vec![(SLOT.to_owned(), "sk-legacy".to_owned())]
        );
        // Any witness rechecks against it as rotated.
        assert_eq!(
            recheck_mcp_service_secret(&store, &named).await.unwrap(),
            McpServiceSecretRecheck::Rotated
        );

        // The next write names itself again.
        let renamed = write(&store, &reference, "sk-legacy").await;
        assert_eq!(
            recheck_mcp_service_secret(&store, &renamed).await.unwrap(),
            McpServiceSecretRecheck::Current
        );
        assert_eq!(
            recheck_mcp_service_secret(&store, &named).await.unwrap(),
            McpServiceSecretRecheck::Rotated
        );
    }

    #[tokio::test]
    async fn only_an_exact_record_with_at_most_a_well_formed_write_is_read() {
        let store = Rows::default();
        let reference = search_reference();
        let witness = write(&store, &reference, "sk-value").await;
        let stored = store.row(&reference).unwrap();
        let record = &stored.metadata[REFERENCE_FIELD];
        let write_id = witness.write().to_string();
        let with_metadata =
            |metadata: serde_json::Value| rewritten(&stored, |tokens| tokens.metadata = metadata);
        for (case, tokens) in [
            (
                "another field beside the write",
                with_metadata(serde_json::json!({
                    REFERENCE_FIELD: record,
                    WRITE_FIELD: write_id,
                    "other": true,
                })),
            ),
            (
                "an unknown field instead of the write",
                with_metadata(serde_json::json!({ REFERENCE_FIELD: record, "other": true })),
            ),
            (
                "an uppercase write",
                with_metadata(serde_json::json!({
                    REFERENCE_FIELD: record,
                    WRITE_FIELD: write_id.to_uppercase(),
                })),
            ),
            (
                "a short write",
                with_metadata(serde_json::json!({
                    REFERENCE_FIELD: record,
                    WRITE_FIELD: &write_id[1..],
                })),
            ),
            (
                "a write that is not a string",
                with_metadata(serde_json::json!({ REFERENCE_FIELD: record, WRITE_FIELD: 7 })),
            ),
            (
                "another mode",
                rewritten(&stored, |tokens| {
                    tokens.auth_mode = PersistedAuthMode::ApiKey;
                }),
            ),
            (
                "an empty value",
                rewritten(&stored, |tokens| {
                    tokens.primary_secret = Some(String::new());
                }),
            ),
        ] {
            store.put(&reference, tokens);
            assert_eq!(
                load_mcp_service_secret(&store, &reference).await.unwrap(),
                None,
                "{case}"
            );
            assert_eq!(
                recheck_mcp_service_secret(&store, &witness).await.unwrap(),
                McpServiceSecretRecheck::Removed,
                "{case}"
            );
        }
    }

    /// `row` with `change` applied.
    fn rewritten(
        row: &PersistedTokens,
        change: impl FnOnce(&mut PersistedTokens),
    ) -> PersistedTokens {
        let mut tokens = row.clone();
        change(&mut tokens);
        tokens
    }

    #[tokio::test]
    async fn a_witness_is_drawn_for_each_write_and_never_carries_or_derives_from_the_value() {
        let store = Rows::default();
        let reference = search_reference();
        const VALUE: &str = "sk-live-value-0123456789abcdef";
        // The same value written twice is two writes with unrelated names:
        // the name is drawn, not computed from the value.
        let first = write(&store, &reference, VALUE).await;
        let row = store.row(&reference).unwrap();
        let second = write(&store, &reference, VALUE).await;
        assert_ne!(first, second);
        assert_ne!(first.write(), second.write());
        for witness in [&first, &second] {
            let rendered = witness.write().to_string();
            assert_eq!(rendered.len(), 32, "{rendered}");
            assert!(
                rendered
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "{rendered}"
            );
        }

        // Nothing that names the write carries the value: not the row's
        // metadata, a witness, a launch record or the values' Debug.
        let values = launched(&store, &reference).await;
        let renderings = [
            serde_json::to_string(&row.metadata).unwrap(),
            format!("{first:?}"),
            format!("{}", first.write()),
            format!("{:?}", first.write()),
            serde_json::to_string(&first).unwrap(),
            format!("{:?}", values.launch()),
            format!("{values:?}"),
            format!("{row:?}"),
        ];
        for rendered in &renderings {
            assert!(!rendered.contains(VALUE), "{rendered}");
            assert!(!rendered.contains("sk-live"), "{rendered}");
        }

        // A witness keeps through serialization exactly, and a malformed one
        // is refused.
        let wire = serde_json::to_value(&second).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "realm": "team",
                "server": "search",
                "env": SLOT,
                "meaning": reference.meaning(),
                "write": second.write().to_string(),
            })
        );
        assert_eq!(
            serde_json::from_value::<McpServiceSecretWitness>(wire.clone()).unwrap(),
            second
        );
        for (field, value) in [
            ("write", serde_json::json!("not-a-write")),
            (
                "write",
                serde_json::json!(second.write().to_string().to_uppercase()),
            ),
            ("meaning", serde_json::json!("short")),
            ("env", serde_json::json!("HAS-DASH")),
            ("realm", serde_json::json!("not a realm")),
            ("extra", serde_json::json!(1)),
        ] {
            let mut malformed = wire.clone();
            malformed[field] = value;
            assert!(
                serde_json::from_value::<McpServiceSecretWitness>(malformed).is_err(),
                "{field}"
            );
        }
    }
}
