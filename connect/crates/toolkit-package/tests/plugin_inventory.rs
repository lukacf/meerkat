//! Exercise ordinary package discovery through the existing inspection process.
//! Fixtures are public local data. No package command or network is invoked.
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn inspect(request: &Value) -> (Value, bool) {
    inspect_raw(&serde_json::to_vec(request).unwrap())
}

fn inspect_raw(request: &[u8]) -> (Value, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_toolkit-package-inspect"))
        .arg("agent-plugin-files")
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(request).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    (
        serde_json::from_slice(&output.stdout).unwrap(),
        output.status.success(),
    )
}

fn inventory(root: &Path, directory: &str) -> (Value, bool) {
    inspect(&json!({"root":root,"directory":"./package","inventory":directory}))
}

fn tree() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("package")).unwrap();
    root
}

fn assert_refused(result: (Value, bool)) {
    let (report, success) = result;
    assert!(!success, "{report}");
    assert_eq!(report["status"], "invalid");
    assert_eq!(report["error"]["kind"], "invalid_metadata");
    for field in [
        "inventory",
        "complete",
        "files",
        "source_digest",
        "plugin_source",
    ] {
        assert!(report.get(field).is_none(), "{report}");
    }
}

#[test]
fn inventory_discovers_unknown_skill_and_supporting_file_before_explicit_read() {
    let root = tree();
    let package = root.path().join("package");
    // The consumer below receives only the staged root. It obtains these
    // otherwise unknown names from the ordinary directory inspection response.
    let skill_name = format!("guide-{}", std::process::id());
    let reference_name = format!("station-{}.txt", std::process::id());
    fs::create_dir_all(package.join("skills").join(&skill_name).join("references")).unwrap();
    let skill = format!(
        "---\nname: {skill_name}\ndescription: Read station guidance.\n---\nRead references/{reference_name}.\n"
    );
    let reference = "Station: Pine Observatory.\nReading: 37 units.\n";
    fs::write(
        package.join("plugin.json"),
        serde_json::to_vec(&json!({
            "$schema":toolkit_package::AGENT_PLUGIN_SCHEMA,"name":"station-source",
            "extensions":{"example.invalid":{"opaque":true}}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        package.join("skills").join(&skill_name).join("SKILL.md"),
        &skill,
    )
    .unwrap();
    fs::write(
        package
            .join("skills")
            .join(&skill_name)
            .join("references")
            .join(&reference_name),
        reference,
    )
    .unwrap();

    let (top, success) = inventory(root.path(), "./");
    assert!(success, "{top}");
    assert_eq!(top["complete"], true);
    let skills = top["inventory"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["path"] == "./skills")
        .unwrap();
    assert_eq!(skills["kind"], "directory");
    let (children, success) = inventory(root.path(), skills["path"].as_str().unwrap());
    assert!(success, "{children}");
    let skill_directory = children["inventory"][0]["path"].as_str().unwrap();
    let (skill_files, success) = inventory(root.path(), skill_directory);
    assert!(success, "{skill_files}");
    let rows = skill_files["inventory"].as_array().unwrap();
    let skill_path = rows
        .iter()
        .find(|row| row["path"].as_str().unwrap().ends_with("/SKILL.md"))
        .unwrap()["path"]
        .as_str()
        .unwrap();
    let references = rows.iter().find(|row| row["kind"] == "directory").unwrap();
    let (reference_files, success) = inventory(root.path(), references["path"].as_str().unwrap());
    assert!(success, "{reference_files}");
    let reference_path = reference_files["inventory"][0]["path"].as_str().unwrap();
    let (selected, success) = inspect(&json!({
        "root":root.path(),"directory":"./package",
        "files":["./plugin.json",skill_path,reference_path]
    }));
    assert!(success, "{selected}");
    assert_eq!(selected["files"][1]["text"], skill);
    assert_eq!(selected["files"][2]["text"], reference);
    assert!(selected["source_digest"].is_string());
    for report in [top, children, skill_files, reference_files] {
        assert_eq!(report["status"], "inventoried");
        assert_eq!(report["scope"], "metadata_only");
        assert_eq!(report["format"], "agent-plugin-files");
        for field in ["files", "source_digest", "plugin_source", "text"] {
            assert!(report.get(field).is_none());
        }
    }
}

#[test]
fn inventory_is_shallow_sorted_content_free_and_reports_empty_directories() {
    let root = tree();
    let package = root.path().join("package");
    fs::create_dir(package.join("empty")).unwrap();
    fs::create_dir(package.join("deep")).unwrap();
    fs::write(package.join("deep/invalid\nname"), b"never enumerated").unwrap();
    fs::write(package.join("zeta.bin"), vec![0xff; 1024 * 1024]).unwrap();
    fs::write(
        package.join("caf\u{e9}.txt"),
        b"secret fixture contents never read",
    )
    .unwrap();
    let expected = json!({
        "schema_version":1,"scope":"metadata_only","format":"agent-plugin-files",
        "status":"inventoried","directory":"./","complete":true,
        "inventory":[
            {"path":"./caf\u{e9}.txt","kind":"file","byte_length":34},
            {"path":"./deep","kind":"directory"},
            {"path":"./empty","kind":"directory"},
            {"path":"./zeta.bin","kind":"file","byte_length":1024*1024}
        ]
    });
    let first = inventory(root.path(), "./");
    assert!(first.1, "{}", first.0);
    assert_eq!(first.0, expected);
    assert_eq!(inventory(root.path(), "./"), first);
    let (empty, success) = inventory(root.path(), "./empty");
    assert!(success, "{empty}");
    assert_eq!(empty["inventory"], json!([]));
    assert_eq!(empty["complete"], true);
    assert_refused(inventory(root.path(), "./deep"));
}

#[test]
fn inventory_refuses_ambiguous_requests_and_unsafe_directory_paths() {
    let root = tree();
    for extra in [
        json!({}),
        json!({"inventory":null}),
        json!({"inventory":true}),
        json!({"inventory":"./","files":["./plugin.json"]}),
        json!({"inventory":"./","extra":"ignored?"}),
    ] {
        let mut request = json!({"root":root.path(),"directory":"./package"});
        request
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert_refused(inspect(&request));
    }
    for directory in [
        "",
        ".",
        "/",
        "../",
        "./..",
        "./../outside",
        "./x/../outside",
        "./x//y",
        "./x/./y",
        "./x/",
        "./x\\y",
        "./x:y",
        "./x\ny",
        "./x\u{85}y",
        "./missing",
        "./plugin.json",
    ] {
        assert_refused(inventory(root.path(), directory));
    }
    assert_refused(inventory(root.path(), &format!("./{}", "x".repeat(1023))));
}

#[test]
fn inventory_refuses_duplicate_raw_request_keys_before_projection() {
    let root = tree();
    let root_json = serde_json::to_string(root.path()).unwrap();
    for fields in [
        r#""inventory":"./","inventory":"./missing""#,
        r#""inventory":"./","directory":"./another""#,
        r#""files":["./plugin.json"],"files":["./other"]"#,
    ] {
        let raw = format!(r#"{{"root":{root_json},"directory":"./package",{fields}}}"#);
        assert_refused(inspect_raw(raw.as_bytes()));
    }
}

#[test]
fn inventory_refuses_count_and_total_path_limits_without_partial_results() {
    let root = tree();
    let package = root.path().join("package");
    for number in 0..256 {
        fs::write(package.join(format!("file-{number:03}")), b"").unwrap();
    }
    let (full, success) = inventory(root.path(), "./");
    assert!(success, "{full}");
    assert_eq!(full["inventory"].as_array().unwrap().len(), 256);
    fs::write(package.join("overflow"), b"").unwrap();
    assert_refused(inventory(root.path(), "./"));

    let root = tree();
    let relative = format!("./{}/{}", "a".repeat(150), "b".repeat(150));
    let directory = root.path().join("package").join(&relative);
    fs::create_dir_all(&directory).unwrap();
    for number in 0..132 {
        fs::write(
            directory.join(format!("{number:03}{}", "f".repeat(197))),
            b"",
        )
        .unwrap();
    }
    assert_refused(inventory(root.path(), &relative));
}

#[test]
fn inventory_bounds_depth_and_refuses_invalid_names_without_echoing_them() {
    let root = tree();
    let relative = format!(
        "./{}",
        std::iter::repeat_n("level", 15)
            .collect::<Vec<_>>()
            .join("/")
    );
    let directory = root.path().join("package").join(&relative);
    fs::create_dir_all(directory.join("child")).unwrap();
    let (observed, success) = inventory(root.path(), &relative);
    assert!(success, "{observed}");
    assert_eq!(
        observed["inventory"][0]["path"],
        format!("{relative}/child")
    );
    assert_refused(inventory(root.path(), &format!("{relative}/child")));

    for name in [
        "unsafe\nname",
        "unsafe\u{85}name",
        "unsafe:name",
        "unsafe\\name",
    ] {
        let root = tree();
        fs::write(root.path().join("package").join(name), b"").unwrap();
        let result = inventory(root.path(), "./");
        assert!(!result.0.to_string().contains(name));
        assert_refused(result);
    }
}

#[cfg(unix)]
#[test]
fn inventory_classifies_links_and_special_files_without_following_them() {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    let root = tree();
    let outside = tempfile::tempdir().unwrap();
    let package = root.path().join("package");
    fs::create_dir(package.join("skills")).unwrap();
    fs::write(package.join("skills/SKILL.md"), b"guide").unwrap();
    fs::write(
        outside.path().join("private.txt"),
        b"outside must never be read",
    )
    .unwrap();
    symlink(outside.path(), package.join("outside")).unwrap();
    symlink(
        outside.path().join("private.txt"),
        package.join("outside-file"),
    )
    .unwrap();
    symlink("skills", package.join("inside")).unwrap();
    symlink("missing", package.join("broken")).unwrap();
    symlink("loop", package.join("loop")).unwrap();
    let _socket = UnixListener::bind(package.join("socket")).unwrap();
    let output = Command::new("/usr/bin/mkfifo")
        .arg(package.join("pipe"))
        .output()
        .unwrap();
    assert!(output.status.success());
    let (observed, success) = inventory(root.path(), "./");
    assert!(success, "{observed}");
    for row in observed["inventory"].as_array().unwrap() {
        let name = row["path"].as_str().unwrap();
        assert_eq!(
            row["kind"],
            match name {
                "./skills" => "directory",
                "./pipe" | "./socket" => "special",
                _ => "symlink",
            }
        );
        assert!(row.get("byte_length").is_none());
    }
    let (inside, success) = inventory(root.path(), "./inside");
    assert!(success, "{inside}");
    assert_eq!(inside["inventory"][0]["path"], "./inside/SKILL.md");
    for directory in [
        "./outside",
        "./outside-file",
        "./broken",
        "./loop",
        "./pipe",
        "./socket",
    ] {
        assert_refused(inventory(root.path(), directory));
    }
    fs::rename(&package, root.path().join("saved")).unwrap();
    symlink(outside.path(), &package).unwrap();
    assert_refused(inventory(root.path(), "./"));
}

// macOS rejects this fixture at creation; Linux exercises the reader's refusal.
#[cfg(target_os = "linux")]
#[test]
fn inventory_refuses_non_utf8_names_without_lossy_projection() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let root = tree();
    fs::write(
        root.path()
            .join("package")
            .join(OsString::from_vec(vec![0xff])),
        b"",
    )
    .unwrap();
    assert_refused(inventory(root.path(), "./"));
}
