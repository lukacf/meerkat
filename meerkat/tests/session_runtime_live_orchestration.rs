//! Smoke test for `meerkat::session_runtime::live_orchestration`.
//!
//! Covers the surface-agnostic helpers moved out of
//! `meerkat-rpc::session_runtime`:
//!
//! * `apply_precheck_gates` — B19 (realtime capability) fires before B18
//!   (provider has live adapter); a non-realtime non-OpenAI session
//!   reports `ModelNotRealtime` (the more specific failure), not
//!   `ProviderHasNoLiveAdapter`. This pins the gate ordering as
//!   intentional contract — the catalog cannot naturally produce a
//!   realtime-capable non-OpenAI row, so the synthetic
//!   `realtime_capable: true` argument is the only way to exercise the
//!   B18 branch.
//! * `live_channel_requires_close_for_identity_change` — closes on
//!   model swap, closes on provider swap, in-place refresh otherwise.
//! * `extract_system_prompt_from_seed_messages_runtime` — surfaces the
//!   first `System`/`SystemNotice` lead.
//!
//! Coverage of the load-bearing methods (`precheck_live_open`,
//! `materialize_staged_session_for_realtime_open`,
//! `propagate_config_to_live_channels`) lives in `meerkat-rpc`'s
//! integration tests today; once W3-B promotes the RPC accessors
//! upstream, those methods land on `LiveOrchestrator<'a>` and gain
//! direct coverage here.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat::session_runtime::errors::LiveOpenPrecheckError;
use meerkat::session_runtime::live_orchestration::{
    apply_precheck_gates, extract_system_prompt_from_seed_messages_runtime,
    live_channel_requires_close_for_identity_change,
};
use meerkat_core::types::{Message, SystemMessage, SystemNoticeKind, SystemNoticeMessage};
use meerkat_core::{Provider, SessionLlmIdentity};

#[test]
fn precheck_b19_fires_before_b18_for_non_realtime_non_openai() {
    let err = apply_precheck_gates(Provider::Anthropic, "claude-opus-4-7", false)
        .expect_err("non-realtime should fail B19");
    match err {
        LiveOpenPrecheckError::ModelNotRealtime { model, provider } => {
            assert_eq!(model, "claude-opus-4-7");
            assert_eq!(provider, "anthropic");
        }
        other => panic!("expected ModelNotRealtime, got {other:?}"),
    }
}

#[test]
fn precheck_b18_rejects_realtime_capable_non_openai() {
    let err = apply_precheck_gates(Provider::Anthropic, "synthetic-rt-anthropic", true)
        .expect_err("realtime-capable non-OpenAI should fail B18");
    match err {
        LiveOpenPrecheckError::ProviderHasNoLiveAdapter { provider } => {
            assert_eq!(provider, "anthropic");
        }
        other => panic!("expected ProviderHasNoLiveAdapter, got {other:?}"),
    }
}

#[test]
fn precheck_accepts_realtime_capable_openai() {
    apply_precheck_gates(Provider::OpenAI, "gpt-realtime-2", true)
        .expect("realtime-capable OpenAI must pass both gates");
}

#[test]
fn live_channel_close_on_model_swap() {
    let prev = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let next = SessionLlmIdentity {
        model: "gpt-realtime-3".into(),
        ..prev.clone()
    };
    assert!(live_channel_requires_close_for_identity_change(
        Some(&prev),
        &next
    ));
}

#[test]
fn live_channel_close_on_provider_swap() {
    let prev = SessionLlmIdentity {
        model: "shared".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    let next = SessionLlmIdentity {
        provider: Provider::Anthropic,
        ..prev.clone()
    };
    assert!(live_channel_requires_close_for_identity_change(
        Some(&prev),
        &next
    ));
}

#[test]
fn live_channel_in_place_refresh_when_identity_unchanged() {
    let identity = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    assert!(!live_channel_requires_close_for_identity_change(
        Some(&identity),
        &identity
    ));
}

#[test]
fn live_channel_no_close_when_no_bound_identity() {
    let next = SessionLlmIdentity {
        model: "gpt-realtime-2".into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    assert!(!live_channel_requires_close_for_identity_change(
        None, &next
    ));
}

#[test]
fn extract_system_prompt_returns_system_message_content() {
    let msgs = vec![
        Message::System(SystemMessage::new("you are helpful")),
        Message::User(meerkat_core::types::UserMessage::text("hi")),
    ];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        Some("you are helpful".to_string())
    );
}

#[test]
fn extract_system_prompt_returns_rendered_notice_text() {
    let notice = SystemNoticeMessage::new(SystemNoticeKind::McpPending, "MCP servers connecting");
    let rendered = notice.rendered_text();
    let msgs = vec![Message::SystemNotice(notice)];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        Some(rendered)
    );
}

#[test]
fn extract_system_prompt_none_when_first_is_user() {
    let msgs = vec![Message::User(meerkat_core::types::UserMessage::text("hi"))];
    assert_eq!(
        extract_system_prompt_from_seed_messages_runtime(&msgs),
        None
    );
}
