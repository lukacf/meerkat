//! The native realm a launch's members read their configuration from, and the
//! auth binding every member it constructs builds with.
//!
//! `runtime_options.native_realm = {realm_id, context_root, state_root,
//! user_config_root}` names a realm a native host keeps. The gateway reads
//! that realm's configuration with the calls a native host makes: Core's
//! [`meerkat_core::StorageLayout`] resolves the realm at the explicit state
//! root, the store's [`meerkat_store::FilesystemRealmConfigSource`] maps the
//! realm and its chain to their documents, Core's
//! [`meerkat_core::EffectiveConfigReader`] composes them, and the result takes
//! Core's environment overrides and validation. The composed configuration is
//! the members' whole configuration, in place of
//! `runtime_options.meerkat_config_path` and never beside it. Nothing is
//! copied or written, and no path is derived beyond what Core derives from
//! the four inputs.
//!
//! `runtime_options.member_auth_binding = {realm, binding, profile?,
//! origin?}` is Core's [`meerkat_core::AuthBindingRef`]; `origin`, when
//! present, must be `"configured"` ([`meerkat_core::BindingOrigin::Configured`]).
//! At init the gateway resolves it in the members' configuration with Core's
//! explicit resolution, refuses a definition profile whose model or provider
//! the binding does not serve, and refuses every path that would make
//! provider calls off the binding: an automatic spawn policy, a model
//! fallback target that names another binding or none, image generation and
//! the agent memory engines, in the definition and in what the state already
//! holds: the realm profiles the definition references
//! ([`validate_realm_profiles`]), identity declarations an earlier launch
//! adopted ([`validate_adopted_declarations`]) and stored child mobs
//! ([`validate_stored_child_mobs`]). It then installs
//! [`MemberAuthBindingCustomizer`], which forces the binding onto every member
//! spawn Core asks the gateway's spawn customizers about: fresh spawns,
//! helpers, forks, respawns and successors, and the rebuild of a member on its
//! durable session, where an explicit binding takes the place of the durable
//! one; and [`member_build_hook`], which refuses every member build that
//! would run off the binding (a member Core rebuilds from its own records,
//! a fork seated before the launch named the binding, say) or still make
//! provider calls off it ([`MemberBuildGuard`]). A spawn that names another
//! binding is refused, and an agent meets each refusal typed
//! ([`agent_tool_binding_refusal`], [`agent_tool_realm_profile_refusal`]).
//!
//! Both are settled once, before any agent factory is built. A running
//! gateway never rebinds: a changed realm document or binding takes effect
//! in a new launch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `runtime_options` key of the native realm.
const NATIVE_REALM_KEY: &str = "native_realm";
/// `runtime_options` key of the members' auth binding.
const MEMBER_AUTH_BINDING_KEY: &str = "member_auth_binding";
/// `runtime_options` key of the single host config file, which a native realm
/// replaces.
const MEERKAT_CONFIG_PATH_KEY: &str = "meerkat_config_path";
/// The one value `member_auth_binding.origin` may carry.
const CONFIGURED_ORIGIN: &str = "configured";
/// JSON-RPC invalid params.
const INVALID_PARAMS: i64 = -32602;
/// The kind an agent's mob tool answers when a spawn names another auth
/// binding than the launch's (a [`meerkat_core::ToolConsequenceDenial`]
/// code), and the prefix of the wiring error a host spawn meets.
pub(crate) const BINDING_CONFLICT_KIND: &str = MemberRouteRefusalKind::Conflict.as_str();
/// The kind an agent's mob tool answers when a child mob it creates would
/// build members off the launch's auth binding.
pub(crate) const BINDING_UNCARRIED_KIND: &str = MemberRouteRefusalKind::Uncarried.as_str();

/// What a member route refusal is. Its [`Self::as_str`] is the refusal's
/// `error.data.kind` on `meta/init` and on the methods a member auth binding
/// refuses afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberRouteRefusalKind {
    /// `native_realm` beside `meerkat_config_path`.
    ConfigConflict,
    /// `native_realm` without `member_auth_binding`.
    BindingRequired,
    /// `native_realm` is not a closed object of a realm id and three
    /// absolute roots.
    RealmInvalid,
    /// The state root holds no realm of that id.
    RealmUnknown,
    /// The realm's storage cannot be resolved at the state root, or, under a
    /// member auth binding, the launch's state cannot be read to check it
    /// (a realm profile, the adopted declarations, a stored child mob).
    RealmUnreadable,
    /// The realm's documents cannot be read, parsed or composed, or the
    /// composed configuration does not validate.
    ConfigInvalid,
    /// `member_auth_binding` is not a closed object of valid ids.
    BindingInvalid,
    /// `member_auth_binding.origin` other than `"configured"`.
    OriginNotConfigured,
    /// The binding does not resolve in the members' configuration.
    BindingUnknown,
    /// A definition profile's model or provider is not one the binding
    /// serves.
    TargetMismatch,
    /// A path that would build a member without the binding: an automatic
    /// spawn policy, a model fallback target that names another binding or
    /// none, an identity declaration Core materializes on its own, or a
    /// member build that runs off the binding or would make provider calls
    /// off it.
    Uncarried,
    /// A request that would run a member on another binding than the
    /// launch's: a spawn that names one, or an attach of a session whose live
    /// actor runs one.
    Conflict,
}

impl MemberRouteRefusalKind {
    /// The refusal's `error.data.kind`.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ConfigConflict => "native_realm_config_conflict",
            Self::BindingRequired => "member_auth_binding_required",
            Self::RealmInvalid => "native_realm_invalid",
            Self::RealmUnknown => "native_realm_unknown",
            Self::RealmUnreadable => "native_realm_unreadable",
            Self::ConfigInvalid => "native_realm_config_invalid",
            Self::BindingInvalid => "member_auth_binding_invalid",
            Self::OriginNotConfigured => "member_auth_binding_origin",
            Self::BindingUnknown => "member_auth_binding_unknown",
            Self::TargetMismatch => "member_auth_binding_mismatch",
            Self::Uncarried => "member_auth_binding_uncarried",
            Self::Conflict => "member_auth_binding_conflict",
        }
    }

    /// The JSON-RPC code: the storage resolution code for a realm the state
    /// root does not resolve and for state that cannot be read, invalid
    /// params for every other refusal (a realm document that does not read or
    /// validate included, as for the `meerkat_config_path` file).
    pub(crate) const fn code(self) -> i64 {
        match self {
            Self::RealmUnknown | Self::RealmUnreadable => crate::STORAGE_RESOLUTION_CODE,
            Self::ConfigConflict
            | Self::BindingRequired
            | Self::RealmInvalid
            | Self::ConfigInvalid
            | Self::BindingInvalid
            | Self::OriginNotConfigured
            | Self::BindingUnknown
            | Self::TargetMismatch
            | Self::Uncarried
            | Self::Conflict => INVALID_PARAMS,
        }
    }
}

/// A typed `meta/init` refusal of the members' native realm or auth binding:
/// its kind, the message, and `error.data` (`kind` plus the refusal's facts).
/// The gateway refuses init with it before any agent factory is built, and
/// never falls back to the default configuration or the default binding.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MemberRouteRefusal {
    kind: MemberRouteRefusalKind,
    message: String,
    data: Value,
}

impl MemberRouteRefusal {
    fn new(kind: MemberRouteRefusalKind, message: String, facts: Map<String, Value>) -> Self {
        let mut data = facts;
        data.insert("kind".to_string(), Value::from(kind.as_str()));
        Self {
            kind,
            message,
            data: Value::Object(data),
        }
    }

    fn with_reason(kind: MemberRouteRefusalKind, message: String, reason: String) -> Self {
        let mut facts = Map::new();
        facts.insert("reason".to_string(), Value::from(reason));
        Self::new(kind, message, facts)
    }

    fn config_conflict() -> Self {
        Self::new(
            MemberRouteRefusalKind::ConfigConflict,
            format!(
                "runtime_options.{NATIVE_REALM_KEY} and runtime_options.{MEERKAT_CONFIG_PATH_KEY} \
                 are exclusive: the members' configuration is the native realm's or the named \
                 file's, never a merge of both. Remove one of them"
            ),
            Map::new(),
        )
    }

    fn binding_required() -> Self {
        Self::new(
            MemberRouteRefusalKind::BindingRequired,
            format!(
                "runtime_options.{NATIVE_REALM_KEY} needs runtime_options.\
                 {MEMBER_AUTH_BINDING_KEY}: a member's default binding selection starts at its \
                 own mob realm and never reaches the native realm's bindings, so name the \
                 binding the members build with"
            ),
            Map::new(),
        )
    }

    fn realm_invalid(reason: String) -> Self {
        Self::with_reason(
            MemberRouteRefusalKind::RealmInvalid,
            format!("runtime_options.{NATIVE_REALM_KEY} is invalid: {reason}"),
            reason,
        )
    }

    fn realm_unknown(realm_id: &str, state_root: &Path) -> Self {
        let mut facts = Map::new();
        facts.insert("realm_id".to_string(), Value::from(realm_id));
        facts.insert(
            "state_root".to_string(),
            Value::from(state_root.display().to_string()),
        );
        Self::new(
            MemberRouteRefusalKind::RealmUnknown,
            format!(
                "runtime_options.{NATIVE_REALM_KEY} names realm '{realm_id}', which does not \
                 exist under the state root {}",
                state_root.display()
            ),
            facts,
        )
    }

    fn realm_unreadable(realm_id: &str, reason: String) -> Self {
        let mut facts = Map::new();
        facts.insert("realm_id".to_string(), Value::from(realm_id));
        facts.insert("reason".to_string(), Value::from(reason.as_str()));
        Self::new(
            MemberRouteRefusalKind::RealmUnreadable,
            format!(
                "runtime_options.{NATIVE_REALM_KEY} realm '{realm_id}' cannot be resolved at its \
                 state root: {reason}"
            ),
            facts,
        )
    }

    fn config_invalid(realm_id: &str, reason: String) -> Self {
        let mut facts = Map::new();
        facts.insert("realm_id".to_string(), Value::from(realm_id));
        facts.insert("reason".to_string(), Value::from(reason.as_str()));
        Self::new(
            MemberRouteRefusalKind::ConfigInvalid,
            format!(
                "runtime_options.{NATIVE_REALM_KEY} realm '{realm_id}' has no usable \
                 configuration: {reason}"
            ),
            facts,
        )
    }

    fn binding_invalid(reason: String) -> Self {
        Self::with_reason(
            MemberRouteRefusalKind::BindingInvalid,
            format!("runtime_options.{MEMBER_AUTH_BINDING_KEY} is invalid: {reason}"),
            reason,
        )
    }

    fn origin_not_configured(origin: &str) -> Self {
        let mut facts = Map::new();
        facts.insert("origin".to_string(), Value::from(origin));
        Self::new(
            MemberRouteRefusalKind::OriginNotConfigured,
            format!(
                "runtime_options.{MEMBER_AUTH_BINDING_KEY}.origin is '{origin}', and only \
                 '{CONFIGURED_ORIGIN}' is accepted: the members build with a configured binding \
                 of their configuration"
            ),
            facts,
        )
    }

    fn binding_unknown(binding: &meerkat_core::AuthBindingRef, reason: String) -> Self {
        let mut facts = binding_facts(binding);
        facts.insert("reason".to_string(), Value::from(reason.as_str()));
        Self::new(
            MemberRouteRefusalKind::BindingUnknown,
            format!(
                "runtime_options.{MEMBER_AUTH_BINDING_KEY} realm '{}' binding '{}' does not \
                 resolve in the members' configuration: {reason}",
                binding.realm, binding.binding
            ),
            facts,
        )
    }

    fn target_mismatch(
        binding: &meerkat_core::AuthBindingRef,
        binding_provider: meerkat_core::Provider,
        profile: Option<(&str, &str)>,
        reason: String,
    ) -> Self {
        let mut facts = binding_facts(binding);
        facts.insert(
            "binding_provider".to_string(),
            Value::from(binding_provider.as_str()),
        );
        if let Some((name, model)) = profile {
            facts.insert("mob_profile".to_string(), Value::from(name));
            facts.insert("model".to_string(), Value::from(model));
        }
        facts.insert("reason".to_string(), Value::from(reason.as_str()));
        let subject = profile.map_or_else(String::new, |(name, model)| {
            format!(" for profile '{name}' (model '{model}')")
        });
        Self::new(
            MemberRouteRefusalKind::TargetMismatch,
            format!(
                "runtime_options.{MEMBER_AUTH_BINDING_KEY} realm '{}' binding '{}' (provider \
                 '{}') does not match{subject}: {reason}",
                binding.realm,
                binding.binding,
                binding_provider.as_str()
            ),
            facts,
        )
    }

    fn uncarried(reason: String) -> Self {
        Self::with_reason(
            MemberRouteRefusalKind::Uncarried,
            format!(
                "runtime_options.{MEMBER_AUTH_BINDING_KEY} cannot reach every member this \
                 launch builds: {reason}"
            ),
            reason,
        )
    }

    /// The refusal of a member declaration mutation
    /// (`mob/adopt_member_identity_declaration`,
    /// `mob/apply_member_tool_declaration`) on a launch with a member auth
    /// binding: Core materializes a declared identity from its sealed
    /// declaration without the spawn customizers, so the identity would
    /// build with the binding the declaration sealed, or with none, whatever
    /// binding the launch names.
    pub(crate) fn declaration_uncarried(method: &str) -> Self {
        Self::uncarried(format!(
            "{method} has Meerkat materialize the identity from its sealed declaration \
             without the gateway's spawn customizers, so it would not build with this \
             launch's binding; use a launch without runtime_options.member_auth_binding"
        ))
    }

    /// The refusal of agent memory engines (`agent_memory.distiller`,
    /// `agent_memory.steward`) on a launch with a member auth binding: they
    /// build their model clients from the default configuration with no
    /// binding, never from the members' configuration and binding.
    pub(crate) fn memory_engines_uncarried() -> Self {
        Self::uncarried(
            "agent_memory.distiller and agent_memory.steward build their model clients from \
             the default configuration with Meerkat's default binding selection, not from \
             the members' configuration and binding; disable them or the binding"
                .to_string(),
        )
    }

    /// A read of the launch's own state that init's binding checks need (the
    /// mob storage's realm profiles, its identity declarations, a stored child
    /// mob) failed: a storage fault, not a caller error, so it answers with
    /// the storage code, as `native_realm_unreadable`.
    pub(crate) fn state_unreadable(reason: String) -> Self {
        Self::with_reason(
            MemberRouteRefusalKind::RealmUnreadable,
            format!(
                "the launch's state cannot be read to check it against \
                 runtime_options.{MEMBER_AUTH_BINDING_KEY}: {reason}"
            ),
            reason,
        )
    }

    /// The refusal of `method`'s attach of `session_id` (as a member's
    /// session) under a member auth binding, for `reason`. `retryable` marks
    /// a refusal for a read that failed, not for a binding that differs: its
    /// data then carries an advisory `retryable: true`, as the same call can
    /// succeed once the read does.
    fn attach_conflict(
        method: &str,
        session_id: &meerkat_core::types::SessionId,
        reason: String,
        retryable: bool,
    ) -> Self {
        let mut facts = Map::new();
        facts.insert(
            "session_id".to_string(),
            Value::from(session_id.to_string()),
        );
        facts.insert("reason".to_string(), Value::from(reason.as_str()));
        if retryable {
            facts.insert("retryable".to_string(), Value::Bool(true));
        }
        Self::new(
            MemberRouteRefusalKind::Conflict,
            format!(
                "{method} cannot attach session {session_id} under \
                 runtime_options.{MEMBER_AUTH_BINDING_KEY}: {reason}; Meerkat reuses a \
                 session's live actor as it is, so the member would not run on the launch \
                 binding"
            ),
            facts,
        )
    }

    /// The refusal as a JSON-RPC error: its code, message and data.
    pub(crate) fn json_rpc_error(&self) -> crate::rpc::JsonRpcError {
        crate::rpc::JsonRpcError {
            code: self.code(),
            message: self.message.clone(),
            data: Some(self.data.clone()),
        }
    }

