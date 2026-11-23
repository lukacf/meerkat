//! The lookups of a member of a mob the host serves beside its default
//! mob, at the aggregator: the upload store of a member, the inspection of
//! an identity, and the mob a lookup reads. Each looks among the members
//! of the mob it is asked for alone, and the default mob's member of the
//! same name has no part in it.
//!
//! The rows run on a host of two mobs with members: a runtime registered
//! for the default mob and the second mob's console events, each mob with
//! a member of its own and a member of one name.

#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::member_key::DefaultMob;

/// The console events of the mob `second`, whose runtime is `runtime`,
/// register with `aggregator`, as a host registers them when it makes the
/// mob's entry.
fn register_the_second_mob(
    aggregator: &MetaConsoleAggregator,
    runtime: &UnifiedRuntime,
    second: &meerkat_mob::MobId,
) {
    aggregator
        .register_mob_console_events(
            second,
            runtime.mob_runtime().clone(),
            runtime.console_events(),
            Arc::new(AllowAllConsoleVisibilityPolicy),
        )
        .expect("the second mob's console events register");
}

/// A host of two mobs with members. The default mob's runtime holds
/// `alice` and `carol`, the second mob's `alice` and `dora`. The console
/// was told the host's default mob and holds the registration of the
/// default mob's runtime and of the second mob's console events.
///
/// NOT A LAUNCH: the two mobs are two runtimes built side by side, each
/// with a process part of its own, so each has an upload store of its own.
/// On a launch a hosted mob's runtime has the launch's process part, and
/// with it the launch's one upload store. The rows here use the two
/// stores only to tell WHICH MOB's registration a lookup found its member
/// in; what a launch's mobs share is held by a row on a launch
/// (`unified_runtime::mob_console_log_tests`).
struct TwoMobsWithMembers {
    aggregator: MetaConsoleAggregator,
    launch: UnifiedRuntime,
    hosted: UnifiedRuntime,
    second: meerkat_mob::MobId,
}

impl TwoMobsWithMembers {
    async fn shut_down(self) {
        self.hosted.shutdown().await;
        self.launch.shutdown().await;
    }
}

async fn two_mobs_with_members() -> TwoMobsWithMembers {
    two_mobs_with_members_on_a_console(true).await
}

/// [`two_mobs_with_members`], on a console that was told its host's
/// default mob (`told`) or was never told it.
async fn two_mobs_with_members_on_a_console(told: bool) -> TwoMobsWithMembers {
    let launch = super::tests::build_empty_runtime(&super::tests::unique_console_mob_id(
        "send-rows-launch-of-two",
    ))
    .await;
    let second = meerkat_mob::MobId::from(super::tests::unique_console_mob_id(
        "send-rows-second-of-two",
    ));
    let hosted = super::tests::build_empty_runtime(second.as_str()).await;
    for (runtime, members) in [(&launch, ["alice", "carol"]), (&hosted, ["alice", "dora"])] {
        for member in members {
            let seated =
                Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::new("worker", member))).await;
            assert!(seated.is_ok(), "{member}: {:?}", seated.as_ref().err());
        }
    }
    // The rows on this host read lookups alone: the console makes no
    // session history pass of its own.
    let aggregator = MetaConsoleAggregator::in_memory_with_options(ConsoleAggregatorOptions {
        session_history_backfill_enabled: false,
        ..Default::default()
    });
    if told {
        aggregator.tell_default_mob(DefaultMob::new(launch.mob_handle().mob_id().clone()));
    }
    aggregator.register_runtime_handles_with_policy(
        "runtime-a",
        "",
        launch.mob_runtime().clone(),
        None,
        launch.console_events(),
        Arc::new(AllowAllConsoleVisibilityPolicy),
    );
    register_the_second_mob(&aggregator, &hosted, &second);
    TwoMobsWithMembers {
        aggregator,
        launch,
        hosted,
        second,
    }
}

