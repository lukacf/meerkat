use std::fmt::Write;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use meerkat_machine_schema::{
    ClosurePolicy, CommsTrustAuthorityOperation, CompositionSchema, DurableMarkerFieldBinding,
    DurableMarkerProtocol, DurableMarkerRelationProtocol, EffectEmit, EffectHandoffProtocol, Expr,
    FeedbackFieldSource, HelperSchema, MachineSchema, ProtocolGenerationMode, RustTypeAtom,
    TransitionSchema, TriggerMatch, TypeRef, Update, VariantSchema, canonical_composition_schemas,
    canonical_machine_schemas, catalog::dsl, compat_composition_schemas,
};

use crate::public_contracts::repo_root;

/// Run protocol codegen: read all canonical compositions, generate Rust helpers
/// for each declared `EffectHandoffProtocol`, plus the terminal surface mapping.
pub fn run_protocol_codegen() -> Result<()> {
    let root = repo_root()?;
    let mut compositions = canonical_composition_schemas();
    compositions.extend(compat_composition_schemas());
    let machines = canonical_machine_schemas();
    let machine_by_name: std::collections::BTreeMap<&str, &MachineSchema> =
        machines.iter().map(|m| (m.machine.as_str(), m)).collect();

    let mut generated_count = 0;

    for composition in &compositions {
        if composition.handoff_protocols.is_empty() {
            continue;
        }

        for protocol in &composition.handoff_protocols {
            let producer_machine = composition
                .machines
                .iter()
                .find(|m| m.instance_id == protocol.producer_instance)
                .and_then(|inst| machine_by_name.get(inst.machine_name.as_str()).copied());

            let code = generate_protocol_helpers(
                protocol,
                producer_machine,
                composition,
                &machine_by_name,
            )?;
            let code = rustfmt_source(&code)?;
            let output_path = protocol_output_path(&root, protocol);

            if let Some(parent) = output_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create dir {}", parent.display()))?;
            }

            fs::write(&output_path, &code)
                .with_context(|| format!("write {}", output_path.display()))?;
            println!("  generated: {}", output_path.display());
            generated_count += 1;
        }
    }

    // Generate terminal surface mapping for MeerkatMachine turn outcomes.
    let turn_machine = machine_by_name.get("MeerkatMachine");
    if let Some(machine) = turn_machine {
        let code = generate_terminal_surface_mapping(machine)?;
        let code = rustfmt_source(&code)?;
        let output_path = root.join("meerkat-core/src/generated/terminal_surface_mapping.rs");
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        fs::write(&output_path, &code)
            .with_context(|| format!("write {}", output_path.display()))?;
        println!("  generated: {}", output_path.display());
        generated_count += 1;
    }

    let code = generate_comms_trust_authority_sources(&compositions)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/comms_trust_authority_sources.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let code = generate_auth_lease_transition_authority_sources(&compositions)?;
    let code = rustfmt_source(&code)?;
    let output_path =
        root.join("meerkat-core/src/generated/auth_lease_transition_authority_sources.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let code = generate_tool_visibility_owner_protocol()?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/protocol_tool_visibility_owner.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let code = generate_auth_lease_durable_lifecycle_marker_contract(&compositions)?;
    let code = rustfmt_source(&code)?;
    let output_path =
        root.join("meerkat-core/src/generated/auth_lease_durable_lifecycle_marker.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let session_persistence_version_machine =
        dsl::dsl_session_persistence_version_authority_machine_production_schema();
    let code =
        generate_session_persistence_version_authority(&session_persistence_version_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path =
        root.join("meerkat-core/src/generated/session_persistence_version_authority.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let pending_machine = dsl::dsl_pending_continuation_admission_machine_production_schema();
    let code = generate_pending_continuation_admission(&pending_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/pending_continuation_admission.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let approval_machine = dsl::dsl_approval_lifecycle_machine_production_schema();
    let code = generate_approval_lifecycle_authority(&approval_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/approval_lifecycle.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let session_document_machine = dsl::dsl_session_document_machine_production_schema();
    let code = generate_session_document_authority(&session_document_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/session_document.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let session_context_machine =
        dsl::dsl_session_system_context_authority_machine_production_schema();
    let code = generate_session_system_context_authority(&session_context_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/session_system_context_authority.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let session_durable_config_machine =
        dsl::dsl_session_durable_config_authority_machine_production_schema();
    let code = generate_session_durable_config_authority(&session_durable_config_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-core/src/generated/session_durable_config_authority.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    let session_realtime_machine =
        dsl::dsl_session_realtime_transcript_authority_machine_production_schema();
    let code = generate_session_realtime_transcript_authority(&session_realtime_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path =
        root.join("meerkat-core/src/generated/session_realtime_transcript_authority.rs");
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create dir {}", parent.display()))?;
    }
    fs::write(&output_path, &code).with_context(|| format!("write {}", output_path.display()))?;
    println!("  generated: {}", output_path.display());
    generated_count += 1;

    if generated_count == 0 {
        println!("protocol-codegen: no handoff protocols declared — nothing to generate");
    } else {
        println!("protocol-codegen: generated {generated_count} protocol helper(s)");
    }

    Ok(())
}

fn rustfmt_source(source: &str) -> Result<String> {
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut child = Command::new(rustfmt)
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn rustfmt for protocol codegen")?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .context("open rustfmt stdin for protocol codegen")?;
        stdin
            .write_all(source.as_bytes())
            .context("write generated source to rustfmt")?;
    }

    let output = child
        .wait_with_output()
        .context("wait for rustfmt during protocol codegen")?;
    if !output.status.success() {
        bail!(
            "rustfmt failed for generated protocol code: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    String::from_utf8(output.stdout).context("decode rustfmt output as utf-8")
}

fn protocol_output_path(root: &Path, protocol: &EffectHandoffProtocol) -> std::path::PathBuf {
    root.join(protocol.rust.module_path.as_str())
}

/// Public entry point used by the drift test. Renders the full helper
/// source for one protocol against its producer machine schema.
/// The internal `generate_protocol_helpers` path (which accepts
/// `Option<&MachineSchema>` for convenience in the codegen driver)
/// delegates to this.
pub fn render_protocol_helpers(
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
) -> Result<String> {
    generate_protocol_helpers_impl(protocol, producer_machine, composition, machine_by_name)
}

fn generate_protocol_helpers(
    protocol: &EffectHandoffProtocol,
    producer_machine: Option<&MachineSchema>,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
) -> Result<String> {
    let producer_machine = producer_machine.context("producer machine missing")?;
    generate_protocol_helpers_impl(protocol, producer_machine, composition, machine_by_name)
}

fn generate_protocol_helpers_impl(
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
) -> Result<String> {
    let mut out = String::new();

    writeln!(
        &mut out,
        "// @generated — protocol helpers for `{}`",
        protocol.name
    )?;
    writeln!(
        &mut out,
        "// Composition: {}, Producer: {}, Effect: {}",
        composition.name, protocol.producer_instance, protocol.effect_variant
    )?;
    writeln!(
        &mut out,
        "// Closure policy: {}",
        closure_policy_label(&protocol.closure_policy)
    )?;
    if let Some(liveness) = &protocol.liveness_annotation {
        writeln!(&mut out, "// Liveness: {liveness}")?;
    }
    if protocol.name.as_str() == "auth_lease_lifecycle_publication" {
        writeln!(
            &mut out,
            "#![allow(clippy::clone_on_copy, clippy::wrong_self_convention)]"
        )?;
    }
    writeln!(&mut out)?;

    for import in &protocol.rust.required_imports {
        writeln!(&mut out, "{import}")?;
    }
    if !protocol.rust.required_imports.is_empty() {
        writeln!(&mut out)?;
    }
    if uses_generated_authority_bridge(protocol) {
        emit_generated_authority_bridge_token(&mut out, protocol)?;
        emit_generated_authority_bridge_helpers(&mut out, protocol)?;
    }

    let obligation_type = generate_obligation_struct(&mut out, protocol, producer_machine)?;
    generate_feedback_input_pattern_macro(&mut out, protocol, composition, machine_by_name)?;

    // Primary mode first, then each declared additional mode. Stacking order
    // is deterministic: primary block leads, additional modes follow in the
    // order declared on the binding. The composition validator guarantees no
    // duplicates.
    emit_mode(
        &mut out,
        protocol,
        producer_machine,
        composition,
        machine_by_name,
        &obligation_type,
        &protocol.rust.generation_mode,
    )?;
    for extra in &protocol.rust.additional_modes {
        emit_mode(
            &mut out,
            protocol,
            producer_machine,
            composition,
            machine_by_name,
            &obligation_type,
            extra,
        )?;
    }

    Ok(out)
}

fn uses_generated_authority_bridge(protocol: &EffectHandoffProtocol) -> bool {
    protocol.name.as_str() == "auth_lease_lifecycle_publication"
        || protocol.comms_trust_authority.is_some()
}

fn generated_authority_bridge_owner(protocol: &EffectHandoffProtocol) -> &'static str {
    if protocol.name.as_str().starts_with("mob_") {
        "mob"
    } else {
        "runtime"
    }
}

fn emit_generated_authority_bridge_token(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
) -> Result<()> {
    let owner = generated_authority_bridge_owner(protocol);
    let protocol_name = protocol.name.as_str();
    writeln!(out, "struct GeneratedAuthorityBridgeToken;")?;
    writeln!(out)?;
    writeln!(
        out,
        "static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken = GeneratedAuthorityBridgeToken;"
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "pub(crate) fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {{"
    )?;
    writeln!(out, "    &GENERATED_AUTHORITY_BRIDGE_TOKEN")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "#[doc(hidden)]")?;
    writeln!(out, "#[allow(improper_ctypes_definitions, unsafe_code)]")?;
    writeln!(
        out,
        "#[unsafe(export_name = concat!(\"__meerkat_{owner}_generated_authority_bridge_token_is_valid_v1_{protocol_name}_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\")))]"
    )?;
    writeln!(
        out,
        "pub extern \"Rust\" fn generated_authority_bridge_token_is_valid(token: &(dyn std::any::Any + Send + Sync)) -> bool {{"
    )?;
    writeln!(out, "    token.is::<GeneratedAuthorityBridgeToken>()")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_generated_authority_bridge_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
) -> Result<()> {
    if protocol.name.as_str() != "comms_trust_reconcile" {
        return Ok(());
    }
    writeln!(out, "pub(crate) fn generated_peer_comms_install_factory(")?;
    writeln!(
        out,
        "    handle: std::sync::Arc<dyn meerkat_core::handles::PeerCommsHandle>,"
    )?;
    writeln!(
        out,
        "    owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,"
    )?;
    writeln!(
        out,
        ") -> Result<meerkat_core::handles::GeneratedPeerCommsInstallFactory, String> {{"
    )?;
    writeln!(
        out,
        "    meerkat_core::handles::GeneratedPeerCommsInstallFactory::__from_runtime_generated_authority("
    )?;
    writeln!(out, "        generated_authority_bridge_token(),")?;
    writeln!(out, "        handle,")?;
    writeln!(out, "        owner_token,")?;
    writeln!(out, "    )")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "pub(crate) fn generated_peer_comms_install(")?;
    writeln!(
        out,
        "    handle: std::sync::Arc<dyn meerkat_core::handles::PeerCommsHandle>,"
    )?;
    writeln!(
        out,
        "    owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,"
    )?;
    writeln!(out, "    target_peer_id: meerkat_core::comms::PeerId,")?;
    writeln!(
        out,
        ") -> Result<meerkat_core::handles::GeneratedPeerCommsInstall, String> {{"
    )?;
    writeln!(
        out,
        "    meerkat_core::handles::GeneratedPeerCommsInstall::__from_runtime_generated_authority("
    )?;
    writeln!(out, "        generated_authority_bridge_token(),")?;
    writeln!(out, "        handle,")?;
    writeln!(out, "        owner_token,")?;
    writeln!(out, "        target_peer_id,")?;
    writeln!(out, "    )")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn generate_feedback_input_pattern_macro(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
) -> Result<()> {
    if protocol.allowed_feedback_inputs.is_empty() {
        return Ok(());
    }

    let Some(input_enum_path) = protocol.rust.input_enum_path.as_deref() else {
        return Ok(());
    };
    let input_enum_path = macro_crate_path(input_enum_path);
    let macro_name = format!(
        "{}_feedback_input_patterns",
        to_snake_case(protocol.name.as_str())
    );

    writeln!(out, "#[macro_export]")?;
    writeln!(out, "macro_rules! {macro_name} {{")?;
    writeln!(out, "    () => {{")?;
    for (index, feedback) in protocol.allowed_feedback_inputs.iter().enumerate() {
        let target_machine = machine_for_instance(
            composition,
            machine_by_name,
            feedback.machine_instance.as_str(),
        )?;
        let target_variant = target_machine
            .inputs
            .variant_named(feedback.input_variant.as_str())?;
        let separator = if index == 0 { "" } else { "| " };
        let fields = if target_variant.fields.is_empty() {
            ""
        } else {
            " { .. }"
        };
        writeln!(
            out,
            "        {separator}{input_enum_path}::{}{fields}",
            feedback.input_variant.as_str()
        )?;
    }
    writeln!(out, "    }};")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn macro_crate_path(path: &str) -> String {
    let path = path.replace("::dsl::", "::");
    path.strip_prefix("crate::")
        .map(|rest| format!("$crate::{rest}"))
        .unwrap_or(path)
}

fn emit_mode(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
    obligation_type: &str,
    mode: &ProtocolGenerationMode,
) -> Result<()> {
    match mode {
        ProtocolGenerationMode::Executor => {
            generate_executor_helpers(out, protocol, producer_machine, obligation_type)
        }
        ProtocolGenerationMode::EffectExtractor => generate_effect_extractor_helpers(
            out,
            protocol,
            producer_machine,
            composition,
            machine_by_name,
            obligation_type,
        ),
        ProtocolGenerationMode::ShellBridge => generate_shell_bridge_helpers(
            out,
            protocol,
            composition,
            machine_by_name,
            obligation_type,
        ),
        ProtocolGenerationMode::HandleBridge => {
            generate_handle_bridge_helpers(out, protocol, obligation_type)
        }
    }
}

fn generate_obligation_struct(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
) -> Result<String> {
    let obligation_type = format!("{}Obligation", to_pascal_case(protocol.name.as_str()));
    let protected = protected_obligation_protocol(protocol.name.as_str());
    let trust_authority = protocol.comms_trust_authority.as_ref();
    let producer_effect = producer_machine
        .effects
        .variant_named(protocol.effect_variant.as_str())
        .context("producer effect variant missing")?;

    if protocol.name.as_str() == "comms_trust_reconcile" {
        emit_peer_projection_freshness_authority(out)?;
    }
    if is_supervisor_trust_protocol(protocol.name.as_str()) {
        emit_supervisor_trust_freshness_authority(out)?;
    }
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        emit_mob_topology_freshness_authority(
            out,
            protocol.name.as_str() == "mob_member_trust_wiring",
        )?;
    }

    writeln!(out, "#[derive(Debug, Clone)]")?;
    writeln!(out, "pub struct {obligation_type} {{")?;
    if protocol.obligation_fields.is_empty() {
        writeln!(out, "    _private: (),")?;
    } else {
        for field in &protocol.obligation_fields {
            let effect_field = producer_effect
                .field_named(field.as_str())
                .with_context(|| {
                    format!("obligation field `{field}` missing from producer effect")
                })?;
            let visibility = if protected { "" } else { "pub " };
            writeln!(
                out,
                "    {visibility}{}: {},",
                to_snake_case(field.as_str()),
                rust_type(&effect_field.ty)
            )?;
        }
    }
    if trust_authority.is_some() {
        writeln!(
            out,
            "    comms_trust_authority_claims: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,"
        )?;
    }
    if protocol.name.as_str() == "auth_lease_lifecycle_publication" {
        writeln!(
            out,
            "    transition_claimed: std::sync::Arc<std::sync::atomic::AtomicBool>,"
        )?;
    }
    if protocol.name.as_str() == "comms_trust_reconcile" {
        writeln!(
            out,
            "    peer_projection_freshness_authority: PeerProjectionFreshnessAuthority,"
        )?;
    }
    if is_supervisor_trust_protocol(protocol.name.as_str()) {
        writeln!(
            out,
            "    supervisor_trust_freshness_authority: SupervisorTrustFreshnessAuthority,"
        )?;
    }
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        writeln!(
            out,
            "    mob_topology_freshness_authority: MobTopologyFreshnessAuthority,"
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;

    if protected && !protocol.obligation_fields.is_empty() {
        writeln!(out, "impl {obligation_type} {{")?;
        for field in &protocol.obligation_fields {
            let rust_name = to_snake_case(field.as_str());
            let effect_field = producer_effect
                .field_named(field.as_str())
                .with_context(|| {
                    format!("obligation field `{field}` missing from producer effect")
                })?;
            let ty = rust_type(&effect_field.ty);
            if getter_returns_copy(&effect_field.ty) {
                writeln!(out, "    pub fn {rust_name}(&self) -> {ty} {{")?;
                writeln!(out, "        self.{rust_name}")?;
            } else {
                writeln!(out, "    pub fn {rust_name}(&self) -> &{ty} {{")?;
                writeln!(out, "        &self.{rust_name}")?;
            }
            writeln!(out, "    }}")?;
            writeln!(out)?;
        }
        writeln!(out, "}}")?;
        writeln!(out)?;
    }
    if protocol.name.as_str() == "auth_lease_lifecycle_publication" {
        emit_auth_lease_transition_authority_helper(out, &obligation_type)?;
    }
    emit_comms_trust_authority_source_impl(out, protocol, &obligation_type)?;

    Ok(obligation_type)
}

fn emit_auth_lease_transition_authority_helper(
    out: &mut String,
    obligation_type: &str,
) -> Result<()> {
    writeln!(
        out,
        "pub(crate) struct AuthLeaseLifecyclePublicationScope {{"
    )?;
    writeln!(out, "    lease_key: meerkat_core::handles::LeaseKey,")?;
    writeln!(out, "    new_state: AuthLifecyclePhase,")?;
    writeln!(out, "    expires_at: Option<u64>,")?;
    writeln!(out, "    credential_generation: u64,")?;
    writeln!(out, "    credential_published_at_millis: Option<u64>,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl AuthLeaseLifecyclePublicationScope {{")?;
    writeln!(out, "    pub(crate) fn from_authority(")?;
    writeln!(out, "        lease_key: meerkat_core::handles::LeaseKey,")?;
    writeln!(
        out,
        "        authority: &crate::auth_machine::dsl::AuthMachineAuthority,"
    )?;
    writeln!(out, "    ) -> Self {{")?;
    writeln!(out, "        let state = authority.state();")?;
    writeln!(out, "        Self {{")?;
    writeln!(out, "            lease_key,")?;
    writeln!(out, "            new_state: state.lifecycle_phase,")?;
    writeln!(out, "            expires_at: state.expires_at,")?;
    writeln!(
        out,
        "            credential_generation: state.credential_generation,"
    )?;
    writeln!(
        out,
        "            credential_published_at_millis: state.credential_published_at_millis,"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn validate_obligation(&self, obligation: &{obligation_type}) -> Result<(), String> {{"
    )?;
    writeln!(out, "        if self.new_state != obligation.new_state {{")?;
    writeln!(
        out,
        "            return Err(format!(\"generated auth lease lifecycle publication state {{:?}} does not match authority state {{:?}}\", obligation.new_state, self.new_state));"
    )?;
    writeln!(out, "        }}")?;
    writeln!(
        out,
        "        if self.expires_at != obligation.expires_at {{"
    )?;
    writeln!(
        out,
        "            return Err(format!(\"generated auth lease lifecycle publication expires_at {{:?}} does not match authority expires_at {{:?}}\", obligation.expires_at, self.expires_at));"
    )?;
    writeln!(out, "        }}")?;
    writeln!(
        out,
        "        if self.credential_generation != obligation.credential_generation {{"
    )?;
    writeln!(
        out,
        "            return Err(format!(\"generated auth lease lifecycle publication generation {{}} does not match authority generation {{}}\", obligation.credential_generation, self.credential_generation));"
    )?;
    writeln!(out, "        }}")?;
    writeln!(
        out,
        "        if self.credential_published_at_millis != obligation.credential_published_at_millis {{"
    )?;
    writeln!(
        out,
        "            return Err(format!(\"generated auth lease lifecycle publication credential publication time {{:?}} does not match authority publication time {{:?}}\", obligation.credential_published_at_millis, self.credential_published_at_millis));"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        Ok(())")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl {obligation_type} {{")?;
    writeln!(out, "    #[allow(unsafe_code)]")?;
    writeln!(out, "    pub(crate) fn into_auth_lease_transition(")?;
    writeln!(out, "        &self,")?;
    writeln!(out, "        scope: AuthLeaseLifecyclePublicationScope,")?;
    writeln!(
        out,
        "    ) -> Result<meerkat_core::handles::AuthLeaseTransition, String> {{"
    )?;
    writeln!(
        out,
        "        if self.transition_claimed.swap(true, std::sync::atomic::Ordering::SeqCst) {{"
    )?;
    writeln!(
        out,
        "            return Err(\"generated auth lease lifecycle publication was already consumed\".into());"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        scope.validate_obligation(self)?;")?;
    writeln!(out, "        let phase = match self.new_state {{")?;
    writeln!(
        out,
        "            AuthLifecyclePhase::Valid => meerkat_core::handles::AuthLeasePhase::Valid,"
    )?;
    writeln!(
        out,
        "            AuthLifecyclePhase::Expiring => meerkat_core::handles::AuthLeasePhase::Expiring,"
    )?;
    writeln!(
        out,
        "            AuthLifecyclePhase::Expired => meerkat_core::handles::AuthLeasePhase::Expired,"
    )?;
    writeln!(
        out,
        "            AuthLifecyclePhase::Refreshing => meerkat_core::handles::AuthLeasePhase::Refreshing,"
    )?;
    writeln!(
        out,
        "            AuthLifecyclePhase::ReauthRequired => meerkat_core::handles::AuthLeasePhase::ReauthRequired,"
    )?;
    writeln!(
        out,
        "            AuthLifecyclePhase::Released => meerkat_core::handles::AuthLeasePhase::Released,"
    )?;
    writeln!(out, "        }};")?;
    writeln!(
        out,
        "        #[allow(improper_ctypes_definitions, unsafe_code)]"
    )?;
    writeln!(out, "        unsafe extern \"Rust\" {{")?;
    writeln!(
        out,
        "            #[link_name = concat!(\"__meerkat_core_runtime_generated_auth_lease_transition_build_v1_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\"))]"
    )?;
    writeln!(
        out,
        "            fn core_runtime_generated_auth_lease_transition_build("
    )?;
    writeln!(
        out,
        "                token: &'static (dyn std::any::Any + Send + Sync),"
    )?;
    writeln!(
        out,
        "                lease_key: meerkat_core::handles::LeaseKey,"
    )?;
    writeln!(
        out,
        "                phase: meerkat_core::handles::AuthLeasePhase,"
    )?;
    writeln!(out, "                expires_at: u64,")?;
    writeln!(out, "                generation: u64,")?;
    writeln!(
        out,
        "                credential_published_at_millis: Option<u64>,"
    )?;
    writeln!(
        out,
        "            ) -> Result<meerkat_core::handles::AuthLeaseTransition, String>;"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        #[allow(unsafe_code)]")?;
    writeln!(out, "        unsafe {{")?;
    writeln!(
        out,
        "            core_runtime_generated_auth_lease_transition_build("
    )?;
    writeln!(out, "                generated_authority_bridge_token(),")?;
    writeln!(out, "                scope.lease_key,")?;
    writeln!(out, "                phase,")?;
    writeln!(out, "                self.expires_at.unwrap_or(u64::MAX),")?;
    writeln!(out, "                self.credential_generation,")?;
    writeln!(out, "                self.credential_published_at_millis,")?;
    writeln!(out, "            )")?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "#[allow(unsafe_code)]")?;
    writeln!(out, "pub fn generated_auth_lease_handle(")?;
    writeln!(
        out,
        "    handle: std::sync::Arc<crate::handles::RuntimeAuthLeaseHandle>,"
    )?;
    writeln!(
        out,
        ") -> Result<meerkat_core::handles::GeneratedAuthLeaseHandle, String> {{"
    )?;
    writeln!(
        out,
        "    #[allow(improper_ctypes_definitions, unsafe_code)]"
    )?;
    writeln!(out, "    unsafe extern \"Rust\" {{")?;
    writeln!(
        out,
        "        #[link_name = concat!(\"__meerkat_core_runtime_generated_auth_lease_handle_build_v1_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\"))]"
    )?;
    writeln!(
        out,
        "        fn core_runtime_generated_auth_lease_handle_build("
    )?;
    writeln!(
        out,
        "            token: &'static (dyn std::any::Any + Send + Sync),"
    )?;
    writeln!(
        out,
        "            handle: std::sync::Arc<dyn meerkat_core::handles::AuthLeaseHandle>,"
    )?;
    writeln!(
        out,
        "        ) -> Result<meerkat_core::handles::GeneratedAuthLeaseHandle, String>;"
    )?;
    writeln!(out, "    }}")?;
    writeln!(
        out,
        "    let handle: std::sync::Arc<dyn meerkat_core::handles::AuthLeaseHandle> = handle;"
    )?;
    writeln!(out, "    #[allow(unsafe_code)]")?;
    writeln!(
        out,
        "    unsafe {{ core_runtime_generated_auth_lease_handle_build(generated_authority_bridge_token(), handle) }}"
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_peer_projection_freshness_authority(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Clone)]")?;
    writeln!(out, "pub struct PeerProjectionFreshnessAuthority {{")?;
    writeln!(
        out,
        "    authority: Option<std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>>,"
    )?;
    writeln!(
        out,
        "    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,"
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(
        out,
        "impl std::fmt::Debug for PeerProjectionFreshnessAuthority {{"
    )?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{"
    )?;
    writeln!(
        out,
        "        f.debug_struct(\"PeerProjectionFreshnessAuthority\").field(\"present\", &self.authority.is_some()).field(\"owner_present\", &self.source_owner_token.is_some()).finish()"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl PeerProjectionFreshnessAuthority {{")?;
    writeln!(
        out,
        "    pub fn from_authority(authority: std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>) -> Self {{"
    )?;
    writeln!(
        out,
        "        let source_owner_token = authority.lock().unwrap_or_else(std::sync::PoisonError::into_inner).generated_authority_owner_token();"
    )?;
    writeln!(
        out,
        "        Self {{ authority: Some(authority), source_owner_token: Some(source_owner_token) }}"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    fn missing() -> Self {{")?;
    writeln!(
        out,
        "        Self {{ authority: None, source_owner_token: None }}"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {{"
    )?;
    writeln!(
        out,
        "        self.source_owner_token.as_ref().map(std::sync::Arc::clone)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn validate_peer_projection_epoch(&self, expected_epoch: u64) -> Result<(), String> {{"
    )?;
    writeln!(out, "        let Some(authority) = &self.authority else {{")?;
    writeln!(
        out,
        "            return Err(\"generated peer projection freshness authority is absent\".to_string());"
    )?;
    writeln!(out, "        }};")?;
    writeln!(
        out,
        "        let guard = authority.lock().map_err(|_| \"generated peer projection freshness authority was poisoned\".to_string())?;"
    )?;
    writeln!(
        out,
        "        let current_epoch = guard.state().peer_projection_epoch;"
    )?;
    writeln!(out, "        if current_epoch == expected_epoch {{")?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(format!(\"stale generated peer projection trust obligation at epoch {{expected_epoch}} (current {{current_epoch}})\"))"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_mob_topology_freshness_authority(
    out: &mut String,
    include_prepared_topology_epoch: bool,
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone)]")?;
    writeln!(out, "pub struct MobTopologyFreshnessAuthority {{")?;
    writeln!(
        out,
        "    topology_epoch: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,"
    )?;
    writeln!(
        out,
        "    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,"
    )?;
    if include_prepared_topology_epoch {
        writeln!(
            out,
            "    prepared_batch: Option<PreparedBatchTopologyFreshness>,"
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    if include_prepared_topology_epoch {
        writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
        writeln!(out, "struct PreparedBatchBaseTopology {{")?;
        writeln!(out, "    lifecycle_phase: MobPhase,")?;
        writeln!(out, "    topology_epoch: u64,")?;
        writeln!(
            out,
            "    wiring_edges: std::collections::BTreeSet<WiringEdge>,"
        )?;
        writeln!(
            out,
            "    member_peer_ids: std::collections::BTreeMap<AgentIdentity, PeerId>,"
        )?;
        writeln!(
            out,
            "    member_peer_endpoints: std::collections::BTreeMap<AgentIdentity, MemberPeerEndpoint>,"
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(out, "impl PreparedBatchBaseTopology {{")?;
        writeln!(
            out,
            "    fn from_authority(authority: &MobMachineAuthority) -> Self {{"
        )?;
        writeln!(out, "        let state = authority.state();")?;
        writeln!(out, "        Self {{")?;
        writeln!(out, "            lifecycle_phase: state.lifecycle_phase,")?;
        writeln!(out, "            topology_epoch: state.topology_epoch,")?;
        writeln!(out, "            wiring_edges: state.wiring_edges.clone(),")?;
        writeln!(
            out,
            "            member_peer_ids: state.member_peer_ids.clone(),"
        )?;
        writeln!(
            out,
            "            member_peer_endpoints: state.member_peer_endpoints.clone(),"
        )?;
        writeln!(out, "        }}")?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
        writeln!(
            out,
            "    fn validate_live_authority(&self, authority: &MobMachineAuthority) -> Result<(), String> {{"
        )?;
        writeln!(out, "        let state = authority.state();")?;
        writeln!(out, "        let live_epoch = state.topology_epoch;")?;
        writeln!(
            out,
            "        if state.lifecycle_phase != self.lifecycle_phase || live_epoch != self.topology_epoch {{"
        )?;
        writeln!(
            out,
            "            return Err(format!(\"stale generated MobMachine prepared trust batch at base epoch {{}} (current {{live_epoch}})\", self.topology_epoch));"
        )?;
        writeln!(out, "        }}")?;
        writeln!(
            out,
            "        if state.wiring_edges != self.wiring_edges || state.member_peer_ids != self.member_peer_ids || state.member_peer_endpoints != self.member_peer_endpoints {{"
        )?;
        writeln!(
            out,
            "            return Err(format!(\"stale generated MobMachine prepared trust batch base topology changed at epoch {{}}\", self.topology_epoch));"
        )?;
        writeln!(out, "        }}")?;
        writeln!(out, "        Ok(())")?;
        writeln!(out, "    }}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(out, "#[derive(Debug, Clone)]")?;
        writeln!(out, "struct PreparedBatchTopologyFreshness {{")?;
        writeln!(out, "    base: PreparedBatchBaseTopology,")?;
        writeln!(
            out,
            "    obligations: std::collections::BTreeSet<PreparedBatchMemberTrustObligation>,"
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(
            out,
            "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]"
        )?;
        writeln!(out, "struct PreparedBatchMemberTrustObligation {{")?;
        writeln!(out, "    edge: WiringEdge,")?;
        writeln!(out, "    a_peer_id: PeerId,")?;
        writeln!(out, "    b_peer_id: PeerId,")?;
        writeln!(out, "    a_endpoint: MemberPeerEndpoint,")?;
        writeln!(out, "    b_endpoint: MemberPeerEndpoint,")?;
        writeln!(out, "    epoch: u64,")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(out, "impl PreparedBatchMemberTrustObligation {{")?;
        writeln!(
            out,
            "    fn from_effect(effect: &MobMachineEffect) -> Option<Self> {{"
        )?;
        writeln!(out, "        match effect {{")?;
        writeln!(
            out,
            "            MobMachineEffect::MemberTrustWiringRequested {{ edge, a_peer_id, b_peer_id, a_endpoint, b_endpoint, epoch }} => Some(Self {{"
        )?;
        writeln!(out, "                edge: edge.clone(),")?;
        writeln!(out, "                a_peer_id: a_peer_id.clone(),")?;
        writeln!(out, "                b_peer_id: b_peer_id.clone(),")?;
        writeln!(out, "                a_endpoint: a_endpoint.clone(),")?;
        writeln!(out, "                b_endpoint: b_endpoint.clone(),")?;
        writeln!(out, "                epoch: *epoch,")?;
        writeln!(out, "            }}),")?;
        writeln!(out, "            _ => None,")?;
        writeln!(out, "        }}")?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
        writeln!(
            out,
            "    fn from_obligation(obligation: &MobMemberTrustWiringObligation) -> Self {{"
        )?;
        writeln!(out, "        Self {{")?;
        writeln!(out, "            edge: obligation.edge.clone(),")?;
        writeln!(out, "            a_peer_id: obligation.a_peer_id.clone(),")?;
        writeln!(out, "            b_peer_id: obligation.b_peer_id.clone(),")?;
        writeln!(
            out,
            "            a_endpoint: obligation.a_endpoint.clone(),"
        )?;
        writeln!(
            out,
            "            b_endpoint: obligation.b_endpoint.clone(),"
        )?;
        writeln!(out, "            epoch: obligation.epoch,")?;
        writeln!(out, "        }}")?;
        writeln!(out, "    }}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(
            out,
            "fn validate_live_member_trust_obligation(authority: &MobMachineAuthority, obligation: &MobMemberTrustWiringObligation) -> Result<(), String> {{"
        )?;
        writeln!(out, "    let state = authority.state();")?;
        writeln!(out, "    if state.lifecycle_phase != MobPhase::Running {{")?;
        writeln!(
            out,
            "        return Err(\"generated MobMachine member trust authority requires live Running phase\".to_string());"
        )?;
        writeln!(out, "    }}")?;
        writeln!(out, "    if state.topology_epoch != obligation.epoch {{")?;
        writeln!(
            out,
            "        return Err(format!(\"stale generated MobMachine trust obligation at epoch {{}} (current {{}})\", obligation.epoch, state.topology_epoch));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(
            out,
            "    if !state.wiring_edges.contains(&obligation.edge) {{"
        )?;
        writeln!(
            out,
            "        return Err(format!(\"generated MobMachine member trust edge {{:?}} is not live\", obligation.edge));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(
            out,
            "    if state.member_peer_ids.get(&obligation.edge.a) != Some(&obligation.a_peer_id) {{"
        )?;
        writeln!(
            out,
            "        return Err(format!(\"generated MobMachine member trust peer id for {{:?}} is stale\", obligation.edge.a));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(
            out,
            "    if state.member_peer_ids.get(&obligation.edge.b) != Some(&obligation.b_peer_id) {{"
        )?;
        writeln!(
            out,
            "        return Err(format!(\"generated MobMachine member trust peer id for {{:?}} is stale\", obligation.edge.b));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(
            out,
            "    if state.member_peer_endpoints.get(&obligation.edge.a) != Some(&obligation.a_endpoint) {{"
        )?;
        writeln!(
            out,
            "        return Err(format!(\"generated MobMachine member trust endpoint for {{:?}} is stale\", obligation.edge.a));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(
            out,
            "    if state.member_peer_endpoints.get(&obligation.edge.b) != Some(&obligation.b_endpoint) {{"
        )?;
        writeln!(
            out,
            "        return Err(format!(\"generated MobMachine member trust endpoint for {{:?}} is stale\", obligation.edge.b));"
        )?;
        writeln!(out, "    }}")?;
        writeln!(out, "    Ok(())")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(out, "#[derive(Debug, Clone)]")?;
        writeln!(
            out,
            "pub(crate) struct MobTopologyPreparedBatchAuthority {{"
        )?;
        writeln!(
            out,
            "    source_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,"
        )?;
        writeln!(out, "    prepared_base: PreparedBatchBaseTopology,")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(out, "impl MobTopologyPreparedBatchAuthority {{")?;
        writeln!(
            out,
            "    pub(crate) fn from_live_authority(authority: &MobMachineAuthority) -> Self {{"
        )?;
        writeln!(
            out,
            "        let prepared_base = PreparedBatchBaseTopology::from_authority(authority);"
        )?;
        writeln!(
            out,
            "        let source_owner_token = authority.generated_authority_owner_token();"
        )?;
        writeln!(out, "        Self {{ source_owner_token, prepared_base }}")?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
        writeln!(
            out,
            "    pub(crate) fn freshness_for_prepared_transitions<'a>(&self, live_authority: &MobMachineAuthority, transitions: impl IntoIterator<Item = &'a MobMachineTransition>) -> Result<MobTopologyFreshnessAuthority, String> {{"
        )?;
        writeln!(
            out,
            "        let live_owner_token = live_authority.generated_authority_owner_token();"
        )?;
        writeln!(
            out,
            "        if !std::sync::Arc::ptr_eq(&self.source_owner_token, &live_owner_token) {{"
        )?;
        writeln!(
            out,
            "            return Err(\"generated MobMachine prepared trust batch was minted by a different live authority owner\".to_string());"
        )?;
        writeln!(out, "        }}")?;
        writeln!(
            out,
            "        self.prepared_base.validate_live_authority(live_authority)?;"
        )?;
        writeln!(
            out,
            "        let mut obligation_epochs = std::collections::BTreeSet::new();"
        )?;
        writeln!(
            out,
            "        let mut obligations = std::collections::BTreeSet::new();"
        )?;
        writeln!(out, "        for transition in transitions {{")?;
        writeln!(out, "            for effect in transition.effects() {{")?;
        writeln!(
            out,
            "                if let Some(obligation) = PreparedBatchMemberTrustObligation::from_effect(effect) {{"
        )?;
        writeln!(out, "                    let epoch = obligation.epoch;")?;
        writeln!(
            out,
            "                    if !obligation_epochs.insert(epoch) {{"
        )?;
        writeln!(
            out,
            "                        return Err(format!(\"prepared MobMachine member trust batch carried duplicate obligation epoch {{epoch}}\"));"
        )?;
        writeln!(out, "                    }}")?;
        writeln!(out, "                    obligations.insert(obligation);")?;
        writeln!(out, "                }}")?;
        writeln!(out, "            }}")?;
        writeln!(out, "        }}")?;
        writeln!(out, "        if obligations.is_empty() {{")?;
        writeln!(
            out,
            "            return Err(\"prepared MobMachine batch did not carry member trust wiring obligations\".to_string());"
        )?;
        writeln!(out, "        }}")?;
        writeln!(out, "        Ok(MobTopologyFreshnessAuthority {{")?;
        writeln!(out, "            topology_epoch: None,")?;
        writeln!(
            out,
            "            source_owner_token: Some(std::sync::Arc::clone(&self.source_owner_token)),"
        )?;
        writeln!(
            out,
            "            prepared_batch: Some(PreparedBatchTopologyFreshness {{ base: self.prepared_base.clone(), obligations }}),"
        )?;
        writeln!(out, "        }})")?;
        writeln!(out, "    }}")?;
        writeln!(out, "}}")?;
        writeln!(out)?;
    }
    writeln!(out, "impl MobTopologyFreshnessAuthority {{")?;
    writeln!(
        out,
        "    pub(crate) fn from_live_topology_epoch(topology_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>, source_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>) -> Self {{"
    )?;
    writeln!(
        out,
        "{}",
        if include_prepared_topology_epoch {
            "        Self { topology_epoch: Some(topology_epoch), source_owner_token: Some(source_owner_token), prepared_batch: None }"
        } else {
            "        Self { topology_epoch: Some(topology_epoch), source_owner_token: Some(source_owner_token) }"
        }
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    if include_prepared_topology_epoch {
        writeln!(
            out,
            "    pub(crate) fn from_live_member_trust_authority(authority: &MobMachineAuthority) -> Self {{"
        )?;
        writeln!(out, "        let _ = authority.state();")?;
        writeln!(
            out,
            "        let source_owner_token = authority.generated_authority_owner_token();"
        )?;
        writeln!(
            out,
            "        Self {{ topology_epoch: None, source_owner_token: Some(source_owner_token), prepared_batch: None }}"
        )?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
    }
    writeln!(out, "    fn missing() -> Self {{")?;
    writeln!(
        out,
        "{}",
        if include_prepared_topology_epoch {
            "        Self { topology_epoch: None, source_owner_token: None, prepared_batch: None }"
        } else {
            "        Self { topology_epoch: None, source_owner_token: None }"
        }
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {{"
    )?;
    writeln!(
        out,
        "        self.source_owner_token.as_ref().map(std::sync::Arc::clone)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn validate_topology_epoch(&self, expected_epoch: u64, allow_next_epoch: bool) -> Result<(), String> {{"
    )?;
    if include_prepared_topology_epoch {
        writeln!(out, "        if self.prepared_batch.is_some() {{")?;
        writeln!(
            out,
            "            return Err(\"generated MobMachine prepared trust freshness requires exact obligation validation\".to_string());"
        )?;
        writeln!(out, "        }}")?;
    }
    writeln!(
        out,
        "        let Some(topology_epoch) = &self.topology_epoch else {{"
    )?;
    writeln!(
        out,
        "            return Err(\"generated MobMachine topology freshness authority is absent\".to_string());"
    )?;
    writeln!(out, "        }};")?;
    writeln!(
        out,
        "        let current_epoch = topology_epoch.load(std::sync::atomic::Ordering::Acquire);"
    )?;
    if include_prepared_topology_epoch {
        writeln!(out, "        debug_assert!(self.prepared_batch.is_none());")?;
    }
    writeln!(
        out,
        "        let matches_current = current_epoch == expected_epoch;"
    )?;
    writeln!(
        out,
        "        let matches_next = allow_next_epoch && current_epoch.checked_add(1) == Some(expected_epoch);"
    )?;
    writeln!(out, "        if matches_current || matches_next {{")?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(format!(\"stale generated MobMachine trust obligation at epoch {{expected_epoch}} (current {{current_epoch}})\"))"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    if include_prepared_topology_epoch {
        writeln!(out)?;
        writeln!(
            out,
            "    fn validate_live_authority_owner(&self, live_authority: &MobMachineAuthority) -> Result<(), String> {{"
        )?;
        writeln!(
            out,
            "        let Some(source_owner_token) = &self.source_owner_token else {{"
        )?;
        writeln!(
            out,
            "            return Err(\"generated MobMachine member trust freshness owner authority is absent\".to_string());"
        )?;
        writeln!(out, "        }};")?;
        writeln!(
            out,
            "        let live_owner_token = live_authority.generated_authority_owner_token();"
        )?;
        writeln!(
            out,
            "        if std::sync::Arc::ptr_eq(source_owner_token, &live_owner_token) {{"
        )?;
        writeln!(out, "            Ok(())")?;
        writeln!(out, "        }} else {{")?;
        writeln!(
            out,
            "            Err(\"generated MobMachine member trust freshness was minted by a different live authority owner\".to_string())"
        )?;
        writeln!(out, "        }}")?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
        writeln!(
            out,
            "    fn validate_member_trust_obligation(&self, obligation: &MobMemberTrustWiringObligation, allow_next_epoch: bool, prepared_live_authority: Option<&MobMachineAuthority>) -> Result<(), String> {{"
        )?;
        writeln!(
            out,
            "        if let Some(prepared_batch) = &self.prepared_batch {{"
        )?;
        writeln!(
            out,
            "            let Some(prepared_live_authority) = prepared_live_authority else {{"
        )?;
        writeln!(
            out,
            "                return Err(\"generated MobMachine prepared trust freshness requires live authority validation\".to_string());"
        )?;
        writeln!(out, "            }};")?;
        writeln!(
            out,
            "            self.validate_live_authority_owner(prepared_live_authority)?;"
        )?;
        writeln!(
            out,
            "            prepared_batch.base.validate_live_authority(prepared_live_authority)?;"
        )?;
        writeln!(
            out,
            "            let expected = PreparedBatchMemberTrustObligation::from_obligation(obligation);"
        )?;
        writeln!(
            out,
            "            if prepared_batch.obligations.contains(&expected) {{"
        )?;
        writeln!(out, "                return Ok(());")?;
        writeln!(out, "            }}")?;
        writeln!(
            out,
            "            return Err(format!(\"generated MobMachine prepared trust batch does not contain exact obligation for edge {{:?}} at epoch {{}} (base {{}})\", obligation.edge, obligation.epoch, prepared_batch.base.topology_epoch));"
        )?;
        writeln!(out, "        }}")?;
        writeln!(
            out,
            "        let Some(prepared_live_authority) = prepared_live_authority else {{"
        )?;
        writeln!(
            out,
            "            return Err(\"generated MobMachine member trust freshness requires live authority validation\".to_string());"
        )?;
        writeln!(out, "        }};")?;
        writeln!(
            out,
            "        self.validate_live_authority_owner(prepared_live_authority)?;"
        )?;
        writeln!(out, "        let _ = allow_next_epoch;")?;
        writeln!(
            out,
            "        validate_live_member_trust_obligation(prepared_live_authority, obligation)"
        )?;
        writeln!(out, "    }}")?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_supervisor_trust_freshness_authority(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Clone)]")?;
    writeln!(out, "pub struct SupervisorTrustFreshnessAuthority {{")?;
    writeln!(
        out,
        "    authority: Option<std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>>,"
    )?;
    writeln!(
        out,
        "    source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,"
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(
        out,
        "impl std::fmt::Debug for SupervisorTrustFreshnessAuthority {{"
    )?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{"
    )?;
    writeln!(
        out,
        "        f.debug_struct(\"SupervisorTrustFreshnessAuthority\").field(\"present\", &self.authority.is_some()).field(\"owner_present\", &self.source_owner_token.is_some()).finish()"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "#[allow(dead_code)]")?;
    writeln!(out, "impl SupervisorTrustFreshnessAuthority {{")?;
    writeln!(
        out,
        "    pub fn from_authority(authority: std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>) -> Self {{"
    )?;
    writeln!(
        out,
        "        let source_owner_token = authority.lock().unwrap_or_else(std::sync::PoisonError::into_inner).generated_authority_owner_token();"
    )?;
    writeln!(
        out,
        "        Self {{ authority: Some(authority), source_owner_token: Some(source_owner_token) }}"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    fn missing() -> Self {{")?;
    writeln!(
        out,
        "        Self {{ authority: None, source_owner_token: None }}"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn source_owner_token(&self) -> Option<std::sync::Arc<dyn std::any::Any + Send + Sync>> {{"
    )?;
    writeln!(
        out,
        "        self.source_owner_token.as_ref().map(std::sync::Arc::clone)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    fn validate_pending_publish(")?;
    writeln!(out, "        &self,")?;
    writeln!(out, "        expected_peer_id: &str,")?;
    writeln!(out, "        expected_name: &str,")?;
    writeln!(out, "        expected_address: &str,")?;
    writeln!(out, "        expected_signing_public_key: Option<&str>,")?;
    writeln!(out, "        expected_epoch: u64,")?;
    writeln!(
        out,
        "        expected_local_endpoint: &Option<PeerEndpoint>,"
    )?;
    writeln!(out, "    ) -> Result<(), String> {{")?;
    writeln!(out, "        let Some(authority) = &self.authority else {{")?;
    writeln!(
        out,
        "            return Err(\"generated supervisor trust freshness authority is absent\".to_string());"
    )?;
    writeln!(out, "        }};")?;
    writeln!(
        out,
        "        let guard = authority.lock().map_err(|_| \"generated supervisor trust freshness authority was poisoned\".to_string())?;"
    )?;
    writeln!(out, "        let state = guard.state();")?;
    writeln!(
        out,
        "        if state.supervisor_publish_pending_peer_id.as_deref() == Some(expected_peer_id)"
    )?;
    writeln!(
        out,
        "            && state.supervisor_publish_pending_name.as_deref() == Some(expected_name)"
    )?;
    writeln!(
        out,
        "            && state.supervisor_publish_pending_address.as_deref() == Some(expected_address)"
    )?;
    writeln!(
        out,
        "            && state.supervisor_publish_pending_signing_public_key.as_deref() == expected_signing_public_key"
    )?;
    writeln!(
        out,
        "            && state.supervisor_publish_pending_epoch == Some(expected_epoch)"
    )?;
    writeln!(
        out,
        "            && state.local_endpoint.as_ref() == expected_local_endpoint.as_ref()"
    )?;
    writeln!(out, "        {{")?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(format!(\"stale generated supervisor trust publish obligation for peer {{expected_peer_id:?}} at epoch {{expected_epoch}}\"))"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn validate_pending_revoke(&self, expected_peer_id: &str, expected_epoch: u64, expected_local_endpoint: &Option<PeerEndpoint>) -> Result<(), String> {{"
    )?;
    writeln!(out, "        let Some(authority) = &self.authority else {{")?;
    writeln!(
        out,
        "            return Err(\"generated supervisor trust freshness authority is absent\".to_string());"
    )?;
    writeln!(out, "        }};")?;
    writeln!(
        out,
        "        let guard = authority.lock().map_err(|_| \"generated supervisor trust freshness authority was poisoned\".to_string())?;"
    )?;
    writeln!(out, "        let state = guard.state();")?;
    writeln!(
        out,
        "        if state.supervisor_revoke_pending_peer_id.as_deref() == Some(expected_peer_id) && state.supervisor_revoke_pending_epoch == Some(expected_epoch) && state.local_endpoint.as_ref() == expected_local_endpoint.as_ref() {{"
    )?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(format!(\"stale generated supervisor trust revoke obligation for peer {{expected_peer_id:?}} at epoch {{expected_epoch}}\"))"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_comms_trust_authority_source_impl(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
) -> Result<()> {
    let Some(trust_authority) = protocol.comms_trust_authority.as_ref() else {
        return Ok(());
    };
    if trust_authority.allowed_operations.iter().any(|operation| {
        matches!(
            operation,
            CommsTrustAuthorityOperation::PublicAdd | CommsTrustAuthorityOperation::PrivateAdd
        )
    }) {
        emit_comms_trust_descriptor_helper(out, protocol, obligation_type)?;
    }

    writeln!(out, "impl {obligation_type} {{")?;
    writeln!(out, "    #[allow(unsafe_code)]")?;
    writeln!(out, "    fn authorize_comms_trust_authority(")?;
    writeln!(out, "        &self,")?;
    writeln!(
        out,
        "        operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,"
    )?;
    writeln!(out, "        peer_id: &str,")?;
    writeln!(
        out,
        "        peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,"
    )?;
    if protocol.name.as_str() == "mob_member_trust_wiring" {
        writeln!(
            out,
            "        prepared_live_authority: Option<&MobMachineAuthority>,"
        )?;
    }
    writeln!(
        out,
        "    ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
    )?;
    emit_comms_trust_allowed_operation_check(out, protocol)?;
    if protocol.name.as_str() == "comms_trust_reconcile" {
        writeln!(
            out,
            "        self.peer_projection_freshness_authority.validate_peer_projection_epoch(self.peer_projection_epoch)?;"
        )?;
    }
    if is_supervisor_trust_protocol(protocol.name.as_str()) {
        if protocol.name.as_str() == "supervisor_trust_publish" {
            writeln!(
                out,
                "        self.supervisor_trust_freshness_authority.validate_pending_publish("
            )?;
            writeln!(out, "            self.peer_id.as_str(),")?;
            writeln!(out, "            self.name.as_str(),")?;
            writeln!(out, "            self.address.as_str(),")?;
            writeln!(out, "            self.signing_public_key.as_deref(),")?;
            writeln!(out, "            self.epoch,")?;
            writeln!(out, "            &self.local_endpoint,")?;
            writeln!(out, "        )?;")?;
        } else {
            writeln!(
                out,
                "        self.supervisor_trust_freshness_authority.validate_pending_revoke(self.peer_id.as_str(), self.epoch, &self.local_endpoint)?;"
            )?;
        }
    }
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        let allow_next_epoch = if protocol.effect_variant.as_str() == "MemberTrustUnwiringRequested"
        {
            "true"
        } else {
            "false"
        };
        if protocol.name.as_str() == "mob_member_trust_wiring" {
            writeln!(
                out,
                "        self.mob_topology_freshness_authority.validate_member_trust_obligation(self, {allow_next_epoch}, prepared_live_authority)?;"
            )?;
        } else {
            writeln!(
                out,
                "        self.mob_topology_freshness_authority.validate_topology_epoch(self.epoch, {allow_next_epoch})?;"
            )?;
        }
    }
    emit_comms_trust_payload_authorization(out, protocol)?;
    writeln!(
        out,
        "        let claim_key = format!(\"{{operation:?}}:{{peer_id}}\");"
    )?;
    writeln!(
        out,
        "        let mut claims = self.comms_trust_authority_claims.lock().map_err(|_| \"generated comms trust authority source claims were poisoned\".to_string())?;"
    )?;
    writeln!(out, "        if !claims.insert(claim_key) {{")?;
    writeln!(
        out,
        "            return Err(format!(\"generated comms trust authority source already minted {{operation:?}} for peer {{peer_id:?}}\"));"
    )?;
    writeln!(out, "        }}")?;
    emit_comms_trust_bridge(out, protocol)?;
    emit_comms_trust_grant_return(out, protocol)?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn emit_comms_trust_bridge(out: &mut String, protocol: &EffectHandoffProtocol) -> Result<()> {
    let bridge_owner = if protocol.name.as_str().starts_with("mob_") {
        "mob"
    } else {
        "runtime"
    };
    writeln!(
        out,
        "        #[allow(improper_ctypes_definitions, unsafe_code)]"
    )?;
    writeln!(out, "        unsafe extern \"Rust\" {{")?;
    writeln!(
        out,
        "            #[link_name = concat!(\"__meerkat_core_{bridge_owner}_generated_comms_trust_authority_build_v1_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\"))]"
    )?;
    writeln!(
        out,
        "            fn core_generated_comms_trust_authority_build("
    )?;
    writeln!(
        out,
        "                token: &'static (dyn std::any::Any + Send + Sync),"
    )?;
    writeln!(
        out,
        "                source_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,"
    )?;
    writeln!(out, "                source_epoch: u64,")?;
    writeln!(
        out,
        "                source_owner_token: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,"
    )?;
    writeln!(
        out,
        "                trust_row_owner_kind: meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind,"
    )?;
    writeln!(
        out,
        "                operation: meerkat_core::comms::GeneratedCommsTrustAuthorityOperation,"
    )?;
    writeln!(out, "                peer_id: String,")?;
    writeln!(out, "                trust_store_peer_id: Option<String>,")?;
    writeln!(
        out,
        "                peer_descriptor: Option<meerkat_core::comms::TrustedPeerDescriptor>,"
    )?;
    writeln!(
        out,
        "            ) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String>;"
    )?;
    writeln!(out, "        }}")?;
    Ok(())
}

fn emit_comms_trust_grant_return(out: &mut String, protocol: &EffectHandoffProtocol) -> Result<()> {
    let trust_authority = protocol
        .comms_trust_authority
        .as_ref()
        .context("comms trust authority metadata missing")?;
    writeln!(out, "        match operation {{")?;
    for operation in &trust_authority.allowed_operations {
        let operation_variant = operation.core_variant();
        let source_kind = trust_authority.source_kind.core_variant();
        let row_owner_kind = trust_authority
            .row_owner_kind
            .unwrap_or(trust_authority.source_kind)
            .core_variant();
        let target_peer_expr = comms_trust_target_peer_expr(protocol)?;
        let Some(target_peer_expr) = target_peer_expr else {
            bail!(
                "comms trust authority protocol `{}` must declare a trust-store peer target",
                protocol.name
            );
        };
        match operation {
            CommsTrustAuthorityOperation::PublicAdd | CommsTrustAuthorityOperation::PrivateAdd => {
                writeln!(out, "            Operation::{operation_variant} => {{")?;
                writeln!(
                    out,
                    "                let peer_descriptor = peer_descriptor.ok_or_else(|| format!(\"generated comms trust add for peer {{peer_id:?}} requires a trusted peer descriptor\"))?;"
                )?;
                writeln!(
                    out,
                    "                let expected_descriptor = trusted_peer_descriptor_for_request(self, peer_id)?;"
                )?;
                writeln!(
                    out,
                    "                if expected_descriptor != peer_descriptor {{"
                )?;
                writeln!(
                    out,
                    "                    return Err(format!(\"generated comms trust descriptor for peer {{peer_id:?}} does not match requested mutation descriptor\"));"
                )?;
                writeln!(out, "                }}")?;
                writeln!(
                    out,
                    "                let trust_store_peer_id = {target_peer_expr}.to_string();"
                )?;
                writeln!(
                    out,
                    "                let generated_peer_id = peer_descriptor.peer_id.to_string();"
                )?;
                let source_epoch_expr = comms_trust_epoch_expr(protocol)?;
                emit_comms_trust_bridge_call(
                    out,
                    CommsTrustBridgeCallArgs {
                        source_kind,
                        row_owner_kind,
                        operation_variant,
                        source_epoch_expr,
                        peer_id_expr: "generated_peer_id",
                        trust_store_peer_id_expr: "Some(trust_store_peer_id)",
                        peer_descriptor_expr: "Some(peer_descriptor)",
                    },
                )?;
                writeln!(out, "            }}")?;
            }
            CommsTrustAuthorityOperation::PublicRemove
            | CommsTrustAuthorityOperation::PrivateRemove => {
                writeln!(out, "            Operation::{operation_variant} => {{")?;
                writeln!(out, "                if peer_descriptor.is_some() {{")?;
                writeln!(
                    out,
                    "                    return Err(format!(\"generated comms trust remove for peer {{peer_id:?}} must not carry a trusted peer descriptor\"));"
                )?;
                writeln!(out, "                }}")?;
                writeln!(
                    out,
                    "                let trust_store_peer_id = {target_peer_expr}.to_string();"
                )?;
                let source_epoch_expr = comms_trust_epoch_expr(protocol)?;
                emit_comms_trust_bridge_call(
                    out,
                    CommsTrustBridgeCallArgs {
                        source_kind,
                        row_owner_kind,
                        operation_variant,
                        source_epoch_expr,
                        peer_id_expr: "peer_id.to_string()",
                        trust_store_peer_id_expr: "Some(trust_store_peer_id)",
                        peer_descriptor_expr: "None",
                    },
                )?;
                writeln!(out, "            }}")?;
            }
        }
    }
    writeln!(
        out,
        "            _ => unreachable!(\"operation checked above\"),"
    )?;
    writeln!(out, "        }}")?;
    Ok(())
}

struct CommsTrustBridgeCallArgs<'a> {
    source_kind: &'a str,
    row_owner_kind: &'a str,
    operation_variant: &'a str,
    source_epoch_expr: &'a str,
    peer_id_expr: &'a str,
    trust_store_peer_id_expr: &'a str,
    peer_descriptor_expr: &'a str,
}

fn emit_comms_trust_bridge_call(
    out: &mut String,
    args: CommsTrustBridgeCallArgs<'_>,
) -> Result<()> {
    let CommsTrustBridgeCallArgs {
        source_kind,
        row_owner_kind,
        operation_variant,
        source_epoch_expr,
        peer_id_expr,
        trust_store_peer_id_expr,
        peer_descriptor_expr,
    } = args;
    writeln!(out, "                #[allow(unsafe_code)]")?;
    writeln!(out, "                unsafe {{")?;
    writeln!(
        out,
        "                    core_generated_comms_trust_authority_build("
    )?;
    writeln!(
        out,
        "                        generated_authority_bridge_token(),"
    )?;
    writeln!(
        out,
        "                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::{source_kind},"
    )?;
    writeln!(out, "                        {source_epoch_expr},")?;
    let owner_token_expr = if source_kind == "MeerkatMachinePeerProjection" {
        "self.peer_projection_freshness_authority.source_owner_token()"
    } else if source_kind.starts_with("MeerkatMachineSupervisor") {
        "self.supervisor_trust_freshness_authority.source_owner_token()"
    } else if source_kind.starts_with("MobMachine") {
        "self.mob_topology_freshness_authority.source_owner_token()"
    } else {
        "None"
    };
    writeln!(out, "                        {owner_token_expr},")?;
    writeln!(
        out,
        "                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::{row_owner_kind},"
    )?;
    writeln!(
        out,
        "                        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::{operation_variant},"
    )?;
    writeln!(out, "                        {peer_id_expr},")?;
    writeln!(out, "                        {trust_store_peer_id_expr},")?;
    writeln!(out, "                        {peer_descriptor_expr},")?;
    writeln!(out, "                    )")?;
    writeln!(out, "                }}")?;
    Ok(())
}

fn emit_comms_trust_descriptor_helper(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
) -> Result<()> {
    match protocol.name.as_str() {
        "comms_trust_reconcile" => {
            emit_endpoint_descriptor_converter(
                out,
                "trusted_peer_descriptor_from_peer_endpoint",
                "crate::meerkat_machine::dsl::PeerEndpoint",
            )?;
            writeln!(
                out,
                "fn trusted_peer_descriptor_for_request(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
            )?;
            writeln!(
                out,
                "    let mut matches = obligation.direct_peer_endpoints.iter().chain(obligation.mob_overlay_peer_endpoints.iter()).filter(|endpoint| endpoint.peer_id.0 == peer_id);"
            )?;
            writeln!(out, "    let Some(endpoint) = matches.next() else {{")?;
            writeln!(
                out,
                "        return Err(format!(\"MeerkatMachine peer projection did not request trust for peer {{peer_id:?}}\"));"
            )?;
            writeln!(out, "    }};")?;
            writeln!(out, "    if matches.next().is_some() {{")?;
            writeln!(
                out,
                "        return Err(format!(\"MeerkatMachine peer projection has ambiguous endpoint descriptors for peer {{peer_id:?}}\"));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(
                out,
                "    trusted_peer_descriptor_from_peer_endpoint(endpoint)"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "supervisor_trust_publish" => {
            writeln!(
                out,
                "fn trusted_peer_descriptor_for_request(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
            )?;
            writeln!(out, "    if obligation.peer_id != peer_id {{")?;
            writeln!(
                out,
                "        return Err(format!(\"MeerkatMachine supervisor trust obligation peer_id {{:?}} does not match requested peer {{peer_id:?}}\", obligation.peer_id));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(
                out,
                "    let signing_public_key = obligation.signing_public_key.as_ref().ok_or_else(|| \"generated supervisor trust publish obligation omitted signing public key\".to_string())?;"
            )?;
            writeln!(
                out,
                "    let pubkey = crate::comms_drain::decode_supervisor_signing_public_key(signing_public_key)?;"
            )?;
            writeln!(
                out,
                "    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(obligation.name.clone(), obligation.peer_id.clone(), pubkey, obligation.address.clone())"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "mob_member_trust_wiring" => {
            emit_endpoint_descriptor_converter(
                out,
                "trusted_peer_descriptor_from_member_endpoint",
                "crate::machines::mob_machine::MemberPeerEndpoint",
            )?;
            writeln!(
                out,
                "fn trusted_peer_descriptor_for_request(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
            )?;
            writeln!(
                out,
                "    let a_matches = obligation.a_endpoint.peer_id.0 == peer_id;"
            )?;
            writeln!(
                out,
                "    let b_matches = obligation.b_endpoint.peer_id.0 == peer_id;"
            )?;
            writeln!(out, "    match (a_matches, b_matches) {{")?;
            writeln!(
                out,
                "        (true, false) => trusted_peer_descriptor_from_member_endpoint(&obligation.a_endpoint),"
            )?;
            writeln!(
                out,
                "        (false, true) => trusted_peer_descriptor_from_member_endpoint(&obligation.b_endpoint),"
            )?;
            writeln!(
                out,
                "        (false, false) => Err(format!(\"MobMachine member trust obligation does not carry requested peer {{peer_id:?}}\")),"
            )?;
            writeln!(
                out,
                "        (true, true) => Err(format!(\"MobMachine member trust obligation has ambiguous endpoint descriptors for peer {{peer_id:?}}\")),"
            )?;
            writeln!(out, "    }}")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "mob_external_peer_trust_wiring" | "mob_external_peer_trust_repair" => {
            emit_endpoint_descriptor_converter(
                out,
                "trusted_peer_descriptor_from_external_endpoint",
                "crate::machines::mob_machine::ExternalPeerEndpoint",
            )?;
            writeln!(
                out,
                "fn trusted_peer_descriptor_for_request(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
            )?;
            writeln!(
                out,
                "    if obligation.edge.endpoint.peer_id.0 != peer_id {{"
            )?;
            writeln!(
                out,
                "        return Err(format!(\"MobMachine external trust obligation peer_id {{:?}} does not match requested peer {{peer_id:?}}\", obligation.edge.endpoint.peer_id.0));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(
                out,
                "    trusted_peer_descriptor_from_external_endpoint(&obligation.edge.endpoint)"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "mob_external_peer_reciprocal_trust" => {
            emit_endpoint_descriptor_converter(
                out,
                "trusted_peer_descriptor_from_member_endpoint",
                "crate::machines::mob_machine::MemberPeerEndpoint",
            )?;
            writeln!(
                out,
                "fn trusted_peer_descriptor_for_request(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
            )?;
            writeln!(
                out,
                "    if obligation.peer_endpoint.peer_id.0 != peer_id {{"
            )?;
            writeln!(
                out,
                "        return Err(format!(\"MobMachine external reciprocal trust obligation peer_id {{:?}} does not match requested peer {{peer_id:?}}\", obligation.peer_endpoint.peer_id.0));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(
                out,
                "    trusted_peer_descriptor_from_member_endpoint(&obligation.peer_endpoint)"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        other => bail!("unsupported descriptor-bearing comms trust protocol `{other}`"),
    }
    Ok(())
}

fn emit_endpoint_descriptor_converter(
    out: &mut String,
    fn_name: &str,
    endpoint_type: &str,
) -> Result<()> {
    writeln!(
        out,
        "fn {fn_name}(endpoint: &{endpoint_type}) -> Result<meerkat_core::comms::TrustedPeerDescriptor, String> {{"
    )?;
    writeln!(
        out,
        "    meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey("
    )?;
    writeln!(out, "        endpoint.name.0.clone(),")?;
    writeln!(out, "        endpoint.peer_id.0.as_str(),")?;
    writeln!(out, "        endpoint.signing_key.0,")?;
    writeln!(out, "        endpoint.address.0.as_str(),")?;
    writeln!(out, "    )")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_comms_trust_allowed_operation_check(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
) -> Result<()> {
    let trust_authority = protocol
        .comms_trust_authority
        .as_ref()
        .context("comms trust authority metadata missing")?;
    if trust_authority.allowed_operations.is_empty() {
        bail!(
            "comms trust authority protocol `{}` must declare at least one operation",
            protocol.name
        );
    }
    writeln!(
        out,
        "        use meerkat_core::comms::GeneratedCommsTrustAuthorityOperation as Operation;"
    )?;
    let allowed = trust_authority
        .allowed_operations
        .iter()
        .map(|operation| format!("Operation::{}", operation.core_variant()))
        .collect::<Vec<_>>()
        .join(" | ");
    writeln!(out, "        if !matches!(operation, {allowed}) {{")?;
    writeln!(
        out,
        "            return Err(format!(\"generated comms trust source cannot authorize operation {{operation:?}}\"));"
    )?;
    writeln!(out, "        }}")?;
    Ok(())
}

fn emit_comms_trust_payload_authorization(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
) -> Result<()> {
    match protocol.name.as_str() {
        "comms_trust_reconcile" => {
            writeln!(out, "        match operation {{")?;
            writeln!(out, "            Operation::PublicAdd => {{")?;
            writeln!(
                out,
                "                let requested = self.direct_peer_endpoints.iter().chain(self.mob_overlay_peer_endpoints.iter()).any(|endpoint| endpoint.peer_id.0 == peer_id);"
            )?;
            writeln!(out, "                if !requested {{")?;
            writeln!(
                out,
                "                    return Err(format!(\"MeerkatMachine peer projection did not request trust for peer {{peer_id:?}}\"));"
            )?;
            writeln!(out, "                }}")?;
            writeln!(out, "            }}")?;
            writeln!(out, "            Operation::PublicRemove => {{")?;
            writeln!(
                out,
                "                let still_requested = self.direct_peer_endpoints.iter().chain(self.mob_overlay_peer_endpoints.iter()).any(|endpoint| endpoint.peer_id.0 == peer_id);"
            )?;
            writeln!(out, "                if still_requested {{")?;
            writeln!(
                out,
                "                    return Err(format!(\"MeerkatMachine peer projection still requests trust for peer {{peer_id:?}}\"));"
            )?;
            writeln!(out, "                }}")?;
            writeln!(out, "            }}")?;
            writeln!(
                out,
                "            _ => unreachable!(\"operation checked above\"),"
            )?;
            writeln!(out, "        }}")?;
        }
        "supervisor_trust_publish" | "supervisor_trust_revoke" => {
            writeln!(out, "        if self.peer_id != peer_id {{")?;
            writeln!(
                out,
                "            return Err(format!(\"MeerkatMachine supervisor trust obligation peer_id {{:?}} does not match requested peer {{peer_id:?}}\", self.peer_id));"
            )?;
            writeln!(out, "        }}")?;
        }
        "mob_member_trust_wiring" | "mob_member_trust_unwiring" => {
            writeln!(
                out,
                "        if self.a_peer_id.0 != peer_id && self.b_peer_id.0 != peer_id {{"
            )?;
            writeln!(
                out,
                "            return Err(format!(\"MobMachine member trust obligation does not carry requested peer {{peer_id:?}}\"));"
            )?;
            writeln!(out, "        }}")?;
        }
        "mob_external_peer_trust_wiring"
        | "mob_external_peer_trust_unwiring"
        | "mob_external_peer_trust_repair"
        | "mob_external_peer_reciprocal_trust" => {
            writeln!(out, "        if self.peer_id.0 != peer_id {{")?;
            writeln!(
                out,
                "            return Err(format!(\"MobMachine external trust obligation peer_id {{:?}} does not match requested peer {{peer_id:?}}\", self.peer_id.0));"
            )?;
            writeln!(out, "        }}")?;
        }
        other => bail!("unsupported comms trust authority protocol `{other}`"),
    }
    Ok(())
}

fn comms_trust_epoch_expr(protocol: &EffectHandoffProtocol) -> Result<&'static str> {
    match protocol.name.as_str() {
        "comms_trust_reconcile" => Ok("self.peer_projection_epoch"),
        "supervisor_trust_publish"
        | "supervisor_trust_revoke"
        | "mob_member_trust_wiring"
        | "mob_member_trust_unwiring"
        | "mob_external_peer_trust_wiring"
        | "mob_external_peer_trust_unwiring"
        | "mob_external_peer_trust_repair"
        | "mob_external_peer_reciprocal_trust" => Ok("self.epoch"),
        other => bail!("unsupported comms trust authority protocol `{other}`"),
    }
}

fn comms_trust_target_peer_expr(protocol: &EffectHandoffProtocol) -> Result<Option<&'static str>> {
    match protocol.name.as_str() {
        "comms_trust_reconcile" | "supervisor_trust_publish" | "supervisor_trust_revoke" => {
            Ok(Some(
                "self.local_endpoint.as_ref().ok_or_else(|| \"generated MeerkatMachine trust obligation did not carry local trust-store endpoint\".to_string())?.peer_id.0.as_str()",
            ))
        }
        "mob_member_trust_wiring" | "mob_member_trust_unwiring" => Ok(Some(
            "if self.a_peer_id.0 == peer_id { self.b_peer_id.0.as_str() } else if self.b_peer_id.0 == peer_id { self.a_peer_id.0.as_str() } else { return Err(format!(\"MobMachine member trust obligation does not carry requested peer {peer_id:?}\")); }",
        )),
        "mob_external_peer_trust_wiring"
        | "mob_external_peer_trust_unwiring"
        | "mob_external_peer_trust_repair" => Ok(Some("self.local_peer_id.0.as_str()")),
        "mob_external_peer_reciprocal_trust" => Ok(Some("self.edge.endpoint.peer_id.0.as_str()")),
        other => bail!("unsupported comms trust authority protocol `{other}`"),
    }
}

fn generate_comms_trust_authority_sources(_compositions: &[CompositionSchema]) -> Result<String> {
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — comms trust authority source marker"
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "// Raw comms trust minting is intentionally not exposed from this public generated module."
    )?;
    writeln!(
        &mut out,
        "// Protocol-specific generated obligation helpers mint authorities only after validating"
    )?;
    writeln!(&mut out, "// extracted machine/composition obligations.")?;
    Ok(out)
}

pub fn render_comms_trust_authority_sources(compositions: &[CompositionSchema]) -> Result<String> {
    generate_comms_trust_authority_sources(compositions)
}

fn generate_auth_lease_transition_authority_sources(
    _compositions: &[CompositionSchema],
) -> Result<String> {
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — auth lease transition authority source marker"
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "// Raw auth lease transition minting is intentionally not exposed from this"
    )?;
    writeln!(
        &mut out,
        "// public generated module. The generated runtime handoff consumes a typed"
    )?;
    writeln!(
        &mut out,
        "// AuthMachine publication obligation before packaging the public transition."
    )?;
    Ok(out)
}

pub fn render_auth_lease_transition_authority_sources(
    compositions: &[CompositionSchema],
) -> Result<String> {
    generate_auth_lease_transition_authority_sources(compositions)
}

fn generate_tool_visibility_owner_protocol() -> Result<String> {
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — tool visibility owner authority bridge"
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]"
    )?;
    writeln!(
        &mut out,
        "#[allow(improper_ctypes_definitions, unsafe_code)]"
    )?;
    writeln!(&mut out, "unsafe extern \"Rust\" {{")?;
    writeln!(
        &mut out,
        "    #[link_name = concat!(\"__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_tool_visibility_owner_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\"))]"
    )?;
    writeln!(
        &mut out,
        "    fn runtime_tool_visibility_owner_generated_authority_bridge_token_is_valid(token: &(dyn std::any::Any + Send + Sync)) -> bool;"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]"
    )?;
    writeln!(&mut out, "#[doc(hidden)]")?;
    writeln!(
        &mut out,
        "#[allow(improper_ctypes_definitions, unsafe_code)]"
    )?;
    writeln!(
        &mut out,
        "#[unsafe(export_name = concat!(\"__meerkat_core_runtime_generated_tool_visibility_owner_build_v1_\", env!(\"MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX\")))]"
    )?;
    writeln!(
        &mut out,
        "pub extern \"Rust\" fn runtime_generated_tool_visibility_owner_build("
    )?;
    writeln!(
        &mut out,
        "    token: &'static (dyn std::any::Any + Send + Sync),"
    )?;
    writeln!(
        &mut out,
        "    owner: std::sync::Arc<dyn crate::tool_scope::ToolVisibilityOwner>,"
    )?;
    writeln!(
        &mut out,
        ") -> Result<crate::tool_scope::GeneratedToolVisibilityOwner, String> {{"
    )?;
    writeln!(
        &mut out,
        "    validate_runtime_tool_visibility_owner_bridge_token(token)?;"
    )?;
    writeln!(
        &mut out,
        "    Ok(crate::tool_scope::GeneratedToolVisibilityOwner::from_generated_authority(owner))"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]"
    )?;
    writeln!(
        &mut out,
        "fn validate_runtime_tool_visibility_owner_bridge_token("
    )?;
    writeln!(&mut out, "    token: &(dyn std::any::Any + Send + Sync),")?;
    writeln!(&mut out, ") -> Result<(), String> {{")?;
    writeln!(&mut out, "    #[allow(unsafe_code)]")?;
    writeln!(
        &mut out,
        "    let valid = unsafe {{ runtime_tool_visibility_owner_generated_authority_bridge_token_is_valid(token) }};"
    )?;
    writeln!(&mut out, "    if valid {{")?;
    writeln!(&mut out, "        Ok(())")?;
    writeln!(&mut out, "    }} else {{")?;
    writeln!(
        &mut out,
        "        Err(\"generated tool visibility owner requires the generated MeerkatMachine protocol bridge token\".into())"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    Ok(out)
}

#[derive(Debug, Clone, Copy)]
struct SessionPersistenceVersionConstants {
    session_envelope_version: u32,
    stored_input_state_version: u32,
    session_metadata_schema_version: u32,
    legacy_session_envelope_version: u32,
    legacy_stored_input_state_version: u32,
    legacy_session_metadata_schema_version: u32,
}

#[derive(Debug, Clone, Copy)]
struct SessionPersistenceVersionFieldSpec {
    enum_variant: &'static str,
    authorize_input: &'static str,
    restore_input: &'static str,
}

#[derive(Debug, Clone)]
struct SessionPersistenceVersionFieldPlan {
    current_version: u32,
    legacy_version: u32,
}

fn session_persistence_init_u32(machine: &MachineSchema, field: &str) -> Result<u32> {
    let init = machine
        .state
        .init
        .fields
        .iter()
        .find(|init| init.field.as_str() == field)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} missing initial field `{field}`",
                machine.machine.as_str()
            )
        })?;
    let Expr::U64(value) = init.expr else {
        bail!(
            "{} initial field `{field}` must be a u64 literal",
            machine.machine.as_str()
        );
    };
    u32::try_from(value).with_context(|| {
        format!(
            "{} initial field `{field}` does not fit u32",
            machine.machine.as_str()
        )
    })
}

fn session_persistence_transitions_for_input<'a>(
    machine: &'a MachineSchema,
    input: &str,
) -> Vec<&'a TransitionSchema> {
    machine
        .transitions
        .iter()
        .filter(|transition| {
            matches!(
                &transition.on,
                TriggerMatch::Input { variant, .. } if variant.as_str() == input
            )
        })
        .collect()
}

fn session_persistence_single_effect<'a>(
    transition: &'a TransitionSchema,
    effect: &str,
) -> Result<&'a EffectEmit> {
    if transition.emit.len() != 1 {
        bail!(
            "{} must emit exactly one effect, emitted {}",
            transition.name.as_str(),
            transition.emit.len()
        );
    }
    let emitted = &transition.emit[0];
    if emitted.variant.as_str() != effect {
        bail!(
            "{} emitted `{}` instead of `{effect}`",
            transition.name.as_str(),
            emitted.variant.as_str()
        );
    }
    Ok(emitted)
}

fn session_persistence_effect_expr<'a>(effect: &'a EffectEmit, field: &str) -> Result<&'a Expr> {
    effect
        .fields
        .iter()
        .find_map(|(candidate, expr)| (candidate.as_str() == field).then_some(expr))
        .with_context(|| format!("{} effect missing field `{field}`", effect.variant.as_str()))
}

fn session_persistence_expect_field_variant(
    effect: &EffectEmit,
    expected_variant: &str,
) -> Result<()> {
    let expr = session_persistence_effect_expr(effect, "field")?;
    let Expr::NamedVariant { enum_name, variant } = expr else {
        bail!(
            "{} effect field selector must be a SessionPersistenceVersionField variant",
            effect.variant.as_str()
        );
    };
    if enum_name.as_str() != "SessionPersistenceVersionField"
        || variant.as_str() != expected_variant
    {
        bail!(
            "{} effect selected {}::{}, expected SessionPersistenceVersionField::{expected_variant}",
            effect.variant.as_str(),
            enum_name.as_str(),
            variant.as_str()
        );
    }
    Ok(())
}

fn session_persistence_effect_state_field(effect: &EffectEmit, field: &str) -> Result<String> {
    let expr = session_persistence_effect_expr(effect, field)?;
    let Expr::Field(state_field) = expr else {
        bail!(
            "{} effect field `{field}` must come from machine state",
            effect.variant.as_str()
        );
    };
    Ok(state_field.as_str().to_owned())
}

fn session_persistence_collect_persisted_version_guards<'a>(
    expr: &'a Expr,
    out: &mut Vec<&'a str>,
) {
    match expr {
        Expr::And(items) => {
            for item in items {
                session_persistence_collect_persisted_version_guards(item, out);
            }
        }
        Expr::Eq(left, right) => {
            if let (Expr::Binding(binding), Expr::Field(field)) = (&**left, &**right)
                && binding == "persisted_version"
            {
                out.push(field.as_str());
            }
            if let (Expr::Field(field), Expr::Binding(binding)) = (&**left, &**right)
                && binding == "persisted_version"
            {
                out.push(field.as_str());
            }
        }
        _ => {}
    }
}

fn session_persistence_restore_guard_field(transition: &TransitionSchema) -> Result<String> {
    let mut fields = Vec::new();
    for guard in &transition.guards {
        session_persistence_collect_persisted_version_guards(&guard.expr, &mut fields);
    }
    match fields.as_slice() {
        [field] => Ok((*field).to_owned()),
        [] => bail!(
            "{} restore transition must guard persisted_version against a state version field",
            transition.name.as_str()
        ),
        _ => bail!(
            "{} restore transition has multiple persisted_version guards: {:?}",
            transition.name.as_str(),
            fields
        ),
    }
}

fn session_persistence_derive_field_plan(
    machine: &MachineSchema,
    spec: SessionPersistenceVersionFieldSpec,
) -> Result<SessionPersistenceVersionFieldPlan> {
    let authorize_transitions =
        session_persistence_transitions_for_input(machine, spec.authorize_input);
    let authorize_transition = match authorize_transitions.as_slice() {
        [transition] => *transition,
        _ => bail!(
            "{} expected exactly one `{}` transition, found {}",
            machine.machine.as_str(),
            spec.authorize_input,
            authorize_transitions.len()
        ),
    };
    let authorize_effect =
        session_persistence_single_effect(authorize_transition, "VersionStampAuthorized")?;
    session_persistence_expect_field_variant(authorize_effect, spec.enum_variant)?;
    let current_state_field = session_persistence_effect_state_field(authorize_effect, "version")?;
    let legacy_state_field =
        session_persistence_effect_state_field(authorize_effect, "legacy_default")?;

    let restore_transitions =
        session_persistence_transitions_for_input(machine, spec.restore_input);
    if restore_transitions.is_empty() {
        bail!(
            "{} missing `{}` restore transitions",
            machine.machine.as_str(),
            spec.restore_input
        );
    }
    let mut accepted_state_fields = std::collections::BTreeSet::new();
    for transition in restore_transitions {
        let restore_effect =
            session_persistence_single_effect(transition, "VersionRestoreAuthorized")?;
        session_persistence_expect_field_variant(restore_effect, spec.enum_variant)?;
        let restored_state_field =
            session_persistence_effect_state_field(restore_effect, "version")?;
        if restored_state_field != current_state_field {
            bail!(
                "{} restores `{}` but authorize transition stamps `{}`",
                transition.name.as_str(),
                restored_state_field,
                current_state_field
            );
        }
        accepted_state_fields.insert(session_persistence_restore_guard_field(transition)?);
    }
    let expected_state_fields =
        std::collections::BTreeSet::from([current_state_field.clone(), legacy_state_field.clone()]);
    if accepted_state_fields != expected_state_fields {
        bail!(
            "{} restore transitions for {} accept {:?}, expected {:?}",
            machine.machine.as_str(),
            spec.enum_variant,
            accepted_state_fields,
            expected_state_fields
        );
    }

    Ok(SessionPersistenceVersionFieldPlan {
        current_version: session_persistence_init_u32(machine, &current_state_field)?,
        legacy_version: session_persistence_init_u32(machine, &legacy_state_field)?,
    })
}

fn validate_session_persistence_version_authority_schema(
    machine: &MachineSchema,
) -> Result<SessionPersistenceVersionConstants> {
    machine
        .validate()
        .context("validate SessionPersistenceVersionAuthorityMachine schema")?;
    if machine.machine.as_str() != "SessionPersistenceVersionAuthorityMachine" {
        bail!(
            "expected SessionPersistenceVersionAuthorityMachine, got {}",
            machine.machine.as_str()
        );
    }
    let required_inputs = [
        "AuthorizeSessionEnvelopeVersionStamp",
        "AuthorizeStoredInputStateVersionStamp",
        "AuthorizeSessionMetadataSchemaVersionStamp",
        "RestoreSessionEnvelopeVersion",
        "RestoreStoredInputStateVersion",
        "RestoreSessionMetadataSchemaVersion",
    ];
    for input in required_inputs {
        if !machine
            .inputs
            .variants
            .iter()
            .any(|variant| variant.name.as_str() == input)
        {
            bail!("{} missing input `{input}`", machine.machine.as_str());
        }
    }
    for effect in ["VersionStampAuthorized", "VersionRestoreAuthorized"] {
        if !machine
            .effects
            .variants
            .iter()
            .any(|variant| variant.name.as_str() == effect)
        {
            bail!("{} missing effect `{effect}`", machine.machine.as_str());
        }
    }
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == "SessionPersistenceVersionField")
        .context(
            "SessionPersistenceVersionAuthorityMachine missing SessionPersistenceVersionField binding",
        )?;
    if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
        bail!("SessionPersistenceVersionField must be a string enum");
    }
    let session_envelope = session_persistence_derive_field_plan(
        machine,
        SessionPersistenceVersionFieldSpec {
            enum_variant: "SessionEnvelope",
            authorize_input: "AuthorizeSessionEnvelopeVersionStamp",
            restore_input: "RestoreSessionEnvelopeVersion",
        },
    )?;
    let stored_input_state = session_persistence_derive_field_plan(
        machine,
        SessionPersistenceVersionFieldSpec {
            enum_variant: "StoredInputState",
            authorize_input: "AuthorizeStoredInputStateVersionStamp",
            restore_input: "RestoreStoredInputStateVersion",
        },
    )?;
    let session_metadata_schema = session_persistence_derive_field_plan(
        machine,
        SessionPersistenceVersionFieldSpec {
            enum_variant: "SessionMetadataSchema",
            authorize_input: "AuthorizeSessionMetadataSchemaVersionStamp",
            restore_input: "RestoreSessionMetadataSchemaVersion",
        },
    )?;
    Ok(SessionPersistenceVersionConstants {
        session_envelope_version: session_envelope.current_version,
        stored_input_state_version: stored_input_state.current_version,
        session_metadata_schema_version: session_metadata_schema.current_version,
        legacy_session_envelope_version: session_envelope.legacy_version,
        legacy_stored_input_state_version: stored_input_state.legacy_version,
        legacy_session_metadata_schema_version: session_metadata_schema.legacy_version,
    })
}

fn generate_session_persistence_version_authority(machine: &MachineSchema) -> Result<String> {
    let constants = validate_session_persistence_version_authority_schema(machine)?;
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — session persistence version authority"
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `{}` transitions.",
        machine.machine.as_str()
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::fmt;")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use serde_json::{{Map, Value, json}};")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub const SESSION_VERSION: u32 = {};",
        constants.session_envelope_version
    )?;
    writeln!(
        &mut out,
        "pub const STORED_INPUT_STATE_VERSION: u32 = {};",
        constants.stored_input_state_version
    )?;
    writeln!(
        &mut out,
        "pub const SESSION_METADATA_SCHEMA_VERSION: u32 = {};",
        constants.session_metadata_schema_version
    )?;
    writeln!(
        &mut out,
        "const LEGACY_SESSION_VERSION: u32 = {};",
        constants.legacy_session_envelope_version
    )?;
    writeln!(
        &mut out,
        "const LEGACY_STORED_INPUT_STATE_VERSION: u32 = {};",
        constants.legacy_stored_input_state_version
    )?;
    writeln!(
        &mut out,
        "const LEGACY_SESSION_METADATA_SCHEMA_VERSION: u32 = {};",
        constants.legacy_session_metadata_schema_version
    )?;
    out.push_str(
        r#"
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPersistenceVersionField {
    SessionEnvelope,
    StoredInputState,
    SessionMetadataSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizedSessionPersistenceVersionStamp {
    field: SessionPersistenceVersionField,
    key: &'static str,
    version: u32,
    legacy_default: u32,
}

impl AuthorizedSessionPersistenceVersionStamp {
    #[must_use]
    pub fn field(&self) -> SessionPersistenceVersionField {
        self.field
    }

    #[must_use]
    pub fn key(&self) -> &'static str {
        self.key
    }

    #[must_use]
    pub fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub fn legacy_default(&self) -> u32 {
        self.legacy_default
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPersistenceVersionAuthorityError {
    field: SessionPersistenceVersionField,
    current: u32,
    legacy: u32,
    observed: Option<u32>,
}

impl fmt::Display for SessionPersistenceVersionAuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.observed {
            Some(observed) => write!(
                f,
                "generated session persistence version authority rejected {:?}: expected current {} or legacy {}, got {}",
                self.field, self.current, self.legacy, observed
            ),
            None => write!(
                f,
                "generated session persistence version authority rejected {:?}: expected current {} or legacy {}, got a non-u32 version",
                self.field, self.current, self.legacy
            ),
        }
    }
}

impl std::error::Error for SessionPersistenceVersionAuthorityError {}

fn authorize_version_stamp(
    field: SessionPersistenceVersionField,
    key: &'static str,
    version: u32,
    legacy_default: u32,
) -> AuthorizedSessionPersistenceVersionStamp {
    AuthorizedSessionPersistenceVersionStamp {
        field,
        key,
        version,
        legacy_default,
    }
}

fn restore_version(
    field: SessionPersistenceVersionField,
    observed: u32,
    current: u32,
    legacy: u32,
) -> Result<u32, SessionPersistenceVersionAuthorityError> {
    if observed == current || observed == legacy {
        Ok(current)
    } else {
        Err(SessionPersistenceVersionAuthorityError {
            field,
            current,
            legacy,
            observed: Some(observed),
        })
    }
}

fn observed_version_from_json(
    stamp: AuthorizedSessionPersistenceVersionStamp,
    value: Option<&Value>,
) -> Result<u32, SessionPersistenceVersionAuthorityError> {
    let Some(value) = value else {
        return Ok(stamp.legacy_default());
    };
    let Some(raw) = value.as_u64() else {
        return Err(SessionPersistenceVersionAuthorityError {
            field: stamp.field(),
            current: stamp.version(),
            legacy: stamp.legacy_default(),
            observed: None,
        });
    };
    u32::try_from(raw).map_err(|_| SessionPersistenceVersionAuthorityError {
        field: stamp.field(),
        current: stamp.version(),
        legacy: stamp.legacy_default(),
        observed: None,
    })
}

#[must_use]
pub fn session_envelope_version() -> u32 {
    SESSION_VERSION
}

#[must_use]
pub fn stored_input_state_version() -> u32 {
    STORED_INPUT_STATE_VERSION
}

#[must_use]
pub fn session_metadata_schema_version() -> u32 {
    SESSION_METADATA_SCHEMA_VERSION
}

#[must_use]
pub fn legacy_session_envelope_version() -> u32 {
    LEGACY_SESSION_VERSION
}

#[must_use]
pub fn legacy_stored_input_state_version() -> u32 {
    LEGACY_STORED_INPUT_STATE_VERSION
}

#[must_use]
pub fn legacy_session_metadata_schema_version() -> u32 {
    LEGACY_SESSION_METADATA_SCHEMA_VERSION
}

pub fn authorize_session_envelope_version_stamp() -> AuthorizedSessionPersistenceVersionStamp {
    authorize_version_stamp(
        SessionPersistenceVersionField::SessionEnvelope,
        "version",
        SESSION_VERSION,
        LEGACY_SESSION_VERSION,
    )
}

pub fn authorize_stored_input_state_version_stamp() -> AuthorizedSessionPersistenceVersionStamp {
    authorize_version_stamp(
        SessionPersistenceVersionField::StoredInputState,
        "stored_input_state_version",
        STORED_INPUT_STATE_VERSION,
        LEGACY_STORED_INPUT_STATE_VERSION,
    )
}

pub fn authorize_session_metadata_schema_version_stamp() -> AuthorizedSessionPersistenceVersionStamp
{
    authorize_version_stamp(
        SessionPersistenceVersionField::SessionMetadataSchema,
        "schema_version",
        SESSION_METADATA_SCHEMA_VERSION,
        LEGACY_SESSION_METADATA_SCHEMA_VERSION,
    )
}

pub fn restore_session_envelope_version(
    observed: u32,
) -> Result<u32, SessionPersistenceVersionAuthorityError> {
    restore_version(
        SessionPersistenceVersionField::SessionEnvelope,
        observed,
        SESSION_VERSION,
        LEGACY_SESSION_VERSION,
    )
}

pub fn restore_stored_input_state_version(
    observed: u32,
) -> Result<u32, SessionPersistenceVersionAuthorityError> {
    restore_version(
        SessionPersistenceVersionField::StoredInputState,
        observed,
        STORED_INPUT_STATE_VERSION,
        LEGACY_STORED_INPUT_STATE_VERSION,
    )
}

pub fn restore_session_metadata_schema_version(
    observed: u32,
) -> Result<u32, SessionPersistenceVersionAuthorityError> {
    restore_version(
        SessionPersistenceVersionField::SessionMetadataSchema,
        observed,
        SESSION_METADATA_SCHEMA_VERSION,
        LEGACY_SESSION_METADATA_SCHEMA_VERSION,
    )
}

pub fn stamp_authorized_version(
    object: &mut Map<String, Value>,
    stamp: AuthorizedSessionPersistenceVersionStamp,
) -> Result<(), SessionPersistenceVersionAuthorityError> {
    let observed = observed_version_from_json(stamp, object.get(stamp.key()))?;
    restore_version(
        stamp.field(),
        observed,
        stamp.version(),
        stamp.legacy_default(),
    )?;
    object.insert(stamp.key().to_string(), json!(stamp.version()));
    Ok(())
}
"#,
    );
    Ok(out)
}

pub fn render_session_persistence_version_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_persistence_version_authority(machine)
}

fn auth_lease_durable_marker_contract(
    compositions: &[CompositionSchema],
) -> Result<(
    &CompositionSchema,
    &EffectHandoffProtocol,
    &DurableMarkerProtocol,
)> {
    let protocols: Vec<_> = compositions
        .iter()
        .flat_map(|composition| {
            composition
                .handoff_protocols
                .iter()
                .filter_map(move |protocol| {
                    protocol
                        .durable_marker
                        .as_ref()
                        .map(|contract| (composition, protocol, contract))
                })
        })
        .collect();
    if protocols.len() != 1 {
        bail!(
            "expected exactly one durable marker handoff protocol, found {}",
            protocols.len()
        );
    }
    let (composition, protocol, contract) = protocols[0];
    validate_durable_marker_field(protocol, &contract.phase, "phase")?;
    validate_durable_marker_field(protocol, &contract.expires_at, "expires_at")?;
    validate_durable_marker_field(protocol, &contract.generation, "generation")?;
    validate_durable_marker_field(
        protocol,
        &contract.credential_published_at_millis,
        "credential_published_at_millis",
    )?;
    match contract.relation {
        DurableMarkerRelationProtocol::AuthLeaseCredentialPublication => {}
    }
    Ok((composition, protocol, contract))
}

fn validate_durable_marker_field(
    protocol: &EffectHandoffProtocol,
    binding: &DurableMarkerFieldBinding,
    label: &str,
) -> Result<()> {
    if !protocol
        .obligation_fields
        .iter()
        .any(|field| field == &binding.obligation_field)
    {
        bail!(
            "durable marker field `{label}` binds to `{}` but protocol `{}` does not carry that obligation field",
            binding.obligation_field,
            protocol.name
        );
    }
    Ok(())
}

fn generate_auth_lease_durable_lifecycle_marker_contract(
    compositions: &[CompositionSchema],
) -> Result<String> {
    let (composition, protocol, contract) = auth_lease_durable_marker_contract(compositions)?;
    let relation_label = match contract.relation {
        DurableMarkerRelationProtocol::AuthLeaseCredentialPublication => {
            "AuthLeaseCredentialPublication"
        }
    };
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — durable AuthMachine lifecycle marker contract"
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(
        &mut out,
        "// Composition: {}, Producer: {}, Protocol: {}",
        composition.name, protocol.producer_instance, protocol.name
    )?;
    writeln!(
        &mut out,
        "// Durable marker source fields: {}->{}, {}->{}, {}->{}, {}->{}, identity fields: {}, {}, {}",
        contract.phase.marker_field,
        contract.phase.obligation_field,
        contract.expires_at.marker_field,
        contract.expires_at.obligation_field,
        contract.generation.marker_field,
        contract.generation.obligation_field,
        contract.credential_published_at_millis.marker_field,
        contract.credential_published_at_millis.obligation_field,
        contract.realm_field,
        contract.binding_field,
        contract.profile_field,
    )?;
    writeln!(&mut out, "// Durable marker relation: {relation_label}")?;
    writeln!(&mut out, "#![allow(clippy::bool_comparison)]")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use crate::auth::{{PersistedTokens, TokenKey}};")?;
    writeln!(
        &mut out,
        "use crate::handles::{{AuthLeasePhase, AuthLeaseSnapshot}};"
    )?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) const METADATA_KEY: &str = {:?};",
        contract.metadata_key
    )?;
    writeln!(
        &mut out,
        "pub(crate) const PREVIOUS_METADATA_KEY: &str = {:?};",
        contract.previous_metadata_key
    )?;
    writeln!(
        &mut out,
        "const FIELD_PUBLISHED: &str = {:?};",
        contract.published_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_VERSION: &str = {:?};",
        contract.version_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_AUTHORITY: &str = {:?};",
        contract.authority_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_PROTOCOL: &str = {:?};",
        contract.protocol_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_REALM: &str = {:?};",
        contract.realm_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_BINDING: &str = {:?};",
        contract.binding_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_PROFILE: &str = {:?};",
        contract.profile_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_PHASE: &str = {:?};",
        contract.phase.marker_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_GENERATION: &str = {:?};",
        contract.generation.marker_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_EXPIRES_AT: &str = {:?};",
        contract.expires_at.marker_field
    )?;
    writeln!(
        &mut out,
        "const FIELD_CREDENTIAL_PUBLISHED_AT_MILLIS: &str = {:?};",
        contract.credential_published_at_millis.marker_field
    )?;
    writeln!(
        &mut out,
        "pub(crate) const SCHEMA_VERSION: u64 = {};",
        contract.schema_version
    )?;
    writeln!(
        &mut out,
        "const AUTHORITY: &str = {:?};",
        protocol.producer_instance.as_str()
    )?;
    writeln!(
        &mut out,
        "const PROTOCOL: &str = {:?};",
        protocol.name.as_str()
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub(crate) struct DurableAuthLifecycleMarker {{")?;
    writeln!(&mut out, "    pub token_key: TokenKey,")?;
    writeln!(&mut out, "    pub phase: AuthLeasePhase,")?;
    writeln!(&mut out, "    pub expires_at: u64,")?;
    writeln!(&mut out, "    pub generation: u64,")?;
    writeln!(&mut out, "    pub credential_published_at_millis: u64,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub enum AuthLeaseDurableMarkerRelation {{")?;
    writeln!(&mut out, "    Matches,")?;
    writeln!(&mut out, "    TokenNewer,")?;
    writeln!(&mut out, "    TokenStale,")?;
    writeln!(&mut out, "    Invalid,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub struct AuthLeaseDurableRestorePublication {{")?;
    writeln!(&mut out, "    token_key: TokenKey,")?;
    writeln!(&mut out, "    phase: AuthLeasePhase,")?;
    writeln!(&mut out, "    expires_at: u64,")?;
    writeln!(&mut out, "    generation: u64,")?;
    writeln!(&mut out, "    credential_published_at_millis: u64,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "impl AuthLeaseDurableRestorePublication {{")?;
    writeln!(&mut out, "    pub fn token_key(&self) -> &TokenKey {{")?;
    writeln!(&mut out, "        &self.token_key")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    pub fn phase(&self) -> AuthLeasePhase {{")?;
    writeln!(&mut out, "        self.phase")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    pub fn expires_at(&self) -> u64 {{")?;
    writeln!(&mut out, "        self.expires_at")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    pub fn generation(&self) -> u64 {{")?;
    writeln!(&mut out, "        self.generation")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "    pub fn credential_published_at_millis(&self) -> u64 {{"
    )?;
    writeln!(&mut out, "        self.credential_published_at_millis")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    #[cfg(not(target_arch = \"wasm32\"))]")?;
    writeln!(&mut out, "    fn from_marker_contract(")?;
    writeln!(&mut out, "        token_key: TokenKey,")?;
    writeln!(&mut out, "        phase: AuthLeasePhase,")?;
    writeln!(&mut out, "        expires_at: u64,")?;
    writeln!(&mut out, "        generation: u64,")?;
    writeln!(&mut out, "        credential_published_at_millis: u64,")?;
    writeln!(&mut out, "    ) -> Self {{")?;
    writeln!(&mut out, "        Self {{")?;
    writeln!(&mut out, "            token_key,")?;
    writeln!(&mut out, "            phase,")?;
    writeln!(&mut out, "            expires_at,")?;
    writeln!(&mut out, "            generation,")?;
    writeln!(&mut out, "            credential_published_at_millis,")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "fn phase_to_wire(phase: AuthLeasePhase) -> &'static str {{"
    )?;
    writeln!(&mut out, "    match phase {{")?;
    writeln!(&mut out, "        AuthLeasePhase::Valid => \"valid\",")?;
    writeln!(
        &mut out,
        "        AuthLeasePhase::Expiring => \"expiring\","
    )?;
    writeln!(&mut out, "        AuthLeasePhase::Expired => \"expired\",")?;
    writeln!(
        &mut out,
        "        AuthLeasePhase::Refreshing => \"refreshing\","
    )?;
    writeln!(
        &mut out,
        "        AuthLeasePhase::ReauthRequired => \"reauth_required\","
    )?;
    writeln!(
        &mut out,
        "        AuthLeasePhase::Released => \"released\","
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "fn phase_from_wire(value: &serde_json::Value) -> Option<AuthLeasePhase> {{"
    )?;
    writeln!(&mut out, "    match value.as_str()? {{")?;
    writeln!(
        &mut out,
        "        \"valid\" => Some(AuthLeasePhase::Valid),"
    )?;
    writeln!(
        &mut out,
        "        \"expiring\" => Some(AuthLeasePhase::Expiring),"
    )?;
    writeln!(
        &mut out,
        "        \"expired\" => Some(AuthLeasePhase::Expired),"
    )?;
    writeln!(
        &mut out,
        "        \"refreshing\" => Some(AuthLeasePhase::Refreshing),"
    )?;
    writeln!(
        &mut out,
        "        \"reauth_required\" => Some(AuthLeasePhase::ReauthRequired),"
    )?;
    writeln!(
        &mut out,
        "        \"released\" => Some(AuthLeasePhase::Released),"
    )?;
    writeln!(&mut out, "        _ => None,")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) fn encode_marker_value(marker: DurableAuthLifecycleMarker) -> serde_json::Value {{"
    )?;
    writeln!(&mut out, "    let mut map = serde_json::Map::new();")?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_PUBLISHED.to_string(), serde_json::Value::Bool(true));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_VERSION.to_string(), serde_json::Value::from(SCHEMA_VERSION));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_AUTHORITY.to_string(), serde_json::Value::String(AUTHORITY.to_string()));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_PROTOCOL.to_string(), serde_json::Value::String(PROTOCOL.to_string()));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_REALM.to_string(), serde_json::Value::String(marker.token_key.realm.as_str().to_string()));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_BINDING.to_string(), serde_json::Value::String(marker.token_key.binding.as_str().to_string()));"
    )?;
    writeln!(
        &mut out,
        "    if let Some(profile) = marker.token_key.profile.as_ref() {{"
    )?;
    writeln!(
        &mut out,
        "        map.insert(FIELD_PROFILE.to_string(), serde_json::Value::String(profile.as_str().to_string()));"
    )?;
    writeln!(&mut out, "    }} else {{")?;
    writeln!(
        &mut out,
        "        map.insert(FIELD_PROFILE.to_string(), serde_json::Value::Null);"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_PHASE.to_string(), serde_json::Value::String(phase_to_wire(marker.phase).to_string()));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_GENERATION.to_string(), serde_json::Value::from(marker.generation));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_EXPIRES_AT.to_string(), serde_json::Value::from(marker.expires_at));"
    )?;
    writeln!(
        &mut out,
        "    map.insert(FIELD_CREDENTIAL_PUBLISHED_AT_MILLIS.to_string(), serde_json::Value::from(marker.credential_published_at_millis));"
    )?;
    writeln!(&mut out, "    serde_json::Value::Object(map)")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) fn decode_marker_value(value: &serde_json::Value) -> Option<DurableAuthLifecycleMarker> {{"
    )?;
    writeln!(
        &mut out,
        "    (value.get(FIELD_PUBLISHED)?.as_bool()? == true).then_some(())?;"
    )?;
    writeln!(
        &mut out,
        "    (value.get(FIELD_VERSION)?.as_u64()? == SCHEMA_VERSION).then_some(())?;"
    )?;
    writeln!(
        &mut out,
        "    (value.get(FIELD_AUTHORITY)?.as_str()? == AUTHORITY).then_some(())?;"
    )?;
    writeln!(
        &mut out,
        "    (value.get(FIELD_PROTOCOL)?.as_str()? == PROTOCOL).then_some(())?;"
    )?;
    writeln!(
        &mut out,
        "    let token_key = TokenKey::parse_with_profile("
    )?;
    writeln!(&mut out, "        value.get(FIELD_REALM)?.as_str()?,")?;
    writeln!(&mut out, "        value.get(FIELD_BINDING)?.as_str()?,")?;
    writeln!(
        &mut out,
        "        value.get(FIELD_PROFILE).and_then(serde_json::Value::as_str),"
    )?;
    writeln!(&mut out, "    ).ok()?;")?;
    writeln!(&mut out, "    Some(DurableAuthLifecycleMarker {{")?;
    writeln!(&mut out, "        token_key,")?;
    writeln!(
        &mut out,
        "        phase: phase_from_wire(value.get(FIELD_PHASE)?)?,"
    )?;
    writeln!(
        &mut out,
        "        expires_at: value.get(FIELD_EXPIRES_AT)?.as_u64()?,"
    )?;
    writeln!(
        &mut out,
        "        generation: value.get(FIELD_GENERATION)?.as_u64()?,"
    )?;
    writeln!(
        &mut out,
        "        credential_published_at_millis: value.get(FIELD_CREDENTIAL_PUBLISHED_AT_MILLIS)?.as_u64()?,"
    )?;
    writeln!(&mut out, "    }})")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) fn read_marker_from_metadata(metadata: &serde_json::Value) -> Option<DurableAuthLifecycleMarker> {{"
    )?;
    writeln!(
        &mut out,
        "    decode_marker_value(metadata.get(METADATA_KEY)?)"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) fn metadata_has_valid_marker(metadata: &serde_json::Value) -> bool {{"
    )?;
    writeln!(
        &mut out,
        "    read_marker_from_metadata(metadata).is_some()"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub(crate) fn metadata_with_marker(metadata: &serde_json::Value, marker: DurableAuthLifecycleMarker) -> serde_json::Value {{"
    )?;
    writeln!(&mut out, "    let marker = encode_marker_value(marker);")?;
    writeln!(&mut out, "    match metadata {{")?;
    writeln!(&mut out, "        serde_json::Value::Object(map) => {{")?;
    writeln!(&mut out, "            let mut map = map.clone();")?;
    writeln!(
        &mut out,
        "            map.insert(METADATA_KEY.to_string(), marker);"
    )?;
    writeln!(&mut out, "            serde_json::Value::Object(map)")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "        serde_json::Value::Null => {{")?;
    writeln!(
        &mut out,
        "            let mut map = serde_json::Map::new();"
    )?;
    writeln!(
        &mut out,
        "            map.insert(METADATA_KEY.to_string(), marker);"
    )?;
    writeln!(&mut out, "            serde_json::Value::Object(map)")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "        other => {{")?;
    writeln!(
        &mut out,
        "            let mut map = serde_json::Map::new();"
    )?;
    writeln!(
        &mut out,
        "            map.insert(METADATA_KEY.to_string(), marker);"
    )?;
    writeln!(
        &mut out,
        "            map.insert(PREVIOUS_METADATA_KEY.to_string(), other.clone());"
    )?;
    writeln!(&mut out, "            serde_json::Value::Object(map)")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "#[cfg(not(target_arch = \"wasm32\"))]")?;
    writeln!(
        &mut out,
        "pub(crate) fn restore_publication_from_metadata(metadata: &serde_json::Value, expected_key: &TokenKey) -> Option<AuthLeaseDurableRestorePublication> {{"
    )?;
    writeln!(
        &mut out,
        "    let marker = read_marker_from_metadata(metadata)?;"
    )?;
    writeln!(
        &mut out,
        "    (&marker.token_key == expected_key).then_some(())?;"
    )?;
    writeln!(
        &mut out,
        "    Some(AuthLeaseDurableRestorePublication::from_marker_contract("
    )?;
    writeln!(&mut out, "            marker.token_key,")?;
    writeln!(&mut out, "            marker.phase,")?;
    writeln!(&mut out, "            marker.expires_at,")?;
    writeln!(&mut out, "            marker.generation,")?;
    writeln!(
        &mut out,
        "            marker.credential_published_at_millis,"
    )?;
    writeln!(&mut out, "    ))")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "pub fn marker_payload_valid_for_tokens(tokens: &PersistedTokens, expected_key: &TokenKey) -> bool {{"
    )?;
    writeln!(
        &mut out,
        "    let Some(marker) = read_marker_from_metadata(&tokens.metadata) else {{"
    )?;
    writeln!(&mut out, "        return false;")?;
    writeln!(&mut out, "    }};")?;
    writeln!(
        &mut out,
        "    marker.token_key == *expected_key && marker.expires_at == crate::persisted_token_expires_at_epoch_secs(tokens)"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "// Relation semantics are selected by the composition durable-marker contract."
    )?;
    writeln!(&mut out, "pub fn marker_relation_for_tokens_and_snapshot(")?;
    writeln!(&mut out, "    tokens: &PersistedTokens,")?;
    writeln!(&mut out, "    snapshot: &AuthLeaseSnapshot,")?;
    writeln!(&mut out, "    expected_key: &TokenKey,")?;
    writeln!(&mut out, ") -> AuthLeaseDurableMarkerRelation {{")?;
    writeln!(
        &mut out,
        "    let Some(marker) = read_marker_from_metadata(&tokens.metadata) else {{"
    )?;
    writeln!(
        &mut out,
        "        return AuthLeaseDurableMarkerRelation::Invalid;"
    )?;
    writeln!(&mut out, "    }};")?;
    writeln!(&mut out, "    if marker.token_key != *expected_key {{")?;
    writeln!(
        &mut out,
        "        return AuthLeaseDurableMarkerRelation::Invalid;"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(
        &mut out,
        "    let token_expires_at = crate::persisted_token_expires_at_epoch_secs(tokens);"
    )?;
    writeln!(&mut out, "    if marker.expires_at != token_expires_at {{")?;
    writeln!(
        &mut out,
        "        return AuthLeaseDurableMarkerRelation::Invalid;"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "    if !snapshot.credential_present {{")?;
    writeln!(
        &mut out,
        "        return AuthLeaseDurableMarkerRelation::TokenStale;"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(
        &mut out,
        "    let generation_matches = marker.generation == snapshot.generation;"
    )?;
    writeln!(
        &mut out,
        "    let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);"
    )?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "    if let Some(snapshot_published_at) = snapshot.credential_published_at_millis {{"
    )?;
    writeln!(
        &mut out,
        "        return match marker.credential_published_at_millis.cmp(&snapshot_published_at) {{"
    )?;
    writeln!(
        &mut out,
        "            std::cmp::Ordering::Greater => AuthLeaseDurableMarkerRelation::TokenNewer,"
    )?;
    writeln!(
        &mut out,
        "            std::cmp::Ordering::Less => AuthLeaseDurableMarkerRelation::TokenStale,"
    )?;
    writeln!(&mut out, "            std::cmp::Ordering::Equal => {{")?;
    writeln!(
        &mut out,
        "                if token_expires_at == snapshot_expires_at && generation_matches {{"
    )?;
    writeln!(
        &mut out,
        "                    AuthLeaseDurableMarkerRelation::Matches"
    )?;
    writeln!(&mut out, "                }} else {{")?;
    writeln!(
        &mut out,
        "                    AuthLeaseDurableMarkerRelation::Invalid"
    )?;
    writeln!(&mut out, "                }}")?;
    writeln!(&mut out, "            }}")?;
    writeln!(&mut out, "        }};")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "    match token_expires_at.cmp(&snapshot_expires_at) {{"
    )?;
    writeln!(
        &mut out,
        "        std::cmp::Ordering::Greater => AuthLeaseDurableMarkerRelation::TokenNewer,"
    )?;
    writeln!(
        &mut out,
        "        std::cmp::Ordering::Less => AuthLeaseDurableMarkerRelation::TokenStale,"
    )?;
    writeln!(&mut out, "        std::cmp::Ordering::Equal => {{")?;
    writeln!(&mut out, "            if generation_matches {{")?;
    writeln!(
        &mut out,
        "                AuthLeaseDurableMarkerRelation::Matches"
    )?;
    writeln!(&mut out, "            }} else {{")?;
    writeln!(
        &mut out,
        "                AuthLeaseDurableMarkerRelation::Invalid"
    )?;
    writeln!(&mut out, "            }}")?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    Ok(out)
}

pub fn render_auth_lease_durable_lifecycle_marker_contract(
    compositions: &[CompositionSchema],
) -> Result<String> {
    generate_auth_lease_durable_lifecycle_marker_contract(compositions)
}

fn generate_approval_lifecycle_authority(machine: &MachineSchema) -> Result<String> {
    validate_approval_lifecycle_schema(machine)?;

    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — approval lifecycle authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `ApprovalLifecycleMachine`."
    )?;
    writeln!(
        &mut out,
        "#![allow(clippy::bool_comparison, clippy::field_reassign_with_default, clippy::never_loop, clippy::nonminimal_bool, clippy::partialeq_to_none, clippy::redundant_clone)]"
    )?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "use std::{{collections::{{BTreeMap, BTreeSet}}, fmt}};"
    )?;
    writeln!(&mut out)?;

    for enum_name in [
        "ApprovalLifecycleStatus",
        "ApprovalLifecycleDecision",
        "ApprovalLifecycleRejectionReason",
    ] {
        emit_approval_named_string_enum(&mut out, machine, enum_name)?;
    }
    emit_approval_variant_enum(&mut out, "ApprovalLifecycleInput", &machine.inputs.variants)?;
    emit_approval_variant_enum(
        &mut out,
        "ApprovalLifecycleEffect",
        &machine.effects.variants,
    )?;
    emit_approval_outcome_and_error(&mut out)?;
    emit_approval_phase_enum(&mut out, machine)?;
    emit_approval_state_struct(&mut out, machine)?;
    emit_approval_transition_enum(&mut out, machine)?;
    emit_approval_authority_impl(&mut out, machine)?;
    for helper in &machine.helpers {
        emit_approval_helper(&mut out, helper, machine)?;
    }

    writeln!(
        &mut out,
        "impl Default for ApprovalLifecycleMachineAuthority {{"
    )?;
    writeln!(&mut out, "    fn default() -> Self {{")?;
    writeln!(&mut out, "        Self::new()")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    Ok(out)
}

fn emit_approval_named_string_enum(
    out: &mut String,
    machine: &MachineSchema,
    name: &str,
) -> Result<()> {
    let variants = approval_named_string_enum_variants(machine, name)?;
    let default_variant = approval_default_variant(name, &variants)?;
    emit_string_enum(out, name, &variants, default_variant)
}

fn approval_named_string_enum_variants(machine: &MachineSchema, name: &str) -> Result<Vec<String>> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| format!("ApprovalLifecycleMachine missing named type `{name}`"))?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("ApprovalLifecycleMachine named type `{name}` must be a string enum");
    };
    Ok(variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect())
}

fn approval_default_variant<'a>(name: &str, variants: &'a [String]) -> Result<&'a str> {
    let wanted = match name {
        "ApprovalLifecycleStatus" => "Pending",
        "ApprovalLifecycleDecision" => "Approve",
        "ApprovalLifecycleRejectionReason" => "NotFound",
        other => bail!("unknown ApprovalLifecycleMachine enum `{other}`"),
    };
    if variants.iter().any(|variant| variant == wanted) {
        Ok(wanted)
    } else {
        bail!("ApprovalLifecycleMachine enum `{name}` missing default variant `{wanted}`");
    }
}

fn emit_approval_variant_enum(
    out: &mut String,
    enum_name: &str,
    variants: &[VariantSchema],
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub enum {enum_name} {{")?;
    for variant in variants {
        emit_approval_variant(out, variant)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_variant(out: &mut String, variant: &VariantSchema) -> Result<()> {
    if variant.fields.is_empty() {
        writeln!(out, "    {},", variant.name)?;
        return Ok(());
    }
    writeln!(out, "    {} {{", variant.name)?;
    for field in &variant.fields {
        writeln!(
            out,
            "        {}: {},",
            field.name,
            render_approval_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "    }},")?;
    Ok(())
}

fn emit_approval_outcome_and_error(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub enum ApprovalLifecycleOutcome {{")?;
    writeln!(out, "    Status(ApprovalLifecycleStatus),")?;
    writeln!(out, "    Rejected(ApprovalLifecycleRejectionReason),")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct ApprovalLifecycleError {{")?;
    writeln!(out, "    op: &'static str,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl fmt::Display for ApprovalLifecycleError {{")?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{"
    )?;
    writeln!(
        out,
        "        write!(f, \"generated approval lifecycle authority rejected {{}}\", self.op)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(
        out,
        "impl std::error::Error for ApprovalLifecycleError {{}}"
    )?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_phase_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    let variants = machine
        .state
        .phase
        .variants
        .iter()
        .map(|variant| variant.name.as_str().to_owned())
        .collect::<Vec<_>>();
    emit_string_enum(
        out,
        machine.state.phase.name.as_str(),
        &variants,
        machine.state.init.phase.as_str(),
    )
}

fn emit_approval_state_struct(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq, Default)]")?;
    writeln!(out, "pub struct ApprovalLifecycleMachineState {{")?;
    writeln!(
        out,
        "    lifecycle_phase: {},",
        machine.state.phase.name.as_str()
    )?;
    for field in &machine.state.fields {
        writeln!(
            out,
            "    {}: {},",
            field.name,
            render_approval_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_transition_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "enum ApprovalLifecycleTransition {{")?;
    for transition in &machine.transitions {
        writeln!(out, "    {},", transition.name)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_authority_impl(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct ApprovalLifecycleMachineAuthority {{")?;
    writeln!(out, "    state: ApprovalLifecycleMachineState,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl ApprovalLifecycleMachineAuthority {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new() -> Self {{")?;
    writeln!(
        out,
        "        let mut state = ApprovalLifecycleMachineState::default();"
    )?;
    writeln!(
        out,
        "        state.lifecycle_phase = {}::{};",
        machine.state.phase.name, machine.state.init.phase
    )?;
    for init in &machine.state.init.fields {
        writeln!(
            out,
            "        state.{} = {};",
            init.field,
            render_approval_init_expr(&init.expr, machine)?
        )?;
    }
    writeln!(out, "        Self {{ state }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn state(&self) -> &ApprovalLifecycleMachineState {{"
    )?;
    writeln!(out, "        &self.state")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn status_for(&self, approval_id: &str) -> Option<ApprovalLifecycleStatus> {{"
    )?;
    writeln!(
        out,
        "        self.state.approval_statuses.get(approval_id).copied()"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    emit_approval_map_lookup_methods(out, machine)?;
    emit_approval_single_transition(out)?;
    emit_approval_apply_input(out, machine)?;
    emit_approval_outcome_from_effects(out, machine)?;
    emit_approval_public_methods(out, machine)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_map_lookup_methods(out: &mut String, machine: &MachineSchema) -> Result<()> {
    for field in &machine.state.fields {
        let TypeRef::Map(key_ty, value_ty) = &field.ty else {
            continue;
        };
        if !matches!(key_ty.as_ref(), TypeRef::String) {
            bail!(
                "ApprovalLifecycleMachine map field `{}` must use String keys",
                field.name
            );
        }
        let value_ty = render_approval_type_ref(value_ty)?;
        let accessor = if approval_type_is_copy(value_ty.as_str()) {
            "copied"
        } else {
            "cloned"
        };
        writeln!(out, "    fn {}_value(", field.name)?;
        writeln!(out, "        &self,")?;
        writeln!(out, "        approval_id: &str,")?;
        writeln!(
            out,
            "    ) -> Result<{value_ty}, ApprovalLifecycleError> {{"
        )?;
        writeln!(
            out,
            "        self.state.{}.get(approval_id).{}().ok_or(ApprovalLifecycleError {{ op: \"{}\" }})",
            field.name, accessor, field.name
        )?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
    }
    Ok(())
}

fn emit_approval_single_transition(out: &mut String) -> Result<()> {
    writeln!(out, "    fn single_transition(")?;
    writeln!(out, "        matches: Vec<ApprovalLifecycleTransition>,")?;
    writeln!(out, "        op: &'static str,")?;
    writeln!(
        out,
        "    ) -> Result<ApprovalLifecycleTransition, ApprovalLifecycleError> {{"
    )?;
    writeln!(out, "        let mut matches = matches.into_iter();")?;
    writeln!(out, "        let Some(first) = matches.next() else {{")?;
    writeln!(
        out,
        "            return Err(ApprovalLifecycleError {{ op }});"
    )?;
    writeln!(out, "        }};")?;
    writeln!(out, "        if matches.next().is_some() {{")?;
    writeln!(
        out,
        "            return Err(ApprovalLifecycleError {{ op }});"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        Ok(first)")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_apply_input(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "    fn apply_input(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        input: ApprovalLifecycleInput,")?;
    writeln!(
        out,
        "    ) -> Result<Vec<ApprovalLifecycleEffect>, ApprovalLifecycleError> {{"
    )?;
    writeln!(out, "        match input {{")?;
    for input_variant in &machine.inputs.variants {
        emit_approval_apply_input_arm(out, machine, input_variant)?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_approval_apply_input_arm(
    out: &mut String,
    machine: &MachineSchema,
    input_variant: &VariantSchema,
) -> Result<()> {
    write!(
        out,
        "            ApprovalLifecycleInput::{}",
        input_variant.name
    )?;
    if input_variant.fields.is_empty() {
        writeln!(out, " => {{")?;
    } else {
        writeln!(out, " {{")?;
        for field in &input_variant.fields {
            writeln!(out, "                {},", field.name)?;
        }
        writeln!(out, "            }} => {{")?;
    }

    let binding_types = approval_binding_types(input_variant);
    let transitions = machine
        .transitions
        .iter()
        .filter(|transition| {
            matches!(
                &transition.on,
                TriggerMatch::Input { variant, .. } if variant.as_str() == input_variant.name.as_str()
            )
        })
        .collect::<Vec<_>>();
    writeln!(out, "                let mut matches = Vec::new();")?;
    for transition in &transitions {
        writeln!(
            out,
            "                if {} {{",
            render_approval_transition_condition(transition, &binding_types, machine)?
        )?;
        writeln!(
            out,
            "                    matches.push(ApprovalLifecycleTransition::{});",
            transition.name
        )?;
        writeln!(out, "                }}")?;
    }
    writeln!(
        out,
        "                let transition = Self::single_transition(matches, \"{}\")?;",
        input_variant.name
    )?;
    writeln!(out, "                match transition {{")?;
    for transition in &transitions {
        emit_approval_transition_apply_block(out, transition, &binding_types, machine)?;
    }
    writeln!(
        out,
        "                    _ => Err(ApprovalLifecycleError {{ op: \"{}_transition\" }}),",
        input_variant.name
    )?;
    writeln!(out, "                }}")?;
    writeln!(out, "            }}")?;
    Ok(())
}

fn emit_approval_transition_apply_block(
    out: &mut String,
    transition: &TransitionSchema,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<()> {
    writeln!(
        out,
        "                    ApprovalLifecycleTransition::{} => {{",
        transition.name
    )?;
    for update in &transition.updates {
        writeln!(
            out,
            "                        {}",
            render_approval_update(update, binding_types, machine)?
        )?;
    }
    writeln!(
        out,
        "                        self.state.lifecycle_phase = {}::{};",
        machine.state.phase.name, transition.to
    )?;
    if transition.emit.is_empty() {
        writeln!(out, "                        Ok(Vec::new())")?;
    } else {
        writeln!(out, "                        Ok(vec![")?;
        for effect in &transition.emit {
            writeln!(
                out,
                "                            {},",
                render_approval_effect_emit(effect, binding_types, machine)?
            )?;
        }
        writeln!(out, "                        ])")?;
    }
    writeln!(out, "                    }}")?;
    Ok(())
}

fn render_approval_transition_condition(
    transition: &TransitionSchema,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    let mut conditions = Vec::new();
    if transition.from.len() == 1 {
        conditions.push(format!(
            "self.state.lifecycle_phase == {}::{}",
            machine.state.phase.name, transition.from[0]
        ));
    } else if !transition.from.is_empty() {
        conditions.push(format!(
            "matches!(self.state.lifecycle_phase, {})",
            transition
                .from
                .iter()
                .map(|phase| format!("{}::{phase}", machine.state.phase.name))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    for guard in &transition.guards {
        conditions.push(render_approval_expr(&guard.expr, binding_types, machine)?);
    }
    if conditions.is_empty() {
        Ok("true".to_string())
    } else {
        Ok(conditions
            .into_iter()
            .map(|condition| format!("({condition})"))
            .collect::<Vec<_>>()
            .join(" && "))
    }
}

fn render_approval_update(
    update: &Update,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match update {
        Update::Assign { field, expr } => Ok(format!(
            "self.state.{field} = {};",
            render_approval_owned_expr(expr, binding_types, machine)?
        )),
        Update::MapInsert { field, key, value } => Ok(format!(
            "self.state.{field}.insert({}, {});",
            render_approval_owned_expr(key, binding_types, machine)?,
            render_approval_owned_expr(value, binding_types, machine)?
        )),
        Update::SetInsert { field, value } => Ok(format!(
            "self.state.{field}.insert({});",
            render_approval_owned_expr(value, binding_types, machine)?
        )),
        other => bail!("unsupported ApprovalLifecycleMachine update `{other:?}`"),
    }
}

fn render_approval_effect_emit(
    effect: &EffectEmit,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    let variant = machine
        .effects
        .variant_named(effect.variant.as_str())
        .with_context(|| {
            format!(
                "ApprovalLifecycleMachine effect `{}` missing from schema",
                effect.variant
            )
        })?;
    if effect.fields.is_empty() {
        return Ok(format!("ApprovalLifecycleEffect::{}", effect.variant));
    }
    let mut rendered = format!("ApprovalLifecycleEffect::{} {{", effect.variant);
    for (idx, (field, expr)) in effect.fields.iter().enumerate() {
        if idx > 0 {
            rendered.push(' ');
        }
        let field_schema = variant.field_named(field.as_str()).with_context(|| {
            format!(
                "ApprovalLifecycleMachine effect `{}` missing field `{}`",
                effect.variant, field
            )
        })?;
        write!(
            &mut rendered,
            " {field}: {},",
            render_approval_effect_field_expr(expr, &field_schema.ty, binding_types, machine)?
        )?;
    }
    rendered.push_str(" }");
    Ok(rendered)
}

fn emit_approval_outcome_from_effects(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "    fn outcome_from_effects(")?;
    writeln!(out, "        effects: Vec<ApprovalLifecycleEffect>,")?;
    writeln!(out, "        op: &'static str,")?;
    writeln!(
        out,
        "    ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError> {{"
    )?;
    writeln!(out, "        for effect in effects {{")?;
    writeln!(out, "            match effect {{")?;
    for effect in &machine.effects.variants {
        let outcome = approval_outcome_for_effect(effect)?;
        let field = match outcome {
            ApprovalEffectOutcome::Status => "status",
            ApprovalEffectOutcome::Rejected => "reason",
        };
        let outcome_ctor = match outcome {
            ApprovalEffectOutcome::Status => "Status",
            ApprovalEffectOutcome::Rejected => "Rejected",
        };
        writeln!(
            out,
            "                ApprovalLifecycleEffect::{} {{ {field}, .. }} => {{",
            effect.name
        )?;
        writeln!(
            out,
            "                    return Ok(ApprovalLifecycleOutcome::{outcome_ctor}({field}));"
        )?;
        writeln!(out, "                }}")?;
    }
    writeln!(out, "            }}")?;
    writeln!(out, "        }}")?;
    writeln!(out, "        Err(ApprovalLifecycleError {{ op }})")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalEffectOutcome {
    Status,
    Rejected,
}

fn approval_outcome_for_effect(effect: &VariantSchema) -> Result<ApprovalEffectOutcome> {
    if effect.fields.iter().any(|field| {
        matches!(&field.ty, TypeRef::Enum(enum_name) if enum_name.as_str() == "ApprovalLifecycleStatus")
    }) {
        return Ok(ApprovalEffectOutcome::Status);
    }
    if effect.fields.iter().any(|field| {
        matches!(&field.ty, TypeRef::Enum(enum_name) if enum_name.as_str() == "ApprovalLifecycleRejectionReason")
    }) {
        return Ok(ApprovalEffectOutcome::Rejected);
    }
    bail!(
        "ApprovalLifecycleMachine effect `{}` has no status or rejection outcome field",
        effect.name
    )
}

fn emit_approval_public_methods(out: &mut String, machine: &MachineSchema) -> Result<()> {
    for input in &machine.inputs.variants {
        let method = to_snake_case(input.name.as_str());
        writeln!(out, "    pub fn {method}(")?;
        writeln!(out, "        &mut self,")?;
        for field in &input.fields {
            writeln!(
                out,
                "        {}: {},",
                field.name,
                render_approval_type_ref(&field.ty)?
            )?;
        }
        writeln!(
            out,
            "    ) -> Result<ApprovalLifecycleOutcome, ApprovalLifecycleError> {{"
        )?;
        if input.fields.is_empty() {
            writeln!(
                out,
                "        let effects = self.apply_input(ApprovalLifecycleInput::{})?;",
                input.name
            )?;
        } else {
            writeln!(
                out,
                "        let effects = self.apply_input(ApprovalLifecycleInput::{} {{",
                input.name
            )?;
            for field in &input.fields {
                writeln!(out, "            {},", field.name)?;
            }
            writeln!(out, "        }})?;")?;
        }
        writeln!(
            out,
            "        Self::outcome_from_effects(effects, \"{method}_effect\")"
        )?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
    }
    Ok(())
}

fn emit_approval_helper(
    out: &mut String,
    helper: &HelperSchema,
    machine: &MachineSchema,
) -> Result<()> {
    write!(out, "fn {}(", helper.name)?;
    for (idx, param) in helper.params.iter().enumerate() {
        if idx > 0 {
            write!(out, ", ")?;
        }
        write!(
            out,
            "{}: {}",
            param.name.as_str(),
            render_approval_type_ref(&param.ty)?
        )?;
    }
    writeln!(
        out,
        ") -> {} {{",
        render_approval_type_ref(&helper.returns)?
    )?;
    let binding_types = helper
        .params
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect();
    writeln!(
        out,
        "    {}",
        render_approval_expr(&helper.body, &binding_types, machine)?
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn render_approval_type_ref(ty: &TypeRef) -> Result<String> {
    match ty {
        TypeRef::Bool => Ok("bool".to_string()),
        TypeRef::U32 => Ok("u32".to_string()),
        TypeRef::U64 => Ok("u64".to_string()),
        TypeRef::String => Ok("String".to_string()),
        TypeRef::Named(name) => Ok(name.as_str().to_string()),
        TypeRef::Enum(name) => Ok(name.as_str().to_string()),
        TypeRef::Option(inner) => Ok(format!("Option<{}>", render_approval_type_ref(inner)?)),
        TypeRef::Set(inner) => Ok(format!("BTreeSet<{}>", render_approval_type_ref(inner)?)),
        TypeRef::Map(key, value) => Ok(format!(
            "BTreeMap<{}, {}>",
            render_approval_type_ref(key)?,
            render_approval_type_ref(value)?
        )),
        other => bail!("unsupported ApprovalLifecycleMachine type `{other:?}`"),
    }
}

fn render_approval_init_expr(expr: &Expr, machine: &MachineSchema) -> Result<String> {
    match expr {
        Expr::EmptySet => Ok("BTreeSet::new()".to_string()),
        Expr::EmptyMap => Ok("BTreeMap::new()".to_string()),
        other => render_approval_expr(other, &std::collections::BTreeMap::new(), machine),
    }
}

fn render_approval_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match expr {
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::String(value) => Ok(format!("{value:?}.to_string()")),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::CurrentPhase => Ok("self.state.lifecycle_phase".to_string()),
        Expr::Phase(phase) => Ok(format!("{}::{phase}", machine.state.phase.name)),
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!(
            "Some({})",
            render_approval_expr(inner, binding_types, machine)?
        )),
        Expr::Not(inner) => Ok(format!(
            "!({})",
            render_approval_expr(inner, binding_types, machine)?
        )),
        Expr::And(items) => render_approval_expr_joined(items, " && ", binding_types, machine),
        Expr::Or(items) => render_approval_expr_joined(items, " || ", binding_types, machine),
        Expr::Eq(left, right) => Ok(format!(
            "{} == {}",
            render_approval_expr(left, binding_types, machine)?,
            render_approval_expr(right, binding_types, machine)?
        )),
        Expr::Neq(left, right) => Ok(format!(
            "{} != {}",
            render_approval_expr(left, binding_types, machine)?,
            render_approval_expr(right, binding_types, machine)?
        )),
        Expr::Contains { collection, value } => Ok(format!(
            "{}.contains({})",
            render_approval_collection_expr(collection)?,
            render_approval_borrowed_string_expr(value, binding_types)?
        )),
        Expr::MapContainsKey { map, key } => Ok(format!(
            "{}.contains_key({})",
            render_approval_collection_expr(map)?,
            render_approval_borrowed_string_expr(key, binding_types)?
        )),
        Expr::MapGet { map, key } => {
            if let Some(rendered) = render_approval_optional_map_value_get(map, key, binding_types)?
            {
                return Ok(rendered);
            }
            let Expr::Field(field) = map.as_ref() else {
                bail!("ApprovalLifecycleMachine MapGet must read a state field: {map:?}");
            };
            Ok(format!(
                "self.{}_value({})?",
                field,
                render_approval_borrowed_string_expr(key, binding_types)?
            ))
        }
        Expr::Call { helper, args } => {
            let rendered_args = args
                .iter()
                .map(|arg| render_approval_expr(arg, binding_types, machine))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("{helper}({rendered_args})"))
        }
        other => bail!("unsupported ApprovalLifecycleMachine expression `{other:?}`"),
    }
}

fn render_approval_optional_map_value_get(
    map: &Expr,
    key: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
) -> Result<Option<String>> {
    let Expr::String(projected_field) = key else {
        return Ok(None);
    };
    if projected_field != "value" {
        return Ok(None);
    }
    let Expr::IfElse {
        condition,
        then_expr,
        else_expr,
    } = map
    else {
        return Ok(None);
    };
    if !matches!(else_expr.as_ref(), Expr::None) {
        return Ok(None);
    }
    let Expr::MapContainsKey {
        map: condition_map,
        key: condition_key,
    } = condition.as_ref()
    else {
        return Ok(None);
    };
    let Expr::Some(then_inner) = then_expr.as_ref() else {
        return Ok(None);
    };
    let Expr::MapGet {
        map: then_map,
        key: then_key,
    } = then_inner.as_ref()
    else {
        return Ok(None);
    };
    if condition_map != then_map || condition_key != then_key {
        bail!("ApprovalLifecycleMachine optional map projection has incoherent key source");
    }
    let Expr::Field(field) = then_map.as_ref() else {
        bail!("ApprovalLifecycleMachine optional map projection must read a state field");
    };
    Ok(Some(format!(
        "self.{}_value({})?",
        field,
        render_approval_borrowed_string_expr(then_key, binding_types)?
    )))
}

fn render_approval_expr_joined(
    items: &[Expr],
    separator: &str,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    if items.is_empty() {
        bail!("ApprovalLifecycleMachine expression cannot join an empty list");
    }
    let rendered = items
        .iter()
        .map(|expr| {
            Ok(format!(
                "({})",
                render_approval_expr(expr, binding_types, machine)?
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(separator))
}

fn render_approval_collection_expr(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        other => bail!("ApprovalLifecycleMachine collection expression must be a field: {other:?}"),
    }
}

fn render_approval_borrowed_string_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
) -> Result<String> {
    match expr {
        Expr::Binding(binding) if matches!(binding_types.get(binding), Some(TypeRef::String)) => {
            Ok(format!("{binding}.as_str()"))
        }
        Expr::String(value) => Ok(format!("{value:?}")),
        other => bail!("ApprovalLifecycleMachine expected borrowed string expression: {other:?}"),
    }
}

fn render_approval_owned_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match expr {
        Expr::Binding(binding) if matches!(binding_types.get(binding), Some(TypeRef::String)) => {
            Ok(format!("{binding}.clone()"))
        }
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::String(value) => Ok(format!("{value:?}.to_string()")),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!(
            "Some({})",
            render_approval_owned_expr(inner, binding_types, machine)?
        )),
        other => render_approval_expr(other, binding_types, machine),
    }
}

fn render_approval_effect_field_expr(
    expr: &Expr,
    ty: &TypeRef,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    if matches!(ty, TypeRef::String) {
        render_approval_owned_expr(expr, binding_types, machine)
    } else {
        render_approval_expr(expr, binding_types, machine)
    }
}

fn approval_binding_types(variant: &VariantSchema) -> std::collections::BTreeMap<String, TypeRef> {
    variant
        .fields
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect()
}

fn approval_type_is_copy(type_name: &str) -> bool {
    matches!(
        type_name,
        "bool"
            | "u32"
            | "u64"
            | "ApprovalLifecycleStatus"
            | "ApprovalLifecycleDecision"
            | "ApprovalLifecycleRejectionReason"
    )
}

pub fn render_approval_lifecycle_authority(machine: &MachineSchema) -> Result<String> {
    generate_approval_lifecycle_authority(machine)
}

fn validate_approval_lifecycle_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate ApprovalLifecycleMachine schema")?;
    if machine.machine.as_str() != "ApprovalLifecycleMachine" {
        bail!(
            "approval lifecycle generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "CreateApproval",
        "RestoreApproval",
        "ObserveApprovalExpiry",
        "DecideApproval",
    ] {
        machine
            .inputs
            .variant_named(required)
            .with_context(|| format!("ApprovalLifecycleMachine missing input `{required}`"))?;
    }
    for required in ["ApprovalStatusResolved", "ApprovalLifecycleRejected"] {
        machine
            .effects
            .variant_named(required)
            .with_context(|| format!("ApprovalLifecycleMachine missing effect `{required}`"))?;
    }
    for required in [
        "ApprovalLifecycleStatus",
        "ApprovalLifecycleDecision",
        "ApprovalLifecycleRejectionReason",
    ] {
        let binding = machine
            .named_types
            .iter()
            .find(|binding| binding.name.as_str() == required)
            .with_context(|| format!("ApprovalLifecycleMachine missing named type `{required}`"))?;
        if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
            bail!("ApprovalLifecycleMachine named type `{required}` must be a string enum");
        }
    }
    Ok(())
}

// =========================================================================
// SessionDocumentMachine — canonical per-session session-document authority
// =========================================================================
//
// This emitter is a pure mechanical schema-walker, modeled on
// `generate_approval_lifecycle_authority`. It iterates `transition.guards`,
// `transition.updates`, and `transition.emit` and lowers each DSL `Expr` /
// `Update` / `EffectEmit` to Rust. It contains NO hand-coded domain semantics:
// every decision (phase transition, store/clear, override legality) lives in
// the SessionDocumentMachine DSL transitions. The only domain knowledge here
// is the same map-key/string-projection lowering the approval emitter uses,
// generalized to a `Named` map key.

/// Rust type emitted for the machine's `Named("SessionId")` map key. The
/// generated file is schema-free (no `meerkat_machine_schema` reference), so
/// it carries its own `Ord + Clone` key newtype rather than borrowing the
/// richer `meerkat_core::SessionId` (which is not `Ord`). The session shell
/// builds this key from the session id string.
const SESSION_DOCUMENT_KEY_TYPE: &str = "SessionDocumentKey";
const SESSION_DOCUMENT_KEY_NAMED: &str = "SessionId";

pub fn render_session_document_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_document_authority(machine)
}

fn generate_session_document_authority(machine: &MachineSchema) -> Result<String> {
    validate_session_document_authority_schema(machine)?;

    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — session document authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `SessionDocumentMachine`."
    )?;
    writeln!(
        &mut out,
        "#![allow(dead_code, clippy::bool_comparison, clippy::field_reassign_with_default, clippy::nonminimal_bool, clippy::partialeq_to_none, clippy::redundant_clone, clippy::redundant_field_names)]"
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::{{collections::BTreeMap, fmt}};")?;
    writeln!(&mut out)?;

    emit_session_document_key_type(&mut out)?;
    for enum_name in ["SessionFirstTurnPhase", "SessionInitialPromptStageDecision"] {
        emit_session_document_named_string_enum(&mut out, machine, enum_name)?;
    }
    emit_session_document_variant_enum(&mut out, "SessionDocumentInput", &machine.inputs.variants)?;
    emit_session_document_variant_enum(
        &mut out,
        "SessionDocumentEffect",
        &machine.effects.variants,
    )?;
    emit_session_document_error(&mut out)?;
    emit_session_document_phase_enum(&mut out, machine)?;
    emit_session_document_state_struct(&mut out, machine)?;
    emit_session_document_transition_enum(&mut out, machine)?;
    emit_session_document_authority_impl(&mut out, machine)?;
    for helper in &machine.helpers {
        emit_session_document_helper(&mut out, helper, machine)?;
    }
    writeln!(
        &mut out,
        "impl Default for SessionDocumentMachineAuthority {{"
    )?;
    writeln!(&mut out, "    fn default() -> Self {{")?;
    writeln!(&mut out, "        Self::new()")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    Ok(out)
}

fn emit_session_document_key_type(out: &mut String) -> Result<()> {
    writeln!(
        out,
        "#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]"
    )?;
    writeln!(out, "pub struct {SESSION_DOCUMENT_KEY_TYPE}(String);")?;
    writeln!(out)?;
    writeln!(out, "impl {SESSION_DOCUMENT_KEY_TYPE} {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new(value: impl Into<String>) -> Self {{")?;
    writeln!(out, "        Self(value.into())")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_named_string_enum(
    out: &mut String,
    machine: &MachineSchema,
    name: &str,
) -> Result<()> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| format!("SessionDocumentMachine missing named type `{name}`"))?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("SessionDocumentMachine named type `{name}` must be a string enum");
    };
    let variants = variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect::<Vec<_>>();
    let default_variant = session_document_default_variant(name)?;
    if !variants.iter().any(|variant| variant == default_variant) {
        bail!("SessionDocumentMachine enum `{name}` missing default variant `{default_variant}`");
    }
    emit_string_enum(out, name, &variants, default_variant)
}

fn session_document_default_variant(name: &str) -> Result<&'static str> {
    match name {
        "SessionFirstTurnPhase" => Ok("Inactive"),
        "SessionInitialPromptStageDecision" => Ok("Clear"),
        other => bail!("unknown SessionDocumentMachine enum `{other}`"),
    }
}

fn emit_session_document_variant_enum(
    out: &mut String,
    enum_name: &str,
    variants: &[VariantSchema],
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub enum {enum_name} {{")?;
    for variant in variants {
        if variant.fields.is_empty() {
            writeln!(out, "    {},", variant.name)?;
            continue;
        }
        writeln!(out, "    {} {{", variant.name)?;
        for field in &variant.fields {
            writeln!(
                out,
                "        {}: {},",
                field.name,
                render_session_document_type_ref(&field.ty)?
            )?;
        }
        writeln!(out, "    }},")?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_error(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct SessionDocumentError {{")?;
    writeln!(out, "    op: &'static str,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl fmt::Display for SessionDocumentError {{")?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{"
    )?;
    writeln!(
        out,
        "        write!(f, \"generated session document authority rejected {{}}\", self.op)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl std::error::Error for SessionDocumentError {{}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_phase_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    let variants = machine
        .state
        .phase
        .variants
        .iter()
        .map(|variant| variant.name.as_str().to_owned())
        .collect::<Vec<_>>();
    emit_string_enum(
        out,
        machine.state.phase.name.as_str(),
        &variants,
        machine.state.init.phase.as_str(),
    )
}

fn emit_session_document_state_struct(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq, Default)]")?;
    writeln!(out, "pub struct SessionDocumentMachineState {{")?;
    writeln!(
        out,
        "    lifecycle_phase: {},",
        machine.state.phase.name.as_str()
    )?;
    for field in &machine.state.fields {
        writeln!(
            out,
            "    {}: {},",
            field.name,
            render_session_document_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_transition_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "enum SessionDocumentTransition {{")?;
    for transition in &machine.transitions {
        writeln!(out, "    {},", transition.name)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_authority_impl(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct SessionDocumentMachineAuthority {{")?;
    writeln!(out, "    state: SessionDocumentMachineState,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl SessionDocumentMachineAuthority {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new() -> Self {{")?;
    writeln!(
        out,
        "        let mut state = SessionDocumentMachineState::default();"
    )?;
    writeln!(
        out,
        "        state.lifecycle_phase = {}::{};",
        machine.state.phase.name, machine.state.init.phase
    )?;
    for init in &machine.state.init.fields {
        writeln!(
            out,
            "        state.{} = {};",
            init.field,
            render_session_document_init_expr(&init.expr, machine)?
        )?;
    }
    writeln!(out, "        Self {{ state }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn state(&self) -> &SessionDocumentMachineState {{"
    )?;
    writeln!(out, "        &self.state")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    emit_session_document_map_lookup_methods(out, machine)?;
    emit_session_document_single_transition(out)?;
    emit_session_document_apply_input(out, machine)?;
    emit_session_document_public_methods(out, machine)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_map_lookup_methods(
    out: &mut String,
    machine: &MachineSchema,
) -> Result<()> {
    for field in &machine.state.fields {
        let TypeRef::Map(key_ty, value_ty) = &field.ty else {
            continue;
        };
        session_document_assert_key(key_ty, field.name.as_str())?;
        let value_ty = render_session_document_type_ref(value_ty)?;
        let accessor = if session_document_type_is_copy(value_ty.as_str()) {
            "copied"
        } else {
            "cloned"
        };
        // Public per-key registry read so the shell can mirror the
        // machine-owned value without re-deriving it.
        writeln!(out, "    #[must_use]")?;
        writeln!(out, "    pub fn {}_for(", field.name)?;
        writeln!(out, "        &self,")?;
        writeln!(out, "        key: &{SESSION_DOCUMENT_KEY_TYPE},")?;
        writeln!(out, "    ) -> Option<{value_ty}> {{")?;
        writeln!(
            out,
            "        self.state.{}.get(key).{}()",
            field.name, accessor
        )?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
        // Fallible internal accessor used by lowered map-projection guards.
        writeln!(out, "    fn {}_value(", field.name)?;
        writeln!(out, "        &self,")?;
        writeln!(out, "        key: &{SESSION_DOCUMENT_KEY_TYPE},")?;
        writeln!(out, "    ) -> Result<{value_ty}, SessionDocumentError> {{")?;
        writeln!(
            out,
            "        self.state.{}.get(key).{}().ok_or(SessionDocumentError {{ op: \"{}\" }})",
            field.name, accessor, field.name
        )?;
        writeln!(out, "    }}")?;
        writeln!(out)?;
    }
    Ok(())
}

fn emit_session_document_single_transition(out: &mut String) -> Result<()> {
    writeln!(out, "    fn single_transition(")?;
    writeln!(out, "        matches: Vec<SessionDocumentTransition>,")?;
    writeln!(out, "        op: &'static str,")?;
    writeln!(
        out,
        "    ) -> Result<SessionDocumentTransition, SessionDocumentError> {{"
    )?;
    writeln!(out, "        let mut matches = matches.into_iter();")?;
    writeln!(out, "        let Some(first) = matches.next() else {{")?;
    writeln!(
        out,
        "            return Err(SessionDocumentError {{ op }});"
    )?;
    writeln!(out, "        }};")?;
    writeln!(out, "        if matches.next().is_some() {{")?;
    writeln!(
        out,
        "            return Err(SessionDocumentError {{ op }});"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        Ok(first)")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_apply_input(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "    fn apply_input(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        input: SessionDocumentInput,")?;
    writeln!(
        out,
        "    ) -> Result<Vec<SessionDocumentEffect>, SessionDocumentError> {{"
    )?;
    writeln!(out, "        match input {{")?;
    for input_variant in &machine.inputs.variants {
        emit_session_document_apply_input_arm(out, machine, input_variant)?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_document_apply_input_arm(
    out: &mut String,
    machine: &MachineSchema,
    input_variant: &VariantSchema,
) -> Result<()> {
    write!(
        out,
        "            SessionDocumentInput::{}",
        input_variant.name
    )?;
    if input_variant.fields.is_empty() {
        writeln!(out, " => {{")?;
    } else {
        writeln!(out, " {{")?;
        for field in &input_variant.fields {
            writeln!(out, "                {},", field.name)?;
        }
        writeln!(out, "            }} => {{")?;
    }

    let binding_types = session_document_binding_types(input_variant);
    let transitions = machine
        .transitions
        .iter()
        .filter(|transition| {
            matches!(
                &transition.on,
                TriggerMatch::Input { variant, .. } if variant.as_str() == input_variant.name.as_str()
            )
        })
        .collect::<Vec<_>>();
    writeln!(out, "                let mut matches = Vec::new();")?;
    for transition in &transitions {
        writeln!(
            out,
            "                if {} {{",
            render_session_document_transition_condition(transition, &binding_types, machine)?
        )?;
        writeln!(
            out,
            "                    matches.push(SessionDocumentTransition::{});",
            transition.name
        )?;
        writeln!(out, "                }}")?;
    }
    writeln!(
        out,
        "                let transition = Self::single_transition(matches, \"{}\")?;",
        input_variant.name
    )?;
    writeln!(out, "                match transition {{")?;
    for transition in &transitions {
        emit_session_document_transition_apply_block(out, transition, &binding_types, machine)?;
    }
    writeln!(
        out,
        "                    #[allow(unreachable_patterns)] _ => Err(SessionDocumentError {{ op: \"{}_transition\" }}),",
        input_variant.name
    )?;
    writeln!(out, "                }}")?;
    writeln!(out, "            }}")?;
    Ok(())
}

fn emit_session_document_transition_apply_block(
    out: &mut String,
    transition: &TransitionSchema,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<()> {
    writeln!(
        out,
        "                    SessionDocumentTransition::{} => {{",
        transition.name
    )?;
    for update in &transition.updates {
        writeln!(
            out,
            "                        {}",
            render_session_document_update(update, binding_types, machine)?
        )?;
    }
    writeln!(
        out,
        "                        self.state.lifecycle_phase = {}::{};",
        machine.state.phase.name, transition.to
    )?;
    if transition.emit.is_empty() {
        writeln!(out, "                        Ok(Vec::new())")?;
    } else {
        writeln!(out, "                        Ok(vec![")?;
        for effect in &transition.emit {
            writeln!(
                out,
                "                            {},",
                render_session_document_effect_emit(effect, binding_types, machine)?
            )?;
        }
        writeln!(out, "                        ])")?;
    }
    writeln!(out, "                    }}")?;
    Ok(())
}

fn emit_session_document_public_methods(out: &mut String, machine: &MachineSchema) -> Result<()> {
    for input in &machine.inputs.variants {
        let method = to_snake_case(input.name.as_str());
        writeln!(out, "    pub fn {method}(")?;
        writeln!(out, "        &mut self,")?;
        for field in &input.fields {
            writeln!(
                out,
                "        {}: {},",
                field.name,
                render_session_document_type_ref(&field.ty)?
            )?;
        }
        writeln!(
            out,
            "    ) -> Result<Vec<SessionDocumentEffect>, SessionDocumentError> {{"
        )?;
        if input.fields.is_empty() {
            writeln!(
                out,
                "        self.apply_input(SessionDocumentInput::{})",
                input.name
            )?;
        } else {
            writeln!(
                out,
                "        self.apply_input(SessionDocumentInput::{} {{",
                input.name
            )?;
            for field in &input.fields {
                writeln!(out, "            {},", field.name)?;
            }
            writeln!(out, "        }})")?;
        }
        writeln!(out, "    }}")?;
        writeln!(out)?;
    }
    Ok(())
}

fn emit_session_document_helper(
    out: &mut String,
    helper: &HelperSchema,
    machine: &MachineSchema,
) -> Result<()> {
    write!(out, "fn {}(", helper.name)?;
    for (idx, param) in helper.params.iter().enumerate() {
        if idx > 0 {
            write!(out, ", ")?;
        }
        write!(
            out,
            "{}: {}",
            param.name.as_str(),
            render_session_document_type_ref(&param.ty)?
        )?;
    }
    writeln!(
        out,
        ") -> {} {{",
        render_session_document_type_ref(&helper.returns)?
    )?;
    let binding_types = helper
        .params
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect();
    writeln!(
        out,
        "    {}",
        render_session_document_expr(&helper.body, &binding_types, machine)?
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn render_session_document_type_ref(ty: &TypeRef) -> Result<String> {
    match ty {
        TypeRef::Bool => Ok("bool".to_string()),
        TypeRef::U32 => Ok("u32".to_string()),
        TypeRef::U64 => Ok("u64".to_string()),
        TypeRef::Enum(name) => Ok(name.as_str().to_string()),
        TypeRef::Named(name) if name.as_str() == SESSION_DOCUMENT_KEY_NAMED => {
            Ok(SESSION_DOCUMENT_KEY_TYPE.to_string())
        }
        TypeRef::Option(inner) => Ok(format!(
            "Option<{}>",
            render_session_document_type_ref(inner)?
        )),
        TypeRef::Map(key, value) => Ok(format!(
            "BTreeMap<{}, {}>",
            render_session_document_type_ref(key)?,
            render_session_document_type_ref(value)?
        )),
        other => bail!("unsupported SessionDocumentMachine type `{other:?}`"),
    }
}

fn render_session_document_init_expr(expr: &Expr, machine: &MachineSchema) -> Result<String> {
    match expr {
        Expr::EmptyMap => Ok("BTreeMap::new()".to_string()),
        other => render_session_document_expr(other, &std::collections::BTreeMap::new(), machine),
    }
}

fn render_session_document_transition_condition(
    transition: &TransitionSchema,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    let mut conditions = Vec::new();
    if transition.from.len() == 1 {
        conditions.push(format!(
            "self.state.lifecycle_phase == {}::{}",
            machine.state.phase.name, transition.from[0]
        ));
    } else if !transition.from.is_empty() {
        conditions.push(format!(
            "matches!(self.state.lifecycle_phase, {})",
            transition
                .from
                .iter()
                .map(|phase| format!("{}::{phase}", machine.state.phase.name))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    for guard in &transition.guards {
        conditions.push(render_session_document_expr(
            &guard.expr,
            binding_types,
            machine,
        )?);
    }
    if conditions.is_empty() {
        Ok("true".to_string())
    } else {
        Ok(conditions
            .into_iter()
            .map(|condition| format!("({condition})"))
            .collect::<Vec<_>>()
            .join(" && "))
    }
}

fn render_session_document_update(
    update: &Update,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match update {
        Update::Assign { field, expr } => Ok(format!(
            "self.state.{field} = {};",
            render_session_document_owned_expr(expr, binding_types, machine)?
        )),
        Update::MapInsert { field, key, value } => Ok(format!(
            "self.state.{field}.insert({}, {});",
            render_session_document_owned_expr(key, binding_types, machine)?,
            render_session_document_owned_expr(value, binding_types, machine)?
        )),
        other => bail!("unsupported SessionDocumentMachine update `{other:?}`"),
    }
}

fn render_session_document_effect_emit(
    effect: &EffectEmit,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    let variant = machine
        .effects
        .variant_named(effect.variant.as_str())
        .with_context(|| {
            format!(
                "SessionDocumentMachine effect `{}` missing from schema",
                effect.variant
            )
        })?;
    if effect.fields.is_empty() {
        return Ok(format!("SessionDocumentEffect::{}", effect.variant));
    }
    let mut rendered = format!("SessionDocumentEffect::{} {{", effect.variant);
    for (idx, (field, expr)) in effect.fields.iter().enumerate() {
        if idx > 0 {
            rendered.push(' ');
        }
        let field_schema = variant.field_named(field.as_str()).with_context(|| {
            format!(
                "SessionDocumentMachine effect `{}` missing field `{}`",
                effect.variant, field
            )
        })?;
        write!(
            &mut rendered,
            " {field}: {},",
            render_session_document_effect_field_expr(
                expr,
                &field_schema.ty,
                binding_types,
                machine
            )?
        )?;
    }
    rendered.push_str(" }");
    Ok(rendered)
}

fn render_session_document_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match expr {
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::CurrentPhase => Ok("self.state.lifecycle_phase".to_string()),
        Expr::Phase(phase) => Ok(format!("{}::{phase}", machine.state.phase.name)),
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!(
            "Some({})",
            render_session_document_expr(inner, binding_types, machine)?
        )),
        Expr::Not(inner) => Ok(format!(
            "!({})",
            render_session_document_expr(inner, binding_types, machine)?
        )),
        Expr::And(items) => {
            render_session_document_expr_joined(items, " && ", binding_types, machine)
        }
        Expr::Or(items) => {
            render_session_document_expr_joined(items, " || ", binding_types, machine)
        }
        Expr::Eq(left, right) => Ok(format!(
            "{} == {}",
            render_session_document_expr(left, binding_types, machine)?,
            render_session_document_expr(right, binding_types, machine)?
        )),
        Expr::Neq(left, right) => Ok(format!(
            "{} != {}",
            render_session_document_expr(left, binding_types, machine)?,
            render_session_document_expr(right, binding_types, machine)?
        )),
        Expr::Gt(left, right) => Ok(format!(
            "{} > {}",
            render_session_document_expr(left, binding_types, machine)?,
            render_session_document_expr(right, binding_types, machine)?
        )),
        Expr::MapContainsKey { map, key } => Ok(format!(
            "{}.contains_key({})",
            render_session_document_collection_expr(map)?,
            render_session_document_borrowed_key_expr(key, binding_types)?
        )),
        Expr::MapGet { map, key } => {
            if let Some(rendered) =
                render_session_document_optional_map_value_get(map, key, binding_types)?
            {
                return Ok(rendered);
            }
            let Expr::Field(field) = map.as_ref() else {
                bail!("SessionDocumentMachine MapGet must read a state field: {map:?}");
            };
            Ok(format!(
                "self.{}_value({})?",
                field,
                render_session_document_borrowed_key_expr(key, binding_types)?
            ))
        }
        Expr::Call { helper, args } => {
            let rendered_args = args
                .iter()
                .map(|arg| render_session_document_expr(arg, binding_types, machine))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("{helper}({rendered_args})"))
        }
        other => bail!("unsupported SessionDocumentMachine expression `{other:?}`"),
    }
}

/// Lower the DSL `get_cloned(k).get("value")` projection — desugared by the
/// macro to `IfElse { MapContainsKey, Some(MapGet), None }` — into the
/// fallible `self.<field>_value(key)?` map lookup. Mirrors the approval
/// emitter's `render_approval_optional_map_value_get`.
fn render_session_document_optional_map_value_get(
    map: &Expr,
    key: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
) -> Result<Option<String>> {
    let Expr::String(projected_field) = key else {
        return Ok(None);
    };
    if projected_field != "value" {
        return Ok(None);
    }
    let Expr::IfElse {
        condition,
        then_expr,
        else_expr,
    } = map
    else {
        return Ok(None);
    };
    if !matches!(else_expr.as_ref(), Expr::None) {
        return Ok(None);
    }
    let Expr::MapContainsKey {
        map: condition_map,
        key: condition_key,
    } = condition.as_ref()
    else {
        return Ok(None);
    };
    let Expr::Some(then_inner) = then_expr.as_ref() else {
        return Ok(None);
    };
    let Expr::MapGet {
        map: then_map,
        key: then_key,
    } = then_inner.as_ref()
    else {
        return Ok(None);
    };
    if condition_map != then_map || condition_key != then_key {
        bail!("SessionDocumentMachine optional map projection has incoherent key source");
    }
    let Expr::Field(field) = then_map.as_ref() else {
        bail!("SessionDocumentMachine optional map projection must read a state field");
    };
    Ok(Some(format!(
        "self.{}_value({})?",
        field,
        render_session_document_borrowed_key_expr(then_key, binding_types)?
    )))
}

fn render_session_document_expr_joined(
    items: &[Expr],
    separator: &str,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    if items.is_empty() {
        bail!("SessionDocumentMachine expression cannot join an empty list");
    }
    let rendered = items
        .iter()
        .map(|expr| {
            Ok(format!(
                "({})",
                render_session_document_expr(expr, binding_types, machine)?
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(separator))
}

fn render_session_document_collection_expr(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        other => {
            bail!("SessionDocumentMachine collection expression must be a field: {other:?}")
        }
    }
}

fn render_session_document_borrowed_key_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
) -> Result<String> {
    match expr {
        Expr::Binding(binding)
            if matches!(
                binding_types.get(binding),
                Some(TypeRef::Named(name)) if name.as_str() == SESSION_DOCUMENT_KEY_NAMED
            ) =>
        {
            Ok(format!("&{binding}"))
        }
        other => bail!("SessionDocumentMachine expected borrowed key expression: {other:?}"),
    }
}

fn render_session_document_owned_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match expr {
        Expr::Binding(binding)
            if matches!(
                binding_types.get(binding),
                Some(TypeRef::Named(name)) if name.as_str() == SESSION_DOCUMENT_KEY_NAMED
            ) =>
        {
            Ok(format!("{binding}.clone()"))
        }
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!(
            "Some({})",
            render_session_document_owned_expr(inner, binding_types, machine)?
        )),
        other => render_session_document_expr(other, binding_types, machine),
    }
}

fn render_session_document_effect_field_expr(
    expr: &Expr,
    ty: &TypeRef,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    if matches!(ty, TypeRef::Named(name) if name.as_str() == SESSION_DOCUMENT_KEY_NAMED) {
        render_session_document_owned_expr(expr, binding_types, machine)
    } else {
        render_session_document_expr(expr, binding_types, machine)
    }
}

fn session_document_binding_types(
    variant: &VariantSchema,
) -> std::collections::BTreeMap<String, TypeRef> {
    variant
        .fields
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect()
}

fn session_document_type_is_copy(type_name: &str) -> bool {
    matches!(
        type_name,
        "bool" | "u32" | "u64" | "SessionFirstTurnPhase" | "SessionInitialPromptStageDecision"
    )
}

fn session_document_assert_key(key_ty: &TypeRef, field_name: &str) -> Result<()> {
    if matches!(key_ty, TypeRef::Named(name) if name.as_str() == SESSION_DOCUMENT_KEY_NAMED) {
        Ok(())
    } else {
        bail!(
            "SessionDocumentMachine map field `{field_name}` must use `{SESSION_DOCUMENT_KEY_NAMED}` keys"
        )
    }
}

fn validate_session_document_authority_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate SessionDocumentMachine schema")?;
    if machine.machine.as_str() != "SessionDocumentMachine" {
        bail!(
            "session document generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "MarkSessionInitialTurnPending",
        "StartSessionInitialTurn",
        "StageSessionInitialPrompt",
        "StageSessionToolResults",
        "ConsumeSessionDeferredInputs",
        "RestoreSessionConsumedInputs",
        "RecoverSessionFirstTurnPhase",
        "ResolveSessionFirstTurnOverridesAllowed",
    ] {
        machine
            .inputs
            .variant_named(required)
            .with_context(|| format!("SessionDocumentMachine missing input `{required}`"))?;
    }
    for required in [
        "SessionFirstTurnPhaseResolved",
        "SessionFirstTurnOverridesResolved",
        "SessionInitialPromptStageResolved",
        "SessionToolResultsStageResolved",
        "SessionConsumedInputsRestoreResolved",
        "SessionFirstTurnPhaseRecovered",
    ] {
        machine
            .effects
            .variant_named(required)
            .with_context(|| format!("SessionDocumentMachine missing effect `{required}`"))?;
    }
    for required in ["SessionFirstTurnPhase", "SessionInitialPromptStageDecision"] {
        let binding = machine
            .named_types
            .iter()
            .find(|binding| binding.name.as_str() == required)
            .with_context(|| format!("SessionDocumentMachine missing named type `{required}`"))?;
        if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
            bail!("SessionDocumentMachine named type `{required}` must be a string enum");
        }
    }
    Ok(())
}

pub fn render_pending_continuation_admission(machine: &MachineSchema) -> Result<String> {
    generate_pending_continuation_admission(machine)
}

pub fn render_session_system_context_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_system_context_authority(machine)
}

pub fn render_session_durable_config_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_durable_config_authority(machine)
}

pub fn render_session_realtime_transcript_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_realtime_transcript_authority(machine)
}

fn generate_session_system_context_authority(machine: &MachineSchema) -> Result<String> {
    validate_session_system_context_authority_schema(machine)?;
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated - session system-context authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `SessionSystemContextAuthorityMachine` transitions."
    )?;
    writeln!(&mut out, "#![allow(clippy::bool_comparison)]")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::{{collections::BTreeSet, fmt}};")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use crate::{{")?;
    writeln!(
        &mut out,
        "    service::{{AppendSystemContextRequest, AppendSystemContextStatus}},"
    )?;
    writeln!(&mut out, "    session::{{")?;
    writeln!(
        &mut out,
        "        PendingSystemContextAppend, SYSTEM_CONTEXT_SEPARATOR, SeenSystemContextKey,"
    )?;
    writeln!(
        &mut out,
        "        SeenSystemContextState, SessionSystemContextState, SystemContextStageError,"
    )?;
    writeln!(&mut out, "    }},")?;
    writeln!(&mut out, "    time_compat::SystemTime,")?;
    writeln!(&mut out, "}};")?;
    writeln!(&mut out)?;

    let cfg = SessionAuthorityRenderConfig {
        schema_name: "SessionSystemContextAuthorityMachine",
        input_enum: "SessionSystemContextAuthorityInput",
        effect_enum: "SessionSystemContextAuthorityEffect",
        phase_enum: "SessionSystemContextAuthorityPhase",
        state_struct: "SessionSystemContextAuthorityMachineState",
        authority_struct: "SessionSystemContextAuthorityMachineAuthority",
        error_struct: "SessionSystemContextAuthorityError",
        error_message: "generated session system-context authority rejected",
    };
    emit_session_named_string_enum(
        &mut out,
        machine,
        "SystemContextAppendDecision",
        "Staged",
        cfg.schema_name,
    )?;
    emit_session_authority_core(&mut out, machine, cfg)?;
    emit_session_system_context_domain_helpers(&mut out)?;
    Ok(out)
}

fn generate_session_durable_config_authority(machine: &MachineSchema) -> Result<String> {
    validate_session_durable_config_authority_schema(machine)?;
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated - session durable-config authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `SessionDurableConfigAuthorityMachine` transitions."
    )?;
    writeln!(&mut out, "#![allow(clippy::bool_comparison)]")?;
    writeln!(&mut out, "#![allow(clippy::too_many_arguments)]")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::fmt;")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "use crate::{{CallTimeoutOverride, Provider, SessionBuildState, SessionMetadata, SessionTooling, ToolCategoryOverride}};"
    )?;
    writeln!(&mut out)?;

    let cfg = SessionAuthorityRenderConfig {
        schema_name: "SessionDurableConfigAuthorityMachine",
        input_enum: "SessionDurableConfigAuthorityInput",
        effect_enum: "SessionDurableConfigAuthorityEffect",
        phase_enum: "SessionDurableConfigAuthorityPhase",
        state_struct: "SessionDurableConfigAuthorityMachineState",
        authority_struct: "SessionDurableConfigAuthorityMachineAuthority",
        error_struct: "SessionDurableConfigAuthorityError",
        error_message: "generated session durable-config authority rejected",
    };
    emit_session_named_string_enum(
        &mut out,
        machine,
        "SessionDurableProviderKind",
        "Other",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "SessionToolCategoryOverrideKind",
        "Inherit",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "SessionCallTimeoutOverrideKind",
        "Inherit",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "SessionSystemPromptSource",
        "DirectMutation",
        cfg.schema_name,
    )?;
    emit_session_authority_core(&mut out, machine, cfg)?;
    emit_session_durable_config_domain_helpers(&mut out);
    Ok(out)
}

fn generate_session_realtime_transcript_authority(machine: &MachineSchema) -> Result<String> {
    validate_session_realtime_transcript_authority_schema(machine)?;
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated - session realtime transcript authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `SessionRealtimeTranscriptAuthorityMachine` transitions."
    )?;
    writeln!(&mut out, "#![allow(clippy::bool_comparison)]")?;
    writeln!(&mut out, "#![allow(clippy::too_many_arguments)]")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::collections::{{BTreeMap, BTreeSet}};")?;
    writeln!(&mut out, "use std::fmt;")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use crate::{{")?;
    writeln!(&mut out, "    realtime_transcript::{{")?;
    writeln!(
        &mut out,
        "        RealtimeTranscriptApplyOutcome, RealtimeTranscriptEvent,"
    )?;
    writeln!(
        &mut out,
        "        RealtimeTranscriptMaterializedMessage, RealtimeTranscriptRole, TranscriptLane,"
    )?;
    writeln!(&mut out, "    }},")?;
    writeln!(&mut out, "    types::{{")?;
    writeln!(
        &mut out,
        "        AssistantBlock, BlockAssistantMessage, ContentInput, Message, StopReason,"
    )?;
    writeln!(&mut out, "        TranscriptSource, Usage, UserMessage,")?;
    writeln!(&mut out, "    }},")?;
    writeln!(&mut out, "}};")?;
    writeln!(&mut out)?;

    let cfg = SessionAuthorityRenderConfig {
        schema_name: "SessionRealtimeTranscriptAuthorityMachine",
        input_enum: "SessionRealtimeTranscriptAuthorityInput",
        effect_enum: "SessionRealtimeTranscriptAuthorityEffect",
        phase_enum: "SessionRealtimeTranscriptAuthorityPhase",
        state_struct: "SessionRealtimeTranscriptAuthorityMachineState",
        authority_struct: "SessionRealtimeTranscriptAuthorityMachineAuthority",
        error_struct: "SessionRealtimeTranscriptAuthorityError",
        error_message: "generated session realtime transcript authority rejected",
    };
    emit_session_named_string_enum(
        &mut out,
        machine,
        "RealtimeTranscriptEventKind",
        "ItemObserved",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "RealtimeTranscriptRoleKind",
        "User",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "RealtimeTranscriptLaneKind",
        "Display",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "RealtimeTranscriptStopReasonKind",
        "Other",
        cfg.schema_name,
    )?;
    emit_session_named_string_enum(
        &mut out,
        machine,
        "RealtimeTranscriptMaterializeDecision",
        "Wait",
        cfg.schema_name,
    )?;
    emit_session_realtime_transcript_event_kind_impl(&mut out)?;
    emit_session_authority_core(&mut out, machine, cfg)?;
    emit_session_realtime_transcript_domain_helpers(&mut out);
    Ok(out)
}

#[derive(Clone, Copy)]
struct SessionAuthorityRenderConfig<'a> {
    schema_name: &'a str,
    input_enum: &'a str,
    effect_enum: &'a str,
    phase_enum: &'a str,
    state_struct: &'a str,
    authority_struct: &'a str,
    error_struct: &'a str,
    error_message: &'a str,
}

fn emit_session_named_string_enum(
    out: &mut String,
    machine: &MachineSchema,
    name: &str,
    default_variant: &str,
    schema_name: &str,
) -> Result<()> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| format!("{schema_name} missing named type `{name}`"))?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("{schema_name} named type `{name}` must be a string enum");
    };
    let variants = variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect::<Vec<_>>();
    emit_string_enum(out, name, &variants, default_variant)
}

fn emit_session_authority_core(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    emit_session_phase_enum(out, machine, cfg)?;
    emit_session_state_struct(out, machine, cfg)?;
    emit_session_input_enum(out, machine, cfg)?;
    emit_session_effect_enum(out, machine, cfg)?;
    emit_session_error(out, cfg)?;
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub struct {} {{", cfg.authority_struct)?;
    writeln!(out, "    state: {},", cfg.state_struct)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    emit_session_authority_impl(out, machine, cfg)?;
    writeln!(out, "impl Default for {} {{", cfg.authority_struct)?;
    writeln!(out, "    fn default() -> Self {{")?;
    writeln!(out, "        Self::new()")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_phase_enum(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    let variants = machine
        .state
        .phase
        .variants
        .iter()
        .map(|variant| variant.name.as_str().to_owned())
        .collect::<Vec<_>>();
    emit_string_enum(
        out,
        cfg.phase_enum,
        &variants,
        machine.state.init.phase.as_str(),
    )
}

fn emit_session_state_struct(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]")?;
    writeln!(out, "pub struct {} {{", cfg.state_struct)?;
    writeln!(out, "    lifecycle_phase: {},", cfg.phase_enum)?;
    for field in &machine.state.fields {
        writeln!(
            out,
            "    {}: {},",
            field.name,
            render_session_type_ref(&field.ty, cfg)?
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_input_enum(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub enum {} {{", cfg.input_enum)?;
    for variant in &machine.inputs.variants {
        emit_session_variant(out, variant, cfg)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_effect_enum(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub enum {} {{", cfg.effect_enum)?;
    for variant in &machine.effects.variants {
        emit_session_variant(out, variant, cfg)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_variant(
    out: &mut String,
    variant: &VariantSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    if variant.fields.is_empty() {
        writeln!(out, "    {},", variant.name)?;
        return Ok(());
    }
    writeln!(out, "    {} {{", variant.name)?;
    for field in &variant.fields {
        writeln!(
            out,
            "        {}: {},",
            field.name,
            render_session_type_ref(&field.ty, cfg)?
        )?;
    }
    writeln!(out, "    }},")?;
    Ok(())
}

fn emit_session_error(out: &mut String, cfg: SessionAuthorityRenderConfig<'_>) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct {} {{", cfg.error_struct)?;
    writeln!(out, "    op: &'static str,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl fmt::Display for {} {{", cfg.error_struct)?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{"
    )?;
    writeln!(
        out,
        "        write!(f, \"{} {{}}\", self.op)",
        cfg.error_message
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl std::error::Error for {} {{}}", cfg.error_struct)?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_authority_impl(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(out, "impl {} {{", cfg.authority_struct)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new() -> Self {{")?;
    writeln!(out, "        let state = {} {{", cfg.state_struct)?;
    writeln!(
        out,
        "            lifecycle_phase: {}::{},",
        cfg.phase_enum, machine.state.init.phase
    )?;
    let all_fields_initialized = machine.state.fields.iter().all(|field| {
        machine
            .state
            .init
            .fields
            .iter()
            .any(|init| init.field == field.name)
    });
    for field in &machine.state.fields {
        if let Some(init) = machine
            .state
            .init
            .fields
            .iter()
            .find(|init| init.field == field.name)
        {
            writeln!(
                out,
                "            {}: {},",
                field.name,
                render_session_expr(&init.expr, cfg)?
            )?;
        }
    }
    if !all_fields_initialized {
        writeln!(out, "            ..{}::default()", cfg.state_struct)?;
    }
    writeln!(out, "        }};")?;
    writeln!(out, "        Self {{ state }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn state(&self) -> &{} {{", cfg.state_struct)?;
    writeln!(out, "        &self.state")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    emit_session_apply_input(out, machine, cfg)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    for helper in &machine.helpers {
        emit_session_helper(out, helper, cfg)?;
    }
    Ok(())
}

fn emit_session_apply_input(
    out: &mut String,
    machine: &MachineSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(out, "    fn apply_input(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        input: {},", cfg.input_enum)?;
    writeln!(
        out,
        "    ) -> Result<Vec<{}>, {}> {{",
        cfg.effect_enum, cfg.error_struct
    )?;
    writeln!(out, "        match input {{")?;
    for input_variant in &machine.inputs.variants {
        emit_session_apply_input_arm(out, machine, input_variant, cfg)?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_apply_input_arm(
    out: &mut String,
    machine: &MachineSchema,
    input_variant: &VariantSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    write!(
        out,
        "            {}::{}",
        cfg.input_enum, input_variant.name
    )?;
    if input_variant.fields.is_empty() {
        writeln!(out, " => {{")?;
    } else {
        writeln!(out, " {{")?;
        for field in &input_variant.fields {
            writeln!(out, "                {},", field.name)?;
        }
        writeln!(out, "            }} => {{")?;
        if input_variant.fields.len() == 1 {
            writeln!(
                out,
                "                let _ = &{};",
                input_variant.fields[0].name
            )?;
        } else {
            writeln!(
                out,
                "                let _ = ({});",
                input_variant
                    .fields
                    .iter()
                    .map(|field| format!("&{}", field.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
    }
    for transition in machine.transitions.iter().filter(|transition| {
        matches!(
            &transition.on,
            TriggerMatch::Input { variant, .. } if variant.as_str() == input_variant.name.as_str()
        )
    }) {
        emit_session_transition_block(out, transition, cfg)?;
    }
    writeln!(
        out,
        "                Err({} {{ op: \"{}\" }})",
        cfg.error_struct, input_variant.name
    )?;
    writeln!(out, "            }}")?;
    Ok(())
}

fn emit_session_transition_block(
    out: &mut String,
    transition: &TransitionSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    writeln!(
        out,
        "                if {} {{",
        render_session_transition_condition(transition, cfg)?
    )?;
    for update in &transition.updates {
        writeln!(
            out,
            "                    {}",
            render_session_update(update, cfg)?
        )?;
    }
    writeln!(
        out,
        "                    self.state.lifecycle_phase = {}::{};",
        cfg.phase_enum, transition.to
    )?;
    if transition.emit.is_empty() {
        writeln!(out, "                    return Ok(Vec::new());")?;
    } else {
        writeln!(out, "                    return Ok(vec![")?;
        for effect in &transition.emit {
            writeln!(
                out,
                "                        {},",
                render_session_effect_emit(effect, cfg)?
            )?;
        }
        writeln!(out, "                    ]);")?;
    }
    writeln!(out, "                }}")?;
    Ok(())
}

fn render_session_transition_condition(
    transition: &TransitionSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<String> {
    let mut conditions = Vec::new();
    if transition.from.len() == 1 {
        conditions.push(format!(
            "self.state.lifecycle_phase == {}::{}",
            cfg.phase_enum, transition.from[0]
        ));
    } else if !transition.from.is_empty() {
        conditions.push(format!(
            "matches!(self.state.lifecycle_phase, {})",
            transition
                .from
                .iter()
                .map(|phase| format!("{}::{phase}", cfg.phase_enum))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    for guard in &transition.guards {
        conditions.push(render_session_expr(&guard.expr, cfg)?);
    }
    if conditions.is_empty() {
        Ok("true".to_string())
    } else if conditions.len() == 1 {
        Ok(conditions.remove(0))
    } else {
        Ok(conditions
            .into_iter()
            .map(|condition| format!("({condition})"))
            .collect::<Vec<_>>()
            .join(" && "))
    }
}

fn render_session_update(update: &Update, cfg: SessionAuthorityRenderConfig<'_>) -> Result<String> {
    match update {
        Update::Assign { field, expr } => Ok(format!(
            "self.state.{field} = {};",
            render_session_expr(expr, cfg)?
        )),
        other => bail!(
            "{} generator does not support update `{other:?}`",
            cfg.schema_name
        ),
    }
}

fn render_session_effect_emit(
    effect: &EffectEmit,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<String> {
    if effect.fields.is_empty() {
        return Ok(format!("{}::{}", cfg.effect_enum, effect.variant));
    }
    let mut rendered = format!("{}::{} {{", cfg.effect_enum, effect.variant);
    for (idx, (field, expr)) in effect.fields.iter().enumerate() {
        if idx > 0 {
            rendered.push(' ');
        }
        let rendered_expr = render_session_expr(expr, cfg)?;
        if rendered_expr == field.as_str() {
            write!(&mut rendered, " {field},")?;
            continue;
        }
        write!(&mut rendered, " {field}: {rendered_expr},")?;
    }
    rendered.push_str(" }");
    Ok(rendered)
}

fn emit_session_helper(
    out: &mut String,
    helper: &HelperSchema,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<()> {
    if helper.returns != TypeRef::Bool {
        bail!(
            "{} helper `{}` must return bool",
            cfg.schema_name,
            helper.name
        );
    }
    write!(out, "fn {}(", helper.name)?;
    for (idx, param) in helper.params.iter().enumerate() {
        if idx > 0 {
            write!(out, ", ")?;
        }
        write!(
            out,
            "{}: {}",
            param.name.as_str(),
            render_session_type_ref(&param.ty, cfg)?
        )?;
    }
    writeln!(out, ") -> bool {{")?;
    writeln!(out, "    {}", render_session_expr(&helper.body, cfg)?)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn render_session_type_ref(ty: &TypeRef, cfg: SessionAuthorityRenderConfig<'_>) -> Result<String> {
    match ty {
        TypeRef::Bool => Ok("bool".to_string()),
        TypeRef::U64 => Ok("u64".to_string()),
        TypeRef::Enum(enum_name) => Ok(enum_name.as_str().to_string()),
        TypeRef::Option(inner) => Ok(format!("Option<{}>", render_session_type_ref(inner, cfg)?)),
        other => bail!(
            "{} generator does not support type `{other:?}`",
            cfg.schema_name
        ),
    }
}

fn render_session_expr(expr: &Expr, cfg: SessionAuthorityRenderConfig<'_>) -> Result<String> {
    match expr {
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        Expr::CurrentPhase => Ok("self.state.lifecycle_phase".to_string()),
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::Phase(phase) => Ok(format!("{}::{phase}", cfg.phase_enum)),
        Expr::Variant(variant) => Ok(variant.clone()),
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!("Some({})", render_session_expr(inner, cfg)?)),
        Expr::Not(inner) => Ok(format!("!({})", render_session_expr(inner, cfg)?)),
        Expr::Eq(left, right) => Ok(format!(
            "{} == {}",
            render_session_expr(left, cfg)?,
            render_session_expr(right, cfg)?
        )),
        Expr::Neq(left, right) => Ok(format!(
            "{} != {}",
            render_session_expr(left, cfg)?,
            render_session_expr(right, cfg)?
        )),
        Expr::Gt(left, right) => Ok(format!(
            "{} > {}",
            render_session_expr(left, cfg)?,
            render_session_expr(right, cfg)?
        )),
        Expr::Or(items) => render_session_expr_joined(items, " || ", cfg),
        Expr::And(items) => render_session_expr_joined(items, " && ", cfg),
        Expr::Call { helper, args } => {
            let rendered_args = args
                .iter()
                .map(|arg| render_session_expr(arg, cfg))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("{helper}({rendered_args})"))
        }
        other => bail!(
            "{} generator does not support expression `{other:?}`",
            cfg.schema_name
        ),
    }
}

fn render_session_expr_joined(
    items: &[Expr],
    separator: &str,
    cfg: SessionAuthorityRenderConfig<'_>,
) -> Result<String> {
    if items.is_empty() {
        bail!("{} expression cannot be empty", cfg.schema_name);
    }
    let rendered = items
        .iter()
        .map(|expr| Ok(format!("({})", render_session_expr(expr, cfg)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(separator))
}

fn emit_session_realtime_transcript_event_kind_impl(out: &mut String) -> Result<()> {
    writeln!(
        out,
        r"impl From<&RealtimeTranscriptEvent> for RealtimeTranscriptEventKind {{
    fn from(event: &RealtimeTranscriptEvent) -> Self {{
        match event {{
            RealtimeTranscriptEvent::ItemObserved {{ .. }} => Self::ItemObserved,
            RealtimeTranscriptEvent::ItemSkipped {{ .. }} => Self::ItemSkipped,
            RealtimeTranscriptEvent::UserTranscriptFinal {{ .. }} => Self::UserTranscriptFinal,
            RealtimeTranscriptEvent::AssistantTextDelta {{ .. }} => Self::AssistantTextDelta,
            RealtimeTranscriptEvent::AssistantTranscriptDelta {{ .. }} => {{
                Self::AssistantTranscriptDelta
            }}
            RealtimeTranscriptEvent::AssistantTranscriptTruncated {{ .. }} => {{
                Self::AssistantTranscriptTruncated
            }}
            RealtimeTranscriptEvent::AssistantTranscriptFinalText {{ .. }} => {{
                Self::AssistantTranscriptFinalText
            }}
            RealtimeTranscriptEvent::AssistantTurnCompleted {{ .. }} => {{
                Self::AssistantTurnCompleted
            }}
            RealtimeTranscriptEvent::AssistantTurnInterrupted {{ .. }} => {{
                Self::AssistantTurnInterrupted
            }}
        }}
    }}
}}"
    )?;
    writeln!(out)?;
    Ok(())
}

fn emit_session_realtime_transcript_domain_helpers(out: &mut String) {
    out.push_str(include_str!(
        "protocol_codegen/session_realtime_transcript_authority_domain.rs.inc"
    ));
    out.push('\n');
}

fn emit_session_system_context_domain_helpers(out: &mut String) -> Result<()> {
    writeln!(
        out,
        "fn system_context_authority_effect(input: SessionSystemContextAuthorityInput, expected: &'static str) -> Result<Vec<SessionSystemContextAuthorityEffect>, SessionSystemContextAuthorityError> {{"
    )?;
    writeln!(
        out,
        "    let mut authority = SessionSystemContextAuthorityMachineAuthority::new();"
    )?;
    writeln!(out, "    let effects = authority.apply_input(input)?;")?;
    writeln!(out, "    if effects.iter().any(|effect| matches!(")?;
    writeln!(out, "        (expected, effect),")?;
    writeln!(
        out,
        "        (\"PendingApplyResolved\", SessionSystemContextAuthorityEffect::PendingApplyResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"ActiveTurnPendingDiscardResolved\", SessionSystemContextAuthorityEffect::ActiveTurnPendingDiscardResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"ActiveTurnKeyedDiscardResolved\", SessionSystemContextAuthorityEffect::ActiveTurnKeyedDiscardResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"AppliedBlocksRecordResolved\", SessionSystemContextAuthorityEffect::AppliedBlocksRecordResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"TransientRuntimeSteerDiscardResolved\", SessionSystemContextAuthorityEffect::TransientRuntimeSteerDiscardResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"RuntimeSteerPromptCleanupResolved\", SessionSystemContextAuthorityEffect::RuntimeSteerPromptCleanupResolved {{ .. }})"
    )?;
    writeln!(
        out,
        "            | (\"SnapshotRestoreAuthorized\", SessionSystemContextAuthorityEffect::SnapshotRestoreAuthorized)"
    )?;
    writeln!(
        out,
        "            | (\"AppendResolved\", SessionSystemContextAuthorityEffect::AppendResolved {{ .. }})"
    )?;
    writeln!(out, "    )) {{")?;
    writeln!(out, "        Ok(effects)")?;
    writeln!(out, "    }} else {{")?;
    writeln!(
        out,
        "        Err(SessionSystemContextAuthorityError {{ op: expected }})"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    emit_session_system_context_static_helpers(out)
}

fn emit_session_system_context_static_helpers(out: &mut String) -> Result<()> {
    let source = r#"
pub fn resolve_append(
    trimmed_text_byte_count: u64,
    idempotency_key_present: bool,
    existing_key_matches: bool,
    existing_key_conflicts: bool,
    active_turn_scoped: bool,
) -> Result<SystemContextAppendResolution, SessionSystemContextAuthorityError> {
    let effects = system_context_authority_effect(
        SessionSystemContextAuthorityInput::ResolveAppend {
            trimmed_text_byte_count,
            idempotency_key_present,
            existing_key_matches,
            existing_key_conflicts,
            active_turn_scoped,
        },
        "AppendResolved",
    )?;
    for effect in effects {
        if let SessionSystemContextAuthorityEffect::AppendResolved {
            decision,
            active_turn_scoped,
        } = effect
        {
            return Ok(SystemContextAppendResolution {
                decision,
                active_turn_scoped,
            });
        }
    }
    Err(SessionSystemContextAuthorityError {
        op: "AppendResolved",
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemContextAppendResolution {
    pub decision: SystemContextAppendDecision,
    pub active_turn_scoped: bool,
}

pub fn restore_system_context_state(
    state: SessionSystemContextState,
) -> Result<SessionSystemContextState, SessionSystemContextAuthorityError> {
    let active_keys_have_known_pending_or_seen = state.active_turn_pending_keys.iter().all(|key| {
        state.seen.contains_key(key)
            || state
                .pending
                .iter()
                .any(|append| append.idempotency_key.as_ref() == Some(key))
    });
    let seen_keys_match_known_appends = state.seen.iter().all(|(key, seen)| {
        state
            .pending
            .iter()
            .chain(state.applied.iter())
            .any(|append| {
                append.idempotency_key.as_ref() == Some(key)
                    && seen.text == append.text
                    && seen.source.as_deref() == append.source.as_deref()
            })
    });
    system_context_authority_effect(
        SessionSystemContextAuthorityInput::RestoreSystemContextState {
            pending_count: usize_to_u64(state.pending.len()),
            applied_count: usize_to_u64(state.applied.len()),
            seen_count: usize_to_u64(state.seen.len()),
            active_turn_pending_key_count: usize_to_u64(state.active_turn_pending_keys.len()),
            active_keys_have_known_pending_or_seen,
            seen_keys_match_known_appends,
        },
        "SnapshotRestoreAuthorized",
    )?;
    Ok(state)
}

pub fn stage_append(
    state: &mut SessionSystemContextState,
    req: &AppendSystemContextRequest,
    accepted_at: SystemTime,
    active_turn_scoped: bool,
) -> Result<AppendSystemContextStatus, SystemContextStageError> {
    let text = req.text.trim();
    let existing = req
        .idempotency_key
        .as_ref()
        .and_then(|key| state.seen.get(key));
    let existing_key_matches = existing.is_some_and(|existing| {
        existing.text == text && existing.source.as_deref() == req.source.as_deref()
    });
    let existing_key_conflicts = existing.is_some() && !existing_key_matches;
    let resolution = resolve_append(
        usize_to_u64(text.len()),
        req.idempotency_key.is_some(),
        existing_key_matches,
        existing_key_conflicts,
        active_turn_scoped,
    )
    .map_err(|err| SystemContextStageError::InvalidRequest(err.to_string()))?;
    let active_turn_scoped = resolution.active_turn_scoped;

    match resolution.decision {
        SystemContextAppendDecision::RejectEmpty => {
            return Err(SystemContextStageError::InvalidRequest(
                "system context text must not be empty".to_string(),
            ));
        }
        SystemContextAppendDecision::RejectConflict => {
            let Some(key) = req.idempotency_key.as_ref() else {
                return Err(SystemContextStageError::InvalidRequest(
                    "generated system-context authority rejected append without a key".to_string(),
                ));
            };
            let Some(existing) = existing else {
                return Err(SystemContextStageError::InvalidRequest(
                    "generated system-context authority rejected append without a conflict"
                        .to_string(),
                ));
            };
            return Err(SystemContextStageError::Conflict {
                key: key.clone(),
                existing_text: existing.text.clone(),
                existing_source: existing.source.clone(),
            });
        }
        SystemContextAppendDecision::Duplicate => {
            return Ok(AppendSystemContextStatus::Duplicate);
        }
        SystemContextAppendDecision::Staged => {}
    }

    let append = PendingSystemContextAppend {
        text: text.to_string(),
        source: req.source.clone(),
        idempotency_key: req.idempotency_key.clone(),
        accepted_at,
    };
    if let Some(key) = req.idempotency_key.as_ref() {
        state.seen.insert(
            key.clone(),
            SeenSystemContextKey {
                text: append.text.clone(),
                source: append.source.clone(),
                state: SeenSystemContextState::Pending,
            },
        );
    }
    if active_turn_scoped && let Some(key) = req.idempotency_key.as_ref() {
        state.active_turn_pending_keys.insert(key.clone());
    }
    state.pending.push(append);
    Ok(AppendSystemContextStatus::Staged)
}

pub fn mark_pending_applied(state: &mut SessionSystemContextState) {
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::MarkPendingApplied {
            pending_count: usize_to_u64(state.pending.len()),
        },
        "PendingApplyResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected pending apply");
            return;
        }
    };
    let Some((
        apply_non_runtime_steer_appends,
        mark_seen_applied,
        remove_runtime_steer_seen,
        clear_pending,
        clear_active_turn_pending_keys,
    )) = effects.into_iter().find_map(|effect| {
        if let SessionSystemContextAuthorityEffect::PendingApplyResolved {
            apply_non_runtime_steer_appends,
            mark_seen_applied,
            remove_runtime_steer_seen,
            clear_pending,
            clear_active_turn_pending_keys,
        } = effect
        {
            Some((
                apply_non_runtime_steer_appends,
                mark_seen_applied,
                remove_runtime_steer_seen,
                clear_pending,
                clear_active_turn_pending_keys,
            ))
        } else {
            None
        }
    }) else {
        tracing::warn!("generated system-context authority omitted pending apply plan");
        return;
    };
    for pending in &state.pending {
        if is_runtime_steer_append(pending) || !apply_non_runtime_steer_appends {
            continue;
        }
        if !state.applied.contains(pending) {
            state.applied.push(pending.clone());
        }
    }
    let mut seen_to_remove = Vec::new();
    for pending in &state.pending {
        if let Some(key) = pending.idempotency_key.as_ref()
            && let Some(seen) = state.seen.get_mut(key)
        {
            if remove_runtime_steer_seen && is_runtime_steer_append(pending) {
                seen_to_remove.push(key.clone());
            } else if mark_seen_applied {
                seen.state = SeenSystemContextState::Applied;
            }
        }
    }
    for key in seen_to_remove {
        state.seen.remove(&key);
    }
    if clear_pending {
        state.pending.clear();
    }
    if clear_active_turn_pending_keys {
        state.active_turn_pending_keys.clear();
    }
}

pub fn discard_unapplied_active_turn_pending(
    state: &mut SessionSystemContextState,
) -> Vec<PendingSystemContextAppend> {
    if state.active_turn_pending_keys.is_empty() {
        return Vec::new();
    }
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::DiscardUnappliedActiveTurnPending {
            active_turn_pending_key_count: usize_to_u64(state.active_turn_pending_keys.len()),
        },
        "ActiveTurnPendingDiscardResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected active-turn discard");
            return Vec::new();
        }
    };
    let Some((discard_matching_active_turn_keys, remove_pending_seen_for_discarded)) =
        effects.into_iter().find_map(|effect| {
            if let SessionSystemContextAuthorityEffect::ActiveTurnPendingDiscardResolved {
                discard_matching_active_turn_keys,
                remove_pending_seen_for_discarded,
            } = effect
            {
                Some((
                    discard_matching_active_turn_keys,
                    remove_pending_seen_for_discarded,
                ))
            } else {
                None
            }
        })
    else {
        tracing::warn!("generated system-context authority omitted active-turn discard plan");
        return Vec::new();
    };
    if !discard_matching_active_turn_keys {
        return Vec::new();
    }

    let active_keys = std::mem::take(&mut state.active_turn_pending_keys);
    let mut discarded = Vec::new();
    state.pending.retain(|append| {
        let should_discard = append
            .idempotency_key
            .as_ref()
            .is_some_and(|key| active_keys.contains(key));
        if should_discard {
            discarded.push(append.clone());
        }
        !should_discard
    });

    for append in &discarded {
        if remove_pending_seen_for_discarded
            && let Some(key) = append.idempotency_key.as_ref()
            && state
                .seen
                .get(key)
                .is_some_and(|seen| seen.state == SeenSystemContextState::Pending)
        {
            state.seen.remove(key);
        }
    }

    discarded
}

pub fn discard_active_turn_pending_by_keys(
    state: &mut SessionSystemContextState,
    idempotency_keys: &[String],
) -> Vec<PendingSystemContextAppend> {
    if idempotency_keys.is_empty() || state.active_turn_pending_keys.is_empty() {
        return Vec::new();
    }
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::DiscardActiveTurnPendingByKeys {
            requested_key_count: usize_to_u64(idempotency_keys.len()),
            active_turn_pending_key_count: usize_to_u64(state.active_turn_pending_keys.len()),
        },
        "ActiveTurnKeyedDiscardResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected active-turn keyed discard");
            return Vec::new();
        }
    };
    let Some((discard_requested_active_turn_keys, remove_pending_seen_for_discarded)) =
        effects.into_iter().find_map(|effect| {
            if let SessionSystemContextAuthorityEffect::ActiveTurnKeyedDiscardResolved {
                discard_requested_active_turn_keys,
                remove_pending_seen_for_discarded,
            } = effect
            {
                Some((
                    discard_requested_active_turn_keys,
                    remove_pending_seen_for_discarded,
                ))
            } else {
                None
            }
        })
    else {
        tracing::warn!("generated system-context authority omitted active-turn keyed discard plan");
        return Vec::new();
    };
    if !discard_requested_active_turn_keys {
        return Vec::new();
    }

    let requested_keys: BTreeSet<&str> = idempotency_keys.iter().map(String::as_str).collect();
    let mut discarded = Vec::new();
    let mut discarded_keys = Vec::new();
    state.pending.retain(|append| {
        let should_discard = append.idempotency_key.as_ref().is_some_and(|key| {
            requested_keys.contains(key.as_str()) && state.active_turn_pending_keys.contains(key)
        });
        if should_discard {
            if let Some(key) = append.idempotency_key.as_ref() {
                discarded_keys.push(key.clone());
            }
            discarded.push(append.clone());
        }
        !should_discard
    });

    for key in discarded_keys {
        state.active_turn_pending_keys.remove(&key);
        if remove_pending_seen_for_discarded
            && state
            .seen
            .get(&key)
            .is_some_and(|seen| seen.state == SeenSystemContextState::Pending)
        {
            state.seen.remove(&key);
        }
    }

    discarded
}

pub fn discard_transient_runtime_steer_state(state: &mut SessionSystemContextState) -> usize {
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::DiscardTransientRuntimeSteerState {
            observed_entry_count: usize_to_u64(
                state.pending.len()
                    + state.applied.len()
                    + state.seen.len()
                    + state.active_turn_pending_keys.len(),
            ),
        },
        "TransientRuntimeSteerDiscardResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected steer cleanup");
            return 0;
        }
    };
    let Some((
        discard_runtime_steer_appends,
        discard_runtime_steer_seen,
        discard_runtime_steer_active_keys,
    )) = effects.into_iter().find_map(|effect| {
        if let SessionSystemContextAuthorityEffect::TransientRuntimeSteerDiscardResolved {
            discard_runtime_steer_appends,
            discard_runtime_steer_seen,
            discard_runtime_steer_active_keys,
        } = effect
        {
            Some((
                discard_runtime_steer_appends,
                discard_runtime_steer_seen,
                discard_runtime_steer_active_keys,
            ))
        } else {
            None
        }
    }) else {
        tracing::warn!("generated system-context authority omitted steer cleanup plan");
        return 0;
    };
    let mut removed = 0usize;

    if discard_runtime_steer_appends {
        let before_pending = state.pending.len();
        state
            .pending
            .retain(|append| !is_runtime_steer_append(append));
        removed += before_pending.saturating_sub(state.pending.len());

        let before_applied = state.applied.len();
        state
            .applied
            .retain(|append| !is_runtime_steer_append(append));
        removed += before_applied.saturating_sub(state.applied.len());
    }

    if discard_runtime_steer_seen {
        let before_seen = state.seen.len();
        state.seen.retain(|key, seen| {
            !is_runtime_steer_key(key) && !seen.source.as_deref().is_some_and(is_runtime_steer_key)
        });
        removed += before_seen.saturating_sub(state.seen.len());
    }

    if discard_runtime_steer_active_keys {
        let before_active = state.active_turn_pending_keys.len();
        state
            .active_turn_pending_keys
            .retain(|key| !is_runtime_steer_key(key));
        removed += before_active.saturating_sub(state.active_turn_pending_keys.len());
    }

    removed
}

#[must_use]
pub fn remove_runtime_steer_prompt_blocks(system_prompt: &str) -> (String, usize) {
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::RemoveRuntimeSteerPromptBlocks {
            observed_part_count: usize_to_u64(system_prompt.split(SYSTEM_CONTEXT_SEPARATOR).count()),
        },
        "RuntimeSteerPromptCleanupResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected prompt cleanup");
            return (system_prompt.to_string(), 0);
        }
    };
    let Some(remove_runtime_steer_blocks) = effects.into_iter().find_map(|effect| {
        if let SessionSystemContextAuthorityEffect::RuntimeSteerPromptCleanupResolved {
            remove_runtime_steer_blocks,
        } = effect
        {
            Some(remove_runtime_steer_blocks)
        } else {
            None
        }
    }) else {
        tracing::warn!("generated system-context authority omitted prompt cleanup plan");
        return (system_prompt.to_string(), 0);
    };
    if !remove_runtime_steer_blocks {
        return (system_prompt.to_string(), 0);
    }
    let parts = system_prompt
        .split(SYSTEM_CONTEXT_SEPARATOR)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let original_len = parts.len();
    let retained = parts
        .into_iter()
        .filter(|part| {
            !(part.starts_with("[Runtime System Context]")
                && part.contains("\nsource: runtime:steer:"))
        })
        .collect::<Vec<_>>();
    let removed = original_len.saturating_sub(retained.len());
    (retained.join(SYSTEM_CONTEXT_SEPARATOR), removed)
}

pub fn record_applied_system_context_blocks(
    state: &mut SessionSystemContextState,
    appends: &[PendingSystemContextAppend],
    current_system_prompt: &str,
) -> Vec<PendingSystemContextAppend> {
    let effects = match system_context_authority_effect(
        SessionSystemContextAuthorityInput::RecordAppliedSystemContextBlocks {
            append_count: usize_to_u64(appends.len()),
        },
        "AppliedBlocksRecordResolved",
    ) {
        Ok(effects) => effects,
        Err(_) => {
            tracing::warn!("generated system-context authority rejected applied context recording");
            return Vec::new();
        }
    };
    let Some((record_new_appends, mark_seen_applied)) =
        effects.into_iter().find_map(|effect| {
            if let SessionSystemContextAuthorityEffect::AppliedBlocksRecordResolved {
                record_new_appends,
                mark_seen_applied,
            } = effect
            {
                Some((record_new_appends, mark_seen_applied))
            } else {
                None
            }
        })
    else {
        tracing::warn!("generated system-context authority omitted applied context recording plan");
        return Vec::new();
    };
    if !record_new_appends {
        return Vec::new();
    }
    let mut new_appends: Vec<PendingSystemContextAppend> = Vec::new();
    for append in appends {
        if append.text.trim().is_empty() {
            continue;
        }
        let rendered = render_system_context_block(append);
        if let Some(key) = append.idempotency_key.as_ref() {
            if let Some(existing) = state.seen.get(key)
                && !seen_system_context_matches(existing, append)
            {
                tracing::warn!(
                    idempotency_key = %key,
                    "skipping conflicting runtime system-context append"
                );
                continue;
            }
            if let Some(existing) = state
                .applied
                .iter()
                .find(|applied| applied.idempotency_key.as_ref() == Some(key))
                && !pending_system_context_matches(existing, append)
            {
                tracing::warn!(
                    idempotency_key = %key,
                    "skipping conflicting runtime system-context append"
                );
                continue;
            }
            if let Some(existing) = new_appends
                .iter()
                .find(|pending| pending.idempotency_key.as_ref() == Some(key))
            {
                if !pending_system_context_matches(existing, append) {
                    tracing::warn!(
                        idempotency_key = %key,
                        "skipping conflicting runtime system-context append"
                    );
                }
                continue;
            }
            if current_system_prompt.contains(&rendered) {
                if mark_seen_applied {
                    record_applied_append(state, append);
                }
                continue;
            }
        } else if new_appends.contains(append) || current_system_prompt.contains(&rendered) {
            continue;
        }
        if mark_seen_applied {
            record_applied_append(state, append);
        }
        new_appends.push(append.clone());
    }
    new_appends
}

#[must_use]
pub fn render_system_context_block(append: &PendingSystemContextAppend) -> String {
    let mut rendered = String::from("[Runtime System Context]");
    if let Some(source) = &append.source {
        rendered.push_str("\nsource: ");
        rendered.push_str(source);
    }
    rendered.push_str("\n\n");
    rendered.push_str(&append.text);
    rendered
}

fn record_applied_append(
    state: &mut SessionSystemContextState,
    append: &PendingSystemContextAppend,
) {
    if let Some(key) = append.idempotency_key.as_ref() {
        state.seen.insert(
            key.clone(),
            SeenSystemContextKey {
                text: append.text.clone(),
                source: append.source.clone(),
                state: SeenSystemContextState::Applied,
            },
        );
        if state
            .applied
            .iter()
            .any(|applied| applied.idempotency_key.as_ref() == Some(key))
        {
            return;
        }
    } else if state.applied.contains(append) {
        return;
    }
    state.applied.push(append.clone());
}

fn is_runtime_steer_key(value: &str) -> bool {
    value.starts_with("runtime:steer:")
}

fn is_runtime_steer_append(append: &PendingSystemContextAppend) -> bool {
    append.source.as_deref().is_some_and(is_runtime_steer_key)
        || append
            .idempotency_key
            .as_deref()
            .is_some_and(is_runtime_steer_key)
}

fn seen_system_context_matches(
    seen: &SeenSystemContextKey,
    append: &PendingSystemContextAppend,
) -> bool {
    seen.text == append.text && seen.source.as_deref() == append.source.as_deref()
}

fn pending_system_context_matches(
    existing: &PendingSystemContextAppend,
    append: &PendingSystemContextAppend,
) -> bool {
    existing.text == append.text && existing.source.as_deref() == append.source.as_deref()
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
"#;
    writeln!(out, "{source}")?;
    Ok(())
}

fn emit_session_durable_config_domain_helpers(out: &mut String) {
    out.push_str(
        r#"
#[derive(Debug, Clone)]
pub struct AuthorizedSessionMetadata {
    metadata: SessionMetadata,
}

impl AuthorizedSessionMetadata {
    #[must_use]
    pub fn into_metadata(self) -> SessionMetadata {
        self.metadata
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizedSessionBuildState {
    state: SessionBuildState,
}

impl AuthorizedSessionBuildState {
    #[must_use]
    pub fn into_state(self) -> SessionBuildState {
        self.state
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedSystemPrompt {
    prompt: String,
    replacing_existing: bool,
}

impl AuthorizedSystemPrompt {
    #[must_use]
    pub fn into_parts(self) -> (String, bool) {
        (self.prompt, self.replacing_existing)
    }
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn provider_kind(provider: Provider) -> SessionDurableProviderKind {
    match provider {
        Provider::Anthropic => SessionDurableProviderKind::Anthropic,
        Provider::OpenAI => SessionDurableProviderKind::OpenAI,
        Provider::Gemini => SessionDurableProviderKind::Gemini,
        Provider::SelfHosted => SessionDurableProviderKind::SelfHosted,
        Provider::Other => SessionDurableProviderKind::Other,
    }
}

fn tool_override_kind(value: ToolCategoryOverride) -> SessionToolCategoryOverrideKind {
    match value {
        ToolCategoryOverride::Inherit => SessionToolCategoryOverrideKind::Inherit,
        ToolCategoryOverride::Enable => SessionToolCategoryOverrideKind::Enable,
        ToolCategoryOverride::Disable => SessionToolCategoryOverrideKind::Disable,
    }
}

fn call_timeout_override_kind(value: &CallTimeoutOverride) -> SessionCallTimeoutOverrideKind {
    match value {
        CallTimeoutOverride::Inherit => SessionCallTimeoutOverrideKind::Inherit,
        CallTimeoutOverride::Disabled => SessionCallTimeoutOverrideKind::Disabled,
        CallTimeoutOverride::Value(_) => SessionCallTimeoutOverrideKind::Value,
    }
}

fn active_skill_count(tooling: &SessionTooling) -> u64 {
    tooling
        .active_skills
        .as_ref()
        .map_or(0, |skills| usize_to_u64(skills.len()))
}

fn metadata_persist_input(metadata: &SessionMetadata) -> SessionDurableConfigAuthorityInput {
    SessionDurableConfigAuthorityInput::AuthorizeSessionMetadataPersist {
        schema_version: u64::from(metadata.schema_version),
        model_present: !metadata.model.trim().is_empty(),
        max_tokens: u64::from(metadata.max_tokens),
        structured_output_retries: u64::from(metadata.structured_output_retries),
        provider: provider_kind(metadata.provider),
        self_hosted_server_present: metadata.self_hosted_server_id.is_some(),
        provider_params_present: metadata.provider_params.is_some(),
        tooling_builtins: tool_override_kind(metadata.tooling.builtins),
        tooling_shell: tool_override_kind(metadata.tooling.shell),
        tooling_comms: tool_override_kind(metadata.tooling.comms),
        tooling_mob: tool_override_kind(metadata.tooling.mob),
        tooling_memory: tool_override_kind(metadata.tooling.memory),
        tooling_schedule: tool_override_kind(metadata.tooling.schedule),
        tooling_workgraph: tool_override_kind(metadata.tooling.workgraph),
        tooling_image_generation: tool_override_kind(metadata.tooling.image_generation),
        tooling_web_search: tool_override_kind(metadata.tooling.web_search),
        active_skill_count: active_skill_count(&metadata.tooling),
        keep_alive: metadata.keep_alive,
        comms_name_present: metadata.comms_name.is_some(),
        peer_meta_present: metadata.peer_meta.is_some(),
        realm_id_present: metadata.realm_id.is_some(),
        instance_id_present: metadata.instance_id.is_some(),
        backend_present: metadata.backend.is_some(),
        config_generation_present: metadata.config_generation.is_some(),
        auth_binding_present: metadata.auth_binding.is_some(),
    }
}

fn build_state_persist_input(state: &SessionBuildState) -> SessionDurableConfigAuthorityInput {
    let mob_tool_authority_context_present = state.mob_tool_authority_context.is_some();
    let mob_tool_authority_context_generated = state
        .mob_tool_authority_context
        .as_ref()
        .is_some_and(|context| context.is_generated_authority_context());
    SessionDurableConfigAuthorityInput::AuthorizeSessionBuildStatePersist {
        system_prompt_present: state.system_prompt.is_some(),
        output_schema_present: state.output_schema.is_some(),
        hook_entry_count: usize_to_u64(state.hooks_override.entries.len()),
        disabled_hook_count: usize_to_u64(state.hooks_override.disable.len()),
        budget_limits_present: state.budget_limits.is_some(),
        recoverable_tool_count: usize_to_u64(state.recoverable_tool_defs.len()),
        silent_comms_intent_count: usize_to_u64(state.silent_comms_intents.len()),
        max_inline_peer_notifications_present: state.max_inline_peer_notifications.is_some(),
        app_context_present: state.app_context.is_some(),
        additional_instruction_count: state
            .additional_instructions
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        shell_env_count: state
            .shell_env
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        mob_tool_authority_context_present,
        mob_tool_authority_context_generated,
        call_timeout_override: call_timeout_override_kind(&state.call_timeout_override),
    }
}

fn build_state_restore_input(state: &SessionBuildState) -> SessionDurableConfigAuthorityInput {
    SessionDurableConfigAuthorityInput::RestoreSessionBuildState {
        system_prompt_present: state.system_prompt.is_some(),
        output_schema_present: state.output_schema.is_some(),
        hook_entry_count: usize_to_u64(state.hooks_override.entries.len()),
        disabled_hook_count: usize_to_u64(state.hooks_override.disable.len()),
        budget_limits_present: state.budget_limits.is_some(),
        recoverable_tool_count: usize_to_u64(state.recoverable_tool_defs.len()),
        silent_comms_intent_count: usize_to_u64(state.silent_comms_intents.len()),
        max_inline_peer_notifications_present: state.max_inline_peer_notifications.is_some(),
        app_context_present: state.app_context.is_some(),
        additional_instruction_count: state
            .additional_instructions
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        shell_env_count: state
            .shell_env
            .as_ref()
            .map_or(0, |items| usize_to_u64(items.len())),
        mob_tool_authority_context_present: state.mob_tool_authority_context.is_some(),
        call_timeout_override: call_timeout_override_kind(&state.call_timeout_override),
    }
}

fn system_prompt_input(
    prompt: &str,
    source: SessionSystemPromptSource,
    replacing_existing: bool,
) -> SessionDurableConfigAuthorityInput {
    SessionDurableConfigAuthorityInput::AuthorizeSystemPromptMutation {
        source,
        prompt_present: !prompt.is_empty(),
        prompt_byte_count: usize_to_u64(prompt.len()),
        replacing_existing,
    }
}

fn require_durable_config_effect(
    input: SessionDurableConfigAuthorityInput,
    expected: &'static str,
) -> Result<Vec<SessionDurableConfigAuthorityEffect>, SessionDurableConfigAuthorityError> {
    let mut authority = SessionDurableConfigAuthorityMachineAuthority::new();
    let effects = authority.apply_input(input)?;
    let matches_expected = effects.iter().all(|effect| {
        matches!(
            (expected, effect),
            (
                "SessionMetadataPersistAuthorized",
                SessionDurableConfigAuthorityEffect::SessionMetadataPersistAuthorized { .. }
            ) | (
                "SessionBuildStatePersistAuthorized",
                SessionDurableConfigAuthorityEffect::SessionBuildStatePersistAuthorized { .. }
            ) | (
                "SessionBuildStateRestoreAuthorized",
                SessionDurableConfigAuthorityEffect::SessionBuildStateRestoreAuthorized { .. }
            ) | (
                "SystemPromptMutationAuthorized",
                SessionDurableConfigAuthorityEffect::SystemPromptMutationAuthorized { .. }
            )
        )
    });
    if !effects.is_empty() && matches_expected {
        Ok(effects)
    } else {
        Err(SessionDurableConfigAuthorityError { op: expected })
    }
}

pub fn authorize_session_metadata_persist(
    mut metadata: SessionMetadata,
) -> Result<AuthorizedSessionMetadata, SessionDurableConfigAuthorityError> {
    metadata.schema_version = crate::session_metadata_schema_version();
    require_durable_config_effect(
        metadata_persist_input(&metadata),
        "SessionMetadataPersistAuthorized",
    )?;
    Ok(AuthorizedSessionMetadata { metadata })
}

pub fn restore_session_metadata(
    metadata: SessionMetadata,
) -> Result<SessionMetadata, SessionDurableConfigAuthorityError> {
    require_durable_config_effect(
        metadata_persist_input(&metadata),
        "SessionMetadataPersistAuthorized",
    )?;
    Ok(metadata)
}

pub fn authorize_session_build_state_persist(
    state: SessionBuildState,
) -> Result<AuthorizedSessionBuildState, SessionDurableConfigAuthorityError> {
    require_durable_config_effect(
        build_state_persist_input(&state),
        "SessionBuildStatePersistAuthorized",
    )?;
    Ok(AuthorizedSessionBuildState { state })
}

pub fn restore_session_build_state(
    state: SessionBuildState,
) -> Result<SessionBuildState, SessionDurableConfigAuthorityError> {
    require_durable_config_effect(
        build_state_restore_input(&state),
        "SessionBuildStateRestoreAuthorized",
    )?;
    Ok(state)
}

pub fn authorize_system_prompt_mutation(
    prompt: String,
    source: SessionSystemPromptSource,
    replacing_existing: bool,
) -> Result<AuthorizedSystemPrompt, SessionDurableConfigAuthorityError> {
    require_durable_config_effect(
        system_prompt_input(&prompt, source, replacing_existing),
        "SystemPromptMutationAuthorized",
    )?;
    Ok(AuthorizedSystemPrompt {
        prompt,
        replacing_existing,
    })
}
"#,
    );
}

fn generate_pending_continuation_admission(machine: &MachineSchema) -> Result<String> {
    validate_pending_continuation_admission_schema(machine)?;

    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — pending continuation admission authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `PendingContinuationAdmissionMachine`."
    )?;
    writeln!(
        &mut out,
        "#![allow(clippy::bool_comparison, clippy::field_reassign_with_default)]"
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::fmt;")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use crate::types::Message;")?;
    writeln!(&mut out)?;

    for enum_name in [
        "ObservedSessionTailKind",
        "PendingContinuationDisposition",
        "PendingContinuationPublicTerminal",
    ] {
        emit_pending_named_string_enum(&mut out, machine, enum_name)?;
    }
    emit_pending_input_enum(&mut out, machine)?;
    emit_pending_effect_enum(&mut out, machine)?;

    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub struct PendingContinuationResolution {{")?;
    writeln!(
        &mut out,
        "    pub disposition: PendingContinuationDisposition,"
    )?;
    writeln!(
        &mut out,
        "    pub public_terminal: Option<PendingContinuationPublicTerminal>,"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(&mut out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub struct PendingContinuationAdmissionError {{")?;
    writeln!(&mut out, "    op: &'static str,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "impl fmt::Display for PendingContinuationAdmissionError {{"
    )?;
    writeln!(
        &mut out,
        "    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{"
    )?;
    writeln!(
        &mut out,
        "        write!(f, \"generated pending-continuation authority rejected {{}}\", self.op)"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(
        &mut out,
        "impl std::error::Error for PendingContinuationAdmissionError {{}}"
    )?;
    writeln!(&mut out)?;

    emit_pending_phase_enum(&mut out, machine)?;
    emit_pending_state_struct(&mut out, machine)?;

    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(
        &mut out,
        "pub struct PendingContinuationAdmissionMachineAuthority {{"
    )?;
    writeln!(
        &mut out,
        "    state: PendingContinuationAdmissionMachineState,"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    emit_pending_authority_impl(&mut out, machine)?;
    writeln!(
        &mut out,
        "impl Default for PendingContinuationAdmissionMachineAuthority {{"
    )?;
    writeln!(&mut out, "    fn default() -> Self {{")?;
    writeln!(&mut out, "        Self::new()")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(&mut out, "#[must_use]")?;
    writeln!(
        &mut out,
        "pub fn observe_session_tail(messages: &[Message]) -> ObservedSessionTailKind {{"
    )?;
    writeln!(&mut out, "    match messages.last() {{")?;
    writeln!(&mut out, "        None => ObservedSessionTailKind::Empty,")?;
    writeln!(
        &mut out,
        "        Some(Message::System(_)) => ObservedSessionTailKind::System,"
    )?;
    writeln!(
        &mut out,
        "        Some(Message::SystemNotice(_)) => ObservedSessionTailKind::SystemNotice,"
    )?;
    writeln!(
        &mut out,
        "        Some(Message::User(_)) => ObservedSessionTailKind::User,"
    )?;
    writeln!(
        &mut out,
        "        Some(Message::Assistant(_)) => ObservedSessionTailKind::Assistant,"
    )?;
    writeln!(
        &mut out,
        "        Some(Message::BlockAssistant(_)) => ObservedSessionTailKind::BlockAssistant,"
    )?;
    writeln!(
        &mut out,
        "        Some(Message::ToolResults {{ .. }}) => ObservedSessionTailKind::ToolResults,"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "pub fn resolve_pending_continuation(")?;
    writeln!(&mut out, "    session_tail: ObservedSessionTailKind,")?;
    writeln!(&mut out, "    staged_tool_result_count: u64,")?;
    writeln!(
        &mut out,
        ") -> Result<PendingContinuationResolution, PendingContinuationAdmissionError> {{"
    )?;
    writeln!(
        &mut out,
        "    PendingContinuationAdmissionMachineAuthority::new()"
    )?;
    writeln!(
        &mut out,
        "        .resolve_pending_continuation(session_tail, staged_tool_result_count)"
    )?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;
    emit_pending_helper(
        &mut out,
        pending_helper(machine, "tail_has_pending_boundary")?,
    )?;
    emit_pending_helper(
        &mut out,
        pending_helper(machine, "has_effective_pending_boundary")?,
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "#[cfg(test)]")?;
    writeln!(&mut out, "#[allow(clippy::expect_used)]")?;
    writeln!(&mut out, "mod tests {{")?;
    writeln!(&mut out, "    use super::*;")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    #[test]")?;
    writeln!(
        &mut out,
        "    fn user_and_tool_results_tails_admit_pending_continuation() {{"
    )?;
    writeln!(
        &mut out,
        "        for tail in [ObservedSessionTailKind::User, ObservedSessionTailKind::ToolResults] {{"
    )?;
    writeln!(
        &mut out,
        "            let resolution = resolve_pending_continuation(tail, 0)"
    )?;
    writeln!(
        &mut out,
        "                .expect(\"generated authority should resolve pending tail\");"
    )?;
    writeln!(
        &mut out,
        "            assert_eq!(resolution.disposition, PendingContinuationDisposition::RunPending);"
    )?;
    writeln!(
        &mut out,
        "            assert_eq!(resolution.public_terminal, None);"
    )?;
    writeln!(&mut out, "        }}")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    #[test]")?;
    writeln!(
        &mut out,
        "    fn staged_tool_results_admit_pending_continuation() {{"
    )?;
    writeln!(
        &mut out,
        "        let resolution = resolve_pending_continuation(ObservedSessionTailKind::Empty, 1)"
    )?;
    writeln!(
        &mut out,
        "            .expect(\"generated authority should resolve staged results\");"
    )?;
    writeln!(
        &mut out,
        "        assert_eq!(resolution.disposition, PendingContinuationDisposition::RunPending);"
    )?;
    writeln!(
        &mut out,
        "        assert_eq!(resolution.public_terminal, None);"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "    #[test]")?;
    writeln!(
        &mut out,
        "    fn non_boundary_tail_emits_no_pending_terminal() {{"
    )?;
    writeln!(
        &mut out,
        "        let resolution = resolve_pending_continuation(ObservedSessionTailKind::Assistant, 0)"
    )?;
    writeln!(
        &mut out,
        "            .expect(\"generated authority should resolve non-boundary tail\");"
    )?;
    writeln!(
        &mut out,
        "        assert_eq!(resolution.disposition, PendingContinuationDisposition::NoPendingBoundary);"
    )?;
    writeln!(&mut out, "        assert_eq!(")?;
    writeln!(&mut out, "            resolution.public_terminal,")?;
    writeln!(
        &mut out,
        "            Some(PendingContinuationPublicTerminal::NoPendingBoundary)"
    )?;
    writeln!(&mut out, "        );")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    Ok(out)
}

fn emit_string_enum(
    out: &mut String,
    name: &str,
    variants: &[String],
    default_variant: &str,
) -> Result<()> {
    writeln!(
        out,
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]"
    )?;
    writeln!(out, "pub enum {name} {{")?;
    for variant in variants {
        if variant == default_variant {
            writeln!(out, "    #[default]")?;
        }
        writeln!(out, "    {variant},")?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_named_string_enum(
    out: &mut String,
    machine: &MachineSchema,
    name: &str,
) -> Result<()> {
    let variants = pending_named_string_enum_variants(machine, name)?;
    let default_variant = pending_default_variant(name, &variants)?;
    emit_string_enum(out, name, &variants, default_variant)
}

fn pending_named_string_enum_variants(machine: &MachineSchema, name: &str) -> Result<Vec<String>> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| {
            format!("PendingContinuationAdmissionMachine missing named type `{name}`")
        })?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("PendingContinuationAdmissionMachine named type `{name}` must be a string enum");
    };
    Ok(variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect())
}

fn pending_default_variant<'a>(name: &str, variants: &'a [String]) -> Result<&'a str> {
    let wanted = match name {
        "ObservedSessionTailKind" => "Empty",
        "PendingContinuationDisposition" => "NoPendingBoundary",
        "PendingContinuationPublicTerminal" => "NoPendingBoundary",
        other => bail!("unknown PendingContinuationAdmissionMachine enum `{other}`"),
    };
    if variants.iter().any(|variant| variant == wanted) {
        Ok(wanted)
    } else {
        bail!(
            "PendingContinuationAdmissionMachine enum `{name}` missing default variant `{wanted}`"
        );
    }
}

fn emit_pending_phase_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    let variants = machine
        .state
        .phase
        .variants
        .iter()
        .map(|variant| variant.name.as_str().to_owned())
        .collect::<Vec<_>>();
    emit_string_enum(
        out,
        "PendingContinuationAdmissionPhase",
        &variants,
        machine.state.init.phase.as_str(),
    )
}

fn emit_pending_state_struct(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]")?;
    writeln!(
        out,
        "pub struct PendingContinuationAdmissionMachineState {{"
    )?;
    writeln!(
        out,
        "    lifecycle_phase: PendingContinuationAdmissionPhase,"
    )?;
    for field in &machine.state.fields {
        writeln!(
            out,
            "    {}: {},",
            field.name,
            render_pending_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_input_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub enum PendingContinuationAdmissionInput {{")?;
    for variant in &machine.inputs.variants {
        emit_pending_variant(out, variant)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_effect_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub enum PendingContinuationAdmissionEffect {{")?;
    for variant in &machine.effects.variants {
        emit_pending_variant(out, variant)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_variant(out: &mut String, variant: &VariantSchema) -> Result<()> {
    if variant.fields.is_empty() {
        writeln!(out, "    {},", variant.name)?;
        return Ok(());
    }
    writeln!(out, "    {} {{", variant.name)?;
    for field in &variant.fields {
        writeln!(
            out,
            "        {}: {},",
            field.name,
            render_pending_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "    }},")?;
    Ok(())
}

fn emit_pending_authority_impl(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "impl PendingContinuationAdmissionMachineAuthority {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new() -> Self {{")?;
    writeln!(
        out,
        "        let mut state = PendingContinuationAdmissionMachineState::default();"
    )?;
    writeln!(
        out,
        "        state.lifecycle_phase = PendingContinuationAdmissionPhase::{};",
        machine.state.init.phase
    )?;
    for init in &machine.state.init.fields {
        writeln!(
            out,
            "        state.{} = {};",
            init.field,
            render_pending_expr(&init.expr)?
        )?;
    }
    writeln!(out, "        Self {{ state }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn state(&self) -> &PendingContinuationAdmissionMachineState {{"
    )?;
    writeln!(out, "        &self.state")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    emit_pending_apply_input(out, machine)?;
    emit_pending_resolve_pending_method(out)?;
    emit_pending_resolve_last_terminal_method(out)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_apply_input(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "    fn apply_input(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        input: PendingContinuationAdmissionInput,")?;
    writeln!(
        out,
        "    ) -> Result<Vec<PendingContinuationAdmissionEffect>, PendingContinuationAdmissionError> {{"
    )?;
    writeln!(out, "        match input {{")?;
    for input_variant in &machine.inputs.variants {
        emit_pending_apply_input_arm(out, machine, input_variant)?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_apply_input_arm(
    out: &mut String,
    machine: &MachineSchema,
    input_variant: &VariantSchema,
) -> Result<()> {
    write!(
        out,
        "            PendingContinuationAdmissionInput::{}",
        input_variant.name
    )?;
    if input_variant.fields.is_empty() {
        writeln!(out, " => {{")?;
    } else {
        writeln!(out, " {{")?;
        for field in &input_variant.fields {
            writeln!(out, "                {},", field.name)?;
        }
        writeln!(out, "            }} => {{")?;
    }
    for transition in machine.transitions.iter().filter(|transition| {
        matches!(
            &transition.on,
            TriggerMatch::Input { variant, .. } if variant.as_str() == input_variant.name.as_str()
        )
    }) {
        emit_pending_transition_block(out, transition)?;
    }
    writeln!(
        out,
        "                Err(PendingContinuationAdmissionError {{ op: \"{}\" }})",
        input_variant.name
    )?;
    writeln!(out, "            }}")?;
    Ok(())
}

fn emit_pending_transition_block(out: &mut String, transition: &TransitionSchema) -> Result<()> {
    writeln!(
        out,
        "                if {} {{",
        render_pending_transition_condition(transition)?
    )?;
    for update in &transition.updates {
        writeln!(
            out,
            "                    {}",
            render_pending_update(update)?
        )?;
    }
    writeln!(
        out,
        "                    self.state.lifecycle_phase = PendingContinuationAdmissionPhase::{};",
        transition.to
    )?;
    if transition.emit.is_empty() {
        writeln!(out, "                    return Ok(Vec::new());")?;
    } else {
        writeln!(out, "                    return Ok(vec![")?;
        for effect in &transition.emit {
            writeln!(
                out,
                "                        {},",
                render_pending_effect_emit(effect)?
            )?;
        }
        writeln!(out, "                    ]);")?;
    }
    writeln!(out, "                }}")?;
    Ok(())
}

fn render_pending_transition_condition(transition: &TransitionSchema) -> Result<String> {
    let mut conditions = Vec::new();
    if transition.from.len() == 1 {
        conditions.push(format!(
            "self.state.lifecycle_phase == PendingContinuationAdmissionPhase::{}",
            transition.from[0]
        ));
    } else if !transition.from.is_empty() {
        conditions.push(format!(
            "matches!(self.state.lifecycle_phase, {})",
            transition
                .from
                .iter()
                .map(|phase| format!("PendingContinuationAdmissionPhase::{phase}"))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    for guard in &transition.guards {
        conditions.push(render_pending_expr(&guard.expr)?);
    }
    if conditions.is_empty() {
        Ok("true".to_string())
    } else {
        Ok(conditions
            .into_iter()
            .map(|condition| format!("({condition})"))
            .collect::<Vec<_>>()
            .join(" && "))
    }
}

fn render_pending_update(update: &Update) -> Result<String> {
    match update {
        Update::Assign { field, expr } => Ok(format!(
            "self.state.{field} = {};",
            render_pending_expr(expr)?
        )),
        other => bail!("unsupported PendingContinuationAdmissionMachine update `{other:?}`"),
    }
}

fn render_pending_effect_emit(effect: &EffectEmit) -> Result<String> {
    if effect.fields.is_empty() {
        return Ok(format!(
            "PendingContinuationAdmissionEffect::{}",
            effect.variant
        ));
    }
    let mut rendered = format!("PendingContinuationAdmissionEffect::{} {{", effect.variant);
    for (idx, (field, expr)) in effect.fields.iter().enumerate() {
        if idx > 0 {
            rendered.push(' ');
        }
        write!(&mut rendered, " {field}: {},", render_pending_expr(expr)?)?;
    }
    rendered.push_str(" }");
    Ok(rendered)
}

fn emit_pending_resolve_pending_method(out: &mut String) -> Result<()> {
    writeln!(out, "    pub fn resolve_pending_continuation(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        session_tail: ObservedSessionTailKind,")?;
    writeln!(out, "        staged_tool_result_count: u64,")?;
    writeln!(
        out,
        "    ) -> Result<PendingContinuationResolution, PendingContinuationAdmissionError> {{"
    )?;
    writeln!(
        out,
        "        let effects = self.apply_input(PendingContinuationAdmissionInput::ResolvePendingContinuation {{"
    )?;
    writeln!(out, "            session_tail,")?;
    writeln!(out, "            staged_tool_result_count,")?;
    writeln!(out, "        }})?;")?;
    writeln!(out, "        let mut disposition = None;")?;
    writeln!(out, "        let mut public_terminal = None;")?;
    writeln!(out, "        for effect in effects {{")?;
    writeln!(out, "            match effect {{")?;
    writeln!(
        out,
        "                PendingContinuationAdmissionEffect::PendingContinuationResolved {{ disposition: value }} => {{"
    )?;
    writeln!(out, "                    disposition = Some(value);")?;
    writeln!(out, "                }}")?;
    writeln!(
        out,
        "                PendingContinuationAdmissionEffect::PendingContinuationPublicTerminalResolved {{ terminal }} => {{"
    )?;
    writeln!(out, "                    public_terminal = Some(terminal);")?;
    writeln!(out, "                }}")?;
    writeln!(out, "            }}")?;
    writeln!(out, "        }}")?;
    writeln!(out, "        let Some(disposition) = disposition else {{")?;
    writeln!(
        out,
        "            return Err(PendingContinuationAdmissionError {{ op: \"pending_continuation_resolution_effect\" }});"
    )?;
    writeln!(out, "        }};")?;
    writeln!(out, "        Ok(PendingContinuationResolution {{")?;
    writeln!(out, "            disposition,")?;
    writeln!(out, "            public_terminal,")?;
    writeln!(out, "        }})")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_resolve_last_terminal_method(out: &mut String) -> Result<()> {
    writeln!(out, "    pub fn resolve_last_public_terminal(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(
        out,
        "    ) -> Result<PendingContinuationPublicTerminal, PendingContinuationAdmissionError> {{"
    )?;
    writeln!(
        out,
        "        let effects = self.apply_input(PendingContinuationAdmissionInput::ResolveLastPendingContinuationPublicTerminal)?;"
    )?;
    writeln!(out, "        for effect in effects {{")?;
    writeln!(
        out,
        "            if let PendingContinuationAdmissionEffect::PendingContinuationPublicTerminalResolved {{ terminal }} = effect {{"
    )?;
    writeln!(out, "                return Ok(terminal);")?;
    writeln!(out, "            }}")?;
    writeln!(out, "        }}")?;
    writeln!(
        out,
        "        Err(PendingContinuationAdmissionError {{ op: \"pending_continuation_public_terminal_effect\" }})"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_pending_helper(out: &mut String, helper: &HelperSchema) -> Result<()> {
    if helper.returns != TypeRef::Bool {
        bail!(
            "PendingContinuationAdmissionMachine helper `{}` must return bool",
            helper.name
        );
    }
    write!(out, "fn {}(", helper.name)?;
    for (idx, param) in helper.params.iter().enumerate() {
        if idx > 0 {
            write!(out, ", ")?;
        }
        write!(
            out,
            "{}: {}",
            param.name.as_str(),
            render_pending_type_ref(&param.ty)?
        )?;
    }
    writeln!(out, ") -> bool {{")?;
    writeln!(out, "    {}", render_pending_expr(&helper.body)?)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn render_pending_type_ref(ty: &TypeRef) -> Result<String> {
    match ty {
        TypeRef::Bool => Ok("bool".to_string()),
        TypeRef::U64 => Ok("u64".to_string()),
        TypeRef::Enum(enum_name) => Ok(enum_name.as_str().to_string()),
        TypeRef::Option(inner) => Ok(format!("Option<{}>", render_pending_type_ref(inner)?)),
        other => bail!("unsupported PendingContinuationAdmissionMachine type `{other:?}`"),
    }
}

fn render_pending_expr(expr: &Expr) -> Result<String> {
    match expr {
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::U64Max => Ok("u64::MAX".to_string()),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::Field(field) => Ok(format!("self.state.{field}")),
        Expr::CurrentPhase => Ok("self.state.lifecycle_phase".to_string()),
        Expr::Binding(binding) => Ok(binding.clone()),
        Expr::Phase(phase) => Ok(format!("PendingContinuationAdmissionPhase::{phase}")),
        Expr::Variant(variant) => Ok(variant.clone()),
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!("Some({})", render_pending_expr(inner)?)),
        Expr::Not(inner) => Ok(format!("!({})", render_pending_expr(inner)?)),
        Expr::Eq(left, right) => Ok(format!(
            "{} == {}",
            render_pending_expr(left)?,
            render_pending_expr(right)?
        )),
        Expr::Neq(left, right) => Ok(format!(
            "{} != {}",
            render_pending_expr(left)?,
            render_pending_expr(right)?
        )),
        Expr::Gt(left, right) => Ok(format!(
            "{} > {}",
            render_pending_expr(left)?,
            render_pending_expr(right)?
        )),
        Expr::Or(items) => render_pending_expr_joined(items, " || "),
        Expr::And(items) => render_pending_expr_joined(items, " && "),
        Expr::Call { helper, args } => {
            let rendered_args = args
                .iter()
                .map(render_pending_expr)
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("{helper}({rendered_args})"))
        }
        other => bail!("unsupported PendingContinuationAdmissionMachine expression `{other:?}`"),
    }
}

fn render_pending_expr_joined(items: &[Expr], separator: &str) -> Result<String> {
    if items.is_empty() {
        bail!("PendingContinuationAdmissionMachine helper expression cannot be empty");
    }
    let rendered = items
        .iter()
        .map(|expr| Ok(format!("({})", render_pending_expr(expr)?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(separator))
}

fn pending_helper<'a>(machine: &'a MachineSchema, name: &str) -> Result<&'a HelperSchema> {
    machine
        .helpers
        .iter()
        .find(|helper| helper.name == name)
        .with_context(|| format!("PendingContinuationAdmissionMachine missing helper `{name}`"))
}

fn validate_pending_continuation_admission_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate PendingContinuationAdmissionMachine schema")?;
    if machine.machine.as_str() != "PendingContinuationAdmissionMachine" {
        bail!(
            "pending continuation generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "ResolvePendingContinuation",
        "ResolveLastPendingContinuationPublicTerminal",
    ] {
        machine.inputs.variant_named(required).with_context(|| {
            format!("PendingContinuationAdmissionMachine missing input `{required}`")
        })?;
    }
    for required in [
        "PendingContinuationResolved",
        "PendingContinuationPublicTerminalResolved",
    ] {
        machine.effects.variant_named(required).with_context(|| {
            format!("PendingContinuationAdmissionMachine missing effect `{required}`")
        })?;
    }
    for transition in &machine.transitions {
        render_pending_transition_condition(transition).with_context(|| {
            format!(
                "PendingContinuationAdmissionMachine transition `{}` has unsupported guard",
                transition.name
            )
        })?;
        for update in &transition.updates {
            render_pending_update(update).with_context(|| {
                format!(
                    "PendingContinuationAdmissionMachine transition `{}` has unsupported update",
                    transition.name
                )
            })?;
        }
        for effect in &transition.emit {
            render_pending_effect_emit(effect).with_context(|| {
                format!(
                    "PendingContinuationAdmissionMachine transition `{}` has unsupported effect",
                    transition.name
                )
            })?;
        }
    }
    Ok(())
}

fn validate_session_system_context_authority_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate SessionSystemContextAuthorityMachine schema")?;
    if machine.machine.as_str() != "SessionSystemContextAuthorityMachine" {
        bail!(
            "session system-context generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "ResolveAppend",
        "MarkPendingApplied",
        "DiscardUnappliedActiveTurnPending",
        "DiscardActiveTurnPendingByKeys",
        "RecordAppliedSystemContextBlocks",
        "DiscardTransientRuntimeSteerState",
        "RemoveRuntimeSteerPromptBlocks",
        "RestoreSystemContextState",
    ] {
        machine.inputs.variant_named(required).with_context(|| {
            format!("SessionSystemContextAuthorityMachine missing input `{required}`")
        })?;
    }
    for required in [
        "AppendResolved",
        "PendingApplyResolved",
        "ActiveTurnPendingDiscardResolved",
        "ActiveTurnKeyedDiscardResolved",
        "AppliedBlocksRecordResolved",
        "TransientRuntimeSteerDiscardResolved",
        "RuntimeSteerPromptCleanupResolved",
        "SnapshotRestoreAuthorized",
    ] {
        machine.effects.variant_named(required).with_context(|| {
            format!("SessionSystemContextAuthorityMachine missing effect `{required}`")
        })?;
    }
    for required in [
        "ResolveAppendEmpty",
        "ResolveAppendConflict",
        "ResolveAppendDuplicate",
        "ResolveAppendNew",
        "AuthorizePendingApplied",
        "AuthorizeDiscardUnappliedActiveTurnPending",
        "AuthorizeDiscardActiveTurnPendingByKeys",
        "AuthorizeRecordAppliedSystemContextBlocks",
        "AuthorizeDiscardTransientRuntimeSteerState",
        "AuthorizeRemoveRuntimeSteerPromptBlocks",
        "AuthorizeRestoreSystemContextState",
    ] {
        if !machine
            .transitions
            .iter()
            .any(|transition| transition.name.as_str() == required)
        {
            bail!("SessionSystemContextAuthorityMachine missing transition `{required}`");
        }
    }
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == "SystemContextAppendDecision")
        .context("SessionSystemContextAuthorityMachine missing SystemContextAppendDecision")?;
    if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
        bail!("SystemContextAppendDecision must be a string enum");
    }
    Ok(())
}

fn validate_session_durable_config_authority_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate SessionDurableConfigAuthorityMachine schema")?;
    if machine.machine.as_str() != "SessionDurableConfigAuthorityMachine" {
        bail!(
            "session durable-config generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "AuthorizeSessionMetadataPersist",
        "AuthorizeSessionBuildStatePersist",
        "RestoreSessionBuildState",
        "AuthorizeSystemPromptMutation",
    ] {
        machine.inputs.variant_named(required).with_context(|| {
            format!("SessionDurableConfigAuthorityMachine missing input `{required}`")
        })?;
    }
    for required in [
        "SessionMetadataPersistAuthorized",
        "SessionBuildStatePersistAuthorized",
        "SessionBuildStateRestoreAuthorized",
        "SystemPromptMutationAuthorized",
    ] {
        machine.effects.variant_named(required).with_context(|| {
            format!("SessionDurableConfigAuthorityMachine missing effect `{required}`")
        })?;
    }
    for required in [
        "AuthorizeSessionMetadataPersist",
        "AuthorizeSessionBuildStatePersist",
        "RestoreSessionBuildState",
        "AuthorizeSystemPromptMutation",
    ] {
        if !machine
            .transitions
            .iter()
            .any(|transition| transition.name.as_str() == required)
        {
            bail!("SessionDurableConfigAuthorityMachine missing transition `{required}`");
        }
    }
    for required in [
        "SessionDurableProviderKind",
        "SessionToolCategoryOverrideKind",
        "SessionCallTimeoutOverrideKind",
        "SessionSystemPromptSource",
    ] {
        let binding = machine
            .named_types
            .iter()
            .find(|binding| binding.name.as_str() == required)
            .with_context(|| {
                format!("SessionDurableConfigAuthorityMachine missing named type `{required}`")
            })?;
        if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
            bail!(
                "SessionDurableConfigAuthorityMachine named type `{required}` must be a string enum"
            );
        }
    }
    Ok(())
}

fn validate_session_realtime_transcript_authority_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate SessionRealtimeTranscriptAuthorityMachine schema")?;
    if machine.machine.as_str() != "SessionRealtimeTranscriptAuthorityMachine" {
        bail!(
            "session realtime transcript generator received unexpected machine `{}`",
            machine.machine
        );
    }
    for required in [
        "ResolveItemObserved",
        "ResolveItemSkipped",
        "ResolveUserTranscriptFinal",
        "ResolveAssistantDelta",
        "ResolveAssistantTextReplacement",
        "ResolveAssistantTurnCompleted",
        "ResolveAssistantTurnInterrupted",
        "ResolveMaterializeCandidate",
        "RestoreRealtimeTranscriptState",
    ] {
        machine.inputs.variant_named(required).with_context(|| {
            format!("SessionRealtimeTranscriptAuthorityMachine missing input `{required}`")
        })?;
    }
    for required in [
        "RealtimeTranscriptEventResolved",
        "MaterializeCandidateResolved",
        "SnapshotRestoreAuthorized",
    ] {
        machine.effects.variant_named(required).with_context(|| {
            format!("SessionRealtimeTranscriptAuthorityMachine missing effect `{required}`")
        })?;
    }
    for required in [
        "ResolveItemObservedPresent",
        "ResolveAssistantDeltaAccepted",
        "ResolveAssistantReplacementAccepted",
        "ResolveAssistantTurnCompletedRecord",
        "ResolveAssistantTurnInterruptedValid",
        "ResolveMaterializeAssistant",
        "AuthorizeRestoreRealtimeTranscriptState",
    ] {
        if !machine
            .transitions
            .iter()
            .any(|transition| transition.name.as_str() == required)
        {
            bail!("SessionRealtimeTranscriptAuthorityMachine missing transition `{required}`");
        }
    }
    for required in [
        "RealtimeTranscriptEventKind",
        "RealtimeTranscriptRoleKind",
        "RealtimeTranscriptLaneKind",
        "RealtimeTranscriptStopReasonKind",
        "RealtimeTranscriptMaterializeDecision",
    ] {
        let binding = machine
            .named_types
            .iter()
            .find(|binding| binding.name.as_str() == required)
            .with_context(|| {
                format!("SessionRealtimeTranscriptAuthorityMachine missing named type `{required}`")
            })?;
        if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
            bail!(
                "SessionRealtimeTranscriptAuthorityMachine named type `{required}` must be a string enum"
            );
        }
    }
    Ok(())
}

fn protected_obligation_protocol(name: &str) -> bool {
    matches!(
        name,
        "comms_trust_reconcile"
            | "supervisor_trust_publish"
            | "supervisor_trust_revoke"
            | "mob_member_peer_overlay"
            | "mob_member_trust_wiring"
            | "mob_member_trust_unwiring"
            | "mob_external_peer_trust_wiring"
            | "mob_external_peer_trust_unwiring"
            | "mob_external_peer_trust_repair"
            | "mob_external_peer_reciprocal_trust"
            | "mob_destroying_session_ingress"
            | "auth_lease_lifecycle_publication"
    )
}

fn is_mob_topology_trust_protocol(name: &str) -> bool {
    matches!(
        name,
        "mob_member_trust_wiring"
            | "mob_member_peer_overlay"
            | "mob_member_trust_unwiring"
            | "mob_external_peer_trust_wiring"
            | "mob_external_peer_trust_unwiring"
            | "mob_external_peer_trust_repair"
            | "mob_external_peer_reciprocal_trust"
    )
}

fn is_supervisor_trust_protocol(name: &str) -> bool {
    matches!(name, "supervisor_trust_publish" | "supervisor_trust_revoke")
}

fn getter_returns_copy(ty: &TypeRef) -> bool {
    matches!(
        ty,
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::Enum(_)
    ) || matches!(ty, TypeRef::Named(name) if is_known_copy_named_type(name.as_str()))
}

fn generate_executor_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
    obligation_type: &str,
) -> Result<()> {
    let rust = &protocol.rust;
    let authority_type = short_type(
        rust.authority_type_path
            .as_deref()
            .context("executor authority type missing")?,
    );
    let input_enum = short_type(
        rust.input_enum_path
            .as_deref()
            .context("executor input enum missing")?,
    );
    let effect_enum = short_type(
        rust.effect_enum_path
            .as_deref()
            .context("executor effect enum missing")?,
    );
    let error_type = short_type(
        rust.error_type_path
            .as_deref()
            .context("executor error type missing")?,
    );
    let trigger_variant_name = rust
        .executor_trigger_input_variant
        .as_ref()
        .map(meerkat_machine_schema::identity::InputVariantId::as_str)
        .context("executor trigger variant missing")?;
    let trigger_variant = producer_machine
        .inputs
        .variant_named(trigger_variant_name)
        .context("executor trigger variant missing from producer machine")?;
    let producer_effect = producer_machine
        .effects
        .variant_named(protocol.effect_variant.as_str())
        .context("producer effect missing")?;

    let result_type = format!("{}ExecutionResult", to_pascal_case(protocol.name.as_str()));
    writeln!(out, "#[derive(Debug)]")?;
    writeln!(out, "pub struct {result_type} {{")?;
    writeln!(out, "    pub effects: Vec<{effect_enum}>,")?;
    writeln!(out, "    pub obligation: Option<{obligation_type}>,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    let execute_name = format!("execute_{}", to_snake_case(trigger_variant_name));
    let trigger_params = trigger_variant
        .fields
        .iter()
        .map(|field| {
            format!(
                "{}: {}",
                to_snake_case(field.name.as_str()),
                rust_type(&field.ty)
            )
        })
        .collect::<Vec<_>>();
    writeln!(
        out,
        "pub fn {execute_name}(authority: &mut {authority_type}{}{}) -> Result<{result_type}, {error_type}> {{",
        if trigger_params.is_empty() { "" } else { ", " },
        trigger_params.join(", ")
    )?;
    writeln!(
        out,
        "    let transition = authority.apply({}::{})?;",
        input_enum,
        ctor_field_list(trigger_variant)
    )?;
    writeln!(
        out,
        "    let obligation = transition.effects().iter().find_map(|effect| match effect {{"
    )?;
    writeln!(
        out,
        "        {}{} => Some({}),",
        effect_enum,
        match_pattern_for_variant(producer_effect),
        obligation_ctor_expr(protocol, obligation_type, producer_effect)?
    )?;
    writeln!(out, "        _ => None,")?;
    writeln!(out, "    }});")?;
    writeln!(
        out,
        "    Ok({result_type} {{ effects: transition.into_effects(), obligation }})"
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    for feedback in &protocol.allowed_feedback_inputs {
        generate_feedback_submitter(
            out,
            protocol,
            feedback,
            producer_machine
                .inputs
                .variant_named(feedback.input_variant.as_str())?,
            FeedbackReturnKind::Effects,
            obligation_type,
        )?;
        if feedback
            .field_bindings
            .iter()
            .all(|binding| matches!(binding.source, FeedbackFieldSource::OwnerContext(_)))
        {
            generate_notify_helper(
                out,
                protocol,
                feedback,
                producer_machine
                    .inputs
                    .variant_named(feedback.input_variant.as_str())?,
                FeedbackReturnKind::Effects,
            )?;
        }
    }

    Ok(())
}

fn generate_effect_extractor_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
    obligation_type: &str,
) -> Result<()> {
    let rust = &protocol.rust;
    let effect_enum = short_type(
        rust.effect_enum_path
            .as_deref()
            .context("effect extractor effect enum missing")?,
    );
    let transition_type = rust.transition_type_path.as_deref().map(short_type);
    let producer_effect = producer_machine
        .effects
        .variant_named(protocol.effect_variant.as_str())
        .context("producer effect missing")?;

    if protocol.name.as_str() == "comms_trust_reconcile" {
        let transition_type = transition_type
            .as_ref()
            .context("comms_trust_reconcile requires transition extractor")?;
        writeln!(
            out,
            "pub fn extract_obligations(transition: &{transition_type}) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(
            out,
            "    extract_obligations_with_freshness(transition, PeerProjectionFreshnessAuthority::missing())"
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(
            out,
            "pub fn extract_obligations_with_freshness(transition: &{transition_type}, peer_projection_freshness_authority: PeerProjectionFreshnessAuthority) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(out, "    transition.effects()")?;
    } else if is_supervisor_trust_protocol(protocol.name.as_str()) {
        let transition_type = transition_type
            .as_ref()
            .context("supervisor trust protocol requires transition extractor")?;
        writeln!(
            out,
            "pub fn extract_obligations(transition: &{transition_type}) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(
            out,
            "    extract_obligations_with_freshness(transition, SupervisorTrustFreshnessAuthority::missing())"
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(
            out,
            "pub fn extract_obligations_with_freshness(transition: &{transition_type}, supervisor_trust_freshness_authority: SupervisorTrustFreshnessAuthority) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(out, "    transition.effects()")?;
    } else if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        let transition_type = transition_type
            .as_ref()
            .context("MobMachine trust protocol requires transition extractor")?;
        writeln!(
            out,
            "pub fn extract_obligations(transition: &{transition_type}) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(
            out,
            "    extract_obligations_with_freshness(transition, MobTopologyFreshnessAuthority::missing())"
        )?;
        writeln!(out, "}}")?;
        writeln!(out)?;
        writeln!(
            out,
            "pub fn extract_obligations_with_freshness(transition: &{transition_type}, mob_topology_freshness_authority: MobTopologyFreshnessAuthority) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(out, "    transition.effects()")?;
    } else if let Some(transition_type) = transition_type {
        writeln!(
            out,
            "pub fn extract_obligations(transition: &{transition_type}) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(out, "    transition.effects()")?;
    } else {
        writeln!(
            out,
            "pub fn extract_obligations(effects: &[{effect_enum}]) -> Vec<{obligation_type}> {{"
        )?;
        writeln!(out, "    effects")?;
    }
    writeln!(out, "        .iter()")?;
    writeln!(out, "        .filter_map(|effect| match effect {{")?;
    writeln!(
        out,
        "            {}{} => Some({}),",
        effect_enum,
        match_pattern_for_variant(producer_effect),
        obligation_ctor_expr(protocol, obligation_type, producer_effect)?
    )?;
    writeln!(out, "            _ => None,")?;
    writeln!(out, "        }})")?;
    writeln!(out, "        .collect()")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    generate_comms_trust_authority_helpers(out, protocol, obligation_type)?;

    // Only emit `submit_*` (authority.apply) helpers when the binding
    // declares an authority. Without one, feedback flows through a
    // stacked `HandleBridge` submitter (see composition validator).
    if rust.authority_type_path.is_none() {
        return Ok(());
    }

    for feedback in &protocol.allowed_feedback_inputs {
        let target_machine = machine_for_instance(
            composition,
            machine_by_name,
            feedback.machine_instance.as_str(),
        )?;
        generate_feedback_submitter(
            out,
            protocol,
            feedback,
            target_machine
                .inputs
                .variant_named(feedback.input_variant.as_str())?,
            FeedbackReturnKind::Transition(std::marker::PhantomData),
            obligation_type,
        )?;
    }

    Ok(())
}

fn generate_comms_trust_authority_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
) -> Result<()> {
    match protocol.name.as_str() {
        "comms_trust_reconcile" => {
            writeln!(
                out,
                "pub fn effective_peers(obligation: &{obligation_type}) -> std::collections::BTreeSet<crate::meerkat_machine::dsl::PeerEndpoint> {{"
            )?;
            writeln!(
                out,
                "    obligation.direct_peer_endpoints.iter().chain(obligation.mob_overlay_peer_endpoints.iter()).cloned().collect()"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
            writeln!(
                out,
                "pub fn peer_projection_epoch(obligation: &{obligation_type}) -> u64 {{"
            )?;
            writeln!(out, "    obligation.peer_projection_epoch")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
            writeln!(
                out,
                "pub fn authority_for_endpoint(obligation: &{obligation_type}, endpoint: &crate::meerkat_machine::dsl::PeerEndpoint) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
            )?;
            writeln!(
                out,
                "    if !effective_peers(obligation).contains(endpoint) {{"
            )?;
            writeln!(
                out,
                "        return Err(format!(\"MeerkatMachine peer projection did not request trust for peer '{{}}'\", endpoint.peer_id.0));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
            writeln!(
                out,
                "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicAdd,"
            )?;
            writeln!(out, "        endpoint.peer_id.0.as_str(),")?;
            writeln!(
                out,
                "        Some(trusted_peer_descriptor_from_peer_endpoint(endpoint)?),"
            )?;
            writeln!(out, "    )")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
            writeln!(
                out,
                "pub fn removal_authority_for_peer_id(obligation: &{obligation_type}, peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
            )?;
            writeln!(
                out,
                "    if effective_peers(obligation).iter().any(|endpoint| endpoint.peer_id.0 == peer_id) {{"
            )?;
            writeln!(
                out,
                "        return Err(format!(\"MeerkatMachine peer projection still requests trust for peer '{{peer_id}}'\"));"
            )?;
            writeln!(out, "    }}")?;
            writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
            writeln!(
                out,
                "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PublicRemove,"
            )?;
            writeln!(out, "        peer_id,")?;
            writeln!(out, "        None,")?;
            writeln!(out, "    )")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "supervisor_trust_publish" => {
            emit_expected_peer_validator(out)?;
            writeln!(
                out,
                "pub fn publish_authority_for_peer(obligation: &{obligation_type}, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
            )?;
            writeln!(
                out,
                "    validate_expected_peer(\"MeerkatMachineSupervisorPublish\", &obligation.peer_id, expected_peer_id)?;"
            )?;
            writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
            writeln!(
                out,
                "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateAdd,"
            )?;
            writeln!(out, "        expected_peer_id,")?;
            writeln!(
                out,
                "        Some(trusted_peer_descriptor_for_request(obligation, expected_peer_id)?),"
            )?;
            writeln!(out, "    )")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
            writeln!(
                out,
                "pub fn cleanup_authority_for_peer(obligation: &{obligation_type}, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
            )?;
            writeln!(
                out,
                "    validate_expected_peer(\"MeerkatMachineSupervisorPublishCleanup\", &obligation.peer_id, expected_peer_id)?;"
            )?;
            writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
            writeln!(
                out,
                "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,"
            )?;
            writeln!(out, "        obligation.peer_id.as_str(),")?;
            writeln!(out, "        None,")?;
            writeln!(out, "    )")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "supervisor_trust_revoke" => {
            emit_expected_peer_validator(out)?;
            writeln!(
                out,
                "pub fn revoke_authority_for_peer(obligation: &{obligation_type}, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
            )?;
            writeln!(
                out,
                "    validate_expected_peer(\"MeerkatMachineSupervisorRevoke\", &obligation.peer_id, expected_peer_id)?;"
            )?;
            writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
            writeln!(
                out,
                "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::PrivateRemove,"
            )?;
            writeln!(out, "        obligation.peer_id.as_str(),")?;
            writeln!(out, "        None,")?;
            writeln!(out, "    )")?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "mob_member_trust_wiring" => {
            emit_member_trust_helpers(out, obligation_type, true)?;
        }
        "mob_member_trust_unwiring" => {
            emit_member_trust_helpers(out, obligation_type, false)?;
        }
        "mob_member_peer_overlay" => {
            writeln!(
                out,
                "pub fn validate_overlay_freshness(obligation: &{obligation_type}) -> Result<(), String> {{"
            )?;
            writeln!(
                out,
                "    obligation.mob_topology_freshness_authority.validate_topology_epoch(obligation.epoch, false)"
            )?;
            writeln!(out, "}}")?;
            writeln!(out)?;
        }
        "mob_external_peer_trust_wiring" => {
            emit_external_peer_trust_helper(
                out,
                obligation_type,
                "wiring_authority_for_peer",
                "MobMachineExternalPeerWiring",
                true,
            )?;
        }
        "mob_external_peer_trust_unwiring" => {
            emit_external_peer_trust_helper(
                out,
                obligation_type,
                "unwiring_authority_for_peer",
                "MobMachineExternalPeerUnwiring",
                false,
            )?;
        }
        "mob_external_peer_trust_repair" => {
            emit_external_peer_trust_helper(
                out,
                obligation_type,
                "repair_authority_for_peer",
                "MobMachineExternalPeerRepair",
                true,
            )?;
        }
        "mob_external_peer_reciprocal_trust" => {
            emit_external_peer_trust_helper(
                out,
                obligation_type,
                "reciprocal_wiring_authority_for_peer",
                "MobMachineExternalPeerReciprocalWiring",
                true,
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn emit_expected_peer_validator(out: &mut String) -> Result<()> {
    writeln!(
        out,
        "fn validate_expected_peer(context: &'static str, actual: &str, expected: &str) -> Result<(), String> {{"
    )?;
    writeln!(out, "    if actual == expected {{")?;
    writeln!(out, "        Ok(())")?;
    writeln!(out, "    }} else {{")?;
    writeln!(
        out,
        "        Err(format!(\"{{context}} peer id {{actual:?}} does not match expected mutation peer id {{expected:?}}\"))"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_member_trust_helpers(
    out: &mut String,
    obligation_type: &str,
    include_wiring_helpers: bool,
) -> Result<()> {
    emit_expected_peer_validator(out)?;
    writeln!(
        out,
        "fn peer_id_for_identity<'a>(obligation: &'a {obligation_type}, identity: &str) -> Option<&'a str> {{"
    )?;
    writeln!(out, "    if obligation.edge.a.0 == identity {{")?;
    writeln!(out, "        Some(obligation.a_peer_id.0.as_str())")?;
    writeln!(out, "    }} else if obligation.edge.b.0 == identity {{")?;
    writeln!(out, "        Some(obligation.b_peer_id.0.as_str())")?;
    writeln!(out, "    }} else {{")?;
    writeln!(out, "        None")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    writeln!(
        out,
        "fn required_peer_id_for_identity<'a>(obligation: &'a {obligation_type}, identity: &str, expected_peer_id: &str) -> Result<&'a str, String> {{"
    )?;
    writeln!(
        out,
        "    let Some(actual) = peer_id_for_identity(obligation, identity) else {{"
    )?;
    writeln!(
        out,
        "        return Err(format!(\"MobMachine member trust obligation does not cover identity {{identity:?}}\"));"
    )?;
    writeln!(out, "    }};")?;
    writeln!(
        out,
        "    validate_expected_peer(\"MobMachineMemberTrust\", actual, expected_peer_id)?;"
    )?;
    writeln!(out, "    Ok(actual)")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    if include_wiring_helpers {
        emit_member_authority_fn(
            out,
            obligation_type,
            "wiring_authority_for_identity",
            true,
            true,
            false,
        )?;
        emit_member_authority_fn(
            out,
            obligation_type,
            "repair_authority_for_identity",
            true,
            true,
            false,
        )?;
        emit_member_authority_fn(
            out,
            obligation_type,
            "wiring_authority_for_identity_with_live_authority",
            true,
            true,
            true,
        )?;
        emit_member_authority_fn(
            out,
            obligation_type,
            "repair_authority_for_identity_with_live_authority",
            true,
            true,
            true,
        )?;
    } else {
        emit_member_authority_fn(
            out,
            obligation_type,
            "unwiring_authority_for_identity",
            false,
            false,
            false,
        )?;
    }
    Ok(())
}

fn emit_member_authority_fn(
    out: &mut String,
    obligation_type: &str,
    fn_name: &str,
    is_add: bool,
    authorizer_requires_live_arg: bool,
    pass_live_authority: bool,
) -> Result<()> {
    if pass_live_authority {
        writeln!(
            out,
            "pub fn {fn_name}(obligation: &{obligation_type}, identity: &str, expected_peer_id: &str, live_authority: &MobMachineAuthority) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
        )?;
    } else {
        writeln!(
            out,
            "pub fn {fn_name}(obligation: &{obligation_type}, identity: &str, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
        )?;
    }
    writeln!(
        out,
        "    let peer_id = required_peer_id_for_identity(obligation, identity, expected_peer_id)?;"
    )?;
    let operation = if is_add { "PublicAdd" } else { "PublicRemove" };
    writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
    writeln!(
        out,
        "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::{operation},"
    )?;
    writeln!(out, "        peer_id,")?;
    if is_add {
        writeln!(
            out,
            "        Some(trusted_peer_descriptor_for_request(obligation, peer_id)?),"
        )?;
    } else {
        writeln!(out, "        None,")?;
    }
    if authorizer_requires_live_arg {
        if pass_live_authority {
            writeln!(out, "        Some(live_authority),")?;
        } else {
            writeln!(out, "        None,")?;
        }
    }
    writeln!(out, "    )")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_external_peer_trust_helper(
    out: &mut String,
    obligation_type: &str,
    fn_name: &str,
    context: &str,
    is_add: bool,
) -> Result<()> {
    emit_expected_peer_validator(out)?;
    writeln!(
        out,
        "pub fn {fn_name}(obligation: &{obligation_type}, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
    )?;
    writeln!(
        out,
        "    validate_expected_peer(\"{context}\", obligation.peer_id.0.as_str(), expected_peer_id)?;"
    )?;
    let operation = if is_add { "PublicAdd" } else { "PublicRemove" };
    writeln!(out, "    obligation.authorize_comms_trust_authority(")?;
    writeln!(
        out,
        "        meerkat_core::comms::GeneratedCommsTrustAuthorityOperation::{operation},"
    )?;
    writeln!(out, "        expected_peer_id,")?;
    if is_add {
        writeln!(
            out,
            "        Some(trusted_peer_descriptor_for_request(obligation, expected_peer_id)?),"
        )?;
    } else {
        writeln!(out, "        None,")?;
    }
    writeln!(out, "    )")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

/// Emit one `submit_*` helper per feedback input that forwards the
/// obligation through a handle trait method instead of calling
/// `authority.apply` directly.
///
/// The handle-method mapping is declared on `ProtocolRustBinding`:
/// `handle_trait_path` names the trait and `handle_feedback_bindings`
/// maps each typed feedback `input_variant` to its method, forwarded
/// fields, and optional typed obligation-field access paths.
///
/// The generator renders positional arguments in field-bindings-declared
/// order: obligation-sourced fields first, owner-context fields next.
/// The return type is always `Result<(), DslTransitionError>` — the
/// canonical shape of the handle trait.
fn generate_handle_bridge_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
) -> Result<()> {
    let rust = &protocol.rust;
    let handle_trait = short_type(
        rust.handle_trait_path
            .as_deref()
            .context("HandleBridge handle_trait_path missing")?,
    );

    // Use the unqualified `DslTransitionError` symbol and rely on the
    // composition's `required_imports` to bring it into scope. This
    // works for both in-crate (`use crate::handles::DslTransitionError;`)
    // and cross-crate (`use meerkat_core::handles::DslTransitionError;`)
    // emit paths, matching what the hand-authored protocol files did.
    let handle_error_path = "DslTransitionError";

    // When HandleBridge is the primary mode AND a bridge source type is
    // declared, emit the `accept_<effect>` helper that wraps the shell-
    // owned source struct into the obligation token. This is exactly the
    // shape ShellBridge would emit; reusing it lets HandleBridge stand
    // alone for protocols whose consumers speak only through the handle
    // trait (no authority.apply target).
    if rust.generation_mode == ProtocolGenerationMode::HandleBridge
        && let Some(bridge_source_path) = rust.bridge_source_type_path.as_deref()
    {
        let bridge_source = short_type(bridge_source_path);
        emit_accept_helper(out, protocol, obligation_type, bridge_source)?;
    }

    // Stable ordering: feedback entries as declared, which matches the
    // primary-mode output for dual-mode protocols (bit-for-bit parity).
    for feedback in &protocol.allowed_feedback_inputs {
        let handle_binding = rust
            .handle_feedback_binding(&feedback.input_variant)
            .with_context(|| {
                format!(
                    "HandleBridge missing handle_feedback_bindings entry for `{}`",
                    feedback.input_variant
                )
            })?;
        let method_name = &handle_binding.method_name;

        // Dual-mode suffix rule: when HandleBridge is stacked with another
        // mode that *actually* emits an authority-backed `submit_<input>`
        // (i.e., `ShellBridge` or `EffectExtractor` with
        // `authority_type_path` set), append `_handle` to avoid symbol
        // collision. When the stacked mode skips its authority submitter
        // (EffectExtractor with no authority), the bare `submit_*` name
        // is used — no collision to avoid.
        let another_mode_emits_submit = match rust.generation_mode {
            ProtocolGenerationMode::HandleBridge => false,
            ProtocolGenerationMode::ShellBridge => true,
            ProtocolGenerationMode::EffectExtractor => rust.authority_type_path.is_some(),
            ProtocolGenerationMode::Executor => true,
        };
        let fn_name = if another_mode_emits_submit {
            format!(
                "submit_{}_handle",
                to_snake_case(feedback.input_variant.as_str())
            )
        } else {
            format!("submit_{}", to_snake_case(feedback.input_variant.as_str()))
        };

        // When this feedback binding declares `forwarded_fields`, treat
        // it as the authoritative positional argument list — obligation
        // fields not in the list are dropped (they are correlation-only
        // and never reach the handle). Otherwise every obligation-sourced
        // binding is forwarded in declaration order.
        let forwarded: Option<&Vec<meerkat_machine_schema::identity::FieldId>> =
            handle_binding.forwarded_fields.as_ref();
        let mut owner_params: Vec<String> = Vec::new();
        let mut call_args: Vec<String> = Vec::new();
        for binding in &feedback.field_bindings {
            match &binding.source {
                FeedbackFieldSource::ObligationField(field) => {
                    if let Some(allowed) = forwarded
                        && !allowed
                            .iter()
                            .any(|f| to_snake_case(f.as_str()) == to_snake_case(field.as_str()))
                    {
                        continue;
                    }
                    let suffix = handle_binding
                        .arg_accessors
                        .get(field)
                        .cloned()
                        .unwrap_or_default();
                    call_args.push(format!(
                        "obligation.{}{suffix}",
                        to_snake_case(field.as_str())
                    ));
                }
                FeedbackFieldSource::OwnerContext(name) => {
                    let snake = to_snake_case(name);
                    if snake == "cause" {
                        owner_params.push(format!("{snake}: ExternalToolSurfaceFailureCause"));
                        call_args.push(snake);
                    } else {
                        owner_params.push(format!("{snake}: impl Into<String>"));
                        call_args.push(format!("{snake}.into()"));
                    }
                }
            }
        }

        writeln!(
            out,
            "pub fn {fn_name}(handle: &(impl {handle_trait} + ?Sized), obligation: {obligation_type}{}{}) -> Result<(), {handle_error_path}> {{",
            if owner_params.is_empty() { "" } else { ", " },
            owner_params.join(", ")
        )?;
        if call_args.is_empty() {
            writeln!(out, "    handle.{method_name}()")?;
        } else if call_args.len() == 1 {
            writeln!(out, "    handle.{method_name}({})", call_args[0])?;
        } else {
            writeln!(out, "    handle.{method_name}(")?;
            for (idx, arg) in call_args.iter().enumerate() {
                let comma = if idx + 1 == call_args.len() { "" } else { "," };
                writeln!(out, "        {arg}{comma}")?;
            }
            writeln!(out, "    )")?;
        }
        writeln!(out, "}}")?;
        writeln!(out)?;
    }
    Ok(())
}

/// Emit the `accept_<effect>` helper that wraps a shell-owned source
/// struct into the obligation token. Used by `ShellBridge` primary
/// mode and, when a `bridge_source_type_path` is declared, by
/// `HandleBridge` primary mode too.
fn emit_accept_helper(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
    bridge_source: &str,
) -> Result<()> {
    let accept_name = format!("accept_{}", to_snake_case(protocol.effect_variant.as_str()));
    writeln!(
        out,
        "pub fn {accept_name}(source: {bridge_source}) -> {obligation_type} {{"
    )?;
    writeln!(out, "    {obligation_type} {{")?;
    if protocol.obligation_fields.is_empty() {
        writeln!(out, "        _private: (),")?;
    } else {
        for field in &protocol.obligation_fields {
            let rust_field = to_snake_case(field.as_str());
            writeln!(out, "        {rust_field}: source.{rust_field},")?;
        }
    }
    if protocol.comms_trust_authority.is_some() {
        writeln!(
            out,
            "        comms_trust_authority_claims: Default::default(),"
        )?;
    }
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn generate_shell_bridge_helpers(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
    obligation_type: &str,
) -> Result<()> {
    let rust = &protocol.rust;
    let bridge_source = short_type(
        rust.bridge_source_type_path
            .as_deref()
            .context("shell bridge source type missing")?,
    );
    emit_accept_helper(out, protocol, obligation_type, bridge_source)?;

    for feedback in &protocol.allowed_feedback_inputs {
        let target_machine = machine_for_instance(
            composition,
            machine_by_name,
            feedback.machine_instance.as_str(),
        )?;
        generate_feedback_submitter(
            out,
            protocol,
            feedback,
            target_machine
                .inputs
                .variant_named(feedback.input_variant.as_str())?,
            FeedbackReturnKind::Transition(std::marker::PhantomData),
            obligation_type,
        )?;
    }

    Ok(())
}

enum FeedbackReturnKind<'a> {
    Effects,
    Transition(std::marker::PhantomData<&'a MachineSchema>),
}

fn generate_feedback_submitter(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
    target_variant: &meerkat_machine_schema::VariantSchema,
    return_kind: FeedbackReturnKind<'_>,
    obligation_type: &str,
) -> Result<()> {
    let rust = &protocol.rust;
    let authority_type = short_type(
        rust.authority_type_path
            .as_deref()
            .context("feedback authority type missing")?,
    );
    let input_enum = short_type(
        rust.input_enum_path
            .as_deref()
            .context("feedback input enum missing")?,
    );
    let effect_enum = rust.effect_enum_path.as_deref().map(short_type);
    let transition_type = rust.transition_type_path.as_deref().map(short_type);
    let error_type = short_type(
        rust.error_type_path
            .as_deref()
            .context("feedback error type missing")?,
    );
    let owner_params = owner_context_params(target_variant, feedback)?;
    let obligation_param = if feedback
        .field_bindings
        .iter()
        .any(|binding| matches!(binding.source, FeedbackFieldSource::ObligationField(_)))
    {
        "obligation"
    } else {
        "_obligation"
    };
    let fn_name = format!("submit_{}", to_snake_case(feedback.input_variant.as_str()));
    let return_type = match return_kind {
        FeedbackReturnKind::Effects => format!(
            "Result<Vec<{}>, {}>",
            effect_enum.context("feedback effects enum missing")?,
            error_type
        ),
        FeedbackReturnKind::Transition(_) => format!(
            "Result<{}, {}>",
            transition_type.context("feedback transition type missing")?,
            error_type
        ),
    };
    writeln!(
        out,
        "pub fn {fn_name}(authority: &mut {authority_type}, {obligation_param}: {obligation_type}{}{}) -> {return_type} {{",
        if owner_params.is_empty() { "" } else { ", " },
        owner_params.join(", ")
    )?;
    writeln!(
        out,
        "    let transition = authority.apply({}::{})?;",
        input_enum,
        ctor_field_list_from_bindings(
            target_variant,
            feedback,
            rust.input_payload_module_path.as_deref()
        )?
    )?;
    match return_kind {
        FeedbackReturnKind::Effects => writeln!(out, "    Ok(transition.into_effects())")?,
        FeedbackReturnKind::Transition(_) => writeln!(out, "    Ok(transition)")?,
    }
    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn generate_notify_helper(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
    target_variant: &meerkat_machine_schema::VariantSchema,
    return_kind: FeedbackReturnKind<'_>,
) -> Result<()> {
    let rust = &protocol.rust;
    let authority_type = short_type(
        rust.authority_type_path
            .as_deref()
            .context("notify authority type missing")?,
    );
    let input_enum = short_type(
        rust.input_enum_path
            .as_deref()
            .context("notify input enum missing")?,
    );
    let effect_enum = rust.effect_enum_path.as_deref().map(short_type);
    let error_type = short_type(
        rust.error_type_path
            .as_deref()
            .context("notify error type missing")?,
    );
    let owner_params = owner_context_params(target_variant, feedback)?;
    let fn_name = format!("notify_{}", to_snake_case(feedback.input_variant.as_str()));
    let return_type = match return_kind {
        FeedbackReturnKind::Effects => format!(
            "Result<Vec<{}>, {}>",
            effect_enum.context("notify effects enum missing")?,
            error_type
        ),
        FeedbackReturnKind::Transition(_) => format!(
            "Result<{}, {}>",
            short_type(
                protocol
                    .rust
                    .transition_type_path
                    .as_deref()
                    .context("notify transition type missing")?,
            ),
            error_type
        ),
    };

    writeln!(
        out,
        "pub fn {fn_name}(authority: &mut {authority_type}{}{}) -> {return_type} {{",
        if owner_params.is_empty() { "" } else { ", " },
        owner_params.join(", ")
    )?;
    writeln!(
        out,
        "    let transition = authority.apply({}::{})?;",
        input_enum,
        ctor_field_list_from_bindings_without_obligation(target_variant, feedback)?
    )?;
    match return_kind {
        FeedbackReturnKind::Effects => writeln!(out, "    Ok(transition.into_effects())")?,
        FeedbackReturnKind::Transition(_) => writeln!(out, "    Ok(transition)")?,
    }
    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(())
}

fn machine_for_instance<'a>(
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &'a MachineSchema>,
    instance_id: &str,
) -> Result<&'a MachineSchema> {
    let instance = composition
        .machines
        .iter()
        .find(|instance| instance.instance_id.as_str() == instance_id)
        .with_context(|| format!("machine instance `{instance_id}` missing from composition"))?;
    machine_by_name
        .get(instance.machine_name.as_str())
        .copied()
        .with_context(|| format!("machine `{}` missing from registry", instance.machine_name))
}

fn short_type(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

fn rust_type(ty: &TypeRef) -> String {
    match ty {
        TypeRef::Bool => "bool".into(),
        TypeRef::U32 => "u32".into(),
        TypeRef::U64 => "u64".into(),
        TypeRef::String => "String".into(),
        TypeRef::Named(name) => name.as_str().to_string(),
        TypeRef::Enum(name) => name.as_str().to_string(),
        TypeRef::Option(inner) => format!("Option<{}>", rust_type(inner)),
        TypeRef::Set(inner) => format!("std::collections::BTreeSet<{}>", rust_type(inner)),
        TypeRef::Seq(inner) => format!("Vec<{}>", rust_type(inner)),
        TypeRef::Map(key, value) => {
            format!(
                "std::collections::BTreeMap<{}, {}>",
                rust_type(key),
                rust_type(value)
            )
        }
    }
}

fn owner_context_params(
    target_variant: &meerkat_machine_schema::VariantSchema,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
) -> Result<Vec<String>> {
    let mut params = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for binding in &feedback.field_bindings {
        if let FeedbackFieldSource::OwnerContext(name) = &binding.source
            && seen.insert(name.clone())
        {
            let field = target_variant
                .field_named(binding.input_field.as_str())
                .with_context(|| {
                    format!(
                        "missing validated feedback binding field `{}`",
                        binding.input_field
                    )
                })?;
            params.push(format!("{}: {}", to_snake_case(name), rust_type(&field.ty)));
        }
    }
    Ok(params)
}

fn ctor_field_list(variant: &meerkat_machine_schema::VariantSchema) -> String {
    if variant.fields.is_empty() {
        variant.name.as_str().to_string()
    } else {
        let fields = variant
            .fields
            .iter()
            .map(|field| {
                let name = to_snake_case(field.name.as_str());
                if field.name.as_str() == name {
                    name
                } else {
                    format!("{}: {}", field.name, name)
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{} {{ {} }}", variant.name, fields)
    }
}

fn ctor_field_list_from_bindings(
    target_variant: &meerkat_machine_schema::VariantSchema,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
    input_payload_module_path: Option<&str>,
) -> Result<String> {
    if target_variant.fields.is_empty() {
        // Tuple-wrapping input enums (kernel-codegen style) require a
        // payload struct literal even for zero-field variants
        // (`Input::Foo(payload::Foo {})`). DSL-emitted input enums use
        // unit variants for zero-field cases (`Input::Foo`). Fail
        // explicitly rather than emit `Input::Foo` and let rustc
        // complain with a distant error message.
        if let Some(module) = input_payload_module_path {
            bail!(
                "zero-field feedback input variant `{}` cannot be emitted through input_payload_module_path `{module}` (kernel-style tuple wrapping requires at least one field, or remove the payload module path to emit a unit variant)",
                target_variant.name
            );
        }
        return Ok(target_variant.name.as_str().to_string());
    }

    let fields = target_variant
        .fields
        .iter()
        .map(|field| -> Result<String> {
            let binding = feedback
                .field_bindings
                .iter()
                .find(|binding| binding.input_field == field.name)
                .with_context(|| {
                    format!("missing validated feedback binding for `{}`", field.name)
                })?;
            let value = match &binding.source {
                FeedbackFieldSource::ObligationField(source) => {
                    format!("obligation.{}", to_snake_case(source.as_str()))
                }
                FeedbackFieldSource::OwnerContext(name) => to_snake_case(name),
            };
            if field.name.as_str() == value {
                Ok(value)
            } else {
                Ok(format!("{}: {}", field.name, value))
            }
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    // Kernel-style input enums wrap named-field payload structs in tuple
    // variants (`Input::VariantName(payload_module::VariantName { ... })`).
    // DSL-emitted input enums use named-field variants directly.
    match input_payload_module_path {
        Some(module) => Ok(format!(
            "{}({}::{} {{ {} }})",
            target_variant.name, module, target_variant.name, fields
        )),
        None => Ok(format!("{} {{ {} }}", target_variant.name, fields)),
    }
}

fn ctor_field_list_from_bindings_without_obligation(
    target_variant: &meerkat_machine_schema::VariantSchema,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
) -> Result<String> {
    if target_variant.fields.is_empty() {
        return Ok(target_variant.name.as_str().to_string());
    }

    let fields = target_variant
        .fields
        .iter()
        .map(|field| -> Result<String> {
            let binding = feedback
                .field_bindings
                .iter()
                .find(|binding| binding.input_field == field.name)
                .with_context(|| {
                    format!("missing validated feedback binding for `{}`", field.name)
                })?;
            let value = match &binding.source {
                FeedbackFieldSource::OwnerContext(name) => to_snake_case(name),
                FeedbackFieldSource::ObligationField(source) => bail!(
                    "notify helper cannot synthesize obligation field `{source}` for `{}`",
                    field.name
                ),
            };
            if field.name.as_str() == value {
                Ok(value)
            } else {
                Ok(format!("{}: {}", field.name, value))
            }
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    Ok(format!("{} {{ {} }}", target_variant.name, fields))
}

fn match_pattern_for_variant(variant: &meerkat_machine_schema::VariantSchema) -> String {
    if variant.fields.is_empty() {
        format!("::{}", variant.name)
    } else {
        let fields = variant
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("::{} {{ {} }}", variant.name, fields)
    }
}

fn obligation_ctor_expr(
    protocol: &EffectHandoffProtocol,
    obligation_type: &str,
    producer_effect: &meerkat_machine_schema::VariantSchema,
) -> Result<String> {
    if protocol.obligation_fields.is_empty() {
        let mut extra_fields = Vec::new();
        if protocol.comms_trust_authority.is_some() {
            extra_fields.push("comms_trust_authority_claims: Default::default()");
        }
        if protocol.name.as_str() == "comms_trust_reconcile" {
            extra_fields.push(
                "peer_projection_freshness_authority: peer_projection_freshness_authority.clone()",
            );
        }
        if is_supervisor_trust_protocol(protocol.name.as_str()) {
            extra_fields.push(
                "supervisor_trust_freshness_authority: supervisor_trust_freshness_authority.clone()",
            );
        }
        if is_mob_topology_trust_protocol(protocol.name.as_str()) {
            extra_fields
                .push("mob_topology_freshness_authority: mob_topology_freshness_authority.clone()");
        }
        let trust_claims = if extra_fields.is_empty() {
            String::new()
        } else {
            format!(", {}", extra_fields.join(", "))
        };
        return Ok(format!(
            "{obligation_type} {{ _private: (){trust_claims} }}"
        ));
    }

    let mut fields = protocol
        .obligation_fields
        .iter()
        .map(|field| -> Result<String> {
            let rust_name = to_snake_case(field.as_str());
            let effect_field = producer_effect
                .field_named(field.as_str())
                .with_context(|| {
                    format!("obligation field `{field}` missing from producer effect")
                })?;
            Ok(format!(
                "{rust_name}: {}",
                clone_expr_for_type(&effect_field.ty, &rust_name)
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    if protocol.comms_trust_authority.is_some() {
        fields.push("comms_trust_authority_claims: Default::default()".to_string());
    }
    if protocol.name.as_str() == "auth_lease_lifecycle_publication" {
        fields.push(
            "transition_claimed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))"
                .to_string(),
        );
    }
    if protocol.name.as_str() == "comms_trust_reconcile" {
        fields.push(
            "peer_projection_freshness_authority: peer_projection_freshness_authority.clone()"
                .to_string(),
        );
    }
    if is_supervisor_trust_protocol(protocol.name.as_str()) {
        fields.push(
            "supervisor_trust_freshness_authority: supervisor_trust_freshness_authority.clone()"
                .to_string(),
        );
    }
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        fields.push(
            "mob_topology_freshness_authority: mob_topology_freshness_authority.clone()"
                .to_string(),
        );
    }
    let fields = fields.join(", ");
    Ok(format!("{obligation_type} {{ {fields} }}"))
}

fn clone_expr_for_type(ty: &TypeRef, rust_name: &str) -> String {
    match ty {
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::Enum(_) => format!("*{rust_name}"),
        TypeRef::Named(name) if is_known_copy_named_type(name.as_str()) => format!("*{rust_name}"),
        _ => format!("{rust_name}.clone()"),
    }
}

fn is_known_copy_named_type(name: &str) -> bool {
    matches!(name, "TurnNumber" | "SurfaceDeltaOperation")
}

/// Generate a standalone terminal surface mapping module for MeerkatMachine.
///
/// Public for the drift test — ensures the codegen-emit path covers this
/// file alongside the per-protocol helpers, closing the gap the review
/// flagged in the original drift-test implementation.
pub fn render_terminal_surface_mapping(machine: &MachineSchema) -> Result<String> {
    generate_terminal_surface_mapping(machine)
}

fn generate_terminal_surface_mapping(machine: &MachineSchema) -> Result<String> {
    let mut out = String::new();

    writeln!(
        &mut out,
        "// @generated — terminal surface mapping for `{}`",
        machine.machine
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(
        &mut out,
        "// Exhaustive match — adding a new TurnTerminalOutcome or TurnTerminalCauseKind variant forces a compile-time update."
    )?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "use crate::turn_execution_authority::{{TurnTerminalCauseKind, TurnTerminalOutcome}};"
    )?;
    writeln!(&mut out)?;

    // SurfaceResultClass enum
    writeln!(
        &mut out,
        "/// Surface result classification for turn execution terminal outcomes."
    )?;
    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub enum SurfaceResultClass {{")?;
    writeln!(&mut out, "    Success,")?;
    writeln!(&mut out, "    HardFailure,")?;
    writeln!(&mut out, "    Cancelled,")?;
    writeln!(&mut out, "    MissingTerminal,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "/// Normalized terminal cause class for surface classification.",
    )?;
    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(&mut out, "enum TerminalCauseClass {{")?;
    writeln!(&mut out, "    Missing,")?;
    writeln!(&mut out, "    Unknown,")?;
    writeln!(&mut out, "    BudgetExhausted,")?;
    writeln!(&mut out, "    TimeBudgetExceeded,")?;
    writeln!(&mut out, "    RetryExhausted,")?;
    writeln!(&mut out, "    StructuredOutputValidationFailed,")?;
    writeln!(&mut out, "    OtherFailure,")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "fn classify_cause(cause_kind: Option<TurnTerminalCauseKind>) -> TerminalCauseClass {{"
    )?;
    writeln!(&mut out, "    match cause_kind {{")?;
    writeln!(&mut out, "        None => TerminalCauseClass::Missing,")?;
    writeln!(
        &mut out,
        "        Some(TurnTerminalCauseKind::Unknown) => TerminalCauseClass::Unknown,"
    )?;
    writeln!(
        &mut out,
        "        Some(TurnTerminalCauseKind::BudgetExhausted) => TerminalCauseClass::BudgetExhausted,"
    )?;
    writeln!(
        &mut out,
        "        Some(TurnTerminalCauseKind::TimeBudgetExceeded) => TerminalCauseClass::TimeBudgetExceeded,"
    )?;
    writeln!(
        &mut out,
        "        Some(TurnTerminalCauseKind::RetryExhausted) => TerminalCauseClass::RetryExhausted,"
    )?;
    writeln!(
        &mut out,
        "        Some(TurnTerminalCauseKind::StructuredOutputValidationFailed) => TerminalCauseClass::StructuredOutputValidationFailed,"
    )?;
    writeln!(&mut out, "        Some(")?;
    writeln!(&mut out, "            TurnTerminalCauseKind::HookDenied")?;
    writeln!(&mut out, "            | TurnTerminalCauseKind::HookFailure")?;
    writeln!(&mut out, "            | TurnTerminalCauseKind::LlmFailure")?;
    writeln!(&mut out, "            | TurnTerminalCauseKind::ToolFailure")?;
    writeln!(
        &mut out,
        "            | TurnTerminalCauseKind::TurnLimitReached"
    )?;
    writeln!(
        &mut out,
        "            | TurnTerminalCauseKind::RuntimeApplyFailure"
    )?;
    writeln!(
        &mut out,
        "            | TurnTerminalCauseKind::FatalFailure,"
    )?;
    writeln!(&mut out, "        ) => TerminalCauseClass::OtherFailure,")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "/// Exhaustive terminal outcome/cause classification for `{}`.",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "/// No default arm — adding a new `TurnTerminalOutcome` or `TurnTerminalCauseKind` variant forces a compile-time update."
    )?;
    writeln!(&mut out, "pub fn classify_terminal(")?;
    writeln!(&mut out, "    outcome: &TurnTerminalOutcome,")?;
    writeln!(&mut out, "    cause_kind: Option<TurnTerminalCauseKind>,")?;
    writeln!(&mut out, ") -> SurfaceResultClass {{")?;
    writeln!(
        &mut out,
        "    match (*outcome, classify_cause(cause_kind)) {{"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::Missing) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::Unknown) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::None, TerminalCauseClass::OtherFailure) => SurfaceResultClass::MissingTerminal,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::Missing) => SurfaceResultClass::Success,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::Unknown) => SurfaceResultClass::Success,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Completed, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::Missing) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::Unknown) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Failed, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::Missing) => SurfaceResultClass::Cancelled,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::Unknown) => SurfaceResultClass::Cancelled,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::Cancelled, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::Success,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::Missing) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::Unknown) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::BudgetExhausted, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::Missing) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::Unknown) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::TimeBudgetExceeded, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::Missing) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::Unknown) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::BudgetExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::TimeBudgetExceeded) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::RetryExhausted) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::StructuredOutputValidationFailed) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(
        &mut out,
        "        (TurnTerminalOutcome::StructuredOutputValidationFailed, TerminalCauseClass::OtherFailure) => SurfaceResultClass::HardFailure,"
    )?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;

    Ok(out)
}

fn closure_policy_label(policy: &ClosurePolicy) -> &'static str {
    match policy {
        ClosurePolicy::AckRequired => "AckRequired",
        ClosurePolicy::AckOrAbort => "AckOrAbort",
        ClosurePolicy::TerminalClosure => "TerminalClosure",
        ClosurePolicy::PublicationOnly => "PublicationOnly",
    }
}

fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => {
                    let mut result = c.to_uppercase().to_string();
                    result.extend(chars);
                    result
                }
                None => String::new(),
            }
        })
        .collect()
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_ascii_lowercase());
    }
    result
}
