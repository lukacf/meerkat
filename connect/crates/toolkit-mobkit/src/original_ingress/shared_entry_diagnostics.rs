//! Observation only for the existing shared-entry reservation future. No
//! authority, retry, owner task, lock, input, or persistence is created here.
use super::{InputAuthorityAssociation, OriginalWriterError};
use sha2::{Digest, Sha256};
use std::{
    cell::{Cell, RefCell},
    future::Future,
    io::Write,
};

// Each digest is SHA-256(domain UTF-8 || NUL || each field's u64 big-endian
// byte length || field bytes). Domains below are versioned and distinct.
fn digest(domain: &str, fields: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    hash.update(domain.as_bytes());
    hash.update([0]);
    for field in fields {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    format!("{:x}", hash.finalize())
}

fn parent_binding(parent: &InputAuthorityAssociation) -> Option<String> {
    let candidate = parent.candidate();
    // Canonical JSON sorts object keys recursively for offline reconstruction.
    // These two identifier projections exclude grants, ceilings, auth evidence,
    // body/context and tool arguments. A serialization failure only omits the
    // observation; it cannot change the reservation's result.
    let namespace = toolkit_package::canonical_json(&candidate.ingress_namespace).ok()?;
    let original = toolkit_package::canonical_json(&candidate.original_work).ok()?;
    Some(digest(
        "toolkit.shared-entry.parent-binding.v1",
        &[&namespace, &original],
    ))
}

#[derive(Clone)]
pub(crate) struct Correlation {
    call: String,
    acquisition: String,
    parent: String,
    parent_binding: String,
}
impl Correlation {
    pub(crate) fn new(
        call: meerkat_core::ToolCallView<'_>,
        acquisition: &toolkit_store::StoreKey,
        parent: &crate::private_originals::PrivateOriginalRef,
        association: &InputAuthorityAssociation,
    ) -> Option<Self> {
        Some(Self {
            call: digest(
                "toolkit.shared-entry.call.v1",
                &[call.id.as_bytes(), call.name.as_bytes()],
            ),
            acquisition: digest(
                "toolkit.shared-entry.acquisition.v1",
                &[
                    acquisition.realm.as_bytes(),
                    acquisition.owner.as_bytes(),
                    acquisition.id.as_bytes(),
                ],
            ),
            parent: digest(
                "toolkit.shared-entry.parent-ref.v1",
                &[
                    parent.route.as_str().as_bytes(),
                    parent.original.as_str().as_bytes(),
                    parent.event_sha256.as_bytes(),
                ],
            ),
            parent_binding: parent_binding(association)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ParentCause {
    OriginalsWouldBlock,
    OriginalsPoisoned,
    CancellationPending,
    Cancelled,
    FirstBindingConflict,
}
impl ParentCause {
    fn tag(self) -> &'static str {
        match self {
            Self::OriginalsWouldBlock => "originals_would_block",
            Self::OriginalsPoisoned => "originals_poisoned",
            Self::CancellationPending => "cancellation_pending",
            Self::Cancelled => "cancelled",
            Self::FirstBindingConflict => "first_binding_conflict",
        }
    }
}
struct Observation {
    correlation: Option<Correlation>,
    observed_parent_binding: RefCell<Option<String>>,
    parent_cause: Cell<Option<ParentCause>>,
    checking_parent: Cell<bool>,
    publish_callback_entered: Cell<bool>,
    #[cfg(test)]
    events: RefCell<Vec<serde_json::Value>>,
}
impl Observation {
    fn new(correlation: Option<Correlation>) -> Self {
        Self {
            correlation,
            observed_parent_binding: RefCell::new(None),
            parent_cause: Cell::new(None),
            checking_parent: Cell::new(false),
            publish_callback_entered: Cell::new(false),
            #[cfg(test)]
            events: RefCell::new(Vec::new()),
        }
    }
}
tokio::task_local! { static CURRENT: Observation; }

/// The exact existing future is polled here. Tokio restores the previous
/// task-local value on pending, return, cancellation and unwind. It is not
/// inherited by spawned tasks; this path directly awaits its owner calls.
pub(crate) async fn scope<T>(
    correlation: Option<Correlation>,
    future: impl Future<Output = T>,
) -> T {
    // None must mask any ambient scope, not borrow another call's identity.
    CURRENT.scope(Observation::new(correlation), future).await
}

pub(super) fn begin_followup(parent: &InputAuthorityAssociation) {
    let _ = CURRENT.try_with(|current| {
        if current.correlation.is_none() {
            return;
        }
        *current.observed_parent_binding.borrow_mut() = parent_binding(parent);
        current.parent_cause.set(None);
        current.publish_callback_entered.set(false);
    });
}

/// Fixed in-memory tag only: called at the existing decision, with no I/O,
/// new lock, retry, currentness read or change to the original guard scope.
pub(super) fn parent_refusal(cause: ParentCause) {
    let _ = CURRENT.try_with(|current| {
        if current.checking_parent.get() {
            current.parent_cause.set(Some(cause));
        }
    });
}
pub(super) fn check_parent<T>(check: impl FnOnce() -> T) -> T {
    let _ = CURRENT.try_with(|current| current.checking_parent.set(true));
    let result = check();
    let _ = CURRENT.try_with(|current| current.checking_parent.set(false));
    result
}
pub(super) fn publish_entered() {
    let _ = CURRENT.try_with(|current| current.publish_callback_entered.set(true));
}

pub(crate) fn writer_kind(error: OriginalWriterError) -> &'static str {
    match error {
        OriginalWriterError::Unavailable => "unavailable",
        OriginalWriterError::Conflict => "conflict",
        OriginalWriterError::WrongOwner => "wrong_owner",
        OriginalWriterError::UnsupportedMandate => "unsupported_mandate",
        OriginalWriterError::Grant(_) => "grant_refused",
        OriginalWriterError::OperationDenied => "operation_denied",
        OriginalWriterError::OperationCancelled => "operation_cancelled",
    }
}

/// Called only at the existing error boundaries, after the originals guard
/// returned. Failure to write diagnostic stderr must not replace the error.
pub(crate) fn emit(
    stage: &'static str,
    raw_error: Option<&'static str>,
    mapped_error: Option<&'static str>,
) {
    let event = CURRENT.try_with(|current| {
        let correlation = current.correlation.as_ref()?;
        let event = serde_json::json!({
            "event": "toolkit_shared_entry_diagnostic", "version": 1,
            "process_id": std::process::id(), "stage": stage,
            "call": correlation.call,
            "acquisition": correlation.acquisition,
            "parent": correlation.parent,
            "parent_binding": correlation.parent_binding,
            "observed_parent_binding": current.observed_parent_binding.borrow().clone(),
            "parent_cause": current.parent_cause.get().map(ParentCause::tag),
            "publish_callback_entered": current.publish_callback_entered.get(),
            "raw_error": raw_error, "mapped_error": mapped_error,
        });
        #[cfg(test)]
        current.events.borrow_mut().push(event.clone());
        Some(event)
    });
    if let Ok(Some(event)) = event {
        let _ = writeln!(std::io::stderr().lock(), "{event}");
    }
}

#[cfg(test)]
pub(crate) async fn capture<T>(
    correlation: Correlation,
    future: impl Future<Output = T>,
) -> (T, Vec<serde_json::Value>) {
    CURRENT
        .scope(Observation::new(Some(correlation)), async {
            let result = future.await;
            let events = CURRENT.with(|current| current.events.borrow().clone());
            (result, events)
        })
        .await
}

#[cfg(test)]
mod tests;
