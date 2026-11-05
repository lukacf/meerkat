//! Target paths of an `apply_patch` body.
//!
//! **Local copy, to be upstreamed.** This mirrors the patch grammar of
//! upstream's `apply_patch` tool (`crates/meerkat-tools/src/builtin/utility/
//! apply_patch.rs`: `parse_patch`, `check_patch_boundaries`, `parse_one_hunk`,
//! `parse_update_file_chunk`), whose parser is private. Only the parts that
//! decide *which* hunks exist and which paths they name are kept; hunk bodies
//! are validated exactly like the tool does but not collected. The approval
//! policy must see the same targets the tool would touch, so any change to the
//! upstream grammar has to be mirrored here until upstream exposes an
//! `affected_paths` function.

use std::path::PathBuf;

const BEGIN_PATCH_MARKER: &str = "*** Begin Patch";
const END_PATCH_MARKER: &str = "*** End Patch";
const ADD_FILE_MARKER: &str = "*** Add File: ";
const DELETE_FILE_MARKER: &str = "*** Delete File: ";
const UPDATE_FILE_MARKER: &str = "*** Update File: ";
const MOVE_TO_MARKER: &str = "*** Move to: ";
const EOF_MARKER: &str = "*** End of File";
const CHANGE_CONTEXT_MARKER: &str = "@@ ";
const EMPTY_CHANGE_CONTEXT_MARKER: &str = "@@";

/// Why a patch does not parse (the tool would refuse it too).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The patch envelope is malformed.
    #[error("invalid patch: {0}")]
    InvalidPatch(String),
    /// A hunk is malformed.
    #[error("invalid hunk at line {line_number}, {message}")]
    InvalidHunk {
        /// What is wrong.
        message: String,
        /// 1-based line of the hunk.
        line_number: usize,
    },
}

