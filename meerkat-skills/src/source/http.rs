//! HTTP skill source.
//!
//! Implements `SkillSource` by calling a remote API using the Meerkat Skills
//! API wire format. Includes TTL-based caching.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use meerkat_core::skills::{
    SkillCollection, SkillDescriptor, SkillDocument, SkillError, SkillFilter, SkillId, SkillSource,
    apply_filter,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Authentication for HTTP skill sources.
#[derive(Debug, Clone)]
pub enum HttpSkillAuth {
    /// Bearer token: sends `Authorization: Bearer {token}`.
    Bearer(String),
    /// Custom header: sends `{name}: {value}` (e.g. X-API-Key).
    Header { name: String, value: String },
}

/// HTTP skill source.
///
/// Fetches skills from a remote API using the Meerkat Skills API format.
/// Includes TTL-based caching to minimize HTTP requests.
pub struct HttpSkillSource {
    /// API root URL (e.g. `http://localhost:8080/api`).
    base_url: String,
    client: Client,
    auth: Option<HttpSkillAuth>,
    cache: Arc<RwLock<SkillCache>>,
    cache_ttl: Duration,
}

struct SkillCache {
    /// Full unfiltered descriptor list. Always fetched via GET /skills (no params).
    descriptors: Option<(Vec<SkillDescriptor>, Instant)>,
    /// Per-ID document cache.
    documents: std::collections::HashMap<SkillId, (SkillDocument, Instant)>,
}

impl SkillCache {
    fn new() -> Self {
        Self {
            descriptors: None,
            documents: std::collections::HashMap::new(),
        }
    }
}

/// Wire format for the list skills response.
#[derive(Debug, Deserialize, Serialize)]
struct ListSkillsResponse {
    skills: Vec<WireSkillDescriptor>,
}

/// Wire format for a single skill descriptor.
#[derive(Debug, Deserialize, Serialize)]
struct WireSkillDescriptor {
    id: String,
    name: String,
    description: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
}

/// Wire format for a skill document (with body).
#[derive(Debug, Deserialize, Serialize)]
struct WireSkillDocument {
    id: String,
    name: String,
    description: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    metadata: std::collections::HashMap<String, String>,
    body: String,
}

/// Wire format for the list collections response.
#[derive(Debug, Deserialize, Serialize)]
struct ListCollectionsResponse {
    collections: Vec<SkillCollection>,
}

impl HttpSkillSource {
    /// Create a new HTTP skill source.
    ///
    /// `base_url` is the API root (e.g. `http://localhost:8080/api`).
    /// Endpoints are appended: `/skills`, `/skills/{id}`, `/skill-collections`.
    pub fn new(
        base_url: impl Into<String>,
        auth: Option<HttpSkillAuth>,
        cache_ttl: Duration,
    ) -> Self {
        let mut base = base_url.into();
        // Ensure no trailing slash
        while base.ends_with('/') {
            base.pop();
        }
        Self {
            base_url: base,
            client: Client::new(),
            auth,
            cache: Arc::new(RwLock::new(SkillCache::new())),
            cache_ttl,
        }
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            Some(HttpSkillAuth::Bearer(token)) => {
                req.header("Authorization", format!("Bearer {token}"))
            }
            Some(HttpSkillAuth::Header { name, value }) => req.header(name, value),
            None => req,
        }
    }

    /// Fetch the full unfiltered skill list from the server.
    /// Cache refresh rule: always fetch unfiltered.
    async fn fetch_all_descriptors(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        let url = format!("{}/skills", self.base_url);
        let req = self.apply_auth(self.client.get(&url));
        let resp = req.send().await.map_err(|e| {
            SkillError::Load(format!("HTTP request to {url} failed: {e}").into())
        })?;

        if !resp.status().is_success() {
            return Err(SkillError::Load(
                format!("HTTP {}: {url}", resp.status()).into(),
            ));
        }

        let body: ListSkillsResponse = resp.json().await.map_err(|e| {
            SkillError::Load(format!("Failed to parse skills response: {e}").into())
        })?;

        Ok(body
            .skills
            .into_iter()
            .map(|w| {
                let metadata = w
                    .metadata
                    .into_iter()
                    .collect::<indexmap::IndexMap<_, _>>();
                SkillDescriptor {
                    id: SkillId(w.id),
                    name: w.name,
                    description: w.description,
                    metadata,
                    ..Default::default()
                }
            })
            .collect())
    }

    /// Get descriptors, using cache if fresh.
    async fn get_descriptors(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        {
            let cache = self.cache.read().await;
            if let Some((ref descs, fetched_at)) = cache.descriptors {
                if fetched_at.elapsed() < self.cache_ttl {
                    return Ok(descs.clone());
                }
            }
        }

        // Cache miss or expired — fetch fresh
        let descs = self.fetch_all_descriptors().await?;
        {
            let mut cache = self.cache.write().await;
            cache.descriptors = Some((descs.clone(), Instant::now()));
        }
        Ok(descs)
    }
}

