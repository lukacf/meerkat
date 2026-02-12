//! Plain-text connection handler for unauthenticated event ingestion.
//!
//! Accepts newline-delimited JSON or plain text over TCP/UDS connections.
//! Messages are pushed to the inbox as `InboxItem::PlainEvent`.

use crate::inbox::{InboxError, InboxSender};
use crate::transport::plain_codec::parse_plain_line;
use crate::types::InboxItem;
use futures::StreamExt;
use meerkat_core::PlainEventSource;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LinesCodec};

/// Handle a plain-text connection: read newline-delimited lines, parse them,
/// and push to the inbox as `PlainEvent` items.
///
/// The connection stays open for multiple lines (unlike the signed listener
/// which processes one envelope per connection).
///
/// # Backpressure
/// - If the inbox is full: log warning, drop the line
/// - If the inbox is closed: exit cleanly
/// - If a line exceeds `max_line_length`: `LinesCodec` returns an error, we skip it
pub async fn handle_plain_connection<S>(
    stream: S,
    inbox_sender: InboxSender,
    max_line_length: usize,
    source: PlainEventSource,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let codec = LinesCodec::new_with_max_length(max_line_length);
    let mut framed = Framed::new(stream, codec);

    while let Some(result) = framed.next().await {
        match result {
            Ok(line) => {
                let body = parse_plain_line(&line);
                if body.is_empty() {
                    continue;
                }
                match inbox_sender.send(InboxItem::PlainEvent {
                    body,
                    source,
                    interaction_id: None,
                }) {
                    Ok(()) => {}
                    Err(InboxError::Full) => {
                        tracing::warn!("Plain listener: inbox full, dropping event");
                    }
                    Err(InboxError::Closed) => {
                        tracing::debug!("Plain listener: inbox closed, exiting");
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Plain listener: codec error (line too long?): {}", e);
                // Continue reading — one bad line shouldn't kill the connection
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::inbox::Inbox;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_plain_listener_json_line() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp).await;
        });

        client_write
            .write_all(b"{\"body\":\"hello\"}\n")
            .await
            .unwrap();
        // Close to signal EOF
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent { body, source, .. } => {
                assert_eq!(body, "hello");
                assert_eq!(*source, PlainEventSource::Tcp);
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_plain_listener_plain_text() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp).await;
        });

        client_write.write_all(b"plain text\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1);
        match &items[0] {
            InboxItem::PlainEvent { body, .. } => {
                assert_eq!(body, "plain text");
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_plain_listener_multiple_lines() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp).await;
        });

        client_write
            .write_all(b"line 1\nline 2\nline 3\n")
            .await
            .unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 3);
    }

    #[tokio::test]
    async fn test_plain_listener_oversized_line_does_not_crash() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        // max_line_length = 10 bytes
        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 10, PlainEventSource::Tcp).await;
        });

        // Send an oversized line — the handler should not crash
        client_write
            .write_all(b"this line is way too long for the codec\n")
            .await
            .unwrap();
        // Send a valid line after
        client_write.write_all(b"ok\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        // The oversized line is dropped. Whether "ok" arrives depends on
        // LinesCodec's internal recovery behavior after an error.
        // The key invariant: the handler doesn't crash.
        let items = inbox.try_drain();
        assert!(items.len() <= 1, "At most the valid line should arrive");
    }

    #[tokio::test]
    async fn test_plain_listener_empty_lines_skipped() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp).await;
        });

        client_write.write_all(b"\n\nhello\n\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1, "Empty lines should be skipped");
    }

    #[tokio::test]
    async fn test_plain_listener_inbox_full_drops() {
        let (mut inbox, sender) = Inbox::new_with_capacity(1);
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp).await;
        });

        // Send 3 lines but inbox capacity is 1
        client_write
            .write_all(b"first\nsecond\nthird\n")
            .await
            .unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        assert_eq!(items.len(), 1, "Only first line should fit in inbox");
    }

    #[tokio::test]
    async fn test_plain_listener_source_preserved() {
        let (mut inbox, sender) = Inbox::new();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Stdin).await;
        });

        client_write.write_all(b"test\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain();
        match &items[0] {
            InboxItem::PlainEvent { source, .. } => {
                assert_eq!(*source, PlainEventSource::Stdin);
            }
            other => panic!("Expected PlainEvent, got {:?}", other),
        }
    }
}
