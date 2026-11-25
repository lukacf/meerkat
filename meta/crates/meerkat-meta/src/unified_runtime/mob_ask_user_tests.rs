//! The question a member of a second mob asks a person, in a real turn.
//!
//! The fixture is the launch of the agent memory rows
//! ([`super::mob_memory_tests`]): a launch that composes an identity plane
//! for its launch mob and for a second mob of its host, with an agent of
//! ONE name, `desk`, in each. The row gives the launch what a gateway with
//! a Console gives it: durable decisions in a state directory and a Console
//! on which a person decides, so the launch mob's agent can ask a person.
//! The members' model calls the `ask_user` tool its member's build carries.

#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex, PoisonError};

use meerkat_core::ask_user::ASK_USER_TOOL;

use super::mob_memory_tests::{
    DESK, config_of_the_launch, desk, launch_with_memory, with_a_second_mob,
};
use crate::console_schedules::{ConsoleScheduleDoor, ConsoleScheduledSend};
use crate::durable_decisions::RuntimeDecisions;

/// What the row sends the launch mob's `desk` to make it ask.
const ASK_OF_THE_LAUNCH_DESK: &str = "ASK-LAUNCH-DESK: ask which day to release.";
/// What the row sends the second mob's `desk` to make it ask.
const ASK_OF_THE_SECOND_DESK: &str = "ASK-SECOND-DESK: ask which day to release.";
/// The file of the durable decisions under a state directory.
const DECISIONS_FILE: &str = "approval_decisions.sqlite3";

/// The id of the tool call the model makes for the ask `ask`.
fn call_id_of(ask: &str) -> String {
    format!(
        "call-{}",
        ask.split(':').next().unwrap_or_default().to_lowercase()
    )
}

/// A Console send path that sends nothing: installed, it says the launch
/// serves a Console on which a person decides.
struct APersonDecides;

#[async_trait::async_trait]
impl ConsoleScheduleDoor for APersonDecides {
    fn takes_a_persons_decision(&self) -> bool {
        true
    }

    fn refuses_every_caller(&self) -> bool {
        false
    }

    async fn send(&self, _send: &ConsoleScheduledSend, _delivery_key: &str) -> Result<(), String> {
        Ok(())
    }
}

/// A members' model that calls its member's `ask_user` tool once for the
/// newest ask its conversation holds ([`ASK_OF_THE_LAUNCH_DESK`],
/// [`ASK_OF_THE_SECOND_DESK`]) and answers every other request in text. It
/// keeps every request it was sent, rendered.
#[derive(Clone, Default)]
struct AsksAPerson {
    asked: Arc<Mutex<Vec<String>>>,
}

