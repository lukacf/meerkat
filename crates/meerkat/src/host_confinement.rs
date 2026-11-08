//! The OS process confinement a host applies to the processes its agents
//! launch, resolved once from the effective `[confinement]` selection (the
//! realm's documents, then the operator's flags) and installed on the host's
//! [`crate::AgentFactory`] with [`crate::AgentFactory::with_host_confinement`].
//!
//! Resolution lowers each launch kind's selection into the mechanical
//! [`ExecutionConfinement`], compiles each distinct requirement once for the
//! host's report, and retains each kind's write authority. It authorizes
//! nothing. A launch the platform backend cannot confine is refused at that
//! launch with its typed [`ConfinementRefusal`] and never runs unconfined.
//!
//! A host that confines every launch kind also keeps the inputs it reads its
//! own administration from out of its confined processes' reach: a start
//! whose realm state or user config document a confined process could
//! change is refused ([`HostConfinement::admit_host_inputs`]), and a
//! convention source (an `mcp.toml` or a project `config.toml` with hooks)
//! that one could change is not trusted ([`HostConfinement::untrusted_source`]).
//! A host that runs any launch kind as the trusted host claims none of this
//! ([`HostAuthority::Withdrawn`]).

use std::path::{Component, Path, PathBuf};

use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_core::{
    ConfinementConfig, ConfinementLaunchKind, LaunchConfinement, WorkspaceConfinement,
    WorkspaceNetwork,
};
use meerkat_sandbox::ConfinementBackend;

/// A host confinement selection that cannot be installed, or a start whose
/// own administration inputs it cannot protect. The host refuses to start
/// rather than run a launch kind it was asked to confine as the trusted
/// host, or serve from inputs a confined process could change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HostConfinementError {
    /// A `workspace` selection on a host without a workspace root.
    #[error(
        "{kind} confinement mode `workspace` needs a workspace root; start the host with --context-root <dir>"
    )]
    WorkspaceRootRequired { kind: ConfinementLaunchKind },
    /// The workspace root is not absolute.
    #[error(
        "{kind} confinement mode `workspace` needs an absolute workspace root, not '{}'",
        root.display()
    )]
    RelativeWorkspaceRoot {
        kind: ConfinementLaunchKind,
        root: PathBuf,
    },
    /// The lowered requirement is invalid.
    #[error("{kind} confinement requirement is invalid: {refusal}")]
    InvalidRequirement {
        kind: ConfinementLaunchKind,
        refusal: ConfinementRefusal,
    },
    /// A confined process of `kind` could change `input`, which the host
    /// reads its own administration from (its confinement, governance,
    /// bindings and the servers and hooks it runs).
    #[error(
        "{kind} confinement lets a confined process change {input} at '{}'; {}",
        path.display(),
        input.remedy()
    )]
    AdministrationInputWritable {
        input: HostInput,
        path: PathBuf,
        kind: ConfinementLaunchKind,
    },
}

/// An input a stock host reads its own administration from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostInput {
    /// The realm state root: every realm config document of the host (its
    /// head realm's and its ancestors') and the head realm's stores.
    RealmState,
    /// The user config document (`<user config root>/.rkat/config.toml`),
    /// the reserved global realm's document.
    UserDocument,
    /// The host's file credential store: the tokens, keys and secrets its
    /// providers, MCP servers, connectors and comms identity authenticate
    /// with.
    CredentialStore,
}

impl HostInput {
    /// What the operator does about an input a confined process could change.
    #[must_use]
    pub const fn remedy(self) -> &'static str {
        match self {
            Self::RealmState => {
                "start the host with --state-root <dir> outside every confinement write grant"
            }
            Self::UserDocument => {
                "keep the user config root outside every confinement write grant, or start the host with --user-config-root <dir> outside them"
            }
            Self::CredentialStore => {
                "keep the credential store outside every confinement write grant (it lives in the user's config directory, XDG_CONFIG_HOME on Linux)"
            }
        }
    }

    /// Whether the host reads the whole subtree under the input's path.
    const fn is_subtree(self) -> bool {
        matches!(self, Self::RealmState | Self::CredentialStore)
    }
}

impl std::fmt::Display for HostInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RealmState => "the realm state",
            Self::UserDocument => "the user config document",
            Self::CredentialStore => "the credential store",
        })
    }
}

/// What a host's confinement guarantees about the inputs it reads its own
/// administration from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAuthority {
    /// No launch kind is confined: the host runs exactly as an unconfined
    /// host does.
    NotConfined,
    /// These launch kinds run as the trusted host, so a process the agent
    /// drives can still change every input: no guarantee is claimed.
    Withdrawn {
        trusted_host: Vec<ConfinementLaunchKind>,
    },
    /// Every launch kind is confined: an administration input a confined
    /// process could change refuses the start, and a convention source one
    /// could change is not trusted.
    Guarded,
}

/// One line of a host's startup report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfinementNotice {
    /// Whether the operator should act on it.
    pub warning: bool,
    pub text: String,
}

/// One granted or protected path of a write requirement, resolved to where
/// it actually lives (its existing ancestors canonical).
#[derive(Debug, Clone, PartialEq, Eq)]
struct WritePath {
    path: PathBuf,
    subtree: bool,
}

impl WritePath {
    fn from_access(access: &PathAccess) -> Self {
        Self {
            path: resolve_existing(access.path()),
            subtree: matches!(access, PathAccess::Subtree(_)),
        }
    }

    fn covers(&self, path: &Path) -> bool {
        if self.subtree {
            path.starts_with(&self.path)
        } else {
            path == self.path
        }
    }
}

/// One confined launch kind's write authority, as its requirement states
/// it: what its processes may write, and what they are kept from writing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KindWrites {
    everything: bool,
    grants: Vec<WritePath>,
    protected: Vec<WritePath>,
}

impl KindWrites {
    fn from_requirement(requirement: &ExecutionConfinement) -> Self {
        let spec = requirement.specification();
        let (everything, grants) = match &spec.write {
            FilesystemAccess::Unrestricted => (true, Vec::new()),
            FilesystemAccess::Paths(paths) => {
                (false, paths.iter().map(WritePath::from_access).collect())
            }
        };
        Self {
            everything,
            grants,
            protected: spec.deny_write.iter().map(WritePath::from_access).collect(),
        }
    }

    /// Whether a process of this kind may create, change, remove or rename
    /// the directory entry at `path`.
    fn writes(&self, path: &Path) -> bool {
        (self.everything || self.grants.iter().any(|grant| grant.covers(path)))
            && !self
                .protected
                .iter()
                .any(|protection| protection.covers(path))
    }

