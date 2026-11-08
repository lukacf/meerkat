//! A realm's protected key for keyed MCP attachment identities.
//!
//! A session's recorded attachment request keeps no secret-bearing part of
//! the descriptor it accepted (URL credentials, command arguments), only an
//! HMAC-SHA-256 binding over them under this key. The key is host key
//! material, not a credential: it lives in the realm's token store under its
//! own key kind and mode ([`TokenKey::mcp_attachment_identity_key`],
//! [`PersistedAuthMode::McpAttachmentIdentityKey`]), outside session state,
//! with no lease or lifecycle. It is created and persisted, under the
//! store's exclusive mutation (and so under the realm's credential fence),
//! before any intent depends on it; an existing key is read without the
//! fence; restore only ever loads it, so a lost key proves nothing and is
//! never replaced there.

use std::fmt::Write as _;

use hmac::{Hmac, Mac};
use sha2::Sha256;

#[cfg(not(target_arch = "wasm32"))]
use super::token_store::{
    CredentialMutationError, CredentialMutationOutcome, ProviderAuthPersistence,
};
use super::token_store::{
    PersistedAuthMode, PersistedTokens, TokenKey, TokenStore, TokenStoreError,
};
use crate::connection::RealmId;

/// The metadata field carrying a stored key's id.
const KEY_ID_FIELD: &str = "mcp_attachment_identity_key_id";

/// A realm's MCP attachment identity key. `Debug` shows only its id.
#[derive(Clone, PartialEq, Eq)]
pub struct McpAttachmentIdentityKey {
    id: String,
    material: [u8; 32],
}

impl std::fmt::Debug for McpAttachmentIdentityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpAttachmentIdentityKey")
            .field("id", &self.id)
            .field("material", &crate::redact::REDACTED)
            .finish()
    }
}

impl McpAttachmentIdentityKey {
    /// A fresh random key with a fresh id.
    #[must_use]
    pub fn generate() -> Self {
        let mut material = [0u8; 32];
        material[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        material[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            material,
        }
    }

    /// The key's id, recorded beside every binding made with it.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// HMAC-SHA-256 under this key over `domain` and `parts`, each length
    /// prefixed, as `hmac-sha256:<hex>`. `None` only if the MAC refuses the
    /// key, which HMAC never does for a 32-byte key.
    #[must_use]
    pub fn mac<'a>(
        &self,
        domain: &'a str,
        parts: impl IntoIterator<Item = &'a str>,
    ) -> Option<String> {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.material).ok()?;
        for part in std::iter::once(domain).chain(parts) {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part.as_bytes());
        }
        Some(format!("hmac-sha256:{:x}", mac.finalize().into_bytes()))
    }

    // Only the coordinated resolver writes a key, and it is not on wasm32.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn to_tokens(&self) -> PersistedTokens {
        let mut hex = String::with_capacity(self.material.len() * 2);
        for byte in self.material {
            let _ = write!(hex, "{byte:02x}");
        }
        let mut tokens = PersistedTokens::api_key(hex);
        tokens.auth_mode = PersistedAuthMode::McpAttachmentIdentityKey;
        tokens.metadata = serde_json::json!({ KEY_ID_FIELD: self.id });
        tokens
    }

    fn from_tokens(tokens: &PersistedTokens) -> Option<Self> {
        if tokens.auth_mode != PersistedAuthMode::McpAttachmentIdentityKey {
            return None;
        }
        let id = tokens.metadata.get(KEY_ID_FIELD)?.as_str()?.to_owned();
        let hex = tokens.primary_secret.as_deref()?;
        if id.is_empty() || hex.len() != 64 {
            return None;
        }
        let mut material = [0u8; 32];
        for (index, byte) in material.iter_mut().enumerate() {
            *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
        }
        Some(Self { id, material })
    }
}

/// Why a realm's identity key could not be resolved.
#[derive(Debug, Clone, thiserror::Error)]
pub enum McpAttachmentIdentityKeyError {
    #[error("MCP attachment identity key store failed: {0}")]
    Store(String),
    #[error("MCP attachment identity key mutation failed: {0}")]
    Mutation(String),
    #[error("stored MCP attachment identity key is unreadable")]
    Unreadable,
}

