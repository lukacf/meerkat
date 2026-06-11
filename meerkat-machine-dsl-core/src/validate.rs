use std::collections::HashSet;

use syn::Error;

use crate::ast::*;

const NATIVE_MOB_MACHINE_HELPERS: &[&str] = &[
    "meerkat_peer_endpoint_set_cardinality_matches",
    "meerkat_peer_endpoint_set_contains_peer_id",
    "meerkat_peer_endpoint_option_peer_id_matches",
    "meerkat_peer_endpoint_peer_id_matches",
    "meerkat_peer_endpoint_set_peer_ids_unique",
    "mob_machine_external_peer_edge_has_matching_key",
    "mob_machine_external_peer_edge_local",
    "mob_machine_external_peer_edge_peer_id",
    "mob_machine_external_peer_identity_absent",
    "mob_machine_external_peer_key_matches_edge",
    "mob_machine_external_peer_key_matches_local",
    "mob_machine_identity_has_session_binding",
    "mob_machine_session_bound_live_runtime_ids_match",
    "mob_machine_member_peer_endpoint_peer_id",
    "mob_machine_member_peer_overlay",
    "mob_machine_member_peer_overlay_complete",
    "mob_machine_member_peer_overlay_peer_ids_unique",
    "mob_machine_wiring_edge_matches_members",
    "mob_machine_run_step_status_after_set",
    "mob_machine_run_step_bool_after_set",
    "mob_machine_run_step_condition_result_after_set",
    "mob_machine_run_step_u64_after_set",
    "mob_machine_run_step_u64_after_increment",
    "mob_machine_run_retry_count_after_increment",
    "mob_machine_frame_node_bool_after_set",
    "mob_machine_frame_node_status_after_admit",
    "mob_machine_frame_ready_queue_after_admit",
    "mob_machine_frame_node_status_after_terminal",
    "mob_machine_frame_ready_queue_after_terminal",
    "mob_machine_node_terminal",
    "mob_machine_step_status_from_frame_node_status",
    "mob_coordination_work_intent_unexpired",
    "mob_coordination_resource_claim_unexpired",
    "mob_coordination_resource_claim_active_at",
    "mob_coordination_resource_claim_inactive_at",
    // WAVE G2 machine folds (#181 respawn generation, #351 membership reconcile).
    "mob_machine_next_respawn_generation",
    "mob_machine_members_to_spawn",
    "mob_machine_members_to_retire",
];

