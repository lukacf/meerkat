//! Principal, grant, and visibility contracts.
//!
//! These types are policy inputs, not policy execution engines. Runtime and
//! machine-owned flows can consume them without treating labels or app context
//! as authorization truth.

use crate::SurfaceMetadata;
use crate::connection::{BindingId, ProfileId, RealmId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Validation errors for principal/grant/visibility contracts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalContractError {
    #[error("principal id must not be empty")]
    EmptyPrincipalId,
    #[error("principal id must not contain control characters")]
    InvalidPrincipalId,
    #[error("application scope namespace must not be empty")]
    EmptyApplicationScopeNamespace,
    #[error("application scope id must not be empty")]
    EmptyApplicationScopeId,
    #[error("trust domain id must not be empty")]
    EmptyTrustDomainId,
    #[error("trust domain id must not contain control characters")]
    InvalidTrustDomainId,
    #[error("principal must name an explicit trust domain")]
    UnqualifiedPrincipal,
}

/// Stable, caller-visible principal id.
///
/// Legacy deserialization preserves previously persisted strings, including
/// those rejected by [`Self::new`]. Qualified [`PrincipalRef`] deserialization
/// validates the id; migrating unqualified persisted ids is a separate step.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalContractError> {
        let value = value.into();
        Self::validate_value(&value)?;
        Ok(Self(value))
    }

    fn validate_value(value: &str) -> Result<(), PrincipalContractError> {
        if value.trim().is_empty() {
            return Err(PrincipalContractError::EmptyPrincipalId);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalContractError::InvalidPrincipalId);
        }
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Exact, opaque trust-domain namespace for a qualified principal id.
///
/// Domain ids are not inferred from hostnames or normalized. Constructing one
/// validates its syntax, not the caller's authority to assert that domain.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(transform = validated_identity_string_schema))]
#[serde(transparent)]
pub struct TrustDomainId(String);

impl TrustDomainId {
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalContractError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PrincipalContractError::EmptyTrustDomainId);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalContractError::InvalidTrustDomainId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TrustDomainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for TrustDomainId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Whether an identity has an explicit trust-domain namespace.
///
/// Unqualified identity preserves trusted-embedded and legacy wire contracts;
/// it must not be silently assigned a domain for governed use. Qualification
/// is identity vocabulary, not proof of authentication or authorization.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "trust_domain_id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PrincipalQualification {
    #[default]
    Unqualified,
    Qualified(TrustDomainId),
}

impl PrincipalQualification {
    #[must_use]
    pub fn is_unqualified(&self) -> bool {
        matches!(self, Self::Unqualified)
    }
}

impl<'de> Deserialize<'de> for PrincipalQualification {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // An empty struct variant rejects a present payload, including null.
        // Serde's adjacent-tag unit variant otherwise accepts a null payload.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum WireQualification {
            Unqualified {},
            Qualified { trust_domain_id: TrustDomainId },
        }

        match WireQualification::deserialize(deserializer)? {
            WireQualification::Unqualified {} => Ok(Self::Unqualified),
            WireQualification::Qualified { trust_domain_id } => {
                Ok(Self::Qualified(trust_domain_id))
            }
        }
    }
}

#[cfg(feature = "schema")]
fn validated_identity_string_constraints() -> serde_json::Value {
    use std::fmt::Write;
    use std::sync::OnceLock;

    static CHARACTER_CLASSES: OnceLock<(String, String)> = OnceLock::new();
    let (whitespace, controls) = CHARACTER_CLASSES.get_or_init(|| {
        let mut whitespace = String::new();
        let mut controls = String::new();
        // Derive the schema from the same Unicode predicates as constructors.
        // Regex shorthand whitespace and end anchors differ between engines.
        for character in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            for (matches, output) in [
                (character.is_whitespace(), &mut whitespace),
                (character.is_control(), &mut controls),
            ] {
                if matches {
                    if character <= '\u{ffff}' {
                        let _ = write!(output, "\\u{:04x}", u32::from(character));
                    } else {
                        output.push(character);
                    }
                }
            }
        }
        (whitespace, controls)
    });
    serde_json::json!([
        { "pattern": format!("[^{whitespace}]") },
        { "not": { "pattern": format!("[{controls}]") } },
    ])
}

