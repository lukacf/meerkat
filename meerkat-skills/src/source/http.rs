//! HTTP skill source via typed external protocol transport.

use std::future::Future;
use std::time::{Duration, Instant};

use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillCollection, SkillDescriptor, SkillDocument,
    SkillError, SkillFilter, SkillId, SkillSource, derive_collections,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::protocol::{
    ExternalArtifactContent, ExternalClient, ExternalSkillArtifact, ExternalSkillPackage,
    ExternalSkillSource, ExternalSkillSummary, HttpSourceEnvelope, SourceCapabilityHandshake,
    SourceProtocolRequest, SourceProtocolResponse,
};

/// Authentication for HTTP skill sources.
#[derive(Debug, Clone)]
pub enum HttpSkillAuth {
    /// Bearer token: sends `Authorization: Bearer {token}`.
    Bearer(String),
    /// Custom header: sends `{name}: {value}` (e.g. X-API-Key).
    Header { name: String, value: String },
}

/// HTTP protocol client implementing the external source contract.
pub struct HttpExternalClient {
    base_url: String,
    client: Client,
    auth: Option<HttpSkillAuth>,
    request_timeout: Duration,
}

impl HttpExternalClient {
    pub fn new(
        source_uuid: impl Into<String>,
        base_url: impl Into<String>,
        auth: Option<HttpSkillAuth>,
    ) -> Self {
        Self::new_with_timeout(source_uuid, base_url, auth, Duration::from_secs(15))
    }

    pub fn new_with_timeout(
        _source_uuid: impl Into<String>,
        base_url: impl Into<String>,
        auth: Option<HttpSkillAuth>,
        request_timeout: Duration,
    ) -> Self {
        let mut base = base_url.into();
        while base.ends_with('/') {
            base.pop();
        }

        Self {
            base_url: base,
            client: Client::builder()
                .timeout(request_timeout)
                .build()
                .unwrap_or_else(|_| Client::new()),
            auth,
            request_timeout,
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

    async fn send_json(
        &self,
        request: SourceProtocolRequest,
    ) -> Result<serde_json::Value, SkillError> {
        let url = format!("{}{}", self.base_url, endpoint_for(&request));

        let request_builder = match &request {
            SourceProtocolRequest::SkillsInvokeFunction {
                source_uuid,
                skill_name,
                function_name,
                arguments,
            } => {
                let invoke_url = format!(
                    "{}/skills/{}/functions/{}",
                    self.base_url,
                    urlencoding::encode(&format_skill_path(source_uuid, skill_name)),
                    urlencoding::encode(function_name)
                );
                self.apply_auth(self.client.post(invoke_url).json(arguments))
            }
            _ => self.apply_auth(self.client.get(&url)),
        };

        let response = tokio::time::timeout(self.request_timeout, request_builder.send())
            .await
            .map_err(|_| {
                SkillError::Load(
                    format!(
                        "HTTP request for {} timed out after {:?}",
                        request.method_name(),
                        self.request_timeout
                    )
                    .into(),
                )
            })?
            .map_err(|e| {
                SkillError::Load(
                    format!("HTTP request for {} failed: {e}", request.method_name()).into(),
                )
            })?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SkillError::NotFound {
                id: SkillId("__external_source__".to_string()),
            });
        }

        if !response.status().is_success() {
            return Err(SkillError::Load(
                format!(
                    "HTTP {} for {} at {}",
                    response.status(),
                    request.method_name(),
                    url
                )
                .into(),
            ));
        }

        tokio::time::timeout(self.request_timeout, response.json::<serde_json::Value>())
            .await
            .map_err(|_| {
                SkillError::Load(
                    format!(
                        "HTTP decode for {} timed out after {:?}",
                        request.method_name(),
                        self.request_timeout
                    )
                    .into(),
                )
            })?
            .map_err(|e| {
                SkillError::Load(
                    format!(
                        "failed to decode HTTP payload for {}: {e}",
                        request.method_name()
                    )
                    .into(),
                )
            })
    }

    fn try_http_envelope(
        value: serde_json::Value,
    ) -> Result<SourceProtocolResponse, serde_json::Error> {
        let envelope: HttpSourceEnvelope<SourceProtocolResponse> = serde_json::from_value(value)?;
        Ok(envelope.payload)
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct LegacyCollectionsResponse {
    collections: Vec<SkillCollection>,
}

#[allow(clippy::manual_async_fn)]
impl ExternalClient for HttpExternalClient {
    fn capabilities_get(
        &self,
        min_protocol_version: u16,
    ) -> impl Future<Output = Result<SourceCapabilityHandshake, SkillError>> + Send {
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::CapabilitiesGet {
                    min_protocol_version,
                })
                .await?;
            match decode_protocol_response(payload, "capabilities/get")? {
                SourceProtocolResponse::CapabilitiesGet(handshake) => Ok(handshake),
                other => Err(unexpected_protocol_variant("capabilities/get", &other)),
            }
        }
    }