    pub(crate) fn code(&self) -> i64 {
        self.kind.code()
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    /// `error.data`: `kind` plus the refusal's facts.
    pub(crate) fn data(&self) -> &Value {
        &self.data
    }
}

impl std::fmt::Display for MemberRouteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MemberRouteRefusal {}

/// The binding's own facts in a refusal's data: `realm`, `binding` and, when
/// the binding names one, its auth profile as `auth_profile`.
fn binding_facts(binding: &meerkat_core::AuthBindingRef) -> Map<String, Value> {
    let mut facts = Map::new();
    facts.insert("realm".to_string(), Value::from(binding.realm.as_str()));
    facts.insert("binding".to_string(), Value::from(binding.binding.as_str()));
    if let Some(profile) = binding.profile.as_ref() {
        facts.insert("auth_profile".to_string(), Value::from(profile.as_str()));
    }
    facts
}

/// A binding as a refusal message names it.
fn describe_binding(binding: &meerkat_core::AuthBindingRef) -> String {
    let profile = binding
        .profile
        .as_ref()
        .map_or_else(String::new, |profile| format!(" profile '{profile}'"));
    format!(
        "realm '{}' binding '{}'{profile}",
        binding.realm, binding.binding
    )
}

/// Why a spawn that names `named` may not build under `launch`: the launch
/// binding is forced, so any other binding (another realm, binding or auth
/// profile) is refused, and the identical one is accepted, as named or in
/// `recorded_form`, the form Core records it in.
pub(crate) fn binding_conflict(
    named: &meerkat_core::AuthBindingRef,
    launch: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    (named != launch && recorded_form != Some(named)).then(|| {
        format!(
            "{BINDING_CONFLICT_KIND}: the spawn names auth binding {}, and every member of \
             this launch builds with {} (runtime_options.member_auth_binding)",
            describe_binding(named),
            describe_binding(launch)
        )
    })
}

/// Why enabled image generation would build off the launch binding, if it is
/// enabled: Core builds an image generation executor for each image provider
/// other than the member's own by default selection in the member's realm,
/// never with the member's binding.
fn image_generation_escape(enabled: bool) -> Option<String> {
    enabled.then(|| {
        "tools.image_generation: Meerkat builds an image generation executor for each image \
         provider other than the member's own by default binding selection, never with the \
         launch binding; disable image generation"
            .to_string()
    })
}

/// Why a member profile would make provider calls off the launch binding,
/// if it would: an enabled fallback target off the binding, or enabled
/// image generation.
fn profile_escape(
    profile: &meerkat_mob::Profile,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    fallback_escape(profile.model_fallback.as_ref(), binding, recorded_form)
        .map(|reason| format!("model_fallback: {reason}"))
        .or_else(|| image_generation_escape(profile.tools.image_generation))
}

/// Why an enabled model fallback `table` would build a member off `binding`,
/// if it would: its first target that names another binding, or none, which
/// Core resolves by default selection and never to the launch binding. A
/// target naming the launch binding, as named or in `recorded_form` (the form
/// Core records it in), is on it. A table that is absent or not enabled
/// builds nothing.
fn fallback_escape(
    table: Option<&meerkat_core::config::ModelFallbackConfig>,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    let table = table.filter(|table| table.is_enabled())?;
    let target = table.chain.iter().find(|target| {
        let named = target.auth_binding.as_ref();
        named != Some(binding) && (named.is_none() || named != recorded_form)
    })?;
    Some(match target.auth_binding.as_ref() {
        None => format!(
            "fallback target '{}' names no auth_binding, so Meerkat would select a default \
             binding for it",
            target.model
        ),
        Some(other) => format!(
            "fallback target '{}' names {}, not {}",
            target.model,
            describe_binding(other),
            describe_binding(binding)
        ),
    })
}

/// `runtime_options.native_realm`: the native realm whose effective
/// configuration the launch's members build from. Every field is required,
/// and no other is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativeRealmSelection {
    /// The realm's id: its directory name under `state_root`, by Core's realm
    /// id rule.
    pub(crate) realm_id: String,
    /// The native host's context root: Core's invocation context, never
    /// walked for the realm.
    pub(crate) context_root: PathBuf,
    /// The realms root the realm lives under, used as given.
    pub(crate) state_root: PathBuf,
    /// The home-like root whose `.rkat/config.toml` is the `global` realm's
    /// document.
    pub(crate) user_config_root: PathBuf,
}

impl NativeRealmSelection {
    /// Parse the option's value, refused as `native_realm_invalid`.
    pub(crate) fn parse(value: &Value) -> Result<Self, MemberRouteRefusal> {
        let selection: Self = serde_json::from_value(value.clone()).map_err(|error| {
            MemberRouteRefusal::realm_invalid(format!(
                "it must be an object {{realm_id, context_root, state_root, user_config_root}}: \
                 {error}"
            ))
        })?;
        meerkat_core::runtime_bootstrap::validate_explicit_realm_id(&selection.realm_id)
            .map_err(|error| MemberRouteRefusal::realm_invalid(format!("realm_id: {error}")))?;
        for (field, root) in [
            ("context_root", &selection.context_root),
            ("state_root", &selection.state_root),
            ("user_config_root", &selection.user_config_root),
        ] {
            if !root.is_absolute() {
                return Err(MemberRouteRefusal::realm_invalid(format!(
                    "{field} must be an absolute path, got '{}'",
                    root.display()
                )));
            }
        }
        Ok(selection)
    }
}

/// The wire form of `runtime_options.member_auth_binding`: Core's
/// `AuthBindingRef` as a closed object. The ids are Core's typed atoms, so
/// serde applies Core's slug rule to each.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberAuthBindingWire {
    realm: meerkat_core::RealmId,
    binding: meerkat_core::BindingId,
    #[serde(default)]
    profile: Option<meerkat_core::ProfileId>,
    #[serde(default)]
    origin: Option<String>,
}

/// Parse `runtime_options.member_auth_binding` into Core's typed binding.
/// `origin` may be absent or `"configured"`, which the binding carries as
/// [`meerkat_core::BindingOrigin::Configured`]; any other value is refused,
/// never dropped and never read as another origin.
pub(crate) fn parse_member_auth_binding(
    value: &Value,
) -> Result<meerkat_core::AuthBindingRef, MemberRouteRefusal> {
    let wire: MemberAuthBindingWire = serde_json::from_value(value.clone()).map_err(|error| {
        MemberRouteRefusal::binding_invalid(format!(
            "it must be an object {{realm, binding, profile?, origin?}}: {error}"
        ))
    })?;
    if let Some(origin) = wire.origin.as_deref()
        && origin != CONFIGURED_ORIGIN
    {
        return Err(MemberRouteRefusal::origin_not_configured(origin));
    }
    Ok(meerkat_core::AuthBindingRef {
        realm: wire.realm,
        binding: wire.binding,
        profile: wire.profile,
        origin: meerkat_core::BindingOrigin::Configured,
    })
}

/// A launch's member route: the native realm its members read their
/// configuration from and the binding they build with, as `meta/init`
/// declared them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MemberRoute {
    pub(crate) native_realm: Option<NativeRealmSelection>,
    pub(crate) member_auth_binding: Option<meerkat_core::AuthBindingRef>,
}

impl MemberRoute {
    /// Parse both options from the init params. A `runtime_options` that is
    /// not an object is left to the general option parse, which refuses it.
    pub(crate) fn from_init_params(params: &Value) -> Result<Self, MemberRouteRefusal> {
        match params.get("runtime_options").and_then(Value::as_object) {
            Some(options) => Self::from_runtime_options(options),
            None => Ok(Self::default()),
        }
    }

    /// Parse both options and check them against each other and against
    /// `meerkat_config_path`.
    pub(crate) fn from_runtime_options(
        options: &Map<String, Value>,
    ) -> Result<Self, MemberRouteRefusal> {
        if options.contains_key(NATIVE_REALM_KEY) && options.contains_key(MEERKAT_CONFIG_PATH_KEY) {
            return Err(MemberRouteRefusal::config_conflict());
        }
        let native_realm = options
            .get(NATIVE_REALM_KEY)
            .map(NativeRealmSelection::parse)
            .transpose()?;
        let member_auth_binding = options
            .get(MEMBER_AUTH_BINDING_KEY)
            .map(parse_member_auth_binding)
            .transpose()?;
        if native_realm.is_some() && member_auth_binding.is_none() {
            return Err(MemberRouteRefusal::binding_required());
        }
        Ok(Self {
            native_realm,
            member_auth_binding,
        })
    }
}

/// Read the native realm's effective configuration with the calls a native
/// host makes, read only: Core's storage layout resolves the realm at the
/// explicit state root (which wins over every probe, so `context_root`
/// selects no document), Core's own realm probe checks that the realm exists
/// there (the reserved `global` realm keeps no manifest: its document is the
/// user document), the store's filesystem source maps the realm and its
/// chain to their documents, Core's effective reader composes them, and the
/// result takes Core's environment overrides and `Config::validate`, as a
/// native host's does. A realm the state root cannot resolve is a storage
/// refusal; documents that do not read, compose or validate are refused as
/// invalid params, as a `meerkat_config_path` file is.
pub(crate) async fn read_native_realm_config(
    selection: &NativeRealmSelection,
) -> Result<meerkat_core::Config, MemberRouteRefusal> {
    let unreadable =
        |reason: String| MemberRouteRefusal::realm_unreadable(&selection.realm_id, reason);
    let invalid = |reason: String| MemberRouteRefusal::config_invalid(&selection.realm_id, reason);
    let storage = meerkat_core::StorageLayout::resolve(
        meerkat_core::StorageLayoutInputs {
            invocation_context: selection.context_root.clone(),
            explicit_state_root: Some(selection.state_root.clone()),
            user_config_root: Some(selection.user_config_root.clone()),
            default_root: Some(meerkat_core::RealmRootDefault::UserGlobal),
            probe_local_candidate: true,
        },
        &meerkat_core::RealmConfig {
            selection: meerkat_core::RealmSelection::Explicit {
                realm_id: selection.realm_id.clone(),
            },
            ..meerkat_core::RealmConfig::default()
        },
    )
    .map_err(|error| unreadable(error.to_string()))?;
    let realm = &storage.locator.realm;
    let state_root = &storage.locator.state_root;
    if !realm.is_global() {
        match meerkat_core::realm_exists_under(state_root, realm) {
            Ok(true) => {}
            Ok(false) => {
                return Err(MemberRouteRefusal::realm_unknown(
                    &selection.realm_id,
                    state_root,
                ));
            }
            Err(error) => return Err(unreadable(error.to_string())),
        }
    }
    let user_document = storage.layout.global_config_path().ok_or_else(|| {
        unreadable("the storage layout resolved no user configuration root".to_string())
    })?;
    let source = Arc::new(meerkat_store::FilesystemRealmConfigSource::new(
        state_root,
        user_document,
        meerkat_models::canonical(),
    ));
    let mut config = meerkat_core::EffectiveConfigReader::new(source)
        .effective_config(realm)
        .await
        .map_err(|error| invalid(error.to_string()))?;
    config
        .apply_env_overrides()
        .map_err(|error| invalid(error.to_string()))?;
    config
        .validate(meerkat_models::canonical())
        .map_err(|error| invalid(error.to_string()))?;
    Ok(config)
}

/// What init's profile checks resolve first: the binding's target, whose
/// backend names the provider the binding serves, and the members' model
/// registry, with the definition's `[models.<id>]`.
fn binding_target(
    config: &meerkat_core::Config,
    binding: &meerkat_core::AuthBindingRef,
    definition: &meerkat_mob::MobDefinition,
) -> Result<
    (
        meerkat_core::ResolvedConnectionTarget,
        meerkat_core::ModelRegistry,
    ),
    MemberRouteRefusal,
> {
    let target = meerkat_core::resolve_explicit_auth_binding_target(config, binding)
        .map_err(|error| MemberRouteRefusal::binding_unknown(binding, error.to_string()))?;
    let registry = meerkat_core::ModelRegistry::from_config_with_models(
        config,
        &definition.models,
        meerkat_models::canonical(),
    )
    .map_err(|error| {
        MemberRouteRefusal::target_mismatch(
            binding,
            target.backend.provider,
            None,
            format!("the members' model registry cannot be built: {error}"),
        )
    })?;
    Ok((target, registry))
}

/// Init's checks of one definition profile `name`, inline or resolved from
/// the realm profile it references (`realm_profile`): its provider and model
/// are ones the binding serves (under a self-hosted binding, registered with
/// the binding's server), and it does not enable image generation.
fn check_definition_profile(
    config: &meerkat_core::Config,
    binding: &meerkat_core::AuthBindingRef,
    target: &meerkat_core::ResolvedConnectionTarget,
    registry: &meerkat_core::ModelRegistry,
    name: &str,
    realm_profile: Option<&str>,
    profile: &meerkat_mob::Profile,
) -> Result<(), MemberRouteRefusal> {
    let binding_provider = target.backend.provider;
    let model = profile.model.as_str();
    let subject = Some((name, model));
    let via = realm_profile.map_or_else(String::new, |realm_profile| {
        format!("realm profile '{realm_profile}': ")
    });
    let mismatch = |reason: String| {
        MemberRouteRefusal::target_mismatch(
            binding,
            binding_provider,
            subject,
            format!("{via}{reason}"),
        )
    };
    let profile_provider = profile
        .provider
        .or_else(|| registry.entry(model).map(|entry| entry.provider));
    match profile_provider {
        Some(provider) if provider == binding_provider => {}
        Some(provider) => {
            return Err(mismatch(format!(
                "the profile's provider is '{}'",
                provider.as_str()
            )));
        }
        None => {
            return Err(mismatch(
                "the profile names no provider and the members' model registry does not \
                 know the model"
                    .to_string(),
            ));
        }
    }
    if let Some(reason) = registry.provider_override_mismatch_reason(binding_provider, model) {
        return Err(mismatch(reason));
    }
    if binding_provider == meerkat_core::Provider::SelfHosted {
        let Some(registered_server) = registry
            .entry_for_provider(meerkat_core::Provider::SelfHosted, model)
            .and_then(|entry| entry.self_hosted.as_ref())
            .map(|server| server.server_id.clone())
        else {
            return Err(mismatch(format!(
                "self-hosted model '{model}' is not registered with a server in the \
                 members' configuration"
            )));
        };
        let server = profile
            .self_hosted_server_id
            .clone()
            .unwrap_or(registered_server);
        meerkat_core::validate_explicit_self_hosted_target(config, target, &server, model)
            .map_err(|error| mismatch(error.to_string()))?;
    }
    if let Some(reason) = image_generation_escape(profile.tools.image_generation) {
        let place = realm_profile.map_or_else(
            || format!("profiles.{name}"),
            |realm_profile| format!("profiles.{name} (realm profile '{realm_profile}')"),
        );
        return Err(MemberRouteRefusal::uncarried(format!("{place}.{reason}")));
    }
    Ok(())
}

/// Check the members' binding at init, against the members' configuration
/// and the mob definition, so a refusal answers before any factory is built:
///
/// - the binding resolves in `config` by Core's explicit resolution: its
///   realm is in the configuration and the binding is defined on that
///   realm's chain;
/// - every inline profile of `definition` resolves to the provider the
///   binding's backend serves (the profile's own provider, else the one the
///   members' model registry, with the definition's `[models.<id>]`, owns
///   the model under) and the model is not registered under another
///   provider; under a self-hosted binding the model is registered with a
///   server, as Core's build requires, and that server is the one the
///   binding's backend declares. A realm-referenced profile is checked the
///   same way, resolved from the mob storage's realm profiles
///   ([`validate_realm_profiles`], once the storage is open);
/// - no path builds a member off the binding: the definition declares no
///   automatic spawn policy, whose members Core builds without the gateway's
///   spawn customizers, and no enabled model fallback table (the members'
///   configuration, the definition's `[runtime.model_fallback]`, an inline
///   profile's `model_fallback`) has a target that names another binding or
///   none, which Core would resolve by default selection;
/// - no inline profile enables image generation, whose executors for the
///   image providers other than the member's own Core builds by default
///   selection.
///
/// Returns the binding in the form Core records on a member's session: the
/// resolution's own, stamped with the realm that owns the binding (the named
/// realm, a parent, or `global`), which a binding named under one realm and
/// defined on another differs from.
pub(crate) fn validate_member_auth_binding(
    config: &meerkat_core::Config,
    binding: &meerkat_core::AuthBindingRef,
    definition: &meerkat_mob::MobDefinition,
) -> Result<meerkat_core::AuthBindingRef, MemberRouteRefusal> {
    let (target, registry) = binding_target(config, binding, definition)?;
    for (name, profile) in &definition.profiles {
        let Some(profile) = profile.as_inline() else {
            continue;
        };
        check_definition_profile(
            config,
            binding,
            &target,
            &registry,
            name.as_str(),
            None,
            profile,
        )?;
    }
    if matches!(
        definition.spawn_policy,
        Some(meerkat_mob::definition::SpawnPolicyConfig::Auto { .. })
    ) {
        return Err(MemberRouteRefusal::uncarried(
            "the definition's automatic spawn policy creates members that Core builds without \
             the gateway's spawn customizers, so they would not carry the binding; remove \
             [spawn_policy] or the binding"
                .to_string(),
        ));
    }
    let fallback_tables = [
        (
            "the members' configuration [model_fallback]".to_string(),
            Some(&config.model_fallback),
        ),
        (
            "[runtime.model_fallback]".to_string(),
            definition.runtime.model_fallback.as_ref(),
        ),
    ]
    .into_iter()
    .chain(definition.profiles.iter().filter_map(|(name, profile)| {
        profile.as_inline().map(|profile| {
            (
                format!("profiles.{name}.model_fallback"),
                profile.model_fallback.as_ref(),
            )
        })
    }));
    for (place, table) in fallback_tables {
        if let Some(reason) = fallback_escape(table, binding, Some(&target.auth_binding)) {
            return Err(MemberRouteRefusal::uncarried(format!(
                "{place}: {reason}; name {} on every fallback target, or disable the fallback",
                describe_binding(binding)
            )));
        }
    }
    Ok(target.auth_binding)
}

