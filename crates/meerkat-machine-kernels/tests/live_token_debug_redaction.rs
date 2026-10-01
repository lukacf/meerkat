//! Generated MeerkatMachine kernel types never print live bootstrap tokens.
//!
//! The token fields are `#[redacted]` in the machine DSL, so the generated
//! state, input and effect structs carry a hand-written `Debug`.

use meerkat_machine_kernels::generated::meerkat::{
    Input, effects::LiveWebsocketTokenIssued, initial_state, inputs::RecordLiveWebrtcTokenIssued,
};

const SECRET: &str = "live-bootstrap-secret";

#[test]
fn generated_live_token_carriers_debug_redact_the_token() {
    let mut state = initial_state();
    state
        .live_webrtc_token_channel_by_token
        .insert(SECRET.to_owned(), "channel-visible".to_owned());
    state
        .live_websocket_consumed_tokens
        .insert(SECRET.to_owned());

    let input = Input::RecordLiveWebrtcTokenIssued(RecordLiveWebrtcTokenIssued {
        session_id: "session-visible".to_owned(),
        channel_id: "channel-visible".to_owned(),
        token: SECRET.to_owned(),
        issued_at_ms: 1,
        ttl_ms: 2,
    });
    let effect = LiveWebsocketTokenIssued {
        session_id: "session-visible".to_owned(),
        channel_id: "channel-visible".to_owned(),
        token: SECRET.to_owned(),
        expires_at_ms: 3,
        sequence: 4,
    };

    let rendered = format!("{state:?} {input:?} {input:#?} {effect:?} {effect:#?}");
    assert!(!rendered.contains(SECRET), "token leaked into Debug output");
    for kept in [
        "live_webrtc_token_channel_by_token: <redacted; 1 entries>",
        "live_websocket_consumed_tokens: <redacted; 1 entries>",
        "session-visible",
        "channel-visible",
        "token: \"<redacted>\"",
    ] {
        assert!(rendered.contains(kept), "missing {kept}");
    }
}
