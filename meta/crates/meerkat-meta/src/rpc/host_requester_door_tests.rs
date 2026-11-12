//! The host-attested requester (`meta/send` `requester`) is reachable only
//! from the host route. These rows pin that no member- or model-reachable
//! door can name one: agent tools, peer and comms delivery, schedules, flows
//! and forks. Each row reads the door's sources and requires that none of
//! them names the host-attested send, its work-spec setter or its origin; a
//! final row requires that the only callers are the host route itself.

use std::path::{Path, PathBuf};

/// The tokens that create or carry a host-attested requester, or stamp a
/// person's typed-text authorship on work (`with_person_typed_text`, set only
/// by the Console's live person send).
const HOST_ATTESTED_TOKENS: &[&str] = &[
    "send_host_attested",
    "with_host_attested_requester",
    "RequesterOrigin::HostAttested",
    "host_requester",
    "with_person_typed_text",
    "RequesterOrigin::ConsoleSignedIn",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Every non-test Rust source under `path` (a file or a directory).
fn sources(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if path.is_file() {
        out.push(path.to_path_buf());
        return out;
    }
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() {
                if name != "tests" {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") && !name.contains("test") {
                out.push(path);
            }
        }
    }
    out
}

fn assert_door_names_no_host_attested_requester(door: &str, paths: &[&str]) {
    let root = repo_root();
    let mut read = 0;
    for relative in paths {
        let path = root.join(relative);
        assert!(path.exists(), "{door}: {relative} exists");
        for file in sources(&path) {
            let text = std::fs::read_to_string(&file).expect("read the door's source");
            read += 1;
            for token in HOST_ATTESTED_TOKENS {
                assert!(
                    !text.contains(token),
                    "{door}: {} names {token}; only the host route may",
                    file.display()
                );
            }
        }
    }
    assert!(read > 0, "{door}: read its sources");
}

#[test]
fn agent_tools_cannot_name_a_host_attested_requester() {
    assert_door_names_no_host_attested_requester(
        "agent tools",
        &[
            "crates/meerkat-mob/src/runtime/tools.rs",
            "crates/meerkat-mob-mcp/src",
            "meta/crates/meerkat-meta/src/tool_compose.rs",
            "meta/crates/meerkat-meta/src/identity_first/customizer_tools.rs",
        ],
    );
}

#[test]
fn peer_and_comms_delivery_cannot_name_a_host_attested_requester() {
    assert_door_names_no_host_attested_requester(
        "peer/comms",
        &[
            "crates/meerkat-comms/src",
            "meta/crates/meerkat-meta/src/member_comms_id.rs",
        ],
    );
}

#[test]
fn schedules_cannot_name_a_host_attested_requester() {
    assert_door_names_no_host_attested_requester(
        "schedules",
        &[
            "crates/meerkat-schedule/src",
            "crates/meerkat-mob/src/runtime/host_schedule.rs",
            "meta/crates/meerkat-meta/src/schedule_wiring.rs",
            "meta/crates/meerkat-meta/src/console_schedules.rs",
            "meta/crates/meerkat-meta/src/rpc/schedule_methods.rs",
        ],
    );
}

#[test]
fn flows_cannot_name_a_host_attested_requester() {
    assert_door_names_no_host_attested_requester(
        "flows",
        &[
            "crates/meerkat-mob/src/runtime/flow.rs",
            "crates/meerkat-mob/src/runtime/flow_frame_engine.rs",
            "crates/meerkat-mob/src/runtime/remote_flow_ticket.rs",
            "meta/crates/meerkat-meta/src/flow_role_targets.rs",
        ],
    );
}

#[test]
fn forks_cannot_name_a_host_attested_requester() {
    assert_door_names_no_host_attested_requester(
        "forks",
        &[
            "crates/meerkat-mob/src/runtime/fork_build.rs",
            "crates/meerkat-mob/src/runtime/fork_context.rs",
            "crates/meerkat-mob/src/runtime/forked_participant_routing.rs",
            "meta/crates/meerkat-meta/src/fork.rs",
        ],
    );
}

/// The only callers: the host route's `meta/send` handler calls the send;
/// the identity runtime defines it and carries it on its lanes; the bridge
/// sets the work-spec requester from it; mob defines the setter and checks
/// it. No other non-test source in Meta or mob names any of these tokens.
#[test]
fn only_the_host_route_names_a_host_attested_requester() {
    let root = repo_root();
    let allowed = [
        "meta/crates/meerkat-meta/src/rpc.rs",
        "meta/crates/meerkat-meta/src/identity_first/runtime.rs",
        "meta/crates/meerkat-meta/src/identity_first/bridge.rs",
        "crates/meerkat-mob/src/ids.rs",
        "crates/meerkat-mob/src/runtime/actor.rs",
        // The Console's live person send (typed text and its origin).
        "meta/crates/meerkat-meta/src/mob_handle_runtime.rs",
    ]
    .map(|relative| root.join(relative).canonicalize().expect("allowed path"));
    for tree in ["meta/crates/meerkat-meta/src", "crates/meerkat-mob/src"] {
        for file in sources(&root.join(tree)) {
            let canonical = file.canonicalize().expect("canonical source path");
            if allowed.contains(&canonical) {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("read source");
            for token in HOST_ATTESTED_TOKENS {
                assert!(
                    !text.contains(token),
                    "{} names {token}; only the host route may",
                    file.display()
                );
            }
        }
    }
    let rpc = std::fs::read_to_string(&allowed[0]).expect("rpc.rs");
    assert_eq!(
        rpc.matches(".send_host_attested(").count(),
        1,
        "meta/send is the one host-route caller"
    );
}
