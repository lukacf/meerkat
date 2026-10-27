//! Meta's lowering of Meerkat's application tool-policy contract.
//!
//! Meerkat 0.8.26 defines the compiled artifact, the provider and snapshot
//! traits, and the registry that binds a member to a policy, but it ships no
//! production implementation of either trait: the only ones in the published
//! crate live inside its own test module. This module is that implementation and
//! nothing more.
//!
//! Two boundaries here are deliberate rather than incidental.
//!
//! ACCEPTED-REVISION OWNERSHIP IS OURS. `ToolConsequenceNarrowingPolicy`
//! documents that the provider owns the snapshot pointer and must reject a
//! revision below one already accepted, because Meerkat "deliberately keeps no
//! second accepted-revision store". So the monotonic check lives here, and it
//! refuses using Meerkat's own vocabulary, `RevisionRollback` and
//! `RevisionDigestConflict`, rather than inventing a local error for a condition
//! upstream already named.
//!
//! EXECUTION AUTHORITY IS THE GRANT, NOT THE CONSEQUENCE CLASS. In v1 a grant is
//! one exact allow entry with no wildcards and no deny entries, its only action
//! is `Invoke`, and neither `ToolConsequenceRequest` nor
//! `ToolConsequenceVerdict` carries a threshold. So an exact member/tool grant
//! allows, a miss defers to the artifact's own `default_deny`, and
//! `CompiledToolConsequence` R0-R3 is reported for observability WITHOUT gating
//! execution. Choosing a threshold here would make Meta the authority on what
//! R2 means, which is upstream's to define if it is ever wanted.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use meerkat_core::{
    CompiledApplicationToolPolicy, CompiledMemberToolAction, PolicyDigest,
    PolicyEvaluationProvenance, PolicyId, PolicyProviderGeneration, PolicyProviderId,
    ToolConsequenceDenial, ToolConsequenceFailure, ToolConsequenceNarrowingPolicy,
    ToolConsequencePolicySnapshot, ToolConsequenceRequest, ToolConsequenceVerdict,
};

/// Denial code used when no exact grant covers the requested member and tool.
pub const DENIAL_CODE_NO_GRANT: &str = "application_tool_policy_no_grant";

/// One immutable compiled policy, served to Meerkat as a snapshot.
#[derive(Debug)]
pub struct CompiledPolicySnapshot {
    policy: CompiledApplicationToolPolicy,
}

impl CompiledPolicySnapshot {
    pub fn new(policy: CompiledApplicationToolPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &CompiledApplicationToolPolicy {
        &self.policy
    }
}

impl ToolConsequencePolicySnapshot for CompiledPolicySnapshot {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: self.policy.revision,
            digest: self.policy.policy_digest.clone(),
        }
    }

    fn evaluate(&self, request: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        let grant = self
            .policy
            .members
            .iter()
            .find(|member| member.member_identity == request.member.member)
            .and_then(|member| {
                member
                    .grants
                    .iter()
                    .find(|grant| grant.tool_name == request.tool_name.as_str())
            });

        match grant {
            // The only action in v1 is `Invoke`, so an exact entry is the
            // decision. The consequence class travels in the observation, not
            // in the verdict.
            Some(grant) => match grant.action {
                CompiledMemberToolAction::Invoke => ToolConsequenceVerdict::Allow,
            },
            None if self.policy.default_deny => {
                ToolConsequenceVerdict::Deny(ToolConsequenceDenial::new(
                    DENIAL_CODE_NO_GRANT,
                    format!(
                        "no exact grant for tool '{}' and member '{}' in policy '{}' revision {}",
                        request.tool_name.as_str(),
                        request.member.member,
                        self.policy.policy_id,
                        self.policy.revision.0
                    ),
                ))
            }
            // `default_deny = false` is the artifact's own switch. Honouring it
            // is reading the policy, not authoring one.
            None => ToolConsequenceVerdict::Allow,
        }
    }
}

#[derive(Debug)]
struct AcceptedPolicy {
    revision: u64,
    digest: PolicyDigest,
    snapshot: Arc<CompiledPolicySnapshot>,
}

/// Serves the compiled policies Meta was configured with, and owns the
/// accepted-revision fence Meerkat delegates to the provider.
#[derive(Debug)]
pub struct CompiledPolicyProvider {
    provider_id: PolicyProviderId,
    generation: PolicyProviderGeneration,
    accepted: RwLock<BTreeMap<PolicyId, AcceptedPolicy>>,
}

impl CompiledPolicyProvider {
    pub fn new(provider_id: PolicyProviderId, generation: PolicyProviderGeneration) -> Self {
        Self {
            provider_id,
            generation,
            accepted: RwLock::new(BTreeMap::new()),
        }
    }

