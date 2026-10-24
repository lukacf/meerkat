//! Mechanical requirements for an already-authorized process launch.
//!
//! These values do not authenticate a caller, grant permission, or supervise a
//! process. The existing operation owner supplies them to an OS adapter. A
//! backend must enforce the complete requirement or refuse that operation.
//! Confinement does not establish semantic confidentiality of model output.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A literal file or directory, or a directory and its descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "path",
    rename_all = "snake_case",
    from = "PathAccessWire"
)]
pub enum PathAccess {
    Literal(PathBuf),
    Subtree(PathBuf),
}

impl PathAccess {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Literal(path) | Self::Subtree(path) => path,
        }
    }
}

/// An empty path list grants no access of this class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "paths",
    rename_all = "snake_case",
    from = "FilesystemAccessWire"
)]
pub enum FilesystemAccess {
    Unrestricted,
    Paths(Vec<PathAccess>),
}

/// IP access is independent of Unix sockets and platform service IPC.
/// Endpoints are resolved by the trusted host, never from child proxy settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    tag = "kind",
    content = "endpoints",
    rename_all = "snake_case",
    from = "IpNetworkAccessWire"
)]
pub enum IpNetworkAccess {
    Denied,
    Unrestricted,
    Connect(Vec<SocketAddr>),
}

// Decode through closed struct variants while preserving the public adjacent-tag
// serialization. Empty struct variants reject even reserved content set to null;
// adjacent-tag unit variants can otherwise discard that unsupported content.
#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PathAccessWire {
    Literal { path: PathBuf },
    Subtree { path: PathBuf },
}

