//! Standalone models rendered from the canonical source and offer DSL schemas.
//! These bounds do not prove the store, driver, native owner, or composition.

use meerkat_machine_schema::identity::{FieldId, InputVariantId};
use meerkat_machine_schema::{InputFieldDomain, InputFieldDomainKind, MachineSchema};

/// Canonical owner model and bounded standalone verification configurations.
pub struct OriginalModelArtifacts {
    pub owner_model: String,
    pub ci_config: String,
    pub deep_config: String,
}

/// One bounded recovery run, including the invariant its mutation must violate.
pub struct OriginalRecoveryProfile {
    pub name: &'static str,
    pub expected_invariant: Option<&'static str>,
    pub config: String,
}

/// Recovery environment composed over the byte-exact generated offer owner.
/// This is a partial model, not native runtime or full stage B acceptance.
pub struct OriginalRecoveryArtifacts {
    pub owner_model: String,
    pub composition_model: &'static str,
    pub profiles: Vec<OriginalRecoveryProfile>,
}

/// Export the one-offer recovery slice and three attributed semantic mutants.
pub fn original_offer_recovery_artifacts()
-> Result<OriginalRecoveryArtifacts, Box<dyn std::error::Error>> {
    let owner = original_offer_model_artifacts()?;
    let (constants, tail) = owner
        .ci_config
        .split_once("INVARIANTS\n")
        .ok_or("generated offer config has no invariants section")?;
    let (invariants, constraint) = tail
        .split_once("CONSTRAINTS\n")
        .ok_or("generated offer config has no constraints section")?;
    if constraint.trim() != "CiStateConstraint"
        || !constants.starts_with("SPECIFICATION Spec\nCONSTANTS\n")
    {
        return Err("unexpected generated offer configuration layout".into());
    }
    // Keep every generated owner invariant and typed domain. Only select the
    // composition specification/bound and add explicit environment invariants.
    let constants = constants.replacen("SPECIFICATION Spec\n", "SPECIFICATION RecoverySpec\n", 1);
    let profiles = [
        ("none", None),
        ("fresh-key-replay", Some("I3_ExactKeyAtMostOnce")),
        ("forge-admission", Some("I7_CoreReceiptProvenance")),
        ("unavailable-as-absent", Some("I13_ReadClassification")),
    ]
    .into_iter()
    .map(|(name, expected_invariant)| OriginalRecoveryProfile {
        name,
        expected_invariant,
        config: format!(
            "{constants}  Fault = \"{name}\"\nINVARIANTS\n{invariants}{environment}CONSTRAINTS\n  RecoveryBound\n",
            environment = concat!(
                "  I3_ExactKeyAtMostOnce\n",
                "  I7_CoreReceiptProvenance\n",
                "  I13_ReadClassification\n",
                "  OwnerBeforeDispatch\n",
                "  OwnerObligationSurvivesCrash\n",
                "  CanonicalDedupIdentity\n",
            ),
        ),
    })
    .collect();
    Ok(OriginalRecoveryArtifacts {
        owner_model: owner.owner_model,
        composition_model: include_str!("../models/original_offer_recovery.tla"),
        profiles,
    })
}

type Artifacts = Result<OriginalModelArtifacts, Box<dyn std::error::Error>>;

/// Render source lifecycle invariants with one CI and two Deep identity samples.
pub fn source_record_model_artifacts() -> Artifacts {
    let mut schema = crate::source_record_schema();
    for input in [
        "ConfirmPreparation",
        "BeginCancellation",
        "ConfirmCancellation",
    ] {
        echo(&mut schema, input, "expected_revision", "revision")?;
    }
    echo(
        &mut schema,
        "ConfirmCancellation",
        "published_revision",
        "revision",
    )?;
    render(&schema)
}

