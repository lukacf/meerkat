//! Realm-scoped connection contracts: backend profiles, auth profiles,
//! provider bindings, and the ingestion wrapper `RealmConfigSection`.
//!
//! This module owns the cross-cutting runtime shapes used by sessions,
//! factories, and surfaces. Provider-runtime-side typed enums
//! (`OpenAiBackendKind`, `AnthropicAuthMethod`, etc.) live in
//! [`crate::provider_matrix`]. Runtime config still carries `backend_kind` /
//! `auth_method` as strings until they are normalized at the provider-runtime
//! catalog boundary.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Config;
use crate::auth::{AuthConstraints, AuthMetadataDefaults};
use crate::provider::Provider;
use crate::provider_matrix::{
    AnthropicAuthMethod, AnthropicBackendKind, GoogleAuthMethod, GoogleBackendKind,
    OpenAiAuthMethod, OpenAiBackendKind, SelfHostedAuthMethod, SelfHostedBackendKind,
};

const AZURE_OPENAI_API_KEY_ENV: &str = "AZURE_OPENAI_API_KEY";
const AZURE_OPENAI_ENDPOINT_ENV: &str = "AZURE_OPENAI_ENDPOINT";
const AZURE_OPENAI_IMAGE_GENERATION_DEPLOYMENT_ENV: &str =
    "AZURE_OPENAI_IMAGE_GENERATION_DEPLOYMENT";
const AZURE_OPENAI_IMAGE_DEPLOYMENT_ENV: &str = "AZURE_OPENAI_IMAGE_DEPLOYMENT";
const AZURE_OPENAI_IMAGE_GENERATION_API_VERSION_ENV: &str =
    "AZURE_OPENAI_IMAGE_GENERATION_API_VERSION";

#[derive(Debug, Clone, PartialEq, Eq)]
struct EnvDefaultSpec {
    backend_kind: &'static str,
    auth_method: &'static str,
    env_var: &'static str,
    fallback: Vec<String>,
    base_url: Option<String>,
    options: serde_json::Value,
}

// ---------------------------------------------------------------------
// Runtime shapes (what providers/surfaces consume at runtime)
// ---------------------------------------------------------------------

/// Error returned when a realm/binding/profile slug fails validation.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum IdentityError {
    #[error("identity slug is empty")]
    Empty,
    #[error(
        "identity slug contains invalid character {0:?}; must be ASCII alphanumeric or one of '-', '_', '.'"
    )]
    InvalidChar(char),
}

/// `skip_serializing_if` helper: keeps `bool` fields off the wire when false,
/// matching the crate convention (`tool_catalog::is_false`).
fn is_false(value: &bool) -> bool {
    !*value
}

fn validate_slug(raw: &str) -> Result<(), IdentityError> {
    if raw.is_empty() {
        return Err(IdentityError::Empty);
    }
    for ch in raw.chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.') {
            return Err(IdentityError::InvalidChar(ch));
        }
    }
    Ok(())
}

macro_rules! slug_newtype {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn parse(raw: impl Into<String>) -> Result<Self, IdentityError> {
                let raw = raw.into();
                validate_slug(&raw)?;
                Ok(Self(raw))
            }

            /// Construct from a compile-time-known-valid slug literal used
            /// internally by synthesis helpers (e.g. the `"env_default"` /
            /// `"default"` synthetic-fallback slugs). A `debug_assert`
            /// validates the slug in debug builds; release builds skip the
            /// check since the only callers pass static, already-valid slugs.
            // Generated for every slug newtype; only some (e.g. RealmId) have a
            // synthesis caller.
            #[allow(dead_code)]
            pub(crate) fn from_known_valid(raw: &'static str) -> Self {
                debug_assert!(
                    validate_slug(raw).is_ok(),
                    "from_known_valid called with invalid slug literal: {raw:?}",
                );
                Self(raw.to_string())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityError;
            fn try_from(s: String) -> Result<Self, Self::Error> {
                Self::parse(s)
            }
        }

        impl From<$name> for String {
            fn from(v: $name) -> String {
                v.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

slug_newtype!(RealmId, "Opaque slug identifying a realm.");
slug_newtype!(
    BindingId,
    "Opaque slug identifying a binding inside a realm."
);
slug_newtype!(
    ProfileId,
    "Opaque slug identifying an auth profile override on a connection."
);

/// The single owner of the synthetic env-var-default realm slug. The literal
/// lives here once; `synthesize_env_default` mints it and [`RealmId::is_env_default`]
/// recognizes it, so no other site recovers the "this is the env-var default
/// realm" fact by comparing a raw `"env_default"` string.
pub const ENV_DEFAULT_REALM_SLUG: &str = "env_default";

/// The single owner of the reserved global-realm slug. The `global` realm is
/// the durable root of the default inheritance chain: a realm with no explicit
/// `parent` edge that is not itself `global` implicitly parents to it. Unlike
/// [`ENV_DEFAULT_REALM_SLUG`] (the ephemeral synthetic env-var fallback),
/// `global` is a normal `Configured` realm that may hold persisted credentials
/// and publish durable leases. Recognized via [`RealmId::is_global`], never by
/// raw string comparison elsewhere.
pub const GLOBAL_REALM_SLUG: &str = "global";

/// Hard cap on realm parent-chain length. A finite config is already bounded by
/// the `seen` dedup set; this is a belt-and-suspenders guard that bounds work
/// and stack independently of config size, and yields a typed error instead of
/// looping. 16 is far beyond any plausible org→team→user→global nesting.
pub const MAX_REALM_CHAIN_DEPTH: usize = 16;

impl RealmId {
    /// True when this realm is the synthetic env-var-default realm (the realm
    /// [`RealmConnectionSet::synthesize_env_default`] mints). Routing/selection
    /// decisions consult this typed predicate instead of a `== "env_default"`
    /// slug comparison.
    #[must_use]
    pub fn is_env_default(&self) -> bool {
        self.as_str() == ENV_DEFAULT_REALM_SLUG
    }

    /// True when this realm is the reserved `global` root of the inheritance
    /// chain. Consulted via this typed predicate, never by a raw
    /// `== "global"` comparison.
    #[must_use]
    pub fn is_global(&self) -> bool {
        self.as_str() == GLOBAL_REALM_SLUG
    }

    /// Mint the reserved `global` [`RealmId`]. Infallible: the slug is a
    /// compile-time-valid constant.
    #[must_use]
    pub fn global() -> RealmId {
        RealmId::from_known_valid(GLOBAL_REALM_SLUG)
    }
}

/// Origin discriminant for an [`AuthBindingRef`].
///
/// Distinguishes a binding that names a durable, config-resolvable identity
/// (`Configured`) from the synthetic env-var fallback the resolver mints when
/// no realm config exists but a well-known API-key env var is set
/// (`SyntheticEnvDefault`). The synthetic origin is ephemeral: it must never be
/// rehydrated as a durable identity nor publish a durable auth lease.
///
/// This is the typed owner of the "is this the env-var default?" fact, replacing
/// the prior recovery-by-magic-slug (`realm == "env_default"`,
/// `binding == "default"`). Identity slugs (`RealmId`/`BindingId`) are pure
/// opaque identity again; origin is carried explicitly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum BindingOrigin {
    /// A durable, config-resolvable binding. This is the back-read default so
    /// that wire/persisted rows written before this field existed deserialize
    /// as a configured identity.
    #[default]
    Configured,
    /// The synthetic env-var fallback binding (ephemeral, not durable).
    SyntheticEnvDefault,
}

/// Session-facing reference to a binding inside a realm.
///
/// `AuthBindingRef` is purely structural — it does NOT carry a `"realm:binding"`
/// string form. Wave-b deleted `parse` and `Display` so that no code path
/// accidentally ferries the opaque join through the runtime. CLI input that
/// arrives as `"realm:binding[:profile]"` must be split at the CLI boundary
/// and constructed field-by-field.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthBindingRef {
    pub realm: RealmId,
    pub binding: BindingId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileId>,
    /// Whether this ref names a configured durable identity or the synthetic
    /// env-var fallback. Defaults to [`BindingOrigin::Configured`] for back-read
    /// of rows persisted before the discriminant existed.
    #[serde(default, skip_serializing_if = "BindingOrigin::is_configured")]
    pub origin: BindingOrigin,
}

impl BindingOrigin {
    /// True when this is the (default) configured origin. Used to keep the
    /// serialized shape wire-additive via `skip_serializing_if`.
    pub fn is_configured(&self) -> bool {
        matches!(self, BindingOrigin::Configured)
    }
}

impl AuthBindingRef {
    /// True when this ref is the synthetic env-var fallback binding rather than
    /// a durable configured identity. Reads the typed [`BindingOrigin`]
    /// discriminant; no slug-string comparison.
    pub fn is_env_default(&self) -> bool {
        matches!(self.origin, BindingOrigin::SyntheticEnvDefault)
    }
}

/// The realm identity a mob member binds to.
///
/// This is the single fail-closed owner of the `mob.{mob_id}` realm form.
/// Both the producer (mob build) and the consumer (mob-mcp ownership routing)
/// derive their realm string through this helper, so the dot/colon divergence
/// that previously made `persisted_mob_binding` never match a real session is
/// impossible: there is one form, validated once.
pub fn mob_realm_id(mob_id: &str) -> Result<RealmId, IdentityError> {
    RealmId::parse(format!("mob.{mob_id}"))
}

/// Error returned when a [`MemberCommsName`] fails to parse.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MemberCommsNameError {
    /// The name did not have exactly three `/`-separated components.
    #[error(
        "mob member comms name must have exactly three '/'-separated components (mob_id/role/member)"
    )]
    WrongComponentCount,
    /// A component was empty or contained characters outside the identifier-safe set.
    #[error(
        "mob member comms name component {component:?} is invalid; \
         each must start with an ASCII letter or '_' and contain only ASCII alphanumerics, '-', or '_'"
    )]
    InvalidComponent { component: String },
}

/// Validate one component of a [`MemberCommsName`].
///
/// Folds the former `is_valid_peer_name_component` rule (first char ASCII
/// alphabetic or `_`; remaining chars ASCII alphanumeric / `-` / `_`). This is
/// strictly tighter than [`validate_slug`], so any valid component is also a
/// valid realm slug — which is why `mob.{component}` always parses.
fn validate_member_comms_name_component(component: &str) -> Result<(), MemberCommsNameError> {
    let mut chars = component.chars();
    let Some(first) = chars.next() else {
        return Err(MemberCommsNameError::InvalidComponent {
            component: component.to_string(),
        });
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return Err(MemberCommsNameError::InvalidComponent {
            component: component.to_string(),
        });
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(MemberCommsNameError::InvalidComponent {
            component: component.to_string(),
        });
    }
    Ok(())
}

/// Typed mob-member comms (peer) name: `mob_id/role/member`.
///
/// This is the single owner of the `{mob_id}/{role}/{member}` join, with one
/// [`Display`](std::fmt::Display) (render) and one fail-closed
/// [`FromStr`](std::str::FromStr) (parse, exactly three identifier-safe
/// components). It replaces the scattered `format!("{}/{}/{}", ..)` producers
/// and the hand-rolled `split('/')` consumers, so the routing-name shape is no
/// longer recovered by string convention.
///
/// Identity and transport are separate facts: this is the transport routing
/// *name* (a [`crate::comms::PeerName`]). Durable identity ownership lives in
/// [`MobMemberBinding`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MemberCommsName {
    mob_id: String,
    role: String,
    member: String,
}

impl MemberCommsName {
    /// Construct from already-typed components, validating each.
    pub fn new(
        mob_id: impl Into<String>,
        role: impl Into<String>,
        member: impl Into<String>,
    ) -> Result<Self, MemberCommsNameError> {
        let mob_id = mob_id.into();
        let role = role.into();
        let member = member.into();
        validate_member_comms_name_component(&mob_id)?;
        validate_member_comms_name_component(&role)?;
        validate_member_comms_name_component(&member)?;
        Ok(Self {
            mob_id,
            role,
            member,
        })
    }

    pub fn mob_id(&self) -> &str {
        &self.mob_id
    }

