//! Trusted local operator access to the existing local recipe persistence owner.
//! Scope comes from the operator, never from an untrusted remote tool request.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use toolkit_package::{
    ActivationManifest, ContentDigest, EvidenceReference, Id, IntegrationBundle, LocalRecipe,
    LocalRecipeError, LocalRecipeStore, LocalSelectionProfile, PackageIdentity,
    REMOTE_EXPORT_DECLARATION, RemoteExportDeclaration, RetainedGuidanceSelection,
    RetainedLocalRecipe, RetainedLocalSelection, RetainedLocalSoftware, Revision, ValidationError,
    canonical_json,
};
use toolkit_store::{SqliteStore, StoreScope};

const MAX_DOCUMENT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const HELP: &str =
    "toolkit-local-recipes init|save|list|list-page|read|export|check-export --database ABSOLUTE_PATH --realm REALM --owner OWNER
init: no additional flags; initialize or reopen the store without saving a recipe
save: --plugin ABSOLUTE_PATH --recipes ABSOLUTE_PATH [--mcp ABSOLUTE_PATH]
      [--decision-owner ID --decision-record ID --decision-revision REVISION]
      omitted decision flags retain the current submission as research provenance
  or: --package ABSOLUTE_PATH --selection ABSOLUTE_PATH --profile python|node|mcpb
  or: --package ABSOLUTE_PATH --declaration ABSOLUTE_PATH --profile python|node
      --component ID --verification ABSOLUTE_PATH plus the same decision flags
      MCPB additionally requires --configuration ABSOLUTE_PATH (nonsecret JSON)
      optionally --plugin-source ABSOLUTE_PATH (exact public plugin observation)
read: --publisher ID --name ID --version REVISION --digest LOWERCASE_SHA256
export: read flags plus exactly one of --selection ABSOLUTE_PATH, --declaration ABSOLUTE_PATH or --guidance ABSOLUTE_PATH,
        and --destination ABSOLUTE_NEW_DIRECTORY
check-export: the same selection/declaration/guidance flag plus --destination ABSOLUTE_PATH
list: no additional flags
list-page: --limit 1_TO_50 [--cursor OPAQUE_CURSOR_FROM_PREVIOUS_PAGE]

Retains host-sanitized recipe documents only. It does not install, authenticate,
activate, grant access, select a recipe or establish readiness. Scope flags are
trusted local operator input, not remote-agent authorization. See LOCAL-RECIPES.md.
Executable export emits one independent whole MCP endpoint, not an operation-filtered proxy.
Guidance export copies explicitly selected retained public skill text without MCP configuration or activation.
Check-export compares current files without writing them; it is not historical success or readiness.
List-page returns bounded historical summaries; follow next_cursor even after an empty page.
";

#[derive(Clone, Copy)]
enum Failure {
    Kind(&'static str),
    BundleValidation(&'static str),
}

impl Failure {
    // Only the initial save parser may attach a pre-store diagnostic. Stored
    // data validation uses Kind because persistence may already have occurred.
    fn bundle_validation(error: ValidationError) -> Self {
        Self::BundleValidation(match error.path.as_str() {
            "mcp.json:$schema" => "mcp_schema",
            "mcp.json" => "mcp_document",
            "plugin.json" => "plugin_document",
            "recipes.json" => "recipe_document",
            _ => "bundle_contract",
        })
    }
}

enum Action {
    Init,
    Save {
        plugin: Vec<u8>,
        mcp: Option<Vec<u8>>,
        recipes: Vec<u8>,
        decision: Option<EvidenceReference>,
    },
    SaveSelection(Box<RetainedLocalSelection>),
    SaveSoftware(Box<RetainedLocalSoftware>),
    List,
    ListPage {
        cursor: Option<String>,
        limit: u32,
    },
    Read {
        identity: PackageIdentity,
        digest: ContentDigest,
    },
    Export {
        identity: PackageIdentity,
        digest: ContentDigest,
        selection: ExportSelection,
        destination: PathBuf,
        inspect: bool,
    },
}

enum ExportSelection {
    Activation(Box<ActivationManifest>),
    Declaration(Box<RemoteExportDeclaration>),
    Guidance(RetainedGuidanceSelection),
}

struct Request {
    database: PathBuf,
    scope: StoreScope,
    action: Action,
}

fn take(values: &mut BTreeMap<String, String>, key: &str) -> Result<String, Failure> {
    values.remove(key).ok_or(Failure::Kind("invalid_arguments"))
}

fn absolute_path(value: String) -> Result<PathBuf, Failure> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(Failure::Kind("absolute_path_required"));
    }
    Ok(path)
}

