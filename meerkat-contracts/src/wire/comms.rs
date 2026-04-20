//! Wire types for `comms/send`.
//!
//! These re-export the typed [`meerkat_core::comms::CommsCommandRequest`]
//! enum and its supporting discriminator enums so the schema-emit pipeline
//! can produce JSON schemas and SDK codegen can derive typed client bindings.
//!
//! The wire shape is `{ "kind": "<variant>", ... }` — serde-tagged on
//! `kind` with structurally enforced per-variant fields.

pub use meerkat_core::comms::{
    CommsCommandError, CommsCommandRequest, InputSource, InputStreamMode, PeerName,
};
pub use meerkat_core::interaction::ResponseStatus;
pub use meerkat_core::types::HandlingMode;
