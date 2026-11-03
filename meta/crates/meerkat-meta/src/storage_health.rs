//! Composition-time storage durability resolution (H1/H2 hotfixes plus the
//! M4 per-slot census of the storage-unification arc).
//!
//! Provider-backed durable slots resolve to a declared backend or a startup
//! error. A bare caller-injected WorkGraph store has no durability
//! declaration; its census entry is unverified rather than inferred from kind.
//! This module carries the vocabulary:
//!
//! - **Blobs (H1)**: [`BlobDurability`] records what the blob slot resolved
//!   to; [`BlobStoreResolutionError`] is the fail-closed startup error a
//!   persistent-mode runtime returns instead of the former silent in-memory
//!   fallback.
//! - **Session persistence (H2)**: [`probe_session_store_incremental`]
//!   duplicates the capability probe `PersistentSessionService` runs
//!   privately, so the whole-blob degradation it silently accepts is logged
//!   at startup and visible on the health surfaces.
//! - **Runtime store (M4)**: [`RuntimeStoreResolutionError`] is the
//!   fail-closed startup error replacing the former silent
//!   `SqliteRuntimeStore` → `InMemoryRuntimeStore` fallback; an in-memory
//!   runtime store is constructible only by declaration.
//! - **Per-slot census (M4)**: [`StorageSlotSummary`] records what every
//!   composed storage slot resolved to (backend, durability class,
//!   resolution, sanctioned degradations). [`StorageSlotDurability`] preserves
//!   meerkat's [`meerkat_core::DurabilityDeclaration`] when present and records
//!   an unverified resolution when a caller supplied no declaration.
//!
//! The resolved [`ResolvedStorageSummary`] rides the bootstrap spec onto the
//! runtime and is reported by `meta/status` / `meta/capabilities`.

use std::path::PathBuf;
use std::sync::Arc;

use meerkat::SessionStore;
use meerkat_core::{DurabilityClass, DurabilityDeclaration, DurabilityResolution};

/// How the runtime's blob slot was resolved at composition time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobDurability {
    /// Disk-backed object store under the state directory.
    PersistentDisk,
    /// In-memory blobs as an explicitly declared choice — the ephemeral
    /// launch modes, or `UnifiedRuntimeBuilder::ephemeral_blobs(true)`.
    DeclaredEphemeral,
    /// Caller-injected blob store; `persistent` mirrors its
    /// `is_persistent()` report.
    Custom { persistent: bool },
}

impl BlobDurability {
    /// Stable wire spelling used by the health surfaces.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PersistentDisk => "persistent_disk",
            Self::DeclaredEphemeral => "declared_ephemeral",
            Self::Custom { .. } => "custom",
        }
    }

    /// Whether the resolved blob store survives process restart.
    pub fn is_persistent(&self) -> bool {
        match self {
            Self::PersistentDisk => true,
            Self::DeclaredEphemeral => false,
            Self::Custom { persistent } => *persistent,
        }
    }
}

/// Durability evidence available for a composed storage slot.
///
/// This is Meta's census vocabulary. An unverified injection does not add
/// a resolution to Meerkat's provider declaration or bypass its validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageSlotDurability {
    /// The selected composition or provider supplied this declaration.
    Declared(DurabilityDeclaration),
    /// The slot's domain and class are known, but its injected store supplied
    /// no durability declaration. Backend kind is not durability evidence.
    Unverified {
        domain: String,
        class: DurabilityClass,
    },
}

impl StorageSlotDurability {
    /// The domain this census entry describes.
    pub fn domain(&self) -> &str {
        match self {
            Self::Declared(declaration) => &declaration.domain,
            Self::Unverified { domain, .. } => domain,
        }
    }

    /// The meaning of losing this domain's contents, independent of backend.
    pub fn class(&self) -> DurabilityClass {
        match self {
            Self::Declared(declaration) => declaration.class,
            Self::Unverified { class, .. } => *class,
        }
    }

    /// The selected store's declaration, if one was supplied.
    pub fn declaration(&self) -> Option<&DurabilityDeclaration> {
        match self {
            Self::Declared(declaration) => Some(declaration),
            Self::Unverified { .. } => None,
        }
    }

    fn resolution_json(&self) -> serde_json::Value {
        match self {
            Self::Declared(declaration) => {
                serde_json::to_value(declaration.resolution).unwrap_or(serde_json::Value::Null)
            }
            Self::Unverified { .. } => serde_json::Value::String("unverified".to_string()),
        }
    }
}

