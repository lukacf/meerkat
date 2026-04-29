use meerkat::contracts::{
    RealtimeCapabilities, RealtimeChannelClosedFrame, RealtimeChannelOpenFrame,
    RealtimeChannelOpenedFrame, RealtimeChannelState, RealtimeChannelStatus, RealtimeClientFrame,
    RealtimeEvent, RealtimeInputChunk, RealtimeInputKind, RealtimeOpenInfo, RealtimeOutputKind,
    RealtimeProtocolVersion, RealtimeServerFrame, RealtimeTextChunk,
};
use meerkat::{
    RealtimeChannel, RealtimeChannelRole, RealtimeChannelTarget, RealtimeReconnectPolicy,
    RealtimeTurningMode,
};

#[test]
fn session_realtime_channel_builds_default_open_request() {
    let channel = RealtimeChannel::session("session-1");
    let request = channel.open_request();

    assert_eq!(
        request.target,
        RealtimeChannelTarget::SessionTarget {
            session_id: "session-1".to_string(),
        }
    );
    assert_eq!(request.role, RealtimeChannelRole::Primary);
    assert_eq!(request.turning_mode, RealtimeTurningMode::ProviderManaged);
    assert_eq!(request.reconnect_policy, None);
}

#[test]
fn session_realtime_channel_carries_overrides() {
    let request = RealtimeChannel::session("session-builder")
        .role(RealtimeChannelRole::Observer)
        .turning_mode(RealtimeTurningMode::ExplicitCommit)
        .reconnect_policy(RealtimeReconnectPolicy {
            max_attempts: 5,
            initial_backoff_ms: 250,
            max_backoff_ms: 2_000,
            max_total_ms: 15_000,
        })
        .open_request();

    assert_eq!(
        request.target,
        RealtimeChannelTarget::SessionTarget {
            session_id: "session-builder".to_string(),
        }
    );
    assert_eq!(request.role, RealtimeChannelRole::Observer);
    assert_eq!(request.turning_mode, RealtimeTurningMode::ExplicitCommit);
    assert_eq!(
        request.reconnect_policy,
        Some(RealtimeReconnectPolicy {
            max_attempts: 5,
            initial_backoff_ms: 250,
            max_backoff_ms: 2_000,
            max_total_ms: 15_000,
        })
    );
}

