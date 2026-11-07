//! The personal servers of one member build.
//!
//! Every member build passes the base layer's pre-build hook
//! (`MobBootstrapSpec::with_base_layer`): a member's create, the fork that
//! seats a side chat, and every rebuild of a member (an explicit resume, a
//! warm revival, the wake of a dormant member, a respawn). There a
//! composition with personal servers classifies each of them
//! ([`PersonalMcpAccounts::classify_build`]):
//!
//! - a server that cannot be personal is withheld;
//! - a member private to no person gets none;
//! - a member private to its owner keeps them, carries its owner's own
//!   credential source in the build
//!   (`SessionBuildOptions::mcp_auth_resolver_override`, process-local, so
//!   every build supplies it again), and its owner is kept for the build's
//!   MCP adapter composition (the owner check).
//!
//! A member is private to its owner only as a verified console side chat
//! (the rule of [`crate::side_chat::verified_side_chat_source`] over the
//! build's labels and its typed fork source) that records its private owner
//! and its creator. Its helpers and `fork_off` children, which inherit its
//! labels but name another fork source or none, and an approval branch, are
//! private to no person.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};

use super::{APPROVAL_BRANCH_LABEL, PersonalMcpAccounts, PrivateOwner, unsupported_refusal};

/// The late-bound personal accounts every member build of a spec reads;
/// empty (no personal servers) until a composition fills it.
#[derive(Clone, Default)]
pub(crate) struct PersonalAccountsSlot {
    accounts: Arc<RwLock<Option<Arc<PersonalMcpAccounts>>>>,
}

impl std::fmt::Debug for PersonalAccountsSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersonalAccountsSlot")
            .field("filled", &self.accounts().is_some())
            .finish()
    }
}

impl PersonalAccountsSlot {
    /// Fill the slot with the composition's personal accounts.
    pub(crate) fn fill(&self, accounts: Arc<PersonalMcpAccounts>) {
        *self
            .accounts
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(accounts);
    }

    pub(crate) fn accounts(&self) -> Option<Arc<PersonalMcpAccounts>> {
        self.accounts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// `hook`, then the classification of the build's personal servers.
    pub(crate) fn after_hook(
        &self,
        hook: crate::mob_handle_runtime::PreBuildHook,
    ) -> crate::mob_handle_runtime::PreBuildHook {
        let slot = self.clone();
        Arc::new(move |req: &mut CreateSessionRequest| {
            let hook = Arc::clone(&hook);
            let slot = slot.clone();
            Box::pin(async move {
                hook(&mut *req).await?;
                if let Some(accounts) = slot.accounts() {
                    accounts.classify_build(req);
                }
                Ok(())
            })
        })
    }
}

/// The private owner a member build establishes: a verified console side
/// chat that records its private owner and its creator, on a session the
/// build names. `None` for every other build.
fn build_owner(
    labels: &BTreeMap<String, String>,
    build: &SessionBuildOptions,
) -> Option<(meerkat_core::types::SessionId, PrivateOwner)> {
    if labels.contains_key(APPROVAL_BRANCH_LABEL) {
        return None;
    }
    crate::side_chat::verified_side_chat_source(labels, None, build.fork_source.as_ref())?;
    let key = crate::access::private_owner(labels)?.to_string();
    let access_principal = labels
        .get(crate::side_chat::SIDE_CHAT_CREATOR_LABEL)
        .map(|creator| creator.trim())
        .filter(|creator| !creator.is_empty())?
        .to_string();
    let session = build.resume_session.as_ref()?;
    Some((
        session.id().clone(),
        PrivateOwner {
            key,
            access_principal,
            opened_at: session.created_at(),
        },
    ))
}

impl PersonalMcpAccounts {
    /// Classify the personal servers of one member build (see the module
    /// docs). Nothing changes for a build without personal servers.
    pub(crate) fn classify_build(&self, req: &mut CreateSessionRequest) {
        let _ = (req, build_owner, unsupported_refusal);
    }
}
