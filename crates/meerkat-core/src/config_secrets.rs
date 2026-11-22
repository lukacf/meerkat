//! Secret values of the configuration document: where they sit, what an
//! answer shows of them and what a write keeps of them.
//!
//! PLACES. The document holds a secret value at a fixed set of places: the
//! inline secret of an auth source and the environment values of a
//! credential command; the environment values of a stdio MCP server and the
//! header values of an HTTP one; the environment values of a command hook
//! and the header values of an HTTP one; the environment values of a stdio
//! skills repository, the bearer token or header value of an HTTP one and
//! the token of a git one. This module names each of them once
//! (`source_secrets`, `server_secrets`, `hook_secrets`,
//! `repository_secrets`, `SecretSections::of`), destructuring with no
//! rest pattern and matching with no wildcard arm over a source, a
//! transport or a runtime: a new section, field, source, transport or
//! runtime does not compile until it is placed. What is not a secret value
//! stays as written: an argument list, an address, a free option value.
//!
//! ANSWERS. [`mask_config_secrets`] puts [`CONFIG_SECRET_MARKER`] in place
//! of every secret value; the names of a value map stay.
//!
//! WRITES. A write may carry the marker where an answer showed it:
//! [`resolve_written_config`] (a full replace) and
//! [`resolve_written_patch`] (a merge patch) put the stored value of the
//! SAME place back, or refuse the write:
//!
//! - an auth source's marker stands for the stored value of the same
//!   profile of the same realm, of the same kind of source and, in a
//!   command's environment, under the same name;
//! - inside a list the marker stands for a stored value of the same entry:
//!   the one stored entry that carries the written entry's own key (an MCP
//!   server's name, a hook's id, a skills repository's source id with its
//!   name), when exactly one stored and one written entry carry it. Never by
//!   position: a list may be written in another order;
//! - of the same kind of entry (transport or runtime), at the same place
//!   (the same name in a value map, the same kind of credential);
//! - a stored secret value stays with the address it was stored for: the
//!   marker is not resolved in an entry whose URL the write changes (an HTTP
//!   MCP server, an HTTP hook, an HTTP or git skills repository). The write
//!   sends the values again with the new address;
//! - the same holds outside the lists for an auth profile's stored inline
//!   secret: a write that changes where it is sent (the profile's provider
//!   or auth method; for a binding that uses the profile, its backend, that
//!   backend's entry, the self-hosted server the backend names or, for a
//!   self-hosted backend that names none, the whole `[self_hosted]`
//!   section, its default model) is refused unless the write itself states
//!   the secret as a value, with or without a marker in it;
//! - a patch that makes an auth profile an inline secret states the secret
//!   itself: a `secret` key that an earlier form of the profile left in the
//!   document is not taken up;
//! - the marker text is never stored: where nothing is stored that it can
//!   stand for, in a secret place or anywhere else in what the write
//!   writes, the write is refused.
//!
//! A refusal is [`ConfigError::Validation`]; it names the entry and the
//! place, never a value, and nothing is written.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::config::{
    CommandRuntimeConfig, Config, ConfigError, HookAdapterConfig, HookEntryConfig, HooksConfig,
    HttpRuntimeConfig, SelfHostedConfig, SelfHostedServerConfig, ToolsConfig,
};
use crate::connection::{
    AuthProfileConfig, BackendProfileConfig, CredentialSourceSpec, RealmConfigSection,
};
use crate::mcp_config::{McpHttpConfig, McpServerConfig, McpStdioConfig, McpTransportConfig};
use crate::skills_config::{
    GitSkillRepoAuth, HttpSkillRepoAuth, SkillRepoTransport, SkillRepositoryConfig, SkillsConfig,
};

/// The text a configuration answer shows in place of a stored secret value,
/// and that a write may send back in its place. A write never stores it.
pub const CONFIG_SECRET_MARKER: &str = crate::redact::REDACTED;

/// What an entry's secret values are sent to, or handed to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SecretTarget {
    /// The address the values are sent to, by a transport (`http`, `git`).
    Url {
        transport: &'static str,
        url: String,
    },
    /// A process the entry starts. What it runs is not part of the target:
    /// a write may change a command or its arguments and keep the
    /// environment values.
    Process,
}

/// How a written entry differs from the stored one in where its secret
/// values go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Moved {
    /// Another URL by the same transport.
    Url,
    /// Another kind of entry: another transport or runtime.
    Kind,
}

/// Whether an entry stored with one target and written with another still
/// sends its secret values where they were stored for (`None`: a transport
/// or runtime that holds no secret value).
fn moved(stored: Option<&SecretTarget>, written: Option<&SecretTarget>) -> Option<Moved> {
    match (stored, written) {
        (None, None) | (Some(SecretTarget::Process), Some(SecretTarget::Process)) => None,
        (
            Some(SecretTarget::Url { transport, url }),
            Some(SecretTarget::Url {
                transport: written_transport,
                url: written_url,
            }),
        ) => {
            if transport != written_transport {
                Some(Moved::Kind)
            } else if url != written_url {
                Some(Moved::Url)
            } else {
                None
            }
        }
        (None | Some(SecretTarget::Process), Some(SecretTarget::Url { .. }))
        | (None | Some(SecretTarget::Url { .. }), Some(SecretTarget::Process))
        | (Some(_), None) => Some(Moved::Kind),
    }
}

/// One secret value's place below its entry (or below its auth source).
#[derive(Clone, Copy)]
struct SecretPlace<'p> {
    /// The path of the value in the document's JSON form: `headers`, then
    /// the header's name.
    path: &'p [&'p str],
    /// What tells the value from another kind of value at the same path: a
    /// repository credential's scheme and, for a header credential, the
    /// header's name. Empty for a value map, whose path ends in the name.
    scheme: &'p str,
}

/// Visits one secret value: its place and the value.
type VisitSecret<'v> = &'v mut dyn FnMut(SecretPlace<'_>, &mut String);

/// Visit each value of a map of secret values, placed under `map`.
fn visit_values<'m>(
    map: &[&str],
    values: impl Iterator<Item = (&'m String, &'m mut String)>,
    visit: VisitSecret<'_>,
) {
    for (name, value) in values {
        let mut path = map.to_vec();
        path.push(name.as_str());
        visit(
            SecretPlace {
                path: &path,
                scheme: "",
            },
            value,
        );
    }
}

/// The secret values of an auth source.
fn source_secrets(source: &mut CredentialSourceSpec, visit: VisitSecret<'_>) {
    match source {
        CredentialSourceSpec::InlineSecret { secret } => visit(
            SecretPlace {
                path: &["secret"],
                scheme: "",
            },
            secret,
        ),
        CredentialSourceSpec::Command {
            program: _,
            args: _,
            cwd: _,
            env,
            timeout_ms: _,
            refresh_interval_ms: _,
        } => visit_values(&["env"], env.iter_mut(), visit),
        // Names, handles and descriptors: the value is not in the document.
        CredentialSourceSpec::ManagedStore
        | CredentialSourceSpec::Env {
            env: _,
            fallback: _,
        }
        | CredentialSourceSpec::ExternalResolver { handle: _ }
        | CredentialSourceSpec::PlatformDefault
        | CredentialSourceSpec::FileDescriptor {
            fd: _,
            scope_override: _,
        } => {}
    }
}

/// The secret values of an MCP server, and what they are sent or handed to.
fn server_secrets(server: &mut McpServerConfig, visit: VisitSecret<'_>) -> Option<SecretTarget> {
    let McpServerConfig {
        name: _,
        transport,
        connect_timeout_secs: _,
        tool_names: _,
        access: _,
        connector: _,
        account: _,
    } = server;
    Some(match transport {
        McpTransportConfig::Stdio(McpStdioConfig {
            command: _,
            args: _,
            env,
            // Names only: the values are in the realm's credential store.
            secret_env: _,
            cwd: _,
        }) => {
            visit_values(&["env"], env.iter_mut(), visit);
            SecretTarget::Process
        }
        McpTransportConfig::Http(McpHttpConfig {
            url,
            headers,
            transport: _,
            // An account's token is in the credential store, under a key
            // that holds the URL: it is not read for another address.
            oauth_account: _,
            oauth_account_selection: _,
        }) => {
            visit_values(&["headers"], headers.iter_mut(), visit);
            SecretTarget::Url {
                transport: "http",
                url: url.clone(),
            }
        }
    })
}

