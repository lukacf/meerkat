//! Real R3 rows and notice admissions, followed by process loss and the actual
//! native terminal census. No installed G or physical adoption is claimed.
use super::*;
use crate::application_consent::{
    ApplicationConsentOwner, ConsentApplicationIdentity, ConsentAssociation,
    ConsentIssuerSelection, tests as issuer,
};
use futures_util::FutureExt;
use meerkat_core::{
    ApprovalRecord, ApprovalStatus,
    approval::{
        ApprovalConsentCustody, ApprovalConsentDeliveryState, ApprovalConsentTransition,
        ApprovalConsentWithdrawalReason, ApprovalListFilter,
    },
};
use meerkat_mob::MobSessionService;
use meerkat_runtime::{
    MeerkatMachine, PredecessorOwnerLoss, RunStartHoldReason, RuntimeSessionPersistenceProfile,
    terminal_status::{
        InputTerminalReceiptObservation, InteractionSelector, TerminalWitnessSource,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Read, Write},
    panic::{AssertUnwindSafe, resume_unwind},
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const ROOT_ENV: &str = "CONNECT_MULTIROW_ROUTABLE_ROOT";
const READY_PREFIX: &str = "CONNECT_MULTIROW_ROUTABLE_READY=";
const BOUND: std::time::Duration = std::time::Duration::from_secs(60);
type Service = meerkat_session::PersistentSessionService<FactoryAgentBuilder>;

struct Native {
    service: Arc<Service>,
    machine: Arc<MeerkatMachine>,
    store: Arc<dyn RuntimeStore>,
    stock: Arc<StockGovernance>,
    session: Session,
    config: meerkat_core::Config,
    owner: meerkat::RuntimeDeliveryOwner,
    host: Arc<meerkat::surface::SessionServiceDeliveryHost<Service>>,
    delivery: Option<meerkat::RuntimeDeliveryOwnerHandle>,
    retired: Option<meerkat::RuntimeDeliveryOwnerRetired>,
}
impl Native {
    async fn open(
        storage: &meerkat_core::ResolvedStorage,
        id: &meerkat_core::SessionId,
        consent: &Arc<ApplicationConsentOwner>,
        server: &meerkat_core::McpServerConfig,
    ) -> Result<Self, ConnectError> {
        let (_, bundle) = meerkat::governed_realm::open_stock_realm_persistence_as(
            &meerkat::DiskStorageProvider,
            &storage.locator.state_root,
            storage.locator.realm.as_str(),
            None,
            Some(meerkat_store::RealmOrigin::Explicit),
            Some(storage.layout.clone()),
            DEFAULT_STOCK_AGENT,
        )
        .await
        .map_err(|e| ConnectError::native("Open native consent control realm", e))?;
        let stock = bundle
            .stock_governance()
            .cloned()
            .ok_or_else(|| invalid("Actual stock governance missing"))?;
        let session = bundle
            .session_store()
            .load(id)
            .await
            .map_err(|e| ConnectError::native("Read real committed consent session", e))?
            .ok_or_else(|| invalid("Committed consent session absent"))?;
        let store = bundle.runtime_store();
        require(
            store.session_persistence_profile()
                == RuntimeSessionPersistenceProfile::HeadCanonicalV1,
            "The control requires the actual HeadCanonical store",
        )?;
        let owner = bundle.runtime_delivery_owner();
        let inbox = bundle.runtime_delivery_inbox();
        let bindings = bundle.continuation_bindings();
        let domain = inbox
            .hosting_capability()
            .domain_ref()
            .ok_or_else(|| invalid("Native hosting domain absent"))?;
        let startup = SessionStartup::open(storage.clone()).await?;
        let factory = startup
            .factory
            .with_operation_review(issuer::review(consent));
        let mut config = startup.config.clone();
        config.tools.mcp_servers = vec![server.clone()];
        let mut builder = FactoryAgentBuilder::new(factory, config.clone());
        builder.wait_for_mcp = true;
        // Unlike the default reconfigure wrapper, this public composition does
        // not arm an inbox owner. The single explicit owner carries notices.
        let (service, machine) =
            meerkat::surface::build_runtime_backed_service_with_capacities(builder, 2, 2, bundle)
                .map_err(|e| ConnectError::native("Compose native consent control", e))?;
        let service = Arc::new(service);
        let host = Arc::new(
            meerkat::surface::SessionServiceDeliveryHost::new(
                &service,
                &machine,
                meerkat::DetachedJobService::new(owner.job_store()),
                Some(storage.locator.realm.to_string()),
            )
            .with_continuation_bindings(bindings.clone())
            .with_consent_notices(Arc::new(meerkat::ConsentNoticeProjector::new(
                Arc::new(consent.approvals()),
                domain,
                inbox,
                &machine,
            ))),
        );
        bindings.announce_runless_input_terminals_of(&machine);
        let owner = owner
            .with_attachment_commits(machine.subscribe_attachment_commits())
            .with_run_settlements(machine.subscribe_run_settlements())
            .with_source_changes(bindings.subscribe_source_changes());
        Ok(Self {
            service,
            machine,
            store,
            stock,
            session,
            owner,
            host,
            delivery: None,
            retired: None,
            config,
        })
    }
    fn arm(&mut self) {
        assert!(self.delivery.is_none());
        self.delivery = Some(
            self.owner
                .clone()
                .arm(self.host.clone())
                .expect("one actual notice owner"),
        );
    }
    async fn retire_delivery(&mut self) -> Result<(), ConnectError> {
        if let Some(owner) = self.delivery.as_ref() {
            let mut timed_out = false;
            loop {
                match owner.retire().join(BOUND).await {
                    Ok(report) => {
                        self.retired = Some(report);
                        break;
                    }
                    Err(_) => timed_out = true,
                }
            }
            self.delivery.take();
            require(
                !timed_out,
                "Delivery retirement exceeded its bound before the same owner joined",
            )?;
        }
        Ok(())
    }
    async fn attach(
        &self,
        storage: &meerkat_core::ResolvedStorage,
        server: &meerkat_core::McpServerConfig,
    ) -> Result<ConnectSession, ConnectError> {
        let id = self.session.id().clone();
        let admission = self
            .service
            .reserve_create_session_admission()
            .await
            .map_err(|e| ConnectError::native("Reserve real consent actor", e))?;
        let mut prepared = self
            .machine
            .prepare_session_materialization(id.clone())
            .await
            .map_err(|e| ConnectError::native("Prepare real consent actor", e))?;
        let registration = self
            .machine
            .current_session_registration_witness(&id)
            .await
            .ok_or_else(|| invalid("Actual consent registration absent"))?;
        let result = async {
            self.stock
                .bind_session_agent(&self.machine, &id, [server.name.as_str()])
                .await
                .map_err(|e| ConnectError::native("Bind actual consent tool mandate", e))?;
            let model = meerkat::resolve_create_session_model(
                &self.config,
                native_model_peer::model_request(REALM, MODEL)
                    .map_err(|e| ConnectError::native("Select fixture model", e))?,
            )
            .map_err(|e| ConnectError::native("Resolve fixture model", e))?;
            let mut build = meerkat_core::SessionBuildOptions {
                mcp_servers: vec![server.clone()],
                realm_id: Some(storage.locator.realm.clone()),
                provider: Some(model.provider),
                auth_binding: model.auth_binding,
                ..Default::default()
            };
            build.resume_existing_session(self.session.clone());
            let request = CreateSessionRequest {
                model: model.model,
                prompt: String::new().into(),
                injected_context: Vec::new(),
                system_prompt: meerkat::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                build: Some(build),
                labels: None,
            };
            let (_, slot) = Box::pin(
                meerkat::surface::materialize_prepared_session_actor_unattached_with_actor_slot(
                    &self.service,
                    &self.machine,
                    &mut prepared,
                    self.session.clone(),
                    request,
                    admission,
                ),
            )
            .await
            .map_err(|e| ConnectError::native("Materialize real consent actor", e))?;
            self.machine
                .ensure_session_with_executor(
                    id.clone(),
                    Box::new(
                        meerkat::surface::PersistentRuntimeExecutor::new(
                            self.service.clone(),
                            self.machine.clone(),
                            id.clone(),
                        )
                        .with_publication_actor_slot(slot),
                    ),
                )
                .await
                .map_err(|e| ConnectError::native("Attach real consent executor", e))?;
            self.machine
                .current_executor_attachment_witness(&id)
                .await
                .ok_or_else(|| invalid("Real consent executor witness absent"))
        }
        .await;
        let attachment =
            match result {
                Ok(value) => value,
                Err(primary) => {
                    return combine(
                        Err(primary),
                        prepared.rollback_now().await.map(|_| ()).map_err(|e| {
                            ConnectError::native("Roll back refused consent actor", e)
                        }),
                    );
                }
            };
        Ok(ConnectSession {
            service: self.service.clone(),
            machine: self.machine.clone(),
            session_id: id,
            realm: storage.locator.clone(),
            registration,
            attachment,
            _research: None,
            service_preparation: None,
            _connector_auth: None,
            stock: Some(self.stock.clone()),
            stock_submission: Arc::new(super::super::super::StockSubmission::default()),
        })
    }
    async fn close(&mut self) -> Result<(), ConnectError> {
        let delivery = self.retire_delivery().await;
        let native = match self
            .machine
            .current_session_registration_witness(self.session.id())
            .await
        {
            Some(witness) => self
                .machine
                .unregister_session_registration_until_terminal_if_current(&witness)
                .await
                .map(|_| ())
                .map_err(|e| ConnectError::native("Join exact consent registration", e)),
            None => Ok(()),
        };
        let service = self
            .service
            .try_shutdown()
            .await
            .map_err(|e| ConnectError::native("Join real consent service", e));
        combine(delivery, combine(native, service))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    root: PathBuf,
    session: meerkat_core::SessionId,
    identity: ConsentApplicationIdentity,
    issuer: ConsentIssuerSelection,
    association: ConsentAssociation,
    rows: Vec<ApprovalRecord>,
    foreign: ApprovalRecord,
    original_inputs: Vec<meerkat_core::lifecycle::InputId>,
    server: meerkat_core::McpServerConfig,
    approved_inputs: Vec<meerkat_core::lifecycle::InputId>,
}
fn delivery_ids(
    rows: &[ApprovalRecord],
    transition: ApprovalConsentTransition,
) -> Vec<meerkat_core::lifecycle::InputId> {
    rows.iter()
        .filter_map(
            |row| match &row.consent.as_ref()?.delivery(transition)?.state {
                ApprovalConsentDeliveryState::Admitted { input_id, .. } => Some(
                    serde_json::from_value(serde_json::json!(input_id))
                        .expect("actual admitted input ID"),
                ),
                _ => None,
            },
        )
        .collect()
}
fn scoped(
    owner: &ApplicationConsentOwner,
    scope: &meerkat_core::approval::ApprovalConsentScope,
) -> Vec<ApprovalRecord> {
    let mut rows: Vec<_> = owner
        .approvals()
        .list(ApprovalListFilter::default())
        .unwrap()
        .into_iter()
        .filter(|row| row.consent.as_ref().and_then(|c| c.scope.as_ref()) == Some(scope))
        .collect();
    rows.sort_by(|a, b| a.approval_id.cmp(&b.approval_id));
    rows
}
async fn wait_rows(
    owner: &ApplicationConsentOwner,
    scope: &meerkat_core::approval::ApprovalConsentScope,
    delivery: &meerkat::RuntimeDeliveryOwnerHandle,
    accept: impl Fn(&[ApprovalRecord]) -> bool,
) -> Vec<ApprovalRecord> {
    tokio::time::timeout(BOUND, async {
        let approvals = owner.approvals();
        let mut changed = approvals.subscribe_consent_deliveries();
        let mut passes = delivery.subscribe_passes();
        let mut passes_open = true;
        let mut reported_failures = Vec::new();
        loop {
            // Observe this owner's completed passes without substituting them
            // for the actual approval rows and admitted notice inputs below.
            let pass = passes.borrow_and_update().clone();
            if !pass.failures.is_empty() && pass.failures != reported_failures {
                eprintln!(
                    "routable consent delivery pass {}: {:?}",
                    pass.generation, pass.failures
                );
            }
            reported_failures = pass.failures;
            let rows = scoped(owner, scope);
            if accept(&rows) {
                return rows;
            }
            tokio::select! {
                result = changed.changed() => result.expect("actual approval owner remains open"),
                result = passes.changed(), if passes_open => passes_open = result.is_ok(),
            }
        }
    })
    .await
    .expect("bounded actual consent transition/delivery")
}

async fn durable_inputs(native: &Native, expected: &HashSet<meerkat_core::lifecycle::InputId>) {
    for id in expected {
        let observed = native
            .machine
            .input_terminal_receipt_observation(
                native.session.id(),
                InteractionSelector::InputId(id.clone()),
            )
            .await
            .unwrap()
            .expect("actual terminal witness");
        assert_eq!(observed.source, TerminalWitnessSource::DurableStore);
        let InputTerminalReceiptObservation::Finalized(receipt) = observed.report else {
            panic!("a queued/migrated row is not a terminal receipt")
        };
        assert_eq!(receipt.input_id(), id);
        let recipients = receipt.recipient_input_ids();
        assert!(
            recipients.contains(id),
            "the observed input belongs to its actual terminal batch"
        );
        assert_eq!(
            recipients.first(),
            Some(receipt.owner_input_id()),
            "the first recipient owns the batch"
        );
        assert!(
            recipients
                .iter()
                .all(|recipient| expected.contains(recipient)),
            "the terminal batch contains only inputs in the exact observed census"
        );
    }
}

fn release_setup(old: Opened) -> tempfile::TempDir {
    let Opened {
        session,
        store,
        stock,
        client,
        model_peer,
        mcp_calls,
        setup,
        approvals,
        connectors,
        refused_close,
        continuation,
        approval_presenter,
        _host_presenter,
        _directory,
    } = old;
    drop((
        session,
        store,
        stock,
        client,
        model_peer,
        mcp_calls,
        setup,
        approvals,
        connectors,
        refused_close,
        continuation,
        approval_presenter,
        _host_presenter,
    ));
    _directory
}

/// The private child is the only predecessor producer. Its native model client,
/// staged controller, grants, tool context and issued retained work are real.
/// The loopback peer supplies transport responses, never a run or input receipt.
#[tokio::test]
#[ignore = "private child of routable multi-row consent census"]
async fn routable_consent_predecessor_child() {
    let parent = PathBuf::from(std::env::var_os(ROOT_ENV).expect("parent-owned root"));
    let client = Arc::new(ScriptedProvider::default());
    let observed = client.clone();
    let peer = ModelPeer::open(
        MODEL,
        Arc::new(move |body| Box::pin(std::future::ready(observed.reply(body)))),
    )
    .await
    .unwrap();
    let mut setup: Option<Opened> = None;
    let mut directory = None;
    let mut runtime: Option<Native> = None;
    let mut active: Option<ConnectSession> = None;
    let mut issuer_owner = None;
    let caught: Result<(), _> = AssertUnwindSafe(async {
        // Observe actual stdio helper PIDs for later liveness checks. They are
        // never used to mint native process custody or to kill foreign groups.
        let peer_source = format!("printf '%s\\n' \"$$\" >> \"$1.pids\"\n{MCP_PEER}");
        setup = Some(Opened::open_with_selected_mcp_in(None, &peer, client.clone(),
            &peer_source, Some(&parent)).await.unwrap());
        let opened = setup.as_ref().unwrap();
        let selected = opened.continuation.storage.clone();
        let id = opened.continuation.session_id.clone();
        let server = opened.continuation.server.clone();
        setup.as_mut().unwrap().finish().await.unwrap();
        directory = Some(release_setup(setup.take().unwrap()));
        let root = directory.as_ref().unwrap().path().canonicalize().unwrap();
        // Trusted fixture administration changes only this local realm's review
        // rule. The controller and exact MCP grants above were really reviewed.
        let config_path = crate::realm::config_source(&selected).unwrap()
            .config_doc_path(&selected.locator.realm);
        let mut config = std::fs::read_to_string(&config_path).unwrap();
        config.push_str("\n[[governance.review.rules]]\ntarget = { kind = \"mcp_server\", server = \"toolkit-setup\" }\ntier = \"r3\"\nconsent_assurance = \"person_authenticated\"\n");
        std::fs::write(&config_path, config).unwrap();
        let identity = issuer::identity_for_realm(selected.locator.realm.clone());
        let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
        issuer_owner = Some(owner.clone());
        runtime = Some(Native::open(&selected, &id, &owner, &server).await.unwrap());
        let native = runtime.as_mut().unwrap();
        active = Some(native.attach(&selected, &server).await.unwrap());
        let claim = native.machine.grant_session_hosting(&id).unwrap();
        assert!(claim.is_cross_process());
        let association = owner.bind_association(issuer::artifact(&root.join("association.json"),
            b"{\"fixture\":\"three actual native R3 runs\"}"), &claim).unwrap();
        let mut original_inputs = Vec::new();
        for number in 0..3 {
            client.1.lock().unwrap().push_back((format!("routable-{number}"),
                "setup_help".into(), serde_json::json!({})));
            let (accepted, completion) = active.as_ref().unwrap().submit(format!("Request number {number}")).await.unwrap();
            let AcceptOutcome::Accepted { input_id, .. } = accepted else { panic!("actual native admission required") };
            let completion = completion.try_wait_with_terminal_outcome().await.unwrap();
            let (outcome, terminal) = completion.into_parts();
            assert!(matches!(outcome, CompletionOutcome::Completed(_)));
            assert_eq!(terminal, Some(meerkat_runtime::InputTerminalOutcome::Consumed));
            original_inputs.push(input_id);
        }
        let rows = scoped(&owner, association.scope());
        assert_eq!(rows.len(), 3);
        let mut origins = HashSet::new();
        for row in &rows {
            let consent = row.consent.as_ref().unwrap();
            assert_eq!(row.status, ApprovalStatus::Pending);
            assert!(consent.routable() && consent.withdrawal.is_none() && consent.consumed_at.is_none());
            assert!(matches!(consent.custody, Some(ApprovalConsentCustody::Parked)));
            assert!(consent.deliveries.is_empty());
            let work = &consent.origin.as_ref().unwrap().retained_work;
            assert_eq!(work.runtime_id(), LogicalRuntimeId::for_session(&id).0.as_str());
            assert_eq!(work.contributors().len(), 1);
            let input = work.contributors()[0].input_id.clone();
            assert!(original_inputs.contains(&input));
            assert!(origins.insert(input));
            let actual = native.machine.input_terminal_receipt_observation(&id,
                InteractionSelector::InputId(work.contributors()[0].input_id.clone())).await.unwrap().unwrap();
            assert_eq!(actual.source, TerminalWitnessSource::DurableStore);
            let InputTerminalReceiptObservation::Finalized(receipt) = actual.report else { panic!("real R3 run terminal") };
            assert_eq!(receipt.run_id(), Some(work.run_id()));
        }
        assert_eq!(origins.len(), 3, "three real native origins, no copied run identity");
        let held = native.machine.hold_run_starts(&id, RunStartHoldReason::MobStop).await.unwrap();
        assert!(held.current_run.is_none());
        for row in &rows[..2] { issuer::approve(&owner, &row.approval_id); }
        native.arm();
        let rows = wait_rows(&owner, association.scope(), native.delivery.as_ref().unwrap(), |rows|
            rows.len() == 3 && delivery_ids(rows, ApprovalConsentTransition::Approved).len() == 2).await;
        let approved_inputs = delivery_ids(&rows, ApprovalConsentTransition::Approved);
        assert_eq!(approved_inputs.iter().collect::<HashSet<_>>().len(), 2);
        let stored = native.store.load_input_states_strict(&LogicalRuntimeId::for_session(&id)).await.unwrap();
        for input in &approved_inputs {
            let row = stored.iter().find(|row| &row.state.input_id == input).unwrap();
            assert_eq!(row.seed.phase, meerkat_runtime::input_state::InputLifecycleState::Queued);
            assert!(row.seed.terminal_outcome.is_none());
        }
        // A genuine issuer row outside the target session/domain is a negative
        // census sentinel. It is intentionally not claimed as routable work.
        let foreign_session = meerkat_core::SessionId::new();
        let foreign_hosting = meerkat_core::session_hosting::HostingCapability::OsLock(Arc::new(
            meerkat_core::session_hosting::HostingPaths { hosting_lock_dir: root.join("foreign-hosting"),
                cold_delivery_lock: root.join("foreign-delivery.lock"), database: None }));
        let foreign_claim = meerkat_core::session_hosting::grant_session_hosting(&foreign_hosting,
            &meerkat_core::session_hosting::HostingOwner::mint(), &foreign_session).unwrap();
        let probe = Arc::new(issuer::Probe::default());
        let foreign_id = issuer::request(&probe, &issuer::context(&owner, &foreign_claim), "foreign-row").await;
        let foreign = owner.approvals().get(&foreign_id).unwrap();
        assert_ne!(foreign.consent.as_ref().unwrap().scope.as_ref(), Some(association.scope()));
        assert_eq!(probe.0.load(Ordering::SeqCst), 0);
        assert!(!root.join("unexpected-mcp-calls").exists(), "R3 issued consent before any MCP effect");
        assert!(std::fs::read_to_string(root.join("unexpected-mcp-calls.pids")).unwrap().lines().count() >= 2,
            "both actual setup and resumed native MCP peers were observed");
        let ready = Ready { root, session: id, identity, issuer: owner.selection().clone(), association,
            rows, foreign, original_inputs, approved_inputs, server };
        println!("{READY_PREFIX}{}", serde_json::to_string(&ready).unwrap());
        std::io::stdout().flush().unwrap();
        let mut release = [0]; std::io::stdin().read_exact(&mut release).unwrap();
        assert_eq!(release, [1]);
        // Deliberate process loss leaves real native queued notice admissions
        // and unspent approval rows. This is not a graceful scope-drop proxy.
        std::process::exit(77);
    }).catch_unwind().await;
    let mut failures = Vec::new();
    if let Some(native) = runtime.as_mut()
        && native.retire_delivery().await.is_err()
    {
        failures.push("delivery retirement");
    }
    if let Some(session) = active.take()
        && session.close().await.is_err()
    {
        failures.push("actual session close");
    }
    if let Some(native) = runtime.as_mut()
        && native.close().await.is_err()
    {
        failures.push("actual native close");
    }
    if let Some(opened) = setup.as_mut()
        && opened.finish().await.is_err()
    {
        failures.push("staged setup close");
    }
    if peer.finish().await.is_err() {
        failures.push("model peer join");
    }
    drop((runtime, issuer_owner));
    if let Some(directory) = directory {
        let _retained = directory.keep();
    }
    if !failures.is_empty() {
        let _ = writeln!(std::io::stderr(), "child cleanup failures: {failures:?}");
    }
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
    panic!("the predecessor must exit only through the parent's release");
}

async fn helpers_gone(root: &Path) -> Result<(), String> {
    let bytes = match std::fs::read_to_string(root.join("unexpected-mcp-calls.pids")) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("read actual helper observations: {error}")),
    };
    let pids: Vec<u32> = bytes
        .lines()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|error| format!("parse actual helper observation: {error}"))?;
    tokio::time::timeout(BOUND, async {
        loop {
            let mut live = false;
            for pid in &pids {
                // Observation only: this fixture does not grant itself kill
                // authority over a PID read from an artifact. Zombies are dead.
                let output = std::process::Command::new("/bin/ps")
                    .args(["-p", &pid.to_string(), "-o", "stat="])
                    .output()
                    .map_err(|error| format!("observe exact helper: {error}"))?;
                let status = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
                if !output.status.success()
                    && !(output.status.code() == Some(1) && status.trim().is_empty())
                {
                    return Err("process observation failed; cannot call helper stopped".into());
                }
                if output.status.success()
                    && !status.trim().is_empty()
                    && !status.trim().starts_with('Z')
                {
                    live = true;
                }
            }
            if !live {
                break Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| "observed native helper remained live after cleanup".to_owned())?
}

#[test]
fn routed_multirow_census_requires_every_real_notice_terminal_and_repeats_stably() {
    durable::run();
}

#[path = "routable_consent/durable.rs"]
mod durable;

#[path = "routable_consent/current_stock.rs"]
mod current_stock;
