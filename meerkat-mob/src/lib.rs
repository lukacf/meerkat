#![forbid(unsafe_code)]

pub mod build;
pub mod definition;
pub mod error;
pub mod event;
pub mod ids;
pub mod profile;
pub mod roster;
pub mod runtime;
pub mod store;
pub mod storage;
pub mod tasks;
pub mod validate;
pub mod tools;
pub mod prefab;

pub use build::{build_agent_config, to_create_session_request};
pub use definition::{McpServerConfig, MobDefinition, OrchestratorConfig, RolePair, WiringRules, SkillSource};
pub use error::MobError;
pub use event::{MobEvent, MobEventKind, NewMobEvent};
pub use ids::{MeerkatId, MobId, ProfileName};
pub use profile::{Profile, ToolConfig};
pub use runtime::{MobBuilder, MobHandle, MobState};
pub use roster::{Roster, RosterEntry};
pub use store::{InMemoryMobEventStore, MobEventStore};
pub use storage::MobStorage;
pub use tasks::{MobTask, TaskBoard, TaskStatus};
pub use prefab::{Prefab, PrefabInfo};
