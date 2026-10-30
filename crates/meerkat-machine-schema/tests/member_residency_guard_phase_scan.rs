#![allow(clippy::expect_used, clippy::panic)]
//! No phase predicate inside a member-residency guard.
//!
//! Schema lowering (meerkat-machine-dsl-core `strip_phase_guards`) removes a
//! `self.lifecycle_phase` comparison from a guard, removes a guard CALL that
//! takes the phase, and drops a phase disjunct from an `||`. A guard that
//! mixes the phase with other conditions therefore lowers to a model that is
//! stronger or weaker than production, and TLC checks the wrong arm.
//!
//! The member-residency arms (Dormant, Waking and Live, the wake, release,
//! seat, seat session replay, rotation, boot-plan and wiring-wake arms, and
//! the existing arms they rewrote) are written one arm per lifecycle phase
//! instead. This scan reads the `MobMachine` DSL source and fails when any of
//! those arms has a guard, or a call inside one, that mentions
//! `lifecycle_phase` beyond the plain source predicate
//! `guard { self.lifecycle_phase == Phase::X }` (or a disjunction of such
//! comparisons only), and when a residency helper takes or reads the
//! lifecycle phase. It also fails when an expected arm family is missing, so
//! the scan cannot pass vacuously.

/// Transition-name prefixes of the member-residency arms the scan covers.
const RESIDENCY_ARM_PREFIXES: &[&str] = &[
    "WakeMember",
    "PlanBootMaterialization",
    "SealBootMaterializationPlan",
    "CommitDormantSeat",
    "CommitMemberRotation",
    "BeginMemberRotation",
    "ResolveMemberRevival",
    "AbandonMemberWake",
    "ReleaseMemberMaterialization",
    "ClassifyPeerNotice",
    "SeedMemberResidency",
    "ResolveMemberHostPreparation",
    "ClassifyMemberLiveMaterialization",
    "AuthorizeExplicitResumeMember",
    "ClassifyExplicitResumeMemberLive",
    "SettleExplicitResumeMember",
    "ResolveRecreatedMemberSessionSucceeded",
    "ClassifyMemberRelease",
    "RetireRunningReleasingArchiveOnly",
    "RetireRunningPreservingBindingArchiveOnly",
    "RetireStoppedReleasingArchiveOnly",
    "RetireStoppedPreservingBindingArchiveOnly",
    "RetireMemberArchiveOnly",
    "AdmitDestroyMemberRetireArchiveOnly",
    "WireExternalPeerDormant",
    "WireExternalPeerAlreadyWiredDormant",
    "UnwireExternalPeerDormant",
    "WireMembers",
    "RecoverMemberSeatSession",
];

/// Families that must each name at least one arm, so the scan is not vacuous.
const REQUIRED_ARM_FAMILIES: &[&str] = &[
    "WakeMember",
    "PlanBootMaterialization",
    "SealBootMaterializationPlan",
    "CommitDormantSeat",
    "CommitMemberRotation",
    "BeginMemberRotation",
    "AbandonMemberWake",
    "ReleaseMemberMaterialization",
    "ClassifyPeerNotice",
    "SeedMemberResidency",
    "ResolveMemberHostPreparation",
    "ResolveMemberRevival",
    "ClassifyMemberRelease",
    "RetireMemberArchiveOnly",
    "AdmitDestroyMemberRetireArchiveOnly",
    "WireExternalPeerDormant",
    "UnwireExternalPeerDormant",
    "WireMembersPlacedPeerWake",
    "WireMembersWithTrustPlacedPeerWake",
    "ResolveMemberRevivalSessionCreated",
    "RecoverMemberSeatSession",
];

/// Transition-name prefixes of the `MeerkatMachine` arms that serve member
/// residency (the empty-resume verdict and the composition's residency
/// claim). Each must name at least one arm.
const MEERKAT_RESIDENCY_ARM_PREFIXES: &[&str] =
    &["ResolveEmptyResumeRetirement", "ClaimCompositionResidency"];