    /// Refuse a `member_identity` that cannot name a roster row.
    ///
    /// The grant lookup in [`CompiledPolicySnapshot::evaluate`] is exact string
    /// equality against `MobMemberBinding.member`, and Meerkat sets that from
    /// the member's `AgentIdentity` - the ENCODED DURABLE identity
    /// (`meerkat-mob` build.rs, `member: agent_identity.as_str()`, whose very
    /// next statement requires `comms_name()` to validate). Neither side
    /// decodes. So an artifact compiled against any other id space matches no
    /// member at all, and because `validate()` forces `default_deny` true, that
    /// policy denies EVERY tool for EVERY member the moment it is bound.
    ///
    /// Install is the only loud moment available. What this prevents is not a
    /// wrong verdict but an artifact that is accepted, looks deployed, and is
    /// either inert (nothing bound) or total (something bound) - two states no
    /// surface distinguishes, because `Allow` emits no observation and only
    /// `Deny` reaches the observer.
    ///
    /// The exact production shape from a downstream app, 2026-08-31: 17 members compiled as
    /// `mk--rt_cdomain_ccalendar_c0`, the encoded AGENT RUNTIME ID.
    /// [`crate::member_comms_id::roster_member_id_for_identity`] documents that
    /// conversion as producing "binding detail that no longer names a roster
    /// row at all", and warns it "does not fail loudly". Here it does.
    fn validate_member_identity(member_identity: &str) -> Result<(), String> {
        if member_identity.is_empty() {
            return Err("is empty".to_string());
        }
        let decoded = crate::member_comms_id::runtime_alias_str(member_identity);
        if crate::member_comms_id::is_reserved_generated_alias(member_identity) {
            return Err(
                match crate::member_comms_id::durable_identity_from_runtime_alias(decoded.as_ref())
                {
                    Some(durable) => format!(
                        "names the agent RUNTIME id '{}', which is binding detail and matches no \
                         roster row; compile the encoded durable identity '{}' instead",
                        decoded,
                        crate::member_comms_id::mob_member_id_str(&durable)
                    ),
                    None => format!(
                        "names the reserved '{decoded}' runtime namespace, which matches no roster \
                         row"
                    ),
                },
            );
        }
        let canonical = crate::member_comms_id::mob_member_id_str(decoded.as_ref()).into_owned();
        if canonical != member_identity {
            return Err(format!(
                "is not a canonical roster member id; the encoded form of '{decoded}' is \
                 '{canonical}'"
            ));
        }
        Ok(())
    }

    /// Install a compiled policy, refusing any revision that is not a forward
    /// move for its policy id.
    ///
    /// Validates the artifact, including its digest and required default-deny
    /// setting, before taking the accepted-state write lock. Also rejects a
    /// policy belonging to another provider, a `member_identity` that names no
    /// roster row (see [`Self::validate_member_identity`]), a revision below
    /// one already accepted, and a repeat of an accepted revision whose bytes
    /// differ. Validation happens at installation so evaluation can use the
    /// accepted immutable snapshot without rechecking the artifact.
    pub fn accept(
        &self,
        policy: CompiledApplicationToolPolicy,
    ) -> Result<(), ToolConsequenceFailure> {
        if policy.provider_id != self.provider_id {
            return Err(ToolConsequenceFailure::EvaluationFailed {
                reason: format!(
                    "compiled policy names provider '{}' but this provider is '{}'",
                    policy.provider_id, self.provider_id
                ),
            });
        }

        for member in &policy.members {
            if let Err(reason) = Self::validate_member_identity(&member.member_identity) {
                return Err(ToolConsequenceFailure::EvaluationFailed {
                    reason: format!(
                        "compiled policy '{}' revision {} declares member_identity '{}' which {reason}",
                        policy.policy_id, policy.revision.0, member.member_identity
                    ),
                });
            }
        }

        policy
            .validate()
            .map_err(|error| ToolConsequenceFailure::EvaluationFailed {
                reason: format!("compiled application tool policy rejected: {error}"),
            })?;

        let mut accepted = self
            .accepted
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if let Some(current) = accepted.get(&policy.policy_id) {
            if policy.revision.0 < current.revision {
                return Err(ToolConsequenceFailure::RevisionRollback {
                    provider_id: self.provider_id.clone(),
                    policy_id: policy.policy_id.clone(),
                    accepted_revision: current.revision,
                    observed_revision: policy.revision.0,
                });
            }
            if policy.revision.0 == current.revision && policy.policy_digest != current.digest {
                return Err(ToolConsequenceFailure::RevisionDigestConflict {
                    provider_id: self.provider_id.clone(),
                    policy_id: policy.policy_id.clone(),
                    revision: policy.revision.0,
                });
            }
        }

        let policy_id = policy.policy_id.clone();
        let revision = policy.revision.0;
        let digest = policy.policy_digest.clone();
        accepted.insert(
            policy_id,
            AcceptedPolicy {
                revision,
                digest,
                snapshot: Arc::new(CompiledPolicySnapshot::new(policy)),
            },
        );
        Ok(())
    }

    /// Parse and install one canonical compiled-policy payload.
    ///
    /// Parsing goes through `parse_canonical_json`, so unknown fields, missing
    /// fields, non-canonical byte form and a mismatched digest all fail here
    /// rather than at first evaluation.
    pub fn accept_canonical_json(&self, bytes: &[u8]) -> Result<(), ToolConsequenceFailure> {
        let policy =
            CompiledApplicationToolPolicy::parse_canonical_json(bytes).map_err(|error| {
                ToolConsequenceFailure::EvaluationFailed {
                    reason: format!("compiled application tool policy rejected: {error}"),
                }
            })?;
        self.accept(policy)
    }

    /// Revision currently accepted for `policy_id`, for diagnostics.
    pub fn accepted_revision(&self, policy_id: &PolicyId) -> Option<u64> {
        self.accepted
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(policy_id)
            .map(|accepted| accepted.revision)
    }
}

