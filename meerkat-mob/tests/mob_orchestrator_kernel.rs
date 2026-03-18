use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_mob::MobRuntimeMode;
use meerkat_mob::definition::{
    MobDefinition, OrchestratorConfig, PolicyMode, SupervisorSpec, TopologySpec,
};
use meerkat_mob::ids::{MobId, ProfileName};
use meerkat_mob::profile::{Profile, ToolConfig};
use meerkat_mob::runtime::topology::MobTopologyService;
use meerkat_mob::runtime::{MobOrchestratorKernel, MobOrchestratorSnapshot};

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
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        },
    );

    MobDefinition {
        id: MobId::from("mob-orchestrator-owner-test"),
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
fn mob_orchestrator_kernel_tracks_binding_pending_spawn_and_resume_semantics_for_machine_verify() {
    let definition = definition();
    let topology = Arc::new(MobTopologyService::new(definition.topology.clone()));
    let kernel = MobOrchestratorKernel::new(&definition, topology);

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
    let staged = kernel.snapshot();
    assert_eq!(staged.pending_spawn_count, 1);
    assert_eq!(staged.active_flow_count, 1);
    assert_eq!(staged.topology_revision, 2);

    kernel.record_spawn_completed().expect("complete spawn");
    kernel.record_flow_finished();
    kernel.record_stop();
    let stopped = kernel.snapshot();
    assert!(!stopped.coordinator_bound);
    assert_eq!(stopped.pending_spawn_count, 0);
    assert_eq!(stopped.active_flow_count, 0);
    assert!(!stopped.supervisor_active);

    kernel.record_resume();
    let resumed = kernel.snapshot();
    assert!(resumed.coordinator_bound);
    assert!(resumed.supervisor_active);
    assert!(
        resumed.topology_revision >= stopped.topology_revision,
        "resume must preserve monotonic orchestrator-owned topology revisions"
    );
}
