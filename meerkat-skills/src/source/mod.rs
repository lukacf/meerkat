//! Skill sources.

pub mod composite;
pub mod embedded;
pub mod filesystem;
pub mod git;
#[cfg(any(feature = "skills-http", test))]
pub mod http;
pub mod memory;

pub use composite::{CompositeSkillSource, NamedSource};
pub use embedded::EmbeddedSkillSource;
pub use filesystem::FilesystemSkillSource;
pub use memory::InMemorySkillSource;
