//! The no-wait rule at this crate's own waiting sites.
//!
//! Meta and Core call into this crate from inside their protected sections
//! (`meerkat_core::protected_section`): the governed bind, the native
//! admission readers, the historical admission callback, the store
//! transaction. Code there never takes a lock another actor may hold across
//! I/O, never spins and never sleeps. Each such site in this crate names
//! itself here before it may wait: inside a protected section that is a
//! violation, which a debug build reports.
//!
//! [`KNOWN_SITES`] lists the sites that still wait inside a protected
//! section, each with what it waits for, the change that removes it and one
//! test that reaches it there. A listed site is not
//! reported. The list only shrinks: the tests fail for a listed site that no
//! longer names itself in this crate, and a debug build reports a site that
//! waits inside a section without being listed, at the first row that
//! reaches it there.

/// A site that still waits inside a protected section.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "the baseline's own tests read its description")
)]
pub(crate) struct KnownSite {
    /// The site, as its wait names itself.
    pub(crate) site: &'static str,
    /// What it waits for.
    pub(crate) primitive: &'static str,
    /// The change that removes it.
    pub(crate) owner: &'static str,
    /// One test that reaches it inside a section.
    pub(crate) test: &'static str,
}

/// Sites that still wait inside a protected section. Every one names the
/// later change that prepares outside protection what these sites now wait for
/// inside it.
pub(crate) const KNOWN_SITES: &[KnownSite] = &[
    KnownSite {
        site: "custody_recovery.enroll",
        primitive: "the recovery control lock (a std mutex), then the publication writer",
        owner: "the send enrolls its recovery custody before its bind (governed bind; \
            post-write: the first custody attempt is written first)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "custody_recovery.cold_publish",
        primitive: "the recovery control lock (a std mutex), then the publication writer",
        owner: "a restored original is published without the recovery control lock (historical \
            admission callback)",
        test: "private_originals::governed_submit::tests::first_input_tests::persistent::custody_recovery_cold_native_tests::installed_promoted_attempt_restores_from_same_native_input_without_resubmission",
    },
    KnownSite {
        site: "original_ingress.wait_for_lock",
        primitive: "a bounded try-lock spin with sleeps on a std mutex",
        owner: "native admission reads answer from published custody without spinning (native \
            admission reads)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.admitted_guard",
        primitive: "the originals collection (a std mutex), taken blocking",
        owner: "the admitted-custody check reads published custody; first sends, the Console \
            delegate and the peer bind are prepared outside protection",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.admitted_guard.change",
        primitive: "the publication writer (begin_owner_change)",
        owner: "as original_ingress.admitted_guard (its slow path takes the publication)",
        test: "private_originals::governed_submit::tests::first_input_tests::first_send_not_now::a_first_send_held_inside_an_owner_change_at_the_admitted_check_waits_as_shipped",
    },
    KnownSite {
        site: "original_ingress.admitted_guard.recheck",
        primitive: "the originals collection (a std mutex), taken blocking",
        owner: "as original_ingress.admitted_guard (its slow path rereads the collection)",
        test: "private_originals::governed_submit::tests::first_input_tests::first_send_not_now::a_first_send_held_inside_an_owner_change_at_the_admitted_check_waits_as_shipped",
    },
    KnownSite {
        site: "original_ingress.source_writer",
        primitive: "the source writer (a tokio mutex)",
        owner: "the first binding (controller grant and binding record) is written before the \
            send, outside protection",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.first_binding_write",
        primitive: "the publication writer (begin_owner_change)",
        owner: "as original_ingress.source_writer (the binding record's retention)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.publish_binding",
        primitive: "the publication writer (begin_owner_change)",
        owner: "as original_ingress.source_writer (the binding's publication)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.record_candidate",
        primitive: "the publication writer (begin_owner_change)",
        owner: "the candidate is staged with the recovery enrollment, before the bind",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.rebind_candidate",
        primitive: "the publication writer (begin_owner_change)",
        owner: "as original_ingress.record_candidate (a replay's rebind of its candidate)",
        test: "private_originals::setup_workflow::launch::tests::replay_tests::repeated_policy_change_stops_after_one_replay",
    },
    KnownSite {
        site: "original_ingress.enroll_settlement",
        primitive: "the publication writer (begin_owner_change)",
        owner: "as original_ingress.record_candidate (the candidate's settlement enrollment)",
        test: "private_originals::setup_workflow::launch::tests::replay_tests::deadline_during_the_writer_wait_never_replays",
    },
    KnownSite {
        site: "original_ingress.admit",
        primitive: "the publication writer (begin_owner_change)",
        owner: "an accepted candidate is promoted by its custody machine, after the entry \
            (governed entry settlement)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
    KnownSite {
        site: "original_ingress.withdraw_candidate",
        primitive: "the publication writer (begin_owner_change)",
        owner: "a refused candidate is withdrawn by its custody machine, outside protection",
        test: "private_originals::governed_submit::tests::custody_recovery_tests::a_candidate_the_bind_cannot_enroll_is_withdrawn_inside_the_bind",
    },
    KnownSite {
        site: "launch.workflow_accepted",
        primitive: "the publication writer (begin_owner_change)",
        owner: "the launch records its acceptance through its machine, after the entry \
            (governed entry settlement)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_the_admitted_check_waits_as_shipped_and_starts_once",
    },
    KnownSite {
        site: "launch.stage",
        primitive: "the publication writer (begin_owner_change)",
        owner: "the launch stages its workflow custody before its bind, outside protection",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_the_admitted_check_waits_as_shipped_and_starts_once",
    },
    KnownSite {
        site: "class_callbacks.exclusion",
        primitive: "the class callback exclusion (a tokio mutex)",
        owner: "a class input's callback grants are issued before the send (its first binding), \
            outside protection",
        test: "private_originals::governed_submit::tests::dynamic_member_native_tests::native_operations_tests::a_class_input_first_send_marks_its_callback_grant_and_keeps_its_shipped_wait",
    },
    KnownSite {
        site: "class_callbacks.issue_root",
        primitive: "the grant publication's writer, inside the root grant's issuance",
        owner: "as class_callbacks.exclusion (the callback root grant's issuance)",
        test: "private_originals::governed_submit::tests::dynamic_member_native_tests::native_operations_tests::a_class_input_first_send_marks_its_callback_grant_and_keeps_its_shipped_wait",
    },
    KnownSite {
        site: "class_callbacks.issue_leaf",
        primitive: "the grant publication's writer, inside each callback leaf's issuance",
        owner: "as class_callbacks.exclusion (the callback leaf grants' issuance)",
        test: "private_originals::governed_submit::tests::dynamic_member_native_tests::native_operations_tests::a_class_input_first_send_marks_its_callback_grant_and_keeps_its_shipped_wait",
    },
    KnownSite {
        site: "service_setup.issue_child",
        primitive: "the grant publication's writer, inside the child grant's issuance",
        owner: "a service setup's child grants are issued before the send (its first binding), \
            outside protection",
        test: "private_originals::governed_submit::tests::first_input_tests::helper_report::warm_reacquisition::a_batch_task_first_send_marks_its_service_setup_grant_and_keeps_its_shipped_wait",
    },
    KnownSite {
        site: "collector.disposition",
        primitive: "the collector's originals (a std mutex)",
        owner: "collector reads answer from published state (native admission reads)",
        test: "private_originals::governed_submit::tests::dynamic_member_native_tests::native_operations_tests::a_class_input_first_send_marks_its_callback_grant_and_keeps_its_shipped_wait",
    },
    KnownSite {
        site: "collector.current_original",
        primitive: "the collector's originals (a std mutex)",
        owner: "collector reads answer from published state (native admission reads)",
        test: "installed_host::tests::gateway_identity::collector::public_occurrence_reads_and_retains_native_audit_before_joined_close",
    },
    KnownSite {
        site: "store.begin_record_change",
        primitive: "the publication writer (begin_owner_change)",
        owner: "a store transaction never takes the shared publication writer (host store \
            transaction)",
        test: "private_originals::setup_workflow::launch::tests::first_send_not_now_tests::a_first_launch_held_at_input_preparation_keeps_its_shipped_refusal",
    },
];

fn known(site: &str) -> bool {
    KNOWN_SITES.iter().any(|known| known.site == site)
}

/// The known sites this test process reached inside a protected section.
#[cfg(test)]
static REACHED: std::sync::Mutex<std::collections::BTreeSet<&str>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Test builds: whether this test process reached the known site `site`
/// inside a protected section.
#[cfg(test)]
pub(crate) fn reached_inside_a_section(site: &str) -> bool {
    REACHED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(site)
}

/// Test builds: record the first time this test process reaches each known
/// site inside a protected section; when `TOOLKIT_KNOWN_WAIT_LOG` names a
/// directory, also write it there (`site`, then the selected test), so that
/// every listed site can name a test that reaches it.
#[cfg(test)]
fn record_known_wait(site: &'static str) {
    use std::io::Write;
    if meerkat_core::protected_section::current().is_none()
        || !REACHED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(site)
    {
        return;
    }
    let Some(directory) = std::env::var_os("TOOLKIT_KNOWN_WAIT_LOG") else {
        return;
    };
    let test = std::env::args()
        .skip(1)
        .find(|argument| !argument.starts_with('-'))
        .unwrap_or_default();
    let path =
        std::path::Path::new(&directory).join(format!("known-waits-{}.tsv", std::process::id()));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{site}\t{test}");
    }
}

