//! The host-attested requester (`meta/send` `requester`) is reachable only
//! from the host route. These rows pin that no member- or model-reachable
//! door can name one: agent tools, peer and comms delivery, schedules, flows
//! and forks. Each row reads the door's sources and requires that none of
//! them names the host-attested send, its work-spec setter or its origin; a
//! final row requires that the only callers are the host route itself.
//! The attested doors record a requester origin
//! and never the typed-text turn stamp (the voice mirror's fact); a row pins
//! that no Meta or mob source stamps it.
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "source-pin rows: an unreadable source or a pinned token is a test failure"
)]

use std::path::{Path, PathBuf};

/// The tokens that create or carry an attested requester origin: the
/// host-attested send and its work-spec setter, either origin value (the
/// Console's live person send records `ConsoleSignedIn`), and the setters
/// that FORWARD an origin from one carrier to the next (a work spec's
/// `with_requester_origin`, a `requester_origin = ...` assignment). A
/// requester's own `with_origin(...)` is checked in files that name
/// `TurnRequester` (see [`names_a_forwarding_setter`]).
const HOST_ATTESTED_TOKENS: &[&str] = &[
    "send_host_attested",
    "with_host_attested_requester",
    "RequesterOrigin::HostAttested",
    "host_requester",
    "RequesterOrigin::ConsoleSignedIn",
    "with_requester_origin",
    "requester_origin =",
    // The attested wire spellings: the only texts the origin decoder turns
    // into an attested origin, so a source that holds one could build one by
    // decoding.
    "\"console_signed_in\"",
    "\"host_attested\"",
];

/// Whether `text` names an attested-origin token, the forwarding setters
/// included. `with_origin(` counts only in a source that names
/// `TurnRequester`: other types (an identity entry's origin) have their own
/// unrelated `with_origin`.
fn names_a_forwarding_setter(text: &str) -> Option<&'static str> {
    if let Some(token) = HOST_ATTESTED_TOKENS
        .iter()
        .find(|token| names_token(text, token))
    {
        return Some(token);
    }
    if sets_requester_origin_field(text) {
        return Some("requester_origin field");
    }
    (text.contains("TurnRequester") && text.contains(".with_origin(")).then_some(".with_origin(")
}

