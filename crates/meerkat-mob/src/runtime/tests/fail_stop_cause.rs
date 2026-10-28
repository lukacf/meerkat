//! An actor that fail-stops on an uncertain durable mutation records that
//! mutation's own error: status, resume and the lifecycle observation report
//! a `MobError::DurabilityFailStop` that carries it, and the command that hit
//! it gets the same error. One row per kind of fail-stop site, then the
//! source guard that keeps every site on the one record.

use super::*;

/// The record a fail-stopped mob reports: `status()` and `resume()` report it
/// shared, and the lifecycle observation carries the same record.
async fn recorded_fail_stop(handle: &MobHandle) -> Arc<MobError> {
    wait_for_actor_fail_stop(handle).await;
    let record = match handle.status().await {
        Err(MobError::SharedLifecycleFailure(record)) => record,
        other => panic!("a fail-stopped mob reports why, typed: {other:?}"),
    };
    match handle.resume().await {
        Err(MobError::SharedLifecycleFailure(resumed)) => assert!(
            Arc::ptr_eq(&resumed, &record),
            "resume reports the record status reports: {resumed:?} vs {record:?}"
        ),
        other => panic!("resume reports the record too: {other:?}"),
    }
    let observed = handle
        .lifecycle_observation()
        .failure
        .unwrap_or_else(|| panic!("the observation carries the record: {record:?}"));
    assert!(
        Arc::ptr_eq(&observed, &record),
        "the observation carries the record status reports: {observed:?} vs {record:?}"
    );
    record
}

/// The cause a durability fail-stop record of this mob carries.
fn cause_of<'a>(handle: &MobHandle, record: &'a MobError) -> &'a Arc<MobError> {
    match record {
        MobError::DurabilityFailStop { mob_id, cause } => {
            assert_eq!(mob_id, handle.mob_id(), "the record names its mob");
            cause
        }
        other => panic!("an in-loop fail-stop records its own cause: {other:?}"),
    }
}

/// `reply`, the error the command that hit the fail-stop got, is the
/// recorded cause itself, shared, so its Display and wire code are the
/// cause's own.
fn assert_replied_with(reply: &MobError, cause: &Arc<MobError>) {
    let MobError::SharedLifecycleFailure(replied) = reply else {
        panic!("the command that hit the fail-stop gets the recorded cause, shared: {reply:?}");
    };
    assert!(
        Arc::ptr_eq(replied, cause),
        "the reply carries the recorded cause: {replied:?} vs {cause:?}"
    );
    assert_eq!(reply.to_string(), cause.to_string());
    assert_eq!(reply.wire_error_code(), cause.wire_error_code());
    assert_eq!(reply.failure_class(), cause.failure_class());
}

/// What `mob/list` lists for the record: its cause and code are those of an
/// admission closed for a durability fail-stop, and its message names the
/// site's own cause.
fn assert_listed_as_the_close(handle: &MobHandle, record: &MobError, cause: &MobError) {
    let close = MobError::AdmissionClosed {
        mob_id: handle.mob_id().clone(),
        reason: crate::runtime::MobAdmissionCloseReason::DurabilityFailStop,
    };
    assert_eq!(
        crate::mob_error_wire_code(record),
        crate::mob_error_wire_code(&close),
        "the listed cause is the close's"
    );
    assert_eq!(
        record.wire_error_code(),
        close.wire_error_code(),
        "the listed code is the close's"
    );
    assert!(
        record.to_string().contains(&cause.to_string()),
        "the listed message names the site's cause: {record}"
    );
}