fn document(path: PathBuf) -> Result<Vec<u8>, Failure> {
    // No final-component symlink or special file. Directory ownership remains
    // the host's responsibility; this is not a filesystem sandbox.
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    if !fs::symlink_metadata(&path)
        .map_err(|_| Failure::Kind("input_unavailable"))?
        .file_type()
        .is_file()
    {
        return Err(Failure::Kind("regular_file_required"));
    }
    let file = options
        .open(path)
        .map_err(|_| Failure::Kind("input_unavailable"))?;
    let metadata = file
        .metadata()
        .map_err(|_| Failure::Kind("input_unavailable"))?;
    if !metadata.is_file() {
        return Err(Failure::Kind("regular_file_required"));
    }
    if metadata.len() > MAX_DOCUMENT as u64 {
        return Err(Failure::Kind("input_too_large"));
    }
    let mut bytes = Vec::new();
    file.take((MAX_DOCUMENT + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure::Kind("input_unavailable"))?;
    if bytes.len() > MAX_DOCUMENT {
        return Err(Failure::Kind("input_too_large"));
    }
    Ok(bytes)
}

fn parse(args: Vec<String>) -> Result<Request, Failure> {
    let (verb, rest) = args
        .split_first()
        .ok_or(Failure::Kind("invalid_arguments"))?;
    if !matches!(
        verb.as_str(),
        "init" | "save" | "list" | "list-page" | "read" | "export" | "check-export"
    ) || rest.len() % 2 != 0
    {
        return Err(Failure::Kind("invalid_arguments"));
    }
    let common = ["--database", "--realm", "--owner"];
    let specific: &[&str] = match verb.as_str() {
        "save" => &[
            "--plugin",
            "--recipes",
            "--mcp",
            "--decision-owner",
            "--decision-record",
            "--decision-revision",
            "--package",
            "--selection",
            "--declaration",
            "--profile",
            "--component",
            "--verification",
            "--configuration",
            "--plugin-source",
        ],
        "read" => &["--publisher", "--name", "--version", "--digest"],
        "list-page" => &["--limit", "--cursor"],
        "export" | "check-export" => &[
            "--publisher",
            "--name",
            "--version",
            "--digest",
            "--selection",
            "--declaration",
            "--guidance",
            "--destination",
        ],
        _ => &[],
    };
    let mut values = BTreeMap::new();
    for pair in rest.as_chunks::<2>().0 {
        if !common.contains(&pair[0].as_str()) && !specific.contains(&pair[0].as_str()) {
            return Err(Failure::Kind("invalid_arguments"));
        }
        if values.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(Failure::Kind("invalid_arguments"));
        }
    }
    let database = absolute_path(take(&mut values, "--database")?)?;
    let scope = StoreScope {
        realm: take(&mut values, "--realm")?,
        owner: take(&mut values, "--owner")?,
    };
    scope
        .validate()
        .map_err(|_| Failure::Kind("invalid_scope"))?;
    let action = match verb.as_str() {
        "init" => Action::Init,
        "save" => {
            let decision = if ["--decision-owner", "--decision-record", "--decision-revision"]
                .iter().any(|key| values.contains_key(*key))
            {
                Some(EvidenceReference {
                    owner: Id::new(take(&mut values, "--decision-owner")?)
                        .map_err(|_| Failure::Kind("invalid_decision"))?,
                    record: Id::new(take(&mut values, "--decision-record")?)
                        .map_err(|_| Failure::Kind("invalid_decision"))?,
                    revision: Revision::new(take(&mut values, "--decision-revision")?)
                        .map_err(|_| Failure::Kind("invalid_decision"))?,
                })
            } else {
                None
            };
            if values.contains_key("--package") {
                let decision = decision.ok_or(Failure::Kind("invalid_arguments"))?;
                let package = document(absolute_path(take(&mut values, "--package")?)?)?;
                let declaration = values.contains_key("--declaration");
                if declaration && values.contains_key("--selection") {
                    return Err(Failure::Kind("invalid_arguments"));
                }
                let selection = document(absolute_path(take(&mut values,
                    if declaration { "--declaration" } else { "--selection" })?)?)?;
                let profile = match take(&mut values, "--profile")?.as_str() {
                    "python" => LocalSelectionProfile::Python,
                    "node" => LocalSelectionProfile::Node,
                    "mcpb" => LocalSelectionProfile::Mcpb,
                    _ => return Err(Failure::Kind("unsupported_local_profile")),
                };
                let component = Id::new(take(&mut values, "--component")?)
                    .map_err(|_| Failure::Kind("invalid_component"))?;
                let verification: Vec<EvidenceReference> = serde_json::from_slice(&document(
                    absolute_path(take(&mut values, "--verification")?)?,
                )?)
                .map_err(|_| Failure::Kind("invalid_verification"))?;
                let configuration = if profile == LocalSelectionProfile::Mcpb {
                    Some(document(absolute_path(take(
                        &mut values,
                        "--configuration",
                    )?)?)?)
                } else {
                    None
                };
                if declaration {
                    if configuration.is_some() { return Err(Failure::Kind("invalid_arguments")); }
                    let software = RetainedLocalSoftware::parse(&package, &selection, profile,
                        component, decision, verification)
                        .map_err(|_| Failure::Kind("invalid_local_selection"))?;
                    let software = match values.remove("--plugin-source") {
                        Some(path) => software.with_plugin_source(&document(absolute_path(path)?)?)
                            .map_err(|_| Failure::Kind("invalid_plugin_source"))?,
                        None => software,
                    };
                    Action::SaveSoftware(Box::new(software))
                } else {
                    let selected = if let Some(configuration) = &configuration {
                        RetainedLocalSelection::parse_mcpb(
                            &package,
                            &selection,
                            configuration,
                            component.clone(),
                            decision.clone(),
                            verification.clone(),
                        )
                    } else {
                        RetainedLocalSelection::parse(
                            &package,
                            &selection,
                            profile,
                            component.clone(),
                            decision.clone(),
                            verification.clone(),
                        )
                    }
                    .map_err(|_| Failure::Kind("invalid_local_selection"))?;
                    let selected = match values.remove("--plugin-source") {
                        Some(path) => selected
                            .with_plugin_source(&document(absolute_path(path)?)?)
                            .map_err(|_| Failure::Kind("invalid_plugin_source"))?,
                        None => selected,
                    };
                    Action::SaveSelection(Box::new(selected))
                }
            } else {
                let plugin_path = absolute_path(take(&mut values, "--plugin")?)?;
                let recipes_path = absolute_path(take(&mut values, "--recipes")?)?;
                let mcp_path = values.remove("--mcp").map(absolute_path).transpose()?;
                let plugin = document(plugin_path)?;
                let recipes = document(recipes_path)?;
                let mcp = mcp_path.map(document).transpose()?;
                // Validate before opening or creating a database. The persistence
                // owner independently validates again at its save boundary.
                IntegrationBundle::parse(&plugin, mcp.as_deref(), &recipes)
                    .map_err(Failure::bundle_validation)?;
                Action::Save {
                    plugin,
                    mcp,
                    recipes,
                    decision,
                }
            }
        }
        "list-page" => {
            let limit = take(&mut values, "--limit")?
                .parse::<u32>()
                .map_err(|_| Failure::Kind("invalid_page"))?;
            let cursor = values.remove("--cursor");
            LocalRecipeStore::validate_page_request(&scope, cursor.as_deref(), limit)
                .map_err(|_| Failure::Kind("invalid_page"))?;
            Action::ListPage { limit, cursor }
        }
        "read" | "export" | "check-export" => {
            let identity = PackageIdentity {
                publisher: Id::new(take(&mut values, "--publisher")?)
                    .map_err(|_| Failure::Kind("invalid_identity"))?,
                name: Id::new(take(&mut values, "--name")?)
                    .map_err(|_| Failure::Kind("invalid_identity"))?,
                version: Revision::new(take(&mut values, "--version")?)
                    .map_err(|_| Failure::Kind("invalid_identity"))?,
            };
            let digest = ContentDigest::parse(take(&mut values, "--digest")?)
                .map_err(|_| Failure::Kind("invalid_digest"))?;
            if verb == "read" {
                Action::Read { identity, digest }
            } else {
                let selection = match (
                    values.remove("--selection"),
                    values.remove("--declaration"),
                    values.remove("--guidance"),
                ) {
                    (Some(path), None, None) => ExportSelection::Activation(
                        serde_json::from_slice(&document(absolute_path(path)?)?)
                            .map_err(|_| Failure::Kind("invalid_selection"))?,
                    ),
                    (None, Some(path), None) => ExportSelection::Declaration(
                        serde_json::from_slice(&document(absolute_path(path)?)?)
                            .map_err(|_| Failure::Kind("invalid_selection"))?,
                    ),
                    (None, None, Some(path)) => ExportSelection::Guidance(
                        serde_json::from_value(
                            toolkit_package::strict_mcpb_json(&document(absolute_path(path)?)?)
                                .map_err(|_| Failure::Kind("invalid_selection"))?,
                        )
                        .map_err(|_| Failure::Kind("invalid_selection"))?,
                    ),
                    _ => return Err(Failure::Kind("invalid_arguments")),
                };
                Action::Export {
                    identity,
                    digest,
                    selection,
                    destination: absolute_path(take(&mut values, "--destination")?)?,
                    inspect: verb == "check-export",
                }
            }
        }
        _ => Action::List,
    };
    // Reject mixed save formats instead of silently ignoring the unused input.
    if !values.is_empty() {
        return Err(Failure::Kind("invalid_arguments"));
    }
    Ok(Request {
        database,
        scope,
        action,
    })
}

