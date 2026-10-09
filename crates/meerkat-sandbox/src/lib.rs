//! OS lowering and exact native spawn. Authorization and process-group
//! containment remain with their existing runtime owners.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;

pub use meerkat_core::confinement::{ConfinementRefusal, ExecutionConfinement};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod native;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native_child;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod native_stdio;
#[cfg(target_os = "macos")]
mod seatbelt;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use native_child::{NativeChild, SpawnIo, StdioMode};
#[cfg(not(target_arch = "wasm32"))]
mod child;
#[cfg(not(target_arch = "wasm32"))]
pub use child::ProcessChild;

/// Exact host-prepared launch data. Environment values are never inherited.
pub struct ProcessLaunchSpec {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    program: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    arguments: Vec<OsString>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    directory: PathBuf,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    environment: BTreeMap<OsString, OsString>,
    // Preserve an opaque validated launch when no backend can retain its data.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    _unsupported: (),
}

impl ProcessLaunchSpec {
    pub fn new(
        program: PathBuf,
        arguments: Vec<OsString>,
        directory: PathBuf,
        environment: BTreeMap<OsString, OsString>,
    ) -> Result<Self, ConfinementRefusal> {
        if !program.is_absolute()
            || !directory.is_absolute()
            || contains_nul(program.as_os_str())
            || contains_nul(directory.as_os_str())
            || arguments.iter().any(|value| contains_nul(value))
            || environment.iter().any(|(key, value)| {
                contains_nul(key)
                    || contains_nul(value)
                    || key.is_empty()
                    || key.as_encoded_bytes().contains(&b'=')
            })
        {
            return Err(ConfinementRefusal::InvalidLaunch);
        }
        // The immutable OS executor runs before the target under policy. It
        // cannot accept loader/startup injection.
        for key in environment.keys() {
            let key = key.as_encoded_bytes();
            if [
                b"DYLD_".as_slice(),
                b"LD_",
                b"_RLD_",
                b"__XPC_",
                b"BASH_FUNC_",
            ]
            .iter()
            .any(|prefix| key.starts_with(prefix))
                || [
                    b"ENV".as_slice(),
                    b"BASH_ENV",
                    b"SHELLOPTS",
                    b"BASHOPTS",
                    b"ZDOTDIR",
                    b"GLIBC_TUNABLES",
                ]
                .contains(&key)
            {
                return Err(ConfinementRefusal::UnsupportedRequirement);
            }
        }
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Ok(Self {
                program,
                arguments,
                directory,
                environment,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = (program, arguments, directory, environment);
            Ok(Self { _unsupported: () })
        }
    }
}

impl std::fmt::Debug for ProcessLaunchSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProcessLaunchSpec([REDACTED])")
    }
}

fn contains_nul(value: &OsStr) -> bool {
    value.as_encoded_bytes().contains(&0)
}

/// Prepared exec chain. Fields cannot be retargeted after preparation.
pub struct PreparedConfinement {
    #[cfg(target_os = "macos")]
    executor: PathBuf,
    #[cfg(target_os = "macos")]
    arguments: Vec<OsString>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    launch: ProcessLaunchSpec,
    #[cfg(target_os = "linux")]
    policy: std::sync::Arc<linux::LinuxPolicy>,
    // Compilation and binding always refuse when no supported backend exists.
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    _unsupported: std::convert::Infallible,
}

impl std::fmt::Debug for PreparedConfinement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PreparedConfinement([REDACTED])")
    }
}