    /// Whether a process of this kind may write anywhere outside
    /// `directory`, where a directory it can rename could be moved to.
    fn writes_outside(&self, directory: &Path) -> bool {
        self.everything
            || self
                .grants
                .iter()
                .any(|grant| !grant.path.starts_with(directory))
    }

    /// Whether a process of this kind may write anywhere inside `directory`.
    fn writes_inside(&self, directory: &Path) -> bool {
        (self.everything && self.writes(directory))
            || self.grants.iter().any(|grant| {
                grant.path.starts_with(directory)
                    && !self
                        .protected
                        .iter()
                        .any(|protection| protection.covers(&grant.path))
            })
    }

    /// Whether a process of this kind could change what the host reads at
    /// `path` (`subtree`: everything under it): write the object itself,
    /// create it where it is missing, or replace a directory entry on the
    /// route the host's open follows. Each entry is judged where it
    /// actually lives, and a symbolic link is followed to the rest of its
    /// route; an entry this kind cannot create ends the route, since nothing
    /// below a missing entry exists until it does.
    fn controls(&self, path: &Path, subtree: bool) -> bool {
        let absolute;
        let path: &Path = if path.is_absolute() {
            path
        } else {
            absolute = std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf());
            &absolute
        };
        let normal: Vec<&std::ffi::OsStr> = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            // A route with `.` or `..` is judged at its resolved location.
            return self.controls_resolved(&resolve_existing(path), subtree);
        }
        let mut parent = path
            .ancestors()
            .last()
            .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
        for (index, name) in normal.iter().enumerate() {
            let entry = parent.join(name);
            let last = index + 1 == normal.len();
            let Ok(metadata) = std::fs::symlink_metadata(&entry) else {
                return self.writes(&entry);
            };
            if metadata.file_type().is_symlink() {
                if self.writes(&entry) {
                    return true;
                }
                // A link whose target cannot be resolved is unresolved
                // ancestry: judged conservatively.
                let Ok(target) = std::fs::canonicalize(&entry) else {
                    return true;
                };
                if self.controls_canonical_route(&target, last) {
                    return true;
                }
                parent = target;
                continue;
            }
            if self.writes(&entry) && (last || self.writes_outside(&entry)) {
                return true;
            }
            parent = entry;
        }
        subtree && self.writes_inside(&parent)
    }

    /// [`Self::controls`] for a route already resolved to where it lives.
    fn controls_resolved(&self, path: &Path, subtree: bool) -> bool {
        if self.controls_canonical_route(path, true) {
            return true;
        }
        subtree && self.writes_inside(path)
    }

    /// Whether an entry on a route without symbolic links could be changed:
    /// any directory on it this kind could rename away, or the final object
    /// itself when `object` is set.
    fn controls_canonical_route(&self, route: &Path, object: bool) -> bool {
        let mut prefix = PathBuf::new();
        for component in route.components() {
            prefix.push(component);
            if !matches!(component, Component::Normal(_)) {
                continue;
            }
            let last = prefix.as_path() == route;
            if !self.writes(&prefix) {
                continue;
            }
            if (last && object) || self.writes_outside(&prefix) {
                return true;
            }
            if std::fs::symlink_metadata(&prefix).is_err() {
                // Missing and creatable.
                return true;
            }
        }
        false
    }
}

/// `path` through its longest existing ancestor's canonical form, with the
/// missing rest appended.
fn resolve_existing(path: &Path) -> PathBuf {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&existing) {
            return missing.iter().rev().fold(canonical, |mut resolved, name| {
                resolved.push(name);
                resolved
            });
        }
        match (
            existing.file_name().map(ToOwned::to_owned),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// One required launch kind: its requirement, the working directory of the
/// processes that launch from a fixed directory (command hooks and MCP
/// stdio servers; the shell keeps its own), the mode its selection
/// requested, what this platform's backend made of it at resolution, and
/// its write authority.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequiredLaunch {
    requirement: ExecutionConfinement,
    working_directory: PathBuf,
    requested: &'static str,
    backend: Result<ConfinementBackend, ConfinementRefusal>,
    writes: KindWrites,
}

/// The host's resolved OS process confinement, per launch kind. A kind
/// without a requirement launches as the trusted host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostConfinement {
    shell: Option<RequiredLaunch>,
    command_hooks: Option<RequiredLaunch>,
    mcp_stdio: Option<RequiredLaunch>,
}

impl HostConfinement {
    /// Every launch kind runs as the trusted host: the default.
    #[must_use]
    pub fn trusted_host() -> Self {
        Self::default()
    }

    /// Lower the effective `[confinement]` selection (documents composed,
    /// flags applied) over `workspace_root`, the host's workspace: the shell's
    /// project root, which the `workspace` preset grants and command hooks and
    /// MCP stdio servers start in. A host without one refuses that preset.
    /// Each distinct requirement is compiled once, for the host's report.
    pub fn resolve(
        selection: &ConfinementConfig,
        workspace_root: Option<&Path>,
    ) -> Result<Self, HostConfinementError> {
        let mut resolved = Self::default();
        let mut compiled: Vec<(
            ExecutionConfinement,
            Result<ConfinementBackend, ConfinementRefusal>,
        )> = Vec::new();
        for kind in ConfinementLaunchKind::ALL {
            let launch = match selection.statement(kind) {
                None | Some(LaunchConfinement::TrustedHost) => None,
                Some(LaunchConfinement::Workspace(workspace)) => Some((
                    "workspace",
                    lower_workspace(kind, workspace, workspace_root)?,
                )),
                Some(LaunchConfinement::Custom(custom)) => Some((
                    "custom",
                    (custom.requirement.clone(), custom.working_directory.clone()),
                )),
            };
            *resolved.slot_mut(kind) =
                launch.map(|(requested, (requirement, working_directory))| {
                    let backend = match compiled.iter().find(|(seen, _)| *seen == requirement) {
                        Some((_, backend)) => *backend,
                        None => {
                            let backend =
                                meerkat_sandbox::CompiledConfinement::compile(&requirement)
                                    .map(|compiled| compiled.capabilities().backend());
                            compiled.push((requirement.clone(), backend));
                            backend
                        }
                    };
                    RequiredLaunch {
                        writes: KindWrites::from_requirement(&requirement),
                        requirement,
                        working_directory,
                        requested,
                        backend,
                    }
                });
        }
        Ok(resolved)
    }