/// One composed storage slot's durability evidence, concrete backend and
/// any sanctioned degradation detail, recorded at composition time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageSlotSummary {
    /// The selected store's declaration or explicit absence of one.
    pub durability: StorageSlotDurability,
    /// Human-readable backend name (`"SqliteRuntimeStore"`,
    /// `"InMemoryConsoleLogStore (declared default)"`, ...).
    pub backend: String,
    /// Extra context: the sanctioned boot-without degradation reason, the
    /// declared-default rationale, or a provider note.
    pub detail: Option<String>,
    /// True for the sanctioned boot-without degradations (schedule /
    /// workgraph store open failure): the feature is disabled and the slot
    /// is health-visible instead of a warn line, per the storage plan.
    pub degraded: bool,
}

impl StorageSlotSummary {
    /// A slot backed by persistent storage.
    pub fn persistent(domain: &str, backend: impl Into<String>) -> Self {
        Self {
            durability: StorageSlotDurability::Declared(DurabilityDeclaration::durable(
                domain,
                DurabilityResolution::Persistent,
            )),
            backend: backend.into(),
            detail: None,
            degraded: false,
        }
    }

    /// A durable-class slot resolving non-persistent as an explicit,
    /// documented choice (declared ephemeral / declared default).
    pub fn declared_ephemeral(
        domain: &str,
        backend: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            durability: StorageSlotDurability::Declared(DurabilityDeclaration::durable(
                domain,
                DurabilityResolution::DeclaredEphemeral,
            )),
            backend: backend.into(),
            detail: Some(detail.into()),
            degraded: false,
        }
    }

    /// The sanctioned boot-without degradation (schedule / workgraph store
    /// open failure): the feature is disabled, the slot resolves
    /// non-persistent, and the record is health-visible.
    pub fn degraded(domain: &str, detail: impl Into<String>) -> Self {
        Self {
            durability: StorageSlotDurability::Declared(DurabilityDeclaration::durable(
                domain,
                DurabilityResolution::NonPersistent,
            )),
            backend: "disabled".to_string(),
            detail: Some(detail.into()),
            degraded: true,
        }
    }

    /// A caller-injected slot with no durability declaration. Its domain's
    /// class does not assert that the selected backend persists those bytes.
    pub fn unverified(domain: &str, class: DurabilityClass, backend: impl Into<String>) -> Self {
        Self {
            durability: StorageSlotDurability::Unverified {
                domain: domain.to_string(),
                class,
            },
            backend: backend.into(),
            detail: None,
            degraded: false,
        }
    }

    /// Attach (or replace) the free-form detail note.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A Scratch-class slot: ephemeral by design (the in-process ring
    /// buffers), classified explicitly so the non-durability is a documented
    /// decision rather than an accident.
    pub fn scratch(domain: &str, backend: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            durability: StorageSlotDurability::Declared(DurabilityDeclaration {
                domain: domain.to_string(),
                class: DurabilityClass::Scratch,
                resolution: DurabilityResolution::DeclaredEphemeral,
            }),
            backend: backend.into(),
            detail: Some(detail.into()),
            degraded: false,
        }
    }

    fn status_json(&self) -> serde_json::Value {
        let mut object = serde_json::json!({
            "domain": self.durability.domain(),
            "class": serde_json::to_value(self.durability.class())
                .unwrap_or(serde_json::Value::Null),
            "resolution": self.durability.resolution_json(),
            "backend": self.backend,
            "degraded": self.degraded,
        });
        if let (Some(detail), Some(map)) = (self.detail.as_ref(), object.as_object_mut()) {
            map.insert(
                "detail".to_string(),
                serde_json::Value::String(detail.clone()),
            );
        }
        object
    }
}

