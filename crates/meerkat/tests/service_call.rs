//! Governed service calls: a scheduled host runnable's connector calls,
//! admitted through the machine's installed native host under a service
//! original the trusted host enrolled, and decided per call by the
//! operator's grant for exactly the installation's MCP server.
//!
//! The fixture's admitted-work policy is deliberately permissive (it checks
//! nothing), so every refusal these rows observe is Core's own: the
//! enrollment comparison, the grant lineage, the connector approval the
//! grant records, the review tier and the connector credential.
//!
//! The machine is persistent over an in-memory runtime store, whose
//! service-call audit log is the calls' audit owner: the rows read each
//! occurrence's observations back from it, and script its appends.
#![cfg(all(unix, feature = "local-authorization", feature = "mcp"))]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance, single_tool_operation_target,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::mcp_installation::{mcp_connector_action, mcp_connector_domain};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::audit::{AuditObservation, AuthorizationAuditObservation};
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DomainRestriction, ExactRestriction, ExecutionRestrictions,
    LifetimeRestriction, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, TargetGrantLineage,
    WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, ControllerReadinessFailure, OperationRefusalKind, OperationRefused,
    OperationReviewTier, PreparedAuthorizationBinding, ToolAuthorizationTarget,
};
use meerkat_core::connection::{
    AuthCredentialIdentity, CredentialAccountId, CredentialAccountRef, RealmId,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::mcp_config::{McpConnectorBinding, McpConnectorOperation};
use meerkat_core::{
    AgentToolDispatcher, ControllerModelSelection, McpServerConfig, OperationAuthorizationError,
    PrincipalKind, PrincipalRef, Provider, SessionLlmIdentity, ToolCallView, ToolDef,
    ToolDispatchOutcome, ToolError, ToolExecutionResolutionContext, ToolOperationTarget,
    ToolResult, TrustDomainId,
};
use meerkat_providers::auth_oauth::OAuthTokenResult;
use meerkat_providers::auth_store::{
    EphemeralTokenStore, InMemoryCoordinator, ProviderAuthPersistence,
};
use meerkat_providers::connector_login::{
    ConnectorAccountStrategy, ConnectorOAuthCallback, ConnectorOAuthTarget,
};
use meerkat_providers::connector_oauth::{
    AccountSelection, ConnectorAccountObservation, ConnectorOAuthDescriptor, ConnectorOAuthRefusal,
};
use meerkat_runtime::input_authority::{
    NativeAdmissionError, NativeIngressContext, ServiceCallRequest, ServiceOccurrence,
};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, EnrolledServiceOriginal,
    OriginalDisposition, RetainedCustody, ScopedGrantComposition, ScopedInvocation,
    ScopedMandateOwner, ServiceCall, ServiceCallAuditError, ServiceCallOutcome,
    original_operation_grant, require_source_original_live,
};
use meerkat_runtime::{
    InMemoryRuntimeStore, MeerkatMachine, RuntimeStore, RuntimeStoreError, ServiceCallAuditBatch,
    ServiceCallAuditKey,
};

const STRATEGY: &str = "service-call-fixture-v1";
const SUBJECT: &str = "subject-a";
const OTHER_SUBJECT: &str = "subject-b";
const ACCESS: &str = "access-a";
const OTHER_ACCESS: &str = "access-b";
const CODE: &str = "code-a";
const OTHER_CODE: &str = "code-b";
const GRANTED_SCOPE: &str = "mail.read mail.modify";
const REDIRECT: &str = "http://127.0.0.1:41001/connector/oauth/callback";
const REALM: &str = "tenant-a";
const SLOT: &str = "mail-work";
const CLIENT: &str = "client";
const RESOURCE: &str = "https://mail.example";
const DESTINATION: &str = "https://mail.example";
const SERVER: &str = "mail";
const SECOND_SERVER: &str = "calendar";
const LIMIT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Identities and the service original
// ---------------------------------------------------------------------------

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(kind: PrincipalKind, name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        kind,
        name,
        TrustDomainId::new("service-call").expect("domain"),
    )
    .expect("qualified fixture principal")
}

fn operator() -> PrincipalRef {
    principal(PrincipalKind::Human, "operator")
}

fn collector() -> PrincipalRef {
    principal(PrincipalKind::ServiceAccount, "collector")
}

fn host() -> PrincipalRef {
    principal(PrincipalKind::RuntimeHost, "collector-host")
}

fn ingress_domain() -> ResourceDomain {
    ResourceDomain {
        authority: host(),
        namespace: "collector-setups".into(),
    }
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ingress_domain(),
            resource_id: value.into(),
        },
        revision: id("revision-1"),
        digest: EvidenceDigest::from_array([7; 32]),
    }
}

fn namespace() -> QualifiedIngressNamespace {
    QualifiedIngressNamespace {
        realm: RealmId::parse(REALM).expect("realm"),
        ingress: ingress_domain(),
        occurrence_scope: id("collection"),
    }
}

fn source_target() -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: operator(),
        logical_runtime: id("source/mob-1"),
        context: id("inbox"),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: operator(),
        },
    }
}

/// The candidate of the service original the host enrolls for `setup`,
/// selecting `lineage` for exactly `server`.
fn candidate(
    setup: &str,
    server: &str,
    lineage: &GrantLineageRef,
) -> InputAuthorityAssociationCandidate {
    InputAuthorityAssociationCandidate {
        requester: operator(),
        ingress_actor: host(),
        represented_subject: None,
        original_authentication: evidence("setup-review"),
        logical_executor: collector(),
        target: source_target(),
        original_work: OriginalWorkRef {
            authority: host(),
            work: id(setup),
        },
        root_event: evidence("setup-finalized"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: server.into(),
                },
                lineage: vec![lineage.clone()],
            }],
        },
        controller_grant_lineage: Vec::new(),
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: namespace(),
        contract: ContractRequirements::local_governed_v1(),
    }
}

fn association(candidate: InputAuthorityAssociationCandidate) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(candidate).expect("a valid association")
}

fn occurrence(name: &str) -> ServiceOccurrence {
    ServiceOccurrence::new(
        id("schedule-1"),
        id(name),
        id(&format!("schedule:schedule-1:occurrence:{name}")),
    )
}

// ---------------------------------------------------------------------------
// Grants: the operator's grant to the collector, for exactly one installation
// ---------------------------------------------------------------------------

fn mcp_action(operation: &str) -> ActionRef {
    ActionRef {
        feature: "mcp".into(),
        action: operation.into(),
    }
}

fn installation_domain(server: &str) -> ResourceDomain {
    ResourceDomain {
        authority: operator(),
        namespace: format!("mcp-server/{server}"),
    }
}

/// The operator's restrictions for the collector on `server`: exactly
/// `operations`, the installation's domain and, when `approval` is given,
/// the connector approval of the reviewed binding.
fn restrictions(
    server: &str,
    operations: &[&str],
    approval: Option<&meerkat_core::McpConnectorApproval>,
    lifetime: LifetimeRestriction,
) -> ExecutionRestrictions {
    covering(&[(server, operations, approval)], lifetime)
}

/// The operator's restrictions for the collector on every installation of
/// `installations`: (server, its operations, its connector approval).
fn covering(
    installations: &[(&str, &[&str], Option<&meerkat_core::McpConnectorApproval>)],
    lifetime: LifetimeRestriction,
) -> ExecutionRestrictions {
    let mut actions: Vec<ActionRef> = Vec::new();
    let mut domains = Vec::new();
    for (server, operations, approval) in installations {
        actions.extend(operations.iter().map(|op| mcp_action(op)));
        domains.push(installation_domain(server));
        if let Some(approval) = approval {
            actions.push(mcp_connector_action(server, approval));
            domains.push(mcp_connector_domain(&operator(), server, approval));
        }
    }
    ExecutionRestrictions {
        actions: ExactRestriction::exact(actions),
        resource_domains: DomainRestriction::exact(domains),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: collector(),
        }]),
        lifetime,
        ..ExecutionRestrictions::unrestricted()
    }
}