/// The secret values of a hook, and what they are sent or handed to; `None`
/// for a runtime that holds none.
fn hook_secrets(hook: &mut HookEntryConfig, visit: VisitSecret<'_>) -> Option<SecretTarget> {
    let HookEntryConfig {
        id: _,
        enabled: _,
        point: _,
        mode: _,
        capability: _,
        priority: _,
        timeout_ms: _,
        model_operations: _,
        runtime,
    } = hook;
    match runtime {
        HookAdapterConfig::InProcess(_) => None,
        HookAdapterConfig::Command(CommandRuntimeConfig {
            command: _,
            args: _,
            env,
        }) => {
            visit_values(&["runtime", "env"], env.iter_mut(), visit);
            Some(SecretTarget::Process)
        }
        HookAdapterConfig::Http(HttpRuntimeConfig {
            url,
            method: _,
            headers,
        }) => {
            visit_values(&["runtime", "headers"], headers.iter_mut(), visit);
            Some(SecretTarget::Url {
                transport: "http",
                url: url.clone(),
            })
        }
    }
}

/// The secret values of a skills repository, and what they are sent or
/// handed to; `None` for a transport that holds none.
fn repository_secrets(
    repository: &mut SkillRepositoryConfig,
    visit: VisitSecret<'_>,
) -> Option<SecretTarget> {
    let SkillRepositoryConfig {
        name: _,
        source_uuid: _,
        transport,
    } = repository;
    match transport {
        SkillRepoTransport::Filesystem { path: _ } => None,
        SkillRepoTransport::Stdio {
            command: _,
            args: _,
            cwd: _,
            env,
            timeout_seconds: _,
        } => {
            visit_values(&["env"], env.iter_mut(), visit);
            Some(SecretTarget::Process)
        }
        SkillRepoTransport::Http {
            url,
            auth,
            refresh_seconds: _,
            timeout_seconds: _,
        } => {
            match auth {
                Some(HttpSkillRepoAuth::Bearer { token }) => visit(
                    SecretPlace {
                        path: &["auth", "token"],
                        scheme: "bearer",
                    },
                    token,
                ),
                Some(HttpSkillRepoAuth::Header { name, value }) => visit(
                    SecretPlace {
                        path: &["auth", "value"],
                        scheme: &format!("header {name}"),
                    },
                    value,
                ),
                None => {}
            }
            Some(SecretTarget::Url {
                transport: "http",
                url: url.clone(),
            })
        }
        SkillRepoTransport::Git {
            url,
            git_ref: _,
            ref_type: _,
            skills_root: _,
            auth,
            refresh_seconds: _,
            depth: _,
        } => {
            match auth {
                Some(GitSkillRepoAuth::Token { token }) => visit(
                    SecretPlace {
                        path: &["auth", "token"],
                        scheme: "token",
                    },
                    token,
                ),
                // A path, not a value.
                Some(GitSkillRepoAuth::SshKey { path: _ }) | None => {}
            }
            Some(SecretTarget::Url {
                transport: "git",
                url: url.clone(),
            })
        }
    }
}

/// The sections of the document that hold secret values. Every section of
/// the document, and every field of the three sections that hold a list, is
/// named with no rest pattern: a new one does not compile until it is
/// placed as holding a secret value or not.
struct SecretSections<'c> {
    realms: &'c mut BTreeMap<String, RealmConfigSection>,
    servers: &'c mut Vec<McpServerConfig>,
    hooks: &'c mut Vec<HookEntryConfig>,
    repositories: &'c mut Vec<SkillRepositoryConfig>,
}

impl<'c> SecretSections<'c> {
    fn of(config: &'c mut Config) -> Self {
        let Config {
            agent: _,
            storage: _,
            budget: _,
            retry: _,
            tools,
            models: _,
            max_tokens: _,
            shell: _,
            store: _,
            // The pairing passwords of these two are never serialized.
            comms: _,
            mob_host: _,
            compaction: _,
            limits: _,
            rest: _,
            hooks,
            skills,
            // Addresses and model names.
            self_hosted: _,
            provider_tools: _,
            model_fallback: _,
            presentation: _,
            governance: _,
            confinement: _,
            realm,
        } = config;
        let ToolsConfig {
            mcp_servers,
            default_timeout: _,
            tool_timeouts: _,
            max_concurrent: _,
            builtins_enabled: _,
            shell_enabled: _,
            comms_enabled: _,
            mob_enabled: _,
            schedule_enabled: _,
            agent_schedules_may_create_sessions: _,
            workgraph_enabled: _,
        } = tools;
        let HooksConfig {
            default_timeout_ms: _,
            payload_max_bytes: _,
            background_max_concurrency: _,
            entries,
        } = hooks;
        let SkillsConfig {
            enabled: _,
            max_injection_bytes: _,
            inventory_threshold: _,
            repositories,
            health_thresholds: _,
            identity: _,
        } = skills;
        Self {
            realms: realm,
            servers: mcp_servers,
            hooks: entries,
            repositories,
        }
    }
}

/// The auth sources of a realm section: each with its path below the
/// section and what a refusal calls it. Every field of the section is named
/// with no rest pattern.
fn realm_sources(
    section: &mut RealmConfigSection,
) -> Vec<(Vec<String>, String, &mut CredentialSourceSpec)> {
    let RealmConfigSection {
        // An address and free options: not secret values.
        backend: _,
        auth,
        binding: _,
        default_binding: _,
        parent: _,
        // Never holds a client secret.
        connector_client: _,
        copilot,
    } = section;
    let mut sources: Vec<(Vec<String>, String, &mut CredentialSourceSpec)> = auth
        .iter_mut()
        .map(|(name, profile)| {
            (
                vec!["auth".to_string(), name.clone(), "source".to_string()],
                format!("the auth profile `{name}`"),
                &mut profile.source,
            )
        })
        .collect();
    if let Some(copilot) = copilot {
        sources.push((
            vec!["copilot".to_string(), "source".to_string()],
            "the Copilot account".to_string(),
            &mut copilot.source,
        ));
    }
    sources
}

/// Put [`CONFIG_SECRET_MARKER`] in place of every secret value of `config`:
/// the document a configuration answer shows. The names of a value map
/// stay, and so does everything that is not a secret value.
pub fn mask_config_secrets(config: &mut Config) {
    let mut mask = |_: SecretPlace<'_>, value: &mut String| {
        CONFIG_SECRET_MARKER.clone_into(value);
    };
    let sections = SecretSections::of(config);
    for section in sections.realms.values_mut() {
        for (_, _, source) in realm_sources(section) {
            source_secrets(source, &mut mask);
        }
    }
    for server in sections.servers {
        server_secrets(server, &mut mask);
    }
    for hook in sections.hooks {
        hook_secrets(hook, &mut mask);
    }
    for repository in sections.repositories {
        repository_secrets(repository, &mut mask);
    }
}

/// Whether `value` (a document or a merge patch in JSON form) holds the
/// marker text as a whole string value anywhere: only then does a write
/// have a marker to resolve.
pub fn holds_secret_marker(value: &Value) -> bool {
    marker_path(value, &mut Vec::new()).is_some()
}

/// Whether a full replace by `written` is judged against the stored
/// document: it carries the marker, or it states an auth profile with an
/// inline secret (whose route the write may change). Any other full
/// replace keeps nothing of the stored document's secrets and reads
/// nothing more.
pub fn replace_reads_stored(written: &Config) -> Result<bool, ConfigError> {
    let states_an_inline_secret = written.realm.values().any(|section| {
        section
            .auth
            .values()
            .any(|profile| matches!(profile.source, CredentialSourceSpec::InlineSecret { .. }))
    });
    Ok(states_an_inline_secret || holds_secret_marker(&serde_json::to_value(written)?))
}

