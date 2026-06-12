use crate::archive::MobpackArchive;
use async_trait::async_trait;
use meerkat_mob::{
    FlowId, MobBuilder, MobError, MobHandle, MobRun, MobSessionService, MobStorage, ProfileName,
    RunId, SpawnMemberSpec, mob_machine_run_status_is_terminal,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq)]
pub struct MobpackRunOutcome {
    pub run_id: String,
    pub final_result_digest: Option<String>,
    pub final_result: Option<serde_json::Value>,
}

pub fn has_mobpack_callable(archive: &MobpackArchive) -> bool {
    archive.manifest.adaptive.is_some()
}

pub async fn run_mobpack_callable(
    archive: &MobpackArchive,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    if archive.manifest.adaptive.is_none() {
        return Err(MobError::Internal(
            "mobpack has no callable flow".to_string(),
        ));
    }
    run_adaptive_callable(archive, control_mob, session_service, objective).await
}

struct PackAdaptiveRuntime {
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
}

#[async_trait]
impl meerkat_mob_adaptive::AdaptiveDriverRuntime for PackAdaptiveRuntime {
    type Layer = MobHandle;

    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    async fn run_planning_turn(
        &mut self,
        request: meerkat_mob_adaptive::PlanningTurnRequest,
    ) -> Result<meerkat_mob_adaptive::LayerDecision, meerkat_mob_adaptive::AdaptiveError> {
        let run_id = self
            .control_mob
            .run_flow(
                FlowId::from("plan"),
                serde_json::json!({
                    "adaptive_run_id": request.adaptive_run_id.as_str(),
                    "objective": request.objective,
                    "previous_layer_result": request.previous_layer_result,
                }),
            )
            .await?;
        let run = await_flow_terminal(&self.control_mob, run_id.clone()).await?;
        let decision = run
            .root_step_outputs
            .get(&meerkat_mob::StepId::from("plan"))
            .or_else(|| {
                if run.root_step_outputs.len() == 1 {
                    run.root_step_outputs.values().next()
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                meerkat_mob_adaptive::AdaptiveError::DriverRuntime(format!(
                    "mobpack planning run '{run_id}' produced no LayerDecision output; status={:?}; failures={:?}; steps={:?}",
                    run.status, run.failure_ledger, run.step_ledger
                ))
            })?;
        serde_json::from_value(decision.clone()).map_err(Into::into)
    }

    async fn provision_layer(
        &mut self,
        compiled: &meerkat_mob_adaptive::CompiledLayer,
    ) -> Result<Self::Layer, meerkat_mob_adaptive::AdaptiveError> {
        let mut builder = MobBuilder::from_mobpack(
            compiled.definition.clone(),
            BTreeMap::new(),
            MobStorage::in_memory(),
        )?
        .with_session_service(Arc::clone(&self.session_service));
        if let Some(adapter) = self.session_service.runtime_adapter() {
            builder = builder.with_runtime_adapter(adapter);
        }
        let handle = builder.create().await?;
        let spawn_results = handle.spawn_many(compiled.spawn_specs.clone()).await?;
        if let Some(failure) = spawn_results
            .iter()
            .find_map(|result| result.as_ref().err())
        {
            return Err(meerkat_mob_adaptive::AdaptiveError::DriverRuntime(format!(
                "mobpack layer spawn failed: {failure}"
            )));
        }
        Ok(handle)
    }

    async fn start_layer_flow(
        &mut self,
        layer: &Self::Layer,
        activation_params: BTreeMap<String, serde_json::Value>,
    ) -> Result<RunId, meerkat_mob_adaptive::AdaptiveError> {
        Ok(layer
            .run_flow(
                FlowId::from("layer-flow"),
                serde_json::to_value(activation_params)?,
            )
            .await?)
    }

    async fn await_layer_terminal(
        &mut self,
        layer: &Self::Layer,
        run_id: RunId,
    ) -> Result<MobRun, meerkat_mob_adaptive::AdaptiveError> {
        await_flow_terminal(layer, run_id).await
    }

    async fn cleanup_layer(
        &mut self,
        _layer: Self::Layer,
        _layer_id: &meerkat_mob_adaptive::LayerId,
        _attempt: u64,
    ) -> Result<meerkat_mob_adaptive::AdaptiveLayerCleanup, meerkat_mob_adaptive::AdaptiveError>
    {
        Ok(meerkat_mob_adaptive::AdaptiveLayerCleanup::Destroyed)
    }
}

async fn run_adaptive_callable(
    archive: &MobpackArchive,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    let policy = load_policy(archive)?;
    let schema_registry = load_schema_registry(archive)?;
    let profile_templates = load_profile_templates(archive)?;
    if let Some(adaptive) = &archive.manifest.adaptive {
        let flowmaster_profile = ProfileName::from(adaptive.flowmaster_profile.as_str());
        let roster = control_mob.roster().await;
        if roster.by_profile(&flowmaster_profile).next().is_none() {
            control_mob
                .spawn_spec(SpawnMemberSpec::new(
                    flowmaster_profile,
                    "adaptive-flowmaster",
                ))
                .await
                .map_err(|err| {
                    MobError::Internal(format!("mobpack FlowMaster spawn failed: {err}"))
                })?;
        }
    }
    let adaptive_run_id = fresh_run_id()?;
    let compile_context = meerkat_mob_adaptive::CompileContext {
        adaptive_run_id: adaptive_run_id.clone(),
        attempt: 1,
        schema_registry,
        profile_templates,
        previous_layer_result: None,
    };
    let driver = meerkat_mob_adaptive::AdaptiveDriver::new(control_mob.clone());
    let mut runtime = PackAdaptiveRuntime {
        control_mob,
        session_service,
    };
    let outcome = meerkat_mob_adaptive::run_adaptive_loop(
        &driver,
        &mut runtime,
        meerkat_mob_adaptive::AdaptiveRunRequest {
            adaptive_run_id: adaptive_run_id.clone(),
            policy,
            compile_context,
            objective: objective.to_string(),
            started_at_ms: now_ms(),
        },
    )
    .await
    .map_err(|err| MobError::Internal(err.to_string()))?;
    Ok(MobpackRunOutcome {
        run_id: adaptive_run_id.as_str().to_string(),
        final_result_digest: outcome
            .final_result_digest
            .as_ref()
            .map(|digest| digest.as_str().to_string()),
        final_result: outcome.final_result,
    })
}

fn load_policy(archive: &MobpackArchive) -> Result<meerkat_mob_adaptive::AdaptivePolicy, MobError> {
    let bytes = archive
        .adaptive
        .get("adaptive/policies.toml")
        .ok_or_else(|| MobError::Internal("mobpack missing adaptive/policies.toml".to_string()))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|err| MobError::Internal(format!("mobpack policy is not valid UTF-8: {err}")))?;
    toml::from_str(text).map_err(|err| MobError::Internal(format!("invalid mobpack policy: {err}")))
}

