//! MobStorage bundle.
//!
//! Groups the event store with a session store for a mob's isolated storage.

#[cfg(not(target_arch = "wasm32"))]
use crate::store::SqliteMobStores;
use crate::store::private::{
    MobDefinitionEpochAppendOutcome, MobDefinitionEpochPersistenceAuthority,
    MobEventStoreSealed as _,
};
use crate::store::{
    ForkedParticipantStore, InMemoryForkedParticipantStore, InMemoryMobEventStore,
    InMemoryMobIdentityStatusStore, InMemoryMobIdentityStore, InMemoryMobRunStore,
    InMemoryMobRuntimeMetadataStore, InMemoryMobSpecStore, InMemoryRealmProfileStore,
    MobEventStore, MobIdentityMemberStore, MobIdentityStatusStore, MobIdentityStore, MobRunStore,
    MobRuntimeMetadataStore, MobSpecStore, RealmProfileStore, authority_validating_mob_run_store,
    current_definition_authority,
};
use crate::{
    MobDefinition, MobError,
    error::MobDefinitionProjectionMismatchKind,
    event::{MobEvent, MobEventKind, NewMobEvent},
};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

/// Read-only health of the durable definition authority and spec projection.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobDefinitionProjectionHealth {
    Healthy {
        authority_epoch: u64,
        projection_revision: u64,
    },
    ProjectionMissing {
        authority_epoch: u64,
    },
    ProjectionStale {
        authority_epoch: u64,
        projection_revision: u64,
    },
    Diverged {
        authority_epoch: u64,
        projection_revision: u64,
        kind: MobDefinitionProjectionMismatchKind,
    },
}

/// Receipt for a durable definition epoch committed through [`MobStorage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobDefinitionRevision {
    pub epoch: u64,
    pub projection_revision: u64,
    pub event_cursor: u64,
}

/// Storage-minted canonical definition authority used to bind a later resume.
#[derive(Debug, Clone, PartialEq)]
pub struct MobDefinitionSnapshot {
    definition: MobDefinition,
    epoch: u64,
    event_cursor: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefinitionProjectionComposition {
    Independent,
    AtomicSqlite,
}

impl MobDefinitionSnapshot {
    #[must_use]
    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn event_cursor(&self) -> u64 {
        self.event_cursor
    }
}

/// Storage bundle for a mob.
///
/// Contains both the mob event store (structural state) and session
/// store (for meerkat sessions). Each mob has its own isolated storage.
#[derive(Clone)]
pub struct MobStorage {
    /// Event store for mob structural events.
    pub(crate) events: Arc<dyn MobEventStore>,
    /// Flow run persistence store.
    pub(crate) runs: Arc<dyn MobRunStore>,
    /// Flow spec persistence store.
    pub(crate) specs: Arc<dyn MobSpecStore>,
    /// Proof that the event and spec stores share one built-in transaction boundary.
    pub(crate) definition_projection_composition: DefinitionProjectionComposition,
    /// Runtime metadata store for supervisor authority and compatibility projections.
    pub(crate) runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    /// Sole desired-state authority for identity intent, leases, and immutable custody.
    pub(crate) identity: Arc<dyn MobIdentityStore>,
    /// Optional built-in-only transaction joining identity authority to the
    /// structural member and identity-local wiring event targets. Custom store
    /// compositions cannot provide this implicitly.
    pub(crate) identity_member: Option<Arc<dyn MobIdentityMemberStore>>,
    /// Replaceable output-only identity convergence diagnostics.
    pub(crate) identity_status: Arc<dyn MobIdentityStatusStore>,
    /// Process-owned ordering custody scoped to this exact storage composition.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) identity_status_projection_order: crate::runtime::IdentityStatusProjectionOrder,
    /// Realm-scoped reusable profile store.
    pub(crate) realm_profiles: Option<Arc<dyn RealmProfileStore>>,
    /// Source-owned forked-participant capability records.
    ///
    /// Optional for the same reason as `realm_profiles`: a custom store
    /// composition cannot provide it implicitly, and a runtime without it
    /// simply cannot own forked-participant capabilities.
    pub(crate) forked_participants: Option<Arc<dyn ForkedParticipantStore>>,
}

impl MobStorage {
    /// Create a storage bundle with in-memory stores (for tests and ephemeral mobs).
    pub fn in_memory() -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        let events = InMemoryMobEventStore::new();
        let identity = InMemoryMobIdentityStore::paired_with_event_store(
            &events,
            Arc::new(crate::store::SystemMobIdentityStoreClock),
        );
        let identity_member: Arc<dyn MobIdentityMemberStore> = Arc::new(identity.clone());
        Self {
            events: Arc::new(events),
            runs,
            specs,
            definition_projection_composition: DefinitionProjectionComposition::Independent,
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity: Arc::new(identity),
            identity_member: Some(identity_member),
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
            #[cfg(not(target_arch = "wasm32"))]
            identity_status_projection_order:
                crate::runtime::IdentityStatusProjectionOrder::default(),
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
            forked_participants: Some(Arc::new(InMemoryForkedParticipantStore::new())),
        }
    }

    /// Create in-memory run/spec stores for flow persistence.
    pub fn in_memory_flow_stores() -> (Arc<dyn MobRunStore>, Arc<dyn MobSpecStore>) {
        (
            authority_validating_mob_run_store(Arc::new(InMemoryMobRunStore::new())),
            Arc::new(InMemoryMobSpecStore::new()),
        )
    }

