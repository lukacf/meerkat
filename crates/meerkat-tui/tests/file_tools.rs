#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for the in-crate file tools.

use meerkat_core::{AgentToolDispatcher, ToolCallView, ToolError, ToolResult};
use meerkat_tui::paths::ProjectRoot;
use meerkat_tui::tools::{FILE_TOOL_NAMES, FileDiff, FileToolDispatcher};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::sync::mpsc;

struct Fixture {
    dir: TempDir,
    tools: FileToolDispatcher,
    diffs: mpsc::UnboundedReceiver<FileDiff>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = ProjectRoot::new(dir.path()).unwrap();
        let (tx, diffs) = mpsc::unbounded_channel();
        Self {
            dir,
            tools: FileToolDispatcher::new(root, Some(tx)),
            diffs,
        }
    }

    fn path(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).unwrap()
    }

    async fn try_call(&self, name: &str, args: Value) -> Result<ToolResult, ToolError> {
        let raw = serde_json::value::to_raw_value(&args).unwrap();
        let view = ToolCallView {
            id: "call-1",
            name,
            args: &raw,
        };
        self.tools
            .dispatch(view)
            .await
            .map(|outcome| outcome.result)
    }

    async fn call(&self, name: &str, args: Value) -> (String, bool) {
        let result = self.try_call(name, args).await.unwrap();
        assert_eq!(result.tool_use_id, "call-1");
        (result.text_content(), result.is_error)
    }

    async fn ok(&self, name: &str, args: Value) -> String {
        let (text, is_error) = self.call(name, args).await;
        assert!(!is_error, "{name} unexpectedly failed: {text}");
        text
    }

    async fn err(&self, name: &str, args: Value) -> String {
        let (text, is_error) = self.call(name, args).await;
        assert!(is_error, "{name} unexpectedly succeeded: {text}");
        text
    }
}

fn set_mtime(path: &Path, time: SystemTime) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(time).unwrap();
}

// ---------------------------------------------------------------- tools() ---

#[test]
fn tools_are_exactly_the_five_with_object_schemas() {
    let fx = Fixture::new();
    let defs = fx.tools.tools();
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        ["read_file", "write_file", "edit_file", "glob", "grep"]
    );
    assert_eq!(names, FILE_TOOL_NAMES);

    let expected_required: [(&str, &[&str]); 5] = [
        ("read_file", &["path"]),
        ("write_file", &["path", "content"]),
        ("edit_file", &["path", "old_string", "new_string"]),
        ("glob", &["pattern"]),
        ("grep", &["pattern"]),
    ];
    for (name, required) in expected_required {
        let def = defs.iter().find(|d| d.name == name).unwrap();
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object", "{name} schema: {schema}");
        assert!(schema["properties"].is_object());
        let got: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for field in required {
            assert!(got.contains(field), "{name} must require {field}: {got:?}");
        }
        assert_eq!(got.len(), required.len(), "{name} required: {got:?}");
        assert!(!def.description.is_empty());
    }

    // The output_mode enum is inlined, not a $ref.
    let grep = defs.iter().find(|d| d.name == "grep").unwrap();
    let mode = grep.input_schema["properties"]["output_mode"].to_string();
    assert!(mode.contains("files_with_matches"), "{mode}");
    assert!(!mode.contains("$ref"), "{mode}");

    // tools() is cached.
    assert!(std::sync::Arc::ptr_eq(&defs, &fx.tools.tools()));
}

#[tokio::test]
async fn unknown_tool_and_bad_args_are_tool_errors() {
    let fx = Fixture::new();
    assert!(matches!(
        fx.try_call("nope", json!({})).await,
        Err(ToolError::NotFound { .. })
    ));
    assert!(matches!(
        fx.try_call("read_file", json!({"offset": 1})).await,
        Err(ToolError::InvalidArguments { .. })
    ));
    assert!(matches!(
        fx.try_call("read_file", json!({"path": ""})).await,
        Err(ToolError::InvalidArguments { .. })
    ));
}

// -------------------------------------------------------------- read_file ---

