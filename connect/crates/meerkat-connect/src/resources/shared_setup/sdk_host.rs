//! Explicit live SDK host for the two-child native control. This launch owner
//! creates the WorkGraph records and sends the corresponding real Original
//! through the public private-host protocol. It is not the stock installed
//! application's invocation producer, a persisted route map, or a scheduler.

use async_trait::async_trait;
use meerkat_toolkit::{
    continuation::{
        ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation,
        ContinuationReconciliation, SetupContinuationDelivery, SetupContinuationOwner,
    },
    host_lifetime::{NativeHostLifetime, control::HostLifetimeError},
    request::{ContextReference, TaskContinuation},
    setup::{SetupWorkBackend, SetupWorkContext},
};
use meerkat_workgraph::{
    ClaimWorkItemRequest, CreateWorkItemRequest, LinkWorkItemsRequest, WorkEdgeKind, WorkItemRef,
    WorkOwner, WorkOwnerKey,
};
use serde_json::{Value, json};
use std::{
    os::fd::IntoRawFd,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::watch,
};
use toolkit_mobkit::{
    ToolAdmissionError,
    installed_host::acquisition::{
        InstalledNativeContinuation, InstalledOriginalInvocation, InstalledOriginalTaskBinding,
        InstalledOriginalTaskOwner,
    },
    private_host::{InheritedHostPipes, PRIVATE_HOST_PROTOCOL_VERSION},
    private_originals::{
        InstalledWorkflowContextTransfer, InstalledWorkflowPlanning, PrivateAuthenticatedEvent,
        PrivateOriginalRef, PrivateOriginalSubmission, PrivateOriginalSubmit, PrivateOriginals,
        PrivateSourceSettlement,
    },
};
use toolkit_package::{Id, canonical_json};

const BOUND: Duration = Duration::from_secs(20);

/// The test's normal SDK parent owns these exact local endpoints. Construct
/// them before the test runtime, matching InheritedHostPipes' entry contract.
pub(crate) struct SdkParent {
    commands: std::os::unix::net::UnixStream,
    responses: std::os::unix::net::UnixStream,
}
impl SdkParent {
    pub(crate) fn pair() -> (InheritedHostPipes, Self) {
        let (commands, host_commands) = std::os::unix::net::UnixStream::pair().unwrap();
        let (host_responses, responses) = std::os::unix::net::UnixStream::pair().unwrap();
        let inherited =
            InheritedHostPipes::adopt(host_commands.into_raw_fd(), host_responses.into_raw_fd())
                .unwrap();
        (
            inherited,
            Self {
                commands,
                responses,
            },
        )
    }
    pub(crate) fn enter(self) -> Arc<tokio::sync::Mutex<SdkClient>> {
        self.commands.set_nonblocking(true).unwrap();
        self.responses.set_nonblocking(true).unwrap();
        Arc::new(tokio::sync::Mutex::new(SdkClient {
            commands: tokio::net::UnixStream::from_std(self.commands).unwrap(),
            responses: BufReader::new(tokio::net::UnixStream::from_std(self.responses).unwrap()),
            next: 0,
            poisoned: false,
        }))
    }
}

