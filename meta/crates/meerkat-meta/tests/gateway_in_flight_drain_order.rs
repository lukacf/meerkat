//! The order of the in-flight drain's steps, read from the source with
//! every comment and string literal blanked, so a mention in a comment or a
//! log line can neither stand in for a step nor hide one.
//!
//! The gateway phase stops the host's admission before it stops the
//! schedule host and the HTTP listener, and only then drains the runtime.
//! The runtime drain closes cross-mob control and identity foreground
//! admission, stops the actor-loop probe and runtime delivery, stops the mob
//! and only then waits for its runs. Both closes of the callback bridge come
//! after the phase: one follows it in `run_persistent_inner`, and `finish`,
//! which holds the other, runs once `run_persistent_inner` has returned.

#![allow(clippy::expect_used, clippy::panic)]

const GATEWAY: &str = include_str!("../src/rpc_gateway.rs");
const LIFECYCLE: &str = include_str!("../src/unified_runtime/lifecycle.rs");

fn is_ident(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The source with every comment and every string or char literal blanked
/// to spaces, newlines kept, so braces and calls are read from code only.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let blank = |character: char| if character == '\n' { '\n' } else { ' ' };
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                out.push(' ');
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                let pair = (chars[index], chars.get(index + 1).copied());
                if pair == ('/', Some('*')) {
                    depth += 1;
                    out.push_str("  ");
                    index += 2;
                } else if pair == ('*', Some('/')) {
                    depth -= 1;
                    out.push_str("  ");
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(chars[index]));
                    index += 1;
                }
            }
            continue;
        }
        let starts_raw = character == 'r'
            && (index == 0
                || !is_ident(chars[index - 1])
                || (chars[index - 1] == 'b' && (index < 2 || !is_ident(chars[index - 2]))));
        if starts_raw {
            let mut hashes = 0;
            while chars.get(index + 1 + hashes) == Some(&'#') {
                hashes += 1;
            }
            if chars.get(index + 1 + hashes) == Some(&'"') {
                let open = 2 + hashes;
                out.push_str(&" ".repeat(open));
                index += open;
                while index < chars.len() {
                    if chars[index] == '"'
                        && (1..=hashes).all(|offset| chars.get(index + offset) == Some(&'#'))
                    {
                        out.push_str(&" ".repeat(1 + hashes));
                        index += 1 + hashes;
                        break;
                    }
                    out.push(blank(chars[index]));
                    index += 1;
                }
                continue;
            }
        }
        if character == '"' {
            out.push(' ');
            index += 1;
            while index < chars.len() {
                match chars[index] {
                    '\\' => {
                        out.push(' ');
                        if let Some(escaped) = chars.get(index + 1) {
                            out.push(blank(*escaped));
                        }
                        index += 2;
                    }
                    '"' => {
                        out.push(' ');
                        index += 1;
                        break;
                    }
                    other => {
                        out.push(blank(other));
                        index += 1;
                    }
                }
            }
            continue;
        }
        if character == '\'' {
            // A char literal, not a lifetime: `'x'`, or an escape such as
            // `'\''`, `'\n'` or `'\u{7b}'`.
            let end = if next == Some('\\') {
                (index + 2..chars.len().min(index + 12))
                    .find(|&at| chars[at] == '\'' && at > index + 2)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                out.push_str(&" ".repeat(end + 1 - index));
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// The body of the one function whose code contains `signature`: from its
/// opening brace through the brace that closes it.
fn body<'a>(code: &'a str, signature: &str) -> &'a str {
    assert_eq!(code.matches(signature).count(), 1, "one {signature}");
    let start = code.find(signature).expect("the signature");
    let open = start + code[start..].find('{').expect("a body");
    let mut depth = 0usize;
    for (offset, character) in code[open..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &code[open..=open + offset];
                }
            }
            _ => {}
        }
    }
    panic!("the body of {signature} never closes");
}

/// Each step occurs exactly once in `body`, and in the order given.
fn assert_steps_in_order(body: &str, what: &str, steps: &[&str]) {
    let at: Vec<usize> = steps
        .iter()
        .map(|step| {
            assert_eq!(body.matches(step).count(), 1, "{what}: one {step}");
            body.find(step).expect("the step")
        })
        .collect();
    assert!(
        at.windows(2).all(|pair| pair[0] < pair[1]),
        "{what}: the steps {steps:?} are out of order (at {at:?})"
    );
}

/// The phase's own steps: host admission first, the runtime drain last.
#[test]
fn the_drain_phase_stops_ingress_before_it_drains_the_runtime() {
    let code = code_only(GATEWAY);
    assert_steps_in_order(
        body(&code, "async fn run_in_flight_drain_phase("),
        "run_in_flight_drain_phase",
        &[
            "in_flight_drain_plan(",
            "host.stop_admitting(",
            "schedule_host.shutdown()",
            "watchdog.abort()",
            "server.stop_accepting()",
            ".drain_in_flight_turns(",
        ],
    );
}

/// The runtime drain closes every other ingress and stops the mob before
/// it waits for the mob's runs.
#[test]
fn the_runtime_drain_closes_ingress_then_stops_the_mob_then_waits() {
    let code = code_only(LIFECYCLE);
    assert_steps_in_order(
        body(&code, "pub async fn drain_in_flight_turns("),
        "drain_in_flight_turns",
        &[
            "self.cross_mob_control_task",
            "close_foreground_operations_for_drain()",
            "cancel_identity_bootstrap()",
            "join_foreground_operations()",
            "self.actor_loop_probe_task",
            "stop_runtime_delivery()",
            "stop_mob_with(",
            "drain_stopped_runs(",
        ],
    );
}

/// Both closes of the callback bridge come after the phase: a drained turn
/// keeps its host callbacks through the drain.
#[test]
fn the_callback_bridge_closes_only_after_the_drain_phase() {
    let code = code_only(GATEWAY);
    assert_steps_in_order(
        body(&code, "async fn run_persistent_inner("),
        "run_persistent_inner",
        &[
            "announcements.stop()",
            "run_in_flight_drain_phase(",
            "host.begin_close(dispatch_end.host_reason())",
            "bridge.close()",
        ],
    );
    assert_steps_in_order(
        body(&code, "async fn run_persistent_owned("),
        "run_persistent_owned",
        &["run_persistent_inner(", "resources.finish("],
    );
    let finish = body(&code, "async fn finish(");
    assert!(
        finish.matches("bridge.close()").count() >= 1,
        "finish holds the other bridge close"
    );
    assert!(
        !finish.contains("run_in_flight_drain_phase("),
        "finish never runs the phase"
    );
}