/// Transition-name prefixes of the `MeerkatMachine` arms that hold a kept
/// governed input for its controller client and release it, and of the arms
/// whose guards refuse a held input or its run (`StageForRun`, `MarkApplied`,
/// `MarkAppliedPendingConsumption`, and `ResumeRun`, which also waits for a
/// resumed run's destination). Each must name at least one arm.
const MEERKAT_CONTROLLER_HOLD_ARM_PREFIXES: &[&str] = &[
    "HoldInputController",
    "ReleaseInputController",
    "StageForRun",
    "MarkApplied",
    "ResumeRun",
];

/// Helper-name prefixes of the residency helpers: they take member facts
/// only, never the lifecycle phase.
const RESIDENCY_HELPER_PREFIXES: &[&str] = &[
    "member_wake",
    "member_boot",
    "peer_notice",
    "member_release",
    "member_registration",
];

fn dsl_source(file: &str) -> String {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    );
    let path = manifest_dir.join("src/catalog/dsl").join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn mob_machine_dsl_source() -> String {
    dsl_source("mob_machine.rs")
}

/// The source with every `//` line comment blanked (string literals kept), so
/// prose about guards never reads as a guard.
fn without_line_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut cut = line.len();
        let bytes = line.as_bytes();
        for (index, &byte) in bytes.iter().enumerate() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    in_string = false;
                }
                continue;
            }
            if byte == b'"' {
                in_string = true;
            } else if byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
                cut = index;
                break;
            }
        }
        out.push_str(&line[..cut]);
        out.push('\n');
    }
    out
}