struct NoController;

impl ControllerGrantMutationCustody for NoController {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

// ---------------------------------------------------------------------------
// The host's owners
// ---------------------------------------------------------------------------

/// The host's admitted-work policy. Deliberately permissive: every refusal
/// these rows observe is Core's, not the host's.
struct Permissive;

impl ScopedMandateOwner for Permissive {
    fn authorize_invocation(
        &self,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Ok(())
    }
}

impl AdmittedWorkPolicyOwner for Permissive {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }

    fn review_mandate(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        Ok(None)
    }
}

/// The operation owner: an MCP server operation's tuples are the raw
/// operation and the connector approval of the connection it runs on, both
/// on the installation's domain, as the stock owner maps them. Operations in
/// `reviewed` need an R2 review.
struct Operations {
    reviewed: BTreeSet<String>,
}

impl OperationPolicyOwner for Operations {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let refused = || OperationRefused::new(OperationRefusalKind::Denied).into();
        if purpose != LocalPolicyPurpose::Operation {
            return Err(refused());
        }
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(refused());
        };
        let ToolAuthorizationTarget::Dispatcher(plan) = &tool.target else {
            return Err(refused());
        };
        let Some(ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            connector,
            ..
        }) = single_tool_operation_target(plan)?
        else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        let claims = association.candidate();
        let tuple = |action: ActionRef| LocalOperationValues {
            action,
            resource_domain: installation_domain(server),
            processor: ProcessorRef::Principal {
                principal: claims.logical_executor.clone(),
            },
            audience: claims.target.audience.clone(),
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![
                tuple(mcp_action(raw_operation)),
                tuple(mcp_connector_action(server, connector)),
            ],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: if self.reviewed.contains(raw_operation) {
                OperationReviewTier::R2
            } else {
                OperationReviewTier::R1
            },
        })
    }
}

/// The host's original source: service originals it enrolled, admitted
/// originals it retains, and the originals whose operations it cancelled.
#[derive(Default)]
struct Originals {
    enrolled: Mutex<Vec<InputAuthorityAssociation>>,
    retained: Mutex<Vec<InputAuthorityAssociation>>,
    cancelled: Mutex<Vec<OriginalWorkRef>>,
    unreadable: AtomicBool,
}

impl Originals {
    fn enroll(&self, association: &InputAuthorityAssociation) {
        self.enrolled.lock().unwrap().push(association.clone());
    }

    fn revoke(&self, original: &OriginalWorkRef) {
        self.enrolled
            .lock()
            .unwrap()
            .retain(|enrolled| &enrolled.candidate().original_work != original);
    }

    fn retain_admitted(&self, association: &InputAuthorityAssociation) {
        self.retained.lock().unwrap().push(association.clone());
    }

    fn cancel(&self, original: &OriginalWorkRef) {
        self.cancelled.lock().unwrap().push(original.clone());
    }

    fn readable(&self) -> Result<(), ControllerReadinessFailure> {
        if self.unreadable.load(Ordering::SeqCst) {
            Err(ControllerReadinessFailure::PolicyUnavailable)
        } else {
            Ok(())
        }
    }
}

fn names(
    association: &InputAuthorityAssociation,
    namespace: &QualifiedIngressNamespace,
    original: &OriginalWorkRef,
) -> bool {
    &association.candidate().ingress_namespace == namespace
        && &association.candidate().original_work == original
}

impl AuthenticatedOriginalSource for Originals {
    fn authenticated_original(
        &self,
        _: &NativeIngressContext,
        _: &QualifiedIngressNamespace,
        _: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        Ok(None)
    }

    fn retained_custody(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        self.readable()?;
        Ok(self
            .retained
            .lock()
            .unwrap()
            .iter()
            .find(|retained| names(retained, namespace, original))
            .map_or(RetainedCustody::NotRetained, |retained| {
                RetainedCustody::Retained(retained.candidate().authority_basis.clone())
            }))
    }

    fn original_disposition(
        &self,
        _: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        self.readable()?;
        Ok(
            if self.cancelled.lock().unwrap().iter().any(|o| o == original) {
                OriginalDisposition::OperationCancelled {
                    evidence: evidence("cancelled"),
                }
            } else {
                OriginalDisposition::Active
            },
        )
    }

    fn enrolled_service_original(
        &self,
        namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<EnrolledServiceOriginal>, ControllerReadinessFailure> {
        self.readable()?;
        Ok(self
            .enrolled
            .lock()
            .unwrap()
            .iter()
            .find(|enrolled| names(enrolled, namespace, original))
            .map(|enrolled| EnrolledServiceOriginal::new(enrolled.clone())))
    }
}

/// The decisions that allowed a call, among an occurrence's observations.
fn prepared(observations: &[AuthorizationAuditObservation]) -> usize {
    observations
        .iter()
        .filter(|seen| matches!(seen.observation, AuditObservation::Prepared { .. }))
        .count()
}

/// The refusals of `kind`, among an occurrence's observations.
fn refused(observations: &[AuthorizationAuditObservation], kind: OperationRefusalKind) -> usize {
    observations
        .iter()
        .filter(|seen| {
            matches!(&seen.observation, AuditObservation::Refused { reason, .. } if *reason == kind)
        })
        .count()
}

/// The kind of one observation, for rows that read the log's order.
fn kind(observation: &AuthorizationAuditObservation) -> &'static str {
    match observation.observation {
        AuditObservation::ReviewAttemptStarted { .. } => "review_attempt_started",
        AuditObservation::Prepared { .. } => "prepared",
        AuditObservation::Refused { .. } => "refused",
        AuditObservation::AuthorizationUnavailable { .. } => "authorization_unavailable",
        AuditObservation::Entry { .. } => "entry",
        AuditObservation::Outcome { .. } => "outcome",
        AuditObservation::MandatoryHookWithheld { .. } => "mandatory_hook_withheld",
        _ => "other",
    }
}

fn kinds(observations: &[AuthorizationAuditObservation]) -> Vec<&'static str> {
    observations.iter().map(kind).collect()
}

// ---------------------------------------------------------------------------
// The connector credential: a local issuer and a fixture account strategy
// ---------------------------------------------------------------------------

/// The fixture provider: each access token belongs to one subject.
struct Strategy;

#[async_trait::async_trait]
impl ConnectorAccountStrategy for Strategy {
    fn strategy_id(&self) -> &str {
        STRATEGY
    }

    async fn observe_account(
        &self,
        descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
        _nonce: Option<&str>,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        let account = match tokens.access_token.as_str() {
            ACCESS => SUBJECT,
            OTHER_ACCESS => OTHER_SUBJECT,
            _ => return Err(ConnectorOAuthRefusal::VerificationUnavailable),
        };
        Ok(ConnectorAccountObservation {
            account: account.to_owned(),
            granted_scopes: descriptor.granted_scopes_from_response(tokens),
        })
    }
}

/// A local issuer: discovery, and a token endpoint that exchanges `CODE`
/// for `ACCESS` and `OTHER_CODE` for `OTHER_ACCESS`.
async fn issuer() -> String {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let metadata = serde_json::json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "code_challenge_methods_supported": ["S256"],
    });
    let app = axum::Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(move || {
                let metadata = metadata.clone();
                async move { axum::Json(metadata) }
            }),
        )
        .route(
            "/token",
            post(
                |axum::Form(form): axum::Form<BTreeMap<String, String>>| async move {
                    let access = match form.get("code").map(String::as_str) {
                        Some(CODE) => ACCESS,
                        Some(OTHER_CODE) => OTHER_ACCESS,
                        _ => {
                            return Err((
                                axum::http::StatusCode::BAD_REQUEST,
                                axum::Json(serde_json::json!({"error": "invalid_grant"})),
                            ));
                        }
                    };
                    Ok(axum::Json(serde_json::json!({
                        "access_token": access,
                        "refresh_token": format!("refresh-{access}"),
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "scope": GRANTED_SCOPE,
                    })))
                },
            ),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    base
}