/// Read the `application_tool_policies` init parameter.
///
/// Absent means an empty list: a boot that declared nothing arms nothing. A
/// present value that is not an array of strings is an error rather than a
/// silent empty, because a host that supplied policies in the wrong shape
/// expects them in force and would otherwise see the refusal much later as an
/// unexplained access denial.
///
/// Lives here rather than inline in the gateway so the gateway, the wire
/// fixture test, and the Python SDK are all exercising ONE extraction. The
/// hardcoded-provider defect shipped precisely because this logic was
/// unreachable from a test while it sat in a binary.
pub fn compiled_policy_payloads_from_init_params(
    params: &serde_json::Value,
) -> Result<Vec<String>, ToolConsequenceFailure> {
    match params.get("application_tool_policies") {
        // ABSENT means empty. An explicit `null` is PRESENT and not an array of
        // strings, so it is malformed: a host that wrote the key expects it in
        // force, and treating null as absent would arm nothing while looking
        // like it had worked. This distinction is the whole fail-closed claim.
        None => Ok(Vec::new()),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            ToolConsequenceFailure::EvaluationFailed {
                reason: format!("application_tool_policies is malformed: {error}"),
            }
        }),
    }
}

/// Read the `child_application_tool_policy` init parameter: the application
/// tool policy every child mob member (agent `mob_create`, delegate's implicit
/// mob) is built with, as an `ApplicationToolPolicyBinding` JSON object, for
/// example `{"kind":"provider","provider_id":"example-provider","policy_id":"child-tools"}`
/// or the explicit opt-out `{"kind":"unmanaged"}`.
///
/// Absent means none was chosen. A present value that is not a binding,
/// including `null`, refuses the boot for the same reason a malformed
/// `application_tool_policies` does: a host that wrote the key expects it in
/// force. `{"kind":"inherit"}` is refused too: a child mob has no member to
/// inherit from when it is created, and Meerkat would refuse every child mob
/// at use.
pub fn child_application_tool_policy_from_init_params(
    params: &serde_json::Value,
) -> Result<Option<meerkat_core::ApplicationToolPolicyBinding>, ToolConsequenceFailure> {
    let Some(value) = params.get("child_application_tool_policy") else {
        return Ok(None);
    };
    let binding: meerkat_core::ApplicationToolPolicyBinding = serde_json::from_value(value.clone())
        .map_err(|error| ToolConsequenceFailure::EvaluationFailed {
            reason: format!("child_application_tool_policy is malformed: {error}"),
        })?;
    if matches!(binding, meerkat_core::ApplicationToolPolicyBinding::Inherit) {
        return Err(ToolConsequenceFailure::EvaluationFailed {
            reason: "child_application_tool_policy cannot be {\"kind\":\"inherit\"}: a child \
                     mob has no member to inherit from; choose a provider binding or \
                     {\"kind\":\"unmanaged\"}"
                .to_string(),
        });
    }
    Ok(Some(binding))
}

/// Check a configured child policy against the providers this boot serves,
/// so a binding Meerkat could never resolve refuses the boot instead of
/// failing every later `mob_create` and `delegate`. A provider binding needs
/// its provider among `providers` and a policy that provider can load;
/// `Unmanaged` needs nothing.
pub fn validate_child_application_tool_policy(
    binding: &meerkat_core::ApplicationToolPolicyBinding,
    providers: &[Arc<dyn ToolConsequenceNarrowingPolicy>],
) -> Result<(), ToolConsequenceFailure> {
    let meerkat_core::ApplicationToolPolicyBinding::Provider {
        provider_id,
        policy_id,
    } = binding
    else {
        return Ok(());
    };
    let provider = providers
        .iter()
        .find(|provider| provider.provider_id() == provider_id)
        .ok_or_else(|| ToolConsequenceFailure::EvaluationFailed {
            reason: format!(
                "child_application_tool_policy names provider '{provider_id}', which no \
                 application_tool_policies entry carries"
            ),
        })?;
    provider.snapshot(policy_id).map(|_| ()).map_err(|error| {
        ToolConsequenceFailure::EvaluationFailed {
            reason: format!(
                "child_application_tool_policy names policy '{policy_id}' of provider \
                 '{provider_id}', which it cannot load: {error}"
            ),
        }
    })
}

/// Build one provider per distinct provider id CARRIED by the supplied
/// canonical payloads.
///
/// The host must not name the provider. A compiled policy declares its own
/// author, a member's `ApplicationToolPolicyBinding::Provider { provider_id, .. }`
/// is resolved by that same carried id, and
/// `ToolConsequencePolicyRegistry::new` takes a vec precisely so one host can
/// serve several. Naming a provider here would refuse every policy compiled by
/// anyone else, which is not a narrower feature but a dead one.
///
/// Returns providers ordered by id, so a boot is deterministic in what it
/// registers.
pub fn providers_from_canonical_payloads<I, S>(
    payloads: I,
) -> Result<Vec<Arc<CompiledPolicyProvider>>, ToolConsequenceFailure>
where
    I: IntoIterator<Item = S>,
    S: AsRef<[u8]>,
{
    let mut by_provider: BTreeMap<PolicyProviderId, Arc<CompiledPolicyProvider>> = BTreeMap::new();
    for payload in payloads {
        let policy = CompiledApplicationToolPolicy::parse_canonical_json(payload.as_ref())
            .map_err(|error| ToolConsequenceFailure::EvaluationFailed {
                reason: format!("compiled application tool policy rejected: {error}"),
            })?;
        let provider_id = policy.provider_id.clone();
        let provider = by_provider.entry(provider_id.clone()).or_insert_with(|| {
            Arc::new(CompiledPolicyProvider::new(
                provider_id,
                PolicyProviderGeneration(1),
            ))
        });
        provider.accept(policy)?;
    }
    Ok(by_provider.into_values().collect())
}

