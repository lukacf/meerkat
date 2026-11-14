use crate::digest::{MobpackDigest, canonical_digest_from_map};
use crate::manifest::MobpackManifest;
use crate::signing::{PackSignature, sign_digest};
use crate::targz::{create_targz, extract_targz_safe, normalize_for_archive};
use crate::validate::PackValidationError;
use crate::vocabulary::{
    ArchiveSection, Ed25519PublicKeyHex, Ed25519SignatureHex, Rfc3339Timestamp, SignerId,
};
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use meerkat_contracts::capability::{
    CapabilityId, MobpackCapabilityId, known_mobpack_capability_tokens,
    mobpack_capability_known_to_host,
};
use meerkat_mob::adaptive::AdaptivePolicy;
use meerkat_mob::{MobDefinition, definition::SkillSource};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Canonical archive path of the bundled LayerDecision schema artifact.
const LAYER_DECISION_SCHEMA_PATH: &str = "adaptive/layer-decision.schema.json";

/// Canonical archive path of the adaptive policy document.
const ADAPTIVE_POLICIES_PATH: &str = "adaptive/policies.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackResult {
    pub archive_bytes: Vec<u8>,
    pub digest: MobpackDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectResult {
    pub name: String,
    pub version: String,
    pub file_count: usize,
    pub files: Vec<String>,
    pub digest: MobpackDigest,
}

/// Inputs required to sign a mobpack.
///
/// `signer_id` is the semantic identity recorded in `signature.toml` and looked
/// up against the trust store. `key_path` is key material used to authenticate
/// that identity; it is deliberately separate because the filesystem layout of
/// the key file is not a source of identity.
#[derive(Debug, Clone, Copy)]
pub struct SigningRequest<'a> {
    pub signer_id: &'a str,
    pub key_path: &'a Path,
}

#[derive(Debug)]
pub(crate) struct ValidatedPackFiles {
    pub manifest: MobpackManifest,
    pub definition: MobDefinition,
}

pub fn pack_directory(
    directory: &Path,
    signing: Option<SigningRequest<'_>>,
) -> Result<PackResult, PackValidationError> {
    pack_directory_with_excludes(directory, signing, &[])
}

pub fn pack_directory_with_excludes(
    directory: &Path,
    signing: Option<SigningRequest<'_>>,
    excluded_paths: &[&Path],
) -> Result<PackResult, PackValidationError> {
    let mut files = scan_directory(directory)?;
    exclude_paths_from_pack(directory, excluded_paths, &mut files)?;
    if let Some(request) = signing {
        exclude_paths_from_pack(directory, &[request.key_path], &mut files)?;
    }
    prepare_adaptive_build_outputs(&mut files)?;
    validate_extracted_pack_files(&files)?;

    if let Some(request) = signing {
        let unsigned_archive = create_targz(&files)?;
        let digest = compute_archive_digest(&unsigned_archive)?;
        let signing_key = load_signing_key(request.key_path)?;
        let signature = sign_digest(&signing_key, digest);
        let signer_id = SignerId::from_str(request.signer_id).map_err(|err| {
            PackValidationError::InvalidSignature(format!("invalid signer id: {err}"))
        })?;
        let timestamp =
            Rfc3339Timestamp::from_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true))
                .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
        let signature_toml = toml::to_string(&PackSignature {
            signer_id,
            public_key: Ed25519PublicKeyHex::from_verifying_key(signing_key.verifying_key()),
            digest,
            signature: Ed25519SignatureHex::from_signature(signature),
            timestamp,
        })
        .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        files.insert("signature.toml".to_string(), signature_toml.into_bytes());
    }

    let archive_bytes = create_targz(&files)?;
    let digest = compute_archive_digest(&archive_bytes)?;
    Ok(PackResult {
        archive_bytes,
        digest,
    })
}

pub fn inspect_archive_bytes(bytes: &[u8]) -> Result<InspectResult, PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    let ValidatedPackFiles { manifest, .. } = validate_extracted_pack_files(&files)?;
    let mut file_list: Vec<String> = files.keys().cloned().collect();
    file_list.sort();
    Ok(InspectResult {
        name: manifest.mobpack.name,
        version: manifest.mobpack.version,
        file_count: files.len(),
        files: file_list,
        digest: canonical_digest_from_map(&files),
    })
}

pub fn compute_archive_digest(bytes: &[u8]) -> Result<MobpackDigest, PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    Ok(canonical_digest_from_map(&files))
}

pub(crate) fn validate_extracted_pack_files(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<ValidatedPackFiles, PackValidationError> {
    validate_required_files(files)?;
    ensure_no_trust_section(files)?;
    let manifest = parse_manifest(files)?;
    validate_manifest_identity(&manifest)?;
    validate_required_capabilities_known(&manifest)?;
    validate_adaptive_files(&manifest, files)?;
    let definition = parse_definition(files)?;
    reject_realm_refs(&definition)?;
    reject_profile_auth_bindings(&definition)?;
    validate_skill_paths(&definition, files)?;
    Ok(ValidatedPackFiles {
        manifest,
        definition,
    })
}

/// Fail-closed capability knowledge gate, run at every pack load.
///
/// Every `[requires]` capability must classify into the typed vocabulary this
/// host build knows ([`mobpack_capability_known_to_host`]); a token from the
/// future (or a vendor token) is rejected with the host's known set instead
/// of being silently ignored and the pack downgraded. Additionally, a
/// manifest that declares `[adaptive]` must carry the stamped
/// `adaptive_flow` capability so pre-stamp packs cannot smuggle adaptive
/// payloads past hosts that gate on `[requires]`.
fn validate_required_capabilities_known(
    manifest: &MobpackManifest,
) -> Result<(), PackValidationError> {
    if let Some(requires) = &manifest.requires {
        for capability in &requires.capabilities {
            if !mobpack_capability_known_to_host(capability.id()) {
                return Err(PackValidationError::UnknownRequiredCapability {
                    capability: capability.token().to_string(),
                    supported: known_mobpack_capability_tokens(),
                });
            }
        }
    }
    if manifest.adaptive.is_some() && !manifest_declares_adaptive_capability(manifest) {
        return Err(PackValidationError::MissingAdaptiveCapability {
            capability: CapabilityId::AdaptiveFlow.to_string(),
        });
    }
    Ok(())
}

fn manifest_declares_adaptive_capability(manifest: &MobpackManifest) -> bool {
    manifest.requires.as_ref().is_some_and(|requires| {
        requires
            .capability_ids()
            .any(|id| id == MobpackCapabilityId::Known(CapabilityId::AdaptiveFlow))
    })
}

/// Build-side preparation for adaptive packs, run before validation when
/// packing a directory.
///
/// For a manifest that declares `[adaptive]` this
/// 1. stamps the `adaptive_flow` capability into `[requires]` (idempotent —
///    an already-stamped manifest is left byte-identical), and
/// 2. emits the host's canonical LayerDecision schema as
///    `adaptive/layer-decision.schema.json`, overwriting any hand-supplied
///    file (authors no longer maintain this artifact; the archive digest and
///    signature cover the emitted bytes).
///
/// Non-adaptive packs and packs whose manifest does not parse are left
/// untouched; validation reports those errors with full context.
fn prepare_adaptive_build_outputs(
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    let Some(manifest_bytes) = files.get("manifest.toml") else {
        return Ok(());
    };
    let Ok(manifest_text) = std::str::from_utf8(manifest_bytes) else {
        return Ok(());
    };
    let Ok(manifest) = toml::from_str::<MobpackManifest>(manifest_text) else {
        return Ok(());
    };
    if manifest.adaptive.is_none() {
        return Ok(());
    }

    if !manifest_declares_adaptive_capability(&manifest) {
        let stamped = stamp_adaptive_capability(manifest_text)?;
        files.insert("manifest.toml".to_string(), stamped.into_bytes());
    }
    files.insert(
        LAYER_DECISION_SCHEMA_PATH.to_string(),
        canonical_layer_decision_schema_bytes()?,
    );
    Ok(())
}

/// Append `adaptive_flow` to `[requires].capabilities`, preserving the rest
/// of the manifest document (comments, ordering, unknown entries) via
/// `toml_edit`.
fn stamp_adaptive_capability(manifest_text: &str) -> Result<String, PackValidationError> {
    let mut doc: toml_edit::DocumentMut = manifest_text
        .parse()
        .map_err(|err| PackValidationError::InvalidManifest(format!("{err}")))?;
    let requires = doc
        .entry("requires")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    // `as_table_like_mut` accepts both `[requires]` tables and
    // `requires = { ... }` inline tables — the typed manifest parser does.
    let requires_table = requires.as_table_like_mut().ok_or_else(|| {
        PackValidationError::InvalidManifest("`requires` must be a table".to_string())
    })?;
    if requires_table.get("capabilities").is_none() {
        requires_table.insert(
            "capabilities",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    let array = requires_table
        .get_mut("capabilities")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| {
            PackValidationError::InvalidManifest(
                "`requires.capabilities` must be an array".to_string(),
            )
        })?;
    array.push(CapabilityId::AdaptiveFlow.to_string());
    Ok(doc.to_string())
}

/// The canonical `adaptive/layer-decision.schema.json` bytes for this host
/// version, rendered deterministically from
/// [`meerkat_mob::adaptive::layer_decision_schema`].
fn canonical_layer_decision_schema_bytes() -> Result<Vec<u8>, PackValidationError> {
    let schema = canonical_layer_decision_schema()?;
    let mut bytes = serde_json::to_vec_pretty(&schema).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("failed to render canonical schema: {err}"),
        }
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn canonical_layer_decision_schema() -> Result<serde_json::Value, PackValidationError> {
    meerkat_mob::adaptive::layer_decision_schema().map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("failed to derive canonical schema: {err}"),
        }
    })
}