/// Only the existing framed commands are used. This is the bounded test
/// parent's transport, not a product RPC server or an alternate admission API.
pub(crate) struct SdkClient {
    commands: tokio::net::UnixStream,
    responses: BufReader<tokio::net::UnixStream>,
    next: u64,
    poisoned: bool,
}
impl SdkClient {
    pub(crate) async fn call(&mut self, command: Value) -> Result<Value, String> {
        if self.poisoned {
            return Err("private transport remains unresolved; only close is allowed".into());
        }
        self.next += 1;
        let id = self.next;
        let mut request = canonical_json(&json!({
            "version": PRIVATE_HOST_PROTOCOL_VERSION, "id": id, "command": command,
        }))
        .map_err(|error| error.to_string())?;
        request.push(b'\n');
        if request.len() > 1_048_576 {
            return Err("bounded private request exceeded".into());
        }
        // An incomplete exchange may already have entered native admission.
        // Keep that uncertainty sticky. Even a dropped call future cannot let
        // a later child consume the remainder as its own response.
        self.poisoned = true;
        let result: Result<Value, String> = tokio::time::timeout(BOUND, async {
            self.commands
                .write_all(&request)
                .await
                .map_err(|error| error.to_string())?;
            self.commands
                .flush()
                .await
                .map_err(|error| error.to_string())?;
            // A bounded read keeps a missing delimiter from allocating an
            // unbounded response. Failed framing is never retried.
            let mut line = Vec::new();
            loop {
                let available = self
                    .responses
                    .fill_buf()
                    .await
                    .map_err(|error| error.to_string())?;
                if available.is_empty() {
                    return Err("private owner closed before reply".into());
                }
                let end = available.iter().position(|byte| *byte == b'\n');
                let count = end.map_or(available.len(), |index| index + 1);
                if line.len() + count > 1_048_576 {
                    return Err("bounded private reply exceeded".into());
                }
                line.extend_from_slice(&available[..count]);
                self.responses.consume(count);
                if end.is_some() {
                    break;
                }
            }
            let reply: Value = serde_json::from_slice(&line).map_err(|error| error.to_string())?;
            if reply.as_object().is_none_or(|value| value.len() != 3)
                || reply["version"] != PRIVATE_HOST_PROTOCOL_VERSION
                || reply["id"] != id
                || !reply["result"].is_object()
            {
                return Err("private reply did not bind the actual command".into());
            }
            Ok(reply["result"].clone())
        })
        .await
        .map_err(|_| "private command outcome is unresolved".to_string())?;
        if result.is_ok() {
            self.poisoned = false;
        }
        let value = result?;
        if value["kind"] == "error" {
            return Err(format!("private owner refused: {value}"));
        }
        Ok(value)
    }
    pub(crate) async fn close(&mut self) -> Result<(), String> {
        self.commands
            .shutdown()
            .await
            .map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct Admission {
    session: String,
    input: String,
    ticket: String,
}
struct ChildLaunch {
    original: PrivateOriginalRef,
    member: meerkat_core::MobMemberBinding,
    work: SetupWorkContext,
    admitted: watch::Sender<Option<Admission>>,
}

/// A bounded two-child launcher used at the existing InstalledSetupBootstrap
/// continuation field. Each entry is the launch operation itself: it owns the
/// actual source request, WorkGraph child and native admission receipt together.
/// No caller can attach a WorkGraph child to an unrelated observed RunId.
pub(super) struct SdkLaunchOwner {
    native: Arc<InstalledNativeContinuation>,
    host: Arc<NativeHostLifetime>,
    backend_generation: Id,
    backend: OnceLock<SetupWorkBackend>,
    parent: OnceLock<WorkItemRef>,
    parent_owner: WorkOwnerKey,
    cycle: String,
    client: Arc<tokio::sync::Mutex<SdkClient>>,
    children: Mutex<Vec<Arc<ChildLaunch>>>,
    launches: Mutex<Vec<tokio::task::JoinHandle<Result<PrivateOriginalRef, String>>>>,
    planning: Option<OnceLock<Arc<InstalledWorkflowPlanning>>>,
    transfer_source: OnceLock<Arc<PrivateOriginals>>,
    transfers: Mutex<
        Vec<(
            PrivateOriginalRef,
            Arc<InstalledWorkflowContextTransfer>,
            meerkat_workgraph::WorkGraphService,
        )>,
    >,
}
impl SdkLaunchOwner {
    pub(super) fn new(
        native: Arc<InstalledNativeContinuation>,
        host: Arc<NativeHostLifetime>,
        backend_generation: Id,
        client: Arc<tokio::sync::Mutex<SdkClient>>,
        parent_owner: WorkOwnerKey,
        cycle: String,
    ) -> Arc<Self> {
        Self::new_selected(
            native,
            host,
            backend_generation,
            client,
            parent_owner,
            cycle,
            false,
        )
    }

    pub(super) fn new_shared(
        native: Arc<InstalledNativeContinuation>,
        host: Arc<NativeHostLifetime>,
        backend_generation: Id,
        client: Arc<tokio::sync::Mutex<SdkClient>>,
        parent_owner: WorkOwnerKey,
        cycle: String,
    ) -> Arc<Self> {
        Self::new_selected(
            native,
            host,
            backend_generation,
            client,
            parent_owner,
            cycle,
            true,
        )
    }

    fn new_selected(
        native: Arc<InstalledNativeContinuation>,
        host: Arc<NativeHostLifetime>,
        backend_generation: Id,
        client: Arc<tokio::sync::Mutex<SdkClient>>,
        parent_owner: WorkOwnerKey,
        cycle: String,
        shared: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            native,
            host,
            backend_generation,
            backend: OnceLock::new(),
            parent: OnceLock::new(),
            parent_owner,
            cycle,
            client,
            children: Mutex::new(vec![]),
            launches: Mutex::new(vec![]),
            planning: shared.then(OnceLock::new),
            transfer_source: OnceLock::new(),
            transfers: Mutex::new(Vec::new()),
        })
    }

