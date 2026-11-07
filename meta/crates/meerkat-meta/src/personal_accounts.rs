//! Personal accounts on MCP servers, in members private to their owner.
//!
//! The operator marks an MCP server as using personal accounts
//! ([`McpAccountMode::Personal`]); every other server keeps one account per
//! server and realm, as before ([`McpAccountMode::System`], the default).
//! Server definitions stay where they are (a profile's `mcp_servers`): the
//! mode is keyed by the server's name.
//!
//! A personal account belongs to the person who connected it and is used
//! only where its results reach that one person: in a member private to its
//! owner, the console side chat a signed-in person opened
//! ([`crate::access::PRIVATE_TO_LABEL`]). So a personal server
//!
//! - is withheld from every other member's build: nothing reaches the server
//!   from a shared conversation and no other account is tried
//!   ([`PersonalAccountRefusal::SharedThread`]). The helpers and `fork_off`
//!   children of a private member, and an approval branch
//!   ([`APPROVAL_BRANCH_LABEL`]), are private to no person here;
//! - is served, in a member private to P, only for a run whose direct
//!   requester is P ([`PersonalAccountRefusal::NotOwner`] otherwise). A
//!   requester carried across a peer hop never selects an account, and a run
//!   without a requester acts for nobody;
//! - is refused for an owner whose sessions an access admin revoked after
//!   the private member was opened ([`PersonalAccountRefusal::Suspended`]).
//!   A private member opened after a fresh sign-in uses the accounts again;
//!   the credentials are kept;
//! - reads its credential only from its owner's own credential realm, below
//!   the installation's realm ([`PersonalAccountCredentials`]), through the
//!   member's own credential source, which every build of the member
//!   carries; it never falls back to a system account: a member factory's
//!   credential source holds nothing for a personal server
//!   ([`PersonalMcpAccounts::member_credential_source`]).
//!
//! Only a streamable HTTP server whose account the person proves at sign-in
//! (`oauth_account_selection = "discover"`) can be personal; a stdio or
//! connector-bound server, a fixed account or a static `Authorization`
//! header is [`PersonalAccountRefusal::Unsupported`]. So is a member placed
//! on another host whose profile declares a personal server: that host would
//! serve the server with its own credentials, so the spawn and the respawn of
//! such a member are refused, and so is sending a member placed before the
//! server was marked personal to its host again (`placement`).
//!
//! The owner check runs in an MCP adapter composition: a dispatcher over each
//! member's MCP adapter, inside the agent's grant gate (which authorizes
//! first) and before the router's per-call preparation. It refuses with a
//! typed policy denial whose code is the refusal's kind. Each member factory
//! has an owner check of its own ([`PersonalAccountOwnerCheck`]): a member
//! keeps a personal server only in a build whose factory composes it, and
//! the owner's credential serves only a build that check composed.

pub(crate) mod build;
pub(crate) mod gate;
pub(crate) mod owner_check;
pub(crate) mod placement;
pub(crate) mod resolver;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::SystemTime;

use meerkat_core::McpServerConfig;
use meerkat_core::mcp_config::{McpHttpTransport, McpOAuthAccountSelection, McpTransportConfig};
use serde::{Deserialize, Serialize};

pub use owner_check::PersonalAccountOwnerCheck;
pub use resolver::PersonalAccountCredentials;

/// The label of an approval branch (the grants design's one-time fork that
/// runs approved work): it belongs to no person, so it gets no personal
/// server, whatever else it carries.
pub const APPROVAL_BRANCH_LABEL: &str = "decision.approval_branch";

/// Whose account an MCP server's operations act with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpAccountMode {
    /// One account per server and realm, set up by the operator.
    #[default]
    System,
    /// Each signed-in person connects their own account, which an agent uses
    /// only in that person's private conversations.
    Personal,
}

/// The account mode of one MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerAccounts {
    pub accounts: McpAccountMode,
}

/// The account mode of each MCP server, by server name. A server it does not
/// name is [`McpAccountMode::System`].
///
/// TOML (`meta_gateway`'s `config/mcp_accounts.toml`):
///
/// ```toml
/// [servers.notes]
/// accounts = "personal"
/// ```
///
/// JSON (`rpc_gateway`'s `runtime_options.mcp_accounts`):
/// `{"servers": {"notes": {"accounts": "personal"}}}`. Any other key or mode
/// is refused, and so is a server name that is not a plain server name (1 to
/// 64 ASCII letters, digits, `-`, `_` or `.`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpAccountsConfig {
    #[serde(default)]
    pub servers: BTreeMap<String, McpServerAccounts>,
}

