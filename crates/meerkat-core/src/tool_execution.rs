//! Internal tool-execution declarations and pre-dispatch resolution.
//!
//! Execution metadata lives beside the internal tool catalog. It is not part
//! of provider-facing [`crate::ToolDef`] serialization.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

/// Execution class declared by a tool identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolExecutionMode {
    Fast,
    Streaming,
    Detached,
}

/// Owner of one deadline that contributes to a resolved tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolDeadlineOwner {
    CoreToolDispatch,
    Dispatcher,
    ToolInternal,
    StreamingAbsolute,
    MetaPublicCallback,
    SdkCallbackCancellation,
    GatewayWire,
    DetachedSubmission,
    DirectCaller,
}

impl ToolDeadlineOwner {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CoreToolDispatch => "core tool dispatch",
            Self::Dispatcher => "dispatcher",
            Self::ToolInternal => "tool internal",
            Self::StreamingAbsolute => "streaming absolute",
            Self::MetaPublicCallback => "meta public callback",
            Self::SdkCallbackCancellation => "sdk callback cancellation",
            Self::GatewayWire => "gateway wire",
            Self::DetachedSubmission => "detached submission",
            Self::DirectCaller => "direct caller",
        }
    }
}

/// How a contributor's owner arrived at its deadline for this call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ToolDeadlineSource {
    /// The owner's default for every tool (for core dispatch,
    /// `tools.default_timeout`).
    #[default]
    OwnerDefault,
    /// An explicit per-tool override the operator configured for this tool
    /// by name (for core dispatch, `tools.tool_timeouts.<tool>`).
    PerToolOverride,
}

/// One finite or unbounded deadline in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolDeadlineContributor {
    owner: ToolDeadlineOwner,
    timeout: Option<Duration>,
    source: ToolDeadlineSource,
}

impl ToolDeadlineContributor {
    pub const fn finite(owner: ToolDeadlineOwner, timeout: Duration) -> Self {
        Self {
            owner,
            timeout: Some(timeout),
            source: ToolDeadlineSource::OwnerDefault,
        }
    }

    pub const fn unbounded(owner: ToolDeadlineOwner) -> Self {
        Self {
            owner,
            timeout: None,
            source: ToolDeadlineSource::OwnerDefault,
        }
    }

    /// A finite deadline the operator configured for this tool by name.
    pub const fn per_tool_override(owner: ToolDeadlineOwner, timeout: Duration) -> Self {
        Self {
            owner,
            timeout: Some(timeout),
            source: ToolDeadlineSource::PerToolOverride,
        }
    }

    pub const fn owner(&self) -> ToolDeadlineOwner {
        self.owner
    }

    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub const fn source(&self) -> ToolDeadlineSource {
        self.source
    }
}

/// Invalid deadline-chain declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineChainError {
    Empty,
    Zero { owner: ToolDeadlineOwner },
}

impl std::fmt::Display for DeadlineChainError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("tool deadline chain must not be empty"),
            Self::Zero { owner } => write!(
                formatter,
                "tool deadline from '{}' must be greater than zero",
                owner.as_str()
            ),
        }
    }
}

impl std::error::Error for DeadlineChainError {}

/// A resolved deadline chain did not retain its upstream chain as an ordered
/// prefix.
///
/// Resolvers may only append contributors. Replacing, removing, or reordering
/// an upstream contributor can silently widen a caller-owned deadline and is
/// therefore rejected before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadlineChainExtensionError {
    ShorterThanUpstream {
        upstream_len: usize,
        resolved_len: usize,
    },
    ContributorMismatch {
        index: usize,
        expected: ToolDeadlineContributor,
        actual: ToolDeadlineContributor,
    },
}

impl std::fmt::Display for DeadlineChainExtensionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShorterThanUpstream {
                upstream_len,
                resolved_len,
            } => write!(
                formatter,
                "resolved tool deadline chain has {resolved_len} contributors but its upstream chain has {upstream_len}"
            ),
            Self::ContributorMismatch {
                index,
                expected,
                actual,
            } => write!(
                formatter,
                "resolved tool deadline contributor {index} replaced upstream owner '{}' ({:?}) with '{}' ({:?})",
                expected.owner().as_str(),
                expected.timeout(),
                actual.owner().as_str(),
                actual.timeout()
            ),
        }
    }
}

impl std::error::Error for DeadlineChainExtensionError {}

/// Ordered deadline chain resolved before dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDeadlineChain {
    contributors: Vec<ToolDeadlineContributor>,
}

impl ToolDeadlineChain {
    pub fn new(contributors: Vec<ToolDeadlineContributor>) -> Result<Self, DeadlineChainError> {
        if contributors.is_empty() {
            return Err(DeadlineChainError::Empty);
        }
        if let Some(contributor) = contributors
            .iter()
            .find(|contributor| contributor.timeout == Some(Duration::ZERO))
        {
            return Err(DeadlineChainError::Zero {
                owner: contributor.owner,
            });
        }
        Ok(Self { contributors })
    }

    pub fn contributors(&self) -> &[ToolDeadlineContributor] {
        &self.contributors
    }

    pub fn winner(&self) -> Option<&ToolDeadlineContributor> {
        self.contributors
            .iter()
            .filter(|contributor| contributor.timeout.is_some())
            .min_by_key(|contributor| contributor.timeout)
    }

    pub fn effective_timeout(&self) -> Option<Duration> {
        self.winner().and_then(ToolDeadlineContributor::timeout)
    }

    pub fn diagnostic(&self) -> String {
        let mut diagnostic = String::new();
        match self.effective_timeout() {
            Some(timeout) => {
                let _ = writeln!(
                    diagnostic,
                    "effective deadline: {}",
                    format_duration(timeout)
                );
            }
            None => diagnostic.push_str("effective deadline: unbounded\n"),
        }
        diagnostic.push_str("contributors:\n");
        for contributor in &self.contributors {
            let limit = contributor
                .timeout
                .map_or_else(|| "unbounded".to_string(), format_duration);
            let source = match contributor.source {
                ToolDeadlineSource::OwnerDefault => "",
                ToolDeadlineSource::PerToolOverride => " (per-tool override)",
            };
            let _ = writeln!(
                diagnostic,
                "  {}: {limit}{source}",
                contributor.owner.as_str()
            );
        }
        diagnostic.push_str("winner: ");
        diagnostic.push_str(
            self.winner()
                .map(|winner| winner.owner.as_str())
                .unwrap_or("unbounded"),
        );
        diagnostic
    }

    pub fn with_contributor(
        &self,
        contributor: ToolDeadlineContributor,
    ) -> Result<Self, DeadlineChainError> {
        let mut contributors = self.contributors.clone();
        contributors.push(contributor);
        Self::new(contributors)
    }

    /// Verify that `upstream` is an exact ordered prefix of this chain.
    pub fn validate_extends(&self, upstream: &Self) -> Result<(), DeadlineChainExtensionError> {
        if self.contributors.len() < upstream.contributors.len() {
            return Err(DeadlineChainExtensionError::ShorterThanUpstream {
                upstream_len: upstream.contributors.len(),
                resolved_len: self.contributors.len(),
            });
        }
        for (index, expected) in upstream.contributors.iter().copied().enumerate() {
            let actual = self.contributors[index];
            if actual != expected {
                return Err(DeadlineChainExtensionError::ContributorMismatch {
                    index,
                    expected,
                    actual,
                });
            }
        }
        Ok(())
    }
}

fn format_duration(duration: Duration) -> String {
    if duration.subsec_nanos() == 0 {
        format!("{}s", duration.as_secs())
    } else if duration.as_millis() > 0 {
        format!("{}ms", duration.as_millis())
    } else if duration.as_micros() > 0 {
        format!("{}us", duration.as_micros())
    } else {
        format!("{}ns", duration.as_nanos())
    }
}

/// Restart behavior declared by a detached runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RestartClass {
    Adoptable,
    CheckpointResumable,
    Replayable,
    NonResumable,
}

/// Stable submission-deduplication scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdempotencyScope {
    ToolCall,
    InteractionAndArguments,
    HostSemanticKey,
}

/// Stable runner identity and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerIdentity {
    name: String,
    version: String,
}

impl RunnerIdentity {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        let name = name.into();
        let version = version.into();
        if name.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyRunnerName);
        }
        if version.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyRunnerVersion);
        }
        Ok(Self { name, version })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Whether one resolved-plan facet applies to the selected execution mode.
///
/// This is intentionally not represented as `Option<T>`: callers must handle
/// the semantic distinction between a facet that does not apply and one whose
/// applicable value is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolExecutionApplicability<T> {
    NotApplicable,
    Applicable(T),
}

/// Non-secret credential context to resolve afresh for an execution attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCredentialContextRef {
    /// Resolve the current owning realm/profile using the required scopes.
    OwningProfile { required_scopes: BTreeSet<String> },
    /// Resolve a specific typed auth binding. The binding is an identity
    /// reference only; it does not contain secret material.
    AuthBinding {
        auth_binding: crate::AuthBindingRef,
        required_scopes: BTreeSet<String>,
    },
}

/// Where the selected execution mode commits its canonical output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputPolicy {
    InlineTerminal,
    StreamingEvents,
    DurableJobResult,
}

/// Where the selected execution mode publishes non-terminal progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProgressPolicy {
    StreamingEvents,
    DurableJobEvents,
}

/// Turn-local identity of one live logical catalog binding.
///
/// Projection allocation identity is deliberately absent: dispatchers may
/// rebuild equivalent `Arc<ToolDef>` values on every catalog read. The
/// semantic declaration is paired with a chain of process-local authority
/// instances and their live epochs. Mutable authorities must advance their
/// epoch when a logical binding is replaced, even by identical metadata.
///
/// This type intentionally has no serialization implementation. It is
/// pre-dispatch TOCTOU fencing only, never durable job, restart, or recovery
/// authority. A reconstructed dispatcher must resolve a fresh plan. A
/// retained consent re-resolves its call that way rather than reusing its
/// plan ([`crate::dispatch_retained_consent_attempt`]).
///
/// ```compile_fail
/// fn requires_durable_serialization<T: serde::Serialize>() {}
/// requires_durable_serialization::<meerkat_core::EphemeralToolBindingFingerprint>();
/// ```
#[derive(Clone)]
pub struct EphemeralToolBindingFingerprint {
    tool_name: crate::ToolName,
    description: String,
    input_schema: serde_json::Value,
    provenance: Option<crate::ToolProvenance>,
    audience: crate::ToolAudience,
    plane: crate::ToolPlaneClass,
    callability: crate::ToolCallability,
    deferred_eligibility: crate::ToolCatalogDeferredEligibility,
    execution: ToolExecutionContract,
    authority_chain: Vec<(usize, u64)>,
}

impl std::fmt::Debug for EphemeralToolBindingFingerprint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EphemeralToolBindingFingerprint")
            .field("tool_name", &self.tool_name)
            .field("authority_depth", &self.authority_chain.len())
            .finish_non_exhaustive()
    }
}

impl PartialEq for EphemeralToolBindingFingerprint {
    fn eq(&self, other: &Self) -> bool {
        self.tool_name == other.tool_name
            && self.description == other.description
            && self.input_schema == other.input_schema
            && self.provenance == other.provenance
            && self.audience == other.audience
            && self.plane == other.plane
            && self.callability == other.callability
            && self.deferred_eligibility == other.deferred_eligibility
            && self.execution == other.execution
            && self.authority_chain == other.authority_chain
    }
}

impl Eq for EphemeralToolBindingFingerprint {}

pub fn ephemeral_tool_catalog_binding_fingerprint(
    entry: &crate::ToolCatalogEntry,
) -> EphemeralToolBindingFingerprint {
    EphemeralToolBindingFingerprint {
        tool_name: entry.tool.name.clone(),
        description: entry.tool.description.clone(),
        input_schema: entry.tool.input_schema.clone(),
        provenance: entry.tool.provenance.clone(),
        audience: entry.tool.audience,
        plane: entry.plane,
        callability: entry.callability,
        deferred_eligibility: entry.deferred_eligibility.clone(),
        execution: entry.execution.clone(),
        authority_chain: Vec::new(),
    }
}

impl EphemeralToolBindingFingerprint {
    /// The durable digest of the declaration this binding advertised
    /// ([`ToolBindingDigest`]): its live authorities and its current
    /// callability play no part.
    pub(crate) fn declaration_digest(&self) -> ToolBindingDigest {
        ToolBindingDigest::of_declaration(&ToolDeclaration {
            name: self.tool_name.as_str(),
            description: &self.description,
            input_schema: &self.input_schema,
            provenance: self.provenance.as_ref(),
            audience: self.audience,
            plane: self.plane,
            deferred_eligibility: &self.deferred_eligibility,
            execution: &self.execution,
        })
    }

