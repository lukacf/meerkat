//! A host's admission of one administrative mutation, checked by the
//! mutation's owner at the mutation's entry.
//!
//! An owner that writes a realm's configuration or credentials for an
//! authenticated administrator waits for custody first (the config
//! document's locks and generation, the credential mutation owner's
//! exclusion). The administrator can lose that authority while the write
//! waits, so a check made before or around those waits admits nothing. The
//! owner runs the host's [`AdministrationEntry`] after it holds that custody,
//! immediately before it writes anything, and holds the returned
//! [`EnteredAdministration`] until the mutation has settled.
//!
//! An admission can state who administers: the principal the host
//! authenticated and admitted, with the login it was authenticated in
//! ([`AdministeringPrincipal`]), and the host's [`AdministrationRecorder`]
//! ([`EnteredAdministration::administered_by`]). The mutation's owner then
//! hands that recorder one [`AdministrationRecord`] when the mutation
//! settles: who administered it, what it changed and how it ended, also when
//! the caller that asked for the mutation stopped waiting. The host keeps the
//! record in its own audit; Core keeps none.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::PrincipalRef;
use crate::approval::consent::HostAuthenticationScope;
use crate::approval::decision::Sha256Digest;
use crate::auth::principal::PrincipalContractError;
use crate::connection::RealmId;

/// A host's admission check for one administrative mutation of a realm's
/// configuration or credentials.
///
/// The mutation's owner calls [`Self::enter`] at the mutation's entry: after
/// it holds every custody it waits for, and immediately before anything is
/// written. A refusal writes nothing. The check runs while the owner holds
/// that custody, so it decides from state it can read at once and never
/// waits; it is the host's own authentication and authorization, which the
/// owner neither performs nor replaces.
pub trait AdministrationEntry: Send + Sync {
    /// Admit the mutation now, or refuse it.
    fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal>;
}

/// An administrative mutation its host admitted at the mutation's entry.
///
/// The mutation's owner holds it until the mutation has settled (written, or
/// failed with its outcome known or unknown) and only then drops it, so a
/// host that records its entered operations sees the mutation as entered
/// until it settles. It authorizes nothing by itself. An admission that
/// states its administrator ([`Self::administered_by`]) is recorded by the
/// host's recorder when the mutation settles; Core stores none.
pub struct EnteredAdministration {
    _held: Option<Box<dyn std::any::Any + Send>>,
    administration: Option<StatedAdministration>,
}

/// What an admission states about its administrator, and where its record
/// goes.
struct StatedAdministration {
    administrator: AdministeringPrincipal,
    // Read by the record's delivery, which a later change adds.
    #[allow(dead_code)]
    recorder: Arc<dyn AdministrationRecorder>,
    admitted_at: DateTime<Utc>,
}

impl EnteredAdministration {
    /// An admission that holds nothing.
    pub fn admitted() -> Self {
        Self {
            _held: None,
            administration: None,
        }
    }

    /// An admission holding `guard`, the host's own record of the entered
    /// operation, until the mutation settles.
    pub fn holding(guard: impl Send + 'static) -> Self {
        Self {
            _held: Some(Box::new(guard)),
            administration: None,
        }
    }

    /// This admission stating `administrator`, the principal the host
    /// authenticated and admitted to the mutation now, and `recorder`, the
    /// host's record of its administration: when the mutation settles, its
    /// owner hands `recorder` one [`AdministrationRecord`] attributing the
    /// mutation to `administrator`. It authorizes nothing: the admission
    /// itself is the host's decision.
    #[must_use]
    pub fn administered_by(
        mut self,
        administrator: AdministeringPrincipal,
        recorder: Arc<dyn AdministrationRecorder>,
    ) -> Self {
        self.administration = Some(StatedAdministration {
            administrator,
            recorder,
            admitted_at: Utc::now(),
        });
        self
    }

    /// The administrator this admission states, if any.
    #[must_use]
    pub fn administrator(&self) -> Option<&AdministeringPrincipal> {
        self.administration
            .as_ref()
            .map(|administration| &administration.administrator)
    }
}

impl std::fmt::Debug for EnteredAdministration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("EnteredAdministration");
        if let Some(administration) = &self.administration {
            debug
                .field("administrator", &administration.administrator)
                .field("admitted_at", &administration.admitted_at);
        }
        debug.finish_non_exhaustive()
    }
}

/// The administrator an admission states: the qualified principal the host
/// authenticated and admitted, and, when the host named one, the digest of
/// the authentication scope (login or session) it was authenticated in.
///
/// A host statement, opaque to Core: constructing one authenticates nobody
/// and grants nothing. It names what the mutation's record attributes the
/// mutation to; the login is kept only as its digest
/// ([`HostAuthenticationScope::digest`]), never the scope itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdministeringPrincipal {
    principal: PrincipalRef,
    authentication_scope_digest: Option<Sha256Digest>,
}