#[tokio::test]
async fn realtime_channel_connects_and_exchanges_frames()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{accept_async, tungstenite::Message};

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let ws_url = format!("ws://{addr}/realtime/ws");
    let protocol_version = RealtimeProtocolVersion::CURRENT.as_str().to_string();
    let server_protocol_version = protocol_version.clone();

    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut websocket = accept_async(stream).await?;

        let open = websocket
            .next()
            .await
            .ok_or_else(|| std::io::Error::other("missing open frame"))??;
        let open_frame: RealtimeClientFrame = match open {
            Message::Text(text) => serde_json::from_str(text.as_ref())?,
            other => {
                return Err(std::io::Error::other(format!(
                    "expected text open frame, got {other:?}"
                ))
                .into());
            }
        };
        assert_eq!(
            open_frame,
            RealtimeClientFrame::ChannelOpen(RealtimeChannelOpenFrame {
                protocol_version: server_protocol_version.clone(),
                open_token: "token-1".to_string(),
                role: RealtimeChannelRole::Primary,
                turning_mode: RealtimeTurningMode::ProviderManaged,
            })
        );

        websocket
            .send(Message::Text(
                serde_json::to_string(&RealtimeServerFrame::ChannelOpened(
                    RealtimeChannelOpenedFrame {
                        protocol_version: server_protocol_version.clone(),
                        status: RealtimeChannelStatus {
                            state: RealtimeChannelState::Ready,
                            attempt_count: 0,
                            next_retry_at: None,
                            deadline_at: None,
                            reason: None,
                        },
                        capabilities: RealtimeCapabilities {
                            input_kinds: vec![RealtimeInputKind::Text],
                            output_kinds: vec![RealtimeOutputKind::Text],
                            turning_modes: vec![RealtimeTurningMode::ProviderManaged],
                            interrupt_supported: true,
                            transcript_supported: true,
                            tool_lifecycle_events_supported: false,
                            video_supported: false,
                            audio_input_format: None,
                            audio_output_format: None,
                        },
                        role: RealtimeChannelRole::Primary,
                    },
                ))?
                .into(),
            ))
            .await?;

        let input = websocket
            .next()
            .await
            .ok_or_else(|| std::io::Error::other("missing input frame"))??;
        let input_frame: RealtimeClientFrame = match input {
            Message::Text(text) => serde_json::from_str(text.as_ref())?,
            other => {
                return Err(std::io::Error::other(format!(
                    "expected text input frame, got {other:?}"
                ))
                .into());
            }
        };
        assert_eq!(
            input_frame,
            RealtimeClientFrame::ChannelInput(meerkat::contracts::RealtimeChannelInputFrame {
                chunk: RealtimeInputChunk::TextChunk(RealtimeTextChunk {
                    text: "hello".to_string(),
                }),
            })
        );

        websocket
            .send(Message::Text(
                serde_json::to_string(&RealtimeServerFrame::ChannelEvent(
                    meerkat::contracts::RealtimeChannelEventFrame {
                        event: RealtimeEvent::OutputTextDelta {
                            delta: "world".to_string(),
                        },
                    },
                ))?
                .into(),
            ))
            .await?;

        let commit = websocket
            .next()
            .await
            .ok_or_else(|| std::io::Error::other("missing commit frame"))??;
        let commit_frame: RealtimeClientFrame = match commit {
            Message::Text(text) => serde_json::from_str(text.as_ref())?,
            other => {
                return Err(std::io::Error::other(format!(
                    "expected text commit frame, got {other:?}"
                ))
                .into());
            }
        };
        assert_eq!(commit_frame, RealtimeClientFrame::ChannelCommitTurn);

        let interrupt = websocket
            .next()
            .await
            .ok_or_else(|| std::io::Error::other("missing interrupt frame"))??;
        let interrupt_frame: RealtimeClientFrame = match interrupt {
            Message::Text(text) => serde_json::from_str(text.as_ref())?,
            other => {
                return Err(std::io::Error::other(format!(
                    "expected text interrupt frame, got {other:?}"
                ))
                .into());
            }
        };
        assert_eq!(interrupt_frame, RealtimeClientFrame::ChannelInterrupt);

        let close = websocket
            .next()
            .await
            .ok_or_else(|| std::io::Error::other("missing close frame"))??;
        let close_frame: RealtimeClientFrame = match close {
            Message::Text(text) => serde_json::from_str(text.as_ref())?,
            other => {
                return Err(std::io::Error::other(format!(
                    "expected text close frame, got {other:?}"
                ))
                .into());
            }
        };
        assert_eq!(close_frame, RealtimeClientFrame::ChannelClose);
        websocket
            .send(Message::Text(
                serde_json::to_string(&RealtimeServerFrame::ChannelClosed(
                    RealtimeChannelClosedFrame {
                        reason: Some("client_closed".to_string()),
                    },
                ))?
                .into(),
            ))
            .await?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let open_info = RealtimeOpenInfo {
        ws_url,
        open_token: "token-1".to_string(),
        expires_at: "2026-04-15T12:00:00Z".to_string(),
        target: RealtimeChannelTarget::SessionTarget {
            session_id: "session-1".to_string(),
        },
        supported_protocol_versions: vec![protocol_version.clone()],
        default_protocol_version: protocol_version,
        capabilities: RealtimeCapabilities {
            input_kinds: vec![RealtimeInputKind::Text],
            output_kinds: vec![RealtimeOutputKind::Text],
            turning_modes: vec![RealtimeTurningMode::ProviderManaged],
            interrupt_supported: true,
            transcript_supported: true,
            tool_lifecycle_events_supported: false,
            video_supported: false,
            audio_input_format: None,
            audio_output_format: None,
        },
    };

    let channel = RealtimeChannel::session("session-1");
    let mut connection = channel.connect(&open_info).await?;
    connection
        .send_input(RealtimeInputChunk::TextChunk(RealtimeTextChunk {
            text: "hello".to_string(),
        }))
        .await?;
    let frame = connection
        .next_frame()
        .await?
        .ok_or_else(|| std::io::Error::other("missing output event"))?;
    assert_eq!(
        frame,
        RealtimeServerFrame::ChannelEvent(meerkat::contracts::RealtimeChannelEventFrame {
            event: RealtimeEvent::OutputTextDelta {
                delta: "world".to_string(),
            },
        })
    );
    connection.commit_turn().await?;
    connection.interrupt().await?;
    connection.close().await?;

    server.await??;
    Ok(())
}
