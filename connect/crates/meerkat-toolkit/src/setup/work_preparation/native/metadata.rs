//! Codec inside the existing preparation owner. No target writes, lifecycle,
//! launch callback, current authorization decision or effect retry live here.
use super::*;
use meerkat_core::service::WorkGraphNamespaceGrant;
use meerkat_core::storage_durability::{
    DurabilityClass, DurabilityDeclaration, DurabilityResolution,
};
use meerkat_workgraph::{
    ExternalWorkRef, MemoryWorkGraphStore, WorkGraphEvent, WorkGraphEventFilter,
    WorkGraphEventKind, WorkGraphMachine, WorkGraphService, WorkGraphStore, WorkItem, WorkItemId,
    WorkStatus,
};
use std::sync::Arc;

const CORRELATION_KIND: &str = "meerkat_toolkit.setup_work_correlation.v3";
const METADATA_TITLE: &str = "Toolkit setup preparation";

/// Exact trusted host configuration, not proof of a physical store incarnation.
/// The host retains the generation across restarts and changes it on replacement
/// unless continuity is established. Reusing it for another store violates the
/// composition contract; paths, `kind()` and Arc identity cannot attest this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupWorkBackendPin {
    provider: String,
    realm: Id,
    namespace: WorkNamespace,
    backend_generation: Id,
    // The domain is the store slot; keep the exact declaration with the pin.
    durability: DurabilityDeclaration,
}

impl SetupWorkBackendPin {
    pub fn provider(&self) -> &str {
        &self.provider
    }
    pub fn realm(&self) -> &Id {
        &self.realm
    }
    pub fn namespace(&self) -> &WorkNamespace {
        &self.namespace
    }
    pub fn backend_generation(&self) -> &Id {
        &self.backend_generation
    }
    pub fn durability(&self) -> &DurabilityDeclaration {
        &self.durability
    }

    fn grant(&self) -> Result<WorkGraphNamespaceGrant, SetupError> {
        if self.provider.is_empty()
            || self.provider.trim() != self.provider
            || self.provider.chars().any(char::is_control)
            || self.durability.domain != "workgraph"
            || self.durability.class != DurabilityClass::Durable
            || self.durability.resolution != DurabilityResolution::Persistent
        {
            return Err(SetupError::InvalidRecord);
        }
        WorkGraphNamespaceGrant::new(self.realm.as_str(), self.namespace.as_str())
            .map_err(|_| SetupError::InvalidRecord)
    }

    fn check_scope(&self, scope: &HostWorkGraphScope) -> Result<(), SetupError> {
        self.grant()?;
        if &self.realm != scope.realm() || &self.namespace != scope.namespace() {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }
}

/// Selected native backend for the preparation's eventual metadata destination.
/// This cannot be deserialized from tool input. The host must pass the actual
/// selected service and declarations captured together from its provider open,
/// not fabricate a declaration or open a second setup store. This constructor
/// checks the declaration and exact service scope; it cannot authenticate that
/// host assertion or certify durable storage from an implementation name.
#[derive(Clone)]
pub struct SetupWorkBackend {
    service: WorkGraphService,
    pin: SetupWorkBackendPin,
}

impl SetupWorkBackend {
    pub fn new(
        service: WorkGraphService,
        provider: String,
        backend_generation: Id,
        declarations: &[DurabilityDeclaration],
    ) -> Result<Self, SetupError> {
        let mut matches = declarations
            .iter()
            .filter(|item| item.domain == "workgraph");
        let durability = matches.next().ok_or(SetupError::InvalidRecord)?.clone();
        if matches.next().is_some() {
            return Err(SetupError::InvalidRecord);
        }
        let grant = service.namespace_grant();
        let pin = SetupWorkBackendPin {
            provider,
            realm: Id::new(grant.realm_id.clone()).map_err(|_| SetupError::InvalidRecord)?,
            namespace: WorkNamespace::new(grant.namespace.clone())?,
            backend_generation,
            durability,
        };
        pin.grant()?;
        Ok(Self { service, pin })
    }

    pub fn pin(&self) -> &SetupWorkBackendPin {
        &self.pin
    }

