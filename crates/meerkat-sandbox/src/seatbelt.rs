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
// Process information is denied explicitly: the default deny does not cover
// every process-info read on every macOS version (such as another process's
// arguments and environment). The later same-sandbox allow wins over it.
const BASELINE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(deny process-info*)
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::confinement::{ConfinementSpec, PlatformBaseline};

    /// A rendered profile denies process information, then allows it for the
    /// child's own sandbox. The later matching rule wins, so a process outside
    /// the sandbox stays denied whatever the default deny covers.
    #[test]
    fn profile_denies_process_info_before_allowing_the_same_sandbox() {
        let requirement = ExecutionConfinement::try_from(ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(Vec::new()),
            write: FilesystemAccess::Paths(Vec::new()),
            deny_read: Vec::new(),
            deny_write: Vec::new(),
            network: IpNetworkAccess::Denied,
            unix_connect: Vec::new(),
            require_descendant_termination: false,
        })
        .unwrap();
        let policy = lower(&requirement, &[]).unwrap();
        let deny = policy.find("\n(deny process-info*)\n");
        let allow = policy.find("\n(allow process-info* (target same-sandbox))\n");
        assert!(
            matches!((deny, allow), (Some(deny), Some(allow)) if deny < allow),
            "{policy}"
        );
        assert_eq!(policy.matches("(allow process-info").count(), 1, "{policy}");
    }

    // The granted paths below exist on no host and pass through no system
    // alias, so the lowering is the same on every platform that runs the row.
    fn subtrees(paths: &[&str]) -> FilesystemAccess {
        FilesystemAccess::Paths(
            paths
                .iter()
                .map(|path| PathAccess::Subtree(PathBuf::from(path)))
                .collect(),
        )
    }

    fn policy(
        read: FilesystemAccess,
        write: FilesystemAccess,
        deny_read: &[&str],
        network: IpNetworkAccess,
    ) -> String {
        let requirement = ExecutionConfinement::try_from(ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read,
            write,
            deny_read: deny_read
                .iter()
                .map(|path| PathAccess::Subtree(PathBuf::from(path)))
                .collect(),
            deny_write: Vec::new(),
            network,
            unix_connect: Vec::new(),
            require_descendant_termination: false,
        })
        .unwrap();
        lower(&requirement, &[]).unwrap()
    }

    fn metadata(path: &str) -> String {
        format!("(allow file-read-metadata (literal \"{path}\"))\n")
    }

    /// A program that resolves a path component by component (a script
    /// runtime resolving its main file, `realpath`) reads the metadata of
    /// every directory above a granted path. Each of them is allowed, for
    /// metadata alone: no listing and no file data above a grant.
    #[test]
    fn profile_allows_metadata_alone_on_each_directory_above_a_granted_path() {
        let policy = policy(
            subtrees(&["/meerkat-row/work/project", "/meerkat-row/tool/bin"]),
            subtrees(&["/meerkat-row/work/project", "/meerkat-row/out/build"]),
            &[],
            IpNetworkAccess::Denied,
        );
        for above in [
            "/meerkat-row",
            "/meerkat-row/work",
            "/meerkat-row/tool",
            "/meerkat-row/out",
        ] {
            assert_eq!(
                policy.matches(&metadata(above)).count(),
                1,
                "{above}: {policy}"
            );
            for broader in ["file-read*", "file-read-data", "file-write*"] {
                assert!(
                    !policy.contains(&format!("(allow {broader} (literal \"{above}\"))"))
                        && !policy.contains(&format!("(subpath \"{above}\")")),
                    "{above} gets more than metadata: {policy}"
                );
            }
        }
        // The root is readable by the baseline; a granted path is granted whole.
        for granted in ["/", "/meerkat-row/work/project", "/meerkat-row/tool/bin"] {
            assert!(!policy.contains(&metadata(granted)), "{granted}: {policy}");
        }
    }

    /// A denied read stays denied where it lies above a granted path: its
    /// deny rule follows the metadata allowance, and the later rule wins.
    #[test]
    fn profile_denies_a_read_after_allowing_metadata_above_granted_paths() {
        let policy = policy(
            subtrees(&["/meerkat-row/private/shared"]),
            subtrees(&[]),
            &["/meerkat-row/private"],
            IpNetworkAccess::Denied,
        );
        let allow = policy.find(&metadata("/meerkat-row/private"));
        let deny = policy.find(
            "(deny file-read* (require-any (literal \"/meerkat-row/private\") (subpath \"/meerkat-row/private\")))\n",
        );
        assert!(
            matches!((allow, deny), (Some(allow), Some(deny)) if allow < deny),
            "{policy}"
        );
    }

    /// With every read allowed there is nothing above a grant left to allow.
    #[test]
    fn profile_with_unrestricted_reads_names_no_directory_above_a_grant() {
        let policy = policy(
            FilesystemAccess::Unrestricted,
            subtrees(&["/meerkat-row/work/project"]),
            &[],
            IpNetworkAccess::Denied,
        );
        assert!(!policy.contains("\"/meerkat-row/work\""), "{policy}");
    }

    /// `uname` reads the host name and the kernel version besides the names
    /// the baseline already allowed, and fails as a whole when one is
    /// refused. The system aliases `/etc`, `/tmp` and `/var` are links: a
    /// path spelled through one resolves only when the link itself can be
    /// read. The fixed system directories above baseline paths are allowed
    /// for metadata like the directories above a grant.
    #[test]
    fn baseline_allows_the_names_uname_reads_and_system_link_metadata() {
        let policy = policy(subtrees(&[]), subtrees(&[]), &[], IpNetworkAccess::Denied);
        for name in ["kern.hostname", "kern.version"] {
            assert_eq!(
                policy.matches(&format!("(sysctl-name \"{name}\")")).count(),
                1,
                "{name}: {policy}"
            );
        }
        // The baseline's one metadata allowance, up to its closing line.
        let allowance = policy
            .split_once("(allow file-read-metadata\n")
            .and_then(|(_, rest)| rest.split_once("))\n"))
            .map(|(allowance, _)| format!("{allowance})"))
            .unwrap_or_default();
        for path in [
            "/etc", "/tmp", "/var", "/private", "/usr", "/System", "/dev",
        ] {
            assert_eq!(
                allowance.matches(&format!("(literal \"{path}\")")).count(),
                1,
                "{path}: {policy}"
            );
        }
        assert!(!allowance.contains("subpath"), "{policy}");
    }

    const RESOLVER_SOCKET: &str = "(allow network-outbound (remote unix-socket (literal \"/private/var/run/mDNSResponder\")))\n";
    const RESOLVER_SERVICE: &str =
        "(allow mach-lookup (global-name \"com.apple.dnssd.service\"))\n";
    const SYSTEM_CA_FILES: &str = "(allow file-read* (require-any (literal \"/private/etc/ssl\") (subpath \"/private/etc/ssl\")))\n";

    /// IP access without name resolution reaches addresses only. A launch
    /// with IP access also reaches the system resolver (its socket and its
    /// service name, nothing else of either kind) and reads the system
    /// certificate authority files. The service allowance follows the
    /// general service denial, so it is the one that holds.
    #[test]
    fn unrestricted_network_reaches_the_system_resolver_and_ca_files() {
        let policy = policy(
            subtrees(&[]),
            subtrees(&[]),
            &[],
            IpNetworkAccess::Unrestricted,
        );
        for rule in [
            "(allow system-socket (socket-domain AF_UNIX))\n",
            RESOLVER_SOCKET,
            RESOLVER_SERVICE,
            SYSTEM_CA_FILES,
        ] {
            assert_eq!(policy.matches(rule).count(), 1, "{rule}: {policy}");
        }
        assert_eq!(policy.matches("unix-socket").count(), 1, "{policy}");
        assert_eq!(policy.matches("(allow mach-lookup").count(), 1, "{policy}");
        let denial = policy.find("(deny mach-lookup (xpc-service-name-prefix \"\"))\n");
        let service = policy.find(RESOLVER_SERVICE);
        assert!(
            matches!((denial, service), (Some(denial), Some(service)) if denial < service),
            "{policy}"
        );
    }

    /// Without IP access a launch reaches no resolver, no service and no
    /// Unix socket, and reads no certificate authority files: resolving a
    /// name is itself a way to send data out.
    #[test]
    fn denied_network_reaches_no_resolver_service_or_socket() {
        for network in [
            IpNetworkAccess::Denied,
            IpNetworkAccess::Connect(Vec::new()),
        ] {
            let policy = policy(subtrees(&[]), subtrees(&[]), &[], network);
            for absent in [
                "mDNSResponder",
                "(allow mach-lookup",
                "system-socket",
                "network-outbound",
                "/private/etc/ssl",
            ] {
                assert!(!policy.contains(absent), "{absent}: {policy}");
            }
        }
    }
}