fn database_path(path: &Path, create: bool) -> Result<(), Failure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(Failure::Kind("database_regular_file_required")),
        Err(error) if error.kind() == io::ErrorKind::NotFound && create => {
            if path.parent().is_some_and(Path::is_dir) {
                Ok(())
            } else {
                Err(Failure::Kind("database_parent_unavailable"))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(Failure::Kind("database_not_found"))
        }
        Err(_) => Err(Failure::Kind("database_unavailable")),
    }
}

fn classify(error: &LocalRecipeError) -> &'static str {
    match error {
        LocalRecipeError::InvalidPage => "invalid_page",
        LocalRecipeError::Invalid(_) => "invalid_bundle",
        LocalRecipeError::StoreNotDurable => "durable_store_required",
        LocalRecipeError::ImmutableConflict => "immutable_conflict",
        LocalRecipeError::Unconfirmed(_) => "save_unconfirmed",
        LocalRecipeError::Unverified(_) | LocalRecipeError::IdentityMismatch { .. } => {
            "stored_recipe_unverified"
        }
        _ => "store_unavailable",
    }
}

fn summary(recipe: &LocalRecipe) -> Result<Value, Failure> {
    recipe
        .summary()
        .map_err(|_| Failure::Kind("stored_recipe_unverified"))
}

