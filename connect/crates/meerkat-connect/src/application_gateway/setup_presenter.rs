//! In-process enrollment in the gateway's one terminal presenter.

use std::{path::PathBuf, sync::{Arc, OnceLock}};

use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

use crate::installed_service_review::{InstalledServiceReview, account_request::{InstalledAccountRequest, InstalledAccountRequests, InstalledRequest, public::InstalledPublicRequest}};
use crate::{ConnectError, approval::{ApprovalPresenter, ApprovalPrompt, LocalApprovals}};

// Resource bounds only. Neither capacity nor enrollment is native permission.
const MAX_PRESENTERS: usize = 8;

/// Fresh review owners for the explicitly transferred terminal. Obtain this
/// handle from `InheritedOperatorTerminal::setup_reviews`, then retain it only
/// in the selected application's private composition. It has no wire codec.
/// Native construction must bind each returned owner to its actual session.
#[derive(Clone)]
pub struct LocalSetupApprovals {
    artifact_parent: PathBuf,
    sender: mpsc::Sender<Enrollment>,
    capacity: Arc<Semaphore>,
    console: Option<Arc<OnceLock<super::console_setup::ConsoleSetup>>>,
}

/// Setup presenters consumed by the same loop as local controller reviews.
/// Remote controller reviews keep their existing private presenter and wire.
pub struct LocalSetupReviews(mpsc::Receiver<Enrollment>);

impl LocalSetupApprovals {
    /// Configuration-time check of this exact enrollment receiver. Construction
    /// and every entered review still enforce their own current custody.
    pub(crate) fn check_open(&self) -> Result<(), ConnectError> {
        if self.sender.is_closed() { Err(unavailable()) } else { Ok(()) }
    }

    pub(crate) fn with_console(mut self, console: super::console_setup::ConsoleSetup) -> Self {
        self.console = Some(Arc::new(OnceLock::from(console))); self
    }

    /// The same enrollment owner is constructed before native held members.
    /// The final private host supplies its actual Console binding once, before
    /// any member can publish. No prompt may select a different presenter.
    pub(crate) fn with_pending_console(mut self) -> Self {
        self.console = Some(Arc::new(OnceLock::new())); self
    }

    pub(crate) fn adopt_console(&self, console: super::console_setup::ConsoleSetup)
        -> Result<(), ConnectError>
    {
        self.console.as_ref().ok_or_else(unavailable)?.set(console).map_err(|_| unavailable())
    }
    pub(crate) fn has_console(&self) -> bool { self.console.is_some() }

    fn approval_pair(&self) -> Result<(LocalApprovals, ApprovalPresenter), ConnectError> {
        if self.console.is_some() { LocalApprovals::create_console_setup(&self.artifact_parent) }
        else { LocalApprovals::create(&self.artifact_parent) }
    }