/// What the upload store's lookup for the member `identity` of the mob
/// `mob` answers when `aggregator` (the host's console, or a view of it)
/// is asked: whose store it is, or the refusal.
async fn upload_store_in(
    aggregator: &MetaConsoleAggregator,
    host: &TwoMobsWithMembers,
    identity: &str,
    mob: Option<&meerkat_mob::MobId>,
) -> String {
    let found = match Box::pin(aggregator.binary_blob_store_for_member(identity, mob)).await {
        Ok(Some(found)) => found,
        Ok(None) => return "no store".to_string(),
        Err(ConsoleSendError::UnknownIdentity(_)) => return "unknown identity".to_string(),
        Err(error) => return error.to_string(),
    };
    let is = |store: Option<Arc<dyn BinaryBlobStore>>| {
        store.is_some_and(|store| std::ptr::addr_eq(Arc::as_ptr(&store), Arc::as_ptr(&found)))
    };
    if is(host.hosted.mob_runtime().binary_blob_store()) {
        "the second mob's store".to_string()
    } else if is(host.launch.mob_runtime().binary_blob_store()) {
        "the default mob's store".to_string()
    } else {
        "a store of neither mob".to_string()
    }
}

/// The upload store of a mob's member is found among that mob's members
/// alone and is that mob's runtime's. A member only the second mob has is
/// found there and not in the default mob; a member only the default mob
/// has is found there and not in the second; of the two members of one
/// name each lookup answers its own mob's runtime's store (two stores
/// here, where a launch has one: see [`TwoMobsWithMembers`]). The default
/// mob named by its id is the default mob. A mob whose console events are
/// not registered answers as a member no mob has: an unknown identity.
#[tokio::test]
async fn the_upload_store_of_a_mobs_member_is_found_in_that_mob_alone() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    let launch = host.launch.mob_handle().mob_id().clone();
    let not_registered = meerkat_mob::MobId::from("send-rows-no-such-mob");
    // A member is found once its mob lists it: a moment after it is seated.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while (upload_store_in(&host.aggregator, &host, "carol", None).await == "unknown identity"
        || upload_store_in(&host.aggregator, &host, "dora", Some(&second)).await
            == "unknown identity")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let mut answers = Vec::new();
    for (identity, mob) in [
        ("dora", Some(&second)),
        ("dora", None),
        ("carol", Some(&second)),
        ("carol", None),
        ("alice", Some(&second)),
        ("alice", None),
        ("alice", Some(&launch)),
        ("alice", Some(&not_registered)),
    ] {
        answers.push(upload_store_in(&host.aggregator, &host, identity, mob).await);
    }
    host.shut_down().await;

    assert_eq!(
        answers,
        [
            "the second mob's store",
            "unknown identity",
            "unknown identity",
            "the default mob's store",
            "the second mob's store",
            "the default mob's store",
            "the default mob's store",
            "unknown identity",
        ],
        "the lookups, in order: the second mob's own member in its mob and in the default mob; \
         the default mob's own member in the second mob and in its own; the member of one name \
         in the second mob, in the default mob, in the default mob named by its id, and in a \
         mob whose console events are not registered"
    );
}

/// What the inspection of the identity `identity` of the mob `mob`
/// answers when `aggregator` (the host's console, or a view of it) is
/// asked: the identity, the runtime key of the registration it was found
/// in and its session; or that no such identity is known.
async fn inspected_in(
    aggregator: &MetaConsoleAggregator,
    identity: &str,
    mob: Option<&meerkat_mob::MobId>,
) -> String {
    match Box::pin(aggregator.inspect_identity_of_mob(mob, identity)).await {
        Ok(Some(inspection)) => format!(
            "{} of {}, session {}",
            inspection.identity.identity,
            inspection.identity.runtime_key,
            inspection.identity.session_id.as_deref().unwrap_or("none")
        ),
        Ok(None) => "unknown identity".to_string(),
        Err(error) => format!("refused: {error}"),
    }
}

