use std::collections::HashSet;

use syn::Error;

use crate::ast::*;

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
            // Last rule should be a fallback (no condition)
            if let Some(last) = proj.rules.last()
                && last.condition.is_some()
                && proj.rules.len() == def.phase_enum.variants.len()
            {
                // All rules have conditions — no fallback. This may be intentional
                // but is a common mistake, so warn via the error.
                // Actually, don't error here — the unreachable! in codegen catches it.
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

        // Validate field references in guard
        if let Some(guard) = &t.guard {
            let binding_names: HashSet<_> = t
                .trigger
                .bindings
                .iter()
                .map(std::string::ToString::to_string)
                .collect();
            validate_expr(
                guard,
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
            if !helpers.contains(&helper.to_string()) {
                errors.push(Error::new(
                    helper.span(),
                    format!("unknown helper `{helper}`"),
                ));
            }
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
        ExprDef::MapGet { map, key } => {
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
        | ExprDef::StringLit(_)
        | ExprDef::None
        | ExprDef::EmptySet
        | ExprDef::EmptyMap
        | ExprDef::CurrentPhase
        | ExprDef::Phase(_)
        | ExprDef::NamedVariant { .. } => {}
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
