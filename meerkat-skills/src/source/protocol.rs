//! Typed external skill-source protocol payloads and adapters.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::RwLock;
use tokio::time::timeout;

use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillDescriptor, SkillDocument, SkillError, SkillFilter,
    SkillId, SkillScope, SkillSource, apply_filter,
};

pub const SOURCE_PROTOCOL_MIN_VERSION: u16 = 1;
const EXTERNAL_SOURCE_SENTINEL_ID: &str = "__external_source__";

/// Versioned capability handshake payload for `capabilities/get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceCapabilityHandshake {
    pub protocol_version: u16,
    pub methods: Vec<String>,
}

/// Summary metadata for a remotely hosted skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSkillSummary {
    pub source_uuid: String,
    pub skill_name: String,
    pub description: String,
}

/// Full package payload for a remote skill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSkillPackage {
    pub summary: ExternalSkillSummary,
    pub body: String,
}

/// Artifact descriptor for a remote skill package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalSkillArtifact {
    pub path: String,
    pub mime_type: String,
    pub byte_length: usize,
}

/// Artifact content payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalArtifactContent {
    pub path: String,
    pub mime_type: String,
    pub content: String,
}

/// Typed request methods for the external protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum SourceProtocolRequest {
    #[serde(rename = "capabilities/get")]
    CapabilitiesGet { min_protocol_version: u16 },
    #[serde(rename = "skills/list_summaries")]
    SkillsListSummaries,
    #[serde(rename = "skills/load_package")]
    SkillsLoadPackage {
        source_uuid: String,
        skill_name: String,
    },
    #[serde(rename = "skills/list_artifacts")]
    SkillsListArtifacts {
        source_uuid: String,
        skill_name: String,
    },
    #[serde(rename = "skills/read_artifact")]
    SkillsReadArtifact {
        source_uuid: String,
        skill_name: String,
        artifact_path: String,
    },
    #[serde(rename = "skills/invoke_function")]
    SkillsInvokeFunction {
        source_uuid: String,
        skill_name: String,
        function_name: String,
        arguments: serde_json::Value,
    },
}

impl SourceProtocolRequest {
    pub fn method_name(&self) -> &'static str {
        match self {
            Self::CapabilitiesGet { .. } => "capabilities/get",
            Self::SkillsListSummaries => "skills/list_summaries",
            Self::SkillsLoadPackage { .. } => "skills/load_package",
            Self::SkillsListArtifacts { .. } => "skills/list_artifacts",
            Self::SkillsReadArtifact { .. } => "skills/read_artifact",
            Self::SkillsInvokeFunction { .. } => "skills/invoke_function",
        }
    }
}

/// Typed response payloads for the external protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "result")]
pub enum SourceProtocolResponse {
    #[serde(rename = "capabilities/get")]
    CapabilitiesGet(SourceCapabilityHandshake),
    #[serde(rename = "skills/list_summaries")]
    SkillsListSummaries {
        summaries: Vec<ExternalSkillSummary>,
    },
    #[serde(rename = "skills/load_package")]
    SkillsLoadPackage { package: ExternalSkillPackage },
    #[serde(rename = "skills/list_artifacts")]
    SkillsListArtifacts {
        artifacts: Vec<ExternalSkillArtifact>,
    },
    #[serde(rename = "skills/read_artifact")]
    SkillsReadArtifact { artifact: ExternalArtifactContent },
    #[serde(rename = "skills/invoke_function")]
    SkillsInvokeFunction { output: serde_json::Value },
}

/// Stdio transport envelope for source-protocol exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StdioSourceEnvelope<T> {
    pub jsonrpc: String,
    pub id: String,
    pub payload: T,
}

/// HTTP transport envelope for source-protocol exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpSourceEnvelope<T> {
    pub status: u16,
    pub payload: T,
}

/// External client boundary.
pub trait ExternalClient: Send + Sync {
    fn capabilities_get(
        &self,
        min_protocol_version: u16,
    ) -> impl Future<Output = Result<SourceCapabilityHandshake, SkillError>> + Send;

