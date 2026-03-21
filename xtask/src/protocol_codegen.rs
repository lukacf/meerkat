use std::fmt::Write;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use meerkat_machine_schema::{
    ClosurePolicy, CompositionSchema, EffectHandoffProtocol, FeedbackFieldSource, MachineSchema,
    ProtocolGenerationMode, TypeRef, canonical_composition_schemas, canonical_machine_schemas,
};

use crate::public_contracts::repo_root;

/// Run protocol codegen: read all canonical compositions, generate Rust helpers
/// for each declared `EffectHandoffProtocol`, plus the terminal surface mapping.
pub fn run_protocol_codegen() -> Result<()> {
    let root = repo_root()?;
    let compositions = canonical_composition_schemas();
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

    // Generate terminal surface mapping for TurnExecutionMachine
    let turn_machine = machine_by_name.get("TurnExecutionMachine");
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

    if generated_count == 0 {
        println!("protocol-codegen: no handoff protocols declared — nothing to generate");
    } else {
        println!("protocol-codegen: generated {generated_count} protocol helper(s)");
    }

    Ok(())
}

fn rustfmt_source(source: &str) -> Result<String> {
    let mut child = Command::new("rustfmt")
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
    root.join(&protocol.rust.module_path)
}

fn generate_protocol_helpers(
    protocol: &EffectHandoffProtocol,
    producer_machine: Option<&MachineSchema>,
    composition: &CompositionSchema,
    machine_by_name: &std::collections::BTreeMap<&str, &MachineSchema>,
) -> Result<String> {
    let mut out = String::new();
    let producer_machine = producer_machine.context("producer machine missing")?;

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

    let obligation_type = generate_obligation_struct(&mut out, protocol, producer_machine)?;

    match protocol.rust.generation_mode {
        ProtocolGenerationMode::Executor => {
            generate_executor_helpers(&mut out, protocol, producer_machine, &obligation_type)?;
        }
        ProtocolGenerationMode::EffectExtractor => generate_effect_extractor_helpers(
            &mut out,
            protocol,
            producer_machine,
            composition,
            machine_by_name,
            &obligation_type,
        )?,
        ProtocolGenerationMode::ShellBridge => generate_shell_bridge_helpers(
            &mut out,
            protocol,
            composition,
            machine_by_name,
            &obligation_type,
        )?,
    }

    Ok(out)
}

