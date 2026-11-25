//! Rows of the access cache's key space for the members of a mob nested
//! under a mob of the host: a mob a member of that mob made. Such a member
//! is recorded apart from the members of the mob it is nested under, by its
//! mob and its name there, and the rules are asked about it exactly as they
//! are asked about a member of the mob it is nested under. A view of the
//! outer mob reads it under the door's string as before while no member of
//! the outer mob holds that string: a string a member of the outer mob
//! holds names that member there.

#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use meerkat_mob::MobId;

use super::controller::{CapturedLineageLink, NestedMemberUnnamed, NestedMobRef};
use super::model::AccessEffect;
use super::{
    ACTION_AGENT_SEND, ACTION_AGENT_VIEW, AccessControlConfig, AccessController, AccessRule,
    AccessView, AgentResourceAttributes, PRIVATE_TO_LABEL,
};

/// The nested mob.
const TEAM: &str = "pair-team";
/// A member's name in the nested mob.
const NAME: &str = "helper";
/// That member as the door of the mob it is nested under names it; also the
/// name of a member of that mob.
const ONE_STRING: &str = "pair-team/helper";
const ROOT: &str = "root@example.test";
const ALICE: &str = "alice@example.test";
const BOB: &str = "bob@example.test";

fn team() -> MobId {
    MobId::from(TEAM)
}

/// The key a person owns its own members under.
fn own_key(person: &str) -> String {
    format!("person:{person}")
}

/// The view of `person`.
fn view_of(access: &AccessController, person: &str) -> AccessView {
    access.view_for_caller(Some(person), Some(&own_key(person)))
}

/// The labels of a member private to `private_to` (no one: every
/// person's), with the further labels `more`.
fn labels(private_to: Option<&str>, more: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut held = BTreeMap::new();
    if let Some(person) = private_to {
        held.insert(PRIVATE_TO_LABEL.to_string(), own_key(person));
    }
    for (key, value) in more {
        held.insert((*key).to_string(), (*value).to_string());
    }
    held
}

/// A member of the outer mob named `identity`, as the console records it.
fn outer_member(identity: &str, private_to: Option<&str>) -> AgentResourceAttributes {
    AgentResourceAttributes {
        identity: identity.to_string(),
        agent_id: Some(identity.to_string()),
        role: Some("worker".to_string()),
        labels: labels(private_to, &[]),
    }
}

/// The labels of a member of the nested mob.
fn nested_labels(private_to: Option<&str>, more: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut held = labels(private_to, more);
    held.insert("source_mob_id".to_string(), TEAM.to_string());
    held
}

/// The member `name` of the nested mob as the console recorded it before
/// the nested mob had a key space: among the outer mob's members, its
/// identity and its agent id the door's string.
fn former_record(
    name: &str,
    private_to: Option<&str>,
    more: &[(&str, &str)],
) -> AgentResourceAttributes {
    let identity = format!("{TEAM}/{name}");
    AgentResourceAttributes {
        agent_id: Some(identity.clone()),
        identity,
        role: Some("worker".to_string()),
        labels: nested_labels(private_to, more),
    }
}

/// Record the member `name` of the nested mob, under the host's default
/// mob.
fn record_nested(
    access: &AccessController,
    name: &str,
    private_to: Option<&str>,
    more: &[(&str, &str)],
) {
    access
        .record_nested_agent_attributes(
            NestedMobRef {
                under: None,
                mob: &team(),
            },
            name,
            Some("worker".to_string()),
            nested_labels(private_to, more),
        )
        .expect("the door can name the member");
}

/// A rule of `subject` over `actions`.
fn rule(id: &str, subject: &str, actions: &[&str]) -> AccessRule {
    AccessRule {
        id: id.to_string(),
        subjects: vec![subject.to_string()],
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        ..AccessRule::default()
    }
}

/// The persons of the rule row, each with what the rules decide for it on
/// the nested member: a view of it and a send to it.
const PERSONS: [(&str, (bool, bool)); 8] = [
    // Every agent.
    ("carol@example.test", (true, true)),
    // The member by the door's string.
    ("dave@example.test", (true, false)),
    // The label of the mob the member sits in.
    ("erin@example.test", (true, false)),
    // Every agent of the nested mob listed in `mobs`: the member is asked
    // as a member of the mob it is nested under, so the rule selects none.
    ("frank@example.test", (false, false)),
    // Every agent, and no send by the label of the mob.
    ("grace@example.test", (true, false)),
    // No rule.
    ("heidi@example.test", (false, false)),
    // The member by its bare name in its mob: no rule names it so.
    ("ivan@example.test", (false, false)),
    // Every agent, and no send in the nested mob listed in `mobs`: that
    // deny selects no member of the mob it is nested under.
    ("judy@example.test", (true, true)),
];

fn rules() -> AccessController {
    let by_mob_label = || BTreeMap::from([("source_mob_id".to_string(), TEAM.to_string())]);
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", "carol@example.test", &["agent.*"])
            },
            AccessRule {
                agents: vec![ONE_STRING.to_string()],
                ..rule("by-the-string", "dave@example.test", &[ACTION_AGENT_VIEW])
            },
            AccessRule {
                match_labels: by_mob_label(),
                ..rule("by-mob-label", "erin@example.test", &[ACTION_AGENT_VIEW])
            },
            AccessRule {
                agents: vec!["*".to_string()],
                mobs: vec![TEAM.to_string()],
                ..rule("by-mobs", "frank@example.test", &[ACTION_AGENT_VIEW])
            },
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent-too", "grace@example.test", &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                match_labels: by_mob_label(),
                ..rule("no-send-there", "grace@example.test", &[ACTION_AGENT_SEND])
            },
            AccessRule {
                agents: vec![NAME.to_string()],
                ..rule("by-the-name", "ivan@example.test", &[ACTION_AGENT_VIEW])
            },
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent-three", "judy@example.test", &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec!["*".to_string()],
                mobs: vec![TEAM.to_string()],
                ..rule(
                    "no-send-in-that-mob",
                    "judy@example.test",
                    &[ACTION_AGENT_SEND],
                )
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    // The host has said which mob is its default mob, so a rule that lists
    // mobs is read against that mob's id.
    access.bind_default_mob(&crate::member_key::DefaultMob::new(MobId::from("launch")));
    access
}

/// What the rules decide for each person of [`PERSONS`], for a view and a
/// send of the member `name` in the view `of` gives of a person.
fn decisions(
    access: &AccessController,
    name: &str,
    of: impl Fn(AccessView) -> AccessView,
) -> Vec<(bool, bool)> {
    PERSONS
        .iter()
        .map(|(person, _)| {
            let view = of(view_of(access, person));
            (
                view.allows_agent(ACTION_AGENT_VIEW, name),
                view.allows_agent(ACTION_AGENT_SEND, name),
            )
        })
        .collect()
}

/// A member of a mob and a member of a mob nested under it whose names the
/// door spells with one string are two entries: each is decided by what was
/// recorded of it, whichever was recorded last. The outer view reads the
/// outer mob's member under the string; the nested view reads the nested
/// one, by its name there and by the door's string; a name only the outer
/// mob holds is no member of the nested mob.
#[test]
fn a_member_and_a_nested_member_of_one_spelling_are_two_entries() {
    for nested_first in [false, true] {
        let access = AccessController::audience_only();
        let record_nested = || record_nested(&access, NAME, Some(ALICE), &[]);
        let record_outer = || access.record_agent_attributes(outer_member(ONE_STRING, Some(BOB)));
        if nested_first {
            record_nested();
            record_outer();
        } else {
            record_outer();
            record_nested();
        }
        access.record_agent_attributes(outer_member("alpha", None));
        let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
        assert!(
            alice.hides_agent(ONE_STRING) && !bob.hides_agent(ONE_STRING),
            "nested first {nested_first}: the outer mob's member is its own person's"
        );
        for name in [NAME, ONE_STRING] {
            let (owners, others) = (alice.in_nested_mob(&team()), bob.in_nested_mob(&team()));
            assert!(
                owners.knows_agent(name) && !owners.hides_agent(name) && others.hides_agent(name),
                "nested first {nested_first}: the nested member, asked as {name}, is its own \
                 person's"
            );
        }
        let nested = alice.in_nested_mob(&team());
        assert!(
            !nested.knows_agent("alpha")
                && !nested.hides_agent("alpha")
                && nested.allows_agent(ACTION_AGENT_VIEW, "alpha")
                    == nested.allows_agent(ACTION_AGENT_VIEW, "never-recorded"),
            "nested first {nested_first}: a member of the outer mob is no member of the nested \
             mob"
        );
    }
}

