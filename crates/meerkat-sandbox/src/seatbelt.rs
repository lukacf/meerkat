// Selected policy-hardening patterns adapted from OpenAI Codex, Apache-2.0.
// Copyright 2025 OpenAI. Modified for Meerkat's explicit mechanical contract.
// Source: codex-rs/sandboxing/src/seatbelt.rs and seatbelt_base_policy.sbpl,
// revision d6c3b448a41311ece3255c52ec3dbfd9ff36f154. See ../third_party/.

use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use meerkat_core::confinement::{
    ConfinementRefusal, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
};

// CommandRuntimeV1 is deliberately smaller than Codex's compatibility profile:
// no preferences service, directory service, shared scratch, PTYs, home,
// Applications, networking, or broad Mach/Unix socket grants.
// The root directory itself is readable for loader startup, never its children.
// Rust guard-page initialization also requires the exact compatibility page size.
const BASELINE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read
  (sysctl-name "hw.ncpu") (sysctl-name "hw.memsize")
  (sysctl-name "hw.pagesize") (sysctl-name "hw.pagesize_compat")
  (sysctl-name "hw.logicalcpu")
  (sysctl-name "hw.physicalcpu") (sysctl-name "hw.machine")
  (sysctl-name "kern.ostype") (sysctl-name "kern.osrelease")
  (sysctl-name "kern.osversion") (sysctl-name "kern.argmax"))
(allow file-read*
  (literal "/")
  (subpath "/System/Library") (subpath "/usr/lib")
  (subpath "/bin") (subpath "/usr/bin")
  (subpath "/sbin") (subpath "/usr/sbin")
  (literal "/dev/null") (literal "/dev/random") (literal "/dev/urandom"))
(allow file-write-data (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))
"#;

