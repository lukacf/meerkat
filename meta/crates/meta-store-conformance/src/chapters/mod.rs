//! Conformance chapters.
//!
//! Each chapter is an async entry point taking store factories (or, for the
//! bundled-implementation and legacy-data chapters, a filesystem root) and
//! returning `Result<(), ConformanceFailure>` with chapter/step context.
//! Chapters are per-trait capability profiles — instantiate exactly the
//! chapters a backend's declared capabilities support:
//!
//! | chapter                             | applies to |
//! |-------------------------------------|------------|
//! | [`event_log`]                       | every `EventLogStore` |
//! | [`console_log`]                     | every `ConsoleLogStore` |
//! | [`binary_blobs`]                    | every `BinaryBlobStore` |
//! | [`legacy_blob_layout`]              | the bundled `ObjectStoreBlobStore::local` only |
//! | [`agent_memory`]                    | every `AgentMemoryProvider` |
//! | [`legacy_memory_database`]          | the bundled `SqliteAgentMemoryStore` only (fabricates its schema) |
//! | [`mob_references`]                  | every `ExtensionDocumentStore` a host supplies (`meta-extension-state`) |
//! | [`mob_id_holders`]                  | every `ExtensionDocumentStore` a host supplies (`meta-extension-state`) |

mod agent_memory;
mod blobs;
mod console_log;
mod event_log;
mod extension_documents;
mod legacy_data;

pub use agent_memory::agent_memory;
pub use blobs::{binary_blobs, legacy_blob_layout};
pub use console_log::console_log;
pub use event_log::event_log;
pub use extension_documents::{mob_id_holders, mob_references};
pub use legacy_data::legacy_memory_database;
