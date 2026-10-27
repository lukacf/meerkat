//! Git-backed skill source.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use meerkat_core::skills::{
    QuarantinedSkillIdentity, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey,
    SkillName, SkillQuarantineDiagnostic, SkillScope, SkillSource, SourceHealthSnapshot,
    SourceHealthThresholds, SourceUuid, apply_filter,
};

use crate::parser::parse_skill_md;
use crate::source::remote::{RemoteCache, health_from_cache, load_cached};

mod credential;

#[derive(Debug, Clone)]
pub enum GitRef {
    Branch(String),
    Tag(String),
    Commit(String),
}

impl GitRef {
    fn as_str(&self) -> &str {
        match self {
            Self::Branch(value) | Self::Tag(value) | Self::Commit(value) => value,
        }
    }
}

#[derive(Clone)]
pub enum GitSkillAuth {
    /// A token for an `https://` repository, which git is given for the
    /// repository's host alone and never on a command line or in a file
    /// (on Unix; on Windows it is a header option for the repository URL on
    /// each git command line, with redirects not followed, never in a
    /// file). Any other transport is never sent it.
    HttpsToken(String),
    SshKey(String),
}

impl std::fmt::Debug for GitSkillAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HttpsToken(_) => f.write_str("HttpsToken(<redacted>)"),
            Self::SshKey(_) => f.write_str("SshKey(<redacted>)"),
        }
    }
}

#[derive(Clone)]
pub struct GitSkillConfig {
    pub repo_url: String,
    pub git_ref: GitRef,
    pub cache_dir: PathBuf,
    pub skills_root: Option<String>,
    pub refresh_interval: Duration,
    pub auth: Option<GitSkillAuth>,
    pub depth: Option<usize>,
    pub source_uuid: SourceUuid,
    pub health_thresholds: SourceHealthThresholds,
}

impl std::fmt::Debug for GitSkillConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitSkillConfig")
            .field("repo_url", &redact_url(&self.repo_url))
            .field("git_ref", &self.git_ref)
            .field("cache_dir", &self.cache_dir)
            .field("skills_root", &self.skills_root)
            .field("refresh_interval", &self.refresh_interval)
            .field("auth", &self.auth)
            .field("depth", &self.depth)
            .field("source_uuid", &self.source_uuid)
            .field("health_thresholds", &self.health_thresholds)
            .finish()
    }
}

pub struct GitSkillSource {
    config: GitSkillConfig,
    checkout_dir: PathBuf,
    cache: Arc<RwLock<RemoteCache>>,
    failure_streak: Arc<RwLock<u32>>,
    // A shared predicate, which a panic cannot leave half-changed, so the
    // source stays `UnwindSafe` and `RefUnwindSafe`.
    untrusted: Option<std::panic::AssertUnwindSafe<crate::UntrustedPaths>>,
}

impl GitSkillSource {
    pub fn new(config: GitSkillConfig) -> Self {
        let checkout_dir = config.cache_dir.join(format!(
            "{}-{}",
            config.source_uuid,
            stable_hash(&format!(
                "{}:{}:{}",
                config.repo_url,
                config.git_ref.as_str(),
                config.skills_root.as_deref().unwrap_or("")
            ))
        ));
        Self {
            config,
            checkout_dir,
            cache: Arc::new(RwLock::new(RemoteCache::default())),
            failure_streak: Arc::new(RwLock::new(0)),
            untrusted: None,
        }
    }

    /// Run `git`, and every program it starts by name (`ssh`, `git-lfs`, a
    /// credential helper), from a `PATH` without relative directories or the
    /// directories `untrusted` names
    /// ([`meerkat_core::HostLaunchExemption::git`]), so none is a program a
    /// confined process put there.
    #[must_use]
    pub fn with_untrusted_paths(mut self, untrusted: crate::UntrustedPaths) -> Self {
        self.untrusted = Some(std::panic::AssertUnwindSafe(untrusted));
        self
    }

