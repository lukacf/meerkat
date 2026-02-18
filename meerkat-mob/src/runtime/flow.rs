use super::*;

impl MobRuntime {
    pub(super) async fn execute_run(
        &self,
        run_id: RunId,
        request: MobActivationRequest,
        spec: MobSpec,
        expected_epoch: u64,
    ) -> MobResult<()> {
        let flow = spec
            .flows
            .get(request.flow_id.as_ref())
            .cloned()
            .ok_or_else(|| MobError::FlowNotFound {
                flow_id: request.flow_id.to_string(),
            })?;

        let _ = self
            .run_store
            .cas_run_status(run_id.as_ref(), MobRunStatus::Pending, MobRunStatus::Running)
            .await?;

        if request.dry_run {
            let _ = self
                .run_store
                .cas_run_status(run_id.as_ref(), MobRunStatus::Running, MobRunStatus::Completed)
                .await?;
            self.emit_event(
                self.event(spec.mob_id.clone(), MobEventCategory::Flow, MobEventKind::RunCompleted)
                    .run_id(run_id.clone())
                    .flow_id(request.flow_id.clone())
                    .payload(json!({
                        "status": MobRunStatus::Completed,
                        "dry_run": true,
                    }))
                    .build(),
            )
            .await?;
            return Ok(());
        }

        let _ = self
            .reconcile(MobReconcileRequest {
                mob_id: request.mob_id.clone(),
                mode: crate::model::ReconcileMode::Apply,
            })
            .await?;

        let flow_ctx = FlowContext {
            run_id: run_id.clone(),
            mob_id: spec.mob_id.clone(),
            flow_id: request.flow_id.clone(),
            spec_revision: spec.revision,
            activation_payload: request.payload.clone(),
        };

        let run_result = self.execute_flow(&spec, &flow_ctx, &flow, expected_epoch).await;

        match run_result {
            Ok(has_failures) => {
                if self.is_run_canceled(&run_id).await? {
                    return Ok(());
                }

                let target_status = if has_failures {
                    MobRunStatus::Failed
                } else {
                    MobRunStatus::Completed
                };

                let _ = self
                    .run_store
                    .cas_run_status(run_id.as_ref(), MobRunStatus::Running, target_status)
                    .await?;

                self.emit_event(
                    self.event(
                        spec.mob_id.clone(),
                        MobEventCategory::Flow,
                        if target_status == MobRunStatus::Completed {
                            MobEventKind::RunCompleted
                        } else {
                            MobEventKind::RunFailed
                        },
                    )
                    .run_id(run_id.clone())
                    .flow_id(request.flow_id.clone())
                    .payload(json!({"status": target_status}))
                    .build(),
                )
                .await?;

                if target_status == MobRunStatus::Failed {
                    self.supervisor_escalate(
                        &spec,
                        run_id.as_ref(),
                        request.flow_id.as_ref(),
                        "run finished with failed steps",
                        json!({"status": "failed"}),
                    )
                    .await?;
                }
            }
            Err(MobError::RunCanceled { .. } | MobError::ResetBarrier { .. }) => {
                let _ = self
                    .run_store
                    .cas_run_status(run_id.as_ref(), MobRunStatus::Running, MobRunStatus::Canceled)
                    .await?;
            }
            Err(err) => {
                if self.is_run_canceled(&run_id).await? {
                    return Ok(());
                }

                let _ = self
                    .run_store
                    .cas_run_status(run_id.as_ref(), MobRunStatus::Running, MobRunStatus::Failed)
                    .await?;
                self.run_store
                    .append_failure_entry(
                        run_id.as_ref(),
                        FailureLedgerEntry {
                            timestamp: Utc::now(),
                            step_id: None,
                            target_meerkat: None,
                            error: err.to_string(),
                        },
                    )
                    .await?;

                self.emit_event(
                    self.event(spec.mob_id.clone(), MobEventCategory::Flow, MobEventKind::RunFailed)
                        .run_id(run_id.clone())
                        .flow_id(request.flow_id.clone())
                        .payload(json!({"error": err.to_string()}))
                        .build(),
                )
                .await?;

                self.supervisor_escalate(
                    &spec,
                    run_id.as_ref(),
                    request.flow_id.as_ref(),
                    "run execution error",
                    json!({"error": err.to_string()}),
                )
                .await?;
            }
        }

        Ok(())
    }