/// Every path an `apply_patch` body would add, delete, update or move to, in
/// patch order, as written in the patch (relative paths are relative to the
/// project root).
///
/// # Errors
///
/// Returns [`ParseError`] when the patch does not parse.
pub fn affected_paths(patch: &str) -> Result<Vec<PathBuf>, ParseError> {
    let lines: Vec<&str> = patch.trim().lines().collect();
    check_patch_boundaries(&lines)?;

    let last_line_index = lines.len().saturating_sub(1);
    let mut remaining_lines = lines.get(1..last_line_index).unwrap_or_default();
    let mut line_number = 2usize;
    let mut paths = Vec::new();
    while !remaining_lines.is_empty() {
        let consumed_lines = parse_one_hunk(remaining_lines, line_number, &mut paths)?;
        line_number += consumed_lines;
        remaining_lines = remaining_lines.get(consumed_lines..).unwrap_or_default();
    }
    Ok(paths)
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

/// Parse one hunk, push its paths, and return the number of lines consumed.
fn parse_one_hunk(
    lines: &[&str],
    line_number: usize,
    paths: &mut Vec<PathBuf>,
) -> Result<usize, ParseError> {
    let Some(first_line) = lines.first().map(|line| line.trim()) else {
        return Err(ParseError::InvalidPatch(
            "patch must contain at least one hunk".to_string(),
        ));
    };
    let rest = lines.get(1..).unwrap_or_default();

    if let Some(path) = first_line.strip_prefix(ADD_FILE_MARKER) {
        let added = rest.iter().take_while(|line| line.starts_with('+')).count();
        paths.push(PathBuf::from(path));
        return Ok(1 + added);
    }

    if let Some(path) = first_line.strip_prefix(DELETE_FILE_MARKER) {
        paths.push(PathBuf::from(path));
        return Ok(1);
    }

    if let Some(path) = first_line.strip_prefix(UPDATE_FILE_MARKER) {
        let mut remaining_lines = rest;
        let mut parsed_lines = 1;
        let move_path = remaining_lines
            .first()
            .and_then(|line| line.strip_prefix(MOVE_TO_MARKER))
            .map(PathBuf::from);
        if move_path.is_some() {
            remaining_lines = remaining_lines.get(1..).unwrap_or_default();
            parsed_lines += 1;
        }

        let mut chunks = 0usize;
        while let Some(head) = remaining_lines.first() {
            if head.trim().is_empty() {
                parsed_lines += 1;
                remaining_lines = remaining_lines.get(1..).unwrap_or_default();
                continue;
            }
            if head.starts_with("***") {
                break;
            }
            let chunk_lines =
                parse_update_file_chunk(remaining_lines, line_number + parsed_lines, chunks == 0)?;
            chunks += 1;
            parsed_lines += chunk_lines;
            remaining_lines = remaining_lines.get(chunk_lines..).unwrap_or_default();
        }

        if chunks == 0 {
            return Err(ParseError::InvalidHunk {
                message: format!("Update file hunk for path '{path}' is empty"),
                line_number,
            });
        }

        paths.push(PathBuf::from(path));
        paths.extend(move_path);
        return Ok(parsed_lines);
    }

    Err(ParseError::InvalidHunk {
        message: format!(
            "'{first_line}' is not a valid hunk header. Valid hunk headers: '*** Add File: {{path}}', '*** Delete File: {{path}}', '*** Update File: {{path}}'"
        ),
        line_number,
    })
}

/// Validate one update chunk and return the number of lines consumed.
fn parse_update_file_chunk(
    lines: &[&str],
    line_number: usize,
    allow_missing_context: bool,
) -> Result<usize, ParseError> {
    let Some(first) = lines.first() else {
        return Err(ParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_string(),
            line_number,
        });
    };

    let start_index =
        if *first == EMPTY_CHANGE_CONTEXT_MARKER || first.starts_with(CHANGE_CONTEXT_MARKER) {
            1
        } else if allow_missing_context {
            0
        } else {
            return Err(ParseError::InvalidHunk {
                message: format!(
                    "Expected update hunk to start with a @@ context marker, got: '{first}'"
                ),
                line_number,
            });
        };

    let body = lines.get(start_index..).unwrap_or_default();
    if body.is_empty() {
        return Err(ParseError::InvalidHunk {
            message: "Update hunk does not contain any lines".to_string(),
            line_number: line_number + 1,
        });
    }

    let mut parsed_lines = 0usize;
    for line in body {
        match *line {
            EOF_MARKER => {
                if parsed_lines == 0 {
                    return Err(ParseError::InvalidHunk {
                        message: "Update hunk does not contain any lines".to_string(),
                        line_number: line_number + 1,
                    });
                }
                parsed_lines += 1;
                break;
            }
            "" => parsed_lines += 1,
            line_contents => {
                match line_contents.chars().next() {
                    Some(' ' | '+' | '-') => {}
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

    Ok(parsed_lines + start_index)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn paths(patch: &str) -> Vec<String> {
        affected_paths(patch)
            .unwrap()
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn collects_every_hunk_kind_in_order() {
        let patch = "*** Begin Patch\n*** Add File: a.txt\n+x\n*** Update File: src/main.rs\n*** Move to: src/lib.rs\n@@\n-fn main() {}\n+fn main() { }\n*** Delete File: old.rs\n*** End Patch";
        assert_eq!(
            paths(patch),
            ["a.txt", "src/main.rs", "src/lib.rs", "old.rs"]
        );
    }

    #[test]
    fn indented_headers_are_hunks() {
        let patch = "*** Begin Patch\n*** Add File: src/ok.rs\n+x\n  *** Add File: .git/config\n+[core]\n*** End Patch";
        assert_eq!(paths(patch), ["src/ok.rs", ".git/config"]);
    }

    #[test]
    fn malformed_patches_are_errors() {
        assert!(matches!(
            affected_paths("*** Add File: a\n+x\n*** End Patch"),
            Err(ParseError::InvalidPatch(_))
        ));
        assert!(matches!(
            affected_paths("*** Begin Patch\n*** Add File: a\n+x"),
            Err(ParseError::InvalidPatch(_))
        ));
        assert!(matches!(
            affected_paths("*** Begin Patch\n*** Update File: a\n*** End Patch"),
            Err(ParseError::InvalidHunk { .. })
        ));
        assert!(matches!(
            affected_paths("*** Begin Patch\n*** Frobnicate: a\n*** End Patch"),
            Err(ParseError::InvalidHunk { .. })
        ));
        assert!(matches!(
            affected_paths("*** Begin Patch\n*** Update File: a\n@@\n*** End Patch"),
            Err(ParseError::InvalidHunk { .. })
        ));
    }

    #[test]
    fn second_chunk_needs_a_context_marker() {
        let ok = "*** Begin Patch\n*** Update File: a\n-x\n+y\n@@ fn f\n-z\n+w\n*** End Patch";
        assert_eq!(paths(ok), ["a"]);
    }
}
