use super::*;

impl MobRuntime {
    pub(super) async fn supervisor_escalate(
        &self,
        mob_id: &MobId,
        spec: &MobSpec,
        run_id: &str,
        flow_id: &str,
        reason: &str,
        details: Value,
    ) -> MobResult<()> {
        let inflight_dispatches = self.inflight_dispatch_count().await;
        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Supervisor,
                MobEventKind::SupervisorEscalation,
            )
            .run_id(run_id)
            .flow_id(flow_id)
            .payload(json!({
                "reason": reason,
                "details": details.clone(),
                "inflight_dispatches": inflight_dispatches,
            }))
            .build(),
        )
        .await?;

        let Some(notify_role) = spec.supervisor.notify_role.as_deref() else {
            return Ok(());
        };

        let targets = self
            .resolve_step_targets(mob_id, spec, notify_role, "*", Value::Null)
            .await?;
        if targets.is_empty() {
            return Ok(());
        }

        let supervisor = self.supervisor_runtime(mob_id).await?;
        let params = json!({
            "mob_id": mob_id.clone(),
            "run_id": run_id,
            "flow_id": flow_id,
            "reason": reason,
            "details": details,
        });

        for target in targets {
            let command = CommsCommand::PeerRequest {
                to: meerkat::PeerName::new(target.comms_name.clone())
                    .map_err(MobError::Comms)?,
                intent: "mob.supervisor.escalation".to_string(),
                params: params.clone(),
                stream: InputStreamMode::None,
            };
            let _ = supervisor
                .send(command)
                .await
                .map_err(|err| MobError::Comms(err.to_string()))?;
        }

        Ok(())
    }

    pub(super) async fn current_reset_epoch(&self, mob_id: &MobId) -> u64 {
        let epochs = self.reset_epochs.read().await;
        epochs.get(mob_id).copied().unwrap_or(0)
    }

    pub(super) async fn bump_reset_epoch(&self, mob_id: &MobId) -> u64 {
        let mut epochs = self.reset_epochs.write().await;
        let next = epochs.get(mob_id).copied().unwrap_or(0) + 1;
        epochs.insert(mob_id.clone(), next);
        next
    }

    pub(super) async fn is_run_canceled(&self, run_id: &RunId) -> MobResult<bool> {
        if let Some(status) = self.cached_run_status(run_id).await {
            return Ok(status == MobRunStatus::Canceled);
        }

        let Some(run) = self.run_store.get_run(run_id.as_ref()).await? else {
            return Ok(false);
        };
        self.cache_run_status(run_id.clone(), run.status).await;
        Ok(run.status == MobRunStatus::Canceled)
    }

    pub(super) async fn ensure_run_active(
        &self,
        run_id: &RunId,
        mob_id: &MobId,
        expected_epoch: u64,
    ) -> MobResult<()> {
        let current_epoch = self.current_reset_epoch(mob_id).await;
        if current_epoch != expected_epoch {
            return Err(MobError::ResetBarrier {
                mob_id: mob_id.to_string(),
                expected: expected_epoch,
                current: current_epoch,
            });
        }

        if let Some(status) = self.cached_run_status(run_id).await {
            if !matches!(status, MobRunStatus::Pending | MobRunStatus::Running) {
                return Err(MobError::RunCanceled {
                    run_id: run_id.to_string(),
                });
            }
            return Ok(());
        }

        let run = self
            .run_store
            .get_run(run_id.as_ref())
            .await?
            .ok_or_else(|| MobError::RunNotFound {
                run_id: run_id.to_string(),
            })?;
        self.cache_run_status(run_id.clone(), run.status).await;
        if !matches!(run.status, MobRunStatus::Pending | MobRunStatus::Running) {
            return Err(MobError::RunCanceled {
                run_id: run_id.to_string(),
            });
        }
        Ok(())
    }

    pub(super) async fn force_reset_mob(&self, mob_id: &MobId) -> MobResult<()> {
        let epoch = self.bump_reset_epoch(mob_id).await;

        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Supervisor,
                MobEventKind::ForceResetTransition,
            )
            .payload(json!({"phase": "quiesce", "epoch": epoch}))
            .build(),
        )
        .await?;

        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Supervisor,
                MobEventKind::ForceResetTransition,
            )
            .payload(json!({"phase": "drain_cancel", "epoch": epoch}))
            .build(),
        )
        .await?;

        let runs = self
            .run_store
            .list_runs(MobRunFilter {
                mob_id: Some(mob_id.clone()),
                limit: Some(10_000),
                ..MobRunFilter::default()
            })
            .await?;
        for run in runs {
            if matches!(run.status, MobRunStatus::Pending | MobRunStatus::Running) {
                let _ = self.cancel_run(run.run_id.as_ref()).await;
            }
        }

        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Supervisor,
                MobEventKind::ForceResetTransition,
            )
            .payload(json!({"phase": "archive", "epoch": epoch}))
            .build(),
        )
        .await?;

        let keys: Vec<String> = {
            let managed = self.managed_meerkats.read().await;
            managed
                .get(mob_id)
                .map(|items| items.keys().cloned().collect())
                .unwrap_or_default()
        };
        for key in keys {
            let _ = self.retire_meerkat(mob_id, &key).await;
        }

        let reconcile = self
            .reconcile(MobReconcileRequest {
                mob_id: mob_id.clone(),
                mode: crate::model::ReconcileMode::Apply,
            })
            .await?;

        self.emit_event(
            self.event(
                mob_id.clone(),
                MobEventCategory::Supervisor,
                MobEventKind::ForceResetTransition,
            )
            .payload(json!({
                "phase": "restart",
                "epoch": epoch,
                "spawned": reconcile.spawned,
                "retired": reconcile.retired,
            }))
            .build(),
        )
        .await?;

        Ok(())
    }
}