fn quote(value: &str) -> Result<String, ConfinementRefusal> {
    if value.chars().any(char::is_control) {
        return Err(ConfinementRefusal::UnsupportedRequirement);
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

const SYSTEM_ALIASES: [(&str, &str); 3] = [
    ("/tmp", "/private/tmp"),
    ("/var", "/private/var"),
    ("/etc", "/private/etc"),
];

pub(super) fn used_system_aliases(
    requirement: &ExecutionConfinement,
) -> Vec<(&'static str, &'static str)> {
    let spec = requirement.specification();
    fn filesystem(access: &FilesystemAccess) -> &[PathAccess] {
        match access {
            FilesystemAccess::Unrestricted => &[],
            FilesystemAccess::Paths(paths) => paths.as_slice(),
        }
    }
    SYSTEM_ALIASES
        .into_iter()
        .filter(|(alias, _)| {
            filesystem(&spec.read)
                .iter()
                .chain(filesystem(&spec.write))
                .chain(&spec.deny_read)
                .chain(&spec.deny_write)
                .chain(&spec.unix_connect)
                .any(|access| access.path().starts_with(alias))
        })
        .collect()
}

pub(super) fn validate_system_aliases(aliases: &[(&str, &str)]) -> Result<(), ConfinementRefusal> {
    for (alias, expected) in aliases {
        if std::fs::canonicalize(alias).map_err(|_| ConfinementRefusal::BackendUnavailable)?
            != Path::new(expected)
        {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
    }
    Ok(())
}

/// Normalize only immutable system aliases. Mutable symlink components are
/// refused, including for missing descendants; never grant their current target.
pub(super) fn normalize(path: &Path) -> Result<PathBuf, ConfinementRefusal> {
    let mut normalized = path.to_path_buf();
    for (alias, actual) in SYSTEM_ALIASES {
        if let Ok(suffix) = path.strip_prefix(alias) {
            if std::fs::canonicalize(alias).map_err(|_| ConfinementRefusal::PreparationFailed)?
                != Path::new(actual)
            {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
            normalized = Path::new(actual).join(suffix);
            break;
        }
    }
    for ancestor in normalized.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ConfinementRefusal::PreparationFailed),
        }
    }
    Ok(normalized)
}

fn filter(access: &PathAccess) -> Result<(String, PathBuf), ConfinementRefusal> {
    let path = normalize(access.path())?;
    let value = quote(
        path.to_str()
            .ok_or(ConfinementRefusal::UnsupportedRequirement)?,
    )?;
    let predicate = match access {
        PathAccess::Literal(_) => format!("(literal {value})"),
        // A subtree includes its root, including a directory that does not
        // exist yet. Subpath alone does not cover every root-creation check.
        PathAccess::Subtree(_) => format!("(require-any (literal {value}) (subpath {value}))"),
    };
    Ok((predicate, path))
}

fn grant(
    policy: &mut String,
    operation: &str,
    access: &FilesystemAccess,
) -> Result<(), ConfinementRefusal> {
    match access {
        FilesystemAccess::Unrestricted => {
            writeln!(policy, "(allow {operation})")
                .map_err(|_| ConfinementRefusal::PreparationFailed)?;
        }
        FilesystemAccess::Paths(paths) => {
            for path in paths {
                let (predicate, _) = filter(path)?;
                writeln!(policy, "(allow {operation} {predicate})")
                    .map_err(|_| ConfinementRefusal::PreparationFailed)?;
            }
        }
    }
    Ok(())
}

pub(super) fn validate_support(
    requirement: &ExecutionConfinement,
) -> Result<(), ConfinementRefusal> {
    let spec = requirement.specification();
    if spec.require_descendant_termination
        || matches!(&spec.network, IpNetworkAccess::Connect(endpoints) if !endpoints.is_empty())
    {
        // Seatbelt's supported localhost spelling covers both address families;
        // substituting it for one exact SocketAddr would broaden the request.
        return Err(ConfinementRefusal::UnsupportedRequirement);
    }
    Ok(())
}

pub(super) fn lower(
    requirement: &ExecutionConfinement,
    protected_executables: &[&Path],
) -> Result<String, ConfinementRefusal> {
    validate_support(requirement)?;
    let spec = requirement.specification();
    let mut policy = BASELINE.to_owned();
    grant(&mut policy, "file-read*", &spec.read)?;
    grant(&mut policy, "file-write*", &spec.write)?;
    match &spec.network {
        IpNetworkAccess::Denied => {}
        IpNetworkAccess::Unrestricted => {
            policy.push_str("(allow network-outbound (remote ip \"*:*\"))\n(allow network-inbound (local ip \"*:*\"))\n(allow network-bind (local ip \"*:*\"))\n");
        }
        // Empty Connect is exactly Denied; nonempty was refused above.
        IpNetworkAccess::Connect(_) => {}
    }
    if !spec.unix_connect.is_empty() {
        policy.push_str("(allow system-socket (socket-domain AF_UNIX))\n");
        for path in &spec.unix_connect {
            let (predicate, _) = filter(path)?;
            // Unix connect resolves the original system alias and requires its
            // metadata. Grant only that validated alias, never directory data.
            for (alias, _) in SYSTEM_ALIASES {
                if path.path().starts_with(alias) {
                    writeln!(
                        policy,
                        "(allow file-read-metadata (literal {}))",
                        quote(alias)?
                    )
                    .map_err(|_| ConfinementRefusal::PreparationFailed)?;
                }
            }
            writeln!(
                policy,
                "(allow network-outbound (remote unix-socket {predicate}))"
            )
            .map_err(|_| ConfinementRefusal::PreparationFailed)?;
        }
    }
    let mut anchors = BTreeSet::new();
    if let FilesystemAccess::Paths(paths) = &spec.write {
        for path in paths {
            let (_, normalized) = filter(path)?;
            // Root replacement may rebind a literal file or a writable subtree.
            anchors.insert(normalized);
        }
    }
    for (operation, paths) in [
        ("file-read*", &spec.deny_read),
        ("file-write*", &spec.deny_write),
    ] {
        for path in paths {
            let (predicate, normalized) = filter(path)?;
            writeln!(policy, "(deny {operation} {predicate})")
                .map_err(|_| ConfinementRefusal::PreparationFailed)?;
            // Denied read resources cannot be renamed/linked into an allowed
            // path by this child, or reopened through their Unix socket path.
            if operation == "file-read*" {
                writeln!(policy, "(deny file-write* {predicate})")
                    .map_err(|_| ConfinementRefusal::PreparationFailed)?;
                writeln!(
                    policy,
                    "(deny network-outbound (remote unix-socket {predicate}))"
                )
                .map_err(|_| ConfinementRefusal::PreparationFailed)?;
            }
            anchors.extend(normalized.ancestors().map(Path::to_path_buf));
        }
    }
    // Every child is prevented from altering this launch's trusted executables
    // or replacing their ancestors for a subsequent, more restrictive launch.
    // Initial installation and ordinary same-user host processes remain trusted.
    for executable in protected_executables {
        let (predicate, normalized) = filter(&PathAccess::Literal(executable.to_path_buf()))?;
        writeln!(policy, "(deny file-write* {predicate})")
            .map_err(|_| ConfinementRefusal::PreparationFailed)?;
        anchors.extend(normalized.ancestors().map(Path::to_path_buf));
    }
    for anchor in anchors {
        let value = quote(
            anchor
                .to_str()
                .ok_or(ConfinementRefusal::UnsupportedRequirement)?,
        )?;
        writeln!(policy, "(deny file-write-unlink (literal {value}))")
            .map_err(|_| ConfinementRefusal::PreparationFailed)?;
    }
    policy.push_str("(deny mach-lookup (xpc-service-name-prefix \"\"))\n");
    if !protected_executables.is_empty()
        || !matches!(spec.write, FilesystemAccess::Unrestricted)
        || !spec.deny_write.is_empty()
        || !spec.deny_read.is_empty()
    {
        // These mutate files through read-only descriptors, outside file-write*.
        policy.push_str("(deny system-fcntl (fcntl-command 80 110))\n");
    }
    Ok(policy)
}