    pub fn role(&self) -> &str {
        &self.role
    }

    pub fn member(&self) -> &str {
        &self.member
    }

    /// The durable identity binding implied by this comms name.
    pub fn to_member_binding(&self) -> MobMemberBinding {
        MobMemberBinding {
            mob_id: self.mob_id.clone(),
            role: self.role.clone(),
            member: self.member.clone(),
        }
    }
}

impl std::fmt::Display for MemberCommsName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}/{}", self.mob_id, self.role, self.member)
    }
}

impl std::str::FromStr for MemberCommsName {
    type Err = MemberCommsNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('/');
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(mob_id), Some(role), Some(member), None) => Self::new(mob_id, role, member),
            _ => Err(MemberCommsNameError::WrongComponentCount),
        }
    }
}

/// Typed role of a peer relative to a mob.
///
/// Replaces the magic `"external"` string the synthetic peer-added fallback
/// previously invented when a peer name failed to parse as a member comms
/// name. A peer is either a `Member` of a mob (carrying its parsed role) or
/// `External`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerRole {
    /// A peer that is a member of a mob, with its parsed role component.
    Member(String),
    /// A peer that is not a recognized mob member.
    External,
}

impl PeerRole {
    /// The wire/display label for this role.
    pub fn as_label(&self) -> &str {
        match self {
            PeerRole::Member(role) => role.as_str(),
            PeerRole::External => "external",
        }
    }
}

/// Durable, typed identity of a mob member, carried on
/// [`SessionMetadata`](crate::session::SessionMetadata).
///
/// This is the canonical owner of the `(mob_id, role, member)` identity fact
/// that ownership routing (`owns_persisted_bridge_session`) and outbound
/// peer-added payloads previously recovered by splitting the untyped
/// `comms_name` string and re-deriving the realm by format convention.
///
/// `comms_name`/`realm_id`/`peer_meta` remain on the metadata as the transport
/// routing name and discovery metadata — identity and transport are separate
/// facts. Old persisted rows written before this field existed deserialize as
/// `None` (the field is `#[serde(default, skip_serializing_if)]` on the
/// metadata), so back-read is safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct MobMemberBinding {
    pub mob_id: String,
    pub role: String,
    pub member: String,
}

impl MobMemberBinding {
    /// The transport comms name implied by this binding.
    pub fn comms_name(&self) -> Result<MemberCommsName, MemberCommsNameError> {
        MemberCommsName::new(self.mob_id.clone(), self.role.clone(), self.member.clone())
    }
}

/// Backend profile: where requests go and which backend contract applies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackendProfile {
    pub id: String,
    pub provider: Provider,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

/// Auth profile: how credentials are obtained, refreshed, constrained.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthProfile {
    pub id: String,
    pub provider: Provider,
    pub auth_method: String,
    pub source: CredentialSourceSpec,
    #[serde(default)]
    pub constraints: AuthConstraints,
    #[serde(default)]
    pub metadata_defaults: AuthMetadataDefaults,
}

/// Typed identity of an externally-registered auth resolver.
///
/// Resolver handles are free-form names chosen by the host at registration, so
/// this is carried as a string on the wire; the newtype keeps it a distinct
/// typed identity in memory (and as the resolver-registry map key) so it cannot
/// be confused with any other arbitrary string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct ExternalResolverId(String);

impl ExternalResolverId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ExternalResolverId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ExternalResolverId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for ExternalResolverId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

/// Where credentials come from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialSourceSpec {
    InlineSecret {
        secret: String,
    },
    /// Binding-scoped credential material stored in the configured
    /// [`TokenStore`](crate::auth::TokenStore). The storage key is the
    /// resolved typed binding identity (`realm`, `binding`), not a
    /// second free-form profile string.
    ManagedStore,
    Env {
        env: String,
        /// Ordered fallback env var names consulted when `env` is
        /// unset. Used for providers with multiple well-known names
        /// (e.g. Gemini falls back to `GOOGLE_API_KEY` when
        /// `GEMINI_API_KEY` is absent). The resolver's RKAT_*-prefix
        /// precedence applies to each name in turn.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        fallback: Vec<String>,
    },
    ExternalResolver {
        handle: ExternalResolverId,
    },
    PlatformDefault,
    /// External command that prints a bearer token on stdout. Reference:
    /// Codex `external_bearer.rs:17-157`. The runner lives in
    /// `meerkat-client/src/auth_store/command.rs`.
    Command {
        program: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Timeout for the subprocess in milliseconds.
        #[serde(default = "default_command_timeout_ms")]
        timeout_ms: u64,
        /// Optional cached-token lifetime. `None` disables caching.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        refresh_interval_ms: Option<u64>,
    },
    /// Read credentials from an inherited file descriptor (Claude Code
    /// pattern for sandboxed host-injected tokens).
    FileDescriptor {
        fd: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope_override: Option<String>,
    },
}

impl CredentialSourceSpec {
    pub const ALL_KIND_LABELS: &'static [&'static str] = &[
        "inline_secret",
        "managed_store",
        "env",
        "external_resolver",
        "platform_default",
        "command",
        "file_descriptor",
    ];

    pub const fn kind_label(&self) -> &'static str {
        match self {
            Self::InlineSecret { .. } => "inline_secret",
            Self::ManagedStore => "managed_store",
            Self::Env { .. } => "env",
            Self::ExternalResolver { .. } => "external_resolver",
            Self::PlatformDefault => "platform_default",
            Self::Command { .. } => "command",
            Self::FileDescriptor { .. } => "file_descriptor",
        }
    }
}

fn default_command_timeout_ms() -> u64 {
    30_000
}

/// Policy overrides carried on a binding.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BindingPolicy {
    #[serde(default)]
    pub allow_auth_override: bool,
    #[serde(default)]
    pub require_metadata_account: bool,
    #[serde(default)]
    pub require_metadata_workspace: bool,
}

/// A binding is what sessions actually refer to: one backend + one auth
/// profile, plus policy and an optional default model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderBinding {
    pub id: String,
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub policy: BindingPolicy,
    /// Typed per-binding marker: this binding is the default for its provider.
    /// Owns the "default for provider X" fact that was previously carried only
    /// by the `default_<provider>` name convention. The realm-level
    /// [`RealmConnectionSet::default_binding`] expresses a single per-realm
    /// default; this flag expresses the per-provider default.
    #[serde(default, skip_serializing_if = "is_false")]
    pub provider_default: bool,
}

/// Realm-scoped set of backends, auth profiles, and bindings.
///
/// Produced by [`RealmConnectionSet::from_config`] from a
/// [`RealmConfigSection`] ingested from TOML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmConnectionSet {
    pub realm_id: RealmId,
    pub backends: BTreeMap<String, BackendProfile>,
    pub auth_profiles: BTreeMap<String, AuthProfile>,
    pub bindings: BTreeMap<String, ProviderBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
}

/// Fully resolved connection target selected from config-owned identity
/// policy. Surfaces should use this instead of inventing realm or binding
/// defaults locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConnectionTarget {
    pub realm: RealmConnectionSet,
    pub auth_binding: AuthBindingRef,
    pub binding: ProviderBinding,
    pub backend: BackendProfile,
    pub auth_profile: AuthProfile,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConnectionTargetError {
    #[error("connection target did not name a realm and no configured default realm was available")]
    MissingRealm,
    #[error("realm '{0}' not found in config.realm")]
    UnknownRealm(String),
    #[error("realm '{realm}' has no default binding")]
    MissingDefaultBinding { realm: String },
    #[error("invalid realm id '{realm}': {source}")]
    InvalidRealmId {
        realm: String,
        source: IdentityError,
    },
    #[error("invalid binding id '{binding}': {source}")]
    InvalidBindingId {
        binding: String,
        source: IdentityError,
    },
    #[error("realm '{realm}' config invalid: {source}")]
    RealmConfigInvalid {
        realm: String,
        source: ProviderBindingError,
    },
    #[error("binding '{realm}:{binding}' is invalid: {source}")]
    BindingInvalid {
        realm: String,
        binding: String,
        source: ProviderBindingError,
    },
    #[error(
        "binding '{realm}:{binding}' resolves backend={backend:?} auth={auth:?}, expected provider {expected:?}"
    )]
    ProviderMismatch {
        realm: String,
        binding: String,
        expected: Provider,
        backend: Provider,
        auth: Provider,
    },
    #[error(transparent)]
    RealmChain(#[from] RealmChainError),
}

/// Fail-closed errors from resolving a realm parent chain.
///
/// The chain walk is fully typed and panic-free: every malformed topology
/// yields one of these variants rather than looping, unwrapping, or silently
/// truncating. Internal to resolution (not a wire type).
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum RealmChainError {
    /// A `parent` edge re-enters an already-visited realm (includes a realm
    /// naming itself as parent). The captured path is for diagnostics.
    #[error("realm parent chain has a cycle: {}", .chain.join(" -> "))]
    Cycle { chain: Vec<String> },
    /// The chain exceeded [`MAX_REALM_CHAIN_DEPTH`].
    #[error("realm parent chain from '{head}' exceeds max depth {max}")]
    DepthExceeded { head: String, max: usize },
    /// A `parent` edge names a realm absent from config (and it is not the
    /// reserved `global` root, which is allowed to be implicit).
    #[error("realm '{realm}' names parent '{parent}' which is not configured")]
    MissingParent { realm: String, parent: String },
    /// The reserved `global` realm declares a `parent`; it must be the root.
    #[error("the reserved 'global' realm (via '{realm}') must not declare a parent")]
    GlobalHasParent { realm: String },
    /// A `parent` edge targets the synthetic env-var-default slug, which may
    /// never be a chain node.
    #[error("realm '{realm}' names the reserved env_default slug as its parent")]
    ParentIsEnvDefault { realm: String },
}

/// An ordered realm inheritance chain, most-derived first.
///
/// `realms()[0]` is the head (consuming) realm; the last element is the
/// reserved `global` root when one participates. Built only via the fallible
/// [`RealmChain::resolve`]; the field is private so the only way to obtain a
/// chain is through the validated walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmChain {
    realms: Vec<RealmId>,
}

impl RealmChain {
    /// Resolve the parent chain for `head`, walking `parent` edges to the root.
    ///
    /// Ordering is `[head, parent, .., global?]` — fully determined by the
    /// linear `parent` edges, with zero dependence on map iteration order. A
    /// realm with no explicit `parent` that is not itself `global` implicitly
    /// appends `global` IFF `global` is configured and not already visited.
    ///
    /// Absent head: if `head` is not in `config.realm`, the chain is `[head]`
    /// alone (it contributes no section) plus the implicit-global tail when
    /// applicable — it is NOT a hard error. Callers that require an explicit
    /// realm to exist enforce that separately (see the explicit-ref path in
    /// the connection resolvers). Fails closed on cycle, depth, missing
    /// parent, a parent pointing at `global`-with-a-parent, or a parent that
    /// is the env_default slug. Iterative (no recursion) — wasm stack-safe.
    pub fn resolve(config: &Config, head: &RealmId) -> Result<RealmChain, RealmChainError> {
        let mut ordered: Vec<RealmId> = Vec::new();
        let mut seen: BTreeSet<RealmId> = BTreeSet::new();

        // Seed with the head so a realm naming itself as parent is a 1-cycle.
        ordered.push(head.clone());
        seen.insert(head.clone());

        let mut current = head.clone();
        loop {
            if ordered.len() > MAX_REALM_CHAIN_DEPTH {
                return Err(RealmChainError::DepthExceeded {
                    head: head.as_str().to_string(),
                    max: MAX_REALM_CHAIN_DEPTH,
                });
            }

            // A `global` node must be the root: it may not declare a parent.
            let current_section = config.realm.get(current.as_str());
            if current.is_global() && current_section.and_then(|s| s.parent.as_ref()).is_some() {
                return Err(RealmChainError::GlobalHasParent {
                    realm: current.as_str().to_string(),
                });
            }

            let Some(parent) = current_section.and_then(|s| s.parent.clone()) else {
                // No explicit parent: terminate. Append the implicit `global`
                // tail when the current terminal is not already `global`, the
                // global realm is configured, and it has not been visited.
                if !current.is_global() && config.realm.contains_key(GLOBAL_REALM_SLUG) {
                    let global = RealmId::global();
                    if seen.insert(global.clone()) {
                        ordered.push(global);
                    }
                }
                return Ok(RealmChain { realms: ordered });
            };

            if parent.is_env_default() {
                return Err(RealmChainError::ParentIsEnvDefault {
                    realm: current.as_str().to_string(),
                });
            }
            // A parent edge must resolve to a configured realm, except the
            // reserved `global` root which is allowed to be implicit/absent.
            if !parent.is_global() && !config.realm.contains_key(parent.as_str()) {
                return Err(RealmChainError::MissingParent {
                    realm: current.as_str().to_string(),
                    parent: parent.as_str().to_string(),
                });
            }
            if !seen.insert(parent.clone()) {
                let mut chain: Vec<String> =
                    ordered.iter().map(|r| r.as_str().to_string()).collect();
                chain.push(parent.as_str().to_string());
                return Err(RealmChainError::Cycle { chain });
            }
            ordered.push(parent.clone());
            current = parent;
        }
    }

