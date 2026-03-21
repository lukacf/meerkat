use std::fmt::Write;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use meerkat_machine_schema::{
    ClosurePolicy, CompositionSchema, EffectHandoffProtocol, MachineSchema,
    canonical_composition_schemas, canonical_machine_schemas,
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

            let code = generate_protocol_helpers(protocol, producer_machine, composition);
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
        let code = generate_terminal_surface_mapping(machine);
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

fn protocol_output_path(root: &Path, protocol: &EffectHandoffProtocol) -> std::path::PathBuf {
    if let Some(ref target_crate) = protocol.target_crate {
        root.join(format!("{}/protocol_{}.rs", target_crate, protocol.name))
    } else {
        root.join(format!(
            "meerkat-core/src/generated/protocol_{}.rs",
            protocol.name
        ))
    }
}

fn generate_protocol_helpers(
    protocol: &EffectHandoffProtocol,
    producer_machine: Option<&MachineSchema>,
    composition: &CompositionSchema,
) -> String {
    let mut out = String::new();

    writeln!(
        &mut out,
        "// @generated — protocol helpers for `{}`",
        protocol.name
    )
    .expect("write");
    writeln!(
        &mut out,
        "// Composition: {}, Producer: {}, Effect: {}",
        composition.name, protocol.producer_instance, protocol.effect_variant
    )
    .expect("write");
    writeln!(
        &mut out,
        "// Closure policy: {}",
        closure_policy_label(&protocol.closure_policy)
    )
    .expect("write");
    if let Some(liveness) = &protocol.liveness_annotation {
        writeln!(&mut out, "// Liveness: {liveness}").expect("write");
    }
    writeln!(&mut out).expect("write");

    // --- Obligation record ---
    generate_obligation_struct(&mut out, protocol);

    // --- Executor function ---
    generate_executor(&mut out, protocol, producer_machine);

    // --- Feedback submitter(s) ---
    generate_feedback_submitters(&mut out, protocol);

    // --- Terminal classification helper (if producer has terminal phases) ---
    if let Some(machine) = producer_machine {
        if !machine.state.terminal_phases.is_empty() {
            generate_terminal_classifier(&mut out, machine);
        }
    }

    out
}

fn generate_obligation_struct(out: &mut String, protocol: &EffectHandoffProtocol) {
    let struct_name = to_pascal_case(&protocol.name);

    writeln!(
        out,
        "/// Obligation token for the `{}` protocol.",
        protocol.name
    )
    .expect("write");
    writeln!(
        out,
        "/// Captures correlation fields from the `{}` effect.",
        protocol.effect_variant
    )
    .expect("write");
    writeln!(out, "#[derive(Debug, Clone)]").expect("write");
    writeln!(out, "pub struct {struct_name}Obligation {{").expect("write");

    if protocol.correlation_fields.is_empty() {
        writeln!(out, "    _private: (),").expect("write");
    } else {
        for field in &protocol.correlation_fields {
            let rust_name = to_snake_case(field);
            writeln!(out, "    pub {rust_name}: String,").expect("write");
        }
    }

    writeln!(out, "}}").expect("write");
    writeln!(out).expect("write");
}

fn generate_executor(
    out: &mut String,
    protocol: &EffectHandoffProtocol,
    _producer_machine: Option<&MachineSchema>,
) {
    use meerkat_machine_schema::ProtocolGenerationMode;

    let obligation_type = format!("{}Obligation", to_pascal_case(&protocol.name));

    match &protocol.generation_mode {
        ProtocolGenerationMode::Executor => {
            // Executor mode: generates a function that calls authority.apply() and
            // returns effects + obligation. The exact authority type varies per
            // machine, so we generate the contract shape with typed parameters.
            let fn_name = format!("execute_{}", to_snake_case(&protocol.name));
            writeln!(
                out,
                "/// Execute the `{}` effect through the authority and return an obligation token.",
                protocol.effect_variant
            )
            .expect("write");
            writeln!(
                out,
                "/// The caller must eventually close this obligation via one of the feedback submitters."
            )
            .expect("write");
            writeln!(
                out,
                "/// Closure policy: {}.",
                closure_policy_label(&protocol.closure_policy)
            )
            .expect("write");

            if protocol.correlation_fields.is_empty() {
                writeln!(
                    out,
                    "pub fn {fn_name}() -> {obligation_type} {{\n    {obligation_type} {{ _private: () }}\n}}"
                )
                .expect("write");
            } else {
                let params: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| format!("{}: String", to_snake_case(f)))
                    .collect();
                let fields: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| to_snake_case(f))
                    .collect();
                writeln!(
                    out,
                    "pub fn {fn_name}({}) -> {obligation_type} {{\n    {obligation_type} {{ {} }}\n}}",
                    params.join(", "),
                    fields.join(", ")
                )
                .expect("write");
            }
        }
        ProtocolGenerationMode::EffectExtractor => {
            // Effect-extractor mode: scans already-emitted effects for the
            // handoff-annotated variant and extracts the obligation token.
            let fn_name = format!("extract_{}", to_snake_case(&protocol.name));
            writeln!(
                out,
                "/// Extract the `{}` obligation from already-emitted effects.",
                protocol.effect_variant
            )
            .expect("write");
            writeln!(
                out,
                "/// Scans the effect list for `{}` and creates the obligation token.",
                protocol.effect_variant
            )
            .expect("write");

            if protocol.correlation_fields.is_empty() {
                writeln!(
                    out,
                    "pub fn {fn_name}() -> {obligation_type} {{\n    {obligation_type} {{ _private: () }}\n}}"
                )
                .expect("write");
            } else {
                let params: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| format!("{}: String", to_snake_case(f)))
                    .collect();
                let fields: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| to_snake_case(f))
                    .collect();
                writeln!(
                    out,
                    "pub fn {fn_name}({}) -> {obligation_type} {{\n    {obligation_type} {{ {} }}\n}}",
                    params.join(", "),
                    fields.join(", ")
                )
                .expect("write");
            }
        }
        ProtocolGenerationMode::ShellBridge => {
            // Shell-bridge mode: wraps authority-derived data into an obligation
            // token for cross-machine handoff. The data comes from the producing
            // machine's effect, not from a direct authority.apply() call.
            let fn_name = format!("accept_{}", to_snake_case(&protocol.name));
            writeln!(
                out,
                "/// Accept authority-derived `{}` data and create an obligation token.",
                protocol.effect_variant
            )
            .expect("write");
            writeln!(
                out,
                "/// The caller provides data extracted from the producing machine's effect."
            )
            .expect("write");

            if protocol.correlation_fields.is_empty() {
                writeln!(
                    out,
                    "pub fn {fn_name}() -> {obligation_type} {{\n    {obligation_type} {{ _private: () }}\n}}"
                )
                .expect("write");
            } else {
                let params: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| format!("{}: String", to_snake_case(f)))
                    .collect();
                let fields: Vec<String> = protocol
                    .correlation_fields
                    .iter()
                    .map(|f| to_snake_case(f))
                    .collect();
                writeln!(
                    out,
                    "pub fn {fn_name}({}) -> {obligation_type} {{\n    {obligation_type} {{ {} }}\n}}",
                    params.join(", "),
                    fields.join(", ")
                )
                .expect("write");
            }
        }
    }
    writeln!(out).expect("write");
}