    fn skills_list_summaries(
        &self,
    ) -> impl Future<Output = Result<Vec<ExternalSkillSummary>, SkillError>> + Send {
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::SkillsListSummaries)
                .await?;
            match decode_protocol_response(payload, "skills/list_summaries")? {
                SourceProtocolResponse::SkillsListSummaries { summaries } => Ok(summaries),
                other => Err(unexpected_protocol_variant("skills/list_summaries", &other)),
            }
        }
    }

    fn skills_load_package(
        &self,
        source_uuid: &str,
        skill_name: &str,
    ) -> impl Future<Output = Result<ExternalSkillPackage, SkillError>> + Send {
        let source_uuid = source_uuid.to_string();
        let skill_name = skill_name.to_string();
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::SkillsLoadPackage {
                    source_uuid: source_uuid.clone(),
                    skill_name,
                })
                .await?;
            match decode_protocol_response(payload, "skills/load_package")? {
                SourceProtocolResponse::SkillsLoadPackage { package } => Ok(package),
                other => Err(unexpected_protocol_variant("skills/load_package", &other)),
            }
        }
    }

    fn skills_list_artifacts(
        &self,
        source_uuid: &str,
        skill_name: &str,
    ) -> impl Future<Output = Result<Vec<ExternalSkillArtifact>, SkillError>> + Send {
        let source_uuid = source_uuid.to_string();
        let skill_name = skill_name.to_string();
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::SkillsListArtifacts {
                    source_uuid,
                    skill_name,
                })
                .await?;
            match decode_protocol_response(payload, "skills/list_artifacts")? {
                SourceProtocolResponse::SkillsListArtifacts { artifacts } => Ok(artifacts),
                other => Err(unexpected_protocol_variant("skills/list_artifacts", &other)),
            }
        }
    }

    fn skills_read_artifact(
        &self,
        source_uuid: &str,
        skill_name: &str,
        artifact_path: &str,
    ) -> impl Future<Output = Result<ExternalArtifactContent, SkillError>> + Send {
        let source_uuid = source_uuid.to_string();
        let skill_name = skill_name.to_string();
        let artifact_path = artifact_path.to_string();
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::SkillsReadArtifact {
                    source_uuid,
                    skill_name,
                    artifact_path,
                })
                .await?;
            match decode_protocol_response(payload, "skills/read_artifact")? {
                SourceProtocolResponse::SkillsReadArtifact { artifact } => Ok(artifact),
                other => Err(unexpected_protocol_variant("skills/read_artifact", &other)),
            }
        }
    }

    fn skills_invoke_function(
        &self,
        source_uuid: &str,
        skill_name: &str,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        let source_uuid = source_uuid.to_string();
        let skill_name = skill_name.to_string();
        let function_name = function_name.to_string();
        async move {
            let payload = self
                .send_json(SourceProtocolRequest::SkillsInvokeFunction {
                    source_uuid,
                    skill_name,
                    function_name,
                    arguments,
                })
                .await?;
            match decode_protocol_response(payload, "skills/invoke_function")? {
                SourceProtocolResponse::SkillsInvokeFunction { output } => Ok(output),
                other => Err(unexpected_protocol_variant(
                    "skills/invoke_function",
                    &other,
                )),
            }
        }
    }
}

fn decode_protocol_response(
    payload: serde_json::Value,
    method: &str,
) -> Result<SourceProtocolResponse, SkillError> {
    if let Ok(protocol) = serde_json::from_value::<SourceProtocolResponse>(payload.clone()) {
        return Ok(protocol);
    }
    if let Ok(protocol) = HttpExternalClient::try_http_envelope(payload) {
        return Ok(protocol);
    }
    Err(SkillError::Load(
        format!("invalid typed protocol payload for {method}").into(),
    ))
}

fn unexpected_protocol_variant(method: &str, response: &SourceProtocolResponse) -> SkillError {
    SkillError::Load(
        format!(
            "unexpected protocol response variant for {method}: {}",
            endpoint_for_response(response)
        )
        .into(),
    )
}