    /// The resolved chain, most-derived (head) first, root (`global`) last.
    #[must_use]
    pub fn realms(&self) -> &[RealmId] {
        &self.realms
    }
}

/// Synthesize the typed env-var-default target for `provider`.
///
/// The single owner of the synthetic fallback materialization, shared by the
/// single-target and candidate resolvers so the ephemeral
/// [`BindingOrigin::SyntheticEnvDefault`] identity is minted in exactly one
/// place.
fn env_default_target(
    provider: Provider,
    profile: Option<ProfileId>,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    let realm = RealmConnectionSet::synthesize_env_default(provider);
    let binding =
        BindingId::parse("default").map_err(|source| ConnectionTargetError::InvalidBindingId {
            binding: "default".to_string(),
            source,
        })?;
    materialize_connection_target(
        realm,
        Some(provider),
        binding,
        profile,
        BindingOrigin::SyntheticEnvDefault,
    )
}

/// Walk `head`'s realm parent chain and collect, in chain order, one
/// owner-stamped provider candidate per chain member that defines a usable
/// provider binding (via the unified [`selected_binding_id_for_provider`]
/// policy). This is the single owner of the default-selection cross-realm
/// candidate order — it replaces both the deleted flat scan over
/// `config.realm.keys()` and the deleted literal `"default"` realm candidate.
///
/// Provenance (decision A): each candidate is materialized from the OWNING
/// chain member's OWN [`RealmConnectionSet`], so `AuthBindingRef.realm` is the
/// realm that DEFINES the binding, and `materialize_connection_target` stamps
/// `realm.realm_id` for an owner whose `realm_id == owner` by construction —
/// the strict registry equality stays a real invariant with no relaxation.
///
/// Isolation (per-member fail-closed, MF-03): an ancestor member with an
/// absent or structurally-invalid section is SKIPPED (it cannot take down
/// resolution for valid descendants). The head isolates the same way unless
/// `head_required`, in which case an absent head is `UnknownRealm` and an
/// invalid head is `RealmConfigInvalid`.
fn collect_provider_candidates_on_chain(
    config: &Config,
    provider: Provider,
    head: &RealmId,
    head_required: bool,
) -> Result<Vec<ResolvedConnectionTarget>, ConnectionTargetError> {
    let chain = RealmChain::resolve(config, head)?;
    let mut out = Vec::new();
    for (idx, member) in chain.realms().iter().enumerate() {
        let is_head = idx == 0;
        let Some(section) = config.realm.get(member.as_str()) else {
            if is_head && head_required {
                return Err(ConnectionTargetError::UnknownRealm(
                    member.as_str().to_string(),
                ));
            }
            continue;
        };
        let realm = match RealmConnectionSet::from_config(member.as_str(), section) {
            Ok(realm) => realm,
            Err(source) => {
                if is_head && head_required {
                    return Err(ConnectionTargetError::RealmConfigInvalid {
                        realm: member.as_str().to_string(),
                        source,
                    });
                }
                continue;
            }
        };
        let binding_id = match selected_binding_id_for_provider(&realm, provider) {
            Ok(binding_id) => binding_id,
            Err(err) => {
                if is_head && head_required {
                    return Err(err);
                }
                continue;
            }
        };
        if let Some(binding_id) = binding_id {
            out.push(materialize_connection_target(
                realm,
                Some(provider),
                binding_id,
                None,
                BindingOrigin::Configured,
            )?);
        }
    }
    Ok(out)
}

/// Resolve an EXPLICIT binding id along `head`'s chain, returning the
/// owner-stamped target for the first chain member (child-first) that defines
/// it. A binding may be inherited: the head names the consuming realm, but the
/// owner stamped is the chain member that actually declares the binding.
///
/// Head validity: an absent head is `UnknownRealm` and an invalid head is
/// `RealmConfigInvalid` when `head_required`; ancestors isolate. If no chain
/// member declares the binding, fail closed with `BindingInvalid`/UnknownBinding
/// attributed to the head realm. A provider mismatch on the explicitly named
/// binding propagates as `ProviderMismatch` (explicit requests are strict).
fn resolve_explicit_binding_on_chain(
    config: &Config,
    expected_provider: Option<Provider>,
    head: &RealmId,
    binding: &BindingId,
    profile: Option<&ProfileId>,
    head_required: bool,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    let chain = RealmChain::resolve(config, head)?;
    for (idx, member) in chain.realms().iter().enumerate() {
        let is_head = idx == 0;
        let Some(section) = config.realm.get(member.as_str()) else {
            if is_head && head_required {
                return Err(ConnectionTargetError::UnknownRealm(
                    member.as_str().to_string(),
                ));
            }
            continue;
        };
        let realm = match RealmConnectionSet::from_config(member.as_str(), section) {
            Ok(realm) => realm,
            Err(source) => {
                if is_head && head_required {
                    return Err(ConnectionTargetError::RealmConfigInvalid {
                        realm: member.as_str().to_string(),
                        source,
                    });
                }
                continue;
            }
        };
        if realm.bindings.contains_key(binding.as_str()) {
            return materialize_connection_target(
                realm,
                expected_provider,
                binding.clone(),
                profile.cloned(),
                BindingOrigin::Configured,
            );
        }
    }
    Err(ConnectionTargetError::BindingInvalid {
        realm: head.as_str().to_string(),
        binding: binding.as_str().to_string(),
        source: ProviderBindingError::UnknownBinding(binding.as_str().to_string()),
    })
}

/// Resolve an explicitly named auth binding and infer its provider from the
/// effective realm configuration.
///
/// This is the provider-neutral companion to
/// [`resolve_auth_binding_or_default_for_provider`]. It exists for ingress
/// seams where a caller supplied `auth_binding` but omitted `provider`: the
/// binding's configured backend/auth pair is the typed provider authority.
/// Resolution walks the named realm's inheritance chain and returns the
/// owner-stamped binding; callers must not reimplement that walk or infer a
/// provider from binding names.
pub fn resolve_explicit_auth_binding_target(
    config: &Config,
    auth_binding: &AuthBindingRef,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    if auth_binding.is_env_default() {
        return Err(ConnectionTargetError::UnknownRealm(
            auth_binding.realm.as_str().to_string(),
        ));
    }
    resolve_explicit_binding_on_chain(
        config,
        None,
        &auth_binding.realm,
        &auth_binding.binding,
        auth_binding.profile.as_ref(),
        /* head_required = */ true,
    )
}

/// Resolve a connection target from config-owned identity facts.
///
/// `explicit_realm` / `explicit_binding` are request atoms, not defaults.
/// When either is absent, selection falls back to the preferred realm and
/// that realm's `default_binding`, then to the configured `default` realm.
/// Provider-shaped binding names and hard-coded realm names must not be
/// encoded by REST/RPC/SDK surfaces.
pub fn resolve_realm_binding_target_for_provider(
    config: &Config,
    provider: Provider,
    explicit_realm: Option<&RealmId>,
    explicit_binding: Option<&BindingId>,
    explicit_profile: Option<&ProfileId>,
    preferred_realm: Option<&RealmId>,
    allow_env_default: bool,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    // Head of the chain: an explicit realm names it (and must exist); else the
    // preferred realm; else the reserved `global` root. Resolution walks
    // head -> parents -> global; an unrelated sibling realm is NOT a candidate
    // (the flat scan and the literal `default` realm are both gone — `global`
    // is the universal default head).
    let global = RealmId::global();
    let head = explicit_realm.or(preferred_realm).unwrap_or(&global);
    let head_required = explicit_realm.is_some();

    if let Some(binding) = explicit_binding {
        return resolve_explicit_binding_on_chain(
            config,
            Some(provider),
            head,
            binding,
            explicit_profile,
            head_required,
        );
    }
    let candidates = collect_provider_candidates_on_chain(config, provider, head, head_required)?;
    if let Some(first) = candidates.into_iter().next() {
        return Ok(first);
    }
    if head_required {
        return Err(ConnectionTargetError::MissingDefaultBinding {
            realm: head.as_str().to_string(),
        });
    }

    if allow_env_default && explicit_realm.is_none() && explicit_binding.is_none() {
        return env_default_target(provider, explicit_profile.cloned());
    }

    Err(ConnectionTargetError::MissingRealm)
}

/// Resolve an explicit [`AuthBindingRef`] or the configured default target for
/// the selected provider. This is the shared factory/runtime path for
/// auth-binding-less provider resolution.
pub fn resolve_auth_binding_or_default_for_provider(
    config: &Config,
    provider: Provider,
    auth_binding: Option<&AuthBindingRef>,
    preferred_realm: Option<&RealmId>,
    allow_env_default: bool,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    if let Some(auth_binding) = auth_binding {
        // The synthetic env-var fallback is never a durable, config-resolvable
        // identity: reject it BEFORE any chain walk (guards the reorder).
        if auth_binding.is_env_default() {
            return Err(ConnectionTargetError::UnknownRealm(
                auth_binding.realm.as_str().to_string(),
            ));
        }
        // Walk the named realm's chain so an explicitly-referenced binding that
        // is defined only in a parent/global resolves at its OWNING realm.
        return resolve_explicit_binding_on_chain(
            config,
            Some(provider),
            &auth_binding.realm,
            &auth_binding.binding,
            auth_binding.profile.as_ref(),
            /* head_required = */ true,
        );
    }

    resolve_realm_binding_target_for_provider(
        config,
        provider,
        None,
        None,
        None,
        preferred_realm,
        allow_env_default,
    )
}

fn selected_binding_id_for_provider(
    realm: &RealmConnectionSet,
    provider: Provider,
) -> Result<Option<BindingId>, ConnectionTargetError> {
    let mut provider_bindings = Vec::new();
    let mut provider_default_binding: Option<&str> = None;
    for (binding_id, binding) in &realm.bindings {
        let backend = realm
            .backends
            .get(&binding.backend_profile)
            .ok_or_else(|| ConnectionTargetError::BindingInvalid {
                realm: realm.realm_id.to_string(),
                binding: binding_id.clone(),
                source: ProviderBindingError::UnknownBackend(binding.backend_profile.clone()),
            })?;
        let auth = realm
            .auth_profiles
            .get(&binding.auth_profile)
            .ok_or_else(|| ConnectionTargetError::BindingInvalid {
                realm: realm.realm_id.to_string(),
                binding: binding_id.clone(),
                source: ProviderBindingError::UnknownAuth(binding.auth_profile.clone()),
            })?;
        if backend.provider == provider && auth.provider == provider {
            provider_bindings.push(binding_id.as_str());
            // Typed per-provider default marker replaces the
            // `default_<provider>` name convention. First marked wins
            // (BTreeMap iteration is deterministic by id).
            if binding.provider_default && provider_default_binding.is_none() {
                provider_default_binding = Some(binding_id.as_str());
            }
        }
    }

    if let Some(default_binding) = realm.default_binding.as_deref()
        && provider_bindings.contains(&default_binding)
    {
        return BindingId::parse(default_binding.to_string())
            .map(Some)
            .map_err(|source| ConnectionTargetError::InvalidBindingId {
                binding: default_binding.to_string(),
                source,
            });
    }

    if let Some(provider_default_binding) = provider_default_binding {
        return BindingId::parse(provider_default_binding.to_string())
            .map(Some)
            .map_err(|source| ConnectionTargetError::InvalidBindingId {
                binding: provider_default_binding.to_string(),
                source,
            });
    }

    match provider_bindings.as_slice() {
        [binding_id] => BindingId::parse((*binding_id).to_string())
            .map(Some)
            .map_err(|source| ConnectionTargetError::InvalidBindingId {
                binding: (*binding_id).to_string(),
                source,
            }),
        _ => Ok(None),
    }
}