fn load_schema_registry(
    archive: &MobpackArchive,
) -> Result<meerkat_mob_adaptive::SchemaRegistry, MobError> {
    let registry_bytes = archive
        .schemas
        .get("schemas/registry.json")
        .ok_or_else(|| MobError::Internal("mobpack missing schemas/registry.json".to_string()))?;
    let declared: BTreeMap<String, String> = serde_json::from_slice(registry_bytes)
        .map_err(|err| MobError::Internal(format!("invalid schemas/registry.json: {err}")))?;
    let mut registry = meerkat_mob_adaptive::SchemaRegistry::default();
    for (name, path) in declared {
        let schema_bytes = archive
            .schemas
            .get(&path)
            .or_else(|| archive.schemas.get(&format!("schemas/{path}")))
            .ok_or_else(|| {
                MobError::Internal(format!("schema registry entry '{name}' missing '{path}'"))
            })?;
        let schema: serde_json::Value = serde_json::from_slice(schema_bytes)
            .map_err(|err| MobError::Internal(format!("invalid schema '{path}': {err}")))?;
        registry
            .insert(
                meerkat_mob_adaptive::SchemaName::new(name)
                    .map_err(|err| MobError::Internal(err.to_string()))?,
                schema,
            )
            .map_err(|err| MobError::Internal(format!("invalid mobpack schema: {err}")))?;
    }
    Ok(registry)
}

fn load_profile_templates(
    archive: &MobpackArchive,
) -> Result<BTreeMap<ProfileName, meerkat_mob::Profile>, MobError> {
    let mut profiles = BTreeMap::new();
    for (name, binding) in &archive.definition.profiles {
        let Some(profile) = binding.as_inline() else {
            return Err(MobError::Internal(format!(
                "mobpack profile '{name}' uses a realm profile reference; inline profile templates are required"
            )));
        };
        profiles.insert(name.clone(), profile.clone());
    }
    Ok(profiles)
}

async fn await_flow_terminal(
    mob: &MobHandle,
    run_id: RunId,
) -> Result<MobRun, meerkat_mob_adaptive::AdaptiveError> {
    loop {
        if let Some(run) = mob.flow_status(run_id.clone()).await?
            && mob_machine_run_status_is_terminal(&run_id, &run.status)?
        {
            return Ok(run);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn fresh_run_id() -> Result<meerkat_mob_adaptive::AdaptiveRunId, MobError> {
    meerkat_mob_adaptive::AdaptiveRunId::new(RunId::new().to_string())
        .map_err(|err| MobError::Internal(err.to_string()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_run_id_is_unique_per_invocation() -> Result<(), Box<dyn std::error::Error>> {
        let first = fresh_run_id()?;
        let second = fresh_run_id()?;

        assert_ne!(first, second);
        let _first: RunId = first.as_str().parse()?;
        let _second: RunId = second.as_str().parse()?;
        Ok(())
    }
}