    fn skills_list_summaries(
        &self,
    ) -> impl Future<Output = Result<Vec<ExternalSkillSummary>, SkillError>> + Send;

    fn skills_load_package(
        &self,
        source_uuid: &str,
        skill_name: &str,
    ) -> impl Future<Output = Result<ExternalSkillPackage, SkillError>> + Send;

    fn skills_list_artifacts(
        &self,
        source_uuid: &str,
        skill_name: &str,
    ) -> impl Future<Output = Result<Vec<ExternalSkillArtifact>, SkillError>> + Send;

    fn skills_read_artifact(
        &self,
        source_uuid: &str,
        skill_name: &str,
        artifact_path: &str,
    ) -> impl Future<Output = Result<ExternalArtifactContent, SkillError>> + Send;

    fn skills_invoke_function(
        &self,
        source_uuid: &str,
        skill_name: &str,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send;
}

/// External source adapter that performs and caches capability handshake per instance.
pub struct ExternalSkillSource<C: ExternalClient> {
    source_uuid: String,
    client: C,
    handshake: Arc<RwLock<Option<SourceCapabilityHandshake>>>,
    skill_source_index: Arc<RwLock<HashMap<SkillId, String>>>,
    health: Arc<RwLock<meerkat_core::skills::SourceHealthSnapshot>>,
}

impl<C: ExternalClient> ExternalSkillSource<C> {
    pub fn new(source_uuid: impl Into<String>, client: C) -> Self {
        Self {
            source_uuid: source_uuid.into(),
            client,
            handshake: Arc::new(RwLock::new(None)),
            skill_source_index: Arc::new(RwLock::new(HashMap::new())),
            health: Arc::new(RwLock::new(
                meerkat_core::skills::SourceHealthSnapshot::default(),
            )),
        }
    }

    async fn mark_handshake_failure(&self) {
        let mut health = self.health.write().await;
        health.handshake_failed = true;
        health.failure_streak = health.failure_streak.saturating_add(1);
        health.state = meerkat_core::skills::SourceHealthState::Unhealthy;
    }

    async fn mark_handshake_success(&self) {
        let mut health = self.health.write().await;
        health.handshake_failed = false;
        health.failure_streak = 0;
        health.state = meerkat_core::skills::SourceHealthState::Healthy;
    }

    async fn ensure_capability(&self, capability: &str) -> Result<(), SkillError> {
        {
            let cache = self.handshake.read().await;
            if let Some(handshake) = cache.as_ref() {
                if let Err(error) = validate_capability(handshake, capability) {
                    self.mark_handshake_failure().await;
                    return Err(error);
                }
                self.mark_handshake_success().await;
                return Ok(());
            }
        }

        let handshake = match self
            .client
            .capabilities_get(SOURCE_PROTOCOL_MIN_VERSION)
            .await
        {
            Ok(handshake) => handshake,
            Err(error) => {
                self.mark_handshake_failure().await;
                return Err(error);
            }
        };

        if let Err(error) = validate_capability(&handshake, capability) {
            self.mark_handshake_failure().await;
            return Err(error);
        }

        let mut cache = self.handshake.write().await;
        if cache.is_none() {
            *cache = Some(handshake);
        }

        self.mark_handshake_success().await;
        Ok(())
    }

    async fn source_for_skill(&self, skill_id: &SkillId) -> Result<String, SkillError> {
        if let Some(found) = self.skill_source_index.read().await.get(skill_id).cloned() {
            return Ok(found);
        }

        self.ensure_capability("skills/list_summaries").await?;
        let summaries = self.client.skills_list_summaries().await?;

        let mut discovered = None;
        let mut index = self.skill_source_index.write().await;
        for summary in summaries {
            let id = SkillId(summary.skill_name.clone());
            if id == *skill_id {
                discovered = Some(summary.source_uuid.clone());
            }
            index.insert(id, summary.source_uuid);
        }

        Ok(discovered.unwrap_or_else(|| self.source_uuid.clone()))
    }