/// The inspection of an identity of a mob looks among that mob's members
/// alone. A member only the second mob has is found there and not in the
/// default mob; a member only the default mob has is found there and not
/// in the second; of the two members of one name each inspection answers
/// its own mob's member, under its own registration and with its own
/// session. The default mob named by its id is the default mob. A mob
/// whose console events are not registered answers as an identity no mob
/// has: none.
#[tokio::test]
async fn the_inspection_of_a_mobs_identity_is_found_in_that_mob_alone() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    let launch = host.launch.mob_handle().mob_id().clone();
    let not_registered = meerkat_mob::MobId::from("send-rows-no-such-mob");
    // What the inspection of a member must answer: its name, the runtime
    // key of its mob's registration and the session its own runtime holds
    // for it, which is bound a moment after the member is seated.
    let own = |runtime: &UnifiedRuntime, runtime_key: String, member: &'static str| {
        let handle = runtime.mob_handle();
        async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let session = handle
                    .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from(member))
                    .await;
                if let Some(session) = session {
                    return format!("{member} of {runtime_key}, session {session}");
                }
                if tokio::time::Instant::now() >= deadline {
                    return format!("{member} of {runtime_key}, with no session in time");
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    };
    let of_the_second = format!("mob/{}", second.as_str());
    let dora = own(&host.hosted, of_the_second.clone(), "dora").await;
    let alice_of_the_second = own(&host.hosted, of_the_second, "alice").await;
    let carol = own(&host.launch, "runtime-a".to_string(), "carol").await;
    let alice_of_the_default = own(&host.launch, "runtime-a".to_string(), "alice").await;
    let mut answers = Vec::new();
    for (identity, mob) in [
        ("dora", Some(&second)),
        ("dora", None),
        ("carol", Some(&second)),
        ("carol", None),
        ("alice", Some(&second)),
        ("alice", None),
        ("alice", Some(&launch)),
        ("alice", Some(&not_registered)),
    ] {
        answers.push(inspected_in(&host.aggregator, identity, mob).await);
    }
    host.shut_down().await;

    assert_eq!(
        answers,
        [
            dora.as_str(),
            "unknown identity",
            "unknown identity",
            carol.as_str(),
            alice_of_the_second.as_str(),
            alice_of_the_default.as_str(),
            alice_of_the_default.as_str(),
            "unknown identity",
        ],
        "the inspections, in order: the second mob's own member in its mob and in the default \
         mob; the default mob's own member in the second mob and in its own; the member of one \
         name in the second mob, in the default mob, in the default mob named by its id, and \
         in a mob whose console events are not registered"
    );
}

/// A policy that hides the identities it names, in every mob.
struct Hides(&'static [&'static str]);

impl ConsoleVisibilityPolicy for Hides {
    fn identity_visible(&self, record: &ConsoleIdentityRecord) -> bool {
        !self.0.contains(&record.identity.as_str())
    }
}

/// A member a view's policy hides is answered by the upload store exactly
/// as a member no mob has: an unknown identity, in the second mob and in
/// the default mob. The console's own lookup, which hides nothing, finds
/// both members, and the view still finds a member its policy shows, in
/// each mob.
#[tokio::test]
async fn the_upload_store_answers_a_hidden_member_as_a_member_no_mob_has() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while (upload_store_in(&host.aggregator, &host, "carol", None).await == "unknown identity"
        || upload_store_in(&host.aggregator, &host, "dora", Some(&second)).await
            == "unknown identity")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let view = host
        .aggregator
        .policy_view(Arc::new(Hides(&["dora", "carol"])));
    let shown = [
        upload_store_in(&host.aggregator, &host, "dora", Some(&second)).await,
        upload_store_in(&host.aggregator, &host, "carol", None).await,
    ];
    let mut through_the_view = Vec::new();
    for (identity, mob) in [
        ("dora", Some(&second)),
        ("erin", Some(&second)),
        ("carol", None),
        ("erin", None),
        ("alice", Some(&second)),
        ("alice", None),
    ] {
        through_the_view.push(upload_store_in(&view, &host, identity, mob).await);
    }
    host.shut_down().await;

    assert_eq!(
        shown,
        ["the second mob's store", "the default mob's store"],
        "the console's own lookup finds the two members the view's policy hides"
    );
    assert_eq!(
        through_the_view,
        [
            "unknown identity",
            "unknown identity",
            "unknown identity",
            "unknown identity",
            "the second mob's store",
            "the default mob's store",
        ],
        "through the view, in order: the second mob's hidden member and a member that mob \
         does not have; the default mob's hidden member and a member it does not have; the \
         member the policy shows, in each mob"
    );
}