/// A member of a mob and a member of a mob nested under it of one BARE name
/// are two entries too: the outer view reads the outer mob's member under
/// the name, never the nested one; the nested view reads the nested one;
/// and the door's string, which no member of the outer mob holds, names the
/// nested member in the outer view.
#[test]
fn a_bare_name_names_the_outer_mobs_member_in_the_outer_view() {
    for nested_first in [false, true] {
        let access = AccessController::audience_only();
        let record_nested = || record_nested(&access, NAME, Some(ALICE), &[]);
        let record_outer = || access.record_agent_attributes(outer_member(NAME, Some(BOB)));
        if nested_first {
            record_nested();
            record_outer();
        } else {
            record_outer();
            record_nested();
        }
        let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
        assert!(
            alice.hides_agent(NAME) && !bob.hides_agent(NAME),
            "nested first {nested_first}: the outer view reads the outer mob's member"
        );
        let (owners, others) = (alice.in_nested_mob(&team()), bob.in_nested_mob(&team()));
        assert!(
            !owners.hides_agent(NAME) && others.hides_agent(NAME),
            "nested first {nested_first}: the nested view reads the nested mob's member"
        );
        assert!(
            !alice.hides_agent(ONE_STRING) && bob.hides_agent(ONE_STRING),
            "nested first {nested_first}: the door's string names the nested member in the \
             outer view"
        );
    }
}

/// The rules reach a nested member exactly as before it had a key space of
/// its own: an allow without `mobs` grants it, by every agent, by the
/// door's string and by the label of its mob; its bare name in its mob is
/// no name a rule reaches it by; a rule that lists the nested mob in
/// `mobs`, an allow or a deny, does not select it (it is asked as a member
/// of the mob it is nested under); a deny holds. Each person is decided the
/// same for the member recorded in the nested mob's key space, asked of
/// the nested view by its name there or by the door's string, or of the
/// outer view by the door's string, as for the member recorded the former
/// way.
#[test]
fn the_rules_decide_a_nested_member_as_they_did() {
    let expected = PERSONS
        .iter()
        .map(|(_, decided)| *decided)
        .collect::<Vec<_>>();
    let former = rules();
    former.record_agent_attributes(former_record(NAME, None, &[]));
    assert_eq!(
        decisions(&former, ONE_STRING, |view| view),
        expected,
        "the fixture: what the rules decided for the member recorded the former way"
    );
    let access = rules();
    record_nested(&access, NAME, None, &[]);
    for name in [NAME, ONE_STRING] {
        assert_eq!(
            decisions(&access, name, |view| view.in_nested_mob(&team())),
            expected,
            "the member in the nested mob's key space, asked of the nested view as {name}"
        );
    }
    assert_eq!(
        decisions(&access, ONE_STRING, |view| view),
        expected,
        "the member in the nested mob's key space, asked of the outer view"
    );
}

/// The outer view reads a nested member as before while no member of the
/// outer mob holds its string: by the door's string it is known, its own
/// person's, and hidden from another person, so a reader that takes no
/// nested view is answered for it as it was. Its bare name is no name of it
/// in the outer view, as before, also where two nested mobs each have a
/// member of that name.
#[test]
fn the_outer_view_reads_a_nested_member_by_the_doors_string() {
    let access = AccessController::audience_only();
    record_nested(&access, NAME, Some(ALICE), &[]);
    let other_team = MobId::from("other-team");
    access
        .record_nested_agent_attributes(
            NestedMobRef {
                under: None,
                mob: &other_team,
            },
            NAME,
            Some("worker".to_string()),
            labels(Some(BOB), &[("source_mob_id", "other-team")]),
        )
        .expect("the door can name the member");
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    assert!(
        alice.knows_agent(ONE_STRING)
            && !alice.hides_agent(ONE_STRING)
            && bob.hides_agent(ONE_STRING),
        "the nested member, asked of the outer view by the door's string, is known and its \
         own person's"
    );
    assert!(
        bob.knows_agent("other-team/helper")
            && !bob.hides_agent("other-team/helper")
            && alice.hides_agent("other-team/helper"),
        "the other nested mob's member likewise"
    );
    assert!(
        !alice.knows_agent(NAME) && !bob.knows_agent(NAME),
        "a bare name that two nested mobs' members have names no member in the outer view"
    );
    let former = AccessController::audience_only();
    former.record_agent_attributes(former_record(NAME, Some(ALICE), &[]));
    assert!(
        !view_of(&former, ALICE).knows_agent(NAME),
        "the fixture: the bare name named no member in the outer view before"
    );
}

/// A nested member that has ended keeps the entry that was recorded of its
/// end: it is known, its own person's still, in its mob's key space and in
/// the outer view, and a later record of another member does not take it
/// away.
#[test]
fn an_ended_nested_member_keeps_its_entry() {
    let access = AccessController::audience_only();
    access
        .record_ended_nested_agent_attributes(
            NestedMobRef {
                under: None,
                mob: &team(),
            },
            NAME,
            labels(Some(ALICE), &[]),
        )
        .expect("the door can name the member");
    record_nested(&access, "another", None, &[]);
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    for name in [NAME, ONE_STRING] {
        let (owners, others) = (alice.in_nested_mob(&team()), bob.in_nested_mob(&team()));
        assert!(
            owners.knows_agent(name) && !owners.hides_agent(name) && others.hides_agent(name),
            "the ended nested member, asked of the nested view as {name}, is known and its \
             own person's"
        );
    }
    assert!(
        alice.knows_agent(ONE_STRING)
            && !alice.hides_agent(ONE_STRING)
            && bob.hides_agent(ONE_STRING),
        "the ended nested member is read by the outer view under the door's string"
    );
}

/// A live nested member of which what a spawn registered cannot be told is
/// recorded as such in its mob's key space: its role and its own labels
/// are known, so it is its own person's, and a rule keyed on a label it
/// does not hold is decided closed for it, as for such a member of the
/// outer mob.
#[test]
fn a_nested_member_without_spawn_facts_is_recorded_as_such() {
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", ALICE, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                match_labels: BTreeMap::from([("tier".to_string(), "closed".to_string())]),
                ..rule("no-closed-tier", ALICE, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    };
    let former = AccessController::new(config.clone()).expect("the rules are valid");
    former.record_agent_spawn_facts_not_known(former_record(NAME, Some(ALICE), &[]));
    let access = AccessController::new(config).expect("the rules are valid");
    access
        .record_nested_agent_spawn_facts_not_known(
            NestedMobRef {
                under: None,
                mob: &team(),
            },
            NAME,
            Some("worker".to_string()),
            nested_labels(Some(ALICE), &[]),
        )
        .expect("the door can name the member");
    let before = {
        let view = view_of(&former, ALICE);
        (
            view.hides_agent(ONE_STRING),
            view.can_view_agent(ONE_STRING),
        )
    };
    assert_eq!(
        before,
        (false, false),
        "the fixture: a deny on a label that is not held refuses a member whose spawn facts \
         are not known"
    );
    for name in [NAME, ONE_STRING] {
        let view = view_of(&access, ALICE).in_nested_mob(&team());
        assert_eq!(
            (view.hides_agent(name), view.can_view_agent(name)),
            before,
            "the nested member without spawn facts, asked of the nested view as {name}"
        );
    }
    assert!(
        view_of(&access, BOB)
            .in_nested_mob(&team())
            .hides_agent(NAME),
        "it is its own person's"
    );
}

/// A nested member's ancestors are looked up where the console's record of
/// them says they sit: the console writes a spawner of the nested mob by
/// its mob and its name, so a parent of the nested mob's own form
/// (`<mob>/<name>`) names a member of that mob, and every other name a
/// member of the mob it is nested under (the member that made the mob, and
/// that member's ancestors). So a nested member whose maker is another
/// person's own is hidden from everyone else, also when the nested mob has
/// a member whose name there is the maker's name; a deny on the maker
/// reaches it; an ancestor no entry describes is decided as before; and a
/// parent of the nested mob's own form is that mob's member. Where a
/// member of the outer mob holds the same string, the label cannot tell
/// which of the two made the child: both are its ancestors, and the one
/// that is another person's own hides it.
#[test]
fn a_nested_members_ancestors_are_read_in_the_mob_they_sit_in() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", ALICE, &["agent.*"])
            },
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent-too", BOB, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec!["denied-maker".to_string()],
                ..rule("not-that-maker", ALICE, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    };
    let nested_members = [
        ("of-a-private-maker", "maker"),
        ("of-a-denied-maker", "denied-maker"),
        ("of-an-unknown-maker", "never-recorded"),
    ];
    let record_outer = |access: &AccessController| {
        access.record_agent_attributes(outer_member("maker", Some(ALICE)));
        access.record_agent_attributes(outer_member("denied-maker", None));
    };
    let former = AccessController::new(config.clone()).expect("the rules are valid");
    record_outer(&former);
    let access = AccessController::new(config).expect("the rules are valid");
    record_outer(&access);
    // The twin: a member of the nested mob whose name there is the outer
    // maker's name, every person's and made by nobody.
    record_nested(&access, "maker", None, &[]);
    for (name, maker) in nested_members {
        former.record_agent_attributes(former_record(name, None, &[(spawned_by, maker)]));
        record_nested(&access, name, None, &[(spawned_by, maker)]);
    }
    let seen = |access: &AccessController, person: &str, nested: bool, name: &str| {
        let view = view_of(access, person);
        let view = if nested {
            view.in_nested_mob(&team())
        } else {
            view
        };
        (view.hides_agent(name), view.can_view_agent(name))
    };
    let expected = [
        // (the member, the person, (hidden, viewed))
        ("of-a-private-maker", ALICE, (false, true)),
        ("of-a-private-maker", BOB, (true, false)),
        ("of-a-denied-maker", ALICE, (false, false)),
        ("of-a-denied-maker", BOB, (false, true)),
    ];
    for (name, person, decided) in expected {
        let one_string = format!("{TEAM}/{name}");
        assert_eq!(
            seen(&former, person, false, &one_string),
            decided,
            "the fixture: {name} recorded the former way, for {person}"
        );
        assert_eq!(
            seen(&access, person, true, name),
            decided,
            "{name} in the nested mob's key space, for {person}"
        );
        assert_eq!(
            seen(&access, person, false, &one_string),
            decided,
            "{name} asked of the outer view, for {person}"
        );
    }
    for person in [ALICE, BOB] {
        assert_eq!(
            seen(&access, person, true, "of-an-unknown-maker"),
            seen(&former, person, false, "pair-team/of-an-unknown-maker"),
            "a nested member whose maker no entry describes, for {person}"
        );
    }
    // A parent of the nested mob's own form: the nested mob's member.
    record_nested(&access, "sibling", None, &[]);
    record_nested(&access, "child", None, &[(spawned_by, "pair-team/sibling")]);
    assert_eq!(
        seen(&access, ALICE, true, "child"),
        (false, true),
        "a parent of the nested mob's own form is that mob's member"
    );
    // A member of the outer mob, another person's own, holds the same
    // string: it is an ancestor too, in the nested view and in the outer
    // view alike.
    access.record_agent_attributes(outer_member("pair-team/sibling", Some(BOB)));
    for (person, decided) in [(ALICE, (true, false)), (BOB, (false, true))] {
        assert_eq!(
            (
                seen(&access, person, true, "child"),
                seen(&access, person, false, "pair-team/child"),
            ),
            (decided, decided),
            "a parent's string that two members hold names both, for {person}"
        );
    }
}