/// Why an account mode configuration was refused.
#[derive(Debug)]
pub enum McpAccountsConfigError {
    /// The configuration could not be read.
    Io(String),
    /// The configuration is not a valid account mode configuration.
    Invalid(String),
}

impl std::fmt::Display for McpAccountsConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(reason) => write!(f, "the MCP account modes could not be read: {reason}"),
            Self::Invalid(reason) => write!(f, "the MCP account modes are invalid: {reason}"),
        }
    }
}

impl std::error::Error for McpAccountsConfigError {}

/// Whether `name` is a plain server name: 1 to 64 ASCII letters, digits,
/// `-`, `_` or `.`.
fn plain_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl McpAccountsConfig {
    /// Parse a TOML configuration (see the type docs).
    pub fn from_toml_str(text: &str) -> Result<Self, McpAccountsConfigError> {
        let config: Self = toml::from_str(text)
            .map_err(|error| McpAccountsConfigError::Invalid(error.message().to_string()))?;
        config.validated()
    }

    /// Parse a JSON configuration (see the type docs).
    pub fn from_json_value(value: serde_json::Value) -> Result<Self, McpAccountsConfigError> {
        let config: Self = serde_json::from_value(value)
            .map_err(|error| McpAccountsConfigError::Invalid(error.to_string()))?;
        config.validated()
    }

    /// Read and parse the TOML configuration at `path`.
    pub fn load(path: &Path) -> Result<Self, McpAccountsConfigError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| McpAccountsConfigError::Io(format!("{}: {error}", path.display())))?;
        Self::from_toml_str(&text)
    }

    fn validated(self) -> Result<Self, McpAccountsConfigError> {
        match self.servers.keys().find(|name| !plain_server_name(name)) {
            Some(name) => Err(McpAccountsConfigError::Invalid(format!(
                "{name:?} is not a server name (1 to 64 ASCII letters, digits, '-', '_' or '.')"
            ))),
            None => Ok(self),
        }
    }

    /// The account mode of the server named `server`.
    pub fn mode(&self, server: &str) -> McpAccountMode {
        self.servers
            .get(server)
            .map_or(McpAccountMode::System, |server| server.accounts)
    }

    /// Whether the server named `server` uses personal accounts.
    pub fn is_personal(&self, server: &str) -> bool {
        self.mode(server) == McpAccountMode::Personal
    }

    /// Whether any server uses personal accounts.
    pub fn has_personal_servers(&self) -> bool {
        self.personal_servers().next().is_some()
    }

    /// The servers that use personal accounts, by name.
    pub fn personal_servers(&self) -> impl Iterator<Item = &str> {
        self.servers
            .iter()
            .filter(|(_, server)| server.accounts == McpAccountMode::Personal)
            .map(|(name, _)| name.as_str())
    }

    /// Refuse a personal server among `definitions` that cannot use personal
    /// accounts ([`personal_server_shape`]), naming the first one.
    pub fn check_definitions<'a>(
        &self,
        definitions: impl IntoIterator<Item = &'a McpServerConfig>,
    ) -> Result<(), PersonalAccountRefusal> {
        definitions
            .into_iter()
            .filter(|definition| self.is_personal(&definition.name))
            .find_map(unsupported_refusal)
            .map_or(Ok(()), Err)
    }

    /// [`Self::check_definitions`] over the MCP servers the inline profiles
    /// of `definition` declare: the definitions a launch reads at startup. A
    /// server defined elsewhere (a realm profile, a host's document binding)
    /// is checked at each member build, which withholds it.
    pub fn check_mob_definition(
        &self,
        definition: &meerkat_mob::MobDefinition,
    ) -> Result<(), PersonalAccountRefusal> {
        self.check_definitions(
            definition
                .profiles
                .values()
                .filter_map(meerkat_mob::ProfileBinding::as_inline)
                .flat_map(|profile| profile.tools.mcp_servers.iter()),
        )
    }
}

/// A typed refusal of a personal account. Its [`kind`](Self::kind) is the
/// policy denial code a refused tool call carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PersonalAccountRefusal {
    /// The member is private to no person (a shared agent, a side chat on a
    /// console without sign-in, a private member's helper, an approval
    /// branch).
    SharedThread { server: String },
    /// The run acts for someone other than the member's owner: another
    /// person (an access admin included), a requester carried across a peer
    /// hop, or no requester at all.
    NotOwner { server: String },
    /// The owner has not connected an account on the server.
    NotConnected { server: String },
    /// An access admin revoked the owner's sessions after this private
    /// member was opened.
    Suspended { server: String },
    /// The server cannot use personal accounts.
    Unsupported {
        server: String,
        reason: UnsupportedPersonalServer,
    },
}