fn full(recipe: &LocalRecipe) -> Result<Value, Failure> {
    fn text(bytes: &[u8]) -> Result<&str, Failure> {
        std::str::from_utf8(bytes).map_err(|_| Failure::Kind("stored_recipe_unverified"))
    }
    let mut value = summary(recipe)?;
    value["documents"] = json!({
        "plugin_json": text(recipe.plugin_json())?,
        "mcp_json": recipe.mcp_json().map(text).transpose()?,
        "recipes_json": text(recipe.recipes_json())?,
    });
    Ok(value)
}

fn retained_summary(recipe: &RetainedLocalRecipe) -> Result<Value, Failure> {
    recipe
        .summary()
        .map_err(|_| Failure::Kind("stored_recipe_unverified"))
}

fn retained_full(recipe: &RetainedLocalRecipe) -> Result<Value, Failure> {
    match recipe {
        RetainedLocalRecipe::Recipe(recipe) => full(recipe),
        RetainedLocalRecipe::Software(recipe) => {
            let mut value = retained_summary(&RetainedLocalRecipe::Software(recipe.clone()))?;
            value["documents"] = json!({
                "package_json": std::str::from_utf8(recipe.package_json())
                    .map_err(|_| Failure::Kind("stored_recipe_unverified"))?,
                "declaration_json": std::str::from_utf8(recipe.declaration_json())
                    .map_err(|_| Failure::Kind("stored_recipe_unverified"))?,
            });
            if let Some(source) = recipe.plugin_source_json() {
                value["documents"]["plugin_source_json"] = json!(std::str::from_utf8(source)
                    .map_err(|_| Failure::Kind("stored_recipe_unverified"))?);
            }
            Ok(value)
        }
        RetainedLocalRecipe::Selection(recipe) => {
            let mut value = retained_summary(&RetainedLocalRecipe::Selection(recipe.clone()))?;
            value["documents"] = json!({
                "package_json": std::str::from_utf8(recipe.package_json())
                    .map_err(|_| Failure::Kind("stored_recipe_unverified"))?,
                "selection_json": std::str::from_utf8(recipe.selection_json())
                    .map_err(|_| Failure::Kind("stored_recipe_unverified"))?,
            });
            if let Some(source) = recipe.plugin_source_json() {
                value["documents"]["plugin_source_json"] = json!(
                    std::str::from_utf8(source)
                        .map_err(|_| Failure::Kind("stored_recipe_unverified"))?
                );
            }
            if let Some(configuration) = recipe.configuration_json() {
                value["documents"]["configuration_json"] = json!(
                    std::str::from_utf8(configuration)
                        .map_err(|_| Failure::Kind("stored_recipe_unverified"))?
                );
            }
            Ok(value)
        }
    }
}