/// Inside a nested view a string that begins with the nested mob's id and
/// `/` is the door's string of a member of that mob; any other string is a
/// name there. A member of the nested mob whose own name there begins so is
/// named by the door's string of it alone. Each of two such members is
/// decided by its own record, in either order of recording.
#[test]
fn a_nested_view_reads_the_doors_string_and_the_name_there() {
    for longer_first in [false, true] {
        let access = AccessController::audience_only();
        let record_short = || record_nested(&access, NAME, Some(ALICE), &[]);
        // A member of the nested mob whose name there is `pair-team/helper`.
        let record_long = || record_nested(&access, ONE_STRING, Some(BOB), &[]);
        if longer_first {
            record_long();
            record_short();
        } else {
            record_short();
            record_long();
        }
        let (alice, bob) = (
            view_of(&access, ALICE).in_nested_mob(&team()),
            view_of(&access, BOB).in_nested_mob(&team()),
        );
        for name in [NAME, ONE_STRING] {
            assert!(
                !alice.hides_agent(name) && bob.hides_agent(name),
                "longer first {longer_first}: {name} names the member `helper`"
            );
        }
        assert!(
            alice.hides_agent("pair-team/pair-team/helper")
                && !bob.hides_agent("pair-team/pair-team/helper"),
            "longer first {longer_first}: the longer string names the member whose name \
             there begins with the mob's id"
        );
    }
}

/// The outer mob may be a mob of the host other than its default mob, and
/// one nested mob's id under two outer mobs is two key spaces. The default
/// mob named by its id and by no id is one outer mob once the host has said
/// which mob it is. A mob that a member of a nested mob made is recorded
/// under the same mob of the host: a nested view taken of a nested view
/// names its mob anew. A string of another nested mob's form is no member
/// of this one, and a nested mob's members are no members of a mob of the
/// host of that id.
#[test]
fn a_nested_mob_is_keyed_by_the_mob_of_the_host_it_is_under() {
    let hosted = MobId::from("hosted-mob");
    let launch = MobId::from("launch");
    let access = AccessController::audience_only();
    access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
    access
        .record_nested_agent_attributes(
            NestedMobRef {
                under: Some(&launch),
                mob: &team(),
            },
            NAME,
            Some("worker".to_string()),
            nested_labels(Some(ALICE), &[]),
        )
        .expect("the door can name the member");
    access
        .record_nested_agent_attributes(
            NestedMobRef {
                under: Some(&hosted),
                mob: &team(),
            },
            NAME,
            Some("worker".to_string()),
            nested_labels(Some(BOB), &[]),
        )
        .expect("the door can name the member");
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    for name in [NAME, ONE_STRING] {
        assert!(
            !alice.in_nested_mob(&team()).hides_agent(name)
                && bob.in_nested_mob(&team()).hides_agent(name),
            "under the default mob, recorded by its id and read by no id, {name} is alice's"
        );
        assert!(
            alice
                .in_mob(&hosted)
                .in_nested_mob(&team())
                .hides_agent(name)
                && !bob.in_mob(&hosted).in_nested_mob(&team()).hides_agent(name),
            "under the hosted mob, {name} is bob's"
        );
        let anew = MobId::from("another-team");
        assert!(
            !alice
                .in_nested_mob(&anew)
                .in_nested_mob(&team())
                .hides_agent(name)
                && bob
                    .in_nested_mob(&anew)
                    .in_nested_mob(&team())
                    .hides_agent(name),
            "a nested view of a nested view names its mob anew: {name}"
        );
    }
    let nested = alice.in_nested_mob(&team());
    assert!(
        !nested.knows_agent("another-team/helper"),
        "a string of another nested mob's form is no member of this one"
    );
    assert!(
        !alice.in_mob(&team()).knows_agent(NAME) && !alice.in_mob(&team()).knows_agent(ONE_STRING),
        "a nested mob's members are no members of a mob of the host of that id"
    );
}

/// Forgetting a mob forgets what is recorded of it and nothing of another
/// mob: its agents as a mob of the host, its members as a mob nested under
/// the default mob and under another mob of the host, and the current
/// runtime aliases of its identities. The default mob's entries, another
/// mob's entries and aliases, and the entries of a mob nested under the
/// forgotten mob stay. An id that names the default mob forgets nothing.
#[test]
fn forgetting_a_mob_forgets_its_own_entries_and_aliases_and_no_other_mobs() {
    let (launch, hosted, other, team) = (
        MobId::from("launch"),
        MobId::from("hosted-mob"),
        MobId::from("other-mob"),
        team(),
    );
    let access = AccessController::audience_only();
    access.bind_default_mob(&crate::member_key::DefaultMob::new(launch.clone()));
    access.record_agent_attributes(outer_member("desk", None));
    for name in ["desk", "courier"] {
        access.record_agent_attributes_in(&hosted, outer_member(name, None));
    }
    access.record_agent_attributes_in(&other, outer_member("scout", None));
    access.record_current_runtime_aliases_in(
        &hosted,
        [("desk".to_string(), Some("rt:desk:1".to_string()))],
    );
    access.record_current_runtime_aliases_in(
        &other,
        [("scout".to_string(), Some("rt:scout:1".to_string()))],
    );
    // The hosted mob's id as a mob nested under the default mob and under
    // the other mob; a mob nested under the hosted mob; and one nested
    // under the default mob.
    for (under, mob) in [
        (&launch, &hosted),
        (&other, &hosted),
        (&hosted, &team),
        (&launch, &team),
    ] {
        access
            .record_nested_agent_attributes(
                NestedMobRef {
                    under: Some(under),
                    mob,
                },
                NAME,
                Some("worker".to_string()),
                BTreeMap::new(),
            )
            .expect("the door can name the member");
    }
    // What is recorded for a mob of the host (none: the default mob): its
    // own agents, and the members of each mob nested under it.
    let recorded = |mob: Option<&MobId>| access.recorded_identities_in_tests(mob);
    let names = |list: &[&str]| -> Vec<String> { list.iter().map(ToString::to_string).collect() };
    let nested = |mobs: &[(&str, &str)]| -> BTreeMap<String, Vec<String>> {
        mobs.iter()
            .map(|(mob, member)| ((*mob).to_string(), vec![(*member).to_string()]))
            .collect()
    };
    // How many links the alias `alias` names in the view of `mob`: none
    // unless it is the current alias of an identity recorded there.
    let alias_links = |mob: &MobId, alias: &str| {
        access
            .view_for_subject(None)
            .in_mob(mob)
            .lineage_links(alias)
    };
    assert_eq!(
        (
            recorded(None),
            recorded(Some(&hosted)),
            recorded(Some(&other)),
            alias_links(&hosted, "rt:desk:1"),
            alias_links(&other, "rt:scout:1"),
        ),
        (
            (
                names(&["desk"]),
                nested(&[("hosted-mob", "hosted-mob/helper"), (TEAM, ONE_STRING)])
            ),
            (names(&["courier", "desk"]), nested(&[(TEAM, ONE_STRING)])),
            (
                names(&["scout"]),
                nested(&[("hosted-mob", "hosted-mob/helper")])
            ),
            1,
            1,
        ),
        "the scene: (the default mob, the hosted mob, the other mob, the hosted mob's alias, \
         the other mob's alias)"
    );

    access.forget_mob(&hosted);
    // The mob created under the id is read: its `desk` is recorded anew,
    // and no alias of it.
    access.record_agent_attributes_in(&hosted, outer_member("desk", None));
    assert_eq!(
        (
            recorded(None),
            recorded(Some(&hosted)),
            recorded(Some(&other)),
            alias_links(&hosted, "rt:desk:1"),
            alias_links(&other, "rt:scout:1"),
        ),
        (
            (names(&["desk"]), nested(&[(TEAM, ONE_STRING)])),
            (names(&["desk"]), nested(&[(TEAM, ONE_STRING)])),
            (names(&["scout"]), nested(&[])),
            0,
            1,
        ),
        "after the forget: the hosted mob's entries and alias went, as a mob of the host and \
         as a nested mob; every other mob's stay, the mob nested under it among them"
    );

    access.forget_mob(&launch);
    assert_eq!(
        recorded(None),
        (names(&["desk"]), nested(&[(TEAM, ONE_STRING)])),
        "an id that names the default mob forgets nothing"
    );
}

