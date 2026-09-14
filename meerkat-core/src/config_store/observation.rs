use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};

use super::EffectiveConfigReader;
use crate::{
    Config, ConfigError,
    connection::{MAX_REALM_CHAIN_DEPTH, RealmChain, RealmId},
};

/// Typed values and presence parsed from one observed byte string. This
/// neither promises an atomic multi-file snapshot nor fences future edits.
pub struct ConfigDocumentObservation {
    config: Config,
    raw: toml::Value,
    digest: [u8; 32],
}

impl ConfigDocumentObservation {
    pub fn from_toml(content: &str) -> Result<Self, ConfigError> {
        let raw: toml::Value =
            toml::from_str(content).map_err(|_| ConfigError::InvalidDocumentObservation)?;
        let config: Config = raw
            .clone()
            .try_into()
            .map_err(|_| ConfigError::InvalidDocumentObservation)?;
        config
            .reject_unwired_agent_provider_params()
            .map_err(|_| ConfigError::InvalidDocumentObservation)?;
        Ok(Self {
            config,
            raw,
            digest: Sha256::digest(content.as_bytes()).into(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ConfigObservationDigest([u8; 32]);

impl ConfigObservationDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for ConfigObservationDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfigObservationDigest([REDACTED])")
    }
}

pub struct EffectiveConfigObservation {
    config: Config,
    digest: ConfigObservationDigest,
}

impl EffectiveConfigObservation {
    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn digest(&self) -> ConfigObservationDigest {
        self.digest
    }

    pub fn into_parts(self) -> (Config, ConfigObservationDigest) {
        (self.config, self.digest)
    }
}

impl EffectiveConfigReader {
    /// Observe each selected document once, then use the existing chain and
    /// merge authorities. Missing documents are part of the comparison image.
    /// Callers must reobserve around awaited resolution and publish through
    /// their runtime owner; equality here is only content evidence.
    pub async fn observe_effective_config(
        &self,
        head: &RealmId,
    ) -> Result<EffectiveConfigObservation, ConfigError> {
        let mut docs = BTreeMap::new();
        let mut raw_docs = BTreeMap::new();
        let mut digests = BTreeMap::new();
        let mut seen = BTreeSet::new();
        let mut frontier = vec![RealmId::global(), head.clone()];
        while let Some(realm) = frontier.pop() {
            if !seen.insert(realm.clone()) {
                continue;
            }

            #[cfg(all(test, not(target_arch = "wasm32")))]
            mod tests {
                use super::*;
                use crate::RealmConfigSource;
                use std::sync::{Arc, Mutex};

                #[derive(Default)]
                struct Documents(Mutex<BTreeMap<String, String>>);

                #[async_trait::async_trait]
                impl RealmConfigSource for Documents {
                    async fn config_for_realm(
                        &self,
                        _: &RealmId,
                    ) -> Result<Option<Config>, ConfigError> {
                        Err(ConfigError::InternalError(
                            "separate typed read must not be used".into(),
                        ))
                    }

                    async fn observe_config_for_realm(
                        &self,
                        realm: &RealmId,
                    ) -> Result<Option<ConfigDocumentObservation>, ConfigError>
                    {
                        self.0
                            .lock()
                            .unwrap()
                            .get(realm.as_str())
                            .map(|text| ConfigDocumentObservation::from_toml(text))
                            .transpose()
                    }
                }

                #[tokio::test]
                async fn coherent_observation_binds_presence_ancestors_and_absence()
                -> Result<(), ConfigError> {
                    let source = Arc::new(Documents::default());
                    let head = RealmId::parse("child").unwrap();
                    let reader = EffectiveConfigReader::new(source.clone());
                    let absent = reader.observe_effective_config(&head).await?;
                    source
                        .0
                        .lock()
                        .unwrap()
                        .insert("child".into(), String::new());
                    let empty = reader.observe_effective_config(&head).await?;
                    assert_ne!(absent.digest(), empty.digest());
                    source.0.lock().unwrap().extend([
                        ("global".into(), "[skills]\nenabled = false\n".into()),
                        (
                            "child".into(),
                            "[realm.child]\nparent = \"global\"\n[skills]\nenabled = true\n".into(),
                        ),
                    ]);
                    let first = reader.observe_effective_config(&head).await?;
                    assert!(
                        first.config().skills.enabled,
                        "same-read presence overrides back to default"
                    );
                    assert_eq!(
                        reader.observe_effective_config(&head).await?.digest(),
                        first.digest()
                    );
                    source
                        .0
                        .lock()
                        .unwrap()
                        .insert("global".into(), "[skills]\nenabled = true\n".into());
                    let second = reader.observe_effective_config(&head).await?;
                    assert!(second.config().skills.enabled);
                    assert_ne!(
                        second.digest(),
                        first.digest(),
                        "all ancestor content is bound even if shadowed"
                    );
                    source
                        .0
                        .lock()
                        .unwrap()
                        .insert("child".into(), "secret = \"SENTINEL\"\n[".into());
                    let error = reader.observe_effective_config(&head).await.err().unwrap();
                    assert!(matches!(error, ConfigError::InvalidDocumentObservation));
                    assert!(!format!("{error:?} {error}").contains("SENTINEL"));
                    Ok(())
                }

                struct LegacyOnly;

                #[async_trait::async_trait]
                impl RealmConfigSource for LegacyOnly {
                    async fn config_for_realm(
                        &self,
                        _: &RealmId,
                    ) -> Result<Option<Config>, ConfigError> {
                        Ok(None)
                    }
                }

                #[tokio::test]
                async fn coherent_observation_never_upgrades_legacy_separate_reads() {
                    let reader = EffectiveConfigReader::new(Arc::new(LegacyOnly));
                    let head = RealmId::parse("child").unwrap();
                    assert!(reader.effective_config(&head).await.is_ok());
                    assert!(matches!(
                        reader.observe_effective_config(&head).await,
                        Err(ConfigError::CoherentObservationUnsupported)
                    ));
                }
            }
            if seen.len() > MAX_REALM_CHAIN_DEPTH + 4 {
                return Err(ConfigError::IncompleteDocumentObservation);
            }
            match self.source.observe_config_for_realm(&realm).await? {
                None => {
                    digests.insert(realm, None);
                }
                Some(observation) => {
                    if let Some(parent) = observation
                        .config
                        .realm
                        .get(realm.as_str())
                        .and_then(|section| section.parent.clone())
                    {
                        frontier.push(parent);
                    }
                    digests.insert(realm.clone(), Some(observation.digest));
                    raw_docs.insert(realm.clone(), observation.raw);
                    docs.insert(realm, observation.config);
                }
            }
        }
        let config = crate::config::compose_effective_config(&docs, &raw_docs, head)?;
        let chain = RealmChain::resolve(&config, head)?;
        if chain.realms().iter().any(|realm| !seen.contains(realm)) {
            return Err(ConfigError::IncompleteDocumentObservation);
        }
        let mut digest = Sha256::new();
        digest.update(b"meerkat.config-observation.v1\0");
        digest.update((head.as_str().len() as u64).to_be_bytes());
        digest.update(head.as_str().as_bytes());
        for (realm, content) in digests {
            digest.update((realm.as_str().len() as u64).to_be_bytes());
            digest.update(realm.as_str().as_bytes());
            match content {
                Some(content) => {
                    digest.update([1]);
                    digest.update(content);
                }
                None => digest.update([0]),
            }
        }
        Ok(EffectiveConfigObservation {
            config,
            digest: ConfigObservationDigest(digest.finalize().into()),
        })
    }
}