#[async_trait]
impl SkillSource for HttpSkillSource {
    async fn list(&self, filter: &SkillFilter) -> Result<Vec<SkillDescriptor>, SkillError> {
        let all = self.get_descriptors().await?;
        Ok(apply_filter(&all, filter))
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError> {
        // Check per-ID cache
        {
            let cache = self.cache.read().await;
            if let Some((doc, fetched_at)) = cache.documents.get(id) {
                if fetched_at.elapsed() < self.cache_ttl {
                    return Ok(doc.clone());
                }
            }
        }

        // Cache miss — fetch from server
        let encoded_id = urlencoding::encode(&id.0);
        let url = format!("{}/skills/{encoded_id}", self.base_url);
        let req = self.apply_auth(self.client.get(&url));
        let resp = req.send().await.map_err(|e| {
            SkillError::Load(format!("HTTP request to {url} failed: {e}").into())
        })?;

        if !resp.status().is_success() {
            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(SkillError::NotFound { id: id.clone() });
            }
            return Err(SkillError::Load(
                format!("HTTP {}: {url}", resp.status()).into(),
            ));
        }

        let wire: WireSkillDocument = resp.json().await.map_err(|e| {
            SkillError::Load(format!("Failed to parse skill document: {e}").into())
        })?;

        let metadata = wire
            .metadata
            .into_iter()
            .collect::<indexmap::IndexMap<_, _>>();
        let doc = SkillDocument {
            descriptor: SkillDescriptor {
                id: id.clone(),
                name: wire.name,
                description: wire.description,
                metadata,
                ..Default::default()
            },
            body: wire.body,
            extensions: indexmap::IndexMap::new(),
        };

        // Cache the document
        {
            let mut cache = self.cache.write().await;
            cache.documents.insert(id.clone(), (doc.clone(), Instant::now()));
        }

        Ok(doc)
    }