/// Why a server cannot use personal accounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedPersonalServer {
    /// A stdio server: its tokens are launch secrets, which stay system level.
    Stdio,
    /// A connector-bound server: native connector OAuth stays system level.
    ConnectorBound,
    /// An HTTP server that is not a streamable HTTP server.
    NotStreamableHttp,
    /// The definition names one fixed account (`oauth_account`).
    FixedAccount,
    /// The definition sends a static `Authorization` header.
    StaticAuthorization,
    /// The definition does not bind the account the person proves at sign-in
    /// (`oauth_account_selection = "discover"`).
    NoAccountProof,
    /// The member would be placed on another host, which serves the
    /// server's credentials on its own side.
    HostedElsewhere,
}

impl UnsupportedPersonalServer {
    fn describe(self) -> &'static str {
        match self {
            Self::Stdio => "it is a stdio server, whose tokens stay system level",
            Self::ConnectorBound => "it is bound to a connector, whose accounts stay system level",
            Self::NotStreamableHttp => "it is not a streamable HTTP server",
            Self::FixedAccount => "its definition names one fixed account",
            Self::StaticAuthorization => "its definition sends a static Authorization header",
            Self::NoAccountProof => {
                "its definition does not bind the account the person proves at sign-in \
                 (oauth_account_selection = \"discover\")"
            }
            Self::HostedElsewhere => {
                "the member would run on another host, whose own credentials would serve it"
            }
        }
    }
}

impl PersonalAccountRefusal {
    /// The refusal's typed kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SharedThread { .. } => "personal_account_shared_thread",
            Self::NotOwner { .. } => "personal_account_not_owner",
            Self::NotConnected { .. } => "personal_account_not_connected",
            Self::Suspended { .. } => "personal_account_suspended",
            Self::Unsupported { .. } => "personal_accounts_unsupported",
        }
    }

    /// The server the refusal names.
    pub fn server(&self) -> &str {
        match self {
            Self::SharedThread { server }
            | Self::NotOwner { server }
            | Self::NotConnected { server }
            | Self::Suspended { server }
            | Self::Unsupported { server, .. } => server,
        }
    }

    /// A message for the model and the person, naming no one.
    pub fn message(&self) -> String {
        match self {
            Self::SharedThread { server } => format!(
                "'{server}' uses personal accounts, which only a private conversation of their \
                 owner can use; this conversation is shared"
            ),
            Self::NotOwner { server } => format!(
                "'{server}' uses the personal account of this conversation's owner, and this run \
                 does not act for them"
            ),
            Self::NotConnected { server } => format!(
                "'{server}' uses personal accounts, and this conversation's owner has not \
                 connected one: connect it under Your accounts"
            ),
            Self::Suspended { server } => format!(
                "the personal account on '{server}' is suspended for this conversation: its \
                 owner's sessions were revoked after it was opened"
            ),
            Self::Unsupported { server, reason } => format!(
                "'{server}' cannot use personal accounts: {}",
                reason.describe()
            ),
        }
    }

    /// The tool error a refused call ends with: a policy denial whose code
    /// is [`Self::kind`].
    pub fn to_tool_error(&self) -> meerkat_core::ToolError {
        meerkat_core::ToolError::policy_denied(meerkat_core::ToolConsequenceDenial::new(
            self.kind(),
            self.message(),
        ))
    }
}

impl std::fmt::Display for PersonalAccountRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind(), self.message())
    }
}

impl std::error::Error for PersonalAccountRefusal {}

/// Whether `config` can use personal accounts: a streamable HTTP server whose
/// account the person proves at sign-in (`oauth_account_selection =
/// "discover"`), with no connector binding, no fixed account and no static
/// `Authorization` header.
pub fn personal_server_shape(config: &McpServerConfig) -> Result<(), UnsupportedPersonalServer> {
    if config.connector.is_some() {
        return Err(UnsupportedPersonalServer::ConnectorBound);
    }
    let McpTransportConfig::Http(http) = &config.transport else {
        return Err(UnsupportedPersonalServer::Stdio);
    };
    if http.transport.unwrap_or_default() != McpHttpTransport::StreamableHttp {
        return Err(UnsupportedPersonalServer::NotStreamableHttp);
    }
    if http.oauth_account.is_some() {
        return Err(UnsupportedPersonalServer::FixedAccount);
    }
    if http
        .headers
        .keys()
        .any(|name| name.eq_ignore_ascii_case("authorization"))
    {
        return Err(UnsupportedPersonalServer::StaticAuthorization);
    }
    if http.oauth_account_selection != Some(McpOAuthAccountSelection::Discover) {
        return Err(UnsupportedPersonalServer::NoAccountProof);
    }
    Ok(())
}