fn slot() -> CredentialAccountRef {
    CredentialAccountRef {
        realm: RealmId::parse(REALM).unwrap(),
        account: CredentialAccountId::parse(SLOT).unwrap(),
    }
}

/// Sign the slot in through the host's own auth service with `code`.
async fn sign_in(host_auth: &meerkat::HostAuthService, issuer: &str, code: &str) {
    let target = ConnectorOAuthTarget {
        slot: slot(),
        issuer: issuer.to_owned(),
        client: CLIENT.into(),
        resource: RESOURCE.into(),
        scopes: ["mail.read".to_owned(), "mail.modify".to_owned()].into(),
        strategy_id: STRATEGY.into(),
        account: AccountSelection::Discover,
        client_auth_profile: None,
    };
    let start = host_auth
        .connector_login_start(&target, REDIRECT)
        .await
        .expect("login admitted");
    host_auth
        .connector_login_complete(
            &target,
            ConnectorOAuthCallback {
                redirect_uri: REDIRECT.into(),
                state: start.state,
                code: code.into(),
                host_attempt_handle: None,
            },
        )
        .await
        .expect("signed in");
}

// ---------------------------------------------------------------------------
// The connector-bound MCP server
// ---------------------------------------------------------------------------

/// A stdio server named `name` exposing `operations`; it appends every
/// `tools/call` request line to `calls`.
fn server(
    name: &str,
    operations: &[&str],
    calls: &Path,
    issuer: &str,
    account: &str,
) -> McpServerConfig {
    let tools = operations
        .iter()
        .map(|op| format!(r#"{{"name":"{op}","inputSchema":{{"type":"object"}}}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"{name}","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{tools}]}}}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf '%s\n' "$line" >> '{calls}'; printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"ok"}}]}}}}\n' "$id" ;;
  esac
done
"#,
        calls = calls.display(),
    );
    let mut config = McpServerConfig::stdio(
        name,
        "/bin/sh",
        vec!["-c".to_owned(), script],
        HashMap::new(),
    );
    config.connector = Some(McpConnectorBinding {
        realm_id: REALM.into(),
        slot_id: SLOT.into(),
        issuer: issuer.to_owned(),
        client: CLIENT.into(),
        resource: RESOURCE.into(),
        strategy_id: STRATEGY.into(),
        account: account.to_owned(),
        operations: operations
            .iter()
            .map(|op| {
                (
                    (*op).to_owned(),
                    McpConnectorOperation {
                        required_scopes: if op.starts_with("archive") {
                            ["mail.modify".to_owned()].into()
                        } else {
                            ["mail.read".to_owned()].into()
                        },
                        destination_origin: DESTINATION.into(),
                        read_only: !op.starts_with("archive"),
                    },
                )
            })
            .collect(),
    });
    config
}

const MAIL_OPERATIONS: [&str; 3] = ["list_messages", "get_message", "archive_message"];
const CALENDAR_OPERATIONS: [&str; 1] = ["list_events"];

/// Every `tools/call` request line the servers received.
fn calls(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON-RPC request line"))
        .collect()
}

// ---------------------------------------------------------------------------
// The governed host
// ---------------------------------------------------------------------------

struct Host {
    _dir: tempfile::TempDir,
    calls: PathBuf,
    issuer: String,
    grants: Arc<LocalGrantAuthority>,
    originals: Arc<Originals>,
    /// The machine's runtime store, whose service-call audit log holds the
    /// occurrences' observations.
    store: Arc<InMemoryRuntimeStore>,
    machine: MeerkatMachine,
    host_auth: meerkat::HostAuthService,
    /// The collector's grant for the mail installation.
    grant: GrantLineageRef,
    /// The approval of the reviewed mail binding.
    approval: meerkat_core::McpConnectorApproval,
    /// The enrolled service original.
    original: InputAuthorityAssociation,
}

struct HostOptions {
    reviewed: BTreeSet<String>,
    lifetime: LifetimeRestriction,
    signed_in_code: &'static str,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            reviewed: BTreeSet::new(),
            lifetime: LifetimeRestriction::unrestricted(),
            signed_in_code: CODE,
        }
    }
}

impl Host {
    async fn new() -> Self {
        Self::with(HostOptions::default()).await
    }

    async fn with(options: HostOptions) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let calls = dir.path().join("calls.log");
        let issuer = issuer().await;
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: operator(),
                    namespace: id("collector-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("the realm's grant owner"),
        );
        let approval =
            server(SERVER, &MAIL_OPERATIONS, &calls, &issuer, SUBJECT).connector_approval();
        let grant = grants
            .issue_root(
                &operator(),
                id("collector-mail"),
                collector(),
                None,
                restrictions(SERVER, &MAIL_OPERATIONS, Some(&approval), options.lifetime),
            )
            .expect("the operator's grant to the collector");
        let originals = Arc::new(Originals::default());
        let original = association(candidate("setup-1", SERVER, &grant));
        originals.enroll(&original);
        let store = Arc::new(InMemoryRuntimeStore::new());
        let machine = MeerkatMachine::persistent_with_local_grant_authorization(
            Arc::clone(&store) as Arc<dyn RuntimeStore>,
            None,
            composition(&grants, &originals, options.reviewed),
        )
        .expect("a governed machine");
        let host_auth = meerkat::HostAuthService::new(
            ProviderAuthPersistence::new(
                Arc::new(EphemeralTokenStore::new()),
                Arc::new(InMemoryCoordinator::new()),
            ),
            machine.provider_auth_runtime_authority(),
        )
        .with_connector_strategy(Arc::new(Strategy));
        sign_in(&host_auth, &issuer, options.signed_in_code).await;
        Self {
            _dir: dir,
            calls,
            issuer,
            grants,
            originals,
            store,
            machine,
            host_auth,
            grant,
            approval,
            original,
        }
    }

    /// The collector's router, with the live connector provider over the
    /// host's own connector owner, and `servers` staged on it.
    async fn router(&self, servers: Vec<McpServerConfig>) -> Arc<meerkat_mcp::McpRouterAdapter> {
        let provider = Arc::new(meerkat::ConnectorMcpCallContextProvider::live(
            self.host_auth
                .connector_oauth_authority()
                .expect("the connector owner"),
        ));
        let mut router = meerkat::mcp::standalone_router_with_stdio_profile(
            meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
        )
        .with_call_context_provider(provider);
        for server in servers {
            router.add_server(server).await.expect("staged");
        }
        Arc::new(meerkat_mcp::McpRouterAdapter::new(router))
    }

    async fn mail_router(&self) -> Arc<meerkat_mcp::McpRouterAdapter> {
        self.router(vec![self.mail()]).await
    }

    fn mail(&self) -> McpServerConfig {
        server(SERVER, &MAIL_OPERATIONS, &self.calls, &self.issuer, SUBJECT)
    }

    fn request(&self, association: InputAuthorityAssociation) -> Arc<ServiceCallRequest> {
        self.request_for(association, "occurrence-1")
    }

    fn request_for(
        &self,
        association: InputAuthorityAssociation,
        occurrence_name: &str,
    ) -> Arc<ServiceCallRequest> {
        ServiceCallRequest::from_trusted_host(association, occurrence(occurrence_name))
            .expect("a service original's request")
    }

    fn prepare(
        &self,
        association: InputAuthorityAssociation,
    ) -> Result<ServiceCall, OperationAuthorizationError> {
        self.machine.prepare_service_call(self.request(association))
    }

    fn admitted(&self) -> ServiceCall {
        self.prepare(self.original.clone())
            .expect("the enrolled original's occurrence is admitted")
    }

    /// The observations the store's service-call audit log holds for `key`,
    /// in order, read through its protected native read.
    async fn audit(&self, key: &ServiceCallAuditKey) -> Vec<AuthorizationAuditObservation> {
        self.store
            .read_service_call_audit(key)
            .await
            .expect("the protected read of the audit log")
            .into_iter()
            .map(|record| record.observation.observation)
            .collect()
    }

    fn calls(&self) -> Vec<serde_json::Value> {
        calls(&self.calls)
    }

    fn revoke_grant(&self, grant: &GrantLineageRef) {
        self.grants
            .revoke(&operator(), grant, &mut NoController)
            .expect("the operator revokes the grant");
    }
}

fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::DirectCaller,
            LIMIT,
        )])
        .expect("finite caller deadline"),
    )
}

/// One call whose observations all reach the audit log: its result.
async fn dispatch<T: AgentToolDispatcher + ?Sized + 'static>(
    call: &ServiceCall,
    dispatcher: &Arc<T>,
    name: &str,
) -> Result<ToolDispatchOutcome, ToolError> {
    let outcome = dispatch_outcome(call, dispatcher, name).await;
    assert!(
        outcome.audit.is_ok(),
        "every observation of the call is in the log: {:?}",
        outcome.audit
    );
    outcome.result
}

/// One call: its result and whether its observations reached the log.
async fn dispatch_outcome<T: AgentToolDispatcher + ?Sized + 'static>(
    call: &ServiceCall,
    dispatcher: &Arc<T>,
    name: &str,
) -> ServiceCallOutcome {
    let args = serde_json::value::to_raw_value(&serde_json::json!({})).unwrap();
    tokio::time::timeout(
        LIMIT,
        call.dispatch(
            dispatcher,
            ToolCallView {
                id: "call-1",
                name,
                args: &args,
            },
            resolution(),
        ),
    )
    .await
    .expect("the call settles")
}

/// The scoped grant composition of the fixture's grant owner and original
/// source, with `reviewed` operations needing review.
fn composition(
    grants: &Arc<LocalGrantAuthority>,
    originals: &Arc<Originals>,
    reviewed: BTreeSet<String>,
) -> meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration {
    ScopedGrantComposition {
        grants: Arc::clone(grants),
        ingress_domain: ingress_domain(),
        originals: Arc::clone(originals) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Permissive)),
        operation_owner: Arc::new(Operations { reviewed }),
        retained_resume: None,
        kept_original: None,
    }
    .into_configuration()
}

fn refusal_kind(result: &Result<ToolDispatchOutcome, ToolError>) -> Option<OperationRefusalKind> {
    match result {
        Err(ToolError::AuthorizationRefused { refusal }) => Some(refusal.kind()),
        _ => None,
    }
}

fn prepare_refusal(
    result: Result<ServiceCall, OperationAuthorizationError>,
) -> OperationAuthorizationError {
    match result {
        Ok(_) => panic!("the service call was admitted"),
        Err(error) => error,
    }
}

fn is_refused(error: &OperationAuthorizationError, kind: OperationRefusalKind) -> bool {
    matches!(error, OperationAuthorizationError::Refused(refusal) if refusal.kind() == kind)
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// An enrolled service original's occurrence is admitted, and its call
/// reaches the connector-bound server with the slot's credential for the
/// verified account in protected `_meta`.
#[tokio::test]
async fn an_enrolled_service_original_reaches_the_connector_with_the_slots_credential() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();

    dispatch(&call, &mail, "list_messages")
        .await
        .expect("the governed call reaches the server");

    let calls = host.calls();
    assert_eq!(calls.len(), 1, "one call reached the server");
    let credential = &calls[0]["params"]["_meta"]["meerkat/connector_credential"];
    assert_eq!(credential["authorization"], format!("Bearer {ACCESS}"));
    assert_eq!(credential["account"]["subject"], SUBJECT);
    assert_eq!(credential["account"]["strategy_id"], STRATEGY);
    assert_eq!(credential["destination_origin"], DESTINATION);
    assert_eq!(
        prepared(&host.audit(call.audit_key()).await),
        1,
        "the decision was audited"
    );
}

/// An ungoverned machine has no native host: the request is refused as
/// unavailable, and the router's own entry (the gap this seam closes) still
/// refuses a connector-bound call without governed work.
#[tokio::test]
async fn an_ungoverned_machine_refuses_a_service_call_before_anything_is_sent() {
    let host = Host::new().await;
    let ungoverned = MeerkatMachine::ephemeral();
    let request = host.request(host.original.clone());
    let key = request.audit_key().clone();
    let refused = prepare_refusal(ungoverned.prepare_service_call(request));
    assert!(matches!(refused, OperationAuthorizationError::Unavailable));

    let mail = host.mail_router().await;
    let args = serde_json::json!({});
    let direct = mail
        .dispatch(ToolCallView {
            id: "direct",
            name: "list_messages",
            args: &serde_json::value::to_raw_value(&args).unwrap(),
        })
        .await;
    assert!(direct.is_err(), "an ungoverned connector call is refused");
    assert!(host.calls().is_empty(), "nothing reached the server");
    assert!(host.audit(&key).await.is_empty());
}

/// A native host that serves no service call (the trait's default).
struct WithoutServiceCalls;

impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for WithoutServiceCalls {
    fn authenticate_association(
        &self,
        _: &meerkat_runtime::identifiers::LogicalRuntimeId,
        _: &meerkat_runtime::Input,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Err(NativeAdmissionError::Readiness(
            ControllerReadinessFailure::PolicyUnavailable,
        ))
    }

    fn work_context(
        &self,
        _: &meerkat_runtime::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_runtime::input_authority::NativeWorkContextError,
    > {
        Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// A host or an admitted-work owner without service call support
/// refuses as unavailable: neither default borrows any authority. (A
/// custom host installs only on a storeless machine, which already refuses
/// a service call for having no audit log.)
#[tokio::test]
async fn a_host_or_owner_without_service_call_support_refuses_unavailable() {
    let host = Host::new().await;

    let custom = MeerkatMachine::ephemeral()
        .with_native_work_authorization_host(Arc::new(WithoutServiceCalls))
        .expect("an installed host");
    let refused = prepare_refusal(custom.prepare_service_call(host.request(host.original.clone())));
    assert!(matches!(refused, OperationAuthorizationError::Unavailable));

    // An owner that is not a scoped composition's keeps the default.
    let unscoped = MeerkatMachine::persistent_with_local_grant_authorization(
        Arc::new(InMemoryRuntimeStore::new()),
        None,
        meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration {
            grants: Arc::clone(&host.grants),
            ingress: Arc::new(|_, _, _, _| Ok(())),
            invocation_owner: Arc::new(Permissive),
            operation_owner: Arc::new(Operations {
                reviewed: BTreeSet::new(),
            }),
            retained_resume: None,
            kept_original: None,
            require_scoped_destination: false,
            configured_mcp_attachment: None,
        },
    )
    .expect("a governed machine");
    let refused =
        prepare_refusal(unscoped.prepare_service_call(host.request(host.original.clone())));
    assert!(matches!(refused, OperationAuthorizationError::Unavailable));
    assert!(host.calls().is_empty());
}

/// An original the host never enrolled, a cancelled one and one the
/// host cannot read now are refused (Denied, Denied, Unavailable), and a
/// revocation or cancellation between two calls of one occurrence refuses
/// the next call locally.
#[tokio::test]
async fn an_unenrolled_cancelled_or_unreadable_original_refuses() {
    let host = Host::new().await;

    let unenrolled = association(candidate("setup-unknown", SERVER, &host.grant));
    let refused = prepare_refusal(host.prepare(unenrolled));
    assert!(
        is_refused(&refused, OperationRefusalKind::Denied),
        "{refused:?}"
    );

    host.originals
        .cancel(&host.original.candidate().original_work);
    let refused = prepare_refusal(host.prepare(host.original.clone()));
    assert!(
        is_refused(&refused, OperationRefusalKind::Denied),
        "{refused:?}"
    );
    host.originals.cancelled.lock().unwrap().clear();

    host.originals.unreadable.store(true, Ordering::SeqCst);
    let refused = prepare_refusal(host.prepare(host.original.clone()));
    assert!(
        matches!(refused, OperationAuthorizationError::Unavailable),
        "{refused:?}"
    );
    host.originals.unreadable.store(false, Ordering::SeqCst);

    let mail = host.mail_router().await;
    let call = host.admitted();
    dispatch(&call, &mail, "list_messages")
        .await
        .expect("the first call is permitted");
    host.originals
        .revoke(&host.original.candidate().original_work);
    let refused = dispatch(&call, &mail, "get_message").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert_eq!(host.calls().len(), 1, "the revoked call was not sent");

    host.originals.enroll(&host.original);
    let call = host.admitted();
    host.originals
        .cancel(&host.original.candidate().original_work);
    let refused = dispatch(&call, &mail, "get_message").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert_eq!(host.calls().len(), 1, "the cancelled call was not sent");
}

/// The operator's revocation between two calls refuses the next call
/// locally; nothing earlier is reused.
#[tokio::test]
async fn a_grant_revoked_between_calls_refuses_the_next_call_locally() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    dispatch(&call, &mail, "list_messages")
        .await
        .expect("permitted before the revocation");
    host.revoke_grant(&host.grant);
    let refused = dispatch(&call, &mail, "list_messages").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert_eq!(
        host.calls().len(),
        1,
        "the call after the revocation was not sent"
    );
}

/// Another connector-bound server on the same router has no lineage in
/// the service original's basis: its call is Denied before its credential
/// is admitted, even under a grant that covers both installations, so only
/// the basis's selection by target keeps the call to the selected server.
#[tokio::test]
async fn a_second_connector_server_has_no_lineage_and_is_denied() {
    let host = Host::new().await;
    let calendar = server(
        SECOND_SERVER,
        &CALENDAR_OPERATIONS,
        &host.calls,
        &host.issuer,
        SUBJECT,
    );
    let calendar_approval = calendar.connector_approval();
    let both_installations = host
        .grants
        .issue_root(
            &operator(),
            id("collector-both"),
            collector(),
            None,
            covering(
                &[
                    (SERVER, &MAIL_OPERATIONS, Some(&host.approval)),
                    (
                        SECOND_SERVER,
                        &CALENDAR_OPERATIONS,
                        Some(&calendar_approval),
                    ),
                ],
                LifetimeRestriction::unrestricted(),
            ),
        )
        .expect("a grant covering both installations");
    let original = association(candidate("setup-both", SERVER, &both_installations));
    host.originals.enroll(&original);
    let both = host.router(vec![host.mail(), calendar]).await;
    let call = host.prepare(original).expect("admitted");
    let refused = dispatch(&call, &both, "list_events").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert!(host.calls().is_empty(), "nothing reached either server");
    dispatch(&call, &both, "list_messages")
        .await
        .expect("the selected server stays permitted");
}

/// A server re-installed under the same name with a binding for
/// another account no longer matches the approval the grant records
/// (Denied); a grant that records no approval is missing evidence
/// (Unavailable).
#[tokio::test]
async fn a_reinstalled_binding_with_another_account_is_denied_and_an_unrecorded_one_unavailable() {
    let host = Host::new().await;
    let other_account = server(
        SERVER,
        &MAIL_OPERATIONS,
        &host.calls,
        &host.issuer,
        OTHER_SUBJECT,
    );
    assert_ne!(
        other_account.connector_approval(),
        host.approval,
        "the binding for another account has another approval"
    );
    let reinstalled = host.router(vec![other_account]).await;
    let call = host.admitted();
    let refused = dispatch(&call, &reinstalled, "list_messages").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );

    let unrecorded = host
        .grants
        .issue_root(
            &operator(),
            id("collector-mail-unrecorded"),
            collector(),
            None,
            restrictions(
                SERVER,
                &MAIL_OPERATIONS,
                None,
                LifetimeRestriction::unrestricted(),
            ),
        )
        .expect("a grant without an approval record");
    let original = association(candidate("setup-unrecorded", SERVER, &unrecorded));
    host.originals.enroll(&original);
    let mail = host.mail_router().await;
    let call = host.prepare(original).expect("admitted");
    let unavailable = dispatch(&call, &mail, "list_messages").await;
    assert!(
        matches!(
            unavailable,
            Err(ToolError::OperationAuthorizationUnavailable)
        ),
        "{unavailable:?}"
    );
    assert!(host.calls().is_empty(), "nothing reached the server");
}

/// A host dispatcher whose plans carry `target` (or none): not an MCP
/// server operation.
struct Targeted {
    target: Option<ToolOperationTarget>,
    ran: AtomicBool,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for Targeted {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            "host_tool",
            "",
            serde_json::json!({"type": "object"}),
        ))]
        .into()
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _: &meerkat_core::ToolDispatchContext,
        resolution: &ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let plan = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?
            .execution
            .resolve_default(resolution.deadlines().clone())?;
        let Some(target) = &self.target else {
            return Ok(plan);
        };
        plan.with_owner_witness(
            meerkat_core::ToolExecutionOwnerWitness::new(
                "host-owner",
                call.name,
                self.execution_binding_fingerprint(call.name)?,
            )?
            .with_operation_target(target.clone()),
        )
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.into(), "ran".into(), false).into())
    }
}

/// A service call admits only MCP server operations: a host tool with
/// a builtin or a comms target, or with no target, is malformed and never
/// runs.
#[tokio::test]
async fn a_builtin_comms_or_untargeted_operation_is_malformed_for_a_service_call() {
    let host = Host::new().await;
    let call = host.admitted();
    for target in [
        Some(ToolOperationTarget::BuiltinOperation {
            operation: meerkat_core::BuiltinToolOperation::WebSearch,
        }),
        Some(ToolOperationTarget::PeerDirectoryRead),
        None,
    ] {
        let dispatcher = Arc::new(Targeted {
            target,
            ran: AtomicBool::new(false),
        });
        let refused = dispatch(&call, &dispatcher, "host_tool").await;
        assert_eq!(
            refusal_kind(&refused),
            Some(OperationRefusalKind::MalformedFacts),
            "{refused:?}"
        );
        assert!(!dispatcher.ran.load(Ordering::SeqCst), "the tool never ran");
    }
}

/// An operation whose owner requires review is refused, not skipped:
/// a service call has no review composition.
#[tokio::test]
async fn an_operation_whose_tier_needs_review_is_refused_not_skipped() {
    let host = Host::with(HostOptions {
        reviewed: ["archive_message".to_owned()].into(),
        ..HostOptions::default()
    })
    .await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    let refused = dispatch(&call, &mail, "archive_message").await;
    assert!(refused.is_err(), "an R2 operation is refused: {refused:?}");
    assert!(host.calls().is_empty(), "nothing reached the server");
    dispatch(&call, &mail, "list_messages")
        .await
        .expect("an R1 operation stays permitted");
}

/// Autoref probe: `is_clone` is the inherent method only for `Clone` types.
struct Probe<T>(std::marker::PhantomData<T>);

trait NotCloneProbe {
    fn is_clone(&self) -> bool {
        false
    }
}

impl<T> NotCloneProbe for Probe<T> {}

