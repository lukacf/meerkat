use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Ident;

use crate::ast::*;
use crate::gen_state::state_struct_name;

/// Controls how `self.field` references are generated.
#[derive(Clone, Copy)]
pub(crate) enum FieldPrefix {
    /// `self.state.field` — inside authority methods
    AuthorityState,
    /// `self.field` — inside state struct methods (e.g., phase())
    DirectSelf,
}

/// Generate the sealed dispatch function (apply/apply_signal).
pub fn generate(def: &MachineDef) -> TokenStream {
    let state_name = state_struct_name(def);
    let input_name = &def.inputs.name;
    let phase_name = &def.phase_enum.name;
    let effect_name = &def.effects.name;
    let machine_name = &def.name;

    let transition_name = format_ident!("{}Transition", machine_name);
    let error_name = format_ident!("{}TransitionError", machine_name);
    let authority_name = format_ident!("{}Authority", machine_name);
    let mutator_trait = format_ident!("{}Mutator", machine_name);

    let input_transitions: Vec<_> = def
        .transitions
        .iter()
        .filter(|t| matches!(t.trigger.kind, TriggerKindDef::Input))
        .collect();
    let signal_transitions: Vec<_> = def
        .transitions
        .iter()
        .filter(|t| matches!(t.trigger.kind, TriggerKindDef::Signal))
        .collect();

    let input_arms = gen_match_arms(def, &input_transitions, input_name, &error_name);

    let has_signals = !def.signals.variants.is_empty() && !signal_transitions.is_empty();
    let signal_name = &def.signals.name;
    let signal_arms = if has_signals {
        gen_match_arms(def, &signal_transitions, signal_name, &error_name)
    } else {
        TokenStream::new()
    };

    let invariant_checks: Vec<_> = def
        .invariants
        .iter()
        .map(|inv| {
            let inv_name_str = inv.name.to_string();
            let check = gen_expr(&inv.expr, FieldPrefix::AuthorityState);
            quote! {
                debug_assert!(#check, "invariant '{}' violated", #inv_name_str);
            }
        })
        .collect();

    let helpers: Vec<_> = def.helpers.iter().map(gen_helper).collect();

    let signal_method = if has_signals {
        quote! {
            pub fn apply_signal(&mut self, signal: #signal_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                let mut effects = Vec::new();

                match signal {
                    #signal_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: format!("{:?}", from_phase),
                        trigger: format!("{:?}", signal),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name { from_phase, to_phase, effects })
            }
        }
    } else {
        TokenStream::new()
    };

    quote! {
        // Type alias so DSL expressions can use `Phase::Variant`
        use #phase_name as Phase;

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct #transition_name {
            pub from_phase: #phase_name,
            pub to_phase: #phase_name,
            pub effects: Vec<#effect_name>,
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #error_name {
            NoMatchingTransition { phase: String, trigger: String },
        }

        impl std::fmt::Display for #error_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::NoMatchingTransition { phase, trigger } => {
                        write!(f, "no matching transition from phase {} for {}", phase, trigger)
                    }
                }
            }
        }

        impl std::error::Error for #error_name {}

        mod sealed {
            pub trait Sealed {}
        }

        pub trait #mutator_trait: sealed::Sealed {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name>;
        }

        pub struct #authority_name {
            pub state: #state_name,
        }

        impl sealed::Sealed for #authority_name {}

        impl #authority_name {
            pub fn new() -> Self {
                Self { state: #state_name::default() }
            }

            pub fn from_state(state: #state_name) -> Self {
                Self { state }
            }

            #(#helpers)*

            #signal_method
        }

        impl #mutator_trait for #authority_name {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                #[allow(unused_mut)]
                let mut effects = Vec::new();

                match input {
                    #input_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: format!("{:?}", from_phase),
                        trigger: format!("{:?}", input),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name { from_phase, to_phase, effects })
            }
        }
    }
}