/// Validate the parsed machine definition for semantic correctness.
///
/// Checks all cross-references between blocks, ensuring the DSL definition
/// is internally consistent before code generation.
pub fn validate(def: &MachineDef) -> Result<(), Error> {
    let mut errors = Vec::new();

    let field_names: HashSet<_> = def
        .state_fields
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    let phase_names: HashSet<_> = def
        .phase_enum
        .variants
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    let input_variants: HashSet<_> = def
        .inputs
        .variants
        .iter()
        .map(|v| v.name.to_string())
        .collect();
    let signal_variants: HashSet<_> = def
        .signals
        .variants
        .iter()
        .map(|v| v.name.to_string())
        .collect();
    let effect_variants: HashSet<_> = def
        .effects
        .variants
        .iter()
        .map(|v| v.name.to_string())
        .collect();
    let helper_names: HashSet<_> = def.helpers.iter().map(|h| h.name.to_string()).collect();

    // --- Init phase must be a valid phase ---
    if !phase_names.contains(&def.init_phase.to_string()) {
        errors.push(Error::new(
            def.init_phase.span(),
            format!("init phase `{}` is not in the phase enum", def.init_phase),
        ));
    }

    // --- Terminal phases must be valid phases ---
    for tp in &def.terminal_phases {
        if !phase_names.contains(&tp.to_string()) {
            errors.push(Error::new(
                tp.span(),
                format!("terminal phase `{tp}` is not in the phase enum"),
            ));
        }
    }

    // --- Init field names must be valid state fields ---
    for init in &def.init_fields {
        if !field_names.contains(&init.name.to_string()) {
            errors.push(Error::new(
                init.span,
                format!("init field `{}` is not in the state block", init.name),
            ));
        }
    }

    // --- Stored-phase validation ---
    if def.is_stored_phase() {
        if def.phase_projection.is_some() {
            errors.push(Error::new(
                def.name.span(),
                "stored-phase machines should not have a `phase_projection` block",
            ));
        }
    } else {
        // Derived-phase: must have phase_projection
        if def.phase_projection.is_none() {
            errors.push(Error::new(
                def.name.span(),
                "derived-phase machines require a `phase_projection` block",
            ));
        }
        // Validate projection covers all phases
        if let Some(proj) = &def.phase_projection {
            let projected: HashSet<_> = proj.rules.iter().map(|r| r.phase.to_string()).collect();
            for phase in &def.phase_enum.variants {
                if !projected.contains(&phase.to_string()) {
                    errors.push(Error::new(
                        phase.span(),
                        format!("phase `{phase}` is not covered by `phase_projection`"),
                    ));
                }
            }
            // Totality + no-dead-arms (fail closed at the DSL boundary). The
            // generated `phase()` evaluates the projection rules top-to-bottom,
            // returning on the first match. The FINAL rule must be an
            // unconditional fallback so the projection is exhaustive by
            // construction (the generated method then needs no panicking
            // `unreachable!`), and NO earlier rule may be unconditional — an
            // unconditional non-final rule makes every following rule dead.
            for (idx, rule) in proj.rules.iter().enumerate() {
                let is_last = idx + 1 == proj.rules.len();
                match (is_last, rule.condition.is_some()) {
                    (true, true) => errors.push(Error::new(
                        rule.phase.span(),
                        "the final `phase_projection` rule must be unconditional (a total \
                         fallback with no `if` condition) so the projection is exhaustive by \
                         construction",
                    )),
                    (false, false) => errors.push(Error::new(
                        rule.phase.span(),
                        "only the final `phase_projection` rule may be unconditional; an earlier \
                         unconditional rule makes every following projection rule unreachable",
                    )),
                    _ => {}
                }
            }
        }
    }

    // --- Transition validation ---
    let mut transition_names = HashSet::new();
    for t in &def.transitions {
        // Unique transition names
        if !transition_names.insert(t.name.to_string()) {
            errors.push(Error::new(
                t.name.span(),
                format!("duplicate transition name `{}`", t.name),
            ));
        }

        // Trigger variant exists
        match &t.trigger.kind {
            TriggerKindDef::Input => {
                if !input_variants.contains(&t.trigger.variant.to_string()) {
                    errors.push(Error::new(
                        t.trigger.variant.span(),
                        format!(
                            "input variant `{}` not found in input enum",
                            t.trigger.variant
                        ),
                    ));
                }
            }
            TriggerKindDef::Signal => {
                if !signal_variants.contains(&t.trigger.variant.to_string()) {
                    errors.push(Error::new(
                        t.trigger.variant.span(),
                        format!(
                            "signal variant `{}` not found in signal enum",
                            t.trigger.variant
                        ),
                    ));
                }
            }
        }

        // Bindings match the variant's field names
        let variant_def = match &t.trigger.kind {
            TriggerKindDef::Input => def
                .inputs
                .variants
                .iter()
                .find(|v| v.name == t.trigger.variant),
            TriggerKindDef::Signal => def
                .signals
                .variants
                .iter()
                .find(|v| v.name == t.trigger.variant),
        };
        if let Some(vdef) = variant_def {
            let variant_field_names: HashSet<_> =
                vdef.fields.iter().map(|f| f.name.to_string()).collect();
            for binding in &t.trigger.bindings {
                if !variant_field_names.contains(&binding.to_string()) {
                    errors.push(Error::new(
                        binding.span(),
                        format!(
                            "binding `{binding}` not found in variant `{}`'s fields",
                            t.trigger.variant
                        ),
                    ));
                }
            }
        }

        // Target phase exists
        if !phase_names.contains(&t.to_phase.to_string()) {
            errors.push(Error::new(
                t.to_phase.span(),
                format!("target phase `{}` not in the phase enum", t.to_phase),
            ));
        }

        // Effect variants exist
        for effect in &t.effects {
            if !effect_variants.contains(&effect.variant.to_string()) {
                errors.push(Error::new(
                    effect.variant.span(),
                    format!("effect variant `{}` not in effect enum", effect.variant),
                ));
            }
        }

        // Validate field references in guards
        for guard in &t.guards {
            let binding_names: HashSet<_> = t
                .trigger
                .bindings
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            validate_expr(
                &guard.expr,
                &field_names,
                &binding_names,
                &helper_names,
                &mut errors,
            );
        }

        // Validate field references in updates
        for update in &t.updates {
            let binding_names: HashSet<_> = t
                .trigger
                .bindings
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            validate_update(
                update,
                &field_names,
                &binding_names,
                &helper_names,
                &mut errors,
            );
        }
    }

    // --- Disposition validation ---
    for d in &def.dispositions {
        if !effect_variants.contains(&d.effect.to_string()) {
            errors.push(Error::new(
                d.effect.span(),
                format!("disposition effect `{}` not in effect enum", d.effect),
            ));
        }
    }

    // --- Invariant validation ---
    for inv in &def.invariants {
        validate_expr(
            &inv.expr,
            &field_names,
            &HashSet::new(),
            &helper_names,
            &mut errors,
        );
    }

    // --- Helper validation ---
    for h in &def.helpers {
        let param_names: HashSet<_> = h.params.iter().map(|p| p.name.to_string()).collect();
        validate_expr(
            &h.body,
            &field_names,
            &param_names,
            &helper_names,
            &mut errors,
        );
    }

    // Fail closed on helper-call cycles. A cyclic helper graph has no
    // topological order, so downstream codegen (TLA helper emission) cannot
    // define a helper before the helper it calls. Detect this here, at DSL
    // validation time, rather than relying on a silent arbitrary-order
    // fallback during generation. Run only when structural validation is
    // clean so an unknown-helper error is not double-reported as a cycle.
    if errors.is_empty()
        && let Err(cycle_error) = validate_no_helper_cycles(def)
    {
        errors.push(cycle_error);
    }

    // Fidelity: schema `from` phase sets must be derivable without the
    // historical silent `all_phases` fallback. Run `derive_from_phases`
    // only if the structural validation above is clean — otherwise every
    // transition on a malformed machine piles on a spurious "cannot
    // derive from-set" secondary error that drowns the real one.
    if errors.is_empty() {
        for t in &def.transitions {
            if let Err(msg) = crate::gen_schema::derive_from_phases(def, t) {
                errors.push(Error::new(t.span, msg));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        // Combine all errors into one
        let mut combined = errors.remove(0);
        for e in errors {
            combined.combine(e);
        }
        Err(combined)
    }
}

/// Collect the names of helpers directly called within an expression.
///
/// Only `ExprDef::Call` nodes contribute an edge; the walk recurses through
/// every compound expression so a helper call nested arbitrarily deep is still
/// observed.
fn collect_helper_call_names(expr: &ExprDef, calls: &mut HashSet<String>) {
    match expr {
        ExprDef::Call { helper, args } => {
            calls.insert(helper.to_string());
            for arg in args {
                collect_helper_call_names(arg, calls);
            }
        }
        ExprDef::FieldAccess { base, .. }
        | ExprDef::EnumVariantIs { value: base, .. }
        | ExprDef::EnumStringSetPayload { value: base, .. } => {
            collect_helper_call_names(base, calls);
        }
        ExprDef::Not(inner)
        | ExprDef::IsSome(inner)
        | ExprDef::IsNone(inner)
        | ExprDef::Len(inner)
        | ExprDef::MapKeys(inner)
        | ExprDef::Some(inner) => {
            collect_helper_call_names(inner, calls);
        }
        ExprDef::And(exprs) | ExprDef::Or(exprs) => {
            for e in exprs {
                collect_helper_call_names(e, calls);
            }
        }
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => {
            collect_helper_call_names(l, calls);
            collect_helper_call_names(r, calls);
        }
        ExprDef::Contains { collection, value } | ExprDef::Count { collection, value } => {
            collect_helper_call_names(collection, calls);
            collect_helper_call_names(value, calls);
        }
        ExprDef::MapContainsKey { map, key }
        | ExprDef::MapGet { map, key }
        | ExprDef::MapGetCopied { map, key }
        | ExprDef::MapGetCloned { map, key } => {
            collect_helper_call_names(map, calls);
            collect_helper_call_names(key, calls);
        }
        ExprDef::ForAll { over, body, .. } | ExprDef::Exists { over, body, .. } => {
            collect_helper_call_names(over, calls);
            collect_helper_call_names(body, calls);
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_helper_call_names(condition, calls);
            collect_helper_call_names(then_expr, calls);
            collect_helper_call_names(else_expr, calls);
        }
        // Leaves and phase/binding/field refs contribute no helper edges.
        ExprDef::Field(_)
        | ExprDef::Binding(_)
        | ExprDef::Bool(_)
        | ExprDef::U64(_)
        | ExprDef::U64Max
        | ExprDef::StringLit(_)
        | ExprDef::None
        | ExprDef::EmptySeq
        | ExprDef::EmptySet
        | ExprDef::EmptyMap
        | ExprDef::CurrentPhase
        | ExprDef::Phase(_)
        | ExprDef::NamedVariant { .. } => {}
    }
}

/// Fail-closed helper-call cycle detector.
///
/// Builds the helper-call dependency graph (helper name -> directly-called
/// declared helper names) and runs an iterative DFS with a visited set plus an
/// explicit recursion stack. The first back-edge found yields an error naming
/// the cycle path; an acyclic graph yields `Ok(())`.
///
/// Only edges to helpers declared on this machine are followed; self-calls and
/// calls to native/undeclared helpers (already reported by the structural pass
/// when truly unknown) are not ordering edges.
fn validate_no_helper_cycles(def: &MachineDef) -> Result<(), Error> {
    use std::collections::BTreeMap;

    let declared: HashSet<String> = def.helpers.iter().map(|h| h.name.to_string()).collect();
    let deps_of = |helper: &HelperDef| -> Vec<String> {
        let mut calls = HashSet::new();
        collect_helper_call_names(&helper.body, &mut calls);
        let name = helper.name.to_string();
        let mut edges: Vec<String> = calls
            .into_iter()
            .filter(|dep| dep != &name && declared.contains(dep))
            .collect();
        // Deterministic edge order keeps the reported cycle path stable.
        edges.sort();
        edges
    };
    let by_name: BTreeMap<String, &HelperDef> = def
        .helpers
        .iter()
        .map(|h| (h.name.to_string(), h))
        .collect();
    let span_of = |name: &str| -> proc_macro2::Span {
        match by_name.get(name) {
            Some(helper) => helper.name.span(),
            None => proc_macro2::Span::call_site(),
        }
    };

    let mut visited: HashSet<String> = HashSet::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut stack_path: Vec<String> = Vec::new();
    let mut frames: Vec<(String, Vec<String>, usize)> = Vec::new();

    for root in &def.helpers {
        let root_name = root.name.to_string();
        if visited.contains(&root_name) {
            continue;
        }
        frames.push((root_name.clone(), deps_of(root), 0));
        on_stack.insert(root_name.clone());
        stack_path.push(root_name);

        while !frames.is_empty() {
            // Read the next dependency (if any) from the top frame, advancing
            // its cursor, then release the borrow before mutating `frames`.
            let next_dep = {
                let Some((_, deps, idx)) = frames.last_mut() else {
                    break;
                };
                if *idx < deps.len() {
                    let dep = deps[*idx].clone();
                    *idx += 1;
                    Some(dep)
                } else {
                    None
                }
            };

            match next_dep {
                Some(dep) => {
                    if on_stack.contains(&dep) {
                        let start = stack_path.iter().position(|n| n == &dep).unwrap_or(0);
                        let mut cycle = stack_path[start..].to_vec();
                        cycle.push(dep.clone());
                        return Err(Error::new(
                            span_of(&dep),
                            format!("helper-call cycle detected: {}", cycle.join(" -> ")),
                        ));
                    }
                    if visited.contains(&dep) {
                        continue;
                    }
                    let dep_deps = match by_name.get(&dep) {
                        Some(helper) => deps_of(helper),
                        None => Vec::new(),
                    };
                    on_stack.insert(dep.clone());
                    stack_path.push(dep.clone());
                    frames.push((dep, dep_deps, 0));
                }
                None => {
                    if let Some((node, _, _)) = frames.pop() {
                        on_stack.remove(&node);
                        visited.insert(node);
                    }
                    stack_path.pop();
                }
            }
        }
    }

    Ok(())
}

fn validate_expr(
    expr: &ExprDef,
    fields: &HashSet<String>,
    bindings: &HashSet<String>,
    helpers: &HashSet<String>,
    errors: &mut Vec<Error>,
) {
    match expr {
        ExprDef::Field(name) => {
            if !fields.contains(&name.to_string()) {
                errors.push(Error::new(
                    name.span(),
                    format!("unknown state field `{name}`"),
                ));
            }
        }
        ExprDef::Binding(name) => {
            if !bindings.contains(&name.to_string()) {
                errors.push(Error::new(name.span(), format!("unknown binding `{name}`")));
            }
        }
        ExprDef::Call { helper, .. } => {
            let helper_name = helper.to_string();
            if !helpers.contains(&helper_name)
                && !NATIVE_MOB_MACHINE_HELPERS.contains(&helper_name.as_str())
            {
                errors.push(Error::new(
                    helper.span(),
                    format!("unknown helper `{helper}`"),
                ));
            }
        }
        ExprDef::FieldAccess { base, .. }
        | ExprDef::EnumVariantIs { value: base, .. }
        | ExprDef::EnumStringSetPayload { value: base, .. } => {
            validate_expr(base, fields, bindings, helpers, errors);
        }
        ExprDef::Not(inner)
        | ExprDef::IsSome(inner)
        | ExprDef::IsNone(inner)
        | ExprDef::Len(inner)
        | ExprDef::MapKeys(inner)
        | ExprDef::Some(inner) => {
            validate_expr(inner, fields, bindings, helpers, errors);
        }
        ExprDef::And(exprs) | ExprDef::Or(exprs) => {
            for e in exprs {
                validate_expr(e, fields, bindings, helpers, errors);
            }
        }
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => {
            validate_expr(l, fields, bindings, helpers, errors);
            validate_expr(r, fields, bindings, helpers, errors);
        }
        ExprDef::Contains { collection, value } => {
            validate_expr(collection, fields, bindings, helpers, errors);
            validate_expr(value, fields, bindings, helpers, errors);
        }
        ExprDef::Count { collection, value } => {
            validate_expr(collection, fields, bindings, helpers, errors);
            validate_expr(value, fields, bindings, helpers, errors);
        }
        ExprDef::MapContainsKey { map, key } => {
            validate_expr(map, fields, bindings, helpers, errors);
            validate_expr(key, fields, bindings, helpers, errors);
        }
        ExprDef::MapGet { map, key }
        | ExprDef::MapGetCopied { map, key }
        | ExprDef::MapGetCloned { map, key } => {
            validate_expr(map, fields, bindings, helpers, errors);
            validate_expr(key, fields, bindings, helpers, errors);
        }
        ExprDef::ForAll {
            binding,
            over,
            body,
        }
        | ExprDef::Exists {
            binding,
            over,
            body,
        } => {
            validate_expr(over, fields, bindings, helpers, errors);
            let mut inner_bindings = bindings.clone();
            inner_bindings.insert(binding.to_string());
            validate_expr(body, fields, &inner_bindings, helpers, errors);
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            validate_expr(condition, fields, bindings, helpers, errors);
            validate_expr(then_expr, fields, bindings, helpers, errors);
            validate_expr(else_expr, fields, bindings, helpers, errors);
        }
        // Literals and phase refs don't need validation
        ExprDef::Bool(_)
        | ExprDef::U64(_)
        | ExprDef::U64Max
        | ExprDef::StringLit(_)
        | ExprDef::None
        | ExprDef::EmptySeq
        | ExprDef::EmptySet
        | ExprDef::EmptyMap
        | ExprDef::CurrentPhase
        | ExprDef::Phase(_)
        | ExprDef::NamedVariant { .. } => {}
    }
}

/// Reject non-literal arithmetic `amount` expressions.
///
/// The schema-side `Update::Increment` / `Decrement` / `MapIncrement` /
/// `MapDecrement` variants encode `amount: u64`, so only compile-time
/// literal amounts can be faithfully represented in the TLA+ model.
///
/// Rather than silently coerce non-literal amounts to `1` (the prior
/// behavior), we refuse to compile the machine. If you need a computed
/// amount, either reduce the expression to a literal in the DSL or open
/// a follow-up to lift `amount` to `Expr` in `meerkat-machine-schema`.
fn validate_arithmetic_amount(field: &syn::Ident, amount: &ExprDef, errors: &mut Vec<Error>) {
    if !matches!(amount, ExprDef::U64(_)) {
        errors.push(Error::new(
            field.span(),
            format!(
                "arithmetic amount for field `{field}` must be a u64 literal; \
                 non-literal amounts (field refs, bindings, arithmetic) are \
                 not representable in MachineSchema's `amount: u64` slot — \
                 lift to `Update::Assign` with an explicit expression, or \
                 extend `Update::Increment.amount` to `Expr` in meerkat-machine-schema \
                 and update codegen/kernel accordingly"
            ),
        ));
    }
}

fn validate_update(
    update: &UpdateDef,
    fields: &HashSet<String>,
    bindings: &HashSet<String>,
    helpers: &HashSet<String>,
    errors: &mut Vec<Error>,
) {
    match update {
        UpdateDef::Assign { field, value } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(value, fields, bindings, helpers, errors);
        }
        UpdateDef::Increment { field, amount } | UpdateDef::Decrement { field, amount } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(amount, fields, bindings, helpers, errors);
            validate_arithmetic_amount(field, amount, errors);
        }
        UpdateDef::SetInsert { field, value } | UpdateDef::SetRemove { field, value } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(value, fields, bindings, helpers, errors);
        }
        UpdateDef::MapInsert { field, key, value } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(key, fields, bindings, helpers, errors);
            validate_expr(value, fields, bindings, helpers, errors);
        }
        UpdateDef::MapRemove { field, key } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(key, fields, bindings, helpers, errors);
        }
        UpdateDef::MapIncrement { field, key, amount }
        | UpdateDef::MapDecrement { field, key, amount } => {
            if !fields.contains(&field.to_string()) {
                errors.push(Error::new(
                    field.span(),
                    format!("unknown state field `{field}`"),
                ));
            }
            validate_expr(key, fields, bindings, helpers, errors);
            validate_expr(amount, fields, bindings, helpers, errors);
            validate_arithmetic_amount(field, amount, errors);
        }
        UpdateDef::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            validate_expr(condition, fields, bindings, helpers, errors);
            for u in then_updates {
                validate_update(u, fields, bindings, helpers, errors);
            }
            for u in else_updates {
                validate_update(u, fields, bindings, helpers, errors);
            }
        }
    }
}
