//! The seam's calls into Core's change path: a create of 1 to 256 agents,
//! and the changes of one agent as a whole (its archive, its unarchive and
//! its removal). Each is one call of the mob's handle.

use meerkat_mob::{
    CreatedIdentityObservation, CreatedIdentityRequest, CreatedIdentitySpec,
    IdentityAddressability, IdentityArchiveChange, IdentityChangeOutcome,
    IdentityChangeRequesterInput, IdentityGrantRequest, IdentityOperation, IdentityPreparationMode,
    IdentityRemoval, IdentitySourceEvidence, OwnedEdgeAdmission, OwnedIdentityEdge,
    StoreOwnedIdentityCreate,
};

use crate::identity_first::AgentAddressability;

use super::super::config::PreparationMode;
use super::super::digest::Sha256Digest;
use super::super::policy::PreparationRule;
use super::super::refusal::{IdentityRefusal, IdentityRefusalKind, RequestProblem};
use super::super::request::{CreatedSpec, EdgeBasis, core_member_name};
use super::{
    AgentAdmission, ChangeCommit, ChangeCommitted, ChangeRequester, CoreChangeExecutor,
    ExecutorError, committed_receipt, core_name, outcome_of_change, outcome_of_receipt,
    refusal_of_error,
};

/// A change of one agent as a whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EntryChange {
    Archive,
    Unarchive,
    Remove,
}

/// The spec `spec` in the form an agent's record holds it: field for field,
/// under the name Core's mob holds the agent by. `None` for a name the
/// encoding does not admit. Its digest is the one spec digest every surface
/// names, before and after the agent's adoption.
pub(crate) fn row_spec(
    spec: &crate::identity_first::DurableAgentSpec,
) -> Option<CreatedIdentitySpec> {
    let member = super::core_member_name(spec.identity.as_str())?;
    let mut row = CreatedIdentitySpec::new(member, spec.profile.clone())
        .with_labels(spec.labels.clone())
        .with_additional_instructions(spec.additional_instructions.clone());
    row.addressability = match spec.addressability {
        AgentAddressability::Addressable => IdentityAddressability::Addressable,
        AgentAddressability::InternalOnly => IdentityAddressability::InternalOnly,
    };
    row.display_name = spec
        .display_name
        .as_ref()
        .map(|name| name.as_str().to_owned());
    row.context = spec.context.clone();
    row.initial_message = spec.initial_message.clone();
    row.runtime_mode_override = spec.runtime_mode_override;
    row.backend = spec.backend;
    row.binding = spec.binding.clone();
    row.placement = spec.placement.clone();
    Some(row)
}

/// What a step of a commit answers: its value, or the typed refusal the
/// commit ends with.
type Step<T> = Result<Result<T, IdentityRefusal>, ExecutorError>;

/// The answer of a change Core committed as the commit's operation.
pub(super) fn committed(
    commit: &ChangeCommit<'_>,
    change: &IdentityChangeOutcome,
) -> Result<ChangeCommitted, ExecutorError> {
    outcome_of_change(commit.mob, commit.operation_id, change)
}