/// A site that builds its error and returns it (a supervisor rotation whose
/// pending record's exact reread stayed unreadable) records that error, and
/// the rotation's caller gets it.
#[tokio::test]
async fn a_returned_cause_is_recorded_and_replied() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let runtime_metadata = Arc::new(FaultInjectedRuntimeMetadataStore::new());
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "fail-stop-cause-returned"),
        MobStorage::with_events_and_runtime_metadata(
            Arc::new(InMemoryMobEventStore::new()),
            runtime_metadata.clone(),
        ),
    )
    .with_session_service(service)
    .create()
    .await
    .expect("create mob");

    // The first compare is the pending rotation's record.
    runtime_metadata.lose_nth_compare_supervisor_completion_with_unreadable_reread(1);
    let reply = handle
        .rotate_supervisor()
        .await
        .expect_err("an unreadable reread fail-stops the actor");
    let record = recorded_fail_stop(&handle).await;
    let cause = cause_of(&handle, &record);
    assert!(
        matches!(cause.as_ref(), MobError::Internal(message)
            if message.contains("persist_pending_supervisor_rotation")
                && message.contains("exact reread remained unreadable")),
        "the record carries the rotation's own error: {cause:?}"
    );
    assert_replied_with(&reply, cause);
    assert_listed_as_the_close(&handle, &record, cause);
    crash_stop_and_release_routes(handle).await;
}

/// A site that forwards an error it did not build (a flow's terminal
/// commit whose event append failed after the run's terminal status
/// persisted) records that error as it is: the store's own typed error.
#[tokio::test]
async fn a_forwarded_cause_is_recorded_as_it_is() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    events.fail_appends_for("FlowCompleted").await;
    let (handle, _service, runs, _specs, _runtime_metadata, _identity, _identity_status) =
        create_test_mob_with_recoverable_fault_events(
            with_unique_mob_id(
                sample_definition_with_single_step_flow(500, 8),
                "fail-stop-cause-forwarded",
            ),
            events,
        )
        .await;
    handle
        .spawn(
            ProfileName::from("worker"),
            AgentIdentity::from("w-1"),
            None,
        )
        .await
        .expect("spawn worker");
    let run_id = handle
        .run_flow(FlowId::from("demo"), serde_json::json!({}))
        .await
        .expect("run flow");
    wait_for_run_terminal_in_store(runs.as_ref(), &run_id, Duration::from_secs(3)).await;

    let record = recorded_fail_stop(&handle).await;
    let cause = cause_of(&handle, &record);
    assert!(
        matches!(cause.as_ref(), MobError::StorageError(_)),
        "the record carries the store's own typed error: {cause:?}"
    );
    let rendered = cause.to_string();
    assert!(
        rendered.contains("terminal event append failed") && rendered.contains("FlowCompleted"),
        "the record names the terminal commit's failed append: {rendered}"
    );
}

/// A site that logs a typed error its command answers later (a spawn whose
/// external member's cleanup is uncertain) records that error with its own
/// variant, and the spawn's caller still gets it bare.
#[tokio::test]
async fn a_logged_typed_cause_keeps_its_variant() {
    let _serial = lock_real_comms_tests();
    let definition = with_unique_mob_id(
        sample_definition_with_external_backend(),
        "fail-stop-cause-logged",
    );
    let mob_id = definition.id.clone();
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    let external = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "w-ext")).await;

    external.garble_next_bind_reply();
    let reply = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            AgentIdentity::from("w-ext"),
            None,
            external.binding(),
        )
        .await
        .expect_err("an undecodable post-commit bind reply is uncertain");
    let MobError::ExternalMemberCleanupUncertain { reason } = &reply else {
        panic!("the spawn's caller gets the typed error, bare: {reply:?}");
    };
    let record = recorded_fail_stop(&handle).await;
    match cause_of(&handle, &record).as_ref() {
        MobError::ExternalMemberCleanupUncertain { reason: recorded } => {
            assert_eq!(recorded, reason, "the record carries the spawn's own error");
        }
        other => panic!("the record keeps the typed cause's variant: {other:?}"),
    }
}

/// A quarantine helper (a BindHost reply that names another host) records
/// the error it builds and returns, and the bind's caller gets it.
#[tokio::test]
async fn a_quarantine_helpers_cause_is_recorded_and_replied() {
    let _serial = lock_real_comms_tests();
    let (handle, _mob_id, _runtime_metadata) =
        create_host_ceremony_mob("fail-stop-cause-quarantine").await;
    let stub = spawn_host_daemon_stub("fail-stop-cause-quarantine", None).await;
    *stub.bind_peer_id_override.write().await = Some("host-imposter".to_string());

    let reply = handle
        .bind_host(stub.bind_request())
        .await
        .expect_err("a reply that names another host fails the ceremony");
    let record = recorded_fail_stop(&handle).await;
    let cause = cause_of(&handle, &record);
    assert!(
        matches!(cause.as_ref(), MobError::Internal(message)
            if message.contains("BindHost outcome") && message.contains("identity mismatch")),
        "the record carries the quarantine's own error: {cause:?}"
    );
    assert_replied_with(&reply, cause);
}

