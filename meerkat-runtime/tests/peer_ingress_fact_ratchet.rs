use std::fs;
use std::path::Path;

fn read_runtime_source(relative: &str) -> String {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(manifest_dir.join(relative)).expect("read runtime source")
}

fn production_source(source: &str) -> &str {
    source
        .split("\n#[cfg(test)]\n#[allow")
        .next()
        .unwrap_or(source)
}

fn peer_input_candidate_struct_body(source: &str) -> &str {
    let start = source
        .find("pub struct PeerInputCandidate")
        .expect("PeerInputCandidate struct exists");
    let source = &source[start..];
    let end = source
        .find("\n}\n\n")
        .expect("PeerInputCandidate struct body has closing brace");
    &source[..end]
}

#[test]
fn comms_drain_does_not_route_or_trust_on_inbox_interaction_from() {
    let source = read_runtime_source("src/comms_drain.rs");
    let source = production_source(&source);
    let forbidden = concat!("candidate.interaction", ".from");
    assert!(
        !source.contains(forbidden),
        "comms_drain routing/trust must consume PeerIngressFact, not InboxInteraction::from"
    );
}

#[test]
fn comms_bridge_does_not_project_peer_id_from_inbox_interaction_from() {
    let source = read_runtime_source("src/comms_bridge.rs");
    let source = production_source(&source);
    for forbidden in [
        concat!("interaction", ".from"),
        "interaction_to_peer_input",
        "legacy_ingress_fact_for_interaction",
    ] {
        assert!(
            !source.contains(forbidden),
            "comms_bridge prompt/schema projection must consume PeerIngressFact: {forbidden}"
        );
    }
}

#[test]
fn peer_input_candidate_does_not_duplicate_ingress_class_or_auth() {
    let source = read_runtime_source("../meerkat-core/src/interaction.rs");
    let body = peer_input_candidate_struct_body(&source);
    for forbidden in ["pub class:", "pub auth:"] {
        assert!(
            !body.contains(forbidden),
            "PeerInputCandidate must derive class/auth from PeerIngressFact, not carry duplicate field: {forbidden}"
        );
    }
}