    /// Build a full storage bundle from a custom event store and in-memory flow stores.
    pub fn with_events(events: Arc<dyn MobEventStore>) -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events,
            runs,
            specs,
            definition_projection_composition: DefinitionProjectionComposition::Independent,
            runtime_metadata: Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity: Arc::new(InMemoryMobIdentityStore::new()),
            identity_member: None,
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
            #[cfg(not(target_arch = "wasm32"))]
            identity_status_projection_order:
                crate::runtime::IdentityStatusProjectionOrder::default(),
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
            forked_participants: Some(Arc::new(InMemoryForkedParticipantStore::new())),
        }
    }

    /// Test-only convenience with explicitly ephemeral identity authority.
    #[cfg(test)]
    pub(crate) fn with_events_and_runtime_metadata(
        events: Arc<dyn MobEventStore>,
        runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    ) -> Self {
        let (runs, specs) = Self::in_memory_flow_stores();
        Self {
            events,
            runs,
            specs,
            definition_projection_composition: DefinitionProjectionComposition::Independent,
            runtime_metadata,
            identity: Arc::new(InMemoryMobIdentityStore::new()),
            identity_member: None,
            identity_status: Arc::new(InMemoryMobIdentityStatusStore::new()),
            #[cfg(not(target_arch = "wasm32"))]
            identity_status_projection_order:
                crate::runtime::IdentityStatusProjectionOrder::default(),
            realm_profiles: Some(Arc::new(InMemoryRealmProfileStore::new())),
            forked_participants: Some(Arc::new(InMemoryForkedParticipantStore::new())),
        }
    }

    /// Build a storage bundle from custom store implementations.
    pub fn custom(
        events: Arc<dyn MobEventStore>,
        runs: Arc<dyn MobRunStore>,
        specs: Arc<dyn MobSpecStore>,
        identity: Arc<dyn MobIdentityStore>,
        identity_status: Arc<dyn MobIdentityStatusStore>,
    ) -> Self {
        Self::custom_with_runtime_metadata(
            events,
            runs,
            specs,
            Arc::new(InMemoryMobRuntimeMetadataStore::new()),
            identity,
            identity_status,
        )
    }

    /// Build a storage bundle from custom store implementations, including runtime metadata.
    pub fn custom_with_runtime_metadata(
        events: Arc<dyn MobEventStore>,
        runs: Arc<dyn MobRunStore>,
        specs: Arc<dyn MobSpecStore>,
        runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
        identity: Arc<dyn MobIdentityStore>,
        identity_status: Arc<dyn MobIdentityStatusStore>,
    ) -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let identity_status_projection_order =
            crate::runtime::IdentityStatusProjectionOrder::for_status_store(&identity_status);
        Self {
            events,
            runs: authority_validating_mob_run_store(runs),
            specs,
            definition_projection_composition: DefinitionProjectionComposition::Independent,
            runtime_metadata,
            identity,
            identity_member: None,
            identity_status,
            #[cfg(not(target_arch = "wasm32"))]
            identity_status_projection_order,
            realm_profiles: None,
            forked_participants: None,
        }
    }

    /// Attach the realm-scoped reusable profile store used by mob runtimes.
    pub fn with_realm_profile_store(
        mut self,
        realm_profiles: Option<Arc<dyn RealmProfileStore>>,
    ) -> Self {
        self.realm_profiles = realm_profiles;
        self
    }

    /// Attach the source-owned forked-participant capability store.
    pub fn with_forked_participant_store(
        mut self,
        forked_participants: Option<Arc<dyn ForkedParticipantStore>>,
    ) -> Self {
        self.forked_participants = forked_participants;
        self
    }

    /// Borrow the forked-participant capability store, when composed.
    pub fn forked_participant_store(&self) -> Option<&Arc<dyn ForkedParticipantStore>> {
        self.forked_participants.as_ref()
    }

    /// Return whether the structural event log is empty.
    pub async fn is_event_log_empty(&self) -> Result<bool, crate::store::MobStoreError> {
        Ok(self.events.latest_cursor().await? == 0)
    }

    /// Read the definition that the persistent mob storage was created for.
    ///
    /// This is a pre-actuation read: it does not build a runtime, resume
    /// members, or emit events. `MobCreated` establishes epoch 1 and each
    /// `MobDefinitionUpdated` strict successor replaces that authority; reset
    /// does not manufacture another creation event.
    pub async fn created_definition(
        &self,
    ) -> Result<Option<MobDefinition>, crate::store::MobStoreError> {
        Ok(self
            .created_definition_snapshot()
            .await?
            .map(|snapshot| snapshot.definition))
    }

    /// Read the canonical definition together with its sealed resume precondition.
    pub async fn created_definition_snapshot(
        &self,
    ) -> Result<Option<MobDefinitionSnapshot>, crate::store::MobStoreError> {
        let events = self.events.replay_all().await?;
        Self::current_definition_snapshot_for_log(&events)
    }

    /// Replay structural events together with their authoritative definition.
    ///
    /// Runtime resume and the public pre-actuation read share this exact
    /// selection path so neither can reinterpret the event log independently.
    pub(crate) async fn replay_with_created_definition(
        &self,
    ) -> Result<(Vec<MobEvent>, Option<MobDefinition>), crate::store::MobStoreError> {
        let events = self.events.replay_all().await?;
        let definition =
            Self::current_definition_authority_for_log(&events)?.map(|(definition, _)| definition);
        Ok((events, definition))
    }

    /// Inspect definition/spec coherence without mutating either store.
    pub async fn definition_projection_health(
        &self,
    ) -> Result<Option<MobDefinitionProjectionHealth>, crate::store::MobStoreError> {
        let events = self.events.replay_all().await?;
        let Some((definition, authority_epoch)) =
            Self::current_definition_authority_for_log(&events)?
        else {
            return Ok(None);
        };
        let health = match self.specs.get_spec(&definition.id).await? {
            None => MobDefinitionProjectionHealth::ProjectionMissing { authority_epoch },
            Some((projected, projection_revision)) if projection_revision < authority_epoch => {
                MobDefinitionProjectionHealth::ProjectionStale {
                    authority_epoch,
                    projection_revision,
                }
            }
            Some((_projected, projection_revision)) if projection_revision > authority_epoch => {
                MobDefinitionProjectionHealth::Diverged {
                    authority_epoch,
                    projection_revision,
                    kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
                }
            }
            Some((projected, projection_revision)) if projected != definition => {
                MobDefinitionProjectionHealth::Diverged {
                    authority_epoch,
                    projection_revision,
                    kind: MobDefinitionProjectionMismatchKind::DefinitionMismatch,
                }
            }
            Some((_, projection_revision)) => MobDefinitionProjectionHealth::Healthy {
                authority_epoch,
                projection_revision,
            },
        };
        Ok(Some(health))
    }

    /// Advance a durable, inactive-process mob definition by one generated-machine epoch.
    ///
    /// Call this before constructing/resuming a `MobHandle`. The event log is
    /// canonical and commits first under an atomic epoch CAS. A crash before
    /// the spec projection write is repaired on exact retry or resume; a
    /// projection that claims the same/newer epoch with different content is
    /// refused.
    pub async fn update_definition(
        &self,
        expected_revision: u64,
        definition: MobDefinition,
    ) -> Result<MobDefinitionRevision, MobError> {
        let mut diagnostics = crate::validate::validate_definition(&definition);
        diagnostics.extend(crate::spec::SpecValidator::validate(&definition));
        let (errors, warnings) = crate::validate::partition_diagnostics(diagnostics);
        if !errors.is_empty() {
            return Err(MobError::DefinitionError(errors));
        }
        for warning in warnings {
            tracing::warn!(
                code = %warning.code,
                location = ?warning.location,
                "{}",
                warning.message
            );
        }

        let _definition_update_guard = self.events.acquire_definition_update_claim().await?;
        let events = self.events.replay_all().await?;
        let Some((current_definition, current_epoch)) =
            Self::current_definition_authority_for_log(&events)?
        else {
            return Err(MobError::MobNotFound(definition.id));
        };
        if current_definition.id != definition.id {
            return Err(MobError::MobNotFound(definition.id));
        }

        let next_epoch = expected_revision.checked_add(1).ok_or_else(|| {
            MobError::Internal(format!(
                "mob '{}' definition epoch is exhausted at {expected_revision}",
                definition.id
            ))
        })?;
        let exact_replay_cursor = events.iter().rev().find_map(|event| match &event.kind {
            MobEventKind::MobDefinitionUpdated {
                epoch,
                definition: stored,
            } if *epoch == current_epoch && **stored == definition => Some(event.cursor),
            _ => None,
        });
        if current_epoch == next_epoch
            && current_definition == definition
            && exact_replay_cursor.is_some()
        {
            let projection_revision = self
                .converge_definition_projection(&definition, current_epoch)
                .await?;
            let event_cursor = exact_replay_cursor.ok_or_else(|| {
                MobError::Internal(format!(
                    "mob '{}' exact definition epoch replay has no matching event",
                    definition.id
                ))
            })?;
            return Ok(MobDefinitionRevision {
                epoch: current_epoch,
                projection_revision,
                event_cursor,
            });
        }
        if current_epoch != expected_revision {
            return Err(MobError::SpecRevisionConflict {
                mob_id: definition.id,
                expected: Some(expected_revision),
                actual: current_epoch,
            });
        }
        let projection_ahead_repair = matches!(
            self.definition_projection_health().await?,
            Some(MobDefinitionProjectionHealth::Diverged {
                authority_epoch,
                projection_revision,
                kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
            }) if authority_epoch == current_epoch
                && projection_revision == next_epoch
                && self.definition_projection_composition
                    == DefinitionProjectionComposition::AtomicSqlite
        );
        if !projection_ahead_repair {
            self.converge_definition_projection(&current_definition, current_epoch)
                .await?;
        }

        let mut authority =
            crate::runtime::recover_definition_epoch_authority(&events, &current_definition)?;
        let transition = crate::machines::mob_machine::MobMachineMutator::apply(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::AdvanceDefinitionEpoch {
                expected_epoch: current_epoch,
                next_epoch,
            },
        )
        .map_err(|error| MobError::MobMachineRejected {
            context: "advance definition epoch",
            reason: error.to_string(),
        })?;
        let event_head_cursor = events
            .iter()
            .filter(|event| event.mob_id == definition.id)
            .map(|event| event.cursor)
            .max()
            .unwrap_or(0);
        let persistence_authority = MobDefinitionEpochPersistenceAuthority::from_transition(
            definition.id.clone(),
            event_head_cursor,
            &transition,
        )?;
        let outcome = self
            .events
            .append_definition_epoch(
                NewMobEvent {
                    mob_id: definition.id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MobDefinitionUpdated {
                        epoch: next_epoch,
                        definition: Box::new(definition.clone()),
                    },
                },
                &persistence_authority,
            )
            .await?;
        let event_cursor = match outcome {
            MobDefinitionEpochAppendOutcome::Appended(event)
            | MobDefinitionEpochAppendOutcome::AlreadyCommitted(event) => event.cursor,
        };
        let projection_revision = self
            .converge_definition_projection(&definition, next_epoch)
            .await?;
        Ok(MobDefinitionRevision {
            epoch: next_epoch,
            projection_revision,
            event_cursor,
        })
    }

    pub(crate) async fn converge_definition_projection(
        &self,
        definition: &MobDefinition,
        authority_epoch: u64,
    ) -> Result<u64, MobError> {
        loop {
            match self.specs.get_spec(&definition.id).await? {
                None => {
                    let revision = self
                        .specs
                        .put_spec(&definition.id, definition, None)
                        .await?;
                    if revision > authority_epoch {
                        return Err(Self::definition_projection_mismatch(
                            definition.id.clone(),
                            authority_epoch,
                            revision,
                            MobDefinitionProjectionMismatchKind::ProjectionAhead,
                        ));
                    }
                }
                Some((projected, projection_revision)) if projection_revision < authority_epoch => {
                    let revision = self
                        .specs
                        .put_spec(&definition.id, definition, Some(projection_revision))
                        .await?;
                    if revision > authority_epoch {
                        return Err(Self::definition_projection_mismatch(
                            definition.id.clone(),
                            authority_epoch,
                            revision,
                            MobDefinitionProjectionMismatchKind::ProjectionAhead,
                        ));
                    }
                }
                Some((_projected, projection_revision))
                    if projection_revision > authority_epoch =>
                {
                    return Err(Self::definition_projection_mismatch(
                        definition.id.clone(),
                        authority_epoch,
                        projection_revision,
                        MobDefinitionProjectionMismatchKind::ProjectionAhead,
                    ));
                }
                Some((projected, projection_revision)) if projected != *definition => {
                    return Err(Self::definition_projection_mismatch(
                        definition.id.clone(),
                        authority_epoch,
                        projection_revision,
                        MobDefinitionProjectionMismatchKind::DefinitionMismatch,
                    ));
                }
                Some((_, projection_revision)) => return Ok(projection_revision),
            }
        }
    }

    /// Create a storage bundle backed by a single SQLite database file.
    ///
    /// Uses WAL mode — no exclusive file lock is held, so the same path
    /// can be reopened after drop within the same process.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn persistent(path: impl AsRef<Path>) -> Result<Self, crate::MobError> {
        let stores = SqliteMobStores::open(path)?;
        let identity_status_projection_order =
            crate::runtime::IdentityStatusProjectionOrder::for_backend_scope(
                stores.identity_status_projection_scope(),
            );
        let identity = stores.identity_store();
        let identity_member: Arc<dyn MobIdentityMemberStore> = Arc::new(identity.clone());
        Ok(Self {
            events: Arc::new(stores.event_store()),
            runs: authority_validating_mob_run_store(Arc::new(stores.run_store())),
            specs: Arc::new(stores.spec_store()),
            definition_projection_composition: DefinitionProjectionComposition::AtomicSqlite,
            runtime_metadata: Arc::new(stores.runtime_metadata_store()),
            identity: Arc::new(identity),
            identity_member: Some(identity_member),
            identity_status: Arc::new(stores.identity_status_store()),
            identity_status_projection_order,
            realm_profiles: Some(Arc::new(stores.realm_profile_store())),
            forked_participants: Some(Arc::new(stores.forked_participant_store())),
        })
    }

    fn current_definition_authority_for_log(
        events: &[MobEvent],
    ) -> Result<Option<(MobDefinition, u64)>, crate::store::MobStoreError> {
        let Some(mob_id) = events.iter().find_map(|event| match &event.kind {
            MobEventKind::MobCreated { .. } => Some(event.mob_id.clone()),
            _ => None,
        }) else {
            return Ok(None);
        };
        current_definition_authority(events, &mob_id)
    }

    pub(crate) fn current_definition_snapshot_for_log(
        events: &[MobEvent],
    ) -> Result<Option<MobDefinitionSnapshot>, crate::store::MobStoreError> {
        let Some((definition, epoch)) = Self::current_definition_authority_for_log(events)? else {
            return Ok(None);
        };
        let event_cursor = events
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                MobEventKind::MobDefinitionUpdated {
                    epoch: event_epoch,
                    definition: event_definition,
                } if *event_epoch == epoch && **event_definition == definition => {
                    Some(event.cursor)
                }
                MobEventKind::MobCreated {
                    definition: event_definition,
                } if epoch == 1 && **event_definition == definition => Some(event.cursor),
                _ => None,
            })
            .ok_or_else(|| {
                crate::store::MobStoreError::Internal(format!(
                    "mob '{}' definition authority has no matching event cursor",
                    definition.id
                ))
            })?;
        Ok(Some(MobDefinitionSnapshot {
            definition,
            epoch,
            event_cursor,
        }))
    }

    fn definition_projection_mismatch(
        mob_id: crate::MobId,
        authority_epoch: u64,
        projection_revision: u64,
        kind: MobDefinitionProjectionMismatchKind,
    ) -> MobError {
        MobError::MobDefinitionProjectionMismatch {
            mob_id,
            authority_epoch,
            projection_revision,
            kind,
        }
    }
}

