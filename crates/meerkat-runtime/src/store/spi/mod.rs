//! Host store adapters: Core's realm state kept in a host's own database
//! through the host store SPI ([`meerkat_core::host_store`]).
//!
//! - [`SpiRealm`] is the one per-process handle of a realm: the fence and
//!   its lease, the per-scope journal state and the commit engine. Every
//!   adapter of the process commits through it.
//! - [`SpiRuntimeStore`] is the [`crate::store::RuntimeStore`] over the
//!   realm: it keeps the in-memory store's semantics
//!   ([`crate::store::InMemoryRuntimeStore`], WholeBlobV1) and writes every
//!   mutation through to the host as one batch before it answers.
//! - [`SpiSessionStore`] is the realm's session slot: reads of the committed
//!   documents and the catalog through the runtime store, writes refused.
//! - `chapter::large_documents`, built with the `test-support` feature, is a
//!   check a host runs against its own store: Core's adapter puts 20 MiB
//!   documents through a round commit and a re-base, and reads them back
//!   byte-exact from the host after each restart.
//!
//! # Layout
//!
//! Each runtime is one session scope ([`RUNTIME_FAMILY`]): its head pins
//! the facts a boot needs (the catalog entry, the machine lifecycle, the
//! delivery authority and the boot facts), and its rows carry everything
//! else as keyed entries (input states, receipts, the WholeBlob body and
//! its provisional tail, ...). Realm-wide maps are record scopes, one per
//! entry: continuation key bindings, direct member high waters, mob host
//! bindings and their revocations. Values larger than
//! [`realm::INLINE_VALUE_LIMIT`] live in chunk scopes, written before the
//! batch that references them. See [`codec`] for the durable formats.
//!
//! # Boot and memory
//!
//! A boot lists every head once. The pinned entries of every runtime are
//! imported, so listings, the delivery authority listing and the controller
//! inventory see every runtime. A runtime's body is loaded at boot only
//! when recovery must act on it: it has nonterminal inputs or work its ops
//! lifecycle snapshot owes (an undelivered background operation completion,
//! a running detached job wait), pending terminal owners, a provisional
//! tail (an open run cursor), a current run in its lifecycle, or input rows
//! without a lifecycle. Archived runtimes are never loaded at boot. Every
//! other body loads on the first call that names the runtime and stays
//! resident until the runtime is archived.
//! Resident memory is therefore bounded by the bodies of the runtimes
//! opened since the process started and not archived since, plus every
//! runtime's pinned head entries. The store's obligation scan asks the same
//! predicate as the boot and answers from memory, with no host read.
//!
//! The boot reads the runtimes up to 16 at once (the head entries a runtime
//! keeps as chunks, and the bodies it loads at boot); one runtime's reads
//! stay in order. It installs nothing until every read succeeded, then each
//! runtime in key order, so a failed read fails the open with no runtime
//! installed.
//!
//! # Retention
//!
//! A row entry that changes is appended again, whole, so a scope's rows
//! hold superseded values until a journal checkpoint drops them (the host
//! store reference calls it a re-base; it is not the run re-base of a
//! WholeBlob run after a compaction): once the rows appended since the last
//! checkpoint pass four times the scope's live bytes or
//! [`realm::CHECKPOINT_MIN_BYTES`], whichever is larger, its next commit
//! writes the live image and drops every earlier row in one batch. Only a
//! loaded scope is checkpointed: a body that stays on the host keeps its
//! rows until its next commit.
//!
//! # Limits of this adapter
//!
//! - OAuth login flow snapshots are process-local: the three snapshot
//!   methods of [`crate::store::RuntimeStore`] are synchronous and the SPI
//!   is not, so an interactive login in progress does not survive a
//!   restart (the user starts it again). Completed credentials are kept by
//!   the credential stores and are not affected.
//! - Temporary councils are process-local too: they have no host store
//!   adapter, and the default council store declares itself process-bound,
//!   so a council that requires durable custody is refused and a
//!   process-bound one does not survive a restart.
//! - The WholeBlob candidate document is written whole at every per-round
//!   commit, so the bytes a run writes grow with the square of its rounds.
//!   Its chunks are replaced, not accumulated.

#![cfg(not(target_arch = "wasm32"))]

#[cfg(any(test, feature = "test-support"))]
pub mod chapter;
pub mod codec;
pub mod realm;
mod runtime;
mod session;
#[cfg(test)]
pub(crate) mod test_host;
#[cfg(test)]
mod tests;
pub mod writer_fence;

