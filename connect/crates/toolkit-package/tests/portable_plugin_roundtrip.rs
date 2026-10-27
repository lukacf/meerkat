#![cfg(unix)]

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use toolkit_package::{
    AGENT_PLUGIN_MCP_SCHEMA, AGENT_PLUGIN_SCHEMA, ImportedMcpServers, ImportedPluginManifest,
    PluginRoot, PortableExportCheckState, PortablePluginExport,
};

// Authored local fixture, with no subprocess, network or credential access.
// The server reads data supplied after the script was authored, so neither an
// exporter nor the generic client can satisfy the call from a canned answer.
const SERVER: &str = r##"#!/bin/sh
while IFS= read -r request; do
    case "$request" in
        *'"method":"initialize"'*)
            printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"portable-roundtrip","version":"1"}}}'
            ;;
        *'"method":"notifications/initialized"'*) ;;
        *'"method":"tools/list"'*)
            printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"read_value","description":"Read the local fixture value","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]}}'
            ;;
        *'"method":"tools/call"'*)
            IFS= read -r reading < "$READING_FILE" || exit 2
            printf '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"%s"}]}}\n' "$reading"
            ;;
        *) exit 3 ;;
    esac
done
"##;

/// A deliberately independent portable consumer: reads only plugin.json and
/// mcp.json, ignores every extension namespace, and launches the declared stdio
/// token with a client-owned environment. It does not call Toolkit import,
/// placeholder, selection, installation or dispatch APIs.
struct GenericStdioClient {
    child: Child,
    stdin: Option<ChildStdin>,
    replies: Receiver<Value>,
    reader: Option<JoinHandle<()>>,
}

impl GenericStdioClient {
    fn connect(root: &Path, data: &Path) -> Self {
        let root = fs::canonicalize(root).unwrap();
        let manifest: Value =
            serde_json::from_slice(&fs::read(root.join("plugin.json")).unwrap()).unwrap();
        assert_eq!(
            manifest["$schema"],
            "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json"
        );
        let config: Value =
            serde_json::from_slice(&fs::read(root.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(
            config["$schema"],
            "https://agent-plugins.org/schemas/1.0.0/mcp.schema.json"
        );
        let server = &config["mcpServers"]["local"];
        assert_eq!(server["type"], "stdio");
        let command = server["command"].as_str().unwrap();
        let command = fs::canonicalize(root.join(command.strip_prefix("./").unwrap())).unwrap();
        assert!(command.starts_with(&root));
        fs::create_dir(data).unwrap();
        let data = fs::canonicalize(data).unwrap();
        let mut process = Command::new(command);
        process
            .current_dir(&root)
            .env_clear()
            .env("PLUGIN_ROOT", &root)
            .env("PLUGIN_DATA", &data)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (name, value) in server["env"].as_object().unwrap() {
            assert!(!matches!(name.as_str(), "PLUGIN_ROOT" | "PLUGIN_DATA"));
            // This fixture uses one literal prefix. No shell or ambient
            // environment expansion occurs in this independent client.
            let value = value.as_str().unwrap();
            let value = match value.strip_prefix("${PLUGIN_ROOT}/") {
                Some(relative) => root.join(relative).to_str().unwrap().to_owned(),
                None => value.to_owned(),
            };
            process.env(name, value);
        }
        let mut child = process
            .spawn()
            .expect("generic client must launch exported executable");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, replies) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Ok(value) = serde_json::from_str(&line) else {
                    break;
                };
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            replies,
            reader: Some(reader),
        }
    }

    fn send(&mut self, request: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        serde_json::to_writer(&mut *stdin, &request).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, request: Value) -> Value {
        let id = request["id"].clone();
        self.send(request);
        let reply = self.replies.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(reply["jsonrpc"], "2.0");
        assert_eq!(reply["id"], id);
        assert!(reply.get("error").is_none());
        reply["result"].clone()
    }

    fn close(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "fixture must exit on EOF");
            thread::sleep(Duration::from_millis(5));
        }
        self.reader.take().unwrap().join().unwrap();
    }
}

