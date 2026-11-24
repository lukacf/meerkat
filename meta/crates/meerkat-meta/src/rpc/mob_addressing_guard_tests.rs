//! Source guard for the handlers below the addressing seam.
//!
//! A handler that serves one mob holds a resolved mob and reads that mob
//! through it. A method of the runtime that means the default mob
//! ([`DEFAULT_MOB_METHODS`]) answers for the default mob whatever mob the
//! request named, so no handler may call one on the runtime, neither on a
//! `runtime` it was handed nor through the resolved mob's `runtime()`.
//!
//! Four methods of the runtime read the default mob's wrapper and are not
//! on that list, because each answers the same whatever mob a request
//! names: the list keeps them in groups of their own beside it.
//! `speaks_for_composition` and `resolved_storage` are facts of the launch
//! ([`LAUNCH_FACTS`]: whether the launch speaks for its composition; the
//! storage summary of its state directory). `quarantined_mobs` and
//! `binary_blob_store` are facts of the host ([`HOST_FACTS`]: the mobs of
//! its mob set whose stores did not restore; the host's blob store, which
//! the host-scoped `meta/blob/get` reads without resolving a mob). A
//! handler that reads one through the runtime reads nothing of another
//! mob.
//! `the_two_launch_facts_answer_the_same_for_every_mob_a_request_names` and
//! `the_host_facts_are_the_same_for_every_mob_a_request_names` pin that
//! for exactly the names of the two groups, so that no per-mob fact can be
//! put into one of them unseen.
//!
//! No method of the list is skipped. Every operation a handler does on one
//! mob has a form on the resolved mob (`MobEntryRef`), under the name the
//! runtime's own method has or had, and the handler calls that form: on the
//! resolved mob it holds, or, where it hands the operation an owner of the
//! runtime, on the mob resolved again by its id.
//!
//! What this guard does not see, said plainly:
//! - a free function that takes the runtime and reads the default mob
//!   inside (two forms are named and refused here: `DefaultMob::of_runtime`
//!   and a handler's own `DefaultMob::new`);
//! - a call on a runtime under a name other than `runtime` and
//!   `runtime_owner`, where nothing declares its type (a `let` without a
//!   type, a closure's parameter); a declared runtime is kept to those
//!   names by `every_runtime_a_handler_holds_has_a_name_the_guard_reads`;
//! - a runtime that is a field of another value (`value.runtime.method()`
//!   and `value.runtime.runtime().method()` are skipped, because the
//!   identity context's `runtime` field is another type, and so is what its
//!   `runtime()` hands out);
//! - what a value named `runtime` is: a local of another type under that
//!   name is read as the runtime, and its call of a listed name is counted
//!   (the handlers bind the identity runtime as `identity_runtime` or
//!   `identity_rt`);
//! - the console's handlers, which do not hold the runtime at all: they
//!   read the default mob's handles from the console's state, and move to
//!   the resolved mob's handles with their own rows.

#![allow(clippy::expect_used, clippy::panic)]

use crate::unified_runtime::default_mob_methods_tests::{
    DEFAULT_MOB_METHODS, HOST_FACTS, LAUNCH_FACTS,
};

/// The production files of the RPC handlers, by module path.
const HANDLER_FILES: &[(&str, &str)] = &[
    ("rpc.rs", include_str!("../rpc.rs")),
    ("rpc/console_ingress.rs", include_str!("console_ingress.rs")),
    ("rpc/forget_methods.rs", include_str!("forget_methods.rs")),
    ("rpc/gating_methods.rs", include_str!("gating_methods.rs")),
    (
        "rpc/identity_methods.rs",
        include_str!("identity_methods.rs"),
    ),
    ("rpc/memory_methods.rs", include_str!("memory_methods.rs")),
    (
        "rpc/memory_scope_methods.rs",
        include_str!("memory_scope_methods.rs"),
    ),
    ("rpc/mob_addressing.rs", include_str!("mob_addressing.rs")),
    ("rpc/mob_hosting.rs", include_str!("mob_hosting.rs")),
    ("rpc/mob_methods.rs", include_str!("mob_methods.rs")),
    (
        "rpc/operator_methods.rs",
        include_str!("operator_methods.rs"),
    ),
    ("rpc/params.rs", include_str!("params.rs")),
    ("rpc/purge_methods.rs", include_str!("purge_methods.rs")),
    (
        "rpc/question_set_methods.rs",
        include_str!("question_set_methods.rs"),
    ),
    (
        "rpc/realm_mcp_methods.rs",
        include_str!("realm_mcp_methods.rs"),
    ),
    (
        "rpc/routing_delivery_methods.rs",
        include_str!("routing_delivery_methods.rs"),
    ),
    (
        "rpc/schedule_methods.rs",
        include_str!("schedule_methods.rs"),
    ),
    (
        "rpc/session_store_methods.rs",
        include_str!("session_store_methods.rs"),
    ),
    ("rpc/storage_methods.rs", include_str!("storage_methods.rs")),
    (
        "rpc/subscribe_methods.rs",
        include_str!("subscribe_methods.rs"),
    ),
    (
        "rpc/tool_grant_methods.rs",
        include_str!("tool_grant_methods.rs"),
    ),
    (
        "rpc/topology_methods.rs",
        include_str!("topology_methods.rs"),
    ),
    (
        "rpc/workgraph_methods.rs",
        include_str!("workgraph_methods.rs"),
    ),
];

/// The other files that hold a request door.
const DOOR_FILES: &[(&str, &str)] = &[
    ("http_console.rs", include_str!("../http_console.rs")),
    ("http_sse.rs", include_str!("../http_sse.rs")),
];

/// `source` with every comment, string literal and character literal
/// replaced by spaces (line breaks kept), so that what is left is code.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for byte in &mut out[from..to] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |n| at + n);
            blank(&mut out, at, end);
            at = end;
        } else if rest.starts_with(b"/*") {
            let mut depth = 1;
            let mut end = at + 2;
            while end < bytes.len() && depth > 0 {
                if bytes[end..].starts_with(b"/*") {
                    depth += 1;
                    end += 2;
                } else if bytes[end..].starts_with(b"*/") {
                    depth -= 1;
                    end += 2;
                } else {
                    end += 1;
                }
            }
            blank(&mut out, at, end);
            at = end;
        } else if rest[0] == b'"' {
            let mut end = at + 1;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            let end = (end + 1).min(bytes.len());
            blank(&mut out, at, end);
            at = end;
        } else if rest[0] == b'r' && raw_string_hashes(rest).is_some() {
            let hashes = raw_string_hashes(rest).unwrap_or(0);
            let mut close = vec![b'"'];
            close.extend(std::iter::repeat_n(b'#', hashes));
            let body = at + 2 + hashes;
            let end = bytes[body..]
                .windows(close.len())
                .position(|window| window == close.as_slice())
                .map_or(bytes.len(), |n| body + n + close.len());
            blank(&mut out, at, end);
            at = end;
        } else if rest[0] == b'\'' {
            // A character literal is `'x'` or `'\..'`; a lifetime has no
            // closing quote that near.
            let end = if rest.len() > 2 && rest[1] == b'\\' {
                rest[2..]
                    .iter()
                    .position(|b| *b == b'\'')
                    .map(|n| at + 3 + n)
            } else if rest.len() > 2 && rest[2] == b'\'' {
                Some(at + 3)
            } else {
                None
            };
            match end {
                Some(end) => {
                    blank(&mut out, at, end);
                    at = end;
                }
                None => at += 1,
            }
        } else {
            at += 1;
        }
    }
    String::from_utf8(out).expect("blanking keeps the text")
}

/// The number of `#` of a raw string that starts `rest`, if one does and
/// the `r` is no part of a longer name.
fn raw_string_hashes(rest: &[u8]) -> Option<usize> {
    let hashes = rest[1..].iter().take_while(|b| **b == b'#').count();
    (rest.get(1 + hashes) == Some(&b'"')).then_some(hashes)
}

