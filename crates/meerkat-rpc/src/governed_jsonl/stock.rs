//! The stock host's governed JSONL composition.
//!
//! A stock-governed realm serves governed JSONL with the same composition as
//! a stock CLI prompt: the connection authenticates the local operator
//! through the stock ingress, each created session is bound to its canonical
//! stock destination, and each input is bound to an authenticated original
//! recorded in the realm's ledger. The connection's own association path is
//! never used: an input the stock host does not bind is refused.
//!
//! As a stock CLI session does, each created session is also bound to the
//! agent the realm was opened for ([`StockGovernance::bind_session_agent`]),
//! at create and again right before its promotion builds the agent. The
//! agent's mandate names every server its selection names, each with its own
//! lineage, so a selected server the realm does not configure can still be
//! added live. The realm's configured MCP servers keep their full
//! descriptors and are carried into each session's build, where every one
//! connects through the governed configured entry; a server with no
//! selected grant is refused at its connection while the session continues.
//!
//! A connector-bound configured server needs the host's connector authority
//! ([`stock_governed_jsonl_setup_with_connector_authority`]): each session's
//! build then supplies the connector call context of each call's own staged
//! binding. Composed without one, the binder refuses such a server locally
//! before any connection effect: it is never carried into a build, and a
//! live `mcp/add` (or restore) of a connector-bound descriptor is refused on
//! that descriptor before any owner decision, even when the agent's
//! selection names it; its descriptor is kept unchanged (the composition by design without a
//! connector authority, as in tests or a host without credentials).
//!
//! Not claimed yet: the authority is composed without the realm's connector
//! client profiles, so an already authorized credential serves its calls,
//! but a refresh that needs a client profile fails locally before sending.
// TODO(r3): add `.with_connector_client_source(RealmConnectorClients::
// from_effective_config(..))` to the host's `HostAuthService` (rkat-rpc
// main.rs) once the connector client profiles chain is in the base.

use std::sync::Arc;

use meerkat::PersistenceBundle;
use meerkat::governed_realm::stock::StockGovernance;
use meerkat::governed_realm::stock::originals::PendingOriginal;
use meerkat_authorization_contracts::work_association::NativeWorkTarget;
use meerkat_core::{Config, ControllerModelClient, McpServerConfig, SessionId, ToolDef};
use meerkat_llm_core::LlmClient;
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::{MeerkatMachine, RuntimeDriverError, input::Input};

use super::stock_consent::StdioConsentSurface;
use super::{
    GovernedBoundInput, GovernedConnection, GovernedCustodyFault, GovernedInputCustody,
    GovernedJsonlSetup, GovernedSessionBinder,
};

/// How long one review attempt may take. It holds the dispatch and the tool
/// batch, so it stays below the tool timeout.
const REVIEW_DEADLINE: std::time::Duration = std::time::Duration::from_mins(2);

/// How long one consent request stays decidable on the pipe.
const CONSENT_VALIDITY: std::time::Duration = std::time::Duration::from_mins(15);