/// The private-owner key of the person a verified OIDC `issuer` and `subject`
/// name: the value of [`crate::access::PRIVATE_TO_LABEL`] on the members
/// private to them, and the `caller_scope` `meta/console/whoami` reports to
/// them. A host that declares a per-person agent sets the label to this.
pub fn private_owner_key(issuer: &str, subject: &str) -> String {
    crate::http_console::person_private_owner_key(issuer, subject)
}

/// The private-owner key of a qualified principal: a human in the trust
/// domain the console qualifies its people with
/// ([`crate::runtime::CONSOLE_HUMAN_TRUST_DOMAIN_PREFIX`] and the issuer).
/// Every other principal owns nothing.
pub fn private_owner_key_of_principal(principal: &meerkat_core::PrincipalRef) -> Option<String> {
    if principal.kind != meerkat_core::PrincipalKind::Human {
        return None;
    }
    let meerkat_core::PrincipalQualification::Qualified(domain) = &principal.qualification else {
        return None;
    };
    let issuer = domain
        .as_str()
        .strip_prefix(crate::runtime::CONSOLE_HUMAN_TRUST_DOMAIN_PREFIX)
        .filter(|issuer| !issuer.is_empty())?;
    Some(private_owner_key(issuer, principal.id.as_str()))
}

/// A member's private owner, as its build established it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateOwner {
    /// The owner's private-owner key ([`crate::access::PRIVATE_TO_LABEL`]).
    pub(crate) key: String,
    /// The owner's console access principal, from the side chat's creator
    /// label: the key the session fence record uses.
    pub(crate) access_principal: String,
    /// When the member's session was created: a revocation of the owner's
    /// sessions at or after it suspends the accounts in this member.
    pub(crate) opened_at: SystemTime,
}

/// A composition's personal accounts: its account modes, and the console's
/// session fence record once a console is composed. Each member factory of
/// the composition has an owner check over them
/// ([`Self::owner_check`]), which keeps where that factory keeps the
/// people's accounts and the private owner each of its builds established.
pub struct PersonalMcpAccounts {
    config: Arc<McpAccountsConfig>,
    fences: RwLock<Option<crate::auth::session_fence::ConsoleSessionFences>>,
}

impl std::fmt::Debug for PersonalMcpAccounts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalMcpAccounts")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl PersonalMcpAccounts {
    pub fn new(config: McpAccountsConfig) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(config),
            fences: RwLock::new(None),
        })
    }

    /// The account modes.
    pub fn config(&self) -> &McpAccountsConfig {
        &self.config
    }

    /// Bind the console's session fence record, which an access admin's
    /// revocation writes ([`PersonalAccountRefusal::Suspended`]).
    pub fn bind_session_fences(&self, fences: crate::auth::session_fence::ConsoleSessionFences) {
        *self.fences.write().unwrap_or_else(PoisonError::into_inner) = Some(fences);
    }

    fn session_fences(&self) -> Option<crate::auth::session_fence::ConsoleSessionFences> {
        self.fences
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The refusal of a use of the personal server `server` by a run whose
    /// direct requester is `requester`, in a member whose private owner is
    /// `owner`; `None` when the use is the owner's own.
    pub(crate) fn refusal(
        &self,
        server: &str,
        owner: Option<&PrivateOwner>,
        requester: Option<&meerkat_core::PrincipalRef>,
    ) -> Option<PersonalAccountRefusal> {
        let server = server.to_string();
        let Some(owner) = owner else {
            return Some(PersonalAccountRefusal::SharedThread { server });
        };
        let acts_for_owner = requester
            .and_then(private_owner_key_of_principal)
            .is_some_and(|key| key == owner.key);
        if !acts_for_owner {
            return Some(PersonalAccountRefusal::NotOwner { server });
        }
        if self.suspended(owner) {
            return Some(PersonalAccountRefusal::Suspended { server });
        }
        None
    }

    /// Whether an access admin revoked the owner's sessions at or after the
    /// member was opened.
    fn suspended(&self, owner: &PrivateOwner) -> bool {
        let Some(fence) = self
            .session_fences()
            .and_then(|fences| fences.not_before(&owner.access_principal))
        else {
            return false;
        };
        let opened = owner
            .opened_at
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        fence >= opened
    }
}

/// The refusal of `config` when it is a personal server that cannot be one.
pub(crate) fn unsupported_refusal(config: &McpServerConfig) -> Option<PersonalAccountRefusal> {
    personal_server_shape(config)
        .err()
        .map(|reason| PersonalAccountRefusal::Unsupported {
            server: config.name.clone(),
            reason,
        })
}
