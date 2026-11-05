//! OS process confinement of the shell the session launches.
//!
//! `--confinement` ([`ConfinementArg`]) is lowered into an
//! [`ExecutionConfinement`] ([`lower`]), checked against this host's
//! backend once at start-up (`meerkat_sandbox::CompiledConfinement`), and
//! handed to the factory as the shell's [`ShellConfinement::Required`]
//! requirement (`AgentFactory::with_shell_confinement`):
//!
//! | mode | read and write | read only | IP network |
//! |---|---|---|---|
//! | `workspace` (default) | the workspace, the temp dir, the Cargo home, the user cache dir | the rustup home, `/usr/local`, `/usr/share`, `/usr/include`, `/opt`, `/etc/ssl`, `/etc/pki`, `/etc/ca-certificates`; the files `/etc/{resolv.conf,hosts,nsswitch.conf,passwd,group,gitconfig}` | allowed (`cargo fetch`) |
//! | `strict` | as `workspace` | as `workspace` | denied |
//! | `off` (`--no-confinement`) | the trusted host: no OS confinement | | the host's |
//!
//! Every launch also gets the backend's `command_runtime_v1` baseline
//! (`/bin`, `/usr/bin`, `/lib`, `/usr/lib`, … and `/etc/ld.so.cache`).
//!
//! The requirement is built here rather than through upstream's `workspace`
//! preset (`HostConfinement::resolve`), whose extra grants are directory
//! subtrees only: the Linux backend refuses a grant that overlaps another or
//! the baseline (so `/etc` and `/usr` cannot be granted whole), that does
//! not exist, or that passes through a symbolic link, which leaves single
//! files such as `/etc/resolv.conf` as the only way to give a confined
//! command name resolution. [`grants`] keeps each extra path only when it
//! exists, is canonical, and is disjoint from the other grants and the
//! baseline. A host whose `/etc/resolv.conf` is a symbolic link (systemd's
//! stub resolver) cannot grant it, so name resolution fails inside the
//! Linux sandbox there; `--no-confinement` is the way out until upstream
//! grows a resolver grant.
//!
//! Only the shell (foreground calls, background jobs, monitors) is confined.
//! Command hooks and local MCP stdio servers keep running as the trusted
//! host: confining every launch kind makes a host guard its own inputs
//! (`HostConfinement::admit_host_inputs`), and the TUI's default realm state
//! lives inside the workspace (`<workspace>/.rkat`), which the Linux backend
//! cannot protect (it has no deny lists). On macOS the requirement denies
//! writes to `<workspace>/.rkat`, as the upstream preset does; on Linux a
//! confined shell can write it, and the approval policy's protected paths,
//! not the sandbox, keep the model from editing it.
//!
//! The command-line choice always wins: a realm config that declares
//! `[confinement]` is ignored for the TUI's sessions, and the start-up
//! notice says so.
//!
//! A mode this host's backend cannot enforce (no unprivileged user
//! namespaces, as on Ubuntu 24.04 with
//! `kernel.apparmor_restrict_unprivileged_userns=1`; an unsupported OS)
//! refuses the start with the backend's [`ConfinementRefusal`];
//! `--no-confinement` is the documented way out.

use std::path::{Path, PathBuf};

use meerkat_core::Config;
use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_tools::builtin::shell::ShellConfinement;

use crate::cli::ConfinementArg;
use crate::error::TuiError;

/// The paths of the backends' `command_runtime_v1` baseline (mirrors the
/// Linux backend's list): no grant may overlap them.
const BASELINE: &[&str] = &[
    "/bin",
    "/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/lib",
    "/lib64",
    "/usr/lib",
    "/usr/lib64",
    "/etc/ld.so.cache",
    "/dev",
];

/// Read-only system directories a confined command commonly needs.
const SYSTEM_READ_DIRS: &[&str] = &[
    "/usr/local",
    "/usr/share",
    "/usr/include",
    "/opt",
    "/etc/ssl",
    "/etc/pki",
    "/etc/ca-certificates",
];

/// Read-only system files: name resolution, user lookup, git defaults.
const SYSTEM_READ_FILES: &[&str] = &[
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/nsswitch.conf",
    "/etc/passwd",
    "/etc/group",
    "/etc/gitconfig",
];

