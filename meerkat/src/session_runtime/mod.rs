//! Surface-agnostic session runtime — the canonical orchestrator that
//! glues `PersistentSessionService`, `StagedSessionRegistry`,
//! `MeerkatMachine`, and the optional `LiveAdapterHost` together.
//!
//! Every Meerkat surface composes this crate (Rust SDK first, then
//! `rkat-rpc` for the Python/TS SDKs, then CLI / REST / MCP-server /
//! web SDK / embedded `examples/*`). Nothing here imports `RpcError`,
//! `RpcResponse`, axum types, or any other wire type — surfaces map
//! the typed errors in [`errors`] onto their own wire shapes.
//!
//! See `SESSION_RUNTIME_SPLIT_TODO.md` for the migration plan.

pub mod admission;
pub mod errors;
pub mod live_orchestration;
pub mod recovery;
pub mod runtime_state;
pub mod staged_promotion;

pub use errors::LiveOpenPrecheckError;
pub use runtime_state::{SessionInfo, SessionState};

/// Surface-agnostic session runtime.
///
/// Currently empty — populated by the Wave-1/2/3 moves out of
/// `meerkat-rpc::session_runtime::SessionRuntime`. Every method added
/// here MUST satisfy the V5 surface-symmetry audit: callable from the
/// Rust SDK, REST handler, CLI subcommand, and embedded example
/// without smuggling RPC wire types.
pub struct MeerkatSessionRuntime {
    // Fields land in W3-B / W3-C; intentionally empty during F2 to
    // make the move path explicit.
    _placeholder: (),
}

impl MeerkatSessionRuntime {
    /// Builder entry point. The full builder API lands in W3-C.
    #[must_use]
    pub fn builder() -> SessionRuntimeBuilder {
        SessionRuntimeBuilder::default()
    }
}

/// Constructor builder for [`MeerkatSessionRuntime`].
///
/// Empty during F2; populated by W3-C with `with_live_adapter_host`,
/// `with_config_runtime`, `with_default_llm_client`, `with_realm_id`,
/// `with_instance_id`, `with_backend`, `with_skill_identity_registry`.
#[derive(Default)]
pub struct SessionRuntimeBuilder {
    _placeholder: (),
}

impl SessionRuntimeBuilder {
    /// Finalize the builder. Returns the populated runtime.
    ///
    /// Panics during F2 — every surface still constructs the
    /// runtime via `meerkat-rpc::session_runtime::SessionRuntime`.
    /// Wave 4 rewires the constructors.
    #[must_use]
    pub fn build(self) -> MeerkatSessionRuntime {
        MeerkatSessionRuntime { _placeholder: () }
    }
}
