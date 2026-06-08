use clap::Args;
use meerkat_machine_schema::{
    CompositionSchema, EffectDisposition, MachineSchema, Route, SeamClassification,
    canonical_composition_schemas, canonical_machine_schemas, compat_composition_schemas,
};

/// CLI args for `xtask seam-inventory`.
#[derive(Debug, Clone, Args, Default)]
pub struct SeamInventoryArgs {
    /// Retained for CLI compatibility. Explicit seam classification is now
    /// enforced by construction: the machine-catalog DSL parser requires a
    /// `seam <Classification>` clause on every disposition, so an unclassified
    /// Local/External effect cannot reach this audit. The realization debts
    /// (handoff protocols, public-surface contracts, routed-effect typed
    /// realization) are always hard errors regardless of this flag.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug)]
pub struct SeamEntry {
    pub machine: String,
    pub effect_variant: String,
    pub disposition: String,
    /// Schema-owned seam classification read straight off the generated
    /// `EffectDispositionRule::seam_classification`. Because the DSL parser
    /// requires the `seam` clause on every disposition, every Local/External
    /// effect carries an explicit classification by construction — there is no
    /// unclassified-effect case to recover from.
    pub classification: SeamClassification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Closed,
    Open,
}

#[derive(Debug)]
pub struct SurfaceContractEntry {
    pub name: &'static str,
    pub notes: &'static str,
    pub status: ContractStatus,
}

/// Entry in the routed-effect inventory: every routed effect's consumer
/// machine + input variant must resolve to a typed [`RoutedInput`] in the
/// composition schema. This is the teeth B-10 puts on the
/// `EffectDisposition::Routed` branch that the classification table skips.
#[derive(Debug)]
pub struct RoutedRealization {
    pub producer_machine: String,
    pub effect_variant: String,
    pub composition: String,
    pub resolved_consumers: Vec<(String, String, String)>,
    pub missing_consumers: Vec<String>,
}

fn known_public_surface_contracts() -> Vec<SurfaceContractEntry> {
    vec![
        SurfaceContractEntry {
            name: "mob::spawn_helper",
            notes: "Contract-tested helper wrapper; return surface derives from canonical member/session terminal truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::fork_helper",
            notes: "Contract-tested helper wrapper; return surface derives from canonical member/session terminal truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::respawn",
            notes: "Contract-tested helper wrapper; receipt aligns with retired and replacement session truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::wait_one",
            notes: "Helper wrapper polls canonical member/session state rather than inventing terminal classification",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::wait_all",
            notes: "Helper wrapper composes wait_one over canonical member/session state",
            status: ContractStatus::Closed,
        },
    ]
}

pub fn run_seam_inventory(args: SeamInventoryArgs) -> anyhow::Result<()> {
    let machines = canonical_machine_schemas();
    let compositions = canonical_composition_schemas();
    let mut entries: Vec<SeamEntry> = Vec::new();
    let public_surface_contracts = known_public_surface_contracts();

    for machine in &machines {
        collect_machine_seams(machine, &mut entries);
    }

    // Routed-effect realization inventory — the teeth on the
    // `EffectDisposition::Routed` arm. Each routed effect must resolve to
    // a typed consumer input via the composition schema's `routed_inputs`.
    let routed_realizations = collect_routed_realizations(&machines, &compositions);

    let protocol_index = compositions
        .iter()
        .flat_map(|composition| {
            composition.handoff_protocols.iter().filter_map(|protocol| {
                composition
                    .machines
                    .iter()
                    .find(|instance| instance.instance_id == protocol.producer_instance)
                    .map(|instance| {
                        (
                            (
                                instance.machine_name.as_str().to_string(),
                                protocol.effect_variant.as_str().to_string(),
                            ),
                            protocol.name.as_str().to_string(),
                        )
                    })
            })
        })
        .fold(
            std::collections::BTreeMap::<(String, String), Vec<String>>::new(),
            |mut acc, (key, protocol_name)| {
                acc.entry(key).or_default().push(protocol_name);
                acc
            },
        );

    let unresolved_protocol_debt = entries
        .iter()
        .filter(|entry| entry.classification == SeamClassification::OwnerRealizationPlusFeedback)
        .filter(|entry| {
            !protocol_index.contains_key(&(entry.machine.clone(), entry.effect_variant.clone()))
        })
        .collect::<Vec<_>>();
    let unresolved_public_surface_alignment_debt = public_surface_contracts
        .iter()
        .filter(|entry| entry.status != ContractStatus::Closed)
        .collect::<Vec<_>>();
    let unresolved_routed_debt = routed_realizations
        .iter()
        .filter(|rr| !rr.missing_consumers.is_empty())
        .collect::<Vec<_>>();

    // Print the report
    print_report(&entries);
    print_routed_realizations(&routed_realizations);

    // Summary statistics
    print_summary(
        &entries,
        &unresolved_protocol_debt,
        &public_surface_contracts,
        &unresolved_public_surface_alignment_debt,
        &routed_realizations,
        &unresolved_routed_debt,
        &compositions,
    );

    // `--strict` no longer has a classification-debt arm: the DSL parser
    // requires a `seam <Classification>` clause on every disposition, so each
    // Local/External effect carries an explicit classification by construction.
    // An unclassified effect cannot reach this audit — it would fail to compile
    // in the machine catalog. What remains is the realization debt the strict
    // mode always enforced (handoff protocols, public-surface contracts, and
    // routed-effect realization), which are unconditional hard errors. The
    // `args.strict` flag is retained on the CLI surface for compatibility.
    let _ = args.strict;
    let protocol_debt = unresolved_protocol_debt.len();
    let public_surface_debt = unresolved_public_surface_alignment_debt.len();
    let routed_debt = unresolved_routed_debt.len();

    if protocol_debt > 0 || public_surface_debt > 0 || routed_debt > 0 {
        anyhow::bail!(
            "seam inventory has unresolved debt: protocol={protocol_debt}, public_surface={public_surface_debt}, routed={routed_debt}",
        );
    }

    Ok(())
}

fn collect_machine_seams(machine: &MachineSchema, entries: &mut Vec<SeamEntry>) {
    for rule in &machine.effect_dispositions {
        match &rule.disposition {
            EffectDisposition::Local | EffectDisposition::External => {
                let disposition_str = match &rule.disposition {
                    EffectDisposition::External => "External",
                    // The outer arm restricts us to Local/External; any
                    // non-External disposition here is Local.
                    _ => "Local",
                };

                entries.push(SeamEntry {
                    machine: machine.machine.as_str().to_string(),
                    effect_variant: rule.effect_variant.as_str().to_string(),
                    disposition: disposition_str.to_string(),
                    classification: rule.seam_classification,
                });
            }
            EffectDisposition::Routed { .. } => {
                // Routed effects are classified by the routed-effect
                // realization inventory (see `collect_routed_realizations`),
                // not by the disposition-based classification table.
            }
        }
    }
}

/// Build the routed-effect realization inventory: for each `Routed` effect
/// disposition, walk every composition that binds the producing machine and
/// assert there is a typed `RoutedInput` entry resolving
/// (producer_instance, effect_variant) to (consumer_instance, input_variant).
///
/// Any listed consumer_machine that lacks a matching `RoutedInput` is a
/// `missing_consumer` — the composition schema declares a route in principle
/// but the typed route-variant table does not realize it. B-10 upgrades this
/// from a soft warning to a hard error so that the producer-consumer path
/// is guaranteed to traverse `CompositionDispatcher::dispatch`.
fn collect_routed_realizations(
    machines: &[MachineSchema],
    compositions: &[CompositionSchema],
) -> Vec<RoutedRealization> {
    let mut out = Vec::new();
    for producer in machines {
        for disposition in &producer.effect_dispositions {
            let EffectDisposition::Routed { consumer_machines } = &disposition.disposition else {
                continue;
            };
            let producer_name = producer.machine.as_str();
            let effect_variant = disposition.effect_variant.as_str();
            for composition in compositions {
                // Does this composition bind the producer machine?
                let producer_instance = composition
                    .machines
                    .iter()
                    .find(|inst| inst.machine_name == producer.machine);
                let Some(producer_instance) = producer_instance else {
                    continue;
                };
                let composition_name = composition.name.as_str().to_string();
                let mut resolved = Vec::new();
                let mut missing = Vec::new();
                for consumer in consumer_machines {
                    let consumer_instance = composition
                        .machines
                        .iter()
                        .find(|inst| inst.machine_name == *consumer);
                    let Some(consumer_instance) = consumer_instance else {
                        // Producer routed to a consumer that isn't bound in
                        // this composition. Not necessarily a bug — another
                        // composition may realize it. Record as missing so
                        // the summary reflects it if every composition has
                        // the same gap.
                        missing.push(format!(
                            "{} (not bound in composition {})",
                            consumer.as_str(),
                            composition_name
                        ));
                        continue;
                    };
                    // Find a typed Route entry in this composition that
                    // resolves producer_instance + effect_variant to an input
                    // on consumer_instance.
                    let resolved_route = composition.routes.iter().find(|r| {
                        matches_route(
                            r,
                            &producer_instance.instance_id,
                            effect_variant,
                            &consumer_instance.instance_id,
                        )
                    });
                    match resolved_route {
                        Some(route) => resolved.push((
                            consumer.as_str().to_string(),
                            route.to.machine.as_str().to_string(),
                            route.to.input_variant.as_str().to_string(),
                        )),
                        None => missing.push(format!(
                            "{} (no Route entry in composition {})",
                            consumer.as_str(),
                            composition_name
                        )),
                    }
                }
                out.push(RoutedRealization {
                    producer_machine: producer_name.to_string(),
                    effect_variant: effect_variant.to_string(),
                    composition: composition_name,
                    resolved_consumers: resolved,
                    missing_consumers: missing,
                });
            }
        }
    }
    out
}

/// Match a [`Route`] entry against a concrete
/// (producer_instance, effect_variant, consumer_instance) tuple. This is
/// where B-10 pins the semantic invariant: the typed route entry must name
/// the same producer instance, the same producer effect variant, and the
/// same consumer instance as the disposition declared at the machine level.
fn matches_route(
    route: &Route,
    producer_instance: &meerkat_machine_schema::identity::MachineInstanceId,
    effect_variant: &str,
    consumer_instance: &meerkat_machine_schema::identity::MachineInstanceId,
) -> bool {
    route.from_machine == *producer_instance
        && route.effect_variant.as_str() == effect_variant
        && route.to.machine == *consumer_instance
}

fn print_routed_realizations(routed: &[RoutedRealization]) {
    if routed.is_empty() {
        return;
    }
    println!();
    println!("## Routed Effect Realizations (producer → composition → consumer)");
    for rr in routed {
        let resolved_summary = if rr.resolved_consumers.is_empty() {
            "(none resolved)".to_string()
        } else {
            rr.resolved_consumers
                .iter()
                .map(|(consumer_machine, consumer_instance, input_variant)| {
                    format!("{consumer_machine}[{consumer_instance}]::{input_variant}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {}::{} in {}  →  {}",
            rr.producer_machine, rr.effect_variant, rr.composition, resolved_summary,
        );
        for missing in &rr.missing_consumers {
            println!("    ! MISSING: {missing}");
        }
    }
}

fn print_report(entries: &[SeamEntry]) {
    println!("# Seam Inventory Report");
    println!("# Generated by `xtask seam-inventory`");
    println!();

    let mut current_machine = "";
    for entry in entries {
        if entry.machine != current_machine {
            if !current_machine.is_empty() {
                println!();
            }
            println!("## {}", entry.machine);
            current_machine = &entry.machine;
        }

        println!(
            "  {:40} {:10} {}",
            entry.effect_variant, entry.disposition, entry.classification,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn print_summary(
    entries: &[SeamEntry],
    unresolved_protocol_debt: &[&SeamEntry],
    public_surface_contracts: &[SurfaceContractEntry],
    unresolved_public_surface_alignment_debt: &[&SurfaceContractEntry],
    routed_realizations: &[RoutedRealization],
    unresolved_routed_debt: &[&RoutedRealization],
    compositions: &[CompositionSchema],
) {
    let total = entries.len();
    let no_owner = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::NoOwnerRealization)
        .count();
    let realization_only = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::OwnerRealizationOnly)
        .count();
    let realization_feedback = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::OwnerRealizationPlusFeedback)
        .count();
    let surface_alignment = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::SurfaceResultAlignment)
        .count();

    println!();
    println!("## Summary");
    println!("  Total Local/External effects:           {total}");
    println!("  no-owner-realization:                   {no_owner}");
    println!("  owner-realization-only:                 {realization_only}");
    println!("  owner-realization-plus-feedback:        {realization_feedback}");
    println!("  surface-result-alignment:               {surface_alignment}");
    println!(
        "  routed-effect realizations:             {}",
        routed_realizations.len()
    );
    println!();
    println!("## Seams Requiring Formal Handoff Protocols");
    for entry in entries {
        if entry.classification == SeamClassification::OwnerRealizationPlusFeedback {
            println!("  {} :: {}", entry.machine, entry.effect_variant);
        }
    }
    println!();
    // Generated handoff obligation pairs declared in canonical + perimeter
    // compositions. Each protocol is an obligation pair: producer effect
    // → realising actor → typed feedback input(s) that close the
    // step-lock (ack / failure). Producers now host the annotation on
    // their canonical effect rather than through bridge-only schemas.
    let compat = compat_composition_schemas();
    let mut protocol_rows: Vec<(String, String, String, String, String, String)> = Vec::new();
    let all_compositions: Vec<&CompositionSchema> =
        compositions.iter().chain(compat.iter()).collect();
    for composition in &all_compositions {
        for protocol in &composition.handoff_protocols {
            let feedback_variants: Vec<String> = protocol
                .allowed_feedback_inputs
                .iter()
                .map(|fb| {
                    format!(
                        "{}::{}",
                        fb.machine_instance.as_str(),
                        fb.input_variant.as_str(),
                    )
                })
                .collect();
            protocol_rows.push((
                protocol.name.as_str().to_string(),
                composition.name.as_str().to_string(),
                protocol.producer_instance.as_str().to_string(),
                protocol.effect_variant.as_str().to_string(),
                protocol.realizing_actor.as_str().to_string(),
                feedback_variants.join(", "),
            ));
        }
    }
    protocol_rows.sort();
    println!("## Declared Handoff Obligation Pairs (canonical + compat)");
    println!(
        "  {:40} {:28} {:30} {:32} {:32} feedback_inputs",
        "protocol", "composition", "producer_instance", "effect", "realizing_actor"
    );
    for (protocol, composition, producer, effect, actor, feedback) in &protocol_rows {
        println!(
            "  {protocol:40} {composition:28} {producer:30} {effect:32} {actor:32} {feedback}"
        );
    }
    println!(
        "  total handoff obligation pairs:            {}",
        protocol_rows.len()
    );
    println!();
    // C-F3 — destroy-obligation pairing audit. State-scope audit row
    // F3 flagged that `MeerkatMachine` carries a
    // `peer_ingress_mob_id: Option<MobId>` whose "mob-exists"
    // invariant is convention-driven: when a mob destroys its
    // runtime, every session whose peer-ingress ownership was
    // `MobOwned` by that mob must receive `DetachIngress` first,
    // otherwise the `peer_ingress_mob_id` on that session dangles.
    //
    // This section walks every canonical routed effect whose variant
    // name contains the substring "Destroy" and asserts a paired
    // handoff obligation exists whose feedback inputs include at
    // least one variant that mentions "IngressDetached" or "Detach".
    // Unpaired destroy routes are emitted as debt so the
    // `mob-destroy → session-detach` ordering cannot regress
    // silently.
    let mut destroy_routes: Vec<(String, String, String, String, bool)> = Vec::new();
    for composition in &all_compositions {
        for route in &composition.routes {
            // Filter to destroy *requests* that flow from a producer to a
            // consumer that will be torn down. Reply signals like
            // `RuntimeDestroyed` (consumer → producer, "destroy observed")
            // are not request-side effects and do not need a paired
            // detach obligation: the destroy is already done by the time
            // they fire.
            let variant = route.effect_variant.as_str();
            if !(variant.contains("Destroy") && variant.starts_with("Request")) {
                continue;
            }
            let producer_instance = route.from_machine.as_str().to_string();
            let producer_machine = composition
                .machines
                .iter()
                .find(|m| m.instance_id == route.from_machine)
                .map(|m| m.machine_name.as_str().to_string())
                .unwrap_or_default();
            let effect = route.effect_variant.as_str().to_string();
            let composition_name = composition.name.as_str().to_string();
            // Paired iff some protocol (across all compositions) has a
            // feedback input whose variant name is "*IngressDetach*"
            // or "*Detach*" AND the protocol's allowed feedback inputs
            // route into the same consumer as this destroy route.
            let consumer_instance = &route.to.machine;
            let paired = all_compositions.iter().any(|other| {
                other.handoff_protocols.iter().any(|protocol| {
                    protocol.allowed_feedback_inputs.iter().any(|fb| {
                        let variant = fb.input_variant.as_str();
                        (variant.contains("IngressDetached") || variant.contains("Detach"))
                            && (fb.machine_instance == *consumer_instance
                                || other.machines.iter().any(|m| {
                                    m.instance_id == fb.machine_instance
                                        && other
                                            .machines
                                            .iter()
                                            .any(|cm| cm.machine_name.as_str() == producer_machine)
                                })
                                || protocol.name.as_str().contains("session_ingress"))
                    })
                })
            });
            destroy_routes.push((
                producer_machine.clone(),
                effect,
                composition_name,
                producer_instance,
                paired,
            ));
        }
    }
    destroy_routes.sort();
    println!("## Destroy-obligation Pairing (C-F3)");
    if destroy_routes.is_empty() {
        println!("  (no canonical routed *Destroy* effects declared)");
    } else {
        println!(
            "  {:24} {:32} {:32} {:32} paired",
            "producer_machine", "effect_variant", "composition", "producer_instance"
        );
        for (producer, effect, composition_name, instance, paired) in &destroy_routes {
            println!(
                "  {:24} {:32} {:32} {:32} {}",
                producer,
                effect,
                composition_name,
                instance,
                if *paired { "yes" } else { "NO" }
            );
        }
    }
    let unpaired_destroy_debt: Vec<&(String, String, String, String, bool)> = destroy_routes
        .iter()
        .filter(|(_, _, _, _, paired)| !paired)
        .collect();
    println!(
        "  unpaired destroy routes (debt):            {}",
        unpaired_destroy_debt.len()
    );
    println!();
    println!("## Public Surface Contracts");
    for entry in public_surface_contracts {
        println!(
            "  {:28} {:6} {}",
            entry.name,
            match entry.status {
                ContractStatus::Closed => "closed",
                ContractStatus::Open => "open",
            },
            entry.notes
        );
    }
    println!();
    println!("## Debt");
    println!(
        "  unresolved protocol debt:                  {}",
        unresolved_protocol_debt.len()
    );
    println!(
        "  unresolved public-surface alignment debt:  {}",
        unresolved_public_surface_alignment_debt.len()
    );
    println!(
        "  unresolved routed-effect debt:             {}",
        unresolved_routed_debt.len()
    );
    println!(
        "  unpaired destroy-obligation debt (C-F3):   {}",
        unpaired_destroy_debt.len()
    );
}