/// A member the door of the outer mob can give no name (its name there is
/// empty, or its mob's id holds the separator) is not recorded, and the
/// caller is told so: it decides what stands for the member.
#[test]
fn a_nested_member_the_door_cannot_name_is_refused_to_its_recorder() {
    let access = AccessController::audience_only();
    let unnamed = MobId::from("pair/team");
    let recorded = [
        access.record_nested_agent_attributes(
            NestedMobRef {
                under: None,
                mob: &team(),
            },
            "",
            None,
            labels(Some(ALICE), &[]),
        ),
        access.record_ended_nested_agent_attributes(
            NestedMobRef {
                under: None,
                mob: &unnamed,
            },
            NAME,
            labels(Some(ALICE), &[]),
        ),
        access.record_nested_agent_spawn_facts_not_known(
            NestedMobRef {
                under: None,
                mob: &unnamed,
            },
            NAME,
            None,
            labels(Some(ALICE), &[]),
        ),
    ];
    assert_eq!(recorded, [Err(NestedMemberUnnamed); 3]);
    assert!(
        !view_of(&access, ALICE).knows_agent("pair/team/helper"),
        "nothing was recorded"
    );
}

/// The entry the rules of the kept walk's row read: a config in which a
/// person is allowed every agent and denied every agent that carries the
/// label `tier: closed`. A member of whom not every label is known cannot
/// be ruled out of the deny, and is refused.
fn closed_tier_rules(person: &str) -> AccessControlConfig {
    AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", person, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                match_labels: BTreeMap::from([("tier".to_string(), "closed".to_string())]),
                ..rule("no-closed-tier", person, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    }
}

/// The nested mob under the host's default mob.
fn under_the_default_mob(mob: &MobId) -> NestedMobRef<'_> {
    NestedMobRef { under: None, mob }
}

/// A row of a walk that was made earlier and kept (the console
/// experience's snapshot) may be older than what the cache holds of a
/// nested member. It never takes the place of an entry of that member that
/// is not whole: one recorded as ended, or without its spawn facts, stays,
/// and a rule keyed on a label that is not held is still decided closed
/// for it. A whole entry it replaces, and a member the cache does not know
/// it records. Read in the nested view by the name there and by the door's
/// string, and in the outer view by the door's string.
#[test]
fn a_kept_walks_row_of_a_nested_member_replaces_a_whole_entry_alone() {
    let team = team();
    let former = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    former.record_ended_agent_attributes(AgentResourceAttributes {
        role: None,
        labels: labels(Some(ALICE), &[]),
        ..former_record(NAME, None, &[])
    });
    former.record_snapshot_agent_attributes(former_record(NAME, Some(ALICE), &[]));
    assert!(
        !view_of(&former, ALICE).can_view_agent(ONE_STRING),
        "the fixture: a kept walk's row did not take the place of an ended member's entry"
    );
    let access = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    access
        .record_ended_nested_agent_attributes(
            under_the_default_mob(&team),
            "ended",
            labels(Some(ALICE), &[]),
        )
        .expect("the door can name the member");
    access
        .record_nested_agent_spawn_facts_not_known(
            under_the_default_mob(&team),
            "half-known",
            Some("worker".to_string()),
            nested_labels(Some(ALICE), &[]),
        )
        .expect("the door can name the member");
    record_nested(&access, "whole", Some(ALICE), &[]);
    // The kept walk's rows: each member whole as the walk saw it, with no
    // label of a tier. The whole member's row is of another person's.
    for (name, private_to) in [
        ("ended", ALICE),
        ("half-known", ALICE),
        ("whole", BOB),
        ("new", ALICE),
    ] {
        access
            .record_snapshot_nested_agent_attributes(
                under_the_default_mob(&team),
                name,
                Some("worker".to_string()),
                nested_labels(Some(private_to), &[]),
            )
            .expect("the door can name the member");
    }
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    let read = |name: &str| {
        let one_string = format!("{TEAM}/{name}");
        let nested = alice.in_nested_mob(&team);
        [
            (
                nested.knows_agent(name),
                nested.hides_agent(name),
                nested.can_view_agent(name),
            ),
            (
                nested.knows_agent(&one_string),
                nested.hides_agent(&one_string),
                nested.can_view_agent(&one_string),
            ),
            (
                alice.knows_agent(&one_string),
                alice.hides_agent(&one_string),
                alice.can_view_agent(&one_string),
            ),
        ]
    };
    // (known, hidden from its person, viewed by its person)
    assert_eq!(
        read("ended"),
        [(true, false, false); 3],
        "the ended member keeps its entry: the deny on a label is not ruled out"
    );
    assert_eq!(
        read("half-known"),
        [(true, false, false); 3],
        "the member without spawn facts keeps its entry"
    );
    assert_eq!(
        read("whole"),
        [(true, true, false); 3],
        "the whole member's entry is replaced by the walk's row: it is another person's now"
    );
    assert!(
        !bob.in_nested_mob(&team).hides_agent("whole") && !bob.hides_agent("pair-team/whole"),
        "and that person's"
    );
    assert_eq!(
        read("new"),
        [(true, false, true); 3],
        "a member the cache did not know is recorded by the walk's row"
    );
    assert_eq!(
        access.record_snapshot_nested_agent_attributes(
            under_the_default_mob(&MobId::from("pair/team")),
            NAME,
            None,
            labels(Some(ALICE), &[]),
        ),
        Err(NestedMemberUnnamed)
    );
}

/// The reads that decide on attributes held under one lock (the decision
/// fence of a stored record's mob, and the held view of a list) answer a
/// nested member, named by the door's string, as the view's own methods
/// do, for its person and for another: its own entry is read in its mob's
/// key space and its ancestors where they sit. The member's maker is a
/// member of the outer mob, private to a person; a member of the nested
/// mob has the maker's name and is every person's.
#[test]
fn the_held_reads_answer_a_nested_member_as_the_view_does() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let access = AccessController::audience_only();
    access.record_agent_attributes(outer_member("maker", Some(ALICE)));
    record_nested(&access, "maker", None, &[]);
    record_nested(&access, NAME, None, &[(spawned_by, "maker")]);
    let snapshot = AgentResourceAttributes {
        identity: ONE_STRING.to_string(),
        agent_id: Some(ONE_STRING.to_string()),
        role: Some("worker".to_string()),
        labels: nested_labels(None, &[(spawned_by, "maker")]),
    };
    for (person, viewed) in [(ALICE, true), (BOB, false)] {
        let (fenced_view, fenced_origin) = {
            let fence = access.decision_fence(Some(person), Some(&own_key(person)));
            (
                fence.can_view_agent_in(None, ONE_STRING),
                fence.can_see_origin_in(None, ONE_STRING, None),
            )
        };
        let view = view_of(&access, person);
        let (held_refuses, held_snapshot) = {
            let held = view.over_held_attributes();
            (
                held.refuses_view_of_known_agent(ONE_STRING),
                held.decide_agent_with_attributes(ACTION_AGENT_VIEW, &snapshot)
                    .is_allow(),
            )
        };
        assert_eq!(
            (
                view.can_view_agent(ONE_STRING),
                fenced_view,
                !held_refuses,
                held_snapshot,
                view.decide_agent_with_attributes(ACTION_AGENT_VIEW, &snapshot)
                    .is_allow(),
            ),
            (viewed, viewed, viewed, viewed, viewed),
            "the view, the fence, the held view, and the member's own snapshot on the held view \
             and on the view, for {person}"
        );
        assert_eq!(
            (fenced_origin, view.sees_origin(ONE_STRING, None)),
            (viewed, viewed),
            "a record of the member as an origin, on the fence and on the view, for {person}"
        );
    }
}

