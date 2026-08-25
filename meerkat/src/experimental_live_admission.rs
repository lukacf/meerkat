//! Provider-neutral admission for pre-release live execution factories.
//!
//! Compiling an experimental transport is necessary but insufficient. The
//! facade mints an admission witness only when the build contains the feature,
//! an operator selected one exact factory version, the durable realm admits
//! it, and this build carries matching Gate0 qualification evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use meerkat_client::FactoryError;
use meerkat_core::{ModelProfileWitness, ModelReleaseStage, Provider, RealmId, SessionLlmIdentity};
use meerkat_providers::ResolvedConnection;
use thiserror::Error;

const CURRENT_BUILD_VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_GATE0_STATUS: Option<&str> = option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_STATUS");
const BUILD_GATE0_FACTORY_KIND: Option<&str> =
    option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_FACTORY_KIND");
const BUILD_GATE0_FACTORY_VERSION: Option<&str> =
    option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_FACTORY_VERSION");
const BUILD_GATE0_QUALIFICATION_VERSION: Option<&str> =
    option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_QUALIFICATION_VERSION");
const BUILD_GATE0_BUILD_VERSION: Option<&str> =
    option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_BUILD_VERSION");
const BUILD_GATE0_PROTOCOL_DIGEST: Option<&str> =
    option_env!("MEERKAT_EXPERIMENTAL_LIVE_GATE0_PROTOCOL_DIGEST");

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_protocol_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Exact implementation identity selected by operator configuration.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExperimentalLiveFactoryIdentity {
    kind: String,
    version: String,
}

impl ExperimentalLiveFactoryIdentity {
    /// Parse a factory identity without assigning provider semantics to it.
    pub fn parse(
        kind: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, ExperimentalLiveAdmissionError> {
        let kind = kind.into();
        if !valid_component(&kind) {
            return Err(ExperimentalLiveAdmissionError::InvalidFactoryKind);
        }
        let version = version.into();
        if !valid_component(&version) {
            return Err(ExperimentalLiveAdmissionError::InvalidFactoryVersion);
        }
        Ok(Self { kind, version })
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

impl fmt::Debug for ExperimentalLiveFactoryIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalLiveFactoryIdentity")
            .field("kind", &self.kind)
            .field("version", &self.version)
            .finish()
    }
}

/// Version of the external-contract proof required by operator policy.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExperimentalLiveGate0QualificationVersion(String);

impl ExperimentalLiveGate0QualificationVersion {
    pub fn parse(value: impl Into<String>) -> Result<Self, ExperimentalLiveAdmissionError> {
        let value = value.into();
        if !valid_component(&value) {
            return Err(ExperimentalLiveAdmissionError::InvalidGate0QualificationVersion);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ExperimentalLiveGate0QualificationVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Gate0QualificationVersion")
            .field(&self.0)
            .finish()
    }
}

/// Explicit operator intent for one experimental live factory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExperimentalLiveOperatorConfig {
    factory: ExperimentalLiveFactoryIdentity,
    required_gate0: ExperimentalLiveGate0QualificationVersion,
}

impl ExperimentalLiveOperatorConfig {
    pub fn new(
        factory: ExperimentalLiveFactoryIdentity,
        required_gate0: ExperimentalLiveGate0QualificationVersion,
    ) -> Self {
        Self {
            factory,
            required_gate0,
        }
    }

    pub fn factory(&self) -> &ExperimentalLiveFactoryIdentity {
        &self.factory
    }

