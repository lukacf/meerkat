//! Meta's own durable identity records.
//!
//! Two records, each keyed by identity, kept where an earlier Meta never
//! looks (rows of the existing `meta_metadata` table under their own key
//! prefixes; documents of Meta's record family on a host store realm), so a
//! rollback ignores them:
//! - the last-published tool record: the visible tools of the identity's
//!   member as of its last build, written from Core's report of each build
//!   and read by `meta/identity/resolved_tools` for a member Core holds
//!   Dormant, which no read materializes;
//! - the rotation intent: a fresh life (`reset`, `delete_identity`) Meta
//!   asked Core to start, recorded before Core is asked and removed once
//!   Core answers, so the next roster compile drives a rotation a crash
//!   interrupted to its end.
//!
//! Without a durable store both are kept for this process only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use meerkat_core::types::SessionId;

use super::super::first_boot_manifest::{FirstBootLocation, FirstBootManifest};
use super::super::types::{AgentIdentity, DurableAgentSpec};
use super::{IdentityRecords, IdentityRuntime, IdentityRuntimeError};
use crate::runtime::{IdentityRotationIntent, IdentityToolRecord, PersistentMetadataStore};

/// The members the mob must leave to Meta's preparation when it starts
/// (`MobBuilder::with_host_pending_members`), read before the mob is built:
/// each identity with a pending first-boot row, and each with a recorded
/// rotation a crash interrupted. The mob materializes none of them at
/// start, and refuses each one's wake retryable until Meta's seat step for
/// it (the first roster pass) carries the row or the rotation out and ends
/// the mark in the same step.
///
/// Only identities the roster lists are named: the roster pass seats only
/// those, and the mob would hold an unlisted identity for a preparation no
/// pass makes (its retirement would wait on it). Its row or record waits
/// for a roster that lists it, as before. A hosted identity (placed, or
/// bound to an external runtime) is never named either: its host owns its
/// materialization, Core seats no such member, and no seat step could end
/// its mark.
///
/// A first-boot manifest that cannot be read names every roster identity:
/// which of them it covers is unknown, and the roster pass refuses them all.
/// A rotation record store that cannot be read is logged and names none: the
/// rotation re-drive itself waits for a store that reads.
pub(crate) async fn host_pending_members_before_build(
    first_boot: Option<&FirstBootLocation>,
    records: Option<(&dyn PersistentMetadataStore, &str)>,
    roster: &[DurableAgentSpec],
) -> Vec<meerkat_mob::AgentIdentity> {
    let mut pending = BTreeSet::new();
    let manifest = match first_boot {
        Some(FirstBootLocation::StateDir(state_dir)) => {
            let state_dir = state_dir.clone();
            match tokio::task::spawn_blocking(move || FirstBootManifest::read(&state_dir)).await {
                Ok(read) => read.map_err(|error| error.refusal().to_string()),
                Err(error) => Err(format!("the read did not finish: {error}")),
            }
        }
        Some(FirstBootLocation::HostStore(documents)) => {
            FirstBootManifest::read_host_store(documents)
                .await
                .map_err(|error| error.refusal().to_string())
        }
        None => Ok(None),
    };
    match manifest {
        Ok(Some(manifest)) => {
            pending.extend(manifest.rows().iter().map(|row| row.identity().clone()));
        }
        Ok(None) => {}
        Err(detail) => {
            tracing::error!(
                %detail,
                "the first-boot manifest cannot be read before the mob is built: every roster \
                 identity is left to the roster pass, which refuses them until it reads"
            );
            pending.extend(roster.iter().map(|spec| spec.identity.clone()));
        }
    }
    if let Some((store, mob_id)) = records {
        match store.load_identity_rotation_intents(mob_id).await {
            Ok(intents) => {
                pending.extend(
                    intents
                        .iter()
                        .filter_map(|intent| AgentIdentity::parse(&intent.identity).ok()),
                );
            }
            Err(error) => tracing::error!(
                %error,
                "the identities' rotation records cannot be read before the mob is built; no \
                 interrupted rotation is left to the roster pass's preparation"
            ),
        }
    }
    let listed = roster
        .iter()
        .filter(|spec| !super::durable_spec_is_hosted(spec))
        .map(|spec| &spec.identity)
        .collect::<BTreeSet<_>>();
    pending
        .iter()
        .filter(|identity| listed.contains(identity))
        .map(|identity| crate::member_comms_id::mob_member_id(identity.as_str()))
        .collect()
}

/// The mob id the process-only records are keyed by.
const PROCESS_RECORDS_MOB: &str = "";

fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl IdentityRuntime {
    /// Keep Meta's identity records in `store`, keyed by `mob_id`: the
    /// metadata store the runtime's composition opened for its mob.
    pub(crate) fn set_identity_records(
        &self,
        store: Arc<dyn PersistentMetadataStore>,
        mob_id: String,
    ) {
        *self
            .identity_records
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(IdentityRecords { store, mob_id });
    }

    /// The store the identity records go to and the mob id they are keyed
    /// by: the durable store, or this process's own.
    fn identity_record_store(&self) -> (Arc<dyn PersistentMetadataStore>, String) {
        match &*self
            .identity_records
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            Some(records) => (Arc::clone(&records.store), records.mob_id.clone()),
            None => (
                Arc::clone(&self.process_identity_records) as Arc<dyn PersistentMetadataStore>,
                PROCESS_RECORDS_MOB.to_string(),
            ),
        }
    }

    /// Record `identity`'s rotation before Core is asked for it. Refused
    /// typed when the record cannot be written: a rotation a crash
    /// interrupted with no record would be left half done, so none starts.
    pub(super) async fn record_rotation_intent(
        &self,
        identity: &AgentIdentity,
        verb: &'static str,
        from_generation: Option<u64>,
    ) -> Result<(), IdentityRuntimeError> {
        let (store, mob_id) = self.identity_record_store();
        store
            .set_identity_rotation_intent(&IdentityRotationIntent {
                mob_id,
                identity: identity.as_str().to_string(),
                verb: verb.to_string(),
                from_generation,
                recorded_at_ms: unix_ms_now(),
            })
            .await
            .map_err(|error| IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: None,
                detail: format!(
                    "the {verb} was not started: its rotation could not be recorded ({error})"
                ),
            })
    }

    /// Remove `identity`'s rotation intent once Core answered its rotation.
    /// A failed removal is logged: the next roster compile finds the
    /// rotation done and removes it.
    pub(super) async fn clear_rotation_intent(&self, identity: &AgentIdentity) {
        let (store, mob_id) = self.identity_record_store();
        if let Err(error) = store
            .clear_identity_rotation_intent(&mob_id, identity.as_str())
            .await
        {
            tracing::warn!(
                %identity,
                %error,
                "the identity's finished rotation could not be removed from its records; the \
                 next roster compile finds it done and removes it"
            );
        }
    }

    /// Put `identity`'s rotation record back as it was before a rotation
    /// Core refused before any of it started: `earlier`, the record an
    /// earlier call left, or none. A failed write is logged: the record of
    /// the refused rotation then stays, and the identity's next roster
    /// compile may carry that rotation out.
    pub(super) async fn restore_rotation_intent(
        &self,
        identity: &AgentIdentity,
        earlier: Option<IdentityRotationIntent>,
    ) {
        let (store, mob_id) = self.identity_record_store();
        let restored = match &earlier {
            Some(earlier) => store.set_identity_rotation_intent(earlier).await,
            None => {
                store
                    .clear_identity_rotation_intent(&mob_id, identity.as_str())
                    .await
            }
        };
        if let Err(error) = restored {
            tracing::warn!(
                %identity,
                %error,
                "the record of a rotation Core refused could not be taken back; the identity's \
                 next roster compile may carry that rotation out"
            );
        }
    }

    /// The rotation intents recorded for this runtime's identities, by
    /// identity. A record naming no valid identity is skipped and logged.
    pub(crate) async fn pending_rotation_intents(
        &self,
    ) -> Result<BTreeMap<AgentIdentity, IdentityRotationIntent>, IdentityRuntimeError> {
        let (store, mob_id) = self.identity_record_store();
        let intents = store
            .load_identity_rotation_intents(&mob_id)
            .await
            .map_err(|error| IdentityRuntimeError::MemberHistoryUnavailable {
                detail: format!("reading the identities' rotation records: {error}"),
            })?;
        Ok(intents
            .into_iter()
            .filter_map(|intent| match AgentIdentity::parse(&intent.identity) {
                Ok(identity) => Some((identity, intent)),
                Err(error) => {
                    tracing::warn!(
                        identity = %intent.identity,
                        %error,
                        "a rotation record names no valid identity; it is skipped"
                    );
                    None
                }
            })
            .collect())
    }

    /// Record the visible tools of `identity`'s member as of the build Core
    /// just reported: the next build replaces the record, and a read never
    /// does. A failed write is logged; the record stays as it was.
    pub(crate) async fn record_built_tools(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        tools: Vec<String>,
    ) {
        if let Err(error) = self.write_built_tools(identity, session_id, tools).await {
            tracing::warn!(
                %identity,
                %error,
                "the visible tools of the identity's build could not be recorded; \
                 resolved_tools answers its previous record until the next build"
            );
        }
    }

    /// [`Self::record_built_tools`], answering the record written or the
    /// store's error.
    pub(crate) async fn write_built_tools(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        tools: Vec<String>,
    ) -> Result<IdentityToolRecord, crate::runtime::MetadataStoreError> {
        let (store, mob_id) = self.identity_record_store();
        let previous = match store.identity_tool_record(&mob_id, identity.as_str()).await {
            Ok(previous) => previous,
            Err(error) => {
                tracing::debug!(
                    %identity,
                    %error,
                    "the identity's previous tool record could not be read; this build's \
                     record starts a new count"
                );
                None
            }
        };
        let mut tools = tools;
        tools.sort();
        tools.dedup();
        let record = IdentityToolRecord {
            mob_id,
            identity: identity.as_str().to_string(),
            session_id: session_id.clone(),
            tools,
            record_revision: previous
                .map_or(1, |previous| previous.record_revision.saturating_add(1)),
            published_at_ms: unix_ms_now(),
        };
        store.set_identity_tool_record(&record).await?;
        Ok(record)
    }

    /// Warm `identity`: ask Core to materialize its member (Core's wake, the
    /// operator's cause, through the build preparation), delivering no input
    /// and adding nothing to its transcript, then write its live tools to its
    /// tool record. Answers the record once the member is live and the record
    /// is written, or the wake's typed refusal. Idempotent: a member already
    /// live is not built again, and its record is written again from its live
    /// session.
    pub(crate) async fn warm(
        &self,
        identity: &AgentIdentity,
        session_service: Option<&Arc<dyn meerkat_mob::MobSessionService>>,
    ) -> Result<IdentityToolRecord, IdentityRuntimeError> {
        self.materialize(identity).await?;
        let not_live = |detail: &str| IdentityRuntimeError::IdentityWakeRetryable {
            identity: identity.clone(),
            session_id: None,
            detail: detail.to_string(),
        };
        let session_id = self
            .core_live_member(identity)
            .await?
            .and_then(|live| live.session_id)
            .ok_or_else(|| {
                not_live("Core holds no session for the identity's member after its wake")
            })?;
        let snapshot = crate::mob_handle_runtime::live_resolved_tools_for_session(
            session_service,
            identity.as_str(),
            session_id.clone(),
        )
        .await
        .map_err(|error| {
            IdentityRuntimeError::Internal(format!(
                "reading the warmed member's live tools: {error}"
            ))
        })?
        .ok_or_else(|| {
            not_live("the member was released before its live tools were read; retry the warm")
        })?;
        self.write_built_tools(identity, &session_id, snapshot.tools)
            .await
            .map_err(|error| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("writing the identity's tool record: {error}"),
            })
    }

    /// Warm `identity` (`meta/identity/warm`): ask Core to materialize its
    /// member, delivering no input and adding nothing to its transcript, then
    /// write its live tools to its tool record, as a tracked foreground
    /// operation, so a shutdown waits for it. Answers the record once the
    /// member is live and the record is written, or the wake's typed refusal.
    /// Idempotent: a member already live is not built again.
    pub async fn warm_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        session_service: Option<Arc<dyn meerkat_mob::MobSessionService>>,
    ) -> Result<IdentityToolRecord, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move {
            runtime.warm(&identity, session_service.as_ref()).await
        })
        .await
    }

    /// The `meta/identity/resolved_tools` answer for `identity`, whose
    /// status names `session_id` (`supplied` is the id the caller used, which
    /// the answer echoes). A member live in this process answers its live
    /// session's tools. A member Core holds Dormant answers the tools its last
    /// build on this session had, from its tool record, or the typed
    /// `no_build_recorded` answer when no build of it on this session is
    /// recorded yet. No read wakes the member or calls the customizer.
    pub(crate) async fn resolved_tools_answer(
        &self,
        identity: &AgentIdentity,
        supplied: &str,
        session_id: SessionId,
        session_service: Option<&Arc<dyn meerkat_mob::MobSessionService>>,
    ) -> Result<serde_json::Value, String> {
        use crate::mob_handle_runtime::{
            ResolvedToolsSnapshot, ResolvedToolsSource, live_resolved_tools_for_session,
            resolved_tools_answer,
        };
        if let Some(snapshot) =
            live_resolved_tools_for_session(session_service, supplied, session_id.clone())
                .await
                .map_err(|error| error.to_string())?
        {
            return Ok(resolved_tools_answer(&snapshot, ResolvedToolsSource::Live));
        }
        let record = self
            .last_built_tools(identity)
            .await
            .map_err(|error| error.to_string())?
            .filter(|record| record.session_id == session_id);
        let (tools, source) = match record {
            Some(record) => (
                record.tools,
                ResolvedToolsSource::Recorded {
                    record_revision: record.record_revision,
                    published_at_ms: record.published_at_ms,
                },
            ),
            None => (Vec::new(), ResolvedToolsSource::NoBuildRecorded),
        };
        Ok(resolved_tools_answer(
            &ResolvedToolsSnapshot {
                identity: supplied.to_string(),
                session_id: session_id.to_string(),
                tools,
            },
            source,
        ))
    }

    /// `identity`'s last-published tool record, if a build of it was ever
    /// recorded. Read-only: no wake, no customizer call.
    pub(crate) async fn last_built_tools(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<IdentityToolRecord>, IdentityRuntimeError> {
        let (store, mob_id) = self.identity_record_store();
        store
            .identity_tool_record(&mob_id, identity.as_str())
            .await
            .map_err(|error| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading the identity's tool record: {error}"),
            })
    }
}