    pub(super) async fn execute_flow(
        &self,
        spec: &MobSpec,
        flow_ctx: &FlowContext,
        flow: &FlowSpec,
        expected_epoch: u64,
    ) -> MobResult<bool> {
        let step_by_id: IndexMap<StepId, FlowStepSpec> = flow
            .steps
            .iter()
            .map(|step| (step.step_id.clone(), step.clone()))
            .collect();

        let declaration_order: Vec<StepId> =
            flow.steps.iter().map(|step| step.step_id.clone()).collect();
        let mut completed = HashSet::<StepId>::new();
        let mut queued = HashSet::<StepId>::new();
        let mut ready = VecDeque::<StepId>::new();
        let mut outputs = HashMap::<StepId, Value>::new();
        let mut has_failures = false;
        let mut running = FuturesUnordered::new();

        for step in &flow.steps {
            if step.depends_on.is_empty() {
                ready.push_back(step.step_id.clone());
                queued.insert(step.step_id.clone());
            }
        }

        while completed.len() < flow.steps.len() {
            self.ensure_run_active(&flow_ctx.run_id, &spec.mob_id, expected_epoch)
                .await?;

            while running.len() < spec.limits.max_concurrent_ready_steps && !ready.is_empty() {
                if let Some(step_id) = ready.pop_front() {
                    let step = step_by_id
                        .get(&step_id)
                        .ok_or_else(|| MobError::Internal(format!("missing step '{step_id}'")))?
                        .clone();
                    let mut dependency_outputs = BTreeMap::new();
                    for dep in &step.depends_on {
                        if let Some(value) = outputs.get(dep) {
                            dependency_outputs.insert(dep.clone(), value.clone());
                        }
                    }
                    let activation_payload = flow_ctx.activation_payload.clone();
                    let step_id_for_task = step_id.clone();
                    running.push(async move {
                        let result = self
                            .execute_step(StepExecutionInput {
                                spec,
                                flow_id: &flow_ctx.flow_id,
                                run_id: &flow_ctx.run_id,
                                step,
                                dependency_outputs,
                                activation_payload,
                                expected_epoch,
                            })
                            .await;
                        (step_id_for_task, result)
                    });
                }
            }

            let Some((step_id, step_result)) = running.next().await else {
                break;
            };
            let step_result = step_result?;

            outputs.insert(step_id.clone(), step_result.output.clone());
            self.run_store
                .put_step_output(
                    flow_ctx.run_id.as_ref(),
                    step_id.as_ref(),
                    step_result.output,
                )
                .await?;

            let status = step_result.status;
            if matches!(status, StepRunStatus::Failed) {
                has_failures = true;
            }

            let mut run = self
                .run_store
                .get_run(flow_ctx.run_id.as_ref())
                .await?
                .ok_or_else(|| MobError::RunNotFound {
                    run_id: flow_ctx.run_id.to_string(),
                })?;
            run.step_statuses.insert(step_id.clone(), status);
            self.run_store.put_run(run).await?;

            completed.insert(step_id);

            for step_id in &declaration_order {
                if completed.contains(step_id) || queued.contains(step_id) {
                    continue;
                }

                let Some(step) = step_by_id.get(step_id) else {
                    continue;
                };

                if step.depends_on.iter().all(|dep| completed.contains(dep)) {
                    ready.push_back(step_id.clone());
                    queued.insert(step_id.clone());
                }
            }
        }

        if completed.len() != flow.steps.len() {
            return Err(MobError::Internal(
                "flow execution ended with incomplete steps".to_string(),
            ));
        }

        Ok(has_failures)
    }