// ---------------------------------------------------------------------------
// The source guard: every fail-stop site goes through the one record, and
// only the loop exit builds the record variant.
// ---------------------------------------------------------------------------

/// The field that holds the first uncertain mutation's cause, in the mob
/// actor and in the mob host actor alike.
const FAIL_STOP_FIELD: &str = "durable_uncertainty_fail_stop";

/// What may follow the field: its record methods, its reads, and the `:` of
/// its declaration and initialization.
const FAIL_STOP_FIELD_USES: &[&str] =
    &[".record(", ".record_unreplied(", ".is_set(", ".cause(", ":"];

/// The one function that builds the record, in `runtime/actor.rs`.
const RECORD_BUILDER: &str = "fn record_durability_fail_stop";

/// The crate's shipped sources: every `.rs` file under `src/` outside test
/// files, by path relative to `src/`, with comments and literals blanked and
/// test-only items removed.
fn shipped_sources() -> Vec<(String, String)> {
    // Read when the test runs: cargo, nextest (also from an archive) and the
    // Bazel test rule set CARGO_MANIFEST_DIR for the test process, so an
    // archived test binary never carries the builder's path.
    let root = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    )
    .join("src");
    let mut pending = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read source directory") {
            let path = entry.expect("source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| {
            let relative = path
                .strip_prefix(&root)
                .expect("under src")
                .to_string_lossy()
                .replace('\\', "/");
            let stem = relative.rsplit('/').next()?.trim_end_matches(".rs");
            let test_file = stem == "tests"
                || stem.ends_with("_tests")
                || relative.split('/').any(|part| part == "tests");
            if test_file {
                return None;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            Some((relative, shipped_code(&source)))
        })
        .collect()
}

/// The 1-based line of byte `index` in `code`.
fn line_of(code: &str, index: usize) -> usize {
    code[..index].matches('\n').count() + 1
}

/// Whether `name` occurs in `code` at `at` as a whole identifier.
pub(super) fn whole_identifier(code: &str, at: usize, name: &str) -> bool {
    let bytes = code.as_bytes();
    let before = at.checked_sub(1).map(|index| bytes[index]);
    let after = bytes.get(at + name.len()).copied();
    let identifier = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    !before.is_some_and(identifier) && !after.is_some_and(identifier)
}

/// The uses of the fail-stop field in `code` that are not one of
/// [`FAIL_STOP_FIELD_USES`]: a write, a bare read or a borrow of the field
/// itself, as `line: text`.
fn fail_stop_field_misuses(code: &str) -> Vec<String> {
    code.match_indices(FAIL_STOP_FIELD)
        .filter(|(at, name)| whole_identifier(code, *at, name))
        .filter_map(|(at, name)| {
            let rest = code[at + name.len()..].trim_start();
            (!FAIL_STOP_FIELD_USES
                .iter()
                .any(|use_| rest.starts_with(use_)))
            .then(|| {
                let line_start = code[..at].rfind('\n').map_or(0, |index| index + 1);
                let line_end = code[at..].find('\n').map_or(code.len(), |index| at + index);
                format!(
                    "{}: {}",
                    line_of(code, at),
                    code[line_start..line_end].trim()
                )
            })
        })
        .collect()
}

/// The byte index just past the brace group that opens at `open`.
fn brace_group_end(code: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (offset, byte) in code.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open + offset + 1;
                }
            }
            _ => {}
        }
    }
    code.len()
}

