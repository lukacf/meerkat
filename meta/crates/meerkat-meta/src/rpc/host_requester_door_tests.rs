//! The host-attested requester (`meta/send` `requester`) is reachable only
//! from the host route. These rows pin that no member- or model-reachable
//! door can name one: agent tools, peer and comms delivery, schedules, flows
//! and forks. Each row reads the door's sources and requires that none of
//! them names the host-attested send, its work-spec setter or its origin; a
//! final row requires that the only callers are the host route itself.
//! Ruling R1 of delta read 2: the attested doors record a requester origin
//! and never the typed-text turn stamp (the voice mirror's fact); a row pins
//! that no Meta or mob source stamps it.
#![allow(
    clippy::expect_used,
    reason = "source-pin rows: an unreadable source is a test failure"
)]

use std::path::{Path, PathBuf};

/// The tokens that create or carry an attested requester origin: the
/// host-attested send and its work-spec setter, and either origin value (the
/// Console's live person send records `ConsoleSignedIn`).
const HOST_ATTESTED_TOKENS: &[&str] = &[
    "send_host_attested",
    "with_host_attested_requester",
    "RequesterOrigin::HostAttested",
    "host_requester",
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

/// `text` without its inline `#[cfg(test)]` / `#[cfg(all(test, ...))]`
/// modules (`mod name { ... }`, skipped by brace depth), so a test inside a
/// shipped file is not a caller. Out-of-line test modules (`mod name;`) are
/// separate files that [`sources`] already skips.
fn without_inline_test_modules(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let test_cfg = trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test");
        if test_cfg {
            // Attributes may follow before the module line.
            let mut attributes = vec![line];
            while lines
                .peek()
                .is_some_and(|next| next.trim_start().starts_with("#["))
            {
                if let Some(next) = lines.next() {
                    attributes.push(next);
                }
            }
            let inline_module = lines.peek().is_some_and(|next| {
                let next = next.trim_start();
                (next.starts_with("mod ") || next.starts_with("pub(crate) mod "))
                    && next.trim_end().ends_with('{')
            });
            if inline_module {
                let mut depth = 0_i64;
                for body in lines.by_ref() {
                    depth += body.matches('{').count() as i64;
                    depth -= body.matches('}').count() as i64;
                    if depth <= 0 {
                        break;
                    }
                }
                continue;
            }
            for attribute in attributes {
                out.push_str(attribute);
                out.push('\n');
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Whether `text` names `token` as a whole identifier or path (so
/// `tool_grant_host_requester`, an unrelated method, is not `host_requester`).
fn names_token(text: &str, token: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(token).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + token.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// The only callers, across the WHOLE workspace (every crate under `crates/`,
/// `meta/crates/` and `connect/crates/`, inline test modules excluded): the
/// host route's `meta/send` handler calls the send; the identity runtime
/// defines it and carries it on its lanes; the bridge sets the work-spec
/// requester from it; mob defines the setter and checks it; the Console's
/// live person send records its origin; the Distiller's rule 3 reads the
/// origin. No other shipped source names any of these tokens.
#[test]
fn only_the_host_route_names_a_host_attested_requester() {
    let root = repo_root();
    let allowed = [
        "meta/crates/meerkat-meta/src/rpc.rs",
        "meta/crates/meerkat-meta/src/identity_first/runtime.rs",
        "meta/crates/meerkat-meta/src/identity_first/bridge.rs",
        "crates/meerkat-mob/src/ids.rs",
        "crates/meerkat-mob/src/runtime/actor.rs",
        // The Console's live person send (its recorded origin).
        "meta/crates/meerkat-meta/src/mob_handle_runtime.rs",
        // The Distiller's rule 3 reads the recorded origin (never sets it).
        "meta/crates/meerkat-meta/src/memory/distiller.rs",
    ]
    .map(|relative| root.join(relative).canonicalize().expect("allowed path"));
    let mut read = 0;
    for workspace in ["crates", "meta/crates", "connect/crates"] {
        let Ok(crates) = std::fs::read_dir(root.join(workspace)) else {
            panic!("{workspace}: the workspace directory reads");
        };
        for krate in crates.flatten() {
            let src = krate.path().join("src");
            if !src.is_dir() {
                continue;
            }
            for file in sources(&src) {
                let canonical = file.canonicalize().expect("canonical source path");
                if allowed.contains(&canonical) {
                    continue;
                }
                let text = without_inline_test_modules(
                    &std::fs::read_to_string(&file).expect("read source"),
                );
                read += 1;
                for token in HOST_ATTESTED_TOKENS {
                    assert!(
                        !names_token(&text, token),
                        "{} names {token}; only the host route may",
                        file.display()
                    );
                }
            }
        }
    }
    assert!(read > 100, "read the workspace's shipped sources ({read})");
    let rpc = std::fs::read_to_string(&allowed[0]).expect("rpc.rs");
    assert_eq!(
        rpc.matches(".send_host_attested(").count(),
        1,
        "meta/send is the one host-route caller"
    );
}

/// The scan's own helpers: an inline test module is skipped, a shipped item
/// after it is still read, and an unrelated identifier that merely contains a
/// token is not a match.
#[test]
fn the_workspace_scan_skips_inline_tests_and_matches_whole_tokens() {
    let text = "fn shipped() {}\n#[cfg(test)]\nmod tests {\n    fn t() { send_host_attested(); }\n}\nfn after() { x.host_requester; }\n";
    let kept = without_inline_test_modules(text);
    assert!(!kept.contains("send_host_attested"), "{kept}");
    assert!(kept.contains("fn after()"), "{kept}");
    assert!(names_token(&kept, "host_requester"));
    assert!(!names_token(
        "runtime.tool_grant_host_requester()",
        "host_requester"
    ));
    let out_of_line = "#[cfg(test)]\nmod door_tests;\nfn shipped() { send_host_attested(); }\n";
    assert!(names_token(
        &without_inline_test_modules(out_of_line),
        "send_host_attested"
    ));
}

/// Ruling R1: the attested doors never stamp a turn as typed text. That stamp
/// is the live voice mirror's fact (a typed-text turn is quiet text chat), so
/// stamping it from a signed-in console send or a host-attested send would
/// silently mute those turns on a live voice channel. No non-test Meta or mob
/// source names it; Core's own host prompt turns keep stamping it as before.
#[test]
fn no_meta_or_mob_door_stamps_typed_text() {
    let root = repo_root();
    let mut read = 0;
    for tree in ["meta/crates/meerkat-meta/src", "crates/meerkat-mob/src"] {
        for file in sources(&root.join(tree)) {
            let text = std::fs::read_to_string(&file).expect("read source");
            read += 1;
            assert!(
                !text.contains("TranscriptTurnInput::TypedText"),
                "{} stamps or reads the typed-text turn input; the attested doors record \
                 a requester origin instead",
                file.display()
            );
        }
    }
    assert!(read > 0, "read the Meta and mob sources");
}

/// Item 7: every console send surface refuses a `requester` it is given, from
/// one refusal: the three `meta/console/send` handlers and `POST /console/send`.
#[test]
fn every_console_send_surface_refuses_a_requester() {
    let path = repo_root().join("meta/crates/meerkat-meta/src/http_console.rs");
    let source = std::fs::read_to_string(&path).expect("read http_console.rs");
    assert_eq!(
        source.matches("console_send_requester_refusal(&").count(),
        3,
        "the three meta/console/send handlers check the requester"
    );
    assert!(
        source.contains("if body.requester.is_some() {")
            && source.contains("\"requester_not_accepted\""),
        "POST /console/send refuses a body requester"
    );
}
