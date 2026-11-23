//! Source guards of the launch: what the launch's code may not do with its
//! default mob's lifecycle. Each reads the crate's sources from disk, cuts a
//! file's inline test modules, and blanks comments and literals before it
//! matches, so a name in a comment, a doc line or a message is not a hit.

/// The production code of every source of the crate, by its path from
/// `src/`: comments and literals blanked, and the file's inline test
/// modules cut ([`without_test_modules`]).
fn production_code() -> Vec<(String, String)> {
    crate_production_sources()
        .into_iter()
        .map(|(path, source)| {
            let code = String::from_utf8_lossy(&code_only(&source)).into_owned();
            (path, without_test_modules(&code))
        })
        .collect()
}

/// `code` (comments and literals blanked) without its inline test modules:
/// every module declared under `#[cfg(test)]` that has a body is blanked to
/// spaces from the attribute to the brace that closes it, newlines kept, so
/// what is left stands at the same offsets. What stands between two test
/// modules and after the last one is production code and stays.
fn without_test_modules(code: &str) -> String {
    const ATTRIBUTE: &[u8] = b"#[cfg(test)]";
    let bytes = code.as_bytes();
    let mut production = bytes.to_vec();
    let mut from = 0;
    while let Some(attribute) = find_from(bytes, ATTRIBUTE, from) {
        from = attribute + ATTRIBUTE.len();
        // The item the attribute stands on, past any further attributes.
        let mut item = from;
        loop {
            while bytes.get(item).is_some_and(u8::is_ascii_whitespace) {
                item += 1;
            }
            let further = (bytes.get(item) == Some(&b'#'))
                .then(|| find_from(bytes, b"[", item))
                .flatten()
                .and_then(|open| matching_close(bytes, open, b'[', b']'));
            match further {
                Some(after) => item = after,
                None => break,
            }
        }
        let rest = code.get(item..).unwrap_or_default();
        let is_module = ["mod ", "pub mod ", "pub(crate) mod ", "pub(super) mod "]
            .iter()
            .any(|opening| rest.starts_with(opening));
        if !is_module {
            continue;
        }
        // A module with a body opens a brace before any semicolon.
        let body = match (rest.find('{'), rest.find(';')) {
            (Some(open), Some(end)) if open < end => item + open,
            (Some(open), None) => item + open,
            _ => continue,
        };
        let Some(after) = matching_close(bytes, body, b'{', b'}') else {
            continue;
        };
        for byte in &mut production[attribute..after] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        from = after;
    }
    String::from_utf8_lossy(&production).into_owned()
}

/// The paths of the production sources whose code holds one of `needles`,
/// sorted, each with the needles it holds.
fn sources_holding(needles: &[&str]) -> Vec<(String, Vec<String>)> {
    let mut holding: Vec<(String, Vec<String>)> = production_code()
        .into_iter()
        .filter_map(|(path, code)| {
            let held: Vec<String> = needles
                .iter()
                .filter(|needle| code.contains(**needle))
                .map(ToString::to_string)
                .collect();
            (!held.is_empty()).then_some((path, held))
        })
        .collect();
    holding.sort();
    holding
}

/// A mob's phase and the lifecycle intent it still owes are its machine's
/// facts, read from its handle. No production code reads them off the mob's
/// event history: the events that record a Stop (`MobStopped`, the start and
/// the end of a lifecycle quiesce) are named only where the event stream
/// gives every event kind its public name and class.
#[test]
fn no_production_code_reads_a_mobs_phase_off_its_stop_events() {
    let naming_the_events = sources_holding(&[
        "MobStopped",
        "PlacedCompletionLifecycleQuiesceStarted",
        "PlacedCompletionLifecycleQuiesceEnded",
    ]);
    let paths: Vec<&str> = naming_the_events
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["unified_runtime/mob_events.rs"],
        "only the event stream's table of names and classes names a Stop's events: \
         {naming_the_events:?}"
    );
}