    async fn collections(&self) -> Result<Vec<SkillCollection>, SkillError> {
        let url = format!("{}/skill-collections", self.base_url);
        let req = self.apply_auth(self.client.get(&url));
        let resp = req.send().await.map_err(|e| {
            SkillError::Load(format!("HTTP request to {url} failed: {e}").into())
        })?;

        if !resp.status().is_success() {
            // Fall back to deriving from descriptors
            let all = self.get_descriptors().await?;
            return Ok(meerkat_core::skills::derive_collections(&all));
        }

        let body: ListCollectionsResponse = resp.json().await.map_err(|e| {
            SkillError::Load(format!("Failed to parse collections response: {e}").into())
        })?;

        Ok(body.collections)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path, header};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn setup_list_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/skills"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "skills": [
                    {
                        "id": "extraction/email",
                        "name": "email",
                        "description": "Extract from emails",
                        "scope": "project",
                        "metadata": {}
                    },
                    {
                        "id": "formatting/markdown",
                        "name": "markdown",
                        "description": "Markdown output",
                        "scope": "project",
                        "metadata": {}
                    }
                ]
            })))
            .mount(server)
            .await;
    }

    async fn setup_load_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/skills/extraction%2Femail"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "extraction/email",
                "name": "email",
                "description": "Extract from emails",
                "scope": "project",
                "metadata": {},
                "body": "# Email Extractor\n\nExtract entities."
            })))
            .mount(server)
            .await;
    }

    async fn setup_collections_mock(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/skill-collections"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "collections": [
                    {
                        "path": "extraction",
                        "description": "Entity extraction",
                        "count": 2
                    }
                ]
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn test_list_skills_from_http() {
        let server = MockServer::start().await;
        setup_list_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));
        let skills = source.list(&SkillFilter::default()).await.unwrap();

        assert_eq!(skills.len(), 2);
        assert!(skills.iter().any(|s| s.id.0 == "extraction/email"));
        assert!(skills.iter().any(|s| s.id.0 == "formatting/markdown"));
    }

    #[tokio::test]
    async fn test_load_skill_from_http() {
        let server = MockServer::start().await;
        setup_load_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));
        let doc = source
            .load(&SkillId("extraction/email".into()))
            .await
            .unwrap();

        assert_eq!(doc.descriptor.name, "email");
        assert!(doc.body.contains("Email Extractor"));
    }

    #[tokio::test]
    async fn test_list_collections_from_http() {
        let server = MockServer::start().await;
        setup_collections_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));
        let collections = source.collections().await.unwrap();

        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].path, "extraction");
        assert_eq!(collections[0].count, 2);
    }

    #[tokio::test]
    async fn test_cache_serves_on_second_call() {
        let server = MockServer::start().await;
        setup_list_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        // First call — hits server
        let skills1 = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills1.len(), 2);

        // Second call — should serve from cache (mock only responds once by default
        // but wiremock allows multiple, so we verify by checking the data is same)
        let skills2 = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills2.len(), 2);
    }

    #[tokio::test]
    async fn test_cache_expires_after_ttl() {
        let server = MockServer::start().await;
        setup_list_mock(&server).await;

        // Very short TTL
        let source = HttpSkillSource::new(
            server.uri(),
            None,
            Duration::from_millis(50),
        );

        let _ = source.list(&SkillFilter::default()).await.unwrap();

        // Wait for TTL to expire
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Should re-fetch (cache expired)
        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(skills.len(), 2);
    }

    #[tokio::test]
    async fn test_cache_refresh_always_unfiltered() {
        let server = MockServer::start().await;
        setup_list_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        // Filtered call should still cache the full unfiltered list
        let filtered = source
            .list(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1); // Only extraction/email

        // Unfiltered call should hit cache (not server again)
        let all = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2); // Both skills from cache
    }

    #[tokio::test]
    async fn test_auth_bearer_header() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "skills": []
            })))
            .mount(&server)
            .await;

        let source = HttpSkillSource::new(
            server.uri(),
            Some(HttpSkillAuth::Bearer("test-token".into())),
            Duration::from_secs(60),
        );

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_auth_custom_header() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .and(header("X-API-Key", "my-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "skills": []
            })))
            .mount(&server)
            .await;

        let source = HttpSkillSource::new(
            server.uri(),
            Some(HttpSkillAuth::Header {
                name: "X-API-Key".into(),
                value: "my-secret".into(),
            }),
            Duration::from_secs(60),
        );

        let skills = source.list(&SkillFilter::default()).await.unwrap();
        assert!(skills.is_empty());
    }

    #[tokio::test]
    async fn test_url_encoding_slash_ids() {
        let server = MockServer::start().await;
        setup_load_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        // The mock is set up for path "/skills/extraction%2Femail"
        // This verifies that the slash in the ID is URL-encoded
        let doc = source
            .load(&SkillId("extraction/email".into()))
            .await
            .unwrap();
        assert_eq!(doc.descriptor.name, "email");
    }

    #[tokio::test]
    async fn test_collection_filter_applied_client_side() {
        let server = MockServer::start().await;
        setup_list_mock(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        let filtered = source
            .list(&SkillFilter {
                collection: Some("extraction".into()),
                ..Default::default()
            })
            .await
            .unwrap();

        // Should only return extraction/* skills (client-side filtering)
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id.0, "extraction/email");
    }
}
