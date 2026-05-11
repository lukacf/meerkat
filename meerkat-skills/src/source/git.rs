//! Git-backed skill source.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillName,
    SkillQuarantineDiagnostic, SkillScope, SkillSource, SourceHealthSnapshot,
    SourceHealthThresholds, SourceUuid, apply_filter,
};

use crate::parser::parse_skill_md;
use crate::source::remote::{RemoteCache, health_from_cache, load_cached};

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
        }
    }

    fn refresh_if_needed(&self) -> Result<(), SkillError> {
        if self
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
                if let Ok(mut failures) = self.failure_streak.write() {
                    *failures = failures.saturating_add(1);
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
        if self.checkout_dir.join(".git").is_dir() {
            self.run_git(&["fetch", "origin", self.config.git_ref.as_str()])?;
            self.run_git(&["checkout", "FETCH_HEAD"])?;
            return Ok(());
        }

        let auth_url = self.authenticated_url();
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
        args.push(&auth_url);
        let checkout = self.checkout_dir.to_string_lossy().to_string();
        args.push(&checkout);
        self.run_git_at(&args, None)?;
        if matches!(self.config.git_ref, GitRef::Commit(_)) {
            self.run_git(&["checkout", self.config.git_ref.as_str()])?;
        }
        Ok(())
    }

    fn scan_checkout(&self) -> Result<RemoteCache, SkillError> {
        let root = match &self.config.skills_root {
            Some(skills_root) => self.checkout_dir.join(skills_root),
            None => self.checkout_dir.clone(),
        };
        let mut descriptors = Vec::new();
        let mut documents = std::collections::BTreeMap::new();
        let mut quarantined = Vec::new();
        for skill_dir in discover_skill_directories(&root) {
            let key = match skill_key_for_dir(&root, &self.config.source_uuid, &skill_dir) {
                Ok(key) => key,
                Err(err) => {
                    quarantined.push(quarantine(None, skill_dir.display().to_string(), err));
                    continue;
                }
            };
            let content = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
                Ok(content) => content,
                Err(err) => {
                    quarantined.push(quarantine(
                        Some(key),
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
                Some(key.skill_name.leaf_slug()),
            ) {
                Ok(doc) => {
                    descriptors.push(doc.descriptor.clone());
                    documents.insert(doc.descriptor.key.clone(), doc);
                }
                Err(err) => quarantined.push(quarantine(
                    Some(key),
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
            Some(GitSkillAuth::SshKey(path)) => {
                Some(format!("ssh -i {path} -o IdentitiesOnly=yes"))
            }
            _ => None,
        };
        run_command("git", args, cwd, ssh_command.as_deref())
    }

    fn authenticated_url(&self) -> String {
        let Some(GitSkillAuth::HttpsToken(token)) = &self.config.auth else {
            return self.config.repo_url.clone();
        };
        if let Some(rest) = self.config.repo_url.strip_prefix("https://") {
            format!("https://x-access-token:{token}@{rest}")
        } else {
            self.config.repo_url.clone()
        }
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

fn run_command(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    git_ssh_command: Option<&str>,
) -> Result<(), SkillError> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(git_ssh_command) = git_ssh_command {
        command.env("GIT_SSH_COMMAND", git_ssh_command);
    }
    let output = command.output().map_err(|e| {
        SkillError::Load(format!("git skill source command failed to start: {e}").into())
    })?;
    if output.status.success() {
        return Ok(());
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
        if path.is_dir() {
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

fn skill_key_for_dir(
    root: &Path,
    source_uuid: &SourceUuid,
    skill_dir: &Path,
) -> Result<SkillKey, String> {
    let relative = skill_dir
        .strip_prefix(root)
        .map_err(|err| format!("skill path is outside configured root: {err}"))?;
    let mut segments = Vec::new();
    for segment in relative.components() {
        let Some(segment) = segment.as_os_str().to_str() else {
            return Err(format!(
                "skill path is not valid UTF-8: {}",
                skill_dir.display()
            ));
        };
        segments.push(segment);
    }
    let raw = segments.join("/");
    let skill_name = SkillName::parse(&raw).map_err(|err| err.to_string())?;
    Ok(SkillKey::new(source_uuid.clone(), skill_name))
}

fn quarantine(
    key: Option<SkillKey>,
    location: String,
    message: String,
) -> SkillQuarantineDiagnostic {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let identity_hint = key
        .as_ref()
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| location.clone());
    SkillQuarantineDiagnostic {
        key,
        identity_hint,
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

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn skill_key_for_dir_uses_source_relative_path() {
        let root = PathBuf::from("/repo/skills");
        let source_uuid = SourceUuid::parse("a93d587d-8f44-438f-8189-6e8cf549f6e7").unwrap();
        let key = skill_key_for_dir(
            &root,
            &source_uuid,
            &PathBuf::from("/repo/skills/team-a/shared/alpha"),
        )
        .unwrap();

        assert_eq!(key.source_uuid, source_uuid);
        assert_eq!(key.skill_name.as_str(), "team-a/shared/alpha");
    }

    #[test]
    fn invalid_git_skill_path_quarantine_has_no_fabricated_key() {
        let diagnostic = quarantine(
            None,
            "/repo/skills/Bad Skill".to_string(),
            "invalid skill name".to_string(),
        );

        assert_eq!(diagnostic.key, None);
        assert_eq!(diagnostic.identity_hint, "/repo/skills/Bad Skill");
        assert!(!diagnostic.identity_hint.contains("invalid-skill"));
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