/// A record a nested member wrote is decided by what was captured with it,
/// and an ancestor the cache knows now may add a deny. The captured
/// ancestor is the member of the outer mob that made the nested member: a
/// deny that matches that member now refuses the record; a deny that
/// matches only a member of the nested mob that has the maker's name
/// refuses nothing. In the outer view and in the nested view alike.
#[test]
fn a_captured_record_of_a_nested_member_is_refused_by_a_deny_on_its_outer_ancestor_now() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "maker")])),
        },
        CapturedLineageLink {
            identity: "maker".to_string(),
            attributes: Some(outer_member("maker", None)),
        },
    ];
    let closed = |identity: &str| AgentResourceAttributes {
        labels: labels(None, &[("tier", "closed")]),
        ..outer_member(identity, None)
    };
    let viewed = |access: &AccessController| {
        let alice = view_of(access, ALICE);
        (
            alice.can_view_captured_member(&captured),
            alice
                .in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    let access = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    access.record_agent_attributes(outer_member("maker", None));
    assert_eq!(
        viewed(&access),
        (true, true),
        "the fixture: the record is viewed while no deny matches its ancestor"
    );
    record_nested(&access, "maker", None, &[("tier", "closed")]);
    assert_eq!(
        viewed(&access),
        (true, true),
        "a deny that matches only the nested mob's member of the maker's name refuses nothing"
    );
    access.record_agent_attributes(closed("maker"));
    assert_eq!(
        viewed(&access),
        (false, false),
        "a deny that matches the maker now refuses the record"
    );
}

/// A view of a nested mob taken again under the configuration committed
/// now is a view of that nested mob: a bare name still names that mob's
/// member, never the outer mob's member of that name. A nested mob whose
/// id is blank names no mob: a view taken for it is withheld from everyone
/// but an administrator, and so is that view taken again.
#[test]
fn a_refreshed_view_of_a_nested_mob_is_that_view() {
    let team = team();
    let access = AccessController::audience_only();
    access.record_agent_attributes(outer_member(NAME, Some(BOB)));
    record_nested(&access, NAME, Some(ALICE), &[]);
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    assert!(
        alice.refreshed().hides_agent(NAME) && !bob.refreshed().hides_agent(NAME),
        "the fixture: the outer view taken again reads the outer mob's member"
    );
    assert!(
        !alice.in_nested_mob(&team).refreshed().hides_agent(NAME)
            && bob.in_nested_mob(&team).refreshed().hides_agent(NAME),
        "the nested view taken again reads the nested mob's member"
    );
    let ruled = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            agents: vec!["*".to_string()],
            ..rule("every-agent", ALICE, &["agent.*"])
        }],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    let blank = MobId::from("  ");
    for (person, allowed) in [(ALICE, false), (ROOT, true)] {
        let view = view_of(&ruled, person);
        assert!(
            view.allows_agent(ACTION_AGENT_VIEW, "anyone"),
            "the fixture: the rules allow {person} a name no agent holds"
        );
        let withheld = view.in_nested_mob(&blank);
        assert_eq!(
            (
                withheld.allows_agent(ACTION_AGENT_VIEW, "anyone"),
                withheld
                    .refreshed()
                    .allows_agent(ACTION_AGENT_VIEW, "anyone"),
            ),
            (allowed, allowed),
            "a view of a nested mob with a blank id, and that view taken again, for {person}"
        );
    }
}

/// A view of a nested mob asks its records and the rules by one string in
/// every reading: the door's string of the member. A rule that names a
/// member of the outer mob by the bare name the nested member has there
/// never reaches the nested member, and a rule by the door's string does,
/// also in the readings that ask the rules about a name no entry decides:
/// a deny the name alone cannot decide, a record without captured
/// attributes, and a member that was a person's own under a name another
/// member holds now. Each is answered as the outer view answers it for the
/// door's string.
#[test]
fn a_nested_view_asks_the_rules_by_the_doors_string_in_every_reading() {
    let team = team();
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            // Alice: the member by the door's string, and no view of the
            // outer mob's agent `helper`.
            AccessRule {
                agents: vec![ONE_STRING.to_string()],
                ..rule("by-the-string", ALICE, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec![NAME.to_string()],
                ..rule("not-the-outer-helper", ALICE, &[ACTION_AGENT_VIEW])
            },
            // Bob: the outer mob's agent `helper`, and no view of the
            // member by the door's string.
            AccessRule {
                agents: vec![NAME.to_string()],
                ..rule("by-the-name", BOB, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec![ONE_STRING.to_string()],
                ..rule("not-the-nested-helper", BOB, &[ACTION_AGENT_VIEW])
            },
        ],
        ..AccessControlConfig::default()
    };
    let access = AccessController::new(config).expect("the rules are valid");
    // The nested member is a third person's own: neither Alice's nor Bob's
    // entry, so a reading of a member that was theirs decides by the rules.
    record_nested(&access, NAME, Some("carol@example.test"), &[]);
    for (person, undecidable, owned) in [(ALICE, true, true), (BOB, false, false)] {
        let outer = view_of(&access, person);
        let nested = outer.in_nested_mob(&team);
        let own = own_key(person);
        assert_eq!(
            (
                outer.has_deny_undecidable_by_identity(ACTION_AGENT_VIEW, ONE_STRING),
                outer.allows_owned_member(ACTION_AGENT_SEND, ONE_STRING, &own),
                outer
                    .over_held_attributes()
                    .refuses_view_of_known_agent(ONE_STRING),
            ),
            (undecidable, owned, true),
            "the fixture: the outer view asked by the door's string, for {person}"
        );
        for name in [NAME, ONE_STRING] {
            assert_eq!(
                (
                    nested.has_deny_undecidable_by_identity(ACTION_AGENT_VIEW, name),
                    nested.allows_owned_member(ACTION_AGENT_SEND, name, &own),
                    nested
                        .over_held_attributes()
                        .refuses_view_of_known_agent(name),
                ),
                (undecidable, owned, true),
                "the nested view asked as {name}, for {person}"
            );
        }
    }
    // A record without captured attributes: the one operator of a console
    // that identifies nobody is decided by the name alone. The member is
    // every person's here.
    let open = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                agents: vec!["*".to_string()],
                subjects: Vec::new(),
                ..rule("every-agent", "", &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec![NAME.to_string()],
                subjects: Vec::new(),
                ..rule("not-the-outer-helper", "", &[ACTION_AGENT_VIEW])
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    record_nested(&open, NAME, None, &[]);
    let operator = open.view_for_subject(None).on_console(false);
    let by_the_string = operator.can_view_uncaptured_member(ONE_STRING);
    assert!(
        !by_the_string,
        "the fixture: a deny that names another agent cannot be ruled out for the member"
    );
    for name in [NAME, ONE_STRING] {
        assert_eq!(
            operator
                .in_nested_mob(&team)
                .can_view_uncaptured_member(name),
            by_the_string,
            "a record without captured attributes, the nested view asked as {name}"
        );
    }
}

/// A generated alias of a nested member (the name of one incarnation of
/// it) is never the current one: nothing records a current alias for a
/// member of a nested mob. It is judged as the member for the question who
/// the member is private to, and refused as not current to everyone but an
/// administrator, in the nested view by the alias there and in the outer
/// view by the alias in the door's form. The roster's encoded spelling of
/// the name names the member in both.
#[test]
fn a_generated_alias_of_a_nested_member_is_never_current() {
    let team = team();
    let name = "review:helper";
    let access = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    record_nested(&access, name, Some(ALICE), &[]);
    let alias = "rt:review:helper:1";
    assert!(
        crate::member_comms_id::is_reserved_generated_alias(alias)
            && crate::member_comms_id::durable_identity_from_runtime_alias(alias).as_deref()
                == Some(name),
        "the fixture: the literal is a generated alias of the member's name"
    );
    let aliased = format!("{TEAM}/{alias}");
    let encoded = crate::member_comms_id::mob_member_id_str(name).into_owned();
    let encoded_there = format!("{TEAM}/{encoded}");
    // (the person, the member is hidden from it, it views the member by
    // the alias)
    let persons = [
        (ALICE, false, false),
        (BOB, true, false),
        (ROOT, false, true),
    ];
    let former = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    former.record_agent_attributes(former_record(name, Some(ALICE), &[]));
    for (person, hidden, alias_viewed) in persons {
        let view = view_of(&former, person);
        assert_eq!(
            (
                view.hides_agent(&aliased),
                view.can_view_agent(&aliased),
                view.knows_agent(&encoded_there),
                view.hides_agent(&encoded_there),
            ),
            (hidden, alias_viewed, true, hidden),
            "the fixture: the alias and the encoded spelling in the door's form, for the member \
             recorded the former way, for {person}"
        );
    }
    for (person, hidden, alias_viewed) in persons {
        let outer = view_of(&access, person);
        let nested = outer.in_nested_mob(&team);
        assert_eq!(
            (
                nested.hides_agent(alias),
                nested.can_view_agent(alias),
                outer.hides_agent(&aliased),
                outer.can_view_agent(&aliased),
            ),
            (hidden, alias_viewed, hidden, alias_viewed),
            "the alias in the nested view and in the door's form on the outer view, for {person}"
        );
        assert_eq!(
            (
                nested.knows_agent(&encoded),
                nested.hides_agent(&encoded),
                outer.knows_agent(&encoded_there),
                outer.hides_agent(&encoded_there),
            ),
            (true, hidden, true, hidden),
            "the encoded spelling in the nested view and in the door's form on the outer view, \
             for {person}"
        );
    }
}

/// A reader that holds a member's name in its nested mob asks the view for
/// the string the door names that member by, and is given the string its
/// recorder composed: the mob's id, `/`, and the name, whatever the name
/// begins with. Asked by that string each member is decided by its own
/// record. A view of no nested mob, and a member the door can give no
/// name, have no such string.
#[test]
fn the_name_there_is_composed_as_its_recorder_composes_it() {
    let team = team();
    let access = AccessController::audience_only();
    record_nested(&access, NAME, Some(ALICE), &[]);
    // A member of the nested mob whose name there is `pair-team/helper`.
    record_nested(&access, ONE_STRING, Some(BOB), &[]);
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    let nested = alice.in_nested_mob(&team);
    assert_eq!(nested.nested_member(NAME).as_deref(), Some(ONE_STRING));
    let longer = nested.nested_member(ONE_STRING);
    assert_eq!(longer.as_deref(), Some("pair-team/pair-team/helper"));
    let longer = longer.expect("the door can name the member");
    assert!(
        nested.hides_agent(&longer) && !bob.in_nested_mob(&team).hides_agent(&longer),
        "the member whose name there begins with the mob's id is read by its composed string"
    );
    assert_eq!(alice.nested_member(NAME), None, "a view of no nested mob");
    assert_eq!(nested.nested_member(""), None, "an empty name");
    assert_eq!(
        alice
            .in_nested_mob(&MobId::from("pair/team"))
            .nested_member(NAME),
        None,
        "a mob whose id the door cannot compose"
    );
}

/// A roster projection of the outer mob replaces that mob's entries and
/// evicts those it does not hold; the entries of a mob nested under it are
/// neither replaced nor evicted by it.
#[test]
fn a_roster_projection_of_the_outer_mob_leaves_a_nested_mobs_entries() {
    let team = team();
    let access = AccessController::audience_only();
    access.record_agent_attributes(outer_member("alpha", None));
    record_nested(&access, NAME, Some(ALICE), &[]);
    access.replace_agent_attributes([outer_member("beta", None)]);
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    assert!(
        !alice.knows_agent("alpha") && alice.knows_agent("beta"),
        "the fixture: the projection replaced the outer mob's entries"
    );
    for name in [NAME, ONE_STRING] {
        let (owners, others) = (alice.in_nested_mob(&team), bob.in_nested_mob(&team));
        assert!(
            owners.knows_agent(name) && !owners.hides_agent(name) && others.hides_agent(name),
            "the nested member, asked of the nested view as {name}, keeps its entry"
        );
    }
    assert!(
        alice.knows_agent(ONE_STRING) && bob.hides_agent(ONE_STRING),
        "and is read by the outer view under the door's string"
    );
}

/// An ancestor is told apart by the mob it sits in as well as by its
/// string. A member of the nested mob made by another member of that mob
/// that is a person's own is hidden from everyone else. And a lineage that
/// passes a member of the nested mob and, further up, the member of the
/// outer mob that the door spells with the same string holds both: the
/// walk does not stop at the second as at a link it has seen.
///
/// No row here holds a side chat of a nested member: a side chat names its
/// source by the source's name in its own roster and counts only with a
/// host's fork record of it, and no door forks a member of a nested mob.
#[test]
fn an_ancestor_is_told_apart_by_the_mob_it_sits_in() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let access = AccessController::audience_only();
    // A nested maker that is Bob's own, and its child.
    record_nested(&access, "sibling", Some(BOB), &[]);
    record_nested(&access, "child", None, &[(spawned_by, "pair-team/sibling")]);
    // A chain: `kid` of the nested mob, made by `x` of the nested mob, made
    // by `mid` of the outer mob, made by the outer mob's member that the
    // door spells `pair-team/x`, which is Bob's own.
    record_nested(&access, "kid", None, &[(spawned_by, "pair-team/x")]);
    record_nested(&access, "x", None, &[(spawned_by, "mid")]);
    access.record_agent_attributes(AgentResourceAttributes {
        labels: labels(None, &[(spawned_by, "pair-team/x")]),
        ..outer_member("mid", None)
    });
    access.record_agent_attributes(outer_member("pair-team/x", Some(BOB)));
    let (alice, bob) = (view_of(&access, ALICE), view_of(&access, BOB));
    for name in ["child", "kid"] {
        let one_string = format!("{TEAM}/{name}");
        assert_eq!(
            (
                alice.in_nested_mob(&team).hides_agent(name),
                alice.hides_agent(&one_string),
                bob.in_nested_mob(&team).hides_agent(name),
                bob.hides_agent(&one_string),
            ),
            (true, true, false, false),
            "{name} is hidden from everyone but the person an ancestor of it is private to"
        );
    }
}

