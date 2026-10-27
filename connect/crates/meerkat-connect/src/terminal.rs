//! Terminal presentation only. No installation, question or approval state lives here.

use std::{
    future::Future,
    io::{self, Write},
    os::fd::{AsFd, OwnedFd},
};

use meerkat_core::{
    ResolvedStorage,
    runtime_bootstrap::{RuntimeBootstrapError, validate_explicit_realm_id},
};
use meerkat_runtime::completion::CompletionOutcome;
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    sys::termios::{
        FlushArg, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices, Termios, tcflush,
        tcgetattr, tcsetattr,
    },
    unistd::read,
};
use tokio::io::unix::AsyncFd;
#[cfg(test)]
use tokio::sync::mpsc;

use crate::{
    ConnectError, RealmChoice, RealmOptions,
    paired_host::{PairedHostError, PairingCode, TerminalPairingCode},
};

/// Lossless terminal review notation. Newlines, tabs and ordinary Unicode
/// remain readable; controls, direction markers and literal backslashes are
/// visible, so external text cannot conceal or reorder a reviewed operation.
pub fn review_text(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars() {
        match ch {
            '\\' => result.push_str("\\\\"),
            '\n' | '\t' => result.push(ch),
            ch if ch.is_control()
                || matches!(ch,
                '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' |
                '\u{2060}'..='\u{206f}' | '\u{feff}') =>
            {
                use std::fmt::Write;
                let _ = write!(result, "\\u{{{:04X}}}", ch as u32);
            }
            ch => result.push(ch),
        }
    }
    result
}

/// Strip terminal control sequences from untrusted model/catalog output.
/// Keep readable line breaks and tabs. Escape bytes are rendered visibly.
pub fn display_text(text: &str) -> String {
    text.chars()
        .flat_map(|ch| {
            if ch == '\u{1b}' {
                "[ESC]".chars().collect::<Vec<_>>()
            } else if ch == '\n' || ch == '\t' || !ch.is_control() {
                vec![ch]
            } else {
                vec![]
            }
        })
        .collect()
}

enum InputSource {
    Stdin,
    Owned(OwnedFd),
    #[cfg(test)]
    Scripted(mpsc::Receiver<io::Result<String>>),
}

/// The presenter's sole input owner. A read starts only when its future is
/// polled; there is no background task or input read beyond the active phase.
/// Partial ordinary input remains here when a read future is cancelled.
/// Do not read the same input through another owner while this value is in use.
pub struct TerminalInput {
    source: InputSource,
    poisoned: bool,
    ended: bool,
    pending: Vec<u8>,
}

/// Construct the single stdin owner without reading or changing the terminal.
pub fn input_lines() -> TerminalInput {
    TerminalInput {
        source: InputSource::Stdin,
        poisoned: false,
        ended: false,
        pending: Vec::new(),
    }
}

impl TerminalInput {
    /// Adopt an exclusively owned input, for an embedding or an isolated PTY.
    /// The caller must not retain a competing reader of the same terminal.
    pub fn from_owned_fd(input: OwnedFd) -> Self {
        Self {
            source: InputSource::Owned(input),
            poisoned: false,
            ended: false,
            pending: Vec::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn scripted(lines: mpsc::Receiver<io::Result<String>>) -> Self {
        Self {
            source: InputSource::Scripted(lines),
            poisoned: false,
            ended: false,
            pending: Vec::new(),
        }
    }

    #[cfg(test)]
    fn try_recv(&mut self) -> Result<io::Result<String>, mpsc::error::TryRecvError> {
        match &mut self.source {
            InputSource::Scripted(lines) => lines.try_recv(),
            _ => Err(mpsc::error::TryRecvError::Empty),
        }
    }

    /// Read back a failed synchronous Drop cleanup before continuing or exiting
    /// after a cancelled read. Subsequent reads also check this automatically.
    pub fn check_cleanup(&self) -> io::Result<()> {
        if self.poisoned {
            return Err(io::Error::other(
                "terminal cleanup failed; input cannot continue",
            ));
        }
        Ok(())
    }

    fn descriptor(&self) -> io::Result<OwnedFd> {
        self.check_cleanup()?;
        match &self.source {
            InputSource::Stdin => io::stdin().as_fd().try_clone_to_owned(),
            InputSource::Owned(fd) => fd.try_clone(),
            #[cfg(test)]
            InputSource::Scripted(_) => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "protected entry requires a terminal",
            )),
        }
    }

