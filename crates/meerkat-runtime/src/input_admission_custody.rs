//! Process-local custody joined to the existing native input admission task.
//!
//! This carries a caller's real resource owner across the credential handoff.
//! It does not authorize an input or change generated admission decisions.

/// The disposition of one attempted native input admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeInputAdmissionSettlement {
    /// No admission occurred. The owner may restore its reserved resources.
    NotAdmitted,
    /// The native owner accepted the input, or verified an exact prompt replay.
    /// Reserved input content must not become available to another submission.
    Admitted,
    /// Admission may have mutated or committed before its result became
    /// unavailable. Keep the real resource owner fail-closed; do not restore it.
    Uncertain,
}

/// One-shot cleanup using only the existing generated completion observation.
///
/// Native installs the existing resultful cleanup relay on its actual handle
/// before wake or return. This callback cannot replace that handle or delivery.
/// Its future runs after completion, outside native guards. Dropping it must
/// not restore already-admitted resources.
pub type NativeInputAdmissionCompletion = Box<
    dyn FnOnce(
            Result<
                crate::completion::CompletionCleanupObservation,
                crate::completion::CompletionWaitError,
            >,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), crate::completion::CompletionWaitError>>
                    + Send,
            >,
        > + Send,
>;

/// One-shot custody supplied by an existing surface resource owner.
///
/// `settle` must be synchronous, infallible, and local. It may move ownership
/// into the owner's existing input entry, but must not await, perform I/O,
/// acquire native/publication custody, or make an authorization decision.
/// Native admission can call it while holding its driver and mutation guards.
/// Any asynchronous cleanup belongs to the supplied owner after this handoff.
/// The boxed object must not independently restore resources on Drop: the
/// native guard calls `settle` exactly once, including a never-polled future.
pub trait NativeInputAdmissionCustody: Send {
    /// Only `Admitted` may return a completion relay. If no handle is available
    /// after an admission error, the resource owner must remain fail-closed.
    fn settle(
        self: Box<Self>,
        settlement: NativeInputAdmissionSettlement,
    ) -> Option<NativeInputAdmissionCompletion>;
}

pub(crate) struct NativeInputAdmissionGuard {
    custody: Option<Box<dyn NativeInputAdmissionCustody>>,
    settlement: NativeInputAdmissionSettlement,
    completion: Option<NativeInputAdmissionCompletion>,
}

impl NativeInputAdmissionGuard {
    pub(crate) fn new(custody: Box<dyn NativeInputAdmissionCustody>) -> Self {
        Self {
            custody: Some(custody),
            settlement: NativeInputAdmissionSettlement::NotAdmitted,
            completion: None,
        }
    }

    /// Once the driver mutation is attempted, an error or unwind alone cannot
    /// prove that a durable write did not commit.
    pub(crate) fn begin_admission_attempt(&mut self) {
        self.settlement = NativeInputAdmissionSettlement::Uncertain;
    }

    pub(crate) fn settle(&mut self, settlement: NativeInputAdmissionSettlement) {
        self.settlement = settlement;
        if let Some(custody) = self.custody.take() {
            let completion = custody.settle(settlement);
            if settlement == NativeInputAdmissionSettlement::Admitted {
                self.completion = completion;
            }
        }
    }

    pub(crate) fn bind_completion(
        &mut self,
        handle: crate::completion::CompletionHandle,
    ) -> crate::completion::CompletionHandle {
        match self.completion.take() {
            Some(completion) => handle.with_resultful_completion_cleanup(completion),
            None => handle,
        }
    }
}

impl Drop for NativeInputAdmissionGuard {
    fn drop(&mut self) {
        if let Some(custody) = self.custody.take() {
            let _ = custody.settle(self.settlement);
        }
    }
}