/// The lexer pass keeps code and drops what only looks like it: the three
/// event names in a comment, a string and a raw string are no hit, the same
/// name in code is, and a file's inline test modules are cut while the
/// production code between and after them is kept.
#[test]
fn the_guards_read_production_code_only() {
    let source = "// MobStopped in a comment\n\
                  /* MobStopped in a block */\n\
                  const TEXT: &str = \"MobStopped in a string\";\n\
                  const RAW: &str = r#\"MobStopped in a raw string\"#;\n\
                  fn production() {}\n\
                  \n\
                  #[cfg(test)]\n\
                  mod tests {\n\
                  \x20   fn row() { let _ = Kind::MobStopped; }\n\
                  }\n\
                  \n\
                  fn later_production() {}\n\
                  \n\
                  #[cfg(test)]\n\
                  #[allow(clippy::expect_used)]\n\
                  mod more_tests {\n\
                  \x20   fn row() { let _ = Kind::MobStopped; }\n\
                  }\n\
                  \n\
                  #[cfg(test)]\n\
                  fn a_test_only_item() {}\n\
                  fn last_production() {}\n";
    let code = String::from_utf8_lossy(&code_only(source)).into_owned();
    let production = without_test_modules(&code);
    assert_eq!(production.len(), code.len(), "offsets stand");
    assert!(!production.contains("MobStopped"), "{production}");
    for kept in [
        "fn production()",
        "fn later_production()",
        "fn a_test_only_item()",
        "fn last_production()",
    ] {
        assert!(production.contains(kept), "{kept} is kept: {production}");
    }
    let hit = "fn decide(kind: Kind) -> bool { matches!(kind, Kind::MobStopped) }\n";
    assert!(String::from_utf8_lossy(&code_only(hit)).contains("MobStopped"));
}

/// The production functions that build a mob's pre-activation hook
/// (`meerkat_mob::MobBeforeActivation`), by file and signature. Core runs
/// such a hook inside the resume, before the intent the mob's log leaves
/// open is applied to its machine: the hook's read handle reads no owed
/// intent there, and a phase read there is not the phase the resume hands
/// back.
const PRE_ACTIVATION_HOOKS: &[(&str, &str)] = &[("extension_state.rs", "fn before_activation(")];

/// What a pre-activation hook's body may not hold: a read of the intent a
/// mob still owes, the wait on it, and every step of the launch's
/// declaration that it runs its default mob (the resume, the staged
/// activation and its consumption, a decision on the Stopped phase).
const NOT_IN_A_PRE_ACTIVATION_HOOK: &[&str] = &[
    "owed_intent",
    "wait_until_no_lifecycle_intent_is_owed",
    "default_mob_phase_once_no_intent_is_owed",
    "phase_once_no_intent_is_owed_within",
    ".resume(",
    "PendingMobActivation",
    ".activate(",
    "MobState::Stopped",
];

/// No pre-activation hook reads the intent a mob still owes or decides a
/// lift: the launch's declaration runs on the handle the resume returns,
/// never inside the hook. Every production function that builds such a hook
/// is on the list, so a new one is judged too.
#[test]
fn no_pre_activation_hook_reads_an_owed_intent_or_decides_a_lift() {
    let code = production_code();
    let mut builders: Vec<String> = code
        .iter()
        .filter(|(_, code)| code.contains("-> meerkat_mob::MobBeforeActivation"))
        .map(|(path, _)| path.clone())
        .collect();
    builders.sort();
    let mut listed: Vec<String> = PRE_ACTIVATION_HOOKS
        .iter()
        .map(|(path, _)| (*path).to_string())
        .collect();
    listed.sort();
    listed.dedup();
    assert_eq!(
        builders, listed,
        "every function that answers a pre-activation hook is listed"
    );
    for (path, signature) in PRE_ACTIVATION_HOOKS {
        let (_, source) = code
            .iter()
            .find(|(source_path, _)| source_path == path)
            .unwrap_or_else(|| panic!("{path} is a production source"));
        let body = item_body(source, signature);
        assert!(!body.is_empty(), "{path} holds `{signature}`");
        let held: Vec<&str> = NOT_IN_A_PRE_ACTIVATION_HOOK
            .iter()
            .copied()
            .filter(|needle| body.contains(needle))
            .collect();
        assert!(
            held.is_empty(),
            "the pre-activation hook `{signature}` of {path} holds {held:?}"
        );
    }
}