/// Project the H1 [`BlobDurability`] resolution onto its slot-census entry.
pub fn blob_slot_summary(durability: BlobDurability) -> StorageSlotSummary {
    match durability {
        BlobDurability::PersistentDisk => {
            StorageSlotSummary::persistent("blobs", "ObjectStoreBlobStore (local disk)")
        }
        BlobDurability::DeclaredEphemeral => StorageSlotSummary::declared_ephemeral(
            "blobs",
            "ObjectStoreBlobStore (memory)",
            "explicitly declared (ephemeral launch mode or ephemeral_blobs(true))",
        ),
        BlobDurability::Custom { persistent: true } => {
            StorageSlotSummary::persistent("blobs", "custom blob store")
                .with_detail("caller-injected store reporting is_persistent()")
        }
        BlobDurability::Custom { persistent: false } => StorageSlotSummary::declared_ephemeral(
            "blobs",
            "custom blob store",
            "caller-injected store reports !is_persistent()",
        ),
    }
}

/// The three in-process ring buffers (`MetaRuntimeHandle` state), classified
/// `Scratch` explicitly: bounded drop-oldest retention, no store seam. A
/// durable gating-audit slot is a flagged candidate follow-up of the storage
/// plan, deliberately not part of this arc.
pub fn scratch_ring_buffer_slots() -> Vec<StorageSlotSummary> {
    vec![
        StorageSlotSummary::scratch(
            "gating_audit",
            "in-process ring buffer",
            "drop-oldest retention (512 entries); durable audit slot is a flagged follow-up",
        ),
        StorageSlotSummary::scratch(
            "delivery_history",
            "in-process ring buffer",
            "drop-oldest retention (200 entries)",
        ),
        StorageSlotSummary::scratch(
            "routing_resolutions",
            "in-process ring buffer",
            "drop-oldest retention (512 entries)",
        ),
    ]
}

/// Where the runtime store that holds this runtime's durable sessions
/// physically lives, recorded by the composition that opened it. This is
/// the typed fact behind the operator's `rkat session repair-wholeblob`
/// scope in a `SessionRepairRequired` hold: the exact database the command
/// must open, never a guess from the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeStoreLocator {
    /// Meta opened one SQLite runtime database directly (the default
    /// persistent layout, `<state_dir>/runtime.sqlite`). The operator
    /// addresses it with `rkat session repair-wholeblob <id> --runtime-store
    /// <path>`.
    SqliteFile { path: PathBuf },
    /// The runtime store rides a meerkat realm bundle (a composite storage
    /// provider's meerkat-level realm under the state directory). The
    /// operator addresses it with `rkat --state-root <state_root> --realm
    /// <realm> session repair-wholeblob <id>`.
    MeerkatRealm { state_root: PathBuf, realm: String },
}

impl crate::identity_first::SessionRepairScope {
    /// The `rkat` scope that opens exactly the runtime store `locator` names.
    pub fn from_runtime_store_locator(locator: &RuntimeStoreLocator) -> Self {
        match locator {
            RuntimeStoreLocator::SqliteFile { path } => Self {
                state_root: None,
                realm: None,
                runtime_store: Some(path.clone()),
            },
            RuntimeStoreLocator::MeerkatRealm { state_root, realm } => Self {
                state_root: Some(state_root.clone()),
                realm: Some(realm.clone()),
                runtime_store: None,
            },
        }
    }

    /// The scope for the runtime store a composed runtime actually opened,
    /// `None` when the composition recorded no persistent runtime store (an
    /// in-memory or declared-ephemeral runtime store has no document an
    /// operator could repair).
    pub fn from_resolved_storage(summary: &ResolvedStorageSummary) -> Option<Self> {
        summary
            .runtime_store_locator
            .as_ref()
            .map(Self::from_runtime_store_locator)
    }
}

/// Composition-time storage resolution summary, recorded when the runtime's
/// stores are composed and surfaced through `meta/status` and
/// `meta/capabilities`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStorageSummary {
    /// What the blob slot resolved to (H1).
    pub blob_durability: BlobDurability,
    /// Whether the session store behind the persistent session service
    /// advertises the incremental-persistence capability (H2). `None` when
    /// the runtime persists no sessions (ephemeral session service).
    pub session_store_incremental: Option<bool>,
    /// Per-slot durability census (M4). Additive: surfaces emitting the
    /// summary before the census existed keep their wire shape and gain a
    /// `"slots"` array.
    pub slots: Vec<StorageSlotSummary>,
    /// The state directory this runtime's stores resolved under (`None` =
    /// the composing surface recorded none). Consumed by the storage-doctor
    /// RPC to prove a requested state_dir is this runtime's own before
    /// attaching the live census; never part of `status_json`.
    pub state_dir: Option<PathBuf>,
    /// The persistent runtime store this runtime opened, when it opened one:
    /// the typed source of the repair commands in a `SessionRepairRequired`
    /// hold. Never part of `status_json`.
    pub runtime_store_locator: Option<RuntimeStoreLocator>,
}

