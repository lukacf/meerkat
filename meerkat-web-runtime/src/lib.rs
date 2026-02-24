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

#[derive(Debug, Deserialize)]
struct WebDefinition {
    id: String,
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
    let definition: WebDefinition = serde_json::from_slice(
        files
            .get("definition.json")
            .ok_or_else(|| "definition.json is missing".to_string())?,
    )
    .map_err(|err| format!("invalid definition.json: {err}"))?;

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
        definition.id,
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
        if !kind.is_file() {
            continue;
        }
        let normalized = normalize_for_archive(path.to_string_lossy().as_ref())?;
        let mut contents = Vec::new();
        entry
            .read_to_end(&mut contents)
            .map_err(|err| format!("failed reading archive file '{normalized}': {err}"))?;
        files.insert(normalized, contents);
    }
    Ok(files)
}

fn normalize_for_archive(path: &str) -> Result<String, String> {
    let replaced = path.replace('\\', "/");
    if replaced.starts_with('/') || looks_like_windows_absolute(&replaced) {
        return Err("archive contains absolute path entry".to_string());
    }
    let mut parts = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err("archive contains parent directory traversal entry".to_string());
        }
        parts.push(segment);
    }
    if parts.is_empty() {
        return Err("archive contains empty path entry".to_string());
    }
    Ok(parts.join("/"))
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
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
                let append_result =
                    builder.append_data(&mut header, path.as_str(), content.as_slice());
                assert!(
                    append_result.is_ok(),
                    "append data failed: {append_result:?}"
                );
            }
            let finish_result = builder.finish();
            assert!(
                finish_result.is_ok(),
                "finish tar failed: {finish_result:?}"
            );
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let write_result = std::io::Write::write_all(&mut encoder, &tar_bytes);
        assert!(write_result.is_ok(), "write gz failed: {write_result:?}");
        match encoder.finish() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("finish gz failed: {err}"),
        }
    }

    fn create_single_entry_archive(path: &str) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            builder.mode(tar::HeaderMode::Deterministic);
            let mut header = tar::Header::new_gnu();
            header.set_size(1);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_uid(0);
            header.set_gid(0);
            header.set_cksum();
            let append_result = builder.append_data(&mut header, path, b"x".as_slice());
            assert!(
                append_result.is_ok(),
                "append data failed: {append_result:?}"
            );
            let finish_result = builder.finish();
            assert!(
                finish_result.is_ok(),
                "finish tar failed: {finish_result:?}"
            );
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let write_result = std::io::Write::write_all(&mut encoder, &tar_bytes);
        assert!(write_result.is_ok(), "write gz failed: {write_result:?}");
        match encoder.finish() {
            Ok(bytes) => bytes,
            Err(err) => unreachable!("finish gz failed: {err}"),
        }
    }

    #[test]
    fn bootstrap_accepts_browser_safe_pack() {
        let archive = fixture_archive(false);
        let out = match bootstrap_impl(&archive, "hello") {
            Ok(value) => value,
            Err(err) => unreachable!("bootstrap should pass: {err}"),
        };
        assert!(out.contains("bootstrapped:web-mob"));
    }

    #[test]
    fn bootstrap_rejects_forbidden_capability() {
        let archive = fixture_archive(true);
        let err = match bootstrap_impl(&archive, "hello") {
            Ok(value) => unreachable!("should fail, got: {value}"),
            Err(err) => err,
        };
        assert!(err.contains("forbidden capability 'shell'"));
    }

    #[test]
    fn extract_rejects_windows_absolute_paths() {
        let archive = create_single_entry_archive("C:/temp/evil.txt");
        let err = match extract_targz_safe(&archive) {
            Ok(_) => unreachable!("windows absolute should fail"),
            Err(err) => err,
        };
        assert!(err.contains("absolute path"));
    }
}
