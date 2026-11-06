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
/// states its administrator ([`Self::administered_by`]) hands the host's
/// recorder the mutation's record when the owner drops it, before the
/// host's own guard is released; Core stores none.
pub struct EnteredAdministration {
    _held: Option<Box<dyn std::any::Any + Send>>,
    administration: Option<StatedAdministration>,
}

/// What an admission states about its administrator, where its record
/// goes, and what the mutation's owner stated about the mutation since.
struct StatedAdministration {
    administrator: AdministeringPrincipal,
    recorder: Arc<dyn AdministrationRecorder>,
    admitted_at: DateTime<Utc>,
    change: Option<AdministeredChange>,
    outcome: Option<AdministrationOutcome>,
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
            change: None,
            outcome: None,
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

    /// This admission naming `change`, what the admitted mutation changes.
    /// The mutation's owner names it when it receives the admission, before
    /// it writes; the record carries it. Without a stated administrator it
    /// is not kept.
    #[must_use]
    pub fn for_change(mut self, change: AdministeredChange) -> Self {
        if let Some(administration) = &mut self.administration {
            administration.change = Some(change);
        }
        self
    }

    /// The mutation's owner states how the admitted mutation ended, once it
    /// knows; the record carries it when the owner drops the admission. An
    /// admission dropped without it records
    /// [`AdministrationOutcome::Unknown`].
    pub fn settle(&mut self, outcome: AdministrationOutcome) {
        if let Some(administration) = &mut self.administration {
            administration.outcome = Some(outcome);
        }
    }
}

impl Drop for EnteredAdministration {
    /// Dropping an admission that states its administrator hands the host's
    /// recorder the mutation's one record, whichever task drops it last.
    fn drop(&mut self) {
        let Some(StatedAdministration {
            administrator,
            recorder,
            admitted_at,
            change,
            outcome,
        }) = self.administration.take()
        else {
            return;
        };
        let record = AdministrationRecord {
            administrator: administrator.principal,
            authentication_scope_digest: administrator.authentication_scope_digest,
            change,
            outcome: outcome.unwrap_or(AdministrationOutcome::Unknown),
            admitted_at,
            settled_at: Utc::now(),
        };
        // The recorder is host code: it never unwinds through the owner that
        // drops the admission, whose task may be unwinding already.
        let recorded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            recorder.record(record);
        }));
        if recorded.is_err() {
            tracing::warn!("an administration recorder panicked; the record is lost");
        }
    }
}

