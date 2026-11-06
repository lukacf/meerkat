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
//! | `workspace` (default) | the workspace, the temp dir, the Cargo home, the user cache dir, each `--confinement-write` | the rustup home, `/usr/local`, `/usr/share`, `/usr/include`, `/opt`, `/etc/ssl`, `/etc/pki`, `/etc/ca-certificates`, each `--confinement-read`; the files `/etc/{resolv.conf,hosts,nsswitch.conf,host.conf,gai.conf,services,protocols,passwd,group,gitconfig}` | allowed (`cargo fetch`) |
//! | `strict` | as `workspace` | as `workspace` | denied |
//! | `off` (`--no-confinement`) | the trusted host: no OS confinement | | the host's |
//!
//! Every launch also gets the Linux backend's `command_runtime_v1` baseline:
//! the directories `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/lib`,
//! `/lib64`, `/usr/lib`, `/usr/lib64` and the files `/etc/ld.so.cache`,
//! `/dev/null`, `/dev/random`, `/dev/urandom`, read only. Nothing else of
//! `/etc`, `/run`, `/proc`, `/sys` or the home directory is visible: the
//! launch runs in an empty private mount view holding exactly these grants.
//!
//! The requirement is built here rather than through upstream's `workspace`
//! preset (`HostConfinement::resolve`). The Linux backend binds each grant
//! at its own path and refuses a grant that overlaps another or the
//! baseline (so `/etc` and `/usr` cannot be granted whole), that does not
//! exist, that passes through a symbolic link, or that is a file with more
//! than one hard link. One exception: a read-only file grant may itself be
//! a symbolic link (its directories still real), and the backend binds the
//! link's regular-file target at the link's own path. [`grants`] keeps each
//! detected path only when it exists, is canonical (a file grant: up to a
//! final link to a regular file, on Linux), and is disjoint from the other
//! grants and the baseline; single files (`PathAccess::Literal`) give a
//! confined command name resolution and user lookup without the rest of
//! `/etc`.
//!
//! **Name resolution.** `getent hosts localhost` works from the granted
//! `/etc/hosts` and `/etc/nsswitch.conf`; DNS from the granted
//! `/etc/resolv.conf`. Where that is a symbolic link (systemd-resolved:
//! `/run/systemd/resolve/stub-resolv.conf`), the confined command sees the
//! target's content, read-only, at `/etc/resolv.conf` and nothing else of the
//! target's directory; the stub resolver's `127.0.0.53` is reachable
//! wherever the IP network is (`workspace`, not `strict`).
//!
//! **Extra grants.** `--confinement-read <dir>` and `--confinement-write
//! <dir>` ([`ExtraGrants`]) add directories. Each must be absolute,
//! canonical (no symbolic link on the way), an existing directory, off the
//! baseline and `/proc`, `/sys`, `/dev`, and disjoint from the workspace and
//! the other extra grants; one inside a detected grant that already gives
//! at least that access is refused as redundant. A detected grant that
//! overlaps an extra grant otherwise yields to it (the backend refuses
//! overlaps). A violation is a typed start-up error, never a silent drop.
//! The user's own `tui.toml` layers add more with `[confinement] extra_read
//! = [...]` / `extra_write = [...]` ([`ConfinementSection`]), validated the
//! same way; a file grant that overlaps a command-line grant yields to it
//! (the flags win), and one repeating an earlier file grant with no more
//! access is dropped.
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
//! A realm config that requires confinement for the shell (`[confinement]`
//! `shell`, else `all`) is the floor: the command line may only narrow it.
//! The realm's requirement is lowered with upstream's
//! [`meerkat::HostConfinement`] and installed as is under `--confinement
//! workspace` (the default); `--confinement strict` installs it with IP
//! network access denied; `--confinement off` / `--no-confinement` is
//! refused at start-up with [`ConfinementError::BelowRealmFloor`], which
//! names the realm's policy, and extra grants with
//! [`ConfinementError::ExtraGrantsUnderRealmFloor`] (they would widen it).
//! A realm that states `trusted_host` (or nothing) for the shell leaves the
//! command line in charge.
//!
//! **Environment.** A confined command inherits nothing of rkat-tui's
//! environment: the backend clears it and sets only what the shell tool
//! passes. rkat-tui therefore passes [`ConfinementPlan::shell_env`], a
//! curated copy of its own environment ([`confined_environment`]): exactly
//! the variables [`CONFINED_ENV_ALLOWLIST`] names plus the `LC_*` family
//! (program lookup, home-relative tool directories, locale, TLS roots,
//! proxies). Every other variable, provider API keys included, stays
//! outside the sandbox. An unconfined command (`off`) inherits the whole
//! environment as before.
//!
//! A mode this host's backend cannot enforce refuses the start (fail
//! closed, no fallback) with [`ConfinementError::Unenforceable`], whose
//! message explains what blocked it ([`HostUsernsStatus`]: on Linux the
//! user-namespace switches under `/proc/sys`), how to make it work, and how
//! to run without confinement.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use meerkat_core::confinement::{
    ConfinementRefusal, ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_core::{
    Config, ConfinementConfig, ConfinementLaunchKind, LaunchConfinement, WorkspaceNetwork,
};
use meerkat_tools::builtin::shell::ShellConfinement;
use serde::{Deserialize, Serialize};

use crate::cli::{Args, ConfinementArg};