impl PreparedConfinement {
    /// Spawn the immutable OS confinement executor with this exact bound launch.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn spawn(self) -> std::io::Result<NativeChild> {
        self.spawn_with_io(SpawnIo::default())
    }

    /// Spawn with explicit host stdio handling. The operation owner retains
    /// exclusive ownership of the resulting direct child.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn spawn_with_io(self, streams: SpawnIo) -> std::io::Result<NativeChild> {
        #[cfg(target_os = "linux")]
        {
            linux::spawn(self.policy, self.launch, streams, None)
        }
        #[cfg(target_os = "macos")]
        {
            native::spawn(
                &self.executor,
                &self.arguments,
                &self.launch.directory,
                &self.launch.environment,
                streams,
                None,
            )
        }
    }

    /// Spawn behind a host-owned custody release pipe. The fixed gate reads
    /// one release token, closes descriptor 3, then enters the bound executor
    /// without changing PID or process group. EOF or a different token exits
    /// with code 125 before any target code runs.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn spawn_behind_gate(
        self,
        reader: std::os::fd::BorrowedFd<'_>,
        release_token: &OsStr,
        streams: SpawnIo,
    ) -> std::io::Result<NativeChild> {
        if release_token.is_empty()
            || release_token
                .as_encoded_bytes()
                .iter()
                .any(|byte| matches!(byte, 0 | b'\n' | b'\r'))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid custody release token",
            ));
        }
        #[cfg(target_os = "linux")]
        {
            linux::spawn(
                self.policy,
                self.launch,
                streams,
                Some((reader, release_token)),
            )
        }
        #[cfg(target_os = "macos")]
        {
            // Fixed host code only; target arguments remain positional data.
            const PROLOGUE: &str = "IFS= read -r meerkat_custody_gate <&3 || exit 125; case $meerkat_custody_gate in \"$1\") ;; *) exit 125 ;; esac; exec 3<&-; shift; exec \"$@\"";
            let mut arguments = vec![
                OsString::from("-c"),
                OsString::from(PROLOGUE),
                OsString::from("meerkat-custody-gate"),
                release_token.to_owned(),
                self.executor.as_os_str().to_owned(),
            ];
            arguments.extend(self.arguments);
            native::spawn(
                Path::new("/bin/sh"),
                &arguments,
                &self.launch.directory,
                &self.launch.environment,
                streams,
                Some(reader),
            )
        }
    }

    /// Spawn and collect output using null stdin and piped stdout/stderr.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub async fn output(self) -> std::io::Result<std::process::Output> {
        self.spawn()?.wait_with_output().await
    }
}

/// The backend profile which enforces this exact compiled requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfinementBackend {
    MacOsSeatbeltV1,
    LinuxNamespaceSeccompV1,
}

/// Mechanical setup support, not permission or evidence of a launched child.
/// Every requested dimension must be supported; partial support is an error.
pub struct ConfinementCapabilityReport {
    backend: ConfinementBackend,
    requirement: ExecutionConfinement,
}

impl ConfinementCapabilityReport {
    #[must_use]
    pub fn backend(&self) -> ConfinementBackend {
        self.backend
    }

    #[must_use]
    pub fn requirement(&self) -> &ExecutionConfinement {
        &self.requirement
    }
}

impl std::fmt::Debug for ConfinementCapabilityReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ConfinementCapabilityReport([REDACTED])")
    }
}

/// Immutable policy compilation retained by the existing host configuration.
/// Replacing that configuration requires a new compilation. No policy lookup,
/// generation registry, or authority is stored here.
pub struct CompiledConfinement {
    capabilities: ConfinementCapabilityReport,
    #[cfg(target_os = "macos")]
    executor: TrustedExecutable,
    #[cfg(target_os = "macos")]
    policy: OsString,
    #[cfg(target_os = "macos")]
    aliases: Vec<(&'static str, &'static str)>,
    #[cfg(target_os = "linux")]
    policy: std::sync::Arc<linux::LinuxPolicy>,
}

impl std::fmt::Debug for CompiledConfinement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompiledConfinement([REDACTED])")
    }
}

