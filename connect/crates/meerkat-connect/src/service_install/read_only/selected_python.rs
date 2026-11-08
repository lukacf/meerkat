//! Join canonical selected software, its retained receipt and a trusted host
//! declaration. Package effects are consistency checks, never the trust root.
use super::*;
use crate::service_collection::{CollectionProgram, SelectedPackageIdentity};
use toolkit_package::{
    ActivationManifest, DeclaredEffect, Export, IntegrationPackage, LocalSoftwareDeclaration,
    LocalSoftwareExport, OperationDeclaration, PythonInstallPlan,
};

fn refused() -> crate::ConnectError {
    crate::service_probe::unavailable()
}

async fn bound_bytes(
    path: &std::path::Path,
    expected: &str,
) -> Result<Vec<u8>, crate::ConnectError> {
    let file = tokio::fs::File::open(path).await.map_err(|_| refused())?;
    if !file.metadata().await.map_err(|_| refused())?.is_file() {
        return Err(refused());
    }
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| refused())?;
    if bytes.len() > MAX_DOCUMENT || format!("{:x}", Sha256::digest(&bytes)) != expected {
        return Err(refused());
    }
    serde_json::from_slice::<crate::forge::UniqueJson>(&bytes).map_err(|_| refused())?;
    Ok(bytes)
}

fn plan(
    package: &IntegrationPackage,
    bytes: &[u8],
    document: &SelectedDocument,
) -> Result<(Value, Vec<OperationDeclaration>), crate::ConnectError> {
    let (plan, operations) = match document {
        SelectedDocument::Manifest { .. } => {
            let selected: ActivationManifest =
                serde_json::from_slice(bytes).map_err(|_| refused())?;
            let validated = selected.validate(package).map_err(|_| refused())?;
            let selected = validated.manifest();
            let component = &selected.components.first().ok_or_else(refused)?.id;
            let plan =
                PythonInstallPlan::new(package, &validated, component).map_err(|_| refused())?;
            let Some(Export::ServiceConnector(export)) = selected.exports.first() else {
                return Err(refused());
            };
            (plan, export.operations.clone())
        }
        SelectedDocument::Software { .. } => {
            let selected: LocalSoftwareDeclaration =
                serde_json::from_slice(bytes).map_err(|_| refused())?;
            let validated = selected.validate(package).map_err(|_| refused())?;
            let selected = validated.declaration();
            let component = &selected.components.first().ok_or_else(refused)?.id;
            let plan = PythonInstallPlan::from_declaration(package, &validated, component)
                .map_err(|_| refused())?;
            let Some(LocalSoftwareExport::ServiceConnector(export)) = selected.exports.first()
            else {
                return Err(refused());
            };
            (plan, export.operations.clone())
        }
    };
    Ok((
        serde_json::to_value(plan).map_err(|_| refused())?,
        operations,
    ))
}

pub(super) async fn selected(
    installed: &InstalledSelection,
    config: &McpServerConfig,
    operation: &str,
    inputs: Option<&SelectedServiceInputs>,
    program: &dyn CollectionProgram,
) -> Result<Option<InstalledReadOnly>, crate::ConnectError> {
    let inputs = inputs.ok_or_else(refused)?;
    let receipt_sha256 = installed.receipt_sha256.as_deref().ok_or_else(refused)?;
    let root = installed
        .mcp_path
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(refused)?;
    if installed.family != "selected-python"
        || installed.profile != "python"
        || config.connector.is_some()
        || installed.mcp_path != root.join("ready/mcp.json")
        || installed.receipt_path != root.join("ready/installed.json")
    {
        return Err(refused());
    }
    let receipt_bytes = bound_bytes(&installed.receipt_path, receipt_sha256).await?;
    let package_path = root.join("inputs/package.json");
    let document_path = root.join("inputs").join(inputs.document.filename());
    let package_bytes = bound_bytes(&package_path, &inputs.package_sha256).await?;
    let document_bytes = bound_bytes(&document_path, inputs.document.sha256()).await?;
    let package: IntegrationPackage =
        serde_json::from_slice(&package_bytes).map_err(|_| refused())?;
    let (plan, operations) = plan(&package, &document_bytes, &inputs.document)?;
    let receipt: Value = serde_json::from_slice(&receipt_bytes).map_err(|_| refused())?;
    let Some(identity) = identity(
        installed,
        config,
        operation,
        inputs,
        &package,
        &plan,
        &operations,
        &receipt,
    )?
    else {
        return Ok(None);
    };
    let declared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        program.declares_read_only(&identity, operation)
    }))
    .map_err(|_| refused())?;
    if !declared {
        return Ok(None);
    }
    // A trusted callback does not exempt retained inputs from current checks.
    // Never let a callback-time replacement acquire this declaration.
    bound_bytes(&installed.receipt_path, receipt_sha256).await?;
    bound_bytes(&package_path, &inputs.package_sha256).await?;
    bound_bytes(&document_path, inputs.document.sha256()).await?;
    Ok(Some(InstalledReadOnly {
        config: config.clone(),
        operation: operation.into(),
    }))
}