/// Why the shell's confinement cannot be installed.
#[derive(Debug, thiserror::Error)]
pub enum ConfinementError {
    /// The workspace root is not absolute.
    #[error("the workspace root '{}' is not absolute", .0.display())]
    RelativeWorkspace(PathBuf),
    /// The lowered requirement is invalid.
    #[error("the confinement requirement is invalid: {0}")]
    Invalid(#[source] ConfinementRefusal),
    /// This host's backend cannot enforce the selected mode. The message
    /// explains what blocked it, how to make it work and how to run
    /// without confinement ([`unenforceable_help`]).
    #[error("--confinement={mode} cannot be enforced on this host: {source}\n\n{help}")]
    Unenforceable {
        /// The selected mode.
        mode: &'static str,
        /// Whether the realm's floor is what cannot be enforced (then
        /// `--no-confinement` is no way out).
        floor: bool,
        /// What this host's user-namespace switches say.
        status: HostUsernsStatus,
        /// The explanation (see [`unenforceable_help`]).
        help: String,
        /// The backend's refusal.
        #[source]
        source: ConfinementRefusal,
    },
    /// The command line asks for less than the realm's `[confinement]`.
    #[error(
        "--confinement={mode} is below the realm's [confinement] policy for the shell ({policy}); the command line may only narrow it (workspace → strict)"
    )]
    BelowRealmFloor {
        /// The selected mode.
        mode: &'static str,
        /// The realm's policy, described.
        policy: String,
    },
    /// Extra grants would widen the realm's `[confinement]` floor.
    #[error(
        "--confinement-read/--confinement-write (or tui.toml [confinement] extra_read/extra_write) would widen the realm's [confinement] policy for the shell ({policy}); add the directories to the realm's [confinement.shell] read/write lists instead"
    )]
    ExtraGrantsUnderRealmFloor {
        /// The realm's policy, described.
        policy: String,
    },
    /// An extra grant (`--confinement-read`/`--confinement-write`, or a
    /// `tui.toml` `[confinement] extra_read`/`extra_write` entry) is not
    /// grantable.
    #[error("{flag} {}: {problem}", .path.display())]
    ExtraGrant {
        /// Where it came from: the flag (`--confinement-read`,
        /// `--confinement-write`) or the `tui.toml` key
        /// (`[confinement] extra_read`, `[confinement] extra_write`).
        flag: &'static str,
        /// The path as given.
        path: PathBuf,
        /// What is wrong with it.
        problem: ExtraGrantProblem,
    },
    /// Upstream could not lower the realm's policy.
    #[error("the realm's [confinement] policy cannot be lowered: {0}")]
    RealmPolicy(#[source] meerkat::HostConfinementError),
    /// The realm's policy lowered to no requirement for the shell.
    #[error("the realm's [confinement] policy names no shell requirement")]
    NoRealmRequirement,
}

/// Why an extra grant is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExtraGrantProblem {
    /// Not an absolute path.
    #[error("not an absolute path")]
    NotAbsolute,
    /// Does not exist (or cannot be resolved).
    #[error("does not exist")]
    Missing,
    /// Passes through a symbolic link.
    #[error("passes through a symbolic link, which the sandbox refuses; pass {} instead", .0.display())]
    NotCanonical(PathBuf),
    /// Not a directory.
    #[error("not a directory (only directories can be granted)")]
    NotADirectory,
    /// The root, the backend's baseline, or `/proc`, `/sys`, `/dev`.
    #[error("overlaps the sandbox's own system directories (/, /bin, /lib, /usr/bin, /usr/lib, /proc, /sys, /dev, …)")]
    Reserved,
    /// Overlaps the workspace (or the writable directory holding it).
    #[error("overlaps the workspace grant {}", .0.display())]
    OverlapsWorkspace(PathBuf),
    /// Overlaps another extra grant.
    #[error("overlaps the extra grant {}", .0.display())]
    OverlapsGrant(PathBuf),
    /// Already granted, with at least this access, by a detected grant.
    #[error("already granted through {}", .0.display())]
    AlreadyGranted(PathBuf),
}

/// What blocks unprivileged user namespaces on this host, read from the
/// switches under `/proc/sys` (Linux; the first blocker found, in the order
/// below). Only consulted once the backend refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostUsernsStatus {
    /// `user.max_user_namespaces` is `0`: no user namespaces at all.
    MaxNamespacesZero,
    /// `kernel.unprivileged_userns_clone` is `0` (Debian and some other
    /// distribution kernels).
    UnprivilegedCloneDisabled,
    /// `kernel.apparmor_restrict_unprivileged_userns` is `1` (the default
    /// on Ubuntu 24.04 and later).
    AppArmorRestricted,
    /// None of the switches above blocks it (a container's seccomp
    /// profile, a missing capability, a kernel built without them, …).
    NoKnownBlocker,
    /// Not Linux: the backend is Seatbelt (macOS) or there is none.
    NotLinux,
}

impl HostUsernsStatus {
    /// The status of this host.
    #[must_use]
    pub fn detect() -> Self {
        if cfg!(target_os = "linux") {
            Self::detect_in(Path::new("/proc"))
        } else {
            Self::NotLinux
        }
    }

    /// The status read from the procfs mounted at `proc_root` (Linux).
    #[must_use]
    pub fn detect_in(proc_root: &Path) -> Self {
        let read = |path: &str| {
            std::fs::read_to_string(proc_root.join(path))
                .ok()
                .map(|value| value.trim().to_string())
        };
        if read("sys/user/max_user_namespaces").as_deref() == Some("0") {
            Self::MaxNamespacesZero
        } else if read("sys/kernel/unprivileged_userns_clone").as_deref() == Some("0") {
            Self::UnprivilegedCloneDisabled
        } else if read("sys/kernel/apparmor_restrict_unprivileged_userns").as_deref() == Some("1")
        {
            Self::AppArmorRestricted
        } else {
            Self::NoKnownBlocker
        }
    }
}

/// The sysctl drop-in file the explanations suggest.
const SYSCTL_DROP_IN: &str = "/etc/sysctl.d/60-rkat-tui-userns.conf";