    /// Explicit test-host opt-in, independent of selecting a planning member.
    /// The actual graph backend is already captured from public preparation.
    pub(super) fn enable_context_transfers(&self, source: Arc<PrivateOriginals>) {
        assert!(self.planning.is_some() && self.children.lock().unwrap().is_empty());
        assert!(self.transfer_source.set(source).is_ok());
    }

    pub(super) fn context_transfers(
        &self,
    ) -> Vec<(
        PrivateOriginalRef,
        Arc<InstalledWorkflowContextTransfer>,
        meerkat_workgraph::WorkGraphService,
    )> {
        self.transfers.lock().unwrap().clone()
    }

    /// These are the actual captured backend and claimed parent, not a new
    /// graph opened to reconstruct apparently equal terms.
    pub(super) fn workflow_parent(
        &self,
    ) -> Result<
        (
            meerkat_toolkit::setup::SetupWorkBackendPin,
            WorkItemRef,
            WorkOwnerKey,
            String,
        ),
        ToolAdmissionError,
    > {
        self.host
            .validate_current_selection()
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        Ok((
            self.backend
                .get()
                .ok_or(ToolAdmissionError::Unavailable)?
                .pin()
                .clone(),
            self.parent
                .get()
                .ok_or(ToolAdmissionError::Unavailable)?
                .clone(),
            self.parent_owner.clone(),
            self.cycle.clone(),
        ))
    }

    pub(super) fn select_planning(
        &self,
        plan: Arc<InstalledWorkflowPlanning>,
    ) -> Result<(), ToolAdmissionError> {
        self.host
            .validate_current_selection()
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        let selected = self.planning.as_ref().ok_or(ToolAdmissionError::Denied)?;
        let retained = selected.get_or_init(|| plan.clone());
        if !Arc::ptr_eq(retained, &plan) {
            return Err(ToolAdmissionError::Denied);
        }
        Ok(())
    }

    pub(super) fn selected_planning(
        &self,
    ) -> Result<Option<Arc<InstalledWorkflowPlanning>>, ToolAdmissionError> {
        self.planning
            .as_ref()
            .map(|selected| {
                selected
                    .get()
                    .cloned()
                    .ok_or(ToolAdmissionError::Unavailable)
            })
            .transpose()
    }

