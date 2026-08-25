//! Explicit, versioned system-prompt replacement.
//!
//! A prompt update is a typed transcript rewrite initiated by a host. Prompt
//! versions remain immutable System rows in the durable transcript, while the
//! model/compaction projection selects only the latest version for each key.
//! Nothing in session construction, restore, or materialization calls this
//! operation, so those paths cannot mint versions as a side effect.

use super::{
    Session, TranscriptEditError, TranscriptRewriteCommit, TranscriptRewriteReason,
    TranscriptRewriteSelection,
};
use crate::types::{
    Message, SessionId, SystemMessage, SystemPromptKey, SystemPromptVersion,
    SystemPromptVersionIdentity, message_timestamp_now,
};
use serde::{Deserialize, Serialize};

/// Explicit host request to replace one keyed system-prompt slot.
///
/// The first update for a key must identify an existing unversioned System row
/// with `target_message_index` and use `expected_version = None`. Later calls
/// address the slot by key and must CAS the latest typed version. Supplying the
/// target again is allowed only when it still names the latest row. The minted
/// version advances the whole retained session lineage, so it may skip after a
/// host restores an older transcript revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SystemPromptUpdateRequest {
    pub key: SystemPromptKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<SystemPromptVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_message_index: Option<usize>,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// Optional transcript-head CAS in addition to the prompt-version CAS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_parent_revision: Option<String>,
}

/// Whether an explicit prompt-update call minted a new version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SystemPromptUpdateStatus {
    Applied,
    Duplicate,
}

