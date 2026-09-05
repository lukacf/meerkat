During the pre-push gate for #1096 (branch fix/release-doctor-drift @96aacef33, which changes only release-doctor scripts and workflow-contract tests, stacked on the merged #1095 tree), the `workspace deterministic unit + integration + e2e gate` failed 12 tests at once while the VM's 1-minute load average was ~257 on 192 cores (five worktrees compiling from cold in parallel). The identical meerkat-mob/meerkat code had passed the same gate an hour earlier for #1095. Re-push at low load is in progress; if it passes, these are load-sensitive tests, and the goal is to un-flake them (tighter synchronisation, not longer timeouts).

```
     Summary [ 205.432s] 2523 tests run: 2511 passed (9 slow), 12 failed, 72 skipped
        FAIL [  23.930s] meerkat::cold_restart_resume_after_compaction tests::cold_restart_resume_after_compaction_incremental_head_representation
        FAIL [  24.475s] meerkat::cold_restart_resume_after_compaction tests::rewrite_audit_append_failure_preserves_compaction_outbox_for_idempotent_recovery
        FAIL [  36.520s] meerkat-mob::cold_restart_mob_resume mob_cold_restart_resume_after_kill_between_commit_points
        FAIL [  33.870s] meerkat-mob::cold_restart_mob_resume mob_retirement_recovers_reload_required_runtime_before_normal_archive
        FAIL [  26.696s] meerkat-mob::cross_host_events empty_same_session_resume_page_advances_real_pump_to_resolved_floor
        FAIL [  17.011s] meerkat-mob::host_bind_ceremony rebind_same_supervisor_identity_replaces_stale_address
        FAIL [  14.195s] meerkat-mob::host_materialize_serving executor_stop_between_ensure_and_attach_return_cleans_preinstalled_sidecar
        FAIL [  23.671s] meerkat-mob::host_materialize_serving host_status_marks_retired_registered_member_unhealthy
        FAIL [  23.901s] meerkat-mob::host_materialize_serving host_status_marks_stale_idle_registration_without_executor_unhealthy
        FAIL [  21.716s] meerkat-mob::host_materialize_serving host_status_marks_stopped_member_unhealthy_and_replay_repairs_it
        FAIL [  20.336s] meerkat-mob::host_materialize_serving materialize_identity_mismatch_preserves_durable_session_and_quiesces_both_identities
        FAIL [  10.513s] meerkat-store::conformance jsonl_store::baseline
error: test run failed
```

Failure messages (not all are plain timeouts; several are teardown-ordering races in the registration/unregister path):
```
    thread 'executor_stop_between_ensure_and_attach_return_cleans_preinstalled_sidecar' (3396670) panicked at meerkat-mob/tests/host_materialize_serving.rs:1899:6:
    failed attachment cleanup must not wait for the 30s reconciliation grace: Elapsed(())
    thread 'tests::cold_restart_resume_after_compaction_incremental_head_representation' (3347441) panicked at meerkat/tests/cold_restart_resume_after_compaction.rs:1347:14:
    prompt should complete in time: Elapsed(())
    thread 'rebind_same_supervisor_identity_replaces_stale_address' (3390700) panicked at meerkat-mob/tests/host_bind_ceremony.rs:683:10:
    materialize member before rebind: "timed out waiting for bridge reply"
    thread 'empty_same_session_resume_page_advances_real_pump_to_resolved_floor' (3355776) panicked at meerkat-mob/tests/cross_host_events.rs:302:10:
    quiesce generic seed attachment before host-owned explicit resume: UnregisterInProgress { runtime_id: LogicalRuntimeId("rt:session:01a06ed4-2c0c-7c51-ae45-4932639ae978") }
    thread 'materialize_identity_mismatch_preserves_durable_session_and_quiesces_both_identities' (3403343) panicked at meerkat-mob/tests/host_materialize_serving.rs:1828:9:
    actual mismatch identity must not retain runtime registration
    thread 'host_status_marks_stopped_member_unhealthy_and_replay_repairs_it' (3398457) panicked at meerkat-mob/tests/host_materialize_serving.rs:2432:10:
    stop materialized runtime executor: RuntimeStopInProgress { runtime_id: LogicalRuntimeId("rt:session:01a06ed4-484c-7522-bda1-c3fcf49ff4c7") }
    thread 'host_status_marks_retired_registered_member_unhealthy' (3398400) panicked at meerkat-mob/tests/host_materialize_serving.rs:2520:10:
    unregister retired runtime before replay repair: UnregisterInProgress { runtime_id: LogicalRuntimeId("rt:session:01a06ed4-5976-7f61-9454-67546d04a789") }
    thread 'host_status_marks_stale_idle_registration_without_executor_unhealthy' (3398476) panicked at meerkat-mob/tests/host_materialize_serving.rs:2827:10:
    unregister serving runtime before idle-registration probe: UnregisterInProgress { runtime_id: LogicalRuntimeId("rt:session:01a06ed4-5862-7492-9f3c-1bc652b5ffb1") }
  stderr ───
    thread 'mob_retirement_recovers_reload_required_runtime_before_normal_archive' (3355462) panicked at meerkat-mob/tests/cold_restart_mob_resume.rs:2322:10:
    ReloadRequired retirement must converge through normal archive: SharedRetirementFailure(SessionError(Agent(InternalError("failed exact terminal registration disposal for 01a06ed4-33da-7171-9aad-2efc42e867ed: Unregister teardown is still in progress for runtime rt:session:01a06ed4-33da-7171-9aad-2efc42e867ed"))))
```

Full gate log: /tmp/rb/push-1096-restack.log on the release VM (lines 85-240).
