//! ApplyPatch tool for structured file edits within the project root.

use super::protected_paths::ProtectedPaths;
use crate::builtin::{BuiltinTool, BuiltinToolError, LeafEntry, ToolOutput};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::types::{ToolProvenance, ToolSourceKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ApplyPatchTool {
    project_root: PathBuf,
    /// The paths this tool never writes: the engine's own directory under
    /// the project root, and what the host declared as its own.
    protected: ProtectedPaths,
}

impl ApplyPatchTool {
    /// The tool rooted at `project_root`. It never writes
    /// `<project_root>/.rkat`.
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            protected: ProtectedPaths::default(),
        }
    }

    /// The tool with the host's declared paths protected as well.
    pub(crate) fn protecting(mut self, protected: ProtectedPaths) -> Self {
        self.protected = protected;
        self
    }
}

/// Model-facing documentation of the patch grammar. This is the single copy:
/// the `patch` argument schema points here instead of repeating it.
const APPLY_PATCH_DESCRIPTION: &str = "Apply a patch that adds, deletes, updates, or moves files. Paths are relative to the project root; absolute paths and paths that escape the root via '..' are rejected.

The patch begins with '*** Begin Patch' and ends with '*** End Patch', each on its own line and used once, with one or more file operations between them:
- '*** Add File: <path>' followed by the file content, every line prefixed with '+'.
- '*** Delete File: <path>' with no body.
- '*** Update File: <path>', optionally followed by '*** Move to: <new_path>', then one or more hunks. A move still needs at least one hunk.

Hunks:
- A hunk starts with '@@ <anchor>', where <anchor> is the literal text of a line in the file (never a line number). The next line equal to <anchor> is found and the hunk applies to the lines after it. A bare '@@' has no anchor: the hunk lines are searched from the current position (the top of the file for the first hunk).
- Hunk lines start with ' ' (unchanged context; the space is required), '-' (remove) or '+' (add). A blank line inside a hunk is an empty context line.
- Context and '-' lines must match consecutive file lines. Trailing whitespace and Unicode punctuation such as smart quotes are normalized; anything else must match exactly or the patch is rejected.
- Hunks apply top to bottom; each search starts after the previous hunk. Put '*** End of File' after a hunk's last line to drop the file's trailing newline.

Example (update, move, add, delete):
";

/// Documented example; kept separate so a test can apply it verbatim.
const APPLY_PATCH_EXAMPLE: &str = "*** Begin Patch
*** Update File: src/config.rs
@@ fn defaults() {
-    let timeout = 30;
+    let timeout = 60;
     let retries = 3;
*** Update File: src/old_name.rs
*** Move to: src/new_name.rs
@@
-fn old_func() {}
+fn new_func() {}
*** Add File: src/utils/helper.py
+def greet(name):
+    return f\"Hello, {name}\"
*** Delete File: src/deprecated.rs
*** End Patch";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ApplyPatchArgs {
    #[schemars(
        description = "The complete patch text in the format described by the tool description."
    )]
    patch: String,
}

#[derive(Debug, Error)]
enum ApplyPatchError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error("{0}")]
    InvalidPath(String),
    #[error("{0}")]
    ComputeReplacements(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
    /// The native entry step before the first file change refused.
    #[error(transparent)]
    Entry(BuiltinToolError),
    /// A path of the patch is one the file tools never write. The text is
    /// the model-facing reason.
    #[error("{0}")]
    ProtectedPath(String),
}

#[derive(Debug, Error, PartialEq, Clone)]
enum ParseError {
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    #[error("invalid hunk at line {line_number}, {message}")]
    InvalidHunk { message: String, line_number: usize },
}

#[derive(Debug, PartialEq, Clone)]
enum Hunk {
    Add {
        path: PathBuf,
        contents: String,
    },
    Delete {
        path: PathBuf,
    },
    Update {
        path: PathBuf,
        move_path: Option<PathBuf>,
        chunks: Vec<UpdateFileChunk>,
    },
}

#[derive(Debug, PartialEq, Clone)]
struct UpdateFileChunk {
    change_context: Option<String>,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    is_end_of_file: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct AffectedPaths {
    added: Vec<PathBuf>,
    modified: Vec<PathBuf>,
    deleted: Vec<PathBuf>,
}

const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl BuiltinTool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            audience: Default::default(),
            name: self.name().into(),
            description: format!("{APPLY_PATCH_DESCRIPTION}{APPLY_PATCH_EXAMPLE}"),
            input_schema: crate::schema::schema_for::<ApplyPatchArgs>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Builtin,
                source_id: "builtin".into(),
                artifact: None,
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    /// Writes files.
    fn mutation_class(&self) -> meerkat_core::ToolMutationClass {
        meerkat_core::ToolMutationClass::Mutating
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        self.call_entering(args, LeafEntry::none()).await
    }

    async fn call_with_context(
        &self,
        call: meerkat_core::ToolCallView<'_>,
        args: Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolOutput, BuiltinToolError> {
        self.call_entering(args, LeafEntry::for_call(context, call)?)
            .await
    }

    fn review_entry_support(&self) -> meerkat_core::approval::review::ReviewEntrySupport {
        meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
    }
}

impl ApplyPatchTool {
    /// The owned entry moves into the queued blocking worker and is spent
    /// there, after the worker started and resolved the root and the patch,
    /// immediately before the first file change. A dispatch that ended before
    /// the worker resumed has closed the entry, so the worker refuses.
    async fn call_entering(
        &self,
        args: Value,
        entry: LeafEntry,
    ) -> Result<ToolOutput, BuiltinToolError> {
        let args: ApplyPatchArgs = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid arguments: {e}")))?;
        let project_root = self.project_root.clone();
        let protected = self.protected.clone();
        let patch = args.patch;
        let affected = tokio::task::spawn_blocking(move || {
            let mut entry = entry;
            apply_patch(&project_root, &protected, &patch, &mut || {
                entry.enter().map_err(ApplyPatchError::Entry)
            })
        })
        .await
        .map_err(|e| BuiltinToolError::execution_failed(format!("apply_patch task failed: {e}")))?
        .map_err(|e| match e {
            ApplyPatchError::Entry(error) => error,
            ApplyPatchError::ProtectedPath(reason) => BuiltinToolError::invalid_args(reason),
            other => BuiltinToolError::execution_failed(other.to_string()),
        })?;

        Ok(ToolOutput::Json(json!({
            "status": "success",
            "added_files": affected
                .added
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "modified_files": affected
                .modified
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            "deleted_files": affected
                .deleted
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
        })))
    }
}

/// Enter callback run before the first file change of a patch.
type PatchEntry<'a> = &'a mut dyn FnMut() -> Result<(), ApplyPatchError>;

fn apply_patch(
    project_root: &Path,
    protected: &ProtectedPaths,
    patch: &str,
    enter: PatchEntry<'_>,
) -> Result<AffectedPaths, ApplyPatchError> {
    let canonical_root =
        std::fs::canonicalize(project_root).map_err(|source| ApplyPatchError::Io {
            context: format!("failed to resolve project root {}", project_root.display()),
            source,
        })?;
    let hunks = parse_patch(patch)?;
    apply_hunks_to_files(&canonical_root, protected, &hunks, enter)
}

fn parse_patch(patch: &str) -> Result<Vec<Hunk>, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    check_patch_boundaries(&lines)?;