/// Why a public profile input (an inline profile in an agent's `mob_create`
/// definition or `tooling` override) would make provider calls off the
/// launch binding, if it would: an enabled fallback target off the binding,
/// or enabled image generation.
fn profile_input_escape(
    profile: &meerkat_contracts::wire::MobProfileInput,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    fallback_escape(profile.model_fallback.as_ref(), binding, recorded_form)
        .map(|reason| format!("model_fallback: {reason}"))
        .or_else(|| image_generation_escape(profile.tools.image_generation))
}

/// Init's checks of the realm-referenced profiles of `definition`
/// (`realm_profile = "..."`), each resolved from `realm_profiles`, the store
/// the members' builds resolve it from: an inline profile's checks
/// ([`check_definition_profile`]), and every target of the realm profile's
/// own model fallback chain on the binding. A realm profile the store does
/// not hold is left to Meerkat, whose build of the member refuses it; one the
/// store cannot read is refused.
pub(crate) async fn validate_realm_profiles(
    config: &meerkat_core::Config,
    binding: &meerkat_core::AuthBindingRef,
    definition: &meerkat_mob::MobDefinition,
    realm_profiles: &dyn meerkat_mob::RealmProfileStore,
) -> Result<(), MemberRouteRefusal> {
    let references: Vec<(String, String)> = definition
        .profiles
        .iter()
        .filter_map(|(name, profile)| {
            if let meerkat_mob::ProfileBinding::RealmRef { realm_profile } = profile {
                Some((name.to_string(), realm_profile.clone()))
            } else {
                None
            }
        })
        .collect();
    if references.is_empty() {
        return Ok(());
    }
    let (target, registry) = binding_target(config, binding, definition)?;
    for (name, realm_profile) in references {
        let profile = match realm_profiles.get(&realm_profile).await {
            Ok(Some(stored)) => stored.profile,
            Ok(None) => continue,
            Err(error) => {
                return Err(MemberRouteRefusal::state_unreadable(format!(
                    "profiles.{name} references realm profile '{realm_profile}', which the \
                     mob storage cannot read: {error}"
                )));
            }
        };
        check_definition_profile(
            config,
            binding,
            &target,
            &registry,
            &name,
            Some(&realm_profile),
            &profile,
        )?;
        if let Some(reason) = fallback_escape(
            profile.model_fallback.as_ref(),
            binding,
            Some(&target.auth_binding),
        ) {
            return Err(MemberRouteRefusal::uncarried(format!(
                "profiles.{name} (realm profile '{realm_profile}').model_fallback: {reason}; \
                 name {} on every fallback target, or disable the fallback",
                describe_binding(binding)
            )));
        }
    }
    Ok(())
}

/// Why a stored mob definition would build members off the launch binding,
/// if it would: the checks `mob_create` runs on a new child mob
/// ([`child_mob_definition_escape`]), over Meerkat's own definition type. An
/// automatic spawn policy, an enabled `runtime.model_fallback` with a target
/// off the binding, or an inline profile whose fallback leaves the binding or
/// that enables image generation.
pub(crate) fn stored_definition_escape(
    definition: &meerkat_mob::MobDefinition,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    if matches!(
        definition.spawn_policy,
        Some(meerkat_mob::definition::SpawnPolicyConfig::Auto { .. })
    ) {
        return Some(
            "its automatic spawn_policy creates members Meerkat builds without the spawn \
             customizers, so they would not build with the launch's member auth binding"
                .to_string(),
        );
    }
    if let Some(reason) = fallback_escape(
        definition.runtime.model_fallback.as_ref(),
        binding,
        recorded_form,
    ) {
        return Some(format!("runtime.model_fallback: {reason}"));
    }
    definition.profiles.iter().find_map(|(name, profile)| {
        profile
            .as_inline()
            .and_then(|profile| profile_escape(profile, binding, recorded_form))
            .map(|reason| format!("profiles.{name}.{reason}"))
    })
}

/// Whether `database` is a child mob's own event log in the child root (Meerkat
/// names each `mob--<escaped id>.db`; the realm profile and custody databases
/// beside them are not mobs).
fn is_child_mob_event_log(database: &Path) -> bool {
    matches!(
        crate::child_mob_storage::ChildMobDatabase::classify(database),
        crate::child_mob_storage::ChildMobDatabase::MobEventLog
    ) && database
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("mob--"))
}