impl ResolvedStorageSummary {
    /// The pre-census (H1/H2) summary shape: blob durability + the
    /// incremental probe, with an empty slot census.
    pub fn new(blob_durability: BlobDurability, session_store_incremental: Option<bool>) -> Self {
        Self {
            blob_durability,
            session_store_incremental,
            slots: Vec::new(),
            state_dir: None,
            runtime_store_locator: None,
        }
    }

    /// Record where the persistent runtime store was opened (`None` for an
    /// in-memory or declared-ephemeral runtime store).
    #[must_use]
    pub fn with_runtime_store_locator(mut self, locator: Option<RuntimeStoreLocator>) -> Self {
        self.runtime_store_locator = locator;
        self
    }

    /// Attach the per-slot census.
    #[must_use]
    pub fn with_slots(mut self, slots: Vec<StorageSlotSummary>) -> Self {
        self.slots = slots;
        self
    }

    /// Record the state directory the stores resolved under (the doctor
    /// RPC's own-directory census guard).
    #[must_use]
    pub fn with_state_dir(mut self, state_dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(state_dir.into());
        self
    }

    /// The `"storage"` object shared by every `meta/status` /
    /// `meta/capabilities` handler, so the three status shapes stay
    /// field-consistent.
    pub fn status_json(&self) -> serde_json::Value {
        serde_json::json!({
            "blob_durability": self.blob_durability.as_str(),
            "blob_store_persistent": self.blob_durability.is_persistent(),
            "session_store_incremental": self.session_store_incremental,
            "slots": self
                .slots
                .iter()
                .map(StorageSlotSummary::status_json)
                .collect::<Vec<_>>(),
        })
    }
}

/// Fail-closed blob-slot resolution failure (H1).
#[derive(Debug)]
pub enum BlobStoreResolutionError {
    /// The local blob directory under the persistent state path failed to
    /// open. Formerly a silent in-memory fallback; now a startup error.
    OpenFailed { path: PathBuf, message: String },
    /// Persistent mode resolved a blob store that reports
    /// `!is_persistent()` without the explicit ephemeral-blobs declaration.
    NonPersistentUndeclared,
}

impl std::fmt::Display for BlobStoreResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFailed { path, message } => write!(
                f,
                "failed to open persistent binary blob store at {}: {message} \
                 (fix the blob directory, or declare in-memory blobs explicitly \
                 via ephemeral_blobs(true))",
                path.display()
            ),
            Self::NonPersistentUndeclared => write!(
                f,
                "persistent mode resolved a blob store that reports \
                 !is_persistent(); blobs would silently vanish on restart. \
                 Provide a persistent blob store, or declare the ephemeral \
                 choice explicitly via ephemeral_blobs(true)"
            ),
        }
    }
}

impl std::error::Error for BlobStoreResolutionError {}

/// Fail-closed runtime-store resolution failure (M4, the "fifth fallback").
///
/// Formerly a `tracing::warn!` plus a silent `InMemoryRuntimeStore` twin, a
/// degraded mode in which resume across restart and archive operations fail
/// long after boot. Now a startup error. A persistent composition never
/// substitutes an in-memory runtime store; one is constructible only without
/// persistent state, as an explicit declaration
/// (`UnifiedRuntimeBuilder::ephemeral_runtime_store(true)`, or the gateway's
/// `runtime_options.runtime_store = {"storage": "memory"}`).
#[derive(Debug)]
pub struct RuntimeStoreResolutionError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for RuntimeStoreResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to open the persistent runtime store at {}: {} \
             (sessions would not survive restart and archive operations would \
             fail; fix the database file. A composition with persistent state \
             never substitutes an in-memory runtime store)",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for RuntimeStoreResolutionError {}

/// Fail-closed canonical detached-job store open failure.
#[derive(Debug)]
pub struct JobStoreResolutionError {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for JobStoreResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "failed to open the canonical detached-job store at {}: {} \
             (durable detached admission is unavailable; fix the database file)",
            self.path.display(),
            self.message
        )
    }
}

impl std::error::Error for JobStoreResolutionError {}

