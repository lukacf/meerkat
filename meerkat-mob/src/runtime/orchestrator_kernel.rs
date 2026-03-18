use super::topology::MobTopologyService;
use crate::definition::MobDefinition;
use crate::error::MobError;
use crate::ids::ProfileName;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MobOrchestratorSnapshot {
    pub coordinator_bound: bool,
    pub pending_spawn_count: u32,
    pub active_flow_count: u32,
    pub topology_revision: u32,
    pub supervisor_active: bool,
}

#[derive(Clone)]
pub struct MobOrchestratorKernel {
    orchestrator_role: Option<ProfileName>,
    supervisor_enabled: bool,
    topology: Arc<MobTopologyService>,
    snapshot: Arc<Mutex<MobOrchestratorSnapshot>>,
}

impl MobOrchestratorKernel {
    fn snapshot_guard(&self) -> std::sync::MutexGuard<'_, MobOrchestratorSnapshot> {
        self.snapshot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn new(definition: &MobDefinition, topology: Arc<MobTopologyService>) -> Self {
        Self {
            orchestrator_role: definition
                .orchestrator
                .as_ref()
                .map(|orchestrator| orchestrator.profile.clone()),
            supervisor_enabled: definition.supervisor.is_some(),
            topology,
            snapshot: Arc::new(Mutex::new(MobOrchestratorSnapshot::default())),
        }
    }

    pub fn initialize(&self) {
        if self.orchestrator_role.is_some() {
            let revision = self.topology.bind_coordinator();
            let mut snapshot = self.snapshot_guard();
            snapshot.coordinator_bound = true;
            snapshot.topology_revision = revision;
            snapshot.supervisor_active = self.supervisor_enabled;
        }
    }

    pub fn snapshot(&self) -> MobOrchestratorSnapshot {
        self.snapshot_guard().clone()
    }

    pub fn record_spawn_staged(&self) -> Result<(), MobError> {
        let mut snapshot = self.snapshot_guard();
        if self.orchestrator_role.is_none() || !snapshot.coordinator_bound {
            return Err(MobError::Internal(
                "pending spawn ownership requires a bound orchestrator coordinator".into(),
            ));
        }
        snapshot.pending_spawn_count = snapshot.pending_spawn_count.saturating_add(1);
        snapshot.topology_revision = self.topology.note_spawn_boundary();
        Ok(())
    }

    pub fn record_spawn_completed(&self) -> Result<(), MobError> {
        let mut snapshot = self.snapshot_guard();
        if snapshot.pending_spawn_count == 0 {
            return Err(MobError::Internal(
                "spawn completion arrived with no pending orchestrator-owned spawn".into(),
            ));
        }
        snapshot.pending_spawn_count -= 1;
        snapshot.topology_revision = self.topology.note_spawn_boundary();
        Ok(())
    }

    pub fn record_flow_started(&self) {
        let mut snapshot = self.snapshot_guard();
        snapshot.active_flow_count = snapshot.active_flow_count.saturating_add(1);
    }

    pub fn record_flow_finished(&self) {
        let mut snapshot = self.snapshot_guard();
        snapshot.active_flow_count = snapshot.active_flow_count.saturating_sub(1);
    }

    pub fn record_stop(&self) {
        let revision = self.topology.unbind_coordinator();
        let mut snapshot = self.snapshot_guard();
        snapshot.coordinator_bound = false;
        snapshot.topology_revision = revision;
        snapshot.supervisor_active = false;
    }

    pub fn record_resume(&self) {
        let revision = if self.orchestrator_role.is_some() {
            self.topology.bind_coordinator()
        } else {
            self.topology.revision()
        };
        let mut snapshot = self.snapshot_guard();
        snapshot.coordinator_bound = self.orchestrator_role.is_some();
        snapshot.topology_revision = revision;
        snapshot.supervisor_active = self.supervisor_enabled;
    }

    pub fn record_completed(&self) {
        self.record_stop();
        let mut snapshot = self.snapshot_guard();
        snapshot.active_flow_count = 0;
    }

    pub fn record_destroyed(&self) {
        self.record_stop();
        let mut snapshot = self.snapshot_guard();
        snapshot.pending_spawn_count = 0;
        snapshot.active_flow_count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::definition::{
        MobDefinition, OrchestratorConfig, PolicyMode, SupervisorSpec, TopologySpec,
    };
    use crate::ids::{MobId, ProfileName};
    use crate::profile::{Profile, ToolConfig};
    use std::collections::BTreeMap;

    fn definition() -> MobDefinition {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            ProfileName::from("lead"),
            Profile {
                model: "model".into(),
                skills: Vec::new(),
                tools: ToolConfig::default(),
                peer_description: "lead".into(),
                external_addressable: true,
                backend: None,
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                max_inline_peer_notifications: None,
                output_schema: None,
                provider_params: None,
            },
        );
        MobDefinition {
            id: MobId::from("mob"),
            orchestrator: Some(OrchestratorConfig {
                profile: ProfileName::from("lead"),
            }),
            profiles,
            mcp_servers: BTreeMap::new(),
            wiring: Default::default(),
            skills: BTreeMap::new(),
            backend: Default::default(),
            flows: BTreeMap::new(),
            topology: Some(TopologySpec {
                mode: PolicyMode::Advisory,
                rules: vec![],
            }),
            supervisor: Some(SupervisorSpec {
                role: ProfileName::from("lead"),
                escalation_threshold: 1,
            }),
            limits: None,
            spawn_policy: None,
            event_router: None,
        }
    }

    #[test]
    fn orchestrator_kernel_tracks_binding_pending_spawns_and_supervisor_state() {
        let topology = Arc::new(MobTopologyService::new(definition().topology.clone()));
        let kernel = MobOrchestratorKernel::new(&definition(), topology);
        kernel.initialize();
        assert_eq!(
            kernel.snapshot(),
            MobOrchestratorSnapshot {
                coordinator_bound: true,
                pending_spawn_count: 0,
                active_flow_count: 0,
                topology_revision: 1,
                supervisor_active: true,
            }
        );

        kernel.record_spawn_staged().expect("stage spawn");
        kernel.record_flow_started();
        let snapshot = kernel.snapshot();
        assert_eq!(snapshot.pending_spawn_count, 1);
        assert_eq!(snapshot.active_flow_count, 1);
        assert_eq!(snapshot.topology_revision, 2);

        kernel.record_spawn_completed().expect("complete spawn");
        kernel.record_flow_finished();
        kernel.record_stop();
        let snapshot = kernel.snapshot();
        assert!(!snapshot.coordinator_bound);
        assert_eq!(snapshot.pending_spawn_count, 0);
        assert_eq!(snapshot.active_flow_count, 0);
        assert!(!snapshot.supervisor_active);
    }
}
