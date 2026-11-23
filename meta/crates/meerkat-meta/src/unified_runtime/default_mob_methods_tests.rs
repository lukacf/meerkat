//! The methods of the runtime that mean the default mob, as data, and the
//! row that keeps that list equal to the code.
//!
//! A handler that serves one mob holds a resolved mob
//! ([`MobEntryRef`](super::mob_entry::MobEntryRef)) and reaches the process
//! part through its `runtime()`. A method of the runtime that reads the
//! default mob's entry answers for the default mob whatever mob the handler
//! holds, so such a handler must not call one through `runtime()`. A source
//! guard over the handlers reads the names from
//! [`DEFAULT_MOB_METHODS`].

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

/// Every method of `UnifiedRuntime` (and of its state, `UnifiedRuntimeCore`)
/// that reads the default mob's entry and answers for THAT MOB: directly,
/// or through another such method it calls on `self`. Sorted, one name per
/// line.
///
/// Not listed, although they read the default mob's entry too: the
/// [`SEAM`] (the methods that answer for the mob a caller named), the
/// [`LAUNCH_FACTS`] and the [`HOST_FACTS`] (the same answer whatever mob a
/// request names). Not seen by the rule, so listed by hand when one
/// appears: a free function that takes the runtime; a call on a receiver
/// other than `self` (a local alias of `self` included); and a method that
/// calls the seam with no mob named (`self.resolve_mob(None)`), which means
/// the default mob although the seam it calls is neutral.
pub(crate) const DEFAULT_MOB_METHODS: &[&str] = &[
    "activate_without_identity_context",
    "actor_loop_health",
    "agent_can_address_peer",
    "attach_identity_first_context",
    "attach_identity_gate",
    "attach_identity_policy",
    "attach_identity_reads",
    "bilateral_same_process_state",
    "bind_bootstrap_member_build_wiring",
    "bootstrap",
    "bootstrap_edges_report",
    "bootstrap_for_the_builder",
    "bootstrap_installed_identity_first_context",
    "bootstrap_installed_identity_first_context_held",
    "bootstrap_installed_identity_first_context_held_until",
    "bootstrap_installed_identity_first_context_inner",
    "bootstrap_with_launch_topology",
    "bootstrap_with_options",
    "bootstrap_with_options_and_topology",
    "bootstrap_with_topology",
    "broken_members",
    "build_console_json_router",
    "build_console_json_router_with_policy",
    "build_reference_app_router",
    "build_reference_app_router_with_console_visibility_policy",
    "build_reference_app_router_with_live",
    "clear_cross_mob_wake_routes",
    "close_cross_mob_links",
    "close_event_router",
    "configure_implicit_delegate_retirement",
    "console_identity_roster",
    "console_send_precheck",
    "continuity_mirror_counts",
    "create_console_projection",
    "default_mob",
    "default_mob_mut",
    "default_mob_resolved",
    "delete_mob_labels",
    "delete_run_labels",
    "drain_in_flight_turns",
    "evaluate_gating_action_with_origin",
    "finish_held_identity_bootstrap",
    "forget_agent_memory",
    "fork_member",
    "fork_member_with_identity_runtime",
    "get_mob_labels",
    "get_run_labels",
    "identity_changes_enabled",
    "identity_family",
    "identity_family_seen_by",
    "identity_first_context",
    "identity_runtime",
    "identity_runtime_ref",
    "initialize_console_projection_with_policy",
    "install_and_bootstrap_identity_first_context",
    "install_continuity_mirror",
    "install_identity_first_context",
    "install_identity_first_context_authority",
    "install_identity_first_flow_target_provisioner",
    "install_remote_host_facts",
    "install_test_console_event_ingress",
    "install_test_event_ingress",
    "local_member_peer_facts",
    "local_member_peer_info",
    "materialize_identity_first_for_flow",
    "mint_bootstrap_completion",
    "mob_events_store",
    "mob_handle",
    "mob_id",
    "mob_runtime",
    "mobpack_runtime_catalog_state_snapshot",
    "query_mob_events",
    "recall_agent_memory",
    "reconcile",
    "reconcile_bootstrap_edges_if_configured",
    "reconcile_edges",
    "reconcile_edges_from_members",
    "rediscover",
    "reference_app_router_under",
    "reference_app_routes",
    "refresh_desired_topology",
    "register_peer_runtime",
    "remember_agent_memory",
    "restore_cross_mob_wake_routes",
    "run",
    "send_cross_mob",
    "serve",
    "serve_ask_user",
    "session_bridge",
    "set_access_controller",
    "set_console_identity_roster",
    "set_console_session_fences",
    "set_durable_decisions",
    "set_error_hook",
    "set_gateway_peer_keys",
    "set_identity_request_services",
    "set_mob_labels",
    "set_run_labels",
    "set_topology_control_policy",
    "settle_member_mcp",
    "settle_member_mcp_to_exit",
    "shutdown",
    "shutdown_mob_actor",
    "shutdown_with_root_budget",
    "spawn",
    "spawn_many",
    "start_control_listener",
    "start_control_listener_over",
    "start_control_listener_with_authorizer",
    "start_member_turn",
    "stop_ask_user",
    "stop_mob",
    "stop_mob_for_teardown",
    "stop_mob_unless_candidate",
    "stop_mob_with",
    "subscribe_mob_events",
    "take_bootstrap_completion",
    "topology_concrete_member_id",
    "topology_controller",
    "topology_runtime_handle",
    "unwire_bilateral_same_process",
    "unwire_cross_mob",
    "unwire_local",
    "wire_bilateral_same_process",
    "wire_cross_mob",
    "wire_local",
    "withdraw_bootstrap_completion",
    "workgraph_fact_hub",
    "workgraph_realm_migration",
    "workgraph_service",
];