/// Resolve ordered connection candidates for an omitted `auth_binding`.
///
/// The returned order is the shared "best available" policy used by all
/// factory-backed surfaces, now driven by the realm parent chain:
/// 1. provider binding in the preferred (head) realm
/// 2. provider binding in each ancestor realm, child-first
/// 3. provider binding in the reserved `global` root (implicit chain tail)
/// 4. synthetic env-var fallback when allowed
///
/// An unrelated sibling realm is NOT a candidate — the prior flat scan over
/// `config.realm.keys()` and the literal `"default"` realm candidate are both
/// removed; shared credentials belong in `[realm.global]` (inherited by every
/// realm via the implicit tail) or a named parent. Within a realm,
/// `default_binding` wins when it resolves to the requested provider, then the
/// typed `provider_default` marker, then a single unambiguous provider binding.
/// Explicit `auth_binding` still resolves to one strict (owner-stamped) target.
pub fn resolve_auth_binding_candidates_for_provider(
    config: &Config,
    provider: Provider,
    auth_binding: Option<&AuthBindingRef>,
    preferred_realm: Option<&RealmId>,
    allow_env_default: bool,
) -> Result<Vec<ResolvedConnectionTarget>, ConnectionTargetError> {
    if auth_binding.is_some() {
        return resolve_auth_binding_or_default_for_provider(
            config,
            provider,
            auth_binding,
            preferred_realm,
            allow_env_default,
        )
        .map(|target| vec![target]);
    }

    // Head defaults to the reserved `global` root when no realm is preferred,
    // so an unscoped lookup still resolves the universal default. Candidate
    // discovery never requires the head to exist (an unmaterialized session
    // realm still inherits its chain / global); ancestors isolate.
    let global = RealmId::global();
    let head = preferred_realm.unwrap_or(&global);
    let mut candidates = Vec::new();
    candidates.extend(collect_provider_candidates_on_chain(
        config, provider, head, false,
    )?);

    if allow_env_default {
        candidates.push(env_default_target(provider, None)?);
    }

    if candidates.is_empty() {
        return Err(ConnectionTargetError::MissingRealm);
    }
    Ok(candidates)
}

/// Outcome of classifying where a credential WRITE may land for `(head, binding)`.
///
/// Strict-owner write (decision 5): credential reads inherit down the chain, but
/// a write may target only the realm that DEFINES the binding in its own
/// section. This is the SINGLE owner of that policy — surfaces (REST/RPC/CLI)
/// call it and merely map the typed error to their transport status, rather
/// than each re-deriving "is this binding inherited?".
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum WriteOwnerError {
    /// The binding is inherited from an ancestor realm; the write must target
    /// the owning realm, not the consuming `head`.
    #[error(
        "binding '{binding}' is inherited by realm '{head}' from its owning realm '{owner}'; \
         credential reads inherit down the chain, but writes are strict-owner — target the \
         owning realm '{owner}', not '{head}'"
    )]
    Inherited {
        binding: String,
        head: String,
        owner: String,
    },
    /// No realm on `head`'s chain defines the binding.
    #[error("binding '{binding}' is not defined on realm '{head}' or any realm it inherits from")]
    Unknown { binding: String, head: String },
    /// The realm parent chain could not be resolved.
    #[error(transparent)]
    Chain(#[from] RealmChainError),
}

/// Classify the owning realm for a credential WRITE to `(head, binding)`.
///
/// Returns the head itself when it defines the binding in its OWN section
/// (write allowed). Returns [`WriteOwnerError::Inherited`] naming the owning
/// ancestor when the binding is only inherited (write rejected, strict-owner),
/// or [`WriteOwnerError::Unknown`] when no chain member defines it.
pub fn resolve_write_owner(
    config: &Config,
    head: &RealmId,
    binding: &BindingId,
) -> Result<RealmId, WriteOwnerError> {
    let defines = |realm: &RealmId| {
        config
            .realm
            .get(realm.as_str())
            .is_some_and(|section| section.binding.contains_key(binding.as_str()))
    };
    if defines(head) {
        return Ok(head.clone());
    }
    let chain = RealmChain::resolve(config, head)?;
    // Skip the head (already checked); the first ancestor that defines it owns it.
    if let Some(owner) = chain.realms().iter().skip(1).find(|member| defines(member)) {
        return Err(WriteOwnerError::Inherited {
            binding: binding.as_str().to_string(),
            head: head.as_str().to_string(),
            owner: owner.as_str().to_string(),
        });
    }
    Err(WriteOwnerError::Unknown {
        binding: binding.as_str().to_string(),
        head: head.as_str().to_string(),
    })
}

fn materialize_connection_target(
    realm: RealmConnectionSet,
    expected_provider: Option<Provider>,
    binding: BindingId,
    profile: Option<ProfileId>,
    origin: BindingOrigin,
) -> Result<ResolvedConnectionTarget, ConnectionTargetError> {
    // `realm.realm_id` is already a typed `RealmId` (parsed once at
    // `from_config`/synthesis); no re-parse needed.
    let auth_binding = AuthBindingRef {
        realm: realm.realm_id.clone(),
        binding,
        profile,
        origin,
    };
    let (binding, backend, auth_profile) =
        realm.lookup_auth_binding(&auth_binding).map_err(|source| {
            ConnectionTargetError::BindingInvalid {
                realm: auth_binding.realm.to_string(),
                binding: auth_binding.binding.to_string(),
                source,
            }
        })?;
    let provider = expected_provider.unwrap_or(backend.provider);
    if backend.provider != provider || auth_profile.provider != provider {
        return Err(ConnectionTargetError::ProviderMismatch {
            realm: auth_binding.realm.to_string(),
            binding: auth_binding.binding.to_string(),
            expected: provider,
            backend: backend.provider,
            auth: auth_profile.provider,
        });
    }
    let binding = binding.clone();
    let backend = backend.clone();
    let auth_profile = auth_profile.clone();
    Ok(ResolvedConnectionTarget {
        realm,
        auth_binding,
        binding,
        backend,
        auth_profile,
    })
}

impl RealmConnectionSet {
    /// Validate and materialize a realm connection set from its config
    /// section. Normalizes provider strings into the typed
    /// [`Provider`] enum and verifies that every binding references
    /// existing backend and auth profiles whose providers agree.
    pub fn from_config(
        realm_id: &str,
        section: &RealmConfigSection,
    ) -> Result<Self, ProviderBindingError> {
        let realm_id =
            RealmId::parse(realm_id).map_err(|source| ProviderBindingError::InvalidRealmId {
                realm: realm_id.to_string(),
                source,
            })?;
        let mut backends: BTreeMap<String, BackendProfile> = BTreeMap::new();
        for (id, cfg) in &section.backend {
            let provider = Provider::parse_strict(&cfg.provider)
                .ok_or_else(|| ProviderBindingError::UnknownProviderName(cfg.provider.clone()))?;
            let backend = BackendProfile {
                id: id.clone(),
                provider,
                backend_kind: cfg.backend_kind.clone(),
                base_url: cfg.base_url.clone(),
                options: cfg.options.clone(),
            };
            // id uniqueness within a single BTreeMap key space is
            // guaranteed by the map itself; no extra check needed.
            backends.insert(id.clone(), backend);
        }

        let mut auth_profiles: BTreeMap<String, AuthProfile> = BTreeMap::new();
        for (id, cfg) in &section.auth {
            let provider = Provider::parse_strict(&cfg.provider)
                .ok_or_else(|| ProviderBindingError::UnknownProviderName(cfg.provider.clone()))?;
            let profile = AuthProfile {
                id: id.clone(),
                provider,
                auth_method: cfg.auth_method.clone(),
                source: cfg.source.clone(),
                constraints: cfg.constraints.clone(),
                metadata_defaults: cfg.metadata_defaults.clone(),
            };
            auth_profiles.insert(id.clone(), profile);
        }

        let mut bindings: BTreeMap<String, ProviderBinding> = BTreeMap::new();
        for (id, cfg) in &section.binding {
            let backend = backends
                .get(&cfg.backend_profile)
                .ok_or_else(|| ProviderBindingError::UnknownBackend(cfg.backend_profile.clone()))?;
            let auth = auth_profiles
                .get(&cfg.auth_profile)
                .ok_or_else(|| ProviderBindingError::UnknownAuth(cfg.auth_profile.clone()))?;
            if backend.provider != auth.provider {
                return Err(ProviderBindingError::ProviderMismatch {
                    binding: id.clone(),
                    backend: backend.provider,
                    auth: auth.provider,
                });
            }
            let binding = ProviderBinding {
                id: id.clone(),
                backend_profile: cfg.backend_profile.clone(),
                auth_profile: cfg.auth_profile.clone(),
                default_model: cfg.default_model.clone(),
                policy: cfg.policy.clone(),
                provider_default: cfg.provider_default,
            };
            bindings.insert(id.clone(), binding);
        }

        Ok(Self {
            realm_id,
            backends,
            auth_profiles,
            bindings,
            default_binding: section.default_binding.clone(),
        })
    }

    /// Synthesize a default [`RealmConnectionSet`] for a given provider,
    /// sourcing credentials from a well-known env var. Used by surface
    /// factories when no explicit realm config exists but the user has
    /// set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` in
    /// the environment. OpenAI also supports an Azure env envelope:
    /// `AZURE_OPENAI_API_KEY` plus `AZURE_OPENAI_ENDPOINT` synthesizes the
    /// `azure_openai` backend instead of public OpenAI when no public OpenAI
    /// key is present. The synthesized realm is consumed by the same
    /// `ProviderRuntimeRegistry` path as explicit realms, so env-var auth and
    /// realm-config auth share one resolution pipeline.
    ///
    /// Returns a realm with id `"env_default"` containing one binding
    /// `"default"` pointing at:
    /// - BackendProfile `"default"` with the provider's default
    ///   backend_kind and base_url=None (provider client uses its default).
    /// - AuthProfile `"default"` with `source = Env { env: <ENV_VAR> }` and
    ///   the provider-specific env auth method.
    ///
    /// The ENV_VAR name is per-provider:
    /// - Anthropic: `ANTHROPIC_API_KEY`
    /// - OpenAI:   `OPENAI_API_KEY`
    /// - Azure OpenAI: `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_ENDPOINT`
    /// - Google:   `GEMINI_API_KEY`
    ///
    /// Callers should also honor `RKAT_*`-prefixed overrides via
    /// `ResolverEnvironment::with_process_env()`; that lookup is applied
    /// inside the registry's resolve path when it reads the env source.
    pub fn synthesize_env_default(provider: Provider) -> Self {
        Self::synthesize_env_default_from_lookup(provider, |key| std::env::var(key).ok())
    }

    /// Testable variant of [`Self::synthesize_env_default`] that lets callers
    /// inject the env lookup used to select the OpenAI public-vs-Azure default.
    pub fn synthesize_env_default_from_lookup<F>(provider: Provider, env_lookup: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let spec = env_default_spec(provider, env_lookup);
        Self::synthesize_default_from_spec(provider, spec)
    }

