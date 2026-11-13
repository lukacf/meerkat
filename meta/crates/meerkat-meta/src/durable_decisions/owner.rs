//! The runtime's one durable decision owner.

use std::path::Path;
use std::sync::Arc;

use meerkat_core::RealmId;
use meerkat_core::approval::decision::{DecisionId, DecisionIngress, DecisionReference};
use meerkat_runtime::decision::{DecisionWriteMode, DurableDecisionService};

use super::ingress::DecisionIngressDeclarations;
use crate::runtime::metadata::PersistentMetadataStore;

/// The durable decisions of one runtime: the decision owner of its
/// persistent state directory (`<state dir>/approval_decisions.sqlite3`,
/// the file Core's persistence keeps decisions in, opened by nothing until
/// the first hold creates it), the decision ingresses its launch declares,
/// and the record of the attested decide events it has bound to their
/// decisions.
///
/// A runtime without a persistent state directory (an ephemeral one, or one
/// on a host store realm) keeps no durable decisions: every verb answers
/// capability unavailable, `no_decision_store`. A candidate launch reads
/// decisions and refuses every write, as Core's owner of a candidate does.
#[derive(Clone)]
pub struct RuntimeDecisions {
    owner: DurableDecisionService,
    ingresses: Arc<DecisionIngressDeclarations>,
    events: Option<DecisionEventRecord>,
    realm: Option<RealmId>,
    /// Signalled when a decision of this runtime changed in a way that may
    /// owe work (held, decided, withdrawn): what wakes the executors the
    /// runtime drives ([`super::DecisionExecutorDriver`]).
    work: Arc<tokio::sync::Notify>,
}

/// Where the runtime records which decision each attested decide event was
/// bound to: its metadata store, under the mob's id.
#[derive(Clone)]
struct DecisionEventRecord {
    store: Arc<dyn PersistentMetadataStore>,
    mob_id: String,
}

/// The decision an attested decide event is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionEventBinding {
    /// The event is this decision's own: bound now, or before.
    Own,
    /// Another decision used the event first.
    Another,
}

impl std::fmt::Debug for RuntimeDecisions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeDecisions")
            .field("available", &self.owner.is_available())
            .field("ingresses", &self.ingresses)
            .field("realm", &self.realm)
            .finish_non_exhaustive()
    }
}

impl Default for RuntimeDecisions {
    fn default() -> Self {
        Self::unavailable()
    }
}

impl RuntimeDecisions {
    /// The decisions of a runtime that keeps none.
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            owner: DurableDecisionService::unavailable("this runtime keeps no durable decisions"),
            ingresses: Arc::default(),
            events: None,
            realm: None,
            work: Arc::default(),
        }
    }

    /// The decisions of the persistent state directory `state_dir`, whose
    /// meerkat-level realm is `realm`. The attested decide events are
    /// recorded in `metadata` under `mob_id`. A runtime that does not speak
    /// for its composition (a candidate launch) reads and refuses writes.
    #[must_use]
    pub fn in_state_dir(
        state_dir: &Path,
        realm: RealmId,
        ingresses: DecisionIngressDeclarations,
        metadata: Arc<dyn PersistentMetadataStore>,
        mob_id: impl Into<String>,
        speaks_for_composition: bool,
    ) -> Self {
        let owner = meerkat::ApprovalIssuerSelection::Persistent {
            store_path: state_dir.to_path_buf(),
        }
        .open_decisions();
        Self {
            owner: if speaks_for_composition {
                owner
            } else {
                owner.for_candidate_launch()
            },
            ingresses: Arc::new(ingresses),
            events: Some(DecisionEventRecord {
                store: metadata,
                mob_id: mob_id.into(),
            }),
            realm: Some(realm),
            work: Arc::default(),
        }
    }

    /// These decisions with the decision ingresses `ingresses` declared.
    #[must_use]
    pub fn with_ingresses(mut self, ingresses: DecisionIngressDeclarations) -> Self {
        self.ingresses = Arc::new(ingresses);
        self
    }

    /// The decision owner. A host composition holds its own kinds' decisions
    /// through it.
    pub fn owner(&self) -> &DurableDecisionService {
        &self.owner
    }

    /// Say that a decision of this runtime changed in a way that may owe
    /// work: one was held, decided or withdrawn. The executors the runtime
    /// drives make a pass; with none, nothing happens. A surface that
    /// writes a decision through [`Self::owner`] calls it after the write.
    pub fn notify_work(&self) {
        self.work.notify_one();
    }

    /// The signal [`Self::notify_work`] raises.
    pub(crate) fn work_signal(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.work)
    }

    /// Whether the runtime keeps durable decisions.
    pub fn is_available(&self) -> bool {
        self.owner.is_available()
    }

    /// Whether the owner refuses every write (a candidate launch).
    pub(crate) fn is_candidate(&self) -> bool {
        matches!(self.owner.write_mode(), DecisionWriteMode::CandidateLaunch)
    }

    /// The decision ingresses the launch declares.
    pub fn ingresses(&self) -> &DecisionIngressDeclarations {
        &self.ingresses
    }

    /// The runtime's meerkat-level realm, in whose domain the stdio
    /// operator is named.
    pub fn realm(&self) -> Option<&RealmId> {
        self.realm.as_ref()
    }

    /// Bind the decide event `event_ref` of the ingress `ingress` to
    /// `decision_id`, durably, before the decide input: the first decision
    /// that uses an event keeps it, and a decision may repeat its own.
    ///
    /// # Errors
    /// The metadata store's failure, or that the runtime keeps no record.
    pub(crate) async fn bind_event(
        &self,
        ingress: &DecisionIngress,
        event_ref: &DecisionReference,
        decision_id: &DecisionId,
    ) -> Result<DecisionEventBinding, String> {
        let Some(events) = self.events.as_ref() else {
            return Err("this runtime keeps no record of decision events".to_string());
        };
        let bound = events
            .store
            .bind_decision_event(
                &events.mob_id,
                ingress.as_str(),
                event_ref.as_str(),
                decision_id.as_str(),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(if bound == decision_id.as_str() {
            DecisionEventBinding::Own
        } else {
            DecisionEventBinding::Another
        })
    }
}
