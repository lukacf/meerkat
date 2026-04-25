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
        let mut command = Command::new(&self.command);
        command.args(&self.args).envs(&self.env);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().map_err(|e| {
            SkillError::Load(format!("stdio skill source spawn failed: {e}").into())
        })?;

        let payload = serde_json::to_vec(request)
            .map_err(|e| SkillError::Load(format!("stdio request encode failed: {e}").into()))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&payload).await.map_err(|e| {
                SkillError::Load(format!("stdio skill source write failed: {e}").into())
            })?;
            stdin.write_all(b"\n").await.map_err(|e| {
                SkillError::Load(format!("stdio skill source write failed: {e}").into())
            })?;
        }

        let read_output = async {
            let mut stdout = String::new();
            if let Some(mut child_stdout) = child.stdout.take() {
                child_stdout
                    .read_to_string(&mut stdout)
                    .await
                    .map_err(|e| {
                        SkillError::Load(format!("stdio skill source read failed: {e}").into())
                    })?;
            }
            let status = child.wait().await.map_err(|e| {
                SkillError::Load(format!("stdio skill source wait failed: {e}").into())
            })?;
            if !status.success() {
                return Err(SkillError::Load(
                    format!("stdio skill source exited with {status}").into(),
                ));
            }
            Ok(stdout)
        };

        tokio::time::timeout(self.timeout, read_output)
            .await
            .map_err(|_| SkillError::Load("stdio skill source timed out".into()))?
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
        if self
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
                if let Ok(mut failures) = self.failure_streak.write() {
                    *failures = failures.saturating_add(1);
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

#[derive(Serialize)]
struct StdioRequest<'a> {
    method: &'a str,
    source_uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_name: Option<&'a str>,
}