    let last_line_index = lines.len().saturating_sub(1);
    let mut remaining_lines = &lines[1..last_line_index];
    let mut line_number = 2usize;
    let mut hunks = Vec::new();
    while !remaining_lines.is_empty() {
        let (hunk, consumed_lines) = parse_one_hunk(remaining_lines, line_number)?;
        hunks.push(hunk);
        line_number += consumed_lines;
        remaining_lines = &remaining_lines[consumed_lines..];
    }
    Ok(hunks)
}

fn check_patch_boundaries(lines: &[&str]) -> Result<(), ParseError> {
    let first_line = lines.first().map(|line| line.trim());
    let last_line = lines.last().map(|line| line.trim());

    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => {
            Ok(())
        }
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(ParseError::InvalidPatch(
            "The first line of the patch must be '*** Begin Patch'".to_string(),
        )),
        _ => Err(ParseError::InvalidPatch(
            "The last line of the patch must be '*** End Patch'".to_string(),
        )),
    }
}

fn parse_one_hunk(lines: &[&str], line_number: usize) -> Result<(Hunk, usize), ParseError> {
    let Some(first_line) = lines.first().map(|line| line.trim()) else {
        return Err(ParseError::InvalidPatch(
            "patch must contain at least one hunk".to_string(),
        ));
    };

    if let Some(path) = first_line.strip_prefix(ADD_FILE_MARKER) {
        let mut contents = String::new();
        let mut parsed_lines = 1;
        for add_line in &lines[1..] {
            if let Some(line_to_add) = add_line.strip_prefix('+') {
                contents.push_str(line_to_add);
                contents.push('\n');
                parsed_lines += 1;
            } else {
                break;
            }
        }
        return Ok((
            Hunk::Add {
                path: PathBuf::from(path),
                contents,
            },
            parsed_lines,
        ));
    }

    if let Some(path) = first_line.strip_prefix(DELETE_FILE_MARKER) {
        return Ok((
            Hunk::Delete {
                path: PathBuf::from(path),
            },
            1,
        ));
    }

    if let Some(path) = first_line.strip_prefix(UPDATE_FILE_MARKER) {
        let mut remaining_lines = &lines[1..];
        let mut parsed_lines = 1;
        let move_path = remaining_lines
            .first()
            .and_then(|line| line.strip_prefix(MOVE_TO_MARKER))
            .map(PathBuf::from);
        if move_path.is_some() {
            remaining_lines = &remaining_lines[1..];
            parsed_lines += 1;
        }

        let mut chunks = Vec::new();
        while !remaining_lines.is_empty() {
            if remaining_lines[0].trim().is_empty() {
                parsed_lines += 1;
                remaining_lines = &remaining_lines[1..];
                continue;
            }
            if remaining_lines[0].starts_with("***") {
                break;
            }

            let (chunk, chunk_lines) = parse_update_file_chunk(
                remaining_lines,
                line_number + parsed_lines,
                chunks.is_empty(),
            )?;
            chunks.push(chunk);
            parsed_lines += chunk_lines;
            remaining_lines = &remaining_lines[chunk_lines..];
        }

        if chunks.is_empty() {
            return Err(ParseError::InvalidHunk {
                message: format!("Update file hunk for path '{path}' is empty"),
                line_number,
            });
        }

        return Ok((
            Hunk::Update {
                path: PathBuf::from(path),
                move_path,
                chunks,
            },
            parsed_lines,
        ));
    }

    Err(ParseError::InvalidHunk {
        message: format!(
            "'{first_line}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
        ),
        line_number,
    })
}

fn parse_update_file_chunk(
    lines: &[&str],
    line_number: usize,
    allow_missing_context: bool,
) -> Result<(UpdateFileChunk, usize), ParseError> {
    if lines.is_empty() {
        return Err(ParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_string(),
            line_number,
        });
    }

    let (change_context, start_index) = if lines[0] == EMPTY_CHANGE_CONTEXT_MARKER {
        (None, 1)
    } else if let Some(context) = lines[0].strip_prefix(CHANGE_CONTEXT_MARKER) {
        (Some(context.to_string()), 1)
    } else if allow_missing_context {
        (None, 0)
    } else {
        return Err(ParseError::InvalidHunk {
            message: format!(
                "Expected update hunk to start with a @@ context marker, got: '{}'",
                lines[0]
            ),
            line_number,
        });
    };

    if start_index >= lines.len() {
        return Err(ParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_string(),
            line_number: line_number + 1,
        });
    }

    let mut chunk = UpdateFileChunk {
        change_context,
        old_lines: Vec::new(),
        new_lines: Vec::new(),
        is_end_of_file: false,
    };
    let mut parsed_lines = 0usize;

    for line in &lines[start_index..] {
        match *line {
            EOF_MARKER => {
                if parsed_lines == 0 {
                    return Err(ParseError::InvalidHunk {
                        message: "Update hunk does not contain any lines".to_string(),
                        line_number: line_number + 1,
                    });
                }
                chunk.is_end_of_file = true;
                parsed_lines += 1;
                break;
            }
            "" => {
                chunk.old_lines.push(String::new());
                chunk.new_lines.push(String::new());
                parsed_lines += 1;
            }
            line_contents => {
                match line_contents.chars().next() {
                    Some(' ') => {
                        chunk.old_lines.push(line_contents[1..].to_string());
                        chunk.new_lines.push(line_contents[1..].to_string());
                    }
                    Some('+') => {
                        chunk.new_lines.push(line_contents[1..].to_string());
                    }
                    Some('-') => {
                        chunk.old_lines.push(line_contents[1..].to_string());
                    }
                    _ if parsed_lines > 0 => break,
                    _ => {
                        return Err(ParseError::InvalidHunk {
                            message: format!(
                                "Unexpected line found in update hunk: '{line_contents}'. Every line should start with ' ' (context line), '+' (added line), or '-' (removed line)"
                            ),
                            line_number: line_number + 1,
                        });
                    }
                }
                parsed_lines += 1;
            }
        }
    }

    Ok((chunk, parsed_lines + start_index))
}

