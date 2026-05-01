use std::fs;
use std::path::Path;

fn read_runtime_source(relative: &str) -> Result<String, String> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::read_to_string(manifest_dir.join(relative))
        .map_err(|err| format!("read runtime source `{relative}`: {err}"))
}

fn production_source(source: &str) -> &str {
    source
        .split("\n#[cfg(test)]\n#[allow")
        .next()
        .unwrap_or(source)
}

fn peer_input_candidate_struct_body(source: &str) -> Result<&str, String> {
    let start = source
        .find("pub struct PeerInputCandidate")
        .ok_or_else(|| "PeerInputCandidate struct exists".to_string())?;
    let source = &source[start..];
    let end = source
        .find("\n}\n\n")
        .ok_or_else(|| "PeerInputCandidate struct body has closing brace".to_string())?;
    Ok(&source[..end])
}

fn function_body<'a>(source: &'a str, signature: &str) -> Result<&'a str, String> {
    let start = source
        .find(signature)
        .ok_or_else(|| format!("function `{signature}` exists"))?;
    let source = &source[start..];
    let body_start = source
        .find('{')
        .ok_or_else(|| format!("function `{signature}` has body"))?;
    let mut depth = 0usize;
    for (index, byte) in source[body_start..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&source[body_start..=body_start + index]);
                }
            }
            _ => {}
        }
    }
    Err(format!("function `{signature}` body closes"))
}

#[test]
fn comms_drain_does_not_route_or_trust_on_inbox_interaction_from() -> Result<(), String> {
    let source = read_runtime_source("src/comms_drain.rs")?;
    let source = production_source(&source);
    let forbidden = concat!("candidate.interaction", ".from");
    assert!(
        !source.contains(forbidden),
        "comms_drain routing/trust must consume PeerIngressFact, not InboxInteraction::from"
    );
    Ok(())
}

#[test]
fn comms_bridge_does_not_project_peer_id_from_inbox_interaction_from() -> Result<(), String> {
    let source = read_runtime_source("src/comms_bridge.rs")?;
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
    Ok(())
}

#[test]
fn comms_drain_bridge_authority_matchers_do_not_consume_display_labels() -> Result<(), String> {
    let source = read_runtime_source("src/comms_drain.rs")?;
    let source = production_source(&source);
    for signature in [
        "fn sender_matches_bound_supervisor",
        "fn sender_matches_bridge_peer",
    ] {
        let body = function_body(source, signature)?;
        for forbidden in [".display_name", ".display_label()", "peer.name"] {
            assert!(
                !body.contains(forbidden),
                "bridge authority matcher must use canonical peer id/signing subject, not display metadata: {signature} contains {forbidden}"
            );
        }
    }
    Ok(())
}

#[test]
fn peer_input_candidate_does_not_duplicate_ingress_class_or_auth() -> Result<(), String> {
    let source = read_runtime_source("../meerkat-core/src/interaction.rs")?;
    let body = peer_input_candidate_struct_body(&source)?;
    for forbidden in ["pub class:", "pub auth:"] {
        assert!(
            !body.contains(forbidden),
            "PeerInputCandidate must derive class/auth from PeerIngressFact, not carry duplicate field: {forbidden}"
        );
    }
    Ok(())
}