async fn run(request: Request) -> Result<Value, Failure> {
    let create = matches!(
        request.action,
        Action::Init | Action::Save { .. } | Action::SaveSelection(..) | Action::SaveSoftware(..)
    );
    database_path(&request.database, create)?;
    let store = if create {
        SqliteStore::open(&request.database)
    } else {
        SqliteStore::open_existing(&request.database)
    }
    .map_err(|_| Failure::Kind("database_unavailable"))?;
    let recipes = LocalRecipeStore::new(&store, request.scope)
        .map_err(|error| Failure::Kind(classify(&error)))?;
    match request.action {
        Action::Init => Ok(json!({"status": "initialized"})),
        Action::Save {
            plugin,
            mcp,
            recipes: documents,
            decision,
        } => {
            let recipe = match decision {
                Some(reference) => recipes.save(&plugin, mcp.as_deref(), &documents, reference).await,
                None => recipes.save_current_submission(&plugin, mcp.as_deref(), &documents).await,
            }
            .map_err(|error| Failure::Kind(classify(&error)))?;
            Ok(json!({"status": "retained", "recipe": summary(&recipe)?}))
        }
        Action::SaveSelection(selected) => {
            let recipe = recipes
                .save_retained_selection(*selected)
                .await
                .map_err(|error| Failure::Kind(classify(&error)))?;
            Ok(json!({"status": "retained", "recipe": retained_summary(
                &RetainedLocalRecipe::Selection(recipe),
            )?}))
        }
        Action::SaveSoftware(selected) => {
            let recipe = recipes.save_retained_software(*selected).await
                .map_err(|error| Failure::Kind(classify(&error)))?;
            Ok(json!({"status": "retained", "recipe": retained_summary(
                &RetainedLocalRecipe::Software(recipe),
            )?}))
        }
        Action::Read { identity, digest } => {
            match recipes
                .load_retained_exact(&identity, &digest)
                .await
                .map_err(|error| Failure::Kind(classify(&error)))?
            {
                Some(recipe) => Ok(json!({"status": "found", "recipe": retained_full(&recipe)?})),
                None => Ok(json!({"status": "not_found"})),
            }
        }
        Action::List => {
            let listing = recipes
                .list_retained()
                .await
                .map_err(|error| Failure::Kind(classify(&error)))?;
            Ok(json!({
                "status": "listed",
                "recipes": listing.verified.iter().map(retained_summary).collect::<Result<Vec<_>, _>>()?,
                "unverified": listing.unverified.iter().map(|(_, error)| json!({
                    "error": classify(error),
                })).collect::<Vec<_>>(),
            }))
        }
        Action::ListPage { cursor, limit } => {
            let page = recipes
                .list_retained_page(cursor.as_deref(), limit)
                .await
                .map_err(|error| Failure::Kind(classify(&error)))?;
            Ok(page.wire_value())
        }
        Action::Export {
            identity,
            digest,
            selection,
            destination,
            inspect,
        } => {
            let (
                recipe_summary,
                selection_digest,
                selection_kind,
                source_digest,
                export,
                declaration,
            ) = match selection {
                ExportSelection::Guidance(selection) => {
                    let recipe = recipes
                        .load_retained_exact(&identity, &digest)
                        .await
                        .map_err(|error| Failure::Kind(classify(&error)))?
                        .ok_or(Failure::Kind("recipe_not_found"))?;
                    let pin = ContentDigest::of_bytes(
                        &canonical_json(&selection)
                            .map_err(|_| Failure::Kind("invalid_selection"))?,
                    );
                    let export = recipe
                        .export_guidance(&selection)
                        .map_err(|_| Failure::Kind("portable_export_refused"))?;
                    (
                        retained_summary(&recipe)?,
                        pin,
                        Some("guidance_only"),
                        Some(selection.plugin_source_digest),
                        export,
                        None,
                    )
                }
                selection => {
                    // Keep the existing executable path and refusal order.
                    // Local selection/software records do not become remote MCP exports.
                    let recipe = recipes
                        .load_exact(&identity, &digest)
                        .await
                        .map_err(|error| Failure::Kind(classify(&error)))?
                        .ok_or(Failure::Kind("recipe_not_found"))?;
                    let (pin, kind, exported) = match selection {
                        ExportSelection::Activation(selection) => {
                            let pin = ContentDigest::of_bytes(
                                &canonical_json(selection.as_ref())
                                    .map_err(|_| Failure::Kind("invalid_selection"))?,
                            );
                            (
                                pin,
                                None,
                                recipe
                                    .bundle()
                                    .export_portable_with_selection(&digest, *selection),
                            )
                        }
                        ExportSelection::Declaration(selection) => {
                            let pin = ContentDigest::of_bytes(
                                &canonical_json(selection.as_ref())
                                    .map_err(|_| Failure::Kind("invalid_selection"))?,
                            );
                            (
                                pin,
                                Some(REMOTE_EXPORT_DECLARATION),
                                recipe
                                    .bundle()
                                    .export_portable_declaration(&digest, *selection),
                            )
                        }
                        ExportSelection::Guidance(_) => unreachable!("guidance selected above"),
                    };
                    let (export, declaration) =
                        exported.map_err(|_| Failure::Kind("portable_export_refused"))?;
                    (summary(&recipe)?, pin, kind, None, export, declaration)
                }
            };
            if inspect {
                let mut result = json!({
                    "status": "inspected", "recipe": recipe_summary,
                    "selection_digest": selection_digest,
                    "destination": destination,
                    "export_check": export.inspect_existing(&destination),
                });
                if let Some(kind) = selection_kind {
                    result["selection_kind"] = json!(kind);
                }
                if let Some(digest) = source_digest {
                    result["plugin_source_digest"] = json!(digest);
                }
                if let Some(declaration) = declaration {
                    result["declared_remote_service"] = json!(declaration);
                }
                return Ok(result);
            }
            let files: BTreeMap<_, _> = export
                .file_fingerprints()
                .into_iter()
                .map(|(name, fingerprint)| {
                    (
                        name,
                        json!({"sha256": fingerprint.sha256, "bytes": fingerprint.bytes}),
                    )
                })
                .collect();
            // All selection checks precede the first write. An IO failure may
            // leave an incomplete directory; never report that as an export.
            export
                .write_new(&destination)
                .map_err(|_| Failure::Kind("export_write_unconfirmed"))?;
            let mut result = json!({
                "status": "exported", "recipe": recipe_summary,
                "selection_digest": selection_digest, "files": files,
                "destination": destination,
            });
            if let Some(kind) = selection_kind {
                result["selection_kind"] = json!(kind);
            }
            if let Some(digest) = source_digest {
                result["plugin_source_digest"] = json!(digest);
            }
            if let Some(declaration) = declaration {
                result["declared_remote_service"] = json!(declaration);
            }
            Ok(result)
        }
    }
}