impl std::fmt::Debug for MobStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobStorage")
            .field("events", &"<dyn MobEventStore>")
            .field("runs", &"<dyn MobRunStore>")
            .field("specs", &"<dyn MobSpecStore>")
            .field("runtime_metadata", &"<dyn MobRuntimeMetadataStore>")
            .field("identity", &"<dyn MobIdentityStore>")
            .field(
                "identity_member",
                &self
                    .identity_member
                    .as_ref()
                    .map(|_| "<dyn MobIdentityMemberStore>"),
            )
            .field("identity_status", &"<dyn MobIdentityStatusStore>")
            .field(
                "forked_participants",
                &self
                    .forked_participants
                    .as_ref()
                    .map(|_| "<dyn ForkedParticipantStore>"),
            )
            .field(
                "realm_profiles",
                &self
                    .realm_profiles
                    .as_ref()
                    .map(|_| "<dyn RealmProfileStore>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{MobEventKind, NewMobEvent};
    use crate::ids::{FlowId, FrameId, LoopId, LoopInstanceId, MobId, RunId, StepId};
    use crate::run::{
        FailureLedgerEntry, FrameSnapshot, LoopIterationLedgerEntry, LoopSnapshot, MobRun,
        MobRunProvenanceAuthority, MobRunStatus, StepLedgerEntry,
    };
    use crate::store::MobStoreError;
    use chrono::Utc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FailOnceSpecStore {
        inner: Arc<dyn MobSpecStore>,
        fail_next_put: AtomicBool,
    }

    impl FailOnceSpecStore {
        fn new(inner: Arc<dyn MobSpecStore>) -> Self {
            Self {
                inner,
                fail_next_put: AtomicBool::new(false),
            }
        }

        fn fail_next_put(&self) {
            self.fail_next_put.store(true, Ordering::Relaxed);
        }
    }

    struct RacingSpecStore {
        inner: Arc<dyn MobSpecStore>,
        replacement: Mutex<Option<MobDefinition>>,
    }

    impl RacingSpecStore {
        fn new(inner: Arc<dyn MobSpecStore>) -> Self {
            Self {
                inner,
                replacement: Mutex::new(None),
            }
        }

        fn race_next_read_with(&self, definition: MobDefinition) {
            *self.replacement.lock().unwrap() = Some(definition);
        }
    }

    #[async_trait::async_trait]
    impl MobSpecStore for RacingSpecStore {
        async fn put_spec(
            &self,
            mob_id: &MobId,
            definition: &MobDefinition,
            revision: Option<u64>,
        ) -> Result<u64, MobStoreError> {
            self.inner.put_spec(mob_id, definition, revision).await
        }

        async fn get_spec(
            &self,
            mob_id: &MobId,
        ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
            let observed = self.inner.get_spec(mob_id).await?;
            let replacement = self.replacement.lock().unwrap().take();
            if let Some(replacement) = replacement {
                let revision = observed.as_ref().map(|(_, revision)| *revision);
                self.inner.put_spec(mob_id, &replacement, revision).await?;
            }
            Ok(observed)
        }

        async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
            self.inner.list_specs().await
        }

        async fn delete_spec(
            &self,
            mob_id: &MobId,
            revision: Option<u64>,
        ) -> Result<bool, MobStoreError> {
            self.inner.delete_spec(mob_id, revision).await
        }
    }

    #[async_trait::async_trait]
    impl MobSpecStore for FailOnceSpecStore {
        async fn put_spec(
            &self,
            mob_id: &MobId,
            definition: &MobDefinition,
            revision: Option<u64>,
        ) -> Result<u64, MobStoreError> {
            if self.fail_next_put.swap(false, Ordering::Relaxed) {
                return Err(MobStoreError::WriteFailed(
                    "forced projection write failure".to_string(),
                ));
            }
            self.inner.put_spec(mob_id, definition, revision).await
        }

        async fn get_spec(
            &self,
            mob_id: &MobId,
        ) -> Result<Option<(MobDefinition, u64)>, MobStoreError> {
            self.inner.get_spec(mob_id).await
        }

        async fn list_specs(&self) -> Result<Vec<MobId>, MobStoreError> {
            self.inner.list_specs().await
        }

        async fn delete_spec(
            &self,
            mob_id: &MobId,
            revision: Option<u64>,
        ) -> Result<bool, MobStoreError> {
            self.inner.delete_spec(mob_id, revision).await
        }
    }

    fn valid_definition(id: &str) -> MobDefinition {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "profiles": {
                "worker": {
                    "model": "gpt-5.5"
                }
            }
        }))
        .unwrap()
    }

    struct ForgedRunStore {
        run: Mutex<Option<MobRun>>,
    }

    impl ForgedRunStore {
        fn new(run: Option<MobRun>) -> Self {
            Self {
                run: Mutex::new(run),
            }
        }
    }

    #[async_trait::async_trait]
    impl MobRunStore for ForgedRunStore {
        async fn create_run(&self, run: MobRun) -> Result<(), MobStoreError> {
            *self.run.lock().expect("forged run mutex") = Some(run);
            Ok(())
        }

        async fn get_run(&self, run_id: &RunId) -> Result<Option<MobRun>, MobStoreError> {
            Ok(self
                .run
                .lock()
                .expect("forged run mutex")
                .as_ref()
                .filter(|run| &run.run_id == run_id)
                .cloned())
        }

        async fn list_runs(
            &self,
            mob_id: &MobId,
            flow_id: Option<&FlowId>,
        ) -> Result<Vec<MobRun>, MobStoreError> {
            Ok(self
                .run
                .lock()
                .expect("forged run mutex")
                .as_ref()
                .filter(|run| &run.mob_id == mob_id)
                .filter(|run| flow_id.is_none_or(|flow_id| &run.flow_id == flow_id))
                .cloned()
                .into_iter()
                .collect())
        }

        async fn cas_flow_state_with_authority(
            &self,
            _run_id: &RunId,
            _expected: &crate::run::flow_run::State,
            _next: &crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn cas_run_snapshot_with_authority(
            &self,
            _run_id: &RunId,
            _expected_status: MobRunStatus,
            _expected_flow_state: &crate::run::flow_run::State,
            _next_status: MobRunStatus,
            _next_flow_state: &crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_step_entry_with_authority(
            &self,
            _run_id: &RunId,
            _entry: StepLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_step_entry_if_absent_with_authority(
            &self,
            _run_id: &RunId,
            _entry: StepLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn append_failure_entry_with_authority(
            &self,
            _run_id: &RunId,
            _entry: FailureLedgerEntry,
            _authority: MobRunProvenanceAuthority,
        ) -> Result<(), MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        async fn cas_frame_state_with_authority(
            &self,
            _run_id: &RunId,
            _frame_id: &FrameId,
            _expected: Option<&FrameSnapshot>,
            _next: FrameSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_node_slot_with_authority(
            &self,
            _run_id: &RunId,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_step_and_record_output_with_authority(
            &self,
            _run_id: &RunId,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _step_output_key: String,
            _step_output: serde_json::Value,
            _loop_context: Option<(&LoopId, u64)>,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_start_loop_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _initial_loop: LoopSnapshot,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_loop_request_body_frame_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_grant_body_frame_start_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _initial_frame: FrameSnapshot,
            _ledger_entry: LoopIterationLedgerEntry,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_body_frame_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }

        #[allow(clippy::too_many_arguments)]
        async fn cas_complete_loop_with_authority(
            &self,
            _run_id: &RunId,
            _loop_instance_id: &LoopInstanceId,
            _expected_loop: &LoopSnapshot,
            _next_loop: LoopSnapshot,
            _frame_id: &FrameId,
            _expected_frame: &FrameSnapshot,
            _next_frame: FrameSnapshot,
            _expected_run_state: &crate::run::flow_run::State,
            _next_run_state: crate::run::flow_run::State,
            _authority_inputs: Vec<crate::machines::mob_machine::MobMachineInput>,
        ) -> Result<bool, MobStoreError> {
            Err(MobStoreError::Internal(
                "not implemented in forged store".into(),
            ))
        }
    }

    fn forged_status_run() -> MobRun {
        let mut run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
        run.status = MobRunStatus::Completed;
        run
    }

    #[tokio::test]
    async fn test_in_memory_storage_creates_working_stores() {
        let storage = MobStorage::in_memory();

        // Event store works
        let event = NewMobEvent {
            mob_id: MobId::from("test"),
            timestamp: None,
            kind: MobEventKind::MobCompleted,
        };
        let stored = storage.events.append(event).await.unwrap();
        assert_eq!(stored.cursor, 1);

        let all = storage.events.replay_all().await.unwrap();
        assert_eq!(all.len(), 1);

        let mob_id = MobId::from("test");
        let identity = crate::AgentIdentity::from("member-a");
        assert!(matches!(
            storage
                .identity
                .observe_identity_intent(&mob_id, &identity)
                .await
                .unwrap(),
            crate::IdentityStoredObservation::Missing
        ));
        assert!(matches!(
            storage
                .identity
                .claim_or_renew_identity_lease(
                    &mob_id,
                    &identity,
                    "controller",
                    "incarnation-a",
                    1_000,
                )
                .await
                .unwrap(),
            crate::IdentityLeaseClaimOutcome::Acquired(_)
        ));

        let status = crate::IdentityConvergenceStatus {
            identity: identity.clone(),
            intent_revision: None,
            active_intent_revision: None,
            lease_epoch: Some(1),
            decision: Some(crate::IdentityReconcileDecision::AcquireLease),
            observed_at_ms: 1,
            detail: None,
        };
        storage
            .identity_status
            .replace_identity_convergence_status(&mob_id, &status)
            .await
            .unwrap();
        assert_eq!(
            storage
                .identity_status
                .load_identity_convergence_status(&mob_id, &identity)
                .await
                .unwrap(),
            crate::IdentityStoredObservation::Valid(status)
        );
    }

    #[tokio::test]
    async fn created_definition_reads_latest_epoch_without_mutating_storage() {
        let storage = MobStorage::in_memory();
        assert_eq!(storage.created_definition().await.unwrap(), None);

        let first = valid_definition("created-definition-test");
        storage
            .events
            .append(NewMobEvent {
                mob_id: first.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(first.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&first.id, &first, None)
            .await
            .unwrap();
        let mut latest = first.clone();
        latest.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        storage.update_definition(1, latest.clone()).await.unwrap();

        let cursor_before = storage.events.latest_cursor().await.unwrap();
        assert_eq!(storage.created_definition().await.unwrap(), Some(latest));
        assert_eq!(storage.events.latest_cursor().await.unwrap(), cursor_before);
    }

    #[tokio::test]
    async fn definition_epoch_stale_cas_refuses_without_appending() {
        let storage = MobStorage::in_memory();
        let definition = valid_definition("definition-cas");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let before = storage.events.latest_cursor().await.unwrap();

        let error = storage
            .update_definition(0, definition)
            .await
            .expect_err("stale definition revision must lose");
        assert!(matches!(
            error,
            MobError::SpecRevisionConflict {
                expected: Some(0),
                actual: 1,
                ..
            }
        ));
        assert_eq!(storage.events.latest_cursor().await.unwrap(), before);
    }

    #[tokio::test]
    async fn definition_epoch_witness_rejects_a_changed_event_head() {
        let storage = MobStorage::in_memory();
        let definition = valid_definition("definition-head-race");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let events = storage.events.replay_all().await.unwrap();
        let mut authority =
            crate::runtime::recover_definition_epoch_authority(&events, &definition).unwrap();
        let transition = crate::machines::mob_machine::MobMachineMutator::apply(
            &mut authority,
            crate::machines::mob_machine::MobMachineInput::AdvanceDefinitionEpoch {
                expected_epoch: 1,
                next_epoch: 2,
            },
        )
        .unwrap();
        let witness = MobDefinitionEpochPersistenceAuthority::from_transition(
            definition.id.clone(),
            1,
            &transition,
        )
        .unwrap();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        let mut updated = definition.clone();
        updated.image_generation_provider = Some(meerkat_core::Provider::OpenAI);

        let error = storage
            .events
            .append_definition_epoch(
                NewMobEvent {
                    mob_id: definition.id.clone(),
                    timestamp: None,
                    kind: MobEventKind::MobDefinitionUpdated {
                        epoch: 2,
                        definition: Box::new(updated),
                    },
                },
                &witness,
            )
            .await
            .expect_err("intervening lifecycle event invalidates the machine witness");
        assert!(matches!(
            error,
            MobStoreError::DefinitionEpochEventHeadConflict {
                expected: 1,
                actual: 2,
                ..
            }
        ));
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn definition_update_waits_for_verified_resume_claim() {
        let storage = MobStorage::in_memory();
        let definition = valid_definition("definition-resume-claim");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let resume_claim = storage.events.definition_resume_gate().read_owned().await;
        let mut updated = definition;
        updated.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let update_storage = storage.clone();
        let mut update =
            tokio::spawn(async move { update_storage.update_definition(1, updated).await });

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut update)
                .await
                .is_err(),
            "definition update must not cross a verified resume claim"
        );
        drop(resume_claim);
        assert_eq!(update.await.unwrap().unwrap().epoch, 2);
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn sqlite_verified_resume_claim_holds_cross_process_shared_fence() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mob.db");
        let storage = MobStorage::persistent(&database).unwrap();
        let claim = storage
            .events
            .acquire_definition_resume_claim()
            .await
            .unwrap();

        assert!(
            meerkat_sqlite::ExclusiveFence::try_acquire(&database)
                .unwrap()
                .is_none(),
            "verified resume must hold the database's cross-process shared fence"
        );
        drop(claim);
        assert!(
            meerkat_sqlite::ExclusiveFence::try_acquire(&database)
                .unwrap()
                .is_some(),
            "dropping the resume claim must release the shared fence"
        );
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn definition_epoch_survives_reset_and_advances_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mob.db");
        let storage = MobStorage::persistent(&database).unwrap();
        let definition = valid_definition("definition-reset");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let mut epoch_two = definition.clone();
        epoch_two.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        storage
            .update_definition(1, epoch_two.clone())
            .await
            .unwrap();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobReset,
            })
            .await
            .unwrap();
        drop(storage);
        let storage = MobStorage::persistent(&database).unwrap();
        let mut epoch_three = epoch_two;
        epoch_three.image_generation_provider = Some(meerkat_core::Provider::Gemini);

        let committed = storage
            .update_definition(2, epoch_three.clone())
            .await
            .expect("reset preserves the durable definition epoch");
        assert_eq!(committed.epoch, 3);
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(epoch_three)
        );
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn multiple_definition_updates_recover_for_next_cas() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mob.db");
        let storage = MobStorage::persistent(&database).unwrap();
        let definition = valid_definition("definition-multiple");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let mut epoch_two = definition;
        epoch_two.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        storage
            .update_definition(1, epoch_two.clone())
            .await
            .unwrap();
        let mut epoch_three = epoch_two;
        epoch_three.image_generation_provider = Some(meerkat_core::Provider::Gemini);
        storage
            .update_definition(2, epoch_three.clone())
            .await
            .unwrap();
        drop(storage);

        let storage = MobStorage::persistent(&database).unwrap();
        let mut epoch_four = epoch_three;
        epoch_four.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let committed = storage
            .update_definition(3, epoch_four.clone())
            .await
            .expect("all historical epochs recover before the next CAS");
        assert_eq!(committed.epoch, 4);
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(epoch_four)
        );
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn raw_definition_epoch_event_append_is_sealed_for_memory_and_sqlite() {
        async fn assert_sealed(events: &dyn MobEventStore, definition: &MobDefinition) {
            let event = || NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobDefinitionUpdated {
                    epoch: 2,
                    definition: Box::new(definition.clone()),
                },
            };
            assert!(matches!(
                events.append(event()).await,
                Err(MobStoreError::DefinitionEpochAuthorityRequired)
            ));
            assert!(matches!(
                events.append_batch(vec![event()]).await,
                Err(MobStoreError::DefinitionEpochAuthorityRequired)
            ));
            let created = || NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            };
            events.append(created()).await.unwrap();
            assert!(matches!(
                events.append(created()).await,
                Err(MobStoreError::MobDefinitionAlreadyCreated { .. })
            ));
            assert!(matches!(
                events.append_batch(vec![created()]).await,
                Err(MobStoreError::MobDefinitionAlreadyCreated { .. })
            ));
            let other = valid_definition("definition-sealed-other");
            assert!(matches!(
                events
                    .append(NewMobEvent {
                        mob_id: other.id.clone(),
                        timestamp: None,
                        kind: MobEventKind::MobCreated {
                            definition: Box::new(other),
                        },
                    })
                    .await,
                Err(MobStoreError::MobDefinitionAlreadyCreated { .. })
            ));
        }

        let definition = valid_definition("definition-sealed");
        let memory = MobStorage::in_memory();
        assert_sealed(memory.events.as_ref(), &definition).await;

        let directory = tempfile::tempdir().unwrap();
        let sqlite = MobStorage::persistent(directory.path().join("mob.db")).unwrap();
        assert_sealed(sqlite.events.as_ref(), &definition).await;

        let mixed = vec![
            NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            },
            NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition),
                },
            },
        ];
        assert!(matches!(
            crate::store::validate_initial_mob_created_events(&[], &mixed),
            Err(MobStoreError::MobDefinitionAlreadyCreated { .. })
        ));
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn sqlite_transactional_projection_race_preserves_typed_mismatch() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mob.db");
        let mut storage = MobStorage::persistent(&database).unwrap();
        let definition = valid_definition("definition-race");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();

        let racing_specs = Arc::new(RacingSpecStore::new(storage.specs.clone()));
        storage.specs = racing_specs.clone();
        let mut updated = definition.clone();
        updated.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let mut unauthenticated_projection = definition.clone();
        unauthenticated_projection.image_generation_provider = Some(meerkat_core::Provider::Gemini);
        racing_specs.race_next_read_with(unauthenticated_projection);

        let error = storage
            .update_definition(1, updated)
            .await
            .expect_err("transactional projection race must fail typed");
        assert!(matches!(
            error,
            MobError::MobDefinitionProjectionMismatch {
                authority_epoch: 1,
                projection_revision: 2,
                kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
                ..
            }
        ));
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(definition),
            "racing projection must never become event authority"
        );
    }

    #[tokio::test]
    async fn event_first_projection_failure_converges_on_exact_replay() {
        let mut storage = MobStorage::in_memory();
        let faulting_specs = Arc::new(FailOnceSpecStore::new(storage.specs.clone()));
        storage.specs = faulting_specs.clone();
        let definition = valid_definition("definition-convergence");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let mut updated = definition.clone();
        updated.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        faulting_specs.fail_next_put();

        storage
            .update_definition(1, updated.clone())
            .await
            .expect_err("projection write fails after authority append");
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(updated.clone()),
            "the event-log authority must survive the lost projection acknowledgement"
        );
        assert!(matches!(
            storage.definition_projection_health().await.unwrap(),
            Some(MobDefinitionProjectionHealth::ProjectionStale {
                authority_epoch: 2,
                projection_revision: 1,
            })
        ));
        let cursor_after_authority = storage.events.latest_cursor().await.unwrap();

        let boot_revision = storage
            .converge_definition_projection(&updated, 2)
            .await
            .expect("boot convergence repairs the stale projection");
        assert_eq!(boot_revision, 2);
        let replay = storage
            .update_definition(1, updated.clone())
            .await
            .expect("exact replay converges projection");
        assert_eq!(replay.event_cursor, cursor_after_authority);
        assert_eq!(
            storage.events.latest_cursor().await.unwrap(),
            cursor_after_authority
        );
        assert!(matches!(
            storage.definition_projection_health().await.unwrap(),
            Some(MobDefinitionProjectionHealth::Healthy {
                authority_epoch: 2,
                projection_revision: 2,
            })
        ));
        assert_eq!(
            storage
                .events
                .prune(Utc::now() + chrono::Duration::days(1))
                .await
                .unwrap(),
            0,
            "definition authority events are permanent event-reader anchors"
        );
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(updated),
            "event readers retain the latest definition after pruning"
        );
    }

    #[tokio::test]
    async fn projection_first_crash_state_is_typed_divergence_and_never_wins() {
        let storage = MobStorage::in_memory();
        let definition = valid_definition("definition-projection-first");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let mut projected_only = definition.clone();
        projected_only.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        storage
            .specs
            .put_spec(&definition.id, &projected_only, Some(1))
            .await
            .unwrap();

        assert!(matches!(
            storage.definition_projection_health().await.unwrap(),
            Some(MobDefinitionProjectionHealth::Diverged {
                authority_epoch: 1,
                projection_revision: 2,
                kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
            })
        ));
        let error = storage
            .update_definition(1, projected_only)
            .await
            .expect_err("unauthenticated projection cannot become authority");
        assert!(matches!(
            error,
            MobError::MobDefinitionProjectionMismatch {
                authority_epoch: 1,
                projection_revision: 2,
                kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
                ..
            }
        ));
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(definition),
            "projection-first state must not rewrite event authority"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn assert_split_sqlite_projection_ahead_refused(projected_matches_declared: bool) {
        let directory = tempfile::tempdir().unwrap();
        let stores = SqliteMobStores::open(directory.path().join("mob.db")).unwrap();
        let sqlite_events = stores.event_store();
        let sqlite_specs = stores.spec_store();
        let definition = valid_definition("definition-split-projection");
        sqlite_events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        sqlite_specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();

        let mut declared = definition.clone();
        declared.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let mut projected = declared.clone();
        if !projected_matches_declared {
            projected.image_generation_provider = Some(meerkat_core::Provider::Gemini);
        }
        let external_specs = Arc::new(InMemoryMobSpecStore::new());
        external_specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        external_specs
            .put_spec(&definition.id, &projected, Some(1))
            .await
            .unwrap();

        let storage = MobStorage::custom(
            Arc::new(sqlite_events),
            Arc::new(InMemoryMobRunStore::new()),
            external_specs.clone(),
            Arc::new(InMemoryMobIdentityStore::new()),
            Arc::new(InMemoryMobIdentityStatusStore::new()),
        );
        let cursor_before = storage.events.latest_cursor().await.unwrap();
        let event_count_before = storage.events.replay_all().await.unwrap().len();

        let error = storage
            .update_definition(1, declared)
            .await
            .expect_err("split store composition must never adopt projection residue");
        assert!(matches!(
            error,
            MobError::MobDefinitionProjectionMismatch {
                authority_epoch: 1,
                projection_revision: 2,
                kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
                ..
            }
        ));
        assert_eq!(storage.events.latest_cursor().await.unwrap(), cursor_before);
        assert_eq!(
            storage.events.replay_all().await.unwrap().len(),
            event_count_before
        );
        assert_eq!(
            storage.created_definition().await.unwrap(),
            Some(definition.clone())
        );
        assert_eq!(
            sqlite_specs.get_spec(&definition.id).await.unwrap(),
            Some((definition.clone(), 1))
        );
        assert_eq!(
            external_specs.get_spec(&definition.id).await.unwrap(),
            Some((projected, 2))
        );
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn split_sqlite_event_and_custom_spec_refuses_exact_projection_ahead_residue() {
        assert_split_sqlite_projection_ahead_refused(true).await;
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn split_sqlite_event_and_custom_spec_refuses_mismatched_projection_ahead_residue() {
        assert_split_sqlite_projection_ahead_refused(false).await;
    }

    #[tokio::test]
    #[cfg(not(target_arch = "wasm32"))]
    async fn sqlite_definition_epoch_transaction_rolls_back_both_crash_boundaries() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("mob.db");
        let storage = MobStorage::persistent(&database).unwrap();
        let definition = valid_definition("definition-atomic");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let mut updated = definition.clone();
        updated.image_generation_provider = Some(meerkat_core::Provider::OpenAI);
        let cursor_before = storage.events.latest_cursor().await.unwrap();

        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_definition_projection
                     BEFORE UPDATE OF spec_json ON mob_specs
                     BEGIN
                       SELECT RAISE(ABORT, 'forced projection failure');
                     END;",
                )
                .unwrap();
        }
        storage
            .update_definition(1, updated.clone())
            .await
            .expect_err("projection statement failure rolls back the transaction");
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch("DROP TRIGGER fail_definition_projection;")
                .unwrap();
        }
        assert_eq!(storage.events.latest_cursor().await.unwrap(), cursor_before);
        assert!(matches!(
            storage.definition_projection_health().await.unwrap(),
            Some(MobDefinitionProjectionHealth::Healthy {
                authority_epoch: 1,
                projection_revision: 1,
            })
        ));

        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_definition_authority
                     BEFORE INSERT ON mob_events
                     BEGIN
                       SELECT RAISE(ABORT, 'forced authority failure');
                     END;",
                )
                .unwrap();
        }
        storage
            .update_definition(1, updated.clone())
            .await
            .expect_err("authority statement failure rolls back the prior projection statement");
        {
            let connection = rusqlite::Connection::open(&database).unwrap();
            connection
                .execute_batch("DROP TRIGGER fail_definition_authority;")
                .unwrap();
        }
        assert_eq!(storage.events.latest_cursor().await.unwrap(), cursor_before);
        let (projected, revision) = storage
            .specs
            .get_spec(&definition.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(projected, definition);
        assert_eq!(revision, 1);

        let committed = storage
            .update_definition(1, updated)
            .await
            .expect("retry commits both sides");
        assert_eq!(committed.epoch, 2);
        assert_eq!(committed.projection_revision, 2);
    }

    #[tokio::test]
    async fn completed_mob_definition_update_fails_closed_without_terminal_fabrication() {
        let storage = MobStorage::in_memory();
        let definition = valid_definition("definition-completed");
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCreated {
                    definition: Box::new(definition.clone()),
                },
            })
            .await
            .unwrap();
        storage
            .events
            .append(NewMobEvent {
                mob_id: definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::MobCompleted,
            })
            .await
            .unwrap();
        storage
            .specs
            .put_spec(&definition.id, &definition, None)
            .await
            .unwrap();
        let before = storage.events.latest_cursor().await.unwrap();

        let error = storage
            .update_definition(1, definition)
            .await
            .expect_err("completed lifecycle must reject definition advance");
        assert!(matches!(error, MobError::MobMachineRejected { .. }));
        assert_eq!(storage.events.latest_cursor().await.unwrap(), before);
    }

    #[tokio::test]
    async fn test_in_memory_flow_stores_create_working_run_and_spec_stores() {
        let (runs, specs) = MobStorage::in_memory_flow_stores();
        let run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
        runs.create_run(run.clone()).await.unwrap();
        assert!(runs.get_run(&run.run_id).await.unwrap().is_some());

        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let revision = specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);
    }

    #[tokio::test]
    async fn custom_run_store_rejects_forged_lifecycle_projection() {
        let forged = forged_status_run();
        let storage = MobStorage::custom(
            Arc::new(InMemoryMobEventStore::new()),
            Arc::new(ForgedRunStore::new(Some(forged.clone()))),
            Arc::new(InMemoryMobSpecStore::new()),
            Arc::new(InMemoryMobIdentityStore::new()),
            Arc::new(InMemoryMobIdentityStatusStore::new()),
        );

        let read_error = storage
            .runs
            .get_run(&forged.run_id)
            .await
            .expect_err("custom store read must reject forged run projection");
        assert!(
            read_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom read error: {read_error}"
        );

        let list_error = storage
            .runs
            .list_runs(&forged.mob_id, None)
            .await
            .expect_err("custom store list must reject forged run projection");
        assert!(
            list_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom list error: {list_error}"
        );

        let write_error = storage
            .runs
            .create_run(forged)
            .await
            .expect_err("custom store create must reject forged run projection");
        assert!(
            write_error
                .to_string()
                .contains("not authorized by MobMachine"),
            "unexpected custom create error: {write_error}"
        );
    }

    #[tokio::test]
    async fn test_persistent_storage_uses_shared_database_for_all_stores() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("mob.db");
        let storage = MobStorage::persistent(&db_path).unwrap();

        let event = NewMobEvent {
            mob_id: MobId::from("mob"),
            timestamp: None,
            kind: MobEventKind::MobCompleted,
        };
        storage.events.append(event).await.unwrap();

        let run = MobRun::authority_backed_for_steps(
            RunId::new(),
            MobId::from("mob"),
            crate::FlowId::from("flow"),
            [StepId::from("step-1")],
            MobRunStatus::Pending,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
        storage.runs.create_run(run.clone()).await.unwrap();
        assert!(storage.runs.get_run(&run.run_id).await.unwrap().is_some());

        let definition = crate::definition::MobDefinition::from_toml(
            r#"
[mob]
id = "mob"
[profiles.worker]
model = "test"
"#,
        )
        .unwrap();
        let revision = storage
            .specs
            .put_spec(&MobId::from("mob"), &definition, None)
            .await
            .unwrap();
        assert_eq!(revision, 1);

        let identity = crate::AgentIdentity::from("persistent-member");
        let lease = storage
            .identity
            .claim_or_renew_identity_lease(
                &MobId::from("mob"),
                &identity,
                "controller",
                "incarnation-a",
                30_000,
            )
            .await
            .unwrap();
        let expected_epoch = match lease {
            crate::IdentityLeaseClaimOutcome::Acquired(claim) => claim.epoch,
            other => panic!("expected initial persistent lease, got {other:?}"),
        };
        drop(storage);

        let reopened = MobStorage::persistent(&db_path).unwrap();
        assert!(matches!(
            reopened
                .identity
                .observe_identity_lease(&MobId::from("mob"), &identity)
                .await
                .unwrap(),
            crate::IdentityStoredObservation::Valid(crate::IdentityLeaseRecord {
                epoch_highwater,
                ..
            }) if epoch_highwater == expected_epoch
        ));
    }
}