impl From<PathAccessWire> for PathAccess {
    fn from(value: PathAccessWire) -> Self {
        match value {
            PathAccessWire::Literal { path } => Self::Literal(path),
            PathAccessWire::Subtree { path } => Self::Subtree(path),
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FilesystemAccessWire {
    Unrestricted {},
    Paths { paths: Vec<PathAccess> },
}

impl From<FilesystemAccessWire> for FilesystemAccess {
    fn from(value: FilesystemAccessWire) -> Self {
        match value {
            FilesystemAccessWire::Unrestricted {} => Self::Unrestricted,
            FilesystemAccessWire::Paths { paths } => Self::Paths(paths),
        }
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum IpNetworkAccessWire {
    Denied {},
    Unrestricted {},
    Connect { endpoints: Vec<SocketAddr> },
}

impl From<IpNetworkAccessWire> for IpNetworkAccess {
    fn from(value: IpNetworkAccessWire) -> Self {
        match value {
            IpNetworkAccessWire::Denied {} => Self::Denied,
            IpNetworkAccessWire::Unrestricted {} => Self::Unrestricted,
            IpNetworkAccessWire::Connect { endpoints } => Self::Connect(endpoints),
        }
    }
}

/// Explicit compatibility privileges, enumerated by each backend's policy.
/// There is no ambient home-directory, scratch, network, or credential grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PlatformBaseline {
    /// Executable loaders, system libraries, CPU/OS queries and null/random
    /// devices. User data and optional service IPC require explicit grants.
    CommandRuntimeV1,
}

/// Host configuration before structural validation. Deserializing this value
/// supplies requirements only; it never supplies caller authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConfinementSpec {
    pub baseline: PlatformBaseline,
    pub read: FilesystemAccess,
    pub write: FilesystemAccess,
    /// Exclusions take precedence over all grants, including the baseline.
    pub deny_read: Vec<PathAccess>,
    pub deny_write: Vec<PathAccess>,
    pub network: IpNetworkAccess,
    /// Exact local socket paths or explicitly granted socket directories.
    pub unix_connect: Vec<PathAccess>,
    /// Some backends cannot prove termination of descendants that leave their
    /// process group. Such a backend must refuse when this is required.
    pub require_descendant_termination: bool,
}

/// Validated immutable requirements. Final host-path and backend validation
/// belongs to the adapter at preparation, not this platform-neutral contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "ConfinementSpec", into = "ConfinementSpec")]
pub struct ExecutionConfinement(ConfinementSpec);

impl ExecutionConfinement {
    #[must_use]
    pub fn specification(&self) -> &ConfinementSpec {
        &self.0
    }
}

impl From<ExecutionConfinement> for ConfinementSpec {
    fn from(value: ExecutionConfinement) -> Self {
        value.0
    }
}

impl TryFrom<ConfinementSpec> for ExecutionConfinement {
    type Error = ConfinementRefusal;

    fn try_from(spec: ConfinementSpec) -> Result<Self, Self::Error> {
        fn paths(access: &FilesystemAccess) -> &[PathAccess] {
            match access {
                FilesystemAccess::Unrestricted => &[],
                FilesystemAccess::Paths(paths) => paths.as_slice(),
            }
        }
        for access in paths(&spec.read)
            .iter()
            .chain(paths(&spec.write))
            .chain(&spec.deny_read)
            .chain(&spec.deny_write)
            .chain(&spec.unix_connect)
        {
            let path = access.path();
            if !path.is_absolute()
                || path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                || path.as_os_str().as_encoded_bytes().contains(&0)
            {
                return Err(ConfinementRefusal::InvalidRequirement);
            }
        }
        if let IpNetworkAccess::Connect(endpoints) = &spec.network
            && endpoints
                .iter()
                .any(|endpoint| endpoint.port() == 0 || endpoint.ip().is_unspecified())
        {
            return Err(ConfinementRefusal::InvalidRequirement);
        }
        Ok(Self(spec))
    }
}

/// Bounded operation-local diagnostics. Never expose an environment value,
/// credential path, gate token, or secret-bearing command line in this error.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ConfinementRefusal {
    #[error("invalid execution confinement requirement")]
    InvalidRequirement,
    #[error("invalid confined process launch")]
    InvalidLaunch,
    #[error("required execution confinement is unsupported by this backend")]
    UnsupportedRequirement,
    #[error("required execution confinement backend is unavailable")]
    BackendUnavailable,
    #[error("confined process preparation failed")]
    PreparationFailed,
    // The launch is defined by a configuration source that a confined
    // process can write, so the host does not run it: it is refused before
    // its definition is expanded, authorized, connected or spawned. (A plain
    // comment: a documented variant would split the schema's string enum.)
    #[error("launch configuration comes from a source a confined process can write")]
    UntrustedConfigurationSource,
}

/// A process a stock host launches as itself, outside the OS process
/// confinement it applies to the processes its agents launch (the
/// `[confinement]` launch kinds). Each runs a program the host or its
/// operator chose, for a reason no confinement serves, through its own
/// fixed-program constructor. Under a host that confines its launches, each
/// runs (and every program it starts by name: an interpreter named through
/// `env`, `ssh` under Git, the programs a browser opener tries) from a
/// `PATH` without relative directories or directories a confined process
/// could change ([`trusted_search_path`]), and with no variable that loads
/// code from elsewhere or names a path such a process could change
/// ([`trusted_environment`]); the host also refuses a Git skill source or
/// credential command whose cache, program, interpreter or working
/// directory that process could change.
///
/// Every other process a stock host starts runs under its launch kind's
/// confinement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HostLaunchExemption {
    /// `git`, fetching a configured Git skill repository into the host's
    /// skill cache and checking it out.
    GitSkillSource,
    /// A configured credential command (an auth profile's
    /// `source = { kind = "command" }`) producing a provider credential.
    CredentialHelper,
    /// The platform browser opener (`/usr/bin/open`, `xdg-open` or
    /// `rundll32`) showing a URL, an OAuth authorization page or an HTML
    /// artifact, in the user's desktop session.
    BrowserOpener,
    /// `stty` (`/bin/stty`, else `/usr/bin/stty`, else the one the trusted
    /// `PATH` finds), which the CLI runs on its own terminal to hide a
    /// secret typed at it.
    TerminalControl,
    /// `wasm-pack`, whose version `rkat doctor` reports.
    DeveloperBuildTooling,
}

impl HostLaunchExemption {
    /// Every exemption.
    pub const ALL: [Self; 5] = [
        Self::GitSkillSource,
        Self::CredentialHelper,
        Self::BrowserOpener,
        Self::TerminalControl,
        Self::DeveloperBuildTooling,
    ];