/// Render offer lifecycle invariants, preserving all five Core observations.
pub fn original_offer_model_artifacts() -> Artifacts {
    let mut schema = crate::original_offer_schema();
    for input in [
        "SealPreparation",
        "Offer",
        "WithdrawPrepared",
        "StartDispatch",
        "ObserveDispatch",
        "ObserveRead",
        "CloseUnresolved",
    ] {
        echo(&mut schema, input, "expected_revision", "revision")?;
    }
    echo(&mut schema, "Offer", "source_revision", "source_revision")?;
    schema.input_field_domains.push(InputFieldDomain {
        input: InputVariantId::parse("Prepare")?,
        field: FieldId::parse("source_revision")?,
        domain: InputFieldDomainKind::AdditionalValues((3..=8).collect()),
    });
    // Two process epochs (a preparation, its dispatches, reads and the close's record before
    // and after one restart) and up to three dispatches of one offer within the bounded steps.
    for (input, field, values) in [
        ("Prepare", "preparation_epoch", 1..=2),
        ("WithdrawPrepared", "epoch", 1..=2),
        ("StartDispatch", "epoch", 1..=2),
        ("ObserveRead", "epoch", 1..=2),
        ("CloseUnresolved", "epoch", 1..=2),
        ("StartDispatch", "dispatch_id", 1..=3),
        ("ObserveDispatch", "dispatch_id", 1..=3),
    ] {
        schema.input_field_domains.push(InputFieldDomain {
            input: InputVariantId::parse(input)?,
            field: FieldId::parse(field)?,
            domain: InputFieldDomainKind::AdditionalValues(values.collect()),
        });
    }
    render(&schema)
}

/// Render the component lifetime owner (admission closure, then observed
/// settlement) for the driver composition and its standalone bounds.
pub fn component_lifetime_model_artifacts() -> Artifacts {
    let mut schema = crate::component_lifetime_schema();
    for input in [
        "Open",
        "BeginClose",
        "ConfirmSettlement",
        "ObserveUncertainty",
        "ObserveFailure",
    ] {
        echo(&mut schema, input, "expected_revision", "revision")?;
    }
    render(&schema)
}

fn echo(
    schema: &mut MachineSchema,
    input: &str,
    field: &str,
    state_field: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Each declaration matches an equality guard in the canonical DSL. This
    // keeps valid later revisions reachable; stale-input rejection still needs
    // separate kernel controls and composition mutation witnesses.
    schema.input_field_domains.push(InputFieldDomain {
        input: InputVariantId::parse(input)?,
        field: FieldId::parse(field)?,
        domain: InputFieldDomainKind::StateField(FieldId::parse(state_field)?),
    });
    Ok(())
}

fn render(schema: &MachineSchema) -> Artifacts {
    schema.validate()?;
    Ok(OriginalModelArtifacts {
        owner_model: meerkat_machine_codegen::render_machine_semantic_model(schema)?,
        ci_config: include_empty_string(meerkat_machine_codegen::render_machine_ci_cfg(
            schema, false,
        )?)?,
        deep_config: include_empty_string(meerkat_machine_codegen::render_machine_ci_cfg(
            schema, true,
        )?)?,
    })
}

fn include_empty_string(config: String) -> Result<String, Box<dyn std::error::Error>> {
    // The shared renderer's CI string pool omits empty strings. Both ordinary
    // source registration and unresolved Core observations require an empty
    // reference, so retain that value explicitly in the exported TLC config.
    // Only the finite configuration domain changes; the generated TLA model,
    // transition guards, state updates and invariants are left byte-exact.
    let mut found = 0;
    let mut result = String::new();
    for line in config.lines() {
        if line.starts_with("  StringValues = {") {
            found += 1;
            if !line.ends_with('}') {
                return Err("unexpected generated StringValues assignment".into());
            }
            if !line.contains("\"\"") {
                result.push_str(line.trim_end_matches('}'));
                result.push_str(", \"\"}\n");
                continue;
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if found != 1 {
        return Err("expected one generated StringValues assignment".into());
    }
    Ok(result)
}