impl CoreChangeExecutor {
    /// What a commit answers when Core's call of the change erred. The
    /// bodies of every change answer a Core error through here.
    ///
    /// Core's own refusal of the change says nothing of it was written: it
    /// is the typed refusal, with the request field it concerns. Any other
    /// error may follow Core's commit (Core commits a removal, an archive
    /// and an unarchive, then retires or seats the member, which can fail
    /// on its own), so Core's receipt is read first: a receipt of this
    /// request means the change committed, and it is answered as
    /// committed, with what the receipt says is still to end (`retiring`,
    /// `archiving`); the instruction step is told of a removal that
    /// committed this way. A receipt of another request under the
    /// operation id is `operation_reused`, as Core answers this request at
    /// its commit: this request committed nothing, and the step is told
    /// nothing. Without a receipt a member step in flight is the retryable
    /// `admission_unsettled`, and anything else is the error it was. A
    /// receipt that cannot be read leaves the commit with no outcome.
    pub async fn after_core_error(
        &self,
        commit: &ChangeCommit<'_>,
        error: meerkat_mob::MobError,
    ) -> Result<ChangeCommitted, ExecutorError> {
        // The name Core found taken, to name the agent of the request that
        // asked for it.
        let taken = match &error {
            meerkat_mob::MobError::IdentityChangeRefused {
                refusal: meerkat_mob::IdentityChangeRefusal::NameTaken { identity, .. },
            } => Some(identity.clone()),
            _ => None,
        };
        if matches!(error, meerkat_mob::MobError::IdentityChangeRefused { .. }) {
            let refusal = at_taken_name(commit, refusal_of_error(error)?, taken.as_ref());
            return Ok(ChangeCommitted::refused(at_request_field(commit, refusal)));
        }
        let Ok(receipt) = committed_receipt(commit.mob, commit.operation_id).await else {
            return Err(ExecutorError::Core(Box::new(error)));
        };
        let Some(receipt) = receipt else {
            let refusal = refusal_of_error(error)?;
            return Ok(ChangeCommitted::refused(at_request_field(commit, refusal)));
        };
        let digest = commit
            .request
            .request_digest()
            .map_err(ExecutorError::Request)?;
        if receipt.request_digest != digest.as_str() {
            return Ok(ChangeCommitted::refused(IdentityRefusal::new(
                IdentityRefusalKind::OperationReused,
            )));
        }
        let outcome = outcome_of_receipt(commit.mob, commit.operation_id, &receipt)?;
        if let super::RequestAction::Remove { identity, .. } = &commit.request.action {
            self.after_removal(commit.mob, identity).await;
        }
        Ok(ChangeCommitted::new(outcome, false))
    }
}

/// `refusal` of a create whose agent asked for the name `taken`, about
/// that agent's name in the request (`agents[i].name` of the first agent
/// that asked for it): whoever holds the name, a live agent, an archived
/// one or a removed agent's reserved name.
fn at_taken_name(
    commit: &ChangeCommit<'_>,
    refusal: IdentityRefusal,
    taken: Option<&meerkat_mob::AgentIdentity>,
) -> IdentityRefusal {
    let (super::RequestAction::Create { identities }, Some(taken)) =
        (&commit.request.action, taken)
    else {
        return refusal;
    };
    if refusal.field.is_some() {
        return refusal;
    }
    match identities.iter().position(|created| {
        super::core_member_name(created.spec.identity.as_str()).as_ref() == Some(taken)
    }) {
        Some(index) => refusal.at(format!("agents[{index}].name")),
        None => refusal,
    }
}

/// `refusal`, about the request field it concerns when the request has one
/// for its kind: the spec digest the requester read for a stale spec.
fn at_request_field(commit: &ChangeCommit<'_>, refusal: IdentityRefusal) -> IdentityRefusal {
    if refusal.field.is_some() {
        return refusal;
    }
    let single = commit.request.action.identities().len() == 1
        && !matches!(commit.request.action, super::RequestAction::Create { .. });
    match refusal.kind {
        IdentityRefusalKind::StaleSpec { .. } if single => refusal.at("expected_spec_digest"),
        IdentityRefusalKind::IdentityUnknown | IdentityRefusalKind::NotArchived if single => {
            refusal.at("name")
        }
        _ => refusal,
    }
}

/// The requester as Core's commit takes it. An agent's change carries its
/// grant for `grant`, admitted by the session its tool call ran in or
/// rebuilt from its recorded identity; Core checks it again at the commit.
pub(super) async fn requester_input(
    commit: &ChangeCommit<'_>,
    operation: &IdentityOperation,
    grant: Option<IdentityGrantRequest>,
) -> Step<IdentityChangeRequesterInput> {
    match commit.requester {
        ChangeRequester::Host { surface, operator } => Ok(Ok(if *operator {
            IdentityChangeRequesterInput::host_operator(surface.as_str())
        } else {
            IdentityChangeRequesterInput::host(surface.as_str())
        })),
        ChangeRequester::Human { principal } => Ok(Ok(IdentityChangeRequesterInput::Human {
            principal: principal.clone(),
        })),
        ChangeRequester::Agent {
            identity,
            admission,
            ..
        } => {
            let Some(grant) = grant else {
                return Ok(Err(IdentityRefusal::new(
                    IdentityRefusalKind::IdentityUnknown,
                )
                .at("name")));
            };
            let admitted = match admission {
                AgentAdmission::Session(session) => {
                    commit
                        .mob
                        .admit_identity_change_grant(session, operation, &grant)
                        .await
                }
                AgentAdmission::Recorded => {
                    let Some(member) = core_member_name(identity) else {
                        return Ok(Err(IdentityRefusal::new(IdentityRefusalKind::InvalidName)));
                    };
                    commit
                        .mob
                        .identity_change_grant_for(&member, operation, &grant)
                        .await
                }
            };
            match admitted {
                Ok(grant) => Ok(Ok(IdentityChangeRequesterInput::Agent { grant })),
                Err(error) => refusal_of_error(error).map(Err),
            }
        }
    }
}

