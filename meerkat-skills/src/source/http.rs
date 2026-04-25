//! HTTP skill source.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillKey, SkillQuarantineDiagnostic,
    SkillScope, SkillSource, SourceHealthSnapshot, SourceHealthThresholds, SourceUuid,
};

use crate::source::remote::{
    RemoteCache, cache_from_catalog, filter_cached, health_from_cache, load_cached,
    parse_remote_catalog, parse_remote_document,
};

#[derive(Clone)]
pub enum HttpSkillAuth {
    Bearer(String),
    Header { name: String, value: String },
}

impl std::fmt::Debug for HttpSkillAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bearer(_) => f.write_str("Bearer(<redacted>)"),
            Self::Header { name, .. } => f
                .debug_struct("Header")
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
        }
    }
}

pub struct HttpSkillSource {
    source_uuid: SourceUuid,
    url: String,
    auth: Option<HttpSkillAuth>,
    refresh_interval: Duration,
    request_timeout: Duration,
    thresholds: SourceHealthThresholds,
    client: reqwest::Client,
    cache: Arc<RwLock<RemoteCache>>,
    failure_streak: Arc<RwLock<u32>>,
}

impl HttpSkillSource {
    pub fn new_with_source_uuid(
        source_uuid: SourceUuid,
        url: String,
        auth: Option<HttpSkillAuth>,
        refresh_interval: Duration,
        request_timeout: Duration,
    ) -> Self {
        Self::new_with_thresholds(
            source_uuid,
            url,
            auth,
            refresh_interval,
            request_timeout,
            SourceHealthThresholds::default(),
        )
    }

    pub fn new_with_thresholds(
        source_uuid: SourceUuid,
        url: String,
        auth: Option<HttpSkillAuth>,
        refresh_interval: Duration,
        request_timeout: Duration,
        thresholds: SourceHealthThresholds,
    ) -> Self {
        Self {
            source_uuid,
            url,
            auth,
            refresh_interval,
            request_timeout,
            thresholds,
            client: reqwest::Client::new(),
            cache: Arc::new(RwLock::new(RemoteCache::default())),
            failure_streak: Arc::new(RwLock::new(0)),
        }
    }

    async fn refresh_if_needed(&self) -> Result<(), SkillError> {
        if self
            .cache
            .read()
            .map(|cache| cache.is_fresh(self.refresh_interval))
            .unwrap_or(false)
        {
            return Ok(());
        }

        match self.fetch_catalog().await.and_then(|raw| {
            parse_remote_catalog(
                &raw,
                &self.source_uuid,
                SkillScope::Project,
                redacted_url(&self.url).as_str(),
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
                    tracing::warn!("using stale HTTP skill cache after refresh failure: {err}");
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    async fn fetch_catalog(&self) -> Result<String, SkillError> {
        self.fetch_url(&self.url).await
    }

    async fn fetch_skill(&self, key: &SkillKey) -> Result<SkillDocument, SkillError> {
        let url = format!(
            "{}/skills/{}",
            self.url.trim_end_matches('/'),
            urlencoding::encode(key.skill_name.as_str())
        );
        let raw = self.fetch_url(&url).await?;
        parse_remote_document(&raw, key, SkillScope::Project)
    }

    async fn fetch_url(&self, url: &str) -> Result<String, SkillError> {
        let mut request = self.client.get(url).timeout(self.request_timeout);
        if let Some(auth) = &self.auth {
            request = match auth {
                HttpSkillAuth::Bearer(token) => request.bearer_auth(token),
                HttpSkillAuth::Header { name, value } => request.header(name.as_str(), value),
            };
        }
        let response = request.send().await.map_err(|e| {
            SkillError::Load(
                format!(
                    "HTTP skill source {} request failed: {}",
                    redacted_url(url),
                    e.without_url()
                )
                .into(),
            )
        })?;
        if !response.status().is_success() {
            return Err(SkillError::Load(
                format!(
                    "HTTP skill source {} returned {}",
                    redacted_url(url),
                    response.status()
                )
                .into(),
            ));
        }
        response.text().await.map_err(|e| {
            SkillError::Load(format!("HTTP skill source body read failed: {e}").into())
        })
    }
}

impl SkillSource for HttpSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        self.refresh_if_needed().await?;
        let cache = self
            .cache
            .read()
            .map_err(|_| SkillError::Load("HTTP skill cache lock poisoned".into()))?;
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
        let doc = self.fetch_skill(key).await?;
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
            .map_err(|_| SkillError::Load("HTTP skill cache lock poisoned".into()))?;
        let failures = self.failure_streak.read().map(|f| *f).unwrap_or_default();
        Ok(health_from_cache(
            &cache,
            self.thresholds,
            failures,
            failures >= self.thresholds.unhealthy_failure_streak,
        ))
    }
}

fn redacted_url(url: &str) -> String {
    match url.split_once('@') {
        Some((scheme_and_user, rest)) if scheme_and_user.contains("://") => {
            let scheme = scheme_and_user.split("://").next().unwrap_or("https");
            format!("{scheme}://<redacted>@{rest}")
        }
        _ => url.to_string(),
    }
}