/// Paths outside the workspace a confined shell commonly needs, as found on
/// this host (not yet filtered; see [`grants`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostPaths {
    /// Writable directories: the temp dir, the Cargo home, the user cache dir.
    pub write: Vec<PathBuf>,
    /// Read-only directories: toolchains, shared data, TLS roots.
    pub read: Vec<PathBuf>,
    /// Read-only files: name resolution, user lookup.
    pub read_files: Vec<PathBuf>,
}

impl HostPaths {
    /// The usual paths of this host, from the environment.
    #[must_use]
    pub fn detect() -> Self {
        let home = dirs::home_dir();
        let env_or_home = |var: &str, fallback: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|home| home.join(fallback)))
        };
        let mut read: Vec<PathBuf> = env_or_home("RUSTUP_HOME", ".rustup").into_iter().collect();
        read.extend(SYSTEM_READ_DIRS.iter().map(PathBuf::from));
        Self {
            write: [
                Some(std::env::temp_dir()),
                env_or_home("CARGO_HOME", ".cargo"),
                dirs::cache_dir(),
            ]
            .into_iter()
            .flatten()
            .collect(),
            read,
            read_files: SYSTEM_READ_FILES.iter().map(PathBuf::from).collect(),
        }
    }
}

/// The filesystem grants of a confined shell: `(read-write directories,
/// read-only directories, read-only files)`, every path existing, canonical
/// (not through a symbolic link), each once, none overlapping another or the
/// baseline. The workspace comes first among the writable directories (or
/// the writable directory containing it).
#[must_use]
pub fn grants(workspace: &Path, host: &HostPaths) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let overlaps = |a: &Path, b: &Path| a.starts_with(b) || b.starts_with(a);
    let on_baseline = |path: &Path| BASELINE.iter().any(|base| overlaps(path, Path::new(base)));
    let mut write: Vec<PathBuf> = Vec::new();
    for path in std::iter::once(workspace).chain(host.write.iter().map(PathBuf::as_path)) {
        let Some(path) = canonical(path, true) else {
            continue;
        };
        if on_baseline(&path) || write.iter().any(|kept| path.starts_with(kept)) {
            continue;
        }
        write.retain(|kept| !kept.starts_with(&path));
        write.push(path);
    }
    let mut read: Vec<PathBuf> = Vec::new();
    for path in &host.read {
        let Some(path) = canonical(path, true) else {
            continue;
        };
        if on_baseline(&path) || write.iter().chain(&read).any(|kept| overlaps(&path, kept)) {
            continue;
        }
        read.push(path);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for path in &host.read_files {
        let Some(path) = canonical(path, false) else {
            continue;
        };
        if on_baseline(&path)
            || write.iter().chain(&read).any(|dir| path.starts_with(dir))
            || files.contains(&path)
        {
            continue;
        }
        files.push(path);
    }
    (write, read, files)
}

/// `path` when it exists as a directory (`dir`) or a regular file, and is
/// absolute and canonical: no symbolic link on the way.
fn canonical(path: &Path, dir: bool) -> Option<PathBuf> {
    let resolved = std::fs::canonicalize(path).ok()?;
    let kind_ok = if dir {
        resolved.is_dir()
    } else {
        resolved.is_file()
    };
    (path.is_absolute() && resolved == path && kind_ok).then_some(resolved)
}

/// What the TUI installs for the shell, and what to tell the user.
#[derive(Debug, Clone)]
pub struct ConfinementPlan {
    /// The selected mode.
    pub mode: ConfinementArg,
    /// The shell's requirement (`TrustedHost` for `off`).
    pub shell: ShellConfinement,
    /// One-line start-up notices.
    pub notices: Vec<String>,
}

impl ConfinementPlan {
    /// The lowered requirement, `None` for `off`.
    #[must_use]
    pub fn requirement(&self) -> Option<&ExecutionConfinement> {
        match &self.shell {
            ShellConfinement::Required { requirement } => Some(requirement),
            ShellConfinement::TrustedHost => None,
        }
    }
}

/// The start-up label of a mode.
#[must_use]
pub fn describe(mode: ConfinementArg) -> &'static str {
    match mode {
        ConfinementArg::Workspace => "confinement: workspace (network allowed)",
        ConfinementArg::Strict => "confinement: strict (workspace, network denied)",
        ConfinementArg::Off => "confinement: off (commands run unconfined)",
    }
}