/// The constructions of the record variant `DurabilityFailStop { .. }` in
/// `code`, as byte indices. A pattern is not one: its braces end in `..`, or
/// are followed by `=>`, `|` or the `=` of a `let`. The declaration (the
/// variant name without a path) is not one either.
fn record_constructions(code: &str) -> Vec<usize> {
    let name = "DurabilityFailStop";
    code.match_indices(name)
        .filter(|(at, name)| whole_identifier(code, *at, name))
        .filter_map(|(at, name)| {
            if !code[..at].trim_end().ends_with("::") {
                return None;
            }
            let after = &code[at + name.len()..];
            let open = at + name.len() + (after.len() - after.trim_start().len());
            if code.as_bytes().get(open) != Some(&b'{') {
                return None;
            }
            let end = brace_group_end(code, open);
            let fields = code[open + 1..end.saturating_sub(1)].trim();
            let next = code[end..].trim_start();
            let pattern = fields.ends_with("..")
                || next.starts_with("=>")
                || next.starts_with('|')
                || (next.starts_with('=') && !next.starts_with("=="));
            (!pattern).then_some(at)
        })
        .collect()
}

/// The byte range of the body of the function `signature` starts in `code`.
pub(super) fn function_body(code: &str, signature: &str) -> Option<std::ops::Range<usize>> {
    let start = code.find(signature)?;
    let open = start + code[start..].find('{')?;
    Some(open..brace_group_end(code, open))
}

