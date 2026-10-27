//! Protected main-loop outcome handling for connectors and Forge. Operation feedback keeps the
//! admitted input alive; explicit stop and fatal terminal/cleanup errors join it.
use crate::{ConnectError, ConnectSession};
use meerkat_connect::approval::LocalApprovals;
use meerkat_runtime::AcceptOutcome;
use std::{future::Future, pin::Pin};

pub(crate) async fn continue_after_presentation<F: Future>(
    presented: Result<bool, ConnectError>, session: &ConnectSession,
    admission: &AcceptOutcome, approvals: &LocalApprovals, completion: Pin<&mut F>,
) -> Result<bool, ConnectError> {
    if matches!(presented, Ok(true)) { return Ok(true); }
    approvals.retire();
    let cancelled = session.cancel(admission).await;
    let _ = completion.await;
    match (presented, cancelled) {
        (Err(primary), Err(cleanup)) => Err(ConnectError::Cleanup { primary: Box::new(primary), cleanup: Box::new(cleanup) }),
        (Err(primary), _) => Err(primary),
        (_, Err(cleanup)) => Err(cleanup),
        _ => Ok(false),
    }
}

/// The standalone CLI keeps one generic recipe on initial and continued setup.
/// The existing explicit Reading/native application constructors are unchanged.
pub(crate) fn standalone_forge(previous: Option<meerkat_connect::forge::ForgeFrontend>)
    -> Result<(meerkat_connect::forge::ForgeFrontend, meerkat_connect::forge::ForgePresenter), ConnectError>
{
    match previous {
        None => Ok(meerkat_connect::forge::ForgeFrontend::deferred_application()),
        Some(previous) => previous.continue_application_after_close(),
    }
}