fn generate_obligation_struct(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    producer_machine: &MachineSchema,
) -> Result<String> {
    let obligation_type = format!("{}Obligation", to_pascal_case(&protocol.name));
    let producer_effect = producer_machine
        .effects
        .variant_named(&protocol.effect_variant)
        .context("producer effect variant missing")?;

    writeln!(out, "#[derive(Debug, Clone)]")?;
    writeln!(out, "pub struct {obligation_type} {{")?;
    if protocol.obligation_fields.is_empty() {
        writeln!(out, "    _private: (),")?;
    } else {
        for field in &protocol.obligation_fields {
            let effect_field = producer_effect.field_named(field).with_context(|| {
                format!("obligation field `{field}` missing from producer effect")
            })?;
            writeln!(
                out,
                "    pub {}: {},",
                to_snake_case(field),
                rust_type(&effect_field.ty)
            )?;
        }
    }
    writeln!(out, "}}")?;
    writeln!(out)?;

    Ok(obligation_type)
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
        .as_deref()
        .context("executor trigger variant missing")?;
    let trigger_variant = producer_machine
        .inputs
        .variant_named(trigger_variant_name)
        .context("executor trigger variant missing from producer machine")?;
    let producer_effect = producer_machine
        .effects
        .variant_named(&protocol.effect_variant)
        .context("producer effect missing")?;

    let result_type = format!("{}ExecutionResult", to_pascal_case(&protocol.name));
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
        .map(|field| format!("{}: {}", to_snake_case(&field.name), rust_type(&field.ty)))
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
        "    let obligation = transition.effects.iter().find_map(|effect| match effect {{"
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
        "    Ok({result_type} {{ effects: transition.effects, obligation }})"
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
                .variant_named(&feedback.input_variant)?,
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
                    .variant_named(&feedback.input_variant)?,
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
    let producer_effect = producer_machine
        .effects
        .variant_named(&protocol.effect_variant)
        .context("producer effect missing")?;

    writeln!(
        out,
        "pub fn extract_obligations(effects: &[{effect_enum}]) -> Vec<{obligation_type}> {{"
    )?;
    writeln!(out, "    effects")?;
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

    for feedback in &protocol.allowed_feedback_inputs {
        let target_machine =
            machine_for_instance(composition, machine_by_name, &feedback.machine_instance)?;
        generate_feedback_submitter(
            out,
            protocol,
            feedback,
            target_machine
                .inputs
                .variant_named(&feedback.input_variant)?,
            FeedbackReturnKind::Transition(std::marker::PhantomData),
            obligation_type,
        )?;
    }

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
    let accept_name = format!("accept_{}", to_snake_case(&protocol.effect_variant));

    writeln!(
        out,
        "pub fn {accept_name}(source: {bridge_source}) -> {obligation_type} {{"
    )?;
    writeln!(out, "    {obligation_type} {{")?;
    if protocol.obligation_fields.is_empty() {
        writeln!(out, "        _private: (),")?;
    } else {
        for field in &protocol.obligation_fields {
            let rust_field = to_snake_case(field);
            writeln!(out, "        {rust_field}: source.{rust_field},")?;
        }
    }
    writeln!(out, "    }}")?;
    writeln!(out, "}}")?;
    writeln!(out)?;

    for feedback in &protocol.allowed_feedback_inputs {
        let target_machine =
            machine_for_instance(composition, machine_by_name, &feedback.machine_instance)?;
        generate_feedback_submitter(
            out,
            protocol,
            feedback,
            target_machine
                .inputs
                .variant_named(&feedback.input_variant)?,
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
    let fn_name = format!("submit_{}", to_snake_case(&feedback.input_variant));
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
        ctor_field_list_from_bindings(target_variant, feedback)?
    )?;
    match return_kind {
        FeedbackReturnKind::Effects => writeln!(out, "    Ok(transition.effects)")?,
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
    let fn_name = format!("notify_{}", to_snake_case(&feedback.input_variant));
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
        FeedbackReturnKind::Effects => writeln!(out, "    Ok(transition.effects)")?,
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
        .find(|instance| instance.instance_id == instance_id)
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
        TypeRef::Named(name) | TypeRef::Enum(name) => name.clone(),
        TypeRef::Option(inner) => format!("Option<{}>", rust_type(inner)),
        TypeRef::Set(inner) | TypeRef::Seq(inner) => format!("Vec<{}>", rust_type(inner)),
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
                .field_named(&binding.input_field)
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
        variant.name.clone()
    } else {
        let fields = variant
            .fields
            .iter()
            .map(|field| {
                let name = to_snake_case(&field.name);
                if field.name == name {
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
) -> Result<String> {
    if target_variant.fields.is_empty() {
        return Ok(target_variant.name.clone());
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
                    format!("obligation.{}", to_snake_case(source))
                }
                FeedbackFieldSource::OwnerContext(name) => to_snake_case(name),
            };
            if field.name == value {
                Ok(value)
            } else {
                Ok(format!("{}: {}", field.name, value))
            }
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    Ok(format!("{} {{ {} }}", target_variant.name, fields))
}

fn ctor_field_list_from_bindings_without_obligation(
    target_variant: &meerkat_machine_schema::VariantSchema,
    feedback: &meerkat_machine_schema::FeedbackInputRef,
) -> Result<String> {
    if target_variant.fields.is_empty() {
        return Ok(target_variant.name.clone());
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
            if field.name == value {
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
        return Ok(format!("{obligation_type} {{ _private: () }}"));
    }

    let fields = protocol
        .obligation_fields
        .iter()
        .map(|field| -> Result<String> {
            let rust_name = to_snake_case(field);
            let effect_field = producer_effect.field_named(field).with_context(|| {
                format!("obligation field `{field}` missing from producer effect")
            })?;
            Ok(format!(
                "{rust_name}: {}",
                clone_expr_for_type(&effect_field.ty, &rust_name)
            ))
        })
        .collect::<Result<Vec<_>>>()?
        .join(", ");
    Ok(format!("{obligation_type} {{ {fields} }}"))
}

fn clone_expr_for_type(ty: &TypeRef, rust_name: &str) -> String {
    match ty {
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::Enum(_) => format!("*{rust_name}"),
        TypeRef::Named(name) if is_known_copy_named_type(name) => format!("*{rust_name}"),
        _ => format!("{rust_name}.clone()"),
    }
}

fn is_known_copy_named_type(name: &str) -> bool {
    matches!(name, "TurnNumber" | "SurfaceDeltaOperation")
}

/// Generate a standalone terminal surface mapping module for TurnExecutionMachine.
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
        "// Exhaustive match — adding a new TurnTerminalOutcome variant forces a compile-time update."
    )?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "use crate::turn_execution_authority::TurnTerminalOutcome;"
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
    writeln!(&mut out, "}}")?;
    writeln!(&mut out)?;

    writeln!(
        &mut out,
        "/// Exhaustive terminal outcome classification for `{}`.",
        machine.machine
    )?;
    writeln!(
        &mut out,
        "/// No default arm — adding a new `TurnTerminalOutcome` variant forces a compile-time update."
    )?;
    writeln!(
        &mut out,
        "/// Returns `None` when no terminal outcome has been recorded yet."
    )?;
    writeln!(
        &mut out,
        "pub fn classify_terminal(outcome: &TurnTerminalOutcome) -> Option<SurfaceResultClass> {{"
    )?;
    writeln!(&mut out, "    match outcome {{")?;
    writeln!(&mut out, "        TurnTerminalOutcome::None => None,")?;
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Completed => Some(SurfaceResultClass::Success),"
    )?;
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Failed => Some(SurfaceResultClass::HardFailure),"
    )?;
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Cancelled => Some(SurfaceResultClass::Cancelled),"
    )?;
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::BudgetExhausted => Some(SurfaceResultClass::Success),"
    )?;
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::StructuredOutputValidationFailed => Some(SurfaceResultClass::HardFailure),"
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
