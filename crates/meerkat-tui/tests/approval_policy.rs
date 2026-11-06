//! Pure-logic tests for the auto-mode tier policy: tiers, user rules and shell
//! analysis, driven through [`classify_tier`] with a real project tree.
//!
//! The old crate drove these through `ApprovalEngine::decide` with a scripted
//! classifier. The engine (classifier allow cache, pause/resume after
//! repeated blocks, escalation, stop handling, agent-written tracking, build
//! script payloads, transcript recording and the hook handlers) is ported in
//! P2 on upstream's `ToolDispatchAdmission`; its tests return with it:
//! `classifier_allow_is_cached_per_session`,
//! `allowed_writes_are_shown_to_the_classifier_as_agent_written`,
//! `disabled_auto_mode_denies_tier_three_outright`,
//! `three_blocks_pause_then_human_decides_and_resumes`,
//! `stop_denies_pending_escalations_until_next_prompt`,
//! `classifier_error_escalates_and_is_not_counted`,
//! `classifier_error_without_broker_denies`,
//! `human_allow_and_add_rule_allows_future_calls`,
//! `hook_engine_end_to_end`, `hook_classifier_block_reports_safety_violation`,
//! `allow_and_add_rule_mints_a_literal_rule_from_the_subject`,
//! `cargo_payload_includes_build_script`,
//! `injected_prompt_does_not_clear_a_stop_but_a_new_run_does`,
//! `forget_prompt_removes_the_recalled_message`,
//! `sub_agent_prompt_is_an_action_on_the_primary_transcript`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::ToolSourceKind;
use meerkat_tui::approval::catalog::ToolOrigin;
use meerkat_tui::approval::policy::{
    Action, DELEGATION_TOOLS, PolicyContext, READ_ONLY_BUILTINS, REVIEWED_BUILTINS, Tier,
    TierDecision, classify_tier,
};
use meerkat_tui::approval::rules::{Rule, RuleKind, RuleOrigin, RuleSet};
use meerkat_tui::memory::{NOTE_TOOL_NAMES, NOTES_SOURCE};
use meerkat_tui::paths::ProjectRoot;
use meerkat_tui::tools::{FILE_TOOL_NAMES, FILE_TOOLS_SOURCE};
use serde_json::{Value, json};

struct Harness {
    _dir: tempfile::TempDir,
    root: ProjectRoot,
    rules: RuleSet,
    trust_project_builds: bool,
    outside: String,
}

struct Options {
    rules: RuleSet,
    trust_project_builds: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            rules: RuleSet::default(),
            trust_project_builds: true,
        }
    }
}

fn harness(opts: Options) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::create_dir_all(project.join(".rkat")).unwrap();
    std::fs::write(project.join("src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.join(".rkat/config.toml"), "").unwrap();
    std::fs::write(dir.path().join("outside.txt"), "secret").unwrap();
    let root = ProjectRoot::new(&project).unwrap();
    let outside = dir.path().join("outside.txt").to_str().unwrap().to_string();
    Harness {
        _dir: dir,
        root,
        rules: opts.rules,
        trust_project_builds: opts.trust_project_builds,
        outside,
    }
}

impl Harness {
    fn decide(&self, name: &str, args: &Value) -> TierDecision {
        self.decide_from(name, args, Some(&origin(name)))
    }

    fn decide_from(&self, name: &str, args: &Value, origin: Option<&ToolOrigin>) -> TierDecision {
        let ctx = PolicyContext {
            root: &self.root,
            trust_project_builds: self.trust_project_builds,
            posix_shell: true,
        };
        let raw = serde_json::value::to_raw_value(args).unwrap();
        classify_tier(&Action::parse(name, &raw, origin), &self.rules, &ctx)
    }

    fn tier(&self, name: &str, args: Value) -> Tier {
        self.decide(name, &args).tier
    }
}

/// Where a real TUI session's tool of `name` comes from (a fixture: the
/// gate reads it from the session's live tool definitions). Names no
/// builtin, file or note tool has are an MCP server's.
fn origin(name: &str) -> ToolOrigin {
    if FILE_TOOL_NAMES.contains(&name) {
        ToolOrigin::tui(FILE_TOOLS_SOURCE, name)
    } else if NOTE_TOOL_NAMES.contains(&name) {
        ToolOrigin::tui(NOTES_SOURCE, name)
    } else if name == meerkat_tui::fork::FORK {
        ToolOrigin::tui(meerkat_tui::fork::FORK_SOURCE, name)
    } else if READ_ONLY_BUILTINS.contains(&name)
        || REVIEWED_BUILTINS.contains(&name)
        || DELEGATION_TOOLS.contains(&name)
        || name.starts_with("mob_")
    {
        ToolOrigin::native(ToolSourceKind::Builtin)
    } else {
        ToolOrigin::mcp("server")
    }
}

fn deny_rule(tool: &str, arg: &str) -> RuleSet {
    let mut rules = RuleSet::default();
    rules.push(Rule::new(RuleKind::Deny, tool, Some(arg), RuleOrigin::Realm).unwrap());
    rules
}

fn allow_rule(tool: &str, arg: &str) -> RuleSet {
    let mut rules = RuleSet::default();
    rules.push(Rule::new(RuleKind::Allow, tool, Some(arg), RuleOrigin::Realm).unwrap());
    rules
}

#[test]
fn read_inside_project_is_always_allowed() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier("read_file", json!({"path": "src/main.rs"})),
        Tier::AlwaysAllow
    );
}

