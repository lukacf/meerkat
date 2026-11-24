//! A host's selection of OS process confinement for the processes its agents
//! launch: the `[confinement]` table of a realm or user config document, and
//! the operator's per-process flag selection.
//!
//! A selection states requirements only. The facade lowers it into the
//! mechanical requirement of [`crate::confinement`] and installs that on the
//! host's agent factory; it authorizes no caller and grants no permission.
//!
//! The table is administered by the host in the document itself: an ordinary
//! config write can neither change, remove nor add it
//! ([`crate::config::ConfigError::ConfinementAdministration`]). Across the
//! documents a composition folds, a requirement only ever holds or is added:
//! the most trusted document that requires confinement for a launch kind
//! decides it ([`ConfinementConfig::fold_less_trusted`]), and a less trusted
//! document that states that kind differently is refused
//! ([`check_confinement_layers`]).

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::confinement::ExecutionConfinement;

/// The kinds of process a host launches for its agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfinementLaunchKind {
    /// The shell tool: foreground calls, background jobs and monitors.
    Shell,
    /// Command hooks.
    CommandHooks,
    /// Local MCP stdio servers.
    McpStdio,
}

impl ConfinementLaunchKind {
    /// Every launch kind, in table order.
    pub const ALL: [Self; 3] = [Self::Shell, Self::CommandHooks, Self::McpStdio];

    /// The kind's key in the `[confinement]` table.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shell => "shell",
            Self::CommandHooks => "command_hooks",
            Self::McpStdio => "mcp_stdio",
        }
    }
}

impl std::fmt::Display for ConfinementLaunchKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `[confinement]`: the OS process confinement a host applies to the
/// processes its agents launch.
///
/// `all` covers every launch kind, including kinds a later release adds. A
/// kind's own table replaces `all` for that kind within the same document.
/// An empty table states nothing, so the document inherits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
pub struct ConfinementConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub all: Option<LaunchConfinement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<LaunchConfinement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_hooks: Option<LaunchConfinement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_stdio: Option<LaunchConfinement>,
}

impl ConfinementConfig {
    /// Whether the table states nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// One selection for every launch kind: the operator's flags.
    #[must_use]
    pub fn for_all(selection: LaunchConfinement) -> Self {
        Self {
            all: Some(selection),
            ..Self::default()
        }
    }

    /// This document's statement for `kind`: the kind's own table, else
    /// `all`. `None` states nothing for the kind.
    #[must_use]
    pub fn statement(&self, kind: ConfinementLaunchKind) -> Option<&LaunchConfinement> {
        self.slot(kind).as_ref().or(self.all.as_ref())
    }

    /// Whether this selection and `other` require the same confinement of
    /// every launch kind. `trusted_host` requires nothing, so it is the same
    /// as no statement, and it does not matter whether a requirement is
    /// stated in `all` or in the kind's own table.
    #[must_use]
    pub fn requires_the_same(&self, other: &Self) -> bool {
        ConfinementLaunchKind::ALL
            .into_iter()
            .all(|kind| self.requirement(kind) == other.requirement(kind))
    }

    /// The confinement this selection requires of `kind`, `None` for the
    /// trusted host.
    fn requirement(&self, kind: ConfinementLaunchKind) -> Option<&LaunchConfinement> {
        self.statement(kind)
            .filter(|statement| statement.requires_confinement())
    }

    fn slot(&self, kind: ConfinementLaunchKind) -> &Option<LaunchConfinement> {
        match kind {
            ConfinementLaunchKind::Shell => &self.shell,
            ConfinementLaunchKind::CommandHooks => &self.command_hooks,
            ConfinementLaunchKind::McpStdio => &self.mcp_stdio,
        }
    }

    fn slot_mut(&mut self, kind: ConfinementLaunchKind) -> &mut Option<LaunchConfinement> {
        match kind {
            ConfinementLaunchKind::Shell => &mut self.shell,
            ConfinementLaunchKind::CommandHooks => &mut self.command_hooks,
            ConfinementLaunchKind::McpStdio => &mut self.mcp_stdio,
        }
    }