    pub(super) async fn execute_step(
        &self,
        input: StepExecutionInput<'_>,
    ) -> MobResult<StepExecutionResult> {
        let StepExecutionInput {
            spec,
            flow_id,
            run_id,
            step,
            dependency_outputs,
            activation_payload,
            expected_epoch,
        } = input;

        if let Some(condition) = &step.condition {
            let condition_ctx = ConditionContext {
                activation: activation_payload.clone(),
                steps: dependency_outputs
                    .iter()
                    .map(|(step_id, output)| {
                        (
                            step_id.clone(),
                            StepOutput {
                                status: StepRunStatus::Completed,
                                count: output
                                    .get("success_count")
                                    .and_then(Value::as_u64)
                                    .unwrap_or(0) as usize,
                                output: output.clone(),
                            },
                        )
                    })
                    .collect(),
            };
            if !evaluate_condition(condition, &condition_ctx) {
                self.emit_event(
                    self.event(spec.mob_id.clone(), MobEventCategory::Flow, MobEventKind::StepPartial)
                        .run_id(run_id.clone())
                        .flow_id(flow_id.clone())
                        .step_id(step.step_id.clone())
                        .payload(json!({"skipped": true}))
                        .build(),
                )
                .await?;

                return Ok(StepExecutionResult {
                    status: StepRunStatus::Skipped,
                    output: json!({"skipped": true}),
                });
            }
        }

        self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
            .await?;

        let mut targets = self
            .resolve_step_targets(
                spec,
                &step.targets.role,
                &step.targets.meerkat_id,
                activation_payload.clone(),
            )
            .await?;
        targets.sort_by(|a, b| a.comms_name.cmp(&b.comms_name));

        if matches!(step.dispatch_mode, crate::model::DispatchMode::OneToOne) {
            targets = targets.into_iter().take(1).collect();
        }

        if targets.is_empty() {
            return Ok(StepExecutionResult {
                status: StepRunStatus::Failed,
                output: json!({"error": "no targets resolved"}),
            });
        }

        let supervisor = self.supervisor_runtime(&spec.mob_id).await?;
        let policy = &spec.topology.flow_dispatched;
        let mut dispatch_futures = FuturesUnordered::new();
        let mut dispatched_targets = 0usize;

        for target in targets {
            let intent = step.intent.as_deref().unwrap_or("delegate").to_string();
            let allowed = evaluate_topology(policy, "supervisor", &target.role, "peer_request", &intent);

            if !allowed {
                self.emit_event(
                    self.event(
                        spec.mob_id.clone(),
                        MobEventCategory::Topology,
                        MobEventKind::TopologyViolation,
                    )
                    .run_id(run_id.clone())
                    .flow_id(flow_id.clone())
                    .step_id(step.step_id.clone())
                    .meerkat_id(target.meerkat_id.clone())
                    .payload(json!({
                        "from_role": "supervisor",
                        "to_role": target.role,
                        "intent": intent,
                        "target": target.comms_name,
                    }))
                    .build(),
                )
                .await?;

                if matches!(policy.mode, PolicyMode::Strict) {
                    self.run_store
                        .append_failure_entry(
                            run_id.as_ref(),
                            FailureLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: Some(step.step_id.clone()),
                                target_meerkat: Some(target.meerkat_id.clone()),
                                error: "blocked by strict flow_dispatched topology policy"
                                    .to_string(),
                            },
                        )
                        .await?;
                    continue;
                }
            }

            dispatched_targets += 1;
            let target_meerkat_id = target.meerkat_id.clone();
            let target_comms_name = target.comms_name.clone();
            let step_for_dispatch = step.clone();
            let supervisor_for_dispatch = supervisor.clone();
            let payload = json!({
                "mob_id": spec.mob_id,
                "run_id": run_id,
                "flow_id": flow_id,
                "step_id": step.step_id.clone(),
                "attempt": 1,
                "hop": 1,
                "logical_op_key": format!("{}:{}:{}", run_id, step.step_id, target.meerkat_id),
                "payload": activation_payload.clone(),
                "depends_on_outputs": dependency_outputs.clone(),
            });

            dispatch_futures.push(async move {
                let result = self
                    .dispatch_to_target(DispatchTargetInput {
                        run_id,
                        flow_id,
                        step: &step_for_dispatch,
                        supervisor: supervisor_for_dispatch,
                        target,
                        intent,
                        payload,
                        spec,
                        expected_epoch,
                    })
                    .await;
                (target_meerkat_id, target_comms_name, result)
            });
        }

        if dispatched_targets == 0 {
            return Ok(StepExecutionResult {
                status: StepRunStatus::Failed,
                output: json!({"error": "all targets blocked by topology policy"}),
            });
        }

        let mut success_values = Vec::new();
        let mut failure_values = Vec::new();
        let mut timeout_failures = 0usize;
        let mut non_timeout_failures = 0usize;

        while let Some((meerkat_id, comms_name, item)) = dispatch_futures.next().await {
            match item {
                Ok(value) => success_values.push(value),
                Err(err) => {
                    let timeout = matches!(err, MobError::DispatchTimeout { .. });
                    if timeout {
                        timeout_failures += 1;
                    } else {
                        non_timeout_failures += 1;
                    }
                    failure_values.push(json!({
                        "meerkat_id": meerkat_id,
                        "target": comms_name,
                        "timeout": timeout,
                        "error": err.to_string(),
                    }));
                }
            }

            self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                .await?;

            if should_stop_collection(
                &step.collection_policy,
                dispatched_targets,
                success_values.len(),
                timeout_failures,
                non_timeout_failures,
                dispatch_futures.len(),
            ) {
                break;
            }
        }
        drop(dispatch_futures);

        let success_count = success_values.len();
        let failure_count = timeout_failures + non_timeout_failures;
        let status = classify_step_status(
            &step.collection_policy,
            step.on_timeout,
            dispatched_targets,
            success_count,
            timeout_failures,
            non_timeout_failures,
        );