fn endpoint_for_response(response: &SourceProtocolResponse) -> &'static str {
    match response {
        SourceProtocolResponse::CapabilitiesGet(_) => "capabilities/get",
        SourceProtocolResponse::SkillsListSummaries { .. } => "skills/list_summaries",
        SourceProtocolResponse::SkillsLoadPackage { .. } => "skills/load_package",
        SourceProtocolResponse::SkillsListArtifacts { .. } => "skills/list_artifacts",
        SourceProtocolResponse::SkillsReadArtifact { .. } => "skills/read_artifact",
        SourceProtocolResponse::SkillsInvokeFunction { .. } => "skills/invoke_function",
    }
}

/// HTTP-backed skill source with descriptor/document caching.
pub struct HttpSkillSource {
    client: HttpExternalClient,
    external: ExternalSkillSource<HttpExternalClient>,
    cache_ttl: Duration,
    cache: RwLock<HttpSkillCache>,
}

struct HttpSkillCache {
    descriptors: Option<(Vec<SkillDescriptor>, Instant)>,
    documents: std::collections::HashMap<SkillId, (SkillDocument, Instant)>,
}

impl HttpSkillCache {
    fn new() -> Self {
        Self {
            descriptors: None,
            documents: std::collections::HashMap::new(),
        }
    }
}

impl HttpSkillSource {
    pub fn new(
        base_url: impl Into<String>,
        auth: Option<HttpSkillAuth>,
        cache_ttl: Duration,
    ) -> Self {
        Self::new_with_source_uuid("legacy", base_url, auth, cache_ttl, Duration::from_secs(15))
    }

    pub fn new_with_source_uuid(
        source_uuid: impl Into<String>,
        base_url: impl Into<String>,
        auth: Option<HttpSkillAuth>,
        cache_ttl: Duration,
        request_timeout: Duration,
    ) -> Self {
        let source_uuid = source_uuid.into();
        let mut base_url = base_url.into();
        while base_url.ends_with('/') {
            base_url.pop();
        }
        let client = HttpExternalClient::new_with_timeout(
            source_uuid.clone(),
            base_url.clone(),
            auth.clone(),
            request_timeout,
        );

        Self {
            external: ExternalSkillSource::new(
                source_uuid.clone(),
                HttpExternalClient::new_with_timeout(source_uuid, base_url, auth, request_timeout),
            ),
            client,
            cache_ttl,
            cache: RwLock::new(HttpSkillCache::new()),
        }
    }

    async fn fetch_descriptors(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        self.external.list(&SkillFilter::default()).await
    }

    async fn get_descriptors(&self) -> Result<Vec<SkillDescriptor>, SkillError> {
        {
            let cache = self.cache.read().await;
            if let Some((descriptors, fetched_at)) = &cache.descriptors
                && fetched_at.elapsed() < self.cache_ttl
            {
                return Ok(descriptors.clone());
            }
        }

        let descriptors = self.fetch_descriptors().await?;
        let mut cache = self.cache.write().await;
        cache.descriptors = Some((descriptors.clone(), Instant::now()));
        Ok(descriptors)
    }

    pub async fn list_artifacts(&self, id: &SkillId) -> Result<Vec<SkillArtifact>, SkillError> {
        self.external.list_artifacts(id).await
    }

    pub async fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        self.external.read_artifact(id, artifact_path).await
    }

    pub async fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        self.external
            .invoke_function(id, function_name, arguments)
            .await
    }
}

#[allow(clippy::manual_async_fn)]
impl SkillSource for HttpSkillSource {
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move {
            let all = self.get_descriptors().await?;
            Ok(meerkat_core::skills::apply_filter(&all, filter))
        }
    }

    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move {
            {
                let cache = self.cache.read().await;
                if let Some((doc, fetched_at)) = cache.documents.get(id)
                    && fetched_at.elapsed() < self.cache_ttl
                {
                    return Ok(doc.clone());
                }
            }

            let loaded = self.external.load(id).await?;
            let mut cache = self.cache.write().await;
            cache
                .documents
                .insert(id.clone(), (loaded.clone(), Instant::now()));
            Ok(loaded)
        }
    }

    fn collections(&self) -> impl Future<Output = Result<Vec<SkillCollection>, SkillError>> + Send {
        async move {
            let url = format!("{}/skill-collections", self.client.base_url);
            let req = self.client.apply_auth(self.client.client.get(&url));
            let response = req.send().await.map_err(|e| {
                SkillError::Load(format!("HTTP request to {url} failed: {e}").into())
            })?;

            if response.status().is_success() {
                let parsed = response
                    .json::<LegacyCollectionsResponse>()
                    .await
                    .map_err(|e| {
                        SkillError::Load(
                            format!("failed to parse legacy collections response: {e}").into(),
                        )
                    })?;
                return Ok(parsed.collections);
            }

            let all = self.get_descriptors().await?;
            Ok(derive_collections(&all))
        }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<meerkat_core::skills::SourceHealthSnapshot, SkillError>> + Send
    {
        async move { self.external.health_snapshot().await }
    }

    fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move { self.external.list_artifacts(id).await }
    }

    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { self.external.read_artifact(id, artifact_path).await }
    }

    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move {
            self.external
                .invoke_function(id, function_name, arguments)
                .await
        }
    }
}

