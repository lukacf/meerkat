#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Drift pins: local copies of upstream (`meerkat-tools`) grammars and
//! argument types, checked against the live upstream tools so an upstream
//! change fails here instead of silently desynchronizing the approval policy.

use meerkat_tools::builtin::BuiltinTool;
use meerkat_tools::builtin::shell::{ShellConfig, ShellTool};
use meerkat_tools::builtin::utility::ApplyPatchTool;
use meerkat_tui::approval::patch_targets::affected_paths;
use meerkat_tui::approval::policy::ShellArgs;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------- shell ----

/// The field names `ShellArgs` accepts, read from its own
/// `deny_unknown_fields` error (it derives no schema).
fn shell_args_fields() -> BTreeSet<String> {
    let err = serde_json::from_value::<ShellArgs>(json!({"command": "x", "zz_unknown": 1}))
        .expect_err("ShellArgs must deny unknown fields");
    let message = err.to_string();
    let (_, expected) = message
        .split_once("expected one of ")
        .unwrap_or_else(|| panic!("unexpected serde message: {message}"));
    expected
        .split(',')
        .map(|field| field.trim().trim_matches('`').to_string())
        .filter(|field| !field.is_empty())
        .collect()
}

#[test]
fn shell_args_match_the_live_shell_tool_schema() {
    let def = ShellTool::new(ShellConfig::default()).def();
    assert_eq!(def.name.as_str(), "shell");
    let schema = &def.input_schema;
    let properties: BTreeSet<String> = schema["properties"]
        .as_object()
        .expect("shell schema has properties")
        .keys()
        .cloned()
        .collect();
    let required: BTreeSet<String> = schema["required"]
        .as_array()
        .expect("shell schema has a required list")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();

    let local = shell_args_fields();
    assert_eq!(
        local,
        ["command", "working_dir", "timeout_secs", "background"]
            .map(String::from)
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "ShellArgs' own field set changed"
    );
    // Upstream lists `background` only when its job manager exports
    // canonical async ops (a durable job runtime bound to a session); every
    // other field is always listed.
    let mut upstream = properties;
    upstream.insert("background".to_string());
    assert_eq!(
        upstream, local,
        "upstream shell input fields drifted from crate::approval::policy::ShellArgs"
    );
    assert_eq!(
        required,
        BTreeSet::from(["command".to_string()]),
        "upstream shell required fields drifted"
    );
    // ShellArgs requires exactly `command` too.
    assert!(serde_json::from_value::<ShellArgs>(json!({})).is_err());
    assert!(serde_json::from_value::<ShellArgs>(json!({"command": "ls"})).is_ok());
}

// ---------------------------------------------------------- apply_patch ----

const EXAMPLE_HEADING: &str = "Example (update, move, add, delete):\n";

fn apply_patch_description() -> String {
    ApplyPatchTool::new(PathBuf::from("/nonexistent"))
        .def()
        .description
}

/// The patch grammar the local parser implements, as the upstream tool
/// documents it to the model.
#[test]
fn apply_patch_header_grammar_matches_upstream_description() {
    let description = apply_patch_description();
    for marker in [
        "'*** Begin Patch'",
        "'*** End Patch'",
        "'*** Add File: <path>'",
        "'*** Delete File: <path>'",
        "'*** Update File: <path>'",
        "'*** Move to: <new_path>'",
        "'*** End of File'",
        "'@@ <anchor>'",
        "bare '@@'",
    ] {
        assert!(
            description.contains(marker),
            "upstream apply_patch grammar no longer documents {marker}: {description}"
        );
    }
}

/// Drop `.` components so `./a` and `a` compare equal.
fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// Every regular file under `root` with its content, relative to `root`.
fn snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                stack.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_path_buf();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    out
}