#[test]
fn an_uncertain_durable_mutation_fail_stops_only_through_its_record() {
    let mut misuses = Vec::new();
    for (path, code) in shipped_sources() {
        misuses.extend(
            fail_stop_field_misuses(&code)
                .into_iter()
                .map(|misuse| format!("{path}:{misuse}")),
        );
    }
    assert!(
        misuses.is_empty(),
        "{} use(s) of the fail-stop field bypass its record (a site records its cause with \
         the field's record or record_unreplied):\n{}",
        misuses.len(),
        misuses
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn only_the_loop_exit_builds_the_durability_fail_stop_record() {
    let mut inside = 0usize;
    let mut outside = Vec::new();
    for (path, code) in shipped_sources() {
        let builder = if path == "runtime/actor.rs" {
            function_body(&code, RECORD_BUILDER)
        } else {
            None
        };
        for at in record_constructions(&code) {
            if builder.as_ref().is_some_and(|body| body.contains(&at)) {
                inside += 1;
            } else {
                outside.push(format!("{path}:{}", line_of(&code, at)));
            }
        }
    }
    assert!(
        outside.is_empty(),
        "the durability fail-stop record is built outside the loop exit's record: {outside:?}"
    );
    assert_eq!(
        inside, 1,
        "the loop exit's record ({RECORD_BUILDER}) builds the record exactly once"
    );
}

#[test]
fn the_fail_stop_scans_see_writes_reads_and_constructions() {
    let code = shipped_code(concat!(
        "fn write(&mut self) { self.durable_uncertainty_fail_stop = true; }\n",
        "fn read(&self) -> bool { self.durable_uncertainty_fail_stop }\n",
        "fn borrow(&mut self) { take(&mut self.durable_uncertainty_fail_stop); }\n",
        "fn record(&mut self, e: MobError) -> MobError { self.durable_uncertainty_fail_stop.record(e) }\n",
        "fn set(&self) -> bool { self.durable_uncertainty_fail_stop.is_set() }\n",
        "struct Actor { durable_uncertainty_fail_stop: Record, durable_uncertainty_fail_stop_count: u8 }\n",
        "// self.durable_uncertainty_fail_stop = true;\n",
        "#[cfg(test)]\nfn seam(&mut self) { self.durable_uncertainty_fail_stop = true; }\n",
        "fn build(c: Arc<MobError>) -> MobError { MobError::DurabilityFailStop { mob_id: id(), cause: c } }\n",
        "fn class(e: &MobError) -> u8 { match e { MobError::DurabilityFailStop { .. } => 1, Self::DurabilityFailStop { mob_id, cause } | _ => 2 } }\n",
        "fn peek(e: &MobError) { if let MobError::DurabilityFailStop { cause, .. } = e {} }\n",
        "fn reason() -> R { MobAdmissionCloseReason::DurabilityFailStop }\n",
        "enum MobError { DurabilityFailStop { mob_id: MobId, cause: Arc<MobError> } }\n",
    ));
    let misuses = fail_stop_field_misuses(&code);
    assert_eq!(
        misuses.len(),
        3,
        "the write, the read and the borrow: {misuses:?}"
    );
    assert!(misuses[0].starts_with("1: "), "{misuses:?}");
    assert!(misuses[1].starts_with("2: "), "{misuses:?}");
    assert!(misuses[2].starts_with("3: "), "{misuses:?}");
    let constructions = record_constructions(&code);
    assert_eq!(
        constructions
            .iter()
            .map(|at| line_of(&code, *at))
            .collect::<Vec<_>>(),
        vec![10],
        "only the construction, not the patterns, the close reason or the declaration"
    );
    let body = function_body(&code, "fn build").expect("the builder");
    assert!(body.contains(&constructions[0]));
}

// The comment and literal blanking and test-item removal below are the
// facade's host launch scan's (the host_launch_sites test of the meerkat
// crate).

/// `source` with comments and literals blanked and every test-only item
/// (`#[cfg(test)]`, `#[cfg(all(test, ..))]`, `#[test]` and `#[*::test]`)
/// removed, so only code a shipped build compiles remains.
pub(super) fn shipped_code(source: &str) -> String {
    let code = blank_comments_and_literals(source);
    let bytes = code.as_bytes();
    let mut out = String::with_capacity(code.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#'
            && let Some((attribute, end)) = attribute_at(&code, index)
        {
            if gates_test_only(attribute) {
                let skipped_end = item_end(bytes, end);
                // Keep the line count, so a reported line is the source's.
                out.extend(code[index..skipped_end].chars().filter(|c| *c == '\n'));
                index = skipped_end;
                continue;
            }
            out.push_str(&code[index..end]);
            index = end;
            continue;
        }
        let Some(character) = code[index..].chars().next() else {
            break;
        };
        out.push(character);
        index += character.len_utf8();
    }
    out
}

/// The attribute starting at `start` (`#[...]` or `#![...]`) and the index
/// after it.
fn attribute_at(code: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = code.as_bytes();
    let mut index = start + 1;
    if bytes.get(index) == Some(&b'!') {
        index += 1;
    }
    if bytes.get(index) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&code[start..=index], index + 1));
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// Whether an attribute makes its item test-only.
fn gates_test_only(attribute: &str) -> bool {
    let compact: String = attribute.chars().filter(|c| !c.is_whitespace()).collect();
    compact == "#[test]"
        || (compact.starts_with("#[") && compact.ends_with("::test]"))
        || (compact.contains("::test(") && !compact.starts_with("#[cfg"))
        || compact == "#[cfg(test)]"
        || compact.starts_with("#[cfg(all(test,")
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test,"))
        || (compact.starts_with("#[cfg(all(") && compact.contains(",test))"))
}

/// The index after the item that starts at or after `start`: past any
/// further attributes, through its matching closing brace, or its `;` when
/// it has no body. A match arm, field, variant or argument (anything that
/// does not start with an item or statement keyword) also ends at its `,`,
/// and any item ends where the construct enclosing it closes, so the code
/// after it is scanned.
fn item_end(bytes: &[u8], start: usize) -> usize {
    let separated = !starts_with_item_keyword(bytes, start);
    let mut index = start;
    let mut depth = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'(' | b'[' => depth += 1,
            closing @ (b'}' | b')' | b']') => {
                if depth == 0 {
                    return index;
                }
                depth -= 1;
                if depth == 0 && closing == b'}' {
                    return index + 1;
                }
            }
            b';' if depth == 0 => return index + 1,
            b',' if depth == 0 && separated => return index + 1,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

/// Whether the code at `start`, past whitespace and attributes, starts with
/// an item or statement keyword (whose `,` at the top level, as in generic
/// parameters, does not end it).
fn starts_with_item_keyword(bytes: &[u8], start: usize) -> bool {
    const KEYWORDS: &[&str] = &[
        "async",
        "const",
        "enum",
        "extern",
        "fn",
        "impl",
        "let",
        "macro_rules",
        "mod",
        "pub",
        "static",
        "struct",
        "trait",
        "type",
        "union",
        "unsafe",
        "use",
    ];
    let mut index = start;
    loop {
        while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        if bytes.get(index) != Some(&b'#') {
            break;
        }
        let mut depth = 0usize;
        while let Some(&byte) = bytes.get(index) {
            index += 1;
            match byte {
                b'[' => depth += 1,
                b']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    let word_end = bytes[index.min(bytes.len())..]
        .iter()
        .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .map_or(bytes.len(), |offset| index + offset);
    let word = std::str::from_utf8(&bytes[index.min(bytes.len())..word_end]).unwrap_or("");
    KEYWORDS.contains(&word)
}

/// `source` with every comment and string or character literal replaced by
/// spaces (newlines kept).
fn blank_comments_and_literals(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    let blank = |out: &mut String, character: char| {
        out.push(if character == '\n' { '\n' } else { ' ' });
    };
    while index < chars.len() {
        let character = chars[index];
        let next = chars.get(index + 1).copied();
        if character == '/' && next == Some('/') {
            while index < chars.len() && chars[index] != '\n' {
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        if character == '/' && next == Some('*') {
            let mut depth = 0usize;
            while index < chars.len() {
                if chars[index] == '/' && chars.get(index + 1) == Some(&'*') {
                    depth += 1;
                    blank(&mut out, chars[index]);
                    blank(&mut out, '*');
                    index += 2;
                    continue;
                }
                if chars[index] == '*' && chars.get(index + 1) == Some(&'/') {
                    depth -= 1;
                    blank(&mut out, '*');
                    blank(&mut out, '/');
                    index += 2;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                blank(&mut out, chars[index]);
                index += 1;
            }
            continue;
        }
        // Raw strings: r"..", r#".."#, br#".."#.
        let raw_start = match (character, next) {
            ('r', Some('"' | '#')) => Some(index + 1),
            ('b', Some('r')) if matches!(chars.get(index + 2), Some('"' | '#')) => Some(index + 2),
            _ => None,
        };
        if let Some(mut cursor) = raw_start
            && !identifier_continues(&chars, index)
        {
            let mut hashes = 0usize;
            while chars.get(cursor) == Some(&'#') {
                hashes += 1;
                cursor += 1;
            }
            if chars.get(cursor) == Some(&'"') {
                cursor += 1;
                while cursor < chars.len() {
                    if chars[cursor] == '"'
                        && (0..hashes).all(|offset| chars.get(cursor + 1 + offset) == Some(&'#'))
                    {
                        cursor += 1 + hashes;
                        break;
                    }
                    cursor += 1;
                }
                for &skipped in &chars[index..cursor.min(chars.len())] {
                    blank(&mut out, skipped);
                }
                index = cursor;
                continue;
            }
        }
        if character == '"' {
            blank(&mut out, character);
            index += 1;
            while index < chars.len() {
                let current = chars[index];
                blank(&mut out, current);
                index += 1;
                if current == '\\' {
                    if index < chars.len() {
                        blank(&mut out, chars[index]);
                        index += 1;
                    }
                } else if current == '"' {
                    break;
                }
            }
            continue;
        }
        if character == '\'' {
            // A character literal ('x', '\n', '\u{..}'); a lifetime has no
            // closing quote right after one character or escape.
            let end = if next == Some('\\') {
                chars
                    .get(index + 3..)
                    .and_then(|rest| rest.iter().position(|&c| c == '\''))
                    .map(|offset| index + 3 + offset)
            } else if chars.get(index + 2) == Some(&'\'') {
                Some(index + 2)
            } else {
                None
            };
            if let Some(end) = end {
                for &skipped in &chars[index..=end] {
                    blank(&mut out, skipped);
                }
                index = end + 1;
                continue;
            }
        }
        out.push(character);
        index += 1;
    }
    out
}

/// Whether the character before `index` continues an identifier, so an `r`
/// or `b` there is not a literal prefix.
fn identifier_continues(chars: &[char], index: usize) -> bool {
    index > 0 && (chars[index - 1].is_alphanumeric() || chars[index - 1] == '_')
}