/// The lift of a restored mob is decided in one place: the phase a launch
/// acts on is the one Core's wait answers once no lifecycle intent is owed
/// (`default_mob_phase_once_no_intent_is_owed`, and for a mob preparation
/// just had loaded `default_mob_phase_after_its_load`, which awaits the
/// mob's restore first). The wrapper's production code asks a handle for
/// its phase by `status()` nowhere on the way from the resume to the lift:
/// not in `MobRuntime::prepare`, not in `PendingMobActivation::activate`.
#[test]
fn the_launch_reads_its_mobs_phase_only_from_the_wait_on_owed_intents() {
    let code = production_code();
    let (_, wrapper) = code
        .iter()
        .find(|(path, _)| path == "mob_handle_runtime.rs")
        .expect("the wrapper's source");
    for (signature, wait) in [
        (
            "pub(crate) async fn prepare(",
            "default_mob_phase_after_its_load(",
        ),
        (
            "pub(crate) async fn activate(self)",
            "default_mob_phase_once_no_intent_is_owed(",
        ),
    ] {
        let body = item_body(wrapper, signature);
        assert!(!body.is_empty(), "the wrapper holds `{signature}`");
        assert!(
            !body.contains(".status()"),
            "`{signature}` asks a handle for its phase by status()"
        );
        assert!(
            body.contains(wait),
            "`{signature}` reads the phase from the wait on owed intents (`{wait}`)"
        );
    }
    let after_load = item_body(wrapper, "async fn default_mob_phase_after_its_load(");
    assert!(
        after_load.contains("wait_until_no_lifecycle_intent_is_owed()")
            && !after_load.contains(".status()"),
        "the wait after a load is Core's wait on owed intents"
    );
    assert!(
        after_load.contains("replay_all()"),
        "the wait after a load first awaits a read the mob's actor answers: the bound on an \
         owed intent starts after the mob's restore"
    );
}

/// A launch over a declared store judges the resume of its mob before its
/// mob set exists: in `MobRuntime::prepare` the launch's read of its store
/// (`read_launch_definition()`, which judges the resume from the launch's
/// own open) stands before `settle_before_first_use()`, the step that
/// builds the mob set, and each stands there once. A launch that judgement
/// refuses has then restored no child mob and scheduled no pass of a mob
/// set, on a durable child root and on process-bound child mobs alike.
#[test]
fn the_launch_judges_a_declared_store_before_it_builds_its_mob_set() {
    let code = production_code();
    let (_, wrapper) = code
        .iter()
        .find(|(path, _)| path == "mob_handle_runtime.rs")
        .expect("the wrapper's source");
    let body = item_body(wrapper, "pub(crate) async fn prepare(");
    let (read, settle) = ("read_launch_definition()", "settle_before_first_use()");
    for step in [read, settle] {
        assert_eq!(
            body.matches(step).count(),
            1,
            "`prepare` holds `{step}` once"
        );
    }
    assert!(
        body.find(read) < body.find(settle),
        "`prepare` reads and judges a declared store (`{read}`) before it builds the mob set \
         (`{settle}`)"
    );
}

/// A launch over a declared store declared its mob to its mob set, which
/// builds it: no handle built outside the set is inserted for it. The
/// wrapper inserts a handle into the set, and declares the set complete by
/// hand, in one place only, the registration of a mob it built over an
/// opened storage; and preparation reaches that place once, behind the
/// test that the launch declared no mob.
#[test]
fn no_declared_launch_inserts_a_foreign_handle() {
    let code = production_code();
    let (_, wrapper) = code
        .iter()
        .find(|(path, _)| path == "mob_handle_runtime.rs")
        .expect("the wrapper's source");
    let registration = item_body(wrapper, "async fn register_opened_storage_mob(");
    for call in [".mob_insert_handle(", ".declare_mob_set_restored("] {
        assert!(
            registration.contains(call),
            "the registration of a mob over an opened storage calls `{call}`"
        );
        assert_eq!(
            wrapper.matches(call).count(),
            registration.matches(call).count(),
            "the wrapper calls `{call}` outside the registration of a mob over an opened storage"
        );
    }
    let prepare = item_body(wrapper, "pub(crate) async fn prepare(");
    let calls: Vec<usize> = prepare
        .match_indices("register_opened_storage_mob(")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "preparation registers a mob over an opened storage in one place"
    );
    let condition = prepare[..calls[0]]
        .rfind("if ")
        .map(|at| &prepare[at..calls[0]])
        .unwrap_or_default();
    assert!(
        condition.starts_with("if declared_set.is_none()"),
        "the registration stands behind the test that the launch declared no mob, found \
         `{}`",
        condition.split_whitespace().collect::<Vec<_>>().join(" ")
    );
}

