#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceHandle, SurfaceDiagnosticSnapshot, SurfaceSnapshot,
};
use meerkat_core::tool_scope::ExternalToolSurfaceGlobalPhase;
use meerkat_mcp::external_tool_surface_authority::{
    ExternalToolSurfaceEffect, SurfaceDeltaOperation, SurfaceId, TurnNumber,
};
use meerkat_mcp::generated::protocol_surface_completion::{
    extract_obligations as extract_completion_obligations, submit_pending_failed_handle,
    submit_pending_succeeded_handle,
};
use meerkat_mcp::generated::protocol_surface_snapshot_alignment::{
    extract_obligations as extract_snapshot_obligations, submit_snapshot_aligned,
};

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/generated")
}

#[derive(Default)]
struct RecordingSurfaceHandle {
    pending_succeeded: Mutex<Vec<(String, u64, u64)>>,
    pending_failed: Mutex<Vec<(String, String)>>,
    snapshot_epochs: Mutex<Vec<u64>>,
}

impl ExternalToolSurfaceHandle for RecordingSurfaceHandle {
    fn register(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn stage_add(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn stage_remove(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn stage_reload(&self, _surface_id: String, _now_ms: u64) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn apply_boundary(
        &self,
        _surface_id: String,
        _now_ms: u64,
        _current_turn: u64,
    ) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn mark_pending_succeeded(
        &self,
        surface_id: String,
        pending_task_sequence: u64,
        staged_intent_sequence: u64,
    ) -> Result<(), DslTransitionError> {
        self.pending_succeeded.lock().expect("lock").push((
            surface_id,
            pending_task_sequence,
            staged_intent_sequence,
        ));
        Ok(())
    }

    fn mark_pending_failed(
        &self,
        surface_id: String,
        reason: String,
    ) -> Result<(), DslTransitionError> {
        self.pending_failed
            .lock()
            .expect("lock")
            .push((surface_id, reason));
        Ok(())
    }

    fn call_started(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn call_finished(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn finalize_removal_clean(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn finalize_removal_forced(&self, _surface_id: String) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn snapshot_aligned(&self, epoch: u64) -> Result<(), DslTransitionError> {
        self.snapshot_epochs.lock().expect("lock").push(epoch);
        Ok(())
    }

    fn shutdown_surface(&self) -> Result<(), DslTransitionError> {
        Ok(())
    }

    fn surface_snapshot(&self, _surface_id: &str) -> Option<SurfaceSnapshot> {
        None
    }

    fn diagnostic_snapshot(&self) -> SurfaceDiagnosticSnapshot {
        SurfaceDiagnosticSnapshot {
            surface_phase: ExternalToolSurfaceGlobalPhase::Operating,
            known_surfaces: BTreeSet::new(),
            visible_surfaces: BTreeSet::new(),
            snapshot_epoch: 0,
            snapshot_aligned_epoch: self.snapshot_aligned_epoch(),
            has_pending_or_staged: false,
            entries: vec![],
        }
    }

    fn visible_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn removing_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn pending_surfaces(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }

    fn has_pending_or_staged(&self) -> bool {
        false
    }

    fn snapshot_epoch(&self) -> u64 {
        0
    }

    fn snapshot_aligned_epoch(&self) -> u64 {
        self.snapshot_epochs
            .lock()
            .expect("lock")
            .last()
            .copied()
            .unwrap_or(0)
    }
}

#[test]
fn surface_completion_generated_helpers_are_typed() {
    let path = generated_dir().join("protocol_surface_completion.rs");
    let source = fs::read_to_string(&path).expect("protocol_surface_completion");
    for forbidden in ["KernelState", "KernelInput", "KernelEffect", "KernelValue"] {
        assert!(
            !source.contains(forbidden),
            "{} should not contain `{forbidden}`",
            path.display()
        );
    }
    assert!(
        source.contains("SurfaceCompletionObligation"),
        "{} should expose a typed obligation",
        path.display()
    );
}

#[test]
fn surface_completion_round_trips_typed_values() {
    let effects = vec![ExternalToolSurfaceEffect::ScheduleSurfaceCompletion {
        surface_id: SurfaceId::from("surface-a"),
        operation: SurfaceDeltaOperation::Reload,
        pending_task_sequence: 7,
        staged_intent_sequence: 11,
        applied_at_turn: TurnNumber(13),
    }];
    let obligations = extract_completion_obligations(&effects);
    let obligation = obligations.into_iter().next().expect("obligation");

    let handle = RecordingSurfaceHandle::default();
    submit_pending_succeeded_handle(&handle, obligation.clone()).expect("pending_succeeded");
    submit_pending_failed_handle(&handle, obligation, "reload failed").expect("pending_failed");

    assert_eq!(
        handle.pending_succeeded.lock().expect("lock").as_slice(),
        &[("surface-a".into(), 7, 11)]
    );
    assert_eq!(
        handle.pending_failed.lock().expect("lock").as_slice(),
        &[("surface-a".into(), "reload failed".into())]
    );
}

#[test]
fn surface_snapshot_alignment_generated_helpers_are_typed() {
    let path = generated_dir().join("protocol_surface_snapshot_alignment.rs");
    let source = fs::read_to_string(&path).expect("protocol_surface_snapshot_alignment");
    for forbidden in ["KernelState", "KernelInput", "KernelEffect", "KernelValue"] {
        assert!(
            !source.contains(forbidden),
            "{} should not contain `{forbidden}`",
            path.display()
        );
    }
    assert!(
        source.contains("SurfaceSnapshotAlignmentObligation"),
        "{} should expose a typed obligation",
        path.display()
    );
}

#[test]
fn surface_snapshot_alignment_round_trips_typed_values() {
    let effects = vec![ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch: 42 }];
    let obligations = extract_snapshot_obligations(&effects);
    let obligation = obligations.into_iter().next().expect("obligation");

    let handle = RecordingSurfaceHandle::default();
    submit_snapshot_aligned(&handle, obligation).expect("snapshot_aligned");

    assert_eq!(
        handle.snapshot_epochs.lock().expect("lock").as_slice(),
        &[42]
    );
}