/// Durable result of a keyed prompt update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SystemPromptUpdateResult {
    pub session_id: SessionId,
    pub key: SystemPromptKey,
    pub version: SystemPromptVersion,
    pub message_index: usize,
    pub status: SystemPromptUpdateStatus,
    /// Current transcript revision after the operation. An exact duplicate
    /// reports the already-current revision without minting another commit.
    pub transcript_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<TranscriptRewriteCommit>,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SystemPromptUpdateError {
    #[error("system prompt key '{key}' has no current version; target_message_index is required")]
    MissingInitialTarget { key: SystemPromptKey },
    #[error("system prompt target index {index} is outside transcript length {message_count}")]
    TargetOutOfBounds { index: usize, message_count: usize },
    #[error("system prompt target index {index} does not address an ordinary System message")]
    TargetIsNotSystem { index: usize },
    #[error("system prompt target index {index} is already owned by key '{existing_key}'")]
    TargetOwnedByDifferentKey {
        index: usize,
        existing_key: SystemPromptKey,
    },
    #[error(
        "system prompt target index {index} carries append idempotency identity and cannot be adopted"
    )]
    TargetHasAppendIdentity { index: usize },
    #[error(
        "system prompt target index {index} carries instruction activation identity and cannot be adopted"
    )]
    TargetHasInstructionActivation { index: usize },
    #[error(
        "system prompt key '{key}' expected version {expected:?}, but current version is {actual:?}"
    )]
    VersionConflict {
        key: SystemPromptKey,
        expected: Option<SystemPromptVersion>,
        actual: Option<SystemPromptVersion>,
    },
    #[error(
        "system prompt key '{key}' target index {requested} does not match current version at index {actual}"
    )]
    TargetConflict {
        key: SystemPromptKey,
        requested: usize,
        actual: usize,
    },
    #[error("system prompt key '{key}' exhausted its version space")]
    VersionExhausted { key: SystemPromptKey },
    #[error("system prompt version history is malformed: {0}")]
    MalformedHistory(String),
    #[error(transparent)]
    Transcript(#[from] TranscriptEditError),
    #[error("failed to derive prompt-update transcript revision: {0}")]
    Revision(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptUpdatePredecessor {
    NotRetained,
    Unversioned,
    Versioned(SystemPromptVersion),
}

impl Session {
    fn system_prompt_update_introducing_predecessor(
        &self,
        key: &SystemPromptKey,
        current: &SystemMessage,
    ) -> Result<PromptUpdatePredecessor, SystemPromptUpdateError> {
        fn contains_exact(messages: &[Message], current: &SystemMessage) -> bool {
            messages
                .iter()
                .any(|message| matches!(message, Message::System(system) if system == current))
        }

        fn latest_version(
            messages: &[Message],
            key: &SystemPromptKey,
        ) -> Option<SystemPromptVersion> {
            messages.iter().rev().find_map(|message| {
                let Message::System(system) = message else {
                    return None;
                };
                let identity = system.prompt_version.as_ref()?;
                (&identity.key == key).then_some(identity.version)
            })
        }

        let Some(history) = self.validated_transcript_history_state()? else {
            return Ok(PromptUpdatePredecessor::NotRetained);
        };
        for commit in history.commits() {
            let parent = history.materialize_revision(&commit.parent_revision)?;
            let child = history.materialize_revision(&commit.revision)?;
            if !contains_exact(&parent.messages, current)
                && contains_exact(&child.messages, current)
            {
                return Ok(match latest_version(&parent.messages, key) {
                    Some(version) => PromptUpdatePredecessor::Versioned(version),
                    None => PromptUpdatePredecessor::Unversioned,
                });
            }
        }
        Ok(PromptUpdatePredecessor::NotRetained)
    }

    fn system_prompt_lineage_high_water(
        &self,
        key: &SystemPromptKey,
    ) -> Result<Option<SystemPromptVersion>, SystemPromptUpdateError> {
        fn observe(
            messages: &[Message],
            key: &SystemPromptKey,
            high_water: &mut Option<SystemPromptVersion>,
        ) {
            for message in messages {
                let Message::System(system) = message else {
                    continue;
                };
                let Some(identity) = system.prompt_version.as_ref() else {
                    continue;
                };
                if &identity.key == key
                    && high_water.is_none_or(|version| version < identity.version)
                {
                    *high_water = Some(identity.version);
                }
            }
        }

        let mut high_water = None;
        observe(self.messages(), key, &mut high_water);
        if let Some(history) = self.validated_transcript_history_state()? {
            for body in history.materialize_revision_bodies()? {
                observe(&body.messages, key, &mut high_water);
            }
        }
        Ok(high_water)
    }

    /// Apply one explicit keyed system-prompt replacement.
    ///
    /// Existing versions remain immutable transcript rows. A later version is
    /// inserted immediately after the current version; active materialization
    /// selects the latest row, while transcript history continues to expose
    /// every prior version. The first adoption replaces the caller-addressed
    /// unversioned row and retains its prior body through the rewrite graph.
    pub fn update_system_prompt(
        &mut self,
        request: SystemPromptUpdateRequest,
    ) -> Result<SystemPromptUpdateResult, SystemPromptUpdateError> {
        crate::types::validate_system_prompt_version_order(self.messages())
            .map_err(SystemPromptUpdateError::MalformedHistory)?;

        let current = self
            .messages()
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                let Message::System(system) = message else {
                    return None;
                };
                let identity = system.prompt_version.as_ref()?;
                (identity.key == request.key).then_some((index, system, identity.version))
            })
            .next_back();

        let (selection, replacement_index, version) =
            if let Some((current_index, current_message, current_version)) = current {
                if let Some(requested) = request.target_message_index
                    && requested != current_index
                {
                    return Err(SystemPromptUpdateError::TargetConflict {
                        key: request.key,
                        requested,
                        actual: current_index,
                    });
                }

                let exact_retry = if current_message.content == request.content {
                    let introducing_predecessor = self
                        .system_prompt_update_introducing_predecessor(
                            &request.key,
                            current_message,
                        )?;
                    let legacy_immediate_successor = matches!(
                        introducing_predecessor,
                        PromptUpdatePredecessor::NotRetained
                    ) && (request
                        .expected_version
                        .and_then(SystemPromptVersion::checked_next)
                        == Some(current_version)
                        || (request.expected_version.is_none()
                            && current_version == SystemPromptVersion::INITIAL));
                    let retained_predecessor_matches = match introducing_predecessor {
                        PromptUpdatePredecessor::NotRetained => false,
                        PromptUpdatePredecessor::Unversioned => request.expected_version.is_none(),
                        PromptUpdatePredecessor::Versioned(version) => {
                            request.expected_version == Some(version)
                        }
                    };
                    retained_predecessor_matches || legacy_immediate_successor
                } else {
                    false
                };
                if exact_retry {
                    let revision = self
                        .transcript_content_digest()
                        .map_err(|error| SystemPromptUpdateError::Revision(error.to_string()))?;
                    return Ok(SystemPromptUpdateResult {
                        session_id: self.id().clone(),
                        key: request.key,
                        version: current_version,
                        message_index: current_index,
                        status: SystemPromptUpdateStatus::Duplicate,
                        transcript_revision: revision,
                        commit: None,
                    });
                }
                if request.expected_version != Some(current_version) {
                    return Err(SystemPromptUpdateError::VersionConflict {
                        key: request.key,
                        expected: request.expected_version,
                        actual: Some(current_version),
                    });
                }
                let lineage_high_water = self.system_prompt_lineage_high_water(&request.key)?;
                let version = lineage_high_water
                    .unwrap_or(current_version)
                    .checked_next()
                    .ok_or_else(|| SystemPromptUpdateError::VersionExhausted {
                        key: request.key.clone(),
                    })?;
                let insertion = current_index.saturating_add(1);
                (
                    TranscriptRewriteSelection::MessageRange {
                        start: insertion,
                        end: insertion,
                    },
                    insertion,
                    version,
                )
            } else {
                if request.expected_version.is_some() {
                    return Err(SystemPromptUpdateError::VersionConflict {
                        key: request.key,
                        expected: request.expected_version,
                        actual: None,
                    });
                }
                let target = request.target_message_index.ok_or_else(|| {
                    SystemPromptUpdateError::MissingInitialTarget {
                        key: request.key.clone(),
                    }
                })?;
                let Some(message) = self.messages().get(target) else {
                    return Err(SystemPromptUpdateError::TargetOutOfBounds {
                        index: target,
                        message_count: self.messages().len(),
                    });
                };
                let Message::System(system) = message else {
                    return Err(SystemPromptUpdateError::TargetIsNotSystem { index: target });
                };
                if let Some(identity) = system.prompt_version.as_ref() {
                    return Err(SystemPromptUpdateError::TargetOwnedByDifferentKey {
                        index: target,
                        existing_key: identity.key.clone(),
                    });
                }
                if system.identity.is_some() {
                    return Err(SystemPromptUpdateError::TargetHasAppendIdentity { index: target });
                }
                if system.instruction_activation.is_some() {
                    return Err(SystemPromptUpdateError::TargetHasInstructionActivation {
                        index: target,
                    });
                }
                let lineage_high_water = self.system_prompt_lineage_high_water(&request.key)?;
                let version = match lineage_high_water {
                    Some(high_water) => high_water.checked_next().ok_or_else(|| {
                        SystemPromptUpdateError::VersionExhausted {
                            key: request.key.clone(),
                        }
                    })?,
                    None => SystemPromptVersion::INITIAL,
                };
                (
                    TranscriptRewriteSelection::MessageRange {
                        start: target,
                        end: target.saturating_add(1),
                    },
                    target,
                    version,
                )
            };

        let replacement = Message::System(SystemMessage {
            content: request.content,
            created_at: message_timestamp_now(),
            identity: None,
            prompt_version: Some(SystemPromptVersionIdentity {
                key: request.key.clone(),
                version,
            }),
            instruction_activation: None,
        });
        let commit = self.commit_transcript_rewrite_authorized(
            selection,
            vec![replacement],
            TranscriptRewriteReason::new(format!("system-prompt-update:{}", request.key)),
            request.actor,
            request.expected_parent_revision,
        )?;
        crate::types::validate_system_prompt_version_order(self.messages())
            .map_err(SystemPromptUpdateError::MalformedHistory)?;

        Ok(SystemPromptUpdateResult {
            session_id: self.id().clone(),
            key: request.key,
            version,
            message_index: replacement_index,
            status: SystemPromptUpdateStatus::Applied,
            transcript_revision: commit.revision.clone(),
            commit: Some(commit),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    type TestResult<T = ()> = Result<T, Box<dyn Error>>;

    fn request(
        key: &str,
        expected_version: Option<SystemPromptVersion>,
        target_message_index: Option<usize>,
        content: &str,
    ) -> TestResult<SystemPromptUpdateRequest> {
        Ok(SystemPromptUpdateRequest {
            key: SystemPromptKey::new(key)?,
            expected_version,
            target_message_index,
            content: content.to_string(),
            actor: Some("host-test".to_string()),
            expected_parent_revision: None,
        })
    }

    fn expected_error<T, E>(result: Result<T, E>, message: &'static str) -> TestResult<E>
    where
        E: Error + 'static,
    {
        match result {
            Ok(_) => Err(std::io::Error::other(message).into()),
            Err(error) => Ok(error),
        }
    }

    #[test]
    fn explicit_updates_version_and_materialization_selects_latest() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("original");
        session.push(Message::User(crate::types::UserMessage::text("hello")));

        let first =
            session.update_system_prompt(request("primary", None, Some(0), "version one")?)?;
        assert_eq!(first.version, SystemPromptVersion::INITIAL);
        let second = session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "version two",
        )?)?;
        assert_eq!(second.version.get(), 2);
        assert_eq!(session.messages().len(), 3, "both versions remain durable");

        let materialized = session.messages_for_model_boundary();
        assert_eq!(materialized.len(), 2);
        assert!(matches!(
            &materialized[0],
            Message::System(system) if system.content == "version two"
        ));
        let history = session
            .validated_transcript_history_state()?
            .ok_or_else(|| std::io::Error::other("rewrite history is missing"))?;
        assert_eq!(history.commit_count(), 2);
        Ok(())
    }

    #[test]
    fn boot_round_trip_never_mints_prompt_versions() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("ordinary prompt");
        let before = session.transcript_content_digest()?;
        let bytes = serde_json::to_vec(&session)?;
        let restored: Session = serde_json::from_slice(&bytes)?;
        assert_eq!(restored.transcript_content_digest()?, before);
        assert!(matches!(
            &restored.messages()[0],
            Message::System(system) if system.prompt_version.is_none()
        ));
        Ok(())
    }

    #[test]
    fn stale_cas_retry_is_duplicate_but_current_cas_mints_a_version() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("ordinary prompt");
        session.update_system_prompt(request("primary", None, Some(0), "replacement")?)?;

        let second = session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "replacement",
        )?)?;
        assert_eq!(second.status, SystemPromptUpdateStatus::Applied);
        assert_eq!(second.version.get(), 2);

        let retry = session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "replacement",
        )?)?;
        assert_eq!(retry.status, SystemPromptUpdateStatus::Duplicate);
        assert_eq!(retry.version, second.version);
        assert!(retry.commit.is_none());
        Ok(())
    }

    #[test]
    fn initial_adoption_cannot_erase_append_idempotency_identity() -> TestResult {
        let mut session = Session::new();
        session.append_system_message_idempotent(
            "ordinary context",
            Some("host".to_string()),
            Some("append-1".to_string()),
            message_timestamp_now(),
        )?;
        let update = request("primary", None, Some(0), "replacement")?;
        let error = expected_error(
            session.update_system_prompt(update),
            "identity-bearing append was adopted",
        )?;
        assert!(matches!(
            error,
            SystemPromptUpdateError::TargetHasAppendIdentity { index: 0 }
        ));
        Ok(())
    }

    #[test]
    fn generic_rewrite_cannot_mint_a_prompt_version() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("ordinary prompt");
        let forged = Message::System(SystemMessage {
            content: "forged version".to_string(),
            created_at: message_timestamp_now(),
            identity: None,
            prompt_version: Some(SystemPromptVersionIdentity {
                key: SystemPromptKey::new("primary")?,
                version: SystemPromptVersion::INITIAL,
            }),
            instruction_activation: None,
        });

        let error = expected_error(
            session.commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                vec![forged],
                TranscriptRewriteReason::new("generic-forgery"),
                Some("host-test".to_string()),
                None,
            ),
            "generic rewrite minted a prompt version",
        )?;
        assert!(matches!(
            error,
            TranscriptEditError::InvalidTranscriptShape(detail)
                if detail.contains("cannot mint or alter system prompt")
        ));
        Ok(())
    }

    #[test]
    fn successful_generic_rewrite_invalidates_authored_cache_evidence() -> TestResult {
        let mut session = Session::new();
        session.push(Message::User(crate::types::UserMessage::text("before")));
        session.set_metadata_unchecked_for_test(
            crate::session::SESSION_AUTHORED_CACHE_BREAKPOINTS_KEY,
            serde_json::json!([{"stale": true}]),
        );

        session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            vec![Message::User(crate::types::UserMessage::text("after"))],
            TranscriptRewriteReason::new("cache-invalidation"),
            Some("host-test".to_string()),
            None,
        )?;
        assert!(
            !session
                .metadata()
                .contains_key(crate::session::SESSION_AUTHORED_CACHE_BREAKPOINTS_KEY)
        );
        Ok(())
    }

    #[test]
    fn ordinary_append_cannot_mint_a_prompt_version() -> TestResult {
        let mut session = Session::new();
        let key = SystemPromptKey::new("primary")?;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            session.push(Message::System(SystemMessage {
                content: "forged version".to_string(),
                created_at: message_timestamp_now(),
                identity: None,
                prompt_version: Some(SystemPromptVersionIdentity {
                    key,
                    version: SystemPromptVersion::INITIAL,
                }),
                instruction_activation: None,
            }));
        }));
        let payload = match outcome {
            Ok(()) => {
                return Err(std::io::Error::other(
                    "ordinary Session append accepted a system prompt version",
                )
                .into());
            }
            Err(payload) => payload,
        };
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
        assert_eq!(
            message,
            Some(
                "ordinary Session append cannot mint a system prompt version; use update_system_prompt"
            )
        );
        Ok(())
    }

    #[test]
    fn restored_older_prompt_version_does_not_reuse_lineage_version() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("ordinary prompt");
        session.update_system_prompt(request("primary", None, Some(0), "version one")?)?;
        let version_one = session.messages()[0].clone();
        session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "version two",
        )?)?;
        session.commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![version_one],
            TranscriptRewriteReason::new("test-restore-version-one"),
            Some("host-test".to_string()),
            None,
        )?;

        let result = session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "alternate successor",
        )?)?;
        assert_eq!(result.version.get(), 3);

        let retry = session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "alternate successor",
        )?)?;
        assert_eq!(retry.status, SystemPromptUpdateStatus::Duplicate);
        assert_eq!(retry.version, result.version);
        assert!(retry.commit.is_none());
        Ok(())
    }

    #[test]
    fn same_content_from_another_predecessor_is_not_a_duplicate_retry() -> TestResult {
        let mut session = Session::new();
        session.append_system_message("ordinary prompt");
        session.update_system_prompt(request("primary", None, Some(0), "version one")?)?;
        session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "shared content",
        )?)?;
        session.update_system_prompt(request(
            "primary",
            Some(SystemPromptVersion::new(2)?),
            None,
            "shared content",
        )?)?;

        let update = request(
            "primary",
            Some(SystemPromptVersion::INITIAL),
            None,
            "shared content",
        )?;
        let error = expected_error(
            session.update_system_prompt(update),
            "stale same-content request was accepted as an introducing operation",
        )?;
        assert!(matches!(
            error,
            SystemPromptUpdateError::VersionConflict {
                expected: Some(version),
                actual: Some(actual),
                ..
            } if version == SystemPromptVersion::INITIAL && actual.get() == 3
        ));
        Ok(())
    }
}