    /// The preparation participant may retain this exact provider-open pair
    /// until the final installed lifetime exists. The executable host supplies
    /// the pair captured from that same bootstrap, never a reopened backend.
    pub(super) fn capture_prepared_backend(
        &self,
        service: meerkat_workgraph::WorkGraphService,
        storage: &meerkat_mobkit::storage_health::ResolvedStorageSummary,
    ) -> Result<(), HostLifetimeError> {
        use meerkat_core::storage_durability::{DurabilityClass, DurabilityResolution};
        self.host.validate_current_selection()?;
        let mut slots = storage
            .slots
            .iter()
            .filter(|slot| slot.durability.domain() == "workgraph");
        let slot = slots.next().ok_or(HostLifetimeError::Unavailable)?;
        let declaration = slot
            .durability
            .declaration()
            .ok_or(HostLifetimeError::Unavailable)?;
        let native = self
            .host
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::InvalidSelection)?;
        if slots.next().is_some()
            || slot.degraded
            || declaration.class != DurabilityClass::Durable
            || declaration.resolution != DurabilityResolution::Persistent
            || service.namespace_grant().realm_id != native.realm.to_string()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let backend = SetupWorkBackend::new(
            service,
            slot.backend.clone(),
            self.backend_generation.clone(),
            std::slice::from_ref(declaration),
        )
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
        self.backend
            .set(backend)
            .map_err(|_| HostLifetimeError::InvalidSelection)
    }

    /// The real parent is created only after bootstrap captured the exact
    /// selected service. Child launches cannot supply a different backend.
    pub(super) async fn create_parent(&self) {
        assert!(self.parent.get().is_none());
        let backend = self
            .backend
            .get()
            .expect("actual bootstrap captured WorkGraph");
        let created = backend
            .service()
            .create(CreateWorkItemRequest {
                realm_id: Some(backend.pin().realm().to_string()),
                namespace: Some(backend.pin().namespace().clone()),
                title: "Two independent original requests".into(),
                labels: [self.cycle.clone()].into(),
                ..Default::default()
            })
            .await
            .unwrap();
        let parent = backend
            .service()
            .claim(ClaimWorkItemRequest {
                id: created.id,
                realm_id: Some(created.realm_id.clone()),
                namespace: Some(created.namespace.clone()),
                expected_revision: created.revision,
                owner: WorkOwner {
                    key: self.parent_owner.clone(),
                    display_name: None,
                },
                lease_seconds: None,
                lease_expires_at: None,
            })
            .await
            .unwrap();
        self.parent
            .set(WorkItemRef {
                realm_id: parent.realm_id,
                namespace: parent.namespace,
                item_id: parent.id,
            })
            .expect("one parent launch");
    }

    /// The parent keeps this task even if its observer drops the receiver.
    /// A lost admission reply stays unresolved; it never creates another child
    /// or resends the Original. Final host cleanup must join these exact tasks.
    pub(super) fn launch(
        self: &Arc<Self>,
        child_owner: WorkOwnerKey,
        event: PrivateAuthenticatedEvent,
        content: String,
    ) -> tokio::sync::oneshot::Receiver<Result<PrivateOriginalRef, String>> {
        let mut launches = self.launches.lock().unwrap();
        assert!(
            launches.len() < 2,
            "this fixture owns exactly two child launches"
        );
        let owner = self.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        launches.push(tokio::spawn(async move {
            let result = owner.launch_owned(child_owner, event, content).await;
            let _ = send.send(result.clone());
            result
        }));
        receive
    }

    async fn launch_owned(
        &self,
        child_owner: WorkOwnerKey,
        event: PrivateAuthenticatedEvent,
        content: String,
    ) -> Result<PrivateOriginalRef, String> {
        use sha2::{Digest, Sha256};
        self.host
            .validate_current_selection()
            .map_err(|error| error.to_string())?;
        let backend = self.backend.get().ok_or("bootstrap not captured")?;
        let parent = self.parent.get().ok_or("parent not launched")?;
        // The authenticated host's existing route owns the destination. The
        // launcher cannot choose an arbitrary member alongside its input ID.
        let policy = self.host.selected_policy();
        let native = policy.native_host.as_ref().ok_or("native policy absent")?;
        let route = native
            .routes
            .iter()
            .find(|route| route.id == event.route)
            .ok_or("source route absent")?;
        let (member, _, _) = route
            .source_binding(policy)
            .map_err(|error| error.to_string())?;
        if !native
            .members
            .iter()
            .any(|selected| &selected.member == member)
        {
            return Err("source member absent".into());
        }
        let member = member.clone();
        if event.content_sha256 != format!("{:x}", Sha256::digest(content.as_bytes())) {
            return Err("authenticated content does not match the launch".into());
        }
        let created = backend
            .service()
            .create(CreateWorkItemRequest {
                realm_id: Some(parent.realm_id.clone()),
                namespace: Some(parent.namespace.clone()),
                title: "Perform the independently authenticated request".into(),
                labels: [self.cycle.clone()].into(),
                ..Default::default()
            })
            .await
            .map_err(|error| error.to_string())?;
        let child = backend
            .service()
            .claim(ClaimWorkItemRequest {
                id: created.id,
                realm_id: Some(created.realm_id.clone()),
                namespace: Some(created.namespace.clone()),
                expected_revision: created.revision,
                owner: WorkOwner {
                    key: child_owner.clone(),
                    display_name: None,
                },
                lease_seconds: None,
                lease_expires_at: None,
            })
            .await
            .map_err(|error| error.to_string())?;
        backend
            .service()
            .link(LinkWorkItemsRequest {
                realm_id: Some(parent.realm_id.clone()),
                namespace: Some(parent.namespace.clone()),
                kind: WorkEdgeKind::Parent,
                from_id: child.id.clone(),
                to_id: parent.item_id.clone(),
            })
            .await
            .map_err(|error| error.to_string())?;
        let work = SetupWorkContext::from_owner(
            backend.pin().clone(),
            parent.clone(),
            WorkItemRef {
                realm_id: child.realm_id,
                namespace: child.namespace,
                item_id: child.id,
            },
            self.parent_owner.clone(),
            child_owner,
            self.cycle.clone(),
        )
        .map_err(|error| error.to_string())?;
        work.check_current(backend)
            .await
            .map_err(|error| error.to_string())?;
        let expected = PrivateOriginalRef {
            route: event.route.clone(),
            original: event.original.clone(),
            event_sha256: format!(
                "{:x}",
                Sha256::digest(canonical_json(&event).map_err(|error| error.to_string())?)
            ),
        };
        let retained = self
            .client
            .lock()
            .await
            .call(json!({"kind":"retain_authenticated_event", "event":event}))
            .await?;
        let original: PrivateOriginalRef = serde_json::from_value(retained["original"].clone())
            .map_err(|error| error.to_string())?;
        if retained["kind"] != "authenticated_event_retained"
            || retained["state"] != "retained_pending_native_admission"
            || original != expected
        {
            return Err("retention did not name this actual source event".into());
        }
        let launch = Arc::new(ChildLaunch {
            original: original.clone(),
            member,
            work,
            admitted: watch::channel(None).0,
        });
        {
            let mut children = self.children.lock().unwrap();
            if children.iter().any(|prior| prior.original == original) {
                return Err("duplicate child source refused".into());
            }
            children.push(launch.clone());
        }
        let reply = self
            .client
            .lock()
            .await
            .call(json!({"kind":"submit_authenticated_original", "submission":
            PrivateOriginalSubmit { original: original.clone(), content,
                transport_authentication: event.original_authentication }}))
            .await?;
        if reply["kind"] != "original_submitted"
            || serde_json::from_value::<PrivateOriginalRef>(reply["original"].clone())
                .map_err(|error| error.to_string())?
                != original
        {
            return Err("submission reply named another source".into());
        }
        let outcome: PrivateOriginalSubmission =
            serde_json::from_value(reply["outcome"].clone()).map_err(|error| error.to_string())?;
        let PrivateOriginalSubmission::Admitted {
            session_id,
            input_id,
            deduplicated: false,
            turn_ticket: Some(ticket),
            source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
        } = outcome
        else {
            return Err("fresh native Original admission remains unestablished".into());
        };
        launch.admitted.send_replace(Some(Admission {
            session: session_id,
            input: input_id,
            ticket,
        }));
        Ok(original)
    }

    pub(super) async fn join(&self) -> Vec<Result<PrivateOriginalRef, String>> {
        let launches = std::mem::take(&mut *self.launches.lock().unwrap());
        let mut results = Vec::new();
        for launch in launches {
            // A failed child cannot detach the remaining launch operations.
            // Drain all owners before the caller reports any construction or
            // submission failure and before closing the shared SDK transport.
            results.push(match launch.await {
                Ok(result) => result,
                Err(error) => Err(format!("owned SDK launcher failed: {error}")),
            });
        }
        results
    }

    /// Passive test evidence from the same launch records used by
    /// original_binding. These observations cannot attach a group or admit work.
    pub(super) fn observations(&self) -> Vec<Value> {
        self.children
            .lock()
            .unwrap()
            .iter()
            .map(|child| {
                let admitted = child.admitted.borrow();
                json!({"original":child.original,"member":child.member,"work":child.work,
                "session":admitted.as_ref().map(|value|value.session.clone()),
                "input":admitted.as_ref().map(|value|value.input.clone()),
                "ticket":admitted.as_ref().map(|value|value.ticket.clone())})
            })
            .collect()
    }
}