/// An identity a view's policy hides is answered by the inspection exactly
/// as an identity no mob has: none, in the second mob and in the default
/// mob. The console's own inspection, which hides nothing, finds both, and
/// the view still finds an identity its policy shows, in each mob.
#[tokio::test]
async fn the_inspection_answers_a_hidden_identity_as_an_identity_no_mob_has() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    let of_the_second = format!("mob/{}", second.as_str());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while (inspected_in(&host.aggregator, "carol", None).await == "unknown identity"
        || inspected_in(&host.aggregator, "dora", Some(&second)).await == "unknown identity")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    let view = host
        .aggregator
        .policy_view(Arc::new(Hides(&["dora", "carol"])));
    // The registration an inspection found its identity in: the words
    // before the session.
    let found_in = |answer: String| {
        answer
            .split(", session")
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let shown = [
        found_in(inspected_in(&host.aggregator, "dora", Some(&second)).await),
        found_in(inspected_in(&host.aggregator, "carol", None).await),
    ];
    let mut through_the_view = Vec::new();
    for (identity, mob) in [
        ("dora", Some(&second)),
        ("erin", Some(&second)),
        ("carol", None),
        ("erin", None),
        ("alice", Some(&second)),
        ("alice", None),
    ] {
        through_the_view.push(found_in(inspected_in(&view, identity, mob).await));
    }
    host.shut_down().await;

    assert_eq!(
        shown,
        [
            format!("dora of {of_the_second}"),
            "carol of runtime-a".to_string()
        ],
        "the console's own inspection finds the two identities the view's policy hides"
    );
    assert_eq!(
        through_the_view,
        [
            "unknown identity".to_string(),
            "unknown identity".to_string(),
            "unknown identity".to_string(),
            "unknown identity".to_string(),
            format!("alice of {of_the_second}"),
            "alice of runtime-a".to_string(),
        ],
        "through the view, in order: the second mob's hidden identity and an identity that mob \
         does not have; the default mob's hidden identity and an identity it does not have; \
         the identity the policy shows, in each mob"
    );
}

/// The inspection of a mob's member asks a session history pass for the
/// member it found: under that member's own registration and for its own
/// session, never for the other mob's member of the name. Two mobs each
/// have an `alice`. The inspection of the second mob's asks a pass under
/// the second mob's registration for that member's session; the
/// inspection of the default mob's asks one under the default mob's
/// runtime for its own.
///
/// The row reads what was asked ([`HistoryPassAsked`]); the console makes
/// no pass, and a pass for one found member is asked by nothing else here.
#[tokio::test]
async fn the_inspection_of_a_mobs_identity_asks_the_history_pass_of_the_member_it_found() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    let alice = meerkat_mob::AgentIdentity::from("alice");
    // Each alice is found, and has her session, a moment after she is
    // seated. The lookup that waits for it asks no pass.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    let sessions = loop {
        let of_the_second = host
            .hosted
            .mob_handle()
            .resolve_bridge_session_id(&alice)
            .await;
        let of_the_default = host
            .launch
            .mob_handle()
            .resolve_bridge_session_id(&alice)
            .await;
        let listed = upload_store_in(&host.aggregator, &host, "alice", Some(&second)).await
            != "unknown identity"
            && upload_store_in(&host.aggregator, &host, "alice", None).await != "unknown identity";
        if let (Some(of_the_second), Some(of_the_default), true) =
            (&of_the_second, &of_the_default, listed)
        {
            break Some((of_the_second.to_string(), of_the_default.to_string()));
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    };
    let before = host.aggregator.history_passes_asked().len();
    let found = [
        inspected_in(&host.aggregator, "alice", Some(&second)).await,
        inspected_in(&host.aggregator, "alice", None).await,
    ];
    let asked: Vec<HistoryPassAsked> = host
        .aggregator
        .history_passes_asked()
        .into_iter()
        .skip(before)
        .filter(|asked| matches!(asked, HistoryPassAsked::OfFoundMember { .. }))
        .collect();
    host.shut_down().await;

    let Some((of_the_second, of_the_default)) = sessions else {
        panic!("each mob's alice is listed and has a session");
    };
    assert!(
        found.iter().all(|answer| answer.starts_with("alice of ")),
        "both inspections find their alice: {found:?}"
    );
    assert_eq!(
        asked,
        [
            HistoryPassAsked::OfFoundMember {
                runtime_key: format!("mob/{}", second.as_str()),
                session_id: of_the_second,
            },
            HistoryPassAsked::OfFoundMember {
                runtime_key: "runtime-a".to_string(),
                session_id: of_the_default,
            },
        ],
        "the passes asked for a found member, in order: by the inspection of the second mob's \
         alice and of the default mob's"
    );
}