impl AdministeringPrincipal {
    /// The administrator `principal`, which must name an explicit trust
    /// domain: two hosts' principals with the same id stay apart.
    ///
    /// # Errors
    /// [`PrincipalContractError::UnqualifiedPrincipal`] for a principal
    /// without a trust domain, and the id's own error for an invalid id.
    pub fn new(principal: PrincipalRef) -> Result<Self, PrincipalContractError> {
        principal.validate_qualified()?;
        Ok(Self {
            principal,
            authentication_scope_digest: None,
        })
    }

    /// The administrator authenticated in `scope`, the exact login or
    /// session of the request the host admitted. Only its digest is kept.
    #[must_use]
    pub fn with_authentication_scope(mut self, scope: &HostAuthenticationScope) -> Self {
        self.authentication_scope_digest = Some(scope.digest());
        self
    }

    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    /// The digest of the login the administrator was authenticated in, when
    /// the host named one.
    #[must_use]
    pub fn authentication_scope_digest(&self) -> Option<&Sha256Digest> {
        self.authentication_scope_digest.as_ref()
    }
}

/// A host's record of the administrative mutations it admitted.
///
/// The mutation's owner calls [`Self::record`] once per admission that
/// states an administrator, when it releases the admission: the mutation
/// settled, or the owner's task ended without knowing its result. It may run
/// on the owner's task while the owner still holds the mutation's custody,
/// so it returns promptly and never waits for that owner (a host that
/// persists the record queues it). Nothing reads the record back to decide
/// anything.
pub trait AdministrationRecorder: Send + Sync {
    /// Record one admitted administrative mutation.
    fn record(&self, record: AdministrationRecord);
}

/// One administrative mutation a host admitted, as its owner settled it:
/// who administered it, what it changed, how it ended and when. It carries
/// no secret or value, only non-secret references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AdministrationRecord {
    /// The administrator the admission stated.
    pub administrator: PrincipalRef,
    /// The digest of the login the administrator was authenticated in, when
    /// the host named one ([`HostAuthenticationScope::digest`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication_scope_digest: Option<Sha256Digest>,
    /// What the mutation changes, as its owner named it; `None` when the
    /// owner named nothing before it released the admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<AdministeredChange>,
    pub outcome: AdministrationOutcome,
    /// When the host's entry admitted the mutation.
    pub admitted_at: DateTime<Utc>,
    /// When the mutation's owner released the admission.
    pub settled_at: DateTime<Utc>,
}

/// What an admitted administrative mutation changes, as its owner names it
/// before it writes. Non-secret references only: a value or secret is never
/// part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdministeredChange {
    /// A patch of a realm's own configuration document, which the write
    /// publishes at `generation`. The patched values are not recorded.
    ConfigPatch {
        /// The realm the document's store names, when it names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<String>,
        generation: u64,
    },
    /// A value set for one service-secret slot of an MCP stdio server that
    /// `realm`'s own configuration defines.
    McpServiceSecretSet {
        realm: RealmId,
        server: String,
        env: String,
        /// The server definition's canonical launch meaning (lowercase hex
        /// SHA-256).
        meaning: String,
    },
    /// The value of such a slot removed.
    McpServiceSecretRemove {
        realm: RealmId,
        server: String,
        env: String,
        meaning: String,
    },
    /// A host-registered OAuth client profile installed for one MCP
    /// credential slot.
    McpOauthClientProfileInstall {
        /// The realm of the credential slot, when it names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
        server_url: String,
        /// The slot's credential binding (an opaque digest of the server,
        /// realm and account selection).
        slot: String,
    },
    /// The client profile of such a slot replaced.
    McpOauthClientProfileReplace {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
        server_url: String,
        slot: String,
    },
    /// The client profile of such a slot removed.
    McpOauthClientProfileRemove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
        server_url: String,
        slot: String,
    },
}

/// How an admitted administrative mutation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdministrationOutcome {
    /// The change committed: its owner received the write's durable result.
    Committed,
    /// The write began and its result is not known: the administered object
    /// may hold the earlier state or the new one.
    Unknown,
}

/// Why an administrative mutation was not admitted at its entry. Nothing was
/// written.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdministrationEntryRefusal {
    /// The caller is no longer authenticated.
    #[error("the caller is no longer authenticated")]
    NotAuthenticated,
    /// The caller is authenticated but does not administer now.
    #[error("the caller does not administer this installation")]
    NotAdministrator,
    /// The caller's authority could not be established now.
    #[error("the caller's administration could not be established: {0}")]
    Unavailable(String),
}