impl std::fmt::Debug for EnteredAdministration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug = f.debug_struct("EnteredAdministration");
        if let Some(administration) = &self.administration {
            debug
                .field("administrator", &administration.administrator)
                .field("admitted_at", &administration.admitted_at)
                .field("change", &administration.change)
                .field("outcome", &administration.outcome);
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
        /// The realm the document's store names, when it names a valid
        /// realm id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
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
    /// credential slot. The record names the slot by its server, realm and
    /// credential binding, never by the server's URL, which can carry
    /// credentials in its userinfo or its query.
    McpOauthClientProfileInstall {
        /// The realm of the credential slot, when it names one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
        /// The slot's credential binding: an opaque slug that digests the
        /// server's name and URL, the realm and the account selection.
        slot: String,
    },
    /// The client profile of such a slot replaced.
    McpOauthClientProfileReplace {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
        slot: String,
    },
    /// The client profile of such a slot removed.
    McpOauthClientProfileRemove {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        realm: Option<RealmId>,
        server: String,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::{PrincipalKind, TrustDomainId};

    #[derive(Default)]
    struct Recorded(std::sync::Mutex<Vec<AdministrationRecord>>);

    impl Recorded {
        fn take(&self) -> Vec<AdministrationRecord> {
            std::mem::take(&mut *self.0.lock().unwrap())
        }
    }

    impl AdministrationRecorder for Recorded {
        fn record(&self, record: AdministrationRecord) {
            self.0.lock().unwrap().push(record);
        }
    }

    fn administrator() -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::Human,
            "admin-7",
            TrustDomainId::new("example-idp").unwrap(),
        )
        .unwrap()
    }

    fn stated(recorder: &Arc<Recorded>) -> EnteredAdministration {
        EnteredAdministration::admitted().administered_by(
            AdministeringPrincipal::new(administrator()).unwrap(),
            Arc::clone(recorder) as Arc<dyn AdministrationRecorder>,
        )
    }

    /// An administrator names a trust domain, and its login is kept only as
    /// the login's digest.
    #[test]
    fn an_administrator_is_qualified_and_keeps_only_the_login_digest() {
        assert_eq!(
            AdministeringPrincipal::new(
                PrincipalRef::new(PrincipalKind::Human, "admin-7").unwrap()
            ),
            Err(PrincipalContractError::UnqualifiedPrincipal)
        );
        let scope = HostAuthenticationScope::new("login-1");
        let administrator_in_login = AdministeringPrincipal::new(administrator())
            .unwrap()
            .with_authentication_scope(&scope);
        assert_eq!(administrator_in_login.principal(), &administrator());
        assert_eq!(
            administrator_in_login.authentication_scope_digest(),
            Some(&scope.digest())
        );
        assert!(!format!("{administrator_in_login:?}").contains("login-1"));
    }

    /// Dropping an admission that states its administrator records it once,
    /// with the change and outcome its owner stated; an owner that stated
    /// neither leaves the outcome unknown. An admission without an
    /// administrator records nothing.
    #[test]
    fn a_stated_admission_records_once_when_dropped() {
        let recorder = Arc::new(Recorded::default());
        let change = AdministeredChange::ConfigPatch {
            realm: None,
            generation: 3,
        };
        let mut settled = stated(&recorder).for_change(change.clone());
        settled.settle(AdministrationOutcome::Committed);
        assert!(
            recorder.take().is_empty(),
            "nothing is recorded while the admission is held"
        );
        drop(settled);
        drop(stated(&recorder));
        drop(EnteredAdministration::admitted().for_change(change.clone()));

        let records = recorder.take();
        assert_eq!(records.len(), 2, "one record per stated admission");
        assert_eq!(records[0].administrator, administrator());
        assert_eq!(records[0].change, Some(change));
        assert_eq!(records[0].outcome, AdministrationOutcome::Committed);
        assert!(records[0].admitted_at <= records[0].settled_at);
        assert_eq!(records[1].change, None);
        assert_eq!(records[1].outcome, AdministrationOutcome::Unknown);
    }

    /// The host's own guard is still held while its record is delivered,
    /// and released after it.
    #[test]
    fn the_record_is_delivered_before_the_hosts_guard_is_released() {
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        struct SeesGuard(Arc<AtomicBool>, AtomicBool);
        impl AdministrationRecorder for SeesGuard {
            fn record(&self, _record: AdministrationRecord) {
                self.1
                    .store(self.0.load(Ordering::SeqCst), Ordering::SeqCst);
            }
        }
        let held = Arc::new(AtomicBool::new(true));
        let recorder = Arc::new(SeesGuard(Arc::clone(&held), AtomicBool::new(false)));
        drop(
            EnteredAdministration::holding(Guard(Arc::clone(&held))).administered_by(
                AdministeringPrincipal::new(administrator()).unwrap(),
                Arc::clone(&recorder) as Arc<dyn AdministrationRecorder>,
            ),
        );
        assert!(
            recorder.1.load(Ordering::SeqCst),
            "recorded while the guard was held"
        );
        assert!(!held.load(Ordering::SeqCst), "the guard is released after");
    }

    /// A recorder that panics does not unwind through the owner that drops
    /// the admission.
    #[test]
    fn a_panicking_recorder_does_not_unwind_through_the_owner() {
        struct Panics;
        impl AdministrationRecorder for Panics {
            fn record(&self, _record: AdministrationRecord) {
                panic!("the host's recorder failed");
            }
        }
        let dropped = std::panic::catch_unwind(|| {
            drop(EnteredAdministration::admitted().administered_by(
                AdministeringPrincipal::new(administrator()).unwrap(),
                Arc::new(Panics),
            ));
        });
        assert!(dropped.is_ok(), "the owner did not unwind");
    }

    /// A host keeps the record as it serializes: snake_case kinds and
    /// outcomes, the login only as its digest, and a client profile's slot
    /// without the server's URL.
    #[test]
    fn the_record_has_a_stable_stored_form() {
        let at = Utc::now();
        let record = AdministrationRecord {
            administrator: administrator(),
            authentication_scope_digest: Some(HostAuthenticationScope::new("login-1").digest()),
            change: Some(AdministeredChange::McpOauthClientProfileInstall {
                realm: None,
                server: "search".to_owned(),
                slot: "search-0123".to_owned(),
            }),
            outcome: AdministrationOutcome::Committed,
            admitted_at: at,
            settled_at: at,
        };
        let stored = serde_json::to_value(&record).unwrap();
        assert_eq!(
            stored["change"],
            serde_json::json!({
                "kind": "mcp_oauth_client_profile_install",
                "server": "search",
                "slot": "search-0123",
            })
        );
        assert_eq!(stored["outcome"], "committed");
        assert_eq!(
            stored["authentication_scope_digest"],
            HostAuthenticationScope::new("login-1").digest().as_str()
        );
        assert!(!stored.to_string().contains("login-1"));
        assert_eq!(
            serde_json::from_value::<AdministrationRecord>(stored).unwrap(),
            record
        );
        let secret_set = serde_json::to_value(AdministeredChange::McpServiceSecretSet {
            realm: RealmId::parse("team").unwrap(),
            server: "search".to_owned(),
            env: "SEARCH_API_KEY".to_owned(),
            meaning: "ab".repeat(32),
        })
        .unwrap();
        assert_eq!(secret_set["kind"], "mcp_service_secret_set");
        assert_eq!(secret_set["realm"], "team");

        // A config patch names its realm as a realm id, like every change.
        let config_patch = AdministeredChange::ConfigPatch {
            realm: Some(RealmId::parse("team").unwrap()),
            generation: 3,
        };
        let stored_patch = serde_json::to_value(&config_patch).unwrap();
        assert_eq!(
            stored_patch,
            serde_json::json!({ "kind": "config_patch", "realm": "team", "generation": 3 })
        );
        assert_eq!(
            serde_json::from_value::<AdministeredChange>(stored_patch).unwrap(),
            config_patch
        );
        assert!(
            serde_json::from_value::<AdministeredChange>(serde_json::json!({
                "kind": "config_patch",
                "realm": "not a realm",
                "generation": 3,
            }))
            .is_err(),
            "a stored realm that is not a realm id does not decode"
        );
    }
}