impl<T: Clone> Probe<T> {
    #[allow(dead_code)]
    fn is_clone(&self) -> bool {
        true
    }
}

/// The admitted context is Domain-scoped and runless, a request is
/// admitted once, a `ServiceCall` is not `Clone`, and a call after the
/// grant's expiry is refused.
#[tokio::test]
async fn the_context_is_domain_scoped_runless_and_fresh_per_request() {
    assert!(
        !Probe::<ServiceCall>(std::marker::PhantomData).is_clone(),
        "a ServiceCall is not Clone"
    );
    assert!(
        Probe::<String>(std::marker::PhantomData).is_clone(),
        "the probe detects Clone"
    );

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let host = Host::with(HostOptions {
        lifetime: LifetimeRestriction::window(now, now + 4_000),
        ..HostOptions::default()
    })
    .await;
    let mail = host.mail_router().await;
    let request = host.request(host.original.clone());
    let call = host
        .machine
        .prepare_service_call(Arc::clone(&request))
        .expect("admitted once");
    let again = prepare_refusal(host.machine.prepare_service_call(request));
    assert!(
        matches!(again, OperationAuthorizationError::Unavailable),
        "{again:?}"
    );

    dispatch(&call, &mail, "list_messages")
        .await
        .expect("permitted within the grant's window");
    let observations = host.audit(call.audit_key()).await;
    assert!(!observations.is_empty());
    for seen in &observations {
        assert_eq!(seen.execution_scope, OperationExecutionScope::Domain);
        assert!(seen.run_id.is_none(), "a service call has no run");
    }

    tokio::time::sleep(Duration::from_millis(4_500)).await;
    let expired = dispatch(&call, &mail, "list_messages").await;
    assert!(
        expired.is_err(),
        "a call after the grant's expiry is refused"
    );
    assert_eq!(host.calls().len(), 1, "the expired call was not sent");
}

/// Every decision and refusal of an occurrence's calls goes to that
/// occurrence's own rows of the audit log, and none to another
/// occurrence's rows.
#[tokio::test]
async fn every_decision_and_refusal_is_audited_with_its_occurrence() {
    let host = Host::new().await;
    let both = host
        .router(vec![
            host.mail(),
            server(
                SECOND_SERVER,
                &CALENDAR_OPERATIONS,
                &host.calls,
                &host.issuer,
                SUBJECT,
            ),
        ])
        .await;
    let request = host.request(host.original.clone());
    assert_eq!(request.occurrence(), &occurrence("occurrence-1"));
    let call = host
        .machine
        .prepare_service_call(request)
        .expect("admitted");
    let other = host
        .machine
        .prepare_service_call(host.request_for(host.original.clone(), "occurrence-2"))
        .expect("admitted");
    assert_eq!(
        call.audit_key().original_digest(),
        other.audit_key().original_digest(),
        "one service original"
    );
    assert_ne!(call.audit_key(), other.audit_key(), "two occurrences");

    dispatch(&call, &both, "list_messages")
        .await
        .expect("permitted");
    let refused_call = dispatch(&call, &both, "list_events").await;
    assert!(refused_call.is_err());
    let first = host.audit(call.audit_key()).await;
    assert_eq!(prepared(&first), 1, "the permitted call's decision");
    assert_eq!(
        refused(&first, OperationRefusalKind::Denied),
        1,
        "the refused call's decision"
    );
    assert!(
        host.audit(other.audit_key()).await.is_empty(),
        "nothing went to another occurrence's rows"
    );
}

/// The connector owner still checks the exact credential: a slot
/// holding a credential for another account is refused at the call, even
/// with a current grant and enrollment.
#[tokio::test]
async fn a_slot_credential_for_another_account_is_refused_despite_a_current_grant() {
    let host = Host::with(HostOptions {
        signed_in_code: OTHER_CODE,
        ..HostOptions::default()
    })
    .await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    let refused = dispatch(&call, &mail, "list_messages").await;
    assert!(
        refused.is_err(),
        "another account's credential is refused: {refused:?}"
    );
    assert_eq!(
        prepared(&host.audit(call.audit_key()).await),
        1,
        "the governed decision itself allowed the call"
    );
    assert!(host.calls().is_empty(), "nothing reached the server");
}

/// The grant binds its grantee: an enrolled association whose
/// executor is not the grant's grantee is Denied by the grant owner.
#[tokio::test]
async fn an_association_whose_executor_differs_from_the_grantee_is_denied() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let mut intruder = candidate("setup-intruder", SERVER, &host.grant);
    intruder.logical_executor = principal(PrincipalKind::ServiceAccount, "intruder");
    let intruder = association(intruder);
    host.originals.enroll(&intruder);
    let call = host.prepare(intruder).expect("admitted as enrolled");
    let refused = dispatch(&call, &mail, "list_messages").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert!(host.calls().is_empty(), "nothing reached the server");
}

/// The whole association is compared with the enrolled record: the
/// enrolled original with one claim changed (the requester, then the
/// target) is refused, whatever its basis.
#[tokio::test]
async fn the_enrolled_original_with_one_claim_changed_is_refused() {
    let host = Host::new().await;

    let mut requester = candidate("setup-1", SERVER, &host.grant);
    requester.requester = principal(PrincipalKind::Human, "someone-else");
    let refused = prepare_refusal(host.prepare(association(requester)));
    assert!(
        is_refused(&refused, OperationRefusalKind::MalformedFacts),
        "{refused:?}"
    );

    let mut target = candidate("setup-1", SERVER, &host.grant);
    target.target.context = id("another-source");
    let refused = prepare_refusal(host.prepare(association(target)));
    assert!(
        is_refused(&refused, OperationRefusalKind::MalformedFacts),
        "{refused:?}"
    );
    assert!(host.calls().is_empty());
}

/// An admitted (retained) original of the right shape is no
/// enrollment: it cannot prepare a service call.
#[tokio::test]
async fn an_admitted_retained_original_of_the_right_shape_cannot_prepare_a_service_call() {
    let host = Host::new().await;
    let admitted = association(candidate("admitted-1", SERVER, &host.grant));
    host.originals.retain_admitted(&admitted);
    let refused = prepare_refusal(host.prepare(admitted));
    assert!(
        is_refused(&refused, OperationRefusalKind::Denied),
        "{refused:?}"
    );
    assert!(host.calls().is_empty());
}

