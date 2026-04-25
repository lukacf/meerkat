use meerkat_machine_codegen::render_machine_semantic_model;
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};

#[test]
fn meerkat_semantic_model_keeps_internal_session_transport_domain() {
    let rendered = render_machine_semantic_model(&meerkat_machine());

    assert!(rendered.contains("SessionIdValues"));
    assert!(rendered.contains(
        "PrepareBindingsInitializing(agent_runtime_id, fence_token, generation, arg_session_id) =="
    ));
    assert!(!rendered.contains("MeerkatIdValues"));
}

#[test]
fn mob_semantic_model_is_identity_and_runtime_native() {
    let rendered = render_machine_semantic_model(&mob_machine());

    assert!(rendered.contains("AgentIdentityValues"));
    assert!(rendered.contains("AgentRuntimeIdValues"));
    assert!(rendered.contains("FenceTokenValues"));
    // W3-H-1: SessionIdValues is now present in the MobMachine semantic
    // model because `member_session_bindings: Map<AgentIdentity, SessionId>`
    // makes the bridge session id a first-class MobMachine-owned value.
    // This is the intentional expansion that issue #264 calls for — the
    // binding map is the canonical join between identity continuity
    // (MobMachine) and realtime attachment (MeerkatMachine).
    assert!(rendered.contains("SessionIdValues"));
    assert!(!rendered.contains("MeerkatIdValues"));
}