#[cfg(feature = "schema")]
fn validated_identity_string_schema(schema: &mut schemars::Schema) {
    schema.insert("allOf".to_owned(), validated_identity_string_constraints());
}

#[cfg(feature = "schema")]
fn qualified_principal_schema(schema: &mut schemars::Schema) {
    schema.insert(
        "allOf".to_owned(),
        serde_json::json!([{
            "if": {
                "required": ["qualification"],
                "properties": {
                    "qualification": {
                        "required": ["kind"],
                        "properties": { "kind": { "const": "qualified" } }
                    }
                }
            },
            "then": { "properties": { "id": { "allOf": validated_identity_string_constraints() } } }
        }]),
    );
}

/// Generic principal categories. Product-specific role names belong outside
/// core and may be mapped to these typed categories by clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    PersonalAgent,
    SharedAgent,
    RuntimeHost,
    ServiceAccount,
}

/// Typed principal reference.
///
/// Public fields preserve the existing data contract, so an in-memory value
/// is not proof of valid or authenticated identity. Governed admission must
/// call [`Self::validate_qualified`] and separately establish caller authority.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(transform = qualified_principal_schema))]
pub struct PrincipalRef {
    pub kind: PrincipalKind,
    pub id: PrincipalId,
    #[serde(
        default,
        skip_serializing_if = "PrincipalQualification::is_unqualified"
    )]
    pub qualification: PrincipalQualification,
}

impl PrincipalRef {
    /// Construct an explicitly unqualified trusted-embedded identity.
    pub fn new(kind: PrincipalKind, id: impl Into<String>) -> Result<Self, PrincipalContractError> {
        Ok(Self {
            kind,
            id: PrincipalId::new(id)?,
            qualification: PrincipalQualification::Unqualified,
        })
    }

    /// Construct a qualified identity with validated ids.
    ///
    /// The authenticated boundary remains responsible for proving this
    /// identity. Merely constructing it does not grant authority.
    pub fn in_domain(
        kind: PrincipalKind,
        id: impl Into<String>,
        domain: TrustDomainId,
    ) -> Result<Self, PrincipalContractError> {
        Ok(Self {
            kind,
            id: PrincipalId::new(id)?,
            qualification: PrincipalQualification::Qualified(domain),
        })
    }

    /// Require an explicit domain and validate the current principal id.
    ///
    /// This also checks values assembled or mutated through public fields
    /// after legacy deserialization. It does not authenticate the principal or
    /// authorize its claimed domain; the admitting owner must establish both.
    pub fn validate_qualified(&self) -> Result<(), PrincipalContractError> {
        if self.qualification.is_unqualified() {
            return Err(PrincipalContractError::UnqualifiedPrincipal);
        }
        PrincipalId::validate_value(self.id.as_str())
    }
}