    /// Fold `less_trusted`, the next document of a composition ordered from
    /// the most trusted, under this accumulated one.
    ///
    /// Per launch kind, a requirement this fold already holds stays; only a
    /// kind that no more trusted document confined takes `less_trusted`'s
    /// statement. A less trusted document can therefore add confinement but
    /// never replace or remove it. Folding under nothing keeps the document
    /// as written.
    pub fn fold_less_trusted(&mut self, less_trusted: &Self) {
        if self.is_empty() {
            self.clone_from(less_trusted);
            return;
        }
        for kind in ConfinementLaunchKind::ALL {
            let decided = self
                .statement(kind)
                .filter(|statement| statement.requires_confinement())
                .cloned();
            let folded = match decided {
                Some(requirement) => Some(requirement),
                None => less_trusted
                    .statement(kind)
                    .or_else(|| self.statement(kind))
                    .cloned(),
            };
            *self.slot_mut(kind) = folded;
        }
        let all_decided = self
            .all
            .as_ref()
            .is_some_and(LaunchConfinement::requires_confinement);
        if !all_decided && let Some(all) = &less_trusted.all {
            self.all = Some(all.clone());
        }
    }
}

/// A less trusted config document states confinement for a launch kind
/// differently from the more trusted document that decided it. Nothing is
/// composed: the operator removes or aligns the less trusted statement.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{kind} confinement is required by realm '{decided_by}'; the less trusted realm '{conflicting}' states it differently (remove or align its [confinement] statement)"
)]
pub struct ConfinementLayerConflict {
    /// The launch kind both documents state.
    pub kind: ConfinementLaunchKind,
    /// The realm whose document decided the requirement.
    pub decided_by: String,
    /// The less trusted realm whose document states it differently.
    pub conflicting: String,
}

/// Refuse a less trusted document that weakens or replaces a requirement.
///
/// `layers` runs from the most trusted document to the least trusted one,
/// each with its realm. Per launch kind, the first document whose statement
/// requires confinement decides it, and every later statement of that kind
/// must equal it. `trusted_host` never decides, so a less trusted document
/// may still require confinement for a kind no more trusted one confined.
pub fn check_confinement_layers<'a>(
    layers: impl IntoIterator<Item = (&'a str, &'a ConfinementConfig)>,
) -> Result<(), ConfinementLayerConflict> {
    let layers: Vec<(&str, &ConfinementConfig)> = layers.into_iter().collect();
    for kind in ConfinementLaunchKind::ALL {
        let mut decided: Option<(&str, &LaunchConfinement)> = None;
        for &(realm, document) in &layers {
            let Some(statement) = document.statement(kind) else {
                continue;
            };
            match decided {
                None if statement.requires_confinement() => decided = Some((realm, statement)),
                None => {}
                Some((decided_by, requirement)) if statement != requirement => {
                    return Err(ConfinementLayerConflict {
                        kind,
                        decided_by: decided_by.to_string(),
                        conflicting: realm.to_string(),
                    });
                }
                Some(_) => {}
            }
        }
    }
    Ok(())
}

/// One launch kind's confinement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "mode",
    rename_all = "snake_case",
    from = "LaunchConfinementWire"
)]
pub enum LaunchConfinement {
    /// The launch runs as the trusted host, with no OS confinement: the
    /// default. It never overrides a more trusted document's requirement.
    TrustedHost,
    /// The workspace preset over the host's workspace root.
    Workspace(WorkspaceConfinement),
    /// An explicit requirement and working directory.
    Custom(CustomConfinement),
}

impl LaunchConfinement {
    /// Whether the selection requires OS confinement.
    #[must_use]
    pub fn requires_confinement(&self) -> bool {
        !matches!(self, Self::TrustedHost)
    }
}

