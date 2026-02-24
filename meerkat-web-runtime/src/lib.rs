use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;
use wasm_bindgen::prelude::*;

const FORBIDDEN_CAPABILITIES: &[&str] = &["shell", "mcp_stdio", "process_spawn"];

#[derive(Debug, Deserialize)]
struct WebManifest {
    mobpack: WebMobpackSection,
    #[serde(default)]
    requires: Option<WebRequiresSection>,
}

#[derive(Debug, Deserialize)]
struct WebMobpackSection {
    name: String,
    version: String,
}

#[derive(Debug, Deserialize, Default)]
struct WebRequiresSection {
    #[serde(default)]
    capabilities: Vec<String>,
}

fn bootstrap_impl(mobpack_bytes: &[u8], prompt: &str) -> Result<String, String> {
    if prompt.trim().is_empty() {
        return Err("prompt must not be empty".to_string());
    }
    let files = extract_targz_safe(mobpack_bytes)
        .map_err(|err| format!("failed to parse mobpack archive: {err}"))?;
    let manifest_text = std::str::from_utf8(
        files
            .get("manifest.toml")
            .ok_or_else(|| "manifest.toml is missing".to_string())?,
    )
    .map_err(|err| format!("manifest.toml is not valid UTF-8: {err}"))?;
    let manifest: WebManifest =
        toml::from_str(manifest_text).map_err(|err| format!("invalid manifest.toml: {err}"))?;
    let definition: serde_json::Value = serde_json::from_slice(
        files
            .get("definition.json")
            .ok_or_else(|| "definition.json is missing".to_string())?,
    )
    .map_err(|err| format!("invalid definition.json: {err}"))?;
    let definition_id = definition
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "definition.json missing string field 'id'".to_string())?;

    if let Some(requires) = &manifest.requires {
        for capability in &requires.capabilities {
            if FORBIDDEN_CAPABILITIES.contains(&capability.as_str()) {
                return Err(format!(
                    "forbidden capability '{}' is not allowed in browser-safe mode",
                    capability
                ));
            }
        }
    }

    Ok(format!(
        "bootstrapped:{}:{}@{}:prompt_bytes={}",
        definition_id,
        manifest.mobpack.name,
        manifest.mobpack.version,
        prompt.len()
    ))
}

fn extract_targz_safe(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let decoder = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();
    let entries = archive
        .entries()
        .map_err(|err| format!("failed to read archive entries: {err}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|err| format!("failed reading archive entry: {err}"))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err("archive contains unsupported entry type".to_string());
        }
        let path = entry
            .path()
            .map_err(|err| format!("invalid archive path: {err}"))?;
        if path.is_absolute() {
            return Err("archive contains absolute path entry".to_string());
        }
        if path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("archive contains parent directory traversal entry".to_string());
        }
        if !kind.is_file() {
            continue;
        }
        let normalized = path.to_string_lossy().replace('\\', "/");
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|err| format!("failed reading archive file '{normalized}': {err}"))?;
        files.insert(normalized, contents);
    }
    Ok(files)
}

#[wasm_bindgen]
pub fn bootstrap_mobpack(mobpack_bytes: &[u8], prompt: &str) -> Result<String, JsValue> {
    bootstrap_impl(mobpack_bytes, prompt).map_err(|err| JsValue::from_str(&err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture_archive(with_forbidden_capability: bool) -> Vec<u8> {
        let manifest = if with_forbidden_capability {
            "[mobpack]\nname = \"web\"\nversion = \"1.0.0\"\n\n[requires]\ncapabilities = [\"shell\"]\n"
        } else {
            "[mobpack]\nname = \"web\"\nversion = \"1.0.0\"\n"
        };
        let definition = "{\"id\":\"web-mob\",\"skills\":{}}";
        let files = BTreeMap::from([
            ("manifest.toml".to_string(), manifest.as_bytes().to_vec()),
            (
                "definition.json".to_string(),
                definition.as_bytes().to_vec(),
            ),
        ]);
        create_targz(&files)
    }

    fn create_targz(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            for (path, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_uid(0);
                header.set_gid(0);
                header.set_cksum();
                builder
                    .append_data(&mut header, path.as_str(), content.as_slice())
                    .expect("append data");
            }
            builder.finish().expect("finish tar");
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).expect("write gz");
        encoder.finish().expect("finish gz")
    }

    #[test]
    fn bootstrap_accepts_browser_safe_pack() {
        let archive = fixture_archive(false);
        let out = bootstrap_impl(&archive, "hello").expect("bootstrap should pass");
        assert!(out.contains("bootstrapped:web-mob"));
    }

    #[test]
    fn bootstrap_rejects_forbidden_capability() {
        let archive = fixture_archive(true);
        let err = bootstrap_impl(&archive, "hello").expect_err("should fail");
        assert!(err.contains("forbidden capability 'shell'"));
    }
}