/// The seam where a request that names no mob gets the default mob: the
/// resolver, the question that answers the served mob's id without
/// building anything, the question that answers a served mob's
/// incarnation without loading it, and the methods that hand a request
/// door (or a task of the runtime's own that serves requests) its own
/// resolver. They read the default mob's entry to answer for the mob the
/// caller named, and they are how a handler gets its mob, so they are not
/// among [`DEFAULT_MOB_METHODS`].
const SEAM: &[&str] = &[
    "mob_resolver",
    "mob_resolver_of_its_own_task",
    "resolve_mob",
    "served_mob_id",
    "served_mob_incarnation",
];

/// Facts of the launch: the runtime reads them through the default mob's
/// wrapper, and they are the same for every mob the host serves (whether
/// the launch speaks for the durable composition; the storage durability
/// the launch resolved). A handler may read them from the runtime whatever
/// mob it serves, so they are not among [`DEFAULT_MOB_METHODS`].
pub(crate) const LAUNCH_FACTS: &[&str] = &["resolved_storage", "speaks_for_composition"];

/// Facts of the host, read through the default mob's wrapper only because
/// the wrapper carries the process's part: the blob store, which is the
/// process's and serves every mob, the mob set's answer for the whole
/// host, and the slot the members' `ask_user` tool reads, one for the
/// process, with the decisions the host's question sets are held in. They
/// are the same whatever mob a request names, and reading one neither
/// resolves nor loads a mob, so they are not among
/// [`DEFAULT_MOB_METHODS`].
pub(crate) const HOST_FACTS: &[&str] = &["ask_user_slot", "binary_blob_store", "quarantined_mobs"];

/// The types whose inherent methods are the runtime's.
const RUNTIME_TYPES: &[&str] = &["UnifiedRuntime", "UnifiedRuntimeCore"];

/// Every production source of this crate: each `.rs` file under `src/` (its
/// binaries included), by its path from `src/`, except the test files
/// (`*_tests.rs`, `tests.rs`, `*test_support.rs`, and the files of a `tests`
/// or `*_tests` directory). A file added later is read too.
pub(crate) fn crate_production_sources() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
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
                if name != "tests" && !name.ends_with("_tests") {
                    dirs.push(path);
                }
                continue;
            }
            if !name.ends_with(".rs")
                || name == "tests.rs"
                || name.ends_with("_tests.rs")
                || name.ends_with("test_support.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a source reads");
            let relative = path
                .strip_prefix(&root)
                .expect("a source under src")
                .to_string_lossy()
                .replace('\\', "/");
            sources.push((relative, source));
        }
    }
    sources.sort();
    sources
}

/// Whether `byte` can be part of an identifier.
fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// `source` with every comment and every string, raw string and character
/// literal blanked to spaces (newlines kept), byte for byte: what is left is
/// the code, at the same offsets.
fn code_only(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let end = if bytes[at..].starts_with(b"//") {
            bytes[at..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |line_end| at + line_end)
        } else if bytes[at..].starts_with(b"/*") {
            block_comment_end(bytes, at)
        } else if bytes[at] == b'"' {
            string_end(bytes, at)
        } else if let Some(end) = raw_string_end(bytes, at) {
            end
        } else if let Some(end) = char_literal_end(bytes, at) {
            end
        } else {
            at += 1;
            continue;
        };
        for byte in &mut code[at..end.min(bytes.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    code
}

/// Where the block comment opening at `from` ends: block comments nest.
fn block_comment_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = from;
    while at < bytes.len() {
        if bytes[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if bytes[at..].starts_with(b"*/") {
            depth = depth.saturating_sub(1);
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    bytes.len()
}

/// Where the string literal opening at `at` ends: after its closing quote.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    while let Some(byte) = bytes.get(end) {
        match byte {
            b'\\' => end += 2,
            b'"' => return end + 1,
            _ => end += 1,
        }
    }
    bytes.len()
}

/// Where the raw string literal (`r"..."`, `r#"..."#`, `br"..."`) opening at
/// `at` ends, or `None` when none opens there: a raw identifier such as
/// `r#type`, or an identifier that holds an `r` or a `b`.
fn raw_string_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at
        .checked_sub(1)
        .is_some_and(|before| is_ident(bytes[before]))
    {
        return None;
    }
    let mut quote = at;
    if bytes.get(quote) == Some(&b'b') {
        quote += 1;
    }
    if bytes.get(quote) != Some(&b'r') {
        return None;
    }
    quote += 1;
    let hashes = bytes[quote.min(bytes.len())..]
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    quote += hashes;
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let mut end = quote + 1;
    while end < bytes.len() {
        if bytes[end] == b'"'
            && bytes[end + 1..]
                .iter()
                .take(hashes)
                .filter(|byte| **byte == b'#')
                .count()
                == hashes
        {
            return Some(end + 1 + hashes);
        }
        end += 1;
    }
    Some(bytes.len())
}

/// Where the character literal opening at `at` ends, or `None` when the
/// quote there opens none (a lifetime or a label).
fn char_literal_end(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at) != Some(&b'\'') {
        return None;
    }
    let first = *bytes.get(at + 1)?;
    if first == b'\\' {
        // An escape: its closing quote is the next one after the escaped
        // character.
        let close = bytes
            .get(at + 3..)?
            .iter()
            .position(|byte| *byte == b'\'')?;
        return Some(at + 3 + close + 1);
    }
    // One character, of one to four bytes, then the closing quote.
    let width = match first {
        byte if byte < 0x80 => 1,
        byte if byte >= 0xF0 => 4,
        byte if byte >= 0xE0 => 3,
        _ => 2,
    };
    (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 2 + width)
}

