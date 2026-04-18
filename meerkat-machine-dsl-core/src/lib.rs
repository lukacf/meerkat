mod ast;
mod gen_dispatch;
mod gen_enums;
mod gen_phase;
mod gen_schema;
mod gen_state;
mod parse;
#[cfg(test)]
mod test_machines;
mod validate;

use proc_macro2::TokenStream;
use syn::Error;

/// Expand a `machine! { ... }` invocation into generated Rust code.
///
/// Returns the combined token stream of all generated artifacts, or
/// a compile error with span information pointing at the offending token.
/// Expand `per_phase` transitions into individual per-phase transitions.
///
/// A transition with `per_phase [Idle, Attached, Running]` is expanded into
/// three transitions: `NameIdle`, `NameAttached`, `NameRunning`, each with
/// a phase guard prepended and `to` set to the same phase (self-loop).
fn expand_per_phase(def: &mut ast::MachineDef) {
    let phase_field = def.phase_field_name().cloned();
    let mut expanded = Vec::new();
    for t in def.transitions.drain(..) {
        if let Some(phases) = &t.per_phase {
            for phase in phases {
                let phase_name = phase.to_string();
                let expanded_name =
                    syn::Ident::new(&format!("{}{}", t.name, phase_name), t.name.span());
                let phase_guard = if let Some(pf) = phase_field.as_ref() {
                    ast::ExprDef::Eq(
                        Box::new(ast::ExprDef::Field(pf.clone())),
                        Box::new(ast::ExprDef::Phase(phase.clone())),
                    )
                } else {
                    // Derived-phase: use CurrentPhase
                    ast::ExprDef::Eq(
                        Box::new(ast::ExprDef::CurrentPhase),
                        Box::new(ast::ExprDef::Phase(phase.clone())),
                    )
                };

                // Prepend phase guard as an unnamed guard; keep existing guards
                let mut combined_guards = vec![ast::GuardDef {
                    name: String::new(),
                    expr: phase_guard,
                }];
                combined_guards.extend(t.guards.iter().cloned());

                expanded.push(ast::TransitionDef {
                    name: expanded_name,
                    per_phase: None,
                    trigger: ast::TriggerDef {
                        kind: match &t.trigger.kind {
                            ast::TriggerKindDef::Input => ast::TriggerKindDef::Input,
                            ast::TriggerKindDef::Signal => ast::TriggerKindDef::Signal,
                        },
                        variant: t.trigger.variant.clone(),
                        bindings: t.trigger.bindings.clone(),
                    },
                    guards: combined_guards,
                    updates: t.updates.clone(),
                    to_phase: phase.clone(),
                    effects: t.effects.clone(),
                    span: t.span,
                });
            }
        } else {
            expanded.push(t);
        }
    }
    def.transitions = expanded;
}

/// Fix `MapRemove` → `SetRemove` for fields that are Sets.
///
/// The parser can't distinguish `.remove()` on a Set from `.remove()` on a Map
/// (both take a single argument). This pass inspects the field type and corrects
/// the Update variant.
fn fix_remove_types(def: &mut ast::MachineDef) {
    let set_fields: std::collections::HashSet<String> = def
        .state_fields
        .iter()
        .filter(|f| matches!(f.ty, ast::TypeDef::Set(_)))
        .map(|f| f.name.to_string())
        .collect();

    for t in &mut def.transitions {
        fix_updates_remove_types(&mut t.updates, &set_fields);
    }
}

fn fix_updates_remove_types(
    updates: &mut [ast::UpdateDef],
    set_fields: &std::collections::HashSet<String>,
) {
    for update in updates.iter_mut() {
        match update {
            ast::UpdateDef::MapRemove { field, key } if set_fields.contains(&field.to_string()) => {
                *update = ast::UpdateDef::SetRemove {
                    field: field.clone(),
                    value: key.clone(),
                };
            }
            ast::UpdateDef::Conditional {
                then_updates,
                else_updates,
                ..
            } => {
                fix_updates_remove_types(then_updates, set_fields);
                fix_updates_remove_types(else_updates, set_fields);
            }
            _ => {}
        }
    }
}

pub fn expand_machine(input: TokenStream) -> Result<TokenStream, Error> {
    let mut def = parse::parse_machine(input)?;
    expand_per_phase(&mut def);
    fix_remove_types(&mut def);
    validate::validate(&def)?;

    let mut output = TokenStream::new();
    output.extend(gen_state::generate(&def));
    output.extend(gen_phase::generate(&def));
    output.extend(gen_enums::generate(&def));
    output.extend(gen_dispatch::generate(&def));
    output.extend(gen_schema::generate(&def));

    Ok(output)
}