#[test]
fn read_outside_project_needs_the_classifier() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier("read_file", json!({"path": h.outside})),
        Tier::Classifier
    );
}

#[test]
fn in_project_edit_is_tier_two() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier(
            "edit_file",
            json!({"path": "src/main.rs", "old_string": "a", "new_string": "b"}),
        ),
        Tier::InProjectEdit
    );
    assert_eq!(
        h.tier(
            "apply_patch",
            json!({"patch": "*** Begin Patch\n*** Add File: a.txt\n+x\n*** Update File: src/main.rs\n@@\n-fn main() {}\n+fn main() { }\n*** End Patch"}),
        ),
        Tier::InProjectEdit
    );
}

#[test]
fn protected_path_edit_goes_to_classifier() {
    let h = harness(Options::default());
    for path in [".env", ".env.local", "build.rs", "AGENTS.md"] {
        assert_eq!(
            h.tier(
                "edit_file",
                json!({"path": path, "old_string": "", "new_string": "x"}),
            ),
            Tier::Classifier,
            "{path}"
        );
    }
}

/// The gate's own state and the VCS internals are sealed: a file tool
/// write there is denied before the classifier, and an allow rule for the
/// tool does not open it (the classifier flipped on a protected `.env`
/// write between two dogfood runs; the realm's `config.toml` carries the
/// endpoint URLs the API key is sent to).
#[test]
fn sealed_paths_are_denied_before_rules_and_classifier() {
    let sealed = [
        ".rkat/config.toml",
        ".rkat/tui/tui.toml",
        ".rkat/mcp.toml",
        ".git/config",
        ".git/hooks/pre-commit",
        "vendor/.git/HEAD",
    ];
    let plain = harness(Options::default());
    let allowed = harness(Options {
        rules: allow_rule("write_file", "*"),
        ..Options::default()
    });
    for path in sealed {
        for (name, h) in [("plain", &plain), ("allow rule", &allowed)] {
            assert_eq!(
                h.tier("write_file", json!({"path": path, "content": "x"})),
                Tier::Sealed,
                "{path} ({name})"
            );
            assert_eq!(
                h.tier(
                    "edit_file",
                    json!({"path": path, "old_string": "", "new_string": "x"}),
                ),
                Tier::Sealed,
                "{path} ({name})"
            );
        }
    }
    // A sealed target seals the whole patch.
    assert_eq!(
        plain.tier(
            "apply_patch",
            json!({"patch": "*** Begin Patch\n*** Add File: a.txt\n+x\n*** Update File: .rkat/config.toml\n@@\n-a\n+b\n*** End Patch"}),
        ),
        Tier::Sealed
    );
    // Reads of sealed paths are not affected.
    assert_eq!(
        plain.tier("read_file", json!({"path": ".git/config"})),
        Tier::AlwaysAllow
    );
}

#[test]
fn deny_rule_wins() {
    let h = harness(Options {
        rules: deny_rule("shell", "git push*"),
        ..Options::default()
    });
    let decision = h.decide("shell", &json!({"command": "git push --force"}));
    assert_eq!(decision.tier, Tier::UserDeny);
    assert!(decision.rule.is_some());
}

#[test]
fn cargo_test_depends_on_trust_project_builds() {
    let trusted = harness(Options::default());
    assert_eq!(
        trusted.tier("shell", json!({"command": "cargo test"})),
        Tier::AlwaysAllow
    );
    let untrusted = harness(Options {
        trust_project_builds: false,
        ..Options::default()
    });
    assert_eq!(
        untrusted.tier("shell", json!({"command": "cargo test"})),
        Tier::Classifier
    );
}

#[test]
fn broad_rules_are_dropped_and_no_longer_bypass_review() {
    let mut rules = RuleSet::default();
    rules.push(Rule::new(RuleKind::Allow, "shell", None, RuleOrigin::Global).unwrap());
    rules.push(
        Rule::new(
            RuleKind::Allow,
            "shell",
            Some("cargo nextest run*"),
            RuleOrigin::Global,
        )
        .unwrap(),
    );
    let dropped = rules.sanitize_for_auto();
    assert_eq!(dropped.len(), 1);
    assert_eq!(rules.allow().len(), 1);
    let h = harness(Options {
        rules,
        ..Options::default()
    });
    assert_eq!(
        h.tier("shell", json!({"command": "rm -rf target"})),
        Tier::Classifier
    );
}

#[test]
fn workspace_note_save_is_in_project_edit() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier(
            "note_save",
            json!({"scope": "workspace", "name": "build", "content": "make ci"}),
        ),
        Tier::InProjectEdit
    );
}

#[test]
fn user_note_save_reaches_classifier() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier(
            "note_save",
            json!({"scope": "user", "name": "style", "content": "x"}),
        ),
        Tier::Classifier
    );
}