#[tokio::test]
async fn read_numbers_lines_with_offset_and_limit() {
    let fx = Fixture::new();
    let body: String = (1..=10).fold(String::new(), |mut acc, i| {
        acc.push_str(&format!("line {i}\n"));
        acc
    });
    fx.write("a.txt", &body);

    let all = fx.ok("read_file", json!({"path": "a.txt"})).await;
    assert!(all.starts_with("     1\tline 1\n     2\tline 2\n"));
    assert!(all.contains("    10\tline 10\n"));
    assert!(!all.contains("showing lines"));

    let window = fx
        .ok(
            "read_file",
            json!({"path": "a.txt", "offset": 3, "limit": 2}),
        )
        .await;
    assert!(window.starts_with("     3\tline 3\n     4\tline 4\n"));
    assert!(!window.contains("line 5\n"));
    assert!(window.contains("showing lines 3-4 of 10"));
}

#[tokio::test]
async fn read_truncates_long_lines() {
    let fx = Fixture::new();
    fx.write("long.txt", "x".repeat(5000));
    let out = fx.ok("read_file", json!({"path": "long.txt"})).await;
    assert!(out.contains("[line truncated: 5000 chars]"));
    assert!(out.len() < 2200);
}

#[tokio::test]
async fn read_detects_binary_missing_and_directories() {
    let fx = Fixture::new();
    fx.write("bin.dat", b"abc\0def");
    assert!(
        fx.err("read_file", json!({"path": "bin.dat"}))
            .await
            .contains("binary file")
    );
    assert!(
        fx.err("read_file", json!({"path": "missing.txt"}))
            .await
            .contains("not found")
    );
    std::fs::create_dir(fx.path("dir")).unwrap();
    assert!(
        fx.err("read_file", json!({"path": "dir"}))
            .await
            .contains("directory")
    );
}

#[tokio::test]
async fn read_normalizes_dotdot_within_root() {
    let fx = Fixture::new();
    fx.write("src/a.txt", "hello\n");
    let out = fx
        .ok("read_file", json!({"path": "src/../src/./a.txt"}))
        .await;
    assert!(out.contains("hello"));
}

#[tokio::test]
async fn read_outside_root_is_not_refused_by_dispatcher() {
    let fx = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    let file = outside.path().join("o.txt");
    std::fs::write(&file, "outside\n").unwrap();
    let out = fx
        .ok("read_file", json!({"path": file.to_str().unwrap()}))
        .await;
    assert!(out.contains("outside"));
}

// ------------------------------------------------------------- write_file ---

#[tokio::test]
async fn write_creates_new_files_with_parents_and_emits_diff() {
    let mut fx = Fixture::new();
    let out = fx
        .ok(
            "write_file",
            json!({"path": "deep/nested/new.txt", "content": "one\ntwo\n"}),
        )
        .await;
    assert!(out.contains("created deep/nested/new.txt"), "{out}");
    assert_eq!(fx.read("deep/nested/new.txt"), "one\ntwo\n");

    let diff = fx.diffs.try_recv().unwrap();
    assert_eq!(diff.tool_use_id, "call-1");
    assert_eq!(diff.path, Path::new("deep/nested/new.txt"));
    assert_eq!((diff.added, diff.removed), (2, 0));
    assert!(diff.unified.contains("+++ b/deep/nested/new.txt"));

    // A file the tool wrote itself may be overwritten without reading it.
    fx.ok(
        "write_file",
        json!({"path": "deep/nested/new.txt", "content": "three\n"}),
    )
    .await;
    assert_eq!(fx.read("deep/nested/new.txt"), "three\n");
}

#[tokio::test]
async fn write_refuses_unread_existing_file_then_succeeds_after_read() {
    let mut fx = Fixture::new();
    fx.write("a.txt", "old\n");
    let msg = fx
        .err("write_file", json!({"path": "a.txt", "content": "new\n"}))
        .await;
    assert!(msg.contains("read_file"), "{msg}");
    assert_eq!(fx.read("a.txt"), "old\n");
    assert!(fx.diffs.try_recv().is_err());

    fx.ok("read_file", json!({"path": "a.txt"})).await;
    let out = fx
        .ok("write_file", json!({"path": "a.txt", "content": "new\n"}))
        .await;
    assert!(out.contains("updated a.txt (+1 −1)"), "{out}");
    assert_eq!(fx.read("a.txt"), "new\n");
    let diff = fx.diffs.try_recv().unwrap();
    assert!(diff.unified.contains("-old\n"));
    assert!(diff.unified.contains("+new\n"));
}

