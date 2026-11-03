//! Historical native association plus Toolkit-only proposal constraints.
//!
//! Deserializing or comparing this evidence authenticates nothing. Runtime
//! admission owns association authentication and native operation owners must
//! prepare and execute each privileged effect through WorkAuthorizationContext.

use super::{Action, Ceiling};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, NativeWorkTarget, OriginalWorkRef,
};
use meerkat_core::authorization::WorkAuthorizationContext;
use meerkat_core::connection::RealmId;
use serde::{Deserialize, Deserializer, Serialize};
use toolkit_package::{ContentDigest, Id, Revision, canonical_json};

const VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SetupContextError {
    #[error("setup evidence encoding is invalid")]
    InvalidEncoding,
    #[error("legacy signed request roots require native ingress migration")]
    MigrationRequired,
    #[error("setup evidence is inconsistent")]
    InvalidEvidence,
    #[error("setup evidence belongs to another native work or host")]
    WrongContext,
    #[error("setup proposal has expired")]
    Expired,
    #[error("setup proposal refers to another policy revision")]
    StalePolicy,
    #[error("requested proposal is outside the setup constraints")]
    Denied,
}

/// Application-selected setup limits, not native permission. Persisted values
/// are historical comparisons against the current trusted host context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupProposalConstraints {
    pub tenant: Id,
    pub deployment: Id,
    pub policy_revision: Revision,
    pub issued_at_ms: u64,
    pub expires_at_ms: u64,
    pub ceiling: Ceiling,
}

/// Data only. There is no signature, verified variant, issuance or custody API.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRequestEvidence {
    version: u16,
    association: InputAuthorityAssociation,
    constraints: SetupProposalConstraints,
}

impl std::fmt::Debug for SetupRequestEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SetupRequestEvidence")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl<'de> Deserialize<'de> for SetupRequestEvidence {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Preserve the exact JSON token stream so duplicate fields cannot be
        // discarded by an intermediate Value before strict native decoding.
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        #[derive(Deserialize)]
        struct Version {
            version: u16,
        }
        let version: Version = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom(SetupContextError::InvalidEncoding))?;
        if version.version != VERSION {
            return Err(serde::de::Error::custom(
                SetupContextError::MigrationRequired,
            ));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u16,
            association: InputAuthorityAssociation,
            constraints: SetupProposalConstraints,
        }
        let wire: Wire = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom(SetupContextError::InvalidEncoding))?;
        if wire.version != VERSION {
            return Err(serde::de::Error::custom(
                SetupContextError::MigrationRequired,
            ));
        }
        Self::new(wire.association, wire.constraints).map_err(serde::de::Error::custom)
    }
}

impl SetupRequestEvidence {
    pub fn new(
        association: InputAuthorityAssociation,
        constraints: SetupProposalConstraints,
    ) -> Result<Self, SetupContextError> {
        if constraints.issued_at_ms >= constraints.expires_at_ms {
            return Err(SetupContextError::InvalidEvidence);
        }
        Ok(Self {
            version: VERSION,
            association,
            constraints,
        })
    }
    pub fn association(&self) -> &InputAuthorityAssociation {
        &self.association
    }
    pub fn constraints(&self) -> &SetupProposalConstraints {
        &self.constraints
    }
    pub fn original_work(&self) -> &OriginalWorkRef {
        &self.association.candidate().original_work
    }
    pub fn digest(&self) -> Result<ContentDigest, SetupContextError> {
        Ok(ContentDigest::of_bytes(
            &canonical_json(self).map_err(|_| SetupContextError::InvalidEncoding)?,
        ))
    }
    /// Exact historical identity and proposal checks only. In particular this
    /// does not call native prepare, cache a permission, or create a runnable.
    pub fn check_context(&self, current: &SetupUseContext<'_>) -> Result<(), SetupContextError> {
        let c = &self.constraints;
        let a = self.association.candidate();
        if &c.tenant != current.tenant
            || &c.deployment != current.deployment
            || &a.ingress_namespace.realm != current.realm
            || &a.target != current.target
        {
            return Err(SetupContextError::WrongContext);
        }
        // Exact retained_work::identity_from encoding in the selected native
        // development source. The association binary canonical codec is a
        // different format. This comparison grants no native permission.
        let retained_bytes = serde_json::to_vec(&self.association)
            .map_err(|_| SetupContextError::InvalidEncoding)?;
        let association_digest = ContentDigest::of_bytes(&retained_bytes);
        let retained = current
            .authorization
            .retained_work()
            .ok_or(SetupContextError::WrongContext)?;
        if !retained
            .contributors()
            .iter()
            .any(|item| item.association_digest == association_digest.as_str())
        {
            return Err(SetupContextError::WrongContext);
        }
        if &c.policy_revision != current.policy_revision {
            return Err(SetupContextError::StalePolicy);
        }
        if current.now_ms < c.issued_at_ms || current.now_ms >= c.expires_at_ms {
            return Err(SetupContextError::Expired);
        }
        Ok(())
    }
    /// A proposal ceiling can only reject; it cannot grant a native operation.
    pub fn check_proposal(
        &self,
        action: Action,
        resource: &Id,
        current: &SetupUseContext<'_>,
    ) -> Result<(), SetupContextError> {
        self.check_context(current)?;
        if !self.constraints.ceiling.permits(action, resource) {
            return Err(SetupContextError::Denied);
        }
        Ok(())
    }
}

/// Trusted current runtime/host inputs, never decoded from saved setup data.
/// The current native owner is forwarded to concrete reference/effect owners;
/// possession of this struct or passing a proposal check grants nothing.
pub struct SetupUseContext<'a> {
    pub tenant: &'a Id,
    pub deployment: &'a Id,
    pub realm: &'a RealmId,
    pub target: &'a NativeWorkTarget,
    pub policy_revision: &'a Revision,
    pub now_ms: u64,
    pub authorization: &'a WorkAuthorizationContext,
}

pub(crate) mod principal_codec {
    use meerkat_core::auth::{PrincipalId, PrincipalKind, PrincipalQualification, PrincipalRef};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(
        value: &PrincipalRef,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PrincipalRef, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WirePrincipalRef {
            kind: PrincipalKind,
            id: PrincipalId,
            #[serde(default)]
            qualification: PrincipalQualification,
        }

        let wire = WirePrincipalRef::deserialize(deserializer)?;
        let value = PrincipalRef {
            kind: wire.kind,
            id: wire.id,
            qualification: wire.qualification,
        };
        value
            .validate_qualified()
            .map_err(|_| serde::de::Error::custom("qualified native principal required"))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests;
