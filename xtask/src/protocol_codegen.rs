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

    let session_turn_admission_machine =
        dsl::dsl_session_turn_admission_machine_production_schema();
    let code = generate_session_turn_admission_authority(&session_turn_admission_machine)?;
    let code = rustfmt_source(&code)?;
    let output_path = root.join("meerkat-session/src/generated/session_turn_admission.rs");
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

/// Emitter-side surface descriptor for one persistence-version field.
///
/// These are the field-agnostic *naming* facts (the JSON key the field stamps,
/// the public Rust constant/function identifiers, the input/effect-variant
/// names the schema-walk keys off). They are scaffolding — the analogue of the
/// fixed `SurfaceResultClass` enum in `generate_terminal_surface_mapping` — and
/// carry no version DECISIONS. Every version value and every accepted-version
/// fact below is derived mechanically from the machine's transitions.
#[derive(Debug, Clone, Copy)]
struct SessionPersistenceVersionFieldSpec {
    /// `SessionPersistenceVersionField` enum variant this field corresponds to.
    enum_variant: &'static str,
    /// Input variant whose single transition stamps this field's version.
    authorize_input: &'static str,
    /// Input variant whose restore transitions accept this field's versions.
    restore_input: &'static str,
    /// JSON object key this field's version is stamped under / read from.
    json_key: &'static str,
    /// `pub const` identifier holding the current version.
    current_const: &'static str,
    /// `const` identifier holding the legacy default version.
    legacy_const: &'static str,
    /// `pub fn` returning the current version.
    current_accessor: &'static str,
    /// `pub fn` returning the legacy default version.
    legacy_accessor: &'static str,
    /// `pub fn` constructing the authorized stamp for this field.
    authorize_fn: &'static str,
    /// `pub fn` authorizing a restore of an observed version for this field.
    restore_fn: &'static str,
}

/// Fully schema-derived emit plan for one persistence-version field.
///
/// Every value here is read out of the machine's transitions/init block by the
/// walk below — nothing is a literal in the emitter body.
#[derive(Debug, Clone)]
struct SessionPersistenceVersionFieldPlan {
    spec: SessionPersistenceVersionFieldSpec,
    /// Current version, read from the init field the authorize transition
    /// stamps and the restore transitions restore to.
    current_version: u32,
    /// Legacy default version, read from the init field the legacy-restore
    /// transition guards `persisted_version` against.
    legacy_version: u32,
    /// The exact set of observed versions the machine's restore transitions
    /// accept, derived from their `persisted_version` guard targets. For a pure
    /// version encoder this is `{current_version, legacy_version}`.
    accepted_versions: Vec<u32>,
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

#[allow(clippy::too_many_lines)]
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

    // Map every guarded-against state field to its init version. This is the
    // exact set of observed versions the machine's restore transitions accept,
    // read straight out of the schema rather than assumed by the emitter.
    let mut accepted_versions = Vec::with_capacity(accepted_state_fields.len());
    for state_field in &accepted_state_fields {
        accepted_versions.push(session_persistence_init_u32(machine, state_field)?);
    }
    accepted_versions.sort_unstable();
    accepted_versions.dedup();

    Ok(SessionPersistenceVersionFieldPlan {
        spec,
        current_version: session_persistence_init_u32(machine, &current_state_field)?,
        legacy_version: session_persistence_init_u32(machine, &legacy_state_field)?,
        accepted_versions,
    })
}

/// Schema-derived emit plan for the whole persistence-version authority: the
/// closed field-enum variant list (read off the `SessionPersistenceVersionField`
/// string-enum binding) and one fully-derived [`SessionPersistenceVersionFieldPlan`]
/// per field, in surface emission order.
struct SessionPersistenceVersionPlan {
    /// `SessionPersistenceVersionField` variants, in schema declaration order.
    field_enum_variants: Vec<String>,
    /// Per-field plans, in surface emission order (matches the field specs).
    fields: Vec<SessionPersistenceVersionFieldPlan>,
}

/// The emitter-side surface descriptors. Naming only (JSON keys, public
/// identifiers); every version/equality DECISION is derived from the schema.
/// The `enum_variant` of each spec is cross-checked against the schema's
/// `SessionPersistenceVersionField` binding so a renamed/added/removed variant
/// fails the walk rather than silently mis-emitting.
const SESSION_PERSISTENCE_FIELD_SPECS: [SessionPersistenceVersionFieldSpec; 3] = [
    SessionPersistenceVersionFieldSpec {
        enum_variant: "SessionEnvelope",
        authorize_input: "AuthorizeSessionEnvelopeVersionStamp",
        restore_input: "RestoreSessionEnvelopeVersion",
        json_key: "version",
        current_const: "SESSION_VERSION",
        legacy_const: "LEGACY_SESSION_VERSION",
        current_accessor: "session_envelope_version",
        legacy_accessor: "legacy_session_envelope_version",
        authorize_fn: "authorize_session_envelope_version_stamp",
        restore_fn: "restore_session_envelope_version",
    },
    SessionPersistenceVersionFieldSpec {
        enum_variant: "StoredInputState",
        authorize_input: "AuthorizeStoredInputStateVersionStamp",
        restore_input: "RestoreStoredInputStateVersion",
        json_key: "stored_input_state_version",
        current_const: "STORED_INPUT_STATE_VERSION",
        legacy_const: "LEGACY_STORED_INPUT_STATE_VERSION",
        current_accessor: "stored_input_state_version",
        legacy_accessor: "legacy_stored_input_state_version",
        authorize_fn: "authorize_stored_input_state_version_stamp",
        restore_fn: "restore_stored_input_state_version",
    },
    SessionPersistenceVersionFieldSpec {
        enum_variant: "SessionMetadataSchema",
        authorize_input: "AuthorizeSessionMetadataSchemaVersionStamp",
        restore_input: "RestoreSessionMetadataSchemaVersion",
        json_key: "schema_version",
        current_const: "SESSION_METADATA_SCHEMA_VERSION",
        legacy_const: "LEGACY_SESSION_METADATA_SCHEMA_VERSION",
        current_accessor: "session_metadata_schema_version",
        legacy_accessor: "legacy_session_metadata_schema_version",
        authorize_fn: "authorize_session_metadata_schema_version_stamp",
        restore_fn: "restore_session_metadata_schema_version",
    },
];

