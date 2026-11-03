//! Mechanical stock-input selection plus the actual governed native binder.
//! verifier_tests covers the actual configured Console HTTP route, including
//! its duplicate-send contract and persisted source custody.
use super::*;
use crate::private_originals::ContinuationFixture;
use futures::FutureExt;
use meerkat_runtime::PromptInput;
use std::{panic::AssertUnwindSafe, time::Duration};

const STOCK_KEY: &str =
    "console:00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002";
const WAIT: Duration = Duration::from_secs(20);

#[test]
fn stock_console_key_is_required_and_preserved_without_input_mutation() {
    let mut prompt = PromptInput::new("retained Console content", None);
    let input = Input::Prompt(prompt.clone());
    assert!(stock_console_event_key(&input).is_err());
    prompt.header.idempotency_key = Some(IdempotencyKey::new(STOCK_KEY));
    let input = Input::Prompt(prompt);
    let before = canonical_json(&input).unwrap();
    assert_eq!(
        stock_console_event_key(&input).unwrap(),
        IdempotencyKey::new(STOCK_KEY)
    );
    assert_eq!(canonical_json(&input).unwrap(), before);
}

#[path = "verifier_tests.rs"]
mod verifier_tests;