    pub fn required_gate0(&self) -> &ExperimentalLiveGate0QualificationVersion {
        &self.required_gate0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QualifiedGate0BuildWitness {
    factory: ExperimentalLiveFactoryIdentity,
    qualification: ExperimentalLiveGate0QualificationVersion,
    build_version: String,
    protocol_digest: String,
}

impl QualifiedGate0BuildWitness {
    fn current_build() -> Option<Self> {
        if BUILD_GATE0_STATUS != Some("qualified") {
            return None;
        }
        let witness = Self {
            factory: ExperimentalLiveFactoryIdentity::parse(
                BUILD_GATE0_FACTORY_KIND?,
                BUILD_GATE0_FACTORY_VERSION?,
            )
            .ok()?,
            qualification: ExperimentalLiveGate0QualificationVersion::parse(
                BUILD_GATE0_QUALIFICATION_VERSION?,
            )
            .ok()?,
            build_version: BUILD_GATE0_BUILD_VERSION?.to_string(),
            protocol_digest: BUILD_GATE0_PROTOCOL_DIGEST?.to_ascii_lowercase(),
        };
        valid_protocol_digest(&witness.protocol_digest).then_some(witness)
    }
}

/// Side-effect-free process and realm qualification for the experimental
/// live capability.
///
/// This proof contains no selected model, auth binding, credential, or
/// per-open target. It can therefore drive capability advertisement without
/// resolving a credential or constructing a provider factory.
pub struct ExperimentalLiveCapabilityQualification {
    authority: Arc<AdmissionAuthority>,
    realm: RealmId,
    factory: ExperimentalLiveFactoryIdentity,
    gate0_qualification: ExperimentalLiveGate0QualificationVersion,
    gate0_build_version: String,
    protocol_digest: String,
    lower_qualification:
        Option<meerkat_llm_core::provider_runtime::ExperimentalRealtimeQualificationWitness>,
}

impl fmt::Debug for ExperimentalLiveCapabilityQualification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalLiveCapabilityQualification")
            .field("realm", &self.realm)
            .field("factory", &self.factory)
            .field("gate0_qualification", &self.gate0_qualification)
            .field("gate0_build_version", &self.gate0_build_version)
            .field("protocol_digest", &self.protocol_digest)
            .finish()
    }
}

#[derive(Debug)]
struct AdmissionAuthority;

/// Factory-owned admission authority.
///
/// `Default` is intentionally unusable. Configuration does not bypass the
/// compile or Gate0 predicates: both remain facts of the compiled artifact.
#[derive(Clone)]
pub struct ExperimentalLiveAdmissionOwner {
    authority: Arc<AdmissionAuthority>,
    feature_compiled: bool,
    operator: Option<ExperimentalLiveOperatorConfig>,
    admitted_realms: BTreeSet<RealmId>,
    gate0_build: Option<QualifiedGate0BuildWitness>,
    lower_authorities: BTreeMap<
        RealmId,
        meerkat_llm_core::provider_runtime::ExperimentalRealtimeAdmissionAuthority,
    >,
}

impl fmt::Debug for ExperimentalLiveAdmissionOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalLiveAdmissionOwner")
            .field("feature_compiled", &self.feature_compiled)
            .field("operator_configured", &self.operator.is_some())
            .field("admitted_realms", &self.admitted_realms)
            .field("gate0_build_qualified", &self.gate0_build.is_some())
            .finish()
    }
}

impl Default for ExperimentalLiveAdmissionOwner {
    fn default() -> Self {
        Self {
            authority: Arc::new(AdmissionAuthority),
            feature_compiled: cfg!(feature = "experimental-gpt-live"),
            operator: None,
            admitted_realms: BTreeSet::new(),
            gate0_build: QualifiedGate0BuildWitness::current_build(),
            lower_authorities: BTreeMap::new(),
        }
    }
}

impl ExperimentalLiveAdmissionOwner {
    #[cfg(test)]
    pub(crate) fn qualified_without_lower_authority_for_test(
        realm: RealmId,
        factory: ExperimentalLiveFactoryIdentity,
    ) -> Self {
        let qualification = ExperimentalLiveGate0QualificationVersion("gate0-v1".to_string());
        Self {
            authority: Arc::new(AdmissionAuthority),
            feature_compiled: true,
            operator: Some(ExperimentalLiveOperatorConfig::new(
                factory.clone(),
                qualification.clone(),
            )),
            admitted_realms: BTreeSet::from([realm]),
            gate0_build: Some(QualifiedGate0BuildWitness {
                factory,
                qualification,
                build_version: CURRENT_BUILD_VERSION.to_string(),
                protocol_digest: "ab".repeat(32),
            }),
            lower_authorities: BTreeMap::new(),
        }
    }