/// The guards read a file's production code wherever it stands, also past
/// the file's first inline test module: the stdio gateway keeps its first
/// test module near its top and most of its production code below it.
#[test]
fn the_guards_read_a_files_production_code_past_its_first_test_module() {
    let code = production_code();
    let (_, gateway) = code
        .iter()
        .find(|(path, _)| path == "rpc_gateway.rs")
        .expect("the stdio gateway's source");
    assert!(
        gateway.contains("fn gateway_bootstrap_init_error("),
        "the guards read the stdio gateway's init, which stands below its first test module"
    );
}

/// The stdio gateway opens its launch mob's store itself, in one place, as
/// every release did, and builds the launch's spec over a declared store:
/// the storage it opened becomes the spec's own only behind the test that
/// the launch declares a spec update (which is committed through it).
#[test]
fn the_stdio_gateway_builds_a_spec_over_the_storage_it_opened_only_for_a_declared_update() {
    let code = production_code();
    let (_, gateway) = code
        .iter()
        .find(|(path, _)| path == "rpc_gateway.rs")
        .expect("the stdio gateway's source");
    assert!(
        gateway.contains("MobBootstrapSpec::declared("),
        "the stdio gateway builds its launch's spec over a declared store"
    );
    assert_eq!(
        gateway.matches("persistent_mob_storage(").count(),
        1,
        "the stdio gateway opens a persistent mob storage in one place"
    );
    let kept: Vec<usize> = gateway
        .match_indices("LaunchMobStore::Opened(pair")
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "the storage the stdio gateway opened is kept as the spec's own in one place"
    );
    let condition = gateway[..kept[0]]
        .rfind("if ")
        .map(|at| &gateway[at..kept[0]])
        .unwrap_or_default();
    assert!(
        condition.starts_with("if gateway_options.mob.declare_spec_update.is_some()"),
        "the opened storage is the spec's own only for a launch that declares a spec update, \
         found `{}`",
        condition.split_whitespace().collect::<Vec<_>>().join(" ")
    );
}

/// The console gateway opens its launch mob's store itself once, as every
/// release did, closes it again at once, and says where the store is on
/// both of its launches (the one over a state directory and the one that
/// keeps its mob in memory): its production code builds a launch's spec
/// over a declared store only, and keeps no mob storage of its own.
#[test]
fn the_console_gateway_opens_its_launch_store_once_and_declares_it() {
    let code = production_code();
    let (_, gateway) = code
        .iter()
        .find(|(path, _)| path == "bin/meta_gateway.rs")
        .expect("the console gateway's source");
    assert_eq!(
        gateway.matches("MobBootstrapSpec::declared(").count(),
        2,
        "the console gateway builds the spec of each of its two launches over a declared store"
    );
    assert_eq!(
        gateway.matches("persistent_mob_storage(").count(),
        1,
        "the console gateway opens a persistent mob storage in one place"
    );
    let unspaced: String = gateway.split_whitespace().collect();
    assert!(
        unspaced.contains("drop(meerkat_meta::mob_composition_manifest::persistent_mob_storage("),
        "the console gateway closes the storage it opened where it opened it"
    );
    for keeping in ["MobStorage::", "MobBootstrapSpec::new("] {
        assert!(
            !gateway.contains(keeping),
            "the console gateway keeps no mob storage for its launch, found `{keeping}`"
        );
    }
}

/// The stdio gateway's launch without a state directory declares its mob's
/// store too, a store in memory: the gateway's production code names no
/// mob storage in memory of its own, and builds a spec over an opened
/// storage in one place only, the launch that declares a spec update.
#[test]
fn the_stdio_gateway_declares_the_store_of_a_launch_without_a_state_directory() {
    let code = production_code();
    let (_, gateway) = code
        .iter()
        .find(|(path, _)| path == "rpc_gateway.rs")
        .expect("the stdio gateway's source");
    assert!(
        !gateway.contains("MobStorage::in_memory()"),
        "the stdio gateway opens no mob storage in memory for a launch"
    );
    assert_eq!(
        gateway.matches("MobBootstrapSpec::new(").count(),
        1,
        "the stdio gateway builds a spec over an opened storage in one place"
    );
    assert_eq!(
        gateway.matches("MobBootstrapSpec::declared(").count(),
        2,
        "the stdio gateway builds the spec of a launch with a state directory and of one \
         without over a declared store"
    );
}