    #[must_use]
    pub fn with_live_authority(mut self, authority_instance: usize, epoch: u64) -> Self {
        self.authority_chain.push((authority_instance, epoch));
        self
    }

    #[must_use]
    pub fn with_dependency(mut self, dependency: &Self) -> Self {
        self.authority_chain
            .extend(dependency.authority_chain.iter().copied());
        self
    }
}

/// Durable identity of what one tool catalog entry declares: a SHA-256 digest
/// over a canonical encoding of its tool's name, description, input schema,
/// provenance and audience, its plane, its deferred eligibility and its
/// execution contract (supported and default modes, the streaming policy, the
/// detached policy with its runner, restart class, idempotency scope,
/// submission deadline and credential scopes, and the core deadline).
/// Nothing of the live authorities serving the entry is in it, and neither is
/// its current callability, which is a moment's availability rather than
/// part of the declaration.
///
/// Unlike [`EphemeralToolBindingFingerprint`], it is meant to be kept: a
/// durable record (such as a detached job's runner specification) keeps it to
/// tell, after a restart or once another authority serves the tool, whether
/// the tool it would now reach declares what it was recorded against. Equal
/// digests mean equal declarations, never the same live authority. Its text
/// form is `sha256:` and 64 lowercase hex digits.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToolBindingDigest([u8; 32]);

/// Domain separation for [`ToolBindingDigest`]: the encoding's name and
/// version, hashed before any field.
const TOOL_BINDING_DIGEST_DOMAIN: &[u8] = b"meerkat.tool-binding-digest.v1\0";

/// A text that is not a [`ToolBindingDigest`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a tool binding digest: expected `sha256:` and 64 lowercase hex digits")]
pub struct ToolBindingDigestParseError;

/// The declared facets of one tool binding that [`ToolBindingDigest`]
/// hashes, wherever they are read from.
struct ToolDeclaration<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a serde_json::Value,
    provenance: Option<&'a crate::ToolProvenance>,
    audience: crate::ToolAudience,
    plane: crate::ToolPlaneClass,
    deferred_eligibility: &'a crate::ToolCatalogDeferredEligibility,
    execution: &'a ToolExecutionContract,
}

impl ToolBindingDigest {
    /// The digest of what `entry` declares.
    #[must_use]
    pub fn of_catalog_entry(entry: &crate::ToolCatalogEntry) -> Self {
        Self::of_declaration(&ToolDeclaration {
            name: entry.tool.name.as_str(),
            description: &entry.tool.description,
            input_schema: &entry.tool.input_schema,
            provenance: entry.tool.provenance.as_ref(),
            audience: entry.tool.audience,
            plane: entry.plane,
            deferred_eligibility: &entry.deferred_eligibility,
            execution: &entry.execution,
        })
    }

    fn of_declaration(declaration: &ToolDeclaration<'_>) -> Self {
        let mut encoder = BindingDigestEncoder::new();
        encoder.text("tool.name", declaration.name);
        encoder.text("tool.description", declaration.description);
        encoder.text(
            "tool.input_schema",
            &canonical_json_text(declaration.input_schema),
        );
        encoder.provenance("tool.provenance", declaration.provenance);
        encoder.text(
            "tool.audience",
            match declaration.audience {
                crate::ToolAudience::Model => "model",
                crate::ToolAudience::App => "app",
                crate::ToolAudience::ModelAndApp => "model_and_app",
                crate::ToolAudience::Hidden => "hidden",
            },
        );
        encoder.text(
            "plane",
            match declaration.plane {
                crate::ToolPlaneClass::Session => "session",
                crate::ToolPlaneClass::Control => "control",
            },
        );
        match declaration.deferred_eligibility {
            crate::ToolCatalogDeferredEligibility::InlineOnly => {
                encoder.text("deferred_eligibility", "inline_only");
            }
            crate::ToolCatalogDeferredEligibility::DeferredEligible { provenance } => {
                encoder.text("deferred_eligibility", "deferred_eligible");
                encoder.provenance("deferred_eligibility.provenance", Some(provenance));
            }
        }
        encoder.execution(declaration.execution);
        Self(encoder.finish())
    }

    /// The digest's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Display for ToolBindingDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for ToolBindingDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ToolBindingDigest({self})")
    }
}

impl std::str::FromStr for ToolBindingDigest {
    type Err = ToolBindingDigestParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let hex = text
            .strip_prefix("sha256:")
            .filter(|hex| hex.len() == 64)
            .ok_or(ToolBindingDigestParseError)?;
        let mut bytes = [0_u8; 32];
        for (byte, pair) in bytes.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            let digit = |value: u8| match value {
                b'0'..=b'9' => Ok(value - b'0'),
                b'a'..=b'f' => Ok(value - b'a' + 10),
                _ => Err(ToolBindingDigestParseError),
            };
            *byte = (digit(pair[0])? << 4) | digit(pair[1])?;
        }
        Ok(Self(bytes))
    }
}

impl serde::Serialize for ToolBindingDigest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ToolBindingDigest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The canonical encoding [`ToolBindingDigest`] hashes: the domain, then each
/// field as its label and its value, both length-prefixed (big-endian
/// `u64`), in a fixed order. A list is its length (as a field) and then its
/// items; an absent optional value is the field's own `none` marker.
struct BindingDigestEncoder(Sha256);

impl BindingDigestEncoder {
    fn new() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(TOOL_BINDING_DIGEST_DOMAIN);
        Self(hasher)
    }

    fn bytes(&mut self, label: &str, value: &[u8]) {
        for part in [label.as_bytes(), value] {
            self.0
                .update(u64::try_from(part.len()).unwrap_or(u64::MAX).to_be_bytes());
            self.0.update(part);
        }
    }

    fn text(&mut self, label: &str, value: &str) {
        self.bytes(label, value.as_bytes());
    }

    fn count(&mut self, label: &str, count: usize) {
        self.bytes(
            label,
            &u64::try_from(count).unwrap_or(u64::MAX).to_be_bytes(),
        );
    }

    fn duration(&mut self, label: &str, duration: Duration) {
        self.bytes(label, &duration.as_nanos().to_be_bytes());
    }

    fn provenance(&mut self, label: &str, provenance: Option<&crate::ToolProvenance>) {
        let Some(provenance) = provenance else {
            self.text(label, "none");
            return;
        };
        self.text(
            label,
            match provenance.kind {
                crate::ToolSourceKind::Builtin => "builtin",
                crate::ToolSourceKind::Shell => "shell",
                crate::ToolSourceKind::Comms => "comms",
                crate::ToolSourceKind::Memory => "memory",
                crate::ToolSourceKind::Schedule => "schedule",
                crate::ToolSourceKind::WorkGraph => "work_graph",
                crate::ToolSourceKind::Mob => "mob",
                crate::ToolSourceKind::Callback => "callback",
                crate::ToolSourceKind::Mcp => "mcp",
                crate::ToolSourceKind::RustBundle => "rust_bundle",
            },
        );
        self.text("source_id", provenance.source_id.as_str());
        match &provenance.artifact {
            None => self.text("artifact", "none"),
            Some(artifact) => {
                self.text("artifact", "some");
                self.text("artifact.id", artifact.artifact_id());
                self.text("artifact.digest", artifact.digest());
            }
        }
    }

    fn mode(&mut self, label: &str, mode: ToolExecutionMode) {
        self.text(
            label,
            match mode {
                ToolExecutionMode::Fast => "fast",
                ToolExecutionMode::Streaming => "streaming",
                ToolExecutionMode::Detached => "detached",
            },
        );
    }

    fn restart_class(&mut self, label: &str, restart_class: RestartClass) {
        self.text(
            label,
            match restart_class {
                RestartClass::Adoptable => "adoptable",
                RestartClass::CheckpointResumable => "checkpoint_resumable",
                RestartClass::Replayable => "replayable",
                RestartClass::NonResumable => "non_resumable",
            },
        );
    }

    fn execution(&mut self, execution: &ToolExecutionContract) {
        self.count("execution.supported_modes", execution.supported_modes.len());
        for mode in &execution.supported_modes {
            self.mode("execution.supported_mode", *mode);
        }
        self.mode("execution.default_mode", execution.default_mode);
        match &execution.streaming_policy {
            None => self.text("execution.streaming_policy", "none"),
            Some(policy) => {
                self.text("execution.streaming_policy", "some");
                self.duration("streaming.inactivity_timeout", policy.inactivity_timeout);
                self.duration("streaming.absolute_timeout", policy.absolute_timeout);
            }
        }
        match &execution.detached_policy {
            None => self.text("execution.detached_policy", "none"),
            Some(policy) => {
                self.text("execution.detached_policy", "some");
                self.text("detached.runner.name", policy.runner.name());
                self.text("detached.runner.version", policy.runner.version());
                self.restart_class("detached.restart_class", policy.restart_class);
                self.text(
                    "detached.idempotency_scope",
                    match policy.idempotency_scope {
                        IdempotencyScope::ToolCall => "tool_call",
                        IdempotencyScope::InteractionAndArguments => "interaction_and_arguments",
                        IdempotencyScope::HostSemanticKey => "host_semantic_key",
                    },
                );
                self.duration("detached.submission_timeout", policy.submission_timeout);
                self.count("detached.credential_scopes", policy.credential_scopes.len());
                for scope in &policy.credential_scopes {
                    self.text("detached.credential_scope", scope);
                }
            }
        }
        self.count(
            "execution.detached_restart_classes",
            execution.detached_restart_classes.len(),
        );
        for restart_class in &execution.detached_restart_classes {
            self.restart_class("execution.detached_restart_class", *restart_class);
        }
        self.text(
            "execution.core_deadline",
            match execution.core_deadline {
                CoreDispatchDeadline::Applies => "applies",
                CoreDispatchDeadline::ToolOwned => "tool_owned",
            },
        );
    }

    fn finish(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

/// `value` as JSON text with every object's keys in sorted order, so the
/// same value always has the same text however its objects were built.
fn canonical_json_text(value: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
                serde_json::Value::Object(
                    entries
                        .into_iter()
                        .map(|(key, value)| (key.clone(), sorted(value)))
                        .collect(),
                )
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            scalar => scalar.clone(),
        }
    }
    sorted(value).to_string()
}

/// Opaque, non-provider-facing witness for the dispatcher owner selected
/// during plan resolution.
///
/// A composite dispatcher can attach this witness and require the same live
/// owner/binding object at dispatch. The witness is valid only for the current
/// pre-dispatch resolution flow. It must be discarded on restart, recovery,
/// dispatcher reconstruction, or turn replay; those paths must resolve again.
/// It is not a job fence, attempt token, or durable authority. Revalidation
/// rejects stale, wrong-owner and tampered plans within trusted Rust
/// composition; the constructors and accessors are public, so it is not
/// cryptographic resistance to hostile in-process Rust code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionOwnerWitness {
    authority_key: String,
    owner_key: String,
    binding_fingerprint: EphemeralToolBindingFingerprint,
    operation_target: Option<ToolOperationTarget>,
}