    fn slot(&self, kind: ConfinementLaunchKind) -> Option<&RequiredLaunch> {
        match kind {
            ConfinementLaunchKind::Shell => self.shell.as_ref(),
            ConfinementLaunchKind::CommandHooks => self.command_hooks.as_ref(),
            ConfinementLaunchKind::McpStdio => self.mcp_stdio.as_ref(),
        }
    }

    fn slot_mut(&mut self, kind: ConfinementLaunchKind) -> &mut Option<RequiredLaunch> {
        match kind {
            ConfinementLaunchKind::Shell => &mut self.shell,
            ConfinementLaunchKind::CommandHooks => &mut self.command_hooks,
            ConfinementLaunchKind::McpStdio => &mut self.mcp_stdio,
        }
    }

    /// Whether every launch kind runs as the trusted host.
    #[must_use]
    pub fn is_trusted_host(&self) -> bool {
        self == &Self::default()
    }

    /// The requirement installed for `kind`, `None` for the trusted host.
    #[must_use]
    pub fn requirement(&self, kind: ConfinementLaunchKind) -> Option<&ExecutionConfinement> {
        self.slot(kind).map(|launch| &launch.requirement)
    }

    /// The fixed working directory of `kind`'s confined launches (command
    /// hooks and MCP stdio servers), `None` for the trusted host.
    #[must_use]
    pub fn working_directory(&self, kind: ConfinementLaunchKind) -> Option<&Path> {
        self.slot(kind)
            .map(|launch| launch.working_directory.as_path())
    }

