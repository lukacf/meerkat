//! External skill source protocol over stdio.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillQuarantineDiagnostic,
    SkillScope, SkillSource, SourceHealthSnapshot, SourceHealthThresholds, SourceUuid,
};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::source::remote::{
    RemoteCache, cache_from_catalog, filter_cached, health_from_cache, load_cached,
    parse_remote_catalog, parse_remote_document,
};

/// Client that speaks the external skill-source protocol over stdio.
pub struct StdioExternalClient {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    timeout: Duration,
}

impl StdioExternalClient {
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            cwd,
            timeout: Duration::from_secs(15),
        }
    }

    pub fn new_with_timeout(
        command: impl Into<String>,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
        timeout: Duration,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            cwd,
            timeout,
        }
    }

    async fn request(&self, request: &StdioRequest<'_>) -> Result<String, SkillError> {
        let payload = serde_json::to_vec(request)
            .map_err(|e| SkillError::Load(format!("stdio request encode failed: {e}").into()))?;
        let mut command = Command::new(&self.command);
        command.args(&self.args).envs(&self.env);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command.kill_on_drop(true);
        let mut child = command.spawn().map_err(|e| {
            SkillError::Load(format!("stdio skill source spawn failed: {e}").into())
        })?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SkillError::Load("stdio skill source stdout pipe unavailable".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SkillError::Load("stdio skill source stderr pipe unavailable".into()))?;
        let stdout_task = tokio::spawn(async move {
            let mut output = String::new();
            stdout
                .read_to_string(&mut output)
                .await
                .map(|_| output)
                .map_err(|e| {
                    SkillError::Load(format!("stdio skill source read failed: {e}").into())
                })
        });
        let stderr_task = tokio::spawn(async move {
            let mut output = String::new();
            stderr
                .read_to_string(&mut output)
                .await
                .map(|_| output)
                .map_err(|e| {
                    SkillError::Load(format!("stdio skill source stderr read failed: {e}").into())
                })
        });

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(&payload).await {
                let _ = child.kill().await;
                return Err(SkillError::Load(
                    format!("stdio skill source write failed: {e}").into(),
                ));
            }
            if let Err(e) = stdin.write_all(b"\n").await {
                let _ = child.kill().await;
                return Err(SkillError::Load(
                    format!("stdio skill source write failed: {e}").into(),
                ));
            }
        }

        let status = tokio::select! {
            status = child.wait() => status.map_err(|e| {
                SkillError::Load(format!("stdio skill source wait failed: {e}").into())
            })?,
            () = tokio::time::sleep(self.timeout) => {
                let _ = child.kill().await;
                let _ = stdout_task.await;
                let _ = stderr_task.await;
                return Err(SkillError::Load("stdio skill source timed out".into()));
            }
        };

        let stdout = stdout_task.await.map_err(|e| {
            SkillError::Load(format!("stdio stdout task join failed: {e}").into())
        })??;
        let stderr = stderr_task.await.map_err(|e| {
            SkillError::Load(format!("stdio stderr task join failed: {e}").into())
        })??;
        if !status.success() {
            return Err(SkillError::Load(
                format!("stdio skill source exited with {status}: {stderr}").into(),
            ));
        }
        Ok(stdout)
    }
}

/// Skill source backed by an external protocol client.
pub struct ExternalSkillSource<C> {
    client: C,
    source_uuid: SourceUuid,
    refresh_interval: Duration,
    thresholds: SourceHealthThresholds,
    cache: Arc<RwLock<RemoteCache>>,
    failure_streak: Arc<RwLock<u32>>,
}

impl<C> ExternalSkillSource<C> {
    pub fn new(client: C) -> Self {
        Self::new_with_source_uuid(
            client,
            SourceUuid::builtin(),
            Duration::from_secs(300),
            SourceHealthThresholds::default(),
        )
    }

    pub fn new_with_source_uuid(
        client: C,
        source_uuid: SourceUuid,
        refresh_interval: Duration,
        thresholds: SourceHealthThresholds,
    ) -> Self {
        Self {
            client,
            source_uuid,
            refresh_interval,
            thresholds,
            cache: Arc::new(RwLock::new(RemoteCache::default())),
            failure_streak: Arc::new(RwLock::new(0)),
        }
    }
}

