//! Regression for meerkat 0.7.2 #2: a session persisted in the legacy 0.6
//! durable shape must deserialize/resume through Meta's session path
//! instead of failing closed (which would force a fresh-spawn and silently
//! drop the member's transcript + durable identity).
//!
//! The legacy 0.6 shape carries three wire forms that 0.7 renamed:
//!   * `Provider::OpenAI` serialized as `"open_a_i"` (the snake_case mangling
//!     of `OpenAI`); 0.7 pins the canonical `"openai"` with a read alias.
//!   * `AssistantBlock::ServerToolContent` stored the provider tool name under
//!     `name`; 0.7 renamed it to the typed `kind` with a `name` alias +
//!     legacy-string compat deserializer.
//!   * `comms_name` held the raw member alias (`mob/role/member`) and mob
//!     ownership was recovered by string-split; 0.7 adds the typed
//!     `mob_member_binding` which legacy rows lack and so must default to
//!     `None` without erroring.
//!
//! Meta's resume path reads the persisted row payload back into a
//! `meerkat_core::Session` (see `PersistentSessionService::load_runtime_session_snapshot`,
//! `serde_json::from_slice::<Session>`). If that deserialize fails the member
//! cannot resume. This test persists a legacy-shaped payload through Meta's
//! own `JsonFileSessionStore` and asserts the round-trip Session loads with
//! the migrated fields intact.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]

use meerkat_core::types::{AssistantBlock, ServerToolKind, StopReason, Usage};
use meerkat_core::{Provider, Session, SessionMetadata, SessionTooling};
use meerkat_meta::{JsonFileSessionStore, SessionPersistenceRow};
use serde_json::{Value, json};
use tempfile::tempdir;

const RAW_ALIAS_COMMS_NAME: &str = "ops/orchestrator/ops-lead";