/// `code` (comments and literals already blanked) with every test module
/// blanked: an item `mod name { ... }` under `#[cfg(test)]`.
fn without_test_modules(code: &str) -> String {
    let mut out = code.as_bytes().to_vec();
    let mut search = 0;
    while let Some(found) = code[search..].find("#[cfg(test)]") {
        let gate = search + found;
        search = gate + 1;
        // The item the gate is on: skip further attributes.
        let mut item = gate + "#[cfg(test)]".len();
        loop {
            let rest = code[item..].trim_start();
            item = code.len() - rest.len();
            if rest.starts_with("#[") {
                let mut depth = 0usize;
                let mut end = item;
                for (offset, byte) in rest.bytes().enumerate() {
                    match byte {
                        b'[' => depth += 1,
                        b']' => {
                            depth -= 1;
                            if depth == 0 {
                                end = item + offset + 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                item = end;
            } else {
                break;
            }
        }
        let rest = &code[item..];
        let head = rest.split(['{', ';']).next().unwrap_or("");
        let is_module = head.split_whitespace().any(|word| word == "mod");
        if !is_module || !rest[head.len()..].starts_with('{') {
            continue;
        }
        let open = item + head.len();
        let mut depth = 0usize;
        let mut close = code.len();
        for (offset, byte) in code[open..].bytes().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        for byte in &mut out[gate..close] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        search = close.min(code.len());
    }
    String::from_utf8(out).expect("blanking keeps the text")
}

/// The production code of `source`, with no white space left in it.
fn production_code(source: &str) -> String {
    without_test_modules(&code_only(source))
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The production code of `source` with its white space kept, so that a
/// name stays apart from the word before it (`match runtime.mob_id()`).
fn production_text(source: &str) -> String {
    without_test_modules(&code_only(source))
}

/// The names a handler gives a value that is the runtime.
/// `every_runtime_a_handler_holds_has_a_name_the_guard_reads` keeps a
/// declared runtime to these names.
const RUNTIME_NAMES: &[&str] = &["runtime", "runtime_owner"];

/// The offset after any white space at `at`.
fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The offset after `word`, if `word` stands at `at` as a whole name.
fn after_name(bytes: &[u8], at: usize, word: &str) -> Option<usize> {
    let end = at + word.len();
    let starts = at == 0 || !is_name_byte(bytes[at - 1]);
    let stands = bytes.get(at..end) == Some(word.as_bytes());
    let ends = !bytes.get(end).copied().is_some_and(is_name_byte);
    (starts && stands && ends).then_some(end)
}

/// Whether the text before `at` ends, white space aside, with a field named
/// `runtime` (`value.runtime`): the identity context's field, which is
/// another type than the runtime.
fn ends_with_a_runtime_field(bytes: &[u8], at: usize) -> bool {
    let word = b"runtime";
    let Some(end) = bytes[..at]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|last| last + 1)
    else {
        return false;
    };
    let Some(start) = end.checked_sub(word.len()) else {
        return false;
    };
    bytes.get(start..end) == Some(word.as_slice())
        && bytes[..start]
            .iter()
            .rev()
            .find(|byte| !byte.is_ascii_whitespace())
            == Some(&b'.')
}

/// How often `text` (production code, white space kept) calls `method` on
/// the runtime:
/// - on a value named as [`RUNTIME_NAMES`] say (a whole name, and not a
///   field of another value);
/// - on any value's `runtime()`, which is how a resolved mob hands out the
///   runtime, except the `runtime()` of a field named `runtime` (the
///   identity context's, which hands out the identity runtime);
/// - by path, `UnifiedRuntime::method`.
fn calls_on_the_runtime(text: &str, method: &str) -> usize {
    let bytes = text.as_bytes();
    let mut calls = 0;
    for name in RUNTIME_NAMES {
        for (at, _) in text.match_indices(name) {
            let Some(mut next) = after_name(bytes, at, name) else {
                continue;
            };
            let before = bytes[..at]
                .iter()
                .rposition(|byte| !byte.is_ascii_whitespace());
            next = skip_space(bytes, next);
            if let Some(dot) = before.filter(|dot| bytes[*dot] == b'.') {
                // `value.runtime()` is the runtime; `value.runtime` is a
                // field of another value, and so is what that field's
                // `runtime()` hands out.
                if *name != "runtime"
                    || bytes.get(next) != Some(&b'(')
                    || ends_with_a_runtime_field(bytes, dot)
                {
                    continue;
                }
                next = skip_space(bytes, next + 1);
                if bytes.get(next) != Some(&b')') {
                    continue;
                }
                next = skip_space(bytes, next + 1);
            }
            if bytes.get(next) != Some(&b'.') {
                continue;
            }
            next = skip_space(bytes, next + 1);
            let Some(next) = after_name(bytes, next, method) else {
                continue;
            };
            if bytes.get(skip_space(bytes, next)) == Some(&b'(') {
                calls += 1;
            }
        }
    }
    let by_path = "UnifiedRuntime";
    for (at, _) in text.match_indices(by_path) {
        let Some(next) = after_name(bytes, at, by_path) else {
            continue;
        };
        let next = skip_space(bytes, next);
        if bytes.get(next..next + 2) != Some(b"::".as_slice()) {
            continue;
        }
        if after_name(bytes, skip_space(bytes, next + 2), method).is_some() {
            calls += 1;
        }
    }
    calls
}

/// The lexer pass leaves code and removes what only looks like code.
#[test]
fn the_guards_reading_of_a_file_leaves_code_only() {
    let source = r##"
fn a(runtime: &R) { runtime.mob_handle(); } // runtime.mob_id()
/* runtime.mob_id() /* nested */ runtime.mob_id() */
const TEXT: &str = "runtime.mob_id()"; const RAW: &str = r#"runtime.mob_id()"#;
fn b<'a>(x: &'a str) -> char { let _ = '"'; 'x' }
#[cfg(test)]
#[allow(dead_code)]
mod tests { fn c(runtime: &R) { runtime.mob_id(); } }
#[cfg(test)]
mod elsewhere;
fn d(mob: &M) { mob.runtime().mob_id(); identity_runtime.mob_id(); ctx.runtime.mob_id(); }
fn e(runtime: &R) { match runtime.rediscover() {} }
"##;
    let code = production_code(source);
    let text = production_text(source);
    assert_eq!(calls_on_the_runtime(&text, "mob_handle"), 1, "{text}");
    assert_eq!(calls_on_the_runtime(&text, "mob_id"), 1, "{text}");
    // A call that follows a keyword is a call on the runtime.
    assert_eq!(calls_on_the_runtime(&text, "rediscover"), 1, "{text}");
    assert!(code.contains("modelsewhere;"), "{code}");
    assert!(!code.contains("fnc("), "{code}");
}

/// The scan for calls on the runtime sees a call after a keyword, over a
/// line break, on an owned runtime, through any value's `runtime()` and by
/// path; it does not take a longer name, a field of another value, what
/// such a field's `runtime()` hands out or a longer method for one.
#[test]
fn the_guard_sees_a_call_on_the_runtime_in_every_form_it_names() {
    let seen = [
        "match runtime.mob_handle() {}",
        "return runtime\n        .mob_handle()\n        .stop();",
        "if runtime . mob_handle ( ) {}",
        "let x = runtime_owner.mob_handle();",
        "entry.runtime().mob_handle();",
        "self.my_runtime.runtime().mob_handle();",
        "mob\n    .runtime()\n    .mob_handle();",
        "members.map(UnifiedRuntime::mob_handle);",
        "crate::UnifiedRuntime::mob_handle(runtime);",
    ];
    for text in seen {
        assert_eq!(calls_on_the_runtime(text, "mob_handle"), 1, "{text}");
    }
    let not_seen = [
        "identity_runtime.mob_handle();",
        "mob_runtime.mob_handle();",
        "ctx.runtime.mob_handle();",
        "ctx.runtime_owner.mob_handle();",
        "ctx.runtime.runtime().mob_handle();",
        "identity_ctx\n    .runtime\n    .runtime()\n    .mob_handle();",
        "runtime.mob_handle_of(x);",
        "runtime.mob_handle;",
        "mob.mob_handle();",
        "UnifiedRuntime::mob_handle_of(runtime);",
        "runtime.loaded_modules().mob_handle();",
    ];
    for text in not_seen {
        assert_eq!(calls_on_the_runtime(text, "mob_handle"), 0, "{text}");
    }
}

/// The name a declaration gives a value whose type ends `head`: `head` is
/// the text before the type's last name, so it ends with the type's prefix
/// (`&`, `&mut`, a lifetime, `Arc<`, `Option<`, a path) and before that
/// with `name:`. None when `head` does not end that way (a path such as
/// `crate::UnifiedRuntime::builder()`, a return type, an `impl`).
fn declared_name(head: &str) -> Option<&str> {
    let mut head = head.trim_end();
    loop {
        let lifetime = head.rfind('\'').filter(|at| {
            let name = &head[at + 1..];
            !name.is_empty() && name.bytes().all(is_name_byte)
        });
        let rest = match lifetime {
            Some(at) => Some(&head[..at]),
            None => [
                "&",
                "mut",
                "Arc<",
                "Option<",
                "Box<",
                "std::sync::",
                "crate::",
                "crate::unified_runtime::",
            ]
            .iter()
            .find_map(|prefix| head.strip_suffix(prefix)),
        };
        match rest {
            Some(rest) => head = rest.trim_end(),
            None => break,
        }
    }
    let head = head.strip_suffix(':')?;
    if head.ends_with(':') {
        return None;
    }
    let head = head.trim_end();
    let start = head
        .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(0, |at| at + 1);
    Some(&head[start..])
}

/// Every runtime a handler declares has a name the scan reads: a parameter
/// or a binding whose type is the runtime (by reference, owned or shared)
/// is named as [`RUNTIME_NAMES`] say.
#[test]
fn every_runtime_a_handler_holds_has_a_name_the_guard_reads() {
    for (head, name) in [
        ("fn a(runtime: &", Some("runtime")),
        ("fn a(\n    rt: Option<std::sync::Arc<", Some("rt")),
        ("fn a(other: &'rt mut crate::", Some("other")),
        ("let owner: Arc<", Some("owner")),
        ("let built = crate::", None),
        ("fn a() -> &'rt ", None),
        ("impl ", None),
    ] {
        assert_eq!(declared_name(head), name, "{head}");
    }
    let mut found = Vec::new();
    let mut declared = 0;
    for (file, source) in HANDLER_FILES {
        let text = production_text(source);
        let bytes = text.as_bytes();
        for (at, _) in text.match_indices("UnifiedRuntime") {
            if after_name(bytes, at, "UnifiedRuntime").is_none() {
                continue;
            }
            let head = &text[..at];
            let Some(name) = declared_name(head) else {
                continue;
            };
            declared += 1;
            if !RUNTIME_NAMES.contains(&name) {
                let line = head.matches('\n').count() + 1;
                found.push(format!("{file}:{line}: `{name}`"));
            }
        }
    }
    assert!(declared > 0, "the row reads the handlers' declarations");
    assert!(
        found.is_empty(),
        "a runtime under a name the guard does not read:\n{}",
        found.join("\n")
    );
}

/// Every production file of the RPC handlers is read by this guard: a
/// module `rpc.rs` declares outside a test gate is in the list.
#[test]
fn the_guard_reads_every_production_file_of_the_rpc_handlers() {
    let code = without_test_modules(&code_only(include_str!("../rpc.rs")));
    let mut missing = Vec::new();
    for line in code.lines() {
        let line = line.trim();
        let Some(rest) = line
            .strip_prefix("pub(crate) mod ")
            .or_else(|| line.strip_prefix("mod "))
        else {
            continue;
        };
        let Some(name) = rest.strip_suffix(';') else {
            continue;
        };
        if name.ends_with("_tests") {
            continue;
        }
        let file = format!("rpc/{name}.rs");
        if !HANDLER_FILES.iter().any(|(listed, _)| *listed == file) {
            missing.push(file);
        }
    }
    assert!(missing.is_empty(), "not read by the guard: {missing:?}");
}

/// The launch facts ([`LAUNCH_FACTS`]) answer the same whatever mob a
/// request names: for no mob and for the default mob's id the resolved
/// mob answers what the runtime answers, and a request that names a mob
/// the host does not have changes neither. The row reads exactly the two
/// names of the group: a third name fails it until the row reads that fact
/// too.
#[tokio::test]
async fn the_two_launch_facts_answer_the_same_for_every_mob_a_request_names() {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"launch-facts-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = crate::UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(std::sync::Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    let launch_facts = |runtime: &crate::UnifiedRuntime| {
        (
            runtime.speaks_for_composition(),
            runtime
                .resolved_storage()
                .map(|storage| storage.status_json()),
        )
    };
    let of_the_launch = launch_facts(&runtime);
    let default_mob = meerkat_mob::MobId::from(runtime.mob_id().as_str());
    for named in [None, Some(&default_mob)] {
        let mob = runtime.resolve_mob(named).await.expect("the default mob");
        assert_eq!(mob.speaks_for_composition(), of_the_launch.0, "{named:?}");
        assert_eq!(
            mob.resolved_storage().map(|storage| storage.status_json()),
            of_the_launch.1,
            "{named:?}"
        );
    }
    let absent = meerkat_mob::MobId::from("launch-facts-absent-mob");
    assert!(runtime.resolve_mob(Some(&absent)).await.is_err());
    assert!(runtime.served_mob_id(Some(&absent)).await.is_err());
    assert_eq!(launch_facts(&runtime), of_the_launch);
    assert_eq!(
        LAUNCH_FACTS,
        ["resolved_storage", "speaks_for_composition"],
        "the facts this row reads"
    );
    for method in LAUNCH_FACTS {
        assert!(
            !DEFAULT_MOB_METHODS.contains(method),
            "{method} is a launch fact, not a method that means the default mob"
        );
    }
    runtime.shutdown().await;
}

/// What the runtime answers for the host's quarantined mobs, as the wire
/// carries it.
async fn host_quarantined_mobs(
    runtime: &crate::UnifiedRuntime,
) -> Result<serde_json::Value, String> {
    runtime
        .quarantined_mobs()
        .await
        .map(|quarantined| crate::mob_handle_runtime::quarantined_mobs_json(&quarantined))
        .map_err(|error| error.to_string())
}

/// The answer of `meta/status` on the stdio dispatch to a request that
/// names `mob_id`, or no mob.
async fn status_naming(runtime: &crate::UnifiedRuntime, mob_id: Option<&str>) -> serde_json::Value {
    let params = match mob_id {
        Some(mob_id) => serde_json::json!({ "mob_id": mob_id }),
        None => serde_json::json!({}),
    };
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/status",
        "params": params,
    })
    .to_string();
    let answer = super::handle_unified_rpc_json(
        runtime,
        &request,
        std::time::Duration::from_secs(1),
        None,
        None,
    )
    .await;
    serde_json::from_str(&answer).expect("the dispatch answers JSON")
}

/// What `meta/status` says of the host's quarantined mobs: the two members
/// that carry them, each null when the answer has none.
fn quarantined_mobs_of(status: &serde_json::Value) -> (serde_json::Value, serde_json::Value) {
    (
        status["result"]["quarantined_mobs"].clone(),
        status["result"]["quarantined_mobs_error"].clone(),
    )
}