impl CompiledConfinement {
    /// Compile once from trusted host configuration. This verifies structural
    /// support and the selected installed executables, not future OS success.
    /// Ordinary host processes and the initial executable bytes are trusted.
    pub fn compile(requirement: &ExecutionConfinement) -> Result<Self, ConfinementRefusal> {
        #[cfg(target_os = "macos")]
        {
            seatbelt::validate_support(requirement)?;
            let executor = TrustedExecutable::capture(Path::new("/usr/bin/sandbox-exec"))?;
            let policy = seatbelt::lower(requirement, &[&executor.path])?;
            let aliases = seatbelt::used_system_aliases(requirement);
            Ok(Self {
                capabilities: ConfinementCapabilityReport {
                    backend: ConfinementBackend::MacOsSeatbeltV1,
                    requirement: requirement.clone(),
                },
                executor,
                policy: policy.into(),
                aliases,
            })
        }
        #[cfg(target_os = "linux")]
        {
            let policy = linux::LinuxPolicy::compile(requirement)?;
            Ok(Self {
                capabilities: ConfinementCapabilityReport {
                    backend: ConfinementBackend::LinuxNamespaceSeccompV1,
                    requirement: requirement.clone(),
                },
                policy,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = requirement;
            Err(ConfinementRefusal::UnsupportedRequirement)
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> &ConfinementCapabilityReport {
        &self.capabilities
    }

    /// Bind exact launch data without lowering policy again or hashing files.
    /// Metadata checks retain the selected installation across delayed launches.
    /// The caller must pass this preparation unchanged through its custody gate.
    pub fn bind_launch(
        &self,
        launch: ProcessLaunchSpec,
    ) -> Result<PreparedConfinement, ConfinementRefusal> {
        #[cfg(target_os = "macos")]
        {
            self.executor.validate()?;
            seatbelt::validate_system_aliases(&self.aliases)?;
            let mut arguments = vec![
                OsString::from("-p"),
                self.policy.clone(),
                OsString::from("--"),
                launch.program.as_os_str().to_owned(),
            ];
            arguments.extend(launch.arguments.iter().cloned());
            Ok(PreparedConfinement {
                executor: self.executor.path.clone(),
                arguments,
                launch,
            })
        }
        #[cfg(target_os = "linux")]
        {
            self.policy.validate_launch(&launch)?;
            Ok(PreparedConfinement {
                launch,
                policy: self.policy.clone(),
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let ProcessLaunchSpec { _unsupported: () } = launch;
            Err(ConfinementRefusal::UnsupportedRequirement)
        }
    }
}

/// Compile and bind one launch. Repeated launches should retain a compiled
/// requirement in their existing immutable configuration owner.
pub fn prepare(
    requirement: &ExecutionConfinement,
    launch: ProcessLaunchSpec,
) -> Result<PreparedConfinement, ConfinementRefusal> {
    CompiledConfinement::compile(requirement)?.bind_launch(launch)
}

#[cfg(target_os = "macos")]
#[derive(PartialEq, Eq)]
struct ExecutableIdentity {
    device: u64,
    inode: u64,
    length: u64,
    mode: u32,
    owner: u32,
    group: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

#[cfg(target_os = "macos")]
impl ExecutableIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, ConfinementRefusal> {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o111 == 0 {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            mode: metadata.mode(),
            owner: metadata.uid(),
            group: metadata.gid(),
            links: metadata.nlink(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[cfg(target_os = "macos")]
struct TrustedExecutable {
    path: PathBuf,
    file: std::fs::File,
    identity: ExecutableIdentity,
    ancestors: Vec<(PathBuf, u64, u64)>,
}

#[cfg(target_os = "macos")]
impl TrustedExecutable {
    fn capture(requested: &Path) -> Result<Self, ConfinementRefusal> {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        if !requested.is_absolute()
            || contains_nul(requested.as_os_str())
            || requested
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ConfinementRefusal::BackendUnavailable);
        }
        let path = seatbelt::normalize(requested)?;
        // Nonblocking open ensures a substituted FIFO cannot stall setup.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
        let identity = ExecutableIdentity::from_metadata(
            &file
                .metadata()
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?,
        )?;
        let mut ancestors = Vec::new();
        for ancestor in path.ancestors().skip(1) {
            let metadata = std::fs::symlink_metadata(ancestor)
                .map_err(|_| ConfinementRefusal::BackendUnavailable)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(ConfinementRefusal::BackendUnavailable);
            }
            ancestors.push((ancestor.to_owned(), metadata.dev(), metadata.ino()));
        }
        let executable = Self {
            path,
            file,
            identity,
            ancestors,
        };
        executable.validate()?;
        Ok(executable)
    }

    fn validate(&self) -> Result<(), ConfinementRefusal> {
        use std::os::unix::fs::MetadataExt;
        let unavailable = || ConfinementRefusal::BackendUnavailable;
        let opened = self.file.metadata().map_err(|_| unavailable())?;
        let named = std::fs::symlink_metadata(&self.path).map_err(|_| unavailable())?;
        if named.file_type().is_symlink()
            || ExecutableIdentity::from_metadata(&opened)? != self.identity
            || ExecutableIdentity::from_metadata(&named)? != self.identity
        {
            return Err(unavailable());
        }
        for (path, device, inode) in &self.ancestors {
            let metadata = std::fs::symlink_metadata(path).map_err(|_| unavailable())?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.dev() != *device
                || metadata.ino() != *inode
            {
                return Err(unavailable());
            }
        }
        Ok(())
    }
}
