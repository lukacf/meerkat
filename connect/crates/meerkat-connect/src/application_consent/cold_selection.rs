//! Trusted explicit abandonment selection. No wire request or failed successful
//! close can construct this intent, and it confers no native/physical proof.
use super::*;
use std::time::Duration;

#[derive(Clone)]
pub struct AbandonedWorkflowRecovery {
    associations: Vec<SelectedHostArtifact>,
    native_budget: Duration,
}
impl AbandonedWorkflowRecovery {
    pub fn new(
        associations: Vec<SelectedHostArtifact>,
        native_budget: Duration,
    ) -> Result<Self, ConsentOwnerError> {
        if associations.is_empty()
            || native_budget.is_zero()
            || meerkat_core::time_compat::Instant::now()
                .checked_add(native_budget)
                .is_none()
            || associations.iter().enumerate().any(|(index, artifact)| {
                !artifact.path.is_absolute()
                    || artifact.sha256.len() != 64
                    || !artifact
                        .sha256
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    || associations[..index]
                        .iter()
                        .any(|prior| prior == artifact || prior.path == artifact.path)
            })
        {
            return Err(ConsentOwnerError::InvalidSelection);
        }
        Ok(Self {
            associations,
            native_budget,
        })
    }
    pub(crate) fn selects(&self, artifact: &SelectedHostArtifact) -> bool {
        self.associations.contains(artifact)
    }
    pub(crate) fn native_budget(&self) -> Duration {
        self.native_budget
    }
}
