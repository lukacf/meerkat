//! Disposable human-only delivery of validated PairedHost entry and review links.
//! This is output custody, not authentication, approval or effect authority.

use std::io::Write;

use tokio::sync::mpsc;

use crate::{
    ConnectError,
    paired_host::{ExtensionSecretIntent, FrontendChangeDigest},
    terminal::review_text,
};

/// Intentionally neither serializable nor cloneable. Its Debug representation
/// never includes the private entry path or the selected host. Render only
/// after the originating native completion has been joined, so output failure
/// cannot interrupt a successful effect's settlement.
pub struct SecretEntryNotice {
    kind: NoticeKind,
    url: String,
}

enum NoticeKind {
    SecretEntry {
        intent_id: String,
    },
    PreparedReview {
        change_digest: String,
        candidate_kind: &'static str,
    },
}

/// Private output data only. Serialization is restricted to the protected
/// presenter after the originating native input has joined.
#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum PrivateNotice {
    SecretEntry {
        intent_id: String,
        url: String,
    },
    Review {
        change_digest: String,
        candidate_kind: &'static str,
        url: String,
    },
}

impl std::fmt::Debug for SecretEntryNotice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretEntryNotice").finish_non_exhaustive()
    }
}

/// The sole terminal receiver for secret-entry and prepared-review notices.
/// Dropping it does not cancel an intent or discard a host draft.
/// Join native completion before rendering notices. This output channel does
/// not itself enforce native lifecycle ordering for an embedding presenter.
pub struct SecretEntryPresenter {
    notices: mpsc::Receiver<SecretEntryNotice>,
    reviews: mpsc::Receiver<SecretEntryNotice>,
}

pub(crate) fn channel() -> (
    mpsc::Sender<SecretEntryNotice>,
    mpsc::Sender<SecretEntryNotice>,
    SecretEntryPresenter,
) {
    let (sender, notices) = mpsc::channel(1);
    // A prepared draft never occupies the slot needed for secret entry.
    let (review_sender, reviews) = mpsc::channel(1);
    (
        sender,
        review_sender,
        SecretEntryPresenter { notices, reviews },
    )
}

impl SecretEntryNotice {
    pub(crate) fn into_private(self) -> PrivateNotice {
        match self.kind {
            NoticeKind::SecretEntry { intent_id } => PrivateNotice::SecretEntry {
                intent_id,
                url: self.url,
            },
            NoticeKind::PreparedReview {
                change_digest,
                candidate_kind,
            } => PrivateNotice::Review {
                change_digest,
                candidate_kind,
                url: self.url,
            },
        }
    }
    pub(crate) fn captured(origin: &str, intent: &ExtensionSecretIntent) -> Self {
        Self {
            kind: NoticeKind::SecretEntry {
                intent_id: intent.intent_id.as_str().into(),
            },
            // Both pieces come from the captured client and its strict response
            // validation, never a model-authored URL or host target label.
            url: format!("{origin}{}", intent.entry_path_for_presenter()),
        }
    }

    pub(crate) fn prepared_review(
        origin: &str,
        path: &str,
        change_digest: &FrontendChangeDigest,
        candidate_kind: &'static str,
    ) -> Self {
        Self {
            kind: NoticeKind::PreparedReview {
                change_digest: change_digest.as_str().into(),
                candidate_kind,
            },
            // The client validated the actual returned path against this digest.
            // Never derive a path when the host did not supply one.
            url: format!("{origin}{path}"),
        }
    }

    /// The caller must first join the originating native completion. A display
    /// failure does not undo preparation/creation or authorize a new submission
    /// or a replacement intent. The host rechecks the change's current status.
    pub fn present(self, output: &mut impl Write) -> Result<(), ConnectError> {
        match self.kind {
            NoticeKind::SecretEntry { intent_id } => writeln!(output,
            "PairedHost secret entry\nIntent: {}\nOpen this link in your browser. Pair that browser with PairedHost independently. Never enter the secret in this conversation.\n{}\nEntry and owner confirmation happen in PairedHost. This link does not prove the secret was committed or the selected extension roles are ready.",
            review_text(&intent_id), review_text(&self.url))
            .and_then(|_| output.flush())
            .map_err(|_| ConnectError::InvalidInput(format!(
                "PairedHost intent {} exists, but its entry link could not be displayed. Do not create it again; explicitly request this intent's status and entry link when terminal output is available.",
                intent_id))),
            NoticeKind::PreparedReview { change_digest, candidate_kind } => writeln!(output,
                "PairedHost change review\nKind: {}\nChange: {}\n{}\nPair your browser with PairedHost if prompted. Check PairedHost for this change's current review and activation status.",
                candidate_kind, review_text(&change_digest), review_text(&self.url))
                .and_then(|_| output.flush())
                .map_err(|_| ConnectError::InvalidInput(format!(
                    "The review link for PairedHost change {} could not be displayed.",
                    change_digest))),
        }
    }
}

impl SecretEntryPresenter {
    /// Atomically move the two bounded queue slots after the sole originating
    /// send joins. The private-pipe driver fences its next send before calling
    /// this method and retains the original send ID with the moved objects.
    pub(crate) fn take_pending(&mut self) -> [Option<SecretEntryNotice>; 2] {
        [self.notices.try_recv().ok(), self.reviews.try_recv().ok()]
    }

    /// Receive private output for a human only. Join the originating native
    /// completion before rendering the returned notice; never forward it to
    /// model messages, tools, native events or approval/audit records.
    pub async fn next(&mut self) -> Option<SecretEntryNotice> {
        loop {
            let notice = tokio::select! {
                biased;
                notice = self.notices.recv(), if !self.notices.is_closed() || !self.notices.is_empty() => notice,
                review = self.reviews.recv(), if !self.reviews.is_closed() || !self.reviews.is_empty() => review,
                else => return None,
            };
            if notice.is_some() {
                return notice;
            }
        }
    }

    /// Call only after joining native completion, before rendering the final
    /// answer or leaving the conversation. Output never gates host settlement.
    pub fn present_pending(&mut self, output: &mut impl Write) -> Result<(), ConnectError> {
        while let Ok(notice) = self.notices.try_recv() {
            notice.present(output)?;
        }
        while let Ok(review) = self.reviews.try_recv() {
            review.present(output)?;
        }
        Ok(())
    }
}