/// One agent of a create as Core's commit takes it: its spec, field for
/// field, and its peers with the rule that admitted each. Placement, a
/// runtime binding and a backend are refused `unsupported`: an agent placed
/// on a member host or bound to an external process is not hosted through
/// this surface.
fn create_request(
    created: &CreatedSpec,
    index: usize,
) -> Result<CreatedIdentityRequest, IdentityRefusal> {
    let spec = &created.spec;
    let path = format!("agents[{index}]");
    for (set, feature) in [
        (
            spec.placement.is_some(),
            "placement of an agent on a member host",
        ),
        (spec.binding.is_some(), "a runtime binding of an agent"),
        (spec.backend.is_some(), "a backend of an agent"),
    ] {
        if set {
            return Err(IdentityRefusal::new(IdentityRefusalKind::Unsupported {
                feature: feature.to_owned(),
            })
            .at(path));
        }
    }
    let name = core_name(spec.identity.as_str(), &format!("{path}.name"))?;
    let mut core = CreatedIdentitySpec::new(name, spec.profile.clone())
        .with_addressability(match spec.addressability {
            AgentAddressability::Addressable => IdentityAddressability::Addressable,
            AgentAddressability::InternalOnly => IdentityAddressability::InternalOnly,
        })
        .with_labels(spec.labels.clone())
        .with_additional_instructions(spec.additional_instructions.clone());
    if let Some(display_name) = &spec.display_name {
        core = core.with_display_name(display_name.as_str());
    }
    if let Some(context) = &spec.context {
        core = core.with_context(context.clone());
    }
    if let Some(message) = &spec.initial_message {
        core = core.with_initial_message(message.clone());
    }
    if let Some(runtime_mode) = spec.runtime_mode_override {
        core = core.with_runtime_mode_override(runtime_mode);
    }
    let peers_path = format!("agents[{index}].peers");
    let mut peers = Vec::with_capacity(created.peers.len());
    for (peer, basis) in &created.peers {
        let EdgeBasis::Rule { id } = basis;
        let member = core_name(peer, &peers_path)
            .map_err(|_| IdentityRefusal::new(IdentityRefusalKind::PeerUnknown).at(&peers_path))?;
        let rule = OwnedEdgeAdmission::rule(id.as_str()).map_err(|_| {
            IdentityRefusal::new(IdentityRefusalKind::InvalidRequest {
                problem: RequestProblem::Malformed,
            })
            .at(&peers_path)
        })?;
        peers.push(OwnedIdentityEdge::new(member, rule));
    }
    Ok(CreatedIdentityRequest::new(core).with_peers(peers))
}

/// How the host prepares an agent of `profile` before its first use, as
/// Core's create takes it: the mode the commit was given for the profile,
/// and none for a profile it names no mode for.
pub(super) fn preparation_of(commit: &ChangeCommit<'_>, profile: &str) -> IdentityPreparationMode {
    match commit
        .preparations
        .and_then(|rules| rules.get(profile))
        .map(PreparationRule::mode)
    {
        Some(PreparationMode::Required) => IdentityPreparationMode::Required,
        Some(PreparationMode::Constructed) => IdentityPreparationMode::Constructed,
        Some(PreparationMode::None) | None => IdentityPreparationMode::None,
    }
}