/// The two lookups read the mob they are asked for by one rule, the
/// registration's own: a mob is found by the registration under its id.
/// What each answers for the member `identity` of the mob `mob`: the
/// upload store's answer, and the registration the inspection found its
/// identity in.
async fn both_lookups(
    host: &TwoMobsWithMembers,
    identity: &str,
    mob: Option<&meerkat_mob::MobId>,
) -> [String; 2] {
    [
        upload_store_in(&host.aggregator, host, identity, mob).await,
        inspected_in(&host.aggregator, identity, mob)
            .await
            .split(", session")
            .next()
            .unwrap_or_default()
            .to_string(),
    ]
}

/// The members of the host `host` are listed: the lookup that waits for it
/// names no mob for the default mob's member, and the second mob through
/// the inspection, which reads a mob by its registration whether or not
/// the console was told its default mob.
async fn once_the_members_are_listed(host: &TwoMobsWithMembers) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while (upload_store_in(&host.aggregator, host, "carol", None).await == "unknown identity"
        || inspected_in(&host.aggregator, "dora", Some(&host.second)).await == "unknown identity")
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// A console that was never told its host's default mob still finds a
/// registered mob by its id, at both lookups: the registration of a mob's
/// console events never asks whether the console was told. The second
/// mob's member is found in the second mob, the default mob's member with
/// no mob named, and the second mob's member is not found with no mob
/// named.
///
/// The default mob named by its own id is, on such a console, an id with
/// no registration: both lookups answer an unknown identity, though the
/// default mob has a member of the name. Only a console that was told its
/// default mob reads that id as the default mob.
#[tokio::test]
async fn a_console_never_told_its_default_mob_finds_a_registered_mob_at_both_lookups() {
    let host = two_mobs_with_members_on_a_console(false).await;
    let second = host.second.clone();
    let launch = host.launch.mob_handle().mob_id().clone();
    let of_the_second = format!("mob/{}", second.as_str());
    once_the_members_are_listed(&host).await;
    let answers = [
        both_lookups(&host, "dora", Some(&second)).await,
        both_lookups(&host, "carol", None).await,
        both_lookups(&host, "dora", None).await,
        both_lookups(&host, "alice", Some(&launch)).await,
    ];
    host.shut_down().await;

    assert_eq!(
        answers,
        [
            [
                "the second mob's store".to_string(),
                format!("dora of {of_the_second}")
            ],
            [
                "the default mob's store".to_string(),
                "carol of runtime-a".to_string()
            ],
            [
                "unknown identity".to_string(),
                "unknown identity".to_string()
            ],
            [
                "unknown identity".to_string(),
                "unknown identity".to_string()
            ],
        ],
        "the upload store's answer and the inspection's, for: the second mob's member in its \
         mob; the default mob's member with no mob named; the second mob's member with no mob \
         named; the default mob's member with the default mob named by its own id"
    );
}