fn output(result: Result<Value, Failure>) -> ExitCode {
    let (mut value, code) = match result {
        Ok(value) => (value, ExitCode::SUCCESS),
        Err(Failure::Kind(kind)) => (json!({"status": "error", "error": kind}), ExitCode::FAILURE),
        Err(Failure::BundleValidation(code)) => (
            json!({"status": "error", "error": "invalid_bundle",
                "diagnostic": {"phase": "before_store_open", "code": code}}),
            ExitCode::FAILURE,
        ),
    };
    value["schema_version"] = json!(1);
    value["scope"] = json!("retained_recipe_metadata");
    match serde_json::to_vec(&value) {
        Ok(bytes) if bytes.len() <= MAX_OUTPUT => {
            let mut stdout = io::stdout().lock();
            if stdout
                .write_all(&bytes)
                .and_then(|()| stdout.write_all(b"\n"))
                .is_err()
            {
                let _ = writeln!(io::stderr(), "output_unconfirmed");
                ExitCode::FAILURE
            } else {
                code
            }
        }
        _ => {
            let _ = writeln!(io::stderr(), "output_unavailable");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args = std::env::args_os()
        .skip(1)
        .map(|s| s.into_string())
        .collect::<Result<Vec<_>, _>>();
    if args.as_ref().is_ok_and(|args| args == &["--help"]) {
        return if io::stdout().write_all(HELP.as_bytes()).is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let request = match args
        .map_err(|_| Failure::Kind("invalid_arguments"))
        .and_then(parse)
    {
        Ok(request) => request,
        Err(error) => return output(Err(error)),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return output(Err(Failure::Kind("runtime_unavailable"))),
    };
    output(runtime.block_on(run(request)))
}
