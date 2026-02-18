//! Meerkat Mob orchestration plugin.

pub mod error;
pub mod model;
pub mod resolver;
pub mod runtime;
pub mod runtime_service;
pub mod service;
pub mod spec;
pub mod store;

pub use error::{MobError, MobResult};
pub use model::*;
pub use resolver::*;
pub use runtime::{MobRuntime, MobRuntimeBuilder, RustToolBundleRegistry};
pub use runtime_service::MobRuntimeService;
pub use service::MobService;
pub use spec::{ApplyContext, ApplySpecRequest, SpecValidator};
pub use store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore, MobRunStore,
    MobSpecStore, RedbMobEventStore, RedbMobRunStore, RedbMobSpecStore,
};
