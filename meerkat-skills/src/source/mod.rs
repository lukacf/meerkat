//! Skill sources.

pub mod composite;
pub mod embedded;
pub mod filesystem;
pub mod git;
#[cfg(any(feature = "skills-http", test))]
pub mod http;
pub mod memory;
pub mod protocol;

use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillCollection, SkillDescriptor, SkillDocument,
    SkillFilter, SkillId, SkillQuarantineDiagnostic, SkillSource, SourceHealthSnapshot,
};

pub use composite::{CompositeSkillSource, NamedSource};
pub use embedded::EmbeddedSkillSource;
pub use filesystem::FilesystemSkillSource;
#[cfg(any(feature = "skills-http", test))]
pub use http::HttpSkillSource;
pub use memory::InMemorySkillSource;
pub use protocol::{ExternalSkillSource, StdioExternalClient};

/// Typed source composition node for built-ins + bounded external seam.
pub enum SourceNode {
    Embedded(EmbeddedSkillSource),
    Filesystem(FilesystemSkillSource),
    Git(Box<git::GitSkillSource>),
    Memory(InMemorySkillSource),
    #[cfg(any(feature = "skills-http", test))]
    Http(Box<HttpSkillSource>),
    External(ExternalSkillSource<StdioExternalClient>),
}

impl SkillSource for SourceNode {
    async fn list(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.list(filter).await,
            Self::Filesystem(source) => source.list(filter).await,
            Self::Git(source) => source.list(filter).await,
            Self::Memory(source) => source.list(filter).await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.list(filter).await,
            Self::External(source) => source.list(filter).await,
        }
    }

    async fn load(&self, id: &SkillId) -> Result<SkillDocument, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.load(id).await,
            Self::Filesystem(source) => source.load(id).await,
            Self::Git(source) => source.load(id).await,
            Self::Memory(source) => source.load(id).await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.load(id).await,
            Self::External(source) => source.load(id).await,
        }
    }

    async fn collections(&self) -> Result<Vec<SkillCollection>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.collections().await,
            Self::Filesystem(source) => source.collections().await,
            Self::Git(source) => source.collections().await,
            Self::Memory(source) => source.collections().await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.collections().await,
            Self::External(source) => source.collections().await,
        }
    }

    async fn quarantined_diagnostics(
        &self,
    ) -> Result<Vec<SkillQuarantineDiagnostic>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.quarantined_diagnostics().await,
            Self::Filesystem(source) => source.quarantined_diagnostics().await,
            Self::Git(source) => source.quarantined_diagnostics().await,
            Self::Memory(source) => source.quarantined_diagnostics().await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.quarantined_diagnostics().await,
            Self::External(source) => source.quarantined_diagnostics().await,
        }
    }

    async fn health_snapshot(
        &self,
    ) -> Result<SourceHealthSnapshot, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.health_snapshot().await,
            Self::Filesystem(source) => source.health_snapshot().await,
            Self::Git(source) => source.health_snapshot().await,
            Self::Memory(source) => source.health_snapshot().await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.health_snapshot().await,
            Self::External(source) => source.health_snapshot().await,
        }
    }

    async fn list_artifacts(
        &self,
        id: &SkillId,
    ) -> Result<Vec<SkillArtifact>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.list_artifacts(id).await,
            Self::Filesystem(source) => source.list_artifacts(id).await,
            Self::Git(source) => source.list_artifacts(id).await,
            Self::Memory(source) => source.list_artifacts(id).await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.list_artifacts(id).await,
            Self::External(source) => source.list_artifacts(id).await,
        }
    }

    async fn read_artifact(
        &self,
        id: &SkillId,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.read_artifact(id, artifact_path).await,
            Self::Filesystem(source) => source.read_artifact(id, artifact_path).await,
            Self::Git(source) => source.read_artifact(id, artifact_path).await,
            Self::Memory(source) => source.read_artifact(id, artifact_path).await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.read_artifact(id, artifact_path).await,
            Self::External(source) => source.read_artifact(id, artifact_path).await,
        }
    }

    async fn invoke_function(
        &self,
        id: &SkillId,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.invoke_function(id, function_name, arguments).await,
            Self::Filesystem(source) => source.invoke_function(id, function_name, arguments).await,
            Self::Git(source) => source.invoke_function(id, function_name, arguments).await,
            Self::Memory(source) => source.invoke_function(id, function_name, arguments).await,
            #[cfg(any(feature = "skills-http", test))]
            Self::Http(source) => source.invoke_function(id, function_name, arguments).await,
            Self::External(source) => source.invoke_function(id, function_name, arguments).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn source_node_embedded_dispatches_concretely() {
        let node = SourceNode::Embedded(EmbeddedSkillSource::new());
        let _ = node.list(&SkillFilter::default()).await.unwrap();
    }

    #[tokio::test]
    async fn source_node_external_dispatches_through_bounded_external_client() {
        let response_script = r##"
read line
if echo "$line" | grep -q '"method":"capabilities/get"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"capabilities/get","result":{"protocol_version":1,"methods":["skills/list_summaries","skills/load_package"]}}}'
elif echo "$line" | grep -q '"method":"skills/list_summaries"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/list_summaries","result":{"summaries":[{"source_uuid":"ext-src","skill_name":"external-skill","description":"external"}]}}}'
elif echo "$line" | grep -q '"method":"skills/load_package"'; then
  echo '{"jsonrpc":"2.0","id":"1","payload":{"method":"skills/load_package","result":{"package":{"summary":{"source_uuid":"ext-src","skill_name":"external-skill","description":"external"},"body":"external body"}}}}'
fi
"##;
        let client = StdioExternalClient::new(
            "sh",
            vec!["-c".to_string(), response_script.to_string()],
            BTreeMap::new(),
            None,
        );
        let external = ExternalSkillSource::new("ext-src", client);
        let node = SourceNode::External(external);
        let listed = node.list(&SkillFilter::default()).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id.0, "external-skill");

        let loaded = node
            .load(&SkillId("external-skill".to_string()))
            .await
            .unwrap();
        assert_eq!(loaded.descriptor.id.0, "external-skill");
    }
}
