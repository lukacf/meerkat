#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

#[cfg(unix)]
#[tokio::test]
async fn first_delta_reaches_pipe_before_next_event_or_close() {
    use std::io::{BufWriter, Read};
    use std::os::unix::net::UnixStream;

    let (writer, mut reader) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    let (tx, rx) = mpsc::channel(4);
    // A buffered socket acts like piped stdout; a missing flush holds the bytes.
    let processor = tokio::spawn(process_events(rx, BufWriter::new(writer)));
    tx.send(AgentEvent::TextDelta {
        delta: "first".into(),
        assistant_message_id: None,
    })
    .await
    .unwrap();
    let bytes = tokio::task::spawn_blocking(move || {
        let mut bytes = [0; 5];
        reader.read_exact(&mut bytes).unwrap();
        bytes
    })
    .await
    .unwrap();
    assert_eq!(&bytes, b"first");
    assert!(!tx.is_closed());
    assert!(!processor.is_finished());
    drop(tx);
    processor.await.unwrap().unwrap();
}

struct BrokenOutput;

impl Write for BrokenOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "synthetic broken output",
        ))
    }
}

#[tokio::test]
async fn output_failure_is_returned_and_closes_receiver() {
    let (tx, rx) = mpsc::channel(4);
    tx.send(AgentEvent::TextDelta {
        delta: "hello".into(),
        assistant_message_id: None,
    })
    .await
    .unwrap();
    let error = process_events(rx, BrokenOutput).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(tx.is_closed());
}

/// A writer whose bytes the test can read back after `process_events` ends.
#[derive(Clone, Default)]
struct SharedOutput(Arc<std::sync::Mutex<Vec<u8>>>);

impl Write for SharedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The real wiring, with a scripted model: the text delta the model client
/// emits must reach the channel the example hands to the run, and the channel
/// must close when the run is over.
#[tokio::test]
async fn scripted_model_delta_reaches_the_run_channel() {
    use meerkat_client::TestClient;

    let tmp = tempfile::tempdir().unwrap();
    let store_dir = tmp.path().join("sessions");
    std::fs::create_dir_all(&store_dir).unwrap();
    let factory = AgentFactory::new(store_dir.clone());
    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await.unwrap();
    let store = Arc::new(StoreAdapter::new(store));

    // Answers every request with one text delta, "ok", and a normal end.
    let client: Arc<dyn LlmClient> =
        Arc::new(TestClient::for_provider(meerkat_core::Provider::Anthropic));

    let (event_tx, event_rx) = mpsc::channel::<AgentEvent>(64);
    let output = SharedOutput::default();
    let processor = tokio::spawn(process_events(event_rx, output.clone()));

    let result = run_streaming(&factory, client, store, "any prompt", event_tx)
        .await
        .unwrap();

    // Ends only because the run dropped every sender of the channel.
    tokio::time::timeout(std::time::Duration::from_secs(5), processor)
        .await
        .expect("the event channel closes when the run is over")
        .unwrap()
        .unwrap();
    let streamed = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(streamed, "ok");
    assert_eq!(result.text, "ok");
}
