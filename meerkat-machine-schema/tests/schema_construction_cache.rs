//! Regression gate: generated machine-schema construction must be cached.
//!
//! A generated machine's schema is immutable for the life of the binary.
//! Before the cache, `<State>::schema()` re-parsed the entire machine (every
//! identifier through `identity::validate_slug`, fresh `IndexMap`/`String`
//! allocations) on every call; the schedule serde wire-header path invoked it
//! per row per driver tick and burned ~0.25 core per idle durable member.
//!
//! The contract enforced here is structural, not wall-clock: repeated
//! `schema_static()` calls must return the identical `&'static` allocation
//! (construction ran at most once per process), and the owned `schema()`
//! accessor must be a value-equal clone of that cached construction.

use meerkat_machine_schema::MachineSchema;
use meerkat_machine_schema::catalog::dsl;

fn assert_construction_is_cached(
    schema_static: fn() -> &'static MachineSchema,
    expected_machine: &str,
) {
    let first = schema_static();
    for _ in 0..3 {
        let again = schema_static();
        assert!(
            std::ptr::eq(first, again),
            "{expected_machine}: repeated schema_static() calls must return the same \
             &'static allocation — the schema was re-constructed"
        );
    }
    assert_eq!(
        first.machine.as_str(),
        expected_machine,
        "cached schema handle resolved to the wrong machine"
    );
}

#[test]
fn every_catalog_machine_schema_is_constructed_at_most_once() {
    use dsl::approval_lifecycle::ApprovalLifecycleMachineState;
    use dsl::auth_machine::AuthMachineState;
    use dsl::detached_job::DetachedJobMachineState;
    use dsl::meerkat_machine::MeerkatMachineState;
    use dsl::mob_host_binding_authority::MobHostBindingAuthorityState;
    use dsl::mob_machine::MobMachineState;
    use dsl::occurrence_lifecycle::OccurrenceLifecycleMachineState;
    use dsl::runtime_delivery::RuntimeDeliveryMachineState;
    use dsl::schedule_lifecycle::ScheduleLifecycleMachineState;
    use dsl::session_document::SessionDocumentMachineState;
    use dsl::session_persistence_version_authority::SessionPersistenceVersionAuthorityMachineState;
    use dsl::session_turn_admission::SessionTurnAdmissionMachineState;
    use dsl::work_attention_lifecycle::WorkAttentionLifecycleMachineState;
    use dsl::workgraph_lifecycle::WorkGraphLifecycleMachineState;

    assert_construction_is_cached(MeerkatMachineState::schema_static, "MeerkatMachine");
    assert_construction_is_cached(MobMachineState::schema_static, "MobMachine");
    assert_construction_is_cached(
        ScheduleLifecycleMachineState::schema_static,
        "ScheduleLifecycleMachine",
    );
    assert_construction_is_cached(
        OccurrenceLifecycleMachineState::schema_static,
        "OccurrenceLifecycleMachine",
    );
    assert_construction_is_cached(AuthMachineState::schema_static, "AuthMachine");
    assert_construction_is_cached(
        ApprovalLifecycleMachineState::schema_static,
        "ApprovalLifecycleMachine",
    );
    assert_construction_is_cached(DetachedJobMachineState::schema_static, "DetachedJobMachine");
    assert_construction_is_cached(
        RuntimeDeliveryMachineState::schema_static,
        "RuntimeDeliveryMachine",
    );
    assert_construction_is_cached(
        SessionDocumentMachineState::schema_static,
        "SessionDocumentMachine",
    );
    assert_construction_is_cached(
        SessionTurnAdmissionMachineState::schema_static,
        "SessionTurnAdmissionMachine",
    );
    assert_construction_is_cached(
        WorkGraphLifecycleMachineState::schema_static,
        "WorkGraphLifecycleMachine",
    );
    assert_construction_is_cached(
        WorkAttentionLifecycleMachineState::schema_static,
        "WorkAttentionLifecycleMachine",
    );
    assert_construction_is_cached(
        MobHostBindingAuthorityState::schema_static,
        "MobHostBindingAuthority",
    );
    assert_construction_is_cached(
        SessionPersistenceVersionAuthorityMachineState::schema_static,
        "SessionPersistenceVersionAuthorityMachine",
    );
}

#[test]
fn owned_schema_accessor_clones_the_cached_construction() {
    let cached = dsl::occurrence_lifecycle::OccurrenceLifecycleMachineState::schema_static();
    let owned = dsl::occurrence_lifecycle::OccurrenceLifecycleMachineState::schema();
    assert_eq!(&owned, cached);
}