    /// The exemption's stable name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GitSkillSource => "git_skill_source",
            Self::CredentialHelper => "credential_helper",
            Self::BrowserOpener => "browser_opener",
            Self::TerminalControl => "terminal_control",
            Self::DeveloperBuildTooling => "developer_build_tooling",
        }
    }

    /// Why the host runs it as itself.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::GitSkillSource => {
                "it fetches with the host's own Git credentials and network; a skill cache a \
                 confined process could change refuses the repository"
            }
            Self::CredentialHelper => {
                "it produces the host's own provider credentials, which no confined process may \
                 read; it is refused when a confined process could change its program, the \
                 interpreter its #! line names, or its working directory"
            }
            Self::BrowserOpener => {
                "it shows a URL in the user's desktop session, which a confined process cannot \
                 reach; the platform opener gets the URL as one argument"
            }
            Self::TerminalControl => {
                "host terminal control: stty (by absolute path where the system keeps it there, \
                 else from the trusted PATH) sets the mode of the terminal the operator runs the \
                 CLI on"
            }
            Self::DeveloperBuildTooling => {
                "developer build tooling: rkat doctor reports the version of wasm-pack"
            }
        }
    }

    /// `git`, for a Git skill source.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn git(untrusted: Option<&UntrustedPathFn>) -> std::process::Command {
        Self::GitSkillSource.launch("git", untrusted)
    }

    /// A configured credential command's `program`.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn credential_command(
        program: &Path,
        untrusted: Option<&UntrustedPathFn>,
    ) -> std::process::Command {
        Self::CredentialHelper.launch(program, untrusted)
    }

    /// The platform browser opener showing `url`, as one argument:
    /// `/usr/bin/open` on macOS, the system `rundll32.exe
    /// url.dll,FileProtocolHandler` on Windows, `xdg-open` elsewhere.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn browser_opener(url: &str, untrusted: Option<&UntrustedPathFn>) -> std::process::Command {
        Self::browser_opener_in(url, std::env::vars_os(), untrusted)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn browser_opener_in(
        url: &str,
        environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
        untrusted: Option<&UntrustedPathFn>,
    ) -> std::process::Command {
        #[cfg(target_os = "macos")]
        let mut command = Self::BrowserOpener.launch_in("/usr/bin/open", environment, untrusted);
        #[cfg(target_os = "windows")]
        let mut command = {
            let program = std::env::var_os("SystemRoot").map_or_else(
                || PathBuf::from("rundll32"),
                |root| PathBuf::from(root).join("System32").join("rundll32.exe"),
            );
            let mut command = Self::BrowserOpener.launch_in(program, environment, untrusted);
            command.arg("url.dll,FileProtocolHandler");
            command
        };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let mut command = Self::BrowserOpener.launch_in("xdg-open", environment, untrusted);
        command.arg(url);
        command
    }

    /// `stty`: `/bin/stty`, else `/usr/bin/stty`, else (where neither
    /// exists, as on NixOS) the `stty` the launch's `PATH` finds, which under
    /// a host that confines its launches holds no relative directory and no
    /// directory a confined process could change. It starts no program of
    /// its own.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn terminal_control(untrusted: Option<&UntrustedPathFn>) -> std::process::Command {
        Self::terminal_control_in(
            &[Path::new("/bin/stty"), Path::new("/usr/bin/stty")],
            std::env::vars_os(),
            untrusted,
        )
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn terminal_control_in(
        installed: &[&Path],
        environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
        untrusted: Option<&UntrustedPathFn>,
    ) -> std::process::Command {
        let program = installed
            .iter()
            .find(|program| program.is_file())
            .map_or(std::ffi::OsStr::new("stty"), |program| program.as_os_str());
        Self::TerminalControl.launch_in(program, environment, untrusted)
    }

    /// `wasm-pack`.
    #[cfg(not(target_arch = "wasm32"))]
    #[must_use]
    pub fn wasm_pack(untrusted: Option<&UntrustedPathFn>) -> std::process::Command {
        Self::DeveloperBuildTooling.launch("wasm-pack", untrusted)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn launch(
        self,
        program: impl AsRef<std::ffi::OsStr>,
        untrusted: Option<&UntrustedPathFn>,
    ) -> std::process::Command {
        self.launch_in(program, std::env::vars_os(), untrusted)
    }

    /// The command of this exemption for `program`, found and running with
    /// `environment` (the host's). Under a host that confines its agents'
    /// launches (`untrusted` given), only the variables
    /// [`trusted_environment`] keeps: no variable that loads code from
    /// elsewhere, and a `PATH` without relative directories or the
    /// directories `untrusted` names. Without one there is no confined
    /// process to plant anything, and the host's environment passes as it
    /// is.
    #[cfg(not(target_arch = "wasm32"))]
    fn launch_in(
        self,
        program: impl AsRef<std::ffi::OsStr>,
        environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
        untrusted: Option<&UntrustedPathFn>,
    ) -> std::process::Command {
        let _ = self;
        // The one command every exemption's fixed-program constructor builds.
        #[allow(clippy::disallowed_methods)]
        let mut command = std::process::Command::new(program);
        command.env_clear();
        match untrusted {
            Some(untrusted) => command.envs(trusted_environment(environment, untrusted)),
            None => command.envs(environment),
        };
        command
    }
}

/// Whether a confined process could change what the host reads at a path
/// (`true`: anything under it as well): the judgement a host launch
/// exemption takes its `PATH`, program and working directory by.
pub type UntrustedPathFn = dyn Fn(&Path, bool) -> bool + Send + Sync;

/// `search_path` (a `PATH` value) without its relative entries and without
/// each directory `untrusted` names as a whole, so that a program,
/// interpreter or helper a host launch exemption (or a process it starts)
/// finds by name is one no confined process could put there.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn trusted_search_path(
    search_path: &std::ffi::OsStr,
    untrusted: &UntrustedPathFn,
) -> std::ffi::OsString {
    let kept: Vec<PathBuf> = std::env::split_paths(search_path)
        .filter(|directory| directory.is_absolute() && !untrusted(directory, true))
        .collect();
    std::env::join_paths(kept).unwrap_or_default()
}