/// The explanation of a [`ConfinementError::Unenforceable`]: what blocked
/// the mode (`status`, else the backend's `refusal`), how to make it work,
/// and how to run without confinement. `floor` is the realm's
/// `[confinement]` policy when that is what cannot be enforced; `exe` is
/// this binary (for the AppArmor profile).
#[must_use]
pub fn unenforceable_help(
    status: HostUsernsStatus,
    refusal: &ConfinementRefusal,
    floor: Option<&str>,
    exe: Option<&Path>,
) -> String {
    let sysctl = |key: &str, value: &str, effect: &str| {
        format!(
            "To make it work, {effect}:\n    sudo sysctl -w {key}={value}\n  and keep it across reboots:\n    echo '{key}={value}' | sudo tee {SYSCTL_DROP_IN}\n"
        )
    };
    let (blocked, fix) = match status {
        HostUsernsStatus::MaxNamespacesZero => (
            "What blocked it: /proc/sys/user/max_user_namespaces is 0, so no process may create a user namespace, and the Linux sandbox needs one per confined command.".to_string(),
            sysctl(
                "user.max_user_namespaces",
                "15000",
                "allow user namespaces again (for every program on this host)",
            ),
        ),
        HostUsernsStatus::UnprivilegedCloneDisabled => (
            "What blocked it: /proc/sys/kernel/unprivileged_userns_clone is 0, so unprivileged processes may not create user namespaces, and the Linux sandbox needs one per confined command.".to_string(),
            sysctl(
                "kernel.unprivileged_userns_clone",
                "1",
                "allow unprivileged user namespaces (for every program on this host: a larger kernel attack surface)",
            ),
        ),
        HostUsernsStatus::AppArmorRestricted => {
            let exe = exe.map_or_else(|| "/path/to/rkat-tui".to_string(), |exe| exe.display().to_string());
            (
                "What blocked it: AppArmor restricts unprivileged user namespaces on this host (/proc/sys/kernel/apparmor_restrict_unprivileged_userns is 1, the default on Ubuntu 24.04 and later), and the Linux sandbox needs one per confined command.".to_string(),
                format!(
                    "To make it work, either:\n  - allow user namespaces for this binary only (narrower: the profile is attached by path, so it covers whatever binary sits at {exe}, and a binary installed elsewhere needs its own):\n      printf '%s\\n' 'abi <abi/4.0>,' 'include <tunables/global>' 'profile rkat-tui {exe} flags=(unconfined) {{' '  userns,' '}}' | sudo tee /etc/apparmor.d/rkat-tui\n      sudo apparmor_parser -r /etc/apparmor.d/rkat-tui\n  - or lift the restriction for every program (broader: any unprivileged program may then create user namespaces, a larger kernel attack surface):\n      sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0\n    and keep it across reboots:\n      echo 'kernel.apparmor_restrict_unprivileged_userns=0' | sudo tee {SYSCTL_DROP_IN}\n"
                ),
            )
        }
        HostUsernsStatus::NoKnownBlocker => (
            format!(
                "What blocked it: the sandbox backend refused ({refusal}). None of the usual user-namespace switches is off (user.max_user_namespaces, kernel.unprivileged_userns_clone, kernel.apparmor_restrict_unprivileged_userns); inside a container, its seccomp profile or missing privileges commonly deny unshare(CLONE_NEWUSER)."
            ),
            "To make it work, run rkat-tui where unprivileged user namespaces are allowed (on the host rather than in the container, or with a container profile that permits them).\n".to_string(),
        ),
        HostUsernsStatus::NotLinux if cfg!(target_os = "macos") => (
            format!("What blocked it: macOS confines commands with Seatbelt (sandbox-exec), and it refused this requirement ({refusal})."),
            "To make it work, check that /usr/bin/sandbox-exec is present and not blocked by a management profile.\n".to_string(),
        ),
        HostUsernsStatus::NotLinux => (
            format!("What blocked it: this platform has no sandbox backend ({refusal}); confinement is supported on Linux (user namespaces) and macOS (Seatbelt)."),
            String::new(),
        ),
    };
    let off = match floor {
        None => "To run without OS confinement instead: rkat-tui --no-confinement (or --confinement=off). Commands then run as you, guarded by the approval policy only.".to_string(),
        Some(policy) => format!(
            "Running without confinement is not available from the command line here: the realm's [confinement] policy for the shell ({policy}) requires it, and --confinement=off is refused below it. Make the host support it (above), or remove the shell statement from the realm config's [confinement] table."
        ),
    };
    format!("{blocked}\n\n{fix}\n{off}")
}

/// The paths of the backends' `command_runtime_v1` baseline (mirrors the
/// Linux backend's list), plus the paths the backend never grants: no grant
/// may overlap them.
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
    "/proc",
    "/sys",
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
    "/etc/host.conf",
    "/etc/gai.conf",
    "/etc/services",
    "/etc/protocols",
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

/// Directories the user adds to a confined shell's view:
/// `--confinement-read` / `--confinement-write`, and the `[confinement]`
/// tables of the user's own `tui.toml` layers ([`ConfinementSection`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtraGrants {
    /// Read-only directories (`--confinement-read`).
    pub read: Vec<PathBuf>,
    /// Read-write directories (`--confinement-write`).
    pub write: Vec<PathBuf>,
    /// The `tui.toml` grants; a command-line grant wins over an
    /// overlapping one.
    pub file: ConfinementSection,
}

impl ExtraGrants {
    /// The extra grants of the command line.
    #[must_use]
    pub fn from_args(args: &Args) -> Self {
        Self {
            read: args.confinement_read.clone(),
            write: args.confinement_write.clone(),
            file: ConfinementSection::default(),
        }
    }

    /// Add the grants of a `tui.toml` `[confinement]` table (they yield to
    /// the command line's, see [`grants_with`]).
    pub fn extend_from(&mut self, section: &ConfinementSection) {
        self.file.extend_from(section);
    }

    /// Whether nothing is added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.read.is_empty() && self.write.is_empty() && self.file.is_empty()
    }
}

/// A `[confinement]` table of a user's own `tui.toml` (the same grants as
/// `--confinement-read`/`--confinement-write`). Only user layers may widen
/// the sandbox; a project file must never carry one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfinementSection {
    /// Read-only directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_read: Vec<PathBuf>,
    /// Read-write directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_write: Vec<PathBuf>,
}

impl ConfinementSection {
    /// Append `other`'s grants (a later layer adds to an earlier one).
    pub fn extend_from(&mut self, other: &Self) {
        self.extra_read.extend(other.extra_read.iter().cloned());
        self.extra_write.extend(other.extra_write.iter().cloned());
    }

    /// Number of grants (directories) listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.extra_read.len() + self.extra_write.len()
    }

    /// Whether no grant is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.extra_read.is_empty() && self.extra_write.is_empty()
    }
}

/// The filesystem grants of a confined shell: `(read-write directories,
/// read-only directories, read-only files)`, every path existing, canonical
/// (not through a symbolic link), each once, none overlapping another or the
/// baseline. The workspace comes first among the writable directories (or
/// the writable directory containing it).
#[must_use]
pub fn grants(workspace: &Path, host: &HostPaths) -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
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

