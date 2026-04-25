//! Skill sources.

pub mod composite;
pub mod embedded;
#[cfg(not(target_arch = "wasm32"))]
pub mod filesystem;
#[cfg(not(target_arch = "wasm32"))]
pub mod git;
#[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
pub mod http;
pub mod memory;
#[cfg(not(target_arch = "wasm32"))]
pub mod protocol;
pub(crate) mod remote;

use meerkat_core::skills::{
    SkillArtifact, SkillArtifactContent, SkillCollection, SkillDescriptor, SkillDocument,
    SkillFilter, SkillIntrospectionEntry, SkillKey, SkillQuarantineDiagnostic, SkillSource,
    SourceHealthSnapshot,
};

pub use composite::{CompositeSkillSource, NamedSource};
pub use embedded::EmbeddedSkillSource;
#[cfg(not(target_arch = "wasm32"))]
pub use filesystem::FilesystemSkillSource;
#[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
pub use http::HttpSkillSource;
pub use memory::InMemorySkillSource;
#[cfg(not(target_arch = "wasm32"))]
pub use protocol::{ExternalSkillSource, StdioExternalClient};

/// Typed source composition node for built-ins + bounded external seam.
pub enum SourceNode {
    Embedded(EmbeddedSkillSource),
    #[cfg(not(target_arch = "wasm32"))]
    Filesystem(FilesystemSkillSource),
    #[cfg(not(target_arch = "wasm32"))]
    Git(Box<git::GitSkillSource>),
    Memory(InMemorySkillSource),
    #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
    Http(Box<HttpSkillSource>),
    #[cfg(not(target_arch = "wasm32"))]
    External(ExternalSkillSource<StdioExternalClient>),
}

impl SkillSource for SourceNode {
    async fn list(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillDescriptor>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.list(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.list(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.list(filter).await,
            Self::Memory(source) => source.list(filter).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.list(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.list(filter).await,
        }
    }

    async fn load(
        &self,
        key: &SkillKey,
    ) -> Result<SkillDocument, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.load(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.load(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.load(key).await,
            Self::Memory(source) => source.load(key).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.load(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.load(key).await,
        }
    }

    async fn collections(&self) -> Result<Vec<SkillCollection>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.collections().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.collections().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.collections().await,
            Self::Memory(source) => source.collections().await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.collections().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.collections().await,
        }
    }

    async fn quarantined_diagnostics(
        &self,
    ) -> Result<Vec<SkillQuarantineDiagnostic>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.quarantined_diagnostics().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.quarantined_diagnostics().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.quarantined_diagnostics().await,
            Self::Memory(source) => source.quarantined_diagnostics().await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.quarantined_diagnostics().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.quarantined_diagnostics().await,
        }
    }

    async fn health_snapshot(
        &self,
    ) -> Result<SourceHealthSnapshot, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.health_snapshot().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.health_snapshot().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.health_snapshot().await,
            Self::Memory(source) => source.health_snapshot().await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.health_snapshot().await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.health_snapshot().await,
        }
    }

    async fn list_artifacts(
        &self,
        key: &SkillKey,
    ) -> Result<Vec<SkillArtifact>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.list_artifacts(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.list_artifacts(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.list_artifacts(key).await,
            Self::Memory(source) => source.list_artifacts(key).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.list_artifacts(key).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.list_artifacts(key).await,
        }
    }

    async fn read_artifact(
        &self,
        key: &SkillKey,
        artifact_path: &str,
    ) -> Result<SkillArtifactContent, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.read_artifact(key, artifact_path).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.read_artifact(key, artifact_path).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.read_artifact(key, artifact_path).await,
            Self::Memory(source) => source.read_artifact(key, artifact_path).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.read_artifact(key, artifact_path).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.read_artifact(key, artifact_path).await,
        }
    }

    async fn invoke_function(
        &self,
        key: &SkillKey,
        function_name: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.invoke_function(key, function_name, arguments).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.invoke_function(key, function_name, arguments).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.invoke_function(key, function_name, arguments).await,
            Self::Memory(source) => source.invoke_function(key, function_name, arguments).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.invoke_function(key, function_name, arguments).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.invoke_function(key, function_name, arguments).await,
        }
    }

    async fn list_all_with_provenance(
        &self,
        filter: &SkillFilter,
    ) -> Result<Vec<SkillIntrospectionEntry>, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.list_all_with_provenance(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.list_all_with_provenance(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.list_all_with_provenance(filter).await,
            Self::Memory(source) => source.list_all_with_provenance(filter).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.list_all_with_provenance(filter).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.list_all_with_provenance(filter).await,
        }
    }

    async fn load_from_source(
        &self,
        key: &SkillKey,
        source_name: Option<&str>,
    ) -> Result<SkillDocument, meerkat_core::skills::SkillError> {
        match self {
            Self::Embedded(source) => source.load_from_source(key, source_name).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Filesystem(source) => source.load_from_source(key, source_name).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::Git(source) => source.load_from_source(key, source_name).await,
            Self::Memory(source) => source.load_from_source(key, source_name).await,
            #[cfg(all(any(feature = "skills-http", test), not(target_arch = "wasm32")))]
            Self::Http(source) => source.load_from_source(key, source_name).await,
            #[cfg(not(target_arch = "wasm32"))]
            Self::External(source) => source.load_from_source(key, source_name).await,
        }
    }
}