/// The position of the brace that closes the one at `open`.
fn matching_brace(code: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    for (offset, byte) in code[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return open + offset;
                }
            }
            _ => {}
        }
    }
    code.len().saturating_sub(1)
}

/// The first position at or after `from` and before `to` where the word
/// `token` stands on its own.
fn find_token(code: &[u8], token: &[u8], from: usize, to: usize) -> Option<usize> {
    let mut at = from;
    while at + token.len() <= to {
        if code[at..].starts_with(token)
            && !at
                .checked_sub(1)
                .is_some_and(|before| is_ident(code[before]))
            && !code
                .get(at + token.len())
                .is_some_and(|after| is_ident(*after))
        {
            return Some(at);
        }
        at += 1;
    }
    None
}

/// The first position at or after `at` that is not white space.
fn skip_space(code: &[u8], mut at: usize) -> usize {
    while code.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The end of the identifier that starts at `at` (`at` itself when none
/// starts there).
fn ident_end(code: &[u8], mut at: usize) -> usize {
    while code.get(at).is_some_and(|byte| is_ident(*byte)) {
        at += 1;
    }
    at
}

/// `code` with every `#[cfg(test)]` module blanked, inline or out of line.
fn without_test_modules(mut code: Vec<u8>) -> Vec<u8> {
    const TEST_GATE: &[u8] = b"#[cfg(test)]";
    let mut at = 0;
    while at + TEST_GATE.len() <= code.len() {
        if !code[at..].starts_with(TEST_GATE) {
            at += 1;
            continue;
        }
        let start = at;
        at += TEST_GATE.len();
        // Further attributes, a visibility, then `mod name`.
        let mut item = skip_space(&code, at);
        while code.get(item) == Some(&b'#') {
            let Some(close) = code[item..].iter().position(|byte| *byte == b']') else {
                break;
            };
            item = skip_space(&code, item + close + 1);
        }
        if code[item..].starts_with(b"pub") {
            item = skip_space(&code, ident_end(&code, item));
            if code.get(item) == Some(&b'(') {
                let Some(close) = code[item..].iter().position(|byte| *byte == b')') else {
                    continue;
                };
                item = skip_space(&code, item + close + 1);
            }
        }
        if !code[item..].starts_with(b"mod") || code.get(item + 3).is_some_and(|b| is_ident(*b)) {
            continue;
        }
        let name_end = ident_end(&code, skip_space(&code, item + 3));
        let after = skip_space(&code, name_end);
        let end = match code.get(after) {
            Some(b'{') => matching_brace(&code, after) + 1,
            Some(b';') => after + 1,
            _ => continue,
        };
        for byte in &mut code[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    code
}

/// The inherent methods of the runtime's types in `code` (production code,
/// comments and literals blanked), each with its body.
fn runtime_methods(code: &str, methods: &mut BTreeMap<String, Vec<String>>) {
    let bytes = code.as_bytes();
    let mut at = 0;
    while let Some(found) = find_token(bytes, b"impl", at, bytes.len()) {
        at = found + 4;
        let Some(open) = bytes[at..].iter().position(|byte| *byte == b'{') else {
            return;
        };
        let open = at + open;
        if bytes[at..open].contains(&b';') {
            continue;
        }
        let header = code[at..open].split_whitespace().collect::<Vec<_>>();
        // `impl Trait for Type` is not an inherent impl; `impl Type` names
        // the type, possibly by a path.
        let inherent_of_runtime = !header.contains(&"for")
            && header.len() == 1
            && header[0]
                .rsplit("::")
                .next()
                .is_some_and(|name| RUNTIME_TYPES.contains(&name));
        if !inherent_of_runtime {
            continue;
        }
        let close = matching_brace(bytes, open);
        let mut item = open + 1;
        while let Some(found) = find_token(bytes, b"fn", item, close) {
            let name_start = skip_space(bytes, found + 2);
            let name_end = ident_end(bytes, name_start);
            if name_end == name_start {
                // A function pointer type.
                item = found + 2;
                continue;
            }
            // The body opens at the first brace outside the signature's
            // parentheses and brackets; a `;` there first is a declaration.
            let mut depth = 0_usize;
            let mut body = None;
            for (offset, byte) in bytes[name_end..close].iter().enumerate() {
                match byte {
                    b'(' | b'[' => depth += 1,
                    b')' | b']' => depth = depth.saturating_sub(1),
                    b'{' if depth == 0 => {
                        body = Some(name_end + offset);
                        break;
                    }
                    b';' if depth == 0 => break,
                    _ => {}
                }
            }
            let Some(body_open) = body else {
                item = name_end;
                continue;
            };
            let body_close = matching_brace(bytes, body_open);
            methods
                .entry(code[name_start..name_end].to_string())
                .or_default()
                .push(code[body_open..=body_close].to_string());
            item = body_close + 1;
        }
        at = close;
    }
}

/// The names `body` calls on `self` (`self.name(`) or on its own type
/// (`Self::name(`).
fn self_calls(body: &str) -> BTreeSet<String> {
    let bytes = body.as_bytes();
    let mut calls = BTreeSet::new();
    for (receiver, separator) in [(&b"self"[..], &b"."[..]), (&b"Self"[..], &b"::"[..])] {
        let mut at = 0;
        while let Some(found) = find_token(bytes, receiver, at, bytes.len()) {
            at = found + receiver.len();
            let separator_at = skip_space(bytes, at);
            if !bytes[separator_at..].starts_with(separator) {
                continue;
            }
            let name_start = skip_space(bytes, separator_at + separator.len());
            let name_end = ident_end(bytes, name_start);
            if name_end > name_start && bytes.get(skip_space(bytes, name_end)) == Some(&b'(') {
                calls.insert(body[name_start..name_end].to_string());
            }
        }
    }
    calls
}

/// The methods of the runtime that read the default mob's entry in
/// `sources`: a body that names `.default_mob` (the field or its
/// accessors), and every method that calls such a method on `self`. A call
/// of one of the `neutral` methods does not count: those read the default
/// mob's entry and answer for no mob in particular (a fact of the launch or
/// of the host) or for the mob the caller named.
fn methods_that_mean_the_default_mob(
    sources: &[(String, String)],
    neutral: &[&str],
) -> (usize, BTreeSet<String>) {
    let mut methods = BTreeMap::new();
    for (_, source) in sources {
        let code = without_test_modules(code_only(source));
        runtime_methods(&String::from_utf8_lossy(&code), &mut methods);
    }
    let mut means: BTreeSet<String> = methods
        .iter()
        .filter(|(_, bodies)| bodies.iter().any(|body| body.contains(".default_mob")))
        .map(|(name, _)| name.clone())
        .collect();
    loop {
        let added: Vec<String> = methods
            .iter()
            .filter(|(name, bodies)| {
                !means.contains(*name)
                    && bodies.iter().any(|body| {
                        self_calls(body)
                            .iter()
                            .any(|call| means.contains(call) && !neutral.contains(&call.as_str()))
                    })
            })
            .map(|(name, _)| name.clone())
            .collect();
        if added.is_empty() {
            break;
        }
        means.extend(added);
    }
    (methods.len(), means)
}

/// The scanner reads a method's body past comments, strings and test
/// modules, follows calls on `self` and on `Self`, and leaves other types'
/// methods and other receivers alone.
#[test]
fn the_scanner_finds_the_methods_that_reach_the_default_mob() {
    let source = r##"
impl UnifiedRuntimeCore {
    fn default_mob(&self) -> &Entry {
        &self.default_mob
    }
}

impl Deref for UnifiedRuntime {
    fn deref(&self) -> &Core { self.default_mob(); &self.core }
}

impl super::UnifiedRuntime {
    /// Not a body: fn in_a_comment() { self.default_mob() }
    pub fn direct(&self, _window: [u8; 4]) -> usize {
        let text = "} self.default_mob() {";
        let quote = '{';
        self.default_mob().len() + text.len() + quote.len_utf8()
    }

    pub(crate) async fn through_self(&self) -> usize {
        self
            .direct([0; 4])
    }

    fn through_own_type(runtime: &Self) -> usize {
        Self::through_self(runtime)
    }

    pub fn process_only(&self, other: &Other) -> usize {
        // self.default_mob()
        other.direct() + r#"self.direct("#.len()
    }

    pub fn a_launch_fact(&self) -> bool {
        self.default_mob().is_empty()
    }

    pub fn through_a_fact_only(&self) -> bool {
        self.a_launch_fact()
    }

    fn declared_only(&self);
}

impl Other {
    fn direct(&self) -> usize { self.default_mob() }
}

#[cfg(test)]
mod tests {
    impl UnifiedRuntime {
        fn only_in_tests(&self) { self.default_mob(); }
    }
}
"##;
    let (methods, means) = methods_that_mean_the_default_mob(
        &[("fixture.rs".to_string(), source.to_string())],
        &["a_launch_fact"],
    );
    assert_eq!(methods, 7, "the runtime's own methods with a body");
    assert_eq!(
        means.iter().map(String::as_str).collect::<Vec<_>>(),
        [
            "a_launch_fact",
            "default_mob",
            "direct",
            "through_own_type",
            "through_self"
        ],
        "a method that reaches the default mob only through a neutral method does not mean it"
    );
}

/// The list is the code's: every method of the runtime that reads the
/// default mob's entry, directly or through `self`, is listed, and nothing
/// else is.
#[test]
fn the_list_of_methods_that_mean_the_default_mob_is_what_the_code_reads() {
    let sources = crate_production_sources();
    let neutral: Vec<&str> = [SEAM, LAUNCH_FACTS, HOST_FACTS].concat();
    let (methods, mut means) = methods_that_mean_the_default_mob(&sources, &neutral);
    assert!(
        methods > 100 && means.contains("mob_handle") && means.contains("default_mob"),
        "the scanner reads the runtime's methods: {methods} found"
    );
    for (group, names) in [
        ("the seam", SEAM),
        ("the launch facts", LAUNCH_FACTS),
        ("the host facts", HOST_FACTS),
    ] {
        for name in names {
            assert!(
                means.remove(*name),
                "{name} is listed among {group} and does not read the default mob's entry"
            );
        }
    }
    let listed: BTreeSet<String> = DEFAULT_MOB_METHODS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert!(
        DEFAULT_MOB_METHODS.windows(2).all(|pair| pair[0] < pair[1]),
        "the list is sorted and names no method twice"
    );
    let missing: Vec<&String> = means.difference(&listed).collect();
    let stale: Vec<&String> = listed.difference(&means).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "DEFAULT_MOB_METHODS is not what the code reads.\nread the default mob's entry and are \
         not listed: {missing:?}\nlisted and do not read it: {stale:?}"
    );
}

/// Every function with a body in `code` (production code, comments and
/// literals blanked), by name: a method or a free function, with whatever
/// its body nests.
fn functions(code: &str) -> Vec<(String, String)> {
    let bytes = code.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(start) = find_token(bytes, b"fn", at, bytes.len()) {
        let name_start = skip_space(bytes, start + 2);
        let name_end = ident_end(bytes, name_start);
        if name_end == name_start {
            at = start + 2;
            continue;
        }
        let mut depth = 0_usize;
        let mut body = None;
        for (offset, byte) in bytes[name_end..].iter().enumerate() {
            match byte {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' if depth == 0 => {
                    body = Some(name_end + offset);
                    break;
                }
                b';' if depth == 0 => break,
                _ => {}
            }
        }
        let Some(body_open) = body else {
            at = name_end;
            continue;
        };
        let body_close = matching_brace(bytes, body_open);
        found.push((
            code[name_start..name_end].to_string(),
            code[body_open..=body_close].to_string(),
        ));
        at = body_close + 1;
    }
    found
}

/// The names `body` calls: every identifier an opening parenthesis follows.
fn calls(body: &str) -> BTreeSet<String> {
    let bytes = body.as_bytes();
    let mut names = BTreeSet::new();
    let mut at = 0;
    while at < bytes.len() {
        if !is_ident(bytes[at]) {
            at += 1;
            continue;
        }
        let end = ident_end(bytes, at);
        if bytes.get(skip_space(bytes, end)) == Some(&b'(') {
            names.insert(body[at..end].to_string());
        }
        at = end;
    }
    names
}

/// Whether `call` builds a route group whose doors resolve the mob a
/// request addresses: the console's router (its request door with the
/// multipart route, and its twelve HTTP routes that take a mob id, seven
/// read routes by a query parameter and five write routes by a member of
/// the body), the member, mob and structural event streams, and the
/// WorkGraph fact stream.
fn mounts_a_mob_door(call: &str) -> bool {
    call.starts_with("console_json_router")
        || call.contains("events_sse_router")
        || call.contains("facts_sse_router")
}

/// The request doors hold no runtime: they resolve the mob a request
/// addresses with the resolver the router carries. So every function that
/// mounts one of their route groups for a host installs the runtime's
/// resolver on the router it builds; a builder added later cannot leave it
/// out.
#[test]
fn every_router_builder_that_mounts_a_mob_door_installs_the_mob_resolver() {
    let mut builders = Vec::new();
    for (path, source) in crate_production_sources() {
        // The doors' own files define the route groups, each from the next.
        if path == "http_console.rs" || path.starts_with("http_console/") || path == "http_sse.rs" {
            continue;
        }
        let code = without_test_modules(code_only(&source));
        for (name, body) in functions(&String::from_utf8_lossy(&code)) {
            if calls(&body).iter().any(|call| mounts_a_mob_door(call)) {
                let installs = body.contains("Extension(") && body.contains("mob_resolver()");
                builders.push((path.clone(), name, installs));
            }
        }
    }
    for builder in [
        "build_console_json_router_with_policy",
        "reference_app_routes",
    ] {
        assert!(
            builders
                .iter()
                .any(|(path, name, _)| path == "unified_runtime/http.rs" && name == builder),
            "the scan finds {builder}: {builders:?}"
        );
    }
    let without: Vec<_> = builders
        .iter()
        .filter(|(_, _, installs)| !installs)
        .map(|(path, name, _)| format!("{path}: {name}"))
        .collect();
    assert!(
        without.is_empty(),
        "these functions mount a request door's routes and do not install the mob resolver \
         (`.layer(axum::Extension(self.mob_resolver()))`): {without:?}"
    );
}

/// The two builders of a mob id's refusal that name the host's default
/// mob beside the id the request named.
const REFUSALS_NAMING_THE_DEFAULT_MOB: &[&str] = &["foreign_mob_target", "mob_resolve_refusal"];

/// The member of that refusal that names the host's default mob.
const MEMBER_NAMING_THE_DEFAULT_MOB: &str = "gateway_mob_id";

/// The doors on the host's stdio that live outside the stdio dispatch's
/// own files, by exact path. A stdio door has one caller, the host, and
/// answers a mob id it does not have as the stdio dispatch does: with the
/// requested id and the host's default mob. The decision door reads its
/// methods before the dispatch, and answers a mob id it does not have in
/// that form.
const STDIO_DOORS_OUTSIDE_THE_DISPATCH: &[&str] = &["durable_decisions/stdio.rs"];

/// The functions of the stdio dispatch's own files (`rpc.rs` and the files
/// under `rpc/`) that build the stdio form of a mob id's refusal, by file
/// and name. No file is let through by its path: a handler under `rpc/` is
/// shared by the stdio dispatch and the console's request door.
///
/// The dispatch function answers its one caller, the host. The handler
/// listed beside it is reached from both doors and builds the stdio form
/// only where the stdio dispatch called it: it is told the door it answers
/// on (`MobRefusalDoor`), which the addressing rows hold
/// (`a_shared_handler_refuses_a_mob_id_in_the_form_of_the_door_it_answers_on`).
/// A function is added here only when every caller of it is the host, or
/// when it selects the form by the door it answers on.
const STDIO_FORM_IN_THE_DISPATCH: &[(&str, &str)] = &[
    ("rpc.rs", "handle_unified_rpc_json_inner"),
    ("rpc/mob_methods.rs", "reject_foreign_mob"),
    // The methods over the mobs of a host: both select the form by the
    // door the handler answers on.
    ("rpc/mob_hosting.rs", "not_a_mob_of_the_host"),
    ("rpc/mob_hosting.rs", "of"),
];

/// Whether `code` (comments and literals blanked) names the identifier
/// `name` anywhere: called, imported, renamed or taken as a value.
fn names(code: &str, name: &str) -> bool {
    find_token(code.as_bytes(), name.as_bytes(), 0, code.len()).is_some()
}

/// How often `code` (comments and literals blanked) names the identifier
/// `name`.
fn times_named(code: &str, name: &str) -> usize {
    let bytes = code.as_bytes();
    let mut times = 0;
    let mut at = 0;
    while let Some(found) = find_token(bytes, name.as_bytes(), at, bytes.len()) {
        times += 1;
        at = found + name.len();
    }
    times
}

/// The string literals of `source` that stand in its production code:
/// outside comments and outside `#[cfg(test)]` modules.
fn production_string_literals(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let code = code_only(source);
    let production = without_test_modules(code.clone());
    let mut literals = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let (end, literal) = if bytes[at..].starts_with(b"//") {
            let end = bytes[at..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |line_end| at + line_end);
            (end, false)
        } else if bytes[at..].starts_with(b"/*") {
            (block_comment_end(bytes, at), false)
        } else if bytes[at] == b'"' {
            (string_end(bytes, at), true)
        } else if let Some(end) = raw_string_end(bytes, at) {
            (end, true)
        } else if let Some(end) = char_literal_end(bytes, at) {
            (end, false)
        } else {
            at += 1;
            continue;
        };
        let end = end.min(bytes.len());
        // A literal of a test module follows code the module's blanking
        // took with it.
        let in_production = code[..at]
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .is_none_or(|before| production[before] != b' ');
        if literal && in_production {
            literals.push(&source[at..end]);
        }
        at = end;
    }
    literals
}

/// The scan of string literals keeps production literals and leaves out
/// comments, character literals and the literals of a test module.
#[test]
fn the_literal_scan_reads_production_strings_only() {
    let source = r##"
// "in a line comment"
/* "in a block comment" */
fn production() -> (&'static str, &'static str, char) {
    ("plain \" quoted", r#"raw "one" end"#, '"')
}

#[cfg(test)]
mod tests {
    fn only_in_tests() -> &'static str {
        "in a test module"
    }
}

const AFTER: &str = "after the test module";
"##;
    assert_eq!(
        production_string_literals(source),
        [
            r#""plain \" quoted""#,
            r##"r#"raw "one" end"#"##,
            r#""after the test module""#,
        ]
    );
}

/// A door that has callers (the console's request doors, the event
/// streams) answers a mob id it does not serve to its caller without
/// naming a mob the request did not name. So only what answers the host's
/// stdio, whose one caller is the host itself, builds the refusal that
/// names the host's default mob.
///
/// A builder of that refusal is named (a call, an import under another
/// name, the function taken as a value) only in a stdio door listed by its
/// path, in a function of the stdio dispatch's own files listed by file and
/// name, and in the addressing seam's own builders. No file of the dispatch
/// is let through by its path, `rpc.rs` included, and a builder named
/// outside every function (an import) is refused in each of them. Every
/// listed function names a builder, so the list does not outlive its code.
///
/// The member that names the default mob is spelled in one string of the
/// crate's production code, in the seam's builder, and is no name of the
/// code anywhere (a field or a binding that would be written under it).
#[test]
fn no_door_with_callers_builds_a_refusal_that_names_the_default_mob() {
    let mut naming = Vec::new();
    let mut seam_read = false;
    let mut seam_spells_the_member = 0;
    let mut stdio_doors_read = 0;
    let mut listed_and_naming = BTreeSet::new();
    for (path, source) in crate_production_sources() {
        let code = without_test_modules(code_only(&source));
        let code = String::from_utf8_lossy(&code);
        let a_stdio_door = STDIO_DOORS_OUTSIDE_THE_DISPATCH.contains(&path.as_str());
        stdio_doors_read += usize::from(a_stdio_door);
        let the_seam = path == "rpc/mob_addressing.rs";
        seam_read |= the_seam;

        let spells_the_member = production_string_literals(&source)
            .iter()
            .filter(|literal| literal.contains(MEMBER_NAMING_THE_DEFAULT_MOB))
            .count();
        if the_seam {
            seam_spells_the_member = spells_the_member;
        } else if spells_the_member > 0 {
            naming.push(format!(
                "{path}: spells the member {MEMBER_NAMING_THE_DEFAULT_MOB} in a string"
            ));
        }
        if names(&code, MEMBER_NAMING_THE_DEFAULT_MOB) {
            naming.push(format!(
                "{path}: names {MEMBER_NAMING_THE_DEFAULT_MOB} in its code"
            ));
        }
        if a_stdio_door {
            continue;
        }

        let bodies = functions(&code);
        for builder in REFUSALS_NAMING_THE_DEFAULT_MOB {
            let mut in_functions = 0;
            for (name, body) in &bodies {
                let times = times_named(body, builder);
                in_functions += times;
                // A builder's own body, and the one mapping of the
                // resolver's refusal to it, are the seam's.
                if times == 0
                    || (the_seam && REFUSALS_NAMING_THE_DEFAULT_MOB.contains(&name.as_str()))
                {
                    continue;
                }
                match STDIO_FORM_IN_THE_DISPATCH
                    .iter()
                    .find(|(file, function)| *file == path.as_str() && *function == name.as_str())
                {
                    Some(listed) => {
                        listed_and_naming.insert(*listed);
                    }
                    None => naming.push(format!("{path}: {name} names {builder}")),
                }
            }
            // Outside every function: an import, under its own name or
            // another. The seam defines the builders there.
            if !the_seam && times_named(&code, builder) > in_functions {
                naming.push(format!("{path}: names {builder} outside a function"));
            }
        }
    }
    assert!(seam_read, "the scan reads the addressing seam");
    assert_eq!(
        seam_spells_the_member, 1,
        "the addressing seam spells the member {MEMBER_NAMING_THE_DEFAULT_MOB} in one string: \
         the builder of the stdio form"
    );
    assert_eq!(
        stdio_doors_read,
        STDIO_DOORS_OUTSIDE_THE_DISPATCH.len(),
        "every stdio door listed by path is a file of this crate"
    );
    let listed_and_silent: Vec<_> = STDIO_FORM_IN_THE_DISPATCH
        .iter()
        .filter(|listed| !listed_and_naming.contains(*listed))
        .collect();
    assert!(
        listed_and_silent.is_empty(),
        "these listed functions of the stdio dispatch name no builder of the stdio form; \
         remove them from the list: {listed_and_silent:?}"
    );
    assert!(
        naming.is_empty(),
        "these places of a door that has callers build a refusal that names the host's \
         default mob: {naming:?}"
    );
}

/// The functions that write a mob's entry in place, by file and name: the
/// two that compose a runtime, each before it hands the runtime out (the
/// bootstrap behind every bootstrap form, and the builder's build).
const ENTRY_WRITERS: &[(&str, &str)] = &[
    ("unified_runtime/builder.rs", "build"),
    ("unified_runtime/mod.rs", "bootstrap_with_launch_topology"),
];

/// The two accessors of the entry, the only functions that read the
/// runtime state's field: both in the file that defines it.
const ENTRY_ACCESSORS: &[&str] = &["default_mob", "default_mob_mut"];

/// The functions that name the runtime's state with its fields
/// (`UnifiedRuntimeCore { .. }`), by file and name: the one that builds a
/// runtime's state, and the builder, which builds it again around the entry
/// it wrote. A destructuring pattern of the state reads the same, and
/// reaches the entry's field without a dot.
const STATE_LITERALS: &[(&str, &str)] = &[
    ("unified_runtime/builder.rs", "build"),
    ("unified_runtime/mod.rs", "from_parts"),
];

/// Whether `code` (comments and literals blanked) reads the field `field`
/// of a value: `.field` that is not a method call.
fn reads_field(code: &str, field: &str) -> bool {
    let bytes = code.as_bytes();
    let mut at = 0;
    while let Some(start) = find_token(bytes, field.as_bytes(), at, bytes.len()) {
        let after = skip_space(bytes, start + field.len());
        let after_a_dot = bytes[..start]
            .iter()
            .rposition(|byte| !byte.is_ascii_whitespace())
            .is_some_and(|before| bytes[before] == b'.');
        if after_a_dot && bytes.get(after) != Some(&b'(') {
            return true;
        }
        at = start + field.len();
    }
    false
}

/// Whether `code` (comments and literals blanked) names the type `name`
/// with its fields: `name { .. }`, a literal or a destructuring pattern.
fn names_with_its_fields(code: &str, name: &str) -> bool {
    let bytes = code.as_bytes();
    let mut at = 0;
    while let Some(start) = find_token(bytes, name.as_bytes(), at, bytes.len()) {
        let after = skip_space(bytes, start + name.len());
        if bytes.get(after) == Some(&b'{') {
            return true;
        }
        at = start + name.len();
    }
    false
}

/// A mob's entry is written in place only while its runtime is composed:
/// no function of the crate takes the entry mutably but the two that
/// build a runtime. So what an entry holds as a plain value (the mob's
/// WorkGraph service, its event store, its edge policy, its topology
/// controller) is fixed before any request door exists, and the owned
/// form of a resolved mob may hold it. What a host gives a mob after that
/// (its identity context, its console roster, the services of its identity
/// plane) lives in a cell of the entry and is read when it is asked for.
///
/// The entry is a private field of the runtime's state, which the module
/// that defines the state and its child modules can reach. So the scan
/// also fails any function of those files that reads the field itself and
/// is not one of the two accessors, whatever it does with it; and it counts
/// functions, so a second function of a listed file with a listed name is
/// one too many. A pattern that takes the state apart reaches the field
/// without a dot: a function of those files that names the state with its
/// fields is held against a list of its own ([`STATE_LITERALS`]).
///
/// Not seen: `Self { .. }` inside the state's own `impl`, which names the
/// state without its name.
#[test]
fn a_mobs_entry_is_written_in_place_only_while_its_runtime_is_composed() {
    let mut writers = Vec::new();
    let mut state_literals = Vec::new();
    let mut accessors_defined = 0;
    for (path, source) in crate_production_sources() {
        let code = without_test_modules(code_only(&source));
        let of_the_state = path.starts_with("unified_runtime/");
        for (name, body) in functions(&String::from_utf8_lossy(&code)) {
            if path == "unified_runtime/mod.rs" && ENTRY_ACCESSORS.contains(&name.as_str()) {
                accessors_defined += 1;
                continue;
            }
            if of_the_state && names_with_its_fields(&body, "UnifiedRuntimeCore") {
                state_literals.push((path.clone(), name.clone()));
            }
            if calls(&body).contains("default_mob_mut")
                || (of_the_state && reads_field(&body, "default_mob"))
            {
                writers.push((path.clone(), name));
            }
        }
    }
    assert_eq!(
        accessors_defined,
        ENTRY_ACCESSORS.len(),
        "the scan finds the entry's two accessors, each once"
    );
    writers.sort();
    let listed = |list: &[(&str, &str)]| -> Vec<(String, String)> {
        list.iter()
            .map(|(path, name)| ((*path).to_string(), (*name).to_string()))
            .collect()
    };
    assert_eq!(
        writers,
        listed(ENTRY_WRITERS),
        "a mob's entry is reached in place outside the composition of its runtime: the \
         functions that take it mutably or read the state's field, against the list"
    );
    state_literals.sort();
    assert_eq!(
        state_literals,
        listed(STATE_LITERALS),
        "the runtime's state is named with its fields outside the functions that build it: \
         a literal, or a pattern that reaches the entry's field"
    );
}

/// The functions that name a mob's entry with its fields (`MobEntry { .. }`),
/// by file and name: the one that builds a runtime's state with the default
/// mob's entry, and the one that builds the entry of a mob the launch's mob
/// set built for the host beside it.
const ENTRY_LITERALS: &[(&str, &str)] = &[
    ("unified_runtime/mob_entries.rs", "entry_of_hosted_mob"),
    ("unified_runtime/mod.rs", "from_parts"),
];

/// The one function that reads the runtime state's table of the other mobs'
/// entries: its accessor, in the file that defines the state.
const ENTRY_TABLE_ACCESSOR: &str = "other_mobs";

/// What would write a shared entry in place: a mutable borrow of an entry,
/// or one of the calls that take a shared value back to write it.
const ENTRY_WRITES_IN_PLACE: &[&str] = &["&mut MobEntry", "get_mut(", "make_mut(", "try_unwrap("];

/// The entry of a mob the launch's mob set built for the host beside the
/// default mob is built whole, by one function, and never written in place
/// afterwards: the table that holds it shares it, and what a host gives the
/// mob later lives in a cell of the entry, as for the default mob. So the
/// owned form of such a mob may hold what its entry holds as a plain value,
/// as it does for the default mob.
///
/// The scan holds three things. A mob's entry is named with its fields in
/// two functions of the crate, the one that builds a runtime's state and the
/// one that builds another mob's entry ([`ENTRY_LITERALS`]). The table is a
/// private field of the runtime's state, read by its accessor alone. And the
/// file of the table has no function that borrows an entry mutably or takes
/// a shared entry back to write it ([`ENTRY_WRITES_IN_PLACE`]).
///
/// Not seen: a write through a cell an entry field would hold (the fields
/// that are cells are the ones a host may set), and `Self { .. }` inside
/// the entry's own `impl`.
#[test]
fn the_entry_of_another_mob_is_built_whole_and_never_written_in_place() {
    let mut entry_literals = Vec::new();
    let mut table_readers = Vec::new();
    let mut writes_in_place = Vec::new();
    let mut table_file_read = false;
    for (path, source) in crate_production_sources() {
        let code = without_test_modules(code_only(&source));
        let of_the_state = path.starts_with("unified_runtime/");
        let of_the_table = path == "unified_runtime/mob_entries.rs";
        table_file_read |= of_the_table;
        for (name, body) in functions(&String::from_utf8_lossy(&code)) {
            if names_with_its_fields(&body, "MobEntry") {
                entry_literals.push((path.clone(), name.clone()));
            }
            if of_the_state && reads_field(&body, ENTRY_TABLE_ACCESSOR) {
                table_readers.push((path.clone(), name.clone()));
            }
            if of_the_table {
                for write in ENTRY_WRITES_IN_PLACE {
                    if body.contains(write) {
                        writes_in_place.push((name.clone(), *write));
                    }
                }
            }
        }
    }
    entry_literals.sort();
    assert_eq!(
        entry_literals,
        ENTRY_LITERALS
            .iter()
            .map(|(path, name)| ((*path).to_string(), (*name).to_string()))
            .collect::<Vec<_>>(),
        "a mob's entry is named with its fields by the function that builds a runtime's state \
         and by the one that builds the entry of another mob of the host, and by no other"
    );
    assert_eq!(
        table_readers,
        vec![(
            "unified_runtime/mod.rs".to_string(),
            ENTRY_TABLE_ACCESSOR.to_string()
        )],
        "the table of the other mobs' entries is read by its accessor alone"
    );
    assert!(table_file_read, "the scan reads the file of the table");
    assert!(
        writes_in_place.is_empty(),
        "the file of the table writes a shared entry in place: {writes_in_place:?}"
    );
}