/// [`grants`] with the user's `extra` directories (see the module docs).
///
/// # Errors
///
/// [`ConfinementError::ExtraGrant`] naming the first extra grant that is
/// not grantable.
pub fn grants_with(
    workspace: &Path,
    host: &HostPaths,
    extra: &ExtraGrants,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>), ConfinementError> {
    let (mut write, mut read, mut files) = grants(workspace, host);
    if extra.is_empty() {
        return Ok((write, read, files));
    }
    let workspace_grant = canonical(workspace, true)
        .and_then(|ws| write.iter().find(|dir| ws.starts_with(dir)).cloned());
    // (origin label, path, writable, from the command line)
    let requested = extra
        .write
        .iter()
        .map(|path| ("--confinement-write", path, true, true))
        .chain(extra.read.iter().map(|path| ("--confinement-read", path, false, true)))
        .chain(
            extra
                .file
                .extra_write
                .iter()
                .map(|path| ("[confinement] extra_write", path, true, false)),
        )
        .chain(
            extra
                .file
                .extra_read
                .iter()
                .map(|path| ("[confinement] extra_read", path, false, false)),
        );
    // (canonical path, writable, from the command line)
    let mut accepted: Vec<(PathBuf, bool, bool)> = Vec::new();
    for (flag, given, writable, from_cli) in requested {
        let refuse = |problem| ConfinementError::ExtraGrant {
            flag,
            path: given.clone(),
            problem,
        };
        let path = validate_extra(given).map_err(refuse)?;
        if let Some(ws) = workspace_grant.as_ref().filter(|ws| overlaps(&path, ws)) {
            return Err(refuse(ExtraGrantProblem::OverlapsWorkspace(ws.clone())));
        }
        if !from_cli {
            // The command line wins over the file; a file grant repeating
            // an earlier one with no more access adds nothing.
            let yields = accepted.iter().any(|(other, other_writable, other_cli)| {
                (*other_cli && overlaps(&path, other))
                    || (*other == path && (*other_writable || !writable))
            });
            if yields {
                continue;
            }
        }
        if let Some((other, _, _)) = accepted.iter().find(|(other, _, _)| overlaps(&path, other)) {
            return Err(refuse(ExtraGrantProblem::OverlapsGrant(other.clone())));
        }
        let covering = write
            .iter()
            .chain(if writable { None } else { Some(&read) }.into_iter().flatten())
            .find(|dir| path.starts_with(dir));
        if let Some(dir) = covering {
            return Err(refuse(ExtraGrantProblem::AlreadyGranted(dir.clone())));
        }
        accepted.push((path, writable, from_cli));
    }
    let clashes = |dir: &PathBuf| accepted.iter().any(|(path, _, _)| overlaps(dir, path));
    write.retain(|dir| !clashes(dir));
    read.retain(|dir| !clashes(dir));
    files.retain(|file| !accepted.iter().any(|(path, _, _)| file.starts_with(path)));
    for (path, writable, _) in accepted {
        if writable {
            write.push(path);
        } else {
            read.push(path);
        }
    }
    Ok((write, read, files))
}

/// `given` as a grantable directory: absolute, existing, canonical, a
/// directory, off the baseline.
fn validate_extra(given: &Path) -> Result<PathBuf, ExtraGrantProblem> {
    if !given.is_absolute() {
        return Err(ExtraGrantProblem::NotAbsolute);
    }
    let resolved = std::fs::canonicalize(given).map_err(|_| ExtraGrantProblem::Missing)?;
    if resolved != given {
        return Err(ExtraGrantProblem::NotCanonical(resolved));
    }
    if !resolved.is_dir() {
        return Err(ExtraGrantProblem::NotADirectory);
    }
    if resolved == Path::new("/")
        || BASELINE
            .iter()
            .any(|base| overlaps(&resolved, Path::new(base)))
    {
        return Err(ExtraGrantProblem::Reserved);
    }
    Ok(resolved)
}

/// Whether one of `a` and `b` contains the other.
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

/// `path` when it exists as a directory (`dir`), absolute and canonical (no
/// symbolic link on the way), or as a file the sandbox backend accepts as a
/// read-only file grant ([`meerkat_sandbox::literal_file_grantable`]: on
/// Linux that includes a final symbolic link to a regular file that only
/// root or this user can retarget, bound at the link's path).
fn canonical(path: &Path, dir: bool) -> Option<PathBuf> {
    if dir {
        let resolved = std::fs::canonicalize(path).ok()?;
        return (path.is_absolute() && resolved == path && resolved.is_dir()).then_some(resolved);
    }
    meerkat_sandbox::literal_file_grantable(path).then(|| path.to_path_buf())
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
    /// The environment of a shell command: `Some` curated set of
    /// rkat-tui's own environment ([`confined_environment`]) when the shell
    /// is [`ShellConfinement::Required`] (a confined command inherits
    /// nothing, so rkat-tui passes this set), `None` when it is
    /// [`ShellConfinement::TrustedHost`] (the command inherits the whole
    /// environment, as before).
    pub shell_env: Option<HashMap<String, String>>,
}

/// The host variables a confined command gets, by exact name (the `LC_*`
/// family comes in through [`CONFINED_ENV_PREFIXES`]):
///
/// - program lookup and identity: `PATH` (so `~/.cargo/bin` and the like
///   resolve), `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`;
/// - terminal: `TERM`, `COLORTERM`, `NO_COLOR`;
/// - home-relative tool directories: `CARGO_HOME`, `RUSTUP_HOME`,
///   `RUSTUP_TOOLCHAIN`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`,
///   `XDG_DATA_HOME`;
/// - locale and time: `LANG`, `LANGUAGE`, `TZ`;
/// - TLS roots: `SSL_CERT_FILE`, `SSL_CERT_DIR`, `CURL_CA_BUNDLE`;
/// - proxies, in both spellings.
///
/// Provider API keys, cloud credentials, tokens and every other variable
/// stay outside the sandbox on purpose: a confined command sees only what
/// a build or a lookup needs.
pub const CONFINED_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "LANG",
    "LANGUAGE",
    "TZ",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "CURL_CA_BUNDLE",
    "http_proxy",
    "https_proxy",
    "no_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "ALL_PROXY",
];

/// Name prefixes a confined command gets: the locale categories (`LC_ALL`,
/// `LC_CTYPE`, ...).
pub const CONFINED_ENV_PREFIXES: &[&str] = &["LC_"];

/// The curated environment of a confined command: the variables of `vars`
/// that [`CONFINED_ENV_ALLOWLIST`] names or [`CONFINED_ENV_PREFIXES`]
/// covers. A name or value that is not UTF-8 is dropped.
#[must_use]
pub fn confined_environment(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
) -> HashMap<String, String> {
    vars.into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(name, _)| {
            CONFINED_ENV_ALLOWLIST.contains(&name.as_str())
                || CONFINED_ENV_PREFIXES
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        })
        .collect()
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
/// [`ConfinementError`] when the requirement is invalid (a relative
/// workspace root).
pub fn lower(
    mode: ConfinementArg,
    workspace: &Path,
    host: &HostPaths,
) -> Result<Option<ExecutionConfinement>, ConfinementError> {
    lower_with(mode, workspace, host, &ExtraGrants::default())
}

