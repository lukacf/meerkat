//! Plain-text connection handler for unauthenticated event ingestion.
//!
//! Accepts newline-delimited JSON or plain text over TCP/UDS connections.
//! Messages are pushed to the inbox as `InboxItem::PlainEvent`.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::inbox::{AdmissionOutcome, DropReason, InboxSender};
use crate::transport::plain_codec::classify_plain_line;
use crate::types::InboxItem;
use futures::StreamExt;
use meerkat_core::PlainEventSource;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LinesCodec};

/// A typed, standalone ingress fault for the unauthenticated plain listener.
///
/// Plain ingress is lossy by design (open, unauthenticated). The *fault
/// contract* is the load-bearing half: a dropped line is no longer only a
/// `tracing::warn!` — it is recorded as a typed, observable fault so callers
/// can distinguish overflow from backpressure and surface ingress health.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlainIngressFault {
    /// A line exceeded `max_line_length` (codec framing overflow). The line is
    /// dropped; the connection keeps reading.
    CodecOverflow,
    /// The inbox was at capacity; the classified plain event was dropped.
    InboxFull,
}

/// Observable counters for [`PlainIngressFault`]s on a plain listener.
///
/// Shared across the listener's connection tasks so ingress-fault health is a
/// first-class signal rather than scattered log lines.
#[derive(Debug, Default)]
pub struct PlainIngressFaults {
    codec_overflow: AtomicU64,
    inbox_full: AtomicU64,
}

impl PlainIngressFaults {
    /// Record one occurrence of a typed ingress fault.
    pub fn record(&self, fault: PlainIngressFault) {
        match fault {
            PlainIngressFault::CodecOverflow => {
                self.codec_overflow.fetch_add(1, Ordering::Relaxed);
            }
            PlainIngressFault::InboxFull => {
                self.inbox_full.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Count of codec-overflow (line-too-long) faults observed.
    pub fn codec_overflow(&self) -> u64 {
        self.codec_overflow.load(Ordering::Relaxed)
    }

    /// Count of inbox-full backpressure drops observed.
    pub fn inbox_full(&self) -> u64 {
        self.inbox_full.load(Ordering::Relaxed)
    }

    /// Total ingress faults of any kind observed.
    pub fn total(&self) -> u64 {
        self.codec_overflow() + self.inbox_full()
    }
}

/// Handle a plain-text connection: read newline-delimited lines, classify them
/// through the typed [`classify_plain_line`] contract, and push to the inbox as
/// `PlainEvent` items.
///
/// The connection stays open for multiple lines (unlike the signed listener
/// which processes one envelope per connection).
///
/// # Backpressure / fault contract
/// - If the inbox is full: record [`PlainIngressFault::InboxFull`], drop the line.
/// - If the inbox is closed: exit cleanly (not a fault).
/// - If a line exceeds `max_line_length`: record [`PlainIngressFault::CodecOverflow`]
///   and keep reading (one bad line shouldn't kill the connection).
pub async fn handle_plain_connection<S>(
    stream: S,
    inbox_sender: InboxSender,
    max_line_length: usize,
    source: PlainEventSource,
    faults: &PlainIngressFaults,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let codec = LinesCodec::new_with_max_length(max_line_length);
    let mut framed = Framed::new(stream, codec);

    while let Some(result) = framed.next().await {
        match result {
            Ok(line) => {
                let classified = classify_plain_line(&line);
                if classified.is_empty() {
                    continue;
                }
                match inbox_sender.send_classified(InboxItem::PlainEvent {
                    body: classified.render(),
                    source,
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    interaction_id: None,
                    blocks: None,
                    render_metadata: None,
                    objective_id: None,
                }) {
                    AdmissionOutcome::Admitted => {}
                    AdmissionOutcome::Dropped {
                        reason: DropReason::InboxFull,
                    } => {
                        faults.record(PlainIngressFault::InboxFull);
                        tracing::warn!(
                            inbox_full_faults = faults.inbox_full(),
                            "Plain listener: inbox full, dropping event"
                        );
                    }
                    AdmissionOutcome::Dropped {
                        reason: DropReason::SessionClosed,
                    } => {
                        tracing::debug!("Plain listener: inbox closed, exiting");
                        return;
                    }
                    AdmissionOutcome::Dropped { reason } => {
                        tracing::warn!(
                            reason = ?reason,
                            "Plain listener: inbox dropped plain event at ingress"
                        );
                    }
                }
            }
            Err(e) => {
                faults.record(PlainIngressFault::CodecOverflow);
                tracing::warn!(
                    codec_overflow_faults = faults.codec_overflow(),
                    "Plain listener: codec error (line too long?): {}",
                    e
                );
                // Continue reading — one bad line shouldn't kill the connection
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::classify::test_support;
    use crate::inbox::Inbox;
    use crate::trust::TrustStore;
    use tokio::io::AsyncWriteExt;

    fn classified_inbox() -> (Inbox, crate::InboxSender) {
        Inbox::new_classified(test_support::classification_context(
            TrustStore::new(),
            false,
        ))
    }

    fn classified_inbox_with_capacity(capacity: usize) -> (Inbox, crate::InboxSender) {
        Inbox::new_classified_with_capacity_for_test(
            test_support::classification_context(TrustStore::new(), false),
            capacity,
        )
    }

    fn faults() -> std::sync::Arc<PlainIngressFaults> {
        std::sync::Arc::new(PlainIngressFaults::default())
    }

    #[tokio::test]
    async fn test_plain_listener_json_line() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();
        let faults_task = faults.clone();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp, &faults_task)
                .await;
        });

        client_write
            .write_all(b"{\"body\":\"hello\",\"extra\":1}\n")
            .await
            .unwrap();
        // Close to signal EOF
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::PlainEvent { body, source, .. } => {
                // Typed contract: a structured payload is rendered whole — the
                // `body` field is NOT extracted out of the JSON object.
                assert!(body.contains("hello"));
                assert!(body.contains("extra"));
                assert_ne!(body, "hello");
                assert_eq!(*source, PlainEventSource::Tcp);
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
        assert_eq!(faults.total(), 0);
    }

    #[tokio::test]
    async fn test_plain_listener_plain_text() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp, &faults).await;
        });