    fn synthesize_default_from_spec(provider: Provider, spec: EnvDefaultSpec) -> Self {
        let backend = BackendProfile {
            id: "default".to_string(),
            provider,
            backend_kind: spec.backend_kind.to_string(),
            base_url: spec.base_url,
            options: spec.options,
        };
        let source = CredentialSourceSpec::Env {
            env: spec.env_var.to_string(),
            fallback: spec.fallback,
        };
        let auth = AuthProfile {
            id: "default".to_string(),
            provider,
            auth_method: spec.auth_method.to_string(),
            source,
            constraints: AuthConstraints::default(),
            metadata_defaults: AuthMetadataDefaults::default(),
        };
        let binding = ProviderBinding {
            id: "default".to_string(),
            backend_profile: "default".to_string(),
            auth_profile: "default".to_string(),
            default_model: None,
            policy: BindingPolicy::default(),
            provider_default: true,
        };
        let mut backends = BTreeMap::new();
        backends.insert("default".to_string(), backend);
        let mut auth_profiles = BTreeMap::new();
        auth_profiles.insert("default".to_string(), auth);
        let mut bindings = BTreeMap::new();
        bindings.insert("default".to_string(), binding);
        Self {
            realm_id: RealmId::from_known_valid(ENV_DEFAULT_REALM_SLUG),
            backends,
            auth_profiles,
            bindings,
            default_binding: Some("default".to_string()),
        }
    }

    /// Resolve a binding by id. Returns the binding plus its referenced
    /// backend and auth profiles.
    pub fn lookup_binding(
        &self,
        id: &str,
    ) -> Result<(&ProviderBinding, &BackendProfile, &AuthProfile), ProviderBindingError> {
        let binding = self
            .bindings
            .get(id)
            .ok_or_else(|| ProviderBindingError::UnknownBinding(id.to_string()))?;
        let backend = self
            .backends
            .get(&binding.backend_profile)
            .ok_or_else(|| ProviderBindingError::UnknownBackend(binding.backend_profile.clone()))?;
        let auth = self
            .auth_profiles
            .get(&binding.auth_profile)
            .ok_or_else(|| ProviderBindingError::UnknownAuth(binding.auth_profile.clone()))?;
        Ok((binding, backend, auth))
    }

    /// Resolve a typed auth binding reference. `AuthBindingRef.profile`, when
    /// present, overrides the binding's configured auth profile while keeping
    /// the binding's backend and policy authoritative.
    pub fn lookup_auth_binding(
        &self,
        auth_binding: &AuthBindingRef,
    ) -> Result<(&ProviderBinding, &BackendProfile, &AuthProfile), ProviderBindingError> {
        let binding = self
            .bindings
            .get(auth_binding.binding.as_str())
            .ok_or_else(|| {
                ProviderBindingError::UnknownBinding(auth_binding.binding.to_string())
            })?;
        let backend = self
            .backends
            .get(&binding.backend_profile)
            .ok_or_else(|| ProviderBindingError::UnknownBackend(binding.backend_profile.clone()))?;
        let auth_profile_id = auth_binding
            .profile
            .as_ref()
            .map(ProfileId::as_str)
            .unwrap_or(binding.auth_profile.as_str());
        let auth = self
            .auth_profiles
            .get(auth_profile_id)
            .ok_or_else(|| ProviderBindingError::UnknownAuth(auth_profile_id.to_string()))?;
        Ok((binding, backend, auth))
    }
}

/// Validation / reference-resolution errors for a realm connection set.
///
/// The plan originally listed a `DuplicateId(String)` variant; it's been
/// omitted because `RealmConfigSection` uses `BTreeMap<String, ...>` for
/// backends/auth/bindings, so duplicate ids within one category are
/// impossible at ingestion time. Cross-category id sharing is harmless
/// (lookups are category-keyed). If a future code path constructs a
/// `RealmConfigSection` programmatically and needs duplicate detection,
/// add the variant back alongside the check.
#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderBindingError {
    #[error("unknown binding: {0}")]
    UnknownBinding(String),
    #[error("unknown backend: {0}")]
    UnknownBackend(String),
    #[error("unknown auth: {0}")]
    UnknownAuth(String),
    #[error("provider mismatch on binding {binding}: backend={backend:?} auth={auth:?}")]
    ProviderMismatch {
        binding: String,
        backend: Provider,
        auth: Provider,
    },
    #[error("unknown provider name: {0}")]
    UnknownProviderName(String),
    #[error("invalid realm id '{realm}': {source}")]
    InvalidRealmId {
        realm: String,
        source: IdentityError,
    },
}

// ---------------------------------------------------------------------
// Ingestion shapes (what TOML / config files deserialize into)
// ---------------------------------------------------------------------

/// Ingestion wrapper for `[realm.<id>.*]` TOML tables.
///
/// The singular nouns `backend`/`auth`/`binding` match TOML dotted-key
/// notation (`[realm.dev.backend.openai_default]`) so that one `.backend.X`
/// table becomes one entry in the `backend` map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct RealmConfigSection {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub backend: BTreeMap<String, BackendProfileConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub auth: BTreeMap<String, AuthProfileConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub binding: BTreeMap<String, ProviderBindingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_binding: Option<String>,
    /// Optional parent realm for config inheritance. Resolved into an ordered
    /// chain by [`RealmChain::resolve`]. Schema-invisible: `Config.realm` is
    /// wire-projected as an opaque `BTreeMap<String, Value>`, so this typed
    /// field never reaches the emitted schemas. A realm with no `parent` that
    /// is not itself `global` implicitly inherits from the reserved `global`
    /// realm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<RealmId>,
}

impl RealmConfigSection {
    /// Programmatic constructor for a realm populated from per-provider
    /// inline api keys. Used by surfaces (notably the WASM browser
    /// runtime) that receive credentials as plain strings at bootstrap
    /// and need to translate them into the realm-based config shape
    /// consumed by `AgentFactory::build_agent`.
    ///
    /// For each (provider, secret) pair, emits:
    ///   - a `BackendProfileConfig` whose `backend_kind` is the provider's
    ///     default kind from the typed provider-matrix enum
    ///     (`AnthropicBackendKind::AnthropicApi`, etc.)
    ///   - an `AuthProfileConfig` with `CredentialSourceSpec::InlineSecret`
    ///   - a `ProviderBindingConfig` wiring the two
    ///
    /// The first provider in the input list becomes the
    /// `default_binding` so that build_agent's auth_binding-less
    /// code path can resolve through this realm. Plan §6.10 replacement
    /// for the deleted `ProviderSettings.api_keys` map.
    pub fn from_inline_api_keys(entries: &[(&str, &str)]) -> Self {
        let mut backend = BTreeMap::new();
        let mut auth = BTreeMap::new();
        let mut binding = BTreeMap::new();
        let mut default_binding: Option<String> = None;

        for (idx, (provider, secret)) in entries.iter().enumerate() {
            let id = format!("default_{provider}");
            // Derive the (backend_kind, auth_method) inline-key default pair from
            // the typed provider-matrix enums that own each canonical string.
            // `other =>` stays the open-world fallback: an unrecognized provider
            // name carries its own slug as backend_kind with the conventional
            // `api_key` auth method, since no typed matrix enum owns it.
            let (backend_kind, auth_method) = match *provider {
                "anthropic" => (
                    AnthropicBackendKind::AnthropicApi.as_str(),
                    AnthropicAuthMethod::ApiKey.as_str(),
                ),
                "openai" => (
                    OpenAiBackendKind::OpenAiApi.as_str(),
                    OpenAiAuthMethod::ApiKey.as_str(),
                ),
                "gemini" | "google" => (
                    GoogleBackendKind::GoogleGenAi.as_str(),
                    GoogleAuthMethod::ApiKey.as_str(),
                ),
                other => (other, "api_key"),
            };
            backend.insert(
                id.clone(),
                BackendProfileConfig {
                    provider: provider.to_string(),
                    backend_kind: backend_kind.to_string(),
                    base_url: None,
                    options: serde_json::Value::Null,
                },
            );
            auth.insert(
                id.clone(),
                AuthProfileConfig {
                    provider: provider.to_string(),
                    auth_method: auth_method.to_string(),
                    source: CredentialSourceSpec::InlineSecret {
                        secret: (*secret).to_string(),
                    },
                    constraints: AuthConstraints::default(),
                    metadata_defaults: AuthMetadataDefaults::default(),
                },
            );
            binding.insert(
                id.clone(),
                ProviderBindingConfig {
                    backend_profile: id.clone(),
                    auth_profile: id.clone(),
                    default_model: None,
                    policy: BindingPolicy::default(),
                    // Every minted binding is the per-provider default; the
                    // "default for provider X" fact is carried by this typed
                    // marker, not the `default_<provider>` id convention.
                    provider_default: true,
                },
            );
            if idx == 0 {
                // The first provider also seeds the single per-realm default.
                default_binding = Some(id);
            }
        }

        Self {
            backend,
            auth,
            binding,
            default_binding,
            parent: None,
        }
    }
}

fn env_default_spec<F>(provider: Provider, env_lookup: F) -> EnvDefaultSpec
where
    F: Fn(&str) -> Option<String>,
{
    match provider {
        Provider::Anthropic => EnvDefaultSpec {
            backend_kind: AnthropicBackendKind::AnthropicApi.as_str(),
            auth_method: AnthropicAuthMethod::ApiKey.as_str(),
            env_var: "ANTHROPIC_API_KEY",
            fallback: vec![],
            base_url: None,
            options: serde_json::Value::Null,
        },
        Provider::OpenAI => openai_env_default_spec(env_lookup),
        Provider::Gemini => EnvDefaultSpec {
            backend_kind: GoogleBackendKind::GoogleGenAi.as_str(),
            auth_method: GoogleAuthMethod::ApiKey.as_str(),
            env_var: "GEMINI_API_KEY",
            fallback: vec!["GOOGLE_API_KEY".to_string()],
            base_url: None,
            options: serde_json::Value::Null,
        },
        Provider::SelfHosted => EnvDefaultSpec {
            backend_kind: SelfHostedBackendKind::SelfHosted.as_str(),
            auth_method: SelfHostedAuthMethod::ApiKey.as_str(),
            env_var: "RKAT_SELF_HOSTED_API_KEY",
            fallback: vec![],
            base_url: None,
            options: serde_json::Value::Null,
        },
        // `Provider::Other` has no typed backend/auth-method matrix enum (it is
        // the open-world fallback provider), so these literals have no enum to
        // derive from. They stay as the sole untyped owner of the
        // `other_api` / `api_key` default pair.
        Provider::Other => EnvDefaultSpec {
            backend_kind: "other_api",
            auth_method: "api_key",
            env_var: "RKAT_OTHER_API_KEY",
            fallback: vec![],
            base_url: None,
            options: serde_json::Value::Null,
        },
    }
}

fn openai_env_default_spec<F>(env_lookup: F) -> EnvDefaultSpec
where
    F: Fn(&str) -> Option<String>,
{
    let public_openai_key = env_value_with_rkat(&env_lookup, "OPENAI_API_KEY");
    let azure_key = env_value_with_rkat(&env_lookup, AZURE_OPENAI_API_KEY_ENV);
    let azure_endpoint = env_value_with_rkat(&env_lookup, AZURE_OPENAI_ENDPOINT_ENV);
    let azure_explicit = direct_env_value(&env_lookup, &format!("RKAT_{AZURE_OPENAI_API_KEY_ENV}"))
        .is_some()
        || direct_env_value(&env_lookup, &format!("RKAT_{AZURE_OPENAI_ENDPOINT_ENV}")).is_some();
    if azure_key.is_some()
        && let Some(endpoint) = azure_endpoint
        && (azure_explicit || public_openai_key.is_none())
    {
        let mut options = serde_json::Map::new();
        if let Some(deployment) =
            env_value_with_rkat(&env_lookup, AZURE_OPENAI_IMAGE_GENERATION_DEPLOYMENT_ENV)
                .or_else(|| env_value_with_rkat(&env_lookup, AZURE_OPENAI_IMAGE_DEPLOYMENT_ENV))
        {
            options.insert(
                "image_generation_deployment".to_string(),
                serde_json::Value::String(deployment),
            );
        }
        if let Some(api_version) =
            env_value_with_rkat(&env_lookup, AZURE_OPENAI_IMAGE_GENERATION_API_VERSION_ENV)
        {
            options.insert(
                "image_generation_api_version".to_string(),
                serde_json::Value::String(api_version),
            );
        }
        return EnvDefaultSpec {
            backend_kind: OpenAiBackendKind::AzureOpenAi.as_str(),
            auth_method: OpenAiAuthMethod::AzureApiKey.as_str(),
            env_var: AZURE_OPENAI_API_KEY_ENV,
            fallback: vec![],
            base_url: Some(endpoint),
            options: if options.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::Value::Object(options)
            },
        };
    }
    EnvDefaultSpec {
        backend_kind: OpenAiBackendKind::OpenAiApi.as_str(),
        auth_method: OpenAiAuthMethod::ApiKey.as_str(),
        env_var: "OPENAI_API_KEY",
        fallback: vec![],
        base_url: None,
        options: serde_json::Value::Null,
    }
}