/// A view of a nested mob names that mob's members and no agent of the mob
/// it is nested under: a name only the outer mob holds is no member there,
/// also in a view of a nested mob whose id the door cannot compose a
/// member's name with, and an empty name names no member.
#[test]
fn a_nested_view_names_no_agent_of_the_mob_it_is_nested_under() {
    let access = AccessController::audience_only();
    access.record_agent_attributes(outer_member("alpha", Some(BOB)));
    let alice = view_of(&access, ALICE);
    assert!(
        alice.knows_agent("alpha") && alice.hides_agent("alpha"),
        "the fixture: the outer mob's member is another person's own"
    );
    for mob in [TEAM, "pair/team"] {
        let nested = alice.in_nested_mob(&MobId::from(mob));
        for name in ["alpha", ""] {
            assert!(
                !nested.knows_agent(name) && !nested.hides_agent(name),
                "the name `{name}` in a view of the nested mob {mob} names no member"
            );
        }
    }
}

/// A door that reads one string two members can spell asks which of the two
/// the cache holds. The view of the outer mob says whether an agent of that
/// mob holds the string, and never answers for the nested member the string
/// spells; the view of the nested mob says whether a member of that mob
/// holds it, asked by its name there or by the door's string. An ended
/// agent is held as a live one is.
#[test]
fn a_view_tells_an_agent_of_its_own_mob_from_a_nested_member_of_that_string() {
    let team = team();
    let access = AccessController::audience_only();
    record_nested(&access, NAME, Some(ALICE), &[]);
    let alice = view_of(&access, ALICE);
    assert!(
        alice.knows_agent(ONE_STRING),
        "the fixture: the outer view reads the nested member under the door's string"
    );
    assert!(
        !alice.knows_own_agent(ONE_STRING),
        "no agent of the outer mob holds the string"
    );
    for name in [NAME, ONE_STRING] {
        assert!(
            alice.in_nested_mob(&team).knows_own_agent(name),
            "the nested mob holds its member, asked as {name}"
        );
    }
    access.record_ended_agent_attributes(AgentResourceAttributes {
        role: None,
        labels: labels(Some(BOB), &[]),
        ..outer_member(ONE_STRING, None)
    });
    assert!(
        alice.knows_own_agent(ONE_STRING),
        "an ended agent of the outer mob holds the string"
    );
    assert!(
        !alice.knows_own_agent("never-recorded")
            && !alice.in_nested_mob(&team).knows_own_agent("never-recorded"),
        "a name nothing was recorded under is held by neither"
    );
}

/// Access rules under which each of `allowed` may do everything to every
/// agent, and `denied` is refused every agent that carries the label
/// `tier: closed`.
fn every_agent_but_the_closed_tier(allowed: &[&str], denied: &str) -> AccessControlConfig {
    let mut rules = allowed
        .iter()
        .map(|person| AccessRule {
            agents: vec!["*".to_string()],
            ..rule(&format!("every-agent-{person}"), person, &["agent.*"])
        })
        .collect::<Vec<_>>();
    rules.push(AccessRule {
        effect: AccessEffect::Deny,
        match_labels: BTreeMap::from([("tier".to_string(), "closed".to_string())]),
        ..rule("no-closed-tier", denied, &["agent.*"])
    });
    AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules,
        ..AccessControlConfig::default()
    }
}

/// A label that names a parent holds a string and nothing that says in
/// which mob the parent sits. Where a member of the outer mob holds the
/// string AND a member of a nested mob is spelled by it, the two cannot be
/// told apart, and both are the child's ancestors for what refuses: the one
/// that is another person's own hides the child, and a deny that matches
/// either refuses it. In the view of the nested mob and in the view of the
/// outer mob alike.
#[test]
fn a_parents_string_that_two_members_hold_names_both_for_an_owner_and_a_deny() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    // (hidden, viewed) of the child, in the view of its mob and in the
    // outer view.
    let seen = |access: &AccessController, person: &str| {
        let view = view_of(access, person);
        let nested = view.in_nested_mob(&team);
        [
            (nested.hides_agent("child"), nested.can_view_agent("child")),
            (
                view.hides_agent("pair-team/child"),
                view.can_view_agent("pair-team/child"),
            ),
        ]
    };
    let with_the_child = || {
        let access = AccessController::new(every_agent_but_the_closed_tier(&[ALICE, BOB], ALICE))
            .expect("the rules are valid");
        record_nested(&access, "child", None, &[(spawned_by, "pair-team/sibling")]);
        access
    };
    // The private owner, held by the outer mob's member of the string.
    let access = with_the_child();
    record_nested(&access, "sibling", None, &[]);
    assert_eq!(
        seen(&access, ALICE),
        [(false, true); 2],
        "the fixture: the child of the nested mob's member alone is viewed"
    );
    access.record_agent_attributes(outer_member("pair-team/sibling", Some(BOB)));
    assert_eq!(
        (seen(&access, ALICE), seen(&access, BOB)),
        ([(true, false); 2], [(false, true); 2]),
        "the outer mob's member of the string is another person's own: it hides the child"
    );
    // The private owner, held by the nested mob's member.
    let access = with_the_child();
    record_nested(&access, "sibling", Some(BOB), &[]);
    access.record_agent_attributes(outer_member("pair-team/sibling", None));
    assert_eq!(
        (seen(&access, ALICE), seen(&access, BOB)),
        ([(true, false); 2], [(false, true); 2]),
        "the nested mob's member of the string is another person's own: it hides the child"
    );
    // A deny that matches one of the two alone.
    let closed: &[(&str, &str)] = &[("tier", "closed")];
    let open: &[(&str, &str)] = &[];
    for closed_in_the_nested_mob in [false, true] {
        let access = with_the_child();
        record_nested(
            &access,
            "sibling",
            None,
            if closed_in_the_nested_mob {
                closed
            } else {
                open
            },
        );
        access.record_agent_attributes(AgentResourceAttributes {
            labels: labels(
                None,
                if closed_in_the_nested_mob {
                    open
                } else {
                    closed
                },
            ),
            ..outer_member("pair-team/sibling", None)
        });
        assert_eq!(
            (seen(&access, ALICE), seen(&access, BOB)),
            ([(false, false); 2], [(false, true); 2]),
            "a deny on one holder of the string refuses the child (the nested mob's member \
             holds the label: {closed_in_the_nested_mob})"
        );
    }
}