/// [`lower`] with the user's `extra` directories.
///
/// # Errors
///
/// [`ConfinementError`] when the requirement is invalid (a relative
/// workspace root) or an extra grant is not grantable.
pub fn lower_with(
    mode: ConfinementArg,
    workspace: &Path,
    host: &HostPaths,
    extra: &ExtraGrants,
) -> Result<Option<ExecutionConfinement>, ConfinementError> {
    let network = match mode {
        ConfinementArg::Workspace => IpNetworkAccess::Unrestricted,
        ConfinementArg::Strict => IpNetworkAccess::Denied,
        ConfinementArg::Off => return Ok(None),
    };
    if !workspace.is_absolute() {
        return Err(ConfinementError::RelativeWorkspace(workspace.to_path_buf()));
    }
    let (write, read, files) = grants_with(workspace, host, extra)?;
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
        .map_err(ConfinementError::Invalid)
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

/// The realm's shell requirement, when its `[confinement]` requires one.
fn realm_floor(config: &Config) -> Option<&LaunchConfinement> {
    config
        .confinement
        .statement(ConfinementLaunchKind::Shell)
        .filter(|statement| statement.requires_confinement())
}

/// A one-line description of a realm policy for notices and errors.
#[must_use]
pub fn describe_policy(policy: &LaunchConfinement) -> String {
    match policy {
        LaunchConfinement::TrustedHost => "trusted_host".to_string(),
        LaunchConfinement::Workspace(workspace) => format!(
            "workspace preset, network {}",
            match workspace.network {
                WorkspaceNetwork::Denied => "denied",
                WorkspaceNetwork::Unrestricted => "allowed",
            }
        ),
        LaunchConfinement::Custom(custom) => format!(
            "custom requirement, working directory {}",
            custom.working_directory.display()
        ),
    }
}

/// `floor` narrowed by `mode`: as is for `workspace`, with IP network
/// access denied for `strict`.
fn narrowed(floor: &LaunchConfinement, mode: ConfinementArg) -> Result<LaunchConfinement, ConfinementError> {
    if mode != ConfinementArg::Strict {
        return Ok(floor.clone());
    }
    Ok(match floor {
        LaunchConfinement::Workspace(workspace) => {
            let mut workspace = workspace.clone();
            workspace.network = WorkspaceNetwork::Denied;
            LaunchConfinement::Workspace(workspace)
        }
        LaunchConfinement::Custom(custom) => {
            let mut custom = custom.clone();
            let mut spec = custom.requirement.specification().clone();
            spec.network = IpNetworkAccess::Denied;
            custom.requirement =
                ExecutionConfinement::try_from(spec).map_err(ConfinementError::Invalid)?;
            LaunchConfinement::Custom(custom)
        }
        LaunchConfinement::TrustedHost => LaunchConfinement::TrustedHost,
    })
}

/// The shell's requirement under the realm's `floor`, narrowed by `mode`.
fn lower_floor(
    floor: &LaunchConfinement,
    mode: ConfinementArg,
    workspace: &Path,
) -> Result<ExecutionConfinement, ConfinementError> {
    let selection = ConfinementConfig {
        shell: Some(narrowed(floor, mode)?),
        ..ConfinementConfig::default()
    };
    let resolved = meerkat::HostConfinement::resolve(&selection, Some(workspace))
        .map_err(ConfinementError::RealmPolicy)?;
    resolved
        .requirement(ConfinementLaunchKind::Shell)
        .cloned()
        .ok_or(ConfinementError::NoRealmRequirement)
}

/// Lower `mode` over `workspace` (under the realm's `[confinement]` floor,
/// see the module docs) and check it against this host's backend.
///
/// # Errors
///
/// [`ConfinementError::BelowRealmFloor`] when `mode` is `off` against a
/// realm policy, [`ConfinementError::Unenforceable`] when this host's
/// backend cannot enforce the requirement, otherwise when the requirement is
/// invalid.
pub fn plan(
    mode: ConfinementArg,
    workspace: &Path,
    config: &Config,
    host: &HostPaths,
    extra: &ExtraGrants,
) -> Result<ConfinementPlan, ConfinementError> {
    let floor = realm_floor(config);
    let requirement = match floor {
        Some(policy) if mode == ConfinementArg::Off => {
            return Err(ConfinementError::BelowRealmFloor {
                mode: mode_name(mode),
                policy: describe_policy(policy),
            });
        }
        Some(policy) if !extra.is_empty() => {
            return Err(ConfinementError::ExtraGrantsUnderRealmFloor {
                policy: describe_policy(policy),
            });
        }
        Some(policy) => Some(lower_floor(policy, mode, workspace)?),
        None => lower_with(mode, workspace, host, extra)?,
    };
    let mut notices = vec![match floor {
        Some(policy) => format!(
            "confinement: the realm's [confinement] policy ({}){}",
            describe_policy(policy),
            if mode == ConfinementArg::Strict {
                ", network denied by --confinement=strict"
            } else {
                ""
            }
        ),
        None => describe(mode).to_string(),
    }];
    if floor.is_none() && !config.confinement.is_empty() {
        notices.push(
            "the realm config's [confinement] does not confine the shell; rkat-tui confines it as --confinement selects"
                .to_string(),
        );
    }
    let Some(requirement) = requirement else {
        if !extra.is_empty() {
            notices.push(
                "--confinement-read/--confinement-write and tui.toml [confinement] grants have no effect: commands run unconfined"
                    .to_string(),
            );
        }
        return Ok(ConfinementPlan {
            mode,
            shell: ShellConfinement::TrustedHost,
            notices,
            shell_env: None,
        });
    };
    check(&requirement).map_err(|source| {
        let status = HostUsernsStatus::detect();
        let exe = std::env::current_exe()
            .ok()
            .map(|exe| std::fs::canonicalize(&exe).unwrap_or(exe));
        let policy = floor.map(describe_policy);
        ConfinementError::Unenforceable {
            mode: mode_name(mode),
            floor: floor.is_some(),
            status,
            help: unenforceable_help(status, &source, policy.as_deref(), exe.as_deref()),
            source,
        }
    })?;
    Ok(ConfinementPlan {
        mode,
        shell: ShellConfinement::Required { requirement },
        notices,
        shell_env: Some(confined_environment(std::env::vars_os())),
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

    fn none() -> ExtraGrants {
        ExtraGrants::default()
    }

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
            match plan(mode, &f.ws, &Config::default(), &f.host, &none()) {
                Ok(plan) => {
                    assert_eq!(plan.requirement(), Some(&requirement));
                    assert!(plan.shell_env.is_some(), "{mode:?}");
                }
                Err(err @ ConfinementError::Unenforceable { .. }) => {
                    let refusal = check(&requirement).unwrap_err();
                    let reason = err.to_string();
                    assert!(reason.contains(&refusal.to_string()), "{reason}");
                    assert!(reason.contains("What blocked it"), "{reason}");
                    assert!(reason.contains("rkat-tui --no-confinement"), "{reason}");
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        let off = plan(ConfinementArg::Off, &f.ws, &Config::default(), &f.host, &none()).unwrap();
        assert_eq!(off.shell, ShellConfinement::TrustedHost);
        assert_eq!(off.notices, vec![describe(ConfinementArg::Off).to_string()]);
        assert_eq!(off.shell_env, None);
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    #[test]
    fn a_confined_command_gets_the_allowlist_and_nothing_else() {
        let mut host = vars(&[
            ("PATH", "/home/u/.cargo/bin:/usr/bin"),
            ("HOME", "/home/u"),
            ("LANG", "en_US.UTF-8"),
            ("LC_ALL", "C.UTF-8"),
            ("LC_CTYPE", "en_US.UTF-8"),
            ("https_proxy", "http://proxy:3128"),
            ("SSL_CERT_FILE", "/etc/ssl/cert.pem"),
            ("ANTHROPIC_API_KEY", "sk-ant"),
            ("AWS_SECRET_ACCESS_KEY", "secret"),
            ("GITHUB_TOKEN", "ghp"),
            ("LD_PRELOAD", "/tmp/evil.so"),
            ("BASH_ENV", "/tmp/rc"),
            ("RUSTFLAGS", "-Dwarnings"),
        ]);
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            host.push((
                OsString::from("TERM"),
                OsString::from_vec(vec![0xff, 0xfe]),
            ));
        }
        let env = confined_environment(host);
        for name in [
            "PATH",
            "HOME",
            "LANG",
            "LC_ALL",
            "LC_CTYPE",
            "https_proxy",
            "SSL_CERT_FILE",
        ] {
            assert!(env.contains_key(name), "{name} kept");
        }
        assert_eq!(env["PATH"], "/home/u/.cargo/bin:/usr/bin");
        for name in [
            "ANTHROPIC_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "LD_PRELOAD",
            "BASH_ENV",
            "RUSTFLAGS",
            "TERM",
        ] {
            assert!(!env.contains_key(name), "{name} dropped");
        }
        assert_eq!(env.len(), 7, "{env:?}");
    }

    #[test]
    fn no_allowlisted_name_is_one_the_sandbox_refuses() {
        const REFUSED_PREFIXES: [&str; 5] = ["DYLD_", "LD_", "_RLD_", "__XPC_", "BASH_FUNC_"];
        const REFUSED_KEYS: [&str; 6] = [
            "ENV",
            "BASH_ENV",
            "SHELLOPTS",
            "BASHOPTS",
            "ZDOTDIR",
            "GLIBC_TUNABLES",
        ];
        for name in CONFINED_ENV_ALLOWLIST.iter().chain(CONFINED_ENV_PREFIXES) {
            assert!(!REFUSED_KEYS.contains(name), "{name}");
            for prefix in REFUSED_PREFIXES {
                assert!(!name.starts_with(prefix), "{name}");
                // A prefix family must not overlap a refused family either.
                assert!(!prefix.starts_with(name), "{name}");
            }
        }
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
            // A file grant may be a final link; its directory is canonical.
            let named = if files.contains(path) {
                let parent = std::fs::canonicalize(path.parent().unwrap()).unwrap();
                parent.join(path.file_name().unwrap())
            } else {
                std::fs::canonicalize(path).unwrap()
            };
            assert_eq!(&&named, path);
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
    fn a_file_grant_may_be_a_final_link_to_a_regular_file_on_linux() {
        let f = fixture();
        let root = f.ws.parent().unwrap().to_path_buf();
        let etc = root.join("etc");
        let run = root.join("run");
        std::fs::create_dir_all(&etc).unwrap();
        std::fs::create_dir_all(&run).unwrap();
        // The backend grants a link only where no other user can retarget
        // it, so the link's directories must not be group-writable whatever
        // the umask made them.
        let set_mode = |path: &Path, mode: u32| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        set_mode(&root, 0o700);
        set_mode(&etc, 0o755);
        let target = run.join("stub-resolv.conf");
        std::fs::write(&target, "nameserver 127.0.0.53\n").unwrap();
        let link = etc.join("resolv.conf");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let dir_link = etc.join("dir");
        std::os::unix::fs::symlink(&run, &dir_link).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&etc, &alias).unwrap();
        let linux = cfg!(target_os = "linux");

        assert_eq!(canonical(&link, false), linux.then(|| link.clone()));
        // A link another user could retarget (a group-writable directory) is
        // dropped, as the backend would refuse it.
        set_mode(&etc, 0o775);
        assert_eq!(canonical(&link, false), None);
        set_mode(&etc, 0o755);
        // A regular file stays grantable; a link to a directory, a file
        // reached through a symlinked directory, and a dangling link are
        // refused.
        assert_eq!(canonical(&target, false), Some(target.clone()));
        assert_eq!(canonical(&dir_link, false), None);
        assert_eq!(canonical(&alias.join("resolv.conf"), false), None);
        std::os::unix::fs::symlink(run.join("missing"), etc.join("dangling")).unwrap();
        assert_eq!(canonical(&etc.join("dangling"), false), None);

        // The link alone is granted, as a read-only file at its own path;
        // nothing of the target's directory is.
        let host = HostPaths {
            write: vec![],
            read: vec![],
            read_files: vec![link.clone()],
        };
        let (write, read, files) = grants(&f.ws, &host);
        assert_eq!(write, vec![f.ws.clone()]);
        assert!(read.is_empty(), "{read:?}");
        assert_eq!(files, if linux { vec![link] } else { vec![] });
        let host = HostPaths {
            write: vec![root.clone()],
            ..host
        };
        let (write, read, files) = grants(&f.ws, &host);
        assert_eq!(write, vec![root]);
        assert!(read.is_empty() && files.is_empty(), "{read:?} {files:?}");
    }

    fn realm_with(shell: LaunchConfinement) -> Config {
        Config {
            confinement: ConfinementConfig {
                shell: Some(shell),
                ..ConfinementConfig::default()
            },
            ..Config::default()
        }
    }

    fn workspace_floor(network: WorkspaceNetwork) -> LaunchConfinement {
        LaunchConfinement::Workspace(meerkat_core::WorkspaceConfinement {
            network,
            read: Vec::new(),
            write: Vec::new(),
        })
    }

    #[test]
    fn a_realm_policy_is_the_floor_the_command_line_may_only_narrow() {
        let f = fixture();
        let config = realm_with(workspace_floor(WorkspaceNetwork::Unrestricted));
        // `off` against the realm's policy: a typed start-up error naming it.
        let err = plan(ConfinementArg::Off, &f.ws, &config, &f.host, &none()).unwrap_err();
        assert!(
            matches!(&err, ConfinementError::BelowRealmFloor { policy, .. } if policy.contains("workspace preset")),
            "{err}"
        );
        assert!(err.to_string().contains("realm's [confinement]"), "{err}");
        // `workspace` installs the realm's requirement; `strict` narrows its
        // network. A host that cannot enforce it refuses either, with no
        // `--no-confinement` way out.
        for (mode, network) in [
            (ConfinementArg::Workspace, IpNetworkAccess::Unrestricted),
            (ConfinementArg::Strict, IpNetworkAccess::Denied),
        ] {
            let floor = config
                .confinement
                .statement(ConfinementLaunchKind::Shell)
                .unwrap();
            let requirement = lower_floor(floor, mode, &f.ws).unwrap();
            assert_eq!(requirement.specification().network, network, "{mode:?}");
            match plan(mode, &f.ws, &config, &f.host, &none()) {
                Ok(plan) => {
                    assert_eq!(plan.requirement(), Some(&requirement), "{mode:?}");
                    assert!(plan.notices[0].contains("realm's [confinement]"));
                    assert!(plan.shell_env.is_some(), "{mode:?}");
                }
                Err(err @ ConfinementError::Unenforceable { floor: true, .. }) => {
                    let text = err.to_string();
                    assert!(!text.contains("rkat-tui --no-confinement"), "{text}");
                    assert!(text.contains("workspace preset"), "{text}");
                }
                Err(other) => panic!("unexpected error: {other}"),
            }
        }
        // `trusted_host` in the realm leaves the command line in charge.
        let trusted = realm_with(LaunchConfinement::TrustedHost);
        let off = plan(ConfinementArg::Off, &f.ws, &trusted, &f.host, &none()).unwrap();
        assert_eq!(off.shell, ShellConfinement::TrustedHost);
        assert!(off.notices.iter().any(|n| n.contains("does not confine the shell")));
        assert_eq!(off.shell_env, None);
    }

    #[test]
    fn a_relative_workspace_is_refused() {
        let err = lower(
            ConfinementArg::Workspace,
            Path::new("relative"),
            &HostPaths::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ConfinementError::RelativeWorkspace(_)), "{err}");
    }

    /// A fake procfs holding the three user-namespace switches.
    fn fake_proc(max: Option<&str>, clone: Option<&str>, apparmor: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, value) in [
            ("sys/user/max_user_namespaces", max),
            ("sys/kernel/unprivileged_userns_clone", clone),
            ("sys/kernel/apparmor_restrict_unprivileged_userns", apparmor),
        ] {
            if let Some(value) = value {
                let path = dir.path().join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, format!("{value}\n")).unwrap();
            }
        }
        dir
    }

    #[test]
    fn userns_status_is_read_from_the_proc_switches() {
        for (max, clone, apparmor, expected) in [
            (Some("348787"), Some("1"), Some("1"), HostUsernsStatus::AppArmorRestricted),
            (Some("348787"), None, Some("1"), HostUsernsStatus::AppArmorRestricted),
            (Some("348787"), Some("0"), Some("1"), HostUsernsStatus::UnprivilegedCloneDisabled),
            (Some("0"), Some("0"), Some("1"), HostUsernsStatus::MaxNamespacesZero),
            (Some("348787"), Some("1"), Some("0"), HostUsernsStatus::NoKnownBlocker),
            (None, None, None, HostUsernsStatus::NoKnownBlocker),
        ] {
            let proc_root = fake_proc(max, clone, apparmor);
            assert_eq!(
                HostUsernsStatus::detect_in(proc_root.path()),
                expected,
                "{max:?} {clone:?} {apparmor:?}"
            );
        }
    }

    #[test]
    fn the_refusal_explains_the_blocker_the_fix_and_the_way_out() {
        let refusal = ConfinementRefusal::BackendUnavailable;
        let exe = Path::new("/opt/bin/rkat-tui");
        let apparmor =
            unenforceable_help(HostUsernsStatus::AppArmorRestricted, &refusal, None, Some(exe));
        for needle in [
            "apparmor_restrict_unprivileged_userns is 1",
            "sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0",
            "/etc/sysctl.d/",
            "profile rkat-tui /opt/bin/rkat-tui flags=(unconfined) {",
            "userns,",
            "sudo apparmor_parser -r /etc/apparmor.d/rkat-tui",
            "larger kernel attack surface",
            "rkat-tui --no-confinement (or --confinement=off)",
        ] {
            assert!(apparmor.contains(needle), "missing {needle:?} in:\n{apparmor}");
        }
        let clone =
            unenforceable_help(HostUsernsStatus::UnprivilegedCloneDisabled, &refusal, None, None);
        assert!(clone.contains("sudo sysctl -w kernel.unprivileged_userns_clone=1"), "{clone}");
        let max = unenforceable_help(HostUsernsStatus::MaxNamespacesZero, &refusal, None, None);
        assert!(max.contains("sudo sysctl -w user.max_user_namespaces="), "{max}");
        let unknown = unenforceable_help(HostUsernsStatus::NoKnownBlocker, &refusal, None, None);
        assert!(unknown.contains(&refusal.to_string()), "{unknown}");
        // Under a realm floor the way out is the realm config, not a flag.
        let floor = unenforceable_help(
            HostUsernsStatus::AppArmorRestricted,
            &refusal,
            Some("workspace preset, network allowed"),
            None,
        );
        assert!(floor.contains("realm config's [confinement]"), "{floor}");
        assert!(!floor.contains("rkat-tui --no-confinement"), "{floor}");
    }

    #[test]
    fn extra_grants_are_validated_and_take_precedence() {
        let f = fixture();
        let root = f.ws.parent().unwrap().to_path_buf();
        let extra_dir = root.join("extra");
        let data = root.join("data");
        std::fs::create_dir_all(&extra_dir).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let extra = ExtraGrants {
            read: vec![data.clone()],
            write: vec![extra_dir.clone()],
            ..ExtraGrants::default()
        };
        let (write, read, files) = grants_with(&f.ws, &f.host, &extra).unwrap();
        assert_eq!(write, vec![f.ws.clone(), f.cache.clone(), extra_dir]);
        assert_eq!(read, vec![f.tools.clone(), data.clone()]);
        assert_eq!(files, vec![f.file.clone()]);
        // A detected read grant overlapping a write grant yields to it.
        let inner = f.tools.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let (_, read, _) = grants_with(
            &f.ws,
            &f.host,
            &ExtraGrants {
                read: Vec::new(),
                write: vec![inner.clone()],
                ..ExtraGrants::default()
            },
        )
        .unwrap();
        assert!(!read.contains(&f.tools), "{read:?}");
        // Every violation is a typed error naming the flag and the path.
        let link = root.join("link");
        std::os::unix::fs::symlink(&data, &link).unwrap();
        let file = f.file.clone();
        for (extra, expected) in [
            (vec![PathBuf::from("relative")], ExtraGrantProblem::NotAbsolute),
            (vec![root.join("missing")], ExtraGrantProblem::Missing),
            (vec![link], ExtraGrantProblem::NotCanonical(data.clone())),
            (vec![file], ExtraGrantProblem::NotADirectory),
            (vec![PathBuf::from("/usr/lib")], ExtraGrantProblem::Reserved),
            (vec![PathBuf::from("/")], ExtraGrantProblem::Reserved),
            (vec![f.ws.join("..").join("ws")], ExtraGrantProblem::NotCanonical(f.ws.clone())),
            (vec![root.clone()], ExtraGrantProblem::OverlapsWorkspace(f.ws.clone())),
            (vec![data.clone(), data.clone()], ExtraGrantProblem::OverlapsGrant(data)),
            (vec![f.cache.clone()], ExtraGrantProblem::AlreadyGranted(f.cache.clone())),
            (vec![f.tools.clone()], ExtraGrantProblem::AlreadyGranted(f.tools.clone())),
        ] {
            let err = grants_with(
                &f.ws,
                &f.host,
                &ExtraGrants {
                    read: extra.clone(),
                    write: Vec::new(),
                    ..ExtraGrants::default()
                },
            )
            .unwrap_err();
            match &err {
                ConfinementError::ExtraGrant { flag, problem, .. } => {
                    assert_eq!(*flag, "--confinement-read");
                    assert_eq!(problem, &expected, "{extra:?}");
                }
                other => panic!("unexpected error: {other}"),
            }
        }
        // A read grant may not repeat a detected read grant, but a write
        // grant inside a read grant upgrades that part.
        assert!(
            grants_with(
                &f.ws,
                &f.host,
                &ExtraGrants {
                    read: vec![inner],
                    write: Vec::new(),
                    ..ExtraGrants::default()
                },
            )
            .is_err()
        );
    }

    #[test]
    fn extra_grants_may_not_widen_a_realm_floor() {
        let f = fixture();
        let config = realm_with(workspace_floor(WorkspaceNetwork::Unrestricted));
        let extra = ExtraGrants {
            read: vec![f.tools.clone()],
            write: Vec::new(),
            ..ExtraGrants::default()
        };
        let err = plan(ConfinementArg::Workspace, &f.ws, &config, &f.host, &extra).unwrap_err();
        assert!(
            matches!(err, ConfinementError::ExtraGrantsUnderRealmFloor { .. }),
            "{err}"
        );
        // Under `off` they only earn a notice.
        let off = plan(ConfinementArg::Off, &f.ws, &Config::default(), &f.host, &extra).unwrap();
        assert!(off.notices.iter().any(|n| n.contains("no effect")), "{:?}", off.notices);
    }

    #[test]
    fn a_confinement_table_parses_into_extra_grants() {
        let section: ConfinementSection =
            toml::from_str("extra_read = [\"/a\"]\nextra_write = [\"/b\"]\n").unwrap();
        let mut extra = ExtraGrants::default();
        assert!(extra.is_empty());
        extra.extend_from(&section);
        assert!(!extra.is_empty());
        assert!(extra.read.is_empty() && extra.write.is_empty());
        assert_eq!(extra.file.extra_read, vec![PathBuf::from("/a")]);
        assert_eq!(extra.file.extra_write, vec![PathBuf::from("/b")]);
        assert_eq!(extra.file.len(), 2);
        assert!(toml::from_str::<ConfinementSection>("extra = []\n").is_err());
    }

    #[test]
    fn file_grants_are_validated_and_yield_to_the_command_line() {
        let f = fixture();
        let root = f.ws.parent().unwrap().to_path_buf();
        let data = root.join("data");
        let inner = data.join("inner");
        let other = root.join("other");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let file = |read: Vec<PathBuf>, write: Vec<PathBuf>| ConfinementSection {
            extra_read: read,
            extra_write: write,
        };
        // File grants alone are granted like flags.
        let extra = ExtraGrants {
            file: file(vec![data.clone()], vec![other.clone()]),
            ..ExtraGrants::default()
        };
        let (write, read, _) = grants_with(&f.ws, &f.host, &extra).unwrap();
        assert!(write.contains(&other), "{write:?}");
        assert!(read.contains(&data), "{read:?}");
        // A flag wins over an overlapping file grant, in either direction
        // and whatever the access.
        let extra = ExtraGrants {
            write: vec![data.clone()],
            file: file(vec![data.clone(), inner.clone()], Vec::new()),
            ..ExtraGrants::default()
        };
        let (write, read, _) = grants_with(&f.ws, &f.host, &extra).unwrap();
        assert!(write.contains(&data), "{write:?}");
        assert!(!read.contains(&data) && !read.contains(&inner), "{read:?}");
        let extra = ExtraGrants {
            read: vec![inner.clone()],
            file: file(Vec::new(), vec![data.clone()]),
            ..ExtraGrants::default()
        };
        let (write, read, _) = grants_with(&f.ws, &f.host, &extra).unwrap();
        assert!(read.contains(&inner) && !write.contains(&data), "{write:?} {read:?}");
        // A grant repeated across layers is granted once, with the most
        // access any layer gives.
        let extra = ExtraGrants {
            file: file(vec![data.clone(), data.clone()], vec![data.clone()]),
            ..ExtraGrants::default()
        };
        let (write, read, _) = grants_with(&f.ws, &f.host, &extra).unwrap();
        assert_eq!(write.iter().filter(|d| **d == data).count(), 1);
        assert!(!read.contains(&data));
        // Overlapping file grants and invalid ones are typed errors naming
        // the tui.toml key.
        for (extra, key, expected) in [
            (
                file(vec![data.clone(), inner], Vec::new()),
                "[confinement] extra_read",
                ExtraGrantProblem::OverlapsGrant(data.clone()),
            ),
            (
                file(Vec::new(), vec![root.join("missing")]),
                "[confinement] extra_write",
                ExtraGrantProblem::Missing,
            ),
            (
                file(vec![root.clone()], Vec::new()),
                "[confinement] extra_read",
                ExtraGrantProblem::OverlapsWorkspace(f.ws.clone()),
            ),
        ] {
            let err = grants_with(
                &f.ws,
                &f.host,
                &ExtraGrants {
                    file: extra,
                    ..ExtraGrants::default()
                },
            )
            .unwrap_err();
            match &err {
                ConfinementError::ExtraGrant { flag, problem, .. } => {
                    assert_eq!(*flag, key);
                    assert_eq!(problem, &expected);
                }
                other => panic!("unexpected error: {other}"),
            }
        }
        // They widen a realm floor just like flags.
        let config = realm_with(workspace_floor(WorkspaceNetwork::Unrestricted));
        let extra = ExtraGrants {
            file: file(vec![data], Vec::new()),
            ..ExtraGrants::default()
        };
        assert!(matches!(
            plan(ConfinementArg::Workspace, &f.ws, &config, &f.host, &extra),
            Err(ConfinementError::ExtraGrantsUnderRealmFloor { .. })
        ));
    }
}