/// A launch store that opens and cannot then be read is answered as it was
/// when every launch opened its store itself: by the read's own failure,
/// as a failed bootstrap. Nothing in production code reads a store fault
/// off a failed read to answer it as a store that could not be opened:
/// the names that did are gone.
#[test]
fn no_gateway_answers_a_failed_read_of_its_launch_store_as_a_failed_open() {
    let found = sources_holding(&[
        "store_fault(",
        "launch_definition_read_init_error(",
        "read_declared_launch_store(",
    ]);
    assert!(
        found.is_empty(),
        "production code answers a failed read of a launch store as a failed open: {found:?}"
    );
}

/// Every production source of this crate: each `.rs` file under `src/` (its
/// binaries included), by its path from `src/`, except the test files
/// (`*_tests.rs`, `tests.rs`, and the files of a `tests` or `*_tests`
/// directory). A file added later is read too.
fn crate_production_sources() -> Vec<(String, String)> {
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
            if !name.ends_with(".rs") || name == "tests.rs" || name.ends_with("_tests.rs") {
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

/// The body of the item whose signature starts with `signature` in `code`
/// (comments and literals blanked): from its first `{` to the matching `}`.
/// Empty when the item is absent.
fn item_body<'a>(code: &'a str, signature: &str) -> &'a str {
    let bytes = code.as_bytes();
    code.find(signature)
        .and_then(|start| find_from(bytes, b"{", start))
        .and_then(|open| matching_close(bytes, open, b'{', b'}').map(|close| &code[open..close]))
        .unwrap_or_default()
}

/// `source` with every comment and every string, raw string and character
/// literal blanked to spaces (newlines kept), byte for byte: what is left is
/// the code, at the same offsets.
fn code_only(source: &str) -> Vec<u8> {
    let bytes = source.as_bytes();
    let mut code = bytes.to_vec();
    let mut at = 0;
    while at < bytes.len() {
        let end = match bytes[at] {
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                find_from(bytes, b"\n", at).unwrap_or(bytes.len())
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => block_comment_end(bytes, at),
            b'"' => string_end(bytes, at),
            b'r' | b'b' => match raw_string_end(bytes, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            b'\'' => match char_literal_end(source, at) {
                Some(end) => end,
                None => {
                    at += 1;
                    continue;
                }
            },
            _ => {
                at += 1;
                continue;
            }
        };
        let end = end.min(bytes.len());
        for byte in &mut code[at..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    code
}

/// The first position of `needle` in `haystack` at or after `from`.
fn find_from(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| from + position)
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
/// `r#type`, or an identifier that ends in `r` or `b`.
fn raw_string_end(bytes: &[u8], at: usize) -> Option<usize> {
    if at
        .checked_sub(1)
        .and_then(|before| bytes.get(before))
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
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
    let hashes = bytes
        .get(quote..)?
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    quote += hashes;
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let mut closing = vec![b'"'];
    closing.extend(std::iter::repeat_n(b'#', hashes));
    Some(find_from(bytes, &closing, quote + 1).map_or(bytes.len(), |close| close + closing.len()))
}

/// Where the character literal opening at `at` ends, or `None` when the
/// quote opens a lifetime or a label.
fn char_literal_end(source: &str, at: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(at + 1) == Some(&b'\\') {
        // An escape: the literal ends at the first quote after the escaped
        // character.
        return find_from(bytes, b"'", at + 3).map(|close| close + 1);
    }
    let width = source.get(at + 1..)?.chars().next()?.len_utf8();
    (bytes.get(at + 1 + width) == Some(&b'\'')).then_some(at + 2 + width)
}

/// The position just after the `close` that matches the `open` at `at`.
fn matching_close(code: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    if code.get(at) != Some(&open) {
        return None;
    }
    let mut depth = 0_usize;
    for (offset, byte) in code.get(at..)?.iter().enumerate() {
        if *byte == open {
            depth += 1;
        } else if *byte == close {
            depth -= 1;
            if depth == 0 {
                return Some(at + offset + 1);
            }
        }
    }
    None
}