fn validate_adaptive_files(
    manifest: &MobpackManifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    let Some(adaptive) = &manifest.adaptive else {
        return Ok(());
    };
    let policy_bytes = require_adaptive_file(files, ADAPTIVE_POLICIES_PATH)?;
    let observed_policy_digest = format!("sha256:{}", hex::encode(Sha256::digest(policy_bytes)));
    if adaptive.policy_digest != observed_policy_digest {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: format!(
                "policy_digest mismatch: manifest has `{}`, observed `{observed_policy_digest}`",
                adaptive.policy_digest
            ),
        });
    }
    validate_adaptive_policy_document(policy_bytes)?;
    require_adaptive_file(files, "adaptive/flowmaster.prompt.md")?;
    let schema_bytes = require_adaptive_file(files, LAYER_DECISION_SCHEMA_PATH)?;
    validate_layer_decision_schema(schema_bytes)?;
    let registry_bytes = require_adaptive_file(files, "schemas/registry.json")?;
    let registry: serde_json::Value = serde_json::from_slice(registry_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: err.to_string(),
        }
    })?;
    let Some(registry_obj) = registry.as_object() else {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: "must be a JSON object mapping schema names to bundled files".to_string(),
        });
    };
    for schema_name in &adaptive.required_schemas {
        let Some(schema_file) = registry_obj
            .get(schema_name)
            .and_then(serde_json::Value::as_str)
        else {
            return Err(PackValidationError::InvalidAdaptiveFile {
                path: "schemas/registry.json".to_string(),
                reason: format!("required schema `{schema_name}` is not registered"),
            });
        };
        validate_relative_adaptive_schema_path(schema_file)?;
        require_adaptive_file(files, schema_file)?;
    }
    Ok(())
}

/// Parse and structurally validate the pack's adaptive policy document at
/// pack-validation time, not first at runtime: the TOML must parse into the
/// typed [`AdaptivePolicy`] and its limit record must be complete (every
/// limit non-zero, via `AdaptiveLimitRecord::validate_complete`).
fn validate_adaptive_policy_document(policy_bytes: &[u8]) -> Result<(), PackValidationError> {
    let policy_text = std::str::from_utf8(policy_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: format!("not valid UTF-8: {err}"),
        }
    })?;
    let policy: AdaptivePolicy =
        toml::from_str(policy_text).map_err(|err| PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: err.to_string(),
        })?;
    policy.limits.validate_complete("pack").map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: err.to_string(),
        }
    })?;
    Ok(())
}

/// Validate the bundled LayerDecision schema artifact against this host's
/// canonical schema.
///
/// Match rule: the bundled bytes must parse as a JSON object and, once the
/// documentation keywords are removed from both sides (see
/// [`strip_schema_documentation`]), be structurally equal to
/// [`meerkat_mob::adaptive::layer_decision_schema`] (`serde_json::Value`
/// equality, i.e. key order and formatting do not matter). The builder emits
/// exactly this artifact, so a structural divergence means the pack was built
/// by older or hand-rolled tooling and fails closed with a regenerate hint. A
/// reworded doc comment changes only `description`/`title` text, so packs
/// built with an earlier or later wording still match.
fn validate_layer_decision_schema(schema_bytes: &[u8]) -> Result<(), PackValidationError> {
    let mut bundled: serde_json::Value = serde_json::from_slice(schema_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("not valid JSON: {err}"),
        }
    })?;
    if !bundled.is_object() {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: "must be a JSON object schema".to_string(),
        });
    }
    let mut canonical = canonical_layer_decision_schema()?;
    strip_schema_documentation(&mut bundled);
    strip_schema_documentation(&mut canonical);
    if bundled != canonical {
        return Err(PackValidationError::StaleAdaptiveSchema {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
        });
    }
    Ok(())
}

/// JSON Schema annotation keywords that carry documentation only. schemars
/// fills them from Rust doc comments; they constrain no instance.
const SCHEMA_DOCUMENTATION_KEYWORDS: [&str; 2] = ["title", "description"];

/// Keywords whose value is one subschema (`items` may also be an array of
/// them, the pre-2020-12 tuple form).
const SUBSCHEMA_KEYWORDS: [&str; 12] = [
    "additionalItems",
    "additionalProperties",
    "contains",
    "contentSchema",
    "else",
    "if",
    "items",
    "not",
    "propertyNames",
    "then",
    "unevaluatedItems",
    "unevaluatedProperties",
];

/// Keywords whose value is an array of subschemas.
const SUBSCHEMA_ARRAY_KEYWORDS: [&str; 4] = ["allOf", "anyOf", "oneOf", "prefixItems"];

/// Keywords whose value maps names to subschemas. Their keys are property or
/// definition names, never keywords, so a property named `description` stays.
const SUBSCHEMA_MAP_KEYWORDS: [&str; 5] = [
    "$defs",
    "definitions",
    "dependentSchemas",
    "patternProperties",
    "properties",
];

/// Remove the documentation keywords from every schema position of a JSON
/// Schema document, walking only into subschemas. Instance data (`const`,
/// `enum`, `default`, `examples`) and property names are left untouched, so
/// only doc-comment text, never structure, stops mattering to the match.
fn strip_schema_documentation(schema: &mut serde_json::Value) {
    let serde_json::Value::Object(map) = schema else {
        // `true`/`false` schemas carry no keywords.
        return;
    };
    for keyword in SCHEMA_DOCUMENTATION_KEYWORDS {
        map.remove(keyword);
    }
    for (keyword, value) in map.iter_mut() {
        let keyword = keyword.as_str();
        if SUBSCHEMA_KEYWORDS.contains(&keyword) {
            match value {
                serde_json::Value::Array(items) => {
                    items.iter_mut().for_each(strip_schema_documentation);
                }
                other => strip_schema_documentation(other),
            }
        } else if SUBSCHEMA_ARRAY_KEYWORDS.contains(&keyword) {
            if let serde_json::Value::Array(items) = value {
                items.iter_mut().for_each(strip_schema_documentation);
            }
        } else if SUBSCHEMA_MAP_KEYWORDS.contains(&keyword)
            && let serde_json::Value::Object(entries) = value
        {
            entries.values_mut().for_each(strip_schema_documentation);
        }
    }
}

fn require_adaptive_file<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<&'a [u8], PackValidationError> {
    files
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| PackValidationError::MissingAdaptiveFile {
            path: path.to_string(),
        })
}

fn validate_relative_adaptive_schema_path(path: &str) -> Result<(), PackValidationError> {
    if !path.starts_with("schemas/")
        || path.contains("..")
        || path.contains('\\')
        || path.ends_with('/')
    {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: format!("schema path `{path}` must stay under schemas/"),
        });
    }
    Ok(())
}

fn parse_manifest(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<MobpackManifest, PackValidationError> {
    let bytes = files
        .get("manifest.toml")
        .ok_or(PackValidationError::MissingManifest)?;
    let manifest_text = String::from_utf8(bytes.clone())
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    toml::from_str(&manifest_text)
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))
}