/// Typed, nonsecret identity of the exact operation an execution owner
/// resolved a call to, produced by that owner with its witness and
/// revalidated by the same live owner at dispatch. It is a fact for the
/// operation policy owner, never a permission, and like its witness it is
/// discarded on restart, recovery, reconstruction or replay.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolOperationTarget {
    /// One registered MCP server's raw operation, on the server's current
    /// physical connection generation. Two servers exposing the same raw
    /// operation remain distinct by registration.
    McpServerOperation {
        server: String,
        connection_generation: u64,
        raw_operation: String,
        /// The connector approval of the descriptor that connection was
        /// opened from, for the operation owner to compare with the grant's
        /// approved binding.
        connector: crate::mcp_config::McpConnectorApproval,
        /// The account that connection's operations act with, as its owners
        /// state it for this exact connection: the connector binding's
        /// account, the OAuth credential owner's selected account, the
        /// server configuration's declaration that it acts with none, or
        /// unavailable. Fixed for the connection's lifetime, so a changed
        /// binding is a new connection generation.
        account: std::sync::Arc<crate::OperationAccount>,
    },
    /// The session's own tool catalog control plane, produced only by the
    /// catalog control dispatcher for the operation it owns. A tool that
    /// merely carries a catalog tool name has no such target.
    CatalogControl { operation: CatalogOperation },
    /// One operation of a Rust tool bundle the host registered under
    /// `bundle`, produced only by that registration's
    /// [`crate::RegisteredToolBundleDispatcher`] and revalidated by it at
    /// dispatch. `operation` is the bundle's own tool, distinct by
    /// registration like an MCP server's raw operation: a tool that is merely
    /// named like it, from any other dispatcher, has no such target.
    /// `account` is the one the registration declared for the bundle's
    /// operations ([`crate::RegisteredToolBundleDispatcher::with_account`]),
    /// unavailable when it declared none.
    RegisteredToolBundleOperation {
        bundle: String,
        operation: String,
        account: std::sync::Arc<crate::OperationAccount>,
    },
    /// One registered MCP server's raw operation reached through the checked
    /// route a trusted host registered under the stable name `route`, on
    /// that server's current physical connection generation. Produced only
    /// by that route and revalidated by it at dispatch; the other facts are
    /// the ones [`Self::McpServerOperation`] states for the same connection.
    /// `route` makes it a target of its own: the server's ordinary grant
    /// never covers it, and its grant never covers the ordinary operation.
    CheckedMcpServerOperation {
        route: String,
        server: String,
        connection_generation: u64,
        raw_operation: String,
        connector: crate::mcp_config::McpConnectorApproval,
        account: std::sync::Arc<crate::OperationAccount>,
    },
    /// One comms delivery to exactly one peer, by its canonical
    /// [`crate::comms::PeerId`] (the routing and trust key, never a display
    /// name). Produced by the comms tools' owner for the peer the call
    /// actually routes to: the explicit `peer_id` of a send, or the sender
    /// of the turn's triggering message a reply selects. Revalidated by that
    /// owner at dispatch, so a plan resolved for one peer never delivers to
    /// another. Only the comms tools produce it (with
    /// [`Self::PeerDirectoryRead`] for `peers`): other tools that reach
    /// agents, such as `delegate`, `council`, `mob_spawn_member` and a mob
    /// member's calls up to its mob's operator, carry no peer target. `kind`
    /// is the delivery the tool makes; a response goes only to the requester
    /// of the request it answers (see [`PeerSendKind::Response`]).
    PeerSend {
        peer: crate::comms::PeerId,
        kind: PeerSendKind,
    },
    /// A read of the session's own trusted-peer directory (the comms
    /// `peers` roster): peer identities, addresses and metadata. It
    /// delivers nothing and stands for no later delivery.
    PeerDirectoryRead,
    /// One operation of a memory owner on exactly one memory scope, as that
    /// owner names it. Produced by the memory tool's owner for the scope the
    /// call actually reaches, and revalidated by it at dispatch.
    MemoryOperation {
        scope: crate::memory::MemoryScopeRef,
        operation: MemoryOperation,
    },
    /// One operation of a Meerkat builtin tool: the operation the builtin
    /// implementation itself declares, stated by the registration owner of
    /// that builtin (the composite dispatcher that registered it) and
    /// revalidated by that same registration at dispatch. A tool merely
    /// named like the builtin, from any other owner (a host dispatcher, a
    /// registered bundle, an undeclared builtin), has no such target unless
    /// that owner states it itself, which an in-process dispatcher can: like
    /// every target, it is the dispatching owner's fact, never a permission.
    BuiltinOperation { operation: BuiltinToolOperation },
}

/// A builtin tool operation that its registration owner states as a typed
/// operation target ([`ToolOperationTarget::BuiltinOperation`]). The builtin
/// implementation declares it; it is never derived from a tool name. Later
/// builtin families are additive.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BuiltinToolOperation {
    /// The Meerkat-owned `web_search` builtin: one public web search through
    /// its own hosted-search helper, a model request the helper's own owners
    /// decide as the call's typed delegate.
    WebSearch,
}

impl ToolOperationTarget {
    /// Whether `other` is this same operation as it stays across a
    /// reconnection: equal in everything but an MCP server's live connection
    /// generation. The registration (server or route), the raw operation,
    /// the connector approval and the account still have to be equal, and
    /// every other target is compared whole. A registration is its
    /// configured name: the endpoint it points to is not compared, so a name
    /// re-pointed to another endpoint compares equal unless the account
    /// names it. An account the owner cannot state
    /// ([`crate::OperationAccount::Unavailable`]) compares equal to another
    /// such account.
    pub(crate) fn same_durable_target(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::McpServerOperation {
                    server,
                    connection_generation: _,
                    raw_operation,
                    connector,
                    account,
                },
                Self::McpServerOperation {
                    server: other_server,
                    connection_generation: _,
                    raw_operation: other_raw_operation,
                    connector: other_connector,
                    account: other_account,
                },
            ) => {
                server == other_server
                    && raw_operation == other_raw_operation
                    && connector == other_connector
                    && account == other_account
            }
            (
                Self::CheckedMcpServerOperation {
                    route,
                    server,
                    connection_generation: _,
                    raw_operation,
                    connector,
                    account,
                },
                Self::CheckedMcpServerOperation {
                    route: other_route,
                    server: other_server,
                    connection_generation: _,
                    raw_operation: other_raw_operation,
                    connector: other_connector,
                    account: other_account,
                },
            ) => {
                route == other_route
                    && server == other_server
                    && raw_operation == other_raw_operation
                    && connector == other_connector
                    && account == other_account
            }
            (left, right) => left == right,
        }
    }

    /// The account the owner states this exact operation acts with: `None`
    /// for the session's own catalog control, comms deliveries, peer
    /// directory and memory operations, which act with the session's own
    /// identity and no account of their own, and for a builtin operation,
    /// which acts with no account of its own.
    #[must_use]
    pub fn account(&self) -> Option<&crate::OperationAccount> {
        match self {
            Self::McpServerOperation { account, .. }
            | Self::RegisteredToolBundleOperation { account, .. }
            | Self::CheckedMcpServerOperation { account, .. } => Some(account),
            Self::CatalogControl { .. }
            | Self::PeerSend { .. }
            | Self::PeerDirectoryRead
            | Self::MemoryOperation { .. }
            | Self::BuiltinOperation { .. } => None,
        }
    }
}

/// The catalog control plane's operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CatalogOperation {
    Search,
    Load,
}

/// The kind of delivery a comms operation makes to its one peer. It is
/// descriptive, not an authority boundary: grant lineage selection ignores
/// it, so a host maps `Response` to the same action as `Message`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PeerSendKind {
    /// A message that expects no correlated response (`send_message`, and
    /// `reply_to_peer` answering the turn's peer message).
    Message,
    /// A typed request that expects a correlated response (`send_request`).
    Request,
    /// A typed response to a request (`send_response`). Its addressee must
    /// be the request's requester: the runtime records the sender ingress
    /// admitted for each request this session receives and refuses, before
    /// any transport, a response (progress or terminal) addressed to any
    /// other peer, as it refuses one to a request it cannot answer. The
    /// delivery reaches only the addressed peer.
    Response,
}

/// The kind of operation a memory owner performs on one memory scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MemoryOperation {
    /// Reads, searches or lists the scope's records.
    Read,
    /// Creates or replaces a record in the scope.
    Write,
    /// Removes or tombstones a record in the scope.
    Delete,
    /// Submits a record for the scope's reviewing owner to commit; nothing
    /// is written to the scope by the operation itself.
    Propose,
}

impl ToolExecutionOwnerWitness {
    pub fn new(
        authority_key: impl Into<String>,
        owner_key: impl Into<String>,
        binding_fingerprint: EphemeralToolBindingFingerprint,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        let authority_key = authority_key.into();
        let owner_key = owner_key.into();
        if authority_key.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyOwnerWitnessAuthorityKey);
        }
        if owner_key.trim().is_empty() {
            return Err(ToolExecutionDeclarationError::EmptyOwnerWitnessKey);
        }
        Ok(Self {
            authority_key,
            owner_key,
            binding_fingerprint,
            operation_target: None,
        })
    }

    /// This witness's owner resolved the call to `target`.
    #[must_use]
    pub fn with_operation_target(mut self, target: ToolOperationTarget) -> Self {
        self.operation_target = Some(target);
        self
    }

    /// The exact operation this witness's owner resolved the call to, when
    /// the owner produces one.
    pub fn operation_target(&self) -> Option<&ToolOperationTarget> {
        self.operation_target.as_ref()
    }

    pub fn authority_key(&self) -> &str {
        &self.authority_key
    }

    pub fn owner_key(&self) -> &str {
        &self.owner_key
    }

    pub const fn binding_fingerprint(&self) -> &EphemeralToolBindingFingerprint {
        &self.binding_fingerprint
    }
}

/// Liveness policy for a streaming tool implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamingToolExecutionPolicy {
    inactivity_timeout: Duration,
    absolute_timeout: Duration,
}

impl StreamingToolExecutionPolicy {
    pub fn new(
        inactivity_timeout: Duration,
        absolute_timeout: Duration,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        if inactivity_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroStreamingInactivity);
        }
        if absolute_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroStreamingAbsolute);
        }
        if inactivity_timeout > absolute_timeout {
            return Err(ToolExecutionDeclarationError::StreamingInactivityExceedsAbsolute);
        }
        Ok(Self {
            inactivity_timeout,
            absolute_timeout,
        })
    }

    pub const fn inactivity_timeout(&self) -> Duration {
        self.inactivity_timeout
    }

    pub const fn absolute_timeout(&self) -> Duration {
        self.absolute_timeout
    }
}

/// Submission and restart declaration for a detached tool implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachedToolExecutionPolicy {
    runner: RunnerIdentity,
    restart_class: RestartClass,
    idempotency_scope: IdempotencyScope,
    submission_timeout: Duration,
    credential_scopes: BTreeSet<String>,
}

impl DetachedToolExecutionPolicy {
    pub fn new(
        runner: RunnerIdentity,
        restart_class: RestartClass,
        idempotency_scope: IdempotencyScope,
        submission_timeout: Duration,
    ) -> Result<Self, ToolExecutionDeclarationError> {
        if submission_timeout.is_zero() {
            return Err(ToolExecutionDeclarationError::ZeroDetachedSubmissionDeadline);
        }
        Ok(Self {
            runner,
            restart_class,
            idempotency_scope,
            submission_timeout,
            credential_scopes: BTreeSet::new(),
        })
    }

    #[must_use]
    pub fn with_credential_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.credential_scopes.extend(
            scopes
                .into_iter()
                .map(Into::into)
                .filter(|scope: &String| !scope.trim().is_empty()),
        );
        self
    }

    pub fn runner(&self) -> &RunnerIdentity {
        &self.runner
    }

    pub const fn restart_class(&self) -> RestartClass {
        self.restart_class
    }

    pub const fn idempotency_scope(&self) -> IdempotencyScope {
        self.idempotency_scope
    }

    pub const fn submission_timeout(&self) -> Duration {
        self.submission_timeout
    }

    pub fn credential_scopes(&self) -> &BTreeSet<String> {
        &self.credential_scopes
    }
}

/// Invalid streaming or detached policy declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionDeclarationError {
    EmptyRunnerName,
    EmptyRunnerVersion,
    EmptyOwnerWitnessAuthorityKey,
    EmptyOwnerWitnessKey,
    ZeroStreamingInactivity,
    ZeroStreamingAbsolute,
    StreamingInactivityExceedsAbsolute,
    ZeroDetachedSubmissionDeadline,
}

impl std::fmt::Display for ToolExecutionDeclarationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyRunnerName => formatter.write_str("detached runner name must not be empty"),
            Self::EmptyRunnerVersion => {
                formatter.write_str("detached runner version must not be empty")
            }
            Self::EmptyOwnerWitnessAuthorityKey => {
                formatter.write_str("tool execution owner witness authority key must not be empty")
            }
            Self::EmptyOwnerWitnessKey => {
                formatter.write_str("tool execution owner witness key must not be empty")
            }
            Self::ZeroStreamingInactivity => {
                formatter.write_str("streaming inactivity timeout must be greater than zero")
            }
            Self::ZeroStreamingAbsolute => {
                formatter.write_str("streaming absolute timeout must be greater than zero")
            }
            Self::StreamingInactivityExceedsAbsolute => formatter
                .write_str("streaming inactivity timeout must not exceed the absolute timeout"),
            Self::ZeroDetachedSubmissionDeadline => {
                formatter.write_str("detached submission timeout must be greater than zero")
            }
        }
    }
}

impl std::error::Error for ToolExecutionDeclarationError {}

/// Invalid combination of supported modes and mode-specific policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionContractError {
    NoSupportedModes,
    DefaultModeUnsupported {
        default_mode: ToolExecutionMode,
    },
    MissingStreamingPolicy,
    UnexpectedStreamingPolicy,
    MissingDetachedPolicy,
    UnexpectedDetachedPolicy,
    RequestedModeUnsupported {
        requested_mode: ToolExecutionMode,
    },
    RequestedRestartClassUnsupported {
        restart_class: RestartClass,
    },
    ResolvedPlanFacetMismatch {
        mode: ToolExecutionMode,
        facet: &'static str,
    },
}