// Decode through a closed empty struct variant: content beside
// `mode = "trusted_host"` is refused instead of discarded.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
enum LaunchConfinementWire {
    TrustedHost {},
    Workspace(WorkspaceConfinement),
    Custom(CustomConfinement),
}

impl From<LaunchConfinementWire> for LaunchConfinement {
    fn from(value: LaunchConfinementWire) -> Self {
        match value {
            LaunchConfinementWire::TrustedHost {} => Self::TrustedHost,
            LaunchConfinementWire::Workspace(workspace) => Self::Workspace(workspace),
            LaunchConfinementWire::Custom(custom) => Self::Custom(custom),
        }
    }
}

/// The workspace preset: the launched process reads and writes the host's
/// workspace root and the `write` subtrees, reads the `read` subtrees, has
/// the platform baseline (`command_runtime_v1`), and opens IP sockets only
/// when `network` allows it. It gets no deny lists, Unix socket grants or
/// descendant-termination requirement; use [`CustomConfinement`] for those.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "WorkspaceConfinementWire")]
pub struct WorkspaceConfinement {
    pub network: WorkspaceNetwork,
    /// Extra read-only subtrees (absolute).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<PathBuf>,
    /// Extra writable subtrees (absolute); each is also readable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub write: Vec<PathBuf>,
}

#[derive(Default, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(default, deny_unknown_fields)]
struct WorkspaceConfinementWire {
    network: WorkspaceNetwork,
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
}

impl TryFrom<WorkspaceConfinementWire> for WorkspaceConfinement {
    type Error = InvalidConfinementPath;

    fn try_from(value: WorkspaceConfinementWire) -> Result<Self, Self::Error> {
        for path in value.read.iter().chain(&value.write) {
            validate_confinement_path(path)?;
        }
        Ok(Self {
            network: value.network,
            read: value.read,
            write: value.write,
        })
    }
}

/// IP network access of a workspace-confined launch. Exact connect
/// endpoints are not offered: neither platform backend enforces them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceNetwork {
    /// No IP sockets (the default).
    #[default]
    Denied,
    /// IP sockets over the host's own network.
    Unrestricted,
}

/// An explicit requirement in the [`crate::confinement::ConfinementSpec`]
/// wire shape, and the absolute working directory of the command hooks and
/// MCP stdio servers it confines. The shell keeps its own working directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "CustomConfinementWire")]
pub struct CustomConfinement {
    pub working_directory: PathBuf,
    pub requirement: ExecutionConfinement,
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
struct CustomConfinementWire {
    working_directory: PathBuf,
    requirement: ExecutionConfinement,
}

impl TryFrom<CustomConfinementWire> for CustomConfinement {
    type Error = InvalidConfinementPath;

    fn try_from(value: CustomConfinementWire) -> Result<Self, Self::Error> {
        validate_confinement_path(&value.working_directory)?;
        Ok(Self {
            working_directory: value.working_directory,
            requirement: value.requirement,
        })
    }
}

/// A confinement path that is not absolute, names `.` or `..`, or holds NUL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "confinement path '{}' must be absolute, without '.' or '..' components",
    .0.display()
)]
pub struct InvalidConfinementPath(pub PathBuf);

/// The path rule of [`crate::confinement::ExecutionConfinement`].
fn validate_confinement_path(path: &Path) -> Result<(), InvalidConfinementPath> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        || path.as_os_str().as_encoded_bytes().contains(&0)
    {
        return Err(InvalidConfinementPath(path.to_path_buf()));
    }
    Ok(())
}

/// The confinement mode a stock binary's `--confinement` flag selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfinementFlagMode {
    /// `--confinement trusted-host`.
    TrustedHost,
    /// `--confinement workspace`.
    Workspace,
}

/// The operator's per-process selection on a stock binary's command line:
/// `--confinement`, `--confinement-network`, `--confinement-read` and
/// `--confinement-write`. Given, it replaces every document's
/// `[confinement]` for every launch kind.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfinementFlags {
    pub mode: Option<ConfinementFlagMode>,
    pub network: Option<WorkspaceNetwork>,
    pub read: Vec<PathBuf>,
    pub write: Vec<PathBuf>,
}