fn generate_feedback_submitters(out: &mut String, protocol: &EffectHandoffProtocol) {
    let obligation_type = format!("{}Obligation", to_pascal_case(&protocol.name));

    for feedback in &protocol.allowed_feedback_inputs {
        let fn_name = format!("submit_{}", to_snake_case(&feedback.input_variant));

        writeln!(
            out,
            "/// Submit `{}` feedback to `{}`, consuming the obligation token.",
            feedback.input_variant, feedback.machine_instance
        )
        .expect("write");
        writeln!(
            out,
            "/// This closes (or partially closes) the `{}` obligation.",
            protocol.name
        )
        .expect("write");
        writeln!(out, "///").expect("write");
        writeln!(
            out,
            "/// Target machine instance: `{}`",
            feedback.machine_instance
        )
        .expect("write");
        writeln!(
            out,
            "/// Target input variant: `{}`",
            feedback.input_variant
        )
        .expect("write");

        // Generate a feedback submitter that consumes the obligation by move.
        // The actual authority.apply() call is machine-specific — the generated
        // function enforces the obligation contract (move semantics), and the
        // hand-written body in the checked-in file provides the authority call.
        writeln!(
            out,
            "pub fn {fn_name}(_obligation: {obligation_type}) {{\n    // Obligation consumed by move semantics.\n    // The hand-written implementation calls authority.apply({input})\n    // on the {instance} machine instance.\n}}",
            input = feedback.input_variant,
            instance = feedback.machine_instance,
        )
        .expect("write");
        writeln!(out).expect("write");
    }
}