#[async_trait]
impl InstalledOriginalTaskOwner for SdkLaunchOwner {
    async fn original_task(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<TaskContinuation, ToolAdmissionError> {
        self.native.original_task(invocation).await
    }

    async fn original_binding(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<InstalledOriginalTaskBinding, ToolAdmissionError> {
        let launch = self
            .children
            .lock()
            .unwrap()
            .iter()
            .find(|launch| &launch.original == invocation.original())
            .cloned();
        // Preserve unrelated ordinary SDK input. It must not join this fan-out
        // merely because it runs on a selected route or shares a cycle label.
        let Some(launch) = launch else {
            return self.native.original_binding(invocation).await;
        };
        let mut admitted = launch.admitted.subscribe();
        let receipt = tokio::time::timeout(BOUND, async {
            loop {
                if let Some(value) = admitted.borrow().clone() {
                    return Ok(value);
                }
                admitted
                    .changed()
                    .await
                    .map_err(|_| ToolAdmissionError::Unavailable)?;
            }
        })
        .await
        .map_err(|_| ToolAdmissionError::Unavailable)??;
        let planning = self.selected_planning()?;
        let context_transfer = if let Some(source) = self.transfer_source.get() {
            let plan = planning.as_ref().ok_or(ToolAdmissionError::Denied)?;
            let backend = self.backend.get().ok_or(ToolAdmissionError::Unavailable)?;
            // The host explicitly scopes its own actual backend to G/P. No A/B
            // model-facing service receives this namespace or payload reader.
            let service = meerkat_workgraph::WorkGraphService::with_scope(
                backend.service().store().clone(),
                backend.service().default_realm_id(),
                plan.context_namespace()
                    .map_err(|_| ToolAdmissionError::Denied)?,
            );
            // A real foreign scope on the same backend is insufficient, and
            // rejecting it must not alter the canonical workflow or graph.
            assert!(
                source
                    .prepare_workflow_context_transfer(
                        &invocation,
                        plan.clone(),
                        backend.service().clone(),
                        backend,
                        [Id::new("human").unwrap()].into()
                    )
                    .await
                    .is_err()
            );
            let transfer = source
                .prepare_workflow_context_transfer(
                    &invocation,
                    plan.clone(),
                    service.clone(),
                    backend,
                    [Id::new("human").unwrap()].into(),
                )
                .await
                .map_err(|_| ToolAdmissionError::Denied)?;
            // Keep an equal discarded producer candidate for observation and
            // reconciliation controls. The first candidate is the actual
            // binding; the canonical source must resolve both to that owner.
            let equivalent = source
                .prepare_workflow_context_transfer(
                    &invocation,
                    plan.clone(),
                    service.clone(),
                    backend,
                    [Id::new("human").unwrap()].into(),
                )
                .await
                .map_err(|_| ToolAdmissionError::Denied)?;
            assert!(!Arc::ptr_eq(&transfer, &equivalent));
            let mut observed = self.transfers.lock().unwrap();
            if !observed
                .iter()
                .any(|(old, _, _)| old == invocation.original())
            {
                observed.push((invocation.original().clone(), equivalent, service));
            }
            Some(transfer)
        } else {
            None
        };
        let continuation = self.native.original_task(invocation).await?;
        let native = continuation
            .native_original
            .as_ref()
            .ok_or(ToolAdmissionError::Denied)?;
        if native.origin_session_id.to_string() != receipt.session
            || continuation.revision.as_str() != receipt.input
            || native.member.as_ref() != Some(&launch.member)
        {
            return Err(ToolAdmissionError::Denied);
        }
        launch
            .work
            .check_current(self.backend.get().ok_or(ToolAdmissionError::Unavailable)?)
            .await
            .map_err(|_| ToolAdmissionError::Denied)?;
        Ok(InstalledOriginalTaskBinding {
            continuation,
            work_context: Some(launch.work.clone()),
            planning,
            context_transfer,
        })
    }
}

// The same native continuation remains the only result/admission owner.
#[async_trait]
impl SetupContinuationOwner for SdkLaunchOwner {
    async fn verify_admission(
        &self,
        obligation: &ContinuationObligation,
        receipt: &ContextReference,
    ) -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError> {
        self.native.verify_admission(obligation, receipt).await
    }
}
#[async_trait]
impl SetupContinuationDelivery for SdkLaunchOwner {
    async fn authorize_delivery(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<(), ContinuationError> {
        self.native.authorize_delivery(obligation).await
    }
    async fn admit(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationAdmissionEvidence, ContinuationError> {
        self.native.admit(obligation).await
    }
    async fn reconcile(
        &self,
        obligation: &ContinuationObligation,
    ) -> Result<ContinuationReconciliation, ContinuationError> {
        self.native.reconcile(obligation).await
    }
}