/// Lower `mode` over `workspace` into the shell's requirement, `None` for
/// `off`.
///
/// # Errors
///
/// [`TuiError::Confinement`] when the requirement is invalid (a relative
/// workspace root).
pub fn lower(
    mode: ConfinementArg,
    workspace: &Path,
    host: &HostPaths,
) -> Result<Option<ExecutionConfinement>, TuiError> {
    let network = match mode {
        ConfinementArg::Workspace => IpNetworkAccess::Unrestricted,
        ConfinementArg::Strict => IpNetworkAccess::Denied,
        ConfinementArg::Off => return Ok(None),
    };
    if !workspace.is_absolute() {
        return Err(TuiError::Confinement(format!(
            "the workspace root '{}' is not absolute",
            workspace.display()
        )));
    }
    let (write, read, files) = grants(workspace, host);
    let spec = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(
            subtrees(&write)
                .chain(subtrees(&read))
                .chain(files.into_iter().map(PathAccess::Literal))
                .collect(),
        ),
        write: FilesystemAccess::Paths(subtrees(&write).collect()),
        deny_read: Vec::new(),
        deny_write: protected(workspace),
        network,
        unix_connect: Vec::new(),
        require_descendant_termination: false,
    };
    ExecutionConfinement::try_from(spec)
        .map(Some)
        .map_err(|refusal| TuiError::Confinement(refusal.to_string()))
}

/// Each of `paths` as a subtree grant.
fn subtrees(paths: &[PathBuf]) -> impl Iterator<Item = PathAccess> + '_ {
    paths.iter().cloned().map(PathAccess::Subtree)
}

/// The workspace's own Meerkat state, kept from writes where the backend
/// supports deny lists (macOS).
#[cfg(target_os = "macos")]
fn protected(workspace: &Path) -> Vec<PathAccess> {
    vec![PathAccess::Subtree(workspace.join(".rkat"))]
}

/// The Linux backend has no deny lists.
#[cfg(not(target_os = "macos"))]
fn protected(_workspace: &Path) -> Vec<PathAccess> {
    Vec::new()
}

/// Whether this host's backend can enforce `requirement`.
///
/// # Errors
///
/// The backend's refusal.
pub fn check(requirement: &ExecutionConfinement) -> Result<(), ConfinementRefusal> {
    meerkat_sandbox::CompiledConfinement::compile(requirement).map(|_| ())
}

/// Lower `mode` over `workspace` and check it against this host's backend.
///
/// # Errors
///
/// [`TuiError::Confinement`] when the requirement is invalid or this host's
/// backend cannot enforce it.
pub fn plan(
    mode: ConfinementArg,
    workspace: &Path,
    config: &Config,
    host: &HostPaths,
) -> Result<ConfinementPlan, TuiError> {
    let mut notices = vec![describe(mode).to_string()];
    if !config.confinement.is_empty() {
        notices.push(
            "the realm config's [confinement] is ignored: rkat-tui confines the shell as --confinement selects"
                .to_string(),
        );
    }
    let Some(requirement) = lower(mode, workspace, host)? else {
        return Ok(ConfinementPlan {
            mode,
            shell: ShellConfinement::TrustedHost,
            notices,
        });
    };
    check(&requirement).map_err(|refusal| {
        TuiError::Confinement(format!(
            "--confinement={} cannot be enforced on this host: {refusal}; start with --no-confinement to run commands unconfined",
            mode_name(mode)
        ))
    })?;
    Ok(ConfinementPlan {
        mode,
        shell: ShellConfinement::Required { requirement },
        notices,
    })
}