/// About to take a lock another actor may hold across I/O (or to spin or
/// sleep) at `site`.
#[track_caller]
pub(crate) fn may_wait(site: &'static str) {
    if known(site) {
        #[cfg(test)]
        record_known_wait(site);
    } else {
        meerkat_core::protected_section::assert_may_wait(site);
    }
}

/// Run `wait`, which waits at `site` (for the shared publication writer, or
/// another owner's lock). A listed site runs as a known wait; any other is
/// reported inside a section, as [`may_wait`] reports it.
#[track_caller]
pub(crate) fn wait_at<T>(site: &'static str, wait: impl FnOnce() -> T) -> T {
    if known(site) {
        #[cfg(test)]
        record_known_wait(site);
        meerkat_core::protected_section::known_wait(site, wait)
    } else {
        meerkat_core::protected_section::assert_may_wait(site);
        wait()
    }
}

/// Whether a binder's input check may answer "not now" here instead of
/// waiting or answering what its door shipped: before the attempt's first
/// durable effect.
pub(crate) fn may_answer_not_now() -> bool {
    meerkat_core::protected_section::effect_recorded().is_none()
}

/// Waiting calls the source guard knows inside the protected impls
/// (`file`, `type`, `call`, `owner`): each still waits and names the change
/// that removes it.
#[cfg(test)]
const KNOWN_CALLS: &[(&str, &str, &str, &str)] = &[
    (
        "src/private_originals/governed_submit.rs",
        "OriginalBinder",
        "enroll(",
        "custody_recovery.enroll",
    ),
    (
        "src/private_originals/setup_helpers.rs",
        "SubmitHelperBinder",
        "enroll(",
        "custody_recovery.enroll",
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    /// The traits whose methods Meta or Core call inside a protected
    /// section.
    const PROTECTED_TRAITS: &[&str] = &[
        "GovernedInputBinder",
        "GovernedEntryFence",
        "GovernedEntryPermit",
        "GovernedSettlement",
        "PeerIngressBinder",
        "AuthenticatedOriginalSource",
        "NativeInputAdmissionCustody",
    ];

    /// Protected traits whose implementing type is an owner handle, not a
    /// decision (a whole source owner, an entry fence or permit over a login
    /// or a launch attempt): only the trait impl's own body is scanned,
    /// never the handle's other impls, whose methods run outside every
    /// section (a presenter's wait for a login's retirement, a writer). The
    /// runtime witness covers what the trait methods reach.
    const OWNER_TRAITS: &[&str] = &[
        "AuthenticatedOriginalSource",
        "GovernedEntryFence",
        "GovernedEntryPermit",
    ];

    /// Calls that can wait for another actor: a blocking lock or writer, a
    /// spin, a sleep, an awaited writer or wait helper. Each is matched at a
    /// call boundary, so its non-waiting `try_` form never matches.
    const WAITING_CALLS: &[&str] = &[
        "begin_owner_change(",
        "wait_for_lock(",
        "enroll(",
        "source_writer(",
        "bounded_source_writer(",
        "check_admitted_helper_until(",
        "wait_out_held(",
        "wait_for_control(",
        "wait_until_free(",
        "block_on(",
        "blocking_lock(",
        "blocking_recv(",
        "blocking_send(",
        "thread::sleep(",
        "time::sleep(",
    ];

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                if path.file_name().is_some_and(|name| {
                    let name = name.to_string_lossy();
                    name != "tests" && !name.ends_with("_tests")
                }) {
                    rust_files(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs") && !is_test_file(&path) {
                out.push(path);
            }
        }
    }

    /// Test modules and fixtures, which may wait on purpose.
    fn is_test_file(path: &Path) -> bool {
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        name == "tests"
            || name.ends_with("_tests")
            || name.contains("test_support")
            || name.contains("test_bridge")
            || name.ends_with("_fixture")
    }

    /// `text` with every comment and string or char literal blanked to
    /// spaces (newlines kept), so braces and calls inside them are ignored.
    fn blank(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        let keep = |c: char| if c == '\n' { '\n' } else { ' ' };
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < chars.len() && chars[i] != '\n' {
                    out.push(' ');
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 0usize;
                while i < chars.len() {
                    if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                        depth += 1;
                        out.push_str("  ");
                        i += 2;
                    } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        out.push_str("  ");
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        out.push(keep(chars[i]));
                        i += 1;
                    }
                }
            } else if (c == 'r' || c == 'b')
                && !chars
                    .get(i.wrapping_sub(1))
                    .is_some_and(|p| p.is_alphanumeric() || *p == '_')
                && {
                    let mut j = i + 1;
                    if c == 'b' && chars.get(j) == Some(&'r') {
                        j += 1;
                    }
                    while chars.get(j) == Some(&'#') {
                        j += 1;
                    }
                    chars.get(j) == Some(&'"')
                        && (c == 'r' || j > i + 1 || chars.get(i + 1) == Some(&'"'))
                }
            {
                // A raw or byte string: r"..", r#".."#, br#".."#, b"..".
                let mut j = i + 1;
                if c == 'b' && chars.get(j) == Some(&'r') {
                    j += 1;
                }
                let raw = c == 'r' || (c == 'b' && chars.get(i + 1) == Some(&'r'));
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                for _ in i..=j {
                    out.push(' ');
                }
                i = j + 1;
                while i < chars.len() {
                    if !raw && chars[i] == '\\' {
                        out.push(' ');
                        out.push(keep(chars.get(i + 1).copied().unwrap_or(' ')));
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' && (0..hashes).all(|k| chars.get(i + 1 + k) == Some(&'#')) {
                        for _ in 0..=hashes {
                            out.push(' ');
                        }
                        i += 1 + hashes;
                        break;
                    }
                    out.push(keep(chars[i]));
                    i += 1;
                }
            } else if c == '"' {
                out.push(' ');
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\\' {
                        out.push(' ');
                        out.push(keep(chars.get(i + 1).copied().unwrap_or(' ')));
                        i += 2;
                        continue;
                    }
                    if chars[i] == '"' {
                        out.push(' ');
                        i += 1;
                        break;
                    }
                    out.push(keep(chars[i]));
                    i += 1;
                }
            } else if c == '\'' {
                // A char literal ('x', '\n', '\'', '\u{..}'); a lifetime is
                // kept. An escape's own character is never its close.
                let close = if next == Some('\\') {
                    (i + 3..chars.len().min(i + 12)).find(|&k| chars[k] == '\'')
                } else if chars.get(i + 2) == Some(&'\'') {
                    Some(i + 2)
                } else {
                    None
                };
                match close {
                    Some(end) => {
                        for _ in i..=end {
                            out.push(' ');
                        }
                        i = end + 1;
                    }
                    None => {
                        out.push(c);
                        i += 1;
                    }
                }
            } else {
                out.push(c);
                i += 1;
            }
        }
        out
    }

    /// One `impl` block: its header (before `{`) and body byte range.
    struct ImplBlock {
        header: String,
        body: std::ops::Range<usize>,
    }

    fn impl_blocks(code: &str) -> Vec<ImplBlock> {
        let bytes = code.as_bytes();
        let mut blocks = Vec::new();
        let mut at = 0;
        while let Some(found) = code[at..].find("impl") {
            let start = at + found;
            at = start + 4;
            let before = start.checked_sub(1).map(|k| bytes[k]);
            let after = bytes.get(start + 4).copied();
            let boundary =
                |b: Option<u8>| b.is_none_or(|b| !(b.is_ascii_alphanumeric() || b == b'_'));
            if !boundary(before) || !boundary(after) {
                continue;
            }
            let Some(open) = code[start..].find(['{', ';']).map(|k| start + k) else {
                continue;
            };
            if bytes[open] == b';' {
                continue;
            }
            let mut depth = 0usize;
            let mut end = open;
            for (k, b) in bytes.iter().enumerate().skip(open) {
                match b {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = k;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            blocks.push(ImplBlock {
                header: code[start + 4..open]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" "),
                body: open..end,
            });
        }
        blocks
    }

    /// The last path segment of a type or trait, without generics.
    fn last_segment(path: &str) -> String {
        let path = path.split('<').next().unwrap_or(path);
        let path = path.split(" where ").next().unwrap_or(path);
        path.rsplit("::").next().unwrap_or(path).trim().to_string()
    }

    /// The header's trait and type: `impl<..> Trait for Type` or `impl Type`.
    fn header_names(header: &str) -> (Option<String>, String) {
        let header = header.trim();
        let header = if header.starts_with('<') {
            let mut depth = 0usize;
            let mut cut = header.len();
            for (k, c) in header.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            cut = k + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            header[cut..].trim()
        } else {
            header
        };
        match header.split_once(" for ") {
            Some((trait_path, ty)) => (Some(last_segment(trait_path)), last_segment(ty)),
            None => (None, last_segment(header)),
        }
    }

    fn calls_in(code: &str, body: &std::ops::Range<usize>, call: &str) -> Vec<usize> {
        let bytes = code.as_bytes();
        let mut found = Vec::new();
        let mut at = body.start;
        while let Some(k) = code[at..body.end].find(call) {
            let start = at + k;
            at = start + call.len();
            let before = start.checked_sub(1).map(|p| bytes[p]);
            if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                continue;
            }
            // A definition of a method by that name is not a call.
            if code[..start].trim_end().ends_with("fn") {
                continue;
            }
            found.push(start);
        }
        found
    }

    /// Every waiting call inside a protected impl, and inside the inherent
    /// impls of a type that implements a protected decision trait in the
    /// same file: (`file`, `type`, `call`, `line`).
    fn waiting_calls() -> Vec<(String, String, String, usize)> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        let mut hits = Vec::new();
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let code = blank(&text);
            let blocks = impl_blocks(&code);
            let deciders: BTreeSet<String> = blocks
                .iter()
                .filter_map(|block| match header_names(&block.header) {
                    (Some(trait_name), ty)
                        if PROTECTED_TRAITS.contains(&trait_name.as_str())
                            && !OWNER_TRAITS.contains(&trait_name.as_str()) =>
                    {
                        Some(ty)
                    }
                    _ => None,
                })
                .collect();
            let file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            for block in &blocks {
                let (trait_name, ty) = header_names(&block.header);
                let scanned = match &trait_name {
                    Some(trait_name) => PROTECTED_TRAITS.contains(&trait_name.as_str()),
                    None => deciders.contains(&ty),
                };
                if !scanned {
                    continue;
                }
                for call in WAITING_CALLS {
                    for offset in calls_in(&code, &block.body, call) {
                        let line = code[..offset].matches('\n').count() + 1;
                        hits.push((file.clone(), ty.clone(), (*call).to_string(), line));
                    }
                }
            }
        }
        hits
    }

    /// The source guard: no waiting call inside a protected impl except a
    /// known one, and no known entry that no longer exists.
    #[test]
    fn protected_impls_make_no_unknown_waiting_call() {
        let hits = waiting_calls();
        let unknown: Vec<_> = hits
            .iter()
            .filter(|(file, ty, call, _)| {
                !super::KNOWN_CALLS
                    .iter()
                    .any(|(f, t, c, _)| f == file && t == ty && c == call)
            })
            .collect();
        assert!(
            unknown.is_empty(),
            "waiting calls inside protected impls (file, type, call, line): {unknown:#?}"
        );
        let stale: Vec<_> = super::KNOWN_CALLS
            .iter()
            .filter(|(f, t, c, _)| {
                !hits
                    .iter()
                    .any(|(file, ty, call, _)| f == file && t == ty && c == call)
            })
            .collect();
        assert!(
            stale.is_empty(),
            "known waiting calls that no longer exist: {stale:#?}"
        );
    }

    /// Every known runtime site still names itself in the source.
    #[test]
    fn every_known_site_still_names_itself() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        rust_files(&root.join("src"), &mut files);
        let sources: String = files
            .iter()
            .filter(|path| !path.ends_with("no_wait.rs"))
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();
        let stale: Vec<_> = super::KNOWN_SITES
            .iter()
            .filter(|known| !sources.contains(&format!("\"{}\"", known.site)))
            .map(|known| known.site)
            .collect();
        assert!(
            stale.is_empty(),
            "known sites that no longer exist: {stale:#?}"
        );
    }

    /// Every known site names what it waits for, the change that removes it, and one
    /// test of this crate that reaches it inside a section.
    #[test]
    fn every_known_site_names_its_primitive_owner_and_a_reaching_test() {
        /// Every Rust file under `dir`, tests included: the reaching tests
        /// live in test modules.
        fn every_rust_file(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for path in entries.filter_map(Result::ok).map(|entry| entry.path()) {
                if path.is_dir() {
                    every_rust_file(&path, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        every_rust_file(&root.join("src"), &mut files);
        let sources: String = files
            .iter()
            .filter_map(|path| std::fs::read_to_string(path).ok())
            .collect();
        let incomplete: Vec<_> = super::KNOWN_SITES
            .iter()
            .filter(|known| {
                let name = known.test.rsplit("::").next().unwrap_or_default();
                known.primitive.is_empty()
                    || known.owner.is_empty()
                    || name.is_empty()
                    || !sources.contains(&format!("fn {name}("))
            })
            .map(|known| known.site)
            .collect();
        assert!(
            incomplete.is_empty(),
            "known sites without a primitive, an owner or a reaching test: \
             {incomplete:#?}"
        );
    }

    /// The lexer blanks comments and literals and keeps lifetimes.
    #[test]
    fn the_guard_lexer_ignores_comments_and_literals() {
        let code = blank(
            "fn f<'a>() { let _ = \"enroll(\"; // enroll(\n /* wait_for_lock( */ x.enroll(); \
             let c = '{'; let q = '\\''; let r = r#\"block_on(\"#; }",
        );
        assert_eq!(code.matches("enroll(").count(), 1);
        assert!(code.contains("<'a>"));
        assert!(!code.contains("'{'"));
        assert!(!code.contains("block_on("));
        assert_eq!(code.matches('{').count(), code.matches('}').count());
        let blocks = impl_blocks(
            "impl<T> crate::a::GovernedInputBinder for Wrapper<T> { fn enroll(&self) {} fn b() { x.enroll(); } }",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            header_names(&blocks[0].header),
            (
                Some("GovernedInputBinder".to_string()),
                "Wrapper".to_string()
            )
        );
        let code = "impl<T> crate::a::GovernedInputBinder for Wrapper<T> { fn enroll(&self) {} fn b() { x.enroll(); } }";
        assert_eq!(calls_in(code, &blocks[0].body, "enroll(").len(), 1);
    }

    /// A binder's input check answers "not now" only inside a governed bind
    /// attempt, before the attempt's first durable effect. Outside every
    /// section (a caller that did not enter one) and inside any other section
    /// it answers what its door shipped.
    #[tokio::test]
    async fn only_a_governed_bind_before_its_first_effect_may_answer_not_now() {
        use meerkat_core::protected_section::{ProtectedSection, enter, note_effect, within};
        assert!(!super::may_answer_not_now(), "outside every section");
        within(ProtectedSection::GovernedBind, async {
            assert!(super::may_answer_not_now(), "in a bind, before its effect");
            note_effect("test.effect");
            assert!(!super::may_answer_not_now(), "in a bind, after its effect");
        })
        .await;
        within(ProtectedSection::HostStoreTransaction, async {
            assert!(!super::may_answer_not_now(), "in a host store transaction");
        })
        .await;
        let _section = enter(ProtectedSection::NativeAdmissionRead);
        assert!(!super::may_answer_not_now(), "in a native admission read");
    }
}