impl ToolConsequenceNarrowingPolicy for CompiledPolicyProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.provider_id
    }

    fn generation(&self) -> PolicyProviderGeneration {
        self.generation
    }

    fn snapshot(
        &self,
        policy_id: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        let accepted = self
            .accepted
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match accepted.get(policy_id) {
            Some(accepted) => Ok(Arc::clone(&accepted.snapshot) as Arc<_>),
            None => Err(ToolConsequenceFailure::PolicyMissing {
                provider_id: self.provider_id.clone(),
                policy_id: policy_id.clone(),
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{
        CompiledMemberToolGrant, CompiledMemberToolGrants, CompiledPolicySourceProvenance,
        CompiledToolConsequence, MobMemberBinding, PolicyEvaluationSupervisorConfig,
        PolicyRevision, ToolConsequencePolicyRegistry, ToolName,
    };

    fn provider_id() -> PolicyProviderId {
        PolicyProviderId::new("meta-test-provider").expect("provider id")
    }

    fn policy_id() -> PolicyId {
        PolicyId::new("fleet-baseline").expect("policy id")
    }

    fn source() -> CompiledPolicySourceProvenance {
        CompiledPolicySourceProvenance {
            source_id: "meta-test-source".to_string(),
            source_digest: PolicyDigest::from_canonical_bytes(b"meta-test-source"),
        }
    }

    fn grants_for(member: &str, tool: &str) -> Vec<CompiledMemberToolGrants> {
        vec![CompiledMemberToolGrants {
            member_identity: member.to_string(),
            grants: vec![CompiledMemberToolGrant {
                tool_name: tool.to_string(),
                action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R2,
            }],
        }]
    }

    fn policy(revision: u64, member: &str, tool: &str) -> CompiledApplicationToolPolicy {
        CompiledApplicationToolPolicy::new(
            provider_id(),
            policy_id(),
            PolicyRevision(revision),
            source(),
            grants_for(member, tool),
        )
        .expect("compiled policy should validate")
    }

    fn request(member: &str, tool: &str) -> ToolConsequenceRequest {
        ToolConsequenceRequest {
            member: MobMemberBinding {
                mob_id: "mob".to_string(),
                role: "worker".to_string(),
                member: member.to_string(),
            },
            tool_name: ToolName::new(tool),
            arguments_json: "{}".to_string(),
            arguments_digest: "sha256:test".to_string(),
            run_id: None,
            tool_call_id: "call-1".to_string(),
            provider_id: provider_id(),
            policy_id: policy_id(),
        }
    }

    fn installed(policy: CompiledApplicationToolPolicy) -> Arc<CompiledPolicyProvider> {
        let provider = Arc::new(CompiledPolicyProvider::new(
            provider_id(),
            PolicyProviderGeneration(1),
        ));
        provider.accept(policy).expect("first accept");
        provider
    }

    #[test]
    fn an_exact_member_and_tool_grant_allows() {
        let provider = installed(policy(1, "member-a", "shell"));
        let snapshot = provider.snapshot(&policy_id()).expect("snapshot");
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "shell")),
            ToolConsequenceVerdict::Allow
        ));
    }

    #[test]
    fn a_missing_grant_denies_under_the_artifacts_own_default_deny() {
        let policy = policy(1, "member-a", "shell");
        assert!(
            policy.default_deny,
            "a policy minted through new() must be default-deny"
        );
        let provider = CompiledPolicyProvider::new(provider_id(), PolicyProviderGeneration(1));
        let mut invalid = policy.clone();
        invalid.default_deny = false;
        assert!(matches!(
            provider.accept(invalid.clone()),
            Err(ToolConsequenceFailure::EvaluationFailed { .. })
        ));
        assert_eq!(provider.accepted_revision(&policy_id()), None);
        assert!(matches!(
            provider.snapshot(&policy_id()),
            Err(ToolConsequenceFailure::PolicyMissing { .. })
        ));

        provider.accept(policy).expect("valid policy installs");
        let prior = provider.snapshot(&policy_id()).expect("prior snapshot");
        let provenance = prior.provenance();
        assert!(matches!(
            provider.accept(invalid),
            Err(ToolConsequenceFailure::EvaluationFailed { .. })
        ));
        assert_eq!(provider.accepted_revision(&policy_id()), Some(1));
        let snapshot = provider.snapshot(&policy_id()).expect("snapshot");
        assert!(Arc::ptr_eq(&prior, &snapshot));
        assert_eq!(snapshot.provenance(), provenance);
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "shell")),
            ToolConsequenceVerdict::Allow
        ));

        match snapshot.evaluate(&request("member-a", "network")) {
            ToolConsequenceVerdict::Deny(denial) => {
                assert_eq!(denial.code, DENIAL_CODE_NO_GRANT);
                assert!(denial.message.contains("network"), "{}", denial.message);
            }
            other => panic!("expected a typed denial, got {other:?}"),
        }
        // Same tool, wrong member: the grant is per identity, not global.
        assert!(matches!(
            snapshot.evaluate(&request("member-b", "shell")),
            ToolConsequenceVerdict::Deny(_)
        ));
    }

    /// The exact 17-member shape a downstream app compiled on 2026-08-31: the encoded
    /// AGENT RUNTIME ID rather than the encoded durable identity. Every one of
    /// these matches no roster row, so under the forced `default_deny` the
    /// policy would deny every tool for every member the moment it bound.
    #[test]
    fn an_encoded_runtime_id_is_refused_and_the_message_names_the_right_id() {
        let provider = Arc::new(CompiledPolicyProvider::new(
            provider_id(),
            PolicyProviderGeneration(1),
        ));
        let error = provider
            .accept(
                CompiledApplicationToolPolicy::new(
                    provider_id(),
                    policy_id(),
                    PolicyRevision(1),
                    source(),
                    grants_for("mk--rt_cdomain_ccalendar_c0", "shell"),
                )
                .expect("the artifact itself is well formed; only the id space is wrong"),
            )
            .expect_err("an encoded runtime id must be refused");

        let ToolConsequenceFailure::EvaluationFailed { reason } = &error else {
            panic!("expected EvaluationFailed, got {error:?}");
        };
        // The refusal has to be actionable, which means naming the id to
        // compile INSTEAD - a message that only says "wrong" leaves the caller
        // to rediscover the encoding.
        assert!(
            reason.contains("'mk--domain_ccalendar'"),
            "the refusal must name the encoded DURABLE identity: {reason}"
        );
        assert!(
            reason.contains("RUNTIME"),
            "the refusal must say what was supplied: {reason}"
        );
        assert_eq!(
            provider.accepted_revision(&policy_id()),
            None,
            "a refused artifact must not be half-installed"
        );
    }

    /// The other direction of the same mistake: the durable identity, correct,
    /// but not encoded. This is what a compiler reading our own roster JSON
    /// would emit if it skipped `mob_member_id_str`.
    #[test]
    fn an_unencoded_durable_identity_is_refused_with_its_encoding() {
        let provider = Arc::new(CompiledPolicyProvider::new(
            provider_id(),
            PolicyProviderGeneration(1),
        ));
        let error = provider
            .accept(
                CompiledApplicationToolPolicy::new(
                    provider_id(),
                    policy_id(),
                    PolicyRevision(1),
                    source(),
                    grants_for("domain:calendar", "shell"),
                )
                .expect("well formed artifact"),
            )
            .expect_err("a raw durable identity must be refused");
        let ToolConsequenceFailure::EvaluationFailed { reason } = &error else {
            panic!("expected EvaluationFailed, got {error:?}");
        };
        assert!(
            reason.contains("mk--domain_ccalendar"),
            "the refusal must name the encoding: {reason}"
        );
    }

    /// The positive control the two refusals are only meaningful against: the
    /// id space the gate actually compares installs AND evaluates. Without this
    /// arm the checks above are satisfied by a validator that refuses
    /// everything.
    #[test]
    fn the_encoded_durable_identity_installs_and_evaluates() {
        let roster = crate::member_comms_id::mob_member_id_str("domain:calendar").into_owned();
        assert_eq!(
            roster, "mk--domain_ccalendar",
            "this test is only meaningful if it pins the encoding it claims"
        );
        let provider = installed(
            CompiledApplicationToolPolicy::new(
                provider_id(),
                policy_id(),
                PolicyRevision(1),
                source(),
                grants_for(&roster, "shell"),
            )
            .expect("well formed artifact"),
        );
        let snapshot = provider.snapshot(&policy_id()).expect("snapshot");
        assert!(matches!(
            snapshot.evaluate(&request(&roster, "shell")),
            ToolConsequenceVerdict::Allow
        ));
        assert!(matches!(
            snapshot.evaluate(&request(&roster, "network")),
            ToolConsequenceVerdict::Deny(_)
        ));
    }

    /// A bare name that needs no encoding is already canonical and must keep
    /// installing - the check rejects a wrong id SPACE, not every id that does
    /// not carry the marker.
    #[test]
    fn a_plain_component_member_id_is_still_accepted() {
        let provider = installed(policy(1, "member-a", "shell"));
        assert_eq!(provider.accepted_revision(&policy_id()), Some(1));
    }

    #[test]
    fn brain_swap_exact_grant_allows_and_missing_grant_obeys_default_deny() {
        // Consume Meerkat's exported constant rather than a fourth hardcoded
        // literal: if upstream ever renames the tool, this test breaks loudly
        // instead of pinning grants for a tool that no longer exists.
        const BRAIN_SWAP: &str = meerkat_tools::builtin::brain_swap::BRAIN_SWAP_TOOL_NAME;

        let granted = installed(policy(1, "member-a", BRAIN_SWAP));
        let granted_snapshot = granted.snapshot(&policy_id()).expect("snapshot");
        assert!(matches!(
            granted_snapshot.evaluate(&request("member-a", BRAIN_SWAP)),
            ToolConsequenceVerdict::Allow
        ));

        let missing = policy(1, "member-a", "shell");
        assert!(
            missing.default_deny,
            "a policy minted through new() must be default-deny"
        );
        let denied = installed(missing);
        let denied_snapshot = denied.snapshot(&policy_id()).expect("snapshot");
        match denied_snapshot.evaluate(&request("member-a", BRAIN_SWAP)) {
            ToolConsequenceVerdict::Deny(denial) => {
                assert_eq!(denial.code, DENIAL_CODE_NO_GRANT);
                assert!(denial.message.contains(BRAIN_SWAP), "{}", denial.message);
            }
            other => panic!("expected a typed denial, got {other:?}"),
        }
    }

    #[test]
    fn the_provider_refuses_a_revision_rollback() {
        let provider = installed(policy(7, "member-a", "shell"));
        let error = provider
            .accept(policy(6, "member-a", "shell"))
            .expect_err("a lower revision must be refused");
        match error {
            ToolConsequenceFailure::RevisionRollback {
                accepted_revision,
                observed_revision,
                ..
            } => {
                assert_eq!(accepted_revision, 7);
                assert_eq!(observed_revision, 6);
            }
            other => panic!("expected RevisionRollback, got {other:?}"),
        }
        assert_eq!(provider.accepted_revision(&policy_id()), Some(7));
    }

    #[test]
    fn the_provider_refuses_a_reused_revision_whose_content_changed() {
        let accepted = policy(7, "member-a", "shell");
        let provider = installed(accepted.clone());
        let prior = provider.snapshot(&policy_id()).expect("prior snapshot");
        let provenance = prior.provenance();
        let error = provider
            .accept(policy(7, "member-a", "network"))
            .expect_err("same revision with different bytes must be refused");
        assert!(
            matches!(
                error,
                ToolConsequenceFailure::RevisionDigestConflict { revision: 7, .. }
            ),
            "expected RevisionDigestConflict, got {error:?}"
        );
        let mut stale_digest = accepted;
        stale_digest.members[0].grants[0].tool_name = "network".to_string();
        assert_eq!(stale_digest.revision, provenance.revision);
        assert_eq!(stale_digest.policy_digest, provenance.digest);
        let error = provider
            .accept(stale_digest)
            .expect_err("mutated content retaining the accepted digest must be refused");
        assert!(
            matches!(error, ToolConsequenceFailure::EvaluationFailed { .. }),
            "expected EvaluationFailed, got {error:?}"
        );
        // The accepted snapshot must be unchanged, not partially replaced.
        assert_eq!(provider.accepted_revision(&policy_id()), Some(7));
        let snapshot = provider.snapshot(&policy_id()).expect("snapshot");
        assert!(Arc::ptr_eq(&prior, &snapshot));
        assert_eq!(snapshot.provenance(), provenance);
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "shell")),
            ToolConsequenceVerdict::Allow
        ));
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "network")),
            ToolConsequenceVerdict::Deny(_)
        ));
    }

    #[test]
    fn a_forward_revision_replaces_the_snapshot() {
        let provider = installed(policy(1, "member-a", "shell"));
        provider
            .accept(policy(2, "member-a", "network"))
            .expect("a forward revision is accepted");
        assert_eq!(provider.accepted_revision(&policy_id()), Some(2));
        let snapshot = provider.snapshot(&policy_id()).expect("snapshot");
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "network")),
            ToolConsequenceVerdict::Allow
        ));
        assert!(matches!(
            snapshot.evaluate(&request("member-a", "shell")),
            ToolConsequenceVerdict::Deny(_)
        ));
    }

    #[test]
    fn canonical_bytes_install_and_non_canonical_bytes_do_not() {
        let canonical = policy(3, "member-a", "shell")
            .canonical_json()
            .expect("canonical json");
        let provider = Arc::new(CompiledPolicyProvider::new(
            provider_id(),
            PolicyProviderGeneration(1),
        ));
        provider
            .accept_canonical_json(&canonical)
            .expect("canonical bytes install");
        assert_eq!(provider.accepted_revision(&policy_id()), Some(3));

        // Re-serialised without the canonical trailing newline: same policy,
        // non-canonical bytes, and the parse must refuse rather than normalise.
        let mut mangled = canonical;
        assert_eq!(mangled.pop(), Some(b'\n'));
        provider
            .accept_canonical_json(&mangled)
            .expect_err("non-canonical bytes must be refused");
    }

    #[test]
    fn an_unknown_policy_id_is_a_typed_miss() {
        let provider = installed(policy(1, "member-a", "shell"));
        let other = PolicyId::new("not-installed").expect("policy id");
        // `Arc<dyn Snapshot>` is not `Debug`, so `expect_err` cannot be used
        // here; match instead of loosening the trait upstream.
        let error = match provider.snapshot(&other) {
            Ok(_) => panic!("an unknown policy must not resolve"),
            Err(error) => error,
        };
        assert!(
            matches!(error, ToolConsequenceFailure::PolicyMissing { .. }),
            "expected PolicyMissing, got {error:?}"
        );
    }

    fn policy_for(provider: &str, policy: &str, member: &str, tool: &str) -> Vec<u8> {
        CompiledApplicationToolPolicy::new(
            PolicyProviderId::new(provider).expect("provider id"),
            PolicyId::new(policy).expect("policy id"),
            PolicyRevision(1),
            source(),
            grants_for(member, tool),
        )
        .expect("compiled policy should validate")
        .canonical_json()
        .expect("canonical json")
    }

    #[test]
    fn providers_are_derived_from_the_ids_the_artifacts_carry() {
        // The host compiles provider_id "example-provider"; nothing in Meta names it.
        let payloads = vec![
            policy_for("example-provider", "team-tools", "member-a", "shell"),
            policy_for("some-other-author", "fleet-baseline", "member-b", "network"),
        ];
        let providers =
            providers_from_canonical_payloads(&payloads).expect("both artifacts install");
        let ids: Vec<&str> = providers
            .iter()
            .map(|provider| provider.provider_id().as_str())
            .collect();
        assert_eq!(ids, vec!["example-provider", "some-other-author"]);
    }

    /// The gateway calls `providers_from_canonical_payloads`, not `accept`.
    /// Every other refusal test here drives `accept` directly, and the only
    /// existing coverage of this function feeds it a PASSING id - so without
    /// this arm the refusal could be real at the unit level and unreachable
    /// from the one entry point a boot actually uses.
    #[test]
    fn the_gateways_own_entry_point_refuses_a_wrong_id_space_artifact() {
        let payload = CompiledApplicationToolPolicy::new(
            PolicyProviderId::new("example-provider").expect("provider id"),
            PolicyId::new("team-tools").expect("policy id"),
            PolicyRevision(1),
            source(),
            grants_for("mk--rt_cdomain_ccalendar_c0", "shell"),
        )
        .expect("well formed artifact")
        .canonical_json()
        .expect("canonical json");

        let error = match providers_from_canonical_payloads(&[payload]) {
            Ok(_) => panic!("the gateway entry point must refuse a wrong-id-space artifact"),
            Err(error) => error,
        };
        let ToolConsequenceFailure::EvaluationFailed { reason } = &error else {
            panic!("expected EvaluationFailed, got {error:?}");
        };
        assert!(
            reason.contains("'mk--domain_ccalendar'"),
            "the boot refusal must carry the id to compile instead: {reason}"
        );
    }

    #[test]
    fn a_foreign_provider_id_is_served_rather_than_refused() {
        // This is the bug this function exists to prevent: a gateway that named
        // its own provider refused every policy compiled by anyone else.
        let payloads = vec![policy_for(
            "example-provider",
            "team-tools",
            "member-a",
            "shell",
        )];
        let providers = providers_from_canonical_payloads(&payloads).expect("artifact installs");
        assert_eq!(providers.len(), 1);
        let snapshot = providers[0]
            .snapshot(&PolicyId::new("team-tools").expect("policy id"))
            .expect("the carried policy id resolves");
        let mut request = request("member-a", "shell");
        request.provider_id = PolicyProviderId::new("example-provider").expect("provider id");
        request.policy_id = PolicyId::new("team-tools").expect("policy id");
        assert!(matches!(
            snapshot.evaluate(&request),
            ToolConsequenceVerdict::Allow
        ));
    }

    #[test]
    fn two_policies_from_one_author_share_a_single_provider() {
        let payloads = vec![
            policy_for("example-provider", "team-tools", "member-a", "shell"),
            policy_for("example-provider", "guest-tools", "member-b", "network"),
        ];
        let providers = providers_from_canonical_payloads(&payloads).expect("both install");
        assert_eq!(providers.len(), 1, "one author means one provider");
        for policy_id in ["team-tools", "guest-tools"] {
            providers[0]
                .snapshot(&PolicyId::new(policy_id).expect("policy id"))
                .unwrap_or_else(|error| panic!("{policy_id} should resolve: {error}"));
        }
    }

    #[test]
    fn a_registry_built_from_the_derived_providers_binds_the_carried_identity() {
        // The end-to-end path the gateway performs: artifact -> provider ->
        // registry -> bind. bind() additionally validates provenance and
        // consults the supervisor, so passing here proves the composition and
        // not just the pieces.
        let payloads = vec![policy_for(
            "example-provider",
            "team-tools",
            "member-a",
            "shell",
        )];
        let derived = providers_from_canonical_payloads(&payloads).expect("artifact installs");
        let providers: Vec<Arc<dyn ToolConsequenceNarrowingPolicy>> = derived
            .into_iter()
            .map(|provider| provider as Arc<dyn ToolConsequenceNarrowingPolicy>)
            .collect();
        let registry = Arc::new(
            ToolConsequencePolicyRegistry::new(
                providers,
                PolicyEvaluationSupervisorConfig::default(),
                None,
            )
            .expect("registry builds from the derived providers"),
        );

        let member = MobMemberBinding {
            mob_id: "mob".to_string(),
            role: "worker".to_string(),
            member: "member-a".to_string(),
        };
        registry
            .bind(
                member.clone(),
                PolicyProviderId::new("example-provider").expect("provider id"),
                PolicyId::new("team-tools").expect("policy id"),
            )
            .expect("the carried provider and policy identity must bind");

        // The negative half, which is the shape of the bug this replaces: the
        // registry resolves the CARRIED identity, so a name the artifacts never
        // declared must miss rather than silently bind to whatever is present.
        // `BoundToolConsequencePolicy` is not `Debug`, so match rather than
        // `expect_err`, as elsewhere in this module.
        let error = match registry.bind(
            member,
            PolicyProviderId::new("meta-gateway").expect("provider id"),
            PolicyId::new("team-tools").expect("policy id"),
        ) {
            Ok(_) => panic!("a provider no artifact declared must not bind"),
            Err(error) => error,
        };
        assert!(
            matches!(error, ToolConsequenceFailure::ProviderMissing { .. }),
            "expected ProviderMissing, got {error:?}"
        );
    }

    /// One wire fixture, read by BOTH this test and
    /// sdk/python/tests/test_application_tool_policies.py.
    ///
    /// A Rust test and a Python test that each build their own payload can both
    /// pass while disagreeing about the wire, which is how the role-migration
    /// carrier shipped unreachable from the SDK that composes it. Sharing the
    /// bytes is what makes the two sides testable against one another.
    ///
    /// Regenerate with META_WRITE_FIXTURE=1; a digest is computed from the
    /// canonical bytes, so the fixture cannot be hand-edited into validity.
    #[test]
    fn the_committed_wire_fixture_installs_its_carried_provider() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/application_tool_policies_init_params.json");
        let canonical = String::from_utf8(policy_for(
            "example-provider",
            "team-tools",
            "member-a",
            "shell",
        ))
        .expect("canonical json is utf-8");

        if std::env::var_os("META_WRITE_FIXTURE").is_some() {
            let params = serde_json::json!({
                "has_roster_provider": true,
                "application_tool_policies": [canonical],
            });
            let mut rendered = serde_json::to_string_pretty(&params).expect("render fixture");
            rendered.push('\n');
            std::fs::write(&fixture_path, rendered).expect("write fixture");
        }

        let raw = std::fs::read_to_string(&fixture_path).unwrap_or_else(|error| {
            panic!(
                "missing {}: regenerate with META_WRITE_FIXTURE=1 ({error})",
                fixture_path.display()
            )
        });
        let params: serde_json::Value = serde_json::from_str(&raw).expect("fixture is valid JSON");
        // Go through the SAME extraction the gateway uses. Reading the key here
        // would test a re-implementation of the parser rather than the parser.
        let payloads = compiled_policy_payloads_from_init_params(&params)
            .expect("the committed wire payload parses through the gateway's own extraction");
        assert_eq!(
            payloads,
            vec![canonical],
            "the committed fixture must carry the canonical bytes this build produces"
        );

        // The point of the fixture is that these exact bytes install and bind.
        let providers = providers_from_canonical_payloads(&payloads)
            .expect("the committed wire payload installs");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider_id().as_str(), "example-provider");
        providers[0]
            .snapshot(&PolicyId::new("team-tools").expect("policy id"))
            .expect("the carried policy id resolves");
    }

    #[test]
    fn an_absent_parameter_arms_nothing_but_an_explicit_null_is_refused() {
        let absent = serde_json::json!({ "has_roster_provider": true });
        assert!(
            compiled_policy_payloads_from_init_params(&absent)
                .expect("absent is not an error")
                .is_empty()
        );

        for malformed in [
            serde_json::json!({ "application_tool_policies": serde_json::Value::Null }),
            serde_json::json!({ "application_tool_policies": "not-an-array" }),
            serde_json::json!({ "application_tool_policies": [1, 2] }),
            serde_json::json!({ "application_tool_policies": { "a": "b" } }),
        ] {
            let error = compiled_policy_payloads_from_init_params(&malformed)
                .expect_err("a present non-array value must be refused");
            assert!(
                matches!(error, ToolConsequenceFailure::EvaluationFailed { .. }),
                "expected EvaluationFailed, got {error:?}"
            );
        }
    }

    #[test]
    fn a_policy_from_another_provider_is_refused() {
        let provider = Arc::new(CompiledPolicyProvider::new(
            PolicyProviderId::new("meta-other").expect("provider id"),
            PolicyProviderGeneration(1),
        ));
        provider
            .accept(policy(1, "member-a", "shell"))
            .expect_err("a policy naming another provider must be refused");
    }

    #[test]
    fn the_child_policy_init_parameter_reads_absent_unmanaged_and_provider() {
        use meerkat_core::ApplicationToolPolicyBinding;
        assert_eq!(
            child_application_tool_policy_from_init_params(&serde_json::json!({})).expect("absent"),
            None
        );
        assert_eq!(
            child_application_tool_policy_from_init_params(&serde_json::json!({
                "child_application_tool_policy": {"kind": "unmanaged"}
            }))
            .expect("unmanaged"),
            Some(ApplicationToolPolicyBinding::Unmanaged)
        );
        assert_eq!(
            child_application_tool_policy_from_init_params(&serde_json::json!({
                "child_application_tool_policy": {
                    "kind": "provider",
                    "provider_id": "example-provider",
                    "policy_id": "team-tools"
                }
            }))
            .expect("provider"),
            Some(ApplicationToolPolicyBinding::Provider {
                provider_id: PolicyProviderId::new("example-provider").expect("provider id"),
                policy_id: PolicyId::new("team-tools").expect("policy id"),
            })
        );
    }

    #[test]
    fn a_malformed_or_inherit_child_policy_refuses_the_boot() {
        for value in [
            serde_json::Value::Null,
            serde_json::json!("unmanaged"),
            serde_json::json!({"kind": "provider", "provider_id": "example-provider"}),
            serde_json::json!({"kind": "inherit"}),
        ] {
            let params = serde_json::json!({ "child_application_tool_policy": value });
            let error = child_application_tool_policy_from_init_params(&params)
                .expect_err("a value that is not a usable child binding must be refused");
            assert!(
                error.to_string().contains("child_application_tool_policy"),
                "{value}: {error}"
            );
        }
    }

    #[test]
    fn a_child_policy_must_name_a_served_provider_and_policy() {
        use meerkat_core::ApplicationToolPolicyBinding;
        let providers: Vec<Arc<dyn ToolConsequenceNarrowingPolicy>> =
            providers_from_canonical_payloads(&[policy_for(
                "example-provider",
                "team-tools",
                "member-a",
                "shell",
            )])
            .expect("providers")
            .into_iter()
            .map(|provider| provider as Arc<dyn ToolConsequenceNarrowingPolicy>)
            .collect();
        let binding = |provider: &str, policy: &str| ApplicationToolPolicyBinding::Provider {
            provider_id: PolicyProviderId::new(provider).expect("provider id"),
            policy_id: PolicyId::new(policy).expect("policy id"),
        };
        validate_child_application_tool_policy(
            &binding("example-provider", "team-tools"),
            &providers,
        )
        .expect("a served provider and policy");
        validate_child_application_tool_policy(&ApplicationToolPolicyBinding::Unmanaged, &[])
            .expect("unmanaged needs no provider");
        let error =
            validate_child_application_tool_policy(&binding("other", "team-tools"), &providers)
                .expect_err("an unserved provider");
        assert!(
            error
                .to_string()
                .contains("which no application_tool_policies entry carries")
        );
        let error = validate_child_application_tool_policy(
            &binding("example-provider", "guest"),
            &providers,
        )
        .expect_err("a policy the provider never accepted");
        assert!(error.to_string().contains("cannot load"), "{error}");
        validate_child_application_tool_policy(&binding("example-provider", "team-tools"), &[])
            .expect_err("a provider binding without any served policy");
    }
}