    /// Configure exact operator and realm intent for this compiled artifact.
    pub fn configured_for_current_build(
        operator: ExperimentalLiveOperatorConfig,
        admitted_realms: impl IntoIterator<Item = RealmId>,
    ) -> Self {
        let admitted_realms: BTreeSet<_> = admitted_realms.into_iter().collect();
        let lower_authorities = admitted_realms
            .iter()
            .filter_map(|realm| {
                let policy = meerkat_llm_core::provider_runtime::ExperimentalRealtimeQualificationPolicy::new(
                    realm.clone(),
                    operator.factory.kind(),
                    operator.factory.version(),
                    operator.required_gate0.as_str(),
                )
                .ok()?;
                let authority = meerkat_llm_core::provider_runtime::ExperimentalRealtimeAdmissionAuthority::from_compiled_gate0_policy(policy).ok()?;
                Some((realm.clone(), authority))
            })
            .collect();
        Self {
            authority: Arc::new(AdmissionAuthority),
            feature_compiled: cfg!(feature = "experimental-gpt-live"),
            operator: Some(operator),
            admitted_realms,
            gate0_build: QualifiedGate0BuildWitness::current_build(),
            lower_authorities,
        }
    }

    /// Qualify capability advertisement without selecting a target, resolving
    /// a binding, reading credentials, or constructing provider state.
    pub fn qualify_capability(
        &self,
        realm: &RealmId,
        factory: &ExperimentalLiveFactoryIdentity,
    ) -> Result<ExperimentalLiveCapabilityQualification, ExperimentalLiveAdmissionError> {
        self.validate_predicates(realm)?;
        let operator = self
            .operator
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::OperatorNotConfigured)?;
        if &operator.factory != factory {
            return Err(ExperimentalLiveAdmissionError::OperatorFactoryMismatch);
        }
        let gate0 = self
            .gate0_build
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::Gate0BuildNotQualified)?;
        self.validate_gate0(operator, gate0, factory)?;
        let lower_qualification = match self.lower_authorities.get(realm) {
            Some(authority) => Some(
                authority
                    .qualify(realm, factory.kind(), factory.version())
                    .map_err(ExperimentalLiveAdmissionError::LowerAdmission)?,
            ),
            None if cfg!(test) => None,
            None => return Err(ExperimentalLiveAdmissionError::LowerAuthorityUnavailable),
        };
        Ok(ExperimentalLiveCapabilityQualification {
            authority: Arc::clone(&self.authority),
            realm: realm.clone(),
            factory: factory.clone(),
            gate0_qualification: gate0.qualification.clone(),
            gate0_build_version: gate0.build_version.clone(),
            protocol_digest: gate0.protocol_digest.clone(),
            lower_qualification,
        })
    }

    /// Check every non-credential admission predicate before an auth binding
    /// is materialized. The returned value is opaque outside this crate.
    pub(crate) fn preflight(
        &self,
        qualification: ExperimentalLiveCapabilityQualification,
        identity: SessionLlmIdentity,
        profile: ModelProfileWitness,
    ) -> Result<ExperimentalLiveAdmissionPreflight, ExperimentalLiveAdmissionError> {
        self.validate_qualification(&qualification)?;
        if !profile.matches_identity(&identity) {
            return Err(ExperimentalLiveAdmissionError::TargetProfileMismatch);
        }
        if profile.profile().release_stage != ModelReleaseStage::Experimental {
            return Err(ExperimentalLiveAdmissionError::TargetNotExperimental);
        }
        if !profile.profile().realtime {
            return Err(ExperimentalLiveAdmissionError::TargetNotRealtime);
        }
        Ok(ExperimentalLiveAdmissionPreflight {
            authority: qualification.authority,
            realm: qualification.realm,
            factory: qualification.factory,
            gate0_qualification: qualification.gate0_qualification,
            gate0_build_version: qualification.gate0_build_version,
            protocol_digest: qualification.protocol_digest,
            lower_qualification: qualification.lower_qualification,
            identity,
            profile,
        })
    }

    /// Complete admission after the separately authorized credential
    /// materialization step produced one exact resolved target.
    pub(crate) fn complete(
        &self,
        preflight: ExperimentalLiveAdmissionPreflight,
        connection: ResolvedConnection,
        binding_use_witness: meerkat_core::AuthBindingUseWitness,
    ) -> Result<ExperimentalLiveAdmissionWitness, ExperimentalLiveAdmissionError> {
        if !Arc::ptr_eq(&self.authority, &preflight.authority) {
            return Err(ExperimentalLiveAdmissionError::StaleAdmissionPreflight);
        }
        if preflight.identity.auth_binding.as_ref() != Some(binding_use_witness.auth_binding()) {
            return Err(ExperimentalLiveAdmissionError::BindingUseWitnessMismatch);
        }
        let target = meerkat_providers::ResolvedRealtimeTarget::new(
            preflight.identity,
            preflight.profile,
            connection,
        )
        .ok_or(ExperimentalLiveAdmissionError::ResolvedConnectionMismatch)?;
        let lower_qualification = preflight
            .lower_qualification
            .ok_or(ExperimentalLiveAdmissionError::LowerAuthorityUnavailable)?;
        let lower_authority = self
            .lower_authorities
            .get(&preflight.realm)
            .ok_or(ExperimentalLiveAdmissionError::LowerAuthorityUnavailable)?;
        let target = lower_authority
            .admit_target(lower_qualification, target, binding_use_witness)
            .map_err(ExperimentalLiveAdmissionError::LowerAdmission)?;
        Ok(ExperimentalLiveAdmissionWitness {
            authority: preflight.authority,
            realm: preflight.realm,
            factory: preflight.factory,
            gate0_qualification: preflight.gate0_qualification,
            gate0_build_version: preflight.gate0_build_version,
            protocol_digest: preflight.protocol_digest,
            target,
        })
    }

    /// Advertise the capability from side-effect-free build qualification.
    pub fn advertised_feature_capabilities(
        &self,
        qualification: &ExperimentalLiveCapabilityQualification,
    ) -> Result<&'static [&'static str], ExperimentalLiveAdmissionError> {
        self.validate_qualification(qualification)?;
        Ok(&[meerkat_contracts::LIVE_EXECUTION_IDENTITY_V1_CAPABILITY])
    }

    fn validate_qualification(
        &self,
        qualification: &ExperimentalLiveCapabilityQualification,
    ) -> Result<(), ExperimentalLiveAdmissionError> {
        if !Arc::ptr_eq(&self.authority, &qualification.authority) {
            return Err(ExperimentalLiveAdmissionError::StaleCapabilityQualification);
        }
        self.validate_predicates(&qualification.realm)?;
        let operator = self
            .operator
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::OperatorNotConfigured)?;
        let gate0 = self
            .gate0_build
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::Gate0BuildNotQualified)?;
        self.validate_gate0(operator, gate0, &qualification.factory)?;
        if qualification.gate0_qualification != gate0.qualification
            || qualification.gate0_build_version != gate0.build_version
            || qualification.protocol_digest != gate0.protocol_digest
        {
            return Err(ExperimentalLiveAdmissionError::StaleCapabilityQualification);
        }
        Ok(())
    }

    /// Revalidate an admission before a consequential factory use.
    pub fn validate_witness(
        &self,
        witness: &ExperimentalLiveAdmissionWitness,
        realm: &RealmId,
        factory: &ExperimentalLiveFactoryIdentity,
    ) -> Result<(), ExperimentalLiveAdmissionError> {
        if !Arc::ptr_eq(&self.authority, &witness.authority) {
            return Err(ExperimentalLiveAdmissionError::StaleAdmissionWitness);
        }
        self.validate_predicates(realm)?;
        if &witness.realm != realm {
            return Err(ExperimentalLiveAdmissionError::WitnessRealmMismatch);
        }
        if &witness.factory != factory {
            return Err(ExperimentalLiveAdmissionError::WitnessFactoryMismatch);
        }
        let operator = self
            .operator
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::OperatorNotConfigured)?;
        let gate0 = self
            .gate0_build
            .as_ref()
            .ok_or(ExperimentalLiveAdmissionError::Gate0BuildNotQualified)?;
        self.validate_gate0(operator, gate0, factory)?;
        if witness.gate0_qualification != gate0.qualification
            || witness.gate0_build_version != gate0.build_version
            || witness.protocol_digest != gate0.protocol_digest
        {
            return Err(ExperimentalLiveAdmissionError::StaleAdmissionWitness);
        }
        Ok(())
    }

    fn validate_predicates(&self, realm: &RealmId) -> Result<(), ExperimentalLiveAdmissionError> {
        if !self.feature_compiled {
            return Err(ExperimentalLiveAdmissionError::FeatureNotCompiled);
        }
        if self.operator.is_none() {
            return Err(ExperimentalLiveAdmissionError::OperatorNotConfigured);
        }
        if !self.admitted_realms.contains(realm) {
            return Err(ExperimentalLiveAdmissionError::RealmNotAdmitted {
                realm: realm.clone(),
            });
        }
        if self.gate0_build.is_none() {
            return Err(ExperimentalLiveAdmissionError::Gate0BuildNotQualified);
        }
        Ok(())
    }

    fn validate_gate0(
        &self,
        operator: &ExperimentalLiveOperatorConfig,
        gate0: &QualifiedGate0BuildWitness,
        factory: &ExperimentalLiveFactoryIdentity,
    ) -> Result<(), ExperimentalLiveAdmissionError> {
        if gate0.build_version != CURRENT_BUILD_VERSION {
            return Err(ExperimentalLiveAdmissionError::Gate0BuildStale {
                qualified_build: gate0.build_version.clone(),
                current_build: CURRENT_BUILD_VERSION.to_string(),
            });
        }
        if &gate0.factory != factory {
            return Err(ExperimentalLiveAdmissionError::Gate0FactoryMismatch);
        }
        if gate0.qualification != operator.required_gate0 {
            return Err(ExperimentalLiveAdmissionError::Gate0QualificationMismatch);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct ExperimentalLiveAdmissionPreflight {
    authority: Arc<AdmissionAuthority>,
    realm: RealmId,
    factory: ExperimentalLiveFactoryIdentity,
    gate0_qualification: ExperimentalLiveGate0QualificationVersion,
    gate0_build_version: String,
    protocol_digest: String,
    identity: SessionLlmIdentity,
    profile: ModelProfileWitness,
    lower_qualification:
        Option<meerkat_llm_core::provider_runtime::ExperimentalRealtimeQualificationWitness>,
}

/// Opaque, single-target proof of experimental live admission.
///
/// It owns the resolved target so the registry witness, model identity, and
/// credential resolution admitted together cannot be substituted afterward.
pub struct ExperimentalLiveAdmissionWitness {
    authority: Arc<AdmissionAuthority>,
    realm: RealmId,
    factory: ExperimentalLiveFactoryIdentity,
    gate0_qualification: ExperimentalLiveGate0QualificationVersion,
    gate0_build_version: String,
    protocol_digest: String,
    target: meerkat_llm_core::provider_runtime::AdmittedExperimentalRealtimeTarget,
}

impl fmt::Debug for ExperimentalLiveAdmissionWitness {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExperimentalLiveAdmissionWitness")
            .field("realm", &self.realm)
            .field("factory", &self.factory)
            .field("gate0_qualification", &self.gate0_qualification)
            .field("gate0_build_version", &self.gate0_build_version)
            .field("protocol_digest", &self.protocol_digest)
            .field("provider", &self.target.identity().provider)
            .field("model", &"<registry-admitted>")
            .finish_non_exhaustive()
    }
}

impl ExperimentalLiveAdmissionWitness {
    pub fn realm(&self) -> &RealmId {
        &self.realm
    }

    pub fn factory(&self) -> &ExperimentalLiveFactoryIdentity {
        &self.factory
    }

    pub fn identity(&self) -> &SessionLlmIdentity {
        self.target.identity()
    }

    pub fn provider(&self) -> Provider {
        self.target.identity().provider
    }

    /// Consume the proof into the only lower-layer input accepted by the
    /// experimental provider factory.
    #[cfg(feature = "experimental-gpt-live")]
    pub(crate) fn into_provider_target(
        self,
    ) -> meerkat_llm_core::provider_runtime::AdmittedExperimentalRealtimeTarget {
        self.target
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExperimentalLiveAdmissionError {
    #[error("experimental live support was not compiled into this build")]
    FeatureNotCompiled,
    #[error("experimental live support is not configured by the operator")]
    OperatorNotConfigured,
    #[error("experimental live support is not admitted for realm '{realm}'")]
    RealmNotAdmitted { realm: RealmId },
    #[error("this build has no qualified experimental live Gate0 witness")]
    Gate0BuildNotQualified,
    #[error("the Gate0 witness factory does not match the requested factory")]
    Gate0FactoryMismatch,
    #[error("the Gate0 qualification version does not match operator policy")]
    Gate0QualificationMismatch,
    #[error(
        "the Gate0 witness is stale for this build (qualified {qualified_build}, current {current_build})"
    )]
    Gate0BuildStale {
        qualified_build: String,
        current_build: String,
    },
    #[error("the requested factory does not match operator configuration")]
    OperatorFactoryMismatch,
    #[error("the resolved target is not classified experimental by the model registry")]
    TargetNotExperimental,
    #[error("the resolved target is not classified realtime-capable by the model registry")]
    TargetNotRealtime,
    #[error("the registry profile does not match the requested execution identity")]
    TargetProfileMismatch,
    #[error("the resolved connection does not match the admitted provider/model profile")]
    ResolvedConnectionMismatch,
    #[error("the admission witness belongs to an earlier or different admission owner")]
    StaleAdmissionWitness,
    #[error("the admission preflight belongs to an earlier or different admission owner")]
    StaleAdmissionPreflight,
    #[error("the capability qualification belongs to an earlier or different admission owner")]
    StaleCapabilityQualification,
    #[error("the binding-use witness does not match the selected channel auth binding")]
    BindingUseWitnessMismatch,
    #[error("the lower experimental realtime admission authority is unavailable")]
    LowerAuthorityUnavailable,
    #[error(transparent)]
    LowerAdmission(#[from] meerkat_llm_core::provider_runtime::ExperimentalRealtimeAdmissionError),
    #[error("the admission witness is bound to a different realm")]
    WitnessRealmMismatch,
    #[error("the admission witness is bound to a different factory")]
    WitnessFactoryMismatch,
    #[error("invalid experimental live factory kind")]
    InvalidFactoryKind,
    #[error("invalid experimental live factory version")]
    InvalidFactoryVersion,
    #[error("invalid experimental live Gate0 qualification version")]
    InvalidGate0QualificationVersion,
}

/// Typed composition of ordinary factory resolution and admission failure.
#[derive(Debug, Error)]
pub enum ExperimentalLiveFactoryResolutionError {
    #[error("experimental live target resolution failed: {0}")]
    Factory(#[from] FactoryError),
    #[error("experimental live target admission failed: {0}")]
    Admission(#[from] ExperimentalLiveAdmissionError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::{
        ActingOnBehalfOf, AuthBindingRef, AuthGrant, AuthMetadata, BindingId, BindingOrigin,
        Config, GrantAction, GrantScope, ModelRegistry, PrincipalKind, PrincipalRef,
        authorize_explicit_auth_binding_use,
    };
    use meerkat_providers::{NormalizedBackendKind, StaticLease};

    impl ExperimentalLiveAdmissionOwner {
        fn for_test(
            feature_compiled: bool,
            operator: Option<ExperimentalLiveOperatorConfig>,
            admitted_realms: BTreeSet<RealmId>,
            gate0_build: Option<QualifiedGate0BuildWitness>,
        ) -> Self {
            Self {
                authority: Arc::new(AdmissionAuthority),
                feature_compiled,
                operator,
                admitted_realms,
                gate0_build,
                lower_authorities: BTreeMap::new(),
            }
        }
    }

    fn realm(value: &str) -> RealmId {
        RealmId::parse(value).expect("valid test realm")
    }

    fn factory(version: &str) -> ExperimentalLiveFactoryIdentity {
        ExperimentalLiveFactoryIdentity::parse("private-live", version).expect("valid factory")
    }

    fn qualification(version: &str) -> ExperimentalLiveGate0QualificationVersion {
        ExperimentalLiveGate0QualificationVersion::parse(version).expect("valid qualification")
    }

    fn operator(factory: ExperimentalLiveFactoryIdentity) -> ExperimentalLiveOperatorConfig {
        ExperimentalLiveOperatorConfig::new(factory, qualification("gate0-v1"))
    }

    fn gate0(factory: ExperimentalLiveFactoryIdentity) -> QualifiedGate0BuildWitness {
        QualifiedGate0BuildWitness {
            factory,
            qualification: qualification("gate0-v1"),
            build_version: CURRENT_BUILD_VERSION.to_string(),
            protocol_digest: "ab".repeat(32),
        }
    }

    fn binding() -> AuthBindingRef {
        AuthBindingRef {
            realm: realm("voice"),
            binding: BindingId::parse("chatgpt").expect("valid binding"),
            profile: None,
            origin: BindingOrigin::Configured,
        }
    }

    fn target_parts(model: &str) -> (SessionLlmIdentity, ModelProfileWitness, ResolvedConnection) {
        let config = Config::default();
        let registry = ModelRegistry::from_config(&config, meerkat_models::canonical())
            .expect("canonical registry");
        let profile = registry
            .profile_witness_for_provider(Provider::OpenAI, model)
            .expect("test model profile");
        let identity = SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(binding()),
        };
        let connection = ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::ChatGptBackend,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "test".into(),
                provider: Provider::OpenAI,
                backend_kind: "chatgpt_backend".into(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            auth_lease: Arc::new(StaticLease::empty_lease(AuthMetadata::default(), "test")),
        };
        (identity, profile, connection)
    }

    fn binding_use_witness() -> meerkat_core::AuthBindingUseWitness {
        let principal = PrincipalRef::new(PrincipalKind::Human, "alice").expect("principal");
        let target = PrincipalRef::new(PrincipalKind::PersonalAgent, "agent").expect("target");
        let request =
            meerkat_core::AuthBindingUseRequest::new(principal.clone(), target.clone(), binding());
        let grant = AuthGrant {
            principal: principal.clone(),
            scope: GrantScope::AuthBinding {
                realm_id: binding().realm,
                binding_id: binding().binding,
                profile_id: None,
            },
            actions: BTreeSet::from([GrantAction::UseAuthBinding]),
            acting_on_behalf_of: Some(ActingOnBehalfOf::new(principal, target)),
        };
        authorize_explicit_auth_binding_use(&request, &[grant])
            .into_result()
            .expect("exact grant")
    }

    #[test]
    fn all_four_predicates_are_independent_and_required() {
        let admitted_realm = realm("voice");
        let selected_factory = factory("v1");
        for mask in 0_u8..16 {
            let feature_compiled = mask & 0b0001 != 0;
            let operator_configured = mask & 0b0010 != 0;
            let realm_admitted = mask & 0b0100 != 0;
            let gate0_qualified = mask & 0b1000 != 0;
            let owner = ExperimentalLiveAdmissionOwner::for_test(
                feature_compiled,
                operator_configured.then(|| operator(selected_factory.clone())),
                realm_admitted
                    .then(|| BTreeSet::from([admitted_realm.clone()]))
                    .unwrap_or_default(),
                gate0_qualified.then(|| gate0(selected_factory.clone())),
            );
            let result = owner.qualify_capability(&admitted_realm, &selected_factory);
            assert_eq!(
                result.is_ok(),
                mask == 0b1111,
                "only the full conjunction may mint a witness, mask={mask:04b}"
            );
        }
    }

    #[test]
    fn factory_gate0_release_stage_and_staleness_mismatches_fail_closed() {
        let admitted_realm = realm("voice");
        let selected_factory = factory("v1");
        let different_factory = factory("v2");

        let owner = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(operator(selected_factory.clone())),
            BTreeSet::from([admitted_realm.clone()]),
            Some(gate0(selected_factory.clone())),
        );
        assert_eq!(
            owner
                .qualify_capability(&admitted_realm, &different_factory)
                .expect_err("operator factory mismatch"),
            ExperimentalLiveAdmissionError::OperatorFactoryMismatch
        );

        let gate_factory_mismatch = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(operator(selected_factory.clone())),
            BTreeSet::from([admitted_realm.clone()]),
            Some(gate0(different_factory)),
        );
        assert_eq!(
            gate_factory_mismatch
                .qualify_capability(&admitted_realm, &selected_factory)
                .expect_err("Gate0 factory mismatch"),
            ExperimentalLiveAdmissionError::Gate0FactoryMismatch
        );

        let gate_version_mismatch = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(ExperimentalLiveOperatorConfig::new(
                selected_factory.clone(),
                qualification("gate0-v2"),
            )),
            BTreeSet::from([admitted_realm.clone()]),
            Some(gate0(selected_factory.clone())),
        );
        assert_eq!(
            gate_version_mismatch
                .qualify_capability(&admitted_realm, &selected_factory)
                .expect_err("Gate0 version mismatch"),
            ExperimentalLiveAdmissionError::Gate0QualificationMismatch
        );

        let stale_gate0 = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(operator(selected_factory.clone())),
            BTreeSet::from([admitted_realm.clone()]),
            Some(QualifiedGate0BuildWitness {
                factory: selected_factory.clone(),
                qualification: qualification("gate0-v1"),
                build_version: "stale-build".to_string(),
                protocol_digest: "ab".repeat(32),
            }),
        );
        assert!(matches!(
            stale_gate0
                .qualify_capability(&admitted_realm, &selected_factory)
                .expect_err("stale Gate0 build"),
            ExperimentalLiveAdmissionError::Gate0BuildStale { .. }
        ));

        let qualification = owner
            .qualify_capability(&admitted_realm, &selected_factory)
            .expect("qualified");
        let (_, experimental_profile, _) = target_parts("gpt-live-1-codex");
        let (stable_identity, _, _) = target_parts("gpt-realtime-2");
        assert_eq!(
            owner
                .preflight(qualification, stable_identity, experimental_profile)
                .expect_err("profile/identity substitution must fail"),
            ExperimentalLiveAdmissionError::TargetProfileMismatch
        );

        let (identity, profile, _) = target_parts("gpt-live-1-codex");
        let preflight = owner
            .preflight(
                owner
                    .qualify_capability(&admitted_realm, &selected_factory)
                    .expect("qualified"),
                identity,
                profile,
            )
            .expect("valid preflight");
        let wrong_provider_connection = ResolvedConnection {
            provider: Provider::Gemini,
            backend: NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::OpenAiBackendKind::ChatGptBackend,
            ),
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: "wrong-provider".into(),
                provider: Provider::Gemini,
                backend_kind: "wrong_provider".into(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            }),
            auth_lease: Arc::new(StaticLease::empty_lease(AuthMetadata::default(), "test")),
        };
        assert_eq!(
            owner
                .complete(preflight, wrong_provider_connection, binding_use_witness())
                .expect_err("connection/provider substitution must fail"),
            ExperimentalLiveAdmissionError::ResolvedConnectionMismatch
        );
    }

    #[test]
    fn capability_advertisement_revalidates_owner_realm_and_factory() {
        let admitted_realm = realm("voice");
        let selected_factory = factory("v1");
        let owner = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(operator(selected_factory.clone())),
            BTreeSet::from([admitted_realm.clone()]),
            Some(gate0(selected_factory.clone())),
        );
        let qualification = owner
            .qualify_capability(&admitted_realm, &selected_factory)
            .expect("qualified");
        assert_eq!(
            owner
                .advertised_feature_capabilities(&qualification)
                .expect("current witness advertises"),
            &[meerkat_contracts::LIVE_EXECUTION_IDENTITY_V1_CAPABILITY]
        );

        let replacement_owner = ExperimentalLiveAdmissionOwner::for_test(
            true,
            Some(operator(selected_factory.clone())),
            BTreeSet::from([admitted_realm.clone()]),
            Some(gate0(selected_factory.clone())),
        );
        assert_eq!(
            replacement_owner
                .advertised_feature_capabilities(&qualification)
                .expect_err("a reconfigured owner invalidates old qualification"),
            ExperimentalLiveAdmissionError::StaleCapabilityQualification
        );

        let mut stale_protocol = owner
            .qualify_capability(&admitted_realm, &selected_factory)
            .expect("qualified");
        stale_protocol.protocol_digest = "cd".repeat(32);
        assert_eq!(
            owner
                .advertised_feature_capabilities(&stale_protocol)
                .expect_err("same-semver protocol drift invalidates qualification"),
            ExperimentalLiveAdmissionError::StaleCapabilityQualification
        );
    }

    #[test]
    fn default_owner_is_operator_and_realm_closed() {
        let owner = ExperimentalLiveAdmissionOwner::default();
        assert!(owner.operator.is_none());
        assert!(owner.admitted_realms.is_empty());
    }

    #[test]
    fn typed_components_reject_empty_or_unsafe_values() {
        assert_eq!(
            ExperimentalLiveFactoryIdentity::parse("", "v1").expect_err("empty kind"),
            ExperimentalLiveAdmissionError::InvalidFactoryKind
        );
        assert_eq!(
            ExperimentalLiveFactoryIdentity::parse("live", "../v1").expect_err("unsafe version"),
            ExperimentalLiveAdmissionError::InvalidFactoryVersion
        );
        assert_eq!(
            ExperimentalLiveGate0QualificationVersion::parse("not valid")
                .expect_err("unsafe qualification"),
            ExperimentalLiveAdmissionError::InvalidGate0QualificationVersion
        );
    }
}
