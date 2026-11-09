//! Host-side model-client acquisition for the off-turn memory stages, plus
//! the record-fetch/provenance vocabulary the recall coordinator renders.
//!
//! This module owns shared model-client acquisition for off-turn memory
//! stages, plus the record-fetch/provenance vocabulary used by deterministic
//! recall. The retired §8.3 LLM Selector introduced these seams, but no live
//! type here is selector-specific.
//!
//! Two clusters live here, both re-homed verbatim from `memory::selector`:
//!
//! 1. Client acquisition (§8.1 invocation seam). [`FactoryModelClientHandle`]
//!    wraps meerkat's `AgentFactory::build_llm_client_for_identity`, and the
//!    Distiller (§8.4), Steward (§8.5) and Hygienist (§8.6) all obtain their
//!    clients through this one factory path rather than growing parallel ones
//!    (§8.1 dogma rule 7). [`ModelClientHandle`] is the trait they hold it by
//!    and [`ModelClientError`] is the classified failure they map into their own
//!    error types.
//!
//! 2. Record bodies with provenance. [`AnnotatedRecord`] and
//!    [`RecordProvenance`] are what the coordinator's injection renderer
//!    consumes on EVERY path, selector or not - the default lexical recall
//!    path wraps its plain records into `AnnotatedRecord` with
//!    `provenance: None`. [`SelectedRecordFetch`] is the store-side capability
//!    that can supply the labelled form.
//!
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use meerkat_client::{FactoryError, LlmClient};
use meerkat_core::{Provider, SessionLlmIdentity};

use crate::identity_first::agent_memory::{
    AgentMemoryError, AgentMemoryRecord, compact_whitespace,
};
use crate::memory::records::{MemoryScope, RecordMeta, TrustTier};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ModelClientError {
    /// Client construction / auth resolution failed.
    Auth(String),
    /// The provider call itself failed.
    Client(String),
}

impl std::fmt::Display for ModelClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(msg) => write!(f, "model client auth error: {msg}"),
            Self::Client(msg) => write!(f, "model client error: {msg}"),
        }
    }
}

impl std::error::Error for ModelClientError {}

// ---------------------------------------------------------------------------
// Client acquisition (§8.1 invocation seam)
// ---------------------------------------------------------------------------

/// How a stage obtains (and re-obtains) its model client. The real
/// implementation wraps meerkat's factory seam; tests supply a mock.
#[async_trait]
pub trait ModelClientHandle: Send + Sync {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, ModelClientError>;
    /// Drop any cached client so the next `client()` re-resolves auth.
    fn invalidate(&self);
}

/// Real handle over `AgentFactory::build_llm_client_for_identity`
/// (meerkat 0.7.9 `factory.rs`): realm auth binding + model catalog
/// resolution, the same seam session model hot-swap uses. Clients are
/// cached per `(realm, model)`; [`ModelClientHandle::invalidate`] clears the
/// cache so an auth failure re-enters resolution.
pub struct FactoryModelClientHandle {
    factory: meerkat::AgentFactory,
    config: meerkat::Config,
    realm: String,
    identity: SessionLlmIdentity,
    cache: Mutex<HashMap<(String, String), Arc<dyn LlmClient>>>,
}

impl FactoryModelClientHandle {
    /// The factory seam keyed by raw model/provider - the Distiller (§8.4),
    /// Steward (§8.5) and Hygienist (§8.6) obtain their clients through this
    /// exact path rather than growing a parallel one (§8.1 dogma rule 7).
    ///
    /// The profile-taking `new` constructor retired with the selector stage:
    /// it took a `SelectorProfile`, and every remaining caller already spells
    /// its own profile's `{model, provider}` pair.
    pub fn for_model(
        store_path: impl Into<PathBuf>,
        config: meerkat::Config,
        realm: impl Into<String>,
        model: &str,
        provider: Provider,
    ) -> Self {
        Self {
            factory: meerkat::AgentFactory::new(store_path.into()),
            config,
            realm: realm.into(),
            identity: SessionLlmIdentity {
                model: model.to_string(),
                provider,
                self_hosted_server_id: None,
                provider_params: None,
                // None = the realm's default binding for the provider; the
                // explicit-binding choice is §8.1 open question 3.
                auth_binding: None,
            },
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve and persist this handle's provider credentials in
    /// `persistence` instead of the process default store: the runtime's
    /// provider credential store when its host names one, so a memory stage
    /// shares the credentials and the refresh authority of the runtime's
    /// agents.
    #[must_use]
    pub fn with_provider_auth_persistence(
        mut self,
        persistence: meerkat::ProviderAuthPersistence,
    ) -> Self {
        self.factory = self.factory.with_provider_auth_persistence(persistence);
        self
    }

    /// The credential authority this handle's factory resolves with.
    #[cfg(test)]
    pub(crate) fn provider_auth_authority(
        &self,
    ) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.factory
            .resolution_provider_auth_persistence()
            .ok()
            .flatten()
            .as_ref()
            .map(meerkat::ProviderAuthPersistence::authority_id)
    }
}

#[async_trait]
impl ModelClientHandle for FactoryModelClientHandle {
    async fn client(&self) -> Result<Arc<dyn LlmClient>, ModelClientError> {
        let key = (self.realm.clone(), self.identity.model.clone());
        if let Some(client) = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return Ok(client.clone());
        }
        let client = self
            .factory
            .build_llm_client_for_identity(&self.config, &self.identity)
            .await
            .map_err(|err| match err {
                FactoryError::ProviderAuth(_) | FactoryError::ConnectionTarget(_) => {
                    ModelClientError::Auth(err.to_string())
                }
                other => ModelClientError::Client(other.to_string()),
            })?;
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, client.clone());
        Ok(client)
    }

    fn invalidate(&self) {
        self.cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

// ---------------------------------------------------------------------------
// Manifest row rendering
// ---------------------------------------------------------------------------

/// One manifest row: id, kind, age, rank, title — description (the fields
/// the prompt bundle promises, in fixture `RecordMeta` shape).
///
/// Still live after the selector's retirement: the Distiller renders its
/// candidate manifest through this exact row shape, and the Steward renders
/// its working set through it, so the two off-turn stages agree on how a
/// record announces itself to a model.
pub(crate) fn render_manifest_row(meta: &RecordMeta) -> String {
    let rank = match meta.rank {
        Some(rank) => format!("rank {rank}"),
        None => "unranked".to_string(),
    };
    let mut row = format!(
        "- {} [{}, {}, {}] {}",
        meta.id,
        meta.kind.as_str(),
        age_phrase(meta.age_days),
        rank,
        compact_whitespace(&meta.title),
    );
    let description = compact_whitespace(&meta.description);
    if !description.is_empty() {
        row.push_str(" — ");
        row.push_str(&description);
    }
    row
}

fn age_phrase(age_days: u64) -> String {
    match age_days {
        0 => "saved today".to_string(),
        1 => "saved 1 day ago".to_string(),
        n => format!("saved {n} days ago"),
    }
}

// ---------------------------------------------------------------------------
// Body fetch with provenance
// ---------------------------------------------------------------------------

/// §7.2/§9.1 per-record provenance for rendered bodies: the scope the
/// record was read from and its trust tier, so the injection envelope can
/// label each quoted observation instead of co-rendering scopes
/// indistinguishably.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordProvenance {
    pub scope: MemoryScope,
    pub trust: TrustTier,
}

