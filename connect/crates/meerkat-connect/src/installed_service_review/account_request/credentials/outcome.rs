//! Payload-free observations of the actual native persistence sequence.

use meerkat_core::{McpServerConfig, RealmId};
use serde::Serialize;

/// Read from the trusted defining-realm owner and checked installed job before
/// the existing approval owner records the proposal. This is not permission.
#[derive(Clone, Serialize)]
pub(crate) struct CredentialOwnerObservation {
    pub(crate) defining_realm: RealmId,
    pub(crate) configuration_generation: u64,
    pub(crate) configuration: McpServerConfig,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WriteState {
    NotEntered,
    Installed,
    Stored,
    NotWritten,
    NotAdmitted,
    OutcomeUnknown,
}
#[derive(Serialize)]
pub(super) struct InstallOutcome {
    pub(super) state: WriteState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) generation: Option<u64>,
}
#[derive(Serialize)]
pub(super) struct SecretOutcome {
    pub(super) slot: String,
    pub(super) state: WriteState,
}
#[derive(Serialize)]
pub(super) struct PersistenceOutcome {
    pub(super) defining_realm: RealmId,
    pub(super) server_name: String,
    pub(super) reviewed_configuration_generation: u64,
    pub(super) install: InstallOutcome,
    pub(super) secrets: Vec<SecretOutcome>,
}
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CredentialState { Reviewed, Denied, Incomplete }

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PublicationState { NotEntered, Retained, NotRetained, OutcomeUnknown }

/// No value, native error text, request body or credential-store reference is
/// projected. An installed definition is distinct from all credentials stored,
/// and neither proves provider authentication, attachment or business success.
#[derive(Serialize)]
pub(super) struct CredentialReply {
    pub(super) state: CredentialState,
    pub(super) access_reviewed: bool,
    pub(super) persistence: PersistenceOutcome,
    pub(super) grant_issued: bool,
    pub(super) fresh_instruction_retained: bool,
    pub(super) ready: bool,
    pub(super) task_publication: PublicationState,
}
impl CredentialReply {
    pub(super) fn pending(observed: &CredentialOwnerObservation, slots: impl Iterator<Item = String>) -> Self {
        Self {
            state: CredentialState::Incomplete,
            access_reviewed: false,
            persistence: PersistenceOutcome {
                defining_realm: observed.defining_realm.clone(),
                server_name: observed.configuration.name.clone(),
                reviewed_configuration_generation: observed.configuration_generation,
                install: InstallOutcome { state: WriteState::NotEntered, generation: None },
                secrets: slots.map(|slot| SecretOutcome { slot, state: WriteState::NotEntered }).collect(),
            },
            grant_issued: false,
            fresh_instruction_retained: false,
            ready: false,
            task_publication: PublicationState::NotEntered,
        }
    }
}