impl AsksAPerson {
    /// The first request whose messages hold every one of `needles`, once
    /// there is one, or none after thirty seconds.
    async fn request_with(&self, needles: &[&str]) -> Option<String> {
        for _ in 0..300 {
            let found = self
                .asked
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .find(|request| needles.iter().all(|needle| request.contains(needle)))
                .cloned();
            if found.is_some() {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        None
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for AsksAPerson {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        // The newest ask of the conversation, called for once: a request
        // after the call carries the call, by its id, after the ask.
        let rendered = serde_json::to_string(&request.messages).unwrap_or_default();
        let call = [ASK_OF_THE_LAUNCH_DESK, ASK_OF_THE_SECOND_DESK]
            .into_iter()
            .filter_map(|ask| rendered.rfind(ask).map(|at| (at, ask)))
            .max_by_key(|(at, _)| *at)
            .and_then(|(at, ask)| {
                let after = rendered.get(at..).unwrap_or_default();
                (!after.contains(&call_id_of(ask))).then(|| call_id_of(ask))
            });
        self.asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(rendered);
        let (lead, stop_reason) = match call {
            Some(id) => (
                meerkat_client::LlmEvent::ToolCallComplete {
                    id,
                    name: ASK_USER_TOOL.to_string(),
                    args: serde_json::json!({
                        "title": "Release plan",
                        "questions": [{
                            "id": "day",
                            "prompt": "Which day?",
                            "choices": [
                                {"id": "mon", "label": "Monday"},
                                {"id": "fri", "label": "Friday"},
                            ],
                            "recommended": "mon",
                        }],
                    }),
                    meta: None,
                },
                meerkat_core::StopReason::ToolUse,
            ),
            None => (
                meerkat_client::LlmEvent::TextDelta {
                    delta: "noted".to_string(),
                    meta: None,
                },
                meerkat_core::StopReason::EndTurn,
            ),
        };
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat_core::Provider::OpenAI,
            stop_reason,
        );
        Box::pin(futures::stream::iter(vec![Ok(lead), Ok(usage), Ok(done)]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

/// On a host of two mobs whose launch mob's agents can ask a person, an
/// agent of the second mob that calls `ask_user` in a turn is answered, in
/// that turn, that no person can be asked: an answer could not be routed
/// back to a member of that mob. Nothing is held for it, under its name or
/// any other, and no decision store is written. The launch mob's agent of
/// the same name, asked after it, is asked, and the one set held is its
/// own. Each turn is sent through its mob's own identity runtime, and the
/// tool each model calls is the one its member's build carries.
#[tokio::test]
async fn a_second_mobs_agent_that_asks_a_person_in_a_turn_is_answered_that_none_can_be_asked() {
    let temp = tempfile::tempdir().expect("tempdir");
    let model = AsksAPerson::default();
    let of_the_members = model.clone();
    let mut kept = with_a_second_mob(
        launch_with_memory(&temp, config_of_the_launch(), move |spec| {
            spec.with_options(crate::mob_handle_runtime::MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(of_the_members)),
            })
        })
        .await,
    )
    .await;
    // What a gateway with a Console gives its launch: a Console on which a
    // person decides, and durable decisions in its state directory. The
    // runtime fills its one question surface from them, for its launch mob.
    let state_dir = temp.path().join("decisions");
    std::fs::create_dir_all(&state_dir).expect("the state directory");
    kept.host
        .console_schedule_doors()
        .install(Arc::new(APersonDecides));
    let decisions = RuntimeDecisions::in_state_dir(
        &state_dir,
        meerkat_core::RealmId::parse("team").expect("realm"),
        crate::durable_decisions::DecisionIngressDeclarations::default(),
        Arc::new(
            crate::runtime::SqliteMetadataStore::open(state_dir.join("metadata.sqlite3"))
                .expect("a metadata store in the state directory"),
        ),
        kept.host.mob_handle().mob_id().to_string(),
        true,
    );
    kept.host.set_durable_decisions(decisions.clone());
    let text = |content: &str| meerkat_core::ContentInput::Text(content.to_string());
    let held = || {
        crate::ask_user::open_question_sets(&decisions)
            .expect("the open question sets read")
            .iter()
            .map(|set| set.asker().to_string())
            .collect::<Vec<_>>()
    };

    let second_context = kept.second_context().await;
    assert!(
        second_context.is_some(),
        "the second mob of the host has an identity plane"
    );
    if let Some(second_context) = second_context {
        let to_the_second_desk = second_context
            .runtime
            .send(&desk(), &text(ASK_OF_THE_SECOND_DESK))
            .await;
        assert!(
            to_the_second_desk.is_ok(),
            "the send to the second mob's agent: {:?}",
            to_the_second_desk.as_ref().err()
        );
    }
    let answered_the_second_desk = model
        .request_with(&[
            &call_id_of(ASK_OF_THE_SECOND_DESK),
            r#"\"outcome\":\"no_human_surface\""#,
        ])
        .await;
    assert!(
        answered_the_second_desk.is_some(),
        "the second mob's agent's call is answered, in its turn, that no person can be asked"
    );
    assert_eq!(
        held(),
        Vec::<String>::new(),
        "no question set is held for the second mob's agent, under any name"
    );
    assert!(
        !state_dir.join(DECISIONS_FILE).exists(),
        "the call of the second mob's agent writes no decision store"
    );

    let to_the_launch_desk = kept
        .launch_context
        .runtime
        .send(&desk(), &text(ASK_OF_THE_LAUNCH_DESK))
        .await;
    assert!(
        to_the_launch_desk.is_ok(),
        "the send to the launch mob's agent: {:?}",
        to_the_launch_desk.as_ref().err()
    );
    let answered_the_launch_desk = model
        .request_with(&[
            &call_id_of(ASK_OF_THE_LAUNCH_DESK),
            r#"\"outcome\":\"asked\""#,
        ])
        .await;
    assert!(
        answered_the_launch_desk.is_some(),
        "the launch mob's agent of the same name is asked: its launch has a person to ask"
    );
    assert_eq!(
        held(),
        vec![DESK.to_string()],
        "one set is held: the launch mob's agent's"
    );
    kept.shut_down().await;
}
