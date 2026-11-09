//! The owner check of one member factory.
//!
//! A member private to its owner keeps its personal servers, and carries its
//! owner's credential source, only in a build whose member factory composes
//! every MCP adapter with the owner check (`gate`). Where a build is
//! classified (`build`), the factory that will build it cannot be seen, so
//! the classification reads the owner check its spec was given: the one that
//! spec's member factory installed. Each member factory has a check of its
//! own ([`PersonalMcpAccounts::owner_check`]), which keeps where that factory
//! keeps the people's accounts and the private owner each of its builds
//! established, for that factory's composition to take. A check no factory
//! installed keeps no personal server in any build.
//!
//! The owner's source a build carries serves the owner's credential only
//! once the owner check of that build's own MCP adapter composition took the
//! build's plan ([`PersonalAccountOwnerCheck::take_plan`]): until then a read
//! of a personal server's credential waits, and a plan dropped untaken
//! answers it with nothing. So a build that reaches a factory without the
//! check (a spec given the check another factory installed) never uses an
//! owner's account: its personal server's connection gets no credential and
//! ends at its connect deadline, and nothing reaches the server.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use meerkat::McpAdapterComposer;
use meerkat_core::types::SessionId;
use tokio::sync::watch;

use super::{PersonalAccountCredentials, PersonalMcpAccounts, PrivateOwner};

/// The most member plans kept between a member's build and its MCP adapter
/// composition; a full map starts over.
const MEMBER_PLAN_LIMIT: usize = 4096;

/// The owner check of one member factory over a composition's personal
/// accounts ([`PersonalMcpAccounts::owner_check`]).
///
/// Install it on the member factory ([`Self::install`]), keep the people's
/// accounts in that factory's credential store ([`Self::keep_accounts_in`]),
/// and give it to the spec whose session service builds with that factory
/// ([`crate::MobBootstrapSpec::with_personal_mcp_accounts`]). A member build
/// keeps a personal server only through a check that is installed and keeps
/// the accounts, and its owner's credential serves only a build that this
/// check's composition composed.
#[derive(Clone)]
pub struct PersonalAccountOwnerCheck {
    inner: Arc<OwnerCheck>,
}

struct OwnerCheck {
    accounts: Arc<PersonalMcpAccounts>,
    /// Set once a member factory composes its MCP adapters with this check.
    installed: AtomicBool,
    /// Where that factory keeps the people's accounts.
    credentials: RwLock<Option<PersonalAccountCredentials>>,
    /// The plan each build kept, until the factory's composition of that
    /// build takes it.
    plans: Mutex<BTreeMap<SessionId, MemberPlan>>,
}

/// What one member build keeps for its MCP adapter composition: the private
/// owner it established, and the arm of the owner's source it carries.
struct MemberPlan {
    owner: PrivateOwner,
    /// Fired when the composition takes the plan; the build's owner source
    /// serves the owner's credential only after that.
    arm: watch::Sender<bool>,
}

impl std::fmt::Debug for PersonalAccountOwnerCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalAccountOwnerCheck")
            .field("accounts", &self.inner.accounts)
            .field("installed", &self.installed())
            .finish_non_exhaustive()
    }
}

impl PersonalMcpAccounts {
    /// A new owner check over these accounts, for one member factory (see
    /// [`PersonalAccountOwnerCheck`]).
    pub fn owner_check(self: &Arc<Self>) -> PersonalAccountOwnerCheck {
        PersonalAccountOwnerCheck {
            inner: Arc::new(OwnerCheck {
                accounts: Arc::clone(self),
                installed: AtomicBool::new(false),
                credentials: RwLock::new(None),
                plans: Mutex::new(BTreeMap::new()),
            }),
        }
    }
}

impl PersonalAccountOwnerCheck {
    /// The personal accounts this check is over.
    pub fn accounts(&self) -> &Arc<PersonalMcpAccounts> {
        &self.inner.accounts
    }

    /// `factory`, composing every MCP adapter it builds behind this owner
    /// check, over `host`'s composer when a host installed one.
    #[must_use]
    pub fn install(
        &self,
        factory: meerkat::AgentFactory,
        host: Option<Arc<dyn McpAdapterComposer>>,
    ) -> meerkat::AgentFactory {
        let factory = factory.with_mcp_adapter_composer(super::gate::composer(self.clone(), host));
        self.inner.installed.store(true, Ordering::Release);
        factory
    }

    /// Keep the people's accounts in `factory`'s own provider-auth
    /// persistence, each owner's in a realm of their own below
    /// `installation_realm`, with `authority`, the composition runtime's
    /// credential authority. `system` is the credential source the factory's
    /// members use for every other server (`None` when they connect without
    /// OAuth); a factory that has one uses
    /// [`PersonalMcpAccounts::member_credential_source`] over it.
    ///
    /// # Errors
    ///
    /// The factory has no provider-auth persistence, so no personal account
    /// could be kept; no member build keeps a personal server then.
    pub fn keep_accounts_in(
        &self,
        factory: &meerkat::AgentFactory,
        authority: meerkat_runtime::ProviderAuthRuntimeAuthority,
        installation_realm: meerkat_core::RealmId,
        system: Option<Arc<dyn meerkat_mcp::McpAuthResolver>>,
    ) -> Result<(), meerkat::HostAuthError> {
        let host_auth = meerkat::HostAuthService::from_factory(factory, authority)?;
        self.bind_credentials(PersonalAccountCredentials::new(
            host_auth,
            installation_realm,
            system,
        ));
        Ok(())
    }

    /// Bind where this check's factory keeps the people's accounts. Until it
    /// is bound, no member build keeps a personal server.
    pub fn bind_credentials(&self, credentials: PersonalAccountCredentials) {
        *self
            .inner
            .credentials
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(credentials);
    }

    pub(crate) fn credentials(&self) -> Option<PersonalAccountCredentials> {
        self.inner
            .credentials
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Whether a member factory composes its MCP adapters with this check.
    pub(crate) fn installed(&self) -> bool {
        self.inner.installed.load(Ordering::Acquire)
    }

    /// Keep the plan of the build of `session`: its private owner, and `arm`,
    /// the arm of the owner's source the build carries.
    pub(crate) fn keep_plan(
        &self,
        session: SessionId,
        owner: PrivateOwner,
        arm: watch::Sender<bool>,
    ) {
        let mut plans = self
            .inner
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if plans.len() >= MEMBER_PLAN_LIMIT {
            plans.clear();
        }
        plans.insert(session, MemberPlan { owner, arm });
    }

    /// Take the plan of the build of `session`, as that build's MCP adapter
    /// composition does: the owner's source the build carries serves the
    /// owner's credential from here on.
    pub(crate) fn take_plan(&self, session: &SessionId) -> Option<PrivateOwner> {
        let plan = self
            .inner
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(session)?;
        plan.arm.send_replace(true);
        Some(plan.owner)
    }
}