fn endpoint_for(request: &SourceProtocolRequest) -> String {
    match request {
        SourceProtocolRequest::CapabilitiesGet { .. } => "/capabilities".to_string(),
        SourceProtocolRequest::SkillsListSummaries => "/skills".to_string(),
        SourceProtocolRequest::SkillsLoadPackage {
            source_uuid,
            skill_name,
        } => format!(
            "/skills/{}",
            urlencoding::encode(&format_skill_path(source_uuid, skill_name))
        ),
        SourceProtocolRequest::SkillsListArtifacts {
            source_uuid,
            skill_name,
        } => format!(
            "/skills/{}/artifacts",
            urlencoding::encode(&format_skill_path(source_uuid, skill_name))
        ),
        SourceProtocolRequest::SkillsReadArtifact {
            source_uuid,
            skill_name,
            artifact_path,
        } => format!(
            "/skills/{}/artifacts/{}",
            urlencoding::encode(&format_skill_path(source_uuid, skill_name)),
            urlencoding::encode(artifact_path)
        ),
        SourceProtocolRequest::SkillsInvokeFunction {
            source_uuid,
            skill_name,
            function_name,
            ..
        } => format!(
            "/skills/{}/functions/{}",
            urlencoding::encode(&format_skill_path(source_uuid, skill_name)),
            urlencoding::encode(function_name)
        ),
    }
}

