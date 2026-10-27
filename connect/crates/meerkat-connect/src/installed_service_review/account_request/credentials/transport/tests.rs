use super::*;
use crate::stdio::PromptBridge;

fn slots() -> Vec<ServiceCredentialSlot> {
    ["FIXTURE_API_KEY", "FIXTURE_TOKEN"].into_iter()
        .map(|name| ServiceCredentialSlot::SelectedEnvironment(name.into())).collect()
}
fn prompt() -> Value {
    json!({"kind":"approval","approval_id":"actual-retained-prompt",
        "allowed_decisions":["approve","deny"],"body":{"scope":"component transport control"}})
}
fn answer(decision: &str, credentials: Value) -> Value {
    json!({"review_id":7,"approval_id":"actual-retained-prompt",
        "decision":decision,"credentials":credentials})
}

#[tokio::test]
async fn protected_credentials_bind_exact_exchange_slots_and_one_shot_without_prompt_values() {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let sentinel = "component-private-value-do-not-project";
        let (delivery, mut values) = CredentialDelivery::channel(slots()).unwrap();
        let projection = delivery.projection();
        assert_eq!(projection["slots"].as_array().unwrap().len(), 2);
        assert!(!projection.to_string().contains(sentinel));
        let (bridge, mut prompts, _retire) = PromptBridge::channel();
        let expected = prompt();
        let ask = bridge.ask(expected.clone());
        let offer = async {
            let mut delivery = Some(delivery);
            let mut exchange = Some(prompts.recv().await.unwrap());
            assert_eq!(exchange.as_ref().unwrap().prompt, expected);
            let valid = answer("approve", json!([
                {"slot":"FIXTURE_API_KEY","value":sentinel},
                {"slot":"FIXTURE_TOKEN","value":"another-private-value"}
            ]));
            let mut wrong_review = valid.clone(); wrong_review["review_id"] = json!(8);
            let mut wrong_prompt = valid.clone(); wrong_prompt["approval_id"] = json!("other");
            let mut extra = valid.clone(); extra["principal"] = json!("other-human");
            let mut wrong_slot = valid.clone(); wrong_slot["credentials"][0]["slot"] = json!("OTHER");
            let mut duplicate_slot = valid.clone(); duplicate_slot["credentials"][1]["slot"] = json!("FIXTURE_API_KEY");
            let mut missing = valid.clone(); missing["credentials"].as_array_mut().unwrap().pop();
            let mut denial_with_value = valid.clone(); denial_with_value["decision"] = json!("deny");
            let mut deliveries = 0;
            for rejected in [wrong_review, wrong_prompt, extra, wrong_slot, duplicate_slot, missing, denial_with_value] {
                let error = answer_retained_credentials(7, &mut delivery, &mut exchange,
                    rejected.to_string().as_bytes(), || { deliveries += 1; Ok(()) }).unwrap_err();
                assert!(!format!("{error:?}").contains(sentinel));
                assert!(delivery.is_some() && exchange.is_some());
                assert!(matches!(values.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
            }
            assert_eq!(deliveries, 0);
            // The real HTTP owner's final current-selection check can refuse
            // without consuming either carrier. This is not a synthetic Human.
            assert!(answer_retained_credentials(7, &mut delivery, &mut exchange,
                valid.to_string().as_bytes(), || Err(invalid())).is_err());
            assert!(delivery.is_some() && exchange.is_some());
            assert!(matches!(values.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
            answer_retained_credentials(7, &mut delivery, &mut exchange,
                valid.to_string().as_bytes(), || { deliveries += 1; Ok(()) }).unwrap();
            assert_eq!(deliveries, 1);
            assert!(delivery.is_none() && exchange.is_none());
            assert!(answer_retained_credentials(7, &mut delivery, &mut exchange,
                valid.to_string().as_bytes(), || Ok(())).is_err());
        };
        let (decision, ()) = tokio::join!(ask, offer);
        assert!(matches!(decision.unwrap(), PromptAnswer::Approval { decision: Decision::Approve }));
        let delivered = values.await.unwrap();
        assert_eq!(delivered.values.len(), 2);
        assert_eq!(delivered.values["FIXTURE_API_KEY"].expose(), sentinel);
        assert_eq!(delivered.values["FIXTURE_TOKEN"].expose(), "another-private-value");
        // Only the decision crosses PromptAnswer. No approval owner, native
        // mutation or success is fabricated by this transport component test.
    }).await.expect("protected carrier did not settle");
}

#[tokio::test]
async fn protected_credentials_denial_and_closed_private_receiver_never_deliver_values() {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        for closed in [false, true] {
            let (delivery, values) = CredentialDelivery::channel(slots()).unwrap();
            let mut values = Some(values);
            if closed { drop(values.take()); }
            let (bridge, mut prompts, retire) = PromptBridge::channel();
            let ask = bridge.ask(prompt());
            let offer = async {
                let mut delivery = Some(delivery);
                let mut exchange = Some(prompts.recv().await.unwrap());
                let result = answer_retained_credentials(7, &mut delivery, &mut exchange,
                    answer("deny", json!([])).to_string().as_bytes(), || Ok(()));
                if closed {
                    assert!(result.is_err());
                    assert!(delivery.is_some() && exchange.is_some());
                    retire.send_replace(true);
                } else {
                    result.unwrap();
                    assert!(delivery.is_none() && exchange.is_none());
                }
            };
            let (decision, ()) = tokio::join!(ask, offer);
            if closed { assert!(decision.is_err()); }
            else {
                assert!(matches!(decision.unwrap(), PromptAnswer::Approval { decision: Decision::Deny }));
                assert!(values.unwrap().await.unwrap().values.is_empty());
            }
        }
    }).await.expect("protected carrier did not settle");
}