impl<'de> Deserialize<'de> for PrincipalRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct WirePrincipalRef {
            kind: PrincipalKind,
            id: PrincipalId,
            #[serde(default)]
            qualification: PrincipalQualification,
        }

        let wire = WirePrincipalRef::deserialize(deserializer)?;
        match wire.qualification {
            PrincipalQualification::Unqualified => Ok(Self {
                kind: wire.kind,
                id: wire.id,
                qualification: PrincipalQualification::Unqualified,
            }),
            PrincipalQualification::Qualified(domain) => {
                Self::in_domain(wire.kind, wire.id.as_str(), domain)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

/// Explicit acting-on-behalf-of relationship for audit and policy checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ActingOnBehalfOf {
    pub actor: PrincipalRef,
    pub subject: PrincipalRef,
}

impl ActingOnBehalfOf {
    #[must_use]
    pub fn new(actor: PrincipalRef, subject: PrincipalRef) -> Self {
        Self { actor, subject }
    }
}

/// Generic scope for grants and shared visibility.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "scope_type", rename_all = "snake_case")]
pub enum GrantScope {
    Realm {
        realm_id: String,
    },
    Session {
        session_id: String,
    },
    Mob {
        mob_id: String,
    },
    /// Exact realm credential binding authorized for use. The optional
    /// profile is part of the scope so a grant for the binding's default auth
    /// profile cannot authorize a caller-selected override profile.
    AuthBinding {
        realm_id: RealmId,
        binding_id: BindingId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        profile_id: Option<ProfileId>,
    },
    /// Product-neutral extension point. The typed namespace/id pair is policy
    /// input; labels/app context are still not authority.
    Application {
        namespace: String,
        id: String,
    },
}

impl GrantScope {
    pub fn application(
        namespace: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, PrincipalContractError> {
        let namespace = namespace.into();
        let id = id.into();
        if namespace.trim().is_empty() {
            return Err(PrincipalContractError::EmptyApplicationScopeNamespace);
        }
        if id.trim().is_empty() {
            return Err(PrincipalContractError::EmptyApplicationScopeId);
        }
        Ok(Self::Application { namespace, id })
    }

    #[must_use]
    pub fn matches(&self, requested: &Self) -> bool {
        self == requested
    }
}

/// Typed visibility class for events, artifacts, approvals, or future records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "visibility", rename_all = "snake_case")]
pub enum VisibilityClass {
    Private { principal: PrincipalRef },
    Scoped { scope: GrantScope },
}

/// Actions that grants may allow. Enforcement sites decide which action is
/// needed for a specific operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GrantAction {
    Observe,
    ReplayEvents,
    RequestApproval,
    DecideApproval,
    UseTool,
    /// Use one exact realm credential binding on behalf of a durable target.
    ///
    /// The enforcement seam also requires an exact
    /// [`ActingOnBehalfOf`] match. Binding existence alone never grants this
    /// action.
    UseAuthBinding,
    ManageRuntime,
}

/// A typed grant issued to a principal for a single scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthGrant {
    pub principal: PrincipalRef,
    pub scope: GrantScope,
    pub actions: BTreeSet<GrantAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_on_behalf_of: Option<ActingOnBehalfOf>,
}

impl AuthGrant {
    #[must_use]
    pub fn allows(
        &self,
        principal: &PrincipalRef,
        action: GrantAction,
        scope: &GrantScope,
        acting_on_behalf_of: Option<&ActingOnBehalfOf>,
    ) -> bool {
        if &self.principal != principal {
            return false;
        }
        if !self.actions.contains(&action) {
            return false;
        }
        if !self.scope.matches(scope) {
            return false;
        }
        self.acting_on_behalf_of.as_ref() == acting_on_behalf_of
    }
}

/// Evaluate observation of a typed visibility class.
///
/// Private visibility is intentionally not broadened by scoped grants. A
/// private record is visible only to the exact principal named by the record.
#[must_use]
pub fn can_observe_visibility(
    principal: &PrincipalRef,
    grants: &[AuthGrant],
    visibility: &VisibilityClass,
) -> bool {
    match visibility {
        VisibilityClass::Private { principal: owner } => owner == principal,
        VisibilityClass::Scoped { scope } => grants.iter().any(|grant| {
            grant.allows(principal, GrantAction::Observe, scope, None)
                || grant.allows(principal, GrantAction::ReplayEvents, scope, None)
        }),
    }
}