    /// Check the protected-entry prerequisite before requesting a host code.
    /// This checks the captured input, not a separately opened /dev/tty.
    pub fn require_terminal(&self) -> io::Result<()> {
        let fd = self.descriptor()?;
        tcgetattr(&fd).map_err(|_| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "protected entry requires a terminal",
            )
        })?;
        Ok(())
    }

    /// Read one ordinary line. Dropping this future stops that read immediately
    /// and preserves bytes already read. No worker retains input custody.
    pub async fn recv(&mut self) -> Option<io::Result<String>> {
        if let Err(error) = self.read_ordinary(false).await {
            return Some(Err(error));
        }
        if self.pending.is_empty() && self.ended {
            return None;
        }
        let newline = self.pending.iter().position(|byte| *byte == b'\n');
        let count = newline.map_or(self.pending.len(), |index| index + 1);
        let mut bytes: Vec<_> = self.pending.drain(..count).collect();
        if newline.is_some() {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
        Some(
            String::from_utf8(bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "terminal input is not UTF-8")
            }),
        )
    }

    /// Observe terminal closure during a non-input operation. Ordinary input is
    /// retained, never interpreted as a decision. Cancellation preserves every
    /// byte already read; the total pending input bound is 64 KiB plus newline.
    pub(crate) async fn wait_for_end(&mut self) -> io::Result<()> {
        self.read_ordinary(true).await
    }

    async fn read_ordinary(&mut self, until_end: bool) -> io::Result<()> {
        self.check_cleanup()?;
        if self.ended || (!until_end && self.pending.contains(&b'\n')) {
            return Ok(());
        }
        #[cfg(test)]
        if let InputSource::Scripted(lines) = &mut self.source {
            loop {
                match lines.recv().await {
                    Some(line) => {
                        let line = line?;
                        if self.pending.len() + line.len() > 64 * 1024 {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "terminal input exceeds the supported input limit",
                            ));
                        }
                        self.pending.extend_from_slice(line.as_bytes());
                        self.pending.push(b'\n');
                        if !until_end {
                            return Ok(());
                        }
                    }
                    None => {
                        self.ended = true;
                        return Ok(());
                    }
                }
            }
        }
        let fd = self.descriptor()?;
        let mut mode = InputMode::enter(fd, &mut self.poisoned, false)?;
        let result =
            read_ordinary_bytes(&mode.fd, &mut self.pending, &mut self.ended, until_end).await;
        mode.finish()?;
        result
    }

    /// Begin a visible approval with fresh input, like the existing hidden-entry
    /// boundary. Discard type-ahead before displaying the prompt, not an account
    /// result or native state. EOF stays latched. This is the same sole reader.
    pub(crate) fn fresh_prompt(&mut self) -> io::Result<bool> {
        self.check_cleanup()?;
        self.pending.clear();
        if self.ended {
            return Ok(false);
        }
        #[cfg(test)]
        if let InputSource::Scripted(lines) = &mut self.source {
            loop {
                match lines.try_recv() {
                    Ok(line) => {
                        line?;
                    }
                    Err(mpsc::error::TryRecvError::Empty) => return Ok(true),
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        self.ended = true;
                        return Ok(false);
                    }
                }
            }
        }
        let fd = self.descriptor()?;
        let mut mode = InputMode::enter(fd, &mut self.poisoned, false)?;
        let result = discard_ready_input(&mode.fd, &mut self.ended).and_then(|()| {
            if self.ended {
                return Ok(());
            }
            // Also discard an unfinished kernel canonical line, which a
            // nonblocking read cannot receive before Enter or Ctrl-D.
            match tcflush(&mode.fd, FlushArg::TCIFLUSH) {
                Ok(()) | Err(nix::errno::Errno::ENOTTY) => Ok(()),
                Err(error) => Err(io::Error::from(error)),
            }
        });
        mode.finish()?;
        result?;
        Ok(!self.ended)
    }

    /// Read a hidden pairing code directly into its redacted type. The secret
    /// never passes through `recv` or a general conversation-input queue.
    /// Queued input is discarded before the prompt and on every exit. EOF and
    /// /quit return None. Cancellation is dropping this future; the mode guard
    /// restores synchronously, and a restoration failure poisons future input.
    /// Local format errors can be corrected without leaving hidden input.
    pub async fn read_pairing_code(
        &mut self,
        output: &mut impl Write,
    ) -> io::Result<Option<PairingCode>> {
        self.read_hidden_code(
            output,
            "Enter the six-digit code (hidden), or /quit: ",
            PairingCode::new,
            "enter a six-digit pairing code",
        )
        .await
    }

    /// Host-terminal pairing uses the same exclusive hidden input custody.
    /// The code is returned only in its redacted type, never as a chat line.
    pub async fn read_terminal_pairing_code(
        &mut self,
        output: &mut impl Write,
    ) -> io::Result<Option<TerminalPairingCode>> {
        self.read_hidden_code(
            output,
            "Enter the 12-symbol host-terminal code (hidden), or /quit: ",
            TerminalPairingCode::new,
            "enter a 12-symbol host-terminal pairing code",
        )
        .await
    }

    /// Read a model API key under the same exclusive, hidden terminal guard.
    /// The native redacted value never enters ordinary conversation input.
    pub(crate) async fn read_api_key(
        &mut self,
        output: &mut impl Write,
    ) -> io::Result<Option<meerkat_core::auth::token_store::PersistedTokens>> {
        let fd = self.descriptor()?;
        self.pending.clear();
        let mut mode = InputMode::enter(fd, &mut self.poisoned, true)?;
        // Keep terminal echo disabled across local corrections. A rejected
        // value has not entered native credential storage or another input phase.
        let line = loop {
            write!(output, "Enter the API key (hidden), or /quit: ")?;
            output.flush()?;
            let line = read_line(&mode.fd, 8 * 1024).await;
            match &line {
                Ok(Some(value))
                    if value.trim().is_empty() || value.chars().any(char::is_control) =>
                {
                    writeln!(output)?;
                    writeln!(
                        output,
                        "API key must be nonempty and contain no control characters. Enter it again, or /quit."
                    )?;
                    output.flush()?;
                }
                // Transport, encoding and line-limit errors remain errors.
                _ => break line,
            }
        };
        mode.finish()?;
        writeln!(output)?;
        output.flush()?;
        match line? {
            None => Ok(None),
            Some(line) if line == "/quit" => Ok(None),
            Some(line) => Ok(Some(
                meerkat_core::auth::token_store::PersistedTokens::api_key(line),
            )),
        }
    }

    /// Preserve the Forge password's Unicode and whitespace under one hidden
    /// input guard. Enter finishes the single line; EOF/edit/signal keys keep
    /// their terminal meaning. No value passes through conversation input.
    pub(crate) async fn read_forge_password(
        &mut self,
        output: &mut impl Write,
    ) -> io::Result<Option<crate::forge::ForgePassword>> {
        let fd = self.descriptor()?;
        self.pending.clear();
        let mut mode = InputMode::enter(fd, &mut self.poisoned, true)?;
        mode.forge_password()?;
        let result = loop {
            write!(output, "Forge Console password (hidden), or /quit: ")?;
            output.flush()?;
            match read_password_line(&mode.fd, 16 * 1024).await {
                Ok(None) => break Ok(None),
                Ok(Some(line)) if line == "/quit" => break Ok(None),
                Ok(Some(line)) => match crate::forge::ForgePassword::new(line) {
                    Ok(password) => break Ok(Some(password)),
                    Err(error) => {
                        writeln!(output, "\n{error}. Enter it again, or /quit.")?;
                        output.flush()?;
                    }
                },
                Err(error) => break Err(error),
            }
        };
        mode.finish()?;
        writeln!(output)?;
        output.flush()?;
        result
    }

    /// Service credentials go only to the retained protected installation.
    /// Slot labels are separately validated names. No secret enters recv().
    pub(crate) async fn read_service_secret(
        &mut self, output: &mut impl Write,
    ) -> io::Result<Option<ServiceSecret>> {
        let fd = self.descriptor()?;
        self.pending.clear();
        let mut mode = InputMode::enter(fd, &mut self.poisoned, true)?;
        mode.forge_password()?;
        let result = loop {
            write!(output, "Service credential (hidden), or /quit: ")?;
            output.flush()?;
            match read_password_line(&mode.fd, 16 * 1024).await {
                Ok(None) => break Ok(None),
                Ok(Some(line)) if line == "/quit" => break Ok(None),
                Ok(Some(line)) if line.is_empty() || line.contains('\0') => {
                    writeln!(output, "\nThe credential must be nonempty and contain no NUL. Enter it again, or /quit.")?;
                    output.flush()?;
                },
                Ok(Some(line)) => break ServiceSecret::from_hidden_input(line).map(Some),
                Err(error) => break Err(error),
            }
        };
        mode.finish()?;
        writeln!(output)?;
        output.flush()?;
        result
    }

    /// Connector client secrets go directly to the native update value, never
    /// to recv(), a model tool, or a Debug/serializable host value.
    pub(crate) async fn read_connector_client_secret(
        &mut self, output: &mut impl Write,
    ) -> io::Result<Option<meerkat::ConnectorClientSecretUpdate>> {
        let fd = self.descriptor()?;
        self.pending.clear();
        let mut mode = InputMode::enter(fd, &mut self.poisoned, true)?;
        mode.forge_password()?;
        let result = loop {
            write!(output, "Connector client secret (hidden), or /quit: ")?;
            output.flush()?;
            match read_password_line(&mode.fd, 16 * 1024).await {
                Ok(None) => break Ok(None),
                Ok(Some(line)) if line == "/quit" => break Ok(None),
                Ok(Some(line)) if line.is_empty() => {
                    writeln!(output, "\nThe client secret is empty. Enter it again, or /quit.")?;
                    output.flush()?;
                }
                Ok(Some(line)) => break Ok(Some(meerkat::ConnectorClientSecretUpdate::Set(line))),
                Err(error) => break Err(error),
            }
        };
        mode.finish()?;
        writeln!(output)?;
        output.flush()?;
        result
    }

    async fn read_hidden_code<T>(
        &mut self,
        output: &mut impl Write,
        prompt: &str,
        parse: fn(String) -> Result<T, PairedHostError>,
        invalid: &'static str,
    ) -> io::Result<Option<T>> {
        let fd = self.descriptor()?;
        self.pending.clear();
        let mut mode = InputMode::enter(fd, &mut self.poisoned, true)?;
        let result = loop {
            write!(output, "{prompt}")?;
            output.flush()?;
            match read_line(&mode.fd, 64).await {
                Ok(None) => break Ok(None),
                Ok(Some(line)) if line == "/quit" => break Ok(None),
                Ok(Some(line)) => match parse(line) {
                    Ok(code) => break Ok(Some(code)),
                    Err(_) => {
                        // No code has left this input owner. Keep the same
                        // hidden guard while the human corrects its format.
                        writeln!(output)?;
                        writeln!(output, "Please {invalid}, or /quit.")?;
                        output.flush()?;
                    }
                },
                // Transport, encoding and line-limit errors remain errors.
                Err(error) => break Err(error),
            }
        };
        mode.finish()?;
        writeln!(output)?;
        output.flush()?;
        result
    }
}

