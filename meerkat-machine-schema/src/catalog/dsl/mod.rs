//! DSL-generated machine schemas.
//!
//! These modules contain `machine!` invocations that generate the same
//! `MachineSchema` values as the hand-written catalog entries. They use
//! `rust: "self"` so the generated `schema()` function references
//! `crate::MachineSchema` instead of `meerkat_machine_schema::MachineSchema`.
#![allow(
    dead_code,
    unused_variables,
    unreachable_code,
    clippy::cmp_owned,
    clippy::assign_op_pattern
)]

/// Extension trait providing `.get()` on Option to support the `option_value`
/// schema pattern (`Expr::MapGet { map: Field(...), key: String("value") }`).
/// In the runtime dispatch code, `.get("value")` extracts the inner value.
/// This is only used by the generated dispatch code in this crate (which is
/// dead code — only the `schema()` function is called).
trait OptionValueExt<T: Clone> {
    fn get(&self, _key: &str) -> T;
}
impl<T: Clone + Default> OptionValueExt<T> for Option<T> {
    fn get(&self, _key: &str) -> T {
        self.clone().unwrap_or_default()
    }
}

pub mod auth_machine;
pub mod meerkat_machine;
pub mod mob_machine;
pub mod occurrence_lifecycle;
pub mod schedule_lifecycle;

use crate::{MachineSchema, NamedTypeBinding};

/// Attach authoritative [`NamedTypeBinding`]s to a DSL-generated schema.
///
/// The `machine!` macro does not yet emit `named_types` entries, so the
/// catalog wrappers supply them here. This is the single source of truth
/// for how DSL-declared named types lower to Rust atoms — the codegen
/// consults these bindings rather than matching on type names.
fn with_named_types(mut schema: MachineSchema, bindings: Vec<NamedTypeBinding>) -> MachineSchema {
    schema.named_types = bindings;
    schema
}

pub fn dsl_auth_machine() -> MachineSchema {
    auth_machine::AuthMachineState::schema()
}

pub fn dsl_meerkat_machine() -> MachineSchema {
    with_named_types(
        meerkat_machine::MeerkatMachineState::schema(),
        vec![
            NamedTypeBinding::u64("BoundarySequence"),
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("CommsRuntimeId"),
            NamedTypeBinding::string("InputId"),
            NamedTypeBinding::string("McpServerId"),
            NamedTypeBinding::string("MobId"),
            NamedTypeBinding::string("OperationId"),
            NamedTypeBinding::string("PeerCorrelationId"),
            NamedTypeBinding::string("RunId"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("ToolFilter"),
            NamedTypeBinding::string("WorkId"),
            // Wave-c C-6r: typed PeerEndpoint twin.
            NamedTypeBinding::type_path(
                "PeerEndpoint",
                "crate::catalog::dsl::meerkat_machine::PeerEndpoint",
            ),
            NamedTypeBinding::type_path(
                "PeerName",
                "crate::catalog::dsl::meerkat_machine::PeerName",
            ),
            NamedTypeBinding::type_path("PeerId", "crate::catalog::dsl::meerkat_machine::PeerId"),
            NamedTypeBinding::type_path(
                "PeerAddress",
                "crate::catalog::dsl::meerkat_machine::PeerAddress",
            ),
        ],
    )
}

pub fn dsl_mob_machine() -> MachineSchema {
    with_named_types(
        mob_machine::MobMachineState::schema(),
        vec![
            NamedTypeBinding::u64("FenceToken"),
            NamedTypeBinding::u64("Generation"),
            NamedTypeBinding::string("AgentIdentity"),
            NamedTypeBinding::string("AgentRuntimeId"),
            NamedTypeBinding::string("SessionId"),
            NamedTypeBinding::string("TaskId"),
            NamedTypeBinding::string("WorkId"),
        ],
    )
}

pub fn dsl_schedule_lifecycle_machine() -> MachineSchema {
    schedule_lifecycle::ScheduleLifecycleMachineState::schema()
}

pub fn dsl_occurrence_lifecycle_machine() -> MachineSchema {
    with_named_types(
        occurrence_lifecycle::OccurrenceLifecycleMachineState::schema(),
        vec![
            NamedTypeBinding::string("OccurrenceId"),
            NamedTypeBinding::string("ScheduleId"),
        ],
    )
}