    /// Each required launch kind this platform's backend cannot enforce, as
    /// compiled once at resolution. This is setup support only, for the
    /// host's startup report: each launch is still checked and refused on its
    /// own, and a refusal here changes nothing.
    #[must_use]
    pub fn preflight(&self) -> Vec<(ConfinementLaunchKind, ConfinementRefusal)> {
        ConfinementLaunchKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let refusal = self.slot(kind)?.backend.err()?;
                Some((kind, refusal))
            })
            .collect()
    }

    /// What this host's confinement guarantees about its own inputs.
    #[must_use]
    pub fn authority(&self) -> HostAuthority {
        let trusted_host: Vec<ConfinementLaunchKind> = ConfinementLaunchKind::ALL
            .into_iter()
            .filter(|kind| self.slot(*kind).is_none())
            .collect();
        if trusted_host.len() == ConfinementLaunchKind::ALL.len() {
            HostAuthority::NotConfined
        } else if trusted_host.is_empty() {
            HostAuthority::Guarded
        } else {
            HostAuthority::Withdrawn { trusted_host }
        }
    }

    /// The first confined launch kind whose processes could change what the
    /// host reads at `path` (`subtree`: anything under it), when the host
    /// claims the guarantee ([`HostAuthority::Guarded`]); `None` otherwise.
    /// A kind writes what any of its grants allows and no protection of its
    /// own denies, whatever other kinds protect.
    #[must_use]
    pub fn writer_of(&self, path: &Path, subtree: bool) -> Option<ConfinementLaunchKind> {
        if self.authority() != HostAuthority::Guarded {
            return None;
        }
        ConfinementLaunchKind::ALL.into_iter().find(|kind| {
            self.slot(*kind)
                .is_some_and(|launch| launch.writes.controls(path, subtree))
        })
    }

    /// Whether the convention source at `path` (an `mcp.toml`, or a project
    /// `config.toml` with hooks) is untrusted: the host claims the guarantee
    /// and a confined process could change it. Its MCP servers and hooks are
    /// then refused before anything in them is expanded, resolved,
    /// connected or launched.
    #[must_use]
    pub fn untrusted_source(&self, path: &Path) -> bool {
        self.writer_of(path, false).is_some()
    }

    /// Whether the convention directory at `path` (a skills directory) is
    /// untrusted: the host claims the guarantee and a confined process could
    /// change anything under it, the directory included. Its skills are then
    /// not loaded.
    #[must_use]
    pub fn untrusted_directory(&self, path: &Path) -> bool {
        self.writer_of(path, true).is_some()
    }

    /// [`Self::writer_of`] as a predicate an owner can keep: whether a
    /// confined process could change what the host reads at a path (`true`:
    /// anything under it as well). A host hands it to the owners of the
    /// programs it runs as itself ([`meerkat_core::HostLaunchExemption`]) and
    /// of its skill sources, so a program, working directory, cache or
    /// linked skill a confined process could change is refused.
    #[must_use]
    pub fn untrusted_paths(&self) -> std::sync::Arc<dyn Fn(&Path, bool) -> bool + Send + Sync> {
        let host = self.clone();
        std::sync::Arc::new(move |path, subtree| host.writer_of(path, subtree).is_some())
    }

    /// [`Self::untrusted_paths`] for the programs the host runs as itself
    /// ([`meerkat_core::HostLaunchExemption`]) and its skill sources, `None`
    /// when no launch kind is confined ([`HostAuthority::NotConfined`]):
    /// there is no confined process to plant anything, so such a program
    /// runs with the host's environment exactly as an unconfined host runs
    /// it.
    #[must_use]
    pub fn launch_guard(
        &self,
    ) -> Option<std::sync::Arc<dyn Fn(&Path, bool) -> bool + Send + Sync>> {
        (self.authority() != HostAuthority::NotConfined).then(|| self.untrusted_paths())
    }

    /// Refuse a new start whose administration inputs a confined process
    /// could change, naming the input, its path and the kind. An input the
    /// host reads as a whole subtree is judged with the symbolic links in
    /// it: the host follows a link out of the input to where it leads. A
    /// host that claims no guarantee admits every input.
    pub fn admit_host_inputs(
        &self,
        inputs: &[(HostInput, PathBuf)],
    ) -> Result<(), HostConfinementError> {
        for (input, path) in inputs {
            let writer = self.writer_of(path, input.is_subtree()).or_else(|| {
                input
                    .is_subtree()
                    .then(|| self.link_writer_under(path))
                    .flatten()
            });
            if let Some(kind) = writer {
                return Err(HostConfinementError::AdministrationInputWritable {
                    input: *input,
                    path: path.clone(),
                    kind,
                });
            }
        }
        Ok(())
    }

    /// The first confined launch kind whose processes could change where a
    /// symbolic link under `directory` leads (a link whose target does not
    /// exist yet included), when the host claims the guarantee. The host
    /// follows such a link when it reads the subtree, so a link out of a
    /// protected input into a write grant hands the input to that kind.
    ///
    /// Cost: a guarded host start reads every directory of each subtree
    /// input (the realm state root among them) once, visiting each
    /// directory once, so the walk grows with the number of directories the
    /// realm state holds (one per session or job store, for example). A
    /// trusted host does not walk.
    fn link_writer_under(&self, directory: &Path) -> Option<ConfinementLaunchKind> {
        if self.authority() != HostAuthority::Guarded {
            return None;
        }
        let mut pending = vec![directory.to_path_buf()];
        let mut visited = std::collections::HashSet::new();
        while let Some(current) = pending.pop() {
            let Ok(canonical) = std::fs::canonicalize(&current) else {
                continue;
            };
            if !visited.insert(canonical) {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                let path = entry.path();
                if file_type.is_symlink() {
                    let Ok(target) = std::fs::read_link(&path) else {
                        continue;
                    };
                    let target = current.join(target);
                    let subtree = target.is_dir();
                    if let Some(kind) = self.writer_of(&target, subtree) {
                        return Some(kind);
                    }
                    if subtree {
                        pending.push(target);
                    }
                } else if file_type.is_dir() {
                    pending.push(path);
                }
            }
        }
        None
    }

    /// The host's startup report: each launch kind's requested and effective
    /// confinement (for a kind the backend cannot confine, its refusal and
    /// any host setting the backend can tell it lacks, such as
    /// `vm.memfd_noexec`), what keeps the host's own inputs from its confined
    /// processes (or that nothing does), and each convention source it does
    /// not trust. Empty for a host that confines nothing.
    #[must_use]
    pub fn startup_report(
        &self,
        inputs: &[(HostInput, PathBuf)],
        convention_sources: &[PathBuf],
    ) -> Vec<HostConfinementNotice> {
        let authority = self.authority();
        if authority == HostAuthority::NotConfined {
            return Vec::new();
        }
        let mut notices = Vec::new();
        // What the backend can tell of the host settings it lacks, asked once
        // and only for a kind it could not confine.
        let mut unmet = None;
        for kind in ConfinementLaunchKind::ALL {
            let (warning, effective) = match self.slot(kind) {
                None => (false, "trusted host".to_owned()),
                Some(launch) => match launch.backend {
                    Ok(backend) => (
                        false,
                        format!(
                            "requested {}, enforced by {}",
                            launch.requested,
                            backend_name(backend)
                        ),
                    ),
                    Err(refusal) => {
                        let mut cause = refusal.to_string();
                        if refusal == ConfinementRefusal::BackendUnavailable {
                            let lacking: &[meerkat_sandbox::UnmetHostRequirement] =
                                unmet.get_or_insert_with(meerkat_sandbox::unmet_host_requirements);
                            for requirement in lacking {
                                cause.push_str("; ");
                                cause.push_str(&requirement.to_string());
                            }
                        }
                        (
                            true,
                            format!(
                                "requested {}, cannot be enforced on this host ({cause}): every launch of this kind is refused",
                                launch.requested
                            ),
                        )
                    }
                },
            };
            notices.push(HostConfinementNotice {
                warning,
                text: format!("{kind} confinement: {effective}"),
            });
        }
        match authority {
            HostAuthority::Withdrawn { trusted_host } => notices.push(HostConfinementNotice {
                warning: true,
                text: format!(
                    "no guarantee for the host's own inputs: {} {} as the trusted host, so a process the agent drives can still change them",
                    trusted_host
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    if trusted_host.len() == 1 { "runs" } else { "run" }
                ),
            }),
            HostAuthority::Guarded => {
                if !inputs.is_empty() {
                    notices.push(HostConfinementNotice {
                        warning: false,
                        text: format!(
                            "kept from every confined process: {}",
                            inputs
                                .iter()
                                .map(|(input, path)| format!("{input} at '{}'", path.display()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
                for source in convention_sources {
                    let untrusted = if source.is_dir() {
                        self.untrusted_directory(source)
                    } else {
                        self.untrusted_source(source)
                    };
                    if untrusted {
                        notices.push(HostConfinementNotice {
                            warning: true,
                            text: format!(
                                "untrusted configuration source '{}': a confined process can write it, so the MCP servers, hooks and skills it defines are refused",
                                source.display()
                            ),
                        });
                    }
                }
            }
            HostAuthority::NotConfined => {}
        }
        notices
    }

    /// The host input of a credential store kept in files (`None` for one
    /// kept elsewhere): add it to [`Self::host_inputs`] for each store the
    /// host's factories and handlers read and write, canonically
    /// [`meerkat_providers::auth_store::TokenStoreBackend::default_auto`].
    #[must_use]
    pub fn credential_store_input(
        store: &meerkat_providers::auth_store::TokenStoreBackend,
    ) -> Option<(HostInput, PathBuf)> {
        match store {
            meerkat_providers::auth_store::TokenStoreBackend::File { root }
            | meerkat_providers::auth_store::TokenStoreBackend::Auto { root, .. } => {
                Some((HostInput::CredentialStore, root.clone()))
            }
            _ => None,
        }
    }

    /// The administration inputs of a host whose realm state lives under
    /// `state_root` and whose user config document is `user_document`.
    #[must_use]
    pub fn host_inputs(
        state_root: &Path,
        user_document: Option<&Path>,
    ) -> Vec<(HostInput, PathBuf)> {
        let mut inputs = vec![(HostInput::RealmState, state_root.to_path_buf())];
        if let Some(document) = user_document {
            inputs.push((HostInput::UserDocument, document.to_path_buf()));
        }
        inputs
    }

    /// The convention sources a host reads servers, hooks or skills from:
    /// the project and user `mcp.toml` files, the project `config.toml`
    /// whose hooks are layered in (the user `config.toml` is the user config
    /// document, an administration input), and the project skills
    /// directory.
    #[must_use]
    pub fn convention_sources(
        context_root: Option<&Path>,
        user_config_root: Option<&Path>,
    ) -> Vec<PathBuf> {
        let mut sources = Vec::new();
        if let Some(context) = context_root {
            sources.push(context.join(".rkat").join("mcp.toml"));
            sources.push(context.join(".rkat").join("config.toml"));
            sources.push(context.join(".rkat").join("skills"));
        }
        if let Some(user) = user_config_root {
            sources.push(user.join(".rkat").join("mcp.toml"));
        }
        sources
    }

    /// Log [`Self::startup_report`] in the host's log.
    pub fn log_startup(&self, inputs: &[(HostInput, PathBuf)], convention_sources: &[PathBuf]) {
        for notice in self.startup_report(inputs, convention_sources) {
            if notice.warning {
                tracing::warn!("{}", notice.text);
            } else {
                tracing::info!("{}", notice.text);
            }
        }
    }
}

fn backend_name(backend: ConfinementBackend) -> &'static str {
    match backend {
        ConfinementBackend::MacOsSeatbeltV1 => "macos_seatbelt_v1",
        ConfinementBackend::LinuxNamespaceSeccompV1 => "linux_namespace_seccomp_v1",
        _ => "the platform backend",
    }
}

impl std::fmt::Display for HostConfinement {
    /// One line naming each launch kind's mode, for a host's startup report.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, kind) in ConfinementLaunchKind::ALL.into_iter().enumerate() {
            if index > 0 {
                f.write_str(", ")?;
            }
            let mode = if self.slot(kind).is_some() {
                "required"
            } else {
                "trusted host"
            };
            write!(f, "{kind}: {mode}")?;
        }
        Ok(())
    }
}

/// The workspace preset over `root`: read and write the root and the
/// `write` subtrees, read the `read` subtrees, the platform baseline, and IP
/// sockets only when the network is unrestricted. A subtree that another
/// grant of the same or a stronger class already covers (the root, a
/// writable subtree, or a listed subtree containing it) is not granted
/// again, so the preset adds no nested grant of its own. On macOS the
/// project's Meerkat inputs (`<root>/.rkat`: the CLI's default realm state,
/// its config documents and convention files) are protected from writes;
/// the Linux backend has no deny lists, so there the host's own state must
/// live outside the workspace.
fn lower_workspace(
    kind: ConfinementLaunchKind,
    workspace: &WorkspaceConfinement,
    root: Option<&Path>,
) -> Result<(ExecutionConfinement, PathBuf), HostConfinementError> {
    let root = root.ok_or(HostConfinementError::WorkspaceRootRequired { kind })?;
    if !root.is_absolute() {
        return Err(HostConfinementError::RelativeWorkspaceRoot {
            kind,
            root: root.to_path_buf(),
        });
    }
    let writable = outermost_subtrees(
        std::iter::once(root.to_path_buf()).chain(workspace.write.iter().cloned()),
    );
    let mut readable = writable.clone();
    readable.extend(outermost_subtrees(
        workspace
            .read
            .iter()
            .filter(|path| !writable.iter().any(|granted| path.starts_with(granted)))
            .cloned(),
    ));
    let subtrees = |paths: Vec<PathBuf>| paths.into_iter().map(PathAccess::Subtree).collect();
    let requirement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(subtrees(readable)),
        write: FilesystemAccess::Paths(subtrees(writable)),
        deny_read: Vec::new(),
        deny_write: workspace_protections(root),
        network: match workspace.network {
            WorkspaceNetwork::Denied => IpNetworkAccess::Denied,
            WorkspaceNetwork::Unrestricted => IpNetworkAccess::Unrestricted,
        },
        unix_connect: Vec::new(),
        require_descendant_termination: false,
    }
    .try_into()
    .map_err(|refusal| HostConfinementError::InvalidRequirement { kind, refusal })?;
    Ok((requirement, root.to_path_buf()))
}

/// The paths the workspace preset keeps confined processes from writing.
#[cfg(target_os = "macos")]
fn workspace_protections(root: &Path) -> Vec<PathAccess> {
    vec![PathAccess::Subtree(root.join(".rkat"))]
}

/// The paths the workspace preset keeps confined processes from writing.
#[cfg(not(target_os = "macos"))]
fn workspace_protections(_root: &Path) -> Vec<PathAccess> {
    Vec::new()
}

/// `paths` once each, without a path that another listed path contains, in
/// the order each kept path was first listed.
fn outermost_subtrees(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut kept: Vec<PathBuf> = Vec::new();
    for path in paths {
        if kept.iter().any(|outer| path.starts_with(outer)) {
            continue;
        }
        kept.retain(|inner| !inner.starts_with(&path));
        kept.push(path);
    }
    kept
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::CustomConfinement;

    fn workspace_spec(read: &[&str], write: &[&str], network: IpNetworkAccess) -> ConfinementSpec {
        let subtrees = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| PathAccess::Subtree(PathBuf::from(path)))
                .collect()
        };
        ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(subtrees(read)),
            write: FilesystemAccess::Paths(subtrees(write)),
            deny_read: vec![],
            deny_write: workspace_protections(Path::new("/work")),
            network,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
    }

    #[test]
    fn no_selection_is_the_trusted_host() {
        let resolved =
            HostConfinement::resolve(&ConfinementConfig::default(), Some(Path::new("/work")))
                .unwrap();
        assert!(resolved.is_trusted_host());
        assert_eq!(resolved, HostConfinement::trusted_host());
        assert!(resolved.preflight().is_empty());
        let explicit = ConfinementConfig::for_all(LaunchConfinement::TrustedHost);
        assert!(
            HostConfinement::resolve(&explicit, None)
                .unwrap()
                .is_trusted_host()
        );
    }

    #[test]
    fn workspace_lowers_to_the_root_and_extra_grants_for_every_kind() {
        let selection =
            ConfinementConfig::for_all(LaunchConfinement::Workspace(WorkspaceConfinement {
                network: WorkspaceNetwork::Unrestricted,
                read: vec!["/opt/tool".into(), "/work/docs".into(), "/opt/tool".into()],
                write: vec!["/tmp/build".into(), "/work/target".into()],
            }));
        let resolved = HostConfinement::resolve(&selection, Some(Path::new("/work"))).unwrap();
        let expected: ExecutionConfinement = workspace_spec(
            &["/work", "/tmp/build", "/opt/tool"],
            &["/work", "/tmp/build"],
            IpNetworkAccess::Unrestricted,
        )
        .try_into()
        .unwrap();
        for kind in ConfinementLaunchKind::ALL {
            assert_eq!(resolved.requirement(kind), Some(&expected), "{kind}");
            assert_eq!(
                resolved.working_directory(kind),
                Some(Path::new("/work")),
                "{kind}"
            );
        }
    }

    #[test]
    fn workspace_grants_each_subtree_once_without_nesting() {
        let selection =
            ConfinementConfig::for_all(LaunchConfinement::Workspace(WorkspaceConfinement {
                network: WorkspaceNetwork::Denied,
                read: vec![
                    "/tmp/build/cache".into(),
                    "/opt/tool/bin".into(),
                    "/opt/tool".into(),
                    "/srv/data".into(),
                ],
                write: vec![
                    "/tmp/build/out".into(),
                    "/tmp/build".into(),
                    "/work/target".into(),
                ],
            }));
        let resolved = HostConfinement::resolve(&selection, Some(Path::new("/work"))).unwrap();
        let expected: ExecutionConfinement = workspace_spec(
            &["/work", "/tmp/build", "/opt/tool", "/srv/data"],
            &["/work", "/tmp/build"],
            IpNetworkAccess::Denied,
        )
        .try_into()
        .unwrap();
        assert_eq!(
            resolved.requirement(ConfinementLaunchKind::Shell),
            Some(&expected)
        );
    }

    #[test]
    fn workspace_defaults_to_the_root_alone_with_no_network() {
        let selection = ConfinementConfig::for_all(LaunchConfinement::Workspace(
            WorkspaceConfinement::default(),
        ));
        let resolved = HostConfinement::resolve(&selection, Some(Path::new("/work"))).unwrap();
        let expected: ExecutionConfinement =
            workspace_spec(&["/work"], &["/work"], IpNetworkAccess::Denied)
                .try_into()
                .unwrap();
        assert_eq!(
            resolved.requirement(ConfinementLaunchKind::Shell),
            Some(&expected)
        );
    }

    #[test]
    fn workspace_without_an_absolute_root_is_refused_not_unconfined() {
        let selection = ConfinementConfig::for_all(LaunchConfinement::Workspace(
            WorkspaceConfinement::default(),
        ));
        assert_eq!(
            HostConfinement::resolve(&selection, None),
            Err(HostConfinementError::WorkspaceRootRequired {
                kind: ConfinementLaunchKind::Shell
            })
        );
        assert!(matches!(
            HostConfinement::resolve(&selection, Some(Path::new("relative"))),
            Err(HostConfinementError::RelativeWorkspaceRoot { .. })
        ));
    }

    #[test]
    fn custom_and_per_kind_selections_pass_through_exactly() {
        let requirement: ExecutionConfinement =
            workspace_spec(&["/srv/mcp"], &[], IpNetworkAccess::Denied)
                .try_into()
                .unwrap();
        let selection = ConfinementConfig {
            mcp_stdio: Some(LaunchConfinement::Custom(CustomConfinement {
                working_directory: "/srv/mcp".into(),
                requirement: requirement.clone(),
            })),
            command_hooks: Some(LaunchConfinement::TrustedHost),
            ..ConfinementConfig::default()
        };
        // A custom selection needs no workspace root.
        let resolved = HostConfinement::resolve(&selection, None).unwrap();
        assert_eq!(
            resolved.requirement(ConfinementLaunchKind::McpStdio),
            Some(&requirement)
        );
        assert_eq!(
            resolved.working_directory(ConfinementLaunchKind::McpStdio),
            Some(Path::new("/srv/mcp"))
        );
        assert_eq!(resolved.requirement(ConfinementLaunchKind::Shell), None);
        assert_eq!(
            resolved.requirement(ConfinementLaunchKind::CommandHooks),
            None
        );
        assert_eq!(
            resolved.to_string(),
            "shell: trusted host, command_hooks: trusted host, mcp_stdio: required"
        );
    }

    #[test]
    fn preflight_reports_a_requirement_no_backend_enforces() {
        // Descendant termination is unsupported on every platform backend.
        let mut spec = workspace_spec(&["/work"], &["/work"], IpNetworkAccess::Denied);
        spec.require_descendant_termination = true;
        let selection = ConfinementConfig::for_all(LaunchConfinement::Custom(CustomConfinement {
            working_directory: "/work".into(),
            requirement: spec.try_into().unwrap(),
        }));
        let resolved = HostConfinement::resolve(&selection, None).unwrap();
        assert_eq!(
            resolved.preflight(),
            ConfinementLaunchKind::ALL
                .into_iter()
                .map(|kind| (kind, ConfinementRefusal::UnsupportedRequirement))
                .collect::<Vec<_>>()
        );
    }

    /// A custom selection working in `root` whose requirement reads and
    /// writes the `write` subtrees and protects the `protect` subtrees.
    fn custom(root: &Path, write: &[&Path], protect: &[&Path]) -> LaunchConfinement {
        let subtrees = |paths: &[&Path]| {
            paths
                .iter()
                .map(|path| PathAccess::Subtree(path.to_path_buf()))
                .collect::<Vec<_>>()
        };
        LaunchConfinement::Custom(CustomConfinement {
            working_directory: root.to_path_buf(),
            requirement: ConfinementSpec {
                baseline: PlatformBaseline::CommandRuntimeV1,
                read: FilesystemAccess::Paths(subtrees(write)),
                write: FilesystemAccess::Paths(subtrees(write)),
                deny_read: vec![],
                deny_write: subtrees(protect),
                network: IpNetworkAccess::Denied,
                unix_connect: vec![],
                require_descendant_termination: false,
            }
            .try_into()
            .unwrap(),
        })
    }

    /// A temporary tree: `work` (a workspace with `.rkat/realms`), and the
    /// empty `state`, `home` and `scratch` directories.
    fn tree() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        for directory in ["work/.rkat/realms", "state", "home", "scratch"] {
            std::fs::create_dir_all(base.join(directory)).unwrap();
        }
        (temp, base)
    }

    fn confined(selection: &ConfinementConfig) -> HostConfinement {
        HostConfinement::resolve(selection, None).unwrap()
    }

    /// Where nothing protects it, a realm state root inside a write grant
    /// refuses the start, naming the input and its remedy (the Linux
    /// workspace preset: there the host's state lives outside the
    /// workspace). State outside every grant is admitted.
    #[test]
    fn a_writable_realm_state_refuses_the_start() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let host = confined(&ConfinementConfig::for_all(custom(&work, &[&work], &[])));
        assert_eq!(host.authority(), HostAuthority::Guarded);
        let state = work.join(".rkat").join("realms");
        let refused = host
            .admit_host_inputs(&HostConfinement::host_inputs(&state, None))
            .unwrap_err();
        assert_eq!(
            refused,
            HostConfinementError::AdministrationInputWritable {
                input: HostInput::RealmState,
                path: state,
                kind: ConfinementLaunchKind::Shell,
            }
        );
        assert!(refused.to_string().contains("--state-root"), "{refused}");
        assert!(
            host.admit_host_inputs(&HostConfinement::host_inputs(&base.join("state"), None))
                .is_ok()
        );
    }

    /// A user config document that does not exist is judged by whether a
    /// confined process could create it: outside every grant it is
    /// admitted, under one the start is refused naming the document.
    #[test]
    fn a_missing_user_document_is_refused_only_where_it_could_be_created() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let home = base.join("home");
        let state = base.join("state");
        let document = home.join(".rkat").join("config.toml");
        let host = confined(&ConfinementConfig::for_all(custom(&work, &[&work], &[])));
        assert!(
            host.admit_host_inputs(&HostConfinement::host_inputs(&state, Some(&document)))
                .is_ok()
        );

        // A kind that writes only the home directory cannot move it away (it
        // writes nowhere else), but it can create the missing `.rkat` there.
        let home_writer = confined(&ConfinementConfig::for_all(custom(&home, &[&home], &[])));
        let refused = home_writer
            .admit_host_inputs(&HostConfinement::host_inputs(&state, Some(&document)))
            .unwrap_err();
        assert_eq!(
            refused,
            HostConfinementError::AdministrationInputWritable {
                input: HostInput::UserDocument,
                path: document,
                kind: ConfinementLaunchKind::Shell,
            }
        );
        let message = refused.to_string();
        assert!(
            message.contains("--user-config-root") && !message.contains("--state-root"),
            "{message}"
        );
    }

    /// A protected subtree is kept from a kind that writes around it, unless
    /// that kind could move the directory holding it away: a write grant
    /// elsewhere gives it somewhere to move it to.
    #[test]
    fn a_protected_subtree_holds_unless_its_parent_can_be_moved_away() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let rkat = work.join(".rkat");
        let state = rkat.join("realms");
        let protected = confined(&ConfinementConfig::for_all(custom(
            &work,
            &[&work],
            &[&rkat],
        )));
        assert!(
            protected
                .admit_host_inputs(&HostConfinement::host_inputs(&state, None))
                .is_ok()
        );
        assert!(!protected.untrusted_source(&rkat.join("mcp.toml")));

        let scratch = base.join("scratch");
        let movable = confined(&ConfinementConfig::for_all(custom(
            &work,
            &[&work, &scratch],
            &[&rkat],
        )));
        assert_eq!(
            movable.writer_of(&state, true),
            Some(ConfinementLaunchKind::Shell)
        );
        assert!(movable.untrusted_source(&rkat.join("mcp.toml")));
    }

    /// Writability is per kind: one kind's protection does not cover another
    /// kind that writes the same place without it.
    #[test]
    fn one_kind_without_the_protection_is_enough_to_refuse() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let rkat = work.join(".rkat");
        let selection = ConfinementConfig {
            all: Some(custom(&work, &[&work], &[&rkat])),
            mcp_stdio: Some(custom(&work, &[&work], &[])),
            ..ConfinementConfig::default()
        };
        let state = rkat.join("realms");
        assert_eq!(
            confined(&selection).admit_host_inputs(&HostConfinement::host_inputs(&state, None)),
            Err(HostConfinementError::AdministrationInputWritable {
                input: HostInput::RealmState,
                path: state,
                kind: ConfinementLaunchKind::McpStdio,
            })
        );
    }

    /// The route the host's open follows is judged where each entry lives: a
    /// state root reached through a link into a write grant is writable, and
    /// so is any route through a link a confined process could replace.
    #[cfg(unix)]
    #[test]
    fn a_route_through_links_is_judged_where_it_lives() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let host = confined(&ConfinementConfig::for_all(custom(&work, &[&work], &[])));
        std::os::unix::fs::symlink(work.join(".rkat"), base.join("into-work")).unwrap();
        assert!(
            host.writer_of(&base.join("into-work").join("realms"), true)
                .is_some()
        );
        std::os::unix::fs::symlink(base.join("state"), work.join("to-state")).unwrap();
        assert!(host.writer_of(&work.join("to-state"), true).is_some());
        assert_eq!(host.writer_of(&base.join("state"), true), None);
    }

    /// A host that runs any kind as the trusted host claims no guarantee: it
    /// admits every input, trusts every source, and reports that it claims
    /// none. A host that confines nothing reports nothing.
    #[test]
    fn partial_confinement_withdraws_the_guarantee() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let host = confined(&ConfinementConfig {
            mcp_stdio: Some(custom(&work, &[&work], &[])),
            ..ConfinementConfig::default()
        });
        assert_eq!(
            host.authority(),
            HostAuthority::Withdrawn {
                trusted_host: vec![
                    ConfinementLaunchKind::Shell,
                    ConfinementLaunchKind::CommandHooks
                ]
            }
        );
        let inputs = HostConfinement::host_inputs(&work.join(".rkat").join("realms"), None);
        assert!(host.admit_host_inputs(&inputs).is_ok());
        assert!(!host.untrusted_source(&work.join(".rkat").join("mcp.toml")));
        let report = host.startup_report(
            &inputs,
            &HostConfinement::convention_sources(Some(&work), None),
        );
        assert!(
            report.iter().any(|notice| notice.warning
                && notice.text.contains("no guarantee")
                && notice.text.contains("shell, command_hooks")),
            "{report:?}"
        );
        assert!(
            HostConfinement::trusted_host()
                .startup_report(&inputs, &[])
                .is_empty()
        );
    }

    /// A host that refuses executable memory files (`vm.memfd_noexec` at 2)
    /// cannot run the Linux backend's init, so it confines no launch kind:
    /// the startup report names that setting beside the refusal, so an
    /// operator can act on it. The resolving thread's seccomp filter refuses
    /// `memfd_create` with `MFD_EXEC` as that setting does (EACCES). A host
    /// that does not refuse them is never told to change the setting.
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    #[test]
    fn a_host_refusing_executable_memory_files_names_vm_memfd_noexec() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let selection = ConfinementConfig::for_all(custom(&work, &[&work], &[]));
        // A host that refuses them itself is rightly told to change it.
        let host_refuses = std::fs::read_to_string("/proc/sys/vm/memfd_noexec")
            .is_ok_and(|value| value.trim() == "2");
        if !host_refuses {
            let unrestricted = confined(&selection).startup_report(&[], &[]);
            assert!(
                !unrestricted
                    .iter()
                    .any(|notice| notice.text.contains("vm.memfd_noexec")),
                "{unrestricted:?}"
            );
        }
        let report = std::thread::spawn(move || {
            refuse_executable_memory_files_on_this_thread();
            confined(&selection).startup_report(&[], &[])
        })
        .join()
        .unwrap();
        assert!(
            report.iter().any(|notice| notice.warning
                && notice.text.contains("cannot be enforced")
                && notice.text.contains("vm.memfd_noexec")),
            "{report:?}"
        );
    }

    /// Installs, on the calling thread only, a seccomp filter that refuses
    /// `memfd_create` with `MFD_EXEC` with EACCES, as `vm.memfd_noexec = 2`
    /// does. The filter reads the low word of the call's second argument.
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    fn refuse_executable_memory_files_on_this_thread() {
        use nix::libc;
        const MFD_EXEC: u32 = 0x10;
        #[cfg(target_arch = "x86_64")]
        const ARCH: u32 = 0xc000_003e;
        #[cfg(target_arch = "aarch64")]
        const ARCH: u32 = 0xc000_00b7;
        let statement = |code, k| libc::sock_filter {
            code,
            jt: 0,
            jf: 0,
            k,
        };
        let jump = |code, k, jt, jf| libc::sock_filter { code, jt, jf, k };
        let load = (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16;
        let equal = (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16;
        let bits = (libc::BPF_JMP | libc::BPF_JSET | libc::BPF_K) as u16;
        let ret = (libc::BPF_RET | libc::BPF_K) as u16;
        let mut filter = [
            statement(load, 4),
            jump(equal, ARCH, 1, 0),
            statement(ret, libc::SECCOMP_RET_ERRNO | libc::EPERM as u32),
            statement(load, 0),
            jump(equal, libc::SYS_memfd_create as u32, 0, 3),
            statement(load, 24),
            jump(bits, MFD_EXEC, 0, 1),
            statement(ret, libc::SECCOMP_RET_ERRNO | libc::EACCES as u32),
            statement(ret, libc::SECCOMP_RET_ALLOW),
        ];
        let program = libc::sock_fprog {
            len: filter.len().try_into().unwrap(),
            filter: filter.as_mut_ptr(),
        };
        // SAFETY: a plain prctl call with integer arguments.
        #[allow(unsafe_code)]
        let no_new_privileges = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        assert_eq!(no_new_privileges, 0);
        // SAFETY: `program` points at `filter`, both alive for the call, which
        // copies the filter into the kernel.
        #[allow(unsafe_code)]
        let installed = unsafe {
            libc::prctl(
                libc::PR_SET_SECCOMP,
                libc::SECCOMP_MODE_FILTER,
                std::ptr::addr_of!(program),
            )
        };
        assert_eq!(installed, 0);
    }

    /// The file credential store is an administration input: a start whose
    /// confined processes could change it is refused naming it, and outside
    /// every grant it is admitted and reported among the inputs kept from
    /// them. A store kept elsewhere adds no input.
    #[test]
    fn a_writable_credential_store_refuses_the_start() {
        use meerkat_providers::auth_store::TokenStoreBackend;
        let (_temp, base) = tree();
        let work = base.join("work");
        let host = confined(&ConfinementConfig::for_all(custom(&work, &[&work], &[])));
        let writable = TokenStoreBackend::File {
            root: work.join("credentials"),
        };
        let input =
            HostConfinement::credential_store_input(&writable).expect("a file store is an input");
        assert_eq!(
            input,
            (HostInput::CredentialStore, work.join("credentials"))
        );
        let mut inputs = HostConfinement::host_inputs(&base.join("state"), None);
        inputs.push(input);
        let refused = host.admit_host_inputs(&inputs).unwrap_err();
        assert!(
            matches!(
                refused,
                HostConfinementError::AdministrationInputWritable {
                    input: HostInput::CredentialStore,
                    ..
                }
            ),
            "{refused}"
        );
        assert!(
            refused.to_string().contains("the credential store"),
            "{refused}"
        );

        let outside = TokenStoreBackend::File {
            root: base.join("credentials"),
        };
        let mut inputs = HostConfinement::host_inputs(&base.join("state"), None);
        inputs.extend(HostConfinement::credential_store_input(&outside));
        host.admit_host_inputs(&inputs).unwrap();
        let report = host.startup_report(&inputs, &[]);
        assert!(
            report
                .iter()
                .any(|notice| !notice.warning && notice.text.contains("the credential store at")),
            "{report:?}"
        );
        assert_eq!(
            HostConfinement::credential_store_input(&TokenStoreBackend::Ephemeral),
            None
        );
    }

    /// A skills directory is a convention source judged as a whole: a host
    /// whose confined processes can write anything under the project skills
    /// directory does not trust it and names it in its report; one they
    /// cannot reach is trusted.
    #[test]
    fn a_writable_skills_directory_is_not_trusted() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let skills = work.join(".rkat").join("skills");
        std::fs::create_dir_all(skills.join("planted")).unwrap();
        let nested = confined(&ConfinementConfig::for_all(custom(
            &work,
            &[&skills.join("planted")],
            &[],
        )));
        assert!(
            nested.untrusted_directory(&skills),
            "a writable directory under it is enough"
        );
        assert!(!nested.untrusted_source(&skills));
        let sources = HostConfinement::convention_sources(Some(&work), None);
        assert!(sources.contains(&skills), "{sources:?}");
        let report = nested.startup_report(
            &HostConfinement::host_inputs(&base.join("state"), None),
            &sources,
        );
        assert!(
            report.iter().any(|notice| notice.warning
                && notice.text.contains(&skills.display().to_string())
                && notice.text.contains("skills")),
            "{report:?}"
        );
        let elsewhere = confined(&ConfinementConfig::for_all(custom(
            &base.join("scratch"),
            &[&base.join("scratch")],
            &[],
        )));
        assert!(!elsewhere.untrusted_directory(&skills));
    }

    /// The startup report names each kind's requested and effective
    /// confinement, the inputs kept from confined processes, and each
    /// convention source it does not trust.
    #[test]
    fn the_report_names_kinds_inputs_and_untrusted_sources() {
        let (_temp, base) = tree();
        let work = base.join("work");
        let home = base.join("home");
        let host = confined(&ConfinementConfig::for_all(custom(&work, &[&work], &[])));
        let inputs = HostConfinement::host_inputs(&base.join("state"), None);
        let sources = HostConfinement::convention_sources(Some(&work), Some(&home));
        let report = host.startup_report(&inputs, &sources);
        let texts: Vec<&str> = report.iter().map(|notice| notice.text.as_str()).collect();
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("shell confinement: requested custom")),
            "{texts:?}"
        );
        assert!(
            texts.iter().any(
                |text| text.starts_with("kept from every confined process: the realm state at")
            ),
            "{texts:?}"
        );
        for source in [
            work.join(".rkat").join("mcp.toml"),
            work.join(".rkat").join("config.toml"),
        ] {
            assert!(host.untrusted_source(&source), "{}", source.display());
            assert!(
                report
                    .iter()
                    .any(|notice| notice.warning
                        && notice.text.contains(&source.display().to_string())),
                "{texts:?}"
            );
        }
        assert!(!host.untrusted_source(&home.join(".rkat").join("mcp.toml")));
    }
}