fn env_value_with_rkat<F>(env_lookup: &F, candidate: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let rkat_override = if candidate.starts_with("RKAT_") {
        None
    } else {
        direct_env_value(env_lookup, &format!("RKAT_{candidate}"))
    };
    rkat_override.or_else(|| direct_env_value(env_lookup, candidate))
}

fn direct_env_value<F>(env_lookup: &F, key: &str) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    env_lookup(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Serialized backend profile (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct BackendProfileConfig {
    pub provider: String,
    pub backend_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub options: serde_json::Value,
}

/// Serialized auth profile (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthProfileConfig {
    pub provider: String,
    pub auth_method: String,
    pub source: CredentialSourceSpec,
    #[serde(default)]
    pub constraints: AuthConstraints,
    #[serde(default)]
    pub metadata_defaults: AuthMetadataDefaults,
}

/// Serialized binding (pre-normalization).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ProviderBindingConfig {
    pub backend_profile: String,
    pub auth_profile: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default)]
    pub policy: BindingPolicy,
    /// Marks this binding as the default for its provider. See
    /// [`ProviderBinding::provider_default`].
    #[serde(default, skip_serializing_if = "is_false")]
    pub provider_default: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::str::FromStr;

    // ---- Realm inheritance RCTs (parent chain + reserved global) ----------

    fn rid(s: &str) -> RealmId {
        RealmId::parse(s).expect("valid realm slug")
    }

    /// Build a Config whose `realm` map holds the given `(id, parent)` pairs.
    fn config_with(realms: &[(&str, Option<&str>)]) -> Config {
        let mut cfg = Config::default();
        for (id, parent) in realms {
            cfg.realm.insert(
                (*id).to_string(),
                RealmConfigSection {
                    parent: parent.map(rid),
                    ..Default::default()
                },
            );
        }
        cfg
    }

    fn chain_ids(chain: &RealmChain) -> Vec<&str> {
        chain.realms().iter().map(RealmId::as_str).collect()
    }

    // RCT-01
    #[test]
    fn realm_config_section_parent_roundtrips_and_defaults_none() {
        let with_parent = RealmConfigSection {
            parent: Some(RealmId::global()),
            ..Default::default()
        };
        let serialized = toml::to_string(&with_parent).expect("serialize section");
        let back: RealmConfigSection = toml::from_str(&serialized).expect("parse section");
        assert_eq!(back.parent, Some(RealmId::global()));

        let bare: RealmConfigSection = toml::from_str("").expect("parse empty section");
        assert_eq!(bare.parent, None, "absent parent must default to None");
    }

    // RCT-02
    #[test]
    fn global_realm_is_typed_and_distinct_from_env_default() {
        let global = RealmId::global();
        assert!(global.is_global());
        assert!(!global.is_env_default());
        assert_eq!(global.as_str(), GLOBAL_REALM_SLUG);

        let env = RealmId::from_known_valid(ENV_DEFAULT_REALM_SLUG);
        assert!(env.is_env_default());
        assert!(!env.is_global());

        let other = rid("prod");
        assert!(!other.is_global());
        assert!(!other.is_env_default());
    }

    // RCT-03
    #[test]
    fn realm_chain_resolves_linear_order_with_implicit_global_tail() {
        // child -> team (no parent) ; global configured -> implicit tail.
        let cfg = config_with(&[("child", Some("team")), ("team", None), ("global", None)]);
        let chain = RealmChain::resolve(&cfg, &rid("child")).expect("resolve");
        assert_eq!(chain_ids(&chain), ["child", "team", "global"]);

        // explicit parent==global terminates without double-visiting global.
        let cfg = config_with(&[("child", Some("global")), ("global", None)]);
        let chain = RealmChain::resolve(&cfg, &rid("child")).expect("resolve");
        assert_eq!(chain_ids(&chain), ["child", "global"]);

        // head==global terminates as a single node (no self-tail).
        let cfg = config_with(&[("global", None)]);
        let chain = RealmChain::resolve(&cfg, &rid("global")).expect("resolve");
        assert_eq!(chain_ids(&chain), ["global"]);
    }

    // RCT-04
    #[test]
    fn realm_chain_detects_cycle_depth_missing_global_and_env_default() {
        // self-parent -> Cycle
        let cfg = config_with(&[("a", Some("a"))]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("a")),
            Err(RealmChainError::Cycle { .. })
        ));

        // A -> B -> A -> Cycle
        let cfg = config_with(&[("a", Some("b")), ("b", Some("a"))]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("a")),
            Err(RealmChainError::Cycle { .. })
        ));

        // parent not configured (and not global) -> MissingParent
        let cfg = config_with(&[("a", Some("ghost"))]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("a")),
            Err(RealmChainError::MissingParent { .. })
        ));

        // global with a parent -> GlobalHasParent
        let cfg = config_with(&[("global", Some("x")), ("x", None)]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("global")),
            Err(RealmChainError::GlobalHasParent { .. })
        ));

        // a child reaching a global-that-has-a-parent also fails closed
        let cfg = config_with(&[
            ("child", Some("global")),
            ("global", Some("x")),
            ("x", None),
        ]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("child")),
            Err(RealmChainError::GlobalHasParent { .. })
        ));

        // parent == env_default slug -> ParentIsEnvDefault
        let cfg = config_with(&[("a", Some(ENV_DEFAULT_REALM_SLUG))]);
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("a")),
            Err(RealmChainError::ParentIsEnvDefault { .. })
        ));

        // chain longer than MAX_REALM_CHAIN_DEPTH -> DepthExceeded
        let mut pairs: Vec<(String, Option<String>)> = Vec::new();
        let n = MAX_REALM_CHAIN_DEPTH + 4;
        for i in 0..n {
            let parent = if i + 1 < n {
                Some(format!("r{}", i + 1))
            } else {
                None
            };
            pairs.push((format!("r{i}"), parent));
        }
        let mut cfg = Config::default();
        for (id, parent) in &pairs {
            cfg.realm.insert(
                id.clone(),
                RealmConfigSection {
                    parent: parent.as_deref().map(rid),
                    ..Default::default()
                },
            );
        }
        assert!(matches!(
            RealmChain::resolve(&cfg, &rid("r0")),
            Err(RealmChainError::DepthExceeded { .. })
        ));
    }

    // RCT-05
    #[test]
    fn realm_chain_omits_absent_global_and_terminates_at_explicit_root() {
        // No [realm.global] configured -> no implicit tail appended.
        let cfg = config_with(&[("a", Some("b")), ("b", None)]);
        let chain = RealmChain::resolve(&cfg, &rid("a")).expect("resolve");
        assert_eq!(
            chain_ids(&chain),
            ["a", "b"],
            "no global must not be invented"
        );
    }

    // RCT-26
    #[test]
    fn absent_head_realm_yields_single_node_chain_then_implicit_tail() {
        // Head absent from config, global present -> [head, global].
        let cfg = config_with(&[("global", None)]);
        let chain = RealmChain::resolve(&cfg, &rid("missing")).expect("resolve");
        assert_eq!(chain_ids(&chain), ["missing", "global"]);

        // Head absent, no global -> [head] alone (contributes nothing; resolver
        // falls through to env_default downstream).
        let cfg = Config::default();
        let chain = RealmChain::resolve(&cfg, &rid("missing")).expect("resolve");
        assert_eq!(chain_ids(&chain), ["missing"]);
    }

    #[test]
    fn member_comms_name_round_trips_through_display_and_from_str() {
        let name = MemberCommsName::new("team", "reviewer", "alice").unwrap();
        assert_eq!(name.to_string(), "team/reviewer/alice");
        let parsed = MemberCommsName::from_str("team/reviewer/alice").unwrap();
        assert_eq!(parsed, name);
        assert_eq!(parsed.mob_id(), "team");
        assert_eq!(parsed.role(), "reviewer");
        assert_eq!(parsed.member(), "alice");
    }

    #[test]
    fn member_comms_name_from_str_is_fail_closed() {
        // Wrong component count.
        assert!(matches!(
            MemberCommsName::from_str("team/reviewer"),
            Err(MemberCommsNameError::WrongComponentCount)
        ));
        assert!(matches!(
            MemberCommsName::from_str("team/reviewer/alice/extra"),
            Err(MemberCommsNameError::WrongComponentCount)
        ));
        // Empty component.
        assert!(matches!(
            MemberCommsName::from_str("team//alice"),
            Err(MemberCommsNameError::InvalidComponent { .. })
        ));
        // Leading digit / disallowed first char (folds is_valid_peer_name_component).
        assert!(MemberCommsName::from_str("1team/reviewer/alice").is_err());
        // Disallowed char.
        assert!(MemberCommsName::from_str("te.am/reviewer/alice").is_err());
        // Underscore-first is allowed.
        assert!(MemberCommsName::from_str("_team/reviewer/alice").is_ok());
    }

    #[test]
    fn member_comms_name_components_are_always_valid_realm_slugs() {
        // The component rule is strictly tighter than validate_slug, so any
        // valid comms name yields a parseable realm via the shared helper.
        let name = MemberCommsName::new("team", "reviewer", "alice").unwrap();
        assert!(mob_realm_id(name.mob_id()).is_ok());
        assert_eq!(mob_realm_id("team").unwrap().as_str(), "mob.team");
    }

    #[test]
    fn mob_member_binding_round_trips_to_comms_name() {
        let binding = MobMemberBinding {
            mob_id: "team".to_string(),
            role: "reviewer".to_string(),
            member: "alice".to_string(),
        };
        assert_eq!(
            binding.comms_name().unwrap().to_string(),
            "team/reviewer/alice"
        );
    }

    #[test]
    fn peer_role_external_label_is_typed_not_magic_string() {
        assert_eq!(PeerRole::External.as_label(), "external");
        assert_eq!(
            PeerRole::Member("reviewer".to_string()).as_label(),
            "reviewer"
        );
    }

    fn config_with_realms(toml_input: &str) -> Config {
        Config {
            realm: toml::from_str(toml_input).unwrap(),
            ..Default::default()
        }
    }

    fn openai_target_config() -> Config {
        config_with_realms(
            r#"
[prod]
default_binding = "primary"

[prod.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[prod.auth.openai_oauth]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[prod.binding.primary]
backend_profile = "openai_default"
auth_profile = "openai_oauth"

[prod.binding.secondary]
backend_profile = "openai_default"
auth_profile = "openai_oauth"
"#,
        )
    }

    fn lookup_from_pairs(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<String> {
        move |key| {
            pairs
                .iter()
                .find_map(|(candidate, value)| (*candidate == key).then(|| (*value).to_string()))
        }
    }

    #[test]
    fn auth_binding_is_purely_structural() {
        let c = AuthBindingRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        assert_eq!(c.realm.as_str(), "dev");
        assert_eq!(c.binding.as_str(), "default_openai");
        assert!(c.profile.is_none());
        assert!(!c.is_env_default());
    }

    #[test]
    fn auth_binding_serde_roundtrip_with_profile() {
        let c = AuthBindingRef {
            realm: RealmId::parse("prod").unwrap(),
            binding: BindingId::parse("gpt5").unwrap(),
            profile: Some(ProfileId::parse("override").unwrap()),
            origin: BindingOrigin::Configured,
        };
        let s = serde_json::to_string(&c).unwrap();
        assert!(s.contains("\"realm\":\"prod\""));
        assert!(s.contains("\"binding\":\"gpt5\""));
        assert!(s.contains("\"profile\":\"override\""));
        // Configured origin is the default and is skipped on the wire so the
        // shape stays additive for old readers.
        assert!(!s.contains("origin"));
        let back: AuthBindingRef = serde_json::from_str(&s).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn auth_binding_origin_is_typed_not_slug() {
        // Synthetic env-default origin is carried by the typed discriminant,
        // not recovered from the realm/binding slug text.
        let synthetic = AuthBindingRef {
            realm: RealmId::parse("env_default").unwrap(),
            binding: BindingId::parse("default").unwrap(),
            profile: None,
            origin: BindingOrigin::SyntheticEnvDefault,
        };
        assert!(synthetic.is_env_default());

        // Same slugs, configured origin → NOT an env-default. Proves the
        // decision keys on the typed origin, not on "env_default"/"default".
        let configured = AuthBindingRef {
            realm: RealmId::parse("env_default").unwrap(),
            binding: BindingId::parse("default").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        assert!(!configured.is_env_default());

        // Synthetic origin survives a serde round-trip.
        let s = serde_json::to_string(&synthetic).unwrap();
        assert!(s.contains("\"origin\":\"synthetic_env_default\""));
        let back: AuthBindingRef = serde_json::from_str(&s).unwrap();
        assert_eq!(back, synthetic);

        // A row without an `origin` field reads back as Configured.
        let legacy = r#"{"realm":"env_default","binding":"default"}"#;
        let back: AuthBindingRef = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.origin, BindingOrigin::Configured);
        assert!(!back.is_env_default());
    }

    #[test]
    fn auth_binding_profile_overrides_binding_auth_profile() {
        let toml = r#"
realm_id = "prod"
default_binding = "primary"

[backend.openai_default]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://api.openai.com/v1"

[auth.default_profile]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[auth.override_profile]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OVERRIDE_OPENAI_API_KEY" }

[binding.primary]
backend_profile = "openai_default"
auth_profile = "default_profile"
"#;
        let section: RealmConfigSection = toml::from_str(toml).unwrap();
        let realm = RealmConnectionSet::from_config("prod", &section).unwrap();
        let auth_binding = AuthBindingRef {
            realm: RealmId::parse("prod").unwrap(),
            binding: BindingId::parse("primary").unwrap(),
            profile: Some(ProfileId::parse("override_profile").unwrap()),
            origin: BindingOrigin::Configured,
        };

        let (_binding, _backend, auth) = realm.lookup_auth_binding(&auth_binding).unwrap();
        assert_eq!(auth.id, "override_profile");
    }

    #[test]
    fn identity_slugs_reject_invalid_characters() {
        assert!(RealmId::parse("").is_err());
        assert!(BindingId::parse("bad space").is_err());
        assert!(ProfileId::parse("bad:colon").is_err());
        assert!(RealmId::parse("dev").is_ok());
        assert!(BindingId::parse("openai_default.v1").is_ok());
    }

    #[test]
    fn credential_source_spec_serde() {
        for src in [
            CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            CredentialSourceSpec::ManagedStore,
            CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".into(),
                fallback: Vec::new(),
            },
            CredentialSourceSpec::ExternalResolver {
                handle: "desktop".into(),
            },
            CredentialSourceSpec::PlatformDefault,
        ] {
            let s = serde_json::to_string(&src).unwrap();
            let back: CredentialSourceSpec = serde_json::from_str(&s).unwrap();
            assert_eq!(back, src);
        }
    }

    #[test]
    fn credential_source_spec_rejects_unknown_kind() {
        let bad = r#"{"kind":"nonexistent","foo":"bar"}"#;
        let err = serde_json::from_str::<CredentialSourceSpec>(bad).unwrap_err();
        assert!(
            err.to_string().contains("nonexistent") || err.to_string().contains("unknown variant"),
            "serde error should mention unknown variant: {err}",
        );
    }

    #[test]
    fn env_default_openai_uses_public_openai_without_azure_envelope() {
        let realm = RealmConnectionSet::synthesize_env_default_from_lookup(
            Provider::OpenAI,
            lookup_from_pairs(&[]),
        );
        let backend = realm.backends.get("default").unwrap();
        let auth = realm.auth_profiles.get("default").unwrap();

        assert_eq!(backend.backend_kind, "openai_api");
        assert_eq!(backend.base_url, None);
        assert_eq!(auth.auth_method, "api_key");
        assert_eq!(
            auth.source,
            CredentialSourceSpec::Env {
                env: "OPENAI_API_KEY".to_string(),
                fallback: Vec::new(),
            }
        );
    }

    #[test]
    fn env_default_openai_uses_azure_when_key_and_endpoint_are_present() {
        let realm = RealmConnectionSet::synthesize_env_default_from_lookup(
            Provider::OpenAI,
            lookup_from_pairs(&[
                ("AZURE_OPENAI_API_KEY", "azure-key"),
                ("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com/"),
                (
                    "AZURE_OPENAI_IMAGE_GENERATION_DEPLOYMENT",
                    "image-deployment-a",
                ),
                ("AZURE_OPENAI_IMAGE_GENERATION_API_VERSION", "preview"),
            ]),
        );
        let backend = realm.backends.get("default").unwrap();
        let auth = realm.auth_profiles.get("default").unwrap();

        assert_eq!(backend.backend_kind, "azure_openai");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://example.openai.azure.com/")
        );
        assert_eq!(
            backend.options["image_generation_deployment"],
            "image-deployment-a"
        );
        assert_eq!(backend.options["image_generation_api_version"], "preview");
        assert_eq!(auth.auth_method, "azure_api_key");
        assert_eq!(
            auth.source,
            CredentialSourceSpec::Env {
                env: "AZURE_OPENAI_API_KEY".to_string(),
                fallback: Vec::new(),
            }
        );
    }

    #[test]
    fn env_default_openai_keeps_public_key_when_plain_azure_and_public_keys_are_both_set() {
        let realm = RealmConnectionSet::synthesize_env_default_from_lookup(
            Provider::OpenAI,
            lookup_from_pairs(&[
                ("OPENAI_API_KEY", "public-key"),
                ("AZURE_OPENAI_API_KEY", "azure-key"),
                ("AZURE_OPENAI_ENDPOINT", "https://example.openai.azure.com"),
            ]),
        );
        let backend = realm.backends.get("default").unwrap();

        assert_eq!(backend.backend_kind, "openai_api");
        assert_eq!(backend.base_url, None);
    }

    #[test]
    fn env_default_openai_rkat_azure_envelope_overrides_public_openai_key() {
        let realm = RealmConnectionSet::synthesize_env_default_from_lookup(
            Provider::OpenAI,
            lookup_from_pairs(&[
                ("OPENAI_API_KEY", "public-key"),
                ("RKAT_AZURE_OPENAI_API_KEY", "azure-key"),
                (
                    "RKAT_AZURE_OPENAI_ENDPOINT",
                    "https://example.openai.azure.com",
                ),
            ]),
        );
        let backend = realm.backends.get("default").unwrap();
        let auth = realm.auth_profiles.get("default").unwrap();

        assert_eq!(backend.backend_kind, "azure_openai");
        assert_eq!(
            backend.base_url.as_deref(),
            Some("https://example.openai.azure.com")
        );
        assert_eq!(auth.auth_method, "azure_api_key");
    }

    #[test]
    fn from_config_empty_section_yields_empty_set() {
        let section = RealmConfigSection::default();
        let set = RealmConnectionSet::from_config("dev", &section).expect("empty section is valid");
        assert_eq!(set.realm_id.as_str(), "dev");
        assert!(set.backends.is_empty());
        assert!(set.auth_profiles.is_empty());
        assert!(set.bindings.is_empty());
        assert_eq!(set.default_binding, None);
    }

    #[test]
    fn lookup_binding_returns_unknown_binding() {
        let set = RealmConnectionSet::from_config("dev", &RealmConfigSection::default())
            .expect("empty section valid");
        let err = set
            .lookup_binding("missing")
            .expect_err("empty set has no bindings");
        assert_eq!(err, ProviderBindingError::UnknownBinding("missing".into()));
    }

    #[test]
    fn connection_target_uses_configured_realm_default_binding() {
        let config = openai_target_config();
        let preferred_realm = RealmId::parse("prod").unwrap();
        let target = resolve_realm_binding_target_for_provider(
            &config,
            Provider::OpenAI,
            None,
            None,
            None,
            Some(&preferred_realm),
            false,
        )
        .unwrap();

        assert_eq!(target.auth_binding.realm.as_str(), "prod");
        assert_eq!(target.auth_binding.binding.as_str(), "primary");
        assert_eq!(target.binding.id, "primary");
    }

    #[test]
    fn connection_target_explicit_binding_wins_with_preferred_realm() {
        let config = openai_target_config();
        let preferred_realm = RealmId::parse("prod").unwrap();
        let binding = BindingId::parse("secondary").unwrap();
        let target = resolve_realm_binding_target_for_provider(
            &config,
            Provider::OpenAI,
            None,
            Some(&binding),
            None,
            Some(&preferred_realm),
            false,
        )
        .unwrap();

        assert_eq!(target.auth_binding.realm.as_str(), "prod");
        assert_eq!(target.auth_binding.binding.as_str(), "secondary");
        assert_eq!(target.binding.id, "secondary");
    }

    // An EXPLICITLY named binding whose provider disagrees with the requested
    // provider is a strict ProviderMismatch (explicit requests are not
    // provider-filtered). Default-selection by contrast simply yields no
    // candidate for a provider the realm has no binding for (see RCT-10).
    #[test]
    fn connection_target_rejects_provider_mismatch() {
        let config = openai_target_config();
        let preferred_realm = RealmId::parse("prod").unwrap();
        let binding = BindingId::parse("primary").unwrap();
        let err = resolve_realm_binding_target_for_provider(
            &config,
            Provider::Anthropic,
            None,
            Some(&binding),
            None,
            Some(&preferred_realm),
            false,
        )
        .unwrap_err();

        assert!(matches!(
            err,
            ConnectionTargetError::ProviderMismatch {
                expected: Provider::Anthropic,
                backend: Provider::OpenAI,
                auth: Provider::OpenAI,
                ..
            }
        ));
    }

    // ---- P2: chain-aware resolution + owning-realm provenance -------------

    /// global owns the openai binding `primary`; `child` inherits via parent.
    fn openai_inherit_config(extra_child: &str) -> Config {
        config_with_realms(&format!(
            r#"
[global]
default_binding = "primary"

[global.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[global.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = {{ kind = "platform_default" }}

[global.binding.primary]
backend_profile = "openai_default"
auth_profile = "openai_key"

[child]
parent = "global"
{extra_child}
"#
        ))
    }

    // RCT-06
    #[test]
    fn inherited_binding_stamps_owning_realm_not_consuming_realm() {
        let cfg = openai_inherit_config("");
        let child = rid("child");
        let candidates = resolve_auth_binding_candidates_for_provider(
            &cfg,
            Provider::OpenAI,
            None,
            Some(&child),
            false,
        )
        .expect("resolve");
        assert_eq!(
            candidates[0].auth_binding.realm.as_str(),
            "global",
            "owner is the defining realm, not the consuming child"
        );
        assert_eq!(candidates[0].auth_binding.binding.as_str(), "primary");
    }

    // RCT-07
    #[test]
    fn resolved_target_realm_equals_owning_connection_set_realm_id() {
        let cfg = openai_inherit_config("");
        let child = rid("child");
        let candidates = resolve_auth_binding_candidates_for_provider(
            &cfg,
            Provider::OpenAI,
            None,
            Some(&child),
            false,
        )
        .expect("resolve");
        let target = &candidates[0];
        // The registry equality (auth_binding.realm == realm.realm_id) holds for
        // an inherited binding WITHOUT any relaxation.
        assert_eq!(
            target.realm.realm_id.as_str(),
            target.auth_binding.realm.as_str()
        );
        assert_eq!(target.realm.realm_id.as_str(), "global");
    }

    // RCT-08
    #[test]
    fn inherited_binding_resolves_backend_auth_in_owning_realm_only() {
        // child redefines the SAME auth-profile key as a DIFFERENT provider.
        // The inherited binding (owned by global) must resolve global's auth,
        // never child's shadow (binding-owner == auth-owner invariant).
        let cfg = openai_inherit_config(
            r#"
[child.auth.openai_key]
provider = "gemini"
auth_method = "api_key"
source = { kind = "platform_default" }
"#,
        );
        let child = rid("child");
        let target = resolve_auth_binding_or_default_for_provider(
            &cfg,
            Provider::OpenAI,
            None,
            Some(&child),
            false,
        )
        .expect("resolve");
        assert_eq!(target.auth_binding.realm.as_str(), "global");
        assert_eq!(
            target.auth_profile.provider,
            Provider::OpenAI,
            "auth must resolve in the owning (global) section, not child's shadow"
        );
    }

    // RCT-10
    #[test]
    fn single_target_path_uses_unified_selection_policy() {
        // A realm with NO default_binding but a single unambiguous provider
        // binding resolves it (old default_binding-only path would have failed).
        let cfg = config_with_realms(
            r#"
[solo]

[solo.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[solo.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[solo.binding.only]
backend_profile = "openai_default"
auth_profile = "openai_key"
"#,
        );
        let solo = rid("solo");
        let target = resolve_realm_binding_target_for_provider(
            &cfg,
            Provider::OpenAI,
            None,
            None,
            None,
            Some(&solo),
            false,
        )
        .expect("single unambiguous provider binding resolves without default_binding");
        assert_eq!(target.auth_binding.binding.as_str(), "only");
        assert_eq!(target.auth_binding.realm.as_str(), "solo");
    }

    // RCT-11
    #[test]
    fn explicit_inherited_binding_resolves_at_owning_realm() {
        let cfg = openai_inherit_config("");
        let explicit = AuthBindingRef {
            realm: rid("child"),
            binding: BindingId::parse("primary").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        let target = resolve_auth_binding_or_default_for_provider(
            &cfg,
            Provider::OpenAI,
            Some(&explicit),
            None,
            false,
        )
        .expect("explicit inherited binding resolves");
        assert_eq!(target.auth_binding.realm.as_str(), "global");
        assert_eq!(target.auth_binding.binding.as_str(), "primary");
    }

    // RCT-25
    #[test]
    fn default_realm_literal_not_consulted_then_works_under_global() {
        // [realm.default] is NOT auto-consulted for an unrelated head.
        let with_default = config_with_realms(
            r#"
[default]
default_binding = "p"

[default.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[default.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[default.binding.p]
backend_profile = "openai_default"
auth_profile = "openai_key"

[prod]
"#,
        );
        let prod = rid("prod");
        let candidates = resolve_auth_binding_candidates_for_provider(
            &with_default,
            Provider::OpenAI,
            None,
            Some(&prod),
            false,
        )
        .unwrap_or_default();
        assert!(
            candidates
                .iter()
                .all(|c| c.auth_binding.realm.as_str() != "default"),
            "the literal 'default' realm must not be consulted for an unrelated head"
        );

        // The SAME binding under [realm.global] IS inherited via the implicit tail.
        let under_global = config_with_realms(
            r#"
[global]
default_binding = "p"

[global.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[global.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[global.binding.p]
backend_profile = "openai_default"
auth_profile = "openai_key"

[prod]
"#,
        );
        let candidates = resolve_auth_binding_candidates_for_provider(
            &under_global,
            Provider::OpenAI,
            None,
            Some(&prod),
            false,
        )
        .expect("global is inherited via the implicit tail");
        assert_eq!(candidates[0].auth_binding.realm.as_str(), "global");
        assert_eq!(candidates[0].auth_binding.binding.as_str(), "p");
    }

    // RCT-35
    #[test]
    fn nearest_child_wins_when_head_and_ancestor_both_define_binding() {
        let cfg = config_with_realms(
            r#"
[global]
default_binding = "g"

[global.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[global.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[global.binding.g]
backend_profile = "openai_default"
auth_profile = "openai_key"

[team]
parent = "global"
default_binding = "t"

[team.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[team.auth.openai_key]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "platform_default" }

[team.binding.t]
backend_profile = "openai_default"
auth_profile = "openai_key"
"#,
        );
        let team = rid("team");
        let candidates = resolve_auth_binding_candidates_for_provider(
            &cfg,
            Provider::OpenAI,
            None,
            Some(&team),
            false,
        )
        .expect("resolve");
        assert_eq!(
            candidates[0].auth_binding.realm.as_str(),
            "team",
            "nearest-child first"
        );
        assert_eq!(candidates[0].auth_binding.binding.as_str(), "t");
        assert_eq!(candidates[1].auth_binding.realm.as_str(), "global");
        assert_eq!(candidates[1].auth_binding.binding.as_str(), "g");
    }

    // RCT-38
    #[test]
    fn explicit_env_default_ref_rejected_before_chain_walk() {
        let cfg = openai_inherit_config("");
        let env_ref = AuthBindingRef {
            realm: RealmId::from_known_valid(ENV_DEFAULT_REALM_SLUG),
            binding: BindingId::parse("default").unwrap(),
            profile: None,
            origin: BindingOrigin::SyntheticEnvDefault,
        };
        let err = resolve_auth_binding_or_default_for_provider(
            &cfg,
            Provider::OpenAI,
            Some(&env_ref),
            None,
            true,
        )
        .unwrap_err();
        assert!(matches!(err, ConnectionTargetError::UnknownRealm(_)));
    }

    // Strict-owner write (decision 5) — single core policy consumed by REST/RPC.
    #[test]
    fn resolve_write_owner_classifies_owned_inherited_and_unknown() {
        let cfg = openai_inherit_config(""); // global owns "primary"; child parent=global
        let primary = BindingId::parse("primary").unwrap();

        // Head owns the binding in its OWN section -> write allowed at head.
        assert_eq!(
            resolve_write_owner(&cfg, &rid("global"), &primary)
                .expect("global owns primary")
                .as_str(),
            "global"
        );

        // Inherited by child -> rejected, naming the owning realm.
        let err = resolve_write_owner(&cfg, &rid("child"), &primary).unwrap_err();
        assert!(
            matches!(&err, WriteOwnerError::Inherited { owner, .. } if owner == "global"),
            "expected Inherited{{owner=global}}, got {err:?}"
        );

        // Not defined anywhere on the chain -> Unknown.
        let err = resolve_write_owner(&cfg, &rid("child"), &BindingId::parse("nope").unwrap())
            .unwrap_err();
        assert!(matches!(err, WriteOwnerError::Unknown { .. }));
    }

    #[test]
    fn auth_binding_candidates_prefer_provider_binding_in_preferred_realm() {
        let config = config_with_realms(
            r#"
[dev]
default_binding = "openai_oauth"

[dev.backend.openai_chatgpt]
provider = "openai"
backend_kind = "openai_chatgpt"

[dev.auth.openai_oauth]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "managed_store" }

[dev.binding.openai_oauth]
backend_profile = "openai_chatgpt"
auth_profile = "openai_oauth"
default_model = "test-openai-default"
"#,
        );
        let preferred_realm = RealmId::parse("dev").unwrap();

        let candidates = resolve_auth_binding_candidates_for_provider(
            &config,
            Provider::OpenAI,
            None,
            Some(&preferred_realm),
            true,
        )
        .expect("candidates resolve");

        assert_eq!(candidates[0].auth_binding.realm.as_str(), "dev");
        assert_eq!(candidates[0].auth_binding.binding.as_str(), "openai_oauth");
        assert!(!candidates[0].auth_binding.is_env_default());
    }

    // RCT-09: the flat cross-realm scan is gone. An unrelated sibling realm
    // (not an ancestor of the head) is NOT auto-discovered; only chain members
    // + env_default are candidates.
    #[test]
    fn auth_binding_candidates_exclude_unrelated_sibling_realm() {
        let config = config_with_realms(
            r#"
[dev]

[dev.backend.openai_chatgpt]
provider = "openai"
backend_kind = "openai_chatgpt"

[dev.auth.openai_oauth]
provider = "openai"
auth_method = "chatgpt_oauth"
source = { kind = "managed_store" }

[dev.binding.openai_oauth]
backend_profile = "openai_chatgpt"
auth_profile = "openai_oauth"
"#,
        );
        // Head 'missing' is absent, has no parent edge, and there is no global
        // realm — so 'dev' is an unrelated sibling and must NOT be discovered.
        let preferred_realm = RealmId::parse("missing").unwrap();

        let candidates = resolve_auth_binding_candidates_for_provider(
            &config,
            Provider::OpenAI,
            None,
            Some(&preferred_realm),
            true,
        )
        .expect("candidates resolve");

        assert!(
            candidates
                .iter()
                .all(|c| c.auth_binding.realm.as_str() != "dev"),
            "unrelated sibling 'dev' must not be discovered via a flat scan"
        );
        // Only the synthetic env-var fallback remains.
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].auth_binding.is_env_default());
        assert_eq!(
            candidates[0].auth_binding.origin,
            BindingOrigin::SyntheticEnvDefault
        );
    }

    #[test]
    fn from_inline_api_keys_marks_each_provider_default() {
        let section = RealmConfigSection::from_inline_api_keys(&[
            ("anthropic", "sk-ant"),
            ("openai", "sk-oai"),
        ]);
        // Every provider's minted binding carries the typed per-provider
        // default marker — not just the first (idx==0) one.
        assert!(section.binding["default_anthropic"].provider_default);
        assert!(section.binding["default_openai"].provider_default);
        // The first provider still seeds the single per-realm default.
        assert_eq!(
            section.default_binding.as_deref(),
            Some("default_anthropic")
        );
    }

    #[test]
    fn selected_binding_prefers_typed_provider_default_marker() {
        // Two openai bindings; the second is marked provider_default. The
        // selector must pick by the typed marker, not by any id name.
        let config = config_with_realms(
            r#"
[dev]

[dev.backend.openai_default]
provider = "openai"
backend_kind = "openai_api"

[dev.auth.openai_api]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[dev.binding.alpha]
backend_profile = "openai_default"
auth_profile = "openai_api"

[dev.binding.beta]
backend_profile = "openai_default"
auth_profile = "openai_api"
provider_default = true
"#,
        );
        let preferred_realm = RealmId::parse("dev").unwrap();
        let candidates = resolve_auth_binding_candidates_for_provider(
            &config,
            Provider::OpenAI,
            None,
            Some(&preferred_realm),
            false,
        )
        .expect("candidates resolve");

        assert_eq!(candidates[0].auth_binding.realm.as_str(), "dev");
        assert_eq!(candidates[0].auth_binding.binding.as_str(), "beta");
    }

    #[test]
    fn realm_config_section_serde_empty() {
        let section = RealmConfigSection::default();
        let s = serde_json::to_string(&section).unwrap();
        // All maps empty + no default_binding → empty object.
        assert_eq!(s, "{}");
    }

    #[test]
    fn realm_config_section_serde_populated() {
        // `default_binding` appears BEFORE any section header so that TOML
        // treats it as a top-level field rather than a key inside the last
        // subsection.
        let toml_input = r#"
default_binding = "default_openai"

[backend.openai_default]
provider = "openai"
backend_kind = "openai_api"
base_url = "https://api.openai.com"

[auth.openai_api_key]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[binding.default_openai]
backend_profile = "openai_default"
auth_profile = "openai_api_key"
default_model = "test-openai-other"
"#;
        let section: RealmConfigSection = toml::from_str(toml_input).unwrap();
        assert_eq!(section.backend.len(), 1);
        assert_eq!(section.auth.len(), 1);
        assert_eq!(section.binding.len(), 1);
        assert_eq!(section.default_binding.as_deref(), Some("default_openai"));
        assert_eq!(
            section.backend["openai_default"].base_url.as_deref(),
            Some("https://api.openai.com"),
        );
    }
}
