//! Skill sources.

pub mod composite;
pub mod embedded;
pub mod filesystem;
pub mod memory;

pub use composite::CompositeSkillSource;
pub use embedded::EmbeddedSkillSource;
pub use filesystem::FilesystemSkillSource;
pub use memory::InMemorySkillSource;