/// Flags that state no valid selection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfinementFlagsError {
    /// A workspace option without `--confinement workspace`.
    #[error(
        "--confinement-network, --confinement-read and --confinement-write need --confinement workspace"
    )]
    WorkspaceOptionWithoutWorkspace,
    /// A path that stays invalid after resolving it against the working
    /// directory.
    #[error(transparent)]
    InvalidPath(#[from] InvalidConfinementPath),
}

impl ConfinementFlags {
    /// Whether [`Self::selection`] resolves a path against its base: a
    /// `--confinement workspace` selection with a relative
    /// `--confinement-read` or `--confinement-write` path. Otherwise the
    /// selection never reads its base, so a process reads its working
    /// directory for it only when this holds, and one started without such
    /// a path does not depend on that directory (it may have been removed).
    #[must_use]
    pub fn needs_working_directory(&self) -> bool {
        self.mode == Some(ConfinementFlagMode::Workspace)
            && self
                .read
                .iter()
                .chain(&self.write)
                .any(|path| path.is_relative())
    }

    /// The selection the flags state, or `None` without `--confinement`.
    /// Relative paths resolve against `base`, the process's working
    /// directory; `.` and `..` components are refused, not normalized.
    pub fn selection(
        self,
        base: &Path,
    ) -> Result<Option<LaunchConfinement>, ConfinementFlagsError> {
        let workspace_options =
            self.network.is_some() || !self.read.is_empty() || !self.write.is_empty();
        match self.mode {
            None if workspace_options => {
                Err(ConfinementFlagsError::WorkspaceOptionWithoutWorkspace)
            }
            None => Ok(None),
            Some(ConfinementFlagMode::TrustedHost) if workspace_options => {
                Err(ConfinementFlagsError::WorkspaceOptionWithoutWorkspace)
            }
            Some(ConfinementFlagMode::TrustedHost) => Ok(Some(LaunchConfinement::TrustedHost)),
            Some(ConfinementFlagMode::Workspace) => {
                let resolve =
                    |paths: Vec<PathBuf>| -> Result<Vec<PathBuf>, InvalidConfinementPath> {
                        paths
                            .into_iter()
                            .map(|path| {
                                let path = if path.is_absolute() {
                                    path
                                } else {
                                    base.join(path)
                                };
                                validate_confinement_path(&path).map(|()| path)
                            })
                            .collect()
                    };
                Ok(Some(LaunchConfinement::Workspace(WorkspaceConfinement {
                    network: self.network.unwrap_or_default(),
                    read: resolve(self.read)?,
                    write: resolve(self.write)?,
                })))
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::confinement::{
        ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess, PlatformBaseline,
    };

    fn parse(text: &str) -> Result<ConfinementConfig, toml::de::Error> {
        #[derive(Deserialize)]
        struct Document {
            confinement: ConfinementConfig,
        }
        toml::from_str::<Document>(text).map(|document| document.confinement)
    }

    fn workspace() -> LaunchConfinement {
        LaunchConfinement::Workspace(WorkspaceConfinement::default())
    }

    fn custom(root: &str) -> LaunchConfinement {
        LaunchConfinement::Custom(CustomConfinement {
            working_directory: PathBuf::from(root),
            requirement: ConfinementSpec {
                baseline: PlatformBaseline::CommandRuntimeV1,
                read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.into())]),
                write: FilesystemAccess::Paths(vec![]),
                deny_read: vec![],
                deny_write: vec![],
                network: IpNetworkAccess::Denied,
                unix_connect: vec![],
                require_descendant_termination: false,
            }
            .try_into()
            .unwrap(),
        })
    }