        let output = json!({
            "step_id": step.step_id,
            "status": status,
            "success_count": success_count,
            "failure_count": failure_count,
            "timeout_failure_count": timeout_failures,
            "non_timeout_failure_count": non_timeout_failures,
            "successes": success_values,
            "failures": failure_values,
        });

        match step.schema_policy {
            SchemaPolicy::WarnOnly | SchemaPolicy::RetryThenWarn => {
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Some(schema) = spec.schemas.get(schema_ref)
                {
                    let mut mismatches = Vec::new();
                    for success in output
                        .get("successes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        let candidate = success
                            .get("result")
                            .cloned()
                            .unwrap_or_else(|| success.clone());
                        if !jsonschema::is_valid(schema, &candidate) {
                            mismatches.push(json!({
                                "target": success.get("target"),
                                "interaction_id": success.get("interaction_id"),
                            }));
                        }
                    }

                    if !mismatches.is_empty() {
                        self.emit_event(
                            self.event(
                                spec.mob_id.clone(),
                                MobEventCategory::Supervisor,
                                MobEventKind::Warning,
                            )
                            .run_id(run_id.clone())
                            .flow_id(flow_id.clone())
                            .step_id(step.step_id.clone())
                            .payload(json!({
                                "warning": "mob peer response schema mismatch",
                                "mismatches": mismatches,
                            }))
                            .build(),
                        )
                        .await?;
                    }
                }
            }
            SchemaPolicy::RetryThenFail => {
                if let Some(schema_ref) = &step.expected_schema_ref
                    && let Some(schema) = spec.schemas.get(schema_ref)
                {
                    let mut mismatches = Vec::new();
                    for success in output
                        .get("successes")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                    {
                        let candidate = success
                            .get("result")
                            .cloned()
                            .unwrap_or_else(|| success.clone());
                        if !jsonschema::is_valid(schema, &candidate) {
                            mismatches.push(json!({
                                "target": success.get("target"),
                                "interaction_id": success.get("interaction_id"),
                            }));
                        }
                    }

                    if !mismatches.is_empty() {
                        return Err(MobError::SchemaValidation(format!(
                            "step '{}' peer responses failed schema '{}': {}",
                            step.step_id,
                            schema_ref,
                            json!(mismatches)
                        )));
                    }
                }
            }
        }