fn validate_session_persistence_version_authority_schema(
    machine: &MachineSchema,
) -> Result<SessionPersistenceVersionPlan> {
    machine
        .validate()
        .context("validate SessionPersistenceVersionAuthorityMachine schema")?;
    if machine.machine.as_str() != "SessionPersistenceVersionAuthorityMachine" {
        bail!(
            "expected SessionPersistenceVersionAuthorityMachine, got {}",
            machine.machine.as_str()
        );
    }
    for spec in &SESSION_PERSISTENCE_FIELD_SPECS {
        for input in [spec.authorize_input, spec.restore_input] {
            if !machine
                .inputs
                .variants
                .iter()
                .any(|variant| variant.name.as_str() == input)
            {
                bail!("{} missing input `{input}`", machine.machine.as_str());
            }
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
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("SessionPersistenceVersionField must be a string enum");
    };
    let field_enum_variants: Vec<String> = variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect();

    // The emitter's surface specs must cover exactly the schema's field-enum
    // variants — no more, no fewer — so a schema change to the closed field set
    // forces an explicit emitter update instead of a silent partial emit.
    let schema_variants: std::collections::BTreeSet<&str> =
        field_enum_variants.iter().map(String::as_str).collect();
    let spec_variants: std::collections::BTreeSet<&str> = SESSION_PERSISTENCE_FIELD_SPECS
        .iter()
        .map(|spec| spec.enum_variant)
        .collect();
    if schema_variants != spec_variants {
        bail!(
            "SessionPersistenceVersionField variants {schema_variants:?} do not match emitter field specs {spec_variants:?}"
        );
    }

    let mut fields = Vec::with_capacity(SESSION_PERSISTENCE_FIELD_SPECS.len());
    for spec in SESSION_PERSISTENCE_FIELD_SPECS {
        fields.push(session_persistence_derive_field_plan(machine, spec)?);
    }

    Ok(SessionPersistenceVersionPlan {
        field_enum_variants,
        fields,
    })
}

/// Validate that the field's restore transitions accept exactly the version
/// set the shared `restore_version` helper compares against — i.e. the
/// `{current, legacy}` pair derived from the authorize/legacy state fields.
///
/// The generated `restore_*` function delegates the equality to
/// `restore_version` (`observed == current || observed == legacy`); this gate
/// proves that delegation is faithful to the machine's restore transitions, so
/// a schema that started accepting some other version would fail the walk
/// rather than silently emit a now-wrong comparison.
fn session_persistence_accepted_check(plan: &SessionPersistenceVersionFieldPlan) -> Result<()> {
    let mut sorted = vec![plan.current_version, plan.legacy_version];
    sorted.sort_unstable();
    sorted.dedup();
    if plan.accepted_versions != sorted {
        let enum_variant = plan.spec.enum_variant;
        let accepted = &plan.accepted_versions;
        bail!(
            "{enum_variant} restore transitions accept {accepted:?}, but the authorize/legacy state fields resolve to {sorted:?}"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn generate_session_persistence_version_authority(machine: &MachineSchema) -> Result<String> {
    let plan = validate_session_persistence_version_authority_schema(machine)?;

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

    // (1) Current/legacy version constants, one pair per field, every value
    // read from the machine's init block via the field plan.
    for field in &plan.fields {
        writeln!(
            &mut out,
            "pub const {}: u32 = {};",
            field.spec.current_const, field.current_version
        )?;
    }
    for field in &plan.fields {
        writeln!(
            &mut out,
            "const {}: u32 = {};",
            field.spec.legacy_const, field.legacy_version
        )?;
    }
    writeln!(&mut out)?;

    // (2) The closed field enum, emitted from the schema's
    // `SessionPersistenceVersionField` string-enum variants.
    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(&mut out, "pub enum SessionPersistenceVersionField {{")?;
    for variant in &plan.field_enum_variants {
        writeln!(&mut out, "    {variant},")?;
    }
    writeln!(&mut out, "}}")?;

    // (3) Field-agnostic mechanism: the authorized-stamp record, the typed
    // error, and the three pure helpers (`authorize_version_stamp`,
    // `restore_version`, `observed_version_from_json`). These carry no per-field
    // decision, so they are emitted verbatim once.
    out.push_str(SESSION_PERSISTENCE_MECHANISM);

    // (4) Current-version accessors, one per field, returning the field's
    // schema-derived constant.
    for field in &plan.fields {
        writeln!(&mut out)?;
        writeln!(&mut out, "#[must_use]")?;
        writeln!(
            &mut out,
            "pub fn {}() -> u32 {{",
            field.spec.current_accessor
        )?;
        writeln!(&mut out, "    {}", field.spec.current_const)?;
        writeln!(&mut out, "}}")?;
    }

    // (5) Legacy-version accessors, one per field.
    for field in &plan.fields {
        writeln!(&mut out)?;
        writeln!(&mut out, "#[must_use]")?;
        writeln!(
            &mut out,
            "pub fn {}() -> u32 {{",
            field.spec.legacy_accessor
        )?;
        writeln!(&mut out, "    {}", field.spec.legacy_const)?;
        writeln!(&mut out, "}}")?;
    }

    // (6) Authorize-stamp constructors. Each binds the schema field variant to
    // its JSON key and its schema-derived current/legacy constants — the exact
    // stamp the `VersionStampAuthorized` transition emits.
    for field in &plan.fields {
        writeln!(&mut out)?;
        writeln!(
            &mut out,
            "pub fn {}() -> AuthorizedSessionPersistenceVersionStamp {{",
            field.spec.authorize_fn
        )?;
        writeln!(&mut out, "    authorize_version_stamp(")?;
        writeln!(
            &mut out,
            "        SessionPersistenceVersionField::{},",
            field.spec.enum_variant
        )?;
        writeln!(&mut out, "        \"{}\",", field.spec.json_key)?;
        writeln!(&mut out, "        {},", field.spec.current_const)?;
        writeln!(&mut out, "        {},", field.spec.legacy_const)?;
        writeln!(&mut out, "    )")?;
        writeln!(&mut out, "}}")?;
    }

    // (7) Restore authorizers. The accepted-version equality
    // (`observed == current || observed == legacy`) is the body of the shared
    // `restore_version` helper; here we (a) re-validate that the field's restore
    // transitions accept exactly that derived set and (b) feed the field's
    // schema-derived current/legacy constants, restoring to current.
    for field in &plan.fields {
        // Validation gate: fails the walk if the schema's restore transitions
        // accept anything other than `{current, legacy}`.
        session_persistence_accepted_check(field)?;
        writeln!(&mut out)?;
        writeln!(&mut out, "pub fn {}(", field.spec.restore_fn)?;
        writeln!(&mut out, "    observed: u32,")?;
        writeln!(
            &mut out,
            ") -> Result<u32, SessionPersistenceVersionAuthorityError> {{"
        )?;
        writeln!(&mut out, "    restore_version(")?;
        writeln!(
            &mut out,
            "        SessionPersistenceVersionField::{},",
            field.spec.enum_variant
        )?;
        writeln!(&mut out, "        observed,")?;
        writeln!(&mut out, "        {},", field.spec.current_const)?;
        writeln!(&mut out, "        {},", field.spec.legacy_const)?;
        writeln!(&mut out, "    )")?;
        writeln!(&mut out, "}}")?;
    }

    // (8) The JSON stamp applier — field-agnostic, emitted verbatim.
    out.push_str(SESSION_PERSISTENCE_STAMP_APPLIER);

    Ok(out)
}

/// Field-agnostic mechanism shared by every persistence-version field: the
/// authorized-stamp record + accessors, the typed authority error, and the
/// three pure helpers. Carries no per-field decision, so it is emitted once.
const SESSION_PERSISTENCE_MECHANISM: &str = r#"
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
"#;

/// The JSON stamp applier — field-agnostic; takes an already-authorized stamp
/// and writes its current version into the object under its key.
const SESSION_PERSISTENCE_STAMP_APPLIER: &str = r"
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
";

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
    // (Fixed) The durable-marker mechanism: the marker struct, the relation
    // enum, the restore-publication newtype, the phase <-> wire codec, the
    // encode/decode/metadata helpers, and the marker comparison relation. None
    // of these carry a per-contract DECISION — they are parameterised solely by
    // the schema-derived constants emitted above (METADATA_KEY, AUTHORITY,
    // PROTOCOL, SCHEMA_VERSION, and the FIELD_* keys). The comparison relation
    // is the single, fixed `AuthLeaseCredentialPublication` ordering relation
    // selected by the composition durable-marker contract (the only
    // `DurableMarkerRelationProtocol` variant); it is a pure ordering/equality
    // relation with no schema to walk, so it is emitted verbatim once rather
    // than re-derived line by line. The exhaustive `match contract.relation`
    // above is the compile-time guard: this verbatim mechanism is only valid
    // for `AuthLeaseCredentialPublication`, and adding a second relation variant
    // breaks compilation there, forcing this block to be revisited.
    out.push_str(AUTH_LEASE_DURABLE_MARKER_MECHANISM);
    Ok(out)
}

/// Field-agnostic, fixed durable-marker mechanism appended verbatim by
/// [`generate_auth_lease_durable_lifecycle_marker_contract`].
///
/// This is the analogue of [`SESSION_PERSISTENCE_MECHANISM`]: it carries no
/// per-contract decision. Every contract-specific fact (the metadata keys, the
/// authority/protocol identity, the schema version, and the marker field keys)
/// is emitted as a schema-derived `const` ahead of this block; the code here
/// only references those constants. The marker comparison relation is the
/// single fixed `AuthLeaseCredentialPublication` ordering/equality relation —
/// there is no schema to walk for it — so it is an honest hand-written pure
/// relation rendered verbatim, not a reducer pretending to be schema-derived.
const AUTH_LEASE_DURABLE_MARKER_MECHANISM: &str = r#"
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DurableAuthLifecycleMarker {
    pub token_key: TokenKey,
    pub phase: AuthLeasePhase,
    pub expires_at: u64,
    pub generation: u64,
    pub credential_published_at_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthLeaseDurableMarkerRelation {
    Matches,
    TokenNewer,
    TokenStale,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthLeaseDurableRestorePublication {
    token_key: TokenKey,
    phase: AuthLeasePhase,
    expires_at: u64,
    generation: u64,
    credential_published_at_millis: u64,
}

impl AuthLeaseDurableRestorePublication {
    pub fn token_key(&self) -> &TokenKey {
        &self.token_key
    }

    pub fn phase(&self) -> AuthLeasePhase {
        self.phase
    }

    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn credential_published_at_millis(&self) -> u64 {
        self.credential_published_at_millis
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn from_marker_contract(
        token_key: TokenKey,
        phase: AuthLeasePhase,
        expires_at: u64,
        generation: u64,
        credential_published_at_millis: u64,
    ) -> Self {
        Self {
            token_key,
            phase,
            expires_at,
            generation,
            credential_published_at_millis,
        }
    }
}

fn phase_to_wire(phase: AuthLeasePhase) -> &'static str {
    match phase {
        AuthLeasePhase::Valid => "valid",
        AuthLeasePhase::Expiring => "expiring",
        AuthLeasePhase::Expired => "expired",
        AuthLeasePhase::Refreshing => "refreshing",
        AuthLeasePhase::ReauthRequired => "reauth_required",
        AuthLeasePhase::Released => "released",
    }
}

fn phase_from_wire(value: &serde_json::Value) -> Option<AuthLeasePhase> {
    match value.as_str()? {
        "valid" => Some(AuthLeasePhase::Valid),
        "expiring" => Some(AuthLeasePhase::Expiring),
        "expired" => Some(AuthLeasePhase::Expired),
        "refreshing" => Some(AuthLeasePhase::Refreshing),
        "reauth_required" => Some(AuthLeasePhase::ReauthRequired),
        "released" => Some(AuthLeasePhase::Released),
        _ => None,
    }
}

pub(crate) fn encode_marker_value(marker: DurableAuthLifecycleMarker) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert(FIELD_PUBLISHED.to_string(), serde_json::Value::Bool(true));
    map.insert(FIELD_VERSION.to_string(), serde_json::Value::from(SCHEMA_VERSION));
    map.insert(FIELD_AUTHORITY.to_string(), serde_json::Value::String(AUTHORITY.to_string()));
    map.insert(FIELD_PROTOCOL.to_string(), serde_json::Value::String(PROTOCOL.to_string()));
    map.insert(FIELD_REALM.to_string(), serde_json::Value::String(marker.token_key.realm.as_str().to_string()));
    map.insert(FIELD_BINDING.to_string(), serde_json::Value::String(marker.token_key.binding.as_str().to_string()));
    if let Some(profile) = marker.token_key.profile.as_ref() {
        map.insert(FIELD_PROFILE.to_string(), serde_json::Value::String(profile.as_str().to_string()));
    } else {
        map.insert(FIELD_PROFILE.to_string(), serde_json::Value::Null);
    }
    map.insert(FIELD_PHASE.to_string(), serde_json::Value::String(phase_to_wire(marker.phase).to_string()));
    map.insert(FIELD_GENERATION.to_string(), serde_json::Value::from(marker.generation));
    map.insert(FIELD_EXPIRES_AT.to_string(), serde_json::Value::from(marker.expires_at));
    map.insert(FIELD_CREDENTIAL_PUBLISHED_AT_MILLIS.to_string(), serde_json::Value::from(marker.credential_published_at_millis));
    serde_json::Value::Object(map)
}

pub(crate) fn decode_marker_value(value: &serde_json::Value) -> Option<DurableAuthLifecycleMarker> {
    (value.get(FIELD_PUBLISHED)?.as_bool()? == true).then_some(())?;
    (value.get(FIELD_VERSION)?.as_u64()? == SCHEMA_VERSION).then_some(())?;
    (value.get(FIELD_AUTHORITY)?.as_str()? == AUTHORITY).then_some(())?;
    (value.get(FIELD_PROTOCOL)?.as_str()? == PROTOCOL).then_some(())?;
    let token_key = TokenKey::parse_with_profile(
        value.get(FIELD_REALM)?.as_str()?,
        value.get(FIELD_BINDING)?.as_str()?,
        value.get(FIELD_PROFILE).and_then(serde_json::Value::as_str),
    ).ok()?;
    Some(DurableAuthLifecycleMarker {
        token_key,
        phase: phase_from_wire(value.get(FIELD_PHASE)?)?,
        expires_at: value.get(FIELD_EXPIRES_AT)?.as_u64()?,
        generation: value.get(FIELD_GENERATION)?.as_u64()?,
        credential_published_at_millis: value.get(FIELD_CREDENTIAL_PUBLISHED_AT_MILLIS)?.as_u64()?,
    })
}

pub(crate) fn read_marker_from_metadata(metadata: &serde_json::Value) -> Option<DurableAuthLifecycleMarker> {
    decode_marker_value(metadata.get(METADATA_KEY)?)
}

pub(crate) fn metadata_has_valid_marker(metadata: &serde_json::Value) -> bool {
    read_marker_from_metadata(metadata).is_some()
}

pub(crate) fn metadata_with_marker(metadata: &serde_json::Value, marker: DurableAuthLifecycleMarker) -> serde_json::Value {
    let marker = encode_marker_value(marker);
    match metadata {
        serde_json::Value::Object(map) => {
            let mut map = map.clone();
            map.insert(METADATA_KEY.to_string(), marker);
            serde_json::Value::Object(map)
        }
        serde_json::Value::Null => {
            let mut map = serde_json::Map::new();
            map.insert(METADATA_KEY.to_string(), marker);
            serde_json::Value::Object(map)
        }
        other => {
            let mut map = serde_json::Map::new();
            map.insert(METADATA_KEY.to_string(), marker);
            map.insert(PREVIOUS_METADATA_KEY.to_string(), other.clone());
            serde_json::Value::Object(map)
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn restore_publication_from_metadata(metadata: &serde_json::Value, expected_key: &TokenKey) -> Option<AuthLeaseDurableRestorePublication> {
    let marker = read_marker_from_metadata(metadata)?;
    (&marker.token_key == expected_key).then_some(())?;
    Some(AuthLeaseDurableRestorePublication::from_marker_contract(
            marker.token_key,
            marker.phase,
            marker.expires_at,
            marker.generation,
            marker.credential_published_at_millis,
    ))
}

pub fn marker_payload_valid_for_tokens(tokens: &PersistedTokens, expected_key: &TokenKey) -> bool {
    let Some(marker) = read_marker_from_metadata(&tokens.metadata) else {
        return false;
    };
    marker.token_key == *expected_key && marker.expires_at == crate::persisted_token_expires_at_epoch_secs(tokens)
}

// Relation semantics are selected by the composition durable-marker contract.
pub fn marker_relation_for_tokens_and_snapshot(
    tokens: &PersistedTokens,
    snapshot: &AuthLeaseSnapshot,
    expected_key: &TokenKey,
) -> AuthLeaseDurableMarkerRelation {
    let Some(marker) = read_marker_from_metadata(&tokens.metadata) else {
        return AuthLeaseDurableMarkerRelation::Invalid;
    };
    if marker.token_key != *expected_key {
        return AuthLeaseDurableMarkerRelation::Invalid;
    }
    let token_expires_at = crate::persisted_token_expires_at_epoch_secs(tokens);
    if marker.expires_at != token_expires_at {
        return AuthLeaseDurableMarkerRelation::Invalid;
    }
    if !snapshot.credential_present {
        return AuthLeaseDurableMarkerRelation::TokenStale;
    }
    let generation_matches = marker.generation == snapshot.generation;
    let snapshot_expires_at = snapshot.expires_at.unwrap_or(u64::MAX);

    if let Some(snapshot_published_at) = snapshot.credential_published_at_millis {
        return match marker.credential_published_at_millis.cmp(&snapshot_published_at) {
            std::cmp::Ordering::Greater => AuthLeaseDurableMarkerRelation::TokenNewer,
            std::cmp::Ordering::Less => AuthLeaseDurableMarkerRelation::TokenStale,
            std::cmp::Ordering::Equal => {
                if token_expires_at == snapshot_expires_at && generation_matches {
                    AuthLeaseDurableMarkerRelation::Matches
                } else {
                    AuthLeaseDurableMarkerRelation::Invalid
                }
            }
        };
    }

    match token_expires_at.cmp(&snapshot_expires_at) {
        std::cmp::Ordering::Greater => AuthLeaseDurableMarkerRelation::TokenNewer,
        std::cmp::Ordering::Less => AuthLeaseDurableMarkerRelation::TokenStale,
        std::cmp::Ordering::Equal => {
            if generation_matches {
                AuthLeaseDurableMarkerRelation::Matches
            } else {
                AuthLeaseDurableMarkerRelation::Invalid
            }
        }
    }
}
"#;

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
        "#![allow(dead_code, unused_parens, unused_variables, clippy::bool_comparison, clippy::field_reassign_with_default, clippy::nonminimal_bool, clippy::partialeq_to_none, clippy::redundant_clone, clippy::redundant_field_names, clippy::too_many_arguments)]"
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::{{collections::BTreeMap, fmt}};")?;
    writeln!(&mut out)?;

    emit_session_document_key_type(&mut out)?;
    for enum_name in [
        "SessionFirstTurnPhase",
        "SessionInitialPromptStageDecision",
        "SystemContextAppendDecision",
        "SystemContextPersistAppendAdmission",
        "SystemContextSource",
        "RealtimeTranscriptRoleKind",
        "RealtimeTranscriptLaneKind",
        "RealtimeTranscriptStopReasonKind",
        "RealtimeTranscriptMaterializeDecision",
        "SessionDurableProviderKind",
        "SessionToolCategoryOverrideKind",
        "SessionCallTimeoutOverrideKind",
        "SessionSystemPromptSource",
        "ObservedSessionTailKind",
        "PendingContinuationDisposition",
        "PendingContinuationPublicTerminal",
        "ResumeOverrideRejection",
        "ResumeProviderSelection",
        "ResumeSelfHostedSelection",
    ] {
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
        "SystemContextAppendDecision" => Ok("Staged"),
        "SystemContextPersistAppendAdmission" => Ok("Reject"),
        "SystemContextSource" => Ok("Normal"),
        "RealtimeTranscriptRoleKind" => Ok("User"),
        "RealtimeTranscriptLaneKind" => Ok("Display"),
        "RealtimeTranscriptStopReasonKind" => Ok("Other"),
        "RealtimeTranscriptMaterializeDecision" => Ok("Wait"),
        "SessionDurableProviderKind" => Ok("Other"),
        "SessionToolCategoryOverrideKind" => Ok("Inherit"),
        "SessionCallTimeoutOverrideKind" => Ok("Inherit"),
        "SessionSystemPromptSource" => Ok("DirectMutation"),
        "ObservedSessionTailKind" => Ok("Empty"),
        "PendingContinuationDisposition" => Ok("NoPendingBoundary"),
        "PendingContinuationPublicTerminal" => Ok("NoPendingBoundary"),
        "ResumeOverrideRejection" => Ok("ProviderRequiresModel"),
        "ResumeProviderSelection" => Ok("RecomputeFromModel"),
        "ResumeSelfHostedSelection" => Ok("Clear"),
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
    writeln!(out, "impl SessionDocumentError {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new(op: &'static str) -> Self {{")?;
    writeln!(out, "        Self {{ op }}")?;
    writeln!(out, "    }}")?;
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
        "bool"
            | "u32"
            | "u64"
            | "SessionFirstTurnPhase"
            | "SessionInitialPromptStageDecision"
            | "SystemContextAppendDecision"
            | "SystemContextPersistAppendAdmission"
            | "SystemContextSource"
            | "RealtimeTranscriptRoleKind"
            | "RealtimeTranscriptLaneKind"
            | "RealtimeTranscriptStopReasonKind"
            | "RealtimeTranscriptMaterializeDecision"
            | "ObservedSessionTailKind"
            | "PendingContinuationDisposition"
            | "PendingContinuationPublicTerminal"
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
        "ResolveSystemContextAppend",
        "ResolveSystemContextPendingApplyItem",
        "ResolveSystemContextSteerCleanupItem",
        "RestoreSystemContextSnapshot",
        "ResolveRealtimeItemObserved",
        "ResolveRealtimeItemSkipped",
        "ResolveRealtimeUserTranscriptFinal",
        "ResolveRealtimeAssistantDelta",
        "ResolveRealtimeAssistantTextReplacement",
        "ResolveRealtimeAssistantTurnCompleted",
        "ResolveRealtimeAssistantTurnInterrupted",
        "ResolveRealtimeMaterializeCandidate",
        "RestoreRealtimeTranscriptState",
        "AuthorizeSessionMetadataPersist",
        "AuthorizeSessionBuildStatePersist",
        "RestoreSessionBuildState",
        "AuthorizeSystemPromptMutation",
        "ResolvePendingContinuation",
        "AuthorizeSessionResumeOverrides",
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
        "SystemContextAppendResolved",
        "SystemContextPendingApplyItemResolved",
        "SystemContextSteerCleanupItemResolved",
        "SystemContextSnapshotRestoreAuthorized",
        "RealtimeTranscriptEventResolved",
        "RealtimeMaterializeCandidateResolved",
        "RealtimeTranscriptSnapshotRestoreAuthorized",
        "SessionMetadataPersistAuthorized",
        "SessionBuildStatePersistAuthorized",
        "SessionBuildStateRestoreAuthorized",
        "SystemPromptMutationAuthorized",
        "PendingContinuationResolved",
        "PendingContinuationPublicTerminalResolved",
        "SessionResumeOverridesAuthorized",
        "SessionResumeOverridesRejected",
    ] {
        machine
            .effects
            .variant_named(required)
            .with_context(|| format!("SessionDocumentMachine missing effect `{required}`"))?;
    }
    for required in [
        "SessionFirstTurnPhase",
        "SessionInitialPromptStageDecision",
        "SystemContextAppendDecision",
        "SystemContextSource",
        "RealtimeTranscriptRoleKind",
        "RealtimeTranscriptLaneKind",
        "RealtimeTranscriptStopReasonKind",
        "RealtimeTranscriptMaterializeDecision",
        "SessionDurableProviderKind",
        "SessionToolCategoryOverrideKind",
        "SessionCallTimeoutOverrideKind",
        "SessionSystemPromptSource",
        "ObservedSessionTailKind",
        "PendingContinuationDisposition",
        "PendingContinuationPublicTerminal",
        "ResumeOverrideRejection",
        "ResumeProviderSelection",
        "ResumeSelfHostedSelection",
    ] {
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

// =========================================================================
// SessionTurnAdmissionMachine — canonical ephemeral turn-admission authority
// =========================================================================
//
// Pure mechanical schema-walker (modeled on the SessionDocument authority
// emitter). It iterates `transition.from`, `transition.guards`,
// `transition.updates`, and `transition.emit` and lowers each DSL `Expr` /
// `Update` / `EffectEmit` to Rust. It contains NO hand-coded admission
// semantics: every decision (phase legality, interrupt/shutdown intent,
// dispatch authorization, start-turn disposition) lives in the
// SessionTurnAdmissionMachine DSL transitions. The generated file is
// SCHEMA-FREE (no `meerkat_machine_schema` reference) so the owning
// `meerkat-session` crate needs no machine-schema dependency (wasm-clean).
//
// The only domain knowledge here is the set of local decision-enum names and
// the single EXTERNAL enum `PendingContinuationDisposition`, which is owned by
// the canonical SessionDocumentMachine and referenced by path rather than
// re-emitted locally (one semantic fact, one owner).

const STA_LOCAL_ENUMS: &[&str] = &[
    "StartTurnExecutionKind",
    "StartTurnDisposition",
    "StartTurnPublicTerminal",
    "StartTurnDispatchAuthorization",
];
const STA_EXTERNAL_ENUM: &str = "PendingContinuationDisposition";
const STA_EXTERNAL_ENUM_PATH: &str =
    "meerkat_core::session_document::PendingContinuationDisposition";

pub fn render_session_turn_admission_authority(machine: &MachineSchema) -> Result<String> {
    generate_session_turn_admission_authority(machine)
}

fn generate_session_turn_admission_authority(machine: &MachineSchema) -> Result<String> {
    validate_session_turn_admission_authority_schema(machine)?;

    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — session turn admission authority for `{}`",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "// Generated by `xtask protocol-codegen` from `SessionTurnAdmissionMachine`."
    )?;
    writeln!(
        &mut out,
        "#![allow(dead_code, unused_parens, unused_variables, clippy::bool_comparison, clippy::field_reassign_with_default, clippy::nonminimal_bool, clippy::partialeq_to_none, clippy::redundant_field_names, clippy::too_many_arguments)]"
    )?;
    writeln!(&mut out)?;
    writeln!(&mut out, "use std::fmt;")?;
    writeln!(&mut out)?;
    writeln!(&mut out, "pub use {STA_EXTERNAL_ENUM_PATH};")?;
    writeln!(&mut out)?;

    for enum_name in STA_LOCAL_ENUMS {
        emit_sta_named_string_enum(&mut out, machine, enum_name)?;
    }
    emit_sta_variant_enum(
        &mut out,
        "SessionTurnAdmissionInput",
        &machine.inputs.variants,
    )?;
    emit_sta_variant_enum(
        &mut out,
        "SessionTurnAdmissionEffect",
        &machine.effects.variants,
    )?;
    emit_sta_error(&mut out)?;
    emit_sta_phase_enum(&mut out, machine)?;
    emit_sta_state_struct(&mut out, machine)?;
    emit_sta_transition_enum(&mut out, machine)?;
    emit_sta_authority_impl(&mut out, machine)?;
    for helper in &machine.helpers {
        emit_sta_helper(&mut out, helper, machine)?;
    }
    writeln!(
        &mut out,
        "impl Default for SessionTurnAdmissionMachineAuthority {{"
    )?;
    writeln!(&mut out, "    fn default() -> Self {{")?;
    writeln!(&mut out, "        Self::new()")?;
    writeln!(&mut out, "    }}")?;
    writeln!(&mut out, "}}")?;
    Ok(out)
}

fn sta_default_variant(name: &str) -> Result<&'static str> {
    match name {
        "StartTurnExecutionKind" => Ok("ContentTurn"),
        "StartTurnDisposition" => Ok("RunContentTurn"),
        "StartTurnPublicTerminal" => Ok("NoPendingBoundary"),
        "StartTurnDispatchAuthorization" => Ok("Authorized"),
        other => bail!("unknown SessionTurnAdmissionMachine enum `{other}`"),
    }
}

fn emit_sta_named_string_enum(out: &mut String, machine: &MachineSchema, name: &str) -> Result<()> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| format!("SessionTurnAdmissionMachine missing named type `{name}`"))?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!("SessionTurnAdmissionMachine named type `{name}` must be a string enum");
    };
    let variants = variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect::<Vec<_>>();
    let default_variant = sta_default_variant(name)?;
    if !variants.iter().any(|variant| variant == default_variant) {
        bail!(
            "SessionTurnAdmissionMachine enum `{name}` missing default variant `{default_variant}`"
        );
    }
    emit_string_enum(out, name, &variants, default_variant)
}

fn emit_sta_variant_enum(
    out: &mut String,
    enum_name: &str,
    variants: &[VariantSchema],
) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
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
                render_sta_type_ref(&field.ty)?
            )?;
        }
        writeln!(out, "    }},")?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_error(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, PartialEq, Eq)]")?;
    writeln!(out, "pub struct SessionTurnAdmissionError {{")?;
    writeln!(out, "    op: &'static str,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl SessionTurnAdmissionError {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new(op: &'static str) -> Self {{")?;
    writeln!(out, "        Self {{ op }}")?;
    writeln!(out, "    }}")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn op(&self) -> &'static str {{")?;
    writeln!(out, "        self.op")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl fmt::Display for SessionTurnAdmissionError {{")?;
    writeln!(
        out,
        "    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{"
    )?;
    writeln!(
        out,
        "        write!(f, \"generated session turn admission authority rejected {{}}\", self.op)"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(
        out,
        "impl std::error::Error for SessionTurnAdmissionError {{}}"
    )?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_phase_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
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

fn emit_sta_state_struct(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]")?;
    writeln!(out, "pub struct SessionTurnAdmissionMachineState {{")?;
    writeln!(
        out,
        "    lifecycle_phase: {},",
        machine.state.phase.name.as_str()
    )?;
    for field in &machine.state.fields {
        writeln!(
            out,
            "    pub {}: {},",
            field.name,
            render_sta_type_ref(&field.ty)?
        )?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl SessionTurnAdmissionMachineState {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn phase(&self) -> {} {{",
        machine.state.phase.name.as_str()
    )?;
    writeln!(out, "        self.lifecycle_phase")?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_transition_enum(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "enum SessionTurnAdmissionTransition {{")?;
    for transition in &machine.transitions {
        writeln!(out, "    {},", transition.name)?;
    }
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_authority_impl(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]")?;
    writeln!(out, "pub struct SessionTurnAdmissionMachineAuthority {{")?;
    writeln!(out, "    state: SessionTurnAdmissionMachineState,")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl SessionTurnAdmissionMachineAuthority {{")?;
    writeln!(out, "    #[must_use]")?;
    writeln!(out, "    pub fn new() -> Self {{")?;
    writeln!(
        out,
        "        let mut state = SessionTurnAdmissionMachineState::default();"
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
            render_sta_owned_expr(&init.expr, &std::collections::BTreeMap::new(), machine)?
        )?;
    }
    writeln!(out, "        Self {{ state }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    #[must_use]")?;
    writeln!(
        out,
        "    pub fn state(&self) -> &SessionTurnAdmissionMachineState {{"
    )?;
    writeln!(out, "        &self.state")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    emit_sta_single_transition(out)?;
    emit_sta_apply_input(out, machine)?;
    emit_sta_public_methods(out, machine)?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_single_transition(out: &mut String) -> Result<()> {
    writeln!(out, "    fn single_transition(")?;
    writeln!(out, "        matches: Vec<SessionTurnAdmissionTransition>,")?;
    writeln!(out, "        op: &'static str,")?;
    writeln!(
        out,
        "    ) -> Result<SessionTurnAdmissionTransition, SessionTurnAdmissionError> {{"
    )?;
    writeln!(out, "        let mut matches = matches.into_iter();")?;
    writeln!(out, "        let Some(first) = matches.next() else {{")?;
    writeln!(
        out,
        "            return Err(SessionTurnAdmissionError {{ op }});"
    )?;
    writeln!(out, "        }};")?;
    writeln!(out, "        if matches.next().is_some() {{")?;
    writeln!(
        out,
        "            return Err(SessionTurnAdmissionError {{ op }});"
    )?;
    writeln!(out, "        }}")?;
    writeln!(out, "        Ok(first)")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_apply_input(out: &mut String, machine: &MachineSchema) -> Result<()> {
    writeln!(out, "    fn apply_input(")?;
    writeln!(out, "        &mut self,")?;
    writeln!(out, "        input: SessionTurnAdmissionInput,")?;
    writeln!(
        out,
        "    ) -> Result<Vec<SessionTurnAdmissionEffect>, SessionTurnAdmissionError> {{"
    )?;
    writeln!(out, "        match input {{")?;
    for input_variant in &machine.inputs.variants {
        emit_sta_apply_input_arm(out, machine, input_variant)?;
    }
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    Ok(())
}

fn emit_sta_apply_input_arm(
    out: &mut String,
    machine: &MachineSchema,
    input_variant: &VariantSchema,
) -> Result<()> {
    write!(
        out,
        "            SessionTurnAdmissionInput::{}",
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

    let binding_types = sta_binding_types(input_variant);
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
            render_sta_transition_condition(transition, &binding_types, machine)?
        )?;
        writeln!(
            out,
            "                    matches.push(SessionTurnAdmissionTransition::{});",
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
        emit_sta_transition_apply_block(out, transition, &binding_types, machine)?;
    }
    writeln!(
        out,
        "                    #[allow(unreachable_patterns)] _ => Err(SessionTurnAdmissionError {{ op: \"{}_transition\" }}),",
        input_variant.name
    )?;
    writeln!(out, "                }}")?;
    writeln!(out, "            }}")?;
    Ok(())
}

fn emit_sta_transition_apply_block(
    out: &mut String,
    transition: &TransitionSchema,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<()> {
    writeln!(
        out,
        "                    SessionTurnAdmissionTransition::{} => {{",
        transition.name
    )?;
    for update in &transition.updates {
        writeln!(
            out,
            "                        {}",
            render_sta_update(update, binding_types, machine)?
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
                render_sta_effect_emit(effect, binding_types, machine)?
            )?;
        }
        writeln!(out, "                        ])")?;
    }
    writeln!(out, "                    }}")?;
    Ok(())
}

fn emit_sta_public_methods(out: &mut String, machine: &MachineSchema) -> Result<()> {
    for input in &machine.inputs.variants {
        let method = to_snake_case(input.name.as_str());
        writeln!(out, "    pub fn {method}(")?;
        writeln!(out, "        &mut self,")?;
        for field in &input.fields {
            writeln!(
                out,
                "        {}: {},",
                field.name,
                render_sta_type_ref(&field.ty)?
            )?;
        }
        writeln!(
            out,
            "    ) -> Result<Vec<SessionTurnAdmissionEffect>, SessionTurnAdmissionError> {{"
        )?;
        if input.fields.is_empty() {
            writeln!(
                out,
                "        self.apply_input(SessionTurnAdmissionInput::{})",
                input.name
            )?;
        } else {
            writeln!(
                out,
                "        self.apply_input(SessionTurnAdmissionInput::{} {{",
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

fn emit_sta_helper(out: &mut String, helper: &HelperSchema, machine: &MachineSchema) -> Result<()> {
    write!(out, "fn {}(", helper.name)?;
    for (idx, param) in helper.params.iter().enumerate() {
        if idx > 0 {
            write!(out, ", ")?;
        }
        write!(
            out,
            "{}: {}",
            param.name.as_str(),
            render_sta_type_ref(&param.ty)?
        )?;
    }
    writeln!(out, ") -> {} {{", render_sta_type_ref(&helper.returns)?)?;
    let binding_types = helper
        .params
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect();
    writeln!(
        out,
        "    {}",
        render_sta_expr(&helper.body, &binding_types, machine)?
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    Ok(())
}

fn render_sta_type_ref(ty: &TypeRef) -> Result<String> {
    match ty {
        TypeRef::Bool => Ok("bool".to_string()),
        TypeRef::U32 => Ok("u32".to_string()),
        TypeRef::U64 => Ok("u64".to_string()),
        TypeRef::Enum(name) => Ok(name.as_str().to_string()),
        TypeRef::Named(name) => Ok(name.as_str().to_string()),
        TypeRef::Option(inner) => Ok(format!("Option<{}>", render_sta_type_ref(inner)?)),
        other => bail!("unsupported SessionTurnAdmissionMachine type `{other:?}`"),
    }
}

fn render_sta_transition_condition(
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
    } else if !transition.from.is_empty()
        && transition.from.len() != machine.state.phase.variants.len()
    {
        // A transition legal in every phase needs no phase guard; otherwise
        // gate on the explicit `per_phase` set.
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
        conditions.push(render_sta_expr(&guard.expr, binding_types, machine)?);
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

fn render_sta_update(
    update: &Update,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match update {
        Update::Assign { field, expr } => Ok(format!(
            "self.state.{field} = {};",
            render_sta_owned_expr(expr, binding_types, machine)?
        )),
        other => bail!("unsupported SessionTurnAdmissionMachine update `{other:?}`"),
    }
}

fn render_sta_effect_emit(
    effect: &EffectEmit,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    let variant = machine
        .effects
        .variant_named(effect.variant.as_str())
        .with_context(|| {
            format!(
                "SessionTurnAdmissionMachine effect `{}` missing from schema",
                effect.variant
            )
        })?;
    if effect.fields.is_empty() {
        return Ok(format!("SessionTurnAdmissionEffect::{}", effect.variant));
    }
    let mut rendered = format!("SessionTurnAdmissionEffect::{} {{", effect.variant);
    for (field, expr) in &effect.fields {
        variant.field_named(field.as_str()).with_context(|| {
            format!(
                "SessionTurnAdmissionMachine effect `{}` missing field `{}`",
                effect.variant, field
            )
        })?;
        write!(
            &mut rendered,
            " {field}: {},",
            render_sta_expr(expr, binding_types, machine)?
        )?;
    }
    rendered.push_str(" }");
    Ok(rendered)
}

fn render_sta_expr(
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
            render_sta_expr(inner, binding_types, machine)?
        )),
        Expr::Not(inner) => Ok(format!(
            "!({})",
            render_sta_expr(inner, binding_types, machine)?
        )),
        Expr::And(items) => render_sta_expr_joined(items, " && ", binding_types, machine),
        Expr::Or(items) => render_sta_expr_joined(items, " || ", binding_types, machine),
        Expr::Eq(left, right) => Ok(format!(
            "{} == {}",
            render_sta_expr(left, binding_types, machine)?,
            render_sta_expr(right, binding_types, machine)?
        )),
        Expr::Neq(left, right) => Ok(format!(
            "{} != {}",
            render_sta_expr(left, binding_types, machine)?,
            render_sta_expr(right, binding_types, machine)?
        )),
        Expr::Gt(left, right) => Ok(format!(
            "{} > {}",
            render_sta_expr(left, binding_types, machine)?,
            render_sta_expr(right, binding_types, machine)?
        )),
        Expr::Call { helper, args } => {
            let rendered_args = args
                .iter()
                .map(|arg| render_sta_expr(arg, binding_types, machine))
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            Ok(format!("{helper}({rendered_args})"))
        }
        other => bail!("unsupported SessionTurnAdmissionMachine expression `{other:?}`"),
    }
}

fn render_sta_expr_joined(
    items: &[Expr],
    separator: &str,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    if items.is_empty() {
        bail!("SessionTurnAdmissionMachine expression cannot join an empty list");
    }
    let rendered = items
        .iter()
        .map(|expr| {
            Ok(format!(
                "({})",
                render_sta_expr(expr, binding_types, machine)?
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rendered.join(separator))
}

fn render_sta_owned_expr(
    expr: &Expr,
    binding_types: &std::collections::BTreeMap<String, TypeRef>,
    machine: &MachineSchema,
) -> Result<String> {
    match expr {
        Expr::None => Ok("None".to_string()),
        Expr::Some(inner) => Ok(format!(
            "Some({})",
            render_sta_owned_expr(inner, binding_types, machine)?
        )),
        Expr::Bool(value) => Ok(value.to_string()),
        Expr::U64(value) => Ok(value.to_string()),
        Expr::NamedVariant { enum_name, variant } => {
            Ok(format!("{}::{}", enum_name.as_str(), variant.as_str()))
        }
        Expr::Binding(binding) => Ok(binding.clone()),
        other => render_sta_expr(other, binding_types, machine),
    }
}

fn sta_binding_types(variant: &VariantSchema) -> std::collections::BTreeMap<String, TypeRef> {
    variant
        .fields
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field.ty.clone()))
        .collect()
}

fn validate_session_turn_admission_authority_schema(machine: &MachineSchema) -> Result<()> {
    machine
        .validate()
        .context("validate SessionTurnAdmissionMachine schema")?;
    if machine.machine.as_str() != "SessionTurnAdmissionMachine" {
        bail!(
            "session turn admission generator received unexpected machine `{}`",
            machine.machine
        );
    }
    // Every local decision enum must be a string enum present in the schema.
    for required in STA_LOCAL_ENUMS {
        let binding = machine
            .named_types
            .iter()
            .find(|binding| binding.name.as_str() == *required)
            .with_context(|| {
                format!("SessionTurnAdmissionMachine missing named type `{required}`")
            })?;
        if !matches!(binding.rust, RustTypeAtom::StringEnum { .. }) {
            bail!("SessionTurnAdmissionMachine named type `{required}` must be a string enum");
        }
    }
    // The external pending-continuation enum must be declared (it is referenced
    // by path, owned by SessionDocumentMachine).
    machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == STA_EXTERNAL_ENUM)
        .with_context(|| {
            format!("SessionTurnAdmissionMachine missing external named type `{STA_EXTERNAL_ENUM}`")
        })?;
    Ok(())
}

/// Shared schema-walking helper: emit a `#[derive(...)]` C-like string enum
/// with the given default variant. Used by the approval-lifecycle,
/// session-document, and session-turn-admission authority emitters.
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

/// Optional-cause domain element: either the absent (`None`) cause or one of
/// the closed `TurnTerminalCauseKind` variants.
#[derive(Debug, Clone)]
enum OptionalCauseValue {
    Absent,
    Variant(String),
}

/// Statically evaluate a MeerkatMachine guard `Expr` against a fixed binding
/// environment of `name -> {Absent | NamedVariant}` values.
///
/// Only the closed form produced by the surface-result classification
/// transitions is supported: equalities of a destructure binding against a
/// named-variant literal (optionally wrapped in `Some(...)`) or `None`, joined
/// by `&&`/`||`. Any other shape is a schema authoring error and is rejected so
/// the codegen cannot silently mis-derive policy.
fn eval_surface_guard(
    expr: &Expr,
    env: &std::collections::BTreeMap<&str, OptionalCauseValue>,
) -> Result<bool> {
    match expr {
        Expr::And(items) => {
            for item in items {
                if !eval_surface_guard(item, env)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Expr::Or(items) => {
            for item in items {
                if eval_surface_guard(item, env)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Expr::Eq(left, right) => {
            let Expr::Binding(binding) = left.as_ref() else {
                bail!(
                    "surface-result guard equality must have a destructure binding on the left, got {left:?}"
                );
            };
            let actual = env.get(binding.as_str()).with_context(|| {
                format!("surface-result guard references unknown binding `{binding}`")
            })?;
            match right.as_ref() {
                Expr::None => Ok(matches!(actual, OptionalCauseValue::Absent)),
                Expr::NamedVariant { variant, .. } => Ok(matches!(
                    actual,
                    OptionalCauseValue::Variant(v) if v == variant.as_str()
                )),
                Expr::Some(inner) => {
                    let Expr::NamedVariant { variant, .. } = inner.as_ref() else {
                        bail!(
                            "surface-result guard `Some(..)` must wrap a named variant, got {inner:?}"
                        );
                    };
                    Ok(matches!(
                        actual,
                        OptionalCauseValue::Variant(v) if v == variant.as_str()
                    ))
                }
                other => bail!(
                    "surface-result guard equality right-hand side must be a named variant, `Some(variant)`, or `None`, got {other:?}"
                ),
            }
        }
        other => bail!(
            "unsupported surface-result guard expression `{other:?}`; only `&&`/`||` of binding equalities are allowed"
        ),
    }
}

/// Read the closed variant list of a `StringEnum` named type from the schema.
fn surface_string_enum_variants(machine: &MachineSchema, name: &str) -> Result<Vec<String>> {
    let binding = machine
        .named_types
        .iter()
        .find(|binding| binding.name.as_str() == name)
        .with_context(|| {
            format!(
                "{} missing named type `{name}` required for terminal surface mapping",
                machine.machine.as_str()
            )
        })?;
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        bail!(
            "{} named type `{name}` must be a string enum for terminal surface mapping",
            machine.machine.as_str()
        );
    };
    Ok(variants
        .iter()
        .map(|variant| variant.as_str().to_owned())
        .collect())
}

/// Collect the transitions firing on a given input variant.
fn surface_transitions_for_input<'a>(
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

/// Read the single named-variant value emitted into `effect.field` by a
/// classification transition (mechanical projection of the machine's decision).
fn surface_emitted_named_variant(
    transition: &TransitionSchema,
    effect_variant: &str,
    field: &str,
) -> Result<String> {
    let emit = match transition.emit.as_slice() {
        [emit] => emit,
        _ => bail!(
            "surface classification transition `{}` must emit exactly one effect",
            transition.name.as_str()
        ),
    };
    if emit.variant.as_str() != effect_variant {
        bail!(
            "surface classification transition `{}` emitted `{}`, expected `{effect_variant}`",
            transition.name.as_str(),
            emit.variant.as_str()
        );
    }
    let expr = emit
        .fields
        .iter()
        .find_map(|(candidate, expr)| (candidate.as_str() == field).then_some(expr))
        .with_context(|| {
            format!(
                "surface classification transition `{}` effect missing field `{field}`",
                transition.name.as_str()
            )
        })?;
    let Expr::NamedVariant { variant, .. } = expr else {
        bail!(
            "surface classification transition `{}` field `{field}` must emit a named variant, got {expr:?}",
            transition.name.as_str()
        );
    };
    Ok(variant.as_str().to_owned())
}

/// Resolve the unique transition (and its emitted class) whose guards match a
/// fixed binding environment. Fails closed on gaps or overlaps so the derived
/// table is provably total and unambiguous.
fn surface_resolve_unique(
    transitions: &[&TransitionSchema],
    env: &std::collections::BTreeMap<&str, OptionalCauseValue>,
    effect_variant: &str,
    field: &str,
    cell_description: &str,
) -> Result<String> {
    let mut matched: Option<String> = None;
    for transition in transitions {
        let mut all = true;
        for guard in &transition.guards {
            if !eval_surface_guard(&guard.expr, env)? {
                all = false;
                break;
            }
        }
        if !all {
            continue;
        }
        let emitted = surface_emitted_named_variant(transition, effect_variant, field)?;
        if let Some(existing) = &matched {
            bail!(
                "terminal surface mapping is ambiguous for {cell_description}: both `{existing}` and \
                 `{emitted}` match (transition `{}`)",
                transition.name.as_str()
            );
        }
        matched = Some(emitted);
    }
    matched.with_context(|| {
        format!("terminal surface mapping has no transition covering {cell_description}")
    })
}

fn generate_terminal_surface_mapping(machine: &MachineSchema) -> Result<String> {
    // Closed domains owned by MeerkatMachine.
    let outcomes = surface_string_enum_variants(machine, "TurnTerminalOutcome")?;
    let cause_kinds = surface_string_enum_variants(machine, "TurnTerminalCauseKind")?;
    let cause_classes = surface_string_enum_variants(machine, "TerminalCauseClass")?;
    let _surface_classes = surface_string_enum_variants(machine, "SurfaceResultClass")?;

    let cause_class_transitions =
        surface_transitions_for_input(machine, "ClassifyTurnTerminalCauseClass");
    if cause_class_transitions.is_empty() {
        bail!(
            "{} missing `ClassifyTurnTerminalCauseClass` transitions for terminal surface mapping",
            machine.machine.as_str()
        );
    }
    let surface_transitions = surface_transitions_for_input(machine, "ResolveTurnSurfaceResult");
    if surface_transitions.is_empty() {
        bail!(
            "{} missing `ResolveTurnSurfaceResult` transitions for terminal surface mapping",
            machine.machine.as_str()
        );
    }

    // Derive `cause_kind -> TerminalCauseClass` for the absent cause and every
    // closed cause variant by querying the machine's classification transitions.
    let mut cause_kind_class: Vec<(OptionalCauseValue, String)> =
        Vec::with_capacity(cause_kinds.len() + 1);
    for value in std::iter::once(OptionalCauseValue::Absent).chain(
        cause_kinds
            .iter()
            .map(|v| OptionalCauseValue::Variant(v.clone())),
    ) {
        let mut env = std::collections::BTreeMap::new();
        env.insert("cause_kind", value.clone());
        let description = match &value {
            OptionalCauseValue::Absent => "cause_kind None".to_string(),
            OptionalCauseValue::Variant(v) => format!("cause_kind Some({v})"),
        };
        let class = surface_resolve_unique(
            &cause_class_transitions,
            &env,
            "TurnTerminalCauseClassResolved",
            "cause_class",
            &description,
        )?;
        cause_kind_class.push((value, class));
    }

    // Derive `(outcome, cause_class) -> SurfaceResultClass` over the full cross
    // product by querying the machine's surface-result transitions.
    let mut surface_table: Vec<(String, String, String)> =
        Vec::with_capacity(outcomes.len() * cause_classes.len());
    for outcome in &outcomes {
        for cause_class in &cause_classes {
            let mut env = std::collections::BTreeMap::new();
            env.insert("outcome", OptionalCauseValue::Variant(outcome.clone()));
            env.insert(
                "cause_class",
                OptionalCauseValue::Variant(cause_class.clone()),
            );
            let description = format!("(outcome {outcome}, cause class {cause_class})");
            let surface_class = surface_resolve_unique(
                &surface_transitions,
                &env,
                "TurnSurfaceResultResolved",
                "surface_class",
                &description,
            )?;
            surface_table.push((outcome.clone(), cause_class.clone(), surface_class));
        }
    }

    // Emit the derived module.
    let mut out = String::new();
    writeln!(
        &mut out,
        "// @generated — terminal surface mapping for `{}`",
        machine.machine
    )?;
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`")?;
    writeln!(
        &mut out,
        "// Derived mechanically from `{}` `ClassifyTurnTerminalCauseClass` / `ResolveTurnSurfaceResult` transitions.",
        machine.machine
    )?;
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
    for cause_class in &cause_classes {
        writeln!(&mut out, "    {cause_class},")?;
    }
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "fn classify_cause(cause_kind: Option<TurnTerminalCauseKind>) -> TerminalCauseClass {{"
    )?;
    writeln!(&mut out, "    match cause_kind {{")?;
    for (value, class) in &cause_kind_class {
        let pattern = match value {
            OptionalCauseValue::Absent => "None".to_string(),
            OptionalCauseValue::Variant(v) => format!("Some(TurnTerminalCauseKind::{v})"),
        };
        writeln!(
            &mut out,
            "        {pattern} => TerminalCauseClass::{class},"
        )?;
    }
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
    for (outcome, cause_class, surface_class) in &surface_table {
        writeln!(
            &mut out,
            "        (TurnTerminalOutcome::{outcome}, TerminalCauseClass::{cause_class}) => SurfaceResultClass::{surface_class},"
        )?;
    }
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