/// Composite fail-closed storage composition failure for the persistent
/// bootstrap path: any durable slot that can refuse at composition time.
#[derive(Debug)]
pub enum StorageResolutionError {
    Blob(BlobStoreResolutionError),
    RuntimeStore(RuntimeStoreResolutionError),
    JobStore(JobStoreResolutionError),
    /// A durable council store that exists on disk and cannot be read.
    ///
    /// Absent is NOT this - an absent store is the ordinary first-boot case
    /// and degrades quietly. This variant exists because meerkat's council
    /// custody recovery surfaces read/decode failure as a typed refusal, and
    /// swallowing it here would hide the signal that path exists to raise.
    Council(crate::council_wiring::CouncilStoreError),
    /// The installation refused at the layout: predecessor or mid-adoption
    /// state, or an unresolvable meerkat-level realm.
    Layout(crate::storage_layout::StorageLayoutError),
    /// The composition's runtime machine could not be built or acquired over
    /// its store because execution custody or controller readiness was refused.
    Runtime(meerkat_runtime::RuntimeDriverError),
    Delivery(crate::mob_handle_runtime::MobRuntimeDeliveryError),
}

impl std::fmt::Display for StorageResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blob(error) => error.fmt(f),
            Self::RuntimeStore(error) => error.fmt(f),
            Self::JobStore(error) => error.fmt(f),
            Self::Council(error) => error.fmt(f),
            Self::Layout(error) => error.fmt(f),
            Self::Runtime(error) => error.fmt(f),
            Self::Delivery(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StorageResolutionError {}

impl From<crate::council_wiring::CouncilStoreError> for StorageResolutionError {
    fn from(error: crate::council_wiring::CouncilStoreError) -> Self {
        Self::Council(error)
    }
}

impl From<BlobStoreResolutionError> for StorageResolutionError {
    fn from(error: BlobStoreResolutionError) -> Self {
        Self::Blob(error)
    }
}

impl From<RuntimeStoreResolutionError> for StorageResolutionError {
    fn from(error: RuntimeStoreResolutionError) -> Self {
        Self::RuntimeStore(error)
    }
}

impl From<JobStoreResolutionError> for StorageResolutionError {
    fn from(error: JobStoreResolutionError) -> Self {
        Self::JobStore(error)
    }
}

/// Probe a session store's incremental-persistence capability before handing
/// it to `PersistentSessionService`.
///
/// The service runs the same probe privately and silently degrades to
/// whole-blob persistence (O(session) written per turn) when the capability
/// is absent; this duplicate probe on the same `Arc` makes that degradation
/// loud at startup and feeds the `session_store_incremental` health flag.
/// `store_kind` names the concrete store in the warning (the caller knows
/// which store it composed; the trait object does not).
pub fn probe_session_store_incremental(store: &Arc<dyn SessionStore>, store_kind: &str) -> bool {
    let incremental = Arc::clone(store).as_incremental().is_some();
    if !incremental {
        tracing::warn!(
            session_store = store_kind,
            "session store does not advertise incremental persistence; \
             session persistence degrades to whole-blob saves on every turn \
             (incremental capability absent). Fix: implement \
             ContinuityStore::as_incremental_sessions on the continuity store \
             (Meta's ContinuityIncrementalSessions channel), or forward it \
             from the store your wrapper wraps"
        );
    }
    incremental
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Captures formatted tracing output so the tests can assert on the
    /// startup warning text.
    #[derive(Clone, Default)]
    struct CaptureWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl CaptureWriter {
        fn contents(&self) -> String {
            String::from_utf8_lossy(
                &self
                    .0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            )
            .into_owned()
        }
    }

    impl std::io::Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
        type Writer = CaptureWriter;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn probe_with_captured_warnings(
        store: &Arc<dyn SessionStore>,
        store_kind: &str,
    ) -> (bool, String) {
        let writer = CaptureWriter::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer.clone())
            .with_max_level(tracing::Level::WARN)
            .finish();
        // A second registered Dispatch keeps callsite interest from being
        // cached `never` by threads without a subscriber.
        let _anchor = tracing::Dispatch::new(tracing_subscriber::Registry::default());
        let incremental = tracing::subscriber::with_default(subscriber, || {
            probe_session_store_incremental(store, store_kind)
        });
        (incremental, writer.contents())
    }

    /// H2 (flipped by M4b): the continuity adapter over the bundled local
    /// store now advertises the session-delta channel, so the probe reports
    /// incremental persistence with NO whole-blob degradation warning.
    #[tokio::test]
    async fn probe_reports_continuity_adapter_as_incremental_without_warning() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (store, _fencing_floor) =
            crate::identity_first::LocalContinuityStore::open_with_fencing_floor(
                dir.path().join("continuity.sqlite"),
            )
            .await
            .expect("open continuity store");
        let adapter: Arc<dyn SessionStore> = Arc::new(
            crate::identity_first::ContinuitySessionStoreAdapter::new(Arc::new(store)),
        );

        let (incremental, warnings) =
            probe_with_captured_warnings(&adapter, "ContinuitySessionStoreAdapter");
        assert!(
            incremental,
            "the continuity adapter over the bundled store advertises incremental persistence"
        );
        assert!(
            warnings.is_empty(),
            "no whole-blob degradation warning expected, got: {warnings}"
        );
    }

    /// The degradation warning is still load-bearing for substrates that
    /// genuinely have no delta channel (the wire-verb `GatewayContinuityStore`
    /// shape): the probe must still say so loudly, naming the store kind and
    /// the whole-blob consequence.
    #[test]
    fn probe_still_warns_for_a_whole_blob_only_store() {
        struct WholeBlobOnlyStore;

        #[async_trait::async_trait]
        impl SessionStore for WholeBlobOnlyStore {
            async fn save(
                &self,
                _session: &meerkat_core::Session,
            ) -> Result<(), meerkat_core::SessionStoreError> {
                Ok(())
            }

            async fn load(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<Option<meerkat_core::Session>, meerkat_core::SessionStoreError>
            {
                Ok(None)
            }

            async fn list(
                &self,
                _filter: meerkat_core::SessionFilter,
            ) -> Result<Vec<meerkat_core::SessionMeta>, meerkat_core::SessionStoreError>
            {
                Ok(Vec::new())
            }

            async fn delete(
                &self,
                _id: &meerkat_core::types::SessionId,
            ) -> Result<(), meerkat_core::SessionStoreError> {
                Ok(())
            }

            async fn delete_if_current_revision(
                &self,
                _id: &meerkat_core::types::SessionId,
                _expected_current_revision: &str,
            ) -> Result<bool, meerkat_core::SessionStoreError> {
                Ok(false)
            }
        }

        let store: Arc<dyn SessionStore> = Arc::new(WholeBlobOnlyStore);
        let (incremental, warnings) = probe_with_captured_warnings(&store, "WholeBlobOnlyStore");
        assert!(!incremental);
        assert!(
            warnings.contains("whole-blob"),
            "the startup warning must name the consequence, got: {warnings}"
        );
        assert!(
            warnings.contains("WholeBlobOnlyStore"),
            "the startup warning must name the store kind, got: {warnings}"
        );
    }

    /// H2: an incremental-capable store probes true with no warning.
    #[test]
    fn probe_reports_incremental_sqlite_store_without_warning() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store: Arc<dyn SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(dir.path().join("sessions.db"))
                .expect("open sqlite session store"),
        );

        let (incremental, warnings) = probe_with_captured_warnings(&store, "SqliteSessionStore");
        assert!(incremental, "SqliteSessionStore advertises as_incremental");
        assert!(
            warnings.is_empty(),
            "no degradation warning expected, got: {warnings}"
        );
    }

    #[test]
    fn blob_durability_wire_spellings_are_stable() {
        assert_eq!(BlobDurability::PersistentDisk.as_str(), "persistent_disk");
        assert_eq!(
            BlobDurability::DeclaredEphemeral.as_str(),
            "declared_ephemeral"
        );
        assert_eq!(
            BlobDurability::Custom { persistent: true }.as_str(),
            "custom"
        );
        assert!(BlobDurability::PersistentDisk.is_persistent());
        assert!(!BlobDurability::DeclaredEphemeral.is_persistent());
        assert!(BlobDurability::Custom { persistent: true }.is_persistent());
        assert!(!BlobDurability::Custom { persistent: false }.is_persistent());
    }

    #[test]
    fn status_json_carries_all_health_fields() {
        let summary = ResolvedStorageSummary::new(BlobDurability::DeclaredEphemeral, None);
        let json = summary.status_json();
        assert_eq!(json["blob_durability"], "declared_ephemeral");
        assert_eq!(json["blob_store_persistent"], false);
        assert!(json["session_store_incremental"].is_null());
        assert_eq!(json["slots"], serde_json::json!([]));

        let summary = ResolvedStorageSummary::new(BlobDurability::PersistentDisk, Some(true));
        let json = summary.status_json();
        assert_eq!(json["blob_durability"], "persistent_disk");
        assert_eq!(json["blob_store_persistent"], true);
        assert_eq!(json["session_store_incremental"], true);
    }

    /// M4: the per-slot census rides the same `"storage"` object additively —
    /// pre-census fields keep their spellings; each slot entry carries the
    /// meerkat durability vocabulary plus backend and degradation facts.
    #[test]
    fn status_json_slot_census_is_additive_and_machine_readable() {
        let summary = ResolvedStorageSummary::new(BlobDurability::PersistentDisk, Some(true))
            .with_slots(vec![
                StorageSlotSummary::persistent("runtime", "SqliteRuntimeStore"),
                StorageSlotSummary::declared_ephemeral(
                    "metadata",
                    "InMemoryMetadataStore (declared default)",
                    "this surface keeps metadata in-memory by contract",
                ),
                StorageSlotSummary::degraded("schedule", "schedule store failed to open: disk"),
                StorageSlotSummary::scratch("gating_audit", "in-process ring buffer", "512"),
            ]);
        let json = summary.status_json();
        let slots = json["slots"].as_array().expect("slots array");
        assert_eq!(slots.len(), 4);
        assert_eq!(slots[0]["domain"], "runtime");
        assert_eq!(slots[0]["class"], "durable");
        assert_eq!(slots[0]["resolution"], "persistent");
        assert_eq!(slots[0]["backend"], "SqliteRuntimeStore");
        assert_eq!(slots[0]["degraded"], false);
        assert!(slots[0].get("detail").is_none());
        assert_eq!(slots[1]["resolution"], "declared_ephemeral");
        assert_eq!(slots[2]["resolution"], "non_persistent");
        assert_eq!(slots[2]["degraded"], true);
        assert_eq!(slots[2]["backend"], "disabled");
        assert_eq!(slots[3]["class"], "scratch");
    }

    #[test]
    fn status_json_unverified_slot_does_not_invent_a_declaration() {
        let slot = StorageSlotSummary::unverified(
            "workgraph",
            DurabilityClass::Durable,
            "custom workgraph store",
        );
        assert!(slot.durability.declaration().is_none());
        assert_eq!(slot.durability.domain(), "workgraph");
        assert_eq!(slot.durability.class(), DurabilityClass::Durable);
        assert_eq!(
            slot.status_json(),
            serde_json::json!({
                "domain": "workgraph",
                "class": "durable",
                "resolution": "unverified",
                "backend": "custom workgraph store",
                "degraded": false,
            })
        );
    }

    #[test]
    fn status_json_declared_slot_preserves_provider_resolution_verbatim() {
        for resolution in [
            DurabilityResolution::Persistent,
            DurabilityResolution::DeclaredEphemeral,
            DurabilityResolution::NonPersistent,
        ] {
            let declaration = DurabilityDeclaration::durable("workgraph", resolution);
            let slot = StorageSlotSummary {
                durability: StorageSlotDurability::Declared(declaration.clone()),
                backend: "storage provider 'test'".to_string(),
                detail: None,
                degraded: false,
            };
            assert_eq!(slot.durability.declaration(), Some(&declaration));
            assert_eq!(
                slot.status_json(),
                serde_json::json!({
                    "domain": "workgraph",
                    "class": "durable",
                    "resolution": resolution,
                    "backend": "storage provider 'test'",
                    "degraded": false,
                })
            );
        }
    }

    #[test]
    fn runtime_store_resolution_error_names_the_remediation() {
        let error = RuntimeStoreResolutionError {
            path: PathBuf::from("/state/runtime.sqlite"),
            message: "disk I/O error".to_string(),
        };
        let text = error.to_string();
        assert!(text.contains("/state/runtime.sqlite"));
        assert!(text.contains("fix the database file"));
        // Persistent state refuses an in-memory runtime store, so the text
        // must not recommend declaring one.
        assert!(!text.contains("ephemeral_runtime_store(true)"));
        assert!(!text.contains("runtime_options.runtime_store"));
    }
}
