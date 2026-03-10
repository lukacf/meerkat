//! ApplyPatch tool for structured file edits within the project root.

use crate::builtin::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ApplyPatchTool {
    project_root: PathBuf,
}

impl ApplyPatchTool {
    pub fn new(project_root: PathBuf) -> Self {
        Self { project_root }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ApplyPatchArgs {
    #[schemars(
        description = "Patch text in apply_patch format, beginning with '*** Begin Patch' and ending with '*** End Patch'"
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
            name: self.name().into(),
            description: "Apply a structured patch to files inside the project root. The patch string must use the exact apply_patch grammar and supports Add File, Delete File, Update File, Move to, and End of File markers.".into(),
            input_schema: crate::schema::schema_for::<ApplyPatchArgs>(),
        }
    }

    fn default_enabled(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError> {
        let args: ApplyPatchArgs = serde_json::from_value(args)
            .map_err(|e| BuiltinToolError::invalid_args(format!("Invalid arguments: {e}")))?;
        let project_root = self.project_root.clone();
        let patch = args.patch;
        let affected = tokio::task::spawn_blocking(move || apply_patch(&project_root, &patch))
            .await
            .map_err(|e| {
                BuiltinToolError::execution_failed(format!("apply_patch task failed: {e}"))
            })?
            .map_err(|e| BuiltinToolError::execution_failed(e.to_string()))?;

        Ok(json!({
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
        }))
    }
}

fn apply_patch(project_root: &Path, patch: &str) -> Result<AffectedPaths, ApplyPatchError> {
    let canonical_root =
        std::fs::canonicalize(project_root).map_err(|source| ApplyPatchError::Io {
            context: format!("failed to resolve project root {}", project_root.display()),
            source,
        })?;
    let hunks = parse_patch(patch)?;
    apply_hunks_to_files(&canonical_root, &hunks)
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
    hunks: &[Hunk],
) -> Result<AffectedPaths, ApplyPatchError> {
    if hunks.is_empty() {
        return Err(ApplyPatchError::ComputeReplacements(
            "No files were modified.".to_string(),
        ));
    }

    let mut affected = AffectedPaths::default();
    for hunk in hunks {
        match hunk {
            Hunk::Add { path, contents } => {
                let path = resolve_patch_path(project_root, path)?;
                if path.exists() {
                    return Err(ApplyPatchError::ComputeReplacements(format!(
                        "Failed to write file {}",
                        path.display()
                    )));
                }
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
                let path = resolve_patch_path(project_root, path)?;
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
                let path = resolve_patch_path(project_root, path)?;
                let new_contents = derive_new_contents_from_chunks(&path, chunks)?;

                if let Some(dest) = move_path {
                    let dest = resolve_patch_path(project_root, dest)?;
                    if dest.exists() && dest != path {
                        return Err(ApplyPatchError::Io {
                            context: format!("Failed to write file {}", dest.display()),
                            source: std::io::Error::new(
                                std::io::ErrorKind::AlreadyExists,
                                "destination already exists",
                            ),
                        });
                    }
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

    replacements.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));
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

fn resolve_patch_path(project_root: &Path, relative: &Path) -> Result<PathBuf, ApplyPatchError> {
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
    Ok(resolved)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn wrap_patch(body: &str) -> String {
        format!("*** Begin Patch\n{body}\n*** End Patch")
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

        let affected = apply_patch(root, &patch).unwrap();
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
        apply_patch(root, &patch).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "alpha\ngamma");
    }

    #[test]
    fn addition_only_chunk_inserts_after_context() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let path = root.join("imports.py");
        std::fs::write(&path, "import a\nimport z\n").unwrap();

        let patch = wrap_patch("*** Update File: imports.py\n@@ import a\n+import m");
        apply_patch(root, &patch).unwrap();

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "import a\nimport m\nimport z\n"
        );
    }

    #[test]
    fn rejects_escape_path() {
        let dir = tempdir().unwrap();
        let patch = wrap_patch("*** Add File: ../escape.txt\n+bad");
        let err = apply_patch(dir.path(), &patch).expect_err("escape should fail");
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
        apply_patch(root, &patch).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
    }
}
