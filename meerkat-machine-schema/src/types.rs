//! Typed-ID newtypes shared by catalog DSL machines.
//!
//! These are stub-grade identifiers used by the catalog DSL (TLC-facing twin
//! of the runtime machines). They intentionally mirror the shape of the "real"
//! typed IDs elsewhere in the workspace (e.g. `meerkat_core::SessionId`,
//! `meerkat_mob::ids::AgentRuntimeId`) without taking a dependency on those
//! crates — the catalog DSL is a leaf crate.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<T: Into<String>> From<T> for $name {
            fn from(value: T) -> Self {
                Self(value.into())
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

macro_rules! u64_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

// Unique identifier for a configured MCP server.
//
// Keyed on the server's configured name (matches `.rkat/mcp.toml` section
// headers). The DSL uses this as the key of the `mcp_server_states` map on
// `MeerkatMachine` state so the `[MCP_PENDING]` system-notice toggle is a
// pure read off DSL-owned state.
string_newtype!(McpServerId);

// Opaque correlation identifier for the peer request / response lifecycle.
//
// Catalog-DSL twin of [`meerkat_core::PeerCorrelationId`]. Carried on all
// W1-A peer-interaction inputs and effects; used as the key of the DSL's
// `pending_peer_requests` and `inbound_peer_requests` substate maps so the
// subscriber / stream registries can project deterministically off DSL state.
string_newtype!(PeerCorrelationId);

// Stable identity of a comms runtime instance.
//
// Catalog-DSL identity for an `Arc<dyn CommsRuntime>` pointer so the DSL can
// distinguish distinct runtime instances and reject silent transport
// downgrades (W2-G / issue #264). The runtime derives the string from the
// `Arc`'s pointer address; the catalog DSL treats it as an opaque newtype.
string_newtype!(CommsRuntimeId);

// Mob instance identifier.
//
// Catalog-DSL twin of [`meerkat_mob::ids::MobId`] carried on
// `AttachMobIngress` to record which mob owns a peer-ingress transport
// capability. The runtime mirrors the real typed ID; catalog DSL keeps it
// stub-grade since this crate is a leaf.
string_newtype!(MobId);

// ---------------------------------------------------------------------------
// Shared stub/twin types for canonical and compat machine surfaces
// ---------------------------------------------------------------------------

string_newtype!(FlowId);
string_newtype!(RunId);
string_newtype!(SessionId);
string_newtype!(AgentIdentity);
string_newtype!(AgentRuntimeId);
string_newtype!(MeerkatId);
string_newtype!(ProfileName);
string_newtype!(StepId);
string_newtype!(BranchId);
string_newtype!(FrameId);
string_newtype!(LoopInstanceId);
string_newtype!(FlowNodeId);
string_newtype!(LoopId);
string_newtype!(TaskId);
string_newtype!(WorkId);

u64_newtype!(Generation);
u64_newtype!(FenceToken);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum FlowRunStatus {
    #[default]
    Absent,
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum StepRunStatus {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum DependencyMode {
    #[default]
    All,
    Any,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub enum CollectionPolicyKind {
    #[default]
    All,
    Any,
    Quorum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FlowNodeKind {
    Step,
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum FrameScope {
    Root,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum NodeRunStatus {
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum LoopIterationStage {
    AwaitingBodyFrame,
    BodyFrameActive,
    AwaitingUntil,
}