/// The address of the blob store a read handed out, if it handed one out:
/// two reads of one store answer one address.
fn blob_store_address(
    store: Option<std::sync::Arc<dyn crate::blob_store::BinaryBlobStore>>,
) -> Option<*const ()> {
    store.map(|store| std::sync::Arc::as_ptr(&store).cast::<()>())
}

/// The host facts ([`HOST_FACTS`]) answer the same whatever mob a request
/// names. `meta/status` reports the host's
/// quarantined mobs the same for a request that names no mob and for one
/// that names the default mob's id; the resolved mob hands out the blob
/// store the runtime hands out, for both; a request that names a mob the
/// host does not have is refused and reads nothing; and what the runtime
/// answers for both facts is the same before and after all three. The host
/// of this row has no quarantined mob, so that equal answer is "none"; it
/// has a blob store. The slot the members' `ask_user` tool reads is the one
/// slot of the process: the runtime hands out the slot the resolved mob's
/// wrapper carries, for both. The row reads exactly the three names of the
/// group: a fourth name fails it until the row reads that fact too.
#[tokio::test]
async fn the_host_facts_are_the_same_for_every_mob_a_request_names() {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"host-facts-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = crate::UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(std::sync::Arc::new(meerkat_client::TestClient::default()))
        .blob_store(std::sync::Arc::new(meerkat_store::MemoryBlobStore::new()))
        .build()
        .await
        .expect("runtime builds");
    let of_the_host = host_quarantined_mobs(&runtime).await;
    assert_eq!(
        of_the_host,
        Ok(serde_json::json!([])),
        "a host with no quarantined mob"
    );
    let store_of_the_host = blob_store_address(runtime.binary_blob_store());
    assert!(store_of_the_host.is_some(), "a host with a blob store");
    let default_mob = runtime.mob_id();

    let unnamed = status_naming(&runtime, None).await;
    let named = status_naming(&runtime, Some(default_mob.as_str())).await;
    assert!(unnamed["result"].is_object(), "{unnamed}");
    assert!(named["result"].is_object(), "{named}");
    assert_eq!(quarantined_mobs_of(&named), quarantined_mobs_of(&unnamed));
    let default_id = meerkat_mob::MobId::from(default_mob.as_str());
    for resolving in [None, Some(&default_id)] {
        let mob = runtime
            .resolve_mob(resolving)
            .await
            .expect("the default mob");
        assert_eq!(
            blob_store_address(mob.binary_blob_store()),
            store_of_the_host,
            "{resolving:?}"
        );
    }

    let absent = status_naming(&runtime, Some("host-facts-absent-mob")).await;
    assert_eq!(
        absent["error"]["data"]["kind"],
        serde_json::json!("foreign_mob_target"),
        "{absent}"
    );
    assert!(absent["result"].is_null(), "{absent}");
    assert_eq!(host_quarantined_mobs(&runtime).await, of_the_host);
    assert_eq!(
        blob_store_address(runtime.binary_blob_store()),
        store_of_the_host
    );
    // The slot the members' `ask_user` tool reads is the process's: the
    // runtime hands out the one every resolved mob's wrapper carries.
    for resolving in [None, Some(&default_id)] {
        let mob = runtime
            .resolve_mob(resolving)
            .await
            .expect("the default mob");
        assert!(
            runtime
                .ask_user_slot()
                .is_the_slot_of(mob.mob_runtime().ask_user_slot()),
            "{resolving:?}"
        );
    }

    assert_eq!(
        HOST_FACTS,
        ["ask_user_slot", "binary_blob_store", "quarantined_mobs"],
        "the facts this row reads"
    );
    for method in HOST_FACTS {
        assert!(
            !DEFAULT_MOB_METHODS.contains(method),
            "{method} is a host fact, not a method that means the default mob"
        );
        assert!(!LAUNCH_FACTS.contains(method), "{method} is listed once");
    }
    runtime.shutdown().await;
}

/// No handler calls a method of the runtime that means the default mob:
/// none of the list is skipped.
#[test]
fn no_handler_reaches_the_default_mob_through_the_runtime() {
    let mut found = Vec::new();
    for (file, source) in HANDLER_FILES {
        let code = production_code(source);
        let text = production_text(source);
        let mut in_file = Vec::new();
        for method in DEFAULT_MOB_METHODS {
            let calls = calls_on_the_runtime(&text, method);
            if calls > 0 {
                in_file.push(format!("{method} x{calls}"));
            }
        }
        // A handler is handed the host's default mob. It neither reads it
        // from a runtime nor states it itself.
        for stated in ["DefaultMob::of_runtime(", "DefaultMob::new("] {
            let count = code.matches(stated).count();
            if count > 0 {
                in_file.push(format!("{} x{count}", stated.trim_end_matches('(')));
            }
        }
        if !in_file.is_empty() {
            found.push(format!("{file}: {}", in_file.join(", ")));
        }
    }
    assert!(
        found.is_empty(),
        "handlers that read the default mob through the runtime:\n{}",
        found.join("\n")
    );
}

