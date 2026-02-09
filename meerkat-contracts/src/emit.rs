//! Schema emission — generates JSON schema artifacts.
//!
//! Enabled via `--features schema`. The `emit-schemas` binary writes
//! schema files to `artifacts/schemas/`.

#[cfg(feature = "schema")]
pub fn emit_all_schemas(output_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    use schemars::schema_for;
    use std::fs;

    fs::create_dir_all(output_dir)?;

    // Version
    let version_schema = serde_json::json!({
        "contract_version": crate::version::ContractVersion::CURRENT.to_string(),
    });
    fs::write(
        output_dir.join("version.json"),
        serde_json::to_string_pretty(&version_schema)?,
    )?;

    // Wire types (contracts-owned types only — types embedding core types
    // without JsonSchema use serde for serialization but not for schema generation)
    let wire_types = serde_json::json!({
        "WireUsage": schema_for!(crate::wire::WireUsage),
        "ContractVersion": schema_for!(crate::version::ContractVersion),
    });
    fs::write(
        output_dir.join("wire-types.json"),
        serde_json::to_string_pretty(&wire_types)?,
    )?;

    // Params (only contracts-owned param types)
    let params = serde_json::json!({
        "CommsParams": schema_for!(crate::wire::CommsParams),
        "SkillsParams": schema_for!(crate::wire::SkillsParams),
    });
    fs::write(
        output_dir.join("params.json"),
        serde_json::to_string_pretty(&params)?,
    )?;

    // Errors
    let errors = serde_json::json!({
        "ErrorCode": schema_for!(crate::error::ErrorCode),
        "ErrorCategory": schema_for!(crate::error::ErrorCategory),
        "WireError": schema_for!(crate::error::WireError),
        "CapabilityHint": schema_for!(crate::error::CapabilityHint),
    });
    fs::write(
        output_dir.join("errors.json"),
        serde_json::to_string_pretty(&errors)?,
    )?;

    // Capabilities
    let capabilities = serde_json::json!({
        "CapabilityId": schema_for!(crate::capability::CapabilityId),
        "CapabilityScope": schema_for!(crate::capability::CapabilityScope),
        "CapabilityStatus": schema_for!(crate::capability::CapabilityStatus),
        "CapabilitiesResponse": schema_for!(crate::capability::CapabilitiesResponse),
    });
    fs::write(
        output_dir.join("capabilities.json"),
        serde_json::to_string_pretty(&capabilities)?,
    )?;

    Ok(())
}