/// Paths whose existence or content differs between two snapshots.
fn changed(
    before: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
    after: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

async fn upstream_apply(root: &Path, patch: &str) -> Result<Value, String> {
    ApplyPatchTool::new(root.to_path_buf())
        .call(json!({ "patch": patch }))
        .await
        .map(|output| match output {
            meerkat_tools::builtin::ToolOutput::Json(value) => value,
            other => panic!("unexpected apply_patch output: {other:?}"),
        })
        .map_err(|err| err.to_string())
}

struct Fixture {
    name: &'static str,
    files: Vec<(&'static str, &'static str)>,
    patch: String,
}

fn wrap(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

fn fixtures() -> Vec<Fixture> {
    let description = apply_patch_description();
    let (_, example) = description
        .split_once(EXAMPLE_HEADING)
        .expect("upstream description carries its documented example");
    vec![
        Fixture {
            name: "documented example",
            files: vec![
                (
                    "src/config.rs",
                    "fn defaults() {\n    let timeout = 30;\n    let retries = 3;\n}\n",
                ),
                ("src/old_name.rs", "fn old_func() {}\n"),
                ("src/deprecated.rs", "// old\n"),
            ],
            patch: example.to_string(),
        },
        Fixture {
            name: "add nested",
            files: vec![],
            patch: wrap("*** Add File: nested/new.txt\n+hello"),
        },
        Fixture {
            name: "two adds",
            files: vec![],
            patch: wrap("*** Add File: a.txt\n+a\n*** Add File: b.txt\n+b"),
        },
        Fixture {
            name: "add, update with move, delete",
            files: vec![("source.txt", "a\nb\n"), ("delete.txt", "gone\n")],
            patch: wrap(
                "*** Add File: nested/new.txt\n+hello\n+world\n*** Update File: source.txt\n*** Move to: moved.txt\n@@\n a\n-b\n+c\n*** Delete File: delete.txt",
            ),
        },
        Fixture {
            name: "end of file marker",
            files: vec![("tail.txt", "alpha\nbeta\n")],
            patch: wrap("*** Update File: tail.txt\n@@\n alpha\n-beta\n+gamma\n*** End of File"),
        },
        Fixture {
            name: "anchored hunk",
            files: vec![("imports.py", "import a\nimport b\n")],
            patch: wrap("*** Update File: imports.py\n@@ import a\n+import m"),
        },
        Fixture {
            name: "move onto itself",
            files: vec![("same.txt", "before\n")],
            patch: wrap("*** Update File: same.txt\n*** Move to: ./same.txt\n@@\n-before\n+after"),
        },
        Fixture {
            name: "first chunk without marker, second anchored",
            files: vec![("a", "x\nfn f\nz\n")],
            patch: wrap("*** Update File: a\n-x\n+y\n@@ fn f\n-z\n+w"),
        },
        Fixture {
            name: "two chunks separated by a blank line",
            files: vec![("b.txt", "one\ntwo\nthree\nfour\n")],
            patch: wrap("*** Update File: b.txt\n@@\n-one\n+uno\n\n@@ three\n-four\n+cuatro"),
        },
        Fixture {
            name: "indented header",
            files: vec![],
            patch: wrap("*** Add File: src/ok.rs\n+x\n  *** Add File: other.txt\n+y"),
        },
        Fixture {
            name: "surrounding whitespace",
            files: vec![("w.txt", "w\n")],
            patch: format!("\n\n{}\n\n", wrap("*** Delete File: w.txt")),
        },
    ]
}

/// Every fixture patch, run through upstream's live tool and the local
/// parser: the files the tool actually changed must be exactly the paths the
/// local parser names, and every path the tool reports must be among them.
#[tokio::test]
async fn apply_patch_targets_match_upstream_effects() {
    for fixture in fixtures() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for (path, content) in &fixture.files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let local: BTreeSet<PathBuf> = affected_paths(&fixture.patch)
            .unwrap_or_else(|err| panic!("{}: local parser refused: {err}", fixture.name))
            .iter()
            .map(|path| normalize(path))
            .collect();

        let before = snapshot(&root);
        let reported = upstream_apply(&root, &fixture.patch)
            .await
            .unwrap_or_else(|err| panic!("{}: upstream refused: {err}", fixture.name));
        let effects = changed(&before, &snapshot(&root));
        assert_eq!(
            effects, local,
            "{}: upstream changed files differ from the local parser's targets",
            fixture.name
        );
        for key in ["added_files", "modified_files", "deleted_files"] {
            for path in reported[key].as_array().unwrap() {
                let path = PathBuf::from(path.as_str().unwrap());
                let relative = normalize(path.strip_prefix(&root).unwrap_or(&path));
                assert!(
                    local.contains(&relative),
                    "{}: upstream reported {key} {} the local parser does not name",
                    fixture.name,
                    relative.display()
                );
            }
        }
    }
}

/// Malformed patches: the local parser refuses exactly when upstream's parser
/// does, with the same message (the tool reports `Execution failed: <parse
/// error>`).
#[tokio::test]
async fn apply_patch_parse_errors_match_upstream() {
    let cases = [
        "*** Add File: a\n+x\n*** End Patch".to_string(),
        "*** Begin Patch\n*** Add File: a\n+x".to_string(),
        wrap("*** Update File: a"),
        wrap("*** Frobnicate: a"),
        wrap("*** Update File: a\n@@"),
        wrap("*** Update File: a\n@@\n*** End of File"),
        wrap("*** Update File: a\n@@\n-x\n+y\nbogus"),
        wrap("*** Update File: a\nbogus"),
        wrap("*** Add File: a\n+x\nstray"),
        String::new(),
    ];
    for patch in cases {
        let dir = tempfile::tempdir().unwrap();
        let local = affected_paths(&patch)
            .err()
            .unwrap_or_else(|| panic!("local parser accepted malformed patch {patch:?}"));
        let before = snapshot(dir.path());
        let upstream = upstream_apply(dir.path(), &patch)
            .await
            .err()
            .unwrap_or_else(|| panic!("upstream accepted malformed patch {patch:?}"));
        assert!(
            upstream.ends_with(&local.to_string()),
            "error drift for {patch:?}: upstream {upstream:?}, local {local:?}"
        );
        assert!(changed(&before, &snapshot(dir.path())).is_empty());
    }
}