/// An enrolled service original is never retained work: Core's
/// retained readers refuse it as an operation's original and as a live
/// source original.
#[tokio::test]
async fn an_enrolled_service_original_reads_not_retained_and_its_retained_resume_is_refused() {
    let host = Host::new().await;
    let claims = host.original.candidate();
    let binding = PreparedAuthorizationBinding::new(
        meerkat_core::authorization::OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Source(
                meerkat_core::authorization::SourceAuthorizationFacts {
                    target: meerkat_core::authorization::SourceAuthorizationTarget::External(
                        meerkat_core::authorization::OwnerQualifiedTarget {
                            authority: operator(),
                            namespace: "sources".into(),
                            id: "inbox".into(),
                        },
                    ),
                    usage: meerkat_core::authorization::SourceAuthorizationUse::Read,
                },
            ),
        },
    );
    let grant = original_operation_grant(
        &host.grants,
        host.originals.as_ref(),
        &claims.ingress_namespace,
        &claims.original_work,
        &binding,
        &claims.logical_executor,
        None,
    );
    assert!(
        matches!(
            &grant,
            Err(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "an enrolled original is no retained operation original"
    );
    let live = require_source_original_live(
        &host.grants,
        host.originals.as_ref(),
        &claims.ingress_namespace,
        &claims.original_work,
        &claims.logical_executor,
        None,
    );
    assert!(
        live.is_err(),
        "an enrolled original is no live retained source original"
    );
}

/// A request whose association cannot be a service original is refused
/// as malformed before any host is asked.
#[tokio::test]
async fn a_request_that_cannot_be_a_service_original_is_malformed() {
    let fixture = Host::new().await;
    let base = || candidate("setup-1", SERVER, &fixture.grant);
    let mut cases: Vec<(&str, InputAuthorityAssociationCandidate)> = Vec::new();

    let mut controller = base();
    controller.controller_grant_lineage = vec![fixture.grant.clone()];
    cases.push(("a controller lineage", controller));

    let mut model = base();
    model.controller_model = Some(ControllerModelSelection::new(
        SessionLlmIdentity {
            model: "controller-model".into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        AuthCredentialIdentity::Account(CredentialAccountRef {
            realm: RealmId::parse(REALM).unwrap(),
            account: CredentialAccountId::parse("controller").unwrap(),
        }),
        "controller-profile".into(),
        "controller-transport".into(),
    ));
    cases.push(("a controller model", model));

    let mut subject = base();
    subject.represented_subject = Some(principal(PrincipalKind::Human, "subject"));
    cases.push(("a represented subject", subject));

    let mut contributors = base();
    contributors.contributing_work = vec![OriginalWorkRef {
        authority: host(),
        work: id("another"),
    }];
    cases.push(("contributing work", contributors));

    let mut single = base();
    single.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![fixture.grant.clone()],
    };
    cases.push(("a single lineage for every target", single));

    let mut untargeted = base();
    untargeted.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(vec![fixture.grant.clone()]),
        targets: vec![TargetGrantLineage {
            target: GrantTarget::PeerDirectory,
            lineage: vec![fixture.grant.clone()],
        }],
    };
    cases.push(("no MCP server target", untargeted));

    let mut reserved = base();
    reserved.ingress_namespace.ingress.namespace = "meerkat.consent".into();
    cases.push(("a reserved namespace", reserved));

    for (case, candidate) in cases {
        let refused = ServiceCallRequest::from_trusted_host(
            association(candidate),
            occurrence("occurrence-1"),
        );
        assert!(
            matches!(&refused, Err(refusal) if refusal.kind() == OperationRefusalKind::MalformedFacts),
            "{case}: {refused:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The service-call audit log
// ---------------------------------------------------------------------------

/// The mail router, recording what the audit log held for one occurrence,
/// and how many calls the server had received, when the router's own
/// dispatch began: the first dispatcher code a call runs.
struct LogAtEntry {
    inner: Arc<meerkat_mcp::McpRouterAdapter>,
    store: Arc<InMemoryRuntimeStore>,
    key: ServiceCallAuditKey,
    calls: PathBuf,
    seen: Mutex<Vec<(Vec<&'static str>, usize)>>,
}

#[async_trait::async_trait]
impl AgentToolDispatcher for LogAtEntry {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner.tool_catalog()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        self.inner.tool_mutation_class(tool_name)
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> meerkat_core::LiveBridgeEffectKind {
        self.inner.live_bridge_effect_kind(tool_name)
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        self.inner.review_entry_support(tool_name)
    }

    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        self.inner.execution_binding_epoch(tool_name)
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        self.inner.execution_binding_fingerprint(tool_name)
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        self.inner
            .resolve_execution_plan(call, dispatch_context, resolution_context)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch(call).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.inner.dispatch_with_context(call, context).await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let held: Vec<AuthorizationAuditObservation> = self
            .store
            .read_service_call_audit(&self.key)
            .await
            .expect("the protected read of the audit log")
            .into_iter()
            .map(|record| record.observation.observation)
            .collect();
        self.seen
            .lock()
            .unwrap()
            .push((kinds(&held), calls(&self.calls).len()));
        self.inner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }
}

/// The call's decision and root entry are in the log before any dispatcher
/// code runs: the router's own dispatch reads them there when it begins,
/// before anything reached the server. The rest of the call (its outcome)
/// is appended once it returns.
#[tokio::test]
async fn the_decision_is_durable_before_any_dispatcher_code_runs() {
    let host = Host::new().await;
    let call = host.admitted();
    let router = Arc::new(LogAtEntry {
        inner: host.mail_router().await,
        store: Arc::clone(&host.store),
        key: call.audit_key().clone(),
        calls: host.calls.clone(),
        seen: Mutex::default(),
    });

    dispatch(&call, &router, "list_messages")
        .await
        .expect("the governed call reaches the server");
    assert_eq!(
        *router.seen.lock().unwrap(),
        vec![(vec!["prepared", "entry"], 0)],
        "the decision and entry were durable when the dispatcher began, and nothing was sent"
    );
    let log = kinds(&host.audit(call.audit_key()).await);
    assert_eq!(log[..2], ["prepared", "entry"], "{log:?}");
    assert_eq!(log.last(), Some(&"outcome"), "{log:?}");
    assert_eq!(host.calls().len(), 1);
}

/// A call whose decision cannot be appended to the log is refused before
/// any dispatcher code runs: nothing is sent and nothing is written. Its
/// `ServiceCall` then refuses every later call the same way (its log would
/// have a gap), while a new request for the occurrence is admitted and
/// audited.
#[tokio::test]
async fn a_failed_append_before_the_call_refuses_it_unsent() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    host.store.refuse_service_call_audit_appends_for_test(
        1,
        RuntimeStoreError::WriteFailed("the audit log is full".into()),
    );

    let refused = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        matches!(
            refused.result,
            Err(ToolError::OperationObservationUnavailable)
        ),
        "{:?}",
        refused.result
    );
    assert!(
        matches!(
            refused.audit,
            Err(ServiceCallAuditError::Store(
                RuntimeStoreError::WriteFailed(_)
            ))
        ),
        "{:?}",
        refused.audit
    );
    assert!(host.calls().is_empty(), "nothing was sent");
    assert!(
        host.audit(call.audit_key()).await.is_empty(),
        "nothing was written"
    );

    let spent = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        matches!(
            spent.result,
            Err(ToolError::OperationObservationUnavailable)
        ),
        "{:?}",
        spent.result
    );
    assert!(
        matches!(spent.audit, Err(ServiceCallAuditError::Spent)),
        "{:?}",
        spent.audit
    );
    assert!(host.calls().is_empty(), "nothing was sent");

    let fresh = host.admitted();
    dispatch(&fresh, &mail, "list_messages")
        .await
        .expect("a new request's call is sent");
    assert_eq!(host.calls().len(), 1);
    assert_eq!(
        prepared(&host.audit(fresh.audit_key()).await),
        1,
        "only the sent call's decision is in the log"
    );
}

/// A store without the service-call audit log answers `Unsupported` (the
/// trait's default answer), and the call is refused before it is sent.
#[tokio::test]
async fn a_store_without_the_audit_log_refuses_the_call_unsent() {
    let host = Host::new().await;
    host.store.refuse_service_call_audit_appends_for_test(
        usize::MAX,
        RuntimeStoreError::Unsupported("append_service_call_audit".into()),
    );
    let mail = host.mail_router().await;
    let call = host.admitted();

    let refused = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        matches!(
            refused.result,
            Err(ToolError::OperationObservationUnavailable)
        ),
        "{:?}",
        refused.result
    );
    assert!(
        matches!(
            refused.audit,
            Err(ServiceCallAuditError::Store(
                RuntimeStoreError::Unsupported(_)
            ))
        ),
        "{:?}",
        refused.audit
    );
    assert!(host.calls().is_empty(), "nothing was sent");
}