    /// Use the exact service retained with the provider declaration. Trusted
    /// host composition must still supply current native authority for each
    /// operation; this accessor adds no grant or second service selection.
    pub fn service(&self) -> &WorkGraphService {
        &self.service
    }

    // No target store I/O occurs while preparing or recovering. Keeping this
    // exact service binds the selection to the host-supplied store in process.
    pub(super) fn check_scope(&self, scope: &HostWorkGraphScope) -> Result<(), SetupError> {
        self.pin.check_scope(scope)?;
        let grant = self.service.namespace_grant();
        if grant.realm_id != self.pin.realm.as_str()
            || grant.namespace != self.pin.namespace.as_str()
        {
            return Err(SetupError::WrongScope);
        }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreparedMetadata {
    backend: SetupWorkBackendPin,
    item: WorkItem,
    event: WorkGraphEvent,
}

impl PreparedMetadata {
    /// Native canonical request-digest validation only. This private arena
    /// performs no store read/write and supplies no authority or work receipt.
    pub(super) fn validate_admission(
        &self,
        request: &CreateWorkItemRequest,
        admission: &meerkat_workgraph::WorkAdmission,
    ) -> Result<(), SetupError> {
        let arena = Arc::new(MemoryWorkGraphStore::new());
        let service = WorkGraphService::with_namespace_grant(arena, self.backend.grant()?)?;
        if service.admission_identity(admission.key.clone(), request.clone())? != *admission {
            return Err(SetupError::InvalidRecord);
        }
        Ok(())
    }

    pub(super) fn backend(&self) -> &SetupWorkBackendPin {
        &self.backend
    }
    pub(super) fn item(&self) -> &WorkItem {
        &self.item
    }
    pub(super) fn event(&self) -> &WorkGraphEvent {
        &self.event
    }
    pub(super) fn check_scope(&self, scope: &HostWorkGraphScope) -> Result<(), SetupError> {
        self.backend.check_scope(scope)
    }
    pub(super) fn check_backend(&self, selected: &SetupWorkBackend) -> Result<(), SetupError> {
        let scope =
            HostWorkGraphScope::new(self.backend.realm.clone(), self.backend.namespace.clone())?;
        selected.check_scope(&scope)?;
        if self.backend != selected.pin {
            return Err(StoreError::AdmissionConflict.into());
        }
        Ok(())
    }

    pub(super) async fn prepare(
        selected: &SetupWorkBackend,
        snapshot: &SetupSnapshot,
        effect: &StoredEffect,
        original: &PreparedRecord,
    ) -> Result<Self, SetupError> {
        let scope = HostWorkGraphScope::new(original.realm.clone(), original.namespace.clone())?;
        selected.check_scope(&scope)?;
        let request = metadata_request(snapshot, effect, original, &selected.pin)?;
        let (item, event) = native_candidate(selected.pin.grant()?, request).await?;
        Ok(Self {
            backend: selected.pin.clone(),
            item,
            event,
        })
    }

    pub(super) async fn validate(
        &self,
        snapshot: &SetupSnapshot,
        effect: &StoredEffect,
        original: &PreparedRecord,
    ) -> Result<(), SetupError> {
        let request = metadata_request(snapshot, effect, original, &self.backend)?;
        let realm = original.realm.to_string();
        let namespace = original.namespace.clone();
        if self.item.realm_id != realm
            || self.item.namespace != namespace
            || self.event.realm_id != realm
            || self.event.namespace != namespace
        {
            return Err(SetupError::InvalidRecord);
        }
        WorkGraphMachine::validate_item_projection(&self.item)?;
        // The actual service owns private creation policy. Re-enter it in a
        // private arena; never ordinary-create in the target or replace the
        // persisted winner with this discarded validation candidate.
        native_candidate(self.backend.grant()?, request.clone()).await?;
        let (mut expected_item, mut expected_event) =
            WorkGraphMachine::create_item(request, realm, namespace, self.item.created_at)?;
        // Persisted native randomness/time are the only variable choices. All
        // other fields, including machine state and Created payload, are native
        // derivations from the exact fixed request. A self-hash is insufficient.
        expected_item.id = WorkItemId::new(self.item.id.as_str())?;
        expected_event.item_id = Some(expected_item.id.clone());
        *expected_event
            .payload
            .get_mut("item")
            .ok_or(SetupError::InvalidRecord)? = serde_json::to_value(&expected_item)?;
        if self.item != expected_item || self.event != expected_event {
            return Err(SetupError::InvalidRecord);
        }
        Ok(())
    }
}

fn metadata_request(
    snapshot: &SetupSnapshot,
    effect: &StoredEffect,
    original: &PreparedRecord,
    backend: &SetupWorkBackendPin,
) -> Result<CreateWorkItemRequest, SetupError> {
    let scope = HostWorkGraphScope::new(original.realm.clone(), original.namespace.clone())?;
    backend.check_scope(&scope)?;
    // The original request/reference remain separately persisted and subject to
    // their existing access checks. No title, user text, reference locator,
    // audience, policy, child authority or continuation is copied to WorkGraph.
    // Exact correlation is host-issued and compared, never fuzzy-matched.
    let correlation = ContentDigest::of_bytes(&canonical_json(&(
        CORRELATION_KIND,
        backend,
        original,
        operation_id(snapshot.request())?,
        &effect.key,
        &effect.intent.idempotency_key,
        snapshot.request.input_digest(),
        &snapshot.request.host().continuation,
    ))?);
    Ok(CreateWorkItemRequest {
        realm_id: Some(original.realm.to_string()),
        namespace: Some(original.namespace.clone()),
        title: METADATA_TITLE.into(),
        status: Some(WorkStatus::Blocked),
        external_refs: vec![ExternalWorkRef {
            kind: CORRELATION_KIND.into(),
            id: correlation.to_string(),
            url: None,
        }],
        ..Default::default()
    })
}

async fn native_candidate(
    grant: WorkGraphNamespaceGrant,
    request: CreateWorkItemRequest,
) -> Result<(WorkItem, WorkGraphEvent), SetupError> {
    // Short-lived native preparation arena, not another durable WorkGraph or
    // lifecycle engine. Only the exact resulting pair leaves this function.
    let arena = Arc::new(MemoryWorkGraphStore::new());
    let creator = WorkGraphService::with_namespace_grant(arena.clone(), grant)?;
    let item = creator.create(request).await?;
    let mut events = arena
        .list_events(WorkGraphEventFilter {
            realm_id: Some(item.realm_id.clone()),
            namespace: Some(item.namespace.clone()),
            limit: Some(2),
            ..Default::default()
        })
        .await?;
    if events.len() != 1 {
        return Err(SetupError::InvalidRecord);
    }
    let mut event = events.remove(0);
    // A Blocked creation must not derive any launch/readiness fact. Check before
    // discarding arena-local sequence; target insertion would own its sequence.
    if event.kind != WorkGraphEventKind::Created
        || !event.facts.is_empty()
        || event.item_id.as_ref() != Some(&item.id)
        || event.payload.get("item") != Some(&serde_json::to_value(&item)?)
    {
        return Err(SetupError::InvalidRecord);
    }
    event.seq = None;
    Ok((item, event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_retains_the_exact_host_supplied_sqlite_store_arc() {
        let directory = tempfile::tempdir().unwrap();
        let actual: Arc<dyn WorkGraphStore> = Arc::new(
            meerkat_workgraph::SqliteWorkGraphStore::open(directory.path().join("selected.sqlite"))
                .unwrap(),
        );
        let service = WorkGraphService::with_namespace_grant(
            actual.clone(),
            WorkGraphNamespaceGrant::new("work-realm", "setup").unwrap(),
        )
        .unwrap();
        let selection = SetupWorkBackend::new(
            service,
            "selected-provider".into(),
            Id::new("stable-generation").unwrap(),
            &[DurabilityDeclaration::durable(
                "workgraph",
                DurabilityResolution::Persistent,
            )],
        )
        .unwrap();
        assert!(Arc::ptr_eq(selection.service.store(), &actual));
        assert_eq!(selection.pin.provider(), "selected-provider");
        // Same-process object identity is evidence of this constructor's wiring,
        // not a restart identity or proof of the provider's declaration.
    }
}
