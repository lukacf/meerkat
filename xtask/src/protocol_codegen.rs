use std::fmt::Write;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use meerkat_machine_schema::{
    ClosurePolicy, CommsTrustAuthorityOperation, CompositionSchema, EffectHandoffProtocol,
    FeedbackFieldSource, MachineSchema, ProtocolGenerationMode, TypeRef,
    canonical_composition_schemas, canonical_machine_schemas, compat_composition_schemas,
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
    writeln!(&mut out)?;

    for import in &protocol.rust.required_imports {
        writeln!(&mut out, "{import}")?;
    }
    if !protocol.rust.required_imports.is_empty() {
        writeln!(&mut out)?;
    }
    if uses_generated_authority_bridge(protocol) {
        emit_generated_authority_bridge_token(&mut out, protocol)?;
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
        "fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {{"
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
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        emit_mob_topology_freshness_authority(out)?;
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
    writeln!(out, "impl {obligation_type} {{")?;
    writeln!(out, "    #[allow(unsafe_code)]")?;
    writeln!(out, "    pub(crate) fn into_auth_lease_transition(")?;
    writeln!(out, "        &self,")?;
    writeln!(out, "        lease_key: meerkat_core::handles::LeaseKey,")?;
    writeln!(out, "        expires_at: u64,")?;
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
    writeln!(out, "                lease_key,")?;
    writeln!(out, "                expires_at,")?;
    writeln!(out, "                self.credential_generation,")?;
    writeln!(out, "                self.credential_published_at_millis,")?;
    writeln!(out, "            )")?;
    writeln!(out, "        }}")?;
    writeln!(out, "    }}")?;
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
        "        f.debug_struct(\"PeerProjectionFreshnessAuthority\").field(\"present\", &self.authority.is_some()).finish()"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl PeerProjectionFreshnessAuthority {{")?;
    writeln!(
        out,
        "    pub fn from_authority(authority: std::sync::Arc<std::sync::Mutex<crate::meerkat_machine::dsl::MeerkatMachineAuthority>>) -> Self {{"
    )?;
    writeln!(out, "        Self {{ authority: Some(authority) }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    fn missing() -> Self {{")?;
    writeln!(out, "        Self {{ authority: None }}")?;
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

fn emit_mob_topology_freshness_authority(out: &mut String) -> Result<()> {
    writeln!(out, "#[derive(Debug, Clone)]")?;
    writeln!(out, "pub struct MobTopologyFreshnessAuthority {{")?;
    writeln!(
        out,
        "    topology_epoch: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,"
    )?;
    writeln!(out, "}}")?;
    writeln!(out)?;
    writeln!(out, "impl MobTopologyFreshnessAuthority {{")?;
    writeln!(
        out,
        "    pub fn from_authority(authority: &crate::machines::mob_machine::MobMachineAuthority) -> Self {{"
    )?;
    writeln!(
        out,
        "        Self::from_live_topology_epoch(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(authority.state().topology_epoch)))"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    pub fn from_live_topology_epoch(topology_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>) -> Self {{"
    )?;
    writeln!(
        out,
        "        Self {{ topology_epoch: Some(topology_epoch) }}"
    )?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(out, "    fn missing() -> Self {{")?;
    writeln!(out, "        Self {{ topology_epoch: None }}")?;
    writeln!(out, "    }}")?;
    writeln!(out)?;
    writeln!(
        out,
        "    fn validate_topology_epoch(&self, expected_epoch: u64) -> Result<(), String> {{"
    )?;
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
    writeln!(out, "        if current_epoch == expected_epoch {{")?;
    writeln!(out, "            Ok(())")?;
    writeln!(out, "        }} else {{")?;
    writeln!(
        out,
        "            Err(format!(\"stale generated MobMachine trust obligation at epoch {{expected_epoch}} (current {{current_epoch}})\"))"
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
    if is_mob_topology_trust_protocol(protocol.name.as_str()) {
        writeln!(
            out,
            "        self.mob_topology_freshness_authority.validate_topology_epoch(self.epoch)?;"
        )?;
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
                emit_comms_trust_bridge_call(
                    out,
                    source_kind,
                    row_owner_kind,
                    operation_variant,
                    &comms_trust_epoch_expr(protocol)?,
                    "generated_peer_id",
                    "Some(trust_store_peer_id)",
                    "Some(peer_descriptor)",
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
                emit_comms_trust_bridge_call(
                    out,
                    source_kind,
                    row_owner_kind,
                    operation_variant,
                    &comms_trust_epoch_expr(protocol)?,
                    "peer_id.to_string()",
                    "Some(trust_store_peer_id)",
                    "None",
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

fn emit_comms_trust_bridge_call(
    out: &mut String,
    source_kind: &str,
    row_owner_kind: &str,
    operation_variant: &str,
    source_epoch_expr: &str,
    peer_id_expr: &str,
    trust_store_peer_id_expr: &str,
    peer_descriptor_expr: &str,
) -> Result<()> {
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

fn protected_obligation_protocol(name: &str) -> bool {
    matches!(
        name,
        "comms_trust_reconcile"
            | "supervisor_trust_publish"
            | "supervisor_trust_revoke"
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
            | "mob_member_trust_unwiring"
            | "mob_external_peer_trust_wiring"
            | "mob_external_peer_trust_unwiring"
            | "mob_external_peer_trust_repair"
            | "mob_external_peer_reciprocal_trust"
    )
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
        emit_member_authority_fn(out, obligation_type, "wiring_authority_for_identity", true)?;
        emit_member_authority_fn(out, obligation_type, "repair_authority_for_identity", true)?;
    } else {
        emit_member_authority_fn(
            out,
            obligation_type,
            "unwiring_authority_for_identity",
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
) -> Result<()> {
    writeln!(
        out,
        "pub fn {fn_name}(obligation: &{obligation_type}, identity: &str, expected_peer_id: &str) -> Result<meerkat_core::comms::CommsTrustMutationAuthority, String> {{"
    )?;
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