    pub async fn list_artifacts(
        &self,
        skill_id: &SkillId,
    ) -> Result<Vec<SkillArtifact>, SkillError> {
        self.ensure_capability("skills/list_artifacts").await?;
        let source_uuid = self.source_for_skill(skill_id).await?;
        let artifacts = self
            .client
            .skills_list_artifacts(&source_uuid, &skill_id.0)
            .await?;
        Ok(artifacts
            .into_iter()
            .map(|artifact| SkillArtifact {
                path: artifact.path,
                mime_type: artifact.mime_type,
                byte_length: artifact.byte_length,
            })
            .collect())
    }

    pub async fn read_artifact(
        &self,
        skill_id: &SkillId,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, SkillError> {
        self.ensure_capability("skills/read_artifact").await?;
        let source_uuid = self.source_for_skill(skill_id).await?;
        let artifact = self
            .client
            .skills_read_artifact(&source_uuid, &skill_id.0, artifact_path)
            .await?;
        Ok(SkillArtifactContent {
            path: artifact.path,
            mime_type: artifact.mime_type,
            content: artifact.content,
        })
    }

    pub async fn invoke_function(
        &self,
        skill_id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, SkillError> {
        self.ensure_capability("skills/invoke_function").await?;
        let source_uuid = self.source_for_skill(skill_id).await?;
        self.client
            .skills_invoke_function(&source_uuid, &skill_id.0, function_name, arguments)
            .await
    }
}

fn validate_capability(
    handshake: &SourceCapabilityHandshake,
    capability: &str,
) -> Result<(), SkillError> {
    if handshake.protocol_version < SOURCE_PROTOCOL_MIN_VERSION {
        return Err(SkillError::CapabilityUnavailable {
            id: SkillId(EXTERNAL_SOURCE_SENTINEL_ID.to_string()),
            capability: format!(
                "protocol_version>={SOURCE_PROTOCOL_MIN_VERSION} required, got {}",
                handshake.protocol_version
            ),
        });
    }

    if !handshake.methods.iter().any(|m| m == capability) {
        return Err(SkillError::CapabilityUnavailable {
            id: SkillId(EXTERNAL_SOURCE_SENTINEL_ID.to_string()),
            capability: capability.to_string(),
        });
    }

    Ok(())
}

impl<C: ExternalClient + Send + Sync> SkillSource for ExternalSkillSource<C> {
    fn list(
        &self,
        filter: &SkillFilter,
    ) -> impl Future<Output = Result<Vec<SkillDescriptor>, SkillError>> + Send {
        async move {
            self.ensure_capability("skills/list_summaries").await?;
            let summaries = self.client.skills_list_summaries().await?;

            {
                let mut index = self.skill_source_index.write().await;
                index.clear();
                for summary in &summaries {
                    index.insert(
                        SkillId(summary.skill_name.clone()),
                        summary.source_uuid.clone(),
                    );
                }
            }

            let descriptors = summaries
                .into_iter()
                .map(|summary| SkillDescriptor {
                    id: SkillId(summary.skill_name.clone()),
                    name: summary.skill_name,
                    description: summary.description,
                    scope: SkillScope::Project,
                    ..Default::default()
                })
                .collect::<Vec<_>>();

            Ok(apply_filter(&descriptors, filter))
        }
    }

    fn load(&self, id: &SkillId) -> impl Future<Output = Result<SkillDocument, SkillError>> + Send {
        async move {
            self.ensure_capability("skills/load_package").await?;
            let source_uuid = self.source_for_skill(id).await?;

            let package = self
                .client
                .skills_load_package(&source_uuid, &id.0)
                .await
                .map_err(|error| match error {
                    SkillError::NotFound { .. } => SkillError::NotFound { id: id.clone() },
                    other => other,
                })?;

            Ok(SkillDocument {
                descriptor: SkillDescriptor {
                    id: id.clone(),
                    name: package.summary.skill_name,
                    description: package.summary.description,
                    scope: SkillScope::Project,
                    ..Default::default()
                },
                body: package.body,
                extensions: indexmap::IndexMap::new(),
            })
        }
    }

    fn health_snapshot(
        &self,
    ) -> impl Future<Output = Result<meerkat_core::skills::SourceHealthSnapshot, SkillError>> + Send
    {
        async move { Ok(self.health.read().await.clone()) }
    }

    fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> impl Future<Output = Result<Vec<SkillArtifact>, SkillError>> + Send {
        async move { ExternalSkillSource::list_artifacts(self, id).await }
    }

    fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> impl Future<Output = Result<SkillArtifactContent, SkillError>> + Send {
        async move { ExternalSkillSource::read_artifact(self, id, artifact_path).await }
    }

    fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
        async move { ExternalSkillSource::invoke_function(self, id, function_name, arguments).await }
    }
}

/// External client over stdio transport. This is out-of-process IPC only.
pub struct StdioExternalClient {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: Option<PathBuf>,
    timeout: std::time::Duration,
}

impl StdioExternalClient {
    pub fn new(
        command: impl Into<String>,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
    ) -> Self {
        Self::new_with_timeout(command, args, env, cwd, std::time::Duration::from_secs(15))
    }