/// Index of the `}` matching the `{` at `open`, skipping string literals.
fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate().skip(open) {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

struct ScannedTransition {
    name: String,
    per_phase: bool,
    /// (guard name, empty when unnamed; guard body)
    guards: Vec<(String, String)>,
}

/// Every `transition <Name> { ... }` block of the DSL with its guards.
fn scanned_transitions(source: &str) -> Vec<ScannedTransition> {
    let mut transitions = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = source[cursor..].find("transition ") {
        let start = cursor + found;
        cursor = start + "transition ".len();
        let at_line_start = source[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|prefix| prefix.trim().is_empty());
        if !at_line_start {
            continue;
        }
        let rest = &source[cursor..];
        let name: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        let after_name = cursor + name.len();
        let Some(open_offset) = source[after_name..].find('{') else {
            continue;
        };
        if !source[after_name..after_name + open_offset]
            .trim()
            .is_empty()
        {
            continue;
        }
        let open = after_name + open_offset;
        let Some(close) = matching_brace(source, open) else {
            panic!("transition {name} has no closing brace");
        };
        let body = &source[open + 1..close];
        transitions.push(ScannedTransition {
            name,
            per_phase: body
                .lines()
                .any(|line| line.trim_start().starts_with("per_phase")),
            guards: scanned_guards(body),
        });
        cursor = close;
    }
    transitions
}

fn scanned_guards(body: &str) -> Vec<(String, String)> {
    let mut guards = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = body[cursor..].find("guard") {
        let start = cursor + found;
        cursor = start + "guard".len();
        let at_line_start = body[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|prefix| prefix.trim().is_empty());
        if !at_line_start {
            continue;
        }
        let rest = body[cursor..].trim_start();
        let (name, after_name) = if let Some(named) = rest.strip_prefix('"') {
            let Some(end) = named.find('"') else {
                panic!("unterminated guard name in `{}`", &body[start..]);
            };
            (named[..end].to_string(), &named[end + 1..])
        } else {
            (String::new(), rest)
        };
        if !after_name.trim_start().starts_with('{') {
            continue;
        }
        let open = body.len() - after_name.trim_start().len();
        let Some(close) = matching_brace(body, open) else {
            panic!("guard {name:?} has no closing brace");
        };
        guards.push((name, body[open + 1..close].to_string()));
        cursor = close;
    }
    guards
}

/// `self.lifecycle_phase == Phase::X`, or a disjunction of such comparisons.
fn is_plain_source_predicate(guard_body: &str) -> bool {
    let compact: String = guard_body.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.split("||").all(|disjunct| {
        let disjunct = disjunct.trim();
        let disjunct = disjunct
            .strip_prefix('(')
            .and_then(|inner| inner.strip_suffix(')'))
            .unwrap_or(disjunct)
            .trim();
        disjunct
            .strip_prefix("self.lifecycle_phase == Phase::")
            .is_some_and(|variant| {
                !variant.is_empty() && variant.chars().all(|ch| ch.is_ascii_alphanumeric())
            })
    })
}

fn is_residency_arm(name: &str) -> bool {
    RESIDENCY_ARM_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Every guard of the arms `select` picks that reads the lifecycle phase
/// beyond the plain source predicate.
fn phase_mixing_guards(
    transitions: &[ScannedTransition],
    select: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut violations = Vec::new();
    for transition in transitions
        .iter()
        .filter(|transition| select(&transition.name))
    {
        for (guard_name, guard_body) in &transition.guards {
            if !guard_body.contains("lifecycle_phase") {
                continue;
            }
            let plain_source = guard_name.is_empty()
                && !transition.per_phase
                && is_plain_source_predicate(guard_body);
            if !plain_source {
                let label = if guard_name.is_empty() {
                    "<source guard>".to_string()
                } else {
                    format!("\"{guard_name}\"")
                };
                violations.push(format!(
                    "{} guard {label}: {}",
                    transition.name,
                    guard_body.split_whitespace().collect::<Vec<_>>().join(" ")
                ));
            }
        }
    }
    violations
}

#[test]
fn residency_arm_guards_never_mix_the_lifecycle_phase() {
    let source = without_line_comments(&mob_machine_dsl_source());
    let transitions = scanned_transitions(&source);
    assert!(
        transitions.len() > 500,
        "the DSL scan found only {} transitions",
        transitions.len()
    );

    let violations = phase_mixing_guards(&transitions, is_residency_arm);
    assert!(
        violations.is_empty(),
        "member-residency arms must split per lifecycle phase instead of reading it in a guard \
         or a guard call (schema lowering strips those):\n{}",
        violations.join("\n")
    );

    let missing: Vec<&str> = REQUIRED_ARM_FAMILIES
        .iter()
        .copied()
        .filter(|family| {
            !transitions
                .iter()
                .any(|transition| transition.name.starts_with(family))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "expected member-residency arm families are missing from the DSL: {missing:?}"
    );
}

/// The `MeerkatMachine` arms that serve member residency follow the same rule:
/// the empty-resume verdict and the composition's claim are one arm per
/// lifecycle phase, with no phase read in a guard.
#[test]
fn meerkat_residency_arm_guards_never_mix_the_lifecycle_phase() {
    let source = without_line_comments(&dsl_source("meerkat_machine.rs"));
    let transitions = scanned_transitions(&source);
    assert!(
        transitions.len() > 300,
        "the MeerkatMachine DSL scan found only {} transitions",
        transitions.len()
    );
    let is_meerkat_residency_arm = |name: &str| {
        MEERKAT_RESIDENCY_ARM_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    };
    let violations = phase_mixing_guards(&transitions, is_meerkat_residency_arm);
    assert!(
        violations.is_empty(),
        "MeerkatMachine residency arms must split per lifecycle phase instead of reading it in a \
         guard or a guard call (schema lowering strips those):\n{}",
        violations.join("\n")
    );
    let missing: Vec<&str> = MEERKAT_RESIDENCY_ARM_PREFIXES
        .iter()
        .copied()
        .filter(|family| {
            !transitions
                .iter()
                .any(|transition| transition.name.starts_with(family))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "expected MeerkatMachine residency arm families are missing from the DSL: {missing:?}"
    );
}

/// The `MeerkatMachine` controller-hold arms, and the arms whose guards read
/// the holds or a resumed run's destination, follow the same rule: one arm
/// per lifecycle phase, with no phase read in a guard or a guard call.
#[test]
fn meerkat_controller_hold_arm_guards_never_mix_the_lifecycle_phase() {
    let source = without_line_comments(&dsl_source("meerkat_machine.rs"));
    let transitions = scanned_transitions(&source);
    let is_controller_hold_arm = |name: &str| {
        MEERKAT_CONTROLLER_HOLD_ARM_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
    };
    let violations = phase_mixing_guards(&transitions, is_controller_hold_arm);
    assert!(
        violations.is_empty(),
        "MeerkatMachine controller-hold arms must split per lifecycle phase instead of reading \
         it in a guard or a guard call (schema lowering strips those):\n{}",
        violations.join("\n")
    );
    let missing: Vec<&str> = MEERKAT_CONTROLLER_HOLD_ARM_PREFIXES
        .iter()
        .copied()
        .filter(|family| {
            !transitions
                .iter()
                .any(|transition| transition.name.starts_with(family))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "expected MeerkatMachine controller-hold arm families are missing from the DSL: \
         {missing:?}"
    );
}

#[test]
fn residency_helpers_take_member_facts_only() {
    let source = without_line_comments(&mob_machine_dsl_source());
    let mut checked = Vec::new();
    let mut cursor = 0usize;
    while let Some(found) = source[cursor..].find("helper ") {
        let start = cursor + found;
        cursor = start + "helper ".len();
        let at_line_start = source[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|prefix| prefix.trim().is_empty());
        if !at_line_start {
            continue;
        }
        let name: String = source[cursor..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        if !RESIDENCY_HELPER_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        let Some(open_offset) = source[cursor..].find('{') else {
            panic!("helper {name} has no body");
        };
        let open = cursor + open_offset;
        let Some(close) = matching_brace(&source, open) else {
            panic!("helper {name} has no closing brace");
        };
        let signature_and_body = &source[cursor..close];
        assert!(
            !signature_and_body.contains("lifecycle_phase")
                && !signature_and_body.contains("MobPhase"),
            "residency helper {name} takes or reads the lifecycle phase; give each lifecycle phase its own arm instead"
        );
        checked.push(name);
        cursor = close;
    }
    assert!(
        checked
            .iter()
            .any(|name| name == "member_boot_materialization"),
        "the boot classifier helper member_boot_materialization was not found (checked: {checked:?})"
    );
}

#[test]
fn plain_source_predicates_are_recognized() {
    assert!(is_plain_source_predicate(
        " self.lifecycle_phase == Phase::Running "
    ));
    assert!(is_plain_source_predicate(
        "self.lifecycle_phase == Phase::Running\n || self.lifecycle_phase == Phase::Stopped"
    ));
    assert!(!is_plain_source_predicate(
        "self.lifecycle_phase == Phase::Running || self.destroy_admitted == false"
    ));
    assert!(!is_plain_source_predicate(
        "member_wake_verdict(self.lifecycle_phase, phase) == MemberWakeVerdictKind::Authorized"
    ));
}

#[test]
fn the_scanner_reads_named_and_unnamed_guards() {
    let source = r#"
        transition WakeMemberProbeRunning {
            on input WakeMember { agent_identity }
            guard { self.lifecycle_phase == Phase::Running }
            guard "probe" { probe(self.lifecycle_phase) == true }
            update {}
            to Running
        }
        // transition Commented { guard "x" { self.lifecycle_phase == Phase::Running } }
        transition Other {
            per_phase [Running]
            on input Other
            guard "fine" { self.ready == true }
            update {}
            to Running
        }
    "#;
    let transitions = scanned_transitions(&without_line_comments(source));
    assert_eq!(transitions.len(), 2);
    assert_eq!(transitions[0].name, "WakeMemberProbeRunning");
    assert!(!transitions[0].per_phase);
    assert_eq!(transitions[0].guards.len(), 2);
    assert!(transitions[0].guards[0].0.is_empty());
    assert!(is_plain_source_predicate(&transitions[0].guards[0].1));
    assert_eq!(transitions[0].guards[1].0, "probe");
    assert!(transitions[0].guards[1].1.contains("lifecycle_phase"));
    assert!(transitions[1].per_phase);
}