fn apply_hunks_to_files(
    project_root: &Path,
    protected: &ProtectedPaths,
    hunks: &[Hunk],
    enter: PatchEntry<'_>,
) -> Result<AffectedPaths, ApplyPatchError> {
    if hunks.is_empty() {
        return Err(ApplyPatchError::ComputeReplacements(
            "No files were modified.".to_string(),
        ));
    }

    // Every source and destination of the patch is resolved before the
    // first change, so a patch that names a path this tool never writes (or
    // one outside the project root) applies none of its operations. Each
    // operation resolves its paths again when it runs.
    for hunk in hunks {
        match hunk {
            Hunk::Add { path, .. } | Hunk::Delete { path } => {
                resolve_patch_path(project_root, protected, path)?;
            }
            Hunk::Update {
                path, move_path, ..
            } => {
                resolve_patch_path(project_root, protected, path)?;
                if let Some(dest) = move_path {
                    resolve_patch_path(project_root, protected, dest)?;
                }
            }
        }
    }

    let mut affected = AffectedPaths::default();
    for hunk in hunks {
        match hunk {
            Hunk::Add { path, contents } => {
                let path = resolve_patch_path(project_root, protected, path)?;
                if path.exists() {
                    return Err(ApplyPatchError::ComputeReplacements(format!(
                        "Failed to write file {}",
                        path.display()
                    )));
                }
                enter()?;
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|source| ApplyPatchError::Io {
                        context: format!(
                            "Failed to create parent directories for {}",
                            path.display()
                        ),
                        source,
                    })?;
                }
                std::fs::write(&path, contents).map_err(|source| ApplyPatchError::Io {
                    context: format!("Failed to write file {}", path.display()),
                    source,
                })?;
                affected.added.push(path);
            }
            Hunk::Delete { path } => {
                let path = resolve_patch_path(project_root, protected, path)?;
                let metadata = std::fs::metadata(&path).map_err(|source| ApplyPatchError::Io {
                    context: format!("Failed to delete file {}", path.display()),
                    source,
                })?;
                if metadata.is_dir() {
                    return Err(ApplyPatchError::Io {
                        context: format!("Failed to delete file {}", path.display()),
                        source: std::io::Error::other("Is a directory"),
                    });
                }
                enter()?;
                std::fs::remove_file(&path).map_err(|source| ApplyPatchError::Io {
                    context: format!("Failed to delete file {}", path.display()),
                    source,
                })?;
                affected.deleted.push(path);
            }
            Hunk::Update {
                path,
                move_path,
                chunks,
            } => {
                let path = resolve_patch_path(project_root, protected, path)?;
                let new_contents = derive_new_contents_from_chunks(&path, chunks)?;

                if let Some(dest) = move_path {
                    let dest = resolve_patch_path(project_root, protected, dest)?;
                    if dest.exists() && dest != path {
                        return Err(ApplyPatchError::Io {
                            context: format!("Failed to write file {}", dest.display()),
                            source: std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "destination already exists",
                            ),
                        });
                    }
                    enter()?;
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent).map_err(|source| ApplyPatchError::Io {
                            context: format!(
                                "Failed to create parent directories for {}",
                                dest.display()
                            ),
                            source,
                        })?;
                    }
                    std::fs::write(&dest, new_contents).map_err(|source| ApplyPatchError::Io {
                        context: format!("Failed to write file {}", dest.display()),
                        source,
                    })?;
                    if dest != path {
                        std::fs::remove_file(&path).map_err(|source| ApplyPatchError::Io {
                            context: format!("Failed to remove original {}", path.display()),
                            source,
                        })?;
                    }
                    affected.modified.push(dest);
                } else {
                    enter()?;
                    std::fs::write(&path, new_contents).map_err(|source| ApplyPatchError::Io {
                        context: format!("Failed to write file {}", path.display()),
                        source,
                    })?;
                    affected.modified.push(path);
                }
            }
        }
    }
    Ok(affected)
}

fn derive_new_contents_from_chunks(
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> Result<String, ApplyPatchError> {
    let original_contents =
        std::fs::read_to_string(path).map_err(|source| ApplyPatchError::Io {
            context: format!("Failed to read file to update {}", path.display()),
            source,
        })?;

    let mut original_lines: Vec<String> = original_contents.split('\n').map(String::from).collect();
    if original_lines.last().is_some_and(String::is_empty) {
        original_lines.pop();
    }

    let replacements = compute_replacements(&original_lines, path, chunks)?;
    let mut new_lines = apply_replacements(original_lines, &replacements);
    let should_have_trailing_newline = !chunks.iter().any(|chunk| chunk.is_end_of_file);
    if should_have_trailing_newline && !new_lines.last().is_some_and(String::is_empty) {
        new_lines.push(String::new());
    } else if !should_have_trailing_newline && new_lines.last().is_some_and(String::is_empty) {
        new_lines.pop();
    }
    Ok(new_lines.join("\n"))
}

fn compute_replacements(
    original_lines: &[String],
    path: &Path,
    chunks: &[UpdateFileChunk],
) -> Result<Vec<(usize, usize, Vec<String>)>, ApplyPatchError> {
    let mut replacements = Vec::new();
    let mut line_index = 0usize;

    for chunk in chunks {
        if let Some(ctx_line) = &chunk.change_context {
            if let Some(idx) = seek_sequence(
                original_lines,
                std::slice::from_ref(ctx_line),
                line_index,
                false,
            ) {
                line_index = idx + 1;
            } else {
                return Err(ApplyPatchError::ComputeReplacements(format!(
                    "Failed to find context '{}' in {}",
                    ctx_line,
                    path.display()
                )));
            }
        }

        if chunk.old_lines.is_empty() {
            let insertion_idx = if chunk.change_context.is_some() {
                line_index
            } else if original_lines.last().is_some_and(String::is_empty) {
                original_lines.len() - 1
            } else {
                original_lines.len()
            };
            replacements.push((insertion_idx, 0, chunk.new_lines.clone()));
            continue;
        }

        let mut pattern: &[String] = &chunk.old_lines;
        let mut new_slice: &[String] = &chunk.new_lines;
        let mut found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);

        if found.is_none() && pattern.last().is_some_and(String::is_empty) {
            pattern = &pattern[..pattern.len() - 1];
            if new_slice.last().is_some_and(String::is_empty) {
                new_slice = &new_slice[..new_slice.len() - 1];
            }
            found = seek_sequence(original_lines, pattern, line_index, chunk.is_end_of_file);
        }

        if let Some(start_idx) = found {
            replacements.push((start_idx, pattern.len(), new_slice.to_vec()));
            line_index = start_idx + pattern.len();
        } else {
            return Err(ApplyPatchError::ComputeReplacements(format!(
                "Failed to find expected lines in {}:\n{}",
                path.display(),
                chunk.old_lines.join("\n"),
            )));
        }
    }

    replacements.sort_by_key(|lhs| lhs.0);
    Ok(replacements)
}