/// Whether `text` sets or moves a `requester_origin` field to something other
/// than the literal `None`: `requester_origin: value` (not `: None`, and not
/// a type such as `: Option<...>`) or the shorthand `requester_origin,` /
/// `requester_origin }` in a struct literal or pattern.
fn sets_requester_origin_field(text: &str) -> bool {
    const FIELD: &str = "requester_origin";
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(FIELD).any(|(at, _)| {
        if text[..at].chars().next_back().is_some_and(is_ident) {
            return false;
        }
        let rest = &text[at + FIELD.len()..];
        if rest.chars().next().is_some_and(is_ident) {
            return false;
        }
        let rest = rest.trim_start();
        if let Some(value) = rest.strip_prefix(':') {
            if value.starts_with(':') {
                return false; // a path, `requester_origin::...`
            }
            let value = value.trim_start();
            return !(value.starts_with("None") || value.starts_with("Option<"));
        }
        if rest.starts_with(',') || rest.starts_with('}') {
            let before = text[..at].trim_end();
            return before.ends_with('{') || before.ends_with(',') || before.ends_with('(');
        }
        false
    })
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Every Rust source under `path` (a file or a directory) except test
/// modules by convention: a `tests` directory, `tests.rs` and `*_tests.rs`.
/// A file whose name merely contains "test" (a test-support bridge, a
/// `test_support.rs`) is read: it may hold product code. Inline test modules
/// inside a read file are dropped by [`without_inline_test_modules`].
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
            } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
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
            let text = without_inline_test_modules(
                &std::fs::read_to_string(&file).expect("read the door's source"),
            );
            read += 1;
            if let Some(token) = names_a_forwarding_setter(&text) {
                panic!(
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
/// `meta/crates/` and `connect/crates/`, test modules excluded): the host
/// route's `meta/send` handler calls the send; the identity runtime defines
/// it and carries it on its lanes; the bridge sets the work-spec requester
/// from it; mob defines the setters, checks them and forwards the origin to
/// the runtime input; Core's turn semantics carry it; the runtime stamps the
/// input header's origin onto the turn's requester; the Console's live
/// person send records its origin; the Distiller's rule 3 reads it. No other
/// shipped source names any of these tokens or forwarding setters.
#[test]
fn only_the_host_route_names_a_host_attested_requester() {
    let root = repo_root();
    let allowed = [
        "meta/crates/meerkat-meta/src/rpc.rs",
        "meta/crates/meerkat-meta/src/identity_first/runtime.rs",
        "meta/crates/meerkat-meta/src/identity_first/bridge.rs",
        "crates/meerkat-mob/src/ids.rs",
        "crates/meerkat-mob/src/runtime/actor.rs",
        // Core's turn semantics carry the origin from the host to the turn.
        "crates/meerkat-core/src/service/mod.rs",
        // The runtime stamps the admitted input's origin onto the requester,
        // and carries it on the input through its state transitions.
        "crates/meerkat-runtime/src/input.rs",
        "crates/meerkat-runtime/src/input_state.rs",
        "crates/meerkat-runtime/src/runtime_loop.rs",
        // Mob carries the work spec's origin onto the member's runtime input.
        "crates/meerkat-mob/src/runtime/handle.rs",
        "crates/meerkat-mob/src/runtime/provisioner.rs",
        // The Console's live person send (its recorded origin).
        "meta/crates/meerkat-meta/src/mob_handle_runtime.rs",
        // The Distiller's rule 3 reads the recorded origin (never sets it).
        "meta/crates/meerkat-meta/src/memory/distiller.rs",
        // Core defines the origin, its wire spellings and its decoder.
        "crates/meerkat-core/src/types.rs",
    ]
    .map(|relative| root.join(relative).canonicalize().expect("allowed path"));
    // One unrelated use of an attested spelling, allowed by its exact line:
    // a mob announcement source's own principal kind, which never builds a
    // requester origin.
    let announcement_kind = root
        .join("meta/crates/meerkat-meta/src/mob_announcements/sources.rs")
        .canonicalize()
        .expect("announcement sources path");
    const ANNOUNCEMENT_KIND_LINE: &str = "Self::HostAttested { .. } => \"host_attested\",";
    let mut read = 0;
    for workspace in ["crates", "meta/crates", "connect/crates"] {
        let crates =
            std::fs::read_dir(root.join(workspace)).expect("the workspace directory reads");
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
                let mut text = without_inline_test_modules(
                    &std::fs::read_to_string(&file).expect("read source"),
                );
                if canonical == announcement_kind {
                    assert_eq!(
                        text.matches(ANNOUNCEMENT_KIND_LINE).count(),
                        1,
                        "the announcement kind line is the one allowed spelling"
                    );
                    text = text.replace(ANNOUNCEMENT_KIND_LINE, "");
                }
                read += 1;
                if let Some(token) = names_a_forwarding_setter(&text) {
                    panic!("{} names {token}; only the host route may", file.display());
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
    assert_eq!(
        names_a_forwarding_setter("spec.with_requester_origin(origin)"),
        Some("with_requester_origin")
    );
    assert_eq!(
        names_a_forwarding_setter("delivery.requester_origin = origin;"),
        Some("requester_origin =")
    );
    assert_eq!(
        names_a_forwarding_setter("TurnRequester::direct(p).with_origin(o)"),
        Some(".with_origin(")
    );
    assert_eq!(
        names_a_forwarding_setter("entry.with_origin(IdentityEntryOrigin::Created)"),
        None,
        "another type's with_origin is not the requester's"
    );
    for set in [
        "Header { requester_origin: spec.requester_origin, x }",
        "Header { requester, requester_origin, x }",
        "let Header { requester_origin } = header;",
        "work(a, requester_origin, b)",
    ] {
        assert_eq!(
            names_a_forwarding_setter(set),
            Some("requester_origin field"),
            "{set}"
        );
    }
    assert_eq!(
        names_a_forwarding_setter("let o: RequesterOrigin = from_value(json!(\"host_attested\"));"),
        Some("\"host_attested\"")
    );
    assert_eq!(
        names_a_forwarding_setter("decode(\"console_signed_in\")"),
        Some("\"console_signed_in\"")
    );
    for unset in [
        "Header { requester_origin: None, x }",
        "pub requester_origin: Option<meerkat_core::RequesterOrigin>,",
        "fn f(requester_origin: Option<RequesterOrigin>) {}",
        "let tool_requester_origin = 1;",
    ] {
        assert_eq!(names_a_forwarding_setter(unset), None, "{unset}");
    }
    let out_of_line = "#[cfg(test)]\nmod door_tests;\nfn shipped() { send_host_attested(); }\n";
    assert!(names_token(
        &without_inline_test_modules(out_of_line),
        "send_host_attested"
    ));
}

/// The attested doors never stamp a turn as typed text. That stamp
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

/// Every console send surface refuses a `requester` it is given, from
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