impl std::fmt::Display for ToolExecutionContractError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSupportedModes => {
                formatter.write_str("tool execution contract must support at least one mode")
            }
            Self::DefaultModeUnsupported { default_mode } => write!(
                formatter,
                "default tool execution mode {default_mode:?} is not supported"
            ),
            Self::MissingStreamingPolicy => {
                formatter.write_str("streaming mode requires a streaming policy")
            }
            Self::UnexpectedStreamingPolicy => {
                formatter.write_str("streaming policy requires streaming mode support")
            }
            Self::MissingDetachedPolicy => {
                formatter.write_str("detached mode requires a detached policy")
            }
            Self::UnexpectedDetachedPolicy => {
                formatter.write_str("detached policy requires detached mode support")
            }
            Self::RequestedModeUnsupported { requested_mode } => write!(
                formatter,
                "requested tool execution mode {requested_mode:?} is not supported"
            ),
            Self::RequestedRestartClassUnsupported { restart_class } => write!(
                formatter,
                "requested detached restart class {restart_class:?} is not supported"
            ),
            Self::ResolvedPlanFacetMismatch { mode, facet } => write!(
                formatter,
                "resolved {mode:?} tool execution plan does not match advertised facet '{facet}'"
            ),
        }
    }
}

impl std::error::Error for ToolExecutionContractError {}

/// Typed, caller-owned facts used during pre-dispatch plan resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionResolutionContext {
    deadlines: ToolDeadlineChain,
}

impl ToolExecutionResolutionContext {
    pub const fn new(deadlines: ToolDeadlineChain) -> Self {
        Self { deadlines }
    }

    pub fn deadlines(&self) -> &ToolDeadlineChain {
        &self.deadlines
    }

    pub fn with_deadline(
        &self,
        contributor: ToolDeadlineContributor,
    ) -> Result<Self, DeadlineChainError> {
        self.deadlines.with_contributor(contributor).map(Self::new)
    }

    /// Verify that a resolver only appended deadlines to this caller-owned
    /// chain.
    pub fn validate_resolved_plan(
        &self,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        plan.deadlines
            .validate_extends(&self.deadlines)
            .map_err(ToolExecutionResolutionError::DeadlineExtension)
    }
}

/// Failure to resolve an exact pre-dispatch tool execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionResolutionError {
    NotFound {
        tool_name: String,
    },
    Unavailable {
        tool_name: String,
        reason: crate::tool_catalog::ToolUnavailableReason,
    },
    AccessDenied {
        tool_name: String,
    },
    InvalidArguments {
        tool_name: String,
        reason: String,
    },
    Deadline(DeadlineChainError),
    DeadlineExtension(DeadlineChainExtensionError),
    OwnerWitnessAlreadyAssigned {
        existing: Box<ToolExecutionOwnerWitness>,
        attempted: Box<ToolExecutionOwnerWitness>,
    },
    ResolvedCallMismatch {
        tool_name: String,
    },
    RootDispatcherChanged {
        tool_name: String,
    },
    Declaration(ToolExecutionDeclarationError),
    Contract(ToolExecutionContractError),
}

impl std::fmt::Display for ToolExecutionResolutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { tool_name } => {
                write!(
                    formatter,
                    "tool '{tool_name}' is not present in the catalog"
                )
            }
            Self::Unavailable { tool_name, reason } => {
                write!(formatter, "tool '{tool_name}' is unavailable: {reason}")
            }
            Self::AccessDenied { tool_name } => {
                write!(
                    formatter,
                    "tool '{tool_name}' is denied by execution policy"
                )
            }
            Self::InvalidArguments { tool_name, reason } => {
                write!(
                    formatter,
                    "tool '{tool_name}' arguments cannot resolve an execution plan: {reason}"
                )
            }
            Self::Deadline(error) => std::fmt::Display::fmt(error, formatter),
            Self::DeadlineExtension(error) => {
                write!(
                    formatter,
                    "tool deadline resolution replaced an upstream deadline: {error}"
                )
            }
            Self::OwnerWitnessAlreadyAssigned {
                existing,
                attempted,
            } => write!(
                formatter,
                "tool execution authority '{}' already selected owner '{}'; refusing replacement by '{}'",
                existing.authority_key(),
                existing.owner_key(),
                attempted.owner_key(),
            ),
            Self::ResolvedCallMismatch { tool_name } => write!(
                formatter,
                "resolved execution plan for tool '{tool_name}' was dispatched with a different call identity"
            ),
            Self::RootDispatcherChanged { tool_name } => write!(
                formatter,
                "resolved execution plan for tool '{tool_name}' was dispatched through a different dispatcher instance"
            ),
            Self::Declaration(error) => std::fmt::Display::fmt(error, formatter),
            Self::Contract(error) => std::fmt::Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for ToolExecutionResolutionError {}

impl From<DeadlineChainError> for ToolExecutionResolutionError {
    fn from(error: DeadlineChainError) -> Self {
        Self::Deadline(error)
    }
}

impl From<ToolExecutionContractError> for ToolExecutionResolutionError {
    fn from(error: ToolExecutionContractError) -> Self {
        Self::Contract(error)
    }
}

impl From<ToolExecutionDeclarationError> for ToolExecutionResolutionError {
    fn from(error: ToolExecutionDeclarationError) -> Self {
        Self::Declaration(error)
    }
}

impl From<ToolExecutionResolutionError> for crate::error::ToolError {
    fn from(error: ToolExecutionResolutionError) -> Self {
        match error {
            ToolExecutionResolutionError::NotFound { tool_name } => Self::not_found(tool_name),
            ToolExecutionResolutionError::Unavailable { tool_name, reason } => {
                Self::unavailable(tool_name, reason)
            }
            ToolExecutionResolutionError::AccessDenied { tool_name } => {
                Self::access_denied(tool_name)
            }
            ToolExecutionResolutionError::InvalidArguments { tool_name, reason } => {
                Self::invalid_arguments(tool_name, reason)
            }
            ToolExecutionResolutionError::Deadline(error) => {
                Self::execution_failed(format!("tool deadline resolution failed: {error}"))
            }
            ToolExecutionResolutionError::DeadlineExtension(error) => Self::execution_failed(
                format!("tool deadline resolution replaced an upstream deadline: {error}"),
            ),
            ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing,
                attempted,
            } => Self::execution_failed(format!(
                "tool execution authority '{}' already selected owner '{}'; refusing replacement by '{}'",
                existing.authority_key(),
                existing.owner_key(),
                attempted.owner_key(),
            )),
            ToolExecutionResolutionError::ResolvedCallMismatch { tool_name }
            | ToolExecutionResolutionError::RootDispatcherChanged { tool_name } => {
                Self::unavailable(
                    tool_name,
                    crate::ToolUnavailableReason::ExecutionOwnerChanged,
                )
            }
            ToolExecutionResolutionError::Declaration(error) => {
                Self::execution_failed(format!("tool execution declaration failed: {error}"))
            }
            ToolExecutionResolutionError::Contract(error) => Self::execution_failed(format!(
                "tool execution contract resolution failed: {error}"
            )),
        }
    }
}

/// Internal execution declaration attached to a catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolExecutionContract {
    supported_modes: BTreeSet<ToolExecutionMode>,
    default_mode: ToolExecutionMode,
    streaming_policy: Option<StreamingToolExecutionPolicy>,
    detached_policy: Option<DetachedToolExecutionPolicy>,
    detached_restart_classes: BTreeSet<RestartClass>,
    core_deadline: CoreDispatchDeadline,
}

/// Whether the agent loop's default tool deadline bounds a tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum CoreDispatchDeadline {
    /// The configured core dispatch timeout applies (the default).
    #[default]
    Applies,
    /// The tool owns its own lifetime bound (an explicit, caller-chosen
    /// limit such as fork_off's `max_run_secs` or a council deadline), so the
    /// core default must not cut it. An operator's explicit per-tool
    /// override for the tool, and every other contributor, still apply.
    ToolOwned,
}

impl Default for ToolExecutionContract {
    fn default() -> Self {
        Self {
            supported_modes: BTreeSet::from([ToolExecutionMode::Fast]),
            default_mode: ToolExecutionMode::Fast,
            streaming_policy: None,
            detached_policy: None,
            detached_restart_classes: BTreeSet::new(),
            core_deadline: CoreDispatchDeadline::Applies,
        }
    }
}

impl ToolExecutionContract {
    pub fn new(
        supported_modes: BTreeSet<ToolExecutionMode>,
        default_mode: ToolExecutionMode,
        streaming_policy: Option<StreamingToolExecutionPolicy>,
        detached_policy: Option<DetachedToolExecutionPolicy>,
    ) -> Result<Self, ToolExecutionContractError> {
        if supported_modes.is_empty() {
            return Err(ToolExecutionContractError::NoSupportedModes);
        }
        if !supported_modes.contains(&default_mode) {
            return Err(ToolExecutionContractError::DefaultModeUnsupported { default_mode });
        }
        match (
            supported_modes.contains(&ToolExecutionMode::Streaming),
            streaming_policy.is_some(),
        ) {
            (true, false) => return Err(ToolExecutionContractError::MissingStreamingPolicy),
            (false, true) => return Err(ToolExecutionContractError::UnexpectedStreamingPolicy),
            _ => {}
        }
        match (
            supported_modes.contains(&ToolExecutionMode::Detached),
            detached_policy.is_some(),
        ) {
            (true, false) => return Err(ToolExecutionContractError::MissingDetachedPolicy),
            (false, true) => return Err(ToolExecutionContractError::UnexpectedDetachedPolicy),
            _ => {}
        }
        let detached_restart_classes = detached_policy
            .as_ref()
            .map(|policy| BTreeSet::from([policy.restart_class()]))
            .unwrap_or_default();
        Ok(Self {
            supported_modes,
            default_mode,
            streaming_policy,
            detached_policy,
            detached_restart_classes,
            core_deadline: CoreDispatchDeadline::Applies,
        })
    }

    /// Declare that this tool owns its lifetime bound: the core dispatch
    /// default deadline no longer applies to it.
    #[must_use]
    pub fn with_tool_owned_deadline(mut self) -> Self {
        self.core_deadline = CoreDispatchDeadline::ToolOwned;
        self
    }

    pub const fn core_deadline(&self) -> CoreDispatchDeadline {
        self.core_deadline
    }

    pub fn supported_modes(&self) -> &BTreeSet<ToolExecutionMode> {
        &self.supported_modes
    }

    pub const fn default_mode(&self) -> ToolExecutionMode {
        self.default_mode
    }

    pub fn streaming_policy(&self) -> Option<&StreamingToolExecutionPolicy> {
        self.streaming_policy.as_ref()
    }

    pub fn detached_policy(&self) -> Option<&DetachedToolExecutionPolicy> {
        self.detached_policy.as_ref()
    }

    pub fn detached_restart_classes(&self) -> &BTreeSet<RestartClass> {
        &self.detached_restart_classes
    }

