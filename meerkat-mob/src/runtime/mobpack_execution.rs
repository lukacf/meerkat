#[cfg(target_arch = "wasm32")]
use crate::tokio::time as tokio_time;
use crate::{
    FlowId, MobBuilder, MobDefinition, MobError, MobHandle, MobRun, MobSessionService, MobStorage,
    Profile, ProfileName, RunId, SpawnMemberSpec, mob_machine_run_status_is_terminal,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
#[cfg(not(target_arch = "wasm32"))]
use tokio::time as tokio_time;

const CALLABLE_POLICY_PATH: &str = "adaptive/policies.toml";

#[derive(Clone, Debug, PartialEq)]
pub struct MobpackRunOutcome {
    pub run_id: String,
    pub final_result_digest: Option<String>,
    pub final_result: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobpackCallableConfig {
    coordinator_profile: ProfileName,
}

impl MobpackCallableConfig {
    pub fn new(coordinator_profile: ProfileName) -> Self {
        Self {
            coordinator_profile,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobpackRunSpec {
    definition: MobDefinition,
    packed_skills: BTreeMap<String, Vec<u8>>,
    callable: Option<MobpackCallableConfig>,
    policy_files: BTreeMap<String, Vec<u8>>,
    schemas: BTreeMap<String, Vec<u8>>,
}

impl MobpackRunSpec {
    pub fn new(
        definition: MobDefinition,
        packed_skills: BTreeMap<String, Vec<u8>>,
        callable: Option<MobpackCallableConfig>,
        policy_files: BTreeMap<String, Vec<u8>>,
        schemas: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            definition,
            packed_skills,
            callable,
            policy_files,
            schemas,
        }
    }

    pub fn is_callable(&self) -> bool {
        self.callable.is_some()
    }

    pub fn definition(&self) -> &MobDefinition {
        &self.definition
    }

    pub fn packed_skills(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.packed_skills
    }
}

#[cfg(feature = "runtime-adapter")]
pub async fn run_mobpack_callable(
    spec: &MobpackRunSpec,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    if !spec.is_callable() {
        return Err(MobError::Internal(
            "mobpack has no callable flow".to_string(),
        ));
    }
    run_adaptive_callable(spec, control_mob, session_service, objective).await
}

struct PackAdaptiveRuntime {
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
}

#[cfg(feature = "runtime-adapter")]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl crate::adaptive::AdaptiveDriverRuntime for PackAdaptiveRuntime {
    type Layer = MobHandle;

    fn now_ms(&mut self) -> u64 {
        now_ms()
    }

    async fn run_planning_turn(
        &mut self,
        request: crate::adaptive::PlanningTurnRequest,
    ) -> Result<crate::adaptive::LayerDecision, crate::adaptive::AdaptiveError> {
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
            .get(&crate::StepId::from("plan"))
            .or_else(|| {
                if run.root_step_outputs.len() == 1 {
                    run.root_step_outputs.values().next()
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                crate::adaptive::AdaptiveError::DriverRuntime(format!(
                    "adaptive planning run '{run_id}' produced no LayerDecision output; status={:?}; failures={:?}; steps={:?}",
                    run.status, run.failure_ledger, run.step_ledger
                ))
            })?;
        serde_json::from_value(decision.clone()).map_err(Into::into)
    }

    async fn provision_layer(
        &mut self,
        compiled: &crate::adaptive::CompiledLayer,
    ) -> Result<Self::Layer, crate::adaptive::AdaptiveError> {
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
            return Err(crate::adaptive::AdaptiveError::DriverRuntime(format!(
                "mobpack layer spawn failed: {failure}"
            )));
        }
        Ok(handle)
    }

    async fn start_layer_flow(
        &mut self,
        layer: &Self::Layer,
        activation_params: BTreeMap<String, serde_json::Value>,
    ) -> Result<RunId, crate::adaptive::AdaptiveError> {
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
    ) -> Result<MobRun, crate::adaptive::AdaptiveError> {
        await_flow_terminal(layer, run_id).await
    }

    async fn cleanup_layer(
        &mut self,
        _layer: Self::Layer,
        _layer_id: &crate::adaptive::LayerId,
        _attempt: u64,
    ) -> Result<crate::adaptive::AdaptiveLayerCleanup, crate::adaptive::AdaptiveError> {
        Ok(crate::adaptive::AdaptiveLayerCleanup::Destroyed)
    }
}

#[cfg(feature = "runtime-adapter")]
async fn run_adaptive_callable(
    spec: &MobpackRunSpec,
    control_mob: MobHandle,
    session_service: Arc<dyn MobSessionService>,
    objective: &str,
) -> Result<MobpackRunOutcome, MobError> {
    let policy = load_policy(spec)?;
    let schema_registry = load_schema_registry(spec)?;
    let profile_templates = load_profile_templates(spec)?;
    if let Some(callable) = &spec.callable {
        let coordinator_profile = callable.coordinator_profile.clone();
        let roster = control_mob.roster().await;
        if roster.by_profile(&coordinator_profile).next().is_none() {
            control_mob
                .spawn_spec(SpawnMemberSpec::new(
                    coordinator_profile,
                    "adaptive-flowmaster",
                ))
                .await
                .map_err(|err| {
                    MobError::Internal(format!("mobpack FlowMaster spawn failed: {err}"))
                })?;
        }
    }
    let adaptive_run_id = fresh_run_id()?;
    let compile_context = crate::adaptive::CompileContext {
        adaptive_run_id: adaptive_run_id.clone(),
        attempt: 1,
        schema_registry,
        profile_templates,
        previous_layer_result: None,
    };
    let driver = crate::adaptive::AdaptiveDriver::new(control_mob.clone());
    let mut runtime = PackAdaptiveRuntime {
        control_mob,
        session_service,
    };
    let outcome = crate::adaptive::run_adaptive_loop(
        &driver,
        &mut runtime,
        crate::adaptive::AdaptiveRunRequest {
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

fn load_policy(spec: &MobpackRunSpec) -> Result<crate::adaptive::AdaptivePolicy, MobError> {
    let bytes = spec
        .policy_files
        .get(CALLABLE_POLICY_PATH)
        .ok_or_else(|| MobError::Internal("mobpack missing adaptive/policies.toml".to_string()))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|err| MobError::Internal(format!("mobpack policy is not valid UTF-8: {err}")))?;
    toml::from_str(text).map_err(|err| MobError::Internal(format!("invalid mobpack policy: {err}")))
}

fn load_schema_registry(
    spec: &MobpackRunSpec,
) -> Result<crate::adaptive::SchemaRegistry, MobError> {
    let registry_bytes = spec
        .schemas
        .get("schemas/registry.json")
        .ok_or_else(|| MobError::Internal("mobpack missing schemas/registry.json".to_string()))?;
    let declared: BTreeMap<String, String> = serde_json::from_slice(registry_bytes)
        .map_err(|err| MobError::Internal(format!("invalid schemas/registry.json: {err}")))?;
    let mut registry = crate::adaptive::SchemaRegistry::default();
    for (name, path) in declared {
        let schema_bytes = spec
            .schemas
            .get(&path)
            .or_else(|| spec.schemas.get(&format!("schemas/{path}")))
            .ok_or_else(|| {
                MobError::Internal(format!("schema registry entry '{name}' missing '{path}'"))
            })?;
        let schema: serde_json::Value = serde_json::from_slice(schema_bytes)
            .map_err(|err| MobError::Internal(format!("invalid schema '{path}': {err}")))?;
        registry
            .insert(
                crate::adaptive::SchemaName::new(name)
                    .map_err(|err| MobError::Internal(err.to_string()))?,
                schema,
            )
            .map_err(|err| MobError::Internal(format!("invalid mobpack schema: {err}")))?;
    }
    Ok(registry)
}

fn load_profile_templates(
    spec: &MobpackRunSpec,
) -> Result<BTreeMap<ProfileName, Profile>, MobError> {
    let mut profiles = BTreeMap::new();
    for (name, binding) in &spec.definition.profiles {
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
) -> Result<MobRun, crate::adaptive::AdaptiveError> {
    loop {
        if let Some(run) = mob.flow_status(run_id.clone()).await?
            && mob_machine_run_status_is_terminal(&run_id, &run.status)?
        {
            return Ok(run);
        }
        tokio_time::sleep(Duration::from_millis(250)).await;
    }
}

fn fresh_run_id() -> Result<crate::adaptive::AdaptiveRunId, MobError> {
    crate::adaptive::AdaptiveRunId::new(RunId::new().to_string())
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