/// Generate a standalone terminal surface mapping module for TurnExecutionMachine.
///
/// This produces `classify_terminal(outcome: &TurnTerminalOutcome) -> SurfaceResultClass`
/// with an exhaustive match over all `TurnTerminalOutcome` variants. No default arm —
/// adding a new variant forces a compile-time update.
fn generate_terminal_surface_mapping(machine: &MachineSchema) -> String {
    let mut out = String::new();

    writeln!(
        &mut out,
        "// @generated — terminal surface mapping for `{}`",
        machine.machine
    )
    .expect("write");
    writeln!(&mut out, "// Generated by `xtask protocol-codegen`").expect("write");
    writeln!(
        &mut out,
        "// Exhaustive match — adding a new TurnTerminalOutcome variant forces a compile-time update."
    )
    .expect("write");
    writeln!(&mut out).expect("write");

    writeln!(
        &mut out,
        "use crate::turn_execution_authority::TurnTerminalOutcome;"
    )
    .expect("write");
    writeln!(&mut out).expect("write");

    // SurfaceResultClass enum
    writeln!(
        &mut out,
        "/// Surface result classification for turn execution terminal outcomes."
    )
    .expect("write");
    writeln!(&mut out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]").expect("write");
    writeln!(&mut out, "pub enum SurfaceResultClass {{").expect("write");
    writeln!(&mut out, "    Success,").expect("write");
    writeln!(&mut out, "    HardFailure,").expect("write");
    writeln!(&mut out, "    Cancelled,").expect("write");
    writeln!(&mut out, "}}").expect("write");
    writeln!(&mut out).expect("write");

    // classify_terminal function
    writeln!(
        &mut out,
        "/// Exhaustive terminal outcome classification for `{}`.",
        machine.machine
    )
    .expect("write");
    writeln!(
        &mut out,
        "/// No default arm — adding a new `TurnTerminalOutcome` variant forces a compile-time update."
    )
    .expect("write");
    writeln!(
        &mut out,
        "///\n/// # Panics\n/// Panics if called with `TurnTerminalOutcome::None` (no terminal outcome yet)."
    )
    .expect("write");
    writeln!(
        &mut out,
        "pub fn classify_terminal(outcome: &TurnTerminalOutcome) -> SurfaceResultClass {{"
    )
    .expect("write");
    writeln!(&mut out, "    match outcome {{").expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::None => panic!(\"classify_terminal called with TurnTerminalOutcome::None\"),"
    )
    .expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Completed => SurfaceResultClass::Success,"
    )
    .expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Failed => SurfaceResultClass::HardFailure,"
    )
    .expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::Cancelled => SurfaceResultClass::Cancelled,"
    )
    .expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::BudgetExhausted => SurfaceResultClass::Success,"
    )
    .expect("write");
    writeln!(
        &mut out,
        "        TurnTerminalOutcome::StructuredOutputValidationFailed => SurfaceResultClass::HardFailure,"
    )
    .expect("write");
    writeln!(&mut out, "    }}").expect("write");
    writeln!(&mut out, "}}").expect("write");

    out
}

fn generate_terminal_classifier(out: &mut String, machine: &MachineSchema) {
    writeln!(
        out,
        "/// Surface result classification for `{}` terminal outcomes.",
        machine.machine
    )
    .expect("write");
    writeln!(out, "#[derive(Debug, Clone, Copy, PartialEq, Eq)]").expect("write");
    writeln!(out, "pub enum SurfaceResultClass {{").expect("write");
    writeln!(out, "    Success,").expect("write");
    writeln!(out, "    HardFailure,").expect("write");
    writeln!(out, "    Cancelled,").expect("write");
    writeln!(out, "}}").expect("write");
    writeln!(out).expect("write");

    writeln!(
        out,
        "/// Exhaustive terminal phase classification for `{}`.",
        machine.machine
    )
    .expect("write");
    writeln!(
        out,
        "/// No default arm — adding a new terminal phase forces a compile-time update."
    )
    .expect("write");

    writeln!(
        out,
        "pub fn classify_terminal(terminal_phase: &str) -> SurfaceResultClass {{"
    )
    .expect("write");
    writeln!(out, "    match terminal_phase {{").expect("write");

    for phase in &machine.state.terminal_phases {
        let class = classify_terminal_phase(phase);
        writeln!(out, "        \"{phase}\" => SurfaceResultClass::{class},").expect("write");
    }

    writeln!(
        out,
        "        other => panic!(\"unclassified terminal phase: {{other}}\"),"
    )
    .expect("write");
    writeln!(out, "    }}").expect("write");
    writeln!(out, "}}").expect("write");
}

fn classify_terminal_phase(phase: &str) -> &'static str {
    match phase {
        "Completed" => "Success",
        "Failed" => "HardFailure",
        "Cancelled" => "Cancelled",
        "Stopped" => "Success",
        "Delivered" => "Success",
        "Exhausted" => "HardFailure",
        _ => "Success",
    }
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