/// Whether a merge `patch` is judged against the stored document: it
/// carries the marker, or it names a section that decides where a stored
/// inline secret is sent (`realm`, `self_hosted`). Any other patch reads
/// nothing more.
pub fn patch_reads_stored(patch: &Value) -> bool {
    patch.get("realm").is_some() || patch.get("self_hosted").is_some() || holds_secret_marker(patch)
}

/// The path of the first whole string value of `value` that is the marker
/// text, as a refusal names it.
fn marker_path(value: &Value, path: &mut Vec<String>) -> Option<String> {
    match value {
        Value::String(text) => (text == CONFIG_SECRET_MARKER).then(|| path.join(".")),
        Value::Array(items) => items.iter().enumerate().find_map(|(index, item)| {
            path.push(index.to_string());
            let found = marker_path(item, path);
            path.pop();
            found
        }),
        Value::Object(members) => members.iter().find_map(|(name, member)| {
            path.push(name.clone());
            let found = marker_path(member, path);
            path.pop();
            found
        }),
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

/// The value at `path` of `root`: object members by name, list items by
/// index.
fn value_at_mut<'v>(root: &'v mut Value, path: &[String]) -> Option<&'v mut Value> {
    path.iter().try_fold(root, |value, step| match value {
        Value::Object(members) => members.get_mut(step),
        Value::Array(items) => items.get_mut(step.parse::<usize>().ok()?),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
    })
}

/// A stored secret value, by its place.
struct StoredSecret {
    path: Vec<String>,
    scheme: String,
    value: String,
}

impl StoredSecret {
    fn is_at(&self, place: SecretPlace<'_>) -> bool {
        self.scheme == place.scheme
            && self
                .path
                .iter()
                .map(String::as_str)
                .eq(place.path.iter().copied())
    }
}

/// Collects the secret values a visit passes.
fn collect(into: &mut Vec<StoredSecret>) -> impl FnMut(SecretPlace<'_>, &mut String) + '_ {
    move |place: SecretPlace<'_>, value: &mut String| {
        into.push(StoredSecret {
            path: place.path.iter().map(ToString::to_string).collect(),
            scheme: place.scheme.to_string(),
            value: value.clone(),
        });
    }
}

const SEND_THE_VALUES: &str = "Send the secret values themselves. Nothing was written.";

/// One list of the document whose entries hold secret values.
struct SecretList<E> {
    /// The list's path in the document.
    path: [&'static str; 2],
    /// What the entry's own key is called.
    key_name: &'static str,
    /// The entry's own key.
    key: fn(&E) -> String,
    /// A word on the key for a refusal of a key several entries carry.
    key_note: &'static str,
    /// Whether the written entry of a key carries another name than the
    /// stored one, where an entry has a name beside its key.
    renamed: fn(&E, &E) -> bool,
    /// The entry as a refusal names it.
    describe: fn(&E) -> String,
    secrets: fn(&mut E, VisitSecret<'_>) -> Option<SecretTarget>,
}

const SERVERS: SecretList<McpServerConfig> = SecretList {
    path: ["tools", "mcp_servers"],
    key_name: "name",
    key: |server| server.name.clone(),
    key_note: "",
    renamed: |_, _| false,
    describe: |server| format!("the MCP server `{}` (`tools.mcp_servers`)", server.name),
    secrets: server_secrets,
};

const HOOKS: SecretList<HookEntryConfig> = SecretList {
    path: ["hooks", "entries"],
    key_name: "id",
    key: |hook| hook.id.to_string(),
    key_note: " (an entry written without an id takes the id `hook`)",
    renamed: |_, _| false,
    describe: |hook| format!("the hook `{}` (`hooks.entries`)", hook.id),
    secrets: hook_secrets,
};

const REPOSITORIES: SecretList<SkillRepositoryConfig> = SecretList {
    path: ["skills", "repositories"],
    key_name: "source id",
    key: |repository| repository.source_uuid.to_string(),
    key_note: "",
    renamed: |stored, written| stored.name != written.name,
    describe: |repository| {
        format!(
            "the skills repository `{}` (`skills.repositories`, source id `{}`)",
            repository.name, repository.source_uuid
        )
    },
    secrets: repository_secrets,
};

/// The markers of one write, as they are resolved.
struct Resolve<'w> {
    /// The merge patch of a patch write: a marker is the write's own only
    /// where the patch itself holds it, and its stored value is written
    /// into the patch too. `None` for a full replace, whose every marker is
    /// its own.
    patch: Option<&'w mut Value>,
}

impl Resolve<'_> {
    /// Whether the write itself carries the marker at `path` of the
    /// document.
    fn carries(&mut self, path: &[String]) -> bool {
        match &mut self.patch {
            None => true,
            Some(patch) => value_at_mut(patch, path)
                .is_some_and(|value| value.as_str() == Some(CONFIG_SECRET_MARKER)),
        }
    }

