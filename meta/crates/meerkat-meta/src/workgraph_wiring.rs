//! Wire meerkat's WorkGraph (goals, work items, attention bindings and
//! apply-time attention overlays) into the gateways.
//!
//! A mob profile's `tools.workgraph = true` maps (in `meerkat-mob`) to
//! `override_workgraph = Enable`, but a build with the category enabled and
//! no dispatcher in the agent factory's `default_workgraph_tools` slot is a
//! hard build error on non-wasm surfaces. [`attach_workgraph_tools`] fills
//! that slot from a durable SQLite store; [`attach_workgraph_tools_ephemeral`]
//! is the memory-store variant for non-persistent launches. Both return the
//! realm-scoped [`WorkGraphService`] so the same instance can back the
//! `meta/workgraph/*` RPC surface, mob-runtime attention overlays
//! (`MobBuilder::with_workgraph_service` via `MobBootstrapSpec`), and the
//! schedule host's apply-time overlay injection.
//!
//! The realm id is the mob definition id — mirroring the schedule host's
//! `owner_id` choice. Meerkat hosts refuse to invent a realm, so callers must
//! always pass a real one.

use std::path::Path;
use std::sync::Arc;

use meerkat::{
    FactoryAgentBuilder, MemoryWorkGraphStore, SqliteWorkGraphStore, WorkGraphService,
    WorkGraphStore, WorkGraphToolSurface, WorkNamespace,
};

use crate::workgraph_admission::{
    WorkGraphAdmissionError, WorkGraphAdmissionPermit, WorkGraphAdmissionSlot,
};

/// File name for the durable workgraph store, kept beside the runtime DB so a
/// gateway and a library-mode runtime pointed at the same dir share state.
/// Matches meerkat's `PersistenceBundle` convention.
///
/// Because two PROCESSES may share this file, the duplicate-binding admission
/// guards cannot rely on their per-process gate alone: for SQLite-backed
/// stores the runtime's
/// [`WorkGraphAdmission`](crate::workgraph_admission::WorkGraphAdmission)
/// additionally serializes each
/// check-then-mutate window cross-process through a sidecar lock database
/// ([`WORKGRAPH_ADMISSION_SIDECAR_FILE`](crate::workgraph_admission::WORKGRAPH_ADMISSION_SIDECAR_FILE),
/// created beside this store) — a `BEGIN IMMEDIATE` transaction held for the
/// window's duration. The sidecar is deliberately NOT this store file:
/// holding a write transaction on the real store would deadlock against the
/// service's own writes. Memory-backed runtimes are single-process by
/// construction and keep the in-process gate only.
pub const WORKGRAPH_STORE_FILE: &str = "workgraph.sqlite3";

/// Build a realm-scoped [`WorkGraphService`] over a durable SQLite store
/// under `state_dir`. Returns `None` (with a warning) if the store cannot be
/// opened — the gateway then boots without workgraph rather than failing
/// closed, matching the schedule-tools posture.
#[must_use]
pub fn open_workgraph_service(state_dir: &Path, realm_id: &str) -> Option<WorkGraphService> {
    open_workgraph_service_reporting(state_dir, realm_id).ok()
}

/// [`open_workgraph_service`] with the open failure reported instead of
/// swallowed, so composition sites can record the sanctioned boot-without
/// posture as a health-visible storage-slot degradation (M4) rather than a
/// warn line alone.
pub fn open_workgraph_service_reporting(
    state_dir: &Path,
    realm_id: &str,
) -> Result<WorkGraphService, String> {
    let path = state_dir.join(WORKGRAPH_STORE_FILE);
    let store = match SqliteWorkGraphStore::open(&path) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to open workgraph store; workgraph disabled for this gateway",
            );
            return Err(format!("{}: {error}", path.display()));
        }
    };
    Ok(scoped_workgraph_service(Arc::new(store), realm_id))
}

/// Build a realm-scoped [`WorkGraphService`] over an in-memory store, for
/// non-persistent launches. Tools stay profile-gated, so nothing changes for
/// members that do not opt in.
#[must_use]
pub fn ephemeral_workgraph_service(realm_id: &str) -> WorkGraphService {
    scoped_workgraph_service(Arc::new(MemoryWorkGraphStore::new()), realm_id)
}

/// Scope a WorkGraph service to a mob, using meerkat's CANONICAL mob realm.
///
/// The parameter is a MOB ID, not a raw realm. `meerkat_core::mob_realm_id`
/// renders it as `mob.<mob_id>`, which is the same realm meerkat derives for
/// every member it builds for that mob (`meerkat-mob/src/build.rs`).
///
/// This matters because 0.8.22 validates the two against each other: a
/// WorkGraph namespace grant whose realm differs from the member's build realm
/// is a typed build refusal ("WorkGraph namespace grant realm '..' does not
/// match build realm '..'"). Meta previously scoped the workgraph with the
/// BARE mob id while every other realm-scoped thing for the same mob used
/// `mob.<id>`, so the two could never agree and any member declaring workgraph
/// tools failed to build. Deriving from one helper is what keeps them equal;
/// do not reintroduce a second spelling of "the realm for this mob".
///
/// If the canonical realm cannot be formed the raw id is used, which lets
/// meerkat's own validation produce its typed error naming BOTH realms rather
/// than this function inventing a worse one.
pub(crate) fn scoped_workgraph_service(
    store: Arc<dyn WorkGraphStore>,
    mob_id: &str,
) -> WorkGraphService {
    let realm = match meerkat_core::mob_realm_id(mob_id) {
        Ok(realm) => realm.as_str().to_string(),
        Err(error) => {
            tracing::error!(
                mob_id,
                %error,
                "could not form the canonical mob realm; workgraph scope falls back to the raw \
                 mob id and member builds declaring workgraph tools will be refused upstream"
            );
            mob_id.to_string()
        }
    };
    WorkGraphService::with_scope(store, realm, WorkNamespace::default())
}

/// Fill `builder`'s default workgraph-tools slot with the full
/// [`WorkGraphToolSurface`] over `service`. After this, any member whose
/// profile resolves workgraph tools on (`tools.workgraph = true`) is built
/// with the `workgraph_*` surface. Call on the `FactoryAgentBuilder` BEFORE
/// it is consumed into a session service (the slot is interior-mutable, but
/// attaching up front matches the schedule-tools wiring).
///
/// Returns the dispatcher's late-bound [`WorkGraphAdmissionSlot`]. Register
/// it on the bootstrap spec
/// ([`MobBootstrapSpec::with_workgraph_admission_slot`](crate::MobBootstrapSpec::with_workgraph_admission_slot))
/// so `MobRuntime::bootstrap` fills it with the runtime-wide admission —
/// otherwise the agent tool plane's `workgraph_attention_reassign` skips the
/// duplicate-binding guard the RPC surfaces enforce.
pub fn install_workgraph_tools(
    builder: &FactoryAgentBuilder,
    service: &WorkGraphService,
) -> WorkGraphAdmissionSlot {
    let tools = ScopePinnedWorkGraphTools::new(service);
    let slot = tools.admission_slot();
    meerkat::surface::set_default_workgraph_tools(builder, Some(Arc::new(tools)));
    // 0.8.16 item 6 / 0.8.22 port requirement. Since 0.8.22 meerkat REFUSES to
    // build any agent whose WorkGraph tools are enabled without a host-issued
    // namespace grant (`meerkat/src/factory.rs`, "WorkGraph tools enabled
    // without a host-issued namespace grant"). Without this line every member
    // that declares workgraph tools fails to build on create, and on resume
    // degrades the identity rather than failing cleanly.
    //
    // The grant is taken from the SERVICE rather than reconstructed from a
    // realm id and a namespace we happen to have nearby, so the grant and the
    // scope the tools are pinned to cannot drift apart: `ScopePinnedWorkGraphTools`
    // above pins every call to this same service's realm+namespace, and a
    // grant naming a different scope would authorise something the tools can
    // never address. One value, one source - that is the whole point of item
    // 6's "mechanism owns scope, agents don't wander realms".
    meerkat::surface::set_default_workgraph_namespace_grant(
        builder,
        Some(service.namespace_grant().clone()),
    );
    slot
}

/// Pin every `workgraph_*` tool call to the service's realm + namespace.
///
/// The upstream tool schema exposes `realm_id`/`namespace` as free parameters
/// and models DO fill them with plausible inventions (live-fire: an agent
/// passed `realm_id = "default", namespace = "<mob id>"` — the exact inverse
/// of the service scope), which strands the work outside every operator
/// surface: the RPC snapshot reads the pinned scope, and CAS actions against
/// the invented scope NotFound. In meta one runtime is one realm (the mob
/// definition id) and one namespace: mechanism owns scope, agents don't
/// wander realms. Pinning uniformly on reads AND writes keeps the agent's
/// view self-consistent whatever values it invents.
///
/// Both dispatch entry points are overridden: the trait-default
/// `dispatch_with_context` funnels through `dispatch`, which would DROP the
/// `ToolDispatchContext` carrying the attention-projection witness
/// (`WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY`) — silently bypassing every
/// attention-scoped enforcement in the inner surface and permanently denying
/// `workgraph_policy_escalate`/`workgraph_attention_reassign` to
/// legitimately-delegated agents.
///
/// Round-3 finding R1: `workgraph_attention_reassign` mints an Active
/// binding on the NEW target, so a coordinate-mode member could land a second
/// occupying binding on an occupied member where an ABAC-granted operator on
/// the RPC surface is refused, and could race the RPC guards. Upstream ask 25
/// closes the exact-key Active/Active half of that in the store
/// (`active_target_occupant_tx`), but not the paused, cross-spelling or
/// session-identity-alias cases, and an admitted duplicate is no longer the
/// hard `MultipleActiveBindings` failure it once was - the turn overlay
/// arbitrates newest-binding-wins and silently starves the loser's goal (see
/// [`crate::workgraph_admission`]'s module docs for the full subsumption
/// split). Both dispatch paths therefore intercept the reassign: when the
/// late-bound [`WorkGraphAdmissionSlot`] is filled (by
/// `MobRuntime::bootstrap` — the wrapper is constructed before the mob
/// exists), the call holds the runtime-wide admission across the forward
/// and runs the same occupancy check as the RPC arms; when unfilled
/// (non-mob embedder) it forwards as before.
///
/// Round-4 Q3: the admission is taken only for a reassign that CARRIES the
/// attention-projection witness. Upstream requires the witness on both
/// dispatch entry points — the trait `dispatch` funnels into
/// `dispatch_with_context` with a default (witness-less) context, and a
/// missing projection is an immediate `access_denied` for
/// `workgraph_attention_reassign` before any store access (meerkat 0.8.22,
/// meerkat-workgraph/src/tool_surface.rs; `WorkGraphToolSurface::new` bakes
/// in no projection). A witness-less reassign therefore forwards directly
/// into that cheap upstream denial instead of queueing on the global gate +
/// cross-process sidecar (up to its 30s busy timeout) — otherwise a
/// retry-looping model plus a wedged co-process would stall every operator
/// binding mutation behind calls that can never succeed.
///
/// Round-4 Q2: an admitted reassign whose session target resolves to a
/// roster member is forwarded with the target lowered to the member's owner
/// form — the same normalize-at-write rule as the RPC arms (see
/// [`crate::workgraph_admission`]'s module docs), so tool-plane writes never
/// mint the session-form member rows that are alias-blind cross-process.
/// Round-6: "session target" includes a session spelled as an owner key
/// (`owner_key.kind == "session"`), which canonicalizes through the same
/// path instead of reaching the store verbatim.
struct ScopePinnedWorkGraphTools {
    inner: Arc<WorkGraphToolSurface>,
    service: WorkGraphService,
    admission: WorkGraphAdmissionSlot,
    realm_id: String,
    namespace: String,
}

/// The subset of the upstream `workgraph_attention_reassign` tool schema the
/// admission guard needs (meerkat 0.8.22, meerkat-workgraph/src/tools.rs
/// `attention_reassign_schema`: `binding_id`, `expected_revision`, `target`
/// all required; `target` is the tagged session/owner `GoalAttentionTarget`).
#[derive(serde::Deserialize)]
struct ReassignAdmissionArgs {
    binding_id: meerkat::WorkAttentionBindingId,
    target: meerkat::GoalAttentionTarget,
}

