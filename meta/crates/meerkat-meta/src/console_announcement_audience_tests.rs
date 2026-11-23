//! The Console's mob announcement methods under the private-owner rule: a
//! record of a member hidden from its caller is that member's audience's
//! alone, a request naming such a member is answered as one naming nobody,
//! and every method still serves a member its caller may see. Each row runs
//! on a console with sign-in only and on one under access rules.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::console_private_side_chat_tests::{
    ROOT, TestResult, alice_claims, bearer, bob_claims, enforced_console, fork_on_console,
    root_claims, runtime,
};
use super::console_side_chat_tests::seeded_member;
use super::console_token_lapse_tests::console_rpc_as;
use super::*;
use crate::mob_announcements::tests::SurfaceOwner;

/// A member every caller may see.
const SHARED: &str = "alpha";
/// A member private to Alice.
const PRIVATE: &str = "alpha-side";
/// A name no member holds.
const NOBODY: &str = "omega";
/// The mob the announcement owner's members are local to.
const MOB: &str = SurfaceOwner::MOB;

/// A console over an announcement owner, with Alice's private member.
struct Scene {
    _state: tempfile::TempDir,
    /// The runtime the console serves, with the private member on its roster.
    runtime: MobRuntime,
    console: &'static str,
    enforced: bool,
    app: Router,
    owner: SurfaceOwner,
    alice: String,
    bob: String,
    root: String,
    found: Vec<String>,
}

impl Scene {
    /// `verb` of the mob announcement methods as the holder of `bearer`.
    async fn call(&self, bearer: &str, verb: &str, params: Value) -> Value {
        console_rpc_as(
            &self.app,
            Some(bearer),
            &format!("meta/mob_announcements/{verb}"),
            params,
        )
        .await
    }

    /// [`Self::call`], which must succeed: its result.
    async fn served(&self, bearer: &str, verb: &str, params: Value) -> Value {
        let answer = self.call(bearer, verb, params).await;
        assert!(
            answer["error"].is_null(),
            "{}: {verb}: {answer:#}",
            self.console
        );
        answer["result"].clone()
    }

    /// Note `what` against the row's rule unless it `holds`.
    fn check(&mut self, holds: bool, what: impl FnOnce() -> String) {
        if !holds {
            self.found.push(format!("{}: {}", self.console, what()));
        }
    }

    /// [`Self::call`] whose success the row's rule asks for: its result,
    /// null when it was refused.
    async fn expected(&mut self, bearer: &str, verb: &str, params: Value) -> Value {
        let answer = self.call(bearer, verb, params).await;
        self.check(answer["error"].is_null(), || {
            format!("{verb} is refused: {}", answer["error"])
        });
        answer["result"].clone()
    }
}

/// An access configuration under which every signed-in caller may do
/// everything to every agent, administer the runtime and address every
/// mob, with `root` its one admin. The mob the announcement owner of these
/// rows names its members by is not the console's own mob, and some rows
/// name a peer's mob and a mob a member created: a caller who works with
/// a mob other than the console's own is given a separate allow rule that
/// lists the mobs, and here it lists every mob.
fn everyone_administers_the_runtime() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            AccessRule {
                id: "everyone-administers-the-runtime".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec![
                    "agent.*".to_string(),
                    crate::access::ACTION_RUNTIME_ADMIN.to_string(),
                ],
                ..AccessRule::default()
            },
            AccessRule {
                id: "everyone-addresses-every-mob".to_string(),
                subjects: vec!["*".to_string()],
                actions: vec![crate::access::ACTION_AGENT_VIEW.to_string()],
                mobs: vec!["*".to_string()],
                ..AccessRule::default()
            },
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// The consoles of the row `row`, one after the other: with sign-in only,
/// and under access rules.
async fn scenes(row: &str) -> Result<Vec<Scene>, Box<dyn std::error::Error + Send + Sync>> {
    let mut scenes = Vec::new();
    for (console, access) in [
        ("sign-in only", None),
        ("access rules", Some(everyone_administers_the_runtime())),
    ] {
        let enforced = access.is_some();
        // A runtime's mob id is taken once per process.
        let mob = format!("announcement-audience-{row}-{}", u8::from(enforced));
        let (state, runtime) = runtime(&mob).await?;
        seeded_member(&runtime, "worker", SHARED, &[]).await?;
        let owner = SurfaceOwner::serving(&[SHARED, PRIVATE]);
        let state_of_console = enforced_console(&runtime, access);
        assert!(
            state_of_console
                .mob_announcements
                .set(owner.service())
                .is_ok(),
            "the console takes the owner"
        );
        let app = console_json_router_with_state(state_of_console);
        let alice = bearer(alice_claims());
        let forked = fork_on_console(&app, Some(&alice), SHARED, PRIVATE).await;
        assert!(forked["error"].is_null(), "{console}: {forked:#}");
        scenes.push(Scene {
            _state: state,
            runtime,
            console,
            enforced,
            app,
            owner,
            alice,
            bob: bearer(bob_claims()),
            root: bearer(root_claims()),
            found: Vec::new(),
        });
    }
    Ok(scenes)
}

/// Fails the row with everything its scenes found against its rule.
fn nothing_found(scenes: Vec<Scene>) {
    let found: Vec<String> = scenes.into_iter().flat_map(|scene| scene.found).collect();
    assert!(found.is_empty(), "{found:#?}");
}

fn member(identity: &str) -> Value {
    json!({ "mob_id": MOB, "identity": identity })
}

/// `answer` with each of `names` replaced by a placeholder, so the answers
/// a method gives two names compare as answers.
fn masked(answer: &Value, names: &[&str]) -> String {
    names.iter().fold(answer.to_string(), |text, name| {
        text.replace(name, "<name>")
    })
}

/// The start of `answer`, for a note.
fn brief(answer: &str) -> String {
    answer.chars().take(240).collect()
}

/// A subscription `id` of `identity` on the topic "news".
fn subscription(id: &str, identity: &str) -> Value {
    json!({
        "operation_id": format!("put-{id}"),
        "subscription_id": id,
        "member": member(identity),
        "topics": ["news"],
        "mobs": [MOB],
    })
}

/// An announcement on the topic "news" under `key`.
fn news(key: &str) -> Value {
    json!({
        "mob_id": MOB,
        "source_key": key,
        "topics": ["news"],
        "body": "the build is green",
    })
}

/// A source `id` whose audience is `audience`.
fn source(id: &str, audience: &[&str]) -> Value {
    json!({
        "operation_id": format!("put-{id}"),
        "mob_id": MOB,
        "source_id": id,
        "identity": {
            "provider": "google.gmail",
            "principal": principal(),
            "scope": { "kind": "mailbox_label", "id": id },
        },
        "connection": { "kind": "connect_setup", "id": "setup-1" },
        "observation": { "baseline": "delta", "deletions": "explicit" },
        "topics": [format!("{id}.received")],
        "audience": audience
            .iter()
            .map(|identity| json!({ "kind": "member", "member": member(identity) }))
            .collect::<Vec<_>>(),
    })
}

fn principal() -> Value {
    json!({
        "kind": "connector_verified_account",
        "issuer": "https://accounts.example",
        "strategy_id": "oauth-code",
        "subject": "subject-1",
    })
}

/// A change of the source `id` at `revision`.
fn source_change(verb: &str, id: &str, revision: u64) -> Value {
    json!({
        "operation_id": format!("{verb}-{id}"),
        "mob_id": MOB,
        "source_id": id,
        "expected_revision": revision,
    })
}

/// Declarations at `revision` of the subscriptions `(id, member)`.
fn declared(revision: u64, subscriptions: &[(&str, &str)]) -> Value {
    json!({
        "revision": revision,
        "declarations": {
            "revision": revision,
            "subscriptions": subscriptions
                .iter()
                .map(|(id, identity)| json!({
                    "subscription_id": id,
                    "member": member(identity),
                    "topics": ["news"],
                }))
                .collect::<Vec<_>>(),
        },
    })
}

/// The ids of the records a declarations view lists.
fn declared_ids(view: &Value) -> BTreeSet<String> {
    view["records"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|record| record["id"].as_str())
        .map(str::to_string)
        .collect()
}