    /// Create the existing approval owner and its sole presenter together.
    /// Each construction gets a new owner, including a same-session rebuild.
    /// This never rebinds or retires a prior actor's owner. A busy or closed
    /// terminal queue refuses; callers must not select another presenter.
    ///
    /// Success only transfers presentation custody. It does not establish
    /// session binding, native build success, publication or readiness. The
    /// caller still owns entered account work and the native actor's close/join.
    pub fn create(&self) -> Result<Arc<LocalApprovals>, ConnectError> {
        if self.sender.is_closed() {
            return Err(unavailable());
        }
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| unavailable())?;
        let (approvals, presenter) = self.approval_pair()?;
        let approvals = Arc::new(approvals);
        // Failed transfer drops the actual presenter and retires this same
        // owner before returning. No session or decision has been invented.
        self.sender.try_send(Enrollment { console: self.console.clone(), presenter: Some(presenter), accounts: None, _permit: permit })
            .map_err(|_| unavailable())?;
        Ok(approvals)
    }
    /// Transfer the same account owner and sole presenter from native adapter
    /// construction. No effect future or native preparation receipt is sent.
    pub(crate) fn create_installed(
        &self,
        build: impl FnOnce(Arc<LocalApprovals>) -> Result<InstalledServiceReview, ConnectError>,
    ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, ConnectError> {
        self.create_installed_with_tasks(None, build)
    }

    pub(crate) fn create_installed_with_tasks(
        &self,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
        build: impl FnOnce(Arc<LocalApprovals>) -> Result<InstalledServiceReview, ConnectError>,
    ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, ConnectError> {
        self.create_installed_selected(tasks, None, build)
    }

    pub(crate) fn create_installed_selected(
        &self,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
        selected: Option<Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>>,
        build: impl FnOnce(Arc<LocalApprovals>) -> Result<InstalledServiceReview, ConnectError>,
    ) -> Result<Arc<dyn meerkat_core::AgentToolDispatcher>, ConnectError> {
        if self.sender.is_closed() { return Err(unavailable()); }
        let permit = self.capacity.clone().try_acquire_owned().map_err(|_| unavailable())?;
        let (approvals, presenter) = match &selected {
            Some(selected) => {
                let (approvals, presenter) = LocalApprovals::create_for_selected_policy(
                    &self.artifact_parent, selected.clone(), Some(self.console.is_some()))?;
                (approvals, presenter.ok_or_else(unavailable)?)
            }
            None => self.approval_pair()?,
        };
        let review = build(Arc::new(approvals.with_service_tasks(tasks.clone())))?;
        let (dispatcher, accounts) = match selected {
            Some(selected) => InstalledAccountRequests::with_selected_policy(review, tasks, selected),
            None => InstalledAccountRequests::with_tasks(review, tasks),
        };
        self.sender.try_send(Enrollment { console: self.console.clone(), presenter: Some(presenter), accounts: Some(accounts), _permit: permit })
            .map_err(|_| unavailable())?;
        Ok(dispatcher)
    }

}

fn unavailable() -> ConnectError {
    ConnectError::InvalidInput("The selected terminal setup presenter is unavailable".into())
}

pub(crate) fn channel(artifact_parent: PathBuf) -> (LocalSetupApprovals, LocalSetupReviews) {
    let (sender, receiver) = mpsc::channel(1);
    (LocalSetupApprovals {
        artifact_parent,
        sender,
        capacity: Arc::new(Semaphore::new(MAX_PRESENTERS)),
        console: None,
    }, LocalSetupReviews(receiver))
}

pub(super) struct Enrollment {
    console: Option<Arc<OnceLock<super::console_setup::ConsoleSetup>>>,
    pub(super) presenter: Option<ApprovalPresenter>,
    pub(super) accounts: Option<InstalledAccountRequests>,
    _permit: OwnedSemaphorePermit,
}

impl Enrollment {
    pub(super) fn console(&self) -> Option<super::console_setup::ConsoleSetup> {
        self.console.as_ref().and_then(|slot| slot.get()).cloned()
    }
}

pub(super) enum SetupEvent {
    Approval(ApprovalPrompt),
    Account(InstalledAccountRequest),
    Public(InstalledPublicRequest),
    Credentials(crate::installed_service_review::account_request::credentials::InstalledCredentialRequest),
}
type NextPrompt = BoxFuture<'static, (Enrollment, Option<SetupEvent>)>;

/// Waiting on exact existing receivers needs no task, reader, identity map or
/// lifecycle registry. Dropping the set drops those presenters, which retires
/// unanswered attempts through their existing LocalApprovals owner.
pub(super) struct SetupPresenters {
    incoming: Option<mpsc::Receiver<Enrollment>>,
    waiting: FuturesUnordered<NextPrompt>,
}

impl SetupPresenters {
    pub(super) fn new(reviews: LocalSetupReviews) -> Self {
        Self { incoming: Some(reviews.0), waiting: FuturesUnordered::new() }
    }

    pub(super) fn resume(&mut self, mut enrollment: Enrollment) {
        if enrollment.presenter.is_none() { return; }
        self.waiting.push(Box::pin(async move {
            let event = if let Some(accounts) = enrollment.accounts.as_mut() {
                tokio::select! { biased;
                    prompt = enrollment.presenter.as_mut().expect("enrolled presenter").next() => prompt.map(SetupEvent::Approval),
                    request = accounts.next_request() => request.map(|request| match request {
                        InstalledRequest::Account(request) => SetupEvent::Account(request),
                        InstalledRequest::Public(request) => SetupEvent::Public(request),
                        InstalledRequest::Credentials(request) => SetupEvent::Credentials(request),
                    }),
                }
            } else { enrollment.presenter.as_mut().expect("enrolled presenter").next().await.map(SetupEvent::Approval) };
            (enrollment, event)
        }));
    }

    pub(super) async fn next(&mut self) -> Option<(Enrollment, SetupEvent)> {
        loop {
            if self.incoming.is_none() && self.waiting.is_empty() {
                return None;
            }
            tokio::select! {
                result = self.waiting.next(), if !self.waiting.is_empty() => {
                    if let Some((enrollment, Some(prompt))) = result {
                        return Some((enrollment, prompt));
                    }
                    // A retired owner is released, never reused for a rebuild.
                }
                enrollment = async { self.incoming.as_mut().expect("guarded receiver").recv().await }, if self.incoming.is_some() => {
                    match enrollment {
                        Some(enrollment) => self.resume(enrollment),
                        None => self.incoming = None,
                    }
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
