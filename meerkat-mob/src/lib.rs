#![forbid(unsafe_code)]

pub mod build;
pub mod definition;
pub mod error;
pub mod event;
pub mod ids;
pub mod prefab;
pub mod profile;
pub mod roster;
pub mod runtime;
pub mod storage;
pub mod store;
pub mod tasks;
pub mod tools;
pub mod validate;

pub use build::{build_agent_config, to_create_session_request};
pub use definition::{
    McpServerConfig, MobDefinition, OrchestratorConfig, RolePair, SkillSource, WiringRules,
};
pub use error::MobError;
pub use event::{MobEvent, MobEventKind, NewMobEvent};
pub use ids::{MeerkatId, MobId, ProfileName};
pub use prefab::{Prefab, PrefabInfo};
pub use profile::{Profile, ToolConfig};
pub use roster::{Roster, RosterEntry};
pub use runtime::{MobBuilder, MobHandle, MobState};
pub use storage::MobStorage;
pub use store::{InMemoryMobEventStore, MobEventStore};
pub use tasks::{MobTask, TaskBoard, TaskStatus};