/// Why the reviewed stock setup could not be composed; the realm is not
/// served.
#[derive(Debug, thiserror::Error)]
pub enum StockReviewedSetupError {
    #[error(transparent)]
    Review(#[from] meerkat::governed_realm::stock::review::StockReviewError),
    #[error("the pipe's operator cannot request approvals: {0}")]
    Approval(#[from] meerkat_core::ApprovalError),
    #[error(transparent)]
    Runtime(#[from] RuntimeDriverError),
}

fn refused(error: impl std::fmt::Display) -> RuntimeDriverError {
    RuntimeDriverError::ValidationFailed {
        reason: format!("the stock governed realm refused this: {error}"),
    }
}

/// The stock host's binder of the sessions and inputs of one connection.
pub struct StockJsonlBinder {
    stock: Arc<StockGovernance>,
    /// The realm's configured MCP servers this composition supports, as full
    /// descriptors.
    servers: Vec<McpServerConfig>,
    /// The connector-bound configured servers, refused before connection
    /// because the composition has no connector authority, as full
    /// descriptors.
    unsupported_connector_servers: Vec<McpServerConfig>,
    /// Whether the composition carries the host's connector authority, so
    /// it can execute a connector-bound descriptor.
    connector_supported: bool,
}

impl StockJsonlBinder {
    /// The binder of a composition without a connector authority: its
    /// connector-bound configured servers are refused before connection.
    pub fn new(stock: Arc<StockGovernance>, servers: Vec<McpServerConfig>) -> Arc<Self> {
        let (unsupported_connector_servers, servers): (Vec<_>, Vec<_>) = servers
            .into_iter()
            .partition(|server| server.connector.is_some());
        for server in &unsupported_connector_servers {
            tracing::warn!(
                server = %server.name,
                "connector-bound configured MCP server refused before connection: \
                 the stock governed JSONL composition has no connector authority"
            );
        }
        Arc::new(Self {
            stock,
            servers,
            unsupported_connector_servers,
            connector_supported: false,
        })
    }

    /// The binder of a composition whose sessions carry the host's connector
    /// authority: every configured server, connector-bound ones included, is
    /// carried into each session's build.
    pub fn with_connector_authority(
        stock: Arc<StockGovernance>,
        servers: Vec<McpServerConfig>,
    ) -> Arc<Self> {
        Arc::new(Self {
            stock,
            servers,
            unsupported_connector_servers: Vec::new(),
            connector_supported: true,
        })
    }

    /// The connector-bound configured servers this composition refuses
    /// before connection.
    pub fn unsupported_connector_servers(&self) -> &[McpServerConfig] {
        &self.unsupported_connector_servers
    }
}

/// The configured names [`StockGovernance::bind_session_agent`] binds an
/// older selection's single worker grant for (every explicitly selected
/// server is bound on its own). Only that selection uses them: never an
/// attachment identity or an authorization.
fn agent_selection_names(servers: &[McpServerConfig]) -> impl Iterator<Item = &str> {
    servers.iter().map(|server| server.name.as_str())
}

#[async_trait::async_trait]
impl GovernedSessionBinder for StockJsonlBinder {
    async fn bind_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
    ) -> Result<NativeWorkTarget, RuntimeDriverError> {
        let target = self
            .stock
            .bind_jsonl_session(machine, session_id)
            .await
            .map_err(refused)?;
        // After the destination, before any input or MCP entry: the agent
        // the session acts as, as a stock CLI session binds it.
        self.stock
            .bind_session_agent(machine, session_id, agent_selection_names(&self.servers))
            .await
            .map_err(refused)?;
        Ok(target)
    }

    /// The stock destination (at its next generation) and the agent, again
    /// on the session's current registration, as at create.
    async fn bind_promoting_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
    ) -> Result<Option<NativeWorkTarget>, RuntimeDriverError> {
        self.bind_session(machine, session_id).await.map(Some)
    }

    /// Without the connector authority a connector-bound descriptor cannot
    /// be executed: refused locally before any effect, live add included,
    /// even though the agent's selection may name it.
    fn supports_attachment_descriptor(&self, server: &McpServerConfig) -> bool {
        self.connector_supported || server.connector.is_none()
    }

    fn configured_mcp_servers(&self) -> Vec<McpServerConfig> {
        self.servers.clone()
    }

    /// The stock realm's own installations, as its installation authority
    /// read them when the realm opened (the live-attach installations the
    /// stock policy authorizes against).
    fn realm_mcp_installations(&self) -> Option<Vec<McpServerConfig>> {
        Some(self.stock.installed_mcp_servers())
    }

    async fn bind_input(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input: Input,
        pin: ControllerModelClient,
    ) -> Result<Option<GovernedBoundInput>, RuntimeDriverError> {
        let bound = self
            .stock
            .bind_jsonl_input(machine, session_id, input, pin)
            .await
            .map_err(refused)?;
        Ok(Some(GovernedBoundInput {
            input: bound.input,
            target: bound.target,
            custody: Box::new(StockOriginalCustody(bound.original)),
        }))
    }

    fn allows_empty_callback_tools(&self) -> bool {
        true
    }
}

/// The candidate original of one bound input.
struct StockOriginalCustody(PendingOriginal);

impl GovernedInputCustody for StockOriginalCustody {
    fn admit(self: Box<Self>) -> Result<(), GovernedCustodyFault> {
        self.0
            .admit()
            .map_err(|error| GovernedCustodyFault(error.to_string()))
    }

