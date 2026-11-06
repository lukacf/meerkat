//! In-crate file tools: `read_file`, `write_file`, `edit_file`, `glob`, `grep`.
//!
//! [`FileToolDispatcher`] implements [`AgentToolDispatcher`] and is injected
//! into sessions as an external tool source. It is deliberately policy-free:
//! out-of-project paths are not refused here (the approval layer decides), and
//! the only hard refusals are writing through a symlink and writing a file
//! with more than one hard link. Writes replace the file (a temporary file
//! renamed over it), so a shared inode is never written.
//!
//! Every model-supplied path goes through [`ProjectRoot::resolve`] so the tools
//! and the approval policy agree on what a path means.
//!
//! Failure convention: problems the model can recover from (file not found,
//! no match, stale file, invalid pattern) become `Ok(ToolResult { is_error:
//! true, .. })`; malformed arguments and internal failures become
//! `Err(ToolError)`.

mod diff;
mod edit;
mod glob;
mod grep;
mod read;
mod read_only;
mod write;

pub use diff::FileDiff;
pub use grep::OutputMode;
pub use read_only::{READ_ONLY_TOOL_NAMES, ReadOnlyProjectTools};

use crate::paths::{PathError, ProjectRoot, ResolvedPath};
use async_trait::async_trait;
use meerkat_core::ops::ToolDispatchOutcome;
use meerkat_core::{
    AgentToolDispatcher, SessionId, ToolCallView, ToolDef, ToolDispatchContext, ToolError,
    ToolMutationClass, ToolResult,
};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;

/// Tool name for [`read`].
pub const READ_FILE: &str = "read_file";

/// Provenance source id of the file tools. Stable across builds: a helper's
/// inherited tool ceiling (a mob `delegate` with `inherit_parent` tooling)
/// matches its parent's tools by provenance, so every build tags them alike.
pub const FILE_TOOLS_SOURCE: &str = "rkat-tui.files";

/// Provenance of a TUI-owned tool: a host callback surface named `source`.
#[must_use]
pub fn provenance(source: &str) -> meerkat_core::ToolProvenance {
    meerkat_core::ToolProvenance::new(
        meerkat_core::ToolSourceKind::Callback,
        meerkat_core::ToolSourceId::new(source),
    )
}
/// Tool name for [`write`].
pub const WRITE_FILE: &str = "write_file";
/// Tool name for [`edit`].
pub const EDIT_FILE: &str = "edit_file";
/// Tool name for [`glob`].
pub const GLOB: &str = "glob";
/// Tool name for [`grep`].
pub const GREP: &str = "grep";

/// Names of every tool exposed by [`FileToolDispatcher`].
pub const FILE_TOOL_NAMES: &[&str] = &[READ_FILE, WRITE_FILE, EDIT_FILE, GLOB, GREP];

/// Number of leading bytes inspected for a NUL byte when detecting binary files.
pub(crate) const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Most bytes any file tool reads from one file. `read_file` and `grep`
/// see at most this prefix; `write_file`/`edit_file` refuse to rewrite a
/// larger file (rewriting a prefix would drop its tail).
pub const MAX_FILE_READ_BYTES: u64 = 16 * 1024 * 1024;

/// A file's bytes read through a cap.
pub(crate) struct CappedRead {
    pub(crate) bytes: Vec<u8>,
    /// The file held more than the cap; `bytes` is its first `cap` bytes.
    pub(crate) truncated: bool,
}

/// Why a path could not be read as a regular file.
#[derive(Debug)]
pub(crate) enum ReadRefusal {
    /// Not a regular file (directory, FIFO, socket, device, ...).
    NotRegular(std::fs::FileType),
    Io(io::Error),
}

