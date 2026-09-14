//! Fresh owning-host profile and activation declaration reads.
//!
//! This source selects content only. It neither issues a grant nor installs a
//! runnable binding; provider resolution and generated activation remain
//! separate required steps.

use chrono::{DateTime, Utc};
use meerkat_core::connection::RealmChain;
use meerkat_core::live_execution::activation::{LiveExecutorSelector, LiveProfileRevision};
use meerkat_core::live_execution::profile::{
    LiveProfileDefinition, LiveProfileEntry, LiveProfileId,
};
use meerkat_core::{Config, EffectiveConfigReader, RealmId};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use crate::live_activation::{
    FileLiveExecutionGrantSource, LiveActivationLookup, LiveActivationSelection,
    LiveActivationSourceError, select_live_activation,
};

pub struct LiveProfileDeclarationSource {
    config: EffectiveConfigReader,
    activations: FileLiveExecutionGrantSource,
}

pub struct LiveProfileDeclarationLookup<'a, Member> {
    pub profile_id: &'a LiveProfileId,
    pub requesting_realm: &'a RealmId,
    pub executor: &'a LiveExecutorSelector<Member>,
    pub now: DateTime<Utc>,
}

#[derive(Debug)]
pub enum CurrentLiveProfileDeclaration<Member> {
    Missing,
    Disabled,
    Configured(Box<LiveProfileDeclaration<Member>>),
}

/// Complete document-set content evidence, never a runtime generation or grant.
pub struct LiveProfileObservation<Member> {
    declaration: CurrentLiveProfileDeclaration<Member>,
    digest: LiveProfileObservationDigest,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LiveProfileObservationDigest([u8; 32]);

impl std::fmt::Debug for LiveProfileObservationDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveProfileObservationDigest([REDACTED])")
    }
}

impl LiveProfileObservationDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl<Member> LiveProfileObservation<Member> {
    pub fn declaration(&self) -> &CurrentLiveProfileDeclaration<Member> {
        &self.declaration
    }

    pub fn digest(&self) -> LiveProfileObservationDigest {
        self.digest
    }
}

/// Immutable declaration content from one config read and its activation
/// selection. Resolving credentials does not grant execution or establish
/// currentness after an await; activation still requires the generated owner.
pub struct LiveProfileDeclaration<Member> {
    config: Config,
    profile_id: LiveProfileId,
    requesting_realm: RealmId,
    revision: LiveProfileRevision,
    definition: Box<LiveProfileDefinition>,
    activation: LiveActivationSelection<Member>,
}

impl<Member> std::fmt::Debug for LiveProfileDeclaration<Member> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveProfileDeclaration")
            .field("profile_id", &self.profile_id)
            .field("requesting_realm", &self.requesting_realm)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl<Member> LiveProfileDeclaration<Member> {
    pub fn profile_id(&self) -> &LiveProfileId {
        &self.profile_id
    }

    pub fn requesting_realm(&self) -> &RealmId {
        &self.requesting_realm
    }

    pub fn revision(&self) -> LiveProfileRevision {
        self.revision
    }

    pub fn definition(&self) -> &LiveProfileDefinition {
        &self.definition
    }

    pub fn activation(&self) -> &LiveActivationSelection<Member> {
        &self.activation
    }

    /// Resolve this declaration with its own model and credential-owner
    /// configuration, never a separately supplied or reloaded config.
    pub async fn resolve_adapter_factory(
        &self,
        factory: &crate::AgentFactory,
        auth_lease_handle: Option<meerkat_core::handles::GeneratedAuthLeaseHandle>,
    ) -> Result<
        std::sync::Arc<dyn meerkat_llm_core::live_adapter_factory::LiveAdapterFactory>,
        crate::FactoryError,
    > {
        factory
            .resolve_live_adapter_factory_for_definition(
                &self.config,
                &self.profile_id,
                &self.definition,
                Some(&self.requesting_realm),
                auth_lease_handle,
            )
            .await
    }
}

impl LiveProfileDeclarationSource {
    pub fn new(config: EffectiveConfigReader, activations: FileLiveExecutionGrantSource) -> Self {
        Self {
            config,
            activations,
        }
    }

    /// Read both sources, using the profile's exact computed revision in
    /// activation selection. Revision mismatch refuses; a matched selection
    /// remains declaration content, not post-await currentness authority.
    pub async fn load<Member: Clone + PartialEq + DeserializeOwned>(
        &self,
        lookup: LiveProfileDeclarationLookup<'_, Member>,
    ) -> Result<CurrentLiveProfileDeclaration<Member>, LiveProfileDeclarationSourceError> {
        Ok(self.observe(lookup).await?.declaration)
    }

    pub async fn observe<Member: Clone + PartialEq + DeserializeOwned>(
        &self,
        lookup: LiveProfileDeclarationLookup<'_, Member>,
    ) -> Result<LiveProfileObservation<Member>, LiveProfileDeclarationSourceError> {
        let (mut config, config_digest) = self
            .config
            .observe_effective_config(lookup.requesting_realm)
            .await?
            .into_parts();
        let chain = RealmChain::resolve(&config, lookup.requesting_realm)?;
        let documents = self.activations.observe::<Member>(&chain).await?;
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-profile-observation.v1\0");
        digest.update(config_digest.as_bytes());
        digest.update(documents.digest());
        digest.update((lookup.profile_id.as_str().len() as u64).to_be_bytes());
        digest.update(lookup.profile_id.as_str().as_bytes());
        let digest = LiveProfileObservationDigest(digest.finalize().into());
        let definition = match config.live.profiles.remove(lookup.profile_id) {
            None => {
                return Ok(LiveProfileObservation {
                    declaration: CurrentLiveProfileDeclaration::Missing,
                    digest,
                });
            }
            Some(LiveProfileEntry::Disabled) => {
                return Ok(LiveProfileObservation {
                    declaration: CurrentLiveProfileDeclaration::Disabled,
                    digest,
                });
            }
            Some(LiveProfileEntry::Configured(definition)) => definition,
        };
        let revision = LiveProfileRevision::of(&definition)?;
        let activation = select_live_activation(
            &chain,
            documents.documents(),
            &LiveActivationLookup {
                profile_id: lookup.profile_id,
                profile_revision: revision,
                requesting_realm: lookup.requesting_realm,
                executor: lookup.executor,
                now: lookup.now,
            },
        )?;
        Ok(LiveProfileObservation {
            declaration: CurrentLiveProfileDeclaration::Configured(Box::new(
                LiveProfileDeclaration {
                    config,
                    profile_id: lookup.profile_id.clone(),
                    requesting_realm: lookup.requesting_realm.clone(),
                    revision,
                    definition,
                    activation,
                },
            )),
            digest,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LiveProfileDeclarationSourceError {
    #[error(transparent)]
    Config(#[from] meerkat_core::ConfigError),
    #[error(transparent)]
    RealmChain(#[from] meerkat_core::connection::RealmChainError),
    #[error(transparent)]
    Activation(#[from] LiveActivationSourceError),
    #[error("Live profile revision encoding failed: {0}")]
    RevisionEncoding(#[from] serde_json::Error),
}
