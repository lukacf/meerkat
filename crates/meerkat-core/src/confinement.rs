//! Mechanical requirements for an already-authorized process launch.
//!
//! These values do not authenticate a caller, grant permission, or supervise a
//! process. The existing operation owner supplies them to an OS adapter. A
//! backend must enforce the complete requirement or refuse that operation.
//! Confinement does not establish semantic confidentiality of model output.

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A literal file or directory, or a directory and its descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "path",
    rename_all = "snake_case",
    from = "PathAccessWire"
)]
pub enum PathAccess {
    Literal(PathBuf),
    Subtree(PathBuf),
}

impl PathAccess {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Literal(path) | Self::Subtree(path) => path,
        }
    }
}

/// An empty path list grants no access of this class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "paths",
    rename_all = "snake_case",
    from = "FilesystemAccessWire"
)]
pub enum FilesystemAccess {
    Unrestricted,
    Paths(Vec<PathAccess>),
}

/// IP access is independent of Unix sockets and platform service IPC.
/// Endpoints are resolved by the trusted host, never from child proxy settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "endpoints",
    rename_all = "snake_case",
    from = "IpNetworkAccessWire"
)]
pub enum IpNetworkAccess {
    Denied,
    Unrestricted,
    Connect(Vec<SocketAddr>),
}

// Decode through closed struct variants while preserving the public adjacent-tag
// serialization. Empty struct variants reject even reserved content set to null;
// adjacent-tag unit variants can otherwise discard that unsupported content.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum PathAccessWire {
    Literal { path: PathBuf },
    Subtree { path: PathBuf },
}

