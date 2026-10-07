//! Cold verification of the actual retained owner after local custody loss.

use super::{LocalGrantAuthority, chain};
use crate::publication::LocalAuthorizationPublication;
use std::collections::BTreeSet;
use std::sync::Mutex;

/// Reconstruction failures are infrastructure results, not permission denials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GrantReconstructionError {
    #[error("retained grant owner requires restart")]
    RestartRequired,
    #[error("replacement grant publication is unavailable")]
    ReplacementPublicationUnavailable,
}

impl LocalGrantAuthority {
    /// Verify and reconstruct this actual retained owner under a different
    /// host-selected publication. This cold path preserves every retained row,
    /// revocation, revision and the owner incarnation. It takes no decoded state
    /// and cannot reconstruct grants lost with a process or storage failure.
    ///
    /// The host must first quiesce and verify or rebuild every other owner that
    /// shares the publication, and install the same replacement for those
    /// owners. This method proves only this grant owner's retained state. Do not
    /// call it while holding a guard from either publication. It performs no
    /// clock read, permission decision, replay, external I/O or state repair.
    ///
    /// The old publication is permanently retired, even on failure and even
    /// when only the grant owner's mutex was poisoned. Existing prepared views
    /// remain invalid. On success, subsequent operations still resolve their
    /// exact retained lineage and check fresh time through the normal API.
    ///
    /// # Errors
    /// `RestartRequired` means retained state cannot be verified. It remains
    /// untouched and unavailable; the host must not reuse its old references in
    /// a new owner. `ReplacementPublicationUnavailable` means the replacement
    /// was the old instance, poisoned, retired or exhausted. A failed attempt
    /// never clears poison or publishes the retained state under that instance.
    pub fn reconstruct(
        &mut self,
        replacement: LocalAuthorizationPublication,
    ) -> Result<(), GrantReconstructionError> {
        self.publication.retire();
        if self.publication.same_instance(&replacement) {
            return Err(GrantReconstructionError::ReplacementPublicationUnavailable);
        }
        let mut publication = replacement
            .reserve_owner_change()
            .map_err(|_| GrantReconstructionError::ReplacementPublicationUnavailable)?;

        // Exclusive access permits inspection of the actual poisoned value,
        // not acceptance of an externally supplied snapshot. Never clear its
        // poison: only a completely verified replacement mutex becomes usable.
        let retained = match self.owner.get_mut() {
            Ok(owner) => owner,
            Err(poisoned) => poisoned.into_inner(),
        };
        let reconstructed = super::dsl::GrantAuthorityMachineAuthority::recover_from_state(
            retained.state().clone(),
        )
        .map_err(|_| GrantReconstructionError::RestartRequired)?;
        verify_lineages(&reconstructed)?;

        // Verification and all fallible allocation precede publication. Keep
        // the replacement writer held through installation and old-value drop.
        publication.publish();
        self.owner = Mutex::new(reconstructed);
        self.publication = replacement.clone();
        Ok(())
    }
}

fn verify_lineages(
    owner: &super::dsl::GrantAuthorityMachineAuthority,
) -> Result<(), GrantReconstructionError> {
    use super::dsl::GrantAuthorityPhase;
    let state = owner.state();
    if state.lifecycle_phase != GrantAuthorityPhase::Active {
        return Err(GrantReconstructionError::RestartRequired);
    }
    let root = state
        .root
        .as_ref()
        .ok_or(GrantReconstructionError::RestartRequired)?;
    let mut revisions = BTreeSet::new();
    for record in state.records.values() {
        if !revisions.insert(record.issued_revision) {
            return Err(GrantReconstructionError::RestartRequired);
        }
        let lineage =
            chain(owner, record).map_err(|_| GrantReconstructionError::RestartRequired)?;
        if lineage.first().map(|first| &first.issuer) != Some(root) {
            return Err(GrantReconstructionError::RestartRequired);
        }
        for pair in lineage.windows(2) {
            let parent = &pair[0];
            let child = &pair[1];
            if child.issuer != parent.grantee
                || child.represented_subject != parent.represented_subject
                || child.issued_revision <= parent.issued_revision
            {
                return Err(GrantReconstructionError::RestartRequired);
            }
            // Reuse the existing pure attenuation algebra. The retained child
            // must be a fixed point of narrowing this exact parent by that
            // child. No policy interpretation or new restriction evaluator.
            let narrowed = parent
                .restrictions
                .for_child(&child.restrictions)
                .map_err(|_| GrantReconstructionError::RestartRequired)?;
            if narrowed != child.restrictions {
                return Err(GrantReconstructionError::RestartRequired);
            }
        }
    }
    Ok(())
}