impl ExternalSkillSource<StdioExternalClient> {
    async fn refresh_if_needed(&self) -> Result<(), SkillError> {
        // Authority gate: an Unhealthy source (refresh failing past the
        // unhealthy threshold) is no longer authoritative. It must NOT serve
        // interval-fresh-but-stale cache. We still attempt a refresh so the
        // source can recover, but skip the is_fresh fast-path.
        let unhealthy = {
            let streak = self.failure_streak.read().map(|f| *f).unwrap_or_default();
            streak >= self.thresholds.unhealthy_failure_streak
        };

        if !unhealthy
            && self
                .cache
                .read()
                .map(|cache| cache.is_fresh(self.refresh_interval))
                .unwrap_or(false)
        {
            return Ok(());
        }

        let request = StdioRequest {
            method: "skills/list",
            source_uuid: self.source_uuid.to_string(),
            skill_name: None,
        };
        match self.client.request(&request).await.and_then(|raw| {
            parse_remote_catalog(
                &raw,
                &self.source_uuid,
                SkillScope::Project,
                &self.client.command,
            )
        }) {
            Ok(parsed) => {
                if let Ok(mut cache) = self.cache.write() {
                    *cache = cache_from_catalog(parsed);
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
                if streak >= self.thresholds.unhealthy_failure_streak {
                    return Err(stale_source_error(&self.client.command, streak, &err));
                }
                if self
                    .cache
                    .read()
                    .map(|cache| cache.has_data())
                    .unwrap_or(false)
                {
                    tracing::warn!("using stale stdio skill cache after refresh failure: {err}");
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    async fn load_remote_document(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        let request = StdioRequest {
            method: "skills/load",
            source_uuid: self.source_uuid.to_string(),
            skill_name: Some(key.skill_name.as_str()),
        };
        let raw = self.client.request(&request).await?;
        parse_remote_document(&raw, key, SkillScope::Project)
    }
}

impl SkillSource for ExternalSkillSource<StdioExternalClient> {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        self.refresh_if_needed().await?;
        let cache = self
            .cache
            .read()
            .map_err(|_| SkillError::Load("stdio skill cache lock poisoned".into()))?;
        Ok(filter_cached(&cache, filter))
    }

    async fn load(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        if key.source_uuid != self.source_uuid {
            return Err(SkillError::NotFound { key: key.clone() });
        }
        self.refresh_if_needed().await?;
        if let Ok(cache) = self.cache.read()
            && let Ok(doc) = load_cached(&cache, key)
        {
            return Ok(doc);
        }
        let doc = self.load_remote_document(key).await?;
        if let Ok(mut cache) = self.cache.write() {
            cache.documents.insert(key.clone(), doc.clone());
            if !cache.descriptors.iter().any(|desc| desc.key == *key) {
                cache.descriptors.push(doc.descriptor.clone());
            }
        }
        Ok(doc)
    }

    async fn quarantined_diagnostics(&self) -> Result<Vec<SkillQuarantineDiagnostic>, SkillError> {
        self.refresh_if_needed().await?;
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
            .map_err(|_| SkillError::Load("stdio skill cache lock poisoned".into()))?;
        let failures = self.failure_streak.read().map(|f| *f).unwrap_or_default();
        Ok(health_from_cache(
            &cache,
            self.thresholds,
            failures,
            failures >= self.thresholds.unhealthy_failure_streak,
        ))
    }
}

/// Typed refusal for an Unhealthy stdio skill source: refuse to serve stale
/// (but interval-fresh) cache once refresh has failed past the threshold.
fn stale_source_error(location: &str, failure_streak: u32, cause: &SkillError) -> SkillError {
    SkillError::Load(
        format!(
            "stale source: stdio skill source {location} is unhealthy \
             (failure_streak={failure_streak}); refusing to serve stale cache: {cause}"
        )
        .into(),
    )
}

#[derive(Serialize)]
struct StdioRequest<'a> {
    method: &'a str,
    source_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_name: Option<&'a str>,
}