impl From<PathAccessWire> for PathAccess {
    fn from(value: PathAccessWire) -> Self {
        match value {
            PathAccessWire::Literal { path } => Self::Literal(path),
            PathAccessWire::Subtree { path } => Self::Subtree(path),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FilesystemAccessWire {
    Unrestricted {},
    Paths { paths: Vec<PathAccess> },
}

impl From<FilesystemAccessWire> for FilesystemAccess {
    fn from(value: FilesystemAccessWire) -> Self {
        match value {
            FilesystemAccessWire::Unrestricted {} => Self::Unrestricted,
            FilesystemAccessWire::Paths { paths } => Self::Paths(paths),
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum IpNetworkAccessWire {
    Denied {},
    Unrestricted {},
    Connect { endpoints: Vec<SocketAddr> },
}

impl From<IpNetworkAccessWire> for IpNetworkAccess {
    fn from(value: IpNetworkAccessWire) -> Self {
        match value {
            IpNetworkAccessWire::Denied {} => Self::Denied,
            IpNetworkAccessWire::Unrestricted {} => Self::Unrestricted,
            IpNetworkAccessWire::Connect { endpoints } => Self::Connect(endpoints),
        }
    }
}

/// Explicit compatibility privileges, enumerated by each backend's policy.
/// There is no ambient home-directory, scratch, network, or credential grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformBaseline {
    /// Executable loaders, system libraries, CPU/OS queries and null/random
    /// devices. User data and optional service IPC require explicit grants.
    CommandRuntimeV1,
}

/// Host configuration before structural validation. Deserializing this value
/// supplies requirements only; it never supplies caller authorization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfinementSpec {
    pub baseline: PlatformBaseline,
    pub read: FilesystemAccess,
    pub write: FilesystemAccess,
    /// Exclusions take precedence over all grants, including the baseline.
    pub deny_read: Vec<PathAccess>,
    pub deny_write: Vec<PathAccess>,
    pub network: IpNetworkAccess,
    /// Exact local socket paths or explicitly granted socket directories.
    pub unix_connect: Vec<PathAccess>,
    /// Some backends cannot prove termination of descendants that leave their
    /// process group. Such a backend must refuse when this is required.
    pub require_descendant_termination: bool,
}

/// Validated immutable requirements. Final host-path and backend validation
/// belongs to the adapter at preparation, not this platform-neutral contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ConfinementSpec", into = "ConfinementSpec")]
pub struct ExecutionConfinement(ConfinementSpec);

impl ExecutionConfinement {
    #[must_use]
    pub fn specification(&self) -> &ConfinementSpec {
        &self.0
    }
}

impl From<ExecutionConfinement> for ConfinementSpec {
    fn from(value: ExecutionConfinement) -> Self {
        value.0
    }
}

impl TryFrom<ConfinementSpec> for ExecutionConfinement {
    type Error = ConfinementRefusal;

    fn try_from(spec: ConfinementSpec) -> Result<Self, Self::Error> {
        fn paths(access: &FilesystemAccess) -> &[PathAccess] {
            match access {
                FilesystemAccess::Unrestricted => &[],
                FilesystemAccess::Paths(paths) => paths.as_slice(),
            }
        }
        for access in paths(&spec.read)
            .iter()
            .chain(paths(&spec.write))
            .chain(&spec.deny_read)
            .chain(&spec.deny_write)
            .chain(&spec.unix_connect)
        {
            let path = access.path();
            if !path.is_absolute()
                || path
                    .components()
                    .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
                || path.as_os_str().as_encoded_bytes().contains(&0)
            {
                return Err(ConfinementRefusal::InvalidRequirement);
            }
        }
        if let IpNetworkAccess::Connect(endpoints) = &spec.network
            && endpoints
                .iter()
                .any(|endpoint| endpoint.port() == 0 || endpoint.ip().is_unspecified())
        {
            return Err(ConfinementRefusal::InvalidRequirement);
        }
        Ok(Self(spec))
    }
}

/// Bounded operation-local diagnostics. Never expose an environment value,
/// credential path, gate token, or secret-bearing command line in this error.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum ConfinementRefusal {
    #[error("invalid execution confinement requirement")]
    InvalidRequirement,
    #[error("invalid confined process launch")]
    InvalidLaunch,
    #[error("required execution confinement is unsupported by this backend")]
    UnsupportedRequirement,
    #[error("required execution confinement backend is unavailable")]
    BackendUnavailable,
    #[error("confined process preparation failed")]
    PreparationFailed,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn spec() -> ConfinementSpec {
        ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![]),
            write: FilesystemAccess::Paths(vec![]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
    }

    #[test]
    fn requirements_do_not_add_ambient_grants() {
        let expected = spec();
        let validated = ExecutionConfinement::try_from(expected.clone()).unwrap();
        assert_eq!(validated.specification(), &expected);
    }

    #[test]
    fn deserialization_cannot_skip_path_validation() {
        let mut invalid = spec();
        invalid.write =
            FilesystemAccess::Paths(vec![PathAccess::Subtree(PathBuf::from("relative"))]);
        let encoded = serde_json::to_string(&invalid).unwrap();
        assert!(serde_json::from_str::<ExecutionConfinement>(&encoded).is_err());
    }

    #[test]
    fn connect_requires_an_exact_destination() {
        let mut invalid = spec();
        invalid.network = IpNetworkAccess::Connect(vec!["0.0.0.0:443".parse().unwrap()]);
        assert_eq!(
            ExecutionConfinement::try_from(invalid),
            Err(ConfinementRefusal::InvalidRequirement)
        );
    }

    #[test]
    fn decoder_preserves_existing_variant_wire_shapes() {
        fn roundtrip<T>(value: T, expected: serde_json::Value)
        where
            T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
        {
            assert_eq!(serde_json::to_value(&value).unwrap(), expected);
            assert_eq!(serde_json::from_value::<T>(expected).unwrap(), value);
        }
        roundtrip(
            PathAccess::Literal("/work".into()),
            serde_json::json!({"kind": "literal", "path": "/work"}),
        );
        roundtrip(
            PathAccess::Subtree("/work".into()),
            serde_json::json!({"kind": "subtree", "path": "/work"}),
        );
        roundtrip(
            FilesystemAccess::Unrestricted,
            serde_json::json!({"kind": "unrestricted"}),
        );
        roundtrip(
            FilesystemAccess::Paths(vec![]),
            serde_json::json!({"kind": "paths", "paths": []}),
        );
        roundtrip(
            IpNetworkAccess::Denied,
            serde_json::json!({"kind": "denied"}),
        );
        roundtrip(
            IpNetworkAccess::Unrestricted,
            serde_json::json!({"kind": "unrestricted"}),
        );
        roundtrip(
            IpNetworkAccess::Connect(vec!["127.0.0.1:443".parse().unwrap()]),
            serde_json::json!({"kind": "connect", "endpoints": ["127.0.0.1:443"]}),
        );
    }

    fn decode_override(field: &str, value: serde_json::Value) -> bool {
        let mut encoded = serde_json::to_value(spec()).unwrap();
        encoded[field] = value;
        serde_json::from_value::<ExecutionConfinement>(encoded).is_ok()
    }

    #[test]
    fn decoder_rejects_nested_path_extensions_instead_of_widening_access() {
        for kind in ["literal", "subtree"] {
            let allowed = serde_json::json!({ "kind": kind, "path": "/work" });
            assert!(decode_override(
                "read",
                serde_json::json!({ "kind": "paths", "paths": [allowed] }),
            ));
            let unsupported = serde_json::json!({
                "kind": kind,
                "path": "/work",
                "except": ["/work/secret"]
            });
            assert!(!decode_override(
                "read",
                serde_json::json!({ "kind": "paths", "paths": [unsupported] }),
            ));
        }
    }

    #[test]
    fn decoder_rejects_nested_filesystem_extensions_instead_of_ignoring_them() {
        assert!(decode_override(
            "read",
            serde_json::json!({ "kind": "paths", "paths": [] }),
        ));
        assert!(!decode_override(
            "read",
            serde_json::json!({ "kind": "paths", "paths": [], "future_policy": true }),
        ));
    }

    #[test]
    fn decoder_rejects_nested_network_extensions_instead_of_ignoring_them() {
        assert!(decode_override(
            "network",
            serde_json::json!({ "kind": "connect", "endpoints": ["127.0.0.1:443"] }),
        ));
        assert!(!decode_override(
            "network",
            serde_json::json!({
                "kind": "connect",
                "endpoints": ["127.0.0.1:443"],
                "future_policy": true
            }),
        ));
    }

    #[test]
    fn decoder_rejects_reserved_filesystem_unit_content_even_when_null() {
        assert!(decode_override(
            "read",
            serde_json::json!({ "kind": "unrestricted" }),
        ));
        assert!(!decode_override(
            "read",
            serde_json::json!({ "kind": "unrestricted", "paths": null }),
        ));
    }

    #[test]
    fn decoder_rejects_reserved_network_unit_content_even_when_null() {
        for kind in ["denied", "unrestricted"] {
            assert!(decode_override(
                "network",
                serde_json::json!({ "kind": kind })
            ));
            assert!(!decode_override(
                "network",
                serde_json::json!({ "kind": kind, "endpoints": null }),
            ));
        }
    }

    #[test]
    fn decoder_rejects_invalid_paths_in_every_path_collection() {
        for path in ["relative", "/work/../secret", "/work/\u{0}secret"] {
            for field in ["read", "write"] {
                assert!(!decode_override(
                    field,
                    serde_json::json!({
                        "kind": "paths",
                        "paths": [{ "kind": "subtree", "path": path }]
                    }),
                ));
            }
            for field in ["deny_read", "deny_write", "unix_connect"] {
                assert!(!decode_override(
                    field,
                    serde_json::json!([{ "kind": "literal", "path": path }]),
                ));
            }
        }
    }

    #[test]
    fn decoder_rejects_unspecified_and_zero_port_network_endpoints() {
        for endpoint in ["0.0.0.0:443", "[::]:443", "127.0.0.1:0", "[::1]:0"] {
            assert!(!decode_override(
                "network",
                serde_json::json!({ "kind": "connect", "endpoints": [endpoint] }),
            ));
        }
        for endpoint in ["127.0.0.1:443", "[::1]:443"] {
            assert!(decode_override(
                "network",
                serde_json::json!({ "kind": "connect", "endpoints": [endpoint] }),
            ));
        }
    }
}
