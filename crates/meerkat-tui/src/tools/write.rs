//! `write_file`: create or overwrite a file (read-before-overwrite enforced).

use super::{Shared, ToolOutcome, WRITE_FILE};
use meerkat_core::ToolDef;
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct WriteFileArgs {
    #[schemars(
        description = "Path to the file, absolute or relative to the project root. Parent directories are created."
    )]
    pub path: String,
    #[schemars(description = "The complete new content of the file.")]
    pub content: String,
}

pub(crate) fn definition() -> ToolDef {
    ToolDef::new(
        WRITE_FILE,
        "Create a new file or completely replace an existing one. An existing file \
                      must have been read with read_file first (and not changed since). Prefer \
                      edit_file for modifying part of an existing file."
            .to_string(),
        meerkat_tools::schema_for::<WriteFileArgs>(),
    )
}

pub(crate) fn run(shared: &Shared, id: String, args: WriteFileArgs) -> ToolOutcome {
    let resolved = shared.resolve(WRITE_FILE, &args.path)?;
    let existing = shared.check_writable(&args.path, &resolved)?;
    let display = shared.display(&resolved);
    let created = existing.is_none();
    let old = existing.unwrap_or_default();
    let diff = shared.commit_write(id, &resolved, &old, &args.content)?;
    let lines = args.content.lines().count();
    Ok(if created {
        format!("created {display} ({lines} lines)")
    } else {
        format!("updated {display} {}", diff.stats())
    })
}