        client_write.write_all(b"plain text\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1);
        match &items[0].item {
            InboxItem::PlainEvent { body, .. } => {
                assert_eq!(body, "plain text");
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_plain_listener_multiple_lines() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp, &faults).await;
        });

        client_write
            .write_all(b"line 1\nline 2\nline 3\n")
            .await
            .unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 3);
    }

    /// ROW #241 gate (overflow half): an oversized line is recorded as a typed
    /// [`PlainIngressFault::CodecOverflow`] counter, not only a `tracing::warn!`.
    #[tokio::test]
    async fn test_plain_listener_oversized_line_records_typed_fault() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();
        let faults_task = faults.clone();

        // max_line_length = 10 bytes
        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 10, PlainEventSource::Tcp, &faults_task).await;
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
        let items = inbox.try_drain_classified();
        assert!(items.len() <= 1, "At most the valid line should arrive");
        // The overflow is surfaced as a typed, observable ingress fault.
        assert!(
            faults.codec_overflow() >= 1,
            "an oversized line must record a typed CodecOverflow fault"
        );
    }

    #[tokio::test]
    async fn test_plain_listener_empty_lines_skipped() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp, &faults).await;
        });

        client_write.write_all(b"\n\nhello\n\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1, "Empty lines should be skipped");
    }

    /// ROW #241 gate (backpressure half): inbox-full drops are recorded as a
    /// typed [`PlainIngressFault::InboxFull`] counter, not only a warn+drop.
    #[tokio::test]
    async fn test_plain_listener_inbox_full_records_typed_fault() {
        let (mut inbox, sender) = classified_inbox_with_capacity(1);
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();
        let faults_task = faults.clone();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Tcp, &faults_task)
                .await;
        });

        // Send 3 lines but inbox capacity is 1
        client_write
            .write_all(b"first\nsecond\nthird\n")
            .await
            .unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        assert_eq!(items.len(), 1, "Only first line should fit in inbox");
        // The two overflow lines are surfaced as typed inbox-full faults.
        assert!(
            faults.inbox_full() >= 1,
            "inbox-full backpressure must record a typed InboxFull fault"
        );
    }

    #[tokio::test]
    async fn test_plain_listener_source_preserved() {
        let (mut inbox, sender) = classified_inbox();
        let (client, server) = tokio::io::duplex(4096);
        let (_, mut client_write) = tokio::io::split(client);
        let faults = faults();

        let handle = tokio::spawn(async move {
            handle_plain_connection(server, sender, 1024, PlainEventSource::Stdin, &faults).await;
        });

        client_write.write_all(b"test\n").await.unwrap();
        drop(client_write);
        handle.await.unwrap();

        let items = inbox.try_drain_classified();
        match &items[0].item {
            InboxItem::PlainEvent { source, .. } => {
                assert_eq!(*source, PlainEventSource::Stdin);
            }
            other => panic!("Expected PlainEvent, got {other:?}"),
        }
    }
}
