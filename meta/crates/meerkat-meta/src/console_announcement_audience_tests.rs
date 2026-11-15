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
/// everything to every agent and administer the runtime, with `root` its
/// one admin.
fn everyone_administers_the_runtime() -> AccessController {
    AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec![ROOT.to_string()],
        rules: vec![AccessRule {
            id: "everyone-administers-the-runtime".to_string(),
            subjects: vec!["*".to_string()],
            actions: vec![
                "agent.*".to_string(),
                crate::access::ACTION_RUNTIME_ADMIN.to_string(),
            ],
            ..AccessRule::default()
        }],
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