#[tokio::test]
async fn write_refuses_stale_file() {
    let fx = Fixture::new();
    fx.write("a.txt", "v1\n");
    fx.ok("read_file", json!({"path": "a.txt"})).await;
    fx.write("a.txt", "v2 from user\n");
    set_mtime(
        &fx.path("a.txt"),
        SystemTime::now() + Duration::from_mins(1),
    );
    let msg = fx
        .err("write_file", json!({"path": "a.txt", "content": "v3\n"}))
        .await;
    assert!(msg.contains("modified since"), "{msg}");
    assert_eq!(fx.read("a.txt"), "v2 from user\n");
}

#[tokio::test]
async fn write_to_directory_is_an_error() {
    let fx = Fixture::new();
    std::fs::create_dir(fx.path("d")).unwrap();
    let msg = fx
        .err("write_file", json!({"path": "d", "content": "x"}))
        .await;
    assert!(msg.contains("directory"), "{msg}");
}

#[cfg(unix)]
#[tokio::test]
async fn write_refuses_symlink_final_component() {
    let fx = Fixture::new();
    fx.write("target.txt", "target\n");
    std::os::unix::fs::symlink(fx.path("target.txt"), fx.path("link.txt")).unwrap();
    // Even after reading through the link, writing through it is refused.
    fx.ok("read_file", json!({"path": "link.txt"})).await;
    let msg = fx
        .err("write_file", json!({"path": "link.txt", "content": "x"}))
        .await;
    assert!(msg.contains("symlink"), "{msg}");
    let msg = fx
        .err(
            "edit_file",
            json!({"path": "link.txt", "old_string": "target", "new_string": "x"}),
        )
        .await;
    assert!(msg.contains("symlink"), "{msg}");
    assert_eq!(fx.read("target.txt"), "target\n");

    // Dangling symlinks are refused too.
    std::os::unix::fs::symlink(fx.path("nowhere.txt"), fx.path("dangling.txt")).unwrap();
    let msg = fx
        .err(
            "write_file",
            json!({"path": "dangling.txt", "content": "x"}),
        )
        .await;
    assert!(msg.contains("symlink"), "{msg}");
    assert!(!fx.path("nowhere.txt").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn write_through_symlinked_dir_escaping_root_is_allowed_by_dispatcher() {
    // Policy lives in the approval layer; the dispatcher only refuses symlinked
    // final components. A symlinked *parent* resolves to its target.
    let fx = Fixture::new();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), fx.path("ext")).unwrap();
    fx.ok(
        "write_file",
        json!({"path": "ext/new.txt", "content": "x\n"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(outside.path().join("new.txt")).unwrap(),
        "x\n"
    );
}

// -------------------------------------------------------------- edit_file ---

#[tokio::test]
async fn edit_requires_read_and_applies_single_match() {
    let mut fx = Fixture::new();
    fx.write("src/x.rs", "fn a() {}\nfn b() {}\n");
    let msg = fx
        .err(
            "edit_file",
            json!({"path": "src/x.rs", "old_string": "fn a", "new_string": "fn z"}),
        )
        .await;
    assert!(msg.contains("read_file"), "{msg}");

    fx.ok("read_file", json!({"path": "src/x.rs"})).await;
    let out = fx
        .ok(
            "edit_file",
            json!({"path": "src/x.rs", "old_string": "fn a() {}\n", "new_string": "fn a() {}\nfn c() {}\nfn d() {}\n"}),
        )
        .await;
    assert_eq!(out, "edited src/x.rs (+2 −0)");
    assert_eq!(
        fx.read("src/x.rs"),
        "fn a() {}\nfn c() {}\nfn d() {}\nfn b() {}\n"
    );
    let diff = fx.diffs.try_recv().unwrap();
    assert_eq!(diff.path, Path::new("src/x.rs"));
    assert_eq!((diff.added, diff.removed), (2, 0));

    // Consecutive edits work without re-reading (the tool records its own write).
    fx.ok(
        "edit_file",
        json!({"path": "src/x.rs", "old_string": "fn b", "new_string": "fn bb"}),
    )
    .await;
    assert!(fx.read("src/x.rs").contains("fn bb"));
}

#[tokio::test]
async fn edit_multi_match_and_replace_all() {
    let fx = Fixture::new();
    fx.write("a.txt", "foo\nfoo\nfoo\n");
    fx.ok("read_file", json!({"path": "a.txt"})).await;
    let msg = fx
        .err(
            "edit_file",
            json!({"path": "a.txt", "old_string": "foo", "new_string": "bar"}),
        )
        .await;
    assert!(msg.contains("3 times"), "{msg}");
    assert_eq!(fx.read("a.txt"), "foo\nfoo\nfoo\n");

    let out = fx
        .ok(
            "edit_file",
            json!({"path": "a.txt", "old_string": "foo", "new_string": "bar", "replace_all": true}),
        )
        .await;
    assert_eq!(out, "edited a.txt (+3 −3)");
    assert_eq!(fx.read("a.txt"), "bar\nbar\nbar\n");
}

#[tokio::test]
async fn edit_no_match_identical_and_missing_file() {
    let fx = Fixture::new();
    fx.write("a.txt", "abc\n");
    fx.ok("read_file", json!({"path": "a.txt"})).await;
    assert!(
        fx.err(
            "edit_file",
            json!({"path": "a.txt", "old_string": "zzz", "new_string": "y"})
        )
        .await
        .contains("not found")
    );
    assert!(
        fx.err(
            "edit_file",
            json!({"path": "a.txt", "old_string": "abc", "new_string": "abc"})
        )
        .await
        .contains("identical")
    );
    assert!(
        fx.err(
            "edit_file",
            json!({"path": "nope.txt", "old_string": "a", "new_string": "b"})
        )
        .await
        .contains("not found")
    );
    assert!(matches!(
        fx.try_call(
            "edit_file",
            json!({"path": "a.txt", "old_string": "", "new_string": "b"})
        )
        .await,
        Err(ToolError::InvalidArguments { .. })
    ));
}

#[tokio::test]
async fn edit_detects_stale_file() {
    let fx = Fixture::new();
    fx.write("a.txt", "one\n");
    fx.ok("read_file", json!({"path": "a.txt"})).await;
    fx.write("a.txt", "one changed\n");
    set_mtime(
        &fx.path("a.txt"),
        SystemTime::now() + Duration::from_mins(2),
    );
    let msg = fx
        .err(
            "edit_file",
            json!({"path": "a.txt", "old_string": "one", "new_string": "two"}),
        )
        .await;
    assert!(msg.contains("modified since"), "{msg}");

    // Re-reading clears the staleness.
    fx.ok("read_file", json!({"path": "a.txt"})).await;
    fx.ok(
        "edit_file",
        json!({"path": "a.txt", "old_string": "one", "new_string": "two"}),
    )
    .await;
    assert_eq!(fx.read("a.txt"), "two changed\n");
}

// ------------------------------------------------------------------- glob ---

#[tokio::test]
async fn glob_respects_gitignore_and_sorts_by_mtime() {
    let fx = Fixture::new();
    fx.write(".gitignore", "target/\n*.log\n");
    fx.write("src/old.rs", "");
    fx.write("src/new.rs", "");
    fx.write("src/mid.rs", "");
    fx.write("target/gen.rs", "");
    fx.write("debug.log", "");
    fx.write(".hidden/h.rs", "");
    fx.write(".git/x.rs", "");
    let now = SystemTime::now();
    set_mtime(&fx.path("src/old.rs"), now - Duration::from_mins(5));
    set_mtime(&fx.path("src/mid.rs"), now - Duration::from_secs(200));
    set_mtime(&fx.path("src/new.rs"), now - Duration::from_secs(100));
    set_mtime(&fx.path(".hidden/h.rs"), now - Duration::from_secs(400));

    let out = fx.ok("glob", json!({"pattern": "**/*.rs"})).await;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(
        lines,
        ["src/new.rs", "src/mid.rs", "src/old.rs", ".hidden/h.rs"]
    );

    // `*` does not cross directories.
    let top = fx.ok("glob", json!({"pattern": "*.rs"})).await;
    assert!(top.contains("No files matched"), "{top}");

    // Relative to an explicit search path.
    let scoped = fx
        .ok("glob", json!({"pattern": "*.rs", "path": "src"}))
        .await;
    assert_eq!(scoped.lines().count(), 3);
    assert!(scoped.lines().all(|l| l.starts_with("src/")), "{scoped}");

    assert!(
        fx.err("glob", json!({"pattern": "*", "path": "missing"}))
            .await
            .contains("not found")
    );
    assert!(
        fx.err("glob", json!({"pattern": "a[", "path": "src"}))
            .await
            .contains("invalid glob")
    );
}

#[tokio::test]
async fn glob_caps_results() {
    let fx = Fixture::new();
    for i in 0..510 {
        fx.write(&format!("many/f{i}.txt"), "");
    }
    let out = fx.ok("glob", json!({"pattern": "many/*.txt"})).await;
    assert_eq!(out.lines().count(), 501);
    assert!(
        out.contains("showing 500 of 510"),
        "{}",
        out.lines().last().unwrap()
    );
}

// ------------------------------------------------------------------- grep ---

fn grep_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.write(".gitignore", "ignored/\n");
    fx.write(
        "src/a.rs",
        "fn main() {\n    let x = 1;\n    todo_item();\n}\n",
    );
    fx.write("src/b.rs", "// TODO_ITEM later\nfn b() {}\n");
    fx.write("notes.md", "todo_item in docs\n");
    fx.write("ignored/c.rs", "todo_item\n");
    fx.write("bin.dat", b"todo_item\0binary");
    fx
}

#[tokio::test]
async fn grep_files_with_matches_default() {
    let fx = grep_fixture();
    let out = fx.ok("grep", json!({"pattern": "todo_item"})).await;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines, ["notes.md", "src/a.rs"]);

    let ci = fx
        .ok(
            "grep",
            json!({"pattern": "todo_item", "case_insensitive": true}),
        )
        .await;
    assert_eq!(
        ci.lines().collect::<Vec<_>>(),
        ["notes.md", "src/a.rs", "src/b.rs"]
    );

    let filtered = fx
        .ok(
            "grep",
            json!({"pattern": "todo_item", "glob": "*.rs", "case_insensitive": true}),
        )
        .await;
    assert_eq!(
        filtered.lines().collect::<Vec<_>>(),
        ["src/a.rs", "src/b.rs"]
    );

    let scoped = fx
        .ok("grep", json!({"pattern": "todo_item", "path": "src/a.rs"}))
        .await;
    assert_eq!(scoped.trim(), "src/a.rs");
}

