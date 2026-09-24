use std::io;

use serde_json::Value;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct StdioJsonWriter {
    tx: mpsc::Sender<Value>,
}

impl StdioJsonWriter {
    pub async fn send(&self, message: Value) -> io::Result<()> {
        self.tx
            .send(message)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer task closed"))
    }
}

pub fn spawn_stdio_json_writer<W>(
    writer: W,
    capacity: usize,
) -> (StdioJsonWriter, JoinHandle<io::Result<()>>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Value>(capacity);
    let task = tokio::spawn(async move {
        let mut writer = writer;
        while let Some(message) = rx.recv().await {
            let line = serde_json::to_string(&message)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
        Ok(())
    });

    (StdioJsonWriter { tx }, task)
}