/// This guard is outside the awaited read. The inner AsyncFd registration is
/// dropped first, then Drop restores the original descriptor flags and mode.
/// No asynchronous cleanup or background terminal owner is left behind.
struct InputMode<'a> {
    fd: OwnedFd,
    flags: OFlag,
    termios: Option<Termios>,
    poisoned: &'a mut bool,
    finished: bool,
}

impl<'a> InputMode<'a> {
    fn enter(fd: OwnedFd, poisoned: &'a mut bool, secret: bool) -> io::Result<Self> {
        let flags = OFlag::from_bits_retain(fcntl(&fd, FcntlArg::F_GETFL)?);
        let termios = if secret {
            Some(tcgetattr(&fd).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "protected entry requires a terminal",
                )
            })?)
        } else {
            None
        };
        let guard = Self {
            fd,
            flags,
            termios,
            poisoned,
            finished: false,
        };
        fcntl(&guard.fd, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK))?;
        if let Some(original) = &guard.termios {
            let mut hidden = original.clone();
            hidden.local_flags.remove(
                LocalFlags::ECHO | LocalFlags::ECHONL | LocalFlags::ECHOE | LocalFlags::ECHOK,
            );
            tcsetattr(&guard.fd, SetArg::TCSANOW, &hidden)?;
            tcflush(&guard.fd, FlushArg::TCIFLUSH)?;
        }
        Ok(guard)
    }

    /// Avoid the kernel canonical line limit for this bounded password reader.
    /// Keep signal handling; line finishing, EOF and editing remain input keys.
    fn forge_password(&self) -> io::Result<()> {
        let mut hidden = tcgetattr(&self.fd)?;
        hidden
            .local_flags
            .remove(LocalFlags::ICANON | LocalFlags::IEXTEN);
        hidden.input_flags.remove(
            InputFlags::IGNCR
                | InputFlags::ICRNL
                | InputFlags::INLCR
                | InputFlags::ISTRIP
                | InputFlags::IXON
                | InputFlags::IXOFF,
        );
        hidden.control_chars[SpecialCharacterIndices::VMIN as usize] = 1;
        hidden.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
        tcsetattr(&self.fd, SetArg::TCSANOW, &hidden)?;
        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let mut failed = false;
        if let Some(original) = &self.termios {
            // Flush on both sides of mode restoration: type-ahead belonging
            // to the secret phase must not become later conversation input.
            failed |= tcflush(&self.fd, FlushArg::TCIFLUSH).is_err();
            failed |= tcsetattr(&self.fd, SetArg::TCSANOW, original).is_err();
            failed |= tcflush(&self.fd, FlushArg::TCIFLUSH).is_err();
        }
        failed |= fcntl(&self.fd, FcntlArg::F_SETFL(self.flags)).is_err();
        if failed {
            *self.poisoned = true;
            Err(io::Error::other(
                "terminal cleanup failed; input cannot continue",
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for InputMode<'_> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

// These helpers are called only while TerminalInput owns the nonblocking mode.
// Read one byte at a time so an ordinary line never prefetches a later secret.
async fn read_ordinary_bytes(
    fd: &OwnedFd,
    pending: &mut Vec<u8>,
    ended: &mut bool,
    until_end: bool,
) -> io::Result<()> {
    let input = AsyncFd::new(fd.try_clone()?)?;
    loop {
        let mut byte = [0u8; 1];
        let mut ready = input.readable().await?;
        let count =
            match ready.try_io(|input| read(input.get_ref(), &mut byte).map_err(io::Error::from)) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
            };
        if count == 0 {
            *ended = true;
            return Ok(());
        }
        if pending.len() >= 64 * 1024 + usize::from(byte[0] == b'\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal input exceeds the supported input limit",
            ));
        }
        pending.push(byte[0]);
        if !until_end && byte[0] == b'\n' {
            return Ok(());
        }
    }
}

fn discard_ready_input(fd: &OwnedFd, ended: &mut bool) -> io::Result<()> {
    // A pipe or socket has no terminal queue to flush. Drain only currently
    // available bytes, with the same finite input bound and no blocking read.
    for _ in 0..=64 * 1024 {
        let mut byte = [0u8; 1];
        match read(fd, &mut byte) {
            Ok(0) => {
                *ended = true;
                return Ok(());
            }
            Ok(_) => (),
            Err(nix::errno::Errno::EAGAIN) => return Ok(()),
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "terminal input exceeds the supported input limit",
    ))
}

async fn read_password_line(fd: &OwnedFd, limit: usize) -> io::Result<Option<String>> {
    let input = AsyncFd::new(fd.try_clone()?)?;
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0u8; 1];
        let mut ready = input.readable().await?;
        let count =
            match ready.try_io(|input| read(input.get_ref(), &mut byte).map_err(io::Error::from)) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
            };
        if count == 0 || matches!(byte[0], b'\n' | b'\r' | 4) {
            if (count == 0 || byte[0] == 4) && bytes.is_empty() {
                return Ok(None);
            }
            return String::from_utf8(bytes).map(Some).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "terminal input is not UTF-8")
            });
        }
        if matches!(byte[0], 8 | 127) {
            // Remove one complete UTF-8 scalar, never leave half an edited codepoint.
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "terminal input is not UTF-8")
            })?;
            if let Some((offset, _)) = text.char_indices().next_back() {
                bytes.truncate(offset);
            }
            continue;
        }
        if bytes.len() == limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal input exceeds the supported line limit",
            ));
        }
        bytes.push(byte[0]);
    }
}