/// A parent's string that two members hold grants nothing: an ambiguous
/// name never grants. Neither holder's label, nor a rule that names the
/// string itself, nor a grant on an ancestor above them reaches the child.
/// While one member alone holds the string, the child takes its parent's
/// grants as before.
#[test]
fn a_parents_string_that_two_members_hold_grants_nothing() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let open_team = || BTreeMap::from([("team".to_string(), "open".to_string())]);
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            // Alice: every agent of the open team.
            AccessRule {
                match_labels: open_team(),
                ..rule("the-open-team", ALICE, &["agent.*"])
            },
            // Bob: the agent the parent's string names.
            AccessRule {
                agents: vec!["pair-team/sibling".to_string()],
                ..rule("by-the-parents-string", BOB, &["agent.*"])
            },
            // Carol: an ancestor above the parent.
            AccessRule {
                agents: vec!["maker".to_string()],
                ..rule("by-the-maker", "carol@example.test", &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    };
    let viewed = |access: &AccessController| {
        [ALICE, BOB, "carol@example.test"].map(|person| {
            let view = view_of(access, person);
            (
                view.in_nested_mob(&team).can_view_agent("child"),
                view.can_view_agent("pair-team/child"),
            )
        })
    };
    let access = AccessController::new(config).expect("the rules are valid");
    access.record_agent_attributes(outer_member("maker", None));
    record_nested(
        &access,
        "sibling",
        None,
        &[("team", "open"), (spawned_by, "maker")],
    );
    record_nested(&access, "child", None, &[(spawned_by, "pair-team/sibling")]);
    assert_eq!(
        viewed(&access),
        [(true, true); 3],
        "the fixture: the child of one member takes that member's grants, and its maker's"
    );
    access.record_agent_attributes(outer_member("pair-team/sibling", None));
    assert_eq!(
        viewed(&access),
        [(false, false); 3],
        "the string names two members: neither grants, nor does an ancestor above them"
    );
}

/// A member that a member made does not become an ancestor that grants to
/// the descendants of an agent of the outer mob. An agent of the outer mob
/// is the child of the outer mob's member `pair-team/x`. A mob `pair-team`
/// with a member `x` then spells that parent's string: its labels grant the
/// child nothing, the child's own parent grants it nothing any more (the
/// name is ambiguous), and a deny that matches the nested member refuses
/// the child.
#[test]
fn a_nested_member_is_no_ancestor_that_grants_to_an_outer_agents_descendants() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let open_team = || BTreeMap::from([("team".to_string(), "open".to_string())]);
    let config = || AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            // Alice: every agent of the open team.
            AccessRule {
                match_labels: open_team(),
                ..rule("the-open-team", ALICE, &["agent.*"])
            },
            // Bob: every agent but one of the closed tier.
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", BOB, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                match_labels: BTreeMap::from([("tier".to_string(), "closed".to_string())]),
                ..rule("no-closed-tier", BOB, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    };
    let kid = AgentResourceAttributes {
        labels: labels(None, &[(spawned_by, "pair-team/x")]),
        ..outer_member("kid", None)
    };
    let viewed = |access: &AccessController| {
        [ALICE, BOB].map(|person| view_of(access, person).can_view_agent("kid"))
    };
    // The nested member's label grants nothing.
    let access = AccessController::new(config()).expect("the rules are valid");
    access.record_agent_attributes(outer_member("pair-team/x", None));
    access.record_agent_attributes(kid.clone());
    assert_eq!(
        viewed(&access),
        [false, true],
        "the fixture: the kid of a parent outside the open team"
    );
    record_nested(&access, "x", None, &[("team", "open")]);
    assert_eq!(
        viewed(&access),
        [false, true],
        "the nested member's label grants the outer agent's child nothing"
    );
    // The kid's own parent grants nothing once its name is ambiguous.
    let access = AccessController::new(config()).expect("the rules are valid");
    access.record_agent_attributes(AgentResourceAttributes {
        labels: labels(None, &[("team", "open")]),
        ..outer_member("pair-team/x", None)
    });
    access.record_agent_attributes(kid);
    assert_eq!(
        viewed(&access),
        [true, true],
        "the fixture: the kid of a parent of the open team takes its grant"
    );
    record_nested(&access, "x", None, &[]);
    assert_eq!(
        viewed(&access),
        [false, true],
        "the parent's string names two members: the kid takes no grant through it"
    );
    // A deny that matches the nested member refuses the kid.
    record_nested(&access, "x", None, &[("tier", "closed")]);
    assert_eq!(
        viewed(&access),
        [false, false],
        "a deny on the nested member of the parent's string refuses the kid"
    );
}

/// The walk holds both holders of a string at its bound, and still tells a
/// lineage it walked to the end from one it left an ancestor out of. A
/// lineage of the bound's length whose last link names a parent is refused,
/// as before. One whose last parent string names two members holds one
/// link more than the bound: with no ancestor left it is decided, and with
/// an ancestor of either holder left it is refused.
#[test]
fn the_walk_holds_both_holders_of_a_string_at_its_bound() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    assert_eq!(
        super::MAX_LINEAGE_DEPTH,
        8,
        "the fixture: the chains below are built for a walk of eight links"
    );
    // The member `m0` and a chain of single ancestors up to `last`, whose
    // parent is `above`.
    let chained = |last: usize, above: Option<&str>| {
        let access = AccessController::new(every_agent_but_the_closed_tier(&[ALICE], ALICE))
            .expect("the rules are valid");
        for link in 0..=last {
            let parent = if link == last {
                above.map(str::to_string)
            } else {
                Some(format!("m{}", link + 1))
            };
            access.record_agent_attributes(AgentResourceAttributes {
                labels: match &parent {
                    Some(parent) => labels(None, &[(spawned_by, parent.as_str())]),
                    None => labels(None, &[]),
                },
                ..outer_member(&format!("m{link}"), None)
            });
        }
        access
    };
    let viewed = |access: &AccessController| view_of(access, ALICE).can_view_agent("m0");
    assert!(
        viewed(&chained(7, None)),
        "the fixture: a lineage of eight links walked to its end is decided"
    );
    let deeper = chained(7, Some("m8"));
    deeper.record_agent_attributes(outer_member("m8", None));
    assert!(
        !viewed(&deeper),
        "a lineage of eight links whose last link names a parent is refused"
    );
    // Seven single links, then a parent string that two members hold.
    let both = chained(6, Some("pair-team/top"));
    both.record_agent_attributes(outer_member("pair-team/top", None));
    record_nested(&both, "top", None, &[]);
    assert!(
        viewed(&both),
        "nine links with no ancestor left are decided"
    );
    let left = chained(6, Some("pair-team/top"));
    left.record_agent_attributes(outer_member("pair-team/top", None));
    record_nested(&left, "top", None, &[(spawned_by, "beyond")]);
    left.record_agent_attributes(outer_member("beyond", None));
    assert!(
        !viewed(&left),
        "nine links with an ancestor of one holder left are refused"
    );
}

