//! The MCP Apps sandbox proxy, embedded so a released gateway can write it.
//!
//! The stock Console renders MCP Apps inside an isolated sandbox page served
//! from an origin of its own. That page comes from a small Node proxy:
//! `console/mcp-apps-sandbox.cjs` and the page script it inlines,
//! `console/src/mcp-apps/sandbox.js`. The proxy and the Console share an
//! internal sandbox protocol, so they must come from the same build. The
//! console build copies both files into this crate's `console-dist/`
//! (`npm --prefix console run build`, checked by `embedded:freshness`), and
//! the gateway binaries write them back out with
//! `--write-mcp-apps-sandbox <dir>`.

use std::path::{Path, PathBuf};

/// The proxy's files: where each goes under the target directory, and its
/// bytes. The proxy loads its page script from `src/mcp-apps/sandbox.js`
/// next to itself.
const MCP_APPS_SANDBOX_FILES: [(&str, &str); 2] = [
    (
        "mcp-apps-sandbox.cjs",
        include_str!("../console-dist/mcp-apps-sandbox.cjs"),
    ),
    (
        "src/mcp-apps/sandbox.js",
        include_str!("../console-dist/mcp-apps-sandbox.js"),
    ),
];

/// Write the MCP Apps sandbox proxy into `dir`: `mcp-apps-sandbox.cjs` and
/// `src/mcp-apps/sandbox.js`, creating the directories and replacing earlier
/// copies. Start it with `node <dir>/mcp-apps-sandbox.cjs`. Returns the
/// written paths in that order.
pub fn write_mcp_apps_sandbox(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    MCP_APPS_SANDBOX_FILES
        .iter()
        .map(|(relative_path, contents)| {
            let path = dir.join(relative_path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, contents)?;
            Ok(path)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    /// The written proxy is the console source byte for byte, and a second
    /// write replaces stale copies (an upgrade writes over the old proxy).
    #[test]
    fn writes_the_console_sandbox_sources_over_stale_copies() {
        let console = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../console");
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("sandbox");
        std::fs::create_dir_all(target.join("src/mcp-apps")).expect("stale layout");
        std::fs::write(target.join("mcp-apps-sandbox.cjs"), "stale proxy").expect("stale");
        std::fs::write(target.join("src/mcp-apps/sandbox.js"), "stale page").expect("stale");

        let written = super::write_mcp_apps_sandbox(&target).expect("write the sandbox");

        let read = |path: &std::path::Path| std::fs::read(path).expect("read file");
        assert_eq!(
            written.iter().map(|path| read(path)).collect::<Vec<_>>(),
            vec![
                read(&console.join("mcp-apps-sandbox.cjs")),
                read(&console.join("src/mcp-apps/sandbox.js")),
            ]
        );
        assert_eq!(
            written,
            vec![
                target.join("mcp-apps-sandbox.cjs"),
                target.join("src/mcp-apps/sandbox.js"),
            ]
        );
    }
}