use std::time::Duration;

use meerkat_core::host_store::{FenceToken, HostStoreError};

pub use realm::{
    ArmedScopes, PoisonedScope, ScopeCell, ScopeDelta, ScopeFamily, ScopeGuard, ScopeId,
    ScopeState, SpiRealm, SpiRealmConfig,
};
pub use runtime::{RUNTIME_FAMILY, SpiRuntimeStore};
pub use session::SpiSessionStore;

use super::{RefusedScope, RuntimeStoreError};

/// Why a host store adapter refused or failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SpiError {
    /// The realm lost its fence: every call refuses until it is reopened.
    #[error("host store realm is poisoned: {0}")]
    RealmPoisoned(String),
    /// A batch touching this scope failed after memory changed: the scope
    /// refuses every call until the realm is reopened.
    #[error("host store scope {scope} is poisoned: {cause}")]
    ScopePoisoned { scope: String, cause: String },
    /// A read across the scopes of some families met poisoned ones: it
    /// refuses until the realm is reopened. Names each with its cause.
    #[error("{}", PoisonedScopesDisplay(scopes))]
    ScopesPoisoned { scopes: Vec<PoisonedScope> },
    /// An exclusive acquire met a live holder.
    #[error("host store fence is held by token {holder} for {remaining:?} more")]
    Held {
        holder: FenceToken,
        remaining: Duration,
    },
    /// The local lease is not live and a renew could not restore it; the
    /// call changed nothing.
    #[error("host store lease is not live: {0}")]
    LeaseLapsed(String),
    /// A commit stopped settling its batch when the self-fence window of the
    /// fence token the batch carries closed: Core sends no batch past that
    /// window, since the token's lease at the store may have ended by then.
    /// The batch was sent `attempts` times, `last` answering the last of them
    /// (`None`: it was never sent), so it may or may not have landed. The
    /// scopes it touched are poisoned like those of any batch whose outcome
    /// stays unknown; the realm keeps its fence.
    #[error(
        "host store batch is unsettled: the self-fence window of fence token {token} closed \
         (attempts sent: {attempts}{})",
        LastAnswer(.last.as_ref())
    )]
    UnsettledAtFenceWindow {
        token: FenceToken,
        attempts: u32,
        last: Option<HostStoreError>,
    },
    /// A commit stopped settling its batch when its retry budget ran out,
    /// the self-fence window of its fence token open throughout: every
    /// attempt answered unknown or unavailable and no head read showed the
    /// batch. The batch was sent `attempts` times, `last` answering the last
    /// of them, so it may or may not have landed. The scopes it touched are
    /// poisoned like those of any batch whose outcome stays unknown; the
    /// realm keeps its fence.
    #[error(
        "host store batch is unsettled: its retry budget ran out (attempts sent: {attempts}; \
         last answer: {last})"
    )]
    UnsettledPastRetryBudget { attempts: u32, last: HostStoreError },
    /// The host store failed.
    #[error("host store: {0}")]
    Host(#[from] HostStoreError),
    /// One call's change does not fit one batch: its heads and rows
    /// exceed the byte budget even with every large row set spilled to a
    /// chunk. The scopes it touched are poisoned like any failed batch's.
    #[error(
        "host store batch too large: {head_bytes} head bytes and {row_bytes} row bytes exceed the {budget} byte budget"
    )]
    TooLarge {
        head_bytes: usize,
        row_bytes: usize,
        budget: usize,
    },
    /// Host data does not decode or is inconsistent.
    #[error("host store data is invalid: {0}")]
    Corrupt(String),
    /// The adapter was used against its rules (a bug).
    #[error("host store adapter misuse: {0}")]
    Misuse(String),
}

impl SpiError {
    /// The error a mutating store call answers. A poisoned realm or scope
    /// answers its typed refusal ([`RuntimeStoreError::RealmRefused`] or
    /// [`RuntimeStoreError::ScopeRefused`]), and a batch left unsettled
    /// answers its typed stop: [`RuntimeStoreError::UnsettledAtFenceWindow`]
    /// when its fence window closed, [`RuntimeStoreError::UnsettledPastRetryBudget`]
    /// when its retry budget ran out.
    #[must_use]
    pub fn into_write(self) -> RuntimeStoreError {
        match self.into_typed() {
            Ok(typed) => typed,
            Err(other) => RuntimeStoreError::WriteFailed(other.to_string()),
        }
    }