/// The look at what the cache holds now of a captured record's ancestors
/// asks both holders of an ancestor's string: a deny that matches the
/// nested mob's member of that string now refuses the record, as one that
/// matches the outer mob's member does.
#[test]
fn a_captured_ancestors_string_that_two_members_hold_is_asked_of_both() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "pair-team/x")])),
        },
        CapturedLineageLink {
            identity: "pair-team/x".to_string(),
            attributes: Some(outer_member("pair-team/x", None)),
        },
    ];
    let viewed = |access: &AccessController| {
        let alice = view_of(access, ALICE);
        (
            alice.can_view_captured_member(&captured),
            alice
                .in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    let access = AccessController::new(closed_tier_rules(ALICE)).expect("the rules are valid");
    access.record_agent_attributes(outer_member("pair-team/x", None));
    record_nested(&access, "x", None, &[]);
    assert_eq!(
        viewed(&access),
        (true, true),
        "the fixture: the record is viewed while no deny matches a holder of the string"
    );
    record_nested(&access, "x", None, &[("tier", "closed")]);
    assert_eq!(
        viewed(&access),
        (false, false),
        "a deny that matches the nested mob's member of the ancestor's string refuses the record"
    );
}

/// A captured record's ancestor that the owner could not confirm is held
/// as a name alone, and a rule that names it grants the record through it.
/// Where two members hold that name now (an agent of the outer mob and a
/// member of a nested mob that the string spells), the name is ambiguous:
/// it grants nothing, and the one of the two that is another person's own
/// hides the record. In the outer view and in the nested view alike.
#[test]
fn a_captured_name_that_two_members_hold_now_grants_nothing() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "pair-team/x")])),
        },
        CapturedLineageLink {
            identity: "pair-team/x".to_string(),
            attributes: None,
        },
    ];
    let viewed = |access: &AccessController, person: &str| {
        let view = view_of(access, person);
        (
            view.can_view_captured_member(&captured),
            view.in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    let config = AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            // Alice: the agent the ancestor's name names.
            AccessRule {
                agents: vec!["pair-team/x".to_string()],
                ..rule("by-the-ancestors-name", ALICE, &["agent.*"])
            },
            // Bob: every agent.
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", BOB, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    };
    // The grant.
    let access = AccessController::new(config.clone()).expect("the rules are valid");
    access.record_agent_attributes(outer_member("pair-team/x", None));
    assert_eq!(
        viewed(&access, ALICE),
        (true, true),
        "the fixture: a rule that names the ancestor grants the record while one member holds \
         the name"
    );
    record_nested(&access, "x", None, &[]);
    assert_eq!(
        (viewed(&access, ALICE), viewed(&access, BOB)),
        ((false, false), (true, true)),
        "two members hold the name now: it grants nothing, and a grant on the record's own \
         member still does"
    );
    // The private owner.
    let access = AccessController::new(config).expect("the rules are valid");
    access.record_agent_attributes(outer_member("pair-team/x", None));
    record_nested(&access, "x", Some(ALICE), &[]);
    assert_eq!(
        viewed(&access, BOB),
        (false, false),
        "one of the two that hold the name now is another person's own: it hides the record"
    );
}

/// A parent that no entry describes is a link of its string alone, and a
/// rule that names it grants its descendants. Above a parent's string that
/// two members hold it grants nothing, like every link above them.
#[test]
fn an_unknown_parent_above_a_string_that_two_members_hold_grants_nothing() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            agents: vec!["never-recorded".to_string()],
            ..rule("by-the-unknown-parent", ALICE, &["agent.*"])
        }],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    record_nested(&access, "sibling", None, &[(spawned_by, "never-recorded")]);
    record_nested(&access, "child", None, &[(spawned_by, "pair-team/sibling")]);
    let viewed = || {
        let view = view_of(&access, ALICE);
        (
            view.in_nested_mob(&team).can_view_agent("child"),
            view.can_view_agent("pair-team/child"),
        )
    };
    assert_eq!(
        viewed(),
        (true, true),
        "the fixture: a rule that names a parent no entry describes grants its descendants"
    );
    access.record_agent_attributes(outer_member("pair-team/sibling", None));
    assert_eq!(
        viewed(),
        (false, false),
        "above a string that two members hold the unknown parent grants nothing"
    );
}

/// A captured ancestor the owner confirmed (captured with its attributes) is
/// that ancestor: the owner takes those attributes only from the member
/// whose session the child's lineage is bound to. While no member, or one
/// member, holds its string now, a rule that names the string grants the
/// record through it, as before. Once two members hold the string, the
/// grant no longer flows by the string: a rule that names it grants
/// nothing, a rule that matches what the owner captured of the ancestor
/// still grants, and a rule that matches the other holder of the string
/// alone grants nothing. A deny that names the string refuses the record
/// in each of these states.
#[test]
fn a_confirmed_captured_ancestor_grants_by_what_was_captured_once_two_hold_its_string() {
    const CAROL: &str = "carol@example.test";
    const DAVE: &str = "dave@example.test";
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "pair-team/x")])),
        },
        CapturedLineageLink {
            identity: "pair-team/x".to_string(),
            attributes: Some(former_record("x", None, &[("team", "red")])),
        },
    ];
    let by_team = |team: &str| BTreeMap::from([("team".to_string(), team.to_string())]);
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            // Alice: the agent the ancestor's string names.
            AccessRule {
                agents: vec!["pair-team/x".to_string()],
                ..rule("by-the-ancestors-string", ALICE, &["agent.*"])
            },
            // Bob: what the owner captured of the ancestor.
            AccessRule {
                match_labels: by_team("red"),
                ..rule("by-what-was-captured", BOB, &["agent.*"])
            },
            // Carol: what the other holder of the string alone carries.
            AccessRule {
                match_labels: by_team("blue"),
                ..rule("by-the-other-holder", CAROL, &["agent.*"])
            },
            // Dave: every agent but the one the ancestor's string names.
            AccessRule {
                agents: vec!["*".to_string()],
                ..rule("every-agent", DAVE, &["agent.*"])
            },
            AccessRule {
                effect: AccessEffect::Deny,
                agents: vec!["pair-team/x".to_string()],
                ..rule("not-the-ancestors-string", DAVE, &["agent.*"])
            },
        ],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    let viewed = |person: &str| {
        let view = view_of(&access, person);
        (
            view.can_view_captured_member(&captured),
            view.in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    let all = || [viewed(ALICE), viewed(BOB), viewed(CAROL), viewed(DAVE)];
    let (granted, refused) = ((true, true), (false, false));
    assert_eq!(
        all(),
        [granted, granted, refused, refused],
        "the fixture: no member holds the ancestor's string now"
    );
    access.record_agent_attributes(AgentResourceAttributes {
        labels: labels(None, &[("team", "blue")]),
        ..outer_member("pair-team/x", None)
    });
    assert_eq!(
        all(),
        [granted, granted, refused, refused],
        "one member holds the string now: a rule that names the string grants, as before"
    );
    record_nested(&access, "x", None, &[("team", "red")]);
    assert_eq!(
        all(),
        [refused, granted, refused, refused],
        "two members hold the string now: the grant flows through what was captured of the \
         ancestor, and no longer by the string"
    );
}

/// [`a_confirmed_captured_ancestor_grants_by_what_was_captured_once_two_hold_its_string`]
/// holds a record whose member the records no longer know. While they know
/// the member, its current lineage must allow the record too, and that walk
/// has no session to tell the two holders of a parent's string by: with two
/// holders, the rule that matches what was captured of the ancestor allows
/// nothing either.
#[test]
fn a_record_of_a_member_still_known_takes_no_grant_through_a_string_two_hold() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "pair-team/x")])),
        },
        CapturedLineageLink {
            identity: "pair-team/x".to_string(),
            attributes: Some(former_record("x", None, &[("team", "red")])),
        },
    ];
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            match_labels: BTreeMap::from([("team".to_string(), "red".to_string())]),
            ..rule("by-what-was-captured", BOB, &["agent.*"])
        }],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    let viewed = || {
        let view = view_of(&access, BOB);
        (
            view.can_view_captured_member(&captured),
            view.in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    // The record's member is still known, and one member holds its parent's
    // string: the nested mob's.
    record_nested(&access, "child", None, &[(spawned_by, "pair-team/x")]);
    record_nested(&access, "x", None, &[("team", "red")]);
    assert_eq!(
        viewed(),
        (true, true),
        "the fixture: one member holds the parent's string, and the rule on its label allows \
         the record"
    );
    access.record_agent_attributes(AgentResourceAttributes {
        labels: labels(None, &[("team", "blue")]),
        ..outer_member("pair-team/x", None)
    });
    assert_eq!(
        viewed(),
        (false, false),
        "two members hold the parent's string and the record's member is still known: its \
         current lineage allows nothing through that string"
    );
}

/// A stored lineage can hold a link above a name that two members hold now
/// (another build wrote it): no link at or above such a name grants. A rule
/// that names the link above grants the record while one member holds the
/// name below it, and nothing once two do.
#[test]
fn a_captured_link_above_a_name_that_two_members_hold_now_grants_nothing() {
    let spawned_by = crate::console_spawn::SPAWNED_BY_LABEL;
    let team = team();
    let captured = [
        CapturedLineageLink {
            identity: "pair-team/child".to_string(),
            attributes: Some(former_record("child", None, &[(spawned_by, "pair-team/x")])),
        },
        CapturedLineageLink {
            identity: "pair-team/x".to_string(),
            attributes: None,
        },
        CapturedLineageLink {
            identity: "pair-team/elder".to_string(),
            attributes: Some(former_record("elder", None, &[])),
        },
    ];
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            agents: vec!["pair-team/elder".to_string()],
            ..rule("by-the-link-above", ALICE, &["agent.*"])
        }],
        ..AccessControlConfig::default()
    })
    .expect("the rules are valid");
    let viewed = || {
        let view = view_of(&access, ALICE);
        (
            view.can_view_captured_member(&captured),
            view.in_nested_mob(&team)
                .can_view_captured_member(&captured),
        )
    };
    access.record_agent_attributes(outer_member("pair-team/x", None));
    assert_eq!(
        viewed(),
        (true, true),
        "the fixture: a rule that names the link above grants the record while one member holds \
         the name below it"
    );
    record_nested(&access, "x", None, &[]);
    assert_eq!(
        viewed(),
        (false, false),
        "two members hold the name now: no link at or above it grants"
    );
}
