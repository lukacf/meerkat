//! Mechanical composition of the same retained contributor login owners.
use crate::private_host::ConsoleLoginHandle;
use meerkat_mobkit::identity_first::{
    GovernedEntryFence, GovernedEntryOutcome, GovernedEntryPermit, GovernedEntryRefusal,
};
use std::sync::Arc;

/// The existing helper contributor collector has already selected and
/// deduplicated actual handles. No login labels or authority are resolved here.
pub(super) fn for_logins(logins: Vec<ConsoleLoginHandle>) -> Option<Arc<dyn GovernedEntryFence>> {
    (!logins.is_empty())
        .then(|| Arc::new(ContributorLogins { logins }) as Arc<dyn GovernedEntryFence>)
}

struct ContributorLogins {
    logins: Vec<ConsoleLoginHandle>,
}

impl GovernedEntryFence for ContributorLogins {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        let mut permits: Vec<Box<dyn GovernedEntryPermit>> = Vec::with_capacity(self.logins.len());
        for login in &self.logins {
            match GovernedEntryFence::enter(login) {
                Ok(permit) => permits.push(permit),
                Err(refusal) => {
                    // Native has not received this input. Earlier reservations
                    // are known unentered, not abandoned uncertain admissions.
                    for permit in permits {
                        permit.finish(GovernedEntryOutcome::Unentered);
                    }
                    return Err(refusal);
                }
            }
        }
        Ok(Box::new(ContributorPermit { permits }))
    }
}

struct ContributorPermit {
    permits: Vec<Box<dyn GovernedEntryPermit>>,
}

impl GovernedEntryPermit for ContributorPermit {
    fn finish(self: Box<Self>, outcome: GovernedEntryOutcome) {
        for permit in self.permits {
            permit.finish(outcome);
        }
    }
}
// Dropping an unreported aggregate drops each existing login reservation. Its
// owner records uncertainty and drains the count; source/native custody stays
// with HelperSettlement and SubmitHelperBinder::retain_unresolved.

#[cfg(test)]
mod tests;