/// Commit a create of 1 to 256 agents, all or nothing: Core writes each
/// agent's record (revision 1, its spec, who asked and under what consent),
/// its seat and the receipt in one store transaction, and seats each agent
/// dormant. Nothing wakes but an agent whose spec gives it a first message.
/// An agent of a profile its host prepares is committed pending its
/// preparation instead: it has no member until the host settles it.
pub(super) async fn create(
    executor: &CoreChangeExecutor,
    commit: &ChangeCommit<'_>,
    operation: IdentityOperation,
    identities: &[CreatedSpec],
) -> Result<ChangeCommitted, ExecutorError> {
    let mut requests = Vec::with_capacity(identities.len());
    for (index, created) in identities.iter().enumerate() {
        match create_request(created, index) {
            Ok(request) => requests.push(
                request.with_preparation(preparation_of(commit, created.spec.profile.as_str())),
            ),
            Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
        }
    }
    let grant = matches!(commit.requester, ChangeRequester::Agent { .. })
        .then(|| IdentityGrantRequest::for_specs(requests.iter().map(|request| &request.spec)));
    let requester = match requester_input(commit, &operation, grant).await? {
        Ok(requester) => requester,
        Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
    };
    let mut create = StoreOwnedIdentityCreate::new(operation, requester, requests);
    if let Some(proposer) = &commit.request.proposer {
        create = create.with_proposer(proposer.as_str());
    }
    if let Some(evidence) = &commit.request.source_evidence {
        create = create.with_source_evidence(IdentitySourceEvidence::new(
            evidence.attestor.as_str(),
            evidence.subject.as_str(),
            evidence.source_ref.as_str(),
            evidence.observed_at_ms,
        ));
    }
    match commit
        .mob
        .create_store_owned_identities(create, commit.consent)
        .await
    {
        Ok(created) => committed(commit, &created.change),
        Err(error) => executor.after_core_error(commit, error).await,
    }
}

/// What an agent requester's grant is asked for a change of `member` as a
/// whole: the agent's current profile, from its record. `identity_unknown`
/// when Core holds no readable record of it.
async fn target_grant(
    commit: &ChangeCommit<'_>,
    member: &meerkat_mob::AgentIdentity,
) -> Step<IdentityGrantRequest> {
    let record = commit
        .mob
        .identity_record(member)
        .await
        .map_err(|error| ExecutorError::Core(Box::new(error)))?;
    let profile = match record {
        Some(CreatedIdentityObservation::Created(record)) => record.spec.map(|spec| spec.profile),
        _ => None,
    };
    Ok(match profile {
        Some(profile) => Ok(IdentityGrantRequest::for_identity(profile)),
        None => Err(IdentityRefusal::new(IdentityRefusalKind::IdentityUnknown).at("name")),
    })
}

/// Commit a change of one agent as a whole, decided against the spec whose
/// digest the requester read: Core refuses it `stale_spec` when the
/// agent's spec moved since. After a committed removal, and after its
/// replay, the executor's instruction step is told the agent was removed.
pub(super) async fn entry_change(
    executor: &CoreChangeExecutor,
    commit: &ChangeCommit<'_>,
    operation: IdentityOperation,
    change_kind: EntryChange,
    identity: &str,
    expected_spec_digest: &Sha256Digest,
) -> Result<ChangeCommitted, ExecutorError> {
    let member = match core_name(identity, "name") {
        Ok(member) => member,
        Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
    };
    let grant = if matches!(commit.requester, ChangeRequester::Agent { .. }) {
        match target_grant(commit, &member).await? {
            Ok(grant) => Some(grant),
            Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
        }
    } else {
        None
    };
    let requester = match requester_input(commit, &operation, grant).await? {
        Ok(requester) => requester,
        Err(refusal) => return Ok(ChangeCommitted::refused(refusal)),
    };
    let expected = expected_spec_digest.as_str();
    let answered = match change_kind {
        EntryChange::Archive => {
            commit
                .mob
                .archive_identity(
                    IdentityArchiveChange::new(operation, requester, member)
                        .with_expected_spec_digest(expected),
                    commit.consent,
                )
                .await
        }
        EntryChange::Unarchive => {
            commit
                .mob
                .unarchive_identity(
                    IdentityArchiveChange::new(operation, requester, member)
                        .with_expected_spec_digest(expected),
                    commit.consent,
                )
                .await
        }
        EntryChange::Remove => {
            commit
                .mob
                .remove_identity(
                    IdentityRemoval::new(operation, requester, member)
                        .with_expected_spec_digest(expected),
                    commit.consent,
                )
                .await
        }
    };
    match answered {
        Ok(change) => {
            let committed = committed(commit, &change)?;
            if change_kind == EntryChange::Remove {
                executor.after_removal(commit.mob, identity).await;
            }
            Ok(committed)
        }
        Err(error) => executor.after_core_error(commit, error).await,
    }
}