fn ids(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The id of the one batch sealed for `identity`, as Alice lists it.
async fn sealed_batch(scene: &Scene, identity: &str) -> String {
    scene.owner.round_at(100_000).await;
    let listed = scene
        .served(
            &scene.alice,
            "deliveries/list",
            json!({ "member": member(identity) }),
        )
        .await;
    let batches = listed["deliveries"].as_array().cloned().unwrap_or_default();
    assert_eq!(batches.len(), 1, "one batch for {identity}: {listed:#}");
    batches[0]["id"].as_str().expect("a batch id").to_string()
}

/// A subscription of a member hidden from the caller is answered as an id
/// nobody holds, read or changed, and stays as it was; its owner reads and
/// changes it, and an access administrator reads it.
#[tokio::test]
async fn a_subscription_of_a_hidden_member_is_its_audiences_alone() -> TestResult {
    let mut scenes = scenes("subscription").await?;
    for scene in &mut scenes {
        let (alice, bob, root) = (scene.alice.clone(), scene.bob.clone(), scene.root.clone());
        scene
            .served(
                &alice,
                "subscriptions/put",
                subscription("sub-private", PRIVATE),
            )
            .await;
        let read = json!({ "subscription_id": "sub-private" });
        let before = scene
            .served(&alice, "subscriptions/get", read.clone())
            .await;
        assert_eq!(
            before["subscription"]["member"],
            member(PRIVATE),
            "its owner reads it whole: {before:#}"
        );
        if scene.enforced {
            let administrators = scene.served(&root, "subscriptions/get", read.clone()).await;
            assert_eq!(administrators, before, "an administrator reads it");
        }
        for verb in ["get", "pause", "resume", "cancel"] {
            let request = |id: &str| match verb {
                "get" => json!({ "subscription_id": id }),
                _ => json!({
                    "operation_id": format!("{verb}-{id}"),
                    "subscription_id": id,
                    "expected_revision": 1,
                }),
            };
            let method = format!("subscriptions/{verb}");
            let hidden = masked(
                &scene.call(&bob, &method, request("sub-private")).await,
                &["sub-private"],
            );
            let absent = masked(
                &scene.call(&bob, &method, request("sub-nobody")).await,
                &["sub-nobody"],
            );
            scene.check(hidden == absent, || {
                format!(
                    "{method} tells the two ids apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
        let after = scene.call(&alice, "subscriptions/get", read).await;
        scene.check(after["result"] == before, || {
            format!("the subscription changed: {}", brief(&after.to_string()))
        });
        scene
            .expected(
                &alice,
                "subscriptions/pause",
                json!({
                    "operation_id": "pause-own",
                    "subscription_id": "sub-private",
                    "expected_revision": 1,
                }),
            )
            .await;
    }
    nothing_found(scenes);
    Ok(())
}

/// A delivery to a member hidden from the caller is answered as an id
/// nobody holds, read or changed, and stays as it was; its owner reads it
/// whole.
#[tokio::test]
async fn a_delivery_to_a_hidden_member_is_its_audiences_alone() -> TestResult {
    let mut scenes = scenes("delivery").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene
            .served(
                &alice,
                "subscriptions/put",
                subscription("sub-private", PRIVATE),
            )
            .await;
        scene.served(&alice, "announce", news("k-1")).await;
        let batch = sealed_batch(scene, PRIVATE).await;
        let read = json!({ "batch_id": batch });
        let before = scene
            .served(&alice, "deliveries/observe", read.clone())
            .await;
        assert_eq!(
            before["batch"]["member"],
            member(PRIVATE),
            "its owner reads it whole: {before:#}"
        );
        assert!(
            before["batch"]["body"]
                .as_str()
                .is_some_and(|body| body.contains("the build is green")),
            "{before:#}"
        );
        for verb in ["observe", "reconcile", "redeliver", "dismiss"] {
            let request = |id: &str| match verb {
                "observe" | "reconcile" => json!({ "batch_id": id }),
                _ => json!({ "operation_id": format!("{verb}-{id}"), "batch_id": id }),
            };
            let method = format!("deliveries/{verb}");
            let hidden = masked(
                &scene.call(&bob, &method, request(batch.as_str())).await,
                &[batch.as_str()],
            );
            let absent = masked(
                &scene.call(&bob, &method, request("batch-nobody")).await,
                &["batch-nobody"],
            );
            scene.check(hidden == absent, || {
                format!(
                    "{method} tells the two ids apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
        let after = scene.call(&alice, "deliveries/observe", read).await;
        scene.check(after["result"] == before, || {
            format!("the delivery changed: {}", brief(&after.to_string()))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// Declarations applied on the console: a subscription the applied ones do
/// not hold is refused with one answer when its caller may not see its
/// member, whoever holds the name; an id a hidden member's declared record
/// holds is refused as taken; and an apply that leaves a hidden member's
/// declared record out leaves the record as it was, and lists it to its
/// audience alone. Its owner's own apply changes it as asked.
#[tokio::test]
async fn declared_records_of_a_hidden_member_are_its_audiences_alone() -> TestResult {
    let mut scenes = scenes("declared").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        let applied = scene
            .served(
                &alice,
                "declarations/apply",
                declared(1, &[("decl-private", PRIVATE)]),
            )
            .await;
        assert_eq!(
            applied["records"][0]["status"],
            json!("applied"),
            "{applied:#}"
        );
        let read = json!({ "subscription_id": "decl-private" });
        let before = scene
            .served(&alice, "subscriptions/get", read.clone())
            .await;
        assert_eq!(
            before["subscription"]["state"],
            json!("active"),
            "{before:#}"
        );
        let view = scene.served(&alice, "declarations/get", json!({})).await;

        let hidden = scene
            .call(
                &bob,
                "declarations/apply",
                declared(2, &[("decl-new", PRIVATE)]),
            )
            .await;
        let absent = scene
            .call(
                &bob,
                "declarations/apply",
                declared(2, &[("decl-new", NOBODY)]),
            )
            .await;
        scene.check(
            hidden["error"]["data"]["kind"] == json!("unknown_member"),
            || {
                format!(
                    "a new subscription of a hidden member: {}",
                    brief(&hidden.to_string())
                )
            },
        );
        let (hidden, absent) = (masked(&hidden, &[PRIVATE]), masked(&absent, &[NOBODY]));
        scene.check(hidden == absent, || {
            format!(
                "an apply tells the two names apart: {} | {}",
                brief(&hidden),
                brief(&absent)
            )
        });
        let taken = scene
            .call(
                &bob,
                "declarations/apply",
                declared(2, &[("decl-private", SHARED)]),
            )
            .await;
        scene.check(
            taken["error"]["data"]["kind"] == json!("subscription_id_taken"),
            || format!("an id a hidden record holds: {}", brief(&taken.to_string())),
        );
        let unchanged = scene.call(&alice, "declarations/get", json!({})).await;
        scene.check(unchanged["result"] == view, || {
            format!(
                "a refused apply applied something: {}",
                brief(&unchanged.to_string())
            )
        });

        let others = scene
            .expected(
                &bob,
                "declarations/apply",
                declared(2, &[("decl-shared", SHARED)]),
            )
            .await;
        scene.check(declared_ids(&others) == ids(&["decl-shared"]), || {
            format!("the apply's answer lists {:?}", declared_ids(&others))
        });
        let listed = scene.expected(&bob, "declarations/get", json!({})).await;
        scene.check(declared_ids(&listed) == ids(&["decl-shared"]), || {
            format!("the caller is listed {:?}", declared_ids(&listed))
        });
        let kept = scene.call(&alice, "subscriptions/get", read.clone()).await;
        scene.check(kept["result"] == before, || {
            format!("the record left out changed: {}", brief(&kept.to_string()))
        });
        let owners = scene.expected(&alice, "declarations/get", json!({})).await;
        scene.check(
            declared_ids(&owners) == ids(&["decl-private", "decl-shared"]),
            || format!("its owner is listed {:?}", declared_ids(&owners)),
        );

        scene
            .expected(
                &alice,
                "declarations/apply",
                declared(3, &[("decl-shared", SHARED)]),
            )
            .await;
        let dropped = scene.call(&alice, "subscriptions/get", read).await;
        scene.check(
            dropped["result"]["subscription"]["state"] == json!("cancelled"),
            || {
                format!(
                    "its owner's apply did not drop it: {}",
                    brief(&dropped.to_string())
                )
            },
        );
    }
    nothing_found(scenes);
    Ok(())
}

/// The answers about sources and escalations name no member hidden from
/// their caller, below the row level either; a member the caller may see
/// stays, and the hidden member's owner reads everything.
#[tokio::test]
async fn source_and_escalation_answers_name_no_hidden_member() -> TestResult {
    let mut scenes = scenes("sources").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        let mut feed = source("feed", &[PRIVATE, SHARED]);
        feed["conversation_members"] = json!([member(PRIVATE)]);
        scene.served(&alice, "sources/put", feed).await;
        let mut desk = source("desk", &[PRIVATE, SHARED]);
        desk["routes"] = json!([{
            "kind": "triage",
            "member": member(PRIVATE),
            "forward_topics": ["desk.task"],
        }]);
        scene.served(&alice, "sources/put", desk).await;
        scene
            .served(
                &alice,
                "subscriptions/put",
                subscription("sub-private", PRIVATE),
            )
            .await;
        scene.served(&alice, "announce", news("k-1")).await;
        scene.owner.suspend(PRIVATE);
        scene.owner.round_at(10_000_000).await;
        scene.owner.round_at(10_000_000 + 61 * 60_000).await;

        let reads = [
            ("sources/get", json!({ "mob_id": MOB, "source_id": "feed" })),
            ("sources/get", json!({ "mob_id": MOB, "source_id": "desk" })),
            ("sources/list", json!({ "mob_id": MOB })),
            ("escalations/list", json!({ "open": true })),
            ("escalations/list", json!({})),
        ];
        for (verb, params) in &reads {
            let owners = scene.served(&alice, verb, params.clone()).await;
            assert!(
                owners.to_string().contains(PRIVATE),
                "{verb} {params}: its owner reads the member: {owners:#}"
            );
            let others = scene.expected(&bob, verb, params.clone()).await;
            scene.check(!others.to_string().contains(PRIVATE), || {
                format!("{verb} {params} names the hidden member")
            });
            if verb.starts_with("sources/") {
                scene.check(
                    others.to_string().contains(&member(SHARED).to_string()),
                    || format!("{verb} {params} lost a member the caller may see"),
                );
            }
        }
        let put = scene
            .expected(&bob, "sources/put", source("mine", &[SHARED]))
            .await;
        scene.check(put["source"]["source_id"] == json!("mine"), || {
            format!("a source of its caller's own is answered {put}")
        });
        let mut revision = 1;
        for verb in ["sources/pause", "sources/resume", "sources/end"] {
            let changed = scene
                .expected(&bob, verb, source_change(verb, "feed", revision))
                .await;
            scene.check(!changed.to_string().contains(PRIVATE), || {
                format!("{verb} names the hidden member")
            });
            revision = changed["source"]["revision"].as_u64().unwrap_or(revision);
        }
    }
    nothing_found(scenes);
    Ok(())
}

/// A declared source: the `sources/put` request without its operation id.
fn declared_source(revision: u64, mut source: Value) -> Value {
    if let Some(fields) = source.as_object_mut() {
        fields.remove("operation_id");
    }
    json!({
        "revision": revision,
        "declarations": { "revision": revision, "sources": [source] },
    })
}

/// A source put on the console, and a source declared in an apply there,
/// names only seated members its caller may see: one that names another
/// member, in its audience, a triage route, a forward audience or its
/// conversation members, is refused with one answer, whoever holds the
/// name, and nothing is written. A member the caller may see is named as
/// before.
#[tokio::test]
async fn a_source_names_only_members_its_caller_may_see() -> TestResult {
    let mut scenes = scenes("source-members").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        let naming = |place: &str, identity: &str| {
            let mut request = source("mine", &[SHARED]);
            let grant = json!({ "kind": "member", "member": member(identity) });
            match place {
                "audience" => request["audience"] = json!([grant]),
                "triage route" => {
                    request["routes"] = json!([{
                        "kind": "triage",
                        "member": member(identity),
                        "forward_topics": ["mine.task"],
                    }]);
                }
                "forward audience" => {
                    request["routes"] = json!([{
                        "kind": "triage",
                        "member": member(SHARED),
                        "forward_topics": ["mine.task"],
                        "forward_audience": [grant],
                    }]);
                }
                _ => request["conversation_members"] = json!([member(identity)]),
            }
            request
        };
        let places = [
            "audience",
            "triage route",
            "forward audience",
            "conversation members",
        ];
        for place in places {
            let hidden = scene
                .call(&bob, "sources/put", naming(place, PRIVATE))
                .await;
            scene.check(
                hidden["error"]["data"]["kind"] == json!("unknown_member"),
                || {
                    format!(
                        "a source naming a hidden member in its {place}: {}",
                        brief(&hidden.to_string())
                    )
                },
            );
            let hidden = masked(&hidden, &[PRIVATE]);
            let absent = masked(
                &scene.call(&bob, "sources/put", naming(place, NOBODY)).await,
                &[NOBODY],
            );
            scene.check(hidden == absent, || {
                format!(
                    "a source's {place} tells the two names apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
        let declaring =
            |identity: &str| declared_source(1, naming("conversation members", identity));
        let hidden = scene
            .call(&bob, "declarations/apply", declaring(PRIVATE))
            .await;
        scene.check(
            hidden["error"]["data"]["kind"] == json!("unknown_member"),
            || {
                format!(
                    "a declared source naming a hidden member: {}",
                    brief(&hidden.to_string())
                )
            },
        );
        let hidden = masked(&hidden, &[PRIVATE]);
        let absent = masked(
            &scene
                .call(&bob, "declarations/apply", declaring(NOBODY))
                .await,
            &[NOBODY],
        );
        scene.check(hidden == absent, || {
            format!(
                "a declared source tells the two names apart: {} | {}",
                brief(&hidden),
                brief(&absent)
            )
        });

        let listed = scene
            .served(&alice, "sources/list", json!({ "mob_id": MOB }))
            .await;
        scene.check(listed["sources"] == json!([]), || {
            format!(
                "a refused source was written: {}",
                brief(&listed.to_string())
            )
        });
        let view = scene.served(&alice, "declarations/get", json!({})).await;
        scene.check(view["revision"] == json!(0), || {
            format!("a refused apply was applied: {}", brief(&view.to_string()))
        });
        scene
            .expected(&bob, "sources/put", source("mine", &[SHARED]))
            .await;
    }
    nothing_found(scenes);
    Ok(())
}

/// A refusal that carries a member of a record, where the request named
/// none, names no member its caller may not see. A source whose routes a
/// hidden member's subscriptions would reach twice is refused without the
/// member, on a put and in the pending refusal of a declared source; a
/// caller who may see the member is answered with it.
#[tokio::test]
async fn a_refusal_carries_no_member_its_caller_may_not_see() -> TestResult {
    let mut scenes = scenes("refusals").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        for (id, topic) in [
            ("sub-direct", "cross.received"),
            ("sub-forward", "cross.task"),
        ] {
            let mut request = subscription(id, PRIVATE);
            request["topics"] = json!([topic]);
            scene.served(&alice, "subscriptions/put", request).await;
        }
        let crossing = || {
            let mut request = source("cross", &[SHARED]);
            request["topics"] = json!(["cross.received", "cross.inbox"]);
            request["routes"] = json!([
                { "kind": "direct", "topics": ["cross.received"] },
                {
                    "kind": "triage",
                    "member": member(SHARED),
                    "topics": ["cross.inbox"],
                    "forward_topics": ["cross.task"],
                },
            ]);
            request
        };
        // The member as a record's key spells it.
        let key = format!("{MOB}\u{1f}{PRIVATE}");
        let names_the_member = |answer: &Value| {
            answer.to_string().contains(PRIVATE) || answer.to_string().contains(&key)
        };

        let owners = scene.call(&alice, "sources/put", crossing()).await;
        assert_eq!(
            owners["error"]["data"]["kind"],
            json!("route_overlap"),
            "{owners:#}"
        );
        assert_eq!(
            owners["error"]["data"]["member"],
            member(PRIVATE),
            "its owner is answered with the member: {owners:#}"
        );
        let others = scene.call(&bob, "sources/put", crossing()).await;
        scene.check(
            others["error"]["data"] == json!({ "kind": "route_overlap", "source_id": "cross" }),
            || format!("the refusal is answered {}", brief(&others.to_string())),
        );
        scene.check(!names_the_member(&others), || {
            "the refusal names the hidden member".to_string()
        });

        let applied = scene
            .expected(&bob, "declarations/apply", declared_source(1, crossing()))
            .await;
        scene.check(
            applied["records"][0]["refusal"]["kind"] == json!("route_overlap"),
            || format!("the declared source stands {}", brief(&applied.to_string())),
        );
        scene.check(!names_the_member(&applied), || {
            "the apply's answer names the hidden member".to_string()
        });
        let view = scene.expected(&bob, "declarations/get", json!({})).await;
        scene.check(!names_the_member(&view), || {
            "the declared view names the hidden member".to_string()
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// A member of another mob is not the agent of this one that holds its
/// name: a declared subscription of a member of another mob, and its
/// record, are answered alike whether a member hidden from the caller holds
/// the name here or nobody does.
#[tokio::test]
async fn a_member_of_another_mob_is_judged_as_itself() -> TestResult {
    let mut scenes = scenes("other-mob").await?;
    for scene in &mut scenes {
        let bob = scene.bob.clone();
        let of_peer = |id: &str, identity: &str| {
            json!({
                "subscription_id": id,
                "member": { "mob_id": SurfaceOwner::PEER_MOB, "identity": identity },
                "topics": ["news"],
            })
        };
        let apply = |revision: u64, subscriptions: Vec<Value>| {
            json!({
                "revision": revision,
                "declarations": { "revision": revision, "subscriptions": subscriptions },
            })
        };
        let first = scene
            .call(
                &bob,
                "declarations/apply",
                apply(1, vec![of_peer("decl-a", PRIVATE)]),
            )
            .await;
        let second = scene
            .call(
                &bob,
                "declarations/apply",
                apply(
                    2,
                    vec![of_peer("decl-a", PRIVATE), of_peer("decl-b", NOBODY)],
                ),
            )
            .await;
        scene.check(
            first["error"].is_null() && second["error"].is_null(),
            || {
                format!(
                    "the applies are answered {} | {}",
                    brief(&first.to_string()),
                    brief(&second.to_string())
                )
            },
        );
        let record = |id: &str| {
            second["result"]["records"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|record| record["id"] == json!(id))
                .cloned()
                .unwrap_or(Value::Null)
        };
        let (hidden, absent) = (
            masked(&record("decl-a"), &["decl-a"]),
            masked(&record("decl-b"), &["decl-b"]),
        );
        scene.check(hidden == absent && !record("decl-a").is_null(), || {
            format!("the declared records stand {hidden} | {absent}")
        });
        let hidden = masked(
            &scene
                .call(
                    &bob,
                    "subscriptions/get",
                    json!({ "subscription_id": "decl-a" }),
                )
                .await,
            &["decl-a", PRIVATE],
        );
        let absent = masked(
            &scene
                .call(
                    &bob,
                    "subscriptions/get",
                    json!({ "subscription_id": "decl-b" }),
                )
                .await,
            &["decl-b", NOBODY],
        );
        scene.check(hidden == absent && hidden.contains("\"state\""), || {
            format!("the records read {} | {}", brief(&hidden), brief(&absent))
        });
        let listed = scene.call(&bob, "subscriptions/list", json!({})).await;
        let rows = listed["result"]["subscriptions"]
            .as_array()
            .map_or(0, Vec::len);
        scene.check(rows == 2, || {
            format!("the listing holds {}", brief(&listed.to_string()))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// A request naming a member hidden from its caller is answered as the
/// same request naming a name no member holds, and records nothing.
#[tokio::test]
async fn a_request_naming_a_hidden_member_is_answered_as_one_naming_nobody() -> TestResult {
    let mut scenes = scenes("named").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        let requests = |identity: &str| {
            vec![
                ("subscriptions/put", subscription("sub-other", identity)),
                ("members/get", json!({ "member": member(identity) })),
                ("subscriptions/list", json!({ "member": member(identity) })),
                ("deliveries/list", json!({ "member": member(identity) })),
                (
                    "audit/list",
                    json!({ "mob_id": MOB, "member": member(identity) }),
                ),
            ]
        };
        for ((verb, hidden), (_, absent)) in requests(PRIVATE).into_iter().zip(requests(NOBODY)) {
            let hidden = masked(&scene.call(&bob, verb, hidden).await, &[PRIVATE]);
            let absent = masked(&scene.call(&bob, verb, absent).await, &[NOBODY]);
            scene.check(hidden == absent, || {
                format!(
                    "{verb} tells the two names apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
        let listed = scene
            .served(
                &alice,
                "subscriptions/list",
                json!({ "member": member(PRIVATE) }),
            )
            .await;
        scene.check(listed["subscriptions"] == json!([]), || {
            format!("something was recorded: {}", brief(&listed.to_string()))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// A listing's page is filled from the rows its caller may see: the rows of
/// a hidden member neither show nor take a place on the page.
#[tokio::test]
async fn listings_are_filled_from_the_rows_their_caller_may_see() -> TestResult {
    let mut scenes = scenes("listings").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        for id in ["a-1", "a-2"] {
            scene
                .served(&alice, "subscriptions/put", subscription(id, PRIVATE))
                .await;
        }
        for id in ["b-1", "b-2"] {
            scene
                .served(&bob, "subscriptions/put", subscription(id, SHARED))
                .await;
        }
        scene.served(&alice, "announce", news("k-1")).await;
        scene.owner.round_at(100_000).await;
        let owners = scene.served(&alice, "subscriptions/list", json!({})).await;
        assert_eq!(
            owners["subscriptions"].as_array().map(Vec::len),
            Some(4),
            "its owner is listed every row: {owners:#}"
        );

        let subscriptions = scene
            .served(&bob, "subscriptions/list", json!({ "limit": 2 }))
            .await;
        let listed: Vec<&str> = subscriptions["subscriptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["id"].as_str())
            .collect();
        scene.check(listed == ["b-1", "b-2"], || {
            format!("subscriptions/list holds {listed:?}")
        });
        for (verb, key) in [
            ("deliveries/list", "deliveries"),
            ("members/list", "members"),
        ] {
            let listed = scene.served(&bob, verb, json!({ "limit": 1 })).await;
            let rows = listed[key].as_array().cloned().unwrap_or_default();
            scene.check(
                rows.len() == 1 && rows[0]["member"] == member(SHARED),
                || format!("{verb} holds {}", brief(&listed.to_string())),
            );
        }

        // An audit entry of the private member: its subscription to a topic
        // of a source whose audience does not take it is refused and counted.
        scene
            .served(&alice, "sources/put", source("feed", &[SHARED]))
            .await;
        let mut outside = subscription("a-3", PRIVATE);
        outside["topics"] = json!(["feed.received"]);
        let refused = scene.call(&alice, "subscriptions/put", outside).await;
        assert!(!refused["error"].is_null(), "{refused:#}");
        let entries = json!({ "mob_id": MOB });
        let owners = scene.served(&alice, "audit/list", entries.clone()).await;
        assert!(
            owners.to_string().contains(PRIVATE),
            "its owner reads the entry: {owners:#}"
        );
        let others = scene.served(&bob, "audit/list", entries).await;
        scene.check(!others.to_string().contains(PRIVATE), || {
            format!("audit/list holds {}", brief(&others.to_string()))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// A source key under which a member hidden from the caller announced is
/// answered to that caller as taken, without the announcement it holds,
/// and reads as a key nothing was announced under. A caller who may see
/// the member is answered with the announcement, as everyone is for a key
/// the host announced under.
#[tokio::test]
async fn a_key_a_hidden_member_announced_under_is_answered_as_taken() -> TestResult {
    let mut scenes = scenes("taken-key").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene
            .owner
            .announce_as(PRIVATE, "k-private", "from the private member")
            .await;
        scene.served(&alice, "announce", news("k-host")).await;
        let other = |key: &str| {
            let mut request = news(key);
            request["body"] = json!("something else");
            request
        };
        let read = json!({ "mob_id": MOB, "source_key": "k-private" });

        let owners = scene.call(&alice, "announce", other("k-private")).await;
        assert_eq!(
            owners["error"]["data"]["existing"]["announced_by"],
            json!({ "kind": "member", "identity": PRIVATE }),
            "its owner is answered with the announcement: {owners:#}"
        );
        let owners = scene.served(&alice, "get", read.clone()).await;
        assert!(owners["announcement"].is_object(), "{owners:#}");
        let hosts = scene.call(&bob, "announce", other("k-host")).await;
        assert!(
            hosts["error"]["data"]["existing"].is_object(),
            "a key the host announced under: {hosts:#}"
        );

        let others = scene.call(&bob, "announce", other("k-private")).await;
        scene.check(
            others["error"]["code"] == hosts["error"]["code"]
                && others["error"]["data"] == json!({ "kind": "announcement_conflict" }),
            || format!("the conflict is answered {}", brief(&others.to_string())),
        );
        scene.check(!others.to_string().contains(PRIVATE), || {
            "the conflict names the hidden member".to_string()
        });
        let others = scene.expected(&bob, "get", read).await;
        scene.check(others["announcement"].is_null(), || {
            format!("the key reads {}", brief(&others.to_string()))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// What a member private to a person announces reaches no subscription of
/// a member that is not that person's, whether the subscription starts now
/// or at an earlier frontier; every other announcement reaches it as
/// before, and the private member's own subscription receives its own.
#[tokio::test]
async fn a_private_members_announcement_reaches_no_other_persons_subscription() -> TestResult {
    const WORDS: &str = "from the private member";
    let mut scenes = scenes("announcer-audience").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene.owner.give_role(SHARED, "worker");
        scene.owner.give_person(PRIVATE, "worker", "person-alice");
        // Retained before any subscription exists.
        scene.owner.announce_as(PRIVATE, "k-early", WORDS).await;
        scene.served(&alice, "announce", news("k-host")).await;
        let mut frontier = serde_json::Map::new();
        frontier.insert(MOB.to_string(), json!(0));
        let mut early = subscription("sub-early", SHARED);
        early["start"] = json!({ "kind": "after", "frontier": frontier });
        scene.served(&bob, "subscriptions/put", early).await;
        scene
            .served(
                &alice,
                "subscriptions/put",
                subscription("sub-own", PRIVATE),
            )
            .await;
        // Announced while both subscriptions are live.
        scene.owner.announce_as(PRIVATE, "k-later", WORDS).await;
        scene.owner.round_at(100_000).await;

        let shared = scene
            .served(&bob, "deliveries/list", json!({ "member": member(SHARED) }))
            .await
            .to_string();
        scene.check(shared.contains("the build is green"), || {
            format!(
                "the host's announcement is not replayed: {}",
                brief(&shared)
            )
        });
        scene.check(!shared.contains(WORDS), || {
            "a delivery to the shared member carries the private member's words".to_string()
        });
        scene.check(!shared.contains(PRIVATE), || {
            "a delivery to the shared member names the private member".to_string()
        });
        let own = scene
            .served(
                &alice,
                "deliveries/list",
                json!({ "member": member(PRIVATE) }),
            )
            .await
            .to_string();
        scene.check(own.contains(WORDS), || {
            format!("its own subscription receives {}", brief(&own))
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// What a member private to a person announced is read by key by its
/// person, and reads as nothing announced for another person, while the
/// member lives and after it has ended.
#[tokio::test]
async fn a_key_an_ended_private_member_announced_under_reads_as_nothing_announced() -> TestResult {
    let mut scenes = scenes("ended-announcer").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene
            .owner
            .announce_as(PRIVATE, "k-private", "from the private member")
            .await;
        let read = json!({ "mob_id": MOB, "source_key": "k-private" });
        let others = scene.expected(&bob, "get", read.clone()).await;
        scene.check(others["announcement"].is_null(), || {
            format!(
                "while it lives the key reads {}",
                brief(&others.to_string())
            )
        });

        scene
            .runtime
            .handle()
            .retire(meerkat_mob::AgentIdentity::from(PRIVATE))
            .await?;
        let others = scene.expected(&bob, "get", read.clone()).await;
        scene.check(others["announcement"].is_null(), || {
            format!(
                "after it ended the key reads {}",
                brief(&others.to_string())
            )
        });
        let owners = scene.served(&alice, "get", read).await;
        assert!(
            owners["announcement"].is_object(),
            "its person still reads it: {owners:#}"
        );
    }
    nothing_found(scenes);
    Ok(())
}

/// A role template's change lists its children: none of a member its
/// caller may not see. A caller who may see the member is listed its child.
#[tokio::test]
async fn a_role_templates_change_lists_no_child_of_a_hidden_member() -> TestResult {
    let mut scenes = scenes("role-children").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        for identity in [SHARED, PRIVATE] {
            scene.owner.give_role(identity, "crew");
        }
        let template = scene
            .served(
                &alice,
                "subscriptions/put",
                json!({
                    "operation_id": "put-crew",
                    "subscription_id": "crew-news",
                    "members": { "mob_id": MOB, "role": "crew" },
                    "topics": ["news"],
                    "mobs": [MOB],
                }),
            )
            .await;
        // The children are made when an announcement is captured.
        scene.served(&alice, "announce", news("k-1")).await;
        let change = |verb: &str, revision: &Value| {
            json!({
                "operation_id": format!("{verb}-crew"),
                "subscription_id": "crew-news",
                "expected_revision": revision,
            })
        };
        let children = |answer: &Value| -> BTreeSet<String> {
            answer["children"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|child| child["member"]["identity"].as_str())
                .map(str::to_string)
                .collect()
        };
        let owners = scene
            .served(
                &alice,
                "subscriptions/pause",
                change("pause", &template["subscription"]["revision"]),
            )
            .await;
        assert_eq!(
            children(&owners),
            ids(&[SHARED, PRIVATE]),
            "its owner is listed both children: {owners:#}"
        );
        let others = scene
            .expected(
                &bob,
                "subscriptions/resume",
                change("resume", &owners["template"]["revision"]),
            )
            .await;
        scene.check(children(&others) == ids(&[SHARED]), || {
            format!("the change lists {:?}", children(&others))
        });
        scene.check(!others.to_string().contains(PRIVATE), || {
            "the change names the hidden member".to_string()
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// The record of the role child `id` in a declarations view.
fn role_child<'a>(view: &'a Value, id: &str) -> Option<&'a Value> {
    view["records"]
        .as_array()?
        .iter()
        .find(|record| record["kind"] == json!("role_child") && record["id"] == json!(id))
}

/// What a row of [`a_role_child_is_judged_by_the_member_it_was_made_for`]
/// finds under the id of its child.
#[derive(Clone, Copy, PartialEq)]
enum UnderItsId {
    /// No subscription has the child's id.
    Nothing,
    /// A subscription of another member, made at runtime, has the child's
    /// id.
    AnotherMembersSubscription,
}

/// A role template's child is the record of the member it was made for, at
/// every status and whatever is under its id: the declared view and the
/// escalation listings answer it to a caller who may see that member, and
/// to no other.
///
/// The row declares a template for a role two members hold, the private one
/// refused by its check, so that its child is pending and no subscription
/// is made. For another `status` the check then passes and the child is
/// made, is brought to the status, and its subscription is cancelled and
/// removed by a retention pass; a template that is paused keeps a child as
/// it is, since only a live template's children are reconciled.
async fn a_role_child_is_judged_by_the_member_it_was_made_for(
    status: &str,
    under_its_id: UnderItsId,
) -> TestResult {
    const TEMPLATE: &str = "crew-news";
    let (row, case) = match under_its_id {
        UnderItsId::Nothing => (
            format!("role-child-{status}"),
            format!("the {status} child"),
        ),
        UnderItsId::AnotherMembersSubscription => (
            format!("role-child-{status}-held-id"),
            format!("the {status} child whose id another member's subscription holds"),
        ),
    };
    let mut scenes = scenes(&row).await?;
    for scene in &mut scenes {
        let console = scene.console;
        let (alice, bob, root) = (scene.alice.clone(), scene.bob.clone(), scene.root.clone());
        for identity in [SHARED, PRIVATE] {
            scene.owner.give_role(identity, "crew");
        }
        scene.owner.unseat(PRIVATE);
        let crew = json!({
            "revision": 1,
            "declarations": {
                "revision": 1,
                "subscriptions": [{
                    "subscription_id": TEMPLATE,
                    "members": { "mob_id": MOB, "role": "crew" },
                    "topics": ["news"],
                }],
            },
        });
        // An apply of the applied revision records nothing new and
        // reconciles the template's children.
        let applied = scene
            .served(&alice, "declarations/apply", crew.clone())
            .await;
        let pending: Vec<&Value> = applied["records"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|record| {
                record["kind"] == json!("role_child") && record["status"] == json!("pending")
            })
            .collect();
        assert_eq!(
            pending.len(),
            1,
            "{console}: one pending child: {applied:#}"
        );
        let child = pending[0]["id"].as_str().expect("a child id").to_string();
        let refusal = &pending[0]["refusal"];
        assert_eq!(
            refusal["kind"],
            json!("unknown_member"),
            "{console}: {applied:#}"
        );
        let told = refusal["message"].as_str().unwrap_or_default();
        assert!(
            told.contains(PRIVATE) && told.contains(MOB),
            "{console}: the refusal names the member and its mob: {told}"
        );
        let read = json!({ "subscription_id": child });
        let change = |verb: &str, id: &str, revision: &Value| {
            json!({
                "operation_id": format!("{verb}-{id}"),
                "subscription_id": id,
                "expected_revision": revision,
            })
        };

        if status != "pending" {
            scene.owner.seat(PRIVATE);
            scene
                .served(&alice, "declarations/apply", crew.clone())
                .await;
            let made = scene
                .served(&alice, "subscriptions/get", read.clone())
                .await;
            assert_eq!(
                (
                    &made["subscription"]["member"],
                    &made["subscription"]["state"]
                ),
                (&member(PRIVATE), &json!("active")),
                "{console}: the child is made once its member's check passes: {made:#}"
            );
            if status == "left" {
                scene.owner.give_role(PRIVATE, "bench");
                scene
                    .served(&alice, "declarations/apply", crew.clone())
                    .await;
            }
            if status != "suppressed" {
                let template = scene
                    .served(
                        &alice,
                        "subscriptions/get",
                        json!({ "subscription_id": TEMPLATE }),
                    )
                    .await;
                scene
                    .served(
                        &alice,
                        "subscriptions/pause",
                        change("pause", TEMPLATE, &template["template"]["revision"]),
                    )
                    .await;
            }
            let found = scene
                .served(&alice, "subscriptions/get", read.clone())
                .await;
            scene
                .served(
                    &alice,
                    "subscriptions/cancel",
                    change("cancel", &child, &found["subscription"]["revision"]),
                )
                .await;
            if status == "suppressed" {
                scene
                    .served(&alice, "declarations/apply", crew.clone())
                    .await;
            }
            scene.owner.retention_pass_at(2_000).await;
        }
        if under_its_id == UnderItsId::AnotherMembersSubscription {
            scene
                .served(&alice, "subscriptions/put", subscription(&child, SHARED))
                .await;
        }
        // What the child's id reads as: checked here and again at the end.
        let held_by = match under_its_id {
            UnderItsId::Nothing => Value::Null,
            UnderItsId::AnotherMembersSubscription => member(SHARED),
        };
        let found = scene
            .served(&alice, "subscriptions/get", read.clone())
            .await;
        assert_eq!(
            found["subscription"]["member"], held_by,
            "{console}: {case}: what its id reads as: {found:#}"
        );

        let mut audience = vec![("the member's person", alice.clone())];
        if scene.enforced {
            audience.push(("an access administrator", root.clone()));
        }
        for (who, bearer) in &audience {
            let view = scene.served(bearer, "declarations/get", json!({})).await;
            assert_eq!(
                role_child(&view, &child).map(|record| &record["status"]),
                Some(&json!(status)),
                "{console}: {case}: {who} reads its record: {view:#}"
            );
            let open = scene
                .served(bearer, "escalations/list", json!({ "open": true }))
                .await;
            assert_eq!(
                open.to_string().contains(&child),
                status == "pending",
                "{console}: {case}: {who} reads its escalation while it is pending: {open:#}"
            );
            let events = scene.served(bearer, "escalations/list", json!({})).await;
            assert!(
                events.to_string().contains(&child),
                "{console}: {case}: {who} reads its escalation's events: {events:#}"
            );
        }

        let asked = [
            ("declarations/get", "declarations/get", json!({})),
            (
                "escalations/list (open)",
                "escalations/list",
                json!({ "open": true }),
            ),
            ("escalations/list (events)", "escalations/list", json!({})),
            (
                "the answer of declarations/apply",
                "declarations/apply",
                crew.clone(),
            ),
        ];
        for (what, verb, params) in asked {
            let others = scene.expected(&bob, verb, params).await.to_string();
            scene.check(!others.contains(&child), || {
                format!("{case}: {what} holds its id")
            });
            scene.check(!others.contains(PRIVATE), || {
                format!("{case}: {what} names its member")
            });
            scene.check(!others.contains(MOB), || {
                format!("{case}: {what} names its member's mob")
            });
        }
        let listed = scene.expected(&bob, "declarations/get", json!({})).await;
        let shown = listed["records"].as_array().map_or(0, Vec::len);
        scene.check(
            declared_ids(&listed).contains(TEMPLATE) && shown == 2,
            || {
                format!(
                    "{case}: the caller is listed {:?}, not the template and the child of the \
                     member it may see",
                    declared_ids(&listed)
                )
            },
        );

        // The other caller's apply left the child, and what is under its
        // id, as they stood.
        let kept = scene.served(&alice, "declarations/get", json!({})).await;
        assert_eq!(
            role_child(&kept, &child).map(|record| &record["status"]),
            Some(&json!(status)),
            "{console}: {case}: it stands after the other caller's apply: {kept:#}"
        );
        let still = scene
            .served(&alice, "subscriptions/get", read.clone())
            .await;
        assert_eq!(
            still["subscription"]["member"], held_by,
            "{console}: {case}: what its id reads as after the other caller's apply: {still:#}"
        );
    }
    nothing_found(scenes);
    Ok(())
}

#[tokio::test]
async fn a_pending_role_child_is_judged_by_the_member_it_was_made_for() -> TestResult {
    a_role_child_is_judged_by_the_member_it_was_made_for("pending", UnderItsId::Nothing).await
}

#[tokio::test]
async fn an_applied_role_child_is_judged_by_the_member_it_was_made_for() -> TestResult {
    a_role_child_is_judged_by_the_member_it_was_made_for("applied", UnderItsId::Nothing).await
}

#[tokio::test]
async fn a_left_role_child_is_judged_by_the_member_it_was_made_for() -> TestResult {
    a_role_child_is_judged_by_the_member_it_was_made_for("left", UnderItsId::Nothing).await
}

#[tokio::test]
async fn a_suppressed_role_child_is_judged_by_the_member_it_was_made_for() -> TestResult {
    a_role_child_is_judged_by_the_member_it_was_made_for("suppressed", UnderItsId::Nothing).await
}

/// A pending child whose id a subscription of another member holds (one
/// made at runtime under that id) is still the record of the member it was
/// made for.
#[tokio::test]
async fn a_role_child_under_another_members_id_is_judged_by_the_member_it_was_made_for()
-> TestResult {
    a_role_child_is_judged_by_the_member_it_was_made_for(
        "pending",
        UnderItsId::AnotherMembersSubscription,
    )
    .await
}

/// A member of a mob this gateway holds that runs no more is not seated: a
/// declared subscription and a source that name one are refused with the
/// one answer, whoever holds the name.
#[tokio::test]
async fn a_member_of_a_mob_that_runs_no_more_is_not_seated() -> TestResult {
    let mut scenes = scenes("finished-mob").await?;
    for scene in &mut scenes {
        let bob = scene.bob.clone();
        scene.owner.complete_hosted_mob();
        let of_finished =
            |identity: &str| json!({ "mob_id": SurfaceOwner::HOSTED_MOB, "identity": identity });
        let declaring = |identity: &str| {
            json!({
                "revision": 1,
                "declarations": {
                    "revision": 1,
                    "subscriptions": [{
                        "subscription_id": "decl-finished",
                        "member": of_finished(identity),
                        "topics": ["news"],
                    }],
                },
            })
        };
        let putting = |identity: &str| {
            let mut request = source("mine", &[SHARED]);
            request["conversation_members"] = json!([of_finished(identity)]);
            request
        };
        for (verb, hidden, absent) in [
            ("declarations/apply", declaring(PRIVATE), declaring(NOBODY)),
            ("sources/put", putting(PRIVATE), putting(NOBODY)),
        ] {
            let hidden = scene.call(&bob, verb, hidden).await;
            scene.check(
                hidden["error"]["data"]["kind"] == json!("unknown_member"),
                || format!("{verb} is answered {}", brief(&hidden.to_string())),
            );
            let hidden = masked(&hidden, &[PRIVATE]);
            let absent = masked(&scene.call(&bob, verb, absent).await, &[NOBODY]);
            scene.check(hidden == absent, || {
                format!(
                    "{verb} tells the two names apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
    }
    nothing_found(scenes);
    Ok(())
}

/// While the check of a member cannot be read, a declared subscription and a
/// source that name the member are refused for now with the one answer,
/// whoever holds the name, and nothing is applied or written.
#[tokio::test]
async fn a_member_whose_check_cannot_be_read_is_refused_for_now() -> TestResult {
    let mut scenes = scenes("held-mob").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene.owner.hold_hosted_mob();
        let of_held =
            |identity: &str| json!({ "mob_id": SurfaceOwner::HOSTED_MOB, "identity": identity });
        let declaring = |identity: &str| {
            json!({
                "revision": 1,
                "declarations": {
                    "revision": 1,
                    "subscriptions": [{
                        "subscription_id": "decl-held",
                        "member": of_held(identity),
                        "topics": ["news"],
                    }],
                },
            })
        };
        let putting = |identity: &str| {
            let mut request = source("mine", &[SHARED]);
            request["conversation_members"] = json!([of_held(identity)]);
            request
        };
        for (verb, hidden, absent) in [
            ("declarations/apply", declaring(PRIVATE), declaring(NOBODY)),
            ("sources/put", putting(PRIVATE), putting(NOBODY)),
        ] {
            let hidden = scene.call(&bob, verb, hidden).await;
            scene.check(
                hidden["error"]["data"]["kind"] == json!("native_unavailable"),
                || format!("{verb} is answered {}", brief(&hidden.to_string())),
            );
            let hidden = masked(&hidden, &[PRIVATE]);
            let absent = masked(&scene.call(&bob, verb, absent).await, &[NOBODY]);
            scene.check(hidden == absent, || {
                format!(
                    "{verb} tells the two names apart: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                )
            });
        }
        let view = scene.expected(&alice, "declarations/get", json!({})).await;
        scene.check(view["revision"] == json!(0), || {
            format!("a refused apply was applied: {}", brief(&view.to_string()))
        });
        let listed = scene
            .expected(&alice, "sources/list", json!({ "mob_id": MOB }))
            .await;
        scene.check(listed["sources"] == json!([]), || {
            format!(
                "a refused source was written: {}",
                brief(&listed.to_string())
            )
        });
    }
    nothing_found(scenes);
    Ok(())
}

/// The records of a member hidden from the caller stay its audience's alone
/// when the mob the member was seated in is held here no more.
#[tokio::test]
async fn records_outlive_their_members_mob_as_their_audiences_alone() -> TestResult {
    let mut scenes = scenes("ended-mob").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene.owner.seat_in_hosted_mob(PRIVATE);
        let mut request = subscription("sub-hosted", PRIVATE);
        request["member"] = json!({ "mob_id": SurfaceOwner::HOSTED_MOB, "identity": PRIVATE });
        scene.served(&alice, "subscriptions/put", request).await;
        let read = json!({ "subscription_id": "sub-hosted" });
        let absent = scene
            .call(
                &bob,
                "subscriptions/get",
                json!({ "subscription_id": "sub-nobody" }),
            )
            .await;
        for held in ["held here", "held here no more"] {
            if held == "held here no more" {
                scene.owner.destroy_hosted_mob();
            }
            let owners = scene
                .served(&alice, "subscriptions/get", read.clone())
                .await;
            assert_eq!(
                owners["subscription"]["member"]["identity"],
                json!(PRIVATE),
                "{held}: its owner reads it: {owners:#}"
            );
            let others = scene.call(&bob, "subscriptions/get", read.clone()).await;
            scene.check(others == absent, || {
                format!("{held}: the record reads {}", brief(&others.to_string()))
            });
            let listed = scene.served(&bob, "subscriptions/list", json!({})).await;
            scene.check(!listed.to_string().contains(PRIVATE), || {
                format!("{held}: the listing holds {}", brief(&listed.to_string()))
            });
        }
    }
    nothing_found(scenes);
    Ok(())
}

/// Every mob announcement method serves a caller who may see the member it
/// names or the member its record is of: the member's owner is answered by
/// each method itself.
#[tokio::test]
async fn every_announcement_method_serves_a_member_its_caller_may_see() -> TestResult {
    let scenes = scenes("served").await?;
    for scene in &scenes {
        let console = scene.console;
        let alice = scene.alice.as_str();
        let mut called = BTreeSet::new();
        let mut serve = |verb: &'static str| {
            called.insert(format!("meta/mob_announcements/{verb}"));
            verb
        };
        let own = member(PRIVATE);
        let by_id = json!({ "subscription_id": "sub-own" });
        let change = |verb: &str, revision: u64| {
            json!({
                "operation_id": format!("{verb}-own"),
                "subscription_id": "sub-own",
                "expected_revision": revision,
            })
        };

        let put = scene
            .served(
                alice,
                serve("subscriptions/put"),
                subscription("sub-own", PRIVATE),
            )
            .await;
        assert_eq!(put["subscription"]["member"], own, "{console}: {put:#}");
        scene.served(alice, serve("announce"), news("k-1")).await;
        let announced = scene
            .served(
                alice,
                serve("get"),
                json!({ "mob_id": MOB, "source_key": "k-1" }),
            )
            .await;
        assert!(
            announced["announcement"].is_object(),
            "{console}: {announced:#}"
        );
        serve("deliveries/list");
        let batch = sealed_batch(scene, PRIVATE).await;

        let read = scene
            .served(alice, serve("subscriptions/get"), by_id.clone())
            .await;
        assert_eq!(read["subscription"]["member"], own, "{console}: {read:#}");
        let listed = scene
            .served(alice, serve("subscriptions/list"), json!({ "member": own }))
            .await;
        assert_eq!(
            listed["subscriptions"].as_array().map(Vec::len),
            Some(1),
            "{console}: {listed:#}"
        );
        scene
            .served(alice, serve("subscriptions/pause"), change("pause", 1))
            .await;
        scene
            .served(alice, serve("subscriptions/resume"), change("resume", 2))
            .await;

        for verb in [serve("deliveries/observe"), serve("deliveries/reconcile")] {
            let observed = scene
                .served(alice, verb, json!({ "batch_id": batch }))
                .await;
            assert_eq!(observed["batch"]["member"], own, "{console}: {verb}");
        }
        for verb in [serve("deliveries/redeliver"), serve("deliveries/dismiss")] {
            let answer = scene
                .call(
                    alice,
                    verb,
                    json!({ "operation_id": format!("{verb}-own"), "batch_id": batch }),
                )
                .await;
            assert!(
                answer["error"].is_null() || answer["error"]["data"]["kind"] == json!("not_held"),
                "{console}: {verb} is answered for the batch itself: {answer:#}"
            );
        }

        let declared_source = scene
            .served(alice, serve("sources/put"), source("inbox", &[PRIVATE]))
            .await;
        assert!(
            declared_source.to_string().contains(PRIVATE),
            "{console}: {declared_source:#}"
        );
        let of_source = |operation: &str, version: u64| {
            json!({
                "operation_id": operation,
                "mob_id": MOB,
                "source_id": "inbox",
                "expected_revision": 1,
                "expected_version": version,
                "principal": principal(),
            })
        };
        let mut baseline = of_source("baseline", 0);
        baseline["next_cursor"] = json!("c1");
        baseline["coverage"] = json!("complete");
        scene
            .served(alice, serve("sources/advance"), baseline)
            .await;
        let mut failure = of_source("failure", 1);
        failure["failure"] = json!({ "class": "provider_unavailable" });
        scene
            .served(alice, serve("sources/record_failure"), failure)
            .await;
        for (verb, params) in [
            (
                serve("sources/get"),
                json!({ "mob_id": MOB, "source_id": "inbox" }),
            ),
            (serve("sources/list"), json!({ "mob_id": MOB })),
        ] {
            let sources = scene.served(alice, verb, params).await;
            assert!(
                sources.to_string().contains(PRIVATE),
                "{console}: {verb}: {sources:#}"
            );
        }
        let mut revision = 1;
        for verb in [
            serve("sources/pause"),
            serve("sources/resume"),
            serve("sources/end"),
        ] {
            let changed = scene
                .served(alice, verb, source_change(verb, "inbox", revision))
                .await;
            revision = changed["source"]["revision"].as_u64().expect("a revision");
        }

        scene
            .served(alice, serve("audit/list"), json!({ "mob_id": MOB }))
            .await;
        let of_member = scene
            .served(alice, serve("members/get"), json!({ "member": own }))
            .await;
        assert_eq!(of_member["member"], own, "{console}: {of_member:#}");
        let members = scene.served(alice, serve("members/list"), json!({})).await;
        assert!(
            members.to_string().contains(PRIVATE),
            "{console}: {members:#}"
        );
        scene
            .served(alice, serve("escalations/list"), json!({}))
            .await;
        let applied = scene
            .served(
                alice,
                serve("declarations/apply"),
                declared(1, &[("decl-own", PRIVATE)]),
            )
            .await;
        assert_eq!(
            declared_ids(&applied),
            ids(&["decl-own"]),
            "{console}: {applied:#}"
        );
        let view = scene
            .served(alice, serve("declarations/get"), json!({}))
            .await;
        assert_eq!(
            declared_ids(&view),
            ids(&["decl-own"]),
            "{console}: {view:#}"
        );
        scene
            .served(alice, serve("subscriptions/cancel"), change("cancel", 3))
            .await;

        let every: BTreeSet<String> = crate::mob_announcements::rpc::ANNOUNCEMENT_METHODS
            .iter()
            .map(|method| (*method).to_string())
            .collect();
        assert_eq!(called, every, "{console}: every method has its row");
    }
    Ok(())
}

/// What a host does about a subscription it finds on a member: over the
/// host's own surface it lists the subscription, cancels it, and nothing
/// announced afterwards is delivered into the member.
#[tokio::test]
async fn a_host_lists_and_cancels_a_subscription_and_nothing_more_is_delivered() {
    let owner = SurfaceOwner::serving(&[PRIVATE]);
    let (service, mob) = (owner.service(), owner.mob());
    let host = |verb: &'static str, params: Value| {
        let (service, mob) = (service.clone(), mob.clone());
        async move {
            crate::mob_announcements::rpc::handle_announcement_method(
                Some(&service),
                &mob,
                &format!("meta/mob_announcements/{verb}"),
                &params,
            )
            .await
            .unwrap_or_else(|error| panic!("{verb}: {error:?}"))
        }
    };
    host("subscriptions/put", subscription("sub-found", PRIVATE)).await;
    host("announce", news("k-1")).await;
    owner.round_at(100_000).await;

    let listed = host("subscriptions/list", json!({})).await;
    let rows = listed["subscriptions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(rows.len(), 1, "{listed:#}");
    assert_eq!(rows[0]["id"], json!("sub-found"), "{listed:#}");
    assert_eq!(rows[0]["member"], member(PRIVATE), "{listed:#}");
    assert_eq!(rows[0]["state"], json!("active"), "{listed:#}");
    let deliveries = json!({ "member": member(PRIVATE) });
    let delivered = host("deliveries/list", deliveries.clone()).await;
    assert_eq!(
        delivered["deliveries"].as_array().map(Vec::len),
        Some(1),
        "{delivered:#}"
    );

    let cancelled = host(
        "subscriptions/cancel",
        json!({
            "operation_id": "cancel-found",
            "subscription_id": "sub-found",
            "expected_revision": rows[0]["revision"],
        }),
    )
    .await;
    assert_eq!(
        cancelled["subscription"]["state"],
        json!("cancelled"),
        "{cancelled:#}"
    );
    let again = host("announce", news("k-2")).await;
    assert_eq!(again["captured"], json!(0), "{again:#}");
    owner.round_at(200_000).await;
    let after = host("deliveries/list", deliveries).await;
    assert_eq!(
        after["deliveries"].as_array().map(Vec::len),
        Some(1),
        "nothing more is delivered: {after:#}"
    );
    println!("subscriptions/list: {listed}");
    println!("subscriptions/cancel: {cancelled}");
    println!("announce: {again}");
}

/// An access configuration under which every signed-in caller may do
/// everything to every agent and administer the runtime, and may address
/// the announcement owner's own mob ([`MOB`]) and no other mob: a separate
/// allow rule lists that mob, as the upgrade step for a caller who works
/// with another mob says. `root` is its one admin, who may address every
/// mob.
fn everyone_addresses_the_owners_mob_alone() -> AccessController {
    let everyone = |id: &str, actions: &[&str], mobs: &[&str]| AccessRule {
        id: id.to_string(),
        subjects: vec!["*".to_string()],
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
        mobs: mobs.iter().map(|mob| (*mob).to_string()).collect(),
        ..AccessRule::default()
    };
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![
            everyone(
                "everyone-administers-the-runtime",
                &["agent.*", crate::access::ACTION_RUNTIME_ADMIN],
                &[],
            ),
            everyone("everyone-addresses-the-owners-mob", &["agent.view"], &[MOB]),
        ],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("access controller")
}

/// Every row of the listing `verb` (its rows under `key`) as the holder of
/// `bearer` pages it, `limit` rows at a time: each page is asked after the
/// last row of the page before, until a page is not full.
async fn paged(app: &Router, bearer: &str, verb: &str, key: &str, limit: usize) -> Vec<Vec<Value>> {
    let mut pages: Vec<Vec<Value>> = Vec::new();
    loop {
        let mut params = json!({ "limit": limit });
        if let Some(last) = pages.last().and_then(|page| page.last()) {
            params["after"] = last["id"].clone();
        }
        let answer = console_rpc_as(
            app,
            Some(bearer),
            &format!("meta/mob_announcements/{verb}"),
            params,
        )
        .await;
        assert!(answer["error"].is_null(), "{verb}: {answer:#}");
        let page = answer["result"][key]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let full = page.len() == limit;
        if !page.is_empty() {
            pages.push(page);
        }
        if !full || pages.len() > 16 {
            return pages;
        }
    }
}

/// A caller who may not address a second mob is listed no record of a
/// member of that mob, by a listing that names no mob: neither the member's
/// subscriptions nor its deliveries. The method itself leaves those rows
/// out, so they take no place on a page: paged two at a time (and one at a
/// time), the caller is listed exactly the rows of the mob it may address,
/// in full pages, and the row a page ends on, which the next page is asked
/// after, is the same as if the other mob's rows were not there. An admin,
/// who may address every mob, is listed every row.
#[tokio::test]
async fn a_caller_is_listed_no_record_of_a_member_of_a_mob_it_may_not_address() -> TestResult {
    const BUILDER: &str = "builder";
    let (_state, runtime) = runtime("announcement-audience-second-mob").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    owner.seat_in_hosted_mob(BUILDER);
    let state_of_console =
        enforced_console(&runtime, Some(everyone_addresses_the_owners_mob_alone()));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));
    let of_the_second_mob = json!({ "mob_id": SurfaceOwner::HOSTED_MOB, "identity": BUILDER });

    // The admin files the records: three subscriptions of the member of the
    // owner's mob and, between them in id order, two of the member of the
    // second mob; then one announcement both members capture.
    for (id, second) in [
        ("s-1", false),
        ("s-2", true),
        ("s-3", false),
        ("s-4", true),
        ("s-5", false),
    ] {
        let mut request = subscription(id, SHARED);
        if second {
            request["member"] = of_the_second_mob.clone();
        }
        let put = console_rpc_as(
            &app,
            Some(&root),
            "meta/mob_announcements/subscriptions/put",
            request,
        )
        .await;
        assert!(put["error"].is_null(), "{id}: {put:#}");
    }
    let announced = console_rpc_as(
        &app,
        Some(&root),
        "meta/mob_announcements/announce",
        news("k-1"),
    )
    .await;
    assert!(announced["error"].is_null(), "{announced:#}");
    owner.round_at(100_000).await;

    let of_the_owners_mob = |row: &Value| row["member"]["mob_id"] == json!(MOB);
    let ids = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .filter_map(|row| row["id"].as_str().map(str::to_string))
            .collect()
    };
    let mut found = Vec::new();
    for (verb, key) in [
        ("subscriptions/list", "subscriptions"),
        ("deliveries/list", "deliveries"),
    ] {
        let every: Vec<Value> = paged(&app, &root, verb, key, 100)
            .await
            .into_iter()
            .flatten()
            .collect();
        assert!(
            every.iter().any(|row| row["member"] == of_the_second_mob),
            "{verb}: the admin is listed a record of the second mob's member: {every:#?}"
        );
        let addressable: Vec<Value> = every
            .iter()
            .filter(|row| of_the_owners_mob(row))
            .cloned()
            .collect();
        assert!(
            !addressable.is_empty(),
            "{verb}: the admin is listed a record of the owner's mob: {every:#?}"
        );
        for limit in [2, 1] {
            let pages = paged(&app, &bob, verb, key, limit).await;
            let listed: Vec<Value> = pages.iter().flatten().cloned().collect();
            if ids(&listed) != ids(&addressable) || !listed.iter().all(&of_the_owners_mob) {
                found.push(format!(
                    "{verb}, {limit} at a time: the caller is listed {:?}, the rows of the mob \
                     it may address are {:?}",
                    ids(&listed),
                    ids(&addressable)
                ));
            }
            let full = pages.iter().rev().skip(1).all(|page| page.len() == limit);
            if !full {
                found.push(format!(
                    "{verb}, {limit} at a time: a page before the last is not full: {:?}",
                    pages.iter().map(Vec::len).collect::<Vec<_>>()
                ));
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// `verb` of the mob announcement methods as the holder of `bearer`, which
/// must succeed: its result.
async fn served(app: &Router, bearer: &str, verb: &str, params: Value) -> Value {
    let answer = console_rpc_as(
        app,
        Some(bearer),
        &format!("meta/mob_announcements/{verb}"),
        params,
    )
    .await;
    assert!(answer["error"].is_null(), "{verb}: {answer:#}");
    answer["result"].clone()
}

/// A subscription a caller is shown names no mob the caller may not
/// address: not among the mobs it captures from, not among the mobs it
/// holds a frontier for, not among the mobs that ended. The method itself
/// leaves those entries out, in every answer that carries the subscription
/// (read by its id, listed, and answered by a pause and a resume), so the
/// caller reads it as it reads a subscription that never named the mob, and
/// the listing holds the same rows in the same order. An admin, who may
/// address every mob, reads every entry.
#[tokio::test]
async fn a_shown_subscription_names_no_mob_its_caller_may_not_address() -> TestResult {
    const SECOND: &str = SurfaceOwner::HOSTED_MOB;
    let (_state, runtime) = runtime("announcement-audience-shown-mobs").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    let state_of_console =
        enforced_console(&runtime, Some(everyone_addresses_the_owners_mob_alone()));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));

    // The admin announces into both mobs, so each has a sequence a frontier
    // can name, then files two subscriptions of the one member: one that
    // captures from both mobs, one from the owner's mob alone.
    let mut in_the_second_mob = news("k-0");
    in_the_second_mob["mob_id"] = json!(SECOND);
    served(&app, &root, "announce", in_the_second_mob).await;
    served(&app, &root, "announce", news("k-1")).await;
    let mut of_both_mobs = subscription("s-both", SHARED);
    of_both_mobs["mobs"] = json!([MOB, SECOND]);
    served(&app, &root, "subscriptions/put", of_both_mobs).await;
    served(
        &app,
        &root,
        "subscriptions/put",
        subscription("s-own", SHARED),
    )
    .await;

    let by_id = |id: &str| json!({ "subscription_id": id });
    let change = |verb: &str, id: &str, revision: &Value| {
        json!({
            "operation_id": format!("{verb}-{id}"),
            "subscription_id": id,
            "expected_revision": revision,
        })
    };
    // The three places of a subscription that name mobs.
    let mobs_named = |row: &Value| {
        json!({
            "mobs": row["selector"]["mobs"],
            "frontier": row["frontier"],
            "ended_mobs": row.get("ended_mobs").cloned().unwrap_or_else(|| json!([])),
        })
    };
    let names_the_second_mob =
        |answer: &Value| answer.to_string().contains(&format!("\"{SECOND}\""));
    let mut found = Vec::new();

    // What the admin reads: every entry.
    let whole = served(&app, &root, "subscriptions/get", by_id("s-both")).await;
    assert!(
        whole["subscription"]["selector"]["mobs"]
            .as_array()
            .is_some_and(|mobs| mobs.contains(&json!(SECOND))),
        "the admin reads the second mob among the mobs it captures from: {whole:#}"
    );
    assert!(
        whole["subscription"]["frontier"].get(SECOND).is_some(),
        "the admin reads a frontier for the second mob: {whole:#}"
    );
    let own = served(&app, &bob, "subscriptions/get", by_id("s-own")).await;
    let as_never_named = mobs_named(&own["subscription"]);
    assert_eq!(
        as_never_named["mobs"],
        json!([MOB]),
        "the caller reads the mob it may address: {own:#}"
    );

    // Read by its id.
    let read = served(&app, &bob, "subscriptions/get", by_id("s-both")).await;
    if names_the_second_mob(&read) || mobs_named(&read["subscription"]) != as_never_named {
        found.push(format!("subscriptions/get: {}", brief(&read.to_string())));
    }

    // Listed: the same rows in the same order as the admin's, none naming
    // the second mob.
    let ids = |rows: &Value| -> Vec<String> {
        rows.as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["id"].as_str().map(str::to_string))
            .collect()
    };
    let every = served(&app, &root, "subscriptions/list", json!({})).await;
    let listed = served(&app, &bob, "subscriptions/list", json!({})).await;
    assert_eq!(
        ids(&every["subscriptions"]),
        ["s-both", "s-own"],
        "the admin is listed both: {every:#}"
    );
    if ids(&listed["subscriptions"]) != ids(&every["subscriptions"]) {
        found.push(format!(
            "subscriptions/list: the caller is listed {:?}",
            ids(&listed["subscriptions"])
        ));
    }
    if names_the_second_mob(&listed) {
        found.push(format!(
            "subscriptions/list: {}",
            brief(&listed.to_string())
        ));
    }

    // Answered by a pause and a resume.
    let mut revision = read["subscription"]["revision"].clone();
    for verb in ["pause", "resume"] {
        let changed = served(
            &app,
            &bob,
            &format!("subscriptions/{verb}"),
            change(verb, "s-both", &revision),
        )
        .await;
        revision = changed["subscription"]["revision"].clone();
        if names_the_second_mob(&changed) || mobs_named(&changed["subscription"]) != as_never_named
        {
            found.push(format!(
                "subscriptions/{verb}: {}",
                brief(&changed.to_string())
            ));
        }
    }

    // The second mob is destroyed: it leaves the selector and is kept among
    // the mobs that ended, which the admin reads and the caller does not.
    owner.destroy_hosted_mob();
    owner
        .service()
        .reconcile_mobs()
        .await
        .expect("the destroyed mob is reconciled");
    let whole = served(&app, &root, "subscriptions/get", by_id("s-both")).await;
    assert!(
        whole["subscription"]["ended_mobs"]
            .as_array()
            .is_some_and(|mobs| mobs.contains(&json!(SECOND))),
        "the admin reads the second mob among the mobs that ended: {whole:#}"
    );
    let read = served(&app, &bob, "subscriptions/get", by_id("s-both")).await;
    if names_the_second_mob(&read) || mobs_named(&read["subscription"]) != as_never_named {
        found.push(format!(
            "subscriptions/get after the mob ended: {}",
            brief(&read.to_string())
        ));
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// A role template of a mob its caller may not address is answered as an id
/// nobody holds, read or changed, and stays as it was; an administrator
/// reads and changes it. A role template of a mob the caller may address
/// is read and changed by that caller, and names no other mob it may not
/// address among the mobs its children capture from.
#[tokio::test]
async fn a_role_template_of_a_mob_its_caller_may_not_address_is_answered_as_an_id_nobody_holds()
-> TestResult {
    const SECOND: &str = SurfaceOwner::HOSTED_MOB;
    const BUILDER: &str = "builder";
    let (_state, runtime) = runtime("announcement-audience-template-mobs").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    owner.seat_in_hosted_mob(BUILDER);
    owner.give_role(SHARED, "crew");
    let state_of_console =
        enforced_console(&runtime, Some(everyone_addresses_the_owners_mob_alone()));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));
    let call = |bearer: String, verb: String, params: Value| {
        let app = app.clone();
        async move {
            console_rpc_as(
                &app,
                Some(&bearer),
                &format!("meta/mob_announcements/{verb}"),
                params,
            )
            .await
        }
    };
    let template = |id: &str, of: &str, mobs: &[&str]| {
        json!({
            "operation_id": format!("put-{id}"),
            "subscription_id": id,
            "members": { "mob_id": of, "role": "crew" },
            "topics": ["news"],
            "mobs": mobs,
        })
    };
    served(
        &app,
        &root,
        "subscriptions/put",
        template("builds-news", SECOND, &[SECOND]),
    )
    .await;
    served(
        &app,
        &root,
        "subscriptions/put",
        template("crew-news", MOB, &[MOB, SECOND]),
    )
    .await;
    let read = |id: &str| json!({ "subscription_id": id });
    // The control: an administrator reads the template of the second mob
    // and changes it.
    let made = served(&app, &root, "subscriptions/get", read("builds-news")).await;
    assert_eq!(
        made["template"]["members"]["mob_id"],
        json!(SECOND),
        "an administrator reads the template: {made:#}"
    );
    let mut revision = made["template"]["revision"].clone();
    for (verb, state) in [("pause", "paused"), ("resume", "active")] {
        let changed = served(
            &app,
            &root,
            &format!("subscriptions/{verb}"),
            json!({
                "operation_id": format!("{verb}-builds-as-an-administrator"),
                "subscription_id": "builds-news",
                "expected_revision": revision,
            }),
        )
        .await;
        assert_eq!(
            changed["template"]["state"],
            json!(state),
            "an administrator changes it: {changed:#}"
        );
        revision = changed["template"]["revision"].clone();
    }
    let before = served(&app, &root, "subscriptions/get", read("builds-news")).await;

    let mut found = Vec::new();
    for verb in ["get", "pause", "resume", "cancel"] {
        let request = |id: &str| match verb {
            "get" => json!({ "subscription_id": id }),
            _ => json!({
                "operation_id": format!("{verb}-{id}"),
                "subscription_id": id,
                "expected_revision": revision,
            }),
        };
        let method = format!("subscriptions/{verb}");
        let hidden = masked(
            &call(bob.clone(), method.clone(), request("builds-news")).await,
            &["builds-news"],
        );
        let absent = masked(
            &call(bob.clone(), method.clone(), request("nobody-news")).await,
            &["nobody-news"],
        );
        if hidden != absent {
            found.push(format!(
                "{method} tells the two ids apart: {} | {}",
                brief(&hidden),
                brief(&absent)
            ));
        }
        let after = served(&app, &root, "subscriptions/get", read("builds-news")).await;
        if after != before {
            found.push(format!(
                "{method} changed the template: {}",
                brief(&after.to_string())
            ));
        }
    }

    // A template of the mob the caller may address: read and changed by the
    // caller, and it names that mob alone.
    let names_the_second_mob =
        |answer: &Value| answer.to_string().contains(&format!("\"{SECOND}\""));
    let whole = served(&app, &root, "subscriptions/get", read("crew-news")).await;
    assert!(
        whole["template"]["mobs"]
            .as_array()
            .is_some_and(|mobs| mobs.contains(&json!(MOB)) && mobs.contains(&json!(SECOND))),
        "an administrator reads every mob: {whole:#}"
    );
    let shown = served(&app, &bob, "subscriptions/get", read("crew-news")).await;
    if shown["template"]["mobs"] != json!([MOB]) || names_the_second_mob(&shown) {
        found.push(format!("subscriptions/get: {}", brief(&shown.to_string())));
    }
    let paused = served(
        &app,
        &bob,
        "subscriptions/pause",
        json!({
            "operation_id": "pause-crew",
            "subscription_id": "crew-news",
            "expected_revision": shown["template"]["revision"],
        }),
    )
    .await;
    if paused["template"]["mobs"] != json!([MOB]) || names_the_second_mob(&paused) {
        found.push(format!(
            "subscriptions/pause: {}",
            brief(&paused.to_string())
        ));
    }

    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// An escalation about a source of a mob its caller may not address is not
/// listed to that caller, open or as an event; an escalation about a source
/// of a mob the caller may address is, and the events are read on from the
/// same place. An administrator is listed both.
#[tokio::test]
async fn an_escalation_about_a_source_of_a_mob_its_caller_may_not_address_is_not_listed()
-> TestResult {
    const SECOND: &str = SurfaceOwner::HOSTED_MOB;
    const BUILDER: &str = "builder";
    let (_state, runtime) = runtime("announcement-audience-source-mobs").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    owner.seat_in_hosted_mob(BUILDER);
    let state_of_console =
        enforced_console(&runtime, Some(everyone_addresses_the_owners_mob_alone()));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));

    // A source in each mob, neither ever collected, each stale after a
    // minute without an advance.
    let soon_stale = json!({ "stale_after_ms": 60_000 });
    let mut of_the_owners_mob = source("feed", &[SHARED]);
    of_the_owners_mob["health_policy"] = soon_stale.clone();
    served(&app, &root, "sources/put", of_the_owners_mob).await;
    let mut of_the_second_mob = source("feed-2", &[]);
    of_the_second_mob["mob_id"] = json!(SECOND);
    of_the_second_mob["audience"] = json!([{
        "kind": "member",
        "member": { "mob_id": SECOND, "identity": BUILDER },
    }]);
    of_the_second_mob["health_policy"] = soon_stale;
    served(&app, &root, "sources/put", of_the_second_mob).await;
    owner.round_at(10_000_000).await;
    owner.round_at(10_000_000 + 61 * 60_000).await;

    let subject_of =
        |mob: &str, id: &str| json!({ "kind": "source", "mob_id": mob, "source_id": id });
    let subjects = |answer: &Value, key: &str| -> Vec<Value> {
        answer[key]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| row["subject"].clone())
            .collect()
    };
    let mut found = Vec::new();
    for (params, key) in [
        (json!({ "open": true }), "escalations"),
        (json!({}), "events"),
    ] {
        let every = served(&app, &root, "escalations/list", params.clone()).await;
        let all = subjects(&every, key);
        assert!(
            all.contains(&subject_of(SECOND, "feed-2")) && all.contains(&subject_of(MOB, "feed")),
            "{params}: an administrator is listed both sources' escalations: {every:#}"
        );
        let shown = served(&app, &bob, "escalations/list", params.clone()).await;
        let seen = subjects(&shown, key);
        if seen.contains(&subject_of(SECOND, "feed-2")) || shown.to_string().contains("feed-2") {
            found.push(format!(
                "{params}: the caller is listed {}",
                brief(&shown.to_string())
            ));
        }
        if !seen.contains(&subject_of(MOB, "feed")) {
            found.push(format!(
                "{params}: the caller lost the source of the mob it may address"
            ));
        }
        if shown["next"] != every["next"] {
            found.push(format!(
                "{params}: the caller reads on from {}, an administrator from {}",
                shown["next"], every["next"]
            ));
        }
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// An announcement a caller is shown names no cause that is an announcement
/// of a mob the caller may not address: read by its key, and in the answer
/// that its key is taken. An administrator reads every cause.
#[tokio::test]
async fn a_shown_announcement_names_no_cause_in_a_mob_its_caller_may_not_address() -> TestResult {
    const SECOND: &str = SurfaceOwner::HOSTED_MOB;
    let (_state, runtime) = runtime("announcement-audience-cause-mobs").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    let state_of_console =
        enforced_console(&runtime, Some(everyone_addresses_the_owners_mob_alone()));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));

    let mut in_the_second_mob = news("k-0");
    in_the_second_mob["mob_id"] = json!(SECOND);
    served(&app, &root, "announce", in_the_second_mob).await;
    served(&app, &root, "announce", news("k-1")).await;
    let mut caused = news("k-2");
    caused["causes"] = json!([
        { "mob_id": SECOND, "source_key": "k-0" },
        { "mob_id": MOB, "source_key": "k-1" },
    ]);
    served(&app, &root, "announce", caused).await;

    let read = json!({ "mob_id": MOB, "source_key": "k-2" });
    let of_the_owners_mob = json!([{ "mob_id": MOB, "source_key": "k-1" }]);
    let whole = served(&app, &root, "get", read.clone()).await;
    assert_eq!(
        whole["announcement"]["causes"].as_array().map(Vec::len),
        Some(2),
        "an administrator reads both causes: {whole:#}"
    );
    let mut found = Vec::new();
    let shown = served(&app, &bob, "get", read).await;
    if shown["announcement"]["causes"] != of_the_owners_mob {
        found.push(format!("get: {}", brief(&shown.to_string())));
    }
    let mut other = news("k-2");
    other["body"] = json!("something else");
    let taken = console_rpc_as(&app, Some(&bob), "meta/mob_announcements/announce", other).await;
    assert!(
        taken["error"]["data"]["existing"].is_object(),
        "the key is answered as taken, with the announcement: {taken:#}"
    );
    if taken["error"]["data"]["existing"]["causes"] != of_the_owners_mob {
        found.push(format!("announce: {}", brief(&taken.to_string())));
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// A member of a mob held here that no member created is judged as a member
/// of that mob, not as the agent of the console's own mob that holds its
/// name. Where it is private to another person in its own mob, a caller who
/// may address that mob is listed none of its records, and each method that
/// names it, or a record of it, answers as for a name or an id nobody
/// holds, though every caller may see the member of that name in the
/// console's own mob. A member of such a mob that the console holds no
/// attributes of is withheld the same way from a caller the console keeps
/// apart from other people. Nothing such a caller asks changes a record of
/// either, and an admin is listed every record.
#[tokio::test]
async fn a_member_of_a_mob_no_member_created_is_judged_in_its_own_mob() -> TestResult {
    const SERVED: &str = SurfaceOwner::SERVED_MOB;
    const GHOST: &str = "ghost";
    let (_state, runtime) = runtime("announcement-audience-served-mob").await?;
    seeded_member(&runtime, "worker", SHARED, &[]).await?;
    let owner = SurfaceOwner::serving(&[SHARED]);
    owner.seat_in_served_mob(SHARED);
    owner.seat_in_served_mob(GHOST);
    // Every signed-in caller may address every mob. In the served mob the
    // member under the shared member's name is private to another person;
    // of the other member there the console holds no attributes.
    let access = everyone_administers_the_runtime();
    access.record_agent_attributes_in(
        &meerkat_mob::MobId::from(SERVED),
        crate::access::AgentResourceAttributes {
            identity: SHARED.to_string(),
            agent_id: None,
            role: Some("worker".to_string()),
            labels: std::collections::BTreeMap::from([(
                crate::access::PRIVATE_TO_LABEL.to_string(),
                "person:dana".to_string(),
            )]),
        },
    );
    let state_of_console = enforced_console(&runtime, Some(access));
    assert!(
        state_of_console
            .mob_announcements
            .set(owner.service())
            .is_ok(),
        "the console takes the owner"
    );
    let app = console_json_router_with_state(state_of_console);
    let (bob, root) = (bearer(bob_claims()), bearer(root_claims()));
    let of_served = |identity: &str| json!({ "mob_id": SERVED, "identity": identity });

    // The admin files the records: one subscription of the member of the
    // owner's mob, one of each member of the served mob; then one
    // announcement all three capture.
    for (id, of) in [
        ("s-1", member(SHARED)),
        ("s-2", of_served(SHARED)),
        ("s-3", of_served(GHOST)),
    ] {
        let mut request = subscription(id, SHARED);
        request["member"] = of;
        let put = console_rpc_as(
            &app,
            Some(&root),
            "meta/mob_announcements/subscriptions/put",
            request,
        )
        .await;
        assert!(put["error"].is_null(), "{id}: {put:#}");
    }
    let announced = console_rpc_as(
        &app,
        Some(&root),
        "meta/mob_announcements/announce",
        news("k-1"),
    )
    .await;
    assert!(announced["error"].is_null(), "{announced:#}");
    owner.round_at(100_000).await;

    let in_served = |row: &Value| row["member"]["mob_id"] == json!(SERVED);
    let mut found = Vec::new();
    for (verb, key) in [
        ("subscriptions/list", "subscriptions"),
        ("deliveries/list", "deliveries"),
    ] {
        let every: Vec<Value> = paged(&app, &root, verb, key, 100)
            .await
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(
            every.iter().filter(|row| in_served(row)).count(),
            2,
            "{verb}: the admin is listed the records of both members of the served mob: {every:#?}"
        );
        let listed: Vec<Value> = paged(&app, &bob, verb, key, 100)
            .await
            .into_iter()
            .flatten()
            .collect();
        if listed.is_empty() || listed.iter().any(in_served) {
            found.push(format!(
                "{verb}: the caller is listed {}",
                brief(&json!(listed).to_string())
            ));
        }
    }

    // Each method that names such a member, or a record of it, answers as
    // for a name and an id nobody holds.
    let requests = |identity: &str, id: &str| {
        vec![
            ("members/get", json!({ "member": of_served(identity) })),
            (
                "subscriptions/list",
                json!({ "member": of_served(identity) }),
            ),
            ("deliveries/list", json!({ "member": of_served(identity) })),
            ("subscriptions/get", json!({ "subscription_id": id })),
            (
                "subscriptions/cancel",
                json!({
                    "operation_id": format!("cancel-{id}"),
                    "subscription_id": id,
                    "expected_revision": 1,
                }),
            ),
        ]
    };
    for (name, id) in [(SHARED, "s-2"), (GHOST, "s-3")] {
        for ((verb, hidden), (_, absent)) in requests(name, id)
            .into_iter()
            .zip(requests(NOBODY, "s-nobody"))
        {
            let method = format!("meta/mob_announcements/{verb}");
            let hidden = masked(
                &console_rpc_as(&app, Some(&bob), &method, hidden).await,
                &[id, name],
            );
            let absent = masked(
                &console_rpc_as(&app, Some(&bob), &method, absent).await,
                &["s-nobody", NOBODY],
            );
            if hidden != absent {
                found.push(format!(
                    "{verb} tells {name} of the served mob from a name nobody holds: {} | {}",
                    brief(&hidden),
                    brief(&absent)
                ));
            }
        }
        let read = served(
            &app,
            &root,
            "subscriptions/get",
            json!({ "subscription_id": id }),
        )
        .await;
        if read["subscription"]["state"] != json!("active") {
            found.push(format!("{id} was changed: {}", brief(&read.to_string())));
        }
    }
    assert!(found.is_empty(), "{found:#?}");
    Ok(())
}

/// A mob held here whose origin cannot be read leaves every other mob
/// judged as the records show it: a member of a mob below the console's own
/// mob is still judged by the names of the console's own mob, so its owner
/// files and reads its record, and another person is answered as for an id
/// nobody holds.
#[tokio::test]
async fn a_mob_whose_origin_does_not_read_leaves_the_other_mobs_judged_as_before() -> TestResult {
    let mut scenes = scenes("origin-unread").await?;
    for scene in &mut scenes {
        let (alice, bob) = (scene.alice.clone(), scene.bob.clone());
        scene.owner.seat_in_hosted_mob(PRIVATE);
        scene.owner.origin_does_not_read(SurfaceOwner::SERVED_MOB);
        let mut request = subscription("sub-hosted", PRIVATE);
        request["member"] = json!({ "mob_id": SurfaceOwner::HOSTED_MOB, "identity": PRIVATE });
        scene.expected(&alice, "subscriptions/put", request).await;
        let read = json!({ "subscription_id": "sub-hosted" });
        let owners = scene
            .expected(&alice, "subscriptions/get", read.clone())
            .await;
        scene.check(
            owners["subscription"]["member"]["identity"] == json!(PRIVATE),
            || format!("its owner reads it: {}", brief(&owners.to_string())),
        );
        let others = masked(
            &scene.call(&bob, "subscriptions/get", read).await,
            &["sub-hosted"],
        );
        let absent = masked(
            &scene
                .call(
                    &bob,
                    "subscriptions/get",
                    json!({ "subscription_id": "sub-nobody" }),
                )
                .await,
            &["sub-nobody"],
        );
        scene.check(others == absent, || {
            format!("another person reads {}", brief(&others))
        });
    }
    nothing_found(scenes);
    Ok(())
}