impl From<io::Error> for ReadRefusal {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl ReadRefusal {
    /// Describe the refusal in model-facing terms.
    pub(crate) fn into_failure(self, action: &str, display: &str) -> ToolFailure {
        match self {
            Self::NotRegular(kind) if kind.is_dir() => {
                ToolFailure::recoverable(format!("{display} is a directory, not a file"))
            }
            Self::NotRegular(kind) => ToolFailure::recoverable(format!(
                "{display} is not a regular file ({}); only regular files can be read",
                file_kind(kind)
            )),
            Self::Io(err) => ToolFailure::io(action, display, &err),
        }
    }
}

/// A short name for a non-regular file type.
pub(crate) fn file_kind(kind: std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt as _;
        if kind.is_fifo() {
            return "a named pipe";
        }
        if kind.is_socket() {
            return "a socket";
        }
        if kind.is_char_device() || kind.is_block_device() {
            return "a device";
        }
    }
    if kind.is_dir() {
        "a directory"
    } else if kind.is_symlink() {
        "a symlink"
    } else {
        "a special file"
    }
}

/// Read at most `cap` bytes of the regular file at `path`.
///
/// The path is checked with `metadata` before it is opened (opening a FIFO
/// blocks until a writer appears, and a device may never end), the open
/// handle is checked again (the path may have been swapped in between),
/// and the read goes through [`io::Read::take`], so a file growing after
/// the check still yields at most `cap` bytes.
pub(crate) fn read_capped(path: &Path, cap: u64) -> Result<CappedRead, ReadRefusal> {
    let meta = std::fs::metadata(path)?;
    if !meta.is_file() {
        return Err(ReadRefusal::NotRegular(meta.file_type()));
    }
    let file = std::fs::File::open(path)?;
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Err(ReadRefusal::NotRegular(opened.file_type()));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len().min(cap)).unwrap_or(0));
    file.take(cap.saturating_add(1)).read_to_end(&mut bytes)?;
    let limit = usize::try_from(cap).unwrap_or(usize::MAX);
    let truncated = bytes.len() > limit;
    bytes.truncate(limit);
    Ok(CappedRead { bytes, truncated })
}

/// Why a tool invocation did not succeed.
#[derive(Debug)]
pub(crate) enum ToolFailure {
    /// A problem the model can recover from; surfaced as an error tool result.
    Recoverable(String),
    /// Malformed arguments or an internal failure; surfaced as `Err(ToolError)`.
    Fatal(ToolError),
}

impl ToolFailure {
    pub(crate) fn recoverable(message: impl Into<String>) -> Self {
        Self::Recoverable(message.into())
    }

    /// Describe an I/O error on `display` in model-facing terms.
    pub(crate) fn io(action: &str, display: &str, err: &io::Error) -> Self {
        let message = match err.kind() {
            io::ErrorKind::NotFound => format!("file not found: {display}"),
            io::ErrorKind::PermissionDenied => {
                format!("permission denied while trying to {action} {display}")
            }
            _ => format!("failed to {action} {display}: {err}"),
        };
        Self::Recoverable(message)
    }
}

pub(crate) type ToolOutcome = Result<String, ToolFailure>;

/// Who made a tool call: the call id and the session it belongs to.
#[derive(Debug, Clone)]
pub(crate) struct CallMeta {
    pub(crate) id: String,
    /// The calling session, from the dispatch context
    /// ([`ToolDispatchContext::origin_session_id`]), else the dispatcher's
    /// owning session, else `None` (a context-free call).
    pub(crate) session: Option<SessionId>,
}

/// Key of the read-before-write record: the session that read, and the path.
type ReadKey = (Option<SessionId>, PathBuf);

/// State shared between the dispatcher and its blocking workers.
pub(crate) struct Shared {
    pub(crate) root: ProjectRoot,
    /// (session, absolute path) → mtime observed when that session last
    /// read or wrote the file. Keyed by session so one session's (or
    /// helper's) read never satisfies another session's write check.
    pub(crate) read_state: Mutex<HashMap<ReadKey, SystemTime>>,
    pub(crate) diff_tx: Option<mpsc::UnboundedSender<FileDiff>>,
    /// Skip environment files ([`crate::paths::is_secret_file_name`]) when
    /// walking for `glob`/`grep` (the classifier's read-only tools).
    pub(crate) hide_secrets: bool,
}