async fn read_line(fd: &OwnedFd, limit: usize) -> io::Result<Option<String>> {
    let input = AsyncFd::new(fd.try_clone()?)?;
    let mut bytes = Vec::new();
    loop {
        // One byte avoids userspace prefetch across visible/secret phases.
        let mut byte = [0u8; 1];
        let mut ready = input.readable().await?;
        let count =
            match ready.try_io(|input| read(input.get_ref(), &mut byte).map_err(io::Error::from)) {
                Err(_) => continue,
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
            };
        if count == 0 || byte[0] == b'\n' {
            if count == 0 && bytes.is_empty() {
                return Ok(None);
            }
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            return String::from_utf8(bytes).map(Some).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "terminal input is not UTF-8")
            });
        }
        if bytes.len() == limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal input exceeds the supported line limit",
            ));
        }
        bytes.push(byte[0]);
    }
}

/// Present the already discovered inventory and resolve an explicit choice.
/// Invalid interactive input can be corrected here. Native storage resolution
/// failures remain errors; selecting a name does not open or create its realm.
pub async fn select_realm(
    options: &RealmOptions,
    realms: &[RealmChoice],
    lines: &mut TerminalInput,
    output: &mut impl Write,
    cancelled: impl Future<Output = io::Result<()>>,
) -> Result<Option<ResolvedStorage>, ConnectError> {
    for (index, realm) in realms.iter().enumerate() {
        writeln!(
            output,
            "{}. {} ({})",
            index + 1,
            display_text(&realm.realm_id),
            display_text(&realm.state_root.display().to_string())
        )
        .map_err(|e| ConnectError::native("write realm choices", e))?;
    }
    if realms.is_empty() {
        writeln!(
            output,
            "No stored realms found. Enter a name to create a native realm, or /quit."
        )
        .map_err(|e| ConnectError::native("write realm choices", e))?;
    }
    let mut entering_name = realms.is_empty();
    tokio::pin!(cancelled);
    loop {
        write!(
            output,
            "{}",
            if entering_name {
                "New realm: "
            } else {
                "Choose a realm number, 'new', or /quit: "
            }
        )
        .and_then(|_| output.flush())
        .map_err(|e| ConnectError::native("write realm prompt", e))?;
        let line = tokio::select! {
            biased;
            signal = &mut cancelled => {
                lines.check_cleanup()
                    .map_err(|e| ConnectError::native("restore realm input", e))?;
                signal.map_err(|e| ConnectError::native("listen for cancellation", e))?;
                return Ok(None);
            }
            line = lines.recv() => line.transpose()
                .map_err(|e| ConnectError::native("read realm selection", e))?,
        };
        let Some(line) = line else {
            return Ok(None);
        };
        if line.trim() == "/quit" {
            return Ok(None);
        }
        if !entering_name && line.trim() == "new" {
            entering_name = true;
            continue;
        }
        if entering_name {
            match validate_explicit_realm_id(&line) {
                Ok(()) => {
                    let selected = options.resolve(&line)?;
                    if realms.iter().any(|realm| {
                        realm.realm_id == selected.locator.realm.as_str()
                            && realm.state_root == selected.locator.state_root
                    }) {
                        writeln!(output, "That realm is already listed at this root. Choose its number, 'new', or /quit.")
                            .map_err(|e| ConnectError::native("explain existing realm", e))?;
                        entering_name = false;
                    } else {
                        return Ok(Some(selected));
                    }
                }
                Err(RuntimeBootstrapError::InvalidRealmId(_)) => {
                    writeln!(output, "That name is not valid. Use 1-64 ASCII letters, digits, '-' or '_', starting with a letter or digit. UUID-shaped names are reserved. Enter another name, or /quit.")
                        .map_err(|e| ConnectError::native("explain realm name", e))?;
                }
                Err(error) => return Err(ConnectError::native("validate realm name", error)),
            }
        } else if let Some(choice) = line
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| realms.get(index))
        {
            // Inventory identities are native-owned. Do not apply the stricter
            // new human-name entry rule or normalize a stored identity/root.
            return options.select(choice).map(Some);
        } else {
            writeln!(
                output,
                "Enter a number from 1 to {}, 'new', or /quit.",
                realms.len()
            )
            .map_err(|e| ConnectError::native("explain realm number", e))?;
        }
    }
}