#[tokio::test]
async fn grep_content_and_count_modes() {
    let fx = grep_fixture();
    let content = fx
        .ok(
            "grep",
            json!({"pattern": "todo_item", "output_mode": "content", "path": "src"}),
        )
        .await;
    assert_eq!(content, "src/a.rs:3:    todo_item();\n");

    let ctx = fx
        .ok(
            "grep",
            json!({"pattern": "todo_item", "output_mode": "content", "path": "src", "context": 1}),
        )
        .await;
    assert_eq!(
        ctx,
        "src/a.rs-2-    let x = 1;\nsrc/a.rs:3:    todo_item();\nsrc/a.rs-4-}\n"
    );

    fx.write("src/many.rs", "x\nx\nx\n");
    let count = fx
        .ok("grep", json!({"pattern": "^x$", "output_mode": "count"}))
        .await;
    assert_eq!(count, "src/many.rs:3\n");
}

#[tokio::test]
async fn grep_head_limit_no_match_and_bad_regex() {
    let fx = grep_fixture();
    fx.write("src/many.rs", "x\nx\nx\nx\n");
    let out = fx
        .ok(
            "grep",
            json!({"pattern": "^x$", "output_mode": "content", "head_limit": 2}),
        )
        .await;
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[2].contains("truncated"), "{out}");

    let none = fx.ok("grep", json!({"pattern": "zzz_nothing"})).await;
    assert!(none.contains("No matches"));

    assert!(
        fx.err("grep", json!({"pattern": "("}))
            .await
            .contains("invalid regex")
    );
    assert!(matches!(
        fx.try_call("grep", json!({"pattern": "x", "output_mode": "lines"}))
            .await,
        Err(ToolError::InvalidArguments { .. })
    ));
}