fn apply_replacements(
    mut lines: Vec<String>,
    replacements: &[(usize, usize, Vec<String>)],
) -> Vec<String> {
    for (start_idx, old_len, new_segment) in replacements.iter().rev() {
        for _ in 0..*old_len {
            if *start_idx < lines.len() {
                lines.remove(*start_idx);
            }
        }
        for (offset, new_line) in new_segment.iter().enumerate() {
            lines.insert(*start_idx + offset, new_line.clone());
        }
    }
    lines
}

fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }

    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };

    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if pattern
            .iter()
            .enumerate()
            .all(|(p_idx, pat)| lines[i + p_idx].trim_end() == pat.trim_end())
        {
            return Some(i);
        }
    }
    (search_start..=lines.len().saturating_sub(pattern.len())).find(|&i| {
        pattern
            .iter()
            .enumerate()
            .all(|(p_idx, pat)| normalize_line(&lines[i + p_idx]) == normalize_line(pat))
    })
}

fn normalize_line(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn resolve_patch_path(
    project_root: &Path,
    protected: &ProtectedPaths,
    relative: &Path,
) -> Result<PathBuf, ApplyPatchError> {
    let mut resolved = if relative.is_absolute() {
        PathBuf::new()
    } else {
        project_root.to_path_buf()
    };
    for component in relative.components() {
        match component {
            Component::Prefix(_) => {
                return Err(ApplyPatchError::InvalidPath(format!(
                    "unsupported path prefix '{}'",
                    relative.display()
                )));
            }
            Component::RootDir => resolved = PathBuf::from("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(segment) => resolved.push(segment),
        }
    }

    if !resolved.starts_with(project_root) {
        return Err(ApplyPatchError::InvalidPath(format!(
            "path '{}' escapes the project root",
            relative.display()
        )));
    }

    // Lexical containment is not enough: a component of `resolved` may be a
    // symlink that points outside `project_root`. Resolve the real path by
    // canonicalizing the deepest existing ancestor (the target itself may not
    // exist yet for an Add/Move), then re-appending the not-yet-existing tail,
    // and re-check containment. Both sides MUST be canonicalized: `project_root`
    // itself may be reached through symlinks (e.g. macOS `/var` -> `/private/var`),
    // so comparing a canonical real path against a non-canonical root would both
    // falsely reject legitimate in-root paths and could mis-judge escapes.
    let canonical_root = canonicalize_existing_ancestor(project_root)?;
    let real = canonicalize_existing_ancestor(&resolved)?;
    if !real.starts_with(&canonical_root) {
        return Err(ApplyPatchError::InvalidPath(format!(
            "path '{}' escapes the project root via a symlink",
            relative.display()
        )));
    }

    // Inside the root, the engine's own directory and the host's declared
    // paths are not the project's to write, and a symbolic link that leads
    // nowhere is not a path to write through (the fallback above resolves it
    // to its parent, and the write would create its target).
    protected
        .admit_write(project_root, &resolved)
        .map_err(|refusal| ApplyPatchError::ProtectedPath(refusal.reason(relative)))?;

    Ok(resolved)
}

/// Resolve `path` to its real (symlink-followed) location by canonicalizing the
/// deepest ancestor that exists on disk and re-appending the components that do
/// not yet exist. The target of an Add/Move need not exist; its existing parent
/// chain is what must stay inside the project root.
fn canonicalize_existing_ancestor(path: &Path) -> Result<PathBuf, ApplyPatchError> {
    let mut ancestor = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();

    loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(canonical) => {
                let mut real = canonical;
                for segment in tail.iter().rev() {
                    real.push(segment);
                }
                return Ok(real);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                match ancestor.file_name() {
                    Some(name) => {
                        tail.push(name.to_os_string());
                        if !ancestor.pop() {
                            return Err(ApplyPatchError::InvalidPath(format!(
                                "path '{}' has no resolvable ancestor",
                                path.display()
                            )));
                        }
                    }
                    None => {
                        return Err(ApplyPatchError::InvalidPath(format!(
                            "path '{}' has no resolvable ancestor",
                            path.display()
                        )));
                    }
                }
            }
            Err(source) => {
                return Err(ApplyPatchError::Io {
                    context: format!("failed to resolve real path for {}", ancestor.display()),
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
    }

    fn refused() -> ApplyPatchError {
        ApplyPatchError::Entry(BuiltinToolError::EntryRefused(Box::new(
            meerkat_core::ToolError::ReviewUnavailable {
                kind: meerkat_core::ReviewUnavailableKind::DispatchEnded,
            },
        )))
    }

    /// The worker's entry precedes its FIRST change, here the parent mkdir:
    /// a refusal leaves no directory and no file behind.
    #[test]
    fn refused_entry_precedes_the_first_change_including_mkdir() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let patch = wrap_patch("*** Add File: nested/new.txt\n+hello");
        let mut entries = 0usize;
        let err = apply_patch(root, &ProtectedPaths::default(), &patch, &mut || {
            entries += 1;
            Err(refused())
        })
        .expect_err("refused entry");
        assert!(matches!(
            err,
            ApplyPatchError::Entry(BuiltinToolError::EntryRefused(_))
        ));
        assert_eq!(entries, 1);
        assert!(!root.join("nested").exists(), "no directory was created");
    }

    /// Validation and preparation failures precede entry, so they never
    /// spend it; a multi-change patch enters exactly once.
    #[test]
    fn entry_runs_once_after_preparation_and_never_for_a_failed_preparation() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let mut entries = 0usize;
        apply_patch(root, &ProtectedPaths::default(), "not a patch", &mut || {
            entries += 1;
            Ok(())
        })
        .expect_err("invalid patch");
        assert_eq!(entries, 0, "a parse failure never enters");
        std::fs::write(root.join("exists.txt"), "x").unwrap();
        apply_patch(
            root,
            &ProtectedPaths::default(),
            &wrap_patch("*** Add File: exists.txt\n+y"),
            &mut || {
                entries += 1;
                Ok(())
            },
        )
        .expect_err("existing destination");
        assert_eq!(entries, 0, "a refused precondition never enters");

        let patch = wrap_patch("*** Add File: a.txt\n+a\n*** Add File: b.txt\n+b");
        let mut entered = LeafEntry::none();
        let mut calls = 0usize;
        apply_patch(root, &ProtectedPaths::default(), &patch, &mut || {
            calls += 1;
            entered.enter().map_err(ApplyPatchError::Entry)
        })
        .expect("allowed entry");
        assert_eq!(calls, 2, "each change asks; the owned entry spends once");
        assert!(root.join("a.txt").exists() && root.join("b.txt").exists());
    }

    /// The example embedded in the tool description must be a patch the
    /// tool actually accepts, with the documented anchor semantics.
    #[test]
    fn documented_example_applies_verbatim() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/utils")).unwrap();
        std::fs::write(
            root.join("src/config.rs"),
            "fn defaults() {\n    let timeout = 30;\n    let retries = 3;\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("src/old_name.rs"), "fn old_func() {}\n").unwrap();
        std::fs::write(root.join("src/deprecated.rs"), "// gone\n").unwrap();

        let affected = apply_patch(
            root,
            &ProtectedPaths::default(),
            APPLY_PATCH_EXAMPLE,
            &mut || Ok(()),
        )
        .unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("src/config.rs")).unwrap(),
            "fn defaults() {\n    let timeout = 60;\n    let retries = 3;\n}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("src/new_name.rs")).unwrap(),
            "fn new_func() {}\n"
        );
        assert!(!root.join("src/old_name.rs").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("src/utils/helper.py")).unwrap(),
            "def greet(name):\n    return f\"Hello, {name}\"\n"
        );
        assert!(!root.join("src/deprecated.rs").exists());
        assert_eq!(affected.added.len(), 1);
        assert_eq!(affected.modified.len(), 2);
        assert_eq!(affected.deleted.len(), 1);
    }

    /// The grammar is documented once, in the tool description; the `patch`
    /// argument schema points at it instead of repeating it.
    #[test]
    fn grammar_is_documented_once_and_keeps_every_rule() {
        let def = ApplyPatchTool::new(PathBuf::from("/tmp")).def();
        let schema_text = def.input_schema.to_string();
        assert!(
            !schema_text.contains("*** Begin Patch"),
            "the argument schema must not repeat the grammar: {schema_text}"
        );
        for rule in [
            "*** Begin Patch",
            "*** End Patch",
            "*** Add File: <path>",
            "*** Delete File: <path>",
            "*** Update File: <path>",
            "*** Move to: <new_path>",
            "A move still needs at least one hunk",
            "'@@ <anchor>'",
            "never a line number",
            "bare '@@'",
            "the space is required",
            "A blank line inside a hunk is an empty context line",
            "Trailing whitespace",
            "smart quotes",
            "each search starts after the previous hunk",
            "*** End of File",
            "'..'",
            "absolute paths",
        ] {
            assert!(
                def.description.contains(rule),
                "tool description must document {rule:?}: {}",
                def.description
            );
        }
    }

    #[test]
    fn parse_multiple_hunks() {
        let patch = wrap_patch(
            "*** Add File: foo.txt\n+hello\n*** Delete File: bar.txt\n*** Update File: baz.txt\n@@\n-old\n+new",
        );
        let hunks = parse_patch(&patch).unwrap();
        assert_eq!(hunks.len(), 3);
    }

    #[test]
    fn apply_patch_add_update_move_and_delete() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("source.txt"), "a\nb\n").unwrap();
        std::fs::write(root.join("delete.txt"), "gone\n").unwrap();

        let patch = wrap_patch(
            "*** Add File: nested/new.txt\n+hello\n+world\n*** Update File: source.txt\n*** Move to: moved.txt\n@@\n a\n-b\n+c\n*** Delete File: delete.txt",
        );

        let affected =
            apply_patch(root, &ProtectedPaths::default(), &patch, &mut || Ok(())).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("nested/new.txt")).unwrap(),
            "hello\nworld\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("moved.txt")).unwrap(),
            "a\nc\n"
        );
        assert!(!root.join("source.txt").exists());
        assert!(!root.join("delete.txt").exists());
        assert_eq!(affected.added.len(), 1);
        assert_eq!(affected.modified.len(), 1);
        assert_eq!(affected.deleted.len(), 1);
    }

    #[test]
    fn update_end_of_file_marker_removes_trailing_newline() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("tail.txt");
        std::fs::write(&path, "alpha\nbeta\n").unwrap();

        let patch =
            wrap_patch("*** Update File: tail.txt\n@@\n alpha\n-beta\n+gamma\n*** End of File");
        apply_patch(root, &ProtectedPaths::default(), &patch, &mut || Ok(())).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\ngamma");
    }

    #[test]
    fn addition_only_chunk_inserts_after_context() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("imports.py");
        std::fs::write(&path, "import a\nimport z\n").unwrap();

        let patch = wrap_patch("*** Update File: imports.py\n@@ import a\n+import m");
        apply_patch(root, &ProtectedPaths::default(), &patch, &mut || Ok(())).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "import a\nimport m\nimport z\n"
        );
    }

    #[test]
    fn rejects_escape_path() {
        let dir = tempdir().unwrap();
        let patch = wrap_patch("*** Add File: ../escape.txt\n+bad");
        let err = apply_patch(dir.path(), &ProtectedPaths::default(), &patch, &mut || {
            Ok(())
        })
        .expect_err("escape should fail");
        assert!(err.to_string().contains("escapes the project root"));
    }

    #[test]
    fn move_to_same_normalized_path_updates_in_place() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("same.txt");
        std::fs::write(&path, "before\n").unwrap();

        let patch =
            wrap_patch("*** Update File: same.txt\n*** Move to: ./same.txt\n@@\n-before\n+after");
        apply_patch(root, &ProtectedPaths::default(), &patch, &mut || Ok(())).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape_path() {
        use std::os::unix::fs::symlink;

        // Two separate canonical roots: `root` is the project root the patch is
        // applied against; `outside` is forbidden territory.
        let root_dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let root = root_dir.path();
        let outside = outside_dir.path();

        std::fs::create_dir(outside.join("secret")).unwrap();

        // Inside the project root, plant a symlink that points OUTSIDE the root.
        // A lexical containment check sees `root/link/loot.txt` as in-root, but
        // the real path resolves under `outside/secret`.
        symlink(outside.join("secret"), root.join("link")).unwrap();

        let patch = wrap_patch("*** Add File: link/loot.txt\n+pwned");
        let err = apply_patch(root, &ProtectedPaths::default(), &patch, &mut || Ok(()))
            .expect_err("symlink escape should fail");
        assert!(
            err.to_string().contains("escapes the project root"),
            "expected typed containment rejection, got: {err}"
        );
        // The write must not have happened outside the root.
        assert!(!outside.join("secret/loot.txt").exists());

        // A normal in-root path through a real (non-escaping) directory still works.
        let patch_ok = wrap_patch("*** Add File: nested/ok.txt\n+fine");
        apply_patch(root, &ProtectedPaths::default(), &patch_ok, &mut || Ok(()))
            .expect("in-root path should succeed");
        assert_eq!(
            std::fs::read_to_string(root.join("nested/ok.txt")).unwrap(),
            "fine\n"
        );
    }

    // ---- The engine's own directory, `<project root>/.rkat` ----

    /// A project root as the tool is given it by a host: canonical.
    fn project() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        (dir, root)
    }

    /// One call through the tool, as the dispatcher makes it.
    async fn call_patch(root: &Path, body: &str) -> Result<ToolOutput, BuiltinToolError> {
        ApplyPatchTool::new(root.to_path_buf())
            .call(json!({ "patch": wrap_patch(body) }))
            .await
    }

    /// The call was refused as a write into the engine's own directory, with
    /// the typed invalid-arguments error and its generic text.
    fn assert_engine_refusal(result: &Result<ToolOutput, BuiltinToolError>, case: &str) {
        assert!(
            matches!(
                result,
                Err(BuiltinToolError::InvalidArgs(reason)) if reason.contains("engine's own directory")
            ),
            "{case}: expected the engine directory refusal, got {result:?}"
        );
    }

    /// The call was refused because its path goes through a symbolic link
    /// that leads nowhere.
    fn assert_dangling_link_refusal(result: &Result<ToolOutput, BuiltinToolError>, case: &str) {
        assert!(
            matches!(
                result,
                Err(BuiltinToolError::InvalidArgs(reason)) if reason.contains("symbolic link")
            ),
            "{case}: expected the dangling link refusal, got {result:?}"
        );
    }

    /// The first write would create `.rkat`: it is refused on the path's
    /// components, and nothing is created.
    #[tokio::test]
    async fn engine_directory_that_does_not_exist_yet_is_refused() {
        let (_dir, root) = project();
        let result = call_patch(&root, "*** Add File: .rkat/mcp.toml\n+[[servers]]").await;
        assert_engine_refusal(&result, "add .rkat/mcp.toml");
        assert!(!root.join(".rkat").exists(), "no directory was created");

        let result = call_patch(&root, "*** Add File: .rkat\n+a file named like it").await;
        assert_engine_refusal(&result, "add a file named .rkat");
        assert!(!root.join(".rkat").exists(), "no file was created");
    }

    /// Every spelling of a path under `.rkat` is refused: relative, with
    /// `.` and `..`, absolute, and in another letter case (a case-insensitive
    /// filesystem opens the same directory).
    #[tokio::test]
    async fn engine_directory_is_refused_in_every_path_form() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let absolute = root.join(".rkat/absolute.toml");
        let forms = [
            ".rkat/plain.toml".to_owned(),
            "./.rkat/dot.toml".to_owned(),
            "src/../.rkat/parent.toml".to_owned(),
            ".rkat/realms/../../.rkat/twice.toml".to_owned(),
            absolute.display().to_string(),
            ".RKAT/upper.toml".to_owned(),
            ".Rkat/mixed.toml".to_owned(),
            ".rkat/realms/ws-1/config.toml".to_owned(),
        ];
        for form in forms {
            let result = call_patch(&root, &format!("*** Add File: {form}\n+planted")).await;
            assert_engine_refusal(&result, &form);
        }
        assert_eq!(
            std::fs::read_dir(root.join(".rkat")).unwrap().count(),
            0,
            "nothing was written under .rkat"
        );
        for other_case in [".RKAT", ".Rkat"] {
            assert!(
                !root.join(other_case).exists(),
                "{other_case} was not created"
            );
        }
    }

    /// A symbolic link inside the project that leads into `.rkat` does not
    /// open it: a directory link, and a file link to one of its files.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_into_the_engine_directory_is_refused() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        std::fs::write(root.join(".rkat/config.toml"), "kept = true\n").unwrap();
        symlink(root.join(".rkat"), root.join("settings")).unwrap();
        symlink(".rkat/config.toml", root.join("alias.toml")).unwrap();

        let result = call_patch(&root, "*** Add File: settings/mcp.toml\n+[[servers]]").await;
        assert_engine_refusal(&result, "add through a directory link");
        assert!(!root.join(".rkat/mcp.toml").exists());

        let result = call_patch(
            &root,
            "*** Update File: alias.toml\n@@\n-kept = true\n+kept = false",
        )
        .await;
        assert_engine_refusal(&result, "update through a file link");
        let result = call_patch(&root, "*** Delete File: settings/config.toml").await;
        assert_engine_refusal(&result, "delete through a directory link");
        assert_eq!(
            std::fs::read_to_string(root.join(".rkat/config.toml")).unwrap(),
            "kept = true\n"
        );
    }

    /// A symbolic link into an engine directory deeper in the project does
    /// not open it: the patch path names no `.rkat`, the real path does.
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_into_a_nested_engine_directory_is_refused() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("packages/app/.rkat")).unwrap();
        std::fs::write(root.join("packages/app/.rkat/config.toml"), "kept = true\n").unwrap();
        symlink("packages/app/.rkat", root.join("alias")).unwrap();

        let result = call_patch(&root, "*** Add File: alias/mcp.toml\n+[[servers]]").await;
        assert_engine_refusal(&result, "add through a link into a nested .rkat");
        let result = call_patch(
            &root,
            "*** Update File: alias/config.toml\n@@\n-kept = true\n+kept = false",
        )
        .await;
        assert_engine_refusal(&result, "update through a link into a nested .rkat");
        assert_eq!(
            std::fs::read_dir(root.join("packages/app/.rkat"))
                .unwrap()
                .count(),
            1,
            "nothing was added"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("packages/app/.rkat/config.toml")).unwrap(),
            "kept = true\n"
        );
    }

    /// Update, delete and both ends of a move are refused, and the tree is
    /// unchanged: no engine file is changed, removed, moved out or replaced.
    #[tokio::test]
    async fn update_delete_and_move_of_engine_files_are_refused() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        std::fs::write(root.join(".rkat/config.toml"), "kept = true\n").unwrap();
        std::fs::write(root.join("notes.txt"), "note\n").unwrap();

        let cases = [
            (
                "update",
                "*** Update File: .rkat/config.toml\n@@\n-kept = true\n+kept = false",
            ),
            ("delete", "*** Delete File: .rkat/config.toml"),
            (
                "move out of .rkat",
                "*** Update File: .rkat/config.toml\n*** Move to: taken.toml\n@@\n-kept = true\n+kept = false",
            ),
            (
                "move into .rkat",
                "*** Update File: notes.txt\n*** Move to: .rkat/hooks.toml\n@@\n-note\n+hook",
            ),
        ];
        for (case, body) in cases {
            let result = call_patch(&root, body).await;
            assert_engine_refusal(&result, case);
        }
        assert_eq!(
            std::fs::read_to_string(root.join(".rkat/config.toml")).unwrap(),
            "kept = true\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "note\n"
        );
        assert!(!root.join("taken.toml").exists());
        assert!(!root.join(".rkat/hooks.toml").exists());
    }

    /// A patch with one engine target applies nothing: the refusal precedes
    /// the first write, wherever in the patch the target stands.
    #[tokio::test]
    async fn a_patch_with_one_engine_target_applies_nothing() {
        let (_dir, root) = project();
        std::fs::write(root.join("old.txt"), "old\n").unwrap();
        std::fs::write(root.join("edit.txt"), "before\n").unwrap();

        let result = call_patch(
            &root,
            "*** Add File: first.txt\n+first\n*** Update File: edit.txt\n@@\n-before\n+after\n*** Delete File: old.txt\n*** Add File: .rkat/mcp.toml\n+[[servers]]\n*** Add File: last.txt\n+last",
        )
        .await;
        assert_engine_refusal(&result, "multi-file patch");
        assert!(
            !root.join("first.txt").exists(),
            "an earlier add was applied"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("edit.txt")).unwrap(),
            "before\n",
            "an earlier update was applied"
        );
        assert!(
            root.join("old.txt").exists(),
            "an earlier delete was applied"
        );
        assert!(!root.join(".rkat").exists());
        assert!(!root.join("last.txt").exists());
    }

    /// A symbolic link whose target does not exist is not a path to write
    /// through: the write would create the target, wherever it points, the
    /// engine's directory and a path outside the project root included.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dangling_symlink_is_not_written_through() {
        use std::os::unix::fs::symlink;

        let (_dir, root) = project();
        let outside_dir = tempdir().unwrap();
        let outside = outside_dir.path().canonicalize().unwrap();
        std::fs::write(root.join("notes.txt"), "note\n").unwrap();
        symlink(outside.join("created.txt"), root.join("outside-link")).unwrap();
        symlink(".rkat/mcp.toml", root.join("engine-link")).unwrap();
        symlink(outside.join("missing-dir"), root.join("dir-link")).unwrap();

        let result = call_patch(&root, "*** Add File: outside-link\n+escaped").await;
        assert_dangling_link_refusal(&result, "add through a link to outside the root");
        assert!(
            !outside.join("created.txt").exists(),
            "the add created a file outside the project root"
        );

        let result = call_patch(
            &root,
            "*** Update File: notes.txt\n*** Move to: outside-link\n@@\n-note\n+escaped",
        )
        .await;
        assert_dangling_link_refusal(&result, "move onto a link to outside the root");
        assert!(
            !outside.join("created.txt").exists(),
            "the move created a file outside the project root"
        );
        assert!(
            root.join("notes.txt").exists(),
            "the move source was removed"
        );

        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        let result = call_patch(&root, "*** Add File: engine-link\n+[[servers]]").await;
        assert!(result.is_err(), "add through a link into .rkat: {result:?}");
        assert!(
            !root.join(".rkat/mcp.toml").exists(),
            "the add created a file in the engine's directory"
        );

        let result = call_patch(&root, "*** Add File: dir-link/new.txt\n+escaped").await;
        assert_dangling_link_refusal(&result, "add below a dangling directory link");
        assert!(!outside.join("missing-dir").exists());
    }

    /// A file or directory whose name only resembles `.rkat` is the
    /// project's own and stays writable. Updating and moving an ordinary
    /// file still works.
    #[tokio::test]
    async fn files_beside_the_engine_directory_stay_writable() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join(".rkat")).unwrap();
        for path in [
            ".rkatignore",
            "src/.rkat.rs",
            "rkat/config.toml",
            "packages/app/rkat/notes.txt",
        ] {
            let result = call_patch(&root, &format!("*** Add File: {path}\n+project file")).await;
            assert!(result.is_ok(), "{path}: {result:?}");
            assert_eq!(
                std::fs::read_to_string(root.join(path)).unwrap(),
                "project file\n"
            );
        }
        let result = call_patch(
            &root,
            "*** Update File: packages/app/rkat/notes.txt\n*** Move to: packages/app/rkat/moved.txt\n@@\n-project file\n+moved",
        )
        .await;
        assert!(result.is_ok(), "move of an ordinary file: {result:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("packages/app/rkat/moved.txt")).unwrap(),
            "moved\n"
        );
    }

    /// A `.rkat` directory deeper in the project is an engine directory
    /// too: a host started from that subdirectory reads its MCP servers,
    /// hooks, skills and realms from it. Every operation on it is refused,
    /// whether or not it exists yet.
    #[tokio::test]
    async fn an_engine_directory_deeper_in_the_project_is_refused() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("services/api/.rkat")).unwrap();
        std::fs::write(root.join("services/api/.rkat/config.toml"), "kept = true\n").unwrap();
        std::fs::write(root.join("notes.txt"), "note\n").unwrap();
        let cases = [
            (
                "add under a nested .rkat that does not exist yet",
                "*** Add File: packages/app/.rkat/mcp.toml\n+[[servers]]",
            ),
            (
                "add a realm document under a nested .rkat",
                "*** Add File: packages/app/.rkat/realms/r/config.toml\n+planted",
            ),
            (
                "add under an existing nested .rkat in another case",
                "*** Add File: services/api/.RKAT/mcp.toml\n+[[servers]]",
            ),
            (
                "update in a nested .rkat",
                "*** Update File: services/api/.rkat/config.toml\n@@\n-kept = true\n+kept = false",
            ),
            (
                "delete in a nested .rkat",
                "*** Delete File: services/api/.rkat/config.toml",
            ),
            (
                "move out of a nested .rkat",
                "*** Update File: services/api/.rkat/config.toml\n*** Move to: taken.toml\n@@\n-kept = true\n+kept = false",
            ),
            (
                "move into a nested .rkat",
                "*** Update File: notes.txt\n*** Move to: services/api/.rkat/hooks.toml\n@@\n-note\n+hook",
            ),
        ];
        for (case, body) in cases {
            let result = call_patch(&root, body).await;
            assert_engine_refusal(&result, case);
        }
        assert!(!root.join("packages").exists(), "nothing was created");
        assert_eq!(
            std::fs::read_to_string(root.join("services/api/.rkat/config.toml")).unwrap(),
            "kept = true\n"
        );
        assert_eq!(
            std::fs::read_dir(root.join("services/api/.rkat"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            std::fs::read_to_string(root.join("notes.txt")).unwrap(),
            "note\n"
        );
        assert!(!root.join("taken.toml").exists());
    }

    /// The check before the first change covers the destination of a move
    /// too: a patch whose LAST operation moves a file into `.rkat` applies
    /// nothing, and every file is byte for byte what it was.
    #[tokio::test]
    async fn a_patch_whose_last_operation_moves_into_the_engine_directory_applies_nothing() {
        let (_dir, root) = project();
        std::fs::write(root.join("old.txt"), "old\n").unwrap();
        std::fs::write(root.join("edit.txt"), "before\n").unwrap();
        std::fs::write(root.join("source.txt"), "source\n").unwrap();

        let result = call_patch(
            &root,
            "*** Add File: first.txt\n+first\n*** Update File: edit.txt\n@@\n-before\n+after\n*** Delete File: old.txt\n*** Update File: source.txt\n*** Move to: .rkat/hooks.toml\n@@\n-source\n+hook",
        )
        .await;
        assert_engine_refusal(&result, "a move into .rkat as the last operation");
        let mut entries: Vec<(String, String)> = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(entry.path()).unwrap(),
                )
            })
            .collect();
        entries.sort();
        assert_eq!(
            entries,
            [
                ("edit.txt".to_owned(), "before\n".to_owned()),
                ("old.txt".to_owned(), "old\n".to_owned()),
                ("source.txt".to_owned(), "source\n".to_owned()),
            ],
            "the tree is unchanged"
        );
    }

    /// The paths a host declared as its own are refused like the engine
    /// directory, with everything under them, and a patch that names one
    /// applies nothing.
    #[tokio::test]
    async fn host_declared_paths_are_refused_with_everything_under_them() {
        let (_dir, root) = project();
        std::fs::create_dir_all(root.join("state/realm")).unwrap();
        std::fs::write(root.join("state/realm/config.toml"), "kept = true\n").unwrap();
        let tool = ApplyPatchTool::new(root.clone()).protecting(ProtectedPaths::with_host_paths([
            root.join("state"),
            root.join("credentials"),
        ]));
        let call = |body: &'static str| {
            let tool = tool.clone();
            async move { tool.call(json!({ "patch": wrap_patch(body) })).await }
        };

        let result =
            call("*** Update File: state/realm/config.toml\n@@\n-kept = true\n+kept = false").await;
        assert_engine_refusal(&result, "update in the declared state root");
        let result = call("*** Add File: credentials/token.json\n+{}").await;
        assert_engine_refusal(
            &result,
            "add in a declared directory that does not exist yet",
        );
        let result =
            call("*** Add File: notes.txt\n+note\n*** Delete File: state/realm/config.toml").await;
        assert_engine_refusal(&result, "multi-file patch with a declared path");
        assert!(
            !root.join("notes.txt").exists(),
            "an earlier add was applied"
        );
        assert!(!root.join("credentials").exists());
        assert_eq!(
            std::fs::read_to_string(root.join("state/realm/config.toml")).unwrap(),
            "kept = true\n"
        );

        let result = call("*** Add File: notes.txt\n+note").await;
        assert!(
            result.is_ok(),
            "a file beside the declared paths: {result:?}"
        );
    }

    /// The tool creates plain files and directories only: no operation
    /// makes a symbolic or a hard link, so a session cannot first link a
    /// path of its own to a protected file and then write through it.
    #[cfg(unix)]
    #[tokio::test]
    async fn every_operation_creates_plain_files_only() {
        use std::os::unix::fs::MetadataExt;

        let (_dir, root) = project();
        std::fs::write(root.join("source.txt"), "a\n").unwrap();
        let result = call_patch(
            &root,
            "*** Add File: nested/added.txt\n+added\n*** Update File: source.txt\n*** Move to: nested/moved.txt\n@@\n-a\n+b",
        )
        .await;
        assert!(result.is_ok(), "{result:?}");
        for path in ["nested/added.txt", "nested/moved.txt"] {
            let entry = std::fs::symlink_metadata(root.join(path)).unwrap();
            assert!(entry.file_type().is_file(), "{path} is a regular file");
            assert_eq!(entry.nlink(), 1, "{path} has no second name");
        }
        assert!(
            std::fs::symlink_metadata(root.join("nested"))
                .unwrap()
                .file_type()
                .is_dir()
        );
        assert!(!root.join("source.txt").exists());
    }
}