/// Init's check of the durable child mobs stored under `state_dir`, before
/// anything restores them: each child's stored definition, read without
/// building it, must pass [`stored_definition_escape`], since a restore
/// rebuilds a child with the definition it was created with. A child that an
/// agent created before the launch named the binding, and that would build
/// members off it, refuses the launch (`member_auth_binding_uncarried`, naming
/// it). A child whose destroy is pending is skipped: Meerkat only finishes
/// destroying it and rebuilds none of its members. A child Meerkat keeps from
/// running for another reason (its owning session archived, which Meerkat
/// tears it down for, or not observable, which Meerkat holds it for) is still
/// read and refused when it is off the binding: fail closed, settled by one
/// launch without the binding. A realm-referenced profile of a child resolves
/// at each member's build, where [`MemberBuildGuard`] refuses one off the
/// binding. A child that cannot be read refuses the launch with the storage
/// code.
pub(crate) async fn validate_stored_child_mobs(
    state_dir: &Path,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Result<(), MemberRouteRefusal> {
    let mut refused = Vec::new();
    for database in crate::child_mob_storage::child_mob_databases(state_dir) {
        if !is_child_mob_event_log(&database) {
            continue;
        }
        let unreadable = |error: String| {
            MemberRouteRefusal::state_unreadable(format!(
                "the stored child mob {} cannot be read: {error}",
                database.display()
            ))
        };
        let storage = meerkat_mob::MobStorage::persistent(&database)
            .map_err(|error| unreadable(error.to_string()))?;
        if storage
            .destroy_recovery_pending()
            .await
            .map_err(|error| unreadable(error.to_string()))?
        {
            continue;
        }
        let Some(definition) = storage
            .created_definition()
            .await
            .map_err(|error| unreadable(error.to_string()))?
        else {
            continue;
        };
        if let Some(reason) = stored_definition_escape(&definition, binding, recorded_form) {
            refused.push(format!("child mob '{}': {reason}", definition.id));
        }
    }
    if refused.is_empty() {
        return Ok(());
    }
    Err(MemberRouteRefusal::uncarried(format!(
        "{}; a stored child mob is restored with the definition it was created with, before \
         this launch named the binding: destroy it, or launch without \
         runtime_options.{MEMBER_AUTH_BINDING_KEY}",
        refused.join("; ")
    )))
}

/// Why the identity declarations `declarations` (each its identity, its
/// declaration scope, and the binding it sealed) hold members off the launch
/// binding, if they do: one sealed with no binding, or another than the launch
/// binding in either form (as named, or as `recorded_form`, the form Core
/// records it in).
fn declarations_off_binding(
    declarations: impl IntoIterator<
        Item = (String, Option<String>, Option<meerkat_core::AuthBindingRef>),
    >,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    let off: Vec<String> = declarations
        .into_iter()
        .filter(|(_, _, sealed)| {
            !sealed
                .as_ref()
                .is_some_and(|sealed| sealed == binding || recorded_form == Some(sealed))
        })
        .map(|(identity, scope, sealed)| {
            let scope = scope.map_or_else(String::new, |scope| format!(" (declaration '{scope}')"));
            let sealed = sealed
                .as_ref()
                .map_or_else(|| "no binding".to_string(), describe_binding);
            format!("identity '{identity}'{scope} sealed with {sealed}")
        })
        .collect();
    (!off.is_empty()).then(|| {
        format!(
            "the mob storage holds identity declarations adopted by an earlier launch, which \
             Meerkat materializes from the sealed declaration without the gateway's spawn \
             customizers, so not with {}: {}; launch without \
             runtime_options.{MEMBER_AUTH_BINDING_KEY} to retire them first",
            describe_binding(binding),
            off.join(", ")
        )
    })
}

/// Init's check of the identity declarations the launch's mob storage at
/// `event_log_db` holds for `mob_id`. They were adopted by an earlier launch,
/// since a launch with a member auth binding refuses adoption, and Meerkat
/// materializes a declared identity from its sealed declaration without the
/// gateway's spawn customizers (`SpawnSource::IdentityReconcile`), so one
/// sealed with no binding or another binding refuses the launch
/// (`member_auth_binding_uncarried`, naming the declarations). A declaration
/// Core cannot read, or one that declares the identity absent, materializes
/// nothing.
pub(crate) async fn validate_adopted_declarations(
    event_log_db: &Path,
    mob_id: &meerkat_mob::MobId,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Result<(), MemberRouteRefusal> {
    use meerkat_mob::MobIdentityStore as _;
    let unreadable = |error: String| {
        MemberRouteRefusal::state_unreadable(format!(
            "the identity declarations of the mob storage cannot be read: {error}"
        ))
    };
    let stores = meerkat_mob::store::SqliteMobStores::open(event_log_db)
        .map_err(|error| unreadable(error.to_string()))?;
    let intents = stores
        .identity_store()
        .list_identity_intents(mob_id)
        .await
        .map_err(|error| unreadable(error.to_string()))?;
    let declarations = intents.into_iter().filter_map(|(identity, observation)| {
        let meerkat_mob::identity::IdentityStoredObservation::Valid(record) = observation else {
            return None;
        };
        let meerkat_mob::identity::IdentityIntent::Present { member, .. } = record.intent else {
            return None;
        };
        Some((
            crate::member_comms_id::runtime_alias_str(identity.as_str()).into_owned(),
            record
                .declaration_scope
                .map(|scope| scope.as_str().to_string()),
            member
                .material
                .overlay
                .auth_binding
                .map(meerkat_core::AuthBindingRef::from),
        ))
    });
    match declarations_off_binding(declarations, binding, recorded_form) {
        Some(reason) => Err(MemberRouteRefusal::uncarried(reason)),
        None => Ok(()),
    }
}

/// What the gateway knows, where it builds members, of the launch's member auth
/// binding: the binding as named, the form Core records it in (stamped with
/// the realm that owns it), and, with the members' configuration, what
/// decides whether a member's web search is the provider's own search on the
/// member's binding or Meerkat's fallback executor on default selection.
///
/// The web search decision follows the phase Meerkat builds each member in,
/// read from the build itself as Meerkat's builder reads it
/// ([`built_through_surface_gate`]). With a host's member registration hook
/// installed, Meerkat's mob gives each member registration a tool-surface
/// gate, and Meerkat prepares each such member for the host's decision before
/// finishing it: an unmanaged application tool policy governs that
/// construction too, so the provider's native tools stay off for the
/// member's whole life and its web search is the fallback. Any other build is
/// completed in one step and judged on its model, backend and tool
/// governance ([`Self::escape`]).
pub(crate) struct MemberBuildGuard {
    binding: meerkat_core::AuthBindingRef,
    recorded_form: Option<meerkat_core::AuthBindingRef>,
    native: Option<NativeSearchFacts>,
}

/// The facts the factory decides native web search from that the guard can
/// read ahead of a build: the members' configuration, and whether the
/// binding's backend is a copilot backend, under which Meerkat disables the
/// provider's native tools. The model registry is each build's own: the
/// configuration's, with the build's `custom_models` (the `[models]` of the
/// member's own mob definition), as the factory builds it.
struct NativeSearchFacts {
    config: Arc<meerkat_core::Config>,
    copilot_backend: bool,
}

impl MemberBuildGuard {
    /// The guard of `binding`. `recorded_form` is the form init resolved;
    /// without it the guard resolves it from `config` as init does. Without a
    /// `config`, no web search is taken for the provider's own.
    pub(crate) fn new(
        binding: meerkat_core::AuthBindingRef,
        recorded_form: Option<meerkat_core::AuthBindingRef>,
        config: Option<Arc<meerkat_core::Config>>,
    ) -> Self {
        let target = config.as_deref().and_then(|config| {
            meerkat_core::resolve_explicit_auth_binding_target(config, &binding).ok()
        });
        let recorded_form =
            recorded_form.or_else(|| target.as_ref().map(|target| target.auth_binding.clone()));
        let native = config.map(|config| {
            let copilot_backend = target.as_ref().is_none_or(|target| {
                target
                    .backend
                    .backend_kind
                    .to_ascii_lowercase()
                    .contains("copilot")
            });
            NativeSearchFacts {
                config,
                copilot_backend,
            }
        });
        Self {
            binding,
            recorded_form,
            native,
        }
    }

    /// The form Core records the binding in, when known.
    pub(crate) fn recorded_form(&self) -> Option<&meerkat_core::AuthBindingRef> {
        self.recorded_form.as_ref()
    }

    /// Whether `binding` is the launch binding, as named or as recorded.
    fn is_launch_binding(&self, binding: &meerkat_core::AuthBindingRef) -> bool {
        *binding == self.binding || self.recorded_form.as_ref() == Some(binding)
    }

    /// Why `request`, a build through the gateway, would build a member off
    /// the launch binding or make provider calls off it, if it would:
    ///
    /// - a member build (one carrying a mob member binding) whose auth binding
    ///   is not the launch binding, as named or as recorded. Meerkat rebuilds
    ///   a member from its own records where no spawn customizer is asked (a
    ///   fork seated before the launch named the binding, say), and a build
    ///   with no binding resolves by default selection;
    /// - Meerkat's fallback web search: web search enabled by the build or,
    ///   when the build leaves it to the session, by the tooling a resumed
    ///   session recorded, where the provider's own search is not what serves
    ///   it. The fallback executor resolves its credentials by default
    ///   selection; the provider's own search runs on the member's binding
    ///   and passes;
    /// - image generation, whose executors for the other image providers
    ///   Meerkat builds by default selection;
    /// - a model fallback target off the binding.
    pub(crate) fn escape(
        &self,
        request: &meerkat_core::service::CreateSessionRequest,
    ) -> Option<String> {
        use meerkat_core::ToolCategoryOverride as Category;
        let build = request.build.as_ref()?;
        if build.mob_member_binding.is_some()
            && !build
                .auth_binding
                .as_ref()
                .is_some_and(|binding| self.is_launch_binding(binding))
        {
            let runs_on = build.auth_binding.as_ref().map_or_else(
                || "no auth binding, so Meerkat's default binding selection".to_string(),
                |binding| format!("auth binding {}", describe_binding(binding)),
            );
            return Some(format!(
                "runs on {runs_on}: Meerkat rebuilds a member from its own records where no \
                 spawn customizer is asked (a fork seated before this launch named the \
                 binding, say); re-create it"
            ));
        }
        // What a resume restores from the session's records where the build
        // leaves it open, as Meerkat's resume merge does.
        let recorded = build
            .resume_session
            .as_ref()
            .and_then(|session| session.try_session_metadata().ok().flatten());
        let web_search = match build.override_web_search {
            Category::Inherit if !build.resume_override_mask.override_web_search => recorded
                .as_ref()
                .map_or(Category::Inherit, |metadata| metadata.tooling.web_search),
            decided => decided,
        };
        if web_search == Category::Enable
            && !self.native_web_search(request, build, recorded.as_ref())
        {
            return Some(
                "enables web search (a resumed member by the tooling its session recorded) \
                 where the provider's own search does not serve it, so Meerkat's fallback web \
                 search executor would, which resolves its credentials by default binding \
                 selection, never with the launch binding"
                    .to_string(),
            );
        }
        if build.override_image_generation == Category::Enable {
            return image_generation_escape(true);
        }
        fallback_escape(
            build.model_fallback.as_ref(),
            &self.binding,
            self.recorded_form.as_ref(),
        )
        .map(|reason| format!("model_fallback: {reason}"))
    }

    /// Whether the build's web search is the provider's own, as the factory
    /// decides it: the identity the build runs on (the build's, merged with a
    /// resumed session's by Core's resume rule) supports native web search in
    /// the build's own model registry (the members' configuration with the
    /// build's `custom_models`), and nothing disables the provider's native
    /// tools. Meerkat disables them for a build it prepares for its host's
    /// tool-surface gate ([`built_through_surface_gate`]), whatever its
    /// application tool policy, on a copilot backend, and under tool
    /// governance: a dispatch admission, an application tool policy, or a
    /// restricted tool access policy (the declared restriction of the member's
    /// profile included), each as the build carries it or, where it leaves it
    /// open, as the resumed session recorded it. Whatever the guard cannot
    /// tell is taken as the fallback.
    fn native_web_search(
        &self,
        request: &meerkat_core::service::CreateSessionRequest,
        build: &meerkat_core::service::SessionBuildOptions,
        recorded: Option<&meerkat_core::SessionMetadata>,
    ) -> bool {
        // Meerkat's builder prepares such a build before it finishes it, and
        // a prepared construction is governed from the start, an unmanaged
        // application tool policy included, so the provider's native tools
        // stay off.
        if built_through_surface_gate(build) {
            return false;
        }
        let Some(native) = self.native.as_ref() else {
            return false;
        };
        // The backend facts are the launch binding's; a build on another
        // binding (or none) is not one the guard can tell about.
        if native.copilot_backend
            || !build
                .auth_binding
                .as_ref()
                .is_some_and(|binding| self.is_launch_binding(binding))
            || build.tool_dispatch_admission.is_some()
        {
            return false;
        }
        let mask = &build.resume_override_mask;
        let application_tool_policy = match recorded {
            Some(metadata) if !mask.application_tool_policy => {
                &metadata.tooling.application_tool_policy
            }
            _ => &build.application_tool_policy,
        };
        if !matches!(
            application_tool_policy,
            meerkat_core::ApplicationToolPolicyBinding::Unmanaged
        ) {
            return false;
        }
        if build
            .declared_tool_restriction
            .as_ref()
            .is_some_and(|restriction| !restriction.is_unrestricted())
        {
            return false;
        }
        let launch_policy = match (build.tool_access_policy.as_ref(), recorded) {
            (Some(policy), _) => Some(policy.clone()),
            (None, Some(metadata)) if !mask.tool_access_policy => {
                match metadata.tooling.spawn_tool_access_policy.clone() {
                    Some(spawn) => spawn.into_launch(),
                    None => metadata.tooling.tool_access_policy.clone(),
                }
            }
            (None, _) => None,
        };
        if launch_policy.is_some_and(|policy| {
            !meerkat_core::ToolExecutionPolicy::resolve(policy)
                .is_ok_and(|policy| policy.is_unrestricted())
        }) {
            return false;
        }
        // The registry the factory judges this build against: the members'
        // configuration with the build's own `custom_models`, whichever mob
        // definition (the root's, a child's, a candidate's) they come from.
        let Ok(registry) = meerkat_core::ModelRegistry::from_config_with_models(
            &native.config,
            &build.custom_models,
            meerkat_models::canonical(),
        ) else {
            return false;
        };
        // The identity the factory builds with: the build's, merged with a
        // resumed session's durable identity by Core's resume rule, and a
        // provider the build leaves open recomputed from the model.
        let identity = match recorded {
            Some(metadata) => {
                let durable = metadata.llm_identity();
                let mut merged = meerkat_core::resolve_resumed_session_llm_identity(
                    meerkat_core::SessionLlmIdentity {
                        model: request.model.clone(),
                        provider: build.provider.unwrap_or(durable.provider),
                        self_hosted_server_id: build.self_hosted_server_id.clone(),
                        provider_params: build.provider_params.clone(),
                        auth_binding: build.auth_binding.clone(),
                    },
                    &durable,
                    mask,
                );
                if mask.provider && build.provider.is_none() {
                    let Some(entry) = registry.entry(&merged.model) else {
                        return false;
                    };
                    merged.provider = entry.provider;
                }
                merged
            }
            None => {
                let Some(provider) = build
                    .provider
                    .or_else(|| registry.entry(&request.model).map(|entry| entry.provider))
                else {
                    return false;
                };
                meerkat_core::SessionLlmIdentity {
                    model: request.model.clone(),
                    provider,
                    self_hosted_server_id: build.self_hosted_server_id.clone(),
                    provider_params: build.provider_params.clone(),
                    auth_binding: build.auth_binding.clone(),
                }
            }
        };
        registry
            .profile_for_identity(&native.config, &identity)
            .is_some_and(|profile| profile.supports_web_search)
    }

    /// [`Self::escape`] as the typed refusal of the build:
    /// `member_auth_binding_uncarried`, as Meerkat's
    /// `SessionError::FailedWithData` with the kind and reason in its data.
    pub(crate) fn refusal(
        &self,
        request: &meerkat_core::service::CreateSessionRequest,
    ) -> Option<meerkat_core::service::SessionError> {
        self.escape(request).map(|reason| {
            let mut data = Map::new();
            data.insert("kind".to_string(), Value::from(BINDING_UNCARRIED_KIND));
            data.insert("reason".to_string(), Value::from(reason.as_str()));
            meerkat_core::service::SessionError::FailedWithData {
                message: format!(
                    "{BINDING_UNCARRIED_KIND}: this member's build {reason}; every member of \
                     this launch builds with {} (runtime_options.{MEMBER_AUTH_BINDING_KEY})",
                    describe_binding(&self.binding)
                ),
                data: Value::Object(data),
            }
        })
    }
}

/// Whether Meerkat builds `build` through its registration's tool-surface
/// gate: the build runs on a session runtime registration whose gate slot
/// holds a gate. With a host's member registration hook installed, Meerkat's
/// mob puts a gate on each member registration it provisions, before the
/// member's build reaches the session services, and Meerkat's builder reads
/// this same gate to prepare the construction for the host's decision and
/// finish it after, instead of completing it in one step. A build that runs
/// on no registration, or on one without a gate, is completed in one step.
fn built_through_surface_gate(build: &meerkat_core::service::SessionBuildOptions) -> bool {
    match &build.runtime_build_mode {
        meerkat_core::RuntimeBuildMode::SessionOwned(bindings) => {
            bindings.tool_surface_gate().is_some()
        }
        meerkat_core::RuntimeBuildMode::StandaloneEphemeral => false,
    }
}

/// The pre-build hook a launch with a member auth binding puts on its session
/// services: every build [`MemberBuildGuard::escape`] names is refused typed
/// ([`MemberBuildGuard::refusal`]) before anything is built.
pub(crate) fn member_build_hook(
    guard: Arc<MemberBuildGuard>,
) -> crate::mob_handle_runtime::PreBuildHook {
    Arc::new(move |request| {
        let refusal = guard.refusal(request);
        Box::pin(async move { refusal.map_or(Ok(()), Err) })
    })
}

/// The `error.data` of a spawn that a member build refusal
/// ([`MemberBuildGuard::refusal`]) ended, when Meerkat hands the refusal back
/// as its typed session error: its kind and reason. `None` for every other
/// spawn error, whose data the caller keeps as before.
pub(crate) fn member_build_refusal_data(
    error: &crate::mob_handle_runtime::MobRuntimeError,
) -> Option<Value> {
    let crate::mob_handle_runtime::MobRuntimeError::Mob(error) = error else {
        return None;
    };
    error
        .structured_data()
        .filter(|data| data.get("kind").and_then(Value::as_str) == Some(BINDING_UNCARRIED_KIND))
}

/// The `error.data.kind` of the member route refusal a mob error carries,
/// when it carries one: a member build the gateway refused
/// ([`MemberBuildGuard::refusal`]), which Meerkat hands back as its typed
/// session error with the refusal's data. Read off the error's structured
/// data, through Meerkat's shared failure wrappers, never from its text.
/// `None` for every other error.
pub(crate) fn member_route_refusal_kind(error: &meerkat_mob::MobError) -> Option<&'static str> {
    let data = error.structured_data()?;
    match data.get("kind").and_then(Value::as_str)? {
        BINDING_UNCARRIED_KIND => Some(BINDING_UNCARRIED_KIND),
        BINDING_CONFLICT_KIND => Some(BINDING_CONFLICT_KIND),
        _ => None,
    }
}

/// Why `method` may not attach `session_id` as a member's session under the
/// launch binding `binding`, if it may not: `meta/attach_existing_session`,
/// or `meta/ensure_member` with a `resume_session_id`, both a spawn in
/// Meerkat's resume mode. Meerkat reuses a session that already has a live
/// actor in this process as it is, without a rebuild, so it keeps the binding
/// it runs: a live session whose recorded binding is not the launch binding
/// (another one, or none) is refused as [`MemberRouteRefusalKind::Conflict`],
/// and so is one whose binding cannot be read, marked retryable. The launch
/// binding is accepted in either form: as named, or as `recorded_form`, the
/// form Core records it in, stamped with the realm that owns it
/// ([`validate_member_auth_binding`]'s result). A session without a live
/// actor is rebuilt, and Meerkat applies the launch binding to the rebuild.
/// Without a session service to read (`sessions` is `None`) nothing is
/// known, and the attach is refused.
pub(crate) async fn attach_binding_refusal(
    sessions: Option<&dyn meerkat_mob::MobSessionService>,
    method: &str,
    session_id: &meerkat_core::types::SessionId,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<MemberRouteRefusal> {
    let refuse = |reason: String, retryable: bool| {
        Some(MemberRouteRefusal::attach_conflict(
            method, session_id, reason, retryable,
        ))
    };
    let Some(sessions) = sessions else {
        return refuse(
            "no session service reads whether the session has a live actor".to_string(),
            false,
        );
    };
    match sessions.live_session_actor_registered(session_id).await {
        Ok(false) => return None,
        Ok(true) => {}
        Err(error) => {
            return refuse(
                format!("whether the session has a live actor cannot be read: {error}"),
                true,
            );
        }
    }
    let recorded = match sessions.load_persisted_session_metadata(session_id).await {
        Ok(view) => view
            .and_then(|view| view.session_metadata)
            .and_then(|metadata| metadata.auth_binding),
        Err(error) => {
            return refuse(
                format!("the binding its live actor runs cannot be read: {error}"),
                true,
            );
        }
    };
    let launch = match recorded_form {
        Some(form) if form != binding => format!(
            "{} (recorded as {})",
            describe_binding(binding),
            describe_binding(form)
        ),
        _ => describe_binding(binding),
    };
    match recorded {
        Some(recorded) if recorded == *binding || recorded_form == Some(&recorded) => None,
        Some(recorded) => refuse(
            format!(
                "its live actor runs auth binding {}, not {launch}",
                describe_binding(&recorded)
            ),
            false,
        ),
        None => refuse(
            format!("its live actor runs with no recorded auth binding, not {launch}"),
            false,
        ),
    }
}

/// The typed refusal an agent's mob tool answers under a member auth
/// binding, before the tool runs: `(kind, message)` for a
/// [`meerkat_core::ToolConsequenceDenial`]. `mob_spawn_member` naming
/// another `auth_binding` is [`BINDING_CONFLICT_KIND`]; an inline `tooling`
/// profile (`mob_spawn_member`, `delegate`) or a `mob_create` child
/// definition that would make provider calls off the binding is
/// [`BINDING_UNCARRIED_KIND`]. The launch binding counts as named or in
/// `recorded_form`, the form Core records it in. Arguments that do not decode
/// are left to the tool, which refuses them the same way.
pub(crate) fn agent_tool_binding_refusal(
    tool: &str,
    args: &Value,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<(&'static str, String)> {
    match tool {
        "mob_create" => args
            .get("definition")
            .and_then(|definition| child_mob_definition_escape(definition, binding, recorded_form))
            .map(|reason| {
                (
                    BINDING_UNCARRIED_KIND,
                    format!(
                        "this launch builds every member with runtime_options.\
                         member_auth_binding, and this child mob would not: {reason}"
                    ),
                )
            }),
        "mob_spawn_member" | "delegate" => {
            let named = args
                .get("auth_binding")
                .filter(|value| !value.is_null())
                .and_then(|value| {
                    serde_json::from_value::<meerkat_contracts::wire::WireAuthBindingRef>(
                        value.clone(),
                    )
                    .ok()
                })
                .map(meerkat_core::AuthBindingRef::from);
            if let Some(conflict) = named
                .as_ref()
                .and_then(|named| binding_conflict(named, binding, recorded_form))
            {
                return Some((BINDING_CONFLICT_KIND, conflict));
            }
            tooling_profile_input(args)
                .and_then(|profile| profile_input_escape(&profile, binding, recorded_form))
                .map(|reason| {
                    (
                        BINDING_UNCARRIED_KIND,
                        format!(
                            "this launch builds every member with runtime_options.\
                             member_auth_binding, and this tooling profile would not: {reason}"
                        ),
                    )
                })
        }
        "mob_profile_create" | "mob_profile_update" => args
            .get("profile")
            .and_then(|profile| {
                serde_json::from_value::<meerkat_contracts::wire::MobProfileInput>(profile.clone())
                    .ok()
            })
            .and_then(|profile| profile_input_escape(&profile, binding, recorded_form))
            .map(|reason| {
                (
                    BINDING_UNCARRIED_KIND,
                    format!(
                        "this launch builds every member with runtime_options.\
                         member_auth_binding, and a member built from this realm profile would \
                         not: {reason}"
                    ),
                )
            }),
        _ => None,
    }
}

/// The realm profile an agent spawn's `tooling` names
/// (`{"mode": "profile", "source": {"type": "realm_profile", "name": ...}}`).
fn tooling_realm_profile(args: &Value) -> Option<&str> {
    let tooling = args.get("tooling")?;
    if tooling.get("mode").and_then(Value::as_str) != Some("profile") {
        return None;
    }
    let source = tooling.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("realm_profile") {
        return None;
    }
    source.get("name").and_then(Value::as_str)
}

/// The typed refusal an agent's mob tool answers under a member auth binding
/// for the realm profiles it names, resolved from `realm_profiles`, the agent
/// mob-tool state's store, which Meerkat resolves them from: the `tooling`
/// realm profile of `mob_spawn_member` or `delegate`, and the realm-referenced
/// profiles of a `mob_create` child definition. One that would make provider
/// calls off the binding (a fallback target off it, image generation) is
/// `member_auth_binding_uncarried`, before the tool runs. A realm profile the
/// store does not hold or cannot read is left to the tool, which refuses it.
pub(crate) async fn agent_tool_realm_profile_refusal(
    tool: &str,
    args: &Value,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
    realm_profiles: &dyn meerkat_mob::RealmProfileStore,
) -> Option<(&'static str, String)> {
    let references: Vec<(String, String)> = match tool {
        "mob_spawn_member" | "delegate" => tooling_realm_profile(args)
            .map(|name| vec![("its tooling".to_string(), name.to_string())])
            .unwrap_or_default(),
        "mob_create" => args
            .get("definition")
            .and_then(|definition| {
                serde_json::from_value::<meerkat_contracts::wire::MobDefinitionInput>(
                    definition.clone(),
                )
                .ok()
            })
            .map(|input| {
                input
                    .profiles
                    .iter()
                    .filter_map(|(name, profile)| match profile {
                        meerkat_contracts::wire::MobProfileBindingInput::RealmRef {
                            realm_profile,
                        } => Some((
                            format!("this child mob's profile '{name}'"),
                            realm_profile.clone(),
                        )),
                        meerkat_contracts::wire::MobProfileBindingInput::Inline(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    for (place, name) in references {
        let Ok(Some(stored)) = realm_profiles.get(&name).await else {
            continue;
        };
        if let Some(reason) = profile_escape(&stored.profile, binding, recorded_form) {
            return Some((
                BINDING_UNCARRIED_KIND,
                format!(
                    "this launch builds every member with runtime_options.member_auth_binding, \
                     and {place} names realm profile '{name}', which would not: {reason}"
                ),
            ));
        }
    }
    None
}

/// The inline profile of an agent tool's `tooling` override
/// (`{"mode": "profile", "source": {"type": "inline", ...}}`), as Core's
/// public profile input, when there is one and it decodes.
fn tooling_profile_input(args: &Value) -> Option<meerkat_contracts::wire::MobProfileInput> {
    let tooling = args.get("tooling")?;
    if tooling.get("mode").and_then(Value::as_str) != Some("profile") {
        return None;
    }
    let mut source = tooling.get("source")?.as_object()?.clone();
    if source.remove("type").as_ref().and_then(Value::as_str) != Some("inline") {
        return None;
    }
    serde_json::from_value(Value::Object(source)).ok()
}

/// Why a child mob an agent creates (`mob_create`, whose `definition` is
/// Core's `MobDefinitionInput`) would build members off the launch's member
/// auth binding, if it would: an automatic spawn policy, whose members Core
/// builds without the spawn customizers, an enabled model fallback table
/// (`runtime.model_fallback`, an inline profile's `model_fallback`) with a
/// target that names another binding or none, or an inline profile that
/// enables image generation. A definition that does not decode is left to
/// Core, which refuses it the same way.
pub(crate) fn child_mob_definition_escape(
    definition: &Value,
    binding: &meerkat_core::AuthBindingRef,
    recorded_form: Option<&meerkat_core::AuthBindingRef>,
) -> Option<String> {
    let input: meerkat_contracts::wire::MobDefinitionInput =
        serde_json::from_value(definition.clone()).ok()?;
    if matches!(
        input.spawn_policy,
        Some(meerkat_contracts::wire::MobSpawnPolicyInput::Auto { .. })
    ) {
        return Some(
            "its automatic spawn_policy creates members Meerkat builds without the spawn \
             customizers, so they would not build with the launch's member auth binding"
                .to_string(),
        );
    }
    if let Some(reason) = fallback_escape(
        input.runtime.model_fallback.as_ref(),
        binding,
        recorded_form,
    ) {
        return Some(format!("runtime.model_fallback: {reason}"));
    }
    input
        .profiles
        .iter()
        .find_map(|(name, profile)| match profile {
            meerkat_contracts::wire::MobProfileBindingInput::Inline(profile) => {
                profile_input_escape(profile, binding, recorded_form)
                    .map(|reason| format!("profiles.{name}.{reason}"))
            }
            meerkat_contracts::wire::MobProfileBindingInput::RealmRef { .. } => None,
        })
}

/// Forces the launch's member auth binding onto every member spawn Core asks
/// the gateway's spawn customizers about: fresh spawns, helpers, batch items,
/// flow provisioning, forks, respawns and successors
/// ([`meerkat_mob::SpawnMemberCustomizer::customize_spawn`]), and every build
/// of a member at its wake (`customize_resume`, whose default asks
/// `customize_spawn`): the rebuild of a member on its durable session,
/// process-restart restore included, where Core takes an explicit binding in
/// place of the session's durable one, and the first build of a member Meta
/// seated Dormant or rotated onto a fresh life, whose seat built nothing.
///
/// A spawn that names the same binding, as named or in the form Core records
/// it ([`Self::with_recorded_form`]), is accepted. One that names another
/// binding is refused with [`meerkat_mob::MobError::WiringError`] (as Core's
/// own child tool policy refuses), whoever builds the member, so no caller
/// (an agent's `mob_spawn_member` argument, a helper option) moves a member
/// onto another account; an agent meets it typed first
/// ([`agent_tool_binding_refusal`]). A member this gateway builds is also
/// refused when its profile override enables image generation or carries a
/// model fallback off the binding, or its spawn explicitly enables image
/// generation: Core builds those executors by default selection. Web search
/// is judged at the build ([`MemberBuildGuard`]), where the model it runs on
/// is known: the provider's own search runs on the member's binding, and
/// only Meerkat's fallback executor is refused. A member the spawn places on
/// another member host, or binds to an external process, is built elsewhere,
/// so nothing is filled in; a profile- or definition-level external backend
/// still receives the binding, which nothing builds locally.
///
/// Core asks no customizer for a durable fork's seating (a `fork_off` child,
/// a live delegation fork, a council participant) or its rebuilds: those
/// repeat the member's first build from its own durable records, which name
/// the binding its source was built with, and the build is refused when that
/// is not the launch binding ([`MemberBuildGuard::escape`]). Nor does Core
/// ask one for an identity it materializes from a sealed declaration, so the
/// gateway refuses `mob/adopt_member_identity_declaration` and
/// `mob/apply_member_tool_declaration` under a member auth binding
/// ([`MemberRouteRefusal::declaration_uncarried`]) and init refuses the
/// declarations an earlier launch adopted off it
/// ([`validate_adopted_declarations`]), or for a policy auto-spawn, which
/// init, the stored child-mob check ([`validate_stored_child_mobs`]) and the
/// child-mob guard ([`child_mob_definition_escape`]) refuse. Whatever path
/// builds a member, the build itself is refused when it would run off the
/// binding or still make provider calls off it ([`member_build_hook`]).
pub(crate) struct MemberAuthBindingCustomizer {
    binding: meerkat_core::AuthBindingRef,
    recorded_form: Option<meerkat_core::AuthBindingRef>,
}

impl MemberAuthBindingCustomizer {
    pub(crate) fn new(binding: meerkat_core::AuthBindingRef) -> Self {
        Self {
            binding,
            recorded_form: None,
        }
    }

    /// Accept the binding in `recorded_form` too: the form Core records it
    /// in, stamped with the realm that owns it.
    pub(crate) fn with_recorded_form(
        mut self,
        recorded_form: Option<meerkat_core::AuthBindingRef>,
    ) -> Self {
        self.recorded_form = recorded_form;
        self
    }

    /// The customizer body, factored off the trait so tests can drive it:
    /// `SpawnCustomizationContext` is `#[non_exhaustive]` and only
    /// meerkat-mob constructs it. The binding does not depend on the spawn
    /// source.
    fn apply(&self, spec: &mut meerkat_mob::SpawnMemberSpec) -> Result<(), meerkat_mob::MobError> {
        // A named binding is compared on every spawn, a placed or external
        // member's included, so no caller chooses another account; only a
        // member this gateway builds gets the launch binding filled in.
        if let Some(conflict) = spec
            .auth_binding
            .as_ref()
            .and_then(|named| binding_conflict(named, &self.binding, self.recorded_form.as_ref()))
        {
            return Err(meerkat_mob::MobError::WiringError(format!(
                "member '{}': {conflict}",
                spec.identity
            )));
        }
        if !built_by_this_gateway(spec) {
            return Ok(());
        }
        if spec.auth_binding.is_none() {
            spec.auth_binding = Some(self.binding.clone());
        }
        let enabled = meerkat_core::ToolCategoryOverride::Enable;
        let escape = spec
            .override_profile
            .as_ref()
            .and_then(|profile| profile_escape(profile, &self.binding, self.recorded_form.as_ref()))
            .map(|reason| format!("profile override {reason}"))
            .or_else(|| {
                (spec.tool_category_overrides.image_generation == enabled).then(|| {
                    "tool_category_overrides.image_generation: Meerkat builds image \
                     generation executors for the other image providers by default binding \
                     selection"
                        .to_string()
                })
            });
        if let Some(reason) = escape {
            return Err(meerkat_mob::MobError::WiringError(format!(
                "{BINDING_UNCARRIED_KIND}: member '{}' {reason}; every member of this launch \
                 builds with {} (runtime_options.member_auth_binding)",
                spec.identity,
                describe_binding(&self.binding)
            )));
        }
        Ok(())
    }
}

/// Whether this gateway's member factory builds the member `spec` spawns, as
/// the spawn request says: a local session member, not one it places on
/// another member host and not one it binds to an external process.
fn built_by_this_gateway(spec: &meerkat_mob::SpawnMemberSpec) -> bool {
    spec.placement.is_none()
        && !matches!(spec.backend, Some(meerkat_mob::MobBackendKind::External))
        && matches!(
            spec.binding,
            None | Some(meerkat_mob::RuntimeBinding::Session)
        )
}

impl meerkat_mob::SpawnMemberCustomizer for MemberAuthBindingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &meerkat_mob::SpawnCustomizationContext,
        spec: &mut meerkat_mob::SpawnMemberSpec,
    ) -> Result<(), meerkat_mob::MobError> {
        self.apply(spec)
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::needless_raw_string_hashes
)]
mod tests {
    use super::*;
    use serde_json::json;

    const REALM: &str = "native-team";

    /// The JSON a member auth binding serializes to: Core's own
    /// serialization of the typed binding, which omits the configured
    /// origin.
    fn member_auth_binding_json(binding: &meerkat_core::AuthBindingRef) -> Value {
        serde_json::to_value(binding).expect("Core serializes an auth binding")
    }

    fn binding(name: &str) -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse(REALM).unwrap(),
            binding: meerkat_core::BindingId::parse(name).unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }
    }

    fn selection_json(root: &Path) -> Value {
        json!({
            "realm_id": REALM,
            "context_root": root.join("context"),
            "state_root": root.join("realms"),
            "user_config_root": root.join("home"),
        })
    }

    fn route(options: &Value) -> Result<MemberRoute, MemberRouteRefusal> {
        MemberRoute::from_runtime_options(options.as_object().expect("an options object"))
    }

    fn refusal_kind(result: Result<impl std::fmt::Debug, MemberRouteRefusal>) -> String {
        match result {
            Ok(value) => panic!("expected a refusal, got {value:?}"),
            Err(refusal) => {
                assert_eq!(
                    refusal.data()["kind"],
                    json!(refusal.kind.as_str()),
                    "data.kind is the refusal's kind"
                );
                refusal.kind.as_str().to_string()
            }
        }
    }

    /// The realm's own document: two OpenAI API key bindings, `a` the realm
    /// default and `b` the one the members are bound to, each reading its key
    /// from its own environment variable, and a self-hosted server with one
    /// alias behind a third binding.
    const REALM_DOCUMENT: &str = r#"
[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "http://127.0.0.1:9"
api_style = "chat_completions"

[self_hosted.models.realm-alias]
server = "local"
remote_model = "remote-model"

[realm.native-team]
default_binding = "a"

[realm.native-team.backend.openai_main]
provider = "openai"
backend_kind = "openai_api"

[realm.native-team.auth.key_a]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "NATIVE_REALM_UNIT_KEY_A" }

[realm.native-team.auth.key_b]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "NATIVE_REALM_UNIT_KEY_B" }

[realm.native-team.binding.a]
backend_profile = "openai_main"
auth_profile = "key_a"

[realm.native-team.binding.b]
backend_profile = "openai_main"
auth_profile = "key_b"

[realm.native-team.backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "local"

[realm.native-team.auth.local_none]
provider = "self_hosted"
auth_method = "none"
source = { kind = "platform_default" }

[realm.native-team.binding.local]
backend_profile = "local"
auth_profile = "local_none"
"#;

    /// A native realm on disk: its manifest written by the store's own realm
    /// API, its document beside it, and an empty user configuration root.
    async fn native_realm_on_disk(root: &Path) -> NativeRealmSelection {
        let realms = root.join("realms");
        meerkat_store::ensure_realm_manifest_in(&realms, REALM, None, None)
            .await
            .expect("the realm manifest");
        std::fs::write(
            meerkat_store::realm_paths_in(&realms, REALM).config_path,
            REALM_DOCUMENT,
        )
        .expect("the realm document");
        std::fs::create_dir_all(root.join("home")).expect("the user configuration root");
        std::fs::create_dir_all(root.join("context")).expect("the context root");
        NativeRealmSelection::parse(&selection_json(root)).expect("a valid selection")
    }

    fn definition(profiles: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"native-realm-unit\"\n\n{profiles}"
        ))
        .expect("the test definition")
    }

    const OPENAI_PROFILE: &str = "[profiles.worker]\nmodel = \"gpt-5.5\"\nprovider = \"openai\"\n";

    // -- parse and serde round trip ----------------------------------------

    #[test]
    fn native_realm_parses_and_round_trips_through_serde() {
        let root = Path::new("/srv/native");
        let value = selection_json(root);
        let parsed = NativeRealmSelection::parse(&value).expect("a valid selection");
        assert_eq!(parsed.realm_id, REALM);
        assert_eq!(parsed.state_root, root.join("realms"));
        let serialized = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(serialized, value, "the wire form is the option as sent");
        assert_eq!(
            NativeRealmSelection::parse(&serialized).expect("reparse"),
            parsed,
            "serialize then parse is the identity"
        );
    }

    #[test]
    fn member_auth_binding_parses_into_cores_typed_binding_and_round_trips() {
        let parsed = parse_member_auth_binding(&json!({
            "realm": REALM,
            "binding": "b",
            "profile": "work",
            "origin": "configured",
        }))
        .expect("a configured binding");
        assert_eq!(
            parsed,
            meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse(REALM).unwrap(),
                binding: meerkat_core::BindingId::parse("b").unwrap(),
                profile: Some(meerkat_core::ProfileId::parse("work").unwrap()),
                origin: meerkat_core::BindingOrigin::Configured,
            }
        );
        // Core serializes the configured origin by omitting it; the option
        // reads that form back to the same binding.
        let serialized = member_auth_binding_json(&parsed);
        assert_eq!(
            serialized,
            json!({ "realm": REALM, "binding": "b", "profile": "work" })
        );
        assert_eq!(
            parse_member_auth_binding(&serialized).expect("reparse"),
            parsed
        );
        // And Core's own deserialization of the option's form agrees.
        let core: meerkat_core::AuthBindingRef =
            serde_json::from_value(serialized).expect("Core reads the option's form");
        assert_eq!(core, parsed);
        // Without `origin` and `profile` the binding is configured and has
        // no profile.
        let bare = parse_member_auth_binding(&json!({ "realm": REALM, "binding": "b" }))
            .expect("a bare binding");
        assert_eq!(bare, binding("b"));
    }

    #[test]
    fn absent_options_route_nothing() {
        assert_eq!(route(&json!({})).unwrap(), MemberRoute::default());
        assert_eq!(
            MemberRoute::from_init_params(&json!({})).unwrap(),
            MemberRoute::default()
        );
        // A binding alone routes the members of a host config file or the
        // default configuration.
        let alone = route(&json!({
            "member_auth_binding": { "realm": REALM, "binding": "b" }
        }))
        .expect("a binding alone");
        assert_eq!(alone.member_auth_binding, Some(binding("b")));
        assert_eq!(alone.native_realm, None);
    }

    // -- refusals -----------------------------------------------------------

    #[test]
    fn a_native_realm_beside_the_host_config_file_is_refused() {
        let options = json!({
            "native_realm": selection_json(Path::new("/srv/native")),
            "member_auth_binding": { "realm": REALM, "binding": "b" },
            "meerkat_config_path": "/srv/native/config.toml",
        });
        assert_eq!(
            refusal_kind(route(&options)),
            "native_realm_config_conflict"
        );
        let refusal = route(&options).unwrap_err();
        assert_eq!(refusal.code(), -32602);
        assert!(
            refusal.message().contains("meerkat_config_path"),
            "{refusal}"
        );
    }

    #[test]
    fn a_native_realm_without_a_binding_is_refused() {
        let options = json!({ "native_realm": selection_json(Path::new("/srv/native")) });
        assert_eq!(
            refusal_kind(route(&options)),
            "member_auth_binding_required"
        );
    }

    #[test]
    fn malformed_native_realms_are_refused() {
        let root = Path::new("/srv/native");
        let mut unknown_field = selection_json(root);
        unknown_field["extra"] = json!(true);
        let mut missing = selection_json(root);
        missing.as_object_mut().unwrap().remove("state_root");
        let mut relative = selection_json(root);
        relative["state_root"] = json!("realms");
        let mut bad_id = selection_json(root);
        bad_id["realm_id"] = json!("not a realm");
        for value in [
            json!("native-team"),
            unknown_field,
            missing,
            relative,
            bad_id,
        ] {
            let options = json!({
                "native_realm": value,
                "member_auth_binding": { "realm": REALM, "binding": "b" },
            });
            assert_eq!(
                refusal_kind(route(&options)),
                "native_realm_invalid",
                "{options}"
            );
        }
    }

    #[test]
    fn malformed_bindings_are_refused() {
        for value in [
            json!("native-team:b"),
            json!({ "realm": REALM }),
            json!({ "realm": REALM, "binding": "" }),
            json!({ "realm": REALM, "binding": "has space" }),
            json!({ "realm": REALM, "binding": "b", "token": "x" }),
            json!({ "realm": REALM, "binding": "b", "origin": 1 }),
        ] {
            assert_eq!(
                refusal_kind(parse_member_auth_binding(&value)),
                "member_auth_binding_invalid",
                "{value}"
            );
        }
    }

    #[test]
    fn an_origin_other_than_configured_is_refused() {
        for origin in ["synthetic_env_default", "Configured", "env"] {
            let refusal = parse_member_auth_binding(&json!({
                "realm": REALM,
                "binding": "b",
                "origin": origin,
            }))
            .unwrap_err();
            assert_eq!(refusal.kind, MemberRouteRefusalKind::OriginNotConfigured);
            assert_eq!(refusal.data()["origin"], json!(origin));
            assert_eq!(refusal.code(), -32602);
        }
    }

    #[tokio::test]
    async fn an_unknown_realm_is_refused_with_the_storage_resolution_code() {
        let root = tempfile::tempdir().unwrap();
        let realms = root.path().join("realms");
        std::fs::create_dir_all(&realms).unwrap();
        let selection = NativeRealmSelection::parse(&selection_json(root.path())).unwrap();
        let refusal = read_native_realm_config(&selection).await.unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::RealmUnknown);
        assert_eq!(refusal.code(), crate::STORAGE_RESOLUTION_CODE);
        assert_eq!(refusal.data()["realm_id"], json!(REALM));
        assert!(
            !realms.join(REALM).exists(),
            "reading a native realm creates nothing in it"
        );
    }

    /// A realm document that does not parse is refused as invalid params,
    /// as a `meerkat_config_path` file that does not parse is.
    #[tokio::test]
    async fn an_unreadable_realm_document_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let selection = native_realm_on_disk(root.path()).await;
        std::fs::write(
            meerkat_store::realm_paths_in(&selection.state_root, REALM).config_path,
            "[realm.native-team\nnot toml",
        )
        .unwrap();
        let refusal = read_native_realm_config(&selection).await.unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::ConfigInvalid);
        assert_eq!(refusal.code(), -32602);
        assert_eq!(refusal.data()["kind"], json!("native_realm_config_invalid"));
        assert_eq!(refusal.data()["realm_id"], json!(REALM));
        assert!(refusal.data()["reason"].is_string(), "{:?}", refusal.data());
    }

    #[tokio::test]
    async fn the_native_realm_reads_its_own_document_through_cores_reader() {
        let root = tempfile::tempdir().unwrap();
        let selection = native_realm_on_disk(root.path()).await;
        let config = read_native_realm_config(&selection)
            .await
            .expect("the realm");
        let section = config
            .realm
            .get(REALM)
            .expect("the composed configuration keeps the realm's section");
        assert!(section.binding.contains_key("b"), "{section:?}");
        assert!(
            config.self_hosted.models.contains_key("realm-alias"),
            "the realm document's tables reach the members' configuration"
        );
    }

    #[tokio::test]
    async fn a_missing_binding_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let refusal =
            validate_member_auth_binding(&config, &binding("absent"), &definition(OPENAI_PROFILE))
                .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::BindingUnknown);
        assert_eq!(refusal.data()["binding"], json!("absent"));
        // A binding's auth profile is named `auth_profile`.
        let mut with_profile = binding("absent");
        with_profile.profile = Some(meerkat_core::ProfileId::parse("work").unwrap());
        let refusal =
            validate_member_auth_binding(&config, &with_profile, &definition(OPENAI_PROFILE))
                .unwrap_err();
        assert_eq!(refusal.data()["auth_profile"], json!("work"));
        // A realm the configuration does not hold is refused the same way.
        let mut elsewhere = binding("b");
        elsewhere.realm = meerkat_core::RealmId::parse("another-team").unwrap();
        let refusal =
            validate_member_auth_binding(&config, &elsewhere, &definition(OPENAI_PROFILE))
                .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::BindingUnknown);
        // And the default configuration holds no realm binding at all.
        let refusal = validate_member_auth_binding(
            &meerkat_core::Config::default(),
            &binding("b"),
            &definition(OPENAI_PROFILE),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::BindingUnknown);
    }

    #[tokio::test]
    async fn a_profile_the_binding_does_not_serve_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        validate_member_auth_binding(&config, &binding("b"), &definition(OPENAI_PROFILE))
            .expect("an OpenAI profile matches the OpenAI binding");

        // Another provider, named by the profile.
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(
                "[profiles.lead]\nmodel = \"claude-sonnet-4-5\"\nprovider = \"anthropic\"\n",
            ),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);
        assert_eq!(refusal.data()["mob_profile"], json!("lead"));
        assert_eq!(refusal.data()["binding_provider"], json!("openai"));
        assert!(
            refusal.data().get("profile").is_none() && refusal.data().get("profile_id").is_none(),
            "the definition profile is `mob_profile`, the binding's auth profile `auth_profile`: \
             {:?}",
            refusal.data()
        );

        // Another provider, owned by the catalog.
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition("[profiles.lead]\nmodel = \"claude-sonnet-4-5\"\n"),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);

        // The binding's provider named over a model another provider owns.
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition("[profiles.lead]\nmodel = \"claude-sonnet-4-5\"\nprovider = \"openai\"\n"),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);
        assert_eq!(refusal.data()["model"], json!("claude-sonnet-4-5"));

        // A self-hosted binding serves its own server's aliases only.
        validate_member_auth_binding(
            &config,
            &binding("local"),
            &definition(
                "[profiles.local]\nmodel = \"realm-alias\"\nprovider = \"self_hosted\"\n\
                 self_hosted_server_id = \"local\"\n",
            ),
        )
        .expect("the realm's alias on the binding's server");
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(
                "[profiles.local]\nmodel = \"realm-alias\"\nprovider = \"self_hosted\"\n\
                 self_hosted_server_id = \"local\"\n",
            ),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);
        // A self-hosted model the members' configuration does not register
        // fails at the build, as Core's factory requires it registered with
        // a server: init refuses it first.
        let refusal = validate_member_auth_binding(
            &config,
            &binding("local"),
            &definition(
                "[profiles.local]\nmodel = \"unregistered-model\"\nprovider = \"self_hosted\"\n",
            ),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);
        assert_eq!(refusal.data()["model"], json!("unregistered-model"));
    }

    #[tokio::test]
    async fn an_automatic_spawn_policy_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(&format!(
                "{OPENAI_PROFILE}\n[spawn_policy]\nmode = \"auto\"\n\n\
                 [spawn_policy.profile_map]\nanyone = \"worker\"\n"
            )),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);
    }

    // -- the customizer -----------------------------------------------------

    #[test]
    fn every_locally_built_spawn_carries_the_binding() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut fresh = meerkat_mob::SpawnMemberSpec::new("worker", "keeper");
        customizer.apply(&mut fresh).expect("a fresh spawn");
        assert_eq!(fresh.auth_binding, Some(binding("b")));

        // The rebuild of a member on its durable session.
        let mut resumed = meerkat_mob::SpawnMemberSpec::new("worker", "keeper").with_launch_mode(
            meerkat_mob::launch::MemberLaunchMode::Resume {
                bridge_session_id: meerkat_core::types::SessionId::new(),
                resume_from_role: None,
            },
        );
        customizer.apply(&mut resumed).expect("a rebuild");
        assert_eq!(resumed.auth_binding, Some(binding("b")));

        // A member bound to a local session explicitly.
        let mut session_bound = meerkat_mob::SpawnMemberSpec::new("worker", "spawned");
        session_bound.binding = Some(meerkat_mob::RuntimeBinding::Session);
        customizer
            .apply(&mut session_bound)
            .expect("a session member");
        assert_eq!(session_bound.auth_binding, Some(binding("b")));

        // A spawn that names the launch binding itself is accepted as is.
        let mut same =
            meerkat_mob::SpawnMemberSpec::new("worker", "helper").with_auth_binding(binding("b"));
        customizer.apply(&mut same).expect("the launch binding");
        assert_eq!(same.auth_binding, Some(binding("b")));
    }

    /// The launch binding is forced: a spawn that names another binding,
    /// another realm's or the same binding with another auth profile, is
    /// refused, never built on another account.
    #[test]
    fn a_spawn_that_names_another_binding_is_refused() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut other_profile = binding("b");
        other_profile.profile = Some(meerkat_core::ProfileId::parse("work").unwrap());
        let mut other_realm = binding("b");
        other_realm.realm = meerkat_core::RealmId::parse("global").unwrap();
        for named in [binding("a"), other_profile, other_realm] {
            let mut spec = meerkat_mob::SpawnMemberSpec::new("worker", "helper")
                .with_auth_binding(named.clone());
            let error = customizer.apply(&mut spec).unwrap_err();
            assert!(
                matches!(&error, meerkat_mob::MobError::WiringError(message)
                    if message.contains("member_auth_binding")),
                "{error:?}"
            );
            assert_eq!(spec.auth_binding, Some(named), "nothing is rewritten");
        }
    }

    /// A profile override whose model fallback would build off the binding
    /// is refused at the spawn, as an inline profile's is at init.
    #[test]
    fn a_profile_override_whose_fallback_leaves_the_binding_is_refused() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let override_with = |fallback: &str| {
            let definition = definition(&format!(
                "{OPENAI_PROFILE}\n[profiles.worker.model_fallback]\nenabled = true\n{fallback}"
            ));
            definition
                .profiles
                .values()
                .find_map(|profile| profile.as_inline().cloned())
                .expect("an inline profile")
        };
        let mut unbound = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        unbound.override_profile = Some(override_with(
            "[[profiles.worker.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n",
        ));
        assert!(customizer.apply(&mut unbound).is_err());

        let mut bound = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        bound.override_profile = Some(override_with(
            "[[profiles.worker.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n\
             auth_binding = { realm = \"native-team\", binding = \"b\" }\n",
        ));
        customizer
            .apply(&mut bound)
            .expect("a fallback on the launch binding");
    }

    #[test]
    fn members_built_elsewhere_are_left_alone() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut placed = meerkat_mob::SpawnMemberSpec::new("worker", "remote").with_placement(
            meerkat_mob::machines::mob_machine::HostId::from("member-host".to_string()),
        );
        customizer.apply(&mut placed).expect("a placed member");
        assert_eq!(placed.auth_binding, None);

        let mut external = meerkat_mob::SpawnMemberSpec::new("worker", "peer")
            .with_backend(meerkat_mob::MobBackendKind::External);
        customizer.apply(&mut external).expect("an external member");
        assert_eq!(external.auth_binding, None);
    }

    // -- model fallback, child mobs, adoption -------------------------------

    /// An enabled fallback chain builds a member with each target's own
    /// binding, and a target that names none by default selection: init
    /// refuses either unless the target names the launch binding, wherever
    /// the table is (the members' configuration, the definition runtime, an
    /// inline profile). A disabled table builds nothing and passes.
    #[tokio::test]
    async fn a_fallback_target_off_the_binding_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let unbound = "[[profiles.worker.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n";
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(&format!(
                "{OPENAI_PROFILE}\n[profiles.worker.model_fallback]\nenabled = true\n{unbound}"
            )),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("profiles.worker.model_fallback")),
            "{:?}",
            refusal.data()
        );

        let other = "[[runtime.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n\
                     auth_binding = { realm = \"native-team\", binding = \"a\" }\n";
        let refusal = validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(&format!(
                "[runtime.model_fallback]\nenabled = true\n{other}\n{OPENAI_PROFILE}"
            )),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);

        let mut fallback_config = config.clone();
        fallback_config.model_fallback.enabled = Some(true);
        fallback_config
            .model_fallback
            .chain
            .push(meerkat_core::config::ModelFallbackTarget {
                model: "gpt-5.4".to_string(),
                provider: None,
                auth_binding: None,
            });
        let refusal = validate_member_auth_binding(
            &fallback_config,
            &binding("b"),
            &definition(OPENAI_PROFILE),
        )
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);

        // On the launch binding, or disabled, the chain passes.
        let bound = "[[profiles.worker.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n\
                     auth_binding = { realm = \"native-team\", binding = \"b\" }\n";
        validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(&format!(
                "{OPENAI_PROFILE}\n[profiles.worker.model_fallback]\nenabled = true\n{bound}"
            )),
        )
        .expect("a fallback on the launch binding");
        validate_member_auth_binding(
            &config,
            &binding("b"),
            &definition(&format!(
                "{OPENAI_PROFILE}\n[profiles.worker.model_fallback]\nenabled = false\n{unbound}"
            )),
        )
        .expect("a disabled fallback");
    }

    /// A child mob an agent creates may not build members off the binding:
    /// an automatic spawn policy or a fallback target off the binding is
    /// named, and a plain child definition (or one Core will refuse to
    /// decode anyway) passes.
    #[test]
    fn a_child_mob_that_would_leave_the_binding_is_named() {
        let launch = binding("b");
        let plain = json!({
            "id": "child",
            "profiles": { "worker": { "model": "gpt-5.5" } },
        });
        assert_eq!(child_mob_definition_escape(&plain, &launch, None), None);
        assert_eq!(
            child_mob_definition_escape(&json!("not a definition"), &launch, None),
            None
        );

        let mut auto = plain.clone();
        auto["spawn_policy"] = json!({ "mode": "auto", "profile_map": { "anyone": "worker" } });
        assert!(
            child_mob_definition_escape(&auto, &launch, None)
                .is_some_and(|reason| reason.contains("spawn_policy")),
            "an automatic spawn policy is named"
        );
        let mut none_policy = plain.clone();
        none_policy["spawn_policy"] = json!({ "mode": "none" });
        assert_eq!(
            child_mob_definition_escape(&none_policy, &launch, None),
            None
        );

        let mut runtime_fallback = plain.clone();
        runtime_fallback["runtime"] = json!({ "model_fallback": {
            "enabled": true,
            "chain": [{ "model": "gpt-5.4" }],
        } });
        assert!(
            child_mob_definition_escape(&runtime_fallback, &launch, None)
                .is_some_and(|reason| reason.contains("runtime.model_fallback"))
        );
        let mut profile_fallback = plain.clone();
        profile_fallback["profiles"]["worker"]["model_fallback"] = json!({
            "enabled": true,
            "chain": [{ "model": "gpt-5.4", "auth_binding": { "realm": REALM, "binding": "a" } }],
        });
        assert!(
            child_mob_definition_escape(&profile_fallback, &launch, None)
                .is_some_and(|reason| reason.contains("profiles.worker.model_fallback"))
        );
        let mut bound_fallback = plain;
        bound_fallback["profiles"]["worker"]["model_fallback"] = json!({
            "enabled": true,
            "chain": [{ "model": "gpt-5.4", "auth_binding": member_auth_binding_json(&launch) }],
        });
        assert_eq!(
            child_mob_definition_escape(&bound_fallback, &launch, None),
            None
        );
    }

    #[test]
    fn adoption_under_a_member_auth_binding_is_refused_as_uncarried() {
        let refusal =
            MemberRouteRefusal::declaration_uncarried("mob/adopt_member_identity_declaration");
        assert_eq!(refusal.code(), -32602);
        assert_eq!(
            refusal.data()["kind"],
            json!("member_auth_binding_uncarried")
        );
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("mob/adopt_member_identity_declaration")),
            "{:?}",
            refusal.data()
        );
    }

    /// Agent memory engines build their clients off the binding, so a launch
    /// with one refuses them, typed.
    #[test]
    fn agent_memory_engines_under_a_member_auth_binding_are_refused_as_uncarried() {
        let refusal = MemberRouteRefusal::memory_engines_uncarried();
        assert_eq!(refusal.code(), -32602);
        assert_eq!(
            refusal.data()["kind"],
            json!("member_auth_binding_uncarried")
        );
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("agent_memory.distiller")),
            "{:?}",
            refusal.data()
        );
    }

    /// Image generation builds executors for the other image providers by
    /// default selection: an inline profile that enables it is refused at
    /// init, in a profile override at the spawn, and in a child mob's or a
    /// tooling profile's inline profile at the agent's tool.
    #[tokio::test]
    async fn image_generation_off_the_binding_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let with_images =
            format!("{OPENAI_PROFILE}\n[profiles.worker.tools]\nimage_generation = true\n");
        let refusal =
            validate_member_auth_binding(&config, &binding("b"), &definition(&with_images))
                .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("profiles.worker.tools.image_generation")),
            "{:?}",
            refusal.data()
        );

        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut overridden = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        overridden.override_profile = definition(&with_images)
            .profiles
            .values()
            .find_map(|profile| profile.as_inline().cloned());
        assert!(customizer.apply(&mut overridden).is_err());

        let mut child = json!({
            "id": "child",
            "profiles": { "worker": { "model": "gpt-5.5", "tools": { "image_generation": true } } },
        });
        assert!(
            child_mob_definition_escape(&child, &binding("b"), None)
                .is_some_and(|reason| reason.contains("image_generation"))
        );
        child["profiles"]["worker"]["tools"]["image_generation"] = json!(false);
        assert_eq!(
            child_mob_definition_escape(&child, &binding("b"), None),
            None
        );
    }

    /// A spawn that explicitly enables image generation is refused: Core
    /// builds its executors by default selection. One that enables web search
    /// passes the spawn check: whether the provider's own search or Meerkat's
    /// fallback executor serves it is known at the build, where
    /// [`MemberBuildGuard`] refuses only the fallback.
    #[test]
    fn a_spawn_enabling_executors_off_the_binding_is_refused() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut web = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        web.tool_category_overrides.web_search = meerkat_core::ToolCategoryOverride::Enable;
        customizer
            .apply(&mut web)
            .expect("web search is judged at the build");
        let mut images = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        images.tool_category_overrides.image_generation =
            meerkat_core::ToolCategoryOverride::Enable;
        assert!(customizer.apply(&mut images).is_err());
        let mut inherit = meerkat_mob::SpawnMemberSpec::new("worker", "helper");
        customizer
            .apply(&mut inherit)
            .expect("inherited categories pass");
    }

    /// A placed or external member is built elsewhere and gets nothing filled
    /// in, but one that names another binding is still refused: no caller
    /// chooses another account.
    #[test]
    fn a_member_built_elsewhere_naming_another_binding_is_refused() {
        let customizer = MemberAuthBindingCustomizer::new(binding("b"));
        let mut placed = meerkat_mob::SpawnMemberSpec::new("worker", "remote")
            .with_placement(meerkat_mob::machines::mob_machine::HostId::from(
                "member-host".to_string(),
            ))
            .with_auth_binding(binding("a"));
        assert!(customizer.apply(&mut placed).is_err());
        let mut placed_same = meerkat_mob::SpawnMemberSpec::new("worker", "remote")
            .with_placement(meerkat_mob::machines::mob_machine::HostId::from(
                "member-host".to_string(),
            ))
            .with_auth_binding(binding("b"));
        customizer
            .apply(&mut placed_same)
            .expect("the launch binding itself");
    }

    /// The typed refusals an agent's mob tool answers: another binding is
    /// `member_auth_binding_conflict`; an inline tooling profile or a child
    /// definition off the binding is `member_auth_binding_uncarried`; the
    /// launch binding and other tools pass.
    #[test]
    fn agent_tools_refuse_bindings_and_profiles_off_the_launch_binding_typed() {
        let launch = binding("b");
        let spawn = |extra: Value| {
            let mut args = json!({ "mob_id": "child", "profile": "worker", "member_id": "w1" });
            if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
                args.extend(extra.clone());
            }
            args
        };
        let conflict = agent_tool_binding_refusal(
            "mob_spawn_member",
            &spawn(json!({ "auth_binding": { "realm": REALM, "binding": "a" } })),
            &launch,
            None,
        );
        assert_eq!(conflict.map(|(kind, _)| kind), Some(BINDING_CONFLICT_KIND));
        assert_eq!(
            agent_tool_binding_refusal(
                "mob_spawn_member",
                &spawn(json!({ "auth_binding": { "realm": REALM, "binding": "b" } })),
                &launch,
                None,
            ),
            None
        );
        let tooling = json!({ "tooling": {
            "mode": "profile",
            "source": { "type": "inline", "model": "gpt-5.5", "tools": { "image_generation": true } },
        } });
        assert_eq!(
            agent_tool_binding_refusal("delegate", &tooling, &launch, None).map(|(kind, _)| kind),
            Some(BINDING_UNCARRIED_KIND)
        );
        assert_eq!(
            agent_tool_binding_refusal("mob_spawn_member", &spawn(tooling), &launch, None)
                .map(|(kind, _)| kind),
            Some(BINDING_UNCARRIED_KIND)
        );
        let inherit = json!({ "tooling": { "mode": "inherit_parent" } });
        assert_eq!(
            agent_tool_binding_refusal("delegate", &inherit, &launch, None),
            None
        );
        let child = json!({ "definition": {
            "id": "child",
            "profiles": { "worker": { "model": "gpt-5.5" } },
            "spawn_policy": { "mode": "auto", "profile_map": { "anyone": "worker" } },
        } });
        assert_eq!(
            agent_tool_binding_refusal("mob_create", &child, &launch, None).map(|(kind, _)| kind),
            Some(BINDING_UNCARRIED_KIND)
        );
        assert_eq!(
            agent_tool_binding_refusal("fork_off", &child, &launch, None),
            None
        );
    }

    /// An attach of a session as a member's (`meta/attach_existing_session`,
    /// `meta/ensure_member` with `resume_session_id`) under a member auth
    /// binding: Meerkat reuses a session's live actor as it is, so a live
    /// session that records another binding or none is refused typed
    /// (`member_auth_binding_conflict`), as is one whose live actor cannot be
    /// read (marked retryable) or whose service is missing; a live session on
    /// the launch binding, and a session without a live actor (rebuilt under
    /// the launch binding), are accepted.
    #[tokio::test]
    async fn attaching_a_live_session_off_the_launch_binding_is_refused_typed() {
        async fn attach(
            live: Option<bool>,
            recorded: Option<meerkat_core::AuthBindingRef>,
            session_id: &meerkat_core::types::SessionId,
        ) -> Option<MemberRouteRefusal> {
            let sessions = crate::mob_handle_runtime::recorded_binding_probe(live, recorded);
            attach_binding_refusal(
                Some(&*sessions),
                "meta/attach_existing_session",
                session_id,
                &binding("b"),
                None,
            )
            .await
        }
        let session_id = meerkat_core::types::SessionId::new();

        let conflict = attach(Some(true), Some(binding("a")), &session_id)
            .await
            .expect("a live session on another binding is refused");
        assert_eq!(conflict.code(), INVALID_PARAMS);
        assert_eq!(conflict.data()["kind"], json!(BINDING_CONFLICT_KIND));
        assert_eq!(conflict.data()["session_id"], json!(session_id.to_string()));
        assert!(
            conflict.data().get("retryable").is_none(),
            "a binding that differs is not retryable: {}",
            conflict.data()
        );
        let error = conflict.json_rpc_error();
        assert_eq!(error.code, INVALID_PARAMS);
        assert_eq!(error.message, conflict.message());
        assert_eq!(error.data.as_ref(), Some(conflict.data()));
        assert!(
            conflict
                .message()
                .starts_with("meta/attach_existing_session cannot attach session"),
            "{}",
            conflict.message()
        );

        let kind = |refusal: Option<MemberRouteRefusal>| refusal.map(|r| r.data()["kind"].clone());
        assert_eq!(
            kind(attach(Some(true), None, &session_id).await),
            Some(json!(BINDING_CONFLICT_KIND)),
            "a live session that records no binding"
        );
        let unread = attach(None, Some(binding("b")), &session_id)
            .await
            .expect("a live-actor read that fails is refused");
        assert_eq!(unread.data()["kind"], json!(BINDING_CONFLICT_KIND));
        assert_eq!(
            unread.data()["retryable"],
            json!(true),
            "a failed read is retryable: {}",
            unread.data()
        );
        assert_eq!(
            kind(
                attach_binding_refusal(
                    None,
                    "meta/ensure_member",
                    &session_id,
                    &binding("b"),
                    None
                )
                .await
            ),
            Some(json!(BINDING_CONFLICT_KIND)),
            "no session service"
        );
        assert_eq!(
            attach(Some(true), Some(binding("b")), &session_id).await,
            None,
            "a live session on the launch binding"
        );
        assert_eq!(
            attach(Some(false), Some(binding("a")), &session_id).await,
            None,
            "a session without a live actor is rebuilt under the launch binding"
        );
    }

    /// A live session whose recorded binding cannot be read is refused, not
    /// let through, with the advisory `retryable: true` that tells a host the
    /// same call can succeed once the read does.
    #[tokio::test]
    async fn a_live_session_whose_binding_cannot_be_read_is_refused_as_retryable() {
        let sessions = crate::mob_handle_runtime::failing_metadata_probe();
        let session_id = meerkat_core::types::SessionId::new();
        let refusal = attach_binding_refusal(
            Some(&*sessions),
            "meta/ensure_member",
            &session_id,
            &binding("b"),
            None,
        )
        .await
        .expect("an unread binding is refused");
        assert_eq!(refusal.code(), INVALID_PARAMS);
        assert_eq!(refusal.data()["kind"], json!(BINDING_CONFLICT_KIND));
        assert_eq!(refusal.data()["session_id"], json!(session_id.to_string()));
        assert_eq!(refusal.data()["retryable"], json!(true));
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("cannot be read")),
            "{}",
            refusal.data()
        );
    }

    /// Members' configuration whose binding `g`, named under
    /// `native-team`, is defined on the `global` realm it inherits from;
    /// `native-team` owns only its default binding `a`.
    const INHERITED_BINDING_DOCUMENT: &str = r#"