impl Shared {
    /// Resolve a model-supplied path against the project root.
    pub(crate) fn resolve(&self, tool: &str, raw: &str) -> Result<ResolvedPath, ToolFailure> {
        self.root.resolve(raw).map_err(|err| match err {
            PathError::Empty | PathError::NulByte => {
                ToolFailure::Fatal(ToolError::InvalidArguments {
                    name: tool.to_string(),
                    reason: format!("invalid path: {err}"),
                })
            }
            PathError::NoHome | PathError::Io { .. } => ToolFailure::recoverable(err.to_string()),
        })
    }

    /// Display a resolved path relative to the root when possible.
    pub(crate) fn display(&self, resolved: &ResolvedPath) -> String {
        self.root.display_of(resolved).to_string()
    }

    /// Display an absolute path relative to the root when it lies inside.
    pub(crate) fn display_abs(&self, abs: &Path) -> String {
        abs.strip_prefix(self.root.as_path())
            .unwrap_or(abs)
            .display()
            .to_string()
    }

    pub(crate) fn record_read(&self, session: Option<SessionId>, abs: PathBuf, mtime: SystemTime) {
        self.read_state.lock().insert((session, abs), mtime);
    }

    /// Forward a diff to the UI. Delivery is best-effort: a closed receiver
    /// only means nobody is rendering diffs, which must not fail the tool.
    pub(crate) fn emit_diff(&self, diff: FileDiff) {
        if let Some(tx) = &self.diff_tx
            && tx.send(diff).is_err()
        {
            tracing::debug!("file diff receiver closed; dropping diff");
        }
    }

    /// Verify that an existing file may be overwritten: the final component is
    /// not a symlink, it is a regular file with no other hard link (on Unix),
    /// and it was read (and not changed
    /// since) in this session.
    ///
    /// Returns `Ok(None)` when the target does not exist yet, and the current
    /// content when it does.
    pub(crate) fn check_writable(
        &self,
        session: Option<&SessionId>,
        raw: &str,
        resolved: &ResolvedPath,
    ) -> Result<Option<String>, ToolFailure> {
        let display = self.display(resolved);
        let literal = self.literal_path(raw);
        for candidate in [literal.as_deref(), Some(resolved.abs.as_path())]
            .into_iter()
            .flatten()
        {
            if let Ok(meta) = std::fs::symlink_metadata(candidate)
                && meta.file_type().is_symlink()
            {
                return Err(ToolFailure::recoverable(format!(
                    "refusing to write through symlink {display}; edit the link target directly"
                )));
            }
        }

        let meta = match std::fs::metadata(&resolved.abs) {
            Ok(meta) => meta,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(ToolFailure::io("inspect", &display, &err)),
        };
        if !meta.is_file() {
            return Err(ReadRefusal::NotRegular(meta.file_type()).into_failure("write", &display));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            if meta.nlink() > 1 {
                return Err(ToolFailure::recoverable(format!(
                    "refusing to write {display}: it is a hard link shared with {} other path(s), \
                     which a write would change as well; edit the file at its own path",
                    meta.nlink() - 1
                )));
            }
        }
        let current = meta
            .modified()
            .map_err(|err| ToolFailure::io("read the modification time of", &display, &err))?;
        let recorded = self
            .read_state
            .lock()
            .get(&(session.cloned(), resolved.abs.clone()))
            .copied();
        match recorded {
            None => {
                return Err(ToolFailure::recoverable(format!(
                    "{display} has not been read in this session; read it with read_file before modifying it"
                )));
            }
            Some(seen) if seen != current => {
                return Err(ToolFailure::recoverable(format!(
                    "{display} has been modified since it was last read (by the user or another process); read it again with read_file before modifying it"
                )));
            }
            Some(_) => {}
        }

        let read = read_capped(&resolved.abs, MAX_FILE_READ_BYTES)
            .map_err(|refusal| refusal.into_failure("read", &display))?;
        if read.truncated {
            return Err(ToolFailure::recoverable(format!(
                "{display} is larger than {} MiB; the file tools cannot rewrite it",
                MAX_FILE_READ_BYTES / (1024 * 1024)
            )));
        }
        String::from_utf8(read.bytes)
            .map(Some)
            .map_err(|_| ToolFailure::recoverable(format!("{display} is not valid UTF-8 text")))
    }