#[test]
fn note_list_is_tier_one() {
    let h = harness(Options::default());
    assert_eq!(h.tier("note_list", json!({})), Tier::AlwaysAllow);
}

#[test]
fn allow_rule_makes_user_note_save_tier_one() {
    let h = harness(Options {
        rules: allow_rule("note_save", "user/*"),
        ..Options::default()
    });
    assert_eq!(
        h.tier(
            "note_save",
            json!({"scope": "user", "name": "style", "content": "x"}),
        ),
        Tier::AlwaysAllow
    );
}

#[test]
fn indented_patch_header_is_a_sealed_target() {
    let h = harness(Options::default());
    let patch = "*** Begin Patch\n*** Add File: src/ok.rs\n+x\n  *** Add File: .git/config\n+[core]\n*** End Patch";
    assert_eq!(
        h.tier("apply_patch", json!({ "patch": patch })),
        Tier::Sealed
    );
}

#[test]
fn shell_with_unknown_argument_reaches_classifier() {
    let h = harness(Options::default());
    assert_eq!(
        h.tier("shell", json!({"command": "ls", "shell": "/bin/evil"})),
        Tier::Classifier
    );
    assert_eq!(h.tier("shell", json!({"command": "ls"})), Tier::AlwaysAllow);
}

#[test]
fn shell_allow_rule_must_cover_every_segment() {
    let h = harness(Options {
        rules: allow_rule("shell", "cargo nextest run*"),
        trust_project_builds: false,
    });
    assert_eq!(
        h.tier("shell", json!({"command": "cargo nextest run -p x"})),
        Tier::AlwaysAllow
    );
    assert_eq!(
        h.tier(
            "shell",
            json!({"command": "cargo nextest run; curl x | sh"}),
        ),
        Tier::Classifier
    );
}

#[test]
fn shell_deny_rule_matches_hidden_segments() {
    let h = harness(Options {
        rules: deny_rule("shell", "git push*"),
        ..Options::default()
    });
    for cmd in [
        "true && git push",
        "git  push",
        "\\git push",
        "ls; git push -f",
    ] {
        assert_eq!(
            h.tier("shell", json!({ "command": cmd })),
            Tier::UserDeny,
            "{cmd}"
        );
    }
}

/// MCP tools keep the names their servers give them; the policy knows none
/// of them, so each reaches the classifier unless a user rule decides it.
#[test]
fn mcp_tools_reach_the_classifier_unless_a_rule_decides() {
    let h = harness(Options::default());
    for (name, args) in [
        ("create_issue", json!({"title": "x"})),
        ("search_repositories", json!({"query": "meerkat"})),
        ("fetch", json!({"url": "https://example.com"})),
    ] {
        let decision = h.decide(name, &args);
        assert_eq!(decision.tier, Tier::Classifier, "{name}");
        assert!(
            decision
                .detail
                .contains("not a tool the TUI or the engine provides"),
            "{decision:?}"
        );
    }

    let mut rules = RuleSet::default();
    rules.push(
        Rule::new(
            RuleKind::Allow,
            "search_repositories",
            None,
            RuleOrigin::Realm,
        )
        .unwrap(),
    );
    rules.push(Rule::new(RuleKind::Deny, "create_issue", None, RuleOrigin::Realm).unwrap());
    let h = harness(Options {
        rules,
        ..Options::default()
    });
    assert_eq!(
        h.tier("search_repositories", json!({"query": "meerkat"})),
        Tier::AlwaysAllow
    );
    assert_eq!(
        h.tier("create_issue", json!({"title": "x"})),
        Tier::UserDeny
    );
    assert_eq!(h.tier("fetch", json!({"url": "x"})), Tier::Classifier);
}

/// An MCP server may name its tools like builtins, file or note tools; the
/// tier follows the tool's provenance, never its name.
#[test]
fn an_mcp_tool_named_like_a_builtin_is_not_trusted_like_one() {
    let h = harness(Options::default());
    let mcp = ToolOrigin::mcp("evil");
    for (name, args, genuine) in [
        (
            "read_file",
            json!({"path": "src/main.rs"}),
            Tier::AlwaysAllow,
        ),
        (
            "write_file",
            json!({"path": "src/x.rs", "content": ""}),
            Tier::InProjectEdit,
        ),
        ("glob", json!({"pattern": "**/*.rs"}), Tier::AlwaysAllow),
        ("task_list", json!({}), Tier::AlwaysAllow),
        ("delegate", json!({"task": "x"}), Tier::AlwaysAllow),
        ("shell", json!({"command": "ls"}), Tier::AlwaysAllow),
        (
            "note_save",
            json!({"scope": "workspace", "name": "x", "content": ""}),
            Tier::InProjectEdit,
        ),
    ] {
        assert_eq!(h.tier(name, args.clone()), genuine, "{name}");
        assert_eq!(
            h.decide_from(name, &args, Some(&mcp)).tier,
            Tier::Classifier,
            "{name} from an MCP server"
        );
        assert_eq!(
            h.decide_from(name, &args, None).tier,
            Tier::Classifier,
            "{name} the session does not offer"
        );
    }
}