fn format_skill_path(source_uuid: &str, skill_name: &str) -> String {
    if source_uuid == "legacy" {
        skill_name.to_string()
    } else {
        format!("{source_uuid}/{skill_name}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn capabilities_body() -> serde_json::Value {
        protocol_body(SourceProtocolResponse::CapabilitiesGet(
            SourceCapabilityHandshake {
                protocol_version: 1,
                methods: vec![
                    "skills/list_summaries".to_string(),
                    "skills/load_package".to_string(),
                    "skills/list_artifacts".to_string(),
                    "skills/read_artifact".to_string(),
                    "skills/invoke_function".to_string(),
                ],
            },
        ))
    }

    fn protocol_body(response: SourceProtocolResponse) -> serde_json::Value {
        match serde_json::to_value(response) {
            Ok(value) => value,
            Err(err) => panic!("protocol response should serialize: {err}"),
        }
    }

    async fn setup_protocol_mocks(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(capabilities_body()))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsListSummaries {
                    summaries: vec![ExternalSkillSummary {
                        source_uuid: "legacy".to_string(),
                        skill_name: "extraction/email".to_string(),
                        description: "Email".to_string(),
                    }],
                },
            )))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills/extraction%2Femail"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsLoadPackage {
                    package: ExternalSkillPackage {
                        summary: ExternalSkillSummary {
                            source_uuid: "legacy".to_string(),
                            skill_name: "extraction/email".to_string(),
                            description: "Email".to_string(),
                        },
                        body: "# Email".to_string(),
                    },
                },
            )))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills/extraction%2Femail/artifacts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsListArtifacts {
                    artifacts: vec![ExternalSkillArtifact {
                        path: "resources/schema.json".to_string(),
                        mime_type: "application/json".to_string(),
                        byte_length: 2,
                    }],
                },
            )))
            .mount(server)
            .await;

        Mock::given(method("GET"))
            .and(path(
                "/skills/extraction%2Femail/artifacts/resources%2Fschema.json",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsReadArtifact {
                    artifact: ExternalArtifactContent {
                        path: "resources/schema.json".to_string(),
                        mime_type: "application/json".to_string(),
                        content: "{}".to_string(),
                    },
                },
            )))
            .mount(server)
            .await;

        Mock::given(method("POST"))
            .and(path("/skills/extraction%2Femail/functions/lint"))
            .and(body_json(serde_json::json!({"strict": true})))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsInvokeFunction {
                    output: serde_json::json!({
                        "ok": true
                    }),
                },
            )))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn test_http_transport_lifecycle_summaries_package_resources_functions() {
        let server = MockServer::start().await;
        setup_protocol_mocks(&server).await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id.0, "extraction/email");

        let loaded = source
            .load(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(loaded.body, "# Email");

        let artifacts = source
            .list_artifacts(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(artifacts[0].path, "resources/schema.json");

        let artifact = source
            .read_artifact(
                &SkillId("extraction/email".to_string()),
                "resources/schema.json",
            )
            .await
            .unwrap();
        assert_eq!(artifact.content, "{}");

        let invoke = source
            .invoke_function(
                &SkillId("extraction/email".to_string()),
                "lint",
                serde_json::json!({"strict": true}),
            )
            .await
            .unwrap();
        assert_eq!(invoke["ok"], true);
    }

    #[tokio::test]
    async fn test_handshake_cached_for_http_source_instance() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(capabilities_body()))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsListSummaries {
                    summaries: vec![ExternalSkillSummary {
                        source_uuid: "legacy".to_string(),
                        skill_name: "extraction/email".to_string(),
                        description: "Email".to_string(),
                    }],
                },
            )))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills/extraction%2Femail"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsLoadPackage {
                    package: ExternalSkillPackage {
                        summary: ExternalSkillSummary {
                            source_uuid: "legacy".to_string(),
                            skill_name: "extraction/email".to_string(),
                            description: "Email".to_string(),
                        },
                        body: "# Email".to_string(),
                    },
                },
            )))
            .mount(&server)
            .await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        let _ = source.list(&SkillFilter::default()).await.unwrap();
        let _ = source
            .load(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_capability_mismatch_returns_typed_error_for_http() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/capabilities"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::CapabilitiesGet(SourceCapabilityHandshake {
                    protocol_version: 1,
                    methods: vec![
                        "skills/list_summaries".to_string(),
                        "skills/load_package".to_string(),
                    ],
                }),
            )))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsListSummaries {
                    summaries: vec![ExternalSkillSummary {
                        source_uuid: "legacy".to_string(),
                        skill_name: "extraction/email".to_string(),
                        description: "Email".to_string(),
                    }],
                },
            )))
            .mount(&server)
            .await;

        let source = HttpSkillSource::new(server.uri(), None, Duration::from_secs(60));

        let err = source
            .read_artifact(
                &SkillId("extraction/email".to_string()),
                "resources/schema.json",
            )
            .await
            .expect_err("missing capability should fail");

        assert!(matches!(
            err,
            SkillError::CapabilityUnavailable { capability, .. }
            if capability == "skills/read_artifact"
        ));

        let snapshot = source.health_snapshot().await.unwrap();
        assert!(snapshot.handshake_failed);
        assert_eq!(
            snapshot.state,
            meerkat_core::skills::SourceHealthState::Unhealthy
        );
    }

    #[tokio::test]
    async fn test_auth_headers_still_applied_for_protocol_calls() {
        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/capabilities"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(capabilities_body()))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/skills"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(protocol_body(
                SourceProtocolResponse::SkillsListSummaries { summaries: vec![] },
            )))
            .mount(&server)
            .await;

        let source = HttpSkillSource::new(
            server.uri(),
            Some(HttpSkillAuth::Bearer("test-token".into())),
            Duration::from_secs(60),
        );

        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert!(listed.is_empty());
    }

    #[tokio::test]
    async fn test_http_transport_timeout_returns_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/capabilities"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(200))
                    .set_body_json(capabilities_body()),
            )
            .mount(&server)
            .await;

        let client = HttpExternalClient::new_with_timeout(
            "timeout-src",
            server.uri(),
            None,
            Duration::from_millis(25),
        );

        let error = client
            .capabilities_get(1)
            .await
            .expect_err("timeout should fail");
        let SkillError::Load(message) = error else {
            panic!("expected load error");
        };
        assert!(!message.to_string().is_empty());
    }

    #[test]
    fn test_endpoint_for_protocol_requests() {
        let list = endpoint_for(&SourceProtocolRequest::SkillsListSummaries);
        assert_eq!(list, "/skills");

        let load_legacy = endpoint_for(&SourceProtocolRequest::SkillsLoadPackage {
            source_uuid: "legacy".to_string(),
            skill_name: "extraction/email".to_string(),
        });
        assert_eq!(load_legacy, "/skills/extraction%2Femail");

        let load_structured = endpoint_for(&SourceProtocolRequest::SkillsLoadPackage {
            source_uuid: "dc256086-0d2f-4f61-a307-320d4148107f".to_string(),
            skill_name: "email-extractor".to_string(),
        });
        assert_eq!(
            load_structured,
            "/skills/dc256086-0d2f-4f61-a307-320d4148107f%2Femail-extractor"
        );
    }
}
