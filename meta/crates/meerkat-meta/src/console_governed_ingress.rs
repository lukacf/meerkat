//! The trusted host's governance of Console sends.
//!
//! A host whose members admit only governed input (an authority association
//! and a trusted ingress context, bound by the host) installs a
//! [`ConsoleGovernedIngress`] on the runtime
//! ([`crate::UnifiedRuntime::set_console_governed_ingress`]). A Console send
//! to an identity the host governs then reaches the member only through the
//! host's [`GovernedInputBinder`], built for the authenticated
//! [`AuthenticatedConsoleHuman`] who sent it: Meta carries that human, never
//! mints, alters or drops an association, and never falls back to an
//! ungoverned delivery. A caller without a verified human (an anonymous
//! caller of an open Console, a service identity, a token without a `sub`)
//! is refused before anything is reserved or bound.
//!
//! Compatibility restriction: installing an ingress closes the Console's
//! member-only send path (a target the identity-first runtime does not know,
//! reached by the aggregator's own alias resolution) for every target,
//! governed or not, because that path can carry no binding and the
//! governance decision cannot be checked there. Such sends are refused
//! (`governed_send_refused`). A runtime whose machine has a host's native
//! work authorization owner installed governs every Console send whatever
//! [`ConsoleGovernedIngress::governs`] says.
//!
//! A host that pins its binder to one specific send (for example to an
//! original it reviewed before the human sent it) implements
//! [`ConsoleGovernedIngress::binder_for_send`], which also receives the
//! send's [`ConsoleSendContext`].

use std::sync::Arc;

use crate::identity_first::{AgentIdentity, GovernedInputBinder};
use crate::runtime::AuthenticatedConsoleHuman;

/// The trusted host's governance of Console sends; see the module docs.
pub trait ConsoleGovernedIngress: Send + Sync {
    /// Whether the host governs Console input to `identity` (the canonical
    /// identity the send resolved to). `false` keeps the existing
    /// identity-first delivery for that identity only on a runtime that is
    /// not itself governed; it never reopens the member-only send path,
    /// which an installed ingress closes for every target (see the module's
    /// compatibility restriction), and it cannot ungovern a governed runtime.
    fn governs(&self, identity: &AgentIdentity) -> bool;

    /// The host's binder for one Console send by `human` to `identity`, which
    /// must be governed: because the host [`governs`](Self::governs) it, or
    /// because the runtime itself is governed (whatever `governs` says). The
    /// binder binds the raw input for the member's exact current session with
    /// the human as its authenticated original; an `Err` refuses the send
    /// before anything is submitted.
    fn binder_for(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn GovernedInputBinder>, String>;

    /// The host's binder for one Console send, as
    /// [`binder_for`](Self::binder_for), with that send's
    /// [`ConsoleSendContext`]. This is the method the Console send path
    /// calls; the default ignores the context and returns
    /// [`binder_for`](Self::binder_for), so a host that does not need it
    /// implements only that.
    ///
    /// When and where it runs: once per governed Console send (an exact
    /// retry under the same key is a new send and calls it again with the
    /// same key), synchronously on the send path, after the target resolved
    /// to its canonical `identity` and the sender to the verified `human`,
    /// outside any identity lease or lifecycle lock and BEFORE the Console
    /// reserves anything for the send (its acceptance and transcript frame).
    /// An `Err` refuses the send there: nothing is reserved, bound or
    /// submitted. The returned binder's [`GovernedInputBinder::bind`] runs
    /// later, in the send's delivery, under the identity's lifecycle lock and
    /// lease, for the member's exact current session (the target may be
    /// materialized inside the send, after this call). A refusal in `bind`
    /// admits nothing and fails the accepted send; its idempotency key then
    /// replays that failure, it never resubmits the input.
    ///
    /// The context is a correlation handle, NOT an authority: its
    /// `idempotency_key` is chosen by the client, so any caller allowed to
    /// send may present any key. A host may use it to find what it prepared
    /// for this send (for example a review of the original the human is
    /// about to send), never as proof of that preparation. The binder must
    /// still check everything it relies on at bind: that the canonical
    /// `identity` and the verified human (and its login) are still the ones
    /// the preparation was for, the BOUND
    /// [`GovernedBindContext::session_id`](crate::identity_first::GovernedBindContext::session_id)
    /// (not a session read before the send), and the actual input content
    /// (for example its digest against the reviewed digest). Any mismatch
    /// refuses in `bind`, before admission.
    ///
    /// The input `bind` receives is the stock Console input: created after
    /// the Console's acceptance and reservation, with a fresh `InputId` and
    /// the Console's own idempotency key
    /// (`console:{interaction_id}:{input_frame_id}`) in its header. The
    /// binder binds exactly that input and must not replace it; a host that
    /// ties its preparation to one admitted input records that input's actual
    /// `InputId` itself (for example with its own first-bind
    /// compare-and-set).
    fn binder_for_send(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
        send: &ConsoleSendContext,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        let _ = send;
        self.binder_for(identity, human)
    }
}

/// One Console send as the host sees it at
/// [`ConsoleGovernedIngress::binder_for_send`]: the request's own values,
/// verbatim. Client-supplied: a correlation handle, never an authority (see
/// that method).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConsoleSendContext {
    /// The request's `idempotency_key`, the only identifier of the send
    /// known before it is accepted.
    pub idempotency_key: String,
    /// The request's free-form `origin`.
    pub origin: String,
}

impl ConsoleSendContext {
    pub(crate) fn for_request(request: &crate::console_aggregator::ConsoleSendRequest) -> Self {
        Self {
            idempotency_key: request.idempotency_key.clone(),
            origin: request.origin.clone(),
        }
    }
}

/// The runtime's slot for the installed [`ConsoleGovernedIngress`], read at
/// each Console send so a host may install it after the Console router is
/// built.
#[derive(Clone, Default)]
pub(crate) struct ConsoleGovernedIngressSlot(
    Arc<std::sync::RwLock<Option<Arc<dyn ConsoleGovernedIngress>>>>,
);

impl ConsoleGovernedIngressSlot {
    pub(crate) fn install(&self, ingress: Arc<dyn ConsoleGovernedIngress>) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ingress);
    }

    pub(crate) fn current(&self) -> Option<Arc<dyn ConsoleGovernedIngress>> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for ConsoleGovernedIngressSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleGovernedIngressSlot")
            .field("installed", &self.current().is_some())
            .finish()
    }
}