fn identity<'a>(
    installed: &'a InstalledSelection,
    config: &'a McpServerConfig,
    operation: &str,
    inputs: &'a SelectedServiceInputs,
    package: &'a IntegrationPackage,
    plan: &'a Value,
    operations: &[OperationDeclaration],
    receipt: &Value,
) -> Result<Option<SelectedPackageIdentity<'a>>, crate::ConnectError> {
    let destination = installed
        .mcp_path
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or_else(refused)?;
    let mut plan_identity = plan.clone();
    plan_identity["selection_sha256"] = json!(inputs.document.sha256());
    if installed.family != "selected-python"
        || installed.profile != "python"
        || config.connector.is_some()
        || receipt["schema_version"] != 1
        || receipt["scope"] != "observed_software_installation_only"
        || receipt["destination"].as_str() != destination.to_str()
        || SelectedDocument::from_value(receipt).map_err(|_| refused())? != inputs.document
        || SelectedDocument::from_value(&plan_identity).map_err(|_| refused())? != inputs.document
        || plan["mcp_name"] != installed.server_name
        || config.name != installed.server_name
    {
        return Err(refused());
    }
    let metadata = &plan["declared_service"];
    let slots = inputs
        .environment_secret_slots
        .as_ref()
        .ok_or_else(refused)?;
    if metadata["authentication"]["kind"] != "binding"
        || metadata["environment_secret_slots"]
            != serde_json::to_value(slots).map_err(|_| refused())?
    {
        return Err(refused());
    }
    let names = metadata["operation_tools"]
        .as_object()
        .ok_or_else(refused)?;
    let access = &config
        .access
        .as_ref()
        .ok_or_else(refused)?
        .allowed_operations;
    if !access.contains(operation)
        || access.iter().any(|name| {
            !names
                .values()
                .any(|raw| raw.as_str() == Some(name.as_str()))
        })
    {
        return Err(refused());
    }
    let mut selected = names
        .iter()
        .filter(|(_, name)| name.as_str() == Some(operation));
    let (id, _) = selected.next().ok_or_else(refused)?;
    if selected.next().is_some() {
        return Err(refused());
    }
    let declared = operations
        .iter()
        .find(|candidate| candidate.id.as_str() == id)
        .ok_or_else(refused)?;
    if declared.declared_effect != DeclaredEffect::Read {
        return Ok(None);
    }
    let references: Vec<_> = slots
        .names()
        .iter()
        .cloned()
        .map(ServiceCredentialSlot::SelectedEnvironment)
        .collect();
    let projected = secret_launch_configuration(
        installed,
        &receipt["mcp_configuration"],
        access,
        &references,
    )
    .map_err(|_| refused())?;
    if &projected != config {
        return Err(refused());
    }
    let McpTransportConfig::Stdio(launch) = &projected.transport else {
        return Err(refused());
    };
    let command: Vec<_> = std::iter::once(launch.command.clone())
        .chain(launch.args.iter().cloned())
        .collect();
    if receipt["command"] != json!(command) {
        return Err(refused());
    }
    let mut expected = serde_json::Map::new();
    expected.insert("package.json".into(), json!(inputs.package_sha256));
    expected.insert(
        inputs.document.filename().into(),
        json!(inputs.document.sha256()),
    );
    expected.insert("artifact.whl".into(), plan["artifact_digest"].clone());
    for (name, digest) in plan["closure"].as_object().ok_or_else(refused)? {
        expected.insert(format!("closure/{name}"), digest.clone());
    }
    if receipt["inputs"] != Value::Object(expected) {
        return Err(refused());
    }
    Ok(Some(SelectedPackageIdentity {
        package: &package.identity,
        package_sha256: &inputs.package_sha256,
        selection_document: inputs.document.filename(),
        selection_sha256: inputs.document.sha256(),
        // The setup effect is retained checked proposal identity. It is not a
        // field in the lower-level software installer's on-disk receipt.
        effect_sha256: &inputs.effect_sha256,
        receipt_sha256: installed.receipt_sha256.as_deref().ok_or_else(refused)?,
        component: plan["component"].as_str().ok_or_else(refused)?,
        service: plan["export"].as_str().ok_or_else(refused)?,
        artifact_sha256: plan["artifact_digest"].as_str().ok_or_else(refused)?,
        installed: config,
    }))
}

#[cfg(test)]
mod tests;