fn mode_name(mode: ConfinementArg) -> &'static str {
    match mode {
        ConfinementArg::Workspace => "workspace",
        ConfinementArg::Strict => "strict",
        ConfinementArg::Off => "off",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::{ConfinementConfig, LaunchConfinement};

    struct Fixture {
        _dir: tempfile::TempDir,
        ws: PathBuf,
        cache: PathBuf,
        tools: PathBuf,
        file: PathBuf,
        host: HostPaths,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let ws = root.join("ws");
        let cache = root.join("cache");
        let tools = root.join("tools");
        let file = root.join("resolv.conf");
        for d in [&ws, &cache, &tools] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(&file, "nameserver 127.0.0.53\n").unwrap();
        let host = HostPaths {
            write: vec![cache.clone()],
            read: vec![tools.clone()],
            read_files: vec![file.clone()],
        };
        Fixture {
            _dir: dir,
            ws,
            cache,
            tools,
            file,
            host,
        }
    }

    #[test]
    fn each_mode_lowers_to_its_requirement() {
        let f = fixture();
        assert!(
            lower(ConfinementArg::Off, &f.ws, &f.host)
                .unwrap()
                .is_none()
        );
        for (mode, network) in [
            (ConfinementArg::Workspace, IpNetworkAccess::Unrestricted),
            (ConfinementArg::Strict, IpNetworkAccess::Denied),
        ] {
            let requirement = lower(mode, &f.ws, &f.host).unwrap().unwrap();
            let spec = requirement.specification();
            assert_eq!(spec.network, network, "{mode:?}");
            assert_eq!(
                spec.write,
                FilesystemAccess::Paths(vec![
                    PathAccess::Subtree(f.ws.clone()),
                    PathAccess::Subtree(f.cache.clone()),
                ]),
                "{mode:?}"
            );
            assert_eq!(
                spec.read,
                FilesystemAccess::Paths(vec![
                    PathAccess::Subtree(f.ws.clone()),
                    PathAccess::Subtree(f.cache.clone()),
                    PathAccess::Subtree(f.tools.clone()),
                    PathAccess::Literal(f.file.clone()),
                ]),
                "{mode:?}"
            );
            assert_eq!(spec.baseline, PlatformBaseline::CommandRuntimeV1);
            assert!(!spec.require_descendant_termination);
            // A host that cannot enforce it refuses the plan, naming the
            // refusal and the way out.
            match plan(mode, &f.ws, &Config::default(), &f.host) {
                Ok(plan) => assert_eq!(plan.requirement(), Some(&requirement)),
                Err(TuiError::Confinement(reason)) => {
                    let refusal = check(&requirement).unwrap_err();
                    assert!(reason.contains(&refusal.to_string()), "{reason}");
                    assert!(reason.contains("--no-confinement"), "{reason}");
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        let off = plan(ConfinementArg::Off, &f.ws, &Config::default(), &f.host).unwrap();
        assert_eq!(off.shell, ShellConfinement::TrustedHost);
        assert_eq!(off.notices, vec![describe(ConfinementArg::Off).to_string()]);
    }

    #[test]
    fn grants_are_disjoint_canonical_and_off_the_baseline() {
        let f = fixture();
        let parent = f.ws.parent().unwrap().to_path_buf();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&f.tools, &link).unwrap();
        let host = HostPaths {
            write: vec![f.cache.clone(), f.cache.join("inner"), parent.clone()],
            read: vec![
                f.tools.clone(),
                link,
                PathBuf::from("/usr"),
                PathBuf::from("/does/not/exist"),
            ],
            read_files: vec![f.file.clone(), PathBuf::from("/etc/ld.so.cache")],
        };
        let (write, read, files) = grants(&f.ws, &host);
        // The workspace's parent contains every fixture path: it is the only
        // writable grant, and nothing inside it is granted again. `/usr`
        // overlaps the baseline; the link and the missing path are not
        // canonical.
        assert_eq!(write, vec![parent]);
        assert!(read.is_empty(), "{read:?}");
        assert!(files.is_empty(), "{files:?}");
        // Detected paths always satisfy the same rules.
        let (write, read, files) = grants(&f.ws, &HostPaths::detect());
        let all: Vec<&PathBuf> = write.iter().chain(&read).chain(&files).collect();
        for (i, path) in all.iter().enumerate() {
            assert_eq!(&&std::fs::canonicalize(path).unwrap(), path);
            for other in &all[i + 1..] {
                assert!(
                    !path.starts_with(other) && !other.starts_with(path),
                    "{} overlaps {}",
                    path.display(),
                    other.display()
                );
            }
        }
    }

    #[test]
    fn a_realm_confinement_is_overridden_with_a_notice() {
        let f = fixture();
        let config = Config {
            confinement: ConfinementConfig::for_all(LaunchConfinement::TrustedHost),
            ..Config::default()
        };
        let plan = plan(ConfinementArg::Off, &f.ws, &config, &f.host).unwrap();
        assert!(
            plan.notices.iter().any(|n| n.contains("ignored")),
            "{plan:?}"
        );
    }

    #[test]
    fn a_relative_workspace_is_refused() {
        let err = lower(
            ConfinementArg::Workspace,
            Path::new("relative"),
            &HostPaths::default(),
        )
        .unwrap_err();
        assert!(matches!(err, TuiError::Confinement(_)), "{err}");
    }
}