impl ScopePinnedWorkGraphTools {
    fn new(service: &WorkGraphService) -> Self {
        Self {
            inner: Arc::new(WorkGraphToolSurface::new(service.clone())),
            service: service.clone(),
            admission: Arc::new(std::sync::RwLock::new(None)),
            realm_id: service.default_realm_id().to_string(),
            namespace: service.default_namespace().as_str().to_string(),
        }
    }

    fn admission_slot(&self) -> WorkGraphAdmissionSlot {
        Arc::clone(&self.admission)
    }

    /// Guard a `workgraph_attention_reassign` before it reaches the inner
    /// surface: hold the runtime-wide admission and run the occupancy check
    /// on the parsed target. Returns the permit to hold across the forwarded
    /// dispatch — so a racing RPC `goal/create` on the same target cannot
    /// slip between check and mutate — plus the arguments to forward: for an
    /// admitted reassign whose session target addresses a roster member,
    /// the target is rewritten to the member's owner form (normalize at
    /// write, as on the RPC arms); every other call forwards `pinned`
    /// unchanged. A `None` permit means no guard applies: not a reassign, a
    /// witness-less reassign (upstream denies it before any store access on
    /// both entry points — see the struct docs — so taking the global
    /// admission would only let doomed calls stall real mutations), or an
    /// unfilled slot (a non-mob embedder, which forwards exactly as before
    /// the guard existed).
    async fn admit(
        &self,
        name: &str,
        pinned: Box<serde_json::value::RawValue>,
        witnessed: bool,
    ) -> Result<
        (
            Option<WorkGraphAdmissionPermit>,
            Box<serde_json::value::RawValue>,
        ),
        meerkat_core::ToolError,
    > {
        if name != "workgraph_attention_reassign" || !witnessed {
            return Ok((None, pinned));
        }
        let admission = self
            .admission
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(admission) = admission else {
            return Ok((None, pinned));
        };
        let args: ReassignAdmissionArgs = serde_json::from_str(pinned.get()).map_err(|error| {
            meerkat_core::ToolError::invalid_arguments(
                name,
                format!("invalid workgraph_attention_reassign arguments: {error}"),
            )
        })?;
        let target = admission
            .lower_member_session_target(args.target.clone())
            .await
            .map_err(|error| admission_tool_error(name, error))?;
        let permit = admission
            .acquire()
            .await
            .map_err(|error| admission_tool_error(name, error))?;
        // The pinned args force the service's default namespace, so the
        // occupancy check reads the same (default-namespace) binding set the
        // RPC guards do.
        admission
            .check_target_free(
                &self.service,
                None,
                &target.to_attention_target(),
                Some(&args.binding_id),
                "reassigning this binding onto the same target",
            )
            .await
            .map_err(|error| admission_tool_error(name, error))?;
        let forwarded = if target == args.target {
            pinned
        } else {
            let mut value: serde_json::Value =
                serde_json::from_str(pinned.get()).map_err(|error| {
                    meerkat_core::ToolError::invalid_arguments(
                        name,
                        format!("invalid workgraph_attention_reassign arguments: {error}"),
                    )
                })?;
            value["target"] = serde_json::json!(target);
            serde_json::value::RawValue::from_string(value.to_string()).map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(
                    name,
                    format!("failed to encode normalized reassign target: {error}"),
                )
            })?
        };
        Ok((Some(permit), forwarded))
    }

    /// Re-encode a `workgraph_*` call's arguments with the service scope
    /// pinned; `None` means the call is not a workgraph tool and must be
    /// forwarded unchanged.
    fn pin_args(
        &self,
        call: &meerkat_core::types::ToolCallView<'_>,
    ) -> Result<Option<Box<serde_json::value::RawValue>>, meerkat_core::ToolError> {
        if !call.name.starts_with("workgraph_") {
            return Ok(None);
        }
        let mut args: serde_json::Value =
            serde_json::from_str(call.args.get()).map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(
                    call.name,
                    format!("invalid workgraph tool-call arguments JSON: {error}"),
                )
            })?;
        if let Some(object) = args.as_object_mut() {
            object.insert(
                "realm_id".to_string(),
                serde_json::Value::String(self.realm_id.clone()),
            );
            object.insert(
                "namespace".to_string(),
                serde_json::Value::String(self.namespace.clone()),
            );
            // A pinned namespace makes cross-namespace reads meaningless.
            object.remove("all_namespaces");
        }
        serde_json::value::RawValue::from_string(args.to_string())
            .map(Some)
            .map_err(|error| {
                meerkat_core::ToolError::invalid_arguments(
                    call.name,
                    format!("failed to encode pinned workgraph arguments: {error}"),
                )
            })
    }
}

/// Project an admission refusal onto the tool plane. The occupancy conflict
/// keeps the RPC surface's `workgraph_conflict` vocabulary in structured
/// data and names the occupying binding plus the way out in the message, so
/// the agent can act on it (close or reassign the occupant) instead of
/// retrying a call that can never succeed.
fn admission_tool_error(name: &str, error: WorkGraphAdmissionError) -> meerkat_core::ToolError {
    match error {
        WorkGraphAdmissionError::Occupied { detail } => {
            meerkat_core::ToolError::execution_failed_with_data(
                format!("workgraph conflict: {detail}"),
                serde_json::json!({
                    "kind": "workgraph_conflict",
                    "detail": detail,
                }),
            )
        }
        WorkGraphAdmissionError::Service(error) => meerkat_core::ToolError::execution_failed(
            format!("{name} admission check failed: {error}"),
        ),
        WorkGraphAdmissionError::Lock(detail) => meerkat_core::ToolError::execution_failed(
            format!("{name} admission lock failed: {detail}"),
        ),
    }
}

#[async_trait::async_trait]
impl meerkat_core::AgentToolDispatcher for ScopePinnedWorkGraphTools {
    fn tools(&self) -> Arc<[Arc<meerkat_core::types::ToolDef>]> {
        self.inner.tools()
    }

    /// Forward the inner surface's catalog declarations verbatim.
    ///
    /// This wrapper pins dispatch ARGUMENTS and guards reassign admission; it
    /// deliberately does not narrow the surface, and `tools()` above already
    /// forwards unchanged. Leaving these two on the trait defaults silently
    /// downgraded the composed dispatcher to `exact_catalog: false` with an
    /// EMPTY catalog, which is not a conservative choice - it is a different
    /// surface than the one being wrapped.
    ///
    /// Both flags are load-bearing, not decorative. `AgentBuilder` only fills
    /// `deferred_tool_names` when `exact_catalog` holds AND
    /// (`may_require_catalog_control_plane` or a Control-plane entry exists)
    /// AND the catalog mode is `Deferred`. The inner surface declares both and
    /// mints `session_deferred` entries with WorkGraph provenance for
    /// `workgraph_attention_reassign` and `workgraph_policy_escalate`. Dropping
    /// them left the deferred set empty while an attention overlay still
    /// supplied a WorkGraph-provenance authority, so staging the overlay was
    /// rejected and the member's bound turn died before reaching the LLM - a
    /// member that goes silent for exactly as long as a binding is active.
    ///
    /// The dispatch-side surfaces are deliberately NOT forwarded: the default
    /// `dispatch_resolved_with_context` routes Fast-mode calls back through
    /// THIS type's `dispatch_with_context`, so pinning and admission still
    /// apply. Forwarding it to the inner would bypass both.
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    async fn dispatch(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let Some(pinned) = self.pin_args(&call)? else {
            return self.inner.dispatch(call).await;
        };
        // Plain dispatch carries no context and thus no witness: upstream
        // denies a reassign here regardless, so no admission is taken.
        let (_permit, forwarded) = self.admit(call.name, pinned, false).await?;
        self.inner
            .dispatch(meerkat_core::types::ToolCallView {
                id: call.id,
                name: call.name,
                args: &forwarded,
            })
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: meerkat_core::types::ToolCallView<'_>,
        context: &meerkat_core::agent::ToolDispatchContext,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let Some(pinned) = self.pin_args(&call)? else {
            return self.inner.dispatch_with_context(call, context).await;
        };
        let witnessed = context
            .turn_metadata(meerkat::WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY)
            .is_some();
        let (_permit, forwarded) = self.admit(call.name, pinned, witnessed).await?;
        self.inner
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: call.id,
                    name: call.name,
                    args: &forwarded,
                },
                context,
            )
            .await
    }
}

/// Open the durable store under `state_dir` and attach its tool surface to
/// `builder`. Returns the service and the tool-plane admission slot (see
/// [`install_workgraph_tools`]), or `None` (boot-without) on open failure.
#[must_use]
pub fn attach_workgraph_tools(
    builder: &FactoryAgentBuilder,
    state_dir: &Path,
    realm_id: &str,
) -> Option<(WorkGraphService, WorkGraphAdmissionSlot)> {
    attach_workgraph_tools_reporting(builder, state_dir, realm_id).ok()
}

/// [`attach_workgraph_tools`] with the open failure reported instead of
/// swallowed (see [`open_workgraph_service_reporting`]).
pub fn attach_workgraph_tools_reporting(
    builder: &FactoryAgentBuilder,
    state_dir: &Path,
    realm_id: &str,
) -> Result<(WorkGraphService, WorkGraphAdmissionSlot), String> {
    let service = open_workgraph_service_reporting(state_dir, realm_id)?;
    let slot = install_workgraph_tools(builder, &service);
    Ok((service, slot))
}

/// Memory-store variant of [`attach_workgraph_tools`] for ephemeral launches.
#[must_use]
pub fn attach_workgraph_tools_ephemeral(
    builder: &FactoryAgentBuilder,
    realm_id: &str,
) -> (WorkGraphService, WorkGraphAdmissionSlot) {
    let service = ephemeral_workgraph_service(realm_id);
    let slot = install_workgraph_tools(builder, &service);
    (service, slot)
}