    /// Put the stored values back for the markers of one entry or source,
    /// whose places `visit` passes: each marker the write carries becomes
    /// the stored value of the same place, or the first one with no stored
    /// value refuses.
    fn fill(
        &mut self,
        prefix: &[String],
        described: &str,
        stored: &[StoredSecret],
        visit: impl FnOnce(VisitSecret<'_>),
    ) -> Result<(), ConfigError> {
        let mut refusal = None;
        visit(&mut |place, value| {
            if refusal.is_some() || value.as_str() != CONFIG_SECRET_MARKER {
                return;
            }
            let mut path = prefix.to_vec();
            path.extend(place.path.iter().map(ToString::to_string));
            if !self.carries(&path) {
                return;
            }
            match stored.iter().find(|stored| stored.is_at(place)) {
                Some(stored) if stored.value != CONFIG_SECRET_MARKER => {
                    value.clone_from(&stored.value);
                    if let Some(patch) = &mut self.patch
                        && let Some(written) = value_at_mut(patch, &path)
                    {
                        *written = Value::String(stored.value.clone());
                    }
                }
                Some(_) | None => {
                    refusal = Some(ConfigError::Validation(format!(
                        "{described} carries the marker `{CONFIG_SECRET_MARKER}` at `{}`. The \
                         marker stands for a value the document already holds at the same place, \
                         and it holds none there. {SEND_THE_VALUES}",
                        place.path.join("."),
                    )));
                }
            }
        });
        refusal.map_or(Ok(()), Err)
    }

    /// Whether the write carries a marker in `entry`, the item `index` of
    /// `list`.
    fn carries_in<E: Clone>(&mut self, list: &SecretList<E>, index: usize, entry: &E) -> bool {
        let mut places = Vec::new();
        (list.secrets)(&mut entry.clone(), &mut collect(&mut places));
        places
            .into_iter()
            .filter(|place| place.value == CONFIG_SECRET_MARKER)
            .any(|place| {
                let mut path = entry_path(list, index);
                path.extend(place.path);
                self.carries(&path)
            })
    }

    /// Resolve the markers of one written list against the stored one.
    fn list<E: Clone>(
        &mut self,
        list: &SecretList<E>,
        written: &mut [E],
        stored: &[E],
    ) -> Result<(), ConfigError> {
        let keys: Vec<String> = written.iter().map(list.key).collect();
        for (index, key) in keys.iter().enumerate() {
            if !self.carries_in(list, index, &written[index]) {
                continue;
            }
            let described = (list.describe)(&written[index]);
            let key_name = list.key_name;
            let written_of_key = keys.iter().filter(|other| *other == key).count();
            let stored_of_key: Vec<&E> = stored
                .iter()
                .filter(|entry| (list.key)(entry) == *key)
                .collect();
            let stored_entry = match stored_of_key.as_slice() {
                [stored_entry] if written_of_key == 1 => *stored_entry,
                [] => {
                    return Err(ConfigError::Validation(format!(
                        "{described} carries the marker `{CONFIG_SECRET_MARKER}`. The marker \
                         stands for a value the document already holds in the entry of the same \
                         {key_name}, and it holds no entry of this {key_name}. {SEND_THE_VALUES}"
                    )));
                }
                several => {
                    return Err(ConfigError::Validation(format!(
                        "{described} carries the marker `{CONFIG_SECRET_MARKER}`. The marker \
                         stands for a value of the one stored entry of the same {key_name}, and \
                         {} stored and {written_of_key} written entries carry this {key_name}{}. \
                         {SEND_THE_VALUES}",
                        several.len(),
                        list.key_note,
                    )));
                }
            };
            if (list.renamed)(stored_entry, &written[index]) {
                return Err(ConfigError::Validation(format!(
                    "{described} carries the marker `{CONFIG_SECRET_MARKER}`, and the document \
                     holds the entry of this {key_name} under another name: {}. The marker \
                     stands for a value of the same entry. {SEND_THE_VALUES}",
                    (list.describe)(stored_entry),
                )));
            }
            let mut held = Vec::new();
            let stored_for = (list.secrets)(&mut stored_entry.clone(), &mut collect(&mut held));
            let written_for = (list.secrets)(&mut written[index].clone(), &mut |_, _| {});
            match moved(stored_for.as_ref(), written_for.as_ref()) {
                None => {}
                Some(Moved::Kind) => {
                    return Err(ConfigError::Validation(format!(
                        "{described} carries the marker `{CONFIG_SECRET_MARKER}`, and the \
                         document holds another kind of entry under this {key_name}. The marker \
                         stands for a value of the same kind of entry. {SEND_THE_VALUES}"
                    )));
                }
                Some(Moved::Url) => {
                    return Err(ConfigError::Validation(format!(
                        "{described} holds stored secret values and this write changes its `url` \
                         while it carries the marker `{CONFIG_SECRET_MARKER}` for them. A stored \
                         secret value stays with the address it was stored for: send the secret \
                         values again in the same write, with the new address. Nothing was \
                         written."
                    )));
                }
            }
            let prefix = entry_path(list, index);
            let entry = &mut written[index];
            self.fill(&prefix, &described, &held, |visit| {
                (list.secrets)(entry, visit);
            })?;
        }
        Ok(())
    }

    /// A stored inline secret stays with the route it was stored for: a
    /// write that changes where an auth profile's stored inline secret is
    /// sent ([`ProfileRoutes`]) is refused unless the write itself states
    /// the secret as a value. A full replace states it when it carries a
    /// value there and not the marker; a patch states it when the patch
    /// itself holds the secret. A profile whose credential is not in the
    /// document (a variable, the credential store) holds no stored secret
    /// and its route is not looked at; neither is a removed binding.
    ///
    /// And a patch that makes a profile an inline secret (the stored
    /// profile of that name is none, or there is none) states the secret
    /// itself: a `secret` key that an earlier form of the profile left in
    /// the document is not taken up, wherever the profile sends it now.
    fn inline_secrets_stay_with_their_route(
        &mut self,
        written: &Config,
        stored: &Config,
    ) -> Result<(), ConfigError> {
        for (realm, section) in &written.realm {
            let stored_section = stored.realm.get(realm);
            for (name, profile) in &section.auth {
                let CredentialSourceSpec::InlineSecret { secret } = &profile.source else {
                    continue;
                };
                // Whether the write itself states the secret as a value;
                // `None` for a patch that does not name the secret.
                let stated = match &mut self.patch {
                    None => Some(secret.as_str() != CONFIG_SECRET_MARKER),
                    Some(patch) => {
                        let path = [
                            "realm",
                            realm.as_str(),
                            "auth",
                            name.as_str(),
                            "source",
                            "secret",
                        ]
                        .map(ToString::to_string);
                        value_at_mut(patch, &path)
                            .and_then(|value| value.as_str())
                            .map(|value| value != CONFIG_SECRET_MARKER)
                    }
                };
                if stated == Some(true) {
                    continue;
                }
                let stored_inline = stored_section.and_then(|stored_section| {
                    let stored_profile = stored_section.auth.get(name)?;
                    matches!(
                        stored_profile.source,
                        CredentialSourceSpec::InlineSecret { .. }
                    )
                    .then_some((stored_section, stored_profile))
                });
                let refusal = match stored_inline {
                    Some((stored_section, stored_profile)) => {
                        let Some(changed) =
                            ProfileRoutes::of(stored, stored_section, name, stored_profile)
                                .changed_by(&ProfileRoutes::of(written, section, name, profile))
                        else {
                            continue;
                        };
                        format!(
                            "the auth profile `{name}` of the realm `{realm}` holds a stored \
                             secret and this write changes {changed} without stating the secret. \
                             A stored secret value stays with the address it was stored for: \
                             send the secret again in the same write, with the new address. \
                             Nothing was written."
                        )
                    }
                    // No inline secret is stored under this name, and the
                    // patch names none: the secret would be a key left in
                    // the document.
                    None if stated.is_none() => format!(
                        "the auth profile `{name}` of the realm `{realm}` becomes an inline \
                         secret and this write does not state the secret. A `secret` key that \
                         an earlier form of the profile left in the document is not taken up: \
                         send the secret in the same write. Nothing was written."
                    ),
                    // A marker where nothing is stored: refused where it is
                    // resolved.
                    None => continue,
                };
                return Err(ConfigError::Validation(refusal));
            }
        }
        Ok(())
    }

    /// Resolve every marker the write carries in `written` against
    /// `stored`.
    fn document(&mut self, written: &mut Config, stored: &Config) -> Result<(), ConfigError> {
        self.inline_secrets_stay_with_their_route(written, stored)?;
        let sections = SecretSections::of(written);
        for (realm, section) in sections.realms.iter_mut() {
            let mut stored_section = stored.realm.get(realm).cloned();
            let stored_sources = stored_section
                .as_mut()
                .map(realm_sources)
                .unwrap_or_default();
            for (below, described, source) in realm_sources(section) {
                let mut held = Vec::new();
                if let Some((_, _, stored_source)) = stored_sources
                    .iter()
                    .find(|(stored_below, _, _)| *stored_below == below)
                {
                    source_secrets(
                        &mut CredentialSourceSpec::clone(stored_source),
                        &mut collect(&mut held),
                    );
                }
                let mut prefix = vec!["realm".to_string(), realm.clone()];
                prefix.extend(below);
                let described = format!("{described} of the realm `{realm}`");
                self.fill(&prefix, &described, &held, |visit| {
                    source_secrets(source, visit);
                })?;
            }
        }
        self.list(&SERVERS, sections.servers, &stored.tools.mcp_servers)?;
        self.list(&HOOKS, sections.hooks, &stored.hooks.entries)?;
        self.list(
            &REPOSITORIES,
            sections.repositories,
            &stored.skills.repositories,
        )
    }
}

/// Where a binding sends its auth profile's credential: its backend
/// profile's id, that backend as the realm section states it, the
/// `[self_hosted.servers]` entry the backend names as the document states
/// it, and the binding's default model, which decides the model, and so
/// the server, of a request that names none. (The config store compares
/// the same for a credential command.) A self-hosted backend that names no
/// server sends the credential to the server that serves a request's
/// model, so for it the route also holds the document's whole
/// `[self_hosted]` section, which decides that.
#[derive(PartialEq)]
struct SecretRoute {
    backend_profile: String,
    backend: Option<BackendProfileConfig>,
    server: Option<SelfHostedServerConfig>,
    self_hosted: Option<SelfHostedConfig>,
    default_model: Option<String>,
}

/// Where an auth profile's credential is sent: for its provider and auth
/// method, by each binding of its realm section that uses it.
#[derive(PartialEq)]
struct ProfileRoutes {
    provider: String,
    auth_method: String,
    bindings: BTreeMap<String, SecretRoute>,
}

impl ProfileRoutes {
    fn of(
        config: &Config,
        section: &RealmConfigSection,
        name: &str,
        profile: &AuthProfileConfig,
    ) -> Self {
        let bindings = section
            .binding
            .iter()
            .filter(|(_, binding)| binding.auth_profile == name)
            .map(|(id, binding)| {
                let backend = section.backend.get(&binding.backend_profile).cloned();
                let server = backend
                    .as_ref()
                    .and_then(|backend| backend.server.as_ref())
                    .and_then(|server| config.self_hosted.servers.get(server))
                    .cloned();
                let self_hosted = backend
                    .as_ref()
                    .filter(|backend| {
                        backend.server.is_none()
                            && crate::provider::Provider::parse_strict(&backend.provider)
                                == Some(crate::provider::Provider::SelfHosted)
                    })
                    .map(|_| config.self_hosted.clone());
                (
                    id.clone(),
                    SecretRoute {
                        backend_profile: binding.backend_profile.clone(),
                        backend,
                        server,
                        self_hosted,
                        default_model: binding.default_model.clone(),
                    },
                )
            })
            .collect();
        Self {
            provider: profile.provider.clone(),
            auth_method: profile.auth_method.clone(),
            bindings,
        }
    }