    #[test]
    fn table_parses_every_mode_and_per_kind_override() {
        let config = parse(
            r#"
[confinement.all]
mode = "workspace"
network = "unrestricted"
read = ["/opt/tool"]
write = ["/tmp/build"]

[confinement.mcp_stdio]
mode = "trusted_host"

[confinement.command_hooks]
mode = "custom"
working_directory = "/srv/hooks"
requirement = { baseline = "command_runtime_v1", read = { kind = "paths", paths = [{ kind = "subtree", path = "/srv/hooks" }] }, write = { kind = "paths", paths = [] }, deny_read = [], deny_write = [], network = { kind = "denied" }, unix_connect = [], require_descendant_termination = false }
"#,
        )
        .unwrap();
        assert_eq!(
            config.statement(ConfinementLaunchKind::Shell),
            Some(&LaunchConfinement::Workspace(WorkspaceConfinement {
                network: WorkspaceNetwork::Unrestricted,
                read: vec!["/opt/tool".into()],
                write: vec!["/tmp/build".into()],
            }))
        );
        assert_eq!(
            config.statement(ConfinementLaunchKind::McpStdio),
            Some(&LaunchConfinement::TrustedHost)
        );
        assert_eq!(
            config.statement(ConfinementLaunchKind::CommandHooks),
            Some(&custom("/srv/hooks"))
        );
        // A typed TOML round trip keeps the table exactly.
        #[derive(Serialize)]
        struct Out<'a> {
            confinement: &'a ConfinementConfig,
        }
        let encoded = toml::to_string_pretty(&Out {
            confinement: &config,
        })
        .unwrap();
        assert_eq!(parse(&encoded).unwrap(), config);
    }

    #[test]
    fn workspace_defaults_to_no_network_and_no_extra_grants() {
        let config = parse("[confinement.all]\nmode = \"workspace\"\n").unwrap();
        assert_eq!(config.all, Some(workspace()));
        assert!(parse("[confinement]\n").unwrap().is_empty());
    }

    /// Two selections require the same when every launch kind gets the same
    /// requirement, wherever each states it; `trusted_host` requires
    /// nothing.
    #[test]
    fn requirements_compare_per_launch_kind() {
        let all = parse("[confinement.all]\nmode = \"workspace\"\n").unwrap();
        let per_kind = parse(
            "[confinement.shell]\nmode = \"workspace\"\n\n[confinement.command_hooks]\nmode = \"workspace\"\n\n[confinement.mcp_stdio]\nmode = \"workspace\"\n",
        )
        .unwrap();
        let shell_only = parse("[confinement.shell]\nmode = \"workspace\"\n").unwrap();
        let trusted = parse("[confinement.all]\nmode = \"trusted_host\"\n").unwrap();
        let networked =
            parse("[confinement.all]\nmode = \"workspace\"\nnetwork = \"unrestricted\"\n").unwrap();

        assert!(all.requires_the_same(&per_kind) && per_kind.requires_the_same(&all));
        assert!(trusted.requires_the_same(&ConfinementConfig::default()));
        assert!(!all.requires_the_same(&shell_only));
        assert!(!all.requires_the_same(&networked));
        assert!(!all.requires_the_same(&trusted));
    }

    #[test]
    fn unknown_or_misplaced_keys_are_refused_not_ignored() {
        for text in [
            "[confinement]\nmode = \"workspace\"\n",
            "[confinement.everything]\nmode = \"workspace\"\n",
            "[confinement.all]\nmode = \"sandbox\"\n",
            "[confinement.all]\nnetwork = \"denied\"\n",
            "[confinement.all]\nmode = \"trusted_host\"\nnetwork = \"denied\"\n",
            "[confinement.all]\nmode = \"workspace\"\nnetwork = \"connect\"\n",
            "[confinement.all]\nmode = \"workspace\"\nwritable = [\"/tmp\"]\n",
            "[confinement.all]\nmode = \"custom\"\nworking_directory = \"/srv\"\n",
            "[confinement.all]\nmode = \"workspace\"\nworking_directory = \"/srv\"\n",
        ] {
            assert!(parse(text).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn relative_or_dotted_paths_are_refused() {
        for text in [
            "[confinement.all]\nmode = \"workspace\"\nread = [\"opt\"]\n",
            "[confinement.all]\nmode = \"workspace\"\nwrite = [\"/work/../etc\"]\n",
            "[confinement.shell]\nmode = \"custom\"\nworking_directory = \"srv\"\nrequirement = { baseline = \"command_runtime_v1\", read = { kind = \"unrestricted\" }, write = { kind = \"unrestricted\" }, deny_read = [], deny_write = [], network = { kind = \"denied\" }, unix_connect = [], require_descendant_termination = false }\n",
            "[confinement.shell]\nmode = \"custom\"\nworking_directory = \"/srv\"\nrequirement = { baseline = \"command_runtime_v1\", read = { kind = \"paths\", paths = [{ kind = \"subtree\", path = \"relative\" }] }, write = { kind = \"unrestricted\" }, deny_read = [], deny_write = [], network = { kind = \"denied\" }, unix_connect = [], require_descendant_termination = false }\n",
        ] {
            assert!(parse(text).is_err(), "accepted: {text}");
        }
    }

    #[test]
    fn a_kind_table_replaces_all_within_one_document() {
        let config = ConfinementConfig {
            all: Some(workspace()),
            shell: Some(LaunchConfinement::TrustedHost),
            ..ConfinementConfig::default()
        };
        assert_eq!(
            config.statement(ConfinementLaunchKind::Shell),
            Some(&LaunchConfinement::TrustedHost)
        );
        assert_eq!(
            config.statement(ConfinementLaunchKind::McpStdio),
            Some(&workspace())
        );
    }

    #[test]
    fn a_less_trusted_document_never_weakens_or_replaces_a_requirement() {
        let mut folded = ConfinementConfig::for_all(workspace());
        folded.fold_less_trusted(&ConfinementConfig {
            all: Some(LaunchConfinement::TrustedHost),
            shell: Some(custom("/elsewhere")),
            ..ConfinementConfig::default()
        });
        for kind in ConfinementLaunchKind::ALL {
            assert_eq!(folded.statement(kind), Some(&workspace()), "{kind}");
        }
        assert_eq!(folded.all, Some(workspace()));
    }

    #[test]
    fn a_less_trusted_document_can_add_confinement() {
        let mut folded = ConfinementConfig {
            all: Some(LaunchConfinement::TrustedHost),
            ..ConfinementConfig::default()
        };
        folded.fold_less_trusted(&ConfinementConfig {
            shell: Some(workspace()),
            ..ConfinementConfig::default()
        });
        assert_eq!(
            folded.statement(ConfinementLaunchKind::Shell),
            Some(&workspace())
        );
        assert_eq!(
            folded.statement(ConfinementLaunchKind::McpStdio),
            Some(&LaunchConfinement::TrustedHost)
        );
        let mut empty = ConfinementConfig::default();
        empty.fold_less_trusted(&ConfinementConfig::for_all(workspace()));
        for kind in ConfinementLaunchKind::ALL {
            assert_eq!(empty.statement(kind), Some(&workspace()), "{kind}");
        }
    }

    #[test]
    fn layers_refuse_a_differing_less_trusted_statement() {
        let user = ConfinementConfig::for_all(workspace());
        let same = ConfinementConfig::for_all(workspace());
        let weaker = ConfinementConfig {
            mcp_stdio: Some(LaunchConfinement::TrustedHost),
            ..ConfinementConfig::default()
        };
        let other = ConfinementConfig {
            shell: Some(custom("/srv")),
            ..ConfinementConfig::default()
        };
        let silent = ConfinementConfig::default();
        assert_eq!(
            check_confinement_layers([("global", &user), ("ws", &same), ("x", &silent)]),
            Ok(())
        );
        assert_eq!(
            check_confinement_layers([("global", &user), ("ws", &weaker)]),
            Err(ConfinementLayerConflict {
                kind: ConfinementLaunchKind::McpStdio,
                decided_by: "global".into(),
                conflicting: "ws".into(),
            })
        );
        assert_eq!(
            check_confinement_layers([("global", &user), ("ws", &other)]),
            Err(ConfinementLayerConflict {
                kind: ConfinementLaunchKind::Shell,
                decided_by: "global".into(),
                conflicting: "ws".into(),
            })
        );
        // `trusted_host` never decides: a less trusted document may add.
        let unconfined = ConfinementConfig::for_all(LaunchConfinement::TrustedHost);
        assert_eq!(
            check_confinement_layers([("global", &unconfined), ("ws", &user)]),
            Ok(())
        );
    }

    #[test]
    fn flags_select_the_workspace_preset_or_trusted_host() {
        let base = Path::new("/work");
        assert_eq!(ConfinementFlags::default().selection(base), Ok(None));
        assert_eq!(
            ConfinementFlags {
                mode: Some(ConfinementFlagMode::TrustedHost),
                ..ConfinementFlags::default()
            }
            .selection(base),
            Ok(Some(LaunchConfinement::TrustedHost))
        );
        assert_eq!(
            ConfinementFlags {
                mode: Some(ConfinementFlagMode::Workspace),
                network: Some(WorkspaceNetwork::Unrestricted),
                read: vec!["/opt/tool".into()],
                write: vec!["target".into()],
            }
            .selection(base),
            Ok(Some(LaunchConfinement::Workspace(WorkspaceConfinement {
                network: WorkspaceNetwork::Unrestricted,
                read: vec!["/opt/tool".into()],
                write: vec!["/work/target".into()],
            })))
        );
    }

    #[test]
    fn only_a_relative_workspace_path_needs_the_working_directory() {
        let workspace = |read: &[&str], write: &[&str]| ConfinementFlags {
            mode: Some(ConfinementFlagMode::Workspace),
            network: None,
            read: read.iter().map(PathBuf::from).collect(),
            write: write.iter().map(PathBuf::from).collect(),
        };
        assert!(!ConfinementFlags::default().needs_working_directory());
        assert!(
            !ConfinementFlags {
                mode: Some(ConfinementFlagMode::TrustedHost),
                ..ConfinementFlags::default()
            }
            .needs_working_directory()
        );
        assert!(!workspace(&[], &[]).needs_working_directory());
        assert!(!workspace(&["/opt/tool"], &["/tmp/build"]).needs_working_directory());
        assert!(workspace(&["tool"], &[]).needs_working_directory());
        assert!(workspace(&["/opt/tool"], &["build"]).needs_working_directory());
        // Without the workspace mode a path is a usage error, never resolved.
        assert!(
            !ConfinementFlags {
                write: vec!["build".into()],
                ..ConfinementFlags::default()
            }
            .needs_working_directory()
        );
        // A selection that needs no base never reads it.
        assert_eq!(
            workspace(&["/opt/tool"], &[]).selection(Path::new("")),
            Ok(Some(LaunchConfinement::Workspace(WorkspaceConfinement {
                network: WorkspaceNetwork::Denied,
                read: vec!["/opt/tool".into()],
                write: vec![],
            })))
        );
    }

    #[test]
    fn flags_refuse_workspace_options_without_workspace_and_dotted_paths() {
        let base = Path::new("/work");
        for flags in [
            ConfinementFlags {
                network: Some(WorkspaceNetwork::Denied),
                ..ConfinementFlags::default()
            },
            ConfinementFlags {
                mode: Some(ConfinementFlagMode::TrustedHost),
                write: vec!["/tmp".into()],
                ..ConfinementFlags::default()
            },
        ] {
            assert_eq!(
                flags.selection(base),
                Err(ConfinementFlagsError::WorkspaceOptionWithoutWorkspace)
            );
        }
        assert!(matches!(
            ConfinementFlags {
                mode: Some(ConfinementFlagMode::Workspace),
                read: vec!["../outside".into()],
                ..ConfinementFlags::default()
            }
            .selection(base),
            Err(ConfinementFlagsError::InvalidPath(_))
        ));
    }
}