fn parse_definition(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<MobDefinition, PackValidationError> {
    let bytes = files
        .get("definition.json")
        .ok_or(PackValidationError::MissingDefinition)?;
    serde_json::from_slice(bytes).map_err(|err| PackValidationError::BadDefinition(err.to_string()))
}

fn validate_manifest_identity(manifest: &MobpackManifest) -> Result<(), PackValidationError> {
    if manifest.mobpack.name.trim().is_empty() {
        return Err(PackValidationError::InvalidManifestField {
            field: "mobpack.name".to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if manifest.mobpack.version.trim().is_empty() {
        return Err(PackValidationError::InvalidManifestField {
            field: "mobpack.version".to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    Ok(())
}

fn reject_realm_refs(definition: &MobDefinition) -> Result<(), PackValidationError> {
    for (name, binding) in &definition.profiles {
        if binding.realm_ref_name().is_some() {
            return Err(PackValidationError::RealmRefForbidden {
                profile_name: name.to_string(),
            });
        }
    }
    Ok(())
}

/// A pack cannot pin its members to a realm binding. A profile's
/// `auth_binding` is an explicit build input that resolves on the named
/// realm's chain, so only an operator's own definition may name one.
fn reject_profile_auth_bindings(definition: &MobDefinition) -> Result<(), PackValidationError> {
    for (name, binding) in &definition.profiles {
        if binding
            .as_inline()
            .is_some_and(|profile| profile.auth_binding.is_some())
        {
            return Err(PackValidationError::BadDefinition(format!(
                "profile `{name}` names an auth_binding, which a pack cannot set"
            )));
        }
    }
    Ok(())
}

fn validate_skill_paths(
    definition: &MobDefinition,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    for (skill_name, source) in &definition.skills {
        let SkillSource::Path { path } = source else {
            continue;
        };
        let normalized_path =
            normalize_for_archive(path).map_err(|err| PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: err.to_string(),
            })?;
        if normalized_path != *path {
            return Err(PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: format!("must use canonical archive path `{normalized_path}`"),
            });
        }
        if ArchiveSection::classify(&normalized_path) != Some(ArchiveSection::Skills) {
            return Err(PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: "must be under skills/".to_string(),
            });
        }
        let Some(bytes) = files.get(&normalized_path) else {
            return Err(PackValidationError::MissingSkillFile {
                skill_name: skill_name.clone(),
                path: normalized_path,
            });
        };
        std::str::from_utf8(bytes).map_err(|err| PackValidationError::InvalidSkillUtf8 {
            skill_name: skill_name.clone(),
            path: normalized_path,
            reason: err.to_string(),
        })?;
    }
    Ok(())
}

fn validate_required_files(files: &BTreeMap<String, Vec<u8>>) -> Result<(), PackValidationError> {
    if !files.contains_key("manifest.toml") {
        return Err(PackValidationError::MissingManifest);
    }
    if !files.contains_key("definition.json") {
        return Err(PackValidationError::MissingDefinition);
    }
    Ok(())
}

fn ensure_no_trust_section(files: &BTreeMap<String, Vec<u8>>) -> Result<(), PackValidationError> {
    let bytes = files
        .get("manifest.toml")
        .ok_or(PackValidationError::MissingManifest)?;
    let manifest_text = String::from_utf8(bytes.clone())
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    let value: toml::Value = toml::from_str(&manifest_text)
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    if value.get("trust").is_some() {
        return Err(PackValidationError::TrustSectionForbidden);
    }
    Ok(())
}

fn scan_directory(directory: &Path) -> Result<BTreeMap<String, Vec<u8>>, PackValidationError> {
    let mut files = BTreeMap::new();
    scan_directory_inner(directory, directory, &mut files)?;
    Ok(files)
}

fn scan_directory_inner(
    root: &Path,
    current: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            scan_directory_inner(root, &path, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        let rel_path = normalize_rel_path(&rel.to_string_lossy());
        out.insert(rel_path, std::fs::read(&path)?);
    }
    Ok(())
}

/// Removes the files `paths` name from the scanned set.
///
/// A file is left out by what it is, not by how its path or the directory's
/// is written: both are resolved through the file system before they are
/// compared, so relative and absolute forms, a leading `./`, `..` and
/// symbolic links on the way all name the same file. A path outside the
/// directory removes nothing.
fn exclude_paths_from_pack(
    root: &Path,
    paths: &[&Path],
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    if paths.is_empty() {
        return Ok(());
    }
    let root = std::fs::canonicalize(root)?;
    for path in paths {
        if let Ok(relative) = resolve_excluded_path(path).strip_prefix(&root) {
            files.remove(&normalize_rel_path(&relative.to_string_lossy()));
        }
    }
    Ok(())
}

/// The real path of a file to leave out of a pack.
///
/// A file that exists resolves whole. One that does not exist yet (the output
/// of a first pack) resolves through its directory, with its own name as
/// written. A path whose directory does not resolve either names no file the
/// scan can have found; it is made absolute on the current directory.
fn resolve_excluded_path(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        // A bare file name has the empty parent: the current directory.
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(real_parent) = std::fs::canonicalize(parent) {
            return real_parent.join(name);
        }
    }
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn normalize_rel_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn load_signing_key(path: &Path) -> Result<SigningKey, PackValidationError> {
    let raw = std::fs::read_to_string(path)?;
    let trimmed = raw.trim();
    let bytes = hex::decode(trimmed)
        .map_err(|err| PackValidationError::InvalidSigningKey(err.to_string()))?;
    let secret: [u8; 32] = bytes.try_into().map_err(|_| {
        PackValidationError::InvalidSigningKey(
            "expected 64 hex characters (32-byte key)".to_string(),
        )
    })?;
    Ok(SigningKey::from_bytes(&secret))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::signing::verify_digest;
    use tempfile::TempDir;

    #[test]
    fn test_pack_directory_produces_valid_archive() {
        let temp = fixture_mob_dir();
        let packed = pack_directory(temp.path(), None).unwrap();
        let extracted = extract_targz_safe(&packed.archive_bytes).unwrap();
        assert_eq!(
            extracted.get("skills/review.md"),
            Some(&b"# Review skill\n".to_vec())
        );
        assert_eq!(
            extracted.get("hooks/run.sh"),
            Some(&b"#!/bin/sh\necho run\n".to_vec())
        );
        assert!(extracted.contains_key("manifest.toml"));
        assert!(extracted.contains_key("definition.json"));
    }

    #[test]
    fn test_pack_digest_is_deterministic() {
        let temp = fixture_mob_dir();
        let one = pack_directory(temp.path(), None).unwrap();
        let two = pack_directory(temp.path(), None).unwrap();
        assert_eq!(one.digest, two.digest);
    }

    #[test]
    fn test_pack_digest_deterministic_with_output_inside_source_root_when_excluded() {
        let temp = fixture_mob_dir();
        let output_path = temp.path().join("out.mobpack");
        let first =
            pack_directory_with_excludes(temp.path(), None, &[output_path.as_path()]).unwrap();
        std::fs::write(&output_path, &first.archive_bytes).unwrap();
        let second =
            pack_directory_with_excludes(temp.path(), None, &[output_path.as_path()]).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.archive_bytes, second.archive_bytes);
    }

    #[test]
    fn test_pack_output_outside_source_root_preserves_behavior() {
        let temp = fixture_mob_dir();
        let outside = TempDir::new().unwrap();
        let outside_output = outside.path().join("out.mobpack");

        let baseline = pack_directory(temp.path(), None).unwrap();
        let with_excluded_outside =
            pack_directory_with_excludes(temp.path(), None, &[outside_output.as_path()]).unwrap();

        assert_eq!(baseline.digest, with_excluded_outside.digest);
        assert_eq!(baseline.archive_bytes, with_excluded_outside.archive_bytes);
    }

    // The signing key and the output file are left out by the file they name,
    // however the mob directory and their own paths are written.

    const EXCLUSION_SOURCES: [&str; 3] = ["definition.json", "manifest.toml", "skills/notes.md"];

    /// `<temp>/mob` with the pack sources only.
    fn exclusion_fixture() -> TempDir {
        let temp = TempDir::new().unwrap();
        let mob = temp.path().join("mob");
        std::fs::create_dir_all(mob.join("skills")).unwrap();
        std::fs::write(
            mob.join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(mob.join("definition.json"), br#"{"id":"mob"}"#).unwrap();
        std::fs::write(mob.join("skills").join("notes.md"), "# Notes skill\n").unwrap();
        temp
    }

    /// The same fixture with a signing key (`mob/key.hex`) and the output of
    /// an earlier pack (`mob/out.mobpack`) beside the sources.
    fn exclusion_fixture_with_key_and_output() -> TempDir {
        let temp = exclusion_fixture();
        write_throwaway_key(&temp.path().join("mob").join("key.hex"));
        std::fs::write(
            temp.path().join("mob").join("out.mobpack"),
            b"earlier output",
        )
        .unwrap();
        temp
    }

    /// Throwaway signing key: 32 bytes derived in the test, hex encoded.
    fn write_throwaway_key(path: &Path) {
        let bytes = Sha256::digest(b"mob pack exclusion rows throwaway key");
        std::fs::write(path, hex::encode(bytes)).unwrap();
    }

    fn signed_pack_files(dir: &Path, key: &Path, output: &Path) -> Vec<String> {
        let packed = pack_directory_with_excludes(
            dir,
            Some(SigningRequest {
                signer_id: "test-signer",
                key_path: key,
            }),
            &[output],
        )
        .unwrap();
        extract_targz_safe(&packed.archive_bytes)
            .unwrap()
            .into_keys()
            .collect()
    }

    fn sorted_names(names: &[&str]) -> Vec<String> {
        let mut names: Vec<String> = names.iter().map(|name| (*name).to_string()).collect();
        names.sort();
        names
    }

    /// The signed pack holds the sources and its signature: neither the key
    /// nor the output file.
    fn assert_key_and_output_left_out(spelling: &str, files: &[String]) {
        let packed: Vec<&str> = ["key.hex", "out.mobpack"]
            .into_iter()
            .filter(|name| files.iter().any(|file| file == name))
            .collect();
        assert!(
            packed.is_empty(),
            "{spelling}: packed {packed:?} (key.hex is the signing key, out.mobpack the output \
             file); files: {files:?}"
        );
        let mut expected = EXCLUSION_SOURCES.to_vec();
        expected.push("signature.toml");
        assert_eq!(files, sorted_names(&expected), "{spelling}");
    }

    /// Serializes the rows that set the process current directory. No other
    /// test of this crate depends on it.
    static CURRENT_DIR: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Runs `body` with the process current directory set to `dir` and puts
    /// the earlier one back afterwards, also when `body` panics.
    fn with_current_dir<T>(dir: &Path, body: impl FnOnce() -> T) -> T {
        struct Restore(std::path::PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                std::env::set_current_dir(&self.0).unwrap();
            }
        }
        let _serial = CURRENT_DIR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _restore = Restore(std::env::current_dir().unwrap());
        std::env::set_current_dir(dir).unwrap();
        body()
    }

    /// Where a spelling row's command line is typed.
    #[derive(Clone, Copy)]
    enum TypedIn {
        /// Inside the mob directory: the directory argument is `.`.
        MobDirectory,
        /// In the directory that holds `mob`.
        ParentDirectory,
    }

    /// One spelling of the directory, the key and the output, as typed in
    /// `typed_in`. The fixture holds `mob/key.hex` and `mob/out.mobpack`.
    fn assert_spelling_leaves_key_and_output_out(
        temp: &TempDir,
        typed_in: TypedIn,
        dir: &Path,
        key: &Path,
        output: &Path,
    ) {
        let current = match typed_in {
            TypedIn::MobDirectory => temp.path().join("mob"),
            TypedIn::ParentDirectory => temp.path().to_path_buf(),
        };
        let files = with_current_dir(&current, || signed_pack_files(dir, key, output));
        assert_key_and_output_left_out(
            &format!(
                "directory `{}`, key `{}`, output `{}`",
                dir.display(),
                key.display(),
                output.display()
            ),
            &files,
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_dot_with_bare_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::MobDirectory,
            Path::new("."),
            Path::new("key.hex"),
            Path::new("out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_dot_with_dot_slash_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::MobDirectory,
            Path::new("."),
            Path::new("./key.hex"),
            Path::new("./out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_dir_with_dir_prefixed_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            Path::new("mob"),
            Path::new("mob/key.hex"),
            Path::new("mob/out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_dot_slash_dir_with_dir_prefixed_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            Path::new("./mob"),
            Path::new("mob/key.hex"),
            Path::new("mob/out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_dir_with_dot_slash_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            Path::new("mob"),
            Path::new("./mob/key.hex"),
            Path::new("./mob/out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_absolute_dir_with_relative_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            &temp.path().join("mob"),
            Path::new("mob/key.hex"),
            Path::new("mob/out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_relative_dir_with_absolute_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            Path::new("mob"),
            &temp.path().join("mob").join("key.hex"),
            &temp.path().join("mob").join("out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_spelling_absolute_dir_with_absolute_key_and_output() {
        let temp = exclusion_fixture_with_key_and_output();
        assert_spelling_leaves_key_and_output_out(
            &temp,
            TypedIn::ParentDirectory,
            &temp.path().join("mob"),
            &temp.path().join("mob").join("key.hex"),
            &temp.path().join("mob").join("out.mobpack"),
        );
    }

    #[test]
    fn test_pack_exclusion_guard_directory_with_trailing_separator() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        let mut trailing = mob.clone().into_os_string();
        trailing.push(std::path::MAIN_SEPARATOR_STR);
        let files = signed_pack_files(
            Path::new(&trailing),
            &mob.join("key.hex"),
            &mob.join("out.mobpack"),
        );
        assert_key_and_output_left_out("directory with a trailing separator", &files);
    }

    #[test]
    fn test_pack_exclusion_parent_component() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        let through_parent = mob.join("..").join("mob");

        let files = signed_pack_files(
            &through_parent,
            &mob.join("key.hex"),
            &mob.join("out.mobpack"),
        );
        assert_key_and_output_left_out("directory written mob/../mob", &files);

        let files = signed_pack_files(
            &mob,
            &mob.join("skills").join("..").join("key.hex"),
            &mob.join("skills").join("..").join("out.mobpack"),
        );
        assert_key_and_output_left_out("key and output written mob/skills/../<name>", &files);
    }

    #[cfg(unix)]
    #[test]
    fn test_pack_exclusion_symlinked_directory() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        let link = temp.path().join("mob-link");
        std::os::unix::fs::symlink(&mob, &link).unwrap();

        let files = signed_pack_files(&link, &mob.join("key.hex"), &mob.join("out.mobpack"));
        assert_key_and_output_left_out("symlinked directory, real key and output", &files);

        let files = signed_pack_files(&mob, &link.join("key.hex"), &link.join("out.mobpack"));
        assert_key_and_output_left_out("real directory, key and output through the link", &files);
    }

    #[cfg(unix)]
    #[test]
    fn test_pack_exclusion_key_given_as_symlink() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");

        // A link outside the directory that names the key inside it is the
        // key inside it.
        let outside_link = temp.path().join("key-link.hex");
        std::os::unix::fs::symlink(mob.join("key.hex"), &outside_link).unwrap();
        let files = signed_pack_files(&mob, &outside_link, &mob.join("out.mobpack"));
        assert_key_and_output_left_out("key given as a link outside the directory", &files);

        // So is a link inside the directory: the scan skips the link, and the
        // key it names is left out.
        let inside_link = mob.join("key-link.hex");
        std::os::unix::fs::symlink("key.hex", &inside_link).unwrap();
        let files = signed_pack_files(&mob, &inside_link, &mob.join("out.mobpack"));
        assert_key_and_output_left_out("key given as a link inside the directory", &files);
    }

    #[cfg(unix)]
    #[test]
    fn test_pack_exclusion_guard_symlinks_inside_directory_are_skipped() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        write_throwaway_key(&outside.join("key.hex"));

        // Links in the directory to the key inside it and to a key outside it.
        std::os::unix::fs::symlink("key.hex", mob.join("key-link.hex")).unwrap();
        std::os::unix::fs::symlink(outside.join("key.hex"), mob.join("outside-key-link.hex"))
            .unwrap();

        let files = signed_pack_files(&mob, &mob.join("key.hex"), &mob.join("out.mobpack"));
        assert_key_and_output_left_out("links to keys beside the sources", &files);
    }

    #[cfg(unix)]
    #[test]
    fn test_pack_exclusion_guard_hard_link_under_another_name_is_a_second_file() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        std::fs::hard_link(mob.join("key.hex"), mob.join("key-copy.hex")).unwrap();

        // The exclusion follows the paths it is given. No path names the
        // second name of the key file, so it is packed like any other file.
        let files = signed_pack_files(&mob, &mob.join("key.hex"), &mob.join("out.mobpack"));
        let mut expected = EXCLUSION_SOURCES.to_vec();
        expected.extend(["key-copy.hex", "signature.toml"]);
        assert_eq!(files, sorted_names(&expected));
    }

    #[test]
    fn test_pack_exclusion_guard_key_this_pack_is_not_signed_with_is_an_ordinary_file() {
        let temp = exclusion_fixture();
        let mob = temp.path().join("mob");
        write_throwaway_key(&mob.join("key.hex"));
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        write_throwaway_key(&outside.join("other-key.hex"));
        let mut expected = EXCLUSION_SOURCES.to_vec();
        expected.push("key.hex");

        // An unsigned pack is told of no key.
        let unsigned: Vec<String> =
            extract_targz_safe(&pack_directory(&mob, None).unwrap().archive_bytes)
                .unwrap()
                .into_keys()
                .collect();
        assert_eq!(unsigned, sorted_names(&expected));

        // A pack signed with another key leaves out that key alone.
        let files = signed_pack_files(
            &mob,
            &outside.join("other-key.hex"),
            &outside.join("out.mobpack"),
        );
        expected.push("signature.toml");
        assert_eq!(files, sorted_names(&expected));
    }

    #[test]
    fn test_pack_exclusion_guard_earlier_pack_under_another_name_is_an_ordinary_file() {
        let temp = exclusion_fixture();
        let mob = temp.path().join("mob");
        std::fs::write(mob.join("earlier.mobpack"), b"earlier output").unwrap();

        // Only the output this pack is written to is left out.
        let packed =
            pack_directory_with_excludes(&mob, None, &[mob.join("out.mobpack").as_path()]).unwrap();
        let files: Vec<String> = extract_targz_safe(&packed.archive_bytes)
            .unwrap()
            .into_keys()
            .collect();
        let mut expected = EXCLUSION_SOURCES.to_vec();
        expected.push("earlier.mobpack");
        assert_eq!(files, sorted_names(&expected));
    }

    #[test]
    fn test_pack_exclusion_is_by_file_not_by_name() {
        let temp = exclusion_fixture_with_key_and_output();
        let mob = temp.path().join("mob");
        std::fs::write(mob.join("skills").join("key.hex"), "not the signing key\n").unwrap();
        std::fs::write(mob.join("skills").join("out.mobpack"), "not the output\n").unwrap();

        let files = signed_pack_files(
            &mob.join("..").join("mob"),
            &mob.join("key.hex"),
            &mob.join("out.mobpack"),
        );
        assert!(
            files.iter().any(|file| file == "skills/key.hex"),
            "a file that only shares the key's name must stay packed: {files:?}"
        );
        assert!(
            files.iter().any(|file| file == "skills/out.mobpack"),
            "a file that only shares the output's name must stay packed: {files:?}"
        );
        let mut expected = EXCLUSION_SOURCES.to_vec();
        expected.extend(["signature.toml", "skills/key.hex", "skills/out.mobpack"]);
        assert_eq!(files, sorted_names(&expected));
    }

    #[test]
    fn test_pack_exclusion_output_not_yet_written() {
        let temp = exclusion_fixture();
        let mob = temp.path().join("mob");
        let through_parent = mob.join("..").join("mob");
        let output = mob.join("new.mobpack");
        assert!(!output.exists());

        let first = pack_directory_with_excludes(&through_parent, None, &[output.as_path()])
            .expect("an output that does not exist yet is nothing to leave out");
        let first_files: Vec<String> = extract_targz_safe(&first.archive_bytes)
            .unwrap()
            .into_keys()
            .collect();
        assert_eq!(first_files, sorted_names(&EXCLUSION_SOURCES));

        // The second pack finds the first one's output in the directory.
        std::fs::write(&output, &first.archive_bytes).unwrap();
        let second =
            pack_directory_with_excludes(&through_parent, None, &[output.as_path()]).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.archive_bytes, second.archive_bytes);

        // Neither does an output whose own directory does not exist yet.
        let missing_parent = mob.join("dist").join("new.mobpack");
        std::fs::remove_file(&output).unwrap();
        let third =
            pack_directory_with_excludes(&through_parent, None, &[missing_parent.as_path()])
                .unwrap();
        assert_eq!(first.archive_bytes, third.archive_bytes);
    }

    #[test]
    fn test_pack_exclusion_guard_key_and_output_outside_directory_change_nothing() {
        let temp = exclusion_fixture();
        let mob = temp.path().join("mob");
        // Files inside the directory that share the outside files' names.
        std::fs::write(mob.join("key.hex"), "a source file\n").unwrap();
        std::fs::write(mob.join("out.mobpack"), "another source file\n").unwrap();
        let baseline =
            extract_targz_safe(&pack_directory(&mob, None).unwrap().archive_bytes).unwrap();

        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        write_throwaway_key(&outside.join("key.hex"));
        std::fs::write(outside.join("out.mobpack"), b"earlier output").unwrap();

        for (spelling, key, output) in [
            (
                "outside paths",
                outside.join("key.hex"),
                outside.join("out.mobpack"),
            ),
            (
                "outside paths written through the directory",
                mob.join("..").join("outside").join("key.hex"),
                mob.join("..").join("outside").join("out.mobpack"),
            ),
        ] {
            let packed = pack_directory_with_excludes(
                &mob,
                Some(SigningRequest {
                    signer_id: "test-signer",
                    key_path: &key,
                }),
                &[output.as_path()],
            )
            .unwrap();
            let mut files = extract_targz_safe(&packed.archive_bytes).unwrap();
            assert!(
                files.remove("signature.toml").is_some(),
                "{spelling}: the pack is signed"
            );
            assert_eq!(files, baseline, "{spelling}");
        }
    }

    #[test]
    fn test_pack_sign_produces_valid_signature() {
        let temp = fixture_mob_dir();
        let key_path = temp.path().join("signing.key");
        std::fs::write(
            &key_path,
            "0909090909090909090909090909090909090909090909090909090909090909",
        )
        .unwrap();

        let packed = pack_directory(
            temp.path(),
            Some(SigningRequest {
                signer_id: "test-signer",
                key_path: &key_path,
            }),
        )
        .unwrap();
        let extracted = extract_targz_safe(&packed.archive_bytes).unwrap();
        let signature_toml = extracted.get("signature.toml").expect("signature.toml");
        let signature: PackSignature =
            toml::from_str(std::str::from_utf8(signature_toml).unwrap()).unwrap();
        let recomputed = compute_archive_digest(&packed.archive_bytes).unwrap();
        assert_eq!(recomputed, packed.digest);
        assert_eq!(signature.digest, packed.digest);
        assert_eq!(signature.signer_id.as_str(), "test-signer");

        verify_digest(
            signature.public_key.verifying_key(),
            signature.digest,
            signature.signature.signature(),
        )
        .unwrap();
    }

    #[test]
    fn test_pack_rejects_missing_manifest() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("definition.json"), "{\"id\":\"mob\"}").unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::MissingManifest));
    }

    #[test]
    fn test_pack_rejects_missing_definition() {
        let temp = TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"x\"\nversion = \"1\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::MissingDefinition));
    }

    #[test]
    fn test_pack_rejects_manifest_trust_section() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[trust]\npolicy = \"strict\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::TrustSectionForbidden));
    }

    #[test]
    fn test_pack_rejects_blank_manifest_name() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"   \"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidManifestField { field, .. } if field == "mobpack.name"
        ));
    }

    #[test]
    fn test_pack_rejects_blank_manifest_version() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidManifestField { field, .. } if field == "mobpack.version"
        ));
    }

    #[test]
    fn test_pack_rejects_realm_profile_refs() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("definition.json"),
            br#"{"id":"mob","profiles":{"worker":{"realm_profile":"prod-worker"}}}"#,
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::RealmRefForbidden { profile_name } if profile_name == "worker"
        ));
    }

    #[test]
    fn test_pack_rejects_a_profile_auth_binding() {
        // A pack's definition cannot pin its members to a realm binding: a
        // profile binding is an explicit build input, and only an operator's
        // own definition may name one.
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("definition.json"),
            br#"{"id":"mob","profiles":{"worker":{"model":"claude-sonnet-4-5","auth_binding":{"realm":"global","binding":"team_key"}}}}"#,
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(
            matches!(
                &err,
                PackValidationError::BadDefinition(reason)
                    if reason.contains("`worker`") && reason.contains("auth_binding")
            ),
            "a pack whose profile names an auth binding is refused: {err}"
        );
    }

    #[test]
    fn test_pack_rejects_missing_packed_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("skills/missing.md");
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingSkillFile { skill_name, path }
                if skill_name == "review" && path == "skills/missing.md"
        ));
    }

    #[test]
    fn test_pack_rejects_non_skills_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("config/review.md");
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(temp.path().join("config").join("review.md"), "# Review\n").unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillPath { skill_name, path, .. }
                if skill_name == "review" && path == "config/review.md"
        ));
    }

    #[test]
    fn test_pack_rejects_noncanonical_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("./skills/review.md");
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillPath { skill_name, path, reason }
                if skill_name == "review"
                    && path == "./skills/review.md"
                    && reason.contains("canonical archive path")
        ));
    }

    /// A complete adaptive policy document: every limit non-zero so
    /// `AdaptiveLimitRecord::validate_complete` accepts it.
    const COMPLETE_POLICY: &[u8] = b"[limits]\n\
max_depth = 3\n\
max_total_decisions = 6\n\
max_repair_attempts = 2\n\
max_layer_failures = 2\n\
max_attempts_per_layer = 2\n\
max_members_per_layer = 4\n\
max_total_spawned_members = 12\n\
max_active_members = 4\n\
max_retained_layer_mobs = 1\n\
max_wall_clock_ms = 180000\n\
max_aggregate_tokens = 200000\n\
max_aggregate_tool_calls = 100\n";

    fn adaptive_manifest_text(policy: &[u8]) -> String {
        let policy_digest = format!("sha256:{}", hex::encode(Sha256::digest(policy)));
        format!(
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[adaptive]\nflowmaster_profile = \"flowmaster\"\nobjective_class = \"audit\"\npolicy_digest = \"{policy_digest}\"\nrequired_schemas = [\"finding-set\"]\n"
        )
    }

    /// Write a complete adaptive pack source directory. Authors do not supply
    /// `adaptive/layer-decision.schema.json` — the builder emits it.
    fn adaptive_fixture_dir(policy: &[u8]) -> TempDir {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            adaptive_manifest_text(policy),
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("adaptive")).unwrap();
        std::fs::create_dir_all(temp.path().join("schemas")).unwrap();
        std::fs::write(temp.path().join("adaptive/policies.toml"), policy).unwrap();
        std::fs::write(temp.path().join("adaptive/flowmaster.prompt.md"), "Plan").unwrap();
        std::fs::write(
            temp.path().join("schemas/registry.json"),
            br#"{"finding-set":"schemas/finding-set.schema.json"}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("schemas/finding-set.schema.json"),
            br#"{"type":"object","required":["findings"],"properties":{"findings":{"type":"array","items":{"type":"object"}}}}"#,
        )
        .unwrap();
        temp
    }

    /// An extracted-files map for a complete, builder-shaped adaptive pack
    /// (stamped capability + canonical schema), for load-level tampering tests.
    fn adaptive_pack_files() -> BTreeMap<String, Vec<u8>> {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        let packed = pack_directory(temp.path(), None).unwrap();
        extract_targz_safe(&packed.archive_bytes).unwrap()
    }

    #[test]
    fn test_adaptive_manifest_requires_typed_payload_files_and_policy_digest() {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        pack_directory(temp.path(), None).expect("complete adaptive payload validates");

        // Tampering with the policy after digesting fails closed.
        let mut tampered = COMPLETE_POLICY.to_vec();
        tampered.extend_from_slice(b"# drift\n");
        std::fs::write(temp.path().join("adaptive/policies.toml"), &tampered).unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/policies.toml" && reason.contains("policy_digest mismatch")
        ));

        // Load-level: an archive missing the layer-decision schema fails
        // closed (the builder always emits it, so absence means tampering or
        // foreign tooling).
        let mut files = adaptive_pack_files();
        files.remove("adaptive/layer-decision.schema.json").unwrap();
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingAdaptiveFile { path }
                if path == "adaptive/layer-decision.schema.json"
        ));
    }

    #[test]
    fn test_builder_stamps_adaptive_capability_idempotently() {
        // First pack: manifest has no [requires]; the builder stamps it.
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        let packed = pack_directory(temp.path(), None).unwrap();
        let files = extract_targz_safe(&packed.archive_bytes).unwrap();
        let manifest: MobpackManifest =
            toml::from_str(std::str::from_utf8(files.get("manifest.toml").unwrap()).unwrap())
                .unwrap();
        let stamped_tokens = manifest
            .requires
            .as_ref()
            .expect("builder must create [requires]")
            .capabilities
            .iter()
            .filter(|cap| cap.token() == "adaptive_flow")
            .count();
        assert_eq!(stamped_tokens, 1);

        // Second pack from a source dir whose manifest is already stamped:
        // no duplicate entry, and the manifest survives byte-identical.
        let already = adaptive_fixture_dir(COMPLETE_POLICY);
        let stamped_manifest =
            String::from_utf8(files.get("manifest.toml").unwrap().clone()).unwrap();
        std::fs::write(already.path().join("manifest.toml"), &stamped_manifest).unwrap();
        let repacked = pack_directory(already.path(), None).unwrap();
        let refiles = extract_targz_safe(&repacked.archive_bytes).unwrap();
        assert_eq!(
            std::str::from_utf8(refiles.get("manifest.toml").unwrap()).unwrap(),
            stamped_manifest,
            "an already-stamped manifest must pass through unchanged"
        );
    }

    #[test]
    fn test_builder_emits_canonical_layer_decision_schema() {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        // Even a hand-supplied (vacuous) schema is overwritten by the builder.
        std::fs::write(
            temp.path().join("adaptive/layer-decision.schema.json"),
            br#"{"type":"object"}"#,
        )
        .unwrap();
        let packed = pack_directory(temp.path(), None).unwrap();
        let files = extract_targz_safe(&packed.archive_bytes).unwrap();
        assert_eq!(
            files.get("adaptive/layer-decision.schema.json").unwrap(),
            &canonical_layer_decision_schema_bytes().unwrap(),
        );
        let emitted: serde_json::Value =
            serde_json::from_slice(files.get("adaptive/layer-decision.schema.json").unwrap())
                .unwrap();
        assert!(
            emitted.get("$defs").is_some(),
            "emitted schema must be the structural LayerDecision schema"
        );
    }

    #[test]
    fn test_load_rejects_prestamp_adaptive_pack_without_capability() {
        // Simulate a pack built by pre-stamp tooling: complete adaptive
        // payload, canonical schema, but no adaptive_flow under [requires].
        let mut files = adaptive_pack_files();
        files.insert(
            "manifest.toml".to_string(),
            adaptive_manifest_text(COMPLETE_POLICY).into_bytes(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingAdaptiveCapability { capability }
                if capability == "adaptive_flow"
        ));
    }

    #[test]
    fn test_load_accepts_stamped_adaptive_pack() {
        let files = adaptive_pack_files();
        validate_extracted_pack_files(&files).expect("builder-produced adaptive pack loads");
    }

    #[test]
    fn test_load_rejects_unknown_future_capability_with_supported_set() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[requires]\ncapabilities = [\"comms\", \"quantum_flux\"]\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        match err {
            PackValidationError::UnknownRequiredCapability {
                capability,
                supported,
            } => {
                assert_eq!(capability, "quantum_flux");
                assert!(supported.iter().any(|t| t == "comms"));
                assert!(supported.iter().any(|t| t == "adaptive_flow"));
            }
            other => panic!("expected UnknownRequiredCapability, got {other:?}"),
        }
    }

    #[test]
    fn test_pack_rejects_garbage_adaptive_policy_toml() {
        let policy = b"limits = \"not a table\"\nthis is not TOML at all {{{";
        let temp = adaptive_fixture_dir(policy);
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, .. }
                if path == "adaptive/policies.toml"
        ));
    }

    #[test]
    fn test_pack_rejects_incomplete_adaptive_policy_limits() {
        // Zero limits parse but fail validate_complete at pack time.
        let policy = String::from_utf8(COMPLETE_POLICY.to_vec())
            .unwrap()
            .replace("max_depth = 3", "max_depth = 0");
        let temp = adaptive_fixture_dir(policy.as_bytes());
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/policies.toml" && reason.contains("max_depth")
        ));
    }

    #[test]
    fn test_load_rejects_non_json_layer_decision_schema() {
        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            b"not json at all".to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/layer-decision.schema.json"
                    && reason.contains("not valid JSON")
        ));

        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            b"true".to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/layer-decision.schema.json"
                    && reason.contains("JSON object")
        ));
    }

    #[test]
    fn test_load_rejects_stale_or_vacuous_layer_decision_schema() {
        // The vacuous hand-rolled shape from pre-0.7.1 packs fails closed
        // with a regenerate hint, as does any schema diverging from the
        // host's canonical LayerDecision schema.
        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            br#"{"type":"object"}"#.to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::StaleAdaptiveSchema { path }
                if path == "adaptive/layer-decision.schema.json"
        ));
    }

    #[test]
    fn test_layer_decision_schema_match_is_key_order_insensitive() {
        // Match rule: structural equality (canonical-serialization byte
        // equality), so a re-serialized canonical schema with different
        // formatting still matches.
        let canonical = canonical_layer_decision_schema().unwrap();
        let compact = serde_json::to_vec(&canonical).unwrap();
        let mut files = adaptive_pack_files();
        files.insert("adaptive/layer-decision.schema.json".to_string(), compact);
        validate_extracted_pack_files(&files)
            .expect("formatting-only differences must not fail the schema match");
    }

    /// Reword every doc string (`description`, `title`) in a schema, as a pack
    /// built by a release with different doc comments would carry them.
    /// Returns how many it reworded.
    fn reword_schema_documentation(value: &mut serde_json::Value) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                let mut reworded = 0;
                for (key, entry) in map.iter_mut() {
                    if (key == "description" || key == "title")
                        && let serde_json::Value::String(text) = entry
                    {
                        text.push_str(" (as worded by an earlier release)");
                        reworded += 1;
                    } else {
                        reworded += reword_schema_documentation(entry);
                    }
                }
                reworded
            }
            serde_json::Value::Array(items) => {
                items.iter_mut().map(reword_schema_documentation).sum()
            }
            _ => 0,
        }
    }

    #[test]
    fn test_layer_decision_schema_match_ignores_documentation_text() {
        // The LayerDecision schema carries Rust doc comments as `description`
        // (and `title`) text. Rewording one must not make every adaptive pack
        // built by an earlier release fail with StaleAdaptiveSchema.
        let canonical = canonical_layer_decision_schema().unwrap();
        let mut reworded = canonical.clone();
        assert!(
            reword_schema_documentation(&mut reworded) > 0,
            "the canonical schema must carry doc text for this test to mean anything"
        );
        if let serde_json::Value::Object(root) = &mut reworded {
            root.insert(
                "title".to_string(),
                serde_json::Value::String("A title an earlier release added".to_string()),
            );
        }
        assert_ne!(reworded, canonical);
        let mut files = adaptive_pack_files();
        files.insert(
            LAYER_DECISION_SCHEMA_PATH.to_string(),
            serde_json::to_vec_pretty(&reworded).unwrap(),
        );
        validate_extracted_pack_files(&files)
            .expect("documentation-only differences must not fail the schema match");
    }

    #[test]
    fn test_layer_decision_schema_match_rejects_structural_drift_beside_doc_text() {
        // Ignoring doc text must not hide a structural change: drop one
        // property of one definition and keep every description intact.
        let mut drifted = canonical_layer_decision_schema().unwrap();
        let removed = drifted
            .get_mut("$defs")
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|defs| {
                defs.values_mut().find_map(|definition| {
                    let properties = definition
                        .get_mut("properties")
                        .and_then(serde_json::Value::as_object_mut)?;
                    let name = properties.keys().next()?.clone();
                    properties.remove(&name)
                })
            });
        assert!(
            removed.is_some(),
            "the canonical schema must define properties"
        );
        let mut files = adaptive_pack_files();
        files.insert(
            LAYER_DECISION_SCHEMA_PATH.to_string(),
            serde_json::to_vec(&drifted).unwrap(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::StaleAdaptiveSchema { path }
                if path == "adaptive/layer-decision.schema.json"
        ));
    }

    #[test]
    fn test_schema_documentation_strip_keeps_property_names_and_instance_data() {
        // Only doc keywords at schema positions go: a property or definition
        // NAMED `description`/`title` stays, and so does instance data that
        // happens to carry such keys.
        let mut schema = serde_json::json!({
            "title": "Root",
            "description": "root doc",
            "type": "object",
            "required": ["description", "title"],
            "properties": {
                "description": { "type": "string", "description": "a field named description" },
                "title": { "type": "integer", "title": "a field named title" },
                "mode": {
                    "const": { "description": "instance data" },
                    "default": { "title": "instance data" },
                    "enum": [{ "description": "instance data" }],
                    "examples": [{ "title": "instance data" }]
                }
            },
            "$defs": {
                "description": {
                    "description": "definition doc",
                    "anyOf": [{ "description": "branch doc", "type": "null" }],
                    "items": { "description": "item doc" },
                    "additionalProperties": { "title": "value doc" }
                }
            },
            "prefixItems": [{ "description": "tuple doc", "type": "string" }],
            "items": [{ "description": "legacy tuple doc" }]
        });
        strip_schema_documentation(&mut schema);
        assert_eq!(
            schema,
            serde_json::json!({
                "type": "object",
                "required": ["description", "title"],
                "properties": {
                    "description": { "type": "string" },
                    "title": { "type": "integer" },
                    "mode": {
                        "const": { "description": "instance data" },
                        "default": { "title": "instance data" },
                        "enum": [{ "description": "instance data" }],
                        "examples": [{ "title": "instance data" }]
                    }
                },
                "$defs": {
                    "description": {
                        "anyOf": [{ "type": "null" }],
                        "items": {},
                        "additionalProperties": {}
                    }
                },
                "prefixItems": [{ "type": "string" }],
                "items": [{}]
            })
        );
    }

    /// How the `ToolConfig::deny` doc comment ends in 0.9.1, as schemars
    /// renders it into the LayerDecision schema's `deny` description (doc
    /// lines joined with "\n"). 0.9.1 hosts compare a pack's bundled schema
    /// with their own including this text.
    const DENY_DOC_0_9_1_TAIL: &str = "server's `tool_names`. Denied tools stay listed and the execution gate\n\
refuses every call by name.";

    /// The rewording of that ending (what #1807 made true), deferred to a
    /// release after 0.9.2 so that packs built by 0.9.2 still load on 0.9.1.
    const DENY_DOC_REWORDED_TAIL: &str = "server's `tool_names`. Denied tools are not offered to the model: every\n\
build removes them from the member's visible tool list, and the\n\
execution gate still refuses every call by name.";

    /// Apply `edit` to every `deny` property schema in a LayerDecision schema
    /// and return how many it edited.
    fn edit_deny_property_schemas(
        value: &mut serde_json::Value,
        edit: &mut dyn FnMut(&mut serde_json::Map<String, serde_json::Value>),
    ) -> usize {
        match value {
            serde_json::Value::Object(map) => {
                let mut edited = 0;
                if let Some(serde_json::Value::Object(deny)) = map
                    .get_mut("properties")
                    .and_then(|properties| properties.get_mut("deny"))
                {
                    edit(deny);
                    edited += 1;
                }
                for child in map.values_mut() {
                    edited += edit_deny_property_schemas(child, edit);
                }
                edited
            }
            serde_json::Value::Array(items) => {
                let mut edited = 0;
                for item in items {
                    edited += edit_deny_property_schemas(item, edit);
                }
                edited
            }
            _ => 0,
        }
    }

    /// A bundled schema whose `deny` description ends with `tail` instead of
    /// the wording this host renders.
    fn schema_with_deny_doc_tail(tail: &str) -> serde_json::Value {
        let mut schema = canonical_layer_decision_schema().unwrap();
        let edited = edit_deny_property_schemas(&mut schema, &mut |deny| {
            let description = deny
                .get("description")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let body = description
                .strip_suffix(DENY_DOC_0_9_1_TAIL)
                .or_else(|| description.strip_suffix(DENY_DOC_REWORDED_TAIL))
                .unwrap_or_else(|| panic!("unknown deny doc wording: {description:?}"))
                .to_owned();
            deny.insert(
                "description".to_string(),
                serde_json::Value::String(body + tail),
            );
        });
        assert!(
            edited >= 1,
            "the LayerDecision schema must carry ToolConfig::deny"
        );
        schema
    }

    /// The canonical LayerDecision schema of 0.9.1: what its pack builder
    /// bundled into every adaptive pack, and what its hosts require a bundled
    /// schema to equal exactly, documentation text included (JSON equality;
    /// the fixture escapes non-ASCII text, so only its bytes differ from the
    /// builder's). Captured while every source the schema derives from was
    /// still identical to v0.9.1. It is a released artifact: never regenerate
    /// it from the current code.
    const LAYER_DECISION_SCHEMA_V0_9_1: &str =
        include_str!("../tests/fixtures/layer_decision_schema_v0_9_1.json");

    fn layer_decision_schema_v0_9_1() -> serde_json::Value {
        serde_json::from_str(LAYER_DECISION_SCHEMA_V0_9_1).expect("the 0.9.1 schema fixture parses")
    }

    /// The JSON pointer of the first place where `left` and `right` differ.
    fn first_json_difference(
        left: &serde_json::Value,
        right: &serde_json::Value,
        pointer: &str,
    ) -> Option<String> {
        use serde_json::Value;
        match (left, right) {
            (Value::Object(left), Value::Object(right)) => {
                let child =
                    |key: &str| format!("{pointer}/{}", key.replace('~', "~0").replace('/', "~1"));
                for (key, value) in left {
                    let Some(other) = right.get(key) else {
                        return Some(child(key));
                    };
                    if let Some(found) = first_json_difference(value, other, &child(key)) {
                        return Some(found);
                    }
                }
                right
                    .keys()
                    .find(|key| !left.contains_key(*key))
                    .map(|key| child(key))
            }
            (Value::Array(left), Value::Array(right)) => {
                for (index, (value, other)) in left.iter().zip(right).enumerate() {
                    if let Some(found) =
                        first_json_difference(value, other, &format!("{pointer}/{index}"))
                    {
                        return Some(found);
                    }
                }
                (left.len() != right.len()).then(|| pointer.to_owned())
            }
            _ => (left != right).then(|| pointer.to_owned()),
        }
    }

    #[test]
    fn test_this_host_renders_the_whole_0_9_1_layer_decision_schema() {
        // 0.9.1 hosts compare a pack's bundled schema with their own, doc
        // text included, so a pack this host builds loads on them only while
        // the whole canonical schema is the 0.9.1 one. A reworded doc comment
        // on any type the schema reaches (the `ToolConfig::deny` rewording
        // among them) waits until hosts that ignore schema doc text (0.9.2
        // and later) replace them; a structural change is a new schema for
        // every host.
        let golden = layer_decision_schema_v0_9_1();
        let canonical = canonical_layer_decision_schema().unwrap();
        if let Some(pointer) = first_json_difference(&canonical, &golden, "") {
            panic!(
                "the canonical LayerDecision schema differs from the 0.9.1 schema at `{pointer}` \
                 (now {:?}, 0.9.1 {:?}), so 0.9.1 hosts would refuse every adaptive pack this \
                 host builds as a stale schema",
                canonical.pointer(&pointer),
                golden.pointer(&pointer),
            );
        }
        assert_eq!(canonical, golden);
        let bundled: serde_json::Value =
            serde_json::from_slice(&canonical_layer_decision_schema_bytes().unwrap()).unwrap();
        assert_eq!(bundled, golden, "the builder bundles the 0.9.1 schema");
    }

    #[test]
    fn test_a_pack_carrying_the_0_9_1_schema_validates() {
        // The schema a 0.9.1 builder bundled loads on this host.
        let mut files = adaptive_pack_files();
        files.insert(
            LAYER_DECISION_SCHEMA_PATH.to_string(),
            LAYER_DECISION_SCHEMA_V0_9_1.as_bytes().to_vec(),
        );
        validate_extracted_pack_files(&files).unwrap();
    }

    #[test]
    fn test_the_0_9_1_deny_doc_tail_is_the_released_text() {
        // The tolerant-match tests below reword the released deny text; keep
        // their constant bound to the 0.9.1 schema itself.
        let mut golden = layer_decision_schema_v0_9_1();
        let mut descriptions = Vec::new();
        let edited = edit_deny_property_schemas(&mut golden, &mut |deny| {
            descriptions.push(
                deny.get("description")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            );
        });
        assert!(
            edited >= 1,
            "the 0.9.1 LayerDecision schema carries ToolConfig::deny"
        );
        for description in descriptions {
            let description = description.expect("ToolConfig::deny carries its doc text");
            assert!(
                description.ends_with(DENY_DOC_0_9_1_TAIL),
                "DENY_DOC_0_9_1_TAIL is not the released deny text: {description:?}"
            );
        }
    }

    #[test]
    fn test_first_json_difference_names_the_differing_pointer() {
        let base = serde_json::json!({ "a/b": [1, { "c": true }], "d~": "x" });
        assert_eq!(first_json_difference(&base, &base, ""), None);
        let mut changed = base.clone();
        changed["a/b"][1]["c"] = serde_json::json!(false);
        assert_eq!(
            first_json_difference(&changed, &base, "").as_deref(),
            Some("/a~1b/1/c")
        );
        let mut added = base.clone();
        added["d~"] = serde_json::json!("y");
        added["e"] = serde_json::json!(1);
        assert_eq!(
            first_json_difference(&added, &base, "").as_deref(),
            Some("/d~0")
        );
        let mut removed = base.clone();
        removed.as_object_mut().unwrap().remove("d~");
        assert_eq!(
            first_json_difference(&removed, &base, "").as_deref(),
            Some("/d~0")
        );
        let mut longer = base.clone();
        longer["a/b"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(2));
        assert_eq!(
            first_json_difference(&longer, &base, "").as_deref(),
            Some("/a~1b")
        );
    }

    #[test]
    fn test_packs_with_the_0_9_1_or_the_reworded_deny_doc_validate() {
        // This host ignores schema doc text, so it loads packs built by 0.9.1
        // and packs built by a later host that rewords the doc comment.
        for tail in [DENY_DOC_0_9_1_TAIL, DENY_DOC_REWORDED_TAIL] {
            let mut files = adaptive_pack_files();
            files.insert(
                LAYER_DECISION_SCHEMA_PATH.to_string(),
                serde_json::to_vec_pretty(&schema_with_deny_doc_tail(tail)).unwrap(),
            );
            validate_extracted_pack_files(&files).unwrap_or_else(|err| {
                panic!("a pack whose deny doc ends {tail:?} must validate: {err}")
            });
        }
    }

    #[test]
    fn test_a_structural_deny_change_is_still_a_stale_schema() {
        // A real schema change still fails closed, under either doc wording:
        // `deny` changes from an array of names to a single string.
        for tail in [DENY_DOC_0_9_1_TAIL, DENY_DOC_REWORDED_TAIL] {
            let mut bundled = schema_with_deny_doc_tail(tail);
            edit_deny_property_schemas(&mut bundled, &mut |deny| {
                deny.remove("items");
                deny.insert(
                    "type".to_string(),
                    serde_json::Value::String("string".to_string()),
                );
            });
            let mut files = adaptive_pack_files();
            files.insert(
                LAYER_DECISION_SCHEMA_PATH.to_string(),
                serde_json::to_vec(&bundled).unwrap(),
            );
            let err = validate_extracted_pack_files(&files).unwrap_err();
            assert!(
                matches!(
                    err,
                    PackValidationError::StaleAdaptiveSchema { ref path }
                        if path == "adaptive/layer-decision.schema.json"
                ),
                "a structural deny change must stay stale: {err}"
            );
        }
    }

    #[test]
    fn test_pack_rejects_invalid_utf8_skill_path_content() {
        let temp = fixture_mob_dir_with_skill_path("skills/review.md");
        std::fs::write(temp.path().join("skills").join("review.md"), [0xff, 0xfe]).unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillUtf8 { skill_name, path, .. }
                if skill_name == "review" && path == "skills/review.md"
        ));
    }

    #[test]
    fn test_inspect_rejects_semantically_invalid_archive() {
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[trust]\npolicy = \"strict\"\n"
                    .to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
        ]);
        let archive = create_targz(&files).unwrap();
        let err = inspect_archive_bytes(&archive).unwrap_err();
        assert!(matches!(err, PackValidationError::TrustSectionForbidden));
    }

    #[test]
    fn test_inspect_shows_manifest_and_digest() {
        let temp = fixture_mob_dir();
        let packed = pack_directory(temp.path(), None).unwrap();
        let inspect = inspect_archive_bytes(&packed.archive_bytes).unwrap();
        assert_eq!(inspect.name, "fixture");
        assert_eq!(inspect.version, "1.0.0");
        assert_eq!(inspect.digest, packed.digest);
        assert!(inspect.file_count >= 4);
    }

    fn fixture_mob_dir() -> TempDir {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("skills")).unwrap();
        std::fs::create_dir_all(temp.path().join("hooks")).unwrap();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("definition.json"), br#"{"id":"mob"}"#).unwrap();
        std::fs::write(
            temp.path().join("skills").join("review.md"),
            "# Review skill\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("hooks").join("run.sh"),
            "#!/bin/sh\necho run\n",
        )
        .unwrap();
        temp
    }

    fn fixture_mob_dir_with_skill_path(path: &str) -> TempDir {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("definition.json"),
            format!(
                r#"{{
  "id":"mob",
  "skills":{{"review":{{"source":"path","path":"{path}"}}}}
}}"#
            ),
        )
        .unwrap();
        temp
    }
}