    fn retain(self: Box<Self>) {
        // The ledger keeps the candidate: dropping it would withdraw an
        // original the native owner may have admitted.
        std::mem::forget(self.0);
    }
}

/// The governed JSONL setup of a stock-governed realm opened with `stock`:
/// the connection carries the operator facts the stock host authenticated,
/// and every session and input is bound through the stock composition.
///
/// `config`'s configured MCP servers (the realm's installations among them)
/// move, as full descriptors, from the fixed profile's configuration to the
/// stock composition, which carries the supported configured servers into
/// each session's build (connector-bound ones are refused, see the module
/// documentation).
pub fn stock_governed_jsonl_setup(
    stock: &Arc<StockGovernance>,
    config: Config,
    persistence: PersistenceBundle,
    client: Arc<dyn LlmClient>,
    tools: Vec<ToolDef>,
) -> Result<GovernedJsonlSetup, RuntimeDriverError> {
    stock_governed_jsonl_setup_with_connector_authority(
        stock,
        config,
        persistence,
        client,
        tools,
        None,
    )
}

/// [`stock_governed_jsonl_setup_with_connector_authority`] with the realm
/// owner's operation review installed, deciding through the persistence's
/// one approval issuer: the configured reviewer for R2, built through
/// `reviewer_factory` exactly as the realm selected it, and for R3 this
/// stdio pipe's operator, device trusted in the pipe's own scope (a rule
/// that requires person authentication refuses it). Without review rules
/// nothing is composed and no reviewer is built.
///
/// # Errors
/// An invalid review section, a reviewer the factory cannot build as
/// selected, or an operator that cannot be named as an approval requester.
pub async fn stock_governed_jsonl_setup_reviewed(
    stock: &Arc<StockGovernance>,
    config: Config,
    persistence: PersistenceBundle,
    client: Arc<dyn LlmClient>,
    tools: Vec<ToolDef>,
    connector_authority: Option<meerkat::ConnectorOAuthAuthority>,
    reviewer_factory: &meerkat::AgentFactory,
) -> Result<GovernedJsonlSetup, StockReviewedSetupError> {
    let consent = StdioConsentSurface::new(stock, persistence.approval_issuer())?;
    let review = meerkat::governed_realm::stock::review::stock_operation_review(
        reviewer_factory,
        &config,
        persistence.approval_issuer(),
        REVIEW_DEADLINE,
        Some(meerkat::governed_realm::stock::review::StockConsent::new(
            stock,
            Arc::clone(&consent) as _,
            CONSENT_VALIDITY,
        )),
    )
    .await?;
    let mut setup = stock_governed_jsonl_setup_with_connector_authority(
        stock,
        config,
        persistence,
        client,
        tools,
        connector_authority,
    )?;
    setup.consent = review.is_some().then_some(consent);
    setup.operation_review = review;
    Ok(setup)
}

/// [`stock_governed_jsonl_setup`] whose sessions carry the host's
/// `connector_authority` (the owner the CLI's realm path uses), so that a
/// connector-bound configured server connects and each of its calls is
/// prepared for its own staged binding. `None` is
/// [`stock_governed_jsonl_setup`].
pub fn stock_governed_jsonl_setup_with_connector_authority(
    stock: &Arc<StockGovernance>,
    mut config: Config,
    persistence: PersistenceBundle,
    client: Arc<dyn LlmClient>,
    tools: Vec<ToolDef>,
    connector_authority: Option<meerkat::ConnectorOAuthAuthority>,
) -> Result<GovernedJsonlSetup, RuntimeDriverError> {
    let facts = stock.jsonl_connection_facts().map_err(refused)?;
    let servers = std::mem::take(&mut config.tools.mcp_servers);
    let connection = GovernedConnection::new(
        facts.requester,
        None,
        facts.ingress_actor,
        facts.realm,
        facts.authentication,
        // The stock binder binds every input; this path is never taken.
        Arc::new(|_, _, _| {
            Err(NativeAdmissionError::Refused(
                meerkat_core::OperationRefused::new(
                    meerkat_core::OperationRefusalKind::MalformedFacts,
                ),
            ))
        }),
    )?;
    Ok(GovernedJsonlSetup {
        config,
        persistence,
        client,
        connection,
        tools,
        mcp_attachment_identity_key_custody: None,
        session_binder: Some(match connector_authority {
            Some(_) => StockJsonlBinder::with_connector_authority(Arc::clone(stock), servers),
            None => StockJsonlBinder::new(Arc::clone(stock), servers),
        }),
        connector_authority,
        operation_review: None,
        consent: None,
        host_confinement: meerkat::HostConfinement::trusted_host(),
    })
}