/// Per-slot workgraph injection: scope a CALLER-SUPPLIED [`WorkGraphStore`] to
/// `realm_id` and attach its tool surface to `builder`.
///
/// The twin of [`crate::schedule_wiring::attach_schedule_tools_with_store`],
/// and the one seam that lets an embedder put the workgraph on its own
/// durable backend WITHOUT implementing a whole
/// [`MetaStorageProvider`](crate::storage_provider::MetaStorageProvider):
/// the composite provider is a single realm bundle, so routing the workgraph
/// through it drags continuity, lease authority, console, metadata, blobs,
/// agent memory, schedule, runtime and jobs along with it.
///
/// Returns the tool-plane [`WorkGraphAdmissionSlot`] with the SAME obligation
/// [`install_workgraph_tools`] carries: register it on the bootstrap spec
/// ([`MobBootstrapSpec::with_workgraph_admission_slot`](crate::MobBootstrapSpec::with_workgraph_admission_slot))
/// or the agent plane's `workgraph_attention_reassign` skips the
/// duplicate-binding guard the RPC surfaces enforce.
///
/// # Cross-process admission: what an injected store does NOT get
///
/// The duplicate-binding sidecar lock is keyed on the runtime's STATE DIR
/// ([`workgraph_admission_sidecar_path`](crate::workgraph_admission::workgraph_admission_sidecar_path)),
/// not on the store's location, and the injected backend may live anywhere.
/// Two runtimes sharing ONE injected store from DIFFERENT state dirs
/// therefore take two different sidecars and serialize nothing against each
/// other: what survives is each process's in-process gate plus the store's
/// own transactional occupancy guard, which covers only the exact-key
/// Active/Active case (the paused, cross-spelling and session-identity-alias
/// residuals stay check-then-act). Nothing here refuses that topology - the
/// provider-bundle path shares the same state-dir keying - so a caller that
/// shares one backend across state dirs must serialize admissions itself.
/// Co-processes on one state dir - the documented
/// gateway-plus-library-mode shape - are unaffected.
///
/// # Durability posture: this is an injection, not a provider bundle
///
/// A caller-injected slot deliberately does NOT go through
/// `storage_provider::open_provider_meerkat_stores`, so it takes neither
/// `ensure_realm_manifest_pin_with_candidates` nor
/// `enforce_fail_closed_durability`. That is the same ruling the schedule,
/// blob, continuity, lease and console setters already carry, and it is a
/// ruling rather than an oversight.
///
/// `enforce_fail_closed_durability` judges a PROVIDER-DECLARED
/// [`meerkat_core::DurabilityDeclaration`] - a `DurabilityResolution` the
/// provider asserts and meta holds it to. A bare `Arc<dyn WorkGraphStore>`
/// asserts nothing, and meta cannot derive the missing assertion:
/// `WorkGraphStore::kind()` is a BACKEND-SHAPE tag, not a durability oracle.
/// `Custom` covers every third-party backend indistinguishably - a durable
/// Postgres and an in-process test double both report it - and `Sqlite` says
/// nothing about the caller-supplied path behind it (a tmpfs or temp-dir file
/// reports `Sqlite` exactly as a durable one does). Gating on `kind()` would
/// therefore refuse legitimate durable backends and admit non-durable ones,
/// which is worse than not gating. So durability rides with the injector, and
/// the storage census must label the slot caller-injected rather than assert
/// a resolution meta never established. An embedder that wants the
/// fail-closed rule applied to its workgraph has the seam for it: supply a
/// `MetaStorageProvider` whose `meerkat_provider()` declares the `workgraph`
/// domain.
///
/// For the same reason `workgraph` must NOT be added to
/// `storage_provider::REQUIRED_META_DURABILITY_DOMAINS`:
/// that constant enumerates the META-level slots a `MetaStorageProvider`
/// must declare exactly once, and its completeness check would then refuse
/// every existing provider implementation. The workgraph is a MEERKAT-level
/// slot; on the provider path it is already covered by
/// `enforce_fail_closed_durability` over the meerkat `RealmStoreSet`.
#[must_use]
/// `mob_id` is a MOB ID, not a raw realm. It is rendered into meerkat's
/// canonical mob realm (`mob.<mob_id>`) by
/// `scoped_workgraph_service`, because 0.8.22 refuses to build any member
/// whose WorkGraph namespace grant realm differs from its build realm, and
/// meerkat derives that build realm as `mob.<mob_id>`. Pass the mob
/// definition id; passing an already-prefixed realm would double the prefix.
pub fn attach_workgraph_tools_with_store(
    builder: &FactoryAgentBuilder,
    store: Arc<dyn WorkGraphStore>,
    mob_id: &str,
) -> (WorkGraphService, WorkGraphAdmissionSlot) {
    let service = scoped_workgraph_service(store, mob_id);
    let slot = install_workgraph_tools(builder, &service);
    (service, slot)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat::{AgentFactory, Config, CreateWorkItemRequest, WorkGraphStoreKind};

    fn test_builder(dir: &Path) -> FactoryAgentBuilder {
        FactoryAgentBuilder::new(AgentFactory::new(dir).comms(true), Config::default())
    }

    fn slot_is_filled(builder: &FactoryAgentBuilder) -> bool {
        builder
            .default_workgraph_tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    #[tokio::test]
    async fn attach_workgraph_tools_populates_the_builder_slot_and_opens_the_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let builder = test_builder(dir.path());
        assert!(!slot_is_filled(&builder));

        let (service, _slot) = attach_workgraph_tools(&builder, dir.path(), "wiring-realm")
            .expect("sqlite workgraph store should open in a writable dir");

        assert!(
            slot_is_filled(&builder),
            "attach must fill the default_workgraph_tools slot"
        );
        // CANONICAL mob realm, `mob.<mob_id>`. Pinned as a literal rather
        // than recomputed from `mob_realm_id`, which would be tautological:
        // the spelling decides where rows physically live, so a change to it
        // is a data migration and must fail this test loudly.
        assert_eq!(service.default_realm_id(), "mob.wiring-realm");
        assert_eq!(service.store().kind(), WorkGraphStoreKind::Sqlite);
        assert!(
            dir.path().join(WORKGRAPH_STORE_FILE).exists(),
            "store file must be created beside the runtime DB"
        );
    }

    #[tokio::test]
    async fn sqlite_store_survives_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let service =
            open_workgraph_service(dir.path(), "reopen-realm").expect("open workgraph store");
        let item = service
            .create(CreateWorkItemRequest {
                title: "persisted item".to_string(),
                ..Default::default()
            })
            .await
            .expect("create item");

        let reopened =
            open_workgraph_service(dir.path(), "reopen-realm").expect("reopen workgraph store");
        let loaded = reopened
            .get(None, None, item.id.clone())
            .await
            .expect("item must survive reopen");
        assert_eq!(loaded.title, "persisted item");
        assert_eq!(loaded.realm_id, "mob.reopen-realm");
    }

    #[tokio::test]
    async fn open_failure_boots_without_workgraph() {
        let dir = tempfile::tempdir().expect("temp dir");
        // A directory occupying the store path makes SQLite open fail.
        std::fs::create_dir_all(dir.path().join(WORKGRAPH_STORE_FILE)).expect("blocker dir");
        assert!(
            open_workgraph_service(dir.path(), "broken-realm").is_none(),
            "open failure must degrade to None, not panic"
        );
    }

    #[tokio::test]
    async fn ephemeral_variant_fills_slot_with_memory_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let builder = test_builder(dir.path());

        let (service, _slot) = attach_workgraph_tools_ephemeral(&builder, "ephemeral-realm");

        assert!(slot_is_filled(&builder));
        assert_eq!(service.default_realm_id(), "mob.ephemeral-realm");
        assert_eq!(service.store().kind(), WorkGraphStoreKind::Memory);
        assert!(
            !dir.path().join(WORKGRAPH_STORE_FILE).exists(),
            "ephemeral variant must not create a store file"
        );
    }

    /// Per-slot injection: the caller's own durable store backs the surface,
    /// at the caller's own path - meta neither opens nor names it, and the
    /// conventional store file beside the runtime DB is never created.
    #[tokio::test]
    async fn injected_store_variant_uses_the_callers_backend() {
        let dir = tempfile::tempdir().expect("temp dir");
        let builder = test_builder(dir.path());
        let injected_path = dir.path().join("elsewhere").join("caller-owned.sqlite3");
        std::fs::create_dir_all(injected_path.parent().expect("parent")).expect("caller dir");
        let store: Arc<dyn WorkGraphStore> =
            Arc::new(SqliteWorkGraphStore::open(&injected_path).expect("caller store"));

        let (service, _slot) =
            attach_workgraph_tools_with_store(&builder, store, "injected-realm-slot");

        assert!(slot_is_filled(&builder));
        assert_eq!(service.default_realm_id(), "mob.injected-realm-slot");
        assert_eq!(service.store().kind(), WorkGraphStoreKind::Sqlite);
        assert!(
            injected_path.exists(),
            "the caller's own path backs the slot"
        );
        assert!(
            !dir.path().join(WORKGRAPH_STORE_FILE).exists(),
            "injection must not open meta's conventional store file"
        );
    }

    /// Finding A regression (2026-08-11): the wrapper must declare the SAME
    /// catalog surface as the surface it wraps.
    ///
    /// `ScopePinnedWorkGraphTools` pins dispatch arguments and guards reassign
    /// admission; it does not narrow the surface. When it implemented only
    /// `tools`/`dispatch`/`dispatch_with_context`, these two fell back to the
    /// trait defaults (`exact_catalog: false`, empty catalog) - not a
    /// conservative choice but a DIFFERENT surface than the inner one. That
    /// left `deferred_tool_names` empty while an attention overlay still
    /// supplied a WorkGraph-provenance authority, so staging the overlay was
    /// rejected and a bound member turn died before reaching the LLM.
    ///
    /// Asserts parity rather than literals, so a future inner-only change
    /// cannot silently reopen the gap. The one literal is `exact_catalog`,
    /// because a pure parity test would also pass if BOTH sides regressed to
    /// the default.
    #[tokio::test]
    async fn wrapper_declares_the_same_catalog_surface_as_the_inner() {
        use meerkat_core::AgentToolDispatcher;

        let service = ephemeral_workgraph_service("mob-realm");
        let wrapper = ScopePinnedWorkGraphTools::new(&service);
        let inner = WorkGraphToolSurface::new(service);

        let wrapped = wrapper.tool_catalog_capabilities();
        let direct = inner.tool_catalog_capabilities();
        assert_eq!(
            wrapped.exact_catalog, direct.exact_catalog,
            "wrapper must report the inner surface's exact_catalog"
        );
        assert_eq!(
            wrapped.may_require_catalog_control_plane, direct.may_require_catalog_control_plane,
            "wrapper must report the inner surface's control-plane requirement"
        );
        assert!(
            wrapped.exact_catalog,
            "both sides regressed to the default: an exact catalog is what makes \
             the deferred set reachable at all"
        );

        // `ToolCatalogEntry` is not `PartialEq`, so compare an explicit
        // projection (name, plane, deferred eligibility, provenance) rather
        // than only counting entries.
        fn summarize(entries: &[meerkat_core::ToolCatalogEntry]) -> Vec<String> {
            entries
                .iter()
                .map(|entry| {
                    format!(
                        "{}|{:?}|{:?}|{:?}",
                        entry.tool.name,
                        entry.plane,
                        entry.deferred_eligibility,
                        entry.tool.provenance
                    )
                })
                .collect()
        }

        let wrapped_catalog = wrapper.tool_catalog();
        let direct_catalog = inner.tool_catalog();
        assert_eq!(
            summarize(&wrapped_catalog),
            summarize(&direct_catalog),
            "wrapper must forward the inner catalog entries and their provenance verbatim"
        );
        assert!(
            !wrapped_catalog.is_empty(),
            "an empty catalog is the exact shape of the original defect"
        );
        assert!(
            wrapped_catalog
                .iter()
                .any(|entry| entry.tool.name.as_str() == "workgraph_policy_escalate"),
            "the tool at the centre of Finding A must appear in the forwarded catalog: {:?}",
            wrapped_catalog
                .iter()
                .map(|entry| entry.tool.name.as_str())
                .collect::<Vec<_>>()
        );
    }

    /// Live-fire regression (2026-07-08): the upstream tool schema exposes
    /// realm_id/namespace and a real model invented `realm_id = "default",
    /// namespace = "<mob id>"` — the inverse of the service scope — stranding
    /// its items outside every operator surface. The dispatcher must pin both
    /// on every workgraph_* call, reads and writes alike.
    #[tokio::test]
    async fn tool_calls_with_invented_scope_are_pinned_to_the_service_scope() {
        use meerkat_core::AgentToolDispatcher;
        use serde_json::value::RawValue;

        let service = ephemeral_workgraph_service("mob-realm");
        let tools = ScopePinnedWorkGraphTools::new(&service);

        let create_args = RawValue::from_string(
            serde_json::json!({
                "title": "invented scope",
                "realm_id": "default",
                "namespace": "mob-realm"
            })
            .to_string(),
        )
        .expect("args");
        tools
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-1",
                name: "workgraph_create",
                args: &create_args,
            })
            .await
            .expect("create dispatch");

        // The item must be visible in the SERVICE scope (what the RPC and
        // console read), not the invented one.
        let items = service
            .list(meerkat::WorkItemFilter::default())
            .await
            .expect("list");
        assert_eq!(items.len(), 1, "item must land in the pinned scope");
        assert_eq!(items[0].realm_id, "mob.mob-realm");
        assert_eq!(items[0].namespace.as_str(), "default");

        // Reads with invented scope must see the same world (self-consistent
        // agent view): snapshot with the inverted scope still returns the item.
        let snap_args = RawValue::from_string(
            serde_json::json!({
                "realm_id": "default",
                "namespace": "mob-realm",
                "all_namespaces": false
            })
            .to_string(),
        )
        .expect("snap args");
        let outcome = tools
            .dispatch(meerkat_core::types::ToolCallView {
                id: "call-2",
                name: "workgraph_snapshot",
                args: &snap_args,
            })
            .await
            .expect("snapshot dispatch");
        let rendered = format!("{outcome:?}");
        assert!(
            rendered.contains("invented scope"),
            "pinned snapshot must see the pinned item: {rendered}"
        );
    }

    /// Regression (adversarial finding F1): the trait-default
    /// `dispatch_with_context` funnels through `dispatch` and drops the
    /// dispatch context carrying the attention-projection witness. The wrapper
    /// must forward the context so the inner surface's attention-scoped
    /// enforcement (item-id pinning et al.) still fires on member turns.
    #[tokio::test]
    async fn dispatch_with_context_forwards_the_attention_witness_to_the_surface() {
        use std::collections::BTreeMap;

        use meerkat::{
            AttentionProjectionRequest, GoalAttentionTarget, GoalCreateRequest,
            WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY, WorkOwnerKey,
        };
        use meerkat_core::AgentToolDispatcher;
        use serde_json::value::RawValue;

        let service = ephemeral_workgraph_service("mob-realm");
        let tools = ScopePinnedWorkGraphTools::new(&service);

        let goal = service
            .create_goal(GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: "attention-scoped goal".to_string(),
                description: None,
                target: GoalAttentionTarget::Owner {
                    owner_key: WorkOwnerKey::principal("operator@example.test").expect("owner key"),
                },
                mode: Default::default(),
                completion_policy: Default::default(),
                // meerkat 0.8.22 promoted the item-shaping fields of
                // `CreateWorkItemRequest` onto `GoalCreateRequest`. Eight of
                // the ten already existed on `CreateWorkItemRequest` in
                // 0.8.21, where `create_goal` filled them from
                // `..CreateWorkItemRequest::default()`; the two join policies
                // are new in 0.8.22 and so had no prior value to preserve.
                // The values below therefore reproduce the pre-port item
                // exactly. Both join policies are inert here in any case:
                // this fixture creates no `parent` edges, so no child can
                // fail or cancel into this goal.
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
                delegated_authority: Default::default(),
                projection_policy: Default::default(),
            })
            .await
            .expect("create goal");
        let projection = service
            .attention_projection(AttentionProjectionRequest {
                binding_id: goal.attention.binding_id.clone(),
                realm_id: None,
                namespace: None,
            })
            .await
            .expect("attention projection")
            .projection;
        let decoy = service
            .create(CreateWorkItemRequest {
                title: "outside the attention scope".to_string(),
                ..Default::default()
            })
            .await
            .expect("decoy item");
        let context = meerkat_core::agent::ToolDispatchContext::default().with_turn_metadata(
            BTreeMap::from([(
                WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY.to_string(),
                serde_json::to_value(&projection).expect("projection json"),
            )]),
        );

        // A mutation against a DIFFERENT item must hit the upstream
        // attention-scope rejection — proof the witness reached the surface.
        let foreign_args = RawValue::from_string(
            serde_json::json!({
                "id": decoy.id,
                "expected_revision": decoy.revision,
                "evidence": { "kind": "note", "id": "n-1", "summary": "smuggled" },
            })
            .to_string(),
        )
        .expect("foreign args");
        let error = tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-scope-1",
                    name: "workgraph_add_evidence",
                    args: &foreign_args,
                },
                &context,
            )
            .await
            .expect_err("attention scope must pin the item id");
        assert!(
            error.to_string().contains("scoped to attention work item"),
            "upstream attention-scope rejection must surface: {error}"
        );

        // The same mutation against the bound item passes the scope checks.
        let scoped_args = RawValue::from_string(
            serde_json::json!({
                "id": goal.item.id,
                "expected_revision": goal.item.revision,
                "evidence": { "kind": "note", "id": "n-2", "summary": "in scope" },
            })
            .to_string(),
        )
        .expect("scoped args");
        tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-scope-2",
                    name: "workgraph_add_evidence",
                    args: &scoped_args,
                },
                &context,
            )
            .await
            .expect("attention-scoped call on the bound item succeeds");
    }

    // -- Round-3 R1: the agent tool plane shares the RPC admission guard ----

    const SESSION_OCCUPIED: &str = "019e63c2-0000-7000-8000-0000000000a1";
    const SESSION_MOVER: &str = "019e63c2-0000-7000-8000-0000000000a2";
    const SESSION_FREE: &str = "019e63c2-0000-7000-8000-0000000000a3";

    /// Per-test mob id: same-name supervisor routes in the process-global
    /// in-proc registry collide across concurrently running tests now that
    /// meerkat 0.8.23 refuses displacement, so each plane gets its own id.
    fn unique_admission_mob_id() -> String {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        format!(
            "wiring-admission-mob-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )
    }

    fn admission_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "autonomous_host"

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("parse admission test definition")
    }

    /// Build the REAL tool plane the runtime installs: attach the dispatcher
    /// to a builder, feed the builder into a session service, register the
    /// admission slot on the bootstrap spec, and let `MobRuntime::bootstrap`
    /// fill it — the exact wiring a gateway gets.
    async fn bootstrapped_tool_plane() -> (
        crate::MobRuntime,
        WorkGraphService,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
        tempfile::TempDir,
        String,
    ) {
        let mob_id = unique_admission_mob_id();
        let (runtime, service, dispatcher, dir) = tool_plane_for(&mob_id).await;
        (runtime, service, dispatcher, dir, mob_id)
    }

    /// Durable variant of [`tool_plane_for`] for the twin-plane succession
    /// tests: the roster-owning incumbent and the roster-blind successor are
    /// SEPARATE PROCESSES sharing a durable state dir in production, so the
    /// twin tests model them as two runtimes over one SQLite session store.
    /// The incumbent stands down terminally (releasing its supervisor route -
    /// meerkat 0.8.23 refuses displacement) before the successor bootstraps
    /// the SAME mob id ordinarily; the member session's binding survives in
    /// the shared store, which is exactly what blind resolution reads.
    async fn durable_tool_plane_for(
        mob_id: &str,
        state: &Path,
    ) -> (
        crate::MobRuntime,
        WorkGraphService,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
    ) {
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state.join("sessions.db"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut builder = test_builder(state);
        // The service must be scoped to the definition's own mob realm: member
        // owner keys `mob/<mob_id>/agent/<identity>` are refused in any other
        // realm (meerkat 0.8.41), exactly as the gateways scope it.
        let (service, slot) = attach_workgraph_tools_ephemeral(&builder, mob_id);
        let dispatcher = builder
            .default_workgraph_tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("dispatcher installed");
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        builder.default_blob_store = Some(blob_store.clone());
        let concrete = Arc::new(meerkat_session::PersistentSessionService::new(
            builder,
            8,
            session_store,
            runtime_store,
            blob_store,
        ));
        let spec = crate::MobBootstrapSpec::new(
            admission_definition(mob_id),
            meerkat_mob::MobStorage::in_memory(),
            concrete,
        )
        .with_workgraph_service(Some(service.clone()))
        .with_workgraph_admission_slot(slot)
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = crate::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap mob runtime");
        (runtime, service, dispatcher)
    }

    async fn tool_plane_for(
        mob_id: &str,
    ) -> (
        crate::MobRuntime,
        WorkGraphService,
        Arc<dyn meerkat_core::AgentToolDispatcher>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().expect("temp dir");
        let builder = test_builder(dir.path());
        // The service must be scoped to the definition's own mob realm: member
        // owner keys `mob/<mob_id>/agent/<identity>` are refused in any other
        // realm (meerkat 0.8.41), exactly as the gateways scope it.
        let (service, slot) = attach_workgraph_tools_ephemeral(&builder, mob_id);
        let dispatcher = builder
            .default_workgraph_tools
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .expect("dispatcher installed");
        let session_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(meerkat_session::EphemeralSessionService::new(builder, 8));
        let spec = crate::MobBootstrapSpec::new(
            admission_definition(mob_id),
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_workgraph_service(Some(service.clone()))
        .with_workgraph_admission_slot(slot)
        .with_options(crate::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = crate::MobRuntime::bootstrap(spec)
            .await
            .expect("bootstrap mob runtime");
        (runtime, service, dispatcher, dir)
    }

    async fn create_session_goal(
        service: &WorkGraphService,
        title: &str,
        session_id: &str,
        mode: meerkat::WorkAttentionMode,
    ) -> meerkat::GoalCreateResult {
        service
            .create_goal(meerkat::GoalCreateRequest {
                realm_id: None,
                namespace: None,
                title: title.to_string(),
                description: None,
                target: meerkat::GoalAttentionTarget::Session {
                    session_id: meerkat_core::SessionId::parse(session_id).expect("session id"),
                },
                mode,
                completion_policy: Default::default(),
                // meerkat 0.8.22 item-shaping passthrough (see the note on the
                // other fixture above): eight of these came from
                // `CreateWorkItemRequest::default()` in 0.8.21, the two join
                // policies are new in 0.8.22, and both join policies are inert
                // for a fixture that builds no `parent` edges.
                failed_child_join_policy: Default::default(),
                cancelled_child_join_policy: Default::default(),
                priority: Default::default(),
                labels: Default::default(),
                due_at: None,
                not_before: None,
                snoozed_until: None,
                external_refs: Vec::new(),
                evidence_refs: Vec::new(),
                status: None,
                delegated_authority: Default::default(),
                projection_policy: Default::default(),
            })
            .await
            .expect("create goal")
    }

    /// A dispatch context carrying the binding's attention witness — how a
    /// legitimately-delegated coordinate-mode member's turns arrive.
    async fn witness_context(
        service: &WorkGraphService,
        binding_id: &meerkat::WorkAttentionBindingId,
    ) -> meerkat_core::agent::ToolDispatchContext {
        use std::collections::BTreeMap;
        let projection = service
            .attention_projection(meerkat::AttentionProjectionRequest {
                binding_id: binding_id.clone(),
                realm_id: None,
                namespace: None,
            })
            .await
            .expect("attention projection")
            .projection;
        meerkat_core::agent::ToolDispatchContext::default().with_turn_metadata(BTreeMap::from([(
            meerkat::WORKGRAPH_ATTENTION_DISPATCH_CONTEXT_KEY.to_string(),
            serde_json::to_value(&projection).expect("projection json"),
        )]))
    }

    fn reassign_args(
        goal: &meerkat::GoalCreateResult,
        target_session: &str,
    ) -> Box<serde_json::value::RawValue> {
        serde_json::value::RawValue::from_string(
            serde_json::json!({
                "binding_id": goal.attention.binding_id,
                "expected_revision": goal.attention.machine_state.revision,
                "target": { "kind": "session", "session_id": target_session },
            })
            .to_string(),
        )
        .expect("reassign args")
    }

    /// Round-3 R1: a coordinate-mode member's `workgraph_attention_reassign`
    /// onto an occupied member must be refused with a conflict that names
    /// the occupying binding, exactly as the RPC surfaces refuse it to an
    /// ABAC-granted operator. The refusal must come from META here: the
    /// message asserted below is the admission's, taken before the forward,
    /// so this test pins the tool plane's interception rather than the
    /// upstream store guard (ask 25) that would also catch this particular
    /// Active-vs-Active, same-target-key case. A free target must still
    /// forward and succeed.
    #[tokio::test(flavor = "multi_thread")]
    async fn tool_plane_reassign_onto_an_occupied_target_names_the_occupant() {
        let (runtime, service, dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let occupant = create_session_goal(
            &service,
            "occupant",
            SESSION_OCCUPIED,
            meerkat::WorkAttentionMode::Pursue,
        )
        .await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let context = witness_context(&service, &mover.attention.binding_id).await;

        let onto_occupied = reassign_args(&mover, SESSION_OCCUPIED);
        let error = dispatcher
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-occupied",
                    name: "workgraph_attention_reassign",
                    args: &onto_occupied,
                },
                &context,
            )
            .await
            .expect_err("occupied target must be refused at admission");
        let message = error.to_string();
        assert!(
            message.contains(occupant.attention.binding_id.as_str()),
            "conflict must name the occupying binding: {message}"
        );
        assert!(
            message.contains("close its goal"),
            "conflict must be actionable for the agent: {message}"
        );
        assert_eq!(
            error.structured_data().expect("structured conflict data")["kind"],
            serde_json::json!("workgraph_conflict"),
            "tool plane keeps the RPC conflict vocabulary"
        );

        // The refused move must not have touched the mover binding: the same
        // witness and revision reassign cleanly onto a FREE target.
        let onto_free = reassign_args(&mover, SESSION_FREE);
        dispatcher
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-free",
                    name: "workgraph_attention_reassign",
                    args: &onto_free,
                },
                &context,
            )
            .await
            .expect("free target forwards and succeeds");
        runtime.handle().stop().await.expect("stop");
    }

    /// An UNFILLED admission slot (non-mob embedder: no runtime, no roster)
    /// forwards without any meta-side occupancy check. Since meerkat
    /// 0.7.25 (ask 25) the STORE owns binding uniqueness, so the forwarded
    /// duplicate-target reassign is rejected UPSTREAM with a typed conflict
    /// naming the occupant — the invariant holds even where meta's
    /// defense-in-depth admission is absent. (Pre-0.7.25 this test asserted
    /// the opposite: the unguarded forward LANDED, which is why the
    /// admission layer was load-bearing.)
    #[tokio::test]
    async fn unfilled_admission_slot_forwards_reassign_unguarded() {
        use meerkat_core::AgentToolDispatcher as _;

        let service = ephemeral_workgraph_service("unfilled-realm");
        let tools = ScopePinnedWorkGraphTools::new(&service);
        create_session_goal(
            &service,
            "occupant",
            SESSION_OCCUPIED,
            meerkat::WorkAttentionMode::Pursue,
        )
        .await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let context = witness_context(&service, &mover.attention.binding_id).await;

        let onto_occupied = reassign_args(&mover, SESSION_OCCUPIED);
        let error = tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-unguarded",
                    name: "workgraph_attention_reassign",
                    args: &onto_occupied,
                },
                &context,
            )
            .await
            .expect_err("store-owned uniqueness (0.7.25) rejects the unguarded duplicate");
        let detail = error.to_string();
        assert!(
            detail.contains("conflict"),
            "upstream rejection must be the typed uniqueness conflict: {detail}"
        );
        assert!(
            detail.contains("already targets"),
            "conflict must name the occupant binding: {detail}"
        );

        // A FREE target still forwards and lands — the unfilled slot only
        // means no meta-side pre-check, not a dead surface.
        let onto_free = reassign_args(&mover, SESSION_FREE);
        tools
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-unguarded-free",
                    name: "workgraph_attention_reassign",
                    args: &onto_free,
                },
                &context,
            )
            .await
            .expect("free-target reassign forwards through the unfilled slot");
    }

    /// Round-3 R1 (race): a tool-plane reassign and an RPC `goal/create`
    /// aimed at the same free target must admit exactly one — both sides
    /// hold the SAME runtime-wide admission across their check-then-act
    /// windows, so whichever acquires second sees the other's binding.
    #[tokio::test(flavor = "multi_thread")]
    async fn tool_reassign_and_rpc_goal_create_racing_admit_exactly_one() {
        let (runtime, service, dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let context = witness_context(&service, &mover.attention.binding_id).await;
        let admission = runtime.workgraph_admission();

        let tool_side = {
            let dispatcher = Arc::clone(&dispatcher);
            let args = reassign_args(&mover, SESSION_FREE);
            tokio::spawn(async move {
                dispatcher
                    .dispatch_with_context(
                        meerkat_core::types::ToolCallView {
                            id: "call-race",
                            name: "workgraph_attention_reassign",
                            args: &args,
                        },
                        &context,
                    )
                    .await
            })
        };
        let rpc_side = {
            let service = service.clone();
            tokio::spawn(async move {
                crate::rpc::workgraph_methods::handle_workgraph_method(
                    Some(&service),
                    &admission,
                    crate::rpc::workgraph_methods::WorkgraphSurface::HostStdin,
                    "meta/workgraph/goal/create",
                    &serde_json::json!({
                        "title": "racer",
                        "target": { "kind": "session", "session_id": SESSION_FREE },
                    }),
                )
                .await
            })
        };
        let (tool_result, rpc_result) = tokio::join!(tool_side, rpc_side);
        let (tool_result, rpc_result) = (tool_result.expect("join"), rpc_result.expect("join"));

        let successes = usize::from(tool_result.is_ok()) + usize::from(rpc_result.is_ok());
        assert_eq!(
            successes,
            1,
            "exactly one side wins the target: tool={:?} rpc={rpc_result:?}",
            tool_result
                .as_ref()
                .map(|_| "ok")
                .map_err(ToString::to_string),
        );
        // The loser lost to the occupancy check, not to some other failure.
        match (&tool_result, &rpc_result) {
            (Err(error), Ok(_)) => {
                assert_eq!(
                    error.structured_data().expect("conflict data")["kind"],
                    serde_json::json!("workgraph_conflict"),
                    "{error}"
                );
            }
            (Ok(_), Err(error)) => {
                assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");
            }
            other => panic!("exactly one winner expected: {other:?}"),
        }
        runtime.handle().stop().await.expect("stop");
    }

    // -- Round-3 R3: cross-process serialization on shared sqlite stores ----

    /// Two admission instances (two processes in real life) sharing one
    /// sidecar serialize: the second `acquire` waits until the first permit
    /// drops. The in-process gates are DISJOINT here, so only the sidecar
    /// can be providing the exclusion.
    #[tokio::test(flavor = "multi_thread")]
    async fn admissions_sharing_a_sidecar_serialize_like_two_processes() {
        let (runtime, _service, _dispatcher, dir, _mob_id) = bootstrapped_tool_plane().await;
        let sidecar = crate::workgraph_admission::workgraph_admission_sidecar_path(dir.path());
        let first = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime.handle(),
            runtime.session_service().cloned(),
            Some(sidecar.clone()),
        );
        let second = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime.handle(),
            runtime.session_service().cloned(),
            Some(sidecar),
        );

        let permit = first.acquire().await.expect("first admission");
        let waiter = tokio::spawn(async move { second.acquire().await.map(|_| ()) });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !waiter.is_finished(),
            "second admission must wait on the sidecar while the first permit is held"
        );

        drop(permit);
        waiter
            .await
            .expect("join")
            .expect("released sidecar admits the waiter");
        runtime.handle().stop().await.expect("stop");
    }

    // -- Round-4 Q2: normalize member targets to owner form at write --------

    async fn spawn_helper_member(runtime: &crate::MobRuntime) -> meerkat_core::types::SessionId {
        runtime
            .handle()
            .spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", "helper"))
            .await
            .expect("spawn member");
        runtime
            .handle()
            .resolve_bridge_session_id_observation(&meerkat_mob::ids::AgentIdentity::from("helper"))
            .await
            .expect("member bridge session id")
    }

    async fn rpc_goal_create(
        service: &WorkGraphService,
        admission: &crate::workgraph_admission::WorkGraphAdmission,
        title: &str,
        target: serde_json::Value,
    ) -> Result<serde_json::Value, crate::rpc::JsonRpcError> {
        crate::rpc::workgraph_methods::handle_workgraph_method(
            Some(service),
            admission,
            crate::rpc::workgraph_methods::WorkgraphSurface::HostStdin,
            "meta/workgraph/goal/create",
            &serde_json::json!({ "title": title, "target": target }),
        )
        .await
    }

    /// Round-4 Q2 + round-5 S1: the roster is PROCESS-local, so in the
    /// documented two-process deployment (gateway + library-mode runtime on
    /// one state dir) a co-process whose roster has never seen the member
    /// used to write raw SESSION-form rows for it — invisible to every
    /// identity-form occupancy check. The admission now resolves session →
    /// member through the SHARED session store's
    /// `session_metadata.mob_member_binding` when the roster misses, so the
    /// blind co-process (a) lowers its own session-form create to the
    /// member's OWNER form, (b) refuses duplicates against that owner-form
    /// row in BOTH spellings, and (c) still leaves a genuinely non-member
    /// session in session form (no aliasing exists for it).
    #[tokio::test(flavor = "multi_thread")]
    async fn blind_roster_admission_resolves_members_through_the_shared_session_store() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mob_id = unique_admission_mob_id();
        let (runtime_knows, service, _dispatcher) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let session_id = spawn_helper_member(&runtime_knows).await;
        // Succession through the real lifecycle: the roster-owning incumbent
        // stands down (terminal shutdown releases its supervisor route), then
        // the roster-blind co-runtime bootstraps the SAME mob id ordinarily.
        // The blind plane must share the id - store-based member resolution
        // filters bindings by the resolving runtime's own mob id.
        let knows_service = runtime_knows.session_service().cloned();
        runtime_knows
            .handle()
            .shutdown()
            .await
            .expect("incumbent stands down before the blind co-runtime boots");

        // The "co-process": same mob definition, no members ever spawned.
        let (runtime_blind, _service_b, _dispatcher_b) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        assert!(
            runtime_blind
                .handle()
                .resolve_bridge_session_id_observation(&meerkat_mob::ids::AgentIdentity::from(
                    "helper"
                ))
                .await
                .is_none(),
            "the co-process fixture must not know the member"
        );
        let sidecar = crate::workgraph_admission::workgraph_admission_sidecar_path(dir.path());
        // The blind admission reads the member-knowing runtime's session
        // service: in the real deployment both processes hold their own
        // service instance over ONE shared session store, and
        // `load_persisted_session` is the store read this models.
        let blind = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime_blind.handle(),
            knows_service.clone(),
            Some(sidecar),
        );

        // (a) The blind process's OWN session-form create resolves the
        // member through the shared store and lands owner-form.
        let created = rpc_goal_create(
            &service,
            &blind,
            "session-form in from the blind co-process, owner-form stored",
            serde_json::json!({ "kind": "session", "session_id": session_id.to_string() }),
        )
        .await
        .expect("the blind admission lowers via the shared session store");
        assert_eq!(
            created["attention"]["target"]["kind"],
            serde_json::json!("lowered_owner"),
            "{created:#?}"
        );
        assert_eq!(
            created["attention"]["target"]["owner_key"]["id"],
            serde_json::json!(format!("mob/{mob_id}/agent/helper")),
        );
        let occupant = created["attention"]["binding_id"].as_str().unwrap();

        // (b) Duplicates are refused against the owner-form row in both
        // spellings, still roster-blind.
        let error = rpc_goal_create(
            &service,
            &blind,
            "identity-form duplicate via the blind co-process",
            serde_json::json!({ "kind": "identity", "identity": "helper" }),
        )
        .await
        .expect_err("the blind admission must refuse the identity-form duplicate");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");
        assert!(
            error.message.contains(occupant),
            "conflict must name the occupying binding: {error:?}"
        );
        let error = rpc_goal_create(
            &service,
            &blind,
            "session-form duplicate via the blind co-process",
            serde_json::json!({ "kind": "session", "session_id": session_id.to_string() }),
        )
        .await
        .expect_err("the blind admission must refuse the session-form duplicate");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");

        // (c) A session neither roster nor store knows keeps its session
        // form: it is genuinely not a member, so no aliasing exists.
        let non_member = rpc_goal_create(
            &service,
            &blind,
            "non-member session goal",
            serde_json::json!({ "kind": "session", "session_id": SESSION_FREE }),
        )
        .await
        .expect("a non-member session create is admitted");
        assert_eq!(
            non_member["attention"]["target"]["kind"],
            serde_json::json!("session"),
            "{non_member:#?}"
        );

        runtime_blind.handle().stop().await.expect("stop");
    }

    /// Round-4 Q2 (mid-respawn): the same blind window exists IN-process
    /// while a member is absent from the roster. The owner-form row written
    /// while the member existed must still refuse an identity-form duplicate
    /// after the member has left the roster.
    #[tokio::test(flavor = "multi_thread")]
    async fn occupancy_check_holds_while_the_member_is_absent_from_the_roster() {
        let (runtime, service, _dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let session_id = spawn_helper_member(&runtime).await;
        let admission = runtime.workgraph_admission();

        let created = rpc_goal_create(
            &service,
            &admission,
            "written while the member was rostered",
            serde_json::json!({ "kind": "session", "session_id": session_id.to_string() }),
        )
        .await
        .expect("create goal");
        assert_eq!(
            created["attention"]["target"]["kind"],
            serde_json::json!("lowered_owner"),
            "{created:#?}"
        );

        // The respawn window: the member leaves the roster, its binding
        // stays. An idle member disposes promptly after retire.
        runtime
            .handle()
            .retire(meerkat_mob::ids::AgentIdentity::from("helper"))
            .await
            .expect("retire member");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while runtime
            .handle()
            .resolve_bridge_session_id_observation(&meerkat_mob::ids::AgentIdentity::from("helper"))
            .await
            .is_some()
        {
            assert!(
                std::time::Instant::now() < deadline,
                "retired member should leave the roster"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let error = rpc_goal_create(
            &service,
            &admission,
            "duplicate during the respawn window",
            serde_json::json!({ "kind": "identity", "identity": "helper" }),
        )
        .await
        .expect_err("the occupancy check must not depend on the roster");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");
        runtime.handle().stop().await.expect("stop");
    }

    /// Round-4 Q2 (tool plane): a witnessed reassign whose session target
    /// addresses a roster member forwards with the target lowered to owner
    /// form — the tool plane writes the same spelling as the RPC arms.
    #[tokio::test(flavor = "multi_thread")]
    async fn tool_plane_reassign_lowers_member_session_targets_to_owner_form() {
        let (runtime, service, dispatcher, _dir, mob_id) = bootstrapped_tool_plane().await;
        let session_id = spawn_helper_member(&runtime).await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let context = witness_context(&service, &mover.attention.binding_id).await;

        let onto_member = reassign_args(&mover, &session_id.to_string());
        dispatcher
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-member-target",
                    name: "workgraph_attention_reassign",
                    args: &onto_member,
                },
                &context,
            )
            .await
            .expect("reassign onto the member succeeds");

        let bindings = service
            .list_attention(meerkat::AttentionListRequest::default())
            .await
            .expect("list attention");
        let member_binding = bindings
            .attention
            .iter()
            .find(|binding| matches!(binding.status, meerkat::WorkAttentionStatus::Active))
            .expect("the moved binding is active");
        assert_eq!(
            member_binding
                .target
                .owner_key()
                .expect("owner key")
                .canonical(),
            format!("agent:mob/{mob_id}/agent/helper"),
            "tool-plane writes must store the owner form for members"
        );
        runtime.handle().stop().await.expect("stop");
    }

    // -- Round-4 Q3: witness-less reassigns never take the admission --------

    /// Round-4 Q3: upstream denies a witness-less
    /// `workgraph_attention_reassign` before any store access, on BOTH entry
    /// points (plain `dispatch` funnels into a default, witness-less
    /// context). The wrapper must forward such calls WITHOUT taking the
    /// runtime-wide admission — proven by holding the admission for the
    /// whole test: the witness-less call still completes with upstream's
    /// denial, while a witnessed call queues on the held gate.
    #[tokio::test(flavor = "multi_thread")]
    async fn witnessless_reassign_forwards_without_taking_the_admission() {
        let (runtime, service, dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let admission = runtime.workgraph_admission();
        let held = admission.acquire().await.expect("hold the admission");

        let args = reassign_args(&mover, SESSION_FREE);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            dispatcher.dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-no-witness-context",
                    name: "workgraph_attention_reassign",
                    args: &args,
                },
                &meerkat_core::agent::ToolDispatchContext::default(),
            ),
        )
        .await
        .expect("witness-less reassign must not wait on the held admission")
        .expect_err("upstream denies a witness-less reassign");
        assert!(
            matches!(error, meerkat_core::ToolError::AccessDenied { .. }),
            "expected upstream's access denial, got: {error}"
        );

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            dispatcher.dispatch(meerkat_core::types::ToolCallView {
                id: "call-no-witness-plain",
                name: "workgraph_attention_reassign",
                args: &args,
            }),
        )
        .await
        .expect("plain dispatch must not wait on the held admission either")
        .expect_err("upstream denies the witness-less plain dispatch too");
        assert!(
            matches!(error, meerkat_core::ToolError::AccessDenied { .. }),
            "expected upstream's access denial, got: {error}"
        );

        // A WITNESSED reassign still admits: it must wait on the held gate,
        // then complete once the permit drops.
        let witnessed = {
            let dispatcher = Arc::clone(&dispatcher);
            let context = witness_context(&service, &mover.attention.binding_id).await;
            let args = reassign_args(&mover, SESSION_FREE);
            tokio::spawn(async move {
                dispatcher
                    .dispatch_with_context(
                        meerkat_core::types::ToolCallView {
                            id: "call-witnessed",
                            name: "workgraph_attention_reassign",
                            args: &args,
                        },
                        &context,
                    )
                    .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !witnessed.is_finished(),
            "a witnessed reassign must wait on the held admission"
        );
        drop(held);
        witnessed
            .await
            .expect("join")
            .expect("witnessed reassign admits and succeeds onto the free target");
        runtime.handle().stop().await.expect("stop");
    }

    // -- Round-6: owner-spelled session targets + resolution cost ----------

    /// A member session spelled as an OWNER key — the round-6 bypass form:
    /// same canonical occupancy key as `{kind:"session"}`, different target
    /// variant.
    fn owner_spelled_session_target(session_id: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": "owner",
            "owner_key": { "kind": "session", "id": session_id },
        })
    }

    /// Session-service stand-in for the admission's ONE read seam,
    /// `load_persisted_session`: counts store reads and either delegates to
    /// a real service (the round-6 resolution-cache tests) or fails every
    /// read (the fail-closed tests). Every other session-service surface is
    /// inert — the admission never touches it.
    struct AdmissionStoreProbe {
        delegate: Option<Arc<dyn meerkat_mob::MobSessionService>>,
        loads: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionService for AdmissionStoreProbe {
        async fn create_session(
            &self,
            _req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "create_session".to_string(),
            ))
        }

        async fn start_turn(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StartTurnRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "start_turn".to_string(),
            ))
        }

        async fn interrupt(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }

        async fn read(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError>
        {
            Err(meerkat_core::service::SessionError::NotFound { id: id.clone() })
        }

        async fn list(
            &self,
            _query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
        {
            Ok(Vec::new())
        }

        async fn archive(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceCommsExt for AdmissionStoreProbe {}

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceControlExt for AdmissionStoreProbe {
        async fn append_system_context(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            Err(meerkat_core::service::SessionError::Unsupported(
                "append_system_context".to_string(),
            )
            .into())
        }

        async fn stage_tool_results(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StageToolResultsRequest,
        ) -> Result<
            meerkat_core::service::StageToolResultsResult,
            meerkat_core::service::SessionError,
        > {
            Err(meerkat_core::service::SessionError::Unsupported(
                "stage_tool_results".to_string(),
            ))
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for AdmissionStoreProbe {
        async fn read_history(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
        {
            Err(meerkat_core::service::SessionError::NotFound { id: id.clone() })
        }
    }

    #[async_trait::async_trait]
    impl meerkat_mob::MobSessionService for AdmissionStoreProbe {
        async fn subscribe_session_activity(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
            // This double's sessions never report an active turn.
            Ok(meerkat_mob::MemberSessionActivity::inactive())
        }

        // In-memory double (meerkat 0.8.47): export visibility is its durable source.
        async fn observe_live_durable_source(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
            meerkat_mob::observe_live_durable_source_via_projection_visibility(self, session_id)
                .await
        }

        // In-memory double: `read` is its published state (meerkat 0.8.45).
        async fn observe_member_status_view(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
            meerkat_mob::observe_member_status_view_via_read(self, session_id).await
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, meerkat_core::SessionError>
        {
            Err(meerkat_core::SessionError::Unsupported(
                "admission probe has no Live commit authority".into(),
            ))
        }

        // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED.
        // This double owns no durable transcript, so it refuses explicitly rather
        // than inheriting a forward it cannot honour.
        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::types::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, meerkat_core::SessionError>
        {
            Err(meerkat_core::SessionError::Unsupported(
                "admission probe has no live final transcript turn-boundary commit authority"
                    .into(),
            ))
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            _source_session_id: &meerkat_core::types::SessionId,
            _message_count: Option<usize>,
            _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            _target: meerkat_core::DurableSessionForkTarget,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, meerkat_core::SessionError> {
            Err(meerkat_core::SessionError::Unsupported(
                "admission probe has no durable turn-boundary fork authority".into(),
            ))
        }

        // meerkat 0.8.30 made `enqueue_committed_parent_session_boundary_after_runtime_turn`
        // REQUIRED, deleting the default that returned `Unsupported` for a
        // persistent profile and `Ok(0)` otherwise. That default is exactly how
        // production wrappers silently inherited a hole that only a Turbo S
        // execution red found. This double is non-persistent, so `Ok(0)` is the
        // correct behaviour - but it is guarded rather than bare: a bare `Ok(0)`
        // is indistinguishable at the call site from "projected nothing because
        // there was nothing to project", so nothing could ever assert the
        // difference if this double later became persistent.
        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            session_id: &meerkat_core::types::SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, meerkat_core::service::SessionError> {
            if self.supports_persistent_sessions() {
                return Err(meerkat_core::service::SessionError::Unsupported(format!(
                    "AdmissionStoreProbe became persistent and must project the committed \
                     parent-session boundary through canonical session ownership \
                     for {session_id}"
                )));
            }
            Ok(0)
        }

        // 0.8.22 made `materialize_session_resume_verdict` REQUIRED, deliberately
        // without a default, so that a PERSISTENT decorator cannot inherit a
        // composition that converges nothing and silently resume from stale
        // committed authority after a power cut. This probe is non-persistent, so
        // it opts in EXPLICITLY through the public helper rather than hand-rolling
        // the composition. The helper is fail-closed: it refuses if
        // `supports_persistent_sessions()` is ever true here.
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, meerkat_core::service::SessionError>
        {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }
        // meerkat 0.8.22 deleted `prepare_session_for_resume` from this trait.
        // Durable-tail convergence now lives inside `PersistentSessionService`'s
        // OVERRIDE of `materialize_session_resume_verdict`; the trait default
        // routes to `materialize_nonpersistent_session_resume_verdict`, which
        // only re-observes authority around `load_session_for_resume`.
        //
        // Inheriting that default is TRUTHFUL here, and only here, because this
        // probe is a direct implementer with no durable tail of its own: its
        // `delegate` is consulted for `load_persisted_session` alone, and every
        // construction site passes an `EphemeralSessionService`. A wrapper over
        // a persistent inner service must NOT inherit the default - it would
        // compile while silently dropping convergence.
        async fn observe_session_resume_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, meerkat_core::service::SessionError>
        {
            // Test double: truthfully an empty authority bundle (the ephemeral
            // arm of the meerkat 0.8.22 resume-verdict contract).
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }
        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "test double does not acknowledge store-owned runtime boundaries".to_string(),
            ))
        }
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ResumeSessionLoad, meerkat_core::service::SessionError> {
            // Truthful derived answer: this double's resume visibility IS its
            // typed reads' visibility (the meerkat-mob test-double idiom).
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(meerkat_mob::ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            meerkat_core::SessionService::create_session(self, req).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            meerkat_core::SessionService::archive(self, session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }

        async fn load_persisted_session(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::session::Session>, meerkat_core::service::SessionError>
        {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &self.delegate {
                Some(inner) => inner.load_persisted_session(session_id).await,
                None => Err(meerkat_core::service::SessionError::Unsupported(
                    "simulated session-store outage".to_string(),
                )),
            }
        }
    }

    /// Adoption simulator for the round-7 memo test: routes
    /// `load_persisted_session` to one of two probes based on a flag, so a
    /// single session id can flip non-member → member (and back) exactly the
    /// way `resume_session` adoption re-stamps the binding.
    struct SwitchableStore {
        probe_pre: AdmissionStoreProbe,
        probe_post: AdmissionStoreProbe,
        adopted: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionService for SwitchableStore {
        async fn create_session(
            &self,
            _req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "create_session".to_string(),
            ))
        }

        async fn start_turn(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StartTurnRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "start_turn".to_string(),
            ))
        }

        async fn interrupt(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }

        async fn read(
            &self,
            id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError>
        {
            Err(meerkat_core::service::SessionError::NotFound { id: id.clone() })
        }

        async fn list(
            &self,
            _query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
        {
            Ok(Vec::new())
        }

        async fn archive(
            &self,
            _id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceCommsExt for SwitchableStore {}

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceControlExt for SwitchableStore {
        async fn append_system_context(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            Err(meerkat_core::service::SessionError::Unsupported(
                "append_system_context".to_string(),
            )
            .into())
        }

        async fn stage_tool_results(
            &self,
            _id: &meerkat_core::types::SessionId,
            _req: meerkat_core::service::StageToolResultsRequest,
        ) -> Result<
            meerkat_core::service::StageToolResultsResult,
            meerkat_core::service::SessionError,
        > {
            Err(meerkat_core::service::SessionError::Unsupported(
                "stage_tool_results".to_string(),
            ))
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for SwitchableStore {
        async fn read_history(
            &self,
            id: &meerkat_core::types::SessionId,
            _query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
        {
            Err(meerkat_core::service::SessionError::NotFound { id: id.clone() })
        }
    }

    #[async_trait::async_trait]
    impl meerkat_mob::MobSessionService for SwitchableStore {
        async fn subscribe_session_activity(
            &self,
            _session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberSessionActivity, meerkat_core::SessionError> {
            // This double's sessions never report an active turn.
            Ok(meerkat_mob::MemberSessionActivity::inactive())
        }

        // In-memory double (meerkat 0.8.47): export visibility is its durable source.
        async fn observe_live_durable_source(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::LiveDurableSourceObservation, meerkat_core::SessionError> {
            meerkat_mob::observe_live_durable_source_via_projection_visibility(self, session_id)
                .await
        }

        // In-memory double: `read` is its published state (meerkat 0.8.45).
        async fn observe_member_status_view(
            &self,
            session_id: &meerkat_core::SessionId,
        ) -> Result<meerkat_mob::MemberStatusSessionView, meerkat_core::SessionError> {
            meerkat_mob::observe_member_status_view_via_read(self, session_id).await
        }

        async fn commit_live_delegation_final_transcript(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, meerkat_core::SessionError>
        {
            Err(meerkat_core::SessionError::Unsupported(
                "switchable store has no Live commit authority".into(),
            ))
        }

        // meerkat 0.8.41 made `fork_persisted_session_at_turn_boundary` REQUIRED.
        // This double owns no durable transcript, so it refuses explicitly rather
        // than inheriting a forward it cannot honour.
        async fn commit_live_delegation_final_transcript_at_turn_boundary(
            &self,
            _machine: &meerkat_runtime::MeerkatMachine,
            _session_id: &meerkat_core::types::SessionId,
            _provisional: meerkat_core::ProvisionalLiveHandoff,
            _final_event: meerkat_core::RealtimeTranscriptEvent,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, meerkat_core::SessionError>
        {
            Err(meerkat_core::SessionError::Unsupported(
                "switchable store has no live final transcript turn-boundary commit authority"
                    .into(),
            ))
        }

        async fn fork_persisted_session_at_turn_boundary(
            &self,
            _source_session_id: &meerkat_core::types::SessionId,
            _message_count: Option<usize>,
            _tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
            _target: meerkat_core::DurableSessionForkTarget,
            _bound: std::time::Duration,
        ) -> Result<meerkat_core::DurableForkAtTurnBoundary, meerkat_core::SessionError> {
            Err(meerkat_core::SessionError::Unsupported(
                "switchable store has no durable turn-boundary fork authority".into(),
            ))
        }

        // meerkat 0.8.30 made `enqueue_committed_parent_session_boundary_after_runtime_turn`
        // REQUIRED, deleting the default that returned `Unsupported` for a
        // persistent profile and `Ok(0)` otherwise. That default is exactly how
        // production wrappers silently inherited a hole that only a Turbo S
        // execution red found. This double is non-persistent, so `Ok(0)` is the
        // correct behaviour - but it is guarded rather than bare: a bare `Ok(0)`
        // is indistinguishable at the call site from "projected nothing because
        // there was nothing to project", so nothing could ever assert the
        // difference if this double later became persistent.
        async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
            &self,
            session_id: &meerkat_core::types::SessionId,
            _runtime_adapter: &meerkat_runtime::MeerkatMachine,
        ) -> Result<usize, meerkat_core::service::SessionError> {
            if self.supports_persistent_sessions() {
                return Err(meerkat_core::service::SessionError::Unsupported(format!(
                    "SwitchableStore became persistent and must project the committed \
                     parent-session boundary through canonical session ownership \
                     for {session_id}"
                )));
            }
            Ok(0)
        }

        // 0.8.22 made `materialize_session_resume_verdict` REQUIRED, deliberately
        // without a default, so that a PERSISTENT decorator cannot inherit a
        // composition that converges nothing and silently resume from stale
        // committed authority after a power cut. This probe is non-persistent, so
        // it opts in EXPLICITLY through the public helper rather than hand-rolling
        // the composition. The helper is fail-closed: it refuses if
        // `supports_persistent_sessions()` is ever true here.
        async fn materialize_session_resume_verdict(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeVerdict, meerkat_core::service::SessionError>
        {
            meerkat_mob::materialize_nonpersistent_session_resume_verdict(self, session_id).await
        }
        // Same 0.8.22 transition as `AdmissionStoreProbe` above:
        // `prepare_session_for_resume` is gone from the trait, and inheriting
        // the non-persistent `materialize_session_resume_verdict` default is
        // truthful because this switch only routes `load_persisted_session`
        // between two ephemeral-backed probes - it owns no durable tail to
        // converge.
        async fn observe_session_resume_authority(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::SessionResumeAuthority, meerkat_core::service::SessionError>
        {
            // Test double: truthfully an empty authority bundle (the ephemeral
            // arm of the meerkat 0.8.22 resume-verdict contract).
            Ok(meerkat_mob::SessionResumeAuthority::default())
        }
        async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
            _authority: &meerkat_core::CommittedSessionBoundaryAuthority,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Err(meerkat_core::service::SessionError::Unsupported(
                "test double does not acknowledge store-owned runtime boundaries".to_string(),
            ))
        }
        async fn load_session_for_resume(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<meerkat_mob::ResumeSessionLoad, meerkat_core::service::SessionError> {
            // Truthful derived answer: this double's resume visibility IS its
            // typed reads' visibility (the meerkat-mob test-double idiom).
            if let Some(session) = self.load_persisted_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Active(Box::new(session)));
            }
            if let Some(session) = self.load_revivable_retired_session(session_id).await? {
                return Ok(meerkat_mob::ResumeSessionLoad::Revivable(Box::new(session)));
            }
            Ok(meerkat_mob::ResumeSessionLoad::Absent)
        }

        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::types::RunResult, meerkat_core::service::SessionError> {
            meerkat_core::SessionService::create_session(self, req).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            meerkat_core::SessionService::archive(self, session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            _session_id: &meerkat_core::types::SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Ok(())
        }

        async fn load_persisted_session(
            &self,
            session_id: &meerkat_core::types::SessionId,
        ) -> Result<Option<meerkat_core::session::Session>, meerkat_core::service::SessionError>
        {
            if self.adopted.load(std::sync::atomic::Ordering::SeqCst) {
                self.probe_post.load_persisted_session(session_id).await
            } else {
                self.probe_pre.load_persisted_session(session_id).await
            }
        }
    }

    /// Round-6 T1 (RPC write side): a member session spelled as an owner key
    /// must lower to the member's agent-owner form on `goal/create` — stored
    /// verbatim it would be a session-spelled `LoweredOwner` row invisible
    /// to identity-form occupancy checks in a roster-blind process. The
    /// lowered row then conflicts with an identity-form duplicate.
    #[tokio::test(flavor = "multi_thread")]
    async fn owner_spelled_member_session_lowers_to_owner_form_on_goal_create() {
        let (runtime, service, _dispatcher, _dir, mob_id) = bootstrapped_tool_plane().await;
        let session_id = spawn_helper_member(&runtime).await;
        let admission = runtime.workgraph_admission();

        let created = rpc_goal_create(
            &service,
            &admission,
            "owner-spelled member session in, owner form stored",
            owner_spelled_session_target(&session_id.to_string()),
        )
        .await
        .expect("an owner-spelled member session target is admitted");
        assert_eq!(
            created["attention"]["target"]["kind"],
            serde_json::json!("lowered_owner"),
            "{created:#?}"
        );
        assert_eq!(
            created["attention"]["target"]["owner_key"]["kind"],
            serde_json::json!("agent"),
            "{created:#?}"
        );
        assert_eq!(
            created["attention"]["target"]["owner_key"]["id"],
            serde_json::json!(format!("mob/{mob_id}/agent/helper")),
        );

        let error = rpc_goal_create(
            &service,
            &admission,
            "identity-form duplicate",
            serde_json::json!({ "kind": "identity", "identity": "helper" }),
        )
        .await
        .expect_err("the lowered row must conflict with an identity-form duplicate");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");
        runtime.handle().stop().await.expect("stop");
    }

    /// Round-6 T1 (tool plane): the same owner-spelled bypass through
    /// `workgraph_attention_reassign`'s target args must forward with the
    /// target lowered to the member's agent-owner form.
    #[tokio::test(flavor = "multi_thread")]
    async fn tool_plane_reassign_lowers_owner_spelled_member_session_targets() {
        let (runtime, service, dispatcher, _dir, mob_id) = bootstrapped_tool_plane().await;
        let session_id = spawn_helper_member(&runtime).await;
        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let context = witness_context(&service, &mover.attention.binding_id).await;

        let onto_owner_spelled = serde_json::value::RawValue::from_string(
            serde_json::json!({
                "binding_id": mover.attention.binding_id,
                "expected_revision": mover.attention.machine_state.revision,
                "target": owner_spelled_session_target(&session_id.to_string()),
            })
            .to_string(),
        )
        .expect("reassign args");
        dispatcher
            .dispatch_with_context(
                meerkat_core::types::ToolCallView {
                    id: "call-owner-spelled-member",
                    name: "workgraph_attention_reassign",
                    args: &onto_owner_spelled,
                },
                &context,
            )
            .await
            .expect("reassign onto the owner-spelled member session succeeds");

        let bindings = service
            .list_attention(meerkat::AttentionListRequest::default())
            .await
            .expect("list attention");
        let member_binding = bindings
            .attention
            .iter()
            .find(|binding| matches!(binding.status, meerkat::WorkAttentionStatus::Active))
            .expect("the moved binding is active");
        assert_eq!(
            member_binding
                .target
                .owner_key()
                .expect("owner key")
                .canonical(),
            format!("agent:mob/{mob_id}/agent/helper"),
            "the owner-spelled session key must not reach the store verbatim"
        );
        runtime.handle().stop().await.expect("stop");
    }

    /// Round-6 T1 (the two-row duplicate): before the fix, an owner-spelled
    /// member session row was stored session-spelled, and a roster-BLIND
    /// admission then admitted an identity-form create for the same member -
    /// two occupying rows on one member. That is exactly the cross-spelling
    /// residual the upstream store guard does NOT catch (its two `target_key`
    /// spellings differ), so meta still owns this refusal; since ask 25 the
    /// consequence of losing it is a silently starved goal rather than the
    /// former hard turn failure. Both writes now normalize through the shared
    /// session store, so the second conflicts. Uses the `lowered_owner` wire
    /// alias for the first row - both owner spellings must canonicalize.
    #[tokio::test(flavor = "multi_thread")]
    async fn owner_spelled_session_rows_cannot_reopen_the_blind_duplicate_window() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mob_id = unique_admission_mob_id();
        let (runtime_knows, service, _dispatcher) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let session_id = spawn_helper_member(&runtime_knows).await;
        // Succession through the real lifecycle: the roster-owning incumbent
        // stands down (terminal shutdown releases its supervisor route), then
        // the roster-blind co-runtime bootstraps the SAME mob id ordinarily.
        // The blind plane must share the id - store-based member resolution
        // filters bindings by the resolving runtime's own mob id.
        let knows_service = runtime_knows.session_service().cloned();
        runtime_knows
            .handle()
            .shutdown()
            .await
            .expect("incumbent stands down before the blind co-runtime boots");
        let (runtime_blind, _service_b, _dispatcher_b) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let blind = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime_blind.handle(),
            knows_service.clone(),
            None,
        );

        let created = rpc_goal_create(
            &service,
            &blind,
            "owner-spelled member session via the blind co-process",
            serde_json::json!({
                "kind": "lowered_owner",
                "owner_key": { "kind": "session", "id": session_id.to_string() },
            }),
        )
        .await
        .expect("the blind admission canonicalizes the owner-spelled session key");
        assert_eq!(
            created["attention"]["target"]["owner_key"]["id"],
            serde_json::json!(format!("mob/{mob_id}/agent/helper")),
            "{created:#?}"
        );
        let occupant = created["attention"]["binding_id"].as_str().unwrap();

        let error = rpc_goal_create(
            &service,
            &blind,
            "identity-form second row of the round-6 duplicate",
            serde_json::json!({ "kind": "identity", "identity": "helper" }),
        )
        .await
        .expect_err("the identity-form create must conflict, not duplicate the binding");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");
        assert!(
            error.message.contains(occupant),
            "conflict must name the occupying binding: {error:?}"
        );

        runtime_blind.handle().stop().await.expect("stop");
    }

    /// Round-6 T1 (non-member): a session-kind owner key for a session
    /// neither roster nor store knows behaves exactly like a plain session
    /// target — stored in the canonical `{kind:"session"}` arm (same
    /// occupancy key, so the plain spelling conflicts against it), and a
    /// session-kind owner key whose id is not a session id is refused.
    #[tokio::test(flavor = "multi_thread")]
    async fn non_member_session_kind_owner_keys_canonicalize_to_plain_session_targets() {
        let (runtime, service, _dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let admission = runtime.workgraph_admission();

        let created = rpc_goal_create(
            &service,
            &admission,
            "non-member owner-spelled session goal",
            owner_spelled_session_target(SESSION_FREE),
        )
        .await
        .expect("a non-member owner-spelled session create is admitted");
        assert_eq!(
            created["attention"]["target"]["kind"],
            serde_json::json!("session"),
            "{created:#?}"
        );
        assert_eq!(
            created["attention"]["target"]["session_id"],
            serde_json::json!(SESSION_FREE),
        );

        let error = rpc_goal_create(
            &service,
            &admission,
            "plain session duplicate",
            serde_json::json!({ "kind": "session", "session_id": SESSION_FREE }),
        )
        .await
        .expect_err("the plain spelling must conflict with the canonicalized row");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_CONFLICT_CODE, "{error:?}");

        let error = rpc_goal_create(
            &service,
            &admission,
            "session-kind owner key with a garbage id",
            serde_json::json!({
                "kind": "owner",
                "owner_key": { "kind": "session", "id": "not-a-session-id" },
            }),
        )
        .await
        .expect_err("an unparseable session-kind owner id must be refused");
        assert_eq!(error.code, -32602, "{error:?}");
        assert!(
            error.message.contains("does not parse as a session id"),
            "{error:?}"
        );
        runtime.handle().stop().await.expect("stop");
    }

    /// Round-6 T2 + round-7 correction: `load_persisted_session`
    /// deserializes the FULL session — multi-GB for production-profile eternal
    /// members — under the runtime-wide gate, so POSITIVE resolutions are
    /// memoized (TTL-bounded). Session ADOPTION is legitimate (a
    /// free-floating session, or another mob's member, resumed into a
    /// member build re-stamps the binding), so NEGATIVE results are never
    /// cached: each non-member lookup re-reads the store and immediately
    /// observes an adoption.
    #[tokio::test(flavor = "multi_thread")]
    async fn member_resolution_is_cached_after_the_first_store_read() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mob_id = unique_admission_mob_id();
        let (runtime_knows, _service, _dispatcher) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let session_id = spawn_helper_member(&runtime_knows).await;
        // Succession through the real lifecycle: the roster-owning incumbent
        // stands down (terminal shutdown releases its supervisor route), then
        // the roster-blind co-runtime bootstraps the SAME mob id ordinarily.
        // The blind plane must share the id - store-based member resolution
        // filters bindings by the resolving runtime's own mob id.
        let knows_service = runtime_knows.session_service().cloned();
        runtime_knows
            .handle()
            .shutdown()
            .await
            .expect("incumbent stands down before the blind co-runtime boots");
        let (runtime_blind, _service_b, _dispatcher_b) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let probe: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(AdmissionStoreProbe {
            delegate: knows_service.clone(),
            loads: Arc::clone(&loads),
        });
        let admission = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime_blind.handle(),
            Some(probe),
            None,
        );

        let member_target = meerkat::GoalAttentionTarget::Session {
            session_id: session_id.clone(),
        };
        let first = admission
            .lower_member_session_target(member_target.clone())
            .await
            .expect("first resolution lowers through the store");
        assert!(
            matches!(
                &first,
                meerkat::GoalAttentionTarget::Owner { owner_key }
                    if owner_key.id == format!("mob/{mob_id}/agent/helper")
            ),
            "{first:?}"
        );
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);

        let second = admission
            .lower_member_session_target(member_target)
            .await
            .expect("second resolution lowers from the cache");
        assert_eq!(second, first);
        assert_eq!(
            loads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the second resolution must not call load_persisted_session"
        );

        let non_member_target = meerkat::GoalAttentionTarget::Session {
            session_id: meerkat_core::SessionId::parse(SESSION_FREE).expect("session id"),
        };
        for _ in 0..2 {
            let kept = admission
                .lower_member_session_target(non_member_target.clone())
                .await
                .expect("a non-member session keeps its session form");
            assert_eq!(kept, non_member_target);
        }
        assert_eq!(
            loads.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "negative resolutions are NEVER cached — adoption can turn a \
             non-member session into a member session at any moment"
        );

        runtime_blind.handle().stop().await.expect("stop");
    }

    /// Round-7 (final gate): the adoption-resume flow is legitimate — a
    /// plain session (or another mob's member session) can be resumed INTO a
    /// member build, re-stamping `mob_member_binding`. A stale negative memo
    /// would pin it as non-member and re-open the roster-blind duplicate
    /// window; a stale positive would lower to an outdated identity after an
    /// adoption-away. Negatives are uncached (adoption observed on the very
    /// next lookup); positives expire after the TTL.
    #[tokio::test(flavor = "multi_thread")]
    async fn adoption_flips_are_observed_because_negatives_are_uncached_and_positives_expire() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mob_id = unique_admission_mob_id();
        let (runtime_knows, _service, _dispatcher) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let member_session = spawn_helper_member(&runtime_knows).await;
        // Succession through the real lifecycle: the roster-owning incumbent
        // stands down (terminal shutdown releases its supervisor route), then
        // the roster-blind co-runtime bootstraps the SAME mob id ordinarily.
        // The blind plane must share the id - store-based member resolution
        // filters bindings by the resolving runtime's own mob id.
        let knows_service = runtime_knows.session_service().cloned();
        runtime_knows
            .handle()
            .shutdown()
            .await
            .expect("incumbent stands down before the blind co-runtime boots");
        let (runtime_blind, _service_b, _dispatcher_b) =
            durable_tool_plane_for(&mob_id, dir.path()).await;
        let loads = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        // Switchable delegate: starts BLANK (session unknown = non-member),
        // then "adopts" by switching to the member-knowing store — the same
        // session id transitions non-member → member, as resume_session does.
        let adopted = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Pre-adoption blankness must be a service that has never seen the
        // session. The blind plane now shares the durable store with the
        // stood-down incumbent (that sharing IS the co-process wiring), so
        // its own service already reads the binding; a fresh empty ephemeral
        // service is the honest "not adopted yet" view.
        let blank_pre_adoption: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(
            meerkat_session::EphemeralSessionService::new(test_builder(dir.path()), 4),
        );
        let store: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(SwitchableStore {
            probe_pre: AdmissionStoreProbe {
                delegate: Some(blank_pre_adoption),
                loads: Arc::clone(&loads),
            },
            probe_post: AdmissionStoreProbe {
                delegate: knows_service.clone(),
                loads: Arc::clone(&loads),
            },
            adopted: Arc::clone(&adopted),
        });
        let admission = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime_blind.handle(),
            Some(store),
            None,
        )
        .with_member_resolution_ttl(std::time::Duration::ZERO);

        let target = meerkat::GoalAttentionTarget::Session {
            session_id: member_session.clone(),
        };
        // Pre-adoption: non-member, stays session-form.
        let kept = admission
            .lower_member_session_target(target.clone())
            .await
            .expect("pre-adoption lookup");
        assert_eq!(kept, target, "not yet a member of this mob");

        // Adoption happens (same session id now carries the binding).
        adopted.store(true, std::sync::atomic::Ordering::SeqCst);
        let lowered = admission
            .lower_member_session_target(target.clone())
            .await
            .expect("post-adoption lookup");
        assert!(
            matches!(
                &lowered,
                meerkat::GoalAttentionTarget::Owner { owner_key }
                    if owner_key.id == format!("mob/{mob_id}/agent/helper")
            ),
            "the very next lookup observes the adoption: {lowered:?}"
        );

        // Adoption-away: with a zero TTL the positive entry expires
        // immediately, so the reverse flip is observed on the next lookup
        // too (a real deployment bounds this by MEMBER_RESOLUTION_TTL).
        adopted.store(false, std::sync::atomic::Ordering::SeqCst);
        let reverted = admission
            .lower_member_session_target(target.clone())
            .await
            .expect("post-adoption-away lookup");
        assert_eq!(reverted, target, "expired positive re-reads the store");

        runtime_blind.handle().stop().await.expect("stop");
    }

    /// Round-6 T4: pins the fail-closed posture. A session-store read
    /// failure on a roster-miss session target must REFUSE `goal/create`
    /// and `attention/reassign` with the store error — admitting in session
    /// form instead would silently re-open the roster-blind aliasing hole
    /// the store fallback exists to plug.
    #[tokio::test(flavor = "multi_thread")]
    async fn session_store_read_failure_refuses_admission_instead_of_writing_session_form() {
        let (runtime, service, _dispatcher, _dir, _mob_id) = bootstrapped_tool_plane().await;
        let probe: Arc<dyn meerkat_mob::MobSessionService> = Arc::new(AdmissionStoreProbe {
            delegate: None,
            loads: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let admission = crate::workgraph_admission::WorkGraphAdmission::new(
            runtime.handle(),
            Some(probe),
            None,
        );

        for target in [
            serde_json::json!({ "kind": "session", "session_id": SESSION_FREE }),
            owner_spelled_session_target(SESSION_FREE),
        ] {
            let error = rpc_goal_create(
                &service,
                &admission,
                "create against a failing session store",
                target,
            )
            .await
            .expect_err("a store read failure must refuse the create");
            assert_eq!(error.code, crate::rpc::WORKGRAPH_ERROR_CODE, "{error:?}");
            assert!(
                error.message.contains("could not read session"),
                "{error:?}"
            );
            assert!(
                error.message.contains("simulated session-store outage"),
                "the refusal must carry the store error: {error:?}"
            );
        }
        let bindings = service
            .list_attention(meerkat::AttentionListRequest::default())
            .await
            .expect("list attention");
        assert!(
            bindings.attention.is_empty(),
            "a refused create must not write any binding: {:?}",
            bindings.attention
        );

        let mover = create_session_goal(
            &service,
            "mover",
            SESSION_MOVER,
            meerkat::WorkAttentionMode::Coordinate,
        )
        .await;
        let error = crate::rpc::workgraph_methods::handle_workgraph_method(
            Some(&service),
            &admission,
            crate::rpc::workgraph_methods::WorkgraphSurface::HostStdin,
            "meta/workgraph/attention/reassign",
            &serde_json::json!({
                "binding_id": mover.attention.binding_id,
                "expected_revision": mover.attention.machine_state.revision,
                "target": { "kind": "session", "session_id": SESSION_FREE },
            }),
        )
        .await
        .expect_err("a store read failure must refuse the reassign");
        assert_eq!(error.code, crate::rpc::WORKGRAPH_ERROR_CODE, "{error:?}");
        assert!(
            error.message.contains("could not read session"),
            "{error:?}"
        );
        let bindings = service
            .list_attention(meerkat::AttentionListRequest::default())
            .await
            .expect("list attention");
        assert_eq!(bindings.attention.len(), 1, "{:?}", bindings.attention);
        assert_eq!(
            bindings.attention[0]
                .target
                .owner_key()
                .expect("owner key")
                .canonical(),
            format!("session:{SESSION_MOVER}"),
            "the refused reassign must not have moved the binding"
        );
        runtime.handle().stop().await.expect("stop");
    }

    /// The persistent spec (SQLite-backed store, shareable across processes)
    /// must place the admission sidecar beside the store and register the
    /// tool-plane slot; ephemeral (memory-backed, single-process) specs must
    /// register the slot but NOT the sidecar.
    #[tokio::test]
    async fn spec_constructors_configure_the_admission_sidecar_per_store_kind() {
        let dir = tempfile::tempdir().expect("temp dir");
        let spec = crate::MobBootstrapSpec::persistent(
            admission_definition(&unique_admission_mob_id()),
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            dir.path().to_path_buf(),
            8,
        )
        .await
        .expect("persistent spec");
        assert_eq!(
            spec.workgraph_admission_sidecar,
            Some(crate::workgraph_admission::workgraph_admission_sidecar_path(dir.path())),
            "sqlite-backed store gets the sidecar beside it"
        );
        assert_eq!(
            spec.workgraph_admission_slots.len(),
            1,
            "the tool-plane admission slot must travel on the spec"
        );

        let builder = test_builder(dir.path());
        let (service, slot) = attach_workgraph_tools_ephemeral(&builder, "spec-check-realm");
        let session_service: Arc<dyn meerkat_mob::MobSessionService> =
            Arc::new(meerkat_session::EphemeralSessionService::new(builder, 8));
        let ephemeral_spec = crate::MobBootstrapSpec::new(
            admission_definition(&unique_admission_mob_id()),
            meerkat_mob::MobStorage::in_memory(),
            session_service,
        )
        .with_workgraph_service(Some(service))
        .with_workgraph_admission_slot(slot);
        assert_eq!(
            ephemeral_spec.workgraph_admission_sidecar, None,
            "memory-backed runtimes are single-process; no sidecar"
        );
        assert_eq!(ephemeral_spec.workgraph_admission_slots.len(), 1);
    }
}