    pub fn new_with_timeout(
        command: impl Into<String>,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<PathBuf>,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            command: command.into(),
            args,
            env,
            cwd,
            timeout,
        }
    }

    async fn call(
        &self,
        request: SourceProtocolRequest,
    ) -> Result<SourceProtocolResponse, SkillError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        // Ensure subprocesses are not leaked if a timeout path drops the child future.
        cmd.kill_on_drop(true);
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            SkillError::Load(
                format!(
                    "failed to spawn stdio source process '{}': {e}",
                    self.command
                )
                .into(),
            )
        })?;

        let envelope = StdioSourceEnvelope {
            jsonrpc: "2.0".to_string(),
            id: NEXT_ID.fetch_add(1, Ordering::SeqCst).to_string(),
            payload: request,
        };
        let payload = serde_json::to_vec(&envelope)
            .map_err(|e| SkillError::Load(format!("failed to encode stdio request: {e}").into()))?;

        if let Some(mut stdin) = child.stdin.take() {
            timeout(self.timeout, stdin.write_all(&payload))
                .await
                .map_err(|_| {
                    SkillError::Load(
                        format!("stdio source write timed out after {:?}", self.timeout).into(),
                    )
                })
                .inspect_err(|_| {
                    let _ = child.start_kill();
                })?
                .map_err(|e| {
                    SkillError::Load(format!("failed to write stdio request: {e}").into())
                })?;
            timeout(self.timeout, stdin.write_all(b"\n"))
                .await
                .map_err(|_| {
                    SkillError::Load(
                        format!(
                            "stdio source newline write timed out after {:?}",
                            self.timeout
                        )
                        .into(),
                    )
                })
                .inspect_err(|_| {
                    let _ = child.start_kill();
                })?
                .map_err(|e| {
                    SkillError::Load(format!("failed to write stdio newline: {e}").into())
                })?;
        }

        let output = match timeout(self.timeout, child.wait_with_output()).await {
            Ok(result) => result.map_err(|e| {
                SkillError::Load(format!("failed waiting for stdio process: {e}").into())
            })?,
            Err(_) => {
                return Err(SkillError::Load(
                    format!(
                        "stdio source process timed out after {:?} for method {}",
                        self.timeout,
                        envelope.payload.method_name()
                    )
                    .into(),
                ));
            }
        };

        if !output.status.success() {
            return Err(SkillError::Load(
                format!(
                    "stdio source process exited {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                )
                .into(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let response_line = stdout
            .lines()
            .find(|line| !line.trim().is_empty())
            .ok_or_else(|| SkillError::Load("stdio source returned empty response".into()))?;

        if let Ok(envelope) =
            serde_json::from_str::<StdioSourceEnvelope<SourceProtocolResponse>>(response_line)
        {
            return Ok(envelope.payload);
        }

        serde_json::from_str::<SourceProtocolResponse>(response_line).map_err(|e| {
            SkillError::Load(format!("failed to decode stdio source response: {e}").into())
        })
    }
}

impl ExternalClient for StdioExternalClient {
    fn capabilities_get(
        &self,
        min_protocol_version: u16,
    ) -> impl Future<Output = Result<SourceCapabilityHandshake, SkillError>> + Send {
        async move {
            match self
                .call(SourceProtocolRequest::CapabilitiesGet {
                    min_protocol_version,
                })
                .await?
            {
                SourceProtocolResponse::CapabilitiesGet(handshake) => Ok(handshake),
                other => Err(SkillError::Load(
                    format!(
                        "expected capabilities/get response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
            }
        }
    }

    fn skills_list_summaries(
        &self,
    ) -> impl Future<Output = Result<Vec<ExternalSkillSummary>, SkillError>> + Send {
        async move {
            match self
                .call(SourceProtocolRequest::SkillsListSummaries)
                .await?
            {
                SourceProtocolResponse::SkillsListSummaries { summaries } => Ok(summaries),
                other => Err(SkillError::Load(
                    format!(
                        "expected skills/list_summaries response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
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
            match self
                .call(SourceProtocolRequest::SkillsLoadPackage {
                    source_uuid,
                    skill_name,
                })
                .await?
            {
                SourceProtocolResponse::SkillsLoadPackage { package } => Ok(package),
                other => Err(SkillError::Load(
                    format!(
                        "expected skills/load_package response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
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
            match self
                .call(SourceProtocolRequest::SkillsListArtifacts {
                    source_uuid,
                    skill_name,
                })
                .await?
            {
                SourceProtocolResponse::SkillsListArtifacts { artifacts } => Ok(artifacts),
                other => Err(SkillError::Load(
                    format!(
                        "expected skills/list_artifacts response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
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
            match self
                .call(SourceProtocolRequest::SkillsReadArtifact {
                    source_uuid,
                    skill_name,
                    artifact_path,
                })
                .await?
            {
                SourceProtocolResponse::SkillsReadArtifact { artifact } => Ok(artifact),
                other => Err(SkillError::Load(
                    format!(
                        "expected skills/read_artifact response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
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
            match self
                .call(SourceProtocolRequest::SkillsInvokeFunction {
                    source_uuid,
                    skill_name,
                    function_name,
                    arguments,
                })
                .await?
            {
                SourceProtocolResponse::SkillsInvokeFunction { output } => Ok(output),
                other => Err(SkillError::Load(
                    format!(
                        "expected skills/invoke_function response, received {}",
                        response_method_name(&other)
                    )
                    .into(),
                )),
            }
        }
    }
}

fn response_method_name(response: &SourceProtocolResponse) -> &'static str {
    match response {
        SourceProtocolResponse::CapabilitiesGet(_) => "capabilities/get",
        SourceProtocolResponse::SkillsListSummaries { .. } => "skills/list_summaries",
        SourceProtocolResponse::SkillsLoadPackage { .. } => "skills/load_package",
        SourceProtocolResponse::SkillsListArtifacts { .. } => "skills/list_artifacts",
        SourceProtocolResponse::SkillsReadArtifact { .. } => "skills/read_artifact",
        SourceProtocolResponse::SkillsInvokeFunction { .. } => "skills/invoke_function",
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicUsize;

    use tokio::sync::Mutex;
    use tokio::time::{Duration, timeout};

    fn valid_capabilities_response() -> SourceProtocolResponse {
        SourceProtocolResponse::CapabilitiesGet(SourceCapabilityHandshake {
            protocol_version: 1,
            methods: vec![
                "capabilities/get".to_string(),
                "skills/list_summaries".to_string(),
                "skills/load_package".to_string(),
                "skills/list_artifacts".to_string(),
                "skills/read_artifact".to_string(),
                "skills/invoke_function".to_string(),
            ],
        })
    }

    #[test]
    fn test_protocol_payload_roundtrip_stdio_http_envelopes() {
        let response = valid_capabilities_response();

        let stdio = StdioSourceEnvelope {
            jsonrpc: "2.0".to_string(),
            id: "42".to_string(),
            payload: response.clone(),
        };
        let stdio_json = serde_json::to_value(&stdio).expect("serialize stdio envelope");
        let stdio_back: StdioSourceEnvelope<SourceProtocolResponse> =
            serde_json::from_value(stdio_json).expect("deserialize stdio envelope");
        assert_eq!(stdio_back, stdio);

        let http = HttpSourceEnvelope {
            status: 200,
            payload: response,
        };
        let http_json = serde_json::to_value(&http).expect("serialize http envelope");
        let http_back: HttpSourceEnvelope<SourceProtocolResponse> =
            serde_json::from_value(http_json).expect("deserialize http envelope");
        assert_eq!(http_back, http);
    }

    #[derive(Debug, Clone, Copy)]
    enum AdversarialMode {
        FailThenSucceed,
        SchemaInvalid,
        NeverResponds,
        UnexpectedType,
    }

    #[derive(Debug)]
    enum MockProtocolError {
        Transient(String),
    }

    impl std::fmt::Display for MockProtocolError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Transient(msg) => f.write_str(msg),
            }
        }
    }

    impl std::error::Error for MockProtocolError {}

    /// Adversarial source mock scenarios used by external-boundary tests.
    struct AdversarialProtocolMock {
        mode: AdversarialMode,
        calls: AtomicUsize,
        shared_guard: Arc<Mutex<()>>,
    }

    impl AdversarialProtocolMock {
        fn new(mode: AdversarialMode) -> Self {
            Self {
                mode,
                calls: AtomicUsize::new(0),
                shared_guard: Arc::new(Mutex::new(())),
            }
        }

        async fn call(
            &self,
            request: SourceProtocolRequest,
        ) -> Result<serde_json::Value, MockProtocolError> {
            let _guard = self.shared_guard.lock().await;
            let call_index = self.calls.fetch_add(1, Ordering::SeqCst);

            match self.mode {
                AdversarialMode::FailThenSucceed => {
                    if call_index == 0 {
                        return Err(MockProtocolError::Transient(
                            "injected transient transport failure".to_string(),
                        ));
                    }
                    Ok(serde_json::to_value(match request {
                        SourceProtocolRequest::CapabilitiesGet { .. } => {
                            valid_capabilities_response()
                        }
                        _ => SourceProtocolResponse::SkillsListSummaries {
                            summaries: Vec::new(),
                        },
                    })
                    .expect("serialize fail-then-succeed payload"))
                }
                AdversarialMode::SchemaInvalid => Ok(
                    serde_json::json!({"method":"capabilities/get","result":{"protocol_version":"not-a-number","methods":[]}}),
                ),
                AdversarialMode::NeverResponds => {
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                AdversarialMode::UnexpectedType => Ok(
                    serde_json::json!({"method":"skills/list_summaries","result":{"summaries":"not-an-array"}}),
                ),
            }
        }
    }

    #[tokio::test]
    #[ignore = "external-boundary-adversarial"]
    async fn test_adversarial_mock_fail_then_succeed() {
        let mock = AdversarialProtocolMock::new(AdversarialMode::FailThenSucceed);
        let request = SourceProtocolRequest::CapabilitiesGet {
            min_protocol_version: 1,
        };

        let first = mock.call(request.clone()).await;
        assert!(first.is_err());

        let second = mock
            .call(request)
            .await
            .expect("second call should succeed");
        let parsed: SourceProtocolResponse =
            serde_json::from_value(second).expect("response should deserialize");
        assert!(matches!(
            parsed,
            SourceProtocolResponse::CapabilitiesGet(SourceCapabilityHandshake { .. })
        ));
    }

    #[tokio::test]
    #[ignore = "external-boundary-adversarial"]
    async fn test_adversarial_mock_schema_invalid() {
        let mock = AdversarialProtocolMock::new(AdversarialMode::SchemaInvalid);
        let raw = mock
            .call(SourceProtocolRequest::CapabilitiesGet {
                min_protocol_version: 1,
            })
            .await
            .expect("mock should return invalid payload");

        let parsed: Result<SourceProtocolResponse, _> = serde_json::from_value(raw);
        assert!(parsed.is_err());
    }

    #[tokio::test]
    #[ignore = "external-boundary-adversarial"]
    async fn test_adversarial_mock_never_responds() {
        let mock = AdversarialProtocolMock::new(AdversarialMode::NeverResponds);

        let result = timeout(
            Duration::from_millis(25),
            mock.call(SourceProtocolRequest::SkillsListSummaries),
        )
        .await;

        assert!(result.is_err(), "never-responds should time out");
    }

    #[tokio::test]
    #[ignore = "external-boundary-adversarial"]
    async fn test_adversarial_mock_unexpected_type() {
        let mock = AdversarialProtocolMock::new(AdversarialMode::UnexpectedType);
        let raw = mock
            .call(SourceProtocolRequest::SkillsListSummaries)
            .await
            .expect("mock should return payload");

        let parsed: Result<SourceProtocolResponse, _> = serde_json::from_value(raw);
        assert!(
            parsed.is_err(),
            "unexpected type should fail deserialization"
        );
    }

    struct MockExternalClient {
        handshake_calls: Arc<AtomicUsize>,
        methods: Vec<String>,
    }

    impl MockExternalClient {
        fn full() -> Self {
            Self {
                handshake_calls: Arc::new(AtomicUsize::new(0)),
                methods: vec![
                    "skills/list_summaries".into(),
                    "skills/load_package".into(),
                    "skills/list_artifacts".into(),
                    "skills/read_artifact".into(),
                    "skills/invoke_function".into(),
                ],
            }
        }

        fn missing(method: &str) -> Self {
            let mut base = Self::full();
            base.methods.retain(|entry| entry != method);
            base
        }
    }

    impl ExternalClient for MockExternalClient {
        fn capabilities_get(
            &self,
            _min_protocol_version: u16,
        ) -> impl Future<Output = Result<SourceCapabilityHandshake, SkillError>> + Send {
            async move {
                self.handshake_calls.fetch_add(1, Ordering::SeqCst);
                Ok(SourceCapabilityHandshake {
                    protocol_version: 1,
                    methods: self.methods.clone(),
                })
            }
        }

        fn skills_list_summaries(
            &self,
        ) -> impl Future<Output = Result<Vec<ExternalSkillSummary>, SkillError>> + Send {
            async move {
                Ok(vec![ExternalSkillSummary {
                    source_uuid: "source-a".to_string(),
                    skill_name: "extraction/email".to_string(),
                    description: "Extract email entities".to_string(),
                }])
            }
        }

        fn skills_load_package(
            &self,
            _source_uuid: &str,
            skill_name: &str,
        ) -> impl Future<Output = Result<ExternalSkillPackage, SkillError>> + Send {
            let skill_name = skill_name.to_string();
            async move {
                Ok(ExternalSkillPackage {
                    summary: ExternalSkillSummary {
                        source_uuid: "source-a".to_string(),
                        skill_name,
                        description: "Extract email entities".to_string(),
                    },
                    body: "# Email\n\nBody".to_string(),
                })
            }
        }

        fn skills_list_artifacts(
            &self,
            _source_uuid: &str,
            _skill_name: &str,
        ) -> impl Future<Output = Result<Vec<ExternalSkillArtifact>, SkillError>> + Send {
            async move {
                Ok(vec![ExternalSkillArtifact {
                    path: "resources/schema.json".to_string(),
                    mime_type: "application/json".to_string(),
                    byte_length: 2,
                }])
            }
        }

        fn skills_read_artifact(
            &self,
            _source_uuid: &str,
            _skill_name: &str,
            artifact_path: &str,
        ) -> impl Future<Output = Result<ExternalArtifactContent, SkillError>> + Send {
            let artifact_path = artifact_path.to_string();
            async move {
                Ok(ExternalArtifactContent {
                    path: artifact_path,
                    mime_type: "application/json".to_string(),
                    content: "{}".to_string(),
                })
            }
        }

        fn skills_invoke_function(
            &self,
            _source_uuid: &str,
            _skill_name: &str,
            function_name: &str,
            arguments: serde_json::Value,
        ) -> impl Future<Output = Result<serde_json::Value, SkillError>> + Send {
            let function_name = function_name.to_string();
            async move { Ok(serde_json::json!({"function": function_name, "arguments": arguments})) }
        }
    }

    #[tokio::test]
    async fn test_handshake_precedes_ops_and_is_cached_per_source_instance() {
        let client = MockExternalClient::full();
        let handshake_counter = client.handshake_calls.clone();
        let source = ExternalSkillSource::new("source-a", client);

        let listed = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 1);

        let loaded = source
            .load(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(loaded.descriptor.id.0, "extraction/email");

        let artifacts = source
            .list_artifacts(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(artifacts.len(), 1);

        let invoked = source
            .invoke_function(
                &SkillId("extraction/email".to_string()),
                "validate",
                serde_json::json!({"x": 1}),
            )
            .await
            .unwrap();
        assert_eq!(invoked["function"], "validate");

        assert_eq!(handshake_counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_capability_mismatch_returns_typed_error() {
        let source = ExternalSkillSource::new(
            "source-a",
            MockExternalClient::missing("skills/read_artifact"),
        );

        let error = source
            .read_artifact(
                &SkillId("extraction/email".to_string()),
                "resources/schema.json",
            )
            .await
            .expect_err("missing capability should fail");

        assert!(matches!(
            error,
            SkillError::CapabilityUnavailable {
                id: SkillId(ref id),
                capability
            } if id == EXTERNAL_SOURCE_SENTINEL_ID && capability == "skills/read_artifact"
        ));

        let health = source.health_snapshot().await.unwrap();
        assert!(health.handshake_failed);
        assert_eq!(
            health.state,
            meerkat_core::skills::SourceHealthState::Unhealthy
        );
        assert!(health.failure_streak >= 1);
    }

    #[tokio::test]
    async fn test_stdio_transport_lifecycle_summaries_package_resources_functions() {
        let response_script = r##"
read line
if echo "$line" | grep -q '"method":"capabilities/get"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"capabilities/get","result":{"protocol_version":1,"methods":["skills/list_summaries","skills/load_package","skills/list_artifacts","skills/read_artifact","skills/invoke_function"]}}}'
elif echo "$line" | grep -q '"method":"skills/list_summaries"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/list_summaries","result":{"summaries":[{"source_uuid":"source-a","skill_name":"extraction/email","description":"Email"}]}}}'
elif echo "$line" | grep -q '"method":"skills/load_package"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/load_package","result":{"package":{"summary":{"source_uuid":"source-a","skill_name":"extraction/email","description":"Email"},"body":"# Body"}}}}'
elif echo "$line" | grep -q '"method":"skills/list_artifacts"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/list_artifacts","result":{"artifacts":[{"path":"resources/schema.json","mime_type":"application/json","byte_length":2}]}}}'
elif echo "$line" | grep -q '"method":"skills/read_artifact"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/read_artifact","result":{"artifact":{"path":"resources/schema.json","mime_type":"application/json","content":"{}"}}}}'
elif echo "$line" | grep -q '"method":"skills/invoke_function"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/invoke_function","result":{"output":{"ok":true}}}}'
fi
"##;

        let client = StdioExternalClient::new(
            "sh",
            vec!["-c".to_string(), response_script.to_string()],
            BTreeMap::new(),
            None,
        );
        let source = ExternalSkillSource::new("source-a", client);

        let summaries = source.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(summaries.len(), 1);

        let package = source
            .load(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(package.body, "# Body");

        let artifacts = source
            .list_artifacts(&SkillId("extraction/email".to_string()))
            .await
            .unwrap();
        assert_eq!(artifacts[0].path, "resources/schema.json");

        let content = source
            .read_artifact(
                &SkillId("extraction/email".to_string()),
                "resources/schema.json",
            )
            .await
            .unwrap();
        assert_eq!(content.content, "{}");

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
    async fn test_stdio_transport_timeout_returns_typed_error() {
        let script = "sleep 1; echo '{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"payload\":{\"method\":\"capabilities/get\",\"result\":{\"protocol_version\":1,\"methods\":[\"skills/list_summaries\"]}}}'";
        let client = StdioExternalClient::new_with_timeout(
            "sh",
            vec!["-c".to_string(), script.to_string()],
            BTreeMap::new(),
            None,
            Duration::from_millis(20),
        );

        let error = client
            .capabilities_get(1)
            .await
            .expect_err("timeout should fail");
        let SkillError::Load(message) = error else {
            panic!("expected load error");
        };
        assert!(message.to_string().contains("timed out"));
    }
}