[realm.native-team]
default_binding = "a"

[realm.native-team.backend.openai_main]
provider = "openai"
backend_kind = "openai_api"

[realm.native-team.auth.key_a]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "NATIVE_REALM_UNIT_KEY_A" }

[realm.native-team.binding.a]
backend_profile = "openai_main"
auth_profile = "key_a"

[realm.global.backend.openai_global]
provider = "openai"
backend_kind = "openai_api"

[realm.global.auth.key_g]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "NATIVE_REALM_UNIT_KEY_G" }

[realm.global.binding.g]
backend_profile = "openai_global"
auth_profile = "key_g"
"#;

    /// A binding named under one realm but defined on a realm it inherits
    /// from (here `[realm.global]`) is recorded by Core under the realm that
    /// owns it. Init resolves that form, and an attach accepts a live session
    /// that records the launch binding in either form; without the resolved
    /// form, the owner-stamped one would read as another binding.
    #[tokio::test]
    async fn an_inherited_launch_binding_is_accepted_in_the_form_core_records() {
        let config: meerkat_core::Config = toml::from_str(INHERITED_BINDING_DOCUMENT)
            .expect("a configuration whose global realm defines a binding");
        let named = binding("g");
        let recorded = validate_member_auth_binding(&config, &named, &definition(OPENAI_PROFILE))
            .expect("the inherited binding resolves");
        assert_eq!(recorded.realm, meerkat_core::RealmId::global());
        assert_eq!(recorded.binding, named.binding);
        assert_eq!(
            validate_member_auth_binding(&config, &binding("a"), &definition(OPENAI_PROFILE))
                .expect("the head realm's own binding"),
            binding("a"),
            "a binding the named realm owns is recorded as named"
        );

        let session_id = meerkat_core::types::SessionId::new();
        for (records, form) in [
            (recorded.clone(), Some(&recorded)),
            (named.clone(), Some(&recorded)),
        ] {
            let sessions =
                crate::mob_handle_runtime::recorded_binding_probe(Some(true), Some(records));
            assert_eq!(
                attach_binding_refusal(
                    Some(&*sessions),
                    "meta/ensure_member",
                    &session_id,
                    &named,
                    form,
                )
                .await,
                None,
                "the launch binding in either form"
            );
        }
        let sessions =
            crate::mob_handle_runtime::recorded_binding_probe(Some(true), Some(recorded.clone()));
        let refusal = attach_binding_refusal(
            Some(&*sessions),
            "meta/ensure_member",
            &session_id,
            &named,
            None,
        )
        .await
        .expect("the owner-stamped form alone reads as another binding");
        assert_eq!(refusal.data()["kind"], json!(BINDING_CONFLICT_KIND));
        let sessions =
            crate::mob_handle_runtime::recorded_binding_probe(Some(true), Some(binding("a")));
        assert!(
            attach_binding_refusal(
                Some(&*sessions),
                "meta/ensure_member",
                &session_id,
                &named,
                Some(&recorded),
            )
            .await
            .is_some(),
            "another binding is still refused"
        );
    }

    /// A store of realm profiles holding `profiles` (name and the inline
    /// profile of `[profiles.worker]` extended by its TOML).
    async fn realm_profiles(profiles: &[(&str, &str)]) -> meerkat_mob::InMemoryRealmProfileStore {
        let store = meerkat_mob::InMemoryRealmProfileStore::new();
        for (name, toml) in profiles {
            let profile = definition(toml).profiles["worker"]
                .as_inline()
                .cloned()
                .expect("an inline profile");
            meerkat_mob::RealmProfileStore::create(&store, name, &profile)
                .await
                .expect("store the realm profile");
        }
        store
    }

    /// The launch definition with one profile, `worker`, that references the
    /// realm profile `realm_profile`.
    fn referencing(realm_profile: &str) -> meerkat_mob::MobDefinition {
        definition(&format!(
            "[profiles.worker]\nrealm_profile = \"{realm_profile}\"\n"
        ))
    }

    /// A realm-referenced profile of the launch definition resolves from the
    /// mob storage's realm profiles at each member's build, so init resolves
    /// it from that store and checks it as an inline profile: a provider the
    /// binding does not serve is `member_auth_binding_mismatch`, and a model
    /// fallback chain off the binding is `member_auth_binding_uncarried`. A
    /// realm profile on the binding passes, and one the store does not hold
    /// is left to Meerkat's build, which refuses it.
    #[tokio::test]
    async fn a_realm_profile_off_the_binding_is_refused_at_init() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let unbound = format!(
            "{OPENAI_PROFILE}[profiles.worker.model_fallback]\nenabled = true\n\
             [[profiles.worker.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n"
        );
        let store = realm_profiles(&[
            ("plain", OPENAI_PROFILE),
            ("unbound-fallback", unbound.as_str()),
            (
                "other-provider",
                "[profiles.worker]\nmodel = \"claude-sonnet-4-5\"\nprovider = \"anthropic\"\n",
            ),
        ])
        .await;
        validate_realm_profiles(&config, &binding("b"), &referencing("plain"), &store)
            .await
            .expect("a realm profile on the binding");
        validate_realm_profiles(&config, &binding("b"), &referencing("absent"), &store)
            .await
            .expect("a realm profile the store does not hold is left to Meerkat's build");

        let refusal = validate_realm_profiles(
            &config,
            &binding("b"),
            &referencing("unbound-fallback"),
            &store,
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);
        assert!(
            refusal.data()["reason"].as_str().is_some_and(|reason| {
                reason.contains("realm profile 'unbound-fallback'")
                    && reason.contains("model_fallback")
            }),
            "{:?}",
            refusal.data()
        );

        let refusal = validate_realm_profiles(
            &config,
            &binding("b"),
            &referencing("other-provider"),
            &store,
        )
        .await
        .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::TargetMismatch);
        assert_eq!(refusal.data()["mob_profile"], json!("worker"));
        assert_eq!(refusal.data()["model"], json!("claude-sonnet-4-5"));
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.starts_with("realm profile 'other-provider'")),
            "{:?}",
            refusal.data()
        );
    }

    /// A realm-referenced profile that enables image generation is refused at
    /// init like an inline one (`member_auth_binding_uncarried`): Meerkat
    /// would build image generation executors by default binding selection.
    #[tokio::test]
    async fn a_realm_profile_enabling_image_generation_is_refused_at_init() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let imaging = format!("{OPENAI_PROFILE}[profiles.worker.tools]\nimage_generation = true\n");
        let store = realm_profiles(&[("imaging", imaging.as_str())]).await;
        let refusal =
            validate_realm_profiles(&config, &binding("b"), &referencing("imaging"), &store)
                .await
                .unwrap_err();
        assert_eq!(refusal.kind, MemberRouteRefusalKind::Uncarried);
        assert!(
            refusal.data()["reason"].as_str().is_some_and(|reason| {
                reason
                    .starts_with("profiles.worker (realm profile 'imaging').tools.image_generation")
            }),
            "{:?}",
            refusal.data()
        );
    }

    /// Identity declarations an earlier launch adopted are materialized from
    /// their sealed declaration without the gateway's customizers, so one
    /// sealed with no binding or another binding holds a member off the
    /// launch binding and is named; one sealed with the launch binding, as
    /// named or in the form Core records, is not.
    #[test]
    fn declarations_sealed_off_the_binding_are_named() {
        let launch = binding("b");
        let mut owner_form = binding("b");
        owner_form.realm = meerkat_core::RealmId::global();
        let declaration = |identity: &str, sealed: Option<meerkat_core::AuthBindingRef>| {
            (
                identity.to_string(),
                Some(format!("{identity}-scope")),
                sealed,
            )
        };
        assert_eq!(
            declarations_off_binding(
                [
                    declaration("named", Some(binding("b"))),
                    declaration("recorded", Some(owner_form.clone())),
                ],
                &launch,
                Some(&owner_form),
            ),
            None,
            "the launch binding in either form"
        );
        let reason = declarations_off_binding(
            [
                declaration("unsealed", None),
                declaration("elsewhere", Some(binding("a"))),
                declaration("named", Some(binding("b"))),
            ],
            &launch,
            None,
        )
        .expect("declarations off the binding are named");
        assert!(
            reason.contains(
                "identity 'unsealed' (declaration 'unsealed-scope') sealed with no binding"
            ) && reason.contains("identity 'elsewhere'")
                && reason.contains("binding 'a'")
                && !reason.contains("identity 'named'"),
            "{reason}"
        );
    }

    /// A stored child mob definition is checked as `mob_create` checks a new
    /// one: an automatic spawn policy, a fallback target off the binding and
    /// image generation each hold its members off the binding; a plain child
    /// passes.
    #[test]
    fn a_stored_child_definition_off_the_binding_is_named() {
        let launch = binding("b");
        assert_eq!(
            stored_definition_escape(&definition(OPENAI_PROFILE), &launch, None),
            None
        );
        let image = definition(&format!(
            "{OPENAI_PROFILE}[profiles.worker.tools]\nimage_generation = true\n"
        ));
        assert!(
            stored_definition_escape(&image, &launch, None)
                .is_some_and(|reason| reason.starts_with("profiles.worker.tools.image_generation"))
        );
        let fallback = definition(&format!(
            "[runtime.model_fallback]\nenabled = true\n\
             [[runtime.model_fallback.chain]]\nmodel = \"gpt-5.4\"\n\n{OPENAI_PROFILE}"
        ));
        assert!(
            stored_definition_escape(&fallback, &launch, None)
                .is_some_and(|reason| reason.starts_with("runtime.model_fallback"))
        );
    }

    /// A build of `model` through the gateway's session services with
    /// `build`.
    fn build_request(
        model: &str,
        build: meerkat_core::service::SessionBuildOptions,
    ) -> meerkat_core::service::CreateSessionRequest {
        meerkat_core::service::CreateSessionRequest {
            model: model.to_string(),
            prompt: meerkat_core::ContentInput::Text("build".to_string()),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            build: Some(build),
            labels: None,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            injected_context: Vec::new(),
        }
    }

    /// A session on the OpenAI `model` whose recorded tooling enables web
    /// search.
    fn session_recording_web_search(model: &str) -> meerkat_core::Session {
        let mut session = meerkat_core::Session::new();
        let tooling = meerkat_core::SessionTooling {
            web_search: meerkat_core::ToolCategoryOverride::Enable,
            ..Default::default()
        };
        let metadata = meerkat_core::SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: model.to_string(),
            model_fallback: None,
            max_tokens: 1024,
            structured_output_retries: meerkat_core::config::default_structured_output_retries(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            tooling,
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        };
        session
            .set_session_metadata(metadata)
            .expect("record the tooling");
        session
    }

    /// The build seam's check without the members' configuration, where no
    /// web search is known to be the provider's own: web search a resumed
    /// session's recorded tooling turns back on (when the build leaves it to
    /// the session), image generation and a fallback target off the binding
    /// each refuse the build; a build that decides web search off, or records
    /// nothing, passes.
    #[test]
    fn a_member_build_off_the_binding_is_named() {
        use meerkat_core::ToolCategoryOverride as Category;
        let guard = MemberBuildGuard::new(binding("b"), None, None);
        let build = |web_search: Category, resume: bool| {
            build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    override_web_search: web_search,
                    resume_session: resume.then(|| session_recording_web_search("gpt-5.5")),
                    ..Default::default()
                },
            )
        };
        assert!(
            guard
                .escape(&build(Category::Inherit, true))
                .is_some_and(|reason| reason.contains("web search"))
        );
        assert!(guard.escape(&build(Category::Enable, false)).is_some());
        assert_eq!(guard.escape(&build(Category::Disable, true)), None);
        assert_eq!(guard.escape(&build(Category::Inherit, false)), None);
        let image = build_request(
            "gpt-5.5",
            meerkat_core::service::SessionBuildOptions {
                override_image_generation: Category::Enable,
                ..Default::default()
            },
        );
        assert!(
            guard
                .escape(&image)
                .is_some_and(|reason| reason.starts_with("tools.image_generation"))
        );
        let fallback = build_request(
            "gpt-5.5",
            meerkat_core::service::SessionBuildOptions {
                model_fallback: Some(meerkat_core::config::ModelFallbackConfig {
                    enabled: Some(true),
                    chain: vec![meerkat_core::config::ModelFallbackTarget {
                        model: "gpt-5.4".to_string(),
                        provider: None,
                        auth_binding: None,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
        assert!(
            guard
                .escape(&fallback)
                .is_some_and(|reason| reason.starts_with("model_fallback"))
        );
    }

    /// A member Meerkat rebuilds from its own records, where no spawn
    /// customizer is asked (a fork seated before the launch named the
    /// binding), builds with the binding its session recorded, or by default
    /// selection with none. Such a member build is refused typed unless it
    /// runs on the launch binding, as named or in the form Core records it;
    /// a build that is not a member's is not judged by its binding.
    #[test]
    fn a_member_rebuilt_from_its_records_off_the_binding_is_refused() {
        let launch = binding("b");
        let mut owner_form = binding("b");
        owner_form.realm = meerkat_core::RealmId::global();
        let guard = MemberBuildGuard::new(launch.clone(), Some(owner_form.clone()), None);
        let member = meerkat_core::MobMemberBinding {
            mob_id: "main".to_string(),
            role: "worker".to_string(),
            member: "forked".to_string(),
        };
        let source = meerkat_core::MobMemberBinding {
            member: "source".to_string(),
            ..member.clone()
        };
        let rebuild = |auth_binding: Option<meerkat_core::AuthBindingRef>| {
            build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    mob_member_binding: Some(member.clone()),
                    fork_source: Some(meerkat_core::service::ForkBuildSource::new(
                        source.clone(),
                        meerkat_core::types::SessionId::new(),
                    )),
                    auth_binding,
                    ..Default::default()
                },
            )
        };
        let unbound = guard
            .escape(&rebuild(None))
            .expect("no binding is default selection");
        assert!(unbound.contains("no auth binding"), "{unbound}");
        let other = guard
            .escape(&rebuild(Some(binding("a"))))
            .expect("another binding");
        assert!(other.contains("binding 'a'"), "{other}");
        assert_eq!(
            guard.escape(&rebuild(Some(launch))),
            None,
            "the binding as named"
        );
        assert_eq!(
            guard.escape(&rebuild(Some(owner_form))),
            None,
            "the binding as Core records it"
        );
        let refusal = guard.refusal(&rebuild(None)).expect("a typed refusal");
        assert!(
            matches!(&refusal, meerkat_core::service::SessionError::FailedWithData { data, .. }
                if data["kind"] == json!(BINDING_UNCARRIED_KIND)),
            "{refusal:?}"
        );
        assert!(
            refusal
                .to_string()
                .starts_with("member_auth_binding_uncarried:"),
            "{refusal}"
        );
        assert_eq!(
            guard.escape(&build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions::default()
            )),
            None,
            "a build that is not a member's"
        );
    }

    /// Under a member auth binding only Meerkat's fallback web search is
    /// refused. A model whose native search the provider serves on the
    /// member's binding passes, as does a resumed member whose recorded
    /// tooling enables search on such a model. A model without native
    /// search, a resumed session recorded on one, a build whose tool
    /// governance disables the provider's native tools, and a build off the
    /// launch binding would get the fallback executor and are refused.
    #[tokio::test]
    async fn only_the_fallback_web_search_is_refused() {
        use meerkat_core::ToolCategoryOverride as Category;
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let guard = MemberBuildGuard::new(binding("b"), None, Some(Arc::new(config)));
        let search = |model: &str, on: &str| {
            build_request(
                model,
                meerkat_core::service::SessionBuildOptions {
                    override_web_search: Category::Enable,
                    auth_binding: Some(binding(on)),
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            guard.escape(&search("gpt-5.5", "b")),
            None,
            "the provider's own search on the member's binding"
        );
        assert!(
            guard
                .escape(&search("gpt-5.3-codex", "b"))
                .is_some_and(|reason| reason.contains("fallback web search")),
            "a model without native search"
        );
        assert!(
            guard.escape(&search("gpt-5.5", "a")).is_some(),
            "a build off the launch binding"
        );
        let mut governed = search("gpt-5.5", "b");
        if let Some(build) = governed.build.as_mut() {
            build.application_tool_policy = meerkat_core::ApplicationToolPolicyBinding::Inherit;
        }
        assert!(
            guard.escape(&governed).is_some(),
            "tool governance disables the provider's native tools"
        );
        let resumed = |recorded_model: &str| {
            build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    resume_session: Some(session_recording_web_search(recorded_model)),
                    auth_binding: Some(binding("b")),
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            guard.escape(&resumed("gpt-5.5")),
            None,
            "recorded search on a model with native search"
        );
        assert!(
            guard.escape(&resumed("gpt-5.3-codex")).is_some(),
            "recorded search on a model without it"
        );
    }

    /// A fallback target that names the launch binding in the form Core
    /// records it, stamped with the realm that owns the binding, is on the
    /// binding, as an attach and a declaration accept that form: init, a
    /// stored or new child definition, an agent's spawn naming it and a
    /// member build all pass it, and another binding is still refused.
    #[test]
    fn the_binding_in_its_recorded_form_is_accepted_on_fallback_targets() {
        let config: meerkat_core::Config = toml::from_str(INHERITED_BINDING_DOCUMENT)
            .expect("a configuration whose global realm defines a binding");
        let named = binding("g");
        let chain = |target: &str| {
            definition(&format!(
                "[runtime.model_fallback]\nenabled = true\n\
                 [[runtime.model_fallback.chain]]\nmodel = \"gpt-5.4\"\nauth_binding = {target}\n\n\
                 {OPENAI_PROFILE}"
            ))
        };
        let owner = "{ realm = \"global\", binding = \"g\" }";
        let recorded = validate_member_auth_binding(&config, &named, &chain(owner))
            .expect("a fallback target in the recorded form is on the binding");
        assert_eq!(recorded.realm, meerkat_core::RealmId::global());
        assert!(
            validate_member_auth_binding(
                &config,
                &named,
                &chain("{ realm = \"native-team\", binding = \"a\" }"),
            )
            .is_err(),
            "another binding is still refused"
        );
        assert_eq!(
            stored_definition_escape(&chain(owner), &named, Some(&recorded)),
            None
        );
        assert!(
            stored_definition_escape(&chain(owner), &named, None).is_some(),
            "without the recorded form the owner-stamped target reads as another binding"
        );
        let child = json!({ "id": "child", "profiles": { "worker": {
            "model": "gpt-5.5",
            "model_fallback": {
                "enabled": true,
                "chain": [{
                    "model": "gpt-5.4",
                    "auth_binding": { "realm": "global", "binding": "g" },
                }],
            },
        } } });
        assert_eq!(
            child_mob_definition_escape(&child, &named, Some(&recorded)),
            None
        );
        let spawn = json!({
            "mob_id": "child",
            "profile": "worker",
            "member_id": "w1",
            "auth_binding": { "realm": "global", "binding": "g" },
        });
        assert_eq!(
            agent_tool_binding_refusal("mob_spawn_member", &spawn, &named, Some(&recorded)),
            None,
            "an agent's spawn naming the recorded form"
        );
        let guard = MemberBuildGuard::new(named, None, Some(Arc::new(config)));
        assert_eq!(
            guard.recorded_form(),
            Some(&recorded),
            "the guard resolves the form init resolves"
        );
        let fallback = |target: meerkat_core::AuthBindingRef| {
            build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    model_fallback: Some(meerkat_core::config::ModelFallbackConfig {
                        enabled: Some(true),
                        chain: vec![meerkat_core::config::ModelFallbackTarget {
                            model: "gpt-5.4".to_string(),
                            provider: None,
                            auth_binding: Some(target),
                        }],
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
        };
        assert_eq!(guard.escape(&fallback(recorded)), None);
        assert!(guard.escape(&fallback(binding("a"))).is_some());
    }

    /// A realm profile store that cannot be read.
    struct UnreadableRealmProfiles;

    #[async_trait::async_trait]
    impl meerkat_mob::RealmProfileStore for UnreadableRealmProfiles {
        async fn create(
            &self,
            _name: &str,
            _profile: &meerkat_mob::Profile,
        ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
            Err(meerkat_mob::MobStoreError::WriteFailed("disk".to_string()))
        }

        async fn get(
            &self,
            _name: &str,
        ) -> Result<Option<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobStoreError> {
            Err(meerkat_mob::MobStoreError::ReadFailed("disk".to_string()))
        }

        async fn list(
            &self,
        ) -> Result<Vec<meerkat_mob::StoredRealmProfile>, meerkat_mob::MobStoreError> {
            Err(meerkat_mob::MobStoreError::ReadFailed("disk".to_string()))
        }

        async fn update(
            &self,
            _name: &str,
            _profile: &meerkat_mob::Profile,
            _expected_revision: u64,
        ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
            Err(meerkat_mob::MobStoreError::WriteFailed("disk".to_string()))
        }

        async fn delete(
            &self,
            _name: &str,
            _expected_revision: u64,
        ) -> Result<meerkat_mob::StoredRealmProfile, meerkat_mob::MobStoreError> {
            Err(meerkat_mob::MobStoreError::WriteFailed("disk".to_string()))
        }
    }

    /// A read of the launch's state that init's binding checks need is a
    /// storage fault, not a caller error: a realm profile the store cannot
    /// read refuses the launch as `native_realm_unreadable`, with the storage
    /// resolution code.
    #[tokio::test]
    async fn a_state_read_failure_answers_the_storage_code() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let refusal = validate_realm_profiles(
            &config,
            &binding("b"),
            &referencing("imaging"),
            &UnreadableRealmProfiles,
        )
        .await
        .expect_err("an unreadable realm profile refuses the launch");
        assert_eq!(refusal.kind, MemberRouteRefusalKind::RealmUnreadable);
        assert_eq!(refusal.code(), crate::STORAGE_RESOLUTION_CODE);
        assert_eq!(refusal.data()["kind"], json!("native_realm_unreadable"));
        assert!(
            refusal.data()["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("realm profile 'imaging'")),
            "{:?}",
            refusal.data()
        );
    }

    /// A member build refusal reaches a spawn caller with its kind and
    /// reason as data when Meerkat hands it back as its typed session error;
    /// another spawn error keeps no data.
    #[test]
    fn a_member_build_refusal_keeps_its_data_for_the_spawn_caller() {
        let guard = MemberBuildGuard::new(binding("b"), None, None);
        let refusal = guard
            .refusal(&build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    override_image_generation: meerkat_core::ToolCategoryOverride::Enable,
                    ..Default::default()
                },
            ))
            .expect("refused");
        let spawn_error = |error: meerkat_core::service::SessionError| {
            crate::mob_handle_runtime::MobRuntimeError::Mob(meerkat_mob::MobError::SessionError(
                error,
            ))
        };
        let data = member_build_refusal_data(&spawn_error(refusal)).expect("the refusal's data");
        assert_eq!(data["kind"], json!(BINDING_UNCARRIED_KIND));
        assert!(
            data["reason"]
                .as_str()
                .is_some_and(|reason| reason.starts_with("tools.image_generation")),
            "{data}"
        );
        let other = spawn_error(meerkat_core::service::SessionError::FailedWithData {
            message: "another refusal".to_string(),
            data: json!({ "kind": "another_kind" }),
        });
        assert_eq!(member_build_refusal_data(&other), None);
    }

    /// The native search decision reads each build's own model registry, as
    /// the factory does: the members' configuration with the build's
    /// `custom_models`, the `[models]` of the member's own mob definition. Two
    /// definitions (the root's and a child's, say) that describe one model
    /// differently get different answers: the build whose entry declares
    /// native web search passes, the build whose entry does not is refused,
    /// and so is a build whose definition does not define the model at all.
    #[tokio::test]
    async fn native_search_is_judged_on_the_builds_own_models() {
        use meerkat_core::ToolCategoryOverride as Category;
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let guard = MemberBuildGuard::new(binding("b"), None, Some(Arc::new(config)));
        let models = |web_search: Option<bool>| {
            let mut models = std::collections::BTreeMap::new();
            if let Some(web_search) = web_search {
                models.insert(
                    "team-search-model".to_string(),
                    serde_json::from_value::<meerkat_core::config::CustomModelConfig>(json!({
                        "provider": "openai",
                        "web_search": web_search,
                    }))
                    .expect("a custom model"),
                );
            }
            models
        };
        let search = |custom_models: std::collections::BTreeMap<
            String,
            meerkat_core::config::CustomModelConfig,
        >| {
            build_request(
                "team-search-model",
                meerkat_core::service::SessionBuildOptions {
                    override_web_search: Category::Enable,
                    auth_binding: Some(binding("b")),
                    custom_models,
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            guard.escape(&search(models(Some(true)))),
            None,
            "a definition whose entry declares native web search"
        );
        assert!(
            guard
                .escape(&search(models(Some(false))))
                .is_some_and(|reason| reason.contains("fallback web search")),
            "a definition whose entry does not"
        );
        assert!(
            guard.escape(&search(models(None))).is_some(),
            "a definition that does not define the model"
        );
    }

    /// A resumed member's search is judged on the identity the factory
    /// builds with: the build's, merged with the session's by Core's resume
    /// rule. A member whose profile moved to a model with native search (the
    /// build claims the model) passes although its session recorded one
    /// without; when the build leaves the model to the session, the recorded
    /// model decides.
    #[tokio::test]
    async fn a_resumed_member_is_judged_on_the_merged_identity() {
        let root = tempfile::tempdir().unwrap();
        let config = read_native_realm_config(&native_realm_on_disk(root.path()).await)
            .await
            .expect("the realm");
        let guard = MemberBuildGuard::new(binding("b"), None, Some(Arc::new(config)));
        let resumed = |claims_model: bool| {
            build_request(
                "gpt-5.5",
                meerkat_core::service::SessionBuildOptions {
                    resume_session: Some(session_recording_web_search("gpt-5.3-codex")),
                    auth_binding: Some(binding("b")),
                    resume_override_mask: meerkat_core::service::ResumeOverrideMask {
                        model: claims_model,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
        };
        assert_eq!(
            guard.escape(&resumed(true)),
            None,
            "the profile's model, which has native search"
        );
        assert!(
            guard.escape(&resumed(false)).is_some(),
            "the recorded model, which has none"
        );
    }
}