    pub fn with_detached_restart_classes(
        mut self,
        restart_classes: BTreeSet<RestartClass>,
    ) -> Result<Self, ToolExecutionContractError> {
        let policy = self
            .detached_policy
            .as_ref()
            .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
        if !restart_classes.contains(&policy.restart_class()) {
            return Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported {
                    restart_class: policy.restart_class(),
                },
            );
        }
        self.detached_restart_classes = restart_classes;
        Ok(self)
    }

    pub fn resolve_default(
        &self,
        deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        self.resolve(self.default_mode, deadlines)
    }

    pub fn resolve(
        &self,
        mode: ToolExecutionMode,
        mut deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        if !self.supported_modes.contains(&mode) {
            return Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: mode,
            });
        }
        let (
            kind,
            runner,
            restart_class,
            idempotency_scope,
            credential_context_refs,
            output_policy,
            progress_policy,
        ) = match mode {
            ToolExecutionMode::Fast => (
                ResolvedExecutionKind::Fast,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::NotApplicable,
                ToolExecutionApplicability::Applicable(ToolOutputPolicy::InlineTerminal),
                ToolExecutionApplicability::NotApplicable,
            ),
            ToolExecutionMode::Streaming => {
                let policy = self
                    .streaming_policy
                    .clone()
                    .ok_or(ToolExecutionContractError::MissingStreamingPolicy)?;
                deadlines.contributors.push(ToolDeadlineContributor::finite(
                    ToolDeadlineOwner::StreamingAbsolute,
                    policy.absolute_timeout(),
                ));
                (
                    ResolvedExecutionKind::Streaming(policy),
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::NotApplicable,
                    ToolExecutionApplicability::Applicable(ToolOutputPolicy::StreamingEvents),
                    ToolExecutionApplicability::Applicable(ToolProgressPolicy::StreamingEvents),
                )
            }
            ToolExecutionMode::Detached => {
                let policy = self
                    .detached_policy
                    .clone()
                    .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
                deadlines.contributors.push(ToolDeadlineContributor::finite(
                    ToolDeadlineOwner::DetachedSubmission,
                    policy.submission_timeout(),
                ));
                (
                    ResolvedExecutionKind::Detached(policy.clone()),
                    ToolExecutionApplicability::Applicable(policy.runner().clone()),
                    ToolExecutionApplicability::Applicable(policy.restart_class()),
                    ToolExecutionApplicability::Applicable(policy.idempotency_scope()),
                    ToolExecutionApplicability::Applicable(vec![
                        ToolCredentialContextRef::OwningProfile {
                            required_scopes: policy.credential_scopes().clone(),
                        },
                    ]),
                    ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult),
                    ToolExecutionApplicability::Applicable(ToolProgressPolicy::DurableJobEvents),
                )
            }
        };
        Ok(ResolvedToolExecutionPlan {
            deadlines,
            kind,
            runner,
            restart_class,
            idempotency_scope,
            credential_context_refs,
            output_policy,
            progress_policy,
            owner_witnesses: Vec::new(),
            resolved_call: None,
            root_dispatcher: None,
            resolution_context: None,
            core_deadline: self.core_deadline,
        })
    }

    pub fn resolve_detached_with_restart_class(
        &self,
        restart_class: RestartClass,
        deadlines: ToolDeadlineChain,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionContractError> {
        if !self.detached_restart_classes.contains(&restart_class) {
            return Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported { restart_class },
            );
        }
        let mut policy = self
            .detached_policy
            .clone()
            .ok_or(ToolExecutionContractError::MissingDetachedPolicy)?;
        policy.restart_class = restart_class;
        let mut plan = self.resolve(ToolExecutionMode::Detached, deadlines)?;
        plan.kind = ResolvedExecutionKind::Detached(policy);
        plan.restart_class = ToolExecutionApplicability::Applicable(restart_class);
        Ok(plan)
    }

    /// Validate that a resolver's selected mode and every mode-derived facet
    /// stay within this advertised catalog contract.
    ///
    /// Hybrid resolvers remain free to select any advertised mode from typed
    /// arguments. They may append owner witnesses and additional deadline
    /// contributors, but cannot invent a mode, runner, restart/idempotency
    /// declaration, credential context, or output/progress policy that the
    /// catalog did not advertise.
    pub fn validate_resolved_plan(
        &self,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionContractError> {
        let mode = plan.mode();
        if !self.supported_modes.contains(&mode) {
            return Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: mode,
            });
        }
        let comparison_deadline = ToolDeadlineChain {
            contributors: vec![ToolDeadlineContributor::unbounded(
                ToolDeadlineOwner::Dispatcher,
            )],
        };
        let expected = if mode == ToolExecutionMode::Detached {
            let ToolExecutionApplicability::Applicable(restart_class) = plan.restart_class else {
                return Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                    mode,
                    facet: "restart_class",
                });
            };
            self.resolve_detached_with_restart_class(restart_class, comparison_deadline)?
        } else {
            self.resolve(mode, comparison_deadline)?
        };

        macro_rules! require_facet {
            ($field:ident) => {
                if plan.$field != expected.$field {
                    return Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                        mode,
                        facet: stringify!($field),
                    });
                }
            };
        }

        require_facet!(kind);
        require_facet!(runner);
        require_facet!(restart_class);
        require_facet!(idempotency_scope);
        require_facet!(credential_context_refs);
        require_facet!(output_policy);
        require_facet!(progress_policy);
        Ok(())
    }
}

/// Mode-specific part of a resolved execution plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedExecutionKind {
    Fast,
    Streaming(StreamingToolExecutionPolicy),
    Detached(DetachedToolExecutionPolicy),
}

/// Exact execution and deadline plan selected before tool dispatch.
#[derive(Clone)]
pub struct ResolvedToolExecutionPlan {
    deadlines: ToolDeadlineChain,
    kind: ResolvedExecutionKind,
    runner: ToolExecutionApplicability<RunnerIdentity>,
    restart_class: ToolExecutionApplicability<RestartClass>,
    idempotency_scope: ToolExecutionApplicability<IdempotencyScope>,
    credential_context_refs: ToolExecutionApplicability<Vec<ToolCredentialContextRef>>,
    output_policy: ToolExecutionApplicability<ToolOutputPolicy>,
    progress_policy: ToolExecutionApplicability<ToolProgressPolicy>,
    owner_witnesses: Vec<ToolExecutionOwnerWitness>,
    resolved_call: Option<ResolvedToolCallIdentity>,
    root_dispatcher: Option<RootDispatcherLease>,
    /// The caller's resolution context the fenced root resolution used, so
    /// the same call can be resolved afresh against another allocation.
    resolution_context: Option<ToolExecutionResolutionContext>,
    core_deadline: CoreDispatchDeadline,
}

/// The authority key of the owner witness the fenced root resolution
/// attaches for the root dispatcher (see
/// [`crate::resolve_tool_execution_plan_fenced`]).
pub(crate) const ROOT_DISPATCHER_WITNESS: &str = "root-dispatcher";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedToolCallIdentity {
    tool_use_id: String,
    tool_name: crate::ToolName,
    canonical_arguments_sha256: [u8; 32],
}

trait ErasedRootDispatcherLease: Send + Sync {
    fn data_ptr(&self) -> *const ();
}

struct TypedRootDispatcherLease<T: ?Sized + Send + Sync + 'static> {
    dispatcher: Arc<T>,
}

impl<T: ?Sized + Send + Sync + 'static> ErasedRootDispatcherLease for TypedRootDispatcherLease<T> {
    fn data_ptr(&self) -> *const () {
        Arc::as_ptr(&self.dispatcher).cast::<()>()
    }
}

#[derive(Clone)]
struct RootDispatcherLease(Arc<dyn ErasedRootDispatcherLease>);

impl RootDispatcherLease {
    fn new<T: ?Sized + Send + Sync + 'static>(dispatcher: Arc<T>) -> Self {
        Self(Arc::new(TypedRootDispatcherLease { dispatcher }))
    }

    fn matches<T: ?Sized + Send + Sync + 'static>(&self, dispatcher: &Arc<T>) -> bool {
        self.0.data_ptr() == Arc::as_ptr(dispatcher).cast::<()>()
    }
}

impl std::fmt::Debug for ResolvedToolExecutionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedToolExecutionPlan")
            .field("deadlines", &self.deadlines)
            .field("kind", &self.kind)
            .field("runner", &self.runner)
            .field("restart_class", &self.restart_class)
            .field("idempotency_scope", &self.idempotency_scope)
            .field("credential_context_refs", &self.credential_context_refs)
            .field("output_policy", &self.output_policy)
            .field("progress_policy", &self.progress_policy)
            .field("owner_witnesses", &self.owner_witnesses)
            .field("resolved_call", &self.resolved_call)
            .field("has_root_dispatcher_lease", &self.root_dispatcher.is_some())
            .field("resolution_context", &self.resolution_context)
            .finish()
    }
}

impl PartialEq for ResolvedToolExecutionPlan {
    fn eq(&self, other: &Self) -> bool {
        self.deadlines == other.deadlines
            && self.kind == other.kind
            && self.runner == other.runner
            && self.restart_class == other.restart_class
            && self.idempotency_scope == other.idempotency_scope
            && self.credential_context_refs == other.credential_context_refs
            && self.output_policy == other.output_policy
            && self.progress_policy == other.progress_policy
            && self.owner_witnesses == other.owner_witnesses
            && self.resolved_call == other.resolved_call
            && match (&self.root_dispatcher, &other.root_dispatcher) {
                (Some(left), Some(right)) => left.0.data_ptr() == right.0.data_ptr(),
                (None, None) => true,
                (Some(_), None) | (None, Some(_)) => false,
            }
    }
}

impl Eq for ResolvedToolExecutionPlan {}

impl ResolvedToolExecutionPlan {
    pub const fn mode(&self) -> ToolExecutionMode {
        match self.kind {
            ResolvedExecutionKind::Fast => ToolExecutionMode::Fast,
            ResolvedExecutionKind::Streaming(_) => ToolExecutionMode::Streaming,
            ResolvedExecutionKind::Detached(_) => ToolExecutionMode::Detached,
        }
    }

    pub fn deadlines(&self) -> &ToolDeadlineChain {
        &self.deadlines
    }

    /// The deadline the agent loop enforces for this call. The chain itself
    /// is never rewritten (plans must extend their upstream chain); a tool
    /// that owns its lifetime bound only drops the core dispatch DEFAULT. An
    /// explicit per-tool override the operator configured for the tool still
    /// bounds it.
    pub fn effective_timeout(&self) -> Option<Duration> {
        match self.core_deadline {
            CoreDispatchDeadline::Applies => self.deadlines.effective_timeout(),
            CoreDispatchDeadline::ToolOwned => self
                .deadlines
                .contributors
                .iter()
                .filter(|contributor| {
                    contributor.owner != ToolDeadlineOwner::CoreToolDispatch
                        || contributor.source != ToolDeadlineSource::OwnerDefault
                })
                .filter_map(|contributor| contributor.timeout)
                .min(),
        }
    }

    pub fn kind(&self) -> &ResolvedExecutionKind {
        &self.kind
    }

    pub fn runner(&self) -> &ToolExecutionApplicability<RunnerIdentity> {
        &self.runner
    }

    pub const fn restart_class(&self) -> ToolExecutionApplicability<RestartClass> {
        match self.restart_class {
            ToolExecutionApplicability::NotApplicable => ToolExecutionApplicability::NotApplicable,
            ToolExecutionApplicability::Applicable(value) => {
                ToolExecutionApplicability::Applicable(value)
            }
        }
    }

    pub const fn idempotency_scope(&self) -> ToolExecutionApplicability<IdempotencyScope> {
        match self.idempotency_scope {
            ToolExecutionApplicability::NotApplicable => ToolExecutionApplicability::NotApplicable,
            ToolExecutionApplicability::Applicable(value) => {
                ToolExecutionApplicability::Applicable(value)
            }
        }
    }

    pub fn credential_context_refs(
        &self,
    ) -> &ToolExecutionApplicability<Vec<ToolCredentialContextRef>> {
        &self.credential_context_refs
    }

    pub const fn output_policy(&self) -> &ToolExecutionApplicability<ToolOutputPolicy> {
        &self.output_policy
    }

    pub const fn progress_policy(&self) -> &ToolExecutionApplicability<ToolProgressPolicy> {
        &self.progress_policy
    }

    pub fn owner_witness(&self, authority_key: &str) -> Option<&ToolExecutionOwnerWitness> {
        self.owner_witnesses
            .iter()
            .find(|witness| witness.authority_key() == authority_key)
    }

    pub fn owner_witnesses(&self) -> &[ToolExecutionOwnerWitness] {
        &self.owner_witnesses
    }

    /// The one operation target this plan's owners produced, if any: the
    /// target of the single owner witness that names one.
    ///
    /// # Errors
    /// A plan whose owners name more than one target is ambiguous and
    /// unavailable, never resolved to one of them.
    pub fn single_operation_target(
        &self,
    ) -> Result<Option<&ToolOperationTarget>, crate::authorization::OperationAuthorizationError>
    {
        let mut targets = self
            .owner_witnesses
            .iter()
            .filter_map(ToolExecutionOwnerWitness::operation_target);
        match (targets.next(), targets.next()) {
            (target, None) => Ok(target),
            (_, Some(_)) => Err(crate::authorization::OperationAuthorizationError::Unavailable),
        }
    }

    /// Canonical argument digest bound into this plan at the fenced root
    /// resolution seam. Detached execution uses this exact digest for durable
    /// submission identity instead of re-canonicalizing arguments later.
    pub fn canonical_arguments_sha256(&self) -> Option<[u8; 32]> {
        self.resolved_call
            .as_ref()
            .map(|resolved| resolved.canonical_arguments_sha256)
    }