        Ok(StepExecutionResult { status, output })
    }

    pub(super) async fn dispatch_to_target(&self, input: DispatchTargetInput<'_>) -> MobResult<Value> {
        let DispatchTargetInput {
            run_id,
            flow_id,
            step,
            supervisor,
            target,
            intent,
            payload,
            spec,
            expected_epoch,
        } = input;

        let timeout_ms = step.timeout_ms.unwrap_or(spec.limits.default_step_timeout_ms);
        let max_attempts = 2u32;
        let logical_key = format!("{}:{}:{}", run_id, step.step_id, target.meerkat_id);
        for attempt in 1..=max_attempts {
            self.record_dispatch_start(&logical_key).await;
            self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                .await?;
            self.emit_event(
                self.event(spec.mob_id.clone(), MobEventCategory::Dispatch, MobEventKind::StepStarted)
                    .run_id(run_id.clone())
                    .flow_id(flow_id.clone())
                    .step_id(step.step_id.clone())
                    .meerkat_id(target.meerkat_id.clone())
                    .payload(json!({
                        "target": target.comms_name,
                        "intent": intent,
                        "attempt": attempt,
                    }))
                    .build(),
            )
            .await?;

            let mut attempt_payload = payload.clone();
            if let Some(map) = attempt_payload.as_object_mut() {
                map.insert("attempt".to_string(), json!(attempt));
            }

            let command = CommsCommand::PeerRequest {
                to: meerkat::PeerName::new(target.comms_name.clone()).map_err(MobError::Comms)?,
                intent: intent.clone(),
                params: attempt_payload,
                stream: InputStreamMode::ReserveInteraction,
            };

            let (receipt, mut stream) = match supervisor.send_and_stream(command.clone()).await {
                Ok(result) => result,
                Err(_) => {
                    let receipt = supervisor
                        .send(command)
                        .await
                        .map_err(|err| MobError::Comms(err.to_string()))?;
                    let stream = match &receipt {
                        SendReceipt::PeerRequestSent { interaction_id, .. } => supervisor
                            .stream(meerkat::StreamScope::Interaction(interaction_id.to_owned()))
                            .map_err(|err| MobError::Comms(err.to_string()))?,
                        _ => {
                            return Err(MobError::Comms(
                                "unexpected receipt for peer request".to_string(),
                            ));
                        }
                    };
                    (receipt, stream)
                }
            };

            let interaction_id = match receipt {
                SendReceipt::PeerRequestSent { interaction_id, .. } => interaction_id,
                _ => {
                    return Err(MobError::Comms(
                        "unexpected send receipt for peer request".to_string(),
                    ));
                }
            };

            let target_for_response = target.comms_name.clone();
            let meerkat_id_for_response = target.meerkat_id.clone();
            let wait = async move {
                while let Some(event) = stream.next().await {
                    match event {
                        meerkat::AgentEvent::InteractionComplete {
                            interaction_id: completed_id,
                            result,
                        } if completed_id == interaction_id => {
                            return Ok(json!({
                                "result": result,
                                "interaction_id": interaction_id,
                                "target": target_for_response,
                                "meerkat_id": meerkat_id_for_response,
                            }));
                        }
                        meerkat::AgentEvent::InteractionFailed {
                            interaction_id: failed_id,
                            error,
                        } if failed_id == interaction_id => {
                            return Err(MobError::Comms(error));
                        }
                        _ => {}
                    }
                }
                Err(MobError::Comms(
                    "interaction stream closed before terminal event".to_string(),
                ))
            };

            let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), wait)
                .await
                .map_err(|_| MobError::DispatchTimeout { timeout_ms })?;

            match result {
                Ok(value) => {
                    self.ensure_run_active(run_id, &spec.mob_id, expected_epoch)
                        .await?;
                    let inserted = self
                        .run_store
                        .append_step_entry_if_absent(
                            run_id.as_ref(),
                            &logical_key,
                            StepLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: step.step_id.clone(),
                                target_meerkat: target.meerkat_id.clone(),
                                logical_key: logical_key.clone(),
                                attempt,
                                status: StepRunStatus::Completed,
                                detail: value.clone(),
                            },
                        )
                        .await?;
                    if !inserted {
                        self.record_dispatch_finish(&logical_key).await;
                        return Ok(json!({
                            "attempt": attempt,
                            "target": target.comms_name,
                            "meerkat_id": target.meerkat_id,
                            "result": value.get("result").cloned().unwrap_or(Value::Null),
                            "interaction_id": value.get("interaction_id").cloned(),
                            "duplicate_ignored": true,
                        }));
                    }

                    self.emit_event(
                        self.event(spec.mob_id.clone(), MobEventCategory::Dispatch, MobEventKind::StepCompleted)
                            .run_id(run_id.clone())
                            .flow_id(flow_id.clone())
                            .step_id(step.step_id.clone())
                            .meerkat_id(target.meerkat_id.clone())
                            .payload(json!({
                                "attempt": attempt,
                                "response": value.clone(),
                            }))
                            .build(),
                    )
                    .await?;

                    self.record_dispatch_finish(&logical_key).await;
                    return Ok(json!({
                        "attempt": attempt,
                        "target": target.comms_name,
                        "meerkat_id": target.meerkat_id,
                        "result": value.get("result").cloned().unwrap_or(Value::Null),
                        "interaction_id": value.get("interaction_id").cloned(),
                    }));
                }
                Err(err) => {
                    let retryable =
                        matches!(err, MobError::DispatchTimeout { .. } | MobError::Comms(_));
                    if attempt < max_attempts && retryable {
                        self.record_dispatch_finish(&logical_key).await;
                        tokio::time::sleep(std::time::Duration::from_millis(
                            spec.supervisor.retry_backoff_ms,
                        ))
                        .await;
                        continue;
                    }

                    self.run_store
                        .append_failure_entry(
                            run_id.as_ref(),
                            FailureLedgerEntry {
                                timestamp: Utc::now(),
                                step_id: Some(step.step_id.clone()),
                                target_meerkat: Some(target.meerkat_id.clone()),
                                error: format!("attempt {attempt}: {err}"),
                            },
                        )
                        .await?;

                    self.emit_event(
                        self.event(spec.mob_id.clone(), MobEventCategory::Dispatch, MobEventKind::StepFailed)
                            .run_id(run_id.clone())
                            .flow_id(flow_id.clone())
                            .step_id(step.step_id.clone())
                            .meerkat_id(target.meerkat_id.clone())
                            .payload(json!({"error": err.to_string(), "attempt": attempt}))
                            .build(),
                    )
                    .await?;

                    self.record_dispatch_finish(&logical_key).await;
                    return Err(err);
                }
            }
        }

        Err(MobError::Internal(
            "dispatch attempt loop exited unexpectedly".to_string(),
        ))
    }
}