fn gen_match_arms(
    def: &MachineDef,
    transitions: &[&TransitionDef],
    enum_name: &Ident,
    error_name: &Ident,
) -> TokenStream {
    let mut groups: Vec<(&Ident, Vec<&&TransitionDef>)> = Vec::new();
    for t in transitions {
        let variant = &t.trigger.variant;
        if let Some(group) = groups.iter_mut().find(|(v, _)| *v == variant) {
            group.1.push(t);
        } else {
            groups.push((variant, vec![t]));
        }
    }

    let arms: Vec<_> = groups
        .iter()
        .map(|(variant, transitions)| {
            let binding_pattern = if transitions[0].trigger.bindings.is_empty() {
                quote! {}
            } else {
                let bindings: Vec<_> = transitions[0].trigger.bindings.iter().collect();
                quote! { { #(#bindings),* } }
            };

            let body = gen_transition_chain(def, transitions, error_name);

            quote! {
                #enum_name::#variant #binding_pattern => {
                    #body
                }
            }
        })
        .collect();

    quote! { #(#arms)* }
}

fn gen_transition_chain(
    def: &MachineDef,
    transitions: &[&&TransitionDef],
    error_name: &Ident,
) -> TokenStream {
    if transitions.len() == 1 {
        let t = transitions[0];
        if let Some(guard) = &t.guard {
            let guard_expr = gen_expr(guard, FieldPrefix::AuthorityState);
            let body = gen_transition_body(def, t);
            let variant_str = t.trigger.variant.to_string();
            quote! {
                if #guard_expr {
                    #body
                } else {
                    return Err(#error_name::NoMatchingTransition {
                        phase: format!("{:?}", from_phase),
                        trigger: #variant_str.to_string(),
                    });
                }
            }
        } else {
            gen_transition_body(def, t)
        }
    } else {
        let mut arms = Vec::new();
        for (i, t) in transitions.iter().enumerate() {
            let body = gen_transition_body(def, t);
            if let Some(guard) = &t.guard {
                let guard_expr = gen_expr(guard, FieldPrefix::AuthorityState);
                if i == 0 {
                    arms.push(quote! { if #guard_expr { #body } });
                } else {
                    arms.push(quote! { else if #guard_expr { #body } });
                }
            } else {
                arms.push(quote! { else { #body } });
            }
        }
        let last_has_guard = transitions.last().is_none_or(|t| t.guard.is_some());
        if last_has_guard {
            let variant_str = transitions[0].trigger.variant.to_string();
            arms.push(quote! { else {
                return Err(#error_name::NoMatchingTransition {
                    phase: format!("{:?}", from_phase),
                    trigger: #variant_str.to_string(),
                });
            } });
        }
        quote! { #(#arms)* }
    }
}

fn gen_transition_body(def: &MachineDef, t: &TransitionDef) -> TokenStream {
    let prefix = FieldPrefix::AuthorityState;
    let mut stmts = Vec::new();

    for update in &t.updates {
        stmts.push(gen_update(update, prefix));
    }

    if let Some(phase_field) = def.phase_field_name() {
        let phase_enum = &def.phase_enum.name;
        let to_phase = &t.to_phase;
        stmts.push(quote! { self.state.#phase_field = #phase_enum::#to_phase; });
    }

    let effect_name = &def.effects.name;
    for effect in &t.effects {
        let variant = &effect.variant;
        if effect.fields.is_empty() {
            stmts.push(quote! { effects.push(#effect_name::#variant); });
        } else {
            let field_assigns: Vec<_> = effect
                .fields
                .iter()
                .map(|(name, value)| {
                    let val = gen_expr(value, prefix);
                    // Clone to avoid move issues with String/Vec bindings
                    quote! { #name: (#val).clone() }
                })
                .collect();
            stmts.push(quote! { effects.push(#effect_name::#variant { #(#field_assigns),* }); });
        }
    }

    quote! { #(#stmts)* }
}

/// Generate Rust expression from an ExprDef.
pub(crate) fn gen_expr(expr: &ExprDef, prefix: FieldPrefix) -> TokenStream {
    match expr {
        ExprDef::Bool(v) => quote! { #v },
        ExprDef::U64(v) => quote! { #v },
        ExprDef::StringLit(s) => quote! { #s.to_string() },
        ExprDef::None => quote! { None },
        ExprDef::Some(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { Some(#e) }
        }
        ExprDef::EmptySet => quote! { std::collections::BTreeSet::new() },
        ExprDef::EmptyMap => quote! { std::collections::BTreeMap::new() },
        ExprDef::Field(name) => match prefix {
            FieldPrefix::AuthorityState => quote! { self.state.#name },
            FieldPrefix::DirectSelf => quote! { self.#name },
        },
        ExprDef::Binding(name) => quote! { #name.clone() },
        ExprDef::CurrentPhase => match prefix {
            FieldPrefix::AuthorityState => quote! { self.state.phase() },
            FieldPrefix::DirectSelf => quote! { self.phase() },
        },
        ExprDef::Phase(variant) => {
            quote! { Phase::#variant }
        }
        ExprDef::NamedVariant { enum_name, variant } => {
            quote! { #enum_name::#variant }
        }
        ExprDef::Not(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { !(#e) }
        }
        ExprDef::And(exprs) => {
            let parts: Vec<_> = exprs.iter().map(|e| gen_expr(e, prefix)).collect();
            quote! { (#(#parts)&&*) }
        }
        ExprDef::Or(exprs) => {
            let parts: Vec<_> = exprs.iter().map(|e| gen_expr(e, prefix)).collect();
            quote! { (#(#parts)||*) }
        }
        ExprDef::Eq(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left == #right) }
        }
        ExprDef::Neq(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left != #right) }
        }
        ExprDef::Gt(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left > #right) }
        }
        ExprDef::Gte(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left >= #right) }
        }
        ExprDef::Lt(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left < #right) }
        }
        ExprDef::Lte(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left <= #right) }
        }
        ExprDef::Add(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left + #right) }
        }
        ExprDef::Sub(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left - #right) }
        }
        ExprDef::Contains { collection, value } => {
            let coll = gen_expr(collection, prefix);
            let val = gen_expr(value, prefix);
            quote! { #coll.contains(&#val) }
        }
        ExprDef::Len(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.len() }
        }
        ExprDef::MapGet { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k) }
        }
        ExprDef::MapKeys(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.keys().cloned().collect::<std::collections::BTreeSet<_>>() }
        }
        ExprDef::IsSome(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.is_some() }
        }
        ExprDef::IsNone(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.is_none() }
        }
        ExprDef::ForAll {
            binding,
            over,
            body,
        } => {
            let over_e = gen_expr(over, prefix);
            let body_e = gen_expr(body, prefix);
            quote! { #over_e.iter().all(|#binding| #body_e) }
        }
        ExprDef::Exists {
            binding,
            over,
            body,
        } => {
            let over_e = gen_expr(over, prefix);
            let body_e = gen_expr(body, prefix);
            quote! { #over_e.iter().any(|#binding| #body_e) }
        }
        ExprDef::Call { helper, args } => {
            let arg_exprs: Vec<_> = args
                .iter()
                .map(|a| {
                    let e = gen_expr(a, prefix);
                    quote! { &#e }
                })
                .collect();
            quote! { Self::#helper(#(#arg_exprs),*) }
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            let cond = gen_expr(condition, prefix);
            let then_e = gen_expr(then_expr, prefix);
            let else_e = gen_expr(else_expr, prefix);
            quote! { if #cond { #then_e } else { #else_e } }
        }
    }
}

/// Convenience alias for authority-context expressions.
#[allow(dead_code)]
pub(crate) fn gen_guard_expr(expr: &ExprDef) -> TokenStream {
    gen_expr(expr, FieldPrefix::AuthorityState)
}

fn gen_update(update: &UpdateDef, prefix: FieldPrefix) -> TokenStream {
    match update {
        UpdateDef::Assign { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field = #val; },
                FieldPrefix::DirectSelf => quote! { self.#field = #val; },
            }
        }
        UpdateDef::Increment { field, amount } => {
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field += #amt; },
                FieldPrefix::DirectSelf => quote! { self.#field += #amt; },
            }
        }
        UpdateDef::Decrement { field, amount } => {
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field -= #amt; },
                FieldPrefix::DirectSelf => quote! { self.#field -= #amt; },
            }
        }
        UpdateDef::SetInsert { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.insert(#val); },
                FieldPrefix::DirectSelf => quote! { self.#field.insert(#val); },
            }
        }
        UpdateDef::SetRemove { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.remove(&#val); },
                FieldPrefix::DirectSelf => quote! { self.#field.remove(&#val); },
            }
        }
        UpdateDef::MapInsert { field, key, value } => {
            let k = gen_expr(key, prefix);
            let v = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.insert(#k, #v); },
                FieldPrefix::DirectSelf => quote! { self.#field.insert(#k, #v); },
            }
        }
        UpdateDef::MapRemove { field, key } => {
            let k = gen_expr(key, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.remove(&#k); },
                FieldPrefix::DirectSelf => quote! { self.#field.remove(&#k); },
            }
        }
        UpdateDef::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            let cond = gen_expr(condition, prefix);
            let then_stmts: Vec<_> = then_updates.iter().map(|u| gen_update(u, prefix)).collect();
            let else_stmts: Vec<_> = else_updates.iter().map(|u| gen_update(u, prefix)).collect();
            if else_stmts.is_empty() {
                quote! { if #cond { #(#then_stmts)* } }
            } else {
                quote! { if #cond { #(#then_stmts)* } else { #(#else_stmts)* } }
            }
        }
    }
}

fn gen_helper(helper: &HelperDef) -> TokenStream {
    let name = &helper.name;
    let return_ty = crate::gen_state::gen_type(&helper.return_ty);
    let body = gen_expr(&helper.body, FieldPrefix::AuthorityState);

    let params: Vec<_> = helper
        .params
        .iter()
        .map(|p| {
            let pname = &p.name;
            let pty = crate::gen_state::gen_type(&p.ty);
            quote! { #pname: &#pty }
        })
        .collect();

    if helper.params.is_empty() {
        quote! {
            fn #name(&self) -> #return_ty {
                #body
            }
        }
    } else {
        quote! {
            fn #name(#(#params),*) -> #return_ty {
                #body
            }
        }
    }
}