/// Only a native committed result is printable as completed agent output.
/// Pending callbacks are not ordinary conversational questions or permission.
pub fn committed_text(outcome: &CompletionOutcome) -> Result<&str, &'static str> {
    match outcome {
        CompletionOutcome::Completed(result) => Ok(&result.text),
        CompletionOutcome::CompletedWithoutResult => Ok(""),
        CompletionOutcome::CallbackPending { .. }
        | CompletionOutcome::CallbackBatchPending { .. } => Err(
            "A native action is awaiting external fulfillment. This terminal cannot fulfill it; the conversation has stopped without replay.",
        ),
        CompletionOutcome::Cancelled => Err("Setup turn cancelled."),
        CompletionOutcome::Abandoned { .. } | CompletionOutcome::AbandonedWithError { .. } => {
            Err("The native setup turn failed. No automatic retry was started.")
        }
        CompletionOutcome::CompletedWithFinalizationFailure { .. } => Err(
            "Output was produced, but native durable finalization failed. It is not a committed result.",
        ),
        CompletionOutcome::RuntimeTerminated { .. } => {
            Err("The native runtime ended before the setup turn completed.")
        }
    }
}

/// Deliberately not Clone, Debug or Serialize. Only the protected finalizer
/// borrows its exact value for the native slot setter.
pub(crate) struct ServiceSecret(String);
impl ServiceSecret {
    pub(crate) fn from_hidden_input(value: String) -> io::Result<Self> {
        if value.is_empty() || value.len() > 16 * 1024 || value.contains('\0') {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "The hidden service credential is invalid"));
        }
        Ok(Self(value))
    }
    pub(crate) fn expose(&self) -> &str { &self.0 }
}