    fn refresh_if_needed(&self) -> Result<(), SkillError> {
        // Authority gate: an Unhealthy source (refresh failing past the
        // unhealthy threshold) is no longer authoritative. It must NOT serve
        // interval-fresh-but-stale cache. We still attempt a refresh so the
        // source can recover, but skip the is_fresh fast-path.
        let unhealthy = {
            let streak = self.failure_streak.read().map(|f| *f).unwrap_or_default();
            streak >= self.config.health_thresholds.unhealthy_failure_streak
        };

        if !unhealthy
            && self
                .cache
                .read()
                .map(|cache| cache.is_fresh(self.config.refresh_interval))
                .unwrap_or(false)
        {
            return Ok(());
        }

        match self.refresh_checkout().and_then(|()| self.scan_checkout()) {
            Ok(cache) => {
                if let Ok(mut cached) = self.cache.write() {
                    *cached = cache;
                }
                if let Ok(mut failures) = self.failure_streak.write() {
                    *failures = 0;
                }
                Ok(())
            }
            Err(err) => {
                let streak = if let Ok(mut failures) = self.failure_streak.write() {
                    *failures = failures.saturating_add(1);
                    *failures
                } else {
                    self.failure_streak.read().map(|f| *f).unwrap_or_default()
                };
                if streak >= self.config.health_thresholds.unhealthy_failure_streak {
                    return Err(stale_source_error(
                        &redact_url(&self.config.repo_url),
                        streak,
                        &err,
                    ));
                }
                if self
                    .cache
                    .read()
                    .map(|cache| cache.has_data())
                    .unwrap_or(false)
                {
                    tracing::warn!("using stale git skill cache after refresh failure: {err}");
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    fn refresh_checkout(&self) -> Result<(), SkillError> {
        std::fs::create_dir_all(&self.config.cache_dir).map_err(|e| {
            SkillError::Load(
                format!(
                    "create git skill cache {}: {e}",
                    self.config.cache_dir.display()
                )
                .into(),
            )
        })?;
        // The repository URL and the ref come from configuration; a value
        // that reads as a git option, or carries shell metacharacters, is
        // refused, and every git invocation passes `--end-of-options` before
        // its positionals so a value is never read as one.
        checked_repository_url(&self.config.repo_url)?;
        checked_ref(self.config.git_ref.as_str())?;
        // A local repository inside a confined write grant is one a confined
        // process could have planted: clone runs its upload-pack against
        // that repository's own config.
        if let Some(local) = local_repository_path(&self.config.repo_url)
            && self
                .untrusted
                .as_ref()
                .is_some_and(|untrusted| untrusted.0(&local, true))
        {
            return Err(SkillError::Load(
                "git skill repository refused: a local repository a confined process could change"
                    .into(),
            ));
        }
        if self.checkout_dir.join(".git").is_dir() {
            // A checkout an earlier release cloned holds the HTTPS token in
            // its remote URL (`.git/config`): the configured URL, which
            // holds none, replaces it before git reads it.
            self.run_git(&[
                "remote",
                "set-url",
                "--end-of-options",
                "origin",
                &self.config.repo_url,
            ])?;
            self.run_git(&[
                "fetch",
                "--end-of-options",
                "origin",
                self.config.git_ref.as_str(),
            ])?;
            self.run_git(&["checkout", "--end-of-options", "FETCH_HEAD"])?;
            return Ok(());
        }

        let mut args = vec!["clone"];
        let depth;
        if let Some(value) = self.config.depth {
            depth = value.to_string();
            args.push("--depth");
            args.push(&depth);
        }
        match &self.config.git_ref {
            GitRef::Branch(name) | GitRef::Tag(name) => {
                args.push("--branch");
                args.push(name);
            }
            GitRef::Commit(_) => {}
        }
        args.push("--end-of-options");
        args.push(&self.config.repo_url);
        let checkout = self.checkout_dir.to_string_lossy().to_string();
        args.push(&checkout);
        self.run_git_at(&args, None)?;
        if matches!(self.config.git_ref, GitRef::Commit(_)) {
            self.run_git(&["checkout", "--end-of-options", self.config.git_ref.as_str()])?;
        }
        Ok(())
    }

    fn scan_checkout(&self) -> Result<RemoteCache, SkillError> {
        let root = match &self.config.skills_root {
            Some(skills_root) => self.checkout_dir.join(skills_root),
            None => self.checkout_dir.clone(),
        };
        // The scan root itself may be a link the repository planted; one that
        // leads out of the checkout is not walked (the checkout root leads
        // nowhere, so an unset `skills_root` is always walked).
        if super::leads_out_of(&self.checkout_dir, &root) {
            tracing::warn!(
                dir = %root.display(),
                "skipping Git skills root: it links out of the checkout"
            );
            return Ok(RemoteCache {
                descriptors: Vec::new(),
                documents: std::collections::BTreeMap::new(),
                quarantined: Vec::new(),
                refreshed_at: Some(SystemTime::now()),
            });
        }
        let mut descriptors = Vec::new();
        let mut documents = std::collections::BTreeMap::new();
        let mut quarantined = Vec::new();
        for skill_dir in discover_skill_directories(&root) {
            // Repository content never reaches into the host's filesystem: a
            // skill whose directory or SKILL.md a link leads out of the
            // checkout to is not read, wherever the link leads.
            if super::leads_out_of(&self.checkout_dir, &skill_dir)
                || super::leads_out_of(&self.checkout_dir, &skill_dir.join("SKILL.md"))
            {
                tracing::warn!(
                    dir = %skill_dir.display(),
                    "skipping Git skill: it links out of the checkout"
                );
                continue;
            }
            let Some(file_name) = skill_dir.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let skill_name = match SkillName::parse(file_name) {
                Ok(skill_name) => skill_name,
                Err(err) => {
                    quarantined.push(quarantine(
                        QuarantinedSkillIdentity::new(self.config.source_uuid.clone(), file_name),
                        skill_dir.display().to_string(),
                        err.to_string(),
                    ));
                    continue;
                }
            };
            let key = SkillKey::new(self.config.source_uuid.clone(), skill_name);
            let identity = QuarantinedSkillIdentity::new(
                key.source_uuid.clone(),
                key.skill_name.as_str().to_string(),
            );
            let content = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
                Ok(content) => content,
                Err(err) => {
                    quarantined.push(quarantine(
                        identity,
                        skill_dir.display().to_string(),
                        err.to_string(),
                    ));
                    continue;
                }
            };
            match parse_skill_md(
                key.clone(),
                SkillScope::Project,
                &content,
                Some(key.skill_name.as_str()),
            ) {
                Ok(doc) => {
                    descriptors.push(doc.descriptor.clone());
                    documents.insert(doc.descriptor.key.clone(), doc);
                }
                Err(err) => quarantined.push(quarantine(
                    identity,
                    skill_dir.display().to_string(),
                    err.to_string(),
                )),
            }
        }
        Ok(RemoteCache {
            descriptors,
            documents,
            quarantined,
            refreshed_at: Some(SystemTime::now()),
        })
    }

    fn run_git(&self, args: &[&str]) -> Result<(), SkillError> {
        self.run_git_at(args, Some(&self.checkout_dir))
    }

    fn run_git_at(&self, args: &[&str], cwd: Option<&Path>) -> Result<(), SkillError> {
        let ssh_command = match &self.config.auth {
            Some(GitSkillAuth::SshKey(path)) => Some(git_ssh_command(path)?),
            _ => None,
        };
        let untrusted = self
            .untrusted
            .as_ref()
            .map(|untrusted| untrusted.0.as_ref());
        let https_token = match &self.config.auth {
            Some(GitSkillAuth::HttpsToken(token)) => {
                credential::HttpsToken::for_repository(&self.config.repo_url, token)?
            }
            _ => None,
        };
        // On Unix git asks a socket served for this one invocation for the
        // token; the socket and its directory go when `served` drops, after
        // the invocation. Elsewhere the token is on the command line.
        #[cfg(unix)]
        let served = match &https_token {
            Some(token) => Some(credential::CredentialSocket::serve(
                token,
                &credential::socket_parents(&self.config.cache_dir),
                untrusted,
            )?),
            None => None,
        };
        #[cfg(unix)]
        let socket = served.as_ref().map(credential::CredentialSocket::path);
        #[cfg(not(unix))]
        let socket = None;
        #[cfg(not(unix))]
        if https_token.is_some() {
            warn_command_line_token(&self.config.repo_url);
        }
        run_git_command(
            args,
            cwd,
            ssh_command.as_deref(),
            https_token
                .as_ref()
                .map(|token| (token, credential::TokenRoute::new(socket))),
            untrusted,
        )
    }
}

impl SkillSource for GitSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        self.refresh_if_needed()?;
        let cache = self
            .cache
            .read()
            .map_err(|_| SkillError::Load("git skill cache lock poisoned".into()))?;
        Ok(apply_filter(&cache.descriptors, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key.source_uuid != self.config.source_uuid {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        self.refresh_if_needed()?;
        let cache = self
            .cache
            .read()
            .map_err(|_| SkillError::Load("git skill cache lock poisoned".into()))?;
        load_cached(&cache, key)
    }

    async fn quarantined_diagnostics(&self) -> Result<Vec<SkillQuarantineDiagnostic>, SkillError> {
        self.refresh_if_needed()?;
        Ok(self
            .cache
            .read()
            .map(|cache| cache.quarantined.clone())
            .unwrap_or_default())
    }

    async fn health_snapshot(&self) -> Result<SourceHealthSnapshot, SkillError> {
        let cache = self
            .cache
            .read()
            .map_err(|_| SkillError::Load("git skill cache lock poisoned".into()))?;
        let failures = self.failure_streak.read().map(|f| *f).unwrap_or_default();
        Ok(health_from_cache(
            &cache,
            self.config.health_thresholds,
            failures,
            failures >= self.config.health_thresholds.unhealthy_failure_streak,
        ))
    }
}

/// The `GIT_SSH_COMMAND` for the ssh key at `path`. git runs this string
/// through a shell, so a key path that could carry a shell command is
/// refused rather than run: it must be absolute, an existing regular file,
/// free of control characters and of a single quote (which single-quoting
/// cannot escape), and must not start with `-`. The accepted path is
/// single-quoted, so the shell takes it as one literal argument to `ssh -i`.
fn git_ssh_command(path: &str) -> Result<String, SkillError> {
    let refuse = |why: &str| {
        Err(SkillError::Load(
            format!("git skill ssh key path refused: {why}").into(),
        ))
    };
    if path.starts_with('-') {
        return refuse("it must not start with '-'");
    }
    if path.chars().any(|c| c.is_control() || c == '\'') {
        return refuse("it must not contain control characters or a single quote");
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return refuse("it must be an absolute path");
    }
    if !candidate.is_file() {
        return refuse("it must be an existing regular file");
    }
    Ok(format!("ssh -i '{path}' -o IdentitiesOnly=yes"))
}

/// Refuse a git ref that could be read as an option or carry a shell
/// metacharacter: one that is empty, starts with `-`, or contains
/// whitespace or control characters. Every invocation also passes
/// `--end-of-options` before its refs.
fn checked_ref(value: &str) -> Result<(), SkillError> {
    if value.is_empty() {
        return Err(SkillError::Load(
            "git skill ref refused: it is empty".into(),
        ));
    }
    if value.starts_with('-') {
        return Err(SkillError::Load(
            "git skill ref refused: it must not start with '-'".into(),
        ));
    }
    if value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(SkillError::Load(
            "git skill ref refused: it must not contain whitespace or control characters".into(),
        ));
    }
    Ok(())
}

/// Refuse a repository URL that could be read as an option, carry a shell
/// metacharacter, or name an unsupported transport: it must not start with
/// `-` or contain whitespace or control characters, and its scheme must be
/// one of `ssh`, `https`, `http`, `git` or `file`, a scp-like
/// `[user@]host:path`, or a local absolute path.
fn checked_repository_url(url: &str) -> Result<(), SkillError> {
    let refuse = |why: &str| {
        Err(SkillError::Load(
            format!("git skill repository url refused: {why}").into(),
        ))
    };
    if url.starts_with('-') {
        return refuse("it must not start with '-'");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return refuse("it must not contain whitespace or control characters");
    }
    // `<helper>::<address>` selects a git remote helper (`ext::` runs a
    // command); it is never a supported transport.
    if url.contains("::") {
        return refuse("a remote-helper address (`::`) is not a supported transport");
    }
    let known_scheme = ["ssh://", "https://", "http://", "git://", "file://"]
        .iter()
        .any(|scheme| url.starts_with(scheme));
    // Any other `scheme://` is refused; a scp-like `[user@]host:path` has no
    // `//` after its colon and no `/` before it.
    let other_scheme = !known_scheme && url.contains("://");
    let scp_like = || {
        url.split_once(':').is_some_and(|(host, rest)| {
            !host.is_empty() && !host.contains('/') && !rest.starts_with("//")
        })
    };
    let local_path = Path::new(url).is_absolute();
    if other_scheme || !(known_scheme || local_path || scp_like()) {
        return refuse("its transport is not ssh, https, http, git, file or a local path");
    }
    Ok(())
}

/// The local path a repository URL names (a `file://` URL or an absolute
/// path), for judging it against what a confined process could change.
fn local_repository_path(url: &str) -> Option<PathBuf> {
    let path = url.strip_prefix("file://").unwrap_or(url);
    let path = Path::new(path);
    path.is_absolute().then(|| path.to_path_buf())
}

/// The transports a Git skill invocation may use: `GIT_ALLOW_PROTOCOL`
/// overrides every `protocol.*.allow` setting, so host git config such as
/// `protocol.allow=always` cannot re-enable `ext` or another remote helper.
const GIT_ALLOWED_PROTOCOLS: &str = "ssh:https:http:git:file";

/// git, as a host launch exemption: it, and every program it starts by name,
/// runs as the host from a `PATH` without the directories a confined process
/// could change. Resolution refuses a repository whose skill cache such a
/// process could change.
fn git_command(
    untrusted: Option<&meerkat_core::confinement::UntrustedPathFn>,
) -> std::process::Command {
    meerkat_core::HostLaunchExemption::git(untrusted)
}

fn run_git_command(
    args: &[&str],
    cwd: Option<&Path>,
    git_ssh_command: Option<&str>,
    https_token: Option<(&credential::HttpsToken<'_>, credential::TokenRoute<'_>)>,
    untrusted: Option<&meerkat_core::confinement::UntrustedPathFn>,
) -> Result<(), SkillError> {
    let mut command = git_command(untrusted);
    if let Some((token, route)) = &https_token {
        // Before the subcommand, so no option is written to the checkout.
        for option in token.config_options(route)? {
            command.arg("-c").arg(option);
        }
        // A credential request the token does not answer fails rather than
        // asking anyone: no terminal prompt and no askpass program.
        command.env("GIT_TERMINAL_PROMPT", "0");
        command.env_remove("GIT_ASKPASS");
        command.env_remove("SSH_ASKPASS");
    }
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(git_ssh_command) = git_ssh_command {
        command.env("GIT_SSH_COMMAND", git_ssh_command);
    }
    command.env("GIT_ALLOW_PROTOCOL", GIT_ALLOWED_PROTOCOLS);
    let output = command.output().map_err(|e| {
        SkillError::Load(format!("git skill source command failed to start: {e}").into())
    })?;
    if output.status.success() {
        return Ok(());
    }
    #[cfg(unix)]
    if let Some((_, credential::TokenRoute::Socket(socket))) = https_token
        && !credential_cache_runs(socket, untrusted)
    {
        return Err(SkillError::Load(
            "git skill source: git cannot run its credential-cache helper, which is how an \
             HTTPS token reaches git on this platform (the host serves the token to it for each \
             invocation; it is never on a command line or in a file). Use a git built with \
             credential-cache, an ssh key (auth = { scheme = \"ssh-key\", path = \"...\" }), or \
             a git credential manager configured for the repository host and no token"
                .into(),
        ));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(SkillError::Load(
        format!(
            "git skill source command failed: {}",
            redact_secrets(&stderr)
        )
        .into(),
    ))
}

/// Whether git can run its `credential-cache` client, which a Unix
/// invocation asks for the token. A git built without it fails such an
/// invocation as if the token were wrong; this tells the two apart.
#[cfg(unix)]
fn credential_cache_runs(
    socket: &Path,
    untrusted: Option<&meerkat_core::confinement::UntrustedPathFn>,
) -> bool {
    let mut probe = git_command(untrusted);
    probe
        .arg("credential-cache")
        .arg(format!("--socket={}", socket.display()))
        .arg("get")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    probe.status().is_ok_and(|status| status.success())
}

/// Tell the operator once that on this platform an HTTPS token is on each
/// git command line (never in a file), with the remedies. The token itself
/// is never logged.
#[cfg(not(unix))]
fn warn_command_line_token(repo_url: &str) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            repository = %redact_url(repo_url),
            "a Git skill repository's HTTPS token is passed on each git command line on this \
             platform (as a header for the repository URL alone, with redirects not followed): \
             processes of the same user can read it for that invocation, and git persists it \
             nowhere; use an ssh key, or a git credential manager configured for the \
             repository host, instead of a token"
        );
    });
}