    /// What `written` changes of where the credential is sent, as a refusal
    /// names it; `None` when every binding that uses the profile is one
    /// that already did and sends the credential where it did. A removed
    /// binding changes nothing.
    fn changed_by(&self, written: &Self) -> Option<String> {
        if self.provider != written.provider || self.auth_method != written.auth_method {
            return Some("its provider or auth method".to_string());
        }
        written.bindings.iter().find_map(|(binding, route)| {
            let stored = self.bindings.get(binding);
            if stored == Some(route) {
                return None;
            }
            // Only the section that decides the server of a backend that
            // names none differs.
            let section_alone = stored.is_some_and(|stored| {
                stored.backend_profile == route.backend_profile
                    && stored.backend == route.backend
                    && stored.server == route.server
                    && stored.default_model == route.default_model
            });
            Some(if section_alone {
                format!("`[self_hosted]`, which decides the server of the binding `{binding}`,")
            } else {
                format!("where the binding `{binding}` sends it")
            })
        })
    }
}

/// The path of the item `index` of `list` in the document.
fn entry_path<E>(list: &SecretList<E>, index: usize) -> Vec<String> {
    vec![
        list.path[0].to_string(),
        list.path[1].to_string(),
        index.to_string(),
    ]
}

/// The refusal of a write that would store the marker text at `path`.
fn marker_left(path: &str) -> ConfigError {
    ConfigError::Validation(format!(
        "this write would store the marker `{CONFIG_SECRET_MARKER}` as a value at `{path}`. The \
         marker stands for a secret value the document already holds at the same place, and it \
         can stand for none there. Send the value itself. Nothing was written."
    ))
}

/// Resolve the markers of a full replace: `written`, the document the write
/// states, against `stored`, the document it replaces. Each marker becomes
/// the stored value of the same place (the module documentation has the
/// rules), in `written` itself; the first one that stands for nothing
/// refuses the write, and so does the marker text anywhere else in the
/// document. On a refusal `written` is not to be used.
pub fn resolve_written_config(written: &mut Config, stored: &Config) -> Result<(), ConfigError> {
    Resolve { patch: None }.document(written, stored)?;
    match marker_path(&serde_json::to_value(&*written)?, &mut Vec::new()) {
        Some(path) => Err(marker_left(&path)),
        None => Ok(()),
    }
}

/// Resolve the markers of a merge patch: `patch` as the write sends it,
/// `preview` the document it would commit over `stored`. A marker is the
/// write's own only where `patch` itself holds it; each becomes the stored
/// value of the same place, in `patch` and in `preview` alike, under the
/// rules of [`resolve_written_config`]. The marker text anywhere else in
/// `patch` refuses the write. On a refusal neither is to be used.
pub fn resolve_written_patch(
    patch: &mut Value,
    preview: &mut Config,
    stored: &Config,
) -> Result<(), ConfigError> {
    Resolve {
        patch: Some(&mut *patch),
    }
    .document(preview, stored)?;
    match marker_path(patch, &mut Vec::new()) {
        Some(path) => Err(marker_left(&path)),
        None => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    const WORDS: &str = "synthetic stored value";
    const PIPED: &str = "11111111-1111-4111-8111-111111111111";
    const BEARER: &str = "22222222-2222-4222-8222-222222222222";
    const HEADER: &str = "33333333-3333-4333-8333-333333333333";
    const CLONED: &str = "44444444-4444-4444-8444-444444444444";
    const OTHER_URL: &str = "http://other.synthetic.invalid/";

    /// The stored value of the fixture's place `n`: plain synthetic words,
    /// put together at run time.
    fn value(n: u8) -> String {
        format!("{WORDS} {n:02}")
    }

    /// The document of `json`'s realm sections and three lists.
    fn config_of(document: &Value) -> Config {
        let mut config = Config {
            realm: serde_json::from_value(document["realm"].clone()).unwrap(),
            ..Config::default()
        };
        config.tools.mcp_servers =
            serde_json::from_value(document["tools"]["mcp_servers"].clone()).unwrap();
        config.hooks.entries =
            serde_json::from_value(document["hooks"]["entries"].clone()).unwrap();
        config.skills.repositories =
            serde_json::from_value(document["skills"]["repositories"].clone()).unwrap();
        config
    }

    fn json_of(config: &Config) -> Value {
        serde_json::to_value(config).unwrap()
    }

    /// A document that holds a value at every secret place.
    fn stored_document() -> Config {
        config_of(&json!({
            "realm": {"team": {
                "auth": {
                    "inline": {"provider": "openai", "auth_method": "api_key",
                        "source": {"kind": "inline_secret", "secret": value(1)}},
                    "minted": {"provider": "openai", "auth_method": "api_key",
                        "source": {"kind": "command", "program": "/synthetic/credential",
                            "args": ["--flag"], "env": {"SEED": value(2)}}},
                },
                "copilot": {"source": {"kind": "inline_secret", "secret": value(11)}},
                "backend": {"main": {"provider": "openai", "backend_kind": "openai_api",
                    "base_url": "http://synthetic.invalid/v1"}},
                "binding": {"main": {"backend_profile": "main", "auth_profile": "inline"}},
            }},
            "tools": {"mcp_servers": [
                {"name": "local", "command": "/synthetic/server", "args": ["serve"],
                    "env": {"SYNTHETIC": value(3)}},
                {"name": "remote", "url": "http://synthetic.invalid/mcp",
                    "headers": {"X-Synthetic": value(4)}},
            ]},
            "hooks": {"entries": [
                {"id": "run", "runtime": {"type": "command", "command": "/synthetic/hook",
                    "env": {"SYNTHETIC": value(5)}}},
                {"id": "notify", "runtime": {"type": "http",
                    "url": "http://synthetic.invalid/hook",
                    "headers": {"X-Synthetic": value(6)}}},
            ]},
            "skills": {"repositories": [
                {"name": "piped", "source_uuid": PIPED, "type": "stdio",
                    "command": "/synthetic/skills", "env": {"SYNTHETIC": value(7)}},
                {"name": "bearer", "source_uuid": BEARER, "type": "http",
                    "url": "http://synthetic.invalid/skills",
                    "auth": {"scheme": "bearer", "token": value(8)}},
                {"name": "header", "source_uuid": HEADER, "type": "http",
                    "url": "http://two.synthetic.invalid/skills",
                    "auth": {"scheme": "header", "name": "X-Synthetic", "value": value(9)}},
                {"name": "cloned", "source_uuid": CLONED, "type": "git",
                    "url": "https://synthetic.invalid/skills.git",
                    "auth": {"scheme": "token", "token": value(10)}},
            ]},
        }))
    }

    /// Where each value of the fixture sits in the document's JSON form.
    const PLACES: [(&str, u8); 11] = [
        ("/realm/team/auth/inline/source/secret", 1),
        ("/realm/team/auth/minted/source/env/SEED", 2),
        ("/realm/team/copilot/source/secret", 11),
        ("/tools/mcp_servers/0/env/SYNTHETIC", 3),
        ("/tools/mcp_servers/1/headers/X-Synthetic", 4),
        ("/hooks/entries/0/runtime/env/SYNTHETIC", 5),
        ("/hooks/entries/1/runtime/headers/X-Synthetic", 6),
        ("/skills/repositories/0/env/SYNTHETIC", 7),
        ("/skills/repositories/1/auth/token", 8),
        ("/skills/repositories/2/auth/value", 9),
        ("/skills/repositories/3/auth/token", 10),
    ];

    /// The stored document as an answer shows it, in JSON form.
    fn shown_document() -> Value {
        let mut shown = stored_document();
        mask_config_secrets(&mut shown);
        json_of(&shown)
    }

    /// `document` with `value` at `pointer`, whose parent must exist.
    fn with(mut document: Value, pointer: &str, value: Value) -> Value {
        let (parent, name) = pointer.rsplit_once('/').unwrap();
        match document.pointer_mut(parent).unwrap() {
            Value::Object(members) => {
                members.insert(name.to_string(), value);
            }
            Value::Array(items) => match name.parse::<usize>() {
                Ok(index) if index < items.len() => items[index] = value,
                _ => items.push(value),
            },
            other => unreachable!("no member under {other}"),
        }
        document
    }

    /// The refusal of a full replace of the stored document by `written`:
    /// its text, which must be a validation refusal that holds each of
    /// `says` and no stored value.
    fn refused(stored: &Config, written: &Value, says: &[&str]) -> String {
        let refusal = resolve_written_config(&mut config_of(written), stored)
            .expect_err("the write is refused");
        let text = refusal.to_string();
        assert!(matches!(refusal, ConfigError::Validation(_)), "{text}");
        for says in says {
            assert!(text.contains(says), "`{says}` is not in: {text}");
        }
        assert!(text.contains("Nothing was written"), "{text}");
        assert!(!text.contains(WORDS), "{text}");
        text
    }

    #[test]
    fn the_mask_puts_the_marker_at_every_secret_place_and_changes_nothing_else() {
        let stored = json_of(&stored_document());
        let shown = shown_document();
        let mut expected = stored.clone();
        for (pointer, n) in PLACES {
            assert_eq!(stored.pointer(pointer), Some(&json!(value(n))), "{pointer}");
            assert_eq!(
                shown.pointer(pointer),
                Some(&json!(CONFIG_SECRET_MARKER)),
                "{pointer}"
            );
            *expected.pointer_mut(pointer).unwrap() = json!(CONFIG_SECRET_MARKER);
        }
        assert_eq!(shown, expected, "nothing but the secret values changes");
        assert!(!shown.to_string().contains(WORDS));
        assert_eq!(CONFIG_SECRET_MARKER, "<redacted>");
    }

    #[test]
    fn a_full_replace_by_the_shown_document_resolves_to_the_stored_one() {
        let stored = stored_document();
        let mut written = config_of(&shown_document());
        resolve_written_config(&mut written, &stored).expect("every marker has its stored value");
        assert_eq!(json_of(&written), json_of(&stored));
    }

    #[test]
    fn a_patch_of_the_shown_sections_resolves_in_the_patch_and_in_the_preview() {
        let stored = stored_document();
        let shown = shown_document();
        let mut patch = json!({
            "max_tokens": 2048,
            "realm": shown["realm"],
            "tools": {"mcp_servers": shown["tools"]["mcp_servers"]},
            "hooks": {"entries": shown["hooks"]["entries"]},
            "skills": {"repositories": shown["skills"]["repositories"]},
        });
        assert!(holds_secret_marker(&patch));
        let mut preview = config_of(&shown);
        resolve_written_patch(&mut patch, &mut preview, &stored)
            .expect("every marker has its stored value");
        assert_eq!(json_of(&preview), json_of(&stored));
        assert!(!holds_secret_marker(&patch));
        for (pointer, n) in PLACES {
            assert_eq!(patch.pointer(pointer), Some(&json!(value(n))), "{pointer}");
        }
    }

    /// A marker the document already holds where the patch names nothing
    /// is not the write's: it is neither resolved nor refused.
    #[test]
    fn a_marker_the_patch_does_not_carry_is_left_as_stored() {
        let literal = "/tools/mcp_servers/1/headers/X-Synthetic";
        let stored = config_of(&with(
            json_of(&stored_document()),
            literal,
            json!(CONFIG_SECRET_MARKER),
        ));
        let shown = shown_document();
        let mut patch = json!({"hooks": {"entries": shown["hooks"]["entries"]}});
        let mut preview = config_of(&with(
            json_of(&stored),
            "/hooks/entries",
            shown["hooks"]["entries"].clone(),
        ));
        resolve_written_patch(&mut patch, &mut preview, &stored)
            .expect("the patch's own markers have their stored values");
        assert_eq!(json_of(&preview), json_of(&stored));
        // A full replace states every value: the same document is refused.
        refused(
            &stored,
            &shown,
            &["the MCP server `remote`", "holds none there"],
        );
    }

    #[test]
    fn lists_written_in_another_order_resolve_by_the_entrys_own_key() {
        let stored = stored_document();
        let reversed = |document: &Value| {
            let mut document = document.clone();
            for list in [
                "/tools/mcp_servers",
                "/hooks/entries",
                "/skills/repositories",
            ] {
                document
                    .pointer_mut(list)
                    .and_then(Value::as_array_mut)
                    .unwrap()
                    .reverse();
            }
            document
        };
        let mut written = config_of(&reversed(&shown_document()));
        resolve_written_config(&mut written, &stored).expect("each marker by its entry's key");
        assert_eq!(json_of(&written), reversed(&json_of(&stored)));
    }

    #[test]
    fn a_marker_with_no_stored_value_of_its_own_place_is_refused() {
        let stored = stored_document();
        let shown = shown_document();
        let marker = || json!(CONFIG_SECRET_MARKER);
        let remote = |name: &str, url: &str| json!({"name": name, "url": url, "headers": {"X-Synthetic": CONFIG_SECRET_MARKER}});
        for (what, written, says) in [
            (
                "an added MCP server",
                with(
                    shown.clone(),
                    "/tools/mcp_servers/9",
                    remote("added", "http://synthetic.invalid/mcp"),
                ),
                vec!["the MCP server `added`", "holds no entry of this name"],
            ),
            (
                "an MCP server under another name",
                with(
                    shown.clone(),
                    "/tools/mcp_servers/1",
                    remote("renamed", "http://synthetic.invalid/mcp"),
                ),
                vec!["the MCP server `renamed`", "holds no entry of this name"],
            ),
            (
                "a new name in a value map",
                with(
                    shown.clone(),
                    "/tools/mcp_servers/1/headers/X-Added",
                    marker(),
                ),
                vec![
                    "the MCP server `remote`",
                    "`headers.X-Added`",
                    "holds none there",
                ],
            ),
            (
                "an MCP server written twice",
                with(
                    shown.clone(),
                    "/tools/mcp_servers/9",
                    remote("remote", "http://synthetic.invalid/mcp"),
                ),
                vec!["1 stored and 2 written entries carry this name"],
            ),
            (
                "another kind of entry under a server's name",
                with(
                    shown.clone(),
                    "/tools/mcp_servers/1",
                    json!({"name": "remote", "command": "/synthetic/server",
                        "env": {"X-Synthetic": CONFIG_SECRET_MARKER}}),
                ),
                vec!["the MCP server `remote`", "another kind of entry"],
            ),
            (
                "another kind of entry under a hook's id",
                with(
                    shown.clone(),
                    "/hooks/entries/1/runtime",
                    json!({"type": "command", "command": "/synthetic/hook",
                        "env": {"X-Synthetic": CONFIG_SECRET_MARKER}}),
                ),
                vec!["the hook `notify`", "another kind of entry"],
            ),
            (
                "a git repository under an HTTP repository's source id",
                with(
                    shown.clone(),
                    "/skills/repositories/1",
                    json!({"name": "bearer", "source_uuid": BEARER, "type": "git",
                        "url": "http://synthetic.invalid/skills",
                        "auth": {"scheme": "token", "token": CONFIG_SECRET_MARKER}}),
                ),
                vec!["the skills repository `bearer`", "another kind of entry"],
            ),
            (
                "another kind of credential",
                with(
                    shown.clone(),
                    "/skills/repositories/1/auth",
                    json!({"scheme": "header", "name": "X-Synthetic",
                        "value": CONFIG_SECRET_MARKER}),
                ),
                vec![
                    "the skills repository `bearer`",
                    "`auth.value`",
                    "holds none there",
                ],
            ),
            (
                "a header credential under another header name",
                with(
                    shown.clone(),
                    "/skills/repositories/2/auth/name",
                    json!("X-Other"),
                ),
                vec!["the skills repository `header`", "holds none there"],
            ),
            (
                "a repository under another name",
                with(
                    shown.clone(),
                    "/skills/repositories/1/name",
                    json!("renamed"),
                ),
                vec![
                    "the skills repository `renamed`",
                    "under another name",
                    "`bearer`",
                ],
            ),
            (
                "an added auth profile",
                with(
                    shown.clone(),
                    "/realm/team/auth/added",
                    json!({"provider": "openai", "auth_method": "api_key",
                        "source": {"kind": "inline_secret", "secret": CONFIG_SECRET_MARKER}}),
                ),
                vec![
                    "the auth profile `added` of the realm `team`",
                    "holds none there",
                ],
            ),
            (
                "another kind of source",
                with(
                    shown.clone(),
                    "/realm/team/auth/minted/source",
                    json!({"kind": "inline_secret", "secret": CONFIG_SECRET_MARKER}),
                ),
                vec![
                    "the auth profile `minted` of the realm `team`",
                    "holds none there",
                ],
            ),
            (
                "a new name in a credential command's environment",
                with(
                    shown.clone(),
                    "/realm/team/auth/minted/source/env/ADDED",
                    marker(),
                ),
                vec![
                    "the auth profile `minted`",
                    "`env.ADDED`",
                    "holds none there",
                ],
            ),
            (
                "the marker text as an argument",
                with(shown, "/tools/mcp_servers/0/args/0", marker()),
                vec!["would store the marker", "`tools.mcp_servers.0.args.0`"],
            ),
        ] {
            let text = refused(&stored, &written, &says);
            assert!(text.contains(CONFIG_SECRET_MARKER), "{what}: {text}");
        }
    }

    #[test]
    fn a_marker_under_a_key_that_stored_entries_share_is_refused() {
        let shown = shown_document();
        let twice = |document: Value, list: &str| {
            let entry = document.pointer(&format!("{list}/1")).unwrap().clone();
            with(document, &format!("{list}/9"), entry)
        };
        let stored = config_of(&twice(json_of(&stored_document()), "/tools/mcp_servers"));
        refused(
            &stored,
            &shown,
            &[
                "the MCP server `remote`",
                "2 stored and 1 written entries carry this name",
            ],
        );
        // Two hooks written without an id both take the id `hook`.
        let without_id = |document: Value| {
            let mut document = twice(document, "/hooks/entries");
            for hook in document["hooks"]["entries"].as_array_mut().unwrap() {
                hook.as_object_mut().unwrap().remove("id");
            }
            document
        };
        let stored = config_of(&without_id(json_of(&stored_document())));
        refused(
            &stored,
            &without_id(shown),
            &[
                "the hook `hook`",
                "3 stored and 3 written entries carry this id",
                "an entry written without an id takes the id `hook`",
            ],
        );
    }

    /// A stored secret value stays with the address it was stored for: a
    /// marker is not resolved in an entry whose URL the write changes.
    #[test]
    fn a_marker_in_an_entry_whose_url_changes_is_refused() {
        let stored = stored_document();
        let shown = shown_document();
        for (pointer, named) in [
            ("/tools/mcp_servers/1/url", "the MCP server `remote`"),
            ("/hooks/entries/1/runtime/url", "the hook `notify`"),
            (
                "/skills/repositories/1/url",
                "the skills repository `bearer`",
            ),
            (
                "/skills/repositories/2/url",
                "the skills repository `header`",
            ),
            (
                "/skills/repositories/3/url",
                "the skills repository `cloned`",
            ),
        ] {
            let moved = with(shown.clone(), pointer, json!(OTHER_URL));
            refused(
                &stored,
                &moved,
                &[
                    named,
                    "changes its `url`",
                    "send the secret values again in the same write, with the new address",
                ],
            );
            // The same write with the values stated passes.
            let stated = with(json_of(&stored), pointer, json!(OTHER_URL));
            let mut written = config_of(&stated);
            resolve_written_config(&mut written, &stored).expect("the write states the values");
            assert_eq!(json_of(&written), stated);
        }
    }

    /// What a process entry runs is not part of where its values go: a
    /// write may change a command, an argument list or a working directory
    /// and keep the environment values by their marker.
    #[test]
    fn a_marker_in_a_process_entry_resolves_whatever_the_entry_runs() {
        let stored = stored_document();
        let mut changed = shown_document();
        for (pointer, to) in [
            ("/tools/mcp_servers/0/args", json!(["serve", "--more"])),
            ("/tools/mcp_servers/0/cwd", json!("/synthetic")),
            (
                "/hooks/entries/0/runtime/command",
                json!("/synthetic/other"),
            ),
            ("/skills/repositories/0/command", json!("/synthetic/other")),
            ("/realm/team/auth/minted/source/args", json!(["--other"])),
        ] {
            changed = with(changed, pointer, to);
        }
        let mut written = config_of(&changed);
        resolve_written_config(&mut written, &stored).expect("the markers resolve");
        let written = json_of(&written);
        for (pointer, n) in PLACES {
            assert_eq!(
                written.pointer(pointer),
                Some(&json!(value(n))),
                "{pointer}"
            );
        }
        assert_eq!(
            written.pointer("/tools/mcp_servers/0/args"),
            Some(&json!(["serve", "--more"]))
        );
    }

    #[test]
    fn a_patch_that_carries_the_marker_where_nothing_is_stored_is_refused() {
        let stored = stored_document();
        let shown = shown_document();
        // The marker text as another setting's value.
        let mut patch = json!({"agent": {"system_prompt": CONFIG_SECRET_MARKER}});
        let text = resolve_written_patch(&mut patch, &mut stored.clone(), &stored)
            .expect_err("the marker text is never stored")
            .to_string();
        assert!(text.contains("`agent.system_prompt`"), "{text}");
        // An added entry.
        let added = with(
            shown,
            "/tools/mcp_servers/9",
            json!({"name": "added", "url": OTHER_URL,
                "headers": {"X-Synthetic": CONFIG_SECRET_MARKER}}),
        );
        let mut patch = json!({"tools": {"mcp_servers": added["tools"]["mcp_servers"]}});
        let text = resolve_written_patch(&mut patch, &mut config_of(&added), &stored)
            .expect_err("nothing is stored under the added name")
            .to_string();
        assert!(text.contains("the MCP server `added`"), "{text}");
        assert!(!text.contains(WORDS), "{text}");
    }

    #[test]
    fn only_a_whole_string_value_is_the_marker() {
        assert!(holds_secret_marker(
            &json!({"a": [1, {"b": CONFIG_SECRET_MARKER}]})
        ));
        assert!(!holds_secret_marker(&json!({
            CONFIG_SECRET_MARKER: "a name is not a value",
            "a": format!("{CONFIG_SECRET_MARKER} and more"),
        })));
    }

    /// A stored inline secret stays with the route it was stored for: a
    /// write that changes where the secret is sent must state it as a
    /// value. The marker does not state it, and neither does a patch that
    /// leaves it out.
    #[test]
    fn a_stored_inline_secret_stays_with_its_route() {
        const BASE_URL: &str = "/realm/team/backend/main/base_url";
        const SECRET: &str = "/realm/team/auth/inline/source/secret";
        const PROFILE: &str = "the auth profile `inline` of the realm `team`";
        const WHAT_TO_DO: &str = "send the secret again in the same write, with the new address";
        let stored = stored_document();
        let shown = shown_document();

        // A full replace: the marker does not state the secret, a value does.
        let moved = with(shown.clone(), BASE_URL, json!(OTHER_URL));
        refused(
            &stored,
            &moved,
            &[PROFILE, WHAT_TO_DO, "the binding `main`"],
        );
        let stated = with(moved, SECRET, json!(value(1)));
        resolve_written_config(&mut config_of(&stated), &stored)
            .expect("the write states the secret for the new address");
        for (pointer, to, says) in [
            (
                "/realm/team/binding/main/default_model",
                json!("another-model"),
                "the binding `main`",
            ),
            (
                "/realm/team/binding/second",
                json!({"backend_profile": "main", "auth_profile": "inline"}),
                "the binding `second`",
            ),
            (
                "/realm/team/auth/inline/provider",
                json!("anthropic"),
                "its provider or auth method",
            ),
        ] {
            refused(&stored, &with(shown.clone(), pointer, to), &[PROFILE, says]);
        }
        let mut unbound = shown;
        unbound["realm"]["team"]["binding"]
            .as_object_mut()
            .unwrap()
            .remove("main");
        resolve_written_config(&mut config_of(&unbound), &stored)
            .expect("a removed binding sends the secret nowhere else");

        // A patch: only a patch that itself holds the secret states it.
        let address = json!({"realm": {"team": {"backend": {"main": {"base_url": OTHER_URL}}}}});
        let with_secret = |secret: &str| {
            let mut patch = address.clone();
            patch["realm"]["team"]["auth"] = json!({"inline": {"source": {"secret": secret}}});
            patch
        };
        for (mut patch, commits) in [
            (address.clone(), false),
            (with_secret(CONFIG_SECRET_MARKER), false),
            (with_secret("a secret the writer states"), true),
        ] {
            assert!(patch_reads_stored(&patch));
            let mut document = json_of(&stored);
            crate::merge_patch(&mut document, patch.clone());
            let outcome = resolve_written_patch(&mut patch, &mut config_of(&document), &stored);
            match outcome {
                Ok(()) => assert!(commits, "the patch does not state the secret"),
                Err(refusal) => {
                    let text = refusal.to_string();
                    assert!(!commits, "{text}");
                    assert!(
                        text.contains(PROFILE) && text.contains(WHAT_TO_DO),
                        "{text}"
                    );
                    assert!(!text.contains(WORDS), "{text}");
                }
            }
        }

        // A profile whose credential is not in the document holds no stored
        // secret: its route is not this rule's.
        let held_elsewhere = config_of(&with(
            json_of(&stored),
            "/realm/team/auth/inline/source",
            json!({"kind": "env", "env": "SYNTHETIC_NAME"}),
        ));
        let mut moved = json_of(&held_elsewhere);
        *moved.pointer_mut(BASE_URL).unwrap() = json!(OTHER_URL);
        resolve_written_config(&mut config_of(&moved), &held_elsewhere)
            .expect("no stored secret moves");

        // Which writes are judged against the stored document at all.
        assert!(replace_reads_stored(&stored).unwrap());
        assert!(!replace_reads_stored(&Config::default()).unwrap());
        assert!(patch_reads_stored(&json!({"self_hosted": {"servers": {}}})));
        assert!(!patch_reads_stored(&json!({"max_tokens": 2048})));
    }

    /// A self-hosted backend that names no server sends its binding's
    /// credential to the server that serves a request's model, which the
    /// document's `[self_hosted]` section decides. That section is then
    /// part of the route of a stored inline secret: a write that changes
    /// it must state the secret as a value, in a full replace and in a
    /// patch. A backend that names its server is judged by that entry
    /// alone.
    #[test]
    fn a_stored_inline_secret_on_a_backend_that_names_no_server_stays_with_self_hosted() {
        const PROFILE: &str = "the auth profile `inline` of the realm `team`";
        const SECTION: &str =
            "changes `[self_hosted]`, which decides the server of the binding `main`";
        const SECRET: &str = "/realm/team/auth/inline/source/secret";
        const SPARE_URL: &str = "/self_hosted/servers/spare/base_url";
        // `config_of` and the `[self_hosted]` section of the document.
        let config = |document: &Value| {
            let mut config = config_of(document);
            config.self_hosted = serde_json::from_value(document["self_hosted"].clone()).unwrap();
            config
        };
        // The stored document with its binding on a self-hosted backend,
        // which names `server` or none.
        let stored_with = |server: Option<&str>| {
            let mut backend = json!({"provider": "self_hosted", "backend_kind": "self_hosted"});
            if let Some(server) = server {
                backend["server"] = json!(server);
            }
            let document = with(
                json_of(&stored_document()),
                "/realm/team/backend/main",
                backend,
            );
            config(&with(
                document,
                "/self_hosted",
                json!({
                    "servers": {
                        "box": {"base_url": "http://synthetic.invalid/v1"},
                        "spare": {"base_url": "http://spare.synthetic.invalid/v1"},
                    },
                    "models": {"served": {"server": "box", "remote_model": "served"}},
                }),
            ))
        };
        let shown_of = |stored: &Config| {
            let mut shown = stored.clone();
            mask_config_secrets(&mut shown);
            json_of(&shown)
        };
        let refusal = |outcome: Result<(), ConfigError>, says: &str, what: &str| {
            let refusal = outcome.expect_err(what);
            let text = refusal.to_string();
            assert!(matches!(refusal, ConfigError::Validation(_)), "{text}");
            assert!(text.contains(PROFILE) && text.contains(says), "{text}");
            assert!(text.contains("Nothing was written"), "{text}");
            assert!(!text.contains(WORDS), "{text}");
        };

        let stored = stored_with(None);
        let shown = shown_of(&stored);
        for (pointer, to) in [
            // The address of the server that serves the binding's models.
            ("/self_hosted/servers/box/base_url", json!(OTHER_URL)),
            // A model, served by another server.
            ("/self_hosted/models/served/server", json!("spare")),
            // The model of a request that names none.
            ("/self_hosted/default_model", json!("served")),
        ] {
            // A full replace: the marker does not state the secret.
            let moved = with(shown.clone(), pointer, to.clone());
            refusal(
                resolve_written_config(&mut config(&moved), &stored),
                SECTION,
                "a full replace that changes `[self_hosted]` is refused",
            );
            let stated = with(moved, SECRET, json!(value(1)));
            resolve_written_config(&mut config(&stated), &stored)
                .expect("the full replace states the secret");

            // A patch: only a patch that itself holds the secret states it.
            let mut patch = json!({});
            let mut at = &mut patch;
            for name in pointer.split('/').skip(1) {
                at = &mut at[name];
            }
            *at = to;
            for (secret, commits) in [(None, false), (Some("a secret the writer states"), true)] {
                let mut patch = patch.clone();
                if let Some(secret) = secret {
                    patch["realm"] =
                        json!({"team": {"auth": {"inline": {"source": {"secret": secret}}}}});
                }
                assert!(patch_reads_stored(&patch));
                let mut document = json_of(&stored);
                crate::merge_patch(&mut document, patch.clone());
                let outcome = resolve_written_patch(&mut patch, &mut config(&document), &stored);
                if commits {
                    outcome.expect("the patch states the secret");
                } else {
                    refusal(
                        outcome,
                        SECTION,
                        "a patch that changes `[self_hosted]` is refused",
                    );
                }
            }
        }

        // A backend that names its server is judged by that entry alone:
        // the rest of `[self_hosted]` is not its route.
        let stored = stored_with(Some("box"));
        let shown = shown_of(&stored);
        let elsewhere = with(shown.clone(), SPARE_URL, json!(OTHER_URL));
        resolve_written_config(&mut config(&elsewhere), &stored)
            .expect("another server's entry is not the route of a backend that names its own");
        refusal(
            resolve_written_config(
                &mut config(&with(
                    shown,
                    "/self_hosted/servers/box/base_url",
                    json!(OTHER_URL),
                )),
                &stored,
            ),
            "where the binding `main` sends it",
            "a change of the entry the backend names is refused",
        );
    }
}