/// A fetched record body plus its provenance, when the store can supply
/// it. `provenance: None` renders the body without scope/trust labels
/// (age still renders from the record's own timestamps).
///
/// This is the renderer's input type on EVERY recall path: the default
/// lexical path wraps its plain records with `provenance: None`, so deleting
/// this type would break default recall, not just an optional stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotatedRecord {
    pub record: AgentMemoryRecord,
    pub provenance: Option<RecordProvenance>,
}

/// Fetch active record bodies by id across the composed scopes, in the
/// order the ids were requested. Deliberately a standalone trait rather
/// than an `AgentMemoryProvider` method: the v2 provider trait is owned by
/// the recorder/taint cluster and stays untouched; manifest-capable stores
/// opt in here so id-addressed body reads stay a capability, not a store kind.
#[async_trait]
pub trait SelectedRecordFetch: Send + Sync {
    async fn fetch_records(
        &self,
        scopes: &[MemoryScope],
        ids: &[String],
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError>;

    /// Bodies plus §7.2 provenance labels. The default delegates to
    /// [`Self::fetch_records`] with no provenance so existing stores keep
    /// compiling; stores that know each record's scope and trust tier
    /// should override so injected bodies carry their labels.
    async fn fetch_records_annotated(
        &self,
        scopes: &[MemoryScope],
        ids: &[String],
    ) -> Result<Vec<AnnotatedRecord>, AgentMemoryError> {
        Ok(self
            .fetch_records(scopes, ids)
            .await?
            .into_iter()
            .map(|record| AnnotatedRecord {
                record,
                provenance: None,
            })
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::memory::records::MemoryKind;

    fn meta(id: &str, title: &str, description: &str, age_days: u64) -> RecordMeta {
        RecordMeta {
            id: id.to_string(),
            kind: MemoryKind::Gotcha,
            title: title.to_string(),
            description: description.to_string(),
            age_days,
            rank: Some(1),
        }
    }

    /// The row shape the Distiller and Steward prompts both promise. Pinned
    /// here because the selector's own row tests retired with its stage.
    #[test]
    fn manifest_row_renders_id_kind_age_rank_title_and_description() {
        let row = render_manifest_row(&meta("mem-1", "Cargo wrapper", "When running cargo", 3));
        assert!(
            row.starts_with("- mem-1 [gotcha, saved 3 days ago, rank 1] Cargo wrapper"),
            "{row}"
        );
        assert!(row.contains("When running cargo"), "{row}");
    }

    #[test]
    fn manifest_row_omits_the_description_separator_when_empty() {
        let row = render_manifest_row(&meta("mem-2", "Titled only", "   ", 0));
        assert!(row.ends_with("Titled only"), "{row}");
    }

    #[test]
    fn manifest_row_age_phrases_singular_and_plural() {
        assert!(render_manifest_row(&meta("a", "t", "", 0)).contains("saved today"));
        assert!(render_manifest_row(&meta("b", "t", "", 1)).contains("saved 1 day ago"));
        assert!(render_manifest_row(&meta("c", "t", "", 9)).contains("saved 9 days ago"));
    }

    /// An unranked row still renders every other field, and says so.
    #[test]
    fn manifest_row_marks_unranked_records() {
        let mut unranked = meta("mem-3", "No rank", "", 2);
        unranked.rank = None;
        assert!(
            render_manifest_row(&unranked).contains("unranked"),
            "{}",
            render_manifest_row(&unranked)
        );
    }
}