    pub fn with_owner_witness(
        mut self,
        witness: ToolExecutionOwnerWitness,
    ) -> Result<Self, ToolExecutionResolutionError> {
        if let Some(existing) = self.owner_witness(witness.authority_key()).cloned() {
            return Err(ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing: Box::new(existing),
                attempted: Box::new(witness),
            });
        }
        self.owner_witnesses.push(witness);
        Ok(self)
    }

    pub(crate) fn bind_root_dispatch<T: ?Sized + Send + Sync + 'static>(
        mut self,
        dispatcher: Arc<T>,
        call: crate::ToolCallView<'_>,
        resolution_context: &ToolExecutionResolutionContext,
    ) -> Result<Self, ToolExecutionResolutionError> {
        self.resolved_call = Some(ResolvedToolCallIdentity::from_call(call)?);
        self.root_dispatcher = Some(RootDispatcherLease::new(dispatcher));
        self.resolution_context = Some(resolution_context.clone());
        Ok(self)
    }

    /// The caller's resolution context the fenced root resolution used.
    pub(crate) fn resolution_context(&self) -> Option<&ToolExecutionResolutionContext> {
        self.resolution_context.as_ref()
    }

    /// Whether `fresh`, the same call resolved again by another live
    /// allocation of the session's dispatcher in this process, is the same
    /// durable execution as this plan: the same call (its tool use id, name
    /// and canonical arguments), the same declaration digest
    /// ([`ToolBindingDigest`]) of the tool its root resolution resolved
    /// against, the same execution mode with every mode-derived facet
    /// (runner, restart class, idempotency scope, credential references,
    /// output and progress policies), and the same durable operation targets
    /// of its owners ([`ToolOperationTarget::same_durable_target`]: live
    /// connection generations excluded). The mode is part of it, so a hybrid
    /// tool that selects another mode for the call is not the same
    /// execution. Live allocations, witnesses and deadlines are not compared:
    /// `fresh` proves those itself.
    pub(crate) fn same_durable_execution(&self, fresh: &Self) -> bool {
        fn declaration(plan: &ResolvedToolExecutionPlan) -> Option<ToolBindingDigest> {
            plan.owner_witness(ROOT_DISPATCHER_WITNESS)
                .map(|witness| witness.binding_fingerprint().declaration_digest())
        }
        fn durable_targets(
            plan: &ResolvedToolExecutionPlan,
        ) -> impl Iterator<Item = (&str, &ToolOperationTarget)> {
            plan.owner_witnesses.iter().filter_map(|witness| {
                witness
                    .operation_target()
                    .map(|target| (witness.authority_key(), target))
            })
        }
        let declared = declaration(self);
        self.resolved_call.is_some()
            && self.resolved_call == fresh.resolved_call
            && declared.is_some()
            && declared == declaration(fresh)
            && self.kind == fresh.kind
            && self.runner == fresh.runner
            && self.restart_class == fresh.restart_class
            && self.idempotency_scope == fresh.idempotency_scope
            && self.credential_context_refs == fresh.credential_context_refs
            && self.output_policy == fresh.output_policy
            && self.progress_policy == fresh.progress_policy
            && durable_targets(self).count() == durable_targets(fresh).count()
            && durable_targets(self).zip(durable_targets(fresh)).all(
                |((key, target), (fresh_key, fresh_target))| {
                    key == fresh_key && target.same_durable_target(fresh_target)
                },
            )
    }

    pub(crate) fn validate_root_dispatch<T: ?Sized + Send + Sync + 'static>(
        &self,
        dispatcher: &Arc<T>,
        call: crate::ToolCallView<'_>,
    ) -> Result<(), ToolExecutionResolutionError> {
        let Some(root_dispatcher) = self.root_dispatcher.as_ref() else {
            return Err(ToolExecutionResolutionError::RootDispatcherChanged {
                tool_name: call.name.to_string(),
            });
        };
        if !root_dispatcher.matches(dispatcher) {
            return Err(ToolExecutionResolutionError::RootDispatcherChanged {
                tool_name: call.name.to_string(),
            });
        }
        let actual = ResolvedToolCallIdentity::from_call(call)?;
        if self.resolved_call.as_ref() != Some(&actual) {
            return Err(ToolExecutionResolutionError::ResolvedCallMismatch {
                tool_name: call.name.to_string(),
            });
        }
        Ok(())
    }
}

impl ResolvedToolCallIdentity {
    fn from_call(call: crate::ToolCallView<'_>) -> Result<Self, ToolExecutionResolutionError> {
        let arguments: serde_json::Value =
            serde_json::from_str(call.args.get()).map_err(|error| {
                ToolExecutionResolutionError::InvalidArguments {
                    tool_name: call.name.to_string(),
                    reason: error.to_string(),
                }
            })?;
        let mut canonical = Vec::new();
        write_canonical_json(&arguments, &mut canonical).map_err(|error| {
            ToolExecutionResolutionError::InvalidArguments {
                tool_name: call.name.to_string(),
                reason: error.to_string(),
            }
        })?;
        Ok(Self {
            tool_use_id: call.id.to_string(),
            tool_name: call.name.into(),
            canonical_arguments_sha256: Sha256::digest(canonical).into(),
        })
    }
}

fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), serde_json::Error> {
    match value {
        serde_json::Value::Object(object) => {
            output.push(b'{');
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_canonical_json(&object[key], output)?;
            }
            output.push(b'}');
        }
        serde_json::Value::Array(array) => {
            output.push(b'[');
            for (index, item) in array.iter().enumerate() {
                if index > 0 {
                    output.push(b',');
                }
                write_canonical_json(item, output)?;
            }
            output.push(b']');
        }
        scalar => serde_json::to_writer(output, scalar)?,
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    fn finite(owner: ToolDeadlineOwner, seconds: u64) -> ToolDeadlineContributor {
        ToolDeadlineContributor::finite(owner, Duration::from_secs(seconds))
    }

    fn ephemeral_fingerprint(label: &str) -> EphemeralToolBindingFingerprint {
        ephemeral_tool_catalog_binding_fingerprint(&crate::ToolCatalogEntry::session_inline(
            Arc::new(crate::ToolDef::new(
                format!("test_{label}"),
                label,
                serde_json::json!({"type": "object"}),
            )),
            true,
        ))
    }

    #[test]
    fn deadline_chain_rejects_empty_and_zero_deadlines() {
        assert_eq!(
            ToolDeadlineChain::new(Vec::new()),
            Err(DeadlineChainError::Empty)
        );
        assert_eq!(
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 0,)]),
            Err(DeadlineChainError::Zero {
                owner: ToolDeadlineOwner::CoreToolDispatch,
            })
        );
    }

    #[test]
    fn deadline_chain_retains_every_contributor_and_selects_narrowest() {
        let chain = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::CoreToolDispatch, 600),
            finite(ToolDeadlineOwner::MetaPublicCallback, 120),
            finite(ToolDeadlineOwner::SdkCallbackCancellation, 125),
            finite(ToolDeadlineOwner::GatewayWire, 130),
        ])
        .expect("valid deadline chain");

        assert_eq!(chain.contributors().len(), 4);
        assert_eq!(chain.effective_timeout(), Some(Duration::from_mins(2)));
        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::MetaPublicCallback)
        );
        assert_eq!(
            chain.diagnostic(),
            "effective deadline: 120s\ncontributors:\n  core tool dispatch: 600s\n  meta public callback: 120s\n  sdk callback cancellation: 125s\n  gateway wire: 130s\nwinner: meta public callback"
        );
    }

    #[test]
    fn equal_deadlines_use_declaration_order_as_deterministic_tie_breaker() {
        let chain = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::Dispatcher, 30),
            finite(ToolDeadlineOwner::ToolInternal, 30),
        ])
        .expect("valid deadline chain");

        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::Dispatcher)
        );
    }

    #[test]
    fn unbounded_contributors_are_diagnostic_but_never_win_a_finite_chain() {
        let chain = ToolDeadlineChain::new(vec![
            ToolDeadlineContributor::unbounded(ToolDeadlineOwner::DirectCaller),
            finite(ToolDeadlineOwner::ToolInternal, 30),
        ])
        .expect("valid deadline chain");

        assert_eq!(chain.effective_timeout(), Some(Duration::from_secs(30)));
        assert_eq!(
            chain.winner().map(ToolDeadlineContributor::owner),
            Some(ToolDeadlineOwner::ToolInternal)
        );
        assert!(chain.diagnostic().contains("direct caller: unbounded"));
    }

    #[test]
    fn deadline_diagnostic_never_rounds_a_nonzero_timeout_to_zero() {
        let chain = ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::ToolInternal,
            Duration::from_nanos(1),
        )])
        .expect("valid deadline chain");

        assert!(chain.diagnostic().contains("effective deadline: 1ns"));
        assert!(chain.diagnostic().contains("tool internal: 1ns"));
        assert!(!chain.diagnostic().contains("0ms"));
    }

    #[test]
    fn tool_owned_deadline_lifts_only_the_core_dispatch_default() {
        let chain = ToolDeadlineChain::new(vec![
            ToolDeadlineContributor::finite(
                ToolDeadlineOwner::CoreToolDispatch,
                Duration::from_mins(10),
            ),
            ToolDeadlineContributor::finite(
                ToolDeadlineOwner::GatewayWire,
                Duration::from_mins(15),
            ),
        ])
        .expect("chain");
        let default_plan = ToolExecutionContract::default()
            .resolve_default(chain.clone())
            .expect("default plan");
        assert_eq!(
            default_plan.effective_timeout(),
            Some(Duration::from_mins(10))
        );
        let owned_plan = ToolExecutionContract::default()
            .with_tool_owned_deadline()
            .resolve_default(chain)
            .expect("tool-owned plan");
        assert_eq!(
            owned_plan.effective_timeout(),
            Some(Duration::from_mins(15)),
            "only the core default is lifted; other owners still bound the call"
        );
        assert_eq!(
            owned_plan.deadlines(),
            default_plan.deadlines(),
            "the chain is not rewritten, so the plan still extends its upstream"
        );
    }

    #[test]
    fn tool_owned_deadline_keeps_an_operator_per_tool_override() {
        // `tools.tool_timeouts.<tool>` is an explicit operator cap, not the
        // core default, so a tool that owns its lifetime bound still obeys it.
        let chain = ToolDeadlineChain::new(vec![ToolDeadlineContributor::per_tool_override(
            ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_mins(5),
        )])
        .expect("chain");
        let owned_plan = ToolExecutionContract::default()
            .with_tool_owned_deadline()
            .resolve_default(chain.clone())
            .expect("tool-owned plan");
        assert_eq!(
            owned_plan.effective_timeout(),
            Some(Duration::from_mins(5)),
            "an explicit per-tool override bounds a tool-owned call"
        );
        assert!(
            owned_plan
                .deadlines()
                .diagnostic()
                .contains("per-tool override")
        );

        // Only the default is lifted: with no override the call is unbounded.
        let default_chain = ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_mins(10),
        )])
        .expect("chain");
        let owned_default = ToolExecutionContract::default()
            .with_tool_owned_deadline()
            .resolve_default(default_chain)
            .expect("tool-owned plan");
        assert_eq!(owned_default.effective_timeout(), None);

        // A tool that does not own its bound sees the override as before.
        let ordinary = ToolExecutionContract::default()
            .resolve_default(chain)
            .expect("default plan");
        assert_eq!(ordinary.effective_timeout(), Some(Duration::from_mins(5)));
    }

    #[test]
    fn execution_contract_default_is_fast_only() {
        let contract = ToolExecutionContract::default();

        assert_eq!(contract.default_mode(), ToolExecutionMode::Fast);
        assert_eq!(
            contract.supported_modes(),
            &BTreeSet::from([ToolExecutionMode::Fast])
        );
        assert!(contract.streaming_policy().is_none());
        assert!(contract.detached_policy().is_none());
    }

    #[test]
    fn execution_contract_rejects_incoherent_mode_policies() {
        let streaming =
            StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_mins(1))
                .expect("valid streaming policy");
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");

        assert_eq!(
            ToolExecutionContract::new(BTreeSet::new(), ToolExecutionMode::Fast, None, None,),
            Err(ToolExecutionContractError::NoSupportedModes)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Detached,
                None,
                None,
            ),
            Err(ToolExecutionContractError::DefaultModeUnsupported {
                default_mode: ToolExecutionMode::Detached,
            })
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Fast,
                Some(streaming),
                None,
            ),
            Err(ToolExecutionContractError::UnexpectedStreamingPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Streaming]),
                ToolExecutionMode::Streaming,
                None,
                None,
            ),
            Err(ToolExecutionContractError::MissingStreamingPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Fast]),
                ToolExecutionMode::Fast,
                None,
                Some(detached),
            ),
            Err(ToolExecutionContractError::UnexpectedDetachedPolicy)
        );
        assert_eq!(
            ToolExecutionContract::new(
                BTreeSet::from([ToolExecutionMode::Detached]),
                ToolExecutionMode::Detached,
                None,
                None,
            ),
            Err(ToolExecutionContractError::MissingDetachedPolicy)
        );
    }

    #[test]
    fn detached_resolution_carries_typed_runner_restart_and_idempotency() {
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy")
        .with_credential_scopes(["network"]);
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached.clone()),
        )
        .expect("valid detached contract");
        let deadlines =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");

        let plan = contract
            .resolve_default(deadlines)
            .expect("default mode resolves");

        assert_eq!(plan.mode(), ToolExecutionMode::Detached);
        assert_eq!(
            plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::DetachedSubmission, 10),
            ]
        );
        assert_eq!(plan.kind(), &ResolvedExecutionKind::Detached(detached));
    }

    #[test]
    fn mode_resolution_appends_its_own_absolute_deadline() {
        let upstream =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");
        let streaming =
            StreamingToolExecutionPolicy::new(Duration::from_secs(5), Duration::from_mins(1))
                .expect("valid streaming policy");
        let streaming_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Streaming]),
            ToolExecutionMode::Streaming,
            Some(streaming),
            None,
        )
        .expect("valid streaming contract");

        let streaming_plan = streaming_contract
            .resolve_default(upstream.clone())
            .expect("streaming mode resolves");
        assert_eq!(
            streaming_plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::StreamingAbsolute, 60),
            ]
        );

        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");
        let detached_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");

        let detached_plan = detached_contract
            .resolve_default(upstream)
            .expect("detached mode resolves");
        assert_eq!(
            detached_plan.deadlines().contributors(),
            &[
                finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                finite(ToolDeadlineOwner::DetachedSubmission, 10),
            ]
        );
    }

    #[test]
    fn advertised_contract_rejects_an_unadvertised_resolved_mode() {
        let upstream =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid upstream deadline");
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("dishonest.runner", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(10),
        )
        .expect("valid detached policy");
        let detached_contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");
        let detached_plan = detached_contract
            .resolve_default(upstream)
            .expect("detached plan resolves");

        assert_eq!(
            ToolExecutionContract::default().validate_resolved_plan(&detached_plan),
            Err(ToolExecutionContractError::RequestedModeUnsupported {
                requested_mode: ToolExecutionMode::Detached,
            })
        );
    }

    #[test]
    fn advertised_contract_rejects_forged_mode_derived_facets() {
        let contract = ToolExecutionContract::default();
        let mut plan = contract
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast plan resolves");
        plan.output_policy =
            ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult);

        assert_eq!(
            contract.validate_resolved_plan(&plan),
            Err(ToolExecutionContractError::ResolvedPlanFacetMismatch {
                mode: ToolExecutionMode::Fast,
                facet: "output_policy",
            })
        );
    }

    #[test]
    fn logical_binding_fingerprint_ignores_projection_arc_rebuild() {
        let tool = || {
            std::sync::Arc::new(crate::ToolDef::new(
                "stable_tool",
                "stable declaration",
                serde_json::json!({"type": "object", "properties": {}}),
            ))
        };
        let original = crate::ToolCatalogEntry::session_inline(tool(), true);
        let rebuilt = crate::ToolCatalogEntry::session_inline(tool(), true);
        let changed = crate::ToolCatalogEntry::session_inline(
            std::sync::Arc::new(crate::ToolDef::new(
                "stable_tool",
                "replacement declaration",
                serde_json::json!({"type": "object", "properties": {}}),
            )),
            true,
        );

        assert_eq!(
            ephemeral_tool_catalog_binding_fingerprint(&original),
            ephemeral_tool_catalog_binding_fingerprint(&rebuilt),
            "equivalent projection allocations are one live logical binding"
        );
        assert_ne!(
            ephemeral_tool_catalog_binding_fingerprint(&original),
            ephemeral_tool_catalog_binding_fingerprint(&changed),
            "name reuse with a different declaration must be fenced"
        );
    }

    #[test]
    fn tool_audience_changes_invalidate_execution_binding_fingerprints() {
        let model =
            crate::ToolDef::new("tool", "declaration", serde_json::json!({"type": "object"}));
        let app = model.clone().with_audience(crate::ToolAudience::App);
        let model = crate::ToolCatalogEntry::session_inline(std::sync::Arc::new(model), true);
        let app = crate::ToolCatalogEntry::session_inline(std::sync::Arc::new(app), true);
        assert_ne!(
            ephemeral_tool_catalog_binding_fingerprint(&model),
            ephemeral_tool_catalog_binding_fingerprint(&app)
        );
    }

    #[test]
    fn resolution_context_rejects_replaced_or_reordered_upstream_deadlines() {
        let upstream = ToolDeadlineChain::new(vec![
            finite(ToolDeadlineOwner::CoreToolDispatch, 600),
            finite(ToolDeadlineOwner::Dispatcher, 30),
        ])
        .expect("valid upstream deadline");
        let context = ToolExecutionResolutionContext::new(upstream.clone());
        let valid_plan = ToolExecutionContract::default()
            .resolve_default(
                upstream
                    .with_contributor(finite(ToolDeadlineOwner::ToolInternal, 10))
                    .expect("valid extension"),
            )
            .expect("default contract resolves");
        context
            .validate_resolved_plan(&valid_plan)
            .expect("ordered extension is valid");

        let replaced_plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![
                    finite(ToolDeadlineOwner::CoreToolDispatch, 600),
                    finite(ToolDeadlineOwner::ToolInternal, 10),
                ])
                .expect("valid but replaced chain"),
            )
            .expect("default contract resolves");
        assert_eq!(
            context.validate_resolved_plan(&replaced_plan),
            Err(ToolExecutionResolutionError::DeadlineExtension(
                DeadlineChainExtensionError::ContributorMismatch {
                    index: 1,
                    expected: finite(ToolDeadlineOwner::Dispatcher, 30),
                    actual: finite(ToolDeadlineOwner::ToolInternal, 10),
                }
            ))
        );

        let shorter_plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid but shorter chain"),
            )
            .expect("default contract resolves");
        assert_eq!(
            context.validate_resolved_plan(&shorter_plan),
            Err(ToolExecutionResolutionError::DeadlineExtension(
                DeadlineChainExtensionError::ShorterThanUpstream {
                    upstream_len: 2,
                    resolved_len: 1,
                }
            ))
        );
    }

    #[test]
    fn resolved_plan_exposes_explicit_mode_applicability_and_owner_witness() {
        let detached = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("example.security_scan", "v1").expect("valid runner"),
            RestartClass::NonResumable,
            IdempotencyScope::InteractionAndArguments,
            Duration::from_secs(10),
        )
        .expect("valid detached policy")
        .with_credential_scopes(["network"]);
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(detached),
        )
        .expect("valid detached contract");
        let witness = ToolExecutionOwnerWitness::new(
            "dynamic-composite:1",
            "security_scan",
            ephemeral_fingerprint("security_scan"),
        )
        .expect("valid owner witness");
        let plan = contract
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("detached contract resolves")
            .with_owner_witness(witness.clone())
            .expect("first owner witness is accepted");

        assert_eq!(
            plan.runner(),
            &ToolExecutionApplicability::Applicable(
                RunnerIdentity::new("example.security_scan", "v1").expect("valid runner")
            )
        );
        assert_eq!(
            plan.restart_class(),
            ToolExecutionApplicability::Applicable(RestartClass::NonResumable)
        );
        assert_eq!(
            plan.idempotency_scope(),
            ToolExecutionApplicability::Applicable(IdempotencyScope::InteractionAndArguments)
        );
        assert_eq!(
            plan.credential_context_refs(),
            &ToolExecutionApplicability::Applicable(vec![
                ToolCredentialContextRef::OwningProfile {
                    required_scopes: BTreeSet::from(["network".to_string()]),
                },
            ])
        );
        assert_eq!(
            plan.output_policy(),
            &ToolExecutionApplicability::Applicable(ToolOutputPolicy::DurableJobResult)
        );
        assert_eq!(
            plan.progress_policy(),
            &ToolExecutionApplicability::Applicable(ToolProgressPolicy::DurableJobEvents)
        );
        assert_eq!(plan.owner_witness("dynamic-composite:1"), Some(&witness));

        let fast = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast contract resolves");
        assert_eq!(fast.runner(), &ToolExecutionApplicability::NotApplicable);
        assert_eq!(
            fast.credential_context_refs(),
            &ToolExecutionApplicability::NotApplicable
        );
        assert_eq!(
            fast.output_policy(),
            &ToolExecutionApplicability::Applicable(ToolOutputPolicy::InlineTerminal)
        );
        assert_eq!(
            fast.progress_policy(),
            &ToolExecutionApplicability::NotApplicable
        );
    }

    #[test]
    fn detached_contract_can_advertise_and_validate_call_resolved_restart_classes() {
        let policy = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("meerkat.monitor_script", "v1").expect("runner"),
            RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            Duration::from_secs(30),
        )
        .expect("policy");
        let contract = ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .expect("contract")
        .with_detached_restart_classes(BTreeSet::from([
            RestartClass::Replayable,
            RestartClass::CheckpointResumable,
            RestartClass::NonResumable,
        ]))
        .expect("restart set");
        let deadlines =
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("deadlines");
        let resolved = contract
            .resolve_detached_with_restart_class(
                RestartClass::CheckpointResumable,
                deadlines.clone(),
            )
            .expect("resolved");
        assert_eq!(
            resolved.restart_class(),
            ToolExecutionApplicability::Applicable(RestartClass::CheckpointResumable)
        );
        contract
            .validate_resolved_plan(&resolved)
            .expect("resolved class remains within the advertised contract");
        assert_eq!(
            contract.resolve_detached_with_restart_class(RestartClass::Adoptable, deadlines),
            Err(
                ToolExecutionContractError::RequestedRestartClassUnsupported {
                    restart_class: RestartClass::Adoptable
                }
            )
        );
    }

    #[test]
    fn owner_witness_rejects_blank_owner_keys() {
        assert_eq!(
            ToolExecutionOwnerWitness::new(
                "authority:a",
                "  ",
                ephemeral_fingerprint("blank-owner"),
            ),
            Err(ToolExecutionDeclarationError::EmptyOwnerWitnessKey)
        );

        assert_eq!(
            ToolExecutionOwnerWitness::new(
                "  ",
                "owner:a",
                ephemeral_fingerprint("blank-authority"),
            ),
            Err(ToolExecutionDeclarationError::EmptyOwnerWitnessAuthorityKey)
        );

        let original = ToolExecutionOwnerWitness::new(
            "authority:a",
            "owner:a",
            ephemeral_fingerprint("original"),
        )
        .expect("valid witness");
        let nested = ToolExecutionOwnerWitness::new(
            "authority:b",
            "owner:b",
            ephemeral_fingerprint("nested"),
        )
        .expect("valid witness");
        let attempted = ToolExecutionOwnerWitness::new(
            "authority:a",
            "owner:c",
            ephemeral_fingerprint("attempted"),
        )
        .expect("valid witness");
        let plan = ToolExecutionContract::default()
            .resolve_default(
                ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                    .expect("valid deadline"),
            )
            .expect("fast contract resolves")
            .with_owner_witness(original.clone())
            .expect("first witness is accepted")
            .with_owner_witness(nested.clone())
            .expect("a nested authority may append its own witness");
        assert_eq!(plan.owner_witness("authority:a"), Some(&original));
        assert_eq!(plan.owner_witness("authority:b"), Some(&nested));
        assert_eq!(
            plan.with_owner_witness(attempted.clone()),
            Err(ToolExecutionResolutionError::OwnerWitnessAlreadyAssigned {
                existing: Box::new(original),
                attempted: Box::new(attempted),
            })
        );
    }

    fn callback_provenance() -> crate::ToolProvenance {
        crate::ToolProvenance::new(crate::ToolSourceKind::Callback, "conn-1")
    }

    fn detached_contract(
        version: &str,
        restart_class: RestartClass,
        submission_timeout: Duration,
        scopes: &[&str],
    ) -> ToolExecutionContract {
        let policy = DetachedToolExecutionPolicy::new(
            RunnerIdentity::new("host-runner", version).expect("valid runner"),
            restart_class,
            IdempotencyScope::ToolCall,
            submission_timeout,
        )
        .expect("valid detached policy")
        .with_credential_scopes(scopes.iter().copied());
        ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Detached]),
            ToolExecutionMode::Detached,
            None,
            Some(policy),
        )
        .expect("valid detached contract")
    }

    /// The detached callback tool the digest tests vary one facet of.
    fn deploy_entry(
        tool: crate::ToolDef,
        execution: ToolExecutionContract,
    ) -> crate::ToolCatalogEntry {
        crate::ToolCatalogEntry::session_deferred(Arc::new(tool), true, callback_provenance())
            .with_execution_contract(execution)
    }

    fn deploy_tool() -> crate::ToolDef {
        crate::ToolDef::new(
            "deploy",
            "Deploy a build",
            serde_json::json!({"type": "object"}),
        )
        .with_provenance(callback_provenance())
    }

    fn deploy_contract() -> ToolExecutionContract {
        detached_contract(
            "1",
            RestartClass::Replayable,
            Duration::from_secs(30),
            &["repo:read"],
        )
    }

    /// The encoding is durable: these digests were computed independently
    /// from the documented encoding, so a change to it (which would turn
    /// every recorded digest into a mismatch) fails here first.
    #[test]
    fn tool_binding_digest_pins_its_encoding() {
        let inline = crate::ToolCatalogEntry::session_inline(
            Arc::new(crate::ToolDef::new(
                "lookup",
                "Look a record up",
                serde_json::json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            )),
            true,
        );
        assert_eq!(
            ToolBindingDigest::of_catalog_entry(&inline).to_string(),
            "sha256:d558471f05d8b17d8364510e48e72f145a50e26917dad850470b136f4e042e03"
        );
        assert_eq!(
            ToolBindingDigest::of_catalog_entry(&deploy_entry(deploy_tool(), deploy_contract()))
                .to_string(),
            "sha256:61b859a56f10ace3c438bddd666deecec4d3288f5ed7fca4f5a4e78900d564e8"
        );
    }

    /// Equal declarations have equal digests, however their values were
    /// built (object key order, allocations) and whether the tool can be
    /// called right now; every declared facet changes it.
    #[test]
    fn tool_binding_digest_follows_the_declaration_only() {
        let digest = |entry: &crate::ToolCatalogEntry| ToolBindingDigest::of_catalog_entry(entry);
        let base = digest(&deploy_entry(deploy_tool(), deploy_contract()));

        assert_eq!(
            digest(&deploy_entry(deploy_tool(), deploy_contract())),
            base
        );
        let keyed = |text: &str| {
            let mut tool = deploy_tool();
            tool.input_schema = serde_json::from_str(text).expect("json");
            digest(&deploy_entry(tool, deploy_contract()))
        };
        assert_eq!(
            keyed(r#"{"type":"object","required":["id"]}"#),
            keyed(r#"{"required":["id"],"type":"object"}"#)
        );
        let mut unavailable = deploy_entry(deploy_tool(), deploy_contract());
        unavailable.callability = crate::ToolCallability::unavailable(
            crate::ToolUnavailableReason::CallbackRouteUnclaimed,
        );
        assert_eq!(digest(&unavailable), base);

        let mut described = deploy_tool();
        described.description = "Deploy a build somewhere else".into();
        let mut schema = deploy_tool();
        schema.input_schema = serde_json::json!({"type": "object", "required": ["id"]});
        let mut audience = deploy_tool();
        audience.audience = crate::ToolAudience::ModelAndApp;
        let mut source = deploy_tool();
        source.provenance = Some(crate::ToolProvenance::new(
            crate::ToolSourceKind::Callback,
            "conn-2",
        ));
        let mut control = deploy_entry(deploy_tool(), deploy_contract());
        control.plane = crate::ToolPlaneClass::Control;
        let mut inline_only = deploy_entry(deploy_tool(), deploy_contract());
        inline_only.deferred_eligibility = crate::ToolCatalogDeferredEligibility::InlineOnly;
        let mut renamed = deploy_tool();
        renamed.name = "deploy_v2".into();
        let changed = [
            ("name", deploy_entry(renamed, deploy_contract())),
            ("description", deploy_entry(described, deploy_contract())),
            ("input schema", deploy_entry(schema, deploy_contract())),
            ("audience", deploy_entry(audience, deploy_contract())),
            ("provenance", deploy_entry(source, deploy_contract())),
            ("plane", control),
            ("deferred eligibility", inline_only),
            (
                "runner version",
                deploy_entry(
                    deploy_tool(),
                    detached_contract(
                        "2",
                        RestartClass::Replayable,
                        Duration::from_secs(30),
                        &["repo:read"],
                    ),
                ),
            ),
            (
                "restart class",
                deploy_entry(
                    deploy_tool(),
                    detached_contract(
                        "1",
                        RestartClass::NonResumable,
                        Duration::from_secs(30),
                        &["repo:read"],
                    ),
                ),
            ),
            (
                "submission deadline",
                deploy_entry(
                    deploy_tool(),
                    detached_contract(
                        "1",
                        RestartClass::Replayable,
                        Duration::from_secs(31),
                        &["repo:read"],
                    ),
                ),
            ),
            (
                "credential scopes",
                deploy_entry(
                    deploy_tool(),
                    detached_contract(
                        "1",
                        RestartClass::Replayable,
                        Duration::from_secs(30),
                        &["repo:write"],
                    ),
                ),
            ),
            (
                "core deadline",
                deploy_entry(deploy_tool(), deploy_contract().with_tool_owned_deadline()),
            ),
            (
                "execution mode",
                deploy_entry(deploy_tool(), ToolExecutionContract::default()),
            ),
        ];
        for (facet, entry) in changed {
            assert_ne!(digest(&entry), base, "a changed {facet} changes the digest");
        }
    }

    /// A tool that runs inline by default and may run detached.
    fn hybrid_contract() -> ToolExecutionContract {
        ToolExecutionContract::new(
            BTreeSet::from([ToolExecutionMode::Fast, ToolExecutionMode::Detached]),
            ToolExecutionMode::Fast,
            None,
            Some(
                DetachedToolExecutionPolicy::new(
                    RunnerIdentity::new("host-runner", "1").expect("valid runner"),
                    RestartClass::Replayable,
                    IdempotencyScope::ToolCall,
                    Duration::from_secs(30),
                )
                .expect("valid detached policy"),
            ),
        )
        .expect("valid hybrid contract")
    }

    /// One MCP server operation acting with `subject`'s account, on the
    /// server's connection `generation`.
    fn mcp_target(server: &str, generation: u64, subject: &str) -> ToolOperationTarget {
        ToolOperationTarget::McpServerOperation {
            server: server.to_string(),
            connection_generation: generation,
            raw_operation: "deploy".to_string(),
            connector: crate::mcp_config::McpConnectorApproval::NoConnector,
            account: Arc::new(crate::OperationAccount::Selected(
                crate::QualifiedAccount::new("idp.example", subject).expect("valid account"),
            )),
        }
    }

    /// One fenced root resolution of `call` through the allocation
    /// `dispatcher`, against the hybrid tool `description` declares, in
    /// `mode`, with its root witness and an owner witness naming `target`.
    fn fenced_plan(
        dispatcher: &Arc<u8>,
        call: crate::ToolCallView<'_>,
        description: &str,
        mode: ToolExecutionMode,
        target: ToolOperationTarget,
    ) -> ResolvedToolExecutionPlan {
        let entry = crate::ToolCatalogEntry::session_inline(
            Arc::new(crate::ToolDef::new(
                "deploy",
                description,
                serde_json::json!({"type": "object"}),
            )),
            true,
        )
        .with_execution_contract(hybrid_contract());
        let fingerprint = ephemeral_tool_catalog_binding_fingerprint(&entry)
            .with_live_authority(usize::from(**dispatcher), 1);
        let context = ToolExecutionResolutionContext::new(
            ToolDeadlineChain::new(vec![finite(ToolDeadlineOwner::CoreToolDispatch, 600)])
                .expect("valid deadline"),
        );
        entry
            .execution
            .resolve(mode, context.deadlines().clone())
            .expect("the advertised mode resolves")
            .with_owner_witness(
                ToolExecutionOwnerWitness::new(
                    ROOT_DISPATCHER_WITNESS,
                    call.name,
                    fingerprint.clone(),
                )
                .expect("root witness"),
            )
            .expect("root witness accepted")
            .with_owner_witness(
                ToolExecutionOwnerWitness::new("mcp-router", "deploy-owner", fingerprint)
                    .expect("owner witness")
                    .with_operation_target(target),
            )
            .expect("owner witness accepted")
            .bind_root_dispatch(Arc::clone(dispatcher), call, &context)
            .expect("bound to the allocation")
    }

    /// A retained call resolved afresh by another live allocation of the
    /// session's dispatcher is the same durable execution only for the same
    /// call, declaration, execution mode and durable owner targets: a
    /// reconnection's new connection generation and the new allocation's
    /// live authorities alone keep it. The old plan still never enters
    /// through the new allocation.
    #[test]
    fn a_reresolved_plan_is_the_same_durable_execution_only_for_the_same_call_and_owner() {
        let arguments = serde_json::value::RawValue::from_string(r#"{"build":"1"}"#.to_string())
            .expect("arguments");
        let other_arguments =
            serde_json::value::RawValue::from_string(r#"{"build":"2"}"#.to_string())
                .expect("arguments");
        let call = crate::ToolCallView {
            id: "call-1",
            name: "deploy",
            args: &arguments,
        };
        let (original, rebuilt) = (Arc::new(1_u8), Arc::new(2_u8));
        let declared = "Deploy a build";
        let fast = ToolExecutionMode::Fast;
        let retained = fenced_plan(
            &original,
            call,
            declared,
            fast,
            mcp_target("ci", 1, "alice"),
        );

        for (kept, fresh) in [
            (
                "another allocation",
                fenced_plan(&rebuilt, call, declared, fast, mcp_target("ci", 1, "alice")),
            ),
            (
                "a new connection generation",
                fenced_plan(&rebuilt, call, declared, fast, mcp_target("ci", 2, "alice")),
            ),
        ] {
            assert!(retained.same_durable_execution(&fresh), "{kept}");
        }
        for (differs, fresh) in [
            (
                "tool use id",
                fenced_plan(
                    &rebuilt,
                    crate::ToolCallView {
                        id: "call-2",
                        ..call
                    },
                    declared,
                    fast,
                    mcp_target("ci", 1, "alice"),
                ),
            ),
            (
                "arguments",
                fenced_plan(
                    &rebuilt,
                    crate::ToolCallView {
                        args: &other_arguments,
                        ..call
                    },
                    declared,
                    fast,
                    mcp_target("ci", 1, "alice"),
                ),
            ),
            (
                "declaration",
                fenced_plan(
                    &rebuilt,
                    call,
                    "Deploy anything",
                    fast,
                    mcp_target("ci", 1, "alice"),
                ),
            ),
            (
                "execution mode",
                fenced_plan(
                    &rebuilt,
                    call,
                    declared,
                    ToolExecutionMode::Detached,
                    mcp_target("ci", 1, "alice"),
                ),
            ),
            (
                "account",
                fenced_plan(&rebuilt, call, declared, fast, mcp_target("ci", 1, "bob")),
            ),
            (
                "registration",
                fenced_plan(&rebuilt, call, declared, fast, mcp_target("cd", 1, "alice")),
            ),
        ] {
            assert!(
                !retained.same_durable_execution(&fresh),
                "a different {differs}"
            );
        }
        assert_eq!(
            retained.validate_root_dispatch(&rebuilt, call),
            Err(ToolExecutionResolutionError::RootDispatcherChanged {
                tool_name: "deploy".to_string(),
            }),
            "the old plan never enters through the new allocation"
        );
        assert_eq!(retained.validate_root_dispatch(&original, call), Ok(()));
    }

    #[test]
    fn tool_binding_digest_text_round_trips_and_rejects_other_text() {
        let digest =
            ToolBindingDigest::of_catalog_entry(&deploy_entry(deploy_tool(), deploy_contract()));
        let text = digest.to_string();
        assert_eq!(text.parse::<ToolBindingDigest>(), Ok(digest));
        assert_eq!(
            serde_json::from_value::<ToolBindingDigest>(
                serde_json::to_value(digest).expect("serializes")
            )
            .expect("deserializes"),
            digest
        );
        let hex = &text["sha256:".len()..];
        for other in [
            format!("sha256:{}", hex.to_uppercase()),
            format!("sha512:{hex}"),
            format!("sha256:{}", &hex[..hex.len() - 2]),
            format!("{text}00"),
            format!("sha256:{}", "g".repeat(64)),
            hex.to_string(),
            String::new(),
        ] {
            assert_eq!(
                other.parse::<ToolBindingDigest>(),
                Err(ToolBindingDigestParseError),
                "{other}"
            );
        }
    }
}