impl Drop for GenericStdioClient {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[test]
fn exported_plugin_runs_in_a_generic_client_after_source_removal() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    for directory in ["bin", "assets", "skills/read-value/references"] {
        fs::create_dir_all(source.join(directory)).unwrap();
    }
    fs::write(source.join("bin/server"), SERVER).unwrap();
    fs::set_permissions(source.join("bin/server"), fs::Permissions::from_mode(0o755)).unwrap();
    // Choose fresh fixture data that is not embedded in the server program.
    let reading = format!(
        "sample-{}",
        temp.path().file_name().unwrap().to_str().unwrap()
    );
    fs::write(source.join("assets/reading.txt"), format!("{reading}\n")).unwrap();
    let skill = b"---\nname: read-value\ndescription: Read a local sample using the portable fixture server.\n---\nUse read_value and interpret it with [the format](references/format.md).\n";
    let reference = b"Return the sample unchanged. Do not guess a missing value.\n";
    fs::write(source.join("skills/read-value/SKILL.md"), skill).unwrap();
    fs::write(
        source.join("skills/read-value/references/format.md"),
        reference,
    )
    .unwrap();
    let plugin = json!({"$schema": AGENT_PLUGIN_SCHEMA, "name":"portable.roundtrip",
        "extensions":{"org.example.other-client":{"display":"ignored by generic client"}}});
    let mcp = json!({"$schema": AGENT_PLUGIN_MCP_SCHEMA,"mcpServers":{"local":{
        "type":"stdio","command":"./bin/server", "env":{"READING_FILE":"${PLUGIN_ROOT}/assets/reading.txt"}
    }}});
    fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&plugin).unwrap(),
    )
    .unwrap();
    fs::write(source.join("mcp.json"), serde_json::to_vec(&mcp).unwrap()).unwrap();
    let root = PluginRoot::open(&source).unwrap();
    let manifest = root.read_manifest().unwrap();
    let servers = ImportedMcpServers::parse(&root.read("./mcp.json", 4096).unwrap()).unwrap();
    let mut export = PortablePluginExport::new(&manifest.manifest, &servers.servers, None).unwrap();
    for file in [
        "./bin/server",
        "./assets/reading.txt",
        "./skills/read-value/SKILL.md",
        "./skills/read-value/references/format.md",
    ] {
        export = export.with_file(&root, file).unwrap();
    }
    drop(root);
    fs::remove_dir_all(&source).unwrap();
    let destination = temp.path().join("exported");
    export.write_new(&destination).unwrap();

    // A generic consumer discovers skills at the standard location and follows
    // a normal relative reference, without any Toolkit side document.
    let skills = fs::read_dir(destination.join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(skills.len(), 1);
    assert_eq!(fs::read(skills[0].join("SKILL.md")).unwrap(), skill);
    assert_eq!(
        fs::read(skills[0].join("references/format.md")).unwrap(),
        reference
    );
    let mut client = GenericStdioClient::connect(&destination, &temp.path().join("client-data"));
    let initialized = client.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize",
        "params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"independent-generic-client","version":"1"}}}));
    assert_eq!(initialized["protocolVersion"], "2024-11-05");
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let listed = client.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}));
    assert_eq!(listed["tools"][0]["name"], "read_value");
    let answer = client.request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
        "params":{"name":"read_value","arguments":{}}}));
    assert_eq!(answer["content"][0]["text"], reading);
    client.close();
    assert_eq!(
        export.inspect_existing(&destination).state,
        PortableExportCheckState::Matches
    );
}

#[test]
fn included_files_preserve_executable_intent_without_special_or_writable_modes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let manifest = ImportedPluginManifest::parse(
        &serde_json::to_vec(&json!({
            "$schema": AGENT_PLUGIN_SCHEMA, "name":"file-modes"
        }))
        .unwrap(),
    )
    .unwrap();
    let root = PluginRoot::open(&source).unwrap();
    for (index, mode) in [0o644, 0o600, 0o700, 0o4755, 0o2777, 0o1777]
        .into_iter()
        .enumerate()
    {
        fs::write(source.join("file"), b"inert fixture\n").unwrap();
        fs::set_permissions(source.join("file"), fs::Permissions::from_mode(mode)).unwrap();
        let export = PortablePluginExport::new(&manifest.manifest, &BTreeMap::new(), None)
            .unwrap()
            .with_file(&root, "./file")
            .unwrap();
        // Changes after inclusion must not replace the selected mode or bytes.
        fs::set_permissions(source.join("file"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(source.join("file"), b"later bytes").unwrap();
        let destination = temp.path().join(format!("export-{index}"));
        export.write_new(&destination).unwrap();
        let observed = fs::metadata(destination.join("file"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(observed & 0o111 != 0, mode & 0o111 != 0);
        assert_eq!(observed & 0o7000, 0);
        assert_eq!(observed & 0o022, 0);
        assert_eq!(
            fs::read(destination.join("file")).unwrap(),
            b"inert fixture\n"
        );
    }
}

#[test]
fn export_creation_respects_private_umask() {
    const CHILD: &str = "TOOLKIT_PORTABLE_UMASK_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = Command::new("/bin/sh")
            .args([
                "-c",
                "umask 077; exec \"$1\" --exact export_creation_respects_private_umask --nocapture",
                "portable-umask-test",
            ])
            .arg(std::env::current_exe().unwrap())
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "isolated umask control failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    // The controlled shell set umask before exec of this separate test process
    // with one selected method. It cannot race another test in the parent.
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("server"), b"inert executable fixture\n").unwrap();
    fs::set_permissions(source.join("server"), fs::Permissions::from_mode(0o777)).unwrap();
    let manifest = ImportedPluginManifest::parse(
        &serde_json::to_vec(&json!({
            "$schema": AGENT_PLUGIN_SCHEMA, "name":"private-export"
        }))
        .unwrap(),
    )
    .unwrap();
    let export = PortablePluginExport::new(&manifest.manifest, &BTreeMap::new(), None)
        .unwrap()
        .with_file(&PluginRoot::open(&source).unwrap(), "./server")
        .unwrap();
    let destination = temp.path().join("exported");
    export.write_new(&destination).unwrap();
    assert_eq!(
        fs::metadata(destination.join("server"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(destination.join("plugin.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        export.inspect_existing(&destination).state,
        PortableExportCheckState::Matches
    );
}
