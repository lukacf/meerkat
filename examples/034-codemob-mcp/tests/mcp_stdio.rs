#[test]
fn real_binary_stdio_contract() {
    let output = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/mcp_stdio.py"))
        .arg(env!("CARGO_BIN_EXE_codemob-mcp"))
        .output()
        .expect("python3 is required for the bounded MCP subprocess harness");
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
