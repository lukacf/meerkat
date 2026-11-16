//! Rows of the keyed inject plane for a durable identity that is kept apart
//! from other callers.
//!
//! A peer gateway's keyed delivery names a durable identity this gateway
//! hosts. An identity that is kept apart is answered exactly as a name no
//! identity has: its delivery is refused as for an unknown member, and its
//! status reads as no commit. Nothing reaches its continuation owner.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::remote::KeyedInjectServer;
use super::types::content_digest;
use crate::runtime::keyed_inject::{
    KeyedInject, KeyedInjectCallerScope, KeyedInjectReceiver, NativeStatus,
};

/// An identity every caller may reach.
const OPEN: &str = "agent:lobby";
/// An identity that is kept apart.
const APART: &str = "agent:vault";
/// A name no identity has.
const NOBODY: &str = "agent:nobody";

const NOW_MS: u64 = 1_800_000_000_000;
const WINDOW_MS: u64 = 300_000;

/// A host's roster of two identities: [`OPEN`], and [`APART`] declared with
/// the label that keeps an identity apart.
struct TwoIdentities;

#[async_trait]
impl crate::identity_first::contracts::RosterProvider for TwoIdentities {
    async fn roster(
        &self,
        _context: &crate::identity_first::RosterContext,
    ) -> Result<Vec<crate::identity_first::DurableAgentSpec>, crate::identity_first::RosterError>
    {
        let agent = |name: &str, labels: BTreeMap<String, String>| {
            crate::identity_first::DurableAgentSpec {
                identity: crate::identity_first::AgentIdentity::parse(name)
                    .expect("parse identity"),
                profile: meerkat_mob::ProfileName::from("personal"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels,
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            }
        };
        Ok(vec![
            agent(OPEN, BTreeMap::new()),
            agent(
                APART,
                BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "someone".to_string(),
                )]),
            ),
        ])
    }
}

/// The keyed inject server of an identity-first gateway whose roster is
/// [`TwoIdentities`], over the gateway's own delivery seam, with the
/// runtime that owns it.
async fn server(
    state: &std::path::Path,
    mob_id: &str,
) -> (crate::UnifiedRuntime, KeyedInjectServer) {
    let runtime = Box::pin(
        super::tests::identity_first_test_builder(state, mob_id, "")
            .await
            .roster_provider(Arc::new(TwoIdentities))
            .build(),
    )
    .await
    .expect("build the identity-first runtime");
    let local = Arc::new(super::local_delivery(&runtime).expect("the gateway's native seam"));
    let server = KeyedInjectServer::with_clock(local, WINDOW_MS, Arc::new(|| NOW_MS));
    (runtime, server)
}

fn delivery<'a>(member: &'a str, digest: &'a str) -> KeyedInject<'a> {
    KeyedInject {
        member,
        delivery_key: "batch-1",
        content_digest: digest,
        summary: "1 announcement",
        body: "a body",
    }
}

/// The compared pair for the two keyed verbs: a delivery to the identity
/// kept apart is refused exactly as a delivery to a name no identity has,
/// and its status reads exactly as that name's. An identity every caller
/// may reach is not refused as unknown.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_kept_apart_answers_the_keyed_verbs_as_a_name_no_identity_has() {
    let state = tempfile::tempdir().expect("state dir");
    let (runtime, server) = server(state.path(), "mob-k-apart").await;
    let scope = KeyedInjectCallerScope::for_grant_label("peer-gateway");
    let digest = content_digest("a body");
    let deadline = NOW_MS + WINDOW_MS;

    let to_the_identity = server
        .deliver(&scope, delivery(APART, &digest), deadline)
        .await;
    let to_no_identity = server
        .deliver(&scope, delivery(NOBODY, &digest), deadline)
        .await;
    let refusal_of_no_identity = format!("{:?}", to_no_identity.as_ref().err());
    assert!(
        to_no_identity.is_err(),
        "a delivery to a name no identity has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        format!("{:?}", to_the_identity.as_ref().err()).replace(APART, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "a delivery to the identity kept apart is refused as one to a name no identity has"
    );

    let of_the_identity = server
        .status(&scope, APART, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    let of_no_identity = server
        .status(&scope, NOBODY, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    assert_eq!(
        format!("{of_the_identity:?}").replace(APART, "<identity>"),
        format!("{of_no_identity:?}").replace(NOBODY, "<identity>"),
        "the status of the identity kept apart reads as that of a name no identity has"
    );
    assert!(
        matches!(of_no_identity, Ok((NativeStatus::NotCommitted, None))),
        "a name no identity has holds no commit: {of_no_identity:?}"
    );

    // An identity every caller may reach is not answered as unknown.
    let to_the_open_identity = server
        .deliver(&scope, delivery(OPEN, &digest), deadline)
        .await;
    assert_ne!(
        format!("{:?}", to_the_open_identity.as_ref().err()).replace(OPEN, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "an identity every caller may reach is not refused as unknown: {to_the_open_identity:?}"
    );
    runtime.shutdown().await;
}
