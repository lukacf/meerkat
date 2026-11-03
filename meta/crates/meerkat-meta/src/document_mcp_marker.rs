//! The public marker of a host-bound document MCP descriptor, known to every
//! build.
//!
//! A host's document MCP binding (`extension_state::HostMcpDocumentBinding`,
//! behind the `extension-state` feature) puts this marker into the public
//! stdio descriptor that profiles persist. Before a member is built, the
//! binding replaces each marked descriptor it resolves with its host-only
//! runtime twin, which never carries the marker. A marked descriptor still
//! in a member's build was resolved by no binding, and launching it would
//! run the document service without the host's endpoint and credentials. So
//! every build refuses it, with or without the feature.

use meerkat_core::McpServerConfig;
use meerkat_core::error::AgentError;
use meerkat_core::mcp_config::McpTransportConfig;
use meerkat_core::service::{CreateSessionRequest, SessionError};

use crate::storage_predecessor::PREDECESSOR_DOCUMENT_BINDING_ENV;

/// Public launch marker, not a credential or a caller identity. An unresolved
/// marked descriptor always refuses instead of launching an unbound service.
pub const DOCUMENT_BINDING_ENV: &str = "META_MCP_DOCUMENT_BINDING";

/// Whether `config` carries the marker, in its current spelling or in its
/// predecessor (pre-0.9) one.
pub(crate) fn is_marked(config: &McpServerConfig) -> bool {
    matches!(&config.transport, McpTransportConfig::Stdio(stdio)
        if stdio.env.contains_key(DOCUMENT_BINDING_ENV)
            || stdio.env.contains_key(PREDECESSOR_DOCUMENT_BINDING_ENV))
}

/// Refuse a member build that still carries a marked descriptor. Runs after
/// the host's pre-build hook, which resolves every descriptor a document
/// binding owns.
pub(crate) fn refuse_unresolved_marked_descriptors(
    request: &CreateSessionRequest,
) -> Result<(), SessionError> {
    let Some(build) = request.build.as_ref() else {
        return Ok(());
    };
    match build.mcp_servers.iter().find(|config| is_marked(config)) {
        Some(config) => Err(SessionError::Agent(AgentError::InternalError(format!(
            "MCP server '{}' is a document MCP descriptor that no host document binding \
             resolved; it is refused instead of launched without the host's binding",
            config.name
        )))),
        None => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn stdio(env: &[(&str, &str)]) -> McpServerConfig {
        McpServerConfig::stdio(
            "documents",
            "document-fixture-mcp",
            vec![],
            env.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<HashMap<_, _>>(),
        )
    }

    #[test]
    fn extension_document_marker_is_known_in_both_spellings() {
        assert!(is_marked(&stdio(&[(DOCUMENT_BINDING_ENV, "documents")])));
        assert!(is_marked(&stdio(&[(
            PREDECESSOR_DOCUMENT_BINDING_ENV,
            "documents"
        )])));
        assert!(!is_marked(&stdio(&[("DOCUMENT_TOKEN", "")])));
        // The marker is a stdio launch environment entry only.
        assert!(!is_marked(&McpServerConfig::streamable_http(
            "documents",
            "http://127.0.0.1:1/mcp",
            HashMap::from([(DOCUMENT_BINDING_ENV.to_string(), "documents".to_string())]),
        )));
    }
}