/// A call whose outcome cannot be appended once it returned keeps its own
/// result and reports the failure beside it. The log keeps what was
/// appended before the call was sent, and the `ServiceCall` refuses every
/// later call before it is sent.
#[tokio::test]
async fn a_failed_append_after_the_call_is_reported_beside_its_result() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    let armed = AtomicBool::new(false);
    let store = Arc::downgrade(&host.store);
    host.store
        .observe_service_call_audit_appends_for_test(Arc::new(move |_: &ServiceCallAuditBatch| {
            // The first append (the decision) goes through and arms a
            // refusal of the next one (the outcome).
            if !armed.swap(true, Ordering::SeqCst)
                && let Some(store) = store.upgrade()
            {
                store.refuse_service_call_audit_appends_for_test(
                    1,
                    RuntimeStoreError::WriteFailed("the audit log is full".into()),
                );
            }
        }));

    let outcome = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        outcome.result.is_ok(),
        "the call's own result: {:?}",
        outcome.result
    );
    assert!(
        matches!(
            outcome.audit,
            Err(ServiceCallAuditError::Store(
                RuntimeStoreError::WriteFailed(_)
            ))
        ),
        "{:?}",
        outcome.audit
    );
    assert_eq!(host.calls().len(), 1, "the call was sent once");
    assert_eq!(
        kinds(&host.audit(call.audit_key()).await),
        ["prepared", "entry"],
        "what was appended before the call was sent"
    );

    let spent = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        matches!(
            spent.result,
            Err(ToolError::OperationObservationUnavailable)
        ),
        "{:?}",
        spent.result
    );
    assert!(
        matches!(spent.audit, Err(ServiceCallAuditError::Spent)),
        "{:?}",
        spent.audit
    );
    assert_eq!(host.calls().len(), 1, "nothing more was sent");
}

/// A call dropped while its decision's append is in the store spends its
/// `ServiceCall`: whether that append lands is unknown, so every later call
/// is refused before it is sent, and nothing reaches the server.
#[tokio::test]
async fn a_call_dropped_during_its_append_spends_its_service_call() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    host.store.block_next_service_call_audit_append_for_test(
        call.audit_key().clone(),
        Arc::clone(&entered),
        Arc::clone(&release),
    );
    {
        let dropped = dispatch_outcome(&call, &mail, "list_messages");
        tokio::pin!(dropped);
        tokio::select! {
            () = entered.notified() => {}
            outcome = &mut dropped => {
                panic!("the call settled while its append was blocked: {:?}", outcome.result)
            }
        }
        // Dropped here, while its append waits in the store.
    }
    // Nothing waits on the block any more.
    release.notify_one();

    let next = dispatch_outcome(&call, &mail, "list_messages").await;
    assert!(
        matches!(next.result, Err(ToolError::OperationObservationUnavailable)),
        "{:?}",
        next.result
    );
    assert!(
        matches!(next.audit, Err(ServiceCallAuditError::Spent)),
        "{:?}",
        next.audit
    );
    assert!(host.calls().is_empty(), "nothing was sent");
    assert!(
        host.audit(call.audit_key()).await.is_empty(),
        "the dropped append wrote nothing"
    );
}

/// The protected read gives back every observation of an occurrence in the
/// order it was appended, each with its store-assigned sequence: a sent
/// call's decision, entry and outcome, then a refused call's refusal. Each
/// is Domain-scoped and runless and names no contributing input. The key
/// names the service original's identity digest and the occurrence.
#[tokio::test]
async fn the_log_reads_back_every_observation_in_order_under_its_key() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let call = host.admitted();
    dispatch(&call, &mail, "list_messages")
        .await
        .expect("permitted");
    host.revoke_grant(&host.grant);
    let refused = dispatch(&call, &mail, "get_message").await;
    assert_eq!(
        refusal_kind(&refused),
        Some(OperationRefusalKind::Denied),
        "{refused:?}"
    );

    let records = host
        .store
        .read_service_call_audit(call.audit_key())
        .await
        .expect("the protected read of the audit log");
    let sequences: Vec<u64> = records.iter().map(|record| record.sequence).collect();
    assert_eq!(sequences, (0..records.len() as u64).collect::<Vec<_>>());
    let log: Vec<AuthorizationAuditObservation> = records
        .iter()
        .map(|record| record.observation.observation.clone())
        .collect();
    let order = kinds(&log);
    assert_eq!(order[..2], ["prepared", "entry"], "{order:?}");
    assert_eq!(
        order[order.len() - 2..],
        ["outcome", "refused"],
        "{order:?}"
    );
    let sent = &log[0].operation_id;
    assert!(
        log[..log.len() - 1]
            .iter()
            .all(|seen| &seen.operation_id == sent),
        "the sent call's observations are one operation's"
    );
    assert_ne!(
        &log[log.len() - 1].operation_id,
        sent,
        "the refused call is another operation"
    );
    for record in &records {
        assert!(record.observation.contributors.is_empty());
        assert_eq!(
            record.observation.observation.execution_scope,
            OperationExecutionScope::Domain
        );
        assert!(record.observation.observation.run_id.is_none());
    }

    let key = call.audit_key();
    assert_eq!(key.occurrence(), "occurrence-1");
    assert_eq!(key.original_digest().len(), 64);
    assert!(
        key.original_digest()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        host.request(host.original.clone()).audit_key(),
        key,
        "the key is the original's and the occurrence's, whatever the request"
    );
    let another = association(candidate("setup-other", SERVER, &host.grant));
    assert_ne!(
        host.request(another).audit_key().original_digest(),
        key.original_digest(),
        "another original has another digest"
    );
}

/// An append that waits in the store holds nothing another occurrence's
/// call needs: while one occurrence's call waits for its decision to be
/// appended, another occurrence's call of the same original is admitted,
/// appended and sent through the same router, and the waiting call is sent
/// once its append completes.
#[tokio::test]
async fn a_slow_append_of_one_occurrence_blocks_no_other_occurrence() {
    let host = Host::new().await;
    let mail = host.mail_router().await;
    let slow = host.admitted();
    let other = host
        .machine
        .prepare_service_call(host.request_for(host.original.clone(), "occurrence-2"))
        .expect("admitted");
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    host.store.block_next_service_call_audit_append_for_test(
        slow.audit_key().clone(),
        Arc::clone(&entered),
        Arc::clone(&release),
    );

    let waiting = dispatch_outcome(&slow, &mail, "list_messages");
    let others = async {
        entered.notified().await;
        assert!(host.calls().is_empty(), "the waiting call was not sent");
        let outcome = dispatch_outcome(&other, &mail, "get_message").await;
        let sent = host.calls().len();
        release.notify_one();
        (outcome, sent)
    };
    let (waiting, (other_outcome, sent_while_waiting)) = tokio::join!(waiting, others);
    assert!(other_outcome.result.is_ok(), "{:?}", other_outcome.result);
    assert!(other_outcome.audit.is_ok(), "{:?}", other_outcome.audit);
    assert_eq!(
        sent_while_waiting, 1,
        "the other occurrence's call was sent while the first waited"
    );
    assert!(waiting.result.is_ok(), "{:?}", waiting.result);
    assert!(waiting.audit.is_ok(), "{:?}", waiting.audit);
    assert_eq!(host.calls().len(), 2);
    assert_eq!(prepared(&host.audit(slow.audit_key()).await), 1);
    assert_eq!(prepared(&host.audit(other.audit_key()).await), 1);
}

/// A governed machine without a store has nowhere to keep a call's record,
/// so it admits no service call.
#[tokio::test]
async fn a_storeless_governed_machine_admits_no_service_call() {
    let host = Host::new().await;
    let storeless = MeerkatMachine::ephemeral()
        .with_local_grant_authorization(composition(&host.grants, &host.originals, BTreeSet::new()))
        .expect("a governed machine");
    let refused =
        prepare_refusal(storeless.prepare_service_call(host.request(host.original.clone())));
    assert!(
        matches!(refused, OperationAuthorizationError::Unavailable),
        "{refused:?}"
    );
    assert!(host.calls().is_empty());
}
