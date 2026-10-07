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

/// Same-origin redirects a skills fetch follows at most.
const MAX_SAME_ORIGIN_REDIRECTS: usize = 3;

/// Whether two URLs share an origin: scheme, host and port all equal.
fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// Every fetch carries the source's credential (Bearer or a custom header),
/// so the client follows only same-origin redirects: the credential never
/// leaves the configured origin and never downgrades to plain http.
fn same_origin_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            // A revisited URL stops at once, so the final answer is the 3xx
            // and the caller sees that the call was redirected.
            let follow = attempt.previous().len() <= MAX_SAME_ORIGIN_REDIRECTS
                && !attempt.previous().contains(attempt.url())
                && attempt
                    .previous()
                    .last()
                    .is_some_and(|previous| same_origin(previous, attempt.url()));
            if follow {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|_| "the redirect-limited HTTP client could not be built".to_owned())
}

pub struct HttpSkillSource {
    source_uuid: SourceUuid,
    url: String,
    auth: Option<HttpSkillAuth>,
    refresh_interval: Duration,
    request_timeout: Duration,
    thresholds: SourceHealthThresholds,
    /// Follows only same-origin redirects; a build failure is kept and every
    /// fetch fails with it.
    client: Result<reqwest::Client, String>,
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
            client: same_origin_http_client(),
            cache: Arc::new(RwLock::new(RemoteCache::default())),
            failure_streak: Arc::new(RwLock::new(0)),
        }
    }

    async fn refresh_if_needed(&self) -> Result<(), SkillError> {
        // Authority gate: an Unhealthy source (refresh failing past the
        // unhealthy threshold) is no longer authoritative. It must NOT serve
        // interval-fresh-but-stale cache — health is the authority, not the
        // freshness timer. We still attempt a refresh so the source can
        // recover, but skip the is_fresh fast-path that would otherwise serve
        // stale cache without re-validating.
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
                let streak = if let Ok(mut failures) = self.failure_streak.write() {
                    *failures = failures.saturating_add(1);
                    *failures
                } else {
                    self.failure_streak.read().map(|f| *f).unwrap_or_default()
                };
                // Authority gate: once refresh has failed past the unhealthy
                // threshold the source is no longer authoritative, so we refuse
                // to serve interval-fresh-but-stale cache with a typed stale
                // error rather than laundering it as a successful load.
                if streak >= self.thresholds.unhealthy_failure_streak {
                    return Err(stale_source_error(&redacted_url(&self.url), streak, &err));
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
        let client = self.client.as_ref().map_err(|error| {
            SkillError::Load(format!("HTTP skill source client unavailable: {error}").into())
        })?;
        let mut request = client.get(url).timeout(self.request_timeout);
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
        // A redirect the configured policy did not follow (for example
        // another origin, the hop limit, or a missing or invalid `Location`)
        // is refused by its status; its `Location` is not read.
        if response.status().is_redirection() {
            return Err(SkillError::Load(
                format!(
                    "HTTP skill source {} answered with a redirect (status {}) that the configured redirect policy does not follow; refused",
                    redacted_url(url),
                    response.status().as_u16()
                )
                .into(),
            ));
        }
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

/// Typed refusal for an Unhealthy remote source: rather than serving stale
/// (but interval-fresh) cache, the source declares itself non-authoritative.
fn stale_source_error(location: &str, failure_streak: u32, cause: &SkillError) -> SkillError {
    SkillError::Load(
        format!(
            "stale source: HTTP skill source {location} is unhealthy \
             (failure_streak={failure_streak}); refusing to serve stale cache: {cause}"
        )
        .into(),
    )
}

fn redacted_url(url: &str) -> String {
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

    #[test]
    fn redacted_url_hides_userinfo_and_sensitive_query_values() {
        assert_eq!(
            redacted_url("https://user:secret@example.com/catalog?api_key=secret&safe=1#fragment"),
            "https://<redacted>@example.com/catalog?api_key=<redacted>&safe=1#fragment"
        );
        assert_eq!(
            redacted_url("https://example.com/catalog?email=user@example.com&token=secret"),
            "https://example.com/catalog?email=user@example.com&token=<redacted>"
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn same_origin_compares_scheme_host_and_port() {
        let url = |raw: &str| reqwest::Url::parse(raw).unwrap();
        assert!(same_origin(
            &url("https://h.example/a"),
            &url("https://h.example:443/b")
        ));
        assert!(!same_origin(
            &url("https://h.example:8443/a"),
            &url("http://h.example:8443/a")
        ));
        assert!(!same_origin(
            &url("https://h.example/a"),
            &url("https://cdn.example/a")
        ));
    }

    fn source(url: String, auth: HttpSkillAuth) -> HttpSkillSource {
        HttpSkillSource::new_with_thresholds(
            SourceUuid::builtin(),
            url,
            Some(auth),
            Duration::from_secs(60),
            Duration::from_secs(5),
            SourceHealthThresholds::default(),
        )
    }

    /// A CDN redirect to another origin is refused: the source's custom
    /// header never reaches the other host, and nothing is rendered from the
    /// `Location`.
    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn cross_origin_redirect_is_refused_without_following_it() {
        use wiremock::matchers::any;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let target = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&target)
            .await;
        let origin = MockServer::start().await;
        Mock::given(any())
            .respond_with(ResponseTemplate::new(302).insert_header(
                "location",
                format!("{}/signed?leak=redirect-location-canary", target.uri()).as_str(),
            ))
            .mount(&origin)
            .await;
        let source = source(
            origin.uri(),
            HttpSkillAuth::Header {
                name: "x-skills-key".into(),
                value: "skills-header-canary".into(),
            },
        );
        let error = source
            .fetch_url(&format!("{}/skills", origin.uri()))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("redirect"), "{error}");
        assert!(!error.contains("redirect-location-canary"), "{error}");
        assert!(target.received_requests().await.unwrap().is_empty());
    }

    /// A same-origin redirect (for example a trailing slash) is followed.
    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn same_origin_redirect_is_followed() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let origin = MockServer::start().await;
        Mock::given(path("/skills"))
            .respond_with(ResponseTemplate::new(307).insert_header("location", "/skills/"))
            .mount(&origin)
            .await;
        Mock::given(path("/skills/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("listed"))
            .mount(&origin)
            .await;
        let source = source(origin.uri(), HttpSkillAuth::Bearer("skills-bearer".into()));
        let body = source
            .fetch_url(&format!("{}/skills", origin.uri()))
            .await
            .unwrap();
        assert_eq!(body, "listed");
    }
}
