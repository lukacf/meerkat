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
    })
    .await
    .unwrap();
    let error = process_events(rx, BrokenOutput).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
    assert!(tx.is_closed());
}