/// The environment a host launch exemption runs with under a host that
/// confines its agents' launches (`untrusted`: what its confined processes
/// could change), from `environment` (the host's own): every variable
/// except one that makes the launched program, or a program it starts, load
/// code or startup options from somewhere else
/// (`is_load_injection_variable`), which is dropped, with the paths a
/// confined process could change taken out of the values
/// (`trusted_variable_value`): `PATH` without its relative directories
/// and the directories `untrusted` names, any other list of absolute paths
/// likewise, and a value that is one path `untrusted` names dropped. A
/// variable left without a value is dropped, so the platform's default
/// applies: an empty `PATH` reads as the working directory on some
/// platforms.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn trusted_environment(
    environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    untrusted: &UntrustedPathFn,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    environment
        .into_iter()
        .filter(|(name, _)| !is_load_injection_variable(name))
        .filter_map(|(name, value)| {
            let value = trusted_variable_value(&name, &value, untrusted)?;
            Some((name, value))
        })
        .collect()
}

/// Give `command` the environment an operator configured for it
/// (`configured`, such as a credential command's `env`). Under a host that
/// confines its agents' launches (`untrusted` given) each value loses the
/// paths a confined process could change (`trusted_variable_value`), and
/// a configured variable left without a value is not passed on at all.
/// A configured variable is the operator's own, so no name is refused for
/// itself, and without such a host every value passes as configured.
#[cfg(not(target_arch = "wasm32"))]
pub fn apply_configured_environment<'a>(
    command: &mut std::process::Command,
    configured: impl IntoIterator<Item = (&'a String, &'a String)>,
    untrusted: Option<&UntrustedPathFn>,
) {
    for (name, value) in configured {
        let Some(untrusted) = untrusted else {
            command.env(name, value);
            continue;
        };
        let name = std::ffi::OsStr::new(name);
        match trusted_variable_value(name, std::ffi::OsStr::new(value), untrusted) {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
}

/// `value` of the variable `name` as a host launch exemption may see it
/// (`None`: not at all). `PATH`, and any other value that is a list of
/// absolute paths, keeps only the absolute directories `untrusted` does not
/// name as a whole ([`trusted_search_path`]); a value that is one absolute
/// path is dropped when `untrusted` names it; any other value is kept.
#[cfg(not(target_arch = "wasm32"))]
fn trusted_variable_value(
    name: &std::ffi::OsStr,
    value: &std::ffi::OsStr,
    untrusted: &UntrustedPathFn,
) -> Option<std::ffi::OsString> {
    let entries: Vec<PathBuf> = std::env::split_paths(value).collect();
    let search_path = name.to_string_lossy().eq_ignore_ascii_case("PATH");
    let path_list = entries.iter().any(|entry| entry.is_absolute())
        && entries
            .iter()
            .all(|entry| entry.as_os_str().is_empty() || entry.is_absolute());
    if !search_path && !path_list {
        return Some(value.to_os_string());
    }
    if let [path] = entries.as_slice()
        && !search_path
    {
        return (!untrusted(path, false)).then(|| value.to_os_string());
    }
    Some(trusted_search_path(value, untrusted)).filter(|cleaned| !cleaned.is_empty())
}

/// Whether an environment variable a host launch exemption would inherit
/// can make the launched program, or a program it starts, load code or
/// startup options from somewhere else: the dynamic loader's (`LD_*`,
/// `DYLD_*`, `_RLD_*`, `GLIBC_TUNABLES`), macOS XPC's (`__XPC_*`), a shell's
/// startup files, functions and options (`ENV`, `BASH_ENV`, `BASH_FUNC_*`,
/// `SHELLOPTS`, `BASHOPTS`, `ZDOTDIR`), interpreters' module paths and
/// options (`PYTHON*`, `NODE_OPTIONS`, `NODE_PATH`, `PERL5LIB`, `PERL5OPT`,
/// `PERLLIB`, `RUBYLIB`, `RUBYOPT`, `JAVA_TOOL_OPTIONS`, `_JAVA_OPTIONS`,
/// `JDK_JAVA_OPTIONS`), and Git's repository and program locations
/// (`GIT_DIR`, `GIT_WORK_TREE`, `GIT_EXEC_PATH`). Such a variable is never
/// inherited, whatever its value.
#[cfg(not(target_arch = "wasm32"))]
fn is_load_injection_variable(name: &std::ffi::OsStr) -> bool {
    const PREFIXES: &[&str] = &["LD_", "DYLD_", "_RLD_", "__XPC_", "BASH_FUNC_", "PYTHON"];
    const NAMES: &[&str] = &[
        "ENV",
        "BASH_ENV",
        "SHELLOPTS",
        "BASHOPTS",
        "ZDOTDIR",
        "GLIBC_TUNABLES",
        "NODE_OPTIONS",
        "NODE_PATH",
        "PERL5LIB",
        "PERL5OPT",
        "PERLLIB",
        "RUBYLIB",
        "RUBYOPT",
        "JAVA_TOOL_OPTIONS",
        "_JAVA_OPTIONS",
        "JDK_JAVA_OPTIONS",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_EXEC_PATH",
    ];
    let name = name.to_string_lossy().to_ascii_uppercase();
    PREFIXES.iter().any(|prefix| name.starts_with(prefix)) || NAMES.contains(&name.as_str())
}

impl std::fmt::Display for HostLaunchExemption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::GitSkillSource => "Git skill source",
            Self::CredentialHelper => "credential command",
            Self::BrowserOpener => "browser opener",
            Self::TerminalControl => "terminal control",
            Self::DeveloperBuildTooling => "developer build tooling",
        })
    }
}

