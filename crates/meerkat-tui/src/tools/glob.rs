//! `glob`: gitignore-aware file search by glob pattern, newest first.

use super::{GLOB, Shared, ToolFailure, ToolOutcome};
use globset::GlobBuilder;
use ignore::WalkBuilder;
use meerkat_core::ToolDef;
use serde::Deserialize;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Maximum number of paths returned.
pub(crate) const MAX_RESULTS: usize = 500;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GlobArgs {
    #[schemars(
        description = "Glob pattern matched against paths relative to the search directory, e.g. `**/*.rs` or `src/*.toml`. `*` does not cross `/`; use `**` for any depth."
    )]
    pub pattern: String,
    #[schemars(description = "Directory to search (default: the project root).")]
    #[serde(default)]
    pub path: Option<String>,
}

pub(crate) fn definition() -> ToolDef {
    ToolDef::new(
        GLOB,
        format!(
            "Find files by glob pattern. Respects .gitignore; hidden files are included unless \
             ignored. Returns one path per line, most recently modified first, at most \
             {MAX_RESULTS} results."
        ),
        meerkat_tools::schema_for::<GlobArgs>(),
    )
}

/// Resolve the directory (or file) a search starts from.
pub(crate) fn search_root(
    shared: &Shared,
    tool: &str,
    path: Option<&str>,
) -> Result<PathBuf, ToolFailure> {
    let Some(raw) = path else {
        return Ok(shared.root.as_path().to_path_buf());
    };
    let resolved = shared.resolve(tool, raw)?;
    if !resolved.abs.exists() {
        return Err(ToolFailure::recoverable(format!(
            "path not found: {}",
            shared.display(&resolved)
        )));
    }
    Ok(resolved.abs)
}

/// A gitignore-aware walker that includes hidden files but never descends
/// into `.git`; with `hide_secrets` it also skips environment files.
pub(crate) fn walker(root: &Path, hide_secrets: bool) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .parents(true)
        .require_git(false)
        .filter_entry(move |entry| {
            entry.file_name() != ".git"
                && !(hide_secrets && crate::paths::is_secret_file_name(entry.file_name()))
        });
    builder
}

pub(crate) fn run(shared: &Shared, _id: String, args: GlobArgs) -> ToolOutcome {
    let matcher = GlobBuilder::new(&args.pattern)
        .literal_separator(true)
        .build()
        .map_err(|err| ToolFailure::recoverable(format!("invalid glob pattern: {err}")))?
        .compile_matcher();
    let root = search_root(shared, GLOB, args.path.as_deref())?;
    if !root.is_dir() {
        return Err(ToolFailure::recoverable(format!(
            "{} is not a directory",
            shared.display_abs(&root)
        )));
    }

    let mut hits: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in walker(&root, shared.hide_secrets).build().flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        if !matcher.is_match(relative) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        hits.push((mtime, entry.into_path()));
    }

    if hits.is_empty() {
        return Ok(format!("No files matched `{}`", args.pattern));
    }
    hits.sort_by(|(ta, pa), (tb, pb)| tb.cmp(ta).then_with(|| pa.cmp(pb)));
    let total = hits.len();
    let mut out = String::new();
    for (_, path) in hits.iter().take(MAX_RESULTS) {
        let _ = writeln!(out, "{}", shared.display_abs(path));
    }
    if total > MAX_RESULTS {
        let _ = writeln!(
            out,
            "… [results truncated: showing {MAX_RESULTS} of {total} matches; use a narrower pattern or path]"
        );
    }
    Ok(out)
}