// ------------------------------------------------------ non-regular files ----

/// Create a FIFO at `path` with `mkfifo`; `false` (after a `SKIPPED:` line)
/// when the tool is unavailable.
#[cfg(unix)]
#[allow(clippy::disallowed_methods)] // test-only process launch
fn make_fifo(path: &Path) -> bool {
    match std::process::Command::new("mkfifo").arg(path).status() {
        Ok(status) if status.success() => true,
        other => {
            println!("SKIPPED: mkfifo unavailable ({other:?})");
            false
        }
    }
}

/// Run a tool call, failing the test if it does not return within 10 s.
async fn promptly<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(10), fut)
        .await
        .unwrap_or_else(|_| panic!("{what} hung on a non-regular file"))
}

#[cfg(unix)]
#[tokio::test]
async fn fifo_is_refused_promptly_by_read_grep_and_read_only_tools() {
    use meerkat_tui::tools::ReadOnlyProjectTools;

    let fx = Fixture::new();
    fx.write("plain.txt", "needle\n");
    if !make_fifo(&fx.path("pipe")) {
        return;
    }

    let text = promptly("read_file", fx.err("read_file", json!({"path": "pipe"}))).await;
    assert!(text.contains("not a regular file"), "{text}");
    assert!(text.contains("named pipe"), "{text}");

    let text = promptly(
        "grep path",
        fx.err("grep", json!({"pattern": "needle", "path": "pipe"})),
    )
    .await;
    assert!(text.contains("not a regular file"), "{text}");

    // A walk skips the FIFO and still finds the regular file.
    let text = promptly("grep walk", fx.ok("grep", json!({"pattern": "needle"}))).await;
    assert!(text.contains("plain.txt"), "{text}");
    assert!(!text.contains("pipe"), "{text}");

    // Writing over a FIFO is refused too (never opened for reading).
    let text = promptly(
        "write_file",
        fx.err("write_file", json!({"path": "pipe", "content": "x"})),
    )
    .await;
    assert!(text.contains("not a regular file"), "{text}");

    let ro = ReadOnlyProjectTools::new(ProjectRoot::new(fx.dir.path()).unwrap());
    for (name, args) in [
        ("read_file", json!({"path": "pipe"})),
        ("grep", json!({"pattern": "x", "path": "pipe"})),
    ] {
        let raw = serde_json::value::to_raw_value(&args).unwrap();
        let result = promptly(
            name,
            ro.dispatch(ToolCallView {
                id: "c1",
                name,
                args: &raw,
            }),
        )
        .await
        .unwrap()
        .result;
        assert!(result.is_error, "{name}: {}", result.text_content());
        assert!(
            result.text_content().contains("not a regular file"),
            "{name}: {}",
            result.text_content()
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn socket_is_refused_by_read_and_grep() {
    let fx = Fixture::new();
    let _listener = std::os::unix::net::UnixListener::bind(fx.path("sock")).unwrap();
    let text = promptly("read_file", fx.err("read_file", json!({"path": "sock"}))).await;
    assert!(text.contains("socket"), "{text}");
    let text = promptly(
        "grep",
        fx.err("grep", json!({"pattern": "x", "path": "sock"})),
    )
    .await;
    assert!(text.contains("not a regular file"), "{text}");
}

#[tokio::test]
async fn reads_are_capped_and_say_so() {
    use meerkat_tui::tools::MAX_FILE_READ_BYTES;

    let fx = Fixture::new();
    let cap = usize::try_from(MAX_FILE_READ_BYTES).unwrap();
    // One line of `a`s up to the cap, then a marker line past it.
    let mut content = vec![b'a'; cap - 1];
    content.push(b'\n');
    content.extend_from_slice(b"BEYOND_THE_CAP\n");
    fx.write("big.txt", &content);

    let text = fx.ok("read_file", json!({"path": "big.txt"})).await;
    assert!(
        text.contains("file truncated"),
        "{}",
        &text[text.len() - 200..]
    );
    assert!(!text.contains("BEYOND_THE_CAP"));

    // grep searches only the capped prefix.
    let text = fx.ok("grep", json!({"pattern": "BEYOND_THE_CAP"})).await;
    assert!(text.contains("No matches"), "{text}");

    // A file over the cap is never rewritten from its truncated prefix.
    let text = fx
        .err(
            "edit_file",
            json!({"path": "big.txt", "old_string": "aaa", "new_string": "b"}),
        )
        .await;
    assert!(text.contains("larger than"), "{text}");
    assert_eq!(
        std::fs::metadata(fx.path("big.txt")).unwrap().len(),
        content.len() as u64
    );
}

// ------------------------------------------------------ per-session reads ----

async fn call_as(
    tools: &FileToolDispatcher,
    session: Option<&meerkat_core::SessionId>,
    name: &str,
    args: Value,
) -> ToolResult {
    let raw = serde_json::value::to_raw_value(&args).unwrap();
    let view = ToolCallView {
        id: "call-1",
        name,
        args: &raw,
    };
    let outcome = match session {
        Some(session) => {
            let context = meerkat_core::ToolDispatchContext::default()
                .with_runtime_identity(session.clone(), None);
            tools.dispatch_with_context(view, &context).await
        }
        None => tools.dispatch(view).await,
    };
    outcome.unwrap().result
}

#[tokio::test]
async fn a_read_satisfies_only_the_reading_sessions_writes() {
    let fx = Fixture::new();
    fx.write("shared.txt", "one\n");
    let a = meerkat_core::SessionId::new();
    let b = meerkat_core::SessionId::new();

    let read = call_as(
        &fx.tools,
        Some(&a),
        "read_file",
        json!({"path": "shared.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.text_content());

    for args in [
        json!({"path": "shared.txt", "content": "two\n"}),
        json!({"path": "shared.txt", "old_string": "one", "new_string": "two"}),
    ] {
        let name = if args.get("content").is_some() {
            "write_file"
        } else {
            "edit_file"
        };
        // Neither another session nor a context-free call inherits A's read.
        for other in [Some(&b), None] {
            let result = call_as(&fx.tools, other, name, args.clone()).await;
            assert!(
                result.is_error,
                "{name} as {other:?}: {}",
                result.text_content()
            );
            assert!(result.text_content().contains("has not been read"));
        }
    }
    assert_eq!(fx.read("shared.txt"), "one\n");

    let ok = call_as(
        &fx.tools,
        Some(&a),
        "edit_file",
        json!({"path": "shared.txt", "old_string": "one", "new_string": "two"}),
    )
    .await;
    assert!(!ok.is_error, "{}", ok.text_content());
    assert_eq!(fx.read("shared.txt"), "two\n");
}

#[tokio::test]
async fn an_owned_dispatcher_keys_context_free_calls_by_its_session() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "x\n").unwrap();
    let owner = meerkat_core::SessionId::new();
    let tools =
        FileToolDispatcher::for_session(ProjectRoot::new(dir.path()).unwrap(), None, owner.clone());
    // A context-free read is the owner's, so the owner's contextual write passes.
    let read = call_as(&tools, None, "read_file", json!({"path": "f.txt"})).await;
    assert!(!read.is_error);
    let other = meerkat_core::SessionId::new();
    let refused = call_as(
        &tools,
        Some(&other),
        "write_file",
        json!({"path": "f.txt", "content": "y\n"}),
    )
    .await;
    assert!(refused.is_error);
    let ok = call_as(
        &tools,
        Some(&owner),
        "write_file",
        json!({"path": "f.txt", "content": "y\n"}),
    )
    .await;
    assert!(!ok.is_error, "{}", ok.text_content());
}