/// A blank mob id names no mob: no registration is under it, so both
/// lookups answer as for a mob that is not registered, an unknown
/// identity, and never the default mob's member of the name. Shown with an
/// empty id and with one of white space only, for a name both mobs have.
#[tokio::test]
async fn a_blank_mob_id_answers_at_both_lookups_as_a_mob_that_is_not_registered() {
    let host = two_mobs_with_members().await;
    once_the_members_are_listed(&host).await;
    let named_no_mob = both_lookups(&host, "alice", None).await;
    let mut blank = Vec::new();
    for id in ["", "   "] {
        blank.push(both_lookups(&host, "alice", Some(&meerkat_mob::MobId::from(id))).await);
    }
    host.shut_down().await;

    assert_eq!(
        named_no_mob,
        [
            "the default mob's store".to_string(),
            "alice of runtime-a".to_string()
        ],
        "with no mob named both lookups find the default mob's member"
    );
    let unknown = [
        "unknown identity".to_string(),
        "unknown identity".to_string(),
    ];
    assert_eq!(
        blank,
        [unknown.clone(), unknown],
        "with an empty id, and with an id of white space, both lookups answer an unknown \
         identity"
    );
}

/// A console event of a mob's member asks the session history pass for
/// that mob's member: the pass is asked by the member's name with the mob
/// whose console events carried the event, so it never finds the other
/// mob's member of that name. Two mobs each have an `alice`, and each
/// mob's console events record that its `alice`'s run completed. The
/// second mob's event asks the pass for the second mob's `alice`; the
/// default mob's asks it with no mob.
///
/// The row reads what was asked ([`HistoryPassAsked`]); the console makes
/// no pass. It holds WHOM each ask names, not how many asks there are: a
/// registration reads its console events live and by a replay of what its
/// store holds, so one event can be projected, and ask, more than once.
#[tokio::test]
async fn a_console_event_of_a_mobs_member_asks_the_history_pass_of_that_mobs_member() {
    let host = two_mobs_with_members().await;
    let second = host.second.clone();
    once_the_members_are_listed(&host).await;
    let before = host.aggregator.history_passes_asked().len();
    let of = |mob: Option<&meerkat_mob::MobId>| HistoryPassAsked::OfMember {
        mob: mob.cloned(),
        identity: "alice".to_string(),
    };
    // Whom the passes asked since `before` by the name `alice` name, each
    // once, in the order first asked; read once `wanted` is among them (or
    // as they are, in time).
    let asked_for_alice = |wanted: HistoryPassAsked| {
        let aggregator = host.aggregator.clone();
        async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let mut asked: Vec<HistoryPassAsked> = Vec::new();
                for ask in aggregator.history_passes_asked().into_iter().skip(before) {
                    let of_alice = matches!(
                        &ask,
                        HistoryPassAsked::OfMember { identity, .. } if identity == "alice"
                    );
                    if of_alice && !asked.contains(&ask) {
                        asked.push(ask);
                    }
                }
                if asked.contains(&wanted) || tokio::time::Instant::now() >= deadline {
                    return asked;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        }
    };
    host.hosted
        .console_events()
        .append("alice", None, "run_completed", json!({}))
        .await;
    let after_the_second_mobs_event = asked_for_alice(of(Some(&second))).await;
    host.launch
        .console_events()
        .append("alice", None, "run_completed", json!({}))
        .await;
    let after_both = asked_for_alice(of(None)).await;
    host.shut_down().await;

    assert_eq!(
        after_the_second_mobs_event,
        [of(Some(&second))],
        "every pass the second mob's member's event asks is for that mob's member"
    );
    assert_eq!(
        after_both,
        [of(Some(&second)), of(None)],
        "and the default mob's member's event asks it with no mob, and for no other mob"
    );
}
