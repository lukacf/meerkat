//! Runtime impl of [`meerkat_core::handles::ExternalToolSurfaceHandle`].

use std::sync::Arc;

use meerkat_core::handles::{DslTransitionError, ExternalToolSurfaceHandle};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`ExternalToolSurfaceHandle`] impl.
///
/// Routes every trait method to the corresponding DSL signal on a dedicated
/// per-session MeerkatMachine DSL authority. All external-tool-surface
/// transitions in the current DSL catalog are signals (parameterless); the
/// per-surface maps are populated via other DSL inputs upstream.
#[derive(Debug)]
pub struct RuntimeExternalToolSurfaceHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeExternalToolSurfaceHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    ///
    /// See [`RuntimeTurnStateHandle::ephemeral`].
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl ExternalToolSurfaceHandle for RuntimeExternalToolSurfaceHandle {
    fn stage_add(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StageAdd,
            "ExternalToolSurfaceHandle::stage_add",
        )
    }

    fn stage_remove(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StageRemove,
            "ExternalToolSurfaceHandle::stage_remove",
        )
    }

    fn stage_reload(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::StageReload,
            "ExternalToolSurfaceHandle::stage_reload",
        )
    }

    fn apply_surface_boundary(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::ApplySurfaceBoundary,
            "ExternalToolSurfaceHandle::apply_surface_boundary",
        )
    }

    fn pending_succeeded(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::PendingSucceeded,
            "ExternalToolSurfaceHandle::pending_succeeded",
        )
    }

    fn pending_failed(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::PendingFailed,
            "ExternalToolSurfaceHandle::pending_failed",
        )
    }

    fn call_started(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::CallStarted,
            "ExternalToolSurfaceHandle::call_started",
        )
    }

    fn call_finished(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::CallFinished,
            "ExternalToolSurfaceHandle::call_finished",
        )
    }

    fn finalize_removal_clean(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::FinalizeRemovalClean,
            "ExternalToolSurfaceHandle::finalize_removal_clean",
        )
    }

    fn finalize_removal_forced(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::FinalizeRemovalForced,
            "ExternalToolSurfaceHandle::finalize_removal_forced",
        )
    }

    fn snapshot_aligned(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::SnapshotAligned,
            "ExternalToolSurfaceHandle::snapshot_aligned",
        )
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        self.dsl.apply_signal(
            mm_dsl::MeerkatMachineSignal::ShutdownSurface,
            "ExternalToolSurfaceHandle::shutdown_surface",
        )
    }
}
