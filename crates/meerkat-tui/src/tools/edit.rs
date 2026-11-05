//! `edit_file`: exact-substring replacement (read-before-edit enforced).

use super::{CallMeta, EDIT_FILE, Shared, ToolFailure, ToolOutcome};
use meerkat_core::{ToolDef, ToolError};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EditFileArgs {
    #[schemars(description = "Path to the file, absolute or relative to the project root.")]
    pub path: String,
    #[schemars(
        description = "Exact text to replace, including whitespace and indentation. Must occur exactly once unless replace_all is true."
    )]
    pub old_string: String,
    #[schemars(description = "Replacement text. Must differ from old_string.")]
    pub new_string: String,
    #[schemars(description = "Replace every occurrence of old_string (default false).")]
    #[serde(default)]
    pub replace_all: bool,
}

pub(crate) fn definition() -> ToolDef {
    ToolDef::new(
        EDIT_FILE,
        "Replace an exact substring in a file. The file must have been read with \
                      read_file first (and not changed since). old_string must match exactly \
                      once; include surrounding lines to make it unique, or set replace_all to \
                      replace every occurrence."
            .to_string(),
        meerkat_tools::schema_for::<EditFileArgs>(),
    )
}

pub(crate) fn run(shared: &Shared, call: CallMeta, args: EditFileArgs) -> ToolOutcome {
    if args.old_string.is_empty() {
        return Err(ToolFailure::Fatal(ToolError::InvalidArguments {
            name: EDIT_FILE.to_string(),
            reason: "old_string must not be empty; use write_file to create a file".to_string(),
        }));
    }
    if args.old_string == args.new_string {
        return Err(ToolFailure::recoverable(
            "old_string and new_string are identical; nothing to change",
        ));
    }
    let resolved = shared.resolve(EDIT_FILE, &args.path)?;
    let display = shared.display(&resolved);
    let Some(content) = shared.check_writable(call.session.as_ref(), &args.path, &resolved)? else {
        return Err(ToolFailure::recoverable(format!(
            "file not found: {display}; use write_file to create it"
        )));
    };

    let updated = apply(
        &content,
        &args.old_string,
        &args.new_string,
        args.replace_all,
    )
    .map_err(|err| ToolFailure::recoverable(format!("{err} in {display}")))?;
    let diff = shared.commit_write(call, &resolved, &content, &updated)?;
    Ok(format!("edited {display} {}", diff.stats()))
}

/// Why an edit could not be applied.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum EditError {
    #[error("old_string not found")]
    NotFound,
    #[error(
        "old_string matches {0} times; add surrounding context to make it unique or set replace_all"
    )]
    Ambiguous(usize),
}

/// Replace `old` with `new` in `content`.
pub(crate) fn apply(
    content: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<String, EditError> {
    match content.matches(old).count() {
        0 => Err(EditError::NotFound),
        1 => Ok(content.replacen(old, new, 1)),
        _ if replace_all => Ok(content.replace(old, new)),
        n => Err(EditError::Ambiguous(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_match_replaces() {
        assert_eq!(apply("a b c", "b", "x", false), Ok("a x c".to_string()));
    }

    #[test]
    fn multi_match_requires_replace_all() {
        assert_eq!(apply("a a", "a", "b", false), Err(EditError::Ambiguous(2)));
        assert_eq!(apply("a a", "a", "b", true), Ok("b b".to_string()));
    }

    #[test]
    fn missing_match() {
        assert_eq!(apply("abc", "z", "y", false), Err(EditError::NotFound));
    }
}