impl From<TokenStoreError> for McpAttachmentIdentityKeyError {
    fn from(error: TokenStoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// Load `realm`'s identity key, if it has one. Never creates a key: a missing
/// key is `None`, and an unreadable stored one an error.
pub async fn load_mcp_attachment_identity_key(
    store: &dyn TokenStore,
    realm: &RealmId,
) -> Result<Option<McpAttachmentIdentityKey>, McpAttachmentIdentityKeyError> {
    match store
        .load_for(
            &TokenKey::mcp_attachment_identity_key(realm.clone()),
            super::token_store::TokenStoreAccess::Unattended,
        )
        .await?
    {
        None => Ok(None),
        Some(tokens) => McpAttachmentIdentityKey::from_tokens(&tokens)
            .map(Some)
            .ok_or(McpAttachmentIdentityKeyError::Unreadable),
    }
}

/// `realm`'s identity key, created and persisted first if it has none.
///
/// An existing key is read first, as any read is, without the realm's
/// credential fence: resolving a realm's key never waits for, or is refused
/// by, an admission that holds the fence. Only a missing key is created, and
/// only under the store's exclusive mutation for the key, which takes the
/// credential fence of the realm: concurrent callers (in any process sharing
/// the store) agree on one key, and the key is durable before it is
/// returned. While the fence is held the creation is refused (`Mutation`)
/// and nothing is written. An unreadable stored key is an error, never
/// silently replaced. (Not on wasm32: its token-store futures are not Send,
/// which the exclusive mutation requires.)
#[cfg(not(target_arch = "wasm32"))]
pub async fn resolve_or_create_mcp_attachment_identity_key(
    persistence: &ProviderAuthPersistence,
    realm: &RealmId,
) -> Result<McpAttachmentIdentityKey, McpAttachmentIdentityKeyError> {
    let store = persistence.token_store();
    if let Some(existing) = load_mcp_attachment_identity_key(store.as_ref(), realm).await? {
        return Ok(existing);
    }
    let key = TokenKey::mcp_attachment_identity_key(realm.clone());
    let mutation_key = key.clone();
    let outcome = persistence
        .refresh_coordinator()
        .with_exclusive_mutation_for(
            key,
            super::token_store::TokenStoreAccess::Unattended,
            Box::new(move |guard| {
                Box::pin(async move {
                    // Created by another caller since the first read: keep it.
                    let stored = store
                        .load_for(
                            &mutation_key,
                            super::token_store::TokenStoreAccess::Unattended,
                        )
                        .await
                        .map_err(CredentialMutationError::from_token_store)?;
                    if let Some(tokens) = stored {
                        return Ok(CredentialMutationOutcome::Persisted(tokens));
                    }
                    let tokens = McpAttachmentIdentityKey::generate().to_tokens();
                    store
                        .save_for(
                            &guard,
                            &mutation_key,
                            &tokens,
                            super::token_store::TokenStoreAccess::Unattended,
                        )
                        .await
                        .map_err(CredentialMutationError::from_token_store)?;
                    Ok(CredentialMutationOutcome::Persisted(tokens))
                })
            }),
        )
        .await
        .map_err(|error| McpAttachmentIdentityKeyError::Mutation(error.to_string()))?;
    match outcome {
        CredentialMutationOutcome::Persisted(tokens) => {
            McpAttachmentIdentityKey::from_tokens(&tokens)
                .ok_or(McpAttachmentIdentityKeyError::Unreadable)
        }
        CredentialMutationOutcome::Cleared => Err(McpAttachmentIdentityKeyError::Unreadable),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_key_round_trips_its_stored_form_and_never_prints_its_material() {
        let key = McpAttachmentIdentityKey::generate();
        let tokens = key.to_tokens();
        assert_eq!(
            McpAttachmentIdentityKey::from_tokens(&tokens),
            Some(key.clone())
        );
        let material = tokens.primary_secret.clone().unwrap();
        assert!(!format!("{key:?}").contains(&material));
        assert!(!format!("{tokens:?}").contains(&material));
        let mut credential = tokens;
        credential.auth_mode = PersistedAuthMode::ApiKey;
        assert_eq!(McpAttachmentIdentityKey::from_tokens(&credential), None);
    }

    #[test]
    fn macs_are_keyed_and_unambiguous() {
        let key = McpAttachmentIdentityKey::generate();
        let other = McpAttachmentIdentityKey::generate();
        assert_eq!(key.mac("d", ["a", "b"]), key.mac("d", ["a", "b"]));
        assert_ne!(key.mac("d", ["a", "b"]), other.mac("d", ["a", "b"]));
        assert_ne!(key.mac("d", ["ab"]), key.mac("d", ["a", "b"]));
        assert_ne!(key.mac("d", ["a"]), key.mac("e", ["a"]));
    }
}