/// The files a launch of `program` could execute, in the order the platform
/// tries them: `program` itself when it names a path (relative to
/// `working_directory`, else to the host's working directory), else
/// `program` in each directory of `search_path` (the launch's `PATH`;
/// `/usr/bin:/bin` when it has none) up to and including the first that
/// holds it. A host refuses a launch of its own ([`HostLaunchExemption`])
/// when a confined process could change any of them.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn program_routes(
    program: &Path,
    working_directory: Option<&Path>,
    search_path: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let in_working_directory = |path: &Path| {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        working_directory
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .map_or_else(|| path.to_path_buf(), |base| base.join(path))
    };
    if program.is_absolute() || program.components().count() != 1 {
        return vec![in_working_directory(program)];
    }
    let search_path = search_path.unwrap_or_else(|| std::ffi::OsStr::new("/usr/bin:/bin"));
    let mut routes = Vec::new();
    for directory in std::env::split_paths(search_path) {
        let route = in_working_directory(&directory).join(program);
        let found = route.is_file();
        routes.push(route);
        if found {
            break;
        }
    }
    routes
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn spec() -> ConfinementSpec {
        ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![]),
            write: FilesystemAccess::Paths(vec![]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
    }

    #[test]
    fn requirements_do_not_add_ambient_grants() {
        let expected = spec();
        let validated = ExecutionConfinement::try_from(expected.clone()).unwrap();
        assert_eq!(validated.specification(), &expected);
    }

    #[test]
    fn deserialization_cannot_skip_path_validation() {
        let mut invalid = spec();
        invalid.write =
            FilesystemAccess::Paths(vec![PathAccess::Subtree(PathBuf::from("relative"))]);
        let encoded = serde_json::to_string(&invalid).unwrap();
        assert!(serde_json::from_str::<ExecutionConfinement>(&encoded).is_err());
    }

    #[test]
    fn connect_requires_an_exact_destination() {
        let mut invalid = spec();
        invalid.network = IpNetworkAccess::Connect(vec!["0.0.0.0:443".parse().unwrap()]);
        assert_eq!(
            ExecutionConfinement::try_from(invalid),
            Err(ConfinementRefusal::InvalidRequirement)
        );
    }

    #[test]
    fn decoder_preserves_existing_variant_wire_shapes() {
        fn roundtrip<T>(value: T, expected: serde_json::Value)
        where
            T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
        {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected);
            assert_eq!(serde_json::from_value::<T>(expected).unwrap(), value);
        }
        roundtrip(
            PathAccess::Literal("/work".into()),
            serde_json::json!({"kind": "literal", "path": "/work"}),
        );
        roundtrip(
            PathAccess::Subtree("/work".into()),
            serde_json::json!({"kind": "subtree", "path": "/work"}),
        );
        roundtrip(
            FilesystemAccess::Unrestricted,
            serde_json::json!({"kind": "unrestricted"}),
        );
        roundtrip(
            FilesystemAccess::Paths(vec![]),
            serde_json::json!({"kind": "paths", "paths": []}),
        );
        roundtrip(
            IpNetworkAccess::Denied,
            serde_json::json!({"kind": "denied"}),
        );
        roundtrip(
            IpNetworkAccess::Unrestricted,
            serde_json::json!({"kind": "unrestricted"}),
        );
        roundtrip(
            IpNetworkAccess::Connect(vec!["127.0.0.1:443".parse().unwrap()]),
            serde_json::json!({"kind": "connect", "endpoints": ["127.0.0.1:443"]}),
        );
    }

    fn decode_override(field: &str, value: serde_json::Value) -> bool {
        let mut encoded = serde_json::to_value(spec()).unwrap();
        encoded[field] = value;
        serde_json::from_value::<ExecutionConfinement>(encoded).is_ok()
    }

    #[test]
    fn decoder_rejects_nested_path_extensions_instead_of_widening_access() {
        for kind in ["literal", "subtree"] {
            let allowed = serde_json::json!({ "kind": kind, "path": "/work" });
            assert!(decode_override(
                "read",
                serde_json::json!({ "kind": "paths", "paths": [allowed] }),
            ));
            let unsupported = serde_json::json!({
                "kind": kind,
                "path": "/work",
                "except": ["/work/secret"]
            });
            assert!(!decode_override(
                "read",
                serde_json::json!({ "kind": "paths", "paths": [unsupported] }),
            ));
        }
    }

    #[test]
    fn decoder_rejects_nested_filesystem_extensions_instead_of_ignoring_them() {
        assert!(decode_override(
            "read",
            serde_json::json!({ "kind": "paths", "paths": [] }),
        ));
        assert!(!decode_override(
            "read",
            serde_json::json!({ "kind": "paths", "paths": [], "future_policy": true }),
        ));
    }

    #[test]
    fn decoder_rejects_nested_network_extensions_instead_of_ignoring_them() {
        assert!(decode_override(
            "network",
            serde_json::json!({ "kind": "connect", "endpoints": ["127.0.0.1:443"] }),
        ));
        assert!(!decode_override(
            "network",
            serde_json::json!({
                "kind": "connect",
                "endpoints": ["127.0.0.1:443"],
                "future_policy": true
            }),
        ));
    }

    #[test]
    fn decoder_rejects_reserved_filesystem_unit_content_even_when_null() {
        assert!(decode_override(
            "read",
            serde_json::json!({ "kind": "unrestricted" }),
        ));
        assert!(!decode_override(
            "read",
            serde_json::json!({ "kind": "unrestricted", "paths": null }),
        ));
    }

    #[test]
    fn decoder_rejects_reserved_network_unit_content_even_when_null() {
        for kind in ["denied", "unrestricted"] {
            assert!(decode_override(
                "network",
                serde_json::json!({ "kind": kind })
            ));
            assert!(!decode_override(
                "network",
                serde_json::json!({ "kind": kind, "endpoints": null }),
            ));
        }
    }

    #[test]
    fn decoder_rejects_invalid_paths_in_every_path_collection() {
        for path in ["relative", "/work/../secret", "/work/\u{0}secret"] {
            for field in ["read", "write"] {
                assert!(!decode_override(
                    field,
                    serde_json::json!({
                        "kind": "paths",
                        "paths": [{ "kind": "subtree", "path": path }]
                    }),
                ));
            }
            for field in ["deny_read", "deny_write", "unix_connect"] {
                assert!(!decode_override(
                    field,
                    serde_json::json!([{ "kind": "literal", "path": path }]),
                ));
            }
        }
    }

    #[test]
    fn decoder_rejects_unspecified_and_zero_port_network_endpoints() {
        for endpoint in ["0.0.0.0:443", "[::]:443", "127.0.0.1:0", "[::1]:0"] {
            assert!(!decode_override(
                "network",
                serde_json::json!({ "kind": "connect", "endpoints": [endpoint] }),
            ));
        }
        for endpoint in ["127.0.0.1:443", "[::1]:443"] {
            assert!(decode_override(
                "network",
                serde_json::json!({ "kind": "connect", "endpoints": [endpoint] }),
            ));
        }
    }

    /// A program named by path runs from that path (a relative one from the
    /// working directory); a bare name runs from the first search path
    /// directory that holds it, and every directory before that one could
    /// shadow it.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn program_routes_follow_the_platform_lookup() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let third = temp.path().join("third");
        for directory in [&first, &second, &third] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(second.join("tool"), "").unwrap();
        std::fs::write(third.join("tool"), "").unwrap();
        let search_path = std::env::join_paths([&first, &second, &third]).unwrap();

        assert_eq!(
            program_routes(Path::new("/usr/bin/tool"), None, Some(&search_path)),
            vec![PathBuf::from("/usr/bin/tool")]
        );
        assert_eq!(
            program_routes(Path::new("bin/tool"), Some(temp.path()), Some(&search_path)),
            vec![temp.path().join("bin/tool")]
        );
        assert_eq!(
            program_routes(Path::new("tool"), Some(temp.path()), Some(&search_path)),
            vec![first.join("tool"), second.join("tool")]
        );
        assert_eq!(
            program_routes(Path::new("absent"), Some(temp.path()), Some(&search_path)),
            vec![
                first.join("absent"),
                second.join("absent"),
                third.join("absent")
            ]
        );
    }

    #[test]
    fn host_launch_exemptions_name_their_reasons() {
        for exemption in HostLaunchExemption::ALL {
            assert!(!exemption.as_str().is_empty());
            assert!(!exemption.reason().is_empty());
            assert!(!exemption.to_string().is_empty());
        }
    }

    /// A host launch exemption's `PATH` keeps only absolute directories the
    /// host trusts as a whole.
    #[cfg(unix)]
    #[test]
    fn the_trusted_search_path_drops_relative_and_untrusted_directories() {
        let search_path =
            std::env::join_paths(["/usr/bin", "bin", "/work/node_modules/.bin", "", "/bin"])
                .unwrap();
        let untrusted = |path: &Path, subtree: bool| subtree && path.starts_with("/work");
        assert_eq!(
            trusted_search_path(&search_path, &untrusted),
            std::env::join_paths(["/usr/bin", "/bin"]).unwrap()
        );
    }

    /// Why the rows that write a program and run it run alone: a program a
    /// process still holds open for writing does not run ("Text file busy"),
    /// and a process another test spawns holds a copy of every descriptor
    /// of this binary, the row's write descriptor included, until it executes
    /// its own program.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_program_open_for_writing_does_not_run() {
        crate::process_isolation_tests::alone_in_child_process(
            "confinement::tests::a_program_open_for_writing_does_not_run",
            || {
                use std::os::unix::fs::PermissionsExt;
                let temp = tempfile::tempdir().unwrap();
                let program = temp.path().join("busy");
                std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
                std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
                let writer = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&program)
                    .unwrap();
                #[allow(clippy::disallowed_methods)] // the row runs a program it wrote
                let busy = std::process::Command::new(&program).status();
                assert_eq!(
                    busy.map_err(|error| error.kind()).err(),
                    Some(std::io::ErrorKind::ExecutableFileBusy),
                    "a program open for writing does not run"
                );
                drop(writer);
                #[allow(clippy::disallowed_methods)] // the row runs a program it wrote
                let status = std::process::Command::new(&program).status().unwrap();
                assert!(status.success(), "it runs once the writer is gone");
            },
        );
    }

    /// A program planted in a directory a confined process can write, ahead
    /// of the trusted one on the host's `PATH`, never runs: the exemption
    /// (here the browser opener) finds the trusted program, and a program
    /// only the untrusted directory holds is not found at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_program_planted_on_the_path_is_not_run() {
        crate::process_isolation_tests::alone_in_child_process(
            "confinement::tests::a_program_planted_on_the_path_is_not_run",
            || {
                use std::os::unix::fs::PermissionsExt;
                let temp = tempfile::tempdir().unwrap();
                let base = temp.path().canonicalize().unwrap();
                let writable = base.join("writable");
                let trusted = base.join("trusted");
                let mut markers = Vec::new();
                for (dir, name) in [
                    (&writable, "xdg-open"),
                    (&trusted, "xdg-open"),
                    (&writable, "only-planted"),
                ] {
                    std::fs::create_dir_all(dir).unwrap();
                    let marker = base.join(format!(
                        "{}-{name}",
                        dir.file_name().unwrap().to_string_lossy()
                    ));
                    let program = dir.join(name);
                    std::fs::write(
                        &program,
                        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
                    )
                    .unwrap();
                    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                    markers.push(marker);
                }
                let search_path = std::env::join_paths([&writable, &trusted]).unwrap();
                let untrusted = move |path: &Path, _: bool| path.starts_with(&writable);

                let environment = || vec![(std::ffi::OsString::from("PATH"), search_path.clone())];
                let status = HostLaunchExemption::browser_opener_in(
                    "https://example.invalid/authorize",
                    environment(),
                    Some(&untrusted),
                )
                .status()
                .unwrap();
                assert!(status.success());
                assert!(!markers[0].exists(), "the planted opener never ran");
                assert!(markers[1].exists(), "the trusted opener ran");

                assert!(
                    HostLaunchExemption::GitSkillSource
                        .launch_in("only-planted", environment(), Some(&untrusted))
                        .status()
                        .is_err(),
                    "a program only an untrusted directory holds is not found"
                );
                assert!(!markers[2].exists());
            },
        );
    }

    /// Where neither `/bin/stty` nor `/usr/bin/stty` exists (NixOS keeps
    /// `stty` only on `PATH`), terminal control finds `stty` on the launch's
    /// `PATH`, so a secret typed at the terminal can still be hidden. Under
    /// confinement that `PATH` is the trusted one: a `stty` planted in a
    /// directory a confined process can write, ahead of the trusted one,
    /// never runs.
    #[cfg(target_os = "linux")]
    #[test]
    fn terminal_control_finds_stty_on_the_path_where_no_fixed_path_holds_it() {
        crate::process_isolation_tests::alone_in_child_process(
            "confinement::tests::terminal_control_finds_stty_on_the_path_where_no_fixed_path_holds_it",
            || {
                use std::os::unix::fs::PermissionsExt;
                let temp = tempfile::tempdir().unwrap();
                let base = temp.path().canonicalize().unwrap();
                let writable = base.join("writable");
                let trusted = base.join("trusted");
                let mut markers = Vec::new();
                for dir in [&writable, &trusted] {
                    std::fs::create_dir_all(dir).unwrap();
                    let marker = base.join(format!(
                        "{}-stty-ran",
                        dir.file_name().unwrap().to_string_lossy()
                    ));
                    let program = dir.join("stty");
                    std::fs::write(
                        &program,
                        format!("#!/bin/sh\nprintf ran > '{}'\n", marker.display()),
                    )
                    .unwrap();
                    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                        .unwrap();
                    markers.push(marker);
                }
                let absent = [base.join("bin/stty"), base.join("usr/bin/stty")];
                let installed = [absent[0].as_path(), absent[1].as_path()];

                let environment = vec![(
                    std::ffi::OsString::from("PATH"),
                    trusted.clone().into_os_string(),
                )];
                let status =
                    HostLaunchExemption::terminal_control_in(&installed, environment, None)
                        .status()
                        .unwrap();
                assert!(status.success(), "the stty on the PATH ran: {status}");
                assert!(markers[1].exists(), "the stty on the PATH ran");
                std::fs::remove_file(&markers[1]).unwrap();

                let environment = vec![(
                    std::ffi::OsString::from("PATH"),
                    std::env::join_paths([&writable, &trusted]).unwrap(),
                )];
                let untrusted = move |path: &Path, _: bool| path.starts_with(&writable);
                let status = HostLaunchExemption::terminal_control_in(
                    &installed,
                    environment,
                    Some(&untrusted),
                )
                .status()
                .unwrap();
                assert!(status.success(), "the trusted stty ran: {status}");
                assert!(!markers[0].exists(), "the planted stty never ran");
                assert!(markers[1].exists(), "the trusted stty ran");
            },
        );
    }

    /// A host launch exemption inherits no variable that makes a program
    /// load code or startup options from elsewhere, and no path a confined
    /// process could change: a list of paths keeps its trusted entries, a
    /// single untrusted path is dropped, and every other value is kept.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_host_launch_inherits_no_load_path_a_confined_process_could_change() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let writable = base.join("writable");
        let trusted = base.join("trusted");
        for dir in [&writable, &trusted] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let seen = base.join("environment-seen");
        let environment: Vec<(std::ffi::OsString, std::ffi::OsString)> = [
            ("PATH", "/usr/bin:/bin".to_string()),
            ("PYTHONPATH", trusted.display().to_string()),
            (
                "LD_PRELOAD",
                writable.join("libplanted.so").display().to_string(),
            ),
            ("NODE_OPTIONS", "--require planted".to_string()),
            ("GIT_DIR", trusted.display().to_string()),
            (
                "XDG_DATA_DIRS",
                format!("{}:/usr/share", writable.display()),
            ),
            (
                "CLOUDSDK_CONFIG",
                writable.join("gcloud").display().to_string(),
            ),
            ("HOME", trusted.display().to_string()),
            ("LANG", "C.UTF-8".to_string()),
            ("HTTP_PROXY", "http://proxy.invalid:8080".to_string()),
            ("DISPLAY", ":0".to_string()),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect();
        let untrusted = move |path: &Path, _: bool| path.starts_with(&writable);

        let status = HostLaunchExemption::GitSkillSource
            .launch_in("/bin/sh", environment, Some(&untrusted))
            .arg("-c")
            .arg(format!("env > '{}'", seen.display()))
            .status()
            .unwrap();
        assert!(status.success());
        let seen = std::fs::read_to_string(&seen).unwrap();
        let variable = |name: &str| {
            seen.lines()
                .find_map(|line| line.strip_prefix(&format!("{name}=")))
                .map(str::to_owned)
        };
        for dropped in [
            "PYTHONPATH",
            "LD_PRELOAD",
            "NODE_OPTIONS",
            "GIT_DIR",
            "CLOUDSDK_CONFIG",
        ] {
            assert_eq!(
                variable(dropped),
                None,
                "{dropped} is not inherited:\n{seen}"
            );
        }
        assert_eq!(variable("XDG_DATA_DIRS").as_deref(), Some("/usr/share"));
        assert_eq!(variable("PATH").as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(variable("HOME"), Some(trusted.display().to_string()));
        assert_eq!(variable("LANG").as_deref(), Some("C.UTF-8"));
        assert_eq!(
            variable("HTTP_PROXY").as_deref(),
            Some("http://proxy.invalid:8080")
        );
        assert_eq!(variable("DISPLAY").as_deref(), Some(":0"));
    }

    /// Without a host that confines its agents' launches there is no
    /// confined process to plant anything: a host launch keeps the host's
    /// environment exactly, module paths and relative `PATH` directories
    /// included.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_launch_without_host_confinement_keeps_its_environment() {
        let temp = tempfile::tempdir().unwrap();
        let seen = temp.path().join("environment-seen");
        let environment: Vec<(std::ffi::OsString, std::ffi::OsString)> = [
            ("PATH", "/usr/bin:/bin:relative-tools"),
            ("PYTHONPATH", "/opt/project/src"),
            ("NODE_OPTIONS", "--max-old-space-size=4096"),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect();

        let status = HostLaunchExemption::CredentialHelper
            .launch_in("/bin/sh", environment, None)
            .arg("-c")
            .arg(format!("env > '{}'", seen.display()))
            .status()
            .unwrap();
        assert!(status.success());
        let seen = std::fs::read_to_string(&seen).unwrap();
        for kept in [
            "PATH=/usr/bin:/bin:relative-tools",
            "PYTHONPATH=/opt/project/src",
            "NODE_OPTIONS=--max-old-space-size=4096",
        ] {
            assert!(seen.lines().any(|line| line == kept), "{kept}:\n{seen}");
        }
    }
}