fn discover_skill_directories(base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // A link is a candidate where it is, and the scan judges where it
        // leads; discovery never descends through one, so a link in a
        // repository (to `/`, say) cannot walk the host's filesystem.
        if file_type.is_symlink() {
            if path.join("SKILL.md").is_file() {
                out.push(path);
            }
            continue;
        }
        if file_type.is_dir() {
            if path.join("SKILL.md").is_file() {
                out.push(path);
            } else {
                out.extend(discover_skill_directories(&path));
            }
        }
    }
    out
}

fn stable_hash(input: &str) -> String {
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Typed refusal for an Unhealthy git skill source: refuse to serve stale
/// (but interval-fresh) cache once refresh has failed past the threshold.
fn stale_source_error(location: &str, failure_streak: u32, cause: &SkillError) -> SkillError {
    SkillError::Load(
        format!(
            "stale source: git skill source {location} is unhealthy \
             (failure_streak={failure_streak}); refusing to serve stale cache: {cause}"
        )
        .into(),
    )
}

fn quarantine(
    identity: QuarantinedSkillIdentity,
    location: String,
    message: String,
) -> SkillQuarantineDiagnostic {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    SkillQuarantineDiagnostic {
        identity,
        location,
        error_code: "invalid_git_skill".to_string(),
        error_class: "parse".to_string(),
        message,
        first_seen_unix_secs: now,
        last_seen_unix_secs: now,
    }
}

fn redact_secrets(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| {
            if part.contains("x-access-token:") {
                "<redacted>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_url(url: &str) -> String {
    let redacted = redact_userinfo(url);
    redact_sensitive_query(&redacted)
}

fn redact_userinfo(url: &str) -> String {
    let Some(scheme_pos) = url.find("://") else {
        return url.to_string();
    };
    let authority_start = scheme_pos + 3;
    let authority_end = url[authority_start..]
        .find(['/', '?', '#'])
        .map(|offset| authority_start + offset)
        .unwrap_or(url.len());
    let authority = &url[authority_start..authority_end];
    let Some(userinfo_end) = authority.rfind('@') else {
        return url.to_string();
    };
    format!(
        "{}<redacted>@{}{}",
        &url[..authority_start],
        &authority[userinfo_end + 1..],
        &url[authority_end..]
    )
}

fn redact_sensitive_query(url: &str) -> String {
    let Some((base, query_and_fragment)) = url.split_once('?') else {
        return url.to_string();
    };
    let (query, fragment) = match query_and_fragment.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (query_and_fragment, None),
    };
    let query = query
        .split('&')
        .map(|part| {
            let key = part.split_once('=').map(|(key, _)| key).unwrap_or(part);
            if is_sensitive_query_key(key) {
                format!("{key}=<redacted>")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    match fragment {
        Some(fragment) => format!("{base}?{query}#{fragment}"),
        None => format!("{base}?{query}"),
    }
}

fn is_sensitive_query_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "access_token" | "api_key" | "auth" | "authorization" | "key" | "token"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A skill a Git checkout links to outside the checkout is never read,
    /// wherever the link leads: repository content cannot reach into the
    /// host's filesystem.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_skill_linked_out_of_the_checkout_is_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let outside = base.join("outside").join("linked-skill");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join("SKILL.md"),
            "---\nname: linked-skill\ndescription: Outside the checkout\n---\n\n# Linked",
        )
        .unwrap();
        let source = GitSkillSource::new(GitSkillConfig {
            repo_url: "https://example.invalid/skills.git".into(),
            git_ref: GitRef::Branch("main".into()),
            cache_dir: base.join("cache"),
            skills_root: None,
            refresh_interval: Duration::from_mins(1),
            auth: None,
            depth: None,
            source_uuid: SourceUuid::builtin(),
            health_thresholds: SourceHealthThresholds::default(),
        });
        let inside = source.checkout_dir.join("kept-skill");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(
            inside.join("SKILL.md"),
            "---\nname: kept-skill\ndescription: Inside the checkout\n---\n\n# Kept",
        )
        .unwrap();
        std::os::unix::fs::symlink(&outside, source.checkout_dir.join("linked-skill")).unwrap();

        let cache = source.scan_checkout().unwrap();
        let names: Vec<&str> = cache
            .descriptors
            .iter()
            .map(|descriptor| descriptor.key.skill_name.as_str())
            .collect();
        assert_eq!(names, vec!["kept-skill"]);
    }

    /// The skill sources keep the auto traits they had before they took a
    /// host's judgement or runner.
    #[test]
    fn skill_sources_stay_unwind_safe() {
        fn unwind_safe<T: std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}
        unwind_safe::<GitSkillSource>();
        unwind_safe::<crate::source::FilesystemSkillSource>();
        unwind_safe::<crate::source::StdioExternalClient>();
    }

    /// Discovery never descends through a link, so a link in a repository
    /// (to `/`, say) cannot walk the host's filesystem on each refresh; a
    /// linked skill is judged where it leads by the scan.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used)]
    fn discovery_does_not_descend_through_a_link() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let outside = base.join("outside");
        std::fs::create_dir_all(outside.join("deep")).unwrap();
        std::fs::write(outside.join("deep").join("SKILL.md"), "# Deep").unwrap();
        let checkout = base.join("checkout");
        std::fs::create_dir_all(checkout.join("kept-skill")).unwrap();
        std::fs::write(checkout.join("kept-skill").join("SKILL.md"), "# Kept").unwrap();
        std::os::unix::fs::symlink(&outside, checkout.join("walk")).unwrap();

        let mut found = discover_skill_directories(&checkout);
        found.sort();
        assert_eq!(found, vec![checkout.join("kept-skill")]);
    }

    /// A crafted ssh key path never produces a `GIT_SSH_COMMAND` a shell
    /// could read as more than one `ssh -i <path>` argument; a normal
    /// absolute key file is accepted and single-quoted.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used)]
    fn the_ssh_key_command_refuses_a_path_that_could_carry_a_command() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("id_ed25519");
        std::fs::write(&key, "key").unwrap();
        let key = key.to_string_lossy().to_string();
        for crafted in [
            format!("{key}; touch pwned"),
            format!("{key} -o ProxyCommand=touch pwned"),
            format!("{key}\ntouch pwned"),
            format!("{key}'; touch pwned; '"),
            "-oProxyCommand=touch pwned".to_string(),
            "relative/key".to_string(),
            format!("{}/does-not-exist", temp.path().display()),
        ] {
            assert!(
                git_ssh_command(&crafted).is_err(),
                "{crafted:?} must be refused"
            );
        }
        assert_eq!(
            git_ssh_command(&key).unwrap(),
            format!("ssh -i '{key}' -o IdentitiesOnly=yes")
        );
    }

    /// A real key file whose name contains shell metacharacters is accepted
    /// and single-quoted, so the shell takes the whole path as one literal
    /// argument to `ssh -i` and runs nothing it spells out.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used)]
    fn a_real_metacharacter_key_path_is_quoted_literally() {
        let temp = tempfile::tempdir().unwrap();
        let key = temp.path().join("id; touch marker");
        std::fs::write(&key, "key").unwrap();
        let key = key.to_string_lossy().to_string();
        assert_eq!(
            git_ssh_command(&key).unwrap(),
            format!("ssh -i '{key}' -o IdentitiesOnly=yes")
        );
    }

    /// git takes the HTTPS token from the served socket, for the repository's
    /// host alone, through the options every invocation carries; a credential
    /// helper the host's git configuration names is neither asked for nor sent
    /// it. `git credential fill` (what git-remote-https runs on a 401) gets
    /// the token for the repository's host and nothing for another host or
    /// plain `http`; `git credential approve` (what git runs after the token
    /// authenticated a request) leaves the host's `store` helper's file
    /// without it, though that helper stores what it is sent. Dropping the
    /// served socket removes it and its directory.
    #[cfg(unix)]
    #[test]
    #[allow(clippy::unwrap_used, clippy::expect_used)]
    fn git_takes_the_token_from_the_served_socket_for_the_repository_host_only() {
        use std::io::Write;
        use std::process::Stdio;

        const TOKEN: &str = "unit-socket-token-5c1e";
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let stored = base.join("stored-credentials");
        let host_config = base.join("host-gitconfig");
        std::fs::write(
            &host_config,
            format!(
                "[credential]\n\thelper = store --file={}\n",
                stored.display()
            ),
        )
        .unwrap();
        let token = credential::HttpsToken::for_repository(
            "https://git.example.com:8443/org/skills.git",
            TOKEN,
        )
        .unwrap()
        .expect("an https repository takes the token");
        // Served where the source serves it: under the system temporary
        // directory first. The canonical tempdir is longer (on macOS by
        // `/private` and its own level) than a socket address may be.
        let served =
            credential::CredentialSocket::serve(&token, &credential::socket_parents(&base), None)
                .unwrap();
        let options = token
            .config_options(&credential::TokenRoute::new(Some(served.path())))
            .unwrap();
        let credential = |options: &[String], action: &str, request: &str| {
            let mut command = git_command(None);
            for option in options {
                command.arg("-c").arg(option);
            }
            command
                .args(["credential", action])
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", &host_config)
                .env("GIT_TERMINAL_PROMPT", "0")
                .env_remove("GIT_ASKPASS")
                .env_remove("SSH_ASKPASS")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(request.as_bytes())
                .unwrap();
            child.wait_with_output().unwrap()
        };

        let filled = credential(
            &options,
            "fill",
            "protocol=https\nhost=git.example.com:8443\npath=org/skills.git\n\n",
        );
        let answer = String::from_utf8_lossy(&filled.stdout);
        assert!(
            filled.status.success()
                && answer.contains("username=x-access-token\n")
                && answer.contains(&format!("password={TOKEN}\n")),
            "git takes the token for the repository's host: {answer} {}",
            String::from_utf8_lossy(&filled.stderr)
        );
        for (protocol, host) in [
            ("https", "elsewhere.example"),
            ("http", "git.example.com:8443"),
        ] {
            let refused = credential(
                &options,
                "fill",
                &format!("protocol={protocol}\nhost={host}\n\n"),
            );
            assert!(
                !refused.status.success()
                    && !String::from_utf8_lossy(&refused.stdout).contains(TOKEN),
                "{protocol}://{host} gets no token and no prompt"
            );
        }

        let approve = format!(
            "protocol=https\nhost=git.example.com:8443\nusername=x-access-token\npassword={TOKEN}\n\n"
        );
        assert!(credential(&options, "approve", &approve).status.success());
        assert!(
            !stored.exists() || !std::fs::read_to_string(&stored).unwrap().contains(TOKEN),
            "the host's helper is never sent the token"
        );

        let directory = served.path().parent().unwrap().to_path_buf();
        drop(served);
        assert!(!directory.exists(), "the socket's directory goes with it");

        // The control: without the options, the host's helper is sent and
        // stores what git approves.
        assert!(credential(&[], "approve", &approve).status.success());
        assert!(std::fs::read_to_string(&stored).unwrap().contains(TOKEN));
    }

    /// Only the supported transports resolve a local path; a `file://` URL or
    /// an absolute path is the one judged against the write grants.
    #[test]
    fn local_repository_path_names_only_file_urls_and_absolute_paths() {
        assert_eq!(
            local_repository_path("file:///srv/r"),
            Some(std::path::PathBuf::from("/srv/r"))
        );
        assert_eq!(
            local_repository_path("/srv/r"),
            Some(std::path::PathBuf::from("/srv/r"))
        );
        assert_eq!(local_repository_path("https://h/r.git"), None);
        assert_eq!(local_repository_path("git@h:owner/r.git"), None);
    }

    /// A ref or URL that reads as an option, or carries whitespace or a
    /// control character, is refused; an ordinary one is accepted.
    #[test]
    fn a_ref_or_url_that_looks_like_an_option_is_refused() {
        for bad in ["", "-x", "--upload-pack=sh", "a b", "a\tb", "a\0b"] {
            assert!(checked_ref(bad).is_err(), "ref {bad:?} must be refused");
        }
        for good in ["main", "v1.2.3", "release/9.0", "0123abcd"] {
            assert!(checked_ref(good).is_ok(), "ref {good:?} must be allowed");
        }
        for bad in [
            "-x",
            "--upload-pack=sh",
            "https://h/ r",
            "./rel",
            "a\tb",
            "ext::sh -c touch% /tmp/x",
            "ext::/path/in/grant/evil",
            "fd::3",
            "foo::x",
            "ftp://example.com/r.git",
            "s3://bucket/r",
        ] {
            assert!(
                checked_repository_url(bad).is_err(),
                "url {bad:?} must be refused"
            );
        }
        for good in [
            "https://example.com/r.git",
            "ssh://git@h/r",
            "git@h:owner/r.git",
            "file:///srv/r",
            "/srv/local/r",
        ] {
            assert!(
                checked_repository_url(good).is_ok(),
                "url {good:?} must be allowed"
            );
        }
    }

    #[test]
    fn redact_url_hides_userinfo_and_sensitive_query_values() {
        assert_eq!(
            redact_url("https://x-access-token:secret@example.com/repo.git?token=secret&ref=main"),
            "https://<redacted>@example.com/repo.git?token=<redacted>&ref=main"
        );
        assert_eq!(
            redact_url("https://example.com/repo.git?email=user@example.com&access_token=secret"),
            "https://example.com/repo.git?email=user@example.com&access_token=<redacted>"
        );
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
pub mod tests_support {
    use std::path::Path;
    use std::process::Command;

    pub async fn init_test_repo(repo: &Path, work: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(repo)?;
        std::fs::create_dir_all(work)?;
        run_git(repo, &["init", "--bare"])?;
        run_git(work, &["init"])?;
        run_git(work, &["config", "user.email", "skills@example.com"])?;
        run_git(work, &["config", "user.name", "Skills Test"])?;
        run_git(work, &["add", "."])?;
        run_git(work, &["commit", "-m", "initial"])?;
        run_git(work, &["branch", "-M", "main"])?;
        run_git(
            work,
            &["remote", "add", "origin", repo.to_string_lossy().as_ref()],
        )?;
        run_git(work, &["push", "-u", "origin", "main"])?;
        Ok(())
    }

    fn run_git(cwd: &Path, args: &[&str]) -> std::io::Result<()> {
        let status = Command::new("git").args(args).current_dir(cwd).status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("git {args:?} failed")))
        }
    }
}