    /// The error a reading store call answers. A poisoned realm or scope
    /// answers its typed refusal ([`RuntimeStoreError::RealmRefused`] or
    /// [`RuntimeStoreError::ScopeRefused`]), a host store that could not
    /// serve now reads as [`RuntimeStoreError::Unavailable`], and
    /// data that does not decode or fails the host's own integrity check as
    /// [`RuntimeStoreError::Corrupt`], so a reader can tell a refusal that
    /// lasts until the realm reopens and an outage a later read may get past
    /// from state that needs a repair. A read that changed memory commits
    /// like a write, and a batch it left unsettled answers its typed stop, as
    /// a write's does ([`Self::into_write`]).
    #[must_use]
    pub fn into_read(self) -> RuntimeStoreError {
        match self.into_typed() {
            Ok(typed) => typed,
            Err(error @ Self::Host(HostStoreError::Unavailable(_))) => {
                RuntimeStoreError::Unavailable(error.to_string())
            }
            Err(error @ (Self::Corrupt(_) | Self::Host(HostStoreError::Corrupt(_)))) => {
                RuntimeStoreError::Corrupt(error.to_string())
            }
            Err(other) => RuntimeStoreError::ReadFailed(other.to_string()),
        }
    }

    /// The typed error a read and a write both answer: a refusal until the
    /// realm is reopened ([`Self::into_refusal`]), or a batch left unsettled
    /// when its fence window closed or its retry budget ran out. Any other
    /// error comes back unchanged.
    fn into_typed(self) -> Result<RuntimeStoreError, Self> {
        match self.into_refusal() {
            Ok(refusal) => Ok(refusal),
            Err(error @ Self::UnsettledAtFenceWindow { .. }) => {
                Ok(RuntimeStoreError::UnsettledAtFenceWindow {
                    cause: error.to_string(),
                })
            }
            Err(error @ Self::UnsettledPastRetryBudget { .. }) => {
                Ok(RuntimeStoreError::UnsettledPastRetryBudget {
                    cause: error.to_string(),
                })
            }
            Err(other) => Err(other),
        }
    }

    /// The typed refusal a poisoned realm or scope answers until the realm
    /// is reopened: [`RuntimeStoreError::RealmRefused`] for a realm that lost
    /// its fence, [`RuntimeStoreError::ScopeRefused`] naming each poisoned
    /// scope the call needs. Any other error comes back unchanged.
    fn into_refusal(self) -> Result<RuntimeStoreError, Self> {
        match self {
            Self::RealmPoisoned(cause) => Ok(RuntimeStoreError::RealmRefused { cause }),
            Self::ScopePoisoned { scope, cause } => Ok(RuntimeStoreError::ScopeRefused {
                scopes: vec![RefusedScope::new(scope, cause)],
            }),
            Self::ScopesPoisoned { scopes } => Ok(RuntimeStoreError::ScopeRefused {
                scopes: scopes
                    .into_iter()
                    .map(|poisoned| RefusedScope::new(poisoned.id.to_string(), poisoned.cause))
                    .collect(),
            }),
            other => Err(other),
        }
    }
}

/// The last answer of a [`SpiError::UnsettledAtFenceWindow`] batch, when it
/// was sent.
struct LastAnswer<'a>(Option<&'a HostStoreError>);

impl std::fmt::Display for LastAnswer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(last) => write!(f, "; last answer: {last}"),
            None => Ok(()),
        }
    }
}

/// The scopes of a [`SpiError::ScopesPoisoned`], each with its cause.
struct PoisonedScopesDisplay<'a>(&'a [PoisonedScope]);

impl std::fmt::Display for PoisonedScopesDisplay<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            [] => f.write_str("a host store scope is poisoned"),
            [only] => write!(
                f,
                "host store scope {} is poisoned: {}",
                only.id, only.cause
            ),
            several => {
                f.write_str("host store scopes ")?;
                for (index, scope) in several.iter().enumerate() {
                    if index > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", scope.id)?;
                }
                f.write_str(" are poisoned (")?;
                for (index, scope) in several.iter().enumerate() {
                    if index > 0 {
                        f.write_str("; ")?;
                    }
                    write!(f, "{}: {}", scope.id, scope.cause)?;
                }
                f.write_str(")")
            }
        }
    }
}