/// Below the seam a mob is never optional, and only the resolver makes a
/// resolved mob.
#[test]
fn no_handler_holds_an_optional_mob_or_makes_one() {
    let mut found = Vec::new();
    for (file, source) in HANDLER_FILES.iter().chain(DOOR_FILES) {
        let code = production_code(source);
        for forbidden in [
            "Option<MobEntryRef",
            "Option<&MobEntryRef",
            "Option<ResolvedMob",
            "Option<&ResolvedMob",
            "Option<crate::unified_runtime::MobEntryRef",
            "Option<crate::unified_runtime::ResolvedMob",
            "MobEntryRef{",
            "ResolvedMob{",
        ] {
            let count = code.matches(forbidden).count();
            if count > 0 {
                found.push(format!("{file}: {forbidden} x{count}"));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The stdio methods that are the host's: an arm on this list resolves no
/// mob, and what it answers does not depend on the mob a request names (the
/// id is checked before the arm, which loads nothing). Sorted, one name per
/// line.
///
/// `meta/gating/questions` reads the host's open question sets: the
/// durable decisions they are held in are the host's, and the slot the
/// members' `ask_user` tool reads is the process's (a host fact).
///
/// Four of them are gating methods. The gating state is the host's, one
/// for all its mobs: they read and decide the host's gating state whatever
/// mob of the host the request names. The member an entry's origin names
/// (a Rust host supplies it) is keyed by its mob and its identity. The
/// evaluation is not among them: a request it makes names the mob it was
/// made for ([`STDIO_METHODS_KEYED_BY_THE_SERVED_MOB`]).
pub(super) const HOST_SCOPED_STDIO_METHODS: &[&str] = &[
    "meta/blob/get",
    "meta/call_tool",
    "meta/console/revoke_sessions",
    "meta/cross_mob/directory",
    "meta/gating/audit",
    "meta/gating/decide",
    "meta/gating/pending",
    "meta/gating/questions",
    "meta/gating/withdraw",
    "meta/memory/index",
    "meta/memory/query",
    "meta/memory/stores",
    "meta/models/catalog",
    "meta/peer_pubkey",
    "meta/reconcile",
    "meta/schedule/tools",
    "meta/session_store/bigquery",
    "meta/storage/doctor",
];

/// The families the stdio dispatch serves by a guard instead of a name, by
/// the text of the guard.
const STDIO_FAMILY_GUARDS: &[&str] = &[
    "MOBPACK_AUTHORING_METHODS.contains(&method)",
    "mob_methods::is_member_declaration_method(method)",
    "tool_grant_methods::is_tool_grant_method(method)",
    "identity_methods::is_identity_family_method(method)",
    "realm_mcp_methods::is_realm_mcp_method(method)",
    "is_stdio_live_method(method)",
    "crate::mob_announcements::rpc::is_announcement_method(method)",
    "mob_hosting::is_mob_hosting_method(method)",
    "workgraph_methods::is_workgraph_method(method)",
];

/// The families that are the host's: realm MCP administration, the
/// announcement family, which resolves the mob a method of its own names,
/// and the methods over the mobs of the host, which read the host's mob
/// set and resolve no mob.
const HOST_SCOPED_STDIO_FAMILIES: &[&str] = &[
    "realm_mcp_methods::is_realm_mcp_method(method)",
    "crate::mob_announcements::rpc::is_announcement_method(method)",
    "mob_hosting::is_mob_hosting_method(method)",
];

/// The arm heads of the stdio dispatch that name their method by a
/// constant, each with the method it names.
const STDIO_ARM_CONSTANTS: &[(&str, &str)] = &[
    (
        "CONSOLE_REVOKE_SESSIONS_METHOD",
        crate::rpc::CONSOLE_REVOKE_SESSIONS_METHOD,
    ),
    (
        "schedule_methods::SCHEDULE_TOOLS_METHOD",
        crate::rpc::schedule_methods::SCHEDULE_TOOLS_METHOD,
    ),
    (
        "storage_methods::STORAGE_DOCTOR_METHOD",
        crate::rpc::storage_methods::STORAGE_DOCTOR_METHOD,
    ),
    (
        "topology_methods::TOPOLOGY_APPLY_METHOD",
        crate::rpc::topology_methods::TOPOLOGY_APPLY_METHOD,
    ),
    (
        "topology_methods::TOPOLOGY_AUDIT_METHOD",
        crate::rpc::topology_methods::TOPOLOGY_AUDIT_METHOD,
    ),
    (
        "topology_methods::TOPOLOGY_OPERATION_METHOD",
        crate::rpc::topology_methods::TOPOLOGY_OPERATION_METHOD,
    ),
    (
        "topology_methods::TOPOLOGY_PLAN_METHOD",
        crate::rpc::topology_methods::TOPOLOGY_PLAN_METHOD,
    ),
    (
        "topology_methods::TOPOLOGY_QUERY_METHOD",
        crate::rpc::topology_methods::TOPOLOGY_QUERY_METHOD,
    ),
];

/// The two arms of the stdio dispatch that serve no method of the gateway:
/// a module's own method, and the fall through.
const NOT_A_GATEWAY_METHOD: &[&str] = &["method if is_stdio_module_method(method)", "_"];

/// The name of the list, in `rpc/mob_addressing.rs`, of the stdio methods
/// whose arms act on the launch mob's services without resolving the
/// request's mob. The dispatch refuses such a method for another mob.
const LAUNCH_MOB_ONLY_LIST: &str = "STDIO_METHODS_SERVING_THE_LAUNCH_MOB_ONLY";

/// One arm of the stdio dispatch.
#[derive(Debug)]
struct StdioArm {
    /// The line of the source its head starts on.
    line: usize,
    /// Its head up to `=>`, every run of white space one space.
    head: String,
    /// Whether its code resolves the request's mob: by the mob step
    /// (`request_mob!()`), or by the identity step (`request_identity!()`),
    /// which resolves a mob that is not the launch mob to answer that
    /// mob's own identity context.
    resolves: bool,
    /// Whether its code reads how a record of the host's services names
    /// the mob the request is for (`stored_mob`, the dispatch's value made
    /// from the id it judged, which loads no mob).
    reads_the_stored_mob: bool,
}

/// The dispatch's value an arm keyed by the served mob's id reads.
const STORED_MOB: &str = "stored_mob";

/// The stdio methods whose arms serve the mob a request names by its id
/// alone: the records of the host's routing and delivery services are keyed
/// by the mob, and the arm hands the service the id the dispatch already
/// judged. Such an arm does not resolve the mob: resolving is what loads a
/// mob that is known and not loaded, and a request that needs only an id
/// loads nothing.
///
/// The two methods that select a member's events (`meta/events/subscribe`
/// and `meta/query_events`) are among them: an event of a mob of the host
/// other than the launch mob is kept with that mob and the incarnation it
/// came from, and the arm selects the incarnation of the mob the request
/// names by a question that loads nothing: the one a loaded mob runs at,
/// or the one the mob set's records keep for a mob that is not loaded.
///
/// The gating evaluation is among them too: the gating owner is the
/// host's, and a request it makes names the mob it was made for, which is
/// the id the dispatch judged.
/// Sorted, one name per line.
pub(super) const STDIO_METHODS_KEYED_BY_THE_SERVED_MOB: &[&str] = &[
    "meta/delivery/history",
    "meta/delivery/send",
    "meta/events/subscribe",
    "meta/gating/evaluate",
    "meta/query_events",
    "meta/routing/resolve",
    "meta/routing/routes/add",
    "meta/routing/routes/delete",
    "meta/routing/routes/list",
];

/// The arms of the stdio dispatch in `source`: a head starts on a line at
/// the arm's indent that is no comment and runs to its `=>`, and the arm's
/// code runs to the next head. A comment or a string does not count as
/// code: neither ends a head nor resolves a mob.
fn stdio_arms(source: &str) -> Vec<StdioArm> {
    let code = code_only(source);
    let lines: Vec<(&str, &str)> = source.lines().zip(code.lines()).collect();
    let start = lines
        .iter()
        .position(|(raw, _)| raw.starts_with("async fn handle_unified_rpc_json_inner("))
        .expect("the stdio dispatch");
    let open = (start..lines.len())
        .find(|at| lines[*at].0 == "    let response = match request.method.as_str() {")
        .expect("the dispatch's match");
    let close = (open..lines.len())
        .find(|at| lines[*at].0 == "    };")
        .expect("the end of the dispatch's match");
    let mut arms: Vec<StdioArm> = Vec::new();
    let mut head_open = false;
    for (at, (raw, code)) in lines.iter().enumerate().take(close).skip(open + 1) {
        let text = code.trim();
        if head_open {
            let arm = arms.last_mut().expect("an arm whose head is open");
            arm.head.push(' ');
            arm.head.push_str(raw.trim());
            head_open = !text.contains("=>");
        } else if raw.starts_with("        ")
            && !raw.starts_with("         ")
            && !raw.trim_start().starts_with("//")
            && !matches!(raw.trim(), "" | "{" | "}" | "},")
        {
            head_open = !text.contains("=>");
            arms.push(StdioArm {
                line: at + 1,
                head: raw.trim().to_string(),
                resolves: false,
                reads_the_stored_mob: false,
            });
        }
        if (code.contains("request_mob!()") || code.contains("request_identity!()"))
            && let Some(arm) = arms.last_mut()
        {
            arm.resolves = true;
        }
        if names_the_word(code, STORED_MOB)
            && let Some(arm) = arms.last_mut()
        {
            arm.reads_the_stored_mob = true;
        }
    }
    for arm in &mut arms {
        let head = arm.head.split("=>").next().unwrap_or_default();
        arm.head = head.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    arms
}

/// The arm reader reads a head over several lines, a head by a constant
/// and a family's guard, and takes `request_mob!()` and
/// `request_identity!()` for a resolve only where they are code.
#[test]
fn the_arm_reader_reads_heads_and_resolves_as_it_says() {
    let source = r#"
async fn handle_unified_rpc_json_inner(
) {
    let response = match request.method.as_str() {
        "meta/a" => {
            let mob = request_mob!();
        }
        // "meta/commented" => request_mob!(),
        "meta/b" | "meta/c" => plain(runtime, stored_mob.clone()).await,
        NAMED_METHOD => {
            let text = "request_mob!() request_identity!() stored_mob";
        }
        "meta/d"
        | "meta/e" => {
            let mob = request_mob!();
        }
        method
            if family::is_family_method(method)
                && runtime.family().is_some() =>
        {
            let mob = request_mob!();
        }
        "meta/f" => {
            let identity_ctx = request_identity!();
        }
        _ => fall_through(),
    };
}
"#;
    let arms = stdio_arms(source);
    let keyed: Vec<bool> = arms.iter().map(|arm| arm.reads_the_stored_mob).collect();
    assert_eq!(
        keyed,
        [false, true, false, false, false, false, false],
        "the arms that read the stored mob, where it is code"
    );
    let read: Vec<(usize, String, bool)> = arms
        .into_iter()
        .map(|arm| (arm.line, arm.head, arm.resolves))
        .collect();
    let expected = [
        (5_usize, "\"meta/a\"", true),
        (9, "\"meta/b\" | \"meta/c\"", false),
        (10, "NAMED_METHOD", false),
        (13, "\"meta/d\" | \"meta/e\"", true),
        (
            17,
            "method if family::is_family_method(method) && runtime.family().is_some()",
            true,
        ),
        (23, "\"meta/f\"", true),
        (26, "_", false),
    ]
    .map(|(line, head, resolves)| (line, head.to_string(), resolves));
    assert_eq!(read, expected);
}

/// The names of the list `const <name>: &[&str] = &[ ... ];` in `source`;
/// `None` when the source holds no such list.
fn listed_in_source(source: &str, name: &str) -> Option<Vec<String>> {
    let from = source.find(&format!("const {name}: &[&str] = &["))?;
    let body = &source[from..];
    let body = &body[..body.find("];")?];
    Some(
        body.lines()
            .filter_map(|line| line.trim().strip_prefix('"'))
            .filter_map(|rest| rest.split('"').next())
            .map(str::to_string)
            .collect(),
    )
}

/// Every arm of the stdio dispatch resolves the request's mob, or its
/// method is on exactly one list: the methods that serve the launch mob
/// only, which the dispatch refuses for another mob, the host's methods,
/// or the methods keyed by the served mob's id, whose arms read the id the
/// dispatch judged and load no mob. No method on a list has an arm that
/// resolves, an arm reads that id exactly when its method is keyed by it,
/// and every listed name heads an arm.
///
/// What this row does not see: a method that is answered before the
/// dispatch (the job door, the decision door), and whether an arm on the
/// host's list reads nothing of a mob. An arm whose head it cannot name
/// fails it.
#[test]
fn every_arm_of_the_stdio_dispatch_resolves_its_mob_or_is_on_one_list() {
    let arms = stdio_arms(include_str!("../rpc.rs"));
    assert!(
        arms.len() > 100,
        "the dispatch read here has {} arms",
        arms.len()
    );
    let launch_only = listed_in_source(include_str!("mob_addressing.rs"), LAUNCH_MOB_ONLY_LIST);
    let mut found = Vec::new();
    if launch_only.is_none() {
        found.push(format!(
            "rpc/mob_addressing.rs holds no list {LAUNCH_MOB_ONLY_LIST}"
        ));
    }
    let launch_only = launch_only.unwrap_or_default();
    for (list, names) in [
        (
            "the launch mob's",
            launch_only.iter().map(String::as_str).collect::<Vec<_>>(),
        ),
        ("the host's", HOST_SCOPED_STDIO_METHODS.to_vec()),
        (
            "the served mob's id",
            STDIO_METHODS_KEYED_BY_THE_SERVED_MOB.to_vec(),
        ),
    ] {
        if !names.windows(2).all(|pair| pair[0] < pair[1]) {
            found.push(format!("{list} list is not sorted with each name once"));
        }
    }
    let mut headed = std::collections::BTreeSet::new();
    let mut families = std::collections::BTreeSet::new();
    for arm in &arms {
        let head = arm.head.as_str();
        if NOT_A_GATEWAY_METHOD.contains(&head) {
            if arm.resolves {
                found.push(format!("rpc.rs:{}: `{head}` resolves a mob", arm.line));
            }
            continue;
        }
        if let Some(guard) = STDIO_FAMILY_GUARDS
            .iter()
            .find(|guard| head.contains(**guard))
        {
            families.insert(*guard);
            let the_hosts = HOST_SCOPED_STDIO_FAMILIES.contains(guard);
            if arm.resolves == the_hosts {
                found.push(format!(
                    "rpc.rs:{}: the family `{guard}` {}",
                    arm.line,
                    if the_hosts {
                        "resolves its mob and is listed as the host's"
                    } else {
                        "does not resolve its mob and is not listed as the host's"
                    }
                ));
            }
            continue;
        }
        let mut names: Vec<&str> = head.split('"').skip(1).step_by(2).collect();
        names.extend(
            STDIO_ARM_CONSTANTS
                .iter()
                .filter(|(token, _)| head.split(" | ").any(|part| part.trim() == *token))
                .map(|(_, method)| *method),
        );
        if names.is_empty() {
            found.push(format!(
                "rpc.rs:{}: an arm this row cannot name: `{head}`",
                arm.line
            ));
        }
        for name in names {
            headed.insert(name.to_string());
            let on_launch = launch_only.iter().any(|listed| listed == name);
            let on_host = HOST_SCOPED_STDIO_METHODS.contains(&name);
            let on_keyed = STDIO_METHODS_KEYED_BY_THE_SERVED_MOB.contains(&name);
            let lists = usize::from(on_launch) + usize::from(on_host) + usize::from(on_keyed);
            let what = match (arm.resolves, lists) {
                (true, 0) => continue,
                // An arm keyed by the served mob's id reads that id; no
                // other arm that does not resolve does.
                (false, 1) if on_keyed == arm.reads_the_stored_mob => continue,
                (false, 1) if on_keyed => {
                    "is keyed by the served mob's id and does not read it (`stored_mob`)"
                }
                (false, 1) => {
                    "reads the served mob's id (`stored_mob`) and is not on the list of the methods keyed by it"
                }
                (true, _) => "resolves its mob and is still on a list",
                (false, 0) => "does not resolve its mob and is on no list",
                (false, _) => "is on more than one list",
            };
            found.push(format!("rpc.rs:{}: {name} {what}", arm.line));
        }
    }
    for name in launch_only
        .iter()
        .map(String::as_str)
        .chain(HOST_SCOPED_STDIO_METHODS.iter().copied())
        .chain(STDIO_METHODS_KEYED_BY_THE_SERVED_MOB.iter().copied())
    {
        if !headed.contains(name) {
            found.push(format!(
                "{name}: listed, and no arm of the dispatch is headed by it"
            ));
        }
    }
    for guard in STDIO_FAMILY_GUARDS {
        if !families.contains(guard) {
            found.push(format!(
                "`{guard}`: no arm of the dispatch is served by this guard"
            ));
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Where `name` is called in `text` (production text): each place the name
/// stands as a whole word with an opening parenthesis after it.
fn calls_of(text: &str, name: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut search = 0;
    while let Some(offset) = text[search..].find(name) {
        let at = search + offset;
        let end = at + name.len();
        search = end;
        if (at == 0 || !is_name_byte(bytes[at - 1]))
            && bytes.get(skip_space(bytes, end)) == Some(&b'(')
        {
            found.push(at);
        }
    }
    found
}

/// Every production source file of the crate, by its path under `src`.
fn crate_production_files() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).expect("the crate's sources list") {
            let path = entry.expect("a source entry reads").path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            if path.is_dir() {
                dirs.push(path);
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                let relative = path
                    .strip_prefix(&root)
                    .expect("a source under src")
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push((
                    relative,
                    std::fs::read_to_string(&path).expect("a source reads"),
                ));
            }
        }
    }
    files.sort();
    files
}

/// The two builders of the refusal of a mob id as the stdio dispatch
/// answers it.
const STDIO_FORM_BUILDERS: &[&str] = &["foreign_mob_target", "mob_resolve_refusal"];

/// A handler that the stdio dispatch and a door that has callers both
/// reach answers a mob id it does not serve in the form of the door it
/// answers on. It is handed the door (`MobRefusalDoor`) and builds the
/// stdio form only in the arm of the host's door; the host's door is named
/// as a value by the stdio dispatch alone; and the console's dispatch
/// hands the member declaration family the door that has callers.
#[test]
fn a_shared_handler_refuses_a_mob_id_in_the_form_of_the_door_it_answers_on() {
    let mut found = Vec::new();
    for (file, source) in HANDLER_FILES {
        // The stdio dispatch answers its host; the seam defines the forms.
        if matches!(*file, "rpc.rs" | "rpc/mob_addressing.rs") {
            continue;
        }
        let text = production_text(source);
        for builder in STDIO_FORM_BUILDERS {
            for at in calls_of(&text, builder) {
                let from = text[..at]
                    .char_indices()
                    .rev()
                    .nth(400)
                    .map_or(0, |(index, _)| index);
                let before: String = text[from..at]
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect();
                let before = before
                    .trim_end_matches("mob_addressing::")
                    .trim_end_matches("rpc::")
                    .trim_end_matches("crate::")
                    .trim_end_matches("super::");
                if !before.ends_with("MobRefusalDoor::Host=>") {
                    found.push(format!(
                        "{file}: {builder} is called outside the arm of the host's door"
                    ));
                }
            }
        }
    }
    let mut dispatch_names_the_host = 0;
    let mut console_family_calls = 0;
    for (file, source) in crate_production_files() {
        let code = production_code(&source);
        // The count below reads the host's door by its own name. Under
        // another name it would not be counted, so no file but the seam
        // gives it one.
        if file != "rpc/mob_addressing.rs" {
            for form in other_names_of_the_hosts_door(&production_text(&source)) {
                found.push(format!("{file}: {form}"));
            }
        }
        let named = code.matches("MobRefusalDoor::Host").count();
        let as_an_arm = code.matches("MobRefusalDoor::Host=>").count();
        let as_a_value = named.saturating_sub(as_an_arm);
        if file == "rpc.rs" {
            dispatch_names_the_host = as_a_value;
        } else if as_a_value > 0 {
            found.push(format!(
                "{file}: names the host's door x{as_a_value}; only the stdio dispatch does"
            ));
        }
        if file == "http_console.rs" {
            console_family_calls = code.matches("handle_member_declaration_rpc(").count();
            let hands_the_door = code
                .matches("handle_member_declaration_rpc(crate::rpc::mob_addressing::MobRefusalDoor::Callers,")
                .count();
            if hands_the_door != console_family_calls {
                found.push(format!(
                    "{file}: {console_family_calls} calls of the member declaration family, \
                     {hands_the_door} hand it the door that has callers"
                ));
            }
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
    assert_eq!(
        dispatch_names_the_host, 2,
        "the stdio dispatch hands the host's door to the member declaration family and to \
         the methods over the mobs of the host, once each"
    );
    assert_eq!(
        console_family_calls, 1,
        "the console's dispatch serves the member declaration family at one place"
    );
}

/// The forms in `text` (production text, its white space kept) that give
/// the host's door a name other than `MobRefusalDoor::Host`: the type
/// imported under another name or given one by a type alias, an import of
/// its variants (by name, in a group or all of them), and an import of
/// every item of the addressing seam.
fn other_names_of_the_hosts_door(text: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for statement in text.split(';') {
        let words: Vec<&str> = statement.split_whitespace().collect();
        let joined = words.concat();
        if words.join(" ").contains("MobRefusalDoor as ") {
            found.push("imports MobRefusalDoor under another name");
        }
        if joined.contains("mob_addressing::*") {
            found.push("imports every item of the addressing seam");
        }
        // An import: the word `use`, then its tree to the statement's end.
        let imported = words
            .iter()
            .position(|word| *word == "use")
            .map(|at| words[at + 1..].concat())
            .unwrap_or_default();
        if [
            "MobRefusalDoor::Host",
            "MobRefusalDoor::*",
            "MobRefusalDoor::{",
        ]
        .iter()
        .any(|variant| imported.contains(variant))
        {
            found.push("imports a variant of MobRefusalDoor");
        }
        // A type alias: the word `type`, a name, `=`, and the type's path.
        let aliased = words
            .iter()
            .position(|word| *word == "type")
            .is_some_and(|at| words[at..].concat().contains('='));
        if aliased && joined.ends_with("MobRefusalDoor") {
            found.push("gives MobRefusalDoor another name");
        }
    }
    found
}

/// The guard's reading of the host's door under another name: each form
/// that would let a file name the door without the text the guard counts
/// is seen, and the forms the handlers use are not.
#[test]
fn the_guard_sees_the_hosts_door_under_another_name() {
    for (source, seen) in [
        (
            "use crate::rpc::mob_addressing::MobRefusalDoor as Door;\nfn f() { g(Door::Host); }",
            true,
        ),
        ("use crate::rpc::mob_addressing::*;\n", true),
        (
            "fn f() { use crate::rpc::mob_addressing::MobRefusalDoor::Host; g(Host); }",
            true,
        ),
        (
            "use crate::rpc::mob_addressing::MobRefusalDoor::{Callers, Host};",
            true,
        ),
        (
            "use crate::rpc::mob_addressing::{MobRefusalDoor::*, NamedMob};",
            true,
        ),
        (
            "pub(crate) type Door = crate::rpc::mob_addressing::MobRefusalDoor;",
            true,
        ),
        (
            "use crate::rpc::mob_addressing::MobRefusalDoor;\n\
             fn f(door: MobRefusalDoor) {\n\
                 match door { MobRefusalDoor::Host => a(), MobRefusalDoor::Callers => b() }\n\
             }",
            false,
        ),
        (
            "fn f() { let door = MobRefusalDoor::Callers; g(MobRefusalDoor::Callers, door); }",
            false,
        ),
    ] {
        let forms = other_names_of_the_hosts_door(&production_text(source));
        assert_eq!(!forms.is_empty(), seen, "{source}: {forms:?}");
    }
}

/// The source of the file `path` of this crate (a path under `src`), as it
/// is on disk when the row runs.
fn source_of(path: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(path),
    )
    .expect("a source file of the crate")
}

/// The body of the function `name` in `text` (production text), from its
/// opening brace to the matching one, as a range of `text`.
fn function_body(text: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let bytes = text.as_bytes();
    let mut search = 0;
    while let Some(offset) = text[search..].find(name) {
        let at = search + offset;
        let end = at + name.len();
        search = end;
        let head = text[..at].trim_end();
        let a_definition = head.ends_with("fn")
            && !head.as_bytes()[..head.len() - 2]
                .last()
                .is_some_and(|byte| is_name_byte(*byte))
            && !bytes.get(end).is_some_and(|byte| is_name_byte(*byte));
        if !a_definition {
            continue;
        }
        let mut depth = 0usize;
        let mut open = None;
        for (offset, byte) in bytes[end..].iter().enumerate() {
            match byte {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' if depth == 0 => {
                    open = Some(end + offset);
                    break;
                }
                b';' if depth == 0 => break,
                _ => {}
            }
        }
        let open = open?;
        let mut braces = 0usize;
        for (offset, byte) in bytes[open..].iter().enumerate() {
            match byte {
                b'{' => braces += 1,
                b'}' => {
                    braces -= 1;
                    if braces == 0 {
                        return Some(open..open + offset + 1);
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    None
}

/// What the console's state holds of one mob: the values a request's mob
/// answers for itself.
const PER_MOB_STATE_FIELDS: &[&str] = &[
    "runtime",
    "identity_runtime",
    "mob_events",
    "identity_roster",
    "workgraph",
    "topology",
];

/// One use of a value named `state` in a function's body.
#[derive(Debug, PartialEq, Eq)]
enum StateUse {
    /// `state.<name>`: a field read or a method call.
    Member(String),
    /// `let state =`, or `let (state, <name>) =`: a value is given the name.
    Bound,
    /// The state itself, with the code that stands before it (white space
    /// removed).
    Whole(String),
}

/// Every use of a value named `state` in `text` (production text).
fn uses_of_the_state(text: &str) -> Vec<StateUse> {
    let bytes = text.as_bytes();
    let mut uses = Vec::new();
    let mut search = 0;
    while let Some(offset) = text[search..].find("state") {
        let at = search + offset;
        let end = at + "state".len();
        search = end;
        if (at > 0 && is_name_byte(bytes[at - 1]))
            || bytes.get(end).is_some_and(|byte| is_name_byte(*byte))
        {
            continue;
        }
        let next = skip_space(bytes, end);
        if bytes.get(next) == Some(&b'.') {
            let name_start = skip_space(bytes, next + 1);
            let name_end = (name_start..bytes.len())
                .find(|index| !is_name_byte(bytes[*index]))
                .unwrap_or(bytes.len());
            uses.push(StateUse::Member(text[name_start..name_end].to_string()));
        } else {
            let from = text[..at]
                .char_indices()
                .rev()
                .nth(120)
                .map_or(0, |(index, _)| index);
            let before: String = text[from..at]
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            // `let state =`, or the first name of a pair a taker hands back
            // (`let (state, caller) =`): the state with the caller the route
            // goes on with.
            let before_the_name = text[..at].trim_end();
            let a_binding = (before_the_name.ends_with("let")
                && bytes.get(next) == Some(&b'=')
                && bytes.get(next + 1) != Some(&b'='))
                || (before_the_name
                    .strip_suffix('(')
                    .is_some_and(|before| before.trim_end().ends_with("let"))
                    && bytes.get(next) == Some(&b','));
            uses.push(if a_binding {
                StateUse::Bound
            } else {
                StateUse::Whole(before)
            });
        }
    }
    uses
}

/// The two functions a console route hands its state to, by value, to
/// take its request's mob: `console_route_state` with the mob a query
/// parameter names, and its form for a route whose request names the mob
/// in a member of its body.
const STATE_TAKERS: &[&str] = &["console_route_state", "console_route_state_for_member"];

/// The two functions that tell a console route who its caller is. A route
/// hands them the state before it takes its request's mob.
const CALLER_CHECKS: &[&str] = &[
    "console_request_auth_context(&",
    "console_write_auth_context(&",
];

/// How a route of the console's router comes to the mob its request is
/// served for.
#[derive(Clone, Copy)]
enum RouteMob {
    /// The route serves one mob. The functions from the one the router
    /// mounts down to the one that takes the request's mob
    /// (`console_route_state`), each by file and name.
    Serves(&'static [(&'static str, &'static str)]),
    /// The route reads no value of a mob: it checks the id a request names
    /// and resolves none. The function that does, in `http_console.rs`.
    Checks(&'static str),
    /// The console's request doors: they take the mob with the request's
    /// method (`take_console_request_mob`), under rows of their own.
    RequestDoor,
    /// The route serves the host's blob store, the same for every mob.
    HostFact,
}

const CONSOLE: &str = "http_console.rs";
const APPS: &str = "http_console/mcp_apps.rs";

/// The fourth column of [`CONSOLE_ROUTES`]: whether what a route of one mob
/// reads after it takes its mob is that mob's. `Some(false)`: not yet, the
/// route still reads values keyed by an identity alone or captured when its
/// router was built, so it serves the launch mob only. `Some(true)`: its
/// reads are per mob, and it serves every mob its caller may address.
/// `None`: not a route of one mob.
type PerMobReads = Option<bool>;

/// The route's reads are not per mob yet.
const NOT_YET: PerMobReads = Some(false);

/// The route's reads are per mob: it serves every mob its caller may
/// address.
const READS_PER_MOB: PerMobReads = Some(true);

/// Not a route of one mob: the request doors take their mob per method,
/// and the blob route is the host's.
const NOT_ONE_MOBS_ROUTE: PerMobReads = None;

/// Every route of the console's router: its path, the handler the router
/// mounts, how it comes to its request's mob, and whether its reads are per
/// mob.
const CONSOLE_ROUTES: &[(&str, &str, RouteMob, PerMobReads)] = &[
    (
        "/console/gating/stream",
        "console_gating_stream_handler",
        RouteMob::Serves(&[(CONSOLE, "console_gating_stream_handler")]),
        NOT_YET,
    ),
    (
        "/console/experience",
        "console_json_route",
        RouteMob::Serves(&[
            (CONSOLE, "console_json_route"),
            (CONSOLE, "console_json_answer"),
        ]),
        NOT_YET,
    ),
    (
        "/console/modules",
        "console_json_route",
        RouteMob::Serves(&[
            (CONSOLE, "console_json_route"),
            (CONSOLE, "console_json_answer"),
        ]),
        NOT_YET,
    ),
    (
        "/console/identities",
        "console_identities_handler",
        RouteMob::Checks("console_identities_handler"),
        NOT_YET,
    ),
    (
        "/console/timeline",
        "console_timeline_handler",
        RouteMob::Serves(&[(CONSOLE, "console_timeline_handler")]),
        NOT_YET,
    ),
    (
        "/console/mcp-apps/resolve",
        "mcp_apps::resolve",
        RouteMob::Serves(&[
            (APPS, "resolve"),
            (APPS, "handle"),
            (APPS, "handle_noting_dispatch"),
        ]),
        NOT_YET,
    ),
    (
        "/console/mcp-apps/read-resource",
        "mcp_apps::read_resource",
        RouteMob::Serves(&[
            (APPS, "read_resource"),
            (APPS, "handle"),
            (APPS, "handle_noting_dispatch"),
        ]),
        NOT_YET,
    ),
    (
        "/console/mcp-apps/call-tool",
        "mcp_apps::call_tool",
        RouteMob::Serves(&[(APPS, "call_tool"), (APPS, "handle_noting_dispatch")]),
        NOT_YET,
    ),
    (
        "/console/timeline/stream",
        "console_timeline_stream_handler",
        RouteMob::Serves(&[(CONSOLE, "console_timeline_stream_handler")]),
        NOT_YET,
    ),
    (
        "/console/identity/{identity}/stream",
        "console_identity_timeline_stream_handler",
        RouteMob::Serves(&[
            (CONSOLE, "console_identity_timeline_stream_handler"),
            (CONSOLE, "console_timeline_stream_handler"),
        ]),
        NOT_YET,
    ),
    (
        "/console/send",
        "console_send_handler",
        RouteMob::Serves(&[(CONSOLE, "console_send_handler")]),
        READS_PER_MOB,
    ),
    (
        "/console/send/recover",
        "console_send_recover_handler",
        RouteMob::Serves(&[(CONSOLE, "console_send_recover_handler")]),
        READS_PER_MOB,
    ),
    (
        "/console/rpc",
        "console_rpc_route",
        RouteMob::RequestDoor,
        NOT_ONE_MOBS_ROUTE,
    ),
    (
        "/console/rpc/multipart",
        "console_rpc_multipart_route",
        RouteMob::RequestDoor,
        NOT_ONE_MOBS_ROUTE,
    ),
    (
        "/blobs/{blob_id}",
        "blob_get_handler",
        RouteMob::HostFact,
        NOT_ONE_MOBS_ROUTE,
    ),
];

/// The routes the console's router mounts, read from its source: each path
/// with the handler mounted on it.
fn mounted_console_routes() -> Vec<(String, String)> {
    let source = source_of(CONSOLE);
    let text = production_text(&source);
    let body = function_body(&text, "console_json_router_with_state")
        .expect("the console's router is built by console_json_router_with_state");
    // The same range of the source as it is written: the paths are string
    // literals, which the production text blanks.
    let written: String = source[body]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    written
        .split(".route(\"")
        .skip(1)
        .map(|route| {
            let (path, rest) = route.split_once("\",").expect("a route's path");
            let handler = rest
                .strip_prefix("get(")
                .or_else(|| rest.strip_prefix("post("))
                .expect("a route is a GET or a POST");
            let handler = handler.split(')').next().unwrap_or_default();
            (path.to_string(), handler.to_string())
        })
        .collect()
}

/// The production text of the console route file `file`.
fn console_route_text(file: &str) -> String {
    production_text(&source_of(file))
}

/// The calls that mount something on a router under a path.
const ROUTER_MOUNTS: &[&str] = &[
    "route",
    "route_service",
    "nest",
    "nest_service",
    "fallback",
    "fallback_service",
];

/// How often `text` (production text) calls the method `name` on a value
/// (`.name(`): a function of that name is no mount.
fn method_calls_of(text: &str, name: &str) -> usize {
    calls_of(text, name)
        .into_iter()
        .filter(|at| text[..*at].ends_with('.'))
        .count()
}

/// How often `text` (production text) mounts something on a router.
fn router_mounts(text: &str) -> usize {
    ROUTER_MOUNTS
        .iter()
        .map(|mount| method_calls_of(text, mount))
        .sum()
}

/// Every production file of the crate that mounts something on a router,
/// with the number of its mounts. The console's are the routes of the
/// table and its frontend's static routes
/// (`no_route_is_mounted_beside_the_console_route_table`). The event
/// streams, the flow editor and the gateway's own routers mount the rest:
/// a route added to any of them changes its count here, and is given the
/// mob it serves in the same change.
const FILES_THAT_MOUNT_ROUTES: &[(&str, usize)] = &[
    ("http_console.rs", 21),
    ("http_flow_editor.rs", 9),
    ("http_sse.rs", 4),
    ("rpc_gateway.rs", 1),
    ("unified_runtime/http.rs", 1),
];

/// No route reaches the console beside its route table. The console's
/// router mounts routes only: it merges no other router, nests none, and
/// mounts no service and no fallback, so every path it serves is a route
/// `every_console_route_takes_its_mob_before_it_reads_one` reads. In the
/// console's files every other mount is the frontend's, whose router holds
/// no state. And the production files of the crate that mount anything on
/// a router are the listed ones, each with its count.
#[test]
fn no_route_is_mounted_beside_the_console_route_table() {
    let mut found = Vec::new();
    let text = console_route_text(CONSOLE);
    let router = &text[function_body(&text, "console_json_router_with_state")
        .expect("the console's router is built by console_json_router_with_state")];
    let frontend = &text[function_body(&text, "console_frontend_router")
        .expect("the console's frontend is mounted by console_frontend_router")];
    for mount in ROUTER_MOUNTS.iter().skip(1).chain(&["merge"]) {
        let times = method_calls_of(router, mount);
        if times > 0 {
            found.push(format!(
                "the console's router calls {mount} x{times}: what it mounts is no route of \
                 the table"
            ));
        }
    }
    if frontend.contains("state") {
        found.push("the console's frontend router names a state".to_string());
    }
    let elsewhere = router_mounts(&text) - router_mounts(router) - router_mounts(frontend);
    if elsewhere > 0 {
        found.push(format!(
            "{CONSOLE} mounts {elsewhere} routes outside the console's router and its frontend's"
        ));
    }
    let mounting: Vec<(String, usize)> = crate_production_files()
        .into_iter()
        .filter_map(|(file, source)| {
            let mounts = router_mounts(&production_text(&source));
            (mounts > 0).then_some((file, mounts))
        })
        .collect();
    let listed: Vec<(String, usize)> = FILES_THAT_MOUNT_ROUTES
        .iter()
        .map(|(file, mounts)| ((*file).to_string(), *mounts))
        .collect();
    if mounting != listed {
        found.push(format!(
            "the production files that mount routes are {mounting:?}; the list names {listed:?}"
        ));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// Every route of the console's router takes the mob its request is served
/// for before it reads a value of a mob.
///
/// A route that serves one mob hands its state to `console_route_state`,
/// by value, once: what it reads of a mob afterwards is the resolved mob's.
/// Before that it reads no per-mob value of the state and hands the whole
/// state only to the check of its caller. A route that reads no value of a
/// mob checks the id and never reads one. The table lists every route the
/// router mounts, so a route added later is in it or fails this row.
#[test]
fn every_console_route_takes_its_mob_before_it_reads_one() {
    let mut found = Vec::new();
    let mounted = mounted_console_routes();
    let listed: Vec<(String, String)> = CONSOLE_ROUTES
        .iter()
        .map(|(path, handler, _, _)| (path.to_string(), handler.to_string()))
        .collect();
    for route in &mounted {
        if !listed.contains(route) {
            found.push(format!(
                "{} -> {}: mounted, not in the table",
                route.0, route.1
            ));
        }
    }
    for route in &listed {
        if !mounted.contains(route) {
            found.push(format!(
                "{} -> {}: in the table, not mounted",
                route.0, route.1
            ));
        }
    }
    let per_mob_reads = |body: &str| -> Vec<String> {
        uses_of_the_state(body)
            .into_iter()
            .filter_map(|state_use| match state_use {
                StateUse::Member(name) if PER_MOB_STATE_FIELDS.contains(&name.as_str()) => {
                    Some(format!("state.{name}"))
                }
                _ => None,
            })
            .collect()
    };
    for (path, _, mob, _) in CONSOLE_ROUTES {
        match mob {
            RouteMob::Serves(chain) => {
                for (index, (file, name)) in chain.iter().enumerate() {
                    let text = console_route_text(file);
                    let Some(range) = function_body(&text, name) else {
                        found.push(format!("{path}: {file} has no function {name}"));
                        continue;
                    };
                    let body = &text[range];
                    if let Some((_, next)) = chain.get(index + 1) {
                        // A function on the way: it hands the state on.
                        if calls_of(body, next).is_empty() {
                            found.push(format!("{path}: {name} does not call {next}"));
                        }
                        for read in per_mob_reads(body) {
                            found.push(format!("{path}: {name} reads {read}"));
                        }
                        continue;
                    }
                    let takes: Vec<(usize, &str)> = STATE_TAKERS
                        .iter()
                        .flat_map(|taker| {
                            calls_of(body, taker)
                                .into_iter()
                                .map(move |at| (at, *taker))
                        })
                        .collect();
                    let [(take, taker)] = takes.as_slice() else {
                        found.push(format!(
                            "{path}: {name} calls console_route_state x{}",
                            takes.len()
                        ));
                        continue;
                    };
                    let argument: String = body[take + taker.len()..]
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .take("(state,".len())
                        .collect();
                    if argument != "(state," {
                        found.push(format!(
                            "{path}: {name} does not hand its state to console_route_state"
                        ));
                    }
                    let before_the_take = uses_of_the_state(&body[..*take]);
                    let last = before_the_take.len().saturating_sub(1);
                    for (at, state_use) in before_the_take.into_iter().enumerate() {
                        match state_use {
                            // The name the served state is given.
                            StateUse::Bound if at == last => {}
                            StateUse::Bound => {
                                found.push(format!(
                                    "{path}: {name} names another value `state` before it takes \
                                     the request's mob"
                                ));
                            }
                            StateUse::Member(member)
                                if PER_MOB_STATE_FIELDS.contains(&member.as_str())
                                    || member == "clone" =>
                            {
                                found.push(format!(
                                    "{path}: {name} reads state.{member} before it takes the \
                                     request's mob"
                                ));
                            }
                            StateUse::Member(_) => {}
                            StateUse::Whole(before) => {
                                if !CALLER_CHECKS.iter().any(|check| before.ends_with(check)) {
                                    found.push(format!(
                                        "{path}: {name} hands its state on before it takes the \
                                         request's mob"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
            RouteMob::Checks(name) => {
                let text = console_route_text(CONSOLE);
                let Some(range) = function_body(&text, name) else {
                    found.push(format!("{path}: no function {name}"));
                    continue;
                };
                let body = &text[range];
                let checks = calls_of(body, "refuse_console_route_of_another_mob").len();
                if checks != 1 {
                    found.push(format!(
                        "{path}: {name} calls refuse_console_route_of_another_mob x{checks}"
                    ));
                }
                for read in per_mob_reads(body) {
                    found.push(format!("{path}: {name} reads {read}"));
                }
            }
            RouteMob::RequestDoor | RouteMob::HostFact => {}
        }
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The guard's reading of a function: it finds a definition by its name,
/// tells a read of a per-mob value from another member of the state, and
/// keeps what stands before the state where it is handed on whole.
#[test]
fn the_route_guard_reads_a_handlers_uses_of_its_state() {
    let text = production_text(
        "fn other_handler(state: S) { state.runtime(); }\n\
         async fn handler(State(state): State<S>, uri: Uri) -> impl IntoResponse {\n\
             let Some(auth) = console_request_auth_context(&state, &headers, &uri) else { return; };\n\
             let decisions = &state.decisions;\n\
             let (state, auth) = match console_route_state(state, resolver, &named, auth).await { Ok(s) => s };\n\
             state.runtime.clone();\n\
             module_state.topology;\n\
         }\n",
    );
    let body = &text[function_body(&text, "handler").expect("the function is found")];
    assert!(body.starts_with('{') && body.ends_with('}'));
    assert!(body.contains("module_state") && !body.contains("other_handler"));
    let [take] = calls_of(body, "console_route_state")[..] else {
        panic!("one call of console_route_state");
    };
    let before = uses_of_the_state(&body[..take]);
    assert!(
        matches!(
            before.as_slice(),
            [StateUse::Whole(check), StateUse::Member(member), StateUse::Bound]
                if check.ends_with("console_request_auth_context(&") && member == "decisions"
        ),
        "{before:?}"
    );
    let after = uses_of_the_state(&body[take..]);
    assert!(
        matches!(
            after.as_slice(),
            [StateUse::Whole(taken), StateUse::Member(member)]
                if taken.ends_with("console_route_state(") && member == "runtime"
        ),
        "{after:?}"
    );
}

/// Whether `line` (code only) names the identifier `word`.
fn names_the_word(line: &str, word: &str) -> bool {
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].bytes().next_back();
        let after = line[at + word.len()..].bytes().next();
        !before.is_some_and(is_name_byte) && !after.is_some_and(is_name_byte)
    })
}

/// The launch composed one identity context, and it is the launch mob's.
/// The stdio dispatch takes it under the name `launch_identity_ctx` and
/// names it in its parameter and in its two seam steps only
/// (`request_identity!()`, `request_identity_if_any!()`): each step hands
/// it to an arm for the launch mob, and the first also asks whether the
/// launch composed one at all before it serves a mob whose entry holds
/// none. An arm that works with an identity context binds `identity_ctx`
/// from one of the two steps before it reads it. So no arm hands a handler
/// the launch's identity runtime for a request that is for another mob.
#[test]
fn no_arm_of_the_stdio_dispatch_reads_the_identity_context_outside_the_seam_step() {
    let source = include_str!("../rpc.rs");
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with("async fn handle_unified_rpc_json_inner("))
        .expect("the stdio dispatch");
    let end = (start..lines.len())
        .find(|at| lines[*at] == "}")
        .expect("the end of the stdio dispatch");
    let mut found = Vec::new();
    let named: Vec<usize> = (start..end)
        .filter(|at| names_the_word(lines[*at], "launch_identity_ctx"))
        .collect();
    if named.len() != 4 {
        found.push(format!(
            "the dispatch names launch_identity_ctx on {} lines (its parameter, the launch mob's \
             line of its two seam steps and the first step's question whether the launch \
             composed one are four): {:?}",
            named.len(),
            named.iter().map(|at| at + 1).collect::<Vec<_>>()
        ));
    }
    let arms = stdio_arms(source);
    let match_end = (arms.last().map_or(start, |arm| arm.line)..lines.len())
        .find(|at| lines[*at] == "    };")
        .expect("the end of the dispatch's match");
    let mut bound = 0;
    for (index, arm) in arms.iter().enumerate() {
        let first = arm.line - 1;
        let last = arms.get(index + 1).map_or(match_end, |next| next.line - 1);
        let Some(first_read) = (first..last).find(|at| names_the_word(lines[*at], "identity_ctx"))
        else {
            continue;
        };
        let read = lines[first_read].trim();
        if read == "let identity_ctx = request_identity!();"
            || read == "let identity_ctx = request_identity_if_any!();"
        {
            bound += 1;
        } else {
            found.push(format!(
                "rpc.rs:{}: `{}` reads identity_ctx at line {} without taking it from a seam step",
                arm.line,
                arm.head,
                first_read + 1
            ));
        }
    }
    assert!(
        bound > 50 || !found.is_empty(),
        "the arms that work with an identity context are read here: {bound}"
    );
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The methods whose arms take their identity context from the seam step
/// that answers the context of the mob a request names
/// (`request_identity!()`) and are not refused before their arm (the
/// methods that serve the launch mob only) are exactly the methods the
/// behaviour rows ask for a second served mob, and four families take it
/// the same way. The methods whose arms take it from the step that refuses
/// nothing (`request_identity_if_any!()`: for a mob that is not the launch
/// mob the context its own entry holds, or none) are exactly the listed
/// ones: no arm moves to that step without the list saying so.
#[test]
fn the_methods_working_with_an_identity_context_take_it_from_the_seam_step() {
    let source = include_str!("../rpc.rs");
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let arms = stdio_arms(source);
    let match_end = (arms.last().map_or(0, |arm| arm.line)..lines.len())
        .find(|at| lines[*at] == "    };")
        .expect("the end of the dispatch's match");
    let launch_only = listed_in_source(include_str!("mob_addressing.rs"), LAUNCH_MOB_ONLY_LIST)
        .unwrap_or_default();
    let mut taking = std::collections::BTreeSet::new();
    let mut never_refused = std::collections::BTreeSet::new();
    let mut families = Vec::new();
    for (index, arm) in arms.iter().enumerate() {
        let first = arm.line - 1;
        let last = arms.get(index + 1).map_or(match_end, |next| next.line - 1);
        // The step that refuses nothing: the launch's context for the
        // launch mob, the entry's own or none for any other.
        if (first..last).any(|at| lines[at].contains("request_identity_if_any!()")) {
            never_refused.extend(arm.head.split('"').skip(1).step_by(2).map(str::to_string));
        }
        if !(first..last).any(|at| lines[at].contains("request_identity!()")) {
            continue;
        }
        if let Some(guard) = STDIO_FAMILY_GUARDS
            .iter()
            .find(|guard| arm.head.contains(**guard))
        {
            families.push(*guard);
            continue;
        }
        taking.extend(
            arm.head
                .split('"')
                .skip(1)
                .step_by(2)
                .filter(|name| !launch_only.iter().any(|listed| listed == name))
                .map(str::to_string),
        );
    }
    let listed: std::collections::BTreeSet<String> =
        super::mob_addressing_tests::STDIO_METHODS_WORKING_WITH_AN_IDENTITY_CONTEXT
            .iter()
            .map(|name| (*name).to_string())
            .collect();
    let mut found = Vec::new();
    for name in listed.difference(&taking) {
        found.push(format!(
            "{name}: listed, and its arm does not take the identity context from the seam step"
        ));
    }
    for name in taking.difference(&listed) {
        found.push(format!(
            "{name}: its arm takes the identity context from the seam step, and it is not listed"
        ));
    }
    let listed_as_never_refused: std::collections::BTreeSet<String> =
        super::mob_addressing_tests::STDIO_METHODS_NEVER_REFUSED_FOR_AN_IDENTITY_CONTEXT
            .iter()
            .map(|name| (*name).to_string())
            .collect();
    for name in listed_as_never_refused.difference(&never_refused) {
        found.push(format!(
            "{name}: listed as never refused for want of an identity context, and its arm does \
             not take it from that seam step"
        ));
    }
    for name in never_refused.difference(&listed_as_never_refused) {
        found.push(format!(
            "{name}: its arm takes the identity context from the seam step that refuses nothing, \
             and it is not listed"
        ));
    }
    let expected_families = [
        "mob_methods::is_member_declaration_method(method)",
        "tool_grant_methods::is_tool_grant_method(method)",
        "identity_methods::is_identity_family_method(method)",
        "is_stdio_live_method(method)",
    ];
    if families != expected_families {
        found.push(format!(
            "the families that take the identity context from the seam step: {families:?}"
        ));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The text between the parentheses of the call that starts at `at` in
/// `text` (production text): the call's arguments.
fn call_arguments(text: &str, at: usize) -> &str {
    let bytes = text.as_bytes();
    let open = (at..bytes.len())
        .find(|index| bytes[*index] == b'(')
        .expect("a call has an opening parenthesis");
    let mut depth = 0_usize;
    for (index, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..index];
                }
            }
            _ => {}
        }
    }
    panic!("a call's parentheses close");
}

/// The form in which a route takes its mob for the launch mob only: it
/// hands its taker the caller as one who addresses the launch mob alone.
const FOR_THE_LAUNCH_MOB_ONLY: &str = "LaunchMobOnly(";

/// The paths a list constant of the console's behaviour rows names, read
/// from that file's source: the string literals between the constant's name
/// and the end of its value.
fn routes_the_behaviour_row_lists() -> Vec<String> {
    let source = source_of("console_request_mob_tests.rs");
    let from = source
        .find("const ROUTES_THAT_READ_PER_MOB")
        .expect("the behaviour row's list");
    let value = &source[from..];
    let value = &value[..value.find("];").expect("the list ends")];
    value
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// A console route serves a mob other than the launch mob only once what
/// it reads after the take is that mob's. Until then (the table's fourth
/// column) it takes its mob for the launch mob only: it hands its taker,
/// or its check, the caller as one who addresses the launch mob alone, so
/// the request of any other mob is refused before anything is asked about
/// that mob. A route whose reads are per mob hands its caller as it is. The
/// behaviour row's list names exactly the routes whose column says so.
#[test]
fn a_console_route_whose_reads_are_not_per_mob_takes_its_mob_for_the_launch_mob_only() {
    let mut found = Vec::new();
    let mut read_per_mob = Vec::new();
    for (path, _, mob, reads_per_mob) in CONSOLE_ROUTES {
        let Some(reads_per_mob) = reads_per_mob else {
            if matches!(mob, RouteMob::Serves(_) | RouteMob::Checks(_)) {
                found.push(format!("{path}: a route of one mob has no fourth column"));
            }
            continue;
        };
        if *reads_per_mob {
            read_per_mob.push((*path).to_string());
        }
        let (file, name, takers): (&str, &str, &[&str]) = match mob {
            RouteMob::Serves(chain) => {
                let (file, name) = chain.last().expect("a chain names a function");
                (*file, *name, STATE_TAKERS)
            }
            RouteMob::Checks(name) => (CONSOLE, *name, &["refuse_console_route_of_another_mob"]),
            RouteMob::RequestDoor | RouteMob::HostFact => {
                found.push(format!(
                    "{path}: not a route of one mob, with a fourth column"
                ));
                continue;
            }
        };
        let text = console_route_text(file);
        let Some(range) = function_body(&text, name) else {
            found.push(format!("{path}: {file} has no function {name}"));
            continue;
        };
        let body = &text[range];
        let takes: Vec<usize> = takers
            .iter()
            .flat_map(|taker| calls_of(body, taker))
            .collect();
        let [take] = takes.as_slice() else {
            found.push(format!("{path}: {name} takes its mob x{}", takes.len()));
            continue;
        };
        let arguments: String = call_arguments(body, *take)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let for_the_launch_mob_only = arguments.contains(FOR_THE_LAUNCH_MOB_ONLY);
        if !*reads_per_mob && !for_the_launch_mob_only {
            found.push(format!(
                "{path}: its reads are not per mob yet, and {name} does not take its mob for \
                 the launch mob only"
            ));
        }
        if *reads_per_mob && for_the_launch_mob_only {
            found.push(format!(
                "{path}: its reads are per mob, and {name} takes its mob for the launch mob only"
            ));
        }
    }
    read_per_mob.sort();
    let mut listed = routes_the_behaviour_row_lists();
    listed.sort();
    if listed != read_per_mob {
        found.push(format!(
            "the behaviour row lists {listed:?} as reading per mob, the table {read_per_mob:?}"
        ));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The fields of the struct literal that opens on line `open` of `lines`,
/// each with the binding its value names: a shorthand field names itself,
/// and a value names the last name in it that is not `Some`, `Arc` or
/// `clone`. The literal ends at the first later line indented like `open`.
fn literal_bindings(lines: &[&str], open: usize) -> std::collections::BTreeMap<String, String> {
    let indent = lines[open].len() - lines[open].trim_start().len();
    let mut bindings = std::collections::BTreeMap::new();
    for line in &lines[open + 1..] {
        if line.len() - line.trim_start().len() <= indent && !line.trim().is_empty() {
            break;
        }
        // A field of the literal itself: one level in, `name` or
        // `name: value`.
        if line.len() - line.trim_start().len() != indent + 4 {
            continue;
        }
        let field = line.trim().trim_end_matches(',');
        let (name, value) = field.split_once(": ").unwrap_or((field, field));
        if name.is_empty() || !name.bytes().all(is_name_byte) {
            continue;
        }
        let binding = value
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .rfind(|word| !word.is_empty() && !matches!(*word, "Some" | "Arc" | "clone"))
            .unwrap_or_default();
        bindings.insert(name.to_string(), binding.to_string());
    }
    bindings
}

/// What is wrong with how the launch in `source` (the gateway's source text)
/// gives the launch mob's entry the three services a request needs beside
/// the mob's identity context: nothing, when it gives them once, before it
/// builds the context it hands the stdio dispatch, and by the bindings that
/// context carries.
fn launch_request_services_findings(source: &str) -> Vec<String> {
    const SERVICES: [&str; 3] = [
        "agent_memory_provider",
        "compaction_floors",
        "transcript_edit_service",
    ];
    let code = code_only(source);
    let lines: Vec<&str> = code.lines().collect();
    let opening = |text: &str| -> Vec<usize> {
        (0..lines.len())
            .filter(|at| lines[*at].contains(text))
            .collect()
    };
    let given = opening("set_identity_request_services(");
    let handed = opening("crate::rpc::IdentityFirstContext {");
    let (&[given], &[handed]) = (given.as_slice(), handed.as_slice()) else {
        return vec![format!(
            "the launch gives its entry the request services once, and builds one dispatch \
             context: lines {given:?} and {handed:?}"
        )];
    };
    let mut found = Vec::new();
    if given > handed {
        found.push("the dispatch context is built before the entry holds the services".to_string());
    }
    let of_the_entry = literal_bindings(&lines, given);
    let of_the_context = literal_bindings(&lines, handed);
    for service in SERVICES {
        match (of_the_entry.get(service), of_the_context.get(service)) {
            (Some(entry), Some(context)) if entry == context && !entry.is_empty() => {}
            (entry, context) => found.push(format!(
                "{service}: the entry is given {entry:?}, the dispatch context carries {context:?}"
            )),
        }
    }
    if of_the_entry.len() != SERVICES.len() {
        found.push(format!(
            "the entry is given other members than the three services: {:?}",
            of_the_entry.keys().collect::<Vec<_>>()
        ));
    }
    found
}

/// The launch gives the launch mob's entry the three services a request
/// needs beside the mob's identity context, once, and they are the values
/// the context it hands the stdio dispatch carries: the same three
/// bindings, by name. A host that serves a mob through its entry (the
/// dispatch does, for every mob that is not the launch mob) reads the
/// services the launch composed and no others. A launch that stops giving
/// them, or gives another value, fails here.
#[test]
fn the_launch_gives_the_launch_mobs_entry_the_request_services_its_dispatch_context_carries() {
    let found = launch_request_services_findings(include_str!("../rpc_gateway.rs"));
    assert!(found.is_empty(), "{}", found.join("\n"));
}

/// The row above reads the launch as it says: the gateway's text with the
/// call taken out, with one service given from another binding, and with
/// one service given as nothing, each fails it.
#[test]
fn the_launch_row_fails_a_launch_that_gives_its_entry_no_request_services_or_other_ones() {
    let source = include_str!("../rpc_gateway.rs");
    let call = source
        .find("runtime.set_identity_request_services(")
        .expect("the launch's call");
    let end = call + source[call..].find("});").expect("the end of the call") + "});".len();
    let without_the_call = format!("{}{}", &source[..call], &source[end..]);
    let from_another_binding = source.replacen(
        "compaction_floors: Some(Arc::clone(&compaction_floors)),",
        "compaction_floors: Some(Arc::clone(&floors_of_another_mob)),",
        1,
    );
    let given_as_nothing = source.replacen(
        "agent_memory_provider: agent_memory_provider.clone(),",
        "agent_memory_provider: None,",
        1,
    );
    for (what, text) in [
        ("without the call", &without_the_call),
        ("a service from another binding", &from_another_binding),
        ("a service given as nothing", &given_as_nothing),
    ] {
        assert_ne!(text.as_str(), source, "{what}: the text was changed");
        assert!(
            !launch_request_services_findings(text).is_empty(),
            "{what}: the row passes a launch it must fail"
        );
    }
}

/// The names by which a file of the stdio dispatch could reach the host's
/// console events: the runtime's accessor of its store and its three
/// methods that write it. The store they reach is the launch mob's: every
/// key in it is an identity name, and an identity is a name inside one mob.
const HOST_CONSOLE_EVENTS: &[&str] = &[
    "console_events",
    "record_console_lifecycle",
    "reserve_caller_identity_interaction",
    "reserve_identity_interaction",
];

/// The dispatch's one step that answers the console events of the mob a
/// request is for.
const CONSOLE_EVENTS_STEP: &str = "console_events_of_a_served_mob";

/// The console events an arm of the stdio dispatch, or a handler below it,
/// reads or writes are those of the mob the request names: each takes them
/// from the dispatch's one step, which answers the host's store for the
/// launch mob and, for any other mob, that mob's own store. No file of the
/// dispatch names the host's console events anywhere else, so no request
/// for one mob's identity reaches the pending interactions, the run
/// tracking or the events of another mob's identity of the same name.
#[test]
fn the_stdio_dispatch_takes_a_mobs_console_events_from_one_step() {
    let mut found = Vec::new();
    let mut taken = 0;
    for (file, source) in HANDLER_FILES {
        let text = production_text(source);
        let step = if *file == "rpc.rs" {
            let step = function_body(&text, CONSOLE_EVENTS_STEP);
            if step.is_none() {
                found.push(format!("rpc.rs holds no function {CONSOLE_EVENTS_STEP}"));
            }
            step
        } else {
            None
        };
        for name in HOST_CONSOLE_EVENTS {
            let outside = calls_of(&text, name)
                .into_iter()
                .filter(|at| !step.as_ref().is_some_and(|body| body.contains(at)))
                .count();
            if outside > 0 {
                found.push(format!(
                    "{file}: {name} x{outside} outside {CONSOLE_EVENTS_STEP}"
                ));
            }
        }
        // The step's own definition is no use of it.
        taken += calls_of(&text, CONSOLE_EVENTS_STEP)
            .len()
            .saturating_sub(usize::from(step.is_some()));
    }
    assert!(found.is_empty(), "{}", found.join("\n"));
    assert!(
        taken >= 6,
        "the places that take a mob's console events from the step (six methods): {taken}"
    );
}

/// The live family's test as the stdio dispatch's arm states it, and as the
/// addressing seam states it for the rule that refuses the family for a mob
/// that is not the launch mob (`stdio_method_serves_the_launch_mob_only`):
/// both must be the one function `is_stdio_live_method(method)` and nothing
/// beside it; what differs, `None` when they are one. A live method one of
/// them names and the other does not would be served for another mob with
/// the launch mob's transport, or refused for the launch mob's own.
fn live_family_tests_differ(dispatch: &str, seam: &str) -> Option<String> {
    const THE_TEST: &str = "is_stdio_live_method(method)";
    let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let arms = stdio_arms(dispatch);
    let Some(arm) = arms
        .iter()
        .find(|arm| arm.head.contains("is_stdio_live_method") || arm.head.contains("meta/live/"))
    else {
        return Some("the stdio dispatch has no arm of the live family".to_string());
    };
    let of_the_arm = arm.head.strip_prefix("method if ").unwrap_or(&arm.head);
    if of_the_arm != THE_TEST {
        return Some(format!(
            "rpc.rs:{}: the arm's test is `{of_the_arm}`, not `{THE_TEST}` alone",
            arm.line
        ));
    }
    let head = "pub(crate) fn stdio_method_serves_the_launch_mob_only(method: &str) -> bool {";
    let Some(from) = seam.find(head) else {
        return Some(
            "the addressing seam has no function stdio_method_serves_the_launch_mob_only"
                .to_string(),
        );
    };
    let body = &seam[from + head.len()..];
    let of_the_seam = one_line(&body[..body.find("\n}").unwrap_or(body.len())]);
    let expected =
        format!("{LAUNCH_MOB_ONLY_LIST} .binary_search(&method) .is_ok() || super::{THE_TEST}");
    (of_the_seam != expected)
        .then(|| format!("the seam's rule is `{of_the_seam}`, not the list or `super::{THE_TEST}`"))
}

/// The live family is named once for two uses: the arm that serves it and
/// the rule that refuses it for another mob call one and the same function.
#[test]
fn the_live_family_is_the_same_for_its_arm_and_for_the_rule_that_refuses_it() {
    let dispatch = include_str!("../rpc.rs");
    let seam = include_str!("mob_addressing.rs");
    assert_eq!(live_family_tests_differ(dispatch, seam), None);
    // The comparison fails a method that only one of the two names.
    let only_in_the_seam = seam.replacen(
        "|| super::is_stdio_live_method(method)\n",
        "|| super::is_stdio_live_method(method) || method == \"meta/voice\"\n",
        1,
    );
    let only_in_the_arm = dispatch.replacen(
        "method if is_stdio_live_method(method) =>",
        "method if is_stdio_live_method(method) || method == \"meta/voice\" =>",
        1,
    );
    for (what, dispatch, seam) in [
        (
            "a method only the seam names",
            dispatch,
            only_in_the_seam.as_str(),
        ),
        (
            "a method only the arm names",
            only_in_the_arm.as_str(),
            seam,
        ),
    ] {
        assert!(
            live_family_tests_differ(dispatch, seam).is_some(),
            "{what}: the comparison passes two tests that differ"
        );
    }
    assert_ne!(only_in_the_seam, seam, "the seam's text was changed");
    assert_ne!(only_in_the_arm, dispatch, "the arm's text was changed");
}