#[cfg(test)]
mod tests {
    #[test]
    fn review_notation_preserves_text_and_exposes_controls_and_direction_markers() {
        assert_eq!(
            super::review_text("read\n\tRäksmörgås\u{202e}hidden\u{001b}\r\\u{202E}"),
            "read\n\tRäksmörgås\\u{202E}hidden\\u{001B}\\u{000D}\\\\u{202E}"
        );
    }

    use super::*;

    #[tokio::test]
    async fn service_secret_entry_is_hidden_and_cancel_restores_the_same_input() {
        use futures_util::FutureExt;
        use nix::{pty::openpty,unistd::write};
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            let pair = openpty(None,None).unwrap();
            let observer = pair.slave.try_clone().unwrap();
            let original = tcgetattr(&observer).unwrap();
            let mut input = TerminalInput::from_owned_fd(pair.slave);
            let mut output = Vec::new();
            let value = {
                let mut reading = Box::pin(input.read_service_secret(&mut output));
                assert!(reading.as_mut().now_or_never().is_none());
                assert!(!tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO));
                write(&pair.master,b"synthetic-service-value\n").unwrap();
                reading.await.unwrap().unwrap()
            };
            assert_eq!(value.expose(),"synthetic-service-value");
            assert!(!String::from_utf8_lossy(&output).contains(value.expose()));
            assert_eq!(tcgetattr(&observer).unwrap(),original);
            input.check_cleanup().unwrap();
            {
                let mut reading = Box::pin(input.read_service_secret(&mut output));
                assert!(reading.as_mut().now_or_never().is_none());
                assert!(!tcgetattr(&observer).unwrap().local_flags.contains(LocalFlags::ECHO));
                drop(reading);
            }
            assert_eq!(tcgetattr(&observer).unwrap(),original);
            input.check_cleanup().unwrap();
            write(&pair.master,b"ordinary next input\n").unwrap();
            assert_eq!(input.recv().await.unwrap().unwrap(),"ordinary next input");
        }).await.expect("protected entry and input restoration must settle");
    }

    async fn retain_bytes(input: &mut TerminalInput, expected: &[u8]) {
        // Let the actual AsyncFd reader consume bytes without a complete line.
        // Cancelling it at each scheduler turn must preserve that partial read.
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                tokio::select! {
                    result = input.wait_for_end() => panic!("unexpected close: {result:?}"),
                    _ = tokio::task::yield_now() => (),
                }
                if input.pending == expected {
                    break;
                }
            }
        })
        .await
        .expect("input readiness was not observed");
    }

    #[tokio::test]
    async fn cancelled_eof_wait_preserves_partial_utf8_for_the_same_reader() {
        let (socket, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        writer.write_all(&[b'a', 0xc3]).unwrap();
        retain_bytes(&mut input, &[b'a', 0xc3]).await;
        input.check_cleanup().unwrap();
        writer.write_all(&[0xa4, b'b', b'\n']).unwrap();
        assert_eq!(input.recv().await.unwrap().unwrap(), "aäb");
        input.check_cleanup().unwrap();
    }

    #[tokio::test]
    async fn fresh_prompt_discards_retained_and_unread_type_ahead_without_poisoning_input() {
        let (socket, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        writer.write_all(b"approve unseen").unwrap();
        retain_bytes(&mut input, b"approve unseen").await;
        writer.write_all(b"\napprove another unseen\n").unwrap();
        assert!(input.fresh_prompt().unwrap());
        assert!(input.pending.is_empty());
        writer.write_all(b"deny visible\n").unwrap();
        assert_eq!(input.recv().await.unwrap().unwrap(), "deny visible");
        input.check_cleanup().unwrap();
    }

    #[tokio::test]
    async fn eof_is_latched_and_never_becomes_a_fresh_approval() {
        let (socket, mut writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let mut input = TerminalInput::from_owned_fd(socket.into());
        writer.write_all(b"ordinary unfinished input").unwrap();
        writer.shutdown(std::net::Shutdown::Write).unwrap();
        input.wait_for_end().await.unwrap();
        assert_eq!(
            input.recv().await.unwrap().unwrap(),
            "ordinary unfinished input"
        );
        assert!(input.recv().await.is_none());
        assert!(!input.fresh_prompt().unwrap());
        input.check_cleanup().unwrap();
    }

    fn options(root: &std::path::Path) -> RealmOptions {
        RealmOptions {
            context_root: root.to_path_buf(),
            state_root: Some(root.join("state")),
            user_config_root: Some(root.join("home")),
        }
    }

    fn lines(values: &[&str]) -> TerminalInput {
        let (sender, receiver) = mpsc::channel(values.len().max(1));
        for value in values {
            sender.try_send(Ok((*value).to_owned())).unwrap();
        }
        TerminalInput::scripted(receiver)
    }

    fn choices(root: &std::path::Path) -> Vec<RealmChoice> {
        // The same native identity at two roots must stay an explicit choice.
        // It is deliberately outside the stricter newly typed name grammar.
        ["one", "two"]
            .into_iter()
            .map(|name| RealmChoice {
                realm_id: "mob.household".into(),
                state_root: root.join(name),
                backend: meerkat_store::RealmBackend::Sqlite,
            })
            .collect()
    }

    fn untouched(root: &std::path::Path) {
        assert_eq!(
            std::fs::read_dir(root).unwrap().count(),
            0,
            "selection must not create native realms, config or setup artifacts"
        );
    }

    #[tokio::test]
    async fn invalid_numbers_reprompt_and_preserve_exact_stored_identity_and_root() {
        let directory = tempfile::tempdir().unwrap();
        let inventory = choices(directory.path());
        let mut input = lines(&[
            "",
            "family",
            "0",
            "3",
            "999999999999999999999999999999999",
            " 2 ",
        ]);
        let mut output = Vec::new();
        let selected = select_realm(
            &options(directory.path()),
            &inventory,
            &mut input,
            &mut output,
            std::future::pending(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(selected.locator.realm.as_str(), "mob.household");
        assert_eq!(selected.locator.state_root, inventory[1].state_root);
        let text = String::from_utf8(output).unwrap();
        assert_eq!(
            text.matches("Choose a realm number, 'new', or /quit: ")
                .count(),
            6
        );
        assert_eq!(
            text.matches("Enter a number from 1 to 2, 'new', or /quit.")
                .count(),
            5
        );
        untouched(directory.path());
    }

    #[tokio::test]
    async fn invalid_new_names_use_native_validation_and_allow_correction_without_materializing() {
        let directory = tempfile::tempdir().unwrap();
        let long = "a".repeat(65);
        let invalid = [
            "",
            "family chat",
            "../family",
            "mob.household",
            "-family",
            &long,
            "123e4567-e89b-12d3-a456-426614174000",
            " family ",
        ];
        for name in invalid {
            assert!(validate_explicit_realm_id(name).is_err());
        }
        for inventory in [Vec::new(), choices(directory.path())] {
            let mut entries = Vec::new();
            if !inventory.is_empty() {
                entries.push("new");
            }
            entries.extend(invalid);
            entries.push("Family_2");
            let mut input = lines(&entries);
            let mut output = Vec::new();
            let selected = select_realm(
                &options(directory.path()),
                &inventory,
                &mut input,
                &mut output,
                std::future::pending(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(selected.locator.realm.as_str(), "Family_2");
            assert_eq!(selected.locator.state_root, directory.path().join("state"));
            assert_eq!(
                String::from_utf8(output)
                    .unwrap()
                    .matches("That name is not valid.")
                    .count(),
                invalid.len()
            );
            untouched(directory.path());
        }
    }

    #[tokio::test]
    async fn new_name_collision_returns_to_choices_without_conflating_other_roots() {
        let directory = tempfile::tempdir().unwrap();
        for same_root in [true, false] {
            let inventory = vec![RealmChoice {
                realm_id: "family".into(),
                state_root: directory
                    .path()
                    .join(if same_root { "state" } else { "other" }),
                backend: meerkat_store::RealmBackend::Sqlite,
            }];
            let mut entries = vec!["new", "family"];
            if same_root {
                entries.push("1");
            }
            let mut output = Vec::new();
            let selected = select_realm(
                &options(directory.path()),
                &inventory,
                &mut lines(&entries),
                &mut output,
                std::future::pending(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(selected.locator.realm.as_str(), "family");
            assert_eq!(selected.locator.state_root, directory.path().join("state"));
            assert_eq!(
                String::from_utf8(output)
                    .unwrap()
                    .contains("That realm is already listed at this root."),
                same_root,
            );
            untouched(directory.path());
        }
    }

    #[tokio::test]
    async fn quit_eof_and_cancellation_leave_both_selection_modes_unmaterialized() {
        let directory = tempfile::tempdir().unwrap();
        for inventory in [Vec::new(), choices(directory.path())] {
            for entries in [&["bad choice", " /quit "][..], &["bad choice"][..], &[][..]] {
                let mut output = Vec::new();
                assert!(
                    select_realm(
                        &options(directory.path()),
                        &inventory,
                        &mut lines(entries),
                        &mut output,
                        std::future::pending()
                    )
                    .await
                    .unwrap()
                    .is_none()
                );
                untouched(directory.path());
            }
            // Cancellation wins even if a valid line is already queued. No
            // native root resolution or accidental selection follows Ctrl+C.
            let mut input = lines(&[if inventory.is_empty() { "family" } else { "1" }]);
            assert!(
                select_realm(
                    &options(directory.path()),
                    &inventory,
                    &mut input,
                    &mut Vec::new(),
                    async { Ok(()) }
                )
                .await
                .unwrap()
                .is_none()
            );
            assert!(
                input.try_recv().is_ok(),
                "cancelled selection consumed buffered input"
            );
            untouched(directory.path());
        }
    }

    #[tokio::test]
    async fn native_root_probe_error_is_fatal_and_does_not_consume_another_choice() {
        let directory = tempfile::tempdir().unwrap();
        let rkat = directory.path().join(".rkat");
        std::fs::create_dir(&rkat).unwrap();
        std::fs::write(rkat.join("realms"), "a file cannot be a realm root").unwrap();
        let mut selection = options(directory.path());
        selection.state_root = None;
        let mut input = lines(&["family", "another"]);
        let mut output = Vec::new();
        let error = select_realm(
            &selection,
            &[],
            &mut input,
            &mut output,
            std::future::pending(),
        )
        .await
        .err()
        .expect("native probe failure");
        let ConnectError::Native { source, .. } = error else {
            panic!("native failure must remain typed")
        };
        assert!(matches!(
            source.downcast_ref::<RuntimeBootstrapError>(),
            Some(RuntimeBootstrapError::RootProbeFailed { .. })
        ));
        assert_eq!(input.try_recv().unwrap().unwrap(), "another");
        assert!(
            !String::from_utf8(output)
                .unwrap()
                .contains("That name is not valid.")
        );
        assert!(rkat.join("realms").is_file());
        assert!(!directory.path().join("state").exists());
        assert!(!directory.path().join("home").exists());
    }

    #[tokio::test]
    async fn input_and_cancellation_listener_errors_are_fatal_not_reprompted() {
        let directory = tempfile::tempdir().unwrap();
        let (sender, receiver) = mpsc::channel(2);
        let mut input = TerminalInput::scripted(receiver);
        sender
            .try_send(Err(io::Error::other("input failed")))
            .unwrap();
        sender.try_send(Ok("family".into())).unwrap();
        let error = select_realm(
            &options(directory.path()),
            &[],
            &mut input,
            &mut Vec::new(),
            std::future::pending(),
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            error,
            ConnectError::Native {
                operation: "read realm selection",
                ..
            }
        ));
        assert_eq!(input.try_recv().unwrap().unwrap(), "family");
        let error = select_realm(
            &options(directory.path()),
            &[],
            &mut lines(&["family"]),
            &mut Vec::new(),
            async { Err(io::Error::other("signal listener failed")) },
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(
            error,
            ConnectError::Native {
                operation: "listen for cancellation",
                ..
            }
        ));
        untouched(directory.path());
    }

    #[test]
    fn terminal_control_bytes_cannot_change_the_screen_or_title() {
        assert_eq!(
            display_text("safe\x1b]0;fake\x07\r\nnext\tline"),
            "safe[ESC]]0;fake\nnext\tline"
        );
    }

    #[test]
    fn native_callback_is_never_presented_as_a_completed_question() {
        let outcome = CompletionOutcome::CallbackBatchPending {
            pending_tool_calls: Vec::new(),
        };
        assert!(committed_text(&outcome).is_err());
        assert!(committed_text(&CompletionOutcome::Cancelled).is_err());
    }
}

/// Whether a failed attempt has the exact committed input witness needed to
/// offer a new request. This never turns failed output into a successful result.
pub fn failed_turn_allows_new_input(
    outcome: &CompletionOutcome,
    input_terminal: Option<&meerkat_runtime::input_state::InputTerminalOutcome>,
    exit_requested: bool,
) -> bool {
    !exit_requested
        && matches!(
            outcome,
            CompletionOutcome::Abandoned { .. } | CompletionOutcome::AbandonedWithError { .. }
        )
        && matches!(
            input_terminal,
            Some(
                meerkat_runtime::input_state::InputTerminalOutcome::Consumed
                    | meerkat_runtime::input_state::InputTerminalOutcome::Abandoned { .. }
            )
        )
}