    /// Write `content` to an already-checked path, record its new mtime, and
    /// emit a diff against `old`.
    pub(crate) fn commit_write(
        &self,
        call: CallMeta,
        resolved: &ResolvedPath,
        old: &str,
        content: &str,
    ) -> Result<FileDiff, ToolFailure> {
        let display = self.display(resolved);
        // A temporary file renamed over the target: a hard link created
        // after `check_writable` keeps the old content, never this write.
        crate::paths::write_atomic(
            &resolved.abs,
            content.as_bytes(),
            crate::paths::FileMode::Preserve,
            None,
        )
        .map_err(|err| match err {
            crate::paths::AtomicWriteError::Io { op, source, .. } => {
                ToolFailure::io(op, &display, &source)
            }
            other => ToolFailure::recoverable(other.to_string()),
        })?;
        let mtime = std::fs::metadata(&resolved.abs)
            .and_then(|meta| meta.modified())
            .map_err(|err| ToolFailure::io("read the modification time of", &display, &err))?;
        self.record_read(call.session, resolved.abs.clone(), mtime);
        let diff = FileDiff::new(call.id, PathBuf::from(&display), old, content);
        self.emit_diff(diff.clone());
        Ok(diff)
    }

    /// The model-supplied path joined onto the root *without* resolving
    /// symlinks, so the final component can be inspected with `lstat`.
    fn literal_path(&self, raw: &str) -> Option<PathBuf> {
        let expanded = if raw == "~" {
            dirs::home_dir()?
        } else if let Some(rest) = raw.strip_prefix("~/") {
            dirs::home_dir()?.join(rest)
        } else {
            PathBuf::from(raw)
        };
        Some(if expanded.is_absolute() {
            expanded
        } else {
            self.root.as_path().join(expanded)
        })
    }
}

/// Whether `bytes` looks binary (a NUL byte within the first 8 KiB).
pub(crate) fn looks_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0)
}

/// Truncate `line` to at most `max_chars` characters, appending a marker.
pub(crate) fn truncate_line(line: &str, max_chars: usize) -> std::borrow::Cow<'_, str> {
    match line.char_indices().nth(max_chars) {
        None => std::borrow::Cow::Borrowed(line),
        Some((cut, _)) => {
            let total = line.chars().count();
            std::borrow::Cow::Owned(format!("{}… [line truncated: {total} chars]", &line[..cut]))
        }
    }
}

/// Dispatcher for the in-crate file tools.
pub struct FileToolDispatcher {
    shared: Arc<Shared>,
    tools: Arc<[Arc<ToolDef>]>,
    /// The session this dispatcher serves, when it serves exactly one: the
    /// read-state key of calls whose dispatch context names no session.
    owner: Option<SessionId>,
}

impl FileToolDispatcher {
    /// Create a dispatcher rooted at `root`. Diffs produced by `write_file` and
    /// `edit_file` are sent to `diff_tx` when provided.
    ///
    /// The read-before-write record is kept per calling session: a call
    /// dispatched with a [`ToolDispatchContext`] naming its session
    /// ([`ToolDispatchContext::origin_session_id`], which the agent loop
    /// binds for every turn) is checked against that session's reads only.
    /// Calls without one share a single anonymous record; use
    /// [`Self::for_session`] when the dispatcher serves one known session.
    pub fn new(root: ProjectRoot, diff_tx: Option<mpsc::UnboundedSender<FileDiff>>) -> Self {
        Self::build(root, diff_tx, false)
    }

    /// A dispatcher owned by `session`: calls whose dispatch context names no
    /// session are recorded and checked as `session`'s. A context that names
    /// a session still wins.
    #[must_use]
    pub fn for_session(
        root: ProjectRoot,
        diff_tx: Option<mpsc::UnboundedSender<FileDiff>>,
        session: SessionId,
    ) -> Self {
        let mut dispatcher = Self::build(root, diff_tx, false);
        dispatcher.owner = Some(session);
        dispatcher
    }

    /// A dispatcher whose `glob`/`grep` never list or search environment
    /// files (for the classifier; `read_file` refusals are the caller's job).
    pub(crate) fn hiding_secrets(root: ProjectRoot) -> Self {
        Self::build(root, None, true)
    }

