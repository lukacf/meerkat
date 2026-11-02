//! Piped stdin as prompt context for `rkat run` (`--stdin auto` and `--stdin blob`).
//!
//! The read runs on its own OS thread, so a stdin that stays open and silent
//! never blocks the command forever. With `auto`, input that has not started
//! within a short grace period is treated as absent: one warning names the
//! flags that choose otherwise and the run continues without stdin. Once
//! input starts it is read to end of file. With `blob` the run keeps waiting
//! for end of file and says so after the grace period.
//!
//! A reader thread still blocked on an open stdin is not joined; it ends with
//! the process.

use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

/// How long `--stdin auto` waits for piped input to start.
pub(crate) const AUTO_GRACE: Duration = Duration::from_secs(2);

/// Whether the run gives up on stdin that has not started, or waits for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    /// `--stdin auto`: give up after the grace period if no input started.
    StartWithin(Duration),
    /// `--stdin blob`: wait for end of file; notice after the grace period.
    UntilEof { notice_after: Duration },
}

/// What happened to stdin, for the caller's one line on stderr.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Input read to end of file (possibly empty).
    Read(String),
    /// `auto` saw no input start within its grace period.
    NotStarted,
    /// The read failed.
    Failed(String),
}

/// Read `reader` on a dedicated thread according to `wait`. `on_notice` runs
/// once when `UntilEof` has waited `notice_after` without input.
pub(crate) fn read_blob<R>(reader: R, wait: Wait, mut on_notice: impl FnMut()) -> Outcome
where
    R: Read + Send + 'static,
{
    let (send, chunks) = mpsc::sync_channel::<std::io::Result<Vec<u8>>>(16);
    let spawned = std::thread::Builder::new()
        .name("rkat-stdin".into())
        .spawn(move || {
            let mut reader = reader;
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let chunk = match reader.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(read) => Ok(buffer[..read].to_vec()),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => Err(error),
                };
                let failed = chunk.is_err();
                if send.send(chunk).is_err() || failed {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        return Outcome::Failed(format!("could not start the stdin reader: {error}"));
    }

    // Wait for the first event (a chunk, an error or end of file).
    let first = match wait {
        Wait::StartWithin(grace) => match chunks.recv_timeout(grace) {
            Ok(event) => Some(event),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
            // Input that arrives after this give-up may still be read by the
            // detached reader: one chunk at most, discarded when its send
            // fails. Nothing else reads stdin in `run`, so nothing is lost.
            Err(mpsc::RecvTimeoutError::Timeout) => return Outcome::NotStarted,
        },
        Wait::UntilEof { notice_after } => match chunks.recv_timeout(notice_after) {
            Ok(event) => Some(event),
            Err(mpsc::RecvTimeoutError::Disconnected) => None,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                on_notice();
                chunks.recv().ok()
            }
        },
    };

    // Input started (or ended): read to end of file.
    let mut bytes = Vec::new();
    let mut next = first;
    while let Some(event) = next {
        match event {
            Ok(chunk) => bytes.extend_from_slice(&chunk),
            Err(error) => return Outcome::Failed(error.to_string()),
        }
        next = chunks.recv().ok();
    }
    // Invalid UTF-8 fails as `read_to_string` did, so the caller warns and
    // runs without stdin instead of passing mangled input on.
    match String::from_utf8(bytes) {
        Ok(text) => Outcome::Read(text),
        Err(_) => Outcome::Failed("stream did not contain valid UTF-8".into()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use std::time::Instant;

    /// A reader that blocks until its pipe's write end is dropped.
    fn held_open_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
        std::io::pipe().expect("pipe")
    }

    #[test]
    fn auto_reads_input_that_is_already_there() {
        let outcome = read_blob(
            Cursor::new(b"hello\n".to_vec()),
            Wait::StartWithin(AUTO_GRACE),
            || {},
        );
        assert_eq!(outcome, Outcome::Read("hello\n".into()));
    }

    #[test]
    fn auto_treats_immediate_eof_as_empty_input() {
        let outcome = read_blob(
            Cursor::new(Vec::new()),
            Wait::StartWithin(AUTO_GRACE),
            || {},
        );
        assert_eq!(outcome, Outcome::Read(String::new()));
    }

    #[test]
    fn invalid_utf8_fails_like_read_to_string() {
        let outcome = read_blob(
            Cursor::new(vec![b'o', b'k', 0xff, 0xfe]),
            Wait::StartWithin(AUTO_GRACE),
            || {},
        );
        assert_eq!(
            outcome,
            Outcome::Failed("stream did not contain valid UTF-8".into())
        );
    }

    #[test]
    fn auto_gives_up_on_a_held_open_silent_stdin_after_the_grace() {
        let (reader, _writer) = held_open_pipe();
        let started = Instant::now();
        let outcome = read_blob(reader, Wait::StartWithin(Duration::from_millis(200)), || {});
        assert_eq!(outcome, Outcome::NotStarted);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "auto must not wait for end of file"
        );
    }

    #[test]
    fn auto_reads_to_eof_once_input_started() {
        let (reader, mut writer) = held_open_pipe();
        let feeder = std::thread::spawn(move || {
            writer.write_all(b"part one ").unwrap();
            std::thread::sleep(Duration::from_millis(300));
            writer.write_all(b"part two").unwrap();
        });
        let outcome = read_blob(reader, Wait::StartWithin(Duration::from_secs(10)), || {});
        feeder.join().unwrap();
        assert_eq!(outcome, Outcome::Read("part one part two".into()));
    }

    #[test]
    fn blob_waits_past_the_grace_and_gives_one_notice() {
        let (reader, mut writer) = held_open_pipe();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            writer.write_all(b"late").unwrap();
        });
        let mut notices = 0;
        let outcome = read_blob(
            reader,
            Wait::UntilEof {
                notice_after: Duration::from_millis(100),
            },
            || notices += 1,
        );
        feeder.join().unwrap();
        assert_eq!(outcome, Outcome::Read("late".into()));
        assert_eq!(notices, 1);
    }
}