/// Defensive helper that proves caller metadata is not interpreted as
/// authorization material.
#[must_use]
pub fn metadata_grants_no_visibility(
    principal: &PrincipalRef,
    grants: &[AuthGrant],
    visibility: &VisibilityClass,
    _metadata: &SurfaceMetadata,
) -> bool {
    can_observe_visibility(principal, grants, visibility)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn human(id: &str) -> PrincipalRef {
        PrincipalRef::new(PrincipalKind::Human, id).expect("valid human principal")
    }

    fn personal_agent(id: &str) -> PrincipalRef {
        PrincipalRef::new(PrincipalKind::PersonalAgent, id).expect("valid agent principal")
    }

    fn grant(
        principal: PrincipalRef,
        scope: GrantScope,
        actions: impl IntoIterator<Item = GrantAction>,
    ) -> AuthGrant {
        AuthGrant {
            principal,
            scope,
            actions: actions.into_iter().collect(),
            acting_on_behalf_of: None,
        }
    }

    #[test]
    fn invalid_empty_principal_ids_are_rejected() {
        assert!(matches!(
            PrincipalId::new(""),
            Err(PrincipalContractError::EmptyPrincipalId)
        ));
        assert!(matches!(
            PrincipalId::new(" \t "),
            Err(PrincipalContractError::EmptyPrincipalId)
        ));
        assert!(matches!(
            PrincipalId::new("human:\nmallory"),
            Err(PrincipalContractError::InvalidPrincipalId)
        ));
    }

    fn qualified_human(id: &str, domain: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::Human,
            id,
            TrustDomainId::new(domain).expect("valid domain"),
        )
        .expect("valid qualified principal")
    }

    #[test]
    fn trust_domain_constructor_and_deserializer_validate_without_normalization() {
        for invalid in ["", " \t ", "domain\nother", "domain\u{007f}"] {
            assert!(TrustDomainId::new(invalid).is_err());
            assert!(serde_json::from_value::<TrustDomainId>(json!(invalid)).is_err());
        }
        let domain = TrustDomainId::new("Domain:One").expect("valid domain");
        assert_eq!(domain.as_str(), "Domain:One");
        assert_eq!(domain.to_string(), "Domain:One");
        assert_eq!(
            serde_json::to_value(&domain).expect("serialize"),
            json!("Domain:One")
        );
        assert_ne!(
            domain,
            TrustDomainId::new("domain:one").expect("valid domain")
        );
    }

    #[test]
    fn qualification_is_part_of_principal_identity_and_visibility() {
        let local = human("alice");
        let domain_a = qualified_human("alice", "domain:a");
        let domain_b = qualified_human("alice", "domain:b");
        let identities = BTreeSet::from([local.clone(), domain_a.clone(), domain_b.clone()]);
        assert_eq!(identities.len(), 3);
        assert!(local.qualification.is_unqualified());
        let private = VisibilityClass::Private {
            principal: domain_a.clone(),
        };
        assert!(can_observe_visibility(&domain_a, &[], &private));
        assert!(!can_observe_visibility(&domain_b, &[], &private));
        assert!(!can_observe_visibility(&local, &[], &private));
    }

    #[test]
    fn grants_require_exact_principal_and_delegation_domains() {
        let actor = qualified_human("alice", "domain:a");
        let foreign_actor = qualified_human("alice", "domain:b");
        let subject = qualified_human("bob", "domain:a");
        let relationship = ActingOnBehalfOf::new(actor.clone(), subject);
        let scope = GrantScope::application("project", "one").expect("scope");
        let mut grant = grant(actor.clone(), scope.clone(), [GrantAction::Observe]);
        assert!(grant.allows(&actor, GrantAction::Observe, &scope, None));
        assert!(!grant.allows(&foreign_actor, GrantAction::Observe, &scope, None));
        assert!(!grant.allows(&human("alice"), GrantAction::Observe, &scope, None));
        grant.acting_on_behalf_of = Some(relationship.clone());
        assert!(grant.allows(&actor, GrantAction::Observe, &scope, Some(&relationship)));
        let foreign_subject =
            ActingOnBehalfOf::new(actor.clone(), qualified_human("bob", "domain:b"));
        assert!(!grant.allows(&actor, GrantAction::Observe, &scope, Some(&foreign_subject)));
        let foreign_relationship = ActingOnBehalfOf::new(foreign_actor, relationship.subject);
        assert!(!grant.allows(
            &actor,
            GrantAction::Observe,
            &scope,
            Some(&foreign_relationship)
        ));
    }

    #[test]
    fn qualified_principal_decode_and_construction_reject_invalid_ids() {
        for invalid in ["", " \t ", "alice\nother", "alice\u{007f}"] {
            assert!(
                PrincipalRef::in_domain(
                    PrincipalKind::Human,
                    invalid,
                    TrustDomainId::new("domain:a").expect("domain"),
                )
                .is_err()
            );
            assert!(
                serde_json::from_value::<PrincipalRef>(json!({
                    "kind": "human",
                    "id": invalid,
                    "qualification": { "kind": "qualified", "trust_domain_id": "domain:a" }
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn qualified_validation_rechecks_public_field_construction() {
        assert_eq!(
            human("alice").validate_qualified(),
            Err(PrincipalContractError::UnqualifiedPrincipal)
        );
        assert!(
            qualified_human("alice", "domain:a")
                .validate_qualified()
                .is_ok()
        );
        for id in ["", " \t ", "alice\nother"] {
            let mut legacy: PrincipalRef =
                serde_json::from_value(json!({"kind": "human", "id": id}))
                    .expect("legacy principal remains decodable");
            legacy.qualification = PrincipalQualification::Qualified(
                TrustDomainId::new("domain:a").expect("valid test trust domain"),
            );
            assert!(legacy.validate_qualified().is_err());
        }
    }

    #[test]
    fn malformed_qualification_cannot_downgrade_to_unqualified() {
        for qualification in [
            json!(null),
            json!({}),
            json!({"kind": "future"}),
            json!({"kind": "unqualified", "trust_domain_id": "domain:a"}),
            json!({"kind": "unqualified", "trust_domain_id": null}),
            json!({"kind": "qualified"}),
            json!({"kind": "qualified", "trust_domain_id": ""}),
            json!({"kind": "qualified", "trust_domain_id": "a\nb"}),
            json!({"kind": "qualified", "trust_domain_id": null}),
            json!({"kind": "qualified", "trust_domain_id": "domain:a", "extra": true}),
        ] {
            assert!(
                serde_json::from_value::<PrincipalRef>(json!({
                    "kind": "human", "id": "alice", "qualification": qualification,
                }))
                .is_err()
            );
        }
    }

    #[test]
    fn private_and_application_visibility_do_not_overlap() {
        let alice = human("human:alice");
        let bob = human("human:bob");
        let project_scope =
            GrantScope::application("client.project", "repo-a").expect("valid scope");
        let bob_project_grant = grant(
            bob.clone(),
            project_scope,
            [GrantAction::Observe, GrantAction::ReplayEvents],
        );

        let alice_private = VisibilityClass::Private {
            principal: alice.clone(),
        };

        assert!(can_observe_visibility(&alice, &[], &alice_private));
        assert!(!can_observe_visibility(
            &bob,
            &[bob_project_grant],
            &alice_private
        ));
    }

    #[test]
    fn grant_scope_mismatch_denies_visibility() {
        let bob = human("human:bob");
        let session_scope = GrantScope::Session {
            session_id: "session-1".into(),
        };
        let mob_scope = GrantScope::Mob {
            mob_id: "mob-1".into(),
        };
        let bob_session_grant = grant(bob.clone(), session_scope, [GrantAction::Observe]);
        let mob_visibility = VisibilityClass::Scoped { scope: mob_scope };

        assert!(!can_observe_visibility(
            &bob,
            &[bob_session_grant],
            &mob_visibility
        ));
    }

    #[test]
    fn acting_on_behalf_of_is_typed_and_exact() {
        let bob = human("human:bob");
        let bob_agent = personal_agent("agent:bob-personal");
        let alice = human("human:alice");
        let scope = GrantScope::Session {
            session_id: "session-1".into(),
        };
        let bob_relationship = ActingOnBehalfOf::new(bob_agent.clone(), bob);
        let alice_relationship = ActingOnBehalfOf::new(bob_agent.clone(), alice);
        let grant = AuthGrant {
            principal: bob_agent.clone(),
            scope: scope.clone(),
            actions: BTreeSet::from([GrantAction::RequestApproval]),
            acting_on_behalf_of: Some(bob_relationship.clone()),
        };

        assert!(grant.allows(
            &bob_agent,
            GrantAction::RequestApproval,
            &scope,
            Some(&bob_relationship)
        ));
        assert!(!grant.allows(&bob_agent, GrantAction::RequestApproval, &scope, None));
        assert!(!grant.allows(
            &bob_agent,
            GrantAction::RequestApproval,
            &scope,
            Some(&alice_relationship)
        ));
    }

    #[test]
    fn metadata_is_not_visibility_authority() {
        let bob = human("human:bob");
        let visibility = VisibilityClass::Scoped {
            scope: GrantScope::application("client.project", "repo-a").expect("valid scope"),
        };
        let metadata = SurfaceMetadata::from_optional_parts(
            Some(BTreeMap::from([
                (
                    "visibility".to_string(),
                    "client.project:repo-a".to_string(),
                ),
                ("principal".to_string(), "human:bob".to_string()),
            ])),
            Some(json!({
                "grant": {
                    "action": "observe",
                    "scope": "client.project:repo-a"
                }
            })),
        );

        assert!(!metadata_grants_no_visibility(
            &bob,
            &[],
            &visibility,
            &metadata
        ));
    }
}