    fn build(
        root: ProjectRoot,
        diff_tx: Option<mpsc::UnboundedSender<FileDiff>>,
        hide_secrets: bool,
    ) -> Self {
        let tools: Arc<[Arc<ToolDef>]> = [
            read::definition(),
            write::definition(),
            edit::definition(),
            glob::definition(),
            grep::definition(),
        ]
        .into_iter()
        .map(|def| Arc::new(def.with_provenance(provenance(FILE_TOOLS_SOURCE))))
        .collect();
        Self {
            shared: Arc::new(Shared {
                root,
                read_state: Mutex::new(HashMap::new()),
                diff_tx,
                hide_secrets,
            }),
            tools,
            owner: None,
        }
    }

    /// The project root the tools resolve paths against.
    pub fn root(&self) -> &ProjectRoot {
        &self.shared.root
    }

    async fn run_blocking<A, F>(
        &self,
        call: ToolCallView<'_>,
        session: Option<SessionId>,
        run: F,
    ) -> Result<ToolResult, ToolError>
    where
        A: DeserializeOwned + Send + 'static,
        F: FnOnce(&Shared, CallMeta, A) -> ToolOutcome + Send + 'static,
    {
        let args: A = call
            .parse_args()
            .map_err(|err| ToolError::InvalidArguments {
                name: call.name.to_string(),
                reason: err.to_string(),
            })?;
        let shared = Arc::clone(&self.shared);
        let id = call.id.to_string();
        let meta = CallMeta {
            id: id.clone(),
            session,
        };
        let outcome = tokio::task::spawn_blocking(move || run(&shared, meta, args))
            .await
            .map_err(|err| ToolError::ExecutionFailed {
                message: format!("{} worker failed: {err}", call.name),
            })?;
        match outcome {
            Ok(text) => Ok(ToolResult::new(id, text, false)),
            Err(ToolFailure::Recoverable(message)) => Ok(ToolResult::new(id, message, true)),
            Err(ToolFailure::Fatal(err)) => Err(err),
        }
    }
}

impl std::fmt::Debug for FileToolDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileToolDispatcher")
            .field("root", &self.shared.root)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for FileToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    fn tool_mutation_class(&self, tool_name: &str) -> ToolMutationClass {
        match tool_name {
            READ_FILE | GLOB | GREP => ToolMutationClass::ReadOnly,
            WRITE_FILE | EDIT_FILE => ToolMutationClass::Mutating,
            _ => ToolMutationClass::Unknown,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_as(call, self.owner.clone()).await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let session = context
            .origin_session_id()
            .cloned()
            .or_else(|| self.owner.clone());
        self.dispatch_as(call, session).await
    }
}

impl FileToolDispatcher {
    /// Run `call` with `session` as the read-state key.
    async fn dispatch_as(
        &self,
        call: ToolCallView<'_>,
        session: Option<SessionId>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let result = match call.name {
            READ_FILE => self.run_blocking(call, session, read::run).await,
            WRITE_FILE => self.run_blocking(call, session, write::run).await,
            EDIT_FILE => self.run_blocking(call, session, edit::run).await,
            GLOB => self.run_blocking(call, session, glob::run).await,
            GREP => self.run_blocking(call, session, grep::run).await,
            other => Err(ToolError::NotFound {
                name: other.to_string(),
            }),
        };
        result.map(ToolDispatchOutcome::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_line_keeps_short_lines() {
        assert_eq!(truncate_line("abc", 5), "abc");
        assert_eq!(truncate_line("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_line_marks_long_lines() {
        let out = truncate_line("abcdefg", 3);
        assert!(out.starts_with("abc… [line truncated: 7 chars]"));
    }

    #[test]
    fn truncate_line_respects_char_boundaries() {
        let out = truncate_line("ééééé", 2);
        assert!(out.starts_with("éé…"));
    }

    #[test]
    fn binary_detection_only_sniffs_prefix() {
        assert!(looks_binary(b"ab\0cd"));
        assert!(!looks_binary(b"plain text"));
        let mut late = vec![b'a'; BINARY_SNIFF_BYTES];
        late.push(0);
        assert!(!looks_binary(&late));
    }
}