/// Build a current (0.7) session containing a `ServerToolContent` block and
/// OpenAI session metadata with a raw-alias `comms_name`, then serialize it to
/// JSON so we inherit the correct envelope version + timestamp/usage shapes
/// (rather than hand-guessing the persistence-version byte).
fn current_session_json() -> Value {
    let mut session = Session::new();

    let metadata = SessionMetadata {
        schema_version: meerkat_core::session_metadata_schema_version(),
        model: "gpt-4o".to_string(),
        max_tokens: 4096,
        structured_output_retries: meerkat_core::config::default_structured_output_retries(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: SessionTooling::default(),
        keep_alive: false,
        comms_name: Some(RAW_ALIAS_COMMS_NAME.to_string()),
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        model_fallback: None,
        // 0.7 typed ownership fact: legacy rows do NOT carry this.
        mob_member_binding: None,
    };
    session
        .set_session_metadata(metadata)
        .expect("set session metadata");

    session.append_external_assistant_blocks(
        vec![
            AssistantBlock::Text {
                text: "searching".to_string(),
                meta: None,
            },
            AssistantBlock::ServerToolContent {
                id: Some("call_1".to_string()),
                kind: ServerToolKind::WebSearch,
                content: json!({ "query": "meerkat 0.7.2" }),
                meta: None,
            },
        ],
        StopReason::EndTurn,
        // 0.8.22 requires declared provider accounting on the append seam.
        // This fixture pins the LEGACY on-disk shape, so the identity must
        // match the meta it asserts at the bottom of the file (OpenAI/gpt-4o),
        // not a current-catalog model.
        meerkat_core::TurnUsage::host_declared(Provider::OpenAI, "gpt-4o", Usage::default()),
    );

    serde_json::to_value(&session).expect("serialize current session")
}

/// Rewrite a current-shape session JSON into the legacy 0.6 durable shape:
/// `"openai"` -> `"open_a_i"` on the persisted provider, and the typed
/// `ServerToolContent.kind` -> legacy `name` string. The raw-alias
/// `comms_name` and the *absent* `mob_member_binding` are already legacy.
fn downgrade_to_legacy_shape(mut value: Value) -> Value {
    // Provider lives inside the session metadata map under the reserved key.
    let metadata_key = meerkat_core::session::SESSION_METADATA_KEY;
    let meta = value
        .get_mut("metadata")
        .and_then(Value::as_object_mut)
        .expect("session metadata map");
    let session_meta = meta
        .get_mut(metadata_key)
        .and_then(Value::as_object_mut)
        .expect("session_metadata object");
    assert_eq!(
        session_meta.get("provider"),
        Some(&json!("openai")),
        "current shape serializes the canonical provider name"
    );
    session_meta.insert("provider".to_string(), json!("open_a_i"));
    assert!(
        !session_meta.contains_key("mob_member_binding"),
        "legacy rows must not carry the typed mob_member_binding"
    );

    // Rewrite the ServerToolContent block: drop the typed `kind`, restore the
    // legacy `name` provider-string field.
    let messages = value
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .expect("messages array");
    let mut rewrote = false;
    for message in messages {
        // `Message::BlockAssistant` serializes internally-tagged
        // (`role = "block_assistant"`) with the `blocks` field flattened onto
        // the message object.
        let Some(blocks) = message.get_mut("blocks").and_then(Value::as_array_mut) else {
            continue;
        };
        for block in blocks {
            let is_stc = block.get("block_type") == Some(&json!("server_tool_content"));
            if !is_stc {
                continue;
            }
            let data = block
                .get_mut("data")
                .and_then(Value::as_object_mut)
                .expect("server_tool_content data");
            assert!(
                data.contains_key("kind"),
                "current ServerToolContent serializes a typed `kind`: {data:?}"
            );
            data.remove("kind");
            data.insert("name".to_string(), json!("web_search"));
            rewrote = true;
        }
    }
    assert!(rewrote, "expected to rewrite a server_tool_content block");

    value
}

#[test]
fn legacy_06_session_resumes_through_meta_session_store_without_fresh_spawn() {
    let legacy_payload = downgrade_to_legacy_shape(current_session_json());

    // Persist the legacy-shaped payload through Meta's own session store,
    // exactly as a 0.6-era process would have written it.
    let temp = tempdir().expect("tempdir");
    let sessions_path = temp.path().join("sessions.json");
    let store = JsonFileSessionStore::new(&sessions_path);
    store
        .append_rows(&[SessionPersistenceRow {
            session_id: "legacy-session".to_string(),
            updated_at_ms: 1,
            deleted: false,
            payload: legacy_payload,
            ..Default::default()
        }])
        .expect("persist legacy row");

    let rows = store.read_rows().expect("read legacy rows");
    assert_eq!(rows.len(), 1, "exactly one persisted row");
    let payload = &rows[0].payload;

    // The resume signal: Meta's resume path deserializes the row payload
    // into a `meerkat_core::Session`. On 0.7.1 the legacy provider/tool wire
    // forms would fail this deserialize, forcing the runtime to fresh-spawn.
    let resumed: Session = serde_json::from_value(payload.clone())
        .expect("legacy 0.6 session must deserialize on 0.7.2");

    // Resume (not fresh-spawn) means the durable transcript survived.
    assert!(
        !resumed.messages().is_empty(),
        "resumed session must carry the persisted transcript, not start empty"
    );

    // Provider alias migrated: `"open_a_i"` -> OpenAI.
    let meta = resumed
        .session_metadata()
        .expect("session metadata must restore through generated authority");
    assert_eq!(meta.provider, Provider::OpenAI);
    assert_eq!(meta.model, "gpt-4o");

    // comms_name raw alias preserved as the transport routing name.
    assert_eq!(meta.comms_name.as_deref(), Some(RAW_ALIAS_COMMS_NAME));
    // Legacy rows lack the typed binding; it must read as None, not error.
    assert!(
        meta.mob_member_binding.is_none(),
        "legacy comms_name-only row must read as no typed mob_member_binding"
    );

    // Legacy ServerToolContent `{ name: "web_search" }` migrated to typed kind.
    let server_tool_kind = resumed
        .messages()
        .iter()
        .find_map(|message| {
            serde_json::to_value(message)
                .ok()
                .and_then(|v| find_server_tool_kind(&v))
        })
        .expect("resumed transcript must retain the server tool block");
    assert_eq!(server_tool_kind, ServerToolKind::WebSearch);
}

/// Locate a `server_tool_content` block in a serialized message and decode its
/// (now-typed) kind, proving the legacy `name` form round-tripped.
fn find_server_tool_kind(message: &Value) -> Option<ServerToolKind> {
    let blocks = message.get("blocks").and_then(Value::as_array)?;
    for block in blocks {
        if block.get("block_type") == Some(&json!("server_tool_content")) {
            let parsed: AssistantBlock =
                serde_json::from_value(block.clone()).expect("block re-parses");
            if let AssistantBlock::ServerToolContent { kind, .. } = parsed {
                return Some(kind);
            }
        }
    }
    None
}
